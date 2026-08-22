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
///
/// **It also reports whether the user walked away**, which is the whole reason
/// `away_lock` needed no window and no window procedure of its own. Both
/// messages it cares about (`WM_WTSSESSION_CHANGE` after
/// `WTSRegisterSessionNotification`, and the broadcast `WM_POWERBROADCAST`)
/// are addressed to a window on THIS thread, so they land in this very queue
/// and pass through this loop before being dispatched. Subclassing a
/// dependency's hidden window to see them -- and fighting `muda`'s own menu
/// subclass for the privilege -- would have been a second way of reading a
/// queue already being read here.
///
/// The messages are still translated and dispatched exactly as before:
/// observing one is not consuming it, and `DefWindowProcW` on the tray's
/// window is what Windows expects to see them.
///
/// The FIRST away event in a drain is the one reported; a drain that saw both
/// a lock and a suspend has one answer to give and they lead to the same lock.
#[must_use = "a dropped away event is a vault that stays unlocked after Win+L"]
pub fn pump_windows_messages() -> Option<crate::away_lock::AwayEvent> {
    let mut msg = MSG::default();
    let mut away = None;
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
            away = away.or_else(|| crate::away_lock::away_event(msg.message, msg.wParam.0));
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    away
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
#[allow(clippy::too_many_arguments)]
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
    choice: FillChoice,
    notifier: &dyn sequence::Notifier,
    reprompt: &mut Reprompt<'_>,
) {
    fill_from_vault_with(
        cache,
        injector,
        fill_stats,
        item_id,
        hwnd,
        choice,
        notifier,
        &crate::vault_window::preflight::SendGate::production(),
        reprompt,
    )
}

/// **Where the fill path's master-password re-prompt is scoped, and what it
/// remembers.**
///
/// Three things travel together because none of them means anything without
/// the other two, and passing them as three arguments through
/// [`handle_match`] and `main`'s event loop is three chances to hand one of
/// them a placeholder:
///
/// * `account` -- **the reason this type exists.** [`crate::reprompt::permit`]
///   proves presence with a Windows Hello gesture, and a gesture is verified
///   *for an account*: [`crate::reprompt::gate_from`] answers
///   [`crate::reprompt::RepromptGate::unprovable`] for `None`, and an
///   unprovable gate refuses **every** protected item. So a fill that could
///   not name its account would not be gated, it would be broken -- refused
///   for every enrolled user. `app` has never received an account id; `main`
///   has one, and this is what carries it down.
/// * `proof` -- borrowed, never owned, so that the
///   [`crate::reprompt::PROOF_LASTS`] window is the *same* window across
///   consecutive fills. A `Proof::default()` constructed per fill would ask
///   for a gesture on every single one, which is the friction the sixty
///   seconds exist to avoid.
/// * `gate_for` -- the seam. Building the production gate calls
///   [`crate::hello::state_for`], a WinRT round trip, and satisfying it opens
///   a real Hello dialog on the desktop of whoever ran `cargo test`. This is
///   the same `fn` pointer shape `vault_window::VaultDeps::reprompt` uses and
///   for the same reason recorded there: an `impl Fn` seam could be handed a
///   wrapper that the identity pin could not see.
pub struct Reprompt<'a> {
    /// The account whose Hello enrollment a proof would be taken against.
    ///
    /// Owned rather than borrowed, so the borrow of `main`'s estate that
    /// produced it ends at the call and cannot fight the `&mut` on the proof.
    account: Option<crate::accounts::AccountId>,
    /// The most recent proof, owned by the loop that outlives any one fill.
    proof: &'a mut crate::reprompt::Proof,
    /// [`crate::reprompt::gate_for_account`] in production, a fixture in a
    /// test.
    gate_for: fn(Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate,
}

impl Reprompt<'_> {
    /// The same, with the gate substituted. Test-only, so no shipped path can
    /// hand itself a prover that always says yes.
    #[cfg(test)]
    pub(crate) fn with_gate_for(
        proof: &mut crate::reprompt::Proof,
        gate_for: fn(Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate,
    ) -> Reprompt<'_> {
        Reprompt {
            account: crate::accounts::AccountId::parse("0123456789abcdef0123456789abcdef"),
            proof,
            gate_for,
        }
    }
}

/// **The fill path's proof of presence, and the account it was taken for.**
///
/// Held by `main`'s event loop, because the [`crate::reprompt::PROOF_LASTS`]
/// window has to outlive any one fill for it to mean anything -- a
/// `Proof::default()` built per fill would ask for a gesture every time.
///
/// The second field is the part that is not decoration. The gate is rebuilt
/// per fill from whichever account is active *now*, but the proof is not: an
/// account switch inside the window would otherwise let a gesture given for
/// account A cover a protected item belonging to account B, which is precisely
/// the confusion [`crate::reprompt::Scope`] exists to prevent one level down.
/// [`Self::scoped_to`] is the only way to reach the proof, and it forgets it
/// the moment the account it was taken for is not the account being filled
/// from -- the same rule `Proof::forget` states for a lock.
#[derive(Debug, Default)]
pub struct FillProof {
    proof: crate::reprompt::Proof,
    taken_for: Option<crate::accounts::AccountId>,
}

impl FillProof {
    /// A [`Reprompt`] for `account`, with any proof taken for a *different*
    /// account already forgotten.
    pub fn scoped_to(&mut self, account: Option<&crate::accounts::AccountId>) -> Reprompt<'_> {
        if self.taken_for.as_ref() != account {
            self.proof.forget();
            self.taken_for = account.cloned();
        }
        Reprompt {
            account: account.cloned(),
            proof: &mut self.proof,
            gate_for: crate::reprompt::gate_for_account,
        }
    }
}

/// **Whether this item's secrets may be typed at all**, asked once, before
/// anything is fetched, rendered, shown or sent.
///
/// The shape is `vault_window`'s `permitted`/`gated_command`: the decision
/// goes through [`crate::reprompt::permit`] -- the one public way to combine
/// [`crate::reprompt::need`] with an action -- and the caller acts only on a
/// `true`. It is *not* a second decision path; it is the same one, at the fill.
///
/// # Why it is the first thing in the arm
///
/// Below this line the fill splits in two -- [`FillAction::Default`]'s UI
/// Automation fill and [`FillAction::Sequence`]'s typed plan -- and a gate on
/// either arm alone leaves the other ungated. Worse, a gate placed *inside*
/// the sequence arm would sit after `get_totp`, after `credentials_for`, and
/// after the preflight had already put the target, the username and a
/// "Copy instead" button carrying the plaintext password on screen. Refusing
/// there is refusing after the exposure.
///
/// This position also answers the "no partial fill" requirement by
/// construction rather than by care: nothing has been typed when it returns
/// `false`, because nothing has been *resolved*.
///
/// # The gate is built only for a protected item
///
/// [`crate::reprompt::gate_for_account`] probes Windows Hello, and
/// [`crate::reprompt::need`] answers [`crate::reprompt::Need::Nothing`] for an
/// unprotected item whatever `can_prove` says -- proved by
/// `an_unprotected_item_never_asks_for_anything`. So an unprotected fill pays
/// for no WinRT round trip, and `unprovable()` in that branch cannot change
/// any answer.
///
/// # The refusal is spoken, not swallowed
///
/// Through the same [`sequence::Notifier`] the two other refusals on this path
/// already use, for the reason recorded on them: this path routinely runs with
/// no window of ours in front of the user, so a silent no-op is
/// indistinguishable from a hotkey that never registered -- and here the user
/// would conclude that autofill is broken rather than that the item they
/// themselves ticked "master password re-prompt" on is doing what they asked.
fn permitted_by_reprompt(
    reprompt: &mut Reprompt<'_>,
    item: &VaultItem,
    item_id: &str,
    notifier: &dyn sequence::Notifier,
) -> bool {
    let protected = crate::vault_bridge::reprompt_protected(item);
    let gate = if protected {
        (reprompt.gate_for)(reprompt.account.as_ref())
    } else {
        crate::reprompt::RepromptGate::unprovable()
    };
    let outcome =
        crate::reprompt::permit(&gate, protected, reprompt.proof, std::time::Instant::now(), || {});
    if outcome.happened() {
        return true;
    }
    let message = crate::reprompt::refusal_text(matches!(outcome, crate::reprompt::Outcome::Cannot));
    log::warn!("a master-password re-prompt withheld a fill of item {item_id}: {message}");
    notifier.refused(message);
    false
}

/// [`fill_from_vault`] with the preflight's foreground lookup **injected**.
///
/// The lookup is a live Win32 + COM round trip, so a test that reached the
/// production gate would either be asking the machine it runs on where the
/// mouse is, or -- worse -- would pass because that machine happened to answer
/// the way the test wanted. Handing the gate in is what lets the fill be
/// driven end to end with a foreground that is a value.
///
/// The split is `updater::apply_update`/`apply_update_with`'s, for the reason
/// recorded there: the routing question -- does the sender still get asked? --
/// is the one a pin on a pure decision cannot answer.
#[allow(clippy::too_many_arguments)]
pub fn fill_from_vault_with<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
    choice: FillChoice,
    notifier: &dyn sequence::Notifier,
    gate: &crate::vault_window::preflight::SendGate,
    reprompt: &mut Reprompt<'_>,
) {
    // `get_by_id`, not `items()`: the answer is one item, and cloning the
    // whole vault to find it put 5.66 MB and 46,494 allocations -- 5.6-9.4 ms
    // over a realistic vault -- between the keypress and the password. The
    // miss arm below is unchanged and still reached, because `get_by_id`
    // answers only from the snapshot and returns `None` for an id it does not
    // hold, exactly as the `find` did.
    let item = cache
        .get_by_id(item_id)
        .map(Ok)
        .unwrap_or_else(|| {
            log::warn!("cache miss for item {item_id} during a fill; falling back to bw serve");
            cache.bridge().get_item(item_id)
        });
    match item {
        Ok(item) => {
            // **The master-password re-prompt, ahead of everything.** See
            // [`permitted_by_reprompt`] for why this line and not one inside
            // either arm below, and why it is deliberately ABOVE the
            // preflight rather than beside it.
            if !permitted_by_reprompt(reprompt, &item, item_id, notifier) {
                return;
            }
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
                    // **The preflight gate, in the position that gates.** See
                    // [`preflight_guard_for`] for which fills it speaks for and
                    // `vault_window::preflight::dispatch_with` for why the gate
                    // is a function behind a seam rather than an `if` here.
                    // **No `record_fill` on the `Ok` arm.** `fill_sequence`
                    // returns as soon as the typing thread has been *started*,
                    // so `Ok(())` cannot tell a sequence that typed a password
                    // from one that refused before the first keystroke or was
                    // abandoned when the user alt-tabbed -- and counting the
                    // latter two floats an item that never filled to the top
                    // of the picker. The sink is what records, from the thread
                    // that knows.
                    let sink = fill_outcome_sink(fill_stats, item_id);
                    let rule_image = crate::vault_bridge::extract_app_match(&item).map(|m| m.process);
                    let guard = preflight_guard_for(&choice, rule_image.as_deref());
                    // **The 4b surface, hosted.** It runs BEFORE the gate and
                    // never instead of it: its only affirmative answer lets
                    // this arm go on and call `dispatch_with`, which describes
                    // the foreground again and refuses on its own terms. See
                    // `preflight::SendGate::confirm` for why that ordering is
                    // what keeps the gate's mutation measurement intact.
                    if !confirmed_by_preflight(gate, guard, &item, &choice, totp.as_deref()) {
                        return;
                    }
                    let gated = crate::vault_window::preflight::dispatch_with(
                        gate,
                        guard,
                        || injector.fill_sequence(hwnd, plan, sink),
                    );
                    let sent = match gated {
                        crate::vault_window::preflight::Gated::Sent(result) => result,
                        crate::vault_window::preflight::Gated::Refused(why) => {
                            let message =
                                crate::vault_window::preflight::refusal_notice(Some(why));
                            log::warn!("preflight refused a fill of item {item_id}: {message}");
                            notifier.refused(&message);
                            return;
                        }
                        crate::vault_window::preflight::Gated::NoTarget => {
                            let message = crate::vault_window::preflight::refusal_notice(None);
                            log::warn!("preflight could not describe the foreground; not filling");
                            notifier.refused(&message);
                            return;
                        }
                    };
                    if let Err(e) = sent {
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

/// **Which fills the preflight speaks for**, as a pure function so the scope
/// of the gate is a thing with its own tests rather than a condition buried in
/// [`fill_from_vault`].
///
/// It speaks for a fill that types **a bare secret into whatever holds
/// focus**: [`FillChoice::Just`] of a password or a one-time code. That is
/// precisely the case section 4b is written about, and it is the case where
/// "the focused control is masked" is the right question -- the user put their
/// caret in the password box and asked for the password.
///
/// It deliberately does **not** speak for [`FillChoice::UserTabPass`] or
/// [`FillChoice::Saved`], and the reason is not caution, it is arithmetic:
/// both of those start by typing a username into a field that is not masked,
/// so `preflight::verdict`'s `NotMasked` arm would refuse every one of them.
/// The design's own 4b illustration is such a sequence (Ctrl+A, the address,
/// Tab, the password, Enter) shown in the ALLOWED state, so applying the
/// masking rule to a multi-step sequence would contradict the picture it comes
/// from. That conflict is recorded rather than papered over.
pub fn preflight_guard_for<'a>(
    choice: &FillChoice,
    rule_image: Option<&'a str>,
) -> crate::vault_window::preflight::Guard<'a> {
    match choice {
        FillChoice::Just(key_sequence::FieldRef::Password)
        | FillChoice::Just(key_sequence::FieldRef::Totp) => {
            crate::vault_window::preflight::Guard::Preflight { rule_image }
        }
        FillChoice::Just(_) | FillChoice::UserTabPass | FillChoice::Saved => {
            crate::vault_window::preflight::Guard::NotRequired
        }
    }
}

/// **Asks the hosted 4b confirmation, and answers whether the fill may go on
/// to ask the gate.**
///
/// Three ways this answers `true`, and each is deliberate:
///
/// 1. [`crate::vault_window::preflight::Guard::NotRequired`] -- the fill is
///    not one the preflight speaks for (see [`preflight_guard_for`]), so no
///    window is opened and nothing is asked. Putting a modal in front of every
///    `UserTabPass` fill would make the app unusable, and it would ask a
///    question about masking that those fills deliberately do not answer.
/// 2. The foreground could not be described. **Nothing is confirmed and
///    nothing is sent**: `dispatch_with` is still called, sees the same
///    `None`, and answers `Gated::NoTarget`, which is the arm that tells the
///    user. Returning `true` here rather than short-circuiting keeps the
///    reporting of an undescribable foreground in exactly one place.
/// 3. The user completed the hold.
///
/// Everything else -- Esc, Cancel, Dismiss, "Copy instead", the window closed
/// with the X, a second preflight already open -- answers `false`, and a
/// `false` types nothing.
///
/// # It is not the gate
///
/// A `true` from here is permission to *ask*, not permission to type.
/// `dispatch_with` runs immediately after and makes its own observation. That
/// is what lets this be hosted without weakening the measurement the refusal
/// arms carry: the routing tests drive
/// [`crate::vault_window::preflight::SendGate::describing`], whose
/// confirmation always says `Send`, so what they see is the gate on its own.
fn confirmed_by_preflight(
    gate: &crate::vault_window::preflight::SendGate,
    guard: crate::vault_window::preflight::Guard<'_>,
    item: &VaultItem,
    choice: &FillChoice,
    totp: Option<&str>,
) -> bool {
    let crate::vault_window::preflight::Guard::Preflight { rule_image } = guard else {
        return true;
    };
    let Some(target) = gate.describe() else {
        return true;
    };
    // No rule recorded means no process claim to show, so the surface says the
    // target claims itself -- the same `None` reading `dispatch_with` makes,
    // spelled the same way so the two cannot disagree about what the user was
    // shown and what was then checked.
    let claim = rule_image.unwrap_or(target.image_name.as_str()).to_string();

    let (username, password) = credentials_for(item);
    let password = zeroize::Zeroizing::new(password);
    // The step rows are built from this, and `step_rows(.., false)` writes the
    // mask for a secret in a branch whose `else` is the only one that can
    // resolve a value -- so nothing borrowed here can reach the screen.
    let totp_state = crate::vault_window::detail::TotpState::NoSecret;
    let source = crate::key_sequence::ResolveSource {
        username: &username,
        password: password.as_str(),
        custom: crate::key_sequence::custom_pairs(item),
        totp: &totp_state,
    };
    let sequence = match choice {
        FillChoice::Just(field) => {
            crate::key_sequence::render(&[crate::key_sequence::Token::Field(field.clone())])
        }
        FillChoice::UserTabPass | FillChoice::Saved => sequence_for(item),
    };
    // "Copy instead" is an escape from typing, not from the vault: it is the
    // very value this fill was going to type, and it is the only secret the
    // window is handed. `Zeroizing` so the window's exit wipes it.
    let copy = zeroize::Zeroizing::new(match choice {
        FillChoice::Just(key_sequence::FieldRef::Totp) => totp.unwrap_or_default().to_string(),
        _ => password.to_string(),
    });

    let state =
        crate::vault_window::preflight::PreflightState::new(target, &claim, &sequence, &source);
    match gate.confirm(state, copy) {
        Some(crate::vault_window::preflight::PreflightAction::Send) => true,
        answered => {
            log::info!("the preflight was not confirmed ({answered:?}); nothing was typed");
            false
        }
    }
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

/// Whether the match engine recognised the foreground window.
///
/// A two-variant enum rather than the `Option<(String, AppMatch)>` the engine
/// answers, because [`disposition`] must be callable with no vault, no cache
/// and no `AppMatch` -- and because the id is the only part of the engine's
/// answer the decision uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched<'a> {
    /// Nothing in the vault claims this window.
    No,
    /// This vault item id claims it.
    Yes(&'a str),
}

/// Whether the foreground window contains a masked field, as
/// [`crate::injector::ui_automation::window_has_password_field`] reports it.
///
/// **`Unknown` is a variant and not a `false`.** UI Automation can fail to
/// answer -- no apartment, a window that closed between the focus event and the
/// probe, a provider that exposes nothing -- and it is also what the *matched*
/// path passes, because that path never asks (see [`disposition`]'s note on
/// laziness). Folding either case into `No` would be a claim the code did not
/// make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HasPasswordField {
    /// A masked field was found.
    Yes,
    /// The window was read and has no masked field.
    No,
    /// The question was not asked, or could not be answered.
    Unknown,
}

/// Whether this process can currently answer questions about the vault's
/// contents at all.
///
/// **The third input to [`disposition`], and it is there to stop the no-match
/// card lying.** `main`'s `stand_down_after_unlock` calls
/// `MatchEngine::clear`, and its own log line says what that means: "the app
/// matches are cleared too, so nothing can prompt to autofill until they are
/// rebuilt". Every lock, every declined re-authentication and every failed
/// backend restart goes through it. So with the vault locked
/// `MatchEngine::lookup` answers `None` for *every* window -- including every
/// window that does have a saved login -- and without this input `disposition`
/// read that silence as [`Matched::No`] and put "No saved login for <app>" on
/// screen. That is a false statement about the user's own vault, from the one
/// surface whose entire purpose is to be trusted about it.
///
/// **Read from [`crate::vault_cache::VaultCache::is_populated`]**, which is the
/// honest predicate for exactly this question: it is set by a successful
/// populate and cleared by `VaultCache::clear`, the call every lock path makes
/// beside `engine.clear()`. It is deliberately *not* "the engine is empty" --
/// a user whose vault genuinely holds no app matches has an empty engine and a
/// populated cache, and for them "No saved login for <app>" is true, useful,
/// and exactly what 3a was built to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultAvailability {
    /// The vault has been read into this process and not thrown away, so
    /// "nothing matched" is a fact about the vault.
    Readable,
    /// Locked, stood down, or never opened: "nothing matched" is a fact about
    /// this process and says nothing whatever about the vault.
    Locked,
}

/// [`VaultAvailability`] from [`crate::vault_cache::VaultCache::is_populated`].
///
/// **A named function over a `bool`, not an `if` at the call site.** The call
/// site is inside `main`'s event loop, which no test reaches, and the whole
/// defect this corrects was a decision made where nothing could observe it.
/// Here the mapping is one line a test can execute in both directions
/// (`a_populated_cache_is_readable_and_an_empty_one_is_locked`), and the only
/// thing left unreachable is the `cache.is_populated()` that feeds it.
///
/// **The polarity is the whole of it.** Inverted, every locked vault would
/// report `Readable` and the card would go back to claiming there is no saved
/// login for apps that have one -- and every unlocked vault would say
/// "Deskwarden is locked" while it plainly was not.
pub fn vault_availability(populated: bool) -> VaultAvailability {
    if populated {
        VaultAvailability::Readable
    } else {
        VaultAvailability::Locked
    }
}

/// Whether the user has said *Never for this app* about the window in hand.
///
/// **The fourth input to [`disposition`]**, and it is an input rather than a
/// check somewhere downstream because a "never" that still shows the card is
/// not a never. The control it feeds is per-app, so it is computed for *this*
/// window by [`never_for_app`] and passed in; `disposition` itself never sees
/// a list and so cannot silence the wrong app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeverForApp {
    /// This app is on the list: the overlay's unmatched states stay shut.
    Yes,
    /// It is not.
    No,
}

/// [`NeverForApp`] for `app`, from
/// [`crate::settings::Settings::never_save_for_apps`].
///
/// **A pure function over the list, and the place "a different app is
/// unaffected" is checked.** The whole risk of a per-app silence is that it is
/// not per-app: a substring match, a normalisation that collapses two names, or
/// a list read with the wrong index, and one *Never* silences the overlay
/// everywhere. `a_never_for_one_app_leaves_every_other_app_alone` is the test
/// that holds it, and the comparison here is whole-string equality --
/// deliberately not `contains`, not a prefix, not a path fragment.
///
/// It is **ASCII-case-insensitive**, because the string is
/// [`window_label`]'s answer: Windows hands the same executable back as
/// `Tracker.exe` and `tracker.exe` depending on how it was launched, and a user
/// who silenced one has silenced the app rather than a spelling. It is not
/// Unicode-case-folded: the fold is locale-dependent, and a folding that
/// silenced a *different* app is worse than one that failed to silence the
/// same one twice.
pub fn never_for_app(never: &[String], app: &str) -> NeverForApp {
    if never.iter().any(|a| a.eq_ignore_ascii_case(app)) {
        NeverForApp::Yes
    } else {
        NeverForApp::No
    }
}

/// Whether the user wants the overlay to raise itself at all.
///
/// **The fifth input to [`disposition`], and the one whose absence was the
/// defect.** [`crate::settings::Settings::prompt_on_match`] reached exactly
/// one decision -- [`match_disposition`], on the *matched* path -- so turning
/// the prompt off in Preferences silenced the overlay for the apps the user
/// had saved a login for, and left the no-match card (3a) and the locked card
/// (3b) appearing for the apps they had not. That is backwards: the states the
/// setting silenced were the useful ones.
///
/// **So the setting is the master switch for every prompt the overlay raises
/// by itself**, matched or not. It is one switch because that is what the user
/// means by "disabled in settings", and because a second switch for the
/// unmatched cards would leave a user who had already turned the prompt off
/// still being prompted until they found it -- which is today's complaint with
/// an extra step.
///
/// **It is emphatically not an autofill switch, and the split enforcement is
/// what keeps that true.** `Silenced` suppresses the two *unmatched* cards
/// here; the matched card is suppressed one layer down, by
/// [`match_disposition`] inside [`handle_match`], which is also where
/// [`match_arms_hotkey`] arms `CTRL+ALT+B` for the match **either way**.
/// Suppressing [`Open::Match`] in this function instead would have taken the
/// hotkey arming with it, so off would have meant autofill off entirely -- the
/// opposite of the fallback the setting is documented to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPrompts {
    /// The user leaves Deskwarden free to raise a card on its own.
    Shown,
    /// The user turned the prompt off: nothing opens without them asking.
    Silenced,
}

/// [`OverlayPrompts`] from [`crate::settings::Settings::prompt_on_match`].
///
/// A named function over the `bool` for the reason [`vault_availability`]
/// gives: the only place the field could otherwise be read is inside `main`'s
/// event loop, where no test can watch the mapping, and a decision made where
/// nothing can observe it is this crate's signature defect.
pub fn overlay_prompts(prompt_on_match: bool) -> OverlayPrompts {
    if prompt_on_match {
        OverlayPrompts::Shown
    } else {
        OverlayPrompts::Silenced
    }
}

/// **The image names this build recognises as a web browser, and the single
/// source of that answer.**
///
/// One list, read by one function ([`browser_window`]), consulted at one call
/// site. This crate keeps finding defects in the shape where two enumerations
/// must agree, so there is deliberately no second copy: no per-browser
/// behaviour table, no parallel list in `main`, no string literal anywhere
/// else. `the_browser_list_is_the_only_place_a_browser_is_named` holds that.
///
/// Compared whole and ASCII-case-insensitively against
/// `window_watch::ForegroundEvent::exe_name`, exactly as
/// [`never_for_app`] compares its own list, and for the same reason: Windows
/// hands the same executable back with different capitalisation depending on
/// how it was launched.
///
/// **A browser that is not on this list is treated as an ordinary app**, and
/// that is the deliberate failure direction. The unrecognised browser gets the
/// behaviour every native app gets today -- one no-match card, with a *Never
/// for this app* button on it that silences it permanently (see
/// [`NeverForApp`]). The opposite default -- guessing at browser-ness from
/// something softer than the image name -- would silence native apps the user
/// does want the card for, and a card that never appears cannot be turned back
/// on from the card.
///
/// **UI Automation was considered for this and rejected.** It can report a
/// window's control pattern and framework, and Chromium and Gecko windows do
/// identify as such -- but so does every Electron and WebView2 app on the
/// desktop, which are native apps whose credentials genuinely are keyed on the
/// executable and which must keep the card. The honest question is not "does
/// this window render HTML" but "does this app own credentials per site rather
/// than per executable", and no automation property answers that. A name list
/// answers it exactly, is wrong only in the recoverable direction above, and
/// costs no cross-process call on a path that already pays for one.
pub const BROWSER_IMAGE_NAMES: &[&str] = &[
    "brave.exe",
    "chrome.exe",
    "chromium.exe",
    "firefox.exe",
    "floorp.exe",
    "iexplore.exe",
    "librewolf.exe",
    "msedge.exe",
    "opera.exe",
    "opera_gx.exe",
    "thorium.exe",
    "vivaldi.exe",
    "waterfox.exe",
    "zen.exe",
];

/// Whether the foreground window belongs to a web browser.
///
/// **The sixth input to [`disposition`]**, and it exists because the no-match
/// card is structurally unable to be right in a browser. Every login page has
/// a password field, so the probe answers `Yes` every time; and Deskwarden's
/// match table is keyed on **executables** while a browser's credentials
/// belong to **sites**, so the vault will almost never match. The card is
/// therefore near-permanent noise there -- "No saved login for <the browser>"
/// is both true and useless, because a login saved for a site would not make
/// it stop.
///
/// **It suppresses 3a and 3b only.** A browser the user has deliberately
/// matched to a vault item still raises the fill card: they wrote that rule by
/// hand against that executable, and refusing to honour it would be this
/// function deciding it knows better than an explicit instruction. See
/// [`disposition`]'s `Matched::Yes` arm, which consults none of this.
///
/// **There is no preference to override it**, and that is the answer rather
/// than an omission. The card it suppresses ends in *Save this login for
/// <app>* -- an app-keyed rule -- which is the one thing that cannot be right
/// for a browser, so an override would only re-enable a card whose offer is
/// wrong. The per-app control that does exist ([`NeverForApp`]) can only add
/// silence; this is the same direction, decided once instead of by every user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserWindow {
    /// [`BROWSER_IMAGE_NAMES`] contains this image name.
    Yes,
    /// It does not -- an ordinary native app, as far as this build can tell.
    No,
}

/// [`BrowserWindow`] for one image name, against [`BROWSER_IMAGE_NAMES`].
///
/// **The argument is the executable file name, not [`window_label`]'s
/// answer.** The label falls back to the window *title* for a frame it cannot
/// attribute, and titles are attacker- and user-controlled text: a document
/// named after a browser's executable, open in an editor, would otherwise
/// silence that editor's card.
/// The image name is the only part of a window that names the program.
pub fn browser_window(exe_name: &str) -> BrowserWindow {
    if BROWSER_IMAGE_NAMES.iter().any(|b| b.eq_ignore_ascii_case(exe_name)) {
        BrowserWindow::Yes
    } else {
        BrowserWindow::No
    }
}

/// The apps this process has been told *Never* about.
///
/// Seeded once from `settings.json` and appended to by
/// [`remember_never_for_app`], because `main` holds a `Settings` loaded at
/// startup and never refreshed -- so a *Never* chosen at 10am would otherwise
/// not take effect until the app was restarted, which is the one behaviour
/// this control must not have.
///
/// **Nothing in this crate's tests touches it.** Every decision it feeds is a
/// pure function over a slice ([`never_for_app`]) or over a closure
/// ([`route_save_answer`]); this is the process-wide *cache* in front of those,
/// and reading it lazily loads `settings.json` from `%APPDATA%`, which no test
/// may do.
static NEVER_APPS: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// [`NEVER_APPS`], loading it from `settings.json` the first time.
///
/// Unreachable from a test by design; see [`NEVER_APPS`].
pub fn never_apps() -> Vec<String> {
    let mut held = NEVER_APPS.lock().unwrap_or_else(|e| e.into_inner());
    held.get_or_insert_with(|| {
        crate::settings::default_path()
            .map(|p| crate::settings::Settings::load(&p).never_save_for_apps)
            .unwrap_or_default()
    })
    .clone()
}

/// Records `app` as one the user never wants to be asked about again -- in
/// this process, and on disk.
///
/// **Both halves, and in that order.** The in-memory half is what makes the
/// *next* foreground event silent; the disk half is what makes the one after
/// the next restart silent. Writing only the file would leave the card
/// appearing for the rest of the session immediately after the user pressed
/// the control that says it will not.
///
/// A failed write is logged and not otherwise acted on: the user's answer
/// still holds for this session, which is strictly better than dropping it,
/// and there is no surface on a frameless always-on-top card to report a
/// settings-file failure on.
fn remember_never_for_app(app: &str) {
    {
        let mut held = NEVER_APPS.lock().unwrap_or_else(|e| e.into_inner());
        let list = held.get_or_insert_with(Vec::new);
        if !list.iter().any(|a| a.eq_ignore_ascii_case(app)) {
            list.push(app.to_string());
        }
    }
    if let Some(path) = crate::settings::default_path() {
        if let Err(e) = crate::settings::Settings::persist_never_save_for_app(&path, app) {
            log::warn!("could not record `never for {app}` in {}: {e}", path.display());
        }
    }
}

/// What focusing this window should put on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open<'a> {
    /// The matched-item card (design 2a), for this item id.
    Match(&'a str),
    /// The no-match card (design 3a).
    NoMatch,
    /// The locked card (design 3b) -- a window that asks for a password while
    /// this process cannot read the vault.
    Locked,
    /// Nothing at all -- the silence the app has always kept.
    Nothing,
}

/// **The whole of the "does the overlay open, and as what" decision, as a pure
/// function.**
///
/// Until this existed the overlay had exactly one trigger -- the match engine
/// said yes -- and "the vault has nothing for this window" was indistinguishable
/// from "Deskwarden is broken", because both were silence. This adds the second
/// question and keeps the answer in one place nothing needs a window to reach.
///
/// **The control that stops this becoming "always open" is the third arm.** An
/// ordinary window -- a text editor, a file manager, a browser on a page with
/// no login -- matches nothing and has no masked field, and it must still be
/// silent. A card that appears over every window the vault does not know is
/// not a feature, it is a popup. `an_ordinary_window_with_no_password_field_is_still_silence`
/// is the test that holds it, and it is the first one written.
///
/// **`Matched::Yes` ignores the field entirely, and that is what makes the
/// probe lazy.** The caller is expected to ask
/// [`crate::injector::ui_automation::window_has_password_field`] *only* on the
/// unmatched branch and to pass [`HasPasswordField::Unknown`] otherwise -- so a
/// window the vault already recognises costs nothing extra. Passing `Unknown`
/// there is not a lie: the question genuinely was not asked. The measured cost
/// of asking it is why this matters; see [`PasswordFieldProbe`].
///
/// **`vault` is what stops the unmatched branch lying**, and it splits that
/// branch rather than silencing it. See [`VaultAvailability`] for the chain:
/// while locked the engine is empty, so every window is `Matched::No`, and 3a
/// asserted "No saved login for <app>" about apps that may very well have one.
/// [`VaultAvailability::Locked`] sends that same window to [`Open::Locked`],
/// which says something true (Deskwarden is locked) and claims nothing about
/// whether a match exists -- because a locked vault does not know.
///
/// **The silence control survives untouched, and in both states.** A window
/// with no password field and no match is [`Open::Nothing`] whether the vault
/// is readable or locked; `an_ordinary_window_is_still_silence_in_both_vault_states`
/// is the test that holds it. That is also why the locked answer is gated on
/// the field: a card on every window while the vault is locked is not a
/// feature, it is a popup that follows the user around until they unlock.
///
/// **And it is why the probe is still worth paying for while locked.** The
/// caller must go on asking on the unmatched branch: the answer is *used*
/// here, to choose between 3b and silence, at exactly the cost the unmatched
/// branch already pays. Being locked adds no probe that a readable vault would
/// not also have run. Had the fix been to stay silent when locked, the probe
/// would have had to be skipped rather than paid and discarded -- see
/// `main::process_foreground_event`, which is where that lives.
///
/// # `never` is the fourth input, and it suppresses 3a **and** 3b
///
/// *Never for this app* (design 3c) is the only thing on any of these cards
/// that outlives the card. It is an input to this function rather than a check
/// further downstream for the plainest of reasons: **a "never" that still
/// shows the card is not a "never"**. Anything short of this arm leaves the
/// window opening and closing itself again, which is the behaviour the user
/// just asked to stop.
///
/// **It suppresses [`Open::Locked`] too, and that was a decision.** The
/// argument for exempting 3b is that 3b is a statement about *Deskwarden*, not
/// about the app -- the user said "do not offer to save this app", not "do not
/// tell me you are locked". The argument against it, which wins, is what the
/// user actually experiences: 3b appears on **exactly the same trigger** as 3a
/// -- an unmatched window with a password field -- so an exemption means the
/// silenced app starts popping a card up again the moment the vault locks, and
/// goes quiet again when it unlocks. From the user's chair that is not a
/// different message, it is the same window they turned off, coming back on a
/// schedule they cannot see, for a reason the card does not explain. A control
/// with that behaviour is one users learn not to trust.
///
/// And the exemption would buy nothing: while locked this app can neither fill
/// (the engine is empty, so `Matched::Yes` is unreachable) nor save (3c ends in
/// a write to an unlocked vault), and 3b offers no unlock of its own -- it is
/// purely a notification. Suppressing a notification for an app the user
/// silenced costs them nothing they had.
///
/// **What `never` deliberately does NOT suppress is [`Open::Match`].** The
/// `Matched::Yes` arm never consults it. "Do not offer to save a login for
/// this app" and "do not offer to fill the login I already saved for it" are
/// different questions, and the user answered only the first -- on a card that
/// was only ever shown because there was nothing to fill.
/// `a_never_for_an_app_still_fills_it_if_the_vault_gains_a_match` holds that,
/// and it is the arm that keeps this control from being a way to accidentally
/// switch autofill off for an app forever.
///
/// **The silence control is untouched and still pure.** A window with no
/// password field and no match is [`Open::Nothing`] whatever `never` says --
/// `never` is read only inside the `HasPasswordField::Yes` arm, so it can add
/// silence and can never remove any.
///
/// # `prompts` is the fifth input, and it is the whole setting
///
/// [`OverlayPrompts::Silenced`] suppresses 3a **and** 3b, for the defect
/// [`OverlayPrompts`] describes: the preference reached only the matched path,
/// so "disabled in settings" silenced the card for apps the user HAD saved and
/// left the cards for apps they had not. It deliberately does **not** reach
/// [`Open::Match`] from here -- that arm is gated one layer down by
/// [`match_disposition`], which is also where the hotkey is armed either way,
/// so silencing it here would switch autofill off entirely.
///
/// # `browser` is the sixth, and it suppresses 3a and 3b too
///
/// See [`BrowserWindow`]: in a browser the probe always finds a field and the
/// executable-keyed vault almost never matches, so the card is noise that no
/// saved login can stop. A **matched** browser window still prompts -- the
/// user wrote that rule by hand -- which is again the `Matched::Yes` arm
/// consulting none of this.
///
/// **All four suppressors are read inside the `HasPasswordField::Yes` arm and
/// nowhere else**, which is what keeps the silence control pure: a window with
/// no password field and no match is [`Open::Nothing`] for reasons that have
/// nothing to do with any setting, any list and any browser.
/// `an_ordinary_window_is_still_silence_for_reasons_no_setting_can_change` is
/// the test that holds it, and
/// `an_unmatched_native_app_still_gets_the_card_with_the_setting_on` is the
/// positive control that stops all of this being satisfied by an overlay that
/// never opens.
pub fn disposition<'a>(
    matched: Matched<'a>,
    field: HasPasswordField,
    vault: VaultAvailability,
    never: NeverForApp,
    prompts: OverlayPrompts,
    browser: BrowserWindow,
) -> Open<'a> {
    match matched {
        // A match is only representable when the engine holds entries, which
        // is only true when the vault was read -- so `vault` is not consulted
        // here, and `a_matched_window_ignores_the_vault_state_too` pins that
        // the card the user gets for a recognised window is unchanged.
        Matched::Yes(item_id) => Open::Match(item_id),
        Matched::No => match field {
            // **`never` is read here and nowhere else in the tree**, so it can
            // only ever silence the branch it was added for: an unmatched
            // window with a password field. It does not reach `Matched::Yes`
            // (see the arm above -- a *save* refusal is not a *fill* refusal),
            // and it does not reach the `No | Unknown` arm, which is silent
            // already and whose silence must go on coming from the field
            // answer rather than from a list.
            HasPasswordField::Yes if never == NeverForApp::Yes => Open::Nothing,
            // The setting, and it lands here rather than on `Matched::Yes`
            // for the reason in the doc above: the matched arm's gate is
            // `match_disposition`, and it shares a line with the hotkey
            // arming that must survive the prompt being off.
            HasPasswordField::Yes if prompts == OverlayPrompts::Silenced => Open::Nothing,
            // The browser, last of the three suppressors and the same shape
            // as the two above: it can only ever turn a card into silence.
            HasPasswordField::Yes if browser == BrowserWindow::Yes => Open::Nothing,
            HasPasswordField::Yes => match vault {
                VaultAvailability::Readable => Open::NoMatch,
                VaultAvailability::Locked => Open::Locked,
            },
            // Both silence, and deliberately: a window we could not read is
            // treated exactly as today's build treats every unmatched window.
            // Guessing a card onto the screen from an unanswered question is
            // the failure mode the third arm above exists to prevent, and
            // `Unknown` is the case with the least evidence of all. Locked or
            // readable makes no difference: neither is evidence that this
            // window is asking for a password.
            HasPasswordField::No | HasPasswordField::Unknown => Open::Nothing,
        },
    }
}

/// How long an answer about one window stays good, in the absence of any
/// reason to think the window changed.
///
/// See [`PasswordFieldProbe`] for the measurement this number is chosen
/// against. Short enough that a window the user comes back to a minute later
/// is re-read (a single-page app can navigate to a sign-in form without its
/// `HWND` changing); long enough that cycling through open windows with
/// Alt+Tab costs one probe per window rather than one per keystroke.
pub const PROBE_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// How many windows' answers are remembered at once.
///
/// One entry -- the shape `last_dispatched_hwnd` uses -- would make alternating
/// between two windows cost a full probe every time, which is the commonest
/// thing a user does. Eight covers an ordinary Alt+Tab set; the store is a
/// `Vec` because at this size a scan beats a hash and the code stays readable.
pub const PROBE_MEMORY: usize = 8;

/// One remembered answer.
#[derive(Debug, Clone, Copy)]
struct ProbedWindow {
    hwnd: isize,
    answer: HasPasswordField,
    at: std::time::Instant,
}

/// **The throttle in front of the UI Automation probe, and the reason it is
/// not optional.**
///
/// [`crate::injector::ui_automation::window_has_password_field`] is a
/// cross-process COM call that walks the foreground window's whole
/// accessibility subtree, and when the answer is *no* -- the common case, since
/// most focused windows are not login windows -- there is no early exit and it
/// walks all of it. Measured over the 29 visible top-level windows of a real
/// desktop, with the `IUIAutomation` object reused so the number is the walk
/// and not the setup:
///
/// ```text
/// min 1.7ms   median 27.4ms   p90 133.4ms   max 200.0ms
/// ```
///
/// `CoCreateInstance(CUIAutomation)` is 8.5ms cold and 0.5ms warm, so the
/// setup is not where the time goes -- the tree walk is. The tail is
/// Chromium- and Electron-hosted windows: a Teams chat, a Chrome profile and
/// File Explorer's browser pane were the worst three.
///
/// **That is too expensive to pay on every foreground event**, and the cost
/// lands on the *provider* -- the UI thread of the app the user just switched
/// to -- so it is felt as the newly focused app being slow, not as Deskwarden
/// being slow. Paid once per newly focused window it is acceptable; paid
/// repeatedly for the same window it is a stutter with no possible new answer
/// to show for it.
///
/// So the throttle is **an interval and a change gate**, and both halves are
/// load-bearing:
///
/// * **The change gate** is the map key: a *different* window is a different
///   question and is probed at once, with no wait. A throttle that were only
///   an interval would make the user wait to find out about a window it had
///   never seen.
/// * **The interval** ([`PROBE_TTL`]) is what stops the *same* window being
///   re-asked. A window that has not changed cannot have a different answer,
///   so re-walking its subtree buys nothing. A throttle that were only a
///   change gate would re-ask the same window forever as the user alternated
///   between two apps.
///
/// `dispatch::should_dispatch` already suppresses a repeat foreground event
/// for the same `HWND`, so in the ordinary case this cache is not what saves
/// the call. It is here for the cases that check does not cover -- an app that
/// recreates its top-level window, an Alt+Tab cycle through a set, a window
/// re-focused after an excursion -- and because the probe's own memory should
/// not depend on an unrelated dispatch rule staying the way it is.
///
/// **The probe is a parameter of [`Self::ask`], not a call inside it**, and
/// the clock is too. That is what lets the whole of this be driven by a test
/// that counts calls and moves time by hand, with no COM apartment and no real
/// window anywhere near it.
#[derive(Debug, Default)]
pub struct PasswordFieldProbe {
    /// Most-recently-answered first.
    seen: Vec<ProbedWindow>,
}

impl PasswordFieldProbe {
    /// A probe that has never asked anything.
    pub fn new() -> Self {
        Self { seen: Vec::new() }
    }

    /// The answer for `hwnd` at `now`, asking `probe` only if this window has
    /// no answer still inside [`PROBE_TTL`].
    ///
    /// Returns the answer, so the caller cannot tell a cached answer from a
    /// fresh one -- which is the point: the decision downstream must not
    /// depend on whether the probe was actually run.
    pub fn ask(
        &mut self,
        hwnd: isize,
        now: std::time::Instant,
        probe: impl FnOnce(isize) -> HasPasswordField,
    ) -> HasPasswordField {
        if let Some(hit) = self
            .seen
            .iter()
            .find(|w| w.hwnd == hwnd && now.duration_since(w.at) < PROBE_TTL)
        {
            return hit.answer;
        }

        let answer = probe(hwnd);
        // Any stale or superseded entry for this window goes first, so one
        // window never occupies two slots and a fresh answer cannot be
        // shadowed by the expired one it replaces.
        self.seen.retain(|w| w.hwnd != hwnd);
        self.seen.insert(0, ProbedWindow { hwnd, answer, at: now });
        self.seen.truncate(PROBE_MEMORY);
        answer
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
#[allow(clippy::too_many_arguments)]
pub fn handle_match<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    prompt_on_match: bool,
    window: &crate::window_watch::ForegroundEvent,
    notifier: &dyn sequence::Notifier,
    reprompt: &mut Reprompt<'_>,
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
            // Reads the cache, not `cache.bridge()`: the fill itself
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
            let lookup = || cache.get_by_id(item_id);
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
                fill_from_vault(
                    cache, injector, fill_stats, item_id, hwnd, choice, notifier, reprompt,
                );
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
    /// Shows design **3a** -- the card for a window with a password field that
    /// nothing in the vault matches -- and returns when the user dismisses it.
    ///
    /// **No item argument, and no item in the answer.** There is nothing to
    /// name; a signature that could carry an item id here is a signature
    /// something can later fill in with a sentinel. See [`no_match_arm`].
    ///
    /// It does answer one bit -- whether the user clicked *New login* -- which
    /// is 3a's button finally having a destination, not a promise about an
    /// item. See [`overlay_ui::NoMatchAnswer`].
    fn show_no_match(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::NoMatchAnswer;
    /// Shows design **3c** -- the save-a-new-login form -- and answers what the
    /// user decided together with what they typed.
    ///
    /// `None` is not a decision: it is the overlay refusing to stack a second
    /// window on itself. A user who dismisses the card answers
    /// [`overlay_ui::SaveLoginAction::NotNow`], because "silence today" is a
    /// decision and is spelled as one.
    ///
    /// **It takes the form rather than a label**, which is the whole of the
    /// 3d handoff on this side: 3c must close for 3d to open (`OVERLAY_OPEN`
    /// refuses to stack a second window), so the card comes back a second
    /// time, and a signature that took only a name would re-open it blank --
    /// losing the username the user typed before clicking *Generate*, and the
    /// generated password 3d just produced.
    fn show_save_login(
        &self,
        form: overlay_ui::SaveLoginForm,
        position: Option<(f32, f32)>,
    ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)>;
    /// Shows design **3d** -- the generator -- and answers the password the
    /// user chose to keep, or `None` if they dismissed it.
    ///
    /// `generate` is the round trip to `bw serve`, passed in rather than
    /// reached for: `overlay_ui` has no vault handle, and **there is no
    /// generator in this crate** -- the randomness is the server's, which is
    /// the one property of this feature that must not be reimplemented.
    ///
    /// A borrowed `dyn Fn` rather than a function pointer because the caller's
    /// is a closure over a `&VaultCache`; see [`handle_no_match`].
    fn show_generate(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
        generate: &dyn Fn(
            &crate::vault_bridge::GenerateRequest,
        ) -> Result<zeroize::Zeroizing<String>, String>,
    ) -> Option<zeroize::Zeroizing<String>>;
    /// Shows design **3b** -- the card for a window with a password field
    /// focused while the vault cannot be read -- and returns when the user
    /// dismisses it.
    ///
    /// A separate method rather than a flag on [`Self::show_no_match`], for the
    /// reason [`handle_no_match`] is separate from [`handle_match`]: the two
    /// cards make opposite claims about the vault, and a boolean that chose
    /// between them is a boolean something can pass wrongly.
    ///
    /// **It answers now, and it still takes no item and returns none.** 3b's
    /// *Unlock* button has a destination -- [`crate::unlock_prompt`] -- and
    /// the route to it is this return value, exactly as
    /// [`Self::show_no_match`]'s is the route to the vault window. What
    /// [`overlay_ui::LockedAnswer`] can say is "the user asked to unlock" and
    /// nothing else: it names no item, authorises no fill, and carries no
    /// password. The card is still shown for a vault this process cannot read,
    /// so a signature that could carry an id would be one nothing could
    /// honestly fill.
    fn show_locked(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::LockedAnswer;
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
    /// Asked to put design 3a on screen. Answers whether *New login* was
    /// clicked; see [`PromptPresenter::show_no_match`].
    pub show_no_match: fn(&str, Option<(f32, f32)>) -> overlay_ui::NoMatchAnswer,
    /// Asked to put design 3c on screen. See
    /// [`PromptPresenter::show_save_login`].
    #[allow(clippy::type_complexity)]
    pub show_save_login: fn(
        overlay_ui::SaveLoginForm,
        Option<(f32, f32)>,
    )
        -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)>,
    /// Asked to put design 3d on screen. See
    /// [`PromptPresenter::show_generate`].
    #[allow(clippy::type_complexity)]
    pub show_generate: fn(
        &str,
        Option<(f32, f32)>,
        &dyn Fn(
            &crate::vault_bridge::GenerateRequest,
        ) -> Result<zeroize::Zeroizing<String>, String>,
    ) -> Option<zeroize::Zeroizing<String>>,
    /// Asked to put design 3b on screen. Answers whether *Unlock* was clicked;
    /// see [`PromptPresenter::show_locked`].
    pub show_locked: fn(&str, Option<(f32, f32)>) -> overlay_ui::LockedAnswer,
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

    fn show_no_match(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::NoMatchAnswer {
        (self.show_no_match)(label, position)
    }

    fn show_save_login(
        &self,
        form: overlay_ui::SaveLoginForm,
        position: Option<(f32, f32)>,
    ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
        (self.show_save_login)(form, position)
    }

    fn show_generate(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
        generate: &dyn Fn(
            &crate::vault_bridge::GenerateRequest,
        ) -> Result<zeroize::Zeroizing<String>, String>,
    ) -> Option<zeroize::Zeroizing<String>> {
        (self.show_generate)(label, position, generate)
    }

    fn show_locked(
        &self,
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::LockedAnswer {
        (self.show_locked)(label, position)
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
    show_no_match: overlay_ui::show_no_match_overlay,
    show_locked: overlay_ui::show_locked_overlay,
    show_save_login: overlay_ui::show_save_login_overlay,
    show_generate: overlay_ui::show_generate_overlay,
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

/// **The whole of the no-match arm, as a pure function** -- the 3a sibling of
/// [`prompt_arm`], and written the same way for the same reason.
///
/// It asks the presenter where the card goes and then shows it there. That is
/// two lines, and both of them are exactly the two lines review 32 found could
/// be silently wrong at a call site no test could reach: an overlay pinned to
/// the top of the screen, or one opened for a window other than the one it
/// names. Driven through a recording presenter, neither can be.
///
/// **The placement is asked about [`overlay_ui::NO_MATCH_ROWS`]**, which is
/// the row count the 3a card is sized by. Asking about `0` -- or about a
/// choice list this state does not have -- would clamp the card onto the work
/// area using the wrong height, which on a window anchored near the bottom of
/// the screen puts its footer, and its only dismiss hint, under the taskbar.
///
/// **The label is [`window_label`]**, not `exe_name`, for the reason
/// `prompt_request` gives: a window matched through the title table belongs to
/// a process whose name means nothing to the user. It matters more here than
/// there -- this card's entire content is the name of the app it has nothing
/// for, so a wrong name is the whole message being wrong.
///
/// **Nothing about an item crosses this function**, because there is no item.
/// There is no id, no `VaultItem`, no [`PromptSubject`] and no cache: the
/// re-prompt gate ([`permitted_by_reprompt`]) is defined over an existing item
/// and so cannot be asked here, and this signature is why that is a
/// type-level fact rather than a discipline.
pub fn no_match_arm<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
) -> overlay_ui::NoMatchAnswer {
    let position = presenter.position(window.hwnd, overlay_ui::NO_MATCH_ROWS);
    presenter.show_no_match(window_label(&window.exe_name, &window.title), position)
}

/// **The whole of the 3c arm, as a pure function** -- [`no_match_arm`]'s
/// sibling, written the same way and for the same two reasons.
///
/// **The placement is asked about [`overlay_ui::SAVE_LOGIN_ROWS`]**, not
/// `NO_MATCH_ROWS`. The two are different numbers -- 3c is by far the tallest
/// state the overlay has -- and the clamp onto the monitor's work area is a
/// function of the card's height, so asking about the card the user just left
/// would put this card's *Save* button, its *Never* link and the bottom of its
/// password field under the taskbar for any window anchored near the bottom of
/// the screen.
///
/// **Nothing about an item crosses this function either**, and that is still
/// true even though this is the arm that ends in a create: there is no item
/// *yet*. What comes back is what the user typed, and
/// [`route_save_answer`] is what decides whether it becomes one.
/// **It is handed the form to open with**, rather than building one from the
/// window: design 3d can send the user back here with a password in hand and
/// a username already typed, and an arm that re-derived a blank form would
/// throw both away. [`save_login_flow`] is the loop that does the sending.
pub fn save_login_arm<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
    form: overlay_ui::SaveLoginForm,
) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
    let position = presenter.position(window.hwnd, overlay_ui::SAVE_LOGIN_ROWS);
    presenter.show_save_login(form, position)
}

/// **The 3d arm**: [`save_login_arm`]'s sibling, and the only place design 3d
/// is opened.
///
/// **The placement is asked about [`overlay_ui::GENERATE_ROWS`]** and not
/// about the card the user just left, for the reason `save_login_arm`'s doc
/// gives: the clamp onto the monitor's work area is a function of the card's
/// height, so the wrong height puts this card's *Save to vault* button under
/// the taskbar for any window anchored near the bottom of the screen.
pub fn generate_arm<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
    generate: &dyn Fn(
        &crate::vault_bridge::GenerateRequest,
    ) -> Result<zeroize::Zeroizing<String>, String>,
) -> Option<zeroize::Zeroizing<String>> {
    let position = presenter.position(window.hwnd, overlay_ui::GENERATE_ROWS);
    presenter.show_generate(
        window_label(&window.exe_name, &window.title),
        position,
        generate,
    )
}

/// How many times [`save_login_flow`] will hand the user from 3c to 3d and
/// back before it gives up and answers as if the card had been dismissed.
///
/// **A liveness bound, not a budget on the user.** Regenerating a password
/// happens *inside* 3d -- Ctrl+R, the *New* link, and every change of kind or
/// size -- and costs no hop at all; a hop is only spent going to the
/// generator and coming back, which is a thing a person does once or twice.
/// What the number is really for is that this loop's continuation condition
/// is an answer from a presenter, and a presenter that always answered
/// *Generate* would spin forever with a window on screen and no way out. That
/// presenter exists: it is the recorder
/// `a_presenter_that_only_ever_generates_does_not_spin_forever` drives this
/// with.
pub const GENERATE_HOPS: usize = 8;

/// **The whole of the 3c/3d loop, as a pure function.**
///
/// 3c is shown; if the user clicked *Generate*, 3d is shown, whatever it
/// produced is written into the form's password row, and 3c is shown again --
/// carrying the username they had already typed. Any other answer ends it.
///
/// **The two cards alternate rather than nest** because the overlay is one
/// window at a time: `overlay_ui::OVERLAY_OPEN` refuses to stack a second,
/// and `eframe::run_native` on this thread could not open one anyway. So the
/// form is the thing that crosses between them, and it crosses by value.
///
/// A dismissed 3d (`None`) leaves the password exactly as it was and returns
/// to 3c, which is the only reading of "I changed my mind about generating"
/// that does not destroy what the user typed.
pub fn save_login_flow<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
    generate: &dyn Fn(
        &crate::vault_bridge::GenerateRequest,
    ) -> Result<zeroize::Zeroizing<String>, String>,
) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
    let mut form =
        overlay_ui::SaveLoginForm::new(window_label(&window.exe_name, &window.title));
    for _ in 0..GENERATE_HOPS {
        let (action, answered) = save_login_arm(presenter, window, form)?;
        form = answered;
        if action != overlay_ui::SaveLoginAction::Generate {
            return Some((action, form));
        }
        if let Some(password) = generate_arm(presenter, window, generate) {
            form.password = password;
        }
    }
    // Out of hops. `NotNow` and not `Never`: the strongest answer this card
    // offers is not one to reach by running out of something.
    Some((overlay_ui::SaveLoginAction::NotNow, form))
}

/// The `NewItem` a filled-in 3c form becomes.
///
/// **One function, and it is the whole mapping** from the card's four rows to
/// the four arguments `NewItem::login` takes -- so "which row went where" is a
/// claim a test makes about a value, not a claim a reviewer makes about a call
/// buried in a window callback.
///
/// * **name** is the app name, the one pre-filled row: the item is named after
///   the thing the user was signing in to, which is the only name this process
///   has for it.
/// * **username** and **password** are what the user typed. Both may be empty;
///   `vault_bridge` omits a blank one rather than POSTing `""`.
/// * **folder** is `None`, always. See [`overlay_ui::FOLDER_ROW_TEXT`] for why
///   the card states a folder rather than picking one, and
///   `the_new_login_is_unfiled_because_the_card_offers_no_folder` for the test
///   that holds the two together.
///
/// It takes the form by value so the `Zeroizing` password is *moved* into the
/// payload rather than copied out of a borrow and left behind in the form.
pub fn new_login_item(form: overlay_ui::SaveLoginForm) -> crate::vault_bridge::NewItem {
    crate::vault_bridge::NewItem::login(
        form.app_name.clone(),
        form.username.clone(),
        form.password.as_str(),
        None,
    )
}

/// What routing a 3c answer did.
///
/// **[`Self::Nothing`] and [`Self::Silenced`] are different variants, and that
/// is the point of this type.** *Not now* is silence today; *Never for this
/// app* is silence forever, and it writes to `settings.json`. A routing that
/// answered a `bool` -- or that folded both into "did not save" -- is exactly
/// the bug a user cannot undo without finding a setting, because the two look
/// identical at the moment they are chosen and only diverge the next time the
/// window is focused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// *Save*: the create route was called, and this is what it said.
    Created(Result<String, String>),
    /// *Not now*, Esc, the ✕, or the overlay refusing to open: nothing was
    /// written, nothing was remembered, and the card comes back next time.
    Nothing,
    /// *Never for this app*: nothing was written to the vault, and this app
    /// was remembered so that [`disposition`] stops opening 3a and 3b for it.
    Silenced(String),
}

/// **The three answers, as a pure function.**
///
/// The whole of what 3c decides, with the vault and the settings file behind
/// two closures -- so a test can drive all three answers, count the calls, and
/// assert that *Not now* and *Never* really do different things, without a
/// vault, a window or a disk anywhere near it.
///
/// `create` is called **only** for [`overlay_ui::SaveLoginAction::Save`], and
/// `silence` **only** for [`overlay_ui::SaveLoginAction::Never`]. Neither is
/// called for the other's answer and neither is called twice;
/// `the_three_answers_do_three_different_things` counts both.
///
/// `answer` is an `Option` because [`save_login_arm`] can come back with
/// nothing at all -- the overlay refusing to stack a second window. That is
/// [`SaveOutcome::Nothing`], the same as *Not now*, and deliberately so:
/// a card that never opened has not been answered, and must not be recorded as
/// a "never".
pub fn route_save_answer(
    answer: Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)>,
    create: impl FnOnce(&crate::vault_bridge::NewItem) -> Result<String, String>,
    silence: impl FnOnce(&str),
) -> SaveOutcome {
    let Some((action, form)) = answer else {
        return SaveOutcome::Nothing;
    };
    match action {
        overlay_ui::SaveLoginAction::Save => SaveOutcome::Created(create(&new_login_item(form))),
        overlay_ui::SaveLoginAction::Never => {
            let app = form.app_name.clone();
            silence(&app);
            SaveOutcome::Silenced(app)
        }
        // The ✕, Esc, *Not now*, and the card that closed without an answer
        // all land here, and all of them mean "ask me again". The weakest
        // gestures a user can make must not be read as the strongest answer
        // the card offers.
        overlay_ui::SaveLoginAction::NotNow | overlay_ui::SaveLoginAction::None => {
            SaveOutcome::Nothing
        }
        // **[`overlay_ui::SaveLoginAction::Generate`] is not a decision about
        // the vault**, and it does not reach here through
        // [`save_login_flow`], which resolves it by opening design 3d and
        // asking 3c again. It is spelled out rather than folded into the arm
        // above because the two mean different things and a `_ =>` would hide
        // the day a fifth answer is added: a `Generate` that got this far is
        // a flow that ended mid-hop, and the only safe reading of a card the
        // user never finished answering is "nothing, ask again".
        // `a_generate_that_escapes_the_flow_writes_nothing` holds it.
        overlay_ui::SaveLoginAction::Generate => SaveOutcome::Nothing,
    }
}

/// Dispatches a freshly foregrounded window that asks for a password and that
/// **nothing in the vault matches**: design 3a.
///
/// The sibling of [`handle_match`], and separate from it rather than
/// `handle_match` gaining an `Option<&str>`. Two functions is the stronger
/// shape: "there is no item" is not an argument that could be `Some` here, it
/// is the absence of the parameter, so no sentinel id can be introduced and no
/// `unwrap` can be reached. It is also why this function takes no `VaultCache`,
/// no `Injector`, no `FillStats` and no [`Reprompt`] -- nothing it is given
/// can type, count or unlock anything.
///
/// **Returns nothing, so nothing is armed.** `handle_match` answers
/// `(item_id, hwnd)` for the fill hotkey; there is no item id here, and a
/// hotkey armed against a window with no credentials behind it would fire into
/// whatever holds focus later.
///
/// **It takes a [`VaultCache`] now, and that is a real widening of what this
/// path holds.** 3a's *New login* button leads to 3c, and 3c ends in creating
/// a vault item; nothing that cannot reach the vault can do that. What the
/// widening does *not* buy is any of the things the narrow signature was
/// protecting against: there is still no `Injector` and no `FillStats`, so
/// nothing here can type; there is still no [`Reprompt`], so the re-prompt
/// gate -- which is defined over an existing item -- is still not reachable
/// from a path that has no item; and this still returns `()`, so the fill
/// hotkey is still not armed. The cache is used for exactly one call,
/// [`VaultCache::create_item`], which is the same route the edit form takes.
///
/// Only the real presenter and the real create route are named on these lines;
/// every decision is [`no_match_arm`]'s, [`save_login_arm`]'s and
/// [`route_save_answer`]'s, each of which a test drives with a recorder.
pub fn handle_no_match(
    cache: &VaultCache,
    window: &crate::window_watch::ForegroundEvent,
) -> NoMatchFollowUp {
    let answer = no_match_arm(&REAL_OVERLAY, window);
    if answer != overlay_ui::NoMatchAnswer::NewLogin {
        return no_match_follow_up(answer, window_label(&window.exe_name, &window.title));
    }
    let outcome = route_save_answer(
        save_login_flow(&REAL_OVERLAY, window, &|request| {
            // **The one generator, and it is `bw serve`'s.** Nothing in this
            // crate produces randomness for a password; this closure is the
            // whole of what design 3d can ask for, and it is the same
            // `VaultBridge::generate` the edit form's own generator calls.
            cache.bridge().generate(request).map_err(|e| {
                log::warn!("the overlay could not generate a password: {e:?}");
                overlay_ui::GENERATE_FAILED_TEXT.to_string()
            })
        }),
        // **The one item-creating route in this app**, the same
        // `VaultCache::create_item` the edit form calls -- not a second one.
        // `no_match_wiring_tests` pins this call by source text, the way
        // `prompt_wiring_tests` pins `REAL_OVERLAY`'s two function names,
        // because this line needs a real vault and a real window and so is
        // the one thing here no test can execute.
        |new_item| {
            cache
                .create_item(new_item)
                .map(|item| item.id)
                .map_err(|e| format!("{e:?}"))
        },
        remember_never_for_app,
    );
    log::info!("the save-a-new-login card was answered: {}", describe_outcome(&outcome));
    // 3c's own answers never open a window: `Save` wrote the item, and the
    // other three are silences of different lengths. Nothing follows.
    NoMatchFollowUp::Nothing
}

/// **What the caller must do after one of the two unmatched-window cards
/// closes**, and the only things it can be asked to do.
///
/// **The name is 3a's and the type is now both cards'.** It was written when
/// 3a was the only card that could ask for anything at all; 3b's *Unlock*
/// button gave the locked card its first request, and it travels this same
/// channel because there is only one -- `main::process_foreground_event`
/// returns one value to `run`'s loop, and a second parallel return would be
/// two answers about one event that could disagree. Renaming it would touch
/// every call site for no behavioural difference; what matters is that this
/// list is exhaustive over what the overlay may ask `main` for, and it is.
///
/// Three variants and not a `bool` or an `Option<String>`, for the reason
/// [`overlay_ui::NoMatchAnswer`] is two variants: the value crosses
/// `main::process_foreground_event` and lands in `run`'s loop, and "there is a
/// string" is a worse way to say "open a window" than saying it.
///
/// **This is the whole of what the overlay can make `main` do**, which is the
/// property that matters more than the shape. `handle_no_match` is still handed
/// no `Injector`, no `FillStats` and no [`Reprompt`], and still cannot arm the
/// fill hotkey; what it gained is one request, for a window `run` already opens
/// at three other doors, carrying one string that is
/// [`window_label`]'s answer and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NoMatchFollowUp {
    /// The card was dismissed, or it led to 3c and 3c has finished. Nothing
    /// follows.
    #[default]
    Nothing,
    /// *Search vault* was clicked. Open the vault window, with this in its
    /// search box -- the same [`window_label`] the card itself was naming, so
    /// the user is handed the query they were just looking at rather than an
    /// unfiltered list of everything.
    SearchVault(String),
    /// 3b's *Unlock* was clicked. Put [`crate::unlock_prompt`] on screen, and
    /// if it answers with a session token, resettle the session and dispatch
    /// this window again -- which is how the fill the user asked for resumes.
    ///
    /// **It carries nothing**, and that is not an oversight. The window this
    /// is about is the one `run` just handed to `process_foreground_event`, so
    /// the caller already has it; a copy in here would be a second description
    /// of one window that could disagree with the first -- the exact defect
    /// `handle_match`'s "take one `ForegroundEvent`" rule exists for. In
    /// particular it carries no password, no session token and no item id: it
    /// is a request to ask, not the answer to anything.
    Unlock,
}

/// [`overlay_ui::NoMatchAnswer`] as a [`NoMatchFollowUp`], **as a pure
/// function**.
///
/// It exists so that the mapping is testable at all: the only production
/// caller is [`handle_no_match`], which raises a real always-on-top window and
/// so is the one thing on that path no test in this crate may execute.
///
/// [`overlay_ui::NoMatchAnswer::NewLogin`] answers `Nothing` here and that is
/// not a hole: `handle_no_match` never reaches this function with it, because
/// `NewLogin` means "run 3c" and 3c's own outcome is a save or a silence.
/// Mapping it rather than making it unrepresentable keeps this function total,
/// which is what lets a test walk all three answers.
/// A [`window_label`] as something worth typing into the vault's search box:
/// **the same name without a trailing `.exe`.**
///
/// The card says "No saved login for Atlas Licence.exe" because that is what
/// the window is, and the executable name is what every log line and overlay in
/// this app has always said. But it is a poor *query*: the vault's search runs
/// over item names, usernames and URIs, and an item saved for that app is
/// called "Atlas Licence". Handing the box `Atlas Licence.exe` would open the
/// window on an empty list -- the one result that reads as "you have nothing"
/// on a surface the user came to precisely because they were told that.
///
/// **Only that suffix, and only when something is left.** A host frame's label
/// is its window title and has no suffix to strip; a hypothetical `.exe` would
/// leave the empty string, which is not "no query" here but "match everything",
/// so the label is kept whole instead. Case-insensitively, because Windows
/// paths are.
///
/// It is deliberately not cleverer than that. Splitting on separators or
/// dropping a version number would be guessing at the user's item names, and a
/// query is a starting point they can edit rather than a filter they are stuck
/// with.
pub fn search_query(label: &str) -> &str {
    let cut = label.len().saturating_sub(4);
    if cut > 0 && label.is_char_boundary(cut) && label[cut..].eq_ignore_ascii_case(".exe") {
        return &label[..cut];
    }
    label
}

pub fn no_match_follow_up(
    answer: overlay_ui::NoMatchAnswer,
    app_name: &str,
) -> NoMatchFollowUp {
    match answer {
        overlay_ui::NoMatchAnswer::SearchVault => {
            NoMatchFollowUp::SearchVault(search_query(app_name).to_string())
        }
        overlay_ui::NoMatchAnswer::Dismissed | overlay_ui::NoMatchAnswer::NewLogin => {
            NoMatchFollowUp::Nothing
        }
    }
}

/// [`overlay_ui::LockedAnswer`] as a [`NoMatchFollowUp`], **as a pure
/// function**.
///
/// [`no_match_follow_up`]'s sibling, and it exists for the same reason: the
/// only production caller is [`handle_locked`], which raises a real
/// always-on-top window and so is the one thing on that path no test in this
/// crate may execute.
///
/// It takes no app name, where [`no_match_follow_up`] takes one. There is
/// nothing to name: an unlock is about *Deskwarden*, not about the app the
/// card was shown over, and a query string threaded through here would be a
/// string this follow-up has no use for and something later could act on.
pub fn locked_follow_up(answer: overlay_ui::LockedAnswer) -> NoMatchFollowUp {
    match answer {
        overlay_ui::LockedAnswer::Unlock => NoMatchFollowUp::Unlock,
        overlay_ui::LockedAnswer::Dismissed => NoMatchFollowUp::Nothing,
    }
}

/// What a [`SaveOutcome`] is written to the log as.
///
/// **A function, so that "the log line never carries the password" is a claim
/// a test can make** rather than a property of a format string at an
/// unreachable call site. `SaveOutcome` cannot hold the password -- the
/// `Zeroizing` is moved into the `NewItem` and dropped with it -- but the item
/// *name* and the app name are in it, and this is where what is said about
/// them is decided.
pub fn describe_outcome(outcome: &SaveOutcome) -> String {
    match outcome {
        SaveOutcome::Created(Ok(id)) => format!("saved as vault item {id}"),
        SaveOutcome::Created(Err(e)) => format!("the save failed: {e}"),
        SaveOutcome::Nothing => "not now -- nothing was written, and it will ask again".to_string(),
        SaveOutcome::Silenced(app) => {
            format!("never for {app} -- the overlay will not open for it again")
        }
    }
}

/// **The whole of the locked arm, as a pure function** -- [`no_match_arm`]'s
/// 3b sibling, written the same way and for the same two reasons: the
/// placement must be computed for the window the card names, and it must be
/// computed for the row count the card is *sized* by.
///
/// **The placement is asked about [`overlay_ui::LOCKED_ROWS`]**, not
/// `NO_MATCH_ROWS`, even though the two are equal today. The clamp onto the
/// monitor's work area is a function of the card's height, so asking about the
/// other card's constant would be correct only by coincidence -- and would
/// stop being correct the moment either card changed shape, silently, by
/// putting this card's footer and its only dismiss hint under the taskbar.
///
/// **Nothing about an item or the vault crosses this function.** There is no
/// id, no `VaultItem`, no cache and no [`Reprompt`] -- the same signature-level
/// fact `no_match_arm` records, and it is stronger here: this path runs
/// precisely when the vault cannot be read, so a parameter that could carry an
/// item would be a parameter nothing could honestly fill.
/// **It answers what the card answered, and nothing more.** The one thing 3b
/// can now ask for is the master-password prompt, and this function neither
/// opens it nor decides whether it may be opened: it forwards
/// [`overlay_ui::LockedAnswer`] to `main::process_foreground_event`, which
/// forwards it to `run`'s loop -- the one place in this process that owns the
/// session, the backend child and the tray, and therefore the only place that
/// may resettle any of them. Opened here instead, the unlock would be a second
/// teardown-and-repopulate path beside `main::resettle_session`, which is the
/// defect that function's doc exists to prevent.
pub fn locked_arm<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
) -> overlay_ui::LockedAnswer {
    let position = presenter.position(window.hwnd, overlay_ui::LOCKED_ROWS);
    presenter.show_locked(window_label(&window.exe_name, &window.title), position)
}

/// Dispatches a freshly foregrounded window that asks for a password while the
/// vault cannot be read: design 3b.
///
/// [`handle_no_match`]'s sibling, and separate for the reason that one is
/// separate from [`handle_match`]. **Nothing is armed**: it is handed no
/// cache, no injector, no `FillStats` and no [`Reprompt`], so nothing it holds
/// can type, count or unlock anything -- and that stays true now that it
/// answers, because what it answers is a request rather than an authorisation.
/// [`overlay_ui::LockedAnswer::Unlock`] names no item and carries no password;
/// the caller that acts on it is `run`'s loop, and what it does there is the
/// one resettle sequence this crate has.
///
/// Only the real presenter is named on this line; every decision is
/// [`locked_arm`]'s, which a test drives with a recorder.
pub fn handle_locked(window: &crate::window_watch::ForegroundEvent) -> NoMatchFollowUp {
    locked_follow_up(locked_arm(&REAL_OVERLAY, window))
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
                        value: Some(zeroize::Zeroizing::new(v.to_string())),
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

    /// What the **no-match** card was told: the label, and where it was put.
    /// There is no third element because there is no item -- which is the
    /// whole distinction between this log and [`Shown`].
    type NoMatchShown = (String, Option<(f32, f32)>);

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
        /// The label and placement each `show_no_match` was handed -- design
        /// 3a's own log, kept apart from `shown`.
        no_match_shown: std::cell::RefCell<Vec<NoMatchShown>>,
        /// The label and placement each `show_locked` was handed -- design
        /// 3b's own log, kept apart from both of the others.
        locked_shown: std::cell::RefCell<Vec<NoMatchShown>>,
        /// What `show_locked` answers: whether the user pressed *Unlock*.
        /// Defaults to [`overlay_ui::LockedAnswer::Dismissed`], so every test
        /// that does not name it gets the card being closed rather than a
        /// master-password prompt being asked for.
        locked_answer: overlay_ui::LockedAnswer,
        /// The label and placement each `show_save_login` was handed -- design
        /// 3c's own log, kept apart from all three of the others.
        save_login_shown: std::cell::RefCell<Vec<NoMatchShown>>,
        /// The label and placement each `show_generate` was handed -- design
        /// 3d's own log, kept apart from all four of the others.
        generate_shown: std::cell::RefCell<Vec<NoMatchShown>>,
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

        /// Design 3a goes into a log of its own rather than into `shown`: a
        /// no-match card recorded as though it were a matched one would let
        /// `no_match_arm` satisfy a test written about `prompt_arm`, and the
        /// two states differ by exactly whether an item exists.
        fn show_no_match(
            &self,
            label: &str,
            position: Option<(f32, f32)>,
        ) -> overlay_ui::NoMatchAnswer {
            self.no_match_shown
                .borrow_mut()
                .push((label.to_string(), position));
            // Dismissed, so this recorder never leads on into 3c: the tests
            // that use it are about WHERE 3a opened and WHAT it was called,
            // and a recorder that walked on to the save card would make every
            // one of them a test of two cards.
            overlay_ui::NoMatchAnswer::Dismissed
        }

        /// Design 3c goes into a log of its own too, for the reason 3a and 3b
        /// do: it is the only card of the four that can WRITE, so a recorder
        /// that let it satisfy another card's test would be the loosest of the
        /// four.
        fn show_save_login(
            &self,
            form: overlay_ui::SaveLoginForm,
            position: Option<(f32, f32)>,
        ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
            self.save_login_shown
                .borrow_mut()
                .push((form.app_name.clone(), position));
            None
        }

        /// Design 3d goes into a log of its own for the reason the other
        /// three do, and one more: it is the only card that is opened FROM
        /// another card, so a recorder that shared 3c's log could not tell a
        /// hop from a re-open.
        fn show_generate(
            &self,
            label: &str,
            position: Option<(f32, f32)>,
            _generate: &dyn Fn(
                &crate::vault_bridge::GenerateRequest,
            ) -> Result<zeroize::Zeroizing<String>, String>,
        ) -> Option<zeroize::Zeroizing<String>> {
            self.generate_shown
                .borrow_mut()
                .push((label.to_string(), position));
            None
        }

        /// Design 3b goes into a log of its own, for the same reason 3a does
        /// and one more: 3a and 3b are the two cards that make OPPOSITE
        /// claims about the vault, so a recorder that could not tell them
        /// apart would let either satisfy a test written about the other --
        /// which is exactly the defect this state exists to correct.
        fn show_locked(
            &self,
            label: &str,
            position: Option<(f32, f32)>,
        ) -> overlay_ui::LockedAnswer {
            self.locked_shown
                .borrow_mut()
                .push((label.to_string(), position));
            self.locked_answer
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

    /// **Design 3a opens where it was told, for the window it names.**
    ///
    /// The 3a sibling of `the_overlay_opens_where_the_placement_answered_for_
    /// that_window`, and it exists for the same reason: `handle_no_match`'s one
    /// line is unreachable, so the two things `no_match_arm` does -- ask where
    /// the card goes, and show it there -- would otherwise be exactly as
    /// silently alterable as the matched arm's were (review 32's Important 1,
    /// where mapping the placement's `y` to `0.0` pinned every overlay to the
    /// top of the screen with the whole suite green).
    #[test]
    fn the_no_match_card_opens_where_the_placement_answered_for_that_window() {
        let w = window("AtlasLicence.exe", "Atlas Licence");
        let presenter = RecordingPresenter {
            placement: Some((120.0, 340.0)),
            ..Default::default()
        };

        no_match_arm(&presenter, &w);

        assert_eq!(
            presenter.asked_about.get(),
            Some(w.hwnd),
            "the placement was computed for a handle other than the window that has nothing \
             matching it"
        );
        assert_eq!(
            presenter.asked_rows.get(),
            Some(overlay_ui::NO_MATCH_ROWS),
            "the placement was asked about a row count other than the one the 3a card is \
             SIZED by. The clamp onto the monitor's work area is a function of that height, so \
             a wrong count puts the card's footer -- and its only dismiss hint -- under the \
             taskbar on a window anchored near the bottom of the screen"
        );
        let shown = presenter.no_match_shown.borrow();
        assert_eq!(shown.len(), 1, "the no-match card is shown exactly once");
        assert_eq!(
            shown[0].1,
            Some((120.0, 340.0)),
            "the card must open at the placement that was answered for this window"
        );
        assert!(
            presenter.shown.borrow().is_empty(),
            "the MATCHED card was raised for a window with no match. The two states differ by \
             exactly whether an item exists, and this one has none"
        );
        assert!(
            presenter.locked_shown.borrow().is_empty(),
            "the LOCKED card was raised by `no_match_arm`. That card says Deskwarden cannot \
             read the vault, which is the opposite of what this arm is called for"
        );
    }

    /// [`locked_arm`]'s half of the same claim: the card opens **where** the
    /// placement answered, and the placement is asked **about this window**
    /// and **about the row count the 3b card is sized by**.
    ///
    /// Written out rather than shared with the 3a test above, because the two
    /// arms are two functions with two constants, and a test parameterised
    /// over them would pass if `locked_arm` were a call to `no_match_arm`.
    #[test]
    fn the_locked_card_opens_where_the_placement_answered_for_that_window() {
        let w = window("AtlasLicence.exe", "Atlas Licence");
        let presenter = RecordingPresenter {
            placement: Some((120.0, 340.0)),
            ..Default::default()
        };

        locked_arm(&presenter, &w);

        assert_eq!(
            presenter.asked_about.get(),
            Some(w.hwnd),
            "the placement was computed for a handle other than the window it was asked about"
        );
        assert_eq!(
            presenter.asked_rows.get(),
            Some(overlay_ui::LOCKED_ROWS),
            "the placement was asked about a row count other than the one the 3b card is \
             SIZED by. The clamp onto the work area is a function of that height, so a wrong \
             count puts the card's footer -- and its only dismiss hint -- under the taskbar"
        );
        let shown = presenter.locked_shown.borrow();
        assert_eq!(shown.len(), 1, "the locked card is shown exactly once");
        assert_eq!(
            shown[0].1,
            Some((120.0, 340.0)),
            "the card must open at the placement that was answered for this window"
        );
        assert!(
            presenter.no_match_shown.borrow().is_empty(),
            "`locked_arm` raised the NO-MATCH card. That card asserts there is no saved login \
             for this app, which a locked vault cannot know -- and it is the whole reason 3b \
             exists"
        );
        assert!(
            presenter.shown.borrow().is_empty(),
            "the matched card was raised for a vault that cannot be read at all"
        );
    }

    /// **What the locked card answered comes back out of [`locked_arm`]**, and
    /// a dismissal does not.
    ///
    /// The sibling of the placement test above, and the half that is new: 3b
    /// used to answer nothing at all, so the whole of "the *Unlock* button
    /// reaches anything" lives in this return value. Dropped -- an arm that
    /// showed the card and then answered `Dismissed` -- the button is inert,
    /// which is the failure `overlay_ui::SEARCH_VAULT_LABEL`'s doc calls worse
    /// than not drawing it at all.
    ///
    /// Both directions, from the same recorder, so the assertion cannot be
    /// satisfied by an arm that answers `Unlock` unconditionally: that would
    /// put a modal master-password prompt up every time a user dismissed the
    /// card.
    #[test]
    fn the_locked_arm_answers_what_the_card_answered() {
        let w = window("AtlasLicence.exe", "Atlas Licence");

        let pressed = RecordingPresenter {
            locked_answer: overlay_ui::LockedAnswer::Unlock,
            ..Default::default()
        };
        assert_eq!(
            locked_arm(&pressed, &w),
            overlay_ui::LockedAnswer::Unlock,
            "pressing Unlock on 3b answered a dismissal, so the button does nothing and the \
             user is left with the ~95 MB window the card exists to spare them"
        );

        let dismissed = RecordingPresenter::default();
        assert_eq!(
            locked_arm(&dismissed, &w),
            overlay_ui::LockedAnswer::Dismissed,
            "dismissing 3b asked for a master-password prompt. Closing a card the app put on \
             screen unprompted must cost the user nothing at all"
        );
    }

    /// The join between what 3b answered and what `main` does about it.
    ///
    /// [`no_match_follow_up`]'s table has the same shape and the same reason:
    /// the only production caller is [`handle_locked`], which raises a real
    /// window, so this mapping is the one part of that line a test can read. A
    /// `Nothing` for `Unlock` is an inert button; an `Unlock` for `Dismissed`
    /// is a master-password prompt for a card the user just closed.
    #[test]
    fn a_dismissed_locked_card_asks_for_no_unlock() {
        assert_eq!(
            locked_follow_up(overlay_ui::LockedAnswer::Unlock),
            NoMatchFollowUp::Unlock
        );
        assert_eq!(
            locked_follow_up(overlay_ui::LockedAnswer::Dismissed),
            NoMatchFollowUp::Nothing
        );
        // And it never asks for the vault WINDOW, which is the other door
        // `main` has and the one that costs what this path avoids.
        for answer in [overlay_ui::LockedAnswer::Unlock, overlay_ui::LockedAnswer::Dismissed] {
            assert!(
                !matches!(locked_follow_up(answer), NoMatchFollowUp::SearchVault(_)),
                "the locked card asked to search a vault it cannot read, which opens a window \
                 onto an empty list that reads as `nothing found`"
            );
        }
    }

    /// **Only *Search vault* asks for a window**, and it asks with a query.
    ///
    /// The pair matters more than either half: `NewLogin` mapping to
    /// `SearchVault` would open the vault instead of design 3c, and
    /// `Dismissed` mapping to it would open the vault on every card the user
    /// closed -- an always-on-top card whose ✕ opens a window is worse than
    /// the silence 3a replaced.
    #[test]
    fn only_the_search_button_asks_for_a_window() {
        assert_eq!(
            no_match_follow_up(overlay_ui::NoMatchAnswer::SearchVault, "Ledgerline.exe"),
            NoMatchFollowUp::SearchVault("Ledgerline".to_string()),
            "`Search vault` asked for nothing, or asked with the wrong query"
        );
        for quiet in [overlay_ui::NoMatchAnswer::Dismissed, overlay_ui::NoMatchAnswer::NewLogin] {
            assert_eq!(
                no_match_follow_up(quiet, "Ledgerline.exe"),
                NoMatchFollowUp::Nothing,
                "{quiet:?} opened the vault window. Only one of 3a's controls may, and this \
                 is not it"
            );
        }
    }

    /// The query is the app's name, **without the `.exe` the card shows**.
    ///
    /// Asserted positively and negatively: a suffix that is not stripped opens
    /// the vault on an empty list, and a strip that fires on anything ending
    /// in four characters would cut real names apart.
    #[test]
    fn the_search_query_drops_an_executable_suffix_and_nothing_else() {
        assert_eq!(search_query("Atlas Licence.exe"), "Atlas Licence");
        assert_eq!(search_query("LEDGERLINE.EXE"), "LEDGERLINE", "the suffix is case-blind");
        assert_eq!(
            search_query("Ledgerline -- Sign in"),
            "Ledgerline -- Sign in",
            "a host frame's window title is not an executable name and must survive whole"
        );
        assert_eq!(
            search_query("bank.example"),
            "bank.example",
            "a four-character suffix that is not `.exe` was cut off"
        );
        assert_eq!(
            search_query(".exe"),
            ".exe",
            "the name was reduced to the empty query, which is not `no query` in a search \
             box -- it matches everything, and the user is handed the whole vault"
        );
        // Multi-byte, because the cut is a byte index: a name whose last four
        // BYTES are inside one character must not be sliced through.
        assert_eq!(search_query("Кошелёк"), "Кошелёк");
        assert_eq!(search_query("Кошелёк.exe"), "Кошелёк");
    }

    /// **The label is `window_label`'s answer, and this card is the one place
    /// that matters most.**
    ///
    /// Every other overlay state also names the item; 3a names nothing but the
    /// app, so a raw `exe_name` for a title-matched Store frame is the entire
    /// message being wrong -- "ApplicationFrameHost.exe" is the exact
    /// complaint that produced `window_label` in the first place.
    ///
    /// `HOST` is a host process, which is the only case in which the two
    /// answers differ; a card told `exe_name` fails here and a card told the
    /// title passes, which is what makes this a test and not a restatement.
    #[test]
    fn the_no_match_card_is_told_the_window_label_and_not_the_exe_name() {
        let w = window(HOST, "Speedtest");
        let presenter = RecordingPresenter::default();

        no_match_arm(&presenter, &w);

        let shown = presenter.no_match_shown.borrow();
        assert_eq!(shown.len(), 1);
        assert_eq!(
            shown[0].0, "Speedtest",
            "the 3a card was told {:?}. That name means nothing to the user, and on this card \
             it is the ONLY thing about their window that is on screen",
            shown[0].0
        );
        assert_ne!(shown[0].0, HOST, "the raw executable name reached the card");
        assert_eq!(
            shown[0].1, None,
            "control: no placement is not a placement of (0, 0) -- a `no_match_arm` that handed \
             the card a fixed pair would pass the placement test above and fail this"
        );
    }

    /// The string spy, pointed at 3a: **no argument the card is given carries
    /// a raw executable name.**
    ///
    /// The broader form of the test above, and it fails on a name reaching any
    /// string this arm passes rather than only the one the test names.
    /// The same claim for 3b, and it is not implied by 3a's. The two arms
    /// each compute the label themselves, so `locked_arm` passing
    /// `&window.exe_name` would leave 3a's test green while putting
    /// "ApplicationFrameHost.exe" on the only line of the locked card that
    /// names the user's window.
    #[test]
    fn nothing_the_locked_card_is_given_carries_the_raw_exe_name() {
        let spy = StringSpy::default();
        locked_arm(&spy, &window(HOST, "Speedtest"));

        let seen = spy.seen.borrow();
        assert!(!seen.is_empty(), "control: the spy recorded nothing, so it observed nothing");
        for s in seen.iter() {
            assert!(
                !s.contains(HOST),
                "the 3b card was handed {s:?}, which carries the frame host's executable name"
            );
        }
        assert!(
            seen.iter().any(|s| s == "Speedtest"),
            "control: the window's real name never reached the card either, so the loop above \
             was asserting about nothing"
        );
    }

    #[test]
    fn nothing_the_no_match_card_is_given_carries_the_raw_exe_name() {
        let spy = StringSpy::default();
        no_match_arm(&spy, &window(HOST, "Speedtest"));

        let seen = spy.seen.borrow();
        assert!(!seen.is_empty(), "control: the spy recorded nothing, so it observed nothing");
        for s in seen.iter() {
            assert!(
                !s.contains(HOST),
                "the 3a card was handed {s:?}, which carries the frame host's executable name"
            );
        }
        assert!(
            seen.iter().any(|s| s == "Speedtest"),
            "control: the window's real name never reached the card either, so the loop above \
             passes for the wrong reason"
        );
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

        /// Design 3a's label lands in the same log as everything else: this
        /// spy exists to catch a raw `exe_name` reaching ANY string the
        /// overlay renders, and 3a's label is one -- indeed it is the only
        /// string that card shows about the window.
        fn show_no_match(
            &self,
            label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::NoMatchAnswer {
            self.seen.borrow_mut().push(label.to_string());
            overlay_ui::NoMatchAnswer::Dismissed
        }

        /// And so does design 3c's -- it names the app in its App row, which
        /// is the row the whole card is built around.
        fn show_save_login(
            &self,
            form: overlay_ui::SaveLoginForm,
            _position: Option<(f32, f32)>,
        ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
            self.seen.borrow_mut().push(form.app_name.clone());
            None
        }

        /// And so does design 3d's: it is the same `window_label` answer, and
        /// this test is about every string the overlay is handed.
        fn show_generate(
            &self,
            label: &str,
            _position: Option<(f32, f32)>,
            _generate: &dyn Fn(
                &crate::vault_bridge::GenerateRequest,
            ) -> Result<zeroize::Zeroizing<String>, String>,
        ) -> Option<zeroize::Zeroizing<String>> {
            self.seen.borrow_mut().push(label.to_string());
            None
        }

        /// And so does design 3b's, for the same reason: it is the only
        /// string that card shows about the window.
        fn show_locked(
            &self,
            label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::LockedAnswer {
            self.seen.borrow_mut().push(label.to_string());
            overlay_ui::LockedAnswer::Dismissed
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

        fn show_no_match(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::NoMatchAnswer {
            self.log.borrow_mut().push("no-match card shown");
            overlay_ui::NoMatchAnswer::Dismissed
        }

        fn show_save_login(
            &self,
            _form: overlay_ui::SaveLoginForm,
            _position: Option<(f32, f32)>,
        ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
            self.log.borrow_mut().push("save-login card shown");
            None
        }

        fn show_generate(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
            _generate: &dyn Fn(
                &crate::vault_bridge::GenerateRequest,
            ) -> Result<zeroize::Zeroizing<String>, String>,
        ) -> Option<zeroize::Zeroizing<String>> {
            self.log.borrow_mut().push("generate card shown");
            None
        }

        fn show_locked(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::LockedAnswer {
            self.log.borrow_mut().push("locked card shown");
            overlay_ui::LockedAnswer::Dismissed
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

    static NO_MATCH_FORWARDED: std::sync::Mutex<Vec<NoMatchShown>> =
        std::sync::Mutex::new(Vec::new());

    fn recording_show_no_match(
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::NoMatchAnswer {
        NO_MATCH_FORWARDED
            .lock()
            .unwrap()
            .push((label.to_string(), position));
        overlay_ui::NoMatchAnswer::Dismissed
    }

    static SAVE_LOGIN_FORWARDED: std::sync::Mutex<Vec<NoMatchShown>> =
        std::sync::Mutex::new(Vec::new());

    fn recording_show_save_login(
        form: overlay_ui::SaveLoginForm,
        position: Option<(f32, f32)>,
    ) -> Option<(overlay_ui::SaveLoginAction, overlay_ui::SaveLoginForm)> {
        SAVE_LOGIN_FORWARDED
            .lock()
            .unwrap()
            .push((form.app_name.clone(), position));
        None
    }

    static GENERATE_FORWARDED: std::sync::Mutex<Vec<NoMatchShown>> =
        std::sync::Mutex::new(Vec::new());

    /// **It calls the `generate` it was handed**, and records what came back,
    /// so the forwarding test can tell a dropped closure argument from a
    /// forwarded one. A field that arrived but was never called would look
    /// identical from the label alone.
    fn recording_show_generate(
        label: &str,
        position: Option<(f32, f32)>,
        generate: &dyn Fn(
            &crate::vault_bridge::GenerateRequest,
        ) -> Result<zeroize::Zeroizing<String>, String>,
    ) -> Option<zeroize::Zeroizing<String>> {
        GENERATE_FORWARDED
            .lock()
            .unwrap()
            .push((label.to_string(), position));
        generate(&crate::vault_bridge::GenerateRequest::Password(
            crate::vault_bridge::PasswordRecipe::default(),
        ))
        .ok()
    }

    static LOCKED_FORWARDED: std::sync::Mutex<Vec<NoMatchShown>> =
        std::sync::Mutex::new(Vec::new());

    fn recording_show_locked(
        label: &str,
        position: Option<(f32, f32)>,
    ) -> overlay_ui::LockedAnswer {
        LOCKED_FORWARDED
            .lock()
            .unwrap()
            .push((label.to_string(), position));
        overlay_ui::LockedAnswer::Dismissed
    }

    /// The forwarding is the only code between [`REAL_OVERLAY`]'s named
    /// functions and the screen, so it is driven here -- swapping two fields,
    /// or dropping an argument, fails.
    #[test]
    fn an_fn_presenter_forwards_to_the_two_functions_it_was_built_from() {
        let presenter = FnPresenter {
            position: recording_position,
            show: recording_show,
            show_no_match: recording_show_no_match,
            show_locked: recording_show_locked,
            show_save_login: recording_show_save_login,
            show_generate: recording_show_generate,
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

        // **The two no-item cards are forwarded to DIFFERENT functions**, and
        // each to its own. `show_no_match` and `show_locked` have identical
        // signatures, so a swap of the two fields -- which would show "no
        // saved login" for a locked vault and "Deskwarden is locked" for a
        // readable one, i.e. exactly the defect this state corrects, inverted
        // -- compiles cleanly. Only reading both logs catches it.
        drop(forwarded);
        presenter.show_no_match("Atlas Licence.exe", Some((5.0, 6.0)));
        presenter.show_locked("Ledgerline.exe", Some((7.0, 8.0)));
        assert_eq!(
            *NO_MATCH_FORWARDED.lock().unwrap(),
            vec![("Atlas Licence.exe".to_string(), Some((5.0, 6.0)))],
            "the 3a card was not forwarded to the 3a function, or was handed the 3b call's \r
             arguments"
        );
        assert_eq!(
            *LOCKED_FORWARDED.lock().unwrap(),
            vec![("Ledgerline.exe".to_string(), Some((7.0, 8.0)))],
            "the 3b card was not forwarded to the 3b function. A card that claims the vault \r
             is locked, shown for a readable vault, is the same lie the locked state exists \r
             to stop -- in the other direction"
        );
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
    /// call itself. `|| cache.get_by_id(..)` bound to a `let item` and passed
    /// as `item.as_ref()` -- the shape this line had until step 5 -- keeps the
    /// plaintext item alive for the whole time the overlay is on screen, and
    /// every behavioural test in this file still passes, because the overlay
    /// is told the same things either way. The difference is residency, and
    /// residency has no observable effect at all from outside this line.
    ///
    /// **Re-cut, not loosened, when the read became `get_by_id`.** The needle
    /// still quotes the whole statement verbatim, `let` binding included, so
    /// it still fails on exactly what it failed on before: a `let item =` on
    /// this line, which can only be there in order to outlive the statement.
    /// The reasoning is unchanged -- what moved is only which cache method the
    /// closure calls, and this needle names that method so a silent revert to
    /// the whole-vault clone is a failure here too, not just a slower fill.
    const LOOKUP: &str = concat!("let lookup = || cache.", "get_by_id(item_id);");
    const REAL_POSITION: &str = concat!("position: ", "overlay_position,");
    const REAL_SHOW: &str = concat!("show: ", "overlay_ui::show_prompt_overlay,");
    /// **The two no-item cards, each named in its own field.**
    ///
    /// `show_no_match` and `show_locked` have identical signatures, so
    /// swapping the two values in [`REAL_OVERLAY`] compiles, warns about
    /// nothing, and inverts the entire correction: a locked vault would be
    /// told "No saved login for <app>" -- the defect 3b exists to remove --
    /// and a readable one would be told Deskwarden is locked while it plainly
    /// was not. `FnPresenter`'s forwarding test cannot see it, because the
    /// swap is in the struct literal it does not build.
    const REAL_SHOW_NO_MATCH: &str =
        concat!("show_no_match: ", "overlay_ui::show_no_match_overlay,");
    const REAL_SHOW_LOCKED: &str = concat!("show_locked: ", "overlay_ui::show_locked_overlay,");
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
        let planted = concat!("            let lookup = || cache.", "get_by_id(item_id);");
        assert_eq!(occurrences(planted, LOOKUP), 1, "planted: {planted}");
        let held = concat!("            let item = cache.", "get_by_id(item_id);");
        assert_eq!(occurrences(held, LOOKUP), 0, "planted: {held}");
        // And the whole-vault clone this line stopped paying for: a revert to
        // it is a revert to 5.66 MB and 46,494 allocations between the
        // keypress and the password, and the needle notices.
        let cloned_whole_vault =
            concat!("            let lookup = || cache.items()", ".into_iter().find(|i| i.id == item_id);");
        assert_eq!(occurrences(cloned_whole_vault, LOOKUP), 0, "planted: {cloned_whole_vault}");

        let planted = concat!("    position: ", "overlay_position,");
        assert_eq!(occurrences(planted, REAL_POSITION), 1, "planted: {planted}");
        let mutated = concat!("    position: ", "|hwnd| overlay_position(hwnd).map(|(x, _y)| (x, 0.0)),");
        assert_eq!(occurrences(mutated, REAL_POSITION), 0, "planted: {mutated}");

        let planted = concat!("    show: ", "overlay_ui::show_prompt_overlay,");
        assert_eq!(occurrences(planted, REAL_SHOW), 1, "planted: {planted}");
        let mutated = concat!("    show: ", "|_label, m, p| overlay_ui::show_prompt_overlay(\"\", m, p),");
        assert_eq!(occurrences(mutated, REAL_SHOW), 0, "planted: {mutated}");

        // The swap, planted both ways round, so each needle is shown to
        // notice the other card's function being put in its field.
        let planted = concat!("    show_no_match: ", "overlay_ui::show_no_match_overlay,");
        assert_eq!(occurrences(planted, REAL_SHOW_NO_MATCH), 1, "planted: {planted}");
        let swapped = concat!("    show_no_match: ", "overlay_ui::show_locked_overlay,");
        assert_eq!(occurrences(swapped, REAL_SHOW_NO_MATCH), 0, "planted: {swapped}");
        let planted = concat!("    show_locked: ", "overlay_ui::show_locked_overlay,");
        assert_eq!(occurrences(planted, REAL_SHOW_LOCKED), 1, "planted: {planted}");
        let swapped = concat!("    show_locked: ", "overlay_ui::show_no_match_overlay,");
        assert_eq!(occurrences(swapped, REAL_SHOW_LOCKED), 0, "planted: {swapped}");

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

        assert_eq!(
            occurrences(source(), REAL_SHOW_NO_MATCH),
            1,
            "expected {REAL_SHOW_NO_MATCH:?} exactly once in app.rs -- `REAL_OVERLAY`'s 3a \
             field. Zero most likely means it has been swapped with the 3b field, which \
             compiles and puts \"No saved login for <app>\" back in front of every user whose \
             vault is merely locked"
        );
        assert_eq!(
            occurrences(source(), REAL_SHOW_LOCKED),
            1,
            "expected {REAL_SHOW_LOCKED:?} exactly once in app.rs -- `REAL_OVERLAY`'s 3b \
             field. Zero means a window with a password field and a locked vault reaches some \
             card other than the one that says so"
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
        // **Re-pinned when the re-prompt was threaded through.** The call
        // gained a ninth argument and no longer fits on one line, so the text
        // this pin quotes had to change with it -- deliberately, and to the
        // *whole* new list rather than to a prefix that would stop noticing
        // where `choice` sits. `reprompt` is pinned here too: it is the last
        // argument, and a call that dropped it would not compile, but a call
        // that passed a *different* scoping would -- and the whole list is
        // what this test exists to hold.
        let needle = concat!(
            "fill_from_vault", "(\n",
            "                    cache, injector, fill_stats, item_id, hwnd, choice, notifier, ",
            "reprompt,\n",
        );
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
    use crate::vault_bridge::{LoginData, VaultField};
    use std::sync::{Arc, Mutex};

    /// **The username and password disagree, and neither contains the other.**
    /// A fixture whose two values agree cannot tell which one was typed.
    const USER: &str = "work.account@contoso.com";
    const PASS: &str = "Zq7-tremulous-BADGER";

    /// The foreground the gated fills below are told they are typing into:
    /// the process `item_with`'s rule names, with a masked control focused.
    /// A value, so no test asks the machine it runs on where the mouse is.
    fn a_masked_box_in_the_rules_process() -> Option<crate::injector::target::SendTarget> {
        Some(crate::injector::target::SendTarget {
            title: "Work Microsoft 365 - Sign in".into(),
            image_name: "msedge.exe".into(),
            pid: 7412,
            class_name: "Chrome_WidgetWin_1".into(),
            focused_is_masked: true,
        })
    }

    /// The same window, with the caret in a box that echoes what is typed.
    fn an_unmasked_box_in_the_rules_process() -> Option<crate::injector::target::SendTarget> {
        Some(crate::injector::target::SendTarget {
            focused_is_masked: false,
            ..a_masked_box_in_the_rules_process().unwrap()
        })
    }

    /// A chat window: the design's own example of where a password must not go.
    fn a_chat_box() -> Option<crate::injector::target::SendTarget> {
        Some(crate::injector::target::SendTarget {
            title: "Slack - #payments-oncall".into(),
            image_name: "slack.exe".into(),
            pid: 9001,
            class_name: "Chrome_WidgetWin_1".into(),
            focused_is_masked: false,
        })
    }

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
                    value: Some(zeroize::Zeroizing::new(m.to_field_value())),
                    other: serde_json::Map::new(),
                },
                VaultField {
                    name: Some("PIN".into()),
                    value: Some(zeroize::Zeroizing::new("4821".to_string())),
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

    /// A cache holding exactly `item`, **with no backend behind it at all**.
    ///
    /// The bridge points at [`crate::test_vault::UNREACHABLE_URL`], which is
    /// dead for the whole life of the process. So a fill that reached for the
    /// network instead of the in-memory snapshot fails visibly rather than
    /// quietly succeeding -- which is the property `fill_from_vault`'s doc
    /// claims and nothing else here checks.
    ///
    /// **This used to be a `mockito` server dropped before the fill ran**, and
    /// that no longer proves anything: mockito 1.7 pools its servers, so
    /// dropping the guard resets the mocks but leaves the port bound and hands
    /// the server to the next test. A fill that reached for the network got an
    /// answer from a recycled server instead of an error -- and, worse, raced
    /// whichever test had since been given that port. A permanently dead
    /// address is both an honest assertion and a fixture that cannot collide
    /// with anything.
    fn cache_with(item: VaultItem) -> VaultCache {
        let cache = crate::test_vault::cache_with_items(vec![item]);
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
    /// The re-prompt scoping for every fill test that is **not about** the
    /// re-prompt: a gate that would allow, and a fresh proof.
    ///
    /// None of the fixtures below carries `reprompt: 1`, so
    /// `permitted_by_reprompt` never even builds a gate for them -- this is
    /// what stops the argument being written as `RepromptGate::unprovable()`
    /// at a dozen call sites and one of them quietly becoming the reason a
    /// test passes. The re-prompt's own tests are
    /// `the_reprompt_gates_the_fill`, and they hand in provers.
    fn ungated(proof: &mut crate::reprompt::Proof) -> Reprompt<'_> {
        Reprompt::with_gate_for(proof, |_| crate::reprompt::RepromptGate::allowing_for_test())
    }

    /// Captures this thread's `log` output for the duration of `f`.
    ///
    /// The warning on the fill's cache-miss path returns nothing and changes
    /// nothing, so there is no other observable for it. The logger is global
    /// and installed once; the buffer is thread-local, so tests running in
    /// parallel cannot see each other's lines. If the install ever loses a
    /// race to some other logger the buffer simply stays empty -- which is why
    /// the test using this asserts on a string it knows the line contains, so
    /// "captured nothing" fails rather than passes.
    fn captured_logs(f: impl FnOnce()) -> Vec<String> {
        use std::cell::RefCell;

        thread_local! {
            static LINES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
        }

        struct Capture;
        impl log::Log for Capture {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                LINES.with(|l| {
                    if let Some(lines) = l.borrow_mut().as_mut() {
                        lines.push(record.args().to_string());
                    }
                });
            }
            fn flush(&self) {}
        }

        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = log::set_boxed_logger(Box::new(Capture));
            log::set_max_level(log::LevelFilter::Trace);
        });

        LINES.with(|l| *l.borrow_mut() = Some(Vec::new()));
        f();
        LINES.with(|l| l.borrow_mut().take().unwrap_or_default())
    }

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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
    /// The mock server here answers **one** route, `/object/totp/item-1`, and
    /// is alive for the whole fill -- which is the point: the code fetch is
    /// the one part of a fill that is deliberately allowed to touch the
    /// network, so it is the one thing left that a server should serve. The
    /// cache itself is seeded in memory (see [`crate::test_vault`]), so a mock
    /// that goes unhit here means the fetch did not happen, not that the
    /// seeding took a different route.
    #[test]
    fn a_sequence_that_uses_a_one_time_code_fetches_it_and_types_it() {
        // Contends for the same process-global "already typing" flag as the
        // fills above (see `injector::sequence_test_lock`). Without this the
        // sequence assertion below fails at random when an unrelated test
        // happens to be holding a `SequenceGuard`.
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let totp = server
            .mock("GET", "/object/totp/item-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":"482913"}}"#)
            .create();

        let cache = crate::test_vault::cache_at(
            server.url(),
            vec![item_with("{USERNAME}{TAB}{TOTP}")],
            Vec::new(),
        );

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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
        let totp = server.mock("GET", "/object/totp/item-1").with_status(200).create();

        let cache = crate::test_vault::cache_at(
            server.url(),
            vec![item_with("{USERNAME}{TAB}{PASSWORD}")],
            Vec::new(),
        );

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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
        let totp = server
            .mock("GET", "/object/totp/item-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":"907142"}}"#)
            .create();

        // Deliberately no stored sequence -- see the doc above.
        let cache =
            crate::test_vault::cache_at(server.url(), vec![item_with("")], Vec::new());
        assert!(
            !sequence_needs_a_one_time_code(&item_with("")),
            "the fixture stores a {{TOTP}}, so the old gate would have fetched anyway and this \
             test would prove nothing"
        );

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("choice-totp");
        fill_from_vault_with(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Totp),
            &sequence::RecordingNotifier::default(),
            &crate::vault_window::preflight::SendGate::describing(
                a_masked_box_in_the_rules_process,
            ),
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
        let totp = server.mock("GET", "/object/totp/item-1").with_status(200).create();

        let cache =
            crate::test_vault::cache_at(server.url(), vec![item_with("")], Vec::new());

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("choice-nototp");
        fill_from_vault_with(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Password),
            &sequence::RecordingNotifier::default(),
            &crate::vault_window::preflight::SendGate::describing(
                a_masked_box_in_the_rules_process,
            ),
            &mut ungated(&mut crate::reprompt::Proof::default()),
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

    /// **The cache miss still falls through to the bridge, and still warns.**
    ///
    /// `fill_from_vault_with` asks the cache for one item rather than cloning
    /// the whole vault to `find` it, and the whole point of `get_by_id`
    /// returning `Option` is that this arm survives the change. A `get_by_id`
    /// that reached the bridge itself, or one whose `None` was turned into a
    /// silent "no item", would leave this test's mock unhit or its log line
    /// missing -- and the warning is deliberate: a miss here is a bug signal
    /// worth noticing rather than swallowing.
    ///
    /// The cache holds `item-1` and the fill asks for `item-404`, so the miss
    /// is real and not an empty-cache artefact. Three things are asserted,
    /// because any one alone can pass for the wrong reason: the bridge was
    /// asked (the mock), the miss was announced (the log), and the fetched
    /// item was actually typed (the recorder), which is what makes this the
    /// fallback rather than an abandoned fill.
    #[test]
    fn a_cache_miss_during_a_fill_reaches_the_bridge_and_logs() {
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let fetched = server
            .mock("GET", "/object/item/item-404")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"data":{"id":"item-404","name":"Only At The Bridge",
                "fields":[],"type":1,
                "login":{"username":"bridge-user","password":"bridge-pass"}}}"#,
            )
            .expect(1)
            .create();

        let cache = crate::test_vault::cache_at(server.url(), vec![item_with("")], Vec::new());
        assert!(
            cache.get_by_id("item-404").is_none(),
            "control: the cache already holds item-404, so there would be no miss to fall back \
             from"
        );
        assert!(
            cache.get_by_id("item-1").is_some(),
            "control: the cache is empty, so this would miss for every id and prove nothing \
             about a miss in particular"
        );

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("cache-miss-fallback");
        let lines = captured_logs(|| {
            fill_from_vault_with(
                &cache,
                &injector,
                &stats,
                "item-404",
                4242,
                FillChoice::Just(key_sequence::FieldRef::Password),
                &sequence::RecordingNotifier::default(),
                &crate::vault_window::preflight::SendGate::describing(
                    a_masked_box_in_the_rules_process,
                ),
                &mut ungated(&mut crate::reprompt::Proof::default()),
            );
        });

        fetched.assert();
        assert!(
            lines.iter().any(|l| l.contains("cache miss for item item-404")),
            "the miss was not announced; logged lines were {lines:?}"
        );
        let sequences = rec.sequences.lock().unwrap();
        assert_eq!(sequences.len(), 1, "the fallback item never reached the sequence path");
        assert_eq!(
            sequences[0].1,
            [Step::Text { text: "bridge-pass".to_string(), rate: sequence::DEFAULT_RATE }],
            "what was typed did not come from the item the bridge answered with"
        );
    }

    /// **Autofill really fills from a snapshot restored off the encrypted
    /// disk file, with `bw serve` genuinely absent.**
    ///
    /// This is the claim `fill_from_vault`'s doc makes -- "the path that makes
    /// autofill work with the backend fully stopped" -- carried the one step
    /// further that `main`'s cache-first startup arm depends on. Every other
    /// fill test above seeds its cache in memory; that proves the fill reads
    /// the snapshot, but it says nothing about whether a snapshot that came
    /// out of an encrypted FILE is a snapshot the fill can serve. A
    /// cache-first launch has no other kind.
    ///
    /// The harness is the whole chain and nothing is stubbed in the middle:
    ///
    ///  1. a `VaultCache` over a real `DiskCache` in a scratch directory
    ///     writes the item to a real encrypted file (the Hello step and DPAPI
    ///     are the `DiskCacheEnv` `fn`-pointer fixtures, so nothing derives a
    ///     hardware key and nothing calls DPAPI -- that is the ONLY
    ///     substitution);
    ///  2. a SECOND, independent `VaultCache` -- built the way `main` builds
    ///     it, before anything spawns a backend -- restores from that file
    ///     with `load_from_disk`;
    ///  3. `fill_from_vault_with` runs against that restored cache and the
    ///     recording filler is asked what it was given.
    ///
    /// **What makes it a real answer is the bridge.** It is
    /// `test_vault::unreachable_bridge` -- `127.0.0.1:9`, refused immediately
    /// and for the life of the process. That is not "a backend that happens
    /// not to be asked"; it is a backend that cannot answer. So a fill that
    /// fell through to `cache.bridge().get_item` -- the documented miss path,
    /// three lines into `fill_from_vault_with` -- gets an error and types
    /// nothing, and this test goes red rather than passing for the wrong
    /// reason. `bw serve` still starting behind the tray is strictly weaker
    /// than this: it would eventually answer.
    ///
    /// Both credentials are asserted, not just the password: a fill that lost
    /// the username still logs nobody in.
    #[test]
    fn autofill_fills_from_a_disk_restored_snapshot_with_no_backend_at_all() {
        let _serialised = crate::injector::sequence_test_lock();
        let dir = crate::vault_disk_cache::tests::temp_dir_for("autofill-cache-first");

        // 1. The session that had a backend, once, and wrote the file.
        let writer = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            crate::vault_disk_cache::tests::cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        let epoch = writer.epoch();
        assert_eq!(
            writer.populate_with_vault(
                crate::vault_cache::VaultSnapshot {
                    items: vec![item_with("")],
                    folders: Vec::new(),
                },
                epoch,
            ),
            crate::vault_cache::PopulateOutcome::Populated,
            "control: the writing session never got populated, so there is nothing to have \
             been written to the file"
        );
        assert!(
            dir.join("vault-cache.bin").exists(),
            "control: no encrypted file was written, so the restore below would be \
             restoring nothing and the fill would be reading a cache seeded in memory"
        );

        // 2. The next launch: a fresh cache, no backend, restore from disk.
        //    This is `main` at the point the cache-first arm decides.
        let restored = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            crate::vault_disk_cache::tests::cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        match restored.load_from_disk() {
            crate::vault_disk_cache::DiskCacheLoad::Loaded { .. } => {}
            other => panic!("the launch did not restore from disk: {other:?}"),
        }
        assert!(
            restored.loaded_from_disk_at().is_some(),
            "control: this is not a from-disk restore, so it is not the state a cache-first \
             launch fills from"
        );

        // 3. The fill, exactly as `main`'s hotkey handler issues it.
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("cache-first-fill");
        fill_from_vault_with(
            &restored,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::UserTabPass,
            &sequence::RecordingNotifier::default(),
            &crate::vault_window::preflight::SendGate::describing(
                a_masked_box_in_the_rules_process,
            ),
            &mut ungated(&mut crate::reprompt::Proof::default()),
        );

        let fills = rec.default_fills.lock().unwrap();
        assert_eq!(
            fills.len(),
            1,
            "autofill typed nothing from a vault restored off the encrypted disk copy. A \
             cache-first launch has a tray, a hotkey and no working autofill until `bw \
             serve` comes up -- which is the whole of what that launch exists to avoid"
        );
        assert_eq!(
            (fills[0].0, fills[0].1.as_str(), fills[0].2.as_str()),
            (4242, USER, PASS),
            "the fill reached the restored snapshot but did not carry the credentials out \
             of it intact"
        );

        // And the restore is still a restore: filling read the snapshot and
        // wrote nothing back, so the pill and the age survive the session's
        // first fill exactly as they survive its startup.
        assert!(
            restored.loaded_from_disk_at().is_some(),
            "a fill cleared the from-disk age, so the vault window opened after one autofill \
             would stop saying the vault came from the cache"
        );
    }

    /// **The gate, driven from the entry point it gates.**
    ///
    /// The routing tests in `vault_window::preflight` prove `dispatch_with`
    /// refuses; this proves the FILL goes through it. A real `fill_from_vault_with`
    /// runs end to end with a foreground that is a value, and the question is
    /// whether the recording filler saw a single keystroke.
    ///
    /// Delete the gate from `fill_from_vault_with`'s sequence arm -- or
    /// neutralise it so the sender runs whatever the gate answered -- and the
    /// two refusal cases below type the password, which is exactly the
    /// survivor `updater::installer_is_launchable` records for the shape of
    /// test that only pins the decision.
    ///
    /// Those two mutations are `mutations/cases/01-gate-deleted` and
    /// `02-gate-neutralised`. **How many tests they redden is not written
    /// down here**, because a number in a comment cannot be re-derived: run
    /// `mutations/run.ps1`, which applies each one to a throwaway worktree and
    /// prints the count and the killing test names. `mutations/README.md`
    /// records the last measured output, and case 02's `about.md` records why
    /// the literal `let _gated = dispatch_with(..);` spelling is NOT the
    /// mutation this paragraph means.
    fn gated_password_fill(
        describe: fn() -> Option<crate::injector::target::SendTarget>,
    ) -> (usize, Vec<String>) {
        let _serialised = crate::injector::sequence_test_lock();
        let cache = crate::test_vault::cache_with_items(vec![item_with("")]);

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("preflight-routing");
        let notifier = sequence::RecordingNotifier::default();
        fill_from_vault_with(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Password),
            &notifier,
            &crate::vault_window::preflight::SendGate::describing(describe),
            &mut ungated(&mut crate::reprompt::Proof::default()),
        );
        let typed = rec.sequences.lock().unwrap().len();
        (typed, notifier.take())
    }

    // ---- the surface is HOSTED, not merely written ------------------------
    //
    // `dispatch_with` refuses bad targets whether or not a modal exists, so
    // every routing test above stays green with the 4b confirmation deleted
    // from `fill_from_vault_with` entirely -- which is the state this crate
    // shipped in at `b05c818`: a tested `draw` that nothing put on screen.
    // These three ask the other question: was the user ASKED, and is the
    // answer obeyed?
    //
    // The recorder is a pair of statics rather than a closure because the seam
    // is an `fn` pointer, for the reason `SendGate`'s doc gives: a seam taking
    // an `impl Fn` could be handed a wrapper and the identity pin below could
    // not see it. The whole module is serialised on `sequence_test_lock`.
    static CONFIRMS_ASKED: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static CONFIRM_SAW_SECRET: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn confirmed(
        state: crate::vault_window::preflight::PreflightState,
        copy: zeroize::Zeroizing<String>,
    ) -> Option<crate::vault_window::preflight::PreflightAction> {
        CONFIRMS_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // What the surface was handed: a step list that says there is a
        // secret, and the very value "Copy instead" would put on the clipboard.
        CONFIRM_SAW_SECRET.store(
            state.has_secret() && copy.as_str() == PASS,
            std::sync::atomic::Ordering::SeqCst,
        );
        Some(crate::vault_window::preflight::PreflightAction::Send)
    }

    fn cancelled(
        _state: crate::vault_window::preflight::PreflightState,
        _copy: zeroize::Zeroizing<String>,
    ) -> Option<crate::vault_window::preflight::PreflightAction> {
        CONFIRMS_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(crate::vault_window::preflight::PreflightAction::Cancel)
    }

    fn must_not_be_asked(
        _state: crate::vault_window::preflight::PreflightState,
        _copy: zeroize::Zeroizing<String>,
    ) -> Option<crate::vault_window::preflight::PreflightAction> {
        panic!("a fill the preflight does not speak for opened a confirmation window");
    }

    /// Drives a whole fill with both halves of the gate as fixtures, and
    /// reports how many times the confirmation was asked, how many sequences
    /// were typed, and what the confirmation was handed.
    ///
    /// **All three are read while the lock is still held**, and handed back by
    /// value. The two counters always were; `CONFIRM_SAW_SECRET` used to be
    /// loaded by the caller *after* this function returned, which is after
    /// `_serialised` had been dropped -- so the next test to take the lock
    /// (`an_ungated_fill_opens_no_confirmation`, whose own `hosted_fill` opens
    /// by storing `false` into it) could clobber the flag in the few
    /// microseconds between the release and the read. `asked` and `typed` were
    /// already snapshotted by then, so exactly the third assertion failed,
    /// intermittently, and only on a machine loaded enough for the waiting
    /// thread to win that race. Returning the flag alongside the counters is
    /// what makes the three facts one observation of one fill.
    fn hosted_fill(
        choice: FillChoice,
        confirm: fn(
            crate::vault_window::preflight::PreflightState,
            zeroize::Zeroizing<String>,
        ) -> Option<crate::vault_window::preflight::PreflightAction>,
    ) -> (usize, usize, bool) {
        let _serialised = crate::injector::sequence_test_lock();
        CONFIRMS_ASKED.store(0, std::sync::atomic::Ordering::SeqCst);
        CONFIRM_SAW_SECRET.store(false, std::sync::atomic::Ordering::SeqCst);
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("preflight-hosting");
        fill_from_vault_with(
            &cache_with(item_with("{USERNAME}{TAB}{PASSWORD}")),
            &injector,
            &stats,
            "item-1",
            4242,
            choice,
            &sequence::RecordingNotifier::default(),
            &crate::vault_window::preflight::SendGate::describing_and_confirming(
                a_masked_box_in_the_rules_process,
                confirm,
            ),
            &mut ungated(&mut crate::reprompt::Proof::default()),
        );
        let typed = rec.sequences.lock().unwrap().len();
        (
            CONFIRMS_ASKED.load(std::sync::atomic::Ordering::SeqCst),
            typed,
            CONFIRM_SAW_SECRET.load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    /// **The hosting, driven from the entry point.** Delete the
    /// `confirmed_by_preflight` call from `fill_from_vault_with` and this is
    /// red at `asked == 1` while every routing test above stays green -- the
    /// hosting isolated from the gating. That is
    /// `mutations/cases/03-confirm-deleted`; `mutations/run.ps1` measures how
    /// much else goes red with it.
    #[test]
    fn a_bare_secret_fill_asks_the_confirmation_before_it_types() {
        let (asked, typed, saw_secret) =
            hosted_fill(FillChoice::Just(key_sequence::FieldRef::Password), confirmed);
        assert_eq!(asked, 1, "the 4b confirmation was never shown");
        assert_eq!(typed, 1, "the confirmed fill did not type, so `asked` proves nothing");
        assert!(
            saw_secret,
            "the surface was handed a step list with no secret in it, or a copy payload that              is not the value this fill was about to type"
        );
    }

    /// And the answer is obeyed. Reading the confirmation's answer and
    /// carrying on regardless -- `let _ = confirmed_by_preflight(..);`, the
    /// neutralisation this crate has measured surviving elsewhere at zero
    /// warnings -- is red here. It is
    /// `mutations/cases/04-confirm-answer-ignored`, and `mutations/run.ps1`
    /// is what says so; this test is the only thing that catches it, which is
    /// the reason it exists separately from the one above.
    #[test]
    fn a_cancelled_confirmation_types_nothing() {
        let (asked, typed, _) =
            hosted_fill(FillChoice::Just(key_sequence::FieldRef::Password), cancelled);
        assert_eq!(asked, 1, "control: the confirmation really was shown");
        assert_eq!(typed, 0, "the fill typed a password the user had just cancelled");
    }

    /// The scope is `preflight_guard_for`'s and not one of its own: a
    /// `UserTabPass` fill opens no window at all. Widening the modal to every
    /// fill would put a hold-to-send in front of the app's ordinary path --
    /// and would ask a masking question those fills deliberately do not answer.
    #[test]
    fn an_ungated_fill_opens_no_confirmation() {
        let (asked, typed, _) = hosted_fill(FillChoice::Saved, must_not_be_asked);
        assert_eq!(asked, 0);
        assert_eq!(typed, 1, "control: the ungated fill really ran");
    }

    /// The production seam, pinned by ADDRESS, exactly as the foreground
    /// lookup beside it is. A `confirm` that was a wrapper -- or a
    /// flag-gated `|_, _| Some(Send)` -- is a different address and fails
    /// here whatever it is spelled, and every test above would still pass.
    #[test]
    fn the_production_gate_hosts_the_real_confirmation_window() {
        let production = crate::vault_window::preflight::SendGate::production();
        assert!(
            std::ptr::fn_addr_eq(
                production.confirm_fn(),
                crate::preflight_host::show_preflight
                    as fn(
                        crate::vault_window::preflight::PreflightState,
                        zeroize::Zeroizing<String>,
                    )
                        -> Option<crate::vault_window::preflight::PreflightAction>
            ),
            "the production gate does not open the real preflight window"
        );
    }

    #[test]
    fn a_password_fill_types_only_when_the_preflight_allows_it() {
        // Positive control on the instrument: with the right window and a
        // masked control the fill really does type, so a count of zero below
        // is a refusal and not a harness that never fills anything.
        let (typed, said) = gated_password_fill(a_masked_box_in_the_rules_process);
        assert_eq!(typed, 1, "the allowed case never typed; said: {said:?}");
        assert!(said.is_empty(), "an allowed fill told the user it was refused: {said:?}");
    }

    #[test]
    fn a_password_fill_types_nothing_when_the_preflight_refuses() {
        // The design's own example: the wrong process in front.
        let (typed, said) = gated_password_fill(a_chat_box);
        assert_eq!(typed, 0, "the password was typed into a chat box");
        assert_eq!(said.len(), 1, "the refusal was silent: {said:?}");

        // The right process, with the caret in a box that echoes.
        let (typed, said) = gated_password_fill(an_unmasked_box_in_the_rules_process);
        assert_eq!(typed, 0, "the password was typed into an unmasked control");
        assert_eq!(said.len(), 1, "the refusal was silent: {said:?}");

        // Nowhere describable at all -- an unknown target is not a safe one.
        let (typed, said) = gated_password_fill(|| None);
        assert_eq!(typed, 0, "the password was typed with no idea where it was going");
        assert_eq!(said.len(), 1, "the refusal was silent: {said:?}");
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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
            &mut ungated(&mut crate::reprompt::Proof::default()),
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
                &mut ungated(&mut crate::reprompt::Proof::default()),
            );
        });

        // Positive control: the fill really did take the default path and
        // really did handle the probe, so a `false` above cannot mean "nothing
        // happened".
        assert_eq!(rec.default_fills.lock().unwrap().len(), 1);
        assert!(!leaked, "the default fill freed the password in the clear");
    }

    // ---- the master-password re-prompt, driven from the entry point --------
    //
    // Every test below counts KEYSTROKES, not answers: `reprompt::need` has
    // its own truth table and it was already green while the fill -- the most
    // consequential exposure in the app, because it types the password into
    // another process -- ignored it completely. So what is asserted here is
    // that the recording filler saw nothing.
    //
    // Both fill arms are exercised. A gate on one of them leaves the other
    // open, and the two are reached by different choices.

    fn a_protected(item: VaultItem) -> VaultItem {
        let mut item = item;
        item.other.insert("reprompt".to_string(), serde_json::json!(1));
        assert!(
            crate::vault_bridge::reprompt_protected(&item),
            "the fixture is not protected, so nothing below is a re-prompt test"
        );
        item
    }

    fn a_scope() -> crate::reprompt::Scope {
        crate::reprompt::Scope::new(
            std::path::PathBuf::from("C:/nowhere"),
            crate::accounts::AccountId::parse("0123456789abcdef0123456789abcdef")
                .expect("a 32-char lowercase hex id"),
        )
    }

    fn allows(_: &std::path::Path, _: &crate::accounts::AccountId) -> Result<(), String> {
        Ok(())
    }

    fn refuses(_: &std::path::Path, _: &crate::accounts::AccountId) -> Result<(), String> {
        Err("the user cancelled the Windows Hello prompt".to_string())
    }

    fn a_satisfied_gesture(
        _: Option<&crate::accounts::AccountId>,
    ) -> crate::reprompt::RepromptGate {
        crate::reprompt::RepromptGate::with_prover(Some(a_scope()), allows)
    }

    fn a_cancelled_gesture(
        _: Option<&crate::accounts::AccountId>,
    ) -> crate::reprompt::RepromptGate {
        crate::reprompt::RepromptGate::with_prover(Some(a_scope()), refuses)
    }

    /// No enrollment: `permit` answers `Cannot` without asking anything. The
    /// prover is still the one that would allow, so a `Cannot` here cannot be
    /// the prover's doing.
    fn no_way_to_ask(_: Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate {
        crate::reprompt::RepromptGate::with_prover(None, allows)
    }

    /// A gate that must never be built. Handed to the unprotected control
    /// below: an item with no `reprompt` flag must not cost a Hello probe,
    /// let alone a gesture.
    fn must_not_be_asked_at_all(
        _: Option<&crate::accounts::AccountId>,
    ) -> crate::reprompt::RepromptGate {
        panic!("an unprotected item built a re-prompt gate");
    }

    /// A whole fill, with the re-prompt's prover as a fixture, reported as
    /// (default fills, sequences typed, what the user was told).
    fn fill_under(
        item: VaultItem,
        choice: FillChoice,
        gate_for: fn(Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate,
    ) -> (usize, usize, Vec<String>) {
        let _serialised = crate::injector::sequence_test_lock();
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("reprompt-routing");
        let notifier = sequence::RecordingNotifier::default();
        let mut proof = crate::reprompt::Proof::default();
        fill_from_vault_with(
            &cache_with(item),
            &injector,
            &stats,
            "item-1",
            4242,
            choice,
            &notifier,
            &crate::vault_window::preflight::SendGate::describing(
                a_masked_box_in_the_rules_process,
            ),
            &mut Reprompt::with_gate_for(&mut proof, gate_for),
        );
        let defaults = rec.default_fills.lock().unwrap().len();
        let sequences = rec.sequences.lock().unwrap().len();
        (defaults, sequences, notifier.take())
    }

    /// **A cancelled gesture types nothing, on either arm.**
    ///
    /// Neutralise the gate -- `let _ = permitted_by_reprompt(..);`, the
    /// shape this crate has measured surviving a whole suite at zero warnings
    /// -- and both halves of this are red.
    #[test]
    fn a_cancelled_reprompt_types_no_part_of_a_protected_item() {
        let (defaults, sequences, told) = fill_under(
            a_protected(item_with("")),
            FillChoice::Just(key_sequence::FieldRef::Password),
            a_cancelled_gesture,
        );
        assert_eq!(sequences, 0, "a cancelled re-prompt typed the password anyway");
        assert_eq!(defaults, 0);

        // The other arm. `UserTabPass` on the same protected item goes to
        // `FillAction::Default`, which is UI Automation and then SendInput --
        // a gate written into the sequence arm alone would leave this open,
        // and this is the arm that types a USERNAME first. A half-typed login
        // is worse than none: the user sees it and retries.
        let (defaults, sequences, _) = fill_under(
            a_protected(item_with("")),
            FillChoice::UserTabPass,
            a_cancelled_gesture,
        );
        assert_eq!(defaults, 0, "a cancelled re-prompt still typed the username and password");
        assert_eq!(sequences, 0);

        // **The refusal reaches the user.** This path routinely runs with no
        // window of ours in front of them, so a silent no-op reads as a
        // broken hotkey.
        assert_eq!(
            told,
            vec![crate::reprompt::refusal_text(false).to_string()],
            "a cancelled re-prompt said nothing, or said the wrong one of the two refusals"
        );
    }

    /// The positive control, on the same fixtures: a *satisfied* gesture and
    /// both arms type. Without this, the zeros above are also what a fill
    /// that never runs anything produces -- and a `return` accidentally left
    /// above the gate would pass every assertion in the test above.
    #[test]
    fn a_satisfied_reprompt_lets_a_protected_item_fill() {
        let (_, sequences, told) = fill_under(
            a_protected(item_with("")),
            FillChoice::Just(key_sequence::FieldRef::Password),
            a_satisfied_gesture,
        );
        assert_eq!(sequences, 1, "a satisfied re-prompt did not let the fill through");
        assert!(told.is_empty(), "an allowed fill told the user it had been refused");

        let (defaults, _, _) = fill_under(
            a_protected(item_with("")),
            FillChoice::UserTabPass,
            a_satisfied_gesture,
        );
        assert_eq!(defaults, 1, "a satisfied re-prompt did not let the default fill through");
    }

    /// **The account that cannot prove is refused, and told how to stop
    /// being one.** `Cannot` and `Refused` are different words, and this is
    /// the one the user can act on.
    #[test]
    fn an_account_with_no_enrollment_does_not_fill_and_says_what_to_turn_on() {
        let (defaults, sequences, told) = fill_under(
            a_protected(item_with("")),
            FillChoice::Just(key_sequence::FieldRef::Password),
            no_way_to_ask,
        );
        assert_eq!((defaults, sequences), (0, 0), "an ungateable item was typed anyway");
        assert_eq!(
            told,
            vec![crate::reprompt::refusal_text(true).to_string()],
            "the refusal a user can act on was not the one they were given"
        );
        assert_ne!(
            crate::reprompt::refusal_text(true),
            crate::reprompt::refusal_text(false),
            "control: the two refusals are distinguishable, so the assertion above means \
             something"
        );
    }

    /// **The negative control the whole feature depends on**: an item with no
    /// `reprompt` flag fills exactly as it always did, and does not even
    /// build a gate -- so no ordinary fill pays for a WinRT round trip, and
    /// a mutation that makes the gate refuse everything is red here rather
    /// than invisible.
    #[test]
    fn an_unprotected_item_fills_without_asking_anything() {
        let (_, sequences, told) = fill_under(
            item_with(""),
            FillChoice::Just(key_sequence::FieldRef::Password),
            must_not_be_asked_at_all,
        );
        assert_eq!(sequences, 1, "an ordinary fill was refused, or never ran");
        assert!(told.is_empty());
    }

    /// **One gesture covers the next fill**, which is what
    /// `reprompt::PROOF_LASTS` is for and what a `Proof` constructed per fill
    /// would silently retire: a user filling a protected item twice in a
    /// minute would meet two Hello dialogs.
    #[test]
    fn a_proof_taken_by_one_fill_covers_the_next() {
        let _serialised = crate::injector::sequence_test_lock();
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("reprompt-window");
        let notifier = sequence::RecordingNotifier::default();
        let cache = cache_with(a_protected(item_with("")));
        let mut proof = crate::reprompt::Proof::default();

        let fill = |gate_for: fn(
            Option<&crate::accounts::AccountId>,
        ) -> crate::reprompt::RepromptGate,
                        proof: &mut crate::reprompt::Proof| {
            fill_from_vault_with(
                &cache,
                &injector,
                &stats,
                "item-1",
                4242,
                FillChoice::Just(key_sequence::FieldRef::Password),
                &notifier,
                &crate::vault_window::preflight::SendGate::describing(
                    a_masked_box_in_the_rules_process,
                ),
                &mut Reprompt::with_gate_for(proof, gate_for),
            );
        };

        fill(a_satisfied_gesture, &mut proof);
        assert_eq!(rec.sequences.lock().unwrap().len(), 1, "the premise: the first fill typed");

        // The second fill is handed a gate that CANNOT ask at all. It types
        // only if the proof the first one recorded is still doing the work.
        fill(no_way_to_ask, &mut proof);
        assert_eq!(
            rec.sequences.lock().unwrap().len(),
            2,
            "the proof from the first fill did not cover the second, so every consecutive \
             fill of a protected item asks again"
        );
    }

    /// **`FillProof` forgets across an account switch.**
    ///
    /// The gate is rebuilt per fill from whoever is active now, but the proof
    /// is not -- so without this a gesture given for one account would cover
    /// a protected item belonging to another for the rest of the minute.
    #[test]
    fn a_proof_does_not_survive_a_change_of_account() {
        let a = crate::accounts::AccountId::parse(&"a".repeat(32)).expect("a 32-char hex id");
        let b = crate::accounts::AccountId::parse(&"b".repeat(32)).expect("a 32-char hex id");
        let now = std::time::Instant::now();

        let mut held = FillProof::default();
        held.scoped_to(Some(&a)).proof.record(now);
        assert_eq!(
            held.scoped_to(Some(&a)).proof.need(true, true, now),
            crate::reprompt::Need::Nothing,
            "the premise: the proof covers the account it was taken for"
        );
        assert_eq!(
            held.scoped_to(Some(&b)).proof.need(true, true, now),
            crate::reprompt::Need::Prove,
            "a proof given for one account still covered another account's protected item"
        );
        // And back to `a` does not resurrect it.
        assert_eq!(
            held.scoped_to(Some(&a)).proof.need(true, true, now),
            crate::reprompt::Need::Prove
        );
    }

    /// **Production goes through the real gate.** The seam is only worth
    /// having if it is not also the shipped answer: a `|_| RepromptGate::
    /// allowing_for_test()` left in `scoped_to` would leave every test above
    /// green and every protected item unprotected. Compared by address, as
    /// `SendGate`'s and `RepromptGate`'s own guards are.
    #[test]
    fn the_production_scoping_is_wired_to_the_real_gate() {
        let mut held = FillProof::default();
        let reprompt = held.scoped_to(None);
        assert!(
            std::ptr::fn_addr_eq(
                reprompt.gate_for,
                crate::reprompt::gate_for_account
                    as fn(Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate
            ),
            "the fill path's re-prompt gate is not `reprompt::gate_for_account`"
        );
    }

    /// **The account really is carried, and it is the one it was given.**
    ///
    /// `gate_from` answers `unprovable()` for `None`, and an unprovable gate
    /// refuses EVERY protected item -- so a `scoped_to` that dropped its
    /// argument on the floor would not be a weaker gate, it would be autofill
    /// switched off for every enrolled user with a protected item. Nothing
    /// else here can see that, because every gate above ignores the account.
    #[test]
    fn the_account_reaches_the_gate_that_would_be_asked() {
        static SAW: std::sync::Mutex<Option<Option<crate::accounts::AccountId>>> =
            std::sync::Mutex::new(None);
        fn record(account: Option<&crate::accounts::AccountId>) -> crate::reprompt::RepromptGate {
            *SAW.lock().unwrap() = Some(account.cloned());
            crate::reprompt::RepromptGate::with_prover(Some(a_scope()), allows)
        }

        let _serialised = crate::injector::sequence_test_lock();
        *SAW.lock().unwrap() = None;
        let id = crate::accounts::AccountId::parse(&"c".repeat(32)).expect("a 32-char hex id");
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("reprompt-account");
        let mut proof = crate::reprompt::Proof::default();
        let mut reprompt = Reprompt::with_gate_for(&mut proof, record);
        reprompt.account = Some(id.clone());
        fill_from_vault_with(
            &cache_with(a_protected(item_with(""))),
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Password),
            &sequence::RecordingNotifier::default(),
            &crate::vault_window::preflight::SendGate::describing(
                a_masked_box_in_the_rules_process,
            ),
            &mut reprompt,
        );
        assert_eq!(
            SAW.lock().unwrap().clone(),
            Some(Some(id)),
            "the gate was built for a different account than the fill was scoped to"
        );
    }
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

/// **Design 3c's three answers, the per-app silence, and the one create
/// route** -- everything Task 3 decides that a test can execute.
#[cfg(test)]
mod save_login_tests {
    use super::*;
    use crate::overlay_ui::{SaveLoginAction, SaveLoginForm};
    use crate::vault_bridge::NewItem;

    const APP: &str = "tracker.exe";

    /// A form as the user would have left it: the app pre-filled, the two
    /// credential rows typed in.
    fn filled() -> SaveLoginForm {
        let mut form = SaveLoginForm::new(APP);
        form.username = "a.novak@ledgerline.com".to_string();
        form.password = zeroize::Zeroizing::new("hunter2-but-longer".to_string());
        form
    }

    /// A recorder for the two effects `route_save_answer` can have.
    #[derive(Default)]
    struct Effects {
        created: std::cell::RefCell<Vec<NewItem>>,
        silenced: std::cell::RefCell<Vec<String>>,
    }

    fn route(effects: &Effects, action: SaveLoginAction) -> SaveOutcome {
        route_save_answer(
            Some((action, filled())),
            |new_item| {
                effects.created.borrow_mut().push(new_item.clone());
                Ok("new-id".to_string())
            },
            |app| effects.silenced.borrow_mut().push(app.to_string()),
        )
    }

    /// **The three answers do three different things**, and in particular
    /// *Not now* and *Never* do not do the same thing.
    ///
    /// This is the assertion Task 3 exists for. The two silences are
    /// indistinguishable at the moment they are chosen -- both close the card
    /// and write nothing to the vault -- and they diverge only the next time
    /// the window is focused, by which point a user who meant *Not now* has no
    /// idea why the card stopped coming and no obvious place to look. So the
    /// difference is made here, in a pure function, where it is counted.
    #[test]
    fn the_three_answers_do_three_different_things() {
        let save = Effects::default();
        let saved = route(&save, SaveLoginAction::Save);
        assert_eq!(saved, SaveOutcome::Created(Ok("new-id".to_string())));
        assert_eq!(save.created.borrow().len(), 1, "Save did not create exactly one item");
        assert!(
            save.silenced.borrow().is_empty(),
            "Save silenced the app as well as saving it, so a user who saved a login for an \
             app would never be offered one for it again"
        );

        let not_now = Effects::default();
        assert_eq!(route(&not_now, SaveLoginAction::NotNow), SaveOutcome::Nothing);
        assert!(not_now.created.borrow().is_empty(), "`Not now` wrote to the vault");
        assert!(
            not_now.silenced.borrow().is_empty(),
            "`Not now` silenced the app forever. That is the one bug on this card a user \
             cannot undo without finding a setting: they said `not now` and got `never`"
        );

        let never = Effects::default();
        assert_eq!(route(&never, SaveLoginAction::Never), SaveOutcome::Silenced(APP.to_string()));
        assert!(
            never.created.borrow().is_empty(),
            "`Never for this app` wrote the login to the vault as well as silencing it"
        );
        assert_eq!(
            *never.silenced.borrow(),
            vec![APP.to_string()],
            "`Never for this app` did not record the app, so the silence lasts until the \
             card is closed and no longer"
        );

        // ...and the three outcomes are three distinct values, which is what
        // lets a caller act on them differently at all.
        assert_ne!(saved, SaveOutcome::Nothing);
        assert_ne!(SaveOutcome::Nothing, SaveOutcome::Silenced(APP.to_string()));
    }

    /// A card that never opened is *not* an answer, and above all it is not a
    /// `Never`.
    ///
    /// `save_login_arm` answers `None` when the overlay refuses to stack a
    /// second window on itself. Reading that as the strongest of the three
    /// answers would silence an app the user was never shown a card for.
    #[test]
    fn a_card_that_never_opened_answers_nothing_and_records_nothing() {
        let mut created = 0;
        let mut silenced = 0;
        let outcome = route_save_answer(
            None,
            |_| {
                created += 1;
                Ok(String::new())
            },
            |_| silenced += 1,
        );
        assert_eq!(outcome, SaveOutcome::Nothing);
        assert_eq!(created, 0, "an overlay that never opened created a vault item");
        assert_eq!(silenced, 0, "an overlay that never opened silenced an app forever");
    }

    /// The four rows land on the four arguments of `NewItem::login`, and the
    /// folder is `None`.
    #[test]
    fn the_four_rows_land_on_the_four_arguments_of_the_create() {
        match new_login_item(filled()) {
            NewItem::Login { name, folder_id, username, password } => {
                assert_eq!(name, APP, "the item is not named after the app it is for");
                assert_eq!(username, "a.novak@ledgerline.com");
                assert_eq!(
                    password, "hunter2-but-longer",
                    "the password the user typed is not the password being saved"
                );
                assert_eq!(
                    folder_id, None,
                    "the new login is filed somewhere, and the card offers no folder to file \
                     it in -- see `overlay_ui::FOLDER_ROW_TEXT`"
                );
            }
            other => panic!("3c created something other than a login: {other:?}"),
        }
    }

    /// An empty form is still a valid create -- `vault_bridge` omits a blank
    /// username or password rather than POSTing `""`, and the name is
    /// pre-filled, so there is nothing here that can produce a nameless item.
    #[test]
    fn an_untouched_form_still_names_the_item_after_the_app() {
        match new_login_item(SaveLoginForm::new(APP)) {
            NewItem::Login { name, username, password, .. } => {
                assert_eq!(name, APP);
                assert_eq!(username, "");
                assert_eq!(password, "");
            }
            other => panic!("3c created something other than a login: {other:?}"),
        }
    }

    /// **A `Never` for one app leaves every other app alone.**
    ///
    /// The whole risk of a per-app silence is that it turns out not to be
    /// per-app: a substring match, a normalisation that collapses two names,
    /// and one press of *Never* stops the overlay everywhere. So the positive
    /// and the negative are both asserted, and the negatives include the two
    /// shapes a loose comparison would let through.
    #[test]
    fn a_never_for_one_app_leaves_every_other_app_alone() {
        let list = vec!["tracker.exe".to_string(), "Ledgerline.exe".to_string()];

        assert_eq!(never_for_app(&list, "tracker.exe"), NeverForApp::Yes);
        // Case: Windows hands the same executable back both ways depending on
        // how it was launched, and a user who silenced one silenced the app.
        assert_eq!(never_for_app(&list, "TRACKER.EXE"), NeverForApp::Yes);
        assert_eq!(never_for_app(&list, "ledgerline.exe"), NeverForApp::Yes);

        // Every other app is untouched...
        assert_eq!(never_for_app(&list, "notepad.exe"), NeverForApp::No);
        // ...including the two a loose comparison would catch: a name the
        // silenced one is a prefix of, and one that contains it.
        assert_eq!(
            never_for_app(&list, "tracker.exe.exe"),
            NeverForApp::No,
            "a name the silenced one is a prefix of was silenced too"
        );
        assert_eq!(
            never_for_app(&list, "not-tracker.exe"),
            NeverForApp::No,
            "a name containing the silenced one was silenced too"
        );
        // And the empty list silences nothing, which is what an older
        // `settings.json` parses as.
        assert_eq!(never_for_app(&[], "tracker.exe"), NeverForApp::No);
    }

    /// **A `Never` suppresses 3a and 3b, and does not touch a match or the
    /// silence control.**
    ///
    /// The decision is argued in `disposition`'s own doc. This is the whole of
    /// it as behaviour, in one place:
    ///
    /// * an unmatched password window that would have shown 3a is silent;
    /// * the same window with the vault locked, which would have shown 3b, is
    ///   silent too -- because 3b appears on exactly the same trigger, so
    ///   exempting it means the silenced app starts popping a card up again
    ///   the moment the vault locks;
    /// * a window the vault *does* match still raises the fill prompt, because
    ///   "do not offer to save this" is not "do not offer to fill what I
    ///   saved"; and
    /// * the ordinary window with no password field is silent either way,
    ///   which is the control that `never` can only ever add silence.
    #[test]
    fn a_never_suppresses_both_cards_that_only_appear_when_nothing_matched() {
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Readable,
                NeverForApp::Yes,
                OverlayPrompts::Shown,
                BrowserWindow::No
            ),
            Open::Nothing,
            "the user pressed `Never for this app` and the no-match card came back anyway. A \
             `never` that still shows the card is not a `never`"
        );
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Locked,
                NeverForApp::Yes,
                OverlayPrompts::Shown,
                BrowserWindow::No
            ),
            Open::Nothing,
            "a silenced app started showing the LOCKED card the moment the vault locked. 3b \
             appears on the same trigger as 3a, so from the user's chair that is the window \
             they turned off coming back on a schedule they cannot see"
        );

        // The positive controls: with `No` in the same three places, both
        // cards are exactly what they were.
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Readable,
                NeverForApp::No,
                OverlayPrompts::Shown,
                BrowserWindow::No
            ),
            Open::NoMatch
        );
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Locked,
                NeverForApp::No,
                OverlayPrompts::Shown,
                BrowserWindow::No
            ),
            Open::Locked
        );
    }

    /// **A `Never` does not switch autofill off for the app.**
    ///
    /// The card the user pressed it on was only ever shown because the vault
    /// had nothing for the window. If they later save a login for it by any
    /// route, focusing it prompts to fill exactly as it would for any other
    /// app: `Matched::Yes` never consults `never`.
    #[test]
    fn a_never_for_an_app_still_fills_it_if_the_vault_gains_a_match() {
        let mut checked = 0;
        for field in [HasPasswordField::Yes, HasPasswordField::No, HasPasswordField::Unknown] {
            for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
                assert_eq!(
                    disposition(Matched::Yes("42"), field, vault, NeverForApp::Yes, OverlayPrompts::Shown, BrowserWindow::No),
                    Open::Match("42"),
                    "a saved login stopped being offered because the user had once said \
                     `never save a login for this app`. Those are different questions, and \
                     only the first one was asked"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 6, "the loop must have covered every field/vault pair");
    }

    /// **The silence control is untouched, and `never` can only add silence.**
    ///
    /// An ordinary window -- a text editor, a file manager -- matches nothing
    /// and has no masked field, and it must stay silent whatever else is true.
    /// Asserted across every combination rather than at one point, because the
    /// claim is that `never` cannot *remove* silence anywhere.
    #[test]
    fn an_ordinary_window_is_still_silence_whatever_the_never_list_says() {
        let mut checked = 0;
        for field in [HasPasswordField::No, HasPasswordField::Unknown] {
            for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
                for never in [NeverForApp::Yes, NeverForApp::No] {
                    assert_eq!(
                        disposition(Matched::No, field, vault, never, OverlayPrompts::Shown, BrowserWindow::No),
                        Open::Nothing,
                        "a window with no match and no password field opened a card"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 8, "the loop must have covered every combination");

        // And the other direction, stated as an ordering rather than as a
        // list: for every input, the `Yes` answer is `Nothing` or the same as
        // the `No` answer. Nothing `never` does can turn silence into a card.
        for matched in [Matched::No, Matched::Yes("7")] {
            for field in [HasPasswordField::Yes, HasPasswordField::No, HasPasswordField::Unknown] {
                for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
                    let without = disposition(matched, field, vault, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No);
                    let with = disposition(matched, field, vault, NeverForApp::Yes, OverlayPrompts::Shown, BrowserWindow::No);
                    assert!(
                        with == Open::Nothing || with == without,
                        "with `never` this window opens {with:?} and without it {without:?}, \
                         so the list is changing WHICH card is shown rather than only \
                         whether one is"
                    );
                }
            }
        }
    }

    /// **The one item-creating route, pinned by source text.**
    ///
    /// `handle_no_match` needs a real vault and opens a real always-on-top
    /// window, so no test may execute it; the create it names is therefore the
    /// one line here nothing can observe. This is the same guard
    /// `prompt_wiring_tests` puts on `REAL_OVERLAY`'s function names, and it
    /// is here for the same reason: a second item-creating route added beside
    /// `VaultCache::create_item` would compile, ship, and bypass every
    /// invariant that one carries (the cache push, the epoch, the era).
    #[test]
    fn the_save_goes_through_the_one_create_route_the_edit_form_uses() {
        let source = include_str!("app.rs");
        // Split literals, in this crate's idiom: a whole needle would match
        // its own declaration.
        let needle = concat!("cache", "\n                .create_item(new_item)");
        let alt = concat!("cache", "\r\n                .create_item(new_item)");
        assert_eq!(
            source.matches(needle).count() + source.matches(alt).count(),
            1,
            "`handle_no_match` no longer creates the new login through \
             `VaultCache::create_item`. That is the route the edit form uses and the only one \
             that pushes the created item into the cache the match engine is rebuilt from -- \
             a create that went straight to `VaultBridge` would save the login and leave \
             autofill blind to it until the next sync"
        );
        // The control: the needle does not match a `bridge.create_item`.
        assert_eq!(
            concat!("bridge", "\n                .create_item(new_item)").matches(needle).count(),
            0,
            "control: the needle matches something other than the cache route"
        );
    }

    /// `describe_outcome` never puts a credential in the log.
    ///
    /// `SaveOutcome` cannot hold the password -- the `Zeroizing` is moved into
    /// the `NewItem` and dropped with it -- so this is a claim about the
    /// remaining strings: the app name is logged, and the username is not,
    /// because a log line naming a window and an account is a record of who
    /// signs in to what.
    #[test]
    fn the_log_line_names_the_app_and_no_credential() {
        let created = describe_outcome(&SaveOutcome::Created(Ok("abc".to_string())));
        assert!(created.contains("abc"), "the log does not say what was saved: {created:?}");

        let silenced = describe_outcome(&SaveOutcome::Silenced(APP.to_string()));
        assert!(silenced.contains(APP), "the log does not say which app: {silenced:?}");

        let nothing = describe_outcome(&SaveOutcome::Nothing);
        assert!(
            !nothing.is_empty() && nothing != silenced,
            "`Not now` and `Never` produce the same log line, so the one record of which was \
             chosen does not distinguish them either"
        );

        for line in [created, silenced, nothing] {
            for secret in ["a.novak@ledgerline.com", "hunter2-but-longer"] {
                assert!(
                    !line.contains(secret),
                    "the log line {line:?} carries {secret:?}, which is a credential the user \
                     typed into an overlay"
                );
            }
        }
    }
}


/// The overlay's second trigger: whether a window with no match, but with a
/// password field, opens the no-match card -- and, above all, whether an
/// ordinary window still opens nothing.
#[cfg(test)]
mod disposition_tests {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    // -- Defect 1: the setting gates every card the overlay raises ---------

    /// **The mapping, in both directions.** Inverted, every user who turned
    /// the prompt off would get every card and every user who left it on
    /// would get none.
    #[test]
    fn the_prompt_setting_maps_to_shown_and_silenced_and_not_the_other_way() {
        assert_eq!(overlay_prompts(true), OverlayPrompts::Shown);
        assert_eq!(overlay_prompts(false), OverlayPrompts::Silenced);
        assert_ne!(overlay_prompts(true), overlay_prompts(false));
    }

    /// **The defect, stated as the user stated it: "keeps popping up even
    /// disabled in settings".**
    ///
    /// With the prompt off, the two cards that appear for a window the vault
    /// does not know must not appear -- in either vault state, because 3b
    /// rides on exactly the same trigger as 3a and an exemption would mean the
    /// card the user switched off came back the moment the vault locked.
    #[test]
    fn the_setting_silences_both_cards_that_only_appear_when_nothing_matched() {
        for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
            assert_eq!(
                disposition(
                    Matched::No,
                    HasPasswordField::Yes,
                    vault,
                    NeverForApp::No,
                    OverlayPrompts::Silenced,
                    BrowserWindow::No,
                ),
                Open::Nothing,
                "the prompt is off in Preferences and an unmatched window still opened a \
                 card ({vault:?}). That is the reported defect: the setting silenced the \
                 overlay for apps the user HAD saved a login for, and left it popping up \
                 for the ones they had not"
            );
        }
    }

    /// **The positive control, and it is the same code path.**
    ///
    /// Every other assertion about this fix is that something does *not*
    /// appear, and a suite of those alone would stay green if the overlay were
    /// deleted outright. This is the ordinary case the card exists for: a
    /// native app, not on the never list, with a password field, an unlocked
    /// vault that simply has nothing for it, and the setting on. It must still
    /// get design 3a.
    #[test]
    fn an_unmatched_native_app_still_gets_the_card_with_the_setting_on() {
        assert_eq!(
            browser_window("ledgerline.exe"),
            BrowserWindow::No,
            "the premise: the app in this test is not a browser"
        );
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Readable,
                never_for_app(&[], "ledgerline.exe"),
                overlay_prompts(true),
                browser_window("ledgerline.exe"),
            ),
            Open::NoMatch,
            "the no-match card has stopped appearing for the one case it is FOR. Both \
             halves of this fix only ever remove a card, so nothing else in this file \
             would notice"
        );
        // And the locked half of the same window, for the same reason.
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Locked,
                never_for_app(&[], "ledgerline.exe"),
                overlay_prompts(true),
                browser_window("ledgerline.exe"),
            ),
            Open::Locked
        );
    }

    /// **The setting does not reach the matched card from here, and that is
    /// the decision.**
    ///
    /// `Open::Match` is gated one layer down by `match_disposition`, on the
    /// line that also arms `CTRL+ALT+B` (`match_arms_hotkey`). Suppressing the
    /// match here as well would take the arming with it, and "prompt off"
    /// would become "autofill off entirely" -- which is what the preference
    /// has always documented itself not to be.
    #[test]
    fn the_setting_leaves_the_matched_arm_to_match_disposition() {
        assert_eq!(
            disposition(
                Matched::Yes("42"),
                HasPasswordField::Unknown,
                VaultAvailability::Readable,
                NeverForApp::No,
                OverlayPrompts::Silenced,
                BrowserWindow::No,
            ),
            Open::Match("42"),
            "the unmatched-card gate has swallowed the matched arm too, so the hotkey \
             arming that shares a line with `match_disposition` is now unreachable"
        );
        // The gate that DOES answer for a matched window, in both directions,
        // so this test cannot be read as "the setting does nothing there".
        assert_eq!(match_disposition(false), MatchDisposition::Nothing);
        assert_eq!(match_disposition(true), MatchDisposition::Prompt);
        assert!(match_arms_hotkey(false), "the hotkey must arm with the prompt off");
    }

    // -- Defect 2: a browser gets no unmatched card ------------------------

    /// **Every name on the list is recognised**, matched whole and
    /// case-insensitively, and nothing near one is.
    #[test]
    fn the_browser_list_is_matched_whole_and_case_insensitively() {
        assert!(!BROWSER_IMAGE_NAMES.is_empty());
        for name in BROWSER_IMAGE_NAMES {
            assert_eq!(browser_window(name), BrowserWindow::Yes, "{name}");
            assert_eq!(browser_window(&name.to_uppercase()), BrowserWindow::Yes, "{name}");
        }
        // Not a substring, not a prefix, not a suffix -- the failure shape
        // `never_for_app` is written against, for the same reason.
        for near in ["notfirefox.exe", "firefox.exe.exe", "firefox", "chrome.exe.bak"] {
            assert_eq!(browser_window(near), BrowserWindow::No, "{near}");
        }
        assert_eq!(browser_window(""), BrowserWindow::No);
    }

    /// **The reported window: a browser with a password field and no match
    /// gets nothing**, with the setting on and no `never` recorded -- so the
    /// silence is the browser rule and not one of the other two.
    #[test]
    fn a_browser_gets_neither_unmatched_card_even_with_the_prompt_on() {
        for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
            assert_eq!(
                disposition(
                    Matched::No,
                    HasPasswordField::Yes,
                    vault,
                    never_for_app(&[], "firefox.exe"),
                    overlay_prompts(true),
                    browser_window("firefox.exe"),
                ),
                Open::Nothing,
                "a browser opened an unmatched card ({vault:?}). Every login page has a \
                 password field and the vault is keyed on executables, so this card cannot \
                 be right there and no saved login would make it stop"
            );
        }
    }

    /// **A browser this build does not recognise is an ordinary app**, and the
    /// assertion is the whole of what happens to it: it keeps today's
    /// behaviour, one card with a *Never for this app* button on it.
    #[test]
    fn an_unrecognised_browser_is_treated_as_an_ordinary_app() {
        assert_eq!(browser_window("palemoon.exe"), BrowserWindow::No, "the premise");
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Readable,
                never_for_app(&[], "palemoon.exe"),
                overlay_prompts(true),
                browser_window("palemoon.exe"),
            ),
            Open::NoMatch
        );
        // And the recourse, which is why that is the safe direction to be
        // wrong in: the card's own `Never` button silences it for good.
        assert_eq!(
            disposition(
                Matched::No,
                HasPasswordField::Yes,
                VaultAvailability::Readable,
                never_for_app(&["palemoon.exe".to_string()], "palemoon.exe"),
                overlay_prompts(true),
                browser_window("palemoon.exe"),
            ),
            Open::Nothing
        );
    }

    /// **A matched browser window still prompts.**
    ///
    /// A rule against a browser in the vault is something the user wrote by
    /// hand against that executable. Refusing to honour it would be this rule
    /// deciding it knows better than an explicit instruction, and unlike the
    /// no-match card there is nothing structurally wrong with the offer: there
    /// IS an item, and Fill types it into the window in front.
    #[test]
    fn a_matched_browser_window_still_prompts() {
        for field in [HasPasswordField::Yes, HasPasswordField::No, HasPasswordField::Unknown] {
            assert_eq!(
                disposition(
                    Matched::Yes("42"),
                    field,
                    VaultAvailability::Readable,
                    NeverForApp::No,
                    overlay_prompts(true),
                    browser_window(BROWSER_IMAGE_NAMES[0]),
                ),
                Open::Match("42"),
                "an app-match rule the user wrote against a browser stopped being honoured"
            );
        }
    }

    /// **The silence control is still pure, and now against four
    /// suppressors.**
    ///
    /// A window with no match and no password field must be silent for reasons
    /// that have nothing to do with any setting, any list and any browser --
    /// and no suppressor may ever turn silence INTO a card. Both halves are
    /// asserted across the whole cross-product, because the claim is about
    /// every combination rather than about a point.
    #[test]
    fn an_ordinary_window_is_still_silence_for_reasons_no_setting_can_change() {
        let mut checked = 0;
        for field in [HasPasswordField::No, HasPasswordField::Unknown] {
            for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
                for never in [NeverForApp::Yes, NeverForApp::No] {
                    for prompts in [OverlayPrompts::Shown, OverlayPrompts::Silenced] {
                        for browser in [BrowserWindow::Yes, BrowserWindow::No] {
                            assert_eq!(
                                disposition(Matched::No, field, vault, never, prompts, browser),
                                Open::Nothing,
                                "a window with no match and no password field opened a card"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 32, "the loop must have covered every combination");

        // Neither new input can turn silence into a card, nor swap which card
        // is shown -- the ordering `never` is already held to.
        for matched in [Matched::No, Matched::Yes("7")] {
            for field in [HasPasswordField::Yes, HasPasswordField::No, HasPasswordField::Unknown] {
                for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
                    let base = disposition(
                        matched,
                        field,
                        vault,
                        NeverForApp::No,
                        OverlayPrompts::Shown,
                        BrowserWindow::No,
                    );
                    let silenced = disposition(
                        matched,
                        field,
                        vault,
                        NeverForApp::No,
                        OverlayPrompts::Silenced,
                        BrowserWindow::No,
                    );
                    let browsed = disposition(
                        matched,
                        field,
                        vault,
                        NeverForApp::No,
                        OverlayPrompts::Shown,
                        BrowserWindow::Yes,
                    );
                    for (with, what) in
                        [(silenced, "the prompt setting"), (browsed, "the browser rule")]
                    {
                        assert!(
                            with == Open::Nothing || with == base,
                            "{what} turned {base:?} into {with:?}, so it is changing WHICH \
                             card is shown rather than only whether one is"
                        );
                    }
                }
            }
        }
    }

    /// **[`BROWSER_IMAGE_NAMES`] is the only place this crate names a
    /// browser.**
    ///
    /// The "two enumerations that must agree" shape is where this codebase
    /// keeps finding defects, so the list is pinned as the single source: each
    /// name appears exactly once in `app.rs` before this block of tests, and
    /// `main` -- the one call site -- names none of them at all. A second list
    /// added anywhere would compile, ship, and disagree with this one on the
    /// first browser either of them forgot.
    #[test]
    fn the_browser_list_is_the_only_place_a_browser_is_named() {
        let source = include_str!("app.rs");
        // Everything from the first test module on is test text -- other
        // modules here name browsers as sample windows, quite properly. The
        // claim is about the half of the file that ships.
        let boundary = source
            .find(concat!("mod ", "tests {"))
            .expect("app.rs's first test module is gone, so this pin no longer knows where                      the shipping half of the file ends");
        let production = &source[..boundary];
        assert!(
            production.contains(concat!("pub fn browser", "_window(exe_name: &str)")),
            "the boundary landed before `browser_window`, so `production` does not contain              the list this test is about"
        );
        for name in BROWSER_IMAGE_NAMES {
            assert_eq!(
                production.matches(*name).count(),
                1,
                "{name} appears more than once in app.rs outside these tests: the list is \
                 no longer the single source of what a browser is"
            );
        }
        let main_rs = include_str!("main.rs");
        for name in BROWSER_IMAGE_NAMES {
            assert_eq!(
                main_rs.matches(*name).count(),
                0,
                "{name} is named in main.rs, which must ask `app::browser_window` rather \
                 than keep a second opinion"
            );
        }
    }


    /// **Written first, and the one that must never be deleted.**
    ///
    /// The whole risk of adding a second trigger is that it becomes "always
    /// open". An ordinary window -- a text editor, a file manager, a browser
    /// on a page with no login -- matches nothing and has no masked field, and
    /// the app's answer to it must stay exactly what it is today: nothing at
    /// all.
    ///
    /// `Unknown` is asserted beside `No` because it is the *same* silence for
    /// a different reason: a `disposition` that answered `NoMatch` on an
    /// unanswerable probe would put a card over every window UI Automation
    /// happens to choke on, which is a superset of the windows this feature is
    /// for.
    #[test]
    fn an_ordinary_window_with_no_password_field_is_still_silence() {
        assert_eq!(
            disposition(Matched::No, HasPasswordField::No, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::Nothing,
            "a window the vault does not know and that asks for no password now raises a card. \
             That is every editor, terminal and file manager the user focuses"
        );
        assert_eq!(
            disposition(Matched::No, HasPasswordField::Unknown, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::Nothing,
            "a window UI Automation could not answer for is being treated as a login window. \
             `Unknown` is the case with the LEAST evidence, and it is being read as the most"
        );
    }

    /// **And the silence control survives the locked state**, which is the
    /// half that a fix for the lying no-match card could most easily have
    /// broken.
    ///
    /// The vault is locked for a large part of a session. If lockedness alone
    /// raised a card, Deskwarden would follow the user from window to window
    /// announcing itself until they unlocked -- which is not a feature, it is
    /// a popup, and it is a worse failure than the one being corrected. The
    /// password field stays the gate, in both vault states.
    #[test]
    fn an_ordinary_window_is_still_silence_in_both_vault_states() {
        let mut checked = 0;
        for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
            for field in [HasPasswordField::No, HasPasswordField::Unknown] {
                assert_eq!(
                    disposition(Matched::No, field, vault, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
                    Open::Nothing,
                    "an ordinary window raised a card with {field:?} and {vault:?}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 4, "the loop must have covered both states of both inputs");
    }

    /// The trigger itself, and its opposite number, so the function is pinned
    /// in both directions rather than only where it must stay quiet.
    #[test]
    fn a_password_field_with_no_match_opens_the_no_match_card() {
        assert_eq!(
            disposition(Matched::No, HasPasswordField::Yes, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::NoMatch
        );
        assert_eq!(
            disposition(Matched::Yes("7"), HasPasswordField::Yes, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::Match("7"),
            "a matched window must still open the matched card -- the item id is what the fill \
             is resolved from"
        );
        assert_ne!(
            disposition(Matched::No, HasPasswordField::Yes, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            disposition(Matched::No, HasPasswordField::No, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            "the premise: the password-field answer actually decides something. Equal answers \
             mean the probe is being paid for and ignored"
        );
    }

    /// **The correction, stated as the assertion that used to be false.**
    ///
    /// `main::stand_down_after_unlock` calls `MatchEngine::clear` on every
    /// lock, and its own log line says what that means: "the app matches are
    /// cleared too, so nothing can prompt to autofill until they are rebuilt".
    /// So with the vault locked `lookup` answers `None` for every window,
    /// `Matched::No` for every window, and the 3a card told the user "No saved
    /// login for <app>" about apps that may very well have one -- a false
    /// statement about their own vault, from the one surface whose entire
    /// purpose is to be trusted about it.
    ///
    /// A locked vault gets [`Open::Locked`] instead, which says only that
    /// Deskwarden is locked: a claim it can support.
    #[test]
    fn a_locked_vault_never_claims_there_is_no_saved_login() {
        assert_eq!(
            disposition(Matched::No, HasPasswordField::Yes, VaultAvailability::Locked, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::Locked,
            "with the vault locked and the engine therefore empty, an unmatched login window \
             still opens the card that asserts there is no saved login for it"
        );
        assert_ne!(
            disposition(Matched::No, HasPasswordField::Yes, VaultAvailability::Locked, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            disposition(Matched::No, HasPasswordField::Yes, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            "the premise: the vault state actually decides something here. Equal answers mean \
             the third input is accepted and ignored, which is the defect unchanged"
        );
    }

    /// **A readable but EMPTY vault still gets 3a, and that is deliberate.**
    ///
    /// The cheapest way to spell "the vault cannot answer" would have been
    /// "the match engine is empty" -- which is also the state of a user whose
    /// vault holds no app matches at all. For them "No saved login for <app>"
    /// is true, useful, and exactly what 3a was built to say.
    /// [`VaultAvailability`] is read from `VaultCache::is_populated` rather
    /// than from the engine for this reason, and this is the case that tells
    /// the two predicates apart.
    #[test]
    fn an_empty_but_readable_vault_still_says_there_is_no_saved_login() {
        assert_eq!(
            disposition(Matched::No, HasPasswordField::Yes, vault_availability(true), NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::NoMatch,
            "a user whose vault genuinely has nothing for this app is now told Deskwarden is \
             locked instead, which is both false and useless to them"
        );
        assert_eq!(
            disposition(Matched::No, HasPasswordField::Yes, vault_availability(false), NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
            Open::Locked
        );
    }

    /// The `bool` -> [`VaultAvailability`] mapping, in both directions.
    ///
    /// Its one call site is inside `main`'s event loop, which no test reaches;
    /// inverted there, every locked vault would report `Readable` and the card
    /// would go straight back to claiming what it cannot know.
    #[test]
    fn a_populated_cache_is_readable_and_an_empty_one_is_locked() {
        assert_eq!(vault_availability(true), VaultAvailability::Readable);
        assert_eq!(vault_availability(false), VaultAvailability::Locked);
        assert_ne!(
            vault_availability(true),
            vault_availability(false),
            "`vault_availability` is ignoring its argument, so the overlay's whole knowledge \
             of whether it may speak about the vault is a constant"
        );
    }

    /// **A match outranks the field answer, whatever it is.**
    ///
    /// This is what licenses the caller to skip the probe entirely on the
    /// matched branch and pass `Unknown`. If any field answer could change a
    /// matched window's disposition, that laziness would be a behaviour change
    /// hiding inside an optimisation.
    #[test]
    fn a_matched_window_ignores_the_field_answer_entirely() {
        for field in [HasPasswordField::Yes, HasPasswordField::No, HasPasswordField::Unknown] {
            assert_eq!(
                disposition(Matched::Yes("42"), field, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
                Open::Match("42"),
                "a matched window's disposition changed with {field:?}, so the probe cannot be \
                 skipped on that branch after all"
            );
        }
    }

    /// **And it ignores the vault state too**, so adding the locked card
    /// changed nothing about what a recognised window does. A match is only
    /// representable when the engine holds entries, which is only true when
    /// the vault was read -- so `Locked` beside `Matched::Yes` is a
    /// contradiction, and the arm resolves it by believing the match rather
    /// than by suppressing the fill the user came for.
    #[test]
    fn a_matched_window_ignores_the_vault_state_too() {
        let mut checked = 0;
        for vault in [VaultAvailability::Readable, VaultAvailability::Locked] {
            for field in [
                HasPasswordField::Yes,
                HasPasswordField::No,
                HasPasswordField::Unknown,
            ] {
                assert_eq!(
                    disposition(Matched::Yes("42"), field, vault, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
                    Open::Match("42"),
                    "a matched window's disposition changed with {field:?} and {vault:?}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 6, "the loop must have covered every combination");
    }

    /// The item id is carried through, not merely "some id": a `disposition`
    /// that answered `Open::Match` with a constant would satisfy every
    /// assertion above that only checks the variant.
    #[test]
    fn the_matched_id_is_the_id_that_comes_back_out() {
        for id in ["7", "42", "a-uuid-shaped-thing"] {
            assert_eq!(
                disposition(Matched::Yes(id), HasPasswordField::No, VaultAvailability::Readable, NeverForApp::No, OverlayPrompts::Shown, BrowserWindow::No),
                Open::Match(id)
            );
        }
    }

    /// A probe that records every window it was asked about, so "was it
    /// called" and "how many times" are observable without COM.
    fn counting_probe(calls: &Cell<usize>, answer: HasPasswordField) -> impl Fn(isize) -> HasPasswordField + '_ {
        move |_hwnd| {
            calls.set(calls.get() + 1);
            answer
        }
    }

    /// **The change gate.** A different window is a different question and is
    /// asked at once -- no interval, no wait. A throttle that made the user
    /// wait to learn about a window it had never seen would be the wrong
    /// throttle.
    #[test]
    fn a_window_never_seen_before_is_probed_immediately() {
        let calls = Cell::new(0);
        let mut probe = PasswordFieldProbe::new();
        let now = Instant::now();

        assert_eq!(probe.ask(1, now, counting_probe(&calls, HasPasswordField::Yes)), HasPasswordField::Yes);
        assert_eq!(calls.get(), 1, "the first window was not probed at all");

        // Same instant, different window: still probed.
        assert_eq!(probe.ask(2, now, counting_probe(&calls, HasPasswordField::No)), HasPasswordField::No);
        assert_eq!(
            calls.get(),
            2,
            "a second, different window was answered from another window's cache entry -- the \
             card would name the wrong window's answer"
        );
    }

    /// **The interval.** The same window inside the TTL is answered from
    /// memory and costs no COM call, because a window that has not changed
    /// cannot have a different answer.
    #[test]
    fn the_same_window_inside_the_ttl_is_not_probed_again() {
        let calls = Cell::new(0);
        let mut probe = PasswordFieldProbe::new();
        let start = Instant::now();

        probe.ask(1, start, counting_probe(&calls, HasPasswordField::Yes));
        assert_eq!(calls.get(), 1, "control: the first ask really did probe");

        let answer = probe.ask(
            1,
            start + PROBE_TTL - Duration::from_millis(1),
            counting_probe(&calls, HasPasswordField::No),
        );
        assert_eq!(
            calls.get(),
            1,
            "the same window was re-probed inside the TTL. At a measured median of 27ms and a \
             p90 of 133ms, on the foreground app's own UI thread, that is the stutter this type \
             exists to prevent"
        );
        assert_eq!(
            answer, HasPasswordField::Yes,
            "the cached answer was not the one that comes back -- the second probe's answer \
             leaked through a path that was supposed to skip it"
        );
    }

    /// And the other side of the interval: once it has elapsed the window IS
    /// asked again, so a page that navigated to a sign-in form in the same
    /// `HWND` is eventually noticed. A cache with no expiry would answer `No`
    /// for that window for the life of the session.
    #[test]
    fn the_same_window_past_the_ttl_is_probed_again_and_the_new_answer_wins() {
        let calls = Cell::new(0);
        let mut probe = PasswordFieldProbe::new();
        let start = Instant::now();

        probe.ask(1, start, counting_probe(&calls, HasPasswordField::No));
        let answer = probe.ask(
            1,
            start + PROBE_TTL,
            counting_probe(&calls, HasPasswordField::Yes),
        );
        assert_eq!(calls.get(), 2, "the entry never expired, so this window can never change its answer");
        assert_eq!(
            answer, HasPasswordField::Yes,
            "the window was re-probed and the STALE answer was returned anyway"
        );

        // And the refreshed entry is the one that is now cached -- one window,
        // one slot, no shadowing by the expired entry it replaced.
        let again = probe.ask(
            1,
            start + PROBE_TTL + Duration::from_millis(1),
            counting_probe(&calls, HasPasswordField::No),
        );
        assert_eq!(calls.get(), 2, "the refreshed entry did not take");
        assert_eq!(again, HasPasswordField::Yes, "the expired entry shadowed the fresh one");
    }

    /// Alternating between two windows -- the commonest thing a user does --
    /// must not cost a probe every time. A one-entry cache would probe four
    /// times here; this is why [`PROBE_MEMORY`] is not 1.
    #[test]
    fn alternating_between_two_windows_probes_each_once() {
        let calls = Cell::new(0);
        let mut probe = PasswordFieldProbe::new();
        let now = Instant::now();

        for hwnd in [1, 2, 1, 2, 1, 2] {
            probe.ask(hwnd, now, counting_probe(&calls, HasPasswordField::Yes));
        }
        assert_eq!(
            calls.get(),
            2,
            "each of the two windows should have been probed exactly once; a one-entry cache \
             probes six times"
        );
    }

    /// The memory is bounded, and the bound evicts the oldest rather than
    /// growing without limit for the life of a session.
    #[test]
    fn the_memory_is_bounded_and_evicts_the_oldest_first() {
        let calls = Cell::new(0);
        let mut probe = PasswordFieldProbe::new();
        let now = Instant::now();

        for hwnd in 0..(PROBE_MEMORY as isize + 1) {
            probe.ask(hwnd, now, counting_probe(&calls, HasPasswordField::Yes));
        }
        assert_eq!(calls.get(), PROBE_MEMORY + 1, "control: every distinct window was probed once");

        // The most recent PROBE_MEMORY windows are still remembered...
        probe.ask(PROBE_MEMORY as isize, now, counting_probe(&calls, HasPasswordField::No));
        assert_eq!(calls.get(), PROBE_MEMORY + 1, "the newest entry was evicted instead of the oldest");

        // ...and the very first one is not.
        probe.ask(0, now, counting_probe(&calls, HasPasswordField::No));
        assert_eq!(
            calls.get(),
            PROBE_MEMORY + 2,
            "window 0 survived past the bound, so the memory grows without limit"
        );
    }
}

/// **The preflight gate, and the fact that it is in a gating POSITION.**
///
/// Two claims, and neither covers the other.
///
/// The routing claim -- that nothing reaches the sender without
/// `Verdict::Allowed` -- is `vault_window::preflight`'s, made by driving
/// `dispatch_with` end to end behind its seam and asking whether the sender
/// RAN. That is what kills a deleted or neutralised refusal branch, which a
/// pin on `verdict` alone cannot see; `updater::installer_is_launchable`
/// records the measured survivor that taught this crate the difference.
///
/// The claim HERE is the other half: that `fill_from_vault`'s sequence arm
/// reaches the sender only *through* that function. `fill_from_vault` needs a
/// `VaultCache`, an `Injector` and a `FillStats` and nothing in this crate can
/// call it, so this is a source pin, said plainly, with positive controls on
/// every needle. Move the `injector.fill_sequence` call out of the closure and
/// this fails.
#[cfg(test)]
mod preflight_call_site_tests {

    /// This file with every top-level gated module removed -- including this
    /// one, which spells every needle below. Same line-based cut, and the same
    /// reasons, as `fill_call_site_tests::production_only`.
    fn production() -> String {
        let mut out = String::new();
        let mut skipping = false;
        for line in include_str!("app.rs").lines() {
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

    const SENDER: &str = concat!("injector.fill_", "sequence(");
    const GATED: &str = concat!("|| injector.fill_", "sequence(hwnd, plan, sink)");
    const GATE: &str = concat!("preflight::dispatch_", "with(");

    #[test]
    fn the_sequence_sender_is_reached_only_from_inside_the_gate() {
        let production = production();
        assert!(
            production.len() < include_str!("app.rs").len(),
            "control: the cut removed nothing, so this pin is reading its own fixtures"
        );
        assert_eq!(
            production.matches(GATE).count(),
            1,
            "there must be exactly one gated dispatch in this file"
        );
        assert_eq!(
            production.matches(SENDER).count(),
            1,
            "the sequence sender is called more than once, so one of the calls is ungated"
        );
        assert_eq!(
            production.matches(GATED).count(),
            1,
            "the one call to the sequence sender is not the closure handed to the gate"
        );
        // The gate is what the call is INSIDE, not merely a line above it: the
        // closure is the sender's whole text, so the sender cannot run unless
        // the closure is called, and only the allowed arm calls it.
        let at_gate = production.find(GATE).expect("the gate is in this file");
        let at_send = production.find(SENDER).expect("the sender is in this file");
        assert!(at_gate < at_send, "the sender is called before the gate is even consulted");

        // Positive control on all three needles: they match the spellings they
        // are meant to match, so a count of 1 is a real call site rather than
        // a typo that matches nothing.
        let fixture = concat!(
            "let gated = crate::vault_window::preflight::dispatch_",
            "with(\n",
            "    &gate, guard, || injector.fill_",
            "sequence(hwnd, plan, sink),\n);\n"
        );
        assert_eq!(fixture.matches(GATE).count(), 1);
        assert_eq!(fixture.matches(SENDER).count(), 1);
        assert_eq!(fixture.matches(GATED).count(), 1);
    }
}

/// Which fills the preflight speaks for. See [`preflight_guard_for`] for why
/// the multi-step sequences are deliberately outside it.
#[cfg(test)]
mod preflight_guard_tests {
    use super::*;
    use crate::vault_window::preflight::Guard;

    #[test]
    fn a_bare_secret_typed_at_the_caret_is_gated_and_carries_the_items_rule() {
        for field in [key_sequence::FieldRef::Password, key_sequence::FieldRef::Totp] {
            assert_eq!(
                preflight_guard_for(&FillChoice::Just(field.clone()), Some("saplogon.exe")),
                Guard::Preflight { rule_image: Some("saplogon.exe") },
                "{field:?} types a credential into whatever holds focus and must be gated"
            );
            assert_eq!(
                preflight_guard_for(&FillChoice::Just(field), None),
                Guard::Preflight { rule_image: None },
                "an item with no rule is still gated, on the masking half"
            );
        }
    }

    /// The other choices are NOT gated, and this states the reason as a test
    /// rather than only in prose: both begin by typing a username into a field
    /// that is not masked, so `verdict`'s `NotMasked` arm would refuse every
    /// one of them -- including the design's own 4b illustration.
    #[test]
    fn a_sequence_that_starts_by_typing_a_username_is_not_gated_on_masking() {
        for choice in [
            FillChoice::UserTabPass,
            FillChoice::Saved,
            FillChoice::Just(key_sequence::FieldRef::Username),
        ] {
            assert_eq!(
                preflight_guard_for(&choice, Some("saplogon.exe")),
                Guard::NotRequired,
                "{choice:?} would be refused on every legitimate fill"
            );
        }
    }
}

/// **Design 3d's arm and the 3c/3d loop.**
///
/// Everything here is driven through a recording presenter, because the two
/// real cards each call `eframe::run_native` and no test in this crate may
/// execute one. What is asserted is the shape of the handoff: which card is
/// opened, what it is handed, and what survives crossing between them.
#[cfg(test)]
mod generate_flow_tests {
    use super::*;
    use crate::overlay_ui::{SaveLoginAction, SaveLoginForm};
    use crate::vault_bridge::GenerateRequest;

    const APP: &str = "ledgerline.exe";
    const TYPED: &str = "a.novak@ledgerline.com";
    const GENERATED: &str = "tq7Rvk29mzpLx4hd8";

    fn window() -> crate::window_watch::ForegroundEvent {
        crate::window_watch::ForegroundEvent {
            hwnd: 4242,
            pid: 99,
            exe_name: APP.to_string(),
            title: String::new(),
        }
    }

    /// A generator that never touches a network: it answers `GENERATED` for
    /// any request and records what it was asked for.
    #[derive(Default)]
    struct Generator {
        asked: std::cell::RefCell<Vec<GenerateRequest>>,
    }

    impl Generator {
        fn call(&self) -> impl Fn(&GenerateRequest) -> Result<zeroize::Zeroizing<String>, String> + '_ {
            move |request| {
                self.asked.borrow_mut().push(request.clone());
                Ok(zeroize::Zeroizing::new(GENERATED.to_string()))
            }
        }
    }

    /// What one scripted card answers with.
    enum Answer {
        /// 3c answers with this action, having had `typed` put in its
        /// username row first -- which is how "the username survives the hop"
        /// becomes observable.
        Card(SaveLoginAction, Option<&'static str>),
    }

    /// A presenter that answers 3c from a script and 3d with a fixed
    /// password, recording every form it was handed.
    struct Script {
        answers: std::cell::RefCell<Vec<Answer>>,
        /// The `(app_name, username, password)` of every form 3c was OPENED
        /// with -- not the ones it answered. This is the side the handoff is
        /// visible from.
        opened_with: std::cell::RefCell<Vec<(String, String, String)>>,
        /// The rows each card was placed for, in call order.
        rows: std::cell::RefCell<Vec<usize>>,
        /// What 3d answers: `None` means the user dismissed it.
        generated: Option<&'static str>,
        /// How many times 3d was opened.
        generate_calls: std::cell::Cell<usize>,
    }

    impl Script {
        fn new(answers: Vec<Answer>, generated: Option<&'static str>) -> Self {
            Self {
                answers: std::cell::RefCell::new(answers.into_iter().rev().collect()),
                opened_with: std::cell::RefCell::new(Vec::new()),
                rows: std::cell::RefCell::new(Vec::new()),
                generated,
                generate_calls: std::cell::Cell::new(0),
            }
        }
    }

    impl PromptPresenter for Script {
        fn position(&self, _hwnd: isize, rows: usize) -> Option<(f32, f32)> {
            self.rows.borrow_mut().push(rows);
            Some((1.0, 2.0))
        }

        fn show(
            &self,
            _label: &str,
            _matched: Option<&overlay_ui::OverlayMatch>,
            _position: Option<(f32, f32)>,
            _choices: &[FillChoice],
        ) -> Option<FillChoice> {
            unreachable!("the 3d flow never opens the matched card")
        }

        fn show_no_match(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::NoMatchAnswer {
            unreachable!("the 3d flow never opens 3a")
        }

        fn show_save_login(
            &self,
            form: SaveLoginForm,
            _position: Option<(f32, f32)>,
        ) -> Option<(SaveLoginAction, SaveLoginForm)> {
            self.opened_with.borrow_mut().push((
                form.app_name.clone(),
                form.username.clone(),
                form.password.to_string(),
            ));
            let Answer::Card(action, typed) = self
                .answers
                .borrow_mut()
                .pop()
                .expect("the flow opened 3c more times than the script has answers");
            let mut answered = form;
            if let Some(typed) = typed {
                answered.username = typed.to_string();
            }
            Some((action, answered))
        }

        fn show_generate(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
            generate: &dyn Fn(&GenerateRequest) -> Result<zeroize::Zeroizing<String>, String>,
        ) -> Option<zeroize::Zeroizing<String>> {
            self.generate_calls.set(self.generate_calls.get() + 1);
            // The card really does ask the generator it was handed, so a
            // presenter argument that was accepted and dropped is visible in
            // the `Generator`'s own log.
            let _ = generate(&GeneratedKindRequest::default_request());
            self.generated
                .map(|p| zeroize::Zeroizing::new(p.to_string()))
        }

        fn show_locked(
            &self,
            _label: &str,
            _position: Option<(f32, f32)>,
        ) -> overlay_ui::LockedAnswer {
            unreachable!("the 3d flow never opens 3b")
        }
    }

    /// The request the scripted 3d makes, so the test's generator has
    /// something real to be asked for. It is the card's own opening recipe.
    struct GeneratedKindRequest;
    impl GeneratedKindRequest {
        fn default_request() -> GenerateRequest {
            overlay_ui::GeneratedKind::Characters
                .recipe(overlay_ui::GeneratedKind::Characters.default_size())
        }
    }

    /// **The handoff, in both directions.**
    ///
    /// 3c is opened blank; the user types a username and clicks *Generate*;
    /// 3d answers a password; 3c is opened **again** carrying both. That
    /// second opening is the whole feature -- an arm that rebuilt the form
    /// from the window would open it blank and throw away both -- and it is
    /// asserted on the form the presenter was HANDED rather than on the one
    /// it answered with, because the handed one is what the user would see.
    #[test]
    fn the_generated_password_and_the_typed_username_both_reach_3c() {
        let generator = Generator::default();
        let script = Script::new(
            vec![
                Answer::Card(SaveLoginAction::Generate, Some(TYPED)),
                Answer::Card(SaveLoginAction::Save, None),
            ],
            Some(GENERATED),
        );

        let answer = save_login_flow(&script, &window(), &generator.call());
        let (action, form) = answer.expect("the flow answered nothing");
        assert_eq!(action, SaveLoginAction::Save);
        assert_eq!(form.password.as_str(), GENERATED, "the flow's answer lost the password");
        assert_eq!(form.username, TYPED, "the flow's answer lost the username");

        let opened = script.opened_with.borrow();
        assert_eq!(opened.len(), 2, "3c was not re-opened after the generator");
        assert_eq!(
            opened[0],
            (APP.to_string(), String::new(), String::new()),
            "3c did not open blank the first time, so it is claiming a capture it did not make"
        );
        assert_eq!(
            opened[1],
            (APP.to_string(), TYPED.to_string(), GENERATED.to_string()),
            "3c re-opened without what the user typed and what 3d generated"
        );
        assert_eq!(script.generate_calls.get(), 1, "3d was opened other than once");
        assert_eq!(
            generator.asked.borrow().len(),
            1,
            "the generator the flow was handed was never called, so a presenter that \
             accepted the argument and dropped it would pass"
        );
    }

    /// **Each card is placed for its own height.**
    ///
    /// The clamp onto the monitor's work area is computed from the card's
    /// height, so asking about the card the user just left puts the other
    /// one's controls under the taskbar for any window anchored near the
    /// bottom of the screen. 3c and 3d are different heights, and this is
    /// what fails if the flow asks about the wrong one.
    #[test]
    fn each_card_is_placed_for_the_card_it_opens() {
        let generator = Generator::default();
        let script = Script::new(
            vec![
                Answer::Card(SaveLoginAction::Generate, None),
                Answer::Card(SaveLoginAction::NotNow, None),
            ],
            Some(GENERATED),
        );
        let _ = save_login_flow(&script, &window(), &generator.call());
        assert_eq!(
            *script.rows.borrow(),
            vec![
                overlay_ui::SAVE_LOGIN_ROWS,
                overlay_ui::GENERATE_ROWS,
                overlay_ui::SAVE_LOGIN_ROWS
            ],
            "the flow asked for a placement sized by the wrong card"
        );
        // The control that makes the sequence above mean something: the two
        // constants really are different numbers, so a flow that used one for
        // both would fail rather than coincide.
        assert_ne!(
            overlay_ui::SAVE_LOGIN_ROWS,
            overlay_ui::GENERATE_ROWS,
            "3c and 3d ask for the same window, so this test cannot tell them apart"
        );
    }

    /// **A dismissed generator changes nothing.**
    ///
    /// The user clicked *Generate*, thought better of it and pressed Esc. The
    /// password row must be exactly as they left it -- an Esc that wrote an
    /// empty string over a typed password would destroy work by cancelling.
    #[test]
    fn a_dismissed_generator_leaves_the_form_alone() {
        let generator = Generator::default();
        let script = Script::new(
            vec![
                Answer::Card(SaveLoginAction::Generate, Some(TYPED)),
                Answer::Card(SaveLoginAction::NotNow, None),
            ],
            // The user dismissed 3d.
            None,
        );

        let _ = save_login_flow(&script, &window(), &generator.call());
        let opened = script.opened_with.borrow();
        assert_eq!(opened.len(), 2);
        assert_eq!(
            opened[1],
            (APP.to_string(), TYPED.to_string(), String::new()),
            "a dismissed generator did not return to the form the user had typed into"
        );
    }

    /// **A presenter that only ever generates does not spin forever.**
    ///
    /// The loop's continuation condition is an answer from a presenter, and
    /// this is the presenter that never advances. Without
    /// [`GENERATE_HOPS`] this test hangs; with it the flow gives up and
    /// answers the weakest of 3c's answers.
    #[test]
    fn a_presenter_that_only_ever_generates_does_not_spin_forever() {
        let generator = Generator::default();
        let script = Script::new(
            (0..GENERATE_HOPS)
                .map(|_| Answer::Card(SaveLoginAction::Generate, None))
                .collect(),
            Some(GENERATED),
        );

        let (action, _form) = save_login_flow(&script, &window(), &generator.call())
            .expect("the flow answered nothing");
        assert_eq!(
            action,
            SaveLoginAction::NotNow,
            "running out of hops answered something other than the weakest answer the card \
             offers; `Never` is not a thing to reach by exhaustion"
        );
        assert_eq!(
            script.generate_calls.get(),
            GENERATE_HOPS,
            "the flow did not stop at exactly GENERATE_HOPS trips to the generator"
        );
    }

    /// **A `Generate` that escapes the flow writes nothing.**
    ///
    /// [`save_login_flow`] resolves *Generate* itself and never returns it,
    /// so this is the arm that is unreachable by construction -- which is
    /// exactly why it is asserted rather than assumed. If a later change let
    /// one through, the answer must be "nothing", not a vault write and not a
    /// permanent silence.
    #[test]
    fn a_generate_that_escapes_the_flow_writes_nothing() {
        let created = std::cell::RefCell::new(0);
        let silenced = std::cell::RefCell::new(0);
        let outcome = route_save_answer(
            Some((SaveLoginAction::Generate, SaveLoginForm::new(APP))),
            |_| {
                *created.borrow_mut() += 1;
                Ok("id".to_string())
            },
            |_| *silenced.borrow_mut() += 1,
        );
        assert_eq!(outcome, SaveOutcome::Nothing);
        assert_eq!(*created.borrow(), 0, "a `Generate` created a vault item");
        assert_eq!(*silenced.borrow(), 0, "a `Generate` silenced the app forever");
    }
}

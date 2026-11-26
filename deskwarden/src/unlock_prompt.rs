//! **The daemon's unlock prompt: the app asking one question without
//! launching the app.**
//!
//! When the vault is locked and the user asks for a fill -- `CTRL+ALT+B`, or
//! the overlay -- the only way to type a master password today is to open the
//! full egui window. `docs/superpowers/specs/2026-08-21-daemon-and-ui-split-design.md`
//! measures what that costs: a process that creates an egui window commits
//! **~95 MB of OpenGL driver arenas that nothing releases** -- eframe destroys
//! the GL context and the driver keeps them -- and it **ratchets ~4 MB per
//! open/close cycle**. A process that never creates one costs 1.61 MB. So the
//! commonest interaction a locked vault has is also its most expensive, and
//! the expense buys one text field.
//!
//! This module is that one text field, drawn in bare Win32. No `eframe`, no
//! `glow`, no GL context, no renderer module loaded at any stage.
//!
//! # This is a NEW surface, not a replacement. Do not "unify" it.
//!
//! Design turn 7's first window -- `app_window`, with the vault window's own
//! frame and its loading/slow/unreachable bodies -- is **the UI launching**.
//! It is the app starting up, on its way to showing the vault, and every one
//! of its states is a report on that journey. It should stay exactly as it is.
//!
//! This is **the daemon asking a question**. Nothing is launching. There is no
//! vault window behind it, no backend coming up, no journey to report on, and
//! when it is answered the thing that happens is a *fill into somebody else's
//! window*. The two surfaces look similar on purpose -- they are the same
//! product -- and they are structurally opposite: one is a window that becomes
//! an application, the other is a question that must leave no window behind.
//!
//! Merging them would mean the daemon links `eframe`, which is the entire
//! 95 MB this exists to not spend. If a later change makes these two look like
//! duplication, the duplication is the feature.
//!
//! # What it does NOT own
//!
//! It does not unlock anything itself. It collects a master password and hands
//! it to [`crate::login_ui::authenticate_then_wipe`] -- the same function
//! `login_ui::spawn_auth` calls, against the same profile directory and with
//! the same two arms behind it. There is exactly one route from a master
//! password to an unlocked vault in this app and this is not a second one; it
//! is a second way to *ask*.
//!
//! **Both arms, and that is a correction.** This used to name
//! `run_bw_with_password` -- the CLI arm alone -- with no consultation of
//! [`crate::backend_policy`] anywhere on the path. The locked card that opens
//! this prompt is raised on `VaultCache::is_populated` alone (see
//! `app::disposition`, which takes no backend input at all), so it is raised
//! on direct-REST accounts too -- and there `bw unlock` addresses a profile
//! that was never signed in to, on a machine that need not have a `bw.exe`.
//! See [`run_bw_unlock`].
//!
//! # The password, honestly
//!
//! The buffer this module owns is `Zeroizing` end to end: the `Vec<u16>`
//! `WM_GETTEXT` copies into, and the `String` built from it. Neither is handed
//! to an allocator's free list intact.
//!
//! **There is a copy this module cannot wipe, and it is a real regression
//! against the egui side.** `WM_GETTEXT` copies *out of* the `EDIT` control's
//! own internal buffer, which comctl32 allocated and still owns. `ES_PASSWORD`
//! masks the *display*, not the storage. [`Win32::take_password`] overwrites
//! the control with an equal-length run of filler before the window is
//! destroyed, and that is **best effort in the strict sense**: `SetWindowTextW`
//! is free to release the old allocation and take a new one rather than
//! overwrite in place, and nothing in the API says which it did. On the egui
//! side the string is a `String` the app owns and can zeroize with certainty.
//! Here it is not. The mitigation is real but partial, and it should be
//! described that way and not as "the password is wiped".
//!
//! # Screen-capture exclusion goes on the TOP-LEVEL window
//!
//! `SetWindowDisplayAffinity` is **refused on a child `EDIT`** with
//! `E_INVALIDARG` -- measured, not assumed. It must go on the top-level
//! window, which then covers every child it owns. Three-way pixel capture
//! during the spike put the excluded window at 100% different from an
//! unprotected one and 0.0% different from one that was not on screen at all.
//! [`run_with`] therefore calls `protect` on the top-level handle before the
//! field can be typed into, and
//! `the_capture_exclusion_goes_on_the_top_level_window` asserts that the call
//! is made -- not that a constant exists somewhere.
//!
//! # It takes the foreground, deliberately
//!
//! Unlike `overlay_ui`, which is anchored beside somebody else's field and
//! must not steal focus, this window is a question the user has to type into.
//! It appears on top and takes focus. That costs the fill nothing:
//! `injector::send_input::ensure_foreground` restores the target window before
//! any keystroke is sent, and `foreground`'s own tests already record that
//! `SetForegroundWindow` is allowed to refuse.
//!
//! # Where the seam is
//!
//! No test in this crate may create a window, register a hotkey or call `bw`.
//! So the **decision** -- open, protect, read, gate, attempt, report, close --
//! is [`run_with`], which is pure over a struct of `fn` pointers, and the
//! Win32 calls are [`Win32`], which holds no decisions. `cfg(test)` seams are
//! banned crate-wide; the fakes below are ordinary `fn`s recording into
//! statics, which is this crate's idiom.

use std::sync::atomic::{AtomicBool, AtomicIsize};

use zeroize::Zeroizing;

/// The window's title.
///
/// **Unique across this process**, and that is load-bearing rather than
/// cosmetic: `foreground::pick` finds a window by title and takes the FIRST
/// match in `EnumWindows` order, and this window is provably alive alongside
/// others -- it is opened from the daemon while a tray icon and a hotkey
/// listener already exist. `only_one_window_of_this_process_can_exist_at_a_time`
/// asserts it differs from every other title this crate opens under.
pub const UNLOCK_PROMPT_TITLE: &str = "Unlock Deskwarden";

/// The two handles [`run_with`] deals in: the top-level window, and the
/// password field inside it.
///
/// Both are raw `isize`, not `HWND`, for the same reason `foreground::OwnWindow`
/// carries an `isize`: a decision layer a test can drive must not name a type
/// that only exists behind a Win32 feature gate.
///
/// **Two fields and not one**, because the difference between them is the one
/// mistake this surface can make silently: `SetWindowDisplayAffinity` on
/// `field` is refused, and a refusal this code ignored would leave the master
/// password visible to every screen recorder on the machine with nothing on
/// screen to say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptWindow {
    pub top_level: isize,
    pub field: isize,
}

/// What the user did with the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Unlock, or Enter in the field.
    Submit,
    /// Cancel, Escape, or the close glyph.
    Cancel,
    /// The window went away underneath us -- destroyed by something other
    /// than a button. Treated exactly as `Cancel`: nothing is armed.
    Closed,
}

/// How [`run_with`] finished.
///
/// **`Debug` is hand-written and redacts the token.** It is not a `Zeroizing`,
/// so `debug_leak_guard` would not flag a derived one -- which is precisely
/// the hole that guard's own module doc names as its biggest ("a secret that
/// is not a `Zeroizing`"). A `BW_SESSION` token decrypts the whole vault; it
/// does not go in a log line because a `{:?}` was convenient.
#[derive(Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The vault is open. Carries the session token, which the caller
    /// persists exactly as `reauthenticate` does.
    Unlocked(String),
    /// The user declined. Nothing is armed and nothing was left behind.
    Cancelled,
    /// The window could not be put on screen at all. Distinguished from
    /// `Cancelled` because a caller may want to fall back to the full window,
    /// which is a different thing from the user saying no.
    Unavailable,
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Unlocked(_) => f.write_str("Unlocked(<session token redacted>)"),
            Outcome::Cancelled => f.write_str("Cancelled"),
            Outcome::Unavailable => f.write_str("Unavailable"),
        }
    }
}

/// Whether a submit is worth spending a `bw` spawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submit {
    /// Nothing to send. Carries the line to put under the field.
    Refuse(&'static str),
    /// Send it.
    Attempt,
}

/// The gate in front of the CLI.
///
/// Borrowed from [`crate::login_ui::missing_credential_message`] rather than
/// restated: `bw` takes the master password through `--passwordenv`, and on
/// Windows an environment variable set to the empty string is indistinguishable
/// from one that was never set -- so a blank field produces the CLI complaining
/// about *our own plumbing* ("Provided passwordenv DESKWARDEN_BW_PASSWORD is
/// not set") instead of telling the user they left a box empty. That is the
/// login window's finding; this surface has the same field and would have
/// reproduced it exactly.
///
/// `BwStatus::Locked` and not `Unauthenticated`: this prompt exists only for a
/// vault that is locked. A signed-out account cannot be unlocked by any
/// password, and the daemon has nowhere to put an email field -- that case
/// belongs to the full window and is why [`ask`] checks the status first.
pub fn gate(password: &str) -> Submit {
    match crate::login_ui::missing_credential_message(
        crate::login_ui::BwStatus::Locked,
        "",
        password,
    ) {
        Some(message) => Submit::Refuse(message),
        None => Submit::Attempt,
    }
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. [`REAL`] is the production set.
///
/// Every one of these is a *call*, never a decision. Nothing here chooses
/// whether to protect the window, whether a blank password is worth a spawn,
/// or what happens after a refusal; that is all [`run_with`]'s, which is why
/// it is the part with tests.
pub struct PromptCalls {
    /// Registers the class, creates the window and its controls, applies the
    /// app's fonts and shows it. `None` if any of that failed.
    pub open: fn() -> Option<PromptWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`. `false` is a
    /// refusal, which is logged and does not stop the prompt -- a user who
    /// cannot unlock at all is worse off than one whose prompt is capturable,
    /// and the refusal is recorded either way.
    pub protect: fn(PromptWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PromptWindow) -> Event,
    /// Copies the field out and disturbs the control's own copy. See the
    /// module doc for what that does and does not achieve.
    pub take_password: fn(PromptWindow) -> Zeroizing<String>,
    /// Puts a line under the field, or clears it with `None`.
    pub show_error: fn(PromptWindow, Option<&str>),
    /// Greys the controls and runs the progress bar while the CLI is out.
    pub busy: fn(PromptWindow, bool),
    /// `bw unlock --raw`. `Err` is the CLI's raw stderr.
    pub unlock: fn(&str) -> Result<String, String>,
    /// Destroys the window and releases the class's resources.
    pub close: fn(PromptWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// The ordering here is the security-relevant content, not the button
/// handling:
///
/// 1. `protect` is called **immediately after `open` and before `next`**, so
///    there is no window between "on screen" and "excluded from capture" that
///    a recorder could catch a keystroke in.
/// 2. The password is read into a `Zeroizing` that lives for exactly one
///    iteration of the loop, so a wrong attempt does not leave the previous
///    guess alive while the user types the next one.
/// 3. `close` runs on every exit path, including the failed ones, because the
///    control's un-wipeable copy (module doc) is bounded by the window's
///    lifetime and by nothing else.
/// 4. A refusal returns to the loop with the window still up. A wrong password
///    must not close the prompt -- that is the whole difference between an
///    error line and starting over.
pub fn run_with(calls: &PromptCalls) -> Outcome {
    let Some(window) = (calls.open)() else {
        log::warn!("the unlock prompt could not be put on screen");
        return Outcome::Unavailable;
    };

    // Before the first pump, so the field cannot be typed into while the
    // window is still capturable.
    if !(calls.protect)(window) {
        // Logged and continued. See `PromptCalls::protect`.
        log::warn!(
            "SetWindowDisplayAffinity was refused for the unlock prompt; the master password \
             field is visible to screen capture on this machine"
        );
    }

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Submit => {
                let secret = (calls.take_password)(window);
                match gate(&secret) {
                    Submit::Refuse(message) => (calls.show_error)(window, Some(message)),
                    Submit::Attempt => {
                        (calls.show_error)(window, None);
                        (calls.busy)(window, true);
                        let attempt = (calls.unlock)(&secret);
                        (calls.busy)(window, false);
                        match attempt {
                            Ok(token) => {
                                (calls.close)(window);
                                return Outcome::Unlocked(token);
                            }
                            Err(stderr) => {
                                // The raw text goes to the log, the readable
                                // line to the window -- `login_ui`'s split,
                                // and its function, so the two surfaces cannot
                                // start describing the same failure
                                // differently.
                                log::warn!("unlock from the daemon prompt failed: {stderr}");
                                let line = crate::login_ui::friendly_auth_error(&stderr);
                                (calls.show_error)(window, Some(&line));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The production entry point: ask for the master password, and answer with a
/// session token or nothing.
///
/// `data_dir` is the profile directory the `bw unlock` acts on -- **the
/// account's, never `bw_path::active_data_dir()` read in here**, which is
/// `login_ui::profile_dir_for`'s rule and the one this crate has been bitten
/// by. It is threaded through a static rather than a parameter because
/// [`PromptCalls::unlock`] is a bare `fn` pointer with no room for it; see
/// [`PROFILE_DIR`].
pub fn ask(data_dir: Option<std::path::PathBuf>) -> Outcome {
    set_profile_dir(data_dir);
    let outcome = run_with(&REAL);
    set_profile_dir(None);
    forget_locked_window();
    outcome
}

/// **The locked card, answered with this prompt.** Design 1b's Locked panel
/// is the card a user gets over a locked vault, and the card is where the
/// password goes: there is no second window between pressing Ctrl+Alt+B and
/// typing. So the presenter's `show_locked` lands here, records which app the
/// prompt is for and where its field is, and answers `Unlock` at once -- which
/// `run`'s loop turns into [`ask`], inside the one resettle sequence it has.
/// [`ask`] then opens beside that field, naming that app.
pub fn for_locked_window(
    app_name: &str,
    anchor: Option<(f32, f32)>,
) -> crate::locked_card::LockedAnswer {
    if let Ok(mut slot) = LOCKED_WINDOW.lock() {
        *slot = Some((app_name.to_string(), anchor, std::time::Instant::now()));
    }
    crate::locked_card::LockedAnswer::Unlock
}

/// What [`for_locked_window`] recorded, for the [`ask`] that follows it.
///
/// **Only a fresh one.** The tray's own Unlock also opens this prompt, and a
/// record that outlived the ask it was for (a resettle that never reached its
/// closure) must not put another app's name on that one.
static LOCKED_WINDOW: std::sync::Mutex<Option<LockedWindowRecord>> = std::sync::Mutex::new(None);

/// The app's name, where its field is, and when that was recorded.
type LockedWindowRecord = (String, Option<(f32, f32)>, std::time::Instant);

fn locked_window() -> Option<(String, Option<(f32, f32)>)> {
    let slot = LOCKED_WINDOW.lock().ok()?;
    let (name, anchor, at) = slot.as_ref()?;
    (at.elapsed() < std::time::Duration::from_secs(30)).then(|| (name.clone(), *anchor))
}

fn forget_locked_window() {
    if let Ok(mut slot) = LOCKED_WINDOW.lock() {
        *slot = None;
    }
}

/// The card's two lines: the state, and what unlocking is for. With the app
/// the card was raised over, the second names it; without one (the tray's
/// Unlock) it says what an unlock gives back.
pub fn prompt_text(app_name: Option<&str>) -> (String, String) {
    match app_name {
        Some(name) => crate::locked_card::locked_text(name),
        None => (
            crate::i18n::t("locked_card.title").to_string(),
            crate::i18n::t("unlock_prompt.subtitle").to_string(),
        ),
    }
}

/// The profile directory the next [`ask`] runs `bw` against.
///
/// A static because the seam is a `fn` pointer, and a `fn` pointer captures
/// nothing. Set and cleared by [`ask`] around one call, and this prompt is
/// modal on the daemon's own thread -- it pumps until the user answers -- so
/// there is never a second one running to race with it.
static PROFILE_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

fn set_profile_dir(dir: Option<std::path::PathBuf>) {
    if let Ok(mut slot) = PROFILE_DIR.lock() {
        *slot = dir;
    }
}

/// The production [`PromptCalls`].
pub static REAL: PromptCalls = PromptCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    take_password: win32::take_password,
    show_error: win32::show_error,
    busy: win32::busy,
    unlock: win32::unlock,
    close: win32::close,
};

/// **The master password, turned into an unlocked vault by whichever backend
/// this account is on.**
///
/// **This is the reuse the brief is about.** There is one place in this app
/// that turns a master password into a session, and it is
/// [`crate::login_ui::authenticate_then_wipe`] -- the same function
/// `login_ui`'s sign-in worker calls, with the same two arms behind it. A
/// copy here -- even a faithful one -- would be a second route to the vault
/// key that could drift on the argument vector, on `--passwordenv`, on which
/// profile directory it lands in, or on which backend it reaches at all.
///
/// # The `bw serve` arm is byte-for-byte what it was
///
/// This named [`crate::login_ui::run_bw_with_password`] directly, with no
/// consultation of [`crate::backend_policy`] anywhere on the path. That call
/// is still the whole of the CLI arm: `authenticate_then_wipe` with a `None`
/// login runs `run_cli(&password)` and enrols on success, and the enrolment
/// passed here does nothing -- so on every `bw serve` account this function
/// runs exactly `bw unlock --raw`, against exactly the directory
/// [`PROFILE_DIR`] holds, and nothing else. That is what
/// `the_unlock_asks_which_backend` is about.
///
/// **No Hello re-enrolment**, which is why `enroll` is a no-op rather than
/// `hello::enroll_for`. A sign-in is where quick unlock is set up; an unlock
/// is where it is spent. Sealing the password again here would be this
/// surface acquiring a side effect the call it replaced never had.
///
/// # The direct-REST arm is the defect this closes
///
/// On a direct-REST account there is no `bw` session behind this profile and
/// there may be no `bw.exe` at all -- `fn main` stopped treating its absence
/// as fatal for exactly these accounts. `bw unlock` there could only ever
/// fail, and [`run_with`] does not give up on a failed attempt: it shows the
/// error and asks again. So the locked card's *Unlock* button was a button
/// that could not unlock, on the one account whose vault the daemon reads
/// with no subprocess at all.
///
/// [`crate::backend_policy::direct_rest_login`] is gated on the choice and
/// not merely on the field (see its own doc), so `Some` here means this
/// process really is serving the active account over REST -- and the active
/// account is the one this prompt is for, since `unlock_from_the_locked_card`
/// derives [`PROFILE_DIR`] from it. Its `adopt` sink writes the derived key
/// to that account's [`crate::user_key_store`] and puts a
/// [`crate::rest::backend::RestBackend`] in the vault slot, which is exactly
/// what the resettle behind this prompt goes on to repopulate from:
/// `restart_backend_after_unlock` answers `NoSubprocess` on this account and
/// falls through to the readiness probe for that reason.
///
/// **A two-step account still cannot complete here**, and it now says so
/// instead of blaming a CLI. The daemon installs `DirectRestLogin::prompt`
/// as `None` -- it has no code stage to draw -- so a second factor ends the
/// attempt with the message `authenticate_then_wipe` returns, which
/// [`run_with`] puts in the error row. That is the state such an account was
/// already in on this surface; what changes is that the reason reported is
/// the true one. The route that does complete one is the tray's Open Vault,
/// whose window has the stage.
///
/// Called on a worker thread by [`win32::unlock`], which is what keeps the
/// window painting while the derivation -- or the CLI -- is out. The `String`
/// it is handed is wiped by that caller; the copy made here is wiped by
/// `authenticate_then_wipe`, which takes it by value into a `Zeroizing` on
/// its first line and drops that on every path out, unwinds included.
fn run_bw_unlock(password: &str) -> Result<String, String> {
    let dir = PROFILE_DIR.lock().ok().and_then(|slot| slot.clone());
    crate::login_ui::authenticate_then_wipe(
        password.to_string(),
        |password| {
            crate::login_ui::run_bw_with_password(&["unlock", "--raw"], password, dir.as_deref())
        },
        // See "No Hello re-enrolment" above: an unlock does not re-seal the
        // master password for quick unlock. A closure that does nothing is
        // what keeps the CLI arm identical to the call this replaced.
        |_| {},
        crate::backend_policy::direct_rest_login(),
    )
}

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%. Every number here is either read off `theme` or
// taken from `login_ui`'s own constants for the surface this one mirrors --
// design 3h's unlock card, which `examples/ui_preview` renders as
// `login_unlock`. Numbers invented here would be a second layout that has to
// agree with a first, which is this codebase's standing defect shape.
// ---------------------------------------------------------------------------

/// The card's width: design 1b's Locked panel, 410 -- the same card as every
/// other panel the daemon puts beside a field.
pub const WIDTH: i32 = crate::panel_draw::CARD_W;

/// D 1.6: `pad 18`, `gap 14`, and the title block's `gap 4`.
const PAD: i32 = crate::panel_draw::PAD;
const GAP: i32 = crate::panel_draw::BLOCK_GAP;
const TITLE_GAP: i32 = 4;
const TITLE_H: i32 = 26;
const BODY_H: i32 = 22;

/// The password box: the panels' own control height and radius (46, r13).
const FIELD_H: i32 = crate::panel_draw::BUTTON_H;
const FIELD_RADIUS: i32 = crate::panel_draw::BUTTON_RADIUS;
/// The Locked panel's tall button (D 1.6: `h54, r15`).
const BUTTON_H: i32 = crate::panel_draw::TALL_BUTTON_H;
/// The room the `Locked` pill may take; it is drawn as wide as its word.
const PILL_ROOM: i32 = 120;

/// One rectangle of the prompt, in logical pixels from the window's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Box2 {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Box2 {
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
}

/// Every rectangle the prompt paints, computed once.
///
/// Pure arithmetic with no Win32 in it, so the height discipline can be
/// asserted without a window. A control whose bottom edge fell past the
/// window's would simply be invisible: this window does not scroll and cannot
/// be resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    pub mark: Box2,
    pub wordmark: Box2,
    pub pill: Box2,
    pub close_glyph: Box2,
    pub title: Box2,
    /// The body line -- and, after a refused attempt, the error in its place,
    /// so the card never grows or reserves an empty row for one.
    pub subtitle: Box2,
    pub field: Box2,
    pub progress: Box2,
    pub unlock: Box2,
}

/// Where the password `EDIT` sits inside the field box the parent paints:
/// 14 in from each side, and 20 high, centred, so it stops as far above the
/// box's bottom border as it starts below its top and never paints over the
/// focus border.
pub fn password_child(field: Box2) -> Box2 {
    const TEXT_H: i32 = 20;
    Box2 { x: field.x + 14, y: field.y + (field.h - TEXT_H) / 2, w: field.w - 28, h: TEXT_H }
}

/// The prompt's geometry: design 1b's Locked panel, with the password box
/// between its two lines and its button.
pub fn layout() -> Layout {
    let content_w = WIDTH - 2 * PAD;

    let lockup = crate::panel_draw::lockup();
    let mark = Box2 { x: PAD, y: PAD, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark = Box2 { x: mark.right() + lockup.gap, y: PAD, w: lockup.word_w, h: lockup.mark_h };
    let close = crate::panel_draw::CLOSE;
    let close_glyph = Box2 { x: WIDTH - PAD - close, y: mark.y + (mark.h - close) / 2, w: close, h: close };
    let pill = Box2 {
        x: close_glyph.x - crate::panel_draw::BUTTON_GAP - PILL_ROOM,
        y: mark.y + (mark.h - crate::panel_draw::PILL_H) / 2,
        w: PILL_ROOM,
        h: crate::panel_draw::PILL_H,
    };

    let title = Box2 { x: PAD, y: mark.bottom() + GAP, w: content_w, h: TITLE_H };
    let subtitle = Box2 { x: PAD, y: title.bottom() + TITLE_GAP, w: content_w, h: BODY_H };
    let field = Box2 { x: PAD, y: subtitle.bottom() + GAP, w: content_w, h: FIELD_H };
    // The 3px track in the gap under the box, inset past its corners.
    let progress = Box2 { x: PAD + FIELD_RADIUS, y: field.bottom() + (GAP - 3) / 2, w: content_w - 2 * FIELD_RADIUS, h: 3 };
    let unlock = Box2 { x: PAD, y: field.bottom() + GAP, w: content_w, h: BUTTON_H };
    let window = Box2 { x: 0, y: 0, w: WIDTH, h: unlock.bottom() + PAD };

    Layout { window, mark, wordmark, pill, close_glyph, title, subtitle, field, progress, unlock }
}

// ---------------------------------------------------------------------------
// The Win32 half. No decisions live below this line.
// ---------------------------------------------------------------------------

/// The event the window procedure last recorded, as a discriminant.
///
/// A static because a window procedure is an `extern "system"` function with
/// nowhere to keep state, which is the same reason `foreground::Win32Desktop`
/// keeps its enumeration callback's accumulator in an `LPARAM`.
static PENDING: AtomicIsize = AtomicIsize::new(0);
/// Set by `WM_DESTROY`, so a window that goes away underneath the pump is
/// reported as [`Event::Closed`] rather than pumped forever.
static GONE: AtomicBool = AtomicBool::new(false);
/// Whether the controls are greyed and the bar is running.
static BUSY: AtomicBool = AtomicBool::new(false);

const PENDING_NONE: isize = 0;
const PENDING_SUBMIT: isize = 1;
const PENDING_CANCEL: isize = 2;

/// The line under the field, or empty.
static ERROR_LINE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn error_line() -> String {
    ERROR_LINE.lock().map(|line| line.clone()).unwrap_or_default()
}

/// The Win32 calls, and **nothing else**.
///
/// # Why every pixel here is painted by hand
///
/// The last raw-Win32 surface in this project was deleted for being ugly, not
/// for being broken. `scratch_window`'s module doc records the epitaph: it
/// "had none of the app's theme, tokens or type, and it was the only surface
/// in the product drawn that way". A themed `BUTTON` renders in the shell's
/// grey with the shell's font and cannot be told to take the app's blue; a
/// default control inherits the ancient bitmap `SYSTEM_FONT`. Both are how a
/// surface ends up looking foreign.
///
/// So the controls here are real `EDIT` and `BUTTON` windows -- which is what
/// buys focus, the caret, IME, and `IsDialogMessage` traversal -- with their
/// **painting taken over completely**: the buttons are subclassed and drawn
/// from `theme`'s palette, and the field's box is drawn by the parent with the
/// borderless `EDIT` sitting inside it. The type is the app's own Archivo,
/// registered privately with `AddFontMemResourceEx` from
/// [`crate::theme::ARCHIVO_FACES`] -- the same bytes egui gets, not a second
/// copy -- by [`crate::win32_draw::register_fonts`], which registers the four
/// Noto Cyrillic cuts beside them for the same reason egui's stacks carry
/// them.
///
/// **The `EDIT` control paints its own text, and that is the one surface here
/// the Cyrillic pairing does not reach.** `win32_draw::draw_text` swaps the
/// face for a run it is handed; the field's contents never pass through it,
/// because `EDIT` rasterises them itself from the `HFONT` it was given at
/// `WM_SETFONT`. A Cyrillic password typed into it therefore still falls back
/// the way it always has. That is left alone deliberately: the alternative is
/// re-deciding the control's font as the user types, which makes the field
/// change typeface mid-word.
///
/// Nothing in this module decides anything. See [`run_with`].
mod win32 {
    use super::{
        Box2, Event, PromptWindow, BUSY, GONE, PENDING, PENDING_CANCEL, PENDING_NONE,
        PENDING_SUBMIT, UNLOCK_PROMPT_TITLE,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC,
        SelectObject, SetBkColor, SetBkMode, SetTextCharacterExtra, SetTextColor,
        CLEARTYPE_QUALITY, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID,
        SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW,
        PeekMessageW, RegisterClassW, SendMessageW, SetForegroundWindow,
        SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
        TranslateMessage, BN_CLICKED, BS_DEFPUSHBUTTON, CS_HREDRAW, CS_VREDRAW,
        ES_AUTOHSCROLL, ES_PASSWORD, GWLP_WNDPROC, HMENU, IDC_ARROW, MSG, PM_REMOVE,
        SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND,
        WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_GETTEXT, WM_GETTEXTLENGTH, WM_LBUTTONDOWN,
        WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_SETFONT, WNDCLASSW, WS_CHILD,
        WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    const ID_PASSWORD: usize = 101;
    const ID_UNLOCK: usize = 102;
    
    const CLASS_NAME: PCWSTR = w!("DeskwardenUnlockPrompt");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **This is the SYSTEM DPI, not the monitor's**, and that is a known
    /// limitation rather than an oversight. `GetDpiForWindow` -- the
    /// per-monitor answer -- lives in the `windows` crate's `Win32_UI_HiDpi`
    /// feature, which this crate does not enable, and enabling it means
    /// re-pinning `job_object.rs`'s whole-file hash of `Cargo.toml`. On a
    /// single-scale desktop, and on the primary monitor of a mixed one, this
    /// is exact; on a second monitor at a different scale the prompt is drawn
    /// at the primary's scale and will read slightly large or small. See the
    /// report.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// `theme`'s `Color32` as GDI's BGR `COLORREF`.
    ///
    /// One conversion, used everywhere, so that no hex value in this file is
    /// a palette entry written out a second time. Lives in
    /// [`crate::win32_draw`] now, alongside [`crate::win32_draw::draw_button`]
    /// which every button in this window paints through.
    use crate::win32_draw::rgb;

    // ---- fonts -------------------------------------------------------------

    /// Registers every bundled face privately with GDI, once.
    ///
    /// **The loop that used to be here now lives in
    /// [`crate::win32_draw::register_fonts`], behind one process-wide
    /// `OnceLock` instead of one per card.** `AddFontMemResourceEx` copies the
    /// font data into the process font table rather than refcounting a shared
    /// buffer, so every card that carried its own copy of this loop handed GDI
    /// another private copy of all four Archivo cuts -- roughly 750 KB of pure
    /// duplicate per card opened in a session. Adding the four Noto Cyrillic
    /// cuts the cards now need for Cyrillic runs would have made that worse
    /// rather than better, so the registration moved to the one module every
    /// card already draws its text through.
    fn register_fonts() {
        crate::win32_draw::register_fonts();
    }

    /// An `HFONT` for one of the app's faces at one logical size.
    ///
    /// `family` is an egui family name from `theme`; the GDI family and weight
    /// come from [`crate::theme::gdi_face_for`], which reads them out of the
    /// files' own `name` records rather than guessing.
    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                // ClearType, explicitly. The default quality on a memory DC
                // is not it, and grayscale-antialiased Archivo beside the
                // app's ClearType egui text is exactly the "almost right"
                // that reads as a different program.
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the prompt paints with, created at open and destroyed at
    /// close. Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        /// "Deskwarden" beside the mark, 15/800.
        wordmark: HFONT,
        /// The `Locked` pill's word, 12/700.
        pill: HFONT,
        /// The title, 19/800, and the line under it, 15.
        title: HFONT,
        subtitle: HFONT,
        /// What is typed, and the placeholder, 16.
        field: HFONT,
        /// The *Unlock* button, 16/700.
        button: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, EXTRABOLD, REGULAR};
            Fonts {
                wordmark: font(EXTRABOLD, crate::panel_draw::WORD_PX),
                pill: font(BOLD, crate::panel_draw::PILL_PX),
                title: font(EXTRABOLD, crate::panel_draw::TITLE_PX),
                subtitle: font(REGULAR, crate::panel_draw::BODY_PX),
                field: font(REGULAR, 16),
                button: font(BOLD, 16),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.wordmark, self.pill, self.title, self.subtitle, self.field, self.button] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide and not tied to a
    // thread. The prompt is modal on one thread, so nothing shares them; the
    // `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl Send for Fonts {}

    /// The subclassed buttons' original procedures, so painting can be taken
    /// over without losing the focus and keyboard behaviour that makes
    /// `IsDialogMessage` work.
    static UNLOCK_PROC: AtomicIsize = AtomicIsize::new(0);
    /// The password `EDIT`'s original procedure: it is subclassed only to
    /// paint the placeholder while it is empty.
    static FIELD_PROC: AtomicIsize = AtomicIsize::new(0);
    /// The app the card was raised over, for its second line. Empty for the
    /// tray's Unlock.
    static APP_NAME: Mutex<String> = Mutex::new(String::new());
    /// Which button the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    // ---- the window --------------------------------------------------------

    pub(super) fn open() -> Option<PromptWindow> {
        register_fonts();
        PENDING.store(PENDING_NONE, Ordering::SeqCst);
        GONE.store(false, Ordering::SeqCst);
        BUSY.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut line) = super::ERROR_LINE.lock() {
            line.clear();
        }

        unsafe {
            DPI_PERCENT.store(
                {
                    let dc = GetDC(None);
                    let dpi = GetDeviceCaps(dc, LOGPIXELSX);
                    ReleaseDC(None, dc);
                    if dpi > 0 { dpi * 100 / 96 } else { 100 }
                },
                Ordering::SeqCst,
            );
        }

        register_class();
        *FONTS.lock().ok()? = Some(Fonts::build());

        let l = super::layout();
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        // Raised over a locked app, it opens where the locked card would have:
        // beside the field the user was in. From the tray it has no field to
        // stand beside, so it is centred on the work area.
        let context = super::locked_window();
        if let Ok(mut slot) = APP_NAME.lock() {
            *slot = context.as_ref().map(|(name, _)| name.clone()).unwrap_or_default();
        }
        let (x, y) = match context.and_then(|(_, anchor)| anchor) {
            Some(anchor) => crate::prompt_card::place(Some(anchor), work_area(), w, h),
            None => centred(w, h),
        };

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the
                // user was doing. NOT `WS_EX_NOACTIVATE`: this window takes
                // focus deliberately -- see the module doc.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(UNLOCK_PROMPT_TITLE),
                // Frameless. A `WS_CAPTION` frame is the single loudest
                // "system dialog" signal there is, and the app's own windows
                // are frameless with a drawn titlebar.
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                w,
                h,
                None,
                None,
                None,
                None,
            )
        }
        .ok()?;

        round_corners(window);

        let fonts = FONTS.lock().ok()?;
        let fonts = fonts.as_ref()?;

        // The password field: borderless, because the box around it is
        // painted by the parent in the app's colours. `ES_PASSWORD` masks the
        // display only -- see the module doc on what that does not do.
        let field = child(
            window,
            w!("EDIT"),
            "",
            WS_TABSTOP.0 | ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
            super::password_child(l.field),
            ID_PASSWORD,
            fonts.field,
        )?;

        let unlock = child(
            window,
            w!("BUTTON"),
            "&Unlock",
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            l.unlock,
            ID_UNLOCK,
            fonts.button,
        )?;
        subclass(unlock, &UNLOCK_PROC);
        unsafe {
            let previous = SetWindowLongPtrW(field, GWLP_WNDPROC, field_proc as *const () as isize);
            FIELD_PROC.store(previous, Ordering::SeqCst);
        }

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the same
            // property `foreground` records. A refusal leaves a topmost window
            // on screen that the user clicks once to focus.
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(field);
        }

        TOP_LEVEL.store(handle_of(window), Ordering::SeqCst);
        Some(PromptWindow { top_level: handle_of(window), field: handle_of(field) })
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied here and never to `window.field`: Windows refuses
    /// `SetWindowDisplayAffinity` on a child with `E_INVALIDARG`, and the
    /// top-level flag covers every child it owns.
    pub(super) fn protect(window: PromptWindow) -> bool {
        if !crate::screen_share::capture_protection_enabled() {
            return true;
        }
        unsafe { SetWindowDisplayAffinity(hwnd(window.top_level), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **`IsDialogMessageW` is what makes Tab, Shift+Tab, Enter and the
    /// `&`-mnemonics work at all.** A bare `TranslateMessage`/`DispatchMessage`
    /// pump around controls that are not in a dialog gives none of them: Tab
    /// types a tab character into the field and Enter does nothing. Escape is
    /// handled before it, because `IsDialogMessage` only cancels for a real
    /// dialog box.
    pub(super) fn next(window: PromptWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
        use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

        let top = hwnd(window.top_level);
        loop {
            if GONE.load(Ordering::SeqCst) {
                return Event::Closed;
            }
            match PENDING.swap(PENDING_NONE, Ordering::SeqCst) {
                PENDING_SUBMIT => return Event::Submit,
                PENDING_CANCEL => return Event::Cancel,
                _ => {}
            }

            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        return Event::Closed;
                    }
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    if !IsDialogMessageW(top, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    match PENDING.swap(PENDING_NONE, Ordering::SeqCst) {
                        PENDING_SUBMIT => return Event::Submit,
                        PENDING_CANCEL => return Event::Cancel,
                        _ => {}
                    }
                }
            }
            // Idle. A sleep rather than `GetMessageW` because the busy state
            // animates its bar off the same loop.
            std::thread::sleep(std::time::Duration::from_millis(8));
            if BUSY.load(Ordering::SeqCst) {
                repaint(top);
            }
        }
    }

    /// Copies the field out into buffers this process owns and wipes every
    /// copy it can reach.
    ///
    /// See the module doc for the copy it **cannot** reach -- comctl32's own
    /// buffer, which `SetWindowTextW` is asked to overwrite here and is free
    /// to reallocate around instead.
    pub(super) fn take_password(window: PromptWindow) -> zeroize::Zeroizing<String> {
        unsafe {
            let field = hwnd(window.field);
            let len = SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0;
            if len <= 0 {
                return zeroize::Zeroizing::new(String::new());
            }
            let mut buf: zeroize::Zeroizing<Vec<u16>> =
                zeroize::Zeroizing::new(vec![0u16; len as usize + 1]);
            let copied = SendMessageW(
                field,
                WM_GETTEXT,
                WPARAM(buf.len()),
                LPARAM(buf.as_mut_ptr() as isize),
            )
            .0
            .max(0) as usize;
            zeroize::Zeroizing::new(String::from_utf16_lossy(&buf[..copied.min(len as usize)]))
        }
    }

    /// Overwrites the control's own buffer with an equal-length run of filler.
    ///
    /// **Best effort in the strict sense.** `SetWindowTextW` may overwrite in
    /// place or may release the old allocation and take a new one; the API
    /// does not say, and nothing here can find out. Equal length is what makes
    /// the in-place case likely rather than certain. Called from [`close`], so
    /// it runs on every exit path including the successful one.
    fn scrub_field(field: HWND) {
        unsafe {
            let len = SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0.max(0) as usize;
            if len > 0 {
                let filler = HSTRING::from("\u{2022}".repeat(len));
                let _ = SetWindowTextW(field, &filler);
            }
            let _ = SetWindowTextW(field, w!(""));
        }
    }

    pub(super) fn show_error(window: PromptWindow, line: Option<&str>) {
        if let Ok(mut slot) = super::ERROR_LINE.lock() {
            slot.clear();
            if let Some(text) = line {
                slot.push_str(text);
            }
        }
        if line.is_some() {
            // Put the caret back in the field with everything selected, so the
            // next character the user types replaces the guess that just
            // failed. Without this the field keeps the rejected password with
            // the caret at its end, and correcting a mistyped master password
            // means selecting it by hand first -- on a control whose contents
            // are masked, which is the worst place in the app to ask someone
            // to aim.
            //
            // NOT a clear: the user may have mistyped one character of a long
            // password, and a field that empties itself has thrown away the
            // other thirty.
            unsafe {
                // . The  crate does not project the EDIT
                // control messages under the features this crate enables, so
                // it is the documented constant, named here rather than left
                // as a bare hex literal at the call.
                const EM_SETSEL: u32 = 0x00B1;
                let field = hwnd(window.field);
                let _ = SetFocus(field);
                SendMessageW(field, EM_SETSEL, WPARAM(0), LPARAM(-1));
            }
        }
        repaint(hwnd(window.top_level));
    }

    pub(super) fn busy(window: PromptWindow, on: bool) {
        BUSY.store(on, Ordering::SeqCst);
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
            // The credential zone goes inert while the password is with the
            // CLI -- design 3h's own behaviour, and for its reason: a field
            // that still takes typing during an attempt is a field whose
            // contents cannot affect the answer coming back.
            for id in [ID_PASSWORD, ID_UNLOCK] {
                if let Ok(control) = GetDlgItem(hwnd(window.top_level), id as i32) {
                    let _ = EnableWindow(control, !on);
                }
            }
        }
        repaint(hwnd(window.top_level));
    }

    /// `bw unlock --raw`, on a worker, while this thread keeps the window
    /// alive.
    ///
    /// **The thread is not an optimisation.** A CLI spawn plus a network round
    /// trip is seconds; run inline it would stop this window's message pump for
    /// all of it, and a top-level window that stops pumping does not merely
    /// freeze -- it stops repainting, so dragging anything over it smears, and
    /// Windows eventually offers to kill it. `login_ui` moved the same call to
    /// a worker for the same reason and records it (`spawn_auth`); this is that
    /// decision again on a surface that has no frame loop to fall back on.
    ///
    /// The password is moved in and zeroized on the worker once the CLI is done
    /// with it, exactly as `spawn_auth` does, so the worker leaves no live copy
    /// behind either.
    ///
    /// This is the ONE `std::thread::spawn` in this file, and
    /// `job_object`'s `THREAD_SPAWN_SITES` census grants it exactly one.
    pub(super) fn unlock(password: &str) -> Result<String, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut owned = password.to_string();
        std::thread::spawn(move || {
            let result = super::run_bw_unlock(&owned);
            zeroize::Zeroize::zeroize(&mut owned);
            let _ = tx.send(result);
        });

        // Pump while it is out, so the bar animates and the window stays a
        // window. `Event`s are deliberately NOT acted on here: the controls
        // are disabled, and a second submit arriving mid-attempt is exactly
        // what `busy` exists to prevent.
        loop {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                // The worker died without answering. Reported as a failure
                // rather than waited on forever.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("the unlock worker stopped without answering".to_string());
                }
            }
            drain_and_repaint();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    /// One turn of the pump during a busy wait: dispatch whatever is queued and
    /// advance the progress bar.
    ///
    /// `WM_QUIT` and `WM_DESTROY` are honoured -- a window closed while the CLI
    /// is out sets `GONE`, and [`next`] reports `Closed` on the next call, so
    /// the attempt's answer is discarded rather than acted on against a window
    /// that no longer exists.
    fn drain_and_repaint() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    GONE.store(true, Ordering::SeqCst);
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Only the bar's own rectangle, so a repaint every 16ms does not
        // re-run the whole surface's text layout sixty times a second.
        unsafe {
            let top = TOP_LEVEL.load(Ordering::SeqCst);
            if top != 0 {
                let l = super::layout();
                let bar = RECT {
                    left: scale(l.progress.x),
                    top: scale(l.progress.y) - 2,
                    right: scale(l.progress.right()),
                    bottom: scale(l.progress.bottom()) + 2,
                };
                let _ = InvalidateRect(hwnd(top), Some(&bar), false);
            }
        }
    }

    /// The window [`unlock`] repaints while it waits.
    ///
    /// A static for the reason everything else here is: `PromptCalls::unlock`
    /// is a bare `fn(&str)` -- the seam deliberately does not hand it a window,
    /// because the DECISION layer has no business knowing there is one.
    static TOP_LEVEL: AtomicIsize = AtomicIsize::new(0);

    pub(super) fn close(window: PromptWindow) {
        TOP_LEVEL.store(0, Ordering::SeqCst);
        unsafe {
            scrub_field(hwnd(window.field));
            let _ = DestroyWindow(hwnd(window.top_level));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        if let Ok(mut line) = super::ERROR_LINE.lock() {
            line.clear();
        }
        if let Ok(mut name) = APP_NAME.lock() {
            name.clear();
        }
    }

    // ---- plumbing ----------------------------------------------------------

    fn handle_of(h: HWND) -> isize {
        h.0 as isize
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn repaint(window: HWND) {
        unsafe {
            let _ = InvalidateRect(window, None, false);
        }
    }

    fn inset(b: Box2, left: i32, top: i32, right: i32, bottom: i32) -> Box2 {
        Box2 {
            x: b.x + left,
            y: b.y + top,
            w: b.w - left - right,
            h: b.h - top - bottom,
        }
    }

    fn work_area() -> (i32, i32, i32, i32) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            };
            let mut area = RECT::default();
            let ok = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            if ok.is_err() || area.right <= area.left || area.bottom <= area.top {
                return (0, 0, 1920, 1080);
            }
            (area.left, area.top, area.right, area.bottom)
        }
    }

    fn centred(w: i32, h: i32) -> (i32, i32) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            };
            let mut area = RECT::default();
            let ok = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            if ok.is_err() || area.right <= area.left {
                return (200, 200);
            }
            (
                area.left + (area.right - area.left - w) / 2,
                // Slightly above centre: a box the eye has to find sits better
                // a little high, and this is where every OS credential prompt
                // puts itself.
                area.top + (area.bottom - area.top - h) * 2 / 5,
            )
        }
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                // No background brush: `WM_ERASEBKGND` is answered and the
                // whole client area is painted from one back buffer, which is
                // what keeps the surface from flashing the system grey on
                // every repaint.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    fn child(
        parent: HWND,
        class: PCWSTR,
        text: &str,
        style: u32,
        at: Box2,
        id: usize,
        font: HFONT,
    ) -> Option<HWND> {
        let text = HSTRING::from(text);
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &text,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                scale(at.x),
                scale(at.y),
                scale(at.w),
                scale(at.h),
                parent,
                HMENU(id as *mut c_void),
                None,
                None,
            )
        }
        .ok()?;
        unsafe {
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        }
        Some(h)
    }

    /// Hands the card's corners to [`crate::card_shell`]: the design's 18 px
    /// radius, antialiased, with its soft shadow, in place of the fixed ~8 px
    /// corner `DWMWCP_ROUND` gave every card. The card's own background is
    /// `theme::CARD`, which is what the shell paints under it.
    fn round_corners(window: HWND) {
        crate::card_shell::attach(window, crate::theme::CARD);
    }

    fn subclass(button: HWND, slot: &AtomicIsize) {
        unsafe {
            let previous = SetWindowLongPtrW(button, GWLP_WNDPROC, button_proc as *const () as isize);
            slot.store(previous, Ordering::SeqCst);
        }
    }

    // ---- the window procedures ---------------------------------------------

    unsafe extern "system" fn wnd_proc(
        window: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            // The field sits inside a box the parent painted, so its own
            // background has to be the card's white rather than the system's.
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetBkColor(hdc, rgb(crate::theme::CARD));
                SetTextColor(
                    hdc,
                    rgb(if BUSY.load(Ordering::SeqCst) {
                        crate::theme::TEXT_GHOST
                    } else {
                        crate::theme::INK
                    }),
                );
                LRESULT(card_brush().0 as isize)
            }
            // Frameless windows are dragged by their background.
            WM_NCHITTEST => {
                // **The close glyph is the one part of the background that is
                // not a title bar.** It is painted by this window rather than
                // being a child control, so answering `HTCAPTION` for the whole
                // client area turned every press on it into a window drag and
                // `WM_LBUTTONDOWN` below never fired -- the reported "clicking
                // on X doesn't work". See `win32_draw::frameless_hit`, which is
                // the pure half of this and the half the pin decides.
                crate::win32_draw::frameless_hit_test(
                    window,
                    DefWindowProcW(window, msg, wparam, lparam),
                    lparam,
                    close_glyph_rect(),
                )
            }
            WM_LBUTTONDOWN => {
                if in_close_glyph(window, lparam) {
                    PENDING.store(PENDING_CANCEL, Ordering::SeqCst);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // The close glyph's hover, and the buttons' un-hover: a
                // pointer that left a button without entering another one is
                // seen here rather than by the button it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                use windows::Win32::UI::WindowsAndMessaging::IDOK;
                let id = (wparam.0 & 0xffff) as i32;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                // `IDOK` is what `IsDialogMessage` sends for Enter.
                if (id == ID_UNLOCK as i32 && notification == BN_CLICKED) || id == IDOK.0 {
                    if !BUSY.load(Ordering::SeqCst) {
                        PENDING.store(PENDING_SUBMIT, Ordering::SeqCst);
                    }
                } else if id == ID_PASSWORD as i32 && (notification == 0x0100 || notification == 0x0200) {
                    // `EN_SETFOCUS` / `EN_KILLFOCUS`, which the `windows`
                    // crate does not project under this crate's features.
                    // The box's border follows the caret in and out.
                    repaint(window);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // No thread quit is posted here, deliberately. `close` calls
                // `DestroyWindow`, which dispatches this message SYNCHRONOUSLY on
                // the calling thread, and this window is opened on the thread
                // that goes on to run egui windows (`unlock_from_the_locked_card`
                // returns into `resume_fill_after_unlock`, which opens the
                // autofill overlay and the preflight window, both
                // `eframe::run_native` on this same thread). A quit posted here
                // is never drained -- `next` has already returned and no pump
                // runs before the caller acts -- so the next `run_native` would
                // take it out of `GetMessageW`, leave its loop before drawing,
                // and silently return its default answer. `GONE` above is what
                // `next` actually reads; a quit posted from OUTSIDE is still
                // honoured by `next`'s own `WM_QUIT` branch.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// The subclassed buttons: everything except painting and hover is the
    /// original `BUTTON` procedure's, which is what keeps focus, the space
    /// bar, and `IsDialogMessage`'s traversal working.
    unsafe extern "system" fn button_proc(
        button: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let id = GetWindowLongPtrW(button, windows::Win32::UI::WindowsAndMessaging::GWLP_ID);
        let original = UNLOCK_PROC.load(Ordering::SeqCst);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_button(button);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(id, Ordering::SeqCst) != id {
                    repaint(button);
                }
                LRESULT(0)
            }
            _ => {
                if original == 0 {
                    DefWindowProcW(button, msg, wparam, lparam)
                } else {
                    CallWindowProcW(
                        Some(std::mem::transmute::<
                            isize,
                            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                        >(original)),
                        button,
                        msg,
                        wparam,
                        lparam,
                    )
                }
            }
        }
    }

    fn card_brush() -> HBRUSH {
        static BRUSH: OnceLock<isize> = OnceLock::new();
        HBRUSH(*BRUSH.get_or_init(|| unsafe { CreateSolidBrush(rgb(crate::theme::CARD)).0 as isize })
            as *mut c_void)
    }

    /// The close glyph's rect in DEVICE pixels.
    ///
    /// One derivation, read by both the hit test and `in_close_glyph`, so the
    /// rect `WM_NCHITTEST` excuses from the drag and the rect `WM_LBUTTONDOWN`
    /// answers on can never be two different rectangles.
    fn close_glyph_rect() -> RECT {
        let l = super::layout();
        RECT {
            left: scale(l.close_glyph.x),
            top: scale(l.close_glyph.y),
            right: scale(l.close_glyph.right()),
            bottom: scale(l.close_glyph.bottom()),
        }
    }

    fn in_close_glyph(window: HWND, lparam: LPARAM) -> bool {
        let _ = window;
        crate::win32_draw::on_close_glyph(
            (lparam.0 & 0xffff) as i16 as i32,
            ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            close_glyph_rect(),
        )
    }

    // ---- painting ----------------------------------------------------------

    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            // Double-buffered. A surface repainted directly to the window
            // flickers visibly on every keystroke.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            // The whole window is the card: design 1b's white panel.
            fill(mem, client, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            let l = super::layout();
            let busy = BUSY.load(Ordering::SeqCst);

            if let Some(fonts) = fonts {
                crate::panel_draw::draw_lockup(mem, dev(l.mark), dev(l.wordmark), fonts.wordmark);
                let word = crate::locked_card::locked_pill();
                crate::panel_draw::draw_pill(
                    mem,
                    fitted_pill(mem, fonts.pill, dev(l.pill), word),
                    word,
                    fonts.pill,
                    crate::theme::BLUE_WASH,
                    crate::theme::BLUE_DEEP,
                );

                let name = APP_NAME.lock().map(|n| n.clone()).unwrap_or_default();
                let (title, body) = super::prompt_text((!name.is_empty()).then_some(name.as_str()));
                text(mem, fonts.title, l.title, &title, crate::theme::INK, DT_LEFT | DT_END_ELLIPSIS, 0);
                // A refused attempt says why in the body's place.
                let line = super::error_line();
                if line.is_empty() {
                    text(mem, fonts.subtitle, l.subtitle, &body, crate::theme::TEXT_FAINT, DT_LEFT | DT_END_ELLIPSIS, 0);
                } else {
                    text(mem, fonts.subtitle, l.subtitle, &line, crate::theme::ERROR, DT_LEFT | DT_END_ELLIPSIS, 0);
                }
            }

            // The password box, and its focus halo. The `EDIT` itself is a
            // child sitting inside this.
            let focused = handle_of(GetFocus());
            let field_focused = GetDlgItem(window, ID_PASSWORD as i32)
                .map(|f| handle_of(f) == focused)
                .unwrap_or(false);
            if field_focused {
                rounded(mem, inset(l.field, -3, -3, -3, -3), super::FIELD_RADIUS + 3, crate::theme::FOCUS_RING, None);
            }
            rounded(
                mem,
                l.field,
                super::FIELD_RADIUS,
                crate::theme::CARD,
                Some((1, if field_focused { crate::theme::BLUE } else { crate::theme::BORDER_STRONG })),
            );

            if busy {
                paint_progress(mem, l.progress);
            }

            paint_close_glyph(mem, l.close_glyph);

            drop(guard);
            let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    fn dev(b: Box2) -> RECT {
        RECT { left: scale(b.x), top: scale(b.y), right: scale(b.right()), bottom: scale(b.bottom()) }
    }

    /// The pill's device rect, as wide as its word and the design's 10 px
    /// padding, right-aligned in the layout's room for it.
    fn fitted_pill(hdc: HDC, font: HFONT, room: RECT, word: &str) -> RECT {
        let chars: Vec<u16> = word.encode_utf16().collect();
        let text_w = crate::win32_draw::text_width_utf16(hdc, font, &chars);
        let w = crate::panel_draw::pill_width(text_w, DPI_PERCENT.load(Ordering::SeqCst))
            .min(room.right - room.left);
        RECT { left: room.right - w, ..room }
    }

    /// The *Unlock* button: design 1b's deep-blue tall button.
    fn paint_button(button: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(button, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(button, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == ID_UNLOCK as isize;
            let busy = BUSY.load(Ordering::SeqCst);
            let focused = GetFocus() == button;
            let skin = crate::panel_draw::skin(crate::panel_draw::PanelButton::Deep, hovered && !busy);
            let skin = if busy { skin.disabled() } else { skin };

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            // Drawn over the card it sits on, so the rounded corners show the
            // card's white rather than system grey.
            fill(mem, rc, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };
            let radius = scale(crate::panel_draw::TALL_BUTTON_RADIUS);
            let label = crate::i18n::t("unlock_prompt.unlock");

            let guard = FONTS.lock();
            let font = guard.as_ref().ok().and_then(|s| s.as_ref()).map(|fonts| fonts.button);
            if let Some(font) = font {
                // A ring only when the keyboard has really moved here: the
                // caret starts in the password box, so this is Tab's doing.
                if focused {
                    crate::win32_draw::aa_round_rect(mem, whole, radius + scale(1), rgb(crate::theme::FOCUS_RING), None);
                    let inner = RECT { left: 2, top: 2, right: rc.right - 2, bottom: rc.bottom - 2 };
                    crate::win32_draw::draw_button(mem, inner, label, font, skin, radius);
                } else {
                    crate::win32_draw::draw_button(mem, whole, label, font, skin, radius);
                }
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(button, &ps);
        }
    }

    /// The password `EDIT`, subclassed for one thing: while it is empty it
    /// shows its placeholder, painted over the control's own (empty) paint.
    /// `EM_SETCUEBANNER` would do this, but only under comctl32 v6, which this
    /// executable has no manifest for.
    unsafe extern "system" fn field_proc(field: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let original = FIELD_PROC.load(Ordering::SeqCst);
        let result = if original == 0 {
            DefWindowProcW(field, msg, wparam, lparam)
        } else {
            CallWindowProcW(
                Some(std::mem::transmute::<
                    isize,
                    unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                >(original)),
                field,
                msg,
                wparam,
                lparam,
            )
        };
        if msg == WM_PAINT && SendMessageW(field, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0)).0 == 0 {
            let guard = FONTS.lock();
            if let Some(font) = guard.as_ref().ok().and_then(|s| s.as_ref()).map(|f| f.field) {
                let dc = GetDC(field);
                let mut rc = RECT::default();
                let _ = GetClientRect(field, &mut rc);
                let old = SelectObject(dc, font);
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, rgb(crate::theme::TEXT_GHOST));
                // Past the caret's own column.
                rc.left += scale(2);
                crate::win32_draw::draw_text(
                    dc,
                    crate::i18n::t("unlock_prompt.label"),
                    &mut rc,
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                );
                SelectObject(dc, old);
                ReleaseDC(field, dc);
            }
        }
        result
    }

    /// The 3px indeterminate track, `theme::paint_progress_bar`'s proportions
    /// (32% knob, 1.4s period) driven off the wall clock rather than a frame
    /// time, because this surface has no frame loop.
    fn paint_progress(hdc: HDC, at: Box2) {
        let phase = {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (millis % 1400) as f32 / 1400.0
        };
        rounded(hdc, at, 2, crate::theme::TOGGLE_OFF, None);
        let knob_w = (at.w as f32 * 0.32) as i32;
        // A there-and-back sweep, so the knob never jumps from right to left.
        let travel = at.w - knob_w;
        let t = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
        let knob = Box2 {
            x: at.x + (travel as f32 * t) as i32,
            y: at.y,
            w: knob_w,
            h: at.h,
        };
        rounded(hdc, knob, 2, crate::theme::BLUE, None);
    }

    /// The card header's ✕, drawn as two strokes because no bundled face has
    /// the glyph at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        unsafe {
            let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(crate::theme::TEXT_FAINT));
            let old = SelectObject(hdc, pen);
            let (x, y, w, h) = (scale(at.x), scale(at.y), scale(at.w), scale(at.h));
            let pad = w / 3;
            use windows::Win32::Graphics::Gdi::{LineTo, MoveToEx};
            let _ = MoveToEx(hdc, x + pad, y + pad, None);
            let _ = LineTo(hdc, x + w - pad, y + h - pad);
            let _ = MoveToEx(hdc, x + w - pad, y + pad, None);
            let _ = LineTo(hdc, x + pad, y + h - pad);
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
    }

    fn fill(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// A rounded rectangle in logical coordinates, optionally stroked.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        border: Option<(i32, eframe::egui::Color32)>,
    ) {
        // Antialiased: `win32_draw::aa_round_rect` mixes the edge with what is
        // already under it, where `RoundRect` stair-stepped it.
        crate::win32_draw::aa_round_rect(
            hdc,
            RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            },
            scale(radius),
            rgb(fill_colour),
            border.map(|(width, colour)| (rgb(colour), scale(width).max(1))),
        );
    }

    /// One run of text, vertically centred in `at`.
    ///
    /// `tracking` is GDI's whole-pixel `SetTextCharacterExtra`. The design
    /// specifies fractional em tracking; whole pixels is the closest GDI's
    /// text API offers without laying every glyph out by hand.
    fn text(
        hdc: HDC,
        font: HFONT,
        at: Box2,
        run: &str,
        colour: eframe::egui::Color32,
        align: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
        tracking: i32,
    ) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            SetTextCharacterExtra(hdc, scale(tracking));
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `win32_draw::draw_text`, not `DrawTextW`: an empty run through
            // the raw call is an access violation at address 0x2 that kills
            // the whole daemon without a log line. See the block comment above
            // that wrapper, which is the crate's only `DrawTextW`.
            //
            // `DT_NOPREFIX`: the labels here are the app's own words, and an
            // `&` in one of them is an ampersand, not a mnemonic. The buttons
            // are the only place a mnemonic is meant, and they are drawn by
            // `paint_button` with their own literal.
            crate::win32_draw::draw_text(
                hdc,
                run,
                &mut rc,
                align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
            SetTextCharacterExtra(hdc, 0);
            SelectObject(hdc, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ---- the recorder ------------------------------------------------------
    //
    // `cfg(test)` seams are banned in this crate, so the fakes are ordinary
    // `fn`s -- the shape a `PromptCalls` needs anyway -- recording into a
    // static. Nothing below creates a window, registers a hotkey or calls
    // `bw`; every one of these is a plain function over a `Mutex`.

    #[derive(Debug, Default)]
    struct Tape {
        calls: Vec<String>,
        /// What `next` hands back, in order. Empty means `Closed`.
        script: Vec<Event>,
        /// What `unlock` answers, in order.
        answers: Vec<Result<String, String>>,
        /// What `take_password` hands back, in order.
        passwords: Vec<String>,
        /// `false` makes `open` fail.
        can_open: bool,
        /// What `protect` answers.
        protects: bool,
    }

    static TAPE: Mutex<Option<Tape>> = Mutex::new(None);

    /// **One tape, so one test at a time.**
    ///
    /// `PromptCalls` is a struct of `fn` pointers, which is the seam idiom
    /// this crate uses precisely because a `fn` captures nothing -- so the
    /// fakes have to record into a static, and `cargo test`'s default thread
    /// pool would otherwise interleave two tests through one recorder. That is
    /// not a hypothetical: it showed up as seven failures whose tapes each
    /// held another test's calls.
    ///
    /// Poisoning is recovered from rather than propagated: a test that panics
    /// mid-tape is already failing, and turning that into a cascade of
    /// poison-panics in every later test hides which one broke.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Loads the tape and returns the serialisation guard. Every test binds it
    /// -- `let _serial = start(..)` -- and holding it for the test body is what
    /// makes the recorder single-threaded. Binding it to `_` rather than
    /// `_serial` would drop it immediately and put the interleaving back.
    /// (`MutexGuard` is already `#[must_use]`, so dropping the binding
    /// entirely is a warning rather than a silent regression.)
    fn start(
        script: Vec<Event>,
        passwords: &[&str],
        answers: Vec<Result<String, String>>,
    ) -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *TAPE.lock().unwrap_or_else(|p| p.into_inner()) = Some(Tape {
            script,
            answers,
            passwords: passwords.iter().map(|p| (*p).to_string()).collect(),
            can_open: true,
            protects: true,
            ..Tape::default()
        });
        guard
    }

    fn with<R>(f: impl FnOnce(&mut Tape) -> R) -> R {
        let mut slot = TAPE.lock().unwrap_or_else(|p| p.into_inner());
        f(slot.as_mut().expect("the tape is running"))
    }

    fn calls() -> Vec<String> {
        with(|t| t.calls.clone())
    }

    const FAKE: PromptWindow = PromptWindow { top_level: 0x7000, field: 0x7001 };

    fn fake_open() -> Option<PromptWindow> {
        with(|t| {
            t.calls.push("open".to_string());
            t.can_open.then_some(FAKE)
        })
    }
    fn fake_protect(window: PromptWindow) -> bool {
        with(|t| {
            t.calls.push(format!("protect({:#x})", window.top_level));
            t.protects
        })
    }
    fn fake_next(_: PromptWindow) -> Event {
        with(|t| {
            t.calls.push("next".to_string());
            if t.script.is_empty() {
                Event::Closed
            } else {
                t.script.remove(0)
            }
        })
    }
    fn fake_take_password(_: PromptWindow) -> Zeroizing<String> {
        with(|t| {
            t.calls.push("take_password".to_string());
            Zeroizing::new(if t.passwords.is_empty() {
                String::new()
            } else {
                t.passwords.remove(0)
            })
        })
    }
    fn fake_show_error(_: PromptWindow, line: Option<&str>) {
        with(|t| t.calls.push(format!("show_error({})", line.unwrap_or("<cleared>"))));
    }
    fn fake_busy(_: PromptWindow, on: bool) {
        with(|t| t.calls.push(format!("busy({on})")));
    }
    fn fake_unlock(password: &str) -> Result<String, String> {
        with(|t| {
            t.calls.push(format!("unlock({} chars)", password.chars().count()));
            if t.answers.is_empty() {
                Err("no answer scripted".to_string())
            } else {
                t.answers.remove(0)
            }
        })
    }
    fn fake_close(_: PromptWindow) {
        with(|t| t.calls.push("close".to_string()));
    }

    static FAKE_CALLS: PromptCalls = PromptCalls {
        open: fake_open,
        protect: fake_protect,
        next: fake_next,
        take_password: fake_take_password,
        show_error: fake_show_error,
        busy: fake_busy,
        unlock: fake_unlock,
        close: fake_close,
    };

    // ---- the protection ----------------------------------------------------

    /// **The one assertion the spike says must exist**, and it is about a
    /// call being made rather than a constant being present.
    ///
    /// `SetWindowDisplayAffinity` is refused on a child `EDIT` with
    /// `E_INVALIDARG`, so the only correct target is the top-level window.
    /// A version of this test that checked `WDA_EXCLUDEFROMCAPTURE` appears in
    /// the source would pass against code that applied it to the field and
    /// silently left the password capturable.
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let _serial = start(vec![Event::Cancel], &[], vec![]);
        assert_eq!(run_with(&FAKE_CALLS), Outcome::Cancelled);

        let tape = calls();
        assert!(
            tape.contains(&format!("protect({:#x})", FAKE.top_level)),
            "the prompt never asked for capture exclusion at all: {tape:?}"
        );
        assert!(
            !tape.contains(&format!("protect({:#x})", FAKE.field)),
            "capture exclusion was applied to the child EDIT, which Windows refuses with \
             E_INVALIDARG -- the master password would stay visible to screen capture: {tape:?}"
        );
        // Control on the pair above: the two handles really are different, so
        // the `contains` and the `!contains` are not both true of one value.
        assert_ne!(FAKE.top_level, FAKE.field);
    }

    /// And it is applied **before the window can be typed into**. A protection
    /// applied after the first pump leaves a window on screen, focused, taking
    /// keystrokes, and capturable.
    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        let _serial = start(vec![Event::Cancel], &[], vec![]);
        run_with(&FAKE_CALLS);

        let tape = calls();
        let protect = tape.iter().position(|c| c.starts_with("protect(")).expect("protected");
        let first_pump = tape.iter().position(|c| c == "next").expect("pumped");
        assert!(
            protect < first_pump,
            "the prompt pumped before excluding itself from capture: {tape:?}"
        );
        // ...and after `open`, because there is no window to protect before
        // that. Both bounds, so the ordering claim cannot be satisfied by a
        // `protect` that ran against a handle from a previous prompt.
        let open = tape.iter().position(|c| c == "open").expect("opened");
        assert!(open < protect, "{tape:?}");
    }

    /// A refused exclusion is a logged degradation, not a dead prompt. A user
    /// who cannot unlock at all is worse off than one whose prompt is
    /// capturable, and the refusal is on the record either way.
    #[test]
    fn a_refused_exclusion_still_lets_the_user_unlock() {
        let _serial = start(vec![Event::Submit], &["hunter2"], vec![Ok("token".to_string())]);
        with(|t| t.protects = false);

        assert_eq!(run_with(&FAKE_CALLS), Outcome::Unlocked("token".to_string()));
    }

    // ---- the loop ----------------------------------------------------------

    #[test]
    fn cancelling_closes_the_window_and_unlocks_nothing() {
        let _serial = start(vec![Event::Cancel], &[], vec![]);

        assert_eq!(run_with(&FAKE_CALLS), Outcome::Cancelled);
        let tape = calls();
        assert!(tape.contains(&"close".to_string()), "{tape:?}");
        assert!(
            !tape.iter().any(|c| c.starts_with("unlock(")),
            "cancelling spawned a `bw unlock`: {tape:?}"
        );
        assert!(
            !tape.contains(&"take_password".to_string()),
            "cancelling read the password field: {tape:?}"
        );
    }

    /// A window destroyed underneath the pump is a cancel, not a hang. This is
    /// the case a `while !closed` loop written the obvious way spins on
    /// forever.
    #[test]
    fn a_window_that_disappears_is_a_cancel() {
        let _serial = start(vec![Event::Closed], &[], vec![]);

        assert_eq!(run_with(&FAKE_CALLS), Outcome::Cancelled);
        assert!(calls().contains(&"close".to_string()));
    }

    #[test]
    fn a_window_that_cannot_open_is_unavailable_rather_than_declined() {
        let _serial = start(vec![], &[], vec![]);
        with(|t| t.can_open = false);

        // Deliberately NOT `Cancelled`. A caller deciding whether to fall back
        // to the full egui window has to be able to tell "the user said no"
        // from "we could not ask".
        assert_eq!(run_with(&FAKE_CALLS), Outcome::Unavailable);
        assert!(
            !calls().contains(&"close".to_string()),
            "closed a window that was never opened: {:?}",
            calls()
        );
    }

    #[test]
    fn a_correct_password_answers_with_the_session_token_and_closes() {
        let _serial = start(vec![Event::Submit], &["correct horse"], vec![Ok("SESSION==".to_string())]);

        assert_eq!(run_with(&FAKE_CALLS), Outcome::Unlocked("SESSION==".to_string()));
        let tape = calls();
        assert_eq!(tape.last().map(String::as_str), Some("close"), "{tape:?}");
        assert!(tape.contains(&"unlock(13 chars)".to_string()), "{tape:?}");
    }

    /// **A wrong password leaves the prompt up.** Closing on a failed attempt
    /// would mean a mistyped character costs the user the whole gesture --
    /// `CTRL+ALT+B`, the overlay, all of it -- which is the behaviour that
    /// makes a re-prompt worse than no prompt.
    #[test]
    fn a_wrong_password_shows_a_line_and_keeps_asking() {
        let _serial = start(
            vec![Event::Submit, Event::Submit],
            &["wrong", "right"],
            vec![
                Err("ERROR bitwarden_crypto::keys::master_key: error=The decryption operation \
                     failed"
                    .to_string()),
                Ok("SESSION==".to_string()),
            ],
        );

        assert_eq!(run_with(&FAKE_CALLS), Outcome::Unlocked("SESSION==".to_string()));

        let tape = calls();
        // Exactly one close, at the end -- the failed attempt did not take the
        // window down and then somehow put it back.
        assert_eq!(tape.iter().filter(|c| *c == "close").count(), 1, "{tape:?}");
        assert_eq!(tape.iter().filter(|c| c.starts_with("unlock(")).count(), 2, "{tape:?}");
        // The line shown is `login_ui`'s wording, not the CLI's crypto-layer
        // noise. Asserted against the function rather than a literal, so the
        // two surfaces cannot start describing one failure differently.
        let expected = crate::login_ui::friendly_auth_error(
            "ERROR bitwarden_crypto::keys::master_key: error=The decryption operation failed",
        );
        assert!(
            tape.contains(&format!("show_error({expected})")),
            "the prompt showed something other than `login_ui`'s wording: {tape:?}"
        );
        // Control: that wording is not the raw stderr, so the assertion above
        // is about the translation and not about any string at all.
        assert!(!expected.contains("bitwarden_crypto"), "{expected}");
    }

    /// The busy state brackets the CLI call on **both** sides of a failure.
    /// A prompt left greyed after a wrong password is a prompt the user
    /// cannot correct.
    #[test]
    fn the_controls_come_back_after_a_failed_attempt() {
        let _serial = start(vec![Event::Submit, Event::Cancel], &["wrong"], vec![Err("nope".to_string())]);
        run_with(&FAKE_CALLS);

        let tape = calls();
        let on = tape.iter().position(|c| c == "busy(true)").expect("greyed");
        let off = tape.iter().position(|c| c == "busy(false)").expect("un-greyed");
        let attempt = tape.iter().position(|c| c.starts_with("unlock(")).expect("attempted");
        assert!(on < attempt && attempt < off, "{tape:?}");
    }

    /// A blank field never reaches the CLI.
    ///
    /// Not politeness: `bw` takes the password through `--passwordenv`, and an
    /// empty environment variable on Windows is indistinguishable from an
    /// unset one, so the CLI answers by describing our own plumbing. That is
    /// `login_ui::missing_credential_message`'s finding and this gate is that
    /// function, which is why the expected line is read off it here.
    #[test]
    fn a_blank_password_is_refused_without_spawning_anything() {
        let _serial = start(vec![Event::Submit, Event::Cancel], &[""], vec![]);
        run_with(&FAKE_CALLS);

        let tape = calls();
        assert!(
            !tape.iter().any(|c| c.starts_with("unlock(")),
            "a blank field reached the CLI: {tape:?}"
        );
        let expected = crate::login_ui::missing_credential_message(
            crate::login_ui::BwStatus::Locked,
            "",
            "",
        )
        .expect("a blank password is refused");
        assert!(tape.contains(&format!("show_error({expected})")), "{tape:?}");
        // ...and the window is still up afterwards, so the user can type.
        assert_eq!(tape.iter().filter(|c| *c == "close").count(), 1, "{tape:?}");
    }

    #[test]
    fn the_gate_matches_the_login_windows() {
        assert_eq!(gate(""), Submit::Refuse(crate::login_ui::enter_password()));
        assert_eq!(gate("a"), Submit::Attempt);
        // Whitespace is a real password as far as this app is concerned, and
        // as far as `bw` is: only *empty* is the plumbing hazard.
        assert_eq!(gate("   "), Submit::Attempt);
    }

    // ---- the secret --------------------------------------------------------

    /// **The `Debug` on `Outcome` does not print the session token.**
    ///
    /// `debug_leak_guard` seeds on `Zeroizing`, and a `BW_SESSION` token is a
    /// `String` -- which that guard's own module doc names as its biggest
    /// hole. So this is the hole plugged by hand, with a test, at the one type
    /// in this module that carries one.
    #[test]
    fn the_outcome_does_not_print_its_session_token() {
        let printed = format!("{:?}", Outcome::Unlocked("s3cr3t-session-token".to_string()));
        assert!(!printed.contains("s3cr3t"), "the token reached a Debug line: {printed}");
        assert!(printed.contains("redacted"), "{printed}");
        // Control: the other two arms still print something a log line can
        // use, so the redaction did not flatten the enum to one string.
        assert_eq!(format!("{:?}", Outcome::Cancelled), "Cancelled");
        assert_ne!(format!("{:?}", Outcome::Unavailable), format!("{:?}", Outcome::Cancelled));
    }

    // ---- the layout, without a window --------------------------------------

    /// Every control is inside the window it is painted in.
    ///
    /// This window does not scroll and has no resize border, so a control past
    /// the bottom edge is not clipped -- it is *gone*, with no gesture that can
    /// reveal it. The same discipline `overlay_ui` holds for its card, and the
    /// spike's `layout` subcommand checked for the same reason: it needs no
    /// window, so it can be a real test rather than a comment.
    #[test]
    fn every_control_fits_inside_the_window() {
        let l = layout();
        let boxes = [
            ("mark", l.mark),
            ("wordmark", l.wordmark),
            ("pill", l.pill),
            ("close_glyph", l.close_glyph),
            ("title", l.title),
            ("subtitle", l.subtitle),
            ("field", l.field),
            ("progress", l.progress),
            ("unlock", l.unlock),
        ];
        // Positive control: the list is not empty and the window is not
        // degenerate, so the loop below is a real check rather than a vacuous
        // one over zero rows.
        assert_eq!(boxes.len(), 9);
        assert!(l.window.w > 0 && l.window.h > 0, "{:?}", l.window);

        for (name, b) in boxes {
            assert!(b.x >= 0 && b.y >= 0, "`{name}` starts outside the window: {b:?}");
            assert!(
                b.right() <= l.window.w,
                "`{name}` runs past the window's right edge ({} > {})",
                b.right(),
                l.window.w
            );
            assert!(
                b.bottom() <= l.window.h,
                "`{name}` falls past the bottom of a window that cannot scroll ({} > {})",
                b.bottom(),
                l.window.h
            );
        }
    }

    /// The password box sits inside the field's 1 px border on every side,
    /// so the focus border is drawn whole -- the box once ran to the field's
    /// bottom edge and covered the bottom of it.
    #[test]
    fn the_password_box_stays_inside_the_fields_border() {
        let l = layout();
        let edit = password_child(l.field);
        assert!(edit.x > l.field.x && edit.right() < l.field.right(), "{edit:?} in {:?}", l.field);
        assert!(edit.y > l.field.y && edit.bottom() < l.field.bottom(), "{edit:?} in {:?}", l.field);
        assert_eq!(edit.y - l.field.y, l.field.bottom() - edit.bottom(), "centred top to bottom");
    }

    /// The card's blocks stack top to bottom without touching: header, the
    /// two lines, the password box, then the button -- design 1b's Locked
    /// panel with the box between its text and its button.
    #[test]
    fn the_blocks_stack_in_the_designs_order() {
        let l = layout();
        let order = [("header", l.mark), ("title", l.title), ("subtitle", l.subtitle), ("field", l.field), ("unlock", l.unlock)];
        for pair in order.windows(2) {
            let ((a, above), (b, below)) = (pair[0], pair[1]);
            assert!(above.bottom() <= below.y, "`{a}` runs into `{b}`: {above:?} / {below:?}");
        }
        // The bar runs in the gap under the box, never over the button.
        assert!(l.progress.y >= l.field.bottom() && l.progress.bottom() <= l.unlock.y);
        // The pill sits between the wordmark and the close glyph.
        assert!(l.pill.x >= l.wordmark.right() && l.pill.right() <= l.close_glyph.x);
    }

    /// The card is the panels' card: their width, padding, control heights.
    #[test]
    fn the_prompt_is_the_panels_card() {
        assert_eq!(WIDTH, crate::panel_draw::CARD_W);
        assert_eq!(layout().window.w, WIDTH);
        let l = layout();
        assert_eq!(l.field.h, crate::panel_draw::BUTTON_H);
        assert_eq!(l.unlock.h, crate::panel_draw::TALL_BUTTON_H);
        assert_eq!(l.field.w, WIDTH - 2 * crate::panel_draw::PAD);
        assert_eq!(l.unlock.w, l.field.w);
        assert_eq!(l.window.bottom() - l.unlock.bottom(), crate::panel_draw::PAD);
    }

    /// The locked path records the app and answers `Unlock` at once, the
    /// record names that app on the prompt, and an ask clears it so the
    /// tray's own Unlock never inherits another app's name.
    #[test]
    fn the_locked_path_opens_this_prompt_for_its_app() {
        assert_eq!(
            for_locked_window("Ledgerline Desktop", Some((10.0, 20.0))),
            crate::locked_card::LockedAnswer::Unlock
        );
        assert_eq!(locked_window(), Some(("Ledgerline Desktop".to_string(), Some((10.0, 20.0)))));
        let (_, body) = prompt_text(Some("Ledgerline Desktop"));
        assert!(body.contains("Ledgerline Desktop"), "{body}");
        forget_locked_window();
        assert_eq!(locked_window(), None);
        let (_, tray_body) = prompt_text(None);
        assert!(!tray_body.contains("Ledgerline"), "{tray_body}");
    }

    /// The title is unique, which is what keeps `foreground::pick`'s `find`
    /// exact while this window is up alongside the tray's and the hotkey
    /// listener's.
    #[test]
    fn the_prompt_opens_under_a_title_no_other_window_of_ours_uses() {
        for other in [
            crate::vault_window::WINDOW_TITLE,
            // `preflight_card::PREFLIGHT_CARD_TITLE` was here. Design 4b's
            // send confirmation has been removed -- see
            // `vault_window::preflight`'s module doc -- so there is no such
            // window left to collide with.
            crate::picker_prompt::PICKER_PROMPT_TITLE,
            crate::vault_window::rehearsal::SCRATCH_TITLE,
            crate::region_overlay::REGION_TITLE,
        ] {
            assert_ne!(UNLOCK_PROMPT_TITLE, other);
        }
        assert!(!UNLOCK_PROMPT_TITLE.is_empty());
    }

    // ---- the brand, shared rather than copied ------------------------------

    /// The Win32 surface draws the design's shield, not a lookalike.
    ///
    /// Four quadrants, in the design's 24x28 artboard, in the checkerboard
    /// tone order `theme` deliberately diverged to. If a future edit reaches
    /// for a hand-rolled polygon here, this fails.
    #[test]
    fn the_prompt_paints_the_same_mark_the_rest_of_the_app_does() {
        let outlines = crate::theme::quadrant_outlines();
        assert_eq!(outlines.len(), 4);
        assert_eq!(crate::theme::QUADRANT_FILLS.len(), 4);
        for quadrant in outlines {
            assert!(quadrant.len() >= 4, "a quadrant is not a polygon: {quadrant:?}");
            for p in quadrant {
                assert!((0.0..=24.0).contains(&p.x), "outside the artboard: {p:?}");
                assert!((0.0..=28.0).contains(&p.y), "outside the artboard: {p:?}");
            }
        }
    }

    /// The GDI face names are the ones really in the font files.
    ///
    /// GDI matches on the legacy `name` records, which hold four styles per
    /// family, so Archivo's SemiBold and ExtraBold each carry their own family
    /// name and are `Regular` within it. Asking for `("Archivo", 600)` returns
    /// something that is not SemiBold -- silently, and looking almost right,
    /// which is the failure that made the last raw Win32 surface in this
    /// project read as foreign.
    #[test]
    fn the_win32_font_lookup_names_the_faces_the_files_actually_declare() {
        assert_eq!(crate::theme::gdi_face_for(crate::theme::SEMIBOLD), ("Archivo SemiBold", 400));
        assert_eq!(crate::theme::gdi_face_for(crate::theme::EXTRABOLD), ("Archivo ExtraBold", 400));
        assert_eq!(crate::theme::gdi_face_for(crate::theme::BOLD), ("Archivo", 700));
        // Every face the prompt asks for is one of the four bundled cuts.
        for family in [crate::theme::SEMIBOLD, crate::theme::BOLD, crate::theme::EXTRABOLD] {
            assert!(
                crate::theme::ARCHIVO_FACES.iter().any(|(egui, ..)| *egui == family),
                "`{family}` is not a bundled face"
            );
        }
    }
}

/// The prompt's window procedure must not post a THREAD quit.
///
/// No test can open the real window, so this is a source pin in this crate's
/// established shape ([`crate::app`]'s wiring tests, [`crate::job_object`]'s
/// scanners): it reads this file back with `include_str!`, cuts the
/// `#[cfg(test)]` modules away so only what SHIPS is scanned, strips `//`
/// comments so the explanation at the `WM_DESTROY` arm may name the call it
/// forbids, and asserts the call is absent.
///
/// **Normalised first.** This is a CRLF checkout with no `.gitattributes`;
/// slicing or comparing lines without trimming the carriage return makes the
/// cut a no-op and the whole pin vacuous. The control assertions below are
/// what prove it did not silently scan nothing, or the wrong half.
#[cfg(test)]
mod no_thread_quit_pin {
    // Split across two literals, on ONE line, in this crate's idiom:
    // `include_str!` pulls this module in too, so a whole needle would match
    // its own declaration, and a needle with a newline in it is vacuous on one
    // of the two possible checkouts.
    const FORBIDDEN: &str = concat!("PostQuit", "Message");
    const DESTROY_ARM: &str = concat!("WM_DESTROY", " =>");

    /// `source` with CRLF normalised, every top-level `#[cfg(test)]` module
    /// removed, and every `//` comment stripped.
    ///
    /// The module cut is line-based and anchored at column zero: a
    /// `#[cfg(test)]` on its own unindented line, up to and including the next
    /// unindented `}`. Every gated module in this file has that shape, and
    /// `the_cut_really_discards_something` checks that rather than assuming it.
    pub(super) fn production_only(source: &str) -> String {
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
            // Comments only: a `//` inside a string literal would be cut too,
            // but nothing this pin reads about lives in one, and cutting too
            // much can only make the scan MISS a comment, never invent a call.
            let code = match flat.find("//") {
                Some(at) => &flat[..at],
                None => flat,
            };
            out.push_str(code);
            out.push('\n');
        }
        assert!(!skipping, "a gated module never closed at column zero; the cut is unreliable");
        out
    }

    fn source() -> String {
        production_only(include_str!("unlock_prompt.rs"))
    }

    /// Control: the cut discarded something.
    ///
    /// A `production_only` that returned its input unchanged -- or an empty
    /// string -- would make the pin below pass for the wrong reason.
    #[test]
    fn the_cut_really_discards_something() {
        let whole = include_str!("unlock_prompt.rs");
        let kept = source();
        assert!(!kept.is_empty(), "the cut kept nothing at all; the pin would be vacuous");
        assert!(
            kept.len() < whole.len(),
            "the cut discarded nothing, so the gated modules -- including this one, which names \
             the forbidden call -- are still being scanned"
        );
        // This module's own declaration is inside the half that was cut.
        assert!(
            !kept.contains("mod no_thread_quit_pin"),
            "this pin's own module survived the cut, so it would scan itself"
        );
    }

    /// Control: the half that was KEPT is the window procedure's.
    ///
    /// If the cut ever ate the production half instead, the pin would pass on
    /// an empty-ish string forever. The `WM_DESTROY` arm is the exact line the
    /// rule is about, so requiring it proves the scan is looking at it.
    #[test]
    fn the_kept_half_still_contains_the_destroy_arm() {
        assert!(
            source().contains(DESTROY_ARM),
            "the kept half no longer contains the `WM_DESTROY` arm, so the pin below is not \
             scanning the code it exists to guard"
        );
    }

    /// Control: the scan can see the call when it is really there.
    #[test]
    fn the_scan_would_notice_the_call() {
        let planted = production_only(concat!("    PostQuit", "Message(0);\n"));
        assert!(planted.contains(FORBIDDEN), "the scanner cannot see a call that is present");
        // ...and not when it is only mentioned in a comment.
        let commented = production_only(concat!("    // PostQuit", "Message(0);\n"));
        assert!(!commented.contains(FORBIDDEN), "the comment strip does not work");
    }

    #[test]
    fn the_unlock_prompt_never_posts_a_thread_quit() {
        assert!(
            !source().contains(FORBIDDEN),
            "the unlock prompt posts a thread quit. `close` destroys this window SYNCHRONOUSLY on \
             the calling thread, and that thread goes on to run egui windows: \
             `unlock_from_the_locked_card` returns into `resume_fill_after_unlock`, which opens \
             the autofill overlay and the preflight window. Nothing drains the quit in between, so \
             the next `eframe::run_native` takes it out of `GetMessageW`, leaves its loop BEFORE \
             it draws, and returns its default answer -- the fill the user just unlocked for \
             silently does nothing. `GONE` is what `next` reads; the quit is redundant as well as \
             harmful."
        );
    }
}

/// **The unlock this prompt performs is the one this account's backend
/// understands.**
///
/// [`run_bw_unlock`] used to name [`crate::login_ui::run_bw_with_password`]
/// directly -- the CLI arm, and only the CLI arm -- with no consultation of
/// [`crate::backend_policy`] anywhere on the path from the locked card to the
/// spawn. The card that opens this prompt is raised on
/// `VaultCache::is_populated` alone (`crate::app::disposition` takes no
/// backend input at all), so it is raised on direct-REST accounts too, where
/// `bw unlock` addresses a profile that was never signed in to.
///
/// Three source facts, because this file's `bw` calls cannot be *run* by a
/// test -- `bw_path`'s verified-exe path is a process global other tests in
/// this suite set, so calling [`run_bw_unlock`] here would attempt a
/// `CreateProcess` against whatever they left behind:
///
///  * the two-armed function is what the worker calls, and it is handed a
///    [`crate::backend_policy`] answer rather than `None`;
///  * the CLI arm still asks for `unlock --raw`, which the closure's `&str`
///    signature would not have protected;
///  * there is no second, ungated route to the CLI left in this file.
///
/// What `authenticate_then_wipe` then DOES with the two arms is pinned where
/// it lives, by `login_ui`'s `a_direct_rest_account_never_spawns_the_cli` and
/// its `a_bw_serve_account_still_spawns_the_cli` control, both driven over
/// closures that record instead of spawning. This module is about the wiring
/// into it, which is the part that was missing.
#[cfg(test)]
mod the_unlock_asks_which_backend {
    use super::no_thread_quit_pin::production_only;

    fn source() -> String {
        production_only(include_str!("unlock_prompt.rs"))
    }

    /// **The `bw serve` arm still runs exactly `bw unlock --raw`.**
    ///
    /// The argument vector, unchanged, because that is the half of the CLI
    /// arm the reshuffle could have altered silently: `authenticate_then_wipe`
    /// hands its `run_cli` closure a `&str` and nothing else, so a closure
    /// that had picked up a different subcommand -- or lost `--raw`, and with
    /// it the bare-token output this prompt returns -- would still compile and
    /// still be reached.
    ///
    /// **A source pin and not a call, deliberately.** Calling
    /// [`super::run_bw_unlock`] takes the CLI arm all the way to
    /// `Command::output`, and `bw_path`'s verified-exe path is a process
    /// global that other tests in this suite set -- so a call here would be a
    /// test that attempts a `CreateProcess` against whatever path happened to
    /// be installed, which is both flaky and the thing no test in this crate
    /// may do. Observed once, as `os error 3`.
    ///
    /// What the arm DOES once entered is `login_ui`'s
    /// `a_bw_serve_account_still_spawns_the_cli`, over a `run_cli` that
    /// records instead of spawning.
    #[test]
    fn the_cli_arm_still_asks_for_an_unlock_and_a_raw_token() {
        assert!(
            source().contains(concat!("&[\"unlock\", ", "\"--raw\"]")),
            "the unlock prompt's CLI arm no longer runs `bw unlock --raw`. Without \
             `--raw` the CLI prints a sentence and this prompt returns it as a session \
             token; with a different subcommand it returns nothing usable at all"
        );
    }

    /// **The other arm is asked for.**
    ///
    /// A source pin and not a behaviour test, deliberately: taking the
    /// direct-REST arm for real means `DirectRestLogin::second_factor.start`,
    /// which is six hundred thousand PBKDF2 iterations and a network round
    /// trip to a self-hosted server. No test in this crate may do either.
    #[test]
    fn the_unlock_worker_consults_the_backend_policy() {
        let source = source();
        assert!(
            source.contains(concat!("authenticate_then_", "wipe(")),
            "the unlock prompt no longer turns the master password into a session through \
             the one function that has both arms, so a direct-REST account is back to \
             being offered a `bw unlock` against a profile it never signed in to"
        );
        assert!(
            source.contains(concat!("direct_rest_", "login()")),
            "the unlock prompt asks for no direct-REST login, so `authenticate_then_wipe` \
             is handed `None` and the CLI arm is the only one it can take"
        );
    }

    /// **And there is no second route to the CLI in this file.**
    ///
    /// One mention, inside the closure `authenticate_then_wipe` calls only on
    /// the `bw serve` arm. A second would be a `bw unlock` reachable without
    /// the backend having been consulted, which is exactly the shape this
    /// change removed.
    #[test]
    fn the_cli_is_named_once_and_only_inside_the_gated_closure() {
        let source = source();
        assert_eq!(
            source.matches(concat!("run_bw_with_", "password(")).count(),
            1,
            "`bw unlock` is spawned from more than one place in this file, so the arm \
             `authenticate_then_wipe` gates is not the only way to one"
        );
        let at = source
            .find(concat!("authenticate_then_", "wipe("))
            .expect("asserted by the test above");
        let cli = source
            .find(concat!("run_bw_with_", "password("))
            .expect("asserted by the count above");
        assert!(
            at < cli,
            "the one `bw unlock` is written BEFORE the call that is supposed to gate it, \
             so it is not the closure's body"
        );
    }
}

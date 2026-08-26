//! **Design 4b: the send preflight, in bare Win32.**
//!
//! The confirmation shown to the user *before* a stored sequence types a real
//! password into whatever window is in front. This is the **seventh** surface
//! in this crate drawn with `CreateWindowExW` and GDI rather than with egui --
//! after `crate::unlock_prompt`, `crate::picker_prompt`,
//! `crate::generate_prompt`, `crate::prompt_card`, `crate::locked_card` and
//! `crate::save_login_card` -- and it is **the last egui window on the
//! daemon's fill path**.
//!
//! # Why it is not an egui window any more
//!
//! The tray daemon measures 9.9 MB with no window ever opened. The moment any
//! egui window opens it becomes ~60 MB resident and **never returns**: the
//! OpenGL driver's committed arenas survive the window's destruction and are
//! only reclaimed at process exit. The six Win32 cards already in this crate
//! measure ~1.8 MB with their window on screen. With this card ported, the
//! daemon can complete a whole fill -- match, prompt, unlock, confirm, type --
//! **without ever creating a GL context**, which is the precondition for
//! splitting the app into two processes. `crate::preflight_host`, the module
//! that held the egui window, is gone with it.
//!
//! # This card holds the secret by value, and that is why it is here
//!
//! Every other card in this crate could in principle be drawn by a separate UI
//! process. This one cannot: `copy_payload` is the value that is about to be
//! typed or copied, and a process boundary would mean putting it on a pipe. So
//! it is redrawn here, in the daemon, and the care it takes is:
//!
//! * the payload is taken **by value** as a [`zeroize::Zeroizing<String>`], so
//!   every return from [`run_with`] -- including the reentrancy refusal and
//!   every failure inside `open` -- drops it and wipes it;
//! * it is handed to exactly one seam, [`PreflightCalls::copy`], and as a
//!   `&str`, so it is never cloned into an owned value this module keeps;
//! * `open` is **not** handed it at all -- the window is built from
//!   [`card_text`], which reads only [`crate::vault_window::detail_edit::StepRow`]
//!   fields the editor already masked;
//! * nothing secret is logged, reaches a `Debug`, or crosses the `fn`-pointer
//!   seam except as that one `&str`.
//!
//! # The refusal is the answer, always
//!
//! `None` means **do not send**. `crate::preflight_host`'s doc said it and it
//! survives the port unchanged: a confirmation that answered `Send` because it
//! could not be shown would be the exact inversion of what this window is for.
//! Every failure path here -- a second preflight, a window that would not open
//! -- answers `None`, and the one path that could answer `Send` for a refused
//! verdict answers [`PreflightAction::Cancel`] instead. See [`run_with`].
//!
//! # Screen-capture exclusion goes on the TOP-LEVEL window
//!
//! `SetWindowDisplayAffinity` is refused on a child control with
//! `E_INVALIDARG` -- measured on `unlock_prompt`, not assumed. It goes on the
//! top-level window, which covers every child it owns, and it goes on **before
//! the first pump**. What it protects here is the name and pid of the window
//! this user is signing in to, and the step list that says a password is about
//! to be typed into it.
//!
//! # Space is the send, and it is not a button press
//!
//! There is no send *button*: the most dangerous action in the app must not be
//! reachable by a stray click on a window that just took focus, so the send is
//! a **held key**. That has a consequence particular to Win32 which the egui
//! card did not have: a focused `BUTTON` treats the space bar as a click. So
//! `VK_SPACE` is intercepted in the pump **before** `IsDialogMessageW` and
//! before `DispatchMessageW`, and never reaches a control. On this card the
//! space bar is the hold and nothing else; the two footer buttons are pressed
//! with the mouse, or with Enter while they hold focus.

use crate::vault_window::preflight::{
    self, PreflightAction, PreflightState, Refusal, Verdict,
};
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

/// One preflight at a time per process. A second one would be a second window
/// asking about a foreground that the first one is already standing in front
/// of.
static PREFLIGHT_OPEN: AtomicBool = AtomicBool::new(false);

/// The window's title, and **it is this card's own**.
///
/// The egui host opened under the literal `"Deskwarden"` that `vault_window`,
/// `app_window` and `loading_ui` all raise under, which is the reason its row
/// in `foreground`'s exemption table gave for not raising. This card holds its
/// own `HWND` and never finds a window by name, and its title is distinct from
/// every other title this crate opens under -- so `crate::foreground::pick`'s
/// `find` cannot bring one of the others forward instead.
pub const PREFLIGHT_CARD_TITLE: &str = "Deskwarden send preflight";

/// The header's own words, under the brand lockup. Distinct from
/// [`preflight::HEADING_TARGET`], which is the caption over the target name;
/// this is what the card *is*.
pub const PREFLIGHT_CARD_LABEL: &str = "Confirm before sending";

/// How many step rows the list draws before it starts saying how many it did
/// not.
///
/// A cap and not a scrollbar: this window is frameless, always-on-top and
/// cannot scroll, so a list of any length would run past the bottom edge and
/// take the hold affordance with it. A cap that hid rows *without saying so*
/// is the defect this project keeps finding, which is why the overflow is
/// drawn as its own row -- see [`CardText::dropped`].
pub const STEP_CAP: usize = 8;

/// The overflow row's words, when a sequence has more steps than [`STEP_CAP`].
pub fn dropped_line(dropped: usize) -> String {
    format!("+{dropped} more step(s) not shown")
}

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the same reason
/// `save_login_card::SaveWindow` is: a decision layer a test can drive must not
/// name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreflightWindow(pub isize);

/// What the user did with the window.
///
/// **No secret reaches this type**, and no typed text does either -- this card
/// has no text boxes. It is four answers and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Escape, the header ✕, *Cancel · Esc* or *Dismiss*.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// The hold completed. **Not a click**, and never emitted while the
    /// verdict is a refusal -- see [`win32::next`], which does not read the
    /// key at all in that state, and [`run_with`], which refuses it a second
    /// time if it ever arrives.
    Send,
    /// *Copy instead*: the escape the design offers beside the refusal.
    CopyInstead,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct PreflightCalls {
    /// Lays out and shows the card for `text`. `None` if it could not be put
    /// on screen.
    ///
    /// **It is handed [`CardText`] and never the payload.** The window has
    /// nothing to draw the secret with and is given no way to.
    pub open: fn(&CardText) -> Option<PreflightWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the **top-level**
    /// window, called before the first `next`.
    pub protect: fn(PreflightWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PreflightWindow) -> Event,
    /// Destroys the window and releases its resources.
    pub close: fn(PreflightWindow),
    /// Puts the payload on the clipboard. `crate::clipboard::copy_secret` in
    /// production, pinned by address in
    /// [`tests::the_production_calls_are_the_real_ones`].
    ///
    /// **A `&str` and not an owned value**: the only secret this module holds
    /// is the `Zeroizing<String>` [`run_with`] owns, and this seam borrows it
    /// rather than being given a copy to keep.
    pub copy: fn(&str),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// 1. The reentrancy guard first. Its refusal is `None`, which every caller
///    reads as *do not send* -- and `copy_payload`, taken by value, is dropped
///    and wiped on that path exactly as on every other.
/// 2. `protect` runs immediately after `open` and **before the first `next`**.
/// 3. `close` runs on every exit path that has a window to close. `open`
///    returning `None` returns before ever calling it, because there is no
///    window there.
/// 4. **A refused verdict cannot answer `Send`.** The pump does not read the
///    hold key in that state and the paint path draws no hold affordance, so
///    `Event::Send` should be unreachable there; if it arrives anyway it is
///    answered [`PreflightAction::Cancel`]. Three independent barriers, because
///    the failure this one protects against is a password typed into a chat
///    box.
/// 5. The payload reaches `copy` and nothing else, on the one answer that asks
///    for it.
pub fn run_with(
    calls: &PreflightCalls,
    state: PreflightState,
    copy_payload: Zeroizing<String>,
) -> Option<PreflightAction> {
    if PREFLIGHT_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "a preflight was requested while one is already open in this process; refusing \
             rather than stacking a second confirmation over the same foreground"
        );
        // **`None`, which the caller reads as "do not send".** A second
        // confirmation that answered `Send` because it could not be shown
        // would be the exact inversion of what this window is for.
        //
        // `copy_payload` is dropped on this line and wiped with it.
        return None;
    }
    // Released by `Drop`, so a panic inside `decide` cannot leave the process
    // unable to ever show a preflight again -- which would turn one crash into
    // every subsequent gated fill refusing.
    let _open = OpenGuard;
    decide(calls, &state, &copy_payload)
}

/// Releases [`PREFLIGHT_OPEN`] however [`run_with`] leaves.
struct OpenGuard;

impl Drop for OpenGuard {
    fn drop(&mut self) {
        PREFLIGHT_OPEN.store(false, Ordering::SeqCst);
    }
}

/// [`run_with`]'s body, once the reentrancy guard is held.
fn decide(
    calls: &PreflightCalls,
    state: &PreflightState,
    copy_payload: &Zeroizing<String>,
) -> Option<PreflightAction> {
    let allowed = matches!(state.verdict, Verdict::Allowed);
    let text = card_text(state);

    let Some(window) = (calls.open)(&text) else {
        log::warn!(
            "the send preflight could not be put on screen; nothing will be typed"
        );
        return None;
    };

    // Before the first pump, so the window is excluded from capture before the
    // name and pid of the app this user is signing in to are on screen.
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the send preflight; what it names is \
             visible to screen capture on this machine"
        );
    }

    let event = (calls.next)(window);
    (calls.close)(window);

    match event {
        Event::Cancel | Event::Closed => Some(PreflightAction::Cancel),
        Event::CopyInstead => {
            // The clipboard is the one place this value is allowed to go, and
            // it goes there because the user asked for it in preference to
            // typing. Borrowed, never moved.
            (calls.copy)(copy_payload.as_str());
            Some(PreflightAction::CopyInstead)
        }
        Event::Send if allowed => Some(PreflightAction::Send),
        Event::Send => {
            // Unreachable through the shipped pump, which does not read the
            // hold key while the verdict is a refusal, and through the paint
            // path, which draws no hold affordance there. Kept because the
            // thing on the other side of it is a password typed into the wrong
            // window, and a refusal is cheap.
            log::warn!(
                "the preflight pump answered a send for a refused target; nothing was sent"
            );
            Some(PreflightAction::Cancel)
        }
    }
}

/// **Puts design 4b on screen and answers what the user decided.**
///
/// The signature `preflight_host::show_preflight` had, so
/// `crate::vault_window::preflight::SendGate::production` changes by one path
/// and nothing else.
pub fn show_preflight_card(
    state: PreflightState,
    copy_payload: Zeroizing<String>,
) -> Option<PreflightAction> {
    run_with(&REAL, state, copy_payload)
}

/// [`show_preflight_card`], told which [`PreflightCalls`] to use.
///
/// `examples/preflight_preview.rs` is its one non-production caller, swapping
/// [`PreflightCalls::protect`] and [`PreflightCalls::copy`] for stubs so the
/// window can be screenshotted without touching the real clipboard.
pub fn ask_with(
    calls: &PreflightCalls,
    state: PreflightState,
    copy_payload: Zeroizing<String>,
) -> Option<PreflightAction> {
    run_with(calls, state, copy_payload)
}

/// The production [`PreflightCalls`].
pub static REAL: PreflightCalls = PreflightCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    close: win32::close,
    copy: crate::clipboard::copy_secret,
};

// ---------------------------------------------------------------------------
// What the card says
// ---------------------------------------------------------------------------

/// One step of the sequence, as the card draws it.
///
/// **Every field is already masked.** They are copied out of
/// [`crate::vault_window::detail_edit::StepRow`], which `PreflightState::new`
/// built with the eye shut -- `step_rows(.., false)` writes
/// `SECRET_MASK` for a password in an `if` whose `else` is the only branch
/// that can resolve a value. Nothing here re-reads a vault item, and nothing
/// here is handed the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub number: String,
    pub label: String,
    /// The mask for a secret, the resolved value for a revealed non-secret,
    /// and empty otherwise.
    pub payload: String,
    /// Whether the row carries [`preflight::MASKED_ONLY`] after it.
    pub secret: bool,
}

/// Everything the window paints, as plain strings.
///
/// A snapshot rather than the [`PreflightState`] itself, for two reasons. The
/// window procedure has nowhere to keep a borrow, so what it draws has to live
/// in a `static`; and a snapshot is the narrowest thing that can be put there
/// -- it holds no `SendTarget`, no rows, no verdict and no way to reach the
/// vault.
///
/// **`Debug` is derived and that is safe here**: every string in it came
/// through [`card_text`], which reads only already-masked fields.
/// `debug_leak_guard` is the test that holds that claim for the crate.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CardText {
    /// `true` for [`Verdict::Allowed`]. The card has exactly two shapes and
    /// this is which one.
    pub allowed: bool,
    /// The heading: [`preflight::HEADING_TARGET`] when allowed,
    /// [`preflight::REFUSED_HEADING`] when refused.
    pub heading: String,
    /// The window the sequence would be typed into, by its own title.
    pub target: String,
    /// The line under it: image, pid and rule claim when allowed; image and
    /// focused class when refused.
    pub subtitle: String,
    /// The refusal, in words. Empty when allowed.
    pub message: String,
    /// The steps, capped at [`STEP_CAP`]. Empty when refused -- a refused card
    /// lists nothing it will not do.
    pub steps: Vec<Step>,
    /// How many steps the cap hid. Drawn as its own row, never silently.
    pub dropped: usize,
    /// The left footer button: [`preflight::CANCEL_LABEL`] when allowed,
    /// [`preflight::DISMISS_LABEL`] when refused.
    pub dismiss_label: String,
}

/// **The card's words, from the state, and nothing else.**
///
/// Pure, so every string the window can paint is reachable from a test with no
/// desktop. It reads `state.rows` only through the three already-masked fields
/// [`Step`] names, and never the payload -- which it is not given.
pub fn card_text(state: &PreflightState) -> CardText {
    match state.verdict {
        Verdict::Allowed => {
            let (shown, truncated) =
                crate::win32_draw::visible_rows(state.rows.len(), STEP_CAP);
            CardText {
                allowed: true,
                heading: preflight::HEADING_TARGET.to_string(),
                target: state.target.title.clone(),
                subtitle: preflight::target_line(state),
                message: String::new(),
                steps: state.rows[..shown]
                    .iter()
                    .map(|row| Step {
                        number: row.number.to_string(),
                        label: row.label.clone(),
                        payload: row.payload.clone(),
                        secret: row.secret,
                    })
                    .collect(),
                dropped: if truncated { state.rows.len() - shown } else { 0 },
                dismiss_label: preflight::CANCEL_LABEL.to_string(),
            }
        }
        Verdict::Refused(why) => CardText {
            allowed: false,
            heading: preflight::REFUSED_HEADING.to_string(),
            target: state.target.title.clone(),
            subtitle: format!(
                "{} \u{b7} {} focused",
                state.target.image_name, state.target.class_name
            ),
            message: preflight::refusal_message(state, why),
            // **No step list on a refusal.** The design's refused state lists
            // nothing: the card is telling the user what it will NOT do, and a
            // list of steps beside that reads as an offer.
            steps: Vec::new(),
            dropped: 0,
            dismiss_label: preflight::DISMISS_LABEL.to_string(),
        },
    }
}

/// Which refusal a state carries, for a caller that wants it without matching
/// on [`Verdict`] itself. Used by the preview.
pub fn refusal_of(state: &PreflightState) -> Option<Refusal> {
    match state.verdict {
        Verdict::Refused(why) => Some(why),
        Verdict::Allowed => None,
    }
}

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the six
// Win32 cards this one sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's. The same
/// [`crate::picker_prompt::WIDTH`] every other daemon card is, because two
/// frameless daemon cards of different widths read as two different programs.
pub const WIDTH: i32 = crate::picker_prompt::WIDTH;

/// Content inset, and the top margin.
const MARGIN_X: i32 = 14;
const MARGIN_TOP: i32 = 12;

/// The heading caption's line box -- 11px type.
const CAPTION_H: i32 = 14;

/// The target name's line box -- 15px type.
const TARGET_H: i32 = 21;

/// The line under the target -- 11px type.
const SUBTITLE_H: i32 = 16;

/// One step row's pitch, and the four lanes across it.
const STEP_H: i32 = 20;
const STEP_NUM_W: i32 = 14;
const STEP_GAP: i32 = 4;
const STEP_LABEL_W: i32 = 78;
const STEP_PAYLOAD_W: i32 = 96;

/// The hold affordance. Full content width, so nothing else can be put beside
/// the one control on this card that sends.
const HOLD_H: i32 = 30;

/// Button height. `theme::BUTTON_HEIGHT`, pinned by
/// [`tests::the_cards_dimensions_are_the_themes`].
const BUTTON_H: i32 = 32;

/// The footer's two answers, and the gap between them.
const DISMISS_W: i32 = 106;
const COPY_W: i32 = 112;
const FOOTER_GAP: i32 = 8;

/// The footnote under the footer -- 11px type.
const FOOTNOTE_H: i32 = 15;

/// One wrapped line of the refusal message.
const MESSAGE_LINE_H: i32 = 17;

/// How many characters of the refusal message fit on one line of the content
/// column at 12px.
///
/// **Deliberately an under-estimate.** `layout` is pure and has no DC to
/// measure with, and the direction of the error matters: too few characters
/// per line over-counts the lines, which makes the box taller than the text
/// needs. Too many would under-count and clip the last line off a window that
/// cannot scroll.
const MESSAGE_CHARS_PER_LINE: usize = 48;

/// The most lines of refusal message the card will grow for.
///
/// Both refusal messages this app can produce fit well inside it; the cap is
/// what stops a future one from pushing the two answers off the bottom edge of
/// a frameless window. The message is drawn `DT_WORDBREAK` into exactly this
/// box, so a longer one is clipped rather than allowed to escape.
pub const MESSAGE_LINES_MAX: usize = 6;

/// How many lines `run` takes at `chars_per_line`, wrapped on whitespace.
///
/// Greedy, and pure, so [`layout`] stays a function a test can run. A word
/// longer than the line takes a line of its own rather than looping forever.
pub fn wrapped_lines(run: &str, chars_per_line: usize) -> usize {
    if run.trim().is_empty() || chars_per_line == 0 {
        return 0;
    }
    let mut lines = 1usize;
    let mut used = 0usize;
    for word in run.split_whitespace() {
        let w = word.chars().count();
        if used == 0 {
            used = w;
        } else if used + 1 + w <= chars_per_line {
            used += 1 + w;
        } else {
            lines += 1;
            used = w;
        }
    }
    lines
}

/// One rectangle of the card, in logical pixels from the window's top left.
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

/// The four lanes of one step row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepBoxes {
    pub number: Box2,
    pub label: Box2,
    pub payload: Box2,
    /// [`preflight::MASKED_ONLY`], for a secret row.
    pub tail: Box2,
}

/// Every rectangle the card paints, computed once.
///
/// Pure arithmetic with no Win32 in it, for `prompt_card::layout`'s reason: a
/// control whose bottom edge fell past the window's would simply be invisible
/// on a window that neither scrolls nor resizes. On this card the control past
/// the edge would be the hold affordance -- the only way to send -- or *Copy
/// instead*, which is the escape offered to a user who has just been refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and the wordmark beside it. **Not
    /// optional.** Four cards lost the lockup in porting and it had to be
    /// restored afterwards; a frameless always-on-top window that is about to
    /// type a password has to say whose window it is.
    pub mark: Box2,
    pub wordmark: Box2,
    pub card_label: Box2,
    pub close_glyph: Box2,
    /// [`CardText::heading`].
    pub heading: Box2,
    pub target: Box2,
    pub subtitle: Box2,
    pub header_rule: Box2,
    /// [`preflight::HEADING_STEPS`]. `None` on a refusal, which lists nothing.
    pub steps_caption: Option<Box2>,
    pub steps: Vec<StepBoxes>,
    /// The overflow row, when the cap hid steps.
    pub dropped: Option<Box2>,
    /// The hold affordance. **`None` on a refusal**, which is the geometric
    /// half of "a refused target is never offered a way to ask".
    pub hold: Option<Box2>,
    /// The refusal, wrapped. `None` when allowed.
    pub message: Option<Box2>,
    pub footer_rule: Box2,
    /// The tinted band the two answers sit on.
    pub footer: Box2,
    pub dismiss: Box2,
    pub copy: Box2,
    /// [`preflight::FOOTNOTE`]. `None` on a refusal, where nothing is being
    /// sent for focus to interrupt.
    pub footnote: Option<Box2>,
}

/// **The card's geometry, for the two shapes it has.**
///
/// The window is sized to its content and to nothing else: the last control
/// plus one margin *is* the height, so a control that stopped being drawn
/// shortens the window rather than leaving a hole nothing notices.
pub fn layout(text: &CardText) -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let lockup = crate::win32_draw::card_lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };
    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP - 2, w: 20, h: 20 };
    let card_label =
        Box2 { x: MARGIN_X, y: mark.bottom() + lockup.gap_below, w: content_w - 24, h: TARGET_H };

    let heading = Box2 { x: MARGIN_X, y: card_label.bottom() + 8, w: content_w, h: CAPTION_H };
    let target = Box2 { x: MARGIN_X, y: heading.bottom() + 2, w: content_w, h: TARGET_H };
    let subtitle = Box2 { x: MARGIN_X, y: target.bottom(), w: content_w, h: SUBTITLE_H };
    let header_rule = Box2 { x: 0, y: subtitle.bottom() + 10, w: WIDTH, h: 1 };

    let mut cursor = header_rule.bottom() + 10;

    let (steps_caption, steps, dropped, hold, message) = if text.allowed {
        let caption = Box2 { x: MARGIN_X, y: cursor, w: content_w, h: CAPTION_H };
        cursor = caption.bottom() + 4;

        let mut boxes = Vec::with_capacity(text.steps.len());
        for i in 0..text.steps.len() {
            let y = cursor + i as i32 * STEP_H;
            let number = Box2 { x: MARGIN_X, y, w: STEP_NUM_W, h: STEP_H };
            let label =
                Box2 { x: number.right() + STEP_GAP, y, w: STEP_LABEL_W, h: STEP_H };
            let payload =
                Box2 { x: label.right() + STEP_GAP, y, w: STEP_PAYLOAD_W, h: STEP_H };
            let tail = Box2 {
                x: payload.right() + STEP_GAP,
                y,
                w: WIDTH - MARGIN_X - (payload.right() + STEP_GAP),
                h: STEP_H,
            };
            boxes.push(StepBoxes { number, label, payload, tail });
        }
        cursor += text.steps.len() as i32 * STEP_H;

        let dropped = if text.dropped > 0 {
            let at = Box2 { x: MARGIN_X, y: cursor, w: content_w, h: CAPTION_H };
            cursor = at.bottom();
            Some(at)
        } else {
            None
        };

        let hold = Box2 { x: MARGIN_X, y: cursor + 12, w: content_w, h: HOLD_H };
        cursor = hold.bottom();
        (Some(caption), boxes, dropped, Some(hold), None)
    } else {
        let lines = wrapped_lines(&text.message, MESSAGE_CHARS_PER_LINE)
            .clamp(1, MESSAGE_LINES_MAX) as i32;
        let at = Box2 { x: MARGIN_X, y: cursor, w: content_w, h: lines * MESSAGE_LINE_H };
        cursor = at.bottom();
        (None, Vec::new(), None, None, Some(at))
    };

    let footer_rule = Box2 { x: 0, y: cursor + 12, w: WIDTH, h: 1 };
    let dismiss =
        Box2 { x: MARGIN_X, y: footer_rule.bottom() + 10, w: DISMISS_W, h: BUTTON_H };
    let copy =
        Box2 { x: dismiss.right() + FOOTER_GAP, y: dismiss.y, w: COPY_W, h: BUTTON_H };

    let footnote = if text.allowed {
        Some(Box2 { x: MARGIN_X, y: copy.bottom() + 8, w: content_w, h: FOOTNOTE_H })
    } else {
        None
    };

    let height = footnote.map(|f| f.bottom()).unwrap_or_else(|| copy.bottom()) + MARGIN_TOP;
    let window = Box2 { x: 0, y: 0, w: WIDTH, h: height };
    let footer =
        Box2 { x: 0, y: footer_rule.bottom(), w: WIDTH, h: height - footer_rule.bottom() };

    Layout {
        window,
        mark,
        wordmark,
        card_label,
        close_glyph,
        heading,
        target,
        subtitle,
        header_rule,
        steps_caption,
        steps,
        dropped,
        hold,
        message,
        footer_rule,
        footer,
        dismiss,
        copy,
        footnote,
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Whether the window has gone away underneath the pump.
static GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the window procedure recorded, taken by `next` rather than read, so no
/// event can be delivered twice.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// What the paint path draws. Written by `open`, read by every painter, and
/// **cleared by `close`**: it holds the name of a window this user was signing
/// in to, and nothing needs it once the card is down.
static TEXT: std::sync::Mutex<Option<CardText>> = std::sync::Mutex::new(None);

/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it is
/// not restated: a themed control renders in the shell's grey with the shell's
/// font, and the last raw-Win32 surface in this project was deleted for looking
/// foreign rather than for being broken. The two footer buttons are real
/// `BUTTON` windows -- which is what buys focus and `IsDialogMessage`
/// traversal -- with their painting taken over completely and handed to
/// [`crate::win32_draw`], the module all seven cards draw through so none can
/// drift from the palette.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: an egui window was measured at ~102 MB and a D2D device at
/// 53.85 MB against the Win32 prompt's 1.79 MB.
///
/// # GDI object hygiene
///
/// Every brush, pen, font and DC created below is restored and deleted before
/// its function returns. This is a daemon's repaint path -- and this card
/// repaints its hold bar on a timer while the key is down -- so a leaked handle
/// here exhausts the table over a keystroke rather than over a session.
mod win32 {
    use super::{
        dropped_line, Box2, CardText, Event, PreflightWindow, GONE, PENDING,
        PREFLIGHT_CARD_LABEL, PREFLIGHT_CARD_TITLE, TEXT,
    };
    use crate::vault_window::preflight::{
        advance_hold, hold_complete, CANCEL_LABEL, COPY_INSTEAD_LABEL, DISMISS_LABEL, FOOTNOTE,
        HEADING_STEPS, HOLD_HINT, HOLD_TO_SEND, MASKED_ONLY,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, RoundRect,
        SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_CENTER, DT_END_ELLIPSIS,
        DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FW_BOLD, FW_NORMAL,
        HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetForegroundWindow, GetWindowLongPtrW, IsDialogMessageW,
        LoadCursorW, PeekMessageW, RegisterClassW, SendMessageW, SetForegroundWindow,
        SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED,
        BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, GWLP_WNDPROC, HMENU, IDC_ARROW, MSG,
        PM_REMOVE, SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND,
        WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEMOVE,
        WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_TOPMOST,
        WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::win32_draw::{draw_button, draw_card_lockup, rgb, ButtonSkin};

    const ID_DISMISS: usize = 101;
    const ID_COPY: usize = 102;

    /// `BM_CLICK`. The `windows` crate does not project the `BUTTON` control's
    /// messages under the features this crate enables, so it is the documented
    /// constant, named here rather than left as a bare hex literal at the call
    /// -- exactly as `unlock_prompt` names `EM_SETSEL`.
    const BM_CLICK: u32 = 0x00F5;

    const CLASS_NAME: PCWSTR = w!("DeskwardenPreflightCard");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The system DPI, not the monitor's**, and a known limitation rather
    /// than an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the
    /// whole argument: `GetDpiForWindow` lives behind a `windows` crate feature
    /// this crate does not enable, and enabling it re-pins `job_object.rs`'s
    /// whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed `BUTTON`s' original procedure. One slot for both: they
    /// are the same `BUTTON` class registered by the same comctl32, so the
    /// procedure replaced is the same pointer.
    static BUTTON_PROC: AtomicIsize = AtomicIsize::new(0);

    /// Whether the card is in its allowed state. **Read by the pump before it
    /// looks at the hold key at all**, so a refused card accumulates nothing.
    static ALLOWED: AtomicBool = AtomicBool::new(false);

    /// How far the hold has got, in thousandths. Written by the pump, read by
    /// the paint path -- which is the only thing the two share.
    static HELD_PER_MILLE: AtomicI32 = AtomicI32::new(0);

    // ---- fonts -------------------------------------------------------------

    /// Registers the bundled Archivo cuts privately with GDI, once.
    ///
    /// `AddFontMemResourceEx` makes a face available to **this process only** --
    /// nothing is installed and nothing touches the user's font list -- and the
    /// handles are deliberately never released, because freeing one while a
    /// window still has it selected is how a surface repaints in the fallback
    /// face.
    fn register_fonts() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            for (_, _, _, bytes) in crate::theme::ARCHIVO_FACES {
                let installed = std::cell::Cell::new(0u32);
                let handle = AddFontMemResourceEx(
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32,
                    None,
                    installed.as_ptr(),
                );
                if handle.0.is_null() || installed.get() == 0 {
                    log::warn!("could not register a bundled Archivo face with GDI");
                }
            }
        });
    }

    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the card paints with, created at open and destroyed at close.
    /// Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        brand: HFONT,
        title: HFONT,
        caption: HFONT,
        body: HFONT,
        button: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                brand: font(BOLD, crate::win32_draw::card_lockup().word_px),
                title: font(BOLD, 15),
                caption: font(REGULAR, 11),
                body: font(REGULAR, 12),
                button: font(SEMIBOLD, 12),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.brand, self.title, self.caption, self.body, self.button] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them;
    // the `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl std::marker::Send for Fonts {}

    // ---- the window --------------------------------------------------------

    pub(super) fn open(text: &CardText) -> Option<PreflightWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        HELD_PER_MILLE.store(0, Ordering::SeqCst);
        ALLOWED.store(text.allowed, Ordering::SeqCst);
        if let Ok(mut slot) = TEXT.lock() {
            *slot = Some(text.clone());
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }

        unsafe {
            DPI_PERCENT.store(
                {
                    let dc = GetDC(None);
                    let dpi = GetDeviceCaps(dc, LOGPIXELSX);
                    ReleaseDC(None, dc);
                    if dpi > 0 {
                        dpi * 100 / 96
                    } else {
                        100
                    }
                },
                Ordering::SeqCst,
            );
        }

        register_class();
        // **Destroy the previous set before overwriting it.** `Fonts` has no
        // `Drop` -- it holds raw `HFONT`s -- so assigning over a `Some` would
        // leak five fonts per `open` that ran without a matching `close`.
        {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        let l = super::layout(text);
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        // **No anchor.** This card is not beside a field: it is a question
        // about the window in front, so it goes where every OS credential
        // prompt puts itself. `prompt_card::place`'s `None` arm is that
        // placement, and it is the crate's one placement function so the seven
        // cards cannot drift into seven clamps.
        let (x, y) = placed(w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the
                // user was doing.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(PREFLIGHT_CARD_TITLE),
                // Frameless. A `WS_CAPTION` frame is the loudest "system
                // dialog" signal there is, and this app's own windows are
                // frameless with drawn chrome.
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

        // **Below this line the card is on screen.** `WS_VISIBLE` is in the
        // style, so a bare `?` here would return `None`, make `run_with` answer
        // `None`, and leave a frameless topmost card with no controls and no
        // way for the user to dismiss it -- `close` is only reached with a
        // `PreflightWindow` in hand. Every failure path from here on goes
        // through `abandon`, which takes the window down and frees the fonts
        // before answering `None`.
        fn abandon(window: HWND) -> Option<PreflightWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            if let Ok(mut slot) = TEXT.lock() {
                *slot = None;
            }
            None
        }

        // The handle is copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself, so holding the guard
        // across the `child` calls below would deadlock the failure path.
        let Some(button_font) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| f.button))
        else {
            return abandon(window);
        };

        for (id, at) in [(ID_DISMISS, l.dismiss), (ID_COPY, l.copy)] {
            let Some(control) = child(
                window,
                w!("BUTTON"),
                WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                at,
                id,
                button_font,
            ) else {
                return abandon(window);
            };
            subclass(control, &BUTTON_PROC, control_proc);
        }

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // **It asks for the foreground, and the refusal is handled rather
            // than asserted.** The egui host was excused from raising on the
            // ground that it opened under a shared title; this one has a title
            // of its own and holds its own `HWND`. It has to have focus for a
            // reason particular to this card: the send is a HELD KEY, and a
            // card without focus sends that key to the app it is standing in
            // front of -- which, for this card, is a run of spaces typed into
            // the very password box the sequence was aimed at.
            let _ = SetForegroundWindow(window);
            // **Focus stays on the top-level window**, not on a button. The
            // space bar is the hold; a focused `BUTTON` would treat it as a
            // click. The pump intercepts `VK_SPACE` before anything can see
            // it, and this is the second half of that: Tab reaches the two
            // answers when the user asks for them, and nothing has them by
            // default.
            let _ = SetFocus(window);
        }

        Some(PreflightWindow(handle_of(window)))
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns.
    pub(super) fn protect(window: PreflightWindow) -> bool {
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks**, and it is the one pump in this crate with a clock in
    /// it: the send is a hold, so time has to be accumulated between messages
    /// rather than read off one.
    ///
    /// Three properties are held here and each is load-bearing:
    ///
    /// * **`VK_SPACE` never reaches a control.** It is taken out of the queue
    ///   before `IsDialogMessageW` and before `DispatchMessageW`, so a focused
    ///   `BUTTON` cannot read the hold key as a click on itself.
    /// * **The hold is not read at all while the verdict is a refusal.** The
    ///   `ALLOWED` branch is around the accumulation, not around a comparison
    ///   inside it, so there is no frame on which a refused card credits a
    ///   held key with anything.
    /// * **Focus leaving the window throws the hold away**, which is what the
    ///   card's own footnote promises. Read off `GetForegroundWindow` each
    ///   tick rather than off a `WM_KILLFOCUS`, because the window can lose the
    ///   foreground to another process without either.
    pub(super) fn next(window: PreflightWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN, VK_SPACE};

        let top = hwnd(window.0);
        let allowed = ALLOWED.load(Ordering::SeqCst);
        let mut space_down = false;
        let mut held = Duration::ZERO;
        let mut last = Instant::now();

        loop {
            if GONE.load(Ordering::SeqCst) {
                return Event::Closed;
            }
            if let Some(event) = take_pending() {
                return event;
            }

            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        return Event::Closed;
                    }
                    let key = msg.wParam.0 as u16;
                    if msg.message == WM_KEYDOWN && key == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    // **Taken out of the queue here and dispatched nowhere.**
                    // See the function doc.
                    if (msg.message == WM_KEYDOWN || msg.message == WM_KEYUP)
                        && key == VK_SPACE.0
                    {
                        space_down = msg.message == WM_KEYDOWN;
                        continue;
                    }
                    // Enter presses whichever answer holds focus. There is no
                    // default button on this card: the only thing a default
                    // could be is the send, and the send is deliberately not
                    // reachable by pressing one key once.
                    if msg.message == WM_KEYDOWN && key == VK_RETURN.0 {
                        let focused = GetFocus();
                        if !focused.is_invalid() && focused != top {
                            SendMessageW(focused, BM_CLICK, WPARAM(0), LPARAM(0));
                        }
                        continue;
                    }
                    if !IsDialogMessageW(top, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    if let Some(event) = take_pending() {
                        return event;
                    }
                }
            }

            let now = Instant::now();
            let dt = now.saturating_duration_since(last);
            last = now;

            let ours = unsafe { GetForegroundWindow() } == top;
            // **The arithmetic is not this file's.** `advance_hold` and
            // `hold_complete` live beside the verdict they belong to, and the
            // property they carry -- a series of taps never adds up to a send
            // -- is tested there, on a pure function, over a range of `dt` no
            // window could be driven through.
            if allowed && space_down && ours {
                held = advance_hold(held, true, dt);
                if hold_complete(held) {
                    return Event::Send;
                }
            } else {
                held = advance_hold(held, false, dt);
            }

            let fraction = (held.as_secs_f32() / HOLD_TO_SEND.as_secs_f32()).clamp(0.0, 1.0);
            let per_mille = (fraction * 1000.0) as i32;
            if HELD_PER_MILLE.swap(per_mille, Ordering::SeqCst) != per_mille {
                repaint(top);
            }

            // The tick. Short enough that the bar grows smoothly over
            // `HOLD_TO_SEND`, long enough that a card sitting open is not a
            // spin.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    pub(super) fn close(window: PreflightWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        // The card named a window this user was signing in to. Nothing needs
        // it once the card is down.
        if let Ok(mut slot) = TEXT.lock() {
            *slot = None;
        }
        HELD_PER_MILLE.store(0, Ordering::SeqCst);
        ALLOWED.store(false, Ordering::SeqCst);
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

    fn repaint_all(window: HWND) {
        repaint(window);
        unsafe {
            for id in [ID_DISMISS, ID_COPY] {
                if let Ok(control) = GetDlgItem(window, id as i32) {
                    repaint(control);
                }
            }
        }
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    fn card_text() -> CardText {
        TEXT.lock().ok().and_then(|slot| slot.clone()).unwrap_or_default()
    }

    /// Where the window goes, through [`crate::prompt_card::place`] -- the
    /// crate's one placement function, given `None` because this card has no
    /// anchor. See `open`.
    fn placed(w: i32, h: i32) -> (i32, i32) {
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
            crate::prompt_card::place(
                None,
                (area.left, area.top, area.right, area.bottom),
                w,
                h,
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
                // what keeps the card from flashing system grey on a repaint --
                // and this card repaints on a timer.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// One child control. **`BUTTON`s are created with no text**: every label
    /// on this card is painted by `paint_control` from the app's own palette
    /// and type, so a control's own caption would only ever be a second, stale
    /// copy.
    fn child(
        parent: HWND,
        class: PCWSTR,
        style: u32,
        at: Box2,
        id: usize,
        font: HFONT,
    ) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
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

    fn round_corners(window: HWND) {
        unsafe {
            use windows::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            };
            let preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const _ as *const c_void,
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    /// Takes over a control's painting without losing the focus and keyboard
    /// behaviour that makes `IsDialogMessage` work.
    fn subclass(
        control: HWND,
        slot: &AtomicIsize,
        proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    ) {
        unsafe {
            let previous = SetWindowLongPtrW(control, GWLP_WNDPROC, proc as *const () as isize);
            if previous != 0 {
                slot.store(previous, Ordering::SeqCst);
            }
        }
    }

    /// Calls whatever procedure `slot` replaced, or `DefWindowProcW` if there
    /// is none.
    unsafe fn original(
        slot: &AtomicIsize,
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let previous = slot.load(Ordering::SeqCst);
        if previous == 0 {
            DefWindowProcW(control, msg, wparam, lparam)
        } else {
            CallWindowProcW(
                Some(std::mem::transmute::<
                    isize,
                    unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                >(previous)),
                control,
                msg,
                wparam,
                lparam,
            )
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
            // Frameless windows are dragged by their background. Dragging this
            // one does not change which window is in front of the user's app,
            // only where the question sits on screen.
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
                if in_close_glyph(lparam) {
                    set_pending(Event::Cancel);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint_all(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED {
                    clicked(id);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This card is opened on
                // the thread that was about to fill, and that thread goes on to
                // run more message loops -- the vault window's among them.
                // `close()` calls `DestroyWindow`, which dispatches this
                // message synchronously on that thread, so a `PostQuitMessage`
                // here would leave the thread's quit flag set with nothing left
                // to drain it: `next()` has already returned and no pump of
                // ours runs again. The next window's pump then takes that stale
                // `WM_QUIT` out of its queue and leaves before it draws a
                // frame. On this card that hazard is live rather than
                // theoretical.
                //
                // Quitting is not this handler's job in the first place: `GONE`
                // on the line below is what `next()` reads to report
                // `Event::Closed`, and the `WM_QUIT` branch in `next()` stays
                // for a quit posted from outside.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// What a click on control `id` means. **Neither answer is the send**:
    /// there is no button on this card that sends, which is the whole design.
    fn clicked(id: usize) {
        match id {
            ID_DISMISS => set_pending(Event::Cancel),
            ID_COPY => set_pending(Event::CopyInstead),
            _ => {}
        }
    }

    /// The subclassed `BUTTON`s: everything except painting and hover is the
    /// original procedure's, which is what keeps focus and
    /// `IsDialogMessage`'s traversal working.
    unsafe extern "system" fn control_proc(
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let id = GetWindowLongPtrW(control, windows::Win32::UI::WindowsAndMessaging::GWLP_ID);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_control(control, id as usize);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(id, Ordering::SeqCst) != id {
                    repaint(control);
                }
                LRESULT(0)
            }
            _ => original(&BUTTON_PROC, control, msg, wparam, lparam),
        }
    }

    /// The close glyph's rect in DEVICE pixels.
    ///
    /// One derivation, read by both the hit test and `in_close_glyph`, so the
    /// rect `WM_NCHITTEST` excuses from the drag and the rect `WM_LBUTTONDOWN`
    /// answers on can never be two different rectangles.
    fn close_glyph_rect() -> RECT {
        let l = super::layout(&card_text());
        RECT {
            left: scale(l.close_glyph.x),
            top: scale(l.close_glyph.y),
            right: scale(l.close_glyph.right()),
            bottom: scale(l.close_glyph.bottom()),
        }
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
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

            // Double-buffered: a surface painted straight to the window
            // flickers on every hover, and this one repaints on a timer.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let text = card_text();
            let l = super::layout(&text);
            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            fill_rect(mem, client, crate::theme::CARD);
            fill_box(mem, l.footer, crate::theme::CARD_TINT);
            fill_box(mem, l.header_rule, crate::theme::HAIRLINE);
            fill_box(mem, l.footer_rule, crate::theme::HAIRLINE);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                paint_lockup(mem, &l, fonts.brand);
                run(mem, fonts.title, l.card_label, PREFLIGHT_CARD_LABEL, crate::theme::INK);

                let heading_colour = if text.allowed {
                    crate::theme::TEXT_MUTED
                } else {
                    crate::theme::ERROR
                };
                let heading_font = if text.allowed { fonts.caption } else { fonts.title };
                run(mem, heading_font, l.heading, &text.heading, heading_colour);
                run(mem, fonts.title, l.target, &text.target, crate::theme::INK);
                run(
                    mem,
                    fonts.caption,
                    l.subtitle,
                    &text.subtitle,
                    crate::theme::TEXT_SECONDARY,
                );

                if let Some(at) = l.steps_caption {
                    run(mem, fonts.caption, at, HEADING_STEPS, crate::theme::TEXT_MUTED);
                }
                for (step, boxes) in text.steps.iter().zip(l.steps.iter()) {
                    run(
                        mem,
                        fonts.caption,
                        boxes.number,
                        &step.number,
                        crate::theme::TEXT_MUTED,
                    );
                    run(mem, fonts.body, boxes.label, &step.label, crate::theme::INK);
                    if !step.payload.is_empty() {
                        run(
                            mem,
                            fonts.body,
                            boxes.payload,
                            &step.payload,
                            crate::theme::TEXT_SECONDARY,
                        );
                    }
                    if step.secret {
                        run(
                            mem,
                            fonts.caption,
                            boxes.tail,
                            MASKED_ONLY,
                            crate::theme::TEXT_MUTED,
                        );
                    }
                }
                if let Some(at) = l.dropped {
                    run(
                        mem,
                        fonts.caption,
                        at,
                        &dropped_line(text.dropped),
                        crate::theme::TEXT_FAINT,
                    );
                }

                if let Some(at) = l.message {
                    paragraph(mem, fonts.body, at, &text.message, crate::theme::INK);
                }

                if let Some(at) = l.hold {
                    paint_hold(mem, at, fonts.button);
                }

                if let Some(at) = l.footnote {
                    run(mem, fonts.caption, at, FOOTNOTE, crate::theme::TEXT_MUTED);
                }
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

    /// **The hold affordance, and it is not a button.**
    ///
    /// A wash with a filled portion that grows while the key is down, and the
    /// design's own words across it. It is painted here rather than being a
    /// `BUTTON` child precisely so that it cannot be clicked: there is no
    /// control under it to receive a click and nothing in `clicked` that could
    /// answer one.
    fn paint_hold(hdc: HDC, at: Box2, font: HFONT) {
        rounded(hdc, at, 6, crate::theme::BLUE_WASH, None);
        let per_mille = HELD_PER_MILLE.load(Ordering::SeqCst).clamp(0, 1000);
        if per_mille > 0 {
            let filled = Box2 { w: at.w * per_mille / 1000, ..at };
            if filled.w > 0 {
                rounded(hdc, filled, 6, crate::theme::BLUE_EDGE, None);
            }
        }
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(crate::theme::BLUE));
            let mut chars: Vec<u16> = HOLD_HINT.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            DrawTextW(
                hdc,
                &mut chars,
                &mut rc,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
            SelectObject(hdc, old);
        }
    }

    /// One footer answer.
    fn paint_control(control: HWND, id: usize) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == id as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let text = card_text();
            let l = super::layout(&text);
            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            fill_rect(mem, whole, crate::theme::CARD_TINT);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                // **Neither answer is primary.** The primary action on this
                // card is the hold, and it is not a button; giving *Copy
                // instead* the blue would make the escape look like the
                // affirmative.
                let (label, at) = if id == ID_DISMISS {
                    (dismiss_label(&text), l.dismiss)
                } else {
                    (COPY_INSTEAD_LABEL.to_string(), l.copy)
                };
                let skin = ButtonSkin::secondary();
                let skin = if hovered { skin.hovered() } else { skin };
                if focused {
                    // **The ring is given LOGICAL size, from `layout`.**
                    // `rounded` scales everything it is handed, and `rc` came
                    // back from `GetClientRect` in device pixels already.
                    rounded(
                        mem,
                        Box2 { x: 0, y: 0, w: at.w, h: at.h },
                        9,
                        crate::theme::FOCUS_RING,
                        None,
                    );
                    let inner = RECT {
                        left: whole.left + 2,
                        top: whole.top + 2,
                        right: whole.right - 2,
                        bottom: whole.bottom - 2,
                    };
                    draw_button(mem, inner, &label, fonts.button, skin, scale(8));
                } else {
                    draw_button(mem, whole, &label, fonts.button, skin, scale(8));
                }
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// What the left answer says: the design gives the two states different
    /// words, because *Cancel* implies something was going to happen and on a
    /// refusal nothing was.
    fn dismiss_label(text: &CardText) -> String {
        if text.dismiss_label.is_empty() {
            if text.allowed { CANCEL_LABEL } else { DISMISS_LABEL }.to_string()
        } else {
            text.dismiss_label.clone()
        }
    }

    /// The brand lockup, through [`crate::win32_draw::draw_card_lockup`] -- the
    /// crate's one mark painter, which every card draws through.
    fn paint_lockup(hdc: HDC, l: &super::Layout, font: HFONT) {
        let dev = |b: Box2| RECT {
            left: scale(b.x),
            top: scale(b.y),
            right: scale(b.right()),
            bottom: scale(b.bottom()),
        };
        let tracking = scale(crate::win32_draw::card_lockup().tracking);
        draw_card_lockup(hdc, dev(l.mark), dev(l.wordmark), font, tracking);
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        unsafe {
            use windows::Win32::Graphics::Gdi::{LineTo, MoveToEx};
            let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(crate::theme::TEXT_FAINT));
            let old = SelectObject(hdc, pen);
            let (x, y, w, h) = (scale(at.x), scale(at.y), scale(at.w), scale(at.h));
            let pad = w / 3;
            let _ = MoveToEx(hdc, x + pad, y + pad, None);
            let _ = LineTo(hdc, x + w - pad, y + h - pad);
            let _ = MoveToEx(hdc, x + w - pad, y + pad, None);
            let _ = LineTo(hdc, x + pad, y + h - pad);
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
    }

    fn fill_rect(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// [`fill_rect`], for a **logical** rectangle.
    fn fill_box(hdc: HDC, at: Box2, colour: eframe::egui::Color32) {
        fill_rect(
            hdc,
            RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()).max(scale(at.y) + 1),
            },
            colour,
        );
    }

    /// A rounded rectangle in logical coordinates, optionally stroked.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        border: Option<(i32, eframe::egui::Color32)>,
    ) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill_colour));
            let (width, colour) = border.unwrap_or((1, fill_colour));
            let pen = CreatePen(PS_SOLID, scale(width).max(1), rgb(colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let r = scale(radius) * 2;
            let _ = RoundRect(
                hdc,
                scale(at.x),
                scale(at.y),
                scale(at.right()),
                scale(at.bottom()),
                r,
                r,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
    }

    /// One run of text, left-aligned, vertically centred and truncated with an
    /// ellipsis rather than clipped mid-letter.
    ///
    /// Every run this card paints is bounded: the card cannot scroll and cannot
    /// resize, and the target's own title is whatever the foreground window
    /// calls itself.
    fn run(hdc: HDC, font: HFONT, at: Box2, text: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut chars: Vec<u16> = text.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `DT_NOPREFIX`: these are window titles and process names, in
            // which an `&` is an ampersand and never a mnemonic that would be
            // drawn as an underscore.
            DrawTextW(
                hdc,
                &mut chars,
                &mut rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(hdc, old);
        }
    }

    /// The refusal, wrapped into the box [`super::layout`] measured for it.
    ///
    /// `DT_WORDBREAK` into a bounded rectangle: a message longer than
    /// [`super::MESSAGE_LINES_MAX`] lines is clipped rather than allowed to run
    /// past the footer of a window that cannot scroll.
    fn paragraph(hdc: HDC, font: HFONT, at: Box2, text: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut chars: Vec<u16> = text.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            DrawTextW(hdc, &mut chars, &mut rc, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
            SelectObject(hdc, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::target::SendTarget;
    use crate::key_sequence::ResolveSource;
    use crate::vault_window::detail::TotpState;

    // `cfg(test)` seams are banned in this crate, so the fakes below are
    // ordinary `fn`s recording into statics, which is this crate's idiom.

    #[derive(Default)]
    struct Trace {
        opened: usize,
        protected: usize,
        closed: usize,
        copied: usize,
        /// Which order the calls arrived in, by name.
        order: Vec<&'static str>,
        /// The event `next` hands out.
        script: Option<Event>,
        /// What `open` was handed.
        opened_with: Option<CardText>,
        /// The LENGTH of what `copy` was handed, never the value.
        copied_len: usize,
        refuse_open: bool,
        refuse_protect: bool,
    }

    static TRACE: std::sync::Mutex<Option<Trace>> = std::sync::Mutex::new(None);

    /// **One test at a time.** The fakes record into a process-wide `TRACE`,
    /// because the seam is a struct of plain `fn` pointers -- the same shape
    /// the shipped one has to fit through, and one that has nowhere to put a
    /// closure's captures. `PREFLIGHT_OPEN` is process-wide for the same
    /// reason, so two tests in flight at once would refuse each other.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn trace<R>(f: impl FnOnce(&mut Trace) -> R) -> R {
        let mut guard = TRACE.lock().unwrap_or_else(|p| p.into_inner());
        f(guard.get_or_insert_with(Trace::default))
    }

    fn reset(script: Event) {
        let mut guard = TRACE.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(Trace { script: Some(script), ..Trace::default() });
    }

    fn fake_open(text: &CardText) -> Option<PreflightWindow> {
        trace(|t| {
            t.opened += 1;
            t.order.push("open");
            t.opened_with = Some(text.clone());
            if t.refuse_open {
                None
            } else {
                Some(PreflightWindow(7))
            }
        })
    }

    fn fake_protect(_: PreflightWindow) -> bool {
        trace(|t| {
            t.protected += 1;
            t.order.push("protect");
            !t.refuse_protect
        })
    }

    fn fake_next(_: PreflightWindow) -> Event {
        trace(|t| {
            t.order.push("next");
            t.script.unwrap_or(Event::Closed)
        })
    }

    fn fake_close(_: PreflightWindow) {
        trace(|t| {
            t.closed += 1;
            t.order.push("close");
        })
    }

    /// **Records the length and never the value.** This is a fake clipboard in
    /// a test, and the habit is still the point: a fixture recorded by value
    /// is a fixture printed by an assertion failure.
    fn fake_copy(payload: &str) {
        trace(|t| {
            t.copied += 1;
            t.order.push("copy");
            t.copied_len = payload.chars().count();
        })
    }

    static FAKE: PreflightCalls = PreflightCalls {
        open: fake_open,
        protect: fake_protect,
        next: fake_next,
        close: fake_close,
        copy: fake_copy,
    };

    fn target(image: &str, masked: bool) -> SendTarget {
        SendTarget {
            title: "SAP Logon 760 - Sign in".into(),
            image_name: image.into(),
            pid: 7412,
            class_name: "SAPFEWndClass".into(),
            focused_is_masked: masked,
        }
    }

    fn state_for(image: &str, masked: bool, sequence: &str) -> PreflightState {
        let totp = TotpState::NoSecret;
        PreflightState::new(
            target(image, masked),
            "saplogon.exe",
            sequence,
            &ResolveSource {
                username: "ada@example.com",
                password: "hunter2",
                custom: Vec::new(),
                totp: &totp,
            },
        )
    }

    fn allowed_state() -> PreflightState {
        state_for("saplogon.exe", true, "{USERNAME}{TAB}{PASSWORD}{ENTER}")
    }

    fn refused_state() -> PreflightState {
        state_for("slack.exe", false, "{USERNAME}{TAB}{PASSWORD}{ENTER}")
    }

    fn payload() -> Zeroizing<String> {
        Zeroizing::new("a-fixture-not-a-password".to_string())
    }

    // ---- the decision ------------------------------------------------------

    /// **Every answer, and the two that are refusals.**
    #[test]
    fn each_event_answers_the_action_it_names() {
        let _serial = serial();
        for (event, expected) in [
            (Event::Cancel, Some(PreflightAction::Cancel)),
            (Event::Closed, Some(PreflightAction::Cancel)),
            (Event::Send, Some(PreflightAction::Send)),
            (Event::CopyInstead, Some(PreflightAction::CopyInstead)),
        ] {
            reset(event);
            let answered = run_with(&FAKE, allowed_state(), payload());
            assert_eq!(answered, expected, "{event:?} answered {answered:?}");
        }
    }

    /// **The one that matters: a refused verdict cannot answer `Send`.**
    ///
    /// The pump does not read the hold key in that state and the paint path
    /// draws no hold affordance, so this event should be unreachable there --
    /// which is exactly why it is worth driving straight into `run_with`. On
    /// the other side of a `Some(Send)` here is a password typed into a chat
    /// box.
    #[test]
    fn a_refused_target_can_never_be_answered_send() {
        let _serial = serial();
        reset(Event::Send);
        let answered = run_with(&FAKE, refused_state(), payload());
        assert_eq!(
            answered,
            Some(PreflightAction::Cancel),
            "a refused preflight answered {answered:?}. The design's own example is a password \
             reaching a chat box"
        );
        assert_ne!(answered, Some(PreflightAction::Send));
    }

    /// A second preflight refuses rather than confirming, and `None` is a
    /// refusal everywhere it is read.
    #[test]
    fn a_second_preflight_refuses_rather_than_confirming() {
        let _serial = serial();
        reset(Event::Send);
        PREFLIGHT_OPEN.store(true, Ordering::SeqCst);
        let answered = run_with(&FAKE, allowed_state(), payload());
        PREFLIGHT_OPEN.store(false, Ordering::SeqCst);
        assert_eq!(answered, None, "a second preflight answered something other than a refusal");
        assert_eq!(trace(|t| t.opened), 0, "a second preflight opened a window");
    }

    /// And the guard is released however `run_with` leaves, so one refusal is
    /// not every subsequent fill refusing.
    #[test]
    fn the_reentrancy_guard_is_released_on_the_way_out() {
        let _serial = serial();
        reset(Event::Cancel);
        assert_eq!(run_with(&FAKE, allowed_state(), payload()), Some(PreflightAction::Cancel));
        assert!(
            !PREFLIGHT_OPEN.load(Ordering::SeqCst),
            "the preflight guard was left set, so every later gated fill would refuse"
        );
        reset(Event::Cancel);
        assert_eq!(
            run_with(&FAKE, allowed_state(), payload()),
            Some(PreflightAction::Cancel),
            "a second preflight after a clean exit was refused"
        );
    }

    /// **`protect` runs after `open` and before the first pump**, and `close`
    /// runs on the way out.
    #[test]
    fn the_window_is_protected_before_it_is_pumped() {
        let _serial = serial();
        reset(Event::Cancel);
        run_with(&FAKE, allowed_state(), payload());
        assert_eq!(
            trace(|t| t.order.clone()),
            vec!["open", "protect", "next", "close"],
            "the card was pumped before it was excluded from screen capture, or left open"
        );
    }

    /// A refused exclusion is a warning and not a crash -- the card still
    /// closes, and it still answers.
    #[test]
    fn a_refused_exclusion_still_leaves_a_card_that_closes() {
        let _serial = serial();
        reset(Event::Cancel);
        trace(|t| t.refuse_protect = true);
        assert_eq!(run_with(&FAKE, allowed_state(), payload()), Some(PreflightAction::Cancel));
        assert_eq!(trace(|t| t.closed), 1, "a card whose protection was refused was not closed");
    }

    /// **A window that could not be opened answers `None`, which is "do not
    /// send"** -- and it is not closed, because there is nothing to close.
    #[test]
    fn a_window_that_never_opened_refuses_and_is_never_closed() {
        let _serial = serial();
        reset(Event::Send);
        trace(|t| t.refuse_open = true);
        let answered = run_with(&FAKE, allowed_state(), payload());
        assert_eq!(
            answered, None,
            "a preflight that could not be shown answered {answered:?} rather than refusing. \
             That is the exact inversion of what this window is for"
        );
        assert_eq!(trace(|t| t.protected), 0, "a window that does not exist was protected");
        assert_eq!(trace(|t| t.closed), 0, "a window that does not exist was closed");
        assert_eq!(trace(|t| t.copied), 0, "a window that does not exist copied a secret");
    }

    // ---- the payload -------------------------------------------------------

    /// **The payload reaches exactly one seam, on exactly one answer.**
    ///
    /// `open` is handed [`CardText`], which is built from already-masked rows;
    /// `copy` is handed the payload, and only when the user asked for it.
    #[test]
    fn the_payload_reaches_the_clipboard_seam_and_nothing_else() {
        let _serial = serial();
        for (event, copies) in [
            (Event::Cancel, 0),
            (Event::Closed, 0),
            (Event::Send, 0),
            (Event::CopyInstead, 1),
        ] {
            reset(event);
            run_with(&FAKE, allowed_state(), payload());
            assert_eq!(
                trace(|t| t.copied),
                copies,
                "{event:?} put the payload on the clipboard {} time(s)",
                trace(|t| t.copied)
            );
        }
        reset(Event::CopyInstead);
        run_with(&FAKE, allowed_state(), payload());
        assert_eq!(
            trace(|t| t.copied_len),
            "a-fixture-not-a-password".chars().count(),
            "the value that reached the clipboard seam is not the one the caller handed in"
        );
    }

    /// **A refused card still offers *Copy instead*.** That is the escape the
    /// design gives beside the refusal: the user is told nothing will be typed
    /// here, and handed the value to place themselves.
    #[test]
    fn a_refused_card_still_offers_the_escape() {
        let _serial = serial();
        reset(Event::CopyInstead);
        assert_eq!(
            run_with(&FAKE, refused_state(), payload()),
            Some(PreflightAction::CopyInstead)
        );
        assert_eq!(trace(|t| t.copied), 1);
    }

    /// **Nothing the window is handed contains the payload.**
    ///
    /// Driven through the real `card_text`, on a state whose `ResolveSource`
    /// really does hold a password -- so a `step_rows(.., true)` slipping into
    /// `PreflightState::new`, or a payload threaded into `open` for
    /// convenience, fails here.
    #[test]
    fn the_window_is_never_handed_the_secret() {
        let _serial = serial();
        reset(Event::Cancel);
        run_with(&FAKE, allowed_state(), payload());
        let text = trace(|t| t.opened_with.clone()).expect("the card was opened");
        let drawn = format!("{text:?}");
        for secret in ["hunter2", "a-fixture-not-a-password"] {
            assert!(
                !drawn.contains(secret),
                "the value the card was handed to DRAW contains a secret. The step list is \
                 built by `step_rows(.., false)` with the eye shut and the payload is handed \
                 only to the clipboard seam"
            );
        }
        assert!(
            text.steps.iter().any(|s| s.secret),
            "control: this fixture's sequence really does type a secret, so the check above is \
             looking at a card that had one to leak"
        );
    }

    // ---- what the card says ------------------------------------------------

    #[test]
    fn the_allowed_card_lists_the_steps_and_the_refused_card_lists_none() {
        let allowed = card_text(&allowed_state());
        assert!(allowed.allowed);
        assert_eq!(allowed.heading, preflight::HEADING_TARGET);
        assert_eq!(allowed.dismiss_label, preflight::CANCEL_LABEL);
        assert_eq!(allowed.steps.len(), 4, "the four steps of the fixture sequence");
        assert!(allowed.message.is_empty());

        let refused = card_text(&refused_state());
        assert!(!refused.allowed);
        assert_eq!(refused.heading, preflight::REFUSED_HEADING);
        assert_eq!(refused.dismiss_label, preflight::DISMISS_LABEL);
        assert!(
            refused.steps.is_empty(),
            "the refused card lists steps. It is telling the user what it will NOT do, and a \
             list of steps beside that reads as an offer"
        );
        assert!(!refused.message.is_empty());
    }

    /// **The cap says how many it hid.** A cap that hides rows without saying
    /// so is the defect this project keeps finding.
    #[test]
    fn a_long_sequence_is_capped_and_says_so() {
        let long = "{USERNAME}{TAB}".repeat(9);
        let text = card_text(&state_for("saplogon.exe", true, &long));
        assert_eq!(text.steps.len(), STEP_CAP);
        assert_eq!(text.dropped, 18 - STEP_CAP);
        assert!(dropped_line(text.dropped).contains(&text.dropped.to_string()));
    }

    // ---- geometry ----------------------------------------------------------

    fn every_box(l: &Layout) -> Vec<(&'static str, Box2)> {
        let mut boxes: Vec<(&'static str, Box2)> = vec![
            ("mark", l.mark),
            ("wordmark", l.wordmark),
            ("card_label", l.card_label),
            ("close_glyph", l.close_glyph),
            ("heading", l.heading),
            ("target", l.target),
            ("subtitle", l.subtitle),
            ("header_rule", l.header_rule),
            ("footer_rule", l.footer_rule),
            ("dismiss", l.dismiss),
            ("copy", l.copy),
        ];
        for at in [l.steps_caption, l.dropped, l.hold, l.message, l.footnote]
            .into_iter()
            .flatten()
        {
            boxes.push(("optional", at));
        }
        for step in &l.steps {
            boxes.push(("step.number", step.number));
            boxes.push(("step.label", step.label));
            boxes.push(("step.payload", step.payload));
            boxes.push(("step.tail", step.tail));
        }
        boxes
    }

    /// **Nothing escapes the window, in either state and at every length this
    /// card can be handed.**
    ///
    /// Frameless, always-on-top, no scrollbar and no resize border. A control
    /// past the bottom edge here is the hold affordance -- the only way to
    /// send -- or *Copy instead*, which is the escape offered to a user who has
    /// just been refused. The user cannot reach either by any means.
    #[test]
    fn every_control_is_inside_the_window() {
        let mut texts = vec![card_text(&allowed_state()), card_text(&refused_state())];
        // The extremes: no steps at all, and a sequence long enough to be
        // capped; and a refusal message long enough to hit the line cap.
        texts.push(card_text(&state_for("saplogon.exe", true, "")));
        texts.push(card_text(&state_for(
            "saplogon.exe",
            true,
            &"{USERNAME}{TAB}".repeat(9),
        )));
        let mut long_refusal = card_text(&refused_state());
        long_refusal.message = "word ".repeat(200);
        texts.push(long_refusal);

        for text in texts {
            let l = layout(&text);
            for (name, at) in every_box(&l) {
                assert!(at.x >= 0, "`{name}` starts left of the window");
                assert!(at.y >= 0, "`{name}` starts above the window");
                assert!(
                    at.right() <= l.window.w,
                    "`{name}` ends {}px past the window's right edge, on a card that cannot \
                     scroll sideways",
                    at.right() - l.window.w
                );
                assert!(
                    at.bottom() <= l.window.h,
                    "`{name}` ends {}px past the window's bottom edge, on a card that has no \
                     scrollbar, no title bar and no resize border -- the user cannot reach it \
                     by any means",
                    at.bottom() - l.window.h
                );
            }
            // And the window is not taller than its content: the last control
            // plus one margin IS the height, so a control that stopped being
            // drawn shortens the window rather than leaving a hole nothing
            // notices.
            let last = l.footnote.map(|f| f.bottom()).unwrap_or_else(|| l.copy.bottom());
            assert_eq!(
                l.window.h,
                last + 12,
                "the window has slack in it that no test can tell from a control that vanished"
            );
        }
    }

    /// **A refused card has no hold affordance at all**, which is the geometric
    /// half of "a refused target is never offered a way to ask".
    #[test]
    fn the_refused_card_paints_no_way_to_send() {
        let refused = layout(&card_text(&refused_state()));
        assert!(
            refused.hold.is_none(),
            "the refused card lays out a hold affordance. `preflight::draw`'s refusal state \
             paints none, and neither does this one"
        );
        assert!(refused.steps.is_empty());
        assert!(refused.steps_caption.is_none());
        assert!(refused.message.is_some());

        let allowed = layout(&card_text(&allowed_state()));
        assert!(allowed.hold.is_some(), "control: the allowed card does lay one out");
        assert!(allowed.message.is_none());
    }

    #[test]
    fn no_two_controls_overlap() {
        for text in [card_text(&allowed_state()), card_text(&refused_state())] {
            let l = layout(&text);
            assert!(l.dismiss.right() < l.copy.x, "the two footer answers run into each other");
            assert!(
                l.wordmark.right() < l.close_glyph.x,
                "the wordmark runs into the header's ✕"
            );
            assert!(l.header_rule.bottom() <= l.footer_rule.y);
            for step in &l.steps {
                assert!(step.number.right() <= step.label.x);
                assert!(step.label.right() <= step.payload.x);
                assert!(step.payload.right() <= step.tail.x);
                assert!(step.tail.w > 0, "the masked-field note has no lane to be drawn in");
            }
            for pair in l.steps.windows(2) {
                assert!(pair[0].number.bottom() <= pair[1].number.y, "two steps overlap");
            }
        }
    }

    /// The card's dimensions come from `theme`, not from numbers invented here.
    #[test]
    fn the_cards_dimensions_are_the_themes() {
        assert_eq!(
            BUTTON_H,
            crate::theme::BUTTON_HEIGHT as i32,
            "the footer's buttons are no longer `theme::BUTTON_HEIGHT` tall"
        );
        assert_eq!(
            WIDTH,
            crate::picker_prompt::WIDTH,
            "two frameless daemon cards of different widths read as two different programs"
        );
        let lockup = crate::win32_draw::card_lockup();
        let l = layout(&card_text(&allowed_state()));
        assert_eq!(l.mark.h, lockup.mark_h);
        assert_eq!(l.wordmark.w, lockup.word_w);
    }

    #[test]
    fn the_wrap_estimate_counts_lines_and_never_loops() {
        assert_eq!(wrapped_lines("", 40), 0);
        assert_eq!(wrapped_lines("short", 40), 1);
        assert_eq!(wrapped_lines("aaaa bbbb cccc", 9), 2);
        // A word longer than the line takes a line of its own rather than
        // spinning.
        assert_eq!(wrapped_lines(&"x".repeat(200), 10), 1);
        assert_eq!(wrapped_lines("a b c", 0), 0);
        // The real message wraps to something the card is sized for.
        let lines = wrapped_lines(
            &card_text(&refused_state()).message,
            MESSAGE_CHARS_PER_LINE,
        );
        assert!(
            (1..=MESSAGE_LINES_MAX).contains(&lines),
            "the app's own refusal message wraps to {lines} lines, past the cap the card grows \
             for -- so it would be clipped on the surface whose whole job is to be read"
        );
    }

    // ---- labels ------------------------------------------------------------

    /// The window's title is this card's own, so `foreground::pick`'s `find`
    /// cannot bring one of the other six forward instead -- and, unlike the
    /// egui host it replaces, it is not the literal three raising windows
    /// share.
    #[test]
    fn the_card_opens_under_a_title_of_its_own() {
        assert!(!PREFLIGHT_CARD_TITLE.is_empty());
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::vault_window::WINDOW_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::prompt_card::PROMPT_CARD_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::locked_card::LOCKED_CARD_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::picker_prompt::PICKER_PROMPT_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::generate_prompt::GENERATE_PROMPT_TITLE);
        assert_ne!(PREFLIGHT_CARD_TITLE, crate::save_login_card::SAVE_LOGIN_CARD_TITLE);
    }

    /// The production seam is the real clipboard, pinned by ADDRESS.
    #[test]
    fn the_production_calls_are_the_real_ones() {
        assert!(
            std::ptr::fn_addr_eq(REAL.copy, crate::clipboard::copy_secret as fn(&str)),
            "the production card does not put *Copy instead* on the real clipboard"
        );
    }

    // ---- source pins -------------------------------------------------------

    /// The production half of this file: everything before the first column-0
    /// `#[cfg(test)]`, with line endings normalised first because this
    /// repository checks out CRLF.
    fn production() -> (String, usize) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("preflight_card.rs");
        let raw = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    /// The production half with comments stripped, so a rule that forbids a
    /// call does not also forbid explaining why the call is not there.
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_preflight_window_never_posts_a_thread_quit() {
        let (production, discarded) = production();
        let code = code(&production);

        // CONTROLS, so a pin that scanned nothing cannot pass.
        assert!(
            discarded > 0,
            "control: the `#[cfg(test)]` cut marker was not found, so this scan is reading the \
             test module as production and the rule below is meaningless"
        );
        assert!(
            code.contains("WM_DESTROY =>"),
            "control: the production cut does not contain the window procedure's WM_DESTROY \
             arm, so the cut is in the wrong place"
        );
        assert!(
            code.contains("GONE.store(true, Ordering::SeqCst);"),
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one \
             surviving statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "preflight_card.rs's production half posts a thread quit. This card is opened on \
             the thread that was about to fill, and that thread goes on to run more message \
             loops -- the vault window's among them. `close()` calls `DestroyWindow`, which \
             dispatches WM_DESTROY synchronously on that thread, and nothing drains the queue \
             afterwards. `GONE` is what `next()` reads; quitting the thread is not this \
             window's job."
        );
    }

    /// **The capture exclusion goes on the top-level window, and once.**
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SetWindowDisplayAffinity(").count(),
            1,
            "this card names the window a password is about to be typed into and excludes \
             itself from screen capture other than exactly once"
        );
        assert!(
            code.contains("SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE)"),
            "the exclusion is not applied to the top-level window this module was handed. \
             Windows refuses it on a child control with E_INVALIDARG, and the top-level flag \
             covers every child it owns"
        );
        assert_eq!(
            code.matches("SetForegroundWindow(").count(),
            1,
            "this card's send is a HELD KEY, so it asks for the foreground -- once, and \
             handled rather than asserted"
        );
    }

    /// **Nothing in the production half creates a GPU device**, which is the
    /// whole reason this card stopped being an egui window -- and this is the
    /// card whose port takes the daemon's fill path to zero GL contexts.
    #[test]
    fn the_card_is_gdi_only() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        for banned in ["D2D1CreateFactory", "D3D11CreateDevice", "ID2D1", "run_native"] {
            assert!(
                !code.contains(banned),
                "preflight_card.rs names `{banned}`. The whole reason this card is bare Win32 \
                 is that the first GPU device this process creates costs ~50 MB of driver \
                 arenas that are never released"
            );
        }
    }

    /// **`IsDialogMessageW` is in the pump**, and the three keys the card is
    /// answered with are read there.
    #[test]
    fn the_pump_traverses_and_reads_the_cards_own_keys() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("IsDialogMessageW(top, &msg)"),
            "the pump no longer runs `IsDialogMessageW`, so Tab cannot reach the two answers"
        );
        assert!(
            code.contains("VK_ESCAPE.0"),
            "Escape is no longer read in the pump, and the card's own left button says `Esc`"
        );
        assert!(
            code.contains("VK_SPACE.0"),
            "the hold key is no longer read in the pump, so the card cannot be sent from at all"
        );
    }

    /// **The space bar never reaches a control**, or a focused `BUTTON` reads
    /// the hold key as a click on itself -- and *Copy instead* would fire on a
    /// key the user is holding to send.
    #[test]
    fn the_hold_key_is_taken_out_of_the_queue_before_anything_can_see_it() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        let body = code
            .split("pub(super) fn next(")
            .nth(1)
            .expect("control: `next` is not in the production half");
        let (before, after) = body
            .split_once("IsDialogMessageW(")
            .expect("control: the pump's `IsDialogMessage` is inside `next`");
        assert!(
            before.contains("VK_SPACE.0"),
            "the hold key is read AFTER `IsDialogMessageW`, so a focused button sees it first"
        );
        assert!(
            before.contains("continue;"),
            "the hold key is no longer taken out of the queue -- it falls through to the \
             dispatch below"
        );
        assert!(
            !after.contains("VK_SPACE"),
            "control: the hold key is read once, before the dispatch, and not again after it"
        );
        // And the accumulation is gated on the verdict, not merely compared
        // against it: a refused card must have no frame on which a held key is
        // credited with anything.
        assert!(
            body.contains("if allowed && space_down && ours {"),
            "the hold is no longer gated on the verdict, the key being down AND the window \
             holding the foreground -- the card's own footnote promises the last of those"
        );
    }

    /// **No click on this card can send, and there is nothing to click.**
    ///
    /// The egui surface this replaces was held to that by clicking the centre
    /// of every rectangle it painted and asserting none of them sent. In GDI
    /// the property is structural instead, and stronger: the hold affordance
    /// is *painted*, not a `BUTTON` child, so no control exists under it to
    /// receive a click; and the only function that turns a control id into an
    /// event has two arms, neither of which is `Send`.
    #[test]
    fn no_click_on_this_card_can_send() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);

        let clicked = code
            .split("fn clicked(id: usize) {")
            .nth(1)
            .expect("control: `clicked` is not in the production half");
        let clicked = clicked.split("\n    }").next().unwrap_or(clicked);
        assert!(
            clicked.contains("ID_DISMISS => set_pending(Event::Cancel)"),
            "control: the click router's arms are not in the text being scanned: {clicked:?}"
        );
        assert!(
            !clicked.contains("Event::Send"),
            "a click on a control can send. The most dangerous action in the app must not be \
             reachable by a stray click on a window that just took focus: {clicked:?}"
        );

        // And the hold affordance is painted rather than created, so there is
        // no window under it at all. Two child controls, both of them footer
        // answers.
        assert_eq!(
            code.matches("for (id, at) in [(ID_DISMISS, l.dismiss), (ID_COPY, l.copy)]").count(),
            1,
            "the card creates its children somewhere other than the one loop that makes two \
             footer answers, so something else on it may now be clickable"
        );
        assert!(
            code.contains("fn paint_hold("),
            "the hold affordance is no longer painted by this card"
        );
    }

    /// **The brand lockup is drawn, and it is drawn through the crate's one
    /// mark painter.** Four cards lost it in porting and it had to be restored
    /// afterwards.
    #[test]
    fn the_card_carries_the_brand_lockup() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("draw_card_lockup("),
            "the card does not draw the brand lockup. A frameless always-on-top window that is \
             about to type a password has to say whose window it is"
        );
    }

    /// **The payload is taken by value and moved nowhere**, so every return
    /// from `run_with` -- the reentrancy refusal included -- drops it and wipes
    /// it, and the seam it reaches borrows rather than keeps it.
    #[test]
    fn the_payload_is_owned_here_and_borrowed_everywhere_else() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("copy_payload: Zeroizing<String>,"),
            "`run_with` no longer OWNS the payload, so there is no drop on its exit paths to \
             wipe it"
        );
        assert!(
            code.contains("pub copy: fn(&str),"),
            "the clipboard seam no longer borrows the payload. An owned `String` handed across \
             it is a copy this module cannot wipe"
        );
        assert!(
            code.contains("pub open: fn(&CardText) -> Option<PreflightWindow>,"),
            "`open` is handed something other than the already-masked `CardText`. The window \
             has nothing to draw the secret with and must be given no way to"
        );
        // And nothing logs it: every `log::` line in this file is checked to
        // name no payload binding.
        for line in code.lines().filter(|l| l.contains("log::")) {
            assert!(
                !line.contains("copy_payload"),
                "a log line names the payload: {line}"
            );
        }
    }

    // ---- the payoff --------------------------------------------------------

    /// **Every module a fill can reach between a hotkey and a keystroke.**
    ///
    /// Written out rather than derived, because the claim below is about *this
    /// path* and not about the crate: `login_ui`, `prefs_ui`, `vault_window`
    /// and `app_window` are egui windows, they stay egui windows, and a guard
    /// that could not tell them apart from the fill path would either be false
    /// or would have to be weakened until it said nothing.
    ///
    /// What is on it: the daemon's dispatcher; the seven cards a fill can put
    /// on screen; the gate and the injector that types; and the pure modules
    /// between them. What is deliberately **not**:
    ///
    /// * `foreground` -- the crate's window-classification guard. It names
    ///   every needle below in its own tables and prose, which is its job, and
    ///   it opens nothing.
    /// * `main` -- it launches the vault window, so it names `eframe`'s entry
    ///   point legitimately. What it does on the fill path is call into `app`,
    ///   which is on this list.
    /// * `vault_window/*` beyond `preflight` -- the vault window is egui and
    ///   stays egui.
    ///
    /// `vault_window/preflight.rs` **is** on it, and it is the interesting row:
    /// it still holds the egui `draw` the vault window uses, so this pins that
    /// the surviving egui half of 4b draws into somebody else's `Ui` and never
    /// opens a viewport of its own.
    const FILL_PATH: [(&str, &str); 21] = [
        ("app", include_str!("app.rs")),
        ("dispatch", include_str!("dispatch.rs")),
        ("app_candidates", include_str!("app_candidates.rs")),
        ("app_match", include_str!("app_match.rs")),
        ("match_engine", include_str!("match_engine.rs")),
        ("key_sequence", include_str!("key_sequence.rs")),
        ("clipboard", include_str!("clipboard.rs")),
        ("reprompt", include_str!("reprompt.rs")),
        ("fill_stats", include_str!("fill_stats.rs")),
        ("vault_window::preflight", include_str!("vault_window/preflight.rs")),
        ("injector", include_str!("injector/mod.rs")),
        ("injector::sequence", include_str!("injector/sequence.rs")),
        ("injector::send_input", include_str!("injector/send_input.rs")),
        ("injector::target", include_str!("injector/target.rs")),
        ("injector::ui_automation", include_str!("injector/ui_automation.rs")),
        ("unlock_prompt", include_str!("unlock_prompt.rs")),
        ("picker_prompt", include_str!("picker_prompt.rs")),
        ("generate_prompt", include_str!("generate_prompt.rs")),
        ("prompt_card", include_str!("prompt_card.rs")),
        ("locked_card", include_str!("locked_card.rs")),
        ("save_login_card", include_str!("save_login_card.rs")),
    ];

    /// Every way this crate has of putting an OpenGL context on the screen.
    ///
    /// `eframe::` itself is **not** one of them and must not be: every Win32
    /// card on the list above names `eframe::egui::Color32`, because `theme` is
    /// the one palette both renderers read. What costs the ~50 MB is starting a
    /// loop or a viewport, and these four are the only spellings of that in
    /// this crate.
    ///
    /// **Every one is spelled in two halves**, which is not decoration:
    /// `foreground::only_one_window_of_this_process_can_exist_at_a_time` counts
    /// the viewport needle over the RAW bytes of every window module -- test half
    /// included, deliberately, because a zero-count guard is stricter that way
    /// -- and this module is a window module. A needle written whole here would
    /// make this file look like it opened a second viewport. It is the same
    /// idiom `production()` uses on `#[cfg(test)]` for the same reason.
    const GL_ENTRY_POINTS: [&str; 4] = [
        concat!("run_", "native("),
        concat!("run_ui_", "native("),
        concat!("Viewport", "Builder"),
        concat!("show_", "viewport"),
    ];

    /// A module's production half, cut at its first column-0 `#[cfg(test)]`,
    /// with line endings normalised first because this repository checks out
    /// CRLF.
    fn production_half(source: &str) -> (String, usize) {
        let raw = source.replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    /// **THE POINT OF THE WHOLE EXERCISE: the daemon can complete a fill
    /// without ever creating a GL context.**
    ///
    /// The tray daemon measures 9.9 MB with no window ever opened. The first
    /// egui window takes it to ~60 MB **permanently** -- the OpenGL driver's
    /// committed arenas survive the window's destruction and are only reclaimed
    /// at process exit. Seven cards have moved to bare Win32 to make this claim
    /// true, and until the last of them landed the claim was not checkable,
    /// because one module on the path still called `eframe::run_native`.
    ///
    /// It is checkable now, so it is checked. **Nothing on [`FILL_PATH`]'s
    /// production half names any of [`GL_ENTRY_POINTS`]** -- so a fill that
    /// matched an app, offered a card, unlocked a vault, generated a password,
    /// confirmed a send and typed it touches no code that can start an
    /// `eframe` loop or a viewport.
    ///
    /// # What this can and cannot see
    ///
    /// It can see that no module on the path *names* the entry points. It
    /// cannot see a call reached through a third module not on the list, and it
    /// cannot measure resident memory -- that number is in the module doc and
    /// came from a profiler, not from a test. What makes the source pin worth
    /// having anyway is the failure it catches: the regression this guards
    /// against is somebody adding one egui window back to the fill path for one
    /// small surface, and that shows up here as a new name in a listed file.
    /// A module added to the path and not to this list escapes it, which is why
    /// the list is written out with its exclusions reasoned rather than derived
    /// from something that could quietly widen.
    #[test]
    fn the_daemons_fill_path_creates_no_gl_context() {
        // CONTROL on the needles themselves, through a module that really does
        // open an egui window and is deliberately NOT on the path. Without
        // this, a typo in `GL_ENTRY_POINTS` makes every assertion below vacuous
        // -- which is the exact shape of guard this crate keeps finding.
        let (login_ui, cut) = production_half(include_str!("login_ui.rs"));
        assert!(cut > 0, "control: no test module was cut out of `login_ui`");
        assert!(
            GL_ENTRY_POINTS.iter().any(|needle| login_ui.contains(needle)),
            "control: `login_ui` opens an egui window and its production half names none of \
             {GL_ENTRY_POINTS:?}, so these needles find nothing anywhere and the walk below \
             proves nothing"
        );

        let mut walked = 0usize;
        for (module, source) in FILL_PATH {
            let (production, discarded) = production_half(source);
            // CONTROL per row: a cut that found nothing would be reading a
            // module's test fixtures as its production half, and a zero over
            // an empty string is not an absence.
            assert!(
                discarded > 0,
                "control: no test module was cut out of `{module}`, so this row is scanning \
                 that file's fixtures as well as its code"
            );
            assert!(
                production.len() > 200,
                "control: `{module}`'s production half is {} bytes -- the cut is in the wrong \
                 place and the counts below are zero for the wrong reason",
                production.len()
            );
            // Comments stripped, for the reason every other pin in this file
            // strips them: this rule must not forbid a module from EXPLAINING
            // why it does not call `run_native`, and four modules on this list
            // do exactly that in their doc comments.
            let code = code(&production);
            for needle in GL_ENTRY_POINTS {
                assert!(
                    !code.contains(needle),
                    "`{module}` is on the daemon's fill path and its production half names \
                     `{needle}`. The tray daemon measures 9.9 MB with no window ever opened; \
                     the first egui window takes it to ~60 MB and NEVER RETURNS, because the \
                     OpenGL driver's committed arenas survive the window's destruction and are \
                     only reclaimed at process exit. Seven cards were redrawn in bare Win32 to \
                     make a whole fill possible without that, and this is the guard that says \
                     so. If a new surface on the fill path needs a window, it is a Win32 card \
                     -- there are seven to copy."
                );
            }
            walked += 1;
        }
        assert_eq!(
            walked,
            FILL_PATH.len(),
            "control: the walk did not reach every row of the fill-path list"
        );
        // And the card this test ships with is itself on the path -- through
        // its own `production()`, which reads the file rather than an
        // `include_str!` of it, so the two cannot disagree about which bytes
        // are being scanned.
        let (mine, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of this file");
        let mine = code(&mine);
        for needle in GL_ENTRY_POINTS {
            assert!(
                !mine.contains(needle),
                "preflight_card.rs -- the card that made this claim checkable -- names \
                 `{needle}`"
            );
        }
    }
}

//! A small spinner window shown while a background thread does slow,
//! non-interactive startup work (currently: waiting for `bw serve` to become
//! ready after login).
//!
//! Without this, the gap between the login window closing and the tray icon
//! appearing -- up to ~28s on a cold `bw serve` start -- showed nothing from
//! Deskwarden at all. Whatever else happened to be on screen (a terminal, in
//! more than one report) filled that silence, reading as "the app opened an
//! empty terminal" rather than "the app is still starting up".

use crate::login_ui::{
    draw_window_chrome_with_extra, round_window_corners, ChromeAction, ChromeMetrics, CloseControl,
};
use crate::theme;
use eframe::egui::{self, Margin};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// Named rather than inlined at the `run_ui_native` call, because
/// `foreground::raise_window` finds this window BY this title -- one
/// declaration means the two cannot drift apart.
const WINDOW_TITLE: &str = "Deskwarden";

/// Shows a "Deskwarden" window with a spinner and `message` until `rx`
/// yields a value, then closes and returns `Some(value)`.
///
/// `rx` is expected to be the receiving half of a channel whose sending half
/// was handed to a worker thread computing `T`. Both current call sites
/// (`main::wait_for_vault_ready_with_spinner` and
/// `picker_ui::pick_vault_item`) *detach* that worker with a bare
/// `std::thread::spawn`, giving it owned data (a `VaultBridge` clone, an
/// `Arc<VaultCache>` clone) rather than borrowing from the caller's stack.
/// That is deliberate: a `std::thread::scope`d worker -- which is what both
/// sites used to do -- forces the caller to block until the worker finishes
/// even after this function has already returned `None` because the user
/// closed the window, which made "the user can close this" a lie (review
/// 12). Detached, the worker simply finishes on its own and its result is
/// dropped unheard. `rx` itself is self-contained (an `mpsc::Receiver<T>`
/// borrows nothing), so it is fine to move into this window's own `'static`
/// closure regardless of where it was created.
///
/// Returns `None` if the window closed without `rx` ever yielding a value --
/// either the user closed it via the heading's ✕ or Alt+F4 (that ✕ is drawn
/// live -- `CloseControl::Active` -- because nothing about "loading" makes
/// THIS window modal or un-closable; the startup window's working stage is the
/// one that cannot be closed, and it says so by ghosting its own), or the
/// worker thread disconnected the channel without
/// sending (e.g. it panicked). Review 11's Critical: this used to
/// `.expect()` on exactly that case, which meant a user closing this spinner
/// while `pick_vault_item`'s populate ran panicked the main thread, and
/// `main.rs`'s panic hook only logs -- so the process unwound out of `main`
/// and the tray icon, hotkey, and autofill all vanished with it. Every
/// caller must now decide for itself what "the user closed this" means
/// (abandon quietly, treat as a failure, etc.) rather than that decision
/// being made for it by a crash.
/// The wait itself: **this app's window heading**, and under it design turn
/// 7's sliding bar and one line of prose, centred on a flat [`theme::CANVAS`]
/// panel that fills whatever is left.
///
/// The heading is the same [`draw_window_chrome_with_extra`] every other window
/// in this app draws, not a second titlebar -- there was no chrome here at all
/// until the user asked for it ("keep the same window heading as the rest of
/// the windows"), which made this the one screen in the app with no title, no
/// drag zone and no window controls.
///
/// Under it, the treatment is the vault window's OWN loading body
/// (`vault_window`'s `VaultBodyState::Loading`), which is the screen this one
/// hands over to: [`theme::progress_bar`] over one line of 13px prose. **Both
/// were a 28px rotating disc until design turn 7**, whose two waiting bodies
/// are drawn as a bar sliding in a track and which references the rotating
/// keyframe nowhere; the owner's report was that these screens still had the
/// "old design with round spinner". Matched rather than re-proportioned,
/// because the two are seen seconds apart in the same window and the second
/// must not look like a different app's idea of waiting -- which is also why
/// the widget itself lives in [`theme`] rather than being drawn twice here.
/// The mark that used to sit above the indicator is gone with it --
/// the heading already carries the wordmark, so drawing it a second time in the
/// middle of the screen was saying the app's name twice on a screen that has
/// one thing to say.
///
/// Returns what the heading asked for, which the host serves -- the two hosts
/// have different windows to serve it in. `close` is the host's answer to
/// whether its window may be closed AT ALL right now; see [`CloseControl`].
///
/// A function and not the inline body of [`show_while`] because there are two
/// places this look has to appear and they must not drift. `show_while` is one
/// -- a whole small window, its own `run_ui_native`, still used by the
/// cached-session launch and by `main`'s resettle. The other is the WORKING
/// STAGE of the single startup window (`app_window`), which is the same wait
/// with a window already around it: the user signs in and this replaces the
/// card in the SAME window rather than closing it and opening a second one,
/// which is the flicker the whole change exists to remove. Two hand-written
/// copies of "mark, spinner, label" would be two chances for the second window
/// the user is no longer supposed to notice to start looking like a different
/// app again.
///
/// Takes `&mut Ui` and paints only -- no channel, no viewport commands, no
/// window. That is also what makes it the one part of this feature a headless
/// `egui::Context` can actually render and read glyphs back from; everything
/// else here needs a live `eframe::Frame`, which no test can construct.
pub fn draw_spinner_body(ui: &mut egui::Ui, message: &str, close: CloseControl) -> ChromeAction {
    // `advance_cursor_after_rect` inside the chrome reads `item_spacing.y`
    // EAGERLY and bakes it into the cursor it leaves behind, so the gap between
    // the bar and the panel under it has to be zeroed before the call, not
    // after -- the same dance, for the same reason, as `vault_window`'s own
    // chrome call. Restored immediately, so this does not silently become the
    // ambient spacing of whatever the host draws next.
    let saved_item_spacing_y = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let action = draw_window_chrome_with_extra(ui, WINDOW_TITLE, HEADING, false, close, |_ui| {});
    ui.spacing_mut().item_spacing.y = saved_item_spacing_y;

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::CANVAS)
                .inner_margin(Margin::same(24)),
        )
        .show(ui, |ui| {
            // **Centred in the area BELOW THE HEADING**, rather than a fixed
            // offset from the top and no longer in the whole window either:
            // `available_height` here is what the panel was given, which the
            // chrome above has already taken its bar out of. This body is drawn
            // in two very differently sized windows -- the standalone spinner
            // window, small enough that a top offset once passed for centred,
            // and the single startup window, which is the vault window's full
            // size, where the same offset left everything huddled against the
            // top edge of an otherwise empty screen.
            let leftover = ui.available_height() - CONTENT_HEIGHT;
            ui.add_space((leftover / 2.0).max(0.0));
            ui.vertical_centered(|ui| {
                theme::progress_bar(ui, NARROW_BAR);
                ui.add_space(BAR_TO_LABEL);
                ui.label(theme::semibold(message, LABEL_SIZE).color(theme::TEXT_SECONDARY));
            });
        });
    action
}

/// The heading's metrics: the VAULT window's, not the login window's.
///
/// The startup window's working stage IS the vault window a second later -- the
/// spinner is replaced by the vault in the same window -- so a 46px bar that
/// becomes a 46px bar is a heading that does not move when the vault arrives.
/// `show_while`'s own small window uses it too, for the reason this body is
/// shared at all: two hosts, one look.
const HEADING: ChromeMetrics = ChromeMetrics::VAULT;

/// **The design's two track widths**, which differ by how much room the body
/// has rather than by which body it is: 260px in design 7a's full frame, 200px
/// in 7b's half-width card.
///
/// [`draw_first_window_body`] always draws in the vault window's own 1240px
/// frame, so it takes the wide one; [`draw_spinner_body`] is drawn in
/// [`show_while`]'s 360px window as well, where 260px would leave 26px of
/// margin either side, so it takes the narrow one. Neither is invented here --
/// they are the two the design already uses.
const WIDE_BAR: f32 = 260.0;
const NARROW_BAR: f32 = 200.0;

/// The gap between the bar and the text under it -- design 7a's own 22px, and
/// bigger than the 12px the disc had because the bar is 3px tall rather than
/// 28px and a tight gap under it reads as an underline on the heading.
const BAR_TO_LABEL: f32 = 22.0;

const LABEL_SIZE: f32 = 13.0;

/// What [`draw_spinner_body`]'s stack occupies, used to centre it.
///
/// Summed from the pieces above rather than written as one number, so it
/// cannot drift from them -- a hand-written total is the kind of constant
/// that stays put while the thing it describes changes underneath it. The
/// label's line box is its font size times egui's default line height for
/// this face; being a pixel or two out is invisible in a centring, whereas
/// the top-anchored version this replaces was out by hundreds.
const CONTENT_HEIGHT: f32 = theme::BAR_HEIGHT + BAR_TO_LABEL + LABEL_SIZE * 1.4;

/// This window GREW when the heading arrived: 320×150 had exactly enough room
/// for a mark, a 22px spinner and a line of text, and none at all for a 46px bar
/// above them -- at that size the stack overruns the panel's own bottom margin
/// and ends 19px off the window's edge, which is a cramped dialog rather than
/// the same screen the full-size window shows.
/// `the_standalone_window_has_room_for_the_heading_and_the_stack` is that
/// arithmetic asserted rather than eyeballed. Wider as well as taller, because
/// the bar has a minimum of its own: mark, wordmark and three 42px control zones
/// side by side.
const WINDOW_SIZE: [f32; 2] = [360.0, 220.0];

pub fn show_while<T: Send + 'static>(message: &str, rx: Receiver<T>) -> Option<T> {
    let result: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();
    let message = message.to_string();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            // Frameless, like every other window this app opens, because as of
            // the heading this body now draws there would otherwise be two
            // titlebars stacked on this one: the OS's and the app's.
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let mut styled = false;

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            // Frameless windows in this app ask DWM for the rounded corners and
            // shadow the OS frame would have given them. The OS window exists by
            // this first painted frame, which is the hook both this and the
            // raise below rely on.
            round_window_corners(WINDOW_TITLE);
            // This is where the window is brought to the front. See
            // `foreground`: a refusal from Windows flashes the taskbar button
            // rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if let Ok(value) = rx.try_recv() {
            *result_for_closure.borrow_mut() = Some(value);
        }

        // **This window's ✕ is live.** Nothing here holds a process the way the
        // startup window's working stage does; closing it returns `None` and
        // every caller already decides for itself what that means (see this
        // function's doc comment). With the frame gone, the chrome's ✕ and — are
        // the only mouse-operable way to close or minimise it.
        match draw_spinner_body(ui, &message, CloseControl::Active) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        if result_for_closure.borrow().is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // No user input drives this window, so without an explicit
            // repaint request it would sit static between rx polls instead
            // of animating the spinner / noticing the channel has a value.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
    });

    let value = result.borrow_mut().take();
    value
}

// ---------------------------------------------------------------------------
// DESIGN TURN 7 -- THE WINDOW BEFORE THE VAULT
// ---------------------------------------------------------------------------

/// **Which of the three bodies design 7 draws is on screen.**
///
/// The design's governing line is "same frame, same 1240x700 as the vault
/// window, so nothing jumps when the list arrives -- only the body changes".
/// This enum IS "only the body": one value, drawn by [`draw_first_window_body`]
/// into a window whose geometry is decided once, by the host, and never again.
///
/// It is deliberately not a superset of [`draw_spinner_body`]'s single message.
/// That body is a `&str` the caller composes, which is the right shape while
/// there is one thing to say; these three differ in structure -- one has a
/// live button, one has a number that ticks -- and a caller assembling them out
/// of strings would be a caller that can assemble a fourth one nothing has
/// designed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWindowBody {
    /// 7a. The ordinary wait.
    Loading,
    /// 7b left. The same wait, still going after [`SLOW_AFTER`], now saying so
    /// with the real number of seconds it has been -- see [`waiting_body`],
    /// which is where that number comes from.
    ///
    /// `local` is design 7b's *Open the local copy*: a wait that is dragging
    /// has somewhere else to go, when there is a copy to go to.
    Slow { seconds: u64, local: LocalCopy },
    /// 7b right. The probe answered with a failure, or spent its deadline.
    ///
    /// `local` is design 7b's *Continue offline*, and the line under the copy
    /// that says how old the local vault is.
    Unreachable { retry: RetryOffer, local: LocalCopy },
}

/// **What the encrypted copy on this machine can offer this screen** --
/// design 7b's *Open the local copy* and *Continue offline*.
///
/// Both bodies used to say there was no such thing, in as many words, and it
/// was true: there was no `vault_disk_cache` module to open a copy from.
/// There is one now, so the two buttons the design drew can be drawn -- but
/// only where they would really work, which is what these two values
/// separate.
///
/// **[`LocalCopy::None`] draws no button at all**, not a disabled one and not
/// a "coming soon". That is this crate's recorded decision, in
/// `prefs_ui::draw_not_yet`: all three of those treatments "look like a
/// feature that is present and broken". It covers every way there is nothing
/// to open -- the setting is off, no file was ever written, or the file was
/// rejected on its header and deleted before any key was derived.
///
/// `synced` is the copy's age. **`Option`, because a copy that has not been
/// opened has no age to report**: a session whose Hello prompt was cancelled
/// knows the file is on the disk and knows nothing about what is inside it,
/// including when it was written. A screen that filled that in -- with a
/// placeholder, or with "just written" -- would be inventing the one number
/// the user is being asked to trust. The button is still offered, because the
/// copy really is there and asking for the key again really can open it.
///
/// A [`Duration`] and not a `SystemTime`, so nothing on this screen depends on
/// what day it is: the host does the subtraction once, and this module renders
/// the same pixels forever for the same age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCopy {
    /// Nothing usable is on this machine. No button.
    None,
    /// A copy is here and can be opened. `synced` is its age, when this
    /// session has actually read the file and so knows one.
    Here { synced: Option<Duration> },
}

/// Whether the unreachable body still has a retry to offer.
///
/// **The bound is a state, not a counter the body reads.** The host counts
/// attempts; the body is told only whether pressing Retry is still a thing
/// that can happen, so the two ways this screen can end -- try again, or give
/// up and close -- are the two things it can draw and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOffer {
    /// There is an attempt left. The button is drawn and live.
    Offered,
    /// Every attempt has been spent. No button at all -- **not a disabled
    /// one**: `prefs_ui::draw_not_yet` records this crate's decision that a
    /// greyed control "looks like a feature that is present and broken". The
    /// copy says what is left instead, which is closing the window.
    Spent,
}

/// **How long the ordinary wait may run before the copy admits it is slow.**
///
/// Three seconds, which is the design's own number ("after three seconds the
/// copy says what is actually slow"). It is short enough that the second body
/// is what a user with a real problem actually sees -- a cold `bw serve` was
/// eight seconds in the reporter's own log, and the readiness deadline is
/// ~30s -- and long enough that an ordinary warm launch, which answers well
/// inside it, never flashes a "taking longer than usual" that was not true.
pub const SLOW_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// **The body for a wait that is still running**, chosen from how long it has
/// actually been running.
///
/// A pure function of the elapsed time and the only place the threshold is
/// applied, so "the slow body appears on a threshold rather than immediately"
/// is one testable rule rather than a comparison written into a frame closure
/// where nothing can reach it. The seconds it reports are the elapsed time
/// TRUNCATED, so the screen never claims a second that has not finished.
///
/// `local` rides along rather than being decided here: whether there is a
/// copy on this machine has nothing to do with how long this attempt has
/// been running, and the ordinary body offers nothing either way -- design 7a
/// draws no button, and a wait that is one second old has no business telling
/// the user to give up on it.
pub fn waiting_body(elapsed: Duration, local: LocalCopy) -> FirstWindowBody {
    if elapsed < SLOW_AFTER {
        FirstWindowBody::Loading
    } else {
        FirstWindowBody::Slow { seconds: elapsed.as_secs(), local }
    }
}

/// **What the footer says about the shortcut, given what is actually known.**
///
/// The design draws this line as "Autofill is already listening · Ctrl+⇧+F".
/// Two things in that are wrong for this app and both are corrected here.
///
/// The chord is `Ctrl+Alt+B` -- what [`crate::hotkey`] registers -- and a
/// window that teaches the user a chord this process never claims is a window
/// that has taught them nothing.
///
/// And "is already listening" is an assertion, made at the one moment in the
/// app's life when it is least likely to be true: `register_fill_hotkey` runs
/// after the startup window closes, so on the launch path
/// [`crate::hotkey::availability`] answers
/// [`crate::hotkey::Unavailable::NotYetAttempted`] -- nothing has tried. That
/// `Option`-backed "not yet attempted" exists precisely because a default that
/// is a well-formed answer is a claim nothing has established (see
/// `hotkey`'s own `STATUS`), and printing "already listening" over it would put
/// the claim back one screen earlier. So each state gets its own line, and the
/// only one that says the shortcut works is the one where it does.
pub fn hotkey_footnote(status: crate::hotkey::HotkeyStatus) -> &'static str {
    use crate::hotkey::{HotkeyStatus, Unavailable};
    match status {
        HotkeyStatus::Armed => "Autofill is listening · Ctrl+Alt+B",
        HotkeyStatus::Unavailable(Unavailable::NotYetAttempted) => {
            "Autofill starts when your vault opens · Ctrl+Alt+B"
        }
        HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram) => {
            "Another program is using Ctrl+Alt+B, so autofill has no shortcut"
        }
        HotkeyStatus::Unavailable(Unavailable::NoManager)
        | HotkeyStatus::Unavailable(Unavailable::Refused) => {
            "Windows refused Ctrl+Alt+B, so autofill has no shortcut"
        }
    }
}

/// The footer strip's two facts, as the caller knows them.
pub struct FirstWindowFooter<'a> {
    /// The signed-in address, or `None` when this window has not been told
    /// one.
    ///
    /// **Optional rather than a placeholder.** The design shows "Signed in as
    /// a.novak@ledgerline.com", and the recovery paths that host this window
    /// do not all have an address in scope. A window that has not been told
    /// simply does not draw the line, which is a smaller thing than drawing
    /// "Signed in as ..." over an address nobody supplied.
    pub account: Option<&'a str>,
    /// What [`crate::hotkey::availability`] answers, rendered by
    /// [`hotkey_footnote`].
    pub hotkey: crate::hotkey::HotkeyStatus,
}

/// What one frame of [`draw_first_window_body`] was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstWindowOutcome {
    /// Close / minimise, from the same chrome every window in this app draws.
    /// **Live in every body**, which is the requirement a spinner with no way
    /// out already failed once.
    pub chrome: ChromeAction,
    /// The unreachable body's Retry was pressed this frame.
    pub retry: bool,
    /// *Open the local copy* / *Continue offline* was pressed this frame.
    ///
    /// **One field for the two buttons**, because they are one request: stop
    /// waiting for Bitwarden and open what is on this machine. The two labels
    /// differ because the sentences around them do -- one interrupts a wait
    /// that could still succeed, the other answers a wait that failed -- but
    /// the host does the same thing for both, and a second field would be a
    /// second chance for the two paths to diverge.
    ///
    /// Never `true` when the body it came from was drawn with
    /// [`LocalCopy::None`]: there is no button on those frames to press.
    pub open_local_copy: bool,
}

/// The unreachable body's heading.
///
/// Design 7b's failure card also offers *Continue offline* and says "your
/// vault opened from the copy on this machine, last synced 2 h ago". Both
/// describe the encrypted vault disk cache, which **did not exist** when this
/// screen was first drawn -- it was a plan, with no `vault_disk_cache` module
/// to open a copy from -- so neither was drawn, not greyed and not as a
/// "coming soon": `prefs_ui::draw_not_yet` had already settled that all three
/// treatments "look like a feature that is present and broken".
///
/// The module exists now, and so does the button. What has not changed is the
/// rule: it is drawn only where it would really work, and where it would not
/// there is still nothing at all. See [`LocalCopy`], which is the whole of
/// that decision, and [`offline_line`], which is the "last synced" half.
const UNREACHABLE_TITLE: &str = "Couldn't reach Bitwarden";
const UNREACHABLE_OFFERED: &str =
    "Deskwarden can't open your vault until Bitwarden answers. Nothing has been lost — your \
     items are still in your account.";
const UNREACHABLE_SPENT: &str =
    "Bitwarden still isn't answering. Closing this window leaves your vault locked; nothing \
     has been lost, and opening Deskwarden again once you're back online picks up where this \
     left off.";

/// The label on the slow body's way out: design 7b's own *Open the local
/// copy*.
const OPEN_LOCAL_LABEL: &str = "Open the local copy";

/// The label on the unreachable body's: design 7b's own *Continue offline*.
///
/// A different word for the same act, because the sentence above it is
/// different -- one interrupts a wait that could still succeed, the other
/// answers a wait that has failed. Both report through
/// [`FirstWindowOutcome::open_local_copy`], which is the one field the host
/// acts on.
const CONTINUE_OFFLINE_LABEL: &str = "Continue offline";

/// **"last synced 2 h ago"** -- design 7b's line about the local copy, with
/// this app's own words for the age.
///
/// The age wording is [`crate::vault_window::cache_age_text`], which is what
/// the vault window's toolbar pill says over a restored vault ("Loaded from
/// cache · 3 h old"). One formatter and not two: the user meets both within
/// seconds of each other in the same window, and a second way of saying the
/// same age is a second chance for the two to disagree about it.
///
/// With no age -- the file is there but this session never opened it, so
/// there is no `written_at` to have read -- the line says what is actually
/// known, which is that a copy exists and that opening it needs the prompt
/// again. See [`LocalCopy`] on why that is not filled in with a placeholder.
fn offline_line(synced: Option<Duration>) -> String {
    match synced {
        Some(age) => format!(
            "Your copy on this machine · {}",
            crate::vault_window::cache_age_text(age)
        ),
        None => "There's a copy on this machine. Opening it asks Windows Hello again.".to_string(),
    }
}

/// The footer strip's height -- design 7a's own 40px bar.
const FOOTER_HEIGHT: f32 = 40.0;

/// The body panel's margin. The bottom one carries the footer as well as its
/// own space, so the stack above is centred in what is left over rather than
/// in an area the footer is painted across.
const BODY_MARGIN: i8 = 24;

/// Amber, from design 7b's own failure badge (`#fef6e7` on `#f2d99b`, mark in
/// `#8a5a06`). Module-local rather than in [`theme`], for the reason
/// `scratch_window` keeps its own two: these three appear on exactly one
/// screen, and a token in the shared palette is a claim that the rest of the
/// app has a warning treatment to match.
const WARN_FILL: egui::Color32 = egui::Color32::from_rgb(0xfe, 0xf6, 0xe7);
const WARN_EDGE: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xd9, 0x9b);
const WARN_INK: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x5a, 0x06);

const BADGE_SIZE: f32 = 38.0;

/// The unreachable body's badge-to-text gap -- design 7b's own 16px, and its
/// own constant rather than [`BAR_TO_LABEL`] because the two head very
/// different things: a 3px rail that needs air under it, and a 38px disc that
/// does not.
const BADGE_TO_LABEL: f32 = 16.0;

const TITLE_SIZE: f32 = 17.0;
const SUB_SIZE: f32 = 13.0;
const FOOT_SIZE: f32 = 12.0;
const TITLE_TO_SUB: f32 = 7.0;
const BLOCK_GAP: f32 = 18.0;

/// Between two stacked buttons -- tighter than [`BLOCK_GAP`], which separates
/// the copy from the controls under it. Retry and *Continue offline* are two
/// answers to one question and read as a pair; the design's own 18px between
/// them would read as two unrelated blocks.
const BUTTON_GAP: f32 = 10.0;

/// The measure the unreachable copy wraps into -- design 7b's own `40ch`, at
/// this app's 13px face.
const COPY_WIDTH: f32 = 380.0;

/// **One frame, three bodies** -- design turn 7.
///
/// Draws the app's usual heading, then whichever of the three bodies it was
/// given, then the footer strip. It takes no window and no channel and paints
/// only, exactly as [`draw_spinner_body`] does and for the same reason: it is
/// then the part of this feature a headless `egui::Context` can render and
/// read glyphs back from, and the part the preview example can put in a PNG.
///
/// The heading's metrics are [`HEADING`] -- the VAULT window's -- so a 46px bar
/// here is the 46px bar the item list arrives under. Nothing about the bar or
/// the footer changes between the three bodies: the whole design is that the
/// frame is decided once and only the middle of it moves.
pub fn draw_first_window_body(
    ui: &mut egui::Ui,
    body: FirstWindowBody,
    footer: FirstWindowFooter<'_>,
    close: CloseControl,
) -> FirstWindowOutcome {
    // The same eager-cursor dance `draw_spinner_body` does, for the same
    // reason: the chrome bakes `item_spacing.y` into the cursor it leaves.
    let saved_item_spacing_y = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let chrome = draw_window_chrome_with_extra(ui, WINDOW_TITLE, HEADING, false, close, |_ui| {});
    ui.spacing_mut().item_spacing.y = saved_item_spacing_y;

    let full = ui.max_rect();
    let mut retry = false;
    let mut open_local_copy = false;

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin {
            left: BODY_MARGIN,
            right: BODY_MARGIN,
            top: BODY_MARGIN,
            // The footer is painted over the bottom of this panel, so the
            // centring below has to be told the strip is there -- otherwise
            // the stack sits low and the last line of the unreachable copy
            // runs under it.
            bottom: BODY_MARGIN + FOOTER_HEIGHT as i8,
        }))
        .show(ui, |ui| {
            let leftover = ui.available_height() - content_height(body);
            ui.add_space((leftover / 2.0).max(0.0));
            ui.vertical_centered(|ui| match body {
                FirstWindowBody::Loading => {
                    theme::progress_bar(ui, WIDE_BAR);
                    ui.add_space(BAR_TO_LABEL);
                    ui.label(theme::semibold("Loading your vault", TITLE_SIZE).color(theme::INK));
                    ui.add_space(TITLE_TO_SUB);
                    ui.label(
                        theme::semibold("This stays on your machine", SUB_SIZE)
                            .color(theme::TEXT_FAINT),
                    );
                }
                FirstWindowBody::Slow { seconds, local } => {
                    theme::progress_bar(ui, WIDE_BAR);
                    ui.add_space(BAR_TO_LABEL);
                    ui.label(
                        theme::semibold("Still syncing with Bitwarden", TITLE_SIZE)
                            .color(theme::INK),
                    );
                    ui.add_space(TITLE_TO_SUB);
                    ui.label(theme::semibold(slow_line(seconds), SUB_SIZE).color(theme::TEXT_FAINT));
                    // **Secondary here and primary on the failure body**, and
                    // that is the whole difference: this wait can still
                    // succeed on its own, so leaving it is an option and not
                    // the thing to do. The bar above is still moving.
                    if let LocalCopy::Here { synced } = local {
                        ui.add_space(TITLE_TO_SUB);
                        ui.label(theme::semibold(offline_line(synced), SUB_SIZE).color(theme::TEXT_GHOST));
                        ui.add_space(BLOCK_GAP);
                        open_local_copy |= theme::secondary_button(ui, OPEN_LOCAL_LABEL).clicked();
                    }
                }
                FirstWindowBody::Unreachable { retry: offer, local } => {
                    draw_warning_badge(ui);
                    ui.add_space(BADGE_TO_LABEL);
                    ui.label(theme::semibold(UNREACHABLE_TITLE, TITLE_SIZE).color(theme::INK));
                    ui.add_space(TITLE_TO_SUB);
                    let copy = match offer {
                        RetryOffer::Offered => UNREACHABLE_OFFERED,
                        RetryOffer::Spent => UNREACHABLE_SPENT,
                    };
                    // Bounded, so the sentence wraps into the design's own
                    // measure rather than running the full width of a 1240px
                    // window as one line.
                    ui.allocate_ui_with_layout(
                        egui::vec2(COPY_WIDTH, 0.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(theme::semibold(copy, SUB_SIZE).color(theme::TEXT_FAINT));
                        },
                    );
                    if let LocalCopy::Here { synced } = local {
                        ui.add_space(TITLE_TO_SUB);
                        ui.label(
                            theme::semibold(offline_line(synced), SUB_SIZE)
                                .color(theme::TEXT_GHOST),
                        );
                    }
                    if offer == RetryOffer::Offered {
                        ui.add_space(BLOCK_GAP);
                        retry = theme::primary_button(ui, "Retry", None).clicked();
                    }
                    if let LocalCopy::Here { .. } = local {
                        // Stacked under the Retry when there is one -- a
                        // button-to-button gap; standing alone under the copy
                        // when there is not -- the same block gap the Retry
                        // would have had.
                        ui.add_space(match offer {
                            RetryOffer::Offered => BUTTON_GAP,
                            RetryOffer::Spent => BLOCK_GAP,
                        });
                        // **Primary exactly when it is the only thing left.**
                        // With a Retry still on offer, trying Bitwarden again
                        // is the better outcome and keeps the weight; once the
                        // attempts are spent, the alternative to this button
                        // is closing the window, and a lone secondary control
                        // reads as a footnote to a screen that has no main
                        // action left.
                        let pressed = match offer {
                            RetryOffer::Offered => {
                                theme::secondary_button(ui, CONTINUE_OFFLINE_LABEL).clicked()
                            }
                            RetryOffer::Spent => {
                                theme::primary_button(ui, CONTINUE_OFFLINE_LABEL, None).clicked()
                            }
                        };
                        open_local_copy |= pressed;
                    }
                }
            });
        });

    draw_footer(ui, full, &footer);

    FirstWindowOutcome { chrome, retry, open_local_copy }
}

/// "Taking longer than usual — 12 s", with a real number in it.
///
/// Separate and pure because the number is the part that must not be
/// decorative: a formatter with its own test, fed from a real `Instant`, is how
/// "the elapsed time is real" stops being a claim in a comment.
pub fn slow_line(seconds: u64) -> String {
    format!("Taking longer than usual — {seconds} s")
}

/// What the stack in the middle occupies, summed from its own pieces rather
/// than written as a total, for the reason [`CONTENT_HEIGHT`] gives.
fn content_height(body: FirstWindowBody) -> f32 {
    // Head and gap together, because the bar and the badge differ in both:
    // 3px over 22px of air, or 38px over 16px.
    let head = match body {
        FirstWindowBody::Loading | FirstWindowBody::Slow { .. } => theme::BAR_HEIGHT + BAR_TO_LABEL,
        FirstWindowBody::Unreachable { .. } => BADGE_SIZE + BADGE_TO_LABEL,
    };
    let mut height = head + TITLE_SIZE * 1.4 + TITLE_TO_SUB + SUB_SIZE * 1.4;
    // The line about the local copy, on whichever body is offering it. One
    // wrapped line either way -- both forms fit [`COPY_WIDTH`] at 13px.
    if matches!(
        body,
        FirstWindowBody::Slow { local: LocalCopy::Here { .. }, .. }
            | FirstWindowBody::Unreachable { local: LocalCopy::Here { .. }, .. }
    ) {
        height += TITLE_TO_SUB + SUB_SIZE * 1.4;
    }
    if let FirstWindowBody::Slow { local: LocalCopy::Here { .. }, .. } = body {
        height += BLOCK_GAP + theme::BUTTON_HEIGHT;
    }
    if let FirstWindowBody::Unreachable { retry, local } = body {
        // The copy is two or three wrapped lines rather than one.
        height += SUB_SIZE * 1.4 * 2.0;
        if retry == RetryOffer::Offered {
            height += BLOCK_GAP + theme::BUTTON_HEIGHT;
        }
        if local != LocalCopy::None {
            height += match retry {
                RetryOffer::Offered => BUTTON_GAP,
                RetryOffer::Spent => BLOCK_GAP,
            } + theme::BUTTON_HEIGHT;
        }
    }
    height
}

/// Design 7b's amber badge: a filled disc with a hairline edge and one mark.
fn draw_warning_badge(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(BADGE_SIZE, BADGE_SIZE), egui::Sense::hover());
    ui.painter().circle(
        rect.center(),
        BADGE_SIZE / 2.0,
        WARN_FILL,
        egui::Stroke::new(1.0, WARN_EDGE),
    );
    ui.put(rect, egui::Label::new(theme::bold("!", 18.0).color(WARN_INK)));
}

/// The strip along the bottom: who is signed in, and what the shortcut is
/// actually doing.
///
/// Painted after the panel rather than as a panel of its own, so it is one
/// rect at one height in `full` -- the window's whole rect, read once at the
/// top of the frame. That is what makes "the footer does not move when the
/// body does" a fact about the code and not a coincidence between three arms.
fn draw_footer(ui: &mut egui::Ui, full: egui::Rect, footer: &FirstWindowFooter<'_>) {
    let strip =
        egui::Rect::from_min_max(egui::pos2(full.min.x, full.max.y - FOOTER_HEIGHT), full.max);
    let painter = ui.painter().clone();
    painter.rect_filled(strip, egui::CornerRadius::ZERO, theme::CARD);
    painter.hline(
        strip.x_range(),
        strip.min.y,
        egui::Stroke::new(1.0, theme::HAIRLINE),
    );

    let inner = strip.shrink2(egui::vec2(16.0, 0.0));
    if let Some(account) = footer.account {
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left.label(
            theme::semibold(format!("Signed in as {account}"), FOOT_SIZE).color(theme::TEXT_FAINT),
        );
    }

    let mut right = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right.label(theme::semibold(hotkey_footnote(footer.hotkey), FOOT_SIZE).color(theme::TEXT_GHOST));
}

/// The one part of this window a headless `egui::Context` can reach.
///
/// [`show_while`] itself cannot be tested: it blocks on a real winit event
/// loop and opens a real OS window. [`draw_spinner_body`] only paints into a
/// `&mut Ui`, so a `Context::run_ui` frame renders it for real -- real glyphs
/// off real galleys, real shapes -- which is what makes "the two hosts show the
/// same thing" a claim with something behind it rather than a comment.
#[cfg(test)]
mod spinner_body_tests {
    use super::*;
    use eframe::egui::{Color32, Rect};

    /// A context with this app's fonts actually installed. `theme::apply` takes
    /// effect at the START of the next frame, so the warm-up frames are not
    /// optional -- without them `theme::semibold` looks up a family that does
    /// not exist yet and the frame panics.
    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    /// The standalone spinner window's real size, so what these tests render is
    /// what that window renders.
    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(WINDOW_SIZE[0], WINDOW_SIZE[1]),
            )),
            ..Default::default()
        }
    }

    fn frame(message: &str) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(raw_input(), |ui| {
            draw_spinner_body(ui, message, CloseControl::Active);
        })
    }

    /// Every character actually RENDERED, glyph by glyph off the galleys --
    /// not `Galley::text()`, which answers with the source string and would
    /// therefore report a message that was laid out into zero rows.
    fn rendered(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    for row in &text.galley.rows {
                        for glyph in &row.glyphs {
                            out.push(glyph.chr);
                        }
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn filled_rects(output: &egui::FullOutput) -> Vec<(Rect, Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn the_spinner_body_paints_the_message_it_was_given() {
        let painted = rendered(&frame("Setting up your vault..."));
        assert!(
            painted.contains("Setting up your vault..."),
            "the spinner drew no message, so the only thing on screen during a multi-second              wait is an unlabelled spinner: {painted:?}"
        );
        // Negative + positive control on the same renderer: a DIFFERENT
        // message really is absent, so the assertion above is about what was
        // passed in and not about a renderer that answers yes to anything.
        assert!(
            !painted.contains("Reconnecting"),
            "control: the glyph reader reports text that was never drawn: {painted:?}"
        );
        let other = rendered(&frame("Reconnecting"));
        assert!(
            other.contains("Reconnecting"),
            "control: the renderer cannot draw a second message at all: {other:?}"
        );
    }

    /// **The same window heading as the rest of the app.**
    ///
    /// The user's words: "keep the same window heading as the rest of the
    /// windows". This screen was the only one in the app that painted no chrome
    /// at all -- no title, no drag zone, no window controls -- and a glyph
    /// reader is exactly the instrument that says whether the heading's title is
    /// really being drawn, since the bar itself is just a filled rect that an
    /// empty titlebar would paint too.
    #[test]
    fn the_spinner_body_wears_the_window_heading() {
        let painted = rendered(&frame("Setting up your vault..."));
        assert!(
            painted.contains(WINDOW_TITLE),
            "the wait screen paints no window title, so it is the one screen in this app with \
             no heading -- and, being frameless, with nothing to drag or close it by: {painted:?}"
        );
        // Positive control: the same reader, on the same frame, finds the body's
        // own message -- so a missing title above is a missing title rather than
        // a frame that rendered no text at all.
        assert!(
            painted.contains("Setting up your vault..."),
            "control: nothing at all was rendered: {painted:?}"
        );
    }

    /// The flat fill is the half a glyph reader cannot see, and it is the half
    /// that makes the working stage read as the SAME window as the sign-in
    /// card behind it rather than as a bare grey dialog.
    #[test]
    fn the_spinner_body_fills_its_region_with_the_apps_canvas() {
        let output = frame("Setting up your vault...");
        let rects = filled_rects(&output);
        assert!(
            rects.iter().any(|(_, fill)| *fill == crate::theme::CANVAS),
            "nothing in the spinner is filled with `theme::CANVAS`, so the window behind the              spinner is whatever egui defaults to: {rects:?}"
        );
        // Positive control: the walker really found shapes, so the `any`
        // above is a search through something rather than through nothing.
        assert!(
            !rects.is_empty(),
            "control: no filled rectangles were painted at all"
        );
    }
}

#[cfg(test)]
mod spinner_centring_tests {
    use super::*;
    use eframe::egui::{Rect, pos2, vec2};

    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(input(600.0), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(600.0), |_ui| {});
        ctx
    }

    fn input(height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(840.0, height))),
            ..Default::default()
        }
    }

    /// The heading's height, written out rather than read from
    /// [`ChromeMetrics::VAULT`], so these tests state where they expect the
    /// stack to be instead of re-deriving it from the thing they are checking.
    /// If the design's bar height ever changes, the numbers below are meant to
    /// be re-decided, not to follow along quietly.
    const BAR: f32 = 46.0;

    /// The vertical span of everything painted BELOW THE HEADING -- text glyphs
    /// and the spinner's own shapes alike, so this measures the whole stack
    /// rather than whichever piece happens to be a rect.
    ///
    /// Two exclusions, and they are different:
    /// * anything starting at or above the bar's bottom edge is the heading
    ///   itself (its background, its hairline, its title, its ✕/▢/— glyphs),
    ///   which is not what "centred" is about here;
    /// * anything taller than 100px is a background fill -- the window body, the
    ///   canvas panel -- which would swamp the measurement. A height bound, not
    ///   a "smaller than the window" bound: the canvas panel in a SHORT window is
    ///   smaller than the window and still fills all of it.
    fn painted_span_below_the_heading(output: &egui::FullOutput) -> Option<(f32, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                other => {
                    let r = other.visual_bounding_rect();
                    if r.is_finite() && r.height() > 0.0 && r.height() < 100.0 && r.min.y >= BAR {
                        out.push(r);
                    }
                }
            }
        }
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut rects);
        }
        let top = rects.iter().map(|r| r.min.y).fold(f32::INFINITY, f32::min);
        let bottom = rects.iter().map(|r| r.max.y).fold(f32::NEG_INFINITY, f32::max);
        (top.is_finite() && bottom.is_finite()).then_some((top, bottom))
    }

    fn spinner_frame(height: f32) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(input(height), |ui| {
            draw_spinner_body(ui, "Setting up your vault...", CloseControl::Active);
        })
    }

    /// **The stack sits in the middle of the area below the heading.**
    ///
    /// It used to start at a fixed 18px from the top, which looked centred in
    /// the small standalone spinner window and left everything huddled
    /// against the top edge of the full-size startup window -- which is what
    /// the user saw and called "bad looking". A test that only asserts the
    /// message is painted cannot tell the two apart; this asserts where.
    ///
    /// What "centred" means changed when the heading arrived: the middle of the
    /// window would sit the stack `BAR / 2` too high, with more empty space
    /// under it than over it. The expected centre is written out below rather
    /// than derived from `CONTENT_HEIGHT` or the metrics -- a test that computes
    /// its expectation from the constants under test agrees with itself no
    /// matter what they say.
    #[test]
    fn the_spinner_stack_is_centred_below_the_heading_in_a_tall_window() {
        let tall = 600.0;
        let output = spinner_frame(tall);

        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let centre = (top + bottom) / 2.0;

        // (46 + 600) / 2. Generous: this is about "middle" versus "pinned to an
        // edge", a difference of hundreds of pixels. The layout before the
        // centring fix put the stack's centre near 75px in a 600px window.
        assert!(
            (centre - 323.0).abs() < 60.0,
            "the spinner stack is centred at y={centre:.1} in a {tall:.0}px window, not near 323 \
             -- it is anchored to an edge rather than centred in the area below the heading"
        );
    }

    /// **No logo above the indicator.** The user: "no need for logo in the
    /// middle of the screen - just bigger spinner".
    ///
    /// Measured rather than asserted about the source, because "the mark is
    /// gone" is a claim about the screen. Design 7's 3px bar over 22px of air
    /// over one 13px line is about 43px tall; the mark that used to sit above
    /// it was 32px with a 14px gap under it, so restoring it takes the stack
    /// past 90. The heading's own wordmark is excluded by
    /// `painted_span_below_the_heading`, which is the point -- the app's name
    /// is drawn once, up there.
    #[test]
    fn the_stack_is_an_indicator_and_a_line_of_text_and_not_a_logo_as_well() {
        let output = spinner_frame(600.0);
        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let height = bottom - top;
        assert!(
            height < 80.0,
            "the stack under the heading is {height:.1}px tall, which is more than a spinner and \
             one line of prose -- the mark is being drawn in the middle of the screen again"
        );
        // Positive control: it is not empty either, which is the other way a
        // height bound can be satisfied by accident.
        assert!(
            height > 20.0,
            "control: the stack under the heading is only {height:.1}px tall, so the bound above \
             is satisfied by nothing being drawn"
        );
    }

    /// **The standalone window is big enough for both.**
    ///
    /// The one test here that renders the REAL [`WINDOW_SIZE`] rather than a
    /// chosen height, because that constant is the decision this makes: the
    /// window it describes used to be 320×150, which fit a spinner and a line of
    /// text and nothing above them. 40px is "clearly more than the panel's own
    /// 24px margin"; at the old size the stack overruns that margin and ends
    /// 19px off the bottom edge, which is the stack wedged against it.
    #[test]
    fn the_standalone_window_has_room_for_the_heading_and_the_stack() {
        let height = WINDOW_SIZE[1];
        let output = spinner_frame(height);
        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");

        let above = top - BAR;
        let below = height - bottom;
        assert!(
            above >= 40.0 && below >= 40.0,
            "in the {height:.0}px standalone window the stack has {above:.1}px of clearance \
             under the heading and {below:.1}px above the bottom edge -- the window is too \
             short for a heading and a spinner both, so the two are jammed together"
        );
    }

    /// The control for the centring test, and the arithmetic behind
    /// [`WINDOW_SIZE`]: in a SHORT window the same body still paints, still near
    /// the middle, and -- newly load-bearing -- still FITS between the heading
    /// and the bottom edge. The previous centring bug was invisible at small
    /// sizes and only showed at full size, so the short case stays; the fit is
    /// the opposite risk, and it is the one the heading introduced.
    #[test]
    fn the_stack_fits_below_the_heading_in_the_small_window() {
        let short = 180.0;
        let output = spinner_frame(short);

        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let centre = (top + bottom) / 2.0;
        // (46 + 180) / 2.
        assert!(
            (centre - 113.0).abs() < 30.0,
            "the spinner stack is centred at y={centre:.1} in a {short:.0}px window, not near 113"
        );
        // Deliberately not an assertion that `top >= BAR` -- the span filter
        // already drops everything above the bar, so that would be a test of the
        // filter. The bottom edge is the one nothing has already guaranteed.
        assert!(
            bottom <= short,
            "the stack ends at y={bottom:.1} in a {short:.0}px window -- the heading pushed it \
             off the bottom edge, which is what a window too short for both looks like"
        );
    }
}

/// **Design turn 7, rendered for real.**
///
/// [`draw_first_window_body`] paints into a `&mut Ui` and nothing else, so a
/// headless `Context::run_ui` frame draws it exactly as the recovery window
/// does -- real glyphs off real galleys, real rects. Every claim below is
/// therefore a statement about what the user sees, which is the only kind this
/// screen's requirements can be settled by: "nothing jumps" and "the close
/// control is live in every body" are both facts about pixels.
#[cfg(test)]
mod first_window_body_tests {
    use super::*;
    use crate::hotkey::{HotkeyStatus, Unavailable};
    use eframe::egui::Rect;

    /// **The vault window's own size**, which is the whole premise: what these
    /// tests render is what arrives one frame before the item list does.
    const VAULT_SIZE: [f32; 2] = [1240.0, 700.0];

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(VAULT_SIZE[0], VAULT_SIZE[1]),
            )),
            ..Default::default()
        }
    }

    /// Fonts actually installed -- `theme::apply` lands at the start of the
    /// NEXT frame, so the warm-ups are not optional.
    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    fn frame_with(body: FirstWindowBody, account: Option<&'static str>) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(raw_input(), |ui| {
            draw_first_window_body(
                ui,
                body,
                FirstWindowFooter {
                    account,
                    hotkey: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
                },
                CloseControl::Active,
            );
        })
    }

    fn frame(body: FirstWindowBody) -> egui::FullOutput {
        frame_with(body, Some("a.novak@ledgerline.com"))
    }

    /// Every character actually RENDERED, glyph by glyph off the galleys --
    /// not `Galley::text()`, which answers with the source string and would
    /// report copy that was laid out into zero rows.
    fn rendered(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    for row in &text.galley.rows {
                        for glyph in &row.glyphs {
                            out.push(glyph.chr);
                        }
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Every filled rect, rounded to whole pixels so two frames can be
    /// compared without float noise deciding the answer.
    fn rects(output: &egui::FullOutput) -> Vec<(i32, i32, i32, i32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(i32, i32, i32, i32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((
                    r.rect.min.x.round() as i32,
                    r.rect.min.y.round() as i32,
                    r.rect.max.x.round() as i32,
                    r.rect.max.y.round() as i32,
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Three hours, as an elapsed span. **Never a date**: the age these
    /// bodies render is a `Duration` the host has already worked out, so a
    /// fixture cannot rot into a different picture next year.
    const THREE_HOURS: Duration = Duration::from_secs(3 * 3600);

    /// A copy on this machine whose age is known.
    const COPY_HERE: LocalCopy = LocalCopy::Here { synced: Some(THREE_HOURS) };

    /// The bodies this window can show, in the order it can show them --
    /// **each with and without a local copy**, because the offline
    /// affordances change the height of the stack and the whole-frame
    /// assertions below are about a stack that fits.
    fn all_bodies() -> [FirstWindowBody; 8] {
        [
            FirstWindowBody::Loading,
            FirstWindowBody::Slow { seconds: 12, local: LocalCopy::None },
            FirstWindowBody::Slow { seconds: 12, local: COPY_HERE },
            FirstWindowBody::Unreachable {
                retry: RetryOffer::Offered,
                local: LocalCopy::None,
            },
            FirstWindowBody::Unreachable { retry: RetryOffer::Offered, local: COPY_HERE },
            FirstWindowBody::Unreachable { retry: RetryOffer::Spent, local: LocalCopy::None },
            FirstWindowBody::Unreachable { retry: RetryOffer::Spent, local: COPY_HERE },
            FirstWindowBody::Unreachable {
                retry: RetryOffer::Offered,
                local: LocalCopy::Here { synced: None },
            },
        ]
    }

    /// **The slow body is on a THRESHOLD, and the threshold is three seconds.**
    ///
    /// A body that said "taking longer than usual" from the first frame would
    /// be saying it about every launch, including the ones that answer in
    /// 300ms -- a window that lies in the ordinary case to be right in the
    /// rare one.
    #[test]
    fn the_slow_body_waits_for_the_threshold() {
        assert_eq!(
            waiting_body(Duration::ZERO, LocalCopy::None),
            FirstWindowBody::Loading,
            "the first frame already claims the wait is unusual"
        );
        assert_eq!(
            waiting_body(SLOW_AFTER - Duration::from_millis(1), LocalCopy::None),
            FirstWindowBody::Loading,
            "the threshold fires early"
        );
        assert_eq!(
            waiting_body(SLOW_AFTER, LocalCopy::None),
            FirstWindowBody::Slow { seconds: 3, local: LocalCopy::None },
            "the threshold never fires, so a wedged launch shows `Loading your vault` for the \
             whole readiness deadline and says nothing about why"
        );
        assert_eq!(SLOW_AFTER, Duration::from_secs(3));
    }

    /// **The threshold does not consult the disk, and the ordinary body
    /// offers nothing.**
    ///
    /// `waiting_body` carries `local` through untouched: a copy on this
    /// machine is not a reason to call a one-second wait slow, and a wait that
    /// has only just started must not already be telling the user to give up
    /// on it. Design 7a draws no button, whatever is on the disk.
    #[test]
    fn a_local_copy_does_not_move_the_threshold_and_7a_still_offers_nothing() {
        assert_eq!(
            waiting_body(Duration::ZERO, COPY_HERE),
            FirstWindowBody::Loading,
            "a copy on the disk made the ordinary wait skip straight to the slow body"
        );
        assert_eq!(
            waiting_body(SLOW_AFTER, COPY_HERE),
            FirstWindowBody::Slow { seconds: 3, local: COPY_HERE },
            "the slow body was handed a different local-copy answer than it was given"
        );
        let painted = rendered(&frame(FirstWindowBody::Loading));
        for absent in [OPEN_LOCAL_LABEL, CONTINUE_OFFLINE_LABEL] {
            assert!(
                !painted.contains(absent),
                "the ordinary loading body offers {absent:?} on its first frame: {painted:?}"
            );
        }
    }

    /// **The number is the elapsed time, not decoration.**
    ///
    /// The design draws "Taking longer than usual — 12 s". Twelve is the
    /// design's example; what the window must show is however long it has
    /// actually been, truncated, so it never claims a second that has not
    /// finished.
    #[test]
    fn the_seconds_shown_are_the_seconds_elapsed() {
        for (millis, want) in [(3_000u64, 3u64), (3_999, 3), (12_400, 12), (61_000, 61)] {
            assert_eq!(
                waiting_body(Duration::from_millis(millis), LocalCopy::None),
                FirstWindowBody::Slow { seconds: want, local: LocalCopy::None },
                "{millis}ms is reported as something other than {want} s"
            );
        }
        let painted =
            rendered(&frame(FirstWindowBody::Slow { seconds: 12, local: LocalCopy::None }));
        assert!(
            painted.contains("12 s"),
            "the slow body renders no elapsed time at all, so the one number on the screen \
             that tells the user this is progressing is missing: {painted:?}"
        );
    }

    /// **Nothing jumps.** The frame -- the heading bar and the footer strip --
    /// is identical in every body; only the middle changes.
    ///
    /// This is design turn 7's governing line asserted rather than described.
    /// A body that drew its own heading, or a footer that moved when the copy
    /// grew, would be the two-window flow again inside one window.
    #[test]
    fn only_the_body_changes_between_the_bodies() {
        let baseline = rects(&frame(FirstWindowBody::Loading));
        let bar = *baseline
            .iter()
            .find(|(_, top, _, bottom)| *top == 0 && *bottom == HEADING.bar_height as i32)
            .unwrap_or_else(|| {
                panic!("no heading bar of {}px at the top of the window", HEADING.bar_height)
            });
        let strip = *baseline
            .iter()
            .find(|(left, top, right, bottom)| {
                *left == 0
                    && *right == VAULT_SIZE[0] as i32
                    && *bottom == VAULT_SIZE[1] as i32
                    && *top == (VAULT_SIZE[1] - FOOTER_HEIGHT) as i32
            })
            .expect("no footer strip along the bottom of the window");

        for body in all_bodies() {
            let painted = rects(&frame(body));
            assert!(
                painted.contains(&bar),
                "{body:?} does not draw the heading bar at {bar:?}, so the bar moves or \
                 changes size when the body does -- which is the jump this design exists to \
                 remove"
            );
            assert!(
                painted.contains(&strip),
                "{body:?} does not draw the footer strip at {strip:?}, so the bottom of the \
                 window moves under the user between one body and the next"
            );
        }
    }

    /// **Close and minimise are live in every body**, including the one that
    /// can sit on an unreachable backend indefinitely.
    ///
    /// A spinner with no way out is what the merged-window work already fixed
    /// once; a failure screen with no way out would be worse, because there is
    /// nothing left for it to be waiting for.
    /// The chrome draws its ✕ and — as LINE SEGMENTS, not glyphs (see
    /// `login_ui`: "glyphs are drawn, not typed, so they can't fall through to
    /// a fallback font"), and a live control is stroked in
    /// [`theme::TEXT_FAINT`] where a ghosted one is [`theme::TEXT_GHOST`]. So
    /// counting live-stroked segments inside the heading bar is what
    /// "the way out is really there" means on this screen.
    fn live_control_segments(output: &egui::FullOutput) -> usize {
        fn walk(shape: &egui::Shape, out: &mut usize) {
            match shape {
                egui::Shape::LineSegment { points, stroke } => {
                    if stroke.color == theme::TEXT_FAINT
                        && points.iter().all(|p| p.y < HEADING.bar_height)
                    {
                        *out += 1;
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = 0;
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn every_body_can_be_left() {
        // Two crossing segments for the ✕, one for the —. The ▢ between them
        // is ghosted, which is the vault window's own treatment and not a
        // control this window is taking away.
        const LIVE_CONTROL_SEGMENTS: usize = 3;
        for body in all_bodies() {
            assert_eq!(
                live_control_segments(&frame(body)),
                LIVE_CONTROL_SEGMENTS,
                "{body:?} does not draw a live close and minimise in the heading bar, so the \
                 only way out of this window is Task Manager -- which is what the merged \
                 startup window already had to fix once"
            );
        }
        // Control on the needle: a GHOSTED close really does score differently,
        // so the count above is a statement about liveness and not just about
        // there being lines in the bar.
        let ctx = styled_ctx();
        let ghosted = ctx.run_ui(raw_input(), |ui| {
            draw_first_window_body(
                ui,
                FirstWindowBody::Loading,
                FirstWindowFooter {
                    account: None,
                    hotkey: HotkeyStatus::Armed,
                },
                CloseControl::Disabled,
            );
        });
        assert!(
            live_control_segments(&ghosted) < LIVE_CONTROL_SEGMENTS,
            "control: a ghosted close control counts the same as a live one, so the \
             assertions above measure nothing"
        );
    }

    /// **What the unreachable body offers with nothing on the disk, and what
    /// it must not pretend to.**
    ///
    /// Design 7b draws *Continue offline* and "last synced 2 h ago", and both
    /// describe a file. When there is no usable file -- the setting is off, or
    /// none was ever written, or one was rejected on its header and deleted --
    /// this screen offers Retry and **nothing else**: not a greyed *Continue
    /// offline*, which `prefs_ui::draw_not_yet` records as looking "like a
    /// feature that is present and broken", and not a "last synced" line over
    /// a copy that is not there.
    #[test]
    fn with_no_local_copy_the_unreachable_body_offers_only_retry() {
        let painted = rendered(&frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Offered,
            local: LocalCopy::None,
        }));
        assert!(
            painted.contains("reach Bitwarden"),
            "the failure is not named: {painted:?}"
        );
        assert!(
            painted.contains("Retry"),
            "the unreachable body has no Retry, which is the only thing this screen can \
             actually do about the failure it is reporting: {painted:?}"
        );
        for absent in [CONTINUE_OFFLINE_LABEL, OPEN_LOCAL_LABEL, "this machine"] {
            assert!(
                !painted.contains(absent),
                "the unreachable body says {absent:?} with no copy on this machine to open. \
                 Every one of these is a promise the app cannot keep: {painted:?}"
            );
        }
    }

    /// **The copy is offered, and its age is the vault window's own wording.**
    ///
    /// Design 7b's "last synced 2 h ago", drawn from the file's real age.
    /// The wording is `vault_window::cache_age_text`, which is what the
    /// toolbar pill says over the same file a moment later -- asserted here
    /// against the formatter rather than against a literal, so the two cannot
    /// be changed apart.
    #[test]
    fn a_local_copy_is_offered_with_the_age_the_vault_window_would_report() {
        let painted = rendered(&frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Offered,
            local: COPY_HERE,
        }));
        assert!(
            painted.contains(CONTINUE_OFFLINE_LABEL),
            "there is a readable copy on this machine and the failure body does not offer it, \
             which is the whole of what the disk cache bought this screen: {painted:?}"
        );
        assert!(
            painted.contains("Retry"),
            "offering the copy took away the Retry, so a user whose network came back has no \
             way to ask again: {painted:?}"
        );
        let age = crate::vault_window::cache_age_text(THREE_HOURS);
        assert!(
            painted.contains(&age),
            "the copy's age is not on screen as {age:?}, so the user is asked to open a vault \
             without being told how old it is: {painted:?}"
        );
    }

    /// **A copy whose age is unknown is still offered, and no age is
    /// invented.**
    ///
    /// The cancelled-Hello state: the file is there, this session never read
    /// its header, so there is no `written_at` to report. The button stays --
    /// pressing it asks for the key again -- and the line says what that
    /// costs instead of guessing a number.
    #[test]
    fn a_copy_with_no_known_age_is_offered_without_one_being_made_up() {
        let painted = rendered(&frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Offered,
            local: LocalCopy::Here { synced: None },
        }));
        assert!(
            painted.contains(CONTINUE_OFFLINE_LABEL),
            "a dismissed fingerprint prompt hid the local copy the user still has, which is \
             the accident this state exists for: {painted:?}"
        );
        assert!(
            painted.contains("Windows Hello"),
            "nothing says why this one needs a prompt: {painted:?}"
        );
        for invented in ["just written", "old", "ago"] {
            assert!(
                !painted.contains(invented),
                "an age was rendered as {invented:?} for a file whose header this session \
                 never read: {painted:?}"
            );
        }
    }

    /// **A spent retry draws no button at all**, not a dead one.
    #[test]
    fn a_spent_retry_leaves_no_button_behind() {
        let painted = rendered(&frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Spent,
            local: LocalCopy::None,
        }));
        assert!(
            !painted.contains("Retry"),
            "the button is still on screen after its attempts are spent -- a control that \
             looks live and does nothing: {painted:?}"
        );
        assert!(
            painted.contains("reach Bitwarden"),
            "and the screen still has to say what happened: {painted:?}"
        );
    }

    /// **With the retries spent and a copy on the disk, exactly one button is
    /// left.**
    ///
    /// The other half of the state above: "no button at all" was the honest
    /// answer while there was nothing else this screen could do, and it stops
    /// being the answer the moment there is. Retry is still gone -- its
    /// attempts really are spent -- and *Continue offline* is what is left.
    #[test]
    fn a_spent_retry_over_a_local_copy_leaves_the_copy() {
        let painted = rendered(&frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Spent,
            local: COPY_HERE,
        }));
        assert!(
            !painted.contains("Retry"),
            "a spent Retry came back because a copy was on the disk: {painted:?}"
        );
        assert!(
            painted.contains(CONTINUE_OFFLINE_LABEL),
            "every attempt is spent and there is a readable copy on this machine, and the \
             screen's only advice is to close the window: {painted:?}"
        );
    }

    /// **The slow body's own way out** -- design 7b's *Open the local copy*,
    /// under a bar that is still moving.
    ///
    /// Its own label and not *Continue offline*: this wait has not failed, so
    /// leaving it is a choice and not a consolation. Both report through the
    /// one [`FirstWindowOutcome::open_local_copy`] field.
    #[test]
    fn the_slow_body_offers_the_copy_only_when_there_is_one() {
        let with = rendered(&frame(FirstWindowBody::Slow { seconds: 12, local: COPY_HERE }));
        assert!(
            with.contains(OPEN_LOCAL_LABEL),
            "a wait that has been going twelve seconds over a readable local copy offers no \
             way into it: {with:?}"
        );
        assert!(
            with.contains("12 s"),
            "and it still has to say how long it has been: {with:?}"
        );
        let without =
            rendered(&frame(FirstWindowBody::Slow { seconds: 12, local: LocalCopy::None }));
        assert!(
            !without.contains(OPEN_LOCAL_LABEL),
            "the slow body offers a local copy that is not on this machine: {without:?}"
        );
    }

    /// **The footer never says the shortcut works unless it does.**
    ///
    /// The design's line is "Autofill is already listening · Ctrl+⇧+F", and
    /// both halves are wrong here: this app registers `Ctrl+Alt+B`, and on the
    /// launch path nothing has tried to register anything --
    /// `hotkey::availability` answers `NotYetAttempted`. A first window that
    /// asserts a working hotkey is the same defect class the `Option`-backed
    /// `hotkey::STATUS` was introduced to remove.
    #[test]
    fn the_footer_tells_the_truth_about_the_shortcut() {
        assert!(
            hotkey_footnote(HotkeyStatus::Armed).contains("is listening"),
            "the one state where the shortcut really works does not say so"
        );
        for status in [
            Unavailable::NotYetAttempted,
            Unavailable::TakenByAnotherProgram,
            Unavailable::NoManager,
            Unavailable::Refused,
        ] {
            let line = hotkey_footnote(HotkeyStatus::Unavailable(status));
            assert!(
                !line.contains("is listening"),
                "{status:?} claims autofill is listening: {line:?}"
            );
        }
        for status in [
            HotkeyStatus::Armed,
            HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
        ] {
            let line = hotkey_footnote(status);
            assert!(
                line.contains("Ctrl+Alt+B"),
                "{status:?} names a chord that is not the one this app registers: {line:?}"
            );
            assert!(
                !line.contains('\u{21e7}'),
                "{status:?} shows the design's own chord rather than this app's: {line:?}"
            );
        }
        // ...and it really reaches the screen, in every body.
        for body in all_bodies() {
            let painted = rendered(&frame(body));
            assert!(
                painted.contains("Ctrl+Alt+B"),
                "{body:?} draws no shortcut line at all: {painted:?}"
            );
        }
    }

    /// **No account, no line** -- rather than "Signed in as" over nothing.
    #[test]
    fn the_footer_names_an_account_only_when_it_was_given_one() {
        let named = rendered(&frame_with(FirstWindowBody::Loading, Some("a@b.example")));
        assert!(
            named.contains("Signed in as a@b.example"),
            "the address the caller supplied is not on screen: {named:?}"
        );
        let anonymous = rendered(&frame_with(FirstWindowBody::Loading, None));
        assert!(
            !anonymous.contains("Signed in"),
            "a window told no address still draws the label, which is a sentence about \
             nobody: {anonymous:?}"
        );
    }

    /// The loading body says what it is doing and where the work happens --
    /// design 7a, minus the item count, which this window does not know: the
    /// items are what it is still waiting for.
    #[test]
    fn the_loading_body_says_what_it_is_doing() {
        let painted = rendered(&frame(FirstWindowBody::Loading));
        assert!(
            painted.contains("Loading your vault"),
            "the loading body has no heading: {painted:?}"
        );
        assert!(
            painted.contains("This stays on your machine"),
            "the loading body drops the one reassurance the design gives it: {painted:?}"
        );
        assert!(
            !painted.contains("items"),
            "the loading body counts items it has not received yet: {painted:?}"
        );
    }
    /// **The indicator is design 7's sliding bar, and no longer a disc.**
    ///
    /// The owner's report was that these screens still had the "old design
    /// with round spinner". A test that only checked something blue was
    /// painted would pass for either, so this asserts the shape the design
    /// actually specifies: a `WIDE_BAR`-wide, `theme::BAR_HEIGHT`-tall track
    /// filled with the hairline grey, and inside it a knob of the design's
    /// 32%. `egui::Spinner` paints an arc -- a `Shape::Path` -- and no rect at
    /// all, so neither of these can be satisfied by the thing this replaces.
    ///
    /// Rendered a quarter of the way through the cycle rather than at t=0.
    /// Design 7's own first keyframe is `translateX(-100%)`, which puts the
    /// knob entirely outside the track, and `paint_progress_bar` clips it --
    /// so at t=0 there is honestly nothing blue to find. That is a fact about
    /// the animation and not a bug, and it is why the preview draws its still
    /// frames off the clock rather than at zero.
    #[test]
    fn the_waiting_bodies_draw_the_designs_bar_and_not_a_disc() {
        for body in [
            FirstWindowBody::Loading,
            FirstWindowBody::Slow { seconds: 12, local: LocalCopy::None },
        ] {
            let ctx = styled_ctx();
            let input = egui::RawInput {
                time: Some(f64::from(crate::theme::BAR_PERIOD) / 4.0),
                ..raw_input()
            };
            let output = ctx.run_ui(input, |ui| {
                draw_first_window_body(
                    ui,
                    body,
                    FirstWindowFooter {
                        account: None,
                        hotkey: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
                    },
                    CloseControl::Active,
                );
            });
            let filled = filled_rects(&output);
            let bar = |want_w: f32, fill: egui::Color32| {
                filled.iter().any(|(rect, colour): &(egui::Rect, egui::Color32)| {
                    *colour == fill
                        && (rect.width() - want_w).abs() < 0.6
                        && (rect.height() - crate::theme::BAR_HEIGHT).abs() < 0.6
                })
            };
            assert!(
                bar(WIDE_BAR, crate::theme::HAIRLINE),
                "{body:?} paints no {WIDE_BAR}x{}px hairline track, so the design's rail is not \
                 on screen: {filled:?}",
                crate::theme::BAR_HEIGHT
            );
            assert!(
                bar(WIDE_BAR * 0.32, crate::theme::BLUE),
                "{body:?} paints no blue knob a third of the track wide, so what is moving in \
                 the middle of this window is not design 7's bar: {filled:?}"
            );
        }
    }

    /// Control on the test above: the UNREACHABLE body is not a wait and must
    /// draw no bar at all. Without this, a `draw_first_window_body` that
    /// painted the bar unconditionally -- under the failure copy, where it
    /// would say the app is still trying when it has stopped -- would pass
    /// every assertion there.
    #[test]
    fn the_unreachable_body_draws_no_bar_because_nothing_is_running() {
        let output = frame(FirstWindowBody::Unreachable {
            retry: RetryOffer::Offered,
            local: LocalCopy::None,
        });
        let filled = filled_rects(&output);
        assert!(
            !filled.iter().any(|(rect, colour)| *colour == crate::theme::HAIRLINE
                && (rect.height() - crate::theme::BAR_HEIGHT).abs() < 0.6
                && rect.width() > 100.0),
            "the unreachable body draws a progress track under a message that says the app has \
             given up: {filled:?}"
        );
    }

    /// Every filled rect with its colour, unrounded -- the bar's own
    /// assertions are about a 3px height and a 32% width, which rounding to
    /// whole pixels would blur.
    fn filled_rects(output: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }
}

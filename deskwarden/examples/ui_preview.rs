//! Visual preview of the themed windows.
//!
//! Interactive:
//!
//! ```text
//! cargo run --example ui_preview            # the autofill overlay (design 2a)
//! cargo run --example ui_preview -- --login # the login/unlock window (design 3h)
//! ```
//!
//! The overlay closes on Enter/Esc/✕; the login preview just draws (its
//! Continue does nothing here -- no `bw` is spawned from a preview).
//!
//! Self-screenshotting (for reviewing the design implementation without a
//! human at the keyboard):
//!
//! ```text
//! cargo run --example ui_preview -- --screenshot          # the overlay
//! cargo run --example ui_preview -- --login --screenshot  # the login window
//! cargo run --example ui_preview -- --all                 # EVERY surface below
//! ```
//!
//! `--all` is what CI runs. It walks [`Surface`] in one process -- one
//! `run_native`, resized between surfaces -- and writes a PNG per surface into
//! `target/ui_preview/`. One process rather than one invocation per surface
//! because a window creation and an eframe startup each cost more than the
//! frames they exist to produce, and this job must not double the CI time.
//!
//! # Why this example is worth its length
//!
//! Unit coverage on this crate is around 92% and functional coverage was
//! zero. Three real defects in one day were invisible to 2500 tests -- card
//! editing silently gated off for a fortnight, a rehearsal window that worked
//! and looked wrong, and a text baseline displacement a person spotted in a
//! second. Four separate UI tests were found *structurally blind* to the thing
//! they appeared to check.
//!
//! This does not fix that, and it is deliberately not a golden-image gate:
//! pixel-diffing an anti-aliased egui surface reds on a font-rendering change
//! and gets switched off within a month. It puts every surface where a human
//! can look at it on every change, which is the cheapest thing that would have
//! caught any of the three.
//!
//! # Everything here is a fixture
//!
//! No surface below reads a real vault, touches the network, or spawns `bw`.
//! The items are `serde_json` literals in the wire shape `bw serve` returns --
//! the same shape the deserializer is tested against, so a fixture that stops
//! parsing is a fixture that stopped describing the real thing. The breach
//! cache is handed a check that panics if it is ever called (it is not:
//! `check_breaches` is off), and the preflight is handed a [`SendTarget`]
//! value rather than a real foreground window.
//!
//! Every surface renders the exact draw function the app ships, never a copy,
//! so what these PNGs show is what the real app shows.

use deskwarden::breach::BreachCache;
use deskwarden::hello::HelloState;
use deskwarden::injector::target::SendTarget;
use deskwarden::key_sequence::ResolveSource;
use deskwarden::login_ui::{self, BwStatus, LoginForm};
use deskwarden::vault_bridge::{Folder, VaultItem};
use deskwarden::vault_window::detail::{self, RevealState, TotpState};
use deskwarden::vault_window::detail_edit::{self, EditDraft};
use deskwarden::vault_window::item_list;
use deskwarden::vault_window::password_health;
use deskwarden::vault_window::sidebar::{self, SidebarFilter};
use deskwarden::vault_window::preflight::{self, PreflightState};
use deskwarden::vault_window::record_ui::{self, RecordDraft};
use deskwarden::vault_window::rehearsal;
use deskwarden::vault_window::totp_add::{self, TotpAdd};
use deskwarden::{app_identity::AppIdentityCache, overlay_ui, prefs_ui, scratch_window, theme};
use eframe::egui::{self, Margin};
use std::path::PathBuf;

/// The instant every time-dependent preview is drawn at.
///
/// A literal, not `SystemTime::now()`: the one-time code shot's two most
/// prominent numbers are a code and a countdown, and a screenshot that changes
/// every run is one no reviewer can diff against the last.
const PREVIEW_UNIX: u64 = 1_699_999_980;

/// Where the PNGs go: `$CARGO_TARGET_DIR` when the environment sets one,
/// and the historical relative `target` when it does not.
///
/// **Why the environment has to win.** A bare `target/` is resolved against
/// the process's working directory, which for `cargo run --example` is the
/// package root -- so in a normal checkout this example dropped nine PNGs
/// into `deskwarden/target`, the directory this project forbids writing to
/// because the user runs the shipped app out of it. Anyone who redirects the
/// build away from that directory, which is the whole point of setting
/// `CARGO_TARGET_DIR`, was redirecting everything except this.
///
/// **Why the fallback is unchanged.** CI sets no `CARGO_TARGET_DIR`, so it
/// still gets `target/ui_preview` and its nine-PNG path check in
/// `.github/workflows/ci.yml` keeps working without an edit. The
/// `create_dir_all` at the write site already handled a base that does not
/// exist yet, which is what makes an absolute out-of-tree base safe here.
fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("target"),
    }
}

/// One screenshotable surface.
///
/// The list is the point of this file: adding a window to the app and not
/// adding it here is how a surface goes unlooked-at for a year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The autofill overlay (design 2a).
    Overlay,
    /// The autofill overlay with NOTHING to offer (design 3a): a window that
    /// asks for a password and that the vault does not match.
    ///
    /// Its own surface rather than a variation of `Overlay`, because it is a
    /// different card -- no avatar, no row, no Enter chip -- drawn by a
    /// different function, and a state nobody renders is a state nobody looks
    /// at.
    OverlayNoMatch,
    OverlayLocked,
    /// The autofill overlay's save-a-new-login form (design 3c): four rows and
    /// three answers, reached from `OverlayNoMatch`'s *New login* button.
    ///
    /// **By far the tallest state the overlay has**, and the one this list
    /// most needs: a frameless, always-on-top window of a hardcoded height
    /// with no `ScrollArea` anywhere, so a row or a button past the bottom
    /// edge is unreachable, and the geometry tests can only say the card fits
    /// -- not whether it reads as a form somebody would fill in.
    OverlaySaveLogin,
    /// The autofill overlay's generator (design 3d) with a password in hand:
    /// the state the card is in for almost all of the time it is on screen.
    OverlayGenerate,
    /// Design 3d while the round-trip to `bw serve` is outstanding.
    ///
    /// Its own surface rather than a footnote to the one above, because it is
    /// a state a reviewer has to be able to LOOK at: it is what the user sees
    /// for as long as the vault takes to answer, all four of its controls are
    /// disabled in it, and one fixed-size window has to hold it as well as
    /// the other two.
    OverlayGenerateWorking,
    /// Design 3d after the round-trip failed -- the state the design does not
    /// draw at all.
    ///
    /// The one this list most needs of the three: it must still carry a live
    /// *New* control, or a failed generate is a card the user can do nothing
    /// with but Esc.
    OverlayGenerateFailed,
    /// The login window with a vault that exists and is locked.
    LoginUnlock,
    /// The login window with no account yet -- server dropdown and the Hello
    /// opt-in, self-hosted, which is the tallest state it has.
    LoginSignin,
    /// The vault window's read pane for an ordinary login.
    LoginDetail,
    /// The read pane for a **card**: brand mark, masked number, and the
    /// shared expiry/security-code line. The surface whose edit path was
    /// gated off for a fortnight without a test noticing.
    CardDetail,
    /// The same card **with its number revealed** -- the state a user puts the
    /// pane into in order to read the number against the card in their hand.
    ///
    /// A surface of its own because it is the only place the number's digit
    /// grouping can be seen at all: masked, the grouping is a shape made of
    /// dots, and a reviewer looking at `detail_card` cannot tell whether
    /// revealing keeps the groups or runs the digits together. It ran them
    /// together for a release -- "when Bank number masked it has spaces but
    /// when open -- doesn't" -- and no rendered surface showed it.
    CardDetailRevealed,
    /// The edit form with the discard confirmation over it.
    DiscardConfirm,
    /// The record composer -- the Send export form and its seed warning.
    RecordComposer,
    /// **Design 6c/6d**: the by-hand "add a one-time code" form, with a URI
    /// typed in so the confirmation -- the live code, its countdown, the
    /// masked secret and the spelled-out parameters -- is on screen, and
    /// against an item that ALREADY has a code so the replace warning is too.
    /// Those two are the whole surface; a shot of the empty form would show
    /// neither.
    TotpAddConfirm,
    /// **Design 6a's picker**, which is the door every other route to a
    /// one-time code is behind. Four rows in the design's order, the fourth
    /// drawn dead with its reason, and the privacy line pinned under them.
    /// Its own surface rather than a second state of the one above because
    /// nothing of it is on that shot: a confirmation card is what happens
    /// AFTER a route has been chosen.
    TotpAddPicker,
    /// The preflight, allowed: the rule's process is in front and the focused
    /// control is masked, so the hold-to-send is offered.
    PreflightAllowed,
    /// The preflight, refused: a password sequence aimed at the wrong process
    /// and an unmasked control, which is the state that must never grow a
    /// send button.
    PreflightRefused,
    /// The preferences window's Clipboard page, everything switched on --
    /// four live pills, the interval field, the always-on note and the reset
    /// button. The page 3e does not contain, so there is no drawing to
    /// compare it against and a screenshot is the only review there is.
    PrefsClipboard,
    /// The same page with the master switch OFF: three greyed pills and a
    /// greyed field, still present rather than hidden. Its own surface
    /// because "looks disabled" is precisely the claim a picture can check
    /// and a `contains` assertion cannot.
    PrefsClipboardOff,
    /// **The About page, which is now two facts and nothing that acts.**
    ///
    /// Its own surface, and a new one: About used to be inseparable from the
    /// update card below it, so every "About" picture in this directory was
    /// really a picture of the update flow with a version row on top. The
    /// card left for `Section::Updates`; this is what is left, and whether
    /// what is left reads as a finished identity page or as an emptied one is
    /// exactly the question only a picture answers.
    ///
    /// Drawn with a signed-in account from a fixture, because the version row
    /// alone would not show whether the second row balances the card.
    PrefsAbout,
    /// **The Updates page's flow card, checked and current.** The state the
    /// tray item this replaced could not express at all: that item was created
    /// as a disabled `MenuItem::new("Update available", ..)`, so "there is no
    /// update" was rendered as a permanent claim that there was one. That this
    /// picture exists, and reads "This is the latest release", is the review.
    ///
    /// Drawn with automatic checks OFF as well -- and since the switch is now
    /// the card directly above, this one picture carries the whole consent
    /// argument: the pill off, the button offered anyway, and the sentence
    /// saying which of the two the preference governs, all in one glance.
    /// That glance is the reason the page exists and it is not assertable.
    PrefsUpdatesNoUpdate,
    /// The same card with a release found: the version, the download button,
    /// and the release notes below. The notes fixture is deliberately longer
    /// than its region, so the PNG shows the region *scrolling* rather than
    /// growing -- the layout claim that matters, this window being unresizable
    /// and this crate having pushed a control out of reach before.
    PrefsUpdatesAvailable,
    /// **The same card with notes that FIT.** Its whole point is the absence
    /// of a scrollbar: the region reserves the bar's lane either way, so this
    /// picture beside `prefs_updates_available` is the review of "the
    /// cue appears only when something is actually clipped, and the card's
    /// right edge does not move when it does".
    PrefsUpdatesShortNotes,
    /// **The card for a user several releases behind**, whose notes are the
    /// union of every release they skipped, newest first, each under its own
    /// version heading. The case the panel was reading one release's worth of
    /// for, and the case where a scrollbar is legitimately wanted -- so this
    /// picture is the review of both halves at once.
    PrefsUpdatesManyReleases,
    /// **The preferences WINDOW, chrome included**, rather than its body.
    ///
    /// Every other prefs surface draws `draw_prefs_body` at the size the
    /// window gives it, which is the right frame for the pages -- and means
    /// the seam between the titlebar and the top of the nav rail appears in
    /// no picture at all. That seam is what was reported ("there is a gap
    /// between window title panel and left nav panel"), so it gets a surface.
    PrefsWindowChrome,
    /// Mid-download: the progress bar, the byte count, and the notes still
    /// readable underneath. Its own surface because "is the bar there and does
    /// the card still fit" is exactly what a picture answers and an assertion
    /// does not.
    PrefsUpdatesDownloading,
    /// A failed download, with the reason on the page and a retry beside it.
    /// The old flow's failure went to a tray tooltip, visible only to someone
    /// already hovering a 16px icon.
    PrefsUpdatesFailed,
    /// **The Breaches page, before anything has been asked for.** The consent
    /// pill, the scan button, the sentence saying the button ignores the
    /// pill, and an empty history that says so in words rather than being a
    /// blank panel.
    ///
    /// Its own surface because the whole point of the page is that those four
    /// things are readable in one glance -- which is a claim about a picture
    /// and not about the order of painted rects.
    PrefsBreachesIdle,
    /// **A scan in flight, with failures already counted.** The state this
    /// design is arranged around: a run that will end with forty failures
    /// must not look clean while it runs, and the progress line has to fit
    /// beside a disabled button that has not vanished.
    PrefsBreachesRunning,
    /// **A finished run that could not check most of what it asked about.**
    /// The failure count is the last thing the sentence says, because it
    /// qualifies everything before it, and the history row for such a run is
    /// painted in the error ink rather than as an ordinary result.
    PrefsBreachesFailed,
    /// **The history under the button**, several runs deep, each with its own
    /// local timestamp and outcome -- including one that failed, so the two
    /// inks are in one picture.
    PrefsBreachesHistory,
    /// **The Password health screen with breach findings on it**, All filter.
    ///
    /// The state the whole breach half exists for, and the one no rect
    /// assertion answers: whether four bands of findings in one column read
    /// as four groups or as one long run -- and whether the reused password
    /// that is ALSO breached is visibly the same item in two lists rather
    /// than looking like two problems.
    VaultHealthBreached,
    /// The same report under **Reused**, **Weak** and **Breached**. Three
    /// pictures rather than one, because the thing being reviewed is what a
    /// narrowed list looks like -- including a band that is the only thing on
    /// the page.
    VaultHealthReusedOnly,
    VaultHealthWeakOnly,
    VaultHealthBreachedOnly,
    /// **The Password health screen with a filter that has nothing in it.**
    /// The empty line is a result and not a blank pane, and this is where
    /// that is looked at.
    VaultHealthEmptyFilter,
    /// **The vault window's item list**, at the exact width the window gives
    /// it, with a card of every network this app can name in it.
    ///
    /// The surface this example was missing, and the one two of the list's
    /// three reported defects live on: how big a favicon looks inside its
    /// tile, and whether a card's network badge can be READ. Neither is a
    /// question a rect assertion answers -- `paint_tests` can say the image
    /// is inset by exactly `theme::AVATAR_ICON_INSET` and say nothing at all
    /// about whether the result looks like an icon in a tile or an icon
    /// adrift in a frame.
    ///
    /// Every brand in `card_brand::CARD_BRANDS` is on it, on purpose: the
    /// badges have to be tellable apart from each other, and one card in a
    /// picture cannot show that.
    VaultList,
    /// **The vault window's rail**, at the exact width the window gives it.
    ///
    /// The surface the rail's own grouping is decided on: whether the three
    /// kinds of row it holds -- cuts of the vault, folders, and the rail's two
    /// SCREENS -- read as three groups or as one long run with two lines
    /// through it. That is a question about how a column looks, and no
    /// assertion about the order of painted rects answers it.
    ///
    /// The screens sit below the folders now, so the shot has to reach the
    /// bottom of the rail: it is drawn at the shipped window's own height with
    /// a folder list short enough to leave them on screen.
    VaultRail,
    /// The vault window's **Password health** screen, in the same item-list
    /// column at the same width as [`Surface::VaultList`] -- so the two can
    /// be laid side by side and their tiles' left and right edges compared,
    /// which is the only way to see that one pane's rows line up with the
    /// other's.
    ///
    /// Carries a **pathologically long item name**, which is the other reason
    /// it is here: a finding row paints its name with `Painter::text`, which
    /// takes no width, and the name used to be drawn straight out past the
    /// tile's right edge and over the pane behind it.
    VaultHealth,
    /// **Design 4d's rehearsal window, finished.** The twelfth surface, and
    /// the one this example exists for: this window shipped as a raw Win32
    /// dialog with none of the app's theme, tokens or type, and no screenshot
    /// job ever looked at it. It is drawn through `scratch_window::draw` --
    /// the exact function the viewport paints -- with a transcript built by
    /// the real `rehearsal::transcript`, so what this PNG shows is what a user
    /// watching a rehearsal sees.
    Rehearsal,
    /// **The startup spinner, in the window it now lives in.**
    ///
    /// `loading_ui::draw_spinner_body` at the VAULT window's size, with a LIVE
    /// close control -- a state that did not exist on screen before the two
    /// startup windows were merged. At full size the spinner was only ever
    /// drawn with its close control ghosted (the sign-in launch, which may not
    /// be abandoned), and live it was only ever drawn in a 360x220 window of
    /// its own. Both halves of that are what a picture answers: whether one
    /// spinner and one line of prose look deliberate in a 1240x740 window
    /// rather than lost in it, and whether the control that closes it is
    /// visibly there.
    VaultSetupSpinner,
    /// **Design turn 7's first window, loading.** The body the recovery
    /// window opens on, at the vault window's own size, with a live close
    /// control and the footer strip under it.
    FirstWindowLoading,
    /// The same window three seconds later, saying what is slow and how long
    /// it has been.
    FirstWindowSlow,
    /// The same wait with an encrypted copy on this machine: design 7b's
    /// *Open the local copy*, under the line that says how old opening it
    /// would leave you.
    FirstWindowSlowLocalCopy,
    /// The failure, in the window rather than in a message box, with the
    /// Retry it can actually offer -- and **nothing else**, because this
    /// machine has no copy of the vault to continue offline from.
    FirstWindowUnreachable,
    /// The same failure with a copy on this machine: Retry keeps the weight,
    /// *Continue offline* sits under it as a secondary.
    FirstWindowUnreachableLocalCopy,
    /// The same failure with its retries spent: the button is GONE rather
    /// than greyed, which is the half of this state a picture is the only way
    /// to check.
    FirstWindowUnreachableSpent,
    /// Retries spent **and** a copy on this machine -- so there is exactly one
    /// button left, and it is the primary. The picture is how "the last thing
    /// on this screen is not a footnote" gets checked.
    FirstWindowUnreachableSpentLocalCopy,
    /// A copy is on this machine and this session dismissed the Hello prompt,
    /// so its age is unknown. The button is still offered -- the file really
    /// is there -- and the line under the copy says what pressing it costs
    /// instead of inventing a date.
    FirstWindowUnreachableDeclinedCopy,
    /// **The lock screen while the master password is with the server.**
    ///
    /// The owner named this one -- "Locking - same" -- and it was the
    /// only pre-vault surface with a rotating disc that no picture in this
    /// list showed: `LoginUnlock` draws the same card at rest, where the
    /// indicator does not exist. A state nobody renders is a state nobody
    /// looks at, which is this file's whole argument.
    LoginUnlockBusy,
    /// **One cycle of design 7's `dw-bar`, laid out as a filmstrip.**
    ///
    /// The one thing a PNG of this widget cannot show is the thing it is:
    /// a still frame of a sliding bar is a blue dash somewhere in a grey
    /// rail, and at the design's own opening keyframe -- `translateX(-100%)`
    /// -- it is an empty rail and nothing else. So this surface draws the
    /// SAME painter the live widget uses at six fixed phases across one
    /// period, which is a picture of the motion rather than a picture taken
    /// during it. Everything else here renders one state; this renders the
    /// only part of the design that is a state MACHINE over time.
    ProgressBarCycle,
}

/// The detail pane's exact width in the shipped vault window.
///
/// `WINDOW_SIZE[0] - SIDEBAR_WIDTH - LIST_WIDTH`, i.e. `1240 - 212 - 390`.
/// Spelled out rather than imported because those three are `pub(crate)` in
/// `vault_window::mod` and an example is a separate crate. If that window is
/// ever resized, this is the number that has to follow it.
const PANE_WIDTH: f32 = 1240.0 - 212.0 - 390.0;

/// The item list panel's exact width in the shipped vault window, i.e.
/// `vault_window::mod`'s `LIST_WIDTH`. Spelled out here for the reason
/// [`PANE_WIDTH`] is spelled out.
const LIST_WIDTH: f32 = 390.0;

/// The rail's exact width in the shipped vault window, i.e.
/// `vault_window::mod`'s `SIDEBAR_WIDTH`.
const SIDEBAR_WIDTH: f32 = 212.0;

/// Tall enough that no pane below scrolls, so a screenshot is the whole
/// surface rather than the top of it. The shipped window is 740 high.
const PANE_HEIGHT: f32 = 740.0;

/// The preferences window's body: its 1000x780 outer size less the 40pt
/// chrome bar `draw_window_chrome` paints above `draw_prefs_body`. Spelled out
/// rather than imported for the same reason [`PANE_WIDTH`] is -- an example is
/// a separate crate and `WINDOW_SIZE` is private to `prefs_ui`.
const PREFS_BODY_WIDTH: f32 = 1000.0;
const PREFS_BODY_HEIGHT: f32 = 780.0 - PREFS_CHROME_HEIGHT;
/// The titlebar's height, which is `ChromeMetrics::LOGIN`'s.
const PREFS_CHROME_HEIGHT: f32 = 40.0;

/// The detail pane's own frame, copied from the `CentralPanel` in
/// `vault_window::mod` that hosts it: `theme::CANVAS` and
/// `Margin::symmetric(20, 18)`. The margin is part of the width the pane's
/// contents get, so guessing it would put the layout back off by tens of
/// pixels -- see [`Surface::size`].
fn pane_frame() -> egui::Frame {
    egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18))
}

/// Every surface, in the order `--all` walks them.
const ALL: &[Surface] = &[
    Surface::Overlay,
    Surface::OverlayNoMatch,
    Surface::OverlayLocked,
    Surface::OverlaySaveLogin,
    Surface::OverlayGenerate,
    Surface::OverlayGenerateWorking,
    Surface::OverlayGenerateFailed,
    Surface::LoginUnlock,
    Surface::LoginSignin,
    Surface::LoginDetail,
    Surface::CardDetail,
    Surface::CardDetailRevealed,
    Surface::DiscardConfirm,
    Surface::RecordComposer,
    Surface::TotpAddConfirm,
    Surface::TotpAddPicker,
    Surface::PreflightAllowed,
    Surface::PreflightRefused,
    Surface::PrefsClipboard,
    Surface::PrefsClipboardOff,
    Surface::PrefsAbout,
    Surface::PrefsUpdatesNoUpdate,
    Surface::PrefsUpdatesAvailable,
    Surface::PrefsUpdatesShortNotes,
    Surface::PrefsUpdatesManyReleases,
    Surface::PrefsWindowChrome,
    Surface::PrefsUpdatesDownloading,
    Surface::PrefsUpdatesFailed,
    Surface::PrefsBreachesIdle,
    Surface::PrefsBreachesRunning,
    Surface::PrefsBreachesFailed,
    Surface::PrefsBreachesHistory,
    Surface::VaultList,
    Surface::VaultRail,
    Surface::VaultHealth,
    Surface::VaultHealthBreached,
    Surface::VaultHealthReusedOnly,
    Surface::VaultHealthWeakOnly,
    Surface::VaultHealthBreachedOnly,
    Surface::VaultHealthEmptyFilter,
    Surface::Rehearsal,
    Surface::VaultSetupSpinner,
    Surface::FirstWindowLoading,
    Surface::FirstWindowSlow,
    Surface::FirstWindowSlowLocalCopy,
    Surface::FirstWindowUnreachable,
    Surface::FirstWindowUnreachableLocalCopy,
    Surface::FirstWindowUnreachableSpent,
    Surface::FirstWindowUnreachableSpentLocalCopy,
    Surface::FirstWindowUnreachableDeclinedCopy,
    Surface::LoginUnlockBusy,
    Surface::ProgressBarCycle,
];

impl Surface {
    /// The PNG's file stem. Stable, because a human reviewing an artifact
    /// looks for the same name every time.
    fn stem(self) -> &'static str {
        match self {
            Surface::Overlay => "overlay",
            Surface::OverlayNoMatch => "overlay_no_match",
            Surface::OverlayLocked => "overlay_locked",
            Surface::OverlaySaveLogin => "overlay_save_login",
            Surface::OverlayGenerate => "overlay_generate",
            Surface::OverlayGenerateWorking => "overlay_generate_working",
            Surface::OverlayGenerateFailed => "overlay_generate_failed",
            Surface::LoginUnlock => "login_unlock",
            Surface::LoginSignin => "login_signin",
            Surface::LoginDetail => "detail_login",
            Surface::CardDetail => "detail_card",
            Surface::CardDetailRevealed => "detail_card_revealed",
            Surface::DiscardConfirm => "edit_discard_confirm",
            Surface::RecordComposer => "record_composer",
            Surface::TotpAddConfirm => "totp_add_confirm",
            Surface::TotpAddPicker => "totp_add_picker",
            Surface::PreflightAllowed => "preflight_allowed",
            Surface::PreflightRefused => "preflight_refused",
            Surface::PrefsClipboard => "prefs_clipboard",
            Surface::PrefsClipboardOff => "prefs_clipboard_off",
            // **Renamed from `prefs_about_*`, not merely re-pointed.** The
            // card these show moved to `Section::Updates`; a picture called
            // `prefs_about_downloading` that draws the Updates page is a file
            // name that lies about which page is under review, and the whole
            // value of this directory is that the name says what you are
            // looking at.
            Surface::PrefsAbout => "prefs_about",
            Surface::PrefsUpdatesNoUpdate => "prefs_updates_no_update",
            Surface::PrefsUpdatesAvailable => "prefs_updates_available",
            Surface::PrefsUpdatesShortNotes => "prefs_updates_short_notes",
            Surface::PrefsUpdatesManyReleases => "prefs_updates_many_releases",
            Surface::PrefsWindowChrome => "prefs_window_chrome",
            Surface::PrefsUpdatesDownloading => "prefs_updates_downloading",
            Surface::PrefsUpdatesFailed => "prefs_updates_failed",
            Surface::PrefsBreachesIdle => "prefs_breaches_idle",
            Surface::PrefsBreachesRunning => "prefs_breaches_running",
            Surface::PrefsBreachesFailed => "prefs_breaches_failed",
            Surface::PrefsBreachesHistory => "prefs_breaches_history",
            Surface::VaultList => "vault_item_list",
            Surface::VaultRail => "vault_rail",
            Surface::VaultHealth => "vault_password_health",
            Surface::VaultHealthBreached => "vault_password_health_breached",
            Surface::VaultHealthReusedOnly => "vault_password_health_reused",
            Surface::VaultHealthWeakOnly => "vault_password_health_weak",
            Surface::VaultHealthBreachedOnly => "vault_password_health_breached_only",
            Surface::VaultHealthEmptyFilter => "vault_password_health_empty_filter",
            Surface::Rehearsal => "rehearsal",
            Surface::VaultSetupSpinner => "vault_setup_spinner",
            Surface::FirstWindowLoading => "first_window_loading",
            Surface::FirstWindowSlow => "first_window_slow",
            Surface::FirstWindowSlowLocalCopy => "first_window_slow_local_copy",
            Surface::FirstWindowUnreachable => "first_window_unreachable",
            Surface::FirstWindowUnreachableLocalCopy => "first_window_unreachable_local_copy",
            Surface::FirstWindowUnreachableSpent => "first_window_unreachable_spent",
            Surface::FirstWindowUnreachableSpentLocalCopy => {
                "first_window_unreachable_spent_local_copy"
            }
            Surface::FirstWindowUnreachableDeclinedCopy => {
                "first_window_unreachable_declined_copy"
            }
            Surface::LoginUnlockBusy => "login_unlock_busy",
            Surface::ProgressBarCycle => "progress_bar_cycle",
        }
    }

    /// The viewport size this surface is drawn at.
    ///
    /// The two login states are *starting* sizes only: they size to content
    /// exactly as `run_login_flow` does, and the capture waits for that to
    /// settle. Everything else is the real window's size.
    ///
    /// The panes are drawn at [`PANE_WIDTH`] **to the pixel**, for a reason
    /// that is not cosmetic: the card's face lays out through
    /// `detail::card_face_line_fits`, which puts the expiry and the security
    /// code on one line or two depending on the width it is given. A preview
    /// rendered fifty pixels narrow shows a layout the app never produces, and
    /// a screenshot of a layout nobody ships is worse than no screenshot.
    fn size(self) -> egui::Vec2 {
        match self {
            Surface::Overlay | Surface::OverlayNoMatch | Surface::OverlayLocked => {
                egui::vec2(396.0, 164.0)
            }
            // Read off the module rather than written out: 3c is the one
            // overlay state that is NOT 164pt tall, and a preview rendered at
            // the wrong height is a picture of a layout nobody ships.
            Surface::OverlaySaveLogin => egui::vec2(
                overlay_ui::OVERLAY_WIDTH,
                overlay_ui::overlay_height(overlay_ui::SAVE_LOGIN_ROWS),
            ),
            // Read off the module for the same reason, and it is a THIRD
            // height: 3d is neither 164pt nor 3c's 264.
            Surface::OverlayGenerate
            | Surface::OverlayGenerateWorking
            | Surface::OverlayGenerateFailed => egui::vec2(
                overlay_ui::OVERLAY_WIDTH,
                overlay_ui::overlay_height(overlay_ui::GENERATE_ROWS),
            ),
            Surface::LoginUnlock | Surface::LoginSignin | Surface::LoginUnlockBusy => {
                egui::vec2(470.0, 588.0)
            }
            // Wide enough for the design's own 260px track with room
            // either side, and tall enough for six of them stacked with
            // their labels.
            Surface::ProgressBarCycle => egui::vec2(420.0, 420.0),
            Surface::LoginDetail
            | Surface::CardDetail
            | Surface::CardDetailRevealed
            | Surface::DiscardConfirm
            | Surface::RecordComposer
            | Surface::TotpAddConfirm
            | Surface::TotpAddPicker => egui::vec2(PANE_WIDTH, PANE_HEIGHT),
            Surface::PreflightAllowed | Surface::PreflightRefused => egui::vec2(
                deskwarden::preflight_host::PREFLIGHT_WIDTH,
                deskwarden::preflight_host::PREFLIGHT_HEIGHT,
            ),
            // The shipped window is 1000x780 with a 40px chrome bar on top;
            // this draws the BODY, which is what `draw_prefs_body` is, so the
            // page lays out against exactly the width it has in the app.
            Surface::PrefsClipboard
            | Surface::PrefsClipboardOff
            | Surface::PrefsAbout
            | Surface::PrefsUpdatesNoUpdate
            | Surface::PrefsUpdatesAvailable
            | Surface::PrefsUpdatesShortNotes
            | Surface::PrefsUpdatesManyReleases
            | Surface::PrefsUpdatesDownloading
            | Surface::PrefsUpdatesFailed
            | Surface::PrefsBreachesIdle
            | Surface::PrefsBreachesRunning
            | Surface::PrefsBreachesFailed
            | Surface::PrefsBreachesHistory => egui::vec2(PREFS_BODY_WIDTH, PREFS_BODY_HEIGHT),
            // The WHOLE window, chrome included -- the one prefs surface that
            // is not the body alone, because the seam it exists to show is
            // between the two.
            Surface::PrefsWindowChrome => {
                egui::vec2(PREFS_BODY_WIDTH, PREFS_BODY_HEIGHT + PREFS_CHROME_HEIGHT)
            }
            // The list panel's exact shipped width, spelled out for the same
            // reason [`PANE_WIDTH`] is: `vault_window::mod`'s `LIST_WIDTH` is
            // `pub(crate)` and an example is a separate crate. A list drawn
            // narrow elides its titles and drops its chips, which is a picture
            // of a layout nobody ships.
            Surface::VaultList => egui::vec2(LIST_WIDTH, PANE_HEIGHT),
            // `vault_window::mod`'s `SIDEBAR_WIDTH`, spelled out for the same
            // reason as the two above, and the shipped window's own height --
            // the rail is measured against the window's floor, so a preview
            // drawn taller would hide exactly the overflow it is here to show.
            Surface::VaultRail => egui::vec2(SIDEBAR_WIDTH, PANE_HEIGHT),
            // The same column the item list is drawn in, at the same height:
            // Password health replaces the list in place, so a preview at any
            // other width is a picture of a pane nobody ships -- and this
            // surface is entirely about what happens at the column's right
            // edge.
            Surface::VaultHealth
            | Surface::VaultHealthBreached
            | Surface::VaultHealthReusedOnly
            | Surface::VaultHealthWeakOnly
            | Surface::VaultHealthBreachedOnly
            | Surface::VaultHealthEmptyFilter => egui::vec2(LIST_WIDTH, PANE_HEIGHT),
            // The viewport's own inner size, read off the module that builds
            // it -- so a window resized in the app is a preview resized with
            // it, rather than a picture of a layout nobody ships.
            Surface::Rehearsal => {
                egui::vec2(scratch_window::SCRATCH_WIDTH, scratch_window::SCRATCH_HEIGHT)
            }
            // The VAULT window's own size, because that is the window this
            // spinner is drawn in now -- the launch that already has a session
            // opens the vault window and paints the spinner in it until the
            // item list is ready. Rendered at the 360x220 the standalone
            // spinner window used, this picture would answer a question about
            // a window nobody opens any more.
            Surface::VaultSetupSpinner => egui::vec2(1240.0, 740.0),
            // **The vault window's own size, for all four**, which is the
            // whole of design turn 7: the frame is decided once and only the
            // body changes. Rendered at four different sizes these pictures
            // could not answer the one question they exist for.
            Surface::FirstWindowLoading
            | Surface::FirstWindowSlow
            | Surface::FirstWindowUnreachable
            | Surface::FirstWindowSlowLocalCopy
            | Surface::FirstWindowUnreachableLocalCopy
            | Surface::FirstWindowUnreachableSpent
            | Surface::FirstWindowUnreachableSpentLocalCopy
            | Surface::FirstWindowUnreachableDeclinedCopy => egui::vec2(1240.0, 740.0),
        }
    }

    /// Whether this surface draws the login window's own titlebar.
    fn is_login_window(self) -> bool {
        matches!(
            self,
            Surface::LoginUnlock | Surface::LoginSignin | Surface::LoginUnlockBusy
        )
    }
}

fn main() -> eframe::Result {
    let arg = |name: &str| std::env::args().any(|a| a == name);
    let all = arg("--all");
    let screenshot = all || arg("--screenshot");
    let signin = arg("--signin");
    let login = signin || arg("--login");
    let list = arg("--list");
    let rail = arg("--rail");
    let health = arg("--health");

    // `--all` walks the whole list; otherwise the single surface the flags
    // name, exactly as this example has always behaved.
    let queue: Vec<Surface> = if all {
        ALL.to_vec()
    } else if signin {
        vec![Surface::LoginSignin]
    } else if login {
        vec![Surface::LoginUnlock]
    } else if list {
        vec![Surface::VaultList]
    } else if rail {
        vec![Surface::VaultRail]
    } else if health {
        vec![Surface::VaultHealth]
    } else {
        vec![Surface::Overlay]
    };
    let first = queue[0];

    // Transparent and undecorated for every surface: the overlay and the
    // preflight need it for their rounded corners, and the login window draws
    // its own titlebar. The panes are drawn on their own opaque CANVAS frame
    // below, so transparency costs them nothing.
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(first.size())
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_icon(theme::window_icon());
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    // Single-surface runs keep their historical file names, because notes and
    // plans elsewhere refer to them by path. `--all` gets a directory, so the
    // artifact upload is one glob and a reviewer sees the set together.
    //
    // The BASE is `target_dir()`, not a bare relative `target/`: this used to
    // write into whatever `./target` the shell happened to be standing over,
    // which in a normal checkout is `deskwarden/target` -- the one directory
    // this project forbids writing to, because the user runs the app out of
    // it. See `target_dir` for the fallback that keeps CI's path unchanged.
    let out: PathBuf = if all {
        target_dir().join("ui_preview")
    } else if signin {
        target_dir().join("ui_preview_signin.png")
    } else if login {
        target_dir().join("ui_preview_login.png")
    } else if list {
        target_dir().join("ui_preview_vault_item_list.png")
    } else if rail {
        target_dir().join("ui_preview_vault_rail.png")
    } else if health {
        target_dir().join("ui_preview_vault_password_health.png")
    } else {
        target_dir().join("ui_preview_overlay.png")
    };

    // Cloned before the closure takes it: the count check below outlives
    // the run.
    let out_dir = out.clone();
    let outcome = eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Preview {
                queue,
                at: 0,
                directory: all,
                out,
                form: LoginForm::default(),
                // The app name a real 3c card would have been pre-filled with,
                // and nothing else: the other three rows are what this app can
                // actually know about the window, which is nothing.
                save_login: overlay_ui::SaveLoginForm::new("Atlas Licence"),
                generate: [
                    {
                        let mut ready =
                            overlay_ui::GenerateForm::new(overlay_ui::GeneratedKind::Characters);
                        ready.finish(Ok(zeroize::Zeroizing::new(
                            "tq7Rvk29mzpLx4-hd8".to_string(),
                        )));
                        ready
                    },
                    overlay_ui::GenerateForm::new(overlay_ui::GeneratedKind::Characters),
                    {
                        let mut failed =
                            overlay_ui::GenerateForm::new(overlay_ui::GeneratedKind::Characters);
                        failed.finish(Err(overlay_ui::GENERATE_FAILED_TEXT.to_string()));
                        failed
                    },
                ],
                screenshot,
                done: false,
                frames: 0,
                icons: None,
                list_search: String::new(),
                // The first row, so the shot carries the selected treatment
                // (blue border, blue wash behind the tile) as well as the
                // ordinary one -- see `draw_vault_list`.
                list_selected: Some("list-0001".to_string()),
                list_visible: Vec::new(),
                // An ordinary item row selected, so the shot carries the
                // selected treatment and neither screen is up -- the state the
                // window is in almost all of the time.
                rail_selected: SidebarFilter::Logins,
                rail_sends: false,
                rail_health: false,
                // The weak finding, so the health shot carries the selected
                // treatment too -- and it is the row with a detail line
                // under its name, which is the taller of the row's two
                // layouts.
                health_selected: Some("health-weak".to_string()),
                window_height: 0.0,
                styled: false,
                fixtures: Fixtures::new(),
            }))
        }),
    );

    // **The run's own error comes first.** Counting before propagating it
    // turned a real eframe failure into "wrote 0 PNG(s)", which says nothing
    // about why -- measured on a CI runner, where this masked the actual
    // cause for a whole run.
    outcome?;

    // **The walk checks its own arithmetic, so CI does not have to.**
    //
    // The workflow used to assert a hardcoded PNG count. It said nine
    // while `ALL` held eleven, and the screenshots job was red over a
    // number nobody had to touch when a surface was added -- two
    // enumerations obliged to agree, which is the defect this crate keeps
    // losing to. The count now lives where the surfaces do.
    if all {
        let written = std::fs::read_dir(&out_dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            written,
            ALL.len(),
            "the preview walk wrote {} PNG(s) into {} for {} surface(s) -- a walk that stops part way leaves a perfectly valid, perfectly useless artifact",
            written,
            out_dir.display(),
            ALL.len()
        );
    }
    Ok(())
}

/// How many settled frames a surface gets before it is captured.
///
/// Not one: fonts go live a frame after `set_fonts`, a viewport resize lands
/// asynchronously, and the panes lay out against a width they only learn from
/// the frame they are drawn in. The counter is reset whenever a resize is
/// issued (see `draw_login` and `advance`), so this is "frames since the
/// geometry last moved" rather than "frames since the surface appeared".
const WARMUP_FRAMES: u32 = 12;

struct Preview {
    /// The surfaces to draw, and where in them we are.
    queue: Vec<Surface>,
    at: usize,
    /// Whether `out` names a directory (`--all`) or a single PNG.
    directory: bool,
    out: PathBuf,
    /// Form state for the login preview (typing works; Continue doesn't).
    form: LoginForm,
    /// Form state for design 3c (typing works; Save doesn't -- there is no
    /// vault behind this example).
    save_login: overlay_ui::SaveLoginForm,
    /// Design 3d in each of its three states, one form per state.
    ///
    /// **Three forms rather than one that is re-pointed**, because the card
    /// takes `&mut` and its own controls move it between states: a single
    /// form would show whichever state the last frame's clicks left it in,
    /// which is not the state the file name promises. Nothing here reaches a
    /// vault -- the passwords are fixtures, and `GenerateForm::finish` is the
    /// production way into both settled states.
    generate: [overlay_ui::GenerateForm; 3],
    /// Capture and exit, rather than sit there being looked at.
    screenshot: bool,
    /// Every surface has been captured and `Close` has been asked for. The
    /// frames eframe still draws after that must not try to capture a tenth.
    done: bool,
    /// Frames drawn since this surface's geometry last moved.
    frames: u32,
    /// The item-list shot's own state. Built on the first frame that needs it
    /// and then held: `load_texture` allocates a new texture on every call, so
    /// rebuilding the cache per frame would upload nine images a frame for the
    /// twelve warm-up frames.
    icons: Option<item_list::IconCache>,
    list_search: String,
    list_selected: Option<String>,
    list_visible: Vec<String>,
    /// The rail shot's own selection state.
    rail_selected: SidebarFilter,
    rail_sends: bool,
    rail_health: bool,
    /// The Password health shot's own selection state.
    health_selected: Option<String>,
    /// Last applied window height, for the login window's size-to-content.
    window_height: f32,
    /// Whether the theme has been applied yet. Done on the first update
    /// frame, not in the creation context, for the same reason as the real
    /// windows (see login_ui): eframe re-applies its own style after
    /// creation, and egui font sets go live a frame after `set_fonts`.
    styled: bool,
    fixtures: Fixtures,
}

impl Preview {
    fn current(&self) -> Surface {
        self.queue[self.at]
    }

    /// Where this surface's PNG goes.
    fn png_path(&self) -> PathBuf {
        if self.directory {
            self.out.join(format!("{}.png", self.current().stem()))
        } else {
            self.out.clone()
        }
    }

    /// Moves to the next surface, or reports that there is none.
    ///
    /// **`at` is not advanced past the last surface.** `Close` is a request,
    /// not a return: eframe draws at least one more frame after it, and an
    /// index one past the end made that frame panic on `current()` -- after
    /// every PNG had been written, so the artifacts were right and the job
    /// still failed.
    fn advance(&mut self, ctx: &egui::Context) -> bool {
        if self.at + 1 >= self.queue.len() {
            self.done = true;
            return false;
        }
        self.at += 1;
        self.frames = 0;
        self.window_height = 0.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.current().size()));
        if self.current().is_login_window() {
            login_ui::round_window_corners("Deskwarden preview");
        }
        true
    }
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        if !self.styled {
            theme::apply(&ctx);
            if self.current().is_login_window() {
                login_ui::round_window_corners("Deskwarden preview");
            }
            self.styled = true;
            ctx.request_repaint();
            return;
        }
        self.frames += 1;

        match self.current() {
            Surface::Overlay => self.draw_overlay(root, &ctx),
            Surface::OverlayNoMatch => self.draw_overlay_no_match(root, &ctx),
            Surface::OverlayLocked => self.draw_overlay_locked(root, &ctx),
            Surface::OverlaySaveLogin => self.draw_overlay_save_login(root, &ctx),
            Surface::OverlayGenerate
            | Surface::OverlayGenerateWorking
            | Surface::OverlayGenerateFailed => {
                self.draw_overlay_generate(root, &ctx, self.current())
            }
            Surface::LoginUnlock => self.draw_login(root, &ctx, false, false),
            Surface::LoginUnlockBusy => self.draw_login(root, &ctx, false, true),
            Surface::ProgressBarCycle => draw_progress_bar_cycle(root),
            Surface::LoginSignin => self.draw_login(root, &ctx, true, false),
            Surface::LoginDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Login)),
            Surface::CardDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Card)),
            Surface::CardDetailRevealed => {
                self.draw_pane(root, PaneKind::Detail(DetailShot::CardRevealed))
            }
            Surface::DiscardConfirm => self.draw_pane(root, PaneKind::Discard),
            Surface::RecordComposer => self.draw_pane(root, PaneKind::Composer),
            Surface::TotpAddConfirm => self.draw_pane(root, PaneKind::TotpAdd),
            Surface::TotpAddPicker => self.draw_pane(root, PaneKind::TotpPicker),
            Surface::PreflightAllowed => self.draw_pane(root, PaneKind::Preflight(true)),
            Surface::PreflightRefused => self.draw_pane(root, PaneKind::Preflight(false)),
            Surface::PrefsClipboard => self.draw_prefs(root, true),
            Surface::PrefsClipboardOff => self.draw_prefs(root, false),
            Surface::PrefsAbout => self.draw_prefs_about(root),
            Surface::PrefsUpdatesNoUpdate
            | Surface::PrefsUpdatesAvailable
            | Surface::PrefsUpdatesShortNotes
            | Surface::PrefsUpdatesManyReleases
            | Surface::PrefsUpdatesDownloading
            | Surface::PrefsUpdatesFailed => self.draw_prefs_updates(root, self.current()),
            Surface::PrefsBreachesIdle
            | Surface::PrefsBreachesRunning
            | Surface::PrefsBreachesFailed
            | Surface::PrefsBreachesHistory => self.draw_prefs_breaches(root, self.current()),
            Surface::PrefsWindowChrome => self.draw_prefs_window(root),
            Surface::VaultList => self.draw_vault_list(root),
            Surface::VaultRail => self.draw_vault_rail(root),
            Surface::VaultHealth
            | Surface::VaultHealthBreached
            | Surface::VaultHealthReusedOnly
            | Surface::VaultHealthWeakOnly
            | Surface::VaultHealthBreachedOnly
            | Surface::VaultHealthEmptyFilter => {
                self.draw_vault_health(root, self.current())
            }
            Surface::Rehearsal => self.draw_rehearsal(root),
            Surface::VaultSetupSpinner => self.draw_vault_setup_spinner(root),
            Surface::FirstWindowLoading
            | Surface::FirstWindowSlow
            | Surface::FirstWindowUnreachable
            | Surface::FirstWindowSlowLocalCopy
            | Surface::FirstWindowUnreachableLocalCopy
            | Surface::FirstWindowUnreachableSpent
            | Surface::FirstWindowUnreachableSpentLocalCopy
            | Surface::FirstWindowUnreachableDeclinedCopy => {
                self.draw_first_window(root, self.current())
            }
        }

        if self.screenshot && !self.done {
            if self.frames == WARMUP_FRAMES {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let captured = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = captured {
                let path = self.png_path();
                // The directory is created rather than assumed. A build with
                // `CARGO_TARGET_DIR` pointed elsewhere has no `./target`, and
                // this example used to panic on exactly that -- which is what
                // would happen on a runner that caches its target directory
                // outside the checkout.
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("could not create the screenshot dir");
                }
                save_png(&path, &image).expect("could not write the screenshot PNG");
                println!("wrote {}", path.display());
                if !self.advance(&ctx) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            // Keep frames coming: a hidden/idle window repaints lazily, and
            // the screenshot round-trip needs the pump to keep turning.
            ctx.request_repaint();
        }

        if self.current() == Surface::Overlay
            && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Which of the in-window panes `draw_pane` is drawing.
enum PaneKind {
    /// The read pane, in one of its three shots.
    Detail(DetailShot),
    /// The edit form with its discard confirmation up.
    Discard,
    /// The Send record composer.
    Composer,
    /// The "add a one-time code" form, mid-confirmation.
    TotpAdd,
    /// Design 6a's picker, the front door onto the four routes.
    TotpPicker,
    /// The preflight; `true` for the allowed state, `false` for the refusal.
    Preflight(bool),
}

/// Which fixture the read pane is drawn from, and in what reveal state.
///
/// A three-way enum rather than the pair of bools it would otherwise have
/// become: `Detail(true, false)` at a call site names neither of the things it
/// decides, and the reveal flag is the whole reason the third shot exists.
#[derive(Clone, Copy)]
enum DetailShot {
    /// The login fixture, masked -- `detail_login`.
    Login,
    /// The card fixture as it opens: number and code masked -- `detail_card`.
    Card,
    /// The card fixture with the NUMBER revealed and the security code still
    /// masked -- `detail_card_revealed`. The code stays hidden because that is
    /// the state a user reading their number is really in (the two rows have
    /// separate flags), and because a CVV in a checked-in PNG is worth
    /// avoiding even from a fixture.
    CardRevealed,
}

impl Preview {
    fn draw_overlay(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            // The preview closes on the dismiss ✕ too, so the affordance can
            // actually be clicked here rather than only looked at.
            if overlay_ui::draw_overlay_card(
                ui,
                "ledgerline.exe",
                "Ledgerline",
                Some("a.novak@ledgerline.com"),
            ) == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3a, drawn by the shipped function rather than re-implemented --
    /// which is why `draw_no_match_card` is public.
    fn draw_overlay_no_match(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_no_match_card(ui, "Atlas Licence")
                == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3b, drawn by the shipped function for the same reason 3a is:
    /// this card and the no-match one differ by three strings, and a
    /// re-implementation here could show either while the app showed the
    /// other.
    fn draw_overlay_locked(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_locked_card(ui, "Atlas Licence")
                == overlay_ui::OverlayAction::Dismiss
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3c, drawn by the shipped function for the same reason 3a and 3b
    /// are.
    ///
    /// The form is drawn **blank apart from the App row**, which is the state
    /// the user is really shown: exactly one of the four rows can be
    /// pre-filled, because `injector::ui_automation` has no username reader
    /// and a password field's contents are not read. A preview that typed
    /// plausible values into the other two would be a picture of a capture
    /// this app does not make.
    fn draw_overlay_save_login(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            if overlay_ui::draw_save_login_card(ui, &mut self.save_login)
                != overlay_ui::SaveLoginAction::None
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    /// Design 3d, in whichever of its three states `surface` names.
    ///
    /// The card is the shipped one, so this preview cannot drift from it. The
    /// action is dropped rather than acted on: *Copy* would reach the real
    /// clipboard and *New* would need a vault, and a preview must do neither.
    fn draw_overlay_generate(
        &mut self,
        root: &mut egui::Ui,
        ctx: &egui::Context,
        surface: Surface,
    ) {
        let slot = match surface {
            Surface::OverlayGenerate => 0,
            Surface::OverlayGenerateWorking => 1,
            _ => 2,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| {
                if overlay_ui::draw_generate_card(ui, &mut self.generate[slot])
                    == overlay_ui::GenerateAction::Dismiss
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    }

    fn draw_login(
        &mut self,
        root: &mut egui::Ui,
        ctx: &egui::Context,
        signin: bool,
        auth_in_progress: bool,
    ) {
        // The exact chrome the shipped window draws.
        if login_ui::draw_window_chrome(root, "Log in to Deskwarden")
            == login_ui::ChromeAction::Close
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let mut resized = false;
        egui::Frame::new()
            .inner_margin(Margin { left: 26, right: 26, top: 24, bottom: 30 })
            .show(root, |ui| {
                ui.set_min_width(ui.available_width());
                // Sample data mirroring the 3h mock (unlock: Hello shown as
                // enrolled so the panel renders; sign-in: available but
                // unenrolled so the opt-in and server dropdown render);
                // actions are ignored -- a preview must never spawn `bw` or
                // pop Hello.
                let (status, email, hello) = if signin {
                    // Self-hosted: the tallest state (URL field + email +
                    // password + Hello panel), which is what overflowed a
                    // fixed-height window.
                    self.form.server_choice = login_ui::ServerChoice::SelfHosted;
                    (
                        BwStatus::Unauthenticated,
                        None,
                        HelloState { available: true, enrolled: false },
                    )
                } else {
                    (
                        BwStatus::Locked,
                        Some("a.novak@ledgerline.com"),
                        HelloState { available: true, enrolled: true },
                    )
                };
                let mut flow_bottom = 0.0;
                let _ = login_ui::draw_login_window(
                    ui,
                    status,
                    email,
                    "vault.ledgerline.eu",
                    hello,
                    &mut self.form,
                    &mut flow_bottom,
                    // In flight only for `LoginUnlockBusy`, and even there
                    // nothing is actually running: this draws the window's
                    // states and never spawns a real `bw`.
                    auth_in_progress,
                    // Not a first run: the preview draws the window an
                    // existing account meets, so the first-run notice is not
                    // part of what this screenshots.
                    false,
                );
                // Size to content, exactly as run_login_flow does, so the
                // screenshot shows the window the app would show.
                let wanted = (flow_bottom + login_ui::FOOTER_RESERVE).ceil();
                if (wanted - self.window_height).abs() > 0.5 {
                    self.window_height = wanted;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        470.0, wanted,
                    )));
                    resized = true;
                }
            });
        // The geometry moved, so the warm-up starts again: a capture taken on
        // the frame a resize was asked for shows the PREVIOUS size, which is
        // how a screenshot of a window nobody ships gets into an artifact.
        if resized {
            self.frames = 0;
        }
    }

    /// The preferences window's Clipboard page.
    ///
    /// Draws `prefs_ui::draw_prefs_body` -- the real nav-plus-content shell,
    /// not a reconstruction of it -- on the window's own background, so the
    /// PNG shows the page exactly as the app draws it. The `PrefsState` is
    /// rebuilt each frame rather than held: nothing on this page needs to
    /// survive between frames for a screenshot, and a held state would make
    /// the two surfaces share the interval field's text buffer.
    fn draw_prefs(&mut self, root: &mut egui::Ui, master_on: bool) {
        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings {
            clear_clipboard: master_on,
            ..deskwarden::settings::Settings::default()
        });
        state.show(prefs_ui::Section::Clipboard);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    /// The About page's update card, in each of the four states worth looking
    /// at.
    ///
    /// **Nothing here can reach the network, and that is structural rather
    /// than careful.** The panel is parked in a stage
    /// (`prefs_ui::PrefsState::show_update_stage`) with no receiver behind it,
    /// and the flow refuses to start any work at all without a process-wide
    /// `update_panel::UpdateEnv` -- which only `main.rs` installs and this
    /// example does not. So a preview run makes no request and spawns no
    /// thread even if a frame's button were somehow clicked.
    ///
    /// The state is rebuilt each frame, as `draw_prefs` rebuilds its own:
    /// there is nothing here to carry between frames, because every stage
    /// these surfaces show is stated outright rather than arrived at.
    /// The preferences window as the OS shows it: the titlebar this app
    /// paints itself, and the form directly under it.
    ///
    /// `prefs_ui::draw_prefs_window` is the exact function `run` calls every
    /// frame, so what this picture says about the seam between the chrome and
    /// the rail is what the real window says.
    /// The Breaches page, in each of the four states worth looking at.
    ///
    /// **Nothing here can reach the network, and that is structural rather
    /// than careful.** The panel is parked in a stage with no receiver behind
    /// it (`PrefsState::show_scan_stage`), and the flow refuses to start any
    /// work at all without a process-wide `breach_scan::ScanEnv` -- which
    /// only `main.rs` installs and this example does not. So a preview run
    /// makes no request and spawns no thread even if a frame's button were
    /// somehow clicked.
    ///
    /// The history is supplied here rather than read off disk: this example
    /// must not touch `%APPDATA%\Deskwarden`, and the instants below are
    /// stated so the picture is the same one twice running.
    fn draw_prefs_breaches(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::breach_scan::ScanStage;
        use deskwarden::scan_history::{ScanHistory, ScanRecord};

        // 2026-08-18T00:30:00Z, and the days before it. Rendered in the
        // machine's own timezone, which is the rule -- see `local_time`.
        const AT: i64 = 1_787_013_000_000;
        const DAY: i64 = 86_400_000;
        let entry = |ago: i64, checked: u32, items: u32, found: u32, failed: u32| ScanRecord {
            finished_at_unix_millis: AT - ago * DAY,
            passwords_checked: checked,
            items_covered: items,
            found,
            failed,
        };

        let failed_run = entry(0, 128, 1_600, 3, 40);
        let stage = match surface {
            Surface::PrefsBreachesRunning => {
                ScanStage::Running { done: 61, total: 128, found: 3, failed: 40 }
            }
            Surface::PrefsBreachesFailed => ScanStage::Finished(failed_run),
            _ => ScanStage::Idle,
        };
        let history = match surface {
            // The empty state is a result and not a blank panel, so it is a
            // picture of its own.
            Surface::PrefsBreachesIdle | Surface::PrefsBreachesRunning => Vec::new(),
            Surface::PrefsBreachesFailed => vec![failed_run],
            // Several runs deep, and DELIBERATELY mixed: a clean run, a run
            // that found something, and a run that could not check most of
            // what it asked about -- so the two inks the list uses are both
            // in one picture and can be told apart.
            _ => vec![
                entry(0, 128, 1_600, 3, 0),
                entry(1, 127, 1_598, 0, 12),
                entry(4, 126, 1_590, 1, 0),
                entry(11, 120, 1_502, 0, 0),
            ],
        };

        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings::default());
        state.show(prefs_ui::Section::Breaches);
        state.show_scan_stage(stage);
        state.show_scan_history(ScanHistory { entries: history });
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    fn draw_prefs_window(&mut self, root: &mut egui::Ui) {
        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings::default());
        state.show(prefs_ui::Section::About);
        let _ = prefs_ui::draw_prefs_window(root, &mut state);
    }

    /// **The About page on its own**, now that there is an "on its own" to
    /// draw: two rows of fact, and nothing that can be pressed.
    ///
    /// The account comes from a fixture for the same reason the Updates
    /// surfaces park their stage -- this example publishes nothing, installs
    /// no process globals, and must not go anywhere near `bw` or
    /// `%APPDATA%\Deskwarden`.
    fn draw_prefs_about(&mut self, root: &mut egui::Ui) {
        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings::default());
        state.show(prefs_ui::Section::About);
        state.show_account_source(|| {
            Some(prefs_ui::AccountStatus::SignedIn {
                email: Some("someone@example.invalid".to_string()),
                server: Some("https://vault.example.invalid/api".to_string()),
            })
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    fn draw_prefs_updates(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::update_panel::UpdateStage;
        use deskwarden::updater::ReleaseInfo;

        // Longer than the page can show on purpose, so the screenshots answer
        // "does the region scroll or does it grow past the window" -- the one
        // thing about this card that a too-long release body could break, on
        // a window with no scrollbar of its own and no resize. The region now
        // takes the page's remaining height rather than a fixed 128 points,
        // so this body has to beat the WINDOW, not a constant.
        let release = ReleaseInfo {
            version: semver::Version::parse("0.9.0").unwrap(),
            installer_download_url: "https://example.invalid/deskwarden-0.9.0-installer.exe"
                .to_string(),
            installer_sha256: deskwarden::updater::parse_asset_digest(&format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .unwrap(),
            // **Markdown, because a real GitHub release body is.** Every
            // construct in the rendered subset is in here on purpose, so one
            // picture is the review of all of them: headings, bullets and
            // their nesting, bold, italic, inline code, a link (whose words
            // are styled and whose destination is beside them, and which
            // opens nothing), and -- at the end -- the things that are
            // deliberately NOT in the subset, painted as the characters they
            // are.
            body: concat!(
                "## Added\n",
                "- The update flow moved out of the tray and onto **this page**.\n",
                "- Release notes are shown *before* anything is downloaded.\n",
                "  - Including the ones from releases you skipped.\n",
                "- The download reports its progress where you started it.\n",
                "\n",
                "## Fixed\n",
                "- The tray no longer claims an update exists when none does.\n",
                "- A failed update says why, on a page rather than in a tooltip.\n",
                "- `release_notes_for_display` still strips what it stripped.\n",
                "\n",
                "## Notes\n",
                "- The full list is on [the releases page](https://example.invalid/r).\n",
                // The refusal, beside the acceptance, so one picture reviews
                // both: an https link is blue, underlined and followable; a
                // link with any other scheme is plain text with its
                // destination still beside it, and nothing on the page is
                // painted as a link that will not behave as one.
                "- Anything but https is text: [open settings](ms-settings:windowsupdate).\n",
                "- Raw HTML is not in the subset: <b>this stays literal</b>.\n",
                "- It is deliberately long here so this screenshot shows the\n",
                "  region scrolling rather than pushing the buttons off the\n",
                "  page.\n",
            )
            .to_string(),
        };

        let stage = match surface {
            Surface::PrefsUpdatesAvailable => UpdateStage::Available(release),
            // Short enough to fit the region with room to spare, which is the
            // state whose review is that there is NO bar beside it -- and,
            // since these lines are the ones that stay above the fold, where
            // the link and the inline code are put so a reviewer can see
            // what they look like without scrolling a screenshot.
            Surface::PrefsUpdatesShortNotes => UpdateStage::Available(ReleaseInfo {
                body: concat!(
                    "### Fixed\n",
                    "- The vault window remembers its size, via `settings.json`.\n",
                    "- Details on [the release page](https://example.invalid/r).\n",
                    "- Raw HTML stays literal: <b>not bold</b>.\n",
                )
                .to_string(),
                ..release
            }),
            // **What a user seven releases behind is shown.** Built in the
            // exact shape `updater::notes_across` composes -- one `##`
            // heading per version, newest first, and the release that
            // published no notes NAMED rather than missing, because a range
            // with a hole in it is not a range. This is also the case where
            // a scrollbar is legitimately wanted, so one picture reviews
            // both halves.
            //
            // **Seven rather than three, since the region grew.** The notes
            // region now takes the page's remaining height instead of a
            // fixed 128 points, and three versions no longer come close to
            // filling it -- which would have left every screenshot in this
            // set showing a region that fits, and the scrolling half of the
            // behaviour reviewed by nobody. The count is chosen against the
            // window, which is what it was always really measuring.
            Surface::PrefsUpdatesManyReleases => UpdateStage::Available(ReleaseInfo {
                body: concat!(
                    "## Deskwarden 0.9.0\n",
                    "- Release notes now cover **every** version you skipped.\n",
                    "- The scrollbar appears only when there is more to read.\n",
                    "\n",
                    "## Deskwarden 0.8.6\n",
                    "_This release came with no notes._\n",
                    "\n",
                    "## Deskwarden 0.8.5\n",
                    "- The vault window remembers its size, via `settings.json`.\n",
                    "- Details on [the release page](https://example.invalid/r).\n",
                    "\n",
                    "## Deskwarden 0.8.4\n",
                    "- Cyrillic names render in the app's own typeface.\n",
                    "- One Deskwarden per session, and starting it again takes over.\n",
                    "- Password health rows line up with the item list again.\n",
                    "\n",
                    "## Deskwarden 0.8.3\n",
                    "- The favourite star is lighter, rounder and quieter.\n",
                    "- Browsers no longer get the \"no saved login\" card.\n",
                    "\n",
                    "## Deskwarden 0.8.2\n",
                    "- The autofill prompt setting silences every pop-up.\n",
                    "- The reveal eye is taller and rounder.\n",
                    "\n",
                    "## Deskwarden 0.8.1\n",
                    "- The tray menu names the account it is signed in as.\n",
                    "- Preferences opens on the page it was last left on.\n",
                )
                .to_string(),
                ..release
            }),
            Surface::PrefsUpdatesDownloading => UpdateStage::Downloading {
                release,
                done: 2_400_000,
                total: Some(6_291_456),
            },
            Surface::PrefsUpdatesFailed => UpdateStage::Failed {
                message: "failed to download installer: connection closed".to_string(),
                release: Some(release),
            },
            _ => UpdateStage::UpToDate,
        };

        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings {
            // Off, so the "this button still asks, because you asked it to"
            // note is in the no-update picture. It is the visible half of the
            // decision that the manual check is not governed by the automatic
            // one, and a decision only a picture can show being communicated.
            check_for_updates: false,
            ..deskwarden::settings::Settings::default()
        });
        state.show(prefs_ui::Section::Updates);
        state.show_update_stage(stage);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    /// **Design 4d, finished**, drawn exactly as the rehearsal viewport draws
    /// it: `scratch_window::draw` on a root `Ui` filling the window, which is
    /// what `show_viewport_deferred` hands its callback.
    ///
    /// The arrived text is the literal a Win32 edit control would hold after
    /// the design's sequence -- a tab and a Windows line ending -- so the two
    /// glyph substitutions `rehearsal::arrived_panel` makes are in the
    /// picture rather than merely in a unit test.
    ///
    /// Nothing here sends anything: there is no window, no `Injector` and no
    /// plan. The view is a value.
    /// The startup spinner, drawn through the same `draw_spinner_body` the
    /// warm launch window paints -- not a re-creation of it, so what this PNG
    /// shows is what the user watches while `bw serve` starts.
    ///
    /// `CloseControl::Active` because that is the argument the warm launch
    /// host passes: this stage may be abandoned, and the picture is how a
    /// reviewer sees that the control saying so is drawn live rather than
    /// ghosted.
    fn draw_vault_setup_spinner(&mut self, root: &mut egui::Ui) {
        let _ = deskwarden::loading_ui::draw_spinner_body(
            root,
            "Setting up your vault...",
            login_ui::CloseControl::Active,
        );
    }

    /// **Design turn 7's window before the vault**, drawn through the same
    /// `draw_first_window_body` the recovery window paints -- so what these
    /// PNGs show is what a user meets when `bw serve` will not answer.
    ///
    /// `CloseControl::Active` because that is the argument the host passes in
    /// every body: this window may always be left, and the picture is how a
    /// reviewer sees that the control saying so is drawn live rather than
    /// ghosted.
    ///
    /// The hotkey status is `NotYetAttempted`, which is not a decorative
    /// choice: on the launch path nothing has tried to register the chord yet,
    /// so that is the line the real window shows -- and a preview showing
    /// `Armed` here would be a picture of a claim the app does not make.
    fn draw_first_window(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::loading_ui::{FirstWindowBody, LocalCopy, RetryOffer};
        // **A `Duration` and not a date.** The age these bodies render is an
        // elapsed span the host has already worked out, so these pictures are
        // the same pixels in a year's time as today -- see `LocalCopy`.
        const THREE_HOURS: std::time::Duration = std::time::Duration::from_secs(3 * 3600);
        let here = LocalCopy::Here { synced: Some(THREE_HOURS) };
        let body = match surface {
            Surface::FirstWindowSlow => {
                FirstWindowBody::Slow { seconds: 12, local: LocalCopy::None }
            }
            Surface::FirstWindowSlowLocalCopy => {
                FirstWindowBody::Slow { seconds: 12, local: here }
            }
            Surface::FirstWindowUnreachable => FirstWindowBody::Unreachable {
                retry: RetryOffer::Offered,
                local: LocalCopy::None,
            },
            Surface::FirstWindowUnreachableLocalCopy => FirstWindowBody::Unreachable {
                retry: RetryOffer::Offered,
                local: here,
            },
            Surface::FirstWindowUnreachableSpent => FirstWindowBody::Unreachable {
                retry: RetryOffer::Spent,
                local: LocalCopy::None,
            },
            Surface::FirstWindowUnreachableSpentLocalCopy => FirstWindowBody::Unreachable {
                retry: RetryOffer::Spent,
                local: here,
            },
            // The copy is there; its age is not, because this session never
            // got the key to read the file's header with.
            Surface::FirstWindowUnreachableDeclinedCopy => FirstWindowBody::Unreachable {
                retry: RetryOffer::Offered,
                local: LocalCopy::Here { synced: None },
            },
            _ => FirstWindowBody::Loading,
        };
        let _ = deskwarden::loading_ui::draw_first_window_body(
            root,
            body,
            deskwarden::loading_ui::FirstWindowFooter {
                account: Some("a.novak@ledgerline.com"),
                hotkey: deskwarden::hotkey::HotkeyStatus::Unavailable(
                    deskwarden::hotkey::Unavailable::NotYetAttempted,
                ),
            },
            login_ui::CloseControl::Active,
        );
    }

    fn draw_rehearsal(&mut self, root: &mut egui::Ui) {
        let _ = scratch_window::draw(
            root,
            &self.fixtures.rehearsal,
            &mut self.fixtures.rehearsal_arrived,
        );
    }

    /// The vault window's item list, drawn through `draw_item_list` itself on
    /// the same `theme::CANVAS` panel `vault_window::mod` hosts it in.
    ///
    /// The action is dropped: every one of them wants a vault behind it, and a
    /// preview must never reach one. The search buffer and the selection are
    /// the preview's own state so the shot shows a row in its SELECTED
    /// treatment -- which is the state the tile's border, fill and shadow all
    /// change in, and therefore the one a favicon has to look right against.
    fn draw_vault_list(&mut self, root: &mut egui::Ui) {
        let icons = self.icons.get_or_insert_with(|| {
            preview_icons(root.ctx(), &self.fixtures.list, &["Ledgerline", "Ledgerline corporate card"])
        });
        let fixtures = &self.fixtures;
        let (search, selected, visible) =
            (&mut self.list_search, &mut self.list_selected, &mut self.list_visible);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS))
            .show(root, |ui| {
                let _ = item_list::draw_item_list(
                    ui,
                    Some(&fixtures.list),
                    &fixtures.folders,
                    &SidebarFilter::All,
                    search,
                    selected,
                    None,
                    icons,
                    visible,
                    None,
                    false,
                );
            });
    }

    /// The vault window's **Password health** screen, drawn in the item-list
    /// column it really occupies and carrying a **pathologically long item
    /// name** -- longer than the column can be dragged to at any size.
    ///
    /// That name is the whole reason this surface is in the list. A finding
    /// row paints its name itself rather than through a `Label`, and
    /// `Painter::text` takes no width: the name used to be laid out at its
    /// natural width and drawn straight out past the tile's right edge and
    /// over the pane behind it. There are unit tests for that now, but a
    /// truncation is a thing somebody has to LOOK at -- whether the ellipsis
    /// is hung too close to the rounded corner, and whether the row still
    /// reads as a row, are not questions an assertion answers.
    ///
    /// Both bands are in the picture: a reuse group, whose rows are a name
    /// alone, and a weak finding, whose row is a name over a detail line.
    /// They are the two different vertical layouts the row has, and the long
    /// name is in both.
    /// The Password health screen, with and without a scan behind it, and
    /// under each of its four filters.
    ///
    /// **Nothing here reaches the network, and the type says so**:
    /// `report_with_scan` takes a `ScanResults` -- a map of answers with no
    /// channel, no agent and no URL -- and the answers below are stated
    /// outright rather than looked up.
    ///
    /// The filter is egui memory under an `Id`, which is where the pane keeps
    /// it, so it is set the way a click sets it: by pressing the chip. That
    /// is deliberately not a back door into the pane's state -- the picture
    /// shows what a user pressing that chip would see, or it shows nothing.
    fn draw_vault_health(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::breach::BreachStatus;
        use deskwarden::breach_scan::ScanResults;

        let mut scan = ScanResults::default();
        if !matches!(surface, Surface::VaultHealth) {
            // `health-long` is one of the pair on a reused password, so the
            // group it lands in is a REUSED password that is also breached --
            // the most urgent thing a vault can hold, and the case that
            // cross-cuts the two sections.
            scan.set_status(&["health-long".to_string(), "health-short".to_string()],
                BreachStatus::Breached(40_231));
            scan.set_status(&["health-breached".to_string()], BreachStatus::Breached(3));
            // Asked about, no answer. Listed as unknown rather than left out:
            // "not shown" reading as "safe" is the failure this section is
            // arranged against.
            scan.set_status(&["health-unknown".to_string()], BreachStatus::Unavailable);
            scan.set_status(&["health-weak".to_string()], BreachStatus::Safe);
        }
        let report = password_health::report_with_scan(&self.fixtures.health, &scan);

        // Which chip to press, if any. `EmptyFilter` presses Breached over a
        // report with no scan behind it, which is the state whose whole point
        // is the sentence it draws instead of a list.
        let chip = match surface {
            Surface::VaultHealthReusedOnly => Some(password_health::HealthFilter::Reused),
            Surface::VaultHealthWeakOnly => Some(password_health::HealthFilter::Weak),
            Surface::VaultHealthBreachedOnly | Surface::VaultHealthEmptyFilter => {
                Some(password_health::HealthFilter::Breached)
            }
            _ => None,
        };
        let report = if matches!(surface, Surface::VaultHealthEmptyFilter) {
            password_health::report_for(&self.fixtures.health)
        } else {
            report
        };

        // Written into the SAME egui memory the chip writes -- the pane's own
        // storage, not a second copy of it -- because a chip's rect is not
        // known until after the frame that draws it, and a screenshot run has
        // no second frame to click on.
        password_health::show_filter(root.ctx(), chip.unwrap_or(password_health::HealthFilter::All));

        let selected = &mut self.health_selected;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS))
            .show(root, |ui| {
                password_health::draw_password_health(ui, &report, selected, false);
            });
    }

    /// The vault window's rail, drawn through `draw_sidebar` itself in the same
    /// `theme::CARD` panel frame `vault_window::mod` hosts it in -- margins
    /// included, because the rail's insets are measured against them.
    ///
    /// Four folder rows: enough that the FOLDERS group reads as a group, few
    /// enough that the two screen rows below them are in the picture.
    fn draw_vault_rail(&mut self, root: &mut egui::Ui) {
        let fixtures = &self.fixtures;
        let (selected, sends, health) =
            (&mut self.rail_selected, &mut self.rail_sends, &mut self.rail_health);
        egui::CentralPanel::default()
            // Design 4.8's own frame, copied from the `Panel::left` in
            // `vault_window::mod`: `theme::CARD` with `padding: 14px 10px`.
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(10, 14)))
            .show(root, |ui| {
                let _ = sidebar::draw_sidebar(
                    ui,
                    sidebar::VaultLists {
                        sends: Some(3),
                        health_findings: 4,
                        ..sidebar::VaultLists::live_only(&fixtures.list)
                    },
                    &fixtures.rail_folders,
                    selected,
                    sidebar::Screens { sends, health },
                    "Locks in 11:42",
                );
            });
    }

    /// The surfaces that live *inside* the vault window rather than in one of
    /// their own, drawn on the window's own canvas so the PNG shows them
    /// against the background they actually sit on.
    fn draw_pane(&mut self, root: &mut egui::Ui, kind: PaneKind) {
        let fixtures = &mut self.fixtures;
        egui::CentralPanel::default()
            .frame(pane_frame())
            .show(root, |ui| match kind {
                PaneKind::Detail(shot) => {
                    let card = !matches!(shot, DetailShot::Login);
                    let item = if card { &fixtures.card } else { &fixtures.login };
                    // **Set, never toggled.** One `Fixtures` is shared by the
                    // whole `--all` walk, so a reveal flag left standing would
                    // change a later surface depending on the order the walk
                    // happened to run in. Assigning the whole state makes each
                    // shot independent of the ones before it.
                    fixtures.reveal = RevealState {
                        card_number: matches!(shot, DetailShot::CardRevealed),
                        ..RevealState::default()
                    };
                    // A card has no one-time-code row to have a state for; the
                    // login fixture shows a live code, which is the state that
                    // row spends most of its life in.
                    let no_totp = TotpState::NoSecret;
                    let totp = if card { &no_totp } else { &fixtures.totp };
                    let _ = detail::draw_detail_read(
                        ui,
                        item,
                        Some("Work"),
                        // The preview has no vault behind it, so the kebab's
                        // "Move to folder" submenu says "No folders yet" --
                        // which is a real state of that submenu and the
                        // honest one to show without a folder list.
                        &[],
                        if card { 0 } else { 42 },
                        totp,
                        false,
                        &mut fixtures.reveal,
                        // No favicon texture: the monogram fallback is what
                        // every avatar in this app shows without one, and
                        // loading an image here would mean a file to ship.
                        None,
                        &mut fixtures.apps,
                        // Breach checking OFF -- `detail::should_check` is the
                        // gate, and with it false the cache below is never
                        // asked anything and no worker is started. A preview
                        // must not reach Have I Been Pwned.
                        false,
                        // The TOTP seed row is a preference that is off by
                        // default; the preview draws the default window.
                        false,
                        &mut fixtures.breaches,
                    );
                }
                PaneKind::Discard => {
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.draft,
                        &fixtures.folders,
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.login),
                        &fixtures.totp,
                    );
                    // The form's Cancel is what sets this in the app; a
                    // preview has nobody to click it, so it is re-armed every
                    // frame. Re-armed rather than set once, because the
                    // confirmation's own KeepEditing arm clears it -- and a
                    // stray Escape delivered to this window would otherwise
                    // take the dialog away before the capture.
                    fixtures.draft.discard_prompt = true;
                }
                PaneKind::Composer => {
                    let _ = record_ui::draw_export_form(
                        ui,
                        &mut fixtures.record,
                        "Ledgerline \u{b7} a.novak@ledgerline.com",
                        false,
                    );
                }
                PaneKind::TotpAdd => {
                    // A FIXED instant, not `SystemTime::now()`: the code and
                    // the countdown are the point of this shot, and a
                    // screenshot whose two most prominent numbers change every
                    // run is a screenshot no reviewer can diff.
                    let _ = totp_add::draw_add_form(ui, &mut fixtures.totp_add, PREVIEW_UNIX);
                }
                PaneKind::TotpPicker => {
                    let _ = totp_add::draw_picker(ui, &mut fixtures.totp_picker);
                }
                PaneKind::Preflight(allowed) => {
                    let state = if allowed { &mut fixtures.allowed } else { &mut fixtures.refused };
                    let _ = preflight::draw(ui, state);
                }
            });
    }
}

/// Everything the panes need, built once from literals.
struct Fixtures {
    login: VaultItem,
    card: VaultItem,
    /// The item list's rows -- see [`LIST_JSON`].
    list: Vec<VaultItem>,
    /// More folders than the rail's height fits -- see `draw_vault_rail`.
    rail_folders: Vec<Folder>,
    folders: Vec<Folder>,
    totp: TotpState,
    reveal: RevealState,
    apps: AppIdentityCache,
    breaches: BreachCache,
    draft: EditDraft,
    record: RecordDraft,
    totp_add: TotpAdd,
    totp_picker: TotpAdd,
    allowed: PreflightState,
    refused: PreflightState,
    rehearsal: scratch_window::RehearsalView,
    rehearsal_arrived: String,
    /// The Password health screen's items -- see [`HEALTH_JSON`].
    health: Vec<VaultItem>,
}

impl Fixtures {
    fn new() -> Self {
        let login = item(LOGIN_JSON);
        let card = item(CARD_JSON);
        let list = items(LIST_JSON);
        // **The fixture is checked against the enumeration, not against a
        // number written here twice.** The list shot exists to show every
        // network's badge beside every other one; a brand added to
        // `CARD_BRANDS` and not to `LIST_JSON` would leave a badge nobody ever
        // looks at, which is exactly the failure this whole example is for.
        let networked: std::collections::BTreeSet<_> = list
            .iter()
            .filter_map(|i| item_list::card_network(i).map(|b| b.canonical()))
            .collect();
        assert_eq!(
            networked.len(),
            deskwarden::card_brand::CARD_BRANDS.len(),
            "the item-list fixture shows {} of the {} card networks: {networked:?}",
            networked.len(),
            deskwarden::card_brand::CARD_BRANDS.len()
        );
        let mut draft = EditDraft::from_item(&login);
        // Dirty, because the confirmation only exists for a dirty draft --
        // showing it over a pristine form would be a picture of a state the
        // app does not reach.
        draft.password.push_str("-edited");
        draft.discard_prompt = true;
        let totp = TotpState::Code { code: "418902".to_string(), seconds_left: 19 };
        // The seed's tick on, so the composer's seed warning -- the sentence
        // that decides whether that tick was a mistake -- is in the picture.
        let mut record = RecordDraft { open: true, ..Default::default() };
        record.set_totp(true);
        // Opened against an item that ALREADY has a code, so the replace
        // warning is in the picture, and with a URI typed whose parameters are
        // all non-default -- 8 digits over 60 seconds under SHA-256 -- because
        // that is the case the confirmation exists to catch and the one a shot
        // of a plain 6/30 card cannot show.
        let mut totp_add = TotpAdd::opening("preview", "Git Host \u{b7} anovak", true);
        totp_add.typed = zeroize::Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host\
             &digits=8&period=60&algorithm=SHA256"
                .to_string(),
        );
        // A SECOND state of the same form, on design 6a. Carrying a refusal,
        // because a picker with nothing wrong on it shows three of the four
        // things this surface has to get right and not the fourth: a refusal
        // rendered as a sentence that names its reason.
        let mut totp_picker = TotpAdd::opening("preview", "Git Host \u{b7} anovak", true);
        totp_picker.refusal = Some(totp_add::PickerRefusal::NoCode(totp_add::CodeSource::Region));

        Self {
            folders: vec![
                Folder { id: "f-work".into(), name: "Work".into(), other: Default::default() },
                Folder {
                    id: "f-personal".into(),
                    name: "Personal".into(),
                    other: Default::default(),
                },
            ],
            reveal: RevealState::default(),
            apps: AppIdentityCache::default(),
            // A check that is never called: `check_breaches` is false at the
            // call site, so `should_check` refuses before this is consulted.
            // It panics rather than returning a plausible answer, so a future
            // change that starts asking is a loud failure in CI instead of a
            // screenshot quietly claiming a password is safe.
            breaches: BreachCache::new(std::sync::Arc::new(|_, _| {
                unreachable!("a preview must never check a password against a breach corpus")
            })),
            allowed: preflight_state(true, &login, &totp),
            refused: preflight_state(false, &login, &totp),
            rehearsal: rehearsal_view(),
            health: items(HEALTH_JSON),
            // What a text field really holds after the design's sequence: the
            // Tab arrived as a tab, the Enter as a Windows line ending.
            rehearsal_arrived: format!(
                "{}	
{}
",
                rehearsal::SAMPLE_USER,
                rehearsal::SAMPLE_PASSWORD
            ),
            record,
            totp_add,
            totp_picker,
            draft,
            login,
            card,
            list,
            // Three named folders plus `bw serve`'s virtual "No Folder" bucket.
            // Chosen so the WHOLE rail fits the shot: the point of this surface
            // is whether the three groups read as three, and a picture whose
            // bottom group is off the bottom edge cannot answer that. That the
            // rail scrolls when a vault has more is pinned by
            // `the_screen_rows_survive_a_vault_with_a_folder_for_every_letter`,
            // which is a claim about reachability rather than about looks.
            rail_folders: [
                "Engineering",
                "Personal",
                "Shared with me",
            ]
            .iter()
            .enumerate()
            .map(|(i, name)| Folder {
                id: format!("rf-{i}"),
                name: (*name).to_string(),
                other: Default::default(),
            })
            .chain(std::iter::once(Folder {
                id: String::new(),
                name: "No Folder".into(),
                other: Default::default(),
            }))
            .collect(),
            totp,
        }
    }
}

/// **Design 4d's finished rehearsal, built the way production builds it.**
///
/// `sample_plan` -> `rehearsal_plan` -> `transcript` is the exact chain the
/// window runs, so the acts listed in the PNG are the acts a real rehearsal of
/// the design's sequence produces -- chunking, joining and all -- rather than a
/// hand-written list that could drift from it. No vault is touched: every field
/// in that plan resolves to a fixed sample by construction.
fn rehearsal_view() -> scratch_window::RehearsalView {
    const DESIGN_SEQUENCE: &str = "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}";
    let planned = rehearsal::sample_plan(DESIGN_SEQUENCE).expect("the design's sequence plans");
    let sent = rehearsal::transcript(
        &rehearsal::rehearsal_plan(&planned).expect("the substituted sequence re-plans"),
    );
    scratch_window::RehearsalView {
        headline: rehearsal::finished_line(
            std::time::Duration::from_millis(2100),
            sent.len(),
        ),
        finished: true,
        sent,
        failure: None,
    }
}

/// A stand-in favicon, built rather than downloaded.
///
/// **No network, and no bundled third-party artwork.** What the favicon shot
/// has to show is the RELATIONSHIP between a piece of edge-to-edge artwork and
/// the tile it sits in, and any image whose colour runs to all four edges
/// answers that. This one is a filled ground with a lighter ring on it -- the
/// shape of a great many real site icons -- at the 64px longest edge
/// `favicon::decode_rgba` resamples every real icon to, so the tile is asked
/// to scale exactly what it is asked to scale in the app.
fn stand_in_favicon(ctx: &egui::Context, name: &str, ground: egui::Color32) -> egui::TextureHandle {
    const PX: usize = 64;
    let mut pixels = vec![egui::Color32::TRANSPARENT; PX * PX];
    let centre = (PX as f32 - 1.0) / 2.0;
    for y in 0..PX {
        for x in 0..PX {
            let d = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // A ring at roughly half the radius, which is the only detail on
            // it: enough to see the artwork scale, not so much that the eye
            // starts reviewing the drawing instead of the tile.
            pixels[y * PX + x] = if (14.0..20.0).contains(&d) {
                egui::Color32::from_rgb(0xff, 0xff, 0xff)
            } else {
                ground
            };
        }
    }
    ctx.load_texture(
        format!("preview-favicon-{name}"),
        egui::ColorImage { size: [PX, PX], pixels, source_size: egui::vec2(PX as f32, PX as f32) },
        egui::TextureOptions::LINEAR,
    )
}

/// The icon cache the list preview draws against: a stand-in favicon on the
/// named items and nothing on the rest, so ONE picture holds all three of the
/// row's leading treatments -- favicon in a tile, monogram fallback, and (on a
/// card) the network badge over each of them.
fn preview_icons(
    ctx: &egui::Context,
    items: &[VaultItem],
    with_icons: &[&str],
) -> item_list::IconCache {
    let mut cache = item_list::IconCache::default();
    for (n, item) in items.iter().filter(|i| with_icons.contains(&i.name.as_str())).enumerate() {
        let ground = if n % 2 == 0 {
            egui::Color32::from_rgb(0x1f, 0x6f, 0x5c)
        } else {
            egui::Color32::from_rgb(0x8a, 0x2f, 0x3c)
        };
        cache
            .textures
            .insert(item.id.clone(), stand_in_favicon(ctx, &item.id, ground));
    }
    cache
}

/// A fixture item, in the wire shape `bw serve` returns.
///
/// Parsed rather than built field by field, for the reason the wire tests are
/// written the same way: a struct literal keeps compiling when the JSON the app
/// actually receives has moved on, and a fixture that no longer describes the
/// real thing is worse than no fixture.
fn item(json: &str) -> VaultItem {
    serde_json::from_str(json).expect("the preview's fixture item must parse as a VaultItem")
}

/// A whole fixture list, parsed for the same reason [`item`] is.
fn items(json: &str) -> Vec<VaultItem> {
    serde_json::from_str(json).expect("the preview's fixture list must parse as VaultItems")
}

const LOGIN_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a001",
  "type": 1,
  "name": "Ledgerline",
  "folderId": "f-work",
  "favorite": true,
  "notes": "Finance approves new seats on the first Monday of the month.",
  "login": {
    "username": "a.novak@ledgerline.com",
    "password": "correct-horse-battery-staple-7",
    "totp": "otpauth://totp/Ledgerline:a.novak?secret=JBSWY3DPEHPK3PXP&issuer=Ledgerline",
    "uris": [{ "uri": "https://app.ledgerline.eu/signin" }]
  },
  "fields": [{ "name": "Employee ID", "value": "LL-40912", "type": 0 }]
}"#;

const CARD_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a002",
  "type": 3,
  "name": "Ledgerline corporate card",
  "folderId": "f-work",
  "notes": "Expenses only. Anything over EUR 500 needs a purchase order first.",
  "card": {
    "cardholderName": "ANNA NOVAK",
    "brand": "Visa",
    "number": "4111111111111111",
    "expMonth": "11",
    "expYear": "2029",
    "code": "417"
  }
}"#;

/// The item list's rows: one card per network, plus the two login shapes.
///
/// **Every brand in `card_brand::CARD_BRANDS`, once.** The network badge's
/// whole job is to say WHICH network, so the only useful picture of it is one
/// with all of them side by side -- a single Visa row proves nothing about
/// whether Visa can be told from Discover at 12pt. `Fixtures::new` checks the
/// count against the enumeration, so a network added later cannot quietly go
/// unphotographed.
///
/// The brands are stored on the items rather than left to be inferred from the
/// digits: `item_list::card_network` prefers a stored brand, which is the path
/// a real vault takes, and the numbers here are the published test numbers
/// anyway.
/// The Password health screen's fixture -- see `draw_vault_health`.
///
/// Two items on one password (a reuse group) and one item on a short
/// single-class one (a weak finding), and **the long name is on one of
/// each**, because the reuse row and the weak row are different vertical
/// layouts and the truncation has to be looked at in both.
///
/// The long name is the one from the user's report, which is a real page
/// title -- a product name with the platforms it supports appended after a
/// pipe. Names like this are what a browser extension saves, so this is the
/// ordinary case and not a contrived one.
const HEALTH_JSON: &str = r#"[
  {
    "id": "health-long", "type": 1,
    "name": "Visual Studio App Center | iOS, Android, Xamarin & React Native App Development",
    "login": { "username": "a.novak@ledgerline.com", "password": "reused-across-two-sites" }
  },
  {
    "id": "health-short", "type": 1, "name": "Northwind Mail",
    "login": { "username": "anna@northwind.example", "password": "reused-across-two-sites" }
  },
  {
    "id": "health-weak", "type": 1,
    "name": "Meridian Freight Bill Pay | Invoices, Statements & Payment History",
    "login": { "username": "anna@northwind.example", "password": "meridian9" }
  },
  {
    "id": "health-breached", "type": 1, "name": "Harbourline Rail",
    "login": { "username": "anna@northwind.example", "password": "harbour-single-use" }
  },
  {
    "id": "health-unknown", "type": 1, "name": "Cantilever Studio",
    "login": { "username": "anna@northwind.example", "password": "cantilever-single-use" }
  }
]"#;

const LIST_JSON: &str = r#"[
  {
    "id": "list-0001", "type": 1, "name": "Ledgerline", "folderId": "f-work",
    "favorite": true,
    "login": { "username": "a.novak@ledgerline.com", "password": "x",
      "uris": [{ "uri": "https://app.ledgerline.eu/signin" }] }
  },
  {
    "id": "list-0002", "type": 1, "name": "Northwind Mail",
    "login": { "username": "anna@northwind.example", "password": "x" }
  },
  {
    "id": "list-0003", "type": 3, "name": "Ledgerline corporate card",
    "folderId": "f-work",
    "card": { "brand": "Visa", "number": "4111111111111111", "expMonth": "11",
      "expYear": "2029" }
  },
  {
    "id": "list-0004", "type": 3, "name": "Household card",
    "card": { "brand": "Mastercard", "number": "5555555555554444",
      "expMonth": "02", "expYear": "2028" }
  },
  {
    "id": "list-0005", "type": 3, "name": "Travel card",
    "card": { "brand": "Amex", "number": "378282246310005",
      "expMonth": "07", "expYear": "2027" }
  },
  {
    "id": "list-0006", "type": 3, "name": "Rewards card",
    "card": { "brand": "Discover", "number": "6011111111111117",
      "expMonth": "01", "expYear": "2030" }
  },
  {
    "id": "list-0007", "type": 3, "name": "Osaka office card",
    "card": { "brand": "JCB", "number": "3530111333300000", "expMonth": "09",
      "expYear": "2026" }
  },
  {
    "id": "list-0008", "type": 3, "name": "Entertainment card",
    "card": { "brand": "Diners Club", "number": "30569309025904",
      "expMonth": "05", "expYear": "2029" }
  },
  {
    "id": "list-0009", "type": 3, "name": "Shenzhen supplier card",
    "card": { "brand": "UnionPay", "number": "6200000000000005",
      "expMonth": "12", "expYear": "2028" }
  },
  {
    "id": "list-0010", "type": 3, "name": "Berlin contractor card",
    "card": { "brand": "Maestro", "number": "5018000000000009",
      "expMonth": "03", "expYear": "2029" }
  },
  {
    "id": "list-0011", "type": 3, "name": "Bengaluru office card",
    "card": { "brand": "RuPay", "number": "6069000000000009",
      "expMonth": "08", "expYear": "2030" }
  },
  {
    "id": "list-0012", "type": 3, "name": "Store loyalty card",
    "card": { "brand": "Other", "number": "9900112233445566",
      "expMonth": "06", "expYear": "2031" }
  }
]"#;

/// **Design 7's `dw-bar`, one cycle, as six stills.**
///
/// Every other surface in this file is a picture of a state. This one is a
/// picture of a state machine, and it exists because of what a PNG of an
/// animated widget honestly cannot say. A single frame of a sliding bar is a
/// blue dash somewhere in a grey rail; whether it MOVES -- and whether it
/// moves on the design's curve rather than at a constant crawl -- is exactly
/// the property a still cannot carry. At the design's own opening keyframe,
/// `translateX(-100%)`, there is not even a dash: the knob is entirely outside
/// the track and `theme::paint_progress_bar` clips it away, so a reviewer
/// looking at a PNG taken at t=0 would see an empty rail and reasonably
/// conclude the bar was broken.
///
/// So the phases are laid out side by side and labelled. It is
/// `theme::paint_progress_bar` -- the SAME painter the live widget calls, one
/// line below its own clock -- so nothing here is a second drawing of the bar
/// that could drift from the shipped one. What a reader can check off this
/// picture: the knob is a third of the track, it starts and ends out of sight,
/// and the middle rows are further apart than the outer ones, which is the
/// `ease-in-out` the design asks for and the difference between a bar that
/// reads as motion and a marquee.
fn draw_progress_bar_cycle(root: &mut egui::Ui) {
    theme::paint_window_background(root);
    egui::Frame::new()
        .fill(theme::CANVAS)
        .inner_margin(Margin::same(24))
        .show(root, |ui| {
            ui.label(theme::bold("dw-bar, one 1.4s cycle", 15.0).color(theme::INK));
            ui.add_space(4.0);
            ui.label(
                theme::semibold("theme::paint_progress_bar at six fixed phases", 12.0)
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(18.0);
            for step in 0..=5 {
                let at = f64::from(step) * f64::from(theme::BAR_PERIOD) / 5.0;
                let phase = theme::bar_phase(at);
                ui.label(
                    theme::semibold(format!("t = {at:.2} s"), 11.0).color(theme::TEXT_GHOST),
                );
                ui.add_space(3.0);
                let (track, _) = ui.allocate_exact_size(
                    egui::vec2(260.0, theme::BAR_HEIGHT),
                    egui::Sense::hover(),
                );
                theme::paint_progress_bar(ui.painter(), track, phase);
                ui.add_space(20.0);
            }
        });
}

/// The two preflight states.
///
/// `allowed`: the rule's own process is in front and the focused control is
/// masked. `refused`: a different process, with an unmasked control -- both
/// facts wrong at once, which is the state whose message has to name both.
fn preflight_state(allowed: bool, item: &VaultItem, totp: &TotpState) -> PreflightState {
    let target = if allowed {
        SendTarget {
            title: "Ledgerline \u{2014} Sign in".to_string(),
            image_name: "ledgerline.exe".to_string(),
            pid: 8124,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: true,
        }
    } else {
        SendTarget {
            title: "chat \u{2014} #finance".to_string(),
            image_name: "teams.exe".to_string(),
            pid: 5310,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: false,
        }
    };
    let login = item.login.as_ref();
    let source = ResolveSource {
        username: login.and_then(|l| l.username.as_deref()).unwrap_or(""),
        password: login.and_then(|l| l.password.as_deref()).map(|p| p.as_str()).unwrap_or(""),
        custom: deskwarden::key_sequence::custom_pairs(item),
        totp,
    };
    PreflightState::new(target, "ledgerline.exe", "{USERNAME}{TAB}{PASSWORD}{ENTER}", &source)
}

fn save_png(path: &PathBuf, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let [w, h] = image.size;
    let mut data = Vec::with_capacity(w * h * 4);
    for p in &image.pixels {
        data.extend_from_slice(&p.to_array());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&data)?;
    Ok(())
}

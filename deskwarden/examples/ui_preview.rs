//! Visual preview of the themed windows.
//!
//! Interactive:
//!
//! ```text
//! cargo run --example ui_preview            # the save-a-login form (design 3c)
//! cargo run --example ui_preview -- --login # the login/unlock window (design 3h)
//! ```
//!
//! The save-a-login card closes on Enter/Esc/✕; the login preview just draws (its
//! Continue does nothing here -- no `bw` is spawned from a preview).
//!
//! Self-screenshotting (for reviewing the design implementation without a
//! human at the keyboard):
//!
//! ```text
//! cargo run --example ui_preview -- --screenshot          # the 3c form
//! cargo run --example ui_preview -- --login --screenshot  # the login window
//! cargo run --example ui_preview -- --cli-setup --screenshot      # the CLI requirement modal
//! cargo run --example ui_preview -- --backend-choice --screenshot # the self-hosted backend choice
//! cargo run --example ui_preview -- --vault --screenshot        # the Vault page, both clients
//! cargo run --example ui_preview -- --sends --screenshot        # design 5b's Sends screen, six states
//! cargo run --example ui_preview -- --pickers --screenshot      # the Add app pickers and their empty states
//! cargo run --example ui_preview -- --totp --screenshot         # designs 6a/6c/6d, the add-a-code card, seven states
//! cargo run --example ui_preview -- --edit --screenshot         # design 8a's Edit login form, five states
//! cargo run --example ui_preview -- --kit --screenshot          # the v2 kit sheet (1.0 redesign)
//! cargo run --example ui_preview -- --shell --screenshot        # the v2 shell, navy and light, and the splash
//! cargo run --example ui_preview -- --all                 # EVERY surface below
//! ```
//!
//! `--sends` writes six PNGs into `target/ui_preview_sends/`: a Send picked,
//! nothing picked, the composer, an account with none, `Shared with me`, and
//! the whole screen at the app's minimum window size. It is a directory for
//! the reason `--vault` and `--kinds` are -- the states are only worth
//! looking at beside each other, and beside design 5b.
//!
//! `--totp` writes seven PNGs into `target/ui_preview_totp/`, and it exists
//! because **the add-a-code modal could not be rendered here at all**. Every
//! other surface in this file could be looked at without the app running;
//! design 6d could only be reached by opening a vault, opening an item,
//! pressing Add code and typing into the field -- so a refusal that wrapped
//! to three lines and painted itself over the box above it shipped, and the
//! only review anybody could give it was a screenshot taken by hand. The
//! seven are 6a's picker, 6d typed and accepted, 6d refused (the reported
//! `sdf`), 6d carrying the LONGEST refusal the parser can produce, 6c's
//! scanned confirmation, the fused confirmation with non-default parameters,
//! and the whole card at the app's minimum window size. A directory for
//! `--sends`' reason: the line under the field is only reviewable beside the
//! states either side of it.
//!
//! `--edit` writes five PNGs into `target/ui_preview_edit/`, and it exists for
//! `--totp`'s reason one step on: the **Edit login form** -- design 8a -- had
//! no surface here at all. Two shots of individual BLOCKS of it did
//! (`edit_websites`, `edit_sparse`), each drawn at a height chosen so its own
//! block cleared the fold, and neither was a picture of the form. The owner
//! screenshotted the whole pane, said "not as per design", and there was
//! nothing in this directory a reviewer could have laid beside 8a to see what
//! they meant. The five are the form as it opens, the same form with edits in
//! it (the dirty state 8a is mostly about), the form at the pane's width at
//! `settings::MIN_VAULT_WINDOW_SIZE`, and the two block shots.
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
//! `check_breaches` is off).
//!
//! **The 4b preflight is no longer one of these surfaces.** It was, while it
//! was an egui window; it is bare Win32 now -- `deskwarden::preflight_card` --
//! and it has its own preview, `examples/preflight_preview.rs`, for the reason
//! the other Win32 cards do: this example walks egui surfaces through one
//! `run_native`, and there is no egui in that card to walk.
//!
//! Every surface renders the exact draw function the app ships, never a copy,
//! so what these PNGs show is what the real app shows.

use deskwarden::breach::BreachCache;
use deskwarden::hello::HelloState;
use deskwarden::login_ui::{self, BwStatus, LoginForm};
use deskwarden::local_time;
use deskwarden::receive_history::{ReceiveHistory, ReceivedRecord};
use deskwarden::send;
use deskwarden::vault_bridge::{Folder, VaultItem};
use deskwarden::vault_window::detail::{self, RevealState, TotpState};
use deskwarden::vault_window::detail_edit::{self, EditDraft};
use deskwarden::vault_window::item_list;
use deskwarden::vault_window::password_health;
use deskwarden::vault_window::sidebar::{self, SidebarFilter};
use deskwarden::vault_window::record_ui::{self, RecordDraft};
use deskwarden::vault_window::rehearsal;
use deskwarden::vault_window::send_ui;
use deskwarden::otpauth::OtpRefusal;
use deskwarden::vault_window::totp_add::{self, TotpAdd};
use deskwarden::{app_identity::AppIdentityCache, prefs_ui, scratch_window, theme};
use eframe::egui;

/// Set each frame from `--dark`, so the mock panels this harness draws
/// itself (the real window fills its list column with the palette) follow it.
static DARK_SHOTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The list column's ground in the palette the shot is taken in.
fn list_ground() -> egui::Color32 {
    if DARK_SHOTS.load(std::sync::atomic::Ordering::Relaxed) {
        deskwarden::kit::tokens::Palette::DARK.list_bg
    } else {
        deskwarden::kit::tokens::Palette::LIGHT.list_bg
    }
}
use std::path::PathBuf;

/// The instant every time-dependent preview is drawn at.
///
/// A literal, not `SystemTime::now()`: the one-time code shot's two most
/// prominent numbers are a code and a countdown, and a screenshot that changes
/// every run is one no reviewer can diff against the last.
const PREVIEW_UNIX: u64 = 1_699_999_980;

/// [`PREVIEW_UNIX`] as the milliseconds a [`send::SendClock`] speaks in.
///
/// The record composer's §5a Access block prints the day its link dies, and
/// this constant is that shot's half of the same rule: a screenshot carrying
/// today's date is one no reviewer can diff against the last.
const PREVIEW_MILLIS: i64 = PREVIEW_UNIX as i64 * 1_000;

/// The timezone every preview shot stands in.
///
/// UTC, injected, for [`PREVIEW_UNIX`]'s reason applied one step further out:
/// the day a Send expires on is the USER's day, so a shot taken with the
/// machine's own zone would name a different date depending on who ran it.
const PREVIEW_ZONE: local_time::FixedOffset = local_time::FixedOffset(0);

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
    // **No design 2a here.** The matched-item card is
    // `deskwarden::prompt_card` now -- bare Win32, no eframe anywhere, and
    // anchored beside the field it answers for -- and this walk drives egui
    // surfaces through one `run_native`. Its preview is
    // `examples/prompt_preview.rs`, beside `picker_preview`,
    // `generate_preview` and `unlock_prompt_preview` for the same reason.
    // **No design 3b here either.** The locked-vault card is
    // `deskwarden::locked_card` now, for the same reason and by the same
    // route; its preview is `examples/locked_preview.rs`.
    // **No design 3c here either, and it was the last one.** The
    // save-a-login card is `deskwarden::save_login_card` now -- bare Win32,
    // no eframe anywhere -- and `deskwarden::overlay_ui`, the module that held
    // every egui card the daemon showed, is gone with it. Its preview is
    // `examples/save_login_preview.rs`.
    // **No design 3d here.** The generator card is
    // `deskwarden::generate_prompt` now -- bare Win32, no eframe anywhere --
    // and this walk drives egui surfaces through one `run_native`. Its
    // preview is `examples/generate_preview.rs`, beside `picker_preview` and
    // `unlock_prompt_preview` for the same reason.
    /// The login window with a vault that exists and is locked.
    LoginUnlock,
    /// The login window with no account yet -- server dropdown and the Hello
    /// opt-in, self-hosted, which is the tallest state it has.
    LoginSignin,
    /// **The login window with the CLI-setup modal open**, at its first
    /// state ([`deskwarden::bw_acquire::CliSetupState::Asking`]).
    ///
    /// A surface of its own because the modal REPLACES the credential card:
    /// nothing of it is on `login_signin`, and the two shots share no widget
    /// below the subtitle. It shipped in 0.15.2 painting a heading, three
    /// body lines and two buttons -- and rendering as an empty panel with
    /// only the buttons on it, which every test in the suite agreed was
    /// correct because they all asserted on `title()` and `body()`, the two
    /// functions that were never the problem. This is the shot that shows
    /// the panel itself.
    LoginCliSetup,
    /// **The backend CHOICE modal**, which only a self-hosted sign-in
    /// meets: `CliSetupState::Choosing`. A surface of its own beside
    /// `login_cli_setup` because the two are different questions -- that one
    /// states a requirement, this one puts a fork -- and because this body is
    /// three paragraphs to that one's three, and it carries the app's blue
    /// primary button, which is 32px in a row the requirement modal fills
    /// with 18px ones -- between them, the length that decides whether the
    /// card still fits the window.
    LoginBackendChoice,
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
    /// **Design 8a's own subject: the Edit login form, as it opens.**
    ///
    /// The surface this file did not have. `EditWebsites` and `EditSparse`
    /// below each exist to show ONE block of this form, at a height chosen so
    /// that block is not below the fold -- neither of them is a picture of the
    /// form, and neither is drawn at a height the app ships. The owner
    /// screenshotted the whole pane and said "not as per design"; there was no
    /// rendered surface a reviewer could have put beside 8a to see what they
    /// meant.
    ///
    /// Untouched, because the report was about the form as it OPENS.
    EditLogin,
    /// **The same form with edits in it**, which is the half 8a is mostly
    /// about: the `Unsaved changes` state, the per-section `Changed` marks and
    /// the footer's change count only exist on a dirty draft, so a shot of a
    /// pristine form shows none of the design's dirty state.
    EditLoginDirty,
    /// **The same form at the pane's width at `MIN_VAULT_WINDOW_SIZE`**, which
    /// is [`EDIT_NARROW_WIDTH`] -- 298 points, not 638.
    ///
    /// Every card, every two-column split and every footer row on this form
    /// has to survive that width, and 8a is drawn at a 1240-point window where
    /// nothing has to. The `--sends` set learned this the expensive way and
    /// carries `SendsNarrow` for it; this is the same shot for the same
    /// reason.
    EditLoginNarrow,
    /// **The edit form's websites block, with three of them on it.**
    ///
    /// A surface of its own because the block's whole shape is the thing a
    /// list can get wrong and a one-entry shot cannot show: three numbered
    /// labels, a Remove under each box, the "first one counts" note that only
    /// appears once there IS more than one, and the Add under all of it. The
    /// form had no URI editor at all until this block existed -- the owner
    /// opened an item with a website on it and found no website on the form
    /// -- so this is the picture of the gap being closed.
    EditWebsites,
    /// **The same form with its password history open.**
    ///
    /// Design 8a's `Password history (3)`, which the form did not surface at
    /// all until this pass: `passwordHistory` has been decrypted on every sync
    /// for as long as `rest/sync.rs` has existed and the READ pane has drawn
    /// it, while the edit form -- whose own footer promises that saving will
    /// add the current password to a history -- showed nothing of it.
    ///
    /// A surface of its own because the control is shut by default, so
    /// [`Surface::EditLogin`] is a picture of one caption and a count. What
    /// has to be looked at is the open list: three dates, three masks, the
    /// read-only note under them, and whether all of that sitting under the
    /// generator still reads as part of the credentials card rather than as a
    /// panel that fell into it.
    EditLoginHistory,
    /// **A card item's edit form.** The kind whose body has the most optional
    /// rows -- cardholder name, both expiry boxes, the security code, the bank
    /// and the billing ZIP all carry a Remove -- and, until this pass, the
    /// clearest picture of the form not being consistent between its own
    /// cards: an `Item` card in §8a's label column directly above a `Card`
    /// card stacking label-above-field.
    ///
    /// There was no edit surface for any kind but a login and an identity, so
    /// the six-Remove case was never in a picture at all.
    EditCard,
    /// **The same card form at `settings::MIN_VAULT_WINDOW_SIZE`.**
    ///
    /// The row idiom has two arms -- §8a's label column above
    /// `theme::section_rows_fit`, the stacked caption below it -- and the
    /// Remove chip moves between them: in the column it sits under the
    /// caption, stacked it sits beside it. Only the pair of shots shows that
    /// both arms are a row a user can read.
    EditCardNarrow,
    /// **A sparse item's edit form**: an identity that has a name and an
    /// email and nothing else, so the form is four rows and an Add control
    /// rather than eighteen empty boxes.
    ///
    /// The picture is the argument. The shape being replaced -- every row
    /// drawn whether or not the item uses it -- reads perfectly reasonably in
    /// prose and is miserable on screen, and the only way to see that the
    /// replacement is better is to look at it.
    EditSparse,
    /// **The same item with the Add menu open**, which is the other half of
    /// that surface: the rows that are NOT on the form have to be reachable
    /// and legible as a list of names, and a chip row that wrapped into an
    /// unreadable block would satisfy every geometry assertion in the suite.
    EditSparseAdding,
    /// The v2 editor creating a login: no About, no Move to trash.
    EditNewLogin,
    /// The v2 editor with the avatar menu open.
    EditAvatarMenu,
    /// **Menus slice**: a form select (a website's match chooser) held open.
    MenuSelectOpen,
    /// **Menus slice**: an item row's context menu held open over the list.
    MenuContextOpen,
    /// **Menus slice**: a long list scrolled, with the thin floating bar the
    /// theme gives every `ScrollArea` drawn as it looks with the pointer over
    /// the list (a still has no pointer, so the idle opacity is lifted to it).
    ScrolledList,
    /// The v2 editor's sign-in steps, half way through a test.
    EditStepsTesting,
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
    /// **Design 6d as a user first meets it**: a bare base32 seed typed into
    /// the field, the green check and *"Valid base32 · 16 characters · spaces
    /// ignored"* under it, the two parameter runs live, and the live code
    /// below.
    ///
    /// [`Surface::TotpAddConfirm`] is not this picture. It pastes a whole
    /// URI, which takes the parameter controls out of the user's hands and
    /// puts the card into its 8/60/SHA-256 state -- the case worth catching,
    /// and not the case worth *looking* at. The by-hand route is the one 6d
    /// draws and the one every number on that panel was measured from, so it
    /// is the one that has to be laid beside it.
    TotpAddTyped,
    /// **The reported defect**: `sdf` in 6d's field.
    ///
    /// Three base32 characters are fifteen bits, which is not whole bytes, so
    /// the parser refuses -- correctly -- and the card has to say so in the
    /// one short line the design draws there. It said so in a three-line
    /// paragraph painted over the box above it. This is the picture that
    /// makes both halves of that reviewable, and the reason the seed is
    /// spelled `sdf` rather than something tidier is that `sdf` is what the
    /// owner typed.
    TotpAddRefused,
    /// **The LONGEST refusal the parser can produce**, which is this line's
    /// worst case and therefore the only width test that means anything.
    ///
    /// A short sentence fits whatever the layout does with it, so a picture
    /// of a short refusal reviews nothing: the defect above was invisible
    /// until a sentence arrived that had to wrap. The fixture asserts its own
    /// sentence is the longest [`totp_add::refusal_sentence`] renders (see
    /// `Fixtures::new`), so a refusal reworded longer than this one moves the
    /// picture with it instead of leaving it behind.
    TotpAddRefusedLongest,
    /// **Design 6c**: a payload a decoder read, confirmed rather than typed.
    ///
    /// The header's green check and *"Code read"*, the masked secret, the
    /// four parameter chips and the field table -- none of which is on the
    /// typed card, because a scanned URI must never be poured into a box
    /// that paints what it holds. The two are one card a step apart and the
    /// only way to see that they still read as one card is to look at them
    /// side by side.
    TotpAddScanned,
    /// **The whole card at `settings::MIN_VAULT_WINDOW_SIZE`**, 900x600.
    ///
    /// The card is 470 wide at every window size -- that is
    /// `stage_width(Stage::Manual)` and it does not move -- so what the floor
    /// decides is the card's HEIGHT: `draw_add_form` measures its header,
    /// its caution band and its footer against `content_rect().height()` and
    /// gives the body whatever is left, which at 600 is the case where the
    /// confirmation scrolls instead of growing. Drawn with the confirmation
    /// up, because an empty field would fit any window and answer nothing.
    TotpAddNarrow,
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
    /// **Downloaded, verified, and offering the restart.**
    ///
    /// The one stage of this flow that had no picture, and it is the stage the
    /// button the user pressed lives on. "Click restart to update - nothing
    /// happens" was reported against a surface no screenshot in this set
    /// showed, which is a poor place for the set to have a hole.
    PrefsUpdatesReady,
    /// A failed download, with the reason on the page and a retry beside it.
    /// The old flow's failure went to a tray tooltip, visible only to someone
    /// already hovering a 16px icon.
    PrefsUpdatesFailed,
    /// **The stage this page opens on, and the one with no picture until
    /// now.** Every other surface in this set shows the card after something
    /// has happened to it; this is what a user sees the moment they click
    /// Updates, which makes it the most-viewed state of the card and the last
    /// one anybody was reviewing.
    ///
    /// It earned a surface when the card's title was removed. `Idle` is where
    /// that removal is most visible -- the row is now a paragraph and a
    /// button with no heading over either -- so "does a card with no title
    /// still read as a card" is answered here or nowhere.
    PrefsUpdatesIdle,
    /// **Mid-check**, with the button disabled and labelled "Checking...".
    ///
    /// The other half of the pair above, and the reason the busy stages draw
    /// a disabled button instead of nothing: a row whose control vanishes
    /// reflows the card under the cursor. That is a claim about two frames,
    /// and this is the second of them.
    PrefsUpdatesChecking,
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
    /// **The Vault page with the official Bitwarden CLI chosen**, on a
    /// self-hosted account -- the only account where the choice is live.
    ///
    /// Its own surface because the page's whole shape depends on the answer:
    /// the two-cell picker at the top says which client, and the row about
    /// keeping `bw serve` running exists only on this side of it. That is a
    /// claim about a picture -- does the page read as one decision and its
    /// consequences? -- and no count of painted rectangles answers it.
    ///
    /// **The page had no preview at all until now**, which is how a settings
    /// screen ships with copy nobody has read at its real width. The sibling
    /// requirement modal shipped blank for exactly that reason.
    PrefsVaultOfficialCli,
    /// **The same page with Deskwarden's built-in client chosen**, and the
    /// picture the pair exists for: one row fewer, because
    /// `keep_backend_running` is about a subprocess this side does not have.
    ///
    /// Rendered beside its twin rather than instead of it, so the difference
    /// between the two states is something a reviewer can see rather than
    /// something they have to take on trust.
    PrefsVaultBuiltIn,
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
    /// **Design 5b's Sends screen, whole, with a Send picked.**
    ///
    /// The surface this example was missing for the whole of the Sends
    /// feature's life, and the reason the owner could say "Send UI is very
    /// basic - not even close to UI" about a screen every one of whose boxes
    /// had been measured correctly. Every pass before this one verified
    /// rectangles in isolation; nobody had put the screen beside 5b.
    ///
    /// It is drawn at the width the pane really gets -- the window minus the
    /// rail -- because the whole question 5b asks is whether the list column
    /// and the detail column read as ONE screen. A shot of either alone
    /// cannot show that, which is exactly how a list row drawn in a lighter
    /// face than the vault row it shares a column with survived this long.
    SendsSelected,
    /// The same screen with **nothing picked**: the list on its own beside
    /// the prompt. The state the screen opens in, and therefore the one that
    /// decides whether a user's eye goes to a row or to an empty column.
    SendsList,
    /// The account with **no Sends at all**. The empty case is a sentence,
    /// not a blank pane, and this is where whether it reads as one is looked
    /// at.
    SendsEmpty,
    /// **`Shared with me`**: the received-records screen, which shares this
    /// pane and has neither state pills nor a fetch behind it.
    SendsReceived,
    /// **The whole screen at `settings::MIN_VAULT_WINDOW_SIZE`.**
    ///
    /// 688x600 of pane, which leaves the detail column 298pt -- the width at
    /// which this screen has broken every time it has broken. It is a
    /// separate surface rather than a resize of [`Surface::SendsSelected`]
    /// because the two have to be laid side by side: the defect is never
    /// "something is missing", it is "the same pane reads differently", and
    /// only the pair shows that.
    SendsNarrow,
    /// The Send detail modal with its revoke question standing (spec B 7):
    /// Copy link and Extend held back, Cancel where Revoke was.
    SendsConfirm,
    /// A person share's modal (design 6c).
    SendsShare,
    /// The same modal with Stop sharing's question up.
    SendsShareStop,
    /// Shared with me with records from people above the imported Sends.
    SendsPeople,
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
    /// **The item list with NO DETAIL PANE BESIDE IT**, at the width it
    /// really gets in that state.
    ///
    /// A new state, and the only one this change creates: the list used to be
    /// [LIST_WIDTH] whatever was on screen, and now it runs to the right
    /// edge whenever nothing is selected. It is also the state the detail
    /// pane's slide starts from and returns to, so it is the one end of that
    /// animation a still picture CAN show -- the other end is
    /// [Surface::VaultList], which is unchanged.
    ///
    /// Drawn with nothing selected, because that is the only way the window
    /// ever reaches this width.
    VaultListWide,
    /// **One row of every KIND, with favicons of shapes a real site serves.**
    ///
    /// Two reports live on this surface and neither is answerable from
    /// [`Surface::VaultList`].
    ///
    /// * *"icons feels like not centered by start from top and cut of at the
    ///   bottom"*. The list shot's stand-in favicon is a hand-built 64px
    ///   SQUARE -- the one shape `favicon::decode_rgba` can never have to
    ///   change, and therefore the one shape that cannot show the defect.
    ///   These rows carry a 180x180 `apple-touch-icon`, a wide page-declared
    ///   wordmark and a tall one, each built as a FILE and taken through the
    ///   shipped decoder, and each drawn with a border hugging all four edges
    ///   so a lost bottom is visible rather than deducible.
    /// * *"we should also have different icons for SSH, notes, IDs"*. A note,
    ///   an identity, an SSH key and a card sit here beside a login, which is
    ///   the comparison the report is about: four marks that have to be
    ///   tellable from each other AND from the monogram a login still gets.
    VaultListKinds,
    /// **The v2 kit, every piece on one sheet**: both sidebars, the mark ramp,
    /// switches, segments, buttons, a card with tags, a help bubble, a hint
    /// and a toast. Slice 1 of the 1.0 redesign; held beside design 1a/1c.
    /// Every piece is drawn through `kit`'s own calls, so nothing here is a
    /// second drawing that could drift.
    KitSheet,
    /// **The translation system's glyph sheet**: a test catalog in English,
    /// Japanese, Simplified and Traditional Chinese, Korean, Russian, Greek and
    /// Vietnamese, each row set in the four weights the app uses, with plural
    /// forms picked by each language's own rule. It exists so the fonts every
    /// language needs -- system faces for CJK and Greek, the bundled ones for
    /// Cyrillic and Vietnamese -- can be LOOKED AT before a catalog is written.
    /// `--i18n --screenshot` writes it on its own.
    I18nFonts,
    /// **Design 1e's reference sheet**: every network and bank tile as the
    /// vault list draws it (44pt), the detail header's 72pt tiles with a
    /// network chip on a bank, and the text-fallback tiles beside them.
    /// Drawn through `kit::avatar::paint`, the call every row makes.
    CardLogos,
    /// **The v2 shell, whole**: the real rail (navy), the real top bar, and
    /// the 0.15 list and detail panes beneath it, at design 1a's 1280x820.
    VaultShellNavy,
    /// The same, with the light rail.
    VaultShellLight,
    /// The shell with the top bar's Settings gear menu held open (Shared,
    /// Password health, Generator, Settings).
    VaultShellGearMenu,
    /// The launch splash over the whole window (design 1a).
    VaultSplash,
    /// The v2 shell with a card selected: the card hero and its field rows.
    VaultShellCard,
    /// The v2 shell offline: the cached vault, a failed sync, the banner.
    VaultShellOffline,
    /// The New item chooser over the shell (spec B §5).
    ModalNewItem,
    /// The delete confirmation over the shell (slice 15c).
    ModalDelete,
    /// The folder editor over the shell.
    ModalFolder,
    /// The icon picker over the shell.
    ModalIcon,
    /// The Empty trash confirmation over the shell.
    ModalEmptyTrash,
    /// The forget-a-person confirmation over the shell.
    ModalForget,
    /// The people-sharing dialog (design 6a), asking, invited, and asking
    /// the organisation question.
    ModalShare,
    ModalShareInvited,
    ModalShareQuestion,
    /// The record import form, fetched and asking about a name collision.
    ModalRecordImport,
    /// The in-form password generator, and with its options open.
    ModalGenerator,
    ModalGeneratorOptions,
    /// The running-window picker, one row staged.
    ModalAppPicker,
    /// The fill-rule (sequence) builder.
    ModalSequence,
    /// The Add 2FA modal while the screen is being read.
    ModalTotpScanning,
    /// The Add 2FA modal asking which camera to use.
    ModalTotpCameras,
    /// The Import wizard's four steps over the shell (spec B §3).
    ModalImportSource,
    ModalImportFile,
    ModalImportProgress,
    ModalImportDone,
    /// The Generator screen (spec C §1).
    ToolGenerator,
    /// Settings as a screen (spec C §2), one shot per page worth seeing.
    SettingsGeneral,
    SettingsSecurity,
    SettingsVault,
    SettingsShortcuts,
    SettingsAutofill,
    SettingsApps,
    SettingsAppsKeys,
    SettingsGenerator,
    SettingsNotifications,
    SettingsImportExport,
    SettingsDevices,
    SettingsAccount,
    SettingsAbout,
    SettingsSecurityPrivacy,
    SettingsSecurityBreaches,
    SettingsVaultBackend,
    /// Settings > Vault with the `?` tip of the TOTP-secrets row open, the way
    /// the design shows it on hover.
    SettingsVaultTip,
    /// General with the Language menu open.
    SettingsLanguageMenu,
    /// **The read pane's header WITH a favicon**, which no shot has ever
    /// covered.
    ///
    /// [`Surface::LoginDetail`] draws the header's monogram fallback, and
    /// its own comment said why: loading an image would have meant a file
    /// to ship. [`shaped_favicon`] builds the file, so that reason is
    /// gone -- and the owner's report said "icons", plural, so the header
    /// has to be in a picture beside the row.
    ///
    /// The fixture's artwork has TRANSPARENT MARGINS, which is the shape
    /// most real favicons have and the one this surface exists for: the
    /// tile's fill is what a favicon shows through and its border is the
    /// edge that makes a pale one read as a tile at all.
    LoginDetailFavicon,
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
    /// **The "Choose a vault item" picker**, first step of the tray's Add
    /// app: the window's own body at its own size, one login selected.
    PickerItems,
    /// The same picker with a search that matches nothing.
    PickerItemsNoMatch,
    /// **The "Add app to Deskwarden" picker**, one process selected, backend
    /// ready.
    PickerApps,
    /// The same window with `bw serve` unanswered: the note and its Retry.
    PickerAppsBackendDown,
    /// The same window with a Store-app host process selected, which Save
    /// refuses and says why.
    PickerAppsRefused,
    /// **The picker's empty states**, which were message boxes: an empty vault,
    /// a vault with no logins, an unreachable backend, a locked vault.
    PickerNoticeEmpty,
    PickerNoticeNoLogins,
    PickerNoticeUnreachable,
    PickerNoticeLocked,
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
    /// The signed-out split screen with Self-hosted chosen and its URL field.
    LoginSigninSelfHosted,
    /// The signed-out split screen after a refused sign-in.
    LoginSigninError,
    /// The lock screen on its master-password stage.
    LoginUnlockPassword,
    /// The lock screen on its master-password stage after a wrong password.
    LoginUnlockPasswordError,
    /// The second-factor prompt, on the split screen.
    LoginSecondFactor,
    /// The code stage after a wrong code: six red boxes and the line under them.
    LoginSecondFactorWrong,
    /// The code stage for an emailed code, after Send code.
    LoginSecondFactorEmail,
    /// The code stage for a YubiKey (one field, not six boxes).
    LoginSecondFactorYubiKey,
    /// The code stage while the code is with the server.
    LoginSecondFactorBusy,
    /// The stage for an account whose only methods are ones Deskwarden cannot complete.
    LoginSecondFactorUnsupported,
    /// The API-key sign-in's master-password step.
    LoginApiKeyPassword,
    /// The API-key sign-in's first step after the server refused the key.
    LoginApiKeyError,
    /// The API-key sign-in while the server is answering.
    LoginApiKeyBusy,
    /// The API-key sign-in, on the split screen.
    LoginApiKey,
    /// The sign-in screen with the offline banner over the form (cloud server).
    LoginSigninOffline,
    /// The sign-in screen with the red "Can't reach your server" banner (Self-hosted).
    LoginSigninServerDown,
    /// The sign-in screen with the email box empty and red.
    LoginSigninEmailError,
    /// The sign-in screen with Self-hosted chosen and its address box empty and red.
    LoginSigninUrlError,
    /// The sign-in screen while the server is answering: greyed fields, "Checking" over its bar.
    LoginSigninBusy,
    /// The "Too many tries" stage.
    LoginSigninTooMany,
    /// The "You're in" stage.
    LoginSigninDone,
}

/// The detail pane's exact width in the shipped vault window.
///
/// `WINDOW_SIZE[0] - SIDEBAR_WIDTH - LIST_WIDTH`, i.e. `1240 - 212 - 390`.
/// Spelled out rather than imported because those three are `pub(crate)` in
/// `vault_window::mod` and an example is a separate crate. If that window is
/// ever resized, this is the number that has to follow it.
const PANE_WIDTH: f32 = 1280.0 - 252.0 - 380.0;

/// The item list panel's exact width in the shipped vault window, i.e.
/// `vault_window::mod`'s `LIST_WIDTH`. Spelled out here for the reason
/// [`PANE_WIDTH`] is spelled out.
const LIST_WIDTH: f32 = 380.0;

/// The rail's exact width in the shipped vault window, i.e.
/// `vault_window::mod`'s `SIDEBAR_WIDTH`.
const SIDEBAR_WIDTH: f32 = 252.0;

/// Tall enough that no pane below scrolls, so a screenshot is the whole
/// surface rather than the top of it. The shipped window is 740 high.
const PANE_HEIGHT: f32 = 740.0;

/// The detail pane's width at `settings::MIN_VAULT_WINDOW_SIZE`, which is
/// `(900, 600)`: `900 - 212 - 390 = 298`.
///
/// **The number design 8a never has to survive.** 8a is drawn on a 1240-point
/// window whose rail and card grid have 1028 points between them; the edit
/// form as this app actually ships it gets 298 at the floor, and every card,
/// every two-column split and every footer row on it has to hold there. The
/// `--sends` set carries `SENDS_NARROW_WIDTH` for exactly this and found real
/// defects with it; this is the same measurement for the same reason.
const EDIT_NARROW_WIDTH: f32 = 900.0 - 212.0 - 390.0;


/// The v2 editor's width in the wide window: the window less the rail.
const EDITOR_WIDTH: f32 = 1280.0 - 252.0;
/// The v2 editor at its floor.
const EDITOR_NARROW_WIDTH: f32 = 678.0;

/// The Sends screen's own width: the whole window less the rail.
///
/// **The pane, not the detail column.** `send_ui::draw_send_pane` puts its
/// own `Panel::left` at [`LIST_WIDTH`] inside whatever it is given and hangs
/// the detail column off the remainder, so it has to be handed the width the
/// `CentralPanel` in `vault_window::mod` hands it -- `1240 - 212`. Handed
/// [`PANE_WIDTH`] it would draw a 390pt list beside a 248pt detail, which is
/// a screen the app never shows.
const SENDS_WIDTH: f32 = 1240.0 - 212.0;

/// The same pane at `settings::MIN_VAULT_WINDOW_SIZE`, which is `(900, 600)`.
///
/// `900 - 212 = 688`, of which `draw_send_pane` gives 390 to the list and
/// **298 to the detail column** -- the width this screen has broken at every
/// time it has broken, and the one every new paint test on it is run at. A
/// picture of it belongs beside the comfortable one for the same reason the
/// two Vault-page shots belong beside each other: the thing under review is
/// the difference.
const SENDS_NARROW_WIDTH: f32 = 900.0 - 212.0;
const SENDS_NARROW_HEIGHT: f32 = 600.0;

/// The preferences window's body: its 1000x780 outer size less the 40pt
/// chrome bar `draw_window_chrome` paints above `draw_prefs_body`. Spelled out
/// rather than imported for the same reason [`PANE_WIDTH`] is -- an example is
/// a separate crate and `WINDOW_SIZE` is private to `prefs_ui`.
const PREFS_BODY_WIDTH: f32 = 1000.0;
const PREFS_BODY_HEIGHT: f32 = 780.0 - PREFS_CHROME_HEIGHT;
/// The titlebar's height, which is `ChromeMetrics::LOGIN`'s.
const PREFS_CHROME_HEIGHT: f32 = 40.0;

/// The detail pane's own frame, copied from the `CentralPanel` in
/// `vault_window::mod` that hosts it: `list_ground()` and **no margin**. The
/// app moved its `Margin::symmetric(20, 18)` to the Sends branch so the detail
/// and edit header strips reach the pane's edges (see that module's
/// `detail_pane_frame_placement_tests`); a margin here drew the edit form as a
/// card floating on grey that the app does not show.
fn pane_frame() -> egui::Frame {
    egui::Frame::new().fill(list_ground())
}

/// Every surface, in the order `--all` walks them.
const ALL: &[Surface] = &[
    Surface::LoginUnlock,
    Surface::LoginSignin,
    Surface::LoginCliSetup,
    Surface::LoginBackendChoice,
    Surface::LoginDetail,
    Surface::CardDetail,
    Surface::CardDetailRevealed,
    Surface::DiscardConfirm,
    Surface::EditLogin,
    Surface::EditLoginDirty,
    Surface::EditLoginNarrow,
    Surface::EditLoginHistory,
    Surface::EditCard,
    Surface::EditCardNarrow,
    Surface::EditWebsites,
    Surface::EditSparse,
    Surface::EditSparseAdding,
    Surface::EditNewLogin,
    Surface::EditAvatarMenu,
    Surface::MenuSelectOpen,
    Surface::MenuContextOpen,
    Surface::ScrolledList,
    Surface::EditStepsTesting,
    Surface::RecordComposer,
    Surface::TotpAddConfirm,
    Surface::TotpAddPicker,
    Surface::TotpAddTyped,
    Surface::TotpAddRefused,
    Surface::TotpAddRefusedLongest,
    Surface::TotpAddScanned,
    Surface::TotpAddNarrow,
    Surface::PrefsClipboard,
    Surface::PrefsClipboardOff,
    Surface::PrefsAbout,
    Surface::PrefsUpdatesNoUpdate,
    Surface::PrefsUpdatesAvailable,
    Surface::PrefsUpdatesShortNotes,
    Surface::PrefsUpdatesManyReleases,
    Surface::PrefsWindowChrome,
    Surface::PrefsUpdatesDownloading,
    Surface::PrefsUpdatesReady,
    Surface::PrefsUpdatesFailed,
    Surface::PrefsUpdatesIdle,
    Surface::PrefsUpdatesChecking,
    Surface::PrefsBreachesIdle,
    Surface::PrefsBreachesRunning,
    Surface::PrefsBreachesFailed,
    Surface::PrefsBreachesHistory,
    Surface::PrefsVaultOfficialCli,
    Surface::PrefsVaultBuiltIn,
    Surface::VaultList,
    Surface::VaultListWide,
    Surface::VaultListKinds,
    Surface::KitSheet,
    Surface::I18nFonts,
    Surface::CardLogos,
    Surface::VaultShellNavy,
    Surface::VaultShellLight,
    Surface::VaultShellGearMenu,
    Surface::VaultSplash,
    Surface::VaultShellCard,
    Surface::VaultShellOffline,
    Surface::ModalNewItem,
    Surface::ModalDelete,
    Surface::ModalFolder,
    Surface::ModalIcon,
    Surface::ModalEmptyTrash,
    Surface::ModalForget,
    Surface::ModalShare,
    Surface::ModalShareInvited,
    Surface::ModalShareQuestion,
    Surface::ModalRecordImport,
    Surface::ModalGenerator,
    Surface::ModalGeneratorOptions,
    Surface::ModalAppPicker,
    Surface::ModalSequence,
    Surface::ModalTotpScanning,
    Surface::ModalTotpCameras,
    Surface::ModalImportSource,
    Surface::ModalImportFile,
    Surface::ModalImportProgress,
    Surface::ModalImportDone,
    Surface::ToolGenerator,
    Surface::SettingsGeneral,
    Surface::SettingsSecurity,
    Surface::SettingsVault,
    Surface::SettingsShortcuts,
    Surface::SettingsAutofill,
    Surface::SettingsApps,
    Surface::SettingsAppsKeys,
    Surface::SettingsGenerator,
    Surface::SettingsNotifications,
    Surface::SettingsImportExport,
    Surface::SettingsDevices,
    Surface::SettingsAccount,
    Surface::SettingsAbout,
    Surface::SettingsSecurityPrivacy,
    Surface::SettingsSecurityBreaches,
    Surface::SettingsVaultBackend,
    Surface::SettingsVaultTip,
    Surface::SettingsLanguageMenu,
    Surface::LoginDetailFavicon,
    Surface::VaultRail,
    Surface::VaultHealth,
    Surface::VaultHealthBreached,
    Surface::VaultHealthReusedOnly,
    Surface::VaultHealthWeakOnly,
    Surface::VaultHealthBreachedOnly,
    Surface::VaultHealthEmptyFilter,
    Surface::SendsSelected,
    Surface::SendsList,
    Surface::SendsEmpty,
    Surface::SendsReceived,
    Surface::SendsNarrow,
    Surface::SendsConfirm,
    Surface::SendsShare,
    Surface::SendsShareStop,
    Surface::SendsPeople,
    Surface::Rehearsal,
    Surface::VaultSetupSpinner,
    Surface::PickerItems,
    Surface::PickerItemsNoMatch,
    Surface::PickerApps,
    Surface::PickerAppsBackendDown,
    Surface::PickerAppsRefused,
    Surface::PickerNoticeEmpty,
    Surface::PickerNoticeNoLogins,
    Surface::PickerNoticeUnreachable,
    Surface::PickerNoticeLocked,
    Surface::FirstWindowLoading,
    Surface::FirstWindowSlow,
    Surface::FirstWindowSlowLocalCopy,
    Surface::FirstWindowUnreachable,
    Surface::FirstWindowUnreachableLocalCopy,
    Surface::FirstWindowUnreachableSpent,
    Surface::FirstWindowUnreachableSpentLocalCopy,
    Surface::FirstWindowUnreachableDeclinedCopy,
    Surface::LoginUnlockBusy,
    Surface::LoginSigninSelfHosted,
    Surface::LoginSigninError,
    Surface::LoginUnlockPassword,
    Surface::LoginUnlockPasswordError,
    Surface::LoginSecondFactor,
    Surface::LoginApiKey,
    Surface::LoginSecondFactorWrong,
    Surface::LoginSecondFactorEmail,
    Surface::LoginSecondFactorYubiKey,
    Surface::LoginSecondFactorBusy,
    Surface::LoginSecondFactorUnsupported,
    Surface::LoginApiKeyPassword,
    Surface::LoginApiKeyError,
    Surface::LoginApiKeyBusy,
    Surface::LoginSigninOffline,
    Surface::LoginSigninServerDown,
    Surface::LoginSigninEmailError,
    Surface::LoginSigninUrlError,
    Surface::LoginSigninBusy,
    Surface::LoginSigninTooMany,
    Surface::LoginSigninDone,
];

impl Surface {
    /// The PNG's file stem. Stable, because a human reviewing an artifact
    /// looks for the same name every time.
    fn stem(self) -> &'static str {
        match self {
            Surface::LoginUnlock => "login_unlock",
            Surface::LoginSignin => "login_signin",
            Surface::LoginCliSetup => "login_cli_setup",
            Surface::LoginBackendChoice => "login_backend_choice",
            Surface::LoginDetail => "detail_login",
            Surface::CardDetail => "detail_card",
            Surface::CardDetailRevealed => "detail_card_revealed",
            Surface::DiscardConfirm => "edit_discard_confirm",
            Surface::EditLogin => "edit_login",
            Surface::EditLoginDirty => "edit_login_dirty",
            Surface::EditLoginNarrow => "edit_login_narrow",
            Surface::EditLoginHistory => "edit_login_history",
            Surface::EditCard => "edit_card",
            Surface::EditCardNarrow => "edit_card_narrow",
            Surface::EditWebsites => "edit_websites",
            Surface::EditSparse => "edit_sparse",
            Surface::EditSparseAdding => "edit_sparse_adding",
            Surface::EditNewLogin => "edit_new_login",
            Surface::EditAvatarMenu => "edit_avatar_menu",
            Surface::MenuSelectOpen => "menu_select_open",
            Surface::MenuContextOpen => "menu_context_open",
            Surface::ScrolledList => "scrolled_list",
            Surface::EditStepsTesting => "edit_steps_testing",
            Surface::RecordComposer => "record_composer",
            Surface::TotpAddConfirm => "totp_add_confirm",
            Surface::TotpAddPicker => "totp_add_picker",
            Surface::TotpAddTyped => "totp_add_typed",
            Surface::TotpAddRefused => "totp_add_refused",
            Surface::TotpAddRefusedLongest => "totp_add_refused_longest",
            Surface::TotpAddScanned => "totp_add_scanned",
            Surface::TotpAddNarrow => "totp_add_narrow",
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
            Surface::PrefsUpdatesReady => "prefs_updates_ready",
            Surface::PrefsUpdatesFailed => "prefs_updates_failed",
            Surface::PrefsUpdatesIdle => "prefs_updates_idle",
            Surface::PrefsUpdatesChecking => "prefs_updates_checking",
            Surface::PrefsBreachesIdle => "prefs_breaches_idle",
            Surface::PrefsBreachesRunning => "prefs_breaches_running",
            Surface::PrefsBreachesFailed => "prefs_breaches_failed",
            Surface::PrefsBreachesHistory => "prefs_breaches_history",
            Surface::PrefsVaultOfficialCli => "prefs_vault_official_cli",
            Surface::PrefsVaultBuiltIn => "prefs_vault_built_in",
            Surface::VaultList => "vault_item_list",
            Surface::VaultListWide => "vault_item_list_wide",
            Surface::VaultListKinds => "vault_item_list_kinds",
            Surface::KitSheet => "kit_sheet",
            Surface::I18nFonts => "i18n_fonts",
            Surface::CardLogos => "card_logos",
            Surface::VaultShellNavy => "vault_shell_navy",
            Surface::VaultShellLight => "vault_shell_light",
            Surface::VaultShellGearMenu => "vault_shell_gear_menu",
            Surface::VaultSplash => "vault_splash",
            Surface::VaultShellCard => "vault_shell_card",
            Surface::VaultShellOffline => "vault_shell_offline",
            Surface::ModalNewItem => "modal_new_item",
            Surface::ModalDelete => "modal_delete",
            Surface::ModalFolder => "modal_folder_edit",
            Surface::ModalIcon => "modal_icon_pick",
            Surface::ModalEmptyTrash => "modal_empty_trash",
            Surface::ModalForget => "modal_forget",
            Surface::ModalShare => "modal_share",
            Surface::ModalShareInvited => "modal_share_invited",
            Surface::ModalShareQuestion => "modal_share_question",
            Surface::ModalRecordImport => "modal_record_import",
            Surface::ModalGenerator => "modal_password_generator",
            Surface::ModalGeneratorOptions => "modal_password_generator_options",
            Surface::ModalAppPicker => "modal_app_window_picker",
            Surface::ModalSequence => "modal_sequence_builder",
            Surface::ModalTotpScanning => "totp_add_scanning",
            Surface::ModalTotpCameras => "totp_add_cameras",
            Surface::ModalImportSource => "modal_import_source",
            Surface::ModalImportFile => "modal_import_file",
            Surface::ModalImportProgress => "modal_import_progress",
            Surface::ModalImportDone => "modal_import_done",
            Surface::ToolGenerator => "tool_generator",
            Surface::SettingsGeneral => "settings_general",
            Surface::SettingsSecurity => "settings_security",
            Surface::SettingsVault => "settings_vault",
            Surface::SettingsShortcuts => "settings_shortcuts",
            Surface::SettingsAutofill => "settings_autofill",
            Surface::SettingsApps => "settings_apps",
            Surface::SettingsAppsKeys => "settings_apps_keys",
            Surface::SettingsGenerator => "settings_generator",
            Surface::SettingsNotifications => "settings_notifications",
            Surface::SettingsImportExport => "settings_import_export",
            Surface::SettingsDevices => "settings_devices",
            Surface::SettingsAccount => "settings_account",
            Surface::SettingsAbout => "settings_about",
            Surface::SettingsSecurityPrivacy => "settings_security_privacy",
            Surface::SettingsSecurityBreaches => "settings_security_breaches",
            Surface::SettingsVaultBackend => "settings_vault_backend",
            Surface::SettingsVaultTip => "settings_vault_tip",
            Surface::SettingsLanguageMenu => "settings_language_menu",
            Surface::LoginDetailFavicon => "detail_login_favicon",
            Surface::VaultRail => "vault_rail",
            Surface::VaultHealth => "vault_password_health",
            Surface::VaultHealthBreached => "vault_password_health_breached",
            Surface::VaultHealthReusedOnly => "vault_password_health_reused",
            Surface::VaultHealthWeakOnly => "vault_password_health_weak",
            Surface::VaultHealthBreachedOnly => "vault_password_health_breached_only",
            Surface::VaultHealthEmptyFilter => "vault_password_health_empty_filter",
            Surface::SendsSelected => "sends_selected",
            Surface::SendsList => "sends_list",
            Surface::SendsEmpty => "sends_empty",
            Surface::SendsReceived => "sends_received",
            Surface::SendsNarrow => "sends_narrow",
            Surface::SendsConfirm => "sends_confirm",
            Surface::SendsShare => "sends_share",
            Surface::SendsShareStop => "sends_share_stop",
            Surface::SendsPeople => "sends_people",
            Surface::Rehearsal => "rehearsal",
            Surface::VaultSetupSpinner => "vault_setup_spinner",
            Surface::PickerItems => "picker_items",
            Surface::PickerItemsNoMatch => "picker_items_no_match",
            Surface::PickerApps => "picker_apps",
            Surface::PickerAppsBackendDown => "picker_apps_backend_down",
            Surface::PickerAppsRefused => "picker_apps_refused",
            Surface::PickerNoticeEmpty => "picker_notice_empty",
            Surface::PickerNoticeNoLogins => "picker_notice_no_logins",
            Surface::PickerNoticeUnreachable => "picker_notice_unreachable",
            Surface::PickerNoticeLocked => "picker_notice_locked",
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
            Surface::LoginSigninSelfHosted => "login_signin_self_hosted",
            Surface::LoginSigninError => "login_signin_error",
            Surface::LoginUnlockPassword => "login_unlock_password",
            Surface::LoginUnlockPasswordError => "login_unlock_password_error",
            Surface::LoginSecondFactor => "login_second_factor",
            Surface::LoginApiKey => "login_api_key",
            Surface::LoginSecondFactorWrong => "login_second_factor_wrong",
            Surface::LoginSecondFactorEmail => "login_second_factor_email",
            Surface::LoginSecondFactorYubiKey => "login_second_factor_yubikey",
            Surface::LoginSecondFactorBusy => "login_second_factor_busy",
            Surface::LoginSecondFactorUnsupported => "login_second_factor_unsupported",
            Surface::LoginApiKeyPassword => "login_api_key_password",
            Surface::LoginApiKeyError => "login_api_key_error",
            Surface::LoginApiKeyBusy => "login_api_key_busy",
            Surface::LoginSigninOffline => "login_signin_offline",
            Surface::LoginSigninServerDown => "login_signin_server_down",
            Surface::LoginSigninEmailError => "login_signin_email_error",
            Surface::LoginSigninUrlError => "login_signin_url_error",
            Surface::LoginSigninBusy => "login_signin_busy",
            Surface::LoginSigninTooMany => "login_signin_too_many",
            Surface::LoginSigninDone => "login_signin_done",
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
            Surface::LoginUnlock
            | Surface::LoginSignin
            | Surface::LoginCliSetup
            | Surface::LoginBackendChoice
            | Surface::LoginUnlockBusy
            | Surface::LoginSigninSelfHosted
            | Surface::LoginSigninError
            | Surface::LoginUnlockPassword
            | Surface::LoginUnlockPasswordError
            | Surface::LoginSecondFactor
            | Surface::LoginSigninOffline
            | Surface::LoginSigninServerDown
            | Surface::LoginSigninEmailError
            | Surface::LoginSigninUrlError
            | Surface::LoginSigninBusy
            | Surface::LoginSigninTooMany
            | Surface::LoginSigninDone
            | Surface::LoginSecondFactorWrong
            | Surface::LoginSecondFactorEmail
            | Surface::LoginSecondFactorYubiKey
            | Surface::LoginSecondFactorBusy
            | Surface::LoginSecondFactorUnsupported
            | Surface::LoginApiKeyPassword
            | Surface::LoginApiKeyError
            | Surface::LoginApiKeyBusy
            | Surface::LoginApiKey => egui::vec2(1280.0, 820.0),
            // Wide enough for the design's own 260px track with room
            // either side, and tall enough for six of them stacked with
            // their labels.
            Surface::LoginDetail
            | Surface::LoginDetailFavicon
            | Surface::CardDetail
            | Surface::CardDetailRevealed
            | Surface::DiscardConfirm
            | Surface::TotpAddConfirm
            | Surface::TotpAddPicker
            | Surface::TotpAddTyped
            | Surface::TotpAddRefused
            | Surface::TotpAddRefusedLongest
            | Surface::TotpAddScanned => egui::vec2(PANE_WIDTH, 900.0),
            // **The shipped window's floor, not a pane's width.** The
            // add-a-code card is a modal over the whole vault window rather
            // than a pane inside it, and the number it reads is
            // `content_rect().height()` -- so the surface that reviews it at
            // the floor has to BE the floor, `settings::MIN_VAULT_WINDOW_SIZE`
            // to the point. Spelled out for [`PANE_WIDTH`]'s reason.
            Surface::TotpAddNarrow => egui::vec2(900.0, 600.0),
            // Taller than the shipped pane on purpose. A login carrying three
            // websites is a longer form than 740pt holds, and the shipped
            // pane's answer -- scroll it -- is right for the app and useless
            // for a screenshot: the block this surface exists to show would
            // be below the fold in the PNG. The width is the real one, which
            // is the axis a layout can get wrong.
            // **The shipped pane, to the point, and nothing taller.** This is
            // the surface the owner's own screenshot is of, so the one thing
            // it must not do is show more of the form than the app does: a
            // preview stretched until everything fits answers "does it all
            // fit" with a yes the user never sees. What falls below the fold
            // here falls below the fold there.
            // v2: the editor is the window under the top bar, less the rail.
            Surface::EditLogin | Surface::EditLoginDirty => egui::vec2(EDITOR_WIDTH, 820.0),
            // The editor's floor (window floor less the rail): one column.
            Surface::EditLoginNarrow => egui::vec2(EDITOR_NARROW_WIDTH, 820.0),
            // **Taller than the shipped pane, for `EditWebsites`' stated
            // reason**: the list this surface exists to show sits under the
            // password box on the SECOND card, and at the pane's own 740 it is
            // below the fold. The width is the real one, which is the axis the
            // layout can get wrong.
            Surface::EditLoginHistory => egui::vec2(PANE_WIDTH, 1000.0),
            // A card has six rows that carry a Remove and three that do not,
            // and the whole point of the shot is the nine of them in one
            // column -- so it gets the height that holds them rather than the
            // pane's, for `EditWebsites`' reason.
            Surface::EditCard => egui::vec2(PANE_WIDTH, 1100.0),
            // The narrow arm at the window floor's real width. Taller than the
            // floor's 600 on purpose: stacked, every row costs a caption line
            // as well as a box, so the pane the app ships SCROLLS here -- and
            // a screenshot of the top third of a scrolled form shows two rows
            // and answers nothing about the other seven.
            Surface::EditCardNarrow => egui::vec2(EDIT_NARROW_WIDTH, 1300.0),
            Surface::EditWebsites => egui::vec2(PANE_WIDTH, 1180.0),
            // The shipped pane's own size, and that is the point of these
            // two: a sparse identity FITS it, which the eighteen-row form it
            // replaces did not. A taller preview would hide the very thing
            // being shown. The Add menu adds a wrapped chip row, so the open
            // state gets a little more room rather than a scroll bar.
            // Taller than the shipped pane, for [`Surface::EditWebsites`]'s
            // reason and with a sharper edge to it: this card grew a FOOTER
            // BAND, and a footer is the one part of a form whose whole job is
            // to be the last thing -- a screenshot that stops above it is a
            // picture of the card with the reviewed part cropped off. At 740
            // it did exactly that, through the pass that added the band.
            Surface::RecordComposer => egui::vec2(PANE_WIDTH, 900.0),
            Surface::EditSparse => egui::vec2(PANE_WIDTH, PANE_HEIGHT),
            Surface::EditSparseAdding => egui::vec2(PANE_WIDTH, 900.0),
            Surface::MenuSelectOpen => egui::vec2(PANE_WIDTH, 1180.0),
            Surface::MenuContextOpen => egui::vec2(LIST_WIDTH + 260.0, PANE_HEIGHT),
            Surface::ScrolledList => egui::vec2(420.0, 520.0),
            // The v2 editor takes the window under the top bar: the wide
            // window's width less the rail, tall enough for the two columns.
            Surface::EditNewLogin | Surface::EditAvatarMenu | Surface::EditStepsTesting => {
                egui::vec2(EDITOR_WIDTH, 820.0)
            }
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
            | Surface::PrefsUpdatesReady
            | Surface::PrefsUpdatesFailed
            | Surface::PrefsUpdatesIdle
            | Surface::PrefsUpdatesChecking
            | Surface::PrefsBreachesIdle
            | Surface::PrefsBreachesRunning
            | Surface::PrefsBreachesFailed
            | Surface::PrefsBreachesHistory
            | Surface::PrefsVaultOfficialCli
            | Surface::PrefsVaultBuiltIn => egui::vec2(PREFS_BODY_WIDTH, PREFS_BODY_HEIGHT),
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
            // The window minus its sidebar: what the list is given when no
            // detail pane is on screen.
            Surface::VaultListWide => {
                egui::vec2(LIST_WIDTH + PANE_WIDTH, PANE_HEIGHT)
            }
            // The list's own column, because these are list rows. Shorter
            // than [`PANE_HEIGHT`] only because the fixture is: nine rows
            // fit, and a shot with empty canvas under them wastes the
            // reviewer's screen rather than telling them anything.
            Surface::VaultListKinds => egui::vec2(LIST_WIDTH, 620.0),
            // A sheet, not a pane: its size is what the sheet needs.
            Surface::KitSheet => egui::vec2(1280.0, 900.0),
            Surface::I18nFonts => egui::vec2(1280.0, 980.0),
            Surface::CardLogos => egui::vec2(1000.0, 520.0),
            Surface::VaultShellNavy
            | Surface::VaultShellLight
            | Surface::VaultShellGearMenu
            | Surface::VaultSplash
            | Surface::VaultShellCard
            | Surface::VaultShellOffline
            | Surface::ModalNewItem
            | Surface::ModalImportSource
            | Surface::ModalImportFile
            | Surface::ModalImportProgress
            | Surface::ModalImportDone
            | Surface::ModalDelete
            | Surface::ModalFolder
            | Surface::ModalIcon
            | Surface::ModalEmptyTrash
            | Surface::ModalForget
            | Surface::ModalShare
            | Surface::ModalShareInvited
            | Surface::ModalShareQuestion
            | Surface::ModalRecordImport
            | Surface::ModalGenerator
            | Surface::ModalGeneratorOptions
            | Surface::ModalAppPicker
            | Surface::ModalSequence
            | Surface::ModalTotpScanning
            | Surface::ModalTotpCameras => egui::vec2(1280.0, 820.0),
            Surface::ToolGenerator
            | Surface::SettingsGeneral
            | Surface::SettingsSecurity
            | Surface::SettingsVault
            | Surface::SettingsShortcuts
            | Surface::SettingsAutofill
            | Surface::SettingsApps
            | Surface::SettingsAppsKeys
            | Surface::SettingsGenerator
            | Surface::SettingsNotifications
            | Surface::SettingsImportExport
            | Surface::SettingsDevices
            | Surface::SettingsAccount
            | Surface::SettingsAbout
            | Surface::SettingsSecurityPrivacy
            | Surface::SettingsSecurityBreaches
            | Surface::SettingsVaultBackend
            | Surface::SettingsVaultTip
            | Surface::SettingsLanguageMenu => egui::vec2(1280.0 - 252.0, 820.0),
            // `vault_window::mod`'s `SIDEBAR_WIDTH`, spelled out for the same
            // reason as the two above, and the shipped window's own height --
            // the rail is measured against the window's floor, so a preview
            // drawn taller would hide exactly the overflow it is here to show.
            Surface::VaultRail => egui::vec2(SIDEBAR_WIDTH, PANE_HEIGHT),
            // A whole-body page now (spec A 4), so the Shared page's size.
            Surface::VaultHealth
            | Surface::VaultHealthBreached
            | Surface::VaultHealthReusedOnly
            | Surface::VaultHealthWeakOnly
            | Surface::VaultHealthBreachedOnly
            | Surface::VaultHealthEmptyFilter => egui::vec2(SENDS_WIDTH, PANE_HEIGHT),
            // The Sends screen is TWO columns and is only worth looking at as
            // one picture; see [`SENDS_WIDTH`].
            Surface::SendsSelected
            | Surface::SendsList
            | Surface::SendsEmpty
            | Surface::SendsConfirm
            | Surface::SendsShare
            | Surface::SendsShareStop
            | Surface::SendsPeople
            | Surface::SendsReceived => egui::vec2(SENDS_WIDTH, PANE_HEIGHT),
            Surface::SendsNarrow => egui::vec2(SENDS_NARROW_WIDTH, SENDS_NARROW_HEIGHT),
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
            // The pickers' own windows, at the sizes `picker_ui` opens them in.
            Surface::PickerItems | Surface::PickerItemsNoMatch => {
                egui::Vec2::from(deskwarden::picker_ui::ITEM_WINDOW)
            }
            Surface::PickerApps | Surface::PickerAppsBackendDown | Surface::PickerAppsRefused => {
                egui::Vec2::from(deskwarden::picker_ui::APP_WINDOW)
            }
            Surface::PickerNoticeEmpty
            | Surface::PickerNoticeNoLogins
            | Surface::PickerNoticeUnreachable
            | Surface::PickerNoticeLocked => egui::Vec2::from(deskwarden::picker_ui::NOTICE_WINDOW),
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
            Surface::LoginUnlock
                | Surface::LoginSignin
                | Surface::LoginCliSetup
                | Surface::LoginBackendChoice
                | Surface::LoginUnlockBusy
                | Surface::LoginSigninSelfHosted
                | Surface::LoginSigninError
                | Surface::LoginUnlockPassword
                | Surface::LoginUnlockPasswordError
                | Surface::LoginSecondFactor
                | Surface::LoginSigninOffline
                | Surface::LoginSigninServerDown
                | Surface::LoginSigninEmailError
                | Surface::LoginSigninUrlError
                | Surface::LoginSigninBusy
                | Surface::LoginSigninTooMany
                | Surface::LoginSigninDone
                | Surface::LoginSecondFactorWrong
                | Surface::LoginSecondFactorEmail
                | Surface::LoginSecondFactorYubiKey
                | Surface::LoginSecondFactorBusy
                | Surface::LoginSecondFactorUnsupported
                | Surface::LoginApiKeyPassword
                | Surface::LoginApiKeyError
                | Surface::LoginApiKeyBusy
                | Surface::LoginApiKey
        )
    }
}

/// The person the Sends previews share a record with.
const SHARE_EMAIL: &str = "anna@example.com";

fn main() -> eframe::Result {
    let arg = |name: &str| std::env::args().any(|a| a == name);
    // `--lang <tag>`: draw every surface in that language, so a translator
    // can see where a longer sentence wraps or overflows. With `--all` the
    // shots go to `ui_preview_<tag>/` beside the English ones.
    let lang: Option<String> = {
        let args: Vec<String> = std::env::args().collect();
        args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1).cloned())
    };
    if let Some(tag) = lang.as_deref() {
        assert!(deskwarden::i18n::set_language(tag), "no catalog for language {tag}");
    }
    // The sign-in, unlock, first-window and second-factor screens only.
    let auth = arg("--auth");
    let all = arg("--all") || auth;
    let screenshot = all || arg("--screenshot") || arg("--i18n");
    let signin = arg("--signin");
    let cli_setup = arg("--cli-setup");
    // The self-hosted sign-in's backend choice. Its own flag, because it is
    // its own question and a reviewer asks to see it on its own.
    let backend_choice = arg("--backend-choice");
    // Preferences' Vault page, in both of its backend-picker states. One
    // flag for the pair, exactly as `--backend-choice` is one flag for the
    // two shapes of the modal it names: the two pictures are only useful
    // beside each other, because what they are for is the difference.
    let vault = arg("--vault");
    let login = signin || cli_setup || backend_choice || arg("--login");
    let list = arg("--list");
    // The kind marks and the odd-shaped favicons, together: the two
    // reports they answer were filed together and are looked at together.
    let kinds = arg("--kinds");
    let kit = arg("--kit");
    // `--dark`: every surface painted with the dark palette (what a window
    // opened under Settings > Theme > Dark gets), shots in `<dir>_dark`.
    let dark = arg("--dark");
    // The translation system's glyph sheet: every script the app draws.
    let i18n_sheet = arg("--i18n");
    let shell = arg("--shell");
    let rail = arg("--rail");
    let health = arg("--health");
    // Design 5b's Sends screen, in every state that has a different SHAPE:
    // a Send picked, nothing picked, the composer up, an account with none,
    // `Shared with me`, and the whole thing at the app's minimum size. One
    // flag for the six, exactly as `--vault` is one flag for a pair and
    // `--kinds` is one for a trio -- what is under review is the set.
    let sends = arg("--sends");
    // The "Add app" pickers and their empty states (nine states of two windows),
    // and the chrome / loading / rehearsal screens repainted with them.
    let pickers = arg("--pickers");
    // Designs 6a, 6c and 6d -- the add-a-code card, which had NO preview at
    // all until this flag and could only be looked at by running the app and
    // typing into it. One flag for the seven states for `--sends`' reason,
    // and more sharply than for any other set here: the line under the field
    // is a refusal in one shot and an acceptance in the next, and whether it
    // sits where the design puts it is a question about the pair.
    let totp = arg("--totp");
    // **Design 8a's Edit login form**, which had no flag of its own until this
    // one and could only be looked at by running the app and clicking Edit.
    // The owner screenshotted it and said "not as per design"; there was no
    // rendered picture to put beside 8a.
    //
    // Five states rather than one, for `--sends`' reason: pristine, dirty, at
    // the window floor, the websites block, and the sparse form. What is under
    // review is the SET -- the dirty state only exists on one of them, the
    // floor is where the grid has to survive, and the two block shots are the
    // parts of the form a one-shot picture crops.
    let edit = arg("--edit");

    // `--all` walks the whole list; otherwise the single surface the flags
    // name, exactly as this example has always behaved.
    let queue: Vec<Surface> = if auth {
        ALL.iter()
            .copied()
            .filter(|s| s.stem().starts_with("login_") || s.stem().starts_with("first_window_"))
            .collect()
    } else if all {
        ALL.to_vec()
    } else if cli_setup {
        vec![Surface::LoginCliSetup]
    } else if backend_choice {
        vec![Surface::LoginBackendChoice]
    } else if vault {
        vec![Surface::PrefsVaultOfficialCli, Surface::PrefsVaultBuiltIn]
    } else if signin {
        vec![Surface::LoginSignin]
    } else if login {
        vec![Surface::LoginUnlock]
    } else if kinds {
        vec![Surface::VaultListKinds, Surface::LoginDetailFavicon]
    } else if kit {
        vec![Surface::KitSheet]
    } else if i18n_sheet {
        vec![Surface::I18nFonts]
    } else if shell {
        vec![
            Surface::VaultShellNavy,
            Surface::VaultShellLight,
            Surface::VaultShellGearMenu,
            Surface::VaultSplash,
            Surface::VaultShellCard,
            Surface::VaultShellOffline,
            Surface::ModalNewItem,
            Surface::ModalDelete,
            Surface::ModalFolder,
            Surface::ModalIcon,
            Surface::ModalEmptyTrash,
            Surface::ModalForget,
            Surface::ModalShare,
            Surface::ModalShareInvited,
            Surface::ModalShareQuestion,
            Surface::ModalRecordImport,
            Surface::ModalGenerator,
            Surface::ModalGeneratorOptions,
            Surface::ModalAppPicker,
            Surface::ModalSequence,
            Surface::ModalTotpScanning,
            Surface::ModalTotpCameras,
            Surface::ModalImportSource,
            Surface::ModalImportFile,
            Surface::ModalImportProgress,
            Surface::ModalImportDone,
            Surface::ToolGenerator,
            Surface::SettingsGeneral,
            Surface::SettingsSecurity,
            Surface::SettingsVault,
            Surface::SettingsShortcuts,
            Surface::SettingsAutofill,
            Surface::SettingsApps,
            Surface::SettingsAppsKeys,
            Surface::SettingsGenerator,
            Surface::SettingsNotifications,
            Surface::SettingsImportExport,
            Surface::SettingsDevices,
            Surface::SettingsAccount,
            Surface::SettingsAbout,
            Surface::SettingsSecurityPrivacy,
            Surface::SettingsSecurityBreaches,
            Surface::SettingsVaultBackend,
            Surface::SettingsVaultTip,
            Surface::SettingsLanguageMenu,
        ]
    } else if list {
        vec![Surface::VaultList]
    } else if rail {
        vec![Surface::VaultRail]
    } else if health {
        vec![Surface::VaultHealth]
    } else if pickers {
        vec![
            Surface::PickerItems,
            Surface::PickerItemsNoMatch,
            Surface::PickerApps,
            Surface::PickerAppsBackendDown,
            Surface::PickerAppsRefused,
            Surface::PickerNoticeEmpty,
            Surface::PickerNoticeNoLogins,
            Surface::PickerNoticeUnreachable,
            Surface::PickerNoticeLocked,
            // The other screens this slice repainted: the shared window chrome
            // (sign-in, the vault-sized spinner and first window) and the
            // rehearsal window.
            Surface::LoginSignin,
            Surface::VaultSetupSpinner,
            Surface::FirstWindowLoading,
            Surface::FirstWindowUnreachable,
            Surface::Rehearsal,
        ]
    } else if sends {
        vec![
            Surface::SendsSelected,
            Surface::SendsList,
            Surface::SendsEmpty,
            Surface::SendsReceived,
            Surface::SendsNarrow,
            Surface::SendsConfirm,
            Surface::SendsShare,
            Surface::SendsShareStop,
            Surface::SendsPeople,
        ]
    } else if edit {
        // Pristine first, because that is the picture the report was about;
        // then the dirty state beside it, then the floor, then the two blocks
        // a single shot of a 740-point pane cannot hold.
        vec![
            Surface::EditLogin,
            Surface::EditLoginDirty,
            Surface::EditLoginNarrow,
            Surface::EditLoginHistory,
            Surface::EditCard,
            Surface::EditCardNarrow,
            Surface::EditWebsites,
            Surface::EditSparse,
            Surface::EditNewLogin,
            Surface::EditAvatarMenu,
            Surface::EditStepsTesting,
        ]
    } else if totp {
        // 6a first, because it is the door; then 6d in the three states of
        // its one line -- accepted, refused, and refused at its worst -- then
        // 6c, then the fused confirmation, then the whole card at the floor.
        // The order is the order a reviewer walks them in.
        vec![
            Surface::TotpAddPicker,
            Surface::TotpAddTyped,
            Surface::TotpAddRefused,
            Surface::TotpAddRefusedLongest,
            Surface::TotpAddScanned,
            Surface::TotpAddConfirm,
            Surface::TotpAddNarrow,
        ]
    } else {
        vec![Surface::LoginUnlock]
    };
    let first = queue[0];

    // Transparent and undecorated for every surface: the overlay needs it for
    // its rounded corners, and the login window draws its own titlebar. The panes are drawn on their own opaque CANVAS frame
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
        match lang.as_deref() {
            Some(tag) => target_dir().join(format!("ui_preview_{tag}")),
            None => target_dir().join("ui_preview"),
        }
    } else if cli_setup {
        target_dir().join("ui_preview_cli_setup.png")
    } else if backend_choice {
        // A FILE again. It was a directory while this flag rendered two
        // surfaces -- the modal a new account met and the differently worded
        // one a returning account met. The modal is the same every time now,
        // so a second shot would be a duplicate of the first.
        target_dir().join("ui_preview_backend_choice.png")
    } else if vault {
        // A DIRECTORY: this flag renders the Vault page in both of its
        // picker states, and the whole point of the pair is that they can be
        // laid side by side.
        target_dir().join("ui_preview_vault")
    } else if signin {
        target_dir().join("ui_preview_signin.png")
    } else if login {
        target_dir().join("ui_preview_login.png")
    } else if kinds {
        // A DIRECTORY: this flag renders the rows and the mark sheet, and the
        // pair is only useful together.
        target_dir().join("ui_preview_kinds")
    } else if kit {
        target_dir().join("ui_preview_kit.png")
    } else if i18n_sheet {
        target_dir().join("ui_preview_i18n.png")
    } else if shell {
        // A DIRECTORY: navy, light and the splash belong side by side.
        target_dir().join("ui_preview_shell")
    } else if list {
        target_dir().join("ui_preview_vault_item_list.png")
    } else if rail {
        target_dir().join("ui_preview_vault_rail.png")
    } else if health {
        target_dir().join("ui_preview_vault_password_health.png")
    } else if pickers {
        target_dir().join("ui_preview_pickers")
    } else if sends {
        // A DIRECTORY, for `--vault`'s and `--kinds`' reason: this flag
        // renders six states of one screen and the whole value of it is that
        // they can be laid out together.
        target_dir().join("ui_preview_sends")
    } else if edit {
        // A DIRECTORY, for the same reason: five states of one form.
        target_dir().join("ui_preview_edit")
    } else if totp {
        // A DIRECTORY, for the same reason: seven states of one card.
        target_dir().join("ui_preview_totp")
    } else {
        target_dir().join("ui_preview_login.png")
    };

    let out = if dark {
        let name = out.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let named = match out.extension() {
            Some(ext) => format!("{name}_dark.{}", ext.to_string_lossy()),
            None => format!("{name}_dark"),
        };
        out.with_file_name(named)
    } else {
        out
    };
    // Cloned before the closure takes it: the count check below outlives
    // the run.
    let out_dir = out.clone();
    let outcome = eframe::run_native(
        "Deskwarden preview",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Preview {
                dark,
                corners: deskwarden::window_host::CornerMask::new(),
                queue,
                at: 0,
                directory: all || vault || kinds || sends || pickers || totp || edit || shell,
                out,
                form: LoginForm::default(),
                // The app name a real 3c card would have been pre-filled with,
                // and nothing else: the other three rows are what this app can
                // actually know about the window, which is nothing.
                    screenshot,
                done: false,
                frames: 0,
                warm_since: 0.0,
                shot_asked: false,
                icons: None,
                kind_icons: None,
                detail_icon: None,
                list_search: String::new(),
                wide_search: String::new(),
                wide_selected: None,
                wide_visible: Vec::new(),
                kinds_search: String::new(),
                // The SSH key row, so one mark is photographed in the
                // selected treatment on the rows themselves and not only
                // on the sheet beside them.
                kinds_selected: Some("kinds-ssh".to_string()),
                kinds_visible: Vec::new(),
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
                rail_received: false,
                rail_scope: sidebar::SendScope::All,
                gear_menu_open: false,
                list_offline: false,
                // 5b's own picked row: the Used Send whose link is dead and
                // whose views are spent. Picked because it is the ONE row
                // where every cell of the detail pane is in its interesting
                // state at once -- a struck-through address, a full meter, a
                // green pill and an activity sentence with something in it.
                sends_selected: Some("send-sap".to_string()),
                // A Waiting Send for the narrow shot, deliberately a
                // different one: the pair is looked at side by side, and two
                // pictures of the same Send would only show the width.
                sends_narrow_selected: Some("send-wifi".to_string()),
                // A record whose item IS still in `LIST_JSON`, so the received
                // detail draws the present-tense sentence; the row below it
                // in the same picture is the one whose item has gone.
                sends_received_selected: Some("list-0001".to_string()),
                sends_confirm_selected: Some("send-wifi".to_string()),
                sends_share_selected: Some(format!("share:list-0001:{SHARE_EMAIL}")),
                sends_none_selected: None,
                window_height: 0.0,
                login_variant: LoginVariant::Plain,
                second_factor: None,
                second_factor_for: None,
                api_key_for: None,
                api_key: deskwarden::api_key_ui::ApiKeyForm::new(),
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

/// How long a surface is drawn before it is captured, at least. The sign-in
/// and lock screens rise into place over about half a second (`kit::auth`'s
/// `rise`); twelve frames at 60 Hz caught them faded and mid-rise.
const MOTION_SETTLE_SECONDS: f64 = 0.9;

struct Preview {
    /// Paint with the dark palette (`--dark`).
    dark: bool,
    /// Cuts the login window's corners; see `window_host::CornerMask`.
    corners: deskwarden::window_host::CornerMask,
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
    /// Capture and exit, rather than sit there being looked at.
    screenshot: bool,
    /// Every surface has been captured and `Close` has been asked for. The
    /// frames eframe still draws after that must not try to capture a tenth.
    done: bool,
    /// Frames drawn since this surface's geometry last moved.
    frames: u32,
    /// When `frames` last started counting from one.
    warm_since: f64,
    /// The screenshot for this warm-up has been asked for.
    shot_asked: bool,
    /// The item-list shot's own state. Built on the first frame that needs it
    /// and then held: `load_texture` allocates a new texture on every call, so
    /// rebuilding the cache per frame would upload nine images a frame for the
    /// twelve warm-up frames.
    icons: Option<item_list::IconCache>,
    /// [`Surface::VaultListKinds`]'s own cache, built and held for the
    /// same reason [`Preview::icons`] is.
    kind_icons: Option<item_list::IconCache>,
    /// [`Surface::LoginDetailFavicon`]'s header artwork, held for the same
    /// reason the caches above are.
    detail_icon: Option<egui::TextureHandle>,
    list_search: String,
    wide_search: String,
    wide_selected: Option<String>,
    wide_visible: Vec<String>,
    kinds_search: String,
    kinds_selected: Option<String>,
    kinds_visible: Vec<String>,
    list_selected: Option<String>,
    list_visible: Vec<String>,
    /// The rail shot's own selection state.
    rail_selected: SidebarFilter,
    rail_sends: bool,
    rail_health: bool,
    /// Design 5b's `Shared with me` row, and which of its SHARING
    /// sub-filters is in force. Held beside the two flags above for
    /// `sidebar::Screens`' reason: the rail keeps three selection axes and
    /// this shot has to be able to put any of them in any state.
    rail_received: bool,
    rail_scope: sidebar::SendScope,
    /// Whether the shell holds the top bar's gear menu open this frame.
    gear_menu_open: bool,
    /// Whether the shell's list draws the offline banner.
    list_offline: bool,
    /// Which row each Sends shot has picked.
    ///
    /// **One field per surface, not one shared field**, for
    /// [`Preview::wide_selected`]'s reason and rather more sharply: the
    /// Sends pane WRITES this -- a click is a write to `selected`, and the
    /// screenshot run draws every surface in one process -- so a shared
    /// selection would let the narrow shot's fixture decide what the wide
    /// one showed, and a reviewer comparing the pair would be comparing two
    /// different Sends.
    sends_selected: Option<String>,
    sends_narrow_selected: Option<String>,
    sends_received_selected: Option<String>,
    sends_confirm_selected: Option<String>,
    sends_share_selected: Option<String>,
    /// The two shots that are pictures of a column with NOTHING picked. A
    /// field rather than a temporary, because the pane takes `&mut` and a
    /// temporary would be a place a click could write to that nothing ever
    /// read back.
    sends_none_selected: Option<String>,
    /// Last applied window height, for the login window's size-to-content.
    window_height: f32,
    /// Which state of the sign-in or lock screen the current surface shows.
    login_variant: LoginVariant,
    second_factor: Option<deskwarden::second_factor_ui::Prompt>,
    second_factor_for: Option<Surface>,
    api_key_for: Option<Surface>,
    api_key: deskwarden::api_key_ui::ApiKeyForm,
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
        if self.dark {
            deskwarden::kit::tokens::set_palette(&ctx, deskwarden::kit::tokens::Palette::DARK);
            DARK_SHOTS.store(true, std::sync::atomic::Ordering::Relaxed);
            // The real windows paint their own ground first (window_host does,
            // every frame); without it a dark shot sits on egui's light panel.
            theme::paint_window_background(root);
        }
        self.frames += 1;
        if self.frames == 1 {
            self.warm_since = ctx.input(|i| i.time);
            self.shot_asked = false;
        }
        // The glyph sheet draws every script at once, so the first time it is
        // current it loads every language's system faces, the way the Settings
        // picker does. They go live a frame later; the warm-up frames cover it.
        if self.current() == Surface::I18nFonts && !I18N_SHEET_FONTS_LOADED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            theme::set_fonts_for(&ctx, &I18N_SHEET_FONTS);
            self.frames = 0;
            ctx.request_repaint();
            return;
        }

        match self.current() {
            Surface::LoginUnlock => self.draw_login(root, &ctx, false, false, None),
            Surface::LoginUnlockBusy => self.draw_login(root, &ctx, false, true, None),
            Surface::LoginSigninSelfHosted => {
                self.login_variant = LoginVariant::SignInSelfHosted;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninError => {
                self.login_variant = LoginVariant::SignInError;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginUnlockPassword => {
                self.login_variant = LoginVariant::UnlockPassword;
                self.draw_login(root, &ctx, false, false, None)
            }
            Surface::LoginUnlockPasswordError => {
                self.login_variant = LoginVariant::UnlockPasswordError;
                self.draw_login(root, &ctx, false, false, None)
            }
            Surface::LoginSigninOffline => {
                self.login_variant = LoginVariant::SignInOffline;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninServerDown => {
                self.login_variant = LoginVariant::SignInServerDown;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninEmailError => {
                self.login_variant = LoginVariant::SignInEmailError;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninUrlError => {
                self.login_variant = LoginVariant::SignInUrlError;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninBusy => self.draw_login(root, &ctx, true, true, None),
            Surface::LoginSigninTooMany => {
                self.login_variant = LoginVariant::SignInTooMany;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSigninDone => {
                self.login_variant = LoginVariant::SignInDone;
                self.draw_login(root, &ctx, true, false, None)
            }
            Surface::LoginSecondFactor
            | Surface::LoginSecondFactorWrong
            | Surface::LoginSecondFactorEmail
            | Surface::LoginSecondFactorYubiKey
            | Surface::LoginSecondFactorBusy
            | Surface::LoginSecondFactorUnsupported => {
                let surface = self.current();
                self.draw_second_factor(root, surface)
            }
            Surface::LoginApiKey
            | Surface::LoginApiKeyPassword
            | Surface::LoginApiKeyError
            | Surface::LoginApiKeyBusy => {
                let surface = self.current();
                self.draw_api_key(root, surface)
            }
            Surface::LoginSignin => self.draw_login(root, &ctx, true, false, None),
            Surface::LoginCliSetup => self.draw_login(
                root,
                &ctx,
                true,
                false,
                Some(&deskwarden::bw_acquire::CliSetupState::Asking),
            ),
            // **One shot, because there is one modal.** It used to be two:
            // the state carried the client in use and whether the account had
            // signed in before, and those varied a sentence and the button
            // order. They do not exist any more.
            Surface::LoginBackendChoice => self.draw_login(
                root,
                &ctx,
                true,
                false,
                Some(&deskwarden::bw_acquire::CliSetupState::Choosing),
            ),
            Surface::LoginDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Login)),
            Surface::CardDetail => self.draw_pane(root, PaneKind::Detail(DetailShot::Card)),
            Surface::CardDetailRevealed => {
                self.draw_pane(root, PaneKind::Detail(DetailShot::CardRevealed))
            }
            Surface::DiscardConfirm => self.draw_pane(root, PaneKind::Discard),
            Surface::EditLogin | Surface::EditLoginNarrow => {
                self.draw_pane(root, PaneKind::EditLogin(false))
            }
            Surface::EditLoginDirty => self.draw_pane(root, PaneKind::EditLogin(true)),
            Surface::EditLoginHistory => self.draw_pane(root, PaneKind::EditLoginHistory),
            Surface::EditCard | Surface::EditCardNarrow => {
                self.draw_pane(root, PaneKind::EditCard)
            }
            Surface::EditWebsites => self.draw_pane(root, PaneKind::EditWebsites),
            Surface::EditSparse => self.draw_pane(root, PaneKind::EditSparse(false)),
            Surface::EditSparseAdding => self.draw_pane(root, PaneKind::EditSparse(true)),
            Surface::EditNewLogin => self.draw_pane(root, PaneKind::EditNew),
            Surface::EditAvatarMenu => {
                detail_edit::open_avatar_menu(root.ctx());
                self.draw_pane(root, PaneKind::EditLogin(false))
            }
            Surface::MenuSelectOpen => {
                deskwarden::kit::menu::open_select_for_preview(root.ctx(), "(\"website-match\"");
                self.draw_pane(root, PaneKind::EditWebsites)
            }
            Surface::MenuContextOpen => {
                self.draw_vault_list(root);
                if let Some(item) = self.fixtures.list.get(1) {
                    let entries = item_list::menu_entries(item, &self.fixtures.folders, deskwarden::vault_window::sidebar::FilterSource::LiveVault, true);
                    deskwarden::kit::menu::show_at(
                        root.ctx(),
                        egui::Id::new("preview-context-menu"),
                        egui::pos2(150.0, 190.0),
                        |ui| {
                            for entry in &entries {
                                match entry {
                                    item_list::MenuEntry::Command(c) => {
                                        let row = deskwarden::kit::menu::Item::new(&c.label)
                                            .danger(c.label.starts_with("Delete"))
                                            .enabled(c.enabled);
                                        deskwarden::kit::menu::item(ui, row);
                                    }
                                    item_list::MenuEntry::MoveToFolder(_) => {
                                        deskwarden::kit::menu::item(ui, deskwarden::kit::menu::Item::new(item_list::move_to_folder_label()));
                                    }
                                    item_list::MenuEntry::Icon(_) => {
                                        deskwarden::kit::menu::item(ui, deskwarden::kit::menu::Item::new(item_list::icon_label()));
                                    }
                                }
                            }
                        },
                    );
                }
            }
            Surface::ScrolledList => draw_scrolled_list(root),
            Surface::EditStepsTesting => {
                let sequence = self.fixtures.clean_draft.app.as_ref().map(|a| a.sequence.clone()).unwrap_or_default();
                if !root.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("preview-steps-started")).unwrap_or(false)) {
                    deskwarden::vault_window::sign_in_steps::play_test_from(root.ctx(), &sequence, 1.4, self.fixtures.clean_draft.session);
                    root.ctx().data_mut(|d| d.insert_temp(egui::Id::new("preview-steps-started"), true));
                }
                self.draw_pane(root, PaneKind::EditLogin(false))
            }
            Surface::RecordComposer => self.draw_pane(root, PaneKind::Composer),
            Surface::TotpAddConfirm => self.draw_pane(root, PaneKind::TotpAdd),
            Surface::TotpAddPicker => self.draw_pane(root, PaneKind::TotpPicker),
            Surface::TotpAddTyped => self.draw_pane(root, PaneKind::TotpForm(TotpShot::Typed)),
            Surface::TotpAddRefused => {
                self.draw_pane(root, PaneKind::TotpForm(TotpShot::Refused))
            }
            Surface::TotpAddRefusedLongest => {
                self.draw_pane(root, PaneKind::TotpForm(TotpShot::RefusedLongest))
            }
            Surface::TotpAddScanned => self.draw_pane(root, PaneKind::TotpForm(TotpShot::Scanned)),
            Surface::TotpAddNarrow => self.draw_pane(root, PaneKind::TotpForm(TotpShot::Narrow)),
            Surface::PrefsClipboard => self.draw_prefs(root, true),
            Surface::PrefsClipboardOff => self.draw_prefs(root, false),
            Surface::PrefsAbout => self.draw_prefs_about(root),
            Surface::PrefsUpdatesNoUpdate
            | Surface::PrefsUpdatesAvailable
            | Surface::PrefsUpdatesShortNotes
            | Surface::PrefsUpdatesManyReleases
            | Surface::PrefsUpdatesDownloading
            | Surface::PrefsUpdatesReady
            | Surface::PrefsUpdatesIdle
            | Surface::PrefsUpdatesChecking
            | Surface::PrefsUpdatesFailed => self.draw_prefs_updates(root, self.current()),
            Surface::PrefsBreachesIdle
            | Surface::PrefsBreachesRunning
            | Surface::PrefsBreachesFailed
            | Surface::PrefsBreachesHistory => self.draw_prefs_breaches(root, self.current()),
            Surface::PrefsVaultOfficialCli | Surface::PrefsVaultBuiltIn => {
                self.draw_prefs_vault(root, self.current())
            }
            Surface::PrefsWindowChrome => self.draw_prefs_window(root),
            Surface::VaultList => self.draw_vault_list(root),
            Surface::VaultListWide => self.draw_vault_list_wide(root),
            Surface::VaultListKinds => self.draw_vault_list_kinds(root),
            Surface::KitSheet => draw_kit_sheet(root),
            Surface::I18nFonts => draw_i18n_fonts(root),
            Surface::CardLogos => draw_card_logos(root),
            Surface::VaultShellNavy => {
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, false)
            }
            Surface::VaultShellLight => {
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Light, DetailShot::Login, false)
            }
            Surface::VaultShellGearMenu => {
                self.gear_menu_open = true;
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, false)
            }
            Surface::VaultShellCard => {
                // The list's own first card is the row shown selected.
                if let Some(card) = self.fixtures.list.iter().find(|i| i.item_type == Some(3)) {
                    self.list_selected = Some(card.id.clone());
                }
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Card, false)
            }
            Surface::ToolGenerator => {
                let mut state = deskwarden::vault_window::generator_screen::GeneratorScreen::new(Default::default());
                let _ = deskwarden::vault_window::generator_screen::draw(root, &mut state);
            }
            surface @ (Surface::SettingsGeneral
            | Surface::SettingsSecurity
            | Surface::SettingsVault
            | Surface::SettingsShortcuts
            | Surface::SettingsAutofill
            | Surface::SettingsApps
            | Surface::SettingsAppsKeys
            | Surface::SettingsGenerator
            | Surface::SettingsNotifications
            | Surface::SettingsImportExport
            | Surface::SettingsDevices
            | Surface::SettingsAccount
            | Surface::SettingsAbout
            | Surface::SettingsSecurityPrivacy
            | Surface::SettingsSecurityBreaches
            | Surface::SettingsVaultBackend
            | Surface::SettingsVaultTip
            | Surface::SettingsLanguageMenu) => {
                self.draw_settings_page(root, surface);
            }
            Surface::ModalNewItem => {
                self.list_selected = self.fixtures.list.first().map(|i| i.id.clone());
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, false);
                let _ = deskwarden::vault_window::new_item_modal::draw(root.ctx());
            }
            surface @ (Surface::ModalDelete
            | Surface::ModalFolder
            | Surface::ModalIcon
            | Surface::ModalEmptyTrash
            | Surface::ModalForget
            | Surface::ModalShare
            | Surface::ModalShareInvited
            | Surface::ModalShareQuestion
            | Surface::ModalRecordImport
            | Surface::ModalGenerator
            | Surface::ModalGeneratorOptions
            | Surface::ModalAppPicker
            | Surface::ModalSequence
            | Surface::ModalTotpScanning
            | Surface::ModalTotpCameras) => {
                use deskwarden::vault_window::{delete_modal, folder_modal, icon_modal, record_ui, trash_modal};
                self.list_selected = self.fixtures.list.first().map(|i| i.id.clone());
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, false);
                let ctx = root.ctx().clone();
                match surface {
                    Surface::ModalDelete => {
                        let mut state = delete_modal::DeleteConfirmState::new(&self.fixtures.login, None, delete_modal::DeleteKind::ToTrash);
                        let _ = delete_modal::draw_delete_modal(&ctx, &mut state);
                    }
                    Surface::ModalFolder => {
                        let mut state = folder_modal::FolderEditState::new("f1".into(), "Work/Clients".into());
                        let _ = folder_modal::draw_folder_edit_modal(&ctx, &mut state);
                    }
                    Surface::ModalIcon => {
                        let mut state = icon_modal::IconPickState::new(&self.fixtures.login, None, None);
                        let _ = icon_modal::draw_icon_modal(&ctx, &mut state);
                    }
                    Surface::ModalEmptyTrash => {
                        let _ = trash_modal::draw(&ctx, 3);
                    }
                    Surface::ModalForget => {
                        let names = vec!["Ledgerline".to_string(), "Northwind Mail".to_string()];
                        let _ = record_ui::draw_forget_confirm(&ctx, "anna@northwind.example", &names);
                    }
                    Surface::ModalShare | Surface::ModalShareInvited | Surface::ModalShareQuestion => {
                        use deskwarden::vault_window::share_ui::{SharePreview, SharePreviewStage};
                        let stage = match surface {
                            Surface::ModalShare => SharePreviewStage::Asking,
                            Surface::ModalShareInvited => SharePreviewStage::Invited,
                            _ => SharePreviewStage::Question,
                        };
                        SharePreview::new(stage).draw(&ctx);
                    }
                    Surface::ModalRecordImport => {
                        let mut state = record_ui::RecordImport::opening();
                        state.draft.link = "https://vault.northwind.example/#/send/Q2x1/ZkxN".to_string();
                        state.fetched = Some(Ok(deskwarden::record::Record {
                            name: "Ledgerline".to_string(),
                            username: Some("a.novak@ledgerline.com".to_string()),
                            password: Some(zeroize::Zeroizing::new("hunter2".to_string())),
                            uri: Some("https://app.ledgerline.eu".to_string()),
                            notes: None,
                            totp_sealed: None,
                            not_after: None,
                        }));
                        let collision = deskwarden::record::import::Collision::SameName { existing_id: "list-0001".to_string() };
                        let now = send::FixedClock(PREVIEW_MILLIS);
                        let _ = record_ui::draw_import_modal(&ctx, &mut state, &collision, &now);
                    }
                    Surface::ModalGenerator | Surface::ModalGeneratorOptions => {
                        detail_edit::preview_generator_modal(
                            &ctx,
                            "k7#Qm2vP9xL!tR4wZb8",
                            matches!(surface, Surface::ModalGeneratorOptions),
                        );
                    }
                    Surface::ModalAppPicker => {
                        let row = |title: &str, exe: &str, hwnd: isize| detail_edit::AppWindowRow {
                            title: title.to_string(),
                            exe_name: exe.to_string(),
                            exe_path: format!("C:\\Program Files\\{exe}"),
                            hosted: false,
                            pid: 100 + hwnd as u32,
                            hwnd,
                        };
                        let windows = vec![
                            row("SAP Logon 760", "saplogon.exe", 1),
                            row("Remote Desktop Connection", "mstsc.exe", 2),
                            row("Ledgerline Desktop", "ledgerline.exe", 3),
                            row("Postgres admin", "pgadmin4.exe", 4),
                            row("Notepad", "notepad.exe", 5),
                        ];
                        detail_edit::preview_app_window_picker(&ctx, windows, Some(2));
                    }
                    Surface::ModalSequence => {
                        use deskwarden::vault_window::sequence_builder;
                        let edit_login = item(EDIT_LOGIN_JSON);
                        let mut draft = sequence_builder::SequenceDraft::for_item(&edit_login, "Work", "SAP GUI for Windows")
                            .expect("the 8a fixture carries an app binding");
                        let palette = vec![
                            deskwarden::key_sequence::FieldRef::Username,
                            deskwarden::key_sequence::FieldRef::Password,
                        ];
                        let totp = deskwarden::vault_window::detail::TotpState::NoSecret;
                        let source = deskwarden::key_sequence::ResolveSource {
                            username: "a.novak@ledgerline.com",
                            password: "correct-horse-battery",
                            custom: Vec::new(),
                            totp: &totp,
                        };
                        let mut apps = deskwarden::app_identity::AppIdentityCache::default();
                        let _ = sequence_builder::draw_sequence_builder(root, &mut draft, &palette, &source, None, &mut apps);
                    }
                    Surface::ModalTotpCameras => {
                        let mut state = TotpAdd::opening("1", "Ledgerline", false);
                        state.stage = totp_add::Stage::Webcam;
                        let camera = |name: &str| deskwarden::webcam::Device { name: name.into(), id: name.into() };
                        state.webcam = Some(totp_add::WebcamStage {
                            devices: vec![camera("Integrated Camera"), camera("Logitech BRIO")],
                            chosen: None,
                            session: None,
                            size: None,
                            texture: None,
                        });
                        let _ = totp_add::draw_add_modal(&ctx, &mut state, PREVIEW_UNIX);
                    }
                    _ => {
                        let mut state = TotpAdd::opening("1", "Ledgerline", false);
                        state.stage = totp_add::Stage::Scanning;
                        let _ = totp_add::draw_add_modal(&ctx, &mut state, PREVIEW_UNIX);
                    }
                }
            }
            surface @ (Surface::ModalImportSource
            | Surface::ModalImportFile
            | Surface::ModalImportProgress
            | Surface::ModalImportDone) => {
                use deskwarden::vault_window::import_formats::ImportSource;
                use deskwarden::vault_window::import_ui::{self, Step, Summary, Wizard};
                self.list_selected = self.fixtures.list.first().map(|i| i.id.clone());
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, false);
                let done = Summary { imported: 128, duplicates: 3, ..Summary::default() };
                let mut wizard = match surface {
                    Surface::ModalImportSource => Wizard::default(),
                    Surface::ModalImportFile => Wizard::preview(Step::File, ImportSource::Browser, Summary::default(), (0, 0)),
                    Surface::ModalImportProgress => {
                        Wizard::preview(Step::Progress, ImportSource::Browser, Summary::default(), (84, 128))
                    }
                    _ => Wizard::preview(Step::Done, ImportSource::Browser, done, (0, 0)),
                };
                let _ = import_ui::draw(root.ctx(), &mut wizard);
            }
            Surface::VaultShellOffline => {
                // The login the detail pane shows is the row selected.
                self.list_selected = self.fixtures.list.first().map(|i| i.id.clone());
                self.draw_vault_shell(root, deskwarden::kit::tokens::SidebarStyle::Navy, DetailShot::Login, true)
            }
            Surface::VaultSplash => {
                let rect = root.max_rect();
                deskwarden::vault_window::splash::paint(root.ctx(), rect, false, 0.0);
            }
            Surface::LoginDetailFavicon => {
                self.draw_pane(root, PaneKind::Detail(DetailShot::LoginFavicon))
            }
            Surface::VaultRail => self.draw_vault_rail(root),
            Surface::VaultHealth
            | Surface::VaultHealthBreached
            | Surface::VaultHealthReusedOnly
            | Surface::VaultHealthWeakOnly
            | Surface::VaultHealthBreachedOnly
            | Surface::VaultHealthEmptyFilter => {
                self.draw_vault_health(root, self.current())
            }
            Surface::SendsSelected
            | Surface::SendsList
            | Surface::SendsEmpty
            | Surface::SendsReceived
            | Surface::SendsConfirm
            | Surface::SendsShare
            | Surface::SendsShareStop
            | Surface::SendsPeople
            | Surface::SendsNarrow => self.draw_sends(root, self.current()),
            Surface::Rehearsal => self.draw_rehearsal(root),
            Surface::VaultSetupSpinner => self.draw_vault_setup_spinner(root),
            Surface::PickerItems
            | Surface::PickerItemsNoMatch
            | Surface::PickerApps
            | Surface::PickerAppsBackendDown
            | Surface::PickerAppsRefused
            | Surface::PickerNoticeEmpty
            | Surface::PickerNoticeNoLogins
            | Surface::PickerNoticeUnreachable
            | Surface::PickerNoticeLocked => self.draw_picker(root, self.current()),
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

        // The login window's outer corners, cut the way the real window cuts
        // them, so the PNG carries the rounded silhouette (transparent pixels
        // outside it). The pane surfaces are not whole windows.
        if self.current().is_login_window() {
            self.corners.paint(&ctx);
        }

        if self.screenshot && !self.done {
            let warm_for = ctx.input(|i| i.time) - self.warm_since;
            if self.frames >= WARMUP_FRAMES && !self.shot_asked && warm_for >= MOTION_SETTLE_SECONDS {
                self.shot_asked = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            } else if !self.shot_asked {
                ctx.request_repaint();
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

    }
}

/// Which of the in-window panes `draw_pane` is drawing.
enum PaneKind {
    /// The read pane, in one of its three shots.
    Detail(DetailShot),
    /// The edit form with its discard confirmation up.
    Discard,
    /// Design 8a's subject: the Edit login form, pristine (`false`) or with
    /// edits in it (`true`). **Two fixtures, never one re-typed** -- see
    /// `PaneKind::TotpForm`'s note, and the `--all` walk that shares one
    /// `Fixtures`.
    EditLogin(bool),
    /// The same form with its password-history list open -- see
    /// [`Surface::EditLoginHistory`]. A third draft rather than a flag on the
    /// clean one, for `EditLogin`'s stated reason: the `--all` walk shares one
    /// `Fixtures`, and a disclosure opened for this shot would decide what
    /// `edit_login` looked like if it happened to run after it.
    EditLoginHistory,
    /// The edit form of a CARD, which is the kind with six removable rows on
    /// it -- see [`Surface::EditCard`].
    EditCard,
    /// The edit form of a login carrying several websites.
    EditWebsites,
    /// The edit form of a sparse identity, with the Add menu open or shut.
    EditSparse(bool),
    /// The v2 editor creating a login.
    EditNew,
    /// The Send record composer.
    Composer,
    /// The "add a one-time code" form, mid-confirmation.
    TotpAdd,
    /// Design 6a's picker, the front door onto the four routes.
    TotpPicker,
    /// The same form in one of [`TotpShot`]'s states -- the five this
    /// example added when it turned out design 6d could not be rendered
    /// here at all.
    TotpForm(TotpShot),
}

/// Which state of the add-a-code card `PaneKind::TotpForm` is drawing.
///
/// An enum rather than five `PaneKind` variants, for [`DetailShot`]'s reason:
/// these are one surface in five states, and the thing under review is the
/// difference between them.
#[derive(Clone, Copy)]
enum TotpShot {
    /// A bare base32 seed, accepted -- `totp_add_typed`.
    Typed,
    /// The reported `sdf` -- `totp_add_refused`.
    Refused,
    /// The longest sentence [`totp_add::refusal_sentence`] can render --
    /// `totp_add_refused_longest`.
    RefusedLongest,
    /// A decoded payload, confirmed rather than typed -- `totp_add_scanned`.
    Scanned,
    /// The accepted card again, in a 900x600 window -- `totp_add_narrow`.
    Narrow,
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
    /// The same login with a favicon on its header --
    /// `detail_login_favicon`. See [`Surface::LoginDetailFavicon`].
    LoginFavicon,
    /// The card fixture as it opens: number and code masked -- `detail_card`.
    Card,
    /// The card fixture with the NUMBER revealed and the security code still
    /// masked -- `detail_card_revealed`. The code stays hidden because that is
    /// the state a user reading their number is really in (the two rows have
    /// separate flags), and because a CVV in a checked-in PNG is worth
    /// avoiding even from a fixture.
    CardRevealed,
}

/// The states of the sign-in and lock screens a surface can ask for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoginVariant {
    Plain,
    SignInSelfHosted,
    SignInError,
    SignInOffline,
    SignInServerDown,
    SignInEmailError,
    SignInUrlError,
    SignInTooMany,
    SignInDone,
    UnlockPassword,
    UnlockPasswordError,
}

impl Preview {
    fn draw_login(
        &mut self,
        root: &mut egui::Ui,
        ctx: &egui::Context,
        signin: bool,
        auth_in_progress: bool,
        // `Some` draws the CLI-setup modal in place of the credential card,
        // exactly as `run_login_flow` does. It used to be hardcoded `None`
        // here, with a comment saying the modal "has its own tests" -- and
        // it did, all of them on `title()` and `body()`, none of them on the
        // panel that failed to paint either one.
        cli_setup: Option<&deskwarden::bw_acquire::CliSetupState>,
    ) {
        // The exact surfaces the shipped window draws: the split screen while
        // signed out, the navy lock screen while locked, each with its own
        // caption buttons.
        let full = root.max_rect();
        // A fresh form each frame: the variant below decides what it holds.
        self.form = LoginForm::default();
        // Sample data mirroring the design (unlock: Hello shown as enrolled
        // so the Hello stage renders; sign-in: available but unenrolled so
        // the opt-in renders); actions are ignored -- a preview must never
        // spawn `bw` or pop Hello.
        let (status, email, hello) = if signin {
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
        // The lock screen says what the auto-lock does; the shipped default.
        self.form.auto_lock_minutes = Some(15);
        // ...and why this lock happened: the idle timer, the design's own line.
        self.form.lock_reason = Some(deskwarden::lock_reason::LockReason::Idle { minutes: 15 });
        if signin && auth_in_progress {
            self.form.email = "a.novak@ledgerline.com".to_string();
        }
        if signin && cli_setup.is_some() {
            self.form.server_choice = login_ui::ServerChoice::SelfHosted;
        }
        match self.login_variant {
            LoginVariant::SignInSelfHosted => {
                self.form.server_choice = login_ui::ServerChoice::SelfHosted;
                self.form.server_url = "https://vault.ledgerline.eu".to_string();
            }
            LoginVariant::SignInError => {
                self.form.email = "a.novak@ledgerline.com".to_string();
                self.form.error =
                    Some("That email or vault password didn't work. Check them and try again.".to_string());
            }
            LoginVariant::SignInOffline => {
                self.form.email = "a.novak@ledgerline.com".to_string();
                self.form.error = Some(
                    "Couldn't reach the server. Check your connection \u{2014} and the server URL, if this is a self-hosted account."
                        .to_string(),
                );
            }
            LoginVariant::SignInServerDown => {
                self.form.server_choice = login_ui::ServerChoice::SelfHosted;
                self.form.server_url = "https://vault.ledgerline.com".to_string();
                self.form.email = "a.novak@ledgerline.com".to_string();
                self.form.error = Some(
                    "Couldn't reach the server. Check your connection \u{2014} and the server URL, if this is a self-hosted account."
                        .to_string(),
                );
            }
            LoginVariant::SignInEmailError => {
                self.form.error = Some(login_ui::enter_email().to_string());
            }
            LoginVariant::SignInUrlError => {
                self.form.server_choice = login_ui::ServerChoice::SelfHosted;
                self.form.email = "a.novak@ledgerline.com".to_string();
                self.form.error = Some(login_ui::enter_server().to_string());
            }
            LoginVariant::SignInTooMany => {
                self.form.error = Some("Too many failed attempts. Wait a few minutes, then try again.".to_string());
            }
            LoginVariant::SignInDone => {
                self.form.signed_in = true;
            }
            LoginVariant::UnlockPassword => {
                // Typed, and shown as dots.
                self.form.password = "correct horse battery".to_string();
            }
            LoginVariant::UnlockPasswordError => {
                self.form.password = "correct horse".to_string();
                self.form.error = Some("That vault password is not right.".to_string());
            }
            LoginVariant::Plain => {}
        }
        let mut flow_bottom = 0.0;
        let form = &mut self.form;
        let mut body = |ui: &mut egui::Ui| {
            let _ = login_ui::draw_login_window(
                ui,
                status,
                email,
                "vault.ledgerline.eu",
                hello,
                form,
                &mut flow_bottom,
                // In flight only for `LoginUnlockBusy`, and even there
                // nothing is actually running: this draws the window's
                // states and never spawns a real `bw`.
                auth_in_progress,
                // Not a first run: the preview draws the window an
                // existing account meets, so the first-run notice is not
                // part of what this screenshots.
                false,
                cli_setup,
            );
        };
        let caption = if signin {
            deskwarden::kit::auth::split(root, full, login_ui::CloseControl::Active, &mut body)
        } else {
            deskwarden::kit::auth::lock_split(root, full, login_ui::CloseControl::Active, &mut body)
        };
        deskwarden::kit::auth::apply_caption(ctx, caption);
        self.login_variant = LoginVariant::Plain;
    }

    /// The second-factor prompt on the split screen, in the state `surface` names.
    fn draw_second_factor(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::rest::api::SecondFactor;
        if self.second_factor_for != Some(surface) {
            self.second_factor_for = Some(surface);
            let both = vec![
                SecondFactor::Authenticator,
                SecondFactor::Email { masked: Some("a***@ledgerline.com".to_string()) },
            ];
            let mut prompt = match surface {
                Surface::LoginSecondFactorEmail => deskwarden::second_factor_ui::Prompt::new(vec![
                    SecondFactor::Email { masked: Some("a***@ledgerline.com".to_string()) },
                    SecondFactor::Authenticator,
                ]),
                Surface::LoginSecondFactorYubiKey => {
                    deskwarden::second_factor_ui::Prompt::new(vec![SecondFactor::YubiKey, SecondFactor::Authenticator])
                }
                Surface::LoginSecondFactorUnsupported => {
                    deskwarden::second_factor_ui::Prompt::new(vec![SecondFactor::Unsupported(2)])
                }
                _ => deskwarden::second_factor_ui::Prompt::new(both),
            };
            match surface {
                Surface::LoginSecondFactor => prompt.code = "482".to_string(),
                Surface::LoginSecondFactorWrong => {
                    prompt.code = "482913".to_string();
                    prompt.went_wrong(deskwarden::second_factor_ui::Trouble::CodeRejected);
                    prompt.code = "482913".to_string();
                }
                Surface::LoginSecondFactorEmail => {
                    prompt.choose(SecondFactor::Email { masked: Some("a***@ledgerline.com".to_string()) });
                    prompt.sent_a_code();
                    prompt.code = "71".to_string();
                }
                Surface::LoginSecondFactorYubiKey => {
                    prompt.choose(SecondFactor::YubiKey);
                    prompt.code = "ccccccbtkgrn".to_string();
                }
                Surface::LoginSecondFactorBusy => {
                    prompt.code = "482913".to_string();
                    prompt.busy = true;
                }
                _ => {}
            }
            self.second_factor = Some(prompt);
        }
        if let Some(prompt) = self.second_factor.as_mut() {
            let _ = deskwarden::second_factor_ui::draw(root, prompt);
        }
    }

    /// The API-key sign-in on the split screen, in the state `surface` names.
    fn draw_api_key(&mut self, root: &mut egui::Ui, surface: Surface) {
        if self.api_key_for != Some(surface) {
            self.api_key_for = Some(surface);
            let mut form = deskwarden::api_key_ui::ApiKeyForm::new();
            match surface {
                Surface::LoginApiKeyPassword => {
                    form.step = deskwarden::api_key_ui::ApiKeyStep::MasterPassword;
                }
                Surface::LoginApiKeyError => {
                    form.client_id = "user.0b3c1a52-7a1e".to_string();
                    form.secret = zeroize::Zeroizing::new("not-a-real-secret".to_string());
                    form.error = Some(deskwarden::api_key_ui::message_for(
                        deskwarden::api_key_ui::ApiKeyRefusal::KeyPairRejected,
                    )
                    .to_string());
                }
                Surface::LoginApiKeyBusy => {
                    form.client_id = "user.0b3c1a52-7a1e".to_string();
                    form.secret = zeroize::Zeroizing::new("not-a-real-secret".to_string());
                    form.busy = true;
                }
                _ => {}
            }
            self.api_key = form;
        }
        let _ = deskwarden::api_key_ui::draw(root, &mut self.api_key);
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
        state.show_scrolled(470.0);
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
    /// The Vault page, in each of the two states the backend picker has.
    ///
    /// **A self-hosted account throughout**, installed through
    /// `PrefsState::show_account_source` and never through
    /// `prefs_ui::publish_account_status`: that writes a process-wide value,
    /// and this example draws every surface in one process. It has to be
    /// self-hosted, because that is the only account where the choice is live
    /// -- on `bitwarden.com` both cells are ghosted and the two pictures
    /// would be the same picture.
    ///
    /// Nothing here reads a vault, a file or the network. The state is
    /// rebuilt each frame, as `draw_prefs` rebuilds its own: there is nothing
    /// on this page to carry between frames, and the Windows Hello probe
    /// `PrefsState::new` installs answers "unavailable" without asking the
    /// OS.
    fn draw_prefs_vault(&mut self, root: &mut egui::Ui, surface: Surface) {
        fn self_hosted() -> Option<prefs_ui::AccountStatus> {
            Some(prefs_ui::AccountStatus::SignedIn {
                email: Some("someone@example.com".to_string()),
                server: Some("https://vault.example.com".to_string()),
            })
        }

        theme::paint_window_background(root);
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings {
            use_official_bw_crypto: Some(matches!(surface, Surface::PrefsVaultOfficialCli)),
            ..deskwarden::settings::Settings::default()
        });
        state.show(prefs_ui::Section::Vault);
        state.show_scrolled(720.0);
        state.show_account_source(self_hosted);
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

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
        state.show_scrolled(1330.0);
        state.show_scan_stage(stage);
        state.show_scan_history(ScanHistory { entries: history });
        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(root, |ui| prefs_ui::draw_prefs_body(ui, &mut state));
    }

    /// One page of the Settings screen, the way the vault window draws it and
    /// the tray's Preferences window now does too. The state is rebuilt each
    /// frame; nothing here reads a file, the registry or the network.
    fn draw_settings_page(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::prefs_ui::settings_v2::{draw, Page};
        use deskwarden::service_keys::{Access, KeyRecord, Scope, Subject};
        fn self_hosted() -> Option<prefs_ui::AccountStatus> {
            Some(prefs_ui::AccountStatus::SignedIn {
                email: Some("someone@example.com".to_string()),
                server: Some("https://vault.example.com".to_string()),
            })
        }
        fn now() -> u64 {
            1_800_000_000
        }
        let (mut page, scroll) = match surface {
            Surface::SettingsSecurity => (Page::Security, None),
            Surface::SettingsSecurityPrivacy => (Page::Security, Some(470.0)),
            Surface::SettingsSecurityBreaches => (Page::Security, Some(1330.0)),
            Surface::SettingsVault => (Page::Vault, None),
            Surface::SettingsVaultBackend => (Page::Vault, Some(720.0)),
            // From the top: the scroll offset of the shot before it is
            // remembered by egui, so say where this one starts.
            Surface::SettingsVaultTip => (Page::Vault, Some(0.0)),
            Surface::SettingsShortcuts => (Page::Shortcuts, None),
            Surface::SettingsAutofill => (Page::Autofill, None),
            Surface::SettingsApps => (Page::Apps, None),
            Surface::SettingsAppsKeys => (Page::Apps, Some(560.0)),
            Surface::SettingsGenerator => (Page::Generator, None),
            Surface::SettingsNotifications => (Page::Notifications, None),
            Surface::SettingsImportExport => (Page::ImportExport, None),
            Surface::SettingsDevices => (Page::Devices, None),
            Surface::SettingsAccount => (Page::Account, None),
            Surface::SettingsAbout => (Page::About, None),
            _ => (Page::General, None),
        };
        let mut state = prefs_ui::PrefsState::new(deskwarden::settings::Settings {
            use_official_bw_crypto: Some(false),
            service_enabled: matches!(surface, Surface::SettingsAppsKeys),
            ..deskwarden::settings::Settings::default()
        });
        state.show_account_source(self_hosted);
        state.show_key_clock(now);
        if matches!(surface, Surface::SettingsAppsKeys) {
            state.show_service_keys(vec![
                KeyRecord {
                    name: "Backup script".into(),
                    hash: "00".into(),
                    created_unix: now() - 86_400 * 12,
                    expires_unix: None,
                    scopes: vec![Scope { access: Access::Read, subject: Subject::All }],
                },
                KeyRecord {
                    name: "Old CI job".into(),
                    hash: "01".into(),
                    created_unix: now() - 86_400 * 90,
                    expires_unix: Some(now() - 86_400),
                    scopes: vec![Scope { access: Access::Write, subject: Subject::All }],
                },
            ]);
        }
        if let Some(y) = scroll {
            state.show_scrolled(y);
        }
        if page == Page::Security {
            // The score card needs the numbers the window offers it.
            use deskwarden::vault_window::password_health::{FoundItem, HealthReport, OldItem, ReuseGroup, WeakItem};
            let found = |id: &str| FoundItem { id: id.into(), name: id.into() };
            let weak = |id: &str| WeakItem {
                item: found(id),
                chars: 4,
                classes: deskwarden::password_strength::CharClasses::of("abcd"),
            };
            let report = HealthReport {
                weak: vec![weak("a"), weak("b")],
                reused: vec![ReuseGroup { items: vec![found("c"), found("d")] }],
                old: (0..3).map(|i| OldItem { item: found(&format!("o{i}")), days: 400 }).collect(),
                ..Default::default()
            };
            prefs_ui::security_score::offer(root.ctx(), &report);
        }
        if matches!(surface, Surface::SettingsVaultTip) {
            // No pointer to hover with: hold the bubble open on the row the
            // design would be hovered on.
            prefs_ui::pin_row_tip(root.ctx(), "Show TOTP secrets on the details screen");
        }
        if matches!(surface, Surface::SettingsLanguageMenu) {
            prefs_ui::language_picker::open(root.ctx());
        }
        let _ = draw(root, &mut state, &mut page);
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
                // **The release's own heading, first**, because that is what
                // production sends: `.github/workflows/release.yml` opens
                // every published body with this line. It is also the only
                // heading over these notes now -- the panel's own "What is
                // new" and `updater::notes_heading`'s "Deskwarden 0.9.0" were
                // both removed, and this fixture is where that is reviewable.
                // Without it, this picture showed a body no release has ever
                // had and the duplication was invisible here.
                "## What's new in 0.9.0\n",
                "\n",
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
                "\n",
                // The workflow's own last line, verbatim. `release.yml`
                // composes `**[Full changelog](...)**` for every release, so
                // this is a copy of production output rather than an invented
                // string -- and it is the one line of the panel a reader is
                // most likely to want to follow, which makes it worth a
                // picture that it renders as a link rather than as its own
                // square brackets in bold.
                "**[Full changelog](https://example.invalid/CHANGELOG.md)**\n",
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
                    "## What's new in 0.9.0\n",
                    "- Release notes now cover **every** version you skipped.\n",
                    "- The scrollbar appears only when there is more to read.\n",
                    "\n",
                    // **The one release still filed under a heading of the
                    // app's own**, and the reason `updater::notes_heading`
                    // survives at all: this release published nothing, so
                    // there is no `## What's new in 0.8.6` of its own to name
                    // it, and without this line the sentence below would sit
                    // unattributed between two other releases' notes. Every
                    // other entry here carries the heading `release.yml` wrote
                    // for it, which is why they no longer carry two.
                    "## Deskwarden 0.8.6\n",
                    "_This release came with no notes._\n",
                    "\n",
                    "## What's new in 0.8.5\n",
                    "- The vault window remembers its size, via `settings.json`.\n",
                    "- Details on [the release page](https://example.invalid/r).\n",
                    "\n",
                    "## What's new in 0.8.4\n",
                    "- Cyrillic names render in the app's own typeface.\n",
                    "- One Deskwarden per session, and starting it again takes over.\n",
                    "- Password health rows line up with the item list again.\n",
                    "\n",
                    "## What's new in 0.8.3\n",
                    "- The favourite star is lighter, rounder and quieter.\n",
                    "- Browsers no longer get the \"no saved login\" card.\n",
                    "\n",
                    "## What's new in 0.8.2\n",
                    "- The autofill prompt setting silences every pop-up.\n",
                    "- The reveal eye is taller and rounder.\n",
                    "\n",
                    "## What's new in 0.8.1\n",
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
            Surface::PrefsUpdatesIdle => UpdateStage::Idle,
            Surface::PrefsUpdatesChecking => UpdateStage::Checking,
            Surface::PrefsUpdatesReady => UpdateStage::Ready(release),
            // **The handover refusal, not a network one.** A connection that
            // closed was already covered by the download's own error; what
            // this flow had no picture of is the failure the user actually
            // hit, where the installer is downloaded and verified and cannot
            // be run because another Deskwarden still holds the app mutex.
            // That case used to produce no message at all -- the process
            // exited on a spawn that succeeded and an install that did not --
            // so this is the picture of the thing that was missing.
            Surface::PrefsUpdatesFailed => UpdateStage::Failed {
                message: "another Deskwarden is still running, so the installer would find \
                          the app in use and give up without showing anything. Quit every \
                          Deskwarden window and try again. (It still holds \
                          Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630; the \
                          verified installer is at \
                          C:\\Users\\you\\AppData\\Local\\Deskwarden\\cache\\updates\\\
                          deskwarden-0.9.0-installer.exe.)"
                    .to_string(),
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

    /// **The two "Add app" pickers and their empty states**, through the same
    /// `picker_ui::draw_*` bodies the windows paint, so these PNGs are what the
    /// tray's Add app shows.
    fn draw_picker(&mut self, root: &mut egui::Ui, surface: Surface) {
        use deskwarden::picker_ui::{self, AppPickerView, BackendReadiness, PickerNotice};
        use deskwarden::window_list::WindowInfo;
        let notice = match surface {
            Surface::PickerNoticeEmpty => Some(PickerNotice::EmptyVault),
            Surface::PickerNoticeNoLogins => Some(PickerNotice::NoLogins),
            Surface::PickerNoticeUnreachable => Some(PickerNotice::Unreachable(
                "connection refused (os error 10061)".to_string(),
            )),
            Surface::PickerNoticeLocked => Some(PickerNotice::Locked),
            _ => None,
        };
        if let Some(notice) = notice {
            let _ = picker_ui::draw_picker_notice(root, &notice);
            return;
        }
        if matches!(surface, Surface::PickerItems | Surface::PickerItemsNoMatch) {
            let logins = items(
                r#"[
                {"id":"i1","name":"Ledgerline","type":1,"fields":[],"login":{"username":"a.novak@ledgerline.com"}},
                {"id":"i2","name":"SAP GUI for Windows","type":1,"fields":[],"login":{"username":"anovak"}},
                {"id":"i3","name":"Git Host","type":1,"fields":[],"login":{"username":"anovak"}},
                {"id":"i4","name":"Postgres admin","type":1,"fields":[],"login":{"username":"postgres"}},
                {"id":"i5","name":"Remote Desktop Gateway","type":1,"fields":[],"login":{"username":"anovak"}},
                {"id":"i6","name":"Payroll portal","type":1,"fields":[],"login":{"username":"alex.n"}}
            ]"#,
            );
            let mut filter =
                if surface == Surface::PickerItemsNoMatch { "zzzz".to_string() } else { String::new() };
            let mut selected = Some("i2".to_string());
            let _ = picker_ui::draw_item_picker(root, &logins, &mut filter, &mut selected);
            return;
        }
        let row = |title: &str, exe: &str, hwnd: isize| WindowInfo {
            hwnd,
            pid: 4100 + hwnd as u32,
            exe_path: format!("C:\\Program Files\\{exe}"),
            exe_name: exe.to_string(),
            title: title.to_string(),
            hosted: false,
        };
        let windows = vec![
            row("SAP Logon 760", "saplogon.exe", 1),
            row("Remote Desktop Connection", "mstsc.exe", 2),
            row("Ledgerline Desktop", "ledgerline.exe", 3),
            row("Postgres admin", "pgadmin4.exe", 4),
            row("Calculator", "ApplicationFrameHost.exe", 5),
            row("Notepad", "notepad.exe", 6),
        ];
        let backend = if surface == Surface::PickerAppsBackendDown {
            BackendReadiness::Unavailable("connection refused (os error 10061)".to_string())
        } else {
            BackendReadiness::Ready
        };
        let mut filter = String::new();
        let mut selected = Some(if surface == Surface::PickerAppsRefused { 5 } else { 3 });
        let mut icons = picker_ui::IconCache::new();
        let _ = picker_ui::draw_app_picker(
            root,
            &AppPickerView {
                target_name: "SAP GUI for Windows",
                windows: &windows,
                backend: &backend,
                existing_host_match: None,
                save_error: None,
                save_notice: None,
                already_saved: false,
            },
            &mut filter,
            &mut selected,
            &mut icons,
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
                // The shipped picker chord, spelled rather than read from
                // `hotkey::published_chord_text`: this preview registers
                // nothing, so nothing has been published, and a still frame of
                // a design ought to show the combination a user would see
                // rather than an empty one.
                hotkey_chord: "CTRL+ALT+B",
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


    /// One row of every kind, with real-shaped favicons -- see
    /// [`Surface::VaultListKinds`]. Drawn through `draw_item_list` itself, on
    /// the same panel the shipped list is hosted in.
    fn draw_vault_list_kinds(&mut self, root: &mut egui::Ui) {
        let icons = self
            .kind_icons
            .get_or_insert_with(|| shaped_icons(root.ctx(), &self.fixtures.kinds));
        let fixtures = &self.fixtures;
        let (search, selected, visible) =
            (&mut self.kinds_search, &mut self.kinds_selected, &mut self.kinds_visible);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(list_ground()))
            .show(root, |ui| {
                let _ = item_list::draw_item_list(
                    ui,
                    Some(&fixtures.kinds),
                    &fixtures.folders,
                    &SidebarFilter::All,
                    search,
                    selected,
                    icons,
                    // The preview drives no drag and opens no menu, so
                    // the un-file destination has nothing to decide.
                    false,
                    visible,
                    None,
                    false,
                    // A preview draws no window, so nothing reads the open
                    // gesture here; the slide it arms lives in ault_window::mod.
                    &mut false,
                    // **Design 5d's `shared` pill: nothing is shared
                    // here.** The set comes from the Sends list the window
                    // fetches, and a preview fetches nothing -- an empty
                    // set is the same answer `send_ui::shared_names` gives
                    // before that list has been answered, which is the
                    // state every shot of this screen has been taken in.
                    &std::collections::HashSet::new(),
                    // Design 6's person marks: a preview holds no sharing view,
                    // so no row wears a person or a `view-only` chip.
                    &item_list::RowPeople::default(),
                    "All items",
                    false,
                );
            });
    }

    /// The vault window's item list, drawn through `draw_item_list` itself on
    /// the same `list_ground()` panel `vault_window::mod` hosts it in.
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
        let offline = self.list_offline;
        let (search, selected, visible) =
            (&mut self.list_search, &mut self.list_selected, &mut self.list_visible);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(list_ground()))
            .show(root, |ui| {
                let _ = item_list::draw_item_list(
                    ui,
                    Some(&fixtures.list),
                    &fixtures.folders,
                    &SidebarFilter::All,
                    search,
                    selected,
                    icons,
                    // The preview drives no drag and opens no menu, so
                    // the un-file destination has nothing to decide.
                    false,
                    visible,
                    None,
                    false,
                    // A preview draws no window, so nothing reads the open
                    // gesture here; the slide it arms lives in ault_window::mod.
                    &mut false,
                    // **Design 5d's `shared` pill: nothing is shared
                    // here.** The set comes from the Sends list the window
                    // fetches, and a preview fetches nothing -- an empty
                    // set is the same answer `send_ui::shared_names` gives
                    // before that list has been answered, which is the
                    // state every shot of this screen has been taken in.
                    &std::collections::HashSet::new(),
                    // Design 6's person marks: a preview holds no sharing view,
                    // so no row wears a person or a `view-only` chip.
                    &item_list::RowPeople::default(),
                    "All items",
                    offline,
                );
            });
    }

    /// [`draw_vault_list`], with NOTHING SELECTED, in the full-width column
    /// the list is given when no detail pane is on screen.
    ///
    /// Its own fixture selection state, not [`draw_vault_list`]'s: the two
    /// surfaces are drawn in the same process by `--all` and a shared
    /// `list_selected` would let one shot decide what the other showed.
    fn draw_vault_list_wide(&mut self, root: &mut egui::Ui) {
        let icons = self.icons.get_or_insert_with(|| {
            preview_icons(root.ctx(), &self.fixtures.list, &["Ledgerline", "Ledgerline corporate card"])
        });
        let fixtures = &self.fixtures;
        let (search, selected, visible) = (
            &mut self.wide_search,
            &mut self.wide_selected,
            &mut self.wide_visible,
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(list_ground()))
            .show(root, |ui| {
                let _ = item_list::draw_item_list(
                    ui,
                    Some(&fixtures.list),
                    &fixtures.folders,
                    &SidebarFilter::All,
                    search,
                    selected,
                    icons,
                    // The preview drives no drag and opens no menu, so
                    // the un-file destination has nothing to decide.
                    false,
                    visible,
                    None,
                    false,
                    &mut false,
                    // **Design 5d's `shared` pill: nothing is shared
                    // here.** The set comes from the Sends list the window
                    // fetches, and a preview fetches nothing -- an empty
                    // set is the same answer `send_ui::shared_names` gives
                    // before that list has been answered, which is the
                    // state every shot of this screen has been taken in.
                    &std::collections::HashSet::new(),
                    // Design 6's person marks: a preview holds no sharing view,
                    // so no row wears a person or a `view-only` chip.
                    &item_list::RowPeople::default(),
                    "All items",
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

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(list_ground()))
            .show(root, |ui| {
                let _ = password_health::draw_password_health(ui, &report, false);
            });
    }

    /// The vault window's rail, drawn through `draw_sidebar` itself in the same
    /// `theme::CARD` panel frame `vault_window::mod` hosts it in -- margins
    /// included, because the rail's insets are measured against them.
    ///
    /// Four folder rows: enough that the FOLDERS group reads as a group, few
    /// enough that the two screen rows below them are in the picture.
    /// The v2 shell: the real rail in `style`, the real top bar, and the
    /// 0.15 list and detail panes laid out under the bar as the window lays
    /// them out.
    fn draw_vault_shell(
        &mut self,
        root: &mut egui::Ui,
        style: deskwarden::kit::tokens::SidebarStyle,
        shot: DetailShot,
        offline: bool,
    ) {
        self.list_offline = offline;
        use deskwarden::vault_window::top_bar;
        deskwarden::kit::tokens::set_sidebar_style(root.ctx(), style);
        let window = root.max_rect();
        let bar = egui::Rect::from_min_max(
            egui::pos2(window.left() + SIDEBAR_WIDTH, window.top()),
            egui::pos2(window.right(), window.top() + top_bar::HEIGHT),
        );
        let pill = if offline {
            top_bar::SyncPill::from_v1(theme::ERROR, "Sync failed")
        } else {
            top_bar::SyncPill::from_v1(theme::BLUE, "Synced just now")
        };
        let mut search = String::new();
        let placeholder = item_list::search_hint(Some(self.fixtures.list.len()), &SidebarFilter::All);
        let mut gear = None;
        let _ = top_bar::draw(root, bar, &mut search, &placeholder, &pill, false, false, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(top_bar::AVATAR), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), top_bar::AVATAR / 2.0, theme::INK);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "AN",
                egui::FontId::new(14.0, egui::FontFamily::Name(theme::BOLD.into())),
                egui::Color32::WHITE,
            );
        }, false, &mut gear);
        if std::mem::take(&mut self.gear_menu_open) {
            if let Some(gear) = gear {
                // Held open the way the real menu is drawn, under the gear.
                let at = gear.rect.left_bottom() + egui::vec2(0.0, deskwarden::kit::menu::ANCHOR_GAP);
                let lists = sidebar::VaultLists { health_findings: 4, ..sidebar::VaultLists::live_only(&self.fixtures.list) };
                deskwarden::kit::menu::show_at(root.ctx(), egui::Id::new("preview-gear-menu"), at, |ui| {
                    sidebar::tools_rows(
                        ui,
                        lists,
                        &mut sidebar::Screens {
                            sends: &mut false,
                            health: &mut true,
                            received: &mut false,
                            generator: &mut false,
                            settings: &mut false,
                            scope: &mut sidebar::SendScope::All,
                        },
                    )
                });
            }
        }
        {
            let fixtures = &self.fixtures;
            let (selected, sends, health, received, scope) = (
                &mut self.rail_selected,
                &mut self.rail_sends,
                &mut self.rail_health,
                &mut self.rail_received,
                &mut self.rail_scope,
            );
            egui::Panel::left("vault-sidebar")
                .exact_size(SIDEBAR_WIDTH)
                .resizable(false)
                .frame(sidebar::panel_frame(style))
                .show(root, |ui| {
                    let _ = sidebar::draw_sidebar(
                        ui,
                        sidebar::VaultLists { health_findings: 4, ..sidebar::VaultLists::live_only(&fixtures.list) },
                        &fixtures.rail_folders,
                        selected,
                        sidebar::Screens { sends, health, received, generator: &mut false, settings: &mut false, scope },
                        "Locks in 11:42",
                        false,
                    );
                });
        }
        egui::Panel::top("vault-top-bar-band")
            .exact_size(top_bar::HEIGHT)
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(root, |_ui| {});
        egui::Panel::left("vault-item-list")
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(root, |ui| self.draw_vault_list(ui));
        self.draw_pane(root, PaneKind::Detail(shot));
    }

    fn draw_vault_rail(&mut self, root: &mut egui::Ui) {
        let fixtures = &self.fixtures;
        let (selected, sends, health, received, scope) = (
            &mut self.rail_selected,
            &mut self.rail_sends,
            &mut self.rail_health,
            &mut self.rail_received,
            &mut self.rail_scope,
        );
        egui::CentralPanel::default()
            // Design 4.8's own frame, copied from the `Panel::left` in
            // `vault_window::mod`: `theme::CARD` with `padding: 14px 10px`.
            .frame(sidebar::panel_frame(deskwarden::kit::tokens::SidebarStyle::Navy))
            .show(root, |ui| {
                let _ = sidebar::draw_sidebar(
                    ui,
                    sidebar::VaultLists {
                        // 5b's own five Sends, split across its own three
                        // sub-rows -- 2 waiting, 1 used, 2 ended -- so the
                        // shot shows the section adding up rather than a
                        // number invented for the picture.
                        sends: Some(sidebar::SendCounts::over([
                            crate::send::SendState::Waiting,
                            crate::send::SendState::Waiting,
                            crate::send::SendState::Used,
                            crate::send::SendState::Expired,
                            crate::send::SendState::Revoked,
                        ])),
                        received: 52,
                        health_findings: 4,
                        ..sidebar::VaultLists::live_only(&fixtures.list)
                    },
                    &fixtures.rail_folders,
                    selected,
                    sidebar::Screens { sends, health, received, generator: &mut false, settings: &mut false, scope },
                    "Locks in 11:42",
                    false,
                );
            });
    }

    /// **Design 5b's Sends screen**, drawn through `send_ui::draw_send_pane`
    /// itself on the same `list_ground()` `CentralPanel` -- margins included,
    /// which here means NO margin: `vault_window::mod` gives that panel none,
    /// because both of this screen's columns carry white strips that have to
    /// span them edge to edge, and a margin on the container is exactly the
    /// darker band the owner reported around the detail pane.
    ///
    /// **Nothing here reaches `bw`.** The pane is handed a `SendPaneState`
    /// built by the shipped `pane_state` out of a `Result` stated outright,
    /// so what these shots show is what the real screen shows for that
    /// answer -- and the wording of every date on them comes from
    /// `send_ui`'s own functions, against [`PREVIEW_MILLIS`] and
    /// [`PREVIEW_ZONE`], so a shot does not change between runs.
    ///
    /// The verdict the pane returns is **applied and discarded here rather
    /// than dropped**: `SendUiVerdict` counts its own abandonment, and the
    /// shipped window `debug_assert!`s that count is zero. A preview that
    /// let one fall on the floor would be a preview that trips that assertion
    /// in a debug build -- which is the whole point of the counter, and not
    /// something an example gets an exemption from.
    fn draw_sends(&mut self, root: &mut egui::Ui, surface: Surface) {
        // Which answer the CLI is pretending to have given. `Empty` is the
        // real empty ANSWER -- `Ok(vec![])` -- and not a filtered-away list,
        // because those are two different screens and `pane_state` is the one
        // place that difference is decided.
        let answer: Result<Vec<send::SendSummary>, send::SendError> =
            if matches!(surface, Surface::SendsEmpty) {
                Ok(Vec::new())
            } else {
                Ok(self.fixtures.sends.clone())
            };
        let state = send_ui::pane_state(
            Some(&answer),
            &send::FixedClock(PREVIEW_MILLIS),
            &PREVIEW_ZONE,
        );

        let received = send_ui::received_rows(
            &self.fixtures.received,
            &self.fixtures.list,
            &send::FixedClock(PREVIEW_MILLIS),
            &PREVIEW_ZONE,
        );
        // The people the pane is told about: one person share, and one record
        // somebody shares with this account.
        let shares = vec![deskwarden::vault_window::share_ui::ShareRow {
            key: format!("share:list-0001:{SHARE_EMAIL}"),
            item_id: "list-0001".to_string(),
            name: "Ledgerline".to_string(),
            email: SHARE_EMAIL.to_string(),
            state: deskwarden::rest::sharing::PersonState::Shared,
            subtitle: format!("to {SHARE_EMAIL} \u{b7} Shared"),
            since: Some("14 Sep".to_string()),
        }];
        let from_people = vec![deskwarden::vault_window::share_ui::ReceivedShareRow {
            key: "recv:list-0002".to_string(),
            item_id: "list-0002".to_string(),
            name: "Northwind Mail".to_string(),
            from: "denis@example.com".to_string(),
            verified: true,
            organization_id: "org-1".to_string(),
        }];
        let view = if matches!(surface, Surface::SendsReceived | Surface::SendsPeople) {
            send_ui::SendView::Received(&received)
        } else {
            send_ui::SendView::Mine(sidebar::SendScope::All)
        };
        // Each shot picks out of its OWN field; see `Preview::sends_selected`
        // on why one shared selection would let one picture decide another's.
        let selected: &mut Option<String> = match surface {
            Surface::SendsSelected => &mut self.sends_selected,
            Surface::SendsNarrow => &mut self.sends_narrow_selected,
            Surface::SendsReceived => &mut self.sends_received_selected,
            Surface::SendsConfirm => &mut self.sends_confirm_selected,
            Surface::SendsShare | Surface::SendsShareStop => &mut self.sends_share_selected,
            // `SendsList` and `SendsEmpty` are the pictures of a column with
            // nothing picked, so they are handed a selection that is None and
            // stays None -- a field of their own would be a field nothing
            // ever reads.
            _ => &mut self.sends_none_selected,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(list_ground()))
            .show(root, |ui| {
                let delete = if matches!(surface, Surface::SendsConfirm) {
                    send_ui::SendDeleteView { confirming: Some("send-wifi"), ..send_ui::SendDeleteView::default() }
                } else {
                    send_ui::SendDeleteView::default()
                };
                use deskwarden::vault_window::share_ui::PeopleRows;
                let mut people = match surface {
                    Surface::SendsShare => PeopleRows::with_shares(&shares),
                    Surface::SendsShareStop => {
                        PeopleRows::with_shares(&shares).confirming_stop(&format!("share:list-0001:{SHARE_EMAIL}"))
                    }
                    Surface::SendsPeople => PeopleRows::with_received(&from_people),
                    // No daemon behind the other shots: no person share.
                    _ => PeopleRows::none(),
                };
                let verdict = send_ui::draw_send_pane(
                    ui,
                    &state,
                    None,
                    delete,
                    view,
                    selected,
                    // No vault behind the preview: every tile a monogram.
                    send_ui::RecordTiles::default(),
                    &mut people,
                );
                // Taken and thrown away: a preview has no window to act on.
                // See the doc above for why it must not simply be dropped.
                let _ = verdict.into_action();
            });
    }

    /// The surfaces that live *inside* the vault window rather than in one of
    /// their own, drawn on the window's own canvas so the PNG shows them
    /// against the background they actually sit on.
    fn draw_pane(&mut self, root: &mut egui::Ui, kind: PaneKind) {
        // Built before `fixtures` is borrowed, and built ONCE:
        // `load_texture` allocates on every call and this pane is drawn
        // for every warm-up frame.
        let wants_icon = matches!(kind, PaneKind::Detail(DetailShot::LoginFavicon));
        let header_icon = wants_icon.then(|| {
            let ctx = root.ctx().clone();
            self.detail_icon
                .get_or_insert_with(|| inset_favicon(&ctx, "detail-header", 180))
                .clone()
        });
        let fixtures = &mut self.fixtures;
        egui::CentralPanel::default()
            .frame(pane_frame())
            .show(root, |ui| match kind {
                PaneKind::Detail(shot) => {
                    let card = matches!(shot, DetailShot::Card | DetailShot::CardRevealed);
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
                        // A favicon only on the shot that is ABOUT one --
                        // every other detail shot keeps the monogram
                        // fallback, which is what an avatar shows without
                        // an icon and what those shots have always shown.
                        header_icon.as_ref(),
                        &mut fixtures.apps,
                        // **Design 5d's `SHARING` card: no live Send.**
                        // Same reason as the pill above -- the answer
                        // comes from a Sends list a preview does not
                        // fetch -- and the card is drawn only when there
                        // IS one, so this is the screen as every shot of
                        // it has shown it.
                        None,
                        // Breach checking OFF -- `detail::should_check` is the
                        // gate, and with it false the cache below is never
                        // asked anything and no worker is started. A preview
                        // must not reach Have I Been Pwned.
                        false,
                        // The TOTP seed row is a preference that is off by
                        // default; the preview draws the default window.
                        false,
                        &mut fixtures.breaches,
                        // Design 6: nothing about sharing -- a preview has no
                        // daemon to ask whether a Share control belongs here.
                        detail::PeopleSharing::default(),
                        // No vault behind the preview either: a card's bank
                        // is named from its domain alone.
                        &[],
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
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &fixtures.totp,
                        None,
                    );
                    // The form's Cancel is what sets this in the app; a
                    // preview has nobody to click it, so it is re-armed every
                    // frame. Re-armed rather than set once, because the
                    // confirmation's own KeepEditing arm clears it -- and a
                    // stray Escape delivered to this window would otherwise
                    // take the dialog away before the capture.
                    fixtures.draft.discard_prompt = true;
                }
                PaneKind::EditLogin(dirty) => {
                    let draft = if dirty {
                        &mut fixtures.dirty_draft
                    } else {
                        &mut fixtures.clean_draft
                    };
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        draft,
                        &fixtures.folders,
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.edit_login),
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &fixtures.totp,
                        None,
                    );
                }
                PaneKind::EditNew => {
                    let mut draft = EditDraft::empty_of(deskwarden::vault_bridge::ItemKind::Login);
                    draft.name = "Ledgerline".into();
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut draft,
                        &fixtures.folders,
                        true,
                        &mut fixtures.apps,
                        None,
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &TotpState::NoSecret,
                        None,
                    );
                }
                PaneKind::EditLoginHistory => {
                    // **Set, never toggled**, for the reason
                    // `PaneKind::EditSparse` sets its own menu that way: one
                    // `Fixtures` is shared by the whole `--all` walk, and a
                    // disclosure left open would change whichever surface ran
                    // next.
                    fixtures.history_draft.history_open = true;
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.history_draft,
                        &fixtures.folders,
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.edit_login),
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &fixtures.totp,
                        None,
                    );
                }
                PaneKind::EditCard => {
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.card_draft,
                        &fixtures.folders,
                        // An EDIT, not a create: a create hides nothing and so
                        // draws no Remove at all, which is the one thing this
                        // surface exists to show.
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.card),
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &TotpState::NoSecret,
                        None,
                    );
                }
                PaneKind::EditWebsites => {
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.websites_draft,
                        &fixtures.folders,
                        // An EDIT, not a create: `NewItem::Login` carries no
                        // `uris`, so the create form draws the notice rather
                        // than the boxes and this shot would be a picture of
                        // one greyed line.
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.websites_login),
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &TotpState::NoSecret,
                        None,
                    );
                }
                PaneKind::EditSparse(adding) => {
                    // **Set, never toggled**, for the reason the read pane's
                    // reveal flags are: one `Fixtures` is shared by the whole
                    // `--all` walk, and a menu left open would change
                    // whichever surface happened to run next.
                    fixtures.sparse_draft.add_menu_open = adding;
                    let _ = detail_edit::draw_detail_edit(
                        ui,
                        &mut fixtures.sparse_draft,
                        &fixtures.folders,
                        false,
                        &mut fixtures.apps,
                        Some(&fixtures.sparse_identity),
                        0,
                        &deskwarden::rest::organizations::Audience::Personal,
                        &TotpState::NoSecret,
                        None,
                    );
                }
                PaneKind::Composer => {
                    // v2: the Share modal over the shot (spec B §6), with
                    // the chips (advanced sharing on).
                    let mut state = record_ui::RecordSend::opening(&fixtures.login.id, "Ledgerline");
                    state.draft.selection = fixtures.record.selection;
                    let _ = record_ui::draw_export_modal(
                        ui.ctx(),
                        &mut state,
                        Some(&fixtures.login),
                        Some("Engineering"),
                        true,
                        &deskwarden::sent_history::SentHistory::default(),
                        false,
                        &send::FixedClock(PREVIEW_MILLIS),
                        &PREVIEW_ZONE,
                    );
                }
                PaneKind::TotpAdd => {
                    // A FIXED instant, not `SystemTime::now()`: the code and
                    // the countdown are the point of this shot, and a
                    // screenshot whose two most prominent numbers change every
                    // run is a screenshot no reviewer can diff.
                    fixtures.totp_add.stage = totp_add::Stage::Manual;
                    let _ = totp_add::draw_add_modal(ui.ctx(), &mut fixtures.totp_add, PREVIEW_UNIX);
                }
                PaneKind::TotpPicker => {
                    let _ = totp_add::draw_add_modal(ui.ctx(), &mut fixtures.totp_picker, PREVIEW_UNIX);
                }
                // **One fixture per shot, never one fixture re-typed.** The
                // `--all` walk shares a single `Fixtures`, so a state
                // rewritten in place here would decide whichever surface
                // happened to be drawn next -- the trap the read pane's
                // `RevealState` comment records, and the one the Sends
                // composer fell into.
                PaneKind::TotpForm(shot) => {
                    let state = match shot {
                        TotpShot::Typed => &mut fixtures.totp_typed,
                        TotpShot::Refused => &mut fixtures.totp_refused,
                        TotpShot::RefusedLongest => &mut fixtures.totp_refused_longest,
                        TotpShot::Scanned => &mut fixtures.totp_scanned,
                        TotpShot::Narrow => &mut fixtures.totp_narrow,
                    };
                    state.stage = totp_add::Stage::Manual;
                    let _ = totp_add::draw_add_modal(ui.ctx(), state, PREVIEW_UNIX);
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
    /// The login fixture opened for editing and NOT touched -- design 8a's
    /// subject as it opens. See [`Surface::EditLogin`].
    clean_draft: EditDraft,
    /// The same draft with two edits in it, so the dirty state has something
    /// to report. See [`Surface::EditLoginDirty`].
    dirty_draft: EditDraft,
    /// A THIRD draft off the same item, opened only so its password-history
    /// list can be left open -- see [`Surface::EditLoginHistory`].
    history_draft: EditDraft,
    /// The card fixture opened for editing -- see [`Surface::EditCard`].
    card_draft: EditDraft,
    /// The item behind [`Self::clean_draft`] and [`Self::dirty_draft`], kept
    /// because `draw_detail_edit` takes the item as well as the draft -- the
    /// keystroke palette and its preview are resolved against it.
    edit_login: VaultItem,
    /// The item behind [`Self::websites_draft`], kept because
    /// `draw_detail_edit` takes the item as well as the draft.
    websites_login: VaultItem,
    /// A login carrying three websites, opened for editing -- see
    /// [`Surface::EditWebsites`].
    websites_draft: EditDraft,
    /// The item behind [`Self::sparse_draft`].
    sparse_identity: VaultItem,
    /// An identity with four of its eighteen fields filled in, opened for
    /// editing -- see [`Surface::EditSparse`].
    sparse_draft: EditDraft,
    record: RecordDraft,
    totp_add: TotpAdd,
    totp_picker: TotpAdd,
    /// 6d with a bare seed typed in -- see [`Surface::TotpAddTyped`].
    totp_typed: TotpAdd,
    /// 6d with the owner's `sdf` in it -- see [`Surface::TotpAddRefused`].
    totp_refused: TotpAdd,
    /// 6d with the longest refusal the parser renders -- see
    /// [`Surface::TotpAddRefusedLongest`].
    totp_refused_longest: TotpAdd,
    /// 6c, a payload a decoder read -- see [`Surface::TotpAddScanned`].
    totp_scanned: TotpAdd,
    /// The accepted card again, drawn in a 900x600 window -- see
    /// [`Surface::TotpAddNarrow`].
    totp_narrow: TotpAdd,
    rehearsal: scratch_window::RehearsalView,
    rehearsal_arrived: String,
    /// The Password health screen's items -- see [`HEALTH_JSON`].
    health: Vec<VaultItem>,
    /// One row of every kind -- see [`KINDS_JSON`].
    kinds: Vec<VaultItem>,
    /// Design 5b's own five Sends plus a file one -- see [`SENDS_JSON`].
    sends: Vec<send::SendSummary>,
    /// The `Shared with me` screen's records -- see [`received_history`].
    received: ReceiveHistory,
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

        // **Design 8a's own two shots.** Two drafts off one item, never one
        // re-typed between surfaces: the `--all` walk shares a single
        // `Fixtures`, so a draft dirtied for the dirty shot would decide
        // whichever surface ran next. The trap `PaneKind::TotpForm`'s comment
        // records.
        let edit_login = item(EDIT_LOGIN_JSON);
        let clean_draft = EditDraft::from_item(&edit_login);
        // The premise of the whole set: the card grid has something in every
        // card. A fixture that lost its binding or its fields would render a
        // form of empty states and look like a layout regression.
        assert!(
            clean_draft.app.is_some(),
            "the 8a fixture carries no app binding, so the autofill card is a picture of an \
             empty state"
        );
        // By NAME, not by count: the binding above rides on a
        // `deskwarden:app-match` field of its own, which the draft carries and
        // the form does not draw, so a length check here would be a check on
        // an internal convention rather than on what is in the picture.
        for named in ["Client ID", "Security PIN"] {
            assert!(
                clean_draft.fields.iter().any(|f| f.name == named),
                "the 8a fixture lost its {named:?} field, so the custom-fields card is a \
                 picture of an empty state"
            );
        }
        let mut dirty_draft = EditDraft::from_item(&edit_login);
        // Exactly 8a's own two changes -- "Password / Native app added" on its
        // rail, "2 changes" in its footer -- so the picture beside the design
        // is a picture of the same edit.
        dirty_draft.password = "tq7Rvk29mzpLx4-hd8".to_string();
        if let Some(app) = dirty_draft.app.as_mut() {
            app.process = "LedgerlineSync.exe".to_string();
        }
        assert!(
            dirty_draft.is_dirty(),
            "the dirty shot's draft reads clean, so it is a second picture of the pristine form"
        );
        // The history shot's own draft. The LIST comes off the item rather
        // than off the draft -- the form never edits it -- so what this third
        // draft is for is the disclosure flag, and it exists rather than being
        // flipped on `clean_draft` for the reason above: one `Fixtures`, one
        // `--all` walk, and a flag left set decides the next surface.
        let history_draft = EditDraft::from_item(&edit_login);
        assert_eq!(
            detail::visible_history_count(&deskwarden::vault_bridge::password_history(
                &edit_login
            )),
            3,
            "the 8a fixture lost its password history, so the history shot is a picture of a \
             card with no control on it"
        );
        // The card fixture opened for editing. Checked for its Removes rather
        // than for its rows: a card whose optional fields were all empty would
        // draw them behind the Add control instead, and the shot would be a
        // picture of the one thing it is not about.
        let card_draft = EditDraft::from_item(&card);
        assert!(
            card_draft
                .shown_slots(false)
                .iter()
                .filter(|slot| !slot.always_shown())
                .count()
                >= 1,
            "the card shot is not showing the removable rows it exists for"
        );
        // The same account reached three ways, which is the ordinary shape of
        // a multi-URI login and not a contrived one: the app, the SSO host it
        // redirects to, and the Android package. Untouched, so the shot is of
        // a form as it OPENS -- the state the owner's report was about.
        let websites_login = item(WEBSITES_JSON);
        let websites_draft = EditDraft::from_item(&websites_login);
        assert_eq!(
            websites_draft.uris.len(),
            3,
            "the websites shot is not showing a multi-website login, so it shows nothing the \
             one-website shots do not"
        );
        // Four of eighteen fields, which is what a real identity looks like.
        // The point of the shot is the fourteen that are NOT drawn.
        let sparse_identity = item(SPARSE_IDENTITY_JSON);
        let sparse_draft = EditDraft::from_item(&sparse_identity);
        assert_eq!(
            sparse_draft.shown_slots(false).len(),
            4,
            "the sparse shot is showing {} rows, so it is not a picture of a sparse form",
            sparse_draft.shown_slots(false).len()
        );
        assert_eq!(
            sparse_draft.addable_slots(false).len(),
            14,
            "the sparse shot has nothing behind its Add control"
        );
        let totp = TotpState::Code { code: "418902".to_string(), seconds_left: 19, period: 30 };
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

        // **Design 6d as the design itself draws it**: the panel's own seed,
        // spaces and all, so the picture beside `#6d` differs from it in
        // nothing but the rendering. Against an item with NO code, because
        // the replace warning is `totp_add` above's half of this set and a
        // caution band on every shot would push the line under the field --
        // the thing these pictures exist for -- further down each one.
        let mut totp_typed = TotpAdd::opening("preview", "Git Host \u{b7} anovak", false);
        totp_typed.typed = zeroize::Zeroizing::new("JBSW Y3DP EHPK 3PXP".to_string());

        // **The reported defect, spelled the way it was reported.**
        let mut totp_refused = TotpAdd::opening("preview", "Git Host \u{b7} anovak", false);
        totp_refused.typed = zeroize::Zeroizing::new("sdf".to_string());

        // **The worst case the line under the field can be handed**, chosen
        // here rather than written down.
        //
        // Every refusal the one field can be driven into is listed with the
        // input that drives it there; the shot takes whichever renders the
        // longest sentence. A refusal reworded longer than today's worst case
        // therefore MOVES this picture instead of leaving it behind, which is
        // the whole failure this surface exists to prevent: the three-line
        // overlap shipped because no rendered state had a sentence long
        // enough to wrap.
        let drivable: Vec<(&str, OtpRefusal)> = vec![
            ("https://example.com/login", OtpRefusal::NotOtpAuth),
            (
                "otpauth://hotp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&counter=1",
                OtpRefusal::NotTotp,
            ),
            ("otpauth://totp/Git%20Host:anovak?issuer=Git%20Host", OtpRefusal::NoSecret),
            ("not!base32", OtpRefusal::BadSecret),
            ("sdf", OtpRefusal::PartialSecret(3)),
            (
                "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&surprise=icon.png",
                OtpRefusal::UnknownParameter("surprise".to_string()),
            ),
            (
                "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&period=0",
                OtpRefusal::BadParameter("period"),
            ),
        ];
        // Each input really produces the refusal it is filed under, so the
        // choice below is over sentences this field can actually show rather
        // than over a table somebody once wrote.
        for (typed, expected) in &drivable {
            let totp_add::Reading::Refused(actual) = totp_add::read_field(typed, 6, 30) else {
                panic!("the preview's {expected:?} fixture is not refused at all: {typed}");
            };
            assert_eq!(&actual, expected, "{typed} no longer produces {expected:?}");
        }
        let (longest_input, longest_refusal) = drivable
            .iter()
            .max_by_key(|(_, refusal)| totp_add::refusal_sentence(refusal).chars().count())
            .expect("the refusal table is not empty");
        // And nothing OUTSIDE that table renders longer. `TooLong` is the
        // one refusal no typed fixture here reaches -- it needs a URI past
        // `otpauth::MAX_URI_LEN` -- so without this the table could silently
        // stop covering the worst case.
        let longest_rendered = totp_add::refusal_sentence(longest_refusal).chars().count();
        for refusal in [
            OtpRefusal::NotOtpAuth,
            OtpRefusal::NotTotp,
            OtpRefusal::NoSecret,
            OtpRefusal::BadSecret,
            OtpRefusal::PartialSecret(3),
            OtpRefusal::UnknownParameter("surprise".to_string()),
            OtpRefusal::BadParameter("period"),
            OtpRefusal::TooLong,
        ] {
            let sentence = totp_add::refusal_sentence(&refusal);
            assert!(
                sentence.chars().count() <= longest_rendered,
                "{refusal:?} renders {} characters, longer than anything the preview can \
                 drive the field into ({longest_rendered}) -- the worst-case shot is no \
                 longer the worst case",
                sentence.chars().count()
            );
        }
        let mut totp_refused_longest =
            TotpAdd::opening("preview", "Git Host \u{b7} anovak", false);
        totp_refused_longest.typed = zeroize::Zeroizing::new((*longest_input).to_string());

        // **Design 6c**: a payload a decoder handed over, through the very
        // call the region scan makes -- `accept_decoded` -- rather than by
        // setting `scanned` beside `typed` here. Two fields that have to
        // agree are two fields that come to disagree, and what this shot is
        // for is the card that flag selects.
        let mut totp_scanned = TotpAdd::opening("preview", "Git Host \u{b7} anovak", false);
        totp_scanned.accept_decoded(zeroize::Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host\
             &digits=8&period=60&algorithm=SHA256"
                .to_string(),
        ));

        // **The tallest the card ever gets, in the shortest window it is ever
        // drawn in**: an accepted seed (so the confirmation, the live code and
        // the countdown are all on it) against an item that already has one
        // (so the caution band is too), at 900x600.
        let mut totp_narrow = TotpAdd::opening("preview", "Git Host \u{b7} anovak", true);
        totp_narrow.typed = zeroize::Zeroizing::new("JBSW Y3DP EHPK 3PXP".to_string());

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
            rehearsal: rehearsal_view(),
            health: items(HEALTH_JSON),
            kinds: items(KINDS_JSON),
            sends: sends(SENDS_JSON),
            received: received_history(),
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
            totp_typed,
            totp_refused,
            totp_refused_longest,
            totp_scanned,
            totp_narrow,
            draft,
            edit_login,
            clean_draft,
            dirty_draft,
            history_draft,
            card_draft,
            websites_login,
            websites_draft,
            sparse_identity,
            sparse_draft,
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

/// **The v2 kit sheet**: everything slice 1 of the 1.0 redesign built, drawn
/// through `kit`'s own calls, to hold beside the design (artboards 1a and 1c).
fn draw_card_logos(root: &mut egui::Ui) {
    use deskwarden::kit::avatar::{paint, Look};
    use deskwarden::kit::logos::{Mark, Net, BANKS, NETS};
    use egui::{vec2, Rect};
    let area = root.max_rect();
    root.painter().rect_filled(area, egui::CornerRadius::ZERO, egui::Color32::WHITE);
    let ink = egui::Color32::from_rgb(0x20, 0x1e, 0x1d);
    let faint = egui::Color32::from_rgb(0x8a, 0x87, 0x85);
    let label = |ui: &egui::Ui, at: egui::Pos2, text: &str| {
        ui.painter().text(at, egui::Align2::LEFT_TOP, text, egui::FontId::proportional(12.0), ink);
    };
    label(root, area.min + vec2(24.0, 16.0), "Card networks (44pt list tile, 72pt detail tile)");
    for (i, net) in NETS.into_iter().enumerate() {
        let x = 24.0 + i as f32 * 94.0;
        let list = Rect::from_min_size(area.min + vec2(x, 44.0), vec2(44.0, 44.0));
        paint(root.painter(), list, &Look::Brand { mark: Mark::Net(net), chip: None }, 12);
        let detail = Rect::from_min_size(area.min + vec2(x, 100.0), vec2(72.0, 72.0));
        paint(root.painter(), detail, &Look::Brand { mark: Mark::Net(net), chip: None }, 20);
        label(root, area.min + vec2(x, 180.0), net.name());
        root.painter().text(area.min + vec2(x, 196.0), egui::Align2::LEFT_TOP, net.rule(), egui::FontId::proportional(9.0), faint);
    }
    label(root, area.min + vec2(24.0, 232.0), "Banks, with the card's network as a chip");
    for (i, bank) in BANKS.into_iter().enumerate() {
        let x = 24.0 + i as f32 * 94.0;
        let chip = [Net::Visa, Net::Mastercard, Net::Amex, Net::Mir][i % 4];
        let list = Rect::from_min_size(area.min + vec2(x, 264.0), vec2(44.0, 44.0));
        paint(root.painter(), list, &Look::Brand { mark: Mark::Bank(bank), chip: Some(chip) }, 12);
        let detail = Rect::from_min_size(area.min + vec2(x, 320.0), vec2(72.0, 72.0));
        paint(root.painter(), detail, &Look::Brand { mark: Mark::Bank(bank), chip: Some(chip) }, 20);
        label(root, area.min + vec2(x, 410.0), bank.name());
    }
}

/// The system-face groups the glyph sheet needs on top of English.
const I18N_SHEET_FONTS: [deskwarden::i18n::Fonts; 5] = [
    deskwarden::i18n::Fonts::Greek,
    deskwarden::i18n::Fonts::ChineseSimplified,
    deskwarden::i18n::Fonts::ChineseTraditional,
    deskwarden::i18n::Fonts::Japanese,
    deskwarden::i18n::Fonts::Korean,
];

static I18N_SHEET_FONTS_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A test catalog per language, in the real catalog format: a title, a
/// sentence, a button word and a plural. Parsed by the same reader the app
/// uses, and filled by the same `fill`.
const I18N_SHEET: [(&str, &str); 8] = [
    (
        "en",
        "title = Welcome back\nbody = Enter your vault password to unlock your vault.\nbutton = Unlock\nitems[one] = {n} item\nitems[other] = {n} items\n",
    ),
    (
        "ja",
        "title = おかえりなさい\nbody = マスターパスワードを入力して保管庫のロックを解除してください。\nbutton = ロック解除\nitems[other] = {n} 件のアイテム\n",
    ),
    (
        "zh-Hans",
        "title = 欢迎回来\nbody = 输入主密码以解锁您的密码库。\nbutton = 解锁\nitems[other] = {n} 个项目\n",
    ),
    (
        "zh-Hant",
        "title = 歡迎回來\nbody = 輸入主密碼以解鎖您的密碼庫。\nbutton = 解鎖\nitems[other] = {n} 個項目\n",
    ),
    (
        "ko",
        "title = 다시 오신 것을 환영합니다\nbody = 마스터 비밀번호를 입력하여 보관함을 잠금 해제하세요.\nbutton = 잠금 해제\nitems[other] = {n}개 항목\n",
    ),
    (
        "ru",
        "title = С возвращением\nbody = Введите мастер-пароль, чтобы разблокировать хранилище.\nbutton = Разблокировать\nitems[one] = {n} элемент\nitems[few] = {n} элемента\nitems[many] = {n} элементов\nitems[other] = {n} элемента\n",
    ),
    (
        "el",
        "title = Καλώς ήρθατε ξανά\nbody = Εισαγάγετε τον κύριο κωδικό πρόσβασης για να ξεκλειδώσετε το θησαυροφυλάκιό σας.\nbutton = Ξεκλείδωμα\nitems[one] = {n} στοιχείο\nitems[other] = {n} στοιχεία\n",
    ),
    (
        "vi",
        "title = Chào mừng trở lại\nbody = Nhập mật khẩu chính để mở khóa kho của bạn.\nbutton = Mở khóa\nitems[other] = {n} mục\n",
    ),
];

/// The glyph sheet: one band per language, each in the app's four weights.
fn draw_i18n_fonts(root: &mut egui::Ui) {
    use deskwarden::i18n::{self, plural::Category};
    use deskwarden::kit::tokens;
    use egui::{vec2, CornerRadius, Rect, UiBuilder};

    let p = tokens::palette(root.ctx());
    let area = root.max_rect();
    root.painter().rect_filled(area, CornerRadius::ZERO, p.window_bg);
    root.scope_builder(UiBuilder::new().max_rect(area.shrink2(vec2(28.0, 20.0))), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
        ui.label(theme::bold("Translation fonts: every script the app draws", 18.0).color(theme::INK));
        ui.add_space(4.0);
        for (tag, text) in I18N_SHEET {
            let locale = i18n::locale(tag).expect("a shipped language");
            let parsed = i18n::catalog::parse(text);
            assert!(parsed.problems.is_empty(), "{tag}: {:?}", parsed.problems);
            let get = |key: &str| parsed.catalog.get(key, None).unwrap_or("?");
            let band = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 100.0));
            ui.painter().rect_filled(band, CornerRadius::same(tokens::R_CARD), p.card);
            ui.scope_builder(UiBuilder::new().max_rect(band.shrink2(vec2(18.0, 10.0))), |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
                ui.horizontal(|ui| {
                    ui.label(theme::semibold(format!("{tag}  {}", locale.native_name), 12.0).color(theme::TEXT_MUTED));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    ui.label(theme::extrabold(get("title"), 22.0).color(theme::INK));
                    ui.label(theme::bold(get("button"), 15.0).color(theme::BLUE));
                    ui.label(theme::semibold(get("button"), 15.0).color(theme::BLUE));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    ui.label(theme::regular(get("body"), 14.0).color(theme::INK));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for n in [1u64, 2, 5, 21] {
                        let category = locale.rule.category(n);
                        let template = parsed
                            .catalog
                            .get("items", Some(category))
                            .or_else(|| parsed.catalog.get("items", Some(Category::Other)))
                            .unwrap_or("?");
                        let line = i18n::fill(template, &[("n", &n.to_string())]);
                        ui.label(theme::regular(format!("{line}  [{}]", category.name()), 13.0).color(theme::TEXT_MUTED));
                    }
                });
            });
            ui.advance_cursor_after_rect(band);
        }
    });
}

fn draw_kit_sheet(root: &mut egui::Ui) {
    use deskwarden::kit::{card, controls, icon, tips, toast, tokens};
    use egui::{pos2, vec2, CornerRadius, Rect, UiBuilder};

    let p = tokens::palette(root.ctx());
    let area = root.max_rect();
    root.painter().rect_filled(area, CornerRadius::ZERO, p.window_bg);

    // 1. Both sidebars, side by side.
    for (i, style) in [tokens::SidebarStyle::Navy, tokens::SidebarStyle::Light].into_iter().enumerate() {
        let sb = style.palette();
        let strip = Rect::from_min_size(area.min + vec2(24.0 + i as f32 * 268.0, 24.0), vec2(252.0, 400.0));
        root.painter().rect_filled(strip, CornerRadius::same(tokens::R_CARD), sb.bg);
        root.scope_builder(UiBuilder::new().max_rect(strip.shrink2(vec2(14.0, 22.0))), |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                let (mark, _) = ui.allocate_exact_size(vec2(28.0, 32.0), egui::Sense::hover());
                theme::paint_mark_colours(ui.painter(), mark, theme::mark_on_sidebar(sb.q1));
                ui.add_space(11.0);
                ui.label(tokens::job(tokens::WORDMARK, "Deskwarden", sb.text));
            });
            ui.add_space(18.0);
            let (new_item, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), egui::Sense::hover());
            ui.painter().rect_filled(new_item, CornerRadius::same(12), sb.new_bg);
            let plus = Rect::from_min_size(new_item.min + vec2(16.0, 14.0), vec2(18.0, 18.0));
            icon::stroke(ui.painter(), plus, icon::glyph::PLUS, 2.4, sb.new_fg);
            let label = ui.painter().layout_job(tokens::job(tokens::BUTTON, "New item", sb.new_fg));
            ui.painter().galley(pos2(plus.right() + 10.0, new_item.center().y - label.size().y / 2.0), label, sb.new_fg);
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                card::section_label(ui, "Vault", sb.muted);
            });
            for (row, (glyph, name, count, active)) in [
                (icon::glyph::ALL, "All items", "12", true),
                (icon::glyph::FAV, "Favorites", "3", false),
                (icon::glyph::LOGIN, "Logins", "9", false),
            ]
            .into_iter()
            .enumerate()
            {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), egui::Sense::hover());
                let fg = if active { sb.active_fg } else { sb.nav_fg };
                if active {
                    ui.painter().rect_filled(rect, CornerRadius::same(12), sb.active);
                }
                let glyph_rect = Rect::from_min_size(rect.min + vec2(12.0, 12.5), vec2(19.0, 19.0));
                icon::stroke(ui.painter(), glyph_rect, glyph, 2.0, fg);
                let text = ui.painter().layout_job(tokens::job(tokens::NAV, name, fg));
                ui.painter().galley(pos2(glyph_rect.right() + 12.0, rect.center().y - text.size().y / 2.0), text, fg);
                let n = ui.painter().layout_job(tokens::job(tokens::LABEL, count, fg.gamma_multiply(0.7)));
                ui.painter().galley(pos2(rect.right() - 12.0 - n.size().x, rect.center().y - n.size().y / 2.0), n, fg);
                let _ = row;
            }
            ui.add_space(12.0);
            let (lock, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), egui::Sense::hover());
            let glyph_rect = Rect::from_min_size(lock.min + vec2(12.0, 12.5), vec2(19.0, 19.0));
            icon::stroke(ui.painter(), glyph_rect, icon::glyph::LOCK, 2.0, sb.nav_fg);
            let text = ui.painter().layout_job(tokens::job(tokens::NAV, "Lock", sb.nav_fg));
            ui.painter().galley(pos2(glyph_rect.right() + 12.0, lock.center().y - text.size().y / 2.0), text, sb.nav_fg);
        });
    }

    // 2. The mark ramp, then reversed on deep blue.
    let mut x = area.left() + 580.0;
    let base = area.top() + 24.0 + 56.0;
    for (w, h) in [(48.0, 56.0), (32.0, 37.0), (22.0, 26.0), (14.0, 16.0)] {
        theme::paint_mark_colours(root.painter(), Rect::from_min_size(pos2(x, base - h), vec2(w, h)), theme::QUADRANT_FILLS);
        x += w + 26.0;
    }
    let tile = Rect::from_min_size(pos2(x + 10.0, area.top() + 24.0), vec2(92.0, 92.0));
    root.painter().rect_filled(tile, CornerRadius::same(24), p.blue_deep);
    theme::paint_mark_colours(root.painter(), Rect::from_center_size(tile.center(), vec2(46.0, 53.0)), theme::MARK_REVERSED);

    // 3-6. Controls, a card, help and a hint, laid out to the right.
    let column = Rect::from_min_max(pos2(area.left() + 580.0, area.top() + 140.0), pos2(area.right() - 24.0, area.bottom() - 110.0));
    root.scope_builder(UiBuilder::new().max_rect(column), |ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 14.0);
        ui.horizontal(|ui| {
            let (mut on, mut off) = (true, false);
            controls::switch(ui, &mut on);
            controls::switch(ui, &mut off);
            controls::segmented(ui, &["Light", "Dark", "System"], 2, controls::SEG_SETTINGS);
        });
        ui.horizontal(|ui| {
            controls::button(ui, controls::ButtonKind::Primary, controls::LARGE, "Save", None);
            controls::button(ui, controls::ButtonKind::Outline, controls::LARGE, "Cancel", None);
            controls::button(ui, controls::ButtonKind::Tinted, controls::SMALL, "Generate", Some(icon::glyph::GEN));
            controls::button(ui, controls::ButtonKind::Danger, controls::ROW, "Delete forever", None);
        });
        card::card(ui, tokens::R_CARD, egui::Margin::symmetric(24, 18), |ui| {
            ui.set_width(560.0);
            card::field_label(ui, "Username");
            ui.label(tokens::job(tokens::FIELD_VALUE, "a.novak", p.ink));
            card::row_rule(ui);
            ui.horizontal(|ui| {
                card::field_label(ui, "Fills in");
                let dot = tips::help_dot(ui, egui::Id::new("sheet-tip"), "", 260.0, tips::Placement::Above).rect;
                tips::paint_help_bubble(
                    ui,
                    dot,
                    "Deskwarden fills these apps and sites when you press the shortcut.",
                    260.0,
                    tips::Placement::Above,
                );
            });
            ui.horizontal(|ui| {
                for size in [card::TagSize::List, card::TagSize::Detail] {
                    card::tag(ui, card::TagKind::App, size);
                    card::tag(ui, card::TagKind::TwoFa, size);
                    card::tag(ui, card::TagKind::Shared, size);
                }
            });
        });
        ui.add_space(60.0);
        let search = ui.allocate_exact_size(vec2(60.0, 26.0), egui::Sense::hover()).0;
        ui.painter().rect_stroke(search, CornerRadius::same(6), egui::Stroke::new(1.0, p.kbd_border), egui::StrokeKind::Inside);
        tips::paint_hint(ui, search, "Quick search", &["Mod", "K"]);
    });

    // 7. A toast, at the bottom.
    toast::paint_message(root.ctx(), "Folder created");
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



/// A favicon with TRANSPARENT MARGINS, taken through the app's own decoder.
///
/// The shape most real favicons have, and the one the avatar tile's fill and
/// border exist for: a mark inset inside its own canvas with nothing around
/// it. Painted with no tile behind it, artwork like this floats on whatever
/// the pane's background happens to be and has no edge of its own -- which is
/// what [`Surface::LoginDetailFavicon`] is a picture of.
fn inset_favicon(ctx: &egui::Context, name: &str, side: usize) -> egui::TextureHandle {
    let centre = (side as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let d = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // A disc at 62% of the canvas, and a lighter ring inside it, so
            // the mark is unmistakably artwork and the margin around it is
            // unmistakably nothing.
            let paint = if (0.20 * side as f32..0.26 * side as f32).contains(&d) {
                [0xff, 0xff, 0xff, 0xff]
            } else if d < 0.31 * side as f32 {
                [0x1f, 0x6f, 0x5c, 0xff]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&paint);
        }
    }
    let mut file = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut file, side as u32, side as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("the fixture favicon writes a PNG header")
            .write_image_data(&rgba)
            .expect("the fixture favicon writes its pixels");
    }
    let (w, h, decoded) =
        deskwarden::favicon::decode_rgba(&file).expect("the shipped decoder reads the fixture");
    ctx.load_texture(
        format!("preview-inset-favicon-{name}"),
        egui::ColorImage::from_rgba_unmultiplied([w, h], &decoded),
        egui::TextureOptions::LINEAR,
    )
}

/// A stand-in favicon of a REAL source SHAPE, taken through the app's own
/// decoder.
///
/// **Why [`stand_in_favicon`] is not enough.** That one hands the tile a
/// texture built by hand at exactly 64x64 -- which is one of the shapes
/// `favicon::decode_rgba` emits, and the only one it never has to CHANGE
/// anything to reach. A real favicon is a file: a 180x180
/// `apple-touch-icon.png`, or the wide wordmark a page declares in its `<link
/// rel="icon">`. Those are downscaled and then letterboxed onto a transparent
/// square, and what the tile is finally handed is that function's output. A
/// report about an icon sitting high in its tile is a report about that
/// pipeline, so the picture that answers it has to run the pipeline.
///
/// So this builds the FILE, at `(w, h)`, and decodes it with the shipped
/// `favicon::decode_rgba`.
///
/// **The artwork is a measuring stick, not a logo.** A border hugging all four
/// edges, with a wedge pointing UP inside it and a bar along the bottom edge:
/// an icon drawn top-anchored loses the bottom border and the bar, an icon
/// stretched to a square shows the wedge fallen over, and an icon that is
/// right shows an even border on all four sides. None of those three is a
/// judgement call.
fn shaped_favicon(
    ctx: &egui::Context,
    name: &str,
    (w, h): (usize, usize),
    ground: egui::Color32,
) -> egui::TextureHandle {
    let edge = (w.min(h) as f32 * 0.09).max(2.0) as usize;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let border = x < edge || y < edge || x + edge >= w || y + edge >= h;
            // A wedge pointing up, drawn in the middle band, plus a solid bar
            // just above the bottom border.
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let wedge = (0.30..0.70).contains(&fy)
                && ((fx - 0.5).abs() < (fy - 0.30) * 0.7)
                && ((fx - 0.5).abs() > (fy - 0.30) * 0.7 - 0.10);
            let bar = (0.78..0.86).contains(&fy) && (0.25..0.75).contains(&fx);
            let paint = if border || wedge || bar {
                [0xff, 0xff, 0xff, 0xff]
            } else {
                [ground.r(), ground.g(), ground.b(), 0xff]
            };
            rgba.extend_from_slice(&paint);
        }
    }
    let mut file = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut file, w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("the fixture favicon writes a PNG header")
            .write_image_data(&rgba)
            .expect("the fixture favicon writes its pixels");
    }
    let (dw, dh, decoded) =
        deskwarden::favicon::decode_rgba(&file).expect("the shipped decoder reads the fixture");
    ctx.load_texture(
        format!("preview-shaped-favicon-{name}"),
        egui::ColorImage::from_rgba_unmultiplied([dw, dh], &decoded),
        egui::TextureOptions::LINEAR,
    )
}

/// The icon cache [`Surface::VaultListKinds`] draws against: a favicon of a
/// different SOURCE SHAPE on each of the three items whose fixture id says it
/// wants one, and nothing on the rest -- so the kinds' marks and the shapes'
/// geometry are one picture.
///
/// The shapes are read off the ids rather than a list written here, so a row
/// added to [`KINDS_JSON`] cannot quietly go without the icon it was added
/// for.
fn shaped_icons(ctx: &egui::Context, items: &[VaultItem]) -> item_list::IconCache {
    let mut cache = item_list::IconCache::default();
    for item in items {
        // 180x180 is `apple-touch-icon.png`'s own size, 128x44 is the shape
        // of a page-declared wordmark, and 44x128 is that lying down -- the
        // three shapes a real fetch comes back with that are not a square.
        //
        // **16x16 is the fourth, and it is the one the sizing rule is FOR.**
        // It is still what most sites serve from `/favicon.ico`, and it is
        // the only one of the four that comes out of `favicon::decode_rgba`
        // SMALLER than the 32pt tile -- the other three are reduced to a 64px
        // longest edge and so still fill it. Without a row like this the
        // preview cannot show the difference between "never upscale" and
        // "unchanged": every icon in the shot would be full bleed either way,
        // which is exactly the picture that would have let this ship broken.
        let shape = match item.id.as_str() {
            "kinds-login-touch" => Some(((180, 180), egui::Color32::from_rgb(0x1f, 0x6f, 0x5c))),
            "kinds-login-wide" => Some(((128, 44), egui::Color32::from_rgb(0x8a, 0x2f, 0x3c))),
            "kinds-login-tall" => Some(((44, 128), egui::Color32::from_rgb(0x33, 0x3d, 0x8a))),
            "kinds-login-small" => Some(((16, 16), egui::Color32::from_rgb(0x7a, 0x4a, 0x1f))),
            _ => None,
        };
        if let Some((size, ground)) = shape {
            cache.textures.insert(item.id.clone(), shaped_favicon(ctx, &item.id, size, ground));
        }
    }
    cache
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

/// The Sends fixture, **through the shipped `bw send list` parser**.
///
/// Not a hand-built `Vec<SendSummary>`, for the reason every fixture in this
/// file is a wire literal: a `SendSummary` written out by hand cannot stop
/// describing the real thing, so it cannot warn anybody when the real thing
/// changes. Taken through `parse_send_list`, a fixture that stops parsing is
/// a fixture that stopped being what `bw` emits -- and `is_file`,
/// `has_password` and the state derivation are then computed by the exact
/// code the app runs, rather than asserted into the picture by the preview.
fn sends(json: &str) -> Vec<send::SendSummary> {
    send::parse_send_list(json).expect("the preview's Sends fixture must parse as a bw send list")
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

/// **Design 8a's Ledgerline, as far as this app can model it.**
///
/// A login with a username, a password, a one-time code, a website, a bound
/// native app and two custom fields -- so every card on the redrawn form has
/// something in it and none of them is a picture of an empty state. 8a's own
/// record is this one: the design's `ledgerline.exe` target is here, its
/// `Client ID` and `Security PIN` fields are here.
///
/// **Its own fixture rather than a field added to [`LOGIN_JSON`]**, because
/// that one is the READ pane's subject in three shots and a binding added to
/// it would silently redraw all three.
///
/// 8a's other two native targets are deliberately absent: `AppMatchDraft`
/// models exactly one app per record, in a single JSON field, and a fixture
/// carrying three would be a picture of a feature that does not exist.
///
/// **And three previous passwords**, because 8a's credentials card says
/// `Password history (3)` and there was no fixture in this file carrying a
/// `passwordHistory` at all -- not for this form and not for the read pane,
/// which has drawn the same array since it learned to. The three are
/// deliberately unlike each other: a recent one, one a year back, and one with
/// **no `lastUsedDate`**, which is the case `detail::history_label` falls back
/// to `Earlier` for and which nothing in this directory pictured.
///
/// **These two dates drift**, and the shot has to be read knowing it. The
/// wording comes from `relative_time::ago`, which is relative to
/// `SystemTime::now()`, so `3d` reads `9d` a week later; there is no fixed
/// clock to pass it the way `PREVIEW_UNIX` is passed to the one-time-code
/// card. What does not drift is everything the shot is FOR -- the count in the
/// caption, the row rhythm, the mask, and where the list sits on the card.
const EDIT_LOGIN_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a002",
  "type": 1,
  "name": "Ledgerline",
  "folderId": "f-work",
  "favorite": true,
  "notes": "Workspace SSO is disabled for the desktop client - use the direct login. Recovery codes live in the Engineering secure note.",
  "login": {
    "username": "a.novak@ledgerline.com",
    "password": "correct-horse-battery-staple-7",
    "totp": "otpauth://totp/Ledgerline:a.novak?secret=JBSWY3DPEHPK3PXP&issuer=Ledgerline",
    "uris": [{ "uri": "https://app.ledgerline.com" }]
  },
  "fields": [
    { "name": "Client ID", "value": "LGL-4471", "type": 0 },
    { "name": "Security PIN", "value": "419077", "type": 1 },
    { "name": "deskwarden:app-match", "value": "{\"process\":\"ledgerline.exe\",\"trigger\":\"auto\"}", "type": 0 }
  ],
  "passwordHistory": [
    { "password": "correct-horse-battery-staple-6", "lastUsedDate": "2026-09-08T09:12:00.000Z" },
    { "password": "Tr0ub4dor&3", "lastUsedDate": "2025-06-02T17:41:00.000Z" },
    { "password": "ledgerline2024" }
  ]
}"#;

/// A login reached at three addresses, each entry carrying a `match` key this
/// form has no editor for -- see `detail_edit::UriDraft`. The keys are on the
/// fixture on purpose: a preview of a block that preserves nothing would look
/// exactly like this one.
const WEBSITES_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a003",
  "type": 1,
  "name": "Ledgerline",
  "folderId": "f-work",
  "login": {
    "username": "a.novak@ledgerline.com",
    "password": "correct-horse-battery-staple-7",
    "uris": [
      { "uri": "https://app.ledgerline.eu/signin", "match": 0 },
      { "uri": "https://sso.ledgerline.eu", "match": 1 },
      { "uri": "androidapp://eu.ledgerline.mobile", "match": null }
    ]
  },
  "fields": []
}"#;

/// An identity with a name, an email and a phone number on it -- four of the
/// eighteen fields Bitwarden models -- and nothing else. That ratio is the
/// ordinary one, and it is why the form's old shape was fourteen empty boxes
/// deep.
const SPARSE_IDENTITY_JSON: &str = r#"{
  "id": "6f1c2f5e-0000-4a10-9c31-2b7a51d0a004",
  "type": 4,
  "name": "Anna Novak",
  "folderId": "f-personal",
  "identity": {
    "firstName": "Anna",
    "lastName": "Novak",
    "email": "a.novak@ledgerline.com",
    "phone": "+31 20 555 0142"
  },
  "fields": []
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
  },
  "fields": [
    { "name": "deskwarden:bank-domain", "value": "brex.com", "type": 0 }
  ]
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


/// **One row of every kind the app knows, and one it does not** -- see
/// [`Surface::VaultListKinds`].
///
/// The three logins at the top carry favicons of three different SOURCE
/// shapes (see [`shaped_icons`]); the fourth carries none, so the monogram a
/// login still falls back to is in the same picture as the marks. The card is
/// branded, because a card's network stays on its trailing pill and the shot
/// has to show the generic card mark and the `VISA` pill coexisting.
const KINDS_JSON: &str = r#"[
  {
    "id": "kinds-login-touch", "type": 1, "name": "Harbourline Rail",
    "login": { "username": "anna@harbourline.example", "password": "x",
      "uris": [{ "uri": "https://harbourline.example" }] }
  },
  {
    "id": "kinds-login-wide", "type": 1, "name": "Meridian Freight",
    "login": { "username": "ops@meridian.example", "password": "x",
      "uris": [{ "uri": "https://meridian.example" }] }
  },
  {
    "id": "kinds-login-tall", "type": 1, "name": "Cantilever Studio",
    "login": { "username": "anna@cantilever.example", "password": "x",
      "uris": [{ "uri": "https://cantilever.example" }] }
  },
  {
    "id": "kinds-login-small", "type": 1, "name": "Ashgrove Books",
    "login": { "username": "anna@ashgrove.example", "password": "x",
      "uris": [{ "uri": "https://ashgrove.example" }] }
  },
  {
    "id": "kinds-login-plain", "type": 1, "name": "Northwind Mail",
    "login": { "username": "anna@northwind.example", "password": "x" }
  },
  {
    "id": "kinds-card", "type": 3, "name": "Ledgerline corporate card",
    "card": { "brand": "Visa", "number": "4111111111111111", "expMonth": "11",
      "expYear": "2029" }
  },
  {
    "id": "kinds-note", "type": 2, "name": "Datacentre recovery codes",
    "notes": "Eight one-time codes, used top to bottom."
  },
  {
    "id": "kinds-identity", "type": 4, "name": "Anna Novak",
    "identity": { "firstName": "Anna", "lastName": "Novak",
      "email": "anna@northwind.example" }
  },
  {
    "id": "kinds-ssh", "type": 5, "name": "Build server key",
    "sshKey": { "keyFingerprint": "SHA256:3Xr0ZQ4l9tKcE2mBw7YvHn1sPd6aUqRf8gTjLxCiN0k" }
  },
  {
    "id": "kinds-unknown", "type": 9, "name": "Something newer than this app"
  }
]"#;

/// **Design 5b's own five Sends**, in `bw send list`'s wire shape, plus one
/// this app could not have made.
///
/// The five are 5b's, name for name, and their dates are stated relative to
/// [`PREVIEW_UNIX`] so each one derives -- through the shipped
/// `send::send_state` and not through anything written here -- to the state
/// 5b draws beside it:
///
/// * **SAP Production** is `Used`: one view of one is spent, which is the
///   state that strikes its address through, fills its meter and makes its
///   activity sentence say something.
/// * **Office WiFi** and **Postgres** are `Waiting` and carry the two
///   different subtitles a live Send has -- a view count, and a deadline.
/// * **Remote Desktop** is `Expired`: its `expirationDate` has gone by while
///   its `deletionDate` has not, which is the gap this client reads two dates
///   to see and the one a screen reading `deletionDate` alone would call
///   live.
/// * **Atlas Studio** is `Revoked`, which is `disabled` and NOT deleted.
///
/// The sixth is a **file Send**, which 5b has none of and this screen must
/// show: this app cannot create one, so the only way a user learns that a
/// public link exists on their account is that it is listed here with its
/// `FILE` tag and the paragraph that explains it. A fixture without one would
/// leave both of those unlooked-at forever, which is precisely how they came
/// to need looking at.
const SENDS_JSON: &str = r#"[
  {
    "id": "send-sap", "name": "SAP Production", "type": 0,
    "accessUrl": "https://send.deskwarden.app/g7HqK2mV3nQ8xLpR4tYw",
    "deletionDate": "2023-11-15T22:13:00.000Z",
    "expirationDate": "2023-11-15T21:13:00.000Z",
    "maxAccessCount": 1, "accessCount": 1, "disabled": false,
    "password": "$argon2id$v=19$m=65536,t=3,p=4$notarealhash"
  },
  {
    "id": "send-wifi", "name": "Office WiFi — Guest", "type": 0,
    "accessUrl": "https://send.deskwarden.app/4bNc8sQ1zEfT6uJvHy2d",
    "deletionDate": "2023-11-20T22:13:00.000Z",
    "expirationDate": "2023-11-20T22:13:00.000Z",
    "maxAccessCount": 10, "accessCount": 3, "disabled": false
  },
  {
    "id": "send-postgres", "name": "Postgres — Prod", "type": 0,
    "accessUrl": "https://send.deskwarden.app/Kq9WmD3pXa7RbZt5Ln0c",
    "deletionDate": "2023-11-21T22:13:00.000Z",
    "expirationDate": "2023-11-15T01:13:00.000Z",
    "accessCount": 0, "disabled": false,
    "password": "$argon2id$v=19$m=65536,t=3,p=4$notarealhash"
  },
  {
    "id": "send-bastion", "name": "Remote Desktop — Bastion", "type": 0,
    "accessUrl": "https://send.deskwarden.app/P2yTf6VsMw9hJd1kGr4b",
    "deletionDate": "2023-11-16T22:13:00.000Z",
    "expirationDate": "2023-11-14T19:13:00.000Z",
    "maxAccessCount": 5, "accessCount": 0, "disabled": false
  },
  {
    "id": "send-atlas", "name": "Atlas Studio", "type": 0,
    "accessUrl": "https://send.deskwarden.app/Ub7NxQ0eLc2SmFj8Zt5v",
    "deletionDate": "2023-11-21T22:13:00.000Z",
    "expirationDate": "2023-11-21T22:13:00.000Z",
    "accessCount": 2, "disabled": true
  },
  {
    "id": "send-boardpack", "name": "Q3 board pack.pdf", "type": 1,
    "accessUrl": "https://send.deskwarden.app/Hv3RjE8wYn6tQz1aDs9m",
    "deletionDate": "2023-11-20T22:13:00.000Z",
    "expirationDate": "2023-11-20T22:13:00.000Z",
    "maxAccessCount": 2, "accessCount": 0, "disabled": false
  }
]"#;

/// The `Shared with me` screen's records.
///
/// Built rather than parsed from a literal, because this one is not a wire
/// shape at all: `crate::receive_history` is a file this app writes itself,
/// and its timestamps are milliseconds rather than the CLI's ISO strings.
/// They are stated relative to [`PREVIEW_UNIX`] for the same reason every
/// other date here is -- a shot carrying "today" is a shot no reviewer can
/// diff against the last one.
///
/// **The first two name items that are in [`LIST_JSON`] and the third names
/// one that is not.** That is the whole point of the third: the history
/// outlives the item, and the row whose item has gone is an ordinary state of
/// this screen rather than an error -- a picture with only the happy rows in
/// it would never show the sentence that says so.
fn received_history() -> ReceiveHistory {
    ReceiveHistory {
        entries: vec![
            ReceivedRecord {
                received_at_unix_millis: 1_699_997_580_000,
                name: "Ledgerline".to_string(),
                item_id: "list-0001".to_string(),
            },
            ReceivedRecord {
                received_at_unix_millis: 1_699_740_780_000,
                name: "Northwind staging keys".to_string(),
                item_id: "list-0002".to_string(),
            },
            ReceivedRecord {
                received_at_unix_millis: 1_699_222_380_000,
                name: "Contractor VPN profile".to_string(),
                item_id: "gone-from-the-vault".to_string(),
            },
        ],
    }
}

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

/// A long list scrolled part-way, in a card, under the theme's scroll bar.
/// A still has no pointer, so the dormant opacity is lifted to the active one:
/// the bar as it looks while the pointer is over the list.
fn draw_scrolled_list(root: &mut egui::Ui) {
    let p = deskwarden::kit::tokens::palette(root.ctx());
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.window_bg).inner_margin(24)).show(root, |ui| {
        deskwarden::kit::tokens::card_frame(&p, 18).inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.dormant_handle_opacity = scroll.active_handle_opacity;
            egui::ScrollArea::vertical().max_height(400.0).vertical_scroll_offset(260.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for i in 0..40 {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), egui::Sense::hover());
                    ui.painter().text(
                        rect.left_center() + egui::vec2(12.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        format!("Row {i}"),
                        egui::FontId::new(15.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
                        p.ink,
                    );
                }
            });
        });
    });
}

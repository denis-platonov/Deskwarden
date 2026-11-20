//! **Adding a one-time code to an item that already exists, by hand** --
//! design section 6d's left half, and the 6c confirmation every later route
//! shares.
//!
//! # What this file is
//!
//! One field that takes **either** a base32 secret **or** a whole
//! `otpauth://` URI, two parameter controls, and a confirmation card that
//! shows what was read back **with a live code and its countdown**. The
//! confirmation is the point of the whole feature: a seed typed off a card,
//! or read out of a QR, is only known to be right when the code it produces
//! matches the one the site is showing at that moment. Everything else here
//! exists so that check can be made **before** anything is written.
//!
//! # Nothing here decides anything inside a closure
//!
//! Every answer this surface paints is a pure function of what was typed --
//! [`read_field`], [`validity_line`], [`refusal_sentence`], [`masked`],
//! [`code_at`], [`seconds_left`], [`uri_to_write`] -- and the two `draw_*`
//! functions do nothing but paint those answers. That is `record_ui`'s rule
//! and it is kept for its reason: a rule written inside an `eframe` closure is
//! a rule no test in this crate can run.
//!
//! # The parse is [`crate::otpauth`]'s, always
//!
//! A typed *secret* is not validated by a second base32 reader written here.
//! It is percent-encoded into an `otpauth://` URI and handed to
//! [`parse_otpauth`], so the by-hand route and the scanned route are refused
//! by the **same** parser, with the same strictness, for the same reasons. A
//! second validator is how the two routes come to disagree about what a valid
//! seed is -- and the URI is what gets written anyway.
//!
//! # The live code is computed HERE, and why that is not the vault's job
//!
//! `vault_bridge::get_totp` asks `bw serve` for an item's current code, and
//! that is the right answer everywhere else in this app. It cannot be the
//! answer here: **the secret being confirmed is not saved yet.** There is no
//! item to ask about, and the entire purpose of this screen is to decide
//! whether to create one. Asking the vault would mean writing the seed first
//! and checking afterwards, which inverts the confirmation into "save, then
//! find out" -- and the thing being saved is destructive when the item
//! already has a code.
//!
//! So [`code_at`] computes HMAC-based one-time codes (RFC 4226/6238) from the
//! `sha1`/`sha2` crates **already in this crate's tree**. No new dependency,
//! including no `hmac` crate: HMAC is nine lines over a digest and they are
//! [`hmac`] below, pinned against the RFC 2202/4231 vectors as well as
//! against RFC 6238's own TOTP vectors for all three algorithms.
//!
//! **This is the crate's second use of SHA-1 and the first that is a
//! primitive rather than an index.** `breach.rs`'s
//! `sha1_is_confined_to_the_breach_module` is re-pinned to name this file, and
//! its carve-out controls are extended to it -- see that test. The name is
//! carried from the card; the HMAC under it is the algorithm RFC 6238 names,
//! and refusing to compute it would mean shipping a confirmation screen that
//! cannot confirm the default case.
//!
//! # The seed
//!
//! Everything holding one is a [`Zeroizing`]: the typed text, the URI built
//! from it, the decoded key bytes, the HMAC's own buffers and the rendered
//! code. The masked form is the default and [`TotpAdd::revealed`] starts
//! `false`.

use crate::otpauth::{parse_otpauth, to_uri, Algorithm, OtpAuth, OtpRefusal};
use crate::region_overlay::Outcome;
use crate::screen_capture::CaptureRefusal;
use crate::theme;
use crate::webcam::{CameraRefusal, Session, WebcamSeams};
use eframe::egui::{self, CornerRadius};
use zeroize::Zeroizing;

mod ui;
pub use ui::{
    draw_add_form, draw_add_modal, draw_picker, draw_scanning, draw_stage, draw_webcam, PickerFrame, RouteCard,
    code_copy_hint, route_cards,
};

// The 26px button height this form used to declare is gone with the two bare
// `egui::Button`s it sized. 6d's footer answers are `theme`'s own -- see
// `manual_footer` -- and `theme::BUTTON_HEIGHT` is 6d's declared 32.

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// **The card's header band while the secret is being typed**, verbatim from
/// design 6d's own (`Enter the secret`, set `font-size: 14px; font-weight:
/// 700` over a `1px solid #eae7e7` rule).
///
/// **Not the "Add a TOTP" this used to say.** That phrase is
/// [`ADD_TOTP_LABEL`] -- the name of the *control that opens this modal* --
/// and a card headed with the name of the button that opened it says nothing
/// about what the card is for. 6d's own header says what to do next, and the
/// same band switches to 6c's [`CODE_READ_LABEL`] when a decoder has already
/// done the typing.
///
/// `PICKER_TITLE`'s note applies here from the other side: "Manual entry &
/// failures" is the design document's CAPTION for the `id="6d"` panel, in the
/// badge-and-title row outside the card, and is not painted anywhere.
pub fn heading() -> &'static str {
    crate::i18n::t("totp.heading")
}

/// What the control that opens this form is CALLED, and the chord that also
/// opens it.
///
/// **The control itself is not on screen yet, and this file says so rather
/// than pretending otherwise.** It belongs in the detail pane's own header
/// strip beside the ✉, which is where the record composer's control ended up
/// after the titlebar pill was rejected for acting on the selected item from a
/// strip of window-wide controls -- adding a code acts on the selected item in
/// exactly the same way. That strip is `detail.rs`'s, and `detail.rs` is being
/// edited elsewhere as this lands. The chord is the door that exists today.
///
/// Both constants are here rather than in `vault_window::mod` for
/// `record_ui::SEND_RECORD_SHORTCUT`'s reason: this is the string a human is
/// shown, and
/// `vault_window::tests::the_add_code_chord_is_spelled_the_way_it_is_bound`
/// compares it against `ADD_TOTP_MODIFIERS`/`ADD_TOTP_KEY`, the values the key
/// handler really matches on, so it cannot advertise a binding the code does
/// not have.
pub fn add_totp_label() -> &'static str {
    crate::i18n::t("totp.add_label")
}
/// See [`ADD_TOTP_LABEL`].
pub const ADD_TOTP_SHORTCUT: &str = "CTRL+SHIFT+2";

/// **The caption over the one field**, verbatim from design 6d.
///
/// It is drawn where 6d draws it -- in a `div` above the box, at
/// `SECRET_LABEL_PX` in [`theme::TEXT_MUTED`] -- and not as the
/// `TextEdit::hint_text` it used to be. See `SECRET_BLOCK_GAP` on why the
/// difference is not cosmetic: a placeholder is gone by the second character,
/// and what this sentence says is that the box takes *either* form.
///
/// The name is kept because it is what every test in this file asks for the
/// field by.
pub fn secret_hint() -> &'static str {
    crate::i18n::t("totp.secret_hint")
}

/// **What replaces the field when the payload was scanned**, verbatim from
/// design 6c's own header row.
///
/// A scanned payload is a whole `otpauth://` URI with `secret=` in the middle
/// of it, and 6d's field is a plain `TextEdit` that paints what it holds. Put
/// one in the other and the confirmation's masked secret row is decoration:
/// the seed is already on screen, in the clear, two rows above it. So the
/// scanned routes get 6c's row -- which says what was read without saying what
/// it was -- and the field is drawn only for what the user is typing
/// themselves.
pub fn code_read_label() -> &'static str {
    crate::i18n::t("totp.code_read")
}
/// See [`CODE_READ_LABEL`]. What kind of thing it was.
pub const CODE_READ_KIND: &str = "otpauth://totp";

/// The question printed beside the live code, verbatim from design 6c. It is
/// the reason the code and the countdown are on screen at all.
pub fn match_question() -> &'static str {
    crate::i18n::t("totp.match_question")
}

/// **The only destructive act in this feature, verbatim.**
///
/// Shown whenever the item already has a `totp`, and pinned by content in
/// [`tests::the_replace_warning_is_the_designs_own_sentence`] the way this
/// crate pins its refusal messages. A seed cannot be recovered once it has
/// been overwritten -- "rotating" it means re-enrolling the second factor with
/// the service, which this app can neither do nor offer -- so this sentence is
/// the only place the user is told what pressing the button costs. A reworded
/// one must be a deliberate edit that reds a test, not a tidy-up.
pub fn replace_warning() -> &'static str {
    crate::i18n::t("totp.replace_warning")
}

/// The submit button when the item has no code yet, verbatim from design 6d.
pub fn save_label() -> &'static str {
    crate::i18n::t("totp.save")
}

/// The submit button when there is one to overwrite, verbatim from design 6c.
///
/// A **different word** from [`SAVE_LABEL`] deliberately: the button that
/// destroys something says so on its face, not only in the paragraph above it.
pub fn replace_label() -> &'static str {
    crate::i18n::t("totp.replace")
}

/// **What the footer says at its right-hand end**, verbatim from design 6c's
/// footer row, and it is a promise about a key that really answers:
/// [`draw_add_modal`] returns [`TotpAddAction::Cancel`] on Escape at every
/// stage but [`Stage::Scanning`]. See `manual_footer` for why this line is
/// drawn *instead of* 6d's second button rather than beside it.
pub fn dismiss_hint() -> &'static str {
    crate::i18n::t("totp.dismiss_hint")
}

/// The control that unmasks the secret.
pub fn reveal_label() -> &'static str {
    crate::i18n::t("totp.reveal")
}
/// See [`REVEAL_LABEL`].
pub fn hide_label() -> &'static str {
    crate::i18n::t("totp.hide")
}

/// The label over the masked secret row.
pub fn secret_row_label() -> &'static str {
    crate::i18n::t("totp.row_secret")
}
/// See [`SECRET_ROW_LABEL`].
pub fn issuer_row_label() -> &'static str {
    crate::i18n::t("totp.row_issuer")
}
/// See [`SECRET_ROW_LABEL`].
pub fn account_row_label() -> &'static str {
    crate::i18n::t("totp.row_account")
}
/// See [`SECRET_ROW_LABEL`]. **The parameters are spelled out and not
/// summarised**: a card that is 8 digits over 60 seconds under SHA-256 is
/// exactly the case a confirmation screen exists to catch, and it is
/// invisible if the row says only "TOTP".
pub fn parameters_row_label() -> &'static str {
    crate::i18n::t("totp.row_parameters")
}

/// The label over the live code.
pub fn code_row_label() -> &'static str {
    crate::i18n::t("totp.row_code")
}

/// Why the two controls are dead: the link already said.
///
/// **A sentence, not a fragment.** It read `from the URI`, which is the
/// tail of a sentence whose head was never drawn -- on screen, under two
/// greyed controls, it said nothing at all. The owner read it next to a
/// heading that should not have been there and reported both together.
///
/// It is still the app's own words rather than the design's: 6d draws
/// these controls LIVE and so has no line for this state, and greyed
/// controls with no reason beside them are worse than either.
pub fn parameters_from_uri() -> &'static str {
    crate::i18n::t("totp.parameters_from_uri")
}

/// The captions on the two parameter controls.
pub fn digits_label() -> &'static str {
    crate::i18n::t("totp.digits")
}
/// See [`DIGITS_LABEL`].
pub fn period_label() -> &'static str {
    crate::i18n::t("totp.period")
}

/// The digits a card may ask for.
///
/// **Design 6d draws 6, 7 and 8; this offers 6 and 8.** Seven is not a value
/// [`parse_otpauth`] will read back -- it refuses anything but 6 or 8 by name
/// -- so a 7 written into the vault here would be a URI this app itself
/// cannot re-read, which is worse than a control that never offered it.
pub const DIGITS_CHOICES: [u8; 2] = [6, 8];

/// The periods a card may ask for, from design 6d.
pub const PERIOD_CHOICES: [u16; 2] = [30, 60];

/// The default parameters, RFC 6238's own.
pub const DEFAULT_DIGITS: u8 = 6;
/// See [`DEFAULT_DIGITS`].
pub const DEFAULT_PERIOD: u16 = 30;

// ---------------------------------------------------------------------------
// Design 6a -- the picker
// ---------------------------------------------------------------------------

/// **The title in design 6a's own header band**, verbatim from it.
///
/// Not [`HEADING`]'s "Add a TOTP", which is 6d's heading over the by-hand
/// form, and not the "How to add it" this used to be. That phrase is the
/// design document's CAPTION for the `id="6a"` panel: it sits in the
/// badge-and-title row every panel in that file carries, outside the card and
/// beside the blue "6a" chip, exactly as 6b's "Scanning" caption sits outside
/// 6b's. Painting it inside the card gave the picker two headings where the
/// design draws one.
pub fn picker_title() -> &'static str {
    crate::i18n::t("totp.picker_title")
}

/// Spec B §8: what a saved code says.
pub fn saved_toast() -> &'static str {
    crate::i18n::t("totp.saved_toast")
}

/// The label over the item name at the top of the picker, verbatim from 6a.
pub fn adding_to_label() -> &'static str {
    crate::i18n::t("totp.adding_to")
}

/// **The privacy line, verbatim from design 6a, and it must stay true of the
/// code below it.**
///
/// Each clause is a claim about a specific piece of this feature:
///
/// * *"Decoding happens on this machine"* -- [`crate::qr`] wraps `rqrr`, which
///   is pure Rust with no I/O and no network at all, and nothing on either
///   route sends a byte anywhere.
/// * *"The captured pixels are discarded once the secret is read"* --
///   [`crate::screen_capture::Rgba`] wipes its buffer on drop and
///   [`crate::region_overlay::read_region_with`] drops it before it returns;
///   the image route's own buffers are [`Zeroizing`] and die inside
///   [`decode_image_with`]. Neither route hands pixels to a caller.
/// * *"the secret is never written to disk outside the vault"* -- the decoded
///   string is a [`Zeroizing`] all the way from the decoder to
///   [`uri_to_write`], nothing on either route opens a file for writing, and
///   nothing logs it. The image route **reads** a file the user already had;
///   it creates none.
///
/// The one thing this sentence does **not** claim, because it would not be
/// true: that no copy of the seed's bits ever reaches the allocator. `rqrr`
/// builds un-wiped intermediates during a decode and [`crate::qr::decode_qr`]
/// says so in its own documentation. "Discarded once the secret is read" is a
/// statement about what this app keeps, and that one holds.
pub fn privacy_line() -> &'static str {
    crate::i18n::t("totp.privacy_line")
}

/// The four routes of design 6a, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 6b: drag a box on a dimmed desktop.
    ScanRegion,
    /// A picture the user already has, in any format [`image_to_rgba`] reads.
    ImageFile,
    /// 6d: the form that was already here.
    ByHand,
    /// A camera, a live preview, and the same decoder the other two use.
    /// [`crate::webcam`] owns the device.
    Webcam,
}

/// One row of the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRow {
    /// What pressing it means.
    pub route: Route,
    /// The row's face.
    pub title: &'static str,
    /// The line under the title.
    pub subtitle: &'static str,
    /// Whether the row does anything. `false` is a **visible** deferral --
    /// see [`DEFERRED_REASON`].
    pub enabled: bool,
}

/// **What a row that is drawn and dead says on its face.**
///
/// **Nothing in [`ROUTES`] is deferred today** -- the webcam row was the one
/// that was, and it is now a route like the other three. The machinery is
/// kept, and this constant with it, because the argument for it has not
/// changed and the next deferral should not have to re-invent it: a route the
/// design promises and the product silently omits reads as a bug, since the
/// user looks for it, does not find it, and cannot tell whether they are
/// looking in the wrong place. A row that says what it is and why it is off
/// answers that in one glance, in the place they went looking.
///
/// `no_route_is_deferred_today` pins the first sentence above, and
/// `a_deferred_row_still_says_that_it_is_deferred` paints a synthetic
/// disabled row so that the treatment stays exercised rather than becoming
/// code nothing has run in a year.
pub fn deferred_reason() -> &'static str {
    crate::i18n::t("totp.deferred_reason")
}

/// **Design 6a's four routes, in its order** -- *"ordered by how often they're
/// the right one on Windows"*.
///
/// **The `ImageFile` subtitle names every format the decoder reads, which is
/// now more than 6a's *"PNG, JPG"* rather than less.** It used to say PNG
/// alone, and that was a deliberate edit: the route shipped a `png`-crate
/// decode and no JPEG one, so a row promising JPG over a dialog that would
/// not show one was a promise broken a click later. The owner's answer to
/// that was "all images should work", so the decoder widened
/// ([`image_to_rgba`]) and the copy widened with it. The rule did not change,
/// only which way it points: this row, the file dialog's filter
/// ([`crate::file_picker::QR_FILTER_SPEC`]) and what
/// [`image_to_rgba`] can actually decode say one thing, and a row that hid a
/// format the app reads would be the same defect as one that offered a format
/// it does not.
/// **The first row's copy is a deliberate departure from 6a**, and it follows
/// a change to what the row does rather than a change of mind about how to
/// say it. 6a's row is *"Scan a region of my screen"* over *"Drag a box
/// around the QR code in any window"*, and both sentences were exactly right
/// while pressing the row opened a surface to drag a box on. It no longer
/// does: [`crate::region_overlay`] scans every monitor first and opens design
/// 6b only when it cannot answer -- the owner's *"would be nice if it could
/// recognize the QR itself without drawing a box"*. A row that still promised
/// a box to drag would describe the fallback and not the route.
///
/// The rule the rewrite follows is the same one that widened the image row's
/// format list: this row, the surface it opens and what that surface really
/// does say **one** thing. The drag is still named, because it is still what
/// happens when the scan misses, and a route that turned out to want a
/// gesture the row never mentioned would be the same broken promise one click
/// later.
pub fn routes() -> [RouteRow; 4] {
    [
        RouteRow {
            route: Route::ScanRegion,
            title: crate::i18n::t("totp.route_scan_title"),
            subtitle: crate::i18n::t("totp.route_scan_sub"),
            enabled: true,
        },
        RouteRow {
            route: Route::ImageFile,
            title: crate::i18n::t("totp.route_image_title"),
            subtitle: crate::i18n::t("totp.route_image_sub"),
            enabled: true,
        },
        RouteRow {
            route: Route::ByHand,
            title: crate::i18n::t("totp.route_hand_title"),
            subtitle: crate::i18n::t("totp.route_hand_sub"),
            enabled: true,
        },
        RouteRow {
            route: Route::Webcam,
            title: crate::i18n::t("totp.route_webcam_title"),
            // 6a's own line for this row is about a camera being pointed at
            // something, and what a user needs to know before pressing it is what
            // they will have to hold up. The second clause is the promise
            // [`crate::webcam`] keeps and `PRIVACY.md` repeats: the camera is not
            // on until this row is pressed, and nothing it sees is kept.
            subtitle: crate::i18n::t("totp.route_webcam_sub"),
            enabled: true,
        },
    ]
}

/// What the picker's way back to itself is called, from the two halves that
/// have one.
pub fn other_ways_label() -> &'static str {
    crate::i18n::t("totp.other_ways")
}

/// §6d's own second answer, verbatim: *"Cancel"*.
///
/// The typed card and the scanned one do NOT share a footer any more, and
/// that was the report: "Save code and Cancel buttons, no Esc descards unless
/// same everywhere". §6d draws two buttons and nothing else; §6c draws
/// `Replace code`, a way back, and `Esc discards` pushed to the far right.
/// Fusing them gave the typed card §6c's furniture -- a route back to a
/// picker it did not arrive from, and a hint no other modal in this app
/// prints (`delete_modal`, `folder_modal`, `icon_modal` and the preferences
/// capture all bind Escape and all say nothing about it).
pub fn cancel_label() -> &'static str {
    crate::i18n::t("totp.cancel")
}


/// The heading on the card that stands while the screen is being read.
///
/// **The card came back, and what came back is a spinner rather than an
/// instruction.** The one that was removed said "Scanning your screen -- drag
/// a box", and it was removed because on the path where the whole-screen scan
/// FINDS a code there is never an overlay and never a box to drag: it asked
/// for something that would not exist. The owner then: "the read a screen etc
/// - takes few seconds and not clear what to expect - render the spinner in
///   the meantime maybe on that open modal".
///
/// Both reports are about the same second and a half, and they are not in
/// conflict. What was wrong was the WORDS, not the card: a surface that tells
/// you to drag a box you may never be given is misleading, and a surface that
/// says the app is busy is the plain truth on both paths. So the card says
/// what is happening, the bar says it is still happening, and neither
/// promises a step that may not come. The overlay, when there is one, still
/// says what to do in its own stripe.
pub fn scanning_heading() -> &'static str {
    crate::i18n::t("totp.scanning_heading")
}

/// The line under [`SCANNING_HEADING`]. Says what to expect, which is the
/// whole of what the owner asked for -- a wait with no end in sight is what
/// makes a second and a half feel like a hang.
pub fn scanning_note() -> &'static str {
    crate::i18n::t("totp.scanning_note")
}

/// The heading over the camera preview.
///
/// An instruction rather than a status, for the reason 6b's own bar is an
/// instruction: the user is holding a phone in one hand and has one thing to
/// do with it.
pub fn webcam_heading() -> &'static str {
    crate::i18n::t("totp.webcam_heading")
}

/// The line under [`WEBCAM_HEADING`].
///
/// Deliberately the same two clauses as `region_overlay::DRAG_HINT` -- it
/// reads by itself, and nothing is saved yet -- because they are the same two
/// facts and a user who has tried both routes should not have to learn them
/// twice.
pub fn webcam_hint() -> &'static str {
    crate::i18n::t("totp.webcam_hint")
}

/// What the preview says before the first frame arrives.
///
/// A camera can take two or three seconds to wake, and a black rectangle for
/// that long is indistinguishable from a broken route. See
/// `crate::webcam::FIRST_FRAME_GRACE` for what happens if it never wakes.
pub fn webcam_starting() -> &'static str {
    crate::i18n::t("totp.webcam_starting")
}

/// The heading over the device list, when there is more than one camera.
pub fn webcam_choose() -> &'static str {
    crate::i18n::t("totp.webcam_choose")
}

/// The line under [`WEBCAM_CHOOSE`], which is the promise the list keeps: no
/// device is opened, so no camera light comes on, until one is pressed.
pub fn webcam_choose_hint() -> &'static str {
    crate::i18n::t("totp.webcam_choose_hint")
}

/// The way back to the device list from an open camera, when there is more
/// than one to go back to.
pub fn webcam_another_label() -> &'static str {
    crate::i18n::t("totp.webcam_another")
}

/// **The camera's own privacy line, and it must stay true of
/// [`crate::webcam`].**
///
/// [`PRIVACY_LINE`]'s clauses cover the pixels; this one covers the *device*,
/// which is the thing a user is entitled to be told about in the moment the
/// light comes on. Each clause is a claim about specific code:
///
/// * *"only while this is open"* -- [`TotpAdd::webcam`] is the device's
///   lifetime and `crate::webcam::Session`'s `Drop` stops the capture thread,
///   so there is no state of this app other than this stage in which a camera
///   is open.
/// * *"the picture is read here and thrown away"* --
///   `crate::webcam::Frame`'s buffer is a [`Zeroizing`], the slot it passes
///   through wipes what it replaces, and nothing on this route hands pixels
///   to a caller.
/// * *"nothing is recorded"* -- no path in `crate::webcam` opens a file, and
///   `this_module_writes_nothing_anywhere` reads the module's own source to
///   say so.
pub fn webcam_privacy_line() -> &'static str {
    crate::i18n::t("totp.webcam_privacy")
}

/// Which half of the form is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 6a. Where every open starts.
    Picker,
    /// The 6b overlay is up **on top of this window**. The card underneath
    /// says so and offers the way out, because the overlay is a separate OS
    /// window: a user who alt-tabs away from it would otherwise be looking at
    /// a modal with no visible state at all.
    Scanning,
    /// **A camera is open and its preview is inside this card.** Unlike
    /// [`Self::Scanning`] there is no second OS window: the surface a webcam
    /// needs is a rectangle of pixels, and this card already has one to give
    /// it. See [`WebcamStage`], which is what keeps the device alive, and
    /// [`TotpAdd::webcam`], which is what lets it go.
    Webcam,
    /// 6d's field and 6c's confirmation. Reached by hand, or by a decode
    /// landing in [`TotpAdd::typed`].
    Manual,
}

/// Where a QR was looked for, for the one refusal whose noun differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSource {
    Region,
    Image,
}

/// **Every way a route can fail before the field ever sees a string.**
///
/// The parser's refusals are [`OtpRefusal`] and render through
/// [`refusal_sentence`]; these are the ones that happen earlier -- the
/// capture, the file, the decoder. They are separate because they are
/// recovered from differently: the user picks a route again rather than
/// editing a field.
///
/// Carries nothing derived from the pixels or from the payload, so no arm of
/// [`PickerRefusal::sentence`] can print a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerRefusal {
    /// The pixels arrived and held no QR code. 6d's *"No code in that
    /// region"*.
    NoCode(CodeSource),
    /// The capture itself refused. The words are [`CaptureRefusal::title`]'s
    /// and `detail`'s, from design 6d.
    Capture(CaptureRefusal),
    /// The camera refused, or never produced a picture. The words are
    /// [`CameraRefusal::title`]'s and `detail`'s, and they are that module's
    /// rather than this one's for [`Self::Capture`]'s reason exactly: the
    /// thing that knows why a device would not open is the thing that tried
    /// to open it.
    Camera(CameraRefusal),
    /// The file is not a picture this app can decode -- it is not one of the
    /// formats named in `Cargo.toml`'s `image` features, or it is one of them
    /// and is malformed, truncated or absurdly large. **One refusal for all
    /// of those on purpose**: the user's next move is the same in every case
    /// (choose a different file), and a variant per failure mode would be a
    /// diagnosis of a file this app is deliberately not describing back to
    /// them.
    NotAnImage,
    /// The file could not be opened at all.
    Unreadable,
}

/// The clause both [`CodeSource`]s share, so "no code found" is **one**
/// refusal with one piece of advice rather than two that drift apart.
fn no_code_advice() -> &'static str {
    crate::i18n::t("totp.no_code_advice")
}

impl PickerRefusal {
    /// **The refusal as a sentence naming the reason.**
    ///
    /// Exhaustive with no catch-all, [`refusal_sentence`]'s rule and for its
    /// reason: a generic failure teaches the user to retry the thing that will
    /// not work.
    ///
    /// [`Self::NoCode`] is **one variant carrying one piece of advice**; only
    /// its noun and its second clause follow the route. "Drag again" is not
    /// something a user of the file dialog can do, and a single shared
    /// sentence that told them to would be exactly the generic refusal this
    /// rule exists to prevent.
    pub fn sentence(&self) -> String {
        match self {
            PickerRefusal::NoCode(CodeSource::Region) => {
                crate::i18n::tf("totp.no_code_region", &[("advice", no_code_advice())])
            }
            PickerRefusal::NoCode(CodeSource::Image) => {
                crate::i18n::tf("totp.no_code_image", &[("advice", no_code_advice())])
            }
            PickerRefusal::Capture(why) => {
                crate::i18n::tf("totp.title_detail", &[("title", why.title()), ("detail", why.detail())])
            }
            // The same two-clause shape as the capture's, because it is the
            // same kind of thing: a device that would not give this app what
            // it asked for, said in the device's own words rather than
            // paraphrased into a second vocabulary here.
            PickerRefusal::Camera(why) => {
                crate::i18n::tf("totp.title_detail", &[("title", why.title()), ("detail", why.detail())])
            }
            PickerRefusal::NotAnImage => crate::i18n::t("totp.not_an_image").to_string(),
            PickerRefusal::Unreadable => crate::i18n::t("totp.unreadable").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The image-file route
// ---------------------------------------------------------------------------

/// The one call the image route makes into something it does not own.
///
/// [`crate::region_overlay::RegionSeams`]'s shape, and its reason: behind a
/// seam every arm of [`decode_image_with`] is reachable from a test that
/// builds its bytes arithmetically, and that test can **look at the pixels**
/// the decoding half produced rather than trust that it produced any. That
/// matters more now than it did when this route read one format: what a test
/// through this seam can say about a JPEG or a WebP is that the picture came
/// out the size and the colours it went in as, which is the whole of what
/// this file owns.
#[derive(Clone, Copy)]
pub struct ImageSeams {
    /// [`crate::qr::decode_qr`] in production.
    pub decode: fn(&[u8], usize, usize) -> Option<Zeroizing<String>>,
}

impl ImageSeams {
    /// The real one. A test asserts this is [`crate::qr::decode_qr`] **by
    /// address**, so a seam quietly re-pointed at a stub fails rather than
    /// passing.
    pub fn production() -> Self {
        ImageSeams { decode: crate::qr::decode_qr }
    }
}

/// [`crate::qr::MAX_PIXELS`] expressed as a limit on ONE side, which is the
/// only shape [`image::Limits`] understands.
///
/// `image` bounds width and height separately and has no notion of an area,
/// while the bound this module actually wants is on the product. Capping each
/// side at the square root would be the tighter rule and the wrong one: a
/// panorama screenshot is legitimately thousands of times wider than it is
/// tall and well inside the pixel cap. So this is the *loose* half of the
/// bound and [`image_to_rgba`]'s own area check is the tight half. What this
/// half buys is that a header claiming four billion pixels of width is
/// refused by `image` inside the decoder's own constructor, before that
/// number is allowed to size a row buffer this module never sees.
const MAX_SIDE: u32 = crate::qr::MAX_PIXELS as u32;

/// The most any decode here may allocate, counting the decoder's own working
/// buffers as well as the pixels it hands back.
///
/// The area check below bounds the *picture*; it says nothing about what a
/// compressed format asks the allocator for on the way to producing it, and
/// the file that chooses that is the file the user was talked into opening.
/// Four bytes a pixel at the pixel cap is exactly the size of the RGBA answer
/// this module is going to build, so the rule is "no intermediate larger than
/// the result": a source carrying more precision per pixel than that is
/// carrying more than a QR decoder can use.
const MAX_DECODE_BYTES: u64 = crate::qr::MAX_PIXELS as u64 * 4;

/// A picture file's pixels as straight RGBA8, rows top to bottom, no padding
/// -- what [`crate::qr::decode_qr`] and `GetDIBits` both speak.
///
/// **Every ordinary raster format, because the owner's rule for this route is
/// that "all images should work".** PNG, JPEG, GIF, BMP, WebP and ICO: what a
/// screenshot tool, a phone camera and a Windows disk produce between them.
/// The list is named once in `Cargo.toml`'s `image` features and mirrored by
/// [`crate::file_picker::QR_FILTER_SPEC`], because a dialog that offers a
/// format this cannot read hands the user a file and then a refusal, and a
/// dialog that hides one it can read is the same defect pointing the other
/// way.
///
/// **Not [`crate::favicon::decode_rgba`]**, which is this crate's other
/// picture reader. That one resamples every image down to 64 pixels on its
/// longest edge for the item list, and 64 pixels is smaller than a QR code's
/// module grid: it would hand back a picture of a code that no decoder could
/// read.
///
/// # Bounded before anything is allocated
///
/// The header is attacker-chosen -- it is a file -- and every allocation on
/// this path is sized from it. So [`image::ImageReader::into_decoder`] is
/// used rather than `decode`: it parses the header and stops, which makes the
/// declared dimensions readable *before* a pixel buffer exists. They are
/// checked against [`crate::qr::MAX_PIXELS`], `qr`'s own bound, so a picture
/// refused here is exactly a picture the decoder would have refused anyway;
/// [`MAX_SIDE`] and [`MAX_DECODE_BYTES`] go in as [`image::Limits`] so that
/// the same rule also binds the decoder's internals, which this module cannot
/// see and cannot size.
///
/// # Both buffers are [`Zeroizing`], and that is not incidental
///
/// A picture of a QR code is a picture of a seed. The decoder is asked to
/// write its pixels *into a buffer this module owns* rather than to hand one
/// back, so the full-size copy of the seed's image lives in a `Zeroizing`
/// from the moment it exists -- which is what the `png`-only version of this
/// function did, and switching to a general decoder did not give it up.
/// What remains outside this module's reach is what
/// [`crate::qr::decode_qr`]'s documentation already concedes for `rqrr`: a
/// third-party decoder's own intermediates are ordinary allocations, released
/// un-wiped when the call returns. That concession is unchanged in kind here,
/// not widened.
fn image_to_rgba(bytes: &[u8]) -> Result<(Zeroizing<Vec<u8>>, usize, usize), PickerRefusal> {
    // Imported here rather than at the top of the file because this is the
    // one function in it that speaks to a decoder; `as _` because none of the
    // trait's own name is wanted, only its methods.
    use image::ImageDecoder as _;

    // The format comes from the CONTENT and never from the name: the path was
    // typed by a shell dialog and its extension is whatever the file happens
    // to be called, so a `.png` holding a JPEG is a file this route should
    // read rather than refuse.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| PickerRefusal::NotAnImage)?;
    // Assigned rather than built as a literal because `image::Limits` is
    // `#[non_exhaustive]`, so a struct expression -- functional update
    // included -- will not compile outside its own crate.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);

    // Header only. Nothing is decoded until `read_image` below, so every
    // check between here and it happens before a pixel is allocated.
    let decoder = reader.into_decoder().map_err(|_| PickerRefusal::NotAnImage)?;
    let (declared_width, declared_height) = decoder.dimensions();
    let (width, height) = (declared_width as usize, declared_height as usize);
    if width == 0 || height == 0 {
        return Err(PickerRefusal::NotAnImage);
    }
    let pixels = match width.checked_mul(height) {
        Some(pixels) if pixels <= crate::qr::MAX_PIXELS => pixels,
        _ => return Err(PickerRefusal::NotAnImage),
    };
    // `read_image` PANICS on a buffer of the wrong length, so this is sized
    // from the decoder's own arithmetic and not from the product above -- the
    // two differ by the bytes per pixel of whatever colour type the file
    // turned out to hold.
    if decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err(PickerRefusal::NotAnImage);
    }
    let color = decoder.color_type();
    let total =
        usize::try_from(decoder.total_bytes()).map_err(|_| PickerRefusal::NotAnImage)?;

    let mut buf = Zeroizing::new(vec![0u8; total]);
    decoder.read_image(&mut buf).map_err(|_| PickerRefusal::NotAnImage)?;

    let rgba = expand_to_rgba(&buf, color, pixels)?;
    if rgba.len() < pixels * 4 {
        // Short of what was declared. A decode can succeed on a file whose
        // last rows are missing, and a short buffer handed to `decode_qr`
        // answers `None` -- which would be reported as "no code in that
        // image" rather than as the broken file it is.
        return Err(PickerRefusal::NotAnImage);
    }
    Ok((rgba, width, height))
}

/// The top eight bits of a 16-bit sample.
///
/// `ImageDecoder::read_image` documents that it writes wide samples in the
/// **machine's own** byte order, so `from_ne_bytes` is the correct reader and
/// indexing `[1]` for the high byte would be a little-endian assumption
/// written down as a fact.
fn high_byte(sample: &[u8]) -> u8 {
    (u16::from_ne_bytes([sample[0], sample[1]]) >> 8) as u8
}

/// `source` -- whatever colour type `image` decoded the file into -- as
/// straight RGBA8.
///
/// Kept out of [`image_to_rgba`] so that the arms can be read as the one
/// table they are. `pixels` only reserves; the arms decide the length, and
/// the caller checks it.
fn expand_to_rgba(
    source: &[u8],
    color: image::ColorType,
    pixels: usize,
) -> Result<Zeroizing<Vec<u8>>, PickerRefusal> {
    let mut rgba = Zeroizing::new(Vec::with_capacity(pixels.saturating_mul(4)));
    match color {
        image::ColorType::Rgba8 => rgba.extend_from_slice(source),
        image::ColorType::Rgb8 => {
            for px in source.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 0xff]);
            }
        }
        // The shape a screenshot tool saving a black-and-white QR really
        // produces, and the arm that has to expand one channel into four.
        image::ColorType::L8 => {
            for grey in source {
                rgba.extend_from_slice(&[*grey, *grey, *grey, 0xff]);
            }
        }
        image::ColorType::La8 => {
            for px in source.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        // The wide arms are reachable through PNG, which is the only format
        // in this app's list that carries sixteen bits a channel. Narrowing
        // to the top byte is not a loss worth defending against: `decode_qr`
        // weighs the three channels into one eight-bit luma before it looks
        // at a single module.
        image::ColorType::L16 => {
            for px in source.as_chunks::<2>().0 {
                let grey = high_byte(px);
                rgba.extend_from_slice(&[grey, grey, grey, 0xff]);
            }
        }
        image::ColorType::La16 => {
            for px in source.as_chunks::<4>().0 {
                let grey = high_byte(&px[..2]);
                rgba.extend_from_slice(&[grey, grey, grey, high_byte(&px[2..])]);
            }
        }
        image::ColorType::Rgb16 => {
            for px in source.as_chunks::<6>().0 {
                rgba.extend_from_slice(&[
                    high_byte(&px[..2]),
                    high_byte(&px[2..4]),
                    high_byte(&px[4..]),
                    0xff,
                ]);
            }
        }
        image::ColorType::Rgba16 => {
            for px in source.as_chunks::<8>().0 {
                rgba.extend_from_slice(&[
                    high_byte(&px[..2]),
                    high_byte(&px[2..4]),
                    high_byte(&px[4..6]),
                    high_byte(&px[6..]),
                ]);
            }
        }
        // **Refused rather than converted, and the catch-all is required
        // rather than lazy**: `image::ColorType` is `#[non_exhaustive]`, so
        // this match cannot be written exhaustively from outside that crate.
        // What it covers today is the two floating-point colour types, which
        // only OpenEXR and Radiance HDR produce and neither is a format this
        // app turns on (see `Cargo.toml`). A conversion written here for them
        // would be a conversion no test in this crate could reach, which is
        // worse than a refusal that says the file is not one this app reads.
        _ => return Err(PickerRefusal::NotAnImage),
    }
    Ok(rgba)
}

/// **A picture file's bytes to the string its QR carries**, through `seams`.
///
/// Nothing leaves this function but the decoded string: the pixels are a
/// [`Zeroizing`] local that dies here, which is [`PRIVACY_LINE`]'s second
/// clause on this route.
pub fn decode_image_with(
    seams: &ImageSeams,
    bytes: &[u8],
) -> Result<Zeroizing<String>, PickerRefusal> {
    let (rgba, width, height) = image_to_rgba(bytes)?;
    (seams.decode)(&rgba, width, height).ok_or(PickerRefusal::NoCode(CodeSource::Image))
}

/// [`decode_image_with`] against the real decoder.
pub fn decode_image(bytes: &[u8]) -> Result<Zeroizing<String>, PickerRefusal> {
    decode_image_with(&ImageSeams::production(), bytes)
}

/// Reads the file the user pointed at, and decodes it.
///
/// The **only** line in this feature that touches the filesystem, and it only
/// reads. The bytes are a [`Zeroizing`] for `image_to_rgba`'s reason; nothing is
/// written, copied out, or logged.
pub fn read_image_file(path: &std::path::Path) -> Result<Zeroizing<String>, PickerRefusal> {
    let bytes = Zeroizing::new(std::fs::read(path).map_err(|_| PickerRefusal::Unreadable)?);
    decode_image(&bytes)
}

// ---------------------------------------------------------------------------
// Reading the field
// ---------------------------------------------------------------------------

/// What the typed text currently means.
///
/// **No `Debug`**: [`Self::Ok`] holds an [`OtpAuth`], whose own `Debug` is
/// hand-written to redact the seed, but a derived one here would be a second
/// place that decision has to be got right.
pub enum Reading {
    /// Nothing typed yet. Not a refusal: an empty box is not a wrong answer,
    /// and painting one a user has not finished giving is how a form nags.
    Empty,
    /// A seed and its parameters, ready to confirm.
    Ok(OtpAuth),
    /// Why what is typed is not a one-time code.
    Refused(OtpRefusal),
}

/// Whether `text` is being offered as a whole URI rather than as a bare seed.
///
/// The test is `://` and **not** `otpauth://`, so a pasted `https://...` is
/// read as the URI it is and refused by name
/// ([`OtpRefusal::NotOtpAuth`]) rather than being fed to the base32 reader and
/// refused as a bad secret. Two different mistakes, two different sentences;
/// that is this feature's whole rule about refusals.
fn looks_like_a_uri(text: &str) -> bool {
    text.contains("://")
}

/// Percent-encodes everything outside RFC 3986's unreserved set into `out`.
///
/// Needed because the one field takes **raw human input** and the validator it
/// is handed to is a URI parser. A typed `&` or `%` left alone would either
/// invent a query parameter or be read as an escape; encoded, it arrives at
/// [`parse_otpauth`] as the literal character it was and is refused as
/// [`OtpRefusal::BadSecret`], which is what it is.
fn push_percent_encoded(out: &mut String, text: &str) {
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble").to_ascii_uppercase());
                out.push(char::from_digit((byte & 0xf) as u32, 16).expect("nibble").to_ascii_uppercase());
            }
        }
    }
}

/// A typed bare secret, as the URI [`parse_otpauth`] will validate.
///
/// The parameters come from the form's own controls, because a bare seed
/// carries none. `Zeroizing`, allocated once at an upper bound: three bytes
/// per input byte plus the fixed text.
fn secret_as_uri(secret: &str, digits: u8, period: u16) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(64 + 3 * secret.len()));
    out.push_str("otpauth://totp/?secret=");
    push_percent_encoded(&mut out, secret);
    out.push_str("&algorithm=SHA1&digits=");
    out.push_str(if digits == 8 { "8" } else { "6" });
    out.push_str("&period=");
    out.push_str(&period.to_string());
    out
}

/// Reads the one field, with the two parameter controls as they stand.
///
/// **A whole URI overrides the controls, and that is deliberate**: a card that
/// says 8 digits over 60 seconds means it, and a form that silently applied
/// its own 6/30 to a URI that stated otherwise would save a seed that
/// generates confidently wrong codes. The controls apply to a **bare** seed,
/// which states nothing.
pub fn read_field(text: &str, digits: u8, period: u16) -> Reading {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Reading::Empty;
    }
    let uri: Zeroizing<String> = if looks_like_a_uri(trimmed) {
        Zeroizing::new(trimmed.to_string())
    } else {
        secret_as_uri(trimmed, digits, period)
    };
    match parse_otpauth(&uri) {
        Ok(auth) => Reading::Ok(auth),
        Err(refusal) => Reading::Refused(refusal),
    }
}

/// What the two parameter controls should show, and whether they may be
/// touched: `(digits, period, enabled)`.
///
/// **A pasted URI states its own parameters, so the controls follow it and go
/// dead.** Leaving them on 6/30 beside a confirmation reading "8 digits · 60
/// s" puts two contradictory answers on one card, and the one the user would
/// believe is the one they can click. Disabled rather than hidden: the fact
/// that this card is 8/60 *is* what the controls are now saying.
pub fn controls_for(reading: &Reading, typed: &str, digits: u8, period: u16) -> (u8, u16, bool) {
    match reading {
        Reading::Ok(auth) if looks_like_a_uri(typed.trim()) => (auth.digits, auth.period, false),
        _ => (digits, period, true),
    }
}

/// The line under the field: design 6d's *"Valid base32 · 16 characters ·
/// spaces ignored"*, or the reason it is not.
///
/// `None` for [`Reading::Empty`] -- see that variant.
pub fn validity_line(reading: &Reading) -> Option<String> {
    match reading {
        Reading::Empty => None,
        Reading::Ok(auth) => Some(crate::i18n::tf("totp.valid_line", &[("count", &auth.secret.len().to_string())])),
        Reading::Refused(refusal) => Some(refusal_sentence(refusal)),
    }
}

/// Every refusal as **its own sentence naming the reason**.
///
/// Exhaustive with no catch-all, so a variant added to [`OtpRefusal`] is a
/// compile error here rather than a shrug on screen. A generic failure teaches
/// the user to retry the thing that will not work, which is the opposite of
/// what a rejected payload should teach.
///
/// Carries **nothing of the secret** in any arm --
/// [`OtpRefusal::BadSecret`] deliberately holds nothing, because what was
/// wrong with it is the thing that must not be printed.
/// [`OtpRefusal::PartialSecret`] holds a character count, which
/// [`validity_line`] prints for every *accepted* secret anyway: a length is
/// not a seed, and here it is the whole diagnosis.
///
/// # Every sentence here has to fit the slot the design drew for it
///
/// `#6d` draws **one short line** under the field -- *"Valid base32 · 16
/// characters · spaces ignored"* -- and these sentences are painted in that
/// same slot by [`validity_row`]. At [`VALIDITY_PX`] across the body's own
/// [`MANUAL_WIDTH`] less its padding, one line is a little over seventy
/// characters; the length refusal below was written at two hundred and
/// twenty, wrapped to three lines, and was reported as prose lying over the
/// box above it. A refusal that is *correct* and four times the size of the
/// space it is drawn in is still a defect, and the length of the string is
/// the part of that this function owns.
///
/// So: **name the reason and point at the fix, and stop.** The mechanism --
/// why base32 cannot be three characters, why an unknown parameter is refused
/// rather than dropped -- is documentation and belongs in prose like this,
/// not under a text field. `the_refusals_fit_the_line_the_design_drew_for_them`
/// holds the whole family to it, so the next long sentence reds a test rather
/// than a screenshot.
pub fn refusal_sentence(refusal: &OtpRefusal) -> String {
    match refusal {
        OtpRefusal::NotOtpAuth => crate::i18n::t("totp.ref_not_otpauth").to_string(),
        OtpRefusal::NotTotp => crate::i18n::t("totp.ref_not_totp").to_string(),
        OtpRefusal::NoSecret => crate::i18n::t("totp.ref_no_secret").to_string(),
        OtpRefusal::BadSecret => crate::i18n::t("totp.ref_bad_secret").to_string(),
        // The characters are all fine, so saying anything about the alphabet
        // here would send the reader hunting for a bad one that is not there.
        // What is wrong is the count, so the count is what this names -- and a
        // length in this class is always exactly one character away from one
        // that works, in either direction, which is why the fix this points
        // at is a single character in either direction and nothing else.
        //
        // **What this used to say, and why it does not any more.** It opened
        // with "Those are all base32 characters, but a secret cannot be N of
        // them" and then taught the arithmetic -- five bits a character, a run
        // that stops part-way through a byte. Every clause of that was true
        // and the whole of it was four times the line it had to live on: three
        // wrapped rows of red in a slot the design draws one short row in. The
        // reassurance that the alphabet is fine is carried now by what the
        // sentence does NOT say (nothing about A-Z or 2-7, which
        // `three_good_characters_are_not_a_valid_secret` pins), and the
        // arithmetic is in `otpauth::decodes_to_whole_bytes`, which is where
        // somebody who wants it will look.
        //
        // Pluralised, because this class contains 1: "1 characters cannot
        // decode" is the kind of sentence that makes a user doubt the rest of
        // the card.
        OtpRefusal::PartialSecret(characters) => crate::i18n::tn("totp.ref_partial", *characters as u64, &[]),
        // Shortened for the reason above it: this was three wrapped lines as
        // well, and the clause that made it so -- "because a code saved from
        // it would be wrong and nothing on screen would say why" -- is an
        // argument for the refusal rather than anything the reader can act on.
        // What they can act on is the key, so the key is what is left, and
        // "refused, not ignored" is the one-clause version of the argument.
        OtpRefusal::UnknownParameter(key) => crate::i18n::tf("totp.ref_unknown_param", &[("key", key)]),
        OtpRefusal::BadParameter(name) => crate::i18n::tf("totp.ref_bad_param", &[("name", name)]),
        OtpRefusal::TooLong => crate::i18n::t("totp.ref_too_long").to_string(),
    }
}

/// The secret as a run of bullets, one for each character.
///
/// Length-shaped rather than a fixed run, so the row says how much seed there
/// is without saying what it is -- and so an eight-character seed and a
/// thirty-two-character one do not look identical. Ungrouped, because the
/// seed it stands in for is: see [`secret_as_typed`]. 6c draws its own mask
/// in fours; this row is the owner's.
/// **This app's general rule is the opposite**, and the exception is this
/// card's alone. `theme::MASKED_BULLETS` forbids a length-tracking mask -- it
/// tells a shoulder-surfer how many characters to expect -- and the Send
/// composer's password row obeys it. This row does not, because it is the
/// CONFIRMATION of a seed the user has just captured off their own screen,
/// with a Reveal sitting beside it: the question it answers is "is this the
/// thing I scanned", and a mask the same shape for every seed answers
/// nothing.
pub fn masked(secret: &str) -> String {
    "\u{2022}".repeat(secret.chars().count())
}

/// The seed exactly as it was read, for when it is actually shown.
///
/// **The masked row and the revealed row are one shape, and neither is
/// grouped.**
///
/// They were two shapes: the mask came in groups of four and the seed came
/// out as one unbroken run, so Reveal changed the row's LENGTH as well as
/// its contents. The owner asked for one shape -- "diff formatting should be
/// same like ******** abcdefgh" -- and the first reading of that put BOTH in
/// fours. That was wrong: `********` beside `abcdefgh` is eight of one
/// against eight of the other with no separator in either. The correction,
/// verbatim: "secret jsut store as text no separators".
///
/// This function is kept rather than inlined because [`masked`] is its pair,
/// and a row whose two halves are produced in one place cannot drift apart
/// again -- which is the whole history of this row.
pub fn secret_as_typed(secret: &str) -> String {
    secret.to_string()
}

/// The parameters row of the confirmation, spelled out -- **as four separate
/// chips**, which is how design 6c draws it: `TOTP`, `SHA1`, `6 digits`,
/// `30 s`, each in its own monospace pill.
///
/// Four values rather than one sentence because that is what they are. A card
/// that is 8 digits over 60 seconds under SHA-256 is exactly the case a
/// confirmation screen exists to catch, and four pills make the odd one out
/// findable at a glance in a way a single run of text separated by middots
/// does not.
pub fn parameter_chips(auth: &OtpAuth) -> [String; 4] {
    [
        "TOTP".to_string(),
        auth.algorithm.canonical().to_string(),
        crate::i18n::tf("totp.chip_digits", &[("digits", &auth.digits.to_string())]),
        crate::i18n::tf("totp.chip_seconds", &[("seconds", &auth.period.to_string())]),
    ]
}

/// The same four values joined for anything that wants one string of them --
/// a refusal, a log line, a test. **Built from [`parameter_chips`]** so the
/// row the user reads and the sentence anything else prints cannot come to
/// disagree about what the card said.
pub fn parameters_line(auth: &OtpAuth) -> String {
    parameter_chips(auth).join(" \u{b7} ")
}

/// The line above 6c's countdown track, verbatim from the design: *"refreshes
/// in 22 s"*.
pub fn refresh_line(seconds_left: u16) -> String {
    crate::i18n::tf("totp.refresh_line", &[("seconds", &seconds_left.to_string())])
}

/// The same countdown in §6d's words, which are the number and the unit and
/// nothing else: *"22 s"*.
///
/// §6c writes it out because its countdown is a stacked column with room for
/// a sentence; §6d's strip is ONE ROW -- code, track, seconds -- and "refreshes
/// in" beside a 4-point bar is the bar's caption twice. Built from the same
/// `seconds_left` so the two cards cannot disagree about the number.
pub fn seconds_line(seconds_left: u16) -> String {
    format!("{seconds_left} s")
}

/// How much of 6c's countdown track is still filled: the seconds left over the
/// card's own period.
///
/// The design draws the track at `width: 73%` beside *"refreshes in 22 s"* on
/// a 30-second card, and 22/30 is that number -- so the bar is the countdown
/// rather than a decoration that happens to sit next to it.
///
/// A card claiming a zero period reads as empty rather than dividing by it.
pub fn countdown_fraction(auth: &OtpAuth, unix_seconds: u64) -> f32 {
    if auth.period == 0 {
        return 0.0;
    }
    f32::from(seconds_left(auth, unix_seconds)) / f32::from(auth.period)
}

// ---------------------------------------------------------------------------
// The live code
// ---------------------------------------------------------------------------

/// The base32 alphabet, RFC 4648, uppercase.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Decodes a **normalised** base32 seed to key bytes.
///
/// "Normalised" means what [`parse_otpauth`] guarantees: uppercase, unpadded,
/// no spaces, entirely in [`BASE32_ALPHABET`]. Anything else returns `None`
/// rather than guessing -- this is fed a seed that has already been through
/// the one validator, so a `None` here means that validator and this reader
/// have come to disagree, and a wrong key is a wrong code with nothing on
/// screen to explain it.
///
/// `Zeroizing`, at an exact upper bound: five bits per character.
fn decode_base32(secret: &str) -> Option<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::with_capacity(secret.len() * 5 / 8 + 1));
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in secret.bytes() {
        let value = BASE32_ALPHABET.iter().position(|c| *c == byte)? as u32;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// One digest of `data` under `algorithm`.
///
/// The three arms are the only place this file names a hash. `sha1` and `sha2`
/// are already this crate's dependencies; see the module docs for why no
/// fourth crate is added for the HMAC on top of them.
fn digest(algorithm: Algorithm, data: &[u8]) -> Zeroizing<Vec<u8>> {
    use sha2::Digest as _;
    Zeroizing::new(match algorithm {
        Algorithm::Sha1 => {
            use sha1::Digest as _;
            sha1::Sha1::digest(data).to_vec()
        }
        Algorithm::Sha256 => sha2::Sha256::digest(data).to_vec(),
        Algorithm::Sha512 => sha2::Sha512::digest(data).to_vec(),
    })
}

/// The digest's block size in bytes, which HMAC's padding is defined over.
fn block_size(algorithm: Algorithm) -> usize {
    match algorithm {
        Algorithm::Sha1 | Algorithm::Sha256 => 64,
        Algorithm::Sha512 => 128,
    }
}

/// HMAC (RFC 2104) over one of the three digests.
///
/// Written out rather than pulled in as a crate, and it is nine lines: a key
/// longer than the block is hashed, a shorter one is zero-padded, and the
/// digest is taken twice under the two pads. Every intermediate is
/// [`Zeroizing`] because every one of them is derived from the seed --
/// including the padded key, which is the seed XORed with a constant and is
/// therefore the seed.
///
/// Pinned against RFC 2202's and RFC 4231's published vectors, so "it looks
/// right" is not the evidence.
fn hmac(algorithm: Algorithm, key: &[u8], message: &[u8]) -> Zeroizing<Vec<u8>> {
    let block = block_size(algorithm);
    let mut padded = Zeroizing::new(vec![0u8; block]);
    if key.len() > block {
        let hashed = digest(algorithm, key);
        padded[..hashed.len()].copy_from_slice(&hashed);
    } else {
        padded[..key.len()].copy_from_slice(key);
    }

    let mut inner = Zeroizing::new(Vec::with_capacity(block + message.len()));
    let mut outer = Zeroizing::new(Vec::with_capacity(block + 64));
    for byte in padded.iter() {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    let inner_digest = digest(algorithm, &inner);
    outer.extend_from_slice(&inner_digest);
    digest(algorithm, &outer)
}

/// The one-time code for `auth` at `unix_seconds`, or `None` if the seed
/// cannot be decoded.
///
/// RFC 4226's dynamic truncation over RFC 6238's time counter. `Zeroizing`,
/// and zero-padded to the card's digit count -- a code rendered without its
/// leading zero is a code the site will reject, and it is the failure a user
/// would blame on the seed.
pub fn code_at(auth: &OtpAuth, unix_seconds: u64) -> Option<Zeroizing<String>> {
    let key = decode_base32(&auth.secret)?;
    let period = u64::from(auth.period.max(1));
    let counter = unix_seconds / period;
    let mac = hmac(auth.algorithm, &key, &counter.to_be_bytes());

    // Dynamic truncation: the low nibble of the last byte picks the offset.
    let offset = (mac[mac.len() - 1] & 0x0f) as usize;
    let binary = (u32::from(mac[offset] & 0x7f) << 24)
        | (u32::from(mac[offset + 1]) << 16)
        | (u32::from(mac[offset + 2]) << 8)
        | u32::from(mac[offset + 3]);

    let digits = if auth.digits == 8 { 8 } else { 6 };
    let modulus = 10u32.pow(digits);
    let value = binary % modulus;

    let mut out = Zeroizing::new(String::with_capacity(digits as usize));
    for place in (0..digits).rev() {
        let d = (value / 10u32.pow(place)) % 10;
        out.push(char::from_digit(d, 10).expect("a decimal digit"));
    }
    Some(out)
}

/// Seconds until the code changes. Always in `1..=period`, so the countdown
/// never shows a zero that sits there for a second.
pub fn seconds_left(auth: &OtpAuth, unix_seconds: u64) -> u16 {
    seconds_left_in_period(auth.period, unix_seconds)
}

/// The same arithmetic as [`seconds_left`], over a bare period rather than a
/// whole [`OtpAuth`].
///
/// Split out because the vault window needs this for a seed it could NOT
/// read: its One-time code row shows a countdown for every item with a code
/// on screen, including the `steam://` and `hotp` shapes
/// [`crate::otpauth`] refuses, and there is no `OtpAuth` to hand for those.
/// `vault_window::mod::current_totp_seconds_left` was a second, independent
/// copy of this line with `30` written into it, which is precisely the bug a
/// shared implementation prevents: it counted 30 -> 1 twice inside one
/// 60-second code's life, so the row said "about to change" thirty seconds
/// before the code actually changed and then said it again when it did.
///
/// `period.max(1)` rather than a debug assertion: `crate::otpauth` already
/// refuses `period=0` at the parser, so a zero here can only come from a
/// caller that made one up, and the honest response to that is a countdown of
/// one second rather than a division-by-zero panic in a UI thread.
pub fn seconds_left_in_period(period: u16, unix_seconds: u64) -> u16 {
    let period = period.max(1);
    period - (unix_seconds % u64::from(period)) as u16
}

/// The code as design 6c draws it: `640 118`, split down the middle.
///
/// A borrowed grouping from every authenticator that shows one, and it is not
/// decoration: the check this screen exists for is a human comparing two short
/// strings, and an ungrouped run of eight is the shape that comparison fails
/// at.
pub fn grouped_code(code: &str) -> Zeroizing<String> {
    let half = code.len() / 2;
    let mut out = Zeroizing::new(String::with_capacity(code.len() + 1));
    for (i, ch) in code.chars().enumerate() {
        if i == half && half > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// A saved `login.totp` value as an [`OtpAuth`], however it was stored, or
/// `None` if this app cannot read it.
///
/// Bitwarden's `totp` field holds **either** a whole `otpauth://totp` URI or a
/// bare base32 seed, and both are common: the URI is what a scanned QR code
/// produces, the bare seed is what a user typing from a website's setup page
/// produces. `bw serve` accepts both, so this must too, or half the user's
/// TOTP items go blank on a backend switch.
///
/// **The bare seed is handled by making it a URI and re-parsing**, rather than
/// by constructing an [`OtpAuth`] here. That is deliberate: every rule about
/// what a seed may contain -- the base32 alphabet, the padding, the case, the
/// length bound -- then has exactly one implementation, in
/// [`crate::otpauth`], and a seed this crate would refuse to import is a seed
/// it also refuses to compute from. The RFC 6238 defaults a bare seed implies
/// (SHA-1, six digits, thirty seconds) are applied by that same parser, so
/// they are not restated here either.
///
/// `Zeroizing` throughout: the intermediate URI is a seed with twenty-odd
/// characters in front of it.
///
/// # Why this lives here rather than in the REST backend, where it was written
///
/// It was `rest::backend`'s private helper, because that backend is where the
/// discovery was made that a saved seed has two spellings. It has a **second**
/// caller now: [`super::totp_poll_plan`], the vault window's per-poll decision
/// about whether the selected item's code can be computed from the snapshot
/// already on screen. Copying it there would have been two readers of the same
/// field that could come to disagree about which seeds this app can read --
/// and "can this app read this seed?" is exactly the question that decides
/// whether the window answers locally or asks the backend, so a disagreement
/// would show as a code appearing on one path and not the other.
///
/// **`None` here is never "show no code".** It is "this app cannot answer, so
/// ask the backend", which on `bw serve` means asking the CLI, which reads
/// seed shapes this crate deliberately refuses ([`crate::otpauth`] rejects an
/// unknown parameter rather than guessing at it, and `steam://` is not base32
/// at all). See [`super::TotpPoll`].
pub fn read_seed(stored: &Zeroizing<String>) -> Option<OtpAuth> {
    match parse_otpauth(stored) {
        Ok(auth) => return Some(auth),
        // Anything that *is* an `otpauth://` URI and was still refused is
        // refused for a reason -- an `hotp` counter this app cannot advance,
        // an unknown parameter, a bad seed -- and re-reading it as a bare
        // seed would be reinterpreting a value whose meaning is already
        // known.
        Err(refusal) if refusal != OtpRefusal::NotOtpAuth => return None,
        Err(_) => {}
    }
    // Whitespace only: a seed copied off a setup page arrives in groups of
    // four. Everything else about the value is the parser's business.
    let mut bare = Zeroizing::new(String::with_capacity(stored.len()));
    bare.extend(stored.chars().filter(|c| !c.is_whitespace()));
    if bare.is_empty() {
        return None;
    }
    let uri = Zeroizing::new(format!("otpauth://totp/?secret={}", bare.as_str()));
    parse_otpauth(&uri).ok()
}

// ---------------------------------------------------------------------------
// What gets written
// ---------------------------------------------------------------------------

/// The value to write into the item's `totp` field: **the whole URI**.
///
/// Not the bare seed, ever, and not only when the parameters are unusual.
/// [`to_uri`] writes every parameter out including the ones that equal the RFC
/// defaults, so what is stored says what it means -- and a stored URI is what
/// makes the round trip in
/// [`tests::what_is_written_round_trips_with_its_parameters`] a real
/// guarantee rather than a coincidence of defaults.
///
/// `None` unless the field currently reads as a valid code, so there is no
/// spelling of "save" that can write a refused payload.
pub fn uri_to_write(state: &TotpAdd) -> Option<Zeroizing<String>> {
    match read_field(&state.typed, state.digits, state.period) {
        Reading::Ok(auth) => Some(to_uri(&auth)),
        Reading::Empty | Reading::Refused(_) => None,
    }
}

/// **A row of 6c's field table that carries a value worth copying.**
///
/// Three of the four. `Parameters` is not one: it is a run of chips -- the
/// kind, the algorithm, the digit count, the period -- and there is no single
/// string a click on it could mean. A row that reacted and copied something
/// arbitrary would be worse than one that does not react, because there is no
/// way to tell from the outside which it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableField {
    Issuer,
    Account,
    /// The seed. A credential, and copied as one -- see [`field_to_copy`],
    /// which answers a `Zeroizing` for every row rather than only for this
    /// one, so the one that matters is not the exception anybody has to
    /// remember.
    Secret,
}

/// **What a copied row would put on the clipboard.**
///
/// Read from the same [`read_field`] the table was drawn from, so what is
/// copied is what is shown. `None` where the row has no value -- an item with
/// no issuer draws an em dash, and an em dash is not a thing to copy.
pub fn field_to_copy(state: &TotpAdd, field: TableField) -> Option<Zeroizing<String>> {
    let Reading::Ok(auth) = read_field(&state.typed, state.digits, state.period) else {
        return None;
    };
    let value = match field {
        TableField::Issuer => auth.issuer.clone()?,
        TableField::Account => auth.account.clone()?,
        // Already a `Zeroizing` on the way in; cloned into another one rather
        // than through a `String`, so the plaintext never has an unwiped home.
        TableField::Secret => return Some(Zeroizing::new(auth.secret.to_string())),
    };
    (!value.is_empty()).then(|| Zeroizing::new(value))
}

/// **The code on screen right now**, for the caller to put on the clipboard.
///
/// Derived from the same reading and the same clock the panel draws from, so
/// what is copied is what is shown rather than a second answer computed a
/// moment later -- a code that rolled between the click and the copy would be
/// the one defect this is most likely to have, and asking one function twice
/// with the same `now_unix` is what rules it out.
///
/// `None` whenever the field does not read as a code, which is every state in
/// which there is no panel on screen to click.
pub fn code_to_copy(state: &TotpAdd, now_unix: u64) -> Option<Zeroizing<String>> {
    match read_field(&state.typed, state.digits, state.period) {
        Reading::Ok(auth) => code_at(&auth, now_unix),
        Reading::Empty | Reading::Refused(_) => None,
    }
}

/// Whether the submit button may be pressed at all.
pub fn can_save(reading: &Reading) -> bool {
    matches!(reading, Reading::Ok(_))
}

/// The submit button's face: [`REPLACE_LABEL`] when there is a code to
/// destroy, [`SAVE_LABEL`] when there is not.
/// **Whether `item` really carries a one-time code**, which is what
/// [`REPLACE_WARNING`] and [`submit_label`] are about.
///
/// The VALUE, not the key. `login.totp.is_some()` answers true for a record
/// carrying `totp: ""` -- a field Bitwarden leaves behind when a code is
/// removed -- so the card told the owner "this record already has a one-time
/// code" and offered to replace it on a login that had none. There is nothing
/// to replace and nothing to lose, and saying there is makes the one warning
/// in this card that guards an unrecoverable secret the one warning a user
/// learns to scroll past.
///
/// Trimmed, because whitespace is not a code either and a seed is base32 or
/// an `otpauth://` link: neither has meaningful leading space.
///
/// A function rather than the expression at the call site, so the two panes
/// that open this card cannot come to disagree about what "already has one"
/// means -- and so it can be asserted without a window.
pub fn item_has_code(item: &crate::vault_bridge::VaultItem) -> bool {
    item.login
        .as_ref()
        .and_then(|login| login.totp.as_deref())
        .is_some_and(|totp| !totp.trim().is_empty())
}


pub fn submit_label(already_has_code: bool) -> &'static str {
    if already_has_code {
        replace_label()
    } else {
        save_label()
    }
}

// ---------------------------------------------------------------------------
// The surface
// ---------------------------------------------------------------------------

/// The form's per-open state: **which item it was opened against**, what has
/// been typed, and the two controls.
///
/// The id and the name are both held for [`super::record_ui::RecordSend`]'s
/// reason exactly: the id is what the caller re-resolves the item by when Save
/// is pressed, because the vault can be re-read in between; the name is what
/// the card paints, copied so the heading cannot go blank if the item
/// disappears underneath.
///
/// **No `Debug`**: [`Self::typed`] is a [`Zeroizing`] holding a seed.
pub struct TotpAdd {
    /// The item this will be written to. See the struct doc.
    pub item_id: String,
    /// The item's name, as painted. See the struct doc.
    pub item_name: String,
    /// Whether that item already has a `totp`. Drives [`REPLACE_WARNING`] and
    /// [`submit_label`], and it is read **at the open** so the warning cannot
    /// appear or vanish under the user mid-form.
    pub already_has_code: bool,
    /// What is in the one field.
    pub typed: Zeroizing<String>,
    /// The digits control. [`DEFAULT_DIGITS`] until the user says otherwise.
    pub digits: u8,
    /// The period control. [`DEFAULT_PERIOD`] until the user says otherwise.
    pub period: u16,
    /// Whether the secret row is unmasked. **Starts `false`** and is never
    /// persisted anywhere.
    pub revealed: bool,
    /// Which half of the surface is on screen. **Starts [`Stage::Picker`]**:
    /// design 6a is the front door, and the by-hand form is one of four
    /// things behind it.
    pub stage: Stage,
    /// The last route-level refusal, painted on the picker. `None` once a
    /// route is chosen again, so a stale sentence cannot sit under a fresh
    /// attempt.
    pub refusal: Option<PickerRefusal>,
    /// Whether [`Self::typed`] came from a decoder rather than from a
    /// keyboard. Drives [`CODE_READ_LABEL`] -- see it, because this flag is a
    /// privacy decision and not a cosmetic one.
    pub scanned: bool,
    /// **The open camera, while there is one.** `Some` exactly while
    /// [`Stage::Webcam`] is on screen.
    ///
    /// **This field is the device's lifetime**, and putting it here rather
    /// than beside the form in `vault_window::mod` is the whole release
    /// story. [`crate::webcam::Session`]'s `Drop` stops the capture thread, so
    /// every way this surface can end -- Save, Cancel, Escape, the way back to
    /// the picker, a decode landing, the vault locking, the window being
    /// destroyed, a panic unwinding through the frame -- releases the camera,
    /// because every one of them drops this struct or replaces this field.
    /// None of them has to remember to.
    ///
    /// The 6b overlay is held the other way round, beside the form, and that
    /// is not an inconsistency: an overlay is a *window*, and a window has to
    /// be re-shown every frame by something that outlives one draw call. A
    /// camera has to be **let go of**, which is the opposite requirement and
    /// wants the opposite home.
    pub webcam: Option<WebcamStage>,
}

impl TotpAdd {
    /// Opens the form against one item, **on the picker**.
    pub fn opening(item_id: &str, item_name: &str, already_has_code: bool) -> Self {
        Self {
            item_id: item_id.to_string(),
            item_name: item_name.to_string(),
            already_has_code,
            typed: Zeroizing::new(String::new()),
            digits: DEFAULT_DIGITS,
            period: DEFAULT_PERIOD,
            revealed: false,
            stage: Stage::Picker,
            refusal: None,
            scanned: false,
            webcam: None,
        }
    }

    /// **A decoded payload becomes what is in the field.**
    ///
    /// The scanned routes do not get a confirmation card of their own: the
    /// decoded URI is put in [`Self::typed`] and read by the same
    /// [`read_field`] a pasted one is, so there is one card, drawn by one
    /// function, from one string.
    ///
    /// **[`Self::scanned`] is what that one card branches on**, in the two
    /// places where the two routes are genuinely different surfaces: the
    /// decoded URI is not put in a `TextEdit` (see [`CODE_READ_LABEL`]), and
    /// 6c's heading and its field table are drawn only here (see
    /// [`draw_confirmation`]). Both differences are about a payload no human
    /// read -- which is exactly what this flag means.
    ///
    /// It also means a hostile QR is refused by exactly the sentence a hostile
    /// paste is: [`parse_otpauth`] is the only validator either reaches.
    ///
    /// [`Self::revealed`] is put back to `false`, because the seed that was on
    /// screen a moment ago is not this one.
    /// **The camera is let go of here**, and this is the line that makes
    /// "the light goes out the moment it has read the code" true. It is in
    /// `accept_decoded` rather than in the webcam route's own code so that
    /// there is no spelling of "a code was accepted" that leaves a device
    /// open behind the confirmation card.
    pub fn accept_decoded(&mut self, text: Zeroizing<String>) {
        self.typed = text;
        self.scanned = true;
        self.stage = Stage::Manual;
        self.refusal = None;
        self.revealed = false;
        self.webcam = None;
    }

    /// Back to 6a, with the field emptied.
    ///
    /// **Emptied, not kept**: what is in it is a seed, and a form the user
    /// stepped away from is not a place to leave one resident. The
    /// `Zeroizing` is replaced rather than cleared in place so the old
    /// allocation is wiped on drop.
    ///
    /// **And the camera is closed**, for the same reason one step further
    /// out: a user who has walked back to the picker is not looking at a
    /// preview, and a device left open behind a card that is no longer
    /// showing it is exactly the leak this route must not have.
    pub fn back_to_picker(&mut self) {
        self.typed = Zeroizing::new(String::new());
        self.scanned = false;
        self.revealed = false;
        self.stage = Stage::Picker;
        self.refusal = None;
        self.webcam = None;
    }
}

/// **What the 6b overlay came back with, applied to the form.**
///
/// A free function taking `&mut TotpAdd` rather than a method on the overlay,
/// for this file's standing rule: the decision is a pure function of the
/// outcome, so a test can drive all four arms without a window anywhere.
///
/// [`Outcome::Cancelled`] leaves **no refusal**. The user pressed Escape; a
/// sentence explaining that to them is an app narrating their own action back
/// at them, which is the thing `apply_export_action` already refuses to do for
/// a dismissed dialog.
///
/// **Unchanged by the whole-screen scan, and that is the design.** The scan
/// runs inside `region_overlay`, and the two answers it can give that this
/// file has no words for -- "nothing on any monitor" and "more than one code,
/// choose which" -- never arrive here at all: they open 6b with a line in its
/// bar saying so, and the user then drags. What reaches this function is still
/// exactly what a drag can produce. So a code the scan found and a code the
/// user framed by hand land on the same [`TotpAdd::accept_decoded`], and from
/// there on 6c's confirmation card, where nothing is written until Save is
/// pressed.
///
/// **Unchanged by the reveal, too.** A found code now spends
/// `region_overlay::REVEAL_DWELL` on screen, ringed where it sits, before the
/// overlay closes. That is entirely inside the overlay: it arrives here at the
/// same moment it always did relative to `show` answering `false`, as the same
/// [`Outcome::Decoded`], and it still saves nothing.
pub fn apply_region_outcome(state: &mut TotpAdd, outcome: Outcome) {
    match outcome {
        Outcome::Decoded(text) => state.accept_decoded(text),
        Outcome::Cancelled => {
            state.stage = Stage::Picker;
            state.refusal = None;
        }
        Outcome::NoCode => {
            state.stage = Stage::Picker;
            state.refusal = Some(PickerRefusal::NoCode(CodeSource::Region));
        }
        Outcome::Refused(why) => {
            state.stage = Stage::Picker;
            state.refusal = Some(PickerRefusal::Capture(why));
        }
    }
}

/// **What the file dialog came back with, applied to the form.**
///
/// `None` is a cancelled dialog and leaves no refusal, for
/// [`apply_region_outcome`]'s reason.
pub fn apply_image_pick(state: &mut TotpAdd, picked: Option<&std::path::Path>) {
    let Some(path) = picked else {
        state.refusal = None;
        return;
    };
    match read_image_file(path) {
        Ok(text) => state.accept_decoded(text),
        Err(why) => {
            state.stage = Stage::Picker;
            state.refusal = Some(why);
        }
    }
}

/// What one frame of the form reports back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAddAction {
    None,
    /// Write [`uri_to_write`]'s value to the item.
    ///
    /// **Carries nothing**, for `record_ui::RecordUiAction::SubmitExport`'s
    /// reason: the value is a seed, and routing it through this `Copy` enum
    /// would give the plaintext a second, non-zeroizing home. The caller asks
    /// [`uri_to_write`] for it.
    Save,
    /// Close without writing anything.
    Cancel,
    /// **Open design 6b's overlay.** Reported rather than done, because
    /// opening it needs `screen_capture::monitor_bounds()` and the parent
    /// `egui::Context`, and because the overlay has to be driven by the
    /// window's own frame loop -- see `vault_window::mod`'s block.
    ScanRegion,
    /// **Open the shell's file dialog.** Reported rather than done, for a
    /// harder reason than the above: `IFileOpenDialog::Show` is modal and
    /// pumps its own message loop, so it must be called from the action
    /// handler after this frame's draw closures have returned, exactly where
    /// `EditAction::PickAppFile` is called from.
    OpenImage,
    /// **Open a camera.** Reported rather than done, for
    /// [`Self::OpenImage`]'s reason with one difference worth naming: the
    /// blocking part is the enumeration and not a dialog.
    /// [`crate::webcam::devices`] asks Media Foundation for the list and
    /// returns, which is bounded but not instant, so it belongs in the action
    /// handler after this frame's draw closures have returned rather than
    /// inside one. Everything after the enumeration -- opening the device,
    /// every frame, every decode -- is on a thread of `webcam`'s own and
    /// never on this one.
    OpenWebcam,
    /// **Open one of the enumerated cameras**, by its index in
    /// [`WebcamStage::devices`].
    ///
    /// Reported rather than done for a reason none of the three above
    /// share: opening a device needs [`crate::webcam::WebcamSeams`], and a
    /// draw function that reached for `WebcamSeams::production()` itself
    /// would be a surface that opens a real camera when a test presses a
    /// row on it. The seam has to arrive from the caller for the same
    /// reason it exists at all.
    UseCamera(usize),
    /// **Put the live six-digit code on the clipboard.**
    ///
    /// The owner: "make 6 digit code clickable and copiable - most of the MFA
    /// require copy-paste it first before applying". The card that confirms a
    /// code is exactly where that need lands -- the site asking for the code
    /// is usually still open behind this modal, and the alternative is reading
    /// six digits off one window and typing them into another before they
    /// expire.
    ///
    /// **Carries nothing**, for [`Self::Save`]'s reason: the value is a
    /// credential, and routing it through this `Copy` enum would give the
    /// plaintext a second, non-zeroizing home. The caller asks
    /// [`code_to_copy`] for it, which answers a `Zeroizing` and is the same
    /// function the panel drew from.
    CopyCode,
    /// **Put one of 6c's field rows on the clipboard.**
    ///
    /// The owner, after the live code: "also make copiaeble the rest of the
    /// fields". Names the ROW rather than carrying its value, for
    /// [`Self::CopyCode`]'s reason exactly -- one of the three is the seed.
    /// The caller asks [`field_to_copy`].
    CopyField(TableField),
}

// ---------------------------------------------------------------------------
// The webcam route
// ---------------------------------------------------------------------------

/// **The camera stage's state, and the camera's lifetime.**
///
/// Held by [`TotpAdd::webcam`]; see that field for why it lives there and not
/// beside the form. Everything in here dies together, which is the point:
/// the [`Session`] releases the device, and the texture releases the one copy
/// of a frame this route cannot wipe.
///
/// **No `Debug`.** [`Self::texture`] is a picture of whatever the camera is
/// pointed at, which on this screen is by construction a QR code of a seed.
pub struct WebcamStage {
    /// Every camera Windows offered, in enumeration order. Held even when
    /// there is only one, so [`WEBCAM_ANOTHER_LABEL`] can offer the others
    /// after a refusal without a second enumeration.
    pub devices: Vec<crate::webcam::Device>,
    /// Which of [`Self::devices`] is open, as an index. `None` means the
    /// device picker is on screen: more than one camera, and the user has not
    /// said which.
    pub chosen: Option<usize>,
    /// The running camera. `None` while the device picker is up -- **which is
    /// the point of it being an `Option` rather than always present**: no
    /// device is opened, and so no camera light comes on, until the user has
    /// picked one.
    pub session: Option<Session>,
    /// The last frame's size in pixels, for the preview's aspect ratio. Kept
    /// beside the texture rather than read off it so the two cannot disagree
    /// on the frame a camera changes resolution mid-stream.
    pub size: Option<(usize, usize)>,
    /// The preview's texture. **The one copy of a frame that is not a
    /// [`Zeroizing`]**, because `egui` has no such thing; it is freed when
    /// this struct drops, and what it holds is a picture of a code that is at
    /// that moment being held up in front of the machine.
    pub texture: Option<egui::TextureHandle>,
}

impl WebcamStage {
    /// Whether a picture has arrived yet, which is what the surface says
    /// instead of painting a black rectangle.
    pub fn showing(&self) -> bool {
        self.texture.is_some()
    }
}

/// **What the enumeration came back with, applied to the form.**
///
/// A free function taking `&mut TotpAdd`, [`apply_region_outcome`]'s rule and
/// for its reason: every arm is reachable from a test with no camera
/// anywhere.
///
/// **One camera opens immediately; two or more ask first.** A device picker
/// shown for a single built-in webcam is a question with one answer, and the
/// route that asks it is a route that takes two presses to do what one
/// should.
pub fn open_webcam(
    state: &mut TotpAdd,
    seams: &WebcamSeams,
    found: Result<Vec<crate::webcam::Device>, CameraRefusal>,
) {
    let devices = match found {
        Ok(devices) if !devices.is_empty() => devices,
        Ok(_) => {
            // `webcam::devices` already turns an empty list into `NoCamera`;
            // this arm exists because a seam is a seam and a stub could hand
            // back an empty `Ok`, which must not become a stage with nothing
            // in it.
            state.stage = Stage::Picker;
            state.refusal = Some(PickerRefusal::Camera(CameraRefusal::NoCamera));
            return;
        }
        Err(why) => {
            state.stage = Stage::Picker;
            state.refusal = Some(PickerRefusal::Camera(why));
            return;
        }
    };
    let only_one = devices.len() == 1;
    state.stage = Stage::Webcam;
    state.refusal = None;
    // Assigned rather than mutated: whatever camera was open before this
    // press is dropped here, so pressing the row twice cannot leave two
    // devices running.
    state.webcam = Some(WebcamStage {
        devices,
        chosen: None,
        session: None,
        size: None,
        texture: None,
    });
    if only_one {
        choose_camera(state, seams, 0);
    }
}

/// **Opens one of the enumerated cameras**, by index.
///
/// Out of range is ignored rather than refused: the only thing that can pass
/// an index is the list this stage is drawing, so an out-of-range one is a
/// bug in this file and not a thing to explain to a user.
pub fn choose_camera(state: &mut TotpAdd, seams: &WebcamSeams, index: usize) {
    let Some(stage) = state.webcam.as_mut() else {
        return;
    };
    let Some(device) = stage.devices.get(index).cloned() else {
        return;
    };
    // The previous session is dropped by the assignment, which stops its
    // capture thread -- so switching cameras cannot leave the first one open.
    stage.session = Some(Session::open(seams, device));
    stage.chosen = Some(index);
    stage.size = None;
    stage.texture = None;
}

/// **One frame of the camera stage**, with everything from outside as
/// arguments.
///
/// Returns the newest frame for the caller to paint, having already: decided
/// whether the session has anything to report, applied it, and -- when it was
/// a code -- moved the form to 6c. Nothing here draws, so every one of those
/// decisions is a thing a test can drive.
///
/// **The decode is not here.** It happens on the capture thread, inside
/// [`crate::webcam::offer`], which is why this function cannot be slow however
/// large a frame is or however hard a picture is to read.
pub fn advance_webcam(
    state: &mut TotpAdd,
    now: std::time::Instant,
) -> Option<crate::webcam::Frame> {
    use crate::webcam::Verdict;

    let stage = state.webcam.as_mut()?;
    let Some(session) = stage.session.as_mut() else {
        // The device picker is up. Nothing is open, so there is nothing to
        // ask.
        return None;
    };
    let taken = session.take();
    if let Some(verdict) = taken.verdict {
        return match verdict {
            // `accept_decoded` closes the camera; see its own note.
            Verdict::Read(text) => {
                state.accept_decoded(text);
                None
            }
            Verdict::Refused(why) => {
                refuse_camera(state, why);
                None
            }
            // A producer that ran out with nothing to say. The real capture
            // loop reports `Lost` instead, so this is only reachable from a
            // stub -- but a stage left running against a finished session
            // would sit on "Starting the camera" forever, so it is answered
            // rather than ignored.
            Verdict::Ended => {
                refuse_camera(state, CameraRefusal::Silent);
                None
            }
        };
    }
    if session.stalled(now, crate::webcam::FIRST_FRAME_GRACE) {
        refuse_camera(state, CameraRefusal::Silent);
        return None;
    }
    if let Some(frame) = &taken.frame {
        stage.size = Some((frame.width(), frame.height()));
    }
    taken.frame
}

/// Ends the camera stage with a reason, back on the picker.
///
/// `state.webcam = None` is what closes the device, and it is done here
/// rather than left to the caller so that no refusal path can forget it.
fn refuse_camera(state: &mut TotpAdd, why: CameraRefusal) {
    state.webcam = None;
    state.stage = Stage::Picker;
    state.refusal = Some(PickerRefusal::Camera(why));
}

// ---------------------------------------------------------------------------
// Design 6d's surface.
//
// **Every number in this section was measured in a browser against
// `docs/design/Deskwarden.dc.html`'s own `id="6d"` panel**, and each constant
// names the CSS declaration it came from so the next person to move one can
// check it against the same source rather than against the last render.
//
// # Measured, because the design page is content-box
//
// A declared size on that page is the size INSIDE the border and every border
// adds to what is on screen -- `HEADER_HEIGHT` records the same trap for 6a.
// It bites twice here: 6d's secondary answer declares `height: 32px` inside a
// `1px` border and its rectangle is 34 tall, and every one of the parameter
// cells declares `padding: 5px 0` inside a run whose border is its own. The
// card as a whole is 470 x 345.8 with a 468 x 44 header, a 468 x 240.8 body
// and a 468 x 59 footer, and the numbers below are what add up to that.
//
// # Which card this is
//
// `#6d`'s own card is drawn flat -- `border: 1px solid #dedbd9`, no shadow --
// because that panel is a spec vignette rather than a modal on a scrim. The
// thing on screen is a modal, and it is **the same card 6c declares**
// (`border: 1px solid #d7d3d3` under `box-shadow: 0 14px 34px rgba(45, 43,
// 43, 0.18)`), because 6c is not a second card here: this app reaches the
// confirmation by typing as well as by scanning, so [`draw_confirmation`]
// renders into the bottom of this card's body. One card, one chrome, and it
// is [`stage_card`] -- 6a's, which declares those two rules identically.
//
// # The helpers are 6a's
//
// `lay_out`, `face`, `body_face`, `paint_svg` and [`Svg`] are defined in 6a's
// section below and shared rather than restated. The two stages are one card
// a step apart, and a second set of text and mark helpers is how the two come
// to round, wrap and stroke differently.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Design 6a's surface.
//
// **Every number in this section is read off `docs/design/Deskwarden.dc.html`'s
// `id="6a"` panel**, and each constant names the CSS declaration it came from
// so the next person to move one can check it against the same source rather
// than against the last render.
//
// # Why this card is not `theme::modal_card`
//
// The shared modal frame is a three-band card: a header band filled with the
// card's accent, a white body, and a footer band carrying a row of answers.
// 6a is none of those. Its header is the card's own white with a hairline
// under it, its title is 15px/800 rather than 14px/700 on colour, and its
// footer is a tinted note with no buttons in it at all -- 6a's only way out
// is the ✕ in the header. Forcing this surface through `modal_card` would
// mean painting a coloured band the design does not draw and inventing two
// answers it does not ask for, so the card is built here. What is NOT built
// here is anything the design system already owns: the colours are `theme`'s,
// the ✕ is `theme::modal_dismiss_mark`, and the keycap is
// `theme::paint_return_keycap`.
// ---------------------------------------------------------------------------

/// **The keys design 6a's keycaps promise, and the rows they take.**
///
/// The owner, against the shipped 0.15.21 build: *"Enter and 1,2,3,4 not
/// workong - supposed to be hjotkeys"*. They were right about the promise --
/// 6a draws an affordance on the right of every route row, a filled `↵` chip
/// on the first and a bare ordinal on the other three -- and right that the
/// app answered almost none of it. Only Enter was bound, and it was bound in
/// a place that let it overrule a click landing on the same frame.
///
/// # Why `1` is here when 6a draws no `1`
///
/// A one-line departure from the design, argued rather than slipped in. The
/// design hangs `↵` off the first row and ordinals off rows two to four, so
/// on the page the first row's affordance is a *different kind of thing* from
/// the others -- it says "this is the default", not "this is number one".
/// That reads correctly on a page. In a hand it does not: the rows are
/// numbered one to four down the card, three of the numbers work, and the
/// owner reached for the fourth. A picker whose second, third and fourth rows
/// answer their digit and whose first does not is a worse surface than the
/// design's, because it teaches a rule and then breaks it on the one row it
/// drew in blue.
///
/// So `1` is accepted and **the drawn keycap stays the design's `↵`**. The
/// design decides what the row says; this decides what the keyboard answers,
/// and the two only have to agree in the direction that matters -- every
/// glyph drawn is a key that works. A key that works without being drawn
/// costs nothing and breaks no promise.
///
/// # Every row answers its own digit
///
/// `2` was the one keycap on this card that shipped drawn and dead, and it
/// was dead for a reason that turned out not to be about this file at all.
/// `vault_window::ADD_TOTP_KEY` is `Key::Num2` -- CTRL+SHIFT+2 is the chord
/// that OPENS this modal -- and `mod.rs`'s
/// `the_add_code_chord_is_a_key_no_other_binding_takes` used to hold that
/// key to being named in no production file but its own.
///
/// No keystroke could ever have reached both: that chord is read with
/// `matches_exact(CTRL|SHIFT)` and every key in this table is read with no
/// modifier at all. The guard was a *textual* pin that over-approximated --
/// it forbade the spelling rather than the binding -- and the cost of the
/// over-approximation was a keycap the design draws and the app ignored.
///
/// That guard now reads `one binding per key AND modifier set`, which is
/// what it always meant, so this row is bound like the other three. The
/// owner settled it in those terms: "just 1-4 when window focused, so local
/// chord only for this modal".
///
/// Writing the key some other way to slip past the pin was the alternative
/// and it was never one: a guard a file can dodge protects nothing, and the
/// next person to bind a digit would have found a precedent for dodging it.
const ROUTE_KEYS: [(egui::Key, usize); 5] = [
    (egui::Key::Enter, 0),
    (egui::Key::Num1, 0),
    (egui::Key::Num2, 1),
    (egui::Key::Num3, 2),
    (egui::Key::Num4, 3),
];

/// **Which route a keystroke picks, if any.**
///
/// Pure over the key and the modifier state so the whole table is a thing a
/// test can enumerate, including the rows it must *not* answer.
///
/// # Bare keys only
///
/// A held modifier means the keystroke belongs to somebody else. This window
/// binds CTRL+K, CTRL+L, CTRL+N, CTRL+SHIFT+2 and CTRL+SHIFT+R, and the
/// detail pane binds more; a picker that answered `1` on CTRL+1 would be
/// taking a route on a chord aimed past it. `is_none()` and not
/// "no CTRL", because SHIFT+3 is `#` and ALT+4 is a system gesture, and
/// neither is a user asking for the third or fourth row.
///
/// # A dead row's key is dead
///
/// A disabled row is [`DEFERRED_REASON`]'s case: it is drawn, it says why it
/// does nothing, and it cannot be clicked. Its digit must be exactly as inert
/// as its rectangle, or the keyboard becomes a way into a route the surface
/// has said is not available -- which is worse than a dead keycap, because it
/// is a live key with no affordance at all.
/// `rows` is a parameter and not [`ROUTES`] read directly, for the reason
/// `a_deferred_row_still_says_that_it_is_deferred` paints a synthetic table:
/// nothing in `ROUTES` is disabled today, so the dead-row branch would be
/// code no test had ever run, in the one place where "it does nothing" is the
/// whole requirement.
fn route_for_key(rows: &[RouteRow], key: egui::Key, modifiers: egui::Modifiers) -> Option<Route> {
    if !modifiers.is_none() {
        return None;
    }
    ROUTE_KEYS
        .iter()
        .find(|(bound, _)| *bound == key)
        .and_then(|(_, row)| rows.get(*row))
        .filter(|row| row.enabled)
        .map(|row| row.route)
}

// ---------------------------------------------------------------------------
// 6a's four marks, in the design's own coordinates.
// ---------------------------------------------------------------------------

/// The side of the `viewBox="0 0 24 24"` every mark on this card is drawn in.
const VIEWBOX: f32 = 24.0;

/// One piece of one of 6a's marks, in [`VIEWBOX`] coordinates.
///
/// **Deliberately not `theme::MarkShape`.** That family exists so that
/// `win32_draw` can stroke the same geometry with GDI for the bare-Win32
/// account picker, and it carries the cost of being renderer-agnostic. Every
/// mark here is drawn once, by egui, on one modal, so this is the smaller
/// thing: the three primitives the design's own SVG uses on this panel, and
/// no indirection.
enum Svg {
    /// A polyline through the listed viewBox points, stroked and not closed.
    Line(&'static [(f32, f32)]),
    /// A stroked rectangle: `x, y, width, height, corner radius`, which is
    /// SVG's `<rect>` with its `rx` spelled out.
    Rect(f32, f32, f32, f32, f32),
    /// A stroked circle: `cx, cy, r`.
    Circle(f32, f32, f32),
    /// A filled dot: `cx, cy, r`. What the design writes as `M12 8h.01` --
    /// a zero-length segment, which is a dot under the round line cap those
    /// icon sets are drawn with and nothing at all under SVG's default butt
    /// cap. egui's strokes have no caps at all, so the intent is painted
    /// directly rather than hoping a hairline shows.
    Dot(f32, f32, f32),
}

// The design writes its rounded corners as arcs (`a2 2 0 0 1 2-2`, and the
// image mark's tighter `a1 1 0 0 1 1-1`). Every one of them is written out
// below as its endpoints plus three intermediate samples at 22.5-degree
// steps, in [`VIEWBOX`] units, rather than sampled at draw time: these paths
// are constants, and a quarter-arc this small cut to fewer than three steps
// reads as a mitre at 17px.

/// The region-scan mark: four corner brackets, `M3 8V5a2 2 0 0 1 2-2h3` and
/// its three rotations.
const REGION_MARK: &[Svg] = &[
    Svg::Line(&[(3.0, 8.0), (3.0, 5.0), (3.152, 4.235), (3.586, 3.586), (4.235, 3.152), (5.0, 3.0), (8.0, 3.0)]),
    Svg::Line(&[(16.0, 3.0), (19.0, 3.0), (19.765, 3.152), (20.414, 3.586), (20.848, 4.235), (21.0, 5.0), (21.0, 8.0)]),
    Svg::Line(&[(21.0, 16.0), (21.0, 19.0), (20.848, 19.765), (20.414, 20.414), (19.765, 20.848), (19.0, 21.0), (16.0, 21.0)]),
    Svg::Line(&[(8.0, 21.0), (5.0, 21.0), (4.235, 20.848), (3.586, 20.414), (3.152, 19.765), (3.0, 19.0), (3.0, 16.0)]),
];

/// The image-file mark: a framed picture with a dog-eared sheet behind it --
/// `M4 7V5a1 1 0 0 1 1-1h2`, `<rect x=3 y=9 w=18 h=12 rx=2>`, and
/// `M12 3h7a2 2 0 0 1 2 2v2`.
const IMAGE_MARK: &[Svg] = &[
    Svg::Line(&[(4.0, 7.0), (4.0, 5.0), (4.293, 4.293), (5.0, 4.0), (7.0, 4.0)]),
    Svg::Rect(3.0, 9.0, 18.0, 12.0, 2.0),
    Svg::Line(&[(12.0, 3.0), (19.0, 3.0), (19.765, 3.152), (20.414, 3.586), (20.848, 4.235), (21.0, 5.0), (21.0, 7.0)]),
];

/// The by-hand mark: three ruled lines inside a card -- `M7 8h10`,
/// `M7 12h10`, `M7 16h6` and `<rect x=3 y=4 w=18 h=16 rx=2>`.
const BY_HAND_MARK: &[Svg] = &[
    Svg::Line(&[(7.0, 8.0), (17.0, 8.0)]),
    Svg::Line(&[(7.0, 12.0), (17.0, 12.0)]),
    Svg::Line(&[(7.0, 16.0), (13.0, 16.0)]),
    Svg::Rect(3.0, 4.0, 18.0, 16.0, 2.0),
];

/// The webcam mark: a camera body and its lens flare --
/// `M15 10l4.5-2.6v9.2L15 14` and `<rect x=3 y=6 w=12 h=12 rx=2>`.
const WEBCAM_MARK: &[Svg] = &[
    Svg::Line(&[(15.0, 10.0), (19.5, 7.4), (19.5, 16.6), (15.0, 14.0)]),
    Svg::Rect(3.0, 6.0, 12.0, 12.0, 2.0),
];

/// The footer's mark: a circled `i` -- `<circle cx=12 cy=12 r=9>`,
/// `M12 8h.01` and `M11 12h1v5h1`.
const INFO_MARK: &[Svg] = &[
    Svg::Circle(12.0, 12.0, 9.0),
    Svg::Dot(12.0, 8.0, 1.1),
    Svg::Line(&[(11.0, 12.0), (12.0, 12.0), (12.0, 17.0), (13.0, 17.0)]),
];

/// Which mark a route wears, in 6a's own order.
fn mark_for(route: Route) -> &'static [Svg] {
    match route {
        Route::ScanRegion => REGION_MARK,
        Route::ImageFile => IMAGE_MARK,
        Route::ByHand => BY_HAND_MARK,
        Route::Webcam => WEBCAM_MARK,
    }
}

/// Strokes one mark into `at`, scaling [`VIEWBOX`] coordinates onto that box
/// and the design's `stroke-width` along with them -- a 2.2 stroke on a 24
/// box drawn at 17px is 1.56px on screen, which is what the browser paints
/// and what a hardcoded 2.2 would not be.
fn paint_svg(
    painter: &egui::Painter,
    at: egui::Rect,
    parts: &[Svg],
    stroke_units: f32,
    colour: egui::Color32,
) {
    let scale = at.width() / VIEWBOX;
    let stroke = egui::Stroke::new(stroke_units * scale, colour);
    let point = |x: f32, y: f32| egui::pos2(at.min.x + x * scale, at.min.y + y * scale);
    for part in parts {
        match part {
            Svg::Line(points) => {
                let points: Vec<egui::Pos2> =
                    points.iter().map(|(x, y)| point(*x, *y)).collect();
                painter.add(egui::Shape::line(points, stroke));
            }
            Svg::Rect(x, y, w, h, r) => {
                painter.rect_stroke(
                    egui::Rect::from_min_size(point(*x, *y), egui::vec2(w * scale, h * scale)),
                    CornerRadius::same((r * scale).round() as u8),
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
            Svg::Circle(cx, cy, r) => {
                painter.circle_stroke(point(*cx, *cy), r * scale, stroke);
            }
            Svg::Dot(cx, cy, r) => {
                painter.circle_filled(point(*cx, *cy), r * scale, colour);
            }
        }
    }
}

/// What pressing a route means. A pure function so the routing is a thing a
/// test can enumerate rather than four arms buried in a click handler.
///
/// Three of the four are reported to the caller and one is not, and the line
/// between them is whether anything outside this file has to happen first.
/// [`Route::ByHand`] is a stage change and nothing else; the other three each
/// need something the draw closure must not do -- a second OS window, a modal
/// shell dialog, or an enumeration of capture devices.
pub fn action_for(route: Route) -> TotpAddAction {
    match route {
        Route::ScanRegion => TotpAddAction::ScanRegion,
        Route::ImageFile => TotpAddAction::OpenImage,
        Route::Webcam => TotpAddAction::OpenWebcam,
        // Handled in the picker itself: it is a stage change and not something
        // the caller has to do.
        Route::ByHand => TotpAddAction::None,
    }
}

// ---------------------------------------------------------------------------
// The camera stage
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Design 6c's numbers, all of them lifted out of the CSS under `id="6c"`
// ---------------------------------------------------------------------------



/// **What the copy confirmation calls each of the five things this card
/// copies**, in the app's own toast.
///
/// The row labels themselves wherever there is one -- so the row the user
/// pressed and the sentence they read are the same `const` -- and the READ
/// PANE's word for the live code, because that is the same value the pane
/// copies under the same name and two words for one credential is two
/// credentials as far as a reader is concerned.
pub fn copied_label(field: Option<TableField>) -> &'static str {
    match field {
        Some(TableField::Issuer) => issuer_row_label(),
        Some(TableField::Account) => account_row_label(),
        Some(TableField::Secret) => secret_row_label(),
        None => crate::i18n::t("detail.copy_2fa"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URI whose parameters are all non-default, so a reader that dropped
    /// them would be visible rather than accidentally right.
    const UNUSUAL: &str = "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP\
                           &issuer=Git%20Host&digits=8&period=60&algorithm=SHA256";

    fn auth(uri: &str) -> OtpAuth {
        parse_otpauth(uri).expect("the fixture parses")
    }

    // -----------------------------------------------------------------
    // The field
    // -----------------------------------------------------------------

    #[test]
    fn a_bare_base32_secret_is_read_with_the_forms_own_parameters() {
        let Reading::Ok(a) = read_field("JBSW Y3DP EHPK 3PXP", 8, 60) else {
            panic!("a spaced base32 secret was not accepted");
        };
        assert_eq!(a.secret.as_str(), "JBSWY3DPEHPK3PXP", "spaces were not ignored");
        assert_eq!((a.digits, a.period), (8, 60), "the form's controls were not applied");

        // Control: the SAME text with the default controls takes those
        // instead, so the assertion above is about the controls and not about
        // a constant.
        let Reading::Ok(b) = read_field("JBSW Y3DP EHPK 3PXP", 6, 30) else {
            panic!("the control reading was refused");
        };
        assert_eq!((b.digits, b.period), (6, 30));
    }

    #[test]
    fn a_whole_uri_overrides_the_forms_controls() {
        // The controls say 6/30; the URI says 8/60. The card wins.
        let Reading::Ok(a) = read_field(UNUSUAL, 6, 30) else {
            panic!("a whole URI was not accepted in the same field");
        };
        assert_eq!((a.digits, a.period), (8, 60), "the form silently overrode the card");
        assert_eq!(a.algorithm, Algorithm::Sha256);
        assert_eq!(a.issuer.as_deref(), Some("Git Host"));
        assert_eq!(a.account.as_deref(), Some("anovak"));
    }

    #[test]
    fn the_controls_follow_a_pasted_uri_and_go_dead() {
        let uri = read_field(UNUSUAL, 6, 30);
        assert_eq!(
            controls_for(&uri, UNUSUAL, 6, 30),
            (8, 60, false),
            "the controls kept saying 6/30 beside a confirmation reading 8/60"
        );

        // Control: a typed BARE seed leaves them live and showing the user's
        // own choices, so the disabling above is a decision and not a
        // constant.
        let bare = read_field("JBSWY3DPEHPK3PXP", 8, 60);
        assert_eq!(controls_for(&bare, "JBSWY3DPEHPK3PXP", 8, 60), (8, 60, true));
        let empty = read_field("", 6, 30);
        assert_eq!(controls_for(&empty, "", 6, 30), (6, 30, true));
    }

    #[test]
    fn an_empty_field_is_not_a_refusal() {
        assert!(matches!(read_field("   ", 6, 30), Reading::Empty));
        assert_eq!(validity_line(&read_field("   ", 6, 30)), None, "an empty box was nagged at");
        // Control: something typed DOES produce a line, so the `None` above is
        // about emptiness and not about `validity_line` never speaking.
        assert!(validity_line(&read_field("JBSWY3DPEHPK3PXP", 6, 30)).is_some());
    }

    #[test]
    fn the_validity_line_is_the_designs_own_sentence() {
        let line = validity_line(&read_field("JBSW Y3DP EHPK 3PXP", 6, 30))
            .expect("a valid secret has a line");
        assert_eq!(line, "Valid base32 \u{b7} 16 characters \u{b7} spaces ignored");
    }

    #[test]
    fn a_pasted_plain_url_is_refused_as_a_url_and_not_as_a_bad_secret() {
        // The two are different sentences, and telling a user their base32 is
        // wrong when what they pasted was a web page is the refusal that
        // teaches nothing.
        let Reading::Refused(refusal) = read_field("https://example.com/login", 6, 30) else {
            panic!("a plain URL was accepted");
        };
        assert_eq!(refusal, OtpRefusal::NotOtpAuth);
        assert!(
            refusal_sentence(&refusal).contains("plain URL"),
            "the sentence does not name the reason: {}",
            refusal_sentence(&refusal)
        );

        // Control: a genuinely bad base32 seed still lands on the OTHER
        // refusal, so the routing above is a decision and not a constant.
        let Reading::Refused(bad) = read_field("not!base32", 6, 30) else {
            panic!("a non-base32 secret was accepted");
        };
        assert_eq!(bad, OtpRefusal::BadSecret);
    }

    #[test]
    fn typed_punctuation_cannot_smuggle_a_parameter_into_the_uri() {
        // The field is raw human input and the validator is a URI parser: a
        // typed `&` left unencoded would invent a query parameter. Encoded, it
        // is refused as what it is -- a character that is not base32.
        let Reading::Refused(refusal) = read_field("JBSWY3DPEHPK3PXP&digits=8", 6, 30) else {
            panic!("a typed ampersand was accepted as a secret");
        };
        assert_eq!(refusal, OtpRefusal::BadSecret);

        // And a typed percent escape is not decoded into something else.
        let Reading::Refused(escape) = read_field("JBSW%26Y3DP", 6, 30) else {
            panic!("a typed percent escape was accepted");
        };
        assert_eq!(escape, OtpRefusal::BadSecret);
    }

    #[test]
    fn every_refusal_is_its_own_sentence() {
        let sentences: Vec<String> = [
            OtpRefusal::NotOtpAuth,
            OtpRefusal::NotTotp,
            OtpRefusal::NoSecret,
            OtpRefusal::BadSecret,
            OtpRefusal::PartialSecret(3),
            OtpRefusal::UnknownParameter("surprise".to_string()),
            OtpRefusal::BadParameter("period"),
            OtpRefusal::TooLong,
        ]
        .iter()
        .map(refusal_sentence)
        .collect();
        for (i, one) in sentences.iter().enumerate() {
            assert!(!one.trim().is_empty(), "refusal {i} renders as nothing");
            for (j, other) in sentences.iter().enumerate() {
                assert!(i == j || one != other, "refusals {i} and {j} render the same sentence");
            }
        }
        // Positively: the unknown-parameter sentence QUOTES the key, which is
        // the whole reason that variant carries it.
        assert!(
            refusal_sentence(&OtpRefusal::UnknownParameter("surprise".to_string()))
                .contains("surprise")
        );
        assert!(refusal_sentence(&OtpRefusal::BadParameter("period")).contains("period"));
        // Same, for the one other variant that carries something: the count,
        // which is the whole diagnosis and the only part of a seed this app
        // ever says out loud.
        assert!(refusal_sentence(&OtpRefusal::PartialSecret(3)).contains('3'));
    }

    /// **The reported defect, at the surface the owner saw it on.**
    ///
    /// `asd` in 6d's field answered "Valid base32 · 3 characters · spaces
    /// ignored", with a green check, for a string that is fifteen bits and
    /// cannot decode to bytes. The line under the field now says why instead.
    #[test]
    fn three_good_characters_are_not_a_valid_secret() {
        let Reading::Refused(refusal) = read_field("asd", 6, 30) else {
            panic!("`asd` was accepted as a one-time code secret");
        };
        assert_eq!(refusal, OtpRefusal::PartialSecret(3));

        // The line under the field is the refusal and not the green one. This
        // is the assertion with the teeth in it: the bug was visible entirely
        // through `validity_line`.
        let line = validity_line(&read_field("asd", 6, 30)).expect("a typed field says something");
        assert!(!line.contains("Valid base32"), "the green line survived: {line}");
        assert!(line.contains('3'), "the line does not say how long it is: {line}");

        // The sentence sends the reader to the LENGTH and not to the alphabet.
        // Telling someone whose characters are all fine to check their
        // characters is the generic refusal this module exists to avoid.
        assert!(
            !line.contains("A\u{2013}Z"),
            "the length refusal borrowed the bad-character sentence: {line}"
        );
        assert_ne!(line, refusal_sentence(&OtpRefusal::BadSecret));

        // Paired: `Valid base32 · N characters` is still design 6d's line for
        // a secret that IS one, so nothing above is a wholesale removal of it.
        let good = validity_line(&read_field("JBSW Y3DP EHPK 3PXP", 6, 30)).expect("a line");
        assert!(good.starts_with("Valid base32"), "{good}");
        assert!(good.contains("16 characters"), "{good}");
    }

    /// The new refusal never becomes a **minimum length**, which is a product
    /// decision nobody asked for and which would refuse a legitimate short
    /// seed from some issuer outright.
    ///
    /// Four characters are twenty bits -- two whole bytes and a nibble of
    /// padding -- and are accepted, as they were before. What changed is only
    /// which lengths *cannot exist*.
    #[test]
    fn a_short_secret_that_decodes_is_still_accepted() {
        for typed in ["AB", "ABCD", "ABCDE", "ABCDEFG"] {
            assert!(
                matches!(read_field(typed, 6, 30), Reading::Ok(_)),
                "a {}-character secret that decodes was refused",
                typed.len()
            );
        }
        // Paired, so the loop above is not vacuous: the neighbours of those
        // lengths that do NOT decode are refused, at the same surface.
        for (typed, chars) in [("A", 1usize), ("ABC", 3), ("ABCDEF", 6)] {
            assert!(
                matches!(read_field(typed, 6, 30), Reading::Refused(OtpRefusal::PartialSecret(n)) if n == chars),
                "a {chars}-character secret that cannot decode was accepted"
            );
        }
    }

    #[test]
    fn no_refusal_sentence_can_carry_the_secret() {
        // `BadSecret` holds nothing on purpose. This is the assertion that the
        // rendering did not helpfully add it back.
        let Reading::Refused(refusal) = read_field("SECRETJBSW!!!", 6, 30) else {
            panic!("the fixture was accepted");
        };
        let sentence = refusal_sentence(&refusal);
        assert!(
            !sentence.contains("SECRETJBSW"),
            "the refusal printed what was typed: {sentence}"
        );
    }

    // -----------------------------------------------------------------
    // The masked secret
    // -----------------------------------------------------------------

    /// **The mask is one bullet per character and nothing else.**
    ///
    /// It was grouped in fours, which is what §6c draws. The owner asked for
    /// the mask and the revealed seed to be one shape, and then said which:
    /// "secret jsut store as text no separators". A seed has no separators,
    /// so neither has the thing standing in for it.
    #[test]
    fn the_secret_is_masked_by_length_and_not_grouped() {
        assert_eq!(masked("JBSWY3DPEHPK3PXP"), "\u{2022}".repeat(16));
        assert!(
            !masked("JBSWY3DPEHPK3PXP").contains(' '),
            "the mask is grouped and the seed beside it is not"
        );
        // The two halves of the row are one shape: same length, same lack of
        // separators, so Reveal changes the contents and not the geometry.
        assert_eq!(
            masked("JBSWY3DPEHPK3PXP").chars().count(),
            secret_as_typed("JBSWY3DPEHPK3PXP").chars().count(),
            "the row changes length when it is revealed"
        );
        assert!(
            !masked("JBSWY3DPEHPK3PXP").contains('J'),
            "the mask leaked a character of the seed"
        );
        assert_ne!(
            masked("JBSWY3DP"),
            masked("JBSWY3DPEHPK3PXP"),
            "two seeds of different lengths mask identically, so the row says nothing about \
             what is there"
        );
    }

    // -----------------------------------------------------------------
    // The live code
    // -----------------------------------------------------------------

    /// RFC 4231 section 4.2's first HMAC vector, so the HMAC underneath the
    /// codes is right for a reason other than the codes coming out plausible.
    #[test]
    fn the_hmac_matches_the_published_vectors() {
        let key = [0x0bu8; 20];
        let mac = hmac(Algorithm::Sha256, &key, b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // RFC 2202's first HMAC-SHA-1 vector, same key and message.
        let sha1 = hmac(Algorithm::Sha1, &key, b"Hi There");
        assert_eq!(hex(&sha1), "b617318655057264e28bc0b6fb378c8ef146be00");
        // A key LONGER than the block, which takes the other branch of the
        // padding: RFC 4231 test case 6.
        let long = [0xaau8; 131];
        let big = hmac(Algorithm::Sha256, &long, b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(
            hex(&big),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// RFC 6238's own TOTP vectors, all three algorithms.
    ///
    /// The seeds are the RFC's ASCII ones, base32-encoded: `12345678901234567890`
    /// truncated or repeated to the digest's key length, exactly as appendix B
    /// specifies.
    #[test]
    fn the_codes_match_rfc_6238s_vectors() {
        const SHA1_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        const SHA256_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
        const SHA512_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNA";

        for (seed, algorithm, at, expected) in [
            (SHA1_SEED, "SHA1", 59u64, "94287082"),
            (SHA1_SEED, "SHA1", 1_111_111_109, "07081804"),
            (SHA1_SEED, "SHA1", 1_234_567_890, "89005924"),
            (SHA1_SEED, "SHA1", 2_000_000_000, "69279037"),
            (SHA256_SEED, "SHA256", 59, "46119246"),
            (SHA256_SEED, "SHA256", 1_111_111_109, "68084774"),
            (SHA512_SEED, "SHA512", 59, "90693936"),
            (SHA512_SEED, "SHA512", 1_111_111_109, "25091201"),
        ] {
            let uri = format!(
                "otpauth://totp/rfc?secret={seed}&algorithm={algorithm}&digits=8&period=30"
            );
            let code = code_at(&auth(&uri), at).expect("the vector's seed decodes");
            assert_eq!(
                code.as_str(),
                expected,
                "RFC 6238 vector {algorithm} at T={at} produced the wrong code"
            );
        }
    }

    #[test]
    fn a_six_digit_code_keeps_its_leading_zeros() {
        // A code rendered without its leading zero is a code the site rejects,
        // and the user blames the seed. RFC 6238's SHA-1 vector at T=59 is
        // 94287082 over eight digits; over six it is 287082, and the SAME
        // instant under a seed chosen to produce a leading zero must keep it.
        let six = auth("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&digits=6");
        assert_eq!(code_at(&six, 59).expect("decodes").as_str(), "287082");

        // Every code is exactly as long as the card asked for, at a hundred
        // instants -- which is the property a dropped leading zero breaks.
        for t in 0..100u64 {
            let code = code_at(&six, t * 977).expect("decodes");
            assert_eq!(code.len(), 6, "a six-digit card produced {:?}", code.as_str());
        }
        let eight = auth("otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&digits=8");
        for t in 0..100u64 {
            assert_eq!(code_at(&eight, t * 977).expect("decodes").len(), 8);
        }
    }

    /// A time that is a step boundary for BOTH periods this form offers:
    /// divisible by 30 and by 60, so a test can talk about "the start of the
    /// step" without arithmetic in the assertion.
    const BOUNDARY: u64 = 1_699_999_980;

    #[test]
    fn the_code_changes_on_the_period_boundary_and_not_before() {
        let a = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=30");
        let inside = code_at(&a, BOUNDARY).expect("decodes");
        let still = code_at(&a, BOUNDARY + 29).expect("decodes");
        let after = code_at(&a, BOUNDARY + 30).expect("decodes");
        assert_eq!(inside.as_str(), still.as_str(), "the code changed inside its own step");
        assert_ne!(inside.as_str(), after.as_str(), "the code did not change at the boundary");

        // And a 60-second card steps at 60, so the period is READ rather than
        // assumed.
        let slow = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=60");
        assert_eq!(
            code_at(&slow, BOUNDARY).expect("decodes").as_str(),
            code_at(&slow, BOUNDARY + 59).expect("decodes").as_str()
        );
        assert_ne!(
            code_at(&slow, BOUNDARY).expect("decodes").as_str(),
            code_at(&slow, BOUNDARY + 60).expect("decodes").as_str(),
            "the 60-second card never stepped at all"
        );
    }

    #[test]
    fn the_countdown_runs_from_the_period_down_to_one() {
        let a = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=30");
        assert_eq!(seconds_left(&a, BOUNDARY), 30, "a step boundary should read a full step");
        assert_eq!(seconds_left(&a, BOUNDARY + 1), 29);
        assert_eq!(seconds_left(&a, BOUNDARY + 29), 1, "the countdown never shows a zero");
        let slow = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=60");
        assert_eq!(seconds_left(&slow, BOUNDARY), 60, "the countdown ignored the period");
    }

    #[test]
    fn the_code_is_grouped_for_reading() {
        assert_eq!(grouped_code("640118").as_str(), "640 118");
        assert_eq!(grouped_code("94287082").as_str(), "9428 7082");
    }

    // -----------------------------------------------------------------
    // What is written
    // -----------------------------------------------------------------

    /// **The parameters survive the write.**
    #[test]
    fn what_is_written_round_trips_with_its_parameters() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.typed = Zeroizing::new(UNUSUAL.to_string());

        let written = uri_to_write(&state).expect("a valid URI produces something to write");
        assert!(
            written.starts_with("otpauth://totp/"),
            "the bare seed was written instead of the URI: {}",
            &written[..written.len().min(12)]
        );

        let back = parse_otpauth(&written).expect("what was written parses back");
        assert_eq!(back.secret.as_str(), "JBSWY3DPEHPK3PXP");
        assert_eq!(back.digits, 8);
        assert_eq!(back.period, 60);
        assert_eq!(back.algorithm, Algorithm::Sha256);
        assert_eq!(back.issuer.as_deref(), Some("Git Host"));
        assert_eq!(back.account.as_deref(), Some("anovak"));

        // And the code the confirmation showed is the code the stored value
        // produces -- which is what "the parameters survived" actually means
        // to the user.
        let shown = code_at(&auth(UNUSUAL), 1_700_000_000).expect("decodes");
        let stored = code_at(&back, 1_700_000_000).expect("decodes");
        assert_eq!(shown.as_str(), stored.as_str());
    }

    #[test]
    fn a_typed_bare_seed_is_written_as_a_whole_uri_too() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.typed = Zeroizing::new("JBSW Y3DP EHPK 3PXP".to_string());
        state.digits = 8;
        state.period = 60;
        let written = uri_to_write(&state).expect("a valid seed produces something to write");
        let back = parse_otpauth(&written).expect("what was written parses back");
        assert_eq!(back.secret.as_str(), "JBSWY3DPEHPK3PXP");
        assert_eq!((back.digits, back.period), (8, 60), "the form's controls were not stored");
    }

    #[test]
    fn nothing_is_written_for_an_empty_or_refused_field() {
        let mut state = TotpAdd::opening("id-1", "Git Host", true);
        assert!(uri_to_write(&state).is_none(), "an empty field offered something to write");
        assert!(!can_save(&read_field(&state.typed, state.digits, state.period)));

        state.typed = Zeroizing::new("not!base32".to_string());
        assert!(uri_to_write(&state).is_none(), "a refused field offered something to write");
        assert!(!can_save(&read_field(&state.typed, state.digits, state.period)));

        // Control: a good one DOES, so the two `None`s above are about the
        // field and not about `uri_to_write` never answering.
        state.typed = Zeroizing::new("JBSWY3DPEHPK3PXP".to_string());
        assert!(uri_to_write(&state).is_some());
        assert!(can_save(&read_field(&state.typed, state.digits, state.period)));
    }

    // -----------------------------------------------------------------
    // The replace warning
    // -----------------------------------------------------------------

    /// **Pinned by content**, the way this crate pins its refusal messages.
    #[test]
    fn the_replace_warning_is_the_designs_own_sentence() {
        assert_eq!(
            replace_warning(),
            "This record already has a one-time code. Saving replaces it \u{2014} the old \
             secret cannot be recovered."
        );
        assert_eq!(submit_label(true), "Replace code");
        assert_eq!(submit_label(false), "Save code");
        assert_ne!(
            submit_label(true),
            submit_label(false),
            "the destructive button reads the same as the safe one"
        );
    }

    // -----------------------------------------------------------------
    // The surface itself
    // -----------------------------------------------------------------

    struct Painted(Vec<String>);

    impl Painted {
        fn has(&self, needle: &str) -> bool {
            self.0.iter().any(|t| t.contains(needle))
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => out.0.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    /// `record_ui`'s headless painter, unchanged: two warm-up frames so the
    /// theme's fonts are in place, then one frame whose text shapes are read
    /// back.
    fn paint(draw: impl FnOnce(&mut egui::Ui)) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut draw = Some(draw);
        let output = ctx.run_ui(input(), |ui| {
            (draw.take().expect("run_ui runs the closure once"))(ui);
        });
        let mut painted = Painted(Vec::new());
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            !painted.0.is_empty(),
            "the form painted no text at all, so every assertion over this list would pass \
             against nothing"
        );
        painted
    }

    #[test]
    fn the_confirmation_paints_the_live_code_the_countdown_and_the_parameters() {
        // **A SCANNED card**, because 6c's heading and its field table are the
        // scanned path's -- see `draw_confirmation`. The rows exist to be
        // checked against what the site displayed, and a decoder is the only
        // reader a scanned payload has had.
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.accept_decoded(Zeroizing::new(UNUSUAL.to_string()));
        let painted = paint(|ui| {
            draw_add_form(ui, &mut state, BOUNDARY);
        });

        assert!(painted.has(issuer_row_label()), "the field table the confirmation IS did not draw");
        let expected = code_at(&auth(UNUSUAL), BOUNDARY).expect("decodes");
        assert!(
            painted.has(grouped_code(&expected).as_str()),
            "the live code is not on screen: {:?}",
            painted.0
        );
        // §6d's words, on both cards now: the number and the unit. §6c's
        // `refreshes in 60 s` belonged to the stacked block -- see
        // `draw_code_panel`, which is one strip since the owner asked for
        // one.
        assert!(
            painted.has(&seconds_line(60)),
            "the countdown is not on screen: {:?}",
            painted.0
        );
        // **§6c's question is not drawn any more**, and its absence is the
        // owner's decision rather than a loss this test should hide: the
        // check it invited is what the field table below does, field by
        // field. See `draw_code_panel`.
        assert!(!painted.has(match_question()), "§6c's stacked block is back");
        // The parameters SPELLED OUT: the 8/60/SHA-256 case is exactly the one
        // a confirmation exists to catch.
        assert!(painted.has("SHA256"), "the algorithm is not spelled out: {:?}", painted.0);
        assert!(painted.has("8 digits"), "the digit count is not spelled out");
        assert!(painted.has("60 s"), "the period is not spelled out");
        assert!(painted.has("Git Host"), "the issuer is not shown");
        assert!(painted.has("anovak"), "the account is not shown");
    }

    /// **The parameters are four separate chips**, which is what 6c draws, and
    /// the one-line form anything else prints is built from those four rather
    /// than written out a second time.
    #[test]
    fn the_parameters_are_four_chips_and_the_sentence_is_built_from_them() {
        // The design's own card: `TOTP`, `SHA1`, `6 digits`, `30 s`.
        let plain = auth("otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP");
        assert_eq!(
            parameter_chips(&plain),
            ["TOTP", "SHA1", "6 digits", "30 s"].map(str::to_string)
        );
        // And the unusual card, which is the one this row exists to catch.
        assert_eq!(
            parameter_chips(&auth(UNUSUAL)),
            ["TOTP", "SHA256", "8 digits", "60 s"].map(str::to_string)
        );
        // The joined form is the chips and cannot drift from them.
        assert_eq!(parameters_line(&plain), parameter_chips(&plain).join(" \u{b7} "));
        assert_eq!(parameters_line(&auth(UNUSUAL)), "TOTP \u{b7} SHA256 \u{b7} 8 digits \u{b7} 60 s");
    }

    /// **The countdown track is the countdown**, not a decoration beside it.
    ///
    /// 6c draws the fill at `width: 73%` next to the words *"refreshes in
    /// 22 s"* on a thirty-second card, and 22/30 is 73% -- so the design's own
    /// frame is the assertion, and a track wired to anything else fails here.
    #[test]
    fn the_countdown_track_is_the_seconds_left_over_the_period() {
        let card = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=30");
        // Eight seconds into a step leaves the design's twenty-two.
        let at = BOUNDARY + 8;
        assert_eq!(seconds_left(&card, at), 22);
        assert_eq!(refresh_line(seconds_left(&card, at)), "refreshes in 22 s");
        assert_eq!(
            (countdown_fraction(&card, at) * 100.0).round() as i32,
            73,
            "the track is not the design's 73%"
        );

        // Both ends, so the assertion above is about the arithmetic and not
        // about one lucky instant: a full track at the start of a step, and
        // the smallest one it ever shows at the end.
        assert_eq!(countdown_fraction(&card, BOUNDARY), 1.0);
        assert!((countdown_fraction(&card, BOUNDARY + 29) - 1.0 / 30.0).abs() < f32::EPSILON);
        // A sixty-second card halves at thirty, which a fraction hardcoded to
        // a thirty-second period would get wrong.
        let long = auth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=60");
        assert_eq!(countdown_fraction(&long, BOUNDARY + 30), 0.5);
        // And a made-up zero period is an empty track rather than a division
        // by zero on the UI thread.
        let mut broken = card.clone();
        broken.period = 0;
        assert_eq!(countdown_fraction(&broken, at), 0.0);
    }

    #[test]
    fn the_secret_is_masked_until_reveal_is_pressed() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        // **A SCANNED card**: the masked row lives in 6c's field table, which
        // is the scanned path's. That also removes what used to be this test's
        // one hazard -- a scanned payload is not put in a `TextEdit`, so there
        // is no field painting the user's own typing back at them to defeat
        // the assertion below. It was guarded against before by typing the
        // seed in the spaced grouping a site prints; now it cannot arise.
        state.accept_decoded(Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSW%20Y3DP%20EHPK%203PXP".to_string(),
        ));
        assert!(!state.revealed, "the form opened revealed");

        let masked_frame = paint(|ui| {
            draw_add_form(ui, &mut state, BOUNDARY);
        });
        assert!(
            !masked_frame.has("JBSWY3DPEHPK3PXP"),
            "the seed was painted with nothing revealed: {:?}",
            masked_frame.0
        );
        assert!(masked_frame.has(reveal_label()), "there is no way to reveal it");
        assert!(masked_frame.has("\u{2022}\u{2022}\u{2022}\u{2022}"), "the masked row is missing");

        state.revealed = true;
        let revealed = paint(|ui| {
            draw_add_form(ui, &mut state, BOUNDARY);
        });
        assert!(
            // **Grouped**, because the revealed row and the masked one are
            // one shape now -- see `secret_as_typed`. Built through the same
            // function the row uses rather than written out in fours, so a
            // change to the grouping moves the assertion with it.
            revealed.has(&secret_as_typed("JBSWY3DPEHPK3PXP")),
            "Reveal showed nothing, so the masked assertion above proves nothing: {:?}",
            revealed.0
        );
        assert!(revealed.has(hide_label()), "there is no way back to masked");
    }

    #[test]
    fn the_form_paints_a_refusal_as_its_own_sentence() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.typed = Zeroizing::new("https://example.com".to_string());
        let painted = paint(|ui| {
            draw_add_form(ui, &mut state, 1_700_000_000);
        });
        assert!(painted.has("plain URL"), "the refusal is not on screen: {:?}", painted.0);
        assert!(
            !painted.has(issuer_row_label()),
            "a refused field still painted a confirmation to save from"
        );
    }

    // -----------------------------------------------------------------
    // Design 6a -- the picker's copy
    // -----------------------------------------------------------------

    /// **The four routes, in the design's order.**
    ///
    /// 6a's own words about that order are *"ordered by how often they're the
    /// right one on Windows"*, so the order is content and not layout.
    #[test]
    fn the_picker_offers_the_designs_four_routes_in_its_order() {
        let order: Vec<Route> = routes().iter().map(|r| r.route).collect();
        assert_eq!(
            order,
            vec![Route::ScanRegion, Route::ImageFile, Route::ByHand, Route::Webcam],
            "the routes are not in design 6a's order"
        );
        // **6a's first row, rewritten with the route.** See `ROUTES`: the
        // row now scans before it offers a box to drag, so the design's
        // "Scan a region of my screen" over "Drag a box around the QR code in
        // any window" would describe the fallback rather than the route. Both
        // halves of the replacement are pinned, because both are claims: it
        // says the app looks, and it still names the drag.
        assert_eq!(routes()[0].title, "Scan the code on my screen");
        assert_eq!(
            routes()[0].subtitle,
            "Deskwarden looks for it in every window \u{b7} drag a box if it misses"
        );
        assert!(
            !routes()[0].title.contains("region") && !routes()[0].subtitle.starts_with("Drag"),
            "the scan row still leads with the box the user no longer has to draw"
        );
        assert!(
            routes()[0].subtitle.contains("drag a box"),
            "the scan row no longer names the fallback the overlay still is"
        );
        assert_eq!(routes()[1].title, "Open an image file");
        assert_eq!(routes()[2].title, "Enter the secret by hand");
        assert_eq!(routes()[3].title, "Use a webcam");
        for row in &routes() {
            assert!(!row.subtitle.trim().is_empty(), "{} has no line under it", row.title);
        }
        // **The row names every format the decoder reads, and the design's
        // "PNG, JPG" is now the narrower of the two.** The dialog's filter
        // says the same six, and the two are held to each other here rather
        // than by hope: a row promising a format the decoder cannot read is a
        // promise broken one click later, and a row hiding one it can read
        // sends the user off to convert a file that already worked.
        let spec = crate::file_picker::QR_FILTER_SPEC;
        for (named, extension) in [
            ("PNG", "*.png"),
            ("JPG", "*.jpg"),
            ("GIF", "*.gif"),
            ("BMP", "*.bmp"),
            ("WebP", "*.webp"),
            ("ICO", "*.ico"),
        ] {
            assert!(
                routes()[1].subtitle.contains(named),
                "the image row does not name {named}, which the dialog offers: {}",
                routes()[1].subtitle
            );
            assert!(
                spec.contains(extension),
                "the image row names {named} and the dialog's filter does not offer it"
            );
        }
        // The control on the loop above: a format neither of them names.
        // Without it the assertions pass for a subtitle that names
        // everything.
        assert!(!routes()[1].subtitle.contains("TIFF"), "{}", routes()[1].subtitle);
        assert!(!spec.contains("*.tif"));
    }

    /// **Nothing is deferred: all four of design 6a's routes are live.**
    ///
    /// This test used to say the opposite -- that the webcam row was drawn
    /// dead and said so -- and it is kept, inverted, rather than deleted:
    /// what it pins is the same claim either way, which is that this file
    /// and the picker agree about which routes do something. Both halves
    /// matter, because a row merely absent from an `enabled` check would
    /// satisfy an assertion written one way round only.
    #[test]
    fn no_route_is_deferred_today() {
        let dead: Vec<Route> = routes().iter().filter(|r| !r.enabled).map(|r| r.route).collect();
        assert!(dead.is_empty(), "a route is drawn dead: {dead:?}");
        let live: Vec<Route> = routes().iter().filter(|r| r.enabled).map(|r| r.route).collect();
        assert_eq!(
            live,
            vec![Route::ScanRegion, Route::ImageFile, Route::ByHand, Route::Webcam],
            "the four live routes are not design 6a's four, in its order"
        );
        // The webcam row's own line, which is a promise `crate::webcam`
        // keeps and `PRIVACY.md` repeats.
        let all_routes = routes();
        let webcam = all_routes.iter().find(|r| r.route == Route::Webcam).expect("6a's fourth row");
        assert!(
            webcam.subtitle.contains("stays on this PC"),
            "the camera row does not say where the picture goes: {}",
            webcam.subtitle
        );
        assert!(
            !webcam.subtitle.contains("scan a region"),
            "the camera row still points at the route that used to replace it"
        );
        // And the deferral machinery is still here for the next one.
        assert_eq!(deferred_reason(), "Not in this version");
    }

    /// **The privacy line, verbatim from design 6a.**
    ///
    /// Pinned by content the way this crate pins every sentence a user is
    /// held to. A reworded one must be a deliberate edit that reds this --
    /// and, because of the two tests below it, an edit that has to be
    /// justified against what the code really does.
    #[test]
    fn the_privacy_line_is_the_designs_own_sentence() {
        assert_eq!(
            privacy_line(),
            "Decoding happens on this machine. The captured pixels are discarded once the \
             secret is read, and the secret is never written to disk outside the vault."
        );
    }

    /// The production halves of every file the two scan routes pass through.
    ///
    /// Not the test halves: a needle spelled out in a test below must neither
    /// satisfy nor defeat a claim about the shipping code.
    fn scan_route_sources() -> Vec<(&'static str, String)> {
        [
            ("totp_add.rs", include_str!("totp_add.rs")),
            ("region_overlay.rs", include_str!("../region_overlay.rs")),
            ("screen_capture.rs", include_str!("../screen_capture.rs")),
            ("qr.rs", include_str!("../qr.rs")),
        ]
        .into_iter()
        .map(|(name, whole)| {
            let whole = whole.replace("\r\n", "\n");
            let code = whole.split("#[cfg(test)]").next().unwrap().to_string();
            assert!(code.len() < whole.len(), "{name} has no test module marker to split on");
            (name, code)
        })
        .collect()
    }

    /// **The privacy line's third clause, checked against the code.**
    ///
    /// *"the secret is never written to disk outside the vault"*. Every file
    /// the pixels or the payload pass through is scanned for a write. The one
    /// filesystem call this feature makes is [`read_image_file`]'s
    /// `std::fs::read` -- a read of a file the user already had.
    #[test]
    fn nothing_on_either_route_can_write_the_pixels_or_the_secret_to_disk() {
        let writers = [
            "fs::write",
            "File::create",
            "OpenOptions",
            "write_all",
            "create_dir",
            "tempfile",
            "BufWriter",
        ];
        let sources = scan_route_sources();
        for (name, code) in &sources {
            for needle in writers {
                assert!(
                    !code.contains(needle),
                    "{name} contains `{needle}` -- the pixels are the seed in visual form, and \
                     the picker's privacy line says they never reach the disk"
                );
            }
        }
        // Exactly one filesystem call in the whole feature, and it is a read.
        let totp = &sources.iter().find(|(n, _)| *n == "totp_add.rs").unwrap().1;
        assert_eq!(
            totp.matches("std::fs::").count(),
            1,
            "the image route grew a second filesystem call"
        );
        assert!(
            totp.contains("std::fs::read(path)"),
            "the one filesystem call is not the read this feature is allowed"
        );
        // Positive control on the negatives above: the search really does
        // find things in these files, so "no writer anywhere" is a statement
        // about writers and not about an empty haystack.
        assert!(
            sources.iter().all(|(_, code)| code.contains("Zeroizing")),
            "the source scan found no `Zeroizing` either, so it is reading nothing"
        );
    }

    /// **The privacy line's second clause, checked against the code.**
    ///
    /// *"The captured pixels are discarded once the secret is read"*. On the
    /// region route those pixels are `screen_capture::Rgba`, which wipes on
    /// drop and is dropped inside `read_region_with`. On the image route they
    /// are this file's own buffers, which is the half this file can be held
    /// to directly: both are [`Zeroizing`], and neither is returned.
    #[test]
    fn the_image_routes_own_buffers_are_wiped_and_never_handed_out() {
        let sources = scan_route_sources();
        let totp = &sources.iter().find(|(n, _)| *n == "totp_add.rs").unwrap().1;
        assert!(totp.contains("let bytes = Zeroizing::new(std::fs::read(path)"));
        // **The decoded picture lands in a buffer THIS file owns.** The line
        // below is what says so: `image`'s decoder is asked to write into a
        // `Zeroizing` rather than to hand one back, which is the property the
        // `png`-only version of this route had and which widening the format
        // list did not give up. A `DynamicImage` from `image::ImageReader::
        // decode` would be a full-size copy of the seed's picture in an
        // ordinary allocation, and this pin is what would notice the change.
        assert!(
            totp.contains("let mut buf = Zeroizing::new(vec![0u8; total]);")
                && totp.contains("decoder.read_image(&mut buf)"),
            "the picture is no longer decoded into a buffer this file owns and wipes"
        );
        assert!(totp.contains("let mut rgba = Zeroizing::new(Vec::with_capacity("));
        // And nothing hands pixels back out: the one thing that leaves the
        // image route is a string.
        assert!(
            totp.contains(") -> Result<Zeroizing<String>, PickerRefusal> {"),
            "`decode_image_with`'s answer changed; it must be a string and never pixels"
        );
        // The region route's half, in the module that owns it.
        let overlay = &sources.iter().find(|(n, _)| *n == "region_overlay.rs").unwrap().1;
        assert!(
            overlay.contains("let pixels = match (seams.capture)(rect)"),
            "`read_region_with` no longer owns the captured buffer, so nothing says when it \
             is dropped"
        );
    }

    // -----------------------------------------------------------------
    // The route-level refusals
    // -----------------------------------------------------------------

    /// Every [`PickerRefusal`] there is, so the loops below cannot silently
    /// stop covering one.
    fn every_picker_refusal() -> Vec<PickerRefusal> {
        let mut all = vec![
            PickerRefusal::NoCode(CodeSource::Region),
            PickerRefusal::NoCode(CodeSource::Image),
            PickerRefusal::NotAnImage,
            PickerRefusal::Unreadable,
        ];
        for why in [
            CaptureRefusal::Blocked,
            CaptureRefusal::OffScreen,
            CaptureRefusal::TooSmall,
            CaptureRefusal::TooLarge,
            CaptureRefusal::GdiFailed,
        ] {
            all.push(PickerRefusal::Capture(why));
        }
        all
    }

    #[test]
    fn every_route_refusal_is_its_own_sentence_naming_its_reason() {
        let sentences: Vec<String> =
            every_picker_refusal().iter().map(PickerRefusal::sentence).collect();
        assert_eq!(sentences.len(), 9, "a refusal was added without being covered here");
        for (i, one) in sentences.iter().enumerate() {
            assert!(!one.trim().is_empty(), "refusal {i} renders as nothing");
            assert!(
                one.ends_with('.') && one.split_whitespace().count() >= 6,
                "refusal {i} is not a sentence: {one}"
            );
            for (j, other) in sentences.iter().enumerate() {
                assert!(i == j || one != other, "refusals {i} and {j} render the same sentence");
            }
        }
        // Positively: a capture refusal carries the words `screen_capture`
        // wrote for it, so this surface cannot quietly re-word design 6d's
        // protected-window case into something softer.
        assert!(
            PickerRefusal::Capture(CaptureRefusal::Blocked)
                .sentence()
                .starts_with("Screen capture is blocked"),
            "the blocked-window refusal lost design 6d's headline"
        );
        assert!(PickerRefusal::Capture(CaptureRefusal::Blocked)
            .sentence()
            .contains("marked protected by its app"));
    }

    /// **A file with no QR gives the same named refusal as a region with
    /// none**, which is Task 7's own rule.
    ///
    /// The same variant, from the same function, carrying the same advice.
    /// Only the noun and the second clause follow the route, because "drag
    /// again" is not an instruction a user of the file dialog can act on --
    /// and an instruction that cannot be acted on is the generic refusal this
    /// feature exists to avoid.
    #[test]
    fn a_file_with_no_code_and_a_region_with_none_are_one_refusal() {
        let region = PickerRefusal::NoCode(CodeSource::Region);
        let image = PickerRefusal::NoCode(CodeSource::Image);
        assert!(
            matches!(region, PickerRefusal::NoCode(_))
                && matches!(image, PickerRefusal::NoCode(_)),
            "the two routes report different kinds of failure for the same thing"
        );
        assert!(region.sentence().starts_with("No code in that "));
        assert!(image.sentence().starts_with("No code in that "));
        assert!(region.sentence().contains(no_code_advice()));
        assert!(image.sentence().contains(no_code_advice()));
        // And they are not identical, because one of them would then be
        // telling the wrong user to do the wrong thing.
        assert_ne!(region.sentence(), image.sentence());
        assert!(region.sentence().contains("drag again"));
        assert!(!image.sentence().contains("drag"));
    }

    #[test]
    fn no_route_refusal_can_carry_anything_of_the_payload() {
        // The variants hold a `CaptureRefusal` and a `CodeSource`, both fixed
        // sets. This is the assertion that no arm added the payload back for
        // helpfulness.
        for refusal in every_picker_refusal() {
            let sentence = refusal.sentence();
            assert!(!sentence.contains("JBSW"), "a refusal printed a seed: {sentence}");
            assert!(!sentence.contains("otpauth"), "a refusal printed a payload: {sentence}");
        }
    }

    // -----------------------------------------------------------------
    // The image-file route
    // -----------------------------------------------------------------

    /// Encodes `rgba` as an RGBA8 PNG. `favicon`'s test helper, because the
    /// two need exactly the same thing.
    fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(rgba).expect("png pixel data");
        }
        out
    }

    /// The same, as 8-bit greyscale -- the shape a screenshot tool saving a
    /// black-and-white QR really does produce, and the arm of `expand_to_rgba`
    /// that has to expand one channel into four.
    fn grey_png(width: u32, height: u32, grey: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(grey).expect("png pixel data");
        }
        out
    }

    /// What the seam below was handed, so a test can assert about the pixels
    /// the decoding half produced rather than trust that it produced any.
    static SEEN: std::sync::Mutex<Option<(Vec<u8>, usize, usize)>> =
        std::sync::Mutex::new(None);
    /// What the seam below answers.
    static ANSWER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    /// One test at a time may use the two statics above.
    static SEAM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn recording_decode(rgba: &[u8], width: usize, height: usize) -> Option<Zeroizing<String>> {
        *SEEN.lock().unwrap() = Some((rgba.to_vec(), width, height));
        ANSWER.lock().unwrap().clone().map(Zeroizing::new)
    }

    /// Runs `body` with the recording seam armed to answer `answer`, and hands
    /// back what the seam was shown.
    fn with_recording_seam<T>(
        answer: Option<&str>,
        body: impl FnOnce(&ImageSeams) -> T,
    ) -> (T, Option<(Vec<u8>, usize, usize)>) {
        let _held = SEAM_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        *SEEN.lock().unwrap() = None;
        *ANSWER.lock().unwrap() = answer.map(str::to_string);
        let seams = ImageSeams { decode: recording_decode };
        let out = body(&seams);
        let seen = SEEN.lock().unwrap().clone();
        (out, seen)
    }

    /// **A PNG arrives at the decoder as the pixels it was made from.**
    ///
    /// This is the half of the image route that is this file's own -- the
    /// decode itself is `qr`'s, and is tested there against a real QR code.
    /// What can go wrong here is a row order, a channel order or a stride,
    /// and none of those would be visible from a test that only asked whether
    /// a decode succeeded.
    #[test]
    fn a_png_reaches_the_decoder_as_the_exact_pixels_it_was_made_from() {
        // A gradient, so a transposed or reversed buffer is not the same
        // buffer. 7x5 is deliberately neither square nor a multiple of four.
        let source: Vec<u8> = (0..7 * 5 * 4).map(|i| (i % 251) as u8).collect();
        let png = rgba_png(7, 5, &source);
        let (out, seen) =
            with_recording_seam(Some("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"), |seams| {
                decode_image_with(seams, &png)
            });
        assert!(out.is_ok(), "a well-formed PNG was refused: {:?}", out.err());
        let (rgba, width, height) = seen.expect("the decoder was never called");
        assert_eq!((width, height), (7, 5), "the dimensions were not carried through");
        assert_eq!(rgba, source, "the pixels handed to the decoder are not the ones in the file");
    }

    #[test]
    fn a_greyscale_png_is_expanded_to_opaque_rgba() {
        // The arm a black-and-white screenshot takes. Without it a QR saved
        // as greyscale would reach the decoder at a quarter of the size it
        // claimed, and be reported as "no code in that image".
        let grey: Vec<u8> = vec![0x00, 0x40, 0x80, 0xff, 0x11, 0x22];
        let png = grey_png(3, 2, &grey);
        let (out, seen) =
            with_recording_seam(Some("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"), |s| {
                decode_image_with(s, &png)
            });
        assert!(out.is_ok(), "a greyscale PNG was refused: {:?}", out.err());
        let (rgba, width, height) = seen.expect("the decoder was never called");
        assert_eq!((width, height), (3, 2));
        assert_eq!(rgba.len(), 3 * 2 * 4, "the expansion produced the wrong number of bytes");
        let expected: Vec<u8> = grey.iter().flat_map(|g| [*g, *g, *g, 255u8]).collect();
        assert_eq!(rgba, expected, "greyscale was not expanded to opaque RGBA");
    }

    // -- one case per format the route reads ---------------------------
    //
    // These run through [`decode_image`] -- the PRODUCTION seam, the real
    // `qr::decode_qr` -- rather than through the recording seam above,
    // because the question they answer is end-to-end: does a QR code inside a
    // JPEG come back out as the URI it was made from. A recording seam would
    // only say that some pixels arrived.
    //
    // The code inside every one of them is `qr::tests::FIXTURE`, rendered
    // here rather than copied: a matrix generated outside this repository by
    // a crate that is not a dependency of this app (see `qr.rs` on why that
    // independence is what makes a decode evidence of anything). What each
    // case adds on top of that is the FORMAT -- so a failure means the format
    // arm is missing, or the colour type came back wrong, or the rows are
    // upside down, and not that a new fixture happens not to decode.

    /// The fixture as the two pixel layouts the encoders below want: RGBA8,
    /// and RGB8 for JPEG, which has no alpha channel to give.
    ///
    /// Returns `(rgba, rgb, width, height)`.
    fn fixture_pixels(scale: usize) -> (Vec<u8>, Vec<u8>, u32, u32) {
        let (rgba, width, height) = crate::qr::tests::fixture_rgba(scale);
        let rgb: Vec<u8> =
            rgba.as_chunks::<4>().0.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        (rgba, rgb, width as u32, height as u32)
    }

    /// **Every format the file dialog offers decodes to the code inside it.**
    ///
    /// One table rather than six tests, because the assertion is identical
    /// for all of them and a table makes a missing row visible where six
    /// functions would not.
    #[test]
    fn every_format_the_dialog_offers_decodes_to_the_code_inside_it() {
        use image::ExtendedColorType::{Rgb8, Rgba8};
        use image::ImageEncoder as _;

        // An ICO entry's dimensions are one byte each, so 256 is the whole
        // container's ceiling. The table is rendered small enough to fit it
        // rather than testing one format at a size the others are not.
        let (rgba, rgb, width, height) = fixture_pixels(4);
        assert!(
            width <= 256 && height <= 256,
            "the fixture no longer fits an ICO entry at {width}x{height}"
        );

        let mut files: Vec<(&str, Vec<u8>)> = Vec::new();
        // PNG through the `png` crate rather than through `image`'s encoder,
        // so at least one row of this table is not the same library
        // agreeing with itself about a container.
        files.push(("PNG", rgba_png(width, height, &rgba)));
        {
            let mut out = Vec::new();
            image::codecs::bmp::BmpEncoder::new(&mut out)
                .write_image(&rgba, width, height, Rgba8)
                .expect("BMP encodes");
            files.push(("BMP", out));
        }
        {
            let mut out = Vec::new();
            image::codecs::gif::GifEncoder::new(&mut out)
                .write_image(&rgba, width, height, Rgba8)
                .expect("GIF encodes");
            files.push(("GIF", out));
        }
        {
            let mut out = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .write_image(&rgba, width, height, Rgba8)
                .expect("WebP encodes");
            files.push(("WebP", out));
        }
        {
            let mut out = Vec::new();
            image::codecs::ico::IcoEncoder::new(&mut out)
                .write_image(&rgba, width, height, Rgba8)
                .expect("ICO encodes");
            files.push(("ICO", out));
        }
        {
            // **The one lossy row, and the only one whose bytes are not the
            // fixture's.** Quality 100 still runs the code through a DCT and
            // a chroma pass; that it survives is the point, since a photo of
            // a QR code off a phone is exactly this and worse.
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 100)
                .write_image(&rgb, width, height, Rgb8)
                .expect("JPEG encodes");
            files.push(("JPEG", out));
        }

        assert_eq!(files.len(), 6, "a format was dropped from the table");
        for (format, bytes) in &files {
            // The dialog offers it, so the decoder had better read it.
            assert!(
                !bytes.is_empty(),
                "{format} encoded to nothing, so the assertion below proves nothing"
            );
            let decoded = decode_image(bytes)
                .unwrap_or_else(|why| panic!("a QR code in a {format} was refused: {why:?}"));
            assert_eq!(
                decoded.as_str(),
                crate::qr::tests::FIXTURE_TEXT,
                "a {format} decoded to something other than the code inside it"
            );
        }

        // **The control**: the same pipeline on a picture with no code in it
        // answers the "no code" refusal rather than a URI. Without it, a
        // `decode_image` that returned the fixture text unconditionally would
        // satisfy every assertion above.
        let blank = rgba_png(8, 8, &vec![0xffu8; 8 * 8 * 4]);
        assert_eq!(
            decode_image(&blank).err(),
            Some(PickerRefusal::NoCode(CodeSource::Image)),
            "the control decoded a code out of a blank picture"
        );
    }

    /// **A sixteen-bit PNG is narrowed rather than refused.**
    ///
    /// PNG is the only format in this app's list that carries sixteen bits a
    /// channel, so this is the one test that can reach `expand_to_rgba`'s
    /// wide arms -- and without it those arms would be four untested branches
    /// standing between a screenshot and a refusal.
    #[test]
    fn a_sixteen_bit_png_is_narrowed_to_rgba_rather_than_refused() {
        let (rgba, _, width, height) = fixture_pixels(4);
        // Every eight-bit sample widened into BOTH halves of a sixteen-bit
        // one, which is what saving at that depth does to a picture that had
        // eight bits to begin with. The `png` crate writes those samples
        // big-endian, as the format requires, and `image` hands them back in
        // the machine's own byte order -- which is the difference `high_byte`
        // exists so that this file does not have to care about.
        let wide: Vec<u8> = rgba.iter().flat_map(|sample| [*sample, *sample]).collect();
        assert_eq!(wide.len(), rgba.len() * 2, "the widening produced the wrong length");

        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&wide).expect("png pixel data");
        }
        let decoded = decode_image(&out).expect("a sixteen-bit PNG was refused");
        assert_eq!(decoded.as_str(), crate::qr::tests::FIXTURE_TEXT);
    }

    /// **A picture with no QR in it is the named "no code" refusal**, and a
    /// picture with one is not.
    #[test]
    fn an_image_with_no_code_is_refused_by_name_and_one_with_a_code_is_not() {
        let blank = rgba_png(8, 8, &vec![0xffu8; 8 * 8 * 4]);
        let (refused, _) = with_recording_seam(None, |s| decode_image_with(s, &blank));
        assert_eq!(
            refused.err(),
            Some(PickerRefusal::NoCode(CodeSource::Image)),
            "a picture with no code did not produce the refusal that names why"
        );

        // The control: the SAME bytes through a seam that finds something do
        // decode, so the refusal above is about the decoder's answer and not
        // about the PNG being unreadable.
        let (found, _) =
            with_recording_seam(Some("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"), |s| {
                decode_image_with(s, &blank)
            });
        assert_eq!(
            found.expect("the control decodes").as_str(),
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"
        );
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_refused_as_a_file_and_not_as_a_missing_code() {
        for (what, bytes) in [
            ("empty", Vec::new()),
            ("text", b"this is not a picture at all".to_vec()),
            ("a truncated png", rgba_png(8, 8, &vec![0u8; 8 * 8 * 4])[..20].to_vec()),
        ] {
            let (out, seen) =
                with_recording_seam(Some("ignored"), |s| decode_image_with(s, &bytes));
            assert_eq!(
                out.err(),
                Some(PickerRefusal::NotAnImage),
                "{what} was not refused as a file"
            );
            assert!(seen.is_none(), "{what} reached the QR decoder");
        }
        // Control: a real one is NOT refused, so the three above are about the
        // bytes rather than about `image_to_rgba` refusing everything.
        let good = rgba_png(8, 8, &vec![0u8; 8 * 8 * 4]);
        let (out, seen) = with_recording_seam(Some("ok"), |s| decode_image_with(s, &good));
        assert!(out.is_ok());
        assert!(seen.is_some());
    }

    /// **The production seam is the real decoder, by address.**
    ///
    /// Without this, every test above could be passing against a stub while
    /// the shipping path called something else entirely.
    #[test]
    fn the_image_route_decodes_through_the_real_qr_reader() {
        let real: fn(&[u8], usize, usize) -> Option<Zeroizing<String>> = crate::qr::decode_qr;
        assert!(
            std::ptr::fn_addr_eq(ImageSeams::production().decode, real),
            "the image route's production seam is not `qr::decode_qr`"
        );
    }

    #[test]
    fn a_file_that_cannot_be_opened_is_its_own_refusal() {
        // A path under the OS temp directory that this test does not create.
        // Nothing is written, and nothing under the app's own data directory
        // is touched.
        let missing = std::env::temp_dir().join("deskwarden-no-such-qr-image-9f3a1c.png");
        assert!(!missing.exists(), "the fixture path unexpectedly exists");
        assert_eq!(read_image_file(&missing).err(), Some(PickerRefusal::Unreadable));
    }

    // -----------------------------------------------------------------
    // What the two scanned routes do to the form
    // -----------------------------------------------------------------

    #[test]
    fn a_decoded_region_lands_in_the_field_and_opens_the_confirmation() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.revealed = true;
        apply_region_outcome(&mut state, Outcome::Decoded(Zeroizing::new(UNUSUAL.to_string())));

        assert_eq!(state.stage, Stage::Manual, "a decode did not open the confirmation");
        assert_eq!(state.typed.as_str(), UNUSUAL, "the decoded URI is not what will be saved");
        assert_eq!(state.refusal, None);
        assert!(!state.revealed, "a decode arrived with the previous seed still unmasked");

        // And what it will write is the whole URI with its parameters. The
        // scanned route and the typed route reaching the same place is the
        // whole reason there is one field.
        let written = uri_to_write(&state).expect("a decoded URI is savable");
        let back = parse_otpauth(&written).expect("what was written parses back");
        assert_eq!(back.digits, 8);
        assert_eq!(back.period, 60);
        assert_eq!(back.algorithm, Algorithm::Sha256);
    }

    #[test]
    fn every_other_region_outcome_goes_back_to_the_picker_saying_why() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.stage = Stage::Scanning;
        apply_region_outcome(&mut state, Outcome::NoCode);
        assert_eq!(state.stage, Stage::Picker);
        assert_eq!(state.refusal, Some(PickerRefusal::NoCode(CodeSource::Region)));

        state.stage = Stage::Scanning;
        apply_region_outcome(&mut state, Outcome::Refused(CaptureRefusal::Blocked));
        assert_eq!(state.stage, Stage::Picker);
        assert_eq!(state.refusal, Some(PickerRefusal::Capture(CaptureRefusal::Blocked)));

        // **Escape says nothing.** The user closed a surface they opened;
        // narrating that back at them is what a dismissed dialog is
        // deliberately silent about everywhere else in this window.
        state.stage = Stage::Scanning;
        apply_region_outcome(&mut state, Outcome::Cancelled);
        assert_eq!(state.stage, Stage::Picker);
        assert_eq!(state.refusal, None, "cancelling was reported as a failure");
    }

    #[test]
    fn a_cancelled_file_dialog_says_nothing_and_a_bad_file_says_what() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.refusal = Some(PickerRefusal::NotAnImage);
        apply_image_pick(&mut state, None);
        assert_eq!(state.refusal, None, "a cancelled dialog left a refusal on screen");
        assert_eq!(state.stage, Stage::Picker);

        let missing = std::env::temp_dir().join("deskwarden-no-such-qr-image-2b7e40.png");
        apply_image_pick(&mut state, Some(&missing));
        assert_eq!(state.refusal, Some(PickerRefusal::Unreadable));
        assert_eq!(state.stage, Stage::Picker);
    }

    /// **A hostile QR is refused by exactly the sentence a hostile paste is.**
    ///
    /// The payload is whatever was on the user's screen, and anyone who can
    /// talk them into scanning a code chooses it. There is one validator on
    /// both routes, so there is one refusal.
    #[test]
    fn a_scanned_payload_is_refused_by_the_same_sentence_a_pasted_one_is() {
        let mut scanned = TotpAdd::opening("id-1", "Git Host", false);
        apply_region_outcome(
            &mut scanned,
            Outcome::Decoded(Zeroizing::new("https://example.com/login".to_string())),
        );
        let Reading::Refused(from_scan) =
            read_field(&scanned.typed, scanned.digits, scanned.period)
        else {
            panic!("a plain URL scanned off the screen was accepted as a code");
        };
        let Reading::Refused(from_paste) = read_field("https://example.com/login", 6, 30) else {
            panic!("the control was accepted");
        };
        assert_eq!(from_scan, from_paste);
        assert_eq!(refusal_sentence(&from_scan), refusal_sentence(&from_paste));
        assert!(refusal_sentence(&from_scan).contains("plain URL"));
    }

    #[test]
    fn stepping_back_to_the_picker_takes_the_seed_with_it() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.typed = Zeroizing::new("JBSWY3DPEHPK3PXP".to_string());
        state.stage = Stage::Manual;
        state.revealed = true;
        state.back_to_picker();
        assert_eq!(state.stage, Stage::Picker);
        assert!(state.typed.is_empty(), "the field kept a seed the user stepped away from");
        assert!(!state.revealed, "the reveal survived the step back");
        assert_eq!(state.refusal, None);
    }

    // -----------------------------------------------------------------
    // The live code, pressed
    // -----------------------------------------------------------------

    /// **Every copy this card makes names itself in the app's own
    /// confirmation.**
    ///
    /// The wording is the one thing a toast raised from here can get wrong in
    /// a way nothing else would catch: the toast reads `{label} copied`, and a
    /// label that drifted from the row the user pressed would be a
    /// confirmation about a different field. So the row labels themselves are
    /// the labels, and the live code takes the READ PANE's word for the same
    /// value -- two words for one credential is two credentials as far as a
    /// reader is concerned.
    #[test]
    fn every_copy_names_itself_in_the_rows_own_word() {
        assert_eq!(copied_label(Some(TableField::Issuer)), issuer_row_label());
        assert_eq!(copied_label(Some(TableField::Account)), account_row_label());
        assert_eq!(copied_label(Some(TableField::Secret)), secret_row_label());
        assert_eq!(
            copied_label(None),
            crate::vault_window::detail::copy_shortcut_label(
                crate::vault_window::detail::CopyShortcut::Totp
            ),
            "the card and the read pane call the same credential two different things"
        );
    }

    /// **What is copied is what was drawn**, asked with the same clock.
    #[test]
    fn the_code_offered_to_the_clipboard_is_the_one_on_the_strip() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.accept_decoded(Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP".to_string(),
        ));
        let auth = match read_field(&state.typed, state.digits, state.period) {
            Reading::Ok(auth) => auth,
            // `Reading` has no `Debug`, and deliberately: it carries a
            // decoded payload. So the failure names the fixture rather
            // than printing what was read out of it.
            _ => panic!("the fixture no longer reads as a code"),
        };
        let drawn = code_at(&auth, BOUNDARY).expect("the strip has a code to draw");
        let copied = code_to_copy(&state, BOUNDARY).expect("and one to copy");
        assert_eq!(*copied, *drawn, "the clipboard would get a different code");

        // And a field that is not a code offers nothing -- which is every
        // state in which there is no strip on screen to press.
        let mut empty = TotpAdd::opening("id-1", "Git Host", false);
        assert!(code_to_copy(&empty, BOUNDARY).is_none());
        empty.typed = Zeroizing::new("https://example.com".to_string());
        assert!(code_to_copy(&empty, BOUNDARY).is_none());
    }

    /// **A copy never travels with an answer that closes the form.**
    ///
    /// Read off the source, because producing the collision through a frame
    /// would mean pressing two controls at once. What it pins is the order:
    /// the copy is only adopted when nothing else was reported.
    #[test]
    fn a_copy_cannot_outrank_save_or_cancel() {
        let source = include_str!("totp_add/ui.rs").replace("\r\n", "\n");
        let code = source.as_str();
        assert!(
            code.contains(
                "if copy_asked != TotpAddAction::None && action == TotpAddAction::None {"
            ),
            "the live code's press is adopted without checking what else the frame reported, \
             so a frame that also pressed Save would copy on the way out"
        );
    }

    // -----------------------------------------------------------------
    // The picker as a surface -- pressed, not called
    // -----------------------------------------------------------------

    // -----------------------------------------------------------------
    // Design 6a's geometry, read off the painted shapes.
    //
    // Every number asserted below was measured in a browser against
    // `docs/design/Deskwarden.dc.html`'s own `id="6a"` panel -- the card's
    // rectangle is 470 x 455, its rows 440 x 62, its tiles 36 x 36 -- rather
    // than derived from the constants these tests are meant to hold. A test
    // that recomputed `ROW_PAD_Y * 2.0 + ...` would agree with any value
    // those constants ever took.
    // -----------------------------------------------------------------

    /// **A row that is drawn and dead has a key that is dead too.**
    ///
    /// [`DEFERRED_REASON`]'s row cannot be clicked and says why. A digit that
    /// walked into the route anyway would be worse than a dead keycap: it is
    /// a live key with no affordance at all, on a route the card has just
    /// said is unavailable.
    ///
    /// Driven through a synthetic table because nothing in [`ROUTES`] is
    /// deferred today -- see [`route_for_key`]'s own note.
    #[test]
    fn a_deferred_rows_key_does_nothing() {
        let mut deferred = routes();
        for row in deferred.iter_mut() {
            row.enabled = false;
        }
        for (key, _) in ROUTE_KEYS {
            assert_eq!(
                route_for_key(&deferred, key, egui::Modifiers::default()),
                None,
                "{key:?} walked into a route whose row is drawn dead"
            );
            // And the control: the same key on the live table does answer, so
            // the assertion above is about `enabled` and not about the key
            // being unbound.
            assert!(
                route_for_key(&routes(), key, egui::Modifiers::default()).is_some(),
                "{key:?} answers nothing even on the live table"
            );
        }
    }

    /// **Every keycap the card draws is a key that works.**
    ///
    /// This replaces `the_second_rows_digit_is_the_one_key_still_owed`,
    /// which existed only so that one missing binding could not go quiet. It
    /// is gone because the debt is paid, and what stands in its place is the
    /// rule that debt was a violation of: a drawn keycap that answers
    /// nothing is the defect, so the pin is over every row rather than over
    /// the one row that happened to be wrong.
    ///
    /// Enter and `1` both reach the first row -- the design draws the
    /// keycap as the return mark and the owner asked for the digits -- so
    /// the assertion is that every row is REACHABLE, not that every row has
    /// exactly one key.
    #[test]
    fn every_row_the_picker_draws_answers_its_own_digit() {
        for (row, _) in routes().iter().enumerate() {
            assert!(
                ROUTE_KEYS.iter().any(|(_, bound)| *bound == row),
                "row {row} is drawn with a keycap and no key in `ROUTE_KEYS` reaches it"
            );
        }
        // The digits line up with the rows they are painted on, rather than
        // every row merely being reachable by something.
        for (key, row) in [
            (egui::Key::Num1, 0),
            (egui::Key::Num2, 1),
            (egui::Key::Num3, 2),
            (egui::Key::Num4, 3),
        ] {
            assert_eq!(
                ROUTE_KEYS.iter().find(|(k, _)| *k == key).map(|(_, r)| *r),
                Some(row),
                "{key:?} does not pick the row it is drawn on"
            );
        }
        assert_eq!(routes()[1].route, Route::ImageFile);
    }

    /// **The typed route still gets its field**, so the row above is a
    /// decision about scanned payloads and not a field that vanished for
    /// everyone.
    #[test]
    fn the_by_hand_route_still_gets_an_editable_field() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.stage = Stage::Manual;
        assert!(!state.scanned);
        let painted = paint(|ui| {
            draw_stage(ui, &mut state, BOUNDARY);
        });
        assert!(painted.has("JBSW Y3DP EHPK 3PXP"), "the by-hand field is gone: {:?}", painted.0);
        assert!(!painted.has(code_read_label()), "the by-hand route drew the scanned row");
    }

    /// The caution band is **not** part of the gate above.
    ///
    /// It is a warning about what saving will destroy rather than a field
    /// restating an input, and 6d's mockup has no record behind it with a code
    /// to lose -- so "6d draws no such band" says nothing about whether this
    /// app should. A typed card over an item that already has a code still
    /// shows it.
    #[test]
    fn the_replace_warning_is_not_gated_with_the_field_table() {
        let mut typed = TotpAdd::opening("id-1", "Git Host", true);
        typed.stage = Stage::Manual;
        typed.typed = Zeroizing::new(UNUSUAL.to_string());
        let painted = paint(|ui| {
            draw_stage(ui, &mut typed, BOUNDARY);
        });
        assert!(
            painted.has(replace_warning()),
            "the caution band went out with 6c's table: {:?}",
            painted.0
        );
        assert!(painted.has(replace_label()), "the destructive button stopped saying so");
        // Control, in the same frame: the table really is gone from this card,
        // so the band above survived a gate rather than there being no gate.
        assert!(!painted.has(issuer_row_label()));
    }

    // -----------------------------------------------------------------
    // The wiring, pinned at its far end
    // -----------------------------------------------------------------

    /// **The window really opens the overlay, and really applies its answer.**
    ///
    /// Both arms are inside `vault_window::run`'s frame closure, which no
    /// harness in this crate can call -- the same reason `mod.rs` pins its
    /// other closure-only decisions by source. The behavioural half is the
    /// click tests above, which prove the row reports `ScanRegion`; this is
    /// the far end of that wire.
    #[test]
    fn the_vault_window_opens_the_overlay_and_applies_what_it_answers() {
        let window = include_str!("mod.rs").replace("\r\n", "\n");
        let code = window.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < window.len(), "the test module marker was not found");
        for needle in [
            "crate::region_overlay::RegionOverlay::open(",
            "crate::screen_capture::monitor_bounds()",
            "totp_add::apply_region_outcome(state, outcome)",
            "crate::file_picker::pick_qr_image()",
            "totp_add::apply_image_pick(",
            "overlay.show(ui.ctx())",
        ] {
            assert!(code.contains(needle), "`{needle}` is not in the vault window's frame");
        }
        // Positive control on the split: a needle only ever spelled out below
        // the marker is not found above it.
        assert!(!code.contains("the_vault_window_opens_the_overlay_and_applies"));
    }

    // -----------------------------------------------------------------
    // Design 6d's geometry, read off the painted shapes.
    //
    // Every number asserted below was measured in a browser against
    // `docs/design/Deskwarden.dc.html`'s own `id="6d"` panel -- the card is
    // 470 x 345.8, its field 436 x 40.8, each parameter run 212 x 26 and its
    // footer 468 x 59 -- rather than derived from the constants these tests
    // are meant to hold. A test that recomputed `MANUAL_FOOTER_PAD_Y * 2.0 +
    // ...` would agree with any value those constants ever took.
    // -----------------------------------------------------------------

    /// Every input the one field can be driven into a refusal with, beside
    /// the refusal it produces.
    ///
    /// A table rather than seven inline literals because three tests below
    /// walk it and the thing they are checking is a property of the WHOLE
    /// family: no refusal may outgrow the line design 6d draws for it, and a
    /// variant added to [`OtpRefusal`] without an entry here is a sentence
    /// nobody ever measured.
    fn every_refusal_the_field_can_show() -> Vec<(String, OtpRefusal)> {
        vec![
            ("https://example.com/login".to_string(), OtpRefusal::NotOtpAuth),
            (
                "otpauth://hotp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&counter=1".to_string(),
                OtpRefusal::NotTotp,
            ),
            (
                "otpauth://totp/Git%20Host:anovak?issuer=Git%20Host".to_string(),
                OtpRefusal::NoSecret,
            ),
            ("not!base32".to_string(), OtpRefusal::BadSecret),
            ("sdf".to_string(), OtpRefusal::PartialSecret(3)),
            (
                "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&surprise=icon.png"
                    .to_string(),
                OtpRefusal::UnknownParameter("surprise".to_string()),
            ),
            (
                "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&period=0".to_string(),
                OtpRefusal::BadParameter("period"),
            ),
            // The one refusal that is about the URI's SIZE, so the fixture
            // has to be that size: a bare seed this long becomes a URI past
            // `otpauth::MAX_URI_LEN` on its way through `secret_as_uri`.
            ("A".repeat(crate::otpauth::MAX_URI_LEN), OtpRefusal::TooLong),
        ]
    }

    /// **The table above really drives what it claims to**, which every
    /// assertion built on it depends on: a fixture that quietly stopped
    /// producing its refusal would take three tests down to vacuity without
    /// failing any of them.
    #[test]
    fn every_refusal_fixture_still_produces_its_refusal() {
        for (typed, expected) in every_refusal_the_field_can_show() {
            let Reading::Refused(actual) = read_field(&typed, 6, 30) else {
                panic!("{expected:?}'s fixture is no longer refused at all");
            };
            assert_eq!(actual, expected, "the fixture for {expected:?} now produces {actual:?}");
        }
        // And it covers every variant, counted against the enumeration's own
        // list -- `every_refusal_is_its_own_sentence` keeps that list honest.
        assert_eq!(
            every_refusal_the_field_can_show().len(),
            8,
            "a refusal was added to `OtpRefusal` without an input that reaches it, so the \
             sentence it renders is one nobody has ever measured on screen"
        );
    }

    /// **The length refusal is grammatical at every count in its class**,
    /// including one.
    ///
    /// One character is five bits, which is not whole bytes, so
    /// `PartialSecret(1)` is reachable by typing a single letter -- and "1
    /// characters cannot decode" is the sentence that makes a user doubt the
    /// rest of the card.
    #[test]
    fn the_length_refusal_is_grammatical_at_one() {
        let one = refusal_sentence(&OtpRefusal::PartialSecret(1));
        assert!(one.starts_with("1 character "), "{one}");
        assert!(!one.contains("1 characters"), "{one}");
        // And it is still plural everywhere else, so the arm above is a
        // special case rather than a dropped `s`.
        assert!(refusal_sentence(&OtpRefusal::PartialSecret(3)).starts_with("3 characters "));
        assert!(refusal_sentence(&OtpRefusal::PartialSecret(11)).starts_with("11 characters "));
        // Reachable, not hypothetical: one typed character really lands here.
        assert!(matches!(read_field("A", 6, 30), Reading::Refused(OtpRefusal::PartialSecret(1))));
    }

    // -----------------------------------------------------------------
    // The webcam route, with no camera anywhere
    //
    // Every state this route can be in is reachable here, because the two
    // calls that need a device are behind `crate::webcam::WebcamSeams` and
    // the capture loop is an ordinary function pointer. No test below opens a
    // camera, enumerates one, or would behave differently on a machine that
    // has one.
    // -----------------------------------------------------------------

    use crate::webcam::{Device, Sink, WebcamSeams};

    /// What a camera hands back, in this block. A real `otpauth://` URI so
    /// the confirmation the decode opens has something to confirm.
    const CAMERA_URI: &str = "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP";

    /// Two cameras, named the way Windows names them.
    fn two_cameras() -> Vec<Device> {
        vec![
            Device { name: "Integrated Camera".to_string(), id: r"\\?\usb#one".to_string() },
            Device { name: "Logi C920".to_string(), id: r"\\?\usb#two".to_string() },
        ]
    }

    fn one_camera() -> Vec<Device> {
        two_cameras().into_iter().take(1).collect()
    }

    /// A capture loop that opens nothing and sends nothing, until it is
    /// stopped. What a camera with the lens cap on looks like from here.
    fn silent_pump(_: Device, _: crate::webcam::DecodeFn, sink: Sink) {
        while sink.wanted() {
            std::thread::yield_now();
        }
    }

    /// A capture loop that offers one white frame and then waits.
    fn one_frame_pump(_: Device, _: crate::webcam::DecodeFn, sink: Sink) {
        if let Some(frame) =
            crate::webcam::Frame::new(4, 3, Zeroizing::new(vec![0xffu8; 4 * 3 * 4]))
        {
            sink.show(frame);
        }
        while sink.wanted() {
            std::thread::yield_now();
        }
    }

    /// A capture loop whose device is already open somewhere else.
    fn busy_pump(_: Device, _: crate::webcam::DecodeFn, sink: Sink) {
        sink.refuse(CameraRefusal::Busy);
    }

    /// A capture loop that reads a code off its first frame.
    fn reading_pump(_: Device, _: crate::webcam::DecodeFn, sink: Sink) {
        sink.read(Zeroizing::new(CAMERA_URI.to_string()));
    }

    fn seams(pump: crate::webcam::PumpFn) -> WebcamSeams {
        WebcamSeams { pump, ..WebcamSeams::production() }
    }

    /// Spins until `check` answers, or fails after a bounded wait. See
    /// `webcam`'s own copy of this for why it is a spin and not a sleep.
    fn until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("timed out waiting for {what}");
    }

    /// **One camera opens straight away; two ask first, and open neither.**
    ///
    /// The second half is the privacy-relevant one: a list of cameras must
    /// not switch any of them on to draw itself.
    #[test]
    fn one_camera_opens_at_once_and_several_are_offered_with_none_switched_on() {
        let seams = seams(silent_pump);

        let mut alone = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut alone, &seams, Ok(one_camera()));
        assert_eq!(alone.stage, Stage::Webcam);
        let stage = alone.webcam.as_ref().expect("a camera stage");
        assert_eq!(stage.chosen, Some(0), "the only camera was not opened");
        assert!(stage.session.is_some(), "the only camera has no capture loop");

        let mut several = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut several, &seams, Ok(two_cameras()));
        assert_eq!(several.stage, Stage::Webcam);
        let stage = several.webcam.as_ref().expect("a camera stage");
        assert_eq!(stage.chosen, None, "a camera was chosen for the user");
        assert!(stage.session.is_none(), "a device was opened before anyone picked one");
        assert_eq!(stage.devices.len(), 2, "the list the user has to choose from is not held");
    }

    /// **Every refusal from the enumeration lands back on the picker, named.**
    #[test]
    fn a_camera_that_will_not_open_sends_the_user_back_to_the_picker_with_the_reason() {
        let seams = seams(silent_pump);
        for why in [CameraRefusal::NoCamera, CameraRefusal::Denied, CameraRefusal::Busy] {
            let mut state = TotpAdd::opening("id-1", "Git Host", false);
            open_webcam(&mut state, &seams, Err(why));
            assert_eq!(state.stage, Stage::Picker, "{why:?} left the user on the camera stage");
            assert_eq!(state.refusal, Some(PickerRefusal::Camera(why)));
            assert!(state.webcam.is_none(), "{why:?} left a camera stage behind");
        }
        // An empty list is the same answer, so a seam that hands one back
        // cannot produce a stage with nothing in it.
        let mut empty = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut empty, &seams, Ok(Vec::new()));
        assert_eq!(empty.stage, Stage::Picker);
        assert_eq!(empty.refusal, Some(PickerRefusal::Camera(CameraRefusal::NoCamera)));
    }

    /// **A refusal raised by the capture loop is the same journey**, and the
    /// device is let go of on the way.
    #[test]
    fn a_capture_loop_that_refuses_closes_the_camera_and_says_why() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(busy_pump), Ok(one_camera()));
        assert_eq!(state.stage, Stage::Webcam, "control: it did not even open");

        until("the refusal to arrive", || {
            advance_webcam(&mut state, std::time::Instant::now());
            state.stage == Stage::Picker
        });
        assert_eq!(state.refusal, Some(PickerRefusal::Camera(CameraRefusal::Busy)));
        assert!(state.webcam.is_none(), "the camera stage outlived its refusal");
    }

    /// **A frame arrives, is handed back for painting, and its size is
    /// recorded.**
    #[test]
    fn a_frame_reaches_the_surface_with_its_dimensions() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(one_frame_pump), Ok(one_camera()));

        let mut seen = None;
        until("a frame", || {
            seen = advance_webcam(&mut state, std::time::Instant::now());
            seen.is_some()
        });
        let frame = seen.expect("a frame");
        assert_eq!((frame.width(), frame.height()), (4, 3));
        assert_eq!(
            state.webcam.as_ref().and_then(|s| s.size),
            Some((4, 3)),
            "the preview does not know how big the picture is"
        );
        assert_eq!(state.stage, Stage::Webcam, "a frame ended the stage");
    }

    /// **A code read off the camera lands on 6c, with the field filled in and
    /// NOTHING saved.**
    ///
    /// The last clause is the one the owner asked for explicitly: 6c exists so
    /// the user sees what was extracted before anything is written, and a
    /// route that saved on a successful decode would skip it.
    #[test]
    fn a_code_read_off_the_camera_opens_the_confirmation_and_saves_nothing() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.revealed = true;
        open_webcam(&mut state, &seams(reading_pump), Ok(one_camera()));

        until("the code", || {
            advance_webcam(&mut state, std::time::Instant::now());
            state.stage == Stage::Manual
        });
        assert_eq!(&*state.typed, CAMERA_URI, "the decoded URI is not in the field");
        assert!(state.scanned, "the confirmation will not know this was scanned");
        assert!(!state.revealed, "a scanned seed opened revealed");
        assert!(state.refusal.is_none());
        // **The camera is closed by the same step**, so the light goes out as
        // the confirmation appears rather than when the modal does.
        assert!(state.webcam.is_none(), "the camera is still open behind the confirmation");
        // And the confirmation is reached with the seed unsaved: what a caller
        // would write is available, but nothing here has asked it to.
        assert!(uri_to_write(&state).is_some(), "6c has nothing to confirm");
    }

    /// **A real QR code in front of a camera reaches 6c through the REAL
    /// decoder.**
    ///
    /// Every other test in this block drives the wiring with a stub, which
    /// says the plumbing is connected and nothing about whether a camera
    /// frame is a shape `rqrr` can read. This one hands the capture loop
    /// pixels rendered from `qr::tests::FIXTURE` -- the matrix generated
    /// outside this repository by a crate that is not a dependency of this
    /// app -- and lets [`crate::webcam::WebcamSeams::production`]'s own
    /// decoder read them. The frame is synthetic; the decode is not.
    ///
    /// What it cannot say is that a real sensor produces a readable
    /// picture. Nothing in this crate can say that, and no assertion here
    /// pretends to.
    #[test]
    fn a_real_qr_code_in_front_of_the_camera_reaches_the_confirmation() {
        fn fixture_pump(_: Device, decode: crate::webcam::DecodeFn, sink: Sink) {
            let (rgba, width, height) = crate::qr::tests::fixture_rgba(4);
            if let Some(frame) =
                crate::webcam::Frame::new(width, height, Zeroizing::new(rgba))
            {
                // A cadence of zero so the very first frame is read: this
                // producer has exactly one to give.
                let mut cadence =
                    crate::webcam::DecodeCadence::new(std::time::Duration::ZERO);
                let now = std::time::Instant::now();
                if !crate::webcam::offer(&sink, &mut cadence, decode, frame, now) {
                    return;
                }
            }
            sink.ended();
        }

        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        // Production seams but for the capture loop, so the decoder below
        // is `qr::decode_qr` and not a stub.
        open_webcam(&mut state, &seams(fixture_pump), Ok(one_camera()));
        until("the fixture to decode", || {
            advance_webcam(&mut state, std::time::Instant::now());
            state.stage != Stage::Webcam
        });

        assert_eq!(state.stage, Stage::Manual, "a real code did not open 6c");
        assert_eq!(
            &*state.typed,
            crate::qr::tests::FIXTURE_TEXT,
            "the URI in the field is not the one the code carried"
        );
        assert!(state.webcam.is_none(), "the camera outlived the code it read");
        // And 6c has something to confirm: the parameters survive the trip,
        // which is what makes this an end-to-end assertion rather than a
        // string comparison.
        let Reading::Ok(auth) = read_field(&state.typed, state.digits, state.period) else {
            panic!("the decoded URI does not read as a one-time code");
        };
        assert_eq!(auth.issuer.as_deref(), Some("Git Host"));
        assert_eq!(auth.digits, 8);
        assert_eq!(auth.period, 60);
        // **And nothing was written.** 6c is a card the user has to press
        // Save on, and a scanned code reaches it exactly as a typed one
        // does.
        assert!(uri_to_write(&state).is_some(), "6c has nothing to confirm");
    }

    /// **A camera that opens and never sends a picture is given up on**, with
    /// the refusal that names what to check.
    #[test]
    fn a_camera_that_says_nothing_is_given_up_on_after_the_grace_period() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(one_camera()));
        let start = std::time::Instant::now();

        // Control first: it is NOT given up on immediately, or a camera that
        // takes two seconds to wake would never be usable.
        assert!(advance_webcam(&mut state, start).is_none());
        assert_eq!(state.stage, Stage::Webcam, "a camera was refused before it had a chance");

        assert!(advance_webcam(&mut state, start + crate::webcam::FIRST_FRAME_GRACE).is_none());
        assert_eq!(state.stage, Stage::Picker, "a silent camera was waited on forever");
        assert_eq!(state.refusal, Some(PickerRefusal::Camera(CameraRefusal::Silent)));
        assert!(state.webcam.is_none(), "a silent camera was left open");
    }

    /// **Picking a camera out of the list opens that one**, and switching
    /// closes the one before it.
    #[test]
    fn choosing_a_camera_opens_it_and_switching_closes_the_one_before() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(two_cameras()));

        choose_camera(&mut state, &seams(silent_pump), 1);
        let stage = state.webcam.as_ref().expect("a camera stage");
        assert_eq!(stage.chosen, Some(1));
        let first = stage.session.as_ref().expect("a capture loop");
        assert!(!first.stopped(), "control: the camera it opened is already stopped");

        choose_camera(&mut state, &seams(silent_pump), 0);
        let stage = state.webcam.as_ref().expect("a camera stage");
        assert_eq!(stage.chosen, Some(0));
        assert!(stage.session.is_some());

        // An index the list does not have changes nothing rather than
        // refusing: only this file can pass one.
        choose_camera(&mut state, &seams(silent_pump), 9);
        assert_eq!(state.webcam.as_ref().and_then(|s| s.chosen), Some(0));
    }

    /// **Every way of leaving this stage closes the camera.**
    ///
    /// Driven through the four gestures rather than by calling
    /// `state.webcam = None`, because what is under test is that each of them
    /// reaches that line.
    #[test]
    fn every_way_out_of_the_camera_stage_releases_the_device() {
        /// Whether the capture loop is still running, observed from the loop
        /// itself: `Session::stopped` says what was ASKED for, and this says
        /// what happened.
        fn watched_pump(_: Device, _: crate::webcam::DecodeFn, sink: Sink) {
            RUNNING.store(true, std::sync::atomic::Ordering::Release);
            while sink.wanted() {
                std::thread::yield_now();
            }
            RUNNING.store(false, std::sync::atomic::Ordering::Release);
        }
        static RUNNING: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        /// One test at a time may use the static above.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        let _held = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let running = || RUNNING.load(std::sync::atomic::Ordering::Acquire);

        for (what, leave) in [
            ("the way back to the picker", (|s: &mut TotpAdd| s.back_to_picker()) as fn(&mut TotpAdd)),
            ("a decode landing", |s: &mut TotpAdd| s.accept_decoded(Zeroizing::new(CAMERA_URI.to_string()))),
        ] {
            let mut state = TotpAdd::opening("id-1", "Git Host", false);
            open_webcam(&mut state, &seams(watched_pump), Ok(one_camera()));
            until("the capture loop to start", running);
            leave(&mut state);
            assert!(state.webcam.is_none(), "{what} left a camera stage behind");
            until(&format!("the device to be released after {what}"), || !running());
        }

        // **And the modal simply going away**, which is how it ends on Save,
        // on Cancel, on Escape, when the vault locks and when the window is
        // destroyed: all five drop the `TotpAdd` that owns the device.
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(watched_pump), Ok(one_camera()));
        until("the capture loop to start", running);
        drop(state);
        until("the device to be released when the form is dropped", || !running());
    }

    /// **The camera's refusals reach the user as sentences on the picker**,
    /// each one different from the others and from every other refusal this
    /// surface has.
    #[test]
    fn each_camera_refusal_renders_as_its_own_sentence_on_the_picker() {
        let camera: Vec<String> = [
            CameraRefusal::NoCamera,
            CameraRefusal::Busy,
            CameraRefusal::Denied,
            CameraRefusal::Silent,
            CameraRefusal::Lost,
            CameraRefusal::Unavailable,
        ]
        .iter()
        .map(|why| PickerRefusal::Camera(*why).sentence())
        .collect();
        let others = [
            PickerRefusal::NoCode(CodeSource::Region).sentence(),
            PickerRefusal::NoCode(CodeSource::Image).sentence(),
            PickerRefusal::Capture(CaptureRefusal::Blocked).sentence(),
            PickerRefusal::NotAnImage.sentence(),
            PickerRefusal::Unreadable.sentence(),
        ];
        let all: Vec<&String> = camera.iter().chain(others.iter()).collect();
        for (i, one) in all.iter().enumerate() {
            assert!(!one.trim().is_empty(), "refusal {i} renders as nothing");
            for (j, other) in all.iter().enumerate() {
                assert!(i == j || one != other, "refusals {i} and {j} render the same sentence");
            }
        }
        // Positively: the sentence is the camera module's own two clauses and
        // not a paraphrase invented here.
        assert_eq!(
            PickerRefusal::Camera(CameraRefusal::Busy).sentence(),
            format!("{}. {}", CameraRefusal::Busy.title(), CameraRefusal::Busy.detail())
        );
    }

    // -----------------------------------------------------------------
    // The camera stage, on screen
    // -----------------------------------------------------------------

}

#[cfg(test)]
mod item_has_code_tests {
    /// **A record whose `totp` key is present but EMPTY has no code.**
    ///
    /// The owner, on a login with no second factor: "message is misleading -
    /// there is no TOTP for this creds". The card was asked
    /// `login.totp.is_some()`, and Bitwarden leaves the key behind with an
    /// empty value when a code is removed -- so the card offered to replace
    /// a secret that was not there, under a warning that the old one could
    /// not be recovered.
    ///
    /// All four shapes, because the interesting two are the ones an
    /// `is_some()` gets wrong and a naive `is_empty()` still gets wrong.
    #[test]
    fn only_a_record_with_a_seed_in_it_already_has_a_code() {
        let item = |totp: &str| -> crate::vault_bridge::VaultItem {
            serde_json::from_str(&format!(
                r#"{{"id":"i","type":1,"name":"n","fields":[],"login":{{{totp}}}}}"#
            ))
            .expect("the fixture is valid item JSON")
        };
        assert!(
            !super::item_has_code(&item("")),
            "a login with no `totp` key at all was said to have a code"
        );
        assert!(
            !super::item_has_code(&item(r#""totp":"""#)),
            "a login whose `totp` is the empty string was said to have a code -- this is \
             the shape Bitwarden leaves behind when a code is removed"
        );
        assert!(
            !super::item_has_code(&item(r#""totp":"   ""#)),
            "a login whose `totp` is whitespace was said to have a code; neither base32 \
             nor an otpauth:// link is made of spaces"
        );
        assert!(
            super::item_has_code(&item(r#""totp":"JBSWY3DPEHPK3PXP""#)),
            "the control failed: a login that really carries a seed was said to have none, \
             so the three refusals above are not about the seed"
        );
    }
}
#[cfg(test)]
mod v2_add_2fa_tests {
    //! Spec B §8.
    use super::ui::{self, back_id, code_id, reveal_id, save_id, table_row_id, back_label, code_now, found_title, ID, match_prompt, scan_again_label};
    use super::*;
    use crate::kit::modal;
    use crate::kit::testing::{self, click_at};

    #[test]
    fn the_picker_offers_scan_and_type() {
        let ctx = testing::ctx();
        let mut state = TotpAdd::opening("1", "Git Host", false);
        let mut rows = Vec::new();
        for t in [0.0, 0.1] {
            let p = testing::frame(&ctx, t, egui::vec2(700.0, 900.0), vec![], |ui| {
                rows = draw_picker(ui, &mut state).rows;
            });
            assert!(p.text("Scan from screen").is_some() && p.text("Type the key").is_some());
        }
        let typed = rows.iter().find(|(route, _)| *route == Route::ByHand).expect("the Type the key card").1;
        let _ = testing::frame(&ctx, 0.2, egui::vec2(700.0, 900.0), click_at(typed.center()), |ui| {
            let _ = draw_picker(ui, &mut state);
        });
        assert_eq!(state.stage, Stage::Manual, "Type the key opens the by-hand form");
    }

    #[test]
    fn a_valid_key_lights_the_field_and_previews_the_code() {
        assert!(can_save(&read_field("JBSW Y3DP EHPK 3PXP", 6, 30)), "a base32 key is saveable");
        assert!(!can_save(&read_field("not a key!", 6, 30)), "anything else is not");
    }

    #[test]
    fn saving_toasts_2fa_code_added() {
        assert_eq!(saved_toast(), "2FA code added");
        let source = include_str!("mod.rs");
        let at = source.find(concat!("totp_add::", "saved_toast()")).expect("the window toasts on a saved code");
        let before = &source[..at];
        assert!(before.rfind(concat!("Ok(written)", " => {")).is_some_and(|ok| at - ok < 200), "the toast is not in the save's Ok arm");
    }
    const SCREEN: egui::Vec2 = egui::vec2(700.0, 1000.0);

    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]
    }

    /// Draws the picker for two idle frames and answers where its rows are.
    fn settled_picker(ctx: &egui::Context, state: &mut TotpAdd) -> Vec<(Route, egui::Rect)> {
        let mut rows = Vec::new();
        for t in [0.0, 0.1] {
            let _ = testing::frame(ctx, t, SCREEN, vec![], |ui| rows = draw_picker(ui, state).rows);
        }
        rows
    }

    fn row_of(rows: &[(Route, egui::Rect)], route: Route) -> egui::Rect {
        rows.iter().find(|(r, _)| *r == route).unwrap_or_else(|| panic!("no row for {route:?}")).1
    }

    #[test]
    fn each_route_asks_for_its_own_action_and_no_other() {
        for (route, want) in [
            (Route::ScanRegion, TotpAddAction::ScanRegion),
            (Route::ImageFile, TotpAddAction::OpenImage),
            (Route::Webcam, TotpAddAction::OpenWebcam),
        ] {
            let ctx = testing::ctx();
            let mut state = TotpAdd::opening("1", "Git Host", false);
            let rows = settled_picker(&ctx, &mut state);
            let mut got = TotpAddAction::None;
            let _ = testing::frame(&ctx, 0.2, SCREEN, click_at(row_of(&rows, route).center()), |ui| {
                got = draw_picker(ui, &mut state).action;
            });
            assert_eq!(got, want, "{route:?}");
        }
    }

    #[test]
    fn the_routes_keys_still_answer_and_only_on_the_picker() {
        for (pressed, want) in [
            (egui::Key::Enter, TotpAddAction::ScanRegion),
            (egui::Key::Num1, TotpAddAction::ScanRegion),
            (egui::Key::Num2, TotpAddAction::OpenImage),
            (egui::Key::Num4, TotpAddAction::OpenWebcam),
        ] {
            let ctx = testing::ctx();
            let mut state = TotpAdd::opening("1", "Git Host", false);
            let _ = settled_picker(&ctx, &mut state);
            let mut got = TotpAddAction::None;
            let _ = testing::frame(&ctx, 0.2, SCREEN, key(pressed), |ui| got = draw_picker(ui, &mut state).action);
            assert_eq!(got, want, "{pressed:?}");
        }
        let ctx = testing::ctx();
        let mut state = TotpAdd::opening("1", "Git Host", false);
        let _ = settled_picker(&ctx, &mut state);
        let _ = testing::frame(&ctx, 0.2, SCREEN, key(egui::Key::Num3), |ui| {
            let _ = draw_picker(ui, &mut state);
        });
        assert_eq!(state.stage, Stage::Manual, "3 is Type the key");
        // On the by-hand form a bare digit is a character in the field, and
        // nothing in this file reads it as a route.
        let mut got = TotpAddAction::None;
        let _ = testing::frame(&ctx, 0.3, SCREEN, key(egui::Key::Num2), |ui| got = draw_add_form(ui, &mut state, 0));
        assert_eq!(got, TotpAddAction::None);
    }

    #[test]
    fn the_last_refusal_is_a_danger_note_under_the_rows() {
        let ctx = testing::ctx();
        let mut state = TotpAdd::opening("1", "Git Host", false);
        state.refusal = Some(PickerRefusal::NoCode(CodeSource::Region));
        let mut p = None;
        for t in [0.0, 0.1] {
            p = Some(testing::frame(&ctx, t, SCREEN, vec![], |ui| {
                let _ = draw_picker(ui, &mut state);
            }));
        }
        let p = p.unwrap();
        let sentence = state.refusal.unwrap().sentence();
        assert!(p.text(&sentence[..20]).is_some(), "the refusal is not painted");
        assert!(p.text("Decoding happens on this machine").is_some(), "the privacy line stays under it");
    }

    fn typed_state(text: &str) -> TotpAdd {
        let mut state = TotpAdd::opening("1", "Git Host", false);
        state.stage = Stage::Manual;
        state.typed = Zeroizing::new(text.to_string());
        state
    }

    fn form_frames(ctx: &egui::Context, state: &mut TotpAdd, events: Vec<egui::Event>) -> (testing::Painted, TotpAddAction) {
        let mut action = TotpAddAction::None;
        let mut p = None;
        let base = ctx.input(|i| i.time);
        for (i, step) in [0.1, 0.2, 0.3].into_iter().enumerate() {
            let events = if i == 2 { events.clone() } else { vec![] };
            p = Some(testing::frame(ctx, base + step, SCREEN, events, |ui| action = draw_add_form(ui, state, 1_700_000_000)));
        }
        (p.unwrap(), action)
    }

    #[test]
    fn a_valid_key_paints_the_live_code_row_and_an_empty_one_does_not() {
        let ctx = testing::ctx();
        let mut state = typed_state("JBSW Y3DP EHPK 3PXP");
        let (p, _) = form_frames(&ctx, &mut state, vec![]);
        assert!(p.text(code_now()).is_some(), "no \"Your code right now\" row");
        assert!(p.text("Digits").is_some() && p.text("Period").is_some(), "the two parameters stay");
        assert!(p.text("Save code").is_some() && p.text(back_label()).is_some());
        let ctx = testing::ctx();
        let mut state = typed_state("");
        let (p, _) = form_frames(&ctx, &mut state, vec![]);
        assert!(p.text(code_now()).is_none());
    }

    #[test]
    fn save_answers_only_once_the_field_reads_as_a_code() {
        let ctx = testing::ctx();
        let mut empty = typed_state("");
        let _ = form_frames(&ctx, &mut empty, vec![]);
        let at = ctx.read_response(save_id()).expect("the Save answer").rect.center();
        let (_, action) = form_frames(&ctx, &mut empty, click_at(at));
        assert_eq!(action, TotpAddAction::None, "Save answered on an empty field");

        let ctx = testing::ctx();
        let mut valid = typed_state("JBSW Y3DP EHPK 3PXP");
        let _ = form_frames(&ctx, &mut valid, vec![]);
        let at = ctx.read_response(save_id()).expect("the Save answer").rect.center();
        let (_, action) = form_frames(&ctx, &mut valid, click_at(at));
        assert_eq!(action, TotpAddAction::Save);
    }

    #[test]
    fn back_returns_to_the_picker_and_takes_the_seed_with_it() {
        let ctx = testing::ctx();
        let mut state = typed_state("JBSW Y3DP EHPK 3PXP");
        let _ = form_frames(&ctx, &mut state, vec![]);
        let at = ctx.read_response(back_id()).expect("the Back answer").rect.center();
        let _ = form_frames(&ctx, &mut state, click_at(at));
        assert_eq!(state.stage, Stage::Picker);
        assert!(state.typed.is_empty(), "the seed outlived the form");
    }

    fn scanned_state() -> TotpAdd {
        let mut state = TotpAdd::opening("1", "Git Host", false);
        state.accept_decoded(Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host".to_string(),
        ));
        state
    }

    #[test]
    fn a_scanned_code_is_the_found_card_with_its_table() {
        let ctx = testing::ctx();
        let mut state = scanned_state();
        let (p, _) = form_frames(&ctx, &mut state, vec![]);
        assert!(p.text(found_title()).is_some(), "no Found it");
        assert!(p.text("Git Host \u{b7} anovak").is_some(), "the issuer and the account");
        assert!(p.text("new code in").is_some(), "the countdown");
        assert!(p.text(match_prompt()).is_some());
        for row in [issuer_row_label(), account_row_label(), secret_row_label(), parameters_row_label()] {
            assert!(p.text(row).is_some(), "the table lost its {row} row");
        }
        assert!(p.text(scan_again_label()).is_some() && p.text("Save code").is_some());
        assert!(p.text("JBSWY3DPEHPK3PXP").is_none(), "the secret is masked until Reveal");
    }

    #[test]
    fn pressing_the_live_code_asks_for_a_copy_and_reveal_does_not() {
        let ctx = testing::ctx();
        let mut state = scanned_state();
        let _ = form_frames(&ctx, &mut state, vec![]);
        let code = ctx.read_response(code_id()).expect("the found card").rect.center();
        let (_, action) = form_frames(&ctx, &mut state, click_at(code));
        assert_eq!(action, TotpAddAction::CopyCode);

        let reveal = ctx.read_response(reveal_id()).expect("the Reveal control").rect.center();
        let (_, action) = form_frames(&ctx, &mut state, click_at(reveal));
        assert!(state.revealed, "Reveal did not reveal");
        assert_eq!(action, TotpAddAction::None, "Reveal also asked for a copy");
    }

    #[test]
    fn a_row_with_a_value_copies_it() {
        let ctx = testing::ctx();
        let mut state = scanned_state();
        let _ = form_frames(&ctx, &mut state, vec![]);
        let issuer = ctx.read_response(table_row_id(issuer_row_label())).expect("the Issuer row").rect;
        let (_, action) = form_frames(&ctx, &mut state, click_at(issuer.center()));
        assert_eq!(action, TotpAddAction::CopyField(TableField::Issuer));
    }

    #[test]
    fn the_replace_warning_shows_only_when_there_is_a_code_to_replace() {
        let ctx = testing::ctx();
        let mut state = typed_state("JBSW Y3DP EHPK 3PXP");
        state.already_has_code = true;
        let (p, _) = form_frames(&ctx, &mut state, vec![]);
        assert!(p.text(&replace_warning()[..30]).is_some(), "no replace warning");
        assert!(p.text(replace_label()).is_some(), "the primary does not say Replace");
        let ctx = testing::ctx();
        let mut state = typed_state("JBSW Y3DP EHPK 3PXP");
        let (p, _) = form_frames(&ctx, &mut state, vec![]);
        assert!(p.text(&replace_warning()[..30]).is_none());
    }

    #[test]
    fn the_modal_is_the_v2_dialog_at_every_stage() {
        for stage in [Stage::Picker, Stage::Manual, Stage::Scanning] {
            let ctx = testing::ctx();
            let mut state = TotpAdd::opening("1", "Git Host", false);
            state.stage = stage;
            let mut p = None;
            for t in [0.0, 0.1, 0.2] {
                p = Some(testing::frame(&ctx, t, egui::vec2(1280.0, 820.0), vec![], |ui| {
                    let _ = draw_add_modal(ui.ctx(), &mut state, 1_700_000_000);
                }));
            }
            let p = p.unwrap();
            assert!(p.text("Add 2FA code").is_some() && p.text("For Git Host").is_some(), "{stage:?}: no v2 title block");
            assert!(ctx.read_response(modal::close_id(ID)).is_some(), "{stage:?}: no close mark");
        }
    }

    #[test]
    fn the_scan_panel_says_what_it_is_doing() {
        let ctx = testing::ctx();
        let mut state = TotpAdd::opening("1", "Git Host", false);
        state.stage = Stage::Scanning;
        let mut p = None;
        for t in [0.0, 0.1, 0.2] {
            p = Some(testing::frame(&ctx, t, egui::vec2(1280.0, 820.0), vec![], |ui| {
                let _ = draw_add_modal(ui.ctx(), &mut state, 0);
            }));
        }
        let p = p.unwrap();
        assert!(p.text(ui::looking_caption()).is_some());
        assert!(p.text(scanning_note()).is_some(), "0.15's waiting note is gone");
    }

    #[test]
    fn the_decorative_code_has_its_three_finders_and_no_fourth() {
        let modules = ui::scan_modules();
        // The corners of the three 7x7 finders are dark; the fourth corner is
        // pseudo-random like the rest, and the gap inside each ring is light.
        for corner in [(0, 0), (6, 0), (0, 6), (6, 6), (18, 0), (24, 0), (18, 6), (24, 6), (0, 18), (6, 18), (0, 24), (6, 24)] {
            assert!(modules.contains(&corner), "finder corner {corner:?} is light");
        }
        assert!(!modules.contains(&(1, 1)) && !modules.contains(&(19, 1)) && !modules.contains(&(1, 19)), "a finder's gap is dark");
        assert!(modules.contains(&(3, 3)) && modules.contains(&(21, 3)) && modules.contains(&(3, 21)), "a finder's eye is light");
    }
    #[test]
    fn the_modal_opens_on_the_picker_and_wraps_the_form_with_its_warning_and_code() {
        // The picker first, with its privacy sentence and not the key field.
        let ctx = testing::ctx();
        let mut state = TotpAdd::opening("1", "Git Host", false);
        assert_eq!(state.stage, Stage::Picker, "a fresh form does not open on the picker");
        let mut p = None;
        for t in [0.0, 0.1, 0.2] {
            p = Some(testing::frame(&ctx, t, egui::vec2(1280.0, 820.0), vec![], |ui| {
                let _ = draw_add_modal(ui.ctx(), &mut state, 1_700_000_000);
            }));
        }
        let p = p.unwrap();
        assert!(p.text("Scan from screen").is_some() && p.text("Type the key").is_some());
        assert!(p.text("Decoding happens on this machine").is_some(), "the modal dropped the privacy line");
        assert!(p.text("JBSW Y3DP EHPK 3PXP").is_none(), "the modal opened straight into the by-hand form");

        // Then the form: the warning over the buttons and the live code in it.
        let ctx = testing::ctx();
        let mut state = typed_state("JBSWY3DPEHPK3PXP");
        state.already_has_code = true;
        let mut p = None;
        for t in [0.0, 0.1, 0.2] {
            p = Some(testing::frame(&ctx, t, egui::vec2(1280.0, 820.0), vec![], |ui| {
                let _ = draw_add_modal(ui.ctx(), &mut state, 1_700_000_000);
            }));
        }
        let p = p.unwrap();
        assert!(p.text("already has a one-time code").is_some(), "the modal dropped the warning");
        let auth = parse_otpauth("otpauth://totp/Git%20Host?secret=JBSWY3DPEHPK3PXP").expect("the fixture parses");
        let expected = grouped_code(&code_at(&auth, 1_700_000_000).expect("the fixture's seed decodes"));
        assert!(p.text(&expected).is_some(), "the modal dropped the live code {}", expected.as_str());
    }
}

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

// The 26px button height this form used to declare is gone with the two bare
// `egui::Button`s it sized. 6d's footer answers are `theme`'s own -- see
// `manual_footer` -- and `theme::BUTTON_HEIGHT` is 6d's declared 32.

/// The card's width, matching the record composer's narrow column.
///
/// [`Stage::Scanning`]'s alone now. See [`stage_width`]: the by-hand form
/// took this until the 6d design pass measured `#6d` at 470.
const MODAL_WIDTH: f32 = 380.0;

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
pub const HEADING: &str = "Enter the secret";

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
pub const ADD_TOTP_LABEL: &str = "Add a TOTP";
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
pub const SECRET_HINT: &str = "Secret key or otpauth:// URI";

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
pub const CODE_READ_LABEL: &str = "Code read";
/// See [`CODE_READ_LABEL`]. What kind of thing it was.
pub const CODE_READ_KIND: &str = "otpauth://totp";

/// The heading over the confirmation half, verbatim from design 6c.
pub const CONFIRM_HEADING: &str = "What was extracted";

/// The question printed beside the live code, verbatim from design 6c. It is
/// the reason the code and the countdown are on screen at all.
pub const MATCH_QUESTION: &str = "Matches what the site shows?";

/// **The only destructive act in this feature, verbatim.**
///
/// Shown whenever the item already has a `totp`, and pinned by content in
/// [`tests::the_replace_warning_is_the_designs_own_sentence`] the way this
/// crate pins its refusal messages. A seed cannot be recovered once it has
/// been overwritten -- "rotating" it means re-enrolling the second factor with
/// the service, which this app can neither do nor offer -- so this sentence is
/// the only place the user is told what pressing the button costs. A reworded
/// one must be a deliberate edit that reds a test, not a tidy-up.
pub const REPLACE_WARNING: &str = "This record already has a one-time code. Saving replaces it \
     \u{2014} the old secret cannot be recovered.";

/// The submit button when the item has no code yet, verbatim from design 6d.
pub const SAVE_LABEL: &str = "Save code";

/// The submit button when there is one to overwrite, verbatim from design 6c.
///
/// A **different word** from [`SAVE_LABEL`] deliberately: the button that
/// destroys something says so on its face, not only in the paragraph above it.
pub const REPLACE_LABEL: &str = "Replace code";

/// **What the footer says at its right-hand end**, verbatim from design 6c's
/// footer row, and it is a promise about a key that really answers:
/// [`draw_add_modal`] returns [`TotpAddAction::Cancel`] on Escape at every
/// stage but [`Stage::Scanning`]. See `manual_footer` for why this line is
/// drawn *instead of* 6d's second button rather than beside it.
pub const DISMISS_HINT: &str = "Esc discards";

/// The control that unmasks the secret.
pub const REVEAL_LABEL: &str = "Reveal";
/// See [`REVEAL_LABEL`].
pub const HIDE_LABEL: &str = "Hide";

/// The label over the masked secret row.
pub const SECRET_ROW_LABEL: &str = "Secret";
/// See [`SECRET_ROW_LABEL`].
pub const ISSUER_ROW_LABEL: &str = "Issuer";
/// See [`SECRET_ROW_LABEL`].
pub const ACCOUNT_ROW_LABEL: &str = "Account";
/// See [`SECRET_ROW_LABEL`]. **The parameters are spelled out and not
/// summarised**: a card that is 8 digits over 60 seconds under SHA-256 is
/// exactly the case a confirmation screen exists to catch, and it is
/// invisible if the row says only "TOTP".
pub const PARAMETERS_ROW_LABEL: &str = "Parameters";

/// The label over the live code.
pub const CODE_ROW_LABEL: &str = "Code now";

/// Why the two controls are dead: the pasted URI already said.
pub const PARAMETERS_FROM_URI: &str = "from the URI";

/// The captions on the two parameter controls.
pub const DIGITS_LABEL: &str = "Digits";
/// See [`DIGITS_LABEL`].
pub const PERIOD_LABEL: &str = "Period";

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
pub const PICKER_TITLE: &str = "Add a one-time code";

/// The label over the item name at the top of the picker, verbatim from 6a.
pub const ADDING_TO_LABEL: &str = "Adding to";

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
pub const PRIVACY_LINE: &str = "Decoding happens on this machine. The captured pixels are \
     discarded once the secret is read, and the secret is never written to disk outside the \
     vault.";

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
pub const DEFERRED_REASON: &str = "Not in this version";

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
pub const ROUTES: [RouteRow; 4] = [
    RouteRow {
        route: Route::ScanRegion,
        title: "Scan the code on my screen",
        subtitle: "Deskwarden looks for it in every window \u{b7} drag a box if it misses",
        enabled: true,
    },
    RouteRow {
        route: Route::ImageFile,
        title: "Open an image file",
        subtitle: "A screenshot or photo \u{b7} PNG, JPG, GIF, BMP, WebP, ICO",
        enabled: true,
    },
    RouteRow {
        route: Route::ByHand,
        title: "Enter the secret by hand",
        subtitle: "The base32 key the site shows under the code",
        enabled: true,
    },
    RouteRow {
        route: Route::Webcam,
        title: "Use a webcam",
        // 6a's own line for this row is about a camera being pointed at
        // something, and what a user needs to know before pressing it is what
        // they will have to hold up. The second clause is the promise
        // [`crate::webcam`] keeps and `PRIVACY.md` repeats: the camera is not
        // on until this row is pressed, and nothing it sees is kept.
        subtitle: "Hold the code up to the camera \u{b7} the picture stays on this PC",
        enabled: true,
    },
];

/// What the picker's way back to itself is called, from the two halves that
/// have one.
pub const OTHER_WAYS_LABEL: &str = "Other ways to add it";

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
pub const CANCEL_LABEL: &str = "Cancel";


/// The heading while the 6b overlay is up.
pub const SCANNING_HEADING: &str = "Scanning your screen";

/// The heading over the camera preview.
///
/// An instruction rather than a status, for the reason 6b's own bar is an
/// instruction: the user is holding a phone in one hand and has one thing to
/// do with it.
pub const WEBCAM_HEADING: &str = "Point the camera at the code";

/// The line under [`WEBCAM_HEADING`].
///
/// Deliberately the same two clauses as `region_overlay::DRAG_HINT` -- it
/// reads by itself, and nothing is saved yet -- because they are the same two
/// facts and a user who has tried both routes should not have to learn them
/// twice.
pub const WEBCAM_HINT: &str = "Deskwarden reads it as soon as it can. Nothing is saved yet.";

/// What the preview says before the first frame arrives.
///
/// A camera can take two or three seconds to wake, and a black rectangle for
/// that long is indistinguishable from a broken route. See
/// `crate::webcam::FIRST_FRAME_GRACE` for what happens if it never wakes.
pub const WEBCAM_STARTING: &str = "Starting the camera\u{2026}";

/// The heading over the device list, when there is more than one camera.
pub const WEBCAM_CHOOSE: &str = "Which camera?";

/// The line under [`WEBCAM_CHOOSE`], which is the promise the list keeps: no
/// device is opened, so no camera light comes on, until one is pressed.
pub const WEBCAM_CHOOSE_HINT: &str = "None of them is switched on until you pick one.";

/// The way back to the device list from an open camera, when there is more
/// than one to go back to.
pub const WEBCAM_ANOTHER_LABEL: &str = "Use a different camera";

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
pub const WEBCAM_PRIVACY_LINE: &str = "The camera is on only while this is open. The picture is \
     read on this PC and thrown away \u{2014} nothing is recorded and nothing is sent anywhere.";

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
const NO_CODE_ADVICE: &str = "Include the code's white margin";

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
            PickerRefusal::NoCode(CodeSource::Region) => format!(
                "No code in that region. {NO_CODE_ADVICE}, or zoom the page to 150% and drag \
                 again."
            ),
            PickerRefusal::NoCode(CodeSource::Image) => format!(
                "No code in that image. {NO_CODE_ADVICE}, or open a larger copy of the picture."
            ),
            PickerRefusal::Capture(why) => format!("{}. {}", why.title(), why.detail()),
            // The same two-clause shape as the capture's, because it is the
            // same kind of thing: a device that would not give this app what
            // it asked for, said in the device's own words rather than
            // paraphrased into a second vocabulary here.
            PickerRefusal::Camera(why) => format!("{}. {}", why.title(), why.detail()),
            PickerRefusal::NotAnImage => {
                "That file isn't a picture Deskwarden can read. It reads PNG, JPEG, GIF, BMP, \
                 WebP and ICO \u{2014} choose a screenshot or a photo saved as one of those."
                    .to_string()
            }
            PickerRefusal::Unreadable => {
                "That file could not be opened. Check it is still where it was, and that you \
                 have permission to read it."
                    .to_string()
            }
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
            for px in source.chunks_exact(3) {
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
            for px in source.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        // The wide arms are reachable through PNG, which is the only format
        // in this app's list that carries sixteen bits a channel. Narrowing
        // to the top byte is not a loss worth defending against: `decode_qr`
        // weighs the three channels into one eight-bit luma before it looks
        // at a single module.
        image::ColorType::L16 => {
            for px in source.chunks_exact(2) {
                let grey = high_byte(px);
                rgba.extend_from_slice(&[grey, grey, grey, 0xff]);
            }
        }
        image::ColorType::La16 => {
            for px in source.chunks_exact(4) {
                let grey = high_byte(&px[..2]);
                rgba.extend_from_slice(&[grey, grey, grey, high_byte(&px[2..])]);
            }
        }
        image::ColorType::Rgb16 => {
            for px in source.chunks_exact(6) {
                rgba.extend_from_slice(&[
                    high_byte(&px[..2]),
                    high_byte(&px[2..4]),
                    high_byte(&px[4..]),
                    0xff,
                ]);
            }
        }
        image::ColorType::Rgba16 => {
            for px in source.chunks_exact(8) {
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
        Reading::Ok(auth) => Some(format!(
            "Valid base32 \u{b7} {} characters \u{b7} spaces ignored",
            auth.secret.len()
        )),
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
        OtpRefusal::NotOtpAuth => {
            "That isn't a one-time code. It reads as a plain URL \u{2014} Deskwarden accepts a \
             base32 secret or an otpauth:// URI."
                .to_string()
        }
        OtpRefusal::NotTotp => {
            "That is a counter-based (hotp) code. Deskwarden can only add time-based codes, \
             which is what sites mean by an authenticator app."
                .to_string()
        }
        OtpRefusal::NoSecret => "There is no secret in that \u{2014} nothing to save.".to_string(),
        OtpRefusal::BadSecret => {
            "That is not a valid base32 secret. Base32 uses the letters A\u{2013}Z and the \
             digits 2\u{2013}7; spaces and hyphens are ignored."
                .to_string()
        }
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
        OtpRefusal::PartialSecret(characters) => format!(
            "{characters} character{} cannot decode \u{2014} one is missing, or one has been \
             copied twice.",
            if *characters == 1 { "" } else { "s" }
        ),
        // Shortened for the reason above it: this was three wrapped lines as
        // well, and the clause that made it so -- "because a code saved from
        // it would be wrong and nothing on screen would say why" -- is an
        // argument for the refusal rather than anything the reader can act on.
        // What they can act on is the key, so the key is what is left, and
        // "refused, not ignored" is the one-clause version of the argument.
        OtpRefusal::UnknownParameter(key) => format!(
            "That URI carries {key}, which Deskwarden does not know \u{2014} refused, not ignored."
        ),
        OtpRefusal::BadParameter(name) => format!(
            "That URI's {name} is not a value Deskwarden can use, so the code it saved would \
             never match."
        ),
        OtpRefusal::TooLong => {
            "That is far too long to be a one-time code.".to_string()
        }
    }
}

/// The secret as design 6c draws it: groups of four bullets, `•••• •••• ••••`.
///
/// Length-shaped rather than a fixed run, so the row says how much seed there
/// is without saying what it is -- and so an eight-character seed and a
/// thirty-two-character one do not look identical.
pub fn masked(secret: &str) -> String {
    let mut out = String::with_capacity(secret.len() + secret.len() / 4 + 1);
    for (i, _) in secret.chars().enumerate() {
        if i > 0 && i % 4 == 0 {
            out.push(' ');
        }
        out.push('\u{2022}');
    }
    out
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
        format!("{} digits", auth.digits),
        format!("{} s", auth.period),
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
    format!("refreshes in {seconds_left} s")
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

/// Whether the submit button may be pressed at all.
pub fn can_save(reading: &Reading) -> bool {
    matches!(reading, Reading::Ok(_))
}

/// The submit button's face: [`REPLACE_LABEL`] when there is a code to
/// destroy, [`SAVE_LABEL`] when there is not.
pub fn submit_label(already_has_code: bool) -> &'static str {
    if already_has_code {
        REPLACE_LABEL
    } else {
        SAVE_LABEL
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

    let Some(stage) = state.webcam.as_mut() else {
        return None;
    };
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

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add)
        .inner
}

fn note(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    ui.label(egui::RichText::new(text).size(11.0).color(colour));
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

/// `#6d` declares `width: 470px` and the card is its only child.
///
/// The same number as [`PICKER_WIDTH`] and deliberately a separate constant,
/// for `theme::HEADER_BUTTON_HEIGHT`'s reason: two things the same size today
/// for different reasons are two constants, and folding them together is how
/// one silently follows the other when the design moves. What this replaces
/// is [`MODAL_WIDTH`]'s 380, which is still [`Stage::Scanning`]'s -- that
/// stage is four short lines and has no design panel asking it to be wider.
const MANUAL_WIDTH: f32 = 470.0;

/// The header band: `padding: 14px 16px` over a `1px solid #eae7e7` rule,
/// carrying a 14px `font-weight: 700` title. 468 x 44 on screen.
const MANUAL_HEADER_PAD_X: f32 = 16.0;
/// See [`MANUAL_HEADER_PAD_X`].
const MANUAL_HEADER_PAD_Y: f32 = 14.0;
/// See [`MANUAL_HEADER_PAD_X`].
const MANUAL_HEADER_TITLE_PX: f32 = 14.0;

/// What 6c hangs in the same band once a decoder has spoken: a 16px check
/// stroked at 2.6, `gap: 10px` from the title, and the kind of payload that
/// was read set at the far right in `font-size: 12px; color: #9b9797`.
const MANUAL_HEADER_GAP: f32 = 10.0;
/// See [`MANUAL_HEADER_GAP`].
const MANUAL_HEADER_MARK: f32 = 16.0;
/// See [`MANUAL_HEADER_GAP`].
const MANUAL_HEADER_MARK_STROKE: f32 = 2.6;
/// See [`MANUAL_HEADER_GAP`].
const MANUAL_HEADER_KIND_PX: f32 = 12.0;

/// The body: `padding: 16px` with `gap: 14px` down its column, which is what
/// leaves 6d's own 436-wide field inside a 470 card.
const MANUAL_BODY_PAD: i8 = 16;
/// See [`MANUAL_BODY_PAD`].
const MANUAL_BODY_GAP: f32 = 14.0;

/// How far short of the card's own border the body's scroll bar stops.
///
/// **The report was "2 lines on the right side", and the second line was the
/// scroll bar.** [`draw_add_form`]'s body scrolls -- see [`MODAL_BREATHING`]
/// for why it must -- and egui's default floating bar is pinned flush to the
/// right edge of the area it scrolls, which here is the inside of
/// [`stage_card`]'s 1pt stroke: measured at a 420pt window the bar painted
/// across x = 468.5..469.0 with the border at 469.5. A 6pt rule touching a 1pt
/// rule is two edges on a card the design draws with one.
///
/// **Why an inset rather than `theme::scrollbar_in_gutter`.** That helper is
/// this app's answer to the same defect on the item list, the edit form and
/// the read pane, and it deliberately puts the bar FLUSH to the outer edge --
/// "where the platform's own scroll bars sit" -- because on those three
/// surfaces the outer edge is a PANE boundary with no ink on it, so flush
/// costs the reader nothing and centring would spend the lane on a gap
/// against nothing. This card's outer edge is a drawn border, so the same
/// placement produces precisely the defect being fixed. What carries over is
/// the helper's other three numbers -- [`theme::SCROLLBAR_WIDTH`] for the
/// bar, the same width again for `floating_width` so it does not GROW
/// leftward over the body when hovered -- and they are taken from `theme`
/// rather than respelled here.
///
/// **And no lane is reserved.** `scrollbar_in_gutter`'s containers have zero
/// right padding and hand the bar that space; this body already has
/// [`MANUAL_BODY_PAD`] of its own on every side, and the bar is 6 of it. The
/// bar therefore floats inside padding that was already empty: nothing is
/// reserved, so the body's content width CANNOT change as the bar comes and
/// goes, which is the width jump `AlwaysVisible` exists to prevent on the
/// panes that do reserve. The visibility mode stays egui's default for the
/// same reason -- a bar is painted only when there is something to scroll,
/// and a card that fits shows no bar at all rather than a hidden-but-reserved
/// one.
///
/// The value centres the bar in the padding it floats in, so the clear space
/// either side of it is equal and neither the border nor the body's own right
/// edge is the thing it crowds.
const MANUAL_SCROLLBAR_INSET: f32 = (MANUAL_BODY_PAD as f32 - theme::SCROLLBAR_WIDTH) / 2.0;

/// The field's block: `gap: 7px` between the caption, the box and the line
/// under it, with the caption at `font-size: 12px; color: #605d5d`.
///
/// **A caption and not a placeholder.** 6d draws [`SECRET_HINT`] as a `div`
/// ABOVE the box and puts the typed seed inside it; this form used to hand
/// the same string to `TextEdit::hint_text`, where it is painted only while
/// the box is empty. The two are not interchangeable: the sentence says what
/// the box will accept -- *either* a bare key *or* a whole URI, which is the
/// one thing about this field a user cannot guess -- and a placeholder
/// deletes that as soon as the first character is typed, which is exactly
/// when a half-pasted URI needs it.
const SECRET_BLOCK_GAP: f32 = 7.0;
/// See [`SECRET_BLOCK_GAP`].
const SECRET_LABEL_PX: f32 = 12.0;

/// The box itself: `border-radius: 8px; padding: 9px 11px` inside a `1px`
/// border, around a monospace seed set at `letter-spacing: 0.1em`. §6d's own
/// `font-size: 13px; line-height: 1.6` -- a 40.8-tall box -- is NOT taken;
/// see [`SECRET_TEXT_PX`] for the report that changed both numbers. What is
/// taken is that 6d draws this box focused, in
/// `border: 1px solid #1b3fa0` under `box-shadow: 0 0 0 3px #dbe4f7`
/// ([`theme::BLUE`] and [`theme::FOCUS_RING`], which is the halo every other
/// field in this app already wears).
const SECRET_BOX_RADIUS: u8 = 8;
/// See [`SECRET_BOX_RADIUS`].
const SECRET_BOX_STROKE: f32 = 1.0;
/// See [`SECRET_BOX_RADIUS`].
const SECRET_BOX_PAD_X: f32 = 11.0;
/// The seed's type size -- **this app's own field size, not §6d's 13.**
///
/// The owner, on the 13: "text cursor is huge and text is not that big, text
/// in field not centered and field looks higher". Three of those four are one
/// measurement. The caret egui draws is the ROW's height, which for this face
/// is 15.2 whether the glyphs in it are 13 points or 14; at 13 the caret
/// stands a clear step taller than the letters beside it, and in a box whose
/// interior is §6d's `line-height: 1.6` the pair sits in a field noticeably
/// deeper than every other field on the same card.
///
/// So the seed is set at the size every box in this app sets its value at
/// (`theme::field_box`'s own 14) in a box of [`theme::FIELD_HEIGHT`]. What is
/// KEPT from §6d is everything that makes this field a seed field and not a
/// name field: the monospace face, the `0.1em` tracking, the blue focused
/// border and its halo. The design's proportions were drawn for a browser's
/// caret, which is the glyph height; egui's is the line's.
const SECRET_TEXT_PX: f32 = 14.0;
/// See [`SECRET_BOX_RADIUS`]. The design's em, which egui wants in points.
const SECRET_TEXT_TRACKING: f32 = 0.1;
/// The halo's width, from `box-shadow: 0 0 0 3px`.
const SECRET_FOCUS_RING: f32 = 3.0;

/// The line under the box: a 13px check stroked at 2.8, `gap: 8px`, and 12px
/// copy beside it.
const VALIDITY_GAP: f32 = 8.0;
/// See [`VALIDITY_GAP`].
const VALIDITY_MARK: f32 = 13.0;
/// See [`VALIDITY_GAP`].
const VALIDITY_MARK_STROKE: f32 = 2.8;
/// See [`VALIDITY_GAP`].
const VALIDITY_PX: f32 = 12.0;

/// The green 6d strokes its check in (`stroke="#1b7a3f"`, which is also 6c's
/// header check), and the deeper one it sets the sentence beside it in
/// (`color: #17673a`).
///
/// Declared here rather than in `theme` for `loading_ui`'s reason, which
/// keeps design 7b's amber next to the badge that spends it: neither green is
/// a design-system role this app names anywhere -- there is no "success"
/// anything in `theme`, and the one other copy of `#1b7a3f` in this crate is
/// `scratch_window`'s own private `CHECK_GREEN`. A `theme` constant is for a
/// colour more than one surface reaches for by name, and these two are read
/// off one panel.
const VALID_MARK_INK: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x7a, 0x3f);
/// See [`VALID_MARK_INK`].
const VALID_TEXT_INK: egui::Color32 = egui::Color32::from_rgb(0x17, 0x67, 0x3a);

/// The two parameter controls: a `gap: 12px` pair of `flex: 1` columns, each
/// a 12px `#605d5d` caption over its run at `gap: 6px`.
const CHOICE_COLUMN_GAP: f32 = 12.0;
/// See [`CHOICE_COLUMN_GAP`].
const CHOICE_LABEL_GAP: f32 = 6.0;
/// See [`CHOICE_COLUMN_GAP`].
const CHOICE_LABEL_PX: f32 = 12.0;

/// One run: `border: 1px solid #d7d3d3; border-radius: 7px; overflow: hidden`
/// around cells of `padding: 5px 0` at `font-size: 12px`, the one in force
/// filled `#1b3fa0` with white `font-weight: 600` type and the rest divided
/// from it by `border-left: 1px solid #d7d3d3`. 212 x 26 on screen.
const CHOICE_RADIUS: u8 = 7;
/// See [`CHOICE_RADIUS`].
const CHOICE_STROKE: f32 = 1.0;
/// See [`CHOICE_RADIUS`].
const CHOICE_PAD_Y: f32 = 5.0;
/// See [`CHOICE_RADIUS`].
const CHOICE_TEXT_PX: f32 = 12.0;

/// The footer band: `padding: 12px 16px` under a `1px solid #eae7e7` rule on
/// `background: #fbfaf9` ([`theme::CARD_TINT`]), with `gap: 9px` between its
/// answers. 468 x 59 on screen, which is that padding around the 34 the
/// outlined answer's border makes of its declared 32.
const MANUAL_FOOTER_PAD_X: f32 = 16.0;
/// See [`MANUAL_FOOTER_PAD_X`].
const MANUAL_FOOTER_PAD_Y: f32 = 12.0;
/// See [`MANUAL_FOOTER_PAD_X`].
const MANUAL_FOOTER_GAP: f32 = 9.0;
/// The hint at the band's right-hand end: `font-size: 12px; color: #9b9797`.
const DISMISS_HINT_PX: f32 = 12.0;

/// How much of the window is left round the card before its BODY scrolls,
/// and the floor under what the body is given.
///
/// **The design never drew these two panels fused and this app does.** `#6d`
/// is 345.8 tall and `#6c` is 560; the card that carries both -- with a
/// record that already has a code, so the caution band is up as well -- comes
/// out around 760 against `vault_window::WINDOW_SIZE`'s 740. Something has to
/// give on a window that size, and the bands are what must not: the header
/// says what the card is, and the footer holds both answers and the line
/// naming the key that closes it. So the body scrolls under them.
///
/// That is `draw_add_modal`'s own note answered rather than a new idea: it
/// records a card whose only way out could sit off the bottom of a
/// centre-anchored `Area` that does not scroll, reported as a hang. Escape is
/// the other half of that answer and is unchanged.
///
/// `MODAL_BREATHING` is 16 of scrim above and below, so a card at its limit
/// does not read as one pinned to the window's edges. The floor exists for
/// the window `settings::MIN_VAULT_WINDOW_SIZE` allows: below it the body
/// would be given a negative height and egui would lay the card out inside
/// out.
const MODAL_BREATHING: f32 = 32.0;
/// See [`MODAL_BREATHING`].
const MIN_BODY_HEIGHT: f32 = 160.0;

/// 6c's caution band, which sits between the body and the footer:
/// `border-top: 1px solid #eae7e7` over `background: #fef6e7`, a 15px warning
/// triangle stroked in `#8a5a06` and dropped by its own `margin-top: 1px`,
/// `gap: 9px`, and 12px copy in `#7a4f05` at `line-height: 1.5`.
///
/// The mark's size, stroke, drop, gap, type size and line height are 6a's
/// footer numbers to the point -- the design sets both bands from the same
/// declarations -- so they are [`FOOTER_GLYPH`] and its neighbours rather
/// than a second copy. Only the three colours and the padding are this
/// band's own, and the padding is 6d's 16 rather than 6c's 18 because this is
/// 6d's card and every other band on it is inset 16.
const CAUTION_FILL: egui::Color32 = egui::Color32::from_rgb(0xfe, 0xf6, 0xe7);
/// See [`CAUTION_FILL`].
const CAUTION_MARK_INK: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x5a, 0x06);
/// See [`CAUTION_FILL`].
const CAUTION_TEXT_INK: egui::Color32 = egui::Color32::from_rgb(0x7a, 0x4f, 0x05);

/// The check 6d sets under its field and 6c sets in its header, in the
/// design's own `viewBox` coordinates: `<path d="M20 6 9 17l-5-5">`.
const CHECK_MARK: &[Svg] = &[Svg::Line(&[(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)])];

/// 6d's header band, and **the ✕ in its corner**. Answers whether that mark
/// was pressed, and how tall the band came out.
///
/// # Why there is a mark here at all, when neither 6d nor 6c draws one
///
/// Because 6a does, and 6a is the screen before this one. This card and the
/// picker are one surface a step apart -- same width, same frame, same way
/// back -- and until this pass the corner gesture worked on the first step
/// and silently stopped working on the second. That is worse than having no
/// mark anywhere: a user who dismissed the picker from its corner once has
/// been taught where this card's dismiss is, and the card that answers Escape
/// but not the corner is the one that reads as hung. The app-wide rule --
/// every dialog closes from its corner -- is the one that decides this, and
/// the design panels are silent on it rather than against it.
///
/// It is [`theme::modal_dismiss_mark`] and not a typed U+2715 for
/// `picker_header`'s recorded reason (that codepoint is in neither Archivo nor
/// egui's fallback stack and lands as a tofu box), and its inset is
/// [`theme::MODAL_CLOSE_INSET`] rather than anything derived here, because
/// two files quietly answering "how far off the edge" differently is the
/// exact drift that constant was created to end.
///
/// **What it does is what Escape does**, and that is not a coincidence to be
/// maintained by hand: [`draw_add_form`] turns a press into
/// [`TotpAddAction::Cancel`], which is the single value
/// [`draw_add_modal`]'s Escape branch returns. One outcome, two gestures --
/// so there is no state reachable through one and not the other.
///
/// # The mark does not allocate, so the band is unchanged
///
/// [`theme::modal_dismiss_mark`] is handed a rectangle and interacts at it;
/// it claims no layout space. The band's height is still its padding around
/// the taller of the title and [`MANUAL_HEADER_MARK`] -- which is
/// [`theme::CLOSE_MARK_HIT`]'s sixteen exactly -- so the hit box fits inside
/// what was already reserved and [`stage_header_height`] does not move.
///
/// **Two states, one band.** While the user is typing it is 6d's own
/// [`HEADING`] alone. Once a decoder has filled the field it is 6c's header
/// instead -- a green check, [`CODE_READ_LABEL`], and [`CODE_READ_KIND`] at
/// the far right -- which is the same pair this form used to draw as a row
/// inside the body, put where the design puts it. The privacy decision that
/// pair encodes is unchanged and is [`CODE_READ_LABEL`]'s own: a scanned
/// payload is a URI with `secret=` in the middle of it and must not be poured
/// into a box that paints what it holds.
///
/// The type is 6d's 14px/700 in both states rather than 6c's 15px/800,
/// because this is 6d's card: a header that grew a point and a weight when
/// the field below it was replaced would read as a different card rather than
/// as the same one a step on.
///
/// Reports `(the band's height, whether the ✕ was pressed)`. The height is
/// what the body under it has to be measured against -- see
/// [`MODAL_BREATHING`].
fn manual_header(ui: &mut egui::Ui, scanned: bool) -> (f32, bool) {
    let title = lay_out(
        ui,
        if scanned { CODE_READ_LABEL } else { HEADING },
        f32::INFINITY,
        face(MANUAL_HEADER_TITLE_PX, theme::BOLD, theme::INK),
    );
    let kind = scanned.then(|| {
        lay_out(
            ui,
            CODE_READ_KIND,
            f32::INFINITY,
            body_face(MANUAL_HEADER_KIND_PX, theme::TEXT_GHOST),
        )
    });
    // The band is as tall as its padding around the taller of the title and
    // the mark, plus the rule -- see [`HEADER_HEIGHT`] on why the rule is
    // extra rather than taken out of the padding.
    let content = title.size().y.max(MANUAL_HEADER_MARK);
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), MANUAL_HEADER_PAD_Y * 2.0 + content + RULE),
        egui::Sense::hover(),
    );

    // The padding box, which is the band without its rule: centring the mark
    // on the band itself would drop it half a point low, which is
    // `picker_header`'s own note about the same arithmetic.
    let line = egui::Rect::from_min_max(band.min, egui::pos2(band.right(), band.bottom() - RULE));
    // **Registered before anything else in the band is painted**, so the
    // rectangle the pointer is tested against is claimed before the galleys
    // that share the band go down. Nothing here overlaps it -- see the kind
    // label below, which is held off it -- so paint order costs nothing.
    let close = theme::modal_dismiss_mark(ui, line, theme::CloseInk::OnCard);

    let painter = ui.painter();
    // `border-bottom: 1px solid #eae7e7`, run out past the card's own stroke
    // on both sides so the rule meets the border instead of stopping a point
    // short of it. [`BAND_BLEED`].
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.bottom() - RULE),
            egui::pos2(band.right() + BAND_BLEED, band.bottom()),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    // Centred on the padding box, which is the band without its rule.
    let middle = band.center().y - RULE / 2.0;
    let mut left = band.left() + MANUAL_HEADER_PAD_X;
    if scanned {
        let mark = egui::Rect::from_center_size(
            egui::pos2(left + MANUAL_HEADER_MARK / 2.0, middle),
            egui::Vec2::splat(MANUAL_HEADER_MARK),
        );
        paint_svg(painter, mark, CHECK_MARK, MANUAL_HEADER_MARK_STROKE, VALID_MARK_INK);
        left = mark.right() + MANUAL_HEADER_GAP;
    }
    painter.galley(
        egui::pos2(left, middle - title.size().y / 2.0),
        title,
        theme::INK,
    );
    if let Some(kind) = kind {
        // **Hung off the ✕ and not off the band's padding.** 6c sets this
        // label at the far right of its header because nothing else is there;
        // this card now has a dismiss in that corner, and a right edge
        // computed from [`MANUAL_HEADER_PAD_X`] would put "otpauth://totp"
        // straight through the mark's arms. Measured from `close.rect` rather
        // than from [`theme::MODAL_CLOSE_INSET`] plus a hit width restated
        // here, so the two cannot drift: whatever rectangle the shared mark
        // took is the rectangle this clears, by [`MANUAL_HEADER_GAP`] -- the
        // same gap the header already uses between its check and its title.
        painter.galley(
            egui::pos2(
                close.rect.left() - MANUAL_HEADER_GAP - kind.size().x,
                middle - kind.size().y / 2.0,
            ),
            kind,
            theme::TEXT_GHOST,
        );
    }
    (band.height(), close.clicked())
}

/// **Which item this is being written to**, at the top of the body.
///
/// 6d draws no such block -- its panel is one field and two controls -- and
/// it is here because [`draw_confirmation`] declined to build 6c's "Saving
/// to" row on the grounds that *"the card already names it at the top"*.
/// That sentence has to stay true of this card, and a modal that can rewrite
/// a record's second factor and never names the record is the one thing this
/// surface must not be. So it is 6a's own `Adding to` block, in 6a's type
/// ([`SUBJECT_LABEL_PX`] over [`SUBJECT_NAME_PX`] at [`SUBJECT_GAP`]) --
/// which is what the user read one screen ago -- and without 6a's `padding:
/// 4px 4px 8px`, because that padding exists to separate the block from rows
/// packed 7px apart and this body already spaces its children 14.
fn manual_subject(ui: &mut egui::Ui, name: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = SUBJECT_GAP;
        ui.label(theme::regular(ADDING_TO_LABEL, SUBJECT_LABEL_PX).color(theme::TEXT_FAINT));
        ui.label(theme::bold(name, SUBJECT_NAME_PX).color(theme::INK));
    });
}

/// **6d's one field**, monospaced and tracked as the design sets it.
///
/// # Why this is not `theme::text_field`
///
/// The shared field is 38 tall and paints its contents in 14px proportional
/// type with no tracking. 6d's is 40.8 tall and sets what is in it in
/// `ui-monospace` at `font-size: 13px; letter-spacing: 0.1em`, and that is
/// not decoration on this one field: what is typed here is a base32 key read
/// off a card or a screen, character by character, and the two mistakes it
/// invites -- a 0 for an O, an I for a 1 -- are the two a proportional face
/// hides and a monospaced one separates. The box, its radius and its focused
/// halo ARE the shared field's, painted from the same [`theme`] colours in
/// the same order, so this differs from every other field in the app in
/// exactly the way the design says and in no other.
///
/// The face is carried in through `TextEdit::layouter` and not `.font()`,
/// which takes a `FontId` and can express no letter spacing -- the same
/// reason `detail::title_text` reaches for a layouter.
fn secret_field(ui: &mut egui::Ui, typed: &mut String) -> egui::Response {
    // A placeholder in the paint list, so the box lands UNDER the text egui
    // draws for the `TextEdit`. `theme::field_box`'s own trick.
    let under = ui.painter().add(egui::Shape::Noop);
    // **[`theme::FIELD_HEIGHT`], which is every other box on this card.**
    //
    // §6d's own height is `1px + 9px + 13×1.6 + 9px + 1px` = 40.8, and that is
    // what this allocated. Two points is nothing to measure and plenty to
    // SEE: the seed box sat a step deeper than the Digits and Period controls
    // under it and than every field on the edit form behind it, which is what
    // "field looks higher" was reading. The design draws this card alone on a
    // page and could not have shown that.
    let (outer, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT),
        egui::Sense::hover(),
    );
    // **The text row is CENTRED in the box, not laid out to fill it.**
    //
    // The box is `stroke*2 + pad*2 + line` tall, which is 6d's 40.8 measured
    // as border-box, and that is right. What was wrong is what went inside
    // it: `inner` was the whole padding box, 20.8 tall, and the layouter
    // asked for a line height to match. A 13px row is about 16, so egui had
    // five points of slack to place -- and it puts that slack BELOW the
    // glyphs, not around them. The text sat high in its box by half of it,
    // on every state of this card.
    //
    // So the row is measured and `inner` is exactly that tall, centred in
    // `outer`. The box does not move -- its height is the design's and stays
    // the formula above -- and the padding is what absorbs the difference,
    // which is what padding is for.
    let font = egui::FontId::new(SECRET_TEXT_PX, egui::FontFamily::Monospace);
    let row = theme::ascent_of(ui.ctx(), &font);
    // **The line box is the font's ASCENT**, which is what makes plain
    // centring optically right and the caret the height of the capitals -- see
    // `theme::ascent_of`, which carries the measurements and the report.
    let inner = egui::Rect::from_center_size(
        outer.center(),
        egui::vec2(outer.width() - (SECRET_BOX_STROKE + SECRET_BOX_PAD_X) * 2.0, row),
    );
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
        let mut job = egui::text::LayoutJob::default();
        // `no_max_width`, because this is a single-line field: 6d's box says
        // `word-break: break-all` for a URI too long to show, but a wrap
        // inside a one-line `TextEdit` puts the caret on a row the box is not
        // tall enough to paint. The overflow scrolls instead, which is what
        // every other field in this app does with a long value.
        job.wrap = egui::text::TextWrapping::no_max_width();
        job.append(
            buffer.as_str(),
            0.0,
            egui::TextFormat {
                extra_letter_spacing: SECRET_TEXT_PX * SECRET_TEXT_TRACKING,
                line_height: Some(row),
                font_id: font.clone(),
                color: theme::INK,
                ..Default::default()
            },
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(typed)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(inner.width())
            .layouter(&mut layouter),
    );
    // **Put the layout cursor back where the band left it.**
    //
    // `Ui::put` is not a paint call. It opens a child `Ui` clamped to the
    // rectangle it is given, and when that child closes egui advances the
    // PARENT's cursor past the child's rect -- `advance_cursor_after_rect`,
    // which *sets* the cursor rather than taking a maximum with it. The rect
    // handed over here is `inner`, the box's padding box, whose bottom is
    // `SECRET_BOX_STROKE + SECRET_BOX_PAD_Y` **above** the band this function
    // allocated. So the cursor came out ten points higher than the box it just
    // drew, and the next widget in the column -- [`validity_row`] -- opened
    // ten points into the field: with the design's own one-line sentence the
    // green check sat on the box's bottom border, and with a refusal long
    // enough to wrap the first row of red was painted straight through it.
    // That was reported as "wrong size and overlaps"; the size was the
    // sentence's fault and the overlap was this line's absence.
    //
    // The fix is the one `send_ui` reached for when the Sends strip wound its
    // own cursor backwards the same way: say explicitly where the cursor
    // belongs. It belongs after `outer`, which is what `allocate_exact_size`
    // above already claimed, so this restores rather than reserves and the
    // column's `item_spacing` then applies to the band the user can see
    // instead of to the text inside it.
    ui.advance_cursor_after_rect(outer);

    let radius = CornerRadius::same(SECRET_BOX_RADIUS);
    let border = if response.has_focus() {
        // `expand(2.0)` under a 3px stroke covers 0.5..3.5 outside the rect,
        // flush against the border's outer edge -- `theme::field_box`'s
        // measurement of the same `box-shadow`.
        ui.painter().rect_stroke(
            outer.expand(2.0),
            radius,
            egui::Stroke::new(SECRET_FOCUS_RING, theme::FOCUS_RING),
            egui::StrokeKind::Middle,
        );
        egui::Stroke::new(SECRET_BOX_STROKE, theme::BLUE)
    } else {
        egui::Stroke::new(SECRET_BOX_STROKE, theme::BORDER_STRONG)
    };
    ui.painter().set(
        under,
        egui::epaint::RectShape::new(
            outer,
            radius,
            theme::CARD,
            border,
            egui::StrokeKind::Middle,
        ),
    );
    response
}

/// **6d's line under the field**: the check and *"Valid base32 · 16
/// characters · spaces ignored"*, or the reason it is not.
///
/// **The check is drawn only when there is something to check off.** 6d has
/// one state and it is the valid one; a refusal keeps the sentence
/// [`refusal_sentence`] wrote and takes [`theme::ERROR`], with no mark at
/// all. A green tick beside "that isn't a one-time code" would be this
/// surface's single worst frame, and a red one is not in the design.
fn validity_row(ui: &mut egui::Ui, reading: &Reading) {
    let Some(line) = validity_line(reading) else {
        return;
    };
    let refused = matches!(reading, Reading::Refused(_));
    let ink = if refused { theme::ERROR } else { VALID_TEXT_INK };
    let indent = if refused { 0.0 } else { VALIDITY_MARK + VALIDITY_GAP };
    let width = ui.available_width();
    let text = lay_out(ui, &line, width - indent, body_face(VALIDITY_PX, ink));
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(width, text.size().y.max(VALIDITY_MARK)),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    if !refused {
        paint_svg(
            painter,
            egui::Rect::from_center_size(
                egui::pos2(row.left() + VALIDITY_MARK / 2.0, row.center().y),
                egui::Vec2::splat(VALIDITY_MARK),
            ),
            CHECK_MARK,
            VALIDITY_MARK_STROKE,
            VALID_MARK_INK,
        );
    }
    painter.galley(
        egui::pos2(row.left() + indent, row.center().y - text.size().y / 2.0),
        text,
        ink,
    );
}

/// **6d's two parameter controls**, side by side. Answers whichever of them
/// was pressed this frame.
///
/// The columns are `flex: 1` either side of a 12px gap, so each is exactly
/// half of what is left of the body -- 212 on 6d's own card -- and each run
/// fills its column with equal cells. That is the whole reason this is not
/// `theme::segmented_control`: that control sizes every cell to its own
/// label, which is right for the Preferences window's rows of prose and
/// wrong here, where the design draws two runs of the same width holding
/// cells of the same width and the labels are one character long. Everything
/// else about the run IS the shared control's -- [`CHOICE_RADIUS`] is its
/// `SEGMENT_RADIUS`, the cell in force takes [`theme::BLUE`] behind white and
/// the dead run takes [`theme::BLUE_WASH`] behind [`theme::TEXT_GHOST`],
/// which is `segmented_control_disabled`'s own answer to "you have this one
/// and cannot change it".
fn parameter_controls(
    ui: &mut egui::Ui,
    digits: u8,
    period: u16,
    live: bool,
) -> (Option<u8>, Option<u16>) {
    let digit_faces: Vec<String> = DIGITS_CHOICES.iter().map(|d| d.to_string()).collect();
    let period_faces: Vec<String> =
        PERIOD_CHOICES.iter().map(|p| format!("{p} s")).collect();

    let caption = lay_out(
        ui,
        DIGITS_LABEL,
        f32::INFINITY,
        body_face(CHOICE_LABEL_PX, theme::TEXT_MUTED),
    );
    let run_height = CHOICE_STROKE * 2.0 + CHOICE_PAD_Y * 2.0 + caption.size().y;
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(width, caption.size().y + CHOICE_LABEL_GAP + run_height),
        egui::Sense::hover(),
    );
    let column = (width - CHOICE_COLUMN_GAP) / 2.0;
    let run_top = row.top() + caption.size().y + CHOICE_LABEL_GAP;

    let mut chosen_digits = None;
    let mut chosen_period = None;
    for (index, (label, faces)) in
        [(DIGITS_LABEL, &digit_faces), (PERIOD_LABEL, &period_faces)].into_iter().enumerate()
    {
        let left = row.left() + (column + CHOICE_COLUMN_GAP) * index as f32;
        let caption = lay_out(
            ui,
            label,
            column,
            body_face(CHOICE_LABEL_PX, theme::TEXT_MUTED),
        );
        ui.painter().galley(egui::pos2(left, row.top()), caption, theme::TEXT_MUTED);
        let run = egui::Rect::from_min_size(
            egui::pos2(left, run_top),
            egui::vec2(column, run_height),
        );
        let selected = if index == 0 {
            DIGITS_CHOICES.iter().position(|d| *d == digits)
        } else {
            PERIOD_CHOICES.iter().position(|p| *p == period)
        };
        let pressed = choice_run(ui, run, faces, selected, live, ui.id().with(label));
        match (index, pressed) {
            (0, Some(at)) => chosen_digits = Some(DIGITS_CHOICES[at]),
            (_, Some(at)) => chosen_period = Some(PERIOD_CHOICES[at]),
            (_, None) => {}
        }
    }
    (chosen_digits, chosen_period)
}

/// One run of [`parameter_controls`]: equal cells filling `run`, joined into
/// one pill. Answers the index of the cell pressed this frame.
///
/// `selected` is an `Option` because it can genuinely be none of them: a
/// pasted URI states its own parameters and [`controls_for`] hands them
/// straight through, and `parse_otpauth` will read a `period` this control
/// does not offer. A run with nothing lit says that honestly; a run that
/// defaulted to lighting the first cell would claim the card was 30 seconds
/// when it was 45.
fn choice_run(
    ui: &mut egui::Ui,
    run: egui::Rect,
    faces: &[String],
    selected: Option<usize>,
    live: bool,
    id: egui::Id,
) -> Option<usize> {
    let response = ui.interact(
        run,
        id,
        if live { egui::Sense::click() } else { egui::Sense::hover() },
    );
    let pointer = if live { response.hover_pos() } else { None };
    // The cells are laid out INSIDE the run's border, which is the box the
    // design's `flex: 1` children divide between them.
    let inside = run.shrink(CHOICE_STROKE);
    let count = faces.len();
    let edge = |index: usize| inside.left() + inside.width() * index as f32 / count as f32;

    let mut hovered = None;
    let mut chosen = None;
    for (index, face_text) in faces.iter().enumerate() {
        let cell = egui::Rect::from_min_max(
            egui::pos2(edge(index), inside.top()),
            egui::pos2(edge(index + 1), inside.bottom()),
        );
        let over = pointer.is_some_and(|at| cell.contains(at));
        if over {
            hovered = Some(index);
        }
        let lit = selected == Some(index);
        let (fill, ink) = match (lit, live, over) {
            (true, true, _) => (theme::BLUE, egui::Color32::WHITE),
            (true, false, _) => (theme::BLUE_WASH, theme::TEXT_GHOST),
            (false, true, true) => (theme::CANVAS, theme::INK),
            (false, true, false) => (theme::CARD, theme::INK),
            (false, false, _) => (theme::CARD, theme::TEXT_GHOST),
        };
        // **The rounding belongs to the run and not the cell**, exactly as
        // `theme::segment_corners` puts it: the first cell rounds its left
        // corners, the last its right ones, and the square edges between them
        // are what make the interior lines read as seams.
        let first = if index == 0 { CHOICE_RADIUS } else { 0 };
        let last = if index + 1 == count { CHOICE_RADIUS } else { 0 };
        ui.painter().rect_filled(
            cell,
            CornerRadius { nw: first, sw: first, ne: last, se: last },
            fill,
        );
        if index > 0 {
            // `border-left: 1px solid #d7d3d3` between one cell and the next.
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    cell.min,
                    egui::pos2(cell.left() + CHOICE_STROKE, cell.bottom()),
                ),
                CornerRadius::ZERO,
                theme::BORDER_STRONG,
            );
        }
        let galley = ui.painter().layout_no_wrap(
            face_text.clone(),
            egui::FontId::new(
                CHOICE_TEXT_PX,
                if lit {
                    egui::FontFamily::Name(theme::SEMIBOLD.into())
                } else {
                    egui::FontFamily::Proportional
                },
            ),
            ink,
        );
        ui.painter().galley(
            egui::pos2(
                cell.center().x - galley.size().x / 2.0,
                cell.center().y - galley.size().y / 2.0,
            ),
            galley,
            ink,
        );
    }
    // The outline last, over the cells, so a lit cell's fill cannot paint out
    // the run's own border where the two meet.
    ui.painter().rect_stroke(
        run,
        CornerRadius::same(CHOICE_RADIUS),
        egui::Stroke::new(CHOICE_STROKE, theme::BORDER_STRONG),
        egui::StrokeKind::Inside,
    );
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.clicked() {
        chosen = hovered;
    }
    chosen
}

/// [`REPLACE_WARNING`] laid out to the width the caution band leaves it, once
/// per frame. See [`caution_band`] on why this is not done inside it.
fn caution_text(ui: &egui::Ui, width: f32) -> std::sync::Arc<egui::Galley> {
    lay_out(
        ui,
        REPLACE_WARNING,
        width - MANUAL_FOOTER_PAD_X * 2.0 - FOOTER_GLYPH - FOOTER_GAP,
        egui::TextFormat {
            line_height: Some(FOOTER_TEXT_PX * FOOTER_LINE),
            ..body_face(FOOTER_TEXT_PX, CAUTION_TEXT_INK)
        },
    )
}

/// How tall the caution band comes out around `text`.
///
/// `align-items: flex-start` on 6c's band: both children hang off the top
/// padding, so it is as tall as the taller of them plus the two paddings and
/// the rule over them.
/// How far [`caution_band`]'s sentence drops to sit centred -- see
/// [`theme::ink_drop`]. Its own function so [`caution_band_height`] and the
/// placement cannot disagree about the number.
fn caution_ink_drop(ui: &egui::Ui) -> f32 {
    theme::ink_drop(
        ui.ctx(),
        &egui::FontId::new(FOOTER_TEXT_PX, egui::FontFamily::Proportional),
        Some(FOOTER_TEXT_PX * FOOTER_LINE),
    )
}

fn caution_band_height(text: &egui::Galley) -> f32 {
    RULE + MANUAL_FOOTER_PAD_Y * 2.0
        + text.size().y.max(FOOTER_GLYPH + FOOTER_GLYPH_DROP)
}

/// How tall the footer band comes out. A constant of the design rather than
/// of its contents: both answers are `theme::BUTTON_HEIGHT`, which is 6d's
/// declared `height: 32px`, and the band is `padding: 12px` and its rule
/// around the 34 the outlined one's border makes of that.
fn manual_footer_height() -> f32 {
    RULE + MANUAL_FOOTER_PAD_Y * 2.0 + theme::BUTTON_HEIGHT + CHOICE_STROKE * 2.0
}

/// **6c's caution band**, between the body and the footer.
///
/// The sentence is [`REPLACE_WARNING`] and it has not changed; where it is
/// drawn has. It used to be a red label inside the body, which put the one
/// irreversible fact on this card in the same column as the field and the
/// controls, at the same weight as a validation message. 6c gives it a band
/// of its own on `#fef6e7` directly over the button that does the thing --
/// the last thing crossed on the way to pressing it -- and that is card
/// chrome, so it is built here with the rest of the chrome.
///
/// **Amber and not [`theme::ERROR`]**, which is the design's own choice and
/// the right one: nothing has gone wrong yet. The red belongs to
/// [`validity_row`]'s refusals, where something has.
///
/// The sentence is laid out by [`caution_text`] and handed in rather than
/// measured here, because the body above this band has to be given its room
/// **before** either is drawn -- see [`MODAL_BREATHING`] -- and a second
/// layout of the same wrapping paragraph is a second answer waiting to
/// disagree with the first by a point.
fn caution_band(ui: &mut egui::Ui, text: std::sync::Arc<egui::Galley>) {
    let width = ui.available_width();
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(width, caution_band_height(&text)),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    // Square at both ends: the footer is below this one, so neither edge of
    // it is the card's corner. Bled sideways for [`BAND_BLEED`]'s reason.
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.bottom()),
        ),
        CornerRadius::ZERO,
        CAUTION_FILL,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.top() + RULE),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    // The triangle is `theme`'s and not a fourth mark in this file's own
    // `Svg` vocabulary: 6c's is a rounded path, `Svg::Line` strokes open
    // polylines, and `theme::paint_warning_glyph` already draws exactly this
    // sign at exactly this 15px for exactly the reason it records -- U+26A0
    // is in neither Archivo nor egui's fallback stack.
    theme::paint_warning_glyph(
        painter,
        egui::Rect::from_min_size(
            egui::pos2(
                band.left() + MANUAL_FOOTER_PAD_X,
                band.top() + RULE + MANUAL_FOOTER_PAD_Y + FOOTER_GLYPH_DROP,
            ),
            egui::Vec2::splat(FOOTER_GLYPH),
        ),
        CAUTION_MARK_INK,
    );
    // **Dropped by the ink offset.** The owner: "\"This record...\" text - not
    // centered". The galley's rows are `FOOTER_LINE` tall and the face inks
    // only the upper part of each, so a block hung off the top padding sits
    // high in a band whose height was computed from that same galley -- the
    // slack all lands underneath. See `theme::ink_drop`, which measures it.
    painter.galley(
        egui::pos2(
            band.left() + MANUAL_FOOTER_PAD_X + FOOTER_GLYPH + FOOTER_GAP,
            band.top() + RULE + MANUAL_FOOTER_PAD_Y + caution_ink_drop(ui),
        ),
        text,
        CAUTION_TEXT_INK,
    );
}

/// **The card's answers**, in one band. Reports `(save, back to the picker)`.
///
/// # One row for both of the design's two
///
/// 6d draws `Save code / Cancel` and 6c draws `Replace code ↵ / Scan again`
/// with `Esc discards` pushed to the far right, and they are the same band at
/// two moments rather than two bands: same `padding`, same `gap: 9px`, same
/// `border-top` on the same tint, same two answers in the same order. So it
/// is built once, and what varies is what the design says varies -- the
/// primary's face, which is [`submit_label`], and whether it may be pressed
/// at all, which is [`can_save`].
///
/// # Why the second button is the way back and not `Cancel`
///
/// The design gives this band a primary, ONE secondary and an optional hint,
/// and this app has three answers to fit in it: save, cancel, and the route
/// back to 6a. 6c settles which two: where the hint is drawn, the secondary
/// is the way back (its *"Scan again"*) and the hint carries the cancel.
/// That is not a trade here, it is a strict improvement -- Escape is answered
/// from anywhere on the card, including from inside the field, where a button
/// has to be reached; and it is the fix `draw_add_modal`'s own note records
/// for a card whose only way out could sit off the bottom of a short window.
///
/// The label is [`OTHER_WAYS_LABEL`] and not 6c's literal *"Scan again"*,
/// because this app's way back is 6a -- four routes, of which scanning is
/// one. A button reading "Scan again" that opened a picker would be the kind
/// of promise this file refuses elsewhere.
///
/// # What is NOT drawn: 6c's ↵ keycap
///
/// 6c hangs a `↵` off its primary. Nothing in this crate answers Enter at
/// this stage -- the picker binds it to its own default row and that is the
/// only binding -- so drawing the cap would advertise a key that does
/// nothing, which is `ROUTES`' rule about the ordinals restated. Binding it
/// instead was the other option and is worse: with the field focused, Enter
/// is the key a user presses to finish typing, and on an item that already
/// has a code the action behind this button destroys a seed that cannot be
/// recovered. `theme::destructive_button` refuses a shortcut for the same
/// reason in the delete modal.
fn manual_footer(
    ui: &mut egui::Ui,
    primary: &str,
    enabled: bool,
    scanned: bool,
) -> FooterPress {
    let width = ui.available_width();
    let (band, _) =
        ui.allocate_exact_size(egui::vec2(width, manual_footer_height()), egui::Sense::hover());

    let painter = ui.painter();
    // The tint, rounded into the card's bottom corners with the card's own
    // radius and bled out to its rect -- [`BAND_BLEED`].
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.bottom() + BAND_BLEED),
        ),
        CornerRadius { nw: 0, ne: 0, sw: CARD_RADIUS, se: CARD_RADIUS },
        theme::CARD_TINT,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.top() + RULE),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(
                    band.left() + MANUAL_FOOTER_PAD_X,
                    band.top() + RULE + MANUAL_FOOTER_PAD_Y,
                ),
                egui::pos2(
                    band.right() - MANUAL_FOOTER_PAD_X,
                    band.bottom() - MANUAL_FOOTER_PAD_Y,
                ),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.spacing_mut().item_spacing.x = MANUAL_FOOTER_GAP;
    // The app's own button family rather than two boxes measured off this
    // panel: `theme::BUTTON_HEIGHT` IS 6d's declared `height: 32px`, the fill,
    // the outline and the disabled fade are the design system's, and the one
    // number that differs is the radius -- 7 against 6d's 8 -- which is a
    // point, against a footer whose answers would otherwise round differently
    // from every other footer in this app.
    let save = theme::primary_button_enabled(&mut inner, primary, None, enabled).clicked();
    // **§6c's second answer is the way back; §6d's is Cancel**, and the card
    // knows which it is. The way back exists because a scan that decoded the
    // wrong thing is fixed by choosing a route again -- and a card the user
    // TYPED into arrived from no route, so there is nothing there to go back
    // to and the button would be a door into a picker they never opened.
    if scanned {
        let back = theme::secondary_button(&mut inner, OTHER_WAYS_LABEL).clicked();
        // §6c's hint, and §6c's alone -- see [`CANCEL_LABEL`].
        inner.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(DISMISS_HINT)
                    .size(DISMISS_HINT_PX)
                    .color(theme::TEXT_GHOST),
            );
        });
        FooterPress { save, back, cancel: false }
    } else {
        let cancel = theme::secondary_button(&mut inner, CANCEL_LABEL).clicked();
        FooterPress { save, back: false, cancel }
    }
}

/// Which of [`manual_footer`]'s answers was pressed.
///
/// A struct rather than the `(bool, bool)` this returned, because the band
/// now has THREE possible answers across its two shapes and a third bare
/// bool in a tuple is the kind of thing a call site gets the wrong way round
/// once and nobody notices: `back` closes nothing and `cancel` closes
/// everything.
struct FooterPress {
    /// The primary: write the seed.
    save: bool,
    /// §6c only: back to 6a's routes.
    back: bool,
    /// §6d only: close without writing, exactly as Escape does.
    cancel: bool,
}

/// **Design 6d**, with 6c fused into the bottom of the same card: the header
/// band, the field and its line, the two parameter controls, the confirmation
/// once there is something to confirm, and the answers.
///
/// `now_unix` is the clock, passed in rather than read here: the live code is
/// the one thing on this surface that changes without the user touching it,
/// and a `SystemTime::now()` inside this function would make every assertion
/// about the code untestable. `vault_window::mod` reads the clock once per
/// frame and hands it down.
///
/// **What 6d draws that this does not.** Its panel carries a compact live-code
/// strip of its own -- a 22px code, a `flex: 1` track and `22 s`, on
/// `#eef2fc` -- and that is 6c's [`draw_code_panel`] said smaller. Drawing
/// both would put two live codes and two countdowns for one seed on one card,
/// so the fused card keeps 6c's, which is the larger of the two and the one
/// carrying the question the confirmation exists to ask. 6d's second panel,
/// *"When it doesn't work"*, is a documentation list of 6b's three capture
/// failures and not a control: those sentences are [`PickerRefusal`]'s, they
/// are painted one at a time on 6a where the route that failed can be pressed
/// again, and a permanent list of everything that could go wrong is not
/// something this app has anywhere.
pub fn draw_add_form(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    // Deferred to after the card, because `state` is borrowed inside it and
    // `back_to_picker` replaces the very `Zeroizing` the field is editing.
    let mut back_to_picker = false;
    stage_card(ui, MANUAL_WIDTH, |ui| {
        // The bands are measured before the body is drawn, because what is
        // left of the window after them is what the body may have -- see
        // [`MODAL_BREATHING`]. The caution band's sentence is laid out once
        // here and handed on to the band itself.
        let (header, dismissed) = manual_header(ui, state.scanned);
        // **The corner gesture, answered as the key is.** `draw_add_modal`
        // turns Escape into exactly this value, so the ✕ and Escape cannot
        // reach different states -- see `manual_header`. Assigned here rather
        // than returned early because the rest of the card still has to be
        // drawn this frame: a modal that vanished mid-layout would leave the
        // footer's `Ui` half-built, and `draw_picker` answers its own ✕ the
        // same way for the same reason. Anything below that acts on this
        // frame -- Save, the way back -- assigns over it, which is
        // `draw_add_modal`'s stated ordering rule applied inside the card.
        if dismissed {
            action = TotpAddAction::Cancel;
        }
        let caution = state
            .already_has_code
            .then(|| caution_text(ui, ui.available_width()));
        let bands = header
            + caution.as_ref().map_or(0.0, |text| caution_band_height(text))
            + manual_footer_height();
        let room = (ui.ctx().content_rect().height() - MODAL_BREATHING - bands)
            .max(MIN_BODY_HEIGHT);

        // The body reports what it read, because the two bands under it are
        // decided by it: whether the primary may be pressed is `can_save`.
        //
        // `auto_shrink` is `[false, true]`: the card keeps its 470 whatever is
        // in it -- a modal that narrowed as the confirmation appeared would be
        // a different card -- and the body is only as tall as its contents
        // until it reaches `room`, so the short states of 6d do not open a
        // window-tall card round a single field.
        // **The card keeps ONE edge.** See [`MANUAL_SCROLLBAR_INSET`]: the bar
        // below is moved off the border and into the body's own padding, and
        // stopped from growing back over the body when the pointer is near it.
        // Set on the card's own `Ui` because this is the only scroll area
        // under it; the two bands that follow read no scroll spacing at all.
        {
            let scroll = &mut ui.spacing_mut().scroll;
            scroll.bar_width = theme::SCROLLBAR_WIDTH;
            scroll.floating_width = theme::SCROLLBAR_WIDTH;
            scroll.bar_outer_margin = MANUAL_SCROLLBAR_INSET;
        }
        let reading = egui::ScrollArea::vertical()
            .max_height(room)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(MANUAL_BODY_PAD))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = MANUAL_BODY_GAP;
                        manual_subject(ui, &state.item_name);

                        // **A scanned payload is NOT put in a text field.** See
                        // [`CODE_READ_LABEL`]: a `TextEdit` paints what it holds, and
                        // what a decoder hands over holds `secret=` in the middle of
                        // it. What says it was read is [`manual_header`]'s band.
                        //
                        // The reading is taken AFTER the field has been drawn, not
                        // before: what the user typed this frame is in the buffer by
                        // then, so the line under the box is about the keystroke that
                        // just landed rather than the one before it.
                        let reading = if state.scanned {
                            let reading = read_field(&state.typed, state.digits, state.period);
                            // The scanned card's own line, and only when there is
                            // something wrong. A hostile QR reaches `parse_otpauth`
                            // exactly as a hostile paste does, and its refusal has to
                            // be on screen; the valid case has already said so in the
                            // header and would only be repeating itself -- with the
                            // seed's length, which nothing on a scanned card asked
                            // to know.
                            if matches!(reading, Reading::Refused(_)) {
                                validity_row(ui, &reading);
                            }
                            reading
                        } else {
                            let mut reading = None;
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = SECRET_BLOCK_GAP;
                                ui.label(
                                    theme::regular(SECRET_HINT, SECRET_LABEL_PX)
                                        .color(theme::TEXT_MUTED),
                                );
                                secret_field(ui, &mut state.typed);
                                let read = read_field(&state.typed, state.digits, state.period);
                                validity_row(ui, &read);
                                reading = Some(read);
                            });
                            reading.expect("the field's column runs exactly once")
                        };

                        let (shown_digits, shown_period, controls_live) =
                            controls_for(&reading, &state.typed, state.digits, state.period);
                        let (chose_digits, chose_period) =
                            parameter_controls(ui, shown_digits, shown_period, controls_live);
                        if let Some(digits) = chose_digits {
                            state.digits = digits;
                        }
                        if let Some(period) = chose_period {
                            state.period = period;
                        }
                        if !controls_live {
                            // Why the runs are dead. 6d draws them live and has no
                            // slot for this, so it is the app's own sentence, put
                            // directly under the controls it is about -- the same
                            // place, and for the same reason, that 6a's refusal goes
                            // under the rows rather than under the privacy line.
                            ui.label(
                                theme::regular(PARAMETERS_FROM_URI, CHOICE_LABEL_PX)
                                    .color(theme::TEXT_FAINT),
                            );
                        }

                        if let Reading::Ok(auth) = &reading {
                            draw_confirmation(ui, auth, state, now_unix);
                        }
                        reading
                    })
                    .inner
            })
            .inner;

        if let Some(text) = caution {
            caution_band(ui, text);
        }
        let pressed = manual_footer(
            ui,
            submit_label(state.already_has_code),
            can_save(&reading),
            state.scanned,
        );
        if pressed.save {
            action = TotpAddAction::Save;
        }
        // The same value Escape and the ✕ produce, so §6d's three ways out
        // cannot reach three different states.
        if pressed.cancel {
            action = TotpAddAction::Cancel;
        }
        // **The way back to 6a**, and the reason this form is not a dead end
        // when the user arrived at it by scanning: a decode that produced the
        // wrong card, or a seed typed off the wrong line, is fixed by
        // choosing a route again rather than by cancelling out of the whole
        // feature and re-opening it.
        if pressed.back {
            back_to_picker = true;
        }
    });
    if back_to_picker {
        state.back_to_picker();
    }
    action
}

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

/// The card's width. `#6a` is `width: 470px` and the card is its only child,
/// so the card is 470 wide.
const PICKER_WIDTH: f32 = 470.0;

/// `border-radius: 12px` and `border: 1px solid #d7d3d3`
/// ([`theme::BORDER_STRONG`]) on the card itself.
const CARD_RADIUS: u8 = 12;
const CARD_STROKE: f32 = 1.0;

/// How far a band is painted PROUD of the rect it was allocated, so that none
/// of the card's own fill is left showing between the band and the border.
///
/// `theme::MODAL_BAND_BLEED`'s measurement, restated rather than imported
/// because it is the same 1pt stroke arrived at from the same geometry: an
/// `egui::Frame` strokes down the middle of its rect, so a band laid out
/// inside the frame stops half a point short of the border's inner edge and
/// leaves a hairline of card showing all the way round. That was reported
/// twice on the delete modal; it is the same shape here.
const BAND_BLEED: f32 = CARD_STROKE;

/// `box-shadow: 0 14px 34px rgba(45, 43, 43, 0.18)`.
///
/// Deeper than `theme`'s shared `MODAL_SHADOW` (`0 6px 20px` at the same
/// colour) because 6a's own declaration is deeper -- this card is taller than
/// the two-answer modals and the design lifts it further off the scrim to
/// match. The alpha is 0.18 x 255, rounded.
const CARD_SHADOW: egui::Shadow = egui::Shadow {
    offset: [0, 14],
    blur: 34,
    spread: 0,
    color: egui::Color32::from_rgba_unmultiplied_const(45, 43, 43, 46),
};

/// Every rule on this card: `1px solid #eae7e7` ([`theme::HAIRLINE`]), under
/// the header band and over the footer's.
const RULE: f32 = 1.0;

/// The header band: `padding: 15px 18px`, `gap: 10px`, a 17px mark stroked at
/// 2.2, and a 15px title at `font-weight: 800; letter-spacing: -0.01em`.
const HEADER_PAD_X: f32 = 18.0;
const HEADER_PAD_Y: f32 = 15.0;
const HEADER_GAP: f32 = 10.0;
const HEADER_GLYPH: f32 = 17.0;
const HEADER_GLYPH_STROKE: f32 = 2.2;
const HEADER_TITLE_PX: f32 = 15.0;
const HEADER_TITLE_TRACKING_EM: f32 = -0.01;

/// **The design's boxes are content-box**, so a declared size is the size
/// INSIDE the border and every border adds to what is on screen.
///
/// Measured rather than assumed: `#6a`'s tile declares `width: 34px` with a
/// `1px` border and its rectangle on screen is 36 x 36; a row declares
/// `padding: 12px 13px` with a `1px` border and comes out 62 tall against a
/// 36px tile. Reading those declarations as border-box -- which is what an
/// `egui` rect naturally is -- shrinks every one of them by two points and
/// pulls the whole card 10px short. So each band, row and tile below adds its
/// own border back.
const HEADER_HEIGHT: f32 = HEADER_PAD_Y * 2.0 + HEADER_GLYPH + RULE;

/// The tile's rectangle on screen: `width: 34px` plus its `1px` border on
/// each side. See [`HEADER_HEIGHT`] for why the border is added.
const TILE_OUTER: f32 = TILE_SIZE + TILE_STROKE * 2.0;

/// The body: `padding: 12px 14px 14px` with `gap: 7px` down its column.
const BODY_PAD_X: i8 = 14;
const BODY_PAD_TOP: i8 = 12;
const BODY_PAD_BOTTOM: i8 = 14;
const BODY_GAP: f32 = 7.0;

/// The "Adding to" block: `padding: 4px 4px 8px`, `gap: 2px`, a 12px label in
/// [`theme::TEXT_FAINT`] over a 13px `font-weight: 700` name.
const SUBJECT_PAD: i8 = 4;
const SUBJECT_PAD_BOTTOM: i8 = 8;
const SUBJECT_GAP: f32 = 2.0;
const SUBJECT_LABEL_PX: f32 = 12.0;
const SUBJECT_NAME_PX: f32 = 13.0;

/// One route row: `padding: 12px 13px`, `gap: 12px` between its three
/// children, `border-radius: 10px`, `border: 1px`.
const ROW_PAD_X: f32 = 13.0;
const ROW_PAD_Y: f32 = 12.0;
const ROW_GAP: f32 = 12.0;
const ROW_RADIUS: u8 = 10;
const ROW_STROKE: f32 = 1.0;

/// The icon tile at a row's left: `width/height: 34px`, `border-radius: 8px`,
/// `border: 1px`, holding a 17px mark stroked at 2.
const TILE_SIZE: f32 = 34.0;
const TILE_RADIUS: u8 = 8;
const TILE_STROKE: f32 = 1.0;
const TILE_GLYPH: f32 = 17.0;
const TILE_GLYPH_STROKE: f32 = 2.0;

/// A row's two lines: a 13px title, `gap: 2px`, and a 12px subtitle set at
/// `line-height: 1.45`.
const ROW_TITLE_PX: f32 = 13.0;
const ROW_TEXT_GAP: f32 = 2.0;
const ROW_SUB_PX: f32 = 12.0;
const ROW_SUB_LINE: f32 = 1.45;

/// **Which row wears 6a's selected treatment**: the first, which is the one
/// the design draws in blue and hangs the ↵ keycap off.
///
/// A constant and not a field on [`TotpAdd`], because nothing moves it. 6a's
/// blue row is the DEFAULT route -- "ordered by how often they're the right
/// one on Windows", and this is the one that is -- rather than a cursor the
/// user drives; there is no key on this surface that moves a selection, and a
/// highlight that cannot move is a recommendation.
const DEFAULT_ROW: usize = 0;

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

/// The gap between a dead row's title and the reason beside it.
///
/// The design has no such element -- 6a draws all four routes live, and so
/// does this app now. See [`DEFERRED_REASON`] for why the treatment is kept
/// with nothing currently using it, and [`route_row`] for the inks it says
/// it in.
const REASON_GAP: f32 = 6.0;

/// The footer: `padding: 12px 18px`, `background: #fbfaf9`
/// ([`theme::CARD_TINT`]), `gap: 9px`, a 15px mark stroked at 2.2 and pushed
/// down by its own `margin-top: 1px`, and 12px copy at `line-height: 1.5`.
const FOOTER_PAD_X: f32 = 18.0;
const FOOTER_PAD_Y: f32 = 12.0;
const FOOTER_GAP: f32 = 9.0;
const FOOTER_GLYPH: f32 = 15.0;
const FOOTER_GLYPH_STROKE: f32 = 2.2;
const FOOTER_GLYPH_DROP: f32 = 1.0;
const FOOTER_TEXT_PX: f32 = 12.0;
const FOOTER_LINE: f32 = 1.5;

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

/// The header's mark: a QR code's three finder squares and its quiet corner
/// -- `<rect x=3 y=3 w=7 h=7>` at (3,3), (14,3) and (3,14), then
/// `M14 14h3v3h-3z`, `M20 14v3` and `M17 20h4`.
const QR_MARK: &[Svg] = &[
    Svg::Rect(3.0, 3.0, 7.0, 7.0, 0.0),
    Svg::Rect(14.0, 3.0, 7.0, 7.0, 0.0),
    Svg::Rect(3.0, 14.0, 7.0, 7.0, 0.0),
    Svg::Rect(14.0, 14.0, 3.0, 3.0, 0.0),
    Svg::Line(&[(20.0, 14.0), (20.0, 17.0)]),
    Svg::Line(&[(17.0, 20.0), (21.0, 20.0)]),
];

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

/// One run of text, laid out to a known width so the surface can be measured
/// before it is allocated.
///
/// 6a sizes three of its boxes from their contents -- a row is as tall as its
/// two lines, the footer as tall as its wrapped sentence -- so every galley
/// on this card is laid out first and painted second.
fn lay_out(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    format: egui::TextFormat,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.append(text, 0.0, format);
    ui.painter().layout_job(job)
}

/// A [`lay_out`] format in one of the app's named Archivo faces -- the 600,
/// 700 and 800 weights 6a asks for.
fn face(size: f32, family: &str, colour: egui::Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: egui::FontId::new(size, egui::FontFamily::Name(family.into())),
        color: colour,
        ..Default::default()
    }
}

/// [`face`] at the body weight, which is [`egui::FontFamily::Proportional`]
/// and NOT [`theme::REGULAR`] by name: `theme::apply` binds Archivo Regular
/// as the proportional family itself and registers no named family for it, so
/// `Name("Archivo-Regular")` resolves to nothing and epaint panics on the
/// first galley. `theme::regular` says the same thing from the other side.
fn body_face(size: f32, colour: egui::Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: egui::FontId::new(size, egui::FontFamily::Proportional),
        color: colour,
        ..Default::default()
    }
}

/// A [`lay_out`] format in the monospace face 6a sets its right-hand
/// affordances in (`font-family: ui-monospace...; font-size: 10px`, which is
/// [`theme::CHIP_TEXT_PX`] -- the same size `theme::kbd_chip` renders).
fn mono_face(colour: egui::Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: egui::FontId::new(theme::CHIP_TEXT_PX, egui::FontFamily::Monospace),
        color: colour,
        ..Default::default()
    }
}

/// One frame of [`draw_picker`].
///
/// The rows' rectangles come back with the action, and that is not decoration
/// either: it is how a test **presses a row** rather than calling the function
/// the row would have called. `record_ui` shipped unreachable for a day
/// because every test it had called its draw function directly.
pub struct PickerFrame {
    /// What was pressed, if anything.
    pub action: TotpAddAction,
    /// Every row drawn this frame, with where it was drawn. In [`ROUTES`]'
    /// order.
    pub rows: Vec<(Route, egui::Rect)>,
    /// Where the header's ✕ was drawn, for the rows' reason exactly: it is
    /// 6a's only way out of this card, and a test that closes the picker by
    /// calling the thing the ✕ calls would not notice a ✕ that had stopped
    /// being reachable.
    pub close: egui::Rect,
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

/// **One route row of design 6a**: an icon tile, two lines of type, and the
/// affordance at the right that says how the row is reached.
///
/// # The row measures itself
///
/// 6a's row is `align-items: center` around `padding: 12px 13px` inside a
/// `1px` border, so its height is those plus whichever of its children is
/// taller -- the tile's 36px rectangle, or the title and subtitle stacked
/// with 2px between them. On the design's own copy the tile wins and the row
/// comes out at 62; that is why the height is computed rather than pinned to
/// a number, and it is also what makes a deferred row work. A subtitle
/// longer than anything 6a puts on a row wraps to two lines and the row
/// grows, and the pair of hardcoded heights this replaced (46 for a live
/// row, 62 for the dead one) were two guesses that had already been
/// corrected once against a render.
///
/// # The selected treatment
///
/// [`DEFAULT_ROW`] takes 6a's blue row exactly: `background: #eef2fc` inside
/// `border: 1px solid #1b3fa0`, a white tile edged `#b8c7ea` with a `#1b3fa0`
/// mark in it, a title at `font-weight: 700; color: #14307a`, a subtitle in
/// `#444141`, and the ↵ keycap. Every other row is `border: 1px solid #eae7e7`
/// with no fill, a `#f7f6f5` tile edged `#eae7e7` with a `#605d5d` mark, a
/// title at `font-weight: 600`, a subtitle in `#7d7979`, and its ordinal set
/// in 10px monospace in `#9b9797`.
///
/// **The ordinals are drawn as bare monospace text and the ↵ is drawn in a
/// filled chip, and that difference is 6a's and stays.** What has changed is
/// what they mean: they were decoration, and the owner read them as a promise
/// -- *"Enter and 1,2,3,4 not workong - supposed to be hjotkeys"* -- which is
/// the only reading a number down the right-hand edge of a list supports. So
/// the ordinals are key bindings now; see [`ROUTE_KEYS`] for the table, for
/// why `1` answers a row the design gives no digit, and for why the second
/// row's `2` is the one that is still only paint.
///
/// The chip is still not given to the other three. It marks the DEFAULT
/// route, not "this one has a shortcut", and drawing four chips would say
/// four rows are the recommended one.
fn route_row(ui: &mut egui::Ui, row: &RouteRow, index: usize) -> (egui::Response, egui::Rect) {
    let width = ui.available_width();
    let selected = index == DEFAULT_ROW;

    // The affordance is laid out FIRST because 6a's text column is `flex: 1`:
    // how much room the two lines get is what is left after the tile, the two
    // 12px gaps and whatever sits at the right. The keycap is the same
    // monospace character in a box with the design's `padding: 3px 7px`
    // either side of it, so one galley measures both.
    let ordinal = lay_out(
        ui,
        &(index + 1).to_string(),
        f32::INFINITY,
        mono_face(theme::TEXT_GHOST),
    );
    let affordance = if selected {
        ordinal.size().x + theme::RETURN_KEYCAP_PAD_X * 2.0
    } else {
        ordinal.size().x
    };

    let (title_ink, sub_ink, mark_ink) = if !row.enabled {
        // A dead row is drawn in the ghost ink rather than hidden: the fact
        // that this route exists and is off is the whole content of the row.
        (theme::TEXT_GHOST, theme::TEXT_GHOST, theme::TEXT_GHOST)
    } else if selected {
        (theme::BLUE_DEEP, theme::TEXT_SECONDARY, theme::BLUE)
    } else {
        (theme::INK, theme::TEXT_FAINT, theme::TEXT_MUTED)
    };

    // What is left for the `flex: 1` column once the border, the two 13px
    // paddings, the tile and the two 12px gaps have taken theirs.
    let text_width = (width
        - ROW_STROKE * 2.0
        - ROW_PAD_X * 2.0
        - TILE_OUTER
        - ROW_GAP * 2.0
        - affordance)
        .max(1.0);
    // The deferred row's reason shares the title's line, so the title wraps
    // against what is left of it.
    let reason = (!row.enabled).then(|| {
        lay_out(ui, DEFERRED_REASON, text_width, body_face(ROW_SUB_PX, sub_ink))
    });
    let reason_width = reason.as_ref().map_or(0.0, |g| g.size().x + REASON_GAP);
    let title = lay_out(
        ui,
        row.title,
        text_width - reason_width,
        face(
            ROW_TITLE_PX,
            if selected { theme::BOLD } else { theme::SEMIBOLD },
            title_ink,
        ),
    );
    let subtitle = lay_out(
        ui,
        row.subtitle,
        text_width,
        egui::TextFormat {
            line_height: Some(ROW_SUB_PX * ROW_SUB_LINE),
            ..body_face(ROW_SUB_PX, sub_ink)
        },
    );
    let text_height = title.size().y + ROW_TEXT_GAP + subtitle.size().y;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(
            width,
            ROW_STROKE * 2.0 + ROW_PAD_Y * 2.0 + text_height.max(TILE_OUTER),
        ),
        if row.enabled { egui::Sense::click() } else { egui::Sense::hover() },
    );

    // 6a draws no hover state at all -- it is one still frame -- so the fill
    // a hovered row takes is this app's own, and it is [`theme::CARD_TINT`]
    // rather than the [`theme::BLUE_WASH`] it used to be. The wash is now the
    // SELECTED row's own fill, and a hovered row wearing it would be a second
    // row claiming to be the default one.
    let (fill, edge) = if selected {
        (theme::BLUE_WASH, theme::BLUE)
    } else if row.enabled && response.hovered() {
        (theme::CARD_TINT, theme::HAIRLINE)
    } else {
        (theme::CARD, theme::HAIRLINE)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(ROW_RADIUS), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(ROW_RADIUS),
        egui::Stroke::new(ROW_STROKE, edge),
        egui::StrokeKind::Inside,
    );

    let tile = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + ROW_STROKE + ROW_PAD_X,
            rect.center().y - TILE_OUTER / 2.0,
        ),
        egui::Vec2::splat(TILE_OUTER),
    );
    painter.rect_filled(
        tile,
        CornerRadius::same(TILE_RADIUS),
        if selected { theme::CARD } else { theme::WINDOW_BG },
    );
    painter.rect_stroke(
        tile,
        CornerRadius::same(TILE_RADIUS),
        egui::Stroke::new(TILE_STROKE, if selected { theme::BLUE_EDGE } else { theme::HAIRLINE }),
        egui::StrokeKind::Inside,
    );
    paint_svg(
        painter,
        egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(TILE_GLYPH)),
        mark_for(row.route),
        TILE_GLYPH_STROKE,
        mark_ink,
    );

    let text_left = tile.right() + ROW_GAP;
    let text_top = rect.center().y - text_height / 2.0;
    painter.galley(egui::pos2(text_left, text_top), title.clone(), title_ink);
    if let Some(reason) = reason {
        // Centred on the title's own line rather than sharing its top edge:
        // it is a point smaller, and a shared top would hang it high.
        let reason_y = text_top + (title.size().y - reason.size().y) / 2.0;
        painter.galley(
            egui::pos2(text_left + title.size().x + REASON_GAP, reason_y),
            reason,
            sub_ink,
        );
    }
    painter.galley(
        egui::pos2(text_left, text_top + title.size().y + ROW_TEXT_GAP),
        subtitle,
        sub_ink,
    );

    let affordance_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - ROW_STROKE - ROW_PAD_X - affordance,
            rect.center().y - theme::CHIP_HEIGHT / 2.0,
        ),
        egui::vec2(affordance, theme::CHIP_HEIGHT),
    );
    if selected {
        theme::paint_return_keycap(painter, affordance_rect);
    } else {
        painter.galley(
            egui::pos2(
                affordance_rect.left(),
                affordance_rect.center().y - ordinal.size().y / 2.0,
            ),
            ordinal,
            theme::TEXT_GHOST,
        );
    }

    let response = if row.enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    (response, rect)
}

/// **The card both stages of this modal are drawn in**: white,
/// `border-radius: 12px`, a [`theme::BORDER_STRONG`] hairline round it, and
/// the shadow that lifts it off the scrim.
///
/// One function and not one per stage, because 6a and 6c declare it
/// identically -- `border: 1px solid #d7d3d3` under `box-shadow: 0 14px 34px
/// rgba(45, 43, 43, 0.18)` -- and it is genuinely one card: the by-hand form
/// is what the picker becomes when its third row is pressed, and a second
/// card built beside this one is two cards that round, shadow and bleed two
/// ways. `width` is the caller's because that is the one thing the design
/// does vary panel to panel; today both callers are 470.
///
/// The border is stroked twice, on the `Frame` and again over everything, for
/// `theme::modal_card`'s measured reason: the `Frame`'s own stroke is what
/// the bands are laid out against, and the footer's tint is painted out over
/// it so that no sliver of white card shows in the lower corners. Dropping
/// the first would shrink the layout rect; dropping the second would put the
/// white line back along the bottom curve.
fn stage_card<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let border = egui::Stroke::new(CARD_STROKE, theme::BORDER_STRONG);
    let framed = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .stroke(border)
        .shadow(CARD_SHADOW)
        .show(ui, |ui| {
            // The width INSIDE the border, which is what the design's boxes
            // are measured in: 470 outside, 468 of content, and a body inset
            // 14 either side of that gives 6a's own 440-wide rows.
            ui.set_width(width - CARD_STROKE * 2.0);
            // The three bands butt against each other, so the card's own
            // stack gets no item spacing at all; the body puts its own 7px
            // back inside itself.
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        });
    ui.painter().rect_stroke(
        framed.response.rect,
        CornerRadius::same(CARD_RADIUS),
        border,
        egui::StrokeKind::Middle,
    );
    framed.inner
}

/// 6a's header band: the QR mark, the title, and the ✕ that is this card's
/// only way out. Answers whether the ✕ was pressed, and where it was drawn.
fn picker_header(ui: &mut egui::Ui) -> (bool, egui::Rect) {
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
        egui::Sense::hover(),
    );
    // `border-bottom: 1px solid #eae7e7`, run out past the card's own stroke
    // on both sides so the rule meets the border instead of stopping a point
    // short of it.
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.bottom() - RULE),
            egui::pos2(band.right() + BAND_BLEED, band.bottom()),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );
    // The row is centred on the padding box, which is the band without its
    // rule -- see [`HEADER_HEIGHT`] on why the rule is extra rather than
    // taken out of the padding.
    let content =
        egui::Rect::from_min_max(band.min, egui::pos2(band.right(), band.bottom() - RULE));

    let mark = egui::Rect::from_center_size(
        egui::pos2(content.left() + HEADER_PAD_X + HEADER_GLYPH / 2.0, content.center().y),
        egui::Vec2::splat(HEADER_GLYPH),
    );
    paint_svg(ui.painter(), mark, QR_MARK, HEADER_GLYPH_STROKE, theme::BLUE);

    let title = lay_out(
        ui,
        PICKER_TITLE,
        f32::INFINITY,
        egui::TextFormat {
            // `letter-spacing: -0.01em`, which `egui::RichText` cannot
            // express -- 0.15pt at 15px, and the reason the title is a
            // `LayoutJob` rather than a label.
            extra_letter_spacing: HEADER_TITLE_PX * HEADER_TITLE_TRACKING_EM,
            ..face(HEADER_TITLE_PX, theme::EXTRABOLD, theme::INK)
        },
    );
    ui.painter().galley(
        egui::pos2(mark.right() + HEADER_GAP, content.center().y - title.size().y / 2.0),
        title,
        theme::INK,
    );

    // The ✕ is `theme::modal_dismiss_mark` and not a typed U+2715: that
    // codepoint is in neither Archivo nor egui's fallback stack and reaches
    // the screen as a tofu box, which is the measurement `theme` records
    // beside it.
    //
    // **It moved four points out**, and that is the whole reason it goes
    // through `theme` now. This header used to right-align the mark inside its
    // own [`HEADER_PAD_X`], which put the 16pt box 18 points off the card;
    // `prefs_ui` put its identical box 14 off. Same glyph, same size, two
    // answers, neither file aware of the other. `theme::MODAL_CLOSE_INSET`
    // settles it at 14 -- see that constant for why the smaller number is the
    // one that lines the *ink* up with this card's 18-point content column
    // rather than lining up the invisible box around it.
    //
    // `content` and not `band`: the band's last point is the rule under it,
    // and centring the mark on that would drop it half a point low.
    let close = theme::modal_dismiss_mark(ui, content, theme::CloseInk::OnCard);
    (close.clicked(), close.rect)
}

/// 6a's `Adding to` block: `padding: 4px 4px 8px` around a faint label and
/// the item's own name.
fn picker_subject(ui: &mut egui::Ui, name: &str) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: SUBJECT_PAD,
            right: SUBJECT_PAD,
            top: SUBJECT_PAD,
            bottom: SUBJECT_PAD_BOTTOM,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = SUBJECT_GAP;
            ui.label(
                theme::regular(ADDING_TO_LABEL, SUBJECT_LABEL_PX).color(theme::TEXT_FAINT),
            );
            ui.label(theme::bold(name, SUBJECT_NAME_PX).color(theme::INK));
        });
}

/// 6a's footer band: the info mark and the privacy line, on the card's tint.
///
/// The line is **pinned to the bottom of the card**, below the rows and above
/// nothing, because that is the last thing read before a route is chosen and
/// because a claim about what happens to the pixels belongs beside the row
/// that captures them.
fn picker_footer(ui: &mut egui::Ui) {
    footer_band(ui, PRIVACY_LINE);
}

/// [`picker_footer`]'s band with the sentence as an argument.
///
/// Split out for the camera stage, which makes a claim of its own about a
/// device rather than about pixels ([`WEBCAM_PRIVACY_LINE`]) and must make it
/// in the same place, in the same band, at the same size. A second footer
/// built beside this one is how the two come to disagree about where a
/// privacy claim sits on a card.
fn footer_band(ui: &mut egui::Ui, line: &str) {
    let width = ui.available_width();
    let text = lay_out(
        ui,
        line,
        width - FOOTER_PAD_X * 2.0 - FOOTER_GLYPH - FOOTER_GAP,
        egui::TextFormat {
            line_height: Some(FOOTER_TEXT_PX * FOOTER_LINE),
            ..body_face(FOOTER_TEXT_PX, theme::TEXT_MUTED)
        },
    );
    // `align-items: flex-start`: both children hang off the top padding, so
    // the band is as tall as the taller of them plus the two 12px paddings
    // and the rule over them -- see [`HEADER_HEIGHT`] on the border.
    let content = text.size().y.max(FOOTER_GLYPH + FOOTER_GLYPH_DROP);
    let (band, _) = ui.allocate_exact_size(
        egui::vec2(width, RULE + FOOTER_PAD_Y * 2.0 + content),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    // The tint, bled out to the card's own rect and rounded into its bottom
    // corners with the card's own radius -- see [`BAND_BLEED`].
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.bottom() + BAND_BLEED),
        ),
        CornerRadius { nw: 0, ne: 0, sw: CARD_RADIUS, se: CARD_RADIUS },
        theme::CARD_TINT,
    );
    // `border-top: 1px solid #eae7e7`.
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(band.left() - BAND_BLEED, band.top()),
            egui::pos2(band.right() + BAND_BLEED, band.top() + RULE),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    let mark = egui::Rect::from_min_size(
        egui::pos2(
            band.left() + FOOTER_PAD_X,
            band.top() + RULE + FOOTER_PAD_Y + FOOTER_GLYPH_DROP,
        ),
        egui::Vec2::splat(FOOTER_GLYPH),
    );
    paint_svg(painter, mark, INFO_MARK, FOOTER_GLYPH_STROKE, theme::TEXT_FAINT);
    painter.galley(
        egui::pos2(mark.right() + FOOTER_GAP, band.top() + RULE + FOOTER_PAD_Y),
        text,
        theme::TEXT_MUTED,
    );
}

/// **Design 6a.** Four routes in the design's order, the reason the fourth is
/// dead, and the privacy line under all of them.
pub fn draw_picker(ui: &mut egui::Ui, state: &mut TotpAdd) -> PickerFrame {
    let mut action = TotpAddAction::None;
    let mut rows = Vec::with_capacity(ROUTES.len());
    let mut go_manual = false;
    let mut close = egui::Rect::NOTHING;
    stage_card(ui, PICKER_WIDTH, |ui| {
        let (dismissed, at) = picker_header(ui);
        close = at;
        if dismissed {
            action = TotpAddAction::Cancel;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: BODY_PAD_X,
                right: BODY_PAD_X,
                top: BODY_PAD_TOP,
                bottom: BODY_PAD_BOTTOM,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = BODY_GAP;
                picker_subject(ui, &state.item_name);

                let mut chosen = None;
                for (index, row) in ROUTES.iter().enumerate() {
                    let (response, rect) = route_row(ui, row, index);
                    rows.push((row.route, rect));
                    if response.clicked() {
                        chosen = Some(row.route);
                    }
                }
                // **The keycaps 6a draws, answered.** See [`ROUTE_KEYS`] for
                // which keys those are, why `1` is among them and why `2`
                // still is not.
                //
                // # Only if nothing was pressed on this frame
                //
                // `draw_add_modal`'s Escape handler makes this argument for
                // the key that closes the card, and it is the same argument:
                // a control acting on this frame wins, and the key is the
                // fallback. It is not academic here. The block this replaced
                // read the rows' clicks into `chosen` and then assigned over
                // it unconditionally, so a frame carrying both a click on the
                // webcam row and an Enter took the FIRST row -- a keystroke
                // silently overruling the thing the user actually pressed.
                //
                // # And only on this stage, by construction
                //
                // `draw_stage` calls this function in its `Stage::Picker` arm
                // and nowhere else, so these keys cannot reach 6d, where a
                // bare `3` is a character the user is typing into a secret
                // field, and cannot reach 6b, where the region overlay owns
                // the keyboard. That is a stronger guarantee than the runtime
                // `state.stage != Stage::Scanning` the Escape handler needs,
                // and it is why this lives here rather than beside it.
                //
                // **6c's `↵` is still deliberately not built**; see
                // `theme::destructive_button` and the note above the primary
                // there. Binding Enter on that card could overwrite a seed
                // the user cannot recover from a focused field, and nothing
                // on this card is anywhere near that.
                if chosen.is_none() {
                    chosen = ui.input(|i| {
                        i.events.iter().find_map(|event| match event {
                            egui::Event::Key {
                                key,
                                pressed: true,
                                modifiers,
                                ..
                            } => route_for_key(&ROUTES, *key, *modifiers),
                            _ => None,
                        })
                    });
                }
                if let Some(route) = chosen {
                    if route == Route::ByHand {
                        go_manual = true;
                    } else {
                        action = action_for(route);
                    }
                }

                // The last refusal, if any. 6a has no slot for one -- it is
                // drawn in its clean state -- so this is the app's own, put
                // under the rows rather than under the privacy line so the
                // sentence saying what went wrong sits next to the rows that
                // can be pressed again.
                if let Some(refusal) = state.refusal {
                    ui.label(
                        egui::RichText::new(refusal.sentence())
                            .size(ROW_SUB_PX)
                            .color(theme::ERROR),
                    );
                }
            });
        picker_footer(ui);
    });
    if go_manual {
        state.stage = Stage::Manual;
        state.refusal = None;
    }
    PickerFrame { action, rows, close }
}

/// What the card says while the 6b overlay is up in front of it.
///
/// The words are [`crate::region_overlay`]'s own, so this window and that one
/// cannot come to describe the same gesture differently.
/// It reports nothing: the only thing that can end this stage from *this*
/// window is the way back, and the outcome that really ends it arrives from
/// the overlay through [`apply_region_outcome`]. A Cancel here would be a
/// second way to close a surface whose other window is still up.
///
/// # And that is why this stage gets no ✕, when every other one has one
///
/// 6a's header carries [`theme::modal_dismiss_mark`], `manual_header` now
/// carries the same mark, and the camera stage draws 6a's header outright --
/// so this is the one card in the flow with no corner gesture, and it is a
/// decision rather than an oversight. A ✕ is a *dismiss*, and this card
/// cannot dismiss anything: the surface the user is actually looking at is
/// `region_overlay`'s full-screen window, which is in front of this one and
/// owns both the keyboard and the scan. That is the same fact
/// [`draw_add_modal`] encodes by not answering Escape here. A mark in this
/// corner would either be invisible (covered by the overlay) or be a second
/// cancel that tears the form down while the capture window is still up --
/// and it would contradict the Escape rule one line of code away from it.
/// The way out of this stage is the overlay's own Escape, which cancels the
/// scan and captures nothing, and the way back this card draws.
///
/// **This card is also what is on screen for the moment before the overlay
/// exists**, while `region_overlay` takes and decodes its whole-screen scan,
/// and its heading is the one word for both: *"Scanning your screen"*. The
/// instruction under it is the overlay's and is a beat early -- there is
/// nothing to drag a box on yet -- and it is left that way rather than given
/// a second state, because the alternative is a card that changes its own
/// sentence twice inside a second on the way to a surface that replaces it.
///
/// **How long it is the only thing on screen has changed, and the wording has
/// not had to.** A scan that finds one code used to end the overlay before a
/// window existed, so this card was the last thing the user saw before 6c;
/// the overlay now opens for `region_overlay::REVEAL_DWELL` to show them the
/// code it found, and covers this card while it does. Either way this is what
/// is up while the scan itself runs, and *"Scanning your screen"* is the right
/// heading for exactly that.
pub fn draw_scanning(ui: &mut egui::Ui, state: &mut TotpAdd) {
    let mut go_back = false;
    card(ui, |ui| {
        ui.label(egui::RichText::new(SCANNING_HEADING).size(14.0).color(theme::INK).strong());
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(crate::region_overlay::DRAG_TITLE)
                .size(12.0)
                .color(theme::INK),
        );
        ui.add_space(2.0);
        note(ui, crate::region_overlay::DRAG_HINT, theme::TEXT_MUTED);
        ui.add_space(10.0);
        if theme::link_label(ui, OTHER_WAYS_LABEL, 11.0).clicked() {
            go_back = true;
        }
    });
    if go_back {
        state.back_to_picker();
    }
}

// ---------------------------------------------------------------------------
// The camera stage
// ---------------------------------------------------------------------------

/// How wide the card is with a camera in it.
///
/// [`PICKER_WIDTH`]'s 470, and a separate constant for `MANUAL_WIDTH`'s
/// reason: two things the same size today for different reasons are two
/// constants. The reason here is the one 6a and 6d share -- this stage is
/// reached by pressing a row on the picker, and a card that changed width
/// under the press would read as a second window opening rather than as the
/// same one a step on. It is also simply what a preview wants: a QR code
/// filling a fifth of a webcam's field of view is about forty pixels across
/// at 380 points, and about fifty at 470.
const WEBCAM_WIDTH: f32 = 470.0;

/// The preview's height, in points.
///
/// Fixed rather than derived from the frame's aspect ratio, so the card does
/// not resize the first time a picture arrives and again if the camera
/// changes mode. The picture is letterboxed inside it -- see
/// [`preview_fit`] -- which is the trade this takes: a stable card, and bars
/// at the sides of a 4:3 camera on a 16:9 rectangle.
const PREVIEW_HEIGHT: f32 = 232.0;

/// The preview's corner radius and the ground behind a picture that has not
/// arrived yet. The radius is 6a's row radius, because the preview sits where
/// the rows were.
const PREVIEW_RADIUS: u8 = ROW_RADIUS;

/// One camera's row in the device list: 6a's row padding, at the text size of
/// a row title.
const DEVICE_ROW_PAD_X: f32 = ROW_PAD_X;
/// See [`DEVICE_ROW_PAD_X`].
const DEVICE_ROW_PAD_Y: f32 = 9.0;

/// **Where a frame goes inside the preview rectangle**, preserving its aspect
/// ratio and never enlarging past the rectangle.
///
/// A pure function because it is arithmetic that fails invisibly: a preview
/// that stretched its frame would show a QR code as a rectangle of rectangles,
/// which still looks like a QR code and reads as a camera that cannot focus.
fn preview_fit(into: egui::Rect, frame: (usize, usize)) -> egui::Rect {
    let (w, h) = (frame.0 as f32, frame.1 as f32);
    if w <= 0.0 || h <= 0.0 || !into.is_positive() {
        return into;
    }
    let scale = (into.width() / w).min(into.height() / h);
    let size = egui::vec2(w * scale, h * scale);
    egui::Rect::from_center_size(into.center(), size)
}

/// **The camera stage.** A preview, what to do with it, and the way back.
///
/// `frame` is the newest picture, already taken off the session by
/// [`advance_webcam`] -- this function does not touch the camera at all, which
/// is what keeps the decision about what the session reported out of a draw
/// closure. It is consumed here: the pixels are uploaded to the texture and
/// the [`crate::webcam::Frame`] drops at the end of the call, wiping them.
pub fn draw_webcam(
    ui: &mut egui::Ui,
    state: &mut TotpAdd,
    frame: Option<crate::webcam::Frame>,
) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    let mut go_back = false;
    let mut chosen: Option<usize> = None;
    let mut back_to_list = false;
    let name = state.item_name.clone();

    // The texture is written before anything is laid out, so the rectangle
    // below paints this frame's picture rather than the previous one's.
    if let (Some(stage), Some(frame)) = (state.webcam.as_mut(), frame) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width(), frame.height()],
            frame.pixels(),
        );
        match &mut stage.texture {
            // Written into the texture that already exists rather than
            // loading a new one every frame: at thirty frames a second the
            // second shape allocates and frees a full-size GPU texture thirty
            // times a second, and each of those is another copy of a picture
            // of a seed handed to a driver.
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                stage.texture = Some(ui.ctx().load_texture(
                    "totp-add-webcam-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            }
        }
    }

    let Some(stage) = state.webcam.as_ref() else {
        // Reachable only if the stage was left between `advance_webcam` and
        // here. Nothing is drawn; the next frame is on whatever stage took
        // over.
        return action;
    };
    let showing = stage.showing();
    let several = stage.devices.len() > 1;
    let picking = stage.chosen.is_none();
    let texture = stage.texture.clone();
    let size = stage.size;
    let devices: Vec<String> = stage.devices.iter().map(|d| d.name.clone()).collect();

    stage_card(ui, WEBCAM_WIDTH, |ui| {
        let (dismissed, _) = picker_header(ui);
        if dismissed {
            action = TotpAddAction::Cancel;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: BODY_PAD_X,
                right: BODY_PAD_X,
                top: BODY_PAD_TOP,
                bottom: BODY_PAD_BOTTOM,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = BODY_GAP;
                picker_subject(ui, &name);

                if picking {
                    // **No device is open yet.** The list is the whole stage:
                    // nothing has been switched on, and the line under the
                    // heading says so.
                    ui.label(
                        theme::bold(WEBCAM_CHOOSE, ROW_TITLE_PX).color(theme::INK),
                    );
                    ui.label(
                        theme::regular(WEBCAM_CHOOSE_HINT, ROW_SUB_PX)
                            .color(theme::TEXT_FAINT),
                    );
                    for (index, camera) in devices.iter().enumerate() {
                        if device_row(ui, camera).clicked() {
                            chosen = Some(index);
                        }
                    }
                } else {
                    ui.label(theme::bold(WEBCAM_HEADING, ROW_TITLE_PX).color(theme::INK));
                    preview(ui, texture.as_ref(), size, showing);
                    ui.label(
                        theme::regular(
                            if showing { WEBCAM_HINT } else { WEBCAM_STARTING },
                            ROW_SUB_PX,
                        )
                        .color(theme::TEXT_MUTED),
                    );
                    if several && theme::link_label(ui, WEBCAM_ANOTHER_LABEL, ROW_SUB_PX).clicked()
                    {
                        back_to_list = true;
                    }
                }

                if theme::link_label(ui, OTHER_WAYS_LABEL, ROW_SUB_PX).clicked() {
                    go_back = true;
                }
            });
        // The camera's own claim, in 6a's footer band and not in a note of
        // this stage's invention. See [`WEBCAM_PRIVACY_LINE`].
        footer_band(ui, WEBCAM_PRIVACY_LINE);
    });

    // Answered after the card has been drawn, so the borrow above is over
    // before any of them mutates the stage.
    if go_back {
        state.back_to_picker();
    } else if back_to_list {
        // **Back to the list closes the device**, which is the whole content
        // of the gesture: the user is saying "not that one".
        if let Some(stage) = state.webcam.as_mut() {
            stage.session = None;
            stage.chosen = None;
            stage.texture = None;
            stage.size = None;
        }
    } else if let Some(index) = chosen {
        // Reported, not opened -- see [`TotpAddAction::UseCamera`]. A
        // dismissal from the header wins over it, because a card being
        // closed is not a card that should switch a camera on first.
        if action == TotpAddAction::None {
            action = TotpAddAction::UseCamera(index);
        }
    }
    action
}

/// The preview rectangle: the picture if there is one, and the ground it sits
/// on if there is not.
///
/// The ground is painted whether or not there is a picture, so a camera whose
/// frame does not fill the rectangle is letterboxed against the card's own
/// tint rather than against whatever was underneath.
fn preview(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    size: Option<(usize, usize)>,
    showing: bool,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PREVIEW_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(PREVIEW_RADIUS), theme::CANVAS);
    let (Some(texture), Some(size), true) = (texture, size, showing) else {
        return;
    };
    painter.image(
        texture.id(),
        preview_fit(rect, size),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// One camera's row in the device list.
///
/// 6a's row chrome without its tile: the list is a list of names, and a mark
/// repeated down it would be four copies of the same camera glyph saying
/// nothing about which is which.
fn device_row(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let width = ui.available_width();
    let title = lay_out(
        ui,
        name,
        (width - ROW_STROKE * 2.0 - DEVICE_ROW_PAD_X * 2.0).max(1.0),
        face(ROW_TITLE_PX, theme::SEMIBOLD, theme::INK),
    );
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ROW_STROKE * 2.0 + DEVICE_ROW_PAD_Y * 2.0 + title.size().y),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    let hovered = response.hovered();
    painter.rect_filled(
        rect,
        CornerRadius::same(ROW_RADIUS),
        if hovered { theme::CANVAS } else { theme::CARD },
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(ROW_RADIUS),
        egui::Stroke::new(ROW_STROKE, theme::HAIRLINE),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(
            rect.left() + ROW_STROKE + DEVICE_ROW_PAD_X,
            rect.top() + ROW_STROKE + DEVICE_ROW_PAD_Y,
        ),
        title,
        theme::INK,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}


// ---------------------------------------------------------------------------
// Design 6c's numbers, all of them lifted out of the CSS under `id="6c"`
// ---------------------------------------------------------------------------

/// The gap between 6c's three blocks -- the live code, the field table and
/// whatever follows them (`gap: 16px` on the card's body).
const CONFIRM_GAP: f32 = 16.0;

/// The live-code panel: `padding: 14px 16px; border: 1px solid #b8c7ea;
/// border-radius: 10px; background: #eef2fc`.
const CODE_PANEL_PAD_X: i8 = 16;
/// See [`CODE_PANEL_PAD_X`].
const CODE_PANEL_PAD_Y: i8 = 14;
/// See [`CODE_PANEL_PAD_X`].
const CODE_PANEL_RADIUS: u8 = 10;

/// `Code now`, as 6c sets it: `font-size: 11px; font-weight: 700;
/// letter-spacing: 0.1em; text-transform: uppercase`.
const CODE_LABEL_PX: f32 = 11.0;
/// See [`CODE_LABEL_PX`]. The design's em, which [`theme::letterspaced`] wants
/// in points.
const CODE_LABEL_TRACKING: f32 = 0.1;

/// The code itself: monospace at `font-size: 26px; font-weight: 700;
/// letter-spacing: 0.14em`.
///
/// **Six points larger than the detail pane's live code**, which is 6c's whole
/// argument: this one is being read off the screen and typed into a comparison
/// against the site, once, before anything is saved.
const CODE_PX: f32 = 26.0;
/// See [`CODE_PX`].
const CODE_TRACKING: f32 = 0.14;
/// The gap between the label and the code (`gap: 4px`).
const CODE_LABEL_GAP: f32 = 4.0;

/// The countdown track beside it: `width: 96px; height: 4px;
/// border-radius: 2px`, `#b8c7ea` under `#1b3fa0`.
const COUNTDOWN_WIDTH: f32 = 96.0;
/// See [`COUNTDOWN_WIDTH`].
const COUNTDOWN_HEIGHT: f32 = 4.0;
/// See [`COUNTDOWN_WIDTH`].
const COUNTDOWN_RADIUS: u8 = 2;
/// The two lines the track sits between: `font-size: 12px; color: #14307a`,
/// stacked at `gap: 7px` and right-aligned (`align-items: flex-end`).
const PANEL_SIDE_PX: f32 = 12.0;
/// See [`PANEL_SIDE_PX`].
const PANEL_SIDE_GAP: f32 = 7.0;

/// §6d's code, which is smaller than §6c's: `font-size: 22px` against 26.
///
/// Not an inconsistency to be smoothed over. §6c is the card that has just
/// captured something off the screen and whose whole job is *"is this the
/// code you are looking at?"* -- the code is its subject. §6d is the card you
/// are TYPING a secret into: the code is the proof the secret works, sitting
/// under the field that produced it, and at 26 it would outweigh the thing
/// being entered.
const CODE_PX_TYPED: f32 = 22.0;

/// §6d's strip: `padding: 12px 14px`, a point tighter than §6c's `14px 16px`
/// because it holds one row rather than a two-line column.
const TYPED_PANEL_PAD_X: i8 = 14;
const TYPED_PANEL_PAD_Y: i8 = 12;

/// §6d's `gap: 14px`, between the code and the track and again between the
/// track and the seconds.
const TYPED_PANEL_GAP: f32 = 14.0;


/// The field table: `border: 1px solid #eae7e7; border-radius: 10px;
/// overflow: hidden`.
const TABLE_RADIUS: u8 = 10;
/// One row of it: `padding: 11px 14px; gap: 14px`, divided from the next by
/// `1px solid #f3f2f2`.
const FIELD_PAD_X: f32 = 14.0;
/// See [`FIELD_PAD_X`].
const FIELD_PAD_Y: f32 = 11.0;
/// See [`FIELD_PAD_X`].
const FIELD_GAP: f32 = 14.0;
/// The label column's `width: 92px`, at `font-size: 12px; color: #7d7979`.
const FIELD_LABEL_W: f32 = 92.0;
/// See [`FIELD_LABEL_W`].
const FIELD_LABEL_PX: f32 = 12.0;
/// The values beside it (`font-size: 13px`), and the tracking 6c gives the
/// masked secret alone (`letter-spacing: 0.08em`).
const FIELD_VALUE_PX: f32 = 13.0;
/// See [`FIELD_VALUE_PX`].
const SECRET_TRACKING: f32 = 0.08;
/// The control at the end of the secret's row: `font-size: 12px;
/// font-weight: 600; color: #14307a`.
const FIELD_ACTION_PX: f32 = 12.0;

/// One parameter chip: monospace `font-size: 11px` on `#f3f2f2` at
/// `border-radius: 5px; padding: 2px 7px`, with `gap: 7px` between them.
const PARAM_CHIP_PX: f32 = 11.0;
/// See [`PARAM_CHIP_PX`].
const PARAM_CHIP_PAD_X: f32 = 7.0;
/// See [`PARAM_CHIP_PX`].
const PARAM_CHIP_PAD_Y: f32 = 2.0;
/// See [`PARAM_CHIP_PX`].
const PARAM_CHIP_RADIUS: u8 = 5;
/// See [`PARAM_CHIP_PX`].
const PARAM_CHIP_GAP: f32 = 7.0;

/// **Design 6c**, and the half every later route shares.
///
/// Split out of [`draw_add_form`] so the scanned routes of tasks 4--7 paint
/// the same card from the same function rather than a second one that drifts.
///
/// **What of 6c is here and what is deliberately not.** 6c is drawn as a card
/// of its own, and this app folds it into the bottom of 6d's -- the two are
/// one surface here, reached by typing as well as by scanning. So the three
/// blocks 6c puts inside its body are all below, in its order and with its
/// measurements; its card *chrome* is 6d's, and two of its controls are not
/// drawn at all because this app has no action behind them: the `Change`
/// beside "Saving to" (the record was chosen before this modal opened, and the
/// card already names it at the top) and the greyed `Edit` beside the
/// parameters (6d's own digit and period controls are the live version of
/// that, a few rows up, and they already go dead when a pasted URI has spoken
/// for them).
///
/// # 6c's heading and its table belong to the SCANNED path only
///
/// Fusing 6c into this card was the right call for a payload that was
/// **scanned**: the whole point of a confirmation there is checking the issuer
/// and the account against what the site displayed, because nobody read the
/// QR -- a decoder did, and the user has no other view of what it said.
///
/// None of that is true of a secret the user **typed**. The rows would be
/// their own keystrokes read back to them a few pixels below the box they are
/// still in, which is not a check: there is no second source to disagree with.
/// Design 6d draws the field, the validity line, the two parameter controls
/// and the live-code panel, and stops -- so on the typed path, so does this.
///
/// **The live-code panel stays on both**, and is the one part of 6c that was
/// never really 6c's to begin with: it is in 6d's own mockup, and it is the
/// only check on this card that has a second source to be checked against --
/// the code the site is showing at that moment. Comparing those two is what
/// tells a user their seed is right; the rows never could.
///
/// The caution band for a record that already has a code is **not** part of
/// this and is not drawn here anyway (see [`draw_add_form`]). It is a warning
/// about what saving will destroy, not a field restating an input, and 6d's
/// mockup simply has no record behind it with a code to lose.
fn draw_confirmation(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd, now_unix: u64) {
    // Read out before `state` is lent to the table below.
    let scanned = state.scanned;

    if scanned {
        ui.label(egui::RichText::new(CONFIRM_HEADING).size(12.0).color(theme::INK).strong());
        ui.add_space(6.0);
    }

    // The live code first: it is what the user is here to compare, and a
    // confirmation that buries it under four label rows is a confirmation
    // nobody makes. On the typed path it is the only thing here.
    if let Some(code) = code_at(auth, now_unix) {
        draw_code_panel(ui, auth, &code, now_unix, scanned);
        // The gap is the one BETWEEN the panel and the table, so it goes
        // wherever the table does. Left in on the typed path it would be a
        // stripe of nothing above the footer.
        if scanned {
            ui.add_space(CONFIRM_GAP);
        }
    }

    if scanned {
        draw_field_table(ui, auth, state);
    }
}

/// **The live-code strip, in the design's two shapes.**
///
/// §6c, the card that has just read a code off the screen, draws a two-line
/// block: a `CODE NOW` eyebrow over a 26px code, with `refreshes in 22 s`, the
/// track and `Matches what the site shows?` stacked against the right edge.
/// Its job is to be checked against a screen, so it says so in words.
///
/// §6d, the card you are typing a secret INTO, draws one row: the code, a
/// `flex: 1` track and `22 s`. Nothing else. It is not asking a question --
/// it is showing that what has been typed produces codes, under the field
/// that produced them.
///
/// The owner, on the typed card wearing §6c's block: "blue strip also should
/// only have code and progress bar with seconds - nothing else". Both shapes
/// are drawn here rather than in two functions because they are one strip --
/// same wash, same edge, same radius, same code, same countdown off the same
/// `countdown_fraction` -- and the parts that differ are exactly the parts
/// the design draws differently.
fn draw_code_panel(
    ui: &mut egui::Ui,
    auth: &OtpAuth,
    code: &str,
    now_unix: u64,
    scanned: bool,
) {
    let (pad_x, pad_y) = if scanned {
        (CODE_PANEL_PAD_X, CODE_PANEL_PAD_Y)
    } else {
        (TYPED_PANEL_PAD_X, TYPED_PANEL_PAD_Y)
    };
    egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
        .corner_radius(CornerRadius::same(CODE_PANEL_RADIUS))
        .inner_margin(egui::Margin::symmetric(pad_x, pad_y))
        .show(ui, |ui| {
            if !scanned {
                typed_code_row(ui, auth, code, now_unix);
                return;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Uppercased here rather than stored uppercased, because
                    // `CODE_ROW_LABEL` is the design's own "Code now" and the
                    // capitals are `text-transform`, a rendering of it.
                    ui.label(theme::letterspaced(
                        &CODE_ROW_LABEL.to_uppercase(),
                        CODE_LABEL_PX,
                        theme::BOLD,
                        CODE_LABEL_PX * CODE_LABEL_TRACKING,
                        theme::BLUE_DEEP,
                    ));
                    ui.add_space(CODE_LABEL_GAP);
                    // §6c declares `line-height: 1` on its code, and the
                    // ascent is this app's reading of that: the tightest box
                    // the digits fit in. Without it the eyebrow above and the
                    // code are separated by the face's descender band rather
                    // than by the design's 4-point gap.
                    ui.label(
                        theme::letterspaced_mono_in(
                            &grouped_code(code),
                            CODE_PX,
                            CODE_PX * CODE_TRACKING,
                            theme::BLUE_DEEP,
                            code_ascent(ui, CODE_PX),
                        ),
                    );
                });
                // The right-hand stack takes what the code left and hangs off
                // the panel's right edge, which is 6c's `align-items:
                // flex-end` on a column with `flex: 1` to its left.
                let rest = ui.available_width().max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(rest, 0.0),
                    egui::Layout::top_down(egui::Align::RIGHT),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            egui::RichText::new(refresh_line(seconds_left(auth, now_unix)))
                                .size(PANEL_SIDE_PX)
                                .color(theme::BLUE_DEEP),
                        );
                        ui.add_space(PANEL_SIDE_GAP);
                        draw_countdown(ui, countdown_fraction(auth, now_unix));
                        ui.add_space(PANEL_SIDE_GAP);
                        ui.label(
                            egui::RichText::new(MATCH_QUESTION)
                                .size(PANEL_SIDE_PX)
                                .color(theme::BLUE_DEEP),
                        );
                    },
                );
            });
        });
}

/// The line box the live code is set in: the monospace face's ascent at
/// `size`. See [`theme::ascent_of`] for the rule and the report behind it.
fn code_ascent(ui: &egui::Ui, size: f32) -> f32 {
    theme::ascent_of(ui.ctx(), &egui::FontId::new(size, egui::FontFamily::Monospace))
}

/// §6d's one row: the code, a track that takes what is left, and the seconds.
///
/// The track is `flex: 1` -- §6d's own -- and not §6c's fixed 96, which is why
/// it is measured here instead of calling [`draw_countdown`]: the two cards
/// draw the same bar at two widths, and a bar that ran to 96 in a row built
/// to fill the strip would leave a hole between it and the seconds.
fn typed_code_row(ui: &mut egui::Ui, auth: &OtpAuth, code: &str, now_unix: u64) {
    let seconds = seconds_line(seconds_left(auth, now_unix));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // **The line box is the face's ascent**, so egui's vertical centring
        // of this label against the track and the seconds beside it centres
        // the DIGITS rather than a row box that reserves a descender band a
        // six-digit code has no use for. The owner: "6 digit code is not
        // centered either". See `theme::ascent_of`.
        ui.label(
            theme::letterspaced_mono_in(
                &grouped_code(code),
                CODE_PX_TYPED,
                CODE_PX_TYPED * CODE_TRACKING,
                theme::BLUE_DEEP,
                code_ascent(ui, CODE_PX_TYPED),
            ),
        );
        // **The seconds are measured before the track is drawn**, because the
        // track is what gives way. Laid out in source order the bar would
        // take `available_width` and push `22 s` off the strip's right edge --
        // the same defect the detail header's controls are laid out
        // right-to-left to avoid.
        let tail = ui.painter().layout_no_wrap(
            seconds.clone(),
            egui::FontId::proportional(PANEL_SIDE_PX),
            theme::BLUE_DEEP,
        );
        let track = (ui.available_width() - tail.size().x - TYPED_PANEL_GAP * 2.0).max(0.0);
        ui.add_space(TYPED_PANEL_GAP);
        draw_countdown_at(ui, countdown_fraction(auth, now_unix), track);
        ui.add_space(TYPED_PANEL_GAP);
        ui.label(egui::RichText::new(seconds).size(PANEL_SIDE_PX).color(theme::BLUE_DEEP));
    });
}

/// 6c's countdown track: a 96 by 4 rail of [`theme::BLUE_EDGE`] with
/// `fraction` of it filled in [`theme::BLUE`] from the left.
fn draw_countdown(ui: &mut egui::Ui, fraction: f32) {
    draw_countdown_at(ui, fraction, COUNTDOWN_WIDTH);
}

/// [`draw_countdown`] at a width the caller measured -- §6d's `flex: 1`.
fn draw_countdown_at(ui: &mut egui::Ui, fraction: f32, width: f32) {
    let (track, _) = ui.allocate_exact_size(
        egui::vec2(width, COUNTDOWN_HEIGHT),
        egui::Sense::hover(),
    );
    let radius = CornerRadius::same(COUNTDOWN_RADIUS);
    ui.painter().rect_filled(track, radius, theme::BLUE_EDGE);
    let filled = egui::Rect::from_min_size(
        track.min,
        egui::vec2(track.width() * fraction.clamp(0.0, 1.0), track.height()),
    );
    if filled.width() > 0.0 {
        ui.painter().rect_filled(filled, radius, theme::BLUE);
    }
}

/// **6c's field table**: a bordered, rounded block whose rows are divided by
/// hairlines rather than spaced apart.
fn draw_field_table(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
        .corner_radius(CornerRadius::same(TABLE_RADIUS))
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            ui.set_min_width(width);

            table_row(
                ui,
                ISSUER_ROW_LABEL,
                |ui| {
                    ui.label(
                        theme::semibold(
                            auth.issuer.as_deref().unwrap_or("\u{2014}"),
                            FIELD_VALUE_PX,
                        )
                        .color(theme::INK),
                    );
                },
                |_| {},
            );
            table_seam(ui);

            table_row(
                ui,
                ACCOUNT_ROW_LABEL,
                |ui| {
                    ui.label(
                        egui::RichText::new(auth.account.as_deref().unwrap_or("\u{2014}"))
                            .size(FIELD_VALUE_PX)
                            .color(theme::INK),
                    );
                },
                |_| {},
            );
            table_seam(ui);

            let mut toggle = false;
            table_row(
                ui,
                SECRET_ROW_LABEL,
                |ui| {
                    let shown = if state.revealed {
                        auth.secret.to_string()
                    } else {
                        masked(&auth.secret)
                    };
                    // The tracking is 6c's and it is the mask's, not the
                    // seed's: `••••••••` set solid is one grey block, and the
                    // groups of four exist to say how much seed there is.
                    ui.label(theme::letterspaced_mono(
                        &shown,
                        FIELD_VALUE_PX,
                        FIELD_VALUE_PX * SECRET_TRACKING,
                        theme::INK,
                    ));
                },
                |ui| {
                    toggle = row_action(ui, if state.revealed { HIDE_LABEL } else { REVEAL_LABEL })
                        .clicked();
                },
            );
            if toggle {
                state.revealed = !state.revealed;
            }
            table_seam(ui);

            table_row(
                ui,
                PARAMETERS_ROW_LABEL,
                |ui| {
                    // `flex-wrap: wrap` on the design's chip row: on a modal
                    // narrower than 6c's 470px card the four chips have to be
                    // allowed onto a second line rather than pushing the row
                    // wider than the card holding it.
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(PARAM_CHIP_GAP, PARAM_CHIP_GAP);
                        for chip in parameter_chips(auth) {
                            param_chip(ui, &chip);
                        }
                    });
                },
                |_| {},
            );
        });
}

/// One row of [`draw_field_table`]: the label column, the value, and an
/// optional control hung off the right-hand edge.
fn table_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl FnOnce(&mut egui::Ui),
    trailing: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(FIELD_PAD_Y);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(FIELD_PAD_X);
        ui.allocate_ui(egui::vec2(FIELD_LABEL_W, 0.0), |ui| {
            ui.label(
                egui::RichText::new(label).size(FIELD_LABEL_PX).color(theme::TEXT_FAINT),
            );
        });
        ui.add_space(FIELD_GAP);
        value(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(FIELD_PAD_X);
            trailing(ui);
        });
    });
    ui.add_space(FIELD_PAD_Y);
}

/// The `1px solid #f3f2f2` between two rows. Full-bleed, because 6c's rows are
/// divided rather than spaced: the line runs the whole width of the table and
/// is clipped by its rounded border.
fn table_seam(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::CANVAS);
}

/// 6c's row-end control -- `Reveal`, and `Hide` once it has been pressed.
///
/// Laid out by hand rather than taken from [`theme::link_label`] because 6c
/// sets this one two shades deeper and a weight heavier than this app's links
/// (`font-weight: 600; color: #14307a`, against a link's 400 in `#1b3fa0`):
/// it sits inside a table of values rather than in running text, and the
/// design leans on weight to separate it from the row it ends.
fn row_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::new(FIELD_ACTION_PX, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        theme::BLUE_DEEP,
    );
    theme::link_galley(ui, galley)
}

/// One parameter chip, as 6c draws them.
fn param_chip(ui: &mut egui::Ui, text: &str) {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::new(PARAM_CHIP_PX, egui::FontFamily::Monospace),
        theme::TEXT_SECONDARY,
    );
    let size = galley.size() + egui::vec2(PARAM_CHIP_PAD_X * 2.0, PARAM_CHIP_PAD_Y * 2.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(PARAM_CHIP_RADIUS),
        theme::CANVAS,
    );
    ui.painter().galley(
        rect.min + egui::vec2(PARAM_CHIP_PAD_X, PARAM_CHIP_PAD_Y),
        galley,
        theme::TEXT_SECONDARY,
    );
}

/// [`draw_add_form`] over a dimmed scrim, centred, for `vault_window::mod` to
/// call from its frame closure.
///
/// Built exactly the way `record_ui::draw_export_modal` is -- which is exactly
/// the way `folder_modal::draw_folder_edit_modal` is -- because a second modal
/// built differently is two modals that dim, layer and swallow clicks two
/// ways. Its `Id`s differ from theirs so no two can share egui state.
pub fn draw_add_modal(
    ctx: &egui::Context,
    state: &mut TotpAdd,
    now_unix: u64,
) -> TotpAddAction {
    egui::Area::new(egui::Id::new("totp-add-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                egui::Color32::from_black_alpha(90),
            );
        });

    let action = theme::movable_modal(ctx, egui::Area::new(egui::Id::new("totp-add-modal")))
        .show(ctx, |ui| {
            // **The handle goes on before the stage does, and it is the
            // stage's own header.** Registered first because a drag-sensing
            // strip laid over the header's ✕ swallows the click that dismisses
            // this card -- `theme::modal_drag_handle` records the measurement
            // -- and the height is [`stage_header_height`] because this is the
            // one card in the app whose header changes with what it is showing.
            theme::modal_drag_handle(ui, stage_header_height(state.stage));
            ui.set_max_width(stage_width(state.stage));
            draw_stage(ui, state, now_unix)
        })
        .inner;

    // **Escape closes it, and this is the only key the modal answers.**
    //
    // Every other overlay in this app takes Escape -- the delete
    // confirmation, the icon picker, the folder editor -- and this one took
    // nothing. Before 6a moved the dismiss into the header its only way out
    // was a button at the BOTTOM of a card tall enough to hold four route
    // rows, a refusal line and the privacy note, inside a centre-anchored
    // `Area` that does not scroll: on a short window that button sat off
    // screen, the scrim swallowed clicks, and no key closed it. A surface a
    // user cannot leave is indistinguishable from one that has stopped
    // responding, and it was reported as a hang alongside a real one.
    //
    // **Not while the overlay is up.** Escape belongs to 6b then: it is a
    // full-screen always-on-top window with the keyboard, so this branch
    // could only fire on a stray frame after the overlay had gone -- and it
    // would throw the form away on the keystroke the user meant as "stop
    // scanning". The overlay's own Escape cancels the scan and captures
    // nothing, which is the whole of what that key means at that moment.
    //
    // Answered LAST, after `draw_stage` has reported, so a control that acts
    // on this frame wins. Nothing in the card is bound to Escape today; a
    // control added later that is would be silently overridden here, and this
    // is the order that makes that the safe direction rather than the
    // dangerous one.
    if matches!(action, TotpAddAction::None)
        && state.stage != Stage::Scanning
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        return TotpAddAction::Cancel;
    }
    action
}

/// How wide the card is at each stage.
///
/// **6d is 470, and not the [`MODAL_WIDTH`] it was.** `#6a` and `#6d` both
/// declare `width: 470px`, which they would: they are one card a step apart,
/// and a modal that narrowed by ninety points when its third row was pressed
/// would read as a different window opening. The 380 the by-hand form used to
/// take was the record composer's narrow column, borrowed before either panel
/// had been read off the design, and at that width 6d's own two `flex: 1`
/// parameter columns are 167 apiece and 6c's four parameter chips wrap.
///
/// [`Stage::Scanning`] keeps [`MODAL_WIDTH`]: it is four short lines with no
/// design panel of its own asking it to be wider, and widening it as a side
/// effect of this would make the card jump on the way to 6b and back.
/// [`Stage::Webcam`] is the other way about -- see [`WEBCAM_WIDTH`]: it is a
/// card with a picture in it, reached from the picker with no second window
/// in between, so it takes the picker's width for the reason 6d does.
fn stage_width(stage: Stage) -> f32 {
    match stage {
        Stage::Picker => PICKER_WIDTH,
        Stage::Manual => MANUAL_WIDTH,
        Stage::Scanning => MODAL_WIDTH,
        Stage::Webcam => WEBCAM_WIDTH,
    }
}

/// How tall the card's grabbable header is at each stage: what
/// [`theme::modal_drag_handle`] is handed so the strip the user drags by lies
/// over the band they can see and nowhere else.
///
/// [`stage_width`]'s sibling, and it exists for the same reason -- this is the
/// one card in the app that is four cards, and the header it wears is not the
/// same one at each of them.
///
/// **Every arm is a FLOOR and not a measurement.** The manual stage's band is
/// as tall as its padding around the taller of its title and its mark
/// ([`manual_header`]), and the title at 14px is the taller of those on every
/// face this app ships -- so the mark is the number to build the floor from.
/// Erring short costs a few points at the bottom of a band that are not
/// grabbable; erring long puts a drag-sensing strip over the first control of
/// the body, and a drag-sensing strip over a control eats that control's
/// clicks.
///
/// [`Stage::Scanning`] is the odd one: it is not a [`stage_card`] at all but
/// the plain [`card`], a 12-point margin round a 14px heading, and its handle
/// is that heading's line. It is also the stage the 6b overlay owns the
/// keyboard during -- but the overlay is a window of its own and the pointer
/// is inside it, so nothing here can be dragged while it is up, and this
/// needs no special case for it.
fn stage_header_height(stage: Stage) -> f32 {
    match stage {
        // Both draw `picker_header`, whose band is `HEADER_HEIGHT`.
        Stage::Picker | Stage::Webcam => HEADER_HEIGHT,
        Stage::Manual => MANUAL_HEADER_PAD_Y * 2.0 + MANUAL_HEADER_MARK + RULE,
        Stage::Scanning => SCANNING_HEADER_HEIGHT,
    }
}

/// [`card`]'s 12-point top margin plus the ~18 that [`draw_scanning`]'s 14px
/// bold heading occupies: the grabbable strip of the scanning stage.
///
/// A point short of the line under it -- that heading is followed by
/// `add_space(6.0)` -- for [`stage_header_height`]'s reason.
const SCANNING_HEADER_HEIGHT: f32 = 30.0;

/// **The one place that decides which half of this surface is on screen**, so
/// no caller has to know there are four.
///
/// # The camera is advanced BEFORE the match, and that is load-bearing
///
/// [`advance_webcam`] can change the stage: a code read on this frame moves
/// the form to 6c, and a camera that has just refused moves it back to the
/// picker. Running it first means the frame that learns either of those
/// paints the stage it learned about, rather than painting a preview of a
/// camera that has already been closed. It is `mod.rs`'s reason for driving
/// the 6b overlay above the form rather than below it, one level in.
///
/// # Why the clock here is an `Instant` and not `now_unix`
///
/// `now_unix` is a **wall** clock, and the only thing measured against a
/// camera is an elapsed duration -- how long it has been open without
/// sending a picture. A wall clock moved backwards by a time sync would make
/// that duration negative and the grace period never expire; moved forwards,
/// it would expire instantly on a camera that was working. So this reads a
/// monotonic clock, exactly where `region_overlay::show` reads one for the
/// same kind of question, and hands it to a function that takes it as an
/// argument so every boundary is drivable from a test.
pub fn draw_stage(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let frame = (state.stage == Stage::Webcam)
        .then(|| advance_webcam(state, std::time::Instant::now()))
        .flatten();
    match state.stage {
        Stage::Picker => draw_picker(ui, state).action,
        Stage::Scanning => {
            draw_scanning(ui, state);
            TotpAddAction::None
        }
        Stage::Webcam => draw_webcam(ui, state, frame),
        Stage::Manual => draw_add_form(ui, state, now_unix),
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

    #[test]
    fn the_secret_is_masked_in_groups_of_four_and_by_length() {
        assert_eq!(masked("JBSWY3DPEHPK3PXP"), "\u{2022}\u{2022}\u{2022}\u{2022} \
             \u{2022}\u{2022}\u{2022}\u{2022} \u{2022}\u{2022}\u{2022}\u{2022} \
             \u{2022}\u{2022}\u{2022}\u{2022}".replace("             ", ""));
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
            REPLACE_WARNING,
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

    /// Every rect the form actually INKS, flattened out of the shape tree.
    ///
    /// Fully transparent rects are dropped: egui emits the scroll area's own
    /// clip and content rects as shapes with no fill and no stroke, and one of
    /// them runs the full width of the card's inside -- close enough to the
    /// body's content edge to be mistaken for it by anything measuring widths.
    ///
    /// The scroll bar is the one piece of this card egui draws rather than
    /// this module, so it cannot be checked by reading a constant back: the
    /// only honest question is where the pixels landed.
    fn painted_rects(short_window: bool) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    let inked = rect.fill.a() > 0
                        || (rect.stroke.width > 0.0 && rect.stroke.color.a() > 0);
                    if inked {
                        out.push(rect.rect);
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

        let ctx = egui::Context::default();
        // 420 is short enough that the body overflows `room` and the bar is
        // genuinely needed; 900 is taller than the card, so nothing scrolls.
        let height = if short_window { 420.0 } else { 900.0 };
        let base = move || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, height),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(base(), |_ui| {});

        let mut state = TotpAdd::opening("id-1", "Git Host", true);
        state.typed = Zeroizing::new("JBSWY3DPEHPK3PXP".to_string());

        // **With the pointer inside the card.** A floating bar egui has faded
        // out is still emitted as a shape, at a fraction of its width and at
        // zero alpha, so a frame drawn with the pointer away would measure a
        // ghost. Frames are run until the fade-in has settled; the width
        // assertion below is what makes a bar measured mid-animation fail
        // loudly rather than pass against a smaller number.
        let hovered = move || egui::RawInput {
            events: vec![egui::Event::PointerMoved(egui::pos2(300.0, 150.0))],
            ..base()
        };
        let mut shapes = Vec::new();
        for _ in 0..60 {
            let output = ctx.run_ui(hovered(), |ui| {
                draw_add_form(ui, &mut state, 1_700_000_000);
            });
            shapes = output.shapes;
        }

        let mut rects = Vec::new();
        for clipped in &shapes {
            walk(&clipped.shape, &mut rects);
        }
        assert!(!rects.is_empty(), "the form painted nothing at all");
        rects
    }

    /// The rects that are the width of a scroll bar and tall enough to be one.
    fn scrollbar_rects(rects: &[egui::Rect]) -> Vec<egui::Rect> {
        rects
            .iter()
            .copied()
            .filter(|rect| {
                (rect.width() - theme::SCROLLBAR_WIDTH).abs() < 0.01 && rect.height() > 50.0
            })
            .collect()
    }

    /// The right edge of the widest thing drawn INSIDE the card, i.e. the
    /// body's own content edge rather than the card's.
    fn body_content_right(rects: &[egui::Rect]) -> f32 {
        rects
            .iter()
            .filter(|rect| rect.width() > 300.0 && rect.width() < MANUAL_WIDTH - 1.0)
            .map(|rect| rect.right())
            .fold(f32::MIN, f32::max)
    }

    /// **"also 2 lines on the right side", on a card the design draws with
    /// one.**
    ///
    /// The second line was egui's own. The body scrolls -- see
    /// [`MODAL_BREATHING`] for why it must -- and a floating bar is pinned
    /// flush to the right edge of the area it scrolls, which on this card is
    /// the inside of [`stage_card`]'s stroke. Measured before the fix, the bar
    /// painted across x = 468.5..469.0 against a border at 469.5: two rules a
    /// point apart, which is exactly what was reported.
    ///
    /// Pinned as a GAP rather than as a coordinate. What matters is that the
    /// bar clears the border it was crowding AND clears the body's own right
    /// edge, so that the complaint cannot be answered by sliding the defect
    /// off the border and onto the content. See [`MANUAL_SCROLLBAR_INSET`].
    #[test]
    fn the_cards_scroll_bar_does_not_read_as_a_second_right_edge() {
        let rects = painted_rects(true);
        let bars = scrollbar_rects(&rects);
        assert!(
            !bars.is_empty(),
            "no scroll bar was painted on a window too short for the body, so either the \
             fixture stopped overflowing or the scroll itself was lost"
        );

        // The card is drawn from the origin of this headless frame at
        // `MANUAL_WIDTH`, so its stroke runs down the inside of that edge.
        let card_inner_right = MANUAL_WIDTH - CARD_STROKE;
        let content_right = body_content_right(&rects);
        for bar in &bars {
            assert!(
                (card_inner_right - bar.right() - MANUAL_SCROLLBAR_INSET).abs() < 0.01,
                "the bar at {bar:?} does not stop {MANUAL_SCROLLBAR_INSET} short of the card's \
                 edge at {card_inner_right}; flush against it is the reported defect"
            );
            assert!(
                bar.left() >= content_right,
                "the bar at {bar:?} reaches back over the body, whose right edge is at \
                 {content_right}"
            );
        }
    }

    /// **The card that fits paints no bar at all, and no narrower body.**
    ///
    /// Nothing is reserved for the bar -- it floats inside padding the body
    /// already had -- so the visibility mode stays egui's default and a card
    /// with nothing to scroll shows one edge and no rule beside it. The other
    /// half of that claim is that the body is the same width either way, which
    /// is what a reserved lane would have cost; both are measured here.
    #[test]
    fn a_card_that_fits_paints_no_bar_and_keeps_the_bodys_width() {
        let tall = painted_rects(false);
        assert!(
            scrollbar_rects(&tall).is_empty(),
            "a bar was painted down a card with nothing to scroll: {:?}",
            scrollbar_rects(&tall)
        );

        let short = painted_rects(true);
        assert!(
            (body_content_right(&short) - body_content_right(&tall)).abs() < 0.01,
            "the body's right edge moved when the bar appeared: {} while scrolling against {} \
             while not",
            body_content_right(&short),
            body_content_right(&tall)
        );
    }

    #[test]
    fn the_form_paints_the_replace_warning_only_when_there_is_a_code_to_destroy() {
        let mut fresh = TotpAdd::opening("id-1", "Git Host", false);
        let before = paint(|ui| {
            draw_add_form(ui, &mut fresh, 1_700_000_000);
        });
        assert!(before.has(HEADING), "control: the form drew nothing recognisable");
        assert!(before.has(SAVE_LABEL), "the submit button is not on the form");
        assert!(
            !before.has("cannot be recovered"),
            "the replace warning was painted for an item with no code: {:?}",
            before.0
        );

        let mut existing = TotpAdd::opening("id-1", "Git Host", true);
        let after = paint(|ui| {
            draw_add_form(ui, &mut existing, 1_700_000_000);
        });
        assert!(
            after.has(REPLACE_WARNING),
            "the item already had a code and the warning was NOT painted: {:?}",
            after.0
        );
        assert!(after.has(REPLACE_LABEL), "the destructive button did not say so on its face");
        assert!(!after.has(SAVE_LABEL), "both button faces were painted at once");
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

        assert!(painted.has(CONFIRM_HEADING), "the confirmation heading is missing");
        let expected = code_at(&auth(UNUSUAL), BOUNDARY).expect("decodes");
        assert!(
            painted.has(grouped_code(&expected).as_str()),
            "the live code is not on screen: {:?}",
            painted.0
        );
        assert!(painted.has("refreshes in 60 s"), "the countdown is not on screen: {:?}", painted.0);
        assert!(painted.has(MATCH_QUESTION), "the question the code exists to answer is missing");
        // The parameters SPELLED OUT: the 8/60/SHA-256 case is exactly the one
        // a confirmation exists to catch.
        assert!(painted.has("SHA256"), "the algorithm is not spelled out: {:?}", painted.0);
        assert!(painted.has("8 digits"), "the digit count is not spelled out");
        assert!(painted.has("60 s"), "the period is not spelled out");
        assert!(painted.has("Git Host"), "the issuer is not shown");
        assert!(painted.has("anovak"), "the account is not shown");
    }

    /// **Every measurement the confirmation paints with is design 6c's**,
    /// quoted here beside the CSS it came from, in the order the card stacks.
    #[test]
    fn the_confirmations_numbers_are_the_designs_own() {
        // The body's `gap: 16px` between its blocks.
        assert_eq!(CONFIRM_GAP, 16.0);

        // The live-code panel: `padding: 14px 16px; border-radius: 10px`.
        assert_eq!((CODE_PANEL_PAD_X, CODE_PANEL_PAD_Y), (16, 14));
        assert_eq!(CODE_PANEL_RADIUS, 10);
        // `Code now` at `11px/700`, `letter-spacing: 0.1em`, over the code at
        // `26px/700`, `letter-spacing: 0.14em`, `gap: 4px` between them.
        assert_eq!((CODE_LABEL_PX, CODE_LABEL_TRACKING), (11.0, 0.1));
        assert_eq!((CODE_PX, CODE_TRACKING), (26.0, 0.14));
        assert_eq!(CODE_LABEL_GAP, 4.0);
        // The track: `width: 96px; height: 4px; border-radius: 2px`, between
        // two `12px` lines at `gap: 7px`.
        assert_eq!((COUNTDOWN_WIDTH, COUNTDOWN_HEIGHT), (96.0, 4.0));
        assert_eq!(COUNTDOWN_RADIUS, 2);
        assert_eq!((PANEL_SIDE_PX, PANEL_SIDE_GAP), (12.0, 7.0));

        // The field table: `border-radius: 10px`, rows at `padding: 11px 14px;
        // gap: 14px`, a `92px` label column at `12px` and values at `13px`.
        assert_eq!(TABLE_RADIUS, 10);
        assert_eq!((FIELD_PAD_X, FIELD_PAD_Y, FIELD_GAP), (14.0, 11.0, 14.0));
        assert_eq!((FIELD_LABEL_W, FIELD_LABEL_PX), (92.0, 12.0));
        assert_eq!(FIELD_VALUE_PX, 13.0);
        // The masked secret alone is tracked (`letter-spacing: 0.08em`), and
        // the control that unmasks it is `12px/600`.
        assert_eq!(SECRET_TRACKING, 0.08);
        assert_eq!(FIELD_ACTION_PX, 12.0);

        // The parameter chips: monospace `11px`, `padding: 2px 7px`,
        // `border-radius: 5px`, `gap: 7px`.
        assert_eq!(PARAM_CHIP_PX, 11.0);
        assert_eq!((PARAM_CHIP_PAD_X, PARAM_CHIP_PAD_Y), (7.0, 2.0));
        assert_eq!(PARAM_CHIP_RADIUS, 5);
        assert_eq!(PARAM_CHIP_GAP, 7.0);
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
        assert!(masked_frame.has(REVEAL_LABEL), "there is no way to reveal it");
        assert!(masked_frame.has("\u{2022}\u{2022}\u{2022}\u{2022}"), "the masked row is missing");

        state.revealed = true;
        let revealed = paint(|ui| {
            draw_add_form(ui, &mut state, BOUNDARY);
        });
        assert!(
            revealed.has("JBSWY3DPEHPK3PXP"),
            "Reveal showed nothing, so the masked assertion above proves nothing: {:?}",
            revealed.0
        );
        assert!(revealed.has(HIDE_LABEL), "there is no way back to masked");
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
            !painted.has(CONFIRM_HEADING),
            "a refused field still painted a confirmation to save from"
        );
    }

    #[test]
    fn the_modal_paints_the_form_it_wraps() {
        // The modal is the entry point `vault_window::mod` calls; a scrim that
        // drew nothing inside it would still satisfy every test above.
        let mut state = TotpAdd::opening("id-1", "Git Host", true);
        state.stage = Stage::Manual;
        state.typed = Zeroizing::new("JBSWY3DPEHPK3PXP".to_string());
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        // TWO frames of the modal, and the SECOND is the one read. An
        // `egui::Area` has no size until it has been laid out once, so the
        // frame that introduces it paints nothing -- measured, not foreseen:
        // the first draft of this test read an empty shape list and would have
        // passed against a modal that drew nothing at all had the assertions
        // been written the other way round.
        let _ = ctx.run_ui(input(), |_ui| {
            let _ = draw_add_modal(&ctx, &mut state, BOUNDARY);
        });
        let output = ctx.run_ui(input(), |_ui| {
            let _ = draw_add_modal(&ctx, &mut state, BOUNDARY);
        });
        let mut painted = Painted(Vec::new());
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(painted.has(HEADING), "the modal painted no form: {:?}", painted.0);
        assert!(painted.has(REPLACE_WARNING), "the modal dropped the warning");
        // The confirmation half, asked for by the part of it a TYPED card
        // draws: §6d's live-code strip. 6c's heading and its *"Matches what the
        // site shows?"* are the SCANNED card's now (see `draw_code_panel`), and
        // this state is a typed one -- either would be asserting the old
        // fusion rather than that the modal drew the form.
        //
        // The needle is the code itself, recomputed here from the same seed
        // and the same clock the modal was given, so this cannot be satisfied
        // by a strip that drew SOME code: it is the one this secret produces
        // at this instant.
        let expected = grouped_code(
            &code_at(&auth("otpauth://totp/Git%20Host?secret=JBSWY3DPEHPK3PXP"), BOUNDARY)
                .expect("the fixture's seed decodes"),
        );
        assert!(
            painted.has(&expected),
            "the modal dropped the confirmation: {:?}",
            painted.0
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
        let order: Vec<Route> = ROUTES.iter().map(|r| r.route).collect();
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
        assert_eq!(ROUTES[0].title, "Scan the code on my screen");
        assert_eq!(
            ROUTES[0].subtitle,
            "Deskwarden looks for it in every window \u{b7} drag a box if it misses"
        );
        assert!(
            !ROUTES[0].title.contains("region") && !ROUTES[0].subtitle.starts_with("Drag"),
            "the scan row still leads with the box the user no longer has to draw"
        );
        assert!(
            ROUTES[0].subtitle.contains("drag a box"),
            "the scan row no longer names the fallback the overlay still is"
        );
        assert_eq!(ROUTES[1].title, "Open an image file");
        assert_eq!(ROUTES[2].title, "Enter the secret by hand");
        assert_eq!(ROUTES[3].title, "Use a webcam");
        for row in &ROUTES {
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
                ROUTES[1].subtitle.contains(named),
                "the image row does not name {named}, which the dialog offers: {}",
                ROUTES[1].subtitle
            );
            assert!(
                spec.contains(extension),
                "the image row names {named} and the dialog's filter does not offer it"
            );
        }
        // The control on the loop above: a format neither of them names.
        // Without it the assertions pass for a subtitle that names
        // everything.
        assert!(!ROUTES[1].subtitle.contains("TIFF"), "{}", ROUTES[1].subtitle);
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
        let dead: Vec<Route> = ROUTES.iter().filter(|r| !r.enabled).map(|r| r.route).collect();
        assert!(dead.is_empty(), "a route is drawn dead: {dead:?}");
        let live: Vec<Route> = ROUTES.iter().filter(|r| r.enabled).map(|r| r.route).collect();
        assert_eq!(
            live,
            vec![Route::ScanRegion, Route::ImageFile, Route::ByHand, Route::Webcam],
            "the four live routes are not design 6a's four, in its order"
        );
        // The webcam row's own line, which is a promise `crate::webcam`
        // keeps and `PRIVACY.md` repeats.
        let webcam = ROUTES.iter().find(|r| r.route == Route::Webcam).expect("6a's fourth row");
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
        assert_eq!(DEFERRED_REASON, "Not in this version");
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
            PRIVACY_LINE,
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
        assert!(region.sentence().contains(NO_CODE_ADVICE));
        assert!(image.sentence().contains(NO_CODE_ADVICE));
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
            rgba.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect();
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
    // The picker as a surface -- pressed, not called
    // -----------------------------------------------------------------

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// **The click harness.**
    ///
    /// `record_ui` shipped unreachable for a day because every test it had
    /// called its draw function directly. So nothing below calls
    /// [`action_for`]: each test lays the picker out, finds where a row was
    /// really painted, presses that point, and reads back what the surface
    /// reported.
    struct Picker {
        ctx: egui::Context,
    }

    /// One rectangle the surface really painted, carrying the four things
    /// design 6a specifies for one: where, what fill, what border and what
    /// radius.
    ///
    /// The text collector above cannot see any of these, and the whole of
    /// this design pass is boxes: a card, three bands, four rows, four tiles
    /// and a keycap. Read off the shapes rather than off the constants, so a
    /// row that took the right colour from the right constant and then was
    /// painted somewhere else still reds.
    #[derive(Clone, Copy, Debug)]
    struct PaintedRect {
        rect: egui::Rect,
        fill: egui::Color32,
        stroke: egui::Color32,
        stroke_width: f32,
        radius: u8,
    }

    fn collect_rects(shape: &egui::Shape, out: &mut Vec<PaintedRect>) {
        match shape {
            egui::Shape::Rect(rect) => out.push(PaintedRect {
                rect: rect.rect,
                fill: rect.fill,
                stroke: rect.stroke.color,
                stroke_width: rect.stroke.width,
                // Every corner on this card is one radius, so the first is
                // the whole answer -- and a shape that squared one corner
                // (the footer's tint does) is identified by its fill instead.
                radius: rect.corner_radius.nw,
            }),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rects(shape, out);
                }
            }
            _ => {}
        }
    }

    /// One text shape the surface really painted: what it says, the rectangle
    /// the galley occupies, and **how many rows it wrapped into**.
    ///
    /// [`Painted`] above keeps the strings and throws the geometry away, which
    /// is right for "is this sentence on screen" and blind to both halves of
    /// the defect design 6d's line under the field was reported with: how many
    /// lines the sentence took, and whether it landed on top of the box above
    /// it. Neither is answerable from a `Vec<String>`, and both are answerable
    /// from a `TextShape`, which carries its position and its galley.
    #[derive(Clone, Debug)]
    struct PaintedText {
        text: String,
        rect: egui::Rect,
        rows: usize,
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<PaintedText>) {
        match shape {
            egui::Shape::Text(text) => out.push(PaintedText {
                text: text.galley.text().to_owned(),
                rect: egui::Rect::from_min_size(text.pos, text.galley.size()),
                rows: text.galley.rows.len(),
            }),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_texts(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Every straight segment the surface painted.
    ///
    /// This is how `theme::modal_dismiss_mark` is found, and it is found
    /// rather than computed on purpose: a test that re-derived the mark's
    /// centre from [`theme::MODAL_CLOSE_INSET`] would agree with a card that
    /// had stopped drawing one. The ✕ is the only thing on these cards drawn
    /// as a pair of `line_segment`s -- [`paint_svg`]'s marks are
    /// `Shape::line` paths, the bands are rects and the copy is galleys -- so
    /// two crossing segments [`theme::CLOSE_MARK_SPAN`] across is an
    /// unambiguous signature.
    fn collect_segments(shape: &egui::Shape, out: &mut Vec<[egui::Pos2; 2]>) {
        match shape {
            egui::Shape::LineSegment { points, .. } => out.push(*points),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_segments(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Where the dismiss ✕ was painted: the point its two
    /// [`theme::CLOSE_MARK_SPAN`] diagonals cross at.
    ///
    /// Found off the painted strokes, not computed from
    /// [`theme::MODAL_CLOSE_INSET`], so a card that stopped drawing a mark
    /// fails here instead of quietly agreeing with an arithmetic restatement
    /// of where one would have gone. A free function rather than a method
    /// because two harnesses ask it: the stage on its own, and the whole
    /// modal with its drag handle laid over the header.
    fn close_mark_in(segments: &[[egui::Pos2; 2]]) -> egui::Pos2 {
        let arms: Vec<[egui::Pos2; 2]> = segments
            .iter()
            .copied()
            .filter(|[a, b]| {
                ((a.x - b.x).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.5
                    && ((a.y - b.y).abs() - theme::CLOSE_MARK_SPAN).abs() < 0.5
            })
            .collect();
        assert_eq!(
            arms.len(),
            2,
            "a dismiss ✕ is two crossing {}-point diagonals and {} were painted: {segments:?}",
            theme::CLOSE_MARK_SPAN,
            arms.len()
        );
        let centre =
            |[a, b]: [egui::Pos2; 2]| egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let (first, second) = (centre(arms[0]), centre(arms[1]));
        assert!(
            (first - second).length() < 0.5,
            "the two diagonals do not cross at one point: {first:?} and {second:?}"
        );
        first
    }

    /// Whether two rectangles are the same one, within half a point.
    fn same_rect(a: egui::Rect, b: egui::Rect) -> bool {
        (a.min - b.min).length() < 0.5 && (a.max - b.max).length() < 0.5
    }

    /// One frame of [`Picker`].
    struct PickerRun {
        action: TotpAddAction,
        rows: Vec<(Route, egui::Rect)>,
        close: egui::Rect,
        painted: Painted,
        rects: Vec<PaintedRect>,
    }

    impl PickerRun {
        fn row(&self, route: Route) -> egui::Rect {
            self.rows
                .iter()
                .find(|(r, _)| *r == route)
                .unwrap_or_else(|| panic!("{route:?} was not drawn at all"))
                .1
        }

        /// Every rectangle painted exactly at `at`. A filled box and its
        /// border are two shapes at one rect, so this answers both.
        fn at(&self, at: egui::Rect) -> Vec<PaintedRect> {
            self.rects.iter().copied().filter(|r| same_rect(r.rect, at)).collect()
        }

        /// Every rectangle painted wholly inside `outer` and smaller than it
        /// -- a row's tile and its keycap, found without knowing where the
        /// row put them.
        fn inside(&self, outer: egui::Rect) -> Vec<PaintedRect> {
            self.rects
                .iter()
                .copied()
                .filter(|r| outer.contains_rect(r.rect) && !same_rect(r.rect, outer))
                .collect()
        }
    }

    impl Picker {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            crate::theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Picker { ctx }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    // Room for [`PICKER_WIDTH`] and its shadow. Widened with
                    // the 6a design pass: the card was 380 and is 470.
                    egui::vec2(560.0, 900.0),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&self, state: &mut TotpAdd, events: Vec<egui::Event>) -> PickerRun {
            let mut reported: Option<PickerFrame> = None;
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                ui.set_max_width(stage_width(Stage::Picker));
                reported = Some(draw_picker(ui, state));
            });
            let reported = reported.expect("run_ui runs the closure once");
            let mut painted = Painted(Vec::new());
            let mut rects = Vec::new();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut painted);
                collect_rects(&clipped.shape, &mut rects);
            }
            assert!(
                !painted.0.is_empty(),
                "the picker painted no text at all, so every assertion over this list would \
                 pass against nothing"
            );
            assert!(
                !rects.is_empty(),
                "the picker painted no rectangles at all, so every assertion over that list \
                 would pass against nothing either"
            );
            PickerRun {
                action: reported.action,
                rows: reported.rows,
                close: reported.close,
                painted,
                rects,
            }
        }

        fn idle(&self, state: &mut TotpAdd) -> PickerRun {
            self.frame(state, Vec::new())
        }

        fn click(&self, state: &mut TotpAdd, at: egui::Pos2) -> PickerRun {
            self.frame(state, click_at(at))
        }
    }

    /// **Design 6a, on screen.**
    #[test]
    fn the_picker_paints_all_four_routes_and_the_privacy_line() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let frame = picker.idle(&mut state);

        // **6a's own header title, and not 6d's.** Updated deliberately with
        // the design pass that gave this card 6a's header band: the picker
        // used to paint `HEADING` ("Add a TOTP", which is the by-hand form's)
        // over a second heading reading "How to add it", and 6a draws one
        // title and it is this one. See [`PICKER_TITLE`].
        assert!(frame.painted.has(PICKER_TITLE), "the card has no heading");
        // **Equality and not `has`**, updated deliberately with the 6d design
        // pass: [`HEADING`] is now 6d's own "Enter the secret", and 6a's third
        // row is titled "Enter the secret by hand", so the substring test this
        // used to be would find 6d's heading inside a row title that has
        // always been there. What it is asking is unchanged -- that the picker
        // paints one title and it is 6a's -- and this is the only spelling of
        // it that still asks it.
        assert!(
            !frame.painted.0.iter().any(|t| t == HEADING),
            "the picker paints 6d's heading as well as its own: {:?}",
            frame.painted.0
        );
        assert!(
            frame.painted.has(ADDING_TO_LABEL),
            "the picker does not say what it is adding to"
        );
        assert!(frame.painted.has("Git Host"), "the item it is adding to is not named");
        for row in &ROUTES {
            assert!(frame.painted.has(row.title), "route {:?} is not on screen", row.route);
            assert!(
                frame.painted.has(row.subtitle),
                "route {:?} has no line under it on screen",
                row.route
            );
        }
        assert!(
            frame.painted.has(PRIVACY_LINE),
            "the privacy line is not on the card: {:?}",
            frame.painted.0
        );
        // Nothing on this card says a route is deferred, because none is.
        assert!(
            !frame.painted.has(DEFERRED_REASON),
            "a row is drawn dead: {:?}",
            frame.painted.0
        );
        // And nothing of 6d is on screen yet: the picker is a picker.
        assert!(!frame.painted.has(CONFIRM_HEADING));
        assert_eq!(frame.rows.len(), ROUTES.len(), "a route was drawn without a hit area");
    }

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

    /// **The card is 6a's card**: 470 wide, radius 12, a `#d7d3d3` hairline
    /// round it, a white header band ruled off in `#eae7e7`, and a `#fbfaf9`
    /// footer.
    #[test]
    fn the_picker_card_is_the_designs_own_box() {
        let mut state = TotpAdd::opening("id-1", "Git Host \u{b7} anovak", false);
        let picker = Picker::new();
        let frame = picker.idle(&mut state);

        let card = frame
            .rects
            .iter()
            .find(|r| r.radius == CARD_RADIUS && r.fill == theme::CARD)
            .copied()
            .unwrap_or_else(|| panic!("no 12px-radius white card was painted: {:?}", frame.rects));
        assert!(
            (card.rect.width() - PICKER_WIDTH).abs() <= 1.0,
            "the card is {} wide and 6a's is {PICKER_WIDTH}",
            card.rect.width()
        );
        assert_eq!(card.stroke, theme::BORDER_STRONG, "6a's card is bordered #d7d3d3");
        assert!((card.stroke_width - CARD_STROKE).abs() < f32::EPSILON);

        // The footer's tint, which is the one band on this card that is not
        // the card's own white, and which squares its top corners so the
        // three bands read as one card.
        let footer = frame
            .rects
            .iter()
            .find(|r| r.fill == theme::CARD_TINT)
            .copied()
            .expect("6a's footer band is #fbfaf9 and none was painted");
        assert_eq!(footer.radius, 0, "the footer's TOP corners are square");
        assert!(
            (footer.rect.bottom() - card.rect.bottom()).abs() <= 2.0,
            "the footer band does not reach the bottom of the card"
        );

        // Two hairline rules, one under the header and one over the footer.
        let rules: Vec<_> = frame
            .rects
            .iter()
            .filter(|r| r.fill == theme::HAIRLINE && (r.rect.height() - RULE).abs() < 0.5)
            .collect();
        assert_eq!(rules.len(), 2, "6a rules the header off and the footer on: {rules:?}");
    }

    /// **Every route row is 6a's row**, and the first one is 6a's blue one.
    #[test]
    fn every_route_row_is_the_designs_own_box() {
        let mut state = TotpAdd::opening("id-1", "Git Host \u{b7} anovak", false);
        let picker = Picker::new();
        let frame = picker.idle(&mut state);

        for (index, (route, rect)) in frame.rows.iter().enumerate() {
            // 440 = 6a's 470 card, less its border and the body's 14px
            // margins.
            assert!(
                (rect.width() - 440.0).abs() <= 1.0,
                "{route:?} is {} wide and 6a's row is 440",
                rect.width()
            );
            let painted = frame.at(*rect);
            let fill = painted
                .iter()
                .find(|r| r.fill != egui::Color32::TRANSPARENT)
                .unwrap_or_else(|| panic!("{route:?} painted no fill: {painted:?}"));
            let edge = painted
                .iter()
                .find(|r| r.stroke_width > 0.0)
                .unwrap_or_else(|| panic!("{route:?} painted no border: {painted:?}"));
            assert_eq!(fill.radius, ROW_RADIUS, "{route:?} is not 6a's 10px radius");
            assert!(
                (edge.stroke_width - 1.0).abs() < f32::EPSILON,
                "{route:?}'s border is not 6a's 1px"
            );

            // The tile: `width: 34px` inside a 1px border is 36 on screen.
            let tile = frame
                .inside(*rect)
                .into_iter()
                .find(|r| (r.rect.width() - 36.0).abs() < 0.5 && r.radius == TILE_RADIUS)
                .unwrap_or_else(|| panic!("{route:?} has no 36px 8px-radius icon tile"));
            assert!((tile.rect.height() - 36.0).abs() < 0.5, "{route:?}'s tile is not square");

            if index == DEFAULT_ROW {
                assert_eq!(fill.fill, theme::BLUE_WASH, "6a's first row is #eef2fc");
                assert_eq!(edge.stroke, theme::BLUE, "6a's first row is bordered #1b3fa0");
                assert_eq!(tile.fill, theme::CARD, "the selected row's tile is white");
            } else {
                assert_eq!(fill.fill, theme::CARD, "6a's other rows carry no fill of their own");
                assert_eq!(edge.stroke, theme::HAIRLINE, "6a's other rows are bordered #eae7e7");
                assert_eq!(tile.fill, theme::WINDOW_BG, "an unselected tile is #f7f6f5");
            }
        }

        // **All four rows are 62 tall** -- 6a's own number, which is 12px of
        // padding either side of the 36px tile plus the 1px border. This
        // used to assert three at 62 and a fourth that was TALLER, because
        // the webcam row was drawn dead and its two-line reason wrapped;
        // now that every route is live, four one-line rows is what the
        // design draws and what this holds. A subtitle long enough to wrap
        // would grow its row and red this, which is the right way round:
        // `route_row` measures itself, so a row that got taller did so
        // because its copy grew rather than because a number was edited.
        for (route, rect) in &frame.rows {
            assert!(
                (rect.height() - 62.0).abs() <= 1.0,
                "{route:?} is {} tall and 6a's row is 62",
                rect.height()
            );
        }
    }

    /// **6a's two right-hand affordances**: a filled ↵ keycap on the default
    /// row, and a bare ordinal on each of the others.
    #[test]
    fn the_default_row_wears_the_keycap_and_the_rest_wear_their_ordinal() {
        let mut state = TotpAdd::opening("id-1", "Git Host \u{b7} anovak", false);
        let picker = Picker::new();
        let frame = picker.idle(&mut state);

        let default = frame.row(ROUTES[DEFAULT_ROW].route);
        let keycap = frame
            .inside(default)
            .into_iter()
            .find(|r| r.fill == theme::BLUE && r.radius == 5)
            .unwrap_or_else(|| panic!("the default row has no #1b3fa0 5px-radius keycap"));
        assert!(
            (keycap.rect.height() - theme::CHIP_HEIGHT).abs() < 0.5,
            "the keycap is {} tall; 6a's `padding: 3px` around a 10px line is 18",
            keycap.rect.height()
        );
        assert!(
            keycap.rect.right() < default.right() && keycap.rect.right() > default.right() - 20.0,
            "the keycap is not against the row's right padding"
        );

        // The ordinals are painted as text and the default row's is not: 6a
        // sets 2, 3 and 4 in monospace and gives the first row the cap
        // instead. `has` is a substring test, so "1" is looked for on its own
        // line rather than inside another string.
        for ordinal in ["2", "3", "4"] {
            assert!(
                frame.painted.0.iter().any(|t| t == ordinal),
                "6a's ordinal {ordinal} is not on the card: {:?}",
                frame.painted.0
            );
        }
        assert!(
            !frame.painted.0.iter().any(|t| t == "1"),
            "the default row painted an ordinal as well as its keycap"
        );
        // And no other row painted a keycap.
        for (route, rect) in &frame.rows {
            if *route == ROUTES[DEFAULT_ROW].route {
                continue;
            }
            assert!(
                !frame.inside(*rect).iter().any(|r| r.fill == theme::BLUE),
                "{route:?} painted a filled blue box, so the assertion above says nothing \
                 about which row is the default one"
            );
        }
    }

    /// One bare keystroke, for the keycap tests below.
    fn tap(key: egui::Key) -> Vec<egui::Event> {
        taps(key, egui::Modifiers::default())
    }

    /// One keystroke with whatever modifiers are held.
    fn taps(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]
    }

    /// **Every keycap 6a draws is a key that really answers, and each answers
    /// its OWN row.**
    ///
    /// The defect, reported by the owner against 0.15.21: *"Enter and 1,2,3,4
    /// not workong - supposed to be hjotkeys"*. Enter was bound; nothing else
    /// was. Written as a table rather than four assertions so that a binding
    /// that answered the right key with the wrong row -- the failure mode of
    /// a hand-written `match` over four near-identical arms -- reds with the
    /// pair printed.
    #[test]
    fn each_keycap_takes_the_row_it_is_drawn_on() {
        for (key, route) in [
            (egui::Key::Enter, Route::ScanRegion),
            (egui::Key::Num1, Route::ScanRegion),
            (egui::Key::Num3, Route::ByHand),
            (egui::Key::Num4, Route::Webcam),
        ] {
            let mut state = TotpAdd::opening("id-1", "Git Host", false);
            let picker = Picker::new();
            assert_eq!(
                picker.idle(&mut state).action,
                TotpAddAction::None,
                "{key:?}: the picker asks for something with nothing pressed, so the assertion \
                 below would pass against a picker that reported every frame"
            );
            let pressed = picker.frame(&mut state, tap(key));
            assert_eq!(
                pressed.action,
                action_for(route),
                "{key:?} did not take the {route:?} row 6a hangs it off"
            );
            // `ByHand` reports nothing and moves the stage instead, so the
            // assertion above is vacuous for it on its own.
            if route == Route::ByHand {
                assert_eq!(
                    state.stage,
                    Stage::Manual,
                    "{key:?} reported nothing AND did not open 6d, so it did nothing at all"
                );
            }
        }
    }

    /// **A held modifier means the keystroke belongs to somebody else.**
    ///
    /// This window binds CTRL+K, CTRL+L, CTRL+N and CTRL+SHIFT+2; the last of
    /// those is the chord that opened this very card. A picker that answered
    /// a bare digit's row on any chord containing it would take a route on a
    /// keystroke aimed past it.
    #[test]
    fn a_route_key_under_a_modifier_is_not_this_surfaces_business() {
        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
            egui::Modifiers::SHIFT,
            egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT),
        ] {
            for key in [egui::Key::Num1, egui::Key::Num3, egui::Key::Num4] {
                let mut state = TotpAdd::opening("id-1", "Git Host", false);
                let picker = Picker::new();
                let pressed = picker.frame(&mut state, taps(key, modifiers));
                assert_eq!(
                    pressed.action,
                    TotpAddAction::None,
                    "{key:?} under {modifiers:?} took a route"
                );
                assert_eq!(
                    state.stage,
                    Stage::Picker,
                    "{key:?} under {modifiers:?} moved the stage"
                );
            }
        }
    }

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
        let mut deferred = ROUTES;
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
                route_for_key(&ROUTES, key, egui::Modifiers::default()).is_some(),
                "{key:?} answers nothing even on the live table"
            );
        }
    }

    /// **The route keys do not exist on any stage but the picker.**
    ///
    /// 6d is a form with a secret field in it: a bare `3` there is a
    /// character the user is typing, and taking them off the card mid-seed
    /// would lose it. 6b owns the keyboard outright while the overlay is up.
    /// Both are guaranteed by construction -- `draw_stage` calls
    /// `draw_picker` in one arm only -- and this is what holds that
    /// construction in place.
    #[test]
    fn the_route_keys_are_dead_on_every_other_stage() {
        for stage in [Stage::Manual, Stage::Scanning] {
            for key in [egui::Key::Enter, egui::Key::Num1, egui::Key::Num3, egui::Key::Num4] {
                let mut state = TotpAdd::opening("id-1", "Git Host", false);
                state.stage = stage;
                let ctx = egui::Context::default();
                let _ = ctx.run_ui(Picker::input(Vec::new()), |_ui| {});
                crate::theme::apply(&ctx);
                let mut action = TotpAddAction::None;
                let _ = ctx.run_ui(Picker::input(tap(key)), |ui| {
                    action = draw_stage(ui, &mut state, BOUNDARY);
                });
                assert_eq!(
                    state.stage, stage,
                    "{key:?} moved the form off {stage:?}, where it is not a shortcut"
                );
                assert!(
                    !matches!(
                        action,
                        TotpAddAction::ScanRegion
                            | TotpAddAction::OpenImage
                            | TotpAddAction::OpenWebcam
                    ),
                    "{key:?} took a route from {stage:?}: {action:?}"
                );
            }
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
        for (row, _) in ROUTES.iter().enumerate() {
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
        assert_eq!(ROUTES[1].route, Route::ImageFile);
    }

    /// **A click on this frame beats a key on this frame.**
    ///
    /// The block this replaced read every row's click into `chosen` and then
    /// assigned over it unconditionally when Enter was down, so a frame
    /// carrying both took the FIRST row -- a keystroke silently overruling
    /// the thing the user actually pressed. `draw_add_modal`'s Escape handler
    /// makes the same argument for the key that closes the card.
    #[test]
    fn a_pressed_row_beats_a_key_on_the_same_frame() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let at = picker.idle(&mut state).row(Route::Webcam).center();
        let mut events = click_at(at);
        events.extend(tap(egui::Key::Enter));
        assert_eq!(
            picker.frame(&mut state, events).action,
            action_for(Route::Webcam),
            "Enter overruled the row the user actually pressed"
        );
    }

    /// **The ↵ the default row advertises is a key that really answers.**
    ///
    /// Pressed rather than called, and with the idle frame as the control:
    /// a picker that reported `ScanRegion` every frame would satisfy an
    /// assertion written one way round.
    #[test]
    fn enter_takes_the_designs_default_route() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        assert_eq!(
            picker.idle(&mut state).action,
            TotpAddAction::None,
            "the picker asks for something with nothing pressed"
        );

        let pressed = picker.frame(
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(
            pressed.action,
            action_for(ROUTES[DEFAULT_ROW].route),
            "Enter did not take the route 6a hangs its ↵ keycap off"
        );
    }

    /// **The ✕ in 6a's header is the way out of this card**, and it is the
    /// only one: 6a draws no Cancel button, and the one this used to carry
    /// was removed with the design pass.
    #[test]
    fn the_header_cross_closes_the_picker() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        assert!(
            laid_out.close.width() > 1.0,
            "the header painted no ✕ at all, so pressing it below proves nothing"
        );
        assert!(
            !laid_out.painted.has("Cancel"),
            "the picker still carries the Cancel button 6a does not draw: {:?}",
            laid_out.painted.0
        );

        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        let at = laid_out.close.center();
        assert_eq!(
            picker.click(&mut state, at).action,
            TotpAddAction::Cancel,
            "pressing 6a's ✕ did not close the modal"
        );
    }

    /// **Pressing "Scan a region of my screen" is what opens design 6b.**
    ///
    /// The whole feature hangs off this one press: the capture, the overlay,
    /// the decoder and the parser are reachable only through it. So it is
    /// pressed here rather than called -- and every other row is pressed too,
    /// because a surface where *everything* reported `ScanRegion` would
    /// satisfy an assertion written one way round.
    #[test]
    fn pressing_the_scan_row_asks_for_the_region_overlay_and_no_other_row_does() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        let scan_at = laid_out.row(Route::ScanRegion).center();

        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let _ = picker.idle(&mut state);
        assert_eq!(
            picker.click(&mut state, scan_at).action,
            TotpAddAction::ScanRegion,
            "clicking 6a's first row did not ask for the region overlay, so the whole scan \
             chain is unreachable from the user interface"
        );

        for route in [Route::ImageFile, Route::ByHand, Route::Webcam] {
            let mut state = TotpAdd::opening("id-1", "Git Host", false);
            let picker = Picker::new();
            let laid_out = picker.idle(&mut state);
            let at = laid_out.row(route).center();
            assert_ne!(
                picker.click(&mut state, at).action,
                TotpAddAction::ScanRegion,
                "{route:?} also asked for the region overlay, so the assertion above says \
                 nothing about which row was pressed"
            );
        }
    }

    /// **Pressing "Open an image file" is what opens the shell's dialog.**
    #[test]
    fn pressing_the_image_row_asks_for_the_file_dialog_and_no_other_row_does() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        let image_at = laid_out.row(Route::ImageFile).center();

        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let _ = picker.idle(&mut state);
        assert_eq!(
            picker.click(&mut state, image_at).action,
            TotpAddAction::OpenImage,
            "clicking 6a's second row did not ask for the file dialog"
        );

        for route in [Route::ScanRegion, Route::ByHand, Route::Webcam] {
            let mut state = TotpAdd::opening("id-1", "Git Host", false);
            let picker = Picker::new();
            let laid_out = picker.idle(&mut state);
            let at = laid_out.row(route).center();
            assert_ne!(
                picker.click(&mut state, at).action,
                TotpAddAction::OpenImage,
                "{route:?} also asked for the file dialog"
            );
        }
    }

    /// **Pressing "Enter the secret by hand" paints 6d's field.**
    ///
    /// Read back off the surface rather than off `state.stage`: a stage that
    /// changed and a form that never drew would satisfy the second and not
    /// the first.
    #[test]
    fn pressing_the_by_hand_row_paints_the_field() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        let at = laid_out.row(Route::ByHand).center();
        assert!(
            !laid_out.painted.has(SECRET_HINT),
            "the field was already on screen, so the assertion below proves nothing"
        );

        let after = picker.click(&mut state, at);
        assert_eq!(
            after.action,
            TotpAddAction::None,
            "the by-hand row asked the caller for something to do"
        );
        assert_eq!(state.stage, Stage::Manual);

        // The frame after, through the same entry point the window calls.
        let painted = paint(|ui| {
            draw_stage(ui, &mut state, 1_700_000_000);
        });
        assert!(
            painted.has(SECRET_HINT),
            "the by-hand route reported nothing and painted nothing: {:?}",
            painted.0
        );
    }

    /// **A refusal is painted on the picker, as a sentence.**
    #[test]
    fn the_picker_paints_the_last_refusal_and_drops_it_when_a_route_is_chosen() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let clean = picker.idle(&mut state);
        assert!(!clean.painted.has("No code in that region"));

        state.refusal = Some(PickerRefusal::NoCode(CodeSource::Region));
        let refused = picker.idle(&mut state);
        assert!(
            refused.painted.has(&PickerRefusal::NoCode(CodeSource::Region).sentence()),
            "the refusal is not on the card: {:?}",
            refused.painted.0
        );

        // Choosing a route again clears it: a stale sentence under a fresh
        // attempt is a sentence about the wrong attempt.
        let at = refused.row(Route::ByHand).center();
        let _ = picker.click(&mut state, at);
        assert_eq!(state.refusal, None);
    }

    /// **The modal opens on 6a**, which is what makes the other three routes
    /// reachable at all.
    #[test]
    fn the_modal_opens_on_the_picker_and_not_on_the_by_hand_form() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        assert_eq!(state.stage, Stage::Picker, "a fresh form does not open on the picker");
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        // Two frames, and the SECOND is read: an `egui::Area` has no size
        // until it has been laid out once.
        let _ = ctx.run_ui(input(), |_ui| {
            let _ = draw_add_modal(&ctx, &mut state, BOUNDARY);
        });
        let output = ctx.run_ui(input(), |_ui| {
            let _ = draw_add_modal(&ctx, &mut state, BOUNDARY);
        });
        let mut painted = Painted(Vec::new());
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(painted.has(PICKER_TITLE), "the modal did not open on 6a: {:?}", painted.0);
        assert!(painted.has(PRIVACY_LINE), "the modal dropped the privacy line");
        assert!(painted.has(ROUTES[0].title));
        assert!(!painted.has(SECRET_HINT), "the modal opened straight into the by-hand form");
    }

    /// **What the card says while the overlay is up**, in the overlay's own
    /// words.
    #[test]
    fn the_scanning_card_borrows_the_overlays_own_instruction() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.stage = Stage::Scanning;
        let painted = paint(|ui| {
            draw_stage(ui, &mut state, BOUNDARY);
        });
        assert!(painted.has(SCANNING_HEADING));
        assert!(
            painted.has(crate::region_overlay::DRAG_TITLE),
            "the card and the overlay describe the same gesture differently: {:?}",
            painted.0
        );
        assert!(painted.has(crate::region_overlay::DRAG_HINT));
        assert!(painted.has(OTHER_WAYS_LABEL), "there is no way out of the scanning stage");
    }

    /// **A decoded region reaches 6c's card, through the entry point the
    /// window really calls.**
    #[test]
    fn a_scanned_code_is_confirmed_on_the_same_card_a_typed_one_is() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.stage = Stage::Scanning;
        apply_region_outcome(&mut state, Outcome::Decoded(Zeroizing::new(UNUSUAL.to_string())));

        let painted = paint(|ui| {
            draw_stage(ui, &mut state, BOUNDARY);
        });
        assert!(painted.has(CONFIRM_HEADING), "a scan did not reach 6c: {:?}", painted.0);
        let expected = code_at(&auth(UNUSUAL), BOUNDARY).expect("decodes");
        assert!(
            painted.has(grouped_code(&expected).as_str()),
            "the live code a scanned seed produces is not on screen"
        );
        assert!(painted.has(MATCH_QUESTION));

        // **Masked, exactly as it is for a typed one -- and this is the
        // assertion the first draft of this surface failed.**
        //
        // The decoded URI goes into `typed`, and 6d's field is a `TextEdit`
        // that paints what it holds. Left that way the seed was on screen in
        // the clear, two rows above a masked-secret row that was then pure
        // decoration. So a scanned payload gets 6c's "Code read" row instead
        // of the field, and the seed appears nowhere until Reveal is pressed.
        assert!(
            !painted.has("JBSWY3DPEHPK3PXP"),
            "a scanned seed was painted in the clear: {:?}",
            painted.0
        );
        assert!(!painted.has("secret="), "the raw URI was painted: {:?}", painted.0);
        assert!(painted.has("\u{2022}\u{2022}\u{2022}\u{2022}"));
        assert!(
            painted.has(CODE_READ_LABEL) && painted.has(CODE_READ_KIND),
            "the scanned route drew neither the field nor the row that replaces it"
        );
        assert!(
            !painted.has(SECRET_HINT),
            "the editable field was drawn for a scanned payload"
        );

        // Control, in the same frame shape: Reveal still works, so the
        // absence above is masking rather than a card that shows nothing.
        state.revealed = true;
        let revealed = paint(|ui| {
            draw_stage(ui, &mut state, BOUNDARY);
        });
        assert!(
            revealed.has("JBSWY3DPEHPK3PXP"),
            "Reveal showed nothing, so the masked assertions above prove nothing"
        );
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
        assert!(painted.has(SECRET_HINT), "the by-hand field is gone: {:?}", painted.0);
        assert!(!painted.has(CODE_READ_LABEL), "the by-hand route drew the scanned row");
    }

    /// **6c's heading and its field table are the SCANNED card's**; a typed
    /// secret gets design 6d's list and stops at the live-code panel.
    ///
    /// The rows restate the user's own keystrokes a few pixels below the box
    /// they are still in, which is not a check -- a check needs a second
    /// source to disagree with, and a typed seed has none. A scanned one does:
    /// nobody read that QR, a decoder did, and Issuer and Account are the only
    /// view of what it said.
    ///
    /// **Both halves are asserted in one test on purpose.** Either alone would
    /// pass against a card that drew the table for nobody, or for everybody,
    /// and this is the pair that has to hold -- the fusion of 6c into 6d was a
    /// deliberate decision for the scanned path and stays exactly as it was.
    ///
    /// The live-code panel is the control running through both: it is in 6d's
    /// own mockup, it is the one thing on this card with a second source
    /// (the code the site is showing right now), and it must not have gone
    /// out with the table.
    #[test]
    fn the_field_table_belongs_to_the_scanned_card_and_the_typed_one_stops_at_the_live_code() {
        // 6d, typed by hand.
        let mut typed = TotpAdd::opening("id-1", "Git Host", false);
        typed.stage = Stage::Manual;
        typed.typed = Zeroizing::new(UNUSUAL.to_string());
        let typed_frame = paint(|ui| {
            draw_stage(ui, &mut typed, BOUNDARY);
        });

        // 6c's furniture, gone: the heading and every row of the table.
        assert!(
            !typed_frame.has(CONFIRM_HEADING),
            "a typed secret still got 6c's heading: {:?}",
            typed_frame.0
        );
        // [`SECRET_ROW_LABEL`] is deliberately NOT in this list: it is
        // `"Secret"`, which is a substring of 6d's own field caption
        // [`SECRET_HINT`], and `has` matches by substring -- asserting its
        // absence would assert 6d's field away. The secret row is pinned below
        // by the two things only it draws.
        for row in [ISSUER_ROW_LABEL, ACCOUNT_ROW_LABEL, PARAMETERS_ROW_LABEL] {
            assert!(
                !typed_frame.has(row),
                "the field table's {row:?} row was drawn for a typed secret: {:?}",
                typed_frame.0
            );
        }
        // ...and with the secret row goes its mask and its Reveal, which are
        // the table's and not the card's.
        assert!(!typed_frame.has(REVEAL_LABEL), "the table's Reveal outlived the table");
        assert!(!typed_frame.has("\u{2022}\u{2022}\u{2022}\u{2022}"), "the masked row outlived it");

        // What 6d DOES draw, all of it, so the above is a gate and not a
        // deletion: the field, its validity line, the parameter controls, and
        // the live code with its countdown and its question.
        assert!(typed_frame.has(SECRET_HINT), "6d's field is gone: {:?}", typed_frame.0);
        let code = code_at(&auth(UNUSUAL), BOUNDARY).expect("the fixture decodes");
        assert!(
            typed_frame.has(grouped_code(&code).as_str()),
            "the live code went out with the table: {:?}",
            typed_frame.0
        );
        // §6d's countdown is the NUMBER, not §6c's sentence, and the question
        // beside it is §6c's too -- see `draw_code_panel`, which draws the
        // design's two strips rather than one of them twice. Both halves are
        // asserted, so a strip that quietly went back to 6c's block fails
        // here and not only in a screenshot.
        assert!(
            typed_frame.has(&seconds_line(60)),
            "the countdown went out with the table: {:?}",
            typed_frame.0
        );
        assert!(
            !typed_frame.has(MATCH_QUESTION),
            "the typed strip is wearing §6c's question: {:?}",
            typed_frame.0
        );
        assert!(
            !typed_frame.has(&CODE_ROW_LABEL.to_uppercase()),
            "the typed strip is wearing §6c's eyebrow: {:?}",
            typed_frame.0
        );

        // The same payload, SCANNED. Unchanged: 6c is a design in its own
        // right and nothing here asks it to become something else.
        let mut scanned = TotpAdd::opening("id-1", "Git Host", false);
        scanned.accept_decoded(Zeroizing::new(UNUSUAL.to_string()));
        let scanned_frame = paint(|ui| {
            draw_stage(ui, &mut scanned, BOUNDARY);
        });
        assert!(
            scanned_frame.has(CONFIRM_HEADING),
            "the scanned card lost 6c's heading: {:?}",
            scanned_frame.0
        );
        for row in
            [SECRET_ROW_LABEL, ISSUER_ROW_LABEL, ACCOUNT_ROW_LABEL, PARAMETERS_ROW_LABEL]
        {
            assert!(
                scanned_frame.has(row),
                "the scanned card lost the field table's {row:?} row: {:?}",
                scanned_frame.0
            );
        }
        assert!(scanned_frame.has(REVEAL_LABEL), "the scanned card lost the way to unmask");
        assert!(scanned_frame.has("Git Host"), "the issuer a scan must be checked against is gone");
        assert!(scanned_frame.has("anovak"), "the account a scan must be checked against is gone");
        // And the panel is on that card too, which is what makes it the one
        // piece common to both paths rather than a consolation for one.
        assert!(scanned_frame.has(MATCH_QUESTION));
        assert!(scanned_frame.has(grouped_code(&code).as_str()));
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
            painted.has(REPLACE_WARNING),
            "the caution band went out with 6c's table: {:?}",
            painted.0
        );
        assert!(painted.has(REPLACE_LABEL), "the destructive button stopped saying so");
        // Control, in the same frame: the table really is gone from this card,
        // so the band above survived a gate rather than there being no gate.
        assert!(!painted.has(CONFIRM_HEADING));
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
    /// **Escape closes the modal, from the two stages that own the keyboard.**
    ///
    /// It answered no key at all before this. Its only way out was a button,
    /// and on a short window that button could sit off the bottom of a
    /// centre-anchored card that does not scroll -- a surface with no exit,
    /// which is indistinguishable from one that has stopped responding.
    ///
    /// Driven per stage rather than once, because the stages are different
    /// surfaces: the picker is four rows and the manual form holds a text
    /// field, and a key answered on one and swallowed on the other is exactly
    /// the kind of gap this test exists to close.
    #[test]
    fn escape_closes_the_picker_and_the_manual_form() {
        for stage in [Stage::Picker, Stage::Manual] {
            let modal = Modal::new();
            let mut state = TotpAdd::opening("i1", "Git Host", false);
            state.stage = stage;
            // One frame to lay the card out, so the second is a frame with a
            // real surface under the keystroke rather than a sizing pass.
            let _ = modal.frame(&mut state, Vec::new());
            let action = modal.frame(&mut state, Modal::escape());
            assert_eq!(
                action,
                TotpAddAction::Cancel,
                "{stage:?}: Escape did not close the modal"
            );
        }
    }

    /// **And it is NOT answered while 6b's overlay is up**, where the key
    /// belongs to the overlay.
    ///
    /// The overlay is a full-screen always-on-top window with the keyboard,
    /// so this branch could only fire on a stray frame after it had gone --
    /// and it would throw the whole form away on the keystroke the user meant
    /// as "stop scanning". The overlay's own Escape cancels the scan and
    /// captures nothing, which is what that key means at that moment.
    #[test]
    fn escape_is_the_overlays_while_a_scan_is_running() {
        let modal = Modal::new();
        let mut state = TotpAdd::opening("i1", "Git Host", false);
        state.stage = Stage::Scanning;
        let _ = modal.frame(&mut state, Vec::new());
        let action = modal.frame(&mut state, Modal::escape());
        assert_eq!(
            action,
            TotpAddAction::None,
            "Escape closed the form while a scan was running, discarding it on the key that \
             was meant to stop the scan"
        );
    }

    /// **And the ✕ answers through the whole modal**, not only through the
    /// stage drawn on its own.
    ///
    /// `the_by_hand_card_closes_from_its_corner` presses the mark on the card
    /// alone. In the app the card is inside `theme::movable_modal` with
    /// `theme::modal_drag_handle` laid over its entire header band, and a
    /// drag-sensing strip over a ✕ swallows the click that dismisses the card
    /// -- a defect this modal has already had once and fixed by registering
    /// the handle before the stage. Nothing pinned that ordering; the picker's
    /// own ✕ test drives `draw_picker` directly and would go on passing if the
    /// strip started eating the press tomorrow. This is that pin, on both
    /// stages that carry a mark.
    #[test]
    fn the_dismiss_mark_survives_the_modals_drag_handle() {
        for stage in [Stage::Picker, Stage::Manual] {
            let modal = Modal::new();
            let mut state = TotpAdd::opening("i1", "Git Host", false);
            state.stage = stage;
            // **Two warm-up frames, and the second is the one read.** An
            // `egui::Area` whose size it has never seen runs its first frame
            // as a sizing pass and paints nothing but `Shape::Noop` -- so a
            // mark looked for in that frame is missing for a reason that has
            // nothing to do with whether the card draws one. The Escape tests
            // beside this take one warm-up for the same reason; this needs
            // shapes as well as an answer, so it takes the frame after.
            let _ = modal.frame(&mut state, Vec::new());
            let (idle, segments) = modal.frame_with_segments(&mut state, Vec::new());
            assert_eq!(idle, TotpAddAction::None, "{stage:?}: the card dismissed itself");
            let at = close_mark_in(&segments);
            assert_eq!(
                modal.frame(&mut state, click_at(at)),
                TotpAddAction::Cancel,
                "{stage:?}: the drag handle swallowed the press on the ✕"
            );
        }
    }

    /// **A frame with no keystroke reports nothing**, which is the control
    /// the two above need: an assertion that Escape closes the modal passes
    /// just as well against a modal that closes on every frame.
    #[test]
    fn an_idle_frame_does_not_close_the_modal() {
        let modal = Modal::new();
        let mut state = TotpAdd::opening("i1", "Git Host", false);
        let _ = modal.frame(&mut state, Vec::new());
        let action = modal.frame(&mut state, Vec::new());
        assert_eq!(action, TotpAddAction::None);
    }

    /// The whole modal, driven through [`draw_add_modal`] rather than through
    /// one stage's own draw function.
    ///
    /// The picker harness above calls `draw_picker` directly, which is right
    /// for questions about what a stage paints and useless for this one:
    /// Escape is answered by the modal, after the stage has reported, and a
    /// harness that never runs that code could not see it.
    struct Modal {
        ctx: egui::Context,
    }

    impl Modal {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            crate::theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Modal { ctx }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(560.0, 900.0),
                )),
                events,
                ..Default::default()
            }
        }

        fn escape() -> Vec<egui::Event> {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        }

        fn frame(&self, state: &mut TotpAdd, events: Vec<egui::Event>) -> TotpAddAction {
            self.frame_with_segments(state, events).0
        }

        /// The same frame, with the straight segments it painted.
        ///
        /// Needed by exactly one question and it is a question only this
        /// harness can answer: whether the ✕ still answers **through the
        /// modal**, where `theme::modal_drag_handle` lays a drag-sensing
        /// strip over the whole header band. The stage harness draws the card
        /// without that strip, so a mark it presses happily could still be
        /// swallowed in the app -- which is the defect `draw_add_modal`
        /// records having already fixed once, by registering the handle
        /// first.
        fn frame_with_segments(
            &self,
            state: &mut TotpAdd,
            events: Vec<egui::Event>,
        ) -> (TotpAddAction, Vec<[egui::Pos2; 2]>) {
            let mut action = TotpAddAction::None;
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                action = draw_add_modal(ui.ctx(), state, 0);
            });
            let mut segments = Vec::new();
            for clipped in &output.shapes {
                collect_segments(&clipped.shape, &mut segments);
            }
            (action, segments)
        }
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

    /// One frame of design 6d, with its rectangles as well as its text.
    ///
    /// A harness of its own rather than [`paint`], for [`Picker`]'s reason:
    /// the questions below are about where things were drawn and how big they
    /// came out, and `paint` reads only the text shapes.
    struct Manual {
        ctx: egui::Context,
    }

    struct ManualRun {
        action: TotpAddAction,
        painted: Painted,
        rects: Vec<PaintedRect>,
        texts: Vec<PaintedText>,
        segments: Vec<[egui::Pos2; 2]>,
    }

    impl ManualRun {
        /// Every rectangle matching `pick`, in paint order.
        fn all(&self, pick: impl Fn(&PaintedRect) -> bool) -> Vec<PaintedRect> {
            self.rects.iter().copied().filter(|r| pick(r)).collect()
        }

        /// Every rectangle painted wholly inside `outer` and smaller than it.
        fn inside(&self, outer: egui::Rect) -> Vec<PaintedRect> {
            self.all(|r| outer.contains_rect(r.rect) && !same_rect(r.rect, outer))
        }

        /// The footer's answers, left to right: the primary and the way back.
        ///
        /// Found by their height inside the footer band rather than by their
        /// fill, because the primary's fill is not [`theme::BLUE`] when the
        /// field holds nothing -- `theme::primary_button_enabled` runs a
        /// disabled button inside a faded `Ui`, and that fade IS the signal
        /// the action is off.
        fn answers(&self) -> Vec<PaintedRect> {
            let band = self.only("footer band", |r| r.fill == theme::CARD_TINT);
            let mut found: Vec<PaintedRect> = self
                .inside(band.rect)
                .into_iter()
                .filter(|r| (r.rect.height() - theme::BUTTON_HEIGHT).abs() <= 1.0)
                .collect();
            found.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
            assert_eq!(
                found.len(),
                2,
                "the footer draws two answers and {} were painted: {found:?}",
                found.len()
            );
            found
        }

        /// The one painted text shape whose string contains `needle`, with
        /// its geometry -- see [`PaintedText`].
        fn text(&self, needle: &str) -> PaintedText {
            let found: Vec<&PaintedText> =
                self.texts.iter().filter(|t| t.text.contains(needle)).collect();
            assert_eq!(
                found.len(),
                1,
                "{} text shape(s) carry {needle:?}; the card painted {:?}",
                found.len(),
                self.texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>()
            );
            found[0].clone()
        }

        /// **6d's one field**, found by its geometry rather than by a
        /// remembered rectangle.
        ///
        /// [`SECRET_BOX_RADIUS`] alone would also match the footer's two
        /// buttons, which the design rounds by the same 8, so the box is the
        /// WIDEST rect at that radius -- 6d's field runs the whole body and
        /// no button on this card is a third of it.
        fn secret_box(&self) -> egui::Rect {
            let mut boxes: Vec<PaintedRect> =
                self.all(|r| r.radius == SECRET_BOX_RADIUS).into_iter().collect();
            assert!(
                !boxes.is_empty(),
                "6d painted nothing at the field's own radius: {:?}",
                self.rects
            );
            boxes.sort_by(|a, b| b.rect.width().total_cmp(&a.rect.width()));
            boxes[0].rect
        }

        /// Where the dismiss ✕ was painted -- see [`close_mark_in`].
        fn close_mark(&self) -> egui::Pos2 {
            close_mark_in(&self.segments)
        }

        /// The one rectangle matching `pick`, or a panic naming what was
        /// there instead -- so a test that finds nothing says so rather than
        /// passing over an empty iterator.
        fn only(&self, what: &str, pick: impl Fn(&PaintedRect) -> bool) -> PaintedRect {
            let found = self.all(pick);
            assert!(!found.is_empty(), "6d painted no {what}: {:?}", self.rects);
            found[0]
        }
    }

    impl Manual {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            crate::theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Manual { ctx }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    // Room for [`MANUAL_WIDTH`] and its shadow, and for the
                    // whole card with 6c fused into it.
                    egui::vec2(560.0, 1200.0),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&self, state: &mut TotpAdd, events: Vec<egui::Event>) -> ManualRun {
            let mut action = TotpAddAction::None;
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                ui.set_max_width(stage_width(state.stage));
                action = draw_stage(ui, state, BOUNDARY);
            });
            let mut painted = Painted(Vec::new());
            let mut rects = Vec::new();
            let mut texts = Vec::new();
            let mut segments = Vec::new();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut painted);
                collect_rects(&clipped.shape, &mut rects);
                collect_texts(&clipped.shape, &mut texts);
                collect_segments(&clipped.shape, &mut segments);
            }
            assert!(
                !painted.0.is_empty(),
                "6d painted no text at all, so every assertion over this list would pass \
                 against nothing"
            );
            assert!(
                !rects.is_empty(),
                "6d painted no rectangles at all, so every assertion over that list would \
                 pass against nothing either"
            );
            ManualRun { action, painted, rects, texts, segments }
        }

        fn idle(&self, state: &mut TotpAdd) -> ManualRun {
            self.frame(state, Vec::new())
        }

        fn click(&self, state: &mut TotpAdd, at: egui::Pos2) -> ManualRun {
            self.frame(state, click_at(at))
        }

        /// A form on 6d with `seed` typed into it, as the by-hand route
        /// leaves it.
        fn typing(seed: &str) -> TotpAdd {
            let mut state = TotpAdd::opening("id-1", "Git Host \u{b7} anovak", false);
            state.stage = Stage::Manual;
            state.typed = Zeroizing::new(seed.to_string());
            state
        }

        /// The same card reached the other way: §6c, a `uri` read off the
        /// screen. `accept_decoded` is the real entry point the overlay uses,
        /// so a state built here is the state a scan actually produces --
        /// including `scanned`, which is what the card branches on.
        fn scanning(uri: &str) -> TotpAdd {
            let mut state = TotpAdd::opening("id-1", "Git Host \u{b7} anovak", false);
            state.accept_decoded(Zeroizing::new(uri.to_string()));
            state
        }
    }

    /// **6d is 470 wide, and [`Stage::Scanning`] is not.**
    ///
    /// `#6a` and `#6d` both declare `width: 470px`; the by-hand form took
    /// `MODAL_WIDTH`'s 380 until this pass. Asserted through `stage_width`
    /// AND off the painted card, because the two are different claims: the
    /// first is what the modal asks its `Area` for, the second is what the
    /// card actually came out as.
    #[test]
    fn the_by_hand_stage_is_the_designs_470_and_the_scan_is_left_alone() {
        assert_eq!(stage_width(Stage::Manual), 470.0, "6d declares width: 470px");
        assert_eq!(stage_width(Stage::Picker), 470.0, "6a declares width: 470px");
        assert_eq!(
            stage_width(Stage::Scanning),
            MODAL_WIDTH,
            "the scanning card was widened as a side effect of 6d's pass"
        );

        let mut state = Manual::typing("");
        let frame = Manual::new().idle(&mut state);
        let card = frame.only("12px-radius white card", |r| {
            r.radius == CARD_RADIUS && r.fill == theme::CARD
        });
        assert!(
            (card.rect.width() - 470.0).abs() <= 1.0,
            "the card is {} wide and 6d's is 470",
            card.rect.width()
        );
        assert_eq!(card.stroke, theme::BORDER_STRONG, "6c's card is bordered #d7d3d3");
    }

    /// **The card is the three-band card 6a and 6c declare**: a header ruled
    /// off, a body, and a `#fbfaf9` footer over a second rule.
    #[test]
    fn the_manual_card_carries_the_designs_bands() {
        let mut state = Manual::typing("");
        let frame = Manual::new().idle(&mut state);

        let card = frame.only("card", |r| r.radius == CARD_RADIUS && r.fill == theme::CARD);
        let footer = frame.only("#fbfaf9 footer band", |r| r.fill == theme::CARD_TINT);
        assert_eq!(footer.radius, 0, "the footer's TOP corners are square");
        assert!(
            (footer.rect.bottom() - card.rect.bottom()).abs() <= 2.0,
            "the footer band does not reach the bottom of the card"
        );
        // As wide as the card, because the tint is painted PROUD of the rect
        // it was allocated -- [`BAND_BLEED`], so no hairline of white card is
        // left showing between the band and the border. 6d's own band is 468
        // inside a 470 card, which is the same band.
        assert!(
            (footer.rect.width() - card.rect.width()).abs() <= 1.0,
            "the footer band is {} wide against a {} card",
            footer.rect.width(),
            card.rect.width()
        );
        // 59 tall: `padding: 12px` and the rule around the 34 the outlined
        // answer's border makes of its declared 32.
        assert!(
            (footer.rect.height() - 59.0).abs() <= 1.0,
            "the footer band is {} tall and 6d's is 59",
            footer.rect.height()
        );

        // Two rules on a card with nothing to overwrite: under the header and
        // over the footer. The caution band brings a third, which is the next
        // test's business.
        let rules = frame.all(|r| r.fill == theme::HAIRLINE && (r.rect.height() - RULE).abs() < 0.5);
        assert_eq!(rules.len(), 2, "6d rules the header off and the footer on: {rules:?}");
    }

    /// **6d's field at this app's field height**: 436 x [`theme::FIELD_HEIGHT`]
    /// at `border-radius: 8px`.
    ///
    /// The width is what makes the rest of the card's arithmetic real -- 470
    /// less the card's border and the body's 16px padding either side.
    ///
    /// The height is **not** 6d's 40.8, and the departure is deliberate: see
    /// [`SECRET_TEXT_PX`], which carries the report. Read off the theme's own
    /// constant rather than written out, so this asserts the field is the
    /// app's field rather than that it is one particular number of points.
    #[test]
    fn the_secret_field_is_the_designs_own_box() {
        let mut state = Manual::typing("");
        let frame = Manual::new().idle(&mut state);
        let field = frame.only("8px-radius field box", |r| {
            r.radius == SECRET_BOX_RADIUS && (r.rect.width() - 436.0).abs() <= 1.0
        });
        assert!(
            (field.rect.height() - theme::FIELD_HEIGHT).abs() <= 0.5,
            "the field is {} tall and this app's fields are {}",
            field.rect.height(),
            theme::FIELD_HEIGHT
        );
        assert_eq!(field.stroke, theme::BORDER_STRONG, "an unfocused field is bordered #d7d3d3");
        assert!(
            frame.painted.has(SECRET_HINT),
            "the caption over the field is missing: {:?}",
            frame.painted.0
        );
    }

    /// **The two parameter runs are `flex: 1` columns**: 212 apiece either
    /// side of 6d's 12px gap, each at `border-radius: 7px`.
    #[test]
    fn the_parameter_runs_split_the_body_between_them() {
        let mut state = Manual::typing("");
        let frame = Manual::new().idle(&mut state);
        let runs = frame.all(|r| {
            r.radius == CHOICE_RADIUS
                && r.stroke == theme::BORDER_STRONG
                && (r.rect.width() - 212.0).abs() <= 1.0
        });
        assert_eq!(runs.len(), 2, "6d draws two 212-wide runs: {:?}", frame.rects);
        assert!(
            (runs[0].rect.height() - 26.0).abs() <= 2.0,
            "a run is {} tall and 6d's is 26",
            runs[0].rect.height()
        );
        assert!(
            (runs[1].rect.left() - runs[0].rect.right() - CHOICE_COLUMN_GAP).abs() <= 1.0,
            "the two runs are not 6d's 12px apart"
        );
        assert!(frame.painted.has(DIGITS_LABEL) && frame.painted.has(PERIOD_LABEL));

        // Exactly one cell of each run is filled `#1b3fa0`, which is the one
        // thing on this control the user reads. Counted INSIDE each run: the
        // footer's primary answer is the same blue, and a count taken over
        // the whole card would be a count of three.
        for run in &runs {
            let lit = frame
                .inside(run.rect)
                .into_iter()
                .filter(|r| r.fill == theme::BLUE)
                .count();
            assert_eq!(lit, 1, "a run lit {lit} cells rather than one: {:?}", run.rect);
        }
    }

    /// **Pressing a cell moves the control**, which is what makes the runs a
    /// control rather than a picture of one.
    ///
    /// Pressed rather than called: the whole run is one `interact` rect
    /// dividing itself into cells by hand, so a cell that were drawn in the
    /// wrong place would still light and still never be reachable.
    #[test]
    fn pressing_a_cell_changes_the_parameter_it_names() {
        let manual = Manual::new();
        let mut state = Manual::typing("JBSWY3DPEHPK3PXP");
        assert_eq!(state.digits, DEFAULT_DIGITS);
        assert_eq!(state.period, DEFAULT_PERIOD);
        let laid_out = manual.idle(&mut state);

        let runs = laid_out.all(|r| {
            r.radius == CHOICE_RADIUS
                && r.stroke == theme::BORDER_STRONG
                && (r.rect.width() - 212.0).abs() <= 1.0
        });
        // The last cell of the digits run is 8, and of the period run 60 s.
        let last_cell = |run: egui::Rect, count: usize| {
            egui::pos2(
                run.left() + run.width() * (count as f32 - 0.5) / count as f32,
                run.center().y,
            )
        };

        let _ = manual.click(&mut state, last_cell(runs[0].rect, DIGITS_CHOICES.len()));
        assert_eq!(
            state.digits,
            DIGITS_CHOICES[DIGITS_CHOICES.len() - 1],
            "pressing the last digits cell did not take"
        );
        let _ = manual.click(&mut state, last_cell(runs[1].rect, PERIOD_CHOICES.len()));
        assert_eq!(
            state.period,
            PERIOD_CHOICES[PERIOD_CHOICES.len() - 1],
            "pressing the last period cell did not take"
        );
    }

    /// **A pasted URI states its own parameters, and the runs go inert.**
    #[test]
    fn a_pasted_uri_leaves_the_runs_dead_and_says_why() {
        let manual = Manual::new();
        let mut typed = Manual::typing("JBSWY3DPEHPK3PXP");
        assert!(
            !manual.idle(&mut typed).painted.has(PARAMETERS_FROM_URI),
            "a bare seed's controls already claimed to come from a URI"
        );

        let mut pasted = Manual::typing(UNUSUAL);
        let frame = manual.idle(&mut pasted);
        assert!(
            frame.painted.has(PARAMETERS_FROM_URI),
            "nothing on the card says why the controls cannot be pressed: {:?}",
            frame.painted.0
        );
        // `segmented_control_disabled`'s treatment: the answer in force stays
        // identifiable in the wash rather than every cell greying alike.
        let runs = frame.all(|r| {
            r.radius == CHOICE_RADIUS
                && r.stroke == theme::BORDER_STRONG
                && (r.rect.width() - 212.0).abs() <= 1.0
        });
        assert_eq!(runs.len(), 2, "the dead controls are not on the card at all");
        for run in &runs {
            let cells = frame.inside(run.rect);
            assert!(
                !cells.iter().any(|r| r.fill == theme::BLUE),
                "a dead run painted the live control's #1b3fa0 fill"
            );
            assert!(
                cells.iter().any(|r| r.fill == theme::BLUE_WASH),
                "a dead run lit nothing at all, so it says the card has no parameters"
            );
        }

        // And pressing one does nothing, which is the half a fill cannot say.
        let before = pasted.digits;
        let _ = manual.click(&mut pasted, runs[0].rect.center());
        assert_eq!(pasted.digits, before, "a dead cell was pressable after all");
    }

    /// **6d's line under the field**, in both of its states.
    #[test]
    fn the_validity_line_checks_off_a_seed_and_names_a_refusal() {
        let manual = Manual::new();
        let mut good = Manual::typing("JBSW Y3DP EHPK 3PXP");
        let valid = manual.idle(&mut good);
        assert!(
            valid.painted.has("Valid base32"),
            "6d's own line is not under the field: {:?}",
            valid.painted.0
        );

        let mut bad = Manual::typing("https://example.com");
        let refused = manual.idle(&mut bad);
        assert!(refused.painted.has("plain URL"), "the refusal is not on screen");
        assert!(
            !refused.painted.has("Valid base32"),
            "a refused field still claimed to be valid base32"
        );
        assert!(
            !refused.painted.has(CONFIRM_HEADING),
            "a refused field still painted a confirmation to save from"
        );
    }

    /// What to look for when the fixture is an unknown parameter with a
    /// deliberately long key.
    ///
    /// **Not the key itself**: the key is also in the URI the field is
    /// holding, so a search for it finds the `TextEdit`'s own galley as well
    /// as the refusal and neither test can tell which it measured. This
    /// phrase is in the sentence and in nothing else on the card.
    const OVERLONG_NEEDLE: &str = "which Deskwarden does not know";

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
                "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&image=icon.png"
                    .to_string(),
                OtpRefusal::UnknownParameter("image".to_string()),
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

    /// **No refusal outgrows the line design 6d drew for it.**
    ///
    /// `#6d` puts ONE short line under the field. The length refusal was
    /// written at two hundred and twenty characters and reached the screen as
    /// three wrapped rows of red; the owner's words were *"yes but wrong size
    /// and overlaps"*, and the size half is this. Measured by RENDERING each
    /// refusal on the real card and reading the galley's row count, not by
    /// counting characters against a guessed pixel budget: what decides
    /// whether a sentence wraps is the face, the tracking and the body's real
    /// width, and only the card knows all three.
    ///
    /// Two rows is the ceiling for the family and one is the rule for the
    /// length refusal, which is the one the owner reported and the one whose
    /// whole content is a number and a fix.
    #[test]
    fn the_refusals_fit_the_line_the_design_drew_for_them() {
        let manual = Manual::new();
        for (typed, refusal) in every_refusal_the_field_can_show() {
            let sentence = refusal_sentence(&refusal);
            let mut state = Manual::typing(&typed);
            let run = manual.idle(&mut state);
            let line = run.text(&sentence);
            assert!(
                line.rows <= 2,
                "{refusal:?} wraps to {} rows under 6d's field, which draws one: {sentence:?}",
                line.rows
            );
            if matches!(refusal, OtpRefusal::PartialSecret(_)) {
                assert_eq!(
                    line.rows, 1,
                    "the length refusal takes {} rows: {sentence:?}",
                    line.rows
                );
            }
        }
        // The control the ceiling needs. A sentence long enough to wrap DOES
        // report more than two rows through the same path, so the assertions
        // above are about the copy and not about `PaintedText::rows` always
        // answering one.
        let key = "a".repeat(120);
        let mut overlong = Manual::typing(&format!(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&{key}=1"
        ));
        assert!(
            manual.idle(&mut overlong).text(OVERLONG_NEEDLE).rows > 2,
            "a deliberately overlong refusal reported two rows or fewer, so the ceiling above \
             is measuring nothing"
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

    /// **The line under the field never lies on top of the field**, however
    /// long the sentence in it is.
    ///
    /// This is the other half of *"yes but wrong size and overlaps"*, and it
    /// is a layout defect rather than a copy one: [`secret_field`] draws its
    /// `TextEdit` with `Ui::put`, which advances the parent's cursor past the
    /// CHILD's rectangle -- the box's padding box, ten points above the band
    /// the field actually occupies. Everything after it in the column opened
    /// ten points high. With 6d's own one-line sentence that put the green
    /// check on the box's bottom border; with a refusal long enough to wrap
    /// it painted red prose straight through it.
    ///
    /// **Driven with a refusal deliberately longer than anything this module
    /// ships**, because the copy has just been shortened and a test pinned to
    /// today's longest sentence would stop testing the layout the moment the
    /// next one arrives. The unknown-parameter refusal prints its key
    /// verbatim, so a long key is a long sentence on demand.
    #[test]
    fn a_long_refusal_clears_the_field_it_is_under() {
        let manual = Manual::new();
        let key = "a".repeat(120);
        let mut state = Manual::typing(&format!(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&{key}=1"
        ));
        let run = manual.idle(&mut state);
        let line = run.text(OVERLONG_NEEDLE);
        assert!(
            line.rows >= 3,
            "the fixture stopped wrapping ({} row(s)), so this test would pass against a \
             layout that never moves a wrapped line",
            line.rows
        );

        let field = run.secret_box();
        // The design's own `gap: 7px` between the box and the line under it,
        // read off `#6d`'s field column -- [`SECRET_BLOCK_GAP`]. Before the
        // fix this gap was NEGATIVE three.
        let gap = line.rect.top() - field.bottom();
        assert!(
            (gap - SECRET_BLOCK_GAP).abs() < 1.0,
            "the refusal starts {gap} points under the field, not the design's \
             {SECRET_BLOCK_GAP} -- a negative number here is the reported overlap"
        );
        // And it stays in the field's own column, which is the other thing
        // the owner's screenshot showed: prose running past the box's edges.
        assert!(
            line.rect.right() <= field.right() + 0.5,
            "the refusal runs {} points past the field's right edge",
            line.rect.right() - field.right()
        );
        assert!(
            line.rect.left() >= field.left() - 0.5,
            "the refusal starts {} points left of the field",
            field.left() - line.rect.left()
        );
    }

    /// **6d's accepted line keeps the same gap**, which is the case the fix
    /// above would be easy to get right for the refusal alone and wrong for.
    ///
    /// The overlap was a cursor that wound backwards, so it was never about
    /// the sentence: the green check and *"Valid base32 · 16 characters ·
    /// spaces ignored"* sat three points inside the box too, and on the state
    /// design 6d actually draws.
    #[test]
    fn the_accepted_line_keeps_the_designs_gap_under_the_box() {
        let manual = Manual::new();
        let mut state = Manual::typing("JBSW Y3DP EHPK 3PXP");
        let run = manual.idle(&mut state);
        let line = run.text("Valid base32");
        let field = run.secret_box();
        let gap = line.rect.top() - field.bottom();
        assert!(
            (gap - SECRET_BLOCK_GAP).abs() < 1.0,
            "6d's own line sits {gap} points under the field, not {SECRET_BLOCK_GAP}"
        );
    }

    /// **The by-hand card closes from its corner, exactly as the picker
    /// does.**
    ///
    /// The app's rule is that every dialog closes from its corner, and this
    /// card was the hole in it: `picker_header` has carried
    /// `theme::modal_dismiss_mark` since that pass and `manual_header` drew
    /// nothing, so the gesture worked on 6a and stopped working one step
    /// later in the same flow.
    ///
    /// Both halves, because a mark that paints and does not answer is the
    /// defect this project has shipped twice -- once as a dead keycap, once
    /// as a dead Cancel arm. The mark is FOUND (two crossing diagonals at
    /// `theme::CLOSE_MARK_SPAN`, not a rectangle computed from the inset) and
    /// then PRESSED at the point it was found at.
    #[test]
    fn the_by_hand_card_closes_from_its_corner() {
        for scanned in [false, true] {
            let manual = Manual::new();
            let mut state = Manual::typing("JBSWY3DPEHPK3PXP");
            state.scanned = scanned;
            let laid_out = manual.idle(&mut state);
            let at = laid_out.close_mark();
            // Inside the card's header band and at its right-hand end, so a
            // mark painted somewhere plausible but wrong does not pass.
            let card = laid_out.only("card", |r| r.radius == CARD_RADIUS);
            assert!(
                at.x > card.rect.right() - 40.0 && at.x < card.rect.right(),
                "the ✕ is at x={} on a card whose right edge is {}",
                at.x,
                card.rect.right()
            );
            assert!(
                at.y < card.rect.top() + stage_header_height(Stage::Manual),
                "the ✕ is below the header band"
            );

            // An idle frame reports nothing, so the press below is the thing
            // that produced the answer.
            assert_eq!(
                laid_out.action,
                TotpAddAction::None,
                "scanned={scanned}: the card dismissed itself without being pressed"
            );
            assert_eq!(
                manual.click(&mut state, at).action,
                // The same value `draw_add_modal`'s Escape branch returns, so
                // the key and the corner cannot reach different states.
                TotpAddAction::Cancel,
                "scanned={scanned}: pressing the ✕ did not dismiss the card"
            );
        }
    }

    /// **6c's kind label clears the mark rather than sharing its corner.**
    ///
    /// The scanned header sets `otpauth://totp` at the far right, which is
    /// where the ✕ now is. A right edge computed from
    /// [`MANUAL_HEADER_PAD_X`] would print that label straight through the
    /// mark's arms -- legible in neither direction, and exactly the kind of
    /// collision a headless suite cannot see and a screenshot can.
    #[test]
    fn the_scanned_headers_kind_label_clears_the_dismiss_mark() {
        let manual = Manual::new();
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.accept_decoded(Zeroizing::new(UNUSUAL.to_string()));
        let run = manual.idle(&mut state);
        let at = run.close_mark();
        let kind = run.text(CODE_READ_KIND);
        assert!(
            kind.rect.right() <= at.x - theme::CLOSE_MARK_HIT / 2.0,
            "6c's {CODE_READ_KIND:?} ends at {} and the ✕'s box starts at {}",
            kind.rect.right(),
            at.x - theme::CLOSE_MARK_HIT / 2.0
        );
    }

    /// **The header says what the card is for, and switches when a decoder
    /// did the typing.**
    #[test]
    fn the_header_band_carries_6d_typed_and_6c_scanned() {
        let manual = Manual::new();
        let mut typed = Manual::typing("JBSWY3DPEHPK3PXP");
        let by_hand = manual.idle(&mut typed);
        assert!(by_hand.painted.has(HEADING), "6d's header is not on the card");
        assert!(!by_hand.painted.has(CODE_READ_KIND), "a typed seed claimed to have been read");

        let mut scanned = TotpAdd::opening("id-1", "Git Host", false);
        scanned.accept_decoded(Zeroizing::new(UNUSUAL.to_string()));
        let read = manual.idle(&mut scanned);
        assert!(
            read.painted.has(CODE_READ_LABEL) && read.painted.has(CODE_READ_KIND),
            "the scanned card's header is not 6c's: {:?}",
            read.painted.0
        );
        assert!(
            !read.painted.0.iter().any(|t| t == HEADING),
            "the scanned card still asks for a secret to be entered"
        );
        assert!(
            !read.painted.has(SECRET_HINT),
            "the editable field was drawn for a scanned payload"
        );
    }

    /// **The replace warning is 6c's caution band and not a red label.**
    ///
    /// The sentence is unchanged -- it is pinned by content in
    /// `the_replace_warning_is_the_designs_own_sentence` -- and where it is
    /// drawn is what this pass moved: onto `#fef6e7`, directly over the
    /// button that does the thing, which is 6c's own arrangement.
    #[test]
    fn the_replace_warning_is_drawn_in_the_designs_caution_band() {
        let manual = Manual::new();
        let mut clean = Manual::typing("JBSWY3DPEHPK3PXP");
        let safe = manual.idle(&mut clean);
        assert!(!safe.painted.has("cannot be recovered"));
        assert!(
            safe.all(|r| r.fill == CAUTION_FILL).is_empty(),
            "a card with nothing to overwrite drew the caution band anyway"
        );

        let mut existing = Manual::typing("JBSWY3DPEHPK3PXP");
        existing.already_has_code = true;
        let frame = manual.idle(&mut existing);
        assert!(frame.painted.has(REPLACE_WARNING), "the warning is not on the card");
        let band = frame.only("#fef6e7 caution band", |r| r.fill == CAUTION_FILL);
        let footer = frame.only("footer band", |r| r.fill == theme::CARD_TINT);
        assert!(
            (band.rect.bottom() - footer.rect.top()).abs() <= 1.0,
            "the caution band does not sit directly over the footer it warns about"
        );
        assert!(frame.painted.has(REPLACE_LABEL), "the destructive face is not on the button");
        assert!(!frame.painted.has(SAVE_LABEL), "both button faces were painted at once");
    }

    /// **The footer is the design's TWO, and each card wears its own.**
    ///
    /// §6d draws `Save code` and `Cancel` and nothing else; §6c draws
    /// `Replace code`, a way back, and `Esc discards` pushed to the far right.
    /// They were one fused band, which gave the typed card a route back to a
    /// picker it had not arrived from and a hint no other modal in this app
    /// prints. The owner: "Save code and Cancel buttons, no Esc descards
    /// unless same everywhere".
    ///
    /// **Both cards are read in one test on purpose.** Either alone would pass
    /// against a footer that drew one shape for everybody, which is the defect
    /// this closes.
    #[test]
    fn each_card_wears_its_own_footer() {
        let manual = Manual::new();

        // §6d, typed by hand: two buttons.
        let mut typed = Manual::typing("JBSWY3DPEHPK3PXP");
        let frame = manual.idle(&mut typed);
        assert!(frame.painted.has(SAVE_LABEL), "the primary answer is missing");
        assert!(
            frame.painted.has(CANCEL_LABEL),
            "§6d's second answer is missing: {:?}",
            frame.painted.0
        );
        assert!(
            !frame.painted.has(OTHER_WAYS_LABEL),
            "the typed card offers a way back to a picker it never came from"
        );
        assert!(
            !frame.painted.has(DISMISS_HINT),
            "the typed card prints an Escape hint no other modal in this app prints"
        );

        // §6c, scanned: the way back and the hint, exactly as before.
        let mut scanned = Manual::scanning(UNUSUAL);
        let frame = manual.idle(&mut scanned);
        assert!(
            frame.painted.has(OTHER_WAYS_LABEL),
            "the way back to 6a is missing from the scanned card: {:?}",
            frame.painted.0
        );
        assert!(
            frame.painted.has(DISMISS_HINT),
            "§6c's Esc hint is not on the footer: {:?}",
            frame.painted.0
        );
        // The hint carries the cancel there, so there is no second button
        // saying the same thing. See [`manual_footer`].
        assert!(
            !frame.painted.0.iter().any(|t| t == CANCEL_LABEL),
            "the scanned footer draws both the Esc hint and a Cancel button for one action"
        );

        // And the key the hint names really answers, through the modal that
        // binds it -- pinned already by
        // `escape_closes_the_picker_and_the_manual_form`, restated here as the
        // control this assertion needs: a hint for a dead key is the thing
        // this file refuses.
        let modal = Modal::new();
        let mut state = Manual::typing("JBSWY3DPEHPK3PXP");
        let _ = modal.frame(&mut state, Vec::new());
        assert_eq!(modal.frame(&mut state, Modal::escape()), TotpAddAction::Cancel);
    }

    /// **The primary is dead until there is something to save**, and reports
    /// [`TotpAddAction::Save`] when it is pressed.
    #[test]
    fn the_primary_answers_only_once_the_field_reads_as_a_code() {
        let manual = Manual::new();
        let mut empty = Manual::typing("");
        let blank = manual.idle(&mut empty);
        let primary = blank.answers()[0];
        assert_ne!(
            primary.fill,
            theme::BLUE,
            "the primary is at full strength with nothing to save, so it does not look off"
        );
        assert_eq!(
            manual.click(&mut empty, primary.rect.center()).action,
            TotpAddAction::None,
            "the primary saved an empty field"
        );

        let mut state = Manual::typing("JBSWY3DPEHPK3PXP");
        let laid_out = manual.idle(&mut state);
        let primary = laid_out.answers()[0];
        assert_eq!(primary.fill, theme::BLUE, "a live primary is not the design's #1b3fa0");
        assert_eq!(
            manual.click(&mut state, primary.rect.center()).action,
            TotpAddAction::Save,
            "the primary did not report a save"
        );
    }

    /// **The way back is a button in the footer**, and it really goes back.
    ///
    /// It was a text link in the old button row. 6c draws it as an outlined
    /// answer beside the primary, which is where a second answer belongs, and
    /// pressing it must still empty the field -- see [`TotpAdd::back_to_picker`],
    /// which is the reason this is asserted through a press rather than by
    /// calling the thing the press calls.
    #[test]
    fn the_footers_second_answer_goes_back_to_the_picker() {
        let manual = Manual::new();
        // **A SCANNED card**, which is the only one that has a way back: see
        // [`manual_footer`]. On the typed card the same slot is Cancel, and
        // that half is asserted below.
        let mut state = Manual::scanning(UNUSUAL);
        let laid_out = manual.idle(&mut state);
        let back = laid_out.answers()[1];
        assert_eq!(back.stroke, theme::BORDER_STRONG, "the second answer is not outlined");

        let after = manual.click(&mut state, back.rect.center());
        assert_eq!(after.action, TotpAddAction::None, "the way back asked the caller to act");
        assert_eq!(state.stage, Stage::Picker, "the second answer did not go back to 6a");
        assert!(state.typed.is_empty(), "a seed was left resident on the way back");

        // And §6d's second answer in the same slot: it closes, and it does NOT
        // quietly go back to the picker instead. Asserted through a press for
        // the reason above -- the button's wiring is the claim, not its face.
        let mut typed = Manual::typing("JBSWY3DPEHPK3PXP");
        let laid_out = manual.idle(&mut typed);
        let cancel = laid_out.answers()[1];
        let after = manual.click(&mut typed, cancel.rect.center());
        assert_eq!(
            after.action,
            TotpAddAction::Cancel,
            "§6d's Cancel did not report the same value Escape does"
        );
        assert_eq!(typed.stage, Stage::Manual, "Cancel walked the user back to 6a");
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

    /// **Pressing the webcam row reports it rather than doing it.**
    #[test]
    fn the_camera_row_asks_the_caller_to_enumerate() {
        assert_eq!(action_for(Route::Webcam), TotpAddAction::OpenWebcam);
        // And it is reachable by pressing, not only by calling `action_for`.
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let at = picker.idle(&mut state).row(Route::Webcam).center();
        let pressed = picker.click(&mut state, at);
        assert_eq!(pressed.action, TotpAddAction::OpenWebcam, "the camera row is not reachable");
        // Nothing was opened by the press itself: the device is the caller's
        // to enumerate, and until it does there is no camera in this state.
        assert!(state.webcam.is_none());
        assert_eq!(state.stage, Stage::Picker);
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

    /// **A refusal is painted on the picker the user comes back to.**
    #[test]
    fn the_picker_paints_a_camera_refusal_under_its_rows() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Err(CameraRefusal::Denied));
        let frame = Picker::new().idle(&mut state);
        assert!(
            frame.painted.has(&PickerRefusal::Camera(CameraRefusal::Denied).sentence()),
            "the reason the camera did not open is not on screen: {:?}",
            frame.painted.0
        );
    }

    /// **A deferred row still says that it is deferred.**
    ///
    /// Nothing in [`ROUTES`] is disabled today -- see [`DEFERRED_REASON`] --
    /// so this paints a synthetic one, which is what keeps the treatment from
    /// becoming code nobody has run since the webcam row was lit.
    #[test]
    fn a_deferred_row_still_says_that_it_is_deferred() {
        let dead = RouteRow {
            route: Route::Webcam,
            title: "Something later",
            subtitle: "A route this app does not have yet",
            enabled: false,
        };
        let painted = paint(|ui| {
            ui.set_max_width(PICKER_WIDTH);
            let _ = route_row(ui, &dead, 1);
        });
        assert!(painted.has(dead.title), "control: the row was not drawn at all");
        assert!(painted.has(DEFERRED_REASON), "a dead row does not say it is deferred");

        // The control the other way: a LIVE row does not carry the reason.
        let live = RouteRow { enabled: true, ..dead };
        let painted = paint(|ui| {
            ui.set_max_width(PICKER_WIDTH);
            let _ = route_row(ui, &live, 1);
        });
        assert!(painted.has(live.title));
        assert!(!painted.has(DEFERRED_REASON), "a live row says it is deferred");
    }

    /// **The preview letterboxes rather than stretching.**
    ///
    /// A stretched QR code still looks like a QR code, which is why this is
    /// arithmetic with a test rather than a value trusted to look right.
    #[test]
    fn the_preview_keeps_a_frames_shape() {
        let into = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 200.0));

        // Wider than the box: it fills the width and is centred vertically.
        let wide = preview_fit(into, (800, 200));
        assert!((wide.width() - 400.0).abs() < 0.5, "{wide:?}");
        assert!((wide.height() - 100.0).abs() < 0.5, "{wide:?}");
        assert!((wide.center() - into.center()).length() < 0.5);

        // Taller than the box: it fills the height instead.
        let tall = preview_fit(into, (200, 800));
        assert!((tall.height() - 200.0).abs() < 0.5, "{tall:?}");
        assert!((tall.width() - 50.0).abs() < 0.5, "{tall:?}");

        // A 4:3 camera on this box, which is the ordinary case.
        let ordinary = preview_fit(into, (640, 480));
        assert!(
            (ordinary.width() / ordinary.height() - 640.0 / 480.0).abs() < 0.01,
            "the aspect ratio was not kept: {ordinary:?}"
        );
        assert!(into.contains_rect(ordinary), "the picture spilled out of the preview");

        // Nonsense in, the box out, rather than a division by zero.
        assert_eq!(preview_fit(into, (0, 0)), into);
    }

    // -----------------------------------------------------------------
    // The camera stage, on screen
    // -----------------------------------------------------------------

    /// Draws the camera stage once and hands back what it painted.
    fn paint_webcam(state: &mut TotpAdd) -> Painted {
        paint(|ui| {
            ui.set_max_width(stage_width(Stage::Webcam));
            let _ = draw_webcam(ui, state, None);
        })
    }

    /// **The camera stage says what to do, what is happening, and what is
    /// being done with the picture.**
    #[test]
    fn the_camera_stage_paints_its_instruction_its_state_and_its_privacy_line() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(one_camera()));
        let painted = paint_webcam(&mut state);

        assert!(painted.has(WEBCAM_HEADING), "the stage does not say what to do");
        assert!(painted.has(ADDING_TO_LABEL) && painted.has("Git Host"), "{:?}", painted.0);
        // No picture yet, so it says it is starting rather than showing a
        // black rectangle and nothing else.
        assert!(painted.has(WEBCAM_STARTING), "a camera with no picture yet says nothing");
        assert!(!painted.has(WEBCAM_HINT), "it claims to be reading a picture it has not got");
        assert!(painted.has(WEBCAM_PRIVACY_LINE), "the camera's privacy line is not on the card");
        assert!(painted.has(OTHER_WAYS_LABEL), "there is no way back to the picker");
        // One camera, so no offer to switch to another.
        assert!(!painted.has(WEBCAM_ANOTHER_LABEL));
    }

    /// **Once a picture is arriving the line under it changes**, so "starting"
    /// is a state and not a permanent caption.
    #[test]
    fn the_stage_stops_saying_it_is_starting_once_a_picture_arrives() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(one_frame_pump), Ok(one_camera()));
        let mut frame = None;
        until("a frame", || {
            frame = advance_webcam(&mut state, std::time::Instant::now());
            frame.is_some()
        });
        // The frame is handed to the draw call, which is what creates the
        // texture the preview paints.
        let painted = paint(|ui| {
            ui.set_max_width(stage_width(Stage::Webcam));
            let _ = draw_webcam(ui, &mut state, frame);
        });
        assert!(painted.has(WEBCAM_HINT), "a live preview still says it is starting");
        assert!(!painted.has(WEBCAM_STARTING));
        assert!(
            state.webcam.as_ref().is_some_and(|s| s.showing()),
            "no texture was made for the frame"
        );
    }

    /// **With several cameras the stage is a list, and pressing one opens it.**
    #[test]
    fn several_cameras_are_offered_by_name_and_pressing_one_opens_it() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(two_cameras()));
        let painted = paint_webcam(&mut state);

        assert!(painted.has(WEBCAM_CHOOSE), "the list has no heading");
        assert!(painted.has(WEBCAM_CHOOSE_HINT), "the list does not say nothing is switched on");
        for camera in two_cameras() {
            assert!(painted.has(&camera.name), "{} is not on the list", camera.name);
        }
        // The device path is NOT painted: it is a name for Windows, not prose.
        assert!(
            !painted.0.iter().any(|t| t.contains("usb#")),
            "a device path was painted: {:?}",
            painted.0
        );
        assert!(!painted.has(WEBCAM_HEADING), "it is aiming a camera nobody has chosen");
    }

    /// **Pressing a camera on the list reports which one**, and opens
    /// nothing by itself.
    ///
    /// Driven through a real press rather than by calling
    /// [`choose_camera`], which is the rule `PickerFrame`'s row rectangles
    /// exist for: a list whose rows had stopped being clickable would pass
    /// every assertion written the other way.
    #[test]
    fn pressing_a_camera_on_the_list_reports_that_camera_and_opens_nothing() {
        let ctx = webcam_ctx();
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(two_cameras()));

        // Two settled frames first, then one that presses: egui needs a
        // frame in which the row existed before a click on it can land.
        let at = webcam_text_at(&ctx, &mut state, &two_cameras()[1].name)
            .expect("the second camera was not painted");
        let mut action = TotpAddAction::None;
        let _ = ctx.run_ui(webcam_input(click_at(at)), |ui| {
            ui.set_max_width(stage_width(Stage::Webcam));
            action = draw_webcam(ui, &mut state, None);
        });
        assert_eq!(action, TotpAddAction::UseCamera(1), "the press named the wrong camera");
        // **Nothing was opened by the press itself**: the seam belongs to
        // the caller, so a test can press this row with no camera on the
        // machine at all -- which is the whole reason it is reported.
        let stage = state.webcam.as_ref().expect("a camera stage");
        assert_eq!(stage.chosen, None);
        assert!(stage.session.is_none(), "the draw call opened a device");

        // And what the caller does with it opens that one.
        choose_camera(&mut state, &seams(silent_pump), 1);
        assert_eq!(state.webcam.as_ref().and_then(|s| s.chosen), Some(1));
    }

    /// The camera stage's own headless context, warmed the way [`Picker`]
    /// warms its own: two frames so the theme's fonts are in place.
    fn webcam_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(webcam_input(Vec::new()), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(webcam_input(Vec::new()), |_ui| {});
        ctx
    }

    fn webcam_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(560.0, 900.0),
            )),
            events,
            ..Default::default()
        }
    }

    /// Draws the camera stage twice on `ctx` and answers where `wanted` was
    /// painted the second time.
    ///
    /// Twice because egui settles a surface over a frame, and the rectangle
    /// read off the first one is the one a click would miss. `Painted`
    /// cannot report a position, so this walks the frame's own text shapes.
    fn webcam_text_at(
        ctx: &egui::Context,
        state: &mut TotpAdd,
        wanted: &str,
    ) -> Option<egui::Pos2> {
        let mut at = None;
        for _ in 0..2 {
            let output = ctx.run_ui(webcam_input(Vec::new()), |ui| {
                ui.set_max_width(stage_width(Stage::Webcam));
                let _ = draw_webcam(ui, state, None);
            });
            at = None;
            for clipped in &output.shapes {
                find_text(&clipped.shape, wanted, &mut at);
            }
        }
        at
    }

    /// **The way back to the picker closes the camera**, driven through the
    /// link rather than by calling what the link calls.
    #[test]
    fn the_way_back_from_the_camera_closes_it() {
        let ctx = webcam_ctx();
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        open_webcam(&mut state, &seams(silent_pump), Ok(one_camera()));

        let at = webcam_text_at(&ctx, &mut state, OTHER_WAYS_LABEL)
            .expect("the way back was not painted");
        let _ = ctx.run_ui(webcam_input(click_at(at)), |ui| {
            ui.set_max_width(stage_width(Stage::Webcam));
            let _ = draw_webcam(ui, &mut state, None);
        });
        assert_eq!(state.stage, Stage::Picker, "the way back did not go back");
        assert!(state.webcam.is_none(), "the way back left the camera open");
    }

    /// Where a piece of painted text ended up, for a test that has to press
    /// something the surface does not report a rectangle for.
    fn find_text(shape: &egui::Shape, wanted: &str, out: &mut Option<egui::Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == wanted => {
                *out = Some(text.pos + text.galley.size() / 2.0);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    find_text(shape, wanted, out);
                }
            }
            _ => {}
        }
    }
}

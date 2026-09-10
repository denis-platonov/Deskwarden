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
use eframe::egui::{self, CornerRadius};
use zeroize::Zeroizing;

/// Height of every button on this form, matching `record_ui`'s.
const BUTTON_HEIGHT: f32 = 26.0;

/// The card's width, matching the record composer's narrow column.
const MODAL_WIDTH: f32 = 380.0;

// ---------------------------------------------------------------------------
// The copy
// ---------------------------------------------------------------------------

/// The heading over the whole surface.
pub const HEADING: &str = "Add a TOTP";

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

/// The hint in the one field, verbatim from design 6d.
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
    /// A PNG the user already has.
    ImageFile,
    /// 6d: the form that was already here.
    ByHand,
    /// Present and dead. See [`WEBCAM_REASON`].
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
    /// see [`WEBCAM_REASON`].
    pub enabled: bool,
}

/// **Why the webcam row is drawn and dead rather than left out.**
///
/// It is in design 6a and it is not in this plan. A route the design promises
/// and the product silently omits reads as a bug: the user looks for it, does
/// not find it, and cannot tell whether they are looking in the wrong place.
/// A row that says what it is and why it is off answers that in one glance,
/// in the place they went looking.
pub const WEBCAM_REASON: &str = "Not in this version";

/// See [`WEBCAM_REASON`]. The sentence under the dead row -- the honest
/// reason, not an apology: a webcam needs a capture API, a device picker and a
/// preview surface, and on a Windows desktop the code is nearly always
/// already on the screen, which is what the first row is for.
pub const WEBCAM_DETAIL: &str = "A webcam needs a capture device and a preview of its own. On a \
     desktop the code is nearly always already on screen \u{2014} scan a region instead.";

/// **Design 6a's four routes, in its order** -- *"ordered by how often they're
/// the right one on Windows"*.
///
/// The `ImageFile` subtitle says **PNG** where 6a says *"PNG, JPG"*, and that
/// is a deliberate edit rather than an omission: this ships a `png`-crate
/// decode and no JPEG one, [`crate::file_picker::pick_qr_image`]'s filter says
/// the same, and a row promising JPG over a dialog that will not show one is a
/// promise broken one click later. The copy was changed, not the reader's
/// impression.
pub const ROUTES: [RouteRow; 4] = [
    RouteRow {
        route: Route::ScanRegion,
        title: "Scan a region of my screen",
        subtitle: "Drag a box around the QR code in any window",
        enabled: true,
    },
    RouteRow {
        route: Route::ImageFile,
        title: "Open an image file",
        subtitle: "A screenshot or photo of the code \u{b7} PNG",
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
        subtitle: WEBCAM_DETAIL,
        enabled: false,
    },
];

/// What the picker's way back to itself is called, from the two halves that
/// have one.
pub const OTHER_WAYS_LABEL: &str = "Other ways to add it";

/// The heading while the 6b overlay is up.
pub const SCANNING_HEADING: &str = "Scanning your screen";

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
    /// The file is not a PNG this app can decode.
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
            PickerRefusal::NotAnImage => {
                "That file isn't a PNG Deskwarden can read. Save the picture as a PNG and \
                 choose it again \u{2014} this version reads PNG only."
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
/// the PNG half produced rather than trust that it produced any.
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

/// A PNG's pixels as straight RGBA8, rows top to bottom, no padding -- what
/// [`crate::qr::decode_qr`] and `GetDIBits` both speak.
///
/// **Not [`crate::favicon::decode_rgba`]**, which is this crate's other PNG
/// reader. That one resamples every image down to 64 pixels on its longest
/// edge for the item list, and 64 pixels is smaller than a QR code's module
/// grid: it would hand back a picture of a code that no decoder could read.
///
/// Both buffers are [`Zeroizing`], because a picture of a QR code is a picture
/// of a seed.
fn png_to_rgba(bytes: &[u8]) -> Result<(Zeroizing<Vec<u8>>, usize, usize), PickerRefusal> {
    let mut decoder = png::Decoder::new(bytes);
    // The same normalisation `favicon` uses: indexed and sub-8-bit sources are
    // expanded during the decode, so the match below never sees them.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| PickerRefusal::NotAnImage)?;

    // **Bounded before anything is allocated.** The header is attacker-chosen
    // -- it is a file -- and `output_buffer_size` is derived from it. The
    // bound is `qr`'s own, so a picture refused here is exactly a picture the
    // decoder would have refused anyway.
    let (declared_width, declared_height) = {
        let info = reader.info();
        (info.width as usize, info.height as usize)
    };
    if declared_width == 0 || declared_height == 0 {
        return Err(PickerRefusal::NotAnImage);
    }
    match declared_width.checked_mul(declared_height) {
        Some(pixels) if pixels <= crate::qr::MAX_PIXELS => {}
        _ => return Err(PickerRefusal::NotAnImage),
    }

    let mut buf = Zeroizing::new(vec![0u8; reader.output_buffer_size()]);
    let frame = reader.next_frame(&mut buf).map_err(|_| PickerRefusal::NotAnImage)?;
    let (width, height) = (frame.width as usize, frame.height as usize);
    let used = frame.buffer_size().min(buf.len());
    let source = &buf[..used];

    let mut rgba = Zeroizing::new(Vec::with_capacity(width.saturating_mul(height) * 4));
    match frame.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(source),
        png::ColorType::Rgb => {
            for px in source.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for grey in source {
                rgba.extend_from_slice(&[*grey, *grey, *grey, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in source.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        // Unreachable: `normalize_to_color8` expands indexed frames during the
        // decode, exactly as `favicon::decode_rgba` documents. Kept only so
        // this match stays exhaustive.
        png::ColorType::Indexed => return Err(PickerRefusal::NotAnImage),
    }
    if rgba.len() < width * height * 4 {
        // A truncated frame: `next_frame` can succeed on a file whose last
        // rows are missing. A short buffer handed to the decoder answers
        // `None`, which would be reported as "no code in that image" rather
        // than as the broken file it is.
        return Err(PickerRefusal::NotAnImage);
    }
    Ok((rgba, width, height))
}

/// **A PNG's bytes to the string its QR carries**, through `seams`.
///
/// Nothing leaves this function but the decoded string: the pixels are a
/// [`Zeroizing`] local that dies here, which is [`PRIVACY_LINE`]'s second
/// clause on this route.
pub fn decode_image_with(
    seams: &ImageSeams,
    bytes: &[u8],
) -> Result<Zeroizing<String>, PickerRefusal> {
    let (rgba, width, height) = png_to_rgba(bytes)?;
    (seams.decode)(&rgba, width, height).ok_or(PickerRefusal::NoCode(CodeSource::Image))
}

/// [`decode_image_with`] against the real decoder.
pub fn decode_image(bytes: &[u8]) -> Result<Zeroizing<String>, PickerRefusal> {
    decode_image_with(&ImageSeams::production(), bytes)
}

/// Reads the file the user pointed at, and decodes it.
///
/// The **only** line in this feature that touches the filesystem, and it only
/// reads. The bytes are a [`Zeroizing`] for `png_to_rgba`'s reason; nothing is
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
        OtpRefusal::UnknownParameter(key) => format!(
            "That URI carries a parameter Deskwarden does not know: {key}. It is refused rather \
             than ignored, because a code saved from it would be wrong and nothing on screen \
             would say why."
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
        }
    }

    /// **A decoded payload becomes what is in the field.**
    ///
    /// The scanned routes do not get a confirmation card of their own: the
    /// decoded URI is put in [`Self::typed`], and 6d's field, its validity
    /// line and 6c's confirmation then say the same things about it that they
    /// say about a pasted one. That is what makes "the same 6c confirmation"
    /// true rather than merely intended -- there is one card, drawn by one
    /// function, from one string.
    ///
    /// It also means a hostile QR is refused by exactly the sentence a hostile
    /// paste is: [`parse_otpauth`] is the only validator either reaches.
    ///
    /// [`Self::revealed`] is put back to `false`, because the seed that was on
    /// screen a moment ago is not this one.
    pub fn accept_decoded(&mut self, text: Zeroizing<String>) {
        self.typed = text;
        self.scanned = true;
        self.stage = Stage::Manual;
        self.refusal = None;
        self.revealed = false;
    }

    /// Back to 6a, with the field emptied.
    ///
    /// **Emptied, not kept**: what is in it is a seed, and a form the user
    /// stepped away from is not a place to leave one resident. The
    /// `Zeroizing` is replaced rather than cleared in place so the old
    /// allocation is wiped on drop.
    pub fn back_to_picker(&mut self) {
        self.typed = Zeroizing::new(String::new());
        self.scanned = false;
        self.revealed = false;
        self.stage = Stage::Picker;
        self.refusal = None;
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

/// One `Label   Value` row of the confirmation.
fn field_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(84.0, 16.0), |ui| {
            note(ui, label, theme::TEXT_FAINT);
        });
        ui.label(egui::RichText::new(value).size(12.0).color(theme::INK));
    });
}

/// The manual-entry field, the two controls, the 6c confirmation and the
/// buttons.
///
/// `now_unix` is the clock, passed in rather than read here: the live code is
/// the one thing on this surface that changes without the user touching it,
/// and a `SystemTime::now()` inside this function would make every assertion
/// about the code untestable. `vault_window::mod` reads the clock once per
/// frame and hands it down.
pub fn draw_add_form(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    // Deferred to after the card, because `state` is borrowed inside it and
    // `back_to_picker` replaces the very `Zeroizing` the field is editing.
    let mut back_to_picker = false;
    card(ui, |ui| {
        ui.label(egui::RichText::new(HEADING).size(14.0).color(theme::INK).strong());
        ui.add_space(2.0);
        note(ui, &state.item_name, theme::TEXT_MUTED);
        ui.add_space(10.0);

        // **A scanned payload is NOT put in a text field.** See
        // [`CODE_READ_LABEL`]: a `TextEdit` paints what it holds, and what a
        // decoder hands over holds `secret=` in the middle of it. The row 6c
        // draws instead says what was read without saying what it was.
        if state.scanned {
            field_row(ui, CODE_READ_LABEL, CODE_READ_KIND);
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut *state.typed)
                    .hint_text(SECRET_HINT)
                    .desired_width(f32::INFINITY),
            );
        }

        let reading = read_field(&state.typed, state.digits, state.period);
        if let Some(line) = validity_line(&reading) {
            ui.add_space(4.0);
            let colour = match reading {
                Reading::Refused(_) => theme::ERROR,
                _ => theme::TEXT_MUTED,
            };
            ui.label(egui::RichText::new(line).size(11.0).color(colour));
        }

        ui.add_space(10.0);
        let (shown_digits, shown_period, controls_live) =
            controls_for(&reading, &state.typed, state.digits, state.period);
        ui.horizontal(|ui| {
            note(ui, DIGITS_LABEL, theme::TEXT_FAINT);
            for choice in DIGITS_CHOICES {
                if ui
                    .add_enabled(
                        controls_live,
                        egui::Button::new(
                            egui::RichText::new(choice.to_string()).size(12.0).color(theme::INK),
                        )
                        .selected(shown_digits == choice)
                        .min_size(egui::vec2(34.0, 22.0)),
                    )
                    .clicked()
                {
                    state.digits = choice;
                }
            }
            ui.add_space(10.0);
            note(ui, PERIOD_LABEL, theme::TEXT_FAINT);
            for choice in PERIOD_CHOICES {
                if ui
                    .add_enabled(
                        controls_live,
                        egui::Button::new(
                            egui::RichText::new(format!("{choice} s"))
                                .size(12.0)
                                .color(theme::INK),
                        )
                        .selected(shown_period == choice)
                        .min_size(egui::vec2(40.0, 22.0)),
                    )
                    .clicked()
                {
                    state.period = choice;
                }
            }
            if !controls_live {
                ui.add_space(8.0);
                note(ui, PARAMETERS_FROM_URI, theme::TEXT_FAINT);
            }
        });

        if let Reading::Ok(auth) = &reading {
            ui.add_space(12.0);
            draw_confirmation(ui, auth, state, now_unix);
        }

        if state.already_has_code {
            ui.add_space(10.0);
            // The error colour and the same size as everything else on the
            // card, not fine print: it is the sentence that decides whether
            // the button below it is a mistake.
            ui.label(egui::RichText::new(REPLACE_WARNING).size(12.0).color(theme::ERROR));
        }

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can_save(&reading),
                    egui::Button::new(
                        egui::RichText::new(submit_label(state.already_has_code))
                            .size(12.0)
                            .color(theme::INK),
                    )
                    .min_size(egui::vec2(112.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = TotpAddAction::Save;
            }
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Cancel").size(12.0).color(theme::TEXT_MUTED),
                    )
                    .min_size(egui::vec2(72.0, BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = TotpAddAction::Cancel;
            }
            // **The way back to 6a**, and the reason this form is not a dead
            // end when the user arrived at it by scanning: a decode that
            // produced the wrong card, or a seed typed off the wrong line, is
            // fixed by choosing a route again rather than by cancelling out
            // of the whole feature and re-opening it.
            ui.add_space(8.0);
            if theme::link_label(ui, OTHER_WAYS_LABEL, 11.0).clicked() {
                back_to_picker = true;
            }
        });
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
// the ✕ is `theme::close_glyph`, and the keycap is
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

/// The gap between a dead row's title and the reason beside it.
///
/// The design has no such element -- 6a draws all four routes live. See
/// [`WEBCAM_REASON`] for why this app draws one of them off and says so, and
/// [`route_row`] for the inks it says it in.
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
/// [`Route::Webcam`] answers [`TotpAddAction::None`] and its row is drawn
/// disabled, so there are two independent reasons it does nothing.
pub fn action_for(route: Route) -> TotpAddAction {
    match route {
        Route::ScanRegion => TotpAddAction::ScanRegion,
        Route::ImageFile => TotpAddAction::OpenImage,
        // Handled in the picker itself: it is a stage change and not something
        // the caller has to do.
        Route::ByHand | Route::Webcam => TotpAddAction::None,
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
/// a number, and it is also what makes the deferred row work.
/// [`WEBCAM_DETAIL`] is longer than anything 6a puts on a fourth row and
/// wraps to two lines, and the pair of hardcoded heights this replaced (46
/// for a live row, 62 for the dead one) were two guesses that had already
/// been corrected once against a render.
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
/// **The ordinals are not key bindings and are not drawn as keycaps.** 6a
/// sets them as bare monospace text where it sets the ↵ in a filled chip, and
/// the difference is real: `Key::Num2` is the key that OPENS this modal
/// (`vault_window::ADD_TOTP_KEY`, held to being bound exactly once anywhere
/// in this crate by `the_add_code_chord_is_a_key_no_other_binding_takes`), so
/// a picker that also answered a bare 2 would be advertising the digit that
/// got the user here as the digit that leaves by another door.
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
        lay_out(ui, WEBCAM_REASON, text_width, body_face(ROW_SUB_PX, sub_ink))
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

/// 6a's card: white, `border-radius: 12px`, a [`theme::BORDER_STRONG`] hairline
/// round it, and the shadow that lifts it off the scrim.
///
/// The border is stroked twice, on the `Frame` and again over everything, for
/// `theme::modal_card`'s measured reason: the `Frame`'s own stroke is what
/// the bands are laid out against, and the footer's tint is painted out over
/// it so that no sliver of white card shows in the lower corners. Dropping
/// the first would shrink the layout rect; dropping the second would put the
/// white line back along the bottom curve.
fn picker_card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
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
            ui.set_width(PICKER_WIDTH - CARD_STROKE * 2.0);
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

    // The ✕ is `theme::close_glyph` and not a typed U+2715: that codepoint is
    // in neither Archivo nor egui's fallback stack and reaches the screen as
    // a tofu box, which is the measurement `theme` records beside it.
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(content.left() + HEADER_PAD_X, content.top() + HEADER_PAD_Y),
                egui::pos2(content.right() - HEADER_PAD_X, content.bottom() - HEADER_PAD_Y),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let close = theme::close_glyph(&mut inner);
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
    let width = ui.available_width();
    let text = lay_out(
        ui,
        PRIVACY_LINE,
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
    picker_card(ui, |ui| {
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
                // **What the ↵ on the default row means.** The design draws a
                // keycap and not an ordinal on that one row, and a keycap for
                // a key nothing answers is the kind of promise this file
                // refuses elsewhere (see [`ROUTES`] on "PNG, JPG"). Enter is
                // unbound everywhere else in this window's production, so it
                // is answered here, by the row the design says it belongs to.
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = Some(ROUTES[DEFAULT_ROW].route);
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
fn draw_confirmation(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd, now_unix: u64) {
    ui.label(egui::RichText::new(CONFIRM_HEADING).size(12.0).color(theme::INK).strong());
    ui.add_space(6.0);

    // The live code first: it is what the user is here to compare, and a
    // confirmation that buries it under four label rows is a confirmation
    // nobody makes.
    if let Some(code) = code_at(auth, now_unix) {
        draw_code_panel(ui, auth, &code, now_unix);
        ui.add_space(CONFIRM_GAP);
    }

    draw_field_table(ui, auth, state);
}

/// **6c's live-code panel.** The label and the code down the left, the
/// countdown stack right-aligned beside them.
fn draw_code_panel(ui: &mut egui::Ui, auth: &OtpAuth, code: &str, now_unix: u64) {
    egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
        .corner_radius(CornerRadius::same(CODE_PANEL_RADIUS))
        .inner_margin(egui::Margin::symmetric(CODE_PANEL_PAD_X, CODE_PANEL_PAD_Y))
        .show(ui, |ui| {
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
                    ui.label(theme::letterspaced_mono(
                        &grouped_code(code),
                        CODE_PX,
                        CODE_PX * CODE_TRACKING,
                        theme::BLUE_DEEP,
                    ));
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

/// 6c's countdown track: a 96 by 4 rail of [`theme::BLUE_EDGE`] with
/// `fraction` of it filled in [`theme::BLUE`] from the left.
fn draw_countdown(ui: &mut egui::Ui, fraction: f32) {
    let (track, _) = ui.allocate_exact_size(
        egui::vec2(COUNTDOWN_WIDTH, COUNTDOWN_HEIGHT),
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

    egui::Area::new(egui::Id::new("totp-add-modal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_max_width(stage_width(state.stage));
            draw_stage(ui, state, now_unix)
        })
        .inner
}

/// How wide the card is at each stage.
///
/// 6a's panel is 470 and 6d's composer column is [`MODAL_WIDTH`], and the two
/// are genuinely different surfaces rather than one surface measured twice:
/// the picker carries a 34px tile, two columns of type and an affordance on
/// every one of four rows, where the by-hand form is a single field. Sizing
/// the picker to 380 is what put its subtitles on two lines.
fn stage_width(stage: Stage) -> f32 {
    match stage {
        Stage::Picker => PICKER_WIDTH,
        Stage::Scanning | Stage::Manual => MODAL_WIDTH,
    }
}

/// **The one place that decides which half of this surface is on screen**, so
/// no caller has to know there are three.
pub fn draw_stage(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    match state.stage {
        Stage::Picker => draw_picker(ui, state).action,
        Stage::Scanning => {
            draw_scanning(ui, state);
            TotpAddAction::None
        }
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
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        state.typed = Zeroizing::new(UNUSUAL.to_string());
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
        // Typed in the spaced grouping a site prints, so the text in the FIELD
        // is not the normalised seed the confirmation row holds. Without that,
        // the assertion below would be defeated by the user's own typing being
        // painted back at them by the `TextEdit` -- which is not the row this
        // test is about, and which is the shape a masked-secret test in this
        // crate has been blind to before.
        state.typed = Zeroizing::new("JBSW Y3DP EHPK 3PXP".to_string());
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
        assert!(painted.has(CONFIRM_HEADING), "the modal dropped the confirmation");
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
        assert_eq!(ROUTES[0].title, "Scan a region of my screen");
        assert_eq!(ROUTES[1].title, "Open an image file");
        assert_eq!(ROUTES[2].title, "Enter the secret by hand");
        assert_eq!(ROUTES[3].title, "Use a webcam");
        for row in &ROUTES {
            assert!(!row.subtitle.trim().is_empty(), "{} has no line under it", row.title);
        }
        // **PNG, and not the design's "PNG, JPG".** The dialog's filter says
        // the same, and the two are held to each other from `file_picker`'s
        // own test. A row promising a format the decoder cannot read is a
        // promise broken one click later.
        assert!(
            ROUTES[1].subtitle.contains("PNG") && !ROUTES[1].subtitle.contains("JPG"),
            "the image row offers a format this app cannot decode: {}",
            ROUTES[1].subtitle
        );
        assert_eq!(crate::file_picker::IMAGE_EXTENSION, "png");
    }

    /// **The webcam row is present and disabled; every other row is not.**
    ///
    /// Both halves matter. A row merely absent from an `enabled` check would
    /// satisfy an assertion written one way round only.
    #[test]
    fn the_webcam_row_is_the_only_one_that_is_deferred_and_it_says_why() {
        let dead: Vec<Route> = ROUTES.iter().filter(|r| !r.enabled).map(|r| r.route).collect();
        assert_eq!(dead, vec![Route::Webcam], "the wrong set of routes is disabled");
        let live: Vec<Route> = ROUTES.iter().filter(|r| r.enabled).map(|r| r.route).collect();
        assert_eq!(
            live,
            vec![Route::ScanRegion, Route::ImageFile, Route::ByHand],
            "a route this task shipped is drawn dead"
        );
        assert_eq!(WEBCAM_REASON, "Not in this version");
        assert!(
            WEBCAM_DETAIL.contains("scan a region"),
            "the deferred row does not point at the route that replaces it: {WEBCAM_DETAIL}"
        );
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
        assert!(totp
            .contains("let mut buf = Zeroizing::new(vec![0u8; reader.output_buffer_size()]);"));
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
    /// black-and-white QR really does produce, and the arm of `png_to_rgba`
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
    /// the PNG half produced rather than trust that it produced any.
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
    fn a_file_that_is_not_a_png_is_refused_as_a_file_and_not_as_a_missing_code() {
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
        // bytes rather than about `png_to_rgba` refusing everything.
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
        assert!(
            !frame.painted.has(HEADING),
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
        assert!(frame.painted.has(WEBCAM_REASON), "the dead row does not say it is deferred");
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

        // The three one-line rows are all 62 tall -- 6a's own number, which is
        // 12px of padding either side of the 36px tile plus the 1px border --
        // and the deferred row is taller because its reason wraps. Asserted
        // together so a change that grew every row would still red.
        let live: Vec<f32> =
            frame.rows.iter().take(3).map(|(_, rect)| rect.height()).collect();
        for height in &live {
            assert!((height - 62.0).abs() <= 1.0, "a live row is {height} tall and 6a's is 62");
        }
        assert!(
            frame.row(Route::Webcam).height() > live[0] + 1.0,
            "the deferred row is no taller than a one-line row, so its two-line reason is \
             overflowing rather than fitting"
        );
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

    /// **The dead row is dead when pressed, and not merely greyed.**
    #[test]
    fn pressing_the_webcam_row_does_nothing_at_all() {
        let mut state = TotpAdd::opening("id-1", "Git Host", false);
        let picker = Picker::new();
        let laid_out = picker.idle(&mut state);
        let at = laid_out.row(Route::Webcam).center();

        let after = picker.click(&mut state, at);
        assert_eq!(after.action, TotpAddAction::None);
        assert_eq!(state.stage, Stage::Picker, "the deferred row moved the form somewhere");
        assert_eq!(state.refusal, None);
        // Control: the row IS on screen and IS where this pressed, so the
        // three assertions above are about a dead control rather than about a
        // click that landed on nothing.
        assert!(after.painted.has("Use a webcam"));
        assert!(laid_out.row(Route::Webcam).width() > 1.0);
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
}

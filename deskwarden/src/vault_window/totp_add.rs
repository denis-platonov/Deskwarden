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
pub const HEADING: &str = "Add a one-time code";

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
pub const ADD_TOTP_LABEL: &str = "Add a one-time code";
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

/// 6a's heading over the four routes.
pub const PICKER_HEADING: &str = "How to add it";

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

/// The parameters row of the confirmation, spelled out.
pub fn parameters_line(auth: &OtpAuth) -> String {
    format!(
        "TOTP \u{b7} {} \u{b7} {} digits \u{b7} {} s",
        auth.algorithm.canonical(),
        auth.digits,
        auth.period
    )
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
    let period = auth.period.max(1);
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
// Design 6a's surface
// ---------------------------------------------------------------------------

/// The height of one route row.
const ROW_HEIGHT: f32 = 46.0;

/// The height of the deferred row, which carries two lines rather than one.
///
/// Measured against the rendered surface rather than guessed: at
/// [`MODAL_WIDTH`] [`WEBCAM_DETAIL`] wraps to two lines, and a row sized for
/// one puts the second on top of whatever is under it. `ui_preview`'s
/// `totp_add_picker` shot is what this was corrected from.
const DEAD_ROW_HEIGHT: f32 = 62.0;

/// How tall `row` is drawn.
fn row_height(row: &RouteRow) -> f32 {
    if row.enabled {
        ROW_HEIGHT
    } else {
        DEAD_ROW_HEIGHT
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
}

/// What pressing a route means. A pure function so the routing is a thing a
/// test can enumerate rather than four arms buried in a click handler.
///
/// [`Route::Webcam`] answers [`TotpAddAction::None`] and its row is drawn
/// disabled, so there are two independent reasons it does nothing.
pub fn action_for(route: Route) -> TotpAddAction {
    match route {
        Route::ScanRegion => TotpAddAction::ScanRegion,
        // `ImageFile` is drawn and reports nothing yet: its dialog, its decode
        // and its refusals are Task 7, which lands next. `ByHand` and `Webcam`
        // are handled in the picker itself -- one is a stage change, the other
        // is deferred.
        Route::ImageFile | Route::ByHand | Route::Webcam => TotpAddAction::None,
    }
}

/// One route row: a title, a line under it, and a hit area.
fn route_row(ui: &mut egui::Ui, row: &RouteRow) -> (egui::Response, egui::Rect) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, row_height(row)),
        if row.enabled { egui::Sense::click() } else { egui::Sense::hover() },
    );
    let painter = ui.painter();
    if row.enabled && response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::BLUE_WASH);
    } else {
        painter.rect_filled(rect, CornerRadius::same(6), theme::CARD_TINT);
    }
    // A dead row is drawn in the muted inks rather than hidden: the fact that
    // this route exists and is off is the whole content of the row.
    let (title_ink, sub_ink) = if row.enabled {
        (theme::INK, theme::TEXT_MUTED)
    } else {
        (theme::TEXT_GHOST, theme::TEXT_GHOST)
    };
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(10.0, 6.0)))
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );
    inner.horizontal(|ui| {
        ui.label(egui::RichText::new(row.title).size(12.5).color(title_ink).strong());
        if !row.enabled {
            ui.add_space(6.0);
            note(ui, WEBCAM_REASON, theme::TEXT_GHOST);
        }
    });
    note(&mut inner, row.subtitle, sub_ink);
    (response, rect)
}

/// **Design 6a.** Four routes in the design's order, the reason the fourth is
/// dead, and the privacy line under all of them.
///
/// The privacy line is **pinned to the bottom of the card**, below the rows
/// and above nothing, because that is the last thing read before a route is
/// chosen and because a claim about what happens to the pixels belongs beside
/// the button that captures them.
pub fn draw_picker(ui: &mut egui::Ui, state: &mut TotpAdd) -> PickerFrame {
    let mut action = TotpAddAction::None;
    let mut rows = Vec::with_capacity(ROUTES.len());
    let mut go_manual = false;
    card(ui, |ui| {
        ui.label(egui::RichText::new(HEADING).size(14.0).color(theme::INK).strong());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            note(ui, ADDING_TO_LABEL, theme::TEXT_FAINT);
            ui.label(egui::RichText::new(&state.item_name).size(11.0).color(theme::TEXT_MUTED));
        });
        ui.add_space(10.0);
        ui.label(egui::RichText::new(PICKER_HEADING).size(12.0).color(theme::INK).strong());
        ui.add_space(6.0);

        for row in &ROUTES {
            let (response, rect) = route_row(ui, row);
            rows.push((row.route, rect));
            if response.clicked() {
                if row.route == Route::ByHand {
                    go_manual = true;
                } else {
                    action = action_for(row.route);
                }
            }
            ui.add_space(6.0);
        }

        // The last refusal, if any. Above the privacy line rather than below
        // it, so the sentence that says what went wrong sits next to the rows
        // that can be pressed again.
        if let Some(refusal) = state.refusal {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(refusal.sentence()).size(11.5).color(theme::ERROR));
        }

        ui.add_space(10.0);
        ui.label(egui::RichText::new(PRIVACY_LINE).size(11.0).color(theme::TEXT_MUTED));

        ui.add_space(10.0);
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
    });
    if go_manual {
        state.stage = Stage::Manual;
        state.refusal = None;
    }
    PickerFrame { action, rows }
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

/// **Design 6c**, and the half every later route shares.
///
/// Split out of [`draw_add_form`] so the scanned routes of tasks 4--7 paint
/// the same card from the same function rather than a second one that drifts.
fn draw_confirmation(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd, now_unix: u64) {
    ui.label(egui::RichText::new(CONFIRM_HEADING).size(12.0).color(theme::INK).strong());
    ui.add_space(6.0);

    // The live code first: it is what the user is here to compare, and a
    // confirmation that buries it under four label rows is a confirmation
    // nobody makes.
    if let Some(code) = code_at(auth, now_unix) {
        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(84.0, 16.0), |ui| {
                note(ui, CODE_ROW_LABEL, theme::TEXT_FAINT);
            });
            ui.label(
                egui::RichText::new(grouped_code(&code).to_string())
                    .size(20.0)
                    .color(theme::BLUE)
                    .strong(),
            );
            ui.add_space(8.0);
            note(
                ui,
                &format!("refreshes in {} s", seconds_left(auth, now_unix)),
                theme::TEXT_MUTED,
            );
        });
        ui.add_space(2.0);
        note(ui, MATCH_QUESTION, theme::TEXT_MUTED);
        ui.add_space(8.0);
    }

    field_row(ui, ISSUER_ROW_LABEL, auth.issuer.as_deref().unwrap_or("\u{2014}"));
    field_row(ui, ACCOUNT_ROW_LABEL, auth.account.as_deref().unwrap_or("\u{2014}"));

    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(84.0, 16.0), |ui| {
            note(ui, SECRET_ROW_LABEL, theme::TEXT_FAINT);
        });
        let shown = if state.revealed {
            auth.secret.to_string()
        } else {
            masked(&auth.secret)
        };
        ui.label(egui::RichText::new(shown).size(12.0).color(theme::INK));
        ui.add_space(8.0);
        if theme::link_label(ui, if state.revealed { HIDE_LABEL } else { REVEAL_LABEL }, 11.0)
            .clicked()
        {
            state.revealed = !state.revealed;
        }
    });

    field_row(ui, PARAMETERS_ROW_LABEL, &parameters_line(auth));
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
            ui.set_max_width(MODAL_WIDTH);
            draw_stage(ui, state, now_unix)
        })
        .inner
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
        // A row promising a format the decoder cannot read is a promise
        // broken one click later; Task 7 holds the dialog's filter to this
        // same answer.
        assert!(
            ROUTES[1].subtitle.contains("PNG") && !ROUTES[1].subtitle.contains("JPG"),
            "the image row offers a format this app cannot decode: {}",
            ROUTES[1].subtitle
        );
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
        // No filesystem call at all: the region route has no business with
        // one, and Task 7 will add exactly one read.
        let totp = &sources.iter().find(|(n, _)| *n == "totp_add.rs").unwrap().1;
        assert_eq!(totp.matches("std::fs::").count(), 0, "the picker grew a filesystem call");
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
    /// will be Task 7's own, and are held there.
    #[test]
    fn the_captured_pixels_are_wiped_and_never_handed_out() {
        let sources = scan_route_sources();
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

    /// One frame of [`Picker`].
    struct PickerRun {
        action: TotpAddAction,
        rows: Vec<(Route, egui::Rect)>,
        painted: Painted,
    }

    impl PickerRun {
        fn row(&self, route: Route) -> egui::Rect {
            self.rows
                .iter()
                .find(|(r, _)| *r == route)
                .unwrap_or_else(|| panic!("{route:?} was not drawn at all"))
                .1
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
                    egui::vec2(420.0, 900.0),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&self, state: &mut TotpAdd, events: Vec<egui::Event>) -> PickerRun {
            let mut reported: Option<PickerFrame> = None;
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                ui.set_max_width(MODAL_WIDTH);
                reported = Some(draw_picker(ui, state));
            });
            let reported = reported.expect("run_ui runs the closure once");
            let mut painted = Painted(Vec::new());
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut painted);
            }
            assert!(
                !painted.0.is_empty(),
                "the picker painted no text at all, so every assertion over this list would \
                 pass against nothing"
            );
            PickerRun { action: reported.action, rows: reported.rows, painted }
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

        assert!(frame.painted.has(HEADING), "the card has no heading");
        assert!(
            frame.painted.has(ADDING_TO_LABEL),
            "the picker does not say what it is adding to"
        );
        assert!(frame.painted.has("Git Host"), "the item it is adding to is not named");
        assert!(frame.painted.has(PICKER_HEADING));
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
        assert!(painted.has(PICKER_HEADING), "the modal did not open on 6a: {:?}", painted.0);
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
            "overlay.show(ui.ctx())",
        ] {
            assert!(code.contains(needle), "`{needle}` is not in the vault window's frame");
        }
        // Positive control on the split: a needle only ever spelled out below
        // the marker is not found above it.
        assert!(!code.contains("the_vault_window_opens_the_overlay_and_applies"));
    }
}

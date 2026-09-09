//! **The icon the owner chose for one vault item**, as it is stored on that
//! item.
//!
//! `favicon` answers "which host would this item's picture come from"; this
//! module answers the question that stands in front of it -- "did the owner
//! already say what this item's picture is". When it answers, nothing in
//! `favicon`'s routing runs: no origin fetch, no icon proxy, no third party.
//!
//! # Why the choice lives on the ITEM
//!
//! It syncs. A preference file, or a second on-disk cache keyed by item id,
//! would make the choice a property of one Windows installation -- lost to a
//! reinstall and invisible on the user's other machine. A custom field on the
//! cipher travels wherever the vault does, which is what makes "I set my
//! bank's real logo" survive.
//!
//! The idiom is [`crate::app_match::APP_MATCH_FIELD_NAME`]'s, deliberately and
//! down to the details: one namespaced `deskwarden:` field, one JSON value,
//! `serde` on both ends, and a reader that answers `None` -- never a panic and
//! never a deletion -- for a value it cannot make sense of.
//!
//! # What is NOT here
//!
//! No filesystem, no network and no `egui`. Reading the picked file is
//! [`read_icon_file`] and that is the whole of the I/O; fetching a chosen URL
//! is [`crate::favicon::fetch_chosen_icon`], on the loader's background
//! thread, and turning pixels into a texture is `vault_window`'s. That split
//! is what lets every rule below be tested against bytes built arithmetically.

use crate::favicon;
use crate::vault_bridge::VaultItem;
use serde::{Deserialize, Serialize};

/// The custom field an item's chosen icon is stored on.
///
/// Namespaced the way [`crate::app_match::APP_MATCH_FIELD_NAME`],
/// [`crate::favicon::BANK_DOMAIN_FIELD`] and
/// [`crate::vault_bridge::BILLING_ZIP_FIELD`] are, and declared **once** for
/// the same reason those are: the menu that writes it, the loader that reads
/// it and the clear that removes it are in three different modules, and two
/// spellings of a field name that must match is a defect that shows up only
/// as a picture the app appears to have forgotten.
///
/// **`deskwarden:icon`, not `deskwarden:icon-url` plus `deskwarden:icon-png`
/// -- ONE key with a discriminator inside it.** The user made ONE choice, and
/// two keys make "both are set" a representable state that nothing writes and
/// every reader has to arbitrate. That arbitration would be invented
/// precedence: whichever of the two the reader happened to check first would
/// silently win, and the loser would sit in the vault looking set. One key
/// makes "does this item have a chosen icon" a single question with a single
/// answer, and makes clearing a single removal that cannot half-succeed.
///
/// The precedent points the same way. `deskwarden:bank-domain` and
/// `deskwarden:billing-zip` are separate keys because they are separate
/// FACTS about a card that can both be true at once; `deskwarden:app-match`
/// is one key holding structured JSON because it is one binding with several
/// parts. A chosen icon is the second shape, not the first.
pub const ICON_FIELD_NAME: &str = "deskwarden:icon";

/// The most bytes [`read_icon_file`] will take off disk for one picked image.
///
/// **The same number the direct icon fetch already enforces**
/// (`favicon`'s `MAX_DIRECT_ICON_BYTES`), and that is the argument rather
/// than a coincidence: a file the user pointed at is no more trustworthy than
/// bytes off the wire. Both end up in the same `decode_rgba`, so one cap for
/// both is one rule to get right. A 256x256 32-bit ICO is about 270 KB and a
/// large PNG logo is well under a megabyte, so this is comfortable room for
/// any real icon and no room for a photograph.
///
/// **What it bounds, exactly.** It bounds the bytes read and the bytes handed
/// to the decoder. It does not bound the decoder's own allocation: a 2 MiB
/// PNG can declare 4000x4000 and `png`'s reader will size an output buffer
/// for 64 MB of RGBA. That exposure is not introduced here -- it is the one
/// the direct-fetch path has always accepted under this same cap, from hosts
/// nobody vetted -- and closing it means a dimension check inside
/// `favicon::decode_rgba_unscaled`, which belongs to that function and to
/// both of its callers rather than to this one.
///
/// What a 4000x4000 image does NOT do is reach the vault: whatever decodes is
/// resampled to at most [`crate::favicon`]'s 64px target before a single byte
/// is stored. See [`choice_from_image_bytes`].
pub const MAX_PICKED_ICON_BYTES: usize = 2 * 1024 * 1024;

/// The most bytes of re-encoded PNG that may be written into a vault item.
///
/// **Unreachable by construction, and kept as a checked number anyway.** The
/// stored image has already been through
/// `favicon::resample_for_display`, so it is at most 64x64 -- 16,384 bytes of
/// raw RGBA. A PNG of that cannot exceed it by more than its header, palette
/// and per-scanline filter bytes, which is well inside the 8 KiB of headroom
/// here. So this cap has no behaviour to describe on any real image.
///
/// It exists because "the resample guarantees it" is an argument, and what a
/// vault write deserves is a number. A future change to `ICON_TARGET_PX`, or
/// a decoder that one day hands back something larger than it promised, turns
/// a silent 400 from the server (or a cipher nobody can open in the web
/// vault) into [`IconRefusal::TooBigToStore`] and a sentence. Base64 inflates
/// this by a third on the wire, so the field's value is bounded at about
/// 32 KB.
pub const MAX_STORED_ICON_BYTES: usize = 24 * 1024;

/// **The icon the owner chose, as it sits in the item's custom field.**
///
/// Two variants and a `kind` discriminator, so the field is self-describing:
/// a reader never has to guess whether a string is a URL or a payload by
/// looking at it, and a value written by a later build that grows a third
/// variant fails to parse *as a whole* rather than being mistaken for one of
/// these two.
///
/// **What a failure to parse does, and what it must never do.** Every reader
/// here answers `None`, and `None` means "this item has no usable chosen
/// icon" -- so the item falls back to the automatic one it would have had
/// anyway. Nothing removes the field, rewrites it, or truncates it. That is
/// the same contract [`crate::app_match::AppMatch::from_field_value`] has and
/// it exists for the same reason: this JSON is in a real user's vault, it can
/// be edited by any Bitwarden client, and a client that "repaired" what it
/// could not read would be destroying data on a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum IconChoice {
    /// A URL the user typed, **stored exactly as given**.
    ///
    /// Verbatim for [`crate::app_match::AppMatch::args`]'s reason: anything
    /// this app did to "normalise" it -- lower-casing a host, appending a
    /// slash, re-encoding a query -- would be a guess the user cannot see and
    /// cannot undo, against a URL only their server has to agree about. The
    /// one thing done to it is a `trim`, at the point it is accepted, because
    /// a trailing space is a typing artefact and never part of a URL.
    ///
    /// **Not a promise that it is fetchable.** [`choice_from_url`] checks the
    /// SHAPE -- a scheme this app speaks and a dotted host -- and nothing
    /// more. Whether the host answers, and whether what it answers is an
    /// image, is decided by the fetch, and a failure there leaves the item on
    /// its automatic icon rather than on nothing.
    #[serde(rename = "url")]
    Url { url: String },
    /// A picked file, resampled and re-encoded to PNG, then base64.
    ///
    /// **The bytes and not the path.** A path would be a per-machine
    /// reference that syncs as a broken promise: the other device has no
    /// `C:\Users\...\logo.png`, and this machine will not have it either
    /// after the user tidies their Downloads folder. Carrying the pixels is
    /// the only shape in which "I picked a file" survives a reinstall.
    ///
    /// Standard base64 with padding, through
    /// [`crate::record::seal::base64_into`] -- the crate's existing encoder,
    /// **not a third copy of one**. `record::seal` and `send.rs` already
    /// carry twins of it and a fourth would be the defect those two are one
    /// short of.
    #[serde(rename = "png")]
    Png { png: String },
}

impl IconChoice {
    /// The JSON this is stored as. Infallible for the same reason
    /// [`crate::app_match::AppMatch::to_field_value`] is: both variants are
    /// plain strings and neither can fail to serialize.
    pub fn to_field_value(&self) -> String {
        serde_json::to_string(self).expect("IconChoice always serializes")
    }

    /// The inverse. `Err` for anything this build cannot read -- which every
    /// caller turns into `None` and never into a write.
    pub fn from_field_value(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value)
    }

    /// The chosen URL, for the one caller that has to make a request out of
    /// it. `None` for a stored picture, which is never fetched from anywhere.
    pub fn url(&self) -> Option<&str> {
        match self {
            IconChoice::Url { url } => Some(url),
            IconChoice::Png { .. } => None,
        }
    }

    /// The stored picture's pixels, ready for `egui::ColorImage`, or `None`
    /// for a URL choice and for base64 or PNG bytes that do not decode.
    ///
    /// **Through `favicon::decode_rgba`, the very function a fetched icon
    /// goes through, and that is not tidiness.** That is where the ICO
    /// handling, the colour-type normalisation, the downscale and the
    /// blank-image refusal live. Bytes that have been round-tripped through a
    /// vault, a server and possibly another client's editor are exactly as
    /// untrusted as bytes off the wire, and a second decode path for them
    /// would be a second place all four of those rules have to be got right.
    pub fn pixels(&self) -> Option<(usize, usize, Vec<u8>)> {
        let IconChoice::Png { png } = self else {
            return None;
        };
        favicon::decode_rgba(&crate::record::seal::base64_from(png)?)
    }
}

/// **Every way choosing an icon can fail before anything is written.**
///
/// Separate variants rather than one "that did not work", for
/// `totp_add::PickerRefusal`'s reason and in its shape: a generic refusal
/// teaches the user to retry the thing that will not work. Each of these is
/// recovered from differently -- pick a smaller file, pick a different file,
/// fix the address -- and the sentence has to say which.
///
/// Carries no bytes and no path, so no arm of [`Self::sentence`] can print
/// something out of the user's vault or off their disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconRefusal {
    /// The file could not be opened at all.
    Unreadable,
    /// The file is bigger than [`MAX_PICKED_ICON_BYTES`].
    TooLarge,
    /// The bytes are not an image `favicon::decode_rgba` can read -- which
    /// includes a fully transparent one, because that is not an icon.
    NotAnImage,
    /// The re-encoded PNG came out over [`MAX_STORED_ICON_BYTES`]. See that
    /// constant: no real image reaches this.
    TooBigToStore,
    /// The typed address is not an `http`/`https` URL with a host in it.
    BadUrl,
}

impl IconRefusal {
    /// The refusal as a sentence naming the reason, exhaustive with no
    /// catch-all -- `totp_add::PickerRefusal::sentence`'s rule.
    pub fn sentence(&self) -> String {
        match self {
            IconRefusal::Unreadable => "That file could not be opened. Check it is still where \
                 it was, and that you have permission to read it."
                .to_string(),
            IconRefusal::TooLarge => format!(
                "That file is larger than {} KB. Icons are small pictures \u{2014} pick a logo \
                 rather than a photograph.",
                MAX_PICKED_ICON_BYTES / 1024
            ),
            IconRefusal::NotAnImage => "That file isn't a picture Deskwarden can read. It reads \
                 PNG and ICO files; save the image as a PNG and choose it again."
                .to_string(),
            IconRefusal::TooBigToStore => "That picture is too big to store on the item, even \
                 after being shrunk. Choose a simpler image."
                .to_string(),
            IconRefusal::BadUrl => "That isn't an address Deskwarden can fetch. It needs to \
                 start with http:// or https:// and name a site, like \
                 https://example.com/logo.png."
                .to_string(),
        }
    }
}

/// **The chosen icon on `item`, or `None`.**
///
/// `None` covers three situations on purpose, because all three mean the same
/// thing to every caller: there is no field, the field is empty, or the
/// field's value is not something this build can read. The last one is the
/// interesting case and it is why this returns an `Option` rather than a
/// `Result`: a value written by a newer Deskwarden, or edited by hand in the
/// web vault, must leave the item wearing its ORDINARY icon and must not stop
/// a frame. It is logged at `debug` and then it is somebody else's problem --
/// specifically the user's, who can see the field and can clear it (see
/// [`has_icon_field`], which is what keeps the "Use the automatic icon" entry
/// on the menu for exactly this item).
pub fn chosen_icon(item: &VaultItem) -> Option<IconChoice> {
    let value = icon_field_value(item)?;
    match IconChoice::from_field_value(value) {
        Ok(choice) => Some(choice),
        Err(err) => {
            // The ERROR and the item id, never the value: the field is the
            // user's and this app does not copy it into a log file.
            log::debug!(
                "icon: item {}'s {ICON_FIELD_NAME} field is not a chosen icon this build can \
                 read ({err}); it keeps its ordinary icon",
                item.id
            );
            None
        }
    }
}

/// Whether `item` carries an [`ICON_FIELD_NAME`] field **at all**, readable
/// or not.
///
/// [`chosen_icon`]'s question is "what picture should this item wear";
/// this one is "is there something here to clear". They differ on exactly one
/// item -- one whose field holds a value this build cannot parse -- and that
/// item is the one that most needs the clearing entry, because the picture it
/// is wearing is not the one its field claims. Answering the two with one
/// function would have taken the way out away from the only user who is
/// stuck.
///
/// Modelled on [`crate::vault_bridge::has_app_match_field`], which exists for
/// the identical reason one field over.
pub fn has_icon_field(item: &VaultItem) -> bool {
    item.fields.iter().any(|f| f.name.as_deref() == Some(ICON_FIELD_NAME))
}

/// The raw, trimmed field value, or `None` when it is absent or blank.
fn icon_field_value(item: &VaultItem) -> Option<&str> {
    item.fields
        .iter()
        .find(|f| f.name.as_deref() == Some(ICON_FIELD_NAME))
        .and_then(|f| f.value.as_deref())
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

/// **A typed address to a stored choice.** Shape only.
///
/// Two checks, both borrowed rather than reinvented:
///
///  * the scheme is `http://` or `https://`. Nothing else is even a candidate
///    -- `file://` would make a vault field name a path on this machine, and
///    every other scheme is one `favicon`'s fetcher does not speak.
///  * [`crate::favicon::authority_from_uri`] answers `Some`, which is the
///    dotted-host rule the whole icon path already uses. Reusing it is what
///    keeps `http://localhost:3000/logo.png` refused HERE for the same reason
///    it is refused everywhere else, rather than accepted here and then
///    silently unfetched later.
///
/// The value stored is the trimmed input and not the parsed authority: see
/// [`IconChoice::Url`] for why nothing is normalised.
pub fn choice_from_url(text: &str) -> Result<IconChoice, IconRefusal> {
    let url = text.trim();
    // **A PROBE, lower-cased in the scheme and only in the scheme.**
    //
    // Schemes are case-insensitive per RFC 3986 and a browser accepts
    // `HTTPS://`, so refusing one would be this app inventing a rule. But
    // `favicon::authority_from_uri` strips the two lower-case prefixes and
    // nothing else -- deliberately, so that `androidapp://` and every other
    // scheme this app does not speak fail its dotted-host test rather than
    // parsing as a host. Handing it `HTTPS://example.com/logo.png` therefore
    // gets a refusal for the wrong reason.
    //
    // So the scheme is folded for the CHECK and the original is what is
    // stored. Nothing past the scheme is touched, because path case is
    // significant on most servers -- see [`IconChoice::Url`] on why nothing
    // here normalises what the user typed.
    let Some(len) = ["https://", "http://"]
        .iter()
        .find(|scheme| url.len() >= scheme.len() && url[..scheme.len()].eq_ignore_ascii_case(scheme))
        .map(|scheme| scheme.len())
    else {
        return Err(IconRefusal::BadUrl);
    };
    let probe = format!("{}{}", url[..len].to_ascii_lowercase(), &url[len..]);
    if favicon::authority_from_uri(&probe).is_none() {
        return Err(IconRefusal::BadUrl);
    }
    Ok(IconChoice::Url { url: url.to_string() })
}

/// **Picked image bytes to the thing that goes in the vault.**
///
/// The whole pipeline, in the one order that is safe:
///
/// 1. **The size cap first**, before a decoder ever sees the bytes. See
///    [`MAX_PICKED_ICON_BYTES`].
/// 2. **`favicon::decode_rgba`** -- the same function a fetched icon goes
///    through, which is where the ICO handling, the colour-type
///    normalisation, the downscale to 64px and the refusal of a blank image
///    live. This is the step that makes "a 4000x4000 PNG" into at most
///    64x64: nothing downstream has to trust the source's dimensions,
///    because nothing downstream ever sees them.
/// 3. **Re-encode**, from the resampled RGBA and never from the original
///    bytes. Storing the file the user picked would store whatever it
///    actually is -- an ICO, a 500 KB PNG with an ICC profile and an EXIF
///    block, an interlaced one -- and would put a container this app merely
///    tolerates into a field every other Bitwarden client will sync. What is
///    stored is a plain 8-bit RGBA PNG this app produced from pixels it had
///    already decoded, which is also the answer to "is there anything in that
///    file besides the picture": there is not, because none of it survives
///    the round trip through a pixel buffer.
/// 4. **The stored cap**, then base64.
pub fn choice_from_image_bytes(bytes: &[u8]) -> Result<IconChoice, IconRefusal> {
    if bytes.len() > MAX_PICKED_ICON_BYTES {
        return Err(IconRefusal::TooLarge);
    }
    let (width, height, rgba) = favicon::decode_rgba(bytes).ok_or(IconRefusal::NotAnImage)?;
    let encoded = encode_png(width, height, &rgba).ok_or(IconRefusal::NotAnImage)?;
    if encoded.len() > MAX_STORED_ICON_BYTES {
        return Err(IconRefusal::TooBigToStore);
    }
    let mut png = String::with_capacity(encoded.len().div_ceil(3) * 4);
    crate::record::seal::base64_into(&mut png, &encoded);
    Ok(IconChoice::Png { png })
}

/// Reads the file the user pointed at and turns it into a stored choice.
///
/// **The only line in this feature that touches the filesystem, and it only
/// reads** -- `totp_add::read_image_file`'s rule, restated here because the
/// two are the same shape and neither should grow a write.
///
/// The length is checked **twice**: once against the directory entry, so a
/// twenty-megabyte file is refused without being read into this process at
/// all, and once against the bytes in hand, because metadata is a claim about
/// a file that another program may be writing to while this one reads it. The
/// second check is [`choice_from_image_bytes`]'s and is the one that decides;
/// this one only saves the read.
///
/// **So the metadata check has no observable behaviour of its own, and a
/// mutation of it SURVIVES the suite -- measured, not assumed.** Deleting it
/// leaves every test green, because the same file is then read and refused
/// with the same [`IconRefusal::TooLarge`] one function further on. That is
/// what it is: an optimisation that keeps a twenty-megabyte photograph out of
/// this process's memory, not a rule. It is recorded here rather than
/// defended with a test that would have to reach inside the read to see the
/// difference. The rule itself IS tested -- `an_oversized_file_is_refused_
/// before_it_is_decoded` kills the mutation of the check that decides.
pub fn read_icon_file(path: &std::path::Path) -> Result<IconChoice, IconRefusal> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_PICKED_ICON_BYTES as u64 {
            return Err(IconRefusal::TooLarge);
        }
    }
    let bytes = std::fs::read(path).map_err(|_| IconRefusal::Unreadable)?;
    choice_from_image_bytes(&bytes)
}

/// Straight 8-bit RGBA PNG, no interlacing, nothing ancillary.
///
/// `None` only if the encoder itself fails, which for a buffer whose length
/// this function has just computed means an allocation failure -- handled
/// rather than unwrapped, because this runs inside a frame and a panic there
/// takes the window down.
fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` x `height` PNG of one opaque colour -- the smallest thing
    /// `favicon::decode_rgba` calls an image.
    fn solid_png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let pixels: Vec<u8> = (0..width * height)
                .flat_map(|i| {
                    // Deliberately not one flat colour across the whole
                    // image: a solid block compresses to almost nothing, and
                    // a size assertion against it would be measuring zlib
                    // rather than the picture.
                    let v = (i % 251) as u8;
                    [v, v.wrapping_add(70), v.wrapping_add(140), 255]
                })
                .collect();
            writer.write_image_data(&pixels).expect("png data");
        }
        out
    }

    #[test]
    fn field_name_matches_spec() {
        assert_eq!(ICON_FIELD_NAME, "deskwarden:icon");
    }

    #[test]
    fn a_url_choice_round_trips_through_the_field_value() {
        let original = choice_from_url("  https://example.com/logo.png ").expect("a good URL");
        assert_eq!(original, IconChoice::Url { url: "https://example.com/logo.png".into() });
        let parsed = IconChoice::from_field_value(&original.to_field_value()).expect("parses");
        assert_eq!(original, parsed);
    }

    /// The exact bytes, because the discriminator is the whole reason this is
    /// one field rather than two and a rename of it is a field every already
    /// shipped build stops reading.
    #[test]
    fn the_stored_shape_carries_its_discriminator() {
        assert_eq!(
            choice_from_url("https://example.com/a.png").unwrap().to_field_value(),
            r#"{"kind":"url","url":"https://example.com/a.png"}"#
        );
        let png = IconChoice::Png { png: "AAAA".to_string() };
        assert_eq!(png.to_field_value(), r#"{"kind":"png","png":"AAAA"}"#);
    }

    #[test]
    fn a_picked_image_round_trips_to_pixels() {
        let choice = choice_from_image_bytes(&solid_png(16, 16)).expect("a real PNG");
        let stored = choice.to_field_value();
        let read_back = IconChoice::from_field_value(&stored).expect("parses");
        let (w, h, rgba) = read_back.pixels().expect("the stored bytes decode");
        assert_eq!((w, h), (16, 16), "the picture changed size on the round trip");
        assert_eq!(rgba.len(), 16 * 16 * 4);
    }

    /// The resample is what stops a big picture becoming a big field, and it
    /// is asserted on the STORED value rather than on the decoder, because
    /// the field is the thing with the cost.
    #[test]
    fn a_large_image_is_stored_at_the_icon_target_size() {
        let choice = choice_from_image_bytes(&solid_png(512, 512)).expect("a real PNG");
        let (w, h, _) = choice.pixels().expect("the stored bytes decode");
        assert_eq!(
            (w, h),
            (64, 64),
            "a 512x512 pick was stored at its own size, so one item's icon is a quarter of a \
             megabyte of cipher"
        );
        assert!(
            choice.to_field_value().len() < MAX_STORED_ICON_BYTES,
            "the field value for a 512x512 pick is {} bytes",
            choice.to_field_value().len()
        );
    }

    #[test]
    fn an_oversized_file_is_refused_before_it_is_decoded() {
        let bytes = vec![0u8; MAX_PICKED_ICON_BYTES + 1];
        assert_eq!(choice_from_image_bytes(&bytes), Err(IconRefusal::TooLarge));
    }

    /// The cap is a `>` and not a `>=`: a file of exactly the cap is allowed
    /// through to the decoder, which is where it is then refused for not
    /// being an image. Two different refusals for two different files, which
    /// is what tells the user which one they have.
    #[test]
    fn a_file_of_exactly_the_cap_reaches_the_decoder() {
        let bytes = vec![0u8; MAX_PICKED_ICON_BYTES];
        assert_eq!(choice_from_image_bytes(&bytes), Err(IconRefusal::NotAnImage));
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused() {
        assert_eq!(
            choice_from_image_bytes(b"this is a text file, not a picture"),
            Err(IconRefusal::NotAnImage)
        );
    }

    /// A valid PNG with no ink in it is what an icon service answers when it
    /// has no icon but does not want to say 404 -- see `favicon::decode_rgba`.
    /// It is not an icon when it arrives off the wire and it is not one when
    /// it is picked off disk either.
    #[test]
    fn a_fully_transparent_image_is_not_an_icon() {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 32, 32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&vec![0u8; 32 * 32 * 4]).expect("png data");
        }
        assert_eq!(choice_from_image_bytes(&out), Err(IconRefusal::NotAnImage));
    }

    #[test]
    fn only_http_and_https_urls_with_a_host_are_accepted() {
        for good in [
            "https://example.com/logo.png",
            "http://example.com/logo.png",
            "HTTPS://Example.com/Logo.PNG",
            "https://192.168.1.5:8080/favicon.ico",
        ] {
            assert!(choice_from_url(good).is_ok(), "{good} was refused");
        }
        for bad in [
            "",
            "example.com/logo.png",
            "file:///C:/Users/me/logo.png",
            "ftp://example.com/logo.png",
            "data:image/png;base64,AAAA",
            "https://localhost:3000/logo.png",
            "https:///logo.png",
        ] {
            assert_eq!(choice_from_url(bad), Err(IconRefusal::BadUrl), "{bad} was accepted");
        }
    }

    /// The path is stored as typed. A URL whose case or trailing slash this
    /// app "fixed" is a URL the user's own server may 404.
    #[test]
    fn a_url_is_stored_exactly_as_given_but_for_the_trim() {
        let choice = choice_from_url("\t https://Example.COM/A/Logo.PNG?v=2  \n").unwrap();
        assert_eq!(choice.url(), Some("https://Example.COM/A/Logo.PNG?v=2"));
    }

    /// The reader answers `None` for a value it cannot read, and -- the half
    /// that matters -- the field is still THERE afterwards. A reader that
    /// "repaired" an unparseable value would be destroying the one piece of
    /// evidence the user could act on.
    #[test]
    fn an_unreadable_field_value_is_no_icon_and_no_deletion() {
        let mut item = item_with_icon_field("not json at all");
        assert_eq!(chosen_icon(&item), None);
        assert!(has_icon_field(&item), "reading the field removed it");

        // A shape from an imagined later build: parses as JSON, is not one of
        // this build's two variants.
        item = item_with_icon_field(r#"{"kind":"svg","svg":"<svg/>"}"#);
        assert_eq!(chosen_icon(&item), None);
        assert!(
            has_icon_field(&item),
            "a value this build does not understand left the item with nothing to clear"
        );
    }

    #[test]
    fn a_blank_or_absent_field_is_no_icon() {
        let bare: VaultItem =
            serde_json::from_str(r#"{"id":"i1","name":"Item","type":1}"#).expect("a vault item");
        assert_eq!(chosen_icon(&bare), None);
        assert!(!has_icon_field(&bare));
        let blank = item_with_icon_field("   ");
        assert_eq!(chosen_icon(&blank), None);
        assert!(
            has_icon_field(&blank),
            "a blank value is still a field, and still something to clear"
        );
    }

    /// A URL choice has no pixels of its own: it is fetched, and never
    /// mistaken for a payload.
    #[test]
    fn a_url_choice_has_no_stored_pixels() {
        assert!(choice_from_url("https://example.com/a.png").unwrap().pixels().is_none());
    }

    /// Base64 that is not base64, and base64 of something that is not a PNG.
    /// Both answer `None` rather than panicking inside a frame.
    #[test]
    fn stored_bytes_that_do_not_decode_answer_nothing() {
        assert!(IconChoice::Png { png: "!!!not base64!!!".into() }.pixels().is_none());
        let mut junk = String::new();
        crate::record::seal::base64_into(&mut junk, b"not a png");
        assert!(IconChoice::Png { png: junk }.pixels().is_none());
    }

    /// An item carrying exactly one custom field, this one, with `value` in
    /// it.
    ///
    /// Built by deserializing, the way every other item fixture in this crate
    /// is: `VaultItem` has a `#[serde(flatten)]` catch-all and no `Default`,
    /// and a struct literal here would have to name every field -- so it
    /// would go stale the day the struct grows one, and it would model the
    /// wire shape from memory rather than from the parser production uses.
    fn item_with_icon_field(value: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": "i1",
            "name": "Item",
            "type": 1,
            "fields": [{ "name": ICON_FIELD_NAME, "value": value }],
        }))
        .expect("a vault item")
    }
}

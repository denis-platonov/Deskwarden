//! Reading a QR code out of a buffer of pixels.
//!
//! One function, [`decode_qr`], and it is pure: pixels in, a string out, no
//! I/O of any kind. Where the pixels came from -- a region the user dragged
//! across their screen, or an image file they picked -- is somebody else's
//! problem, and that separation is the point. The capture is the only part of
//! this feature that touches the OS and the only part that cannot be
//! unit-tested; everything downstream of it, including this, is lifted out so
//! that it can be.
//!
//! # Why this file names [`crate::screen_capture`]
//!
//! It imports two things from it and calls no OS API through either: the
//! [`ScreenRect`] type and [`crate::screen_capture::rect_around`], the pure
//! arithmetic that builds one out of four corner points. [`codes_in`] reports
//! **where** in the picture a code was, so the whole-screen scan can ring it
//! on the overlay, and a rectangle is what that answer is -- so it is this
//! crate's rectangle rather than a second one declared here. See
//! [`Codes::One`] for what space the numbers are in, which is the part worth
//! reading before using them. Nothing about this module's purity changes: a
//! type and a `min`/`max` are not I/O.
//!
//! # The pixels ARE the secret
//!
//! A QR code of an `otpauth://` URI is the seed in visual form. So the string
//! that comes back out is a [`Zeroizing`], and the honest account of what is
//! and is not wiped is in [`decode_qr`]'s own documentation -- read it before
//! trusting this module, because part of the answer is "not everything".
//!
//! # What comes out is not trusted
//!
//! [`decode_qr`] hands back **whatever the QR said**. It is not a URI, not a
//! validated anything, and it may be a hostile payload -- anyone who can talk
//! a user into scanning a QR code can choose what is in it. The only thing
//! that may be done with it is to hand it to [`crate::otpauth::parse_otpauth`],
//! which refuses everything it does not recognise. Nothing here treats it as a
//! URL to fetch, a path, or a command.

use zeroize::Zeroizing;

use crate::screen_capture::{rect_around, ScreenRect};

/// The largest buffer [`decode_qr`] will look at, in pixels.
///
/// A bound, not a format rule. 64 megapixels is comfortably larger than any
/// multi-monitor desktop this app will be dragged across, and it exists so
/// that an image file claiming absurd dimensions is refused in one comparison
/// rather than after a detection pass over it.
pub const MAX_PIXELS: usize = 64 * 1024 * 1024;

/// Reads the **first** QR code found in an RGBA buffer, or `None`.
///
/// `rgba` is 8 bits per channel, four channels per pixel, rows top to bottom
/// and no padding between them -- which is what both `GetDIBits` and the `png`
/// crate produce. Anything shorter than `width * height * 4` is `None` rather
/// than a panic: this is handed the output of a capture that can partially
/// fail.
///
/// # `None` is a real answer
///
/// A region with no QR code in it returns `None`, and the surface above says
/// 6d's "No code in that region" -- which is the common case, since the user
/// drags a box before they have finished aiming it. It is not an error and
/// carries no diagnosis, because there is nothing useful to diagnose: the box
/// missed, or the code is too small, or the screenshot is too dark.
///
/// # What is wiped, and what is not
///
/// The returned string is a [`Zeroizing`], so the seed wipes when the caller
/// drops it. It is written into a buffer this module owns, rather than taken
/// from `rqrr`'s `decode()`, which hands back a plain `String` -- one un-wiped
/// copy of the seed, freed wherever the caller happened to drop it. That
/// buffer is a `Zeroizing<Vec<u8>>` reserved at `rqrr::MAX_PAYLOAD_SIZE`, the
/// largest payload any QR code can carry, so it **cannot** grow while the
/// payload is written into it; `String::from_utf8` then re-uses that same
/// allocation rather than copying it, so exactly one buffer in this module
/// ever holds the seed and that one is wiped.
///
/// **What this module cannot reach, said plainly.** `rqrr` builds its own
/// intermediates from the closure below: a binarised copy of the image, the
/// detected grids, and the de-interleaved codewords the payload is assembled
/// from. Those are ordinary allocations inside a dependency; they are not
/// `Zeroize`ing, this module has no handle on them, and they are released
/// un-wiped when the decode returns. **So a copy of the seed's bits does reach
/// the allocator during a decode, and no assertion here says otherwise.**
/// Closing it means either a decoder built on `Zeroize`ing buffers -- which
/// does not exist in Rust today -- or a fork, which is a dependency this app
/// would then own. What is bounded is the lifetime: the intermediates die with
/// the call, and the pixel buffer is the caller's to wipe.
///
/// Two things follow and are worth stating because they are the properties a
/// reader actually cares about: the seed is **never written to disk** by
/// anything on this path -- no temp file, no debug artifact, no log line -- and
/// it never leaves the machine, because this decoder has no I/O at all.
pub fn decode_qr(rgba: &[u8], width: usize, height: usize) -> Option<Zeroizing<String>> {
    let mut first = None;
    walk_codes(rgba, width, height, |text, _where| {
        first = Some(text);
        // The FIRST grid, which is this function's whole contract: three
        // routes ask "what does this picture say" and none of them has a use
        // for a second answer. `false` stops the walk, so no further grid is
        // decoded and no further payload is ever built.
        false
    });
    first
}

/// What [`codes_in`] found.
///
/// **No derived `Debug`**, and it must stay hand-written: [`Codes::One`]
/// holds a seed inside a [`Zeroizing<String>`], whose own `Debug` prints it.
/// `debug_leak_guard` refuses a derive here and is right to.
pub enum Codes {
    /// Nothing in the buffer decoded.
    None,
    /// Exactly one distinct payload, however many grids carried it, and the
    /// box it occupied.
    ///
    /// **The rectangle is a [`ScreenRect`] and it is not on the screen.** The
    /// type is this crate's one rectangle and it is reused rather than a
    /// second one being invented, because a crate with two rectangle types is
    /// a crate where one of them eventually gets passed where the other was
    /// meant. What [`codes_in`] fills in is the code's box **in the picture it
    /// was handed**, with `(0, 0)` at that picture's top-left.
    ///
    /// For a capture of one monitor that is the monitor's own pixels, and
    /// [`crate::screen_capture::place_in_capture`] is what moves it onto the
    /// desktop -- which the whole-screen scan does per monitor, before it
    /// folds two monitors' answers together, precisely so that the rectangle a
    /// caller finally reads is in one known space rather than in whichever
    /// monitor happened to answer.
    ///
    /// It is **not a secret**, unlike the string beside it: it is where on a
    /// screen the user's own code was, which the user is looking at.
    One(Zeroizing<String>, ScreenRect),
    /// Two or more **different** payloads. Deliberately carries none of them
    /// -- see [`codes_in`] -- and therefore no place either: the answer is
    /// "Deskwarden will not choose", and a place is only useful for pointing
    /// at the one code that was chosen.
    Several,
}

impl std::fmt::Debug for Codes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Codes::None => write!(f, "None"),
            // The rectangle is printed and the payload is not. It is not a
            // secret -- it is where on a screen a code was, which the user is
            // looking at -- and it is the one thing about a `One` worth
            // having in a failure message.
            Codes::One(text, at) => {
                write!(f, "One({} chars not shown, at {at:?})", text.len())
            }
            Codes::Several => write!(f, "Several"),
        }
    }
}

/// **How many distinct QR codes a buffer holds, and the payload when there is
/// exactly one.**
///
/// [`decode_qr`]'s sibling rather than a replacement for it, and the split is
/// the point. Three routes -- the drag overlay, the image file, the webcam --
/// ask what a picture says, and widening their answer to carry a count would
/// make each of them handle a case it has no use for: a user who dragged a
/// box around one code, or opened a screenshot of one, has already chosen.
/// This is for the one caller that has **not** been given a choice and must
/// refuse to invent one -- the whole-screen scan, which looks at a desktop it
/// did not frame and can perfectly well find two.
///
/// # Distinct by payload, not by grid
///
/// Two detections of the same code are **one** answer: a page open in two
/// windows, a thumbnail beside its own preview, the same setup page reloaded
/// in a second tab. What a user would be asked to choose between is the
/// *secret*, and in those cases there is only one. Only two genuinely
/// different payloads are [`Codes::Several`].
///
/// # `Several` carries nothing, and that is a security decision
///
/// The obvious signature is `Vec<Zeroizing<String>>`, and it would put every
/// seed on the desktop into one value in order to describe a situation whose
/// entire response is "Deskwarden will not choose for you". At most **one**
/// payload is alive inside this function at any moment: a second, different
/// one ends the walk immediately and the one being held is dropped -- and so
/// wiped -- before the answer leaves.
///
/// Everything [`decode_qr`] documents about `rqrr`'s own un-wiped
/// intermediates applies here unchanged. This function widens nothing: it
/// walks the same grids through the same helper.
pub fn codes_in(rgba: &[u8], width: usize, height: usize) -> Codes {
    let mut tally = Tally::new();
    walk_codes(rgba, width, height, |text, at| tally.saw(text, at));
    tally.finish()
}

/// **The running answer to "how many distinct codes so far", and the rule for
/// combining sightings.**
///
/// Extracted rather than written inline in [`codes_in`] because there are two
/// places that need exactly this fold and they are in different modules:
/// [`codes_in`] runs it over the grids of **one** picture, and the
/// whole-screen scan runs it over the answers from **each monitor in turn**.
/// Written twice, the two would disagree the first time either was corrected
/// -- most likely about whether the same code appearing on two monitors is
/// one code or two, which is precisely the question the user's experience
/// turns on. Written once, a test can drive every transition by hand, which
/// is the other half of why it is a type: the two-different-payloads case
/// cannot be built out of the single committed QR fixture, and this is the
/// seam that makes it assertable anyway.
///
/// **At most one payload is ever held.** A second, different one does not
/// join a list; it drops the one being held and latches [`Self::several`], so
/// the widest this can get is a single seed plus a boolean.
///
/// **The payload and its place are one field, not two.** They are answers to
/// the same sighting and they have to move together: two `Option`s that must
/// agree is a state with a fourth combination in it -- a place with no
/// payload, or the place of a code that was dropped as a duplicate -- and the
/// fourth combination is the one that reaches the user as a blue box drawn
/// round the wrong thing.
pub struct Tally {
    seen: Option<(Zeroizing<String>, ScreenRect)>,
    several: bool,
}

impl Default for Tally {
    fn default() -> Self {
        Tally::new()
    }
}

impl Tally {
    /// An empty tally: nothing seen.
    pub fn new() -> Self {
        Tally {
            seen: None,
            several: false,
        }
    }

    /// Records one decoded payload. Answers **whether it is still worth
    /// looking**: `false` once a second distinct code has been seen, because
    /// from that point no further sighting can change the answer and every
    /// one of them would be another seed pulled out of a picture for nothing.
    ///
    /// A repeat of what is already held is dropped -- and so wiped -- as this
    /// returns.
    ///
    /// **The latch is checked first, and that is not belt-and-braces.** Once
    /// a second distinct code has been seen there is no payload left in here
    /// to compare against, so a caller that kept feeding this -- a merge loop
    /// that ignored the `false`, say -- would find `seen` empty, take the
    /// first branch below, and quietly turn "more than one" back into "one".
    ///
    /// `at` is where that payload was, in whatever space the caller is
    /// working in -- see [`Codes::One`]. **The FIRST sighting's place is the
    /// one kept**, matching the payload it is the place of: a code that turns
    /// up a second time on another monitor is the same secret, but it is not
    /// in the same place, and a mark drawn at the later place would point at
    /// a copy of the code rather than at the code the user is looking at. The
    /// first is no better a guess than the second, and it is the one that
    /// belongs to the string being held.
    pub fn saw(&mut self, text: Zeroizing<String>, at: ScreenRect) -> bool {
        if self.several {
            return false;
        }
        let differs = match self.seen.as_ref() {
            Some((first, _)) => first.as_str() != text.as_str(),
            None => false,
        };
        if differs {
            self.several = true;
            // Both die here: the one being held, and the one just handed in.
            self.seen = None;
            return false;
        }
        if self.seen.is_none() {
            self.seen = Some((text, at));
        }
        true
    }

    /// Folds one whole picture's answer in, for the caller scanning several
    /// pictures. Same return as [`Self::saw`].
    pub fn merge(&mut self, codes: Codes) -> bool {
        if self.several {
            return false;
        }
        match codes {
            Codes::None => true,
            Codes::One(text, at) => self.saw(text, at),
            Codes::Several => {
                self.several = true;
                self.seen = None;
                false
            }
        }
    }

    /// The answer, consuming the tally so the payload is moved out rather
    /// than copied.
    pub fn finish(self) -> Codes {
        match self.seen {
            Some((text, at)) => Codes::One(text, at),
            None if self.several => Codes::Several,
            None => Codes::None,
        }
    }
}

/// Walks the QR grids in an RGBA buffer, handing each decoded payload **and
/// the box it sat in** to `visit` until it answers `false`.
///
/// The box comes from `rqrr::Grid::bounds`, which this is the only place in
/// the crate that reads. It is four corner points in the buffer's own pixels,
/// listed top-left, top-right, bottom-right, bottom-left, and it is turned
/// into a rectangle by [`crate::screen_capture::rect_around`] rather than by
/// arithmetic here -- rectangles are built in one module in this crate, for
/// the reason `region_overlay`'s `Drag::rect` gives at greater length.
///
/// **It is read whether or not the caller wants it**, because the alternative
/// is two walks. It costs four `i32` copies per grid and [`decode_qr`] throws
/// it away.
///
/// **The one place in this crate that talks to `rqrr`**, and it is one place
/// on purpose. The buffer discipline documented at length on [`decode_qr`] --
/// a `Zeroizing` reserved at `rqrr::MAX_PAYLOAD_SIZE` so that writing a
/// payload into it cannot re-allocate and hand a half-written seed back to
/// the allocator, rebuilt inside the loop so a failed decode's partial bytes
/// cannot bleed into the next grid's attempt -- is the kind of thing that
/// must exist once. A second copy of it beside [`codes_in`] would be a second
/// thing to keep right, and this crate has already lost a rectangle to
/// exactly that.
///
/// The bounds checks are here rather than in the callers for the same reason:
/// a caller that forgot one would walk past the end of the buffer, and there
/// are now two callers.
fn walk_codes(
    rgba: &[u8],
    width: usize,
    height: usize,
    mut visit: impl FnMut(Zeroizing<String>, ScreenRect) -> bool,
) {
    if width == 0 || height == 0 {
        return;
    }
    let Some(pixels) = width.checked_mul(height) else {
        return;
    };
    if pixels > MAX_PIXELS {
        return;
    }
    let Some(bytes) = pixels.checked_mul(4) else {
        return;
    };
    if rgba.len() < bytes {
        return;
    }

    // `prepare_from_greyscale` pulls each pixel through this closure, so no
    // greyscale copy of the image is built HERE. `rqrr` builds one of its own
    // and that one is not this module's to wipe; see the note above.
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        let at = (y * width + x) * 4;
        luma(rgba[at], rgba[at + 1], rgba[at + 2])
    });

    for grid in prepared.detect_grids() {
        // Reserved at the largest payload a QR code can hold, so writing into
        // it cannot re-allocate and cannot therefore hand a half-written copy
        // of the seed back to the allocator un-wiped. Built INSIDE the loop:
        // `decode_to` documents that a failed decode may still have written a
        // partial payload, so a reused buffer would carry one grid's bytes
        // into the next grid's attempt.
        let mut out = Zeroizing::new(Vec::<u8>::with_capacity(rqrr::MAX_PAYLOAD_SIZE));
        if grid.decode_to(&mut *out).is_err() || out.is_empty() {
            continue;
        }
        // Read after the decode succeeded, so a grid that was detected but
        // could not be read never produces a place -- there is no payload for
        // it to be the place of.
        let at = rect_around(grid.bounds.map(|point| (point.x, point.y)));
        // `take` moves the buffer out so `from_utf8` can re-use its
        // allocation; the `Zeroizing` left behind holds an empty `Vec`, and
        // the seed's one and only buffer is now inside the `Zeroizing<String>`
        // handed to `visit`. A QR payload need not be UTF-8 -- an `otpauth://`
        // URI is, so anything that is not is not what this is looking for.
        let bytes = std::mem::take(&mut *out);
        match String::from_utf8(bytes) {
            Ok(text) => {
                if !visit(Zeroizing::new(text), at) {
                    return;
                }
            }
            Err(bad) => drop(Zeroizing::new(bad.into_bytes())),
        }
    }
}

/// Rec. 601 luma, the weighting every QR decoder uses.
///
/// Integer arithmetic on purpose: this runs once per pixel over a region that
/// may be a whole monitor, and the overlay re-runs a decode as the user drags.
///
/// **Alpha is ignored, deliberately.** A screen capture through `GetDIBits`
/// comes back with an alpha channel that is meaninglessly zero for ordinary
/// desktop windows, and honouring it would turn every such capture uniformly
/// black -- a decoder that reports "no code" on every real screenshot while
/// passing on every synthetic one.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((77 * r as u32 + 150 * g as u32 + 29 * b as u32) >> 8) as u8
}

/// **`pub(crate)` rather than private**, so that
/// `vault_window::totp_add`'s image-format tests can render THIS matrix
/// rather than commit a second one of their own.
///
/// Those tests ask whether a JPEG, a GIF, a BMP, a WebP and an ICO carrying a
/// QR code come out of the file decoder as a code this app can read. That
/// question is only answered if the code inside them is a real one, and a
/// fixture invented over there would be a second thing to keep correct and a
/// second thing to change when this one changes. The independence argument on
/// [`tests::FIXTURE`] -- that it was generated outside this repository by a
/// crate that is not a dependency -- is exactly what makes it worth sharing:
/// it is evidence for both callers or for neither.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A QR code of [`FIXTURE_TEXT`], one character per module: `#` dark, `.`
    /// light, no quiet zone (the test adds one).
    ///
    /// **A committed fixture in Rust source rather than a PNG**, for two
    /// reasons. This crate pins the exact set of non-Rust files it owns
    /// (`job_object.rs`), and the standing rule beside the drawn card marks is
    /// that no opaque binary enters `assets/` -- a generator and its output,
    /// or nothing. And a text matrix is a fixture a reviewer can read: a
    /// change to it shows up in the diff as changed modules, where a changed
    /// PNG shows up as "binary files differ".
    ///
    /// Generated once, outside this repository, with the `qrcode` crate at
    /// error-correction level M over [`FIXTURE_TEXT`] -- a crate that is NOT a
    /// dependency of this app and is not needed to run this test. It is an
    /// independent implementation from the decoder under test, which is what
    /// makes the decode below evidence of anything: a fixture produced by
    /// `rqrr` itself would only prove `rqrr` agrees with `rqrr`.
    pub(crate) const FIXTURE: [&str; 45] = [
        "#######...#...#.#...#.#..##..#####..#.#######",
        "#.....#.##.#.....####..####........#..#.....#",
        "#.###.#.#..######.#..#.#....#....#.#..#.###.#",
        "#.###.#.##.##..#..#..#..####.#####.##.#.###.#",
        "#.###.#...##...#.#.######..#..#...###.#.###.#",
        "#.....#..##.##...#..#...##...#..#.....#.....#",
        "#######.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#.#######",
        "........#..#..#.#...#...#..####.#####........",
        "#.....#.#.###.##.#.######.##..##..#.###..###.",
        "#..#...#####.##...##.####.#####...#######..#.",
        "#..#.##.#...#.#.##.#....###....#.##.###..#.#.",
        "##.##..##.#..###.#...#.##.....##.##.#...#.###",
        "#...###...##.#.##.#.#..##.#...#.#.#.....#....",
        "######.#..#.##.#.#...##..##.#.####..##.##.#.#",
        "##.#..#..#..####..#.#####.#.##.#.#..###......",
        ".##..#.####....###.###..##.#.#.#...#####..#.#",
        ".####.##.#..###.######..##...#.#.#.......#...",
        "###..#..##.#.###.#.###.......##..#..#..#.####",
        ".#...##..###..###....##.#...###.##.#.#.#.#..#",
        "##.##....####.##.###.....###..#...#..##.####.",
        "#.#.#####...#..##.#######.#..#.#.#.######..#.",
        ".#.##...#...#..###..#...##....##.####...###..",
        "#####.#.###...#.#.#.#.#.#..#.....####.#.#.##.",
        "..#.#...#.###.#.#####...#...#...###.#...####.",
        "..########..#.##.############.###...#####..##",
        "..##...#.#...#.#.#######.#...##..#..####.#..#",
        ".#.#..#....###...##.#..#.......#..##.#...#.#.",
        "#...#..#.#.##.###...#..####.#.###..###.####..",
        ".#.##.####.###.......#.#...#...#.##.##..##.##",
        "#.##.#..###.###.#.#.#.##.#..#.####.##..#.#.##",
        "#..######.#...#.##..#####.#.#..#####..#.#.###",
        ".##.#....##.#..##..##.##..##########..#..##.#",
        "#.###.#.#......#...#.........#.#..#....##.##.",
        "#...##.#..#.###...##...#..######.##....##.#..",
        "....#.#..##.#....##.##.#..###..##.#..#.#.###.",
        ".####...######.##.#..#.##...#.##.####.######.",
        "#..##.#.##.#.#####..#####.#.###.#.########..#",
        "........##...#.##.#.#...#...####....#...#.#.#",
        "#######..###...#.####.#.##.#.####.#.#.#.#.##.",
        "#.....#..#######.####...#..#.###.##.#...###..",
        "#.###.#......##...#.#####....#.#.#.######..#.",
        "#.###.#....###..#...#....#.#.#####...#..#.#.#",
        "#.###.#....##..#....#.#.##.#.#.###..#.#.#..##",
        "#.....#..#..##.#....#.###..#####.......####..",
        "#######.##...####.##.####..#.###.##.###....#.",
    ];

    /// What [`FIXTURE`] encodes. A real `otpauth://` URI with every parameter
    /// stated, so the decode below can be checked against something the rest
    /// of this feature actually consumes.
    pub(crate) const FIXTURE_TEXT: &str = "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host&digits=8&period=60&algorithm=SHA256";

    /// Renders [`FIXTURE`] to an RGBA buffer at `scale` pixels per module,
    /// with a four-module quiet zone -- the margin the QR specification
    /// requires and without which no decoder finds the code.
    ///
    /// Returns `(rgba, width, height)`.
    pub(crate) fn fixture_rgba(scale: usize) -> (Vec<u8>, usize, usize) {
        const QUIET: usize = 4;
        let modules = FIXTURE.len();
        let side = (modules + 2 * QUIET) * scale;
        let mut rgba = vec![0xffu8; side * side * 4];
        for (row, line) in FIXTURE.iter().enumerate() {
            for (col, cell) in line.chars().enumerate() {
                if cell != '#' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = (col + QUIET) * scale + dx;
                        let y = (row + QUIET) * scale + dy;
                        let at = (y * side + x) * 4;
                        rgba[at] = 0x00;
                        rgba[at + 1] = 0x00;
                        rgba[at + 2] = 0x00;
                    }
                }
            }
        }
        (rgba, side, side)
    }

    /// The fixture is square and the rows are all the same length, so
    /// [`fixture_rgba`] cannot be quietly rendering a ragged matrix that
    /// happens to decode.
    #[test]
    fn the_fixture_is_a_square_matrix() {
        for (row, line) in FIXTURE.iter().enumerate() {
            assert_eq!(line.chars().count(), FIXTURE.len(), "row {row} is a different length");
            assert!(
                line.chars().all(|c| c == '#' || c == '.'),
                "row {row} has a character that is neither a module nor a gap"
            );
        }
        // A QR code's version determines its side: 21 + 4 * (version - 1).
        assert_eq!((FIXTURE.len() - 21) % 4, 0, "{} is not a legal QR side", FIXTURE.len());
    }

    /// **A real QR code decodes back to the exact string it was made from.**
    ///
    /// Rendered from a committed matrix rather than captured: no test in this
    /// crate may read the screen, and this one does not -- it builds its
    /// pixels arithmetically.
    #[test]
    fn a_rendered_qr_code_decodes_back_to_its_text() {
        let (rgba, w, h) = fixture_rgba(4);
        let decoded = decode_qr(&rgba, w, h).expect("the fixture decodes");
        assert_eq!(&*decoded, FIXTURE_TEXT);
    }

    /// And what it decodes to is a URI the rest of this feature accepts, with
    /// its parameters intact.
    ///
    /// The two halves of this feature are separately tested and this is the
    /// seam between them: a decoder that returned a *nearly* right string
    /// would pass the test above only by returning exactly the right one, but
    /// this is what says the pair is useful rather than merely consistent.
    #[test]
    fn what_a_decode_produces_is_something_the_parser_accepts() {
        let (rgba, w, h) = fixture_rgba(4);
        let decoded = decode_qr(&rgba, w, h).expect("the fixture decodes");
        let parsed = crate::otpauth::parse_otpauth(&decoded).expect("the decoded URI parses");
        assert_eq!(parsed.issuer.as_deref(), Some("Git Host"));
        assert_eq!(parsed.account.as_deref(), Some("anovak"));
        assert_eq!(parsed.secret.as_str(), "JBSWY3DPEHPK3PXP");
        assert_eq!(parsed.digits, 8);
        assert_eq!(parsed.period, 60);
        assert_eq!(parsed.algorithm, crate::otpauth::Algorithm::Sha256);
    }

    /// It reads the code at more than one size, so the decode above is not an
    /// accident of one scale factor.
    #[test]
    fn the_same_code_reads_at_several_scales() {
        for scale in [3usize, 5, 8] {
            let (rgba, w, h) = fixture_rgba(scale);
            assert_eq!(
                decode_qr(&rgba, w, h).as_deref().map(String::as_str),
                Some(FIXTURE_TEXT),
                "scale {scale} did not decode"
            );
        }
    }

    /// **A buffer with no QR code in it is `None`** -- 6d's "No code in that
    /// region".
    ///
    /// Every case here is paired with the CONTROL below it, because `None` is
    /// the answer a broken decoder gives to everything, and a test that only
    /// ever asserts `None` cannot tell the two apart.
    #[test]
    fn a_buffer_with_no_code_in_it_is_none() {
        // Plain white: the overwhelmingly common miss, a box dragged across
        // empty desktop.
        let white = vec![0xffu8; 200 * 200 * 4];
        assert!(decode_qr(&white, 200, 200).is_none());

        // Plain black: the shape a capture of a protected window comes back
        // as, which must read as "no code" rather than as anything at all.
        let black = vec![0x00u8; 200 * 200 * 4];
        assert!(decode_qr(&black, 200, 200).is_none());

        // Structured noise that is not a QR code -- a checkerboard has the
        // high-contrast edges a detector looks for and none of the finder
        // patterns it needs.
        let mut checks = vec![0xffu8; 200 * 200 * 4];
        for y in 0..200 {
            for x in 0..200 {
                if (x / 4 + y / 4) % 2 == 0 {
                    let at = (y * 200 + x) * 4;
                    checks[at] = 0;
                    checks[at + 1] = 0;
                    checks[at + 2] = 0;
                }
            }
        }
        assert!(decode_qr(&checks, 200, 200).is_none());

        // **The control**: the very same function, on the very same code path,
        // does find a real one. Without this every assertion above is
        // satisfied by `fn decode_qr(..) -> Option<_> { None }`.
        let (rgba, w, h) = fixture_rgba(4);
        assert_eq!(decode_qr(&rgba, w, h).as_deref().map(String::as_str), Some(FIXTURE_TEXT));
    }

    /// A QR code that is not the whole buffer is still found -- which is the
    /// real case, since the user drags a box around a code sitting on a page.
    #[test]
    fn a_code_surrounded_by_other_content_is_still_found() {
        let (code, code_side, _) = fixture_rgba(4);
        let side = code_side + 120;
        let mut page = vec![0xffu8; side * side * 4];
        // Some furniture: horizontal rules above the code, of the kind a web
        // page puts around a setup panel.
        for y in (10..40).step_by(6) {
            for x in 10..side - 10 {
                let at = (y * side + x) * 4;
                page[at] = 0x40;
                page[at + 1] = 0x40;
                page[at + 2] = 0x40;
            }
        }
        let (ox, oy) = (70usize, 60usize);
        for y in 0..code_side {
            for x in 0..code_side {
                let from = (y * code_side + x) * 4;
                let to = ((y + oy) * side + x + ox) * 4;
                page[to..to + 4].copy_from_slice(&code[from..from + 4]);
            }
        }
        assert_eq!(decode_qr(&page, side, side).as_deref().map(String::as_str), Some(FIXTURE_TEXT));
    }

    /// A short, empty or absurd buffer is `None` rather than a panic.
    ///
    /// This is handed the output of a capture that can partially fail, and a
    /// panic on a truncated buffer would take the app down while the user was
    /// dragging a box.
    #[test]
    fn a_malformed_buffer_is_refused_rather_than_panicking() {
        assert!(decode_qr(&[], 0, 0).is_none());
        assert!(decode_qr(&[], 10, 10).is_none());
        // One byte short of `width * height * 4`.
        let short = vec![0xffu8; 10 * 10 * 4 - 1];
        assert!(decode_qr(&short, 10, 10).is_none());
        // Dimensions that overflow the pixel count, and dimensions that are
        // merely absurd. Neither may be walked.
        assert!(decode_qr(&[0xff; 16], usize::MAX, usize::MAX).is_none());
        assert!(decode_qr(&[0xff; 16], MAX_PIXELS + 1, 1).is_none());
        // Zero in one dimension only.
        assert!(decode_qr(&[0xff; 16], 4, 0).is_none());
        assert!(decode_qr(&[0xff; 16], 0, 4).is_none());

        // Control: exactly `width * height * 4` bytes is accepted and walked
        // -- so the length check above is off by nothing.
        let exact = vec![0xffu8; 10 * 10 * 4];
        assert!(decode_qr(&exact, 10, 10).is_none(), "white, so no code -- but not refused");
        let (rgba, w, h) = fixture_rgba(4);
        assert_eq!(rgba.len(), w * h * 4);
        assert!(decode_qr(&rgba, w, h).is_some());
    }

    /// An inverted code is **not** read.
    ///
    /// Not a limitation being papered over -- a decision worth pinning. Light
    /// modules on a dark ground is a different image, and every other reader
    /// the user has tried it with refuses it too; a decoder that silently
    /// inverted would be the only thing on the machine that read a code the
    /// site's own app will not.
    #[test]
    fn an_inverted_code_is_not_read_and_that_is_deliberate() {
        let (mut rgba, w, h) = fixture_rgba(4);
        // Control first: as rendered, it reads.
        assert!(decode_qr(&rgba, w, h).is_some());
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[0] = 255 - pixel[0];
            pixel[1] = 255 - pixel[1];
            pixel[2] = 255 - pixel[2];
        }
        assert!(decode_qr(&rgba, w, h).is_none());
    }

    /// Alpha is ignored, and it has to be.
    ///
    /// `GetDIBits` over an ordinary desktop window comes back with an alpha
    /// channel of zero. A decoder that honoured it would see every real screen
    /// capture as uniformly transparent-black and answer "no code in that
    /// region" every single time, while passing every synthetic test in this
    /// file.
    #[test]
    fn a_fully_transparent_capture_still_decodes() {
        let (mut rgba, w, h) = fixture_rgba(4);
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 0;
        }
        assert_eq!(decode_qr(&rgba, w, h).as_deref().map(String::as_str), Some(FIXTURE_TEXT));
        // Paired: opaque works too, so the assertion above is about alpha
        // being ignored and not about alpha being required to be zero.
        let (opaque, w, h) = fixture_rgba(4);
        assert!(decode_qr(&opaque, w, h).is_some());
    }

    /// The luma weights are Rec. 601 and they see all three channels.
    ///
    /// A weighting that dropped a channel would still decode a black-on-white
    /// fixture perfectly, and would fail on the coloured QR codes some sites
    /// serve.
    #[test]
    fn luma_is_rec601_and_reads_every_channel() {
        assert_eq!(luma(0, 0, 0), 0);
        // 77 + 150 + 29 = 256, so the shift by 8 lands exactly on 255 at
        // white -- the weights sum to one, which is what stops a bright grey
        // region reading darker than it is.
        assert_eq!(luma(255, 255, 255), 255);
        // Green weighs most, blue least -- and each on its own is non-zero, so
        // no channel is being ignored.
        assert!(luma(0, 255, 0) > luma(255, 0, 0));
        assert!(luma(255, 0, 0) > luma(0, 0, 255));
        assert!(luma(0, 0, 255) > 0);
    }

    /// **The decoded seed comes back in a `Zeroizing`**, so it wipes when the
    /// caller drops it.
    ///
    /// The type says this and the compiler enforces it; what this test adds is
    /// the observation that the value really is the seed -- a `Zeroizing`
    /// around an empty string would satisfy the signature and nothing else.
    #[test]
    fn the_decoded_text_is_the_seed_and_it_is_zeroizing() {
        let (rgba, w, h) = fixture_rgba(4);
        let decoded: Zeroizing<String> = decode_qr(&rgba, w, h).expect("decodes");
        assert!(decoded.contains("JBSWY3DPEHPK3PXP"), "the decode did not carry the seed");
        assert_eq!(decoded.len(), FIXTURE_TEXT.len());
    }

    // -- how many, for the caller that must not guess -----------------------

    /// Renders `copies` of [`FIXTURE`] in a row on one white page, separated
    /// by a wide gutter so each keeps the quiet zone a detector needs.
    ///
    /// Returns `(rgba, width, height)`.
    fn fixture_row(copies: usize, scale: usize) -> (Vec<u8>, usize, usize) {
        const GUTTER: usize = 48;
        let (code, side, _) = fixture_rgba(scale);
        let width = copies * side + (copies + 1) * GUTTER;
        let height = side + 2 * GUTTER;
        let mut page = vec![0xffu8; width * height * 4];
        for copy in 0..copies {
            let ox = GUTTER + copy * (side + GUTTER);
            for y in 0..side {
                for x in 0..side {
                    let from = (y * side + x) * 4;
                    let to = ((y + GUTTER) * width + x + ox) * 4;
                    page[to..to + 4].copy_from_slice(&code[from..from + 4]);
                }
            }
        }
        (page, width, height)
    }

    /// **One code on a page is one code, and its payload comes back.**
    #[test]
    fn a_page_with_one_code_reports_exactly_one() {
        let (page, w, h) = fixture_row(1, 4);
        match codes_in(&page, w, h) {
            Codes::One(text, _) => assert_eq!(&*text, FIXTURE_TEXT),
            other => panic!("one code on the page came back as {other:?}"),
        }
    }

    /// **The box comes back with the payload, and it is where the code
    /// actually is.**
    ///
    /// The claim design 6b's reveal is drawn from: after a whole-screen scan
    /// the overlay rings the code, and a ring in the wrong place is worse than
    /// no ring at all. `rqrr` extrapolates a grid's corners from its three
    /// finder patterns rather than measuring them, so this is deliberately
    /// **not** an exact-pixel assertion -- it is the two properties that
    /// matter: the box is inside the picture, and it covers the modules rather
    /// than the quiet zone or the page.
    ///
    /// [`fixture_rgba`] renders [`FIXTURE`]'s 45 modules at `scale` pixels
    /// each with a four-module quiet zone, so the code itself occupies exactly
    /// `[4 * scale, (4 + 45) * scale)` on both axes and every number below is
    /// derived from that rather than typed in.
    #[test]
    fn the_box_that_comes_back_is_where_the_code_is() {
        const QUIET: usize = 4;
        for scale in [3usize, 4, 6] {
            let (rgba, w, h) = fixture_rgba(scale);
            let at = match codes_in(&rgba, w, h) {
                Codes::One(_, at) => at,
                other => panic!("the fixture came back as {other:?} at scale {scale}"),
            };
            let modules_from = (QUIET * scale) as i32;
            let modules_to = ((QUIET + FIXTURE.len()) * scale) as i32;

            // Inside the picture it was found in, on every side.
            assert!(
                at.left >= 0 && at.top >= 0 && at.right <= w as i32 && at.bottom <= h as i32,
                "scale {scale}: {at:?} is not inside the {w}x{h} picture it was found in"
            );
            // And covering the code rather than the page: the box starts in
            // the quiet zone or on the code, and ends on the code or in the
            // quiet zone past it.
            //
            // **Two modules of slack on each side**, and the tolerance is in
            // modules rather than pixels so that it does not silently tighten
            // as `scale` grows. It is not padding for a wrong answer: `rqrr`
            // fits the grid to the three finder patterns and extrapolates the
            // fourth corner, so the far edges come back a module or so past
            // the last dark module while the top-left, which sits on a finder
            // pattern, is exact. Measured at these scales the overshoot is
            // about a module and a third. Anything within two modules is the
            // right code; anything outside it is a different rectangle
            // altogether -- the whole picture, the quiet zone, or nothing.
            let slack = 2 * scale as i32;
            assert!(
                (at.left - modules_from).abs() <= slack && (at.top - modules_from).abs() <= slack,
                "scale {scale}: {at:?} does not start at the code's top-left ({modules_from})"
            );
            assert!(
                (at.right - modules_to).abs() <= slack && (at.bottom - modules_to).abs() <= slack,
                "scale {scale}: {at:?} does not end at the code's bottom-right ({modules_to})"
            );
            // Positive control on the whole assertion: the box is a real,
            // positive rectangle roughly the size of the code and not a
            // degenerate one that would satisfy nothing above by accident.
            assert!(at.width() > 0 && at.height() > 0, "scale {scale}: {at:?} is empty");
        }
    }

    /// **The box moves with the code.** The test above pins it against the
    /// one layout `fixture_rgba` produces, where the code is always in the
    /// same place; this renders the same code twice at two different offsets
    /// on one page and requires the two answers to differ by the offset --
    /// which a decoder that reported a constant, or the whole picture, would
    /// fail.
    #[test]
    fn a_code_further_down_the_page_reports_a_box_further_down_the_page() {
        // `fixture_row` lays copies out left to right with a gap, so the
        // second copy's box must be to the right of the first's by at least
        // the code's own width and must not overlap it.
        let (page, w, h) = fixture_row(2, 4);
        let mut boxes = Vec::new();
        walk_codes(&page, w, h, |_, at| {
            boxes.push(at);
            true
        });
        assert!(boxes.len() >= 2, "only {} copies decoded", boxes.len());
        boxes.sort_by_key(|b| b.left);
        let (first, second) = (boxes[0], boxes[boxes.len() - 1]);
        assert!(
            second.left >= first.right,
            "two copies side by side reported overlapping boxes: {first:?} and {second:?}"
        );
        // Same code, same size, different place -- which is the point.
        assert!(
            (first.width() as i64 - second.width() as i64).abs() <= 4,
            "the same code reported two very different sizes: {first:?} and {second:?}"
        );
        assert!(second.top < h as i32 && first.bottom <= h as i32);
    }

    /// **The same code twice is ONE answer, not two.**
    ///
    /// The rule [`codes_in`] is built on: what a user would be asked to choose
    /// between is the *secret*, and a page open in two windows -- or a
    /// thumbnail beside its own preview -- holds one of those. Refusing to
    /// scan such a desktop would be a refusal with nothing behind it.
    ///
    /// The control is the interesting half. Asserting `One` proves nothing on
    /// its own: a detector that found only the left-hand copy would satisfy it
    /// while the de-duplication below stayed unexercised. So the walk is run
    /// directly first and both copies are required to have been *decoded*
    /// before the answer is asked for.
    #[test]
    fn the_same_code_twice_in_one_picture_is_one_answer() {
        let (page, w, h) = fixture_row(2, 4);
        let mut decoded = 0usize;
        walk_codes(&page, w, h, |text, _at| {
            assert_eq!(&*text, FIXTURE_TEXT);
            decoded += 1;
            true
        });
        assert!(
            decoded >= 2,
            "only {decoded} of the two copies decoded, so the de-duplication below is untested"
        );
        match codes_in(&page, w, h) {
            Codes::One(text, _) => assert_eq!(&*text, FIXTURE_TEXT),
            other => panic!("two copies of one code came back as {other:?}"),
        }
    }

    /// A page with nothing on it reports nothing -- with the usual control, so
    /// the `None` is about the page and not about a broken walk.
    #[test]
    fn a_page_with_no_code_reports_none() {
        let white = vec![0xffu8; 300 * 300 * 4];
        assert!(matches!(codes_in(&white, 300, 300), Codes::None));
        let (page, w, h) = fixture_row(1, 4);
        assert!(matches!(codes_in(&page, w, h), Codes::One(..)));
    }

    /// The same bounds [`decode_qr`] refuses, refused the same way: a
    /// truncated buffer, an overflowing pixel count and a picture larger than
    /// [`MAX_PIXELS`] are all `None` rather than a walk past the end of a
    /// buffer.
    ///
    /// **[`MAX_PIXELS`] is the bound the whole-screen scan runs into**, so it
    /// is checked here against the real constant rather than against a number
    /// copied beside it.
    #[test]
    fn codes_in_refuses_exactly_what_decode_qr_refuses() {
        for (rgba, w, h) in [
            (vec![], 0usize, 0usize),
            (vec![], 10, 10),
            (vec![0xffu8; 10 * 10 * 4 - 1], 10, 10),
            (vec![0xff; 16], usize::MAX, usize::MAX),
            (vec![0xff; 16], MAX_PIXELS + 1, 1),
            (vec![0xff; 16], 4, 0),
            (vec![0xff; 16], 0, 4),
        ] {
            assert!(
                matches!(codes_in(&rgba, w, h), Codes::None),
                "{w}x{h} was walked rather than refused"
            );
            assert!(decode_qr(&rgba, w, h).is_none(), "{w}x{h} disagreed with codes_in");
        }
        // A picture one pixel under the bound is walked rather than refused --
        // so the comparison above is off by nothing. `MAX_PIXELS` pixels of
        // RGBA is more memory than a test should ask for, so the buffer is
        // deliberately short: `walk_codes` gets past the pixel-count gate and
        // is stopped by the length gate, which is the boundary being pinned.
        assert!(matches!(codes_in(&[], MAX_PIXELS, 1), Codes::None));
    }

    /// **Neither the payload nor its length reaches a formatter as a
    /// secret.** `Codes`' `Debug` is hand-written for the reason
    /// `Outcome`'s is.
    #[test]
    fn the_debug_of_a_found_code_does_not_print_it() {
        let at = ScreenRect { left: 10, top: 20, right: 210, bottom: 220 };
        let shown = format!("{:?}", Codes::One(Zeroizing::new(FIXTURE_TEXT.to_string()), at));
        assert!(!shown.contains("JBSWY3DPEHPK3PXP"), "{shown}");
        assert!(!shown.contains("otpauth"), "{shown}");
        assert!(shown.contains("not shown"), "{shown}");
        // The place IS printed. It is not a secret -- it is where on their own
        // screen the user's code was -- and it is the one thing about a `One`
        // that is worth having in a failure message.
        assert!(shown.contains("left: 10"), "{shown}");
        assert_eq!(format!("{:?}", Codes::None), "None");
        assert_eq!(format!("{:?}", Codes::Several), "Several");
    }

    /// **The two entry points agree**, which is what makes it safe for the
    /// scan to use one and every other route to use the other. A refactor
    /// that gave `codes_in` its own copy of the grid walk and let the two
    /// drift is what this would catch.
    #[test]
    fn decode_qr_and_codes_in_read_the_same_code() {
        for scale in [3usize, 5] {
            let (rgba, w, h) = fixture_rgba(scale);
            let one = decode_qr(&rgba, w, h).expect("decode_qr reads it");
            match codes_in(&rgba, w, h) {
                Codes::One(other, _) => assert_eq!(&*one, &*other),
                other => panic!("codes_in disagreed at scale {scale}: {other:?}"),
            }
        }
    }

    /// **Two DIFFERENT payloads are `Several`, and `Several` carries
    /// neither.**
    ///
    /// This is the one claim in this file that cannot be made from
    /// [`FIXTURE`]: a second payload needs a second real QR matrix, and the
    /// standing rule beside the fixture is that no second one is committed --
    /// its independence from `rqrr` is what makes it evidence, and an
    /// invented sibling would be a second thing to keep correct. So the
    /// accumulation is driven directly, through the same walk `codes_in`
    /// runs, with the decoder replaced by a hand-fed sequence of payloads.
    ///
    /// The whole-screen scan's own "more than one code on this desktop" case
    /// is asserted end to end in `region_overlay`, through its seams -- and
    /// it is the same [`Tally`] being driven there.
    #[test]
    fn two_different_payloads_are_several_and_several_carries_nothing() {
        fn fold(payloads: &[&str]) -> Codes {
            let mut tally = Tally::new();
            for (nth, payload) in payloads.iter().enumerate() {
                // A different place per sighting, so the place assertions
                // below are about which one was kept and not about them all
                // being the same anyway.
                if !tally.saw(Zeroizing::new((*payload).to_string()), somewhere(nth as i32)) {
                    break;
                }
            }
            tally.finish()
        }

        let other = "otpauth://totp/Other:b?secret=MFRGGZDFMZTWQ2LK";
        assert!(matches!(fold(&[FIXTURE_TEXT, other]), Codes::Several));
        assert!(matches!(fold(&[other, FIXTURE_TEXT]), Codes::Several));
        // The de-duplication, in the type the picture walk uses: repeats are
        // one, however many of them arrive.
        assert!(matches!(fold(&[FIXTURE_TEXT, FIXTURE_TEXT, FIXTURE_TEXT]), Codes::One(..)));
        assert!(matches!(fold(&[FIXTURE_TEXT]), Codes::One(..)));
        // **The FIRST sighting's place is the one kept**, matching the payload
        // it belongs to. Three sightings at three places answer with the
        // first.
        match fold(&[FIXTURE_TEXT, FIXTURE_TEXT, FIXTURE_TEXT]) {
            Codes::One(_, at) => assert_eq!(at, somewhere(0)),
            other => panic!("three sightings of one code came back as {other:?}"),
        }
        assert!(matches!(fold(&[]), Codes::None));
        // And `Several` carries nothing: the `Debug` has no room for a
        // payload, and the value it was holding was dropped at the moment the
        // second one arrived.
        assert_eq!(format!("{:?}", fold(&[FIXTURE_TEXT, other])), "Several");
    }

    /// **The latch is one-way.** A caller that ignores [`Tally::saw`]'s
    /// `false` and keeps feeding it must not be able to talk it back down to
    /// one code -- which is exactly what would happen if the emptied `seen`
    /// were allowed to be filled again.
    #[test]
    fn a_tally_that_has_seen_two_codes_cannot_be_talked_back_down_to_one() {
        let other = "otpauth://totp/Other:b?secret=MFRGGZDFMZTWQ2LK";
        let mut tally = Tally::new();
        assert!(tally.saw(Zeroizing::new(FIXTURE_TEXT.to_string()), somewhere(0)));
        assert!(
            !tally.saw(Zeroizing::new(other.to_string()), somewhere(1)),
            "the second code did not stop it"
        );
        // Six more sightings of the same payload, all ignored.
        for nth in 0..6 {
            assert!(!tally.saw(Zeroizing::new(FIXTURE_TEXT.to_string()), somewhere(nth)));
        }
        assert!(matches!(tally.finish(), Codes::Several));
    }

    /// A distinct rectangle per `nth`, so a test can tell which sighting's
    /// place a tally kept. Nothing about the numbers matters beyond their
    /// being different from each other.
    fn somewhere(nth: i32) -> ScreenRect {
        ScreenRect {
            left: nth * 100,
            top: nth * 50,
            right: nth * 100 + 80,
            bottom: nth * 50 + 80,
        }
    }

    /// **A whole picture's answer folds in the same way one payload does**,
    /// which is what lets the screen scan run this over monitors rather than
    /// over grids.
    #[test]
    fn merging_whole_pictures_follows_the_same_rule_as_merging_payloads() {
        let other = "otpauth://totp/Other:b?secret=MFRGGZDFMZTWQ2LK";
        let one = |nth| Codes::One(Zeroizing::new(FIXTURE_TEXT.to_string()), somewhere(nth));

        // Empty pictures change nothing.
        let mut tally = Tally::new();
        assert!(tally.merge(Codes::None));
        assert!(tally.merge(one(0)));
        assert!(tally.merge(Codes::None));
        assert!(matches!(tally.finish(), Codes::One(..)));

        // The same code on two monitors is one code -- and it is the FIRST
        // monitor's place that survives, because that is the monitor the
        // payload being held came off. A mark drawn at the second would point
        // at a copy of the code rather than at the code.
        let mut tally = Tally::new();
        assert!(tally.merge(one(0)));
        assert!(tally.merge(one(1)));
        match tally.finish() {
            Codes::One(_, at) => assert_eq!(at, somewhere(0)),
            other => panic!("one code on two monitors came back as {other:?}"),
        }

        // Different codes on two monitors are several.
        let mut tally = Tally::new();
        assert!(tally.merge(one(0)));
        assert!(!tally.merge(Codes::One(Zeroizing::new(other.to_string()), somewhere(1))));
        assert!(matches!(tally.finish(), Codes::Several));

        // And one monitor that held two on its own settles it by itself.
        let mut tally = Tally::new();
        assert!(!tally.merge(Codes::Several));
        assert!(matches!(tally.finish(), Codes::Several));
        // Including when a code had already been found somewhere else.
        let mut tally = Tally::new();
        assert!(tally.merge(one(0)));
        assert!(!tally.merge(Codes::Several));
        assert!(matches!(tally.finish(), Codes::Several));
    }
}

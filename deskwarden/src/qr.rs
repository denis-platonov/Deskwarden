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
    if width == 0 || height == 0 {
        return None;
    }
    let pixels = width.checked_mul(height)?;
    if pixels > MAX_PIXELS {
        return None;
    }
    if rgba.len() < pixels.checked_mul(4)? {
        return None;
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
        // `take` moves the buffer out so `from_utf8` can re-use its
        // allocation; the `Zeroizing` left behind holds an empty `Vec`, and
        // the seed's one and only buffer is now inside the `Zeroizing<String>`
        // returned below. A QR payload need not be UTF-8 -- an `otpauth://`
        // URI is, so anything that is not is not what this is looking for.
        let bytes = std::mem::take(&mut *out);
        match String::from_utf8(bytes) {
            Ok(text) => return Some(Zeroizing::new(text)),
            Err(bad) => drop(Zeroizing::new(bad.into_bytes())),
        }
    }
    None
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

#[cfg(test)]
mod tests {
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
    const FIXTURE: [&str; 45] = [
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
    const FIXTURE_TEXT: &str = "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host&digits=8&period=60&algorithm=SHA256";

    /// Renders [`FIXTURE`] to an RGBA buffer at `scale` pixels per module,
    /// with a four-module quiet zone -- the margin the QR specification
    /// requires and without which no decoder finds the code.
    ///
    /// Returns `(rgba, width, height)`.
    fn fixture_rgba(scale: usize) -> (Vec<u8>, usize, usize) {
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
}

//! Brand logo files: where they are looked for, what is refused, and how one
//! is normalised so that whatever a brand centre published lands in the slot
//! the wordmark already occupies.
//!
//! **This module never draws.** It answers, for one [`CardBrand`], "is there a
//! usable image on disk, and what shape is it?" -- [`crate::card_mark`] owns
//! every pixel that reaches the screen and owns the fallback to the wordmark.
//! The split is what makes the whole arrangement testable with no window: the
//! rules below are decided on plain buffers.
//!
//! # Why files on disk rather than `include_bytes!`
//!
//! Nothing is compiled in. Marks are read at runtime from
//! [`search_dirs`], and that is three decisions at once:
//!
//! * **The user's directory outranks the shipped one**, so a user can replace
//!   a mark this app ships, or supply one for a brand it ships nothing for,
//!   without a build.
//! * **The images and the code ship separately.** This code lands and is
//!   proven against the fallback path with no image present anywhere, which
//!   is exactly what makes it safe to land before the images exist.
//! * **A mark this project does not distribute works identically to one it
//!   does.** There is no privileged set: the loader cannot tell where a file
//!   came from, only which directory it was found in first.
//!
//! # Why every refusal is silent
//!
//! Every failure here -- no directory, no file, a file too big to open, a
//! file that is not a PNG, one whose pixels are 20000 on a side -- resolves
//! to "there is no logo for this brand", and the caller draws the word. That
//! is the property the whole feature rests on: a user with the setting on and
//! six of nine files present sees six logos and three words, and nothing
//! looks broken. There is no error surface because there is no error the user
//! must act on.

use crate::card_brand::CardBrand;
use std::path::{Path, PathBuf};

/// The directory name, under both roots, that marks are read from.
pub const MARK_DIR_NAME: &str = "brand-marks";

/// The largest mark file this app will read off disk, in bytes.
///
/// **Checked on the directory entry's own metadata, before a byte is read**,
/// which is the point: the file's declared size is free, and reading first to
/// find out how big it was is the bug this constant exists to prevent.
///
/// One megabyte is far more than any of these marks needs -- a network's
/// logotype at the 30-odd physical pixels this app draws it at is a few
/// kilobytes -- and it is deliberately generous anyway, because the cost of
/// being wrong in the tight direction is a legitimate asset silently refused
/// and the cost of being wrong in the loose direction is one megabyte read
/// once per brand per session.
pub const MAX_MARK_BYTES: u64 = 1024 * 1024;

/// The largest edge, in pixels, a mark's PNG header may declare.
///
/// **Read out of the header and checked before the frame is decoded.** These
/// are files on the user's own disk, and a user is not an attacker -- but a
/// 20000x20000 PNG is a 1.6 GB decode whoever put it there, and this app's
/// failure on meeting one must be a refused mark and a drawn word, never a
/// hang and never an allocation failure. 2048 is over sixty times the ~32
/// physical pixels the mark is drawn at and still bounds the decode at 16 MB.
pub const MAX_MARK_EDGE_PX: u32 = 2048;

/// The longest edge a normalised mark is reduced to before it becomes a
/// texture.
///
/// The mark is drawn at most [`crate::card_mark::MARK_DETAIL_HEIGHT`] (18)
/// logical points tall, which is 36 physical pixels at the 200% display
/// scaling Windows tops out at; a full-bleed logotype three times as wide as
/// it is tall therefore covers about 110 physical pixels. 128 clears that
/// without asking the renderer to magnify, and it is a fixed number rather
/// than one derived from the live DPI for the same reason
/// `favicon::ICON_TARGET_PX` is: one texture is shared by every monitor the
/// window can be dragged to.
const MARK_TEXTURE_PX: usize = 128;

/// Alpha at or below which a pixel counts as "not there" when a mark's ink is
/// being found. Not zero, because a PNG exported from a vector tool carries a
/// dusting of 1-2/255 alpha where the antialiasing tapers off, and a border
/// judged by `== 0` would call a perfectly ordinary isolated mark opaque.
const ALPHA_INK: u8 = 8;

/// How far a pixel must differ from the ground colour, per channel, to count
/// as ink on a mark that carries its own ground.
///
/// Generous, because the thing being separated is a logotype from the flat
/// colour behind it -- a difference of hundreds, not of tens -- and the thing
/// being ignored is the ground's own compression noise and the outer half of
/// the lettering's antialiasing.
const GROUND_TOLERANCE: i32 = 24;

/// What a mark file turned out to be.
///
/// **`Missing` and `Refused` are separate variants and that is load-bearing
/// for the cache**, not for the caller, who draws a word either way. See
/// `card_mark::Slot`: a refusal is a property of a file that was there and
/// will not change until the file does, while a miss is a property of a
/// directory the user may be about to drop a file into. Collapsing the two is
/// how the favicon cache came to mark a domainless item "dealt with for the
/// session" and never look again.
#[derive(Debug)]
pub enum MarkLoad {
    /// No file for this brand in any search directory.
    Missing,
    /// A file was found and will not be used. Carries why, for the log line
    /// and for the tests -- never for the screen.
    Refused(Refusal),
    /// A usable mark, normalised and ready to become a texture.
    Loaded(Mark),
}

/// Why a file that existed was not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Over [`MAX_MARK_BYTES`], as its own metadata declares.
    TooManyBytes,
    /// The file could not be read, or its PNG header could not be parsed.
    Unreadable,
    /// The header declares an edge over [`MAX_MARK_EDGE_PX`].
    TooManyPixels,
    /// The header parsed but the frame did not decode.
    Undecodable,
    /// Decoded to nothing -- a zero-width or zero-height frame.
    Empty,
}

/// Which of the two shapes a supplied file turned out to be.
///
/// **The distinction is made from the pixels, not from a naming convention**,
/// because both kinds will turn up in one directory and quite possibly for
/// one brand: brand centres publish an isolated mark on transparency *and* a
/// reversed one on the brand's own colour, and a user downloads whichever
/// they found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Transparent border: the mark alone, and the image's edges mean
    /// nothing. Trimmed to its ink, because the transparent margin is
    /// packaging.
    Isolated,
    /// Opaque to its edges: the rectangle **is** the mark. Never trimmed --
    /// the coloured ground is part of the design, and the clear space inside
    /// it is mandated by the brand's own guidelines. Cropping it produces a
    /// mark the brand does not have.
    OnGround,
}

/// A mark that will be drawn: its pixels, and where its INK sits inside them.
#[derive(Debug, Clone)]
pub struct Mark {
    pub width: usize,
    pub height: usize,
    /// Straight (non-premultiplied) RGBA, `width * height * 4` bytes, ready
    /// for `egui::ColorImage::from_rgba_unmultiplied`.
    pub rgba: Vec<u8>,
    pub kind: MarkKind,
    /// The ink's top and bottom, in pixels down from the top of [`Self::rgba`].
    ///
    /// **This is the number the whole fit rests on.** An isolated mark has
    /// been trimmed, so its ink is its whole height; a mark on its own ground
    /// has not, so its ink is the lettering inside the clear space -- often
    /// barely half the file's height. Scaling both to one *image* height is
    /// what makes a full-bleed asset's lettering render visibly smaller than
    /// the wordmarks beside it. Scaling both so that THIS band matches is what
    /// makes a row of mixed sources read as one set.
    pub ink_top: f32,
    pub ink_bottom: f32,
}

impl Mark {
    /// The ink's height in pixels; never zero, so it is safe to divide by.
    pub fn ink_height(&self) -> f32 {
        (self.ink_bottom - self.ink_top).max(1.0)
    }

    /// Where the ink is centred, in pixels down from the top of the image.
    pub fn ink_center(&self) -> f32 {
        (self.ink_top + self.ink_bottom) / 2.0
    }
}

/// The directories a mark is looked for in, **in the order they are
/// searched**.
///
/// 1. **The user's own** -- `%APPDATA%\Deskwarden\brand-marks\`, beside
///    `settings.json` and the icon cache, resolved through
///    [`crate::settings::config_dir`] so the triple is not spelled a second
///    time. First, so that a file the user put there replaces one this app
///    shipped rather than being shadowed by it. That ordering is the whole
///    reason there are two directories: the shipped set is a default, and a
///    default a user cannot override is not a default.
/// 2. **Beside the executable** -- the installer's, populated at install
///    time. Empty, and quite possibly absent, in a development build and in
///    every build until the images are supplied; a missing directory is
///    indistinguishable here from an empty one, by design.
///
/// Either may be absent from the returned list, when the platform has no
/// resolvable config directory or the running executable's path cannot be
/// read. An empty list is a valid answer and means every brand falls back to
/// its word.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config) = crate::settings::config_dir() {
        dirs.push(config.join(MARK_DIR_NAME));
    }
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(MARK_DIR_NAME)))
    {
        dirs.push(beside);
    }
    dirs
}

/// The file name a mark for `brand` is read from, or `None` for a brand that
/// has no logo to look for.
///
/// **Derived from the enum, in the module that owns the enum**
/// ([`CardBrand::mark_stem`]), for the reason `CARD_BRANDS` exists: a brand
/// added later cannot be half-wired, because it does not compile without an
/// answer here.
pub fn file_name(brand: CardBrand) -> Option<String> {
    brand.mark_stem().map(|stem| format!("{stem}.png"))
}

/// The first existing file for `brand` across `dirs`, in the order given.
///
/// Existence is the only test at this stage: a file that is present but
/// unusable is [`Refusal`]'s business, and a search that fell through to the
/// second directory because the first held a *broken* file would be a search
/// whose result depends on which failure came first.
pub fn find_file(dirs: &[PathBuf], brand: CardBrand) -> Option<PathBuf> {
    let name = file_name(brand)?;
    dirs.iter()
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

/// Looks for `brand`'s mark in `dirs` and, if there is one, reads, bounds,
/// decodes and normalises it.
pub fn load(dirs: &[PathBuf], brand: CardBrand) -> MarkLoad {
    let Some(path) = find_file(dirs, brand) else {
        return MarkLoad::Missing;
    };
    match read_bounded(&path) {
        Ok(bytes) => match normalise(&bytes) {
            Some(mark) => MarkLoad::Loaded(mark),
            None => MarkLoad::Refused(Refusal::Undecodable),
        },
        Err(refusal) => MarkLoad::Refused(refusal),
    }
}

/// Reads `path` whole, refusing anything over [`MAX_MARK_BYTES`] **without
/// reading it**.
fn read_bounded(path: &Path) -> Result<Vec<u8>, Refusal> {
    let metadata = std::fs::metadata(path).map_err(|_| Refusal::Unreadable)?;
    if metadata.len() > MAX_MARK_BYTES {
        return Err(Refusal::TooManyBytes);
    }
    std::fs::read(path).map_err(|_| Refusal::Unreadable)
}

/// Turns PNG bytes into a [`Mark`], or `None` for anything refused.
///
/// **The dimension bound is applied to the HEADER, before the frame is
/// decoded**, which is the only order in which it protects anything: the
/// header is a few dozen bytes and carries the width and the height, and by
/// the time a decoder has produced pixels the memory it was supposed to bound
/// has already been allocated.
pub fn normalise(png_bytes: &[u8]) -> Option<Mark> {
    let (declared_w, declared_h) = png_dimensions(png_bytes)?;
    if declared_w > MAX_MARK_EDGE_PX || declared_h > MAX_MARK_EDGE_PX {
        return None;
    }
    let (width, height, rgba) = crate::favicon::decode_rgba_unscaled(png_bytes)?;
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }
    let kind = classify(width, height, &rgba);
    let (width, height, rgba) = match kind {
        MarkKind::Isolated => trim_to_ink(width, height, rgba),
        MarkKind::OnGround => (width, height, rgba),
    };
    let (width, height, rgba) = reduce_for_texture(width, height, rgba);
    let (ink_top, ink_bottom) = match kind {
        // Trimmed: the image IS the ink.
        MarkKind::Isolated => (0.0, height as f32),
        MarkKind::OnGround => ground_ink_band(width, height, &rgba),
    };
    Some(Mark { width, height, rgba, kind, ink_top, ink_bottom })
}

/// The width and height a PNG's header declares, without decoding a frame.
fn png_dimensions(png_bytes: &[u8]) -> Option<(u32, u32)> {
    let mut decoder = png::Decoder::new(png_bytes);
    let info = decoder.read_header_info().ok()?;
    Some((info.width, info.height))
}

/// Which of the two kinds `rgba` is: **isolated if its whole one-pixel border
/// is transparent**, on its own ground otherwise.
///
/// The border rather than "does it have any transparency at all", because a
/// reversed mark on a coloured ground may well carry soft edges inside it and
/// is still a rectangle whose edges are the design. And a border rather than
/// the corners alone, because a mark on a *round* transparent ground has
/// transparent corners and opaque edge midpoints, and it is the edge
/// midpoints that say the artwork reaches the edge.
fn classify(width: usize, height: usize, rgba: &[u8]) -> MarkKind {
    let alpha = |x: usize, y: usize| rgba[(y * width + x) * 4 + 3];
    let border_clear = (0..width).all(|x| alpha(x, 0) <= ALPHA_INK && alpha(x, height - 1) <= ALPHA_INK)
        && (0..height).all(|y| alpha(0, y) <= ALPHA_INK && alpha(width - 1, y) <= ALPHA_INK);
    if border_clear {
        MarkKind::Isolated
    } else {
        MarkKind::OnGround
    }
}

/// Crops an isolated mark to the bounding box of its visible pixels.
///
/// A mark published with the brand's mandated clear space baked in as
/// transparency would otherwise be drawn a third smaller than the mark beside
/// it that was exported tight -- and the two would sit on different optical
/// lines. Trimming makes the file's margin irrelevant, which is the only way
/// two files from two brand centres can be made to agree.
///
/// A frame with no visible pixel at all is returned untouched: there is
/// nothing to trim to, and the caller's bounds still hold.
fn trim_to_ink(width: usize, height: usize, rgba: Vec<u8>) -> (usize, usize, Vec<u8>) {
    let alpha = |x: usize, y: usize| rgba[(y * width + x) * 4 + 3];
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if alpha(x, y) > ALPHA_INK {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return (width, height, rgba);
    }
    (x1 - x0, y1 - y0, crop(&rgba, width, x0, y0, x1, y1))
}

/// The rows `y0..y1` and columns `x0..x1` of `rgba`, as their own buffer.
fn crop(rgba: &[u8], width: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        let from = (y * width + x0) * 4;
        out.extend_from_slice(&rgba[from..from + (x1 - x0) * 4]);
    }
    out
}

/// Where the lettering sits inside a mark that carries its own ground.
///
/// The ground is taken to be the colour of the top-left pixel, and the ink is
/// every row holding a pixel that differs from it by more than
/// [`GROUND_TOLERANCE`] on any channel. **Rows, not a full bounding box**:
/// only the vertical band is used, and asking a narrower question of the
/// pixels is how this stays right on a mark whose ground is a gradient across
/// its width.
///
/// Falls back to the whole rectangle when the ground cannot be told from the
/// ink -- a mark whose corners are already its lettering, or a solid
/// rectangle with nothing in it. That is the conservative answer: it draws
/// the file at the height an untrimmed image would get, which is what the
/// caller would have done with no measurement at all.
fn ground_ink_band(width: usize, height: usize, rgba: &[u8]) -> (f32, f32) {
    let at = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    let ground = at(0, 0);
    let differs = |p: [u8; 4]| {
        (0..4).any(|k| (p[k] as i32 - ground[k] as i32).abs() > GROUND_TOLERANCE)
    };
    let mut band: Option<(usize, usize)> = None;
    for y in 0..height {
        if (0..width).any(|x| differs(at(x, y))) {
            band = Some(match band {
                None => (y, y + 1),
                Some((top, _)) => (top, y + 1),
            });
        }
    }
    match band {
        Some((top, bottom)) if bottom > top => (top as f32, bottom as f32),
        _ => (0.0, height as f32),
    }
}

/// Reduces a normalised mark to [`MARK_TEXTURE_PX`] on its longest edge.
///
/// Downscale only, and through `favicon::box_downscale` rather than a second
/// reduction of this module's own -- the argument for area-averaging over a
/// single bilinear tap, and for doing it premultiplied so a transparent edge
/// does not fringe the artwork black, is made in full over there and is the
/// same argument here. A mark already at or under the target is moved
/// through untouched.
fn reduce_for_texture(width: usize, height: usize, rgba: Vec<u8>) -> (usize, usize, Vec<u8>) {
    let longest = width.max(height);
    if longest <= MARK_TEXTURE_PX {
        return (width, height, rgba);
    }
    let scale = MARK_TEXTURE_PX as f64 / longest as f64;
    let dst_w = ((width as f64 * scale).round() as usize).max(1);
    let dst_h = ((height as f64 * scale).round() as usize).max(1);
    let scaled = crate::favicon::box_downscale(&rgba, width, height, dst_w, dst_h);
    (dst_w, dst_h, scaled)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::card_brand::CARD_BRANDS;

    /// Encodes straight RGBA as a PNG, so every test below drives the real
    /// decoder rather than a buffer this module handed itself.
    pub(crate) fn png_of(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
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

    /// An isolated mark: `ink_h` rows of opaque colour, centred in a
    /// transparent frame `height` tall. The transparent margin is the
    /// packaging a brand centre bakes in and the thing `trim_to_ink` exists
    /// to make irrelevant.
    pub(crate) fn isolated_png(width: u32, height: u32, ink_h: u32) -> Vec<u8> {
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let top = (height - ink_h) / 2;
        for y in top..top + ink_h {
            for x in width / 8..width - width / 8 {
                let i = ((y * width + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[0xEB, 0x00, 0x1B, 0xFF]);
            }
        }
        png_of(width, height, &rgba)
    }

    /// A mark on its own ground: white lettering on brand blue, opaque to
    /// every edge, with `ink_h` rows of lettering and the rest the mandated
    /// clear space.
    pub(crate) fn on_ground_png(width: u32, height: u32, ink_h: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&[0x1A, 0x1F, 0x71, 0xFF]);
        }
        let top = (height - ink_h) / 2;
        for y in top..top + ink_h {
            for x in width / 6..width - width / 6 {
                let i = ((y * width + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            }
        }
        png_of(width, height, &rgba)
    }

    #[test]
    fn every_brand_that_names_a_network_has_a_file_name_and_no_two_share_one() {
        // Driven from the enumeration, like the wordmark test beside it: a
        // brand added later cannot ship with no file to look for, and two
        // brands cannot come to read one file.
        let mut seen: Vec<String> = Vec::new();
        for brand in CARD_BRANDS {
            let Some(name) = file_name(brand) else {
                assert_eq!(brand, CardBrand::Other, "{brand:?} has no mark file name");
                continue;
            };
            assert!(name.ends_with(".png"), "{brand:?} reads {name}, which is not a PNG");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '.'),
                "{brand:?}'s file name {name} is not lower-case ASCII"
            );
            assert!(!seen.contains(&name), "{brand:?} reads {name}, and so does another brand");
            seen.push(name);
        }
        assert_eq!(seen.len(), CARD_BRANDS.len() - 1, "one brand -- `Other` -- has no logo");
    }

    #[test]
    fn the_user_directory_is_searched_before_the_one_beside_the_executable() {
        let dir = tempdir("search-order");
        let (user, shipped) = (dir.join("user"), dir.join("shipped"));
        std::fs::create_dir_all(&user).expect("user dir");
        std::fs::create_dir_all(&shipped).expect("shipped dir");
        let name = file_name(CardBrand::Visa).expect("Visa has a file name");
        std::fs::write(user.join(&name), isolated_png(40, 20, 10)).expect("user file");
        std::fs::write(shipped.join(&name), isolated_png(40, 20, 10)).expect("shipped file");

        let found = find_file(&[user.clone(), shipped], CardBrand::Visa).expect("a file");
        assert_eq!(
            found,
            user.join(&name),
            "the shipped mark won over the user's -- a default a user cannot override"
        );
    }

    #[test]
    fn a_brand_with_no_file_anywhere_is_missing_rather_than_refused() {
        let dir = tempdir("missing");
        std::fs::create_dir_all(&dir).expect("dir");
        assert!(
            matches!(load(&[dir.to_path_buf()], CardBrand::Jcb), MarkLoad::Missing),
            "an absent file must be Missing: `Refused` is terminal for the session, and a \
             directory the user is about to drop a file into is not"
        );
    }

    #[test]
    fn an_oversized_file_is_refused_on_its_metadata_before_it_is_read() {
        let dir = tempdir("too-many-bytes");
        std::fs::create_dir_all(&dir).expect("dir");
        let name = file_name(CardBrand::Visa).expect("a name");
        std::fs::write(dir.join(&name), vec![0u8; MAX_MARK_BYTES as usize + 1]).expect("write");
        assert!(
            matches!(load(&[dir.to_path_buf()], CardBrand::Visa), MarkLoad::Refused(Refusal::TooManyBytes)),
            "a file over the byte bound was not refused on its size"
        );
    }

    #[test]
    fn a_frame_larger_than_the_pixel_bound_is_refused_from_its_header() {
        // The header of a 4096-wide PNG, with no pixel data behind it at all:
        // if the bound were applied after the decode this would not refuse,
        // it would fail somewhere else, and on a real 4096x4096 file it would
        // have allocated 64 MB first.
        let mut header = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut header, MAX_MARK_EDGE_PX + 1, 8);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().expect("header");
        }
        assert!(
            normalise(&header).is_none(),
            "a frame over {MAX_MARK_EDGE_PX}px on an edge was accepted"
        );
    }

    #[test]
    fn a_file_that_is_not_a_png_is_refused_rather_than_panicking() {
        let dir = tempdir("undecodable");
        std::fs::create_dir_all(&dir).expect("dir");
        let name = file_name(CardBrand::Maestro).expect("a name");
        std::fs::write(dir.join(&name), b"this is not a PNG, it is a sentence").expect("write");
        assert!(
            matches!(load(&[dir.to_path_buf()], CardBrand::Maestro), MarkLoad::Refused(Refusal::Undecodable)),
            "a non-PNG was not refused"
        );
    }

    #[test]
    fn an_isolated_mark_is_trimmed_to_its_ink_and_a_grounded_one_is_not() {
        let isolated = normalise(&isolated_png(64, 64, 16)).expect("an isolated mark decodes");
        assert_eq!(isolated.kind, MarkKind::Isolated);
        assert_eq!(
            isolated.height, 16,
            "the transparent clear space survived the trim, so this mark would draw a third \
             the size of the one beside it"
        );
        assert_eq!(
            (isolated.ink_top, isolated.ink_bottom),
            (0.0, 16.0),
            "a trimmed mark's ink is its whole height"
        );

        let grounded = normalise(&on_ground_png(96, 32, 12)).expect("a grounded mark decodes");
        assert_eq!(grounded.kind, MarkKind::OnGround);
        assert_eq!(
            (grounded.width, grounded.height),
            (96, 32),
            "the coloured ground was cropped -- that is a mark the brand does not have"
        );
        assert_eq!(
            grounded.ink_height(),
            12.0,
            "the lettering inside the ground was not found, so the clear space would be drawn \
             as though it were part of the mark"
        );
        assert_eq!(grounded.ink_center(), 16.0, "the lettering is centred in the ground");
    }

    #[test]
    fn a_mark_whose_ink_reaches_its_own_edges_is_read_as_grounded() {
        // The degenerate isolated case: no margin at all. There is no
        // transparent border to find, so this is read as "the rectangle is
        // the mark", which draws the file exactly as given -- the
        // conservative answer, and never a crop.
        let mut rgba = Vec::new();
        for _ in 0..32 * 16 {
            rgba.extend_from_slice(&[0x00, 0x77, 0x33, 0xFF]);
        }
        let mark = normalise(&png_of(32, 16, &rgba)).expect("decodes");
        assert_eq!(mark.kind, MarkKind::OnGround);
        assert_eq!((mark.ink_top, mark.ink_bottom), (0.0, 16.0));
    }

    #[test]
    fn a_huge_mark_is_reduced_before_it_becomes_a_texture() {
        let mark = normalise(&on_ground_png(1024, 512, 200)).expect("decodes");
        assert!(
            mark.width.max(mark.height) <= MARK_TEXTURE_PX,
            "a {}x{} texture was kept for a mark drawn at about 32 physical pixels",
            mark.width,
            mark.height
        );
        assert_eq!(mark.rgba.len(), mark.width * mark.height * 4);
        // The ink band survives the reduction proportionally: it is measured
        // after it, so it cannot drift out of step with the pixels it
        // describes.
        let fraction = mark.ink_height() / mark.height as f32;
        assert!(
            (fraction - 200.0 / 512.0).abs() < 0.05,
            "the ink band came out at {fraction} of the reduced image, not the {} it was",
            200.0 / 512.0
        );
    }

    /// A directory under the process's temp dir, **never
    /// `%APPDATA%\\Deskwarden`**: every test here writes files, and the one
    /// directory this app's real marks would live in is the one no test may
    /// touch.
    /// **Removed when the returned guard drops**, panic included. The card
    /// art these tests write is why: a run left twenty-six `deskwarden-marks-*`
    /// directories behind, and the `visa.png`, `mastercard.png` and `jcb.png`
    /// files inside them were 7,438 of the files found abandoned in `%TEMP%`.
    ///
    /// Bind the result -- `let dir = tempdir("x");` -- rather than using it in
    /// place: the directory lives exactly as long as the binding does.
    pub(crate) fn tempdir(tag: &str) -> crate::test_scratch::ScratchDir {
        crate::test_scratch::ScratchDir::new(&format!("marks-{tag}"))
    }
}

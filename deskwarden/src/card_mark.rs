//! The network mark: a card's network, SET IN TYPE on a small ground, or --
//! where the user has supplied the image and asked for it -- the network's own
//! logo.
//!
//! **The word is the mark, and the logo is an option over it.** The words come
//! from [`CardBrand::wordmark`], which is the one place brands are named --
//! there is no second table here to fall out of step with it. Naming which
//! network a card belongs to is a statement of fact about the user's own card,
//! and `VISA`, `MASTERCARD`, `AMEX` state it.
//!
//! **Logos are drawn when three things are all true**: the user has turned
//! [`crate::settings::Settings::use_brand_logos`] on, a file for that brand is
//! in one of [`crate::brand_mark::search_dirs`]' directories, and it survived
//! that module's bounds. Otherwise the word is drawn -- setting off, no file,
//! unreadable file, undecodable file, a file whose pixels are 20000 on a side.
//! **Every one of those is the same fallback and there is no fourth
//! outcome**, which is the property the whole feature rests on: a user with
//! six of nine files present reads six logos and three words, and nothing on
//! screen looks broken or empty.
//!
//! No image is compiled in and none ships with the source, so this code lands
//! and is exercised entirely against that fallback. See [`crate::brand_mark`]
//! for why runtime loading rather than `include_bytes!`.
//!
//! # How a logo is made to fit the slot the word had
//!
//! The mark's BOX is always `height` tall whichever is drawn, and only its
//! width varies -- so `item_list`'s ink alignment and its truncation budget,
//! both of which are computed off the wordmark's own galley, hold unchanged
//! with a logo in the box.
//!
//! Inside that box a logo is fitted by its INK and not by its rectangle,
//! because the two kinds of file a brand centre publishes disagree about what
//! their rectangle means. An isolated mark on transparency has been trimmed to
//! its ink by `brand_mark`; a reversed mark on the brand's own colour carries
//! that brand's mandated clear space *inside* the image, so its lettering is
//! often barely half the file's height. Scaled to one image height, the second
//! renders visibly smaller than the first and smaller than the words beside
//! it. So both are scaled until their ink stands exactly as tall as the
//! wordmark's ink at this size ([`theme::ink_band_y`]), and both are placed so
//! that their ink centre lands where the word's ink centre would have. Mixed
//! sources then read as one set. See [`logo_fit`].
//!
//! **A logo is drawn BARE -- no pill, no ground.** Three treatments were
//! rendered side by side at the row's real geometry before this was picked:
//! this app's blue pill behind the logo, a neutral ground behind it, and
//! nothing. The blue pill loses either way round: behind an isolated mark it
//! reads as a badge this app invented, in colours the network does not use,
//! and behind a reversed mark it puts one navy rectangle on another and shows
//! as a thin rim of the wrong blue. The neutral ground leaves a full-bleed
//! rectangle sitting inside a pale halo, so a row of mixed sources reads as
//! two different components. Bare, both read as marks: an isolated logo sits
//! like a small logo, and a full-bleed one IS its own pill -- which is what
//! its designer intended it to be. The word keeps the blue pill it has always
//! had, because a word with no ground is not a mark, it is a word.
//!
//! **What this replaced, and why the replacement is not a step down.** Until
//! now the marks were seven PNGs generated from source geometry: a wedge, a
//! diamond, two bars, a ring, each in this app's own blue on an identical blue
//! rounded square. They were honest about not being logos and they were
//! useless. A user reported "VISA icon supposed to be visa and not some Play
//! sign", which is exactly right -- a play triangle names no network, and
//! seven abstract glyphs in one palette do not tell each other apart either. A
//! word is not a logo, but it says which network the card is on, which is the
//! only thing the badge was ever for.
//!
//! An unrecognised brand has no mark: every entry point here takes a
//! [`CardBrand`], and a caller that could not name a brand draws nothing
//! rather than a placeholder.

use crate::brand_mark::{self, MarkLoad};
use crate::card_brand::CardBrand;
use crate::theme;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The mark's height on the detail pane's Number row.
///
/// The size the read pane draws the matched app's icon at, and deliberately
/// the same: both are "a small thing identifying the row, immediately before
/// the row's value".
pub const MARK_DETAIL_HEIGHT: f32 = 18.0;

/// The mark's height in an item list row, where it sits BESIDE the avatar
/// tile rather than inside it.
///
/// **15, because 15 is the height at which [`text_size`] sets the type at
/// 9pt** -- four points under the 13pt the item NAME is set at, and two under
/// the 11pt of the username line below it.
///
/// **The step is the hierarchy, and the hierarchy is the point.** The name is
/// the thing being identified; the network is a qualifier on it. Type size is
/// how a reader is told which is which, and a pill set near the name's size
/// stops annotating the name and starts competing with it.
///
/// **Chosen off a rendered ladder, not by argument.** The pill was drawn at
/// 8, 9, 10 and 11pt and looked at unmagnified, because legibility at 1x is
/// the whole trade being made -- a wordmark that only resolves under a
/// magnifier is decoration rather than information. At 11 the pill reads as a
/// second title; at 8, the size the old corner badge used, the longest word
/// this app sets (`MASTERCARD`) starts to close up. 9 is the quietest rung on
/// which all ten wordmarks are still words at 1x.
///
/// The size also settles the abbreviation question, which is why the two were
/// decided together: smaller type is what lets `MASTERCARD`, `UNIONPAY`,
/// `MAESTRO` and `RUPAY` be spelled out at all. `CardBrand::wordmark` carries
/// those measurements.
///
/// The predecessor of this constant was 13pt tall (8pt type) and existed
/// because the badge was drawn inside the row's 32pt tile, which was its
/// entire width budget. Nothing is drawn inside the tile any more, so the
/// budget is the row's.
pub const MARK_ROW_HEIGHT: f32 = 15.0;

/// The type size for a mark drawn `height` tall.
///
/// A ratio rather than a constant per size, so the row mark and the detail
/// mark are one design at two sizes instead of two designs. 0.62 is set
/// against `theme::avatar`'s own 0.38 monogram ratio: this ground is a tight
/// pill around a word rather than a square with a letter floating in it, so
/// the type fills much more of it. The padding beside the word stays tight
/// because a pill that is mostly padding beside a 13pt word reads as a button
/// rather than as a mark.
fn text_size(height: f32) -> f32 {
    (height * 0.62).round()
}

/// The ground's padding either side of the word.
fn pad_x(height: f32) -> f32 {
    (height * 0.22).round()
}

/// [`galley`], for a caller that has to line the word up against text of its
/// own.
///
/// The item list puts the pill's word on the item NAME's optical line, and
/// where a run's ink falls is a property of the run laid out rather than a
/// number anyone can write down -- the same argument `detail`'s
/// `digits_baseline_drop` makes. Handing out the very galley [`paint_mark`]
/// paints is what keeps that calculation about the run really drawn: a caller
/// laying out its own copy could drift from this one the moment the face or
/// the size moved, and the size here moved three times before it settled.
pub fn word_galley(ui: &egui::Ui, brand: CardBrand, height: f32) -> std::sync::Arc<egui::Galley> {
    galley(ui, brand, height)
}

/// The word, laid out at the size a mark of `height` sets it.
///
/// Bold and letterspaced, which is what makes a short word read as a
/// wordmark rather than as a truncated string: the design's own "card header
/// wordmark" style is `11px / 700 / uppercase / letter-spacing 0.1em`, and
/// this is that style at whatever size the mark is drawn.
fn galley(ui: &egui::Ui, brand: CardBrand, height: f32) -> std::sync::Arc<egui::Galley> {
    let job = theme::letterspaced(
        brand.wordmark(),
        text_size(height),
        theme::BOLD,
        0.06,
        theme::CARD,
    );
    ui.painter().layout_job(job)
}

/// The widest a logo may be drawn, as a multiple of the mark's height.
///
/// **A bound on the row's budget, not on taste.** The list row allocates the
/// mark's box out of the same width the item name is truncated into, and the
/// existing rule ([`crate::vault_window::item_list`]'s
/// `NETWORK_MARK_MIN_TITLE_ROOM`) is what stands the mark aside when the name
/// would be left with nothing. This constant keeps a logo from ever *reaching*
/// that rule where a word would not have: at [`MARK_ROW_HEIGHT`] it is 60pt,
/// under the 72pt `MASTERCARD` already takes, so no file a user drops in can
/// cost the name more room than this app's own widest wordmark does. A file
/// wider than this is not refused -- it is fitted to the cap, which costs it
/// some ink height and keeps it a mark rather than making it a word.
///
/// **The cap was chosen while the mark LED the row, and it survives the mark
/// moving to the trailing edge unchanged** -- every term above is about width
/// taken out of the name's budget, and none of them is about which side of the
/// name it is taken from. The row allocates the same box out of the same total
/// either way. `item_list`'s `a_marked_card_on_a_narrow_pane_keeps_its_mark_
/// inside_the_tile_and_squeezes_the_title_instead` now drives a deliberately
/// 10:1 file through the moved layout and reads the cap back off the painted
/// image, so this is pinned by what is drawn rather than by the arithmetic
/// here.
const MAX_LOGO_ASPECT: f32 = 4.0;

/// How long a brand whose file was NOT FOUND is left alone before the
/// directories are looked at again.
///
/// **A miss and a refusal are cached differently, and this is the difference.**
/// A refusal is a property of a file that is there: re-reading it costs the
/// same bounded work and yields the same answer, so it stands for the session.
/// A miss is a property of a directory the user may be about to drop a file
/// into -- and "marked dealt with for the session, never looked at again" is
/// the exact shape of the bug the favicon cache once had, where an item with
/// no domain was written off and never retried. Two seconds is short enough
/// that dropping a file in and looking at the vault window shows it, and long
/// enough that nine brands on a scrolling list are not nine `stat` calls a
/// frame.
const ABSENT_RECHECK: Duration = Duration::from_secs(2);

/// What this module was told about logos, before it has looked at any file.
///
/// Held per [`egui::Context`], so a window carries the answer it opened with
/// -- the same granularity `vault_window` already reads
/// [`crate::settings::Settings::fetch_icons`] at, and for the same reason: a
/// preference re-read mid-frame is a display that changes under a user who is
/// looking at it.
#[derive(Clone)]
pub struct LogoPolicy {
    /// [`crate::settings::Settings::use_brand_logos`]. `false` means no file
    /// is ever opened.
    pub enabled: bool,
    /// Where to look, in order. Normally [`brand_mark::search_dirs`].
    pub dirs: Arc<Vec<PathBuf>>,
}

impl LogoPolicy {
    /// Logos off, and nowhere to look.
    ///
    /// **What every test harness in this crate installs before it paints**,
    /// and the reason it is a constructor rather than a struct literal spelled
    /// out in fourteen places: a context with no policy installed resolves the
    /// developer's own `settings.json` and their own marks folder, so a suite
    /// that asserts on painted wordmarks would pass or fail according to what
    /// is in a directory on one machine. It is equally the honest production
    /// answer for a platform with no resolvable config directory.
    pub fn off() -> Self {
        LogoPolicy { enabled: false, dirs: Arc::new(Vec::new()) }
    }
}

/// What is known about one brand's file.
#[derive(Clone)]
enum Slot {
    /// No file, as of this instant. Re-checked after [`ABSENT_RECHECK`].
    Absent(Instant),
    /// A file was there and will not be drawn. Terminal for this context.
    Refused,
    /// A texture, and where its ink sits in it.
    Ready(Arc<Logo>),
}

/// A decoded, uploaded brand logo.
struct Logo {
    texture: egui::TextureHandle,
    /// The texture's own pixel dimensions.
    size: egui::Vec2,
    /// The ink's top and bottom within those pixels; see
    /// [`brand_mark::Mark::ink_top`].
    ink_top: f32,
    ink_bottom: f32,
}

impl Logo {
    /// Where the ink is centred, in the texture's own pixels.
    fn ink_center(&self) -> f32 {
        (self.ink_top + self.ink_bottom) / 2.0
    }
}

/// The per-context cache: the policy, and one slot per brand looked at so far.
///
/// A `Vec` of pairs rather than a map because there are ten brands, at most
/// nine of which can have a file -- a linear scan over nine entries is not a
/// thing to build a hash for.
#[derive(Clone)]
struct LogoCache {
    policy: LogoPolicy,
    slots: Vec<(CardBrand, Slot)>,
}

fn cache_id() -> egui::Id {
    egui::Id::new("card-mark-logo-cache")
}

/// Tells this module, for `ctx`, whether logos are wanted and where to look.
///
/// **The seam, and the only one.** Production calls it with the user's setting
/// and [`brand_mark::search_dirs`]; a test calls it with a temp directory it
/// filled itself, which is what keeps the suite off the real
/// `%APPDATA%\Deskwarden`. It is an ordinary function rather than a
/// `cfg(test)` branch precisely so that the path the tests exercise is the
/// path that ships.
///
/// Calling it discards whatever was cached for `ctx`: the policy it carries is
/// what every slot in it was decided under.
pub fn install_logo_policy(ctx: &egui::Context, policy: LogoPolicy) {
    ctx.data_mut(|data| {
        data.insert_temp(cache_id(), LogoCache { policy, slots: Vec::new() });
    });
}

/// The policy in force for `ctx`, reading the user's settings the first time
/// it is asked and remembering the answer.
///
/// The read is one `settings.json` parse per context, on the first card mark
/// drawn in it. A window that draws no card mark never touches the file.
fn policy(ctx: &egui::Context) -> LogoPolicy {
    if let Some(cache) = ctx.data(|data| data.get_temp::<LogoCache>(cache_id())) {
        return cache.policy;
    }
    let enabled = crate::settings::default_path()
        .map(|path| crate::settings::Settings::load(&path).use_brand_logos)
        .unwrap_or(false);
    let policy = if enabled {
        LogoPolicy { enabled, dirs: Arc::new(brand_mark::search_dirs()) }
    } else {
        // Not even the directory names are resolved with the preference off:
        // the setting means "do not look", and looking is what a search path
        // is for.
        LogoPolicy::off()
    };
    install_logo_policy(ctx, policy.clone());
    policy
}

/// The logo to draw for `brand`, or `None` for every case in which the word is
/// drawn instead.
///
/// Decoding happens at most once per brand per context; a scrolling list of
/// four hundred cards asks this question every frame and gets a clone of an
/// `Arc` for it.
fn logo(ui: &egui::Ui, brand: CardBrand) -> Option<Arc<Logo>> {
    let ctx = ui.ctx();
    let policy = policy(ctx);
    if !policy.enabled {
        return None;
    }
    match cached(ctx, brand) {
        Some(Slot::Ready(logo)) => return Some(logo),
        Some(Slot::Refused) => return None,
        Some(Slot::Absent(since)) if since.elapsed() < ABSENT_RECHECK => return None,
        // Never looked, or looked long enough ago that a file the user has
        // since dropped in deserves to be found.
        _ => {}
    }
    // Deliberately outside the `data_mut` above and the one below: this reads
    // a file, decodes it and uploads a texture, and `egui`'s data map is
    // locked while a closure over it runs.
    let slot = match brand_mark::load(&policy.dirs, brand) {
        MarkLoad::Missing => Slot::Absent(Instant::now()),
        MarkLoad::Refused(why) => {
            log::debug!("brand mark for {brand:?} refused ({why:?}); drawing the wordmark");
            Slot::Refused
        }
        MarkLoad::Loaded(mark) => {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [mark.width, mark.height],
                &mark.rgba,
            );
            let texture = ctx.load_texture(
                format!("card-mark:{brand:?}"),
                image,
                egui::TextureOptions::default(),
            );
            Slot::Ready(Arc::new(Logo {
                texture,
                size: egui::vec2(mark.width as f32, mark.height as f32),
                ink_top: mark.ink_top,
                ink_bottom: mark.ink_bottom,
            }))
        }
    };
    let ready = match &slot {
        Slot::Ready(logo) => Some(logo.clone()),
        _ => None,
    };
    remember(ctx, brand, slot);
    ready
}

fn cached(ctx: &egui::Context, brand: CardBrand) -> Option<Slot> {
    ctx.data(|data| {
        data.get_temp::<LogoCache>(cache_id())
            .and_then(|cache| cache.slots.iter().find(|(b, _)| *b == brand).map(|(_, s)| s.clone()))
    })
}

fn remember(ctx: &egui::Context, brand: CardBrand, slot: Slot) {
    ctx.data_mut(|data| {
        let Some(mut cache) = data.get_temp::<LogoCache>(cache_id()) else {
            // No cache to write into: `logo` installs one before it ever gets
            // here, so this is unreachable in practice and is a `return`
            // rather than an insert precisely so that a slot can never be
            // remembered under a policy nobody set.
            return;
        };
        match cache.slots.iter_mut().find(|(b, _)| *b == brand) {
            Some(entry) => entry.1 = slot,
            None => cache.slots.push((brand, slot)),
        }
        data.insert_temp(cache_id(), cache);
    });
}

/// Where a logo is drawn inside a mark box `height` tall, given the word it
/// stands in for.
///
/// Returns the image's rect **relative to the top-left of the mark's box**,
/// padding included, and the box's own width.
///
/// Three rules, in this order:
///
/// * **Scale by ink.** `scale = ink_target / logo_ink`, where `ink_target` is
///   how tall the wordmark's own ink stands at this size. This is the rule
///   that makes an isolated mark and a full-bleed one -- whose lettering may
///   be half its file's height -- come out at the same optical size.
/// * **Then bound the box.** The image may not be taller than `height` or
///   wider than [`MAX_LOGO_ASPECT`] times it. A file with extravagant clear
///   space, or one three times wider than it is tall, is fitted to the bound
///   and loses some ink height; the alternative is a mark that overflows a row
///   whose geometry is fixed.
///
///   **Where the height bound bites, measured.** At [`MARK_ROW_HEIGHT`] the
///   wordmark's ink stands about 6pt in a 15pt box, so a mark on its own
///   ground whose lettering is under about **40% of its file's height** hits
///   the bound and draws its ink proportionally smaller -- a file at 30% comes
///   out about a quarter under the words beside it. Assets from brand centres
///   measured well over that line (the reversed marks seen so far run 45-50%),
///   and an isolated mark can never hit it at all, because trimming makes its
///   ink its whole height. The failure is graceful and bounded either way: a
///   mark slightly small, never a row whose height depends on which files the
///   user downloaded.
/// * **Then centre on the ink**, not on the rectangle: the image is placed so
///   its ink centre lands exactly where the word's ink centre sits inside the
///   same box. Box-centring is what put the pill visibly off the name's line
///   before `0d28e9f`, and a full-bleed logo whose lettering sits low in its
///   own clear space would reintroduce it.
fn logo_fit(logo: &Logo, height: f32, word: &egui::Galley) -> (egui::Rect, f32) {
    let (word_top, word_bottom) = theme::ink_band_y(word).unwrap_or((0.0, height));
    // The word's own ink, as `paint_mark` places it: its galley is centred in
    // the box, so its ink band moves with it.
    let word_offset = (height - word.size().y) / 2.0;
    let ink_target = (word_bottom - word_top).max(1.0);
    let word_ink_center = word_offset + (word_top + word_bottom) / 2.0;

    let logo_ink = (logo.ink_bottom - logo.ink_top).max(1.0);
    let mut scale = ink_target / logo_ink;
    scale = scale.min(height / logo.size.y.max(1.0));
    scale = scale.min(MAX_LOGO_ASPECT * height / logo.size.x.max(1.0));
    let drawn = logo.size * scale;

    let pad = pad_x(height);
    // **The ink centre lands on the word's, and is deliberately NOT clamped
    // back inside the box afterwards.** The bound above already keeps the
    // image from being taller than the box, so the most it can overhang is the
    // distance between the box's centre and the word's ink centre -- a
    // fraction of a point, because a run of capitals sits very nearly in the
    // middle of its own pill by construction.
    //
    // Clamping is what this did first, and it was measurably wrong: on a
    // reversed mark whose lettering is two fifths of its file's height, the
    // fit hits the box-height bound, the clamp then pins the image to the top
    // of the box, and the mark lands half a point off the line every other
    // mark on the row sits on -- which is the exact defect the whole fit
    // exists to avoid. Half a point is not visible on its own; a row where
    // SOME marks have it and others do not is what a reader sees as ragged.
    let top = word_ink_center - logo.ink_center() * scale;
    (
        egui::Rect::from_min_size(egui::pos2(pad, top), drawn),
        drawn.x + 2.0 * pad,
    )
}

/// How wide the mark for `brand` is when drawn `height` tall.
///
/// Measured off the same galley -- or, when there is one, the same logo --
/// [`paint_mark`] paints, so a caller that reserves room for a mark reserves
/// the room the mark takes. The detail pane needs this before it lays out the
/// digits beside it, and a mark left out of that sum is a mark that pushes the
/// reveal eye off a narrow pane. The list row needs it before it truncates the
/// item name into what is left.
///
/// **The two must not be able to disagree**, which is why both answers come
/// out of one place: the logo's width is [`logo_fit`]'s, the same function
/// that positions the image, and the word's is its galley's. Measuring one and
/// drawing the other is how a name comes to be laid out into room the mark is
/// sitting in.
pub fn mark_width(ui: &egui::Ui, brand: CardBrand, height: f32) -> f32 {
    let word = galley(ui, brand, height);
    match logo(ui, brand) {
        Some(logo) => logo_fit(&logo, height, &word).1,
        None => word.size().x + 2.0 * pad_x(height),
    }
}

/// Paints the mark for `brand`, `height` tall, anchored at `pos`; returns the
/// rect it took.
///
/// **A word gets the blue pill; a logo is drawn bare.** The ground under a
/// word is `theme::BLUE` with the word in `theme::CARD` -- this app's own blue
/// and its own white, and no network's colours, so the marks are told apart by
/// their WORD alone, which is why the word has to be legible and why
/// [`MARK_ROW_HEIGHT`] is pinned by a measurement rather than tuned by eye. A
/// logo brings its own colour and, when it is a reversed mark, its own ground;
/// the module header records the three treatments that were rendered before
/// bare was picked.
///
/// **The returned rect is the same box either way** -- `height` tall, as wide
/// as [`mark_width`] said -- so a caller that anchored a word gets a logo in
/// exactly the slot it reserved.
pub fn paint_mark(
    ui: &egui::Ui,
    brand: CardBrand,
    height: f32,
    anchor: egui::Align2,
    pos: egui::Pos2,
) -> egui::Rect {
    let galley = galley(ui, brand, height);
    if let Some(logo) = logo(ui, brand) {
        let (image, width) = logo_fit(&logo, height, &galley);
        let rect = anchor.anchor_size(pos, egui::vec2(width, height));
        // `logo_fit` answers in the box's own coordinates, so the box can be
        // anchored anywhere and the image follows it.
        let at = image.translate(rect.min.to_vec2());
        egui::Image::new((logo.texture.id(), logo.texture.size_vec2())).paint_at(ui, at);
        return rect;
    }
    let size = egui::vec2(galley.size().x + 2.0 * pad_x(height), height);
    let rect = anchor.anchor_size(pos, size);
    ui.painter()
        .rect_filled(rect, theme::avatar_corner_radius(height), theme::BLUE);
    // Centred on the ground rather than placed at a padding from its corner:
    // a galley's height is the font's line height, which is taller than the
    // capitals in it, so centring is what puts the word optically in the
    // middle of the pill at every size.
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, theme::CARD);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_brand::CARD_BRANDS;

    fn raw() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 400.0),
            )),
            ..Default::default()
        }
    }

    /// A context with the app's own fonts really loaded, because everything
    /// this module claims is a claim about laid-out text.
    ///
    /// **Its logo policy is installed off, and that is not incidental.** With
    /// none installed, [`policy`] resolves the user's real `settings.json`
    /// and, if they have turned the preference on, their real
    /// `%APPDATA%\\Deskwarden\\brand-marks` -- so the suite would pass or fail
    /// depending on what the developer happens to have in a folder. Every
    /// context in this module's tests is told what to think first, and the
    /// ones that want a logo are told where to find one they wrote themselves.
    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw(), |_ui| {});
        install_logo_policy(&ctx, no_logos());
        ctx
    }

    /// The policy every context here starts under: logos off, nowhere to look.
    pub(crate) fn no_logos() -> LogoPolicy {
        LogoPolicy::off()
    }

    fn with_ui<R>(f: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let ctx = ctx();
        let mut f = Some(f);
        let mut out = None;
        let _ = ctx.run_ui(raw(), |ui| {
            if let Some(f) = f.take() {
                out = Some(f(ui));
            }
        });
        out.expect("the closure ran")
    }

    #[test]
    fn every_brand_has_a_wordmark_and_no_two_share_one() {
        // Driven from the enumeration and not from a list written here, so a
        // brand added later cannot ship markless. The distinctness half is the
        // extreme form of the failure this design is exposed to: two networks
        // that cannot be told apart because they are the same word.
        let mut seen: Vec<&str> = Vec::new();
        for brand in CARD_BRANDS {
            let word = brand.wordmark();
            assert!(!word.is_empty(), "{brand:?} has no wordmark");
            assert!(
                !seen.contains(&word),
                "{brand:?} and an earlier brand are both {word:?}"
            );
            seen.push(word);
        }
        assert_eq!(seen.len(), CARD_BRANDS.len());
    }

    #[test]
    fn a_wordmark_is_plain_upper_case_ascii() {
        // No length rule any more -- the four-character cap was the 32pt
        // tile's width and the mark left the tile. What survives is the form:
        // these are set in a bold letterspaced upper-case face, and a
        // lower-case glyph in that run reads as a typo rather than as a name.
        for brand in CARD_BRANDS {
            let word = brand.wordmark();
            assert!(!word.is_empty(), "{brand:?} has no wordmark");
            assert!(
                word.chars().all(|c| c.is_ascii_uppercase() || c == ' '),
                "{brand:?}'s wordmark {word:?} is not plain upper-case ASCII"
            );
        }
    }

    /// **The mark is set BELOW the item name, and that is the hierarchy.**
    /// Both ends are pinned: comfortably smaller than the name, so the pill
    /// qualifies the thing it sits beside rather than competing with it, and
    /// not smaller than the corner badge it replaced, which is the size at
    /// which the longest wordmark stops reading at 1x.
    #[test]
    fn a_row_mark_is_set_well_below_the_item_name() {
        // Spelled as the numbers they are because `item_list::TITLE_SIZE` and
        // `SUBTITLE_SIZE` are private to that module; the row really laying
        // its name out at 13 and its username at 11 is that module's own
        // assertion.
        const TITLE_SIZE: f32 = 13.0;
        const SUBTITLE_SIZE: f32 = 11.0;
        // The old corner badge's type size -- the floor the rendered ladder
        // put under this, not a round number.
        const OLD_BADGE_SIZE: f32 = 8.0;
        let set = text_size(MARK_ROW_HEIGHT);
        assert!(
            set < SUBTITLE_SIZE,
            "a {MARK_ROW_HEIGHT}pt mark sets its word at {set}pt, which is not below even the              row's secondary {SUBTITLE_SIZE}pt, let alone the name's {TITLE_SIZE}pt"
        );
        assert!(
            set >= OLD_BADGE_SIZE,
            "the mark is set at {set}pt, under the {OLD_BADGE_SIZE}pt the corner badge used --              below which `MASTERCARD` stops being a word at 1x"
        );
    }

    /// **The measurement the placement rests on.** The mark now sits beside
    /// the 32pt tile on a row in a pane fixed at `vault_window::LIST_WIDTH`,
    /// so what it must fit is the row -- and it must leave the item name it
    /// annotates enough room to still be a name.
    #[test]
    fn every_wordmark_fits_the_row_and_leaves_the_name_its_room() {
        // The title column before any pill is taken out of it: the 390pt pane
        // less its 10pt padding either side, the row's 12pt padding and 1pt
        // border either side, the 32pt tile and the row's 11pt gap.
        const TITLE_COLUMN: f32 = 390.0 - 2.0 * 10.0 - 2.0 * 12.0 - 2.0 * 1.0 - 32.0 - 11.0;
        // What a name plus its `(*9988)` suffix needs to still read as both.
        const NAME_ROOM: f32 = 120.0;
        const GAP: f32 = 11.0;
        with_ui(|ui| {
            for brand in CARD_BRANDS {
                let width = mark_width(ui, brand, MARK_ROW_HEIGHT);
                assert!(
                    width + GAP + NAME_ROOM <= TITLE_COLUMN,
                    "{brand:?}'s {:?} pill is {width}pt wide at {MARK_ROW_HEIGHT}pt tall, which \
                     leaves the name {}pt of the {TITLE_COLUMN}pt column -- under the {NAME_ROOM}pt \
                     a name and its digits need",
                    brand.wordmark(),
                    TITLE_COLUMN - width - GAP
                );
                // ...and the negative: a word taking real room, not a mark
                // that fits because it has shrunk to nearly nothing.
                assert!(
                    width > MARK_ROW_HEIGHT,
                    "{brand:?}'s pill is only {width}pt wide, which is not a word"
                );
            }
        });
    }

    #[test]
    fn a_mark_paints_a_ground_and_its_word_at_the_anchor_it_was_given() {
        // The claim `paint_mark`'s callers rely on: the rect it RETURNS is the
        // rect it PAINTED, so a caller anchoring a badge to a tile's corner
        // gets a badge at that corner rather than one measured one way and
        // drawn another.
        let ctx = ctx();
        let mut painted = egui::Rect::NOTHING;
        let output = ctx.run_ui(raw(), |ui| {
            painted = paint_mark(
                ui,
                CardBrand::Visa,
                MARK_ROW_HEIGHT,
                egui::Align2::RIGHT_BOTTOM,
                egui::Pos2::new(100.0, 60.0),
            );
        });
        assert!(
            (painted.right() - 100.0).abs() < 0.01 && (painted.bottom() - 60.0).abs() < 0.01,
            "the mark was anchored RIGHT_BOTTOM at (100, 60) but landed at {painted:?}"
        );
        assert!((painted.height() - MARK_ROW_HEIGHT).abs() < 0.01);

        let (grounds, words) = collect(&output.shapes);
        assert!(
            grounds
                .iter()
                .any(|(rect, fill)| *fill == theme::BLUE
                    && (rect.width() - painted.width()).abs() < 0.01),
            "no BLUE ground the width of the returned rect was painted: {grounds:?}"
        );
        assert_eq!(
            words,
            vec![CardBrand::Visa.wordmark().to_string()],
            "the mark painted something other than its own word"
        );
    }

    // ---- brand logos ---------------------------------------------------
    //
    // Every test below writes its own fixture images into its own temp
    // directory and points the context at it. **None of them can reach
    // `%APPDATA%\Deskwarden\brand-marks`**, which is the directory the real
    // marks would live in and the one a test must never read: the policy is
    // installed before a mark is drawn, and an installed policy is the only
    // thing `logo` consults.

    use crate::brand_mark::tests::{isolated_png, on_ground_png, tempdir};

    /// A context whose logo policy points at a directory holding exactly
    /// `files`, with the setting `enabled`.
    ///
    /// **The directory comes back with the context and the caller must bind
    /// it.** The policy holds a path, not the files, and every mark is read
    /// off disk at paint time -- so a guard dropped here would delete the
    /// fixture before the first frame, and every test below would pass or fail
    /// on an empty directory rather than on the images it wrote.
    fn ctx_with_marks(
        enabled: bool,
        files: &[(CardBrand, Vec<u8>)],
    ) -> (egui::Context, crate::test_scratch::ScratchDir) {
        let dir = tempdir("card-mark");
        for (brand, bytes) in files {
            let name = crate::brand_mark::file_name(*brand).expect("a brand with a file name");
            std::fs::write(dir.join(name), bytes).expect("the fixture was written");
        }
        let ctx = ctx();
        install_logo_policy(
            &ctx,
            LogoPolicy { enabled, dirs: Arc::new(vec![dir.to_path_buf()]) },
        );
        (ctx, dir)
    }

    /// Paints one mark and reports what really reached the screen: the words,
    /// and the rect of every TEXTURED shape -- which is what an image is.
    fn painted(ctx: &egui::Context, brand: CardBrand, height: f32) -> (Vec<String>, Vec<egui::Rect>) {
        let output = ctx.run_ui(raw(), |ui| {
            paint_mark(ui, brand, height, egui::Align2::LEFT_TOP, egui::pos2(20.0, 20.0));
        });
        let (grounds, words) = collect(&output.shapes);
        let _ = grounds;
        (words, images(&output.shapes))
    }

    fn images(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.brush.is_some() => out.push(rect.rect),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// **The positive case, first, because every fallback test below is
    /// worthless without it**: a test that asserts "the word was drawn" proves
    /// nothing if the image path never draws an image under any circumstances.
    #[test]
    fn a_valid_file_with_the_setting_on_draws_the_image_and_not_the_word() {
        let (ctx, _marks) = ctx_with_marks(true, &[(CardBrand::Visa, isolated_png(96, 48, 24))]);
        let (words, images) = painted(&ctx, CardBrand::Visa, MARK_ROW_HEIGHT);
        assert!(
            images.len() == 1,
            "the logo was not drawn: {} textured shapes reached the screen",
            images.len()
        );
        assert!(
            !words.contains(&CardBrand::Visa.wordmark().to_string()),
            "the logo AND the word were both drawn, so they are stacked on one another"
        );
    }

    /// **Failure mode one: the setting is off.** The file is right there and
    /// perfectly good, which is the point -- nothing but the preference
    /// decides this.
    #[test]
    fn the_setting_off_draws_the_word_even_with_a_perfectly_good_file_present() {
        let files = [(CardBrand::Visa, isolated_png(96, 48, 24))];
        // The control: the very same fixture, with the setting on, is a logo.
        let (on_ctx, _on_marks) = ctx_with_marks(true, &files);
        let (_, drawn) = painted(&on_ctx, CardBrand::Visa, MARK_ROW_HEIGHT);
        assert_eq!(drawn.len(), 1, "the premise: this fixture does draw as a logo");

        let (ctx, _marks) = ctx_with_marks(false, &files);
        let (words, images) = painted(&ctx, CardBrand::Visa, MARK_ROW_HEIGHT);
        assert!(images.is_empty(), "a logo was drawn with the preference off");
        assert_eq!(words, vec![CardBrand::Visa.wordmark().to_string()]);
    }

    /// **Failure mode two: no file for that brand.** The directory exists and
    /// holds another brand's mark, so this is "no file for THIS brand" and not
    /// "no directory".
    #[test]
    fn a_brand_with_no_file_draws_its_word_while_its_neighbour_draws_a_logo() {
        let (ctx, _marks) = ctx_with_marks(true, &[(CardBrand::Visa, isolated_png(96, 48, 24))]);
        let (words, images) = painted(&ctx, CardBrand::Mastercard, MARK_ROW_HEIGHT);
        assert!(images.is_empty(), "a logo was drawn for a brand with no file");
        assert_eq!(words, vec![CardBrand::Mastercard.wordmark().to_string()]);
        // ...and the neighbour, from the same directory and the same frame
        // policy, still gets its image: this is a per-brand miss and not the
        // whole feature quietly switching itself off.
        let (_, images) = painted(&ctx, CardBrand::Visa, MARK_ROW_HEIGHT);
        assert_eq!(images.len(), 1, "the brand that HAS a file lost its logo too");
    }

    /// **Failure mode three: a file that is present and refused.** Three
    /// refusals, each independently: not a PNG, over the byte bound, and a
    /// header declaring more pixels than this app will decode.
    #[test]
    fn a_refused_file_draws_the_word_whichever_way_it_was_refused() {
        let mut oversized_header = Vec::new();
        {
            let mut encoder = png::Encoder::new(
                &mut oversized_header,
                crate::brand_mark::MAX_MARK_EDGE_PX + 1,
                8,
            );
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().expect("a header");
        }
        let refusals: [(&str, Vec<u8>); 3] = [
            ("not a PNG at all", b"just some bytes".to_vec()),
            (
                "over the byte bound",
                vec![0u8; crate::brand_mark::MAX_MARK_BYTES as usize + 1],
            ),
            ("more pixels than we will decode", oversized_header),
        ];
        for (why, bytes) in refusals {
            let (ctx, _marks) = ctx_with_marks(true, &[(CardBrand::Jcb, bytes)]);
            let (words, images) = painted(&ctx, CardBrand::Jcb, MARK_ROW_HEIGHT);
            assert!(images.is_empty(), "a mark {why} was drawn anyway");
            assert_eq!(
                words,
                vec![CardBrand::Jcb.wordmark().to_string()],
                "a mark {why} left the row with no mark at all rather than its word"
            );
        }
    }

    /// **The guarantee that lets a user mix assets from two brand centres.**
    /// An isolated mark on transparency and a reversed mark full-bleed on its
    /// own colour are the two shapes that turn up, they are shaped nothing
    /// like each other, and they must come out looking like one set: the same
    /// ink height, on the same optical line.
    #[test]
    fn an_isolated_mark_and_a_full_bleed_one_draw_at_one_ink_size_on_one_line() {
        // Deliberately different in every dimension a file can differ in:
        // different pixel sizes, different aspect ratios, and lettering that
        // is 100% of one image and 40% of the other.
        let (ctx, _marks) = ctx_with_marks(
            true,
            &[
                (CardBrand::Visa, isolated_png(120, 60, 30)),
                (CardBrand::Mastercard, on_ground_png(300, 100, 40)),
            ],
        );
        let mut ink = Vec::new();
        for brand in [CardBrand::Visa, CardBrand::Mastercard] {
            let (_, images) = painted(&ctx, brand, MARK_ROW_HEIGHT);
            assert_eq!(images.len(), 1, "{brand:?} did not draw as a logo");
            let logo = logo_of(&ctx, brand);
            // What the reader sees of each: the ink band, mapped out of the
            // fit. For the trimmed isolated mark the ink is the whole image;
            // for the grounded one it is the lettering inside its clear space.
            let (fitted, _) = with_ui_on(&ctx, |ui| {
                let word = word_galley(ui, brand, MARK_ROW_HEIGHT);
                logo_fit(&logo, MARK_ROW_HEIGHT, &word)
            });
            let scale = fitted.height() / logo.size.y;
            ink.push((
                (logo.ink_bottom - logo.ink_top) * scale,
                fitted.top() + logo.ink_center() * scale,
            ));
            // ...and the fit is what was really painted, to within the
            // rounding `egui` does when it snaps an image onto the pixel grid
            // (one point is one pixel in this headless context; on a real
            // display at 125-200% it is a fraction of a point).
            let drawn = images[0].translate(-egui::vec2(20.0, 20.0));
            assert!(
                (drawn.top() - fitted.top()).abs() <= 1.0
                    && (drawn.height() - fitted.height()).abs() <= 1.0,
                "{brand:?} was fitted to {fitted:?} and painted at {drawn:?}"
            );
        }
        let (isolated, grounded) = (ink[0], ink[1]);
        assert!(
            (isolated.0 - grounded.0).abs() < 0.2,
            "one mark's lettering is {}pt tall and the other's is {}pt -- the full-bleed asset's \
             clear space was drawn as though it were part of the mark",
            isolated.0,
            grounded.0
        );
        assert!(
            (isolated.1 - grounded.1).abs() < 0.01,
            "the two marks' ink centres are at {} and {} -- they sit on different lines",
            isolated.1,
            grounded.1
        );
        // The positive control: the two rects really are different, so the
        // agreement above is normalisation and not two identical inputs.
        assert!(isolated.0 > 1.0, "the ink measured as nothing");
    }

    /// **A logo lands on the WORD's optical line**, which is what lets
    /// `item_list` keep computing the row's alignment off the wordmark's
    /// galley with a logo in the box.
    #[test]
    fn a_logo_takes_the_line_and_the_box_height_the_word_would_have_had() {
        let (ctx, _marks) = ctx_with_marks(true, &[(CardBrand::Visa, on_ground_png(240, 80, 32))]);
        let (word_ink, height) = with_ui_on(&ctx, |ui| {
            let word = word_galley(ui, CardBrand::Visa, MARK_ROW_HEIGHT);
            let (top, bottom) = theme::ink_band_y(&word).expect("the word has ink");
            let offset = (MARK_ROW_HEIGHT - word.size().y) / 2.0;
            (offset + (top + bottom) / 2.0, bottom - top)
        });
        let logo = logo_of(&ctx, CardBrand::Visa);
        let (_, images) = painted(&ctx, CardBrand::Visa, MARK_ROW_HEIGHT);
        assert_eq!(images.len(), 1, "the premise: this fixture draws as a logo");
        let (fitted, _) = with_ui_on(&ctx, |ui| {
            let word = word_galley(ui, CardBrand::Visa, MARK_ROW_HEIGHT);
            logo_fit(&logo, MARK_ROW_HEIGHT, &word)
        });
        let scale = fitted.height() / logo.size.y;
        let logo_ink_center = fitted.top() + logo.ink_center() * scale;
        assert!(
            (logo_ink_center - word_ink).abs() < 0.01,
            "the logo's ink is centred at {logo_ink_center} in its box and the word's at \
             {word_ink} -- so a row aligned off the word is not aligned to the logo"
        );
        let drawn_ink = (logo.ink_bottom - logo.ink_top) * scale;
        assert!(
            (drawn_ink - height).abs() < 0.2,
            "the logo's lettering stands {drawn_ink}pt against the word's {height}pt"
        );
    }

    /// **No logo may cost the item name more room than the widest word
    /// already does.** The row's truncation budget is allocated out of the
    /// name's column, and it was tuned against `MASTERCARD`; a file with an
    /// extravagant aspect ratio must be fitted rather than allowed to eat the
    /// name.
    #[test]
    fn a_wildly_wide_file_is_bounded_to_less_than_the_widest_wordmark() {
        let widest = with_ui(|ui| {
            CARD_BRANDS
                .iter()
                .map(|brand| mark_width(ui, *brand, MARK_ROW_HEIGHT))
                .fold(0.0f32, f32::max)
        });
        // Twenty to one, which no real mark is: the point is that the bound
        // holds without knowing what a user will drop in.
        let (ctx, _marks) = ctx_with_marks(true, &[(CardBrand::Discover, on_ground_png(1000, 50, 20))]);
        let drawn = with_ui_on(&ctx, |ui| mark_width(ui, CardBrand::Discover, MARK_ROW_HEIGHT));
        assert!(
            drawn <= widest,
            "a 20:1 file drew {drawn}pt wide, more than the {widest}pt the widest wordmark \
             takes -- so the name's truncation room now depends on which files the user has"
        );
        // ...and the width it reports is the width it paints, which is what
        // the row reserves.
        let (_, images) = painted(&ctx, CardBrand::Discover, MARK_ROW_HEIGHT);
        assert!(
            images[0].width() <= drawn,
            "the mark painted {}pt into a box it said was {drawn}pt",
            images[0].width()
        );
    }

    /// **A missing file is re-checked; a refused one is not.** The favicon
    /// cache once wrote an item off for the session and never looked again,
    /// and the two cases are cached differently here precisely so that
    /// dropping a file into the folder does not need a restart.
    #[test]
    fn a_missing_file_is_looked_for_again_and_a_refused_one_is_not() {
        let dir = tempdir("recheck");
        let watching = ctx();
        install_logo_policy(
            &watching,
            LogoPolicy { enabled: true, dirs: Arc::new(vec![dir.to_path_buf()]) },
        );
        let name = crate::brand_mark::file_name(CardBrand::Maestro).expect("a name");

        let (_, images) = painted(&watching, CardBrand::Maestro, MARK_ROW_HEIGHT);
        assert!(images.is_empty(), "the premise: nothing is there yet");
        std::fs::write(dir.join(&name), isolated_png(80, 40, 20)).expect("the file appears");
        // The miss is remembered for `ABSENT_RECHECK`, so the very next frame
        // still shows the word -- and after it, the file is found.
        std::thread::sleep(ABSENT_RECHECK + std::time::Duration::from_millis(50));
        let (_, images) = painted(&watching, CardBrand::Maestro, MARK_ROW_HEIGHT);
        assert_eq!(
            images.len(),
            1,
            "a file dropped into the folder was never picked up -- the miss was cached for the \
             session, which is the favicon cache's old bug"
        );

        // The other half: a refusal stands, so a file this app has already
        // read and rejected is not re-read on a timer forever.
        let refused = tempdir("recheck-refused");
        let second = ctx();
        install_logo_policy(&second, LogoPolicy { enabled: true, dirs: Arc::new(vec![refused.to_path_buf()]) });
        let name = crate::brand_mark::file_name(CardBrand::Jcb).expect("a name");
        std::fs::write(refused.join(&name), b"not a PNG").expect("write");
        let (_, images) = painted(&second, CardBrand::Jcb, MARK_ROW_HEIGHT);
        assert!(images.is_empty(), "the premise: it was refused");
        std::fs::write(refused.join(&name), isolated_png(80, 40, 20)).expect("replace");
        std::thread::sleep(ABSENT_RECHECK + std::time::Duration::from_millis(50));
        let (_, images) = painted(&second, CardBrand::Jcb, MARK_ROW_HEIGHT);
        assert!(
            images.is_empty(),
            "a refusal was retried -- which on a file that stays broken is a decode attempt \
             every two seconds for the life of the window"
        );
    }

    fn logo_of(ctx: &egui::Context, brand: CardBrand) -> Arc<Logo> {
        with_ui_on(ctx, |ui| logo(ui, brand)).expect("this brand has a logo")
    }

    fn with_ui_on<R>(ctx: &egui::Context, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let mut f = Some(f);
        let mut out = None;
        let _ = ctx.run_ui(raw(), |ui| {
            if let Some(f) = f.take() {
                out = Some(f(ui));
            }
        });
        out.expect("the closure ran")
    }

    type Grounds = Vec<(egui::Rect, egui::Color32)>;

    fn collect(shapes: &[egui::epaint::ClippedShape]) -> (Grounds, Vec<String>) {
        fn walk(shape: &egui::Shape, grounds: &mut Grounds, words: &mut Vec<String>) {
            match shape {
                egui::Shape::Rect(rect) => grounds.push((rect.rect, rect.fill)),
                egui::Shape::Text(text) => words.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, grounds, words);
                    }
                }
                _ => {}
            }
        }
        let (mut grounds, mut words) = (Grounds::new(), Vec::new());
        for clipped in shapes {
            walk(&clipped.shape, &mut grounds, &mut words);
        }
        (grounds, words)
    }
}

//! Card-network and bank marks (design 1e): the tables, the bundled marks,
//! and the one function that draws a mark into a box.
//!
//! **Where a mark comes from.** The design draws 19 Simple Icons from a CDN as
//! white glyphs on the brand's own colour, and falls back to the brand's
//! name in type when an image will not load. The glyphs are bundled here
//! ([`super::logo_paths`], CC0) and drawn offline; the text tile is the
//! fallback, and it is also what the six names Simple Icons no longer carries
//! (Mir, Maestro, UnionPay, Citi, ING, Santander) are always drawn as. A user's
//! own picture in the `brand-marks` folder still wins where the card face
//! looks for one (see [`crate::card_mark`]).
//!
//! **How a glyph is drawn.** Simple Icons paths are filled, concave and full
//! of holes, which egui's convex-only polygons cannot fill, so each glyph is
//! rasterised once per size by [`rasterise`] (non-zero winding, exact
//! horizontal coverage and four sub-scanlines) into a white-on-alpha texture
//! one texel to one device pixel, and the texture is tinted white when drawn.
//! The glyph is first cropped to its ink: the design's CSS `contain` on a
//! 24 x 24 viewBox would draw a wide wordmark a few points tall, and the
//! sizes the design gives for each slot are the sizes of the ink.
//!
//! Nothing here is stored in the vault: a mark is derived at render time from
//! a card's number or brand, and a bank from the domain the item already
//! carries.

use super::icon::{parse, Subpath};
use super::logo_paths as paths;
use crate::card_brand::{self, CardBrand};
use crate::theme::EXTRABOLD;
use eframe::egui::{
    self, epaint::RectShape, pos2, vec2, Align, Color32, ColorImage, CornerRadius, FontFamily, FontId, Painter, Pos2,
    Rect, TextureHandle, TextureOptions, Vec2,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A card network, in the design's order (spec D 4.1; first match wins in
/// [`Net::for_number`]'s own order, not this one). RuPay is not in the
/// design's table: it is Bitwarden's ninth brand and keeps a text tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Net {
    Amex,
    Diners,
    Jcb,
    Visa,
    Mir,
    Mastercard,
    Maestro,
    Discover,
    UnionPay,
    RuPay,
}

/// Every network, the design's nine then RuPay.
pub const NETS: [Net; 10] = [
    Net::Amex,
    Net::Diners,
    Net::Jcb,
    Net::Visa,
    Net::Mir,
    Net::Mastercard,
    Net::Maestro,
    Net::Discover,
    Net::UnionPay,
    Net::RuPay,
];

impl Net {
    pub fn name(self) -> &'static str {
        match self {
            Net::Amex => "American Express",
            Net::Diners => "Diners Club",
            Net::Jcb => "JCB",
            Net::Visa => "Visa",
            Net::Mir => "Mir",
            Net::Mastercard => "Mastercard",
            Net::Maestro => "Maestro",
            Net::Discover => "Discover",
            Net::UnionPay => "UnionPay",
            Net::RuPay => "RuPay",
        }
    }

    /// The text fallback in a small tile.
    pub fn short(self) -> &'static str {
        match self {
            Net::Amex => "AMEX",
            Net::Diners => "DC",
            Net::Jcb => "JCB",
            Net::Visa => "VISA",
            Net::Mir => "MIR",
            Net::Mastercard => "MC",
            Net::Maestro => "MAE",
            Net::Discover => "DISC",
            Net::UnionPay => "UP",
            Net::RuPay => "RUPAY",
        }
    }

    /// The text fallback on a card face.
    pub fn long(self) -> &'static str {
        match self {
            Net::Amex => "AMERICAN EXPRESS",
            Net::Diners => "Diners Club",
            Net::Jcb => "JCB",
            Net::Visa => "VISA",
            Net::Mir => "MIR",
            Net::Mastercard => "mastercard",
            Net::Maestro => "maestro",
            Net::Discover => "DISCOVER",
            Net::UnionPay => "UnionPay",
            Net::RuPay => "RUPAY",
        }
    }

    pub fn colour(self) -> Color32 {
        rgb(match self {
            Net::Amex => 0x2e77bc,
            Net::Diners => 0x0079be,
            Net::Jcb => 0x0b4ea2,
            Net::Visa => 0x1a1f71,
            Net::Mir => 0x0f754e,
            Net::Mastercard => 0x201e1d,
            Net::Maestro => 0x0a3a82,
            Net::Discover => 0xe65c1f,
            Net::UnionPay => 0xd7001f,
            Net::RuPay => 0x0f3c7b,
        })
    }

    /// Only Visa's wordmark is set italic.
    pub fn italic(self) -> bool {
        self == Net::Visa
    }

    /// The design's rule text, for the reference sheet.
    pub fn rule(self) -> &'static str {
        match self {
            Net::Amex => "Starts with 34 or 37",
            Net::Diners => "Starts with 30, 36 or 38",
            Net::Jcb => "Starts with 35",
            Net::Visa => "Starts with 4",
            Net::Mir => "Starts with 2200\u{2013}2204",
            Net::Mastercard => "Starts with 51\u{2013}55 or 22\u{2013}27",
            Net::Maestro => "Starts with 50, 56\u{2013}58, 63 or 67",
            Net::Discover => "Starts with 6011 or 65",
            Net::UnionPay => "Starts with 62",
            Net::RuPay => "Starts with 60, 81 or 82",
        }
    }

    fn path(self) -> Option<&'static str> {
        match self {
            Net::Amex => Some(paths::AMERICAN_EXPRESS),
            Net::Diners => Some(paths::DINERS_CLUB),
            Net::Jcb => Some(paths::JCB),
            Net::Visa => Some(paths::VISA),
            Net::Mastercard => Some(paths::MASTERCARD),
            Net::Discover => Some(paths::DISCOVER),
            Net::Mir | Net::Maestro | Net::UnionPay | Net::RuPay => None,
        }
    }

    /// The network of a stored Bitwarden brand. [`CardBrand::Other`] names no
    /// network.
    pub fn from_brand(brand: CardBrand) -> Option<Self> {
        Some(match brand {
            CardBrand::Visa => Net::Visa,
            CardBrand::Mastercard => Net::Mastercard,
            CardBrand::AmericanExpress => Net::Amex,
            CardBrand::Discover => Net::Discover,
            CardBrand::DinersClub => Net::Diners,
            CardBrand::Jcb => Net::Jcb,
            CardBrand::Maestro => Net::Maestro,
            CardBrand::UnionPay => Net::UnionPay,
            CardBrand::RuPay => Net::RuPay,
            CardBrand::Other => return None,
        })
    }

    /// The network a card number belongs to. Mir is tested first, as the
    /// design does, so 2200-2204 is Mir and 2221-2720 is Mastercard. Mir has
    /// no Bitwarden brand, so this is the one place it is known.
    pub fn for_number(number: &str) -> Option<Self> {
        if card_brand::is_mir_number(number) {
            return Some(Net::Mir);
        }
        card_brand::brand_for_number(number).and_then(Self::from_brand)
    }
}

/// A bank the design draws a mark for (spec D 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bank {
    Chase,
    Revolut,
    Hsbc,
    Barclays,
    Monzo,
    Wise,
    Citi,
    BankOfAmerica,
    Ing,
    Santander,
}

pub const BANKS: [Bank; 10] = [
    Bank::Chase,
    Bank::Revolut,
    Bank::Hsbc,
    Bank::Barclays,
    Bank::Monzo,
    Bank::Wise,
    Bank::Citi,
    Bank::BankOfAmerica,
    Bank::Ing,
    Bank::Santander,
];

impl Bank {
    pub fn name(self) -> &'static str {
        match self {
            Bank::Chase => "Chase",
            Bank::Revolut => "Revolut",
            Bank::Hsbc => "HSBC",
            Bank::Barclays => "Barclays",
            Bank::Monzo => "Monzo",
            Bank::Wise => "Wise",
            Bank::Citi => "Citi",
            Bank::BankOfAmerica => "Bank of America",
            Bank::Ing => "ING",
            Bank::Santander => "Santander",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Bank::Chase => "CHASE",
            Bank::Revolut => "R",
            Bank::Hsbc => "HSBC",
            Bank::Barclays => "B",
            Bank::Monzo => "M",
            Bank::Wise => "WISE",
            Bank::Citi => "citi",
            Bank::BankOfAmerica => "BofA",
            Bank::Ing => "ING",
            Bank::Santander => "S",
        }
    }

    pub fn colour(self) -> Color32 {
        rgb(match self {
            Bank::Chase => 0x117aca,
            Bank::Revolut => 0x191c1f,
            Bank::Hsbc => 0xdb0011,
            Bank::Barclays => 0x00aeef,
            Bank::Monzo => 0x14233c,
            Bank::Wise => 0x163300,
            Bank::Citi => 0x003b70,
            Bank::BankOfAmerica => 0x012169,
            Bank::Ing => 0xff6200,
            Bank::Santander => 0xec0000,
        })
    }

    fn path(self) -> Option<&'static str> {
        match self {
            Bank::Chase => Some(paths::CHASE),
            Bank::Revolut => Some(paths::REVOLUT),
            Bank::Hsbc => Some(paths::HSBC),
            Bank::Barclays => Some(paths::BARCLAYS),
            Bank::Monzo => Some(paths::MONZO),
            Bank::Wise => Some(paths::WISE),
            Bank::BankOfAmerica => Some(paths::BANK_OF_AMERICA),
            Bank::Citi | Bank::Ing | Bank::Santander => None,
        }
    }

    /// The registrable domains a bank is known by. A host matches when it is
    /// one of these or a subdomain of one.
    fn domains(self) -> &'static [&'static str] {
        match self {
            Bank::Chase => &["chase.com"],
            Bank::Revolut => &["revolut.com"],
            Bank::Hsbc => &["hsbc.com", "hsbc.co.uk", "hsbc.ca", "hsbc.com.hk"],
            Bank::Barclays => &["barclays.com", "barclays.co.uk", "barclaysus.com"],
            Bank::Monzo => &["monzo.com"],
            Bank::Wise => &["wise.com", "transferwise.com"],
            Bank::Citi => &["citi.com", "citibank.com", "citibank.co.uk"],
            Bank::BankOfAmerica => &["bankofamerica.com", "bofa.com"],
            Bank::Ing => &["ing.com", "ing.nl", "ing.de", "ing.be", "ing.com.au", "ing.pl"],
            Bank::Santander => &["santander.com", "santander.co.uk", "santanderbank.com", "santander.us"],
        }
    }

    /// The bank whose site `domain` (a host, or `host:port`) belongs to.
    pub fn for_domain(domain: &str) -> Option<Self> {
        let host = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        let host = host.split(':').next().unwrap_or("");
        if host.is_empty() {
            return None;
        }
        BANKS.into_iter().find(|bank| {
            bank.domains()
                .iter()
                .any(|d| host == *d || host.strip_suffix(d).is_some_and(|rest| rest.ends_with('.')))
        })
    }
}

/// A mark: a network or a bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    Net(Net),
    Bank(Bank),
}

impl Mark {
    pub fn colour(self) -> Color32 {
        match self {
            Mark::Net(n) => n.colour(),
            Mark::Bank(b) => b.colour(),
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Mark::Net(n) => n.short(),
            Mark::Bank(b) => b.short(),
        }
    }

    /// A bank's long label is its name, in normal style.
    pub fn long(self) -> &'static str {
        match self {
            Mark::Net(n) => n.long(),
            Mark::Bank(b) => b.name(),
        }
    }

    pub fn italic(self) -> bool {
        matches!(self, Mark::Net(n) if n.italic())
    }

    /// The bundled glyph's path data; `None` where the text tile is the mark.
    pub fn path(self) -> Option<&'static str> {
        match self {
            Mark::Net(n) => n.path(),
            Mark::Bank(b) => b.path(),
        }
    }
}

/// Which of the two text fallbacks a slot sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Short,
    Long,
}

/// A glyph parsed once: its subpaths and the box its ink occupies.
struct Parsed {
    paths: Vec<Subpath>,
    min: Pos2,
    size: Vec2,
}

fn parsed(mark: Mark) -> Option<Arc<Parsed>> {
    static CACHE: OnceLock<Mutex<HashMap<Mark, Option<Arc<Parsed>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry(mark)
        .or_insert_with(|| {
            let paths = parse(mark.path()?).ok()?;
            let (mut lo, mut hi) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
            for p in paths.iter().flat_map(|s| s.points.iter()) {
                lo = pos2(lo.x.min(p.x), lo.y.min(p.y));
                hi = pos2(hi.x.max(p.x), hi.y.max(p.y));
            }
            (hi.x > lo.x && hi.y > lo.y).then(|| Arc::new(Parsed { paths, min: lo, size: hi - lo }))
        })
        .clone()
}

/// The ink's width over its height, for a mark that has a glyph.
pub fn ink_aspect(mark: Mark) -> Option<f32> {
    parsed(mark).map(|p| p.size.x / p.size.y)
}

/// Fills `paths` (non-zero winding) into a `w` x `h` coverage mask that maps
/// `size` viewBox units starting at `min` onto the whole mask. Horizontal
/// coverage is exact; vertical is sampled four times per row.
pub fn rasterise(paths: &[Subpath], min: Pos2, size: Vec2, w: usize, h: usize) -> Vec<u8> {
    const SS: usize = 4;
    let (sx, sy) = (w as f32 / size.x, h as f32 / size.y);
    let mut edges: Vec<(Pos2, Pos2)> = Vec::new();
    for path in paths {
        let pts: Vec<Pos2> = path.points.iter().map(|p| pos2((p.x - min.x) * sx, (p.y - min.y) * sy)).collect();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if a.y != b.y {
                edges.push((a, b));
            }
        }
    }
    let mut cover = vec![0.0f32; w * h];
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        for s in 0..SS {
            let y = row as f32 + (s as f32 + 0.5) / SS as f32;
            crossings.clear();
            for (a, b) in &edges {
                if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
                    let t = (y - a.y) / (b.y - a.y);
                    crossings.push((a.x + t * (b.x - a.x), if b.y > a.y { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|l, r| l.0.total_cmp(&r.0));
            let (mut winding, mut start) = (0, 0.0f32);
            for &(x, dir) in &crossings {
                let before = winding;
                winding += dir;
                if before == 0 && winding != 0 {
                    start = x;
                } else if before != 0 && winding == 0 {
                    let (x0, x1) = (start.clamp(0.0, w as f32), x.clamp(0.0, w as f32));
                    let mut px = x0.floor() as usize;
                    while (px as f32) < x1 && px < w {
                        let overlap = x1.min(px as f32 + 1.0) - x0.max(px as f32);
                        cover[row * w + px] += overlap.max(0.0) / SS as f32;
                        px += 1;
                    }
                }
            }
        }
    }
    cover.into_iter().map(|c| (c.min(1.0) * 255.0).round() as u8).collect()
}

type Textures = Arc<Mutex<HashMap<(Mark, usize, usize), TextureHandle>>>;

/// The glyph's mask texture at exactly `w` x `h` device pixels, kept per
/// context. A handful of sizes at most are ever live (list, detail, chip,
/// face, editor); the map is emptied if something drives it past 64.
fn texture(ctx: &egui::Context, mark: Mark, glyph: &Parsed, w: usize, h: usize) -> TextureHandle {
    let id = egui::Id::new("deskwarden-logo-textures");
    let cache: Textures = ctx.data_mut(|d| d.get_temp_mut_or_insert_with::<Textures>(id, Default::default).clone());
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 64 {
        cache.clear();
    }
    cache
        .entry((mark, w, h))
        .or_insert_with(|| {
            let alpha = rasterise(&glyph.paths, glyph.min, glyph.size, w, h);
            let pixels = alpha.into_iter().map(|a| Color32::from_white_alpha(a)).collect();
            ctx.load_texture(
                format!("logo-{mark:?}-{w}x{h}"),
                ColorImage { size: [w, h], pixels, source_size: vec2(w as f32, h as f32) },
                TextureOptions::LINEAR,
            )
        })
        .clone()
}

/// Draws `mark` in white, centred on `centre`: the bundled glyph contained in
/// `image` (the design's image box for the slot), or, when the mark has no
/// glyph, its text label at `text_px`. Returns whether a glyph was drawn.
pub fn paint(painter: &Painter, centre: Pos2, image: Vec2, text_px: f32, mark: Mark, label: Label) -> bool {
    paint_aligned(painter, centre, Align::Center, image, text_px, mark, label)
}

/// [`paint`], with `at` the mark's left edge, centre or right edge by `align`
/// (on the vertical middle line): what a card face needs, whose bank sits at
/// the left and whose network at the right whatever width the mark came out.
pub fn paint_aligned(
    painter: &Painter,
    at: Pos2,
    align: Align,
    image: Vec2,
    text_px: f32,
    mark: Mark,
    label: Label,
) -> bool {
    let place = |width: f32| match align {
        Align::Min => at.x + width / 2.0,
        Align::Center => at.x,
        Align::Max => at.x - width / 2.0,
    };
    if let Some(glyph) = parsed(mark) {
        let ppp = painter.ctx().pixels_per_point().max(0.5);
        let scale = (image.x / glyph.size.x).min(image.y / glyph.size.y);
        let (w, h) = (
            ((glyph.size.x * scale * ppp).round() as usize).max(1),
            ((glyph.size.y * scale * ppp).round() as usize).max(1),
        );
        let size = vec2(w as f32 / ppp, h as f32 / ppp);
        let rect = Rect::from_center_size(pos2(place(size.x), at.y), size);
        let tex = texture(painter.ctx(), mark, &glyph, w, h);
        painter.add(
            RectShape::filled(rect, CornerRadius::ZERO, Color32::WHITE)
                .with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
        );
        return true;
    }
    let text = match label {
        Label::Short => mark.short(),
        Label::Long => mark.long(),
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: FontId::new(text_px, FontFamily::Name(EXTRABOLD.into())),
            color: Color32::WHITE,
            italics: mark.italic(),
            extra_letter_spacing: text_px * 0.02,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let size = galley.size();
    painter.galley(pos2(place(size.x) - size.x / 2.0, at.y - size.y / 2.0), galley, Color32::WHITE);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, Painted};

    const ALL_PATHS: [(&str, &str); 13] = [
        ("amex", paths::AMERICAN_EXPRESS),
        ("diners", paths::DINERS_CLUB),
        ("jcb", paths::JCB),
        ("visa", paths::VISA),
        ("mastercard", paths::MASTERCARD),
        ("discover", paths::DISCOVER),
        ("chase", paths::CHASE),
        ("revolut", paths::REVOLUT),
        ("hsbc", paths::HSBC),
        ("barclays", paths::BARCLAYS),
        ("monzo", paths::MONZO),
        ("wise", paths::WISE),
        ("bofa", paths::BANK_OF_AMERICA),
    ];

    #[test]
    fn every_bundled_path_parses_and_stays_inside_its_viewbox() {
        for (name, d) in ALL_PATHS {
            let subpaths = parse(d).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(!subpaths.is_empty(), "{name}");
            for p in subpaths.iter().flat_map(|s| s.points.iter()) {
                assert!((-0.01..=24.01).contains(&p.x) && (-0.01..=24.01).contains(&p.y), "{name}: {p:?}");
            }
        }
    }

    #[test]
    fn the_marks_with_glyphs_are_exactly_the_thirteen() {
        let nets = NETS.iter().filter(|n| n.path().is_some()).count();
        let banks = BANKS.iter().filter(|b| b.path().is_some()).count();
        assert_eq!((nets, banks), (6, 7));
        for mark in NETS.map(Mark::Net).into_iter().chain(BANKS.map(Mark::Bank)) {
            assert_eq!(mark.path().is_some(), parsed(mark).is_some(), "{mark:?}");
        }
    }

    #[test]
    fn the_tables_carry_the_designs_colours_and_labels() {
        assert_eq!(Net::Visa.colour(), rgb(0x1a1f71));
        assert_eq!((Net::Visa.short(), Net::Visa.long(), Net::Visa.italic()), ("VISA", "VISA", true));
        assert_eq!(Net::Mastercard.long(), "mastercard");
        assert_eq!(Net::Amex.long(), "AMERICAN EXPRESS");
        assert_eq!(Net::Mir.colour(), rgb(0x0f754e));
        assert_eq!(Net::Mir.rule(), "Starts with 2200\u{2013}2204");
        assert_eq!(NETS.iter().filter(|n| n.italic()).count(), 1);
        assert_eq!(Bank::BankOfAmerica.short(), "BofA");
        assert_eq!(Bank::BankOfAmerica.colour(), rgb(0x012169));
        assert_eq!(Mark::Bank(Bank::BankOfAmerica).long(), "Bank of America");
        assert_eq!(Bank::Citi.short(), "citi");
        let mut shorts: Vec<_> = NETS.iter().map(|n| n.short()).collect();
        shorts.sort_unstable();
        shorts.dedup();
        assert_eq!(shorts.len(), NETS.len());
    }

    #[test]
    fn a_number_names_its_network_and_mir_beats_mastercard() {
        assert_eq!(Net::for_number("4111 1111 1111 1111"), Some(Net::Visa));
        assert_eq!(Net::for_number("5412 7512 3412 3456"), Some(Net::Mastercard));
        assert_eq!(Net::for_number("2200 1234"), Some(Net::Mir));
        assert_eq!(Net::for_number("2204"), Some(Net::Mir));
        assert_eq!(Net::for_number("2221 0000"), Some(Net::Mastercard));
        assert_eq!(Net::for_number("2205 0000"), None);
        assert_eq!(Net::for_number("378282246310005"), Some(Net::Amex));
        assert_eq!(Net::for_number(""), None);
        assert_eq!(Net::from_brand(CardBrand::Other), None);
        assert_eq!(Net::from_brand(CardBrand::RuPay), Some(Net::RuPay));
    }

    #[test]
    fn a_bank_is_found_from_a_host_or_its_subdomain_and_nothing_longer() {
        assert_eq!(Bank::for_domain("chase.com"), Some(Bank::Chase));
        assert_eq!(Bank::for_domain("  www.Chase.com "), Some(Bank::Chase));
        assert_eq!(Bank::for_domain("secure.bankofamerica.com:443"), Some(Bank::BankOfAmerica));
        assert_eq!(Bank::for_domain("online.barclays.co.uk"), Some(Bank::Barclays));
        assert_eq!(Bank::for_domain("notchase.com"), None);
        assert_eq!(Bank::for_domain("chase.com.evil.example"), None);
        assert_eq!(Bank::for_domain(""), None);
        assert_eq!(Bank::for_domain("brex.com"), None);
    }

    #[test]
    fn the_rasteriser_fills_a_square_and_leaves_its_hole_clear() {
        let outer = Subpath { points: vec![pos2(0.0, 0.0), pos2(8.0, 0.0), pos2(8.0, 8.0), pos2(0.0, 8.0)], closed: true };
        // Opposite winding, as Simple Icons draws holes.
        let hole = Subpath { points: vec![pos2(2.0, 2.0), pos2(2.0, 6.0), pos2(6.0, 6.0), pos2(6.0, 2.0)], closed: true };
        let mask = rasterise(&[outer, hole], pos2(0.0, 0.0), vec2(8.0, 8.0), 16, 16);
        assert_eq!(mask[16], 255, "the rim");
        assert_eq!(mask[8 * 16 + 8], 0, "the hole");
        // Half-covered column: a 1-unit-wide bar ending mid-pixel.
        let bar = Subpath { points: vec![pos2(0.0, 0.0), pos2(1.5, 0.0), pos2(1.5, 2.0), pos2(0.0, 2.0)], closed: true };
        let mask = rasterise(&[bar], pos2(0.0, 0.0), vec2(2.0, 2.0), 2, 2);
        assert_eq!((mask[0], mask[1]), (255, 128));
    }

    #[test]
    fn a_real_glyph_has_ink_and_holes_over_the_whole_mask() {
        let glyph = parsed(Mark::Bank(Bank::Chase)).expect("chase");
        let mask = rasterise(&glyph.paths, glyph.min, glyph.size, 64, 64);
        let inked = mask.iter().filter(|a| **a > 128).count();
        assert!(inked > 64 * 64 / 4 && inked < 64 * 64, "{inked}");
    }

    fn painted(mark: Mark, label: Label) -> (Painted, bool) {
        let ctx = testing::ctx();
        let mut drawn = false;
        let p = testing::frame(&ctx, 0.0, vec2(200.0, 200.0), vec![], |ui| {
            drawn = paint(ui.painter(), pos2(100.0, 100.0), vec2(36.0, 14.0), 13.0, mark, label);
        });
        (p, drawn)
    }

    #[test]
    fn a_glyph_is_drawn_as_a_texture_inside_its_box_and_no_text() {
        let (p, drawn) = painted(Mark::Net(Net::Visa), Label::Short);
        assert!(drawn);
        let tex = p
            .rects
            .iter()
            .find(|r| r.fill == Color32::WHITE && r.rect.width() > 1.0 && r.rect.center().distance(pos2(100.0, 100.0)) < 1.0)
            .expect("the glyph");
        assert!(tex.rect.width() <= 36.0 + 0.01 && tex.rect.height() <= 14.0 + 0.01, "{:?}", tex.rect);
        assert!(tex.rect.width() > 20.0, "{:?}", tex.rect);
        assert!(p.text("VISA").is_none());
    }

    #[test]
    fn a_mark_is_anchored_by_its_left_or_right_edge_when_asked() {
        let ctx = testing::ctx();
        let p = testing::frame(&ctx, 0.0, vec2(300.0, 200.0), vec![], |ui| {
            paint_aligned(ui.painter(), pos2(50.0, 40.0), Align::Min, vec2(60.0, 24.0), 18.0, Mark::Bank(Bank::Chase), Label::Long);
            paint_aligned(ui.painter(), pos2(250.0, 80.0), Align::Max, vec2(60.0, 24.0), 18.0, Mark::Bank(Bank::Chase), Label::Long);
            paint_aligned(ui.painter(), pos2(50.0, 120.0), Align::Min, vec2(60.0, 24.0), 18.0, Mark::Bank(Bank::Santander), Label::Long);
        });
        let glyphs: Vec<_> = p.rects.iter().filter(|r| r.fill == Color32::WHITE && r.rect.width() > 1.0).collect();
        assert!((glyphs[0].rect.min.x - 50.0).abs() < 1.0, "{:?}", glyphs[0].rect);
        assert!((glyphs[1].rect.max.x - 250.0).abs() < 1.0, "{:?}", glyphs[1].rect);
        let (_, _, text) = p.text("Santander").expect("the text");
        assert!((text.min.x - 50.0).abs() < 3.0, "{text:?}");
    }

    #[test]
    fn a_mark_without_a_glyph_is_set_in_type() {
        let (p, drawn) = painted(Mark::Net(Net::Mir), Label::Short);
        assert!(!drawn);
        assert_eq!(p.text("MIR").expect("the label").1, Color32::WHITE);
        let (p, _) = painted(Mark::Bank(Bank::Santander), Label::Long);
        assert!(p.text("Santander").is_some());
        let (p, _) = painted(Mark::Net(Net::Maestro), Label::Long);
        assert!(p.text("maestro").is_some());
    }
}

//! Item avatars in the v2 design (spec part A section 3.2): a brand tile for
//! a card, a favicon on white, or initials on a colour.

use crate::card_brand::CardBrand;
use crate::theme::{BOLD, EXTRABOLD};
use eframe::egui::{
    self, epaint::RectShape, pos2, Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Rect, Stroke,
    StrokeKind, TextureHandle, Vec2,
};

/// What an avatar tile shows.
#[derive(Clone)]
pub enum Look {
    /// Up to two letters on one of [`AVATAR_PALETTE`]'s pairs.
    Initials { text: String, bg: Color32, fg: Color32 },
    /// A site's favicon, on white.
    Favicon(TextureHandle),
    /// A card network's tile: its short label in white on its colour.
    Brand { label: String, bg: Color32, italic: bool },
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The design's avatar palette (spec 1.1), background then ink.
pub const AVATAR_PALETTE: [(Color32, Color32); 9] = [
    (rgb(0x1b3fa0), Color32::WHITE),
    (rgb(0xdbe4f7), rgb(0x14307a)),
    (rgb(0x201e1d), Color32::WHITE),
    (rgb(0x7fa4ef), rgb(0x14307a)),
    (rgb(0xeceae7), rgb(0x201e1d)),
    (rgb(0x3b74e8), Color32::WHITE),
    (rgb(0xe8f3ec), rgb(0x17673a)),
    (rgb(0x14307a), Color32::WHITE),
    (rgb(0xfef6e7), rgb(0x8a5a06)),
];

/// The favicon tile's inset ring.
const FAVICON_RING: Color32 = rgb(0xe6e3df);

/// The list's tile is 44 and the detail's 72; everything inside scales
/// between the two sizes the design draws.
fn at(height: f32, list: f32, detail: f32) -> f32 {
    if height >= 60.0 {
        detail * height / 72.0
    } else {
        list * height / 44.0
    }
}

/// Paints `look` as a tile filling `rect` at corner `radius`.
pub fn paint(painter: &Painter, rect: Rect, look: &Look, radius: u8) {
    let corner = CornerRadius::same(radius);
    let h = rect.height();
    match look {
        Look::Initials { text, bg, fg } => {
            painter.add(RectShape::filled(rect, corner, *bg));
            let (size, face) = if h >= 60.0 { (at(h, 15.0, 24.0), EXTRABOLD) } else { (at(h, 15.0, 24.0), BOLD) };
            painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::new(size, FontFamily::Name(face.into())), *fg);
        }
        Look::Favicon(texture) => {
            painter.add(RectShape::new(rect, corner, Color32::WHITE, Stroke::new(1.0, FAVICON_RING), StrokeKind::Inside));
            let side = at(h, 22.0, 34.0);
            let image = Rect::from_center_size(rect.center(), Vec2::splat(side));
            // A textured rect rather than `Painter::image`'s mesh: the same
            // pixels, and a shape the window's tests can recognise as a picture.
            painter.add(
                RectShape::filled(image, CornerRadius::ZERO, Color32::WHITE)
                    .with_texture(texture.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
            );
        }
        Look::Brand { label, bg, italic } => {
            painter.add(RectShape::filled(rect, corner, *bg));
            let size = at(h, 13.0, 19.0);
            let mut job = egui::text::LayoutJob::default();
            job.append(
                label,
                0.0,
                egui::TextFormat {
                    font_id: FontId::new(size, FontFamily::Name(EXTRABOLD.into())),
                    color: Color32::WHITE,
                    italics: *italic,
                    extra_letter_spacing: size * 0.02,
                    ..Default::default()
                },
            );
            let galley = painter.layout_job(job);
            let at = rect.center() - galley.size() / 2.0;
            painter.galley(at, galley, Color32::WHITE);
        }
    }
}

/// The palette pair for an item named `name`: an FNV-1a hash of its bytes,
/// so the same name always gets the same colours.
pub fn initials_colours(name: &str) -> (Color32, Color32) {
    let mut hash: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    AVATAR_PALETTE[hash as usize % AVATAR_PALETTE.len()]
}

/// A card network's tile (spec 3.2): short label, colour, and whether the
/// label is set italic. [`CardBrand::Other`] names no network and has none.
pub fn network(brand: CardBrand) -> Option<(&'static str, Color32, bool)> {
    Some(match brand {
        CardBrand::Visa => ("VISA", rgb(0x1a1f71), true),
        CardBrand::Mastercard => ("MC", rgb(0x201e1d), false),
        CardBrand::AmericanExpress => ("AMEX", rgb(0x2e77bc), false),
        CardBrand::Discover => ("DISC", rgb(0xe65c1f), false),
        CardBrand::DinersClub => ("DC", rgb(0x0079be), false),
        CardBrand::Jcb => ("JCB", rgb(0x0b4ea2), false),
        CardBrand::Maestro => ("MAE", rgb(0x0a3a82), false),
        CardBrand::UnionPay => ("UP", rgb(0xd7001f), false),
        // Not in the design's table: RuPay's own navy, in the same style.
        CardBrand::RuPay => ("RUPAY", rgb(0x0f3c7b), false),
        CardBrand::Other => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_brand::CardBrand;
    use crate::kit::testing::{self, Painted};
    use eframe::egui::{self, pos2, vec2, Color32, Rect};


    fn rgb(hex: u32) -> Color32 {
        Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    fn painted(look: &Look, size: f32) -> (Painted, Rect) {
        let ctx = testing::ctx();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(size, size));
        let p = testing::frame(&ctx, 0.0, vec2(200.0, 200.0), vec![], |ui| paint(ui.painter(), rect, look, 12));
        (p, rect)
    }

    #[test]
    fn initials_tiles_paint_their_colours_and_text() {
        let look = Look::Initials { text: "SP".into(), bg: rgb(0x1b3fa0), fg: Color32::WHITE };
        let (p, rect) = painted(&look, 44.0);
        assert!(p.rects.iter().any(|r| r.rect == rect && r.fill == rgb(0x1b3fa0)));
        let (_, ink, at) = p.text("SP").expect("the initials");
        assert_eq!(*ink, Color32::WHITE);
        assert!(rect.contains(at.center()));
    }

    #[test]
    fn a_brand_tile_is_the_networks_colour_with_its_label() {
        let (label, bg, italic) = network(CardBrand::Visa).expect("visa has a tile");
        assert_eq!((label, bg, italic), ("VISA", rgb(0x1a1f71), true));
        assert_eq!(network(CardBrand::AmericanExpress).unwrap().1, rgb(0x2e77bc));
        assert!(network(CardBrand::Other).is_none());
        let look = Look::Brand { label: label.into(), bg, italic };
        let (p, rect) = painted(&look, 44.0);
        assert!(p.rects.iter().any(|r| r.rect == rect && r.fill == bg));
        assert_eq!(p.text("VISA").unwrap().1, Color32::WHITE);
    }

    #[test]
    fn initials_colours_are_stable_per_name_and_from_the_palette() {
        let a = initials_colours("SAP Production");
        assert_eq!(a, initials_colours("SAP Production"));
        assert!(AVATAR_PALETTE.contains(&a));
        let spread: std::collections::HashSet<_> =
            ["Ledgerline", "Atlas", "Git Host", "Vantage", "Postgres", "Office", "Chase", "Monzo"]
                .iter()
                .map(|n| initials_colours(n))
                .collect();
        assert!(spread.len() >= 3, "every name lands on the same colour: {spread:?}");
    }

    #[test]
    fn a_favicon_tile_is_white_with_the_ring() {
        let ctx = testing::ctx();
        let texture = ctx.load_texture("t", egui::ColorImage::new([4, 4], vec![Color32::RED; 16]), Default::default());
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(44.0, 44.0));
        let p = testing::frame(&ctx, 0.0, vec2(200.0, 200.0), vec![], |ui| {
            paint(ui.painter(), rect, &Look::Favicon(texture.clone()), 12)
        });
        let tile = p.rects.iter().find(|r| r.rect == rect).expect("the tile");
        assert_eq!(tile.fill, Color32::WHITE);
        assert_eq!(tile.stroke.color, rgb(0xe6e3df));
    }
}

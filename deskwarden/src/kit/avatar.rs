//! Item avatars in the v2 design (spec part A section 3.2): a brand tile for
//! a card, a favicon on white, or initials on a colour.

use super::logos::{self, Label, Mark, Net};
use crate::card_brand::CardBrand;
use crate::theme::{BOLD, EXTRABOLD};
use eframe::egui::{
    epaint::RectShape, pos2, Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Rect, Stroke,
    StrokeKind, TextureHandle, Vec2, vec2,
};

/// What an avatar tile shows.
#[derive(Clone)]
pub enum Look {
    /// Up to two letters on one of [`AVATAR_PALETTE`]'s pairs.
    Initials { text: String, bg: Color32, fg: Color32 },
    /// A site's favicon, on white.
    Favicon(TextureHandle),
    /// A card's bank, drawn from its own favicon because the bank has no
    /// bundled mark: the icon on white with the card's network as the same
    /// small chip a bundled bank tile wears.
    BankIcon { texture: TextureHandle, chip: Option<Net> },
    /// A network's or bank's tile (design 1e): its mark in white on its colour,
    /// or its short label when it has no bundled mark. A bank's tile carries
    /// the card's network as a small chip over its bottom-right corner.
    Brand { mark: Mark, chip: Option<Net> },
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
        Look::BankIcon { texture, chip } => {
            paint(painter, rect, &Look::Favicon(texture.clone()), radius);
            if let Some(net) = chip {
                paint_chip(painter, rect, *net);
            }
        }
        Look::Brand { mark, chip } => {
            painter.add(RectShape::filled(rect, corner, mark.colour()));
            // The design's slots (spec D 4.6): the list tile's mark box is
            // 36 x 14 and the detail's 57 x 22, never wider than 86% of the tile.
            let image = vec2(at(h, 36.0, 57.0).min(h * 0.86), at(h, 14.0, 22.0));
            logos::paint(painter, rect.center(), image, at(h, 13.0, 19.0), *mark, Label::Short);
            if let Some(net) = chip {
                paint_chip(painter, rect, *net);
            }
        }
    }
}

/// The network chip over a bank tile's bottom-right corner: 30 x 20 with a
/// white 2 border in the list, 42 x 28 with a 3 border in the detail header.
fn paint_chip(painter: &Painter, tile: Rect, net: Net) {
    let h = tile.height();
    let detail = h >= 60.0;
    let (size, radius, border, ring, out, mark_box, text) = if detail {
        (vec2(42.0, 28.0), 8u8, 3.0, rgb(0xf5f4f2), vec2(9.0, 7.0), vec2(34.0, 13.0), 9.0)
    } else {
        (vec2(30.0, 20.0), 6u8, 2.0, Color32::WHITE, vec2(7.0, 6.0), vec2(26.0, 10.0), 7.0)
    };
    let chip = Rect::from_min_size(tile.right_bottom() + out - size, size);
    painter.add(RectShape::filled(chip, CornerRadius::same(radius), ring));
    let inner = chip.shrink(border);
    painter.add(RectShape::filled(inner, CornerRadius::same(radius.saturating_sub(border as u8)), net.colour()));
    logos::paint(painter, inner.center(), mark_box.min(inner.size()), text, Mark::Net(net), Label::Short);
}

/// A square tile for a named thing with an optional picture: the picture on
/// white when there is one, else the name's initials on its palette pair, at
/// `size` with the corner radius the design's tiles keep at that size. A
/// `marked` tile (the picker's selected row, a card on the sequence band)
/// wears a one-point blue edge. Allocates the square and answers it.
pub fn tile(ui: &mut eframe::egui::Ui, size: f32, name: &str, icon: Option<&TextureHandle>, marked: bool) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), eframe::egui::Sense::hover());
    let look = match icon {
        Some(texture) => Look::Favicon(texture.clone()),
        None => {
            let (bg, fg) = initials_colours(name);
            Look::Initials { text: crate::theme::initials(name), bg, fg }
        }
    };
    let radius = (size / 4.0).round() as u8;
    paint(ui.painter(), rect, &look, radius);
    if marked {
        let edge = crate::kit::tokens::palette(ui.ctx()).blue_edge;
        ui.painter().rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.0, edge), StrokeKind::Inside);
    }
    rect
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
    Net::from_brand(brand).map(|n| (n.short(), n.colour(), n.italic()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_brand::CardBrand;
    use crate::kit::logos::{Bank, Mark, Net};
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
        // Mir has no bundled glyph, so its tile is its label.
        let look = Look::Brand { mark: Mark::Net(Net::Mir), chip: None };
        let (p, rect) = painted(&look, 44.0);
        assert!(p.rects.iter().any(|r| r.rect == rect && r.fill == rgb(0x0f754e)));
        assert_eq!(p.text("MIR").unwrap().1, Color32::WHITE);
    }

    #[test]
    fn a_network_with_a_glyph_draws_it_inside_the_tile_and_no_label() {
        let look = Look::Brand { mark: Mark::Net(Net::Visa), chip: None };
        let (p, rect) = painted(&look, 44.0);
        assert!(p.rects.iter().any(|r| r.rect == rect && r.fill == rgb(0x1a1f71)));
        assert!(p.text("VISA").is_none());
        let glyph = p.rects.iter().find(|r| r.rect != rect && rect.contains_rect(r.rect)).expect("the glyph");
        assert!(glyph.rect.width() <= 36.0 + 0.01 && glyph.rect.height() <= 14.0 + 0.01);
    }

    #[test]
    fn a_bank_tile_carries_the_network_chip_over_its_corner_and_a_lone_network_does_not() {
        let with = Look::Brand { mark: Mark::Bank(Bank::Chase), chip: Some(Net::Visa) };
        let (p, rect) = painted(&with, 44.0);
        assert!(p.rects.iter().any(|r| r.rect == rect && r.fill == rgb(0x117aca)));
        let ring = p.rects.iter().find(|r| r.rect.size() == vec2(30.0, 20.0)).expect("the chip");
        assert_eq!(ring.fill, Color32::WHITE);
        assert_eq!(ring.rect.max, rect.right_bottom() + vec2(7.0, 6.0));
        assert!(p.rects.iter().any(|r| r.rect == ring.rect.shrink(2.0) && r.fill == rgb(0x1a1f71)));
        let without = Look::Brand { mark: Mark::Bank(Bank::Chase), chip: None };
        let (p, _) = painted(&without, 44.0);
        assert!(p.rects.iter().all(|r| r.rect.size() != vec2(30.0, 20.0)));
    }

    #[test]
    fn the_detail_tile_draws_the_larger_chip_on_the_panes_ground() {
        let look = Look::Brand { mark: Mark::Bank(Bank::Revolut), chip: Some(Net::Mastercard) };
        let (p, rect) = painted(&look, 72.0);
        let ring = p.rects.iter().find(|r| r.rect.size() == vec2(42.0, 28.0)).expect("the chip");
        assert_eq!(ring.fill, rgb(0xf5f4f2));
        assert_eq!(ring.rect.max, rect.right_bottom() + vec2(9.0, 7.0));
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

    #[test]
    fn a_named_tile_is_a_square_of_initials_and_a_marked_one_wears_a_blue_edge() {
        let ctx = testing::ctx();
        let mut plain = Rect::NOTHING;
        let painted = testing::frame(&ctx, 0.0, vec2(200.0, 200.0), vec![], |ui| {
            plain = tile(ui, 28.0, "Chrome", None, false);
        });
        assert_eq!(plain.size(), vec2(28.0, 28.0));
        assert!(painted.text("CH").is_some() || painted.text("C").is_some(), "the initials are drawn");
        let (bg, _) = initials_colours("Chrome");
        assert!(painted.rects.iter().any(|r| r.fill == bg && r.rect == plain));
        let marked = testing::frame(&ctx, 0.1, vec2(200.0, 200.0), vec![], |ui| {
            tile(ui, 28.0, "Chrome", None, true);
        });
        let edge = crate::kit::tokens::Palette::LIGHT.blue_edge;
        assert!(marked.rects.iter().any(|r| r.stroke.color == edge && r.stroke.width == 1.0));
        assert!(painted.rects.iter().any(|r| r.rect == plain && r.corner_radius == CornerRadius::same(7)), "28 rounds at 7");
    }
}

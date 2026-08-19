//! The network mark: a card's network, SET IN TYPE, on a small ground.
//!
//! **Words, not logos, and this is a decision rather than a shortcut.** The
//! card networks' marks are registered trademarks whose brand guidelines
//! restrict their use to licensed issuers and merchants. This is an
//! MIT-licensed community project and its author does not want that exposure.
//! Naming which network a card belongs to is a statement of fact about the
//! user's own card, so the mark states it: `VISA`, `MC`, `AMEX`. The words
//! come from [`CardBrand::wordmark`], which is the one place brands are named
//! -- there is no second table here to fall out of step with it.
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

use crate::card_brand::CardBrand;
use crate::theme;
use eframe::egui;

/// The mark's height on the detail pane's Number row.
///
/// The size the read pane draws the matched app's icon at, and deliberately
/// the same: both are "a small thing identifying the row, immediately before
/// the row's value".
pub const MARK_DETAIL_HEIGHT: f32 = 18.0;

/// The mark's height as the item list's corner badge.
///
/// **This is the number the whole approach lives or dies on**, because the
/// badge is drawn INSIDE the row's 32pt avatar tile (see `item_list`'s
/// `paint_network_badge`), so the tile's width is the mark's entire width
/// budget. 13pt of height sets the type at [`text_size`]'s 8pt, at which the
/// longest wordmarks this app has -- `VISA`, `AMEX`, `DISC`, all four
/// characters -- fit the tile with the ground's padding included.
/// `a_wordmark_fits_the_list_tile_at_badge_height` holds that, and
/// `CardBrand::wordmark`'s four-character cap is where the constraint is
/// actually enforced.
pub const MARK_BADGE_HEIGHT: f32 = 13.0;

/// The type size for a mark drawn `height` tall.
///
/// A ratio rather than a constant per size, so the badge and the detail mark
/// are one design at two sizes instead of two designs. 0.62 is set against
/// `theme::avatar`'s own 0.38 monogram ratio: this ground is a tight pill
/// around a word rather than a square with a letter floating in it, so the
/// type fills much more of it. The padding beside the word is tighter than a
/// pill's would normally be for one measured reason: at the badge's height,
/// `AMEX` plus a roomier padding came to 32.06pt against a 32pt tile.
fn text_size(height: f32) -> f32 {
    (height * 0.62).round()
}

/// The ground's padding either side of the word.
fn pad_x(height: f32) -> f32 {
    (height * 0.22).round()
}

/// The word, laid out at the size a mark of `height` sets it.
///
/// Bold and letterspaced, which is what makes four characters read as a
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

/// How wide the mark for `brand` is when drawn `height` tall.
///
/// Measured off the same galley [`paint_mark`] paints, so a caller that
/// reserves room for a mark reserves the room the mark takes. The detail pane
/// needs this before it lays out the digits beside it, and a mark left out of
/// that sum is a mark that pushes the reveal eye off a narrow pane.
pub fn mark_width(ui: &egui::Ui, brand: CardBrand, height: f32) -> f32 {
    galley(ui, brand, height).size().x + 2.0 * pad_x(height)
}

/// Paints the mark for `brand`, `height` tall, anchored at `pos`; returns the
/// rect it took.
///
/// **The ground is `theme::BLUE` with the word in `theme::CARD`** -- this
/// app's own blue and its own white, and no network's colours. The same choice
/// the drawn glyphs made, for the same reason: a mark in a network's own
/// livery is a step towards the logo this project deliberately does not ship.
/// The consequence is that the seven marks are told apart by their WORD alone,
/// which is why the word has to be legible, and why [`MARK_BADGE_HEIGHT`] is
/// pinned by a measurement rather than tuned by eye.
pub fn paint_mark(
    ui: &egui::Ui,
    brand: CardBrand,
    height: f32,
    anchor: egui::Align2,
    pos: egui::Pos2,
) -> egui::Rect {
    let galley = galley(ui, brand, height);
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
    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw(), |_ui| {});
        ctx
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
    fn a_wordmark_is_upper_case_and_at_most_four_characters() {
        // The cap is the badge's width budget and not a style rule -- see
        // `CardBrand::wordmark`. Asserted here as well as measured below,
        // because this is the form of the rule a reader can apply while ADDING
        // a network, without rendering anything.
        for brand in CARD_BRANDS {
            let word = brand.wordmark();
            assert!(
                word.chars().all(|c| c.is_ascii_uppercase()),
                "{brand:?}'s wordmark {word:?} is not plain upper-case ASCII"
            );
            assert!(
                word.len() <= 4,
                "{brand:?}'s wordmark {word:?} is {} characters; the list badge has room for 4",
                word.len()
            );
        }
    }

    /// **The measurement the whole design rests on.** The badge is drawn
    /// inside the item row's 32pt avatar tile, so a mark wider than that tile
    /// hangs off the row -- and the only way to fit a longer word is to shrink
    /// the type until it is unreadable, which is the failure this replaced.
    #[test]
    fn a_wordmark_fits_the_list_tile_at_badge_height() {
        // The avatar tile's size. Spelled as the number it is because
        // `item_list::AVATAR_SIZE` is private to that module; the badge really
        // landing inside the tile is that module's own assertion.
        const TILE: f32 = 32.0;
        with_ui(|ui| {
            for brand in CARD_BRANDS {
                let width = mark_width(ui, brand, MARK_BADGE_HEIGHT);
                assert!(
                    width <= TILE,
                    "{brand:?}'s {:?} badge is {width}pt wide at {MARK_BADGE_HEIGHT}pt tall, \
                     which does not fit the {TILE}pt tile it is drawn inside",
                    brand.wordmark()
                );
                // ...and the negative: a word taking real room, not a mark
                // that fits because it has shrunk to nearly nothing.
                assert!(
                    width > MARK_BADGE_HEIGHT,
                    "{brand:?}'s badge is only {width}pt wide, which is not a word"
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
                MARK_BADGE_HEIGHT,
                egui::Align2::RIGHT_BOTTOM,
                egui::Pos2::new(100.0, 60.0),
            );
        });
        assert!(
            (painted.right() - 100.0).abs() < 0.01 && (painted.bottom() - 60.0).abs() < 0.01,
            "the mark was anchored RIGHT_BOTTOM at (100, 60) but landed at {painted:?}"
        );
        assert!((painted.height() - MARK_BADGE_HEIGHT).abs() < 0.01);

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

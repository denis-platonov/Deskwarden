//! The network mark: a card's network, SET IN TYPE, on a small ground.
//!
//! **Words, not logos, and this is a decision rather than a shortcut.** The
//! card networks' marks are registered trademarks whose brand guidelines
//! restrict their use to licensed issuers and merchants. This is an
//! MIT-licensed community project and its author does not want that exposure.
//! Naming which network a card belongs to is a statement of fact about the
//! user's own card, so the mark states it: `VISA`, `MASTERCARD`, `AMEX`. The
//! words come from [`CardBrand::wordmark`], which is the one place brands are
//! named -- there is no second table here to fall out of step with it.
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
/// which is why the word has to be legible, and why [`MARK_ROW_HEIGHT`] is
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

//! The avatar-tile mark for an item that is not a login.
//!
//! **The defect this exists for.** Every row whose item has no favicon fell
//! back to `theme::avatar` with `theme::initials(&item.name)` -- a letter pair
//! in a box. That is right for a login, whose name is a brand and whose
//! monogram therefore says something. It is not right for the rest of a vault:
//! the owner's own file holds 220 secure notes, 170 cards, 150 identities and
//! 30 SSH keys beside its 430 logins, and every one of those 570 non-login
//! rows wore the same shaped tile with a different letter pair in it. Their
//! report was "we should also have different icons for SSH, notes, IDs".
//!
//! **Drawn, not imported.** This crate paints its own marks -- see
//! [`crate::card_mark`], whose wordmarks are laid-out type and whose logos are
//! the user's own files, and [`crate::brand_mark`]. Nothing here adds an image
//! asset, an icon font or a dependency: four glyphs made of `egui` strokes,
//! sized off the tile they sit in so they are one design at any size the tile
//! is ever drawn at.
//!
//! **What this does NOT take over.**
//!
//! * **A favicon still wins.** A card carrying a bank's domain gets the bank's
//!   artwork exactly as it did; the mark is the fallback the monogram was.
//! * **A card's NETWORK stays where it was put.** `VISA` and `MASTERCARD` are
//!   [`crate::card_mark`]'s wordmarks in the row's TRAILING chip run, and the
//!   history in `item_list::paint_network_mark` records that the owner asked
//!   for them out of the tile ("maybe not overlap the icon") and then further
//!   right ("similar to our mfa/app pills"). Putting brand art back in the
//!   tile would undo both requests. So a card's tile carries the GENERIC card
//!   mark whether or not its network is known, and the network is read off the
//!   pill beside the name -- which is also what keeps the tile column
//!   scannable: every card in the list reads as a card at the same glance.
//! * **A login keeps its monogram**, and so does an item of a type this app
//!   does not know ([`ItemKind::Unknown`]). A monogram is a real answer for a
//!   login and the honest one for a kind we cannot name; inventing a mark for
//!   "unsupported" would say more than is known.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, Ui, Vec2};

use crate::theme;
use crate::vault_bridge::ItemKind;

/// The glyph's box, as a fraction of the tile it is centred in: **20pt in a
/// 40pt tile**.
///
/// **Four points smaller than the favicon beside it, and by eye rather than
/// by arithmetic.** It was briefly `theme::ARTWORK_BOX` exactly, on the
/// reasoning that one size down the tile column is one design. The owner
/// looked at it and asked for 20 -- first for the SSH key ("make ssh icons
/// 20px"), then for the rest of the drawn set in the same breath.
///
/// The reason a drawn mark wants less room than an image does is that it is
/// OUTLINE where a favicon is solid: a logo's 24pt square is 24pt of ink,
/// while the key's 24pt square is a thin ring with air around and inside it,
/// so it claims the space without filling it. The four points come back as
/// margin, and the column reads even.
///
/// ONE constant for all four marks, deliberately. The SSH key had its own
/// for a few minutes -- its ring-and-shank spans the box corner to corner
/// where the note's page spans it edge to edge -- and a per-kind table is a
/// place for four numbers to drift apart. If one mark ever reads wrong at
/// this size, the fix belongs in that mark's own geometry, where the shape is
/// described, and not in a second size constant here.
const GLYPH: f32 = 0.5;

/// The stroke every mark is drawn with, as a fraction of the tile size.
///
/// 1/24 is 1.67pt on the list's 40pt tile. **Thinner than it looks like it
/// should be, on purpose:** the note's three rules sit about 3pt apart at
/// this size, and a stroke much heavier closes that gap into a smear.
/// Floored at 1pt so a mark never disappears if the tile is ever drawn
/// small.
fn stroke_width(size: f32) -> f32 {
    (size / 24.0).max(1.0)
}

/// The mark's colour, in the tile's two states.
///
/// **The monogram's own two colours, and deliberately not a third pair.** The
/// selected row inverts the tile (`theme::BLUE_WASH` ground, blue content) and
/// a mark that kept a grey it had chosen for itself would either vanish into
/// that wash or read as a different kind of thing from the monograms around
/// it. Sharing `theme::avatar`'s pair is what makes the selected treatment one
/// decision instead of two.
fn ink(emphasized: bool) -> Color32 {
    if emphasized { theme::BLUE } else { theme::TEXT_MUTED }
}

/// Whether this kind has a mark of its own at all.
///
/// The negative is the interesting half: [`ItemKind::Login`] and
/// [`ItemKind::Unknown`] answer `false` and keep the monogram. See this
/// module's own docs for why.
pub fn has_mark(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::SecureNote | ItemKind::Card | ItemKind::Identity | ItemKind::SshKey
    )
}

/// Draws the avatar tile for `item` when it has no favicon: this module's mark
/// for a kind that has one, and `theme::avatar`'s monogram for a kind that
/// does not.
///
/// One entry point rather than a branch at each call site, because there are
/// two call sites -- the list row and the detail pane's header -- and a row
/// that grew a mark while the header it opens still showed a letter would be
/// two answers to one question.
pub fn avatar(ui: &mut Ui, kind: ItemKind, name: &str, size: f32, emphasized: bool) {
    if !has_mark(kind) {
        theme::avatar(ui, &theme::initials(name), size, emphasized);
        return;
    }
    // `avatar_artwork_tile`, not `avatar_tile`: no fill. The report was
    // "secnote and other icons have gray background - they should not", and
    // it is the same ruling that took the ground out from behind a favicon --
    // a drawn mark is artwork too, and a grey square inside the tile is
    // exactly what the owner asked to remove there. The MONOGRAM keeps its
    // fill, because a letter is type and type needs a ground to sit on; that
    // is the line `has_mark` already draws and this follows it.
    let tile = theme::avatar_artwork_tile(ui, size, emphasized);
    paint_mark(ui, kind, tile, emphasized);
}

/// Paints `kind`'s glyph centred in an already-drawn `tile`.
///
/// Split from [`avatar`] so a caller that has its own tile (a preview sheet
/// laying the marks out side by side, say) draws the very same glyph rather
/// than a second copy of it.
pub fn paint_mark(ui: &Ui, kind: ItemKind, tile: Rect, emphasized: bool) {
    let size = tile.width().min(tile.height());
    let box_side = size * GLYPH;
    let glyph = Rect::from_center_size(tile.center(), Vec2::splat(box_side));
    let stroke = Stroke::new(stroke_width(size), ink(emphasized));
    let painter = ui.painter();
    match kind {
        ItemKind::SecureNote => note(painter, glyph, stroke),
        ItemKind::Card => card(painter, glyph, stroke),
        ItemKind::Identity => identity(painter, glyph, stroke),
        ItemKind::SshKey => ssh_key(painter, glyph, stroke),
        // Unreachable: `avatar` gates on `has_mark`, and the only other
        // caller is the preview sheet, which walks the same list. Drawing
        // nothing is the right answer anyway -- an invented glyph for a kind
        // with no mark is exactly what this module refuses to do.
        ItemKind::Login | ItemKind::Unknown(_) => {}
    }
}

/// **Secure note: a sheet with writing on it.** A portrait page (the glyph box
/// is square, so the page is inset either side of it) with three rules across
/// it, the last one short -- an unfinished last line is what makes three
/// strokes read as text rather than as a grille.
fn note(painter: &egui::Painter, glyph: Rect, stroke: Stroke) {
    let page = Rect::from_center_size(
        glyph.center(),
        egui::vec2(glyph.width() * 0.76, glyph.height()),
    );
    painter.rect_stroke(
        page,
        egui::CornerRadius::same((glyph.width() * 0.12).max(1.0) as u8),
        stroke,
        egui::StrokeKind::Middle,
    );
    let inset = page.width() * 0.2;
    for (n, at) in [0.3f32, 0.5, 0.7].iter().enumerate() {
        let y = page.top() + page.height() * at;
        // The third rule stops short: see this function's own doc.
        let right = if n == 2 { page.right() - inset * 2.2 } else { page.right() - inset };
        painter.line_segment([egui::pos2(page.left() + inset, y), egui::pos2(right, y)], stroke);
    }
}

/// **Card: a landscape card with its magnetic stripe.** The stripe is a FILLED
/// band rather than a fourth outline, which is the whole reason this reads as
/// a card and not as the note lying on its side at a glance.
fn card(painter: &egui::Painter, glyph: Rect, stroke: Stroke) {
    let body = Rect::from_center_size(
        glyph.center(),
        egui::vec2(glyph.width(), glyph.height() * 0.72),
    );
    painter.rect_stroke(
        body,
        egui::CornerRadius::same((glyph.width() * 0.12).max(1.0) as u8),
        stroke,
        egui::StrokeKind::Middle,
    );
    let stripe = Rect::from_min_max(
        egui::pos2(body.left(), body.top() + body.height() * 0.24),
        egui::pos2(body.right(), body.top() + body.height() * 0.48),
    );
    painter.rect_filled(stripe, 0, stroke.color);
}

/// **Identity: head and shoulders.** The shoulders are an open arc rather than
/// a filled half-disc so the mark stays the same weight as the other three --
/// a solid shape beside three outlines is the one that draws the eye down the
/// column.
fn identity(painter: &egui::Painter, glyph: Rect, stroke: Stroke) {
    let head_r = glyph.width() * 0.21;
    let head = egui::pos2(glyph.center().x, glyph.top() + head_r + stroke.width * 0.5);
    painter.circle_stroke(head, head_r, stroke);

    let shoulder_r = glyph.width() * 0.40;
    let base = egui::pos2(glyph.center().x, glyph.bottom() - stroke.width * 0.5);
    // The upper half of a circle centred on the glyph's bottom edge: a dome
    // that meets the edge at both ends, which is what a torso cropped by the
    // tile looks like.
    let arc: Vec<Pos2> = (0..=16)
        .map(|n| {
            let t = std::f32::consts::PI * (1.0 + n as f32 / 16.0);
            egui::pos2(base.x + shoulder_r * t.cos(), base.y + shoulder_r * t.sin())
        })
        .collect();
    painter.add(Shape::line(arc, stroke));
}

/// **SSH key: a key, bit end down.** The bow is a ring (a filled disc would be
/// a dot at this size), the shank drops from it, and two teeth come off the
/// shank's lower half -- two, because one tooth reads as a lollipop and three
/// close up at 32pt.
fn ssh_key(painter: &egui::Painter, glyph: Rect, stroke: Stroke) {
    let bow_r = glyph.width() * 0.26;
    let bow = egui::pos2(glyph.center().x, glyph.top() + bow_r + stroke.width * 0.5);
    painter.circle_stroke(bow, bow_r, stroke);
    let shank_top = bow.y + bow_r;
    painter.line_segment(
        [egui::pos2(bow.x, shank_top), egui::pos2(bow.x, glyph.bottom())],
        stroke,
    );
    let tooth = glyph.width() * 0.26;
    for at in [0.62f32, 0.9] {
        let y = shank_top + (glyph.bottom() - shank_top) * at;
        painter.line_segment([egui::pos2(bow.x, y), egui::pos2(bow.x + tooth, y)], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_that_is_not_a_login_has_a_mark() {
        // The four the owner named, and the two that deliberately do not have
        // one. Spelled out rather than derived, because "which kinds get a
        // mark" is the decision this module exists to make and a derived
        // version of it would assert nothing.
        for kind in [
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
        ] {
            assert!(has_mark(kind), "{kind:?} has no mark of its own");
        }
        assert!(!has_mark(ItemKind::Login), "a login must keep its name's monogram");
        assert!(
            !has_mark(ItemKind::Unknown(9)),
            "an unsupported kind must keep its monogram rather than borrow another kind's mark"
        );
    }

    #[test]
    fn the_stroke_never_thins_below_a_pixel() {
        assert_eq!(stroke_width(32.0), 32.0 / 24.0);
        assert_eq!(stroke_width(8.0), 1.0, "a small tile must still draw a visible mark");
    }

    #[test]
    fn the_selected_treatment_is_the_monograms_own_pair() {
        // A mark that kept its unselected grey on a selected row would sink
        // into `theme::BLUE_WASH`; one that invented a third colour would
        // read as a different kind of thing from the monograms beside it.
        assert_eq!(ink(false), theme::TEXT_MUTED);
        assert_eq!(ink(true), theme::BLUE);
        assert_ne!(ink(false), ink(true));
    }
}

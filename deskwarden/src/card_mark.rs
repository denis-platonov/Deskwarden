//! The drawn network marks: one small badge per card network.
//!
//! **Drawn here, not shipped from the networks.** `assets/generate-marks.py`
//! renders these the way `assets/generate-icon.py` renders the application
//! icon -- from source geometry, with the standard library, so no opaque
//! binary whose provenance anyone has to take on trust enters the repository.
//! The second reason is recorded in the design spec: each network's brand
//! guidelines set rules on colour, clear space and minimum size that a 12 px
//! corner badge breaks simply by existing.
//!
//! **They are not imitations.** Each is a generic geometric glyph in this
//! app's own palette that stands for a network -- a wedge, a diamond, a ring
//! -- and none borrows a distinctive element of the real mark. The honest cost
//! is that a drawn mark is less immediately recognisable than the real one.
//!
//! An unrecognised brand has no mark: [`mark_for`] takes a [`CardBrand`], and
//! the caller that could not name a brand draws nothing rather than a
//! placeholder.

use crate::card_brand::CardBrand;

/// One network's mark: the file it was generated into, and its bytes.
///
/// The bytes are 48x48 8-bit RGBA PNG -- the detail size. The 12 px list badge
/// is this scaled down, rather than a second asset, so there is exactly one
/// drawing per network and no way for two sizes to drift into two designs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkMark {
    /// The mark's file stem under `assets/marks/`.
    ///
    /// Derivable from the brand's canonical spelling, and
    /// [`a_marks_file_name_is_derived_from_the_brands_own_spelling`] holds it
    /// so: a mark filed under the wrong name is a mark on the wrong card.
    pub key: &'static str,
    /// The PNG bytes, embedded at compile time.
    pub png: &'static [u8],
}

/// The mark table.
///
/// Deliberately a lookup and not a `match` returning bytes: removing a brand's
/// entry here is what "a brand ships markless" looks like, and it reds
/// [`every_brand_has_a_mark`] rather than compiling into a blank badge.
const MARKS: &[(CardBrand, NetworkMark)] = &[
    (
        CardBrand::Visa,
        NetworkMark { key: "visa", png: include_bytes!("../assets/marks/visa.png") },
    ),
    (
        CardBrand::Mastercard,
        NetworkMark { key: "mastercard", png: include_bytes!("../assets/marks/mastercard.png") },
    ),
    (
        CardBrand::AmericanExpress,
        NetworkMark {
            key: "american_express",
            png: include_bytes!("../assets/marks/american_express.png"),
        },
    ),
    (
        CardBrand::Discover,
        NetworkMark { key: "discover", png: include_bytes!("../assets/marks/discover.png") },
    ),
    (
        CardBrand::Jcb,
        NetworkMark { key: "jcb", png: include_bytes!("../assets/marks/jcb.png") },
    ),
    (
        CardBrand::DinersClub,
        NetworkMark { key: "diners_club", png: include_bytes!("../assets/marks/diners_club.png") },
    ),
    (
        CardBrand::UnionPay,
        NetworkMark { key: "unionpay", png: include_bytes!("../assets/marks/unionpay.png") },
    ),
];

/// The mark that stands for `brand`.
pub fn mark_for(brand: CardBrand) -> Option<&'static NetworkMark> {
    MARKS.iter().find(|(b, _)| *b == brand).map(|(_, mark)| mark)
}

/// The size, in pixels each way, the marks are generated at.
pub const MARK_DETAIL_PX: u32 = 48;

/// The size the list tile's corner badge draws at.
///
/// **This is the number the whole approach was tested against.** Seven marks
/// inside one blue palette have to be tellable apart here, not at detail size;
/// a pair that only separates when enlarged is a pair that does not work.
///
/// Smaller than [`MARK_DETAIL_PX`] on purpose: scaling down resamples, while
/// scaling up would show the generator's own pixels. If the badge ever needs
/// to be larger, the answer is to regenerate at a larger size.
pub const MARK_BADGE_PX: u32 = 12;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_brand::CARD_BRANDS;

    /// Width and height out of a PNG's IHDR, which is the first chunk and at a
    /// fixed offset. Enough to tell a real image from a truncated one without
    /// pulling a decoder into a unit test.
    fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n".as_slice() || bytes.get(12..16)? != b"IHDR" {
            return None;
        }
        let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        Some((w, h))
    }

    #[test]
    fn every_brand_has_a_mark() {
        // Driven from the enumeration and not from a hand-written list, so a
        // brand added later cannot ship markless -- it reds here instead of
        // rendering a card with a blank corner.
        for brand in CARD_BRANDS {
            let mark = mark_for(brand).unwrap_or_else(|| panic!("{brand:?} has no mark"));
            assert_eq!(
                png_size(mark.png),
                Some((MARK_DETAIL_PX, MARK_DETAIL_PX)),
                "{brand:?}'s mark is not a {MARK_DETAIL_PX}px PNG"
            );
        }
        // Control: the table is the whole enumeration and not a superset with
        // strays, so the loop above ranged over every entry there is.
        assert_eq!(MARKS.len(), CARD_BRANDS.len());
    }

    #[test]
    fn no_two_brands_share_a_mark() {
        // The extreme form of the failure this design is most exposed to: two
        // networks that cannot be told apart because they are the same
        // picture. Distinguishability at 12px is a judgement a human made by
        // looking; identity is one a test can hold.
        for (i, (brand, mark)) in MARKS.iter().enumerate() {
            for (other, other_mark) in MARKS.iter().skip(i + 1) {
                assert_ne!(mark.png, other_mark.png, "{brand:?} and {other:?} are one drawing");
                assert_ne!(mark.key, other_mark.key, "{brand:?} and {other:?} share a file");
            }
        }
    }

    #[test]
    fn a_marks_file_name_is_derived_from_the_brands_own_spelling() {
        // The table names a file per brand by hand, and a hand-written pairing
        // can be crossed -- Discover's bytes filed under Diners Club would
        // draw the wrong network on a real card and nothing else would notice.
        // Recomputing the name from `canonical()` is the check.
        for brand in CARD_BRANDS {
            let expected = brand.canonical().to_ascii_lowercase().replace(' ', "_");
            let mark = mark_for(brand).expect("every brand has a mark");
            assert_eq!(mark.key, expected, "{brand:?} is filed under the wrong name");
        }
        // Control: the derivation really does distinguish the two brands whose
        // names both begin with D, so agreement above is not agreement on a
        // constant.
        assert_eq!(mark_for(CardBrand::Discover).unwrap().key, "discover");
        assert_eq!(mark_for(CardBrand::DinersClub).unwrap().key, "diners_club");
    }

}

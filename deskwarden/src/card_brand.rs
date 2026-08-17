//! Card networks, and everything that follows from one.
//!
//! One pure table. [`brand_for_number`] is load-bearing four times over: the
//! network badge, the digit grouping, the mask and the security-code length
//! are all read from here, so a wrong answer is four wrong answers -- one of
//! which is the mask, which is what tells a user whether the card on screen is
//! the card in their hand.

/// A card network, in Bitwarden's own enumeration.
///
/// Deliberately a plain enum holding nothing: it is derived FROM a card
/// number and never holds one, so it cannot reach a `Zeroizing` and a derived
/// `Debug` on it leaks nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardBrand {
    Visa,
    Mastercard,
    AmericanExpress,
    Discover,
    Jcb,
    DinersClub,
    UnionPay,
}

/// Every brand, once. The dropdown, the mark table and the round-trip test are
/// all driven from this rather than from a second hand-written list, so a
/// brand added later cannot be half-wired.
pub const CARD_BRANDS: [CardBrand; 7] = [
    CardBrand::Visa,
    CardBrand::Mastercard,
    CardBrand::AmericanExpress,
    CardBrand::Discover,
    CardBrand::Jcb,
    CardBrand::DinersClub,
    CardBrand::UnionPay,
];

impl CardBrand {
    /// Bitwarden's own spelling, shared with every other client.
    ///
    /// Not a display choice: the web vault renders its own card art from this
    /// string, so `"MC"` or `"MasterCard"` gives a card that looks right here
    /// and blank everywhere else.
    pub fn canonical(self) -> &'static str {
        match self {
            CardBrand::Visa => "Visa",
            CardBrand::Mastercard => "Mastercard",
            CardBrand::AmericanExpress => "American Express",
            CardBrand::Discover => "Discover",
            CardBrand::Jcb => "JCB",
            CardBrand::DinersClub => "Diners Club",
            CardBrand::UnionPay => "UnionPay",
        }
    }

    /// The digit groups the network prints on the plastic.
    pub fn grouping(self) -> &'static [usize] {
        match self {
            CardBrand::AmericanExpress => &[4, 6, 5],
            CardBrand::DinersClub => &[4, 6, 4],
            _ => &[4, 4, 4, 4],
        }
    }

    /// Three digits, or Amex's four.
    pub fn security_code_len(self) -> usize {
        match self {
            CardBrand::AmericanExpress => 4,
            _ => 3,
        }
    }

    /// Read a `brand` string off the wire.
    ///
    /// Case-insensitive because it has to be: `vault_bridge`'s own fixtures
    /// carry both `"Visa"` and `"visa"`, and what other clients write is not
    /// this app's to normalise.
    pub fn from_canonical(s: &str) -> Option<Self> {
        let s = s.trim();
        CARD_BRANDS
            .into_iter()
            .find(|b| b.canonical().eq_ignore_ascii_case(s))
    }
}

/// One prefix rule: the numeric range `lo..=hi` over the leading `len` digits.
struct PrefixRule {
    len: u32,
    lo: u32,
    hi: u32,
    brand: CardBrand,
}

/// The issuer-identification ranges, **longest prefix first**.
///
/// The order is the whole correctness argument. `3530` is JCB and not Diners,
/// `6011` is Discover and not UnionPay's neighbourhood; consulting the two
/// digit rules first would badge, group and mask real cards as the wrong
/// network. Scanning longest-first makes "longest prefix wins" a property of
/// the table rather than of the reader.
const PREFIX_RULES: &[PrefixRule] = &[
    // Four digits.
    PrefixRule { len: 4, lo: 3528, hi: 3589, brand: CardBrand::Jcb },
    PrefixRule { len: 4, lo: 6011, hi: 6011, brand: CardBrand::Discover },
    PrefixRule { len: 4, lo: 2221, hi: 2720, brand: CardBrand::Mastercard },
    // Three digits.
    PrefixRule { len: 3, lo: 300, hi: 305, brand: CardBrand::DinersClub },
    PrefixRule { len: 3, lo: 644, hi: 649, brand: CardBrand::Discover },
    // Two digits.
    PrefixRule { len: 2, lo: 34, hi: 34, brand: CardBrand::AmericanExpress },
    PrefixRule { len: 2, lo: 37, hi: 37, brand: CardBrand::AmericanExpress },
    PrefixRule { len: 2, lo: 36, hi: 36, brand: CardBrand::DinersClub },
    PrefixRule { len: 2, lo: 38, hi: 39, brand: CardBrand::DinersClub },
    PrefixRule { len: 2, lo: 51, hi: 55, brand: CardBrand::Mastercard },
    PrefixRule { len: 2, lo: 62, hi: 62, brand: CardBrand::UnionPay },
    PrefixRule { len: 2, lo: 65, hi: 65, brand: CardBrand::Discover },
    // One digit.
    PrefixRule { len: 1, lo: 4, hi: 4, brand: CardBrand::Visa },
];

/// The digits of `number`, in order, ignoring the spaces and dashes a card is
/// commonly typed with.
fn digits_of(number: &str) -> String {
    number.chars().filter(char::is_ascii_digit).collect()
}

/// The network a number belongs to, or `None`.
///
/// **`None` is a correct answer and a wrong brand is not.** A number being
/// typed passes through prefixes that match nothing, and every consumer here
/// -- badge, grouping, mask, code length -- degrades gracefully on `None`
/// while a wrong brand misinforms all four at once. No Luhn check: that
/// answers a different question, and a half-typed number always fails it.
pub fn brand_for_number(digits: &str) -> Option<CardBrand> {
    let digits = digits_of(digits);
    PREFIX_RULES
        .iter()
        .find(|rule| {
            digits
                .get(..rule.len as usize)
                .and_then(|head| head.parse::<u32>().ok())
                .is_some_and(|head| (rule.lo..=rule.hi).contains(&head))
        })
        .map(|rule| rule.brand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_network_is_recognised_by_its_own_prefix() {
        assert_eq!(brand_for_number("4111111111111111"), Some(CardBrand::Visa));
        assert_eq!(
            brand_for_number("5555555555554444"),
            Some(CardBrand::Mastercard)
        );
        assert_eq!(
            brand_for_number("2221000000000009"),
            Some(CardBrand::Mastercard)
        );
        assert_eq!(
            brand_for_number("378282246310005"),
            Some(CardBrand::AmericanExpress)
        );
        assert_eq!(
            brand_for_number("6011111111111117"),
            Some(CardBrand::Discover)
        );
        assert_eq!(brand_for_number("3530111333300000"), Some(CardBrand::Jcb));
        assert_eq!(brand_for_number("30569309025904"), Some(CardBrand::DinersClub));
        assert_eq!(
            brand_for_number("6200000000000005"),
            Some(CardBrand::UnionPay)
        );
    }

    #[test]
    fn an_unrecognised_prefix_is_none_and_never_a_wrong_guess() {
        // The failure mode that matters: a partially typed number passes
        // through prefixes matching nothing. `None` is correct; a wrong brand
        // is not.
        for s in ["", "1", "9", "99999999", "abcd"] {
            assert_eq!(brand_for_number(s), None, "{s:?} produced a brand");
        }
        // Control: the function is not simply always None.
        assert_eq!(brand_for_number("4"), Some(CardBrand::Visa));
    }

    #[test]
    fn the_longest_prefix_wins_where_the_ranges_overlap() {
        // `35..` lives inside Diners' `3` neighbourhood and Amex's; the four
        // digit JCB range has to be consulted before the shorter rules or a
        // real JCB card is badged, grouped and masked as something else.
        assert_eq!(brand_for_number("3530111333300000"), Some(CardBrand::Jcb));
        // `3` alone is nobody's: the shorter rules are `34`/`37`/`36`/`38`/`39`
        // and none of them matches one digit.
        assert_eq!(brand_for_number("3"), None);
        // Controls on the neighbours, so the JCB answer above is a decision
        // between live rules and not the only rule beginning with a 3.
        assert_eq!(
            brand_for_number("3782822463100"),
            Some(CardBrand::AmericanExpress)
        );
        assert_eq!(brand_for_number("36000000000008"), Some(CardBrand::DinersClub));
        // `6011` is Discover and `62` is UnionPay -- both begin with a 6.
        assert_eq!(brand_for_number("6011000000000004"), Some(CardBrand::Discover));
        assert_eq!(
            brand_for_number("6200000000000005"),
            Some(CardBrand::UnionPay)
        );
        assert_eq!(brand_for_number("6500000000000002"), Some(CardBrand::Discover));
    }

    #[test]
    fn the_canonical_spellings_are_bitwardens_own() {
        // Interoperability, not tidiness: `brand` is shared with every other
        // Bitwarden client and the web vault draws its own card art from it.
        // "MC" or "MasterCard" renders here and blank everywhere else.
        assert_eq!(CardBrand::Mastercard.canonical(), "Mastercard");
        assert_eq!(CardBrand::AmericanExpress.canonical(), "American Express");
        assert_eq!(CardBrand::DinersClub.canonical(), "Diners Club");
        assert_eq!(CardBrand::Jcb.canonical(), "JCB");
        assert_eq!(CardBrand::UnionPay.canonical(), "UnionPay");
    }

    #[test]
    fn a_brand_read_from_the_wire_is_case_insensitive() {
        // Fixtures in vault_bridge carry both "Visa" and "visa".
        assert_eq!(CardBrand::from_canonical("visa"), Some(CardBrand::Visa));
        assert_eq!(CardBrand::from_canonical("VISA"), Some(CardBrand::Visa));
        assert_eq!(CardBrand::from_canonical("Not A Brand"), None);
    }

    #[test]
    fn every_brand_agrees_with_itself() {
        // The round trip, over the whole enumeration, so a brand added later
        // cannot be half-wired.
        for b in CARD_BRANDS {
            assert_eq!(CardBrand::from_canonical(b.canonical()), Some(b), "{b:?}");
            assert!(!b.grouping().is_empty(), "{b:?} has no grouping");
            assert!(matches!(b.security_code_len(), 3 | 4), "{b:?}");
        }
    }

    #[test]
    fn a_groupings_digits_add_up_to_a_length_that_network_really_issues() {
        // The grouping is what the mask is drawn from, so a grouping summing
        // to the wrong total draws a card of the wrong length.
        for b in CARD_BRANDS {
            let total: usize = b.grouping().iter().sum();
            let expected = match b {
                CardBrand::AmericanExpress => 15,
                CardBrand::DinersClub => 14,
                _ => 16,
            };
            assert_eq!(total, expected, "{b:?} groups to {total} digits");
        }
    }

}

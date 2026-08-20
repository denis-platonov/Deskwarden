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

    /// The network's name as the mark SETS IT IN TYPE -- `VISA`,
    /// `MASTERCARD`, `AMEX`.
    ///
    /// **Words, because this app does not ship the logos.** The network marks
    /// are registered trademarks whose brand guidelines restrict their use to
    /// licensed issuers and merchants; this is an MIT-licensed community
    /// project and its author does not want that exposure. Naming which
    /// network a card belongs to is a statement of fact about the user's own
    /// card, and a word is how a fact is stated.
    ///
    /// **Full words, now that the mark has the row's width and not a tile
    /// corner's.** These were once capped at four characters -- `MC`, `DC`,
    /// `UP` -- because the badge was drawn INSIDE the list row's 32pt avatar
    /// tile and 32pt was the entire width budget. The mark has since moved
    /// out of the tile and sits beside it, in a pill on the row (see
    /// `card_mark::MARK_ROW_HEIGHT`), so the budget is now the row's and the
    /// abbreviations are no longer paying for anything.
    ///
    /// Measured, not assumed. At the 11pt the row sets these in, the pills
    /// come to `JCB` 31pt, `VISA` 35pt, `AMEX` 41pt, `DINERS` 51pt,
    /// `UNIONPAY` 67pt, `DISCOVER` 67pt, `MASTERCARD` 88pt. The list pane is
    /// a fixed 390pt (`vault_window::LIST_WIDTH`, not resizable), which leaves
    /// the title column 301pt before the pill; the widest of these takes 88
    /// plus the row's 11pt gap and still leaves the name 202pt, well over what
    /// the name and its `(*9988)` suffix need.
    ///
    /// **`AMEX` is the one that stays short, and that is the measurement
    /// talking.** `AMERICAN EXPRESS` is 125pt -- over a third of the title
    /// column, for a pill whose job is to annotate the name rather than
    /// compete with it -- and `AMEX` is what that network's own mark is
    /// commonly written as anyway. `DINERS` likewise over `DINERS CLUB`
    /// (84pt).
    ///
    /// Deliberately NOT derived from [`canonical`](Self::canonical) by rule:
    /// `American Express` -> `AMEX` and `UnionPay` -> `UNIONPAY` are not one
    /// rule, and any rule that produced both would have been fitted to this
    /// table anyway. It is a `match` on the enum, in the one file that names
    /// brands, so a network added later does not compile without one.
    pub fn wordmark(self) -> &'static str {
        match self {
            CardBrand::Visa => "VISA",
            CardBrand::Mastercard => "MASTERCARD",
            CardBrand::AmericanExpress => "AMEX",
            CardBrand::Discover => "DISCOVER",
            CardBrand::Jcb => "JCB",
            CardBrand::DinersClub => "DINERS",
            CardBrand::UnionPay => "UNIONPAY",
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

/// The dot a masked digit is drawn as.
const BULLET: char = '\u{2022}';

/// How many trailing digits a mask may reveal, and the floor below which it
/// reveals none.
///
/// The last four are printed on every receipt and asked for by every support
/// line: they are what identifies the card to its owner and they are not
/// enough to use it. Below four stored digits that trade stops paying -- "the
/// last four" of a three-digit fragment is most of it -- so a partial number,
/// which is a data-entry state and not a card, reveals nothing.
const REVEALED: usize = 4;

/// The masked number: `•••• •••• •••• 4242`, or `•••• •••••• •0005` for an
/// Amex.
///
/// **The dot count follows the digits actually stored.** The brand table is
/// the authority for GROUPING and nothing else, so a card whose prefix nobody
/// recognises still masks to its true length rather than to a guess -- a mask
/// that draws sixteen over a fifteen-digit card tells the user the card on
/// screen is a different card from the one in their hand.
pub fn mask_for(number: &str, brand: Option<CardBrand>) -> String {
    let digits = digits_of(number);
    let shown_from = digits.len().saturating_sub(REVEALED);
    let body: Vec<char> = digits
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if digits.len() >= REVEALED && i >= shown_from {
                c
            } else {
                BULLET
            }
        })
        .collect();

    grouped(&body, brand)
}

/// The number as the card prints it, every digit legible:
/// `4111 1111 1111 1111`, or `3782 822463 10005` for an Amex.
///
/// **The revealed twin of [`mask_for`], and deliberately its twin.** Revealing
/// a number exists so a person can check it against the plastic in their hand,
/// and a sixteen-digit run is exactly what cannot be checked. Both forms are
/// grouped by [`grouped`] over a body of the same length, so the spaces land
/// in the same places by construction rather than by two graders agreeing --
/// see `the_mask_and_the_revealed_number_break_in_the_same_places`.
///
/// **Digits only, exactly as the mask counts digits only.** A number stored as
/// `4111-1111-1111-1111` masks to twelve dots and four digits, so revealing it
/// has to show those same sixteen digits and not the user's dashes; letting
/// the two disagree about which characters are digits is the whole defect.
/// The value copied to the clipboard is untouched by this -- the mask and this
/// are both display, and `masked_row` copies the stored string.
///
/// A number too short for the mask to reveal anything still shows here in
/// full: revealing is an explicit act, so what is stored is what is shown, and
/// three digits group as one group of three rather than being padded.
pub fn grouped_for(number: &str, brand: Option<CardBrand>) -> String {
    let body: Vec<char> = digits_of(number).chars().collect();
    grouped(&body, brand)
}

/// Space out `body` at the brand's group boundaries.
///
/// **The one grouper.** Its callers differ only in what they put in `body` --
/// bullets with the last four kept, or every digit -- and never in how long
/// `body` is, which is what makes the two forms line up.
fn grouped(body: &[char], brand: Option<CardBrand>) -> String {
    let mut out = String::with_capacity(body.len() + body.len() / 4);
    for (i, c) in body.iter().enumerate() {
        if group_starts_at(i, body.len(), brand) && !out.is_empty() {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

/// Whether a space falls before position `i` of a `len`-digit number.
///
/// The brand's own grouping is used when it accounts for exactly the digits
/// stored; otherwise -- a half-typed number, or a length no table entry
/// claims -- the fallback groups in fours **from the right**, which keeps the
/// revealed last four together in one group instead of splitting them across
/// a boundary that the left-hand grouping would put in the middle of them.
fn group_starts_at(i: usize, len: usize, brand: Option<CardBrand>) -> bool {
    let own = brand
        .map(CardBrand::grouping)
        .filter(|groups| groups.iter().sum::<usize>() == len);
    match own {
        Some(groups) => {
            let mut at = 0;
            groups.iter().any(|g| {
                at += g;
                at - g == i
            })
        }
        None => len.saturating_sub(i).is_multiple_of(4),
    }
}

/// The masked security code: `•••`, or `••••` for an Amex.
///
/// **Never revealed, unlike the last four.** A security code has no
/// identification use to trade against its risk, so seeing it stays a
/// deliberate act behind the reveal affordance. With no brand to consult the
/// stored code's own length is the truth, bounded by the only two lengths a
/// code has.
pub fn code_mask_for(code: &str, brand: Option<CardBrand>) -> String {
    let len = match brand {
        Some(b) => b.security_code_len(),
        None => digits_of(code).len().clamp(3, 4),
    };
    BULLET.to_string().repeat(len)
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


    #[test]
    fn a_visa_masks_in_fours_with_the_last_four_shown() {
        assert_eq!(
            mask_for("4111111111111111", Some(CardBrand::Visa)),
            "•••• •••• •••• 1111"
        );
    }

    #[test]
    fn an_amex_masks_in_its_own_grouping() {
        // The whole reason this is not a fixed sixteen-in-fours: an Amex is
        // fifteen digits in 4-6-5. A mask that draws sixteen tells the user
        // their card is a different card from the one in their hand.
        assert_eq!(
            mask_for("378282246310005", Some(CardBrand::AmericanExpress)),
            "•••• •••••• •0005"
        );
    }

    #[test]
    fn fewer_than_four_digits_reveals_nothing() {
        // A partial number is a data-entry state, not a card, and "the last
        // four" of a three-digit fragment is most of it.
        let masked = mask_for("123", Some(CardBrand::Visa));
        assert!(
            !masked.contains('1') && !masked.contains('2') && !masked.contains('3'),
            "a digit survived masking: {masked}"
        );
        assert_eq!(masked.chars().filter(|c| *c == BULLET).count(), 3);
        // Control: four digits DOES reveal, so the rule is the floor and not
        // "never reveals".
        assert!(mask_for("1234", Some(CardBrand::Visa)).ends_with("1234"));
    }

    #[test]
    fn an_unknown_brand_masks_to_the_length_actually_stored() {
        // The table is the authority for GROUPING; the dot count follows the
        // digits there really are, so an unrecognised card still masks true.
        let masked = mask_for("1234567890123", None);
        assert!(masked.ends_with("0123"), "{masked}");
        assert_eq!(masked.chars().filter(|c| *c == BULLET).count(), 9);
    }

    #[test]
    fn a_number_stored_with_spaces_masks_to_its_digits_and_not_its_characters() {
        // What `bw` round-trips is whatever was typed, and a card typed in
        // groups is normal. Masking the characters would draw a longer card.
        let masked = mask_for("4111 1111 1111 1111", Some(CardBrand::Visa));
        assert_eq!(masked, "•••• •••• •••• 1111");
        assert_eq!(masked.chars().filter(|c| *c == BULLET).count(), 12);
    }

    /// The positions of the spaces in `s`, which is the only thing the mask
    /// and the revealed number have to agree about.
    fn space_positions(s: &str) -> Vec<usize> {
        s.chars()
            .enumerate()
            .filter(|(_, c)| *c == ' ')
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn the_mask_and_the_revealed_number_break_in_the_same_places() {
        // **The property, not a table.** The reported bug was a revealed
        // number with no spaces at all under a mask that had them; the danger
        // in fixing it is a SECOND grouper that agrees on a Visa and drifts on
        // an Amex. So the assertion is the invariant itself, over every brand
        // and over the lengths that break the brand's own grouping.
        let numbers = [
            "4111111111111111",
            "5555555555554444",
            "378282246310005",
            "30569309025904",
            "6011111111111117",
            "3530111333300000",
            "6200000000000005",
            // Lengths no grouping claims, which take the fours-from-the-right
            // fallback -- the case a second grouper is most likely to get
            // wrong.
            "1234567890123",
            "12345678901234567890",
            "12345",
            "1234",
            "123",
            "1",
            "",
        ];
        for n in numbers {
            for brand in CARD_BRANDS.map(Some).into_iter().chain([None]) {
                let masked = mask_for(n, brand);
                let revealed = grouped_for(n, brand);
                assert_eq!(
                    space_positions(&masked),
                    space_positions(&revealed),
                    "{n:?} as {brand:?}: mask {masked:?} vs revealed {revealed:?}"
                );
                // And the same total width, so the row cannot jump when the
                // eye is clicked.
                assert_eq!(masked.chars().count(), revealed.chars().count(), "{n:?}");
            }
        }
        // Control: the helper can SEE a disagreement. Without this the
        // assertion above passes just as well on two functions that both
        // return the empty string.
        assert_ne!(
            space_positions(&mask_for("4111111111111111", Some(CardBrand::Visa))),
            space_positions("4111111111111111"),
            "the ungrouped form is what the bug looked like and must not match"
        );
        assert_eq!(
            space_positions(&mask_for("4111111111111111", Some(CardBrand::Visa))),
            vec![4, 9, 14]
        );
    }

    #[test]
    fn a_revealed_number_reads_in_its_networks_own_groups() {
        assert_eq!(
            grouped_for("4111111111111111", Some(CardBrand::Visa)),
            "4111 1111 1111 1111"
        );
        // Fifteen digits in 4-6-5, the same shape the mask draws.
        assert_eq!(
            grouped_for("378282246310005", Some(CardBrand::AmericanExpress)),
            "3782 822463 10005"
        );
        assert_eq!(
            grouped_for("30569309025904", Some(CardBrand::DinersClub)),
            "3056 930902 5904"
        );
    }

    #[test]
    fn revealing_normalises_to_the_digits_the_mask_counted() {
        // The stored string is whatever `bw` round-trips, dashes and all. The
        // mask counts digits; so must the revealed form, or the two disagree
        // about which characters exist and the spaces cannot line up.
        assert_eq!(
            grouped_for("4111-1111-1111-1111", Some(CardBrand::Visa)),
            "4111 1111 1111 1111"
        );
        assert_eq!(
            grouped_for("4111 1111 1111 1111", Some(CardBrand::Visa)),
            "4111 1111 1111 1111"
        );
        // Not merely "spaces survive": a non-digit is dropped, never kept.
        assert!(!grouped_for("4111-1111-1111-1111", Some(CardBrand::Visa)).contains('-'));
    }

    #[test]
    fn a_number_too_short_to_mask_still_reveals_what_is_stored() {
        // The mask deliberately reveals nothing below four digits; revealing
        // is the user's explicit act, so it shows the fragment -- grouped as
        // the one short group it is, not padded and not panicking.
        assert_eq!(grouped_for("123", Some(CardBrand::Visa)), "123");
        assert_eq!(grouped_for("1", None), "1");
        assert_eq!(grouped_for("", None), "");
        assert_eq!(grouped_for("12345", Some(CardBrand::Visa)), "1 2345");
    }

    #[test]
    fn the_security_code_mask_is_four_for_amex_and_three_otherwise() {
        assert_eq!(
            code_mask_for("1234", Some(CardBrand::AmericanExpress)),
            "••••"
        );
        assert_eq!(code_mask_for("123", Some(CardBrand::Visa)), "•••");
        // Never revealed by the mask, unlike the last four: a code has no
        // identification use to trade against its risk.
        assert!(!code_mask_for("123", Some(CardBrand::Visa)).contains('1'));
    }

    #[test]
    fn an_unknown_brands_code_mask_follows_the_code_that_is_stored() {
        // Same principle as the number: with no table to consult, the truth
        // is the stored value's own length rather than a guess at three.
        assert_eq!(code_mask_for("1234", None), "••••");
        assert_eq!(code_mask_for("123", None), "•••");
        // An empty code still draws a field rather than nothing, and never
        // more dots than a code can have.
        assert_eq!(code_mask_for("", None), "•••");
        assert_eq!(code_mask_for("1234567", None), "••••");
    }
}

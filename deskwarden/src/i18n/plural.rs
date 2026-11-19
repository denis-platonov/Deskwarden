//! The CLDR plural categories for the locales this app ships, as one small
//! pure table.
//!
//! Only whole numbers are asked about -- every count in this app (items,
//! seconds, minutes, accounts) is an integer -- so the operands CLDR names
//! `v`, `w`, `f` and `t` (the visible fraction digits) are always zero and the
//! rules below are the integer halves of the published ones.
//!
//! The table is checked against the CLDR 44 plural chart by
//! `the_rules_agree_with_the_published_chart`, which lists, per language, the
//! counts that must land in each category.

/// A CLDR plural category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

impl Category {
    /// The suffix a catalog key carries: `key[one]`.
    pub const fn name(self) -> &'static str {
        match self {
            Category::Zero => "zero",
            Category::One => "one",
            Category::Two => "two",
            Category::Few => "few",
            Category::Many => "many",
            Category::Other => "other",
        }
    }

    /// The category a catalog suffix names, or `None` for a word that is not
    /// one.
    pub fn from_name(name: &str) -> Option<Category> {
        Some(match name {
            "zero" => Category::Zero,
            "one" => Category::One,
            "two" => Category::Two,
            "few" => Category::Few,
            "many" => Category::Many,
            "other" => Category::Other,
            _ => return None,
        })
    }
}

/// Which family of rule a locale follows. Named for the language group, not
/// the locale, because several locales share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// No plural distinction at all: `other` only. id, ja, ko, ms, vi, zh.
    Other,
    /// `one` for exactly 1. bg, da, de, el, en, et, fi, hu, nb, nl, sv, tr
    /// and (for whole numbers) ca, it, es, pt-PT with the `many` addition
    /// below for the millions.
    OneOther,
    /// `one` for 1, and `many` for the exact millions: ca, es, it, pt-PT.
    OneManyOther,
    /// `one` for 0 and 1, `many` for the exact millions: fr, pt-BR.
    ZeroOneManyOther,
    /// East Slavic: ru, uk. one / few / many.
    EastSlavic,
    /// Polish: one / few / many.
    Polish,
    /// Czech and Slovak: one / few / other (`many` is for fractions only).
    CzechSlovak,
    /// Croatian and Serbian: one / few / other.
    SouthSlavic,
    /// Slovenian: one / two / few / other.
    Slovenian,
    /// Lithuanian: one / few / other (`many` is for fractions only).
    Lithuanian,
    /// Latvian: zero / one / other.
    Latvian,
    /// Romanian: one / few / other.
    Romanian,
}

impl Rule {
    /// The category `n` falls in.
    pub fn category(self, n: u64) -> Category {
        let m10 = n % 10;
        let m100 = n % 100;
        match self {
            Rule::Other => Category::Other,
            Rule::OneOther => {
                if n == 1 {
                    Category::One
                } else {
                    Category::Other
                }
            }
            Rule::OneManyOther => {
                if n == 1 {
                    Category::One
                } else if is_exact_million(n) {
                    Category::Many
                } else {
                    Category::Other
                }
            }
            Rule::ZeroOneManyOther => {
                if n <= 1 {
                    Category::One
                } else if is_exact_million(n) {
                    Category::Many
                } else {
                    Category::Other
                }
            }
            Rule::EastSlavic => {
                if m10 == 1 && m100 != 11 {
                    Category::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    Category::Few
                } else {
                    Category::Many
                }
            }
            Rule::Polish => {
                if n == 1 {
                    Category::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    Category::Few
                } else {
                    Category::Many
                }
            }
            Rule::CzechSlovak => {
                if n == 1 {
                    Category::One
                } else if (2..=4).contains(&n) {
                    Category::Few
                } else {
                    Category::Other
                }
            }
            Rule::SouthSlavic => {
                if m10 == 1 && m100 != 11 {
                    Category::One
                } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                    Category::Few
                } else {
                    Category::Other
                }
            }
            Rule::Slovenian => match m100 {
                1 => Category::One,
                2 => Category::Two,
                3 | 4 => Category::Few,
                _ => Category::Other,
            },
            Rule::Lithuanian => {
                if m10 == 1 && !(11..=19).contains(&m100) {
                    Category::One
                } else if (2..=9).contains(&m10) && !(11..=19).contains(&m100) {
                    Category::Few
                } else {
                    Category::Other
                }
            }
            Rule::Latvian => {
                if m10 == 0 || (11..=19).contains(&m100) {
                    Category::Zero
                } else if m10 == 1 && m100 != 11 {
                    Category::One
                } else {
                    Category::Other
                }
            }
            Rule::Romanian => {
                if n == 1 {
                    Category::One
                } else if n == 0 || (2..=19).contains(&m100) {
                    Category::Few
                } else {
                    Category::Other
                }
            }
        }
    }

    /// The categories a translator of a language on this rule must supply
    /// for a plural key: every one `category` can answer, `other` last.
    pub fn categories(self) -> &'static [Category] {
        use Category::*;
        match self {
            Rule::Other => &[Other],
            Rule::OneOther => &[One, Other],
            Rule::OneManyOther | Rule::ZeroOneManyOther => &[One, Many, Other],
            Rule::EastSlavic | Rule::Polish => &[One, Few, Many, Other],
            Rule::CzechSlovak | Rule::SouthSlavic | Rule::Lithuanian | Rule::Romanian => {
                &[One, Few, Other]
            }
            Rule::Slovenian => &[One, Two, Few, Other],
            Rule::Latvian => &[Zero, One, Other],
        }
    }
}

/// CLDR's `many` for the Romance languages: `e = 0 and i != 0 and
/// i % 1000000 = 0` -- one million, two million, and so on.
fn is_exact_million(n: u64) -> bool {
    n != 0 && n.is_multiple_of(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Category::*;

    /// A line of the published chart: `n -> category` for the counts a
    /// translator will actually meet, per rule.
    fn chart(rule: Rule, expected: &[(u64, Category)]) {
        for (n, want) in expected {
            assert_eq!(rule.category(*n), *want, "{rule:?} at n = {n}");
        }
    }

    #[test]
    fn the_rules_agree_with_the_published_chart() {
        chart(Rule::Other, &[(0, Other), (1, Other), (2, Other), (100, Other)]);
        chart(Rule::OneOther, &[(0, Other), (1, One), (2, Other), (21, Other)]);
        chart(
            Rule::OneManyOther,
            &[(0, Other), (1, One), (2, Other), (1_000_000, Many), (2_000_000, Many)],
        );
        chart(
            Rule::ZeroOneManyOther,
            &[(0, One), (1, One), (2, Other), (1_000_000, Many), (1_000_001, Other)],
        );
        chart(
            Rule::EastSlavic,
            &[
                (0, Many),
                (1, One),
                (2, Few),
                (4, Few),
                (5, Many),
                (11, Many),
                (12, Many),
                (14, Many),
                (21, One),
                (22, Few),
                (25, Many),
                (101, One),
                (111, Many),
                (112, Many),
            ],
        );
        chart(
            Rule::Polish,
            &[
                (0, Many),
                (1, One),
                (2, Few),
                (5, Many),
                (11, Many),
                (12, Many),
                (21, Many),
                (22, Few),
                (24, Few),
                (25, Many),
                (102, Few),
            ],
        );
        chart(
            Rule::CzechSlovak,
            &[(0, Other), (1, One), (2, Few), (4, Few), (5, Other), (21, Other)],
        );
        chart(
            Rule::SouthSlavic,
            &[(0, Other), (1, One), (2, Few), (5, Other), (11, Other), (21, One), (22, Few), (12, Other)],
        );
        chart(
            Rule::Slovenian,
            &[(0, Other), (1, One), (2, Two), (3, Few), (4, Few), (5, Other), (101, One), (102, Two)],
        );
        chart(
            Rule::Lithuanian,
            &[(0, Other), (1, One), (2, Few), (9, Few), (10, Other), (11, Other), (21, One), (22, Few)],
        );
        chart(
            Rule::Latvian,
            &[(0, Zero), (1, One), (2, Other), (10, Zero), (11, Zero), (19, Zero), (21, One), (22, Other)],
        );
        chart(
            Rule::Romanian,
            &[(0, Few), (1, One), (2, Few), (19, Few), (20, Other), (101, Other), (102, Few), (120, Other)],
        );
    }

    /// Every category a rule can answer is one it advertises, so a catalog
    /// that supplies `categories()` can never meet a form it lacks.
    #[test]
    fn a_rule_only_answers_with_categories_it_advertises() {
        for rule in [
            Rule::Other,
            Rule::OneOther,
            Rule::OneManyOther,
            Rule::ZeroOneManyOther,
            Rule::EastSlavic,
            Rule::Polish,
            Rule::CzechSlovak,
            Rule::SouthSlavic,
            Rule::Slovenian,
            Rule::Lithuanian,
            Rule::Latvian,
            Rule::Romanian,
        ] {
            let advertised = rule.categories();
            assert_eq!(advertised.last(), Some(&Other), "{rule:?} must end on `other`");
            for n in (0..2000u64).chain([1_000_000, 2_000_000, 1_000_001]) {
                assert!(
                    advertised.contains(&rule.category(n)),
                    "{rule:?} answers {:?} for {n} and does not advertise it",
                    rule.category(n)
                );
            }
        }
    }

    #[test]
    fn category_names_round_trip() {
        for c in [Zero, One, Two, Few, Many, Other] {
            assert_eq!(Category::from_name(c.name()), Some(c));
        }
        assert_eq!(Category::from_name("several"), None);
    }
}

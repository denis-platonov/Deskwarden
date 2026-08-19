//! A local, dependency-free password strength heuristic for the vault
//! window's detail-pane metadata strip ("Strength: strong"). Not a real
//! entropy estimator (that's what `zxcvbn` is for) -- just enough signal to
//! flag the two things a saved password can obviously get wrong: too short,
//! or drawn from only one character class.

/// Which of the four character types a password draws from.
///
/// **`pub`, and the one definition of "character type" in this crate.** The
/// vault window's password-health report has to tell the user *why* a
/// password was flagged -- "9 characters, lowercase letters only" is
/// something they can act on, and "Weak" on its own is not -- so it needs
/// the same four booleans [`rate`] decided on. A second copy of these four
/// predicates over there is the two-enumerations-that-must-agree defect this
/// crate keeps losing to: the health pane would go on explaining a rating
/// that had moved on without it. So [`rate`] calls this, and so does the
/// pane, and there is nothing to keep in step.
///
/// **No `Zeroize`, and none is needed:** four booleans about a password are
/// not a password. It is deliberately constructible only from a `&str` here,
/// so it cannot become a place a caller stashes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharClasses {
    pub lower: bool,
    pub upper: bool,
    pub digit: bool,
    /// Anything that is not an ASCII letter or digit -- punctuation, and
    /// also every non-ASCII character. That is deliberately generous rather
    /// than precise: an accented letter really does draw from an alphabet
    /// wider than `[a-z]`, and calling it a symbol overstates nothing a user
    /// could be harmed by.
    pub symbol: bool,
}

impl CharClasses {
    /// The classes `password` actually draws from.
    pub fn of(password: &str) -> Self {
        CharClasses {
            lower: password.chars().any(|c| c.is_ascii_lowercase()),
            upper: password.chars().any(|c| c.is_ascii_uppercase()),
            digit: password.chars().any(|c| c.is_ascii_digit()),
            symbol: password.chars().any(|c| !c.is_ascii_alphanumeric()),
        }
    }

    /// How many of the four are present: 0 only for an empty password, else
    /// 1 through 4.
    pub fn count(self) -> usize {
        [self.lower, self.upper, self.digit, self.symbol]
            .iter()
            .filter(|present| **present)
            .count()
    }

    /// The classes present, in a fixed order, as the nouns a sentence about
    /// this password would use.
    ///
    /// Empty for an empty password, which is why the health report excludes
    /// those before it ever gets here rather than printing a sentence with a
    /// hole in it.
    pub fn names(self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.lower {
            names.push("lowercase letters");
        }
        if self.upper {
            names.push("uppercase letters");
        }
        if self.digit {
            names.push("digits");
        }
        if self.symbol {
            names.push("symbols");
        }
        names
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Weak,
    Fair,
    Good,
    Strong,
}

impl Strength {
    pub fn label(self) -> &'static str {
        match self {
            Strength::Weak => "Weak",
            Strength::Fair => "Fair",
            Strength::Good => "Good",
            Strength::Strong => "Strong",
        }
    }
}

/// Scores on length plus character-class diversity (lower/upper/digit/
/// symbol, up to 4 classes). Short passwords are capped low regardless of
/// diversity -- a 6-character password with all four classes is still weak.
pub fn rate(password: &str) -> Strength {
    let len = password.chars().count();
    if len == 0 {
        return Strength::Weak;
    }

    let classes = CharClasses::of(password).count();

    if len < 8 {
        return Strength::Weak;
    }
    if len < 12 {
        return if classes >= 3 { Strength::Fair } else { Strength::Weak };
    }
    if len < 16 {
        return if classes >= 3 { Strength::Good } else { Strength::Fair };
    }
    if classes >= 3 {
        Strength::Strong
    } else {
        Strength::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_password_is_weak() {
        assert_eq!(rate(""), Strength::Weak);
    }

    #[test]
    fn short_password_is_weak_regardless_of_diversity() {
        assert_eq!(rate("Ab1!"), Strength::Weak);
    }

    #[test]
    fn long_single_class_password_is_not_strong() {
        assert_eq!(rate("aaaaaaaaaaaaaaaaaaaa"), Strength::Good);
    }

    #[test]
    fn long_diverse_password_is_strong() {
        assert_eq!(rate("Tr0ub4dor&3xtraLong!"), Strength::Strong);
    }

    #[test]
    fn medium_diverse_password_is_good() {
        assert_eq!(rate("Correct#Horse1"), Strength::Good);
    }

    /// Both directions on every one of the four flags, from the same call,
    /// so a predicate wired to the wrong `is_ascii_*` cannot pass by having
    /// the others happen to be right.
    #[test]
    fn each_class_is_detected_and_its_absence_reported() {
        let only_lower = CharClasses::of("abcdef");
        assert!(only_lower.lower, "lowercase letters were not seen at all");
        assert!(!only_lower.upper);
        assert!(!only_lower.digit);
        assert!(!only_lower.symbol);
        assert_eq!(only_lower.count(), 1);

        let all_four = CharClasses::of("aB1!");
        assert!(all_four.lower && all_four.upper && all_four.digit && all_four.symbol);
        assert_eq!(all_four.count(), 4);
    }

    #[test]
    fn an_empty_password_draws_from_no_class_at_all() {
        assert_eq!(CharClasses::of("").count(), 0);
        assert!(CharClasses::of("").names().is_empty());
    }

    /// The names are in the declared order and name exactly what is present
    /// -- asserted positively (what is there) and negatively (what is not),
    /// because a `names` that returned all four unconditionally would still
    /// satisfy a `contains` check.
    #[test]
    fn the_class_names_are_exactly_the_classes_present() {
        assert_eq!(CharClasses::of("abc123").names(), vec!["lowercase letters", "digits"]);
        assert_eq!(CharClasses::of("ABC!").names(), vec!["uppercase letters", "symbols"]);
        assert_eq!(
            CharClasses::of("aB1!").names(),
            vec!["lowercase letters", "uppercase letters", "digits", "symbols"]
        );
    }

    /// **The one definition, used twice.** `rate` decides on a class COUNT
    /// and the health pane explains the same password with the class NAMES;
    /// if the two ever came from different predicates, a password could be
    /// rated on three classes and explained with two. Asserted in both
    /// directions -- a password `rate` calls diverse and one it does not.
    #[test]
    fn rate_and_the_class_names_describe_the_same_password() {
        assert_eq!(CharClasses::of("Correct#Horse1").count(), 4);
        assert_eq!(rate("Correct#Horse1"), Strength::Good);

        assert_eq!(CharClasses::of("horsehorsehorse").count(), 1);
        assert_eq!(rate("horsehorsehorse"), Strength::Fair);
    }
}

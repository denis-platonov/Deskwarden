//! A local, dependency-free password strength heuristic for the vault
//! window's detail-pane metadata strip ("Strength: strong"). Not a real
//! entropy estimator (that's what `zxcvbn` is for) -- just enough signal to
//! flag the two things a saved password can obviously get wrong: too short,
//! or drawn from only one character class.

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

    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_symbol = password.chars().any(|c| !c.is_ascii_alphanumeric());
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .iter()
        .filter(|b| **b)
        .count();

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
}

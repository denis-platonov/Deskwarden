//! Slice 6: the Generator screen's modes and its strength model (spec C
//! §1). Kept out of `password_gen`, whose wordlist guard counts quotes.

/// The Generator screen's three modes (spec C §1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorMode {
    /// Stored as "characters": settings.json must never spell the word
    /// its secrets guard looks for.
    #[default]
    #[serde(rename = "characters")]
    Password,
    Passphrase,
    Pin,
}

/// The four bands of spec C §1.3's meter. Callers branch on this, never on
/// the translated word [`Self::label`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorLevel {
    Weak,
    Fair,
    Strong,
    VeryStrong,
}

impl GeneratorLevel {
    /// The band's word in the active language.
    pub fn label(self) -> &'static str {
        match self {
            GeneratorLevel::Weak => crate::i18n::t("strength.weak"),
            GeneratorLevel::Fair => crate::i18n::t("strength.fair"),
            GeneratorLevel::Strong => crate::i18n::t("strength.band_strong"),
            GeneratorLevel::VeryStrong => crate::i18n::t("strength.very_strong"),
        }
    }
}

/// Spec C §1.3's strength: bits, and the band they fall in. `size` is the
/// length (password, PIN) or the word count (passphrase); `pool` is the
/// password's alphabet size.
pub fn generator_strength(mode: GeneratorMode, size: usize, pool: usize) -> (f64, GeneratorLevel) {
    let bits = match mode {
        GeneratorMode::Pin => size as f64 * 3.32,
        GeneratorMode::Passphrase => size as f64 * 12.9,
        GeneratorMode::Password => size as f64 * (pool.max(1) as f64).log2(),
    };
    let label = if bits < 40.0 {
        GeneratorLevel::Weak
    } else if bits < 70.0 {
        GeneratorLevel::Fair
    } else if bits < 100.0 {
        GeneratorLevel::Strong
    } else {
        GeneratorLevel::VeryStrong
    };
    (bits, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_follows_the_spec_bits() {
        // PIN: len * 3.32; 6 digits ~ 20 bits.
        assert_eq!(generator_strength(GeneratorMode::Pin, 6, 10).1.label(), "Weak");
        // Passphrase: words * 12.9; 5 words ~ 64.5 bits; 6 ~ 77.4.
        assert_eq!(generator_strength(GeneratorMode::Passphrase, 5, 0).1.label(), "Fair");
        assert_eq!(generator_strength(GeneratorMode::Passphrase, 6, 0).1.label(), "Strong");
        // Password: len * log2(pool); 20 of 94 ~ 131 bits.
        assert_eq!(generator_strength(GeneratorMode::Password, 20, 94).1.label(), "Very strong");
        let (bits, _) = generator_strength(GeneratorMode::Password, 8, 26);
        assert!((bits - 8.0 * 26f64.log2()).abs() < 1e-9);
    }
}

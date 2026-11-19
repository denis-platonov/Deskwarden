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

/// Spec C §1.3's strength: bits, and the word for them. `size` is the
/// length (password, PIN) or the word count (passphrase); `pool` is the
/// password's alphabet size.
pub fn generator_strength(mode: GeneratorMode, size: usize, pool: usize) -> (f64, &'static str) {
    let bits = match mode {
        GeneratorMode::Pin => size as f64 * 3.32,
        GeneratorMode::Passphrase => size as f64 * 12.9,
        GeneratorMode::Password => size as f64 * (pool.max(1) as f64).log2(),
    };
    let label = if bits < 40.0 {
        "Weak"
    } else if bits < 70.0 {
        "Fair"
    } else if bits < 100.0 {
        "Strong"
    } else {
        "Very strong"
    };
    (bits, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_follows_the_spec_bits() {
        // PIN: len * 3.32; 6 digits ~ 20 bits.
        assert_eq!(generator_strength(GeneratorMode::Pin, 6, 10).1, "Weak");
        // Passphrase: words * 12.9; 5 words ~ 64.5 bits; 6 ~ 77.4.
        assert_eq!(generator_strength(GeneratorMode::Passphrase, 5, 0).1, "Fair");
        assert_eq!(generator_strength(GeneratorMode::Passphrase, 6, 0).1, "Strong");
        // Password: len * log2(pool); 20 of 94 ~ 131 bits.
        assert_eq!(generator_strength(GeneratorMode::Password, 20, 94).1, "Very strong");
        let (bits, _) = generator_strength(GeneratorMode::Password, 8, 26);
        assert!((bits - 8.0 * 26f64.log2()).abs() < 1e-9);
    }
}

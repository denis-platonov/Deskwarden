//! **The EFF long word list**, which Bitwarden's fingerprint phrase is drawn
//! from -- 7,776 words, the list every Bitwarden app uses, so the five words
//! Deskwarden shows for a person are the five words that person's own app shows
//! them under Account > Fingerprint phrase.
//!
//! # Why it is not `assets/wordlist.txt`
//!
//! That is the passphrase list, 4,096 words (`password_gen::WORDLIST_WORDS`).
//! A phrase is only comparable if both sides index the same list, and
//! Bitwarden's is EFF's. This is a second file, not a second use of the first.
//!
//! # Loaded the way `password_gen` loads its list, and for the same reasons
//!
//! Installed beside the executable rather than compiled in: it is needed only
//! when somebody presses **Show fingerprint**, which is rare. Counted,
//! de-duplicated and hashed on every load: a list that is not exactly EFF's
//! would produce a phrase that matches nobody's, and a phrase that looks right
//! and is not is the one failure a fingerprint exists to prevent. The hash is
//! over the parsed words joined by `\n`, not the file, so a CRLF checkout
//! verifies.

use std::collections::HashSet;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// The file name, beside the executable. The installer's `[Files]` section
/// writes it there; `the_installer_ships_the_eff_list_beside_the_executable`
/// holds the two spellings together.
pub const EFF_WORDLIST_FILE: &str = "eff_large_wordlist.txt";

/// How many words the list holds: 6^5, one per roll of five dice.
pub const EFF_WORDS: usize = 7776;

/// SHA-256 of the list's words joined by `\n`. Changing the list means
/// changing this in the same commit.
const EFF_WORDLIST_SHA256: [u8; 32] = [
    0xab, 0xae, 0x49, 0x76, 0x1b, 0x88, 0xf3, 0xf1, 0xba, 0x31, 0xef, 0x94, 0x4b, 0xea, 0x1f,
    0x61, 0xb7, 0x95, 0xa3, 0xcd, 0x7e, 0x1c, 0xfb, 0x7d, 0x27, 0x6e, 0xd4, 0x5b, 0xf7, 0x79,
    0x67, 0xba,
];

/// The list, verified.
pub struct EffLongList {
    words: Vec<String>,
}

impl EffLongList {
    /// The word at `index`. Callers index with a value `< EFF_WORDS`, which
    /// [`verify`] guarantees is in range.
    #[must_use]
    pub fn word(&self, index: usize) -> &str {
        &self.words[index]
    }

    /// Always [`EFF_WORDS`] for a verified list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Never true for a verified list; here because `len` without it is a
    /// clippy warning.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Why a list could not be used. Three causes, for `password_gen`'s reason:
/// they need three different sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffListError {
    /// No file where one is looked for.
    Missing,
    /// A file that is not the EFF list: wrong count, a duplicate, a malformed
    /// word, or a different digest.
    Unusable,
    /// A file that could not be read (held open by an updater, say).
    Unreadable,
}

impl std::fmt::Display for EffListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "The fingerprint word list is missing. Reinstall Deskwarden.",
            Self::Unusable => "The fingerprint word list is not the one Deskwarden ships. Reinstall Deskwarden.",
            Self::Unreadable => "The fingerprint word list could not be read. Try again.",
        })
    }
}

/// Where the list is looked for, in order: beside the executable, then in
/// `assets/` beside it -- `password_gen::wordlist_paths`' two places, and for
/// its reason never the user's config directory.
#[must_use]
pub fn eff_wordlist_paths() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from))
    else {
        return Vec::new();
    };
    vec![dir.join(EFF_WORDLIST_FILE), dir.join("assets").join(EFF_WORDLIST_FILE)]
}

/// The installed list, read and verified now and dropped by the caller.
///
/// # Errors
///
/// See [`EffListError`].
pub fn load() -> Result<EffLongList, EffListError> {
    load_from(&eff_wordlist_paths())
}

/// [`load`] over paths the caller names -- split out, as `password_gen`'s is,
/// so a test can reach it without moving the test binary.
///
/// # Errors
///
/// See [`EffListError`].
pub fn load_from(paths: &[PathBuf]) -> Result<EffLongList, EffListError> {
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(path).map_err(|_| EffListError::Unreadable)?;
        return verify(&text);
    }
    Err(EffListError::Missing)
}

/// The checks, over the text. EFF's lines are `11111\tabacus`: the dice
/// column is read and dropped, and a bare word is accepted too.
///
/// # Errors
///
/// [`EffListError::Unusable`] for anything that is not exactly the EFF list.
pub fn verify(text: &str) -> Result<EffLongList, EffListError> {
    let words: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().last().map(str::to_string))
        .collect();
    if words.len() != EFF_WORDS {
        return Err(EffListError::Unusable);
    }
    // EFF's words are lowercase ASCII, four carry a hyphen (`drop-down`,
    // `felt-tip`, `t-shirt`, `yo-yo`), none is longer than nine characters.
    let well_formed = words.iter().all(|w| {
        (1..=9).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    });
    if !well_formed {
        return Err(EffListError::Unusable);
    }
    if words.iter().map(String::as_str).collect::<HashSet<_>>().len() != EFF_WORDS {
        return Err(EffListError::Unusable);
    }
    let mut hasher = Sha256::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(word.as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if digest != EFF_WORDLIST_SHA256 {
        return Err(EffListError::Unusable);
    }
    Ok(EffLongList { words })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/eff_large_wordlist.txt"))
            .expect("the committed EFF list")
    }

    #[test]
    fn the_committed_list_is_the_eff_long_list() {
        let list = verify(&committed()).expect("the committed list verifies");
        assert_eq!(list.len(), EFF_WORDS);
        assert_eq!(list.word(0), "abacus");
        assert_eq!(list.word(EFF_WORDS - 1), "zoom");
    }

    #[test]
    fn a_short_or_altered_list_is_refused() {
        let text = committed();
        let short: String = text.lines().take(7000).map(|l| format!("{l}\n")).collect();
        assert_eq!(verify(&short).err(), Some(EffListError::Unusable));
        let altered = text.replacen("abacus", "abacas", 1);
        assert_eq!(verify(&altered).err(), Some(EffListError::Unusable));
        let crlf = text.replace('\n', "\r\n");
        assert!(verify(&crlf).is_ok(), "the list must verify under either line ending");
    }

    #[test]
    fn a_missing_list_is_missing_and_not_unusable() {
        let nowhere = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-dir/x.txt"));
        assert_eq!(load_from(&[nowhere]).err(), Some(EffListError::Missing));
    }

    #[test]
    fn the_installer_ships_the_eff_list_beside_the_executable() {
        let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/installer/deskwarden.iss"))
            .expect("the installer script");
        let line = script
            .lines()
            .map(str::trim)
            .find(|l| !l.starts_with(';') && l.contains(EFF_WORDLIST_FILE))
            .expect("the installer does not ship the EFF list");
        assert!(line.starts_with("Source:"), "`{line}` is not a [Files] entry");
        assert!(line.contains(r"..\assets\eff_large_wordlist.txt"), "`{line}`");
        assert!(line.contains(r#"DestDir: "{app}""#), "`{line}`");
    }

    #[test]
    fn the_build_script_copies_the_eff_list_beside_the_executable() {
        let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).expect("build.rs");
        assert!(build.contains("\"eff_large_wordlist.txt\""), "build.rs no longer copies the EFF list");
    }
}

//! The credential a program must present to read the vault over HTTP.
//!
//! # What this stops, and what it does not
//!
//! `docs/superpowers/plans/2026-08-27-the-local-vault-service.md` takes the
//! decision this module implements, and the limit belongs here where anyone
//! reading the code will meet it:
//!
//! **It stops** another user on the machine, and anything that reaches
//! loopback without being able to read this user's files, from getting the
//! vault by connecting to a port. That is what `bw serve` does not stop, and
//! it is the reason this exists.
//!
//! **It does not stop** a program already running as the owner. The token
//! file is DPAPI-wrapped, so it unwraps under this user's credentials -- and
//! so does anything else that user runs. This is the same limit
//! [`crate::session_store`] and [`crate::user_key_store`] already have, and
//! it is the strongest thing available without a per-client credential the
//! owner would have to manage by hand.
//!
//! Saying so is the point. A reader who believes this is an authorisation
//! boundary between programs on one desktop would be wrong, and would build
//! something on top of that belief.
//!
//! # Why the comparison is constant time
//!
//! An attacker who can time a request can otherwise recover the token one
//! character at a time, and this endpoint is not rate-limited by anything.
//! [`matches`] therefore reads every byte of both values before it answers.

/// The service's bearer token.
///
/// **Deliberately not `Debug`.** A token that can be printed will be, into a
/// log the owner then pastes into a bug report --
/// [`crate::debug_leak_guard`] exists because that has happened in this crate
/// before. Reading it out is [`Token::expose`], which is one grep away for a
/// reviewer.
pub struct Token(String);

impl Token {
    /// The token as the text a client sends.
    ///
    /// **The one way out, and named so it is visible.** Two callers may use
    /// it: the code that writes the token file, and the code that compares a
    /// presented credential. Anything else handing this string onward is
    /// widening what holds the secret.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Mints a token from 32 random bytes, hex-encoded.
///
/// The randomness is a parameter rather than a call, for the reason every
/// `fn`-pointer seam in this crate exists: the encoding, the length and the
/// comparison are all worth driving from a test, and none of them should
/// need an unpredictable value to be driven.
///
/// 32 bytes because this is a bearer credential with no rate limit in front
/// of it: it has to be far past guessing on its own.
#[must_use]
pub fn mint(random: fn() -> [u8; 32]) -> Token {
    let bytes = random();
    let mut out = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Token(out)
}

/// The crate's real randomness, for production callers.
///
/// Panics if the OS cannot supply random bytes. That is correct here and is
/// not a judgement call: a token minted from a failed draw would be a
/// predictable credential in front of a decrypted vault, and refusing to
/// start is the only safe answer. [`crate::accounts`] takes the same view of
/// the same call.
#[must_use]
pub fn os_random() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the OS could not supply random bytes for a token");
    bytes
}

/// Whether `presented` is the token, compared in constant time.
///
/// Both the length check and the byte comparison avoid an early return: a
/// function that returned as soon as it found a wrong byte would leak the
/// position of that byte through its own running time, which is enough to
/// recover the whole value one character at a time.
#[must_use]
pub fn matches(expected: &Token, presented: &str) -> bool {
    constant_time_eq(expected.0.as_bytes(), presented.as_bytes())
}

/// Two stored hashes, compared in constant time.
///
/// [`crate::service_keys::find`] runs this on an unauthenticated request,
/// once per stored key. A `==` there would stop at the first differing byte
/// and leak a stored hash one character at a time, which is the same attack
/// [`matches`] exists to refuse -- so it is the same comparison, not a second
/// one written slightly differently.
#[must_use]
pub fn hashes_match(stored: &str, computed: &str) -> bool {
    constant_time_eq(stored.as_bytes(), computed.as_bytes())
}

/// The comparison itself. One implementation, so there is one thing to get
/// right and one thing to pin.
fn constant_time_eq(expected: &[u8], presented: &[u8]) -> bool {
    // Fold the length difference into the same accumulator rather than
    // returning on it, so a wrong-length guess costs what a right-length one
    // does.
    let mut difference = (expected.len() ^ presented.len()) as u8;
    for (index, ours) in expected.iter().enumerate() {
        // Past the end of `presented` this reads a fixed byte rather than
        // stopping, which keeps the loop's length a function of the expected
        // token only -- never of the guess.
        let theirs = presented.get(index).copied().unwrap_or(0);
        difference |= ours ^ theirs;
    }
    difference == 0
}

/// The token out of an `Authorization` header, if it is a bearer one.
///
/// Pure, and strict: exactly `Bearer ` and then the value. A parser that
/// accepted `bearer` or a missing space would be a parser whose behaviour a
/// caller has to guess at.
#[must_use]
pub fn bearer_of(header: Option<&str>) -> Option<&str> {
    header?.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> [u8; 32] {
        [7u8; 32]
    }

    fn other() -> [u8; 32] {
        [8u8; 32]
    }

    #[test]
    fn the_right_token_is_accepted() {
        let expected = mint(fixed);
        let presented = mint(fixed);
        assert!(matches(&expected, presented.expose()));
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let expected = mint(fixed);
        assert!(!matches(&expected, mint(other).expose()));
        assert!(!matches(&expected, "not-the-token"));
        assert!(!matches(&expected, ""));
    }

    /// A prefix must not be accepted. This is the shape a timing attack
    /// builds towards, so it is refused explicitly rather than incidentally.
    #[test]
    fn a_prefix_of_the_token_is_refused() {
        let expected = mint(fixed);
        let full = mint(fixed).expose().to_string();
        assert!(!matches(&expected, &full[..full.len() - 1]));
        assert!(!matches(&expected, &full[..1]));
    }

    /// And so is a value that merely starts with the token.
    #[test]
    fn a_longer_value_that_starts_with_the_token_is_refused() {
        let expected = mint(fixed);
        let full = mint(fixed).expose().to_string();
        assert!(!matches(&expected, &format!("{full}extra")));
    }

    /// 32 bytes, hex: the length is part of the credential's strength and is
    /// not left to be inferred from the implementation.
    #[test]
    fn a_token_is_sixty_four_hex_characters() {
        let token = mint(fixed);
        assert_eq!(token.expose().len(), 64);
        assert!(token.expose().chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The encoding must depend on every byte. `[7; 32]` and `[8; 32]`
    /// differing proves only that it reads one; this proves it reads the
    /// last one too.
    #[test]
    fn every_byte_of_the_draw_reaches_the_token() {
        fn last_byte_differs() -> [u8; 32] {
            let mut bytes = [7u8; 32];
            bytes[31] = 9;
            bytes
        }
        assert_ne!(mint(fixed).expose(), mint(last_byte_differs).expose());
    }

    #[test]
    fn only_a_bearer_header_yields_a_token() {
        assert_eq!(bearer_of(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_of(Some("Basic abc")), None);
        assert_eq!(bearer_of(Some("Bearerabc")), None);
        assert_eq!(bearer_of(Some("bearer abc")), None);
        assert_eq!(bearer_of(None), None);
    }

    /// The house guard: a secret that can be printed will be.
    #[test]
    fn the_token_type_does_not_derive_debug() {
        let source = include_str!("service_token.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        assert!(
            production.contains("pub struct Token"),
            "control: the type is gone, so this guard is vacuous"
        );
        assert!(
            !production.contains("#[derive(Debug"),
            "something here derives Debug. A token that can be printed reaches a log, and the log reaches a bug report."
        );
    }

    /// The comparison must not short-circuit. An absence cannot be read, so
    /// it is pinned: no `return` inside `matches`.
    #[test]
    fn the_comparison_has_no_early_return() {
        let source = include_str!("service_token.rs");
        let start = source.find("fn constant_time_eq(").expect("control: the comparison is gone");
        let body = &source[start..];
        let end = body.find("\n}").expect("control: could not find the end of the comparison");
        let body = &body[..end];
        // Comments are prose and must not be searched: the first version of
        // this pin failed because a comment in `matches` used the word
        // "returning" to explain why it does not return.
        let code: String = body
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("difference"),
            "control: this is not the body of the comparison"
        );
        assert!(
            !code.contains("return"),
            "the comparison has an early return; the time it takes now depends on where the first wrong byte is, which is enough to recover the token one character at a time"
        );
    }
}

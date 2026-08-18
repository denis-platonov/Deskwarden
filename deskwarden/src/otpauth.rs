//! `otpauth://totp/...` URIs: parsing one, and rendering one back out.
//!
//! # Why the whole URI, and not the bare seed
//!
//! Bitwarden's `totp` field accepts a full `otpauth://` URI, and the vault
//! computes the code from whatever is in it. A card that says **8 digits over
//! 60 seconds under SHA-256** and is stored as its seed alone generates codes
//! that are confidently wrong -- six digits, every thirty seconds, correctly
//! formatted, accepted by nothing, with nothing on screen to explain it. So
//! the parameters are *read*, kept, and written back out by [`to_uri`]; they
//! are never assumed.
//!
//! # The payload is untrusted
//!
//! Anyone who can talk a user into scanning a QR code can hand them a hostile
//! one, so this module is written as a parser of hostile input:
//!
//! * **Unknown query parameters are refused, not ignored.** Ignoring a key
//!   nobody recognises is how a payload written for something else imports
//!   silently and wrongly -- the reader sees a saved code and the code is
//!   never right.
//! * **`otpauth://hotp/...` is refused by name**, and so is a plain URL. A
//!   refusal naming the reason is the difference between the user fixing the
//!   problem and the user retrying forever.
//! * **Nothing in the URI is treated as a URL to fetch, a path or a
//!   command.** This module does no I/O of any kind; every function in it is
//!   pure, and that is the whole reason the decoding half is lifted out of the
//!   capture that produces it.
//!
//! # The secret
//!
//! [`OtpAuth::secret`] is a [`Zeroizing`], as `login.totp` already is on the
//! vault side, so every copy of it wipes itself on drop. Everything this
//! module builds *from* it -- the decoded value, the normalised value, the
//! rendered URI -- is a `Zeroizing` too, and each is allocated **once, at its
//! final capacity**: a `String` that grows leaves its old buffer behind
//! unwiped, and `Zeroizing` only ever wipes the buffer it is holding when it
//! drops. `breach.rs` makes the same move for the same reason.

use std::fmt;

use zeroize::Zeroizing;

/// The HMAC a TOTP code is computed under.
///
/// A plain enum holding nothing: it is derived from a URI and never holds one,
/// so it cannot reach a [`Zeroizing`] and a derived `Debug` on it leaks
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    /// The spelling every other authenticator uses in the `algorithm`
    /// parameter. Not a display choice: [`to_uri`] writes this, and a URI
    /// spelled `Sha256` is one a different client may not read back.
    pub fn canonical(self) -> &'static str {
        match self {
            Algorithm::Sha1 => "SHA1",
            Algorithm::Sha256 => "SHA256",
            Algorithm::Sha512 => "SHA512",
        }
    }
}

/// A parsed, validated `otpauth://totp` URI.
///
/// `PartialEq` is derived and `Debug` is **not** -- see the hand-written impl
/// below for why.
#[derive(Clone, PartialEq, Eq)]
pub struct OtpAuth {
    /// The service, from the `issuer` parameter or from the label's prefix.
    pub issuer: Option<String>,
    /// The account at that service, from the label.
    pub account: Option<String>,
    /// The seed: base32, uppercase, unpadded, no whitespace.
    pub secret: Zeroizing<String>,
    pub algorithm: Algorithm,
    /// 6 or 8. Nothing else is accepted; see [`parse_otpauth`].
    pub digits: u8,
    /// The step, in seconds. Non-zero.
    pub period: u16,
}

/// A stand-in for a value that must not be printed, carrying only its length.
///
/// Named for what it is rather than `Redacted`, which `send.rs` already has:
/// `debug_leak_guard` matches types by bare name across files, and two
/// unrelated types sharing one are conflated in its report.
struct SecretLen(usize);

impl fmt::Debug for SecretLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes redacted>", self.0)
    }
}

/// Hand-written so the seed never reaches a formatter.
///
/// `debug_leak_guard` refuses a derived `Debug` on any type whose body
/// mentions a [`Zeroizing`], and it is right to: `Zeroizing<T>`'s own `Debug`
/// prints the inner value, so a derived impl here would print the seed into
/// whatever log or panic message asked for it.
///
/// `issuer` and `account` are kept. They are already on screen, they are the
/// only thing that tells two of these apart, and neither is a secret -- the
/// seed is.
impl fmt::Debug for OtpAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtpAuth")
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .field("secret", &SecretLen(self.secret.len()))
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .finish()
    }
}

/// Why a string is not a one-time code, phrased so a caller can say which.
///
/// Every variant is a *different sentence* on screen. A single `Err(())` is
/// what teaches a user to retry the thing that will not work.
///
/// Holds no [`Zeroizing`] and no fragment of a seed -- [`Self::BadSecret`]
/// deliberately carries nothing, because the thing that was wrong with it is
/// the thing that must not be printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtpRefusal {
    /// Not an `otpauth://` URI at all -- a plain URL, or plain text.
    NotOtpAuth,
    /// An `otpauth://` URI of some other type. `hotp` is the one seen in the
    /// wild: a counter-based code, which this app cannot advance.
    NotTotp,
    /// No `secret` parameter, or an empty one.
    NoSecret,
    /// A `secret` that is not base32.
    BadSecret,
    /// A query parameter this module does not know. Carries the key **as it
    /// was written**, so the sentence can name it.
    UnknownParameter(String),
    /// A known parameter with an unusable value. Carries the parameter's name.
    BadParameter(&'static str),
    /// Longer than [`MAX_URI_LEN`].
    TooLong,
}

/// The longest input [`parse_otpauth`] will look at.
///
/// A bound, not a format rule. A real `otpauth://` URI is a couple of hundred
/// bytes; this is generous by an order of magnitude and exists so that a
/// decoder handing back a megabyte of text -- a QR code can carry a few
/// kilobytes, and a file-backed one is unbounded -- is refused by name rather
/// than percent-decoded byte by byte.
pub const MAX_URI_LEN: usize = 4096;

/// The scheme, matched case-insensitively.
const SCHEME: &str = "otpauth://";

/// Parses an `otpauth://totp` URI, or says why it is not one.
///
/// Strict on purpose; see this module's documentation. The shape is:
///
/// ```text
/// otpauth://totp/ISSUER:ACCOUNT?secret=..&issuer=..&algorithm=..&digits=..&period=..
/// ```
///
/// Both halves of the label are optional, as is every parameter except
/// `secret`. Unstated parameters take the RFC 6238 defaults -- SHA-1, six
/// digits, thirty seconds -- and **only** because those are the defaults every
/// other client applies to the same URI, so reading them here and there give
/// the same code.
///
/// When `issuer` appears both as a parameter and as the label's prefix and the
/// two disagree, the **parameter** wins. RFC 6238's appendix says they should
/// be equal and does not say what to do when they are not; refusing would
/// reject cards that are merely sloppy, and the parameter is the one every
/// other client reads.
pub fn parse_otpauth(text: &str) -> Result<OtpAuth, OtpRefusal> {
    if text.len() > MAX_URI_LEN {
        return Err(OtpRefusal::TooLong);
    }
    let trimmed = text.trim();

    // The scheme, case-insensitively. `OTPAUTH://` is a legal spelling of the
    // same URI and a few desktop authenticators emit it.
    if trimmed.len() < SCHEME.len() || !trimmed[..SCHEME.len()].eq_ignore_ascii_case(SCHEME) {
        return Err(OtpRefusal::NotOtpAuth);
    }
    let rest = &trimmed[SCHEME.len()..];

    let (path, query) = match rest.find('?') {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, ""),
    };

    // The type is the first path segment. `hotp` is a real thing that a real
    // site will hand out, and it is refused by name rather than mis-read as a
    // TOTP whose codes would never match.
    let (kind, label) = match path.find('/') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => (path, ""),
    };
    if !kind.eq_ignore_ascii_case("totp") {
        return Err(OtpRefusal::NotTotp);
    }

    let (label_issuer, account) = split_label(label);

    let mut secret_raw: Option<&str> = None;
    let mut param_issuer: Option<String> = None;
    let mut algorithm: Option<Algorithm> = None;
    let mut digits: Option<u8> = None;
    let mut period: Option<u16> = None;

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.find('=') {
            Some(at) => (&pair[..at], &pair[at + 1..]),
            // A bare flag. There is no valid one in this URI shape, so it is
            // an unknown key rather than a key with an empty value.
            None => (pair, ""),
        };

        // Matched case-insensitively, reported as written: the sentence on
        // screen should quote what the user's payload actually said.
        match key.to_ascii_lowercase().as_str() {
            "secret" => {
                if secret_raw.replace(value).is_some() {
                    return Err(OtpRefusal::BadParameter("secret"));
                }
            }
            "issuer" => {
                if param_issuer.replace(percent_decoded(value).to_string()).is_some() {
                    return Err(OtpRefusal::BadParameter("issuer"));
                }
            }
            "algorithm" => {
                let parsed = match percent_decoded(value).to_ascii_uppercase().as_str() {
                    "SHA1" => Algorithm::Sha1,
                    "SHA256" => Algorithm::Sha256,
                    "SHA512" => Algorithm::Sha512,
                    _ => return Err(OtpRefusal::BadParameter("algorithm")),
                };
                if algorithm.replace(parsed).is_some() {
                    return Err(OtpRefusal::BadParameter("algorithm"));
                }
            }
            "digits" => {
                // 6 and 8 are the only two any authenticator emits, and a
                // stored 7 would produce codes no site accepts.
                let parsed = match value {
                    "6" => 6u8,
                    "8" => 8u8,
                    _ => return Err(OtpRefusal::BadParameter("digits")),
                };
                if digits.replace(parsed).is_some() {
                    return Err(OtpRefusal::BadParameter("digits"));
                }
            }
            "period" => {
                let parsed: u16 =
                    value.parse().map_err(|_| OtpRefusal::BadParameter("period"))?;
                // Zero is not a step; it is a division by zero in whoever
                // computes the code.
                if parsed == 0 {
                    return Err(OtpRefusal::BadParameter("period"));
                }
                if period.replace(parsed).is_some() {
                    return Err(OtpRefusal::BadParameter("period"));
                }
            }
            _ => return Err(OtpRefusal::UnknownParameter(key.to_string())),
        }
    }

    let Some(secret_raw) = secret_raw else {
        return Err(OtpRefusal::NoSecret);
    };
    let secret = normalise_secret(secret_raw)?;

    Ok(OtpAuth {
        issuer: param_issuer.or(label_issuer),
        account,
        secret,
        algorithm: algorithm.unwrap_or(Algorithm::Sha1),
        digits: digits.unwrap_or(6),
        period: period.unwrap_or(30),
    })
}

/// Splits `ISSUER:ACCOUNT` into its two percent-decoded halves.
///
/// The separator may be a literal `:` or the `%3A` an encoder that escaped the
/// whole label would have written; both are seen in the wild, and a reader
/// that knows only one of them turns `Git Host:anovak` into an account called
/// "Git Host:anovak" with no issuer.
fn split_label(label: &str) -> (Option<String>, Option<String>) {
    // A leading `/` from `otpauth://totp//x` or from a label written with one.
    let label = label.trim_start_matches('/');
    if label.is_empty() {
        return (None, None);
    }

    let colon = label.find(':');
    let escaped = label.to_ascii_lowercase().find("%3a");
    // A LITERAL colon wins wherever both spellings appear, rather than the
    // leftmost of the two. `to_uri` percent-encodes each half and then writes
    // a literal `:`, so an issuer that itself contains a colon renders as
    // `Git%3AHost:anovak` -- where the leftmost separator-looking thing is
    // INSIDE the issuer. Splitting there renames the account, silently.
    let split = match (colon, escaped) {
        (Some(a), _) => Some((a, 1)),
        (None, Some(b)) => Some((b, 3)),
        (None, None) => None,
    };

    match split {
        Some((at, width)) => {
            let issuer = non_empty(percent_decoded(&label[..at]).to_string());
            let account = non_empty(percent_decoded(&label[at + width..]).to_string());
            (issuer, account)
        }
        None => (None, non_empty(percent_decoded(label).to_string())),
    }
}

/// `None` for an empty or whitespace-only string, so an absent label half and
/// an empty one are the same thing to every caller.
fn non_empty(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Percent-decoding, plus `+` for a space.
///
/// **Returns a [`Zeroizing`] because the same function decodes the `secret`
/// parameter.** The issuer and the account are not secrets and would not need
/// one; the seed is, and one decoder that always wipes is safer than two whose
/// call sites have to be right.
///
/// **Decoded as bytes, and validated as UTF-8 once at the end.** A percent
/// escape names a *byte*, and one non-ASCII character is spelled as two or
/// three of them -- `Bücher` arrives as `B%C3%BCcher`. A decoder that turned
/// each escape into a `char` would produce `B\u{c3}\u{bc}cher`, which round-
/// trips to a different issuer than the one the user confirmed.
///
/// The `Vec` is allocated once at `raw.len()`, which is an upper bound because
/// decoding only ever shortens, and `String::from_utf8` **reuses that same
/// allocation** rather than copying -- so exactly one buffer ever holds the
/// decoded seed, and `Zeroizing` wipes it. A buffer that grew would have
/// handed an un-wiped copy of everything decoded so far back to the allocator,
/// where `Zeroizing` cannot reach it.
///
/// An invalid escape (`%zz`, or a `%` at the end) is passed through
/// **literally** rather than refused. This is not a URL that will be fetched;
/// the only thing downstream of a mangled issuer is a label on screen, and the
/// only thing downstream of a mangled secret is [`normalise_secret`], which
/// refuses anything that is not base32 -- so a `%` surviving into a seed is
/// caught there, by name.
fn percent_decoded(raw: &str) -> Zeroizing<String> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                // `from_str_radix` on two ASCII hex digits. It also accepts a
                // leading `+`/`-`, which cannot appear here: both bytes are
                // checked to be hex digits first.
                let hex = &raw[i + 1..i + 3];
                match hex_pair(hex) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    match String::from_utf8(out) {
        Ok(text) => Zeroizing::new(text),
        // Escapes that spell no valid character. Rebuilt at an exact upper
        // bound rather than through `String::from_utf8_lossy(..).into_owned()`,
        // which grows a buffer as it goes and would strew fragments of
        // whatever it was decoding across the allocator. The result is not
        // base32, so a seed that lands here is refused by name.
        Err(bad) => {
            let bad = Zeroizing::new(bad.into_bytes());
            let mut lossy = Zeroizing::new(String::with_capacity(3 * bad.len()));
            for byte in bad.iter() {
                if byte.is_ascii() {
                    lossy.push(*byte as char);
                } else {
                    lossy.push(char::REPLACEMENT_CHARACTER);
                }
            }
            lossy
        }
    }
}

/// Two ASCII hex digits as a byte, or `None` if they are not both hex.
fn hex_pair(hex: &str) -> Option<u8> {
    let mut bytes = hex.bytes();
    let high = (bytes.next()? as char).to_digit(16)?;
    let low = (bytes.next()? as char).to_digit(16)?;
    Some((high * 16 + low) as u8)
}

/// The base32 alphabet, RFC 4648, uppercase. No `0`, `1` or `8`.
const BASE32_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Normalises a `secret` parameter value, or refuses it.
///
/// Uppercased, spaces and hyphens dropped (sites print the seed in groups of
/// four for a human to type), `=` padding dropped. What is left must be
/// entirely base32; anything else is [`OtpRefusal::BadSecret`], and the
/// refusal carries **nothing** of what was wrong, because the thing that was
/// wrong is the thing that must not be printed.
///
/// Both allocations are made once at an exact upper bound, for
/// [`percent_decoded`]'s reason.
fn normalise_secret(raw: &str) -> Result<Zeroizing<String>, OtpRefusal> {
    let decoded = percent_decoded(raw);
    let mut out = Zeroizing::new(String::with_capacity(decoded.len()));
    for ch in decoded.chars() {
        match ch {
            ' ' | '-' | '\t' | '=' => continue,
            _ => {
                let upper = ch.to_ascii_uppercase();
                if !BASE32_ALPHABET.contains(upper) {
                    return Err(OtpRefusal::BadSecret);
                }
                out.push(upper);
            }
        }
    }
    if out.is_empty() {
        return Err(OtpRefusal::NoSecret);
    }
    Ok(out)
}

/// The characters a label or an issuer may carry unescaped: RFC 3986's
/// unreserved set.
fn is_unreserved(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~')
}

/// Percent-encodes everything outside [`is_unreserved`], appending to `out`.
///
/// Deliberately conservative: `:` and `/` are escaped too, so an issuer
/// containing one cannot invent a second label separator or a second path
/// segment in the URI this builds.
fn push_encoded(out: &mut String, text: &str) {
    for ch in text.chars() {
        if is_unreserved(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push(char::from_digit((byte >> 4) as u32, 16).expect("nibble").to_ascii_uppercase());
                out.push(char::from_digit((byte & 0xf) as u32, 16).expect("nibble").to_ascii_uppercase());
            }
        }
    }
}

/// Renders an [`OtpAuth`] back to a URI. **This is what is written to the
/// item's `totp` field.**
///
/// Every parameter is written out, including the ones that happen to equal the
/// RFC defaults. That is three redundant bytes and one guarantee: the value
/// stored in the vault says what it means, so a client whose defaults ever
/// differ, or a future reader of this crate's own, reads the same code the
/// user confirmed on screen.
///
/// `Zeroizing`, and built in one allocation at a capacity computed to be an
/// upper bound -- see [`percent_decoded`] for what a re-allocation would cost.
pub fn to_uri(a: &OtpAuth) -> Zeroizing<String> {
    let issuer = a.issuer.as_deref().unwrap_or("");
    let account = a.account.as_deref().unwrap_or("");

    // Upper bound, not an estimate: percent-encoding is at most three bytes
    // per input byte, the fixed text below is 70 bytes at the very most, and
    // the secret is copied verbatim. Over-reserving costs a few unused bytes;
    // under-reserving costs an un-wiped copy of the seed.
    let capacity = 96 + a.secret.len() + 3 * (issuer.len() + account.len()) * 2;
    let mut out = Zeroizing::new(String::with_capacity(capacity));

    out.push_str(SCHEME);
    out.push_str("totp/");
    if !issuer.is_empty() {
        push_encoded(&mut out, issuer);
        if !account.is_empty() {
            out.push(':');
        }
    }
    push_encoded(&mut out, account);

    out.push_str("?secret=");
    out.push_str(&a.secret);
    if !issuer.is_empty() {
        out.push_str("&issuer=");
        push_encoded(&mut out, issuer);
    }
    out.push_str("&algorithm=");
    out.push_str(a.algorithm.canonical());
    out.push_str("&digits=");
    out.push_str(if a.digits == 8 { "8" } else { "6" });
    out.push_str("&period=");
    // Two allocations avoided: `u16::to_string` would make one, and it would
    // be a plain `String` sitting beside a seed-bearing one.
    push_u16(&mut out, a.period);

    debug_assert!(
        out.len() <= capacity,
        "to_uri re-allocated, which leaves an un-wiped copy of the seed behind"
    );
    out
}

/// Writes a `u16` in decimal without allocating.
fn push_u16(out: &mut String, mut value: u16) {
    if value == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 5];
    let mut n = 0;
    while value > 0 {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        out.push(digits[i] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_url_is_refused_by_name() {
        // 6d's failure case: "That QR isn't a one-time code. It decoded to a
        // plain URL." A refusal that names the reason is the difference
        // between the user fixing it and the user retrying forever.
        assert_eq!(parse_otpauth("https://example.com"), Err(OtpRefusal::NotOtpAuth));
        assert_eq!(
            parse_otpauth("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP"),
            Err(OtpRefusal::NotTotp)
        );
        // Not a URI at all -- the other thing a QR in the wild decodes to.
        assert_eq!(parse_otpauth("WIFI:S=home;T=WPA;P=hunter2;;"), Err(OtpRefusal::NotOtpAuth));
        assert_eq!(parse_otpauth(""), Err(OtpRefusal::NotOtpAuth));
        // Control: the two refusals above are about the SCHEME and the TYPE
        // and not about the rest of the string, which is byte-identical here.
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").is_ok());
    }

    #[test]
    fn the_parameters_are_read_and_not_assumed() {
        // The whole reason the URI is stored rather than the bare seed: a card
        // that specifies 8 digits over 60 seconds generates confidently wrong
        // codes if the parameters are dropped.
        let a = parse_otpauth(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host&digits=8&period=60&algorithm=SHA256"
        ).unwrap();
        assert_eq!(a.issuer.as_deref(), Some("Git Host"));
        assert_eq!(a.account.as_deref(), Some("anovak"));
        assert_eq!(a.digits, 8);
        assert_eq!(a.period, 60);
        assert_eq!(a.algorithm, Algorithm::Sha256);
        assert_eq!(a.secret.as_str(), "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn the_defaults_are_the_rfc_defaults_when_unstated() {
        let a = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!((a.digits, a.period, a.algorithm), (6, 30, Algorithm::Sha1));
        // Paired: the defaults are DEFAULTS and not constants. The same three
        // fields read differently from a URI that states them.
        let stated = parse_otpauth(
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=8&period=60&algorithm=SHA512",
        )
        .unwrap();
        assert_eq!((stated.digits, stated.period, stated.algorithm), (8, 60, Algorithm::Sha512));
    }

    #[test]
    fn an_unknown_parameter_is_refused_rather_than_ignored() {
        // Untrusted input. Ignoring unknown keys is how a payload written for
        // something else imports silently and wrongly.
        assert_eq!(
            parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&surprise=1"),
            Err(OtpRefusal::UnknownParameter("surprise".to_string()))
        );
        // The key is reported AS WRITTEN, so the sentence on screen quotes the
        // payload rather than a normalised guess at it.
        assert_eq!(
            parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&Counter=7"),
            Err(OtpRefusal::UnknownParameter("Counter".to_string()))
        );
        // A bare flag with no `=` is an unknown key, not a known one that is
        // empty -- there is no valid bare flag in this URI shape.
        assert_eq!(
            parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&image"),
            Err(OtpRefusal::UnknownParameter("image".to_string()))
        );
        // Control: with the unknown key removed, the same URI parses -- so the
        // refusal is about the key and not about the shape around it.
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").is_ok());
    }

    #[test]
    fn a_secret_that_is_not_base32_is_refused() {
        assert_eq!(parse_otpauth("otpauth://totp/x?secret=not!base32"), Err(OtpRefusal::BadSecret));
        // `0`, `1` and `8` are not in the alphabet, and they are exactly the
        // characters a human mis-transcribing `O`, `I` and `B` produces.
        assert_eq!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DP01"), Err(OtpRefusal::BadSecret));
        // Control: the valid one really does parse, so the refusal is about
        // the secret and not about the URI shape.
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").is_ok());
    }

    #[test]
    fn a_missing_or_empty_secret_is_its_own_refusal() {
        // Distinct from `BadSecret`: "there is no code in this" and "the code
        // in this is malformed" are two different sentences to show.
        assert_eq!(parse_otpauth("otpauth://totp/x"), Err(OtpRefusal::NoSecret));
        assert_eq!(parse_otpauth("otpauth://totp/x?issuer=Git"), Err(OtpRefusal::NoSecret));
        assert_eq!(parse_otpauth("otpauth://totp/x?secret="), Err(OtpRefusal::NoSecret));
        // Padding and spaces alone are still nothing.
        assert_eq!(parse_otpauth("otpauth://totp/x?secret=%20%3D"), Err(OtpRefusal::NoSecret));
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").is_ok());
    }

    #[test]
    fn the_secret_is_normalised_the_way_a_site_prints_it() {
        // Sites print the seed lowercase, in groups of four, sometimes padded.
        // A user who copies it verbatim must get the same item as one who
        // scanned the QR.
        let grouped =
            parse_otpauth("otpauth://totp/x?secret=jbsw%20y3dp%20ehpk%203pxp%3D%3D").unwrap();
        assert_eq!(grouped.secret.as_str(), "JBSWY3DPEHPK3PXP");
        let plain = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(grouped.secret.as_str(), plain.secret.as_str());
        // Paired the other way: normalisation drops separators, it does not
        // drop content. A different seed stays different.
        let other = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXQ").unwrap();
        assert_ne!(other.secret.as_str(), plain.secret.as_str());
    }

    #[test]
    fn a_bad_known_parameter_names_itself() {
        for (uri, which) in [
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=7", "digits"),
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0", "period"),
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=abc", "period"),
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=99999", "period"),
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5", "algorithm"),
            // A repeated parameter: two answers to one question, and no way
            // to know which the card meant.
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=6&digits=8", "digits"),
            ("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&secret=JBSWY3DPEHPK3PXQ", "secret"),
        ] {
            assert_eq!(
                parse_otpauth(uri),
                Err(OtpRefusal::BadParameter(which)),
                "{uri} did not name {which}"
            );
        }
        // Controls: each of those parameters has a value that IS accepted, so
        // the refusals above are about the value and not about the key.
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=6").is_ok());
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=30").is_ok());
        assert!(parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=sha1").is_ok());
    }

    #[test]
    fn an_overlong_payload_is_refused_before_it_is_decoded() {
        let huge = format!("otpauth://totp/x?secret={}", "A".repeat(MAX_URI_LEN));
        assert_eq!(parse_otpauth(&huge), Err(OtpRefusal::TooLong));
        // Control on the bound: one byte under it, the same shape parses.
        let fits = format!(
            "otpauth://totp/x?secret={}",
            "A".repeat(MAX_URI_LEN - "otpauth://totp/x?secret=".len())
        );
        assert_eq!(fits.len(), MAX_URI_LEN);
        assert!(parse_otpauth(&fits).is_ok());
    }

    #[test]
    fn the_label_is_split_on_either_spelling_of_the_separator() {
        let literal = parse_otpauth("otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP")
            .unwrap();
        assert_eq!(literal.issuer.as_deref(), Some("Git Host"));
        assert_eq!(literal.account.as_deref(), Some("anovak"));

        let escaped = parse_otpauth("otpauth://totp/Git%20Host%3Aanovak?secret=JBSWY3DPEHPK3PXP")
            .unwrap();
        assert_eq!(escaped.issuer.as_deref(), Some("Git Host"));
        assert_eq!(escaped.account.as_deref(), Some("anovak"));

        // Paired: with no separator at all, the whole label is the ACCOUNT and
        // the issuer is absent -- not the other way round.
        let bare = parse_otpauth("otpauth://totp/anovak?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(bare.issuer, None);
        assert_eq!(bare.account.as_deref(), Some("anovak"));

        // And an empty label leaves both absent rather than leaving an empty
        // string to render as a blank row.
        let none = parse_otpauth("otpauth://totp/?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!((none.issuer, none.account), (None, None));
    }

    #[test]
    fn the_issuer_parameter_wins_over_the_label_prefix() {
        let a = parse_otpauth(
            "otpauth://totp/Old%20Name:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host",
        )
        .unwrap();
        assert_eq!(a.issuer.as_deref(), Some("Git Host"));
        // Paired: without the parameter the label prefix IS used, so the rule
        // above is a precedence and not a discard.
        let b = parse_otpauth("otpauth://totp/Old%20Name:anovak?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(b.issuer.as_deref(), Some("Old Name"));
    }

    #[test]
    fn the_scheme_and_type_are_matched_case_insensitively() {
        // Real desktop authenticators emit `OTPAUTH://TOTP/`. Refusing it
        // would be a refusal the user cannot act on.
        assert!(parse_otpauth("OTPAUTH://TOTP/x?secret=JBSWY3DPEHPK3PXP").is_ok());
        // Paired: case insensitivity is about the SCHEME and the TYPE, not
        // about everything -- `hotp` in any case is still refused.
        assert_eq!(
            parse_otpauth("OTPAUTH://HOTP/x?secret=JBSWY3DPEHPK3PXP"),
            Err(OtpRefusal::NotTotp)
        );
    }

    #[test]
    fn a_uri_round_trips_through_to_uri() {
        let src = "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host&digits=8&period=60&algorithm=SHA256";
        let back = to_uri(&parse_otpauth(src).unwrap());
        let reparsed = parse_otpauth(&back).unwrap();
        assert_eq!(reparsed.digits, 8);
        assert_eq!(reparsed.period, 60);
        assert_eq!(reparsed.algorithm, Algorithm::Sha256);
        assert_eq!(reparsed.secret.as_str(), "JBSWY3DPEHPK3PXP");
        assert_eq!(reparsed.issuer.as_deref(), Some("Git Host"));
        assert_eq!(reparsed.account.as_deref(), Some("anovak"));
        // Whole-value equality, so a field added later that `to_uri` forgets
        // to write fails here rather than in a code that is silently wrong.
        assert_eq!(reparsed, parse_otpauth(src).unwrap());
    }

    #[test]
    fn the_defaults_survive_a_round_trip_as_values_and_not_as_absences() {
        let a = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP").unwrap();
        let uri = to_uri(&a);
        // Written out explicitly: the vault's copy says what it means rather
        // than relying on the next reader's defaults matching this one's.
        assert!(uri.contains("&algorithm=SHA1"), "{}", &*uri);
        assert!(uri.contains("&digits=6"), "{}", &*uri);
        assert!(uri.contains("&period=30"), "{}", &*uri);
        assert_eq!(parse_otpauth(&uri).unwrap(), a);
    }

    #[test]
    fn an_issuer_cannot_smuggle_a_separator_through_to_uri() {
        // The payload is untrusted, and `to_uri` writes a string that is
        // parsed again. An issuer of `a:b` or `a&secret=..` that went out
        // unescaped would come back as a different URI than the one confirmed.
        let hostile = OtpAuth {
            issuer: Some("Git:Host&secret=AAAA&x=/".to_string()),
            account: Some("a?b".to_string()),
            secret: Zeroizing::new("JBSWY3DPEHPK3PXP".to_string()),
            algorithm: Algorithm::Sha1,
            digits: 6,
            period: 30,
        };
        let uri = to_uri(&hostile);
        let back = parse_otpauth(&uri).expect("the rendered URI parses");
        assert_eq!(back, hostile, "a metacharacter in the issuer changed the URI's meaning");
        // Control on the escaping itself, so the equality above is not two
        // bugs agreeing: the raw text really is escaped.
        assert!(!uri.contains("Git:Host"), "{}", &*uri);
        assert!(uri.contains("Git%3AHost"), "{}", &*uri);
    }

    #[test]
    fn a_non_ascii_issuer_survives_a_round_trip() {
        let a = OtpAuth {
            issuer: Some("Bücher".to_string()),
            account: Some("ünïcode".to_string()),
            secret: Zeroizing::new("JBSWY3DPEHPK3PXP".to_string()),
            algorithm: Algorithm::Sha512,
            digits: 8,
            period: 45,
        };
        assert_eq!(parse_otpauth(&to_uri(&a)).unwrap(), a);
    }

    #[test]
    fn a_multi_byte_character_written_as_escapes_decodes_to_one_character() {
        // `Bücher` is written `B%C3%BCcher` by every encoder there is. A
        // decoder that turned each escape into its own `char` yields
        // `B{c3}{bc}cher` -- which looks broken on screen and round-trips
        // to a DIFFERENT issuer than the one the user confirmed.
        let a = parse_otpauth(
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&issuer=B%C3%BCcher",
        )
        .unwrap();
        assert_eq!(a.issuer.as_deref(), Some("Bücher"));
        assert_eq!(a.issuer.as_deref().map(str::chars).map(Iterator::count), Some(6));
        // Paired: the escapes are really being DECODED, not passed through --
        // the literal text is gone.
        assert!(!a.issuer.as_deref().unwrap().contains("%C3"));

        // And escapes that spell no character at all do not become a seed:
        // the replacement they decode to is not base32, and is refused.
        assert_eq!(
            parse_otpauth("otpauth://totp/x?secret=JBSW%FF%FE"),
            Err(OtpRefusal::BadSecret)
        );
    }

    #[test]
    fn debug_prints_the_labels_and_never_the_seed() {
        let a = parse_otpauth(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP&issuer=Git%20Host",
        )
        .unwrap();
        let printed = format!("{a:?}");
        // The seed is not there, in any casing...
        assert!(!printed.contains("JBSWY3DPEHPK3PXP"), "{printed}");
        assert!(!printed.to_ascii_uppercase().contains("JBSWY3DPEHPK3PXP"), "{printed}");
        // ...and neither is a prefix of it long enough to matter.
        assert!(!printed.contains("JBSWY"), "{printed}");
        // Paired: the impl is not simply empty. What is safe IS printed, and
        // the redaction says a length so a log line can tell "there is a seed"
        // from "the seed is missing".
        assert!(printed.contains("Git Host"), "{printed}");
        assert!(printed.contains("anovak"), "{printed}");
        assert!(printed.contains("16 bytes redacted"), "{printed}");
    }

    /// **The seed does not reach the allocator in the clear** when an
    /// [`OtpAuth`] is dropped, or when the URI built from one is.
    ///
    /// Uses `login_ui`'s allocator-watch harness, the instrument that pins
    /// `LoginForm`'s and `Plan`'s drops. The positive control is asserted
    /// **first**, so a negative reading below means the wipe worked rather
    /// than that the instrument is deaf -- a zeroization test that could not
    /// fail has shipped in this crate before.
    ///
    /// **The honest gap.** The harness scans for one fixed needle, `PROBE`,
    /// and `PROBE` is not valid base32 -- so [`parse_otpauth`] cannot be armed
    /// with it: the seed it would have to carry is refused by
    /// [`normalise_secret`] before any of the intermediates exist. What is
    /// measured here is the two places a seed is *copied*: the struct that
    /// holds it and [`to_uri`], which is the one that writes it to the vault.
    /// `parse_otpauth`'s own intermediates are `Zeroizing` at exact capacity
    /// by inspection and are **not** covered by this instrument.
    #[test]
    fn a_dropped_otpauth_does_not_release_the_seed_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Positive control, first: a bare `String` of the probe, dropped
        // without a wipe, IS seen.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the allocator watch is not seeing an unwiped drop"
        );

        let built = OtpAuth {
            issuer: Some("Git Host".to_string()),
            account: Some("anovak".to_string()),
            secret: Zeroizing::new(PROBE.to_string()),
            algorithm: Algorithm::Sha1,
            digits: 6,
            period: 30,
        };
        assert!(
            !plaintext_reached_the_allocator(move || drop(built)),
            "a dropped OtpAuth released the seed in the clear"
        );
    }

    /// **`to_uri` does not leave an un-wiped copy of the seed behind**, which
    /// it would the moment its buffer grew.
    ///
    /// This is the test with teeth: `Zeroizing<String>` wipes the buffer it is
    /// **holding**, and a `String` that re-allocates has already handed its
    /// previous buffer -- seed and all -- back to the allocator. The
    /// `with_capacity` in [`to_uri`] is what stops that, and the second
    /// control below is a deliberately leaky rendering that proves it.
    #[test]
    fn to_uri_does_not_release_the_seed_through_a_reallocation() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Positive control, first, as the house rule requires.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the allocator watch is not seeing an unwiped drop"
        );

        let plan = OtpAuth {
            issuer: Some("Git Host".to_string()),
            account: Some("anovak".to_string()),
            secret: Zeroizing::new(PROBE.to_string()),
            algorithm: Algorithm::Sha256,
            digits: 8,
            period: 60,
        };

        // Control that this measures anything at all: the rendered URI really
        // does contain the seed. Read OUTSIDE the armed window.
        {
            let uri = to_uri(&plan);
            assert!(uri.contains(PROBE), "to_uri did not write the seed, so the test is vacuous");
        }

        {
            let plan = plan.clone();
            assert!(
                !plaintext_reached_the_allocator(move || drop(to_uri(&plan))),
                "to_uri released the seed in the clear -- a re-allocation leaves the old \
                 buffer, contents and all, with the allocator, and `Zeroizing` cannot reach it"
            );
        }

        // **And the negative control on the mechanism**: the same rendering
        // done into a `String::new()` that grows IS seen. Without this, the
        // assertion above would pass just as happily on a `to_uri` that never
        // needed a re-allocation for an unrelated reason, and the
        // `with_capacity` it is testing could be deleted with the suite green.
        let leaky = plan.clone();
        assert!(
            plaintext_reached_the_allocator(move || {
                let mut grown = Zeroizing::new(String::new());
                grown.push_str(SCHEME);
                grown.push_str("totp/anovak?secret=");
                grown.push_str(&leaky.secret);
                grown.push_str("&period=60");
                drop(grown);
            }),
            "control: a rendering that re-allocates was NOT seen, so this instrument cannot \
             tell a capacity-disciplined build from a growing one and the test above is empty"
        );

        drop(plan);
    }

    #[test]
    fn the_refusal_type_carries_no_fragment_of_a_seed() {
        // `BadSecret` is the refusal for a malformed seed, and the malformed
        // seed is still a seed. It carries nothing, and its `Debug` -- which
        // IS derived, because the type reaches no `Zeroizing` -- proves it.
        let refusal = parse_otpauth("otpauth://totp/x?secret=JBSWY3DP%21%21").unwrap_err();
        assert_eq!(refusal, OtpRefusal::BadSecret);
        let printed = format!("{refusal:?}");
        assert!(!printed.contains("JBSWY"), "{printed}");
        // Paired: `UnknownParameter` DOES carry text, and it is the key, which
        // is not a secret -- so the absence above is a decision, not an
        // accident of the enum having no data anywhere.
        let named = parse_otpauth("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&surprise=1").unwrap_err();
        assert!(format!("{named:?}").contains("surprise"));
    }
}

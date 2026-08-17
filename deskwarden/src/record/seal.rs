//! Passphrase sealing of a TOTP seed.
//!
//! AES-256-GCM under a key derived from the passphrase with **Argon2id**, a
//! random 16-byte salt and a random 12-byte nonce per seal.
//!
//! # Why not the crate's existing KDF
//!
//! `hello.rs` derives its AES key with a **single SHA-256**, and that is right
//! there: its input is a Windows Hello `KeyCredential` signature, which already
//! carries full entropy, and stretching a uniformly random 256-bit input buys
//! nothing.
//!
//! It would be wrong here. The input is a passphrase a human chose and then
//! read out over the phone, and the ciphertext may sit on a Bitwarden server
//! for days with an attacker holding both it and the link. A single SHA-256
//! over a human passphrase is a few hundred million guesses per second on one
//! GPU. So this module uses a deliberately slow, memory-hard KDF instead, and
//! [`ARGON2_MEMORY_KIB`] is the parameter that makes it slow.
//!
//! # The format is versioned on purpose
//!
//! [`SEAL_VERSION`] and [`SEAL_KDF`] travel with every sealed seed, and a
//! reader that does not recognise either **refuses**. Changing the KDF later is
//! then a polite refusal at the recipient rather than a wrong key silently
//! derived from the right passphrase and reported as a bad one.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

/// The shape of a sealed seed on the wire. Bumped when the *layout* changes.
pub const SEAL_VERSION: u32 = 1;

/// The key derivation this version uses, written into the payload so a future
/// change is a refusal rather than a misparse. See the module docs.
pub const SEAL_KDF: &str = "argon2id";

/// Argon2id cost. The memory parameter is the one that matters against custom
/// hardware; 19 MiB with `t=2, p=1` is the OWASP-recommended pairing.
///
/// **Measured, not assumed**, on the machine this was written on: about 0.5 s
/// per derivation in a `debug` build and about 75 ms in `release`. The debug
/// figure is the one the test suite pays, and it is why these numbers were
/// checked rather than turned up further: the whole of this crate's record
/// tests derive a key about a dozen times.
///
/// Turning the memory cost DOWN is the edit to be suspicious of. It is the
/// parameter an attacker's custom hardware has to pay for, and it is the only
/// reason this is not `hello.rs`'s single SHA-256 with extra steps.
pub const ARGON2_MEMORY_KIB: u32 = 19 * 1024;
/// Argon2id time cost (passes). See [`ARGON2_MEMORY_KIB`].
pub const ARGON2_PASSES: u32 = 2;
/// Argon2id parallelism (lanes). See [`ARGON2_MEMORY_KIB`].
pub const ARGON2_LANES: u32 = 1;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// A seed sealed under a passphrase. Never the bare seed.
///
/// **Salt and nonce are random per seal**, which is not a formality: a fixed
/// salt would make the same seed under the same passphrase produce byte-equal
/// ciphertext, so an observer holding one link could recognise the same seed in
/// another link without opening either.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedSeed {
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

/// Redacting, in the house style of `SendSummary` and `CreatedSend`.
///
/// The ciphertext is not plaintext, but it is the whole of what a passphrase
/// guess is tested against — printing it into a log file would hand an
/// offline attacker the oracle and save them the theft. Lengths only.
impl std::fmt::Debug for SealedSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSeed")
            .field("salt", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("ciphertext", &format_args!("<{} bytes redacted>", self.ciphertext.len()))
            .finish()
    }
}

/// Why a seal would not open.
///
/// One arm for "the passphrase is wrong" and for "the bytes were changed",
/// deliberately: AES-GCM cannot tell them apart, and inventing a second arm
/// would be a claim the mechanism does not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealFailed {
    /// The tag did not verify: a wrong passphrase, or tampered bytes.
    WrongPassphrase,
    /// The payload said it was sealed some other way than this build knows.
    UnsupportedSeal,
}

impl SealFailed {
    /// A sentence to show the user. A refusal that renders as a generic
    /// failure teaches the user to retry, which is the opposite of what a
    /// rejected seal should teach.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::WrongPassphrase => {
                "That passphrase does not open the one-time code seed, or the payload was \
                 altered on the way here."
            }
            Self::UnsupportedSeal => {
                "This seed was sealed by a newer version of Deskwarden and cannot be opened here."
            }
        }
    }
}

/// Derives the AES key from `passphrase` and `salt`.
///
/// The output is [`Zeroizing`], and so is nothing else here on purpose: Argon2
/// is handed the passphrase by reference and writes straight into the caller's
/// wiped buffer, so this function makes no plaintext copy of its own.
fn derive_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_PASSES, ARGON2_LANES, Some(32))
        .expect("the Argon2 parameters in this file are constants and are in range");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut_slice())
        .expect("Argon2id over a 16-byte salt into a 32-byte key cannot fail");
    key
}

/// Seals `seed` under `passphrase`.
///
/// Panics only if the operating system will not produce randomness, which is
/// not a condition to paper over with a fixed salt: see [`SealedSeed`].
pub fn seal(seed: &str, passphrase: &str) -> SealedSeed {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).expect("the OS would not produce a random salt");
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).expect("the OS would not produce a random nonce");

    let key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), seed.as_bytes())
        .expect("AES-256-GCM cannot fail on a seed-sized plaintext");

    SealedSeed { salt, nonce, ciphertext }
}

/// Opens a seal, or refuses. The seed comes back wiped-on-drop and never as a
/// plain `String`.
pub fn unseal(sealed: &SealedSeed, passphrase: &str) -> Result<Zeroizing<String>, SealFailed> {
    let key = derive_key(passphrase, &sealed.salt);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()));
    // Wrapped the instant it exists: `decrypt` hands back a plain `Vec<u8>`
    // holding the seed, and that buffer is as much of a leak as the `String`
    // built out of it below would be.
    let plain = Zeroizing::new(
        cipher
            .decrypt(Nonce::from_slice(&sealed.nonce), sealed.ciphertext.as_slice())
            .map_err(|_| SealFailed::WrongPassphrase)?,
    );
    let text = std::str::from_utf8(&plain).map_err(|_| SealFailed::WrongPassphrase)?;
    Ok(Zeroizing::new(text.to_string()))
}

// ---- base64, for putting a sealed seed in JSON -----------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, straight into a pre-reserved buffer.
///
/// `send.rs` has a twin of this and it is private there; the crate has no
/// base64 dependency, and adding one to encode three short byte strings would
/// be a larger change than the feature. **This encodes ciphertext, salt and
/// nonce only** — never a secret — but it still pushes into the caller's buffer
/// rather than returning a fresh `String`, because the caller's buffer is the
/// wiped one and the payload around this text is not public.
pub(crate) fn base64_into(out: &mut String, bytes: &[u8]) {
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
}

/// The inverse. `None` for anything that is not exactly standard base64 —
/// **this is a reader of untrusted text**, so a stray character is a refusal
/// and never a byte quietly skipped.
pub(crate) fn base64_from(text: &str) -> Option<Vec<u8>> {
    if text.len() % 4 != 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut chunks = bytes.chunks(4).peekable();
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        let mut n: u32 = 0;
        let mut pads = 0usize;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                // Padding is legal only at the end, and only in the last two
                // positions of the last group.
                if !last || i < 2 {
                    return None;
                }
                pads += 1;
                n <<= 6;
                continue;
            }
            if pads > 0 {
                // A real character after a pad: `AB=C` is not base64.
                return None;
            }
            let v = B64.iter().position(|&b| b == c)? as u32;
            n = (n << 6) | v;
        }
        // Two pads carry one byte, one pad carries two, none carries three.
        let keep = 3 - pads;
        out.push((n >> 16) as u8);
        if keep > 1 {
            out.push((n >> 8) as u8);
        }
        if keep > 2 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "JBSWY3DPEHPK3PXP";

    #[test]
    fn a_sealed_seed_needs_the_passphrase_and_the_link_is_not_enough() {
        let sealed = seal(SEED, "correct horse battery staple");
        assert!(
            !sealed.ciphertext.windows(4).any(|w| w == b"JBSW"),
            "the seed is recognisable in the ciphertext, so it was not encrypted"
        );
        assert_eq!(unseal(&sealed, "correct horse battery staple").unwrap().as_str(), SEED);
        assert!(matches!(unseal(&sealed, "wrong"), Err(SealFailed::WrongPassphrase)));
    }

    #[test]
    fn two_seals_of_the_same_seed_differ() {
        // A fixed nonce or salt would make identical seeds produce identical
        // ciphertext, so an observer holding one link could recognise the same
        // seed in another.
        let a = seal(SEED, "pw");
        let b = seal(SEED, "pw");
        assert_ne!(a.ciphertext, b.ciphertext);
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.nonce, b.nonce);
        // Control: both really do open, so the inequalities above are about
        // randomised sealing and not about a `seal` that emits noise.
        assert_eq!(unseal(&a, "pw").unwrap().as_str(), SEED);
        assert_eq!(unseal(&b, "pw").unwrap().as_str(), SEED);
    }

    #[test]
    fn a_tampered_ciphertext_is_refused_rather_than_decrypted_to_garbage() {
        let mut sealed = seal(SEED, "pw");
        sealed.ciphertext[0] ^= 0xff;
        assert!(matches!(unseal(&sealed, "pw"), Err(SealFailed::WrongPassphrase)));
    }

    #[test]
    fn the_kdf_is_slow_and_memory_hard_on_purpose() {
        // The parameter, pinned as a decision. `hello.rs`'s single SHA-256 is
        // right for a Hello signature and wrong for a human passphrase; this
        // is the line that would have to be deliberately edited to undo that
        // reasoning, rather than a comment that could be ignored.
        assert_eq!(SEAL_KDF, "argon2id");
        assert!(
            ARGON2_MEMORY_KIB >= 19 * 1024,
            "the memory cost is what an attacker's custom hardware has to pay; \
             {ARGON2_MEMORY_KIB} KiB is below the recommended 19 MiB"
        );
        assert!(ARGON2_PASSES >= 2);
    }

    #[test]
    fn the_debug_of_a_sealed_seed_prints_no_bytes() {
        let sealed = SealedSeed {
            salt: [7u8; SALT_LEN],
            nonce: [9u8; NONCE_LEN],
            ciphertext: vec![0xab; 24],
        };
        let shown = format!("{sealed:?}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!shown.contains("171") && !shown.contains("ab, ab"), "{shown}");
        // Control: the `Debug` produced something at all.
        assert!(shown.contains("SealedSeed"), "{shown}");
    }

    #[test]
    fn base64_round_trips_and_refuses_what_is_not_base64() {
        for case in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            let mut out = String::new();
            base64_into(&mut out, case);
            assert_eq!(base64_from(&out).as_deref(), Some(case), "round trip of {case:?}");
        }
        let mut out = String::new();
        base64_into(&mut out, b"foo");
        assert_eq!(out, "Zm9v");
        // Untrusted text: a stray character is a refusal, not a skipped byte.
        assert_eq!(base64_from("Zm9!"), None);
        assert_eq!(base64_from("Zm9"), None, "a truncated group is not base64");
        assert_eq!(base64_from("Z=9v"), None, "padding in the middle is not base64");
    }

    /// The allocator probe, applied to the two secrets on this path: the seed
    /// going in and the seed coming back out.
    ///
    /// **The control is asserted first**, as every probe test in this crate
    /// does. It is not a formality here: this crate has already shipped a
    /// zeroization test that could not fail, and the probe's `dealloc` wipes
    /// every freed block unconditionally, so "nothing was seen" is exactly what
    /// a broken instrument also reports.
    ///
    /// This test was **shown able to fail** before it was trusted: returning
    /// the unsealed seed from [`unseal`] as a plain `String` instead of a
    /// `Zeroizing<String>` reds the second assertion below.
    #[test]
    fn the_seed_and_the_passphrase_do_not_reach_the_allocator_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Built before the watch is armed, so building it is not what is
        // measured.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: the allocator probe did not see a plain `String` carrying the probe go \
             back to the allocator, so every verdict below is meaningless"
        );

        let seed = Zeroizing::new(String::from_utf8(PROBE.as_bytes().to_vec()).expect("utf8"));
        let passphrase = Zeroizing::new(String::from_utf8(PROBE.as_bytes().to_vec()).expect("utf8"));
        assert!(
            !plaintext_reached_the_allocator(move || {
                let sealed = seal(&seed, &passphrase);
                let opened = unseal(&sealed, &passphrase).expect("it opens");
                assert_eq!(opened.as_str(), seed.as_str());
                drop(opened);
                drop(sealed);
                drop(passphrase);
                drop(seed);
            }),
            "sealing or unsealing handed the seed or the passphrase back to the allocator in \
             the clear. The buffers to look at are the key material, the plaintext `decrypt` \
             returns, and the `String` built out of it."
        );
    }
}

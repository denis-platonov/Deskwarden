//! The client-side cryptography of a direct-REST Bitwarden backend: master
//! key, stretched key, protected symmetric key, `EncString`, and the two
//! unwraps that hang off them.
//!
//! # This changes the app's threat model, and that is the first thing to read
//!
//! **Today Deskwarden never holds the master password.** The login window
//! hands it to the `bw` CLI, `bw` derives every key from it inside its own
//! process, and this process keeps one thing: a session token, DPAPI-wrapped
//! at rest ([`crate::session_store`]). Whatever went wrong in this app's
//! memory, the master key was somewhere else.
//!
//! **A REST backend ends that.** To talk to a Bitwarden-compatible server
//! without the CLI, this process must derive the master key itself, stretch
//! it, unwrap the user's symmetric key with it, and hold that key for as long
//! as the vault is unlocked -- because every item, every organisation key and
//! every attachment key is decrypted under it. The master password is in this
//! process's address space for the length of a login, and the keys below it
//! for the length of a session.
//!
//! That is not an argument against the change; it is what *any* client that
//! is not shelling out to another client has to do, and it is what the
//! official clients do. It is an argument for saying so out loud:
//!
//! * Everything here that is a key or a plaintext is [`Zeroizing`], so it is
//!   wiped when it is dropped rather than left in a freed page.
//! * No type here derives `Debug` over one; see
//!   [`crate::debug_leak_guard`], which fails the suite if one ever does.
//! * No error, log line or panic message in this file carries a key, a
//!   password or a plaintext. "MAC mismatch" is the whole of what a failure
//!   is allowed to say.
//!
//! **`PRIVACY.md` says the current thing, and it will stop being true.** Its
//! wording rests on `bw` owning the master password, and it must be revisited
//! before any of this ships -- not when the HTTP layer lands, but before a
//! user can reach a REST login at all.
//!
//! # What is verified, and what is not
//!
//! Every primitive below is checked against a **published** test vector, cited
//! next to the test that uses it: RFC 7914 for PBKDF2-HMAC-SHA256, RFC 5869
//! for HKDF-SHA256, RFC 4231 for HMAC-SHA256, NIST SP 800-38A for
//! AES-256-CBC, RFC 9106 for Argon2id.
//!
//! **The Bitwarden-specific composition is not**, and this is stated plainly
//! rather than dressed up. Which key stretches into which, that the HKDF info
//! strings are `enc` and `mac`, that the PBKDF2 salt is the lowercased email
//! and the Argon2id salt is its SHA-256, that the protected symmetric key is
//! 64 bytes of encryption-key-then-MAC-key -- all of that is taken from
//! Bitwarden's published format and its clients, and **nothing in this crate
//! can confirm it**. Confirming it needs a real account on a real server,
//! which no test here is allowed to touch. The composition tests below build
//! their fixture out of the verified primitives and check that this code
//! agrees with itself about it; where a NIST vector could supply the
//! ciphertext and plaintext instead of this code, it does, and that is said
//! at the test. See `the_composition_is_self_consistent_and_that_is_all_it_is`.
//!
//! **This holds in both directions now, and the two directions are not
//! equally pinned.** AES-256-CBC is checked against NIST SP 800-38A
//! *encrypting* (F.2.5) as well as decrypting (F.2.6), so the cipher step
//! [`encrypt`] performs is external, not a round trip against this file's
//! own decryptor. What is **not** external on the encrypt side is PKCS#7
//! *construction* -- the padding [`encrypt`] writes is checked only by
//! [`strip_pkcs7`] accepting it back, and no published vector pads NIST's
//! plaintext -- and, above that, the claim that an `EncString` this module
//! emits is one another Bitwarden client will accept. Nothing in this crate
//! can confirm that last claim in either direction, for the same reason:
//! it needs a real account on a real server. The full list is at
//! `the_composition_is_still_not_fully_pinned_and_here_is_what_is_left`.
//!
//! # Scope
//!
//! Symmetric encryption and decryption, and no I/O of any kind. There is no
//! HTTP here, no API client, no login flow, and nothing in the running app
//! calls this yet.
//!
//! [`encrypt`] is the newest thing here and the only one that generates
//! anything: a fresh OS-random IV per call, encrypt-then-MAC, PKCS#7. Its
//! doc comment carries the reasoning, and it should be read alongside
//! [`decrypt`]'s, because the two are one composition written in two places.
//! Writing an `EncString` back out to `type.iv|ct|mac` is [`EncString`]'s
//! `Display`. There is no *asymmetric* encryption: type 4 needs a public key
//! this module does not hold.

use aes::Aes256;
use argon2::{Algorithm, Argon2, ParamsBuilder, Version};
use cbc::cipher::block_padding::NoPadding;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Oaep, RsaPrivateKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::record::seal::{base64_from, base64_into};

/// AES's block size, in bytes. The IV's length, and the granularity every
/// CBC ciphertext has to be a whole number of.
const BLOCK: usize = 16;

/// The length of every key in this module: AES-256 and HMAC-SHA256 alike.
const KEY_LEN: usize = 32;

/// The length of the protected symmetric key's plaintext: an encryption key
/// followed by a MAC key.
const SYMMETRIC_KEY_LEN: usize = KEY_LEN * 2;

type Aes256CbcDec = cbc::Decryptor<Aes256>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type HmacSha256 = Hmac<Sha256>;

// ---- errors ----------------------------------------------------------------

/// Why a derivation, a parse or a decryption did not produce a key.
///
/// **Nothing in here is derived from a secret.** Every arm carries either a
/// fixed string or a length, and lengths of ciphertext are already public to
/// anyone holding the ciphertext. This is not a formality: an error type is
/// the value most likely to reach a log file, and the reason `SealFailed` in
/// [`crate::record::seal`] is shaped the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// A KDF parameter the server sent cannot be used -- a zero iteration
    /// count, an Argon2 memory or lane figure outside what the algorithm
    /// accepts. The `&'static str` names the parameter, never its value's
    /// provenance.
    KdfParams(&'static str),
    /// The `EncString` is not the shape its own type byte claims. Carries a
    /// fixed description of what was wrong with the *structure*.
    Malformed(&'static str),
    /// A recognised Bitwarden `EncString` type this module refuses, **by
    /// name** so the refusal says which format was seen rather than a number.
    Unsupported(&'static str),
    /// The type byte is not one Bitwarden has ever defined.
    UnknownType,
    /// The MAC did not verify. The ciphertext was **not** decrypted: this is
    /// returned before the cipher is constructed.
    MacMismatch,
    /// PKCS#7 padding was not well-formed after a MAC that did verify. That
    /// combination means a bug or a corrupt store, not an attacker -- an
    /// attacker cannot reach this arm without the MAC key.
    Padding,
    /// A decrypted key was not the length a key has to be.
    KeyLength { expected: usize, got: usize },
    /// The operating system would not produce randomness, so no IV could be
    /// generated and **nothing was encrypted**.
    ///
    /// A new arm rather than a reused one, and not an `expect`. Every other
    /// failure here is a statement about data; this is a statement about the
    /// machine, and the one thing that must never happen is for an encrypt
    /// that could not get a fresh IV to fall back on a stale, derived or
    /// constant one. Returning an error is the only behaviour that makes
    /// that impossible to write by accident. It carries no detail from
    /// [`getrandom`], which is a `Display` this module has not audited.
    Rng,
    /// The RSA private key could not be read as PKCS#8 DER, or OAEP
    /// unwrapping failed. One arm for both on purpose: distinguishing them
    /// tells a caller which half of a secret was wrong.
    Rsa,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KdfParams(what) => write!(f, "the server's KDF parameters are unusable: {what}"),
            Self::Malformed(what) => write!(f, "malformed encrypted string: {what}"),
            Self::Unsupported(name) => {
                write!(f, "encrypted string format `{name}` is not supported")
            }
            Self::UnknownType => f.write_str("encrypted string has an unknown type"),
            Self::MacMismatch => f.write_str("MAC mismatch"),
            Self::Padding => f.write_str("padding is not well-formed"),
            Self::KeyLength { expected, got } => {
                write!(f, "decrypted key is {got} bytes, expected {expected}")
            }
            Self::Rng => f.write_str("the operating system would not produce randomness"),
            Self::Rsa => f.write_str("RSA unwrapping failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

// ---- keys ------------------------------------------------------------------

/// The master key: 32 bytes derived from the master password and the email.
///
/// It is not used to decrypt anything directly. It is stretched
/// ([`MasterKey::stretch`]) into the pair that unwraps the user's real
/// symmetric key.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterKey(Zeroizing<[u8; KEY_LEN]>);

/// Redacting, and hand-written because it must be: see the module docs and
/// [`crate::debug_leak_guard`]. A length is all a master key may print.
impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(<32 bytes redacted>)")
    }
}

/// An encryption key and a MAC key, together.
///
/// Two things in Bitwarden have exactly this shape and this code uses one
/// type for both, because they *are* the same thing used at different depths:
/// the stretched master key, and the user's (or an organisation's) symmetric
/// key that the stretched one unwraps.
#[derive(Clone, PartialEq, Eq)]
pub struct SymmetricKey {
    enc: Zeroizing<[u8; KEY_LEN]>,
    mac: Zeroizing<[u8; KEY_LEN]>,
}

/// Redacting, for [`MasterKey`]'s reason.
impl std::fmt::Debug for SymmetricKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SymmetricKey { enc: <32 bytes redacted>, mac: <32 bytes redacted> }")
    }
}

impl SymmetricKey {
    /// Splits the 64-byte plaintext of a protected symmetric key.
    fn from_64(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != SYMMETRIC_KEY_LEN {
            return Err(CryptoError::KeyLength {
                expected: SYMMETRIC_KEY_LEN,
                got: bytes.len(),
            });
        }
        let mut enc = Zeroizing::new([0u8; KEY_LEN]);
        let mut mac = Zeroizing::new([0u8; KEY_LEN]);
        enc.copy_from_slice(&bytes[..KEY_LEN]);
        mac.copy_from_slice(&bytes[KEY_LEN..]);
        Ok(Self { enc, mac })
    }
}

// ---- the key derivation function -------------------------------------------

/// The KDF an account's master key is derived with, as the server reports it.
///
/// Both arms exist because both kinds of account exist: PBKDF2 is what every
/// account had before Argon2id was offered, and an account that has not been
/// migrated still has it. A client that supports only one of them cannot log
/// half its users in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kdf {
    /// PBKDF2-HMAC-SHA256 with the account's own iteration count.
    Pbkdf2 { iterations: u32 },
    /// Argon2id with the account's memory, iteration and parallelism figures.
    ///
    /// **`memory_mib` is MEBIbytes, not kibibytes**, and the distinction is
    /// not pedantry -- it is a factor of 1024 in the memory cost, which makes
    /// a different key out of the right password. An earlier version of this
    /// enum called the field `memory_kib` and passed it to
    /// [`argon2::ParamsBuilder::m_cost`] unchanged, on the stated but wrong
    /// grounds that kibibytes are "the unit the server reports". They are
    /// not: the server's `KdfMemory` is what the user typed into the "memory"
    /// box in MiB (Vaultwarden's default is `64`, meaning 64 MiB), and
    /// Bitwarden's own client multiplies it by 1024 before handing it to
    /// Argon2 -- `crates/bitwarden-crypto/src/keys/kdf.rs`, `derive_kdf_key`:
    /// `let memory = memory.get() * 1024; // Convert MiB to KiB`.
    ///
    /// It was caught by
    /// `the_password_hash_matches_bitwardens_own_vector_for_argon2id`, which
    /// is exactly the kind of thing the module docs said could not be found
    /// without a real payload, and is the reason that vector is worth the
    /// memory it costs to run.
    ///
    /// The conversion happens in [`master_key`], once, so that this type
    /// carries the server's unit and nothing in between has to remember.
    Argon2id { iterations: u32, memory_mib: u32, parallelism: u32 },
}

/// Bitwarden's salt is the account's email, lowercased and trimmed.
///
/// Not a formality: the salt is what stops one precomputation covering every
/// account, and it is the one input to the master key the user does not type.
/// A client that trims differently from the server derives a different key
/// from the right password and reports it as a wrong one.
fn kdf_salt(email: &str) -> Zeroizing<String> {
    Zeroizing::new(email.trim().to_lowercase())
}

/// Derives the master key from the master password and the email.
///
/// The password is taken as bytes rather than `&str` so a caller holding a
/// `Zeroizing<String>` can pass `.as_bytes()` without this function making a
/// second, un-wiped copy of it.
pub fn master_key(password: &[u8], email: &str, kdf: Kdf) -> Result<MasterKey, CryptoError> {
    let salt = kdf_salt(email);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    match kdf {
        Kdf::Pbkdf2 { iterations } => {
            if iterations == 0 {
                return Err(CryptoError::KdfParams("iteration count is zero"));
            }
            pbkdf2::pbkdf2_hmac::<Sha256>(password, salt.as_bytes(), iterations, &mut *out);
        }
        Kdf::Argon2id { iterations, memory_mib, parallelism } => {
            // **Argon2id's salt is the SHA-256 of the email, not the email.**
            // Argon2 requires at least 8 salt bytes and Bitwarden feeds it a
            // fixed 32, which is also why this arm cannot reuse the PBKDF2
            // salt directly.
            let mut hashed = Zeroizing::new([0u8; KEY_LEN]);
            hashed.copy_from_slice(&Sha256::digest(salt.as_bytes()));
            let salt = hashed;
            // MiB -> KiB, which is the unit Argon2 itself counts in. Checked
            // rather than wrapping: a `KdfMemory` of four million from a
            // hostile or broken server would otherwise silently become a
            // small, cheap cost instead of an error.
            let Some(memory_kib) = memory_mib.checked_mul(1024) else {
                return Err(CryptoError::KdfParams("the memory cost does not fit in KiB"));
            };
            let params = ParamsBuilder::new()
                .m_cost(memory_kib)
                .t_cost(iterations)
                .p_cost(parallelism)
                .output_len(KEY_LEN)
                .build()
                .map_err(|_| CryptoError::KdfParams("memory, iteration or lane count"))?;
            Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
                .hash_password_into(password, &*salt, &mut *out)
                .map_err(|_| CryptoError::KdfParams("Argon2id rejected them"))?;
        }
    }
    Ok(MasterKey(out))
}

/// The length of a [`MasterKey`]'s raw bytes.
///
/// Public only so [`crate::user_key_store`]'s on-disk record can be a
/// fixed-size layout whose length check is written against the same constant
/// the key itself is, rather than a `32` typed out a second time.
pub const MASTER_KEY_LEN: usize = KEY_LEN;

impl MasterKey {
    /// The raw key bytes, for **persistence only**.
    ///
    /// # Read this before adding a second caller
    ///
    /// Every other operation on a master key is a method on this type
    /// precisely so the bytes never leave it: [`MasterKey::stretch`] and
    /// [`MasterKey::password_hash`] are the whole of what the vault needs,
    /// and neither hands anything out. This one does hand them out, and it
    /// exists for exactly one reason -- [`crate::user_key_store`] has to put
    /// the key in a DPAPI-wrapped file so that a restart does not have to ask
    /// for the master password again, and DPAPI takes bytes.
    ///
    /// It is `pub(crate)` and it must stay so. The return borrows rather than
    /// copies, so reading it makes no new buffer to forget to wipe, and the
    /// name says `expose` so that a call site reads as a decision.
    ///
    /// **Never format, log or send this.** A master key opens the whole
    /// vault, for ever -- unlike a session token, it does not expire.
    pub(crate) fn expose_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }

    /// The inverse of [`MasterKey::expose_bytes`], for the same one caller.
    ///
    /// Takes the array by value and moves it straight into a [`Zeroizing`],
    /// so the only copy that survives the call is the wiped one. It does not
    /// and cannot check that the bytes are a real key: nothing here can tell
    /// a rotated key from a current one, and the only thing that can is the
    /// server refusing the vault. See [`crate::user_key_store`]'s own doc for
    /// what the caller is required to do about that.
    pub(crate) fn from_bytes(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// **Key stretching**: HKDF-SHA256 *expand* over the master key.
    ///
    /// Expand only -- there is no extract step, and that is correct rather
    /// than an omission. HKDF-Extract exists to condense a non-uniform input
    /// into a pseudorandom key; the master key is already the 32-byte output
    /// of a KDF, so it *is* the PRK, and Bitwarden feeds it in as one.
    ///
    /// Infallible by construction: `from_prk` refuses a PRK shorter than the
    /// hash length and this one is exactly the hash length, and `expand`
    /// refuses more than 255 blocks of output while this asks for one. The
    /// two `expect`s below say so and carry no secret.
    #[must_use]
    pub fn stretch(&self) -> SymmetricKey {
        let hkdf = Hkdf::<Sha256>::from_prk(&*self.0)
            .expect("a 32-byte PRK is exactly SHA-256's output length");
        let mut enc = Zeroizing::new([0u8; KEY_LEN]);
        let mut mac = Zeroizing::new([0u8; KEY_LEN]);
        hkdf.expand(b"enc", &mut *enc).expect("32 bytes is one HKDF block");
        hkdf.expand(b"mac", &mut *mac).expect("32 bytes is one HKDF block");
        SymmetricKey { enc, mac }
    }

    /// The **password hash the server is given**, base64'd: one iteration of
    /// PBKDF2-HMAC-SHA256 with the *master key* as the input and the *master
    /// password* as the salt.
    ///
    /// # The inputs are the other way round from [`master_key`], and that is
    /// the whole point
    ///
    /// [`master_key`] hashes the **password** salted by the **email**, tens
    /// or hundreds of thousands of times. This hashes the **master key**
    /// salted by the **password**, once. Swapping them, or reusing the
    /// account's iteration count here, produces a value the server rejects as
    /// a wrong password with no hint that the code, not the user, is wrong --
    /// which is why this is a separate function with its own name rather than
    /// a parameter on the other one.
    ///
    /// One iteration is not a weakness. The input is already a KDF output
    /// with the account's full work factor behind it, so there is nothing
    /// left for a second iteration to slow down; what this step buys is that
    /// the *server* never sees the master key, only a value derived from it
    /// that cannot be turned back into the key that decrypts the vault. The
    /// server hashes it again, slowly, before storing it.
    ///
    /// **Bitwarden calls this the `ServerAuthorization` purpose.** There is a
    /// second purpose (`LocalAuthorization`, two iterations) used to check a
    /// password offline against a stored hash. This crate has no such store
    /// and does not implement it: an enum with one reachable arm is a worse
    /// thing to have than one function that says what it is for.
    ///
    /// Verified against **Bitwarden's own published test vectors** -- see
    /// `the_password_hash_matches_bitwardens_own_vector_for_pbkdf2` and its
    /// Argon2id sibling below. Those two are the strongest external
    /// statements this module has: they pin the master-key derivation *and*
    /// this hash together, end to end, against values Bitwarden's own client
    /// asserts on.
    #[must_use]
    pub fn password_hash(&self, password: &[u8]) -> Zeroizing<String> {
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        pbkdf2::pbkdf2_hmac::<Sha256>(&*self.0, password, 1, &mut *out);
        let mut text = Zeroizing::new(String::new());
        crate::record::seal::base64_into(&mut text, &*out);
        text
    }
}

// ---- EncString -------------------------------------------------------------

/// Bitwarden's `type.iv|ct|mac` wire format, parsed.
///
/// # Parsed strictly, and never guessed at
///
/// This is a reader of text that arrives from a server, so every ambiguity is
/// a refusal: the type byte must be digits, the number of `|`-separated parts
/// must be exactly what that type has, each part must be exactly standard
/// base64 (through [`base64_from`], which refuses a stray character rather
/// than skipping it), the IV must be 16 bytes, the MAC 32, and the ciphertext
/// a non-zero whole number of AES blocks.
///
/// A parser that repairs its input is a parser that will one day accept an
/// `EncString` whose MAC covers less than its ciphertext.
///
/// # Only two types are accepted
///
/// Type 2 (`AesCbc256_HmacSha256_B64`) is everything symmetric in a modern
/// vault, and type 4 (`Rsa2048_OaepSha1_B64`) is how an organisation's key is
/// wrapped for a member. The rest are refused **by name** -- see
/// [`EncString::from_str`] -- rather than by number, because "type 0 is not
/// supported" does not tell a reader that type 0 is unauthenticated AES-CBC
/// with no MAC at all.
#[derive(Clone, PartialEq, Eq)]
pub enum EncString {
    /// Type 2. AES-256-CBC with a 16-byte IV, authenticated by
    /// HMAC-SHA256 over `iv || ct`.
    AesCbc256HmacSha256B64 { iv: [u8; BLOCK], ct: Vec<u8>, mac: [u8; KEY_LEN] },
    /// Type 4. RSA-2048 OAEP with SHA-1. One part and no MAC: OAEP is not a
    /// MAC but it is not a malleable padding either, and this is the format
    /// organisation keys are actually stored in.
    Rsa2048OaepSha1B64 { ct: Vec<u8> },
}

/// Redacting, in the house style of [`crate::record::seal::SealedSeed`].
///
/// A ciphertext is not a plaintext, but it is the whole of what an offline
/// attack is run against; printing one into a log file hands over the target
/// and saves the theft. Lengths only.
impl std::fmt::Debug for EncString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AesCbc256HmacSha256B64 { ct, .. } => {
                write!(f, "EncString::AesCbc256HmacSha256B64(<{} ciphertext bytes>)", ct.len())
            }
            Self::Rsa2048OaepSha1B64 { ct } => {
                write!(f, "EncString::Rsa2048OaepSha1B64(<{} ciphertext bytes>)", ct.len())
            }
        }
    }
}

/// The wire form: `type.iv|ct|mac`, the exact text
/// [`EncString::from_str`] parses.
///
/// # `Display`, not `Debug`, and the difference is deliberate
///
/// [`EncString`]'s `Debug` prints lengths only, because a `Debug` is what
/// reaches a log line by accident. `Display` is what a caller asks for on
/// purpose when it is about to put the value in a request body, and a
/// ciphertext that cannot be written out is a ciphertext that cannot be
/// stored. The two impls disagree on purpose; neither prints a plaintext or
/// a key, which is the invariant that actually matters.
///
/// # Round-tripping is the contract
///
/// `EncString::from_str(&value.to_string()) == Ok(value)` for every value
/// this module can construct, and
/// `the_rendered_wire_string_parses_back_to_an_equal_value` holds it there.
/// The parser refuses non-standard base64, a short IV and a ciphertext that
/// is not a whole number of blocks; this writer emits standard padded base64
/// through the same [`crate::record::seal`] encoder the parser's
/// [`base64_from`] inverts, so the two cannot drift.
impl std::fmt::Display for EncString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Built in one owned `String` rather than written piecewise into the
        // formatter, because `base64_into` pushes into a `String` and this
        // way the encoder is the single one in the crate.
        let mut out = String::new();
        match self {
            Self::AesCbc256HmacSha256B64 { iv, ct, mac } => {
                out.push_str("2.");
                base64_into(&mut out, iv);
                out.push('|');
                base64_into(&mut out, ct);
                out.push('|');
                base64_into(&mut out, mac);
            }
            Self::Rsa2048OaepSha1B64 { ct } => {
                out.push_str("4.");
                base64_into(&mut out, ct);
            }
        }
        f.write_str(&out)
    }
}

/// The Bitwarden name of every `EncString` type this module refuses, so the
/// refusal can say which format it saw.
///
/// A table rather than a `match` with the names inline, so that
/// [`EncString::from_str`]'s unknown-type arm and this list cannot drift into
/// disagreeing about which numbers are defined at all.
const REFUSED_TYPES: [(u8, &str); 5] = [
    // No MAC whatsoever: unauthenticated CBC, malleable, and the reason
    // padding-oracle attacks have a name. Bitwarden deprecated it.
    (0, "AesCbc256_B64"),
    // AES-128 under a key half the size of everything else in the vault.
    (1, "AesCbc128_HmacSha256_B64"),
    (3, "Rsa2048_OaepSha256_B64"),
    (5, "Rsa2048_OaepSha256_HmacSha256_B64"),
    (6, "Rsa2048_OaepSha1_HmacSha256_B64"),
];

impl std::str::FromStr for EncString {
    type Err = CryptoError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (type_text, rest) =
            text.split_once('.').ok_or(CryptoError::Malformed("no `.` between type and body"))?;
        if type_text.is_empty() || !type_text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CryptoError::Malformed("the type is not a decimal number"));
        }
        let kind: u8 =
            type_text.parse().map_err(|_| CryptoError::Malformed("the type is out of range"))?;
        if let Some((_, name)) = REFUSED_TYPES.iter().find(|(n, _)| *n == kind) {
            return Err(CryptoError::Unsupported(name));
        }
        let parts: Vec<&str> = rest.split('|').collect();
        match kind {
            2 => {
                let [iv, ct, mac] = parts.as_slice() else {
                    return Err(CryptoError::Malformed(
                        "AesCbc256_HmacSha256_B64 needs exactly three `|`-separated parts",
                    ));
                };
                let iv = fixed(iv, "the IV is not 16 bytes")?;
                let mac = fixed(mac, "the MAC is not 32 bytes")?;
                let ct =
                    base64_from(ct).ok_or(CryptoError::Malformed("the ciphertext is not base64"))?;
                if ct.is_empty() || !ct.len().is_multiple_of(BLOCK) {
                    return Err(CryptoError::Malformed(
                        "the ciphertext is not a non-zero whole number of AES blocks",
                    ));
                }
                Ok(Self::AesCbc256HmacSha256B64 { iv, ct, mac })
            }
            4 => {
                let [ct] = parts.as_slice() else {
                    return Err(CryptoError::Malformed(
                        "Rsa2048_OaepSha1_B64 has no `|`-separated parts",
                    ));
                };
                let ct =
                    base64_from(ct).ok_or(CryptoError::Malformed("the ciphertext is not base64"))?;
                if ct.is_empty() {
                    return Err(CryptoError::Malformed("the ciphertext is empty"));
                }
                Ok(Self::Rsa2048OaepSha1B64 { ct })
            }
            _ => Err(CryptoError::UnknownType),
        }
    }
}

/// Decodes one base64 part into an array of exactly `N` bytes, or refuses.
fn fixed<const N: usize>(text: &str, wrong_length: &'static str) -> Result<[u8; N], CryptoError> {
    let bytes = base64_from(text).ok_or(CryptoError::Malformed("a part is not base64"))?;
    bytes.try_into().map_err(|_| CryptoError::Malformed(wrong_length))
}

// ---- symmetric decryption --------------------------------------------------

/// Decrypts a type-2 `EncString` under `key`.
///
/// # The MAC is verified **before** anything is decrypted
///
/// The order is the whole point of this function, so it is written as
/// straight-line code with an early return rather than as anything a later
/// edit could reorder by accident: the HMAC over `iv || ct` is computed and
/// compared first, and the CBC decryptor is not even constructed unless it
/// matched. Verifying afterwards -- or comparing with `==`, which returns on
/// the first differing byte and leaks the position of that byte through
/// timing -- is the classic defect in exactly this composition.
///
/// The comparison is [`hmac::Mac::verify_slice`], which is
/// `subtle::ConstantTimeEq` underneath. That is the reason this file names no
/// `subtle` dependency of its own and the reason there is no `==` on a MAC
/// anywhere in it.
pub fn decrypt(key: &SymmetricKey, enc: &EncString) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let EncString::AesCbc256HmacSha256B64 { iv, ct, mac } = enc else {
        return Err(CryptoError::Unsupported(
            "Rsa2048_OaepSha1_B64 needs a private key, not a symmetric one",
        ));
    };

    let mut hmac = <HmacSha256 as Mac>::new_from_slice(&*key.mac)
        .expect("HMAC accepts a key of any length");
    hmac.update(iv);
    hmac.update(ct);
    hmac.verify_slice(mac).map_err(|_| CryptoError::MacMismatch)?;

    let plain = cbc_decrypt_raw(&key.enc, iv, ct)?;
    strip_pkcs7(plain)
}

/// AES-256-CBC decryption with **no** padding handling: whole blocks in,
/// whole blocks out.
///
/// Separate from the padding strip above it so that
/// `aes_256_cbc_matches_nist_sp_800_38a` can drive it with NIST's own
/// key, IV, ciphertext and plaintext -- none of which carry PKCS#7 padding,
/// and all four of which would otherwise have to be manufactured here.
fn cbc_decrypt_raw(
    key: &[u8; KEY_LEN],
    iv: &[u8; BLOCK],
    ct: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if ct.is_empty() || !ct.len().is_multiple_of(BLOCK) {
        return Err(CryptoError::Malformed(
            "the ciphertext is not a non-zero whole number of AES blocks",
        ));
    }
    let mut buf = Zeroizing::new(ct.to_vec());
    let len = buf.len();
    Aes256CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf[..len])
        .map_err(|_| CryptoError::Malformed("the ciphertext is not a whole number of AES blocks"))?;
    Ok(buf)
}

/// Strips PKCS#7 padding, refusing anything malformed.
///
/// **Reached only after a MAC that verified**, which is what makes a plain
/// byte-by-byte check safe here: an attacker who cannot forge the MAC cannot
/// submit a ciphertext that reaches this function at all, so there is no
/// padding oracle to time. That property is a fact about [`decrypt`]'s
/// ordering, not about this function, which is why it is stated at both.
fn strip_pkcs7(mut plain: Zeroizing<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let pad = usize::from(*plain.last().ok_or(CryptoError::Padding)?);
    if pad == 0 || pad > BLOCK || pad > plain.len() {
        return Err(CryptoError::Padding);
    }
    if plain[plain.len() - pad..].iter().any(|b| usize::from(*b) != pad) {
        return Err(CryptoError::Padding);
    }
    let keep = plain.len() - pad;
    plain.truncate(keep);
    Ok(plain)
}

// ---- symmetric encryption --------------------------------------------------

/// Encrypts `plain` under `key` into a type-2 `EncString`.
///
/// The other half of [`decrypt`], and it is written to be read next to it:
/// what that function verifies, this function must produce, and every
/// property below exists because the reverse of it is invisible in a passing
/// test and fatal in a real vault.
///
/// # A fresh random IV, every call, from the operating system
///
/// The IV is 16 bytes straight from [`getrandom`], which is the OS CSPRNG --
/// `BCryptGenRandom` on Windows, `getrandom(2)` on Linux -- and is the same
/// source [`crate::record::seal`] and [`crate::vault_disk_cache`] use for
/// their nonces. It is **not** a counter, not derived from the key, not
/// derived from the plaintext, and not a constant.
///
/// That is not a style preference. CBC's first block is
/// `E(plaintext_block ^ iv)`, so two encryptions under one key with one IV
/// produce identical leading ciphertext for exactly as long as the two
/// plaintexts agree -- an observer who never breaks AES still learns that
/// two vault items share a prefix, and, byte-block by byte-block, where they
/// stop sharing it. A password changed from `hunter2` to `hunter3` is
/// visibly a one-character edit.
///
/// A deterministic IV is also the failure that no round-trip test can see:
/// `decrypt(encrypt(x)) == x` holds perfectly with the IV nailed to zero.
/// The test that catches it is
/// `two_encryptions_of_one_plaintext_differ_in_both_iv_and_ciphertext`,
/// which encrypts the same bytes twice and asserts the two IVs *and* the two
/// ciphertexts differ. It is a statistical test -- two OS-random 16-byte IVs
/// collide with probability 2^-128 -- and it is statistical on purpose,
/// because the alternative is a test-only hook that lets a caller choose the
/// IV, and a seam in the randomness path is a worse thing to own than a test
/// that could theoretically flake once per universe.
///
/// If the OS will not produce randomness, this returns [`CryptoError::Rng`]
/// and encrypts nothing. There is no fallback IV, because every fallback IV
/// anyone has ever written is a repeated one.
///
/// # Encrypt, then MAC -- in that order, over the ciphertext
///
/// AES-256-CBC under `key.enc` first; HMAC-SHA256 under `key.mac` over
/// `iv || ct` second. Not MAC-then-encrypt, not MAC-over-the-plaintext, and
/// not one key for both. [`decrypt`] verifies the MAC before it constructs a
/// cipher, and that ordering is only worth anything if this side authenticated
/// the *ciphertext* the other side is about to be handed. MAC-then-encrypt
/// would leave `decrypt` no way to reject a forged ciphertext without first
/// decrypting it, which is the padding oracle the module docs describe.
///
/// The IV is inside the MAC, so a flipped IV byte -- which in CBC flips the
/// corresponding bit of the *first plaintext block* and nothing else -- is
/// caught rather than silently delivered.
///
/// # PKCS#7, including the block that looks redundant
///
/// The plaintext is padded to a whole number of blocks, and an input that is
/// already an exact multiple of 16 gains a **full extra block** of `0x10`
/// bytes. That is not an inefficiency to special-case away: without it the
/// last byte of a block-aligned plaintext would be indistinguishable from a
/// padding length, and [`strip_pkcs7`] would truncate real data. Length 0 is
/// therefore one block, not zero -- which also satisfies the parser's refusal
/// of an empty ciphertext.
///
/// # What is wiped
///
/// The padded copy of the plaintext lives in a [`Zeroizing`] buffer, in the
/// module's house style: a padded second copy of a password left for the
/// allocator is a second home for it that nothing wipes. (The cipher encrypts
/// in place, so by the time that buffer is dropped it holds ciphertext -- the
/// `Zeroizing` covers the window between the copy and the encryption, which
/// is the window a crash dump would catch it in.)
///
/// # Type 2 only
///
/// There is no `kind` parameter. Type 4 is RSA and needs a *public* key,
/// which this module does not hold; the refused types are refused for
/// [`REFUSED_TYPES`]'s reasons and writing one would be a downgrade this
/// code should not be able to express. A function that can only produce the
/// one good format cannot be talked into producing a bad one.
pub fn encrypt(key: &SymmetricKey, plain: &[u8]) -> Result<EncString, CryptoError> {
    let mut iv = [0u8; BLOCK];
    getrandom::getrandom(&mut iv).map_err(|_| CryptoError::Rng)?;
    encrypt_with_iv(key, iv, plain)
}

/// [`encrypt`] with the IV handed in rather than generated.
///
/// Split out so that the *randomness* is in exactly one place -- the four
/// lines above -- and everything downstream of it is one function that a
/// fixture builder can also call. `tests::seal` is that caller: it wants a
/// reproducible `EncString`, and the alternative to this split is a second
/// encryptor living in the test module, drifting away from the real one
/// block by block until the fixtures stop resembling what production writes.
///
/// It is **not** a test seam. It has no `cfg` attribute, it is private, and
/// [`encrypt`] is its only caller outside the test module -- the same shape
/// as [`cbc_decrypt_raw`]. What a seam would look like is a public or
/// `cfg(test)`-gated way for a *caller of `encrypt`* to choose the IV, and
/// that does not exist: nothing outside this file can reach this function,
/// so no production path can be talked into a fixed IV.
fn encrypt_with_iv(
    key: &SymmetricKey,
    iv: [u8; BLOCK],
    plain: &[u8],
) -> Result<EncString, CryptoError> {
    // One whole block of headroom, always -- `BLOCK - len % BLOCK` is in
    // `1..=BLOCK`, never 0, which is PKCS#7's rule and the reason a
    // block-aligned input grows by a full block. Every added byte is the pad
    // length itself, which is what [`strip_pkcs7`] checks on the way back.
    let pad = BLOCK - plain.len() % BLOCK;
    let fill = u8::try_from(pad).expect("a pad length is in 1..=16");
    let mut buffer = Zeroizing::new(vec![fill; plain.len() + pad]);
    buffer[..plain.len()].copy_from_slice(plain);

    let len = buffer.len();
    cbc_encrypt_raw(&key.enc, &iv, &mut buffer[..len])?;
    let ct = buffer.to_vec();

    let mut hmac = <HmacSha256 as Mac>::new_from_slice(&*key.mac)
        .expect("HMAC accepts a key of any length");
    hmac.update(&iv);
    hmac.update(&ct);
    let mac = hmac.finalize().into_bytes().into();

    Ok(EncString::AesCbc256HmacSha256B64 { iv, ct, mac })
}

/// AES-256-CBC encryption with **no** padding handling: whole blocks in,
/// whole blocks out, in place.
///
/// The mirror of [`cbc_decrypt_raw`], and separate from PKCS#7 for the same
/// reason: `aes_256_cbc_encrypt_matches_nist_sp_800_38a` drives it with
/// NIST's own key, IV and plaintext and compares against NIST's own
/// ciphertext, none of which carry PKCS#7 padding. Without this split the
/// forward direction could only be checked by round-tripping through this
/// file's own decryptor, which cannot see a defect that is symmetric in both
/// halves.
fn cbc_encrypt_raw(
    key: &[u8; KEY_LEN],
    iv: &[u8; BLOCK],
    buf: &mut [u8],
) -> Result<(), CryptoError> {
    if buf.is_empty() || !buf.len().is_multiple_of(BLOCK) {
        return Err(CryptoError::Malformed(
            "the padded plaintext is not a non-zero whole number of AES blocks",
        ));
    }
    let len = buf.len();
    Aes256CbcEnc::new(key.into(), iv.into())
        .encrypt_padded_mut::<NoPadding>(buf, len)
        .map_err(|_| {
            CryptoError::Malformed("the padded plaintext is not a whole number of AES blocks")
        })?;
    Ok(())
}

// ---- the two unwraps -------------------------------------------------------

/// Unwraps the user's symmetric key -- the account's **protected symmetric
/// key**, as the server calls it -- with the stretched master key.
///
/// This is the hinge of the whole scheme: everything else in a vault is
/// encrypted under what comes out of here, and the master key is used for
/// nothing else. Changing the master password re-wraps this key rather than
/// re-encrypting the vault, which is why it can be done in one request.
pub fn unwrap_user_key(
    stretched: &SymmetricKey,
    protected: &EncString,
) -> Result<SymmetricKey, CryptoError> {
    let plain = decrypt(stretched, protected)?;
    SymmetricKey::from_64(&plain)
}

/// Unwraps an **organisation's** symmetric key with the user's RSA private
/// key, which the caller has already decrypted under the user's own key.
///
/// The private key is PKCS#8 DER -- the bytes Bitwarden stores, once its own
/// `EncString` wrapper has been removed with [`decrypt`].
pub fn unwrap_org_key(
    private_key_pkcs8_der: &[u8],
    protected: &EncString,
) -> Result<SymmetricKey, CryptoError> {
    let plain = decrypt_rsa(private_key_pkcs8_der, protected)?;
    SymmetricKey::from_64(&plain)
}

/// RSA-OAEP-SHA1 decryption of a type-4 `EncString`.
///
/// # One honest caveat about what is wiped
///
/// The returned buffer is [`Zeroizing`] and [`RsaPrivateKey`] zeroizes itself
/// on drop, but the `rsa` crate allocates and returns a plain `Vec<u8>` that
/// this function copies into the wiped one, and that intermediate is outside
/// this crate's control. It is one org key, once per session. It is recorded
/// here rather than left for a reader to assume otherwise.
pub fn decrypt_rsa(
    private_key_pkcs8_der: &[u8],
    enc: &EncString,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let EncString::Rsa2048OaepSha1B64 { ct } = enc else {
        return Err(CryptoError::Unsupported(
            "AesCbc256_HmacSha256_B64 needs a symmetric key, not a private one",
        ));
    };
    let key =
        RsaPrivateKey::from_pkcs8_der(private_key_pkcs8_der).map_err(|_| CryptoError::Rsa)?;
    let plain = key.decrypt(Oaep::new::<sha1::Sha1>(), ct).map_err(|_| CryptoError::Rsa)?;
    Ok(Zeroizing::new(plain))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::str::FromStr;

    // ---- fixtures shared with `rest::sync`'s tests -------------------------
    //
    // `pub` on this module, and on the six items below, exists for one
    // reason: the sync mapper's tests need to *build* ciphertext, and every
    // field this type has is private to this file. The alternatives were both
    // worse -- widening `SymmetricKey`'s production API with a
    // bytes-out accessor nothing in the app would call, or a `cfg(test)` seam
    // in production code, which this crate bans outright. Test helpers
    // sharing a `pub mod tests` is neither: no production item changed
    // visibility, and nothing outside a test build compiles.
    //
    // Nothing here is reachable from the shipped binary.

    /// Hex to bytes, for transcribing a published vector unchanged.
    pub fn hex(text: &str) -> Vec<u8> {
        assert!(text.len().is_multiple_of(2), "a hex vector has an even number of digits");
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("hex digits"))
            .collect()
    }

    /// Standard base64 with padding. The inverse of the crate's
    /// [`base64_from`], used only to *build* the `EncString` text a fixture
    /// is parsed from.
    fn b64(bytes: &[u8]) -> String {
        let mut out = String::new();
        crate::record::seal::base64_into(&mut out, bytes);
        out
    }

    #[test]
    fn the_hex_helper_is_not_the_thing_under_test() {
        // Control: every vector below is transcribed through `hex`, so a
        // broken `hex` would make all of them agree on the wrong bytes.
        assert_eq!(hex(""), Vec::<u8>::new());
        assert_eq!(hex("00ff10"), vec![0x00, 0xff, 0x10]);
        assert_ne!(hex("0102"), hex("0201"));
    }

    // ---- the primitives, against published vectors ------------------------

    /// **RFC 7914 (scrypt), section 11**, which is where the IETF publishes
    /// PBKDF2-HMAC-SHA-256 vectors. RFC 6070's vectors are PBKDF2-HMAC-**SHA1**
    /// and cannot check this code at all -- a point worth writing down,
    /// because RFC 6070 is what gets cited for "PBKDF2 test vectors".
    #[test]
    fn pbkdf2_hmac_sha256_matches_rfc_7914() {
        let mut out = [0u8; 64];
        pbkdf2::pbkdf2_hmac::<Sha256>(b"passwd", b"salt", 1, &mut out);
        assert_eq!(
            out.to_vec(),
            hex(concat!(
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc",
                "49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
            ))
        );

        let mut out = [0u8; 64];
        pbkdf2::pbkdf2_hmac::<Sha256>(b"Password", b"NaCl", 80_000, &mut out);
        assert_eq!(
            out.to_vec(),
            hex(concat!(
                "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56",
                "a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d"
            ))
        );
    }

    /// **RFC 5869, appendix A.1** (basic test case with SHA-256).
    ///
    /// The expand half only, and that is the half this module uses: Bitwarden
    /// feeds the master key in as the PRK. The PRK and OKM below are the
    /// RFC's own, so this checks `Hkdf::from_prk(..).expand(..)` -- the exact
    /// call [`MasterKey::stretch`] makes -- rather than a composition of
    /// extract and expand that this code never performs.
    #[test]
    fn hkdf_sha256_expand_matches_rfc_5869() {
        let prk = hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");
        let info = hex("f0f1f2f3f4f5f6f7f8f9");
        let mut okm = [0u8; 42];
        Hkdf::<Sha256>::from_prk(&prk).expect("32-byte PRK").expand(&info, &mut okm).expect("42 bytes");
        assert_eq!(
            okm.to_vec(),
            hex(concat!(
                "3cb25f25faacd57a90434f64d0362f2a",
                "2d2d0a90cf1a5a4c5db02d56ecc4c5bf",
                "34007208d5b887185865"
            ))
        );
    }

    /// **RFC 4231, test cases 1 and 2** (HMAC-SHA-256).
    ///
    /// This checks the MAC primitive itself. That
    /// [`decrypt`] computes it over `iv || ct` and nothing else is a
    /// different claim, checked separately below.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&[0x0b; 20]).expect("any key length");
        mac.update(b"Hi There");
        assert_eq!(
            mac.finalize().into_bytes().to_vec(),
            hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
        );

        let mut mac = <HmacSha256 as Mac>::new_from_slice(b"Jefe").expect("any key length");
        mac.update(b"what do ya want for nothing?");
        assert_eq!(
            mac.finalize().into_bytes().to_vec(),
            hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
    }

    /// **NIST SP 800-38A, appendix F.2.6** (CBC-AES256.Decrypt).
    ///
    /// Four blocks, no padding -- which is why [`cbc_decrypt_raw`] is its own
    /// function: the vector's plaintext is NIST's, not something this file
    /// manufactured, and PKCS#7 would have forced a fifth block of padding
    /// that no published vector covers.
    #[test]
    fn aes_256_cbc_matches_nist_sp_800_38a() {
        let key: [u8; 32] = hex(NIST_KEY).try_into().expect("32 bytes");
        let iv: [u8; 16] = hex(NIST_IV).try_into().expect("16 bytes");
        let plain = cbc_decrypt_raw(&key, &iv, &hex(NIST_CIPHERTEXT)).expect("four whole blocks");
        assert_eq!(plain.to_vec(), hex(NIST_PLAINTEXT));

        // Control: the vector is not decrypting to itself, and a wrong key
        // does not quietly produce the right answer.
        assert_ne!(hex(NIST_CIPHERTEXT), hex(NIST_PLAINTEXT));
        let wrong = cbc_decrypt_raw(&[0u8; 32], &iv, &hex(NIST_CIPHERTEXT)).expect("still blocks");
        assert_ne!(wrong.to_vec(), hex(NIST_PLAINTEXT));
    }

    /// **NIST SP 800-38A, appendix F.2.5** (CBC-AES256.Encrypt) -- the
    /// forward direction of the vector above, against the same published
    /// key, IV, plaintext and ciphertext.
    ///
    /// The reason this exists separately from
    /// `encrypt_round_trips_through_this_modules_own_decrypt`: a round trip
    /// through this file's own decryptor cannot see a defect that is
    /// symmetric in both halves -- a swapped key schedule, a mode that is
    /// consistently not CBC, an IV applied to the wrong end -- because both
    /// halves would agree on the same wrong answer and the plaintext would
    /// come back intact. Only NIST's ciphertext can say that what this
    /// module *emits* is what another client will read.
    ///
    /// It pins [`cbc_encrypt_raw`], which is the AES-CBC step of
    /// [`encrypt`]. It does **not** pin the PKCS#7 padding that `encrypt`
    /// wraps around it, because NIST's plaintext is four whole blocks and no
    /// published vector pads it; that gap is recorded in
    /// `the_composition_is_still_not_fully_pinned_and_here_is_what_is_left`.
    #[test]
    fn aes_256_cbc_encrypt_matches_nist_sp_800_38a() {
        let key: [u8; 32] = hex(NIST_KEY).try_into().expect("32 bytes");
        let iv: [u8; 16] = hex(NIST_IV).try_into().expect("16 bytes");

        let mut buf = hex(NIST_PLAINTEXT);
        cbc_encrypt_raw(&key, &iv, &mut buf).expect("four whole blocks");
        assert_eq!(
            buf,
            hex(NIST_CIPHERTEXT),
            "AES-256-CBC encryption does not agree with NIST SP 800-38A F.2.5"
        );

        // Control: a wrong key does not quietly produce the right answer,
        // and neither does a wrong IV -- which would be invisible in a round
        // trip, since the same wrong IV would undo it.
        let mut wrong_key = hex(NIST_PLAINTEXT);
        cbc_encrypt_raw(&[0u8; 32], &iv, &mut wrong_key).expect("still blocks");
        assert_ne!(wrong_key, hex(NIST_CIPHERTEXT));

        let mut wrong_iv = hex(NIST_PLAINTEXT);
        cbc_encrypt_raw(&key, &[0u8; 16], &mut wrong_iv).expect("still blocks");
        assert_ne!(wrong_iv, hex(NIST_CIPHERTEXT));
    }

    /// NIST SP 800-38A appendix F.2.5/F.2.6's key, IV, plaintext and
    /// ciphertext. At module scope because the `EncString` fixture below
    /// reuses them -- so that its ciphertext and its expected plaintext both
    /// come from NIST rather than from this file's own encryptor.
    const NIST_KEY: &str = "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4";
    const NIST_IV: &str = "000102030405060708090a0b0c0d0e0f";
    const NIST_PLAINTEXT: &str = concat!(
        "6bc1bee22e409f96e93d7e117393172a",
        "ae2d8a571e03ac9c9eb76fac45af8e51",
        "30c81c46a35ce411e5fbc1191a0a52ef",
        "f69f2445df4f9b17ad2b417be66c3710"
    );
    const NIST_CIPHERTEXT: &str = concat!(
        "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
        "9cfc4e967edb808d679f777bc6702c7d",
        "39f23369a9d9bacfa530e26304231461",
        "b2eb05e2c39be9fcda6c19078c6a9d1b"
    );

    /// **RFC 9106, section 5.3** (Argon2id test vector): v=0x13, t=3,
    /// m=32 KiB, p=4, a 32-byte password of `0x01`, a 16-byte salt of `0x02`,
    /// an 8-byte secret of `0x03`, 12 bytes of associated data `0x04`, and a
    /// 32-byte tag.
    ///
    /// Driven through `argon2` directly rather than through [`master_key`],
    /// because the RFC's vector uses a secret and associated data that
    /// Bitwarden's derivation has neither of. What it pins is that the
    /// algorithm, version and parameter *meanings* this module passes are the
    /// ones the RFC defines -- in particular that `m_cost` is kibibytes and
    /// that [`Version::V0x13`] is the version an existing account was
    /// migrated under.
    #[test]
    fn argon2id_matches_rfc_9106() {
        let params = ParamsBuilder::new()
            .m_cost(32)
            .t_cost(3)
            .p_cost(4)
            .output_len(32)
            .data(
                argon2::AssociatedData::new(&[0x04; 12]).expect("12 bytes of associated data"),
            )
            .build()
            .expect("the RFC's own parameters");
        let argon =
            Argon2::new_with_secret(&[0x03; 8], Algorithm::Argon2id, Version::V0x13, params)
                .expect("an 8-byte secret");
        let mut tag = [0u8; 32];
        argon.hash_password_into(&[0x01; 32], &[0x02; 16], &mut tag).expect("the RFC's inputs");
        assert_eq!(
            tag.to_vec(),
            hex("0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659")
        );
    }

    /// The parameters really do reach Argon2: the same password and salt
    /// under two different cost settings must not produce the same key.
    ///
    /// Without this, [`master_key`]'s Argon2 arm could ignore `memory_mib`
    /// entirely and `argon2id_matches_rfc_9106` -- which does not call it --
    /// would stay green.
    ///
    /// The figures are 1 and 2 MiB rather than anything realistic on purpose:
    /// this test is about the parameters *arriving*, not about a cost, and a
    /// realistic 64 MiB here would allocate sixty-four megabytes three times
    /// for a property one megabyte proves exactly as well.
    #[test]
    fn the_argon2_parameters_this_module_passes_are_not_ignored() {
        let a = master_key(
            b"correct horse",
            "user@example.com",
            Kdf::Argon2id { iterations: 2, memory_mib: 1, parallelism: 1 },
        )
        .expect("usable parameters");
        let b = master_key(
            b"correct horse",
            "user@example.com",
            Kdf::Argon2id { iterations: 3, memory_mib: 1, parallelism: 1 },
        )
        .expect("usable parameters");
        let c = master_key(
            b"correct horse",
            "user@example.com",
            Kdf::Argon2id { iterations: 2, memory_mib: 2, parallelism: 1 },
        )
        .expect("usable parameters");
        assert_ne!(a, b, "the iteration count is not reaching Argon2");
        assert_ne!(a, c, "the memory cost is not reaching Argon2");
    }

    // ---- the salt, and the KDF wrapper ------------------------------------

    /// [`master_key`]'s PBKDF2 arm is PBKDF2 over the lowercased, trimmed
    /// email -- checked against the vector-verified primitive directly, so
    /// this pins the *wiring* rather than re-checking PBKDF2.
    #[test]
    fn the_pbkdf2_arm_salts_with_the_normalised_email() {
        let derived = master_key(b"pa55word", "  User@Example.COM  ", Kdf::Pbkdf2 { iterations: 7 })
            .expect("a non-zero iteration count");
        let mut expected = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(b"pa55word", b"user@example.com", 7, &mut expected);
        assert_eq!(*derived.0, expected);

        // Control: a different email is a different key, so the salt is
        // reaching PBKDF2 at all.
        let other = master_key(b"pa55word", "other@example.com", Kdf::Pbkdf2 { iterations: 7 })
            .expect("a non-zero iteration count");
        assert_ne!(derived, other);
    }

    #[test]
    fn a_zero_iteration_count_is_refused_rather_than_derived_from() {
        assert_eq!(
            master_key(b"pa55word", "user@example.com", Kdf::Pbkdf2 { iterations: 0 }),
            Err(CryptoError::KdfParams("iteration count is zero"))
        );
    }

    #[test]
    fn unusable_argon2_parameters_are_refused_rather_than_panicked_on() {
        // Zero lanes is outside what Argon2 defines; `ParamsBuilder` refuses
        // it, and this module must turn that into an error rather than an
        // unwrap.
        assert!(matches!(
            master_key(
                b"pa55word",
                "user@example.com",
                Kdf::Argon2id { iterations: 1, memory_mib: 1, parallelism: 0 }
            ),
            Err(CryptoError::KdfParams(_))
        ));
    }

    /// The two KDFs are not accidentally the same function: the same password
    /// and email under each must give different keys.
    #[test]
    fn the_two_kdfs_are_not_the_same_derivation() {
        let pbkdf2 = master_key(b"pa55word", "user@example.com", Kdf::Pbkdf2 { iterations: 3 })
            .expect("usable");
        let argon = master_key(
            b"pa55word",
            "user@example.com",
            Kdf::Argon2id { iterations: 1, memory_mib: 1, parallelism: 1 },
        )
        .expect("usable");
        assert_ne!(pbkdf2, argon);
    }

    // ---- stretching -------------------------------------------------------

    /// The stretch is HKDF-expand with `enc` and `mac`, and the two halves
    /// are different keys -- which is the property that matters, because a
    /// scheme where the encryption key and the MAC key are equal is one where
    /// the MAC proves nothing an attacker holding the encryption key could
    /// not forge.
    #[test]
    fn stretching_produces_two_different_keys_from_the_hkdf_the_rfc_pinned() {
        let master = MasterKey(Zeroizing::new([7u8; 32]));
        let stretched = master.stretch();
        assert_ne!(stretched.enc, stretched.mac);

        let hkdf = Hkdf::<Sha256>::from_prk(&[7u8; 32]).expect("32-byte PRK");
        let mut enc = [0u8; 32];
        let mut mac = [0u8; 32];
        hkdf.expand(b"enc", &mut enc).expect("one block");
        hkdf.expand(b"mac", &mut mac).expect("one block");
        assert_eq!(*stretched.enc, enc, "the encryption key is not HKDF-expand with info `enc`");
        assert_eq!(*stretched.mac, mac, "the MAC key is not HKDF-expand with info `mac`");
    }

    // ---- EncString parsing ------------------------------------------------

    /// A valid type-2 string, assembled from the NIST vector so that its
    /// ciphertext is not this file's invention.
    /// A [`SymmetricKey`] from 64 known bytes: the first 32 the encryption
    /// key, the last 32 the MAC key. The split
    /// `a_protected_keys_plaintext_is_the_encryption_key_then_the_mac_key`
    /// pins against Bitwarden's own vector.
    pub fn key_from_64(bytes: &[u8; SYMMETRIC_KEY_LEN]) -> SymmetricKey {
        SymmetricKey::from_64(bytes).expect("64 bytes is exactly a symmetric key")
    }

    /// Seals bytes into a type-2 `EncString`, the way a Bitwarden server's
    /// stored value looks.
    ///
    /// **This is production [`encrypt`]'s output, not a second
    /// implementation of it.** It calls [`encrypt_with_iv`] -- the whole of
    /// `encrypt` below the IV generation -- and renders the result with
    /// [`EncString`]'s own `Display`, so a fixture and the real encryptor
    /// cannot drift apart. It used to be a separate encryptor, written when
    /// this module was decrypt-only; that is no longer true and the copy is
    /// gone.
    ///
    /// It still proves nothing about whether *Bitwarden* arranges an
    /// `EncString` this way -- that question is answered, as far as it can
    /// be, by the published vectors above, and what is left of it is listed
    /// in `the_composition_is_still_not_fully_pinned_and_here_is_what_is_left`.
    /// What it is for is building a payload the *mapper* can be tested
    /// against.
    ///
    /// The IV is fixed rather than random: a fixture wants to be
    /// reproducible, and nothing here is protecting anything.
    pub fn seal(key: &SymmetricKey, plain: &[u8]) -> String {
        encrypt_with_iv(key, [0x5au8; BLOCK], plain)
            .expect("a padded plaintext is always whole blocks")
            .to_string()
    }

    /// Base64 with padding, exported alongside [`seal`] so a caller can build
    /// a type-4 string out of the OpenSSL fixture below.
    pub fn base64(bytes: &[u8]) -> String {
        b64(bytes)
    }

    fn nist_backed_enc_string(mac_key: &[u8; 32]) -> (SymmetricKey, String) {
        let enc_key: [u8; 32] = hex(NIST_KEY).try_into().expect("32 bytes");
        let iv = hex(NIST_IV);
        let ct = hex(NIST_CIPHERTEXT);
        let mut hmac = <HmacSha256 as Mac>::new_from_slice(mac_key).expect("any key length");
        hmac.update(&iv);
        hmac.update(&ct);
        let mac = hmac.finalize().into_bytes();
        let key = SymmetricKey {
            enc: Zeroizing::new(enc_key),
            mac: Zeroizing::new(*mac_key),
        };
        (key, format!("2.{}|{}|{}", b64(&iv), b64(&ct), b64(&mac)))
    }

    #[test]
    fn a_well_formed_type_two_string_parses_into_its_three_parts() {
        let (_, text) = nist_backed_enc_string(&[9u8; 32]);
        let parsed = EncString::from_str(&text).expect("a well-formed type 2 string");
        let EncString::AesCbc256HmacSha256B64 { iv, ct, mac } = parsed else {
            panic!("parsed as the wrong variant");
        };
        assert_eq!(iv.to_vec(), hex(NIST_IV));
        assert_eq!(ct, hex(NIST_CIPHERTEXT));
        assert_eq!(mac.len(), 32);
    }

    #[test]
    fn a_well_formed_type_four_string_parses() {
        let text = format!("4.{}", b64(&[0xab; 256]));
        let parsed = EncString::from_str(&text).expect("a well-formed type 4 string");
        assert_eq!(parsed, EncString::Rsa2048OaepSha1B64 { ct: vec![0xab; 256] });
    }

    /// **Every refused type is refused by name**, which is the whole reason
    /// [`REFUSED_TYPES`] is a table.
    #[test]
    fn the_types_this_module_does_not_implement_are_refused_by_name() {
        for (number, name) in REFUSED_TYPES {
            let text = format!("{number}.{}|{}|{}", b64(&[0; 16]), b64(&[0; 16]), b64(&[0; 32]));
            assert_eq!(
                EncString::from_str(&text),
                Err(CryptoError::Unsupported(name)),
                "type {number} must be refused as `{name}`"
            );
        }
        // Control: the table is not empty and does not name a type that is
        // in fact supported.
        assert_eq!(REFUSED_TYPES.len(), 5);
        assert!(REFUSED_TYPES.iter().all(|(n, _)| *n != 2 && *n != 4));
    }

    #[test]
    fn a_type_bitwarden_never_defined_is_refused_too() {
        assert_eq!(EncString::from_str("9.AAAA"), Err(CryptoError::UnknownType));
    }

    /// Malformed input is refused rather than repaired. Each case is a way a
    /// lenient parser would have guessed.
    #[test]
    fn malformed_strings_are_refused_rather_than_guessed_at() {
        let iv = b64(&[0; 16]);
        let ct = b64(&[0; 16]);
        let mac = b64(&[0; 32]);
        let cases: [(&str, String); 9] = [
            ("no dot at all", format!("2{iv}|{ct}|{mac}")),
            ("an empty type", format!(".{iv}|{ct}|{mac}")),
            ("a non-numeric type", format!("x.{iv}|{ct}|{mac}")),
            ("a type out of a byte's range", format!("300.{iv}|{ct}|{mac}")),
            ("two parts where three are required", format!("2.{iv}|{ct}")),
            ("four parts where three are required", format!("2.{iv}|{ct}|{mac}|{mac}")),
            ("a 32-byte IV", format!("2.{mac}|{ct}|{mac}")),
            ("a 16-byte MAC", format!("2.{iv}|{ct}|{ct}")),
            ("a ciphertext that is not a whole block", format!("2.{iv}|{}|{mac}", b64(&[0; 17]))),
        ];
        for (what, text) in cases {
            assert!(
                EncString::from_str(&text).is_err(),
                "`{what}` was accepted; this parser must refuse rather than guess"
            );
        }

        // Base64 strictness comes from `record::seal::base64_from`, and this
        // is the assertion that it is really in the path: a `!` is not a
        // base64 character and must not be skipped over.
        let mut bad = iv.clone();
        bad.replace_range(0..1, "!");
        assert!(EncString::from_str(&format!("2.{bad}|{ct}|{mac}")).is_err());

        // Control: the shape all nine cases were mutated from does parse, so
        // the loop is not passing because every string is unparseable.
        assert!(EncString::from_str(&format!("2.{iv}|{ct}|{mac}")).is_ok());
    }

    #[test]
    fn an_empty_ciphertext_is_refused_in_both_supported_types() {
        let iv = b64(&[0; 16]);
        let mac = b64(&[0; 32]);
        assert!(EncString::from_str(&format!("2.{iv}||{mac}")).is_err());
        assert!(EncString::from_str("4.").is_err());
    }

    // ---- MAC before decryption --------------------------------------------

    /// **The MAC is checked, and a wrong one is a refusal rather than
    /// garbage.**
    #[test]
    fn a_tampered_ciphertext_is_refused_and_not_decrypted() {
        let (key, text) = nist_backed_enc_string(&[9u8; 32]);
        let good = EncString::from_str(&text).expect("well-formed");

        let EncString::AesCbc256HmacSha256B64 { iv, ct, mac } = good.clone() else {
            panic!("wrong variant");
        };
        let mut flipped = ct.clone();
        flipped[0] ^= 0x01;
        let tampered = EncString::AesCbc256HmacSha256B64 { iv, ct: flipped, mac };
        assert_eq!(decrypt(&key, &tampered), Err(CryptoError::MacMismatch));

        // And a wrong MAC key on the right ciphertext, which is the other way
        // the check can be reached.
        let wrong_key = SymmetricKey { enc: key.enc.clone(), mac: Zeroizing::new([1u8; 32]) };
        assert_eq!(decrypt(&wrong_key, &good), Err(CryptoError::MacMismatch));
    }

    /// The MAC covers **`iv || ct`**, not the ciphertext alone. Without this
    /// an attacker could swap the IV -- which changes the first plaintext
    /// block outright -- and the MAC would still verify.
    #[test]
    fn the_mac_covers_the_iv_as_well_as_the_ciphertext() {
        let (key, text) = nist_backed_enc_string(&[9u8; 32]);
        let EncString::AesCbc256HmacSha256B64 { ct, mac, .. } =
            EncString::from_str(&text).expect("well-formed")
        else {
            panic!("wrong variant");
        };
        let swapped = EncString::AesCbc256HmacSha256B64 { iv: [0xff; 16], ct, mac };
        assert_eq!(decrypt(&key, &swapped), Err(CryptoError::MacMismatch));
    }

    /// A symmetric key cannot be pointed at an RSA string, or the reverse.
    /// Both directions are a named refusal rather than a wrong answer.
    #[test]
    fn the_two_key_kinds_are_not_interchangeable() {
        let (key, _) = nist_backed_enc_string(&[9u8; 32]);
        let rsa = EncString::Rsa2048OaepSha1B64 { ct: vec![0; 256] };
        assert!(matches!(decrypt(&key, &rsa), Err(CryptoError::Unsupported(_))));

        let sym = EncString::AesCbc256HmacSha256B64 {
            iv: [0; 16],
            ct: vec![0; 16],
            mac: [0; 32],
        };
        assert!(matches!(decrypt_rsa(&[0; 8], &sym), Err(CryptoError::Unsupported(_))));
    }

    /// A throwaway RSA-2048 private key in PKCS#8 DER, generated with
    /// **OpenSSL** for `rsa_oaep_sha1_opens_a_ciphertext_openssl_produced`:
    /// `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`, then
    /// `openssl pkcs8 -topk8 -nocrypt -outform DER`. The second step is not
    /// decoration: `genpkey -outform DER` emits a **PKCS#1** `RSAPrivateKey`,
    /// which [`decrypt_rsa`] refuses -- as it should, and as this test
    /// observed before the key was converted.
    ///
    /// It guards nothing and exists only in this file.
    pub const ORG_KEY_PRIVATE_PKCS8_DER: &str = concat!(
        "308204be020100300d06092a864886f70d0101010500048204a8308204a40201",
        "000282010100bca69338a4fdb8d0ccf16c0c1c02f8ffa5ec1b20376b43b28b34",
        "dbbce1dd377487dbe11b36075941a6c9fe418affc3081e790fa792d2f6dd36de",
        "0152d1739ecb8f6c9963e3f016c3d94c8cb7ee74f83ee29521caf6c1cbcb0e4b",
        "d28fe1864519e2b1acf45402a740c7fdb893c28906aa27bd0615a25370dfdbb1",
        "0bd35a5ee6643504c5d551a047acd05fa60d50e135d54f27ad5a2ae708b939b4",
        "7d7a760b43ab99a505f70ca10c782eaa512797019cf667f26780b7b00dfd26d6",
        "27ce053784d97207aac954af97cc2bbe1a47a1327c7837f7f665cbe36bfe20b3",
        "fb0594f753f7b316db286ec5f9b23bab3227286bd25ef9e3041d79e48668e7b1",
        "488f9626fd990203010001028201004a8ae79172606f2ed24c730d35e456cf6d",
        "98a5ff4ce6ad91574043b396ebfa85a94950e197afbfad1962a77cee97b150fb",
        "f98a1e04fe275db1d8775d6a35ed8131e30f9950f0058ecdc659b4341d341a65",
        "1dd884828c8122733bb2aff7c53e78c402c0fcaa5582112ef52a81f8547cb5af",
        "8e1961630ae5870f201e341d79723f674d7b1d50dab8642e272241196486b7da",
        "f90dab6528997e7bb221e092455c48d4948c5fe3d256432b1e615d20660cd0a3",
        "7a9d21179597921977e9f893bdc2fb30a63e03f76b62d1af1cc4bcb29658df0d",
        "940f10f42afca38d1a732a516edc66a237a75c4afc3a47c3e1e73bf03369d076",
        "7dec6f2d9146beeb9a969f989061a102818100f8f12ac269ba0e082e5f271644",
        "ac9427a7a94a0d97937859ca3e523e56f2de2306b3732eca06784fcee0534d40",
        "873d71ca02b21e40edd7cf8c3a18ac1d28766cfd43c72f7c76159e81b19be040",
        "a2edb1147271ba909481f7a3c00457a7b904e2bddc977c5c42fccacade0c93b8",
        "62cfb20a781b66ca4904b379f659960fe988db02818100c1ffcf7add38efb57e",
        "10779b96b5393df7fe6bcbe0143acfaf83cbac394b0691d186b825578a1bbc2b",
        "e4c1df10c5b0ac3c64464fd50aa9912993901dbcd0bfe1dcbfa1b36a24d854df",
        "2169d3bb29a94c18d07208c207f767bf025ac12d9b7e3ddbdba9930ede5bd8ab",
        "776320372dd82b12dd28a4b6b87d0a6c22327e0030b39b02818100e2c33c420f",
        "f0ed2b42a266868053fc390b1ec8580d44c6127489c47d08d2feca45265dbbb7",
        "47a17c8164123d82942ec262538650ccb05b2fb1fa91d2e6549f5bb4707316ac",
        "771c4660b99ad5f1caf85d9fd488087bfeeb4cdb1ae459bc6c6b28e7edf307d3",
        "3b29eec850f07ff72bfb29a123bb422cedca9c7a728f34849624950281810088",
        "66937c00952abd823093d85a837b06de1a0db2e00f79365362a84ea44de3059d",
        "bb4a383f2f84c6ae59fe1217d9d7999230b2db28a0818ee61bb1a5a6ff631aac",
        "3a34b850362dc0a6cdf8797d4c1293c592b1cb0499d3532792c13ab8156f1291",
        "460619b6c792ee69c8dc7267399d96d3819a350d9ff392e36abbf3a9b0946702",
        "818043aba77daf1feec0386385cc678f2ca09382657d23649da0d7b5fc22e277",
        "da3e4676611049e8904a766f134983434e6a847013eaf660d6735226713fe836",
        "21fa9dad6390a2d6ae4cab8a16f4291017a94f1501013cc5f7e77e41eae3be04",
        "5d396e345fa34b0915e3f66f17760343869308500e4a4e43c30aaae69adc550c",
        "71ca",
    );

    /// The 64 bytes `00 01 .. 3f` wrapped to the key above, by **OpenSSL**
    /// and not by this crate: `openssl pkeyutl -encrypt -keyform DER -pkeyopt
    /// rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha1 -pkeyopt
    /// rsa_mgf1_md:sha1`.
    pub const ORG_KEY_WRAPPED_OAEP_SHA1: &str = concat!(
        "005691be74361eb94a830d8968f704a8396c5b1bf3c3c6a6ceb39554a293c17c",
        "74685bc7652f3314b17f1096567d23a69da164094c6094dcc1b58cdac20414c0",
        "a302882cdc41c712bd574f51a1fc125e20a9a37238f77650b533ed8aff6a45c8",
        "8d1c52f9122bb48ab7f19a7a500cca748e0408d15742b6e09887f1f8cd4376ac",
        "48b49336164c9190d4e9875e05a6decbba33e05a1cc654d8535abf48d495676e",
        "02d8f294f046fbafe88b4c834dfe5e9717a988ed1f6be69657fc542788fa0748",
        "03597ac6ab107f69b9e7ac1f62c830173e591cbb51d073c83386e0f3b48000cd",
        "fc5415d5345114f6aff930b0d165286a7da7250836d24d49df0f826599296f7a",
    );

    /// **RSA-OAEP-SHA1, against a ciphertext this code did not produce.**
    ///
    /// The key pair and the ciphertext above came out of OpenSSL, an
    /// independent implementation, and the plaintext is the 64 bytes
    /// `00 01 .. 3f` -- the length and shape an organisation key has. So this
    /// arm has external ground truth, in the way the composition test below
    /// explicitly does not: if this module used SHA-256 for OAEP, or MGF1
    /// with a different hash, or read the key as PKCS#1 rather than PKCS#8,
    /// OpenSSL's ciphertext would not open.
    #[test]
    fn rsa_oaep_sha1_opens_a_ciphertext_openssl_produced() {
        let key = hex(ORG_KEY_PRIVATE_PKCS8_DER);
        let wrapped = EncString::Rsa2048OaepSha1B64 { ct: hex(ORG_KEY_WRAPPED_OAEP_SHA1) };
        let org = unwrap_org_key(&key, &wrapped).expect("OpenSSL's own ciphertext");
        let expected: Vec<u8> = (0..64u8).collect();
        assert_eq!(*org.enc, expected[..32]);
        assert_eq!(*org.mac, expected[32..]);

        // Control: one flipped ciphertext byte does not open, so the
        // assertion above is not passing on something OAEP ignored.
        let mut ct = hex(ORG_KEY_WRAPPED_OAEP_SHA1);
        ct[100] ^= 0x01;
        assert_eq!(
            unwrap_org_key(&key, &EncString::Rsa2048OaepSha1B64 { ct }),
            Err(CryptoError::Rsa)
        );
    }

    /// A private key that is not PKCS#8 DER is an error, not a panic.
    #[test]
    fn an_unreadable_private_key_is_an_error_rather_than_a_panic() {
        let rsa = EncString::Rsa2048OaepSha1B64 { ct: vec![0; 256] };
        assert_eq!(decrypt_rsa(b"not a key", &rsa), Err(CryptoError::Rsa));
    }

    // ---- padding ----------------------------------------------------------

    #[test]
    fn pkcs7_padding_is_stripped_and_malformed_padding_is_refused() {
        let mut block = vec![0xaa; 11];
        block.extend_from_slice(&[5u8; 5]);
        assert_eq!(
            strip_pkcs7(Zeroizing::new(block)).expect("well-formed padding").to_vec(),
            vec![0xaa; 11]
        );

        // A full block of padding, which is what an exact-multiple plaintext
        // gets and the case an off-by-one strips wrongly.
        assert_eq!(strip_pkcs7(Zeroizing::new(vec![16u8; 16])).expect("full block").len(), 0);

        for bad in [vec![0u8; 16], vec![17u8; 17], vec![5u8; 3], {
            let mut v = vec![0xaa; 12];
            v.extend_from_slice(&[4, 4, 3, 4]);
            v
        }] {
            assert_eq!(strip_pkcs7(Zeroizing::new(bad)), Err(CryptoError::Padding));
        }
        assert_eq!(strip_pkcs7(Zeroizing::new(Vec::new())), Err(CryptoError::Padding));
    }

    // ---- the composition, and exactly what it does and does not prove -----

    /// **This test proves the composition is self-consistent, and that is the
    /// whole of what it proves.**
    ///
    /// It builds a protected symmetric key by encrypting 64 known bytes under
    /// a stretched master key, then unwraps it and checks the 64 bytes come
    /// back. The 64 bytes are chosen here, the encryption is done here, and
    /// so a systematic error in the *composition* -- the wrong HKDF info
    /// string, the salt normalised differently from the server, the encryption
    /// and MAC halves swapped -- would be made identically on both sides and
    /// would pass.
    ///
    /// **No external ground truth for this step could be established**, and
    /// nothing in this crate can supply one. It needs an `EncString` produced
    /// by Bitwarden's own client for a known password, email, KDF and
    /// plaintext; Bitwarden publishes no such vector, and generating one here
    /// would mean a real account on a real server, which no test in this
    /// crate may touch. The primitives underneath are each pinned to a
    /// published vector above; the arrangement of them is pinned to
    /// Bitwarden's documentation and to this code agreeing with itself. That
    /// is the honest description and it should not be paraphrased into a
    /// stronger one.
    ///
    /// What it does catch, and is worth having for: a later edit that breaks
    /// the round trip, reorders the MAC check, or changes one side of the
    /// composition without the other.
    #[test]
    fn the_composition_is_self_consistent_and_that_is_all_it_is() {
        use cbc::cipher::block_padding::Pkcs7;
        use cbc::cipher::BlockEncryptMut;
        type Aes256CbcEnc = cbc::Encryptor<Aes256>;

        // **Deliberately not the 600 000 a real account carries.** This test
        // derives three master keys and none of them is testing PBKDF2's
        // cost -- `pbkdf2_hmac_sha256_matches_rfc_7914` already pins the
        // primitive, at the RFC's own 80 000. Running the real figure here
        // adds about thirteen seconds to a debug suite and pins nothing.
        const COMPOSITION_ITERATIONS: u32 = 1_000;

        let master = master_key(b"correct horse battery staple", "User@Example.com", Kdf::Pbkdf2 {
            iterations: COMPOSITION_ITERATIONS,
        })
        .expect("usable");
        let stretched = master.stretch();

        // The user key, known because this test chose it.
        let user_key: Vec<u8> = (0..64u8).collect();

        let iv = [0x5au8; 16];
        let mut buf = vec![0u8; 64 + 16];
        buf[..64].copy_from_slice(&user_key);
        let ct = Aes256CbcEnc::new((&*stretched.enc).into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, 64)
            .expect("room for one padding block")
            .to_vec();
        let mut hmac =
            <HmacSha256 as Mac>::new_from_slice(&*stretched.mac).expect("any key length");
        hmac.update(&iv);
        hmac.update(&ct);
        let mac = hmac.finalize().into_bytes();

        let protected =
            EncString::from_str(&format!("2.{}|{}|{}", b64(&iv), b64(&ct), b64(&mac)))
                .expect("well-formed");
        let unwrapped = unwrap_user_key(&stretched, &protected).expect("the right key");
        assert_eq!(*unwrapped.enc, user_key[..32]);
        assert_eq!(*unwrapped.mac, user_key[32..]);

        // Controls, which are the part of this test that can actually fail
        // for a real reason: the wrong password does not unwrap it, and the
        // wrong email does not either.
        let wrong_password =
            master_key(b"correct horse battery stapl", "User@Example.com", Kdf::Pbkdf2 {
                iterations: COMPOSITION_ITERATIONS,
            })
            .expect("usable");
        assert_eq!(
            unwrap_user_key(&wrong_password.stretch(), &protected),
            Err(CryptoError::MacMismatch)
        );
        let wrong_email =
            master_key(b"correct horse battery staple", "other@example.com", Kdf::Pbkdf2 {
                iterations: COMPOSITION_ITERATIONS,
            })
            .expect("usable");
        assert_eq!(
            unwrap_user_key(&wrong_email.stretch(), &protected),
            Err(CryptoError::MacMismatch)
        );
    }

    /// A decrypted "key" of the wrong length is refused rather than padded or
    /// truncated into one.
    #[test]
    fn a_protected_key_that_is_not_64_bytes_is_refused() {
        assert_eq!(
            SymmetricKey::from_64(&[0u8; 32]),
            Err(CryptoError::KeyLength { expected: 64, got: 32 })
        );
        assert!(SymmetricKey::from_64(&[0u8; 64]).is_ok());
    }

    /// A round trip through the NIST-backed `EncString`: the ciphertext and
    /// the expected plaintext are both NIST's, so this one *does* have
    /// external ground truth for its AES half. Only the MAC is computed here,
    /// and HMAC-SHA256 is pinned to RFC 4231 above.
    ///
    /// The plaintext is not PKCS#7-padded (NIST's vectors are not), so
    /// `decrypt` refuses its padding -- and that refusal is the assertion:
    /// it reached the padding step, which means the MAC verified and the
    /// AES-CBC decryption ran.
    #[test]
    fn the_mac_and_cbc_layers_compose_over_the_nist_vector() {
        let (key, text) = nist_backed_enc_string(&[9u8; 32]);
        let enc = EncString::from_str(&text).expect("well-formed");
        assert_eq!(
            decrypt(&key, &enc),
            Err(CryptoError::Padding),
            "the MAC must verify and the CBC layer must run before padding is judged"
        );

        // And the block underneath really is NIST's plaintext.
        let EncString::AesCbc256HmacSha256B64 { iv, ct, .. } = &enc else {
            panic!("wrong variant");
        };
        assert_eq!(
            cbc_decrypt_raw(&key.enc, iv, ct).expect("whole blocks").to_vec(),
            hex(NIST_PLAINTEXT)
        );
    }

    // ---- no secret in a message -------------------------------------------

    /// **Every error this module can produce prints without a secret in it**,
    /// and every secret-bearing type prints redacted.
    #[test]
    fn nothing_here_prints_a_secret() {
        let master = MasterKey(Zeroizing::new([0xde; 32]));
        let printed = format!("{master:?}");
        assert!(!printed.contains("222"), "a byte of the key reached Debug: {printed}");
        assert!(printed.contains("redacted"));

        let stretched = master.stretch();
        let printed = format!("{stretched:?}");
        assert!(printed.contains("redacted"));
        // Control on the assertion itself: the real bytes would have shown up
        // as decimal in a derived `Debug`, and they do not.
        let first = stretched.enc[0];
        assert!(!printed.contains(&format!("{first}")), "{printed}");

        let (_, text) = nist_backed_enc_string(&[9u8; 32]);
        let enc = EncString::from_str(&text).expect("well-formed");
        let printed = format!("{enc:?}");
        assert!(printed.contains("64 ciphertext bytes"), "{printed}");
        assert!(!printed.contains("245"), "a ciphertext byte reached Debug: {printed}");

        // The errors, each rendered through `Display`, which is what a log
        // line would use.
        for error in [
            CryptoError::KdfParams("iteration count is zero"),
            CryptoError::Malformed("no `.` between type and body"),
            CryptoError::Unsupported("AesCbc256_B64"),
            CryptoError::UnknownType,
            CryptoError::MacMismatch,
            CryptoError::Padding,
            CryptoError::KeyLength { expected: 64, got: 32 },
            CryptoError::Rsa,
        ] {
            let text = error.to_string();
            assert!(!text.is_empty());
            assert!(!text.contains("222"), "{text}");
        }
    }

    // ---- the composition, against BITWARDEN's own published vectors -------
    //
    // Everything above this line checks a primitive against an IETF or NIST
    // vector, or checks this code against itself. The four tests below are a
    // different kind of thing and the module docs' caveat is narrowed by
    // exactly this much: each one transcribes a value asserted by
    // **Bitwarden's own client**, in the open, at
    // `bitwarden/sdk-internal`, `crates/bitwarden-crypto/`. They are not this
    // crate agreeing with itself. If Deskwarden had the arrangement wrong --
    // the salt, the info strings, the iteration count, the order of the three
    // base64 parts -- these go red.
    //
    // They are still not a live account. What they cannot reach is written
    // down at `the_composition_is_still_not_fully_pinned_and_here_is_what_is_left`.

    /// **Bitwarden `crates/bitwarden-crypto/src/keys/master_key.rs`,
    /// `test_password_hash_pbkdf2`.**
    ///
    /// This is the single most valuable vector in the file: it is a function
    /// of the master key, so it pins [`master_key`]'s PBKDF2 arm -- the
    /// password as the input, the *email* as the salt, that count of
    /// iterations, 32 bytes of output -- **and** [`MasterKey::password_hash`]
    /// on top of it. Getting either wrong changes the answer.
    ///
    /// The three salts are Bitwarden's, not this crate's, and they are what
    /// pins [`kdf_salt`]: the same hash must come out of the trimmed and the
    /// untrimmed, the upper- and the lower-cased email.
    #[test]
    fn the_password_hash_matches_bitwardens_own_vector_for_pbkdf2() {
        let password = b"asdfasdf";
        for salt in ["test@bitwarden.com", "TEST@bitwarden.com", " test@bitwarden.com"] {
            let key = master_key(password, salt, Kdf::Pbkdf2 { iterations: 100_000 })
                .expect("100000 is a usable iteration count");
            assert_eq!(
                *key.password_hash(password),
                "wmyadRMyBZOH7P/a/ucTCbSghKgdzDpPqUnu/DAVtSw=",
                "the password hash for salt {salt:?} is not the one Bitwarden's own client \
                 asserts on -- the master key derivation, the salt normalisation or the hash \
                 itself is arranged differently from Bitwarden's"
            );
        }
    }

    /// **Bitwarden `master_key.rs`, `test_password_hash_argon2id`.**
    ///
    /// The Argon2id arm of the same claim, and it pins one thing the PBKDF2
    /// vector cannot: that the Argon2id salt is the **SHA-256 of** the
    /// normalised email rather than the email. Feeding the email straight in
    /// would produce a different key and a different hash here.
    ///
    /// Bitwarden's salt for this vector is `test_salt`, not an email. It is
    /// transcribed unchanged: [`kdf_salt`] leaves it alone (nothing to trim,
    /// nothing to lowercase), so the vector still exercises the arm.
    #[test]
    fn the_password_hash_matches_bitwardens_own_vector_for_argon2id() {
        let password = b"asdfasdf";
        let key = master_key(
            password,
            "test_salt",
            Kdf::Argon2id { iterations: 4, memory_mib: 32, parallelism: 2 },
        )
        .expect("Argon2id accepts m=32 KiB, t=4, p=2");
        assert_eq!(
            *key.password_hash(password),
            "PR6UjYmjmppTYcdyTiNbAhPJuQQOmynKbdEl1oyi/iQ=",
            "the Argon2id master key or the hash over it is arranged differently from \
             Bitwarden's -- the likeliest cause is the salt not being SHA-256'd"
        );
    }

    /// **Bitwarden `crates/bitwarden-crypto/src/keys/utils.rs`,
    /// `test_stretch_kdf_key`.**
    ///
    /// [`MasterKey::stretch`] against Bitwarden's own input and output. This
    /// is what the module docs single out as unconfirmable -- "that the HKDF
    /// info strings are `enc` and `mac`" -- and it is now confirmed, along
    /// with expand-without-extract and which of the two halves is which.
    /// Swapping the two info strings reds the second assertion.
    #[test]
    fn key_stretching_matches_bitwardens_own_vector() {
        let master = MasterKey(Zeroizing::new([
            31, 79, 104, 226, 150, 71, 177, 90, 194, 80, 172, 209, 17, 129, 132, 81, 138, 167, 69,
            167, 254, 149, 2, 27, 39, 197, 64, 42, 22, 195, 86, 75,
        ]));
        let stretched = master.stretch();
        assert_eq!(
            *stretched.enc,
            [
                111, 31, 178, 45, 238, 152, 37, 114, 143, 215, 124, 83, 135, 173, 195, 23, 142,
                134, 120, 249, 61, 132, 163, 182, 113, 197, 189, 204, 188, 21, 237, 96
            ],
            "the stretched ENCRYPTION key does not match Bitwarden's"
        );
        assert_eq!(
            *stretched.mac,
            [
                221, 127, 206, 234, 101, 27, 202, 38, 86, 52, 34, 28, 78, 28, 185, 16, 48, 61, 127,
                166, 209, 247, 194, 87, 232, 26, 48, 85, 193, 249, 179, 155
            ],
            "the stretched MAC key does not match Bitwarden's"
        );
    }

    /// **Bitwarden `crates/bitwarden-crypto/src/enc_string/symmetric.rs`,
    /// `test_enc_from_to_buffer`.**
    ///
    /// Which of a type-2 `EncString`'s three base64 parts is the IV, which is
    /// the ciphertext and which is the MAC -- externally, rather than by this
    /// crate's say-so.
    ///
    /// Bitwarden's vector is a *string* and the *byte buffer* it serialises
    /// to, and the buffer's layout is `type || iv || mac || ct` -- a
    /// different order from the string's `type.iv|ct|mac`, which is precisely
    /// what makes it useful here: the two orders can only be reconciled one
    /// way. Slicing the published buffer at 1, 17 and 49 and demanding the
    /// parsed string's fields equal those slices pins all three assignments
    /// and both fixed lengths at once.
    #[test]
    fn an_enc_strings_parts_are_assigned_the_way_bitwarden_assigns_them() {
        const TEXT: &str = concat!(
            "2.pMS6/icTQABtulw52pq2lg==|XXbxKxDTh+mWiN1HjH2N1w==|",
            "Q6PkuT+KX/axrgN9ubD5Ajk2YNwxQkgs3WJM0S0wtG8="
        );
        // Bitwarden's `to_buffer()` output for exactly that string.
        const BUFFER: [u8; 65] = [
            2, 164, 196, 186, 254, 39, 19, 64, 0, 109, 186, 92, 57, 218, 154, 182, 150, 67, 163,
            228, 185, 63, 138, 95, 246, 177, 174, 3, 125, 185, 176, 249, 2, 57, 54, 96, 220, 49,
            66, 72, 44, 221, 98, 76, 209, 45, 48, 180, 111, 93, 118, 241, 43, 16, 211, 135, 233,
            150, 136, 221, 71, 140, 125, 141, 215,
        ];

        let EncString::AesCbc256HmacSha256B64 { iv, ct, mac } =
            EncString::from_str(TEXT).expect("Bitwarden's own well-formed type 2")
        else {
            panic!("a type-2 EncString parsed as something else");
        };

        assert_eq!(BUFFER[0], 2, "control: the published buffer's type byte is not 2");
        assert_eq!(iv.as_slice(), &BUFFER[1..17], "the first base64 part is not the IV");
        assert_eq!(mac.as_slice(), &BUFFER[17..49], "the third base64 part is not the MAC");
        assert_eq!(ct.as_slice(), &BUFFER[49..], "the second base64 part is not the ciphertext");
    }

    /// **Bitwarden `master_key.rs`, `test_decrypt_user_key_aes_cbc256_b64`.**
    ///
    /// That a protected symmetric key's plaintext is **64 bytes of
    /// encryption key followed by MAC key** -- [`SymmetricKey::from_64`]'s
    /// one assumption, and named in the module docs as unconfirmed.
    ///
    /// # Why this test does the AES itself
    ///
    /// Bitwarden's vector is a **type 0** `EncString` (`AesCbc256_B64`:
    /// unauthenticated AES-256-CBC, no MAC, unwrapped with the master key
    /// *directly* rather than the stretched one). [`EncString`] refuses type
    /// 0 by name and must keep refusing it -- an unauthenticated ciphertext
    /// is exactly what this crate declines to decrypt -- so the vector cannot
    /// be run through [`decrypt`].
    ///
    /// Doing the CBC here instead is not this crate marking its own homework:
    /// the *inputs* (password, salt, iteration count, ciphertext) and the
    /// *outputs* (the two 32-byte halves) are all Bitwarden's, and the only
    /// thing this test supplies is the AES-CBC step, which NIST SP 800-38A
    /// already pins two hundred lines above. What it therefore proves is the
    /// split: that the first 32 bytes are the encryption key and the last 32
    /// the MAC key, and not the reverse.
    ///
    /// It also pins [`master_key`] a second time, at a different iteration
    /// count and a different salt from the PBKDF2 vector above.
    #[test]
    fn a_protected_keys_plaintext_is_the_encryption_key_then_the_mac_key() {
        let key = master_key(b"asdfasdfasdf", "legacy@bitwarden.com", Kdf::Pbkdf2 {
            iterations: 600_000,
        })
        .expect("600000 is a usable iteration count");

        // Bitwarden's type-0 EncString, split by hand because this crate's
        // parser refuses the type: `0.<iv>|<ct>`.
        const IV_B64: &str = "8UClLa8IPE1iZT7chy5wzQ==";
        const CT_B64: &str = concat!(
            "6PVfHnVk5S3XqEtQemnM5yb4JodxmPkkWzmDRdfyHtjORmvxqlLX40tBJZ+CKxQWmS8tpEB5w39r",
            "bgHg/gqs0haGdZG4cPbywsgGzxZ7uNI="
        );
        let iv: [u8; BLOCK] =
            base64_from(IV_B64).expect("base64").try_into().expect("a 16-byte IV");
        let ct = base64_from(CT_B64).expect("base64");

        // The master key IS the AES key here -- no stretch, which is what
        // makes type 0 the legacy format it is.
        let mut plain = Zeroizing::new(ct);
        Aes256CbcDec::new((&*key.0).into(), (&iv).into())
            .decrypt_padded_mut::<NoPadding>(&mut plain)
            .expect("a whole number of blocks");
        let plain = strip_pkcs7(plain).expect("Bitwarden's own ciphertext is well-padded");

        assert_eq!(plain.len(), SYMMETRIC_KEY_LEN, "a protected key is not 64 bytes");
        let split = SymmetricKey::from_64(&plain).expect("64 bytes");
        assert_eq!(
            *split.enc,
            [
                12, 95, 151, 203, 37, 4, 236, 67, 137, 97, 90, 58, 6, 127, 242, 28, 209, 168, 125,
                29, 118, 24, 213, 44, 117, 202, 2, 115, 132, 165, 125, 148
            ],
            "the FIRST 32 bytes are not the encryption key"
        );
        assert_eq!(
            *split.mac,
            [
                186, 215, 234, 137, 24, 169, 227, 29, 218, 57, 180, 237, 73, 91, 189, 51, 253, 26,
                17, 52, 226, 4, 134, 75, 194, 208, 178, 133, 128, 224, 140, 167
            ],
            "the LAST 32 bytes are not the MAC key"
        );
    }

    // ---- encryption -------------------------------------------------------

    /// A key whose two halves are different, so a test cannot pass by using
    /// the encryption key where the MAC key belongs.
    fn encrypt_test_key() -> SymmetricKey {
        let mut bytes = [0u8; SYMMETRIC_KEY_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).expect("64 fits in a byte");
        }
        key_from_64(&bytes)
    }

    /// The round trip, at the four lengths where PKCS#7 and CBC can each go
    /// wrong differently: nothing at all, one exact block, an exact multiple
    /// of the block size (where the padding block is entirely padding), and
    /// something long enough to cross many blocks at a non-aligned length.
    #[test]
    fn encrypt_round_trips_through_this_modules_own_decrypt() {
        let key = encrypt_test_key();
        let cases: [Vec<u8>; 4] = [
            Vec::new(),
            b"sixteen bytes!!!".to_vec(),
            b"exactly two whole blocks of it..".to_vec(),
            (0..1000u32).map(|i| u8::try_from(i % 251).expect("under 256")).collect(),
        ];
        for plain in cases {
            // The fixture must keep covering both sides of the boundary,
            // since block-aligned input is the case PKCS#7 is easy to get
            // wrong on.
            assert!(
                [0usize, 16, 32, 1000].contains(&plain.len()),
                "the fixture lengths drifted"
            );
            let enc = encrypt(&key, &plain).expect("the OS has randomness");
            let EncString::AesCbc256HmacSha256B64 { ref ct, .. } = enc else {
                panic!("encrypt produced something other than type 2");
            };
            assert!(
                ct.len() > plain.len() && ct.len() % BLOCK == 0,
                "PKCS#7 must always add between one and {BLOCK} bytes, at length {}",
                plain.len()
            );
            let back = decrypt(&key, &enc).expect("what this module sealed, it opens");
            assert_eq!(
                back.to_vec(),
                plain,
                "the round trip lost the plaintext at {} bytes",
                plain.len()
            );
        }
    }

    /// **The test the IV exists for.**
    ///
    /// A fixed IV, an IV derived from the key, or an IV derived from the
    /// plaintext all round-trip perfectly and all leak the relationship
    /// between two ciphertexts. The only thing that separates them from a
    /// correct implementation is that encrypting one plaintext twice under
    /// one key gives two different answers.
    ///
    /// Statistical rather than exact, and deliberately so: pinning the IV
    /// would need a test-only hook in the randomness path, which this crate
    /// bans and which would be the very seam an attacker-facing bug hides
    /// behind. Two OS-random 16-byte values collide with probability 2^-128,
    /// which is not a flake anyone will see.
    ///
    /// Both halves are asserted. Equal IVs catch a constant or derived IV;
    /// equal ciphertexts would additionally catch an IV that varies but is
    /// not actually fed into the cipher.
    #[test]
    fn two_encryptions_of_one_plaintext_differ_in_both_iv_and_ciphertext() {
        let key = encrypt_test_key();
        let plain = b"the same plaintext, twice, under the same key";

        let first = encrypt(&key, plain).expect("randomness");
        let second = encrypt(&key, plain).expect("randomness");

        let (
            EncString::AesCbc256HmacSha256B64 { iv: iv_a, ct: ct_a, mac: mac_a },
            EncString::AesCbc256HmacSha256B64 { iv: iv_b, ct: ct_b, mac: mac_b },
        ) = (&first, &second)
        else {
            panic!("encrypt produced something other than type 2");
        };

        assert_ne!(iv_a, iv_b, "the IV is fixed or derived -- it must be fresh OS randomness");
        assert_ne!(ct_a, ct_b, "two encryptions produced the same ciphertext");
        assert_ne!(mac_a, mac_b, "the MAC does not vary with the IV");
        assert_eq!(ct_a.len(), ct_b.len(), "one plaintext, two lengths");

        // And both still open, so the difference is the IV and not damage.
        assert_eq!(decrypt(&key, &first).expect("opens").to_vec(), plain.to_vec());
        assert_eq!(decrypt(&key, &second).expect("opens").to_vec(), plain.to_vec());
    }

    /// Many IVs, all distinct: one pair differing could in principle be luck
    /// with a badly seeded counter that happens to increment; a run of them
    /// being pairwise distinct will not be.
    #[test]
    fn every_iv_in_a_run_of_encryptions_is_distinct() {
        let key = encrypt_test_key();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let enc = encrypt(&key, b"x").expect("randomness");
            let EncString::AesCbc256HmacSha256B64 { iv, .. } = enc else {
                panic!("not type 2");
            };
            assert!(seen.insert(iv), "an IV repeated within 64 encryptions");
        }
        assert_eq!(seen.len(), 64);
    }

    /// The MAC covers the ciphertext: flip one bit of it and `decrypt`
    /// refuses **before** decrypting anything.
    #[test]
    fn a_tampered_ciphertext_is_refused_with_a_mac_mismatch() {
        let key = encrypt_test_key();
        let enc = encrypt(&key, b"a value worth forging").expect("randomness");
        let EncString::AesCbc256HmacSha256B64 { iv, mut ct, mac } = enc else {
            panic!("not type 2");
        };
        ct[0] ^= 0x01;
        assert_eq!(
            decrypt(&key, &EncString::AesCbc256HmacSha256B64 { iv, ct, mac }),
            Err(CryptoError::MacMismatch)
        );
    }

    /// The MAC covers the **IV** too, which is the half that is easy to leave
    /// out. In CBC a flipped IV bit flips the same bit of the first plaintext
    /// block and corrupts nothing else, so a MAC over the ciphertext alone
    /// would hand the caller quietly altered data instead of an error.
    #[test]
    fn a_tampered_iv_is_refused_with_a_mac_mismatch() {
        let key = encrypt_test_key();
        let enc = encrypt(&key, b"a value worth forging").expect("randomness");
        let EncString::AesCbc256HmacSha256B64 { mut iv, ct, mac } = enc else {
            panic!("not type 2");
        };
        iv[0] ^= 0x01;
        assert_eq!(
            decrypt(&key, &EncString::AesCbc256HmacSha256B64 { iv, ct, mac }),
            Err(CryptoError::MacMismatch)
        );
    }

    /// Control for the two above: without the tampering, the same assembly
    /// opens. Otherwise both tests would pass against an `EncString` that was
    /// broken for some other reason entirely.
    #[test]
    fn the_untampered_reassembly_still_opens() {
        let key = encrypt_test_key();
        let enc = encrypt(&key, b"a value worth forging").expect("randomness");
        let EncString::AesCbc256HmacSha256B64 { iv, ct, mac } = enc else { panic!("not type 2") };
        let rebuilt = EncString::AesCbc256HmacSha256B64 { iv, ct, mac };
        assert_eq!(
            decrypt(&key, &rebuilt).expect("untampered").to_vec(),
            b"a value worth forging".to_vec()
        );
    }

    /// A wrong MAC key is a `MacMismatch`, not a decryption -- the MAC is a
    /// real check and not a checksum of the ciphertext alone.
    #[test]
    fn a_ciphertext_from_another_key_does_not_open() {
        let enc = encrypt(&encrypt_test_key(), b"someone else's secret").expect("randomness");
        let other = key_from_64(&[7u8; SYMMETRIC_KEY_LEN]);
        assert_eq!(decrypt(&other, &enc), Err(CryptoError::MacMismatch));
    }

    /// [`EncString`]'s `Display` and its `FromStr` are inverses, for both
    /// variants. This is the property the HTTP layer will rely on when it
    /// puts an encrypted field in a request body.
    #[test]
    fn the_rendered_wire_string_parses_back_to_an_equal_value() {
        let key = encrypt_test_key();
        for len in [0usize, 1, 15, 16, 17, 32, 255] {
            let enc = encrypt(&key, &vec![0xa5u8; len]).expect("randomness");
            let text = enc.to_string();
            assert!(text.starts_with("2."), "a type-2 string must say so: {text}");
            assert_eq!(text.matches('|').count(), 2, "three parts, two separators");
            let parsed = EncString::from_str(&text).expect("what Display wrote, FromStr reads");
            assert!(parsed == enc, "the round trip through the wire form changed the value");
            assert_eq!(
                decrypt(&key, &parsed).expect("still opens").to_vec(),
                vec![0xa5u8; len]
            );
        }

        let rsa = EncString::Rsa2048OaepSha1B64 { ct: vec![0xab; 256] };
        let text = rsa.to_string();
        assert!(text.starts_with("4."), "a type-4 string must say so");
        assert!(!text.contains('|'), "type 4 has no `|`-separated parts");
        assert_eq!(EncString::from_str(&text), Ok(rsa));
    }

    /// The rendered parts are exactly the bytes, standard-alphabet base64
    /// with padding, in the order `iv|ct|mac` behind a `2.`.
    ///
    /// Pinned against a **literal** string rather than against
    /// [`nist_backed_enc_string`]'s output. The fixture builder uses the
    /// same `b64` helper `Display` does, so comparing the two could only
    /// ever check part order and structure -- never the alphabet (standard,
    /// not URL-safe) or the padding. The literal below was produced outside
    /// this crate from NIST SP 800-38A F.2.5's IV and ciphertext and an
    /// HMAC-SHA256 over `iv || ct` under a key of 32 `0x09` bytes, so every
    /// character of it is something this file has to match rather than
    /// something it got to choose.
    #[test]
    fn display_renders_the_wire_form_byte_for_byte() {
        const EXPECTED: &str = concat!(
            "2.AAECAwQFBgcICQoLDA0ODw==",
            "|9YxMBNbl8bp3nqv7X3v71pz8TpZ+24CNZ593e8ZwLH058jNpqdm6z6Uw4mMEIxRhsusF4sOb6fza",
            "bBkHjGqdGw==",
            "|m/AW0QYMpQ2bEO1/UzsCzVTjf4I7YKujQ9eaK4lU0Hg="
        );

        let (_, text) = nist_backed_enc_string(&[9u8; 32]);
        assert_eq!(text, EXPECTED, "the fixture builder no longer writes the pinned string");

        let parsed = EncString::from_str(EXPECTED).expect("well formed");
        assert_eq!(parsed.to_string(), EXPECTED);
    }

    /// `Debug` still redacts after `Display` was added. The two must not be
    /// confused with each other: see [`crate::debug_leak_guard`].
    #[test]
    fn debug_still_prints_lengths_and_display_still_prints_the_wire_form() {
        let enc = encrypt(&encrypt_test_key(), b"secret").expect("randomness");
        let debug = format!("{enc:?}");
        assert!(debug.contains("ciphertext bytes"), "Debug stopped redacting: {debug}");
        assert!(!debug.contains('|'), "Debug is printing the wire form");
        assert!(enc.to_string().contains('|'), "Display is not printing the wire form");
    }

    /// What the published vectors above still do **not** pin, stated as a test so
    /// it is read rather than skipped, and so it fails if someone deletes the
    /// caveat from the module docs without deleting the gap.
    ///
    /// Pinned externally now: the PBKDF2 and Argon2id master keys including
    /// both salts, the password hash, the stretch's info strings and their
    /// order, the three parts of a type-2 `EncString`, the enc-then-mac
    /// split of a protected key, the rendered wire form byte for byte, and
    /// AES-256-CBC in **both** directions -- NIST SP 800-38A F.2.6 for
    /// decryption and F.2.5 for encryption, so the cipher step [`encrypt`]
    /// performs is not merely round-tripped against this file's own
    /// decryptor.
    ///
    /// **Not pinned, and this is the whole of what is left:**
    ///
    /// * **That PKCS#7 *construction* is right**, as opposed to the cipher
    ///   under it. The padding [`encrypt`] writes is checked only by
    ///   [`strip_pkcs7`] accepting it back and by the block-length
    ///   assertions in `encrypt_round_trips_through_this_modules_own_decrypt`
    ///   -- both this file marking its own homework. NIST's CBC vectors are
    ///   whole blocks and pad nothing, so F.2.5 cannot reach it. The failure
    ///   this leaves open is symmetric: a pad byte written wrong and stripped
    ///   wrong in the same way round-trips perfectly here and is rejected by
    ///   every other client.
    /// * **That an `EncString` this module emits is one another Bitwarden
    ///   client accepts.** The encrypt direction is new, and it inherits
    ///   every composition gap below -- the MAC input, the type-2 layout,
    ///   which key encrypts what -- with the failure direction reversed.
    ///   Getting the MAC input wrong on the *decrypt* side fails closed;
    ///   getting it wrong on the *encrypt* side writes a value that this
    ///   module happily reads back and no other client can open, and nothing
    ///   here can tell the difference. Nothing in the running app calls
    ///   [`encrypt`] yet, which is the only reason that is survivable.
    ///
    /// * **That the HMAC covers `iv || ct` rather than `ct` alone.** No
    ///   published vector this crate could find gives a type-2 `EncString`
    ///   together with the key that opens it, so the MAC input is still taken
    ///   from Bitwarden's format description. A wrong MAC input fails
    ///   *closed* -- every decryption returns `MacMismatch` and nothing
    ///   silently decrypts wrong -- which is the safe direction, but it is
    ///   unverified.
    /// * **That the protected user key is wrapped under the STRETCHED master
    ///   key** rather than the master key itself. Type 0 (the vector above)
    ///   uses the master key directly. Bitwarden's *source* is unambiguous
    ///   that type 2 uses the stretched pair --
    ///   `crates/bitwarden-crypto/src/keys/master_key.rs`,
    ///   `decrypt_user_key`: the `Aes256Cbc_HmacSha256_B64` arm builds
    ///   `stretch_key(key)` first -- but reading a client's source is a
    ///   weaker thing than a vector, and it is recorded as the weaker thing.
    /// * **RSA-OAEP-SHA1 for organisation keys**, end to end. The primitive
    ///   round-trips against a generated key above, but no published
    ///   Bitwarden org-key ciphertext with a known plaintext was found.
    ///
    /// What would settle the composition gaps -- the last three above, and
    /// the second one with them: one `EncString` produced by a real
    /// Bitwarden client together with the account it came from -- a throwaway
    /// account on a self-hosted server, logged in once with the official
    /// client, its `/api/sync` response and its master password recorded.
    /// That is a one-off manual capture, and it is the only thing that can
    /// close this.
    #[test]
    fn the_composition_is_still_not_fully_pinned_and_here_is_what_is_left() {
        // Not a tautology: it asserts the module docs still carry the caveat
        // this test enumerates, so removing one without the other reds.
        let docs = include_str!("crypto.rs");
        assert!(
            docs.contains("The Bitwarden-specific composition is not"),
            "the module docs no longer state that the composition is unverified, but the gap \
             this test enumerates is still open"
        );
        assert!(
            docs.contains("This holds in both directions now, and the two directions are not"),
            "the module docs no longer state that the encrypt direction carries its own \
             unverified claims, but the gaps this test enumerates are still open"
        );
    }
}

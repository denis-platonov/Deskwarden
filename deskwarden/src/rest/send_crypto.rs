//! The Send key hierarchy.
//!
//! **A Send is not encrypted under the user key**, and that is the whole
//! reason this file exists beside [`crate::rest::crypto`] rather than inside
//! it. Every other secret in this crate is reachable only by someone holding
//! the vault; a Send is deliberately reachable by someone holding a *link*,
//! because the key travels in the URL fragment. Mixing the two hierarchies in
//! one module is how a value ends up encrypted under the wrong one.
//!
//! The derivation is Bitwarden's `derive_shareable_key`, read from
//! `bitwarden-crypto/src/keys/shareable_key.rs`:
//!
//! ```text
//! prk = HMAC-SHA256(key = "bitwarden-" ++ name, msg = secret)
//! okm = HKDF-Expand-SHA256(prk, info, 64)   ->   enc_key || mac_key
//! ```
//!
//! and it is checked here against **Bitwarden's own published vectors**, not
//! against this crate's own round trip. A round-trip test cannot see a wrong
//! HMAC label: it produces a Send this app can read and no other client can.

use crate::rest::crypto::{decrypt, encrypt, CryptoError, EncString, SymmetricKey};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Bitwarden's send key is 128 bits. Not a parameter: a client that used a
/// different length would produce links no other client could read.
const SEND_KEY_LEN: usize = 16;

/// `SEND_ITERATIONS` in Bitwarden's own source.
const SEND_KDF_ITERATIONS: u32 = 100_000;

/// One Send's key: the 16 bytes that travel in the link.
///
/// **Not `Debug`**, by the rule `Challenge`, `service_token::Token` and
/// `SendInvocation` already follow: these bytes decrypt the Send for anyone
/// who has them, and a `Debug` is what ends up in a log file.
pub struct SendKey(zeroize::Zeroizing<[u8; SEND_KEY_LEN]>);

impl SendKey {
    /// A new key, from the system CSPRNG.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Rng`] if the CSPRNG fails, which is the same failure
    /// [`crate::rest::crypto::encrypt`] answers for a missing IV, and the
    /// same answer: nothing has been sent, because nothing here sends.
    pub fn fresh() -> Result<Self, CryptoError> {
        let mut bytes = zeroize::Zeroizing::new([0u8; SEND_KEY_LEN]);
        getrandom::getrandom(&mut *bytes).map_err(|_| CryptoError::Rng)?;
        Ok(Self(bytes))
    }

    /// A key from bytes already held: [`Self::from_wrapped`]'s own last step,
    /// and this file's vectors.
    pub(crate) fn from_bytes(bytes: [u8; SEND_KEY_LEN]) -> Self {
        Self(zeroize::Zeroizing::new(bytes))
    }

    /// The key a server row carries, unwrapped with the user key.
    ///
    /// # Errors
    ///
    /// [`CryptoError`] if the decryption fails, and
    /// [`CryptoError::KeyLength`] if what came back is not 16 bytes -- a
    /// server row whose key is the wrong size is not a Send this client can
    /// build a link for, and truncating one to fit would produce a link that
    /// opens nothing.
    pub fn from_wrapped(wrapped: &EncString, user: &SymmetricKey) -> Result<Self, CryptoError> {
        let plain = decrypt(user, wrapped)?;
        let bytes: [u8; SEND_KEY_LEN] = plain.as_slice().try_into().map_err(|_| {
            CryptoError::KeyLength { expected: SEND_KEY_LEN, got: plain.len() }
        })?;
        Ok(Self::from_bytes(bytes))
    }

    /// This key, encrypted under the user key, as the server stores it.
    pub fn wrapped_under(&self, user: &SymmetricKey) -> Result<EncString, CryptoError> {
        encrypt(user, &*self.0)
    }

    /// The key the Send's own fields are encrypted under.
    ///
    /// **Derived, never the raw bytes.** See the module docs: this is
    /// `derive_shareable_key(k, "send", "send")`, and the two literals are
    /// the whole of what makes a Send readable by other clients.
    pub fn cipher_key(&self) -> Result<SymmetricKey, CryptoError> {
        derive_shareable_key(&*self.0, "send", "send")
    }

    /// What the server stores as `password`: proof the sender knew it, and
    /// not a key. PBKDF2-HMAC-SHA256, this key as the salt,
    /// [`SEND_KDF_ITERATIONS`] iterations, 32 bytes, base64.
    ///
    /// The salt is the part a reader guesses wrong -- everywhere else in this
    /// crate a PBKDF2 salt is the e-mail address.
    pub fn password_hash(&self, password: &str) -> zeroize::Zeroizing<String> {
        let mut out = zeroize::Zeroizing::new([0u8; 32]);
        pbkdf2::pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            &*self.0,
            SEND_KDF_ITERATIONS,
            &mut *out,
        );
        let mut text = zeroize::Zeroizing::new(String::new());
        crate::record::seal::base64_into(&mut text, &*out);
        text
    }

    /// The link's fragment: base64url, unpadded, through the one encoder this
    /// crate already has for that job.
    pub fn fragment(&self) -> String {
        crate::rest::api::base64_url_no_pad(&*self.0)
    }
}

/// `{web vault}/#/send/{accessId}/{key}`.
///
/// `base` is the server root this client was configured with. For every
/// deployment this backend serves the API and the web vault share an origin;
/// a split deployment would produce a link with the right key and the wrong
/// host, which is the one thing about this feature that only the live check
/// settles. See the design's own note.
pub fn access_url(base: &str, access_id: &str, key: &SendKey) -> String {
    format!("{}/#/send/{}/{}", base.trim_end_matches('/'), access_id, key.fragment())
}

/// Bitwarden's `derive_shareable_key`.
///
/// `secret` is the shared material -- for a Send, the 16 bytes in the URL
/// fragment. `name` becomes the HMAC key as `"bitwarden-{name}"`; `info` is
/// HKDF's info string, and `""` means the empty info Bitwarden's `None`
/// produces.
///
/// # Errors
///
/// [`CryptoError::Malformed`] if HKDF refuses the PRK, which it cannot do
/// here -- the PRK is HMAC-SHA256's output and therefore exactly SHA-256's
/// length. It is returned rather than `expect`ed because this file is one
/// literal away from being called with a different hash one day, and a
/// `Result` at that moment is a compile error at the call site instead of a
/// panic in a worker thread.
pub fn derive_shareable_key(
    secret: &[u8],
    name: &str,
    info: &str,
) -> Result<SymmetricKey, CryptoError> {
    let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(format!("bitwarden-{name}").as_bytes())
        .expect("HMAC accepts a key of any length");
    hmac.update(secret);
    // Copied into an owned array rather than kept as the `GenericArray` the
    // MAC returns: `Zeroizing` needs a `Zeroize` impl, and only the array has
    // one. The PRK is the master secret of this hierarchy and is not left in
    // a buffer nothing wipes.
    let mut prk = zeroize::Zeroizing::new([0u8; 32]);
    prk.copy_from_slice(&hmac.finalize().into_bytes());

    let hkdf = hkdf::Hkdf::<Sha256>::from_prk(&*prk)
        .map_err(|_| CryptoError::Malformed("the shareable key's PRK is the wrong length"))?;
    let mut okm = zeroize::Zeroizing::new([0u8; 64]);
    hkdf.expand(info.as_bytes(), &mut *okm)
        .map_err(|_| CryptoError::Malformed("64 bytes is two HKDF blocks"))?;
    Ok(SymmetricKey::from_okm(&okm))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Bitwarden's own vectors**, both of them, from
    /// `bitwarden-crypto`'s tests for `derive_shareable_key`.
    ///
    /// The two differ only in `info`, which is exactly the parameter a
    /// plausible misreading drops -- so the second is the positive control
    /// for the first: an implementation that ignored `info` would produce
    /// the `None` answer for both and fail here rather than shipping a Send
    /// only this app can open.
    #[test]
    fn the_derivation_matches_bitwardens_published_vectors() {
        let with_info = derive_shareable_key(b"67t9b5g67$%Dh89n", "test_key", "test")
            .expect("the derivation succeeds");
        assert_eq!(
            base64_of(&with_info),
            "F9jVQmrACGx9VUPjuzfMYDjr726JtL300Y3Yg+VYUnVQtQ1s8oImJ5xtp1KALC9h2nav04++1LDW4iFD+infng==",
            "the derivation with an info parameter does not match Bitwarden's vector"
        );

        let without_info = derive_shareable_key(b"&/$%F1a895g67HlX", "test_key", "")
            .expect("the derivation succeeds");
        assert_eq!(
            base64_of(&without_info),
            "4PV6+PcmF2w7YHRatvyMcVQtI7zvCyssv/wFWmzjiH6Iv9altjmDkuBD1aagLVaLezbthbSe+ktR+U6qswxNnQ==",
            "the derivation with no info does not match Bitwarden's vector"
        );

        assert_ne!(
            base64_of(&with_info),
            base64_of(&without_info),
            "the two vectors are the control for each other and must not coincide"
        );
    }

    /// The name is part of the HMAC key, not decoration: two names over one
    /// secret must not agree.
    #[test]
    fn the_name_separates_the_domains() {
        let send = derive_shareable_key(b"0123456789abcdef", "send", "send").expect("derives");
        let other =
            derive_shareable_key(b"0123456789abcdef", "attachment", "send").expect("derives");
        assert_ne!(base64_of(&send), base64_of(&other), "the name is in the HMAC key");
    }

    /// A `SymmetricKey` has no accessor for its bytes -- deliberately. The
    /// vectors are compared through the one thing this crate can publish
    /// about them: the 64 derived bytes, base64'd the way Bitwarden published
    /// them.
    fn base64_of(key: &SymmetricKey) -> String {
        let mut out = String::new();
        crate::record::seal::base64_into(&mut out, &key.expose_okm());
        out
    }

    /// A 64-byte user key stand-in, and the wrap round trip: the key this
    /// process invented survives the server as an `EncString` under the user
    /// key and comes back the same.
    #[test]
    fn a_send_key_survives_being_wrapped_under_the_user_key() {
        let user = crate::rest::crypto::tests::key_from_64(&[7u8; 64]);
        let key = SendKey::fresh().expect("the CSPRNG answers");

        let wrapped = key.wrapped_under(&user).expect("the wrap succeeds");
        let back = SendKey::from_wrapped(&wrapped, &user).expect("the unwrap succeeds");

        assert_eq!(back.fragment(), key.fragment(), "the key did not survive the wrap");

        // The positive control for the assertion above: a DIFFERENT key must
        // not compare equal, or the test would pass against a `fragment`
        // that returned a constant.
        let other = SendKey::fresh().expect("the CSPRNG answers");
        assert_ne!(other.fragment(), key.fragment(), "two fresh keys collided");
    }

    /// **The fragment is base64url and unpadded.** Standard base64 produces
    /// `+` and `/`, which a URL fragment mangles -- a link that looks right,
    /// copies right, and fails for one Send in eight.
    #[test]
    fn the_fragment_is_url_safe() {
        // Chosen so that standard base64 yields both `+` and `/`: the
        // positive control is that the standard encoding of the same bytes
        // DOES contain them, so a `fragment` that had simply returned an
        // alphanumeric constant could not pass.
        let raw = [
            0xfbu8, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff,
            0xbf, 0xfb,
        ];
        let mut standard = String::new();
        crate::record::seal::base64_into(&mut standard, &raw);
        assert!(standard.contains('+') && standard.contains('/'), "the control is wrong");

        let key = SendKey::from_bytes(raw);
        let fragment = key.fragment();
        assert!(!fragment.contains('+'), "a `+` in a URL fragment: {fragment}");
        assert!(!fragment.contains('/'), "a `/` in a URL fragment: {fragment}");
        assert!(!fragment.contains('='), "padding in a URL fragment: {fragment}");
        assert_eq!(fragment.len(), 22, "16 bytes is 22 unpadded base64url characters");
    }

    /// The access URL is the web vault's Send route with the key in the
    /// fragment, and the `#` is what makes it a fragment: everything after it
    /// stays in the browser and never reaches the server.
    #[test]
    fn the_access_url_carries_the_key_after_the_hash() {
        let key = SendKey::from_bytes([1u8; 16]);
        let url = access_url("https://vault.example.com", "abc123", &key);
        assert_eq!(url, format!("https://vault.example.com/#/send/abc123/{}", key.fragment()));
        let (before, after) = url.split_once('#').expect("there is a fragment");
        assert!(!before.contains(&key.fragment()), "the key reached the server-visible half");
        assert!(after.ends_with(&key.fragment()), "the key is not in the fragment");

        // The control for the `trim_end_matches`: a base with a trailing
        // slash must not produce a double one.
        assert_eq!(
            access_url("https://vault.example.com/", "abc123", &key),
            url,
            "a trailing slash on the configured base leaked into the link"
        );
    }

    /// **PBKDF2-SHA256, 100 000 iterations, salted with the send key.** The
    /// salt is the part a reader guesses wrong -- the obvious guess is the
    /// e-mail, as everywhere else in this crate -- so the control is a hash
    /// over the same password with a DIFFERENT send key, which must differ.
    #[test]
    fn the_share_password_is_hashed_with_the_send_key_as_salt() {
        let key = SendKey::from_bytes([2u8; 16]);
        let hash = key.password_hash("correct-horse");
        let same = SendKey::from_bytes([2u8; 16]).password_hash("correct-horse");
        assert_eq!(*hash, *same, "the hash is not a function of its inputs alone");

        let other_key = SendKey::from_bytes([3u8; 16]).password_hash("correct-horse");
        assert_ne!(*hash, *other_key, "the send key is not the salt");

        let other_password = key.password_hash("wrong-horse");
        assert_ne!(*hash, *other_password, "the password does not reach the hash");

        // 32 bytes, base64'd with padding: the wire shape Bitwarden reads.
        assert_eq!(hash.len(), 44, "a 32-byte digest is 44 base64 characters");
    }

    /// The encryption key is derived, never the raw fragment bytes. A Send
    /// encrypted under the raw 16 bytes would be unreadable by every other
    /// client, and the check is that the derived key is the one Task 1's
    /// vectored function produces for the Send parameters.
    #[test]
    fn the_cipher_key_is_the_send_derivation_of_the_fragment() {
        let key = SendKey::from_bytes([4u8; 16]);
        let expected = derive_shareable_key(&[4u8; 16], "send", "send").expect("derives");
        let sealed = encrypt(&key.cipher_key().expect("derives"), b"the body").expect("encrypts");
        let opened = decrypt(&expected, &sealed).expect("the derived key opens it");
        assert_eq!(&*opened, b"the body");

        // The control: the RAW 16 bytes, split as a key would be, are not
        // what the Send is encrypted under -- so a `cipher_key` that had
        // skipped the derivation could not pass the line above.
        assert_ne!(
            base64_of(&key.cipher_key().expect("derives")),
            base64_of(&derive_shareable_key(&[4u8; 16], "send", "").expect("derives")),
            "`info` is not reaching the Send's own derivation"
        );
    }
}

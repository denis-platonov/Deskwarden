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

use crate::rest::crypto::{CryptoError, SymmetricKey};
use hmac::{Hmac, Mac};
use sha2::Sha256;

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
}

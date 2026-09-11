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
//!
//! # The second authority: the JavaScript a recipient actually runs
//!
//! Published vectors settle `derive_shareable_key`. They do **not** settle the
//! two literals this file passes it, the PBKDF2 salt, or the shape of the
//! link -- and every one of those is a way to ship a Send that looks encrypted
//! and opens for nobody. So they were read off the web vault this deployment
//! serves, which is the code the recipient's browser executes when the link is
//! opened, and which therefore cannot be wrong about what a Send is here.
//!
//! `GET /` returns a page whose module graph is `assets/index-*.js` and
//! `assets/shared-*.js`. In the first, beside the Sends screen:
//!
//! ```text
//! var Er = `bitwarden-send`, Dr = `send`, Or = 16, kr = 1e5;
//! async function jr(e) {                         // the Send's two keys
//!   if (e.length >= 64) return { enc: e.slice(0,32), mac: e.slice(32,64) };
//!   let t = await xe(e, Er, Dr, 64);
//!   return { enc: t.slice(0,32), mac: t.slice(32,64) };
//! }
//! async function Mr(e, t) { return Ae(await j(e, t, kr, 32)); } // password
//! function wr(e) { return Ae(e).replace(/\+/g,`-`)             // the fragment
//!                              .replace(/\//g,`_`).replace(/=+$/g,``); }
//! function rc(e, t, n) { return `${e}/#/send/${t}/${n}`; }      // the link
//! ```
//!
//! and in the second, where `xe` and `j` are defined:
//!
//! ```text
//! async function xi(e, t, n, r) {          // xe: HKDF, salt = t, info = n
//!   let o = { name: `HKDF`, salt: K(i), info: K(a), hash: `SHA-256` };
//!   ... deriveBits(o, c, r * 8) ...
//! }
//! async function yi(e, t, n, r) {          // j: PBKDF2-SHA256, salt = t
//!   ... deriveBits({ name: `PBKDF2`, hash: `SHA-256`, salt: K(a),
//!                    iterations: n }, s, r * 8) ...
//! }
//! ```
//!
//! **WebCrypto's HKDF is extract-then-expand**, so `xe(key, "bitwarden-send",
//! "send", 64)` is `HKDF-Extract(salt = "bitwarden-send", ikm = key)` --
//! which is `HMAC-SHA256(key = "bitwarden-send", msg = key)`, the PRK line
//! above -- followed by `HKDF-Expand(prk, "send", 64)`. The two derivations
//! are the same function reached from two directions, and that equality is
//! not argued from the reading alone: the served module was run against
//! Bitwarden's own two published vectors and reproduced both, which is the
//! control for the whole paragraph. See
//! [`tests::the_derivation_matches_the_web_vault_this_server_serves`] for the
//! vectors that came back, and
//! [`tests::a_send_encrypted_by_the_web_vault_opens_with_the_derived_key`] for
//! the one thing a derivation comparison still cannot prove: that a whole
//! ciphertext crosses.
//!
//! What the same reading settles about the rest of this file: the key is
//! `Or` = 16 bytes; `enc` is the first 32 derived bytes and `mac` the next 32;
//! the share password is `kr` = 100 000 PBKDF2-SHA256 iterations **salted with
//! the raw send key** and encoded with `Ae`, which is `btoa` -- standard
//! base64, padding kept; and the fragment is `wr`, the same bytes in base64url
//! with the padding stripped, placed in the link by `rc` as
//! `{base}/#/send/{accessId}/{fragment}`.
//!
//! **File Sends are out of scope** here as they are in
//! [`crate::rest::send`]: the same key opens one, but this module never
//! produces the file half of the body.

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

    /// **The three numbers the recipient's own browser produces**, taken from
    /// the JavaScript this deployment serves and not from any document.
    ///
    /// # How these were obtained
    ///
    /// The module docs quote the source; these are what it *answered*. The
    /// served `assets/shared-*.js` is an ES module, so it was imported into a
    /// Node process -- no browser, no vault, no account -- and its own
    /// exported HKDF, PBKDF2 and base64 called directly on fixed inputs. The
    /// first thing asked of it was Bitwarden's own two published vectors,
    /// which it reproduced byte for byte; that is the control that makes the
    /// three lines below statements about *this* file's parameters rather
    /// than about a function nobody has identified.
    ///
    /// # Why these and not a round trip
    ///
    /// [`the_derivation_matches_bitwardens_published_vectors`] proves the
    /// derivation. It cannot prove the two literals `"send"` and `"send"`, it
    /// cannot prove the PBKDF2 salt is the send key rather than the e-mail,
    /// and it cannot prove the fragment is base64url -- and each of those
    /// produces a Send that encrypts perfectly and opens for nobody. Three
    /// constants, checked against the only implementation whose opinion
    /// decides whether a link works.
    #[test]
    fn the_derivation_matches_the_web_vault_this_server_serves() {
        // `jr(new Uint8Array(16).fill(4))`, its 64 bytes through `Ae`.
        assert_eq!(
            base64_of(&SendKey::from_bytes([4u8; 16]).cipher_key().expect("derives")),
            "/8uwx0f+bA9Ar28VCWWqiOlGy0oDlzVcdP83dA8BLYNSvqOAKdr/3T32IIXakPUZQjpX0cMtJLJceRfHvVTx0w==",
            "this app's Send key is not the one the web vault derives for the same link"
        );

        // `Mr("correct-horse", new Uint8Array(16).fill(2))`.
        assert_eq!(
            *SendKey::from_bytes([2u8; 16]).password_hash("correct-horse"),
            "adfEDIiQVlRimUvuq8IWHHs2g9Xu2jXTpajsJVMccFA=",
            "the share password this app stores is not the one the web vault checks against"
        );

        // `wr()` of the sixteen bytes chosen above to force `+` and `/`.
        assert_eq!(
            SendKey::from_bytes([
                0xfbu8, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb,
                0xff, 0xbf, 0xfb,
            ])
            .fragment(),
            "-_-_-_-_-_-_-_-_-_-_-w",
            "the fragment this app writes is not the one the web vault reads back"
        );
    }

    /// **A Send encrypted by that JavaScript, opened here.**
    ///
    /// The derivation vectors above compare *keys*. This compares a whole
    /// ciphertext, which is the only thing that also settles the pieces a key
    /// comparison leaves out: the `2.iv|ct|mac` layout, CBC with a 16-byte IV,
    /// PKCS#7 padding, and -- the one most likely to be got wrong and least
    /// likely to be noticed -- that the MAC is taken over `iv ++ ct` in that
    /// order and is verified rather than ignored.
    ///
    /// The string was produced by calling the served module's own `Gt`
    /// (`2.${b64(iv)}|${b64(ct)}|${b64(hmac(mac, iv ++ ct))}`) on the keys
    /// `jr` derives for a send key of sixteen `0x04` bytes, over the plaintext
    /// `the body`. It is a constant here because a test that re-fetched the
    /// bundle would be a test that fails when the network does.
    ///
    /// # The other direction, and where the proof of it lives
    ///
    /// This test runs the interoperability the way a test can: a constant in,
    /// an assertion out. The direction that actually decides whether a link
    /// works is the opposite one -- the recipient's browser *decrypts* what
    /// this app wrote -- and a test cannot hold that, because holding it means
    /// running the served JavaScript.
    ///
    /// It was run. Three ciphertexts from this module's own [`encrypt`] over
    /// the same key and plaintext, three different random IVs, were handed to
    /// the served module's own decrypt; all three came back as `the body`, and
    /// the same strings under a Send key of sixteen `0x05` bytes were refused
    /// with `MAC mismatch` -- which is the control saying the MAC was checked
    /// rather than skipped. That is not reproducible from `cargo test` and is
    /// therefore not asserted here; it is recorded so that a reader knows the
    /// claim was checked and not merely argued, and so that the next person to
    /// change this file knows what to re-run. **Nothing was published to any
    /// real account to establish it**: every value on both sides is a fixed
    /// array in a test.
    #[test]
    fn a_send_encrypted_by_the_web_vault_opens_with_the_derived_key() {
        const FROM_THE_WEB_VAULT: &str = "2.goOibGHrL9tl73H+Rar1Zg==|\
             0yK4fibeysWgKmU5M1hXiA==|2dHwum32rZrCy+rL69JggndYzpBbLg1eUWwdRpT1Qcw=";

        let sealed: EncString = FROM_THE_WEB_VAULT.parse().expect("the web vault's own shape");
        let key = SendKey::from_bytes([4u8; 16]);
        let opened = decrypt(&key.cipher_key().expect("derives"), &sealed)
            .expect("the web vault's ciphertext does not open with this app's Send key");
        assert_eq!(&*opened, b"the body", "it opened, and said something else");

        // **The control, and it is two controls in one.** A different send
        // key must not open the same string -- so the success above is a fact
        // about the derivation and not about a `decrypt` that accepts
        // anything -- and the failure must come from the MAC, which is what
        // says the MAC is checked at all.
        let wrong = SendKey::from_bytes([5u8; 16]);
        assert!(
            decrypt(&wrong.cipher_key().expect("derives"), &sealed).is_err(),
            "a Send opened with the wrong key, so nothing above was verified"
        );
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

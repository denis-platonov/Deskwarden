# Sends Without the CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a direct-REST account, a user publishes a text Send, sees it in the Sends list with a working link, and revokes it — with no `bw.exe` on the machine. On a `bw serve` account nothing changes at all: the same child process, the same job object, the same profile directory, byte for byte.

**Architecture:** This is `docs/superpowers/specs/2026-08-30-sends-without-the-cli-design.md`. Its governing decision, and the one every task below inherits: **`crate::send` is not edited except for one visibility change.** `SendRunner`, `SendInvocation`, `plan_to_invocation`, `CliSendRunner` and the four `cli_send_*` doors are untouched, so the three source guards that wall them in keep their needle lists unchanged. The REST path is a sibling module producing the same result types — `CreatedSend`, `SendSummary`, `SendError` — and the branch between them lives in `vault_window`'s three `real_send_*` helpers, which already read process state on the worker thread.

The single visibility change is `send::deletion_date` from private to `pub(crate)`, so both backends stamp the same instant in the same format.

**Tech Stack:** Rust, `hmac` 0.12, `hkdf` 0.12, `sha2` 0.10, `pbkdf2` 0.12, `getrandom` 0.2, `ureq` 2, `serde_json` — every one of them already a dependency. **No new dependency is added by this plan.** Tests use `crate::test_http`, never `mockito` directly.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs or traits in production code.
- Build with `RUSTFLAGS="-D warnings"` **and run `cargo test --no-run` with it too**. CI compiles tests that way, and a warning visible only there has broken this repo's CI twice.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory; ~20 GB free and that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names".
- **Judge a failing test by reading it, never by its name prefix.** A real failure hid behind the `tests::` prefix for three CI runs this week because it looked like the known flaky family.
- Known flakes, and the only three that may be waved off — after reading them: a `vault_bridge` allocator test (~1 run in 3), a `scan_history` test under parallelism, and a mock-HTTP family in `main.rs`'s `tests::`.
- No test may touch the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/rest/send_crypto.rs` (**new**) | The Send key hierarchy: the shareable-key derivation, the 16-byte key, the wrap under the user key, the password hash, the access URL. Pure; no HTTP, no `Session`. |
| `deskwarden/src/rest/send.rs` (**new**) | A plan → a server-ready body; a server row → a `SendSummary`; `RestError` → `SendError`; the three operations. |
| `deskwarden/src/rest/api.rs` (modify) | `create_send`, `list_sends`, `delete_send` on `RestClient`; `send_url`; `base64url` and `base_url` to `pub(crate)`. |
| `deskwarden/src/rest/crypto.rs` (modify) | `SymmetricKey::from_okm`, `pub(crate)`. |
| `deskwarden/src/rest/mod.rs` (modify) | Two `pub mod` lines and the "still missing" paragraph, which becomes wrong the moment Task 4 lands. |
| `deskwarden/src/send.rs` (modify) | `deletion_date` becomes `pub(crate)`. Nothing else. |
| `deskwarden/src/backend_policy.rs` (modify) | `BackendEnv::credentials` and `direct_rest_credentials()`. |
| `deskwarden/src/main.rs` (modify) | Installs `credentials` beside `direct`. |
| `deskwarden/src/vault_window/mod.rs` (modify) | The branch, in `real_send_list`, `real_send_delete`, `real_send_create`. |

---

### Task 1: The Send key derivation, against Bitwarden's own vectors

**Files:** Create `deskwarden/src/rest/send_crypto.rs`; modify `deskwarden/src/rest/mod.rs`, `deskwarden/src/rest/crypto.rs`

**Interfaces**

- *Consumes:* `crypto::SymmetricKey`, `crypto::CryptoError`, `hmac`, `hkdf`, `sha2`.
- *Produces:* `send_crypto::derive_shareable_key(secret: &[u8], name: &str, info: &str) -> Result<SymmetricKey, CryptoError>`, and `crypto::SymmetricKey::from_okm(&[u8; 64]) -> SymmetricKey` (`pub(crate)`).

This task builds **only** the generic derivation, because that is the half with published vectors. The Send-specific parameters arrive in Task 2, where they are two string literals over a function that is already proven.

- [ ] **Step 1: Write the failing test**

Create `deskwarden/src/rest/send_crypto.rs`:

```rust
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
        let other = derive_shareable_key(b"0123456789abcdef", "attachment", "send")
            .expect("derives");
        assert_ne!(base64_of(&send), base64_of(&other), "the name is in the HMAC key");
    }

    /// A `SymmetricKey` has no accessor for its bytes -- deliberately. The
    /// vectors are compared through the one thing that is observable: what
    /// the key encrypts. Two keys are equal iff each opens the other's
    /// ciphertext, and `PartialEq` on `SymmetricKey` says so directly.
    fn base64_of(key: &SymmetricKey) -> String {
        let mut out = String::new();
        crate::record::seal::base64_into(&mut out, &key.expose_okm());
        out
    }
}
```

Run: `RUSTFLAGS="-D warnings" cargo test --lib send_crypto` — fails to resolve `derive_shareable_key`.

- [ ] **Step 2: Write the implementation**

In `deskwarden/src/rest/crypto.rs`, beside `SymmetricKey::from_64`:

```rust
    /// A key built from **64 bytes this process derived**, rather than from
    /// the plaintext of a protected key off a wire.
    ///
    /// The same split as [`Self::from_64`] and deliberately a different
    /// entry point: `from_64` takes a slice and can fail on its length,
    /// because its input is somebody else's ciphertext. This takes an array,
    /// so there is no length to check and no error to invent, and the caller
    /// is `crate::rest::send_crypto`'s HKDF output.
    pub(crate) fn from_okm(okm: &[u8; SYMMETRIC_KEY_LEN]) -> Self {
        let mut enc = Zeroizing::new([0u8; KEY_LEN]);
        let mut mac = Zeroizing::new([0u8; KEY_LEN]);
        enc.copy_from_slice(&okm[..KEY_LEN]);
        mac.copy_from_slice(&okm[KEY_LEN..]);
        Self { enc, mac }
    }

    /// The 64 bytes back, `enc || mac`. **`pub(crate)` and for vectors
    /// only.**
    ///
    /// It exists because the only external statement this crate can make
    /// about `send_crypto`'s derivation is a comparison against Bitwarden's
    /// published base64, and a key whose bytes cannot be read cannot be
    /// compared to a published value. Nothing in production calls it; the
    /// public-surface guard over `rest::crypto` is what keeps that true.
    pub(crate) fn expose_okm(&self) -> [u8; SYMMETRIC_KEY_LEN] {
        let mut out = [0u8; SYMMETRIC_KEY_LEN];
        out[..KEY_LEN].copy_from_slice(&*self.enc);
        out[KEY_LEN..].copy_from_slice(&*self.mac);
        out
    }
```

In `send_crypto.rs`:

```rust
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
    let prk = zeroize::Zeroizing::new(hmac.finalize().into_bytes());

    let hkdf = hkdf::Hkdf::<Sha256>::from_prk(&prk)
        .map_err(|_| CryptoError::Malformed("the shareable key's PRK is the wrong length"))?;
    let mut okm = zeroize::Zeroizing::new([0u8; 64]);
    hkdf.expand(info.as_bytes(), &mut *okm)
        .map_err(|_| CryptoError::Malformed("64 bytes is two HKDF blocks"))?;
    Ok(SymmetricKey::from_okm(&okm))
}
```

Add `pub mod send_crypto;` to `deskwarden/src/rest/mod.rs`.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib send_crypto` — three green. Then `RUSTFLAGS="-D warnings" cargo test --no-run` for the CI-only warnings.

---

### Task 2: The Send's own key, wrapped, hashed and linked

**Files:** Modify `deskwarden/src/rest/send_crypto.rs`, `deskwarden/src/rest/api.rs`

**Interfaces**

- *Consumes:* Task 1's `derive_shareable_key`; `crypto::encrypt`, `crypto::decrypt`, `crypto::EncString`; `api::base64url` (promoted here).
- *Produces:* `send_crypto::SendKey` with `SendKey::fresh()`, `SendKey::from_wrapped(&EncString, &SymmetricKey)`, `SendKey::wrapped_under(&SymmetricKey)`, `SendKey::cipher_key()`, `SendKey::password_hash(&str)`, `SendKey::fragment()`; `send_crypto::access_url(base: &str, access_id: &str, key: &SendKey)`.

- [ ] **Step 1: Write the failing test**

Append to `send_crypto.rs`'s test module:

```rust
    use crate::rest::crypto::{decrypt, encrypt, EncString};

    /// A 16-byte user key stand-in, and the wrap round trip: the key this
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
        let raw = [0xfbu8, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb];
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
        assert_eq!(
            url,
            format!("https://vault.example.com/#/send/abc123/{}", key.fragment())
        );
        let (before, after) = url.split_once('#').expect("there is a fragment");
        assert!(!before.contains(&key.fragment()), "the key reached the server-visible half");
        assert!(after.ends_with(&key.fragment()), "the key is not in the fragment");
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
        let sealed = encrypt(key.cipher_key(), b"the body").expect("encrypts");
        let opened = decrypt(&expected, &sealed).expect("the derived key opens it");
        assert_eq!(&*opened, b"the body");
    }
```

- [ ] **Step 2: Write the implementation**

In `api.rs`, change `fn base64url` to `pub(crate) fn base64url` and give it one added line of doc naming its second caller. In `send_crypto.rs`:

```rust
/// One Send's key: the 16 bytes that travel in the link.
///
/// **Not `Debug`**, by the rule `Challenge`, `service_token::Token` and
/// `SendInvocation` already follow: these bytes decrypt the Send for anyone
/// who has them, and a `Debug` is what ends up in a log file.
pub struct SendKey(zeroize::Zeroizing<[u8; SEND_KEY_LEN]>);

/// Bitwarden's send key is 128 bits. Not a parameter: a client that used a
/// different length would produce links no other client could read.
const SEND_KEY_LEN: usize = 16;

/// `SEND_ITERATIONS` in Bitwarden's own source.
const SEND_KDF_ITERATIONS: u32 = 100_000;

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

    /// A key from bytes already held. **Private outside tests' reach in
    /// production terms**: `pub(crate)` and used by the list path, which gets
    /// its bytes from [`Self::from_wrapped`], and by this file's own vectors.
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
    pub fn password_hash(&self, password: &str) -> zeroize::Zeroizing<String> {
        let mut out = zeroize::Zeroizing::new([0u8; 32]);
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
            password.as_bytes(),
            &*self.0,
            SEND_KDF_ITERATIONS,
            &mut *out,
        );
        let mut text = zeroize::Zeroizing::new(String::new());
        crate::record::seal::base64_into(&mut text, &*out);
        text
    }

    /// The link's fragment: base64url, unpadded, through the one encoder
    /// this crate already has for that job.
    pub fn fragment(&self) -> String {
        crate::rest::api::base64url(&*self.0)
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
```

`cipher_key` returns a `Result`, so the test above becomes `key.cipher_key().expect("derives")`; write it that way.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib send_crypto`, then `--no-run` with the same flags.

---

### Task 3: A plan becomes a server-ready body

**Files:** Create `deskwarden/src/rest/send.rs`; modify `deskwarden/src/rest/mod.rs`, `deskwarden/src/send.rs`

**Interfaces**

- *Consumes:* `crate::send::{SendPlan, validate_plan, SendClock, deletion_date}` (the last promoted to `pub(crate)` here), `send_crypto::SendKey`, `sync::VaultKeys`.
- *Produces:* `rest::send::MappedSend` with `body()` and `key()`, and `rest::send::encrypt_plan(&SendPlan, &VaultKeys, &dyn SendClock) -> Result<MappedSend, SendError>`.

`MappedSend` follows `MappedCipher` and `MappedFolder` exactly: a newtype whose body only the encrypting function can build, so a hand-assembled body — which for a Send would be the *cleartext* — cannot reach `api.rs`.

- [ ] **Step 1: Write the failing test**

Create `deskwarden/src/rest/send.rs` with the module doc and this test module:

```rust
//! Sends over REST: the half `crate::send` does not have.
//!
//! **`crate::send` is not edited by this module and does not know it
//! exists.** The two meet at the result types -- `SendPlan`, `CreatedSend`,
//! `SendSummary`, `SendError` -- and the branch between them is in
//! `vault_window`'s `real_send_*` helpers. See
//! `docs/superpowers/specs/2026-08-30-sends-without-the-cli-design.md` for
//! why the `SendRunner` trait is not widened to admit this path: it is an
//! argv and a stdin body, and a runner behind it would have to parse its own
//! request back out of base64 and then synthesise a CLI-shaped answer for
//! this app's own parser.
//!
//! **Text Sends only.** `type` is `0` and `file` is `null`, which is what
//! `send::plan_to_invocation` already sends to the CLI -- so this is parity
//! with the other backend and not a subtraction from it.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send::{FixedClock, SendPlan};
    use zeroize::Zeroizing;

    const NOW: FixedClock = FixedClock(1_786_408_997_148);

    fn keys() -> crate::rest::sync::VaultKeys {
        crate::rest::sync::tests::keys_from_user(&[9u8; 64])
    }

    fn a_plan() -> SendPlan {
        SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new("correct-horse-battery-staple".to_string()),
            hidden: true,
            delete_in_days: 7,
            password: Some(Zeroizing::new("share-pw-9271".to_string())),
            max_access_count: Some(3),
        }
    }

    /// **The secret is in the body only as ciphertext.** The positive
    /// control is on the next line: the ciphertext field must be present and
    /// non-empty, so a body that was never built cannot pass the absence
    /// assertion.
    #[test]
    fn the_body_carries_the_text_encrypted_and_nowhere_in_the_clear() {
        let mapped = encrypt_plan(&a_plan(), &keys(), &NOW).expect("the plan maps");
        let body = mapped.body().to_string();

        assert!(
            !body.contains("correct-horse-battery-staple"),
            "the Send's body reached the request in the clear"
        );
        assert!(
            !body.contains("share-pw-9271"),
            "the share password reached the request in the clear"
        );
        assert!(
            !body.contains("Wi-Fi password"),
            "the name reached the request in the clear -- Bitwarden encrypts it"
        );

        let text = mapped.body()["text"]["text"].as_str().expect("a text field");
        assert!(text.starts_with("2."), "the text is not an AES-CBC-HMAC EncString: {text}");
        let name = mapped.body()["name"].as_str().expect("a name field");
        assert!(name.starts_with("2."), "the name is not an EncString: {name}");
    }

    /// Every non-secret field the server needs, and the two that say what
    /// kind of Send this is.
    #[test]
    fn the_body_says_it_is_a_text_send_that_expires_when_the_form_said() {
        let mapped = encrypt_plan(&a_plan(), &keys(), &NOW).expect("the plan maps");
        let body = mapped.body();

        assert_eq!(body["type"], 0, "a text Send is type 0");
        assert!(body["file"].is_null(), "a text Send carries no file object");
        assert_eq!(body["text"]["hidden"], true, "the hidden flag did not travel");
        assert_eq!(body["maxAccessCount"], 3);
        assert_eq!(body["disabled"], false);
        assert_eq!(
            body["deletionDate"].as_str().expect("a deletion date"),
            crate::send::deletion_date(7, &NOW),
            "the two backends must stamp the same instant in the same format"
        );
        assert!(body["key"].as_str().expect("a key").starts_with("2."),
            "the send key is not wrapped under the user key");
        assert_eq!(
            body["password"].as_str().expect("a password hash").len(),
            44,
            "the password is not a 32-byte PBKDF2 digest"
        );
    }

    /// A plan with no password and no view cap sends explicit nulls, not
    /// absent keys -- the shape `send::plan_to_invocation` already sends, so
    /// the two backends put the same record on the same server.
    #[test]
    fn a_bare_plan_sends_nulls_rather_than_missing_keys() {
        let plan = SendPlan { password: None, max_access_count: None, ..a_plan() };
        let mapped = encrypt_plan(&plan, &keys(), &NOW).expect("the plan maps");
        assert!(mapped.body()["password"].is_null());
        assert!(mapped.body()["maxAccessCount"].is_null());
        // The control: the fields that DO have values still have them.
        assert!(mapped.body()["name"].is_string());
    }

    /// **Validation happens before any key is generated.** A refused plan
    /// must not consume a CSPRNG draw and must answer in the words the
    /// composer already shows.
    #[test]
    fn a_plan_the_composer_would_refuse_is_refused_here_too() {
        let empty = SendPlan { name: "  ".to_string(), ..a_plan() };
        assert_eq!(
            encrypt_plan(&empty, &keys(), &NOW).map(|_| ()),
            Err(crate::send::SendError::Rejected("Give the Send a name.".to_string()))
        );
        // The control: the same plan with a name maps.
        assert!(encrypt_plan(&a_plan(), &keys(), &NOW).is_ok());
    }
}
```

If `rest::sync::tests::keys_from_user` does not exist, add it in `sync.rs`'s existing test module beside `key_from_64` — a `VaultKeys` with the given user key and no organisations. Do not add a production constructor for it.

- [ ] **Step 2: Write the implementation**

In `deskwarden/src/send.rs`, change `fn deletion_date` to `pub(crate) fn deletion_date` and add to its doc:

```rust
/// **`pub(crate)`, and the reason is one instant.** `crate::rest::send`
/// stamps the same field for the direct-REST backend. A second copy of this
/// arithmetic beside the first is exactly how two backends come to disagree
/// about when a link dies -- the failure `local_time` was extracted to end.
```

Then run the guard that pins this module's public surface
(`vault_window::send_ui::source_pins::the_public_surface_of_the_send_module_is_exactly_these_items`).
If it counts `pub(crate)` declarations, add `deletion_date` to its pinned list
with a one-line comment naming this plan; if it does not, change nothing. **Read
the test before deciding** — do not add an entry to a list that does not want
one.

In `rest/send.rs`:

```rust
use crate::rest::crypto::{encrypt, CryptoError};
use crate::rest::send_crypto::SendKey;
use crate::rest::sync::VaultKeys;
use crate::send::{validate_plan, SendClock, SendError, SendPlan};
use serde_json::{json, Value};

/// A server-ready Send body, and the key it was built with.
///
/// The key is kept because the caller needs it **after** the request: the
/// access URL is assembled from the server's `accessId` and this key, and the
/// key never comes back from the server in a form anyone but the account
/// holder can open. Dropping it and re-reading it from the response would be
/// a second unwrap of a value this process already has.
pub struct MappedSend {
    body: Value,
    key: SendKey,
}

impl MappedSend {
    pub(crate) fn body(&self) -> &Value {
        &self.body
    }

    pub(crate) fn key(&self) -> &SendKey {
        &self.key
    }
}

/// One plan, as the body of `POST /api/sends`.
///
/// The plan is validated first, with [`validate_plan`] -- the composer's own
/// rules, so a plan refused here is refused in the same sentence the form
/// would have used, and no key is generated for a request that will not be
/// made.
pub fn encrypt_plan(
    plan: &SendPlan,
    keys: &VaultKeys,
    now: &dyn SendClock,
) -> Result<MappedSend, SendError> {
    if let Some(problem) = validate_plan(plan) {
        return Err(SendError::Rejected(problem.to_string()));
    }
    build(plan, keys, now).map_err(crypto_failed)
}

/// The half that can fail cryptographically, split out so the `map_err` above
/// is one line and every `?` below is the same kind of failure.
fn build(
    plan: &SendPlan,
    keys: &VaultKeys,
    now: &dyn SendClock,
) -> Result<MappedSend, CryptoError> {
    let key = SendKey::fresh()?;
    let cipher_key = key.cipher_key()?;
    let body = json!({
        "type": 0,
        "name": encrypt(&cipher_key, plan.name.trim().as_bytes())?.to_string(),
        "notes": Value::Null,
        "key": key.wrapped_under(keys.user())?.to_string(),
        "text": {
            "text": encrypt(&cipher_key, plan.text.as_bytes())?.to_string(),
            "hidden": plan.hidden,
        },
        "file": Value::Null,
        "maxAccessCount": plan.max_access_count,
        "deletionDate": crate::send::deletion_date(plan.delete_in_days, now),
        "expirationDate": Value::Null,
        "password": plan.password.as_ref().map(|p| key.password_hash(p).to_string()),
        "emails": Value::Null,
        "disabled": false,
        "hideEmail": false,
    });
    Ok(MappedSend { body, key })
}

/// Every cryptographic failure on this path means the same thing to the user
/// and the same thing about the world: **nothing was sent.**
///
/// It is not [`SendError::is_ambiguous`], and that is the point of mapping it
/// here rather than at the call site: no request has been made, so no link
/// can exist, and sending the user to check their Sends list would be
/// alarming and wrong.
fn crypto_failed(_: CryptoError) -> SendError {
    SendError::Rejected(
        "The Send could not be encrypted on this PC, so nothing was sent.".to_string(),
    )
}
```

Add `pub mod send;` to `rest/mod.rs`.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib rest::send`, then the full `--lib` run once, reading any failure rather than matching its prefix.

---

### Task 4: The three endpoints

**Files:** Modify `deskwarden/src/rest/api.rs`

**Interfaces**

- *Consumes:* `MappedSend`, `Session`, the existing `refreshing`/`bearer`/`value_from`/`unit_from`/`is_url_path_safe`.
- *Produces:* `RestClient::create_send`, `RestClient::list_sends`, `RestClient::delete_send`, `RestClient::base_url()` (`pub(crate)`).

- [ ] **Step 1: Write the failing test**

In `api.rs`'s test module, beside the folder-write tests, using `crate::test_http`:

```rust
    /// **Every field the server needs, and nothing the user typed.** Modelled
    /// on `the_password_grant_sends_every_field_the_server_requires`.
    #[test]
    fn creating_a_send_posts_the_encrypted_body_to_the_sends_route() {
        let mut server = crate::test_http::Server::new();
        let mock = server
            .mock("POST", "/api/sends")
            .match_header("authorization", "Bearer an-invented-access-token")
            .with_status(200)
            .with_body(r#"{"id":"send-1","accessId":"acc-1","deletionDate":"2026-09-06T00:43:17.148Z"}"#)
            .create();

        let client = RestClient::new(server.url());
        let mut session = a_session();
        let mapped = crate::rest::send::encrypt_plan(&a_plan(), &keys(), &FixedClock(0))
            .expect("the plan maps");
        let answer = client.create_send(&mut session, &mapped).expect("the send is created");

        mock.assert();
        assert_eq!(answer["accessId"], "acc-1", "the server's own accessId is what comes back");
    }

    /// A refusal is the server's own words, through the same `classify_400`
    /// every other write uses -- not a new error vocabulary for one feature.
    #[test]
    fn a_refused_send_carries_the_servers_own_sentence() {
        let mut server = crate::test_http::Server::new();
        let _mock = server
            .mock("POST", "/api/sends")
            .with_status(400)
            .with_body(r#"{"message":"You must be a premium user to send files."}"#)
            .create();

        let client = RestClient::new(server.url());
        let mut session = a_session();
        let mapped = crate::rest::send::encrypt_plan(&a_plan(), &keys(), &FixedClock(0))
            .expect("the plan maps");
        match client.create_send(&mut session, &mapped) {
            Err(RestError::Rejected { description, .. }) => {
                assert!(description.contains("premium"), "the server's words were lost");
            }
            other => panic!("expected a Rejected, got {other:?}"),
        }
    }

    /// The list is a `GET`, and the delete is path-scoped with the id
    /// checked -- the same `is_url_path_safe` gate the cipher and folder
    /// routes already use, so an id carrying a `/` cannot aim a `DELETE`
    /// somewhere else.
    #[test]
    fn sends_are_listed_and_revoked_on_their_own_routes() {
        let mut server = crate::test_http::Server::new();
        let list = server
            .mock("GET", "/api/sends")
            .with_status(200)
            .with_body(r#"{"data":[]}"#)
            .create();
        let delete = server.mock("DELETE", "/api/sends/send-1").with_status(200).create();

        let client = RestClient::new(server.url());
        let mut session = a_session();
        assert!(client.list_sends(&mut session).is_ok());
        assert!(client.delete_send(&mut session, "send-1").is_ok());
        list.assert();
        delete.assert();

        assert!(
            matches!(client.delete_send(&mut session, "../ciphers/x"), Err(RestError::UnsafeId)),
            "an id that is not path-safe must be refused before it is a URL"
        );
    }
```

- [ ] **Step 2: Write the implementation**

Beside the folder writers:

```rust
    // ---- writing one Send ---------------------------------------------------

    /// `POST /api/sends` -- a new **text** Send.
    ///
    /// `send` is a [`MappedSend`], which only
    /// [`crate::rest::send::encrypt_plan`] can produce, for the reason
    /// [`Self::create_folder`] takes a [`MappedFolder`]: the body carries the
    /// user's secret in encrypted form, and the type is what keeps a
    /// hand-built one -- which for a Send would be the cleartext body -- out
    /// of this module. Nothing here formats the body.
    ///
    /// Returns the server's own copy: the `id` it assigned and the `accessId`
    /// the link is built from. **The link itself is not in the answer** and
    /// cannot be: it carries a key this client generated and the server has
    /// never seen in the clear.
    pub fn create_send(
        &self,
        session: &mut Session,
        send: &crate::rest::send::MappedSend,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/sends", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.write_agent.post(&url), session).send_json(send.body()))
        })
    }

    /// `GET /api/sends` -- every Send this account has, still encrypted.
    ///
    /// **Its own route rather than `/api/sync`'s `sends` array**, though the
    /// sync carries them: [`crate::rest::sync`] is explicit that Sends are
    /// out of its scope, and teaching the vault's mapper to carry a second
    /// kind of record so that one screen can avoid one request would put
    /// Sends on the path every autofill takes.
    pub fn list_sends(&self, session: &mut Session) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/sends", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.sync_agent.get(&url), session).call())
        })
    }

    /// `DELETE /api/sends/{id}` -- the revoke.
    ///
    /// Through [`Self::unit_from`], on [`Self::delete_folder`]'s reasoning
    /// exactly: what this asserts is that a Send is **gone**, and a
    /// path-scoped status is that answer whole. There is nothing for a body
    /// to confirm, and an error for a delete that worked would push a user
    /// into revoking twice.
    pub fn delete_send(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.send_url(id)?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.delete(&url), session).call())
        })
    }
```

and, beside `folder_url`:

```rust
    /// `{base}/api/sends/{id}`, with the id checked first -- [`Self::cipher_url`]'s
    /// check applied to the third id this module puts in a path, from the
    /// same [`is_url_path_safe`], so three id kinds cannot be validated by
    /// three rules.
    fn send_url(&self, id: &str) -> Result<String, RestError> {
        if !is_url_path_safe(id) {
            return Err(RestError::UnsafeId);
        }
        Ok(format!("{}/api/sends/{}", self.base_url, id))
    }

    /// The server root this client was configured with.
    ///
    /// `pub(crate)` and a borrow: `crate::rest::send` assembles a Send's
    /// access URL from it. See that module for the one deployment shape this
    /// gets wrong.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
```

Finally, correct `rest/mod.rs`'s "still missing" paragraph: Sends are no longer wholly absent. Say what is there — create, list and revoke for text Sends — and what is not: file Sends and receiving from a link. Do not delete the sentence; the file's own convention is to correct a claim and say it was corrected.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib rest::api` — read every failure. This is the module whose mock-HTTP family is on the known-flake list; a failure inside a test **you added** is not that family.

---

### Task 5: A server row becomes a `SendSummary`, and a create becomes a `CreatedSend`

**Files:** Modify `deskwarden/src/rest/send.rs`

**Interfaces**

- *Consumes:* Task 4's three methods, `send_crypto::{SendKey, access_url}`, `crypto::decrypt`.
- *Produces:* `rest::send::{create, list, delete}`, each taking `&RestClient`, `&mut Session`, `&VaultKeys`; and `rest::send::map_error(RestError) -> SendError`.

- [ ] **Step 1: Write the failing test**

```rust
    /// A row the server sends becomes a row the Sends screen can show: a
    /// decrypted name, and a link that carries the key.
    #[test]
    fn a_server_row_becomes_a_summary_with_a_working_link() {
        let keys = keys();
        let key = SendKey::from_bytes([5u8; 16]);
        let cipher_key = key.cipher_key().expect("derives");
        let row = serde_json::json!({
            "id": "send-1",
            "accessId": "acc-1",
            "name": crate::rest::crypto::encrypt(&cipher_key, b"Wi-Fi password")
                .expect("encrypts").to_string(),
            "key": key.wrapped_under(keys.user()).expect("wraps").to_string(),
            "deletionDate": "2026-09-06T00:43:17.148Z",
            "type": 0,
        });

        let summary = summary_from(&row, &keys, "https://vault.example.com")
            .expect("the row maps");
        assert_eq!(summary.name, "Wi-Fi password", "the name was not decrypted");
        assert_eq!(summary.id, "send-1");
        assert!(!summary.is_file, "type 0 is a text Send");
        assert_eq!(
            summary.access_url,
            format!("https://vault.example.com/#/send/acc-1/{}", key.fragment())
        );
    }

    /// **A file Send is listed, not hidden.** It cannot be created here, and
    /// the screen says so -- but a Send made on another client must still be
    /// revocable from this one, which is the whole reason `is_file` exists.
    #[test]
    fn a_file_send_is_listed_and_flagged() {
        let keys = keys();
        let key = SendKey::from_bytes([6u8; 16]);
        let cipher_key = key.cipher_key().expect("derives");
        let row = serde_json::json!({
            "id": "send-2",
            "accessId": "acc-2",
            "name": crate::rest::crypto::encrypt(&cipher_key, b"report.pdf")
                .expect("encrypts").to_string(),
            "key": key.wrapped_under(keys.user()).expect("wraps").to_string(),
            "deletionDate": "2026-09-06T00:43:17.148Z",
            "type": 1,
        });
        let summary = summary_from(&row, &keys, "https://vault.example.com").expect("maps");
        assert!(summary.is_file, "a type 1 Send is a file Send");
        // The control: the same mapper reports false for the text row above.
        assert!(!summary_from(&text_row(&keys), &keys, "https://x").expect("maps").is_file);
    }

    /// A row missing what a revoke or a link needs is a **failure**, not a
    /// short list -- `send::parse_send_list`'s rule, applied to the other
    /// backend so the two screens cannot disagree about what is showable.
    #[test]
    fn a_row_without_an_access_id_is_a_failure_and_not_a_skip() {
        let keys = keys();
        let mut row = text_row(&keys);
        row.as_object_mut().expect("an object").remove("accessId");
        assert!(summary_from(&row, &keys, "https://x").is_err());
        // The control: with it back, the same row maps.
        assert!(summary_from(&text_row(&keys), &keys, "https://x").is_ok());
    }

    /// **A transport failure on a create is ambiguous.** The request may have
    /// reached the server, so a link may exist -- which is exactly what
    /// `SendError::TimedOut` means and what `is_ambiguous` gates the screen
    /// on. Reporting it as `Offline` would offer a plain "try again" over a
    /// link nobody knows about.
    #[test]
    fn a_create_that_never_got_an_answer_is_ambiguous() {
        assert_eq!(
            map_error(RestError::Transport("connection reset".to_string()), Ambiguity::Ambiguous),
            crate::send::SendError::TimedOut
        );
        // The control: the same failure on a LIST is unambiguous -- a list
        // that did not happen created nothing.
        assert_eq!(
            map_error(RestError::Transport("connection reset".to_string()), Ambiguity::Safe),
            crate::send::SendError::Offline
        );
        assert_eq!(
            map_error(RestError::Unauthorized, Ambiguity::Safe),
            crate::send::SendError::Locked
        );
    }
```

- [ ] **Step 2: Write the implementation**

```rust
/// Whether a failed operation could have left a **public link** behind.
///
/// Not a boolean at the call site: this is the distinction the whole module
/// is arranged around -- see `SendError::is_ambiguous` -- and a bare `true`
/// three lines from a `?` is how it gets passed the wrong way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ambiguity {
    /// A create: the request may have been served before the answer was lost.
    Ambiguous,
    /// A list or a revoke: a failure here published nothing.
    Safe,
}

/// One `RestError`, in the vocabulary the Sends screen already speaks.
pub fn map_error(error: RestError, ambiguity: Ambiguity) -> SendError {
    match error {
        RestError::Transport(_) if ambiguity == Ambiguity::Ambiguous => SendError::TimedOut,
        RestError::Transport(_) => SendError::Offline,
        RestError::Unauthorized | RestError::NoRefreshToken => SendError::Locked,
        RestError::Rejected { description, .. } if !description.is_empty() => {
            SendError::Rejected(format!("Bitwarden would not do it: {description}"))
        }
        RestError::Parse(_) if ambiguity == Ambiguity::Ambiguous => SendError::CreatedButUnreadable,
        other => SendError::Rejected(format!("Bitwarden would not do it: {other}")),
    }
}
```

Then `summary_from`, `create`, `list` and `delete`. `create` reads `accessId` off the answer, builds the URL with the key `MappedSend` kept, and answers a `CreatedSend`; an answer without an `id` or an `accessId` is `SendError::CreatedButUnreadable`, verbatim the rule `parse_created_send` applies — the Send exists and its link cannot be shown, which is the one failure this module refuses to call either a success or a clean failure.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib rest::send`.

---

### Task 6: The credentials a worker thread can reach

**Files:** Modify `deskwarden/src/backend_policy.rs`, `deskwarden/src/main.rs`

**Interfaces**

- *Produces:* `BackendEnv::credentials`, `backend_policy::direct_rest_credentials() -> Option<Arc<dyn Fn() -> Option<Authenticated> + Send + Sync>>`.

- [ ] **Step 1: Write the failing test**

In `backend_policy.rs`'s test module:

```rust
    /// **Paired with the choice, exactly as `direct` is.** A `BwServe`
    /// environment must not hand out REST credentials: a Send signed with a
    /// credential the vault is not being read through is the "reads from one
    /// backend, writes to the other" state this module exists to prevent.
    #[test]
    fn credentials_are_handed_out_only_to_the_backend_that_uses_them() {
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::BwServe,
            direct: None,
            credentials: Some(std::sync::Arc::new(|| None)),
        }));
        assert!(
            direct_rest_credentials().is_none(),
            "a bw serve account was handed direct-REST credentials"
        );
        uninstall_env();

        // The positive control: the same accessor answers `Some` for the
        // backend that does use them, or the assertion above would pass
        // against an accessor that always answered `None`.
        assert!(install_env(BackendEnv {
            choice: VaultBackendChoice::DirectRest,
            direct: Some(a_direct_login()),
            credentials: Some(std::sync::Arc::new(|| None)),
        }));
        assert!(direct_rest_credentials().is_some());
        uninstall_env();
    }
```

Every existing `BackendEnv { .. }` literal in the crate gains `credentials: None`; the compiler lists them.

- [ ] **Step 2: Write the implementation**

Add the field with the design's doc comment, the accessor beside `direct_rest_login` and gated the same way, and — in `main.rs`, in the same block that builds `adopt` — install it as a closure over the *same* `UserKeyStore`:

```rust
    // The same store `adopt` writes, read rather than written. One source, so
    // a Send cannot be signed with a credential the vault is not being read
    // through. A refresh performed during a Send is not written back here:
    // that costs the next Send one refresh round trip, and three worker
    // threads writing this file to save it is the worse trade.
    let credentials = {
        let key_store =
            user_key_store::UserKeyStore::new(accounts::user_key_path_for(config_dir, &account.id));
        std::sync::Arc::new(move || key_store.load())
            as std::sync::Arc<dyn Fn() -> Option<rest::api::Authenticated> + Send + Sync>
    };
```

`install_env` keeps its pairing check and gains the second half of it: `DirectRest` with no `credentials` is refused for the same stated reason as `DirectRest` with no `direct`.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib backend_policy`, then a full `--lib` run: `install_env` literals live in several modules' tests.

---

### Task 7: The branch, in the three places that already read process state

**Files:** Modify `deskwarden/src/vault_window/mod.rs`

**Interfaces**

- *Consumes:* `backend_policy::{selected, VaultBackendChoice, direct_rest_credentials, direct_rest_login}`, `rest::send::{create, list, delete}`, and the untouched `crate::send::cli_send_*`.
- *Produces:* no new public item. The three `real_send_*` helpers keep their exact signatures.

- [ ] **Step 1: Write the failing test**

In `send_delete_wiring`'s test module, beside
`the_revoke_child_is_spawned_into_the_job_this_window_holds`, which already
arms `job_object`'s thread-local spawn probe:

```rust
    /// **A direct-REST account starts no process.** The probe is
    /// thread-local, so this must be a call this test makes on its own
    /// thread -- which is exactly why `real_send_delete` is a plain blocking
    /// function and not something only reachable through a spawn.
    #[test]
    fn a_direct_rest_account_revokes_without_a_child() {
        let _guard = InstalledDirectRest::with_no_credentials();
        let probe = crate::job_object::spawn_probe();
        let report = real_send_delete("send-1", "Wi-Fi password", "unused-session");
        assert_eq!(probe.spawns(), 0, "a direct-REST revoke spawned `bw`");
        // With no credentials installed the answer is `Locked`, and that is
        // the assertion that proves the REST arm was reached at all -- a
        // `0` spawn count alone would also be produced by a function that
        // returned early for an unrelated reason.
        assert!(matches!(
            report,
            SendDeleteReport::Failed { error: crate::send::SendError::Locked, .. }
        ));
    }

    /// The control, and the one that matters most: **nothing installed still
    /// goes to the CLI.** Every test process, `examples/ui_preview` and every
    /// `bw serve` account are this case.
    #[test]
    fn with_no_environment_installed_the_revoke_is_still_a_child() {
        crate::backend_policy::uninstall_env();
        let probe = crate::job_object::spawn_probe();
        let _ = real_send_delete("send-1", "Wi-Fi password", "unused-session");
        assert_eq!(probe.spawns(), 1, "the bw serve path stopped spawning `bw`");
    }
```

Use the existing probe helper's real name — read
`the_revoke_child_is_spawned_into_the_job_this_window_holds` and copy its arming
and its assertion style rather than inventing an API. `InstalledDirectRest` is a
guard that installs a `DirectRest` `BackendEnv` and calls `uninstall_env` on
drop: this is process-wide state in a parallel suite, and a leaked `DirectRest`
gates `bw serve` off for every later test in the process.

- [ ] **Step 2: Write the implementation**

In each of the three helpers, one branch, read on the worker thread for the
reason the profile directory already is:

```rust
    pub(super) fn real_send_delete(id: &str, name: &str, session: &str) -> SendDeleteReport {
        // Read here and not captured from the frame, exactly as the profile
        // directory below is and for the same reason: an account switch
        // replaces this between the frame and the thread.
        if crate::backend_policy::selected() == crate::backend_policy::VaultBackendChoice::DirectRest
        {
            return match crate::rest::send::delete_for_active_account(id) {
                Ok(()) => SendDeleteReport::Deleted { name: name.to_string() },
                Err(error) => SendDeleteReport::Failed { name: name.to_string(), error },
            };
        }
        let data_dir = crate::bw_path::active_data_dir();
        match crate::send::cli_send_delete(delete_job(), data_dir.as_deref(), session, id) {
            // ... unchanged
        }
    }
```

`delete_for_active_account`, `list_for_active_account` and
`create_for_active_account` live in `rest::send` and are the three functions
that assemble a client and a session out of the process facts —
`direct_rest_login()` for the server URL, `direct_rest_credentials()` for the
`Authenticated`, `RestClient::sync` plus `VaultKeys::unwrap_from` for the user
key (the revoke skips that: a revoke needs no key). A missing credential is
`SendError::Locked`.

Keep the `bw serve` arm **textually unchanged**, including its long comment
about the environment block and the job object: that comment is about the CLI
path and it is still exactly true of it.

- [ ] **Step 3: Verify**

`RUSTFLAGS="-D warnings" cargo test --lib` in full, then `cargo test --no-run`
with the same flags, then `cargo test --bins`. Re-run the three source guards
by name and read their output:

- `vault_window::send_ui::source_pins::the_public_surface_of_the_send_module_is_exactly_these_items`
- `vault_window::send_ui::source_pins::every_mention_of_the_blocking_fetch_is_sealed_inside_the_spawning_module`
- `vault_window::send_delete_wiring::every_mention_of_the_blocking_delete_is_sealed_inside_its_own_module`

None of them should need a new needle: no `cli_send_*` call moved, no
`CliSendRunner` spelling appeared, and `crate::send` gained no `pub` item. **If
one of them fails, read it before changing it** — a guard failing here is more
likely to be reporting a real widening than to be stale.

---

### Task 8: The words for what is missing

**Files:** Modify `deskwarden/src/vault_window/send_ui.rs`

The design names two losses, and neither may be discovered by a user as a
silent failure.

- [ ] **Step 1: Write the failing test**

An assertion over `send_ui`'s copy that a file Send's row carries the sentence
the design quotes, and that the receive path on a direct-REST account says
what it needs rather than failing as though the link were bad. Both are pure
string tests over the pane's state, in the style `pane_state`'s existing tests
already use; each carries a positive control asserting the *other* row does
not say it.

- [ ] **Step 2 and 3: Wording, then the full suite**

> Deskwarden can send text, not files. This Send holds a file, so you can copy
> its link or delete it here, but it was made somewhere else.

> Reading a Send from a link needs Bitwarden's command-line tool. Publishing,
> listing and revoking your own Sends do not.

Then the full `--lib`, `--bins` and `--no-run` runs with `-D warnings`.

---

## What is deliberately not in this plan

- **File Sends.** `POST /api/sends/file/v2`, the Azure or multipart upload, the
  renewal route and the rollback. The design argues it; the short form is that
  this app cannot create one on either backend today, so deferring it is parity
  and not a subtraction.
- **Receiving a Send from a link over REST.** Bitwarden has moved the anonymous
  access route to a send-access-token grant and this tree cannot read the
  target server's handlers to learn which protocol it speaks. Guessing at an
  unauthenticated route that carries a decryption key is the wrong thing to
  guess at.
- **Editing a Send.** `PUT /api/sends/{id}` exists; the composer has no edit
  mode on either backend.
- **Writing a refreshed token back to `user_key_store`.** Task 6's note.

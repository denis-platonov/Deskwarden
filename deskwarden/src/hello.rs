//! Windows Hello quick unlock (design 3h's "Use Windows Hello" panel).
//!
//! The Bitwarden CLI has no biometric unlock of its own — `bw unlock` wants
//! the master password — so quick unlock works the way the official desktop
//! client's does: the master password is stored locally, sealed so that only
//! a successful Windows Hello verification can release it.
//!
//! Sealing, from the inside out:
//!
//! 1. A Windows Hello **KeyCredential** (`KeyCredentialManager`) named
//!    [`CREDENTIAL_NAME`] signs a fixed challenge. The private key lives in
//!    the TPM/credential guard and every `RequestSignAsync` forces the OS's
//!    Hello verification (face/fingerprint/PIN). RSA PKCS#1 v1.5 signing is
//!    deterministic, so the signature doubles as stable, high-entropy key
//!    material that simply does not exist until Hello has verified the user.
//! 2. SHA-256 over a domain-separation label + that signature derives an
//!    AES-256-GCM key, which seals the master password (random nonce).
//! 3. The sealed blob is DPAPI-wrapped ([`crate::session_store`]'s pattern)
//!    and written to `hello.bin` beside `session.bin`. DPAPI alone would not
//!    gate on Hello — it's the outer layer so the blob at rest is also bound
//!    to the Windows user account.
//!
//! Enrollment is **opt-in**: the login window offers a checkbox and only a
//! successful password unlock with it ticked calls [`enroll`]. A failed
//! decrypt (revoked credential, reset Hello, copied file) deletes the blob
//! and falls back to the password path.

use crate::session_store;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use windows::core::HSTRING;
use windows::Security::Credentials::{
    KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
};
use windows::Security::Cryptography::CryptographicBuffer;
use zeroize::{Zeroize, Zeroizing};

/// Name of the Hello key credential this app owns. Stable across versions:
/// renaming it would orphan every enrollment.
const CREDENTIAL_NAME: &str = "deskwarden-quick-unlock";

/// Fixed challenge whose signature is the key material. The *value* is not
/// secret (the signature is); it only has to be stable and unique to this
/// purpose.
const CHALLENGE: &[u8] = b"deskwarden hello quick-unlock challenge v1";

/// Domain-separation label mixed into the key derivation.
const KDF_LABEL: &[u8] = b"deskwarden hello quick-unlock aes key v1";

const NONCE_LEN: usize = 12;

/// Where the sealed master password lives (`hello.bin`, beside
/// `session.bin`). `None` if the config directory cannot be resolved at all.
pub fn blob_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "deskwarden", "deskwarden")?;
    Some(dirs.config_dir().join("hello.bin"))
}

/// What the login window needs to know to draw 3h's Hello panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelloState {
    /// Windows Hello is set up on this machine (`IsSupportedAsync`).
    pub available: bool,
    /// A sealed master password exists, so quick unlock can be offered.
    pub enrolled: bool,
}

impl HelloState {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            enrolled: false,
        }
    }
}

/// Probes Hello support and enrollment. Called once when the login window
/// opens — `IsSupportedAsync` is quick but not free, and the answer doesn't
/// change mid-dialog.
pub fn state() -> HelloState {
    let available = KeyCredentialManager::IsSupportedAsync()
        .and_then(|op| op.get())
        .unwrap_or(false);
    let enrolled = available && blob_path().map(|p| p.exists()).unwrap_or(false);
    HelloState {
        available,
        enrolled,
    }
}

/// Runs the Hello-gated signature and derives the AES key from it. This is
/// the step that pops the OS verification dialog.
///
/// `create` opens an existing credential for unlocking, or (re)creates it
/// for enrollment — `ReplaceExisting` because a stale credential from an
/// abandoned enrollment has no blob to pair with and is worthless.
fn hello_derived_key(create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);

    let result = if create {
        KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::ReplaceExisting,
        )
    } else {
        KeyCredentialManager::OpenAsync(&name)
    }
    .and_then(|op| op.get())
    .map_err(|e| format!("Windows Hello is unavailable: {e}"))?;

    match result.Status() {
        Ok(KeyCredentialStatus::Success) => {}
        Ok(KeyCredentialStatus::UserCanceled) => {
            return Err("Windows Hello was cancelled.".to_string())
        }
        Ok(KeyCredentialStatus::NotFound) => {
            return Err("No Windows Hello enrollment for Deskwarden on this machine.".to_string())
        }
        Ok(other) => return Err(format!("Windows Hello failed ({other:?})")),
        Err(e) => return Err(format!("Windows Hello failed: {e}")),
    }

    let credential = result
        .Credential()
        .map_err(|e| format!("Windows Hello returned no credential: {e}"))?;

    let challenge = CryptographicBuffer::CreateFromByteArray(CHALLENGE)
        .map_err(|e| format!("could not build the Hello challenge buffer: {e}"))?;
    let signed = credential
        .RequestSignAsync(&challenge)
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows Hello signing failed: {e}"))?;

    match signed.Status() {
        Ok(KeyCredentialStatus::Success) => {}
        Ok(KeyCredentialStatus::UserCanceled) => {
            return Err("Windows Hello was cancelled.".to_string())
        }
        Ok(other) => return Err(format!("Windows Hello verification failed ({other:?})")),
        Err(e) => return Err(format!("Windows Hello verification failed: {e}")),
    }

    let signature_buffer = signed
        .Result()
        .map_err(|e| format!("Windows Hello returned no signature: {e}"))?;
    let mut signature = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&signature_buffer, &mut signature)
        .map_err(|e| format!("could not read the Hello signature: {e}"))?;

    let key = derive_key(&signature);
    // The signature is the key material; don't leave a copy behind.
    let signature_bytes: &mut [u8] = &mut signature;
    signature_bytes.zeroize();
    Ok(key)
}

/// SHA-256(label ‖ signature) → AES-256 key. Split out (and pure) so the
/// seal/unseal round-trip is testable without Hello hardware.
fn derive_key(signature: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(signature);
    Zeroizing::new(hasher.finalize().into())
}

/// AES-256-GCM seal: `nonce ‖ ciphertext`. Pure; used by [`enroll`] under
/// the Hello-derived key and by tests under a fixed one.
fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("no randomness for the nonce: {e}"))?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| "sealing the master password failed".to_string())?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Inverse of [`seal`].
fn unseal(key: &[u8; 32], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    if blob.len() <= NONCE_LEN {
        return Err("the sealed blob is truncated".to_string());
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map(Zeroizing::new)
        .map_err(|_| {
            "could not open the sealed master password (was Windows Hello reset?)".to_string()
        })
}

/// Seals `master_password` behind Windows Hello and stores it. Pops the
/// Hello dialog (credential creation + signing). Only called after the very
/// same password just unlocked the vault, with the user's opt-in.
pub fn enroll(master_password: &str) -> Result<(), String> {
    let path = blob_path().ok_or("could not resolve the config directory")?;
    let key = hello_derived_key(true)?;
    let sealed = seal(&key, master_password.as_bytes())?;
    let wrapped = session_store::protect(&sealed)
        .map_err(|e| format!("DPAPI could not wrap the sealed password: {e}"))?;
    std::fs::write(&path, wrapped).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Releases the master password behind a Windows Hello verification. Pops
/// the Hello dialog. On any failure to *open* the blob (as opposed to Hello
/// being cancelled), the blob is deleted — it can never succeed again, and
/// keeping it would show a quick-unlock panel that always errors.
pub fn unlock_password() -> Result<Zeroizing<String>, String> {
    let path = blob_path().ok_or("could not resolve the config directory")?;
    let wrapped =
        std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let sealed = session_store::unprotect(&wrapped).map_err(|e| {
        let _ = std::fs::remove_file(&path);
        format!("DPAPI could not unwrap the sealed password: {e}")
    })?;

    let key = hello_derived_key(false)?;
    let plaintext = unseal(&key, &sealed).map_err(|e| {
        let _ = std::fs::remove_file(&path);
        e
    })?;

    String::from_utf8(plaintext.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| {
            let _ = std::fs::remove_file(&path);
            "the sealed master password is not valid UTF-8".to_string()
        })
}

/// Removes the enrollment (used when the account logs out: a sealed password
/// for an account the CLI no longer knows is a liability, not a feature).
pub fn unenroll() {
    if let Some(path) = blob_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_unseal_round_trip() {
        let key = derive_key(b"pretend hello signature");
        let sealed = seal(&key, b"hunter2 but longer").unwrap();
        let opened = unseal(&key, &sealed).unwrap();
        assert_eq!(opened.as_slice(), b"hunter2 but longer");
    }

    #[test]
    fn unseal_rejects_the_wrong_key() {
        let sealed = seal(&derive_key(b"signature A"), b"secret").unwrap();
        assert!(unseal(&derive_key(b"signature B"), &sealed).is_err());
    }

    #[test]
    fn unseal_rejects_tampered_ciphertext() {
        let key = derive_key(b"signature");
        let mut sealed = seal(&key, b"secret").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(unseal(&key, &sealed).is_err());
    }

    #[test]
    fn unseal_rejects_a_truncated_blob() {
        let key = derive_key(b"signature");
        assert!(unseal(&key, &[0u8; 8]).is_err());
    }

    #[test]
    fn distinct_signatures_derive_distinct_keys() {
        assert_ne!(
            derive_key(b"signature A").as_slice(),
            derive_key(b"signature B").as_slice()
        );
    }
}

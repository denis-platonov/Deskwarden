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
//! successful password unlock with it ticked calls [`enroll_for`]. A failed
//! decrypt (revoked credential, reset Hello, copied file) deletes the blob
//! and falls back to the password path.
//!
//! ## One credential, many accounts
//!
//! There is exactly **one** Hello credential ([`CREDENTIAL_NAME`]) for the
//! whole app, and accounts are separated *inside* the derivation by
//! [`accounts::hello_kdf_suffix_for`] — never by giving each account a
//! credential of its own. The creation option that would replace a credential
//! in place rotates its private key, which changes the signature, which changes
//! *every* account's derived key. See [`hello_derived_key`]: that option is
//! banned here, and `hello_never_asks_windows_to_replace_the_shared_credential`
//! is the guard.
//!
//! Each account's blob lives at [`accounts::hello_blob_path_for`], inside that
//! account's own directory — one file per account, never a shared one, so
//! un-enrolling or deleting an account cannot touch another's.
//!
//! ## What is still testable without Hello hardware
//!
//! `KeyCredentialManager` needs a real TPM-backed enrolment and a live user at
//! the machine, so nothing that touches it can run under `cargo test`. The
//! decisions are therefore split *out* of it: [`derive_key`], [`seal`],
//! [`unseal`], [`store_blob`], [`open_blob`] and [`state_from`] are all pure or
//! filesystem-only and are tested directly. What remains unpinned is exactly
//! the Windows calls themselves — creating/opening the credential and signing
//! the challenge — plus the *wiring* from the public `_for` functions into
//! them, which is held by source guards because there is no other way to hold
//! it.

use crate::accounts::{self, AccountId};
use crate::session_store;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
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

/// Where `account`'s sealed master password lives — inside that account's own
/// directory, never a path shared between accounts.
///
/// Delegates to [`accounts::hello_blob_path_for`] rather than re-spelling the
/// layout, so there is one definition of where an account's files are.
pub fn blob_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    accounts::hello_blob_path_for(config_dir, id)
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

/// The whole decision in [`state_for`], as a pure function: quick unlock is
/// offered only when Hello itself is usable **and** this account has a blob.
///
/// `available == false` forces `enrolled == false` deliberately. A leftover
/// blob on a machine where Hello has been turned off must not put a quick
/// unlock panel on screen that can only ever fail.
fn state_from(available: bool, blob_exists: bool) -> HelloState {
    HelloState {
        available,
        enrolled: available && blob_exists,
    }
}

/// Probes Hello support and `account`'s enrollment. Called once when the login
/// window opens — `IsSupportedAsync` is quick but not free, and the answer
/// doesn't change mid-dialog.
pub fn state_for(config_dir: &Path, id: &AccountId) -> HelloState {
    let available = KeyCredentialManager::IsSupportedAsync()
        .and_then(|op| op.get())
        .unwrap_or(false);
    state_from(available, blob_path_for(config_dir, id).exists())
}

/// Runs the Hello-gated signature and derives `account_suffix`'s AES key from
/// it. This is the step that pops the OS verification dialog.
///
/// `create` enrols rather than unlocks, and it asks for **`FailIfExists`,
/// never `ReplaceExisting`** — which is what this used to pass. The credential
/// named [`CREDENTIAL_NAME`] is SHARED BY EVERY ACCOUNT; accounts are separated
/// by `account_suffix` ([`accounts::hello_kdf_suffix_for`]), not by having a
/// credential each. Replacing it rotates the private key, which changes the
/// signature, which changes every derived key: enrolling a second account would
/// silently destroy the first one's enrolment, and the first account would find
/// out at the moment it next tried to unlock.
///
/// The old justification recorded here was that "a stale credential from an
/// abandoned enrollment has no blob to pair with and is worthless", so
/// replacing it was free. Both halves of that are now false. A stale *blob* is
/// dealt with by deleting the blob — [`unenroll_for`], [`open_blob`]'s
/// delete-on-failure, and `migration`'s removal of the pre-migration
/// `hello.bin`, which no account's suffix can open anyway. A stale *credential*
/// is not worthless: it is the one credential every other account depends on,
/// and rotating it is the single most destructive thing this module could do.
///
/// `CredentialAlreadyExists` is therefore the NORMAL case for the second and
/// every later enrolment, and falls through to opening the credential that is
/// already there.
fn hello_derived_key(create: bool, account_suffix: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);

    let result = if create {
        let created = KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::FailIfExists,
        )
        .and_then(|op| op.get());
        match created.as_ref().map(|r| r.Status()) {
            Ok(Ok(KeyCredentialStatus::CredentialAlreadyExists)) => {
                KeyCredentialManager::OpenAsync(&name).and_then(|op| op.get())
            }
            _ => created,
        }
    } else {
        KeyCredentialManager::OpenAsync(&name).and_then(|op| op.get())
    }
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

    let key = derive_key(&signature, account_suffix);
    // The signature is the key material; don't leave a copy behind.
    let signature_bytes: &mut [u8] = &mut signature;
    signature_bytes.zeroize();
    Ok(key)
}

/// SHA-256(label ‖ account suffix ‖ signature) → AES-256 key. Pure, and split
/// out for that reason: it is the whole of what separates one account from
/// another, and the only part of the key path that can be tested without Hello
/// hardware.
///
/// The suffix goes *before* the signature so a change of account can never be
/// mistaken for a change of signature by a length-extension-shaped argument;
/// with the fixed-length label first, the three fields are unambiguous.
fn derive_key(signature: &[u8], account_suffix: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(account_suffix);
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

/// Seals under `key`, DPAPI-wraps and writes — everything [`enroll_for`] does
/// once Hello has produced the key. Split out because this half needs no
/// hardware and can therefore be tested against [`open_blob`] directly.
///
/// `create_dir_all` on the parent is required, not defensive: an account's
/// `hello.bin` lives inside that account's directory, and enrolling is a thing
/// the user can do before anything else has had cause to create it.
fn store_blob(path: &Path, key: &[u8; 32], master_password: &str) -> Result<(), String> {
    let sealed = seal(key, master_password.as_bytes())?;
    let wrapped = session_store::protect(&sealed)
        .map_err(|e| format!("DPAPI could not wrap the sealed password: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, wrapped).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Reads, unwraps and opens the blob at `path`, asking `hello_key` for the AES
/// key only once there is something to open.
///
/// The key is behind a closure so the *order* is part of the function rather
/// than of its caller: a missing or DPAPI-unreadable blob must not pop the
/// Hello dialog first and fail afterwards. That ordering, and the
/// delete-on-failure below, are the whole behaviour here, and taking the key
/// as a closure is what lets a test drive them with no Hello hardware.
///
/// On any failure to *open* the blob (as opposed to Hello being cancelled) the
/// blob is deleted: it can never succeed again, and keeping it would show a
/// quick-unlock panel that always errors.
fn open_blob(
    path: &Path,
    hello_key: impl FnOnce() -> Result<Zeroizing<[u8; 32]>, String>,
) -> Result<Zeroizing<String>, String> {
    let wrapped =
        std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let sealed = session_store::unprotect(&wrapped).map_err(|e| {
        let _ = std::fs::remove_file(path);
        format!("DPAPI could not unwrap the sealed password: {e}")
    })?;

    let key = hello_key()?;
    let plaintext = unseal(&key, &sealed).map_err(|e| {
        let _ = std::fs::remove_file(path);
        e
    })?;

    String::from_utf8(plaintext.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| {
            let _ = std::fs::remove_file(path);
            "the sealed master password is not valid UTF-8".to_string()
        })
}

/// Seals `master_password` behind Windows Hello and stores it for `id`. Pops
/// the Hello dialog (credential creation + signing). Only called after the very
/// same password just unlocked that account's vault, with the user's opt-in.
pub fn enroll_for(config_dir: &Path, id: &AccountId, master_password: &str) -> Result<(), String> {
    let suffix = accounts::hello_kdf_suffix_for(id);
    let key = hello_derived_key(true, &suffix)?;
    store_blob(&blob_path_for(config_dir, id), &key, master_password)
}

/// Releases `id`'s master password behind a Windows Hello verification. Pops
/// the Hello dialog.
pub fn unlock_password_for(config_dir: &Path, id: &AccountId) -> Result<Zeroizing<String>, String> {
    let suffix = accounts::hello_kdf_suffix_for(id);
    open_blob(&blob_path_for(config_dir, id), || {
        hello_derived_key(false, &suffix)
    })
}

/// Removes `id`'s enrollment (used when that account logs out: a sealed
/// password for an account the CLI no longer knows is a liability, not a
/// feature). Touches only that account's file.
pub fn unenroll_for(config_dir: &Path, id: &AccountId) {
    let _ = std::fs::remove_file(blob_path_for(config_dir, id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    /// The signature Hello would return. Fixed here; its *value* is not what
    /// any of these tests is about.
    const SIG: &[u8] = b"pretend hello signature";

    fn id(s: &str) -> AccountId {
        AccountId::parse(s).expect("test ids must be valid")
    }

    /// A unique scratch directory. Same `temp_dir()` + nanos pattern the rest
    /// of the suite uses, and never the real config directory — nothing here
    /// may go near the user's own `hello.bin` or Hello enrolment.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-hello-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key_for(account: &AccountId) -> Zeroizing<[u8; 32]> {
        derive_key(SIG, &accounts::hello_kdf_suffix_for(account))
    }

    #[test]
    fn seal_and_unseal_round_trip() {
        let key = derive_key(SIG, b"");
        let sealed = seal(&key, b"hunter2 but longer").unwrap();
        let opened = unseal(&key, &sealed).unwrap();
        assert_eq!(opened.as_slice(), b"hunter2 but longer");
    }

    #[test]
    fn unseal_rejects_the_wrong_key() {
        let sealed = seal(&derive_key(b"signature A", b""), b"secret").unwrap();
        assert!(unseal(&derive_key(b"signature B", b""), &sealed).is_err());
    }

    #[test]
    fn unseal_rejects_tampered_ciphertext() {
        let key = derive_key(b"signature", b"");
        let mut sealed = seal(&key, b"secret").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(unseal(&key, &sealed).is_err());
    }

    #[test]
    fn unseal_rejects_a_truncated_blob() {
        let key = derive_key(b"signature", b"");
        assert!(unseal(&key, &[0u8; 8]).is_err());
    }

    #[test]
    fn distinct_signatures_derive_distinct_keys() {
        assert_ne!(
            derive_key(b"signature A", b"").as_slice(),
            derive_key(b"signature B", b"").as_slice()
        );
    }

    // ---- the derivation is what separates accounts -----------------------

    #[test]
    fn a_blob_sealed_for_one_account_does_not_open_for_another() {
        // The point of the suffix, stated as the attack it prevents: one
        // Windows Hello verification produces ONE signature, so without the
        // suffix every account's key would be the same key and any account
        // could open any other's sealed master password. `derive_key`
        // ignoring `account_suffix` fails this.
        let (a, b) = (id(A), id(B));
        let sealed = seal(&key_for(&a), b"account A master password").unwrap();

        assert!(
            unseal(&key_for(&b), &sealed).is_err(),
            "account B opened account A's sealed password"
        );

        // Positive control on the SAME blob: the failure above is the suffix,
        // not a seal that nothing can open.
        assert_eq!(
            unseal(&key_for(&a), &sealed).unwrap().as_slice(),
            b"account A master password"
        );
    }

    #[test]
    fn no_account_reproduces_the_pre_migration_derivation() {
        // The pre-accounts key was SHA-256(KDF_LABEL ‖ signature) -- an empty
        // suffix. If any account reproduced it, a `hello.bin` that a FAILED
        // migration left behind in the config directory could be opened under
        // that account's identity. `hello_kdf_suffix_for` returning an empty
        // suffix for any id -- the obvious "the first account keeps working"
        // shortcut -- fails this.
        let mut old = Sha256::new();
        old.update(KDF_LABEL);
        old.update(SIG);
        let old: [u8; 32] = old.finalize().into();

        for raw in [A, B, &"0".repeat(32), &"f".repeat(32)] {
            assert_ne!(
                key_for(&id(raw)).as_slice(),
                &old,
                "account {raw} derives the pre-migration key"
            );
        }

        // Positive control: the two derivations differ ONLY because of the
        // suffix, so what is pinned above is the suffix and not some unrelated
        // change to the KDF that would make every comparison unequal.
        assert_eq!(derive_key(SIG, b"").as_slice(), &old);
    }

    #[test]
    fn distinct_accounts_derive_distinct_keys() {
        // Over generated ids rather than two hand-picked ones, so a suffix
        // that collapsed to a constant for anything but the two literals
        // above still fails.
        let ids: Vec<AccountId> = (0..8).map(|_| AccountId::generate()).collect();
        let keys: HashSet<[u8; 32]> = ids.iter().map(|i| *key_for(i)).collect();
        assert_eq!(keys.len(), ids.len(), "two accounts share a derived key");

        // Positive control: the same account twice is the same key, so the set
        // above is not merely counting a randomised derivation.
        assert_eq!(key_for(&ids[0]).as_slice(), key_for(&ids[0]).as_slice());
    }

    // ---- paths follow the account ----------------------------------------

    #[test]
    fn no_two_accounts_share_a_blob_path() {
        // Asserted over a set of ids, not by reading the two paths: the
        // failure this guards against is one account's blob overwriting or
        // deleting another's, and inspection of two examples is how a
        // path that ignores its id for some inputs gets missed.
        let cfg = Path::new(r"C:\cfg");
        let ids: Vec<AccountId> = (0..16).map(|_| AccountId::generate()).collect();
        let paths: HashSet<PathBuf> = ids.iter().map(|i| blob_path_for(cfg, i)).collect();
        assert_eq!(paths.len(), ids.len(), "two accounts share a hello.bin");

        // ...and none of them is the pre-accounts location, which a failed
        // migration can still have a file at.
        for i in &ids {
            assert_ne!(blob_path_for(cfg, i), cfg.join("hello.bin"));
        }

        // Positive control: the same id twice is the same path, so the set
        // above is measuring the id and not some per-call randomness.
        assert_eq!(blob_path_for(cfg, &ids[0]), blob_path_for(cfg, &ids[0]));
    }

    #[test]
    fn a_blob_and_its_removal_follow_the_account() {
        let cfg = scratch_dir("per-account-blob");
        let (a, b) = (id(A), id(B));

        std::fs::create_dir_all(blob_path_for(&cfg, &a).parent().unwrap()).unwrap();
        std::fs::write(blob_path_for(&cfg, &a), b"not a real blob").unwrap();

        assert!(
            !blob_path_for(&cfg, &b).exists(),
            "B must not see A's enrolment"
        );
        assert!(
            !state_from(true, blob_path_for(&cfg, &b).exists()).enrolled,
            "B must not be offered quick unlock on A's blob"
        );
        assert!(
            state_from(true, blob_path_for(&cfg, &a).exists()).enrolled,
            "positive control: A, which does have a blob, is enrolled"
        );

        unenroll_for(&cfg, &b);
        assert!(
            blob_path_for(&cfg, &a).exists(),
            "unenrolling B deleted A's blob"
        );

        unenroll_for(&cfg, &a);
        assert!(
            !blob_path_for(&cfg, &a).exists(),
            "unenrolling A did nothing"
        );

        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn hello_being_unavailable_beats_a_blob_on_disk() {
        // A blob left over from before Hello was turned off must not put a
        // quick-unlock panel on screen that can only ever fail. Deleting the
        // `available &&` in `state_from` fails this; its positive control is
        // the line below it.
        assert!(!state_from(false, true).enrolled);
        assert!(state_from(true, true).enrolled);
        assert!(!state_from(true, false).enrolled);
    }

    // ---- store/open, everything about a real enrolment except Hello ------

    #[test]
    fn a_stored_blob_opens_under_its_own_account_and_no_other() {
        // The full at-rest round trip -- seal, DPAPI-wrap, write, read,
        // unwrap, unseal -- with the ONE Hello-shaped hole filled by a fixed
        // signature. `store_blob`/`open_blob` losing the key, or `enroll_for`
        // and `unlock_password_for` disagreeing about the suffix, land here.
        let cfg = scratch_dir("store-open");
        let (a, b) = (id(A), id(B));
        let (path_a, key_a, key_b) = (blob_path_for(&cfg, &a), key_for(&a), key_for(&b));

        store_blob(&path_a, &key_a, "correct horse battery staple").unwrap();
        assert!(path_a.exists(), "nothing was written");

        assert_eq!(
            open_blob(&path_a, || Ok(key_a.clone())).unwrap().as_str(),
            "correct horse battery staple"
        );

        // B's key on A's blob: refused, and the blob it could not open is gone.
        assert!(open_blob(&path_a, || Ok(key_b.clone())).is_err());
        assert!(
            !path_a.exists(),
            "a blob that could not be opened must be deleted, not left to fail forever"
        );

        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn storing_a_blob_creates_the_accounts_directory() {
        // Enrolling is something the user can do before anything else has had
        // cause to create the account's directory. Dropping the
        // `create_dir_all` from `store_blob` gives
        //     called `Result::unwrap()` on an `Err` value: "could not write
        //     ...\\accounts\\0123...\\hello.bin: The system cannot find the
        //     path specified. (os error 3)"
        let cfg = scratch_dir("store-creates-dir");
        let a = id(A);
        let path = blob_path_for(&cfg, &a);
        assert!(
            !path.parent().unwrap().exists(),
            "positive control: the directory must NOT exist before the call"
        );

        store_blob(&path, &key_for(&a), "pw").unwrap();

        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn a_missing_or_unreadable_blob_never_pops_the_hello_dialog() {
        // Ordering, and it is user-visible: being asked for a fingerprint and
        // THEN told there was nothing to unlock is the failure. Moving the
        // `hello_key()` call to the top of `open_blob` fails both halves.
        let cfg = scratch_dir("no-pointless-prompt");
        let a = id(A);
        let path = blob_path_for(&cfg, &a);
        let asked = std::cell::Cell::new(0);
        let key = || {
            asked.set(asked.get() + 1);
            Ok(key_for(&a))
        };

        assert!(open_blob(&path, key).is_err(), "there is no blob to open");
        assert_eq!(asked.get(), 0, "Hello was asked to verify for nothing");

        // Present but not DPAPI-wrapped: same rule, and the blob is deleted.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not DPAPI output").unwrap();
        assert!(open_blob(&path, key).is_err());
        assert_eq!(asked.get(), 0, "Hello was asked to verify an unusable blob");
        assert!(!path.exists(), "the unusable blob was left behind");

        // Positive control: the same closure IS called when there is something
        // to open, so the counter above is not stuck at zero.
        store_blob(&path, &key_for(&a), "pw").unwrap();
        assert_eq!(open_blob(&path, key).unwrap().as_str(), "pw");
        assert_eq!(asked.get(), 1);

        let _ = std::fs::remove_dir_all(&cfg);
    }

    // ---- what only a source guard can hold -------------------------------

    #[test]
    fn hello_never_asks_windows_to_replace_the_shared_credential() {
        // WIRING, and unavoidably a source guard: `RequestCreateAsync` needs
        // real TPM-backed Hello hardware and a live user, so no test can call
        // it. What it would do is the point -- the replacing option rotates
        // the ONE credential every account's key comes from, so enrolling
        // account B would silently destroy account A's enrolment and A would
        // find out at the moment it next tried to unlock. Migration makes that
        // worse rather than better: it is what leaves stale blobs around, and
        // the answer to a stale blob is to DELETE IT, never to rotate the
        // credential.
        //
        // Both needles are `concat!`-split so neither matches its own
        // declaration here, and single-line so neither depends on whether this
        // file is checked out with LF or CRLF endings.
        let source = include_str!("hello.rs");
        let banned = concat!("KeyCredentialCreationOption", "::ReplaceExisting");
        let required = concat!("KeyCredentialCreationOption", "::FailIfExists");

        assert_eq!(
            source.matches(banned).count(),
            0,
            "`{banned}` must not appear anywhere in this file"
        );
        assert_eq!(
            source.matches(required).count(),
            1,
            "enrolment must ask for `{required}`, exactly once"
        );

        // Positive control on the counting itself: it can see occurrences, and
        // it found one of them in the real source above rather than in a
        // planted string.
        assert_eq!(format!("{banned} and {banned}").matches(banned).count(), 2);
    }

    #[test]
    fn every_key_derivation_is_reached_with_the_right_suffix() {
        // WIRING. `hello_derived_key` cannot be called without Hello hardware,
        // so the fact that it passes the account's suffix through to
        // `derive_key` -- and that the per-account entry points hand it the
        // account's suffix rather than the empty pre-accounts one -- is held
        // here or nowhere. Every mutation below leaves the rest of the suite
        // green, because nothing else in it can execute these lines.
        let source = include_str!("hello.rs");
        let count = |needle: &str| source.matches(needle).count();

        // The suffix reaches the KDF at all.
        assert_eq!(
            count(concat!("derive_key(&signature, ", "account_suffix)")),
            1,
            "`hello_derived_key` must derive under the suffix it was given"
        );

        // The two per-account entry points derive under the ACCOUNT's suffix.
        // This is also the positive control for every count here: it reads 2
        // off the real source, so the mechanism can both find needles and tell
        // one occurrence from more than one. Every other assertion below is
        // for a non-zero count, which a mechanism that found nothing fails.
        assert_eq!(
            count(concat!("accounts::hello_kdf_suffix", "_for(id)")),
            2,
            "`enroll_for` and `unlock_password_for` must each take the account's suffix"
        );
        assert_eq!(count(concat!("hello_derived_key(true, ", "&suffix)")), 1);
        assert_eq!(count(concat!("hello_derived_key(false, ", "&suffix)")), 1);

        // ...and the empty suffix is now reachable from NOWHERE. The two
        // pre-accounts entry points that used it are gone with their last
        // caller, so an empty-suffix derivation could only come back as a
        // fallback for "no account" -- which would seal one account's master
        // password where every account can open it. The `_for(id)` count above
        // is this pair's positive control: it proves the counter finds real
        // needles, so a zero here is an absence and not a broken mechanism.
        assert_eq!(count(concat!("hello_derived_key(true, ", "&[])")), 0);
        assert_eq!(count(concat!("hello_derived_key(false, ", "&[])")), 0);
    }
}

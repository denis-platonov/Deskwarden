//! The optional encrypted on-disk vault snapshot.
//!
//! Off by default. When `Settings::cache_vault_to_disk` is on, the snapshot
//! `VaultCache` holds in memory is also written here, so the next launch
//! reads it in milliseconds instead of paying `bw serve`'s ~8 s cold start
//! before autofill works or the vault window has anything to show.
//!
//! What gates the file, from the inside out:
//!
//! 1. A random 32-byte **content key** encrypts the snapshot (AES-256-GCM),
//!    with the plaintext header as additional authenticated data.
//! 2. That content key is itself sealed (AES-256-GCM) under a key derived
//!    from a **Windows Hello** signature -- `hello.rs`'s existing pattern
//!    applied to a second secret, under its own domain-separation label.
//! 3. The whole thing is DPAPI-wrapped, exactly as `hello.bin` is.
//!
//! The property that justifies the feature: Hello's private key lives in
//! this machine's TPM, so a stolen or imaged disk plus the Windows account
//! password yields the header and two ciphertexts and nothing else. DPAPI
//! alone cannot make that claim, because DPAPI derives from the Windows
//! account credentials, which travel with the image.
//!
//! The header is plaintext *inside* the DPAPI envelope on purpose. DPAPI
//! unwrapping is silent and non-interactive, so the app can read the header,
//! decide the file is expired or belongs to a different account, and delete
//! it **without ever popping a Hello prompt** for a file it is about to
//! throw away.
//!
//! ## What is testable without Hello hardware, and how
//!
//! Two things here reach the OS and nothing in this crate's test suite may:
//! the Hello signature and DPAPI. Both are behind [`DiskCacheEnv`]'s `fn`
//! pointers -- `single_instance::TakeoverEnv`'s idiom -- so every decision on
//! this side of the seam (the format, the header checks, which failures
//! delete the file and which leave it) is driven directly, and no test
//! derives a key, pops a prompt, or wraps a byte.

use crate::vault_bridge::{Folder, VaultItem};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::core::HSTRING;
use windows::Security::Credentials::{
    KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
};
use windows::Security::Cryptography::CryptographicBuffer;
use zeroize::{Zeroize, Zeroizing};

/// File magic, so a foreign or truncated file is rejected before anything
/// tries to interpret its length prefixes.
const MAGIC: &[u8; 4] = b"DWVC";

/// Bumped only for an incompatible layout change. An unknown version is
/// rejected and the file deleted -- there is nothing to migrate, the vault
/// is regenerable from the backend in seconds.
pub const FORMAT_VERSION: u32 = 1;

/// How long a file stays loadable.
///
/// Seven days, and the number is chosen entirely for the abandoned-machine
/// case -- which is the stolen-disk case -- because the file is rewritten on
/// every successful populate, so on a machine in daily use it is hours old,
/// never days. It has to survive the ordinary gaps in a person's usage (a
/// laptop shut on Friday and opened on Monday, a holiday) or the feature
/// quietly stops working for exactly the users who notice cold starts. It
/// also has to be short enough to be a real mitigation: 30 days is theatre,
/// since a stolen disk is imaged in days and a month-old vault dump is still
/// overwhelmingly accurate. A week is the shortest interval that costs the
/// user nothing visible.
///
/// Deliberately a constant and not a setting: making it configurable invites
/// `expiry_days: 3650`, which is no expiry with extra steps.
pub const EXPIRY_SECS: u64 = 7 * 24 * 60 * 60;

/// Clock skew allowed before a `written_at` in the future is treated as
/// evidence the file is invalid (clock moved backwards, or the file came
/// from another machine's timeline). Some tolerance is needed because NTP
/// corrections and suspend/resume routinely move the clock by seconds.
const FUTURE_TOLERANCE_SECS: u64 = 5 * 60;

const NONCE_LEN: usize = 12;
const CONTENT_KEY_LEN: usize = 32;

pub(crate) const FILE_NAME: &str = "vault-cache.bin";
pub(crate) const TMP_FILE_NAME: &str = "vault-cache.bin.tmp";

/// The same Hello key credential quick unlock uses. One credential, two
/// sealed blobs, separated by their derivation labels -- see [`KDF_LABEL`]
/// below and `hello.rs`'s own.
const CREDENTIAL_NAME: &str = "deskwarden-quick-unlock";

/// Distinct from `hello.rs`'s challenge. The spec only requires the *label*
/// to differ; a distinct challenge is strictly additional separation at no
/// cost.
const CHALLENGE: &[u8] = b"deskwarden vault cache challenge v1";

/// Domain separation from quick unlock's key. Sharing `hello.rs`'s label
/// would make the two sealed blobs cross-decryptable, which is sloppy for
/// no gain.
///
/// **Deliberately not mixed with an account suffix**, which is the one place
/// this differs from `hello.rs`'s derivation. Accounts are separated here by
/// the file's location -- one `vault-cache.bin` inside each account's own
/// directory, beside its `session.bin` and `hello.bin` -- and by the header's
/// account fingerprint, which refuses a file belonging to anyone else before
/// a key is derived at all. Mixing the suffix in as well would mean a fresh
/// Hello prompt at every account switch, for separation those two already
/// provide.
const KDF_LABEL: &[u8] = b"deskwarden vault cache aes key v1";

/// Plaintext metadata, inside the DPAPI envelope and outside the sealed
/// body. Also the body's AAD, so none of it can be edited without failing
/// authentication -- notably `written_at`, which expiry depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheHeader {
    pub format_version: u32,
    /// Unix seconds. Authenticated via the AAD binding.
    pub written_at: u64,
    /// `SHA-256(userEmail ‖ 0x1F ‖ serverUrl)`, hex. A hash and not the
    /// values: the file should not be the thing that tells an examiner whose
    /// vault it is.
    pub account_fingerprint: String,
    pub item_count: usize,
}

/// What actually gets encrypted: the existing snapshot types, so the
/// `#[serde(flatten)] other` catch-alls that already round-trip unknown
/// server fields keep doing so across disk.
#[derive(Clone, Serialize, Deserialize)]
pub struct DiskSnapshot {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
}

/// Why a file was refused. Every variant means "delete it and fall back to
/// the backend"; they are distinct so the log says something useful and so
/// the tests can assert on the reason rather than on a bare `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    UnknownVersion,
    Expired,
    ForeignAccount,
    FutureTimestamp,
    Malformed,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectReason::UnknownVersion => "written by a different version of Deskwarden",
            RejectReason::Expired => "older than 7 days",
            RejectReason::ForeignAccount => "belongs to a different account",
            RejectReason::FutureTimestamp => "timestamped in the future",
            RejectReason::Malformed => "malformed",
        }
    }
}

/// The byte spans [`decode_body`] needs, produced by [`parse_header`] so the
/// caller can validate the header and bail out before any Hello prompt.
pub(crate) struct Parsed {
    pub(crate) header_bytes: Vec<u8>,
    pub(crate) sealed_key: Vec<u8>,
    pub(crate) body: Vec<u8>,
}

/// `SHA-256(userEmail ‖ 0x1F ‖ serverUrl)`, hex.
///
/// The 0x1F separator is a unit separator, not decoration: without it
/// `("ab", "c")` and `("a", "bc")` hash identically, which would let one
/// account's file be accepted under another's identity.
pub fn account_fingerprint(user_email: Option<&str>, server_url: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_email.unwrap_or("").as_bytes());
    hasher.update([0x1fu8]);
    hasher.update(server_url.unwrap_or("").as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Header validation, pure and total, so the (setting, file) decision table
/// is unit-tested rather than reasoned about at a call site.
pub fn check_header(
    header: &CacheHeader,
    now_unix: u64,
    fingerprint: &str,
) -> Result<(), RejectReason> {
    if header.format_version != FORMAT_VERSION {
        return Err(RejectReason::UnknownVersion);
    }
    if header.written_at > now_unix.saturating_add(FUTURE_TOLERANCE_SECS) {
        return Err(RejectReason::FutureTimestamp);
    }
    if now_unix.saturating_sub(header.written_at) > EXPIRY_SECS {
        return Err(RejectReason::Expired);
    }
    if header.account_fingerprint != fingerprint {
        return Err(RejectReason::ForeignAccount);
    }
    Ok(())
}

/// Builds the complete file body (everything that goes *inside* the DPAPI
/// envelope).
///
/// Takes the Hello-derived key as a parameter, and there is deliberately no
/// other way to build a file. If a path ever appears that writes one without
/// a Hello seal, the setting's description becomes a false security claim.
pub(crate) fn encode_file(
    hello_key: &[u8; 32],
    header: &CacheHeader,
    snapshot: &DiskSnapshot,
) -> Result<Vec<u8>, String> {
    let header_bytes = serde_json::to_vec(header)
        .map_err(|e| format!("could not serialize the cache header: {e}"))?;

    // A fresh random content key per write. Reusing one across writes would
    // mean reusing a key across many nonces for no benefit; generating one
    // costs nothing at human-paced write frequency.
    let mut content_key = Zeroizing::new([0u8; CONTENT_KEY_LEN]);
    getrandom::getrandom(content_key.as_mut_slice())
        .map_err(|e| format!("no randomness for the content key: {e}"))?;

    let plaintext = Zeroizing::new(
        serde_json::to_vec(snapshot)
            .map_err(|e| format!("could not serialize the vault snapshot: {e}"))?,
    );

    let sealed_key = seal(hello_key, content_key.as_slice(), None)?;
    let body = seal(&content_key, &plaintext, Some(&header_bytes))?;

    let mut out =
        Vec::with_capacity(MAGIC.len() + 4 + header_bytes.len() + 4 + sealed_key.len() + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&(sealed_key.len() as u32).to_le_bytes());
    out.extend_from_slice(&sealed_key);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads the plaintext header and splits out the spans the body needs.
/// **Performs no key derivation**, which is what lets a doomed file be
/// deleted without a Hello prompt.
pub(crate) fn parse_header(bytes: &[u8]) -> Result<(CacheHeader, Parsed), String> {
    let mut cursor = 0usize;
    let take = |cursor: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = cursor
            .checked_add(n)
            .ok_or_else(|| "the cache file is truncated".to_string())?;
        let slice = bytes
            .get(*cursor..end)
            .ok_or_else(|| "the cache file is truncated".to_string())?;
        *cursor = end;
        Ok(slice)
    };

    if take(&mut cursor, MAGIC.len())? != MAGIC {
        return Err("not a Deskwarden vault cache file".to_string());
    }
    let header_len = u32::from_le_bytes(
        take(&mut cursor, 4)?
            .try_into()
            .map_err(|_| "the cache file is truncated".to_string())?,
    ) as usize;
    let header_bytes = take(&mut cursor, header_len)?.to_vec();
    let header: CacheHeader = serde_json::from_slice(&header_bytes)
        .map_err(|e| format!("the cache header is malformed: {e}"))?;

    let sealed_key_len = u32::from_le_bytes(
        take(&mut cursor, 4)?
            .try_into()
            .map_err(|_| "the cache file is truncated".to_string())?,
    ) as usize;
    let sealed_key = take(&mut cursor, sealed_key_len)?.to_vec();
    let body = bytes
        .get(cursor..)
        .ok_or_else(|| "the cache file is truncated".to_string())?
        .to_vec();
    if body.len() <= NONCE_LEN {
        return Err("the cache file is truncated".to_string());
    }

    Ok((
        header,
        Parsed {
            header_bytes,
            sealed_key,
            body,
        },
    ))
}

/// Unseals the content key under the Hello-derived key, then decrypts the
/// snapshot with the header as AAD.
pub(crate) fn decode_body(hello_key: &[u8; 32], parsed: &Parsed) -> Result<DiskSnapshot, String> {
    let content_key = unseal(hello_key, &parsed.sealed_key, None)?;
    let content_key: [u8; CONTENT_KEY_LEN] = content_key
        .as_slice()
        .try_into()
        .map_err(|_| "the sealed content key is the wrong size".to_string())?;
    let content_key = Zeroizing::new(content_key);
    let plaintext = unseal(&content_key, &parsed.body, Some(&parsed.header_bytes))?;
    serde_json::from_slice(&plaintext)
        .map_err(|e| format!("the cached vault snapshot is malformed: {e}"))
}

/// AES-256-GCM: `nonce ‖ ciphertext`, optionally binding `aad`.
///
/// `hello.rs` has the same pair of helpers without the AAD parameter. They
/// are not shared: those are private to that module, this one needs the AAD
/// (which is the whole `written_at`-cannot-be-edited property), and the two
/// blobs must stay independently reasoned about.
fn seal(key: &[u8; 32], plaintext: &[u8], aad: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("no randomness for the nonce: {e}"))?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: aad.unwrap_or(&[]),
            },
        )
        .map_err(|_| "encrypting the vault cache failed".to_string())?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Where one item's sealed bytes are in the body, and which item they are.
///
/// **Plaintext, in the header, on purpose.** Encrypting the index would mean
/// opening the facts section to discover where an item is, which is the
/// whole cost this format exists to avoid. What it exposes is a list of
/// vault ids -- GUIDs the server assigned, which already travel in URLs --
/// and a count the header published anyway.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemSlot {
    pub id: String,
    /// Byte offset from the start of the body.
    pub at: u32,
    pub len: u32,
}

/// The body of a version 2 file: the facts section, then one sealed blob per
/// item, with the header's index saying where each one is.
///
/// `facts` is opaque to this module. **It does not know what a projection**
/// **is**, and should not: `app::ItemFacts` is a decision about what the
/// picker needs, and a sealing layer that had opinions about it would be two
/// modules that must agree. The caller serialises whatever it wants
/// available without a secret; this seals it.
pub fn seal_body(
    content_key: &[u8; 32],
    header_bytes: &[u8],
    facts: &[u8],
    items: &[(String, Vec<u8>)],
) -> Result<(Vec<u8>, Vec<ItemSlot>), String> {
    let mut body = seal(content_key, facts, Some(header_bytes))?;
    let mut index = Vec::with_capacity(items.len());
    for (id, plaintext) in items {
        let sealed = seal(content_key, plaintext, Some(&item_aad(header_bytes, id)))?;
        let at = u32::try_from(body.len())
            .map_err(|_| "the vault cache body outgrew a 32-bit offset".to_string())?;
        let len = u32::try_from(sealed.len())
            .map_err(|_| "one cached item outgrew a 32-bit length".to_string())?;
        body.extend_from_slice(&sealed);
        index.push(ItemSlot { id: id.clone(), at, len });
    }
    Ok((body, index))
}

/// The facts section, which is the first blob in the body.
///
/// `facts_len` comes from the header rather than being inferred, so a body
/// truncated between the facts and the first item is a refusal rather than a
/// slice that happens to parse.
pub fn open_facts(
    content_key: &[u8; 32],
    header_bytes: &[u8],
    body: &[u8],
    facts_len: u32,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let end = facts_len as usize;
    let slice = body.get(..end).ok_or_else(|| "the facts section is truncated".to_string())?;
    unseal(content_key, slice, Some(header_bytes))
}

/// One item, by its slot.
///
/// Every failure is the same answer -- refuse -- but they are distinct
/// strings because the only place this can go wrong quietly is a slot that
/// points somewhere plausible.
pub fn open_item_at(
    content_key: &[u8; 32],
    header_bytes: &[u8],
    body: &[u8],
    slot: &ItemSlot,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let at = slot.at as usize;
    let end = at.checked_add(slot.len as usize).ok_or_else(|| {
        "a cached item's slot runs past the end of addressable memory".to_string()
    })?;
    let slice = body
        .get(at..end)
        .ok_or_else(|| format!("the slot for {} runs past the body", slot.id))?;
    unseal(content_key, slice, Some(&item_aad(header_bytes, &slot.id)))
}

/// What binds one item's ciphertext to one item, in one file.
///
/// # Why an item needs more than the header
///
/// Version 1 sealed the whole snapshot as a single message, so "which item
/// is this?" could not be asked: there was one. Version 2 seals each item
/// separately -- which is what lets the daemon open one without the other
/// 1,665 -- and that makes the entries *fungible* unless something says
/// otherwise.
///
/// GCM authenticates that a message was sealed under a key. It does not
/// authenticate where the message was stored. Without the id here, anyone
/// who can write this file can move the entry for a bank into the slot for a
/// site they control; the daemon would open it, find a valid item, and type
/// that password into the site it was asked about. Nothing downstream could
/// notice, because everything downstream is working correctly.
///
/// # Length-prefixed, not concatenated
///
/// `header ‖ id` alone would let `("ab", "c")` and `("a", "bc")` produce the
/// same binding. Vault ids are server-assigned GUIDs and a separator would
/// be enough for them today -- but the prefix costs four bytes and removes
/// the need to keep being right about what an id may contain.
fn item_aad(header: &[u8], id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + 4 + id.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(&(id.len() as u32).to_le_bytes());
    aad.extend_from_slice(id.as_bytes());
    aad
}

fn unseal(key: &[u8; 32], blob: &[u8], aad: Option<&[u8]>) -> Result<Zeroizing<Vec<u8>>, String> {
    if blob.len() <= NONCE_LEN {
        return Err("the sealed blob is truncated".to_string());
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: aad.unwrap_or(&[]),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| "could not open the encrypted vault cache".to_string())
}

// ---------------------------------------------------------------------------
// The file on disk
// ---------------------------------------------------------------------------

/// The outside-world half of the disk cache, as **`fn` pointers**.
///
/// `single_instance::TakeoverEnv`'s idiom, and the constraint that forces it
/// is the same one, in its strictest form: **no test in this crate may pop a
/// Windows Hello prompt or call DPAPI.** The first needs a live user at the
/// machine and would rotate nothing but would ask a person for a fingerprint
/// in the middle of `cargo test`; the second is a real Win32 call against the
/// user's own credentials. Both are one line each and neither carries a
/// decision.
///
/// What is left on this side of the seam is everything that can be wrong: the
/// format, the order the header is checked in, which failures delete the file
/// and which deliberately leave it, and the fact that no path writes a file
/// without a key.
pub struct DiskCacheEnv {
    /// DPAPI-wraps the encoded file. `session_store::protect` in production.
    pub wrap: fn(&[u8]) -> Result<Vec<u8>, String>,
    /// The inverse. A failure here means the file cannot be ours.
    pub unwrap: fn(&[u8]) -> Result<Vec<u8>, String>,
    /// **The step that puts the OS's Hello dialog on screen**, and the only
    /// way a key is ever obtained.
    pub hello_key: fn() -> Result<Zeroizing<[u8; 32]>, String>,
}

impl DiskCacheEnv {
    /// The real one.
    pub fn production() -> Self {
        Self {
            wrap: |bytes| {
                crate::session_store::protect(bytes)
                    .map_err(|e| format!("DPAPI could not wrap the vault cache: {e}"))
            },
            unwrap: |bytes| {
                crate::session_store::unprotect(bytes)
                    .map_err(|e| format!("DPAPI could not unwrap the vault cache: {e}"))
            },
            hello_key: hello_derived_key,
        }
    }
}

/// The outcome of trying to load the file. Five distinct situations, five
/// variants -- deliberately not collapsed into `Result<Option<..>, ..>`,
/// because they call for different actions. In particular [`Self::Unavailable`]
/// must **leave the file alone** while [`Self::Rejected`] and [`Self::Corrupt`]
/// must delete it.
#[derive(Debug)]
pub enum DiskCacheLoad {
    Loaded {
        items: Vec<VaultItem>,
        folders: Vec<Folder>,
        /// When the file was written, for the vault window's age wording.
        written_at: SystemTime,
    },
    /// No file. Nothing happened, nothing was prompted.
    Absent,
    /// The header disqualified it. Deleted, unread, with no Hello prompt.
    Rejected(RejectReason),
    /// Hello could not be satisfied (cancelled, not enrolled, revoked).
    /// The file is **left in place**: a cancelled biometric is a user
    /// decision, not a reason to throw away their cache.
    Unavailable(String),
    /// The header was fine but the body would not open. Deleted -- a blob
    /// that can never be opened again is worse than no blob.
    Corrupt(String),
}

impl PartialEq for DiskCacheLoad {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DiskCacheLoad::Absent, DiskCacheLoad::Absent) => true,
            (DiskCacheLoad::Rejected(a), DiskCacheLoad::Rejected(b)) => a == b,
            _ => false,
        }
    }
}

/// Whether Windows Hello is set up on this machine at all. The cache needs
/// `available`, not `enrolled`: quick unlock is not a prerequisite, since
/// the cache creates its own credential when none exists.
pub fn hello_available() -> bool {
    KeyCredentialManager::IsSupportedAsync()
        .and_then(|op| op.get())
        .unwrap_or(false)
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The file, and the one key this session holds for it.
pub struct DiskCache {
    /// `Mutex` because an account switch **re-points** this at the target
    /// account's own directory before anything writes, exactly as
    /// `SessionStore`'s path is re-pointed -- see [`Self::repoint`]. A cache
    /// still addressing the account being left would write the incoming
    /// account's vault into the outgoing account's file.
    paths: Mutex<Paths>,
    key: Mutex<KeyState>,
    env: DiskCacheEnv,
}

struct Paths {
    file: PathBuf,
    tmp: PathBuf,
}

impl Paths {
    fn in_dir(dir: &Path) -> Self {
        Self {
            file: dir.join(FILE_NAME),
            tmp: dir.join(TMP_FILE_NAME),
        }
    }
}

/// The Hello-derived key for this session.
///
/// Acquired at most once per launch -- on the startup load, or when the
/// setting is switched on -- because the spec requires a rewrite after every
/// populate *and every mutation*, and deriving per write would pop a
/// biometric prompt on every item edit.
///
/// A cancelled or failed acquisition is **not retried** for the rest of the
/// session: retrying at the next write would pop a Hello prompt out of
/// nowhere at an arbitrary moment. `given_up` records that, so the difference
/// between "not tried yet" and "tried and refused" is in the state rather
/// than inferred from a `None`.
#[derive(Default)]
struct KeyState {
    key: Option<[u8; 32]>,
    given_up: bool,
}

impl Drop for KeyState {
    fn drop(&mut self) {
        if let Some(key) = self.key.as_mut() {
            key.zeroize();
        }
    }
}

impl DiskCache {
    /// A cache over `<dir>/vault-cache.bin`. `dir` is the **active account's**
    /// own directory, beside its `session.bin` and `hello.bin`, never the
    /// shared config root: two accounts sharing one file would each delete
    /// the other's on every switch.
    pub fn new(dir: &Path, env: DiskCacheEnv) -> Self {
        Self {
            paths: Mutex::new(Paths::in_dir(dir)),
            key: Mutex::new(KeyState::default()),
            env,
        }
    }

    /// Points this cache at a different account's directory.
    ///
    /// Called by the account switch **before** it authenticates, for
    /// `SessionStore::path`'s reason exactly: a write issued after the switch
    /// began but against the outgoing account's path is a mutation no
    /// end-state assertion catches. The session key is deliberately kept --
    /// the credential and the label are the same for every account, so
    /// re-deriving would be one more Hello prompt for no separation the file's
    /// location and the header's fingerprint do not already provide.
    pub fn repoint(&self, dir: &Path) {
        *self.lock_paths() = Paths::in_dir(dir);
    }

    /// The file this cache reads and writes.
    pub fn path(&self) -> PathBuf {
        self.lock_paths().file.clone()
    }

    /// Pops the Hello prompt if the key has not been acquired yet. Used by
    /// the Preferences toggle so that enabling the setting is itself the
    /// confirmation gesture.
    pub fn acquire_key(&self) -> Result<(), String> {
        let mut state = self.lock_key();
        self.ensure_key(&mut state).map(|_| ())
    }

    /// Whether a key is held for this session. Not a secret, and not the key:
    /// it is what "a doomed file never derived one" is asserted against.
    pub fn has_session_key(&self) -> bool {
        self.lock_key().key.is_some()
    }

    /// **Whether a copy is sitting on this machine that this session declined
    /// to open.**
    ///
    /// The one state [`DiskCacheLoad`] cannot express. A cancelled Hello
    /// prompt answers [`DiskCacheLoad::Unavailable`] and sets `given_up`, and
    /// the file is still there, untouched -- unlike
    /// [`DiskCacheLoad::Rejected`], which has already deleted it, and unlike
    /// [`DiskCacheLoad::Absent`], where there was never one. A caller that
    /// reads only the load outcome therefore cannot tell "there is no local
    /// copy" from "there is one and you dismissed the fingerprint prompt",
    /// and the offline screens would tell a user who fumbled a prompt that
    /// their vault copy does not exist.
    ///
    /// `given_up` **and** the file, both: `given_up` alone would answer `true`
    /// on a machine with no file at all, which is the same lie in the other
    /// direction. The key is never re-derived on its own (see `KeyState`'s
    /// "not retried for the rest of the session"), so this stays `true` until
    /// something asks for the key again on the user's behalf -- which is
    /// exactly what the button this feeds does.
    pub fn declined_copy_on_disk(&self) -> bool {
        self.lock_key().given_up && self.lock_paths().file.exists()
    }

    /// **Lets one more Hello prompt happen**, after a cancelled or failed one.
    ///
    /// `given_up` exists so a refusal is never retried *on its own* -- see
    /// `KeyState`: a retry at the next write would pop a biometric prompt out
    /// of nowhere while the user was editing an item. That reasoning is about
    /// prompts nobody asked for, and it is the only reasoning `given_up`
    /// carries. It says nothing about a user pressing a button labelled
    /// *Continue offline*, which is a request for exactly this prompt, at
    /// exactly this moment.
    ///
    /// So this is not a loosening of that rule: it is the one gesture the rule
    /// was never about. Without it, a fingerprint prompt dismissed by accident
    /// would put the local copy out of reach for the rest of the session while
    /// the screen went on offering it -- a button that does nothing, which is
    /// the treatment `prefs_ui::draw_not_yet` refuses.
    ///
    /// Only ever called from a user's own press.
    pub fn allow_one_more_key_attempt(&self) {
        self.lock_key().given_up = false;
    }

    fn ensure_key(&self, state: &mut KeyState) -> Result<[u8; 32], String> {
        if let Some(key) = state.key {
            return Ok(key);
        }
        if state.given_up {
            return Err("Windows Hello was not available earlier in this session".to_string());
        }
        match (self.env.hello_key)() {
            Ok(key) => {
                state.key = Some(*key);
                Ok(*key)
            }
            Err(e) => {
                state.given_up = true;
                Err(e)
            }
        }
    }

    /// Reads, validates, and (if everything checks out) decrypts the file.
    ///
    /// Order matters and is the point: DPAPI unwrap and header validation
    /// come first and derive no key, so an expired, foreign, or
    /// wrong-version file is deleted without the user being asked for a
    /// biometric on behalf of a file about to be thrown away.
    pub fn load(&self, fingerprint: &str) -> DiskCacheLoad {
        let (file, tmp) = {
            let paths = self.lock_paths();
            (paths.file.clone(), paths.tmp.clone())
        };
        // A .tmp left by a crash mid-write is meaningless and must not be
        // mistaken for anything; clear it whenever we touch the directory.
        let _ = std::fs::remove_file(&tmp);

        let wrapped = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DiskCacheLoad::Absent,
            Err(e) => {
                log::warn!("could not read the vault cache file: {e}");
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        let inner = match (self.env.unwrap)(&wrapped) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(e) => {
                log::warn!("{e}; deleting it");
                self.delete_quietly(&file, RejectReason::Malformed);
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        let (header, parsed) = match parse_header(&inner) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("the vault cache file is unusable: {e}");
                self.delete_quietly(&file, RejectReason::Malformed);
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        if let Err(reason) = check_header(&header, now_unix(), fingerprint) {
            log::info!("discarding the vault cache: it is {}", reason.as_str());
            self.delete_quietly(&file, reason);
            return DiskCacheLoad::Rejected(reason);
        }

        // Only now, with the file known to be ours and current, is it worth
        // asking the user for a biometric.
        let key = {
            let mut state = self.lock_key();
            match self.ensure_key(&mut state) {
                Ok(key) => key,
                Err(e) => {
                    log::info!("not using the vault cache this session: {e}");
                    return DiskCacheLoad::Unavailable(e);
                }
            }
        };

        match decode_body(&key, &parsed) {
            Ok(snapshot) => DiskCacheLoad::Loaded {
                items: snapshot.items,
                folders: snapshot.folders,
                written_at: UNIX_EPOCH + Duration::from_secs(header.written_at),
            },
            Err(e) => {
                log::warn!("the vault cache could not be decrypted ({e}); deleting it");
                let _ = std::fs::remove_file(&file);
                DiskCacheLoad::Corrupt(e)
            }
        }
    }

    /// Writes the snapshot. Atomic: a full write to `.tmp` followed by a
    /// rename over the target, so a crash mid-write cannot leave a truncated
    /// file whose corruption would cost a Hello prompt to discover.
    ///
    /// **Errors if no session key is held, and there is deliberately no
    /// fallback that writes without one.** This is the property the setting's
    /// description rests on: no path in this crate can produce a file that is
    /// not sealed under a Hello-derived key, because [`encode_file`] takes
    /// that key as a parameter and this is its only caller.
    pub fn write(
        &self,
        fingerprint: &str,
        items: &[VaultItem],
        folders: &[Folder],
    ) -> Result<(), String> {
        let key = {
            let state = self.lock_key();
            state
                .key
                .ok_or_else(|| "no Windows Hello key for the vault cache this session".to_string())?
        };

        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: now_unix(),
            account_fingerprint: fingerprint.to_string(),
            item_count: items.len(),
        };
        let snapshot = DiskSnapshot {
            items: items.to_vec(),
            folders: folders.to_vec(),
        };
        let inner = Zeroizing::new(encode_file(&key, &header, &snapshot)?);
        let wrapped = (self.env.wrap)(&inner)?;

        let paths = self.lock_paths();
        if let Some(parent) = paths.file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&paths.tmp, &wrapped)
            .map_err(|e| format!("could not write {}: {e}", paths.tmp.display()))?;
        std::fs::rename(&paths.tmp, &paths.file).map_err(|e| {
            let _ = std::fs::remove_file(&paths.tmp);
            format!("could not replace {}: {e}", paths.file.display())
        })
    }

    /// Removes the file. Succeeds when there is nothing to remove -- callers
    /// use this on log out, on re-auth, and on disabling the setting, where
    /// "already absent" is the desired end state.
    pub fn delete(&self) -> Result<(), String> {
        let paths = self.lock_paths();
        let _ = std::fs::remove_file(&paths.tmp);
        match std::fs::remove_file(&paths.file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not delete {}: {e}", paths.file.display())),
        }
    }

    fn delete_quietly(&self, file: &Path, reason: RejectReason) {
        if let Err(e) = std::fs::remove_file(file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "could not delete the vault cache that is {}: {e}",
                    reason.as_str()
                );
            }
        }
    }

    fn lock_key(&self) -> std::sync::MutexGuard<'_, KeyState> {
        self.key.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_paths(&self) -> std::sync::MutexGuard<'_, Paths> {
        self.paths.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Removes the encrypted copy in `dir`, for an account that is **logging
/// out**.
///
/// The exact twin of `hello::unenroll_for`, called in the same breath and for
/// the stronger version of its reason: a sealed master password for an
/// account the CLI no longer knows is a liability, and a decrypted dump of
/// that same account's vault is a larger one. Log out is not lock -- it means
/// the account is gone from this machine -- so this is one of the two places
/// the file is deleted rather than left.
///
/// **A free function, and the only one here that does not go through
/// [`DiskCache`].** The rule that keeps this feature honest is that exactly
/// one place *reasons about* the file -- what it holds, when it is written,
/// whether it may still be read -- and `VaultCache` is still that place. The
/// login window has no `VaultCache` and cannot be given one without threading
/// a vault through a window whose entire subject is not having a session yet;
/// what it does have is the account it is logging out, which is all removing
/// a file needs. Errors are logged rather than returned for the same reason
/// `unenroll_for` returns nothing: the account is going either way, and the
/// window that would show the error is about to be replaced by a sign-in
/// card.
pub fn forget_for(dir: &Path) {
    for name in [FILE_NAME, TMP_FILE_NAME] {
        if let Err(e) = std::fs::remove_file(dir.join(name)) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::error!(
                    "could not delete the encrypted vault copy at {}: {e}",
                    dir.join(name).display()
                );
            }
        }
    }
}

/// Runs the Hello-gated signature and derives this module's AES key from it.
/// This is the step that pops the OS verification dialog.
///
/// **Never `ReplaceExisting`.** `hello.rs` bans it for the same reason and
/// says so at length: the credential named [`CREDENTIAL_NAME`] is shared with
/// quick unlock and with every account, and replacing it rotates its private
/// key, which changes the signature, which destroys every enrolment derived
/// from it. Open first; only create -- with `FailIfExists` -- when there is
/// nothing to open.
fn hello_derived_key() -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);

    // `KeyCredentialManager` has no way to be told which window to parent its
    // prompt to, so the only lever is to be the foreground process when the
    // prompt is created. `hello.rs` does the same, immediately before each
    // call that can show one, and for the same reported symptom: "Windows PIN
    // screen launches in background".
    let _ = crate::foreground::raise_this_process();

    let opened = KeyCredentialManager::OpenAsync(&name)
        .and_then(|op| op.get())
        .map_err(|e| format!("Windows Hello is unavailable: {e}"))?;

    let credential = match opened.Status() {
        Ok(KeyCredentialStatus::Success) => opened
            .Credential()
            .map_err(|e| format!("Windows Hello returned no credential: {e}"))?,
        Ok(KeyCredentialStatus::NotFound) => {
            let created = KeyCredentialManager::RequestCreateAsync(
                &name,
                KeyCredentialCreationOption::FailIfExists,
            )
            .and_then(|op| op.get())
            .map_err(|e| format!("Windows Hello is unavailable: {e}"))?;
            match created.Status() {
                Ok(KeyCredentialStatus::Success) => created
                    .Credential()
                    .map_err(|e| format!("Windows Hello returned no credential: {e}"))?,
                Ok(KeyCredentialStatus::UserCanceled) => {
                    return Err("Windows Hello was cancelled.".to_string())
                }
                Ok(other) => return Err(format!("Windows Hello failed ({other:?})")),
                Err(e) => return Err(format!("Windows Hello failed: {e}")),
            }
        }
        Ok(KeyCredentialStatus::UserCanceled) => {
            return Err("Windows Hello was cancelled.".to_string())
        }
        Ok(other) => return Err(format!("Windows Hello failed ({other:?})")),
        Err(e) => return Err(format!("Windows Hello failed: {e}")),
    };

    // Again, immediately before the call that shows the prompt: the
    // create/open above may itself have shown one, in which case the broker
    // took the foreground and the raise at the top is already stale.
    let _ = crate::foreground::raise_this_process();

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

/// `SHA-256(label ‖ signature)` → AES-256 key. Pure, and split out for
/// `hello::derive_key`'s reason: it is the only part of the key path that can
/// be tested without Hello hardware, and the label is the whole of what keeps
/// this blob and quick unlock's from being cross-decryptable.
fn derive_key(signature: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(signature);
    Zeroizing::new(hasher.finalize().into())
}

/// `pub(crate)` so `vault_cache`'s own disk tests can build a [`DiskCache`]
/// through the same substituted seams rather than growing a second set --
/// `cache_with_key` and `temp_dir_for` below. Nothing here reaches a release
/// build.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::vault_bridge::{Folder, VaultItem};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// **The point of the whole format**: one item opens without any other
    /// item being touched, and the facts section opens without any item
    /// being touched at all.
    #[test]
    fn one_item_opens_without_opening_any_other() {
        let k = key(7);
        let header = b"a plausible header".to_vec();
        let items: Vec<(String, Vec<u8>)> = (0..5)
            .map(|n| (format!("item-{n}"), format!("secret number {n}").into_bytes()))
            .collect();
        let (mut body, index) = seal_body(&k, &header, b"the facts", &items).expect("sealed");
        assert_eq!(index.len(), 5);

        // **Independence, proved by breaking a neighbour.** Opening one item
        // and finding it correct does not show the others were untouched --
        // a whole-body read would pass that too. Corrupting item 0 does: if
        // item 3 still opens, its bytes were the only ones authenticated, and
        // if the facts still open they never depended on any item.
        let victim = &index[0];
        body[victim.at as usize + NONCE_LEN] ^= 0xff;
        assert!(
            open_item_at(&k, &header, &body, victim).is_err(),
            "control: the corruption did not take, so nothing below is being proved"
        );

        let third = &index[3];
        assert_eq!(third.id, "item-3");
        let opened = open_item_at(&k, &header, &body, third).expect("the third item");
        assert_eq!(
            &*opened, b"secret number 3",
            "a broken neighbour changed what this item opens as"
        );
    }

    /// A slot that points outside the body is refused rather than panicking.
    /// The index is PLAINTEXT, so its numbers are the one part of this file
    /// an attacker can edit freely -- and a slice with an out-of-range range
    /// is a panic, which in the daemon is the tray disappearing.
    #[test]
    fn a_slot_pointing_past_the_body_is_refused_rather_than_panicking() {
        let k = key(7);
        let header = b"h".to_vec();
        let (body, index) =
            seal_body(&k, &header, b"facts", &[("a".to_string(), b"secret".to_vec())])
                .expect("sealed");
        for bad in [
            ItemSlot { id: "a".to_string(), at: u32::MAX, len: 16 },
            ItemSlot { id: "a".to_string(), at: index[0].at, len: u32::MAX },
            ItemSlot { id: "a".to_string(), at: body.len() as u32, len: 1 },
        ] {
            assert!(
                open_item_at(&k, &header, &body, &bad).is_err(),
                "{bad:?} was read rather than refused"
            );
        }
    }

    /// A facts length that runs past the body is refused for the same
    /// reason: it is a header field, and the header is plaintext.
    #[test]
    fn a_facts_length_past_the_body_is_refused_rather_than_panicking() {
        let k = key(7);
        let header = b"h".to_vec();
        let (body, _) = seal_body(&k, &header, b"facts", &[]).expect("sealed");
        assert!(open_facts(&k, &header, &body, u32::MAX).is_err());
        assert!(open_facts(&k, &header, &body, body.len() as u32 + 1).is_err());
    }

    /// **A secret is bound to its id, not merely to the file.**
    ///
    /// Version 2 seals each item separately so one can be opened without the
    /// other 1,665. That raises a question a single blob never had: what
    /// stops an entry being moved to another item's slot?
    ///
    /// GCM proves *this key sealed this message*. It does not prove *this
    /// message belongs here*. So without the id in the additional
    /// authenticated data, two entries in one file are interchangeable --
    /// somebody who can write the file swaps the ciphertext for a bank with
    /// one for a site they control, and the daemon types the wrong password
    /// into the right box and never knows. The AAD is what closes that, and
    /// this is the test that fails if it is ever dropped as redundant.
    #[test]
    fn a_secret_moved_to_another_items_slot_will_not_open() {
        let k = key(7);
        let header = b"a plausible header".to_vec();
        let item = b"an item's plaintext".to_vec();
        let sealed = seal(&k, &item, Some(&item_aad(&header, "item-a"))).expect("sealed");
        assert!(
            unseal(&k, &sealed, Some(&item_aad(&header, "item-b"))).is_err(),
            "a ciphertext sealed for item-a opened as item-b, so entries in this file are \
             interchangeable and a swapped one would be typed as the item it replaced"
        );
        assert!(
            unseal(&k, &sealed, Some(&item_aad(&header, "item-a"))).is_ok(),
            "control: it does not open as itself either, so this proves nothing"
        );
    }

    /// The same binding from the other side: an entry from a DIFFERENT file
    /// does not open here even under the same id. The header carries
    /// `written_at` and the account fingerprint, so it is what makes one
    /// file's entries useless in another -- an old cache's password cannot be
    /// grafted into a current one.
    #[test]
    fn a_secret_from_another_file_will_not_open_under_the_same_id() {
        let k = key(7);
        let item = b"an item's plaintext".to_vec();
        let sealed =
            seal(&k, &item, Some(&item_aad(b"header of file one", "item-a"))).expect("sealed");
        assert!(
            unseal(&k, &sealed, Some(&item_aad(b"header of file two", "item-a"))).is_err(),
            "an entry from one cache file opened inside another"
        );
    }

    /// The id is length-prefixed rather than concatenated, so `("ab", "c")`
    /// and `("a", "bc")` cannot produce the same binding. A separator would
    /// do for GUIDs, which contain no delimiter; a length prefix does not
    /// have to argue about what an id may contain.
    #[test]
    fn no_two_headers_and_ids_can_produce_the_same_binding() {
        assert_ne!(item_aad(b"h", "ab"), item_aad(b"ha", "b"));
        assert_ne!(item_aad(b"ha", ""), item_aad(b"h", "a"));
    }

    fn key(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    /// The Hello step, substituted. Two of them, so "a file written under a
    /// different key" is expressible without any hardware.
    fn key_seven() -> Result<Zeroizing<[u8; 32]>, String> {
        Ok(Zeroizing::new(key(7)))
    }

    fn hello_cancelled() -> Result<Zeroizing<[u8; 32]>, String> {
        Err("Windows Hello was cancelled.".to_string())
    }

    /// DPAPI, substituted. Identity rather than a fake cipher: what this
    /// module has to get right about the envelope is that it is applied and
    /// removed in the right order, not what it contains.
    fn no_wrap(bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }

    /// A test env whose Hello step is `hello`, with DPAPI stubbed out.
    pub(crate) fn env(hello: fn() -> Result<Zeroizing<[u8; 32]>, String>) -> DiskCacheEnv {
        DiskCacheEnv {
            wrap: no_wrap,
            unwrap: no_wrap,
            hello_key: hello,
        }
    }

    /// A `DiskCache` over `dir` that already holds a fixed key, for the
    /// callers -- here and in `vault_cache` -- that are testing what happens
    /// *after* Hello has been satisfied.
    pub(crate) fn cache_with_key(dir: &Path) -> DiskCache {
        let cache = DiskCache::new(dir, env(key_seven));
        cache.acquire_key().expect("the substituted Hello step cannot fail");
        cache
    }

    /// A cache over `dir` whose Hello step always refuses -- the shape of a
    /// session where the user cancelled the prompt. Nothing it does derives a
    /// key, so no test using it can reach a real biometric.
    pub(crate) fn cache_that_declines_hello(dir: &Path) -> DiskCache {
        DiskCache::new(dir, env(hello_cancelled))
    }

    fn item(id: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "Alpha",
            "type": 1,
            "fields": [],
        }))
        .unwrap()
    }

    fn snapshot() -> DiskSnapshot {
        DiskSnapshot {
            items: vec![item("1")],
            folders: vec![Folder {
                id: "f1".to_string(),
                name: "Work".to_string(),
                other: serde_json::Map::new(),
            }],
        }
    }

    fn header_for(snapshot: &DiskSnapshot, written_at: u64) -> CacheHeader {
        CacheHeader {
            format_version: FORMAT_VERSION,
            written_at,
            account_fingerprint: account_fingerprint(Some("a@example.com"), Some("https://vault")),
            item_count: snapshot.items.len(),
        }
    }

    // -- the format, with no I/O at all ------------------------------------

    #[test]
    fn a_file_round_trips_under_the_same_key() {
        let snap = snapshot();
        let header = header_for(&snap, 1_000);
        let bytes = encode_file(&key(7), &header, &snap).unwrap();

        let (read_header, parsed) = parse_header(&bytes).unwrap();
        assert_eq!(read_header, header);
        let opened = decode_body(&key(7), &parsed).unwrap();
        assert_eq!(opened.items.len(), 1);
        assert_eq!(opened.folders[0].name, "Work");
    }

    #[test]
    fn the_header_is_readable_without_the_key() {
        // The whole reason the header sits outside the sealed body: the app
        // must be able to reject an expired or foreign file *without*
        // popping a Hello prompt for a file it is about to delete.
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let (header, _) = parse_header(&bytes).unwrap();
        assert_eq!(header.item_count, 1);
    }

    #[test]
    fn the_wrong_key_cannot_open_the_body() {
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let (_, parsed) = parse_header(&bytes).unwrap();
        assert!(decode_body(&key(8), &parsed).is_err());
    }

    #[test]
    fn tampering_with_the_body_fails_authentication() {
        let snap = snapshot();
        let mut bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let (_, parsed) = parse_header(&bytes).unwrap();
        assert!(decode_body(&key(7), &parsed).is_err());
    }

    #[test]
    fn tampering_with_the_header_fails_authentication() {
        // This is the AAD binding being live: `written_at` cannot be edited
        // to defeat expiry, because the header authenticates the body.
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let (header, parsed) = parse_header(&bytes).unwrap();

        let mut forged = header.clone();
        forged.written_at = 9_999_999;
        let forged_json = serde_json::to_vec(&forged).unwrap();
        let mut tampered = Vec::new();
        tampered.extend_from_slice(MAGIC);
        tampered.extend_from_slice(&(forged_json.len() as u32).to_le_bytes());
        tampered.extend_from_slice(&forged_json);
        // Splice the original sealed key and body back in unchanged.
        tampered.extend_from_slice(&(parsed.sealed_key.len() as u32).to_le_bytes());
        tampered.extend_from_slice(&parsed.sealed_key);
        tampered.extend_from_slice(&parsed.body);

        let (reread, reparsed) = parse_header(&tampered).unwrap();
        assert_eq!(reread.written_at, 9_999_999, "the forgery did not take");
        assert!(
            decode_body(&key(7), &reparsed).is_err(),
            "an edited header still opened the body: the AAD binding is not live"
        );
    }

    #[test]
    fn a_truncated_file_is_rejected_without_panicking() {
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        for cut in [0usize, 2, 4, 8, 20, bytes.len() / 2, bytes.len() - 1] {
            let rejected = match parse_header(&bytes[..cut]) {
                Err(_) => true,
                Ok((_, parsed)) => decode_body(&key(7), &parsed).is_err(),
            };
            assert!(rejected, "a file truncated to {cut} bytes was accepted");
        }
    }

    #[test]
    fn a_foreign_magic_is_rejected() {
        assert!(parse_header(b"NOPEnotacachefileatall").is_err());
    }

    #[test]
    fn a_header_length_larger_than_the_file_is_rejected_rather_than_panicking() {
        // The length prefixes come from a file an attacker can write. A
        // `u32::MAX` header length must be a rejection, not an allocation or
        // a slice panic.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"{}");
        assert!(parse_header(&bytes).is_err());
    }

    #[test]
    fn unknown_versions_are_rejected() {
        let header = CacheHeader {
            format_version: FORMAT_VERSION + 1,
            written_at: 1_000,
            account_fingerprint: "abc".to_string(),
            item_count: 0,
        };
        assert_eq!(
            check_header(&header, 1_000, "abc"),
            Err(RejectReason::UnknownVersion)
        );
    }

    #[test]
    fn expiry_boundaries() {
        let fp = "abc".to_string();
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: 1_000_000,
            account_fingerprint: fp.clone(),
            item_count: 0,
        };
        // 6 days 23 h old: still usable.
        let almost = 1_000_000 + EXPIRY_SECS - 3_600;
        assert_eq!(check_header(&header, almost, &fp), Ok(()));
        // Exactly at the deadline: still usable (the spec expires files
        // *older* than 7 days).
        assert_eq!(check_header(&header, 1_000_000 + EXPIRY_SECS, &fp), Ok(()));
        // A minute past: gone.
        assert_eq!(
            check_header(&header, 1_000_000 + EXPIRY_SECS + 60, &fp),
            Err(RejectReason::Expired)
        );
    }

    #[test]
    fn a_foreign_account_fingerprint_is_rejected() {
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: 1_000,
            account_fingerprint: account_fingerprint(Some("a@example.com"), Some("https://vault")),
            item_count: 0,
        };
        let other = account_fingerprint(Some("b@example.com"), Some("https://vault"));
        assert_eq!(
            check_header(&header, 1_000, &other),
            Err(RejectReason::ForeignAccount)
        );
    }

    #[test]
    fn a_future_timestamp_beyond_tolerance_is_rejected() {
        // An unbounded future timestamp is an expiry that never fires.
        let fp = "abc".to_string();
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: 1_000_000,
            account_fingerprint: fp.clone(),
            item_count: 0,
        };
        // Small skew is tolerated.
        assert_eq!(check_header(&header, 1_000_000 - 60, &fp), Ok(()));
        // A wildly future stamp is not.
        assert_eq!(
            check_header(&header, 1_000, &fp),
            Err(RejectReason::FutureTimestamp)
        );
    }

    #[test]
    fn the_fingerprint_hides_the_account_and_separates_its_parts() {
        let fp = account_fingerprint(Some("a@example.com"), Some("https://vault.example"));
        assert!(!fp.contains("example"), "the fingerprint leaks the account");
        assert_eq!(fp.len(), 64, "expected hex SHA-256");
        // Without a separator, ("ab", "c") and ("a", "bc") would collide.
        assert_ne!(
            account_fingerprint(Some("ab"), Some("c")),
            account_fingerprint(Some("a"), Some("bc"))
        );
        // Absent fields are stable, not random.
        assert_eq!(
            account_fingerprint(None, None),
            account_fingerprint(None, None)
        );
    }

    #[test]
    fn this_modules_key_derivation_is_not_quick_unlocks() {
        // The two sealed blobs must not be cross-decryptable. `hello.rs`'s
        // label is private to that module, so this asserts the property that
        // makes it hold: the label this module hashes is its own, and the
        // string it holds is not the one quick unlock uses.
        assert_eq!(KDF_LABEL, b"deskwarden vault cache aes key v1");
        assert_ne!(KDF_LABEL, b"deskwarden hello quick-unlock aes key v1");
        assert_ne!(CHALLENGE, b"deskwarden hello quick-unlock challenge v1");
        // And the derivation really depends on the label rather than on the
        // signature alone.
        let mut plain = Sha256::new();
        plain.update(b"signature");
        let unlabelled: [u8; 32] = plain.finalize().into();
        assert_ne!(*derive_key(b"signature"), unlabelled);
    }

    #[test]
    fn unknown_item_and_folder_fields_survive_a_disk_round_trip() {
        // This codebase has shipped a dropped-unknown-field bug four times,
        // in four different structs. The disk path must not become the fifth,
        // and BOTH halves of `DiskSnapshot` are exercised deliberately: a
        // fidelity test covering one of the two types it serializes cannot
        // catch a wrong premise about the other.
        let raw = r#"{
            "id": "1",
            "name": "Alpha",
            "type": 1,
            "fields": [],
            "favorite": false,
            "login": {
                "username": "u",
                "password": "p",
                "uris": [{"uri": "https://x", "match": 3}],
                "totp": "seed",
                "somethingNew": {"deep": true}
            },
            "passwordHistory": [{"password": "old", "lastUsedDate": "2020-01-01"}],
            "notes": "a note",
            "reprompt": 1
        }"#;
        let raw_folder = r#"{
            "id": "f1",
            "name": "Work",
            "organizationId": null,
            "revisionDate": "2026-01-02T03:04:05.000Z"
        }"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let folder: Folder = serde_json::from_str(raw_folder).unwrap();
        let snap = DiskSnapshot {
            items: vec![item],
            folders: vec![folder],
        };
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let (_, parsed) = parse_header(&bytes).unwrap();
        let opened = decode_body(&key(7), &parsed).unwrap();

        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after: serde_json::Value = serde_json::to_value(&opened.items[0]).unwrap();
        assert_eq!(
            before, after,
            "an item field was dropped by the disk round trip"
        );

        let before_folder: serde_json::Value = serde_json::from_str(raw_folder).unwrap();
        let after_folder: serde_json::Value = serde_json::to_value(&opened.folders[0]).unwrap();
        assert_eq!(
            before_folder, after_folder,
            "a folder key was dropped by the disk round trip"
        );
    }

    #[test]
    fn each_write_uses_fresh_nonces_and_a_fresh_content_key() {
        // Two encodes of identical input must not produce identical bytes:
        // a fixed nonce under a reused key is a catastrophic GCM misuse.
        let snap = snapshot();
        let header = header_for(&snap, 1_000);
        let a = encode_file(&key(7), &header, &snap).unwrap();
        let b = encode_file(&key(7), &header, &snap).unwrap();
        assert_ne!(a, b);
    }

    // -- the file on disk --------------------------------------------------

    /// A unique scratch directory. `temp_dir()` + nanos, the pattern the rest
    /// of the suite uses, and **never** the real config directory: nothing
    /// here may go near the user's own vault cache.
    pub(crate) fn temp_dir_for(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-diskcache-test-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a file directly, bypassing the key state, so the load-side
    /// rejection paths can be driven with a header of our choosing.
    fn write_file_with_key(dir: &Path, k: &[u8; 32], header: &CacheHeader, snap: &DiskSnapshot) {
        let inner = encode_file(k, header, snap).unwrap();
        std::fs::write(dir.join(FILE_NAME), no_wrap(&inner).unwrap()).unwrap();
    }

    fn header_at(written_at: u64, fingerprint: &str, version: u32) -> CacheHeader {
        CacheHeader {
            format_version: version,
            written_at,
            account_fingerprint: fingerprint.to_string(),
            item_count: 1,
        }
    }

    #[test]
    fn an_absent_file_reports_absent_and_prompts_for_nothing() {
        let dir = temp_dir_for("absent");
        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(cache.load("fp"), DiskCacheLoad::Absent);
        assert!(!cache.has_session_key());
    }

    #[test]
    fn an_expired_file_is_deleted_unread_and_no_key_is_ever_derived() {
        // The "no Hello prompt for a doomed file" property, asserted
        // behaviourally: the file is gone, and the session key was never
        // populated -- which is what a Hello prompt would have set.
        let dir = temp_dir_for("expired");
        let snap = snapshot();
        let long_ago = now_unix().saturating_sub(EXPIRY_SECS + 3_600);
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(long_ago, "fp", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(
            cache.load("fp"),
            DiskCacheLoad::Rejected(RejectReason::Expired)
        );
        assert!(
            !dir.join(FILE_NAME).exists(),
            "an expired file was left on disk"
        );
        assert!(
            !cache.has_session_key(),
            "a doomed file triggered a key derivation"
        );
    }

    #[test]
    fn a_foreign_account_file_is_deleted_unread() {
        let dir = temp_dir_for("foreign");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "someone-else", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(
            cache.load("fp"),
            DiskCacheLoad::Rejected(RejectReason::ForeignAccount)
        );
        assert!(!dir.join(FILE_NAME).exists());
        assert!(!cache.has_session_key());
    }

    #[test]
    fn an_unknown_version_is_deleted_unread() {
        let dir = temp_dir_for("version");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "fp", FORMAT_VERSION + 1),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(
            cache.load("fp"),
            DiskCacheLoad::Rejected(RejectReason::UnknownVersion)
        );
        assert!(!dir.join(FILE_NAME).exists());
        assert!(!cache.has_session_key());
    }

    #[test]
    fn a_garbage_file_is_deleted_rather_than_kept_forever() {
        // Same self-healing posture as hello's `open_blob`: a blob that can
        // never be opened again is worse than no blob.
        let dir = temp_dir_for("garbage");
        std::fs::write(dir.join(FILE_NAME), b"this is not a cache file").unwrap();

        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(
            cache.load("fp"),
            DiskCacheLoad::Rejected(RejectReason::Malformed)
        );
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn a_file_whose_envelope_will_not_open_is_deleted() {
        // The DPAPI half of the same posture, driven through the seam: a file
        // this Windows user cannot unwrap can never be unwrapped again.
        fn refuse(_: &[u8]) -> Result<Vec<u8>, String> {
            Err("DPAPI could not unwrap the vault cache: 0x8009000b".to_string())
        }
        let dir = temp_dir_for("dpapi");
        std::fs::write(dir.join(FILE_NAME), b"somebody elses envelope").unwrap();

        let cache = DiskCache::new(
            &dir,
            DiskCacheEnv {
                wrap: no_wrap,
                unwrap: refuse,
                hello_key: key_seven,
            },
        );
        assert_eq!(
            cache.load("fp"),
            DiskCacheLoad::Rejected(RejectReason::Malformed)
        );
        assert!(!dir.join(FILE_NAME).exists());
        assert!(
            !cache.has_session_key(),
            "an unwrappable file cost a Hello prompt"
        );
    }

    #[test]
    fn a_leftover_tmp_file_does_not_affect_a_load_and_is_cleaned_up() {
        let dir = temp_dir_for("tmp");
        std::fs::write(dir.join(TMP_FILE_NAME), b"half a write").unwrap();

        let cache = DiskCache::new(&dir, env(key_seven));
        assert_eq!(cache.load("fp"), DiskCacheLoad::Absent);
        assert!(
            !dir.join(TMP_FILE_NAME).exists(),
            "a crash-leftover .tmp file was not cleaned up"
        );
    }

    #[test]
    fn delete_is_idempotent_and_succeeds_when_there_is_nothing_to_delete() {
        let dir = temp_dir_for("delete");
        let cache = DiskCache::new(&dir, env(key_seven));
        assert!(cache.delete().is_ok());
        assert!(cache.delete().is_ok());
    }

    #[test]
    fn writing_without_a_session_key_is_a_no_op_not_a_plaintext_file() {
        // The single most important negative test in the feature: if the key
        // is unavailable (Hello cancelled), nothing must be written at all.
        // There is no unencrypted fallback path, by construction.
        let dir = temp_dir_for("nokey");
        let cache = DiskCache::new(&dir, env(hello_cancelled));
        assert!(!cache.has_session_key());
        let snap = snapshot();
        assert!(cache.write("fp", &snap.items, &snap.folders).is_err());
        assert!(
            !dir.join(FILE_NAME).exists(),
            "a file was written with no Hello-sealed key"
        );
        assert!(!dir.join(TMP_FILE_NAME).exists());
    }

    #[test]
    fn a_cancelled_hello_leaves_the_file_alone_and_is_not_retried() {
        // Two rules in one place, both from the spec's error handling: a
        // cancelled biometric is a user decision, so the cache stays; and it
        // must not re-prompt later in the session, so the second attempt
        // fails without reaching the seam again.
        let dir = temp_dir_for("cancelled");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "fp", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(hello_cancelled));
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Unavailable(_)));
        assert!(
            dir.join(FILE_NAME).exists(),
            "a cancelled Hello prompt threw the users cache away"
        );
        let second = cache.acquire_key().unwrap_err();
        assert!(
            second.contains("earlier in this session"),
            "a refused acquisition was retried: {second}"
        );
    }

    /// **The state `DiskCacheLoad` cannot express**, which is what the offline
    /// screens read: the prompt was declined and the file is still there.
    ///
    /// Driven through the same seam as the test above, because the two are
    /// one situation: `Unavailable` is what the *load* says, and this is what
    /// is left on the disk afterwards.
    #[test]
    fn a_declined_prompt_over_a_real_file_is_visible_as_a_copy_that_is_still_there() {
        let dir = temp_dir_for("declined");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "fp", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(hello_cancelled));
        assert!(
            !cache.declined_copy_on_disk(),
            "a copy was reported as declined before anything had asked for a key"
        );
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Unavailable(_)));
        assert!(
            cache.declined_copy_on_disk(),
            "the user's copy is sitting on the disk after a cancelled prompt and nothing says \
             so, which is how a fumbled fingerprint becomes `there is no local copy`"
        );
    }

    /// **Both halves are required**, and this is the other one: a session that
    /// gave up on a machine with no file has no copy to offer.
    #[test]
    fn a_declined_prompt_with_no_file_offers_nothing() {
        let dir = temp_dir_for("declined-nofile");
        let cache = DiskCache::new(&dir, env(hello_cancelled));
        assert!(cache.acquire_key().is_err());
        assert!(
            !cache.declined_copy_on_disk(),
            "a refused prompt on a machine with no cache file was reported as a local copy"
        );
    }

    /// **One more attempt, and only when something asks for it.**
    ///
    /// `given_up` is not retried on its own -- the test above pins that. This
    /// pins the other half: the user pressing *Continue offline* is allowed to
    /// spend one more prompt, and a copy that then opens really opens.
    #[test]
    fn a_refusal_can_be_taken_back_by_an_explicit_request() {
        let dir = temp_dir_for("retry-key");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "fp", FORMAT_VERSION),
            &snap,
        );

        // Refuses once, then answers -- the shape of a user who cancelled the
        // prompt and then asked for the copy. A `static` counter rather than a
        // capturing closure, because the seam is an `fn` pointer.
        static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
        fn once_refusing() -> Result<Zeroizing<[u8; 32]>, String> {
            if ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err("Windows Hello was cancelled.".to_string());
            }
            key_seven()
        }

        ATTEMPTS.store(0, Ordering::SeqCst);
        let cache = DiskCache::new(&dir, env(once_refusing));
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Unavailable(_)));
        assert!(cache.declined_copy_on_disk());

        // Without this the second load never reaches the seam at all.
        cache.allow_one_more_key_attempt();
        match cache.load("fp") {
            DiskCacheLoad::Loaded { items, .. } => assert_eq!(items.len(), 1),
            other => panic!("the copy did not open after the user asked again: {other:?}"),
        }
        assert_eq!(
            ATTEMPTS.load(Ordering::SeqCst),
            2,
            "the second attempt did not reach Windows Hello, so the button would have done \
             nothing"
        );
        assert!(
            !cache.declined_copy_on_disk(),
            "the copy still reports itself as declined after it opened"
        );
    }

    #[test]
    fn a_write_then_load_round_trips_through_the_file() {
        let dir = temp_dir_for("roundtrip");
        let cache = cache_with_key(&dir);

        let snap = snapshot();
        cache.write("fp", &snap.items, &snap.folders).unwrap();
        assert!(dir.join(FILE_NAME).exists());
        assert!(
            !dir.join(TMP_FILE_NAME).exists(),
            "the temp file survived a successful write"
        );

        match cache.load("fp") {
            DiskCacheLoad::Loaded { items, folders, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(folders[0].name, "Work");
            }
            other => panic!("expected a loaded snapshot, got {other:?}"),
        }
    }

    #[test]
    fn what_lands_on_disk_is_not_the_vault_in_the_clear() {
        // The whole claim, asserted on the bytes: an item name and a password
        // that are in the snapshot are nowhere in the file.
        let dir = temp_dir_for("opaque");
        let cache = cache_with_key(&dir);
        let secret: VaultItem = serde_json::from_value(serde_json::json!({
            "id": "1",
            "name": "Sourdough Bank",
            "type": 1,
            "fields": [],
            "login": {"username": "u", "password": "correct-horse-battery"},
        }))
        .unwrap();
        cache.write("fp", &[secret], &[]).unwrap();

        let bytes = std::fs::read(dir.join(FILE_NAME)).unwrap();
        for needle in [
            b"Sourdough Bank".as_slice(),
            b"correct-horse-battery".as_slice(),
        ] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "{} appears in the file in the clear",
                String::from_utf8_lossy(needle)
            );
        }
    }

    #[test]
    fn a_file_written_under_a_different_key_is_treated_as_corrupt_and_deleted() {
        let dir = temp_dir_for("wrongkey");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(9),
            &header_at(now_unix(), "fp", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(key_seven));
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Corrupt(_)));
        assert!(
            !dir.join(FILE_NAME).exists(),
            "an unopenable file was kept, so it would cost a Hello prompt on every launch forever"
        );
    }

    #[test]
    fn a_repoint_writes_to_the_new_account_and_leaves_the_old_file_untouched() {
        // The account switch's rule: after re-pointing, nothing may reach the
        // directory of the account being left.
        let first = temp_dir_for("repoint-a");
        let second = temp_dir_for("repoint-b");
        let snap = snapshot();

        let cache = cache_with_key(&first);
        cache.write("fp-a", &snap.items, &snap.folders).unwrap();
        let before = std::fs::read(first.join(FILE_NAME)).unwrap();

        cache.repoint(&second);
        cache.write("fp-b", &snap.items, &snap.folders).unwrap();
        assert!(second.join(FILE_NAME).exists());
        assert_eq!(
            std::fs::read(first.join(FILE_NAME)).unwrap(),
            before,
            "a write after the switch reached the account being left"
        );

        // And a delete follows the re-point too.
        cache.delete().unwrap();
        assert!(!second.join(FILE_NAME).exists());
        assert!(first.join(FILE_NAME).exists());
    }

    #[test]
    fn forgetting_an_account_removes_its_copy_and_touches_no_other() {
        // The log-out path. It takes a directory rather than a `DiskCache`
        // because the login window has an account and no vault -- and the
        // directory it is given had better be the only one it reaches.
        let doomed = temp_dir_for("forget-doomed");
        let survivor = temp_dir_for("forget-survivor");
        let snap = snapshot();
        let cache = cache_with_key(&doomed);
        cache.write("fp", &snap.items, &snap.folders).unwrap();
        let survivors_cache = cache_with_key(&survivor);
        survivors_cache.write("fp2", &snap.items, &snap.folders).unwrap();
        std::fs::write(doomed.join(TMP_FILE_NAME), b"a crash left this").unwrap();

        forget_for(&doomed);
        assert!(!doomed.join(FILE_NAME).exists());
        assert!(
            !doomed.join(TMP_FILE_NAME).exists(),
            "a half-written copy of the logged-out account's vault was left behind"
        );
        assert!(
            survivor.join(FILE_NAME).exists(),
            "logging one account out deleted another account's copy"
        );

        // Idempotent: an account with no copy is not an error.
        forget_for(&doomed);
        assert!(!doomed.join(FILE_NAME).exists());
    }

    #[test]
    fn the_key_is_derived_once_per_session_however_many_writes_there_are() {
        // Deriving per write would pop a biometric prompt on every item edit,
        // which is why the key is cached. Asserted through a seam that counts
        // its own calls in a thread-local rather than a global.
        thread_local! {
            static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        fn counted() -> Result<Zeroizing<[u8; 32]>, String> {
            CALLS.with(|c| c.set(c.get() + 1));
            Ok(Zeroizing::new([7u8; 32]))
        }

        let dir = temp_dir_for("once");
        let cache = DiskCache::new(&dir, env(counted));
        let snap = snapshot();
        cache.acquire_key().unwrap();
        for _ in 0..5 {
            cache.write("fp", &snap.items, &snap.folders).unwrap();
        }
        cache.load("fp");
        assert_eq!(CALLS.with(|c| c.get()), 1);
    }
}

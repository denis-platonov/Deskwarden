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
//! 2. That content key is itself sealed (AES-256-GCM) under this account's
//!    **sealing key** -- 32 random bytes minted once and kept DPAPI-wrapped
//!    in `vault-cache-key.bin` beside the cache, exactly the shape
//!    [`crate::user_key_store`] uses for the master key.
//! 3. The whole thing is DPAPI-wrapped, exactly as `hello.bin` is.
//!
//! ## Why the sealing key is not a Windows Hello signature any more
//!
//! It was, until 2026-08-30, and the argument for that was TPM binding: a
//! stolen or imaged disk plus the Windows account password would yield the
//! header and two ciphertexts and nothing else. The argument was true and it
//! was still the wrong trade, for two reasons that are not about
//! cryptography.
//!
//! **It gated a derivative more strictly than the original.** On a
//! direct-REST account the master key -- the thing that opens the *whole*
//! vault, that never expires and cannot be revoked -- sits in
//! [`crate::user_key_store`], DPAPI-wrapped and nothing more; Preferences
//! says so to the user in as many words. An attacker who can run programs as
//! this user takes that file and never looks at this one. So the Hello gate
//! defended nothing that was not already reachable without it, and its whole
//! practical effect was a prompt.
//!
//! **And that prompt was on the startup path.** Deriving the key asked the
//! credential to sign a challenge, which is what puts the OS dialog on
//! screen. On a machine where the dialog did not appear, startup waited for
//! it forever: daemon alive, no window, no further log line. Turning the
//! setting on made the app stop starting. The rule that replaces it is
//! flat -- **nothing on a startup path may block on UI** -- and the way this
//! module keeps it is that its key comes from a file, not from a person.
//!
//! Quick unlock (`hello::enroll_for`) still prompts, and should: there the
//! prompt is the feature the user asked for, not a toll on launching.
//!
//! The header is plaintext *inside* the DPAPI envelope on purpose. DPAPI
//! unwrapping is silent and non-interactive, so the app can read the header
//! and decide the file is expired or belongs to a different account before
//! touching a key at all -- which was what kept a doomed file from costing a
//! prompt, and now keeps it from costing a mint.
//!
//! ## What is testable without touching the OS, and how
//!
//! Two things here reach the OS and nothing in this crate's test suite may:
//! DPAPI, and the sealing key's file. Both are behind [`DiskCacheEnv`]'s `fn`
//! pointers -- `single_instance::TakeoverEnv`'s idiom -- so every decision on
//! this side of the seam (the format, the header checks, which failures
//! delete the file and which leave it) is driven directly, and no test mints
//! a key or wraps a byte.

use crate::vault_bridge::{Folder, VaultItem};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Security::Credentials::KeyCredentialManager;
use zeroize::{Zeroize, Zeroizing};

/// File magic, so a foreign or truncated file is rejected before anything
/// tries to interpret its length prefixes.
const MAGIC: &[u8; 4] = b"DWVC";

/// Bumped only for an incompatible layout change. An unknown version is
/// rejected and the file deleted -- there is nothing to migrate, the vault
/// is regenerable from the backend in seconds.
/// **2 since the body was split into a facts section and per-item secrets.**
///
/// A version 1 file is refused by [`check_header`] as
/// [`RejectReason::UnknownVersion`] -- unread, without touching a key,
/// because the header is plaintext inside the DPAPI envelope. It is deleted
/// rather than migrated: this is a rebuildable cache with a seven-day life,
/// and a migration is code that runs once and is wrong forever after.
pub const FORMAT_VERSION: u32 = 2;

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
/// AES-GCM's authentication tag, appended to every sealed blob. Named
/// because `sealed_len` computes offsets from it and a wrong number there is
/// an index that points into the middle of a neighbour.
const TAG_LEN: usize = 16;
const CONTENT_KEY_LEN: usize = 32;

pub(crate) const FILE_NAME: &str = "vault-cache.bin";
pub(crate) const TMP_FILE_NAME: &str = "vault-cache.bin.tmp";

/// The sealing key's own file, beside the cache in the same account
/// directory. Separate from `vault-cache.bin` rather than a field inside it
/// for the reason [`stored_seal_key`] gives: a cache file is thrown away for
/// half a dozen ordinary reasons, and a key that went with it would be a key
/// rotated by expiry.
pub(crate) const KEY_FILE_NAME: &str = "vault-cache-key.bin";

/// Four bytes at the front of the sealing key's plaintext, so a file that is
/// not one of these is refused rather than read as key material.
/// [`crate::user_key_store`]'s reason exactly: DPAPI already refuses another
/// Windows user, so this catches *our own* mistake -- a `session.bin` copied
/// over this path, or this layout changed without its version.
const KEY_MAGIC: &[u8; 4] = b"DWCK";

/// The key file's half-written name. Named beside its file rather than
/// computed at the two places that touch it, so log out cannot forget one.
pub(crate) const KEY_TMP_FILE_NAME: &str = "vault-cache-key.bin.tmp";

/// Bumped only for an incompatible change to the key file's layout. An
/// unknown version mints a fresh key rather than erroring, which costs one
/// rebuilt cache.
const KEY_FORMAT_VERSION: u8 = 1;

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
    /// Length of the sealed facts section, which is the first blob in the
    /// body. Carried here rather than inferred so a body truncated between
    /// the facts and the first item is a refusal rather than a slice that
    /// happens to parse.
    pub facts_len: u32,
    /// Where each item's sealed bytes are. Plaintext, and deliberately so --
    /// see [`ItemSlot`].
    pub index: Vec<ItemSlot>,
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
/// Takes the sealing key as a parameter, and there is deliberately no other
/// way to build a file. If a path ever appears that writes one without a
/// seal, the setting's description becomes a false security claim.
/// # The header is sealed over, so it is built here rather than passed whole
///
/// `facts_len` and `index` are outputs of the sealing, not inputs to it: the
/// caller cannot know an offset before the blobs exist. So this takes the
/// header the caller CAN fill in, seals the body, and writes the two
/// discovered fields back before serialising it -- which matters because the
/// header is the AAD, so it has to be final before anything is bound to it.
pub(crate) fn encode_file(
    seal_key: &[u8; 32],
    header: &CacheHeader,
    facts: &[u8],
    snapshot: &DiskSnapshot,
) -> Result<Vec<u8>, String> {
    // Sealed once with a placeholder header to learn the sizes, then again
    // with the real one. Two passes rather than a guess: the facts length
    // depends on the ciphertext, the ciphertext depends on the AAD, and the
    // AAD is the header that carries the length.
    let mut header = header.clone();
    let items: Vec<(String, Vec<u8>)> = snapshot
        .items
        .iter()
        .map(|item| {
            serde_json::to_vec(item)
                .map(|bytes| (item.id.clone(), bytes))
                .map_err(|e| format!("could not serialize a cached item: {e}"))
        })
        .collect::<Result<_, _>>()?;

    // The offsets are arithmetic, not a trial run: a GCM blob here is
    // `nonce ‖ ciphertext ‖ tag`, and its length depends only on the
    // plaintext's -- not on the key, the nonce or the AAD. So the index can
    // be computed before anything is sealed, which is what lets the header
    // be final before it becomes the AAD that binds the body to it.
    header.facts_len = sealed_len(facts.len())?;
    let mut at = header.facts_len;
    header.index = Vec::with_capacity(items.len());
    for (id, plaintext) in &items {
        let len = sealed_len(plaintext.len())?;
        header.index.push(ItemSlot { id: id.clone(), at, len });
        at = at
            .checked_add(len)
            .ok_or_else(|| "the vault cache body outgrew a 32-bit offset".to_string())?;
    }

    let header_bytes = serde_json::to_vec(&header)
        .map_err(|e| format!("could not serialize the cache header: {e}"))?;

    // A fresh random content key per write. Reusing one across writes would
    // mean reusing a key across many nonces for no benefit; generating one
    // costs nothing at human-paced write frequency.
    let mut content_key = Zeroizing::new([0u8; CONTENT_KEY_LEN]);
    getrandom::getrandom(content_key.as_mut_slice())
        .map_err(|e| format!("no randomness for the content key: {e}"))?;


    let sealed_key = seal(seal_key, content_key.as_slice(), None)?;
    let (body, sealed_index) = seal_body(&content_key, &header_bytes, facts, &items)?;
    // The arithmetic above and the sealing here must agree, or every offset
    // in the file is a lie that only shows up as a refusal months later.
    debug_assert_eq!(sealed_index, header.index, "the computed index disagrees with the sealed body");

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
/// **Touches no key at all**, which is what lets a doomed file be deleted
/// without so much as unwrapping the sealing key.
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

/// Unseals the content key under this account's sealing key.
///
/// Split out because version 2 has three readers of it -- the whole-snapshot
/// load below, the facts section, and one item at a time -- and each of them
/// needing its own copy of this unwrap is how the three come to disagree.
pub(crate) fn content_key_of(
    seal_key: &[u8; 32],
    parsed: &Parsed,
) -> Result<Zeroizing<[u8; CONTENT_KEY_LEN]>, String> {
    let content_key = unseal(seal_key, &parsed.sealed_key, None)?;
    let content_key: [u8; CONTENT_KEY_LEN] = content_key
        .as_slice()
        .try_into()
        .map_err(|_| "the sealed content key is the wrong size".to_string())?;
    Ok(Zeroizing::new(content_key))
}

/// The whole snapshot, given a content key already unwrapped.
///
/// Split from the whole-file decode because `load` now unwraps the key
/// in its shared preamble and would otherwise unwrap it twice -- which is
/// not merely wasteful: two unwraps are two places that can disagree about
/// what a bad key means.
pub(crate) fn decode_body_with(
    content_key: &[u8; CONTENT_KEY_LEN],
    header: &CacheHeader,
    parsed: &Parsed,
) -> Result<DiskSnapshot, String> {
    let mut items = Vec::with_capacity(header.index.len());
    for slot in &header.index {
        let plaintext = open_item_at(&content_key, &parsed.header_bytes, &parsed.body, slot)?;
        items.push(
            serde_json::from_slice(&plaintext)
                .map_err(|e| format!("a cached item is malformed: {e}"))?,
        );
    }
    // Folders ride in the facts section: they carry no secret, and a reader
    // that wants names wants theirs too.
    let facts = open_facts(&content_key, &parsed.header_bytes, &parsed.body, header.facts_len)?;
    let folders = folders_from_facts(&facts)?;
    Ok(DiskSnapshot { items, folders })
}

/// The folders out of a facts section, without knowing what else is in it.
///
/// The facts blob is the caller's shape -- this module seals it and does not
/// define it -- so this reads the one field it is contracted to contain and
/// ignores the rest. A facts section that has grown a field is not an error
/// here; a facts section with no folders is.
fn folders_from_facts(facts: &[u8]) -> Result<Vec<Folder>, String> {
    #[derive(Deserialize)]
    struct HasFolders {
        folders: Vec<Folder>,
    }
    serde_json::from_slice::<HasFolders>(facts)
        .map(|f| f.folders)
        .map_err(|e| format!("the cached facts section is malformed: {e}"))
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

/// How long a sealed blob is, from its plaintext's length.
///
/// `nonce ‖ ciphertext ‖ tag`, and GCM's ciphertext is the same length as
/// its plaintext -- so this depends on nothing but the number below, which
/// is what lets the index be computed before anything is sealed.
fn sealed_len(plaintext_len: usize) -> Result<u32, String> {
    u32::try_from(NONCE_LEN + plaintext_len + TAG_LEN)
        .map_err(|_| "a cached item outgrew a 32-bit length".to_string())
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
/// is the same one: **no test in this crate may call DPAPI or write into the
/// real account directory.** Both are real Win32 / filesystem effects against
/// the user's own machine, and neither carries a decision this module could
/// get wrong.
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
    /// **The only way a sealing key is ever obtained**, and deliberately a
    /// step that cannot show UI -- see the module doc's account of the launch
    /// this feature used to hang.
    ///
    /// Takes the account's directory because the key lives in a file beside
    /// the cache: one key per account, minted on first use.
    pub seal_key: fn(&Path) -> Result<Zeroizing<[u8; 32]>, String>,
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
            seal_key: stored_seal_key,
        }
    }
}

/// The outcome of trying to load the file. Five distinct situations, five
/// variants -- deliberately not collapsed into `Result<Option<..>, ..>`,
/// because they call for different actions. In particular [`Self::Unavailable`]
/// must **leave the file alone** while [`Self::Rejected`] and [`Self::Corrupt`]
/// must delete it.
impl DiskFactsLoad {
    /// The same refusal, as [`DiskCacheLoad`].
    ///
    /// Only the refusals convert: `Loaded` carries facts, which a
    /// whole-snapshot caller has no use for, and a conversion that invented
    /// an empty vault for it would be a silent wrong answer. It is
    /// unreachable because the shared preamble never produces one.
    fn into_load(self) -> DiskCacheLoad {
        match self {
            Self::Absent => DiskCacheLoad::Absent,
            Self::Rejected(reason) => DiskCacheLoad::Rejected(reason),
            Self::Unavailable(e) => DiskCacheLoad::Unavailable(e),
            Self::Corrupt(e) => DiskCacheLoad::Corrupt(e),
            Self::Loaded { .. } => DiskCacheLoad::Corrupt(
                "the shared preamble returned facts to a whole-snapshot caller".to_string(),
            ),
        }
    }
}

/// What [`DiskCache::load_facts`] found.
///
/// Deliberately the same refusal vocabulary as [`DiskCacheLoad`] rather than
/// a reduced one: the two entry points share every step up to the
/// decryption, so a reason one can give and the other cannot would mean they
/// had drifted.
pub enum DiskFactsLoad {
    Loaded {
        /// The caller's own shape, opaque here. See [`DiskCache::write`].
        facts: Zeroizing<Vec<u8>>,
        written_at: SystemTime,
    },
    Absent,
    Rejected(RejectReason),
    Unavailable(String),
    Corrupt(String),
}

/// Hand-written, and `debug_leak_guard` is why: a derived one prints
/// `facts`, which is a whole vault's worth of names and usernames at best
/// and whatever the caller put there at worst.
///
/// **This module cannot know the facts section is secret-free.** It takes
/// opaque bytes on purpose -- that is the layering that keeps it from having
/// opinions about what a projection contains -- and the price of not knowing
/// is that it must not print them. The length is what a reader debugging a
/// load actually needs.
///
/// The comment on this type used to *claim* the length-only behaviour while
/// the type carried a `#[derive(Debug)]`. The guard caught it; the claim is
/// now the implementation.
impl std::fmt::Debug for DiskFactsLoad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loaded { facts, written_at } => f
                .debug_struct("Loaded")
                .field("facts", &format_args!("<{} bytes>", facts.len()))
                .field("written_at", written_at)
                .finish(),
            Self::Absent => f.write_str("Absent"),
            Self::Rejected(reason) => f.debug_tuple("Rejected").field(reason).finish(),
            Self::Unavailable(e) => f.debug_tuple("Unavailable").field(e).finish(),
            Self::Corrupt(e) => f.debug_tuple("Corrupt").field(e).finish(),
        }
    }
}

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
    /// The header disqualified it. Deleted, unread, without touching a key.
    Rejected(RejectReason),
    /// The sealing key could not be read or minted -- DPAPI refused, or the
    /// key file could not be written. The file is **left in place**: this
    /// session cannot open it, but a later one on a healthier machine can,
    /// and deleting a readable cache over a transient failure is the one
    /// mistake that cannot be undone.
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

/// Whether Windows Hello is set up on this machine at all.
///
/// **Nothing in this module needs it any more** -- the sealing key is DPAPI
/// alone, see the module doc. This is kept because `prefs_ui` still asks it
/// to decide what the disk-cache row says and whether it is offered, and that
/// copy is a user-facing security claim that has to be re-decided by the
/// owner rather than quietly rewritten here.
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
    /// The file this process has open, if [`Self::load_facts`] opened one.
    ///
    /// **Held so that opening one item costs neither a read nor an unwrap.**
    /// The whole point of version 2 is that a fill can reach one secret; if
    /// each reach re-read the file and re-unsealed the content key, every
    /// password would cost two file reads and a DPAPI call.
    ///
    /// What is retained is ciphertext plus one key. The key is the thing
    /// that matters, and it is dropped by [`Self::close`] -- which the same
    /// lock that empties the in-memory snapshot calls, so the daemon does
    /// not hold the means to open the vault after the vault is locked.
    open: Mutex<Option<OpenFile>>,
}

/// A cache file this process can read items out of.
///
/// `body` is ciphertext and `header_bytes` is the AAD it is bound to; both
/// are kept verbatim because re-serialising the header would produce
/// different bytes for the same values and every open would then fail
/// authentication for a reason nobody would guess.
struct OpenFile {
    header: CacheHeader,
    header_bytes: Vec<u8>,
    body: Vec<u8>,
    content_key: Zeroizing<[u8; CONTENT_KEY_LEN]>,
}

struct Paths {
    /// Kept alongside the two derived paths because the sealing key is
    /// obtained *from the directory*, not from the cache file: asking
    /// `file.parent()` for it would let a path with no parent -- which
    /// `Path::parent` permits -- mint a key somewhere nobody chose.
    dir: PathBuf,
    file: PathBuf,
    tmp: PathBuf,
}

impl Paths {
    fn in_dir(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            file: dir.join(FILE_NAME),
            tmp: dir.join(TMP_FILE_NAME),
        }
    }
}

/// The sealing key for this session.
///
/// Read (or minted) at most once per account per launch, because the spec
/// requires a rewrite after every populate *and every mutation*, and going
/// back to the file per write would be a read and an unwrap on every item
/// edit.
///
/// A failed acquisition is **not retried** for the rest of the session. The
/// prompt that rule was written for is gone, but the rule still earns its
/// place: what can fail now is the filesystem or DPAPI, and neither gets
/// better between two writes a second apart -- retrying would turn one
/// logged failure into one per mutation. `given_up` records it, so the
/// difference between "not tried yet" and "tried and failed" is in the state
/// rather than inferred from a `None`.
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
            open: Mutex::new(None),
            env,
        }
    }

    /// Points this cache at a different account's directory.
    ///
    /// Called by the account switch **before** it authenticates, for
    /// `SessionStore::path`'s reason exactly: a write issued after the switch
    /// began but against the outgoing account's path is a mutation no
    /// end-state assertion catches.
    ///
    /// **The session key goes with it**, which is the one thing that changed
    /// when the key stopped being Hello-derived. It used to be kept because
    /// re-deriving cost a prompt and bought no separation the file's location
    /// and the header's fingerprint did not already provide. The key is now
    /// per-directory -- one `vault-cache-key.bin` per account -- so keeping it
    /// would seal the incoming account's vault under the outgoing account's
    /// key, and every later load would find a file it could not open. Reading
    /// the right one costs a file read and no UI.
    pub fn repoint(&self, dir: &Path) {
        *self.lock_paths() = Paths::in_dir(dir);
        *self.lock_key() = KeyState::default();
    }

    /// The file this cache reads and writes.
    pub fn path(&self) -> PathBuf {
        self.lock_paths().file.clone()
    }

    /// Reads this account's sealing key, minting one if there is none yet.
    /// Used by the Preferences toggle, which wants to fail *before* the
    /// setting flips if the key cannot be established at all.
    ///
    /// **Shows nothing and waits for nobody.** It used to pop the Hello
    /// prompt and double as the confirmation gesture; the gesture is now the
    /// toggle itself.
    pub fn acquire_key(&self) -> Result<(), String> {
        let dir = self.lock_paths().dir.clone();
        let mut state = self.lock_key();
        self.ensure_key(&mut state, &dir).map(|_| ())
    }

    /// Whether a key is held for this session. Not a secret, and not the key:
    /// it is what "a doomed file never derived one" is asserted against.
    pub fn has_session_key(&self) -> bool {
        self.lock_key().key.is_some()
    }

    /// **Whether a copy is sitting on this machine that this session could
    /// not open.**
    ///
    /// The one state [`DiskCacheLoad`] cannot express. A failed key read
    /// answers [`DiskCacheLoad::Unavailable`] and sets `given_up`, and
    /// the file is still there, untouched -- unlike
    /// [`DiskCacheLoad::Rejected`], which has already deleted it, and unlike
    /// [`DiskCacheLoad::Absent`], where there was never one. A caller that
    /// reads only the load outcome therefore cannot tell "there is no local
    /// copy" from "there is one and you dismissed the fingerprint prompt",
    /// and the offline screens would tell a user whose key file was
    /// momentarily unreadable that their vault copy does not exist.
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

    /// **Lets one more key attempt happen**, after a failed one.
    ///
    /// `given_up` exists so a failure is never retried *on its own* -- see
    /// `KeyState`: a retry at the next write would repeat a logged failure
    /// once per mutation. That reasoning is about attempts nobody asked for,
    /// and it is the only reasoning `given_up` carries. It says nothing about
    /// a user pressing a button labelled *Continue offline*, which is a
    /// request for exactly this attempt, at exactly this moment.
    ///
    /// So this is not a loosening of that rule: it is the one gesture the rule
    /// was never about. Without it, one transient DPAPI or filesystem failure
    /// would put the local copy out of reach for the rest of the session while
    /// the screen went on offering it -- a button that does nothing, which is
    /// the treatment `prefs_ui` refuses.
    ///
    /// Only ever called from a user's own press.
    pub fn allow_one_more_key_attempt(&self) {
        self.lock_key().given_up = false;
    }

    /// `dir` is passed rather than read from `self.paths` because every
    /// caller already holds the key lock, and taking the paths lock under it
    /// would be the one place in this type where the two are nested.
    fn ensure_key(&self, state: &mut KeyState, dir: &Path) -> Result<[u8; 32], String> {
        if let Some(key) = state.key {
            return Ok(key);
        }
        if state.given_up {
            return Err(
                "the vault cache's key could not be read earlier in this session".to_string(),
            );
        }
        match (self.env.seal_key)(dir) {
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
    /// come first and touch no key, so an expired, foreign, or
    /// wrong-version file is deleted without a sealing key being read or
    /// minted on behalf of a file about to be thrown away.
    pub fn load(&self, fingerprint: &str) -> DiskCacheLoad {
        let file = self.lock_paths().file.clone();
        // The same preamble `load_facts` runs, so the two cannot drift in
        // which failures delete the file or when the key is read.
        let (header, parsed, key) = match self.opened_file(fingerprint) {
            Ok(v) => v,
            Err(refusal) => return refusal.into_load(),
        };
        match decode_body_with(&key, &header, &parsed) {
            Ok(snapshot) => {
                let written_at = UNIX_EPOCH + Duration::from_secs(header.written_at);
                // **The file is left open here too.** A caller that read the
                // whole vault may still want one item later -- a fill, after
                // the snapshot has been cleared by a lock -- and requiring
                // `load_facts` for that would mean two ways to arrive at the
                // same state, one of which silently does not work.
                if let Ok(mut slot) = self.open.lock() {
                    *slot = Some(OpenFile {
                        header,
                        header_bytes: parsed.header_bytes,
                        body: parsed.body,
                        content_key: key,
                    });
                }
                DiskCacheLoad::Loaded {
                    items: snapshot.items,
                    folders: snapshot.folders,
                    written_at,
                }
            }
            Err(e) => {
                log::info!("the vault cache could not be decrypted ({e}); rebuilding it");
                let _ = std::fs::remove_file(&file);
                DiskCacheLoad::Corrupt(e)
            }
        }
    }

    /// The facts section, and the file left open so items can be reached.
    ///
    /// Everything before the decryption is [`Self::load`]'s, unchanged: the
    /// header is checked, an expired or foreign file is deleted without a
    /// key, and the key is read only once the file is known to be ours.
    /// What differs is what comes back -- the facts, and **no secret at all**
    /// -- and that the content key is retained so [`Self::open_item`] costs
    /// nothing.
    pub fn load_facts(&self, fingerprint: &str) -> DiskFactsLoad {
        let file = self.lock_paths().file.clone();
        let (header, parsed, key) = match self.opened_file(fingerprint) {
            Ok(v) => v,
            Err(load) => return load,
        };
        match open_facts(&key, &parsed.header_bytes, &parsed.body, header.facts_len) {
            Ok(facts) => {
                let written_at = UNIX_EPOCH + Duration::from_secs(header.written_at);
                if let Ok(mut slot) = self.open.lock() {
                    *slot = Some(OpenFile {
                        header,
                        header_bytes: parsed.header_bytes,
                        body: parsed.body,
                        content_key: key,
                    });
                }
                DiskFactsLoad::Loaded { facts, written_at }
            }
            Err(e) => {
                log::info!("the vault cache's facts could not be decrypted ({e}); rebuilding it");
                let _ = std::fs::remove_file(&file);
                DiskFactsLoad::Corrupt(e)
            }
        }
    }

    /// One item out of the open file, or `None`.
    ///
    /// `None` covers every way this can fail and they are deliberately not
    /// distinguished: no file open, no such id, a slot that points outside
    /// the body, a blob that will not authenticate. The caller's answer is
    /// the same in all of them -- ask the backend instead -- and a caller
    /// that could tell them apart would be a caller that might treat one as
    /// recoverable.
    ///
    /// **Returns `None` after [`Self::close`]**, because the key is gone. A
    /// locked vault must not be openable by the process that locked it.
    pub fn open_item(&self, id: &str) -> Option<VaultItem> {
        let open = self.open.lock().ok()?;
        let open = open.as_ref()?;
        let slot = open.header.index.iter().find(|slot| slot.id == id)?;
        let plaintext = open_item_at(&open.content_key, &open.header_bytes, &open.body, slot)
            .map_err(|e| log::warn!("the cached copy of item {id} would not open: {e}"))
            .ok()?;
        serde_json::from_slice(&plaintext)
            .map_err(|e| log::warn!("the cached copy of item {id} is malformed: {e}"))
            .ok()
    }

    /// Drops the open file, and with it the content key.
    ///
    /// Called wherever the in-memory snapshot is emptied. The file stays on
    /// disk -- it is meant to survive a lock -- but this process stops being
    /// able to read it until the user unlocks and the file is opened again.
    pub fn close(&self) {
        if let Ok(mut slot) = self.open.lock() {
            *slot = None;
        }
    }

    /// The shared preamble of [`Self::load`] and [`Self::load_facts`]:
    /// read, unwrap, parse, check the header, and only then reach for the
    /// sealing key.
    ///
    /// Returns the load result to hand straight back on any refusal, so the
    /// two entry points cannot drift in which failures delete the file.
    fn opened_file(
        &self,
        fingerprint: &str,
    ) -> Result<(CacheHeader, Parsed, Zeroizing<[u8; CONTENT_KEY_LEN]>), DiskFactsLoad> {
        let (dir, file, tmp) = {
            let paths = self.lock_paths();
            (paths.dir.clone(), paths.file.clone(), paths.tmp.clone())
        };
        // A leftover temp file is a write that was interrupted. Removed here,
        // in the shared preamble, so both entry points clean it up -- it was
        // `load`'s alone until the preamble was split out, and a `load_facts`
        // that left one behind would be a slow leak nobody looked for.
        let _ = std::fs::remove_file(&tmp);
        let wrapped = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(DiskFactsLoad::Absent);
            }
            Err(e) => {
                log::warn!("could not read the vault cache: {e}");
                self.delete_quietly(&file, RejectReason::Malformed);
                return Err(DiskFactsLoad::Rejected(RejectReason::Malformed));
            }
        };
        let inner = match (self.env.unwrap)(&wrapped) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(e) => {
                log::warn!("the vault cache is not readable by this user ({e})");
                self.delete_quietly(&file, RejectReason::Malformed);
                return Err(DiskFactsLoad::Rejected(RejectReason::Malformed));
            }
        };
        let (header, parsed) = match parse_header(&inner) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("the vault cache file is unusable: {e}");
                self.delete_quietly(&file, RejectReason::Malformed);
                return Err(DiskFactsLoad::Rejected(RejectReason::Malformed));
            }
        };
        if let Err(reason) = check_header(&header, now_unix(), fingerprint) {
            log::info!("discarding the vault cache: it is {}", reason.as_str());
            self.delete_quietly(&file, reason);
            return Err(DiskFactsLoad::Rejected(reason));
        }
        let key = {
            let mut state = self.lock_key();
            match self.ensure_key(&mut state, &dir) {
                Ok(key) => key,
                Err(e) => {
                    log::info!("not using the vault cache this session: {e}");
                    return Err(DiskFactsLoad::Unavailable(e));
                }
            }
        };
        let content_key = match content_key_of(&key, &parsed) {
            Ok(k) => k,
            Err(e) => {
                // **Where a cache written by an older Deskwarden lands**, and
                // the reason this is `info` rather than `warn`: a file sealed
                // under the old Hello-derived key cannot be opened with this
                // one, and every machine that had the setting on takes this
                // branch exactly once. That is a cache miss, not a fault --
                // the file is deleted and the next populate writes a fresh
                // one, which is what a cache is for. Nothing above this shows
                // `Corrupt` to the user; `main` logs it and goes to the
                // backend.
                log::info!("the vault cache's content key would not open ({e}); rebuilding it");
                let _ = std::fs::remove_file(&file);
                return Err(DiskFactsLoad::Corrupt(e));
            }
        };
        Ok((header, parsed, content_key))
    }

    /// Writes the snapshot. Atomic: a full write to `.tmp` followed by a
    /// rename over the target, so a crash mid-write cannot leave a truncated
    /// file whose corruption would only be discovered on the next launch.
    ///
    /// **There is deliberately no fallback that writes without a key.** This
    /// is the property the setting's description rests on: no path in this
    /// crate can produce a file that is not sealed, because [`encode_file`]
    /// takes the key as a parameter and this is its only caller.
    ///
    /// It *establishes* the key rather than demanding one already held, which
    /// it could not do while the key came from Hello -- a prompt in the middle
    /// of saving an edit was unacceptable, so the key had to be taken at a
    /// moment the user had chosen. Reading or minting a file is silent, so
    /// that constraint is gone, and its absence closes a real gap: after an
    /// account switch (see [`Self::repoint`]) the incoming account may have no
    /// cache to load, in which case nothing before this would have obtained
    /// its key and every write for the rest of the session would have failed.
    /// `facts` is opaque: whatever the caller wants readable without opening
    /// a secret. This module seals it and records its length; it has no
    /// opinion about what a projection contains, because that is a decision
    /// about what the picker needs and belongs where the picker is.
    pub fn write(
        &self,
        fingerprint: &str,
        facts: &[u8],
        items: &[VaultItem],
        folders: &[Folder],
    ) -> Result<(), String> {
        let key = {
            let dir = self.lock_paths().dir.clone();
            let mut state = self.lock_key();
            self.ensure_key(&mut state, &dir)?
        };

        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: now_unix(),
            account_fingerprint: fingerprint.to_string(),
            item_count: items.len(),
            // Filled in by `encode_file`, which is the only thing that can
            // know them: an offset does not exist until the blobs do.
            facts_len: 0,
            index: Vec::new(),
        };
        let snapshot = DiskSnapshot {
            items: items.to_vec(),
            folders: folders.to_vec(),
        };
        let inner = Zeroizing::new(encode_file(&key, &header, facts, &snapshot)?);
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
/// **The key goes too.** Its file is not the cache, so nothing else deletes
/// it -- `delete` deliberately leaves it, because a re-auth or a disabled
/// setting may be followed by another write and rotating the key there is
/// churn. Log out is the one moment where the account is gone from this
/// machine, and leaving thirty-two bytes that sealed a vault behind is the
/// liability this function's whole doc is about.
pub fn forget_for(dir: &Path) {
    for name in [FILE_NAME, TMP_FILE_NAME, KEY_FILE_NAME, KEY_TMP_FILE_NAME] {
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

/// This account's sealing key: the 32 random bytes in
/// `<dir>/vault-cache-key.bin`, minted on first use.
///
/// **The whole of what replaced the Windows Hello signature**, and the reason
/// the app starts again -- see the module doc. Every step here is silent and
/// non-interactive; there is no path through it that can wait for a person.
///
/// # Why a file of random bytes rather than something derived
///
/// DPAPI protects data, it does not yield a key, and its output is not
/// deterministic -- so "DPAPI-derived key" is not a thing that exists. What
/// does exist is the shape [`crate::user_key_store`] already uses for the
/// master key, which is strictly more valuable than anything this seals: mint
/// randomness once, keep it DPAPI-wrapped, unwrap it on use.
///
/// # Why its own file
///
/// The cache file is deleted for half a dozen ordinary reasons -- expiry, a
/// foreign fingerprint, a re-auth, an unknown format version. A key stored
/// inside it would be a key rotated by each of those, which is churn with no
/// security in it, and the account's directory is already where its
/// `session.bin`, `hello.bin` and `userkey.bin` live.
///
/// # Why an unreadable key file mints a new one instead of failing
///
/// The only thing lost is a cache that could not have been read anyway: the
/// bytes that sealed it are gone. Failing here would leave the app with no
/// key, no cache, and a permanent error, when minting leaves it with a cache
/// rebuilt at the next populate. A DPAPI *call* failing is a different
/// matter -- that says this Windows user's credentials are not available, and
/// it is returned as an error rather than papered over with a fresh key that
/// could not be stored either.
fn stored_seal_key(dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    seal_key_in(
        dir,
        |bytes| crate::session_store::protect(bytes).map_err(|e| e.to_string()),
        |bytes| crate::session_store::unprotect(bytes).map_err(|e| e.to_string()),
    )
}

/// [`stored_seal_key`] with DPAPI as two ordinary parameters.
///
/// **Not a second seam.** `DiskCacheEnv` is this module's one `fn`-pointer
/// struct and stays that way; these are arguments, exactly as
/// `hello::open_blob` takes its key-getter, and there is one production
/// caller directly above that always passes the real pair. What it buys is
/// that the decisions here -- mint when there is no file, mint when the
/// stored bytes are not ours, reject a bad DPAPI call rather than minting
/// over it, and hand back the *same* key the second time -- are testable
/// without any test in this crate calling DPAPI.
fn seal_key_in(
    dir: &Path,
    protect: impl Fn(&[u8]) -> Result<Vec<u8>, String>,
    unprotect: impl Fn(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let path = dir.join(KEY_FILE_NAME);
    match std::fs::read(&path) {
        Ok(wrapped) => match unprotect(&wrapped) {
            Ok(plain) => {
                let plain = Zeroizing::new(plain);
                match parse_seal_key(&plain) {
                    Some(key) => return Ok(key),
                    None => log::warn!(
                        "the vault cache's key file at {} is not one of ours; minting a new key                          and rebuilding the cache",
                        path.display()
                    ),
                }
            }
            // Not an error: a key this Windows user cannot unwrap is a key
            // that can never open the cache beside it again, so there is
            // nothing to preserve by refusing. The mint below is checked, and
            // if DPAPI is genuinely unavailable it fails there with a real
            // message rather than here with a guess.
            Err(e) => log::warn!(
                "the vault cache's key file at {} could not be unwrapped ({e}); minting a new                  key and rebuilding the cache",
                path.display()
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "could not read {}: {e}",
                path.display()
            ))
        }
    }
    mint_seal_key(dir, &path, protect)
}

/// The stored plaintext, checked. `None` for anything that is not one of
/// ours, which the caller answers by minting.
///
/// A `Zeroizing` return and a plain `Option`: [`crate::user_key_store`]'s
/// rule that there is no error type here for a byte to hide in.
fn parse_seal_key(plain: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    if plain.len() != KEY_MAGIC.len() + 1 + 32 {
        return None;
    }
    if &plain[..KEY_MAGIC.len()] != KEY_MAGIC {
        return None;
    }
    if plain[KEY_MAGIC.len()] != KEY_FORMAT_VERSION {
        return None;
    }
    let key: [u8; 32] = plain[KEY_MAGIC.len() + 1..].try_into().ok()?;
    Some(Zeroizing::new(key))
}

/// Mints 32 random bytes, stores them DPAPI-wrapped at `path`, and returns
/// them.
///
/// **The write is checked and its failure is returned.** A key that was
/// handed out but not stored would seal a cache file that the next launch
/// could never open -- a cache rebuilt on every start, which is the exact
/// outcome this feature exists to avoid and the one that would be hardest to
/// notice, because everything would appear to work.
///
/// Written to a temp name and renamed, for the cache file's reason: a crash
/// between the two must not leave a truncated key on the path the next launch
/// reads.
fn mint_seal_key(
    dir: &Path,
    path: &Path,
    protect: impl Fn(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(key.as_mut_slice())
        .map_err(|e| format!("no randomness for the vault cache's key: {e}"))?;

    let mut plain = Zeroizing::new(Vec::with_capacity(KEY_MAGIC.len() + 1 + 32));
    plain.extend_from_slice(KEY_MAGIC);
    plain.push(KEY_FORMAT_VERSION);
    plain.extend_from_slice(key.as_slice());
    let wrapped =
        protect(&plain).map_err(|e| format!("DPAPI could not wrap the vault cache's key: {e}"))?;

    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let tmp = dir.join(KEY_TMP_FILE_NAME);
    std::fs::write(&tmp, &wrapped).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace {}: {e}", path.display())
    })?;
    log::info!(
        "minted a new key for the encrypted vault copy at {}",
        path.display()
    );
    Ok(key)
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

    /// **The reader that makes version 2 worth having**: facts come back
    /// and no secret is opened to produce them.
    #[test]
    fn load_facts_hands_back_the_facts_and_opens_no_secret() {
        let dir = temp_dir_for("facts-open-no-secret");
        let cache = cache_with_key(&dir);
        let snap = snapshot();
        cache
            .write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders)
            .unwrap();

        let DiskFactsLoad::Loaded { facts, .. } = cache.load_facts("fp") else {
            panic!("the facts did not load");
        };
        let rendered = String::from_utf8_lossy(&facts).to_lowercase();
        assert!(
            !rendered.contains("hunter2"),
            "a password reached the facts section, which is the one part the daemon reads \
             whole: {rendered}"
        );
        assert!(rendered.contains("work"), "control: the folders did not come back either");
    }

    /// One item, by id, out of the file `load_facts` left open -- without
    /// going back for the key, which is what the retained content key is for.
    #[test]
    fn open_item_reaches_one_secret_without_asking_for_the_key_again() {
        let dir = temp_dir_for("open-item-no-second-key-read");
        KEY_ASKED.store(0, Ordering::SeqCst);
        let cache = DiskCache::new(&dir, env(counting_key));
        let snap = snapshot();

        cache.acquire_key().expect("the substituted key step");
        cache
            .write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders)
            .unwrap();
        assert!(matches!(cache.load_facts("fp"), DiskFactsLoad::Loaded { .. }));
        let id = snap.items[0].id.clone();
        let opened = cache.open_item(&id).expect("the item");
        assert_eq!(opened.id, id);

        // **Once for the whole session.** A write, a facts load and an item
        // open, and the key file was read a single time. Per-item opens are
        // the thing version 2 exists to allow, and a fill that re-read and
        // re-unwrapped the key for every password would pay two file reads
        // and a DPAPI call each time.
        assert_eq!(
            KEY_ASKED.load(Ordering::SeqCst),
            1,
            "the sealing key was fetched more than once, so reaching a password costs a \
             file read and a DPAPI call every time"
        );
    }

    /// An id the file does not carry is `None`, not a panic and not the
    /// wrong item.
    #[test]
    fn an_unknown_id_is_none() {
        let dir = temp_dir_for("unknown-id-is-none");
        let cache = cache_with_key(&dir);
        let snap = snapshot();
        cache
            .write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders)
            .unwrap();
        assert!(matches!(cache.load_facts("fp"), DiskFactsLoad::Loaded { .. }));
        assert!(cache.open_item("no-such-id").is_none());
    }

    /// **A locked vault cannot be reopened by the process that locked it.**
    ///
    /// `close` drops the content key. The file stays on disk, because it is
    /// meant to survive a lock -- but this process has to ask Hello again
    /// before it can read a password out of it. Without this, locking would
    /// empty the snapshot while leaving the daemon holding the means to
    /// refill it, which is the appearance of locking rather than locking.
    #[test]
    fn an_item_cannot_be_opened_after_the_file_is_closed() {
        let dir = temp_dir_for("closed-file-opens-nothing");
        let cache = cache_with_key(&dir);
        let snap = snapshot();
        cache
            .write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders)
            .unwrap();
        assert!(matches!(cache.load_facts("fp"), DiskFactsLoad::Loaded { .. }));
        let id = snap.items[0].id.clone();
        assert!(cache.open_item(&id).is_some(), "control: it was openable before the close");

        cache.close();
        assert!(
            cache.open_item(&id).is_none(),
            "an item opened after the file was closed, so locking the vault leaves the daemon \
             holding the key to it"
        );
    }

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

    /// A facts section shaped as the production writer makes one.
    ///
    /// `vault_cache` serialises `{ items, folders }`; the only field this
    /// module reads back is `folders`, and a fixture that omitted it would
    /// pass the write and fail every read for a reason unrelated to what was
    /// being tested.
    pub(crate) fn facts_for(folders: &[Folder]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "items": [], "folders": folders }))
            .expect("the facts fixture")
    }

    /// Unwrap the content key, then read the body -- the two steps `load`
    /// takes, as one call so the tests below read as they did before the
    /// preamble was split out.
    ///
    /// A helper here rather than a wrapper in production: nothing in the app
    /// unwraps a key just to read a whole body any more, and a function kept
    /// alive for tests is a function that stops matching what runs.
    fn decode_body(
        seal_key: &[u8; 32],
        header: &CacheHeader,
        parsed: &Parsed,
    ) -> Result<DiskSnapshot, String> {
        let content_key = content_key_of(seal_key, parsed)?;
        decode_body_with(&content_key, header, parsed)
    }

    fn key(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    /// The sealing-key step, substituted: a fixed key, ignoring the
    /// directory. It ignores the directory on purpose -- these tests are
    /// about the file format and the load order, and a fixture that varied
    /// with the path would make every one of them depend on `temp_dir_for`'s
    /// naming. The tests that *are* about the real per-directory key drive
    /// [`stored_seal_key`]'s pure parts directly.
    fn key_seven(_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
        Ok(Zeroizing::new(key(7)))
    }

    /// How many times the substituted key step has been asked.
    ///
    /// A static because `DiskCacheEnv::seal_key` is a `fn` pointer and
    /// cannot close over a counter -- the same reason every other seam in
    /// this crate counts this way.
    static KEY_ASKED: AtomicUsize = AtomicUsize::new(0);

    fn counting_key(_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
        KEY_ASKED.fetch_add(1, Ordering::SeqCst);
        Ok(Zeroizing::new(key(7)))
    }

    fn key_unavailable(_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
        Err("the vault cache's key file could not be read".to_string())
    }

    /// DPAPI, substituted. Identity rather than a fake cipher: what this
    /// module has to get right about the envelope is that it is applied and
    /// removed in the right order, not what it contains.
    fn no_wrap(bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }

    /// DPAPI, substituted for the key file's own tests -- the same identity
    /// as [`no_wrap`], separately named because `seal_key_in` takes it as an
    /// ordinary argument rather than through the env.
    fn wrap_identity(bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }

    /// A test env whose sealing-key step is `seal_key`, with DPAPI stubbed
    /// out.
    pub(crate) fn env(seal_key: fn(&Path) -> Result<Zeroizing<[u8; 32]>, String>) -> DiskCacheEnv {
        DiskCacheEnv {
            wrap: no_wrap,
            unwrap: no_wrap,
            seal_key,
        }
    }

    /// A `DiskCache` over `dir` that already holds a fixed key, for the
    /// callers -- here and in `vault_cache` -- that are testing what happens
    /// *after* Hello has been satisfied.
    pub(crate) fn cache_with_key(dir: &Path) -> DiskCache {
        let cache = DiskCache::new(dir, env(key_seven));
        cache.acquire_key().expect("the substituted key step cannot fail");
        cache
    }

    /// A cache over `dir` whose sealing-key step always fails -- the shape of
    /// a session where the key file cannot be read or minted. Nothing it does
    /// touches the real filesystem's key.
    pub(crate) fn cache_whose_key_is_unavailable(dir: &Path) -> DiskCache {
        DiskCache::new(dir, env(key_unavailable))
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
            // A fixture header: `encode_file` overwrites both, and every
            // test that reads a file goes through it.
            facts_len: 0,
            index: Vec::new(),
        }
    }

    // -- the format, with no I/O at all ------------------------------------

    #[test]
    fn a_file_round_trips_under_the_same_key() {
        let snap = snapshot();
        let header = header_for(&snap, 1_000);
        let bytes = encode_file(&key(7), &header, &facts_for(&snap.folders), &snap).unwrap();

        let (read_header, parsed) = parse_header(&bytes).unwrap();
        // **Not equal to the header that went in, and that is the format**
        // **change.** `encode_file` fills in `facts_len` and `index`, because
        // an offset does not exist until the blobs do. The caller's fields
        // survive; the discovered ones are added.
        assert_eq!(read_header.account_fingerprint, header.account_fingerprint);
        assert_eq!(read_header.item_count, header.item_count);
        assert_eq!(read_header.written_at, header.written_at);
        assert!(read_header.facts_len > 0, "the facts section was not recorded");
        assert_eq!(read_header.index.len(), 1, "the one item got no slot");
        assert_eq!(read_header.index[0].id, snap.items[0].id);

        let opened = decode_body(&key(7), &read_header, &parsed).unwrap();
        assert_eq!(opened.items.len(), 1);
        assert_eq!(opened.folders[0].name, "Work");
    }

    #[test]
    fn the_header_is_readable_without_the_key() {
        // The whole reason the header sits outside the sealed body: the app
        // must be able to reject an expired or foreign file *without*
        // popping a Hello prompt for a file it is about to delete.
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
        let (header, _) = parse_header(&bytes).unwrap();
        assert_eq!(header.item_count, 1);
    }

    #[test]
    fn the_wrong_key_cannot_open_the_body() {
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
        let (header, parsed) = parse_header(&bytes).unwrap();
        assert!(decode_body(&key(8), &header, &parsed).is_err());
    }

    #[test]
    fn tampering_with_the_body_fails_authentication() {
        let snap = snapshot();
        let mut bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let (header, parsed) = parse_header(&bytes).unwrap();
        assert!(decode_body(&key(7), &header, &parsed).is_err());
    }

    #[test]
    fn tampering_with_the_header_fails_authentication() {
        // This is the AAD binding being live: `written_at` cannot be edited
        // to defeat expiry, because the header authenticates the body.
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
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
            decode_body(&key(7), &reread, &reparsed).is_err(),
            "an edited header still opened the body: the AAD binding is not live"
        );
    }

    #[test]
    fn a_truncated_file_is_rejected_without_panicking() {
        let snap = snapshot();
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
        for cut in [0usize, 2, 4, 8, 20, bytes.len() / 2, bytes.len() - 1] {
            let rejected = match parse_header(&bytes[..cut]) {
                Err(_) => true,
                Ok((header, parsed)) => decode_body(&key(7), &header, &parsed).is_err(),
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
            facts_len: 0,
            index: Vec::new(),
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
            facts_len: 0,
            index: Vec::new(),
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
            facts_len: 0,
            index: Vec::new(),
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
            facts_len: 0,
            index: Vec::new(),
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
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &facts_for(&snap.folders), &snap).unwrap();
        let (header, parsed) = parse_header(&bytes).unwrap();
        let opened = decode_body(&key(7), &header, &parsed).unwrap();

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
        let facts = facts_for(&snap.folders);
        let a = encode_file(&key(7), &header, &facts, &snap).unwrap();
        let b = encode_file(&key(7), &header, &facts, &snap).unwrap();
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
        let inner = encode_file(k, header, &facts_for(&snap.folders), snap).unwrap();
        std::fs::write(dir.join(FILE_NAME), no_wrap(&inner).unwrap()).unwrap();
    }

    fn header_at(written_at: u64, fingerprint: &str, version: u32) -> CacheHeader {
        CacheHeader {
            format_version: version,
            written_at,
            account_fingerprint: fingerprint.to_string(),
            item_count: 1,
            facts_len: 0,
            index: Vec::new(),
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
                seal_key: key_seven,
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
        let cache = DiskCache::new(&dir, env(key_unavailable));
        assert!(!cache.has_session_key());
        let snap = snapshot();
        assert!(cache.write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders).is_err());
        assert!(
            !dir.join(FILE_NAME).exists(),
            "a file was written with no Hello-sealed key"
        );
        assert!(!dir.join(TMP_FILE_NAME).exists());
    }

    #[test]
    fn an_unavailable_key_leaves_the_file_alone_and_is_not_retried() {
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

        let cache = DiskCache::new(&dir, env(key_unavailable));
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
    fn an_unavailable_key_over_a_real_file_is_visible_as_a_copy_that_is_still_there() {
        let dir = temp_dir_for("declined");
        let snap = snapshot();
        write_file_with_key(
            &dir,
            &key(7),
            &header_at(now_unix(), "fp", FORMAT_VERSION),
            &snap,
        );

        let cache = DiskCache::new(&dir, env(key_unavailable));
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
    fn an_unavailable_key_with_no_file_offers_nothing() {
        let dir = temp_dir_for("declined-nofile");
        let cache = DiskCache::new(&dir, env(key_unavailable));
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

        // Fails once, then answers -- the shape of a transient key-file
        // failure followed by the user asking for the copy. A `static` counter
        // rather than a capturing closure, because the seam is an `fn`
        // pointer.
        static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
        fn once_refusing(dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
            if ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err("the vault cache's key file could not be read".to_string());
            }
            key_seven(dir)
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
        cache.write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
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
        cache.write("fp", &facts_for(&[]), &[secret], &[]).unwrap();

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
        cache.write("fp-a", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
        let before = std::fs::read(first.join(FILE_NAME)).unwrap();

        cache.repoint(&second);
        cache.write("fp-b", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
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
        cache.write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
        let survivors_cache = cache_with_key(&survivor);
        survivors_cache.write("fp2", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
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

    /// **The startup path cannot reach Windows Hello.**
    ///
    /// This is the whole defect, asserted two ways, because either one alone
    /// can pass while the app hangs.
    ///
    /// By pointer identity: what `production()` will call is
    /// [`stored_seal_key`] and nothing else. A future edit that points the
    /// field at a prompting function fails here rather than at a launch that
    /// never finishes -- which is how this shipped, since a hang produces no
    /// error to notice.
    ///
    /// And by source, because pointer identity only says "it is this
    /// function", not "this function shows no UI": the production half of
    /// this file must name none of the three `KeyCredential` calls that put
    /// the OS dialog on screen. `stored_seal_key` could grow one and stay the
    /// same pointer.
    #[test]
    fn productions_sealing_key_cannot_reach_windows_hello() {
        // Pointer identity. The `as usize` casts are what `export_wiring`
        // does: `fn` items are zero-sized distinct types, so they have to be
        // coerced to a common `fn` pointer type before they compare.
        let production: fn(&Path) -> Result<Zeroizing<[u8; 32]>, String> =
            DiskCacheEnv::production().seal_key;
        let expected: fn(&Path) -> Result<Zeroizing<[u8; 32]>, String> = stored_seal_key;
        assert_eq!(
            production as usize, expected as usize,
            "production's sealing key does not come from the key file"
        );

        // POSITIVE CONTROL for the comparison itself. Without it this test
        // would still pass if every `fn` pointer in the build compared equal
        // -- which is not hypothetical: identical zero-argument bodies have
        // been merged by the linker before.
        let other: fn(&Path) -> Result<Zeroizing<[u8; 32]>, String> = key_seven;
        assert_ne!(
            production as usize, other as usize,
            "control: two different functions compared equal, so the assertion above proves \
             nothing"
        );

        // Source. `concat!`-split so no needle matches its own declaration
        // here, and the search is cut at `mod tests` so this very block does
        // not satisfy it -- the idiom `hello.rs` and `main.rs` both use.
        let source = include_str!("vault_disk_cache.rs");
        let cut = source
            .find(concat!("mod ", "tests {"))
            .expect("the test module opener must be findable, or the cut below is the whole file");
        let production_half = &source[..cut];
        for prompting in [
            concat!("RequestSign", "Async"),
            concat!("RequestCreate", "Async"),
            concat!("KeyCredentialManager::", "OpenAsync"),
        ] {
            assert_eq!(
                production_half.matches(prompting).count(),
                0,
                "the production half of this module names {prompting}, which is a call that can \
                 put a Windows Hello dialog on a startup path"
            );
        }

        // POSITIVE CONTROL for the scan. Three counts of zero are exactly
        // what a broken needle, a bad cut, or a renamed file produce, and
        // this repo has shipped a test that passed because it never reached
        // the thing it named. `IsSupportedAsync` is in the production half
        // (`hello_available`, which `prefs_ui` still calls) and shows the
        // same haystack answering non-zero for a real needle.
        assert!(
            production_half
                .matches(concat!("IsSupported", "Async"))
                .count()
                > 0,
            "control: the scan found nothing at all, so the three zeroes above are the \
             mechanism failing rather than the property holding"
        );
    }

    /// **A missing key file mints one**, stores it, and hands it back.
    ///
    /// Driven through [`seal_key_in`] with DPAPI as identity, so no test
    /// calls the real thing -- the module doc's rule.
    #[test]
    fn a_missing_key_file_mints_one() {
        let dir = temp_dir_for("mint-a-key");
        let path = dir.join(KEY_FILE_NAME);
        // The premise. Without this the test could be reading a key some
        // earlier run left behind and calling it a mint.
        assert!(!path.exists(), "control: the key file was already there");

        let key = seal_key_in(&dir, wrap_identity, wrap_identity).expect("a key should be minted");

        assert!(path.exists(), "the minted key was not stored");
        assert_ne!(*key, [0u8; 32], "the minted key is all zeroes, so it is not random");
        // It is really *in* the file, in this module's own layout -- a mint
        // that returned bytes it did not store would leave every launch
        // rebuilding the cache, which is the failure hardest to notice
        // because everything appears to work.
        let stored = std::fs::read(&path).expect("the key file");
        assert_eq!(
            parse_seal_key(&stored).map(|k| *k),
            Some(*key),
            "the file does not hold the key that was handed out"
        );
        // And no half-written file was left behind by the rename.
        assert!(!dir.join(KEY_TMP_FILE_NAME).exists());
    }

    /// **A second read returns the same key.**
    ///
    /// The property the feature rests on: a key that changed per launch would
    /// make every start find a cache it cannot open, silently rebuild it, and
    /// buy nothing at all -- while looking exactly like success.
    #[test]
    fn a_second_read_returns_the_same_key() {
        let dir = temp_dir_for("stable-key");
        let first = seal_key_in(&dir, wrap_identity, wrap_identity).expect("the mint");
        let second = seal_key_in(&dir, wrap_identity, wrap_identity).expect("the read");
        assert_eq!(*first, *second, "the sealing key changed between two reads");

        // POSITIVE CONTROL. `assert_eq!` on two keys passes just as happily
        // if the function always returns a constant, or if both calls minted
        // and the equality is an artifact of a broken mint. A *different*
        // directory must produce a *different* key, which pins both: the
        // value comes from the file, and the file is per account.
        let other_dir = temp_dir_for("stable-key-other");
        let elsewhere = seal_key_in(&other_dir, wrap_identity, wrap_identity).expect("the mint");
        assert_ne!(
            *first, *elsewhere,
            "control: two accounts' directories yielded the same key, so the equality above is \
             not evidence the key was read back from anywhere"
        );

        // And the round trip really goes through the file rather than through
        // some retained state: a key file replaced with rubbish is not ours,
        // so the next read mints a fresh one instead of failing.
        std::fs::write(dir.join(KEY_FILE_NAME), b"not one of ours").unwrap();
        let after = seal_key_in(&dir, wrap_identity, wrap_identity).expect("a fresh mint");
        assert_ne!(
            *first, *after,
            "an unusable key file was not replaced, so the stored bytes are not what is read"
        );
    }

    /// A DPAPI *call* that fails is an error, not a silent re-mint.
    ///
    /// The distinction [`seal_key_in`] draws: unreadable stored bytes cost a
    /// rebuilt cache and nothing else, but a machine whose DPAPI refuses
    /// cannot store a new key either, and minting over the old one there
    /// would throw away a cache that a healthy later session could have read.
    #[test]
    fn a_refusing_dpapi_is_an_error_rather_than_a_fresh_key() {
        fn refuse(_: &[u8]) -> Result<Vec<u8>, String> {
            Err("0x8009000b".to_string())
        }
        let dir = temp_dir_for("dpapi-refuses-the-key");
        assert!(seal_key_in(&dir, refuse, wrap_identity).is_err());
        assert!(
            !dir.join(KEY_FILE_NAME).exists(),
            "a key file was written even though wrapping it failed"
        );

        // POSITIVE CONTROL: the same call with a working `protect` succeeds,
        // so the error above is DPAPI's refusal and not some unrelated reason
        // this directory could never produce a key.
        assert!(seal_key_in(&dir, wrap_identity, wrap_identity).is_ok());
    }

    #[test]
    fn the_key_is_fetched_once_per_session_however_many_writes_there_are() {
        // Going back to the key file per write would be a read and a DPAPI
        // unwrap on every item edit, which is why the key is cached. Asserted
        // through a seam that counts its own calls in a thread-local rather
        // than a global.
        thread_local! {
            static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        fn counted(_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
            CALLS.with(|c| c.set(c.get() + 1));
            Ok(Zeroizing::new([7u8; 32]))
        }

        let dir = temp_dir_for("once");
        let cache = DiskCache::new(&dir, env(counted));
        let snap = snapshot();
        cache.acquire_key().unwrap();
        for _ in 0..5 {
            cache.write("fp", &facts_for(&snap.folders), &snap.items, &snap.folders).unwrap();
        }
        cache.load("fp");
        assert_eq!(CALLS.with(|c| c.get()), 1);
    }
}

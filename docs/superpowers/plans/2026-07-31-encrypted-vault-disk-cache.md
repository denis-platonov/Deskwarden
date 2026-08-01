# Encrypted Vault Disk Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optionally persist the vault snapshot to disk, encrypted under a random content key that is itself sealed by a Windows Hello (TPM-bound) credential, so deskwarden autofills the instant its tray icon appears and never pays `bw serve`'s ~8 s cold start on a restart.

**Architecture:** One new module, `vault_disk_cache`, owns the entire file: format, crypto, Hello key acquisition, DPAPI wrapping, atomic writes, expiry, and deletion. `VaultCache` holds an optional handle to it and is the only thing that calls it — no other call site persists, deletes, or reasons about the file, which is the property that keeps "exactly one place can be wrong" true. Everything is inert unless `Settings::cache_vault_to_disk` is on.

**Tech Stack:** Rust, Windows (`windows` crate: `KeyCredentialManager`, DPAPI via `session_store`), `aes-gcm`, `sha2`, `getrandom`, `zeroize`, `serde`/`serde_json`, `eframe`/`egui` 0.35, existing `directories` config dir.

**Spec:** `docs/superpowers/specs/2026-07-30-encrypted-vault-disk-cache-design.md` (commit 8227035). Every design decision in it is settled. **Do not re-litigate any of them.** In particular, master-password keying is a recorded dead end (the app never holds the master password — it starts unlocked from the DPAPI-wrapped session token), and the file deliberately **survives a lock**.

---

## Global Constraints

Copy these verbatim into your working memory; every task's requirements implicitly include them.

- **Build and test with `-j 2` only.** Higher parallelism hits a page-file limit on this machine.
  - `cargo test --manifest-path deskwarden/Cargo.toml -j 2`
  - `cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2`
- **Zero warnings.** The tree is warning-free today and must stay that way.
- **Off by default.** `cache_vault_to_disk` defaults to `false`. With default settings **no file is ever created** — and the test for that asserts on the filesystem, not on a flag.
- **Never call `KeyCredentialManager::RequestCreateAsync` with `KeyCredentialCreationOption::ReplaceExisting` from the cache path.** `hello::enroll` uses `ReplaceExisting` and that is correct *there*; doing it here would silently rotate the shared credential and destroy an existing quick-unlock enrollment. The cache uses `OpenAsync`, and only `RequestCreateAsync(FailIfExists)` when no credential is found.
- **Distinct KDF label.** The cache derives its key under `b"deskwarden vault cache aes key v1"`, never `hello.rs`'s `KDF_LABEL`. Sharing it would make the two sealed blobs cross-decryptable for no gain.
- **One write function, and it takes the sealed key as a parameter.** There must be no code path that constructs the file without a Hello-sealed key. If one is ever added "just for robustness", the UI copy becomes a false security claim.
- **Never describe the file as "secure"** in code comments, UI copy, or READMEs. Describe *what gates it*.
- **UI copy in Task 3 is a requirement, not a suggestion.** It is reproduced verbatim from the spec, which drafted it deliberately (it names what is in the file, states the survives-a-lock behaviour in bold because a reasonable person would assume the opposite, and names the residual attacker).
- **Windows Hello not enrolled → the setting is unavailable with an inline explanation.** Confirmed by the user on 2026-07-31 as the final product call. Do **not** build a DPAPI-only fallback variant.
- **Expiry is 7 days**, a named constant with its justification in a doc comment. It is not configurable.
- **Decryption buffers introduced here are `Zeroizing` from the start.** The separate, already-triaged leakiness of the in-memory snapshot (`login.totp` unwrapped, `passwordHistory`/`notes` riding the flattened `other` map, plaintext escaping via fills/clipboard/egui galley/serde buffers) is **out of scope and must not be "fixed" here** — see the `>>>` block in `.superpowers/sdd/progress.md`. `deskwarden/README.md` already states honestly what is and is not wiped.
- **Record each task and each review in `.superpowers/sdd/progress.md` as you go**, including what you deliberately did *not* do and why.
- Line numbers cited below are as of commit `d67b18e` plus the review-13 fix wave; treat them as approximate and locate by function name.

---

## File Structure

| File | Responsibility |
|---|---|
| `deskwarden/src/vault_disk_cache.rs` | **New.** The entire on-disk cache: header type, format encode/decode, AES-GCM seal/unseal, header validation (version/expiry/fingerprint/clock), Hello key acquisition with per-session caching, DPAPI wrapping, atomic write, load, delete. Pure functions split out so the whole format is testable without Hello hardware. |
| `deskwarden/src/vault_cache.rs` | Gains an optional `DiskCache` collaborator. Writes the file after every successful populate and every successful mutation; deletes it on the events that require it. The only caller of `vault_disk_cache`. |
| `deskwarden/src/settings.rs` | One new field, `cache_vault_to_disk: bool`. |
| `deskwarden/src/prefs_ui.rs` | The toggle, in General, below the backend toggle, with available/unavailable copy. |
| `deskwarden/src/login_ui.rs` | `check_bw_status_details_with_session` (so the fingerprint costs no extra `bw status` spawn); log-out deletes the file beside the existing `hello::unenroll()`. |
| `deskwarden/src/main.rs` | Startup: try the disk load before starting `bw serve`; refresh behind it. Re-auth deletes the file before repopulating. |
| `deskwarden/src/vault_window/mod.rs` | The toolbar pill reports the loaded snapshot's age until a sync succeeds *in this session*. |
| `deskwarden/src/lib.rs` | Declares the new module. |
| `README.md`, `deskwarden/README.md` | Security claims that become wrong the moment this ships. |

---

## Task 1: Disk cache format and crypto core (pure, no I/O, no Hello)

Everything in this task is a pure function over bytes. No filesystem, no Windows API, no Hello. That is deliberate: it makes the entire format — including the properties that are easiest to get wrong and hardest to test later — round-trippable on any machine with a fixed key, exactly as `hello.rs`'s tests do.

**Files:**
- Create: `deskwarden/src/vault_disk_cache.rs`
- Modify: `deskwarden/src/lib.rs` (add `pub mod vault_disk_cache;` in the existing alphabetical run of module declarations)
- Test: inline `#[cfg(test)] mod tests` in `deskwarden/src/vault_disk_cache.rs` (this crate tests inline everywhere; do not add a `tests/` directory)

**Interfaces:**
- Consumes: `crate::vault_bridge::{VaultItem, Folder}` (both `Serialize + Deserialize` with `#[serde(flatten)] other` catch-alls). Note `Folder`'s catch-all exists *because of this plan*: until it was added, `Folder` was `{ id, name }` with no catch-all, and serializing `Vec<Folder>` to disk would have silently dropped every other key `bw` sends on a folder (`organizationId`, `revisionDate`, ...) across the round trip. Nothing else in the crate PUTs a folder, so this file is the only path that makes it load-bearing — do not assume the catch-all is someone else's guarantee.
- Produces, for Tasks 2–6:
  - `pub struct CacheHeader { pub format_version: u32, pub written_at: u64, pub account_fingerprint: String, pub item_count: usize }`
  - `pub struct DiskSnapshot { pub items: Vec<VaultItem>, pub folders: Vec<Folder> }`
  - `pub enum RejectReason { UnknownVersion, Expired, ForeignAccount, FutureTimestamp, Malformed }`
  - `pub fn account_fingerprint(user_email: Option<&str>, server_url: Option<&str>) -> String`
  - `pub fn check_header(header: &CacheHeader, now_unix: u64, fingerprint: &str) -> Result<(), RejectReason>`
  - `pub(crate) fn encode_file(hello_key: &[u8; 32], header: &CacheHeader, snapshot: &DiskSnapshot) -> Result<Vec<u8>, String>`
  - `pub(crate) fn parse_header(bytes: &[u8]) -> Result<(CacheHeader, Parsed), String>` where `Parsed` carries the byte spans the body decode needs
  - `pub(crate) fn decode_body(hello_key: &[u8; 32], parsed: &Parsed) -> Result<DiskSnapshot, String>`
  - `pub const FORMAT_VERSION: u32 = 1;`
  - `pub const EXPIRY_SECS: u64 = 7 * 24 * 60 * 60;`

**Why `parse_header` and `decode_body` are separate, and why this split is load-bearing:** the header must be readable *without any Hello prompt*, so the app can decide a file is expired, foreign, or the wrong version and delete it unread. Prompting the user for a biometric and then throwing the file away would be an insult, and the spec's testing section makes "no key derivation was attempted for a doomed file" a behavioural assertion, not an incidental one. If you fold these into one function you will not be able to write that test.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/vault_disk_cache.rs` with only the test module and the imports it needs, so the tests fail to compile against absent items (that is the correct first failure here — Rust has no other way for a call to a non-existent function to "fail").

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::{Folder, LoginData, VaultItem};

    fn key(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    fn snapshot() -> DiskSnapshot {
        DiskSnapshot {
            items: vec![VaultItem {
                id: "1".to_string(),
                name: "Alpha".to_string(),
                fields: vec![],
                login: None,
                item_type: Some(1),
                folder_id: None,
                favorite: false,
                other: serde_json::Map::new(),
            }],
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
        let (header, _) = parse_header(&bytes).unwrap();

        let mut forged = header.clone();
        forged.written_at = 9_999_999;
        let forged_json = serde_json::to_vec(&forged).unwrap();
        let mut tampered = Vec::new();
        tampered.extend_from_slice(MAGIC);
        tampered.extend_from_slice(&(forged_json.len() as u32).to_le_bytes());
        tampered.extend_from_slice(&forged_json);
        // Splice the original sealed key and body back in unchanged.
        let (_, parsed) = parse_header(&bytes).unwrap();
        tampered.extend_from_slice(&(parsed.sealed_key.len() as u32).to_le_bytes());
        tampered.extend_from_slice(&parsed.sealed_key);
        tampered.extend_from_slice(&parsed.body);

        let (_, reparsed) = parse_header(&tampered).unwrap();
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
            assert!(
                parse_header(&bytes[..cut]).is_err() || decode_body(&key(7), &parse_header(&bytes[..cut]).unwrap().1).is_err(),
                "a file truncated to {cut} bytes was accepted"
            );
        }
    }

    #[test]
    fn a_foreign_magic_is_rejected() {
        assert!(parse_header(b"NOPEnotacachefileatall").is_err());
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
        assert_eq!(check_header(&header, 1_000, &other), Err(RejectReason::ForeignAccount));
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
        assert_eq!(account_fingerprint(None, None), account_fingerprint(None, None));
    }

    #[test]
    fn unknown_item_and_folder_fields_survive_a_disk_round_trip() {
        // This codebase has shipped a dropped-unknown-field bug four times,
        // in four different structs (LoginData/VaultItem, UriEntry,
        // VaultField, then Folder). The disk path must not become the fifth.
        //
        // BOTH halves of `DiskSnapshot` are exercised, deliberately. An
        // earlier draft of this plan asserted `VaultItem` only, and stated as
        // fact that `Folder` already had a catch-all -- which was FALSE. A
        // fidelity test that covers one of the two types it serializes cannot
        // catch a wrong premise about the other.
        let raw = r#"{
            "id": "1",
            "name": "Alpha",
            "type": 1,
            "fields": [],
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
        let snap = DiskSnapshot { items: vec![item], folders: vec![folder] };
        let bytes = encode_file(&key(7), &header_for(&snap, 1_000), &snap).unwrap();
        let (_, parsed) = parse_header(&bytes).unwrap();
        let opened = decode_body(&key(7), &parsed).unwrap();

        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after: serde_json::Value = serde_json::to_value(&opened.items[0]).unwrap();
        assert_eq!(before, after, "an item field was dropped by the disk round trip");

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
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_disk_cache`
Expected: compilation errors — `cannot find function encode_file`, `cannot find type DiskSnapshot`, etc. If anything compiles, you have not started from a genuinely absent implementation.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `deskwarden/src/vault_disk_cache.rs`.

```rust
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

use crate::vault_bridge::{Folder, VaultItem};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// File magic, so a foreign or truncated file is rejected before anything
/// tries to interpret its length prefixes.
pub(crate) const MAGIC: &[u8; 4] = b"DWVC";

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// The byte spans `decode_body` needs, produced by `parse_header` so the
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
/// a Hello seal, the setting's description becomes a false security claim --
/// see the spec's Risks section.
pub(crate) fn encode_file(
    hello_key: &[u8; 32],
    header: &CacheHeader,
    snapshot: &DiskSnapshot,
) -> Result<Vec<u8>, String> {
    let header_bytes =
        serde_json::to_vec(header).map_err(|e| format!("could not serialize the cache header: {e}"))?;

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

    let mut out = Vec::with_capacity(
        MAGIC.len() + 4 + header_bytes.len() + 4 + sealed_key.len() + body.len(),
    );
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
        let end = cursor.checked_add(n).ok_or("the cache file is truncated")?;
        let slice = bytes.get(*cursor..end).ok_or("the cache file is truncated")?;
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
        .ok_or("the cache file is truncated")?
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
```

Add to `deskwarden/src/lib.rs`, keeping the existing alphabetical order:

```rust
pub mod vault_disk_cache;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_disk_cache`
Expected: all tests in the module PASS.

Then: `cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2`
Expected: no warnings. If `Parsed`'s fields or `RejectReason::as_str` are flagged as unused, leave them — Task 2 consumes them within the same task cycle; do **not** silence with `#[allow(dead_code)]`. If the check is not clean at the end of Task 1, note it in the commit message and confirm Task 2 clears it.

- [ ] **Step 5: Run the full suite and commit**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2`
Expected: every pre-existing test still passes; the count is the previous total plus the 13 tests above.

```bash
git add deskwarden/src/vault_disk_cache.rs deskwarden/src/lib.rs
git commit -m "feat: vault disk cache format and crypto core

Pure, I/O-free core of the optional encrypted on-disk snapshot: header
type, length-prefixed layout, AES-256-GCM sealing of a random content key
under a caller-supplied Hello-derived key, and body encryption with the
plaintext header as AAD so written_at cannot be edited to defeat expiry.

parse_header deliberately performs no key derivation, so an expired,
foreign, or wrong-version file can be deleted unread without ever popping
a Windows Hello prompt for a file that is about to be thrown away."
```

---

## Task 2: Hello key acquisition, DPAPI wrapping, and the file on disk

**Files:**
- Modify: `deskwarden/src/vault_disk_cache.rs` (append; the pure core from Task 1 stays untouched)
- Test: inline in the same module

**Interfaces:**
- Consumes: Task 1's `CacheHeader`, `DiskSnapshot`, `RejectReason`, `check_header`, `encode_file`, `parse_header`, `decode_body`, `FORMAT_VERSION`; `crate::session_store::{protect, unprotect}`.
- Produces, for Tasks 3–6:
  - `pub struct DiskCache { /* path + per-session key state */ }`
  - `pub fn DiskCache::new(config_dir: &Path) -> DiskCache`
  - `pub fn DiskCache::path(&self) -> &Path`
  - `pub fn DiskCache::load(&self, fingerprint: &str) -> DiskCacheLoad`
  - `pub fn DiskCache::write(&self, fingerprint: &str, items: &[VaultItem], folders: &[Folder]) -> Result<(), String>`
  - `pub fn DiskCache::delete(&self) -> Result<(), String>`
  - `pub fn DiskCache::acquire_key(&self) -> Result<(), String>` (used by the Preferences toggle so enabling prompts immediately)
  - `pub enum DiskCacheLoad { Loaded { items: Vec<VaultItem>, folders: Vec<Folder>, written_at: SystemTime }, Absent, Rejected(RejectReason), Unavailable(String), Corrupt(String) }`
  - `pub fn hello_available() -> bool`

**Two decisions this task makes that the spec does not spell out. Both are load-bearing; record them in the progress ledger.**

1. **The Hello-derived key is acquired at most once per session and cached in memory (`Zeroizing<[u8; 32]>` behind a `Mutex`).** The spec requires a write after every successful populate *and every successful mutation*. Deriving the key per write would pop a biometric prompt on every item edit, which is unusable. The prompt therefore happens at most once per launch: on the startup load, or when the setting is turned on.
2. **A cancelled or failed acquisition makes the disk cache inert for the rest of the session, and is not retried.** The alternative — retrying at the next write — would pop a Hello prompt out of nowhere at an arbitrary moment, which is exactly the behaviour the spec forbids at startup ("do not retry, do not block the tray"). Inert means: no writes, the existing file is **left alone** (a cancelled biometric is a user decision, not a fault, and is not a reason to throw away their cache), and the next launch tries again. Log at `info`, not `error`.

**Distinct challenge as well as a distinct label.** The spec requires the KDF label to differ from `hello.rs`'s. This task also uses a distinct signing challenge. That is strictly additional domain separation at zero cost, and it is not a deviation from any decision the spec made — the spec named the label as the thing that must not be shared, not as the only thing that may differ.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `deskwarden/src/vault_disk_cache.rs`. Note what is and is not testable: Hello cannot be exercised in CI, so these tests drive every path that does *not* need it — which is every rejection path, and the ones that matter most.

```rust
    use std::path::PathBuf;

    fn temp_dir_for(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deskwarden-diskcache-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a file directly, bypassing Hello, so the load-side rejection
    /// paths can be tested on a machine with no Hello enrollment.
    fn write_file_with_key(dir: &std::path::Path, key: &[u8; 32], header: &CacheHeader, snap: &DiskSnapshot) {
        let inner = encode_file(key, header, snap).unwrap();
        let wrapped = crate::session_store::protect(&inner).unwrap();
        std::fs::write(dir.join(FILE_NAME), wrapped).unwrap();
    }

    #[test]
    fn an_absent_file_reports_absent_and_prompts_for_nothing() {
        let dir = temp_dir_for("absent");
        let cache = DiskCache::new(&dir);
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Absent));
    }

    #[test]
    fn an_expired_file_is_deleted_unread_and_no_key_is_ever_derived() {
        // The "no Hello prompt for a doomed file" property, asserted
        // behaviourally: the file is gone, and the session key was never
        // populated (which is what a Hello prompt would have set).
        let dir = temp_dir_for("expired");
        let snap = snapshot();
        let long_ago = now_unix().saturating_sub(EXPIRY_SECS + 3_600);
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: long_ago,
            account_fingerprint: "fp".to_string(),
            item_count: 1,
        };
        write_file_with_key(&dir, &key(7), &header, &snap);

        let cache = DiskCache::new(&dir);
        assert_eq!(cache.load("fp"), DiskCacheLoad::Rejected(RejectReason::Expired));
        assert!(!dir.join(FILE_NAME).exists(), "an expired file was left on disk");
        assert!(!cache.has_session_key(), "a doomed file triggered a key derivation");
    }

    #[test]
    fn a_foreign_account_file_is_deleted_unread() {
        let dir = temp_dir_for("foreign");
        let snap = snapshot();
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: now_unix(),
            account_fingerprint: "someone-else".to_string(),
            item_count: 1,
        };
        write_file_with_key(&dir, &key(7), &header, &snap);

        let cache = DiskCache::new(&dir);
        assert_eq!(cache.load("fp"), DiskCacheLoad::Rejected(RejectReason::ForeignAccount));
        assert!(!dir.join(FILE_NAME).exists());
        assert!(!cache.has_session_key());
    }

    #[test]
    fn an_unknown_version_is_deleted_unread() {
        let dir = temp_dir_for("version");
        let snap = snapshot();
        let header = CacheHeader {
            format_version: FORMAT_VERSION + 1,
            written_at: now_unix(),
            account_fingerprint: "fp".to_string(),
            item_count: 1,
        };
        write_file_with_key(&dir, &key(7), &header, &snap);

        let cache = DiskCache::new(&dir);
        assert_eq!(cache.load("fp"), DiskCacheLoad::Rejected(RejectReason::UnknownVersion));
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn a_garbage_file_is_deleted_rather_than_kept_forever() {
        // Same self-healing posture as hello::unlock_password: a blob that
        // can never be opened again is worse than no blob.
        let dir = temp_dir_for("garbage");
        std::fs::write(dir.join(FILE_NAME), b"this is not a DPAPI blob").unwrap();

        let cache = DiskCache::new(&dir);
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Rejected(RejectReason::Malformed)));
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn a_leftover_tmp_file_does_not_affect_a_load_and_is_cleaned_up() {
        let dir = temp_dir_for("tmp");
        std::fs::write(dir.join(TMP_FILE_NAME), b"half a write").unwrap();

        let cache = DiskCache::new(&dir);
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Absent));
        assert!(
            !dir.join(TMP_FILE_NAME).exists(),
            "a crash-leftover .tmp file was not cleaned up"
        );
    }

    #[test]
    fn delete_is_idempotent_and_succeeds_when_there_is_nothing_to_delete() {
        let dir = temp_dir_for("delete");
        let cache = DiskCache::new(&dir);
        assert!(cache.delete().is_ok());
        assert!(cache.delete().is_ok());
    }

    #[test]
    fn writing_without_a_session_key_is_a_no_op_not_a_plaintext_file() {
        // The single most important negative test in the feature: if the key
        // is unavailable (Hello cancelled), nothing must be written at all.
        // There is no unencrypted fallback path, by construction.
        let dir = temp_dir_for("nokey");
        let cache = DiskCache::new(&dir);
        assert!(!cache.has_session_key());
        let snap = snapshot();
        assert!(cache.write("fp", &snap.items, &snap.folders).is_err());
        assert!(
            !dir.join(FILE_NAME).exists(),
            "a file was written with no Hello-sealed key"
        );
    }

    #[test]
    fn a_write_then_load_round_trips_through_dpapi_with_an_injected_key() {
        // Exercises the real DPAPI + filesystem path (both work in a normal
        // test process) with the Hello step stubbed, which is the only part
        // that needs hardware.
        let dir = temp_dir_for("roundtrip");
        let cache = DiskCache::new(&dir);
        cache.set_session_key_for_test(key(7));

        let snap = snapshot();
        cache.write("fp", &snap.items, &snap.folders).unwrap();
        assert!(dir.join(FILE_NAME).exists());
        assert!(!dir.join(TMP_FILE_NAME).exists(), "the temp file survived a successful write");

        match cache.load("fp") {
            DiskCacheLoad::Loaded { items, folders, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(folders[0].name, "Work");
            }
            other => panic!("expected a loaded snapshot, got {other:?}"),
        }
    }

    #[test]
    fn a_file_written_under_a_different_key_is_treated_as_corrupt_and_deleted() {
        let dir = temp_dir_for("wrongkey");
        let snap = snapshot();
        let header = CacheHeader {
            format_version: FORMAT_VERSION,
            written_at: now_unix(),
            account_fingerprint: "fp".to_string(),
            item_count: 1,
        };
        write_file_with_key(&dir, &key(9), &header, &snap);

        let cache = DiskCache::new(&dir);
        cache.set_session_key_for_test(key(7));
        assert!(matches!(cache.load("fp"), DiskCacheLoad::Corrupt(_)));
        assert!(
            !dir.join(FILE_NAME).exists(),
            "an unopenable file was kept, so it would cost a Hello prompt on every launch forever"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_disk_cache`
Expected: compilation errors for `DiskCache`, `DiskCacheLoad`, `FILE_NAME`, `TMP_FILE_NAME`, `now_unix`, `has_session_key`, `set_session_key_for_test`.

- [ ] **Step 3: Write the implementation**

Append to `deskwarden/src/vault_disk_cache.rs`:

```rust
use crate::session_store;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::core::HSTRING;
use windows::Security::Credentials::{
    KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
};
use windows::Security::Cryptography::CryptographicBuffer;
use zeroize::Zeroize;

pub(crate) const FILE_NAME: &str = "vault-cache.bin";
pub(crate) const TMP_FILE_NAME: &str = "vault-cache.bin.tmp";

/// The same Hello key credential quick unlock uses. One credential, two
/// sealed blobs, separated by their derivation labels -- see `KDF_LABEL`
/// below and `hello::KDF_LABEL`.
const CREDENTIAL_NAME: &str = "deskwarden-quick-unlock";

/// Distinct from `hello.rs`'s challenge. The spec only requires the *label*
/// to differ; a distinct challenge is strictly additional separation at no
/// cost.
const CHALLENGE: &[u8] = b"deskwarden vault cache challenge v1";

/// Domain separation from quick unlock's key. Sharing `hello.rs`'s label
/// would make the two sealed blobs cross-decryptable, which is sloppy for
/// no gain.
const KDF_LABEL: &[u8] = b"deskwarden vault cache aes key v1";

/// The outcome of trying to load the file. Five distinct situations, five
/// variants -- deliberately not collapsed into `Result<Option<..>, ..>`,
/// because they call for different actions and this codebase has repeatedly
/// been bitten by distinct situations sharing one type. In particular
/// `Unavailable` must **leave the file alone** while `Rejected` and
/// `Corrupt` must delete it.
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

pub struct DiskCache {
    path: PathBuf,
    tmp_path: PathBuf,
    /// The Hello-derived key for this session.
    ///
    /// `None` means "not acquired yet or deliberately given up on". It is
    /// acquired at most once per launch -- on the startup load, or when the
    /// setting is switched on -- because the spec requires a rewrite after
    /// every populate *and every mutation*, and deriving per write would pop
    /// a biometric prompt on every item edit.
    ///
    /// A cancelled or failed acquisition is **not retried** for the rest of
    /// the session: retrying at the next write would pop a Hello prompt out
    /// of nowhere at an arbitrary moment. `given_up` records that, so the
    /// difference between "not tried yet" and "tried and refused" is in the
    /// state rather than inferred.
    key: Mutex<KeyState>,
}

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
    pub fn new(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join(FILE_NAME),
            tmp_path: config_dir.join(TMP_FILE_NAME),
            key: Mutex::new(KeyState::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Pops the Hello prompt if the key has not been acquired yet. Used by
    /// the Preferences toggle so that enabling the setting is itself the
    /// confirmation gesture.
    pub fn acquire_key(&self) -> Result<(), String> {
        let mut state = self.lock();
        Self::ensure_key(&mut state).map(|_| ())
    }

    fn ensure_key(state: &mut KeyState) -> Result<[u8; 32], String> {
        if let Some(key) = state.key {
            return Ok(key);
        }
        if state.given_up {
            return Err("Windows Hello was not available earlier in this session".to_string());
        }
        match hello_derived_key() {
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
        // A .tmp left by a crash mid-write is meaningless and must not be
        // mistaken for anything; clear it whenever we touch the directory.
        let _ = std::fs::remove_file(&self.tmp_path);

        let wrapped = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DiskCacheLoad::Absent,
            Err(e) => {
                log::warn!("could not read the vault cache file: {e}");
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        let inner = match session_store::unprotect(&wrapped) {
            Ok(bytes) => Zeroizing::new(bytes),
            Err(e) => {
                log::warn!("DPAPI could not unwrap the vault cache: {e}");
                self.delete_quietly(RejectReason::Malformed);
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        let (header, parsed) = match parse_header(&inner) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("the vault cache file is unusable: {e}");
                self.delete_quietly(RejectReason::Malformed);
                return DiskCacheLoad::Rejected(RejectReason::Malformed);
            }
        };

        if let Err(reason) = check_header(&header, now_unix(), fingerprint) {
            log::info!("discarding the vault cache: it is {}", reason.as_str());
            self.delete_quietly(reason);
            return DiskCacheLoad::Rejected(reason);
        }

        // Only now, with the file known to be ours and current, is it worth
        // asking the user for a biometric.
        let key = {
            let mut state = self.lock();
            match Self::ensure_key(&mut state) {
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
                let _ = std::fs::remove_file(&self.path);
                DiskCacheLoad::Corrupt(e)
            }
        }
    }

    /// Writes the snapshot. Atomic: a full write to `.tmp` followed by a
    /// rename over the target, so a crash mid-write cannot leave a truncated
    /// file whose corruption would cost a Hello prompt to discover.
    ///
    /// Errors if no session key is held. There is deliberately no fallback
    /// that writes without one.
    pub fn write(
        &self,
        fingerprint: &str,
        items: &[VaultItem],
        folders: &[Folder],
    ) -> Result<(), String> {
        let key = {
            let state = self.lock();
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
        let wrapped = session_store::protect(&inner)
            .map_err(|e| format!("DPAPI could not wrap the vault cache: {e}"))?;

        std::fs::write(&self.tmp_path, &wrapped)
            .map_err(|e| format!("could not write {}: {e}", self.tmp_path.display()))?;
        std::fs::rename(&self.tmp_path, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&self.tmp_path);
            format!("could not replace {}: {e}", self.path.display())
        })
    }

    /// Removes the file. Succeeds when there is nothing to remove -- callers
    /// use this on log out, on re-auth, and on disabling the setting, where
    /// "already absent" is the desired end state.
    pub fn delete(&self) -> Result<(), String> {
        let _ = std::fs::remove_file(&self.tmp_path);
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not delete {}: {e}", self.path.display())),
        }
    }

    fn delete_quietly(&self, reason: RejectReason) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "could not delete the vault cache that is {}: {e}",
                    reason.as_str()
                );
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, KeyState> {
        self.key.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn has_session_key(&self) -> bool {
        self.lock().key.is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_session_key_for_test(&self, key: [u8; 32]) {
        self.lock().key = Some(key);
    }
}

/// Runs the Hello-gated signature and derives this module's AES key from it.
/// This is the step that pops the OS verification dialog.
///
/// **Never `ReplaceExisting`.** `hello::enroll` uses it, correctly, because
/// a stale credential there has no blob to pair with. Here it would silently
/// rotate the shared credential and destroy an existing quick-unlock
/// enrollment. Open first; only create -- with `FailIfExists` -- when there
/// is nothing to open.
fn hello_derived_key() -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);

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

    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(&signature[..]);
    let key = Zeroizing::new(hasher.finalize().into());

    let signature_bytes: &mut [u8] = &mut signature;
    signature_bytes.zeroize();
    Ok(key)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_disk_cache`
Expected: all PASS. If `a_write_then_load_round_trips_through_dpapi_with_an_injected_key` fails on DPAPI, the test process lacks a user profile — do not weaken the test; report it.

- [ ] **Step 5: Verify the quick-unlock credential is not disturbed**

This is a manual check and it matters more than any test here: `ReplaceExisting` from this path would silently destroy an existing quick-unlock enrollment.

Read `hello_derived_key` in `vault_disk_cache.rs` and confirm by inspection that `KeyCredentialCreationOption::ReplaceExisting` appears **nowhere** in the file:

Run: `git grep -n "ReplaceExisting" -- deskwarden/src/`
Expected: exactly one hit, in `hello.rs`.

- [ ] **Step 6: Run the full suite and commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
git add deskwarden/src/vault_disk_cache.rs
git commit -m "feat: Hello-sealed, DPAPI-wrapped vault cache file on disk

DiskCache owns the whole file: load with header validation before any key
derivation, atomic write via .tmp + rename, and idempotent delete.

Two decisions the spec left to implementation, both recorded in the
progress ledger: the Hello-derived key is acquired at most once per
session (a rewrite happens after every mutation, so deriving per write
would pop a biometric prompt on every item edit), and a cancelled
acquisition makes the cache inert for the session rather than re-prompting
at an arbitrary later moment -- leaving the existing file alone, because a
cancelled biometric is a user decision and not a reason to discard a cache.

Opens the shared quick-unlock credential and only ever creates one with
FailIfExists; ReplaceExisting would silently rotate it and destroy an
existing quick-unlock enrollment."
```

---

## Task 3: The setting and its Preferences copy

**Files:**
- Modify: `deskwarden/src/settings.rs`
- Modify: `deskwarden/src/prefs_ui.rs`
- Test: inline in both

**Interfaces:**
- Consumes: `vault_disk_cache::hello_available`.
- Produces: `Settings::cache_vault_to_disk: bool`; `prefs_ui::run(settings: Settings, hello_available: bool) -> Settings` (signature change — the one call site is `main.rs`'s tray handler at ~line 605).

**The copy below is reproduced verbatim from the spec and is a requirement.** Do not paraphrase, shorten, or soften it. It names what is in the file rather than saying "vault data"; it states the survives-a-lock behaviour in the negative and in bold because that is the part a reasonable person assumes goes the other way; it names the residual attacker; and it does not use the word "secure".

- [ ] **Step 1: Write the failing tests**

In `deskwarden/src/settings.rs`, extend the existing test module:

```rust
    #[test]
    fn the_disk_cache_is_off_by_default() {
        assert!(!Settings::default().cache_vault_to_disk);
    }

    #[test]
    fn an_older_settings_file_parses_with_the_disk_cache_off() {
        // The partial-file property this struct's `#[serde(default)]`
        // already pins, extended to the new field: a settings.json written
        // by a build that predates this feature must not fail to parse, and
        // must not accidentally arrive with the cache enabled.
        let path = temp_path("partial-disk-cache");
        std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 5}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.cache_vault_to_disk);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, 5);
        let _ = std::fs::remove_file(&path);
    }
```

Also update the existing `settings_round_trip_through_disk` test's struct literal to set `cache_vault_to_disk: true`, so the round trip actually covers the new field:

```rust
        let written = Settings {
            keep_backend_running: false,
            auto_lock_minutes: 5,
            cache_vault_to_disk: true,
        };
```

In `deskwarden/src/prefs_ui.rs`, add a test module (the file has none today):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_available_description_states_the_survives_a_lock_behaviour() {
        // The wording is the requirement here, not an implementation
        // detail: this is the text a user reads before accepting a security
        // tradeoff. These assertions exist so a future edit that quietly
        // drops the uncomfortable half of the sentence fails a test.
        let text = disk_cache_description(true);
        assert!(text.contains("usernames, passwords, notes and two-factor secrets"));
        assert!(text.contains("not"));
        assert!(text.contains("when your vault locks"));
        assert!(text.contains("log out"));
        assert!(text.contains("7 days"));
        assert!(text.contains("TPM"));
        assert!(
            !text.to_lowercase().contains("secure"),
            "the copy must describe what gates the file, not call it secure"
        );
    }

    #[test]
    fn the_unavailable_description_explains_why_and_offers_no_weaker_option() {
        let text = disk_cache_description(false);
        assert!(text.starts_with("Unavailable"));
        assert!(text.contains("Windows Hello"));
        assert!(text.contains("Sign-in options"));
        assert!(
            !text.to_lowercase().contains("secure"),
            "the copy must describe what gates the file, not call it secure"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 settings prefs_ui`
Expected: `settings.rs` fails to compile (`cache_vault_to_disk` is not a field); `prefs_ui` fails to compile (`disk_cache_description` not found).

- [ ] **Step 3: Add the setting**

In `deskwarden/src/settings.rs`, inside `pub struct Settings`, after `auto_lock_minutes`:

```rust
    /// Whether the vault snapshot is persisted to disk, encrypted under a
    /// Windows Hello-sealed key.
    ///
    /// Off by default, and every behaviour it enables is inert while it is
    /// off. On, the file survives a lock (that is the point -- it exists to
    /// survive a restart), is deleted on log out, and expires after 7 days.
    pub cache_vault_to_disk: bool,
```

and in `impl Default`:

```rust
            cache_vault_to_disk: false,
```

- [ ] **Step 4: Add the Preferences row**

In `deskwarden/src/prefs_ui.rs`:

```rust
/// The description shown under the disk-cache toggle.
///
/// Split out as a pure function so the wording -- which is the requirement
/// here, not an implementation detail -- is asserted by tests rather than
/// buried in an eframe closure where nothing can reach it.
fn disk_cache_description(hello_available: bool) -> &'static str {
    if hello_available {
        "Deskwarden opens instantly after a restart and autofill works the moment it starts, \
         instead of waiting about 8 seconds for the Bitwarden backend.\n\n\
         The copy contains your usernames, passwords, notes and two-factor secrets. It is \
         encrypted with a key that Windows Hello keeps in this PC's TPM chip, so a copied disk \
         cannot be read on another machine. It is not deleted when your vault locks — only when \
         you log out, or after 7 days. Anyone who can run programs as you on this PC and pass \
         Windows Hello can read it."
    } else {
        "Unavailable — needs Windows Hello.\n\n\
         This copy is protected by a key held in your PC's TPM chip, which only Windows Hello \
         can release. Without Hello there is no such key, and Deskwarden will not store your \
         vault on disk under weaker protection than this setting describes. Set Hello up in \
         Windows Settings → Accounts → Sign-in options."
    }
}
```

Change `run`'s signature and add the row below the backend toggle:

```rust
pub fn run(settings: Settings, hello_available: bool) -> Settings {
```

and inside the `CentralPanel` closure, after the `keep_backend_running` row:

```rust
                ui.add_space(14.0);

                if hello_available {
                    current.cache_vault_to_disk = toggle_row(
                        ui,
                        "Keep an encrypted copy of your vault on this PC",
                        disk_cache_description(true),
                        current.cache_vault_to_disk,
                    );
                } else {
                    // Not a disabled toggle with a tooltip: the reason has to
                    // be readable without hovering, because it is the answer
                    // to "why can't I turn this on". A DPAPI-only fallback
                    // under this same label was considered and rejected --
                    // the TPM binding is the entire value of the setting, and
                    // offering a weaker file under copy that promises one
                    // would be a straightforwardly misleading security claim.
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(
                            theme::semibold("Keep an encrypted copy of your vault on this PC", 13.0)
                                .color(theme::TEXT_FAINT),
                        );
                        ui.label(
                            RichText::new(disk_cache_description(false))
                                .size(11.0)
                                .color(theme::TEXT_FAINT),
                        );
                    });
                }
```

The window is 300 px tall today and now has a second row with a long description. Increase `with_inner_size` to `[520.0, 460.0]`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 settings prefs_ui`
Expected: PASS.

- [ ] **Step 6: Update the call site and wire the enable/disable side effects**

In `deskwarden/src/main.rs`'s tray handler (search for `prefs_ui::run`, ~line 605), replace the call and the persistence block:

```rust
                let hello_available = vault_disk_cache::hello_available();
                let edited = prefs_ui::run(settings.clone(), hello_available);
                if edited != settings {
                    // Turning the disk cache ON prompts Hello immediately --
                    // that prompt doubles as the confirmation gesture, so
                    // there is no separate modal -- and writes the first file
                    // from the snapshot already in memory. Turning it OFF
                    // deletes the file *before* the settings are saved, so a
                    // failed delete cannot leave a file behind under a
                    // setting that says there is none.
                    if edited.cache_vault_to_disk && !settings.cache_vault_to_disk {
                        match cache.enable_disk_persistence() {
                            Ok(()) => log::info!("encrypted vault disk cache enabled"),
                            Err(e) => {
                                log::warn!("could not enable the vault disk cache: {e}");
                                // Do not persist a setting whose machinery
                                // could not be started: it would render as on
                                // while nothing was ever written.
                                continue;
                            }
                        }
                    } else if !edited.cache_vault_to_disk && settings.cache_vault_to_disk {
                        if let Err(e) = cache.disable_disk_persistence() {
                            // The user asked for the file to be gone and it
                            // is not. This is the one disk-cache failure the
                            // spec says to surface rather than only log.
                            log::error!("could not delete the vault cache file: {e}");
                            crate::dispatch::message_box(
                                "Deskwarden",
                                &format!(
                                    "The encrypted vault copy could not be deleted:\n\n{e}\n\n\
                                     It is still on disk at that location."
                                ),
                            );
                        }
                    }
                    settings = edited;
                    if let Err(e) = settings.save(&settings_path) {
                        log::warn!("could not save settings: {e}");
                    }
                }
```

`cache.enable_disk_persistence` / `disable_disk_persistence` land in Task 4. Until then this will not compile — that is expected and is why Tasks 3 and 4 are adjacent. If you are executing tasks one at a time with a build gate between them, do Step 6 at the **start of Task 4** instead and commit Task 3 without it. Check `dispatch::message_box`'s real name and signature before using it (`git grep -n "fn message_box" deskwarden/src/`); the picker's inline-error idiom is preferred anywhere a window is already open, but this failure happens with no window on screen.

- [ ] **Step 7: Commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
git add deskwarden/src/settings.rs deskwarden/src/prefs_ui.rs
git commit -m "feat: cache_vault_to_disk setting and its Preferences row

Off by default; an older settings.json still parses with it off. The
description is the spec's drafted copy verbatim, split into a pure
function so the wording is test-asserted rather than buried in an eframe
closure -- it names what is in the file, states the survives-a-lock
behaviour, and never calls the file secure.

Without Windows Hello the row renders as unavailable with the reason
inline. A DPAPI-only variant under the same label was considered and
rejected: the TPM binding is the setting's entire value."
```

---

## Task 4: `VaultCache` owns the file

Everything about the file lives behind `VaultCache` from here on. No other call site persists, deletes, or reasons about it — that is the property that keeps "exactly one place can be wrong" true, and it is the same reason every vault write was routed through this type in the first place.

**Files:**
- Modify: `deskwarden/src/vault_cache.rs`
- Modify: `deskwarden/src/main.rs` (construct the cache with the config dir and fingerprint; Task 3 Step 6's tray block)
- Modify: `deskwarden/src/login_ui.rs` (`check_bw_status_details_with_session`)
- Test: inline in `vault_cache.rs`

**Interfaces:**
- Consumes: `vault_disk_cache::{DiskCache, DiskCacheLoad, account_fingerprint}`.
- Produces:
  - `VaultCache::with_disk_cache(bridge: VaultBridge, disk: DiskCache, fingerprint: String, enabled: bool) -> VaultCache`
  - `VaultCache::load_from_disk(&self) -> DiskCacheLoad` (populates the snapshot on `Loaded`)
  - `VaultCache::enable_disk_persistence(&self) -> Result<(), String>`
  - `VaultCache::disable_disk_persistence(&self) -> Result<(), String>`
  - `VaultCache::forget_disk_copy(&self) -> Result<(), String>` (re-auth and log out)
  - `VaultCache::loaded_from_disk_at(&self) -> Option<SystemTime>`
  - `login_ui::check_bw_status_details_with_session(session_token: Option<&str>) -> BwStatusDetails`

**Two behaviours that must not be confused, and the tests below exist to keep them apart:** `clear()` (lock, quit) empties memory and **leaves the file**; `forget_disk_copy()` (re-auth, log out) deletes the file. If a future edit makes `clear()` delete, the feature stops working — the whole point is surviving a restart, and quit calls `clear()`.

- [ ] **Step 1: Write the failing tests**

Append to `vault_cache.rs`'s test module:

```rust
    use crate::vault_disk_cache::DiskCache;

    fn temp_config_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deskwarden-vaultcache-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cache_with_disk(url: String, dir: &std::path::Path, enabled: bool) -> VaultCache {
        let disk = DiskCache::new(dir);
        disk.set_session_key_for_test([7u8; 32]);
        VaultCache::with_disk_cache(VaultBridge::new(url), disk, "fp".to_string(), enabled)
    }

    fn mock_list(server: &mut mockito::ServerGuard) -> (mockito::Mock, mockito::Mock) {
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        (items, folders)
    }

    #[test]
    fn with_the_setting_off_no_file_is_ever_created() {
        // Asserted on the filesystem, not on a flag: "off by default" has to
        // mean nothing is written, not that a flag says so.
        let dir = temp_config_dir("disabled");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let cache = cache_with_disk(server.url(), &dir, false);
        cache.populate().unwrap();
        let item = cache.items().into_iter().next().unwrap();
        let _ = cache.update_item(&item);

        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(entries.is_empty(), "a file was created with the setting off: {entries:?}");
    }

    #[test]
    fn a_successful_populate_writes_the_file() {
        let dir = temp_config_dir("populate-writes");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        assert!(dir.join("vault-cache.bin").exists());
    }

    #[test]
    fn a_failed_populate_does_not_write_a_file() {
        let dir = temp_config_dir("failed-populate");
        let mut server = mockito::Server::new();
        let _f = server.mock("GET", "/list/object/folders").with_status(500).create();

        let cache = cache_with_disk(server.url(), &dir, true);
        assert!(cache.populate_with(vec![]).is_err());
        assert!(!dir.join("vault-cache.bin").exists());
    }

    #[test]
    fn a_successful_mutation_rewrites_the_file() {
        let dir = temp_config_dir("mutation-rewrites");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);
        let _d = server.mock("DELETE", "/object/item/1").with_status(200).create();

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        cache.delete_item("1").unwrap();

        // Reload the file and confirm the deletion is in it, rather than
        // just checking the file's mtime changed.
        let reloaded = cache_with_disk(server.url(), &dir, true);
        match reloaded.load_from_disk() {
            crate::vault_disk_cache::DiskCacheLoad::Loaded { items, .. } => {
                let ids: Vec<String> = items.into_iter().map(|i| i.id).collect();
                assert_eq!(ids, vec!["2".to_string()]);
            }
            other => panic!("expected a loaded snapshot, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_mutation_does_not_rewrite_the_file() {
        let dir = temp_config_dir("failed-mutation");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);
        let _d = server.mock("DELETE", "/object/item/1").with_status(500).create();

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        assert!(cache.delete_item("1").is_err());

        let reloaded = cache_with_disk(server.url(), &dir, true);
        match reloaded.load_from_disk() {
            crate::vault_disk_cache::DiskCacheLoad::Loaded { items, .. } => {
                assert_eq!(items.len(), 2, "a failed delete was persisted anyway");
            }
            other => panic!("expected a loaded snapshot, got {other:?}"),
        }
    }

    #[test]
    fn clear_empties_memory_and_leaves_the_file() {
        // The load-bearing lifecycle rule. Lock and quit both call clear();
        // if either deleted the file the feature would stop working, since
        // surviving a restart is the entire point.
        let dir = temp_config_dir("clear-keeps-file");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        cache.clear();

        assert!(cache.items().is_empty());
        assert!(!cache.is_populated());
        assert!(dir.join("vault-cache.bin").exists(), "clear() deleted the file");
    }

    #[test]
    fn forget_disk_copy_deletes_the_file() {
        let dir = temp_config_dir("forget");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        cache.forget_disk_copy().unwrap();
        assert!(!dir.join("vault-cache.bin").exists());
    }

    #[test]
    fn disabling_persistence_deletes_the_file_and_stops_writing() {
        let dir = temp_config_dir("disable");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let cache = cache_with_disk(server.url(), &dir, true);
        cache.populate().unwrap();
        assert!(dir.join("vault-cache.bin").exists());

        cache.disable_disk_persistence().unwrap();
        assert!(!dir.join("vault-cache.bin").exists());

        cache.populate().unwrap();
        assert!(
            !dir.join("vault-cache.bin").exists(),
            "a populate after disabling wrote the file back"
        );
    }

    #[test]
    fn loading_from_disk_populates_the_snapshot_and_records_its_age() {
        let dir = temp_config_dir("load-populates");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let writer = cache_with_disk(server.url(), &dir, true);
        writer.populate().unwrap();

        let reader = cache_with_disk(server.url(), &dir, true);
        assert!(!reader.is_populated());
        assert!(matches!(
            reader.load_from_disk(),
            crate::vault_disk_cache::DiskCacheLoad::Loaded { .. }
        ));
        assert!(reader.is_populated());
        assert_eq!(reader.items().len(), 2);
        assert!(reader.loaded_from_disk_at().is_some());
    }

    #[test]
    fn a_rejected_load_leaves_the_cache_unpopulated_and_records_no_age() {
        let dir = temp_config_dir("load-rejected");
        let server = mockito::Server::new();
        let cache = cache_with_disk(server.url(), &dir, true);
        assert!(matches!(
            cache.load_from_disk(),
            crate::vault_disk_cache::DiskCacheLoad::Absent
        ));
        assert!(!cache.is_populated());
        assert!(cache.loaded_from_disk_at().is_none());
    }

    #[test]
    fn a_backend_populate_clears_the_from_disk_age() {
        // Once real data has arrived in this session, the snapshot is no
        // longer "loaded from a file written N hours ago", and the toolbar
        // pill must stop saying so.
        let dir = temp_config_dir("age-cleared");
        let mut server = mockito::Server::new();
        let (_i, _f) = mock_list(&mut server);

        let writer = cache_with_disk(server.url(), &dir, true);
        writer.populate().unwrap();

        let reader = cache_with_disk(server.url(), &dir, true);
        reader.load_from_disk();
        assert!(reader.loaded_from_disk_at().is_some());
        reader.populate().unwrap();
        assert!(reader.loaded_from_disk_at().is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_cache`
Expected: compilation errors for `with_disk_cache`, `load_from_disk`, `forget_disk_copy`, `disable_disk_persistence`, `loaded_from_disk_at`.

- [ ] **Step 3: Implement in `vault_cache.rs`**

Replace the module doc's "**Memory only, by design.**" paragraph — it is about to be false — with:

```rust
//! **Memory by default; optionally also on disk.** Nothing is written to
//! disk unless `Settings::cache_vault_to_disk` is on, in which case the same
//! snapshot is persisted through `vault_disk_cache`, encrypted under a
//! Windows Hello-sealed key. `clear` drops the in-memory copy -- `main`
//! calls it whenever the snapshot might outlive the session it was built
//! from (locking, re-authenticating into a possibly different account,
//! quitting) -- and deliberately **leaves the file alone**: surviving a lock
//! and a restart is the entire point of the file, and quit calls `clear`
//! too. Deleting the file is a separate, explicit act
//! (`forget_disk_copy`), used on re-authentication and log out.
//!
//! **This is the only module that touches that file.** No call site
//! persists, deletes, or reasons about it directly, for the same reason
//! every write already routes through here: there is exactly one place that
//! can be wrong.
```

Add to the struct and constructor:

```rust
pub struct VaultCache {
    bridge: VaultBridge,
    snapshot: Mutex<Snapshot>,
    disk: Option<DiskCache>,
    /// Guards `enabled` and `loaded_from_disk_at` together with the disk
    /// file itself, so a populate racing a "disable" cannot write the file
    /// back after the delete.
    disk_state: Mutex<DiskState>,
    fingerprint: String,
}

#[derive(Default)]
struct DiskState {
    enabled: bool,
    /// When the currently-held snapshot was written to disk, if it came from
    /// the file. Cleared the moment real data arrives from the backend in
    /// this session -- see `loaded_from_disk_at`'s use in the toolbar pill.
    loaded_from_disk_at: Option<SystemTime>,
}
```

`VaultCache::new` keeps working for the existing tests and any non-persisting use:

```rust
    pub fn new(bridge: VaultBridge) -> Self {
        Self {
            bridge,
            snapshot: Mutex::new(Snapshot::default()),
            disk: None,
            disk_state: Mutex::new(DiskState::default()),
            fingerprint: String::new(),
        }
    }

    pub fn with_disk_cache(
        bridge: VaultBridge,
        disk: DiskCache,
        fingerprint: String,
        enabled: bool,
    ) -> Self {
        Self {
            bridge,
            snapshot: Mutex::new(Snapshot::default()),
            disk: Some(disk),
            disk_state: Mutex::new(DiskState {
                enabled,
                loaded_from_disk_at: None,
            }),
            fingerprint,
        }
    }
```

The single persist point, called after every successful populate and mutation:

```rust
    /// Rewrites the file from the current snapshot. Best-effort by design:
    /// the in-memory cache is authoritative and the app is fully functional
    /// without the file, so a disk-full or antivirus-locked write is a
    /// `warn` and nothing more. Never surfaces a modal.
    ///
    /// Deliberately takes no arguments and reads the snapshot itself: every
    /// caller is "the snapshot just changed", and passing the data in would
    /// create a second place that could pass the wrong data.
    fn persist(&self) {
        let Some(disk) = self.disk.as_ref() else { return };
        let state = self.lock_disk();
        if !state.enabled {
            return;
        }
        let (items, folders) = {
            let snapshot = self.lock();
            if !snapshot.populated {
                return;
            }
            (snapshot.items.clone(), snapshot.folders.clone())
        };
        if let Err(e) = disk.write(&self.fingerprint, &items, &folders) {
            log::warn!("could not write the encrypted vault cache: {e}");
        }
    }
```

> **Lock-ordering rule, and it is not optional:** `persist` takes `disk_state` and then `snapshot`, and releases `snapshot` before writing. Every other method must take them in that same order, or not hold both. Do not call `persist` while holding the snapshot lock — the write is ~1 MB of AES plus a file rename, and holding the snapshot mutex across it would block the UI thread's `items()` for the duration.

Call `persist()` at the end of the success path of: `populate_with` (which `populate` already delegates to), `create_item`, `update_item`, `set_app_match`, `delete_item`, `create_folder`, `update_folder`, `delete_folder`. In each case after the snapshot mutation and after the guard is dropped. Example for `delete_item`:

```rust
    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)?;
        {
            let mut snapshot = self.lock();
            if snapshot.populated {
                snapshot.items.retain(|i| i.id != id);
            }
        }
        self.persist();
        Ok(())
    }
```

`populate_with` also clears the from-disk age, because the snapshot is no longer the file's.

> **Read the current code before editing this one.** As of commit `48cff27` the populate path
> carries an **epoch**: `populate` and `populate_with` capture `snapshot.epoch` before fetching
> and delegate to a private `populate_with_at_epoch`, which discards its result (returning
> `Ok(())` with the snapshot left empty) if `clear()` bumped the epoch mid-flight. That guard
> exists to stop a detached populate resurrecting a previous account's snapshot after a lock,
> so **the persist and the age-clear must live on the branch that actually adopted the
> snapshot, not on the discard branch** — persisting a discarded populate would write the
> stale account's vault to disk, which is the same bug one layer down.

```rust
    fn populate_with_at_epoch(&self, items: Vec<VaultItem>, epoch: u64) -> Result<(), VaultError> {
        let folders = self.bridge.list_folders()?;
        {
            let mut snapshot = self.lock();
            if snapshot.epoch != epoch {
                // ... existing discard branch, unchanged: return Ok early,
                // and do NOT persist or clear the from-disk age.
                return Ok(());
            }
            snapshot.items = items;
            snapshot.folders = folders;
            snapshot.populated = true;
        }
        self.lock_disk().loaded_from_disk_at = None;
        self.persist();
        Ok(())
    }
```

Add a test that pins this: a populate whose epoch was bumped mid-flight must leave **no file on
disk**, not just an empty snapshot. `vault_cache.rs` already has
`a_populate_whose_epoch_was_bumped_mid_flight_leaves_the_cache_empty` to model it on.

The remaining new methods:

```rust
    /// Populates the snapshot from the file, if there is a usable one. The
    /// caller gets the full outcome rather than a bool, because "no file",
    /// "rejected and deleted", "Hello declined" and "corrupt" call for
    /// different logging and different next steps.
    pub fn load_from_disk(&self) -> DiskCacheLoad {
        let Some(disk) = self.disk.as_ref() else {
            return DiskCacheLoad::Absent;
        };
        if !self.lock_disk().enabled {
            return DiskCacheLoad::Absent;
        }
        let outcome = disk.load(&self.fingerprint);
        if let DiskCacheLoad::Loaded {
            items,
            folders,
            written_at,
        } = &outcome
        {
            {
                let mut snapshot = self.lock();
                snapshot.items = items.clone();
                snapshot.folders = folders.clone();
                snapshot.populated = true;
            }
            self.lock_disk().loaded_from_disk_at = Some(*written_at);
        }
        outcome
    }

    /// When the currently-held snapshot was written to disk, if it came from
    /// the file and nothing has refreshed it from the backend since. The
    /// vault window's toolbar pill reads this so it reports an age instead
    /// of claiming a sync that never happened in this session.
    pub fn loaded_from_disk_at(&self) -> Option<SystemTime> {
        self.lock_disk().loaded_from_disk_at
    }

    /// Turns persistence on: acquires the Hello key (this is the prompt the
    /// user sees, and it doubles as the confirmation gesture) and writes the
    /// first file from the snapshot already in memory.
    pub fn enable_disk_persistence(&self) -> Result<(), String> {
        let disk = self
            .disk
            .as_ref()
            .ok_or_else(|| "no config directory for the vault cache".to_string())?;
        disk.acquire_key()?;
        self.lock_disk().enabled = true;
        self.persist();
        Ok(())
    }

    /// Turns persistence off and deletes the file. The flag is cleared
    /// *before* the delete, so a populate racing this cannot write the file
    /// back immediately after it is removed.
    pub fn disable_disk_persistence(&self) -> Result<(), String> {
        self.lock_disk().enabled = false;
        match self.disk.as_ref() {
            Some(disk) => disk.delete(),
            None => Ok(()),
        }
    }

    /// Deletes the file while leaving persistence enabled -- used on
    /// re-authentication (any master-password prompt, which is a superset of
    /// a master-password change) and on log out. The next successful
    /// populate writes a fresh one.
    pub fn forget_disk_copy(&self) -> Result<(), String> {
        self.lock_disk().loaded_from_disk_at = None;
        match self.disk.as_ref() {
            Some(disk) => disk.delete(),
            None => Ok(()),
        }
    }

    fn lock_disk(&self) -> std::sync::MutexGuard<'_, DiskState> {
        self.disk_state.lock().unwrap_or_else(|e| e.into_inner())
    }
```

Leave `clear()` exactly as it is. Add a comment above it so a future edit does not "helpfully" add a delete:

```rust
    /// Drops the in-memory snapshot. Called on lock, on re-auth, and on
    /// quit.
    ///
    /// **Does not touch the disk file, deliberately.** Surviving a lock and
    /// a restart is the entire reason that file exists, and quit calls this
    /// too. Deleting it is `forget_disk_copy`, which the re-auth and log-out
    /// paths call explicitly.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_cache`
Expected: PASS, including every pre-existing test in that module unchanged.

- [ ] **Step 5: Construct the cache with a disk cache in `main.rs`**

`main.rs` builds the cache at ~line 269 with `VaultCache::new(vault.clone())`. It needs the account fingerprint, which comes from `bw status`. **Do not add a second `bw status` spawn**: the startup path already runs one via `check_bw_status_with_session`.

In `login_ui.rs`, beside the existing `check_bw_status_details`:

```rust
/// [`check_bw_status_details`] for a specific session token. The startup
/// path needs both the status *and* the account identity from a single
/// `bw status` spawn -- the fingerprint the vault disk cache keys its file
/// on comes from here -- so this exists rather than calling `bw status`
/// twice.
pub fn check_bw_status_details_with_session(session_token: Option<&str>) -> BwStatusDetails {
    bw_status_stdout(session_token)
        .map(|out| parse_bw_status_details(&out))
        .unwrap_or(BwStatusDetails {
            status: BwStatus::Unauthenticated,
            user_email: None,
            server_url: None,
        })
}
```

In `main.rs`, replace the `check_bw_status_with_session(Some(&token))` call in the session-token block so the details are captured, keeping the same control flow (`details.status` where `BwStatus` was matched before), and thread a `account_details: BwStatusDetails` binding out of that block. Then:

```rust
    let disk_cache = vault_disk_cache::DiskCache::new(&config_dir);
    let fingerprint = vault_disk_cache::account_fingerprint(
        account_details.user_email.as_deref(),
        account_details.server_url.as_deref(),
    );
    let cache = Arc::new(VaultCache::with_disk_cache(
        vault.clone(),
        disk_cache,
        fingerprint,
        settings.cache_vault_to_disk,
    ));
```

Note the re-auth path (`reauthenticate`) can change which account is signed in, and it happens *after* this binding. Task 5 handles that: re-auth deletes the file, and the fingerprint is refreshed. If you cannot do that cleanly here, leave it for Task 5 and note it — do not paper over it.

- [ ] **Step 6: Apply Task 3 Step 6's tray block**

It compiles now. Run the full suite and `cargo check --all-targets`.

- [ ] **Step 7: Commit**

```bash
git add deskwarden/src/vault_cache.rs deskwarden/src/main.rs deskwarden/src/login_ui.rs
git commit -m "feat: route the disk cache entirely through VaultCache

One persist point, called after every successful populate and mutation;
one delete method for the events that require it. clear() (lock, quit)
deliberately leaves the file, and says so in a comment, because surviving
a restart is the whole point and quit calls clear() too.

The account fingerprint comes from a new
check_bw_status_details_with_session, reusing the bw status spawn startup
already performs rather than adding a second one."
```

---

## Task 5: Startup loads from disk, and re-auth/log-out delete it

**Files:**
- Modify: `deskwarden/src/main.rs`
- Modify: `deskwarden/src/login_ui.rs` (log out)
- Test: inline where a pure decision can be extracted; see Step 1

**Interfaces:**
- Consumes: Task 4's `VaultCache::{load_from_disk, forget_disk_copy}`, `DiskCacheLoad`.
- Produces: `fn startup_plan(disk_outcome_was_loaded: bool, keep_backend_running: bool) -> StartupPlan` and `pub enum StartupPlan { BackendFirst, CacheFirst }`.

**Extract the decision into a pure function and unit-test it.** Logic inside `main`'s startup block is exactly as untestable as logic inside an eframe closure, and untested seams are where every defect on this plan was found. The decision is small; test it anyway.

- [ ] **Step 1: Write the failing test**

In `main.rs`'s existing test module:

```rust
    #[test]
    fn startup_reads_the_disk_cache_before_starting_the_backend_when_it_is_usable() {
        // The entire point of the feature: with a usable file, the tray and
        // autofill come up without paying bw serve's cold start.
        assert_eq!(startup_plan(true, false), StartupPlan::CacheFirst);
        assert_eq!(startup_plan(true, true), StartupPlan::CacheFirst);
    }

    #[test]
    fn without_a_usable_disk_cache_startup_is_exactly_todays_path() {
        assert_eq!(startup_plan(false, true), StartupPlan::BackendFirst);
        assert_eq!(startup_plan(false, false), StartupPlan::BackendFirst);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 startup_plan`
Expected: `cannot find function startup_plan`.

- [ ] **Step 3: Implement the decision and restructure startup**

```rust
/// Whether startup can skip the `bw serve` cold start.
///
/// `keep_backend_running` is taken but deliberately does not change the
/// answer: even in default mode, the cache being usable means the tray and
/// autofill do not have to *wait* for the backend. The backend is still
/// started immediately afterwards under the normal policy -- it is the
/// blocking readiness wait that is skipped, not the backend itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPlan {
    /// Today's path: start the backend, wait for readiness, populate.
    BackendFirst,
    /// Present the cached snapshot now; reconcile the backend behind it.
    CacheFirst,
}

fn startup_plan(disk_cache_loaded: bool, _keep_backend_running: bool) -> StartupPlan {
    if disk_cache_loaded {
        StartupPlan::CacheFirst
    } else {
        StartupPlan::BackendFirst
    }
}
```

In `main`, immediately after the cache is constructed and **before** `start_backend`:

```rust
    // Try the encrypted disk cache before anything spawns `bw serve`. A
    // usable file means the match engine is built and autofill is live in
    // milliseconds instead of after a ~8 s cold start. Every failure mode
    // here falls through to exactly today's path; none of them is a reason
    // the app cannot start.
    let disk_load = cache.load_from_disk();
    match &disk_load {
        DiskCacheLoad::Loaded { written_at, .. } => log::info!(
            "vault loaded from the encrypted disk cache (written {:?} ago)",
            written_at.elapsed().unwrap_or_default()
        ),
        DiskCacheLoad::Absent => {}
        DiskCacheLoad::Rejected(reason) => {
            log::info!("the encrypted disk cache was discarded: it is {}", reason.as_str())
        }
        DiskCacheLoad::Unavailable(e) => {
            log::info!("not using the encrypted disk cache this session: {e}")
        }
        DiskCacheLoad::Corrupt(e) => log::warn!("the encrypted disk cache was corrupt: {e}"),
    }

    let plan = startup_plan(
        matches!(disk_load, DiskCacheLoad::Loaded { .. }),
        settings.keep_backend_running,
    );
```

Then branch. `BackendFirst` is the code that exists today, unchanged — do not rewrite it. `CacheFirst`:

```rust
        StartupPlan::CacheFirst => {
            // The snapshot is already in the cache. Build the match engine
            // from it so autofill works right now, then let the ordinary
            // idle policy reconciliation in the main loop start `bw serve`
            // if the settings call for it. Nothing here blocks on the
            // backend, and nothing here waits for readiness -- that is the
            // whole feature.
            let entries = match_entries(&cache.items());
            log::info!("match engine loaded with {} app match(es) from the disk cache", entries.len());
            engine.rebuild(&entries);
            None
        }
```

with `bw_serve_child` starting as `None` on this path. The existing idle reconciliation in the main loop already brings the backend up when `keep_backend_running` is on and takes it down when it is not, so **do not add a second start here** — that reconciliation is where Critical 2 of the Task-5 review was fixed, and duplicating it is how it regressed before.

A refresh behind the cached snapshot is *not* added here: the main loop's existing policy reconciliation plus the tray Sync item already cover it, and the spec's staleness section requires the pill to state the age rather than silently claiming freshness (Task 6). Adding an automatic startup sync would put a network round-trip back into the startup path the feature exists to shorten. **Record this as a deliberate omission in the progress ledger.**

- [ ] **Step 4: Delete the file on re-authentication**

Every path that prompts for the master password must delete the file before repopulating — a superset of the master-password-change case we cannot detect directly, and free, because the backend is already up and the snapshot is already being rebuilt at that moment.

Find every call site of `reauthenticate(` in `main.rs` (there are several; `git grep -n "reauthenticate(" deskwarden/src/main.rs`). Rather than editing each one, put the deletion inside the existing `cache.clear()` companion at each re-auth site — every one of them already calls `cache.clear()`. Add immediately after each `cache.clear()` **that is a re-auth**, not the lock/quit ones:

```rust
        if let Err(e) = cache.forget_disk_copy() {
            log::warn!("could not delete the encrypted vault cache on re-authentication: {e}");
        }
```

Then verify by inspection that the `clear()` calls on **lock** and on **quit** did *not* gain this. Confirm with:

Run: `git grep -n "forget_disk_copy\|cache.clear()" deskwarden/src/main.rs`
Expected: every `forget_disk_copy` sits at a re-auth site; the lock and quit `clear()` calls stand alone.

The account can change across a re-auth, which makes the fingerprint stale. The file is deleted at that moment so nothing foreign can be *read*, but a subsequent write would key the new account's data under the old fingerprint and it would be rejected on next launch. Either refresh the fingerprint after re-auth (add `VaultCache::set_fingerprint(&self, fp: String)` and call it with a fresh `check_bw_status_details_with_session`) or, if that turns out to ripple further than it looks, leave it and **record it explicitly as a known gap** — a wrong fingerprint costs one wasted cold start, never wrong data, because `check_header` rejects it.

- [ ] **Step 5: Delete the file on log out**

In `login_ui.rs`'s `LoginAction::LogOut` handler, beside the existing `hello::unenroll()`. `login_ui` has no `VaultCache`, so this needs the deletion to reach it. Prefer passing a callback or a `&DiskCache` into `login_ui::run` over reaching for a global. Check the existing signature first; if threading it through is disproportionate, the alternative is to delete the file in `main.rs` immediately after `login_ui::run` returns from a logged-out state — but that is a second place that reasons about the file, so prefer the first option and justify whichever you pick in the ledger.

```rust
                            // A sealed master password for an account the
                            // CLI no longer knows is a liability, not a
                            // feature -- and a decrypted dump of that same
                            // account's vault is a larger one. Both go with
                            // the account.
                            hello::unenroll();
```

- [ ] **Step 6: Run the full suite and commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
git add deskwarden/src/main.rs deskwarden/src/login_ui.rs
git commit -m "feat: start from the encrypted disk cache; delete it on re-auth and log out

With a usable file the match engine is built and autofill is live before
bw serve is touched at all; the main loop's existing idle policy
reconciliation still brings the backend up or down as the settings say, so
no second start was added. Every disk-cache failure falls through to
today's path.

Re-authentication deletes the file before repopulating -- a superset of the
master-password change we cannot detect directly -- and log out deletes it
beside the existing hello::unenroll(). Lock and quit deliberately do not."
```

---

## Task 6: The pill must not claim a sync that did not happen

This is the highest-likelihood defect in the whole feature, because it is a bug this codebase has already shipped once in a narrower form (final review Important 1, fixed in 292a55c: `spawn_vault_load` raced the backend cold start, `populate()` got connection-refused, and the window shipped the pre-sync snapshot while the toolbar pill read "Synced just now"). A disk cache makes it strictly worse: the gap between what the pill claims and what is on screen becomes days rather than one sync interval.

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs`
- Test: inline in the same module

**Interfaces:**
- Consumes: `VaultCache::loaded_from_disk_at`.
- Produces: `fn sync_pill_text(sync_in_progress: bool, sync_status: Option<&Result<(), String>>, last_sync_at: Option<Duration>, loaded_from_disk_age: Option<Duration>) -> (PillTone, String)` and `pub enum PillTone { Neutral, Good, Bad }`.

**`last_sync_at` stays a per-session value.** Do not repurpose it to mean "when the file was written". They answer different questions, and conflating them is how the pill starts lying again.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_disk_loaded_snapshot_reports_its_age_not_a_sync() {
        let (tone, text) = sync_pill_text(false, None, None, Some(Duration::from_secs(3 * 3600)));
        assert_eq!(tone, PillTone::Neutral);
        assert_eq!(text, "Loaded from cache · 3 h old");
    }

    #[test]
    fn a_failed_refresh_never_upgrades_a_disk_loaded_pill_to_a_success() {
        // The exact shape of the bug this codebase already shipped once.
        let err = Err("connection refused".to_string());
        let (tone, text) = sync_pill_text(false, Some(&err), None, Some(Duration::from_secs(86_400 * 2)));
        assert_eq!(tone, PillTone::Bad);
        assert!(text.contains("2 d old"), "the age vanished behind a bare failure: {text}");
    }

    #[test]
    fn a_successful_sync_in_this_session_replaces_the_age_wording() {
        let ok = Ok(());
        let (tone, text) = sync_pill_text(false, Some(&ok), Some(Duration::ZERO), None);
        assert_eq!(tone, PillTone::Good);
        assert_eq!(text, "Synced just now");
    }

    #[test]
    fn a_successful_sync_wins_even_if_the_snapshot_started_on_disk() {
        // Belt and braces: `loaded_from_disk_at` is cleared by a successful
        // populate, but the pill must not depend on that ordering.
        let ok = Ok(());
        let (_, text) = sync_pill_text(false, Some(&ok), Some(Duration::ZERO), Some(Duration::from_secs(9_999)));
        assert_eq!(text, "Synced just now");
    }

    #[test]
    fn in_flight_beats_everything() {
        let (tone, text) = sync_pill_text(true, None, None, Some(Duration::from_secs(60)));
        assert_eq!(tone, PillTone::Neutral);
        assert_eq!(text, "Syncing…");
    }

    #[test]
    fn a_memory_only_session_is_unchanged() {
        assert_eq!(sync_pill_text(false, None, None, None), (PillTone::Neutral, "Sync".to_string()));
    }

    #[test]
    fn cache_age_wording_units() {
        assert_eq!(cache_age_text(Duration::from_secs(30)), "just written");
        assert_eq!(cache_age_text(Duration::from_secs(600)), "10 min old");
        assert_eq!(cache_age_text(Duration::from_secs(3 * 3600)), "3 h old");
        assert_eq!(cache_age_text(Duration::from_secs(50 * 3600)), "2 d old");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 sync_pill_text cache_age_text`
Expected: `cannot find function sync_pill_text`.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillTone {
    Neutral,
    Good,
    Bad,
}

/// The disk cache's age, in the largest unit that reads honestly. Coarser
/// than `synced_ago_text`'s minutes-only wording on purpose: that value is
/// per-session and resets on every auto-sync-on-open, so it never reaches
/// hours, while this one is exactly the number that can be days old and is
/// the whole reason the wording exists.
fn cache_age_text(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        "just written".to_string()
    } else if secs < 3600 {
        format!("{} min old", secs / 60)
    } else if secs < 86_400 {
        format!("{} h old", secs / 3600)
    } else {
        format!("{} d old", secs / 86_400)
    }
}

/// The toolbar pill's tone and wording.
///
/// Pure, and tested directly, because this is the seam that has already
/// produced one shipped bug of exactly the "correct-looking UI over wrong
/// data" class: it claimed "Synced just now" over a snapshot the sync had
/// failed to refresh. With a disk cache behind it, the same lie can now be
/// days wide rather than one sync interval, so the rule is explicit here:
/// **a successful sync in this session is the only thing that may say
/// "Synced"**, and a failure never removes the age from view.
fn sync_pill_text(
    sync_in_progress: bool,
    sync_status: Option<&Result<(), String>>,
    last_sync_at: Option<Duration>,
    loaded_from_disk_age: Option<Duration>,
) -> (PillTone, String) {
    if sync_in_progress {
        return (PillTone::Neutral, "Syncing…".to_string());
    }
    match sync_status {
        Some(Ok(())) => (
            PillTone::Good,
            format!("Synced {}", synced_ago_text(last_sync_at.unwrap_or(Duration::ZERO))),
        ),
        Some(Err(_)) => match loaded_from_disk_age {
            Some(age) => (PillTone::Bad, format!("Sync failed · {}", cache_age_text(age))),
            None => (PillTone::Bad, "Sync failed".to_string()),
        },
        None => match loaded_from_disk_age {
            Some(age) => (
                PillTone::Neutral,
                format!("Loaded from cache · {}", cache_age_text(age)),
            ),
            None => (PillTone::Neutral, "Sync".to_string()),
        },
    }
}
```

Replace the inline `let (dot, label) = if sync_in_progress { .. }` block in the toolbar (around `mod.rs:549`) with a call to `sync_pill_text`, mapping `PillTone` to the existing colors (`Neutral → theme::TEXT_GHOST`, `Good → theme::BLUE`, `Bad → theme::ERROR`) at the call site. Pass `cache.loaded_from_disk_at().and_then(|t| t.elapsed().ok())` as the last argument.

**Verify the extraction is faithful and not a rewrite.** The single most repeated failure on this plan is a pure function that is correct while its live call site composes it differently. Compare the old block and the new call line by line before moving on, and confirm the `.clicked()` behaviour below it is untouched.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 sync_pill`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
git add deskwarden/src/vault_window/mod.rs
git commit -m "feat: the toolbar pill reports a cached snapshot's real age

Extracted as a pure sync_pill_text and tested directly: a snapshot loaded
from disk reads 'Loaded from cache · 3 h old', a failed refresh keeps the
age in view instead of collapsing to a bare failure, and only a sync that
actually succeeded in this session may say 'Synced'.

last_sync_at stays a per-session value; it is deliberately not repurposed
to mean when the file was written."
```

---

## Task 7: README security claims

Both READMEs make claims that were corrected *this month* for the in-memory cache and become wrong the moment this ships. Both must read true whether the setting is on or off.

**Files:**
- Modify: `README.md`
- Modify: `deskwarden/README.md`

- [ ] **Step 1: Fix the root README**

The "Nothing here re-implements vault security" bullet currently ends "reads are served from an in-memory snapshot of what the CLI returned, held only while the vault is unlocked." *Held only while the vault is unlocked* stops being true. Name the opt-in disk cache, with the Hello/TPM protection and the survives-a-lock behaviour stated rather than implied.

In the memory-footprint paragraph further down, add that with the disk cache on, save-memory mode no longer pays a cold start on the first operation after a restart — that is the combination the two features were built for.

- [ ] **Step 2: Fix the crate README**

Two places. The intro paragraph carries the same "held only while the vault is unlocked" phrasing. The **Security notes** list needs a new bullet, in the same flat register as the DPAPI and job-object bullets, covering: off by default; what the file contains; Hello/TPM sealing; DPAPI as the outer layer; survives lock; deleted on log out; expires after 7 days.

Keep the existing bullet about the in-memory snapshot not being individually zeroized. It is still true, and it is exactly the kind of caveat this list should keep.

Neither README may describe the file as "secure". Describe what gates it.

- [ ] **Step 3: Verify and commit**

Run: `git grep -n "held only while the vault is unlocked"`
Expected: no hits.

Run: `git grep -in "secure" README.md deskwarden/README.md`
Expected: no new hit describing this file.

```bash
git add README.md deskwarden/README.md
git commit -m "docs: state the encrypted disk cache honestly in both READMEs

'held only while the vault is unlocked' stops being true with the disk
cache on, in both files. Replaced with what actually gates the file, and
the survives-a-lock behaviour stated rather than implied."
```

---

## Task 8: Final whole-branch review

**Not optional, and not a formality.** Thirteen independent reviews of this codebase have found thirteen real defects, including two Criticals introduced *by* a fix or a redesign. Dispatch a fresh subagent with no prior context in this session.

- [ ] **Step 1: Run the full verification**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
```

Both clean, zero warnings. Record the test count.

- [ ] **Step 2: Dispatch the review**

Give the reviewer: the spec, this plan, the commit range, and these specific things to hunt.

- **The recurring shape.** A change that is correct in isolation but does not reach the behaviour it claims. For every claim, find the real call site and trace what a user actually sees.
- **Is there any path that writes the file without a Hello-sealed key?** If one exists, the setting's description is a false security claim. `encode_file` takes the key as a parameter and `write` errors without one — verify nothing routes around either.
- **Does `ReplaceExisting` appear anywhere outside `hello.rs`?** It would silently destroy an existing quick-unlock enrollment.
- **Do both sealed blobs remain openable after the cache path acquires its key?** The two KDF labels must genuinely differ.
- **Does the AAD binding actually hold at the live call site**, not just in the unit test — i.e. can an edited `written_at` reach a successful decode?
- **Does `clear()` still leave the file, on every path, including quit?** And does every re-auth path delete it?
- **Can the pill claim "Synced" over data that was not refreshed in this session?** This is the highest-likelihood defect in the feature and a bug this codebase has already shipped once.
- **With the setting off, is a file ever created?** Assert on the filesystem.
- **Does a doomed file (expired/foreign/wrong version) ever pop a Hello prompt?**
- **Startup with the cache unusable must be exactly today's path**, and autofill must still work with the backend fully down.
- **Deliberately out of scope, do not flag:** the leaky zeroization of the in-memory snapshot (recorded decision, README states it honestly), `ureq`'s per-syscall read timeout, and review-6's backend-op generation Minors.

- [ ] **Step 3: Triage, fix, and record**

Fix Criticals and Importants. Triage Minors explicitly — defer with a reason or fix. Append the review's verdict, every finding, every fix, and every deliberate deferral to `.superpowers/sdd/progress.md`, in the same style as the existing `### Independent review of …` entries.

---

## Self-Review

**Spec coverage.** §1 what gates the file → Task 1 (crypto) + Task 2 (Hello). §2 file format, header-as-AAD, hashed fingerprint, atomic write, serde fidelity → Task 1 + Task 2. §3 lifecycle (survives lock, deleted on log out, 7-day expiry, master-password change via any re-auth prompt) → Task 1 (`check_header`), Task 4 (`clear` vs `forget_disk_copy`), Task 5 (re-auth, log out). §4 Hello unavailable → Task 3, per the user's confirmed product call. §5 settings and UI copy → Task 3. Data flow → Tasks 4 and 5. Staleness → Task 6. Error handling → Task 2's `DiskCacheLoad` variants (`Unavailable` leaves the file, `Rejected`/`Corrupt` delete it, write failure is a `warn`, deletion failure is surfaced in Task 3's tray block). Risks → Task 8's review checklist. Testing → every bullet in the spec's testing section has a named test in Tasks 1, 2, 4, or 6, **except** the full lifecycle table, which is covered by the individual tests in Task 4 rather than as one table; that is a deliberate simplification, since the events are reached through different call paths and a single table would have to fake them.

**Known open items handed to the implementer rather than decided here.** Two, both flagged inline: whether the account fingerprint is refreshed after a re-auth that changes accounts (Task 5 Step 4 — worst case is one wasted cold start, never wrong data), and how the log-out deletion reaches `login_ui` (Task 5 Step 5 — thread it through, or justify the alternative in the ledger).

**Type consistency.** `DiskCacheLoad` is named identically in Tasks 2, 4, and 5. `account_fingerprint` takes `Option<&str>` in Tasks 1, 2, and 4. `check_header(header, now_unix, fingerprint)` has one argument order throughout. `DiskCache::write(fingerprint, items, folders)` matches `VaultCache::persist`'s call. `prefs_ui::run` gains its second parameter in Task 3 and its only call site is updated in the same task.

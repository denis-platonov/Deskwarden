//! Named API keys: what one is allowed to do, and for how long.
//!
//! `docs/superpowers/specs/2026-08-27-service-auth-design.md` is the design.
//! Three rules carry it, and each is here because failing the other way is a
//! silent grant rather than a visible breakage.
//!
//! # Default deny, including for things this build has never heard of
//!
//! An empty scope set permits nothing. So does a scope whose subject this
//! build cannot parse -- which is what an **older build reading a newer
//! file** sees, and the direction that failure has to fall is not a matter of
//! taste: a subject read as "grant everything" would be a permission nobody
//! wrote, in a file the owner believes they configured.
//!
//! [`Subject::Unrecognised`] exists so that case has a value rather than an
//! error. The record still loads, its other scopes still work, and the one it
//! cannot understand grants nothing.
//!
//! # The hash is fast on purpose
//!
//! Keys are stored as `SHA-256(key)`, not as an Argon2id or PBKDF2 digest,
//! and that is the opposite of what [`crate::rest::crypto`] does two modules
//! away. The difference is the secret, not the storage:
//!
//! A master password is low-entropy and guessable, so the defence is to make
//! each guess expensive. An API key from [`crate::service_token::mint`] is
//! **256 bits of OS randomness** -- there is no candidate list to walk, and a
//! slow KDF would buy nothing while costing a stretch on every request to a
//! service that is meant to answer in a loop.
//!
//! What the hash does buy is real: a key store that is read -- backed up,
//! synced, copied off a disk -- does not hand over working credentials.
//!
//! # Expiry is a question about now
//!
//! [`find`] takes the current time and answers for that moment. Nothing here
//! caches a decision about whether a key is live, because a service that
//! stays up for a week would otherwise honour a key that expired on the
//! second day.

use crate::vault_bridge::ItemKind;
use serde::{Deserialize, Serialize};

/// What a scope is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// Every item in the vault.
    All,
    /// Every item of one kind.
    Category(ItemKind),
    /// One item, by id.
    Item(String),
    /// A subject this build does not understand, kept verbatim.
    ///
    /// **Permits nothing**, and never matches anything -- see the module
    /// doc. Held rather than dropped so that a newer build's file survives a
    /// round trip through an older one instead of being silently rewritten
    /// without the scope the owner set.
    Unrecognised(String),
}

/// Read or write. Two flags with no hierarchy: neither implies the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

/// One grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    #[serde(with = "subject_as_string")]
    pub subject: Subject,
    pub access: Access,
}

/// One API key, as stored.
///
/// **Deliberately not `Debug`.** It holds no plaintext key -- that is the
/// point of `hash` -- but it holds a name and a scope set that describe what
/// a credential opens, and [`crate::debug_leak_guard`] exists because things
/// that can be printed get printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRecord {
    /// What the owner called it, so a key can be recognised and revoked.
    pub name: String,
    /// `SHA-256(key)`, hex. Never the key.
    pub hash: String,
    pub created_unix: u64,
    /// `None` means no expiry, and the screen that mints keys says so out
    /// loud rather than leaving a blank field to be read as "safe".
    pub expires_unix: Option<u64>,
    pub scopes: Vec<Scope>,
}

/// How long a session minted from the master password lasts.
///
/// Fifteen minutes, and short on purpose. This is the credential handed
/// to an interactive tool the owner is sitting in front of; a script that
/// wants to run at 3am gets a named key with an expiry the owner chose,
/// which is the whole reason both paths exist. A session that lasted a day
/// would quietly become the second thing, without the name, the scope or
/// the revoke button.
pub const SESSION_LIFETIME_SECS: u64 = 900;

/// A session record minted from a successful master-password check.
///
/// Full access, because the master password is full access -- scoping it
/// would be theatre against a caller who could mint another session a
/// second later. What bounds it is time, not scope.
///
/// Returns the record to store and the key to hand back exactly once. The
/// record holds only the hash, like every other key.
#[must_use]
pub fn session_record(now_unix: u64, random: fn() -> [u8; 32]) -> (KeyRecord, String) {
    let key = crate::service_token::mint(random);
    let secret = key.expose().to_string();
    let record = KeyRecord {
        name: "Signed in with the master password".to_string(),
        hash: hash_key(&secret),
        created_unix: now_unix,
        expires_unix: Some(now_unix + SESSION_LIFETIME_SECS),
        scopes: vec![
            Scope { subject: Subject::All, access: Access::Read },
            Scope { subject: Subject::All, access: Access::Write },
        ],
    };
    (record, secret)
}

/// Why a key was not minted.
///
/// Every arm is a refusal the owner has to be *told about*, so each carries
/// its own sentence rather than a code the screen has to translate. A
/// refusal that reached the screen as "could not mint a key" would be one
/// the owner cannot act on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MintRefusal {
    /// The name was empty, or nothing but whitespace.
    NoName,
    /// A key by that name already exists.
    DuplicateName(String),
    /// The expiry chosen is already at or before now.
    ExpiryAlreadyPassed,
}

impl MintRefusal {
    /// What the owner is told, in their words rather than the code's.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoName => {
                "Give the key a name. It is how you will recognise it later, and how you \
                 will revoke it."
                    .to_string()
            }
            Self::DuplicateName(name) => format!(
                "There is already a key called \u{201c}{name}\u{201d}. Pick another name, or \
                 revoke that one first."
            ),
            Self::ExpiryAlreadyPassed => {
                "That expiry has already passed, so the key would be dead the moment it was \
                 made. Choose a date in the future, or no expiry at all."
                    .to_string()
            }
        }
    }
}

/// Mints a named key: the record to store, and the plaintext to show **once**.
///
/// The secret is [`crate::service_token::mint`]'s -- 256 bits of OS
/// randomness, hex -- and the record keeps only its hash, exactly as
/// [`session_record`] does. Nothing here writes anything; [`save`] does that,
/// so a caller that refuses at the confirmation screen leaves no trace.
///
/// **Three refusals, and each is a mistake the owner would not otherwise
/// notice:**
///
/// - *No name.* A key with a blank name cannot be told apart in the list, and
///   [`revoke`] takes a name, so a nameless key is one that cannot be
///   revoked through the screen that made it.
/// - *An expiry already at or before `now_unix`.* [`find`] treats expiry
///   inclusively, so such a key is dead on arrival -- and it would look
///   perfectly fine in the list, and its script would fail with a 401 that
///   says nothing about why. A key minted dead is a bug that surfaces at 3am
///   in somebody else's log.
/// - *A duplicate name.* Two keys called the same thing make the list
///   ambiguous and [`revoke`] arbitrary; the owner would revoke one of them
///   and believe they had revoked the other.
///
/// Names are compared **ignoring case and surrounding whitespace**, because
/// `Backup` and `backup ` are one name to the person reading the list. The
/// same comparison is used by [`revoke`], so a name that was refused as a
/// duplicate is exactly a name that revokes the record it collided with.
pub fn mint(
    name: String,
    expires_unix: Option<u64>,
    scopes: Vec<Scope>,
    now_unix: u64,
    random: fn() -> [u8; 32],
    existing: &[KeyRecord],
) -> Result<(KeyRecord, String), MintRefusal> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(MintRefusal::NoName);
    }
    if let Some(clash) = existing.iter().find(|record| same_name(&record.name, &name)) {
        return Err(MintRefusal::DuplicateName(clash.name.clone()));
    }
    if let Some(expiry) = expires_unix {
        if expiry <= now_unix {
            return Err(MintRefusal::ExpiryAlreadyPassed);
        }
    }
    let secret = crate::service_token::mint(random).expose().to_string();
    let record = KeyRecord {
        name,
        hash: hash_key(&secret),
        created_unix: now_unix,
        expires_unix,
        scopes,
    };
    Ok((record, secret))
}

/// Whether two key names are the same name to the owner.
fn same_name(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

/// Removes every record called `name`. Answers whether anything went.
///
/// The answer is the point: a screen that says "revoked" after removing
/// nothing has told the owner their script is locked out when it is not.
///
/// *Every* match rather than the first, and the same name comparison
/// [`mint`] refuses duplicates with -- so a file that was hand-edited into
/// holding two keys of one name cannot leave one of them behind, still
/// working, after the owner revoked what they saw.
pub fn revoke(records: &mut Vec<KeyRecord>, name: &str) -> bool {
    let before = records.len();
    records.retain(|record| !same_name(&record.name, name));
    records.len() != before
}

/// Writes the key store.
///
/// **Atomic**, in [`crate::vault_disk_cache`]'s idiom: a full write to a
/// `.tmp` sibling followed by a rename over the target. A crash between the
/// two leaves the previous store intact, where a truncated `settings.json`
/// costs a preference and a truncated key store silently revokes every key
/// the owner has ever minted -- including the ones their unattended scripts
/// are holding.
///
/// The `.tmp` file is removed if the rename fails, so a failed write does
/// not leave a second copy of the store lying beside the first.
pub fn save(path: &std::path::Path, records: &[KeyRecord]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(records)
        .map_err(|e| format!("could not encode the API keys: {e}"))?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, json).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace {}: {e}", path.display())
    })
}

/// The key store's file name, beside `settings.json`.
///
/// **One constant, because two spellings is a screen that mints keys into a
/// file the service never reads.** Each half would work perfectly on its
/// own, and the owner would be told a key exists while every request using
/// it was refused. It lives here rather than in either caller because this
/// module owns what is in the file; the callers only own where the config
/// directory is.
pub const KEY_STORE_FILE_NAME: &str = "service-keys.json";

/// The key store's path under `config_dir`.
#[must_use]
pub fn key_store_path(config_dir: &std::path::Path) -> std::path::PathBuf {
    config_dir.join(KEY_STORE_FILE_NAME)
}

/// The key file, or an empty list.
///
/// **An unreadable or malformed file reads as no keys**, which is the same
/// direction every other decision here falls: a corrupt key store must not
/// be a key store that grants anything. It is logged, because silently
/// having no keys and silently having a broken file look identical from the
/// outside and only one of them is the owner's doing.
#[must_use]
pub fn load(path: &std::path::Path) -> Vec<KeyRecord> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        // Absent is the ordinary case before any key is minted, and is not
        // worth a warning.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            log::error!("the API key file could not be read ({e}); treating it as empty");
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(records) => records,
        Err(e) => {
            log::error!("the API key file could not be parsed ({e}); treating it as empty");
            Vec::new()
        }
    }
}

/// The wall clock as a Unix timestamp, for the one caller that has to ask.
///
/// Everything in this module takes `now_unix` as a parameter so that expiry
/// is testable without waiting; this is the single place the real clock is
/// read, and it lives here so that the request loop does not grow its own.
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `SHA-256(key)`, hex. See the module doc for why this is not a slow KDF.
#[must_use]
pub fn hash_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The live record matching `presented`, at time `now_unix`.
///
/// Every record is compared, and the comparison is
/// [`crate::service_token::matches`] against the hashes rather than `==`:
/// this runs on an unauthenticated request, and a comparison that stopped at
/// the first differing byte would leak a stored hash one character at a time.
///
/// Expiry is checked in the same pass, and it is **inclusive** -- a key whose
/// `expires_unix` is exactly now is expired. A boundary that had to be
/// guessed at is a boundary somebody guesses wrong.
#[must_use]
pub fn find<'a>(records: &'a [KeyRecord], presented: &str, now_unix: u64) -> Option<&'a KeyRecord> {
    // Delegates, so the liveness and comparison rules exist once. Two copies
    // would be two places for an expiry check to be forgotten.
    find_index(records, presented, now_unix).map(|index| &records[index])
}

/// [`find`], as an index.
///
/// `service_api` needs to know WHICH key authorised a request, so the body
/// it builds can be narrowed to what that key may see. Returning the index
/// rather than the record keeps `Answer` a plain value with no lifetime,
/// which is what lets every routing test compare answers with `assert_eq!`.
#[must_use]
pub fn find_index(records: &[KeyRecord], presented: &str, now_unix: u64) -> Option<usize> {
    let presented_hash = hash_key(presented);
    records.iter().position(|record| {
        let live = match record.expires_unix {
            Some(expiry) => now_unix < expiry,
            None => true,
        };
        live && crate::service_token::hashes_match(&record.hash, &presented_hash)
    })
}

/// Whether `record` may do `access` to `subject`.
///
/// The subject asked about is the one the request is for: a category for a
/// list, one id for one item. A grant matches when it covers that subject --
/// `All` covers everything, a `Category` covers its own kind, an `Item`
/// covers its own id, and [`Subject::Unrecognised`] covers nothing at all.
#[must_use]
pub fn permits(record: &KeyRecord, access: Access, subject: &Subject) -> bool {
    record.scopes.iter().any(|scope| scope.access == access && covers(&scope.subject, subject))
}

/// Whether a granted subject covers a requested one.
fn covers(granted: &Subject, requested: &Subject) -> bool {
    match (granted, requested) {
        // A subject this build cannot read grants nothing, and nothing can
        // be requested under it either. Both arms, so a future subject
        // cannot be reached by asking for it by name.
        (Subject::Unrecognised(_), _) | (_, Subject::Unrecognised(_)) => false,
        (Subject::All, _) => true,
        (Subject::Category(granted), Subject::Category(requested)) => granted == requested,
        (Subject::Item(granted), Subject::Item(requested)) => granted == requested,
        // A category grant does not answer a question about one id, because
        // this function is not given the item and cannot know its kind. The
        // caller that knows both asks about the category.
        _ => false,
    }
}

/// `Subject` on the wire: one string, so an unknown one is a value rather
/// than a parse error that would take the whole file down with it.
mod subject_as_string {
    use super::{ItemKind, Subject};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    fn kind_name(kind: ItemKind) -> Option<&'static str> {
        match kind {
            ItemKind::Login => Some("login"),
            ItemKind::SecureNote => Some("note"),
            ItemKind::Card => Some("card"),
            ItemKind::Identity => Some("identity"),
            ItemKind::SshKey => Some("sshkey"),
            // A kind this build does not know is not a category anyone can
            // be granted.
            ItemKind::Unknown(_) => None,
        }
    }

    fn kind_of(name: &str) -> Option<ItemKind> {
        match name {
            "login" => Some(ItemKind::Login),
            "note" => Some(ItemKind::SecureNote),
            "card" => Some(ItemKind::Card),
            "identity" => Some(ItemKind::Identity),
            "sshkey" => Some(ItemKind::SshKey),
            _ => None,
        }
    }

    pub fn serialize<S: Serializer>(subject: &Subject, out: S) -> Result<S::Ok, S::Error> {
        let text = match subject {
            Subject::All => "all".to_string(),
            Subject::Category(kind) => match kind_name(*kind) {
                Some(name) => format!("category:{name}"),
                None => "unrecognised".to_string(),
            },
            Subject::Item(id) => format!("item:{id}"),
            // Round-trips verbatim, so an older build does not rewrite a
            // newer build's scope into nothing.
            Subject::Unrecognised(raw) => raw.clone(),
        };
        text.serialize(out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<Subject, D::Error> {
        let text = String::deserialize(input)?;
        if text == "all" {
            return Ok(Subject::All);
        }
        if let Some(name) = text.strip_prefix("category:") {
            return Ok(match kind_of(name) {
                Some(kind) => Subject::Category(kind),
                None => Subject::Unrecognised(text.clone()),
            });
        }
        if let Some(id) = text.strip_prefix("item:") {
            if !id.is_empty() {
                return Ok(Subject::Item(id.to_string()));
            }
        }
        Ok(Subject::Unrecognised(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    fn record_with(scopes: Vec<Scope>) -> KeyRecord {
        KeyRecord {
            name: "a key".to_string(),
            hash: hash_key("the-key"),
            created_unix: NOW,
            expires_unix: None,
            scopes,
        }
    }

    fn read(subject: Subject) -> Scope {
        Scope { subject, access: Access::Read }
    }

    /// **Default deny.** A key with no scopes can do nothing at all.
    #[test]
    fn a_key_with_no_scopes_permits_nothing() {
        let key = record_with(vec![]);
        assert!(!permits(&key, Access::Read, &Subject::All));
        assert!(!permits(&key, Access::Write, &Subject::All));
        assert!(!permits(&key, Access::Read, &Subject::Item("abc".to_string())));
        assert!(!permits(&key, Access::Read, &Subject::Category(ItemKind::Login)));
    }

    /// Read does not imply write, and write does not imply read.
    #[test]
    fn the_two_accesses_do_not_imply_each_other() {
        let reader = record_with(vec![read(Subject::All)]);
        assert!(permits(&reader, Access::Read, &Subject::All));
        assert!(!permits(&reader, Access::Write, &Subject::All));

        let writer = record_with(vec![Scope { subject: Subject::All, access: Access::Write }]);
        assert!(permits(&writer, Access::Write, &Subject::All));
        assert!(!permits(&writer, Access::Read, &Subject::All));
    }

    #[test]
    fn a_category_grant_does_not_reach_another_category() {
        let logins = record_with(vec![read(Subject::Category(ItemKind::Login))]);
        assert!(permits(&logins, Access::Read, &Subject::Category(ItemKind::Login)));
        assert!(!permits(&logins, Access::Read, &Subject::Category(ItemKind::Card)));
        assert!(!permits(&logins, Access::Read, &Subject::All));
    }

    /// **The test per-item grants exist for.**
    #[test]
    fn an_item_grant_does_not_reach_another_item() {
        let one = record_with(vec![read(Subject::Item("allowed".to_string()))]);
        assert!(permits(&one, Access::Read, &Subject::Item("allowed".to_string())));
        assert!(!permits(&one, Access::Read, &Subject::Item("other".to_string())));
        // And must not become the whole vault by asking for it.
        assert!(!permits(&one, Access::Read, &Subject::All));
        assert!(!permits(&one, Access::Read, &Subject::Category(ItemKind::Login)));
    }

    /// `All` really does cover the narrower subjects, or the tests above pass
    /// on a `covers` that always answers false.
    #[test]
    fn a_grant_of_everything_covers_the_narrower_subjects() {
        let everything = record_with(vec![read(Subject::All)]);
        assert!(permits(&everything, Access::Read, &Subject::All));
        assert!(permits(&everything, Access::Read, &Subject::Item("anything".to_string())));
        assert!(permits(&everything, Access::Read, &Subject::Category(ItemKind::Card)));
    }

    /// **Expiry is a question about now**, asked per request.
    #[test]
    fn an_expired_key_is_not_found_however_long_the_service_has_been_up() {
        let mut record = record_with(vec![read(Subject::All)]);
        record.expires_unix = Some(1_000);
        let records = vec![record];
        assert!(find(&records, "the-key", 999).is_some(), "control: it works before expiry");
        assert!(find(&records, "the-key", 1_000).is_none(), "expiry is inclusive");
        assert!(find(&records, "the-key", 10_000_000).is_none());
    }

    #[test]
    fn a_key_with_no_expiry_stays_live() {
        let records = vec![record_with(vec![read(Subject::All)])];
        assert!(find(&records, "the-key", 10_000_000_000).is_some());
    }

    #[test]
    fn a_wrong_key_finds_nothing() {
        let records = vec![record_with(vec![read(Subject::All)])];
        assert!(find(&records, "not-the-key", NOW).is_none());
        assert!(find(&records, "", NOW).is_none());
    }

    /// **The forward-compatibility rule.** An older build reading a newer
    /// file must deny the scope it cannot read, not widen it.
    #[test]
    fn a_subject_from_the_future_denies() {
        let json = r#"{"name":"k","hash":"aa","created_unix":1,"expires_unix":null,
            "scopes":[{"subject":"organisation:9","access":"read"}]}"#;
        let record: KeyRecord = serde_json::from_str(json).expect("the record must still load");
        assert_eq!(record.scopes.len(), 1, "control: the scope was dropped, not kept");
        assert!(!permits(&record, Access::Read, &Subject::All));
        assert!(!permits(&record, Access::Read, &Subject::Item("anything".to_string())));
        assert!(!permits(&record, Access::Read, &Subject::Category(ItemKind::Login)));
    }

    /// And it round-trips verbatim, so an older build does not silently
    /// rewrite a scope the owner set in a newer one.
    #[test]
    fn an_unrecognised_subject_survives_a_round_trip() {
        let json = r#"{"name":"k","hash":"aa","created_unix":1,"expires_unix":null,
            "scopes":[{"subject":"organisation:9","access":"read"}]}"#;
        let record: KeyRecord = serde_json::from_str(json).expect("load");
        let written = serde_json::to_string(&record).expect("write");
        assert!(written.contains("organisation:9"), "the scope was rewritten: {written}");
    }

    /// The store holds hashes. A key file that is read must not be a key file
    /// that works.
    #[test]
    fn the_stored_record_does_not_contain_the_key() {
        let record = record_with(vec![read(Subject::All)]);
        let written = serde_json::to_string(&record).expect("write");
        assert!(!written.contains("the-key"), "the key itself is in the record: {written}");
        assert!(written.contains(&hash_key("the-key")), "control: the hash is not there either");
    }

    /// Every known subject survives a round trip, so a scope the owner sets
    /// today still means the same thing after a restart.
    #[test]
    fn every_known_subject_round_trips() {
        for subject in [
            Subject::All,
            Subject::Category(ItemKind::Login),
            Subject::Category(ItemKind::SecureNote),
            Subject::Category(ItemKind::Card),
            Subject::Category(ItemKind::Identity),
            Subject::Category(ItemKind::SshKey),
            Subject::Item("some-id".to_string()),
        ] {
            let scope = Scope { subject: subject.clone(), access: Access::Read };
            let written = serde_json::to_string(&scope).expect("write");
            let read_back: Scope = serde_json::from_str(&written).expect("read");
            assert_eq!(read_back.subject, subject, "{written} did not round trip");
        }
    }

    /// A record that holds a scope set is not `Debug`-printable.
    #[test]
    fn the_record_type_does_not_derive_debug() {
        let source = include_str!("service_keys.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        let at = production.find("pub struct KeyRecord").expect("control: the type is gone");
        let before = &production[..at];
        let derive_at = before.rfind("#[derive(").expect("control: no derive above KeyRecord");
        let derive = &before[derive_at..];
        assert!(
            !derive.contains("Debug"),
            "`KeyRecord` derives Debug; what a credential opens gets printed with it"
        );
    }
    /// A session is full access bounded by time, not by scope.
    #[test]
    fn a_session_can_do_everything_until_it_expires() {
        let (record, _secret) = session_record(NOW, || [3u8; 32]);
        assert!(permits(&record, Access::Read, &Subject::All));
        assert!(permits(&record, Access::Write, &Subject::All));
        assert!(permits(&record, Access::Read, &Subject::Item("anything".to_string())));
    }

    /// **And it expires on its own.** This is the only thing bounding it,
    /// so it is the thing that gets a test.
    #[test]
    fn a_session_expires_without_anybody_revoking_it() {
        let (record, secret) = session_record(NOW, || [3u8; 32]);
        let records = vec![record];
        assert!(find(&records, &secret, NOW).is_some(), "control: it works when minted");
        assert!(find(&records, &secret, NOW + SESSION_LIFETIME_SECS - 1).is_some());
        assert!(find(&records, &secret, NOW + SESSION_LIFETIME_SECS).is_none());
    }

    /// The session record holds a hash, exactly like a named key. A
    /// session written to disk must not be a session that can be replayed
    /// off it.
    #[test]
    fn a_session_record_does_not_contain_its_own_key() {
        let (record, secret) = session_record(NOW, || [3u8; 32]);
        let written = serde_json::to_string(&record).expect("write");
        assert!(!written.contains(&secret), "the session key is in the record: {written}");
    }

    /// A scratch key-store path, unique to this process **and to this call**.
    ///
    /// `temp_dir()` and nothing else, ever: the real key store lives beside
    /// `settings.json` in `%APPDATA%`, and a test that wrote there would
    /// revoke the machine owner's own keys. The pid and the nanos are this
    /// crate's usual pair -- two `cargo test` runs at once, and two tests in
    /// one run asking for the same label, are both real here.
    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "deskwarden-service-keys-test-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is before the epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// The guard the path helper needs, since every file test below is only
    /// as isolated as it is: two labels must not collide, the name must carry
    /// this process, and nothing may escape the system temp directory.
    #[test]
    fn every_scratch_key_store_is_unique_and_inside_the_temp_directory() {
        let a = temp_path("collision-probe");
        let b = temp_path("collision-probe");
        assert_ne!(a, b, "two scratch key stores share a path, so two tests overwrite each other");
        for path in [&a, &b] {
            let name = path.file_name().expect("no file name").to_string_lossy().into_owned();
            assert!(name.contains(&std::process::id().to_string()), "{name} names no process");
            // Control for the two above: the uniqueness is not coming from
            // the label having been dropped.
            assert!(name.contains("collision-probe"), "{name} lost its label");
            assert!(path.starts_with(std::env::temp_dir()), "escaped the temp directory: {path:?}");
        }
    }

    fn minted(name: &str, expires: Option<u64>, existing: &[KeyRecord]) -> (KeyRecord, String) {
        mint(name.to_string(), expires, vec![read(Subject::All)], NOW, || [9u8; 32], existing)
            .expect("this key was supposed to mint")
    }

    /// **A key must never be minted dead.** [`find`] treats expiry
    /// inclusively, so an expiry of exactly now -- or of any earlier moment --
    /// produces a record that is in the list, looks configured, and answers
    /// 401 to the script it was made for. The owner would not see that here;
    /// they would see it in somebody else's log at 3am, if at all. So the
    /// refusal is at creation, where there is a screen to say it on.
    #[test]
    fn a_key_whose_expiry_has_already_passed_is_refused_rather_than_minted() {
        for expiry in [0, 1, NOW - 1, NOW] {
            assert_eq!(
                mint("backup".to_string(), Some(expiry), vec![], NOW, || [9u8; 32], &[]).err(),
                Some(MintRefusal::ExpiryAlreadyPassed),
                "an expiry of {expiry} was minted at {NOW}"
            );
        }
        // Control: the refusal is about the expiry and not about everything.
        // Without this, a `mint` that refused unconditionally would pass.
        let (record, _) = minted("backup", Some(NOW + 1), &[]);
        assert_eq!(record.expires_unix, Some(NOW + 1));
    }

    /// And the boundary the refusal is drawn at is [`find`]'s, not one
    /// guessed beside it: the earliest expiry `mint` accepts is one `find`
    /// still honours at the moment of minting.
    #[test]
    fn the_earliest_accepted_expiry_is_a_key_that_actually_works() {
        let (record, secret) = minted("backup", Some(NOW + 1), &[]);
        let records = vec![record];
        assert!(find(&records, &secret, NOW).is_some(), "the earliest accepted expiry is dead");
        // ...and it is genuinely the edge, not a comfortable margin: one
        // second later it is gone.
        assert!(find(&records, &secret, NOW + 1).is_none());
    }

    /// A key with no expiry at all is still allowed, because the screen says
    /// so out loud and an owner may mean it. Without this the test above
    /// passes on a `mint` that demanded an expiry.
    #[test]
    fn a_key_with_no_expiry_is_allowed() {
        let (record, _) = minted("forever", None, &[]);
        assert_eq!(record.expires_unix, None);
    }

    /// A nameless key cannot be recognised in the list and cannot be handed
    /// to [`revoke`], which takes a name. Minting one would be minting a
    /// credential the owner has no way to take back through the screen that
    /// made it.
    #[test]
    fn a_key_with_no_usable_name_is_refused() {
        for name in ["", "   ", "\t\n"] {
            assert_eq!(
                mint(name.to_string(), None, vec![], NOW, || [9u8; 32], &[]).err(),
                Some(MintRefusal::NoName),
                "{name:?} was accepted as a key name"
            );
        }
        // Control: a name with something in it does mint, so the refusal is
        // about the name being blank rather than about names.
        assert!(mint(" backup ".to_string(), None, vec![], NOW, || [9u8; 32], &[]).is_ok());
    }

    /// The name is stored trimmed, so the list does not show a key with
    /// invisible padding that the owner then cannot match when revoking.
    #[test]
    fn a_padded_name_is_stored_without_its_padding() {
        let (record, _) = minted("  backup  ", None, &[]);
        assert_eq!(record.name, "backup");
    }

    /// **Two keys of one name make the list a lie.** The owner would revoke
    /// one and believe they had revoked the other -- and since [`revoke`]
    /// works by name, the one still working is the one they cannot tell
    /// apart from the one they killed.
    #[test]
    fn a_second_key_of_the_same_name_is_refused() {
        let (first, _) = minted("backup", None, &[]);
        let existing = vec![first];
        assert_eq!(
            mint("backup".to_string(), None, vec![], NOW, || [1u8; 32], &existing).err(),
            Some(MintRefusal::DuplicateName("backup".to_string()))
        );
        // Control: the list is not simply refusing every mint once it is
        // non-empty.
        assert!(mint("restore".to_string(), None, vec![], NOW, || [1u8; 32], &existing).is_ok());
    }

    /// And the comparison is the one a person makes: case and padding are
    /// not two different keys to the human reading the list. This is the
    /// same relation [`revoke`] uses, which is what makes "refused as a
    /// duplicate" and "revokes that record" the same set of names.
    #[test]
    fn names_that_differ_only_in_case_or_padding_are_the_same_name() {
        let (first, _) = minted("Backup", None, &[]);
        let existing = vec![first];
        for name in ["backup", "BACKUP", "  Backup  "] {
            assert!(
                mint(name.to_string(), None, vec![], NOW, || [1u8; 32], &existing).is_err(),
                "{name:?} was minted alongside a key called Backup"
            );
            let mut records = existing.clone();
            assert!(revoke(&mut records, name), "{name:?} revoked nothing");
            assert!(records.is_empty());
        }
    }

    /// Every refusal reaches the owner as a sentence they can act on, and
    /// the duplicate one names the key it collided with -- a refusal that
    /// said only "that name is taken" leaves them hunting the list.
    #[test]
    fn every_refusal_says_something_the_owner_can_act_on() {
        for refusal in [
            MintRefusal::NoName,
            MintRefusal::DuplicateName("backup".to_string()),
            MintRefusal::ExpiryAlreadyPassed,
        ] {
            let message = refusal.message();
            assert!(message.len() > 20, "{message:?} is not an explanation");
            assert!(message.ends_with('.'), "{message:?} is not a sentence");
        }
        assert!(
            MintRefusal::DuplicateName("backup".to_string()).message().contains("backup"),
            "the duplicate refusal does not name the key it collided with"
        );
    }

    /// **Shown once, and stored as a hash.** The plaintext `mint` returns is
    /// a working credential and the record beside it must not be a second
    /// copy of it -- a key store that is read is otherwise a key store that
    /// works.
    #[test]
    fn a_minted_key_is_returned_once_and_never_stored() {
        let (record, secret) = minted("backup", None, &[]);
        let written = serde_json::to_string(&record).expect("write");
        assert!(!written.contains(&secret), "the minted key is in the record: {written}");
        // Control: the record does hold something derived from that key, so
        // the assertion above is not passing on an empty record.
        assert!(written.contains(&hash_key(&secret)));
        // And the plaintext returned is the one that opens it, exactly once
        // -- there is nowhere else to get it from.
        assert!(find(&[record], &secret, NOW).is_some());
    }

    /// Two mints from different draws are different keys. A fixed fake would
    /// otherwise hide a `mint` that ignored its randomness and issued one
    /// key to every consumer.
    #[test]
    fn two_minted_keys_are_not_the_same_key() {
        let first = mint("a".to_string(), None, vec![], NOW, || [1u8; 32], &[]).expect("mint");
        let second = mint("b".to_string(), None, vec![], NOW, || [2u8; 32], &[]).expect("mint");
        assert_ne!(first.1, second.1);
        assert_ne!(first.0.hash, second.0.hash);
    }

    /// Revoking removes the record, and the key stops working -- the second
    /// half being the one that matters, since a list that no longer shows a
    /// key while the service still honours it is the worst of both.
    #[test]
    fn revoking_removes_the_record_and_the_key_stops_working() {
        let (record, secret) = minted("backup", None, &[]);
        let mut records = vec![record];
        assert!(find(&records, &secret, NOW).is_some(), "control: it worked before revoking");
        assert!(revoke(&mut records, "backup"));
        assert!(records.is_empty());
        assert!(find(&records, &secret, NOW).is_none());
    }

    /// Revoking a name that is not there answers `false` rather than
    /// pretending. A screen that said "revoked" after removing nothing tells
    /// the owner a script is locked out when it is not.
    #[test]
    fn revoking_a_name_that_is_not_there_says_so() {
        let (record, secret) = minted("backup", None, &[]);
        let mut records = vec![record];
        assert!(!revoke(&mut records, "restore"));
        // And it left the key it did not recognise alone, rather than
        // clearing the list on the way past.
        assert_eq!(records.len(), 1);
        assert!(find(&records, &secret, NOW).is_some());
    }

    /// Revoking one key of several leaves the others working. Without this,
    /// a `revoke` that emptied the list entirely would satisfy every
    /// assertion above.
    #[test]
    fn revoking_one_key_does_not_disturb_the_others() {
        let (first, first_secret) = minted("backup", None, &[]);
        let existing = vec![first];
        let (second, second_secret) = mint(
            "restore".to_string(),
            None,
            vec![read(Subject::All)],
            NOW,
            || [8u8; 32],
            &existing,
        )
        .expect("mint");
        let mut records = vec![existing[0].clone(), second];
        assert!(revoke(&mut records, "backup"));
        assert_eq!(records.len(), 1);
        assert!(find(&records, &first_secret, NOW).is_none());
        assert!(find(&records, &second_secret, NOW).is_some(), "the wrong key was revoked");
    }

    /// A hand-edited file holding two keys of one name must not leave one of
    /// them behind, still working, after the owner revoked what they saw.
    /// `mint` refuses to create this state; the file is editable, so
    /// `revoke` does not assume it cannot exist.
    #[test]
    fn revoking_removes_every_record_of_that_name() {
        let mut records = vec![record_with(vec![read(Subject::All)]), record_with(vec![])];
        assert_eq!(records.len(), 2, "control: there are two to remove");
        assert!(revoke(&mut records, "a key"));
        assert!(records.is_empty(), "a key of the revoked name survived");
    }

    /// The store round-trips: what was saved is what loads, scopes and
    /// expiry included, and the saved key still opens the service after a
    /// restart. Anything less and revoking would be the only thing that
    /// worked across a restart.
    #[test]
    fn a_saved_store_loads_back_as_what_was_saved() {
        let path = temp_path("round-trip");
        let (record, secret) = minted("backup", Some(NOW + 60), &[]);
        save(&path, &[record]).expect("save");
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "backup");
        assert_eq!(loaded[0].expires_unix, Some(NOW + 60));
        assert_eq!(loaded[0].scopes, vec![read(Subject::All)]);
        assert!(find(&loaded, &secret, NOW).is_some(), "the key did not survive the round trip");
        let _ = std::fs::remove_file(&path);
    }

    /// **The write is atomic, and the temp file is not left behind.** A
    /// truncated `settings.json` costs a preference; a truncated key store
    /// silently revokes every key the owner has minted, including the ones
    /// their unattended scripts are holding. So the write goes to a sibling
    /// and is renamed over the target -- and a leftover sibling would be a
    /// second, stale copy of the store sitting beside the real one.
    #[test]
    fn saving_leaves_no_temp_file_beside_the_store() {
        let path = temp_path("atomic");
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        let (record, _) = minted("backup", None, &[]);
        save(&path, &[record]).expect("save");
        assert!(path.exists(), "control: the store was not written at all");
        assert!(!tmp.exists(), "a stale second copy of the key store was left at {tmp:?}");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&tmp);
    }

    /// Saving over an existing store replaces it rather than appending to
    /// it, so a revoke that is saved is a revoke that took.
    #[test]
    fn saving_over_an_existing_store_replaces_it() {
        let path = temp_path("replace");
        let (record, secret) = minted("backup", None, &[]);
        save(&path, &[record]).expect("save");
        assert_eq!(load(&path).len(), 1, "control: the first save landed");
        let mut records = load(&path);
        assert!(revoke(&mut records, "backup"));
        save(&path, &records).expect("save");
        let loaded = load(&path);
        assert!(loaded.is_empty(), "the revoked key is still in the store");
        assert!(find(&loaded, &secret, NOW).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// Two sessions minted from different draws are different keys.
    /// Without this, a fixed fake would hide a `session_record` that
    /// ignored its randomness entirely.
    #[test]
    fn two_sessions_are_not_the_same_key() {
        let (_, first) = session_record(NOW, || [3u8; 32]);
        let (_, second) = session_record(NOW, || [4u8; 32]);
        assert_ne!(first, second);
    }
}

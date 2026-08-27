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
}

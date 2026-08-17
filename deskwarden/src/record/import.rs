//! Turning a validated [`Record`] into the item that will be created, and
//! deciding whether creating it would collide with something already in the
//! vault.
//!
//! Pure. Nothing here talks to `bw serve`; [`item_from`] hands back a
//! [`NewItem`] and the caller POSTs it through
//! [`VaultBridge::create_item`](crate::vault_bridge::VaultBridge::create_item),
//! which is the bridge's existing `POST /object/item`. Sends go through the
//! CLI; the vault does not.
//!
//! # The seed lands in the item's own `totp` field
//!
//! So the **vault** computes the code. Deskwarden must not become a TOTP
//! implementation, and a seed parked in a note is a seed no client can use.
//! `the_seed_lands_in_the_items_own_totp_field` asserts both halves: that it is
//! in `login.totp`, and that it appears in the whole outgoing payload exactly
//! once.
//!
//! # A seal that will not open produces no item at all
//!
//! Not a partial item with the seed quietly missing. The user asked to import
//! a record; half of one, imported silently, is worse than a refusal — the
//! second factor would appear to have arrived and would not be there. The `?`
//! in [`item_from`] runs before a [`NewItem`] is constructed, so this is the
//! shape of the function rather than a rule to remember.
//!
//! # `not_after` does not gate anything here, and must not
//!
//! The 2026-08-17 decision put the imported record in the recipient's own
//! vault, and accepted knowingly that **a vault item does not expire**. So
//! `not_after` is *staleness information about the record*, not enforcement.
//! This module never reads it, and
//! `a_record_whose_not_after_has_passed_still_imports` is that absence made
//! checkable so nobody later "fixes" it into a hard gate the spec says it
//! cannot be. Presenting it — as advisory copy — is the import surface's job.

use crate::record::payload::Record;
use crate::record::seal::{unseal, SealFailed};
use crate::vault_bridge::{NewItem, VaultItem};
use zeroize::Zeroizing;

/// Why a record could not become an item.
///
/// Each arm is a sentence the import surface can render. A refusal that
/// renders as a generic failure teaches the user to retry, which is the
/// opposite of what a record that will not open should teach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportRefusal {
    /// The record carries a sealed seed and no passphrase was offered.
    PassphraseRequired,
    /// The passphrase did not open the seal, or the bytes were altered.
    /// One arm for both, because AES-GCM cannot tell them apart — see
    /// [`SealFailed::WrongPassphrase`].
    WrongPassphrase,
    /// The seed was sealed some way this build does not know.
    UnsupportedSeal,
}

impl ImportRefusal {
    pub fn sentence(self) -> &'static str {
        match self {
            Self::PassphraseRequired => {
                "This record carries a one-time code seed, which needs the passphrase the \
                 sender set. Nothing was imported."
            }
            Self::WrongPassphrase => SealFailed::WrongPassphrase.sentence(),
            Self::UnsupportedSeal => SealFailed::UnsupportedSeal.sentence(),
        }
    }
}

/// Builds the item to create from a record, opening its sealed seed first.
///
/// Returns `Err` **before building anything** when the seal will not open. See
/// the module docs for why a partial item is not the fallback.
///
/// `not_after` is deliberately not consulted. See the module docs.
pub fn item_from(record: &Record, passphrase: Option<&str>) -> Result<NewItem, ImportRefusal> {
    // The three cases are exhaustive over the pair, so "a sealed seed and no
    // passphrase" cannot fall through to a `None` seed on a created item.
    let totp: Option<Zeroizing<String>> = match (&record.totp_sealed, passphrase) {
        (None, _) => None,
        (Some(_), None) => return Err(ImportRefusal::PassphraseRequired),
        (Some(sealed), Some(passphrase)) => Some(unseal(sealed, passphrase).map_err(|e| {
            match e {
                SealFailed::WrongPassphrase => ImportRefusal::WrongPassphrase,
                SealFailed::UnsupportedSeal => ImportRefusal::UnsupportedSeal,
            }
        })?),
    };

    Ok(NewItem::ImportedRecord {
        name: record.name.clone(),
        // Filed nowhere. Choosing a folder is the surface's business, and
        // guessing one here would file a stranger's record into a folder the
        // recipient curated.
        folder_id: None,
        username: record.username.clone(),
        password: record.password.clone(),
        totp,
        uri: record.uri.clone(),
        // `notes` is text to store. It is not a key sequence, a path or a URL,
        // and nothing here interprets it.
        notes: record.notes.clone().map(Zeroizing::new),
    })
}

/// Whether importing `record` would land on top of something already there.
///
/// A **value**, not an action: the surface asks the user what to do with it.
/// Nothing in this module overwrites anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collision {
    Fresh,
    SameName { existing_id: String },
}

/// Matches on **name only**.
///
/// Deliberately not on username or URI, and this is the load-bearing choice in
/// the whole policy: two genuinely different accounts on one service share both
/// — a personal and an admin login on the same host, with the same email — so
/// matching on either would report a collision between records that are not the
/// same record. The worst outcome of a missed collision is a duplicate item the
/// user can delete. The worst outcome of a *false* collision is a credential
/// silently replaced by someone else's. Those are not symmetric.
pub fn collides(record: &Record, existing: &[VaultItem]) -> Collision {
    match existing.iter().find(|item| item.name == record.name) {
        Some(item) => Collision::SameName { existing_id: item.id.clone() },
        None => Collision::Fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::seal::seal;
    use crate::record::seal::SealedSeed;
    use std::sync::OnceLock;

    const SEED: &str = "JBSWY3DPEHPK3PXP";
    const PASSPHRASE: &str = "correct horse battery staple";

    /// **Sealed once for the whole module.** Argon2id costs roughly 0.7 s per
    /// derivation in a debug build, and `seal` is one; sealing per test would
    /// buy nothing and pay for it three times over. The seal is a value, so
    /// sharing it changes nothing any test asserts.
    fn a_sealed_seed() -> &'static SealedSeed {
        static SEALED: OnceLock<SealedSeed> = OnceLock::new();
        SEALED.get_or_init(|| seal(SEED, PASSPHRASE))
    }

    fn bare(name: &str) -> Record {
        Record {
            name: name.to_string(),
            username: None,
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            not_after: None,
        }
    }

    fn an_existing_item(id: &str, name: &str) -> VaultItem {
        VaultItem {
            id: id.to_string(),
            name: name.to_string(),
            fields: Vec::new(),
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// One Argon2 derivation (the shared seal's) plus one here.
    #[test]
    fn the_seed_lands_in_the_items_own_totp_field() {
        let record = Record {
            notes: Some("nothing to see".to_string()),
            totp_sealed: Some(a_sealed_seed().clone()),
            ..bare("SAP Production")
        };
        let payload = item_from(&record, Some(PASSPHRASE)).expect("the seal opens").to_payload();

        // Positively: it is where the vault will look for it.
        assert_eq!(
            payload["login"]["totp"].as_str(),
            Some(SEED),
            "the seed is not in the item's own totp field: {payload}"
        );
        // And nowhere else. Counted over the WHOLE outgoing body rather than
        // asserted absent from `notes` alone: "not in notes" passes vacuously
        // over an item with no notes, and would say nothing about a seed
        // copied into a custom field or into the name.
        let whole = payload.to_string();
        assert_eq!(
            whole.matches(SEED).count(),
            1,
            "the seed appears more than once in the payload, so it was also written somewhere \
             it cannot be used from: {whole}"
        );
        // Control: the item really does carry notes, so the count above is
        // about a seed that is not in them and not about an empty item.
        assert_eq!(payload["notes"].as_str(), Some("nothing to see"));
    }

    /// One Argon2 derivation.
    #[test]
    fn a_record_whose_seal_will_not_open_produces_no_item_at_all() {
        let record = Record {
            username: Some("dplatonov".to_string()),
            totp_sealed: Some(a_sealed_seed().clone()),
            ..bare("SAP Production")
        };
        assert!(
            matches!(item_from(&record, Some("wrong")), Err(ImportRefusal::WrongPassphrase)),
            "a record whose seed could not be opened produced an item anyway"
        );
    }

    /// No Argon2 at all: the refusal is reached before any derivation.
    #[test]
    fn a_sealed_seed_with_no_passphrase_is_refused_rather_than_dropped() {
        let record =
            Record { totp_sealed: Some(a_sealed_seed().clone()), ..bare("SAP Production") };
        assert!(matches!(item_from(&record, None), Err(ImportRefusal::PassphraseRequired)));
        // Control: the same call with the right passphrase is not a refusal,
        // so the assertion above is about the missing passphrase and not about
        // a function that refuses everything sealed.
        assert!(item_from(&record, Some(PASSPHRASE)).is_ok());
    }

    #[test]
    fn a_record_whose_not_after_has_passed_still_imports() {
        // The 2026-08-17 decision: a vault item does not expire, so `not_after`
        // is staleness information and NOT enforcement. If this test ever reds
        // because `item_from` grew a date check, the check is the defect --
        // the surface says the record is stale, and imports it anyway.
        let stale = Record {
            username: Some("dplatonov".to_string()),
            not_after: Some("2001-01-01T00:00:00Z".to_string()),
            ..bare("SAP Production")
        };
        let item = item_from(&stale, None).expect("a past not_after must not block the import");
        assert_eq!(item.name(), "SAP Production");
        assert_eq!(item.to_payload()["login"]["username"].as_str(), Some("dplatonov"));

        // Control: the record really does carry a not_after that is in the
        // past, so the success above is about a date that was ignored and not
        // about a fixture with no date on it.
        assert_eq!(stale.not_after.as_deref(), Some("2001-01-01T00:00:00Z"));
    }

    #[test]
    fn an_unsent_field_does_not_become_an_empty_one_in_the_vault() {
        // Absence survives the crossing. A blank username written here is a
        // blank the sender never chose, and on a replace it overwrites
        // something real with nothing.
        let payload = item_from(&Record { username: Some("dplatonov".to_string()), ..bare("SAP") }, None)
            .expect("no seal to open")
            .to_payload();
        assert_eq!(payload["login"]["username"].as_str(), Some("dplatonov"));
        assert!(
            payload["login"].get("password").is_none(),
            "an unsent password became a key in the create payload: {payload}"
        );
        assert!(payload.get("notes").is_none(), "{payload}");
    }

    #[test]
    fn a_uri_becomes_the_logins_own_uri_entry() {
        let payload =
            item_from(&Record { uri: Some("https://sap.example".to_string()), ..bare("SAP") }, None)
                .expect("no seal to open")
                .to_payload();
        assert_eq!(payload["login"]["uris"][0]["uri"].as_str(), Some("https://sap.example"));
    }

    #[test]
    fn a_name_already_in_the_vault_is_a_collision_and_names_the_item_it_hit() {
        let vault = [
            an_existing_item("aaa", "Some Other Thing"),
            an_existing_item("bbb", "SAP Production"),
        ];
        assert_eq!(
            collides(&bare("SAP Production"), &vault),
            Collision::SameName { existing_id: "bbb".to_string() }
        );
    }

    #[test]
    fn a_name_not_in_the_vault_is_fresh_and_the_vault_was_not_empty() {
        let vault = [
            an_existing_item("aaa", "Some Other Thing"),
            an_existing_item("bbb", "SAP Production"),
        ];
        assert_eq!(collides(&bare("A Brand New Thing"), &vault), Collision::Fresh);
        // The live control the plan asks for: `Fresh` above is a verdict about
        // a vault that HAS items in it, not the trivially correct answer to an
        // empty list. Asserted through `collides` itself rather than by
        // eyeballing the fixture, so a fixture that quietly emptied would red.
        assert!(!vault.is_empty());
        assert_eq!(
            collides(&bare("SAP Production"), &vault),
            Collision::SameName { existing_id: "bbb".to_string() },
            "control: this fixture vault cannot produce a collision at all, so `Fresh` above \
             means nothing"
        );
    }

    #[test]
    fn a_matching_username_and_uri_under_a_different_name_is_not_a_collision() {
        // The trap this policy exists to avoid. Two genuinely different
        // accounts on one service share a username and a URI -- and a false
        // collision that silently overwrites a credential is far worse than a
        // duplicate item.
        let mut existing = an_existing_item("bbb", "SAP Production (admin)");
        existing.login = Some(crate::vault_bridge::LoginData {
            username: Some("dplatonov".to_string()),
            uris: vec![crate::vault_bridge::UriEntry {
                uri: Some("https://sap.example".to_string()),
                other: serde_json::Map::new(),
            }],
            ..Default::default()
        });
        let vault = [existing];
        let incoming = Record {
            username: Some("dplatonov".to_string()),
            uri: Some("https://sap.example".to_string()),
            ..bare("SAP Production (personal)")
        };
        assert_eq!(
            collides(&incoming, &vault),
            Collision::Fresh,
            "a shared username and URI was read as the same record"
        );
        // Control: the two really do share both, so `Fresh` above is a
        // decision not to match on them rather than a fixture with nothing in
        // common.
        assert_eq!(
            vault[0].login.as_ref().and_then(|l| l.username.as_deref()),
            incoming.username.as_deref()
        );
        assert_eq!(
            vault[0].login.as_ref().and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref()),
            incoming.uri.as_deref()
        );
    }
}

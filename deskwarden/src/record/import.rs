//! Turning a validated [`Record`] into the item that will be created.
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
use crate::vault_bridge::NewItem;
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
}

//! **`sharing_state.json`**: what personal sharing has to remember on this
//! PC, and nothing it could read from the server instead.
//!
//! Spec 5.4 lists it. Everything else about sharing -- who is invited, who is
//! confirmed, which records are in which collections -- is read from the
//! server on every pass, so two PCs never disagree about a share. What cannot
//! be read back from anywhere is:
//!
//! * **Pins**: the SHA-256 of a person's public key, the first time this
//!   account confirmed them. Auto-confirm trusts a key on first use and
//!   refuses a CHANGED one (spec 11.5, 12.1); the first key has to be
//!   remembered somewhere the server cannot rewrite.
//! * **Moves**: the folder and star a record had before it left the personal
//!   vault. An organisation cipher carries neither on NodeWarden; they are put
//!   back when the record comes home.
//! * **Unfinished**: the second half of a move back that failed (spec 13).
//! * **The sharing org's id**, so a rename in the web vault does not strand
//!   the shares (spec 5.1).
//!
//! # Keyed by account
//!
//! One file for every account on this PC, each account's state under its
//! user id, so a pin taken while signed in as one account is never trusted
//! for another.
//!
//! # No secret
//!
//! Emails, ids, hashes and two booleans. `nothing_in_the_file_can_open_anything`
//! holds it there.
//!
//! # Atomic
//!
//! Written to a `.tmp` sibling and renamed over the file, in
//! `service_keys::save`'s idiom: a torn pin file is a person whose key reads
//! as changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name, inside [`crate::settings::config_dir`].
pub const SHARING_STATE_FILE_NAME: &str = "sharing_state.json";

/// Where the file lives, or `None` with no config directory.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(SHARING_STATE_FILE_NAME))
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingState {
    /// Each account's state, by user id.
    pub accounts: BTreeMap<String, AccountSharing>,
}

/// One account's sharing state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountSharing {
    /// The account's own sharing organisation, once chosen.
    pub sharing_org_id: Option<String>,
    /// Public-key pins, by normalised email.
    pub pins: BTreeMap<String, Pin>,
    /// Where a shared record was, by its organisation cipher id.
    pub moves: BTreeMap<String, Move>,
    /// Moves back whose second half failed, retried on every pass.
    pub unfinished: Vec<Unfinished>,
}

/// A person's public key, as first confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pin {
    /// Lowercase hex SHA-256 of the SPKI DER.
    pub sha256: String,
    /// The account the key belonged to.
    pub user_id: String,
    /// When it was pinned, UTC milliseconds.
    pub at: i64,
}

/// Where a record was before it was shared.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Move {
    pub folder_id: Option<String>,
    pub favorite: bool,
}

/// The second half of a move that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Unfinished {
    /// The personal copy was made and the organisation copy was not deleted.
    /// The org copy is filed in no collection, so nobody but the owner sees it.
    #[serde(rename_all = "camelCase")]
    PurgeOrgCopy { org_cipher_id: String, personal_id: String },
}

impl SharingState {
    /// Reads the file. Absent, empty or unparseable is empty, as
    /// `sent_history` reads its own.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        if text.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Writes the whole file atomically: a `.tmp` sibling, then a rename.
    ///
    /// # Errors
    ///
    /// The directory could not be made, or the write or rename failed.
    pub fn persist(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?)?;
        if let Err(e) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
        Ok(())
    }

    /// One account's state, or an empty one.
    #[must_use]
    pub fn account(&self, user_id: &str) -> AccountSharing {
        self.accounts.get(user_id).cloned().unwrap_or_default()
    }

    /// One account's state, made if absent.
    pub fn account_mut(&mut self, user_id: &str) -> &mut AccountSharing {
        self.accounts.entry(user_id.to_string()).or_default()
    }
}

impl AccountSharing {
    /// The pin for a person, by email in any case.
    #[must_use]
    pub fn pin_for(&self, email: &str) -> Option<&Pin> {
        self.pins.get(&crate::rest::organizations::normalise_email(email))
    }

    /// Pins (or re-pins, after "Share anyway") a person's key.
    pub fn set_pin(&mut self, email: &str, pin: Pin) {
        self.pins.insert(crate::rest::organizations::normalise_email(email), pin);
    }

    /// Forgets everything kept about a person.
    pub fn forget_person(&mut self, email: &str) {
        self.pins.remove(&crate::rest::organizations::normalise_email(email));
    }
}

/// Read, change, write -- `sent_history::update`'s shape, for its reason:
/// another process may have written in between. Returns the state as written.
///
/// # Errors
///
/// See [`SharingState::persist`].
pub fn update(path: &Path, change: impl FnOnce(&mut SharingState)) -> std::io::Result<SharingState> {
    let mut state = SharingState::load(path);
    change(&mut state);
    state.persist(path)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> (crate::test_scratch::ScratchDir, PathBuf) {
        let dir = crate::test_scratch::ScratchDir::new(&format!("sharing-state-{tag}"));
        let path = dir.join("sharing_state.json");
        (dir, path)
    }

    /// Round trip, per account, and the on-disk shape spec 5.4 gives for an
    /// unfinished move.
    #[test]
    fn the_file_round_trips_per_account_and_forgives_a_broken_one() {
        let (_dir, path) = temp_path("roundtrip");
        assert_eq!(SharingState::load(&path), SharingState::default(), "a missing file is empty");
        let written = update(&path, |state| {
            let me = state.account_mut("user-1");
            me.sharing_org_id = Some("org1".into());
            me.set_pin(" Anna@Example.com ", Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 7 });
            me.moves.insert("c1".into(), Move { folder_id: Some("f1".into()), favorite: true });
            me.unfinished.push(Unfinished::PurgeOrgCopy { org_cipher_id: "c1".into(), personal_id: "p1".into() });
            state.account_mut("user-2").sharing_org_id = Some("org2".into());
        })
        .expect("writes");
        let read = SharingState::load(&path);
        assert_eq!(read, written);
        assert_eq!(read.account("user-1").pin_for("anna@example.com").map(|p| p.user_id.as_str()), Some("anna-id"));
        assert_eq!(read.account("user-2").sharing_org_id.as_deref(), Some("org2"));
        assert_eq!(read.account("nobody"), AccountSharing::default());
        let raw = std::fs::read_to_string(&path).expect("reads");
        assert!(raw.contains("\"kind\": \"purgeOrgCopy\"") && raw.contains("\"orgCipherId\": \"c1\""), "{raw}");
        std::fs::write(&path, "{ not json").expect("writes");
        assert_eq!(SharingState::load(&path), SharingState::default());
    }

    /// Forget removes a person's pin (spec 12.4: "forget removes a person from
    /// both" files).
    #[test]
    fn forgetting_a_person_drops_their_pin() {
        let mut me = AccountSharing::default();
        me.set_pin("anna@example.com", Pin { sha256: "ab".into(), user_id: "a".into(), at: 1 });
        me.forget_person("ANNA@example.com");
        assert!(me.pin_for("anna@example.com").is_none());
    }

    /// **No secret is a field here** (spec 12.4): emails, ids and key hashes.
    #[test]
    fn nothing_in_the_file_can_open_anything() {
        let source = include_str!("sharing_state.rs");
        let production = source.split(concat!("#[cfg(", "test)]")).next().expect("a production half");
        for banned in ["pub key:", "pub token", "pub password", "pub secret", "Zeroizing"] {
            assert!(!production.contains(banned), "sharing_state gained `{banned}`");
        }
    }
}

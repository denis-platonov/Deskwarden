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
//!
//! # A file that does not parse is never written over
//!
//! [`SharingState::load`] reads one as empty, for display -- a card with no
//! pins shown is harmless. A WRITE is not: [`update`] used to load, change
//! and persist, so the first share, pin or forget after a torn or hand-edited
//! file replaced every pin in it with nothing, silently, and the next confirm
//! trusted a swapped key on first use. Now [`update`] renames such a file
//! aside (`sharing_state.json.unreadable-<unix ms>`, kept for whoever wants
//! to look), logs it, and fails with [`UNREADABLE_SET_ASIDE`], so the
//! operation that wanted to write says so and a retry starts from a fresh
//! file. A file that exists but could not be READ (held open by another
//! process, say) is not corrupt, and is neither renamed nor written: that
//! failure is [`UNREADABLE_NOW`]. `update_never_writes_over_a_file_it_cannot_parse`
//! pins it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name, inside [`crate::settings::config_dir`].
pub const SHARING_STATE_FILE_NAME: &str = "sharing_state.json";

/// A write found the file there and unparseable, and set it aside. The plain
/// sentence the operation that wanted to write fails with.
pub const UNREADABLE_SET_ASIDE: &str =
    "Deskwarden couldn't read what it keeps about sharing on this PC, so it set that file aside. Try again.";

/// A write could not read the file at all (not a parse failure): nothing was
/// renamed and nothing written.
pub const UNREADABLE_NOW: &str = "Deskwarden couldn't read what it keeps about sharing on this PC. Try again.";

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
    /// **This client CREATED `sharing_org_id`** -- set by the share that made
    /// it, kept while the id stays the same, cleared when another is chosen.
    /// On a server that does not attest an org's owner (no `ownerEmail`) it
    /// is the only thing that lets the cached id be used: an org merely OWNED
    /// and NAMED for this account may be one someone else made and added it
    /// to as owner (`Sharing::my_org_id`). `serde(default)`: a file from
    /// before it reads as "not created here", the safe side.
    pub sharing_org_created: bool,
    /// **The user said to use `sharing_org_id`, which this client did not
    /// create** -- the answer Yes to "Use the organisation ... for sharing?
    /// Deskwarden did not create it." (official-path spec §3.1, owner's fix
    /// round 1). From then on the id is used as a created one is: no roster
    /// question, no second ask. Set only by `Sharing::adopt`, kept while the
    /// id stays the same, cleared with it. `serde(default)`: a file from
    /// before it reads as "not adopted", so such an org is asked about once.
    pub sharing_org_adopted: bool,
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

/// Why the file could not be read: the two cases [`update`] treats apart.
enum ReadFailure {
    /// There, and could not be read -- held open, no permission.
    Io,
    /// Read, and not the state: torn, hand-edited, another format.
    Corrupt,
}

/// **Why a write did not happen**, carried inside the `io::Error` [`update`]
/// returns so a caller can tell "the file could not be read" (fail the
/// operation, with [`Self::sentence`]) from a disk that would not take the
/// write. See [`unreadable`].
#[derive(Debug)]
pub struct Unreadable {
    /// The file was unparseable and was renamed aside.
    pub set_aside: bool,
}

impl Unreadable {
    /// The plain sentence for it.
    #[must_use]
    pub fn sentence(&self) -> &'static str {
        if self.set_aside { UNREADABLE_SET_ASIDE } else { UNREADABLE_NOW }
    }
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.sentence())
    }
}

impl std::error::Error for Unreadable {}

/// The [`Unreadable`] inside an [`update`] error, when that is what it was.
#[must_use]
pub fn unreadable(error: &std::io::Error) -> Option<&Unreadable> {
    error.get_ref().and_then(|inner| inner.downcast_ref::<Unreadable>())
}

impl SharingState {
    /// Reads the file. Absent, empty or unparseable is empty, as
    /// `sent_history` reads its own -- **for display only**. Anything that
    /// writes goes through [`update`], which does not read a broken file as
    /// empty (see the module docs).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        Self::read(path).unwrap_or_default()
    }

    /// [`Self::load`], telling "nothing there" from "something there that
    /// cannot be read": absent or empty is `Some(empty)`, a file that will not
    /// read or parse is `None`. What the confirm pass asks before it makes a
    /// pin decision -- an unreadable file read as empty would be "no pins",
    /// and every swapped key would be trusted on first use again.
    #[must_use]
    pub fn read(path: &Path) -> Option<Self> {
        Self::read_checked(path).ok()
    }

    fn read_checked(path: &Path) -> Result<Self, ReadFailure> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Err(ReadFailure::Corrupt),
            Err(_) => return Err(ReadFailure::Io),
        };
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&text).map_err(|_| ReadFailure::Corrupt)
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
/// **Never over a file it cannot parse**: that one is renamed aside and the
/// write refused -- see the module docs.
///
/// # Errors
///
/// An [`Unreadable`] (read it with [`unreadable`]) when the file is there and
/// could not be read or parsed; otherwise see [`SharingState::persist`].
pub fn update(path: &Path, change: impl FnOnce(&mut SharingState)) -> std::io::Result<SharingState> {
    let mut state = readable_for_a_write(path)?;
    change(&mut state);
    state.persist(path)?;
    Ok(state)
}

/// **The file, as a write may start from it** -- or, if it is there and does
/// not parse, set aside, and the error that says so. [`update`]'s first
/// step, and what a sharing write that must make a pin decision asks before
/// it makes one (`Sharing::ensure_member`), so it refuses the way a write
/// would rather than trusting a key on first use over pins it cannot see.
///
/// # Errors
///
/// An [`Unreadable`], read with [`unreadable`].
pub fn readable_for_a_write(path: &Path) -> std::io::Result<SharingState> {
    match SharingState::read_checked(path) {
        Ok(state) => Ok(state),
        Err(ReadFailure::Io) => Err(std::io::Error::other(Unreadable { set_aside: false })),
        Err(ReadFailure::Corrupt) => {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            let aside = path.with_file_name(format!("{SHARING_STATE_FILE_NAME}.unreadable-{millis}"));
            match std::fs::rename(path, &aside) {
                Ok(()) => log::warn!("the sharing state file did not parse; it was set aside as {}", aside.display()),
                Err(e) => log::warn!("the sharing state file did not parse and could not be set aside ({e}); it was left as it is"),
            }
            Err(std::io::Error::other(Unreadable { set_aside: true }))
        }
    }
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
        assert_eq!(SharingState::load(&path), SharingState::default(), "for display, a broken file reads as empty");
    }

    /// **`update` never writes over a file it cannot parse** (review of
    /// Task 12): the file is renamed aside, byte for byte, the write is
    /// refused with the plain sentence, the change is not applied anywhere,
    /// and a retry starts from a fresh file.
    #[test]
    fn update_never_writes_over_a_file_it_cannot_parse() {
        let (dir, path) = temp_path("corrupt");
        let broken = "{ \"accounts\": { \"user-1\": { \"pins\": { torn";
        std::fs::write(&path, broken).expect("writes");
        let mut changed = false;
        let refused = update(&path, |state| {
            changed = true;
            state.account_mut("user-1").sharing_org_id = Some("org-new".into());
        })
        .expect_err("a broken file is not written over");
        assert!(!changed, "the change ran over a state read as empty");
        assert_eq!(unreadable(&refused).map(Unreadable::sentence), Some(UNREADABLE_SET_ASIDE));
        assert_eq!(refused.to_string(), UNREADABLE_SET_ASIDE);
        assert!(!path.exists(), "nothing was written in its place");
        let aside: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .expect("the directory")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("sharing_state.json.unreadable-")))
            .collect();
        assert_eq!(aside.len(), 1, "{aside:?}");
        assert_eq!(std::fs::read_to_string(&aside[0]).expect("reads"), broken, "the file was kept as it was");
        // A retry starts from a fresh file.
        update(&path, |state| state.account_mut("user-1").sharing_org_id = Some("org-new".into())).expect("the retry writes");
        assert_eq!(SharingState::load(&path).account("user-1").sharing_org_id.as_deref(), Some("org-new"));
        // A plain disk error is not an `Unreadable`.
        assert!(unreadable(&std::io::Error::other("disk full")).is_none());
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

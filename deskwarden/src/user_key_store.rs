//! The DPAPI-wrapped **master key and refresh token** for one account, at a
//! path its caller chooses.
//!
//! # This file is the strongest secret this app writes
//!
//! [`crate::session_store`] writes a `bw` session token. That token expires;
//! `bw` holds the master password and the keys derived from it, and this
//! process never sees either. The direct-REST backend
//! ([`crate::rest::backend::RestBackend`]) removes `bw` from the picture, and
//! with it that property: the key that unwraps the vault is now derived *in
//! this process*, and if a restart is not to ask for the master password
//! again, it has to be kept.
//!
//! The owner's decision is to keep it, wrapped by DPAPI exactly as the
//! session token is. What follows from that decision is written here rather
//! than assumed, because the difference from `session.bin` is not the wrapping
//! -- it is what is inside:
//!
//! * **A master key does not expire and cannot be revoked.** Revoking a
//!   session token is a click in the web vault. The only thing that retires a
//!   master key is changing the master password, which re-encrypts the whole
//!   vault.
//! * So the file is per account and DPAPI-wrapped to the *Windows user*, it
//!   is written by exactly one function in this module, and the plaintext
//!   exists only inside a [`Zeroizing`] buffer on both sides of that write.
//! * Nothing in this module formats, logs, or returns a secret in an error.
//!   [`UserKeyStore::load`] answers `Option`, not `Result`, precisely so that
//!   there is no error type here for a byte to hide in: every way a record can
//!   be wrong -- missing file, DPAPI refusal, wrong magic, wrong version,
//!   wrong length, non-UTF-8 token -- collapses to `None`, and `None` means
//!   "ask for the master password".
//!
//! # It takes a path, and does not resolve one
//!
//! Exactly [`crate::session_store::SessionStore`]'s rule, for exactly its
//! reason, and worth restating because the consequence here is worse. A store
//! that knew where to put its own file would be a second definition of the
//! layout; any account whose copy resolved back to a shared location would
//! find and overwrite another account's file. For a session token that logs
//! somebody out. For a master key that would mean **one account's vault key
//! written into another account's slot** -- and a later load would hand
//! `RestBackend` a key that decrypts nothing, or, far worse in a future where
//! the two accounts are on one server, silently the wrong identity's key.
//!
//! So the path comes from [`crate::accounts::user_key_path_for`], beside that
//! account's `session.bin`, and this module has no constructor that resolves a
//! config directory.
//!
//! # A stored record can stop working, and that is normal
//!
//! The master password can be changed on another device, the key can be
//! rotated, the refresh token can be revoked. Nothing on this machine can
//! detect any of those: a `MasterKey` is thirty-two bytes with no checksum and
//! no version, and a refresh token is opaque. **The only thing that can tell
//! is the server**, by refusing.
//!
//! This module therefore promises nothing about whether what it returns still
//! works, and its caller must treat a failure from the first
//! [`crate::rest::api::RestClient`] call as "these credentials are dead":
//! [`UserKeyStore::clear`] the file and ask for the master password. Falling
//! back to *asking* is the required behaviour; falling back to a backend that
//! is constructed but cannot decrypt anything is the failure this paragraph
//! exists to prevent.

use std::path::{Path, PathBuf};

use zeroize::{Zeroize, Zeroizing};

use crate::rest::api::{Authenticated, Session};
use crate::rest::crypto::{MasterKey, MASTER_KEY_LEN};

/// Four bytes at the front of the plaintext, so a file that is not one of
/// these is refused rather than read as one.
///
/// DPAPI-unwrapping already fails on anything that was not protected by this
/// Windows user, so this is not the security boundary -- it is the check that
/// catches *our own* mistake: a `session.bin` copied over a `userkey.bin`, or
/// this file's layout changed without its version being changed with it.
const MAGIC: &[u8; 4] = b"DWUK";

/// The layout version. Bumped when the bytes after the header change shape.
///
/// An unrecognised version is [`UserKeyStore::load`] answering `None`, which
/// means "ask for the master password" -- the same answer as no file at all,
/// and the only safe reading of a record this build cannot parse.
const VERSION: u8 = 1;

/// `MAGIC` + `VERSION` + the key + a `u32` length for the refresh token.
const HEADER_LEN: usize = MAGIC.len() + 1 + MASTER_KEY_LEN + 4;

/// One account's master key and refresh token, at a path its caller chooses.
///
/// See the module docs for why this takes a path and never resolves one, and
/// for what the caller owes the user when the record stops working.
///
/// No `Debug`, derived or otherwise: this type holds a path and nothing else,
/// so there is nothing here to redact -- but printing the path of the file
/// that holds a master key is of no use to anyone debugging and is one more
/// line that could end up in a log.
pub struct UserKeyStore {
    path: PathBuf,
}

impl UserKeyStore {
    /// `path` is one account's `userkey.bin`; see the type's own doc. Its
    /// parent directory must already exist -- the account directory is
    /// created when the account is
    /// ([`crate::accounts::ensure_account_dir`]), not lazily here, exactly as
    /// [`crate::session_store::SessionStore::new`] requires, so that a stored
    /// key can never be the thing that brings an account directory into
    /// being.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The `userkey.bin` this store reads and writes.
    ///
    /// Exists for [`crate::session_store::SessionStore::path`]'s reason: an
    /// account switch's re-point has to be observable *while* the switch runs,
    /// not only at the end of it. A switch that authenticated the incoming
    /// account while this store still addressed the outgoing one would write
    /// the new master key over the account the user is leaving.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the key and the refresh token, DPAPI-wrapped.
    ///
    /// **Returns `Ok(false)` and writes nothing when the session carries no
    /// usable refresh token** -- absent, or present and empty. That is not an
    /// error and it is not a silent success: a session that cannot be
    /// refreshed cannot be revived after a restart, so a file holding only the
    /// key would be a file whose load could never produce a usable
    /// [`Authenticated`]. Writing a master key to disk that can never be used
    /// is strictly worse than not writing one, so this declines, and the
    /// boolean is how the caller can say so.
    ///
    /// The plaintext record is assembled inside a [`Zeroizing`] buffer and
    /// wiped when this function returns, whichever way it returns. What
    /// reaches the disk is only what [`crate::session_store::protect`] hands
    /// back.
    pub fn save(&self, authenticated: &Authenticated) -> std::io::Result<bool> {
        let Some(refresh) = authenticated.session.expose_refresh_token() else {
            return Ok(false);
        };
        let refresh = refresh.as_bytes();
        if refresh.is_empty() {
            return Ok(false);
        }
        let Ok(refresh_len) = u32::try_from(refresh.len()) else {
            // Unreachable against any real server -- a refresh token is a few
            // hundred bytes -- but a length that does not fit the field is a
            // record this module could not read back, and it refuses to write
            // one rather than truncating.
            return Ok(false);
        };

        let mut plain = Zeroizing::new(Vec::with_capacity(HEADER_LEN + refresh.len()));
        plain.extend_from_slice(MAGIC);
        plain.push(VERSION);
        plain.extend_from_slice(authenticated.master_key.expose_bytes());
        plain.extend_from_slice(&refresh_len.to_le_bytes());
        plain.extend_from_slice(refresh);

        // The error carries DPAPI's own status and nothing of `plain`.
        let protected = crate::session_store::protect(&plain)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        std::fs::write(&self.path, protected)?;
        Ok(true)
    }

    /// Reads the record back, or `None`.
    ///
    /// `None` for every way this can fail -- see the module docs for why there
    /// is no error type here -- and in every case it means the same thing to
    /// the caller: **ask for the master password**.
    ///
    /// What comes back is *shaped* like a live login and is not known to be
    /// one. The [`Session`] inside it holds no access token at all and is
    /// already expired by construction (see
    /// [`Session::from_refresh_token`]), so the first authenticated request
    /// made with it refreshes first; a server that refuses that refresh is the
    /// only thing on this machine that can tell the caller these credentials
    /// are dead.
    #[must_use]
    pub fn load(&self) -> Option<Authenticated> {
        let bytes = std::fs::read(&self.path).ok()?;
        let mut plain = crate::session_store::unprotect(&bytes).ok()?;
        let parsed = parse(&plain);
        // Wipe the intermediate plaintext whether or not it parsed, rather
        // than letting a decrypted master key linger in freed heap memory.
        plain.zeroize();
        parsed
    }

    /// Deletes the file, if it is there.
    ///
    /// The caller's move when the server refuses the stored credentials, when
    /// the account is switched away from, and when
    /// `use_official_bw_crypto` is turned back on -- a master key kept on disk
    /// for a backend that is no longer selected is a secret held for no
    /// reason.
    ///
    /// A missing file is success: "there is no stored key" is the state being
    /// asked for, and reporting an error for already being in it would push a
    /// caller into ignoring the result.
    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// The record, back into an [`Authenticated`]. `None` for anything that is
/// not exactly the layout [`UserKeyStore::save`] writes.
///
/// A free function rather than a method so that every length check is done
/// against a single borrowed slice with no indexing that could panic: there is
/// no `unwrap`, no `[..]` range and no slice index in here, because the bytes
/// this walks came off a disk and, one DPAPI unwrap earlier, out of a file a
/// previous version of this app wrote.
fn parse(plain: &[u8]) -> Option<Authenticated> {
    if plain.len() < HEADER_LEN {
        return None;
    }
    let (magic, rest) = plain.split_at(MAGIC.len());
    if magic != MAGIC {
        return None;
    }
    let (version, rest) = rest.split_first()?;
    if *version != VERSION {
        return None;
    }
    let (key, rest) = rest.split_at(MASTER_KEY_LEN);
    let (len, refresh) = rest.split_at(4);

    let mut key_bytes = [0u8; MASTER_KEY_LEN];
    key_bytes.copy_from_slice(key);
    // `from_bytes` moves this into a `Zeroizing`; wipe the stack copy too, so
    // the key exists unwiped in exactly nowhere.
    let master_key = MasterKey::from_bytes(key_bytes);
    key_bytes.zeroize();

    let len_bytes: [u8; 4] = len.try_into().ok()?;
    let refresh_len = u32::from_le_bytes(len_bytes) as usize;
    // Exact, not "at least": a record with trailing bytes is not a record this
    // module wrote, and reading the prefix of one would be guessing.
    if refresh.len() != refresh_len {
        return None;
    }
    // `save` declines to write an empty token, so a record carrying one did
    // not come from this module. Refusing it keeps the invariant a caller
    // relies on: a loaded record always has something to refresh with.
    if refresh.is_empty() {
        return None;
    }
    let refresh = Zeroizing::new(String::from_utf8(refresh.to_vec()).ok()?);

    Some(Authenticated {
        session: Session::from_refresh_token(refresh),
        master_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temp path, never `%APPDATA%\Deskwarden` and never a real account
    /// directory -- the same rule `session_store`'s and `favicon`'s tests
    /// follow. The counter keeps two tests in one process off one path.
    fn temp_path() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "deskwarden-test-userkey-{}-{}.bin",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A login-shaped value with no server and no network behind it. The key
    /// bytes are a recognisable pattern so the round-trip assertion is about
    /// these bytes and not about any thirty-two.
    fn fixture(refresh: &str) -> Authenticated {
        Authenticated {
            session: Session::from_refresh_token(Zeroizing::new(refresh.to_string())),
            master_key: MasterKey::from_bytes([0xA5; MASTER_KEY_LEN]),
        }
    }

    #[test]
    fn round_trips_a_key_and_a_refresh_token_through_dpapi_and_disk() {
        let path = temp_path();
        let store = UserKeyStore::new(path.clone());

        assert!(store.save(&fixture("the-refresh-token")).expect("save"));
        let loaded = store.load().expect("a record was written");

        assert_eq!(loaded.master_key.expose_bytes(), &[0xA5; MASTER_KEY_LEN]);
        assert_eq!(
            loaded.session.expose_refresh_token().map(|t| t.as_str()),
            Some("the-refresh-token")
        );
        store.clear().expect("clear");
    }

    /// The revived session must arrive already needing a refresh and able to
    /// have one, or `RestClient::refreshing` would send the empty access token
    /// and wait for a 401 to find out what it already knows.
    #[test]
    fn a_revived_session_asks_to_be_refreshed_before_its_first_request() {
        let path = temp_path();
        let store = UserKeyStore::new(path.clone());
        store.save(&fixture("t")).expect("save");

        let loaded = store.load().expect("a record was written");
        assert!(loaded.session.can_refresh());
        assert!(loaded.session.needs_refresh_at(std::time::Instant::now()));
        store.clear().expect("clear");
    }

    #[test]
    fn load_returns_none_when_there_is_no_file() {
        assert!(UserKeyStore::new(temp_path()).load().is_none());
    }

    /// Every corruption is the same answer, and the answer is "ask for the
    /// master password". Each case is written as bytes on disk rather than as
    /// a parse call, so the DPAPI unwrap is in the path too.
    #[test]
    fn every_malformed_record_loads_as_none() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("shorter than the header", vec![b'D'; HEADER_LEN - 1]),
            ("wrong magic", {
                let mut v = vec![0u8; HEADER_LEN];
                v[..4].copy_from_slice(b"XXXX");
                v[4] = VERSION;
                v
            }),
            ("a version this build does not know", {
                let mut v = vec![0u8; HEADER_LEN];
                v[..4].copy_from_slice(MAGIC);
                v[4] = VERSION + 1;
                v
            }),
            ("a length field longer than the bytes that follow", {
                let mut v = Vec::new();
                v.extend_from_slice(MAGIC);
                v.push(VERSION);
                v.extend_from_slice(&[7u8; MASTER_KEY_LEN]);
                v.extend_from_slice(&99u32.to_le_bytes());
                v.extend_from_slice(b"short");
                v
            }),
            ("trailing bytes past the declared length", {
                let mut v = Vec::new();
                v.extend_from_slice(MAGIC);
                v.push(VERSION);
                v.extend_from_slice(&[7u8; MASTER_KEY_LEN]);
                v.extend_from_slice(&1u32.to_le_bytes());
                v.extend_from_slice(b"ab");
                v
            }),
            ("a refresh token that is not UTF-8", {
                let mut v = Vec::new();
                v.extend_from_slice(MAGIC);
                v.push(VERSION);
                v.extend_from_slice(&[7u8; MASTER_KEY_LEN]);
                v.extend_from_slice(&2u32.to_le_bytes());
                v.extend_from_slice(&[0xFF, 0xFE]);
                v
            }),
        ];

        for (name, plain) in cases {
            let path = temp_path();
            let protected = crate::session_store::protect(&plain).expect("DPAPI");
            std::fs::write(&path, protected).expect("write");
            let store = UserKeyStore::new(path);
            assert!(store.load().is_none(), "{name} should not load");
            store.clear().expect("clear");
        }
    }

    /// Bytes that were never DPAPI-wrapped at all -- a truncated file, or
    /// somebody else's. The unwrap fails and the answer is the same `None`.
    #[test]
    fn a_file_that_was_never_protected_loads_as_none() {
        let path = temp_path();
        std::fs::write(&path, b"not a DPAPI blob").expect("write");
        let store = UserKeyStore::new(path);
        assert!(store.load().is_none());
        store.clear().expect("clear");
    }

    /// A session with nothing to refresh with cannot survive a restart, so no
    /// file is written -- and, importantly, no *stale* file is left behind
    /// either.
    ///
    /// The case driven here is the empty token rather than the absent one.
    /// They are the same refusal in [`UserKeyStore::save`], and the empty one
    /// is the half a test can reach without a `cfg(test)` constructor in
    /// `rest::api` -- which this crate bans, and which would be a seam in
    /// production code for the sake of one assertion.
    #[test]
    fn a_session_that_cannot_be_refreshed_is_declined_rather_than_written() {
        let path = temp_path();
        let store = UserKeyStore::new(path.clone());

        assert!(!store.save(&fixture("")).expect("save"));
        assert!(!path.exists(), "nothing may be written for a dead session");
    }

    #[test]
    fn clear_is_happy_when_the_file_is_already_gone() {
        UserKeyStore::new(temp_path()).clear().expect("clearing nothing is fine");
    }

    /// Two stores at two paths do not see each other's records. The property
    /// the per-account layout exists for, asserted here at the store rather
    /// than only at `accounts::user_key_path_for`.
    #[test]
    fn two_accounts_paths_hold_two_independent_records() {
        let a = UserKeyStore::new(temp_path());
        let b = UserKeyStore::new(temp_path());

        a.save(&Authenticated {
            session: Session::from_refresh_token(Zeroizing::new("a-token".to_string())),
            master_key: MasterKey::from_bytes([0xAA; MASTER_KEY_LEN]),
        })
        .expect("save a");
        b.save(&Authenticated {
            session: Session::from_refresh_token(Zeroizing::new("b-token".to_string())),
            master_key: MasterKey::from_bytes([0xBB; MASTER_KEY_LEN]),
        })
        .expect("save b");

        assert_eq!(
            a.load().expect("a").master_key.expose_bytes(),
            &[0xAA; MASTER_KEY_LEN]
        );
        assert_eq!(
            b.load().expect("b").master_key.expose_bytes(),
            &[0xBB; MASTER_KEY_LEN]
        );

        // Clearing one leaves the other alone.
        a.clear().expect("clear a");
        assert!(a.load().is_none());
        assert!(b.load().is_some());
        b.clear().expect("clear b");
    }

    /// The path this store is pointed at is the only file it touches. Stated
    /// as a test because the module doc's whole argument for taking a path is
    /// that a store must not write anywhere it resolved for itself.
    #[test]
    fn save_writes_only_the_path_it_was_given() {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-test-userkey-dir-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let before: Vec<_> = std::fs::read_dir(&dir)
            .expect("read")
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();

        let path = dir.join("userkey.bin");
        let store = UserKeyStore::new(path.clone());
        store.save(&fixture("t")).expect("save");

        let mut after: Vec<_> = std::fs::read_dir(&dir)
            .expect("read")
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect();
        after.retain(|n| !before.contains(n));
        assert_eq!(after, vec![std::ffi::OsString::from("userkey.bin")]);

        std::fs::remove_dir_all(&dir).ok();
    }
}

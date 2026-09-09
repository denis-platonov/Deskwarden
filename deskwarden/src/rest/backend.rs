//! [`VaultBackend`] over a Bitwarden server, with no `bw` CLI anywhere.
//!
//! [`crate::rest::api`] is the HTTP, [`crate::rest::sync`] the read mapping
//! and [`crate::rest::write`] the write mapping. This file is the *seam*: the
//! twenty-one operations of [`VaultBackend`] expressed in terms of those
//! three,
//! and nothing else. There is no route, no header and no cryptography here --
//! if you are looking for one, it is in one of those modules.
//!
//! # Where the keys live, and why they live here
//!
//! [`VaultBackend`]'s doc says session and credentials belong to `bw_serve`
//! and `session_store`, and that is true of the backend those two were written
//! for: `bw` holds the master password and this process holds only a
//! DPAPI-wrapped session token, so there is nothing else to keep.
//!
//! A direct-REST backend has two things `bw serve` never handed over -- an
//! OAuth session and the **master key** the vault is unwrapped with -- and
//! [`crate::rest::api`] already says who owns them: they arrive together in
//! one [`Authenticated`], "because neither is any use alone". So they are
//! held **in this object**, which is the thing that has to have them, for as
//! long as the object lives. Nothing here is static, nothing is global,
//! nothing is written to disk, and no constructor in this file derives a key:
//! the caller does the login and hands the result in. That is the whole of the
//! answer, and the trait's shape carries it without a change -- a backend is
//! an object, and an object may own what its methods need.
//!
//! # What one call costs
//!
//! **Every operation that reads a LIST begins with `GET /api/sync` and a
//! decryption of the whole vault.** On `bw serve` a `list_items` is one
//! loopback request against a process that already holds the plaintext; here
//! it is a WAN round trip carrying every cipher the account has, followed by
//! an AES-CBC decrypt and an HMAC verification of every field of every one of
//! them.
//!
//! **The per-item operations no longer do.** `get_item` and `update_item` --
//! and therefore `move_item_to_folder` and `set_app_match`, and every star the
//! window toggles, which are all `update_item` with one field changed -- read
//! `GET
//! /api/ciphers/{id}`, one record. That sentence used to be "there is no
//! per-item endpoint", which was false; see [`RestBackend::get_item`] for the
//! correction and for what believing it cost. In rows on the owner's
//! Cloudflare D1 server, measured with `wrangler d1 insights`: a sync is about
//! **3,374** rows and the free tier's day is five million, so starring an item
//! used to cost the same as loading the whole vault. Six methods still sync,
//! and every one of them is a genuine whole-vault question -- `list_items`,
//! `list_folders`, `list_vault`, `list_trash`, `list_archive`, and `get_totp`'s
//! seed fallback.
//!
//! That is not hidden behind a cache in this file, deliberately.
//! [`crate::vault_cache::VaultCache`] **is** the cache this app has, it is the
//! thing call sites already go through, and a second cache underneath it is
//! how two caches come to disagree about a vault. The cost is stated instead,
//! per operation, in each method's doc, so a caller choosing between two ways
//! to ask the same question can see which one is cheaper.
//!
//! The **one** thing this file does hold on to is the account's unwrapped
//! [`crate::rest::sync::VaultKeys`], and that is not the cache the paragraph
//! above refuses: no item, no folder and no plaintext is kept, and a key is
//! not a thing that changes on every write the way a vault is. It is there
//! because a single-cipher response carries no `profile` and so has nothing to
//! unwrap a key from. [`RestBackend::keys`] argues the invalidation case by
//! case.
//!
//! The call sites worth knowing about, because they are the ones that were
//! free before:
//!
//! * [`crate::vault_cache::VaultCache::populate`] takes the whole vault in
//!   **one** sync, through [`VaultBackend::list_vault`]. It used to call
//!   `list_items` and then `list_folders` -- two full syncs and two
//!   whole-vault decrypts to read two halves of one payload.
//! * [`crate::bw_serve::wait_for_vault_ready`] polls `list_items` on a retry
//!   schedule -- one full sync per attempt. **Not reached on this backend
//!   any more**: it exists to wait out a `bw serve` cold start, there is no
//!   `bw serve` here, and
//!   [`crate::backend_policy::may_skip_the_readiness_probe`] now says so
//!   before the probe is spawned. It was the largest of the three costs, and
//!   the only one nothing logged.
//! * `VaultCache`'s restore/unarchive path reads the item back with
//!   `get_item` to refresh its `revisionDate` -- **one full sync per gesture,
//!   and no longer.** It is one `GET /api/ciphers/{id}` now, which is what
//!   that line always should have described.
//!
//! Together those two changes are the difference between three full syncs and
//! one on a cold vault window; on the owner's 1,668-item account that was
//! about fifteen seconds against about one.
//!
//! # What this backend refuses
//!
//! **Nothing.** All twenty-one operations are implemented.
//!
//! There were six refusals, and they are worth reading about in the order
//! they were lifted, because in every case the refusal named the trap the
//! implementation then had to avoid rather than merely being replaced. The
//! shared half of all six was that this crate would rather crash than answer
//! a question it cannot answer, and an `Ok` for a write that did not happen
//! is the quietest possible wrong answer -- so each of the six is now a write
//! whose *answer is read back*:
//!
//! * [`RestBackend::archive_item`] and [`RestBackend::unarchive_item`] refused
//!   for want of a route, and warned that faking one with an edit that sets
//!   the server-assigned `archivedDate` would report success for a write the
//!   server ignores. They now use real endpoints in [`crate::rest::api`] --
//!   `PUT /api/ciphers/{id}/archive` and `.../unarchive`, the per-id routes
//!   the target server actually implements -- and the cipher the server
//!   echoes back is *read*, because that stamp is server-assigned and no
//!   status can report it. A cipher the server does not return archived is an
//!   error, not an `Ok`.
//! * [`RestBackend::generate`] refused because there is no endpoint anywhere
//!   and implementing it meant this crate deciding how strong its passwords
//!   are. That decision was taken, and it was taken **outside this module**:
//!   [`crate::password_gen`] is the generator, so that every backend reaches
//!   one implementation -- passphrases included, from a word list installed
//!   beside the executable and read only when one is asked for.
//! * [`RestBackend::create_folder`], [`RestBackend::update_folder`] and
//!   [`RestBackend::delete_folder`] refused for want of a folder endpoint,
//!   and warned that a folder name is encrypted under the user key and that
//!   deleting a folder has the user's whole vault downstream of it. The
//!   routes are now in [`crate::rest::api`]; the name is encrypted by
//!   [`crate::rest::write::encrypt_folder_name`], so this file holds no
//!   ciphertext and no second encryptor; the two writes that get an answer
//!   have it **decrypted and compared to the name that was sent** (see
//!   `confirmed_folder`); and the delete edits no cipher, because the server
//!   un-files the items itself and a client-side sweep would be a second,
//!   partial opinion about a change already made.
//!
//! The one place the six do **not** agree with each other is what an empty
//! response body means, and that disagreement is deliberate. Every one of
//! these routes is path-scoped, so the shape of the URL is not what separates
//! them -- what each call *asserts* is. An archive asserts the value of a
//! server-assigned `archivedDate`, which only a body can carry, so an empty
//! one is [`RestError::ArchiveNotConfirmed`]. A `delete_folder` asserts that
//! a folder is gone, which the status already says, so an empty body is
//! success. Both sides of it are argued in
//! [`crate::rest::api::RestClient::delete_folder`].

use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use crate::app_match::AppMatch;
use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::CryptoError;
use crate::rest::sync::{
    DecryptedItem, DecryptedVault, VaultKeys, decrypt_cipher, decrypt_folder, decrypt_vault,
};
use crate::rest::write::{encrypt_folder_name, encrypt_item};
use crate::vault_backend::VaultBackend;
use crate::password_gen::PasswordGenError;
use crate::vault_bridge::{
    Folder, GenerateRequest, NewItem, VaultError, VaultItem, with_app_match,
};
// The seed reader and the arithmetic it feeds both live beside the Add-a-TOTP
// screen, which is the one surface in this app that computes a code without a
// vault to ask. This backend is the second caller and the vault window's poll
// is the third; see `read_seed`'s own doc.
use crate::vault_window::totp_add::read_seed;

/// The name every refusal in this file signs itself with.
///
/// One constant rather than six literals: a log reader grepping for the
/// backend that refused should find every line, and a rename should not be
/// able to leave five of them behind.
const BACKEND: &str = "the direct-REST vault backend";

/// A Bitwarden server, as one of this app's vault backends.
///
/// Construct with [`RestBackend::new`] from a client and a completed login.
pub struct RestBackend {
    client: RestClient,
    /// The session and the master key, behind a lock.
    ///
    /// **A `Mutex` and not a `RwLock`**, because there is no read side: every
    /// authenticated call in [`crate::rest::api`] takes `&mut Session` -- it
    /// may refresh the access token underneath the request -- so every method
    /// here is a writer. A `RwLock` would be a lock whose read half nothing
    /// could ever take.
    ///
    /// The trait is `Send + Sync` and `VaultCache` is shared across threads
    /// (see [`VaultBackend`]'s doc), so the state has to be behind something;
    /// this is the cheapest something that is correct. It also serialises the
    /// token refresh, which is the behaviour wanted anyway: two threads
    /// refreshing one session concurrently is how a refresh token gets spent
    /// twice.
    state: Mutex<Authenticated>,
    /// The vault keys the last `GET /api/sync` produced, so that reading
    /// **one** record need not fetch the whole vault to learn them.
    ///
    /// # This is not the cache the module docs refuse
    ///
    /// The paragraph above says a second cache under
    /// [`crate::vault_cache::VaultCache`] is how two caches come to disagree
    /// about a vault, and that stands: **no item, no folder and no plaintext
    /// is held here.** What is held is the answer to
    /// `VaultKeys::unwrap_from(master_key, profile)`, which is a pure function
    /// of two things -- the master key this object owns, and the account's
    /// wrapped keys. Two caches can disagree about a vault because a vault
    /// changes on every write; a key does not change at all except by the
    /// events enumerated below, and every one of them is arranged to be
    /// unable to leave a stale value here.
    ///
    /// # Why it is safe, event by event
    ///
    /// * **A re-login, and an account switch.** The master key is a field of
    ///   [`Authenticated`], `Authenticated` is only ever built by a login in
    ///   [`crate::rest::api`], and nothing in this file writes
    ///   `state.master_key` -- so a different master key is a different
    ///   `Authenticated`, which is a different `RestBackend`, which is a
    ///   different `Mutex` holding `None`. The cache cannot outlive the
    ///   session it was taken under because it is *inside* the object that
    ///   holds that session. There is no static, no global and no keying by
    ///   account id to get wrong.
    /// * **A master-password change.** Bitwarden re-wraps the *same* user key
    ///   under the new master key; the user key itself is untouched, so this
    ///   cache is still correct. The old master key stops opening
    ///   `profile.key`, so the uncached path would in fact fail where this one
    ///   succeeds -- but the security stamp moves too, so the session dies and
    ///   the next request is a `401` either way.
    /// * **An account key rotation** -- the one event that really does change
    ///   the user key. It rotates the security stamp, which invalidates the
    ///   access token *and* the refresh token, so the very next request this
    ///   backend makes is [`RestError::Unauthorized`] and the app signs in
    ///   again into a fresh `RestBackend`. A stale key here never reaches a
    ///   decrypt, let alone a write.
    /// * **An organisation joined since the last sync**, which is the case
    ///   that is *not* covered by any of the above: the user key is unchanged
    ///   and the session is perfectly valid, but the cached [`VaultKeys`] has
    ///   no entry for the new organisation, so a cipher belonging to it does
    ///   not decrypt at all.
    ///
    /// The last one is why the cache is not merely trusted.
    /// [`RestBackend::one_record`] re-derives the keys from a fresh sync and
    /// decrypts again whenever a **cached** key fails to open the record
    /// cleanly, which covers that case and would also cover a rotation on a
    /// hypothetical server that did not invalidate the session. The cost of
    /// that retry is one sync -- exactly what the old code paid
    /// unconditionally -- so the self-healing path can never be worse than
    /// what it replaces, and the healthy path pays nothing.
    ///
    /// **An `Arc` and a second `Mutex`, rather than a field of the state.**
    /// The two locks are never held at once: `remembered_keys` clones the
    /// `Arc` and releases, and `remember_keys` stores and releases. There is
    /// therefore no lock order to get wrong, and no network call happens with
    /// this lock held. Putting the keys inside `state` was the alternative and
    /// was rejected: `write_through`'s closure takes `&mut Authenticated`, so
    /// the state guard is already borrowed mutably across the send, and a
    /// reader wanting only the keys would have queued behind every write.
    keys: Mutex<Option<Arc<VaultKeys>>>,
    /// The last `GET /api/sync` this backend made, with the revision the
    /// server reported just before it.
    ///
    /// # This one IS a vault cache, and the module docs refuse those
    ///
    /// The paragraph above says a second cache under
    /// [`crate::vault_cache::VaultCache`] is how two caches come to disagree
    /// about a vault. That is true of a cache that decides *for itself* how
    /// long its copy is good for. This one decides nothing: before it is
    /// reused, the server is asked what the account's revision is, and the
    /// copy is used only if the answer is byte-identical to the one recorded
    /// when it was taken. A cache that re-validates against the authority on
    /// every read cannot hold an opinion the authority does not.
    ///
    /// # What it costs and what it saves
    ///
    /// The check is `GET /api/accounts/revision-date` -- one primary-key
    /// lookup, **one row** on the owner's D1-backed server. The sync it
    /// avoids was measured at ~1,686 rows, and `synced` had five callers on
    /// paths that are not "load the vault": listing the Trash, listing the
    /// Archive, and the two halves of a populate. Opening the Trash row read
    /// the entire account.
    ///
    /// # Why the revision is read BEFORE the sync, not after
    ///
    /// The recorded revision must never be *newer* than the vault beside it.
    /// Read first, a write landing between the two calls leaves the cache
    /// tagged with the older revision, so the next check sees a difference
    /// and refetches -- one wasted sync, and correct. Read after, the same
    /// write would leave a vault that predates its own tag, and the next
    /// check would call it current forever.
    ///
    /// # The write path
    ///
    /// An older comment in this file said caching what `synced()` returns is
    /// the change [`RestBackend::update_item`] must not have, because a write
    /// quotes the `revisionDate` of the sync it just made. That is still the
    /// rule and this does not break it: `update_item` and `set_app_match` go
    /// through [`RestBackend::one_record`], which fetches the record by id and
    /// never touches this field. Nothing that writes reads this cache.
    ///
    /// **An `Arc<DecryptedVault>`**, so a hit hands out a pointer rather than
    /// copying a vault. The two callers that need to own the folder list clone
    /// that list alone.
    synced: Mutex<Option<CachedSync>>,
}

/// One sync, and the revision the server reported immediately before it.
struct CachedSync {
    /// From `GET /api/accounts/revision-date`, compared for equality and
    /// never interpreted -- see [`crate::rest::api::RestClient::revision_date`].
    revision: i64,
    vault: Arc<DecryptedVault>,
    keys: Arc<VaultKeys>,
}

/// Hand-written, and it must be: [`Authenticated`] hand-writes its own for
/// [`crate::debug_leak_guard`]'s reason, and this delegates to it rather than
/// deriving something that would print a [`RestClient`]'s base URL beside a
/// redacted session for no gain.
impl std::fmt::Debug for RestBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestBackend").field("state", &"<locked>").finish()
    }
}

impl RestBackend {
    /// One server, one logged-in account.
    ///
    /// `authenticated` comes from [`RestClient::authenticate`]. This function
    /// deliberately does **not** perform the login itself: a constructor that
    /// took a master password would be a constructor that had to be given one
    /// again on every re-auth, and the session lifecycle is the caller's --
    /// exactly as [`VaultBackend`]'s doc says, `sync`, `lock` and `unlock` are
    /// not operations on this trait.
    #[must_use]
    pub fn new(client: RestClient, authenticated: Authenticated) -> Self {
        // `None`, and it must be: a constructor that pre-warmed the key cache
        // would be a constructor that fetches, which is exactly what
        // `signed_in_with_no_sync_route`'s doc relies on not happening.
        Self {
            client,
            state: Mutex::new(authenticated),
            keys: Mutex::new(None),
            synced: Mutex::new(None),
        }
    }

    // ---- the two things every method starts with ---------------------------

    /// The locked state.
    ///
    /// A poisoned lock is recovered from rather than propagated: the guarded
    /// value is a session and a key, not a half-updated invariant, and the
    /// only way to poison it is a panic in one of the short bodies below.
    /// Refusing every later vault operation because an unrelated thread
    /// panicked once would turn a recoverable fault into a dead app -- and it
    /// would do it by way of an `unwrap`, which this module does not have.
    fn locked(&self) -> std::sync::MutexGuard<'_, Authenticated> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// **The keys alone**, from a `GET /api/sync` and without decrypting a
    /// single cipher.
    ///
    /// [`VaultKeys::unwrap_from`] needs `response.profile` and nothing else,
    /// so the three write paths that want a key and no items -- a folder
    /// create, a folder rename, an item create -- were paying a whole-vault
    /// decrypt (every field of every cipher, under every organisation key)
    /// and then dropping the result on the floor. The round trip is the same;
    /// what this saves is the CPU and the plaintexts that were briefly built
    /// for nobody.
    ///
    /// [`Self::synced`] is still what a *vault load* needs; an **edit** wants
    /// neither, and goes through [`Self::one_record`].
    ///
    /// Every sync that unwraps a key stores it in [`Self::keys`] on the way
    /// past, which is what makes the one-record path free after a vault has
    /// been listed once. See that field for why a stored key cannot go stale
    /// unnoticed.
    fn keys_only(&self) -> Result<Arc<VaultKeys>, VaultError> {
        let mut state = self.locked();
        let response = self.client.sync_refreshing(&mut state.session).map_err(rest_error)?;
        let profile = response
            .profile
            .as_ref()
            .ok_or_else(|| VaultError::Parse("the sync payload carries no profile".to_string()))?;
        let (keys, failures) =
            VaultKeys::unwrap_from(&state.master_key, profile).map_err(crypto_error)?;
        if !failures.is_empty() {
            log::warn!(
                "{} organisation key(s) on this account could not be unwrapped: {failures:?}",
                failures.len()
            );
        }
        // The state guard is dropped before the second lock is taken. The two
        // are never held together anywhere in this file; see `Self::keys`.
        drop(state);
        Ok(self.remember_keys(keys))
    }

    /// **One `GET /api/sync`, decrypted**: every item, every folder, and the
    /// keys the write path needs to put anything back.
    ///
    /// The keys are unwrapped a second time here, beside [`decrypt_vault`]'s
    /// own unwrap. That is one AES-CBC decrypt and one HMAC over sixty-four
    /// bytes -- next to nothing against the sync that just happened -- and it
    /// buys keeping [`decrypt_vault`]'s signature as it is rather than
    /// widening the read path's return type for the sake of the write path.
    ///
    /// Decryption failures are logged, not returned. A vault with one corrupt
    /// field is still a vault, which is [`crate::rest::sync`]'s own decision;
    /// what this adds is that the fact reaches a log line instead of being
    /// dropped on the floor. The log carries **counts and field names only**
    /// -- [`crate::rest::sync::DecryptFailure`] holds nothing else by
    /// construction.
    fn synced(&self) -> Result<(Arc<DecryptedVault>, Arc<VaultKeys>), VaultError> {
        // **The revision FIRST, and with no other lock held.** One row on the
        // server, against the ~1,686 the sync below costs. Read before the
        // sync so the recorded revision can never be newer than the vault it
        // is filed with -- see `CachedSync`.
        //
        // A failure here is NOT fatal and is not even reported: the whole
        // point of this call is to avoid work, so a server that will not
        // answer it just means the sync happens, exactly as it always did.
        let revision = {
            let mut state = self.locked();
            self.client.revision_date(&mut state.session).ok()
        };
        if let Some(revision) = revision {
            if let Ok(held) = self.synced.lock() {
                if let Some(cached) = held.as_ref() {
                    if cached.revision == revision {
                        return Ok((Arc::clone(&cached.vault), Arc::clone(&cached.keys)));
                    }
                }
            }
        }

        let mut state = self.locked();
        let response = self.client.sync_refreshing(&mut state.session).map_err(rest_error)?;
        let vault = decrypt_vault(&response, &state.master_key).map_err(crypto_error)?;
        let profile = response
            .profile
            .as_ref()
            .ok_or_else(|| VaultError::Parse("the sync payload carries no profile".to_string()))?;
        let (keys, _) = VaultKeys::unwrap_from(&state.master_key, profile).map_err(crypto_error)?;
        if !vault.failures.is_empty() {
            log::warn!(
                "{} fields of this vault could not be decrypted and are missing from the items \
                 this sync produced: {:?}",
                vault.failures.len(),
                vault.failures
            );
        }
        drop(state);
        let vault = Arc::new(vault);
        let keys = self.remember_keys(keys);
        // Filed under the revision read BEFORE the sync. `None` -- the server
        // would not answer that question -- stores nothing, so the next call
        // syncs again rather than reusing a copy it cannot re-validate.
        if let (Some(revision), Ok(mut held)) = (revision, self.synced.lock()) {
            *held = Some(CachedSync {
                revision,
                vault: Arc::clone(&vault),
                keys: Arc::clone(&keys),
            });
        }
        Ok((vault, keys))
    }

    /// Stores `keys` as the account's current ones and hands back the shared
    /// handle the caller should go on to use.
    ///
    /// It returns what it stored rather than being a `set` followed by a `get`
    /// so that there is no window in which another thread's store could make
    /// a caller use keys it did not derive.
    fn remember_keys(&self, keys: VaultKeys) -> Arc<VaultKeys> {
        let keys = Arc::new(keys);
        let mut slot = self.keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(Arc::clone(&keys));
        keys
    }

    /// The keys a previous sync left behind, if there was one.
    fn remembered_keys(&self) -> Option<Arc<VaultKeys>> {
        self.keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// **One `GET /api/ciphers/{id}`, decrypted, and no sync at all once a
    /// vault has been listed** -- the read behind [`VaultBackend::get_item`]
    /// and behind every edit.
    ///
    /// # The record, and why it is the same record a sync produces
    ///
    /// [`decrypt_cipher`] is the mapper, which is [`decrypt_vault`]'s own
    /// `map_cipher` reached by its other door. So the [`DecryptedItem`] this
    /// returns carries the identical `retained` account of which in-place
    /// values are still the server's ciphertext -- the thing
    /// [`Self::write_through`] lays the modelled fields *over*. A second
    /// mapper here would be how an unmodelled field comes to be rewritten;
    /// there is no second mapper.
    ///
    /// It is also **fresher** than the sync it replaces for the one field
    /// that has to be: `revisionDate` comes off this record, and this record
    /// is the server's copy of exactly the cipher about to be written.
    ///
    /// # The keys, and the one retry
    ///
    /// A single-cipher response carries no `profile`, so there is nothing in
    /// it to unwrap a key from; the keys come from [`Self::keys`] -- see that
    /// field for the whole argument that a stored one cannot be stale.
    ///
    /// The one case that argument does not close by construction is an
    /// organisation joined since the last sync: the session is valid, the user
    /// key is right, and the cached [`VaultKeys`] simply has no entry for the
    /// new organisation, so its ciphers do not decrypt. That is what the retry
    /// below is for. **It fires only when the keys that failed were cached**
    /// -- keys just derived from a sync are as fresh as anything can be, and
    /// retrying them would be an infinite appetite for syncs over a genuinely
    /// corrupt field.
    ///
    /// A retry costs one sync, which is precisely what this whole path used to
    /// cost every time, so the unhappy case is no worse than the code it
    /// replaces and the happy case is free.
    fn one_record(&self, id: &str) -> Result<(DecryptedItem, Arc<VaultKeys>), VaultError> {
        let raw = {
            let mut state = self.locked();
            self.client.fetch_cipher(&mut state.session, id).map_err(|e| match e {
                // The one status worth translating. A server that has no such
                // cipher and `Self::find` failing to see one in a sync are the
                // same fact about the same vault, so they say the same
                // sentence -- and it is the sentence that names the id, which
                // "the server answered 404" does not.
                RestError::Status(404) => no_such_item(id),
                other => rest_error(other),
            })?
        };

        let warm = self.remembered_keys();
        let cached = warm.is_some();
        let keys = match warm {
            Some(keys) => keys,
            None => self.keys_only()?,
        };

        match decrypt_cipher(&raw, &keys) {
            Some((record, failures)) if failures.is_empty() => return Ok((record, keys)),
            // Freshly-derived keys: whatever went wrong is not staleness, and
            // this is the same "a vault with one corrupt field is still a
            // vault" tolerance `synced` applies.
            Some((record, failures)) if !cached => {
                log_record_failures(id, &failures);
                return Ok((record, keys));
            }
            None if !cached => return Err(unreadable_record(id)),
            _ => {}
        }

        log::info!(
            "the cached vault keys did not open item {id} cleanly, so they are being re-derived \
             from a fresh sync; an organisation joined since the last one reads exactly like this"
        );
        let fresh = self.keys_only()?;
        let (record, failures) =
            decrypt_cipher(&raw, &fresh).ok_or_else(|| unreadable_record(id))?;
        log_record_failures(id, &failures);
        Ok((record, fresh))
    }

    /// Encrypts `item` under this vault's keys, carrying `record`'s account of
    /// which of its in-place values are still the server's ciphertext, and
    /// sends it wherever `send` sends it.
    ///
    /// Every write goes through here so that the "lay the modelled fields over
    /// the retained JSON" rule is obeyed in exactly one place, and so that the
    /// mapped cipher -- which is a whole item's worth of ciphertext whose
    /// `Display` prints the wire form -- is never bound to a name anything
    /// could log.
    fn write_through(
        &self,
        record: &DecryptedItem,
        item: VaultItem,
        keys: &VaultKeys,
        send: impl FnOnce(
            &RestClient,
            &mut Authenticated,
            &crate::rest::write::MappedCipher,
        ) -> Result<serde_json::Value, RestError>,
    ) -> Result<VaultItem, VaultError> {
        // **The concurrency token comes from the sync this write just did,
        // not from the caller's copy.**
        //
        // Every caller of this function has already read the server's own copy
        // one statement earlier -- `update_item`, and therefore
        // `move_item_to_folder`, `set_app_match` and every star the window
        // toggles, through `one_record`'s `GET /api/ciphers/{id}`;
        // `create_item` from a record
        // it composed itself -- so `record` is milliseconds old and the item in
        // the caller's hand can be a whole window session old.
        //
        // **That read used to be a whole `/api/sync`, and this comment is the
        // reason the cheap one had to answer with the cipher rather than just
        // its id.** A per-id `GET` returns the record complete with its current
        // `revisionDate`, so the token below is the same value the sync carried
        // and is read one round trip closer to the `PUT`.
        //
        // `revisionDate`
        // is the field where that difference is fatal: a server reads it as
        // "the version you think you are editing", and a superseded one is
        // refused with "The client copy of this cipher is out of date. Resync
        // the client and try again." A star toggled twice met that first,
        // because the second toggle quoted what the first one had already
        // superseded.
        //
        // Dropping the key was tried for one release and is what this
        // replaces: the same refusal came back on an ordinary Save with no
        // token in the body at all. See `crate::rest::write::encrypt_item`.
        //
        // One field and not the whole base, for `with_revision_date_from`'s
        // own reason: callers deliberately shape `other` (an app match, a
        // `deletedDate` kept out of the live snapshot), and the token is the
        // one key where the server's answer is right whatever the caller
        // meant.
        let fresh = crate::vault_bridge::with_revision_date_from(&item, &record.item);
        let cipher = encrypt_item(&record.carrying(fresh), keys).map_err(crypto_error)?;
        let mut state = self.locked();
        let sent = send(&self.client, &mut state, &cipher);
        drop(state);
        // **A refused write says what the body CARRIED, by field name.**
        //
        // The refusal this exists for is "The client copy of this cipher is
        // out of date. Resync the client and try again.", which a server
        // answers when it reads one of these fields as a concurrency token --
        // and which names no field, so the same message has now been chased
        // twice from the message alone. `revisionDate` was the first and is
        // dropped by the mapper; a second refusal after that means a second
        // field, and this is the line that says which fields there were to
        // choose from.
        //
        // Names only. See `MappedCipher::field_names`: this cannot reach a
        // value, and the names are the Bitwarden API's own.
        let answer = sent.map_err(|e| {
            log::warn!(
                "the vault backend refused this write; the body carried the field(s) {:?}",
                cipher.field_names()
            );
            rest_error(e)
        })?;
        // The server's own copy, decrypted: the only source of a created
        // item's id and of a non-stale `revisionDate` after an edit.
        let (written, failures) = decrypt_cipher(&answer, keys).ok_or_else(|| {
            VaultError::Parse("the written cipher the server answered with".to_string())
        })?;
        if !failures.is_empty() {
            log::warn!(
                "the vault backend wrote an item successfully but could not decrypt {} field(s) \
                 of the copy the server answered with: {failures:?}",
                failures.len()
            );
        }
        Ok(written.item)
    }

    /// The whole live vault: neither trashed nor archived.
    ///
    /// **This is a filter and it has to be**, and it is the one place the two
    /// backends' lists could silently diverge. `bw serve` answers three
    /// *disjoint* sets from three query strings; `/api/sync` answers **one**
    /// list containing all three, distinguished by `deletedDate` and
    /// `archivedDate`. See [`RestBackend::list_trash`] on which way that cuts.
    fn live(vault: &DecryptedVault) -> Vec<VaultItem> {
        vault
            .items
            .iter()
            .filter(|d| !is_trashed(&d.item) && !is_archived(&d.item))
            .map(|d| d.item.clone())
            .collect()
    }

    // **`find` is gone with its one caller.** It picked a decrypted cipher out
    // of a whole sync by id, for `get_totp`, which now reads that one record
    // over `GET /api/ciphers/{id}` instead. The refusal it produced has not
    // gone anywhere: `one_record` maps the server's 404 to `no_such_item(id)`,
    // the same sentence naming the same id, so an item that is not there still
    // says so.
}

// ---- the trait ---------------------------------------------------------------

impl VaultBackend for RestBackend {
    /// **Cost: one full sync.** Every live item, in one WAN round trip plus a
    /// whole-vault decryption. See the module docs.
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(Self::live(&vault))
    }

    /// **Cost: one `GET /api/ciphers/{id}`. No sync**, once any vault load has
    /// happened.
    ///
    /// # This was one full sync, and the measurement is why it is not
    ///
    /// This doc used to say there is no per-item endpoint on the Bitwarden
    /// API and that pulling the whole vault is what asking for one item *is*
    /// here. **That was simply wrong**, and it is corrected rather than
    /// quietly deleted because it is what justified the cost. `GET
    /// /api/ciphers/{id}` is the same URL [`crate::rest::api`] already `PUT`s
    /// an edit to; only the verb was missing.
    ///
    /// What the mistake cost is on the record. On the owner's self-hosted
    /// server -- Cloudflare D1, which caps **rows read** -- `wrangler d1
    /// insights` put one `/api/sync` at about 3,374 rows, and the day it hit
    /// the five-million-row limit the server began answering `500 Database not
    /// initialized` to everything. Reading one line per click is the rule;
    /// this method now obeys it.
    ///
    /// So it is `bw serve`'s `GET /object/item/{id}` after all, and the two
    /// backends' costs no longer differ here. `app::fill_from_vault` reaches
    /// this only on a cache miss, and that miss is now one row.
    ///
    /// Trashed and archived items answer too, as `GET /object/item/{id}` does
    /// and as the old sync-and-filter did: the route is by id and knows
    /// nothing of the three lists, so an id the caller holds is an id it may
    /// legitimately ask about whichever list the item is currently in.
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        let (record, _) = self.one_record(id)?;
        Ok(record.item)
    }

    /// **Cost: one full sync.** The folder names ride the same payload as the
    /// ciphers and cannot be asked for on their own, so a `populate` that
    /// wants items and folders pays for the vault twice.
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        let (vault, _) = self.synced()?;
        // Cloned, because the vault behind this is shared -- see the
        // `synced` field. A folder list is a handful of names; the alternative
        // was copying every item beside them on every cache hit.
        Ok(vault.folders.clone())
    }

    /// **Cost: one full sync, for both halves.** This is the method the
    /// module docs' "two full syncs per populate" line was describing the
    /// absence of.
    ///
    /// The ciphers and the folder names arrive in the same `GET /api/sync`
    /// payload and are decrypted by the same `decrypt_vault` pass, so
    /// `list_items` followed by `list_folders` fetches and decrypts the whole
    /// account twice to read two halves of one answer. Here they are simply
    /// both taken off the one `synced()` that already produced them.
    ///
    /// `Self::live` for the items, exactly as `list_items` does -- trashed
    /// and archived ciphers are filtered out of the vault list here and must
    /// stay filtered out when the same list is fetched this way, or the
    /// single-sync path would paint deleted items that the two-call path
    /// hides.
    fn list_vault(&self) -> Result<crate::vault_cache::VaultSnapshot, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(crate::vault_cache::VaultSnapshot {
            items: Self::live(&vault),
            folders: vault.folders.clone(),
        })
    }

    /// **Cost: one full sync, then one `PUT`.**
    ///
    /// [`with_app_match`] and then an ordinary edit -- the same composition
    /// `bw serve`'s backend makes, from the same function, so the custom field
    /// this app writes has one definition and cannot drift between backends.
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.update_item(&with_app_match(item, m))
    }

    /// **Cost: one full sync, then one `POST /api/folders`.**
    ///
    /// The sync is for the keys and nothing else -- a folder name is
    /// [`crate::rest::crypto::encrypt`]ed under the user key, and the user key
    /// arrives on the sync payload, so a create cannot be one request here for
    /// [`Self::create_item`]'s reason exactly.
    ///
    /// # This was a refusal, and what the refusal asked for
    ///
    /// It refused because [`crate::rest::api`] had no folder endpoint and
    /// because "a folder name is encrypted under the user key" is not
    /// something to work out inside a backend method. Both halves are now
    /// answered somewhere that is not this file: the route is in
    /// [`crate::rest::api::RestClient::create_folder`] and the encryption is
    /// [`crate::rest::write::encrypt_folder_name`], which is the crate's one
    /// encryptor reached through the crate's one mapper. **There is no
    /// ciphertext in this method**, and no second way to build a folder body.
    ///
    /// # The answer is read back, and it is read back all the way
    ///
    /// The standard [`Self::archive_item`] set: a status is not a
    /// confirmation. The server's echoed folder is decrypted with
    /// [`decrypt_folder`] -- the *same* mapper a sync uses, so a written
    /// folder and a listed one cannot disagree -- and three things must hold
    /// or this is an error rather than an `Ok`: the answer must be a folder
    /// at all, it must carry a non-empty `id`, and its name must decrypt back
    /// to the name that was asked for. The last one is the one that matters:
    /// it is the only check that would catch a server that stored something
    /// other than what was sent, and it costs one AES-CBC decrypt of a folder
    /// name.
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let keys = self.keys_only()?;
        let body = encrypt_folder_name(name, &keys).map_err(crypto_error)?;
        let mut state = self.locked();
        let answer = self.client.create_folder(&mut state.session, &body).map_err(rest_error)?;
        drop(state);
        confirmed_folder(&answer, name, None, &keys)
    }

    /// **Cost: one full sync, then one `PUT /api/folders/{id}`.**
    ///
    /// [`Self::create_folder`]'s shape, with the id in the path and one more
    /// thing checked in the answer: the folder the server echoes must be
    /// **the folder that was asked about**. A rename whose answer carries a
    /// different id is not this folder renamed, whatever its status said.
    ///
    /// **An id this vault does not hold is not created here.** The Bitwarden
    /// folder `PUT` is not an upsert, and this method does not turn one into
    /// one; a server that answers `404` reaches the caller as an error, which
    /// is what a rename of something that is gone should be.
    ///
    /// The whole record is replaced, which for a folder is the name -- see
    /// [`crate::rest::write::encrypt_folder_name`] on why that is a sentence
    /// about a one-field model and not the cipher hazard in miniature.
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        let keys = self.keys_only()?;
        let body = encrypt_folder_name(name, &keys).map_err(crypto_error)?;
        let mut state = self.locked();
        let answer =
            self.client.update_folder(&mut state.session, id, &body).map_err(rest_error)?;
        drop(state);
        confirmed_folder(&answer, name, Some(id), &keys)
    }

    /// **Cost: one `DELETE /api/folders/{id}`. No sync.**
    ///
    /// Nothing is encrypted, so nothing needs a key: the id is the whole
    /// request, as it is for [`Self::delete_item`].
    ///
    /// # The items in the folder are not deleted, and this backend does not
    /// # touch them
    ///
    /// The refusal this replaces warned that a half-working guess here would
    /// be a guess with the user's whole vault downstream of it, so the fact is
    /// worth stating where it is now true: deleting a folder on Bitwarden
    /// **un-files** the items in it. Their `folderId` is cleared server-side
    /// and they appear, intact, under no folder on the next sync. Nothing here
    /// edits a cipher to make that happen -- a client-side sweep would be a
    /// second opinion about a change the server already makes, and a partial
    /// sweep is how items go missing.
    ///
    /// `bw serve` forwards `DELETE /object/folder/{id}` to this same server
    /// route and likewise edits no item, so **the two backends agree**: a user
    /// who deletes a folder on either one keeps every item that was in it.
    ///
    /// # An empty answer is a success
    ///
    /// Deliberately, and it is the opposite of what an empty answer means to
    /// [`Self::archive_item`]. The argument is
    /// [`crate::rest::api::RestClient::delete_folder`]'s: archive is a bulk
    /// route whose body is its only per-id evidence, while this route's id is
    /// in the path and its status *is* the answer about that id. Treating a
    /// bodiless `204` as a failure here would report a delete that worked as
    /// one that did not, and send the caller back to delete it again.
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.delete_folder(&mut state.session, id).map_err(rest_error)
    }

    /// **Cost: one full sync, then one `POST /api/ciphers`.**
    ///
    /// The sync is needed for the keys, which is the whole of why a create is
    /// not one request here: nothing can be encrypted before the user key is
    /// unwrapped, and the user key arrives on the sync payload.
    ///
    /// The body is built by turning [`NewItem::to_payload`] -- the exact JSON
    /// the `bw serve` backend POSTs, and the one place the "blank means
    /// absent" rule and every wire key name are written -- into a
    /// [`VaultItem`], and encrypting that. Going through the payload rather
    /// than hand-building a `VaultItem` per variant is what keeps the two
    /// backends creating the *same item*: a new `NewItem` variant is picked up
    /// here for free, and a shape fix lands in both at once.
    ///
    /// The item is [`DecryptedItem::newly_composed`], which is exactly true --
    /// nothing in it came from a server -- and is the safe direction for the
    /// two in-place values: every one of them is encrypted.
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        let keys = self.keys_only()?;

        let mut payload = new_item.to_payload();
        // `VaultItem::id` is not optional, and a create has no id yet.
        // `encrypt_item` removes an empty one from the body rather than
        // sending `""`, which is the shape the API wants.
        if let Some(object) = payload.as_object_mut() {
            object.insert("id".to_string(), serde_json::Value::String(String::new()));
        }
        let item: VaultItem = serde_json::from_value(payload)
            .map_err(|e| VaultError::Parse(format!("the new item's own payload: {e}")))?;

        let fresh = DecryptedItem::newly_composed(item.clone());
        self.write_through(&fresh, item, &keys, |client, state, cipher| {
            client.create_cipher(&mut state.session, cipher)
        })
    }

    /// **Cost: one `GET /api/ciphers/{id}`, then one `PUT` to the same URL. No
    /// sync**, once any vault load has happened.
    ///
    /// # This was the expensive one, and it was the commonest
    ///
    /// It began with a full `/api/sync`, and so did every gesture that goes
    /// through it: [`Self::move_item_to_folder`], [`Self::set_app_match`] and
    /// the window's star are all this method with one field changed. On
    /// the owner's Cloudflare D1 server that made **starring an item** cost
    /// about 3,374 rows read, and a day of ordinary editing is what took that
    /// account past the five-million-row cap into `500 Database not
    /// initialized`.
    ///
    /// The read is still not optional and is still not a cache miss -- it
    /// supplies the item's decryption record, which the trait's signature
    /// cannot carry (see [`DecryptedItem::carrying`]). An edit sent without
    /// the record would either bury a field that never decrypted or, worse in
    /// the other direction, write one in the clear -- the failure
    /// [`crate::rest::write`]'s module docs are mostly about. What changed is
    /// that the record is fetched **by id** instead of being filtered out of
    /// the whole account, through [`Self::one_record`], which also carries the
    /// answer to where the keys come from when the response has no `profile`.
    ///
    /// # The concurrency token is strictly better, not merely preserved
    ///
    /// [`Self::write_through`] lays the freshly-read `revisionDate` over the
    /// caller's item, and that is what stops the server refusing the write
    /// with "The client copy of this cipher is out of date." A single-cipher
    /// `GET` answers with *this* cipher's current `revisionDate`, which is the
    /// same value the sync carried for it and is read closer to the `PUT` --
    /// so the token is at least as fresh as before and 3,374 rows cheaper.
    /// `a_write_quotes_the_revision_date_of_the_read_and_not_the_callers_stale_one`
    /// is the pin.
    ///
    /// **An id this vault does not hold is refused rather than created.**
    /// `PUT` on Bitwarden is not an upsert, and a create dressed as an edit
    /// would be an item with no `revisionDate` history and a caller that
    /// believes it edited something. The refusal now comes from the server's
    /// own `404` on the `GET` rather than from a miss in a downloaded vault,
    /// which is the same answer arrived at one round trip earlier -- and it
    /// still happens **before** anything is sent.
    ///
    /// Returns the server's copy, for the reason
    /// [`crate::vault_bridge::VaultBridge::update_item`] gives at length: the
    /// `revisionDate` in the caller's hand is stale from the moment the write
    /// lands, and the next edit of the item is refused if it is kept.
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        let (record, keys) = self.one_record(&item.id)?;
        let id = item.id.clone();
        self.write_through(&record, item.clone(), &keys, move |client, state, cipher| {
            client.update_cipher(&mut state.session, &id, cipher)
        })
    }

    /// **Cost: one full sync, then one `PUT`.**
    ///
    /// # A real difference between the two backends, in the caller's favour
    ///
    /// `bw serve` needed [`crate::vault_bridge::folder_move_body`] and a whole
    /// paragraph of reasoning, because that backend **merges** a `PUT` and
    /// silently ignores a null `folderId` -- so un-filing an item could not be
    /// said at all in the ordinary edit body. The Bitwarden API replaces the
    /// whole cipher, so "no `folderId` key" means no folder, and un-filing is
    /// just an edit. That is why this is `update_item` with one field changed
    /// and not a path of its own.
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        let mut moved = item.clone();
        moved.folder_id = folder_id.map(std::string::ToString::to_string);
        self.update_item(&moved)
    }

    /// **`true`, and it is the reason this capability exists.**
    ///
    /// The paragraph above is the whole argument: this backend's `PUT`
    /// replaces the cipher, `rest::write` omits the `folderId` key for a
    /// `None`, and an omitted key on a replacing write is an absent folder.
    /// `bw serve` answers `false` because the same three spellings there were
    /// measured to change nothing.
    ///
    /// So "Move to folder → No folder" is a real destination on this
    /// backend, and the surfaces that offer it -- `item_list::move_menu`, the
    /// detail pane's kebab and `EditDraft::may_unfile` -- ask this rather
    /// than withholding it from everyone.
    fn can_unfile_items(&self) -> bool {
        true
    }

    /// **Cost: one `PUT /api/ciphers/{id}/delete`. No sync.**
    ///
    /// A **soft** delete -- the item goes to the trash and
    /// [`Self::restore_item`] brings it back. That matches `bw serve`'s
    /// `delete_item`, which is also the soft one; the irreversible one is
    /// [`Self::purge_item`] on both backends.
    ///
    /// One of the four operations here that needs no sync at all, because it
    /// needs no key: the id is the whole request.
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.trash_cipher(&mut state.session, id).map_err(rest_error)
    }

    /// **Cost: one full sync.**
    ///
    /// # The one place the two backends' lists are built oppositely
    ///
    /// `bw serve` answers `?trash=true` with a set **disjoint** from its
    /// default list -- its own doc records that measuring this was necessary,
    /// because two plausible spellings of the query are silently ignored and
    /// answer with the entire vault. `/api/sync` has no query at all: it
    /// returns one list holding live, trashed and archived ciphers together,
    /// and `deletedDate` is what tells them apart.
    ///
    /// So on this backend a trash listing is a **filter**, and the mistake
    /// available here is the mirror image of the one `bw serve` guards
    /// against: a filter that is dropped or inverted shows the user their
    /// whole vault under "Trash", or shows an empty trash that is not empty.
    /// `list_items` above carries the other half of the same filter, and the
    /// two must stay complementary.
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(vault.items.iter().filter(|d| is_trashed(&d.item)).map(|d| d.item.clone()).collect())
    }

    /// **Cost: one full sync.** The archived items, by `archivedDate`.
    ///
    /// This is a genuine read of the payload and not a refusal in disguise,
    /// even though [`Self::archive_item`] is refused: an item archived by
    /// another Bitwarden client appears here, which is exactly what the user
    /// would expect to see.
    ///
    /// **It answers empty on a server that has no archive**, and that is not
    /// this backend inventing anything -- it is the same answer the same
    /// payload gives every other client. The server this crate was written
    /// against documents a good deal as not implemented; if `archivedDate`
    /// never appears, no item is archived, and saying so is correct.
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(vault.items.iter().filter(|d| is_archived(&d.item)).map(|d| d.item.clone()).collect())
    }

    /// **Cost: one `PUT /api/ciphers/{id}/archive`. No sync.**
    ///
    /// # This was a refusal, and what changed is the route, not the argument
    ///
    /// It refused because [`crate::rest::api`] had no archive endpoint, and
    /// because the two ways of faking one are both worse than saying no. That
    /// reasoning still stands and is worth keeping in view, because it is
    /// what the implementation now has to satisfy rather than sidestep:
    ///
    /// * **An ordinary edit setting `archivedDate` is still forbidden.**
    ///   That field is server-assigned; a full-replace `PUT` carrying one is
    ///   at best ignored, which returns `Ok` on an item that stayed exactly
    ///   where it was. Nothing below goes near [`Self::update_item`].
    /// * **A call read only for its status is the same defect wearing a real
    ///   route.** `archivedDate` is assigned by the server, so a `200` says
    ///   the request was accepted and not that the stamp was written.
    ///   [`crate::rest::api::RestClient::archive_route`] is therefore written
    ///   to read the echoed cipher back and to fail with
    ///   [`crate::rest::api::RestError::ArchiveNotConfirmed`] when it does not
    ///   show the state that was asked for -- and it judges that by
    ///   `archivedDate`, the very field [`Self::list_archive`] filters on, so
    ///   this method and that one cannot come to disagree about what
    ///   "archived" means.
    ///
    /// The per-id signature is met by a per-id route, which is what the
    /// target server has: it puts the id in the path exactly as trash and
    /// restore do, and answers with the whole updated cipher. See that
    /// function for the whole of it, including why the earlier **bulk**
    /// spelling was a `404` against NodeWarden.
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.archive_cipher(&mut state.session, id).map_err(rest_error)
    }

    /// **Cost: one `PUT /api/ciphers/{id}/unarchive`. No sync.**
    ///
    /// A route of its own, and it has to be: the two backends do not even
    /// *shape* this the same way. `bw serve` has no unarchive route and
    /// reaches the state through `POST /restore/item/{id}`, the same route as
    /// an un-trash, selected by the item's current state. The Bitwarden API's
    /// restore is trash-only -- `deletedDate` and `archivedDate` are separate
    /// fields -- so [`Self::restore_item`] must not stand in for this one,
    /// and does not.
    ///
    /// Verified the same way round as [`Self::archive_item`]: the echoed
    /// cipher must come back **without** an `archivedDate`, or this is an
    /// error rather than a silent no-op.
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.unarchive_cipher(&mut state.session, id).map_err(rest_error)
    }

    /// **Cost: one `PUT /api/ciphers/{id}/restore`. No sync.**
    ///
    /// Out of the trash, and **only** the trash -- see [`Self::unarchive_item`]
    /// for why that is narrower than `bw serve`'s route of the same name.
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.restore_cipher(&mut state.session, id).map_err(rest_error)
    }

    /// **Cost: one `DELETE /api/ciphers/{id}`. No sync.**
    ///
    /// Gone, with no trash to recover it from. `bw serve` spells the same
    /// operation as its soft delete plus `?permanent=true`, so that backend
    /// carries a test asserting the query is on the wire; here the two are
    /// different HTTP methods on different routes and cannot be confused --
    /// which is why [`crate::rest::api`] named the irreversible one
    /// `hard_delete_cipher` rather than `delete_cipher`.
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        let mut state = self.locked();
        self.client.hard_delete_cipher(&mut state.session, id).map_err(rest_error)
    }

    /// **Computed here, from the item's seed. Cost: one full sync.**
    ///
    /// There is no TOTP endpoint on the Bitwarden API -- a code is not
    /// something the server stores or returns, which
    /// [`crate::vault_backend`]'s own docs say. `bw serve` has
    /// `GET /object/totp/{id}` only because the CLI computes it locally on the
    /// caller's behalf. So this backend does the same arithmetic, and it does
    /// it with **this crate's existing implementation**:
    /// [`crate::otpauth::parse_otpauth`] for the URI and
    /// [`crate::vault_window::totp_add::code_at`] for RFC 4226's truncation
    /// over RFC 6238's counter. Not one line of a second one is written here
    /// -- `breach.rs`'s `sha1_is_confined_to_the_breach_module` guard exists
    /// to keep it that way.
    ///
    /// # What `None` means, and what it does not
    ///
    /// `None` is "this item has no TOTP secret", matching `bw serve`, which
    /// answers that case with a `400` its backend translates the same way.
    /// A seed that is present but **unusable** -- not base32, an
    /// `otpauth://hotp` URI, an unknown parameter -- is `None` as well, and it
    /// is logged: `Ok(None)` is the only thing the trait's return type can say
    /// about it, and a silent one would be a TOTP row that vanishes with no
    /// trace anywhere. It is not an `Err`, because an error here reads to
    /// every call site as "the backend is unwell" and would put a poll into a
    /// failure streak over one malformed item.
    ///
    /// # The sync is why the vault window stopped calling this every second
    ///
    /// It used to be called once per second for as long as a TOTP item stayed
    /// selected, and one full sync per second on a 1,669-item vault is what
    /// made the reporting user's server answer `503` after a few minutes --
    /// with the code being computed locally at the end of it anyway, from a
    /// seed the window already held decrypted. That poll now reads the seed
    /// out of the snapshot it is already rendering
    /// ([`crate::vault_window::totp_poll_plan`]) and reaches this method only
    /// for a seed [`read_seed`] will not read, which on this backend answers
    /// `Ok(None)` **once** and stops the polling.
    ///
    /// **The sync is gone, and this is one row.**
    ///
    /// It was the last per-id read in this file that was still a whole vault,
    /// and the paragraph here argued for keeping it: the cost was "bounded by
    /// a user's clicks", since the fallback is reached once per item and then
    /// stops the polling. The bound was real and the price was not. `wrangler
    /// d1 insights` put one `GET /api/sync` at ~1,686 rows out of `ciphers` on
    /// the owner's server, which is what this paid to read a single seed --
    /// and [`Self::one_record`] answers with the same decrypted item for one.
    ///
    /// Nothing else changes. `one_record` maps a 404 to the same sentence
    /// [`Self::find`] produced for an id the sync did not carry, so an item
    /// that is gone still says it is gone; the seed is the same plaintext out
    /// of the same decrypt; and the malformed case still answers `Ok(None)`
    /// once and stops the poll.
    ///
    /// An older paragraph here said that cheapening this "by caching what
    /// `synced()` returns" would be the change [`Self::update_item`] must not
    /// have, because a write quotes the `revisionDate` of the sync it just
    /// made. That reasoning was about caching the *vault*, and it is answered
    /// where the caching now happens -- see [`Self::synced`], which asks the
    /// server whether the vault moved before it reuses anything, and
    /// `update_item`, which has not gone through `synced` since it moved to
    /// `one_record`.
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        let (record, _) = self.one_record(id)?;
        let item = &record;
        let Some(seed) = item.item.login.as_ref().and_then(|l| l.totp.as_ref()) else {
            return Ok(None);
        };
        let Some(auth) = read_seed(seed) else {
            log::warn!(
                "vault item {id} carries a TOTP secret this app cannot read, so no code can be \
                 shown for it; the secret itself is not logged"
            );
            return Ok(None);
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            // A clock before 1970 is a broken machine, not a broken vault.
            // Refusing by name beats computing a code for counter zero.
            .map_err(|_| {
                VaultError::Http("this machine's clock is before 1970, so no TOTP counter can be \
                                  computed"
                    .to_string())
            })?;
        Ok(crate::vault_window::totp_add::code_at(&auth, now).map(|c| c.to_string()))
    }

    /// **Computed here. No server endpoint exists for this at all**, which
    /// [`crate::vault_backend`]'s module docs say in as many words. **Cost:
    /// no network, no sync.**
    ///
    /// # One line, and that is the point
    ///
    /// This refused, because implementing it meant *this crate writing a
    /// password generator* -- choosing an alphabet, an entropy source and a
    /// passphrase wordlist, and quietly becoming the thing that decides how
    /// strong every password this app creates is. That was recorded as the
    /// owner's decision rather than a backend's, and the decision has since
    /// been taken.
    ///
    /// What it must **not** become is a generator living here.
    /// [`crate::password_gen`] is a module beside the backends precisely so
    /// that the next one, and the fill-path cards, which already build a
    /// [`crate::vault_bridge::PasswordRecipe`], reach the same generator
    /// rather than growing a second. Two generators in one app is two answers
    /// to how strong its passwords are, so this method is a call and a
    /// mapping and holds no alphabet, no draw and no policy of its own.
    ///
    /// # A passphrase is generated too, and a broken word list is refused
    ///
    /// [`crate::password_gen`] now answers passphrases as well, from a list of
    /// 4,096 words installed beside the executable. Two of its three word-list
    /// failures -- the file is absent, or it is present and does not verify --
    /// arrive here as their own variants and are mapped to
    /// [`VaultError::Unsupported`] rather than to a retryable error, because
    /// neither fixes itself: both are an installation this app cannot generate
    /// a passphrase from, and telling a caller to try again would be telling
    /// it to loop.
    ///
    /// The third,
    /// [`PasswordGenError::WordlistUnreadable`], is mapped the other way, and
    /// the split is the point of it. A list that could not be read *right now*
    /// -- an installer or an updater holding the file open while it replaces
    /// it -- does fix itself, so it is [`VaultError::Http`], the same
    /// retryable mapping `Rng` gets. Sending it to `Unsupported` would tell
    /// the caller never to retry and show the user a band saying this backend
    /// cannot generate passphrases at all, neither of which is true a second
    /// later.
    ///
    /// **None of the three is mapped to a weaker passphrase**, which is the
    /// only mapping that would actually be wrong.
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        crate::password_gen::generate(request).map_err(|e| match e {
            // Not `Unsupported`: the operation *is* supported and no decision
            // is missing -- this machine's CSPRNG failed, which is a fault a
            // caller may retry. `Unsupported` would tell it never to.
            PasswordGenError::Rng => VaultError::Http(e.to_string()),
            PasswordGenError::WordlistMissing => VaultError::Unsupported {
                backend: BACKEND,
                operation: "generate (passphrase)",
                why: "the word list a passphrase is built from is not installed beside this \
                      application; generating from an improvised one would produce a passphrase \
                      far weaker than it looks",
            },
            PasswordGenError::WordlistUnusable => VaultError::Unsupported {
                backend: BACKEND,
                operation: "generate (passphrase)",
                why: "the word list installed beside this application is not the one it ships; \
                      generating from a short or altered list would produce a passphrase far \
                      weaker than it looks",
            },
            // Not `Unsupported`, for exactly the reason `Rng` is not: this is
            // an installation that CAN generate a passphrase, and the only
            // thing wrong is the instant it was asked. `Unsupported` tells a
            // caller never to retry and the window says the backend cannot do
            // it at all -- both wrong, and both told the user their word list
            // was broken when an updater merely had it open. `Http` is the
            // crate's retryable failure, and `e.to_string()` carries the
            // sentence saying trying again may work.
            PasswordGenError::WordlistUnreadable => VaultError::Http(e.to_string()),
        })
    }
}

// ---- the small shared pieces -------------------------------------------------

/// The folder a write actually produced, or a refusal saying it cannot be
/// confirmed.
///
/// # This is the "never a false `Ok`" rule, for folders
///
/// Both folder writes answer with the server's own copy of the record, and
/// this is the one place that copy is judged. A `200` says the request was
/// accepted; it does not say the folder is now called what was asked for, and
/// the difference between those two is a rename that silently did not happen.
///
/// Four things must hold, and every one of them is a `?` and not an `unwrap`
/// -- the target is a self-hosted server that answers with a subset of
/// Bitwarden's fields, so every step here is something it could have omitted:
///
/// 1. The answer decrypts as a folder at all ([`decrypt_folder`], the same
///    mapper `GET /api/sync` goes through).
/// 2. Its `id` is not empty -- a created folder with no id is a folder the
///    caller cannot then rename, delete, or file anything into.
/// 3. If `expected_id` is given, the answer is about *that* folder.
/// 4. Its `name` decrypts back to exactly the `name` that was sent.
///
/// # Why the name is compared rather than trusted
///
/// It is the only check that distinguishes "the server stored this" from "the
/// server answered". It also closes the failure that would be quietest: a
/// name that did not decrypt comes out of [`decrypt_folder`] as the empty
/// string with a recorded failure, and without this comparison a rename would
/// return a [`Folder`] whose `name` is `""` -- straight into the Folders
/// sidebar as a blank row, reported as a success.
///
/// **No secret reaches the error.** A folder name is vault plaintext, so the
/// mismatch arm says that the name differs and does not say what either name
/// was; the id is a server-assigned GUID that already appears in URLs, and is
/// the only thing that makes the wrong-folder arm actionable.
fn confirmed_folder(
    answer: &serde_json::Value,
    name: &str,
    expected_id: Option<&str>,
    keys: &VaultKeys,
) -> Result<Folder, VaultError> {
    let (folder, failures) = decrypt_folder(answer, keys).ok_or_else(|| {
        VaultError::Parse(
            "the folder the server answered with: it is not a folder record with an id".to_string(),
        )
    })?;
    if folder.id.is_empty() {
        return Err(VaultError::Parse(
            "the folder the server answered with: it carries no id".to_string(),
        ));
    }
    if let Some(expected) = expected_id {
        if folder.id != expected {
            return Err(VaultError::Http(format!(
                "this write asked about the folder {expected} and the server answered about \
                 {}, so the change cannot be confirmed",
                folder.id
            )));
        }
    }
    if folder.name != name {
        // `failures` is counted, not printed with its `why` alone, because
        // the two cases read very differently to whoever sees this: a name
        // that would not decrypt is a key problem, and a name that decrypted
        // to something else is a server problem.
        return Err(VaultError::Http(format!(
            "the server accepted this folder write but the folder it answered with does not \
             carry the name that was sent, so the change may not have been made ({} field(s) of \
             the answer could not be decrypted)",
            failures.len()
        )));
    }
    Ok(folder)
}

/// Whether a cipher carries a non-null value at `key`.
///
/// `deletedDate` and `archivedDate` both ride [`VaultItem::other`] (neither is
/// a modelled field), and `/api/sync` sends them as `null` on items they do
/// not apply to -- `bw serve` omits them instead. Both spellings of absent
/// mean the same thing here, which is why this asks the question in one place
/// rather than at four call sites.
fn stamped(item: &VaultItem, key: &str) -> bool {
    item.other.get(key).is_some_and(|v| !v.is_null())
}

fn is_trashed(item: &VaultItem) -> bool {
    stamped(item, "deletedDate")
}

fn is_archived(item: &VaultItem) -> bool {
    stamped(item, "archivedDate")
}

// `read_seed` -- the reader for a saved `login.totp`, in either of its two
// spellings -- used to live here, and its move is not a tidy-up. The vault
// window's per-poll fast path needs the same answer to the same question
// ("can this app read this seed?"), so the function now sits beside
// `code_at`, the arithmetic it feeds, in `crate::vault_window::totp_add`.
// See that function's doc for why one reader and not two.

/// A [`RestError`] as the error type the rest of the app already handles.
///
/// Three destinations, and the split is by what a caller must *do*:
///
/// * `Unauthorized` stays itself. It is the one failure that means
///   re-authenticate rather than retry, which is the whole reason
///   [`VaultError::Unauthorized`] exists.
/// * `Parse` becomes `Parse` -- the server answered, and the answer was not
///   the shape this client reads.
/// * `Transport`, and any `Status` in the `5xx` range, becomes `Unreachable`
///   -- the server did not serve the request rather than refusing it, so the
///   user is told to try again instead of being sent to look for what is
///   wrong with data that is fine. **This is the arm the 503 report was
///   about**, and it is this backend's own shape that made the old wording
///   worst here: [`RestBackend::update_item`] calls `synced()` before it
///   sends anything, so a `503` out of that sync fails the write with no
///   `PUT` ever made -- "the vault backend refused the write" named a request
///   that did not exist. See [`VaultError::Unreachable`].
/// * Everything else, `4xx` status and crypto alike, becomes `Http`, whose
///   user-facing wording is "the backend refused". A crypto failure is not
///   literally HTTP; it is put here rather than in `Parse` because "the
///   answer couldn't be read" would send a reader looking at JSON when the
///   problem is a key, and it is a refusal rather than an unreachable server
///   because a retry of the same call cannot fix it.
///
/// The `5xx`/`4xx` line itself is drawn in [`VaultError::from_status`] and
/// not here, so this backend and the `bw serve` bridge cannot come to
/// disagree about what a `502` means.
///
/// **No arm carries a secret.** [`RestError`]'s own doc asserts that of every
/// one of its variants, and this only formats them.
fn rest_error(e: RestError) -> VaultError {
    match e {
        RestError::Unauthorized => VaultError::Unauthorized,
        RestError::Parse(what) => VaultError::Parse(format!("the server's answer was missing {what}")),
        // Neither arm binds anything out of `e`, so `e` is still whole for
        // its own `Display` -- which is the sentence that reaches the log.
        RestError::Transport(_) => VaultError::Unreachable(e.to_string()),
        RestError::Status(code) => VaultError::from_status(code, e.to_string()),
        other => VaultError::Http(other.to_string()),
    }
}

/// "There is no such item here", said once so that the two places that can
/// discover it say the same thing.
///
/// The two are [`RestBackend::find`], which looks for an id in a sync it
/// already has, and [`RestBackend::one_record`], which asks the server for one
/// and is answered `404`. They are the same fact about the same vault, and a
/// caller that matched on the sentence -- or a user reading it -- should not
/// be able to tell which route the question took.
///
/// [`VaultError::Http`] and not [`VaultError::Unsupported`]: the operation
/// *is* supported, the server simply has no such record.
///
/// The id is a server-assigned GUID and appears in URLs, so it is not a
/// secret; it is also the only thing that makes this message actionable.
fn no_such_item(id: &str) -> VaultError {
    VaultError::Http(format!("this vault holds no item with the id {id}"))
}

/// The refusal for a cipher the server returned and this crate could not make
/// an item out of at all -- not a JSON object, or carrying no `id`.
///
/// [`VaultError::Parse`] rather than [`no_such_item`], and the difference
/// matters: the server *has* this record and answered about it, so telling the
/// caller it does not exist would send it looking in the wrong place.
fn unreadable_record(id: &str) -> VaultError {
    VaultError::Parse(format!("the server's copy of the item {id}: it is not a cipher record"))
}

/// The one-record path's half of the rule [`RestBackend::synced`] follows for
/// a whole vault: a field that did not decrypt is **logged**, not fatal.
///
/// Counts and field names only, which is all a
/// [`crate::rest::sync::DecryptFailure`] can hold by construction.
fn log_record_failures(id: &str, failures: &[crate::rest::sync::DecryptFailure]) {
    if !failures.is_empty() {
        log::warn!(
            "{} field(s) of item {id} could not be decrypted and are missing from the record \
             this read produced: {failures:?}",
            failures.len()
        );
    }
}

/// A [`CryptoError`] as a [`VaultError`]. See [`rest_error`] on why `Http`.
///
/// [`CryptoError`]'s own rule is that it never carries a plaintext, a
/// ciphertext or a key -- only a name for what was wrong -- so formatting it
/// into a message that may be logged is safe by that type's construction, not
/// by inspection here.
fn crypto_error(e: CryptoError) -> VaultError {
    VaultError::Http(format!("this vault's cryptography failed: {e}"))
}

/// `pub` for the same reason [`crate::rest::crypto`]'s and
/// [`crate::rest::sync`]'s test modules are, and under the same limit: a
/// sibling module's tests need a **real** `RestBackend` answering a **real**
/// mock server, and every input to one -- the client, the login, the master
/// key -- is private to this file.
///
/// The sibling is `vault_window`, and what it needs it for is the one
/// assertion neither module can make alone: that opening a cold vault window
/// on this backend costs exactly one `GET /api/sync`. A double that merely
/// counts calls would pass that test while the app still made three, because
/// the quantity being asserted about is HTTP requests and only this file
/// knows how to produce a backend that makes them.
///
/// No production item changed visibility, and nothing here compiles into the
/// shipped binary.
#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::rest::api::Device;
    use crate::rest::crypto::tests::{key_from_64, seal};
    use crate::rest::crypto::{Kdf, SymmetricKey, master_key};
    use crate::vault_bridge::{PassphraseRecipe, PasswordRecipe};

    /// The master password every fixture below logs in with. Not a secret:
    /// nothing here reaches a real server, a real vault or `%APPDATA%`.
    const PASSWORD: &[u8] = b"master";
    const EMAIL: &str = "fixture@example.invalid";

    fn device() -> Device {
        Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC")
    }

    /// The fixture login, taken out of its outcome.
    ///
    /// The second-factor arm panics rather than being tolerated: a fixture
    /// server that started asking for one would no longer be serving the
    /// login these tests describe, and quietly skipping the test would be the
    /// worst of the three possible answers.
    fn fixture_login(client: &RestClient) -> Authenticated {
        match client.authenticate(EMAIL, PASSWORD, &device()).expect("the fixture login") {
            crate::rest::api::LoginOutcome::Done(authenticated) => authenticated,
            crate::rest::api::LoginOutcome::NeedsSecondFactor(_) => {
                panic!("the fixture server asked for a second factor")
            }
        }
    }

    /// The 64 bytes of the user key every fixture vault is encrypted under.
    fn user_key_bytes() -> [u8; 64] {
        let mut bytes = [0u8; 64];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i % 251).expect("under 251").wrapping_mul(7).wrapping_add(3);
        }
        bytes
    }

    fn user_key() -> SymmetricKey {
        key_from_64(&user_key_bytes())
    }

    fn enc(plain: &str) -> String {
        seal(&user_key(), plain.as_bytes())
    }

    /// The `profile.key` blob, built through the real arrangement: the
    /// fixture master key, stretched, sealing the sixty-four bytes above.
    fn protected_user_key() -> String {
        let master = master_key(PASSWORD, EMAIL, Kdf::Pbkdf2 { iterations: 1 })
            .expect("one iteration");
        seal(&master.stretch(), &user_key_bytes())
    }

    /// One cipher, shaped like `/api/sync`'s `cipherDetails`.
    ///
    /// `extra` is merged over it, which is how the trashed and archived
    /// fixtures below differ from the live one by exactly the key under test.
    /// `aKeyNoClientModels` is a key **this crate does not model at all**,
    /// present on every fixture so a write assertion can check that the
    /// retained JSON really survived rather than checking a key the mapper
    /// happens to know about anyway.
    fn cipher(id: &str, name: &str, extra: &serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "object": "cipherDetails",
            "id": id,
            "type": 1,
            "creationDate": "2020-01-01T00:00:00.000000Z",
            "revisionDate": "2021-01-01T00:00:00.000000Z",
            "deletedDate": null,
            "archivedDate": null,
            "organizationId": null,
            "key": null,
            "favorite": false,
            "folderId": "f1",
            "reprompt": 1,
            "collectionIds": [],
            "aKeyNoClientModels": "keep me",
            "name": enc(name),
            "fields": [],
            "card": null,
            "identity": null,
            "sshKey": null,
            "secureNote": null,
            "login": {
                "username": enc("u@example.com"),
                "password": enc("p4ssw0rd"),
                "totp": enc("otpauth://totp/Site:u?secret=JBSWY3DPEHPK3PXP&issuer=Site"),
                "uris": [],
                "fido2Credentials": []
            }
        });
        let object = base.as_object_mut().expect("an object");
        for (key, value) in extra.as_object().expect("an object").clone() {
            object.insert(key, value);
        }
        base
    }

    /// A whole sync: one live item, one trashed, one archived, one folder.
    fn sync_body() -> String {
        serde_json::json!({
            "object": "sync",
            "profile": {
                "key": protected_user_key(),
                "privateKey": null,
                "organizations": []
            },
            "folders": [{ "id": "f1", "name": enc("Work"), "object": "folder" }],
            "ciphers": [
                cipher("live-1", "A live item", &serde_json::json!({})),
                cipher(
                    "trash-1",
                    "A trashed item",
                    &serde_json::json!({ "deletedDate": "2022-01-01T00:00:00.000000Z" })
                ),
                cipher(
                    "arch-1",
                    "An archived item",
                    &serde_json::json!({ "archivedDate": "2022-02-01T00:00:00.000000Z" })
                )
            ]
        })
        .to_string()
    }

    /// The organisation key OpenSSL's fixture ciphertext actually contains --
    /// the 64 bytes `00 01 .. 3f`.
    ///
    /// Not transcribed here as a value: it is the plaintext of
    /// [`crate::rest::crypto::tests::ORG_KEY_WRAPPED_OAEP_SHA1`], which
    /// `rest::sync` asserts against, so this function and that constant have
    /// to agree or the RSA unwrap in
    /// `an_organisation_joined_since_the_last_sync_re_derives_the_keys_rather_than_failing`
    /// produces something else and the cipher below does not decrypt.
    fn org_key() -> SymmetricKey {
        let mut bytes = [0u8; 64];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).expect("under 64");
        }
        key_from_64(&bytes)
    }

    /// [`sync_body`]'s account after it has joined one organisation.
    ///
    /// The same `profile.key` -- the user key does not change when a user
    /// joins an organisation, which is exactly why the cached [`VaultKeys`]
    /// stays *plausible* while being wrong. What is added is the RSA private
    /// key and the wrapped organisation key, both OpenSSL's.
    fn sync_body_with_the_organisation() -> String {
        let user = user_key();
        serde_json::json!({
            "object": "sync",
            "profile": {
                "key": protected_user_key(),
                "privateKey": seal(
                    &user,
                    &crate::rest::crypto::tests::hex(
                        crate::rest::crypto::tests::ORG_KEY_PRIVATE_PKCS8_DER
                    )
                ),
                "organizations": [{
                    "id": "org1",
                    "key": format!(
                        "4.{}",
                        crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::hex(
                            crate::rest::crypto::tests::ORG_KEY_WRAPPED_OAEP_SHA1
                        ))
                    )
                }]
            },
            "folders": [],
            "ciphers": []
        })
        .to_string()
    }

    /// The organisation's cipher, as `GET /api/ciphers/org-1` answers it:
    /// every field under [`org_key`] and **not** under the user key, so a
    /// cached key set that has never heard of `org1` cannot open any of it.
    fn organisation_cipher() -> String {
        let key = org_key();
        serde_json::json!({
            "object": "cipherDetails",
            "id": "org-1",
            "type": 1,
            "organizationId": "org1",
            "key": null,
            "creationDate": "2024-01-01T00:00:00.000000Z",
            "revisionDate": "2024-01-01T00:00:00.000000Z",
            "deletedDate": null,
            "archivedDate": null,
            "favorite": false,
            "name": seal(&key, b"Shared login"),
            "login": { "password": seal(&key, b"shared") }
        })
        .to_string()
    }

    /// The live cipher with **one** field this vault's user key cannot open.
    ///
    /// The password is sealed under [`org_key`] -- a real key, and the wrong
    /// one -- so it fails its HMAC and is recorded as a
    /// [`crate::rest::sync::DecryptFailure`] while every other field of the
    /// record decrypts normally.
    ///
    /// That is `map_cipher` answering `Some(item, failures)`, which is a
    /// **different branch** from [`organisation_cipher`]'s `None` and needs
    /// its own fixture: a mutation that neutralised only one of the two would
    /// otherwise survive. It is also the shape [`crate::rest::sync::Retained`]
    /// exists for -- the value stays ciphertext in the model, and a write must
    /// put that exact ciphertext back.
    /// **Called once per test and the result threaded through**, never twice:
    /// `seal` draws a fresh IV, so two calls produce two different
    /// ciphertexts and a test comparing one against the other would be
    /// comparing two encryptions of the same words. Read the value back out
    /// of the returned JSON.
    fn cipher_with_one_unreadable_field() -> serde_json::Value {
        let mut value = cipher("live-1", "A live item", &serde_json::json!({}));
        value["login"]["password"] = serde_json::json!(seal(&org_key(), b"under the wrong key"));
        value
    }

    /// A folder as a folder endpoint answers it: the id the server assigned
    /// and the name **encrypted under the fixture user key**, which is the
    /// only shape `confirmed_folder` can accept and the reason these tests
    /// cannot be passed by a server echoing plaintext.
    fn folder_answer(id: &str, name: &str) -> String {
        serde_json::json!({
            "object": "folder",
            "id": id,
            "name": enc(name),
            "revisionDate": "2023-05-05T00:00:00.000000Z"
        })
        .to_string()
    }

    /// A bulk archive route's answer for one id, stamped or not.
    fn archive_answer(id: &str, archived: bool) -> String {
        let stamp = if archived {
            serde_json::Value::String("2022-03-01T00:00:00.000000Z".to_string())
        } else {
            serde_json::Value::Null
        };
        serde_json::json!({
            "object": "list",
            "data": [{ "object": "cipher", "id": id, "archivedDate": stamp }]
        })
        .to_string()
    }

    /// A logged-in backend against a mock server that answers prelogin,
    /// the grant and `/api/sync`.
    ///
    /// **One PBKDF2 iteration**, because the derivation is not what any test
    /// in this file is checking and six hundred thousand of them per test is
    /// a suite nobody runs; `crypto.rs` pins the real cost separately. The
    /// server is returned so the caller can add write mocks to it and so it
    /// outlives the backend.
    /// A `RestBackend` on a mock server that answers prelogin, the token and
    /// the sync, holding the fixture vault [`sync_body`] describes.
    ///
    /// The sync mock here is deliberately **uncounted** (`expect_at_least(1)`):
    /// most tests in this file are about some other route and would otherwise
    /// be asserting about a number they do not care about.
    pub fn logged_in() -> (crate::test_http::MockServer, RestBackend) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_body())
            .expect_at_least(1)
            .create();
        declare_the_per_id_cipher_route(&mut server);
        (server, backend)
    }

    /// `GET /api/ciphers/{id}` for the three ciphers [`sync_body`] holds, and
    /// a `404` for every other id -- so the fixture server answers about the
    /// **same vault** whichever way it is asked, which is the only way a test
    /// can move a method from the sync to the per-id route and still be
    /// comparing like with like.
    ///
    /// # Two mocks, and the registration order is load-bearing
    ///
    /// [`crate::test_http`] copies `mockito`'s selection verbatim: among the
    /// mocks that match, the first still owing hits, else **the last
    /// registered**. Both of these are `expect_at_least(0)`, which owes
    /// nothing from the start, so for an id they both match the later one
    /// wins. That is why the catch-all `404` is registered *first* and the
    /// three real ids *second*: the other order answers `404` for `live-1`.
    ///
    /// It also means a test may lay down its own `GET /api/ciphers/{id}` after
    /// calling this and have it win, which is what the key-staleness tests
    /// below do.
    ///
    /// Uncounted, for [`logged_in`]'s reason: most tests here are about some
    /// other route and would otherwise be asserting about a number they do not
    /// care about. A test that *is* about the number declares the route itself
    /// on a server from [`signed_in_with_no_sync_route`].
    pub fn declare_the_per_id_cipher_route(server: &mut crate::test_http::MockServer) {
        server
            .mock("GET", crate::test_http::Matcher::Regex("^/api/ciphers/[^/?]+$".to_string()))
            .with_status(404)
            .expect_at_least(0)
            .create();
        server
            .mock(
                "GET",
                crate::test_http::Matcher::Regex(
                    "^/api/ciphers/(live-1|trash-1|arch-1)$".to_string(),
                ),
            )
            .with_body_from_request(|request| fixture_cipher_for(request.path()).into_bytes())
            .expect_at_least(0)
            .create();
    }

    /// The one of [`sync_body`]'s three ciphers that `path` names, as JSON.
    ///
    /// **Built from the same [`cipher`] the sync payload is built from**, with
    /// the same `extra` for the trashed and archived ones -- so the record a
    /// per-id read produces is byte-identical to the record the sync produces
    /// for the same id. A second fixture here would let the two paths drift
    /// apart in exactly the way these tests exist to detect.
    pub fn fixture_cipher_for(path: &str) -> String {
        let id = path.rsplit('/').next().unwrap_or_default();
        let extra = match id {
            "trash-1" => serde_json::json!({ "deletedDate": "2022-01-01T00:00:00.000000Z" }),
            "arch-1" => serde_json::json!({ "archivedDate": "2022-02-01T00:00:00.000000Z" }),
            _ => serde_json::json!({}),
        };
        let name = match id {
            "trash-1" => "A trashed item",
            "arch-1" => "An archived item",
            _ => "A live item",
        };
        cipher(id, name, &extra).to_string()
    }

    /// [`logged_in`] with **the sync route not declared at all**, for a caller
    /// that wants to declare it itself and count it exactly.
    ///
    /// The split exists rather than a re-declaration on top of `logged_in`
    /// because which of two overlapping mockito mocks answers a request is a
    /// property of the mocking library, not of this app, and a counting test
    /// that quietly measured the wrong one of the two would be precisely the
    /// defect it was written to catch. With no first mock there is nothing to
    /// out-rank: the caller's is the only route that can answer, so the number
    /// it reports is the number of syncs the app performed.
    ///
    /// The login itself performs **no sync** -- `fixture_login` is prelogin
    /// and the token endpoint, and `RestBackend::new` does not fetch -- so a
    /// caller's `.expect(n)` counts only what it goes on to ask the backend
    /// for, with nothing to subtract for the fixture.
    pub fn signed_in_with_no_sync_route() -> (crate::test_http::MockServer, RestBackend) {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(
                r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600,
                    "token_type":"Bearer","scope":"api offline_access"}"#,
            )
            .create();

        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        (server, RestBackend::new(client, authenticated))
    }

    /// The body the fixture server answers `GET /api/sync` with: a vault of
    /// one live item, one trashed, one archived, and one folder named `Work`.
    pub fn sync_payload() -> String {
        sync_body()
    }

    /// The control every other test in this file rests on: the fixture really
    /// is ciphertext, and the login really is what opens it.
    ///
    /// Without this, an assertion that `list_items` returns one named item
    /// could be passing over a payload that was never encrypted, and the
    /// whole file would be testing a JSON filter.
    #[test]
    fn the_fixture_vault_is_ciphertext_and_the_login_is_what_opens_it() {
        let body = sync_body();
        assert!(!body.contains("A live item"), "the fixture is not encrypted");
        assert!(body.contains("2."), "no EncString in the fixture: {body}");

        let (_server, backend) = logged_in();
        let items = backend.list_items().expect("the vault opens");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "A live item");
        assert_eq!(
            items[0].login.as_ref().and_then(|l| l.username.as_deref()),
            Some("u@example.com")
        );
    }

    /// `bw serve` answers three **disjoint** sets from three query strings;
    /// `/api/sync` answers one list and this backend cuts it into three. The
    /// cut has to be a partition -- every item in exactly one list -- or the
    /// Trash view shows the user their whole vault, which is the failure
    /// `vault_bridge`'s own list tests were written against from the other
    /// side.
    #[test]
    fn the_live_list_the_trash_and_the_archive_partition_the_vault() {
        let (_server, backend) = logged_in();
        let ids = |items: Vec<VaultItem>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(ids(backend.list_items().expect("live")), vec!["live-1"]);
        assert_eq!(ids(backend.list_trash().expect("trash")), vec!["trash-1"]);
        assert_eq!(ids(backend.list_archive().expect("archive")), vec!["arch-1"]);
    }

    /// A `null` `deletedDate` -- which `/api/sync` sends on every live item
    /// and `bw serve` omits entirely -- must not read as "deleted". This is
    /// the one-character version of showing the user an empty vault.
    #[test]
    fn a_null_date_is_absent_and_not_a_stamp() {
        let (_server, backend) = logged_in();
        let live = backend.get_item("live-1").expect("the live item");
        assert!(live.other.get("deletedDate").expect("the key is carried").is_null());
        assert!(!is_trashed(&live));
        assert!(!is_archived(&live));
    }

    #[test]
    fn folders_come_off_the_same_sync() {
        let (_server, backend) = logged_in();
        let folders = backend.list_folders().expect("folders");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "Work");
    }

    /// **The whole vault for one sync, and the count is the assertion.**
    ///
    /// `folders_come_off_the_same_sync` above establishes that the folder
    /// names ride the cipher payload; this establishes that the app can
    /// actually get both halves for the price of the one payload they ride
    /// on. `list_items` then `list_folders` -- what `VaultCache::populate`
    /// did before `list_vault` existed -- is the control, and it must be
    /// **two**: not because two is wanted, but because a `.expect(1)` that
    /// also passed for the old spelling would be measuring nothing.
    ///
    /// Both halves are checked for content, not just counted. A `list_vault`
    /// that returned one full sync's worth of empty vectors would satisfy a
    /// request count on its own.
    #[test]
    fn the_whole_vault_costs_one_sync_where_asking_twice_costs_two() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();

        let vault = backend.list_vault().expect("the vault");
        sync.assert();

        // The same answers the two separate calls give, or this is a cheaper
        // route to a different vault. `live-1` only: the fixture's trashed
        // and archived ciphers must stay filtered out here exactly as
        // `list_items` filters them, which a bare `vault.items` would not do.
        assert_eq!(vault.items.len(), 1, "the live item, without the trashed or archived ones");
        assert_eq!(vault.items[0].id, "live-1");
        assert_eq!(vault.folders.len(), 1);
        assert_eq!(vault.folders[0].name, "Work");

        // The control, on its own server so the count above stays clean.
        let (mut server, backend) = signed_in_with_no_sync_route();
        let twice = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(2)
            .create();
        let items = backend.list_items().expect("items");
        let folders = backend.list_folders().expect("folders");
        twice.assert();
        assert_eq!(items.len(), vault.items.len());
        assert_eq!(folders.len(), vault.folders.len());
    }

    /// An id the vault does not hold is a refusal that names the id, and
    /// specifically **not** [`VaultError::Unsupported`] -- which would tell a
    /// caller to stop attempting this operation entirely.
    #[test]
    fn an_unknown_id_is_refused_and_is_not_confused_with_an_unsupported_call() {
        let (_server, backend) = logged_in();
        let err = backend.get_item("nope").expect_err("no such item");
        assert!(matches!(err, VaultError::Http(ref m) if m.contains("nope")), "{err:?}");
    }

    /// **The heart of the refusal contract, and there is nothing left in
    /// it.** This backend refuses no operation.
    ///
    /// **This list used to be six, then four, then three, and is now
    /// empty.** `archive_item`, `unarchive_item` and both halves of
    /// `generate` went first; the three folder writes went last. Every one of
    /// them was removed because it was implemented, not because the contract
    /// loosened -- each is asserted against a real route in this file
    /// (`an_archive_sends_a_batch_of_one_and_reads_the_answer_back`,
    /// `a_password_and_a_passphrase_are_both_generated_locally`,
    /// `a_folder_create_puts_the_encrypted_name_on_the_wire_and_learns_the_id`
    /// and its neighbours).
    ///
    /// So the test inverts: instead of listing what refuses, it drives every
    /// operation that ever refused and asserts that **none** of them answers
    /// [`VaultError::Unsupported`]. A refusal reintroduced by accident fails
    /// here; a refusal reintroduced on purpose has to delete this test, which
    /// is a thing a reviewer sees.
    ///
    /// The one `Unsupported` this backend can still produce is not an
    /// operation refusal: `generate` maps a missing or altered passphrase
    /// word list to it, which is an installation fault rather than a decision
    /// this backend declined to take. The word list is present in this
    /// checkout, so the call below exercises the ordinary path.
    #[test]
    fn no_operation_this_backend_offers_refuses_any_more() {
        let (mut server, backend) = logged_in();
        // Every folder route answers, so that a *refusal* is distinguishable
        // from an ordinary transport or shape failure -- this test is about
        // `Unsupported` and nothing else.
        server
            .mock("POST", "/api/folders")
            .with_body(folder_answer("f9", "x"))
            .expect_at_least(0)
            .create();
        server
            .mock("PUT", "/api/folders/f1")
            .with_body(folder_answer("f1", "x"))
            .expect_at_least(0)
            .create();
        server.mock("DELETE", "/api/folders/f1").with_status(204).expect_at_least(0).create();
        server
            .mock("PUT", "/api/ciphers/archive")
            .with_body(archive_answer("live-1", true))
            .expect_at_least(0)
            .create();
        server
            .mock("PUT", "/api/ciphers/unarchive")
            .with_body(archive_answer("arch-1", false))
            .expect_at_least(0)
            .create();

        let outcomes: Vec<(&str, Option<VaultError>)> = vec![
            ("create_folder", backend.create_folder("x").err()),
            ("update_folder", backend.update_folder("f1", "x").err()),
            ("delete_folder", backend.delete_folder("f1").err()),
            ("archive_item", backend.archive_item("live-1").err()),
            ("unarchive_item", backend.unarchive_item("arch-1").err()),
            (
                "generate",
                backend.generate(&GenerateRequest::Password(PasswordRecipe::default())).err(),
            ),
        ];
        for (operation, outcome) in outcomes {
            assert!(
                !matches!(outcome, Some(VaultError::Unsupported { .. })),
                "{operation} refuses again: {outcome:?}"
            );
        }
    }

    // ---- the three folder writes -------------------------------------------

    /// A folder name that would be unmistakable on the wire. Never a real
    /// word, so a hit is a hit.
    const FOLDER_NEEDLE: &str = "NEEDLE-folder-name-never-in-the-clear";

    /// **The replacement for the create refusal.**
    ///
    /// Four things, and every one of them was a way the refusal could have
    /// been lifted wrongly:
    ///
    /// 1. `POST`, to `/api/folders` -- not the cipher route, not a `PUT`.
    /// 2. The bearer token is on it.
    /// 3. The body carries the name as an **`EncString`** and the plaintext
    ///    appears nowhere in it. This is the assertion that matters: the
    ///    obvious wrong implementation of this method is `{"name": name}`,
    ///    which is exactly what `bw serve`'s backend correctly sends to its
    ///    own local process and exactly what must never leave this one.
    /// 4. The created folder's `id` comes from the **answer**, because that
    ///    is the only place it exists.
    #[test]
    fn a_folder_create_puts_the_encrypted_name_on_the_wire_and_learns_the_id() {
        let (mut server, backend) = logged_in();
        let post = server
            .mock("POST", "/api/folders")
            .match_header("Authorization", "Bearer AT-1")
            .match_request(|request| {
                let body = request.utf8_lossy_body().to_string();
                let json: serde_json::Value =
                    serde_json::from_str(&body).expect("the body is JSON");
                let name = json.get("name").and_then(|v| v.as_str()).expect("a name key");
                !body.contains(FOLDER_NEEDLE)
                    && name.starts_with("2.")
                    && name.parse::<crate::rest::crypto::EncString>().is_ok()
                    // A create has no id yet, and must not send an empty one.
                    && json.get("id").is_none()
            })
            .with_body(folder_answer("f9", FOLDER_NEEDLE))
            .expect(1)
            .create();

        let folder = backend.create_folder(FOLDER_NEEDLE).expect("the folder is created");
        post.assert();
        assert_eq!(folder.id, "f9", "the id must come from the server's answer");
        assert_eq!(folder.name, FOLDER_NEEDLE);
    }

    /// **The replacement for the rename refusal.** The id is in the path, the
    /// path is the *folder* one, and the name is still ciphertext.
    ///
    /// The cipher route is watched as well: a rename that reached
    /// `/api/ciphers/...` would be this backend rewriting items in order to
    /// rename a folder.
    #[test]
    fn a_folder_rename_puts_the_encrypted_name_to_the_folder_path() {
        let (mut server, backend) = logged_in();
        let put = server
            .mock("PUT", "/api/folders/f1")
            .match_header("Authorization", "Bearer AT-1")
            .match_request(|request| {
                let body = request.utf8_lossy_body().to_string();
                !body.contains(FOLDER_NEEDLE) && body.contains("2.")
            })
            .with_body(folder_answer("f1", FOLDER_NEEDLE))
            .expect(1)
            .create();
        // Default expectation, so `matched()` means "was hit" -- see the
        // archive test for why this is not an `expect(0)`.
        let cipher_route = server.mock("PUT", "/api/ciphers/live-1").with_status(200).create();

        let folder = backend.update_folder("f1", FOLDER_NEEDLE).expect("the rename lands");
        put.assert();
        assert_eq!(folder.id, "f1");
        assert_eq!(folder.name, FOLDER_NEEDLE);
        assert!(!cipher_route.matched(), "a folder rename touched a cipher");
    }

    /// **The replacement for the delete refusal**, and the two things that
    /// refusal was most worried about.
    ///
    /// 1. `DELETE /api/folders/{id}` and nothing else. **No cipher is
    ///    touched**: the server un-files the items itself, and a client-side
    ///    sweep is how items go missing. The cipher routes are watched, and a
    ///    hit on any of them fails this test.
    /// 2. **An empty `204` is a success.** That is the deliberate difference
    ///    from the archive routes, argued in `RestClient::delete_folder`, and
    ///    it is pinned here because the opposite reading would send a caller
    ///    back to delete an already-deleted folder.
    ///
    /// It also pays no sync: there is no key to fetch for a request that is
    /// just an id, and the `expect(0)`-free idiom above is used again.
    #[test]
    fn a_folder_delete_hits_only_its_own_route_and_an_empty_answer_is_a_success() {
        let (mut server, backend) = logged_in();
        let delete = server.mock("DELETE", "/api/folders/f1").with_status(204).expect(1).create();
        let cipher_edit = server.mock("PUT", "/api/ciphers/live-1").with_status(200).create();
        let cipher_delete = server.mock("DELETE", "/api/ciphers/live-1/delete").with_status(200).create();

        backend.delete_folder("f1").expect("an empty 204 is a delete that happened");
        delete.assert();
        assert!(!cipher_edit.matched(), "a folder delete edited an item");
        assert!(!cipher_delete.matched(), "a folder delete deleted an item");
    }

    /// The delete is id-only and pays for no sync, matching the cost its doc
    /// claims -- the same shape as `the_archive_writes_do_not_pay_for_a_sync`,
    /// with no `/api/sync` mock at all so a sync would be a refused
    /// connection.
    #[test]
    fn a_folder_delete_does_not_pay_for_a_sync() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600}"#)
            .create();
        server.mock("DELETE", "/api/folders/f1").with_status(200).expect(1).create();
        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        let backend = RestBackend::new(client, authenticated);
        backend.delete_folder("f1").expect("the delete, with no sync behind it");
    }

    /// **The false-`Ok` guard, which is why this stopped being a refusal
    /// safely.**
    ///
    /// Every shape below is answered `200`. None of them proves the folder is
    /// now called what was asked for, so none of them may be an `Ok`:
    ///
    /// * a body that is not a folder record at all;
    /// * a folder with no `id`, which a caller could never then use;
    /// * on a rename, a folder with a **different** id -- an answer about
    ///   somebody else's folder;
    /// * a name that is valid ciphertext of **something else** -- the server
    ///   stored a different name;
    /// * a name in the **clear** -- which does not decrypt, comes back as the
    ///   empty string, and would otherwise reach the sidebar as a blank row
    ///   reported as a success.
    #[test]
    fn a_folder_write_the_server_did_not_confirm_is_an_error_and_never_ok() {
        let answers = [
            ("not a folder", r#"{"object":"list","data":[]}"#.to_string()),
            ("no id", serde_json::json!({ "name": enc(FOLDER_NEEDLE) }).to_string()),
            (
                "a different name",
                serde_json::json!({ "id": "f1", "name": enc("something else") }).to_string(),
            ),
            (
                "a plaintext name",
                serde_json::json!({ "id": "f1", "name": FOLDER_NEEDLE }).to_string(),
            ),
        ];
        for (what, body) in answers {
            let (mut server, backend) = logged_in();
            server.mock("POST", "/api/folders").with_body(body.clone()).create();
            server.mock("PUT", "/api/folders/f1").with_body(body.clone()).create();

            let created = backend.create_folder(FOLDER_NEEDLE);
            assert!(created.is_err(), "a create was Ok on an answer that was {what}");
            let renamed = backend.update_folder("f1", FOLDER_NEEDLE);
            assert!(renamed.is_err(), "a rename was Ok on an answer that was {what}");
        }

        // And the one that only a rename can get wrong: the right shape,
        // the right name, the wrong folder.
        let (mut server, backend) = logged_in();
        server
            .mock("PUT", "/api/folders/f1")
            .with_body(folder_answer("a-different-folder", FOLDER_NEEDLE))
            .create();
        let err = backend.update_folder("f1", FOLDER_NEEDLE).expect_err("the wrong folder");
        assert!(
            matches!(err, VaultError::Http(ref m) if m.contains("a-different-folder")),
            "{err:?}"
        );
    }

    /// **The replacement for two of the refusals that used to be in the list
    /// above**, and the property that made them refusals in the first place.
    ///
    /// Three things at once, because they are one behaviour:
    ///
    /// 1. The per-id trait call reaches the **per-id** route, with the id in
    ///    the path and no body at all -- asserted on the wire, not inferred.
    ///    `match_body("")` is what says the old bulk `{"ids": [...]}` is gone;
    ///    the path is what says the request goes where NodeWarden's routing
    ///    table actually has a handler.
    /// 2. It is a route of its own and **not** an edit: the item's plain
    ///    `PUT` never being hit is what says the forbidden `archivedDate`
    ///    fake was not taken instead.
    /// 3. Archive and unarchive are **different** routes, so the second
    ///    cannot quietly be `restore`.
    #[test]
    fn an_archive_reaches_the_per_id_route_and_reads_the_answer_back() {
        let (mut server, backend) = logged_in();
        let archive = server
            .mock("PUT", "/api/ciphers/live-1/archive")
            .match_header("Authorization", "Bearer AT-1")
            .match_body("")
            .with_body(
                r#"{"object":"cipher","id":"live-1",
                    "archivedDate":"2022-03-01T00:00:00.000000Z"}"#,
            )
            .expect(1)
            .create();
        let unarchive = server
            .mock("PUT", "/api/ciphers/arch-1/unarchive")
            .match_body("")
            .with_body(r#"{"object":"cipher","id":"arch-1","archivedDate":null}"#)
            .expect(1)
            .create();
        // The bulk route the client used to send, which nothing may reach now.
        let bulk = server.mock("PUT", "/api/ciphers/archive").with_status(200).create();
        // The edit route, which an archive must never reach.
        // No `expect(0)`: `matched()` reports whether the expected
        // hit count was *met*, so an `expect(0)` mock reads as matched when it
        // was never called and this assertion would be inverted. Left at the
        // default expectation, `matched()` means "was hit", which is the
        // question being asked. Same idiom as `api`'s
        // `an_id_that_is_not_url_path_safe_is_refused_before_anything_is_sent`.
        let edit = server.mock("PUT", "/api/ciphers/live-1").with_status(200).create();

        backend.archive_item("live-1").expect("the archive");
        backend.unarchive_item("arch-1").expect("the unarchive");
        archive.assert();
        unarchive.assert();
        assert!(!edit.matched(), "an archive was expressed as an edit setting `archivedDate`");
        assert!(!bulk.matched(), "an archive still reached the bulk route");
    }

    /// **The whole reason this stopped being a refusal safely.**
    ///
    /// `archivedDate` is assigned by the **server**, so a `200` says the
    /// request was accepted and not that the stamp was written. Every shape
    /// of an accepted-but-unconfirmed answer must be an error; an `Ok` here
    /// is the "reports success while doing nothing" failure the refusal
    /// existed to prevent, and it would be worse arriving through a real
    /// route than through a fake one, because it would look correct.
    ///
    /// Four shapes, all answered `200`: a cipher that is someone else, a
    /// cipher with no id at all, the right cipher in the **wrong state**
    /// (archived asked for, nothing stamped), and a body that is not a cipher
    /// and therefore cannot report the state either.
    #[test]
    fn an_archive_that_did_not_move_this_id_is_an_error_and_never_ok() {
        let bodies = [
            ("another id", r#"{"object":"cipher","id":"someone-else"}"#),
            ("no id at all", r#"{"object":"cipher","archivedDate":"2022-03-01T00:00:00.000000Z"}"#),
            ("the wrong state", r#"{"object":"cipher","id":"live-1","archivedDate":null}"#),
            ("no cipher at all", "null"),
        ];
        for (what, body) in bodies {
            let (mut server, backend) = logged_in();
            server
                .mock("PUT", "/api/ciphers/live-1/archive")
                .with_status(200)
                .with_body(body)
                .expect(1)
                .create();
            let err = backend.archive_item("live-1").expect_err(what);
            assert!(
                matches!(err, VaultError::Http(_)),
                "an archive answered with {what} was not reported as a failure: {err:?}"
            );
        }
    }

    /// The mirror of the previous test for the other direction: an unarchive
    /// whose echoed cipher still carries an `archivedDate` did not happen.
    ///
    /// Worth its own test rather than a fifth row above, because the
    /// predicate is *inverted* here and a single implementation that ignored
    /// the direction would pass the archive cases and fail only this one.
    #[test]
    fn an_unarchive_whose_item_is_still_stamped_is_an_error() {
        let (mut server, backend) = logged_in();
        server
            .mock("PUT", "/api/ciphers/arch-1/unarchive")
            .with_body(
                r#"{"object":"cipher","id":"arch-1",
                    "archivedDate":"2022-02-01T00:00:00.000000Z"}"#,
            )
            .expect(1)
            .create();
        let err = backend.unarchive_item("arch-1").expect_err("still archived");
        assert!(matches!(err, VaultError::Http(_)), "{err:?}");
    }

    /// The archive writes are id-only: neither pays for a full sync, matching
    /// the cost their docs claim and the other id-only writes beside them.
    #[test]
    fn the_archive_writes_do_not_pay_for_a_sync() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600}"#)
            .create();
        // No `/api/sync` mock at all: a sync would be a connection refused.
        server
            .mock("PUT", "/api/ciphers/live-1/archive")
            .with_body(
                r#"{"object":"cipher","id":"live-1",
                    "archivedDate":"2022-03-01T00:00:00.000000Z"}"#,
            )
            .expect(1)
            .create();
        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        let backend = RestBackend::new(client, authenticated);
        backend.archive_item("live-1").expect("the archive, with no sync behind it");
    }

    /// `list_archive` reads `archivedDate`, and so do the writes -- which is
    /// the property that keeps the two from disagreeing about what
    /// "archived" means.
    ///
    /// Asserted end to end: the same fixture item the sync reports as
    /// archived is the one an unarchive is accepted for when the server
    /// echoes it back unstamped, and a stamped echo is refused. If either
    /// half ever moved to a different field, one of these two would fail.
    #[test]
    fn the_archive_reader_and_the_archive_writer_agree_on_the_field() {
        let (mut server, backend) = logged_in();
        let archived = backend.list_archive().expect("the archive listing");
        let ids = archived.into_iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(ids, vec!["arch-1"], "the reader's idea of archived changed");
        server
            .mock("PUT", "/api/ciphers/arch-1/unarchive")
            .with_body(r#"{"object":"cipher","id":"arch-1","archivedDate":null}"#)
            .create();
        backend.unarchive_item("arch-1").expect("an unstamped echo is the success case");
    }

    /// **The other refusal that was implemented**, now in both halves: a
    /// password and a passphrase are each generated, and neither answer is
    /// invented here.
    ///
    /// The unmatched mock is the load-bearing part. `generate` must not
    /// acquire a route: there is no server endpoint for it anywhere, so a
    /// generate that touched the network would mean this backend had invented
    /// one. Both halves are answered from [`crate::password_gen`] -- the
    /// passphrase reads one local file and nothing else.
    #[test]
    fn a_password_and_a_passphrase_are_both_generated_locally() {
        let (mut server, backend) = logged_in();
        // Default expectation, not `expect(0)` -- see the note in
        // `an_archive_sends_a_batch_of_one_and_reads_the_answer_back`.
        let any = server.mock("GET", crate::test_http::Matcher::Any).with_status(200).create();

        let password = backend
            .generate(&GenerateRequest::Password(PasswordRecipe::default()))
            .expect("a generated password");
        assert_eq!(password.len(), 20);
        assert!(password.chars().any(|c| c.is_ascii_digit()), "not the recipe that was asked for");

        let passphrase = backend
            .generate(&GenerateRequest::Passphrase(PassphraseRecipe::default()))
            .expect("a generated passphrase");
        // The default recipe: four words, `-`, capitalised, with a number.
        assert_eq!(passphrase.split('-').count(), 4, "{}", &*passphrase);
        assert_eq!(passphrase.chars().filter(char::is_ascii_digit).count(), 1);

        assert!(!any.matched(), "generate reached the network; there is no endpoint for it");
    }

    /// Two calls to `generate` do not agree, which is the cheapest possible
    /// check that this backend is really delegating to the CSPRNG-backed
    /// generator and has not grown a constant of its own.
    #[test]
    fn two_generated_passwords_differ() {
        let (_server, backend) = logged_in();
        let request = GenerateRequest::Password(PasswordRecipe::default());
        let first = backend.generate(&request).expect("one");
        let second = backend.generate(&request).expect("two");
        assert_ne!(*first, *second);
    }

    // **`a_refusal_is_not_a_transport_failure` was deleted here**, and the
    // reasoning is kept because the test looked reasonable right up until it
    // was examined.
    //
    // It asserted that a refusal (`VaultError::Unsupported`) is never
    // mistakable for a transport failure, and it drove that through
    // `create_folder`. When "The last three refusals answered: create, rename
    // and delete a folder" landed, `create_folder` stopped refusing and
    // started making a real request -- so against a `logged_in()` server,
    // which mocks only prelogin, the token and the sync, it began answering
    // `Http("the server answered 501")`: the unmatched-route status.
    // The test was left behind by its own feature, and it survived unnoticed
    // because this crate's local runs are full of loopback failures on the
    // author's machine and this looked like one more. CI found it the day the
    // branch reached `main`.
    //
    // Repointing it was tried and does not work, which is the interesting
    // part: **this backend has no reachable refusal left.** Every one of the
    // operations answers -- that is pinned, from the other direction,
    // by `no_operation_this_backend_offers_refuses_any_more`. The only
    // `Unsupported` it can still produce is `generate` meeting a missing or
    // altered `assets/wordlist.txt`, and a test may not arrange that: it
    // would have to remove a file the running crate reads. An over-long
    // passphrase recipe does not do it either -- the generator caps the word
    // count rather than refusing.
    //
    // So the invariant is not weakened here, it is unreachable, and a test
    // that cannot reach what it names is the defect this project keeps
    // finding. It is recorded as a comment rather than left as a passing
    // assertion about nothing.

    /// **The rule `rest::write` exists for, checked through the backend.**
    ///
    /// The `PUT` body must carry `aKeyNoClientModels`, which nothing in this
    /// crate models: a mapper that built the body from the model alone would
    /// delete it -- and every attachment and passkey beside it -- from the
    /// user's real vault on the first edit.
    ///
    /// # The unmodelled value is given a value only the per-id read has
    ///
    /// `retained` -- [`DecryptedItem`]'s account of which in-place values are
    /// still the server's ciphertext, and the JSON they are laid over -- used
    /// to come off `/api/sync` and now comes off `GET /api/ciphers/{id}`. Both
    /// go through [`decrypt_cipher`]'s `map_cipher`, so they *should* be the
    /// same record; this test refuses to take that on trust. The per-id route
    /// answers `keep me from the per-id read` where the sync fixture says
    /// `keep me`, and the body must carry the former -- so a `retained` that
    /// had quietly come from anywhere else fails here rather than passing on a
    /// value both sources happened to share.
    #[test]
    fn an_edit_carries_the_fields_this_crate_does_not_model_back_to_the_server() {
        let (mut server, backend) = logged_in();
        server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(
                cipher(
                    "live-1",
                    "A live item",
                    &serde_json::json!({ "aKeyNoClientModels": "keep me from the per-id read" }),
                )
                .to_string(),
            )
            .expect_at_least(1)
            .create();

        let answer = cipher(
            "live-1",
            "A live item",
            &serde_json::json!({ "revisionDate": "2030-01-01T00:00:00.000000Z" }),
        )
        .to_string();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_body(crate::test_http::Matcher::PartialJson(serde_json::json!({
                "aKeyNoClientModels": "keep me from the per-id read",
                "id": "live-1"
            })))
            .with_body(answer)
            .create();

        let mut item = backend.get_item("live-1").expect("the item");
        item.name = "Renamed".to_string();
        let answered = backend.update_item(&item).expect("the edit lands");
        put.assert();

        // The server's copy, not the caller's: a `revisionDate` a caller
        // keeps across a write is stale, and the next edit of the same item
        // is refused if it holds its own. See `VaultBridge::update_item`.
        assert_eq!(
            answered.other.get("revisionDate").and_then(serde_json::Value::as_str),
            Some("2030-01-01T00:00:00.000000Z")
        );
    }

    /// **The write quotes the sync it just made, not the copy in the caller's
    /// hand.**
    ///
    /// Two bug reports, one cause. "Couldn't add \"Secnote\" to your
    /// favourites" and then "Couldn't save your changes to \"Secnote\"", both
    /// answered by the server with "The client copy of this cipher is out of
    /// date. Resync the client and try again."
    ///
    /// The first was read as "do not send the token" and the mapper stopped
    /// sending it. That is what the second report disproves: Save and the
    /// star are one call, so the body refused the second time had no token in
    /// it at all. The token is wanted; what was wrong was its AGE. The item a
    /// window hands back can be a whole session old, while `write_through`'s
    /// own `synced()` ran milliseconds ago.
    ///
    /// The stale value here is deliberately far in the past, so a body
    /// carrying it could not be mistaken for the fresh one by a matcher
    /// comparing loosely.
    ///
    /// # Three dates, because the read moved
    ///
    /// This was `..._of_the_sync_and_not_the_callers_stale_one` and had two
    /// dates in it, which was enough while the fresh one could only have come
    /// from `/api/sync`. It cannot any more: `update_item` reads `GET
    /// /api/ciphers/{id}`, and a test with two dates would pass on a body that
    /// quoted the **sync's** token -- the very thing this change was supposed
    /// to stop being fetched.
    ///
    /// So the three sources are given three distinct values and the body must
    /// carry the middle one:
    ///
    /// * `1999` -- the caller's copy, a whole window session old.
    /// * `2021` -- what the sync fixture says, which nothing should now read.
    /// * `2025` -- what the per-id read answers, which is the server's current
    ///   copy of the cipher about to be written and is the only right answer.
    #[test]
    fn a_write_quotes_the_revision_date_of_the_read_and_not_the_callers_stale_one() {
        let (mut server, backend) = logged_in();

        // The per-id route, overriding the fixture's: registered later, so it
        // wins. See `declare_the_per_id_cipher_route` on the ordering rule.
        server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(
                cipher(
                    "live-1",
                    "A live item",
                    &serde_json::json!({ "revisionDate": "2025-05-05T00:00:00.000000Z" }),
                )
                .to_string(),
            )
            .expect_at_least(1)
            .create();

        let mut item = backend.get_item("live-1").expect("the item");
        assert_eq!(
            item.other.get("revisionDate").and_then(serde_json::Value::as_str),
            Some("2025-05-05T00:00:00.000000Z"),
            "control: the per-id read is not answering the date this test is about, so the \
             assertion below could be satisfied by the sync's token instead"
        );
        assert!(
            sync_payload().contains("2021-01-01T00:00:00.000000Z"),
            "control: the sync fixture no longer carries a DIFFERENT date, so a body quoting \
             the sync would pass this test"
        );

        // A caller holding a copy from an earlier fetch -- which is every
        // caller: the vault window's item list is as old as the window.
        item.other
            .insert("revisionDate".to_string(), serde_json::json!("1999-01-01T00:00:00.000000Z"));
        item.name = "Renamed".to_string();

        let answer = cipher(
            "live-1",
            "Renamed",
            &serde_json::json!({ "revisionDate": "2030-01-01T00:00:00.000000Z" }),
        )
        .to_string();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_body(crate::test_http::Matcher::PartialJson(
                serde_json::json!({ "revisionDate": "2025-05-05T00:00:00.000000Z" }),
            ))
            .with_body(answer)
            .create();

        backend.update_item(&item).expect(
            "the edit must land: a body carrying the caller's 1999 token, or the sync's 2021 \
             one, matches no route here -- which is what a real server answers with a 400 \
             refusal",
        );
        put.assert();
    }

    /// **The measurement this whole change exists for: an edit issues ZERO
    /// `GET /api/sync` requests.**
    ///
    /// # Why a count, and why this count
    ///
    /// The owner's self-hosted server keeps its ciphers in Cloudflare D1,
    /// which caps **rows read** per day. `wrangler d1 insights` for the day it
    /// began answering `500 Database not initialized`: 3,558 runs of the
    /// per-user cipher select at 1,687 rows each, and 3,190 runs of the
    /// attachments join at 1,687 -- about **3,374 rows for one `/api/sync`**,
    /// against a five-million-row day. Starring an item cost that. Renaming
    /// one cost that. Dragging one into a folder cost that.
    ///
    /// No assertion about latency, bytes or method calls can see that: the
    /// quantity is HTTP requests to one route, so the assertion is a count of
    /// them, exactly as `the_totp_poll_reads_the_snapshot` had to be.
    ///
    /// `signed_in_with_no_sync_route` and not `logged_in`, so the sync route
    /// declared here is the **only** one that can answer and the number it
    /// reports is the number of syncs the backend performed. See that
    /// function's doc for why an overlapping second mock would quietly measure
    /// the wrong one.
    ///
    /// # The one sync is the vault load, and it is the control
    ///
    /// `expect(1)` rather than `expect(0)`, because a vault window does open
    /// with a `list_vault` and that sync is the right price for it. It is also
    /// what makes the zero meaningful: a `0` on a route nothing ever reaches
    /// would pass on a backend that could not sync at all. One load, then four
    /// gestures, and the number does not move.
    #[test]
    fn an_edit_issues_no_sync_at_all_and_reads_only_the_record_it_is_about() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        declare_the_per_id_cipher_route(&mut server);
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .with_body(cipher("live-1", "A live item", &serde_json::json!({})).to_string())
            .expect(3)
            .create();

        // The one sync the whole gesture is allowed: the load a vault window
        // does when it opens. Everything after this line is a click.
        let vault = backend.list_vault().expect("the vault load");
        let item = vault.items.first().expect("the live item").clone();

        // A rename.
        let mut renamed = item.clone();
        renamed.name = "Renamed".to_string();
        backend.update_item(&renamed).expect("the rename lands");

        // A star. There is no `set_favorite` on the trait -- the window flips
        // the field and edits, which is the gesture the owner's report named
        // and the one that used to cost a whole vault.
        let mut starred = item.clone();
        starred.favorite = true;
        backend.update_item(&starred).expect("the star lands");

        // A move out of every folder, which goes through `update_item` too.
        backend.move_item_to_folder(&item, None).expect("the move lands");

        // And a plain read, which is the fill path on a cache miss.
        backend.get_item("live-1").expect("the read");

        sync.assert();
        put.assert();
    }

    /// A backend that has never synced still answers one record for one sync
    /// -- and every record after it for none.
    ///
    /// This is `app::fill_from_vault` on a cold process: nothing has loaded a
    /// vault, so nothing has unwrapped a key, and a single-cipher response
    /// carries no `profile` to unwrap one from. The first read therefore pays
    /// for a sync it cannot avoid. **The assertion is that it pays exactly
    /// once**, which is what says the keys were kept rather than re-fetched
    /// per record -- the defect that would make this change worthless while
    /// looking like it worked.
    ///
    /// The trashed and the archived cipher are read too. The per-id route
    /// knows nothing of the three lists, so an id is answerable whichever list
    /// its item is in, exactly as the old sync-and-filter was.
    #[test]
    fn a_cold_backend_pays_for_the_keys_once_and_every_record_after_that_is_free() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        declare_the_per_id_cipher_route(&mut server);

        assert_eq!(backend.get_item("live-1").expect("the first read").name, "A live item");
        assert_eq!(backend.get_item("trash-1").expect("the trashed one").name, "A trashed item");
        assert_eq!(backend.get_item("arch-1").expect("the archived one").name, "An archived item");
        assert_eq!(backend.get_item("live-1").expect("again").name, "A live item");

        sync.assert();
    }

    /// The item a per-id read produces and the item the sync produces are the
    /// **same item**, field for field.
    ///
    /// Both go through [`decrypt_cipher`]'s `map_cipher`, so this should be
    /// true by construction -- and it is asserted anyway, because "by
    /// construction" is what was believed about the two folder mappers before
    /// one of them started answering an empty name. A `get_item` that dropped
    /// `other`, or filtered a field the list keeps, would be a per-item read
    /// that quietly disagreed with every list in the app.
    #[test]
    fn the_record_a_per_id_read_produces_is_the_one_the_sync_produces() {
        let (_server, backend) = logged_in();
        let by_id = backend.get_item("live-1").expect("by id");
        let listed = backend.list_items().expect("the list").remove(0);
        assert_eq!(
            serde_json::to_value(&by_id).expect("a value"),
            serde_json::to_value(&listed).expect("a value"),
            "the two reads of one cipher disagree about its contents"
        );
    }

    /// **The invalidation case the cache cannot rule out by construction: an
    /// organisation joined since the last sync.**
    ///
    /// [`RestBackend::keys`] argues that a re-login, an account switch, a
    /// master-password change and a key rotation all either leave the cached
    /// keys correct or destroy the object holding them. This is the one event
    /// that does neither: the session stays valid, the user key is unchanged,
    /// and the cached [`VaultKeys`] simply has no entry for `org1` -- so the
    /// organisation's cipher does not decrypt at all, and would come back as
    /// an item with no name rather than as an error.
    ///
    /// So a **cached** key that fails to open a record is re-derived once from
    /// a fresh sync, and the record decrypted again. The two sync mocks make
    /// that visible: the first answers an account with no organisations and is
    /// what warms the cache; the second, registered after it, answers the same
    /// account in `org1`. `test_http` gives a request to the first mock still
    /// owing hits, so the order of the two answers is the order of the two
    /// registrations.
    ///
    /// The organisation key here is **OpenSSL's**, through
    /// `rest::crypto`'s transcribed fixture -- the same chain
    /// `rest::sync::an_organisation_cipher_decrypts_through_the_rsa_wrapped_org_key`
    /// checks against ground truth -- so a pass here is a real RSA unwrap and
    /// not a fixture agreeing with itself.
    #[test]
    fn an_organisation_joined_since_the_last_sync_re_derives_the_keys_rather_than_failing() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let before = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        let after = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_body_with_the_organisation())
            .expect(1)
            .create();
        let fetch = server
            .mock("GET", "/api/ciphers/org-1")
            .with_body(organisation_cipher())
            .expect(1)
            .create();

        // The vault load that warms the cache with keys holding no
        // organisation at all.
        backend.list_vault().expect("the vault load");

        let item = backend.get_item("org-1").expect("the organisation's item");
        assert_eq!(item.name, "Shared login");
        assert_eq!(
            item.login.as_ref().and_then(|l| l.password.as_deref()).map(String::as_str),
            Some("shared"),
            "the record decrypted its name but not its password, so only half of it went \
             through the re-derived organisation key"
        );

        // One warm-up sync and one re-derivation, and the record was fetched
        // once -- the retry re-decrypts the body it already has rather than
        // asking the server for it twice.
        before.assert();
        after.assert();
        fetch.assert();
    }

    /// **The retry happens once, and a record that still will not decrypt is
    /// an error rather than a loop.**
    ///
    /// The control for the test above, and the pin on the one way a
    /// self-healing path can be worse than the one it replaces. Here the
    /// second sync answers the same organisation-less account as the first, so
    /// the fresh keys are exactly as useless as the cached ones. The
    /// requirement is that the call ends -- with a refusal, and after
    /// **exactly two** syncs: the warm-up and the single retry.
    ///
    /// `expect(2)` is the whole assertion. A retry that re-fetched keys until
    /// they worked would answer this fixture with an unbounded number of
    /// syncs against the very server the row cap was hit on.
    #[test]
    fn a_record_that_will_not_decrypt_on_fresh_keys_either_is_refused_after_one_retry() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(2)
            .create();
        server
            .mock("GET", "/api/ciphers/org-1")
            .with_body(organisation_cipher())
            .expect(1)
            .create();

        backend.list_vault().expect("the vault load");
        let refused = backend.get_item("org-1").expect_err("no key for that organisation");
        assert!(
            matches!(refused, VaultError::Parse(ref m) if m.contains("org-1")),
            "the refusal must name the record it is about, and must not claim the server has \
             no such item -- the server answered about it: {refused:?}"
        );

        sync.assert();
    }

    /// **`retained` survives the move to the per-id read: a field that would
    /// not decrypt is written back as the very bytes the server sent.**
    ///
    /// This is the failure [`crate::rest::write`]'s module docs are mostly
    /// about, and the one the task of moving this read had to not
    /// reintroduce. [`DecryptedItem`] carries which of an item's in-place
    /// values are still ciphertext; the model's copy of such a value is
    /// **empty**, so a write built from the model alone would replace a
    /// password the user still has with an encryption of the empty string.
    ///
    /// The fixture's password is sealed under the wrong key, so it comes back
    /// as a `DecryptFailure` and stays ciphertext -- and the `PUT` must carry
    /// that exact string, character for character. Anything else is either a
    /// re-encryption (a different IV, so a different string) or a blank.
    ///
    /// `an_edit_carries_the_fields_this_crate_does_not_model_back_to_the_server`
    /// is the neighbouring rule and is **not** this one: that is about keys
    /// the model has never heard of, which ride `VaultItem::other`. This is
    /// about a key the model knows and could not read.
    #[test]
    fn an_edit_writes_back_verbatim_a_value_that_would_not_decrypt() {
        let (mut server, backend) = logged_in();
        let fixture = cipher_with_one_unreadable_field();
        let ciphertext = fixture["login"]["password"]
            .as_str()
            .expect("the fixture's unreadable password")
            .to_string();
        server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(fixture.to_string())
            .expect_at_least(1)
            .create();

        let mut item = backend.get_item("live-1").expect("the item");
        assert_eq!(
            item.login.as_ref().and_then(|l| l.password.as_deref()).map(String::as_str),
            None,
            "control: the fixture's password decrypted after all, so nothing here is retained \
             and the assertion below would pass on a plain round trip"
        );
        item.name = "Renamed".to_string();

        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_body(crate::test_http::Matcher::PartialJson(serde_json::json!({
                "login": { "password": ciphertext }
            })))
            .with_body(cipher("live-1", "Renamed", &serde_json::json!({})).to_string())
            .create();

        backend.update_item(&item).expect("the edit lands");
        put.assert();
    }

    /// One unreadable field on **freshly derived** keys is not a reason to
    /// derive them again.
    ///
    /// The `Some(record, failures)` half of the no-retry rule, where
    /// `a_record_that_will_not_decrypt_on_keys_just_derived_is_not_retried`
    /// below is the `None` half. The two are separate arms of one `match` and
    /// a mutation neutralising either one alone survives the other's test, so
    /// both are pinned.
    ///
    /// The record still comes back -- "a vault with one corrupt field is still
    /// a vault" is [`crate::rest::sync`]'s decision and this path does not
    /// overrule it -- and the sync count is **one**: the cold read's own.
    #[test]
    fn one_unreadable_field_on_keys_just_derived_is_not_retried() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(cipher_with_one_unreadable_field().to_string())
            .expect(1)
            .create();

        let item = backend.get_item("live-1").expect("the item, corrupt field and all");
        assert_eq!(item.name, "A live item");
        sync.assert();
    }

    /// The same record read with a **cached** key does pay for one retry, and
    /// exactly one.
    ///
    /// The honest cost of the self-healing rule, written down rather than
    /// hidden: a genuinely corrupt field is indistinguishable from a stale key
    /// at the point the decrypt fails, so a warm backend re-derives once
    /// before believing it. That is one sync -- what this whole path used to
    /// cost every single time -- and it must not become two.
    #[test]
    fn one_unreadable_field_on_a_cached_key_costs_one_retry_and_no_more() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(2)
            .create();
        server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(cipher_with_one_unreadable_field().to_string())
            .expect(1)
            .create();

        backend.list_vault().expect("the vault load that warms the keys");
        let item = backend.get_item("live-1").expect("the item, corrupt field and all");
        assert_eq!(item.name, "A live item");
        sync.assert();
    }

    /// A **fresh** key that does not open a record is not retried at all.
    ///
    /// The mirror of the two tests above, and the reason the retry is
    /// conditioned on the keys having been cached rather than on the failure
    /// alone. A cold backend derives its keys from a sync it has just made;
    /// those keys are as fresh as anything can be, and re-deriving them
    /// because a genuinely corrupt record would not open is how a vault with
    /// one bad cipher comes to issue a sync per click for ever.
    ///
    /// One sync, and one only: the cold read's own.
    #[test]
    fn a_record_that_will_not_decrypt_on_keys_just_derived_is_not_retried() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        server
            .mock("GET", "/api/ciphers/org-1")
            .with_body(organisation_cipher())
            .expect(1)
            .create();

        backend.get_item("org-1").expect_err("no key for that organisation");
        sync.assert();
    }

    /// An id this vault does not hold is refused rather than created: `PUT`
    /// is not an upsert here, and a create dressed as an edit is an item the
    /// caller believes it edited.
    #[test]
    fn an_edit_of_an_item_this_vault_does_not_hold_is_refused_before_anything_is_sent() {
        let (mut server, backend) = logged_in();
        let never = server.mock("PUT", crate::test_http::Matcher::Any).expect(0).create();
        let mut item = backend.get_item("live-1").expect("the item");
        item.id = "not-in-this-vault".to_string();
        backend.update_item(&item).expect_err("refused");
        never.assert();
    }

    /// **The 503 report, end to end.**
    ///
    /// A self-hosted server answered `503` for a few seconds and the item
    /// editor said "the vault backend refused the write", which is wrong
    /// twice: the server refused nothing, and there was no write to refuse --
    /// [`RestBackend::update_item`] calls `synced()` **before** it sends
    /// anything, so the operation dies on the sync with no `PUT` ever issued.
    /// Both halves are asserted here, at the only place that can see them
    /// both: the variant that decides the band's sentence, and the `PUT` that
    /// never happens.
    ///
    /// `signed_in_with_no_sync_route` rather than `logged_in`, so the failing
    /// sync is the ONLY sync route declared -- which mockito mock answers an
    /// overlapping pair is a property of the mocking library, and a test
    /// whose 503 was quietly out-ranked by a healthy fixture would pass while
    /// asserting nothing.
    #[test]
    fn a_sync_that_answers_503_is_unreachable_and_never_reaches_the_put() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_status(503)
            .expect_at_least(1)
            .create();
        let never = server.mock("PUT", crate::test_http::Matcher::Any).expect(0).create();
        // Deserialized rather than hand-built: every other field of
        // `VaultItem` has a serde default, and none of them can matter to a
        // write that dies before the body is mapped.
        let item: VaultItem = serde_json::from_value(serde_json::json!({
            "id": "live-1",
            "name": "Toshiba Laptop",
        }))
        .expect("the two fields without a default");

        let failed = backend.update_item(&item).expect_err("the sync answered 503");

        assert!(
            matches!(failed, VaultError::Unreachable(_)),
            "a 503 reached the vault window as {failed:?}, whose band words it as a refusal of a \
             write that was never sent"
        );
        never.assert();
    }

    /// The name and the notes really are re-encrypted rather than sent in the
    /// clear. A test that only checked the round trip would pass on a body
    /// that wrote every secret as plaintext.
    #[test]
    fn nothing_the_backend_writes_leaves_a_plaintext_on_the_wire() {
        let (mut server, backend) = logged_in();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_request(|request| {
                let body = request.utf8_lossy_body().to_string();
                !body.contains("NEEDLE-secret") && !body.contains("p4ssw0rd")
            })
            .with_body(cipher("live-1", "A live item", &serde_json::json!({})).to_string())
            .create();

        let mut item = backend.get_item("live-1").expect("the item");
        item.notes = Some(Zeroizing::new("NEEDLE-secret".to_string()));
        backend.update_item(&item).expect("the edit lands");
        put.assert();
    }

    /// Un-filing an item is an ordinary edit here, and the way it says "no
    /// folder" is by the key being **absent** from a body that replaces the
    /// whole cipher. On `bw serve` the same request needs an explicitly
    /// stated `null`, which that backend ignores unless it is spelled a
    /// particular way -- see `move_item_to_folder`'s doc for the difference
    /// and why the two must not be tidied into agreement.
    #[test]
    fn moving_an_item_out_of_every_folder_omits_the_key_rather_than_stating_it() {
        let (mut server, backend) = logged_in();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_request(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body()).expect("json");
                body.get("folderId").is_none()
            })
            .with_body(cipher("live-1", "A live item", &serde_json::json!({})).to_string())
            .create();

        let item = backend.get_item("live-1").expect("the item");
        assert_eq!(item.folder_id.as_deref(), Some("f1"), "the fixture starts filed");
        backend.move_item_to_folder(&item, None).expect("the move lands");
        put.assert();
    }

    /// An app match is an ordinary edit carrying one more custom field, built
    /// by the same [`with_app_match`] the `bw serve` backend uses -- so the
    /// field this app writes has one definition and cannot drift between the
    /// two backends.
    #[test]
    fn saving_an_app_match_writes_the_custom_field_encrypted() {
        let (mut server, backend) = logged_in();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_request(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body()).expect("json");
                let fields = body.get("fields").and_then(serde_json::Value::as_array);
                // One field, and its *label* is encrypted too -- a custom
                // field's name is user data on this wire.
                fields.is_some_and(|f| {
                    f.len() == 1
                        && f[0]
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|n| n.starts_with("2.") && !n.contains("app-match"))
                })
            })
            .with_body(cipher("live-1", "A live item", &serde_json::json!({})).to_string())
            .create();

        let item = backend.get_item("live-1").expect("the item");
        let m = crate::app_match::AppMatch::for_process(
            "RockstarGamesLauncher.exe",
            crate::app_match::TriggerMode::Prompt,
        );
        backend.set_app_match(&item, &m).expect("the match saves");
        put.assert();
    }

    /// A create goes to `POST /api/ciphers`, sends no `id`, and the **only**
    /// place the new id exists is the server's answer.
    #[test]
    fn a_create_posts_a_cipher_with_no_id_and_learns_the_id_from_the_answer() {
        let (mut server, backend) = logged_in();
        let post = server
            .mock("POST", "/api/ciphers")
            .match_request(|request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body()).expect("json");
                // An empty id is omitted, never sent as `""`.
                body.get("id").is_none() && body.get("type") == Some(&serde_json::json!(1))
            })
            .with_body(cipher("brand-new", "Made here", &serde_json::json!({})).to_string())
            .create();

        let created = backend
            .create_item(&NewItem::login("Made here", "u@example.com", "p4ssw0rd", None))
            .expect("the create lands");
        post.assert();
        assert_eq!(created.id, "brand-new");
    }

    /// The three id-only writes, each on its own route and method.
    ///
    /// Asserted together because the risk they share is one of them reaching
    /// another's route: an ordinary delete that hard-deleted, or a "delete
    /// forever" that quietly re-trashed. On `bw serve` those two differ by a
    /// query parameter, which is why that backend asserts the query is on the
    /// wire; here they differ by the HTTP **method**, so this asserts the
    /// method and the path rather than the outcome.
    #[test]
    fn the_id_only_writes_each_hit_their_own_route() {
        let (mut server, backend) = logged_in();
        let trash = server.mock("PUT", "/api/ciphers/live-1/delete").create();
        let restore = server.mock("PUT", "/api/ciphers/trash-1/restore").create();
        let purge = server.mock("DELETE", "/api/ciphers/trash-1/delete").create();

        backend.delete_item("live-1").expect("the soft delete");
        backend.restore_item("trash-1").expect("the restore");
        backend.purge_item("trash-1").expect("the hard delete");

        trash.assert();
        restore.assert();
        purge.assert();
    }

    /// The same three, from the other side: none of them syncs.
    ///
    /// An id is the whole request, so paying for the vault to delete one item
    /// would be a cost with nothing bought. Asserted by refusing the sync
    /// route outright -- a mock that expects zero calls -- rather than by
    /// counting, because a count that drifts to one is a test that still
    /// passes with a rewritten assertion.
    #[test]
    fn the_id_only_writes_do_not_pay_for_a_sync() {
        let (mut server, backend) = logged_in();
        server.mock("PUT", crate::test_http::Matcher::Any).create();
        server.mock("DELETE", crate::test_http::Matcher::Any).create();
        let no_sync =
            server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();

        backend.delete_item("live-1").expect("the soft delete");
        backend.restore_item("trash-1").expect("the restore");
        backend.purge_item("trash-1").expect("the hard delete");
        no_sync.assert();
    }

    /// A `401` has to stay a `401` all the way to the caller: it is the one
    /// failure that means re-authenticate rather than retry, and a backend
    /// that flattened it into `Http` would leave the app retrying a dead
    /// session forever.
    ///
    /// The refresh answers `401` as well, because [`RestClient`] refreshes
    /// once and retries before giving up.
    #[test]
    fn an_expired_session_reaches_the_caller_as_unauthorized() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        let grant = server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .create();
        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        drop(grant);

        // Both the sync and the refresh that follows it say no.
        server.mock("POST", "/identity/connect/token").with_status(401).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_status(401).create();

        let backend = RestBackend::new(client, authenticated);
        let err = backend.list_items().expect_err("a dead session");
        assert!(matches!(err, VaultError::Unauthorized), "{err:?}");
    }

    // ---- the TOTP arithmetic, which is this crate's and not the server's ---

    /// The seed a Bitwarden item carries is **either** a whole `otpauth://`
    /// URI or a bare base32 secret, and both are common. A backend that read
    /// only one of them would blank half of the user's TOTP rows.
    #[test]
    fn a_seed_is_read_whether_it_is_a_uri_or_bare() {
        let from_uri = read_seed(&Zeroizing::new(
            "otpauth://totp/Site:u?secret=JBSWY3DPEHPK3PXP&issuer=Site&digits=8&period=60"
                .to_string(),
        ))
        .expect("a URI seed");
        assert_eq!(from_uri.digits, 8);
        assert_eq!(from_uri.period, 60);

        let bare = read_seed(&Zeroizing::new("jbsw y3dp ehpk 3pxp".to_string()))
            .expect("a bare seed, with the spacing a setup page prints");
        // RFC 6238's defaults, applied by the parser and not restated in the
        // backend.
        assert_eq!(bare.digits, 6);
        assert_eq!(bare.period, 30);
        assert_eq!(*bare.secret, *from_uri.secret, "the two spellings are one seed");
    }

    /// A seed that cannot be used is `None`, and an `otpauth://` URI that was
    /// refused for a *reason* is not then re-read as a bare secret -- that
    /// would reinterpret a value whose meaning is already known, and an
    /// `hotp` counter read as a TOTP seed produces confident wrong codes.
    #[test]
    fn an_unusable_seed_is_none_and_a_refused_uri_is_not_retried_as_a_bare_seed() {
        assert!(read_seed(&Zeroizing::new(String::new())).is_none());
        assert!(read_seed(&Zeroizing::new("   ".to_string())).is_none());
        assert!(read_seed(&Zeroizing::new("not base32!!".to_string())).is_none());
        assert!(
            read_seed(&Zeroizing::new(
                "otpauth://hotp/Site:u?secret=JBSWY3DPEHPK3PXP&counter=1".to_string()
            ))
            .is_none(),
            "a counter-based URI was accepted as a TOTP seed"
        );
    }

    /// **Computed here, not fetched.** There is no TOTP endpoint on this API,
    /// so the code comes from this crate's existing arithmetic.
    ///
    /// Compared against the same shared function rather than against a
    /// hard-coded digit string: what this test is for is the *wiring* -- that
    /// the backend reads the item's seed and calls the one implementation --
    /// and `totp_add`'s own tests pin the arithmetic against RFC 6238's
    /// vectors.
    #[test]
    fn a_totp_code_is_computed_from_the_items_seed() {
        let (_server, backend) = logged_in();
        let code = backend.get_totp("live-1").expect("a code").expect("the item has a seed");
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");

        let auth = read_seed(&Zeroizing::new(
            "otpauth://totp/Site:u?secret=JBSWY3DPEHPK3PXP&issuer=Site".to_string(),
        ))
        .expect("the fixture seed");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_secs();
        let here = crate::vault_window::totp_add::code_at(&auth, now).expect("a code");
        // The thirty-second step can turn over between the backend's read of
        // the clock and this one, so either side of the boundary is correct.
        // Asserting only one of them is a test that fails once in a while for
        // no reason, which is worse than not asserting it.
        let next = crate::vault_window::totp_add::code_at(&auth, now + 30).expect("a code");
        assert!(code == *here || code == *next, "{code} is neither {here:?} nor {next:?}");
    }

    /// An item with no TOTP secret is `Ok(None)`, matching `bw serve` -- not
    /// an error, and not an empty string a UI would render as a code.
    #[test]
    fn an_item_with_no_seed_answers_none_rather_than_failing() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .create();
        let body = serde_json::json!({
            "profile": {
                "key": protected_user_key(),
                "privateKey": null,
                "organizations": []
            },
            "folders": [],
            "ciphers": [cipher("no-totp", "A note", &serde_json::json!({ "login": null }))]
        })
        .to_string();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(body).create();
        // **The per-id route, because this no longer syncs.** `get_totp` reads
        // the one record it needs rather than the whole vault; the sync above
        // stays mocked because the keys still come from it the first time they
        // are wanted.
        let one = server
            .mock("GET", "/api/ciphers/no-totp")
            .with_body(cipher("no-totp", "A note", &serde_json::json!({ "login": null })).to_string())
            .create();

        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        let backend = RestBackend::new(client, authenticated);
        assert_eq!(backend.get_totp("no-totp").expect("no failure"), None);
        one.assert();
    }

    /// **A vault the server says has not moved is not fetched again.**
    ///
    /// The whole of tier 2, as one assertion: two reads that each used to be
    /// a whole `GET /api/sync` are one sync and two one-row questions. On the
    /// owner's D1-backed server that is ~1,686 rows against 1.
    ///
    /// `list_trash` twice rather than `list_vault` twice, because the Trash
    /// row is the case that made this worth doing: opening it read the entire
    /// account, and it is not a vault load by any reading.
    #[test]
    fn an_unchanged_revision_is_not_a_second_sync() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let revision = server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect_at_least(2)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();

        let first = backend.list_trash().expect("the first read");
        let second = backend.list_trash().expect("the second read");

        sync.assert();
        revision.assert();
        assert_eq!(
            first.iter().map(|i| &i.id).collect::<Vec<_>>(),
            second.iter().map(|i| &i.id).collect::<Vec<_>>(),
            "the cached read answered something different from the sync it came from"
        );
        assert!(!first.is_empty(), "the fixture vault has a trashed item; this read found none");
    }

    /// **And a vault the server says HAS moved is.**
    ///
    /// The other half, and the one that makes the test above mean something:
    /// without it, a cache that never refreshed would pass it perfectly.
    #[test]
    fn a_changed_revision_is_fetched_again() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        // A different answer each time, so the second read cannot match the
        // revision the first was filed under.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&asked);
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body_from_request(move |_| {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                format!("{}", 1_700_000_000_000_u64 + n as u64).into_bytes()
            })
            .expect_at_least(2)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(2)
            .create();

        backend.list_trash().expect("the first read");
        backend.list_trash().expect("the second read");

        sync.assert();
    }

    /// **A server that will not answer the cheap question is synced, every
    /// time.**
    ///
    /// The gate exists to avoid work, so failing it must cost nothing but the
    /// work it was avoiding. A `None` here is never stored, so nothing can be
    /// reused without having been re-validated -- which is the property the
    /// cache rests on, held even against a server that has no such route.
    #[test]
    fn a_backend_that_cannot_ask_the_revision_still_reads_the_vault() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_status(404)
            .expect_at_least(0)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(2)
            .create();

        backend.list_trash().expect("the first read");
        backend.list_trash().expect("the second read");

        sync.assert();
    }

    /// This backend has to be usable from the threads `VaultCache` hands it
    /// to. A compile-time assertion, because [`VaultBackend`]'s own doc says
    /// the `Send + Sync` bound is not decoration.
    #[test]
    fn the_backend_is_the_shared_thing_the_trait_requires() {
        fn assert_shared<T: VaultBackend + Send + Sync + 'static>() {}
        assert_shared::<RestBackend>();
        let (_server, backend) = logged_in();
        let shared: std::sync::Arc<dyn VaultBackend> = std::sync::Arc::new(backend);
        assert_eq!(shared.list_items().expect("through the trait object").len(), 1);
    }

    /// **Which failures this backend calls a refusal, and which it calls an
    /// unreachable server.**
    ///
    /// [`rest_error`] is the fan-in where every `RestError` becomes the type
    /// the vault window words for a user, so it is where "the vault backend
    /// refused the write" was being said about a server that had answered
    /// `503` -- or had not answered at all. Both directions are asserted:
    /// a classifier that called everything unreachable would fix the report
    /// and make a genuine `400` (a stale revision token, a concurrent edit)
    /// read as an outage, sending the user to check their network instead of
    /// re-opening the item.
    #[test]
    fn a_server_that_did_not_serve_is_not_reported_as_one_that_refused() {
        for unreachable in [
            RestError::Transport("dns error".to_string()),
            RestError::Status(500),
            RestError::Status(503),
        ] {
            let mapped = rest_error(unreachable);
            assert!(
                matches!(mapped, VaultError::Unreachable(_)),
                "a server that never served the request became {mapped:?}"
            );
        }
        for refusal in [
            RestError::Status(400),
            RestError::Status(404),
            RestError::Rejected {
                error: "invalid_request".to_string(),
                description: "the client copy of this cipher is out of date".to_string(),
            },
        ] {
            let mapped = rest_error(refusal);
            assert!(
                matches!(mapped, VaultError::Http(_)),
                "a refusal the user has to act on became {mapped:?}"
            );
        }
    }

    /// No error this backend produces may carry a secret.
    ///
    /// The refusals are `&'static str` literals by construction, so the live
    /// risk is the two mapping functions -- and the one worth checking is
    /// `crypto_error`, which formats a [`CryptoError`]. That type's own rule
    /// is that it names what was wrong and never what it was wrong *about*;
    /// this asserts the rule survives the trip through here.
    #[test]
    fn no_error_this_backend_produces_carries_a_secret() {
        let mapped = crypto_error(CryptoError::MacMismatch);
        let text = format!("{mapped:?}");
        assert!(text.contains("cryptography failed"), "{text}");
        assert!(!text.contains("2."), "an EncString reached an error message: {text}");

        for refusal in [
            crypto_error(CryptoError::Malformed("a named shape")),
            rest_error(RestError::Transport("dns error".to_string())),
            rest_error(RestError::Status(503)),
        ] {
            let text = format!("{refusal:?}");
            assert!(!text.contains("master"), "{text}");
            assert!(!text.contains("AT-1") && !text.contains("RT-1"), "{text}");
        }
    }
}

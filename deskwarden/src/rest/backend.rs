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
//! **Every operation here begins with `GET /api/sync` and a decryption of the
//! whole vault.** On `bw serve` a `list_items` is one loopback request against
//! a process that already holds the plaintext; here it is a WAN round trip
//! carrying every cipher the account has, followed by an AES-CBC decrypt and
//! an HMAC verification of every field of every one of them.
//!
//! That is not hidden behind a cache in this file, deliberately.
//! [`crate::vault_cache::VaultCache`] **is** the cache this app has, it is the
//! thing call sites already go through, and a second cache underneath it is
//! how two caches come to disagree about a vault. The cost is stated instead,
//! per operation, in each method's doc, so a caller choosing between two ways
//! to ask the same question can see which one is cheaper.
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
//!   `get_item` to refresh its `revisionDate` -- one full sync per gesture.
//!   Still true, and still the price of a gesture rather than of a window.
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

use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::app_match::AppMatch;
use crate::otpauth::{self, OtpAuth};
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
        Self { client, state: Mutex::new(authenticated) }
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

    /// **One `GET /api/sync`, decrypted**: every item, every folder, and the
    /// keys the write path needs to put anything back.
    ///
    /// The keys are unwrapped a second time here, beside
    /// [`decrypt_vault`]'s own unwrap. That is one AES-CBC decrypt and one
    /// HMAC over sixty-four bytes -- next to nothing against the sync that
    /// just happened -- and it buys keeping [`decrypt_vault`]'s signature as
    /// it is rather than widening the read path's return type for the sake of
    /// the write path.
    ///
    /// Decryption failures are logged, not returned. A vault with one corrupt
    /// field is still a vault, which is [`crate::rest::sync`]'s own decision;
    /// what this adds is that the fact reaches a log line instead of being
    /// dropped on the floor. The log carries **counts and field names only**
    /// -- [`crate::rest::sync::DecryptFailure`] holds nothing else by
    /// construction.
    /// **The keys alone**, from the same `GET /api/sync` and without
    /// decrypting a single cipher.
    ///
    /// [`VaultKeys::unwrap_from`] needs `response.profile` and nothing else,
    /// so the three write paths that want a key and no items -- a folder
    /// create, a folder rename, an item create -- were paying a whole-vault
    /// decrypt (every field of every cipher, under every organisation key)
    /// and then dropping the result on the floor. The round trip is the same;
    /// what this saves is the CPU and the plaintexts that were briefly built
    /// for nobody.
    ///
    /// [`Self::synced`] is still what an *edit* needs: it wants the item's
    /// decryption record as well as the keys.
    fn keys_only(&self) -> Result<VaultKeys, VaultError> {
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
        Ok(keys)
    }

    fn synced(&self) -> Result<(DecryptedVault, VaultKeys), VaultError> {
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
        Ok((vault, keys))
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
        let cipher = encrypt_item(&record.carrying(item), keys).map_err(crypto_error)?;
        let mut state = self.locked();
        let answer = send(&self.client, &mut state, &cipher).map_err(rest_error)?;
        drop(state);
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

    /// The decrypted cipher with this id, or a refusal naming the id as
    /// missing.
    ///
    /// Not found is [`VaultError::Http`] and not [`VaultError::Unsupported`]:
    /// the operation *is* supported, the server simply has no such record --
    /// which on `bw serve` arrives as a 404 through the same variant.
    fn find<'v>(vault: &'v DecryptedVault, id: &str) -> Result<&'v DecryptedItem, VaultError> {
        vault.items.iter().find(|d| d.item.id == id).ok_or_else(|| {
            // The id is a server-assigned GUID and appears in URLs, so it is
            // not a secret; it is also the only thing that makes this message
            // actionable.
            VaultError::Http(format!("this vault holds no item with the id {id}"))
        })
    }
}

// ---- the trait ---------------------------------------------------------------

impl VaultBackend for RestBackend {
    /// **Cost: one full sync.** Every live item, in one WAN round trip plus a
    /// whole-vault decryption. See the module docs.
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(Self::live(&vault))
    }

    /// **Cost: one full sync**, for one item.
    ///
    /// This is the operation whose cost differs most from `bw serve`'s, where
    /// it is a `GET /object/item/{id}` that exists precisely so the fill path
    /// need not pull the whole vault. There is no per-item endpoint on the
    /// sync payload, so pulling the whole vault is what asking for one item
    /// *is* here. `app::fill_from_vault` reaches this only on a cache miss.
    ///
    /// Searches trashed and archived items too, as `GET /object/item/{id}`
    /// does: an id the caller holds is an id it may legitimately ask about
    /// whichever list the item is currently in.
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(Self::find(&vault, id)?.item.clone())
    }

    /// **Cost: one full sync.** The folder names ride the same payload as the
    /// ciphers and cannot be asked for on their own, so a `populate` that
    /// wants items and folders pays for the vault twice.
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        let (vault, _) = self.synced()?;
        Ok(vault.folders)
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
        Ok(crate::vault_cache::VaultSnapshot { items: Self::live(&vault), folders: vault.folders })
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

    /// **Cost: one full sync, then one `PUT /api/ciphers/{id}`.**
    ///
    /// The sync is not optional and it is not a cache miss: it supplies the
    /// keys *and* the item's decryption record, which the trait's signature
    /// cannot carry (see [`DecryptedItem::carrying`]). An edit sent without
    /// the record would either bury a field that never decrypted or, worse in
    /// the other direction, write one in the clear -- the failure
    /// [`crate::rest::write`]'s module docs are mostly about.
    ///
    /// **An id this vault does not hold is refused rather than created.**
    /// `PUT` on Bitwarden is not an upsert, and a create dressed as an edit
    /// would be an item with no `revisionDate` history and a caller that
    /// believes it edited something.
    ///
    /// Returns the server's copy, for the reason
    /// [`crate::vault_bridge::VaultBridge::update_item`] gives at length: the
    /// `revisionDate` in the caller's hand is stale from the moment the write
    /// lands, and the next edit of the item is refused if it is kept.
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        let (vault, keys) = self.synced()?;
        let record = Self::find(&vault, &item.id)?;
        let id = item.id.clone();
        self.write_through(record, item.clone(), &keys, move |client, state, cipher| {
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
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        let (vault, _) = self.synced()?;
        let item = Self::find(&vault, id)?;
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

/// A `login.totp` value as an [`OtpAuth`], however it was stored.
///
/// Bitwarden's `totp` field holds **either** a whole `otpauth://totp` URI or a
/// bare base32 seed, and both are common: the URI is what a scanned QR code
/// produces, the bare seed is what a user typing from a website's setup page
/// produces. `bw serve` accepts both, so this must too, or half the user's
/// TOTP items go blank on a backend switch.
///
/// **The bare seed is handled by making it a URI and re-parsing**, rather than
/// by constructing an [`OtpAuth`] here. That is deliberate: every rule about
/// what a seed may contain -- the base32 alphabet, the padding, the case, the
/// length bound -- then has exactly one implementation, in
/// [`crate::otpauth`], and a seed this crate would refuse to import is a seed
/// it also refuses to compute from. The RFC 6238 defaults a bare seed implies
/// (SHA-1, six digits, thirty seconds) are applied by that same parser, so
/// they are not restated here either.
///
/// `Zeroizing` throughout: the intermediate URI is a seed with twenty-odd
/// characters in front of it.
fn read_seed(stored: &Zeroizing<String>) -> Option<OtpAuth> {
    match otpauth::parse_otpauth(stored) {
        Ok(auth) => return Some(auth),
        // Anything that *is* an `otpauth://` URI and was still refused is
        // refused for a reason -- an `hotp` counter this app cannot advance,
        // an unknown parameter, a bad seed -- and re-reading it as a bare
        // seed would be reinterpreting a value whose meaning is already
        // known.
        Err(refusal) if refusal != otpauth::OtpRefusal::NotOtpAuth => return None,
        Err(_) => {}
    }
    // Whitespace only: a seed copied off a setup page arrives in groups of
    // four. Everything else about the value is the parser's business.
    let mut bare = Zeroizing::new(String::with_capacity(stored.len()));
    bare.extend(stored.chars().filter(|c| !c.is_whitespace()));
    if bare.is_empty() {
        return None;
    }
    let uri = Zeroizing::new(format!("otpauth://totp/?secret={}", bare.as_str()));
    otpauth::parse_otpauth(&uri).ok()
}

/// A [`RestError`] as the error type the rest of the app already handles.
///
/// Three destinations, and the split is by what a caller must *do*:
///
/// * `Unauthorized` stays itself. It is the one failure that means
///   re-authenticate rather than retry, which is the whole reason
///   [`VaultError::Unauthorized`] exists.
/// * `Parse` becomes `Parse` -- the server answered, and the answer was not
///   the shape this client reads.
/// * Everything else, transport and status and crypto alike, becomes `Http`,
///   whose user-facing wording is "the backend refused". A crypto failure is
///   not literally HTTP; it is put here rather than in `Parse` because "the
///   answer couldn't be read" would send a reader looking at JSON when the
///   problem is a key.
///
/// **No arm carries a secret.** [`RestError`]'s own doc asserts that of every
/// one of its variants, and this only formats them.
fn rest_error(e: RestError) -> VaultError {
    match e {
        RestError::Unauthorized => VaultError::Unauthorized,
        RestError::Parse(what) => VaultError::Parse(format!("the server's answer was missing {what}")),
        other => VaultError::Http(other.to_string()),
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
        (server, backend)
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
    #[test]
    fn an_edit_carries_the_fields_this_crate_does_not_model_back_to_the_server() {
        let (mut server, backend) = logged_in();
        let answer = cipher(
            "live-1",
            "A live item",
            &serde_json::json!({ "revisionDate": "2030-01-01T00:00:00.000000Z" }),
        )
        .to_string();
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .match_body(crate::test_http::Matcher::PartialJson(
                serde_json::json!({ "aKeyNoClientModels": "keep me", "id": "live-1" }),
            ))
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

        let client = RestClient::new(server.url());
        let authenticated = fixture_login(&client);
        let backend = RestBackend::new(client, authenticated);
        assert_eq!(backend.get_totp("no-totp").expect("no failure"), None);
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

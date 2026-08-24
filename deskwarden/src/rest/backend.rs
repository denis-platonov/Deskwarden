//! [`VaultBackend`] over a Bitwarden server, with no `bw` CLI anywhere.
//!
//! [`crate::rest::api`] is the HTTP, [`crate::rest::sync`] the read mapping
//! and [`crate::rest::write`] the write mapping. This file is the *seam*: the
//! twenty operations of [`VaultBackend`] expressed in terms of those three,
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
//! The three call sites worth knowing about, because they are the ones that
//! were free before:
//!
//! * [`crate::vault_cache::VaultCache::populate`] calls `list_items` **and**
//!   `list_folders`, which is **two** full syncs per populate.
//! * [`crate::bw_serve::wait_for_vault_ready`] polls `list_items` on a retry
//!   schedule -- one full sync per attempt.
//! * `VaultCache`'s restore/unarchive path reads the item back with
//!   `get_item` to refresh its `revisionDate` -- one full sync per gesture.
//!
//! # What this backend refuses, and why refusing is the answer
//!
//! Four of the twenty are [`VaultError::Unsupported`], each naming itself:
//! [`RestBackend::generate`], and the three folder writes. Two more --
//! [`RestBackend::archive_item`] and [`RestBackend::unarchive_item`] -- are
//! refused for a reason of their own. Each method's doc carries the argument;
//! the shared half is that this crate would rather crash than answer a
//! question it cannot answer, and an `Ok` with an empty list is the quietest
//! possible wrong answer.

use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::app_match::AppMatch;
use crate::otpauth::{self, OtpAuth};
use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::CryptoError;
use crate::rest::sync::{DecryptedItem, DecryptedVault, VaultKeys, decrypt_cipher, decrypt_vault};
use crate::rest::write::encrypt_item;
use crate::vault_backend::VaultBackend;
use crate::vault_bridge::{
    Folder, GenerateRequest, NewItem, VaultError, VaultItem, with_app_match,
};

/// The name every refusal in this file signs itself with.
///
/// One constant rather than six literals: a log reader grepping for the
/// backend that refused should find every line, and a rename should not be
/// able to leave five of them behind.
const BACKEND: &str = "the direct-REST vault backend";

/// A Bitwarden server, as one of this app's twenty-operation vault backends.
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

    /// **Cost: one full sync, then one `PUT`.**
    ///
    /// [`with_app_match`] and then an ordinary edit -- the same composition
    /// `bw serve`'s backend makes, from the same function, so the custom field
    /// this app writes has one definition and cannot drift between backends.
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.update_item(&with_app_match(item, m))
    }

    /// **Refused.** There is no folder endpoint in [`crate::rest::api`].
    ///
    /// The Bitwarden API does have one (`POST /api/folders`), and writing it
    /// is a task; inventing it inside a backend implementation is not, because
    /// a folder name is encrypted under the user key and a folder write has
    /// its own full-replace semantics to get right. Refusing here is the
    /// honest state of this crate, and it is visible: a Folders sidebar that
    /// says "couldn't create -- this backend can't" is a bug report, while one
    /// that silently creates nothing is a mystery.
    fn create_folder(&self, _name: &str) -> Result<Folder, VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "create_folder",
            why: "this crate's REST client has no folder endpoints yet -- only ciphers -- and a \
                  backend will not invent one",
        })
    }

    /// **Refused**, for [`Self::create_folder`]'s reason exactly.
    fn update_folder(&self, _id: &str, _name: &str) -> Result<Folder, VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "update_folder",
            why: "this crate's REST client has no folder endpoints yet -- only ciphers -- and a \
                  backend will not invent one",
        })
    }

    /// **Refused**, for [`Self::create_folder`]'s reason exactly.
    ///
    /// Worth stating separately for this one: deleting a folder on Bitwarden
    /// moves every item in it to no folder, so a guessed endpoint that half
    /// worked would be a guess with the user's whole vault downstream of it.
    fn delete_folder(&self, _id: &str) -> Result<(), VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "delete_folder",
            why: "this crate's REST client has no folder endpoints yet -- only ciphers -- and a \
                  backend will not invent one",
        })
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
        let (_, keys) = self.synced()?;

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

    /// **Refused, and this is the refusal worth arguing about.**
    ///
    /// `bw serve` has `POST /archive/item/{id}`. [`crate::rest::api`] has five
    /// cipher endpoints and none of them is archive, so implementing this
    /// would mean writing a new route -- and the route is the part that is not
    /// known here. Bitwarden's own archive is a *bulk* endpoint taking a list
    /// of ids, not the per-id shape this signature has, and the self-hosted
    /// server this work was done against documents whole subsystems as not
    /// implemented.
    ///
    /// **The alternative that must not be taken** is expressing an archive as
    /// an ordinary edit that sets `archivedDate`. `archivedDate` is
    /// server-assigned; a full-replace `PUT` carrying one would at best be
    /// ignored -- returning `Ok` on an item that stayed exactly where it was,
    /// which is the "reports success while doing nothing" failure this crate
    /// treats as worse than a crash -- and at worst would corrupt a field the
    /// server owns. `bw serve`'s own `archive_item` doc records that a 200
    /// there does not even prove the state changed, on a route that really
    /// exists.
    ///
    /// So: a named refusal, and an open decision for the owner. See the
    /// implementation report on this branch.
    fn archive_item(&self, _id: &str) -> Result<(), VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "archive_item",
            why: "this crate's REST client has no archive endpoint, and faking one with an edit \
                  that sets `archivedDate` would report success for a write the server ignores",
        })
    }

    /// **Refused**, for [`Self::archive_item`]'s reason.
    ///
    /// Note that the two backends do not even *shape* this the same way:
    /// `bw serve` has no unarchive route either and reaches it through
    /// `POST /restore/item/{id}`, the same route as an un-trash, selected by
    /// the item's state. The API's `restore` is a trash-only operation, so
    /// [`Self::restore_item`] cannot quietly stand in for this one -- calling
    /// it on an archived item would be a request about the wrong state.
    fn unarchive_item(&self, _id: &str) -> Result<(), VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "unarchive_item",
            why: "this crate's REST client has no archive endpoint, and the API's restore route \
                  un-trashes rather than un-archives, so it cannot stand in",
        })
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

    /// **Refused. There is no server endpoint for this at all**, which
    /// [`crate::vault_backend`]'s module docs say in as many words.
    ///
    /// `bw serve`'s `GET /generate` is `bw`'s **own** generator: its wordlist,
    /// its character classes, its ambiguous-character rules. A backend without
    /// `bw` cannot ask anyone for a password, so implementing this would mean
    /// *this crate writing a password generator* -- choosing an alphabet, an
    /// entropy source and a passphrase wordlist, and quietly becoming the
    /// thing that decides how strong every password this app creates is.
    ///
    /// That is a decision for the owner of this crate, taken deliberately and
    /// reviewed, not one slipped in as the last function of a backend
    /// implementation. So it is refused by name and recorded as open.
    fn generate(&self, _request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        Err(VaultError::Unsupported {
            backend: BACKEND,
            operation: "generate",
            why: "the Bitwarden API has no password generator; supplying one would mean this \
                  crate inventing its own, which is the owner's decision and not a backend's",
        })
    }
}

// ---- the small shared pieces -------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::Device;
    use crate::rest::crypto::tests::{key_from_64, seal};
    use crate::rest::crypto::{Kdf, SymmetricKey, master_key};
    use crate::vault_bridge::PasswordRecipe;

    /// The master password every fixture below logs in with. Not a secret:
    /// nothing here reaches a real server, a real vault or `%APPDATA%`.
    const PASSWORD: &[u8] = b"master";
    const EMAIL: &str = "fixture@example.invalid";

    fn device() -> Device {
        Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC")
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

    /// A logged-in backend against a `mockito` server that answers prelogin,
    /// the grant and `/api/sync`.
    ///
    /// **One PBKDF2 iteration**, because the derivation is not what any test
    /// in this file is checking and six hundred thousand of them per test is
    /// a suite nobody runs; `crypto.rs` pins the real cost separately. The
    /// server is returned so the caller can add write mocks to it and so it
    /// outlives the backend.
    fn logged_in() -> (mockito::ServerGuard, RestBackend) {
        let mut server = mockito::Server::new();
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
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_body())
            .expect_at_least(1)
            .create();

        let client = RestClient::new(server.url());
        let authenticated =
            client.authenticate(EMAIL, PASSWORD, &device()).expect("the fixture login");
        (server, RestBackend::new(client, authenticated))
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

    /// An id the vault does not hold is a refusal that names the id, and
    /// specifically **not** [`VaultError::Unsupported`] -- which would tell a
    /// caller to stop attempting this operation entirely.
    #[test]
    fn an_unknown_id_is_refused_and_is_not_confused_with_an_unsupported_call() {
        let (_server, backend) = logged_in();
        let err = backend.get_item("nope").expect_err("no such item");
        assert!(matches!(err, VaultError::Http(ref m) if m.contains("nope")), "{err:?}");
    }

    /// **The heart of the refusal contract.** Every operation this backend
    /// cannot do says so by name; none of them answers `Ok` with nothing in
    /// it.
    ///
    /// One list rather than six tests, so that a seventh refusal added later
    /// is one line here and the shared property -- names the backend, names
    /// the operation, gives a reason -- is stated once.
    #[test]
    fn every_operation_this_backend_cannot_do_refuses_by_name() {
        let (_server, backend) = logged_in();
        let refusals: Vec<(&str, VaultError)> = vec![
            ("create_folder", backend.create_folder("x").expect_err("refused")),
            ("update_folder", backend.update_folder("f1", "x").expect_err("refused")),
            ("delete_folder", backend.delete_folder("f1").expect_err("refused")),
            ("archive_item", backend.archive_item("live-1").expect_err("refused")),
            ("unarchive_item", backend.unarchive_item("arch-1").expect_err("refused")),
            (
                "generate",
                backend
                    .generate(&GenerateRequest::Password(PasswordRecipe::default()))
                    .err()
                    .expect("refused"),
            ),
        ];
        for (expected, error) in refusals {
            let VaultError::Unsupported { backend: who, operation, why } = error else {
                panic!("{expected} did not refuse by name: {error:?}");
            };
            assert_eq!(operation, expected);
            assert_eq!(who, BACKEND);
            assert!(why.len() > 30, "{expected}'s reason is not a reason: {why}");
        }
    }

    /// A refusal must be distinguishable from a server that said no, because
    /// the two call for opposite responses: retry, or never retry.
    #[test]
    fn a_refusal_is_not_a_transport_failure() {
        let (_server, backend) = logged_in();
        let refused = backend.create_folder("x").expect_err("refused");
        assert!(!matches!(refused, VaultError::Http(_) | VaultError::Parse(_)), "{refused:?}");
        assert!(!matches!(refused, VaultError::Unauthorized), "{refused:?}");
    }

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
            .match_body(mockito::Matcher::PartialJson(
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
        let never = server.mock("PUT", mockito::Matcher::Any).expect(0).create();
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
                let body = request.utf8_lossy_body().expect("a body").to_string();
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
                    serde_json::from_slice(request.body().expect("a body")).expect("json");
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
                    serde_json::from_slice(request.body().expect("a body")).expect("json");
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
                    serde_json::from_slice(request.body().expect("a body")).expect("json");
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
        let purge = server.mock("DELETE", "/api/ciphers/trash-1").create();

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
        server.mock("PUT", mockito::Matcher::Any).create();
        server.mock("DELETE", mockito::Matcher::Any).create();
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
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        let grant = server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .create();
        let client = RestClient::new(server.url());
        let authenticated =
            client.authenticate(EMAIL, PASSWORD, &device()).expect("the fixture login");
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
        let mut server = mockito::Server::new();
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
        let authenticated =
            client.authenticate(EMAIL, PASSWORD, &device()).expect("the fixture login");
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

//! The seam between this app and *whatever* is holding the vault.
//!
//! # What this is
//!
//! Every vault operation the app performs goes through one of exactly twenty
//! calls. Until now those calls were inherent methods on
//! [`crate::vault_bridge::VaultBridge`], the client for the `bw serve`
//! subprocess, and `VaultCache` stored that concrete type -- so "talk to the
//! vault" and "talk to `bw serve` over loopback" were the same sentence and
//! there was no place to say the first without the second.
//!
//! [`VaultBackend`] is that place, and it is deliberately nothing more: it is
//! the existing twenty methods, with the existing signatures, moved behind a
//! name that does not mention `bw`. `VaultBridge` implements it by delegating
//! to the methods it already had -- **no route, no header, no timeout and no
//! error mapping changed** in the introduction of this trait. Behaviour is
//! carried by the impl, and the only impl today is the one that was already
//! running.
//!
//! # Why a trait rather than an enum
//!
//! Both shapes were considered; the constraint that decides it is this
//! crate's ban on `cfg(test)` seams in production code.
//!
//! An enum -- `Bw(VaultBridge)` beside a future `Rest(..)` -- dispatches by
//! `match`, which is closed: every backend that exists must be named in this
//! file. A test that wants a backend answering from a table in memory (no
//! socket, no `bw`, no mockito server on a port this machine's ephemeral
//! range is fighting over) would have to add a variant to production code, or
//! hide one behind the `cfg(test)` this crate does not allow. A trait is
//! open: a test module writes its own `impl VaultBackend` beside the test
//! that needs it, and production code neither knows nor gains a line.
//!
//! The `fn`-pointer seams used elsewhere in this crate are the right shape
//! for *one* injected call; this is twenty that must move together and share
//! a connection, which is what an object with methods is.
//!
//! The cost is dynamic dispatch. It is not a cost worth measuring here: every
//! one of these calls is an HTTP round trip, or is about to become one.
//!
//! # For whoever writes the second backend
//!
//! **Eighteen of these twenty operations are vault data.** A backend talking
//! to a Bitwarden server directly can serve them from `GET /api/sync` plus
//! the cipher and folder write endpoints.
//!
//! **[`VaultBackend::get_totp`] and [`VaultBackend::generate`] are the two
//! that cannot be**, and they are the two a non-`bw` backend must supply
//! itself:
//!
//! * `get_totp` -- a TOTP code is not a field the server stores or returns.
//!   `bw serve` computes it locally from the item's `login.totp` seed, and so
//!   must any replacement. This crate already has the arithmetic:
//!   [`crate::otpauth`] parses the `otpauth://totp` URI and renders the code.
//! * `generate` -- there is no server endpoint at all. This is `bw`'s own
//!   password/passphrase generator, and a backend without `bw` has to
//!   generate the string itself; [`crate::vault_bridge::GenerateRequest`] is
//!   the whole of what a caller asks for.
//!
//! Neither is wired here. This module states the fact so that the second
//! backend's author meets it while reading the trait rather than while
//! debugging an empty TOTP row.

use crate::app_match::AppMatch;
use crate::vault_bridge::{Folder, GenerateRequest, NewItem, VaultBridge, VaultError, VaultItem};
use zeroize::Zeroizing;

/// The twenty vault operations, as the app performs them.
///
/// See the module docs for why this is a trait, and for the two operations a
/// backend that is not `bw serve` has to compute for itself.
///
/// **`Send + Sync`**: `VaultCache` lives in an `Arc` shared between the UI
/// thread and the detached threads that run TOTP polls and readiness probes,
/// and those threads hold an owned handle of their own
/// ([`crate::vault_cache::VaultCache::backend_handle`]). A backend that could
/// not cross a thread boundary would not be usable by the app that exists.
///
/// **No `sync`, `lock` or `unlock` here, deliberately.** Those are lifecycle
/// operations on the `bw serve` *process* and on the session, and they live
/// in `bw_serve` and `session_store` where they always have. This trait is
/// the vault's data, and only that.
pub trait VaultBackend: Send + Sync {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError>;
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError>;
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError>;
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError>;
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError>;
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError>;
    fn delete_folder(&self, id: &str) -> Result<(), VaultError>;
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError>;
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError>;
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError>;
    fn delete_item(&self, id: &str) -> Result<(), VaultError>;
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError>;
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError>;
    fn archive_item(&self, id: &str) -> Result<(), VaultError>;
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError>;
    fn restore_item(&self, id: &str) -> Result<(), VaultError>;
    fn purge_item(&self, id: &str) -> Result<(), VaultError>;
    /// **Computed, not fetched.** See the module docs: no server returns
    /// this, and a backend without `bw` must derive it from the item's seed.
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError>;
    /// **No server endpoint exists for this at all.** See the module docs.
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError>;
}

/// `bw serve`, as a backend.
///
/// Every method delegates to the inherent method of the same name on
/// [`VaultBridge`], which is unchanged and remains the only place the routes,
/// the two agents and the error mapping are written.
///
/// The delegation is kept **here** rather than folded into `vault_bridge.rs`
/// on purpose: that file carries a source-text guard
/// (`every_mutating_route_uses_the_write_agent_and_every_read_the_read_one`)
/// which splits its own production half on `pub fn` and counts routes by HTTP
/// verb. Twenty more function bodies in that file, none of them routes, is
/// noise that guard would have to be taught to ignore -- and a guard taught
/// to ignore things is a guard with a hole in it.
impl VaultBackend for VaultBridge {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        VaultBridge::list_items(self)
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        VaultBridge::get_item(self, id)
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        VaultBridge::list_folders(self)
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        VaultBridge::set_app_match(self, item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        VaultBridge::create_folder(self, name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        VaultBridge::update_folder(self, id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::delete_folder(self, id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        VaultBridge::create_item(self, new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        VaultBridge::update_item(self, item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        VaultBridge::move_item_to_folder(self, item, folder_id)
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::delete_item(self, id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        VaultBridge::list_trash(self)
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        VaultBridge::list_archive(self)
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::archive_item(self, id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::unarchive_item(self, id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::restore_item(self, id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        VaultBridge::purge_item(self, id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        VaultBridge::get_totp(self, id)
    }
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        VaultBridge::generate(self, request)
    }
}

/// A boxed backend **is** a backend.
///
/// [`crate::vault_cache::VaultCache::new`] and its disk-cache twin are
/// generic over `impl VaultBackend + 'static` -- deliberately, so that every
/// caller passes the value itself and the constructor does the boxing (see
/// their own docs). That shape has exactly one caller it cannot serve: the
/// one that *chooses* between two backends at run time and therefore holds
/// the two in the same binding.
///
/// That caller is `main`'s startup, and the choice is
/// [`crate::backend_policy::choose`]. Without this impl the two arms of that
/// `match` have different types and the cache would have to be built twice --
/// once per arm, with the disk cache, the fingerprint and the enabled flag
/// spelled out on both sides -- which is how the two arms come to differ in
/// something other than the backend.
///
/// So the choice is made once, into a `Box<dyn VaultBackend>`, and the cache
/// is built once out of it. `VaultCache` then boxes that box into its `Arc`;
/// that is one extra pointer hop per vault operation, against an operation
/// that is an HTTP round trip either way -- the same trade [`VaultBackend`]'s
/// own doc already made for dynamic dispatch.
use std::sync::Arc;

/// **The vault service's door, with the encrypted cache behind it.**
///
/// Wraps another [`VaultBackend`] and answers `get_item` from the version 2
/// disk file when it can, falling through when it cannot. Everything else
/// goes straight to `inner`.
///
/// # Why this exists at all
///
/// Before it, a consumer that wanted one item without a live backend called
/// `VaultCache::item_from_disk` -- a direct file read, from `app`. That is a
/// second way to reach the vault, and
/// `docs/superpowers/specs/2026-08-27-one-door-to-the-vault.md` says there
/// is one. With the cache behind the door, a consumer holds a
/// `VaultBackend` and cannot tell whether a cache exists, which is what
/// lets the read path become a setting rather than a shape.
///
/// # Reads may be cached. Writes never are.
///
/// **Every write goes to `inner`, unconditionally.** A caching layer that
/// answered a write from a file would be a vault that disagrees with the
/// server -- and this crate has already paid for that defect class once, in
/// `rest::write`, where a `PUT` built from a partial model deleted fields the
/// server was holding. The rule here is mechanical so it cannot be reasoned
/// away per method.
///
/// `list_items` is deliberately **not** cached either. A caller asking for
/// the whole vault is asking for the vault, and `VaultCache`'s snapshot
/// already serves that from memory; a second whole-vault path through the
/// file would be two answers to one question.
///
/// # It answers nothing once the file is closed
///
/// `DiskCache::open_item` returns `None` after `close`, which the vault's
/// lock calls. So a locked vault falls through to `inner` -- which will
/// refuse, because it is locked too. That is the correct shape: the cache
/// must not be a way around a lock.
pub struct CachingBackend {
    inner: Arc<dyn VaultBackend>,
    disk: Arc<crate::vault_disk_cache::DiskCache>,
}

impl CachingBackend {
    #[must_use]
    pub fn new(inner: Arc<dyn VaultBackend>, disk: Arc<crate::vault_disk_cache::DiskCache>) -> Self {
        Self { inner, disk }
    }
}

impl VaultBackend for CachingBackend {
    /// The one read that consults the file.
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        if let Some(item) = self.disk.open_item(id) {
            return Ok(item);
        }
        self.inner.get_item(id)
    }

    // ---- everything below goes straight through --------------------------
    //
    // Written out rather than delegated by a macro on purpose: a new method
    // on the trait fails to compile here until somebody decides which side
    // of the read/write line it falls on. A blanket forward would silently
    // make that decision for them, and the decision is the whole point of
    // this type.

    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.inner.list_items()
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        self.inner.list_folders()
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.inner.set_app_match(item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        self.inner.create_folder(name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        self.inner.update_folder(id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.inner.delete_folder(id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        self.inner.create_item(new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.inner.update_item(item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        self.inner.move_item_to_folder(item, folder_id)
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.inner.delete_item(id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.inner.list_trash()
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.inner.list_archive()
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        self.inner.archive_item(id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        self.inner.unarchive_item(id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        self.inner.restore_item(id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.inner.purge_item(id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        self.inner.get_totp(id)
    }
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        self.inner.generate(request)
    }
}

impl VaultBackend for Box<dyn VaultBackend> {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_items()
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        (**self).get_item(id)
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        (**self).list_folders()
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        (**self).set_app_match(item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        (**self).create_folder(name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        (**self).update_folder(id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        (**self).delete_folder(id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        (**self).create_item(new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        (**self).update_item(item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        (**self).move_item_to_folder(item, folder_id)
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).delete_item(id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_trash()
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_archive()
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).archive_item(id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).unarchive_item(id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).restore_item(id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).purge_item(id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        (**self).get_totp(id)
    }
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        (**self).generate(request)
    }
}

// ---- a backend that is chosen before it is credentialed ---------------------

/// A [`VaultBackend`] slot that answers [`VaultError::Unauthorized`] until
/// something puts a real backend in it.
///
/// # Why this exists, and why it is not an empty `RestBackend`
///
/// `main` builds [`crate::vault_cache::VaultCache`] **before** it knows
/// whether this launch has usable credentials: the cache is what the
/// encrypted disk copy is loaded into, what the tray and autofill are built
/// out of, and what the startup window is handed -- all of which happen above
/// the point at which a sign-in can have produced anything.
///
/// On the `bw serve` path that ordering is free: [`VaultBridge`] is a base
/// URL and nothing else, and it is perfectly constructible before the
/// subprocess it addresses exists. [`crate::rest::backend::RestBackend`] is
/// not: it *owns* the session and the master key (see its module doc, which
/// says so as a design decision rather than an accident), so there is no
/// "empty" `RestBackend` and there should not be one -- a constructor that
/// took no credentials would be a constructor every one of its twenty methods
/// then had to re-check.
///
/// So the emptiness lives **here**, in one small type outside `rest/`, and it
/// is emptiness of exactly one kind: *no credentials have arrived yet*. That
/// is [`VaultError::Unauthorized`]'s own meaning -- "re-authenticate", not
/// "retry" -- which is why this answers that and not [`VaultError::Http`].
/// Every caller in this app already has a path for it.
///
/// # It is a one-way door, deliberately
///
/// [`Self::adopt`] may be called more than once (a re-auth replaces dead
/// credentials with live ones), but there is no `clear`. A slot that could be
/// emptied again would be a slot in which a vault operation could see
/// `Unauthorized` for a reason that is not "the credentials are dead", and
/// the recovery for the two is not the same. Signing out clears the
/// *account's* secrets ([`crate::accounts::AccountSecret`]); this slot is the
/// process's, and the process ends with the sign-out.
pub struct LateBoundBackend {
    inner: std::sync::RwLock<Option<Box<dyn VaultBackend>>>,
}

impl Default for LateBoundBackend {
    fn default() -> Self {
        Self::empty()
    }
}

impl LateBoundBackend {
    /// An empty slot. Every operation answers [`VaultError::Unauthorized`]
    /// until [`Self::adopt`].
    #[must_use]
    pub fn empty() -> Self {
        Self { inner: std::sync::RwLock::new(None) }
    }

    /// A slot that is already filled.
    ///
    /// The launch that finds a usable stored key takes this arm, so that the
    /// window in which the app runs against an empty slot is not merely short
    /// but does not exist.
    #[must_use]
    pub fn filled(backend: Box<dyn VaultBackend>) -> Self {
        Self { inner: std::sync::RwLock::new(Some(backend)) }
    }

    /// Puts a credentialed backend in the slot, replacing anything already
    /// there.
    pub fn adopt(&self, backend: Box<dyn VaultBackend>) {
        match self.inner.write() {
            Ok(mut slot) => *slot = Some(backend),
            // Recovered from rather than propagated, for the reason
            // `RestBackend::locked` gives: what is guarded is a handle, not a
            // half-updated invariant, and refusing to install live
            // credentials because an unrelated thread panicked once would
            // turn a recoverable fault into a dead app.
            Err(poisoned) => *poisoned.into_inner() = Some(backend),
        }
    }

    /// Whether a real backend is in the slot.
    #[must_use]
    pub fn is_filled(&self) -> bool {
        self.read(Option::is_some)
    }

    fn read<T>(&self, f: impl FnOnce(&Option<Box<dyn VaultBackend>>) -> T) -> T {
        match self.inner.read() {
            Ok(slot) => f(&slot),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }

    /// `f` against the backend in the slot, or `Unauthorized`.
    fn with<T>(
        &self,
        f: impl FnOnce(&dyn VaultBackend) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        self.read(|slot| match slot {
            Some(backend) => f(backend.as_ref()),
            None => Err(VaultError::Unauthorized),
        })
    }
}

/// Nothing but the emptiness: the slot holds a backend that owns a session
/// and a master key, and a `Debug` that reached either would be one more line
/// that could end up in a log. See [`crate::debug_leak_guard`].
impl std::fmt::Debug for LateBoundBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LateBoundBackend")
            .field("filled", &self.is_filled())
            .finish()
    }
}

impl VaultBackend for LateBoundBackend {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.with(|b| b.list_items())
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        self.with(|b| b.get_item(id))
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        self.with(|b| b.list_folders())
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.with(|b| b.set_app_match(item, m))
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        self.with(|b| b.create_folder(name))
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        self.with(|b| b.update_folder(id, name))
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.delete_folder(id))
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        self.with(|b| b.create_item(new_item))
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.with(|b| b.update_item(item))
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        self.with(|b| b.move_item_to_folder(item, folder_id))
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.delete_item(id))
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.with(|b| b.list_trash())
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.with(|b| b.list_archive())
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.archive_item(id))
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.unarchive_item(id))
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.restore_item(id))
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.with(|b| b.purge_item(id))
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        self.with(|b| b.get_totp(id))
    }
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        self.with(|b| b.generate(request))
    }
}

/// A shared handle to one [`LateBoundBackend`].
///
/// `VaultCache` owns its backend behind an `Arc<dyn VaultBackend>` and hands
/// out clones of that; the startup path needs a handle typed concretely
/// enough to call [`LateBoundBackend::adopt`] on, which the trait object is
/// not. So the slot is built once, an `Arc` of it is kept by `main`, and a
/// clone of that same `Arc` is what the cache is built from.
pub type SharedLateBoundBackend = std::sync::Arc<LateBoundBackend>;

/// An `Arc` of a slot is a backend too, so the shared handle can be the value
/// `VaultCache` is built from.
impl VaultBackend for SharedLateBoundBackend {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_items()
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        (**self).get_item(id)
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        (**self).list_folders()
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        (**self).set_app_match(item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        (**self).create_folder(name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        (**self).update_folder(id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        (**self).delete_folder(id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        (**self).create_item(new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        (**self).update_item(item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        (**self).move_item_to_folder(item, folder_id)
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).delete_item(id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_trash()
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        (**self).list_archive()
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).archive_item(id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).unarchive_item(id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).restore_item(id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        (**self).purge_item(id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        (**self).get_totp(id)
    }
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        (**self).generate(request)
    }
}

#[cfg(test)]
mod late_bound_tests {
    use super::*;
    use crate::vault_bridge::VaultItem;

    /// A backend that answers from memory: no socket, no `bw`, no mockito
    /// server on a port this machine's ephemeral range is fighting over.
    ///
    /// Exactly the thing this module's own header says a trait buys and an
    /// enum would not -- written beside the test that needs it, with
    /// production code neither knowing nor gaining a line.
    struct OneItem(&'static str);

    /// An item with the given id and nothing else. Spelled out rather than
    /// defaulted because `VaultItem` has no `Default`, and giving it one for
    /// a test would be production code added for a test's convenience.
    fn item(id: &str) -> VaultItem {
        VaultItem {
            id: id.to_string(),
            name: String::new(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.to_string(), name: name.to_string(), other: serde_json::Map::new() }
    }

    impl VaultBackend for OneItem {
        fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(vec![item(self.0)])
        }
        fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
            Ok(item(id))
        }
        fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
            Ok(Vec::new())
        }
        fn set_app_match(&self, item: &VaultItem, _m: &AppMatch) -> Result<VaultItem, VaultError> {
            Ok(item.clone())
        }
        fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
            Ok(folder(name, name))
        }
        fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
            Ok(folder(id, name))
        }
        fn delete_folder(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn create_item(&self, _new_item: &NewItem) -> Result<VaultItem, VaultError> {
            Ok(item("created"))
        }
        fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
            Ok(item.clone())
        }
        fn move_item_to_folder(
            &self,
            item: &VaultItem,
            _folder_id: Option<&str>,
        ) -> Result<VaultItem, VaultError> {
            Ok(item.clone())
        }
        fn delete_item(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(Vec::new())
        }
        fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(Vec::new())
        }
        fn archive_item(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn unarchive_item(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn restore_item(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn purge_item(&self, _id: &str) -> Result<(), VaultError> {
            Ok(())
        }
        fn get_totp(&self, _id: &str) -> Result<Option<String>, VaultError> {
            Ok(None)
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
            Ok(Zeroizing::new("generated".to_string()))
        }
    }

    /// **An empty slot answers `Unauthorized`, not an empty vault.**
    ///
    /// The whole reason this type exists. `Ok(vec![])` from a backend with no
    /// credentials is a vault that reads as empty, which is
    /// indistinguishable -- to the user and to the log -- from a vault that
    /// is; `Unauthorized` is this app's existing "sign in again" and every
    /// caller already has a path for it.
    #[test]
    fn an_empty_slot_refuses_rather_than_answering_an_empty_vault() {
        let slot = LateBoundBackend::empty();
        assert!(!slot.is_filled());
        assert!(matches!(slot.list_items(), Err(VaultError::Unauthorized)));
        assert!(matches!(slot.list_folders(), Err(VaultError::Unauthorized)));
        assert!(matches!(slot.get_item("anything"), Err(VaultError::Unauthorized)));
        assert!(matches!(slot.delete_item("anything"), Err(VaultError::Unauthorized)));
        assert!(matches!(slot.get_totp("anything"), Err(VaultError::Unauthorized)));
    }

    /// And once something is adopted, the calls reach it.
    ///
    /// The positive control for the test above: without it, a slot that
    /// refused everything for ever would satisfy that one.
    #[test]
    fn an_adopted_backend_answers_the_calls_the_empty_slot_refused() {
        let slot = LateBoundBackend::empty();
        slot.adopt(Box::new(OneItem("the-item")));
        assert!(slot.is_filled());
        let items = slot.list_items().expect("the adopted backend answers");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "the-item");
        assert_eq!(slot.get_item("other").expect("answers").id, "other");
    }

    /// A slot built full is full from its first call: the launch that finds a
    /// usable stored key has no window in which the app runs against an empty
    /// one.
    #[test]
    fn a_slot_built_filled_never_refuses_at_all() {
        let slot = LateBoundBackend::filled(Box::new(OneItem("stored-key")));
        assert!(slot.is_filled());
        assert_eq!(slot.list_items().expect("answers")[0].id, "stored-key");
    }

    /// **A re-auth replaces the credentials rather than adding a second
    /// backend beside them.** The one thing a switch must not leave behind is
    /// the previous account's backend still answering reads.
    #[test]
    fn adopting_again_replaces_what_was_there() {
        let slot = LateBoundBackend::filled(Box::new(OneItem("first")));
        slot.adopt(Box::new(OneItem("second")));
        assert_eq!(slot.list_items().expect("answers")[0].id, "second");
    }

    /// The shared handle is itself a backend, which is what lets `main` build
    /// the cache out of the same object it keeps in order to `adopt` into.
    #[test]
    fn the_shared_handle_is_a_backend_and_sees_a_later_adoption() {
        let slot: SharedLateBoundBackend = std::sync::Arc::new(LateBoundBackend::empty());
        let handle = std::sync::Arc::clone(&slot);
        assert!(matches!(handle.list_items(), Err(VaultError::Unauthorized)));
        // Adopted through one handle, visible through the other -- the
        // property the whole late-binding arrangement rests on.
        slot.adopt(Box::new(OneItem("late")));
        assert_eq!(handle.list_items().expect("answers")[0].id, "late");
    }

    /// `Debug` says whether the slot is filled and nothing else: what is in it
    /// owns a session and a master key.
    #[test]
    fn the_debug_form_carries_nothing_but_the_emptiness() {
        let slot = LateBoundBackend::empty();
        assert_eq!(format!("{slot:?}"), "LateBoundBackend { filled: false }");
        slot.adopt(Box::new(OneItem("x")));
        assert_eq!(format!("{slot:?}"), "LateBoundBackend { filled: true }");
    }

    /// A boxed backend is a backend, which is what lets the two arms of the
    /// startup choice share one binding.
    #[test]
    fn a_boxed_backend_delegates_to_what_is_in_the_box() {
        let boxed: Box<dyn VaultBackend> = Box::new(OneItem("boxed"));
        assert_eq!(boxed.list_items().expect("answers")[0].id, "boxed");
        assert_eq!(
            *boxed
                .generate(&GenerateRequest::Password(Default::default()))
                .expect("answers"),
            "generated"
        );
    }
}

#[cfg(test)]
mod caching_backend_tests {
    use super::*;
    use crate::vault_bridge::VaultItem;
    use crate::vault_disk_cache::tests::{cache_with_key, facts_for, temp_dir_for};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts every call that reaches it, so "the cache answered" and "the
    /// backend answered" are distinguishable -- which asserting on the
    /// returned value alone cannot do, because both would return an item.
    #[derive(Default)]
    struct Counting {
        gets: AtomicUsize,
        writes: AtomicUsize,
    }

    fn an_item(id: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "Example",
            "fields": [],
            "type": 1,
            "login": { "username": "me@example.com", "password": "hunter2" }
        }))
        .expect("the fixture item")
    }

    impl VaultBackend for Counting {
        fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Ok(an_item(id))
        }
        fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(Vec::new())
        }
        fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
            Ok(Vec::new())
        }
        fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(Vec::new())
        }
        fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
            Ok(Vec::new())
        }
        fn set_app_match(&self, item: &VaultItem, _m: &AppMatch) -> Result<VaultItem, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(item.clone())
        }
        fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(Folder { id: "f".to_string(), name: name.to_string(), other: Default::default() })
        }
        fn update_folder(&self, _id: &str, name: &str) -> Result<Folder, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(Folder { id: "f".to_string(), name: name.to_string(), other: Default::default() })
        }
        fn delete_folder(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn create_item(&self, _n: &NewItem) -> Result<VaultItem, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(an_item("new"))
        }
        fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(item.clone())
        }
        fn move_item_to_folder(
            &self,
            item: &VaultItem,
            _f: Option<&str>,
        ) -> Result<VaultItem, VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(item.clone())
        }
        fn delete_item(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn archive_item(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn unarchive_item(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn restore_item(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn purge_item(&self, _id: &str) -> Result<(), VaultError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn get_totp(&self, _id: &str) -> Result<Option<String>, VaultError> {
            Ok(None)
        }
        fn generate(&self, _r: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
            Ok(Zeroizing::new(String::new()))
        }
    }

    /// A caching backend over a real version 2 file holding `id`.
    fn over_a_file_holding(name: &str, id: &str) -> (Arc<Counting>, CachingBackend) {
        let dir = temp_dir_for(name);
        let disk = Arc::new(cache_with_key(&dir));
        disk.write("fp", &facts_for(&[]), &[an_item(id)], &[]).expect("the file");
        assert!(
            matches!(disk.load_facts("fp"), crate::vault_disk_cache::DiskFactsLoad::Loaded { .. }),
            "control: the file did not open, so nothing below tests the cache"
        );
        let inner = Arc::new(Counting::default());
        (inner.clone(), CachingBackend::new(inner, disk))
    }

    /// **The read the door exists for**: it is answered from the file, and
    /// the backend behind it is never asked.
    #[test]
    fn get_item_is_answered_from_the_file_without_asking_the_backend() {
        let (inner, backend) = over_a_file_holding("caching-hit", "item-1");
        let got = backend.get_item("item-1").expect("the item");
        assert_eq!(got.id, "item-1");
        assert_eq!(
            inner.gets.load(Ordering::SeqCst),
            0,
            "the backend was asked anyway, so the cache is decoration"
        );
    }

    /// An id the file does not hold falls through, rather than failing.
    #[test]
    fn a_miss_falls_through_to_the_backend() {
        let (inner, backend) = over_a_file_holding("caching-miss", "item-1");
        let got = backend.get_item("item-2").expect("the item");
        assert_eq!(got.id, "item-2");
        assert_eq!(inner.gets.load(Ordering::SeqCst), 1, "the miss did not reach the backend");
    }

    /// **The cache is not a way around a lock.** `close` is what the vault's
    /// lock calls; after it the file answers nothing and the read falls
    /// through to a backend which -- in the running app -- is locked too.
    #[test]
    fn a_closed_file_falls_through_rather_than_answering() {
        let dir = temp_dir_for("caching-closed");
        let disk = Arc::new(cache_with_key(&dir));
        disk.write("fp", &facts_for(&[]), &[an_item("item-1")], &[]).expect("the file");
        assert!(matches!(
            disk.load_facts("fp"),
            crate::vault_disk_cache::DiskFactsLoad::Loaded { .. }
        ));
        let inner = Arc::new(Counting::default());
        let backend = CachingBackend::new(inner.clone(), disk.clone());
        assert!(backend.get_item("item-1").is_ok());
        assert_eq!(inner.gets.load(Ordering::SeqCst), 0, "control: it was cached before");

        disk.close();
        assert!(backend.get_item("item-1").is_ok());
        assert_eq!(
            inner.gets.load(Ordering::SeqCst),
            1,
            "a locked vault was still answered out of the cache"
        );
    }

    /// **Every write reaches the backend.** Driven one at a time rather than
    /// asserted about `get_item`'s neighbours in prose, because the failure
    /// this guards is a FUTURE method quietly answered from a file -- a vault
    /// that disagrees with the server, which this crate has already paid for
    /// once in `rest::write`.
    #[test]
    fn every_write_reaches_the_backend() {
        let (inner, backend) = over_a_file_holding("caching-writes", "item-1");
        let item = an_item("item-1");
        let m = AppMatch {
            process: "x.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: String::new(),
            args: String::new(),
            sequence: String::new(),
            trigger: crate::app_match::TriggerMode::Prompt,
        };
        backend.set_app_match(&item, &m).unwrap();
        backend.create_folder("f").unwrap();
        backend.update_folder("f", "g").unwrap();
        backend.delete_folder("f").unwrap();
        backend.update_item(&item).unwrap();
        backend.move_item_to_folder(&item, None).unwrap();
        backend.delete_item("item-1").unwrap();
        backend.archive_item("item-1").unwrap();
        backend.unarchive_item("item-1").unwrap();
        backend.restore_item("item-1").unwrap();
        backend.purge_item("item-1").unwrap();
        assert_eq!(
            inner.writes.load(Ordering::SeqCst),
            11,
            "a write was answered somewhere other than the backend"
        );
    }
}

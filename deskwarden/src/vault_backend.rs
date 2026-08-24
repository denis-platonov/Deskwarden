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

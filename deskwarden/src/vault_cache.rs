//! An in-memory snapshot of the vault, in front of `VaultBridge`.
//!
//! Every read in the app used to be an HTTP call to `bw serve`, which is
//! why it had to run permanently. Holding items here means reads -- the
//! vault window's list and the autofill match path -- never touch it, so
//! the backend is only needed for sync, writes and TOTP.
//!
//! **Memory only, by design.** Nothing here is written to disk: decrypted
//! vault data at rest would contradict the README's claim that deskwarden
//! never touches encryption or storage. `clear` drops everything; `main`
//! calls it whenever the current snapshot might outlive the session it was
//! built from -- the vault window locking itself, re-authenticating into a
//! possibly different account, and quitting -- so idle never holds stale or
//! leftover vault contents.
//!
//! **All writes go through here.** Each write updates the snapshot on
//! success, so there is exactly one place that can leave the cache stale
//! rather than one per call site.

use crate::app_match::AppMatch;
use crate::vault_bridge::{with_app_match, Folder, NewLoginItem, VaultBridge, VaultError, VaultItem};
use std::sync::Mutex;

#[derive(Default)]
struct Snapshot {
    items: Vec<VaultItem>,
    folders: Vec<Folder>,
    populated: bool,
    /// Bumped by [`VaultCache::clear`]. Every populate captures this before
    /// it starts fetching and refuses to write its result back if the value
    /// has moved on since -- see [`VaultCache::populate`].
    epoch: u64,
}

pub struct VaultCache {
    bridge: VaultBridge,
    snapshot: Mutex<Snapshot>,
}

/// What a populate actually did. Returned instead of a bare `Ok(())` because
/// "the snapshot now holds the vault" and "this result was thrown away
/// because the cache was cleared underneath it" are two different facts, and
/// every caller that could only see `Ok` read the second as the first
/// (review 14's Minor): the picker reported a *locked* vault as an empty one
/// ("your vault doesn't have any items yet"), and the tray's Sync item
/// reported a completed sync for a sync that refreshed nothing.
///
/// What actually holds the distinction is the exhaustive `match` at every
/// call site, and nothing else. The `#[must_use]` below is worth keeping but
/// is NOT the guarantee this comment used to claim (review 15's Minor):
/// because the enum is returned *inside* a `Result`, both
/// `let _ = cache.populate();` and `if let Err(_) = cache.populate() {}`
/// compile with zero warnings -- verified empirically with `rustc`, not
/// assumed. The one form that does warn, a bare `cache.populate();`, warns
/// because of `Result`'s own `#[must_use]` and would warn identically
/// without this attribute. It is kept because it costs nothing and would
/// start earning its keep the moment any API here returns this enum
/// unwrapped, not because it enforces anything today.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulateOutcome {
    /// The snapshot was replaced with the fetched vault and is populated.
    Populated,
    /// A [`VaultCache::clear`] landed while this populate was in flight, so
    /// its result was discarded and the snapshot is still empty and
    /// unpopulated. Nothing *failed* -- the answer stopped being wanted --
    /// which is why this is an `Ok` and not a `VaultError`; but it is also
    /// not data, and a caller must not present the empty cache as the vault.
    DiscardedStale,
}

impl VaultCache {
    pub fn new(bridge: VaultBridge) -> Self {
        Self {
            bridge,
            snapshot: Mutex::new(Snapshot::default()),
        }
    }

    /// The underlying bridge, for the operations that genuinely need the
    /// backend and are not cached: TOTP and `bw sync`.
    pub fn bridge(&self) -> &VaultBridge {
        &self.bridge
    }

    /// Fills the snapshot from the backend. Called once per unlock, and
    /// again after a sync.
    ///
    /// **Epoch-guarded.** Fetching happens with no lock held (deliberately --
    /// this is a real HTTP round-trip and can take seconds), and callers are
    /// free to run it on a detached background thread; `picker_ui`'s
    /// "Add app..." fallback does exactly that. So a [`Self::clear`] --
    /// which `main` performs on lock and on re-authentication, precisely
    /// because the *next* unlock may land on a different account -- can
    /// happen while this call is in flight. Without a guard, the slow
    /// success then lands afterwards and restores the pre-lock account's
    /// decrypted snapshot with `populated = true`, so `app::fill_from_vault`
    /// and the next vault-window open (which short-circuits on
    /// `is_populated`) serve the *previous* account's items and passwords
    /// while the app considers itself locked (review 13's Minor 2).
    ///
    /// A discarded result leaves the cache exactly as `clear` left it --
    /// empty and unpopulated -- and is reported as
    /// [`PopulateOutcome::DiscardedStale`] rather than an error, since
    /// nothing *failed*: the answer simply stopped being wanted. It is
    /// reported at all because an empty cache is not a vault with no items;
    /// see [`PopulateOutcome`].
    pub fn populate(&self) -> Result<PopulateOutcome, VaultError> {
        let epoch = self.epoch();
        let items = self.bridge.list_items()?;
        self.populate_with_at_epoch(items, epoch)
    }

    /// The snapshot's current epoch, for a caller that fetches the vault
    /// itself and then hands the result to [`Self::populate_with`].
    ///
    /// Capture it **before** starting that fetch: the guard can only cover
    /// the window it is given, and a `clear` that lands between a caller's
    /// own `list_items` and its `populate_with` is invisible to an epoch
    /// captured inside `populate_with` (review 14's Minor).
    pub fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// Same as [`Self::populate`], but with items already fetched by the
    /// caller instead of listing them again here.
    ///
    /// Startup's readiness probe (`bw_serve::wait_for_vault_ready`) already
    /// has to call `list_items()` itself, to confirm `bw serve` is actually
    /// answering before anything else proceeds -- so a plain `populate()`
    /// right after it repeated that exact request for data that cannot have
    /// changed in the instant between the two calls. This still fetches
    /// folders, since nothing else already has, and mirrors `populate`'s
    /// atomicity: the snapshot is only replaced if that fetch also succeeds,
    /// not left holding the given `items` with no folders to match.
    ///
    /// Epoch-guarded exactly like [`Self::populate`] -- see its doc for what
    /// the guard is for -- but the epoch is the caller's, taken from
    /// [`Self::epoch`], not one captured on entry here. What the guard
    /// actually promises is "no `clear` landed between that capture and the
    /// write", so the capture has to happen before the *caller's own fetch*
    /// for the caller's fetch to be covered at all. Capturing on entry, after
    /// the caller had already listed items, left exactly the hole the epoch
    /// exists to close: a `clear` in that window was invisible and the
    /// pre-clear account's items were written with `populated = true`
    /// (review 14's Minor -- inert at the only caller today, which runs on
    /// the main thread before any `clear` site exists, but not inert for the
    /// background-thread callers the encrypted-disk-cache work will add).
    pub fn populate_with(&self, items: Vec<VaultItem>, epoch: u64) -> Result<PopulateOutcome, VaultError> {
        self.populate_with_at_epoch(items, epoch)
    }

    fn populate_with_at_epoch(
        &self,
        items: Vec<VaultItem>,
        epoch: u64,
    ) -> Result<PopulateOutcome, VaultError> {
        let folders = self.bridge.list_folders()?;
        let mut snapshot = self.lock();
        if snapshot.epoch != epoch {
            log::info!(
                "discarding a vault populate that finished after the cache was cleared \
                 (epoch {epoch} -> {}); the snapshot stays empty",
                snapshot.epoch
            );
            return Ok(PopulateOutcome::DiscardedStale);
        }
        snapshot.items = items;
        snapshot.folders = folders;
        snapshot.populated = true;
        Ok(PopulateOutcome::Populated)
    }

    pub fn is_populated(&self) -> bool {
        self.lock().populated
    }

    pub fn items(&self) -> Vec<VaultItem> {
        self.lock().items.clone()
    }

    pub fn folders(&self) -> Vec<Folder> {
        self.lock().folders.clone()
    }

    /// Drops everything. Called on lock and on quit.
    ///
    /// Also bumps the epoch, which is what makes this actually stick against
    /// a populate that is already in flight -- see [`Self::populate`].
    pub fn clear(&self) {
        let mut snapshot = self.lock();
        snapshot.items.clear();
        snapshot.items.shrink_to_fit();
        snapshot.folders.clear();
        snapshot.folders.shrink_to_fit();
        snapshot.populated = false;
        snapshot.epoch = snapshot.epoch.wrapping_add(1);
    }

    pub fn create_item(&self, new_item: &NewLoginItem) -> Result<VaultItem, VaultError> {
        let created = self.bridge.create_item(new_item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.push(created.clone());
        }
        Ok(created)
    }

    pub fn update_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        self.bridge.update_item(item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            if let Some(existing) = snapshot.items.iter_mut().find(|i| i.id == item.id) {
                *existing = item.clone();
            }
        }
        Ok(())
    }

    /// Attaches an app match to `item` via [`VaultBridge::set_app_match`] and
    /// updates the cached copy on success, using [`with_app_match`] so the
    /// snapshot holds exactly what was sent -- not a reconstruction of it.
    ///
    /// This exists so callers never have to reach around the cache via
    /// [`Self::bridge`] to save an app match, which would leave the snapshot
    /// holding the pre-change item. Since the edit endpoint is
    /// state-replacing, a later edit of that item would then PUT the stale
    /// copy back as the item's new full state, silently deleting the app
    /// match field the server had just been told to save.
    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError> {
        self.bridge.set_app_match(item, m)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            if let Some(existing) = snapshot.items.iter_mut().find(|i| i.id == item.id) {
                *existing = with_app_match(item, m);
            }
        }
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.retain(|i| i.id != id);
        }
        Ok(())
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let created = self.bridge.create_folder(name)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.push(created.clone());
        }
        Ok(created)
    }

    pub fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        let updated = self.bridge.update_folder(id, name)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            if let Some(existing) = snapshot.folders.iter_mut().find(|f| f.id == updated.id) {
                existing.name = updated.name.clone();
            }
        }
        Ok(updated)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.retain(|f| f.id != id);
        }
        Ok(())
    }

    /// A poisoned lock means another thread panicked mid-update, which
    /// cannot corrupt anything here worse than a stale snapshot -- recover
    /// rather than propagating a panic into the UI thread.
    fn lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::TriggerMode;

    fn cache_for(url: String) -> VaultCache {
        VaultCache::new(VaultBridge::new(url))
    }

    fn items_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1},
            {"id":"2","name":"Beta","fields":[],"type":1}
        ]}}"#
    }

    fn folders_body() -> &'static str {
        r#"{"success":true,"data":{"data":[{"id":"f1","name":"Work"}]}}"#
    }

    #[test]
    fn populate_fills_the_snapshot_and_reads_come_from_it() {
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .expect(1)
            .create();

        let cache = cache_for(server.url());
        assert!(!cache.is_populated());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // Many reads, one fetch: `expect(1)` fails if a read hits HTTP.
        for _ in 0..5 {
            assert_eq!(cache.items().len(), 2);
            assert_eq!(cache.folders().len(), 1);
        }
        items.assert();
        folders.assert();
    }

    #[test]
    fn a_populate_whose_epoch_was_bumped_mid_flight_leaves_the_cache_empty() {
        // Review 13's Minor 2. `picker_ui::pick_vault_item` runs
        // `load_items_for_picker` -> `populate()` on a *detached* thread, and
        // `main` calls `clear()` when the vault window locks and the user
        // re-authenticates -- possibly into a different account. A populate
        // that started before that `clear` and succeeds after it must not
        // restore the pre-lock account's decrypted snapshot with
        // `populated = true`, or `app::fill_from_vault` and the next vault
        // window open (which short-circuits on `is_populated`) serve the
        // previous account's items while the app considers itself locked.
        //
        // The `clear()` is fired from inside the *folders* response handler,
        // so it lands strictly after the populate began fetching and
        // strictly before it tries to write -- the exact interleaving,
        // deterministically, with no sleeping.
        let mut server = mockito::Server::new();
        let cache = std::sync::Arc::new(cache_for(server.url()));
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let cache_for_handler = cache.clone();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |_| {
                cache_for_handler.clear();
                folders_body().as_bytes().to_vec()
            })
            .create();

        // `Ok`, but explicitly `DiscardedStale`: nothing failed, the answer
        // just stopped being wanted -- and a caller must be able to tell that
        // apart from a real populate, because the empty cache it leaves
        // behind is not a vault with no items in it (review 14's Minor).
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::DiscardedStale);

        assert!(
            !cache.is_populated(),
            "a populate that finished after clear() must not mark the cache populated -- \
             the next vault-window open short-circuits on exactly this flag"
        );
        assert!(cache.items().is_empty(), "the pre-clear account's items must not be restored");
        assert!(cache.folders().is_empty());
    }

    #[test]
    fn a_populate_that_finishes_without_an_intervening_clear_still_lands() {
        // The guard must not be so eager that the ordinary path stops
        // working: with no `clear()` in flight the epoch is unchanged and
        // the snapshot is written exactly as before.
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = cache_for(server.url());
        // A previous clear (bumping the epoch to a non-zero value) must not
        // poison later populates -- only one that lands *during* one does.
        cache.clear();
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        assert!(cache.is_populated());
        assert_eq!(cache.items().len(), 2);
    }

    #[test]
    fn populate_with_seeds_items_without_refetching_them() {
        // No `/list/object/items` mock at all: `populate_with` must use the
        // `items` it's given rather than listing them again itself. If it
        // did, the request would hit this unmocked endpoint and the eventual
        // `unwrap()` below would fail.
        let mut server = mockito::Server::new();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .expect(1)
            .create();

        let cache = cache_for(server.url());
        let seeded = vec![VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }];

        assert_eq!(cache.populate_with(seeded, cache.epoch()).unwrap(), PopulateOutcome::Populated);

        assert_eq!(cache.items().len(), 1);
        assert_eq!(cache.folders().len(), 1);
        assert!(cache.is_populated());
        folders.assert();
    }

    #[test]
    fn populate_with_discards_items_the_caller_fetched_before_a_clear() {
        // Review 14's Minor 3. The caller does its own `list_items` (here,
        // `seeded`), and a `clear` -- lock, or re-auth into a *different*
        // account -- lands between that fetch and the handoff. The epoch the
        // caller captured *before* fetching is what makes that window
        // visible; an epoch captured on entry to `populate_with` would see an
        // unchanged value and write the pre-clear account's items with
        // `populated = true`, which is the precise hole the epoch exists to
        // close.
        let mut server = mockito::Server::new();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = cache_for(server.url());
        let epoch = cache.epoch();
        let seeded = vec![VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }];
        cache.clear();

        assert_eq!(
            cache.populate_with(seeded, epoch).unwrap(),
            PopulateOutcome::DiscardedStale
        );
        assert!(!cache.is_populated(), "a pre-clear fetch must not mark the cache populated");
        assert!(cache.items().is_empty(), "the pre-clear account's items must not be restored");
    }

    #[test]
    fn a_failed_populate_with_leaves_the_cache_unpopulated() {
        // Mirrors `populate`'s atomicity: if the folder fetch fails, the
        // given items must not be adopted into a half-formed snapshot.
        let mut server = mockito::Server::new();
        let _f = server.mock("GET", "/list/object/folders").with_status(500).create();

        let cache = cache_for(server.url());
        let seeded = vec![VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }];

        assert!(cache.populate_with(seeded, cache.epoch()).is_err());
        assert!(!cache.is_populated());
        assert!(cache.items().is_empty());
    }

    #[test]
    fn clear_empties_the_snapshot_so_idle_holds_no_vault_contents() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert_eq!(cache.items().len(), 2);

        cache.clear();
        assert!(cache.items().is_empty());
        assert!(cache.folders().is_empty());
        assert!(!cache.is_populated());
    }

    #[test]
    fn a_deleted_item_leaves_the_cache_immediately() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _d = server.mock("DELETE", "/object/item/1").with_status(200).create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache.delete_item("1").unwrap();

        let ids: Vec<String> = cache.items().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["2".to_string()]);
    }

    #[test]
    fn a_failed_write_leaves_the_cache_untouched() {
        // The cache must reflect the server, never an optimistic guess.
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _d = server.mock("DELETE", "/object/item/1").with_status(500).create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert!(cache.delete_item("1").is_err());
        assert_eq!(cache.items().len(), 2, "a failed delete removed it anyway");
    }

    #[test]
    fn a_renamed_folder_updates_in_place() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = server
            .mock("PUT", "/object/folder/f1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"f1","name":"Renamed"}}"#)
            .create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache.update_folder("f1", "Renamed").unwrap();

        assert_eq!(cache.folders()[0].name, "Renamed");
    }

    fn an_app_match() -> AppMatch {
        AppMatch { process: "notepad.exe".to_string(), trigger: TriggerMode::Prompt }
    }

    #[test]
    fn set_app_match_updates_the_cached_item() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = server.mock("PUT", "/object/item/1").with_status(200).create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        let m = an_app_match();

        cache.set_app_match(&item, &m).unwrap();

        let updated = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        let field = updated
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some(crate::app_match::APP_MATCH_FIELD_NAME))
            .expect("app-match field missing after set_app_match");
        assert_eq!(field.value.as_deref(), Some(m.to_field_value().as_str()));
    }

    #[test]
    fn a_failed_set_app_match_leaves_the_cache_untouched() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = server.mock("PUT", "/object/item/1").with_status(500).create();

        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();

        assert!(cache.set_app_match(&item, &an_app_match()).is_err());

        let unchanged = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(
            unchanged.fields.is_empty(),
            "a failed set_app_match modified the cached item anyway"
        );
    }

    #[test]
    fn a_write_against_a_cleared_cache_does_not_resurrect_a_one_item_snapshot() {
        // After `clear()` (idle/locked), the snapshot must stay empty until
        // the next `populate()` -- the bridge call still happens and its
        // result is still returned, but the local snapshot must not gain a
        // one-item "vault" from a stray write while locked.
        let mut server = mockito::Server::new();
        let created_body = r#"{"success":true,"data":{"id":"3","name":"Gamma","fields":[],"type":1}}"#;
        let _c = server
            .mock("POST", "/object/item")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(created_body)
            .create();

        let cache = cache_for(server.url());
        assert!(!cache.is_populated());

        let new_item = NewLoginItem {
            name: "Gamma".to_string(),
            username: String::new(),
            password: String::new(),
            folder_id: None,
        };
        cache.create_item(&new_item).unwrap();

        assert!(cache.items().is_empty(), "a write on a cleared cache resurrected a snapshot");
        assert!(!cache.is_populated());
    }
}

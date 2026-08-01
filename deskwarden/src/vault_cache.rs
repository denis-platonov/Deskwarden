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
//! rather than one per call site -- and, since review 21's Critical, exactly
//! one place that knows which ids have been written but not yet re-fetched,
//! so a populate whose fetch predates a write can no longer undo it (see
//! `populate_with_at_epoch`).

use crate::app_match::AppMatch;
use crate::vault_bridge::{with_app_match, Folder, NewLoginItem, VaultBridge, VaultError, VaultItem};
use std::sync::Mutex;

/// Which *era* of the snapshot a caller is talking about.
///
/// A new era begins at every [`VaultCache::clear`] -- lock, re-authentication
/// (possibly into a different account), quit -- and at nothing else. Two equal
/// `VaultEra`s therefore prove exactly one thing, and it is worth stating
/// both halves, because reading more into it is review 18's third finding:
///
///  * **What equality DOES prove:** the snapshot has not been dropped and
///    rebuilt in between, so it still belongs to the same vault session --
///    the same account, the same unlock. Nothing here can be another
///    account's data, and (since `clear` is the only thing that sets
///    `populated` back to `false`) it is still populated.
///  * **What equality does NOT prove:** that the snapshot's CONTENTS are
///    unchanged. `set_app_match`, `update_item`, `create_item` and
///    `delete_item` all mutate it in place and deliberately do not begin a new
///    era. That is not an oversight: a write is *newer truth* than any fetch
///    that predates it, so a consumer holding a stale era-tagged fetch does
///    not want to be told "something changed, give up" -- it wants the
///    snapshot as it stands now.
///
/// So an era answers "may I still act on the vault session I saw?", never "is
/// what I fetched still current?". A consumer that needs items should ask
/// [`VaultCache::items_unless_superseded`], which answers the first question
/// and hands back the answer to the second in one step, under one lock,
/// rather than comparing eras and then re-deriving data itself.
///
/// A *writer* -- a populate about to overwrite the snapshot wholesale -- needs
/// a strictly stronger fact than this one, and takes a whole [`VaultEpoch`]
/// instead. See there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultEra(u64);

impl std::fmt::Display for VaultEra {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A point in the snapshot's history, captured **before** a fetch starts and
/// handed back to the populate that fetch feeds: which [`VaultEra`] the
/// snapshot was in, and how many local writes had landed by then.
///
/// Two facts, two fields, deliberately not one number -- this crate's
/// repeatedly-relearnt lesson (review 21's Critical). They are asked opposite
/// questions and get opposite answers:
///
///  * the **era** decides whether the fetch may be written back AT ALL. A
///    `clear` in between means a different vault session, so the fetch is
///    discarded outright ([`PopulateOutcome::DiscardedStale`]).
///  * the **write position** decides which parts of the fetch are already
///    out of date. Everything the cache has written locally since this mark
///    was taken is newer truth than the fetch, and is re-applied over it
///    rather than either clobbering it or throwing the whole fetch away --
///    see [`VaultCache::populate_with`].
///
/// The write position is private and is NOT an identity: `VaultEpoch`'s
/// `PartialEq` is "the same point in history", which two marks taken either
/// side of a write correctly fail. A consumer asking "same vault session?"
/// wants [`Self::era`], which is the question [`VaultEra`] exists to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VaultEpoch {
    era: VaultEra,
    writes: u64,
}

impl VaultEpoch {
    /// Just the vault-session half -- what [`VaultCache::items_unless_superseded`]
    /// takes, and all a *reader* of the snapshot ever needs.
    pub fn era(&self) -> VaultEra {
        self.era
    }
}

/// One local write the snapshot has applied that no completed fetch is known
/// to reflect yet.
///
/// Only the id, never a copy of the item: the write has already been applied
/// to the snapshot, so the snapshot itself holds the data and re-reading it at
/// replay time is both cheaper and impossible to get out of step. (It also
/// keeps this log from becoming a second, longer-lived home for decrypted
/// vault contents.)
struct PendingWrite {
    /// The value of `Snapshot::writes` when this write landed. A populate
    /// replays exactly those entries whose `seq` is *after* the mark it
    /// captured before fetching.
    seq: u64,
    id: String,
    /// `true` for a delete -- the id must be removed from whatever a fetch
    /// brings back, rather than copied out of the snapshot (it isn't there).
    deleted: bool,
}

/// Records a write against `log`, replacing any earlier entry for the same id.
///
/// Last-write-wins per id is exactly right and is what bounds this log: replay
/// only ever needs the snapshot's CURRENT copy of an id (or the fact that it
/// is gone), so an id can never need two entries, and the log can never grow
/// past the number of distinct ids written since the last fetch that covered
/// them. Ordering between different ids does not matter, since replaying them
/// touches disjoint entries.
fn record_write(log: &mut Vec<PendingWrite>, seq: u64, id: &str, deleted: bool) {
    log.retain(|w| w.id != id);
    log.push(PendingWrite {
        seq,
        id: id.to_string(),
        deleted,
    });
}

/// Overlays the writes in `pending` that landed after `since` onto `fetched`,
/// taking each written value from `current` -- the snapshot as it stands now.
/// Returns how many were replayed, for the log line.
///
/// Generic over items and folders because the rule is identical for both and
/// having it once means it cannot be right for one and wrong for the other.
fn replay_writes<T: Clone>(
    fetched: &mut Vec<T>,
    current: &[T],
    pending: &[PendingWrite],
    since: u64,
    id_of: impl Fn(&T) -> &str,
) -> usize {
    let mut replayed = 0;
    for write in pending.iter().filter(|w| w.seq > since) {
        if write.deleted {
            fetched.retain(|t| id_of(t) != write.id);
        } else if let Some(local) = current.iter().find(|t| id_of(t) == write.id) {
            match fetched.iter_mut().find(|t| id_of(t) == write.id) {
                // In place, so a fetch's ordering survives an edit.
                Some(slot) => *slot = local.clone(),
                None => fetched.push(local.clone()),
            }
        } else {
            // Recorded as written but no longer in the snapshot, without a
            // delete having replaced the entry. Not reachable -- every write
            // below records only what it actually applied, and a delete
            // replaces the id's entry (see `record_write`) -- and skipped
            // rather than unwrapped, because the cost of being wrong is a
            // panic on the UI thread.
            continue;
        }
        replayed += 1;
    }
    replayed
}

#[derive(Default)]
struct Snapshot {
    items: Vec<VaultItem>,
    folders: Vec<Folder>,
    populated: bool,
    /// Bumped by [`VaultCache::clear`]. Every populate captures this before
    /// it starts fetching and refuses to write its result back if the value
    /// has moved on since -- see [`VaultCache::populate`].
    era: u64,
    /// Monotonic count of local writes applied to this snapshot. Never reset
    /// (not even by `clear`, which resets the logs below instead): its only
    /// job is to order writes against a mark taken earlier.
    writes: u64,
    /// The writes no completed fetch is known to reflect yet, one entry per
    /// id. Emptied of everything a populate's own fetch covered, and entirely
    /// by `clear`.
    pending_items: Vec<PendingWrite>,
    pending_folders: Vec<PendingWrite>,
}

impl Snapshot {
    fn epoch(&self) -> VaultEpoch {
        VaultEpoch {
            era: VaultEra(self.era),
            writes: self.writes,
        }
    }

    /// Allocates the next write sequence number. `saturating_add` rather than
    /// `+`: an overflow would need ~1.8e19 writes and is not worth a panic on
    /// a write path, and saturating leaves the counter merely unable to order
    /// two writes rather than wrapping and ordering them backwards.
    fn next_write_seq(&mut self) -> u64 {
        self.writes = self.writes.saturating_add(1);
        self.writes
    }

    fn note_item_write(&mut self, id: &str, deleted: bool) {
        let seq = self.next_write_seq();
        record_write(&mut self.pending_items, seq, id, deleted);
    }

    fn note_folder_write(&mut self, id: &str, deleted: bool) {
        let seq = self.next_write_seq();
        record_write(&mut self.pending_folders, seq, id, deleted);
    }
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
    ///
    /// **Write-guarded too**, and for a window this one opens itself: the
    /// `list_items` below and the `list_folders` inside are both real HTTP
    /// round-trips, and a write can land in between. See
    /// [`Self::populate_with`] for what happens to it.
    pub fn populate(&self) -> Result<PopulateOutcome, VaultError> {
        let epoch = self.epoch();
        let items = self.bridge.list_items()?;
        self.populate_with_at_epoch(items, epoch)
    }

    /// The snapshot's current epoch, for a caller that fetches the vault
    /// itself and then hands the result to [`Self::populate_with`].
    ///
    /// Capture it **before** starting that fetch: both guards it carries can
    /// only cover the window they are given. A `clear` that lands between a
    /// caller's own `list_items` and its `populate_with` is invisible to an
    /// epoch captured inside `populate_with` (review 14's Minor), and so is a
    /// write (review 21's Critical) -- with the difference that the write
    /// window is *always* open, because `populate_with` fetches folders after
    /// the caller has already fetched items.
    pub fn epoch(&self) -> VaultEpoch {
        self.lock().epoch()
    }

    /// The snapshot's items **as they stand now**, or `None` if a
    /// [`Self::clear`] has started a new era since `era` was captured.
    ///
    /// This is the one place that answers "is a result computed back in
    /// `era` still applicable, and if so what should I act on?". It exists
    /// because those are two questions that were being answered by two
    /// different mechanisms, and the pairing was wrong (review 18's third
    /// finding): callers compared eras to decide applicability and then
    /// acted on a `Vec` frozen at their own, earlier fetch. Since a write
    /// mutates the snapshot without starting a new era (see [`VaultEra`]),
    /// "the era still matches" was read as "so my frozen copy is still the
    /// snapshot", which it never was -- and an app match saved while a sync
    /// was in flight was silently overwritten by that sync's older list.
    ///
    /// Returning the items rather than a bool is the whole point: the answer
    /// to "still applicable?" is useless without the data it applies to, and
    /// splitting them is what let them drift apart. Both facts are read under
    /// a single lock, so no `clear` can land between the check and the read.
    ///
    /// `None` also covers an unpopulated snapshot in the same era. That is
    /// unreachable today -- `clear` is the only thing that unpopulates and it
    /// begins a new era -- but this function's job is to hand back a vault, and
    /// an empty unpopulated snapshot is not one (see [`PopulateOutcome`]);
    /// deriving that fact from the era instead of checking it is exactly the
    /// inference this whole type exists to stop people making.
    pub fn items_unless_superseded(&self, era: VaultEra) -> Option<Vec<VaultItem>> {
        let snapshot = self.lock();
        if snapshot.epoch().era() != era || !snapshot.populated {
            return None;
        }
        Some(snapshot.items.clone())
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
    /// actually promises is "no `clear`, and no write, landed between that
    /// capture and the write-back", so the capture has to happen before the
    /// *caller's own fetch* for the caller's fetch to be covered at all.
    /// Capturing on entry, after the caller had already listed items, left
    /// exactly the hole the epoch exists to close: a `clear` in that window
    /// was invisible and the pre-clear account's items were written with
    /// `populated = true` (review 14's Minor -- inert at the only caller
    /// today, which runs on the main thread before any `clear` site exists,
    /// but not inert for the background-thread callers the
    /// encrypted-disk-cache work will add).
    ///
    /// **Local writes made since `epoch` survive** -- review 21's Critical.
    /// See [`Self::populate_with_at_epoch`] for the mechanism and for why it
    /// re-applies rather than refuses.
    pub fn populate_with(
        &self,
        items: Vec<VaultItem>,
        epoch: VaultEpoch,
    ) -> Result<PopulateOutcome, VaultError> {
        self.populate_with_at_epoch(items, epoch)
    }

    /// The one place a fetch is written back over the snapshot, and therefore
    /// the one place that can silently undo a local write.
    ///
    /// **Review 21's Critical, reproduced live against the real helpers.**
    /// Everything between the caller's own `list_items` and the lock below --
    /// which includes the `list_folders` round-trip on the line below, so the
    /// window is *never* empty -- is time in which `set_app_match`,
    /// `update_item`, `create_item`, `delete_item` or any of the folder
    /// writes can land. A write is newer truth than a fetch that predates it
    /// (see [`VaultEra`]), so overwriting `items` wholesale reverted it *in
    /// the cache*, which is worse than losing it in the match engine: a later
    /// vault-window edit of that item PUTs the cached copy back as the item's
    /// new state, and the item's `fields` array is always present in that body
    /// (`VaultItem::fields` has no `skip_serializing_if`, so `bw serve`'s
    /// merge-on-omitted-keys behaviour cannot save it -- see
    /// `.superpowers/sdd/put-semantics-capture.md`). Session-scoped loss
    /// becomes permanent loss.
    ///
    /// **What it does instead: re-apply, not refuse.** The snapshot keeps a
    /// small log of the ids it has written locally ([`PendingWrite`]); every
    /// entry newer than the caller's mark is replayed over the fetched data,
    /// taking the value from the snapshot itself. The result is the fetch as
    /// the server would have answered it *after* those writes, which is the
    /// truth both halves are trying to describe.
    ///
    /// Three alternatives were weighed and rejected:
    ///
    ///  * **Refusing the populate** (returning a new [`PopulateOutcome`]
    ///    variant) is safe but wasteful and gets worse the longer the fetch
    ///    takes: one saved app match would throw away a whole-vault refresh
    ///    that succeeded, leave the cache stale until some later populate got
    ///    lucky, and force every caller -- the tray's Sync item above all --
    ///    to describe a refresh that did not happen. A user editing steadily
    ///    in the vault window during a slow `bw sync` could starve the refresh
    ///    indefinitely. Replaying keeps the refresh AND the write, so
    ///    [`PopulateOutcome`] gains nothing to distinguish and deliberately
    ///    does not grow a variant; the replay is reported in the log line
    ///    instead.
    ///  * **Fetching folders before items** narrows the window but cannot
    ///    close it: the caller's fetch is outside this function either way,
    ///    and `sync_outcome_from`'s `list_items` is where the reviewer's
    ///    reproduction actually landed its write.
    ///  * **Holding the lock across the fetch** closes it and is forbidden --
    ///    the fetch is HTTP and can take seconds, and every read in the app
    ///    (autofill included) would block on it.
    ///
    /// The lock ordering is what makes this airtight rather than merely
    /// likely. Every write does its HTTP first and takes the lock afterwards,
    /// so at the moment this function holds the lock a concurrent write has
    /// either already recorded itself (and is replayed here) or has not yet
    /// touched the snapshot at all (and applies immediately after this
    /// returns, on top of the fetch). There is no third case.
    fn populate_with_at_epoch(
        &self,
        mut items: Vec<VaultItem>,
        epoch: VaultEpoch,
    ) -> Result<PopulateOutcome, VaultError> {
        let mut folders = self.bridge.list_folders()?;
        let mut snapshot = self.lock();
        if snapshot.epoch().era() != epoch.era() {
            log::info!(
                "discarding a vault populate that finished after the cache was cleared \
                 (era {} -> {}); the snapshot stays empty",
                epoch.era(),
                snapshot.epoch().era()
            );
            return Ok(PopulateOutcome::DiscardedStale);
        }

        let replayed = replay_writes(
            &mut items,
            &snapshot.items,
            &snapshot.pending_items,
            epoch.writes,
            |i| &i.id,
        ) + replay_writes(
            &mut folders,
            &snapshot.folders,
            &snapshot.pending_folders,
            epoch.writes,
            |f| &f.id,
        );
        if replayed > 0 {
            log::info!(
                "a vault populate finished after {replayed} local write(s) it could not have \
                 fetched; re-applying them over it so the newer local truth survives"
            );
        }

        snapshot.items = items;
        snapshot.folders = folders;
        snapshot.populated = true;
        // What this fetch covered is now confirmed by it; what it did not is
        // still unconfirmed by any fetch, and another populate with an older
        // mark (there is more than one producer -- see the ledger) must still
        // replay it. Retaining exactly the un-covered entries is also what
        // keeps these logs from growing across a session.
        snapshot.pending_items.retain(|w| w.seq > epoch.writes);
        snapshot.pending_folders.retain(|w| w.seq > epoch.writes);
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
    /// Also begins a new era, which is what makes this actually stick against
    /// a populate that is already in flight -- see [`Self::populate`]. It is
    /// the ONLY thing that begins one, deliberately: see [`VaultEra`] for why
    /// the writes below must not, and [`Self::items_unless_superseded`] for
    /// what consumers should ask instead of comparing eras themselves.
    ///
    /// The pending-write logs go with the data they describe: there is no
    /// snapshot left to replay them onto, and an in-flight populate from the
    /// previous era is discarded outright rather than replayed. The write
    /// COUNTER is deliberately not reset -- it only ever has to order writes
    /// against a mark, and restarting it would let a mark from the previous
    /// era compare as newer than a write in this one.
    pub fn clear(&self) {
        let mut snapshot = self.lock();
        snapshot.items.clear();
        snapshot.items.shrink_to_fit();
        snapshot.folders.clear();
        snapshot.folders.shrink_to_fit();
        snapshot.pending_items.clear();
        snapshot.pending_items.shrink_to_fit();
        snapshot.pending_folders.clear();
        snapshot.pending_folders.shrink_to_fit();
        snapshot.populated = false;
        snapshot.era = snapshot.era.wrapping_add(1);
    }

    pub fn create_item(&self, new_item: &NewLoginItem) -> Result<VaultItem, VaultError> {
        let created = self.bridge.create_item(new_item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.push(created.clone());
            snapshot.note_item_write(&created.id, false);
        }
        Ok(created)
    }

    pub fn update_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        self.bridge.update_item(item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            // By index rather than `iter_mut().find()`: the write has to be
            // recorded on the same snapshot, and an outstanding `&mut` into
            // its items would still be alive inside an `if let`.
            if let Some(at) = snapshot.items.iter().position(|i| i.id == item.id) {
                snapshot.items[at] = item.clone();
                // Recorded only when the snapshot actually changed, so the
                // log can never name an id the snapshot cannot supply at
                // replay time -- see `replay_writes`.
                snapshot.note_item_write(&item.id, false);
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
            if let Some(at) = snapshot.items.iter().position(|i| i.id == item.id) {
                snapshot.items[at] = with_app_match(item, m);
                snapshot.note_item_write(&item.id, false);
            }
        }
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.retain(|i| i.id != id);
            // Unconditional, unlike the edits above: a delete has to be
            // recorded even when the snapshot did not hold the item, because
            // what it has to survive is a fetch that DOES hold it.
            snapshot.note_item_write(id, true);
        }
        Ok(())
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let created = self.bridge.create_folder(name)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.push(created.clone());
            snapshot.note_folder_write(&created.id, false);
        }
        Ok(created)
    }

    pub fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        let updated = self.bridge.update_folder(id, name)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            if let Some(at) = snapshot.folders.iter().position(|f| f.id == updated.id) {
                snapshot.folders[at].name = updated.name.clone();
                snapshot.note_folder_write(&updated.id, false);
            }
        }
        Ok(updated)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.retain(|f| f.id != id);
            snapshot.note_folder_write(id, true);
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
    fn items_unless_superseded_hands_back_a_write_that_landed_after_the_epoch_was_captured() {
        // The contract in one test (review 18's third finding). A caller
        // captures the epoch, goes away for a while, and comes back to ask
        // whether its result still applies. A write landed meanwhile. The
        // right answer is NOT "superseded" -- nothing changed vault session,
        // and the write is newer truth than anything that caller fetched --
        // it is "still applicable, and here is the snapshot INCLUDING that
        // write". An epoch comparison alone answers the first half and
        // silently invites the caller to act on its own stale copy.
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
        let epoch = cache.epoch();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.set_app_match(&item, &an_app_match()).unwrap();
        // Mechanically adapted for review 21's split of `VaultEpoch` into
        // "which era" plus "how many writes", with its meaning intact: what
        // it always asserted is that a write does not supersede a reader,
        // and the era is exactly the half that decides that. (The whole
        // epoch DOES move, and must -- that is what stops a populate from
        // reverting this write.)
        assert_eq!(
            cache.epoch().era(),
            epoch.era(),
            "a write must not start a new era"
        );

        let items = cache
            .items_unless_superseded(epoch.era())
            .expect("a write is not a supersession -- the vault session is the same one");
        let updated = items.into_iter().find(|i| i.id == "1").unwrap();
        assert!(
            updated
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(crate::app_match::APP_MATCH_FIELD_NAME)),
            "the snapshot handed back must be the live one, not a copy from when the epoch \
             was captured"
        );
    }

    #[test]
    fn items_unless_superseded_refuses_once_a_clear_has_started_a_new_epoch() {
        // The other half, so the test above cannot pass by never refusing.
        // A `clear` is a lock, a re-auth into a possibly different account,
        // or a quit: a result computed in the previous epoch must not be
        // acted on, and there is no snapshot to hand back either.
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
        let epoch = cache.epoch();
        cache.clear();

        assert!(cache.items_unless_superseded(epoch.era()).is_none());
        // ...and a *repopulate* for the new account does not make the old
        // era applicable again: this is the cross-account case, and the
        // era is what closes it.
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert!(
            cache.items_unless_superseded(epoch.era()).is_none(),
            "an era from before a clear must stay superseded even once the cache refills"
        );
        assert!(cache.items_unless_superseded(cache.epoch().era()).is_some());
    }

    /// Every mock a populate plus a write to item 1 needs.
    fn populating_server_with_a_writable_item() -> mockito::ServerGuard {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        server
            .mock("PUT", "/object/item/1")
            .with_status(200)
            .create();
        server
            .mock("DELETE", "/object/item/1")
            .with_status(200)
            .create();
        server
    }

    fn has_app_match(item: &VaultItem) -> bool {
        item.fields
            .iter()
            .any(|f| f.name.as_deref() == Some(crate::app_match::APP_MATCH_FIELD_NAME))
    }

    #[test]
    fn a_write_that_lands_while_a_populate_is_fetching_is_not_reverted_by_it() {
        // REVIEW 21'S CRITICAL, at the cache level, in the ordering the suite
        // did not cover. Everything between a populate's mark and the lock it
        // finally takes is a window: the caller's own `list_items` (a tray
        // sync's, here) and then `populate_with`'s `list_folders`, both real
        // HTTP round-trips. A write landing in it is newer truth than the
        // fetch -- and before this fix the fetch was assigned wholesale, so
        // the write was reverted IN THE CACHE. That is worse than losing it in
        // the match engine: the next vault-window edit of that item PUTs the
        // cached copy back as the item's new state, and the `fields` array is
        // always present in that body, so the loss becomes permanent.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // The sync worker: mark first (before ANY fetch), then its fetch.
        let mark = cache.epoch();
        let fetched = cache.bridge().list_items().unwrap();

        // "Add app...": the user's save lands while that fetch is in flight.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.set_app_match(&item, &an_app_match()).unwrap();

        // ...and only now does the populate write its older fetch back.
        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated,
            "the refresh itself must still land: the write is a reason to re-apply it, not a \
             reason to throw a whole-vault fetch away"
        );

        let after = cache.items();
        assert!(
            has_app_match(after.iter().find(|i| i.id == "1").unwrap()),
            "the app match saved while the populate was fetching was reverted by the populate"
        );
        assert_eq!(
            after.len(),
            2,
            "the rest of the fetch must still have landed"
        );
    }

    #[test]
    fn a_delete_that_lands_while_a_populate_is_fetching_is_not_undone_by_it() {
        // The other direction, and the one a "keep the local copy" rule gets
        // wrong if it only knows how to copy: the snapshot has no item 1 to
        // put back, so the fetch's own copy has to be REMOVED instead.
        // Without that, a deleted item reappears in the vault window and in
        // the match engine, and re-deleting it 404s.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        let mark = cache.epoch();
        let fetched = cache.bridge().list_items().unwrap();
        assert!(
            fetched.iter().any(|i| i.id == "1"),
            "the fetch must predate the delete"
        );

        cache.delete_item("1").unwrap();

        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );
        let ids: Vec<String> = cache.items().into_iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            vec!["2".to_string()],
            "an item deleted while the populate was fetching came back with the fetch"
        );
    }

    #[test]
    fn a_populate_whose_own_fetch_already_covers_a_write_replaces_it_wholesale() {
        // THE MIRROR, and the reason the write position is a sequence rather
        // than a "has anything been written?" flag. Once a fetch has been
        // taken that is NEWER than a write, that write is no longer newer
        // truth about anything: the fetch is. So a populate whose mark was
        // captured after the save must adopt what it fetched -- here a copy
        // with no app-match field on it, the shape the server would answer
        // with once that field had been removed from another client --
        // exactly as it did before this fix.
        //
        // It is also what keeps this bounded: if a replayed write were never
        // retired, every populate for the rest of the session would keep
        // re-applying it over fresher server data, and the pending log would
        // never shrink.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.set_app_match(&item, &an_app_match()).unwrap();
        let saved = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(has_app_match(&saved));

        // A later populate: mark captured AFTER the save, so whatever it
        // fetches is by definition at least as new as the save.
        let mark = cache.epoch();
        let fetched = cache.bridge().list_items().unwrap();
        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );

        let after = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(
            !has_app_match(&after),
            "a write the populate's own fetch already covers must not be replayed over it -- \
             the fetch is the newer truth by then"
        );
        assert_eq!(
            after.name, "Alpha",
            "the fetched item must be adopted as it stands"
        );
        assert_eq!(cache.items().len(), 2);
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

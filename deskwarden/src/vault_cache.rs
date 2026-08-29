//! An in-memory snapshot of the vault, in front of `VaultBridge`.
//!
//! Every read in the app used to be an HTTP call to `bw serve`, which is
//! why it had to run permanently. Holding items here means reads -- the
//! vault window's list and the autofill match path -- never touch it, so
//! the backend is only needed for sync, writes and TOTP.
//!
//! **Memory by default; optionally also on disk.** Nothing is written to
//! disk unless `Settings::cache_vault_to_disk` is on, in which case the same
//! snapshot is persisted through [`crate::vault_disk_cache`], encrypted
//! under a Windows Hello-sealed key. `clear` drops the in-memory copy;
//! `main` calls it whenever the current snapshot might outlive the session
//! it was built from -- the vault window locking itself, re-authenticating
//! into a possibly different account, and quitting -- so idle never holds
//! stale or leftover vault contents.
//!
//! `clear` deliberately **leaves the file alone**. Surviving a lock and a
//! restart is the entire reason that file exists, and quit calls `clear`
//! too. Deleting it is a separate, explicit act ([`VaultCache::forget_disk_copy`]),
//! used on re-authentication and on log out.
//!
//! **This is the only module that touches that file.** No call site
//! persists, deletes, or reasons about it directly, for the same reason
//! every write already routes through here: there is exactly one place that
//! can be wrong.
//!
//! **All writes go through here.** Each write updates the snapshot on
//! success, so there is exactly one place that can leave the cache stale
//! rather than one per call site -- and, since review 21's Critical, exactly
//! one place that knows which ids have been written but not yet re-fetched,
//! so a populate whose fetch predates a write can no longer undo it (see
//! `write_back_at_epoch`, which is the one place a snapshot is replaced
//! however the vault reached it).

use crate::app_match::AppMatch;
use crate::vault_backend::VaultBackend;
use crate::vault_bridge::{
    with_favorite, without_deleted_date, Folder, NewItem,
    VaultError, VaultItem,
};
use crate::vault_disk_cache::{DiskCache, DiskCacheLoad};
#[cfg(test)]
use crate::vault_bridge::VaultBridge;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

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
/// what I fetched still current?". A consumer that needs the vault should ask
/// [`VaultCache::snapshot_unless_superseded`], which answers the first question
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
    /// Just the vault-session half -- what
    /// [`VaultCache::snapshot_unless_superseded`] takes, and all a *reader* of
    /// the snapshot ever needs.
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
/// Returns the ids replayed, for the log line: they are vault item/folder ids,
/// not secrets (`main` already logs one on the save path), and they are exactly
/// what a post-mortem of a lost write needs.
///
/// Generic over items and folders because the rule is identical for both and
/// having it once means it cannot be right for one and wrong for the other.
///
/// **KNOWN, ACCEPTED, AND NOT TO BE "FIXED" NAIVELY** (review 23's third
/// Minor): the `None => fetched.push(..)` arm below can resurrect an item the
/// fetch legitimately dropped. If our own PUT raced a remote delete inside the
/// window -- the item was deleted on another client after our write, so the
/// fetch correctly does not contain it -- we push our local copy back and the
/// item reappears until the next populate, which retires the entry and drops
/// it for good. The alternative rule ("if the fetch does not have it, the
/// fetch is right") is strictly worse: it is exactly the Critical this replay
/// exists to close, since a CREATE the fetch predates is also absent from it,
/// and that loss is permanent rather than self-healing. Distinguishing the two
/// needs a server-side revision the bridge does not expose. A transient extra
/// row that heals itself beats a silently discarded write.
/// Names the replayed ids for the replay log line, **keeping item ids and
/// folder ids apart** -- review 25's Minor 5.
///
/// They were previously concatenated into one `Vec<String>` under the words
/// "local write(s)", which is two different id spaces rendered
/// indistinguishably: reading that line, nothing said whether `abc` was an
/// item the user had edited or a folder they had renamed, and the two are
/// looked up through different endpoints. The count stays a single total,
/// because what it measures -- how much this populate could not have fetched
/// -- genuinely is one number.
///
/// Pure and separate from the `log::info!` so the wording is testable without
/// a logger, and so it can be called after the snapshot lock is released.
fn replayed_summary(items: &[String], folders: &[String]) -> String {
    let mut parts = Vec::new();
    if !items.is_empty() {
        parts.push(format!("item(s) {}", items.join(", ")));
    }
    if !folders.is_empty() {
        parts.push(format!("folder(s) {}", folders.join(", ")));
    }
    parts.join("; ")
}

fn replay_writes<T: Clone>(
    fetched: &mut Vec<T>,
    current: &[T],
    pending: &[PendingWrite],
    since: u64,
    id_of: impl Fn(&T) -> &str,
) -> Vec<String> {
    let mut replayed = Vec::new();
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
            // delete having replaced the entry.
            //
            // THIS ARM IS REACHABLE, and the claim that it was not is review
            // 25's Important (it was true only while a populate also pruned
            // the log, which review 23 had to delete). Nothing about the
            // moment of WRITING can rule it out, because what puts the
            // snapshot into this state happens afterwards and to somebody
            // else's id: a populate whose own fetch legitimately does not
            // contain the id -- our PUT raced a remote delete -- assigns
            // `snapshot.items = items` and the id leaves the snapshot, while
            // the entry stays (the entry is retired per-populate, by
            // `seq > since`, and never deleted for the others). An OLDER
            // in-flight populate, whose mark predates the write, then arrives
            // here and finds the entry live and the snapshot empty of it.
            // `a_populate_whose_fetch_dropped_a_written_id_the_snapshot_no_longer_has_skips_it`
            // walks exactly that, deterministically.
            //
            // SKIPPING IS THE CORRECT ANSWER, not a defensive guess. The
            // entry says "the snapshot holds newer truth for this id than
            // your fetch does"; the snapshot no longer holds anything for it,
            // so there is nothing newer to re-apply and the id is correctly
            // absent from `replayed`. A fetch taken before the id was dropped
            // has nothing to say about the drop either way, so whatever this
            // fetch carries for the id is left exactly as fetched -- which is
            // the recorded fetch-vs-fetch staleness deferral (an older
            // populate's items can lag a newer one's until the next populate),
            // not a fresh loss.
            //
            // Still `continue` rather than `unwrap`/`unreachable!`: this runs
            // under the snapshot lock on whatever thread a populate landed on,
            // including the UI thread.
            continue;
        }
        replayed.push(write.id.clone());
    }
    replayed
}

/// The whole vault as one era-checked observation: the items and the folders
/// the snapshot held **at a single acquisition of the lock**.
///
/// This is a pair rather than two calls because a consumer that draws a vault
/// needs both halves to describe the same session, and nothing outside this
/// module can make that true after the fact. See
/// [`VaultCache::snapshot_unless_superseded`].
#[derive(Debug, Clone)]
pub struct VaultSnapshot {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
}

/// Why an era-checked read had no vault to hand back. Two situations, two
/// variants, because they want OPPOSITE handling from the caller and a bare
/// `Option` was making them share one (review 26's Minor 3):
///
///  * [`Self::Superseded`] -- a [`VaultCache::clear`] has begun a new era
///    since the caller's era was captured. Fetching cannot help: a populate
///    takes its own, newer epoch and refills the cache for the session that
///    exists NOW, which is not the one the caller is asking about. The caller
///    must give up, and `picker_ui` reports it as a locked vault.
///  * [`Self::Unpopulated`] -- the same era, but nothing has ever been
///    fetched into it. A populate is exactly the cure, and this is the state
///    every "Add app..." click sees before the first one lands (a fresh
///    process is era 0 and unpopulated, so an era captured before the first
///    populate compares EQUAL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultUnavailable {
    Superseded,
    Unpopulated,
}

/// What [`VaultCache::set_app_match`] actually managed to do, beyond the
/// server accepting the write.
///
/// Returned instead of a bare `Ok(())` because the write-through to the
/// snapshot can miss, and a caller that arms its match engine from that
/// snapshot needs to know it did (review 26's Minor 2). It is not a *failure*
/// -- the server accepted the write -- so it is not a [`VaultError`]; but it
/// is emphatically not "saved and live" either, and every call site that
/// could only see `Ok` read it as that.
///
/// **ONE VARIANT FOR TWO MISSES, AND THE GROUND IS "SAME IMMEDIATE CALLER
/// ACTION", NOT "SAME REMEDY"** -- review 28's Important 2 corrected this.
/// Until then the doc said both misses were cured by the next full sync.
/// That is true of the unpopulated miss and FALSE of the other one (see
/// [`Self::ServerOnly`]), so a variant justified on a shared remedy was
/// justified on something that does not hold. What both misses really share
/// is the only thing the caller can act on at the instant it gets the answer:
/// **the snapshot does not reflect this write, so do not arm anything from
/// it as though it did.** That is one fact, so it is one variant.
/// [`PopulateOutcome`]'s split exists because ITS two cases demand opposite
/// caller behaviour; these two demand identical behaviour and differ only
/// diagnostically, which is why the difference lives in two `log::warn!`
/// lines instead of the type.
///
/// Deliberately NOT `#[must_use]`: the enum travels inside a `Result`, where
/// (as [`PopulateOutcome`]'s doc records, measured rather than assumed) the
/// attribute buys almost nothing, and the existing `.unwrap();` statements in
/// `main`'s tests would warn for a value they have no reason to inspect.
/// What holds the distinction is that every production `match` on this enum is
/// exhaustive, with no catch-all, so a third variant fails to compile at each
/// of them rather than being swallowed at one. (No count and no list here --
/// review 30's Important 2 was a prose caller-enumeration in this same file
/// going stale within a commit; `grep` counts, a doc comment cannot.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMatchWrite {
    /// The server accepted the match AND the snapshot now holds the matched
    /// item, so anything rebuilt from the cache has it.
    WroteThrough,
    /// The server accepted the match but the snapshot does not hold it.
    /// Nothing is pending either -- the `position` miss skips
    /// [`Snapshot::note_item_write`], so there is no snapshot copy for a
    /// later populate's replay to take.
    ///
    /// **TWO MISSES, AND THEY DO NOT HAVE THE SAME CURE** (review 28's
    /// Important 2 -- the doc used to claim they did):
    ///
    ///  * **The cache is unpopulated.** Wide open, and cured by ANY populate:
    ///    a `clear` (lock, re-auth, quit) with nothing filled after it. The
    ///    item still exists, so the next sync really does make the match live.
    ///  * **The id is absent from a POPULATED snapshot.** Narrow, and NOT
    ///    curable by a sync. Work out what reaches it: the PUT succeeded, so
    ///    the item existed server-side when it was processed; for a populate's
    ///    fetch to then lack the id, the item must have stopped existing
    ///    between that moment and the fetch; and that populate's write-back
    ///    must beat this method's `self.lock()`, which is the statement
    ///    immediately after the PUT returns -- so the whole window is the
    ///    PUT's response-return latency. Real, but tiny. The scenario the doc
    ///    named before ("our PUT raced a remote delete") does not hold as
    ///    written: if the delete lands FIRST, `VaultBridge::set_app_match`
    ///    returns `Err` and this variant is never produced. The delete has to
    ///    come AFTER, which is exactly why no sync can bring the match back --
    ///    the item it was attached to is gone.
    ///
    /// So nothing on this path may promise the user a sync will fix it. What
    /// is true for both is that the vault has the write and this process's
    /// snapshot does not; see `picker_ui::server_only_notice` for the copy
    /// that says only that, and `main`'s `add_app_rebuild_source` for the one
    /// thing a caller can do about it without a fetch.
    ServerOnly,
}

/// The one-line post-mortem a populate leaves when it had to re-apply local
/// writes over its own fetch.
///
/// `populate_seq` is the ordering fact, and it is the point of this function
/// existing at all (review 26's Minor 1). The line is emitted OUTSIDE the
/// snapshot lock, so two concurrent populates serialise their snapshot
/// updates under the mutex but can emit their lines in the opposite order --
/// and "which populate landed first" is exactly the question a lost write is
/// investigated with. The number is allocated under the lock, so the reader
/// can order them even when the lines are not.
fn replay_log_line(populate_seq: u64, items: &[String], folders: &[String]) -> String {
    format!(
        "populate #{} finished after {} local write(s) it could not have fetched ({}); \
         re-applied them over it so the newer local truth survives. This describes the \
         snapshot AS THIS POPULATE LEFT IT, not as it stands now -- a later populate may \
         already have replaced it.",
        populate_seq,
        items.len() + folders.len(),
        replayed_summary(items, folders)
    )
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
    /// The writes this session has applied locally, one entry per id
    /// (last-write-wins -- see `record_write`, which is what bounds these).
    /// A populate does NOT prune them: whether a given entry is still newer
    /// truth is a question each populate answers against its OWN mark, and
    /// deleting entries on one populate's behalf is review 23's Critical (see
    /// `write_back_at_epoch`). Only `clear` empties them.
    pending_items: Vec<PendingWrite>,
    pending_folders: Vec<PendingWrite>,
    /// Monotonic count of populates that have reached the write-back lock,
    /// allocated under it -- including the ones discarded as stale, which are
    /// the populates most likely to explain a lost write. NOT reset by
    /// `clear`, for the same reason `writes` is not: its only job is to order
    /// events across the whole process life, and restarting it would make two
    /// populates from different eras compare equal. See `replay_log_line`.
    populates: u64,
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

    /// Allocates this populate's ordering number. `saturating_add` for the
    /// same reason [`Self::next_write_seq`] uses it: the counter only has to
    /// order events, and a panic on a log-line number would be absurd.
    fn next_populate_seq(&mut self) -> u64 {
        self.populates = self.populates.saturating_add(1);
        self.populates
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
    /// The vault backend, behind [`crate::vault_backend::VaultBackend`]
    /// rather than as the concrete `bw serve` client it used to be. The only
    /// implementation today is still that client, and nothing here knows
    /// which one it is holding -- which is the entire point of the field's
    /// type.
    ///
    /// `Arc` rather than `Box` because two callers need an owned handle that
    /// outlives the borrow: the TOTP poll in `vault_window` and the readiness
    /// probe in `picker_ui` both move one onto a detached thread. They used
    /// to `cache.bridge().clone()` a `VaultBridge`, which was cheap for the
    /// same reason this is -- the connection pools inside it were already
    /// shared. See [`Self::backend_handle`].
    bridge: Arc<dyn VaultBackend>,
    snapshot: Mutex<Snapshot>,
    /// The encrypted file, when there is a directory to put one in. `None`
    /// in the fixtures and in any use that has no business writing to disk;
    /// the methods below are then inert rather than conditional at every
    /// call site.
    disk: Option<DiskCache>,
    /// Guards `enabled`, the account fingerprint and `loaded_from_disk_at`
    /// together with the file itself, so a populate racing a "disable"
    /// cannot write the file back after the delete, and a write racing an
    /// account switch cannot key one account's vault under another's name.
    disk_state: Mutex<DiskState>,
}

#[derive(Default)]
struct DiskState {
    /// `Settings::cache_vault_to_disk`, as it stands now. Held here rather
    /// than read from a `Settings` copy so that turning the setting off and
    /// deleting the file are one ordered act -- see
    /// [`VaultCache::disable_disk_persistence`].
    enabled: bool,
    /// [`crate::vault_disk_cache::account_fingerprint`] for the account this
    /// cache is currently serving. Re-pointed by an account switch alongside
    /// the file's path; a stale one costs one wasted cold start, because
    /// `check_header` refuses it, and never wrong data.
    fingerprint: String,
    /// When the currently-held snapshot was written to disk, if it came from
    /// the file. Cleared the moment real data arrives from the backend in
    /// this session -- see [`VaultCache::loaded_from_disk_at`], which the
    /// vault window's toolbar pill reads so it reports an age instead of
    /// claiming a sync that never happened.
    loaded_from_disk_at: Option<SystemTime>,
}

/// Where a write-back's data came from, which decides two things that always
/// move together and must therefore never be set independently.
///
/// [`Source::Backend`] is new truth: the from-disk age stops applying, and
/// the file is rewritten. [`Source::DiskCache`] is the file's own contents
/// coming back in: the age becomes the file's own `written_at`, and
/// **nothing is written**. Rewriting on a restore would stamp `written_at`
/// with the current time at every launch, so the seven-day expiry would
/// never fire and the pill would report a vault that was always "just
/// written" however old it really was.
/// No `Debug`, deliberately. It carries an instant and nothing else, so it
/// could not print a secret -- but `debug_leak_guard` flags a derived `Debug`
/// on any type declared in a file that can reach one, and the honest answer
/// to that flag here is not an exemption with a paragraph of reasoning
/// attached: it is that nothing prints this and nothing needs to.
#[derive(Clone, Copy)]
enum Source {
    Backend,
    DiskCache(SystemTime),
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
    /// A cache that holds the vault in memory only. Nothing it does can
    /// reach the disk, because it has no file to reach.
    ///
    /// Generic over the backend rather than taking `Arc<dyn VaultBackend>`
    /// so that every existing caller -- and every test that wants a backend
    /// of its own -- passes the value itself and this constructor does the
    /// boxing. There is no second way to build one of these.
    pub fn new(bridge: impl VaultBackend + 'static) -> Self {
        Self {
            bridge: Arc::new(bridge),
            snapshot: Mutex::new(Snapshot::default()),
            disk: None,
            disk_state: Mutex::new(DiskState::default()),
        }
    }

    /// The same, plus the optional encrypted file.
    ///
    /// `enabled` is `Settings::cache_vault_to_disk`. With it `false` this is
    /// [`Self::new`] with a file it never touches -- not a weaker version of
    /// the persisting cache, an inert one: every method below returns early,
    /// and the test that says so asserts on the filesystem.
    pub fn with_disk_cache(
        bridge: impl VaultBackend + 'static,
        disk: DiskCache,
        fingerprint: String,
        enabled: bool,
    ) -> Self {
        Self {
            bridge: Arc::new(bridge),
            snapshot: Mutex::new(Snapshot::default()),
            disk: Some(disk),
            disk_state: Mutex::new(DiskState {
                enabled,
                fingerprint,
                loaded_from_disk_at: None,
            }),
        }
    }

    /// The underlying backend, for the operations that genuinely need it and
    /// are not cached: TOTP, password generation, and the readiness probe.
    ///
    /// Named `bridge` still, because that is what its ~25 call sites call it
    /// and renaming them is churn in a change whose whole claim is that it
    /// changed no behaviour.
    pub fn bridge(&self) -> &dyn VaultBackend {
        self.bridge.as_ref()
    }

    /// The same backend as an **owned** handle, for a caller that moves it
    /// onto a thread.
    ///
    /// Separate from [`Self::bridge`] rather than folded into it because the
    /// two are different requests and only two call sites make this one --
    /// the TOTP poll and the readiness probe, each of which detaches a thread
    /// that must not borrow the cache. Cloning the `Arc` shares one backend;
    /// it does not open a second connection pool.
    pub fn backend_handle(&self) -> Arc<dyn VaultBackend> {
        Arc::clone(&self.bridge)
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

    /// How many populates have reached the write-back lock. Test-only: it is
    /// an ordering fact for the log, not something production code may branch
    /// on -- a populate count says nothing about which vault session the
    /// snapshot belongs to, which is what [`VaultEra`] is for.
    #[cfg(test)]
    fn populate_sequence(&self) -> u64 {
        self.lock().populates
    }

    /// The vault as one observation: items **and** folders, read under a
    /// single lock and checked against `era` in the same acquisition.
    ///
    /// **THE ONE CHECKED DOOR.** Every era-checked read in the crate comes
    /// through here; there is no second door. That is the invariant, and it is
    /// deliberately stated WITHOUT enumerating the callers (review 30's
    /// Important 2): this doc used to name them and give their count, the
    /// count went stale within one commit of being written, and a prose caller
    /// list going stale is the very defect review 28's Important 1 was about --
    /// recurring inside its own fix. `grep` counts; a doc comment cannot.
    ///
    /// The items-only projection that used to sit above this,
    /// `items_unless_superseded`, is GONE: it was introduced as a two-line
    /// `.ok().map(|s| s.items)` with one caller and a recorded tripwire ("if a
    /// third caller appears, the projection has become a door again"), and the
    /// tripwire had already tripped when it was written. What it cost was not
    /// a lock -- it delegated -- but the REFUSAL: it folded `Superseded` and
    /// `Unpopulated` into one `None`, and `settle_sync_outcome` then logged
    /// "after the vault was cleared" for a refusal it had not distinguished.
    /// See that function.
    ///
    /// **The price of having only this door**, weighed rather than waved past:
    /// a caller that wants only one half still clones both, under the mutex
    /// that `app::handle_match` blocks every autofill on. The half that gets
    /// discarded in practice is `folders`, and a `Vec<Folder>` clone is cheap
    /// against the `Vec<VaultItem>` clone on the same line: a `Folder` is two
    /// `String`s plus a `#[serde(flatten)]` map that a real folder does carry
    /// entries in (at least `"object": "folder"`), where each item carries a
    /// name, a field vec, and a login with its uris. Two to three orders of
    /// magnitude apart even counting the flattened map, so it does not change
    /// the shape of the critical section. If it ever does, the answer is a
    /// borrow-based read under the guard, NOT a second door. (Not every caller
    /// discards a half -- the vault window's loader runs off the UI thread and
    /// uses both -- which is another reason not to describe the callers here.)
    ///
    /// **Why an era-checked read at all** (review 18's third finding).
    /// "Is a result computed back in `era` still applicable?" and "so what
    /// should I act on?" were being answered by two different mechanisms, and
    /// the pairing was wrong: callers compared eras to decide applicability
    /// and then acted on a `Vec` frozen at their own, earlier fetch. Since a
    /// write mutates the snapshot without starting a new era (see
    /// [`VaultEra`]), "the era still matches" was read as "so my frozen copy
    /// is still the snapshot", which it never was -- and an app match saved
    /// while a sync was in flight was silently overwritten by that sync's
    /// older list. Returning the data rather than a bool is the whole point:
    /// the answer to "still applicable?" is useless without the data it
    /// applies to, and splitting them is what let them drift apart.
    ///
    /// **Why items and folders together** (review 26's Important 2). The file
    /// had an era-checked door for items and none for folders, so the repair
    /// anyone reading it reaches for -- the checked items read followed by a
    /// bare `folders()` -- takes the lock twice with a `clear` window between
    /// the two, and LOOKS checked. The result is a refresh that paints one
    /// account's items filed under another account's folders. Same argument
    /// as above, applied to the pair rather than to check-and-data: the check
    /// and the two halves must not be able to drift apart, and the only place
    /// that can be guaranteed is inside one lock scope, here.
    /// [`Self::items`] and [`Self::folders`] remain as the UNCHECKED reads,
    /// for callers that have established both facts some other way.
    ///
    /// **Why the refusal is typed rather than an `Option`** (review 26's
    /// Minor 3). The two ways to have no vault want opposite handling from
    /// the caller -- give up versus go and fetch one -- and folding them into
    /// `None` made `picker_ui` re-derive the difference at the call site by
    /// running a whole vault populate under the spinner just to fail the
    /// re-check afterwards. See [`VaultUnavailable`] for each.
    ///
    /// The `Superseded` check comes first because it is the stronger fact:
    /// after a `clear` the snapshot is *also* unpopulated, and reporting that
    /// would invite the very populate that cannot help.
    ///
    /// `Unpopulated` in the caller's OWN era is not defensive: `era` is a
    /// `u64` starting at zero and only `clear` advances it, so the snapshot a
    /// process starts with -- never populated, era 0 -- compares EQUAL to an
    /// era captured before the first populate, which is precisely what every
    /// "Add app..." click asks about. Reading the era as a proxy for "so it
    /// is populated" would hand back an empty vault and disarm autofill.
    pub fn snapshot_unless_superseded(
        &self,
        era: VaultEra,
    ) -> Result<VaultSnapshot, VaultUnavailable> {
        let snapshot = self.lock();
        if snapshot.epoch().era() != era {
            return Err(VaultUnavailable::Superseded);
        }
        if !snapshot.populated {
            return Err(VaultUnavailable::Unpopulated);
        }
        Ok(VaultSnapshot {
            items: snapshot.items.clone(),
            folders: snapshot.folders.clone(),
        })
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
    /// See [`Self::write_back_at_epoch`] for the mechanism and for why it
    /// re-applies rather than refuses.
    ///
    /// **THE ONE RULE FOR CALLERS, and it is not enforced by the type:**
    /// capture `epoch` BEFORE the fetch that produced `items`. A mark taken
    /// afterwards is still *safe* -- it can only make the guard narrower, so
    /// the failure is a lost write or a missed `clear`, never a corrupt
    /// snapshot -- but it silently gives up the protection this parameter
    /// exists to provide. All four production captures are correct today
    /// (`main`'s startup, unlock and sync paths, and [`Self::populate`]'s own).
    ///
    /// Review 23 weighed making a late capture unrepresentable -- a token
    /// minted by a `begin_fetch()` that the caller must hand back -- and did
    /// NOT do it, because it does not actually close the hole: nothing stops a
    /// caller calling `begin_fetch()` after its own `list_items`, so it is a
    /// rename of [`Self::epoch`] with a better-placed doc.
    ///
    /// **It then recorded that the hole "cannot be enforced without
    /// re-fetching", and that is WRONG** -- review 25's finding, corrected
    /// here so nobody re-derives it. The claim only rules out inverting
    /// control (`populate_from(|| bridge.list_items())`, mark captured
    /// inside), which really is in direct conflict with why this function
    /// exists: startup and sync pass in items from a fetch THAT HAS ALREADY
    /// SUCCEEDED, deliberately, so a later transient failure cannot disarm the
    /// engine (review 16's Important). The third shape enforces it while
    /// re-fetching nothing:
    ///
    /// ```text
    /// pub struct Fetched { items: Vec<VaultItem>, epoch: VaultEpoch } // no public constructor
    /// fn fetch_items(&self) -> Result<Fetched, VaultError>            // stamps BEFORE the request
    /// fn populate_with(&self, f: Fetched) -> Result<PopulateOutcome, VaultError>
    /// ```
    ///
    /// A late capture becomes unrepresentable because there is no other way to
    /// build a `Fetched`, and review 16's requirement is preserved exactly: by
    /// the time `populate_with` is entered the items fetch has already
    /// succeeded, and the caller can still hold `f.items` to arm the engine
    /// from if the folders fetch inside later fails. [`Self::populate`] is
    /// already this shape internally; it just is not extractable by callers.
    ///
    /// WHY IT IS DEFERRED RATHER THAN DONE, which is the part nobody had
    /// named: startup and unlock do not get their items from this cache at
    /// all. They come from `bw_serve::wait_for_vault_ready`'s readiness probe,
    /// which would have to carry the stamp -- taking a `&VaultCache`, or a
    /// `|| cache.fetch_items()` closure -- so the change reaches `bw_serve.rs`
    /// and the startup path and is a task of its own. Until then the invariant
    /// stays a documented one, stated here rather than inferred.
    pub fn populate_with(
        &self,
        items: Vec<VaultItem>,
        epoch: VaultEpoch,
    ) -> Result<PopulateOutcome, VaultError> {
        self.populate_with_at_epoch(items, epoch)
    }

    /// [`Self::populate_with`]'s core: fetch the one half the caller does not
    /// have, then hand both halves to the single write-back.
    ///
    /// Everything that makes that write-back safe -- the era guard and the
    /// replay of local writes -- lives in [`Self::write_back_at_epoch`], so
    /// that it is shared verbatim with [`Self::populate_with_vault`], which
    /// fetches nothing at all. What belongs to *this* entry point and to no
    /// other is the `list_folders` below: a real HTTP round-trip, issued
    /// after the caller has already fetched its items, and therefore the
    /// reason the write window the write-back guards against is **never**
    /// empty for a fetching populate. It is exactly that round-trip that
    /// [`Self::populate_with_vault`] exists to let a caller skip.
    fn populate_with_at_epoch(
        &self,
        items: Vec<VaultItem>,
        epoch: VaultEpoch,
    ) -> Result<PopulateOutcome, VaultError> {
        let folders = self.bridge.list_folders()?;
        Ok(self.write_back_at_epoch(items, folders, epoch, Source::Backend))
    }

    /// Writes a **whole vault the caller already holds** -- items *and*
    /// folders -- back over the snapshot, fetching nothing.
    ///
    /// This is [`Self::populate_with`] with its one remaining round-trip
    /// removed. `populate_with` exists for a caller that has already fetched
    /// the items, and it still had to fetch the folders itself, because until
    /// now there was no way to hand both halves in. A caller that holds both
    /// -- the encrypted disk cache restoring a snapshot from disk
    /// (`docs/superpowers/plans/2026-07-31-encrypted-vault-disk-cache.md`),
    /// and every test fixture that wants a populated cache -- had no door,
    /// and stood up an HTTP server for a fetch whose answer it already knew.
    ///
    /// **It is not a second way to write a snapshot.** It is *the* way; the
    /// fetching entry points are wrappers that decide what to hand it. Every
    /// rule [`Self::populate_with`] and [`Self::populate`] document holds
    /// here unchanged, because below this line it is the same code:
    ///
    ///  * the era guard is applied exactly once, in
    ///    [`Self::write_back_at_epoch`], so a [`Self::clear`] since `epoch`
    ///    was captured discards this populate as
    ///    [`PopulateOutcome::DiscardedStale`] and leaves the snapshot empty
    ///    and unpopulated;
    ///  * local writes newer than `epoch` are replayed over the given data
    ///    rather than clobbered by it;
    ///  * nothing here begins an era. [`Self::clear`] remains the only thing
    ///    that does -- see [`VaultEra`].
    ///
    /// **The one rule for callers is the same one, and is likewise not
    /// enforced by the type**: capture `epoch` from [`Self::epoch`] BEFORE
    /// whatever produced `vault`. For a disk read that means before the read;
    /// for a fixture building a literal it is simply `cache.epoch()`, which
    /// cannot be late because nothing can have happened in between.
    ///
    /// It returns a bare [`PopulateOutcome`] rather than a `Result` because
    /// with nothing fetched there is nothing left that can fail: the only two
    /// answers are "written" and "discarded as stale".
    pub fn populate_with_vault(&self, vault: VaultSnapshot, epoch: VaultEpoch) -> PopulateOutcome {
        self.write_back_at_epoch(vault.items, vault.folders, epoch, Source::Backend)
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
    ///
    /// **Where the fetch is, now that this function does not do one.** The
    /// `list_folders` this doc keeps referring to moved one level up, into
    /// [`Self::populate_with_at_epoch`], so that
    /// [`Self::populate_with_vault`] could reach the write-back without it.
    /// Nothing above changes: for a *fetching* populate the window described
    /// here is exactly as wide as it ever was, because the caller's own
    /// `list_items` and the folder fetch both still happen before this lock.
    /// For a populate that fetches nothing the window is narrower -- only the
    /// caller's own read of wherever the vault came from -- and the guard and
    /// the replay are unchanged, which is the point of there being one
    /// write-back rather than two.
    fn write_back_at_epoch(
        &self,
        mut items: Vec<VaultItem>,
        mut folders: Vec<Folder>,
        epoch: VaultEpoch,
        source: Source,
    ) -> PopulateOutcome {
        let mut snapshot = self.lock();
        // Allocated under the lock, before anything can return: it is what
        // orders this populate against every other one in the log, and both
        // lines below are emitted after the guard is dropped (review 26's
        // Minor 1).
        let populate_seq = snapshot.next_populate_seq();
        if snapshot.epoch().era() != epoch.era() {
            let now = snapshot.epoch().era();
            // Logged after the guard is dropped, for the reason given at the
            // replay log line below: `log::info!` is file I/O.
            drop(snapshot);
            log::info!(
                "discarding vault populate #{} that finished after the cache was cleared \
                 (era {} -> {}); the snapshot stays empty",
                populate_seq,
                epoch.era(),
                now
            );
            return PopulateOutcome::DiscardedStale;
        }

        let replayed_items = replay_writes(
            &mut items,
            &snapshot.items,
            &snapshot.pending_items,
            epoch.writes,
            |i| &i.id,
        );
        let replayed_folders = replay_writes(
            &mut folders,
            &snapshot.folders,
            &snapshot.pending_folders,
            epoch.writes,
            |f| &f.id,
        );

        snapshot.items = items;
        snapshot.folders = folders;
        snapshot.populated = true;
        // NOTHING IS PRUNED HERE, and that is review 23's Critical. It used to
        // `retain(|w| w.seq > epoch.writes)` -- "drop what MY fetch covered" --
        // which is not a fact about anybody else. There is more than one
        // populate producer and none is gated on the others (see the ledger),
        // so an OLDER in-flight populate has a SMALLER mark and needs a
        // SUPERSET of these entries; everything in `(older_mark, my_mark]` was
        // destroyed, and the older populate then wrote its pre-write fetch
        // back over the write with nothing left to replay. The prune was a
        // pure optimisation and no assertion depended on it.
        //
        // WHAT BOUNDS THE LOGS NOW, since it is no longer this: `record_write`
        // is last-write-wins per id, so there is at most one entry per DISTINCT
        // id written since the last `clear`, and an entry is a `u64` and an id
        // -- never a copy of an item. `clear` empties both logs outright. So
        // the logs are bounded by the number of distinct ids a user writes in
        // one vault session, and the replay itself stays correct however long
        // they live: `seq > since` retires every entry a given populate's own
        // fetch covered *for that populate*, without deleting it for anyone
        // else.
        drop(snapshot);

        // OUTSIDE THE LOCK, deliberately -- review 25's Minor 4. Building
        // this line clones every replayed id and joins them into one string,
        // and `log::info!` then does file I/O; none of that is work the
        // snapshot has to be held for, and every read in the app (autofill
        // included) blocks behind this mutex. It used to be a single `usize`,
        // which is how it went unnoticed; with the prune gone the replay set
        // is bounded only by the distinct ids written since the last `clear`.
        //
        // WHICH IS WHY IT CARRIES `populate_seq` (review 26's Minor 1):
        // moving the line out of the critical section left two concurrent
        // populates able to log in the opposite order to the one they took
        // the lock in, and this line's whole purpose is the post-mortem of a
        // lost write, where that order is the question. See `replay_log_line`.
        if !replayed_items.is_empty() || !replayed_folders.is_empty() {
            log::info!(
                "{}",
                replay_log_line(populate_seq, &replayed_items, &replayed_folders)
            );
        }

        // AFTER the guard is dropped, and only on the branch that actually
        // adopted the snapshot. Persisting a DISCARDED populate would write
        // the previous account's vault to disk -- the same bug the era guard
        // exists to stop, one layer down -- and clearing the from-disk age
        // there would tell the pill a refresh had landed when the whole
        // point of that branch is that none did.
        match source {
            Source::Backend => {
                self.lock_disk().loaded_from_disk_at = None;
                self.persist();
            }
            Source::DiskCache(written_at) => {
                self.lock_disk().loaded_from_disk_at = Some(written_at);
            }
        }
        PopulateOutcome::Populated
    }

    pub fn is_populated(&self) -> bool {
        self.lock().populated
    }

    pub fn items(&self) -> Vec<VaultItem> {
        self.lock().items.clone()
    }

    /// **Every item, as whatever the caller needs and nothing more.**
    ///
    /// The projection runs against a borrowed item under the lock and the
    /// item is never cloned, so a caller that wants three strings gets three
    /// strings -- not a deep copy of a vault it then throws most of away.
    ///
    /// # Why this exists rather than another `items()` caller
    ///
    /// [`Self::items`] hands out `VaultItem`s, and a `VaultItem` carries a
    /// [`crate::vault_bridge::LoginData::password`]. Every caller of it in the
    /// daemon therefore holds every password in the vault, in plaintext, for
    /// as long as it holds the `Vec` -- and the account picker's caller held
    /// one for the life of a card. The picker needs an id, a name, a username
    /// and a list of URIs; it has never needed a secret.
    ///
    /// A combinator rather than a fixed `facts()` returning a fixed struct,
    /// because the two callers want different shapes and a struct wide enough
    /// for both is a struct with fields neither needs. It also keeps this
    /// module out of the business of knowing what a palette or an icon domain
    /// is: those live in `app` and `key_sequence`, and the closure is where
    /// they stay.
    ///
    /// **The lock is held for the whole map.** The closure must not call back
    /// into the cache -- that is a deadlock, not a slow path -- and every
    /// caller here passes a pure function of one item.
    pub fn project<T>(&self, of: impl Fn(&VaultItem) -> T) -> Vec<T> {
        self.lock().items.iter().map(of).collect()
    }

    /// One item, cloned; the rest of the snapshot is not.
    ///
    /// **This exists for the autofill path's latency, not for tidiness.**
    /// `items()` deep-clones the whole vault -- measured at 5.66 MB and
    /// 46,494 allocations, 5.6-9.4 ms, over a realistic 1,663-item vault --
    /// and the two fill-path call sites in `app.rs` paid all of that to
    /// answer "which item has this id". That cost sat between the keypress
    /// and the password appearing. `main.rs`'s breach scan genuinely wants
    /// every item and still calls `items()`.
    ///
    /// **`None` is a cache miss, and the caller must still handle it.**
    /// `fill_from_vault_with` falls through to `bridge().get_item` and logs
    /// when it does; that warning is deliberate, because a miss here is a bug
    /// signal worth noticing rather than silently swallowing. This function
    /// answers only from the snapshot -- reading the snapshot rather than
    /// `bw serve` is the path that makes autofill work with the backend fully
    /// stopped -- so it never reaches the bridge itself.
    ///
    /// **Lock discipline.** The guard is a temporary of this one expression
    /// and is dropped at the end of it. Everything done while it is held is a
    /// field comparison and a `VaultItem::clone`: no callback, no closure
    /// supplied by the caller, no bridge call, no `persist`, nothing that can
    /// re-enter this cache. The returned item is owned, so the caller holds no
    /// borrow of the snapshot either.
    pub fn get_by_id(&self, id: &str) -> Option<VaultItem> {
        self.lock().items.iter().find(|i| i.id == id).cloned()
    }

    pub fn folders(&self) -> Vec<Folder> {
        self.lock().folders.clone()
    }

    /// Drops everything. Called on lock and on quit.
    ///
    /// **Does not touch the encrypted disk file, deliberately.** Surviving a
    /// lock and a restart is the entire reason that file exists, and quit
    /// calls this too, so a `clear` that deleted it would leave the feature
    /// with nothing to do. Deleting it is [`Self::forget_disk_copy`], which
    /// the re-authentication and log-out paths call explicitly.
    ///
    /// The from-disk *age* does go, because that describes the snapshot this
    /// call is dropping and not the file: an age left behind would have the
    /// toolbar pill dating an empty cache.
    ///
    /// Also begins a new era, which is what makes this actually stick against
    /// a populate that is already in flight -- see [`Self::populate`]. It is
    /// the ONLY thing that begins one, deliberately: see [`VaultEra`] for why
    /// the writes below must not, and [`Self::snapshot_unless_superseded`] for
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
        drop(snapshot);
        self.lock_disk().loaded_from_disk_at = None;
        // **And the file this process could read items out of.**
        //
        // The encrypted copy is meant to survive a lock -- it exists so the
        // next launch does not pay a cold start -- but the CONTENT KEY must
        // not. Version 2 lets the daemon open one item at a time out of that
        // file; a lock that emptied the snapshot and left the key in place
        // would be a vault the daemon could refill without the user, which
        // is the appearance of locking rather than locking. Hello is asked
        // again on the next unlock.
        if let Some(disk) = self.disk.as_ref() {
            disk.close();
        }
    }

    /// One item out of the encrypted disk copy, without disturbing the
    /// snapshot.
    ///
    /// **The point of the version 2 file**, seen from this side: a caller
    /// that needs one secret -- a fill, about to type a password -- gets that
    /// one, rather than the daemon holding every password so that any of them
    /// is available.
    ///
    /// `None` when there is no disk cache, when it is switched off, when the
    /// file was never opened this session, or when the id is not in it. Every
    /// one of those means the same thing to a caller: ask the backend.
    // **`item_from_disk` was removed here**, and the reason is the rule
    // rather than tidiness. It was a direct read of the cache file from the
    // type consumers hold -- a second way to reach the vault, which
    // `docs/superpowers/specs/2026-08-27-one-door-to-the-vault.md` forbids.
    //
    // A consumer asks its `VaultBackend`; `vault_backend::CachingBackend` is
    // what consults the file, and
    // `nothing_outside_the_vault_service_opens_an_item_from_the_file` is what
    // keeps it that way. The two tests that lived here -- that a closed file
    // answers nothing, and that an open one answers -- moved with the
    // behaviour, to `caching_backend_tests`.

    pub fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.create_item_writing(new_item))
    }

    /// [`Self::create_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn create_item_writing(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        let created = self.bridge.create_item(new_item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.push(created.clone());
            snapshot.note_item_write(&created.id, false);
        }
        Ok(created)
    }

    /// **Returns the item as the SERVER answered, not `Ok(())`**, for the
    /// reason [`Self::set_favorite`] does and one more: the vault window keeps
    /// its own `Vec` and used to reinstate `updated` -- the value it SENT --
    /// after a save. That value carries a `revisionDate` the write has already
    /// superseded, so the next write of that item is refused with a 400. See
    /// `vault_bridge`'s `REVISION_DATE_KEY`.
    pub fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.update_item_writing(item))
    }

    /// [`Self::update_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn update_item_writing(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        let saved = self.bridge.update_item(item)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            // By index rather than `iter_mut().find()`: the write has to be
            // recorded on the same snapshot, and an outstanding `&mut` into
            // its items would still be alive inside an `if let`.
            if let Some(at) = snapshot.items.iter().position(|i| i.id == item.id) {
                snapshot.items[at] = saved.clone();
                // Recorded only when the snapshot actually changed, so the
                // log can never name an id the snapshot cannot supply at
                // replay time -- see `replay_writes`.
                snapshot.note_item_write(&item.id, false);
            }
        }
        Ok(saved)
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
    /// **The write-through can miss, and it is REPORTED rather than swallowed**
    /// -- review 26's Minor 2. This used to answer `Ok(())` whether or not the
    /// snapshot took the match. The consequence is the same for either miss:
    /// `main` rebuilds the engine from a same-era populated snapshot (its
    /// `else` warn does NOT fire) and arms it without the match, while the
    /// user has been told the save succeeded. It had -- server-side -- which
    /// is why this is an [`AppMatchWrite`] and not a [`VaultError`].
    ///
    /// The unpopulated miss needs only a `clear` with nothing filled after it.
    /// What reaches the `position` miss is narrower than this doc used to
    /// claim, and the difference matters because the two misses have different
    /// cures -- see [`AppMatchWrite::ServerOnly`], which works it out in full
    /// (review 28's Important 2).
    ///
    /// **The other writes on this type are NOT all the same case**, and the
    /// difference is in what the miss costs, not in its shape:
    ///
    ///  * `create_item`/`create_folder` push unconditionally and
    ///    `delete_item`/`delete_folder` record unconditionally, so neither can
    ///    miss an id at all. Only the unpopulated skip applies to them, and an
    ///    unpopulated cache has nothing for them to be missing FROM.
    ///  * [`Self::update_item`] and [`Self::update_folder`] have exactly this
    ///    `position` miss and exactly this silence. They are deliberately left
    ///    alone here, for a reason and not for tidiness -- but NOT the reason
    ///    recorded in review 26, which was false (review 28's Minor 2). That
    ///    reason was "an id absent from the snapshot is also absent from the
    ///    vault window's list, so there is no follow-on edit to PUT a stale
    ///    copy back". The window's list is a LOCAL `Vec` loaded once when the
    ///    window opens and mutated in place on every save
    ///    (`items[pos] = updated`), whether or not the write-through landed,
    ///    so snapshot absence says nothing whatever about list presence.
    ///
    ///    The real reason is stronger, and it is the same fact read the other
    ///    way round: the BASE for a subsequent edit is also that local vec, so
    ///    it already carries the prior edit. A second edit therefore PUTs the
    ///    prior edit plus the new one, which is correct -- there is no stale
    ///    copy anywhere on the path, and the missed write-through costs only
    ///    an item the window's own list is right about and the cache is
    ///    behind on until the next populate. Nothing arms itself from that.
    ///    `set_app_match` is different because the *point* of the write is a
    ///    match engine rebuilt from the snapshot moments later, so a silent
    ///    miss there is a feature that visibly does nothing. Both of their
    ///    call sites live in `vault_window/`, which neither pass owned;
    ///    changing their return types stays a recorded follow-up.
    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<AppMatchWrite, VaultError> {
        self.persisting(|| self.set_app_match_writing(item, m))
    }

    /// [`Self::set_app_match`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn set_app_match_writing(&self, item: &VaultItem, m: &AppMatch) -> Result<AppMatchWrite, VaultError> {
        // The SERVER's copy, not `with_app_match(item, m)`. The two agree on
        // every field the app cares about, and differ on the one that decides
        // whether the NEXT write of this item is accepted at all -- see
        // `vault_bridge`'s `REVISION_DATE_KEY`.
        let saved = self.bridge.set_app_match(item, m)?;
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "saved an app match onto vault item {} but the cache holds no snapshot to write \
                 it through to; the vault has it and any populate will bring it in",
                item.id
            );
            return Ok(AppMatchWrite::ServerOnly);
        }
        // The two misses are one variant on purpose -- see `AppMatchWrite`,
        // which states the ground: the same IMMEDIATE caller action, NOT the
        // same remedy. Review 28's Important 2 found the remedy is not in
        // fact shared, and that the variant had been justified on it.
        // The distinction that IS genuinely useful between them is
        // diagnostic, so it lives in these two log lines, not in the type.
        match snapshot.items.iter().position(|i| i.id == item.id) {
            Some(at) => {
                snapshot.items[at] = saved;
                snapshot.note_item_write(&item.id, false);
                Ok(AppMatchWrite::WroteThrough)
            }
            None => {
                drop(snapshot);
                log::warn!(
                    "saved an app match onto vault item {} but the snapshot no longer holds that \
                     id -- a populate's fetch dropped it, which needs the item to have stopped \
                     existing AFTER the server accepted this write, so a later sync will NOT \
                     bring the match back. Nothing rebuilt from the cache alone will have it",
                    item.id
                );
                Ok(AppMatchWrite::ServerOnly)
            }
        }
    }

    /// Files `item` under `folder_id`, or un-files it when that is `None`,
    /// via [`VaultBridge::move_item_to_folder`], and moves the snapshot's copy
    /// with it.
    ///
    /// **This is the only way a move may be made.** Reaching
    /// [`Self::bridge`]`.move_item_to_folder` directly would leave the
    /// snapshot filing the item where it used to be, and the snapshot is what
    /// the vault window reads when it opens and when it re-reads without a
    /// forced refresh -- so the move would appear to undo itself.
    ///
    /// **Returns `Result<(), VaultError>` -- deliberately NOT an
    /// [`AppMatchWrite`]-style outcome, and this is the judgement most worth
    /// challenging in this change.** The UX the user chose is: a failed move
    /// reverts the dragged row and shows an inline error. That is `Err`, which
    /// this type gives. The question is whether the SUCCESS case needs
    /// splitting the way `set_app_match`'s does, and both of that split's
    /// misses were traced here rather than assumed:
    ///
    ///  * **Cache unpopulated.** The write-through is skipped, but the next
    ///    populate fetches from the server, which HAS the move. Self-curing.
    ///  * **Populated, id absent.** For a populate's fetch to lack the id, the
    ///    item must have stopped existing after the server accepted this write
    ///    (see [`AppMatchWrite::ServerOnly`], which works the window out in
    ///    full). The item is gone, so there is no row to show in either
    ///    folder, and no notice about folders would be true.
    ///
    /// Neither leaves a UI that claims a move the vault does not have, and
    /// neither is cured by anything the caller could do differently -- so the
    /// two cases demand IDENTICAL caller behaviour, which is exactly the test
    /// [`AppMatchWrite`]'s own doc sets for whether a distinction earns a type
    /// ("PopulateOutcome's split exists because ITS two cases demand opposite
    /// caller behaviour"). What `set_app_match` has that this does not is a
    /// consumer that ARMS from the snapshot moments later, making a silent
    /// miss a feature that visibly does nothing; folder assignment is not read
    /// by the match engine at all. So the distinction lives in the two
    /// `log::warn!` lines below, as that doc prescribes.
    ///
    /// If the UI half finds a consumer this trace missed -- an inline "this
    /// window's copy is behind" notice, say -- the change is an enum and one
    /// `match` at one call site.
    pub fn move_item_to_folder(&self, item: &VaultItem, folder_id: Option<&str>) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.move_item_to_folder_writing(item, folder_id))
    }

    /// [`Self::move_item_to_folder`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn move_item_to_folder_writing(&self, item: &VaultItem, folder_id: Option<&str>) -> Result<VaultItem, VaultError> {
        // Bridge call BEFORE `self.lock()`, like every other write here: no
        // lock may be held across HTTP.
        //
        // The SERVER's copy, not `with_folder(item, folder_id)`: the locally
        // rebuilt value carries the `revisionDate` this very write has just
        // superseded, and the next write of the item would be refused for it
        // -- see `vault_bridge`'s `REVISION_DATE_KEY`.
        let moved = self.bridge.move_item_to_folder(item, folder_id)?;
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "moved vault item {} to folder {:?} but the cache holds no snapshot to write it \
                 through to; the vault has it and any populate will bring it in",
                item.id,
                folder_id
            );
            return Ok(moved);
        }
        match snapshot.items.iter().position(|i| i.id == item.id) {
            Some(at) => {
                snapshot.items[at] = moved.clone();
                // Recorded only when the snapshot actually changed, so the
                // replay log can never name an id the snapshot cannot supply
                // -- see `replay_writes`. This is what stops a populate that
                // was already in flight from filing the item back where it
                // was.
                snapshot.note_item_write(&item.id, false);
            }
            None => {
                drop(snapshot);
                log::warn!(
                    "moved vault item {} to folder {:?} but the snapshot no longer holds that id \
                     -- a populate's fetch dropped it, which needs the item to have stopped \
                     existing AFTER the server accepted this move, so there is no row left to \
                     file anywhere",
                    item.id,
                    folder_id
                );
            }
        }
        Ok(moved)
    }

    /// Marks `item` as a favourite, or clears the mark, and writes the change
    /// through to the snapshot.
    ///
    /// **Returns the item as it was written**, rather than `Ok(())`. That is
    /// the point of the signature: the vault window keeps its own local `Vec`
    /// of items and paints the detail pane from it, so on success it needs
    /// the exact value that went to the server -- not a locally toggled guess
    /// that could diverge from it. And because the return is a `Result`, the
    /// only way to obtain the new item is for the write to have succeeded:
    /// a UI cannot flip its star and *then* discover the PUT failed, because
    /// it has nothing to flip to until the PUT has landed. A failed write
    /// leaves the caller holding its original item and an error to show.
    ///
    /// Bridge call BEFORE `self.lock()`, like every other write on this type:
    /// no lock may be held across HTTP.
    ///
    /// A separate method rather than "build it with [`with_favorite`] and
    /// call [`Self::update_item`]" for the same reason
    /// [`Self::move_item_to_folder`] is separate: one door per operation, so
    /// the rule about what gets written lives in one place. Unlike the folder
    /// move there is no wire trap to dodge here --
    /// [`crate::vault_bridge::VaultItem::favorite`] is stated on every write
    /// (see [`with_favorite`]) -- so this one really is `update_item`'s body
    /// with a named front door, and it is the front door that is worth
    /// having.
    pub fn set_favorite(&self, item: &VaultItem, favorite: bool) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.set_favorite_writing(item, favorite))
    }

    /// [`Self::set_favorite`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn set_favorite_writing(&self, item: &VaultItem, favorite: bool) -> Result<VaultItem, VaultError> {
        // The SERVER's answer, not the locally built `with_favorite(..)`
        // value that was sent. They agree on `favorite`; they differ on
        // `revisionDate`, which the write has just bumped and which the NEXT
        // write of this item must carry or be refused with a 400. Toggling a
        // star is the operation a user repeats on ONE item, so it is the one
        // that met that refusal first -- see `vault_bridge`'s
        // `REVISION_DATE_KEY`.
        let updated = self.bridge.update_item(&with_favorite(item, favorite))?;
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "set favourite={} on vault item {} but the cache holds no snapshot to write it \
                 through to; the vault has it and any populate will bring it in",
                favorite,
                updated.id
            );
            return Ok(updated);
        }
        match snapshot.items.iter().position(|i| i.id == updated.id) {
            Some(at) => {
                snapshot.items[at] = updated.clone();
                // Recorded only when the snapshot actually changed, so the
                // replay log can never name an id the snapshot cannot supply
                // -- see `replay_writes`. This is what stops an in-flight
                // populate from putting the old flag back.
                snapshot.note_item_write(&updated.id, false);
            }
            None => {
                drop(snapshot);
                log::warn!(
                    "set favourite={} on vault item {} but the snapshot no longer holds that id \
                     -- a populate's fetch dropped it, which needs the item to have stopped \
                     existing AFTER the server accepted this write",
                    favorite,
                    updated.id
                );
            }
        }
        Ok(updated)
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.persisting(|| self.delete_item_writing(id))
    }

    /// [`Self::delete_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn delete_item_writing(&self, id: &str) -> Result<(), VaultError> {
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

    /// The vault's trash in one call, **without the era guard** -- reachable
    /// from tests only.
    ///
    /// `#[cfg(test)]` and private, and the gating is the point rather than
    /// tidying. Every production path goes through
    /// [`Self::list_trash_unless_superseded`], which throws away a result that
    /// landed after a [`Self::clear`] began a new [`VaultEra`]. This function
    /// was `pub` and sat immediately beside the guarded one with nothing
    /// marking it, which is exactly how the next call site quietly reacquires
    /// the hole `ca67475` closed -- account B's trashed item names under
    /// account A's chrome -- by reaching for the shorter name. Under the cfg,
    /// a production caller is a compile error rather than a review finding.
    ///
    /// The tests keep it because what they assert is the QUERY the bridge puts
    /// on the wire (`?trash=true`, and the archive filter's spelling) and the
    /// error mapping around it. The era guard is irrelevant to both, and
    /// [`Self::list_trash_unless_superseded`] would only wrap the answer in an
    /// `Option` those assertions would have to unwrap again.
    ///
    /// See [`Self::list_trash_unless_superseded`] for why neither list is
    /// cached at all.
    #[cfg(test)]
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_trash()
    }

    /// [`Self::list_trash`] for the archive, test-only for its reasons.
    #[cfg(test)]
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_archive()
    }

    /// The vault's trash, fetched fresh every time, and **discarded if a
    /// [`Self::clear`] began a new [`VaultEra`] while the fetch was in
    /// flight** -- `Ok(None)`. The one door production has onto the trash.
    ///
    /// # Why the trash is not cached
    ///
    /// **TRASH IS DELIBERATELY NOT IN THE SNAPSHOT**, and this is the design
    /// decision of the Trash backend. The snapshot is `items` + `folders`
    /// wrapped in the era/epoch machinery, the pending-write log and
    /// [`Self::snapshot_unless_superseded`] -- a structure that exists because
    /// fifteen consecutive review findings came out of concurrent reads of it.
    /// Putting a second collection inside it makes every one of those findings
    /// reachable again for that collection (a replay that must know which of
    /// two lists an id belongs to; a `VaultSnapshot` whose shape changes under
    /// every existing consumer; a restore that has to move an id BETWEEN two
    /// era-guarded lists atomically), and it buys a cache for a list the user
    /// measured at seven items.
    ///
    /// The argument is not just "small, so who cares" -- **not caching it is
    /// what makes two of this feature's three correctness requirements
    /// unfalsifiable rather than tested**:
    ///
    ///  * after a purge the trash list must not contain the item. Here that
    ///    holds because there is no trash list to be stale: the next call asks
    ///    the server, which has already purged it. A cached trash list would
    ///    have to be pruned on purge, and that pruning is a thing that can be
    ///    forgotten.
    ///  * likewise after a restore.
    ///
    /// What it costs, stated rather than glossed: one HTTP round-trip when the
    /// Trash row is opened (and one per refresh), and **a sidebar badge cannot
    /// count the trash without paying that round-trip**. A badge that must be
    /// live at all times is the one requirement that would change this answer;
    /// nothing in the backend brief asks for one.
    ///
    /// It routes through the cache anyway, rather than callers reaching
    /// [`Self::bridge`], so that there is one door for the whole feature and
    /// so a later decision to cache it changes this function and nothing else.
    ///
    /// # Why the era guard
    ///
    /// This is the on-demand lists' half of the guarantee
    /// [`Self::snapshot_unless_superseded`] gives the live load, and it exists
    /// because the on-demand fetch had no equivalent at all. Its only guard
    /// was the vault window's `load_generation`, which is a SPAWN TAG: it is
    /// incremented when a vault load is spawned, and `clear` does not touch
    /// it. So a Trash fetch outstanding across a `clear` and a re-populate
    /// under a different account came back carrying the same generation it
    /// left with, matched, and was applied -- account B's trashed item names
    /// under account A's chrome.
    ///
    /// No reachable path produces that today: both the lock and the re-auth
    /// that would `clear` the cache also close the window, so the drain is
    /// gone before the result lands. **This is defence in depth, and it is
    /// worth the six lines for the reason [`VaultEra`]'s own doc gives** --
    /// the era machinery was introduced specifically so this window would
    /// stop resting on "every production `clear` happens to run on the main
    /// thread", and an unguarded fetch beside it quietly restores that
    /// reliance for one more path.
    ///
    /// The era is checked **after** the fetch, never before. Before, it would
    /// answer "is this still the session that existed one instruction ago?" --
    /// yes, by construction -- which is the same non-question
    /// `window_era_placement_tests` exists to keep out of the load spawns. The
    /// only useful moment to ask is once the slow part is over.
    ///
    /// A superseded fetch that also FAILED reports `Ok(None)` rather than its
    /// error, deliberately: the error describes a vault this window has
    /// already left, and surfacing it would put "Trash could not be read" in
    /// front of a user whose session was merely replaced.
    pub fn list_trash_unless_superseded(
        &self,
        era: VaultEra,
    ) -> Result<Option<Vec<VaultItem>>, VaultError> {
        self.list_unless_superseded(era, |bridge| bridge.list_trash())
    }

    /// The vault's archive under [`Self::list_trash_unless_superseded`]'s
    /// guard, for its reasons and with its caching decision.
    pub fn list_archive_unless_superseded(
        &self,
        era: VaultEra,
    ) -> Result<Option<Vec<VaultItem>>, VaultError> {
        self.list_unless_superseded(era, |bridge| bridge.list_archive())
    }

    /// The one copy of the check above, so the two lists cannot come to
    /// disagree about when a result is still current.
    fn list_unless_superseded(
        &self,
        era: VaultEra,
        fetch: impl FnOnce(&dyn VaultBackend) -> Result<Vec<VaultItem>, VaultError>,
    ) -> Result<Option<Vec<VaultItem>>, VaultError> {
        let fetched = fetch(self.bridge());
        if self.epoch().era() != era {
            return Ok(None);
        }
        fetched.map(Some)
    }

    /// Puts `item` into the archive and takes it out of the live snapshot.
    ///
    /// The snapshot side is [`Self::delete_item`]'s, and for the same reason:
    /// an archived item is gone from `GET /list/object/items` (measured), so
    /// as far as every consumer of this snapshot is concerned -- the item
    /// list, the match engine, autofill -- archiving is a removal. Recording
    /// it in the pending-write log is what stops a populate that was already
    /// in flight from putting it straight back.
    ///
    /// **This does NOT read a list back to confirm the archive**, and that is
    /// deliberate rather than an omission. A 200 from `/archive/item/{id}`
    /// does not prove the state changed: an item archived immediately after
    /// creation answered 200 and stayed in the default list until a ~1.5s
    /// settle had passed (`.superpowers/sdd/item-shapes-capture.md`). A read
    /// taken here would race that settle and report a failure that did not
    /// happen -- worse than the write it was meant to police, because the
    /// caller would then undo a correct archive. The next ordinary refresh
    /// reconciles instead.
    pub fn archive_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        self.persisting(|| self.archive_item_writing(item))
    }

    /// [`Self::archive_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn archive_item_writing(&self, item: &VaultItem) -> Result<(), VaultError> {
        // Bridge call BEFORE `self.lock()`: no lock may be held across HTTP.
        self.bridge.archive_item(&item.id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.retain(|i| i.id != item.id);
            // Recorded even when the snapshot did not hold the item, exactly
            // as `delete_item` and `purge_item` do: what it has to survive is
            // a fetch that predates the archive and DOES hold it.
            snapshot.note_item_write(&item.id, true);
        }
        Ok(())
    }

    /// Takes `item` out of the archive and puts it back in the live snapshot.
    ///
    /// The snapshot side is [`Self::restore_item`]'s exactly -- the item
    /// reappears in `GET /list/object/items`, and the `deleted: true` entry
    /// [`Self::archive_item`] left in the pending-write log has to be
    /// overwritten or `replay_writes` strips the id out of every later fetch
    /// for the rest of the session.
    ///
    /// [`without_deleted_date`] is NOT applied here, and that is the one
    /// difference from `restore_item`: an archived item never carried a
    /// `deletedDate` in the first place (its keys are an ordinary item's --
    /// measured), so removing one would be removing a key that is not there.
    /// An item that is somehow BOTH is not a state this backend has been
    /// observed to produce, and inventing handling for it would be modelling
    /// from memory.
    ///
    /// **Returns the item as it was stored, not `Ok(())`**, for
    /// [`Self::set_favorite`]'s reason exactly: the vault window keeps its own
    /// `Vec`, and pushing the caller's copy into it after this call is what
    /// put a superseded revision token back in front of the next write. See
    /// [`Self::current_revision_of`].
    pub fn unarchive_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.unarchive_item_writing(item))
    }

    /// [`Self::unarchive_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn unarchive_item_writing(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.bridge.unarchive_item(&item.id)?;
        // Both bridge calls BEFORE `self.lock()`: no lock may be held across
        // HTTP, and this one is HTTP too.
        let unarchived = self.current_revision_of(item);
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "took vault item {} out of the archive but the cache holds no snapshot to write \
                 it through to; the vault has it live and any populate will bring it in",
                item.id
            );
            return Ok(unarchived);
        }
        match snapshot.items.iter().position(|i| i.id == item.id) {
            Some(at) => snapshot.items[at] = unarchived.clone(),
            None => snapshot.items.push(unarchived.clone()),
        }
        snapshot.note_item_write(&item.id, false);
        Ok(unarchived)
    }

    /// `item` with the revision token the server currently reports for it, or
    /// `item` unchanged when that read fails.
    ///
    /// **The third stale-token path.** `fba91ff` fixed the two writes that
    /// answer with the server's copy ([`Self::update_item`],
    /// [`Self::move_item_to_folder`], and [`Self::set_favorite`] through the
    /// first) by adopting that copy. `POST /restore/item/{id}` -- the one
    /// route behind both [`Self::restore_item`] and [`Self::unarchive_item`]
    /// -- answers with a body this crate has never verified the shape of, so
    /// those two stored the CALLER's copy instead, token and all. Its token is
    /// the one the item had before the restore. If this backend bumps
    /// `revisionDate` on a restore, the very next write of that item is
    /// refused with the 400 in `vault_bridge`'s `REVISION_DATE_KEY` -- and the
    /// user's report that started all of this was a favourite that "shows as
    /// faved in folder but not in original client", which is that 400 exactly.
    ///
    /// **Whether this backend bumps the token on a restore is NOT verified.**
    /// `bw serve` was not running when this was written, so the code fact (the
    /// caller's token was kept) is what was established and the consequence is
    /// inferred. That is why this reads the token rather than assuming either
    /// answer: if the restore does not bump, the read returns the same string
    /// and this is a no-op; if it does, the stale one never reaches the
    /// snapshot. The cost is one `GET /object/item/{id}` per restore, on a
    /// gesture a user makes by hand.
    ///
    /// **A failed read is not a failed restore.** The write already succeeded,
    /// so this cannot turn it into an error; it falls back to the caller's
    /// copy, which is precisely today's behaviour, and says so in the log.
    fn current_revision_of(&self, item: &VaultItem) -> VaultItem {
        match self.bridge.get_item(&item.id) {
            Ok(server) => crate::vault_bridge::with_revision_date_from(item, &server),
            Err(e) => {
                log::warn!(
                    "could not read vault item {} back after taking it out of the trash or the \
                     archive, so this app is holding the revision token the item had BEFORE that \
                     write ({e:?}); if the backend advanced it, the next edit of this item will \
                     be refused until the next full refresh",
                    item.id
                );
                item.clone()
            }
        }
    }

    /// Takes `item` out of the trash and puts it back in the live snapshot.
    ///
    /// Takes the whole trashed item, not just its id, for the reason
    /// [`Self::move_item_to_folder`] does: the snapshot then holds what the
    /// caller had rather than a reconstruction, and `POST /restore/item/{id}`
    /// returns nothing this crate has verified the shape of.
    ///
    /// **[`without_deleted_date`] is load-bearing, not tidying.** The item the
    /// caller holds came from [`Self::list_trash_unless_superseded`] and carries
    /// `deletedDate`
    /// in [`VaultItem::other`], which is serialized on every write this app
    /// makes -- so pushing it in verbatim would leave the live snapshot's copy
    /// claiming a deletion date, and the vault window's next ordinary edit of
    /// that item would PUT the key back at a backend whose handling of it is
    /// unverified.
    ///
    /// **`note_item_write` is the one thing here that can be wrong**, and it
    /// is unconditional rather than recorded only on the `Some` arm, because
    /// unlike every other write on this type the restore is *undoing a
    /// recorded delete*. `record_write` is last-write-wins per id
    /// (see `record_write`), so the entry this call replaces is the
    /// `deleted: true` one that [`Self::delete_item`] left when the item was
    /// trashed in the first place. Without this line that entry survives, and
    /// `replay_writes` then strips the restored id out of **every** subsequent
    /// fetch until the next [`Self::clear`] -- the item comes back on the
    /// server and stays invisible in this process for the rest of the session.
    /// `a_restore_overrides_the_pending_delete_that_trashed_the_item` walks
    /// exactly that, deterministically.
    ///
    /// **Returns the item as it was stored**, carrying the revision token the
    /// server reports NOW rather than the one the trashed copy arrived with --
    /// see [`Self::current_revision_of`], and hand it to whatever local list
    /// the caller keeps.
    ///
    /// Returns no [`AppMatchWrite`]-style miss/hit outcome, on the same test
    /// that doc sets (the two misses must demand identical caller behaviour):
    ///
    ///  * **Cache unpopulated.** Write-through skipped, and self-curing: any
    ///    populate fetches from a server that has the item live. Note that the
    ///    stale-pending-entry hazard above cannot apply in this case --
    ///    `clear` is the only thing that unpopulates, and it empties both
    ///    pending logs.
    ///  * **Populated.** Cannot miss: the `None` arm pushes, so the id is
    ///    present either way and the write is always recorded.
    ///
    /// Nothing arms itself from an item's trashed-ness, so neither case leaves
    /// a consumer acting on a claim the vault does not support.
    pub fn restore_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.persisting(|| self.restore_item_writing(item))
    }

    /// [`Self::restore_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn restore_item_writing(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        // Bridge call BEFORE `self.lock()`, like every other write here: no
        // lock may be held across HTTP -- and `current_revision_of` is a
        // second HTTP call, so it goes here too and not below the lock.
        self.bridge.restore_item(&item.id)?;
        let restored = self.current_revision_of(&without_deleted_date(item));
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "restored vault item {} from the trash but the cache holds no snapshot to write \
                 it through to; the vault has it live and any populate will bring it in",
                item.id
            );
            return Ok(restored);
        }
        match snapshot.items.iter().position(|i| i.id == item.id) {
            // A restored item is by definition absent from the live snapshot,
            // so `push` is the ordinary arm and `Some` is the unusual one --
            // reachable when an older populate's fetch still carried the item
            // as live. Replacing rather than pushing there is what keeps the
            // snapshot from holding the same id twice.
            Some(at) => snapshot.items[at] = restored.clone(),
            None => snapshot.items.push(restored.clone()),
        }
        // UNCONDITIONAL, unlike the `position`-guarded writes above: both arms
        // changed the snapshot, and this entry has a stale `deleted: true` to
        // overwrite. See the doc.
        snapshot.note_item_write(&item.id, false);
        Ok(restored)
    }

    /// Deletes a trashed item for good, via
    /// [`VaultBridge::purge_item`]'s `permanent=true`.
    ///
    /// The snapshot side is [`Self::delete_item`]'s, and for the same reason:
    /// the id must be recorded as deleted **unconditionally**, because what
    /// the record has to survive is not the current snapshot (a trashed item
    /// is normally absent from it already) but a fetch that predates the
    /// purge and still carries the item as live. The `retain` is the belt to
    /// that braces -- a purge of an item an older populate had just restored
    /// to the snapshot must still leave.
    ///
    /// Nothing prunes a cached trash list here because there is none; see
    /// [`Self::list_trash_unless_superseded`] for why that is the point rather
    /// than an omission.
    pub fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.persisting(|| self.purge_item_writing(id))
    }

    /// [`Self::purge_item`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn purge_item_writing(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.purge_item(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.retain(|i| i.id != id);
            snapshot.note_item_write(id, true);
        }
        Ok(())
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        self.persisting(|| self.create_folder_writing(name))
    }

    /// [`Self::create_folder`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn create_folder_writing(&self, name: &str) -> Result<Folder, VaultError> {
        let created = self.bridge.create_folder(name)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.push(created.clone());
            snapshot.note_folder_write(&created.id, false);
        }
        Ok(created)
    }

    pub fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        self.persisting(|| self.update_folder_writing(id, name))
    }

    /// [`Self::update_folder`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn update_folder_writing(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
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
        self.persisting(|| self.delete_folder_writing(id))
    }

    /// [`Self::delete_folder`]'s body. Separated only so that the disk
    /// rewrite happens after this returns and therefore after the
    /// snapshot guard below is released -- see [`Self::persisting`].
    fn delete_folder_writing(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.folders.retain(|f| f.id != id);
            snapshot.note_folder_write(id, true);
        }
        Ok(())
    }

    // -- the optional encrypted file --------------------------------------
    //
    // Everything about that file lives behind this type, for the reason
    // every vault write already routes through here: there is exactly one
    // place that can be wrong. No call site persists, deletes, or reasons
    // about it directly.

    /// Rewrites the file from the current snapshot.
    ///
    /// **Best-effort by design.** The in-memory cache is authoritative and
    /// the app is fully functional without the file, so a disk-full,
    /// permission-denied or antivirus-locked write is a `warn` and nothing
    /// more. It never surfaces a modal; the only cost of a failed write is a
    /// slower next launch.
    ///
    /// Deliberately takes no arguments and reads the snapshot itself: every
    /// caller is "the snapshot just changed", and passing the data in would
    /// create a second place that could pass the wrong data.
    ///
    /// **Lock ordering, and it is not optional:** `disk_state` first, then
    /// `snapshot`, and the snapshot guard is released before the write. The
    /// write is ~1 MB of AES plus a file rename, and holding the snapshot
    /// mutex across it would block every read in the app -- autofill's
    /// included -- for the duration. Every other method here takes the two in
    /// that same order, or takes only one.
    fn persist(&self) {
        let Some(disk) = self.disk.as_ref() else {
            return;
        };
        let fingerprint = {
            let state = self.lock_disk();
            if !state.enabled {
                return;
            }
            state.fingerprint.clone()
        };
        let (items, folders) = {
            let snapshot = self.lock();
            if !snapshot.populated {
                return;
            }
            (snapshot.items.clone(), snapshot.folders.clone())
        };
        // **The facts section, built here because this is where the items
        // are.** `vault_disk_cache` takes it as opaque bytes and has no
        // opinion about what a projection contains -- see its `write`. What
        // it buys is that a reader wanting names, usernames and websites
        // never opens a single sealed secret.
        //
        // `crate::app::ItemFacts` is referenced from here rather than moved
        // somewhere neutral, and that is a deliberate small debt: the type is
        // a decision about what the account picker needs, its constructor
        // reaches `key_sequence`, `favicon` and `app`'s own sequence lookup,
        // and splitting it across modules to satisfy a layering diagram would
        // scatter one decision over three files.
        // `{ items, folders }`, and the folders are in here rather than
        // beside the secrets on purpose: a folder is a name and an id, so a
        // reader that wants the vault's shape should not have to open a
        // single sealed item to get it.
        #[derive(serde::Serialize)]
        struct Facts<'a> {
            items: Vec<crate::app::ItemFacts>,
            folders: &'a [Folder],
        }
        let facts = Facts {
            items: items.iter().map(crate::app::ItemFacts::of).collect(),
            folders: &folders,
        };
        let facts_bytes = match serde_json::to_vec(&facts) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("could not serialize the vault cache's facts section: {e}");
                return;
            }
        };
        if let Err(e) = disk.write(&fingerprint, &facts_bytes, &items, &folders) {
            log::warn!("could not write the encrypted vault cache: {e}");
        }
    }

    /// Runs a snapshot-mutating write and rewrites the file if it succeeded.
    ///
    /// **Every mutating door on this type goes through here**, which is what
    /// keeps "the file is rewritten after every successful mutation" one fact
    /// rather than thirteen. It is also why each of those doors is a
    /// one-line wrapper over a `_writing` body: the body holds the snapshot
    /// guard to its last statement, and [`Self::persist`] must not be called
    /// while that guard is alive. Running the body to completion first is the
    /// lock-ordering rule enforced by construction rather than remembered.
    ///
    /// A failed write is not persisted, which is the whole reason the check
    /// is on the `Result` and not on "we got here": the bridge call comes
    /// first in every body, so an error means the server refused and the
    /// snapshot was never touched.
    fn persisting<T>(
        &self,
        write: impl FnOnce() -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let outcome = write();
        if outcome.is_ok() {
            self.persist();
        }
        outcome
    }

    /// Populates the snapshot from the file, if there is a usable one.
    ///
    /// The caller gets the full outcome rather than a `bool`, because "no
    /// file", "rejected and deleted", "Hello declined" and "corrupt" call for
    /// different logging and different next steps -- and because three of
    /// them have already left the file in different states by the time this
    /// returns.
    ///
    /// **It restores through [`Self::populate_with_vault`]**, so the era
    /// guard, the replay of local writes and the `DiscardedStale` answer are
    /// the same code the network path runs. A restore that had its own
    /// write-back would be a second place those rules could drift; the
    /// only thing this path adds is [`Source::DiskCache`], which records the
    /// file's age instead of clearing it and writes nothing back.
    ///
    /// The epoch is captured **before** the read, as
    /// [`Self::populate_with_vault`] requires: the read is file I/O and can
    /// be preceded by a Hello prompt the user takes seconds over, and a
    /// `clear` in that window must discard this restore rather than resurrect
    /// a locked vault.
    pub fn load_from_disk(&self) -> DiskCacheLoad {
        let Some(disk) = self.disk.as_ref() else {
            return DiskCacheLoad::Absent;
        };
        let fingerprint = {
            let state = self.lock_disk();
            if !state.enabled {
                return DiskCacheLoad::Absent;
            }
            state.fingerprint.clone()
        };
        let epoch = self.epoch();
        let outcome = disk.load(&fingerprint);
        if let DiskCacheLoad::Loaded {
            items,
            folders,
            written_at,
        } = &outcome
        {
            let vault = VaultSnapshot {
                items: items.clone(),
                folders: folders.clone(),
            };
            match self.write_back_at_epoch(
                vault.items,
                vault.folders,
                epoch,
                Source::DiskCache(*written_at),
            ) {
                PopulateOutcome::Populated => {}
                PopulateOutcome::DiscardedStale => log::info!(
                    "the vault was cleared while the encrypted disk cache was being read; \
                     the restore was discarded and the snapshot stays empty"
                ),
            }
        }
        outcome
    }

    /// When the currently-held snapshot was written to disk, if it came from
    /// the file and nothing has refreshed it from the backend since.
    ///
    /// The vault window's toolbar pill reads this so it reports an age
    /// instead of claiming a sync that never happened in this session. This
    /// codebase has already shipped that bug once in a narrower form; a disk
    /// cache makes the possible gap days wide rather than one sync interval.
    pub fn loaded_from_disk_at(&self) -> Option<SystemTime> {
        self.lock_disk().loaded_from_disk_at
    }

    /// Whether there is a local copy this session declined to open, because
    /// the Windows Hello prompt was cancelled or failed.
    ///
    /// [`Self::load_from_disk`] answers [`DiskCacheLoad::Unavailable`] in that
    /// case and leaves the file exactly where it is, so nothing in the outcome
    /// says a copy is sitting there. The offline affordances need to know,
    /// because "there is no copy of your vault on this machine" and "you
    /// dismissed a fingerprint prompt" must not look the same on screen.
    ///
    /// Gated on `enabled` for the reason every other disk method here is: with
    /// the setting off this cache is inert, and a file left behind by a
    /// session that had it on is not something to offer.
    pub fn disk_copy_awaiting_key(&self) -> bool {
        if !self.lock_disk().enabled {
            return false;
        }
        self.disk
            .as_ref()
            .is_some_and(|disk| disk.declined_copy_on_disk())
    }

    /// Restores from the file **at the user's own request**, after they
    /// pressed *Open the local copy* / *Continue offline*.
    ///
    /// [`Self::load_from_disk`] with one thing added: a session that already
    /// refused a Hello prompt is allowed one more. That refusal is normally
    /// final for the session, so the next write does not pop a biometric out
    /// of nowhere -- see `DiskCache::allow_one_more_key_attempt` for why a
    /// button press is the one gesture that reasoning was never about.
    ///
    /// Nothing else differs. In particular it is still `Source::DiskCache`
    /// underneath, so opening the copy does not stamp the file with the
    /// current time and does not reset its seven-day expiry.
    pub fn open_disk_copy(&self) -> DiskCacheLoad {
        if let Some(disk) = self.disk.as_ref() {
            disk.allow_one_more_key_attempt();
        }
        self.load_from_disk()
    }

    /// Turns persistence on: acquires the Windows Hello key -- **this is the
    /// prompt the user sees, and it doubles as the confirmation gesture, so
    /// there is no separate modal** -- and writes the first file from the
    /// snapshot already in memory.
    ///
    /// The key comes first and the flag second, so a refused or failed
    /// acquisition leaves the setting exactly as it was. A cache that
    /// reported itself enabled with no key to run on would render as on while
    /// nothing was ever written.
    pub fn enable_disk_persistence(&self) -> Result<(), String> {
        let disk = self
            .disk
            .as_ref()
            .ok_or_else(|| "there is no directory to keep the vault copy in".to_string())?;
        disk.acquire_key()?;
        self.lock_disk().enabled = true;
        self.persist();
        Ok(())
    }

    /// Turns persistence off and deletes the file.
    ///
    /// The flag is cleared **before** the delete, so a populate racing this
    /// cannot write the file back immediately after it is removed. The error
    /// is returned rather than logged because this is the one disk-cache
    /// failure worth surfacing: the user asked for the file to be gone and it
    /// is not.
    pub fn disable_disk_persistence(&self) -> Result<(), String> {
        {
            let mut state = self.lock_disk();
            state.enabled = false;
            state.loaded_from_disk_at = None;
        }
        match self.disk.as_ref() {
            Some(disk) => disk.delete(),
            None => Ok(()),
        }
    }

    /// Deletes the file while leaving persistence enabled.
    ///
    /// Used on **re-authentication** -- any master-password prompt, which is
    /// a superset of the master-password change we cannot detect directly,
    /// since `bw status` exposes no key fingerprint and the session token
    /// changes on every unlock -- and on **log out**. The next successful
    /// populate writes a fresh one, which costs nothing at either moment
    /// because the backend is already up and the snapshot is already being
    /// rebuilt.
    ///
    /// **Not what lock and quit do.** Those call [`Self::clear`], which
    /// leaves the file alone.
    pub fn forget_disk_copy(&self) -> Result<(), String> {
        self.lock_disk().loaded_from_disk_at = None;
        match self.disk.as_ref() {
            Some(disk) => disk.delete(),
            None => Ok(()),
        }
    }

    /// Points this cache's file and fingerprint at a different account.
    ///
    /// Called by the account switch **before** it authenticates, for the
    /// reason `SessionStore::path` gives about its own re-point: a write
    /// issued after the switch has begun but against the outgoing account's
    /// path is a mutation no end-state assertion catches. Here it would key
    /// the incoming account's vault under the outgoing account's fingerprint,
    /// in the outgoing account's directory.
    ///
    /// The from-disk age goes with them: whatever the pill was reporting was
    /// about a file this cache no longer addresses.
    pub fn repoint_disk_cache(&self, dir: &std::path::Path, fingerprint: String) {
        let mut state = self.lock_disk();
        state.fingerprint = fingerprint;
        state.loaded_from_disk_at = None;
        if let Some(disk) = self.disk.as_ref() {
            disk.repoint(dir);
        }
    }

    /// The file this cache would read and write, if it has one. For the
    /// startup log and for the tests that assert on the filesystem.
    pub fn disk_cache_path(&self) -> Option<std::path::PathBuf> {
        self.disk.as_ref().map(|disk| disk.path())
    }

    fn lock_disk(&self) -> std::sync::MutexGuard<'_, DiskState> {
        self.disk_state.lock().unwrap_or_else(|e| e.into_inner())
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

    /// See `vault_bridge::echoing_item_put`: what the server reports back as
    /// the item's new `revisionDate`, chosen to differ from every fixture's so
    /// "kept what we sent" and "took what the server answered" cannot look
    /// alike.
    const NEXT_REVISION: &str = "2026-08-03T02:33:03.427Z";

    use crate::vault_bridge::{echoing_item_put, with_app_match};
    use crate::app_match::TriggerMode;
    use crate::vault_bridge::{CardData, IdentityData};

    fn cache_for(url: String) -> VaultCache {
        VaultCache::new(VaultBridge::new(url))
    }

    /// One item that actually carries a password, which `items_body`'s do
    /// not. Separate rather than added to that fixture, because every test
    /// using it asserts on a two-item list and a third would move them all.
    fn items_body_with_a_password() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1,
             "login":{"username":"me@example.com","password":"hunter2"}}
        ]}}"#
    }

    /// **The daemon must not hold a decrypted password.**
    ///
    /// It owns the tray, the hotkey and the match engine, and it runs for
    /// days. `clear()` empties the snapshot on lock -- but auto-lock is a
    /// setting a user can turn off, and the owner's is off with a 999-minute
    /// timeout, so "until the vault locks" is in practice "until the process
    /// exits".
    ///
    /// Driven through the public accessor a caller actually has rather than
    /// by reaching into the field: what matters is what the daemon can
    /// *reach*.
    #[test]
    #[ignore = "BLOCKED on two gates, both in \
                docs/superpowers/plans/2026-08-27-the-tray-stops-holding-passwords.md. \
                (1) A fill still reads a cached password, and must, because autofill has to \
                work with `bw serve` stopped -- see \
                `autofill_really_fills_from_a_restored_snapshot`. (2) The sign-in path still \
                draws the vault window in-process and reads the cache directly. Both need \
                per-item read from the encrypted cache, which is \
                docs/superpowers/specs/2026-08-27-the-vault-lives-in-a-place-not-a-process.md. \
                Ignored rather than deleted or weakened: this is the finish line, and it \
                fails for the right reason today."]
    fn nothing_the_daemon_can_reach_hands_back_a_password() {
        let cache = cache_for("http://127.0.0.1:1".to_string());
        let epoch = cache.epoch();
        // The outcome is asserted rather than discarded: a populate that was
        // refused would leave the snapshot empty, and this test would then
        // pass by finding no password in nothing at all.
        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body_with_a_password()),
                    folders: Vec::new(),
                },
                epoch,
            ),
            PopulateOutcome::Populated,
            "control: the snapshot was not filled, so there is nothing to find a password in"
        );
        let reachable: Vec<String> = cache
            .items()
            .into_iter()
            .filter_map(|item| item.login.and_then(|l| l.password).map(|p| p.to_string()))
            .collect();
        assert!(
            reachable.is_empty(),
            "the daemon can read {} cached password(s) out of its own snapshot",
            reachable.len()
        );
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

    /// The list inside one of the `*_body` fixtures above, as values.
    ///
    /// Parsed from the very same JSON the mocked routes serve, so a test that
    /// seeds its cache offline and one that seeds it over HTTP cannot drift
    /// apart: change `items_body` and both move together.
    fn body_list<T: serde::de::DeserializeOwned>(body: &str) -> Vec<T> {
        let envelope: serde_json::Value =
            serde_json::from_str(body).expect("the fixture body is JSON");
        serde_json::from_value(envelope["data"]["data"].clone())
            .expect("the fixture body's `data.data` is a list")
    }

    /// An **offline** cache already holding the `items_body` items and the
    /// `folders_body` folder, with a bridge that can never answer.
    ///
    /// For the tests below whose subject is what the cache does with a
    /// snapshot it already has -- the era-checked reads above all -- rather
    /// than how it got one. They used to stand up a `mockito` server for two
    /// GETs and then never touch it again, which cost a pooled port for the
    /// life of the test and, since mockito 1.7 recycles rather than shuts
    /// down, made them able to disturb whichever unrelated test was handed
    /// that port next. See [`crate::test_vault`].
    ///
    /// Tests that are ABOUT the fetch -- `populate`, the readiness probe, the
    /// write-through of every mutation -- deliberately keep their servers:
    /// HTTP is what they are checking.
    fn seeded_offline_cache() -> VaultCache {
        crate::test_vault::cache_with(body_list(items_body()), body_list(folders_body()))
    }

    /// A hit answers with the item, and with the SAME item `items()` would
    /// have found.
    ///
    /// The equality against the `items()`-and-`find` shape is the point:
    /// `get_by_id` exists to make the autofill path stop cloning the whole
    /// vault, and a replacement that answered anything different would be a
    /// behaviour change dressed up as a latency fix. The bridge here can
    /// never answer, so a `get_by_id` that went to HTTP for a hit could not
    /// pass this.
    #[test]
    fn get_by_id_answers_a_hit_with_what_items_would_have_found() {
        let cache = seeded_offline_cache();
        let found = cache.get_by_id("2").expect("the fixture holds item 2");
        assert_eq!(found.name, "Beta");
        // `VaultItem` is not `PartialEq`, and giving it one for a test would
        // put a derive on a type carrying plaintext secrets purely to serve
        // this line. Its `Serialize` is the whole value, so comparing the two
        // serialisations compares every field, not just the two named above.
        let via_items = cache
            .items()
            .into_iter()
            .find(|i| i.id == "2")
            .expect("the whole-vault clone finds it too");
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            serde_json::to_value(&via_items).unwrap(),
            "get_by_id and the whole-vault clone it replaced disagree about item 2"
        );
    }

    /// A miss is `None`, from the snapshot, without touching the bridge.
    ///
    /// `None` is what `app.rs`'s fill path turns into its warned fallback to
    /// `bw serve`; a `get_by_id` that reached the bridge itself would swallow
    /// that miss silently and would also break filling with the backend
    /// stopped. The offline fixture's bridge cannot answer, so reaching it
    /// would hang or error rather than return `None`.
    #[test]
    fn get_by_id_answers_a_miss_with_none_and_does_not_reach_the_bridge() {
        let cache = seeded_offline_cache();
        assert!(
            !cache.items().is_empty(),
            "control: an empty cache would answer None for every id"
        );
        assert!(cache.get_by_id("no-such-id").is_none());
    }

    #[test]
    fn populate_fills_the_snapshot_and_reads_come_from_it() {
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
            ssh_key: None,
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
        let mut server = crate::test_http::server();
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
            ssh_key: None,
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
        let mut server = crate::test_http::server();
        let _f = server.mock("GET", "/list/object/folders").with_status(500).create();

        let cache = cache_for(server.url());
        let seeded = vec![VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
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
        }];

        assert!(cache.populate_with(seeded, cache.epoch()).is_err());
        assert!(!cache.is_populated());
        assert!(cache.items().is_empty());
    }

    /// **`populate_with_vault` asks the bridge for nothing.**
    ///
    /// The bridge here can never answer -- see [`crate::test_vault`] -- so a
    /// door that still fetched its folders, or that had grown any other
    /// round-trip, could not populate this cache at all. The empty folder
    /// list is the one the CALLER handed in, which the assertion below
    /// distinguishes from "the fetch quietly returned nothing" by handing in
    /// a folder and finding it.
    #[test]
    fn populate_with_vault_seeds_both_halves_without_touching_the_bridge() {
        let cache = VaultCache::new(crate::test_vault::unreachable_bridge());
        let epoch = cache.epoch();

        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch
            ),
            PopulateOutcome::Populated
        );

        assert!(cache.is_populated());
        assert_eq!(cache.items().len(), 2);
        assert_eq!(
            cache.folders().into_iter().map(|f| f.name).collect::<Vec<_>>(),
            vec!["Work".to_string()],
            "the folders must be the caller's, not an empty list from a fetch that never happened"
        );
    }

    /// **The era guard is the same guard**, and the new door is not a way
    /// around it.
    ///
    /// A `clear` between the caller's mark and the write-back means a
    /// different vault session, so the snapshot must stay empty and
    /// unpopulated exactly as `populate` and `populate_with` leave it -- see
    /// `a_populate_whose_epoch_was_bumped_mid_flight_leaves_the_cache_empty`,
    /// which asserts the same thing through the fetching door. If
    /// `populate_with_vault` ever grew its own write-back this test is what
    /// notices.
    #[test]
    fn populate_with_vault_discards_a_vault_whose_era_has_been_cleared_away() {
        let cache = VaultCache::new(crate::test_vault::unreachable_bridge());
        let epoch = cache.epoch();

        // Stands in for "the caller was reading the vault from disk while the
        // user locked the app" -- the disk-cache restore this door exists for.
        cache.clear();

        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch
            ),
            PopulateOutcome::DiscardedStale
        );
        assert!(!cache.is_populated(), "a discarded populate must not mark the cache populated");
        assert!(cache.items().is_empty());
        assert!(cache.folders().is_empty());
    }

    /// **And `clear` is still the only thing that begins an era.** A populate
    /// through the new door leaves the era exactly where it found it, so a
    /// caller holding an era captured before it can still read the result --
    /// which is the whole reason [`VaultEra`] is not bumped by writes either.
    #[test]
    fn populate_with_vault_does_not_begin_an_era() {
        let cache = VaultCache::new(crate::test_vault::unreachable_bridge());
        let era_before = cache.epoch().era();

        let epoch = cache.epoch();
        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot { items: body_list(items_body()), folders: Vec::new() },
                epoch
            ),
            PopulateOutcome::Populated
        );

        assert_eq!(cache.epoch().era(), era_before, "populating started a new era");
        assert_eq!(
            cache.snapshot_unless_superseded(era_before).expect("the era still stands").items.len(),
            2,
            "a reader holding the pre-populate era must still be able to read the result"
        );
    }

    #[test]
    fn clear_empties_the_snapshot_so_idle_holds_no_vault_contents() {
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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

    /// The vault the stale-token tests run against: one item, carrying the
    /// `revisionDate` a real `/list/object/items` element carries.
    fn items_body_with_a_revision_date() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1,"favorite":false,
             "revisionDate":"2026-08-03T02:31:59.604Z"}
        ]}}"#
    }

    const FETCHED_REVISION: &str = "2026-08-03T02:31:59.604Z";
    const AFTER_FIRST_WRITE: &str = "2026-08-03T02:32:06.832Z";
    const AFTER_SECOND_WRITE: &str = "2026-08-03T02:33:03.427Z";

    fn cache_with_one_dated_item(server: &mut mockito::Server) -> VaultCache {
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body_with_a_revision_date())
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache
    }

    /// THE USER-REPORTED DEFECT. Favouriting is the one operation a user
    /// repeats on a single item -- star, look elsewhere, star again -- so it
    /// is where the stale optimistic-concurrency token surfaced first.
    ///
    /// `revisionDate` rides `VaultItem::other`, so it is on the wire of every
    /// full-state PUT, and Bitwarden reads it as "the version you think you
    /// are editing". The write bumps it. An app that keeps the value it SENT
    /// is holding a superseded token from that instant, and the live backend
    /// answers its next write of that item with
    /// `400 The client copy of this cipher is out of date. Resync the client
    /// and try again.` -- measured against the user's `bw serve` 2026.7.0.
    ///
    /// The two mocks are keyed on the token so this cannot pass by accident:
    /// the second write must carry what the FIRST write's response reported,
    /// not what the populate did.
    #[test]
    fn a_second_favourite_toggle_carries_the_revision_date_the_first_write_returned() {
        let mut server = crate::test_http::server();
        let cache = cache_with_one_dated_item(&mut server);
        let first = echoing_item_put(&mut server, "/object/item/1", AFTER_FIRST_WRITE)
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "revisionDate": FETCHED_REVISION,
            })))
            .expect(1)
            .create();
        let second = echoing_item_put(&mut server, "/object/item/1", AFTER_SECOND_WRITE)
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "revisionDate": AFTER_FIRST_WRITE,
            })))
            .expect(1)
            .create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert_eq!(
            item.other.get("revisionDate").and_then(|v| v.as_str()),
            Some(FETCHED_REVISION),
            "the premise: the populated item carries the token the fetch reported"
        );

        let starred = cache.set_favorite(&item, true).expect("the first toggle");
        // POSITIVE CONTROL: the first write really happened, and really was
        // the one keyed on the fetched token. Without this, the assertion
        // below could pass on a path that made no request at all.
        first.assert();
        assert!(starred.favorite, "the first toggle did not report the item as favourited");
        assert_eq!(
            starred.other.get("revisionDate").and_then(|v| v.as_str()),
            Some(AFTER_FIRST_WRITE),
            "set_favorite handed back the value it SENT, not the server's answer -- so the \
             caller now holds a superseded token"
        );

        let unstarred = cache.set_favorite(&starred, false).expect(
            "the second toggle was refused -- the body it sent carried a superseded revisionDate",
        );
        second.assert();
        assert!(!unstarred.favorite);
        assert_eq!(
            cache
                .items()
                .into_iter()
                .find(|i| i.id == "1")
                .unwrap()
                .other
                .get("revisionDate")
                .and_then(|v| v.as_str()),
            Some(AFTER_SECOND_WRITE),
            "the snapshot kept a stale token, so the THIRD write of this item would be refused"
        );
    }

    /// The same defect on the edit path, which is the other repeated write.
    /// Separate rather than folded into the test above because the two reach
    /// different `VaultCache` methods, and this repository's most-repeated
    /// finding is a decision that is right while one of the wires into it is
    /// not.
    #[test]
    fn a_second_edit_of_one_item_carries_the_revision_date_the_first_save_returned() {
        let mut server = crate::test_http::server();
        let cache = cache_with_one_dated_item(&mut server);
        let first = echoing_item_put(&mut server, "/object/item/1", AFTER_FIRST_WRITE)
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "revisionDate": FETCHED_REVISION,
            })))
            .expect(1)
            .create();
        let second = echoing_item_put(&mut server, "/object/item/1", AFTER_SECOND_WRITE)
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "revisionDate": AFTER_FIRST_WRITE,
            })))
            .expect(1)
            .create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        let mut renamed = item.clone();
        renamed.name = "Alpha renamed".to_string();
        let saved = cache.update_item(&renamed).expect("the first save");
        first.assert();
        // POSITIVE CONTROL: the answer this adopts is still the edit that was
        // made, not a fixture that happens to carry the right token.
        assert_eq!(saved.name, "Alpha renamed", "the first save lost the edit");

        let mut again = saved.clone();
        again.name = "Alpha renamed twice".to_string();
        cache
            .update_item(&again)
            .expect("the second save was refused -- it sent a superseded revisionDate");
        second.assert();
    }

    fn an_app_match() -> AppMatch {
        AppMatch::for_process("notepad.exe", TriggerMode::Prompt)
    }

    #[test]
    fn set_app_match_updates_the_cached_item() {
        let mut server = crate::test_http::server();
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
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();

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
        assert_eq!(field.value.as_ref().map(|v| v.as_str()), Some(m.to_field_value().as_str()));
    }

    #[test]
    fn a_failed_set_app_match_leaves_the_cache_untouched() {
        let mut server = crate::test_http::server();
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

    /// The two-item vault of `items_body`, with item 1 filed under `f1` so a
    /// move OUT of a folder has somewhere to move out of.
    fn items_body_with_one_filed_item() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1,"folderId":"f1"},
            {"id":"2","name":"Beta","fields":[],"type":1}
        ]}}"#
    }

    fn populated_cache_with_a_filed_item(server: &mut mockito::Server) -> VaultCache {
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body_with_one_filed_item())
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache
    }

    #[test]
    fn moving_an_item_into_a_folder_updates_the_cached_item() {
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = echoing_item_put(&mut server, "/object/item/2", NEXT_REVISION).create();

        let item = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert_eq!(item.folder_id, None, "the premise: item 2 starts unfiled");

        cache.move_item_to_folder(&item, Some("f1")).unwrap();

        let moved = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert_eq!(
            moved.folder_id.as_deref(),
            Some("f1"),
            "the server took the move but the snapshot did not, so the next read from the \
             cache still files the item where it was"
        );
    }

    #[test]
    fn unfiling_an_item_clears_the_cached_folder_id() {
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        // The mock matches ONLY a body that states `folderId` as present and
        // null, so this also pins that the cache routes through
        // `VaultBridge::move_item_to_folder` and not through `update_item` --
        // the latter omits the key and would get a 501 here.
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "Alpha",
                "type": 1,
                "fields": [],
                "favorite": false,
                "folderId": null,
            })))
            .create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert_eq!(item.folder_id.as_deref(), Some("f1"), "the premise: item 1 starts in f1");

        cache.move_item_to_folder(&item, None).unwrap();

        let moved = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert_eq!(
            moved.folder_id, None,
            "the un-file did not reach the snapshot, so the sidebar's folder counts and the \
             next window open still show the item under its old folder"
        );
    }

    #[test]
    fn a_failed_move_leaves_the_cache_untouched() {
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = server.mock("PUT", "/object/item/1").with_status(500).create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(
            cache.move_item_to_folder(&item, None).is_err(),
            "a rejected move must come back Err -- the vault window reverts the dragged row \
             on exactly that"
        );

        let unchanged = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert_eq!(
            unchanged.folder_id.as_deref(),
            Some("f1"),
            "a failed move un-filed the cached item anyway"
        );
    }

    #[test]
    fn favouriting_an_item_writes_through_to_the_snapshot() {
        // The sidebar's Favorites row counts and filters on `item.favorite`
        // read from THIS snapshot (`sidebar::SidebarFilter::Favorites`), so a
        // write the server took and the cache did not means the star lights
        // up and the row the user just favourited is not in Favorites.
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        // The mock matches only a body that STATES `favorite: true`, so this
        // also pins the wire shape and not merely the in-memory result.
        let _u = echoing_item_put(&mut server, "/object/item/2", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "2",
                "name": "Beta",
                "type": 1,
                "fields": [],
                "favorite": true,
            })))
            .create();

        let item = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert!(!item.favorite, "the premise: item 2 starts un-favourited");

        let written = cache.set_favorite(&item, true).unwrap();
        assert!(written.favorite, "the item handed back does not carry the new flag");

        let cached = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert!(
            cached.favorite,
            "the server took the favourite but the snapshot did not, so the sidebar's \
             Favorites row still excludes it"
        );
    }

    #[test]
    fn un_favouriting_an_item_writes_through_too() {
        // Both directions, so the write-through cannot rot into "always sets".
        let mut server = crate::test_http::server();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"data":{"data":[
                    {"id":"1","name":"Alpha","fields":[],"type":1,"favorite":true}
                ]}}"#,
            )
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "Alpha",
                "type": 1,
                "fields": [],
                "favorite": false,
            })))
            .create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(item.favorite, "the premise: item 1 starts favourited");

        let written = cache.set_favorite(&item, false).unwrap();
        assert!(!written.favorite);
        assert!(!cache.items().into_iter().find(|i| i.id == "1").unwrap().favorite);
    }

    #[test]
    fn a_failed_favourite_write_leaves_the_cache_untouched_and_hands_back_no_item() {
        // THE "DOES NOT CLAIM SUCCESS" GUARD, and the reason `set_favorite`
        // returns the written item rather than `()`: on a rejected PUT there
        // is no new item to hand back at all, so a caller physically cannot
        // paint a filled star from a failed write -- it holds only its
        // original item and an error. The snapshot must be untouched too, or
        // the sidebar would count a favourite the vault does not have.
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = server.mock("PUT", "/object/item/2").with_status(500).create();

        let item = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert!(!item.favorite, "the premise");

        let result = cache.set_favorite(&item, true);
        assert!(result.is_err(), "a rejected favourite write must come back Err");

        assert!(
            !cache.items().into_iter().find(|i| i.id == "2").unwrap().favorite,
            "a failed favourite write marked the cached item anyway"
        );
    }

    #[test]
    fn a_401_on_a_favourite_write_arrives_as_unauthorized() {
        // The re-auth path keys off this variant. A locked vault behind a
        // star click must not read as a generic failure.
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = server.mock("PUT", "/object/item/2").with_status(401).create();
        let item = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        assert!(matches!(cache.set_favorite(&item, true), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn a_favourite_that_lands_while_a_populate_is_fetching_is_not_reverted_by_it() {
        // The same interleaving `a_move_that_lands_while_a_populate_is_fetching`
        // pins, for this write: without the `note_item_write` in
        // `set_favorite`, a populate whose fetch predates the click silently
        // un-stars the item on the next sync.
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = echoing_item_put(&mut server, "/object/item/2", NEXT_REVISION).create();

        let mark = cache.epoch();
        let fetched = cache.bridge().list_items().unwrap();
        assert!(
            !fetched.iter().find(|i| i.id == "2").unwrap().favorite,
            "the fetch must predate the write"
        );

        let item = cache.items().into_iter().find(|i| i.id == "2").unwrap();
        cache.set_favorite(&item, true).unwrap();
        assert_eq!(
            cache.epoch().era(),
            mark.era(),
            "a favourite write must not start a new era"
        );

        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(
            cache.items().into_iter().find(|i| i.id == "2").unwrap().favorite,
            "a populate holding a pre-click fetch un-starred the item"
        );
    }

    #[test]
    fn a_move_that_lands_while_a_populate_is_fetching_is_not_reverted_by_it() {
        // The same interleaving as
        // `a_write_that_lands_while_a_populate_is_fetching_is_not_reverted_by_it`,
        // for the move: mark, fetch, move, and only then let the populate
        // write its older fetch back. A move is a write like any other here,
        // so it must not start a new era and must be replayed over a fetch
        // that predates it. Without the `note_item_write` in
        // `move_item_to_folder` the item is silently filed back under `f1`,
        // and the user's drag undoes itself on the next sync.
        //
        // NOTE the ordering this test had to be corrected to. Written first as
        // "move, then populate", it failed -- correctly: a populate whose
        // fetch STARTS after the write is entitled to that fetch, and only a
        // mock could return pre-move data from it. The replay covers the
        // window between a populate's mark and its lock, and nothing else.
        let mut server = crate::test_http::server();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();

        let mark = cache.epoch();
        let fetched = cache.bridge().list_items().unwrap();
        assert_eq!(
            fetched.iter().find(|i| i.id == "1").unwrap().folder_id.as_deref(),
            Some("f1"),
            "the fetch must predate the move"
        );

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.move_item_to_folder(&item, None).unwrap();
        assert_eq!(
            cache.epoch().era(),
            mark.era(),
            "a move must not start a new era -- that would supersede every reader holding one"
        );

        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );
        let after = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert_eq!(
            after.folder_id, None,
            "a populate reverted a move that was newer than its own fetch"
        );
    }

    // RENAMED IN REVIEW 30 (Minor 6), for this test and the two below it.
    // Review 28 deleted `items_unless_superseded` and deliberately kept these
    // three names so the by-name verification the review briefs run would not
    // break. The cost turned out to be worse: a test name that names a deleted
    // function is the same lie with a `#[test]` on it, and a grep for
    // `snapshot_unless_superseded` found only three of its six tests. The
    // contract wording after the prefix is preserved VERBATIM, so the mapping
    // is mechanical (`items_unless_superseded_*` -> `snapshot_unless_superseded_*`)
    // and any brief citing the old names can still find them.
    #[test]
    fn snapshot_unless_superseded_hands_back_a_write_that_landed_after_the_epoch_was_captured() {
        // The contract in one test (review 18's third finding). A caller
        // captures the epoch, goes away for a while, and comes back to ask
        // whether its result still applies. A write landed meanwhile. The
        // right answer is NOT "superseded" -- nothing changed vault session,
        // and the write is newer truth than anything that caller fetched --
        // it is "still applicable, and here is the snapshot INCLUDING that
        // write". An epoch comparison alone answers the first half and
        // silently invites the caller to act on its own stale copy.
        let mut server = crate::test_http::server();
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
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();

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
            .snapshot_unless_superseded(epoch.era())
            .expect("a write is not a supersession -- the vault session is the same one")
            .items;
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
    fn snapshot_unless_superseded_refuses_once_a_clear_has_started_a_new_epoch() {
        // The other half, so the test above cannot pass by never refusing.
        // A `clear` is a lock, a re-auth into a possibly different account,
        // or a quit: a result computed in the previous epoch must not be
        // acted on, and there is no snapshot to hand back either.
        //
        // WHICH refusal, not merely that one happened (review 30's Minor 6):
        // the state right after a `clear` is BOTH superseded and unpopulated,
        // which is exactly why the check order inside the door is load-bearing
        // -- reporting `Unpopulated` here would invite the populate that
        // cannot help. An `is_err` assertion passes under the swapped order
        // and so pinned nothing about the thing this test is named for.
        let mut server = crate::test_http::server();
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

        assert_eq!(
            cache.snapshot_unless_superseded(epoch.era()).unwrap_err(),
            VaultUnavailable::Superseded,
            "a cleared cache is superseded AND unpopulated; the stronger fact is the one the \
             caller must be told, or it goes and fetches a vault that cannot help it"
        );
        // ...and a *repopulate* for the new account does not make the old
        // era applicable again: this is the cross-account case, and the
        // era is what closes it.
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert_eq!(
            cache.snapshot_unless_superseded(epoch.era()).unwrap_err(),
            VaultUnavailable::Superseded,
            "an era from before a clear must stay superseded even once the cache refills"
        );
        assert!(cache.snapshot_unless_superseded(cache.epoch().era()).is_ok());
    }

    /// Every mock a populate plus a write to item 1 needs.
    fn populating_server_with_a_writable_item() -> crate::test_http::MockServer {
        let mut server = crate::test_http::server();
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
        echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();
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
    fn snapshot_unless_superseded_refuses_an_unfilled_snapshot_in_its_own_era() {
        // THE HALF THAT IS NOT ABOUT ERAS AT ALL, and that review 25's Minor 3
        // made load-bearing when `items_if_populated` was retired into this
        // function. A brand-new process has era 0 and `populated == false`, so
        // an era captured before the first populate compares EQUAL -- the era
        // check alone says "still yours" and would hand back an empty `Vec` as
        // though it were the vault. Both the picker's "Add app..." click and
        // `main`'s engine rebuild ask exactly here, and rebuilding the match
        // engine from an empty snapshot DISARMS autofill rather than merely
        // failing to arm whatever was just saved.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        let era_before_any_populate = cache.epoch().era();
        assert_eq!(
            cache
                .snapshot_unless_superseded(era_before_any_populate)
                .unwrap_err(),
            VaultUnavailable::Unpopulated,
            "a snapshot that has never been filled is not a vault, however current its era"
        );

        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert_eq!(
            cache
                .snapshot_unless_superseded(era_before_any_populate)
                .unwrap()
                .items
                .len(),
            2,
            "a populate does not begin a new era, so the same era must now yield the vault"
        );

        cache.clear();
        assert_eq!(
            cache
                .snapshot_unless_superseded(era_before_any_populate)
                .unwrap_err(),
            VaultUnavailable::Superseded,
            "a cleared snapshot is not a vault either -- and the era moved, so the refusal that \
             reaches the caller is the supersession, not the emptiness underneath it"
        );
    }

    #[test]
    fn a_write_survives_two_overlapping_populates_landing_out_of_order() {
        // REVIEW 23'S CRITICAL. There is more than one populate producer (a
        // tray sync, the picker, the vault window's force-refresh) and none of
        // them is gated on the others, so two can be in flight at once with
        // DIFFERENT marks. Pruning the pending log to "what MY mark did not
        // cover" throws away entries the OTHER, older, in-flight populate
        // still needs: my mark says nothing about what its fetch covered.
        //
        // B captures the older mark, W lands, A captures the newer mark and
        // fetches a copy that legitimately includes W. A lands first and
        // adopts its own fetch (correct). If A also PRUNES W's entry, B --
        // whose fetch predates W entirely -- then writes its pre-write items
        // back with nothing left to replay, and W is reverted. Per
        // `put-semantics-capture.md` the next vault-window edit PUTs that
        // stale copy back with a present `fields` array, so the loss becomes
        // permanent.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // Populate B: mark first, then its fetch. Both predate the write.
        let mark_b = cache.epoch();
        let fetched_b = cache.bridge().list_items().unwrap();
        assert!(
            !has_app_match(fetched_b.iter().find(|i| i.id == "1").unwrap()),
            "B's fetch must predate the write"
        );

        // The user's save lands between the two populates.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.set_app_match(&item, &an_app_match()).unwrap();

        // Populate A: mark captured AFTER the save, and a fetch that really
        // does cover it -- what the server would answer once the PUT landed.
        // (The mock always replays the pre-write body, so the covering fetch
        // is built here rather than requested.)
        let mark_a = cache.epoch();
        let fetched_a: Vec<VaultItem> = cache
            .bridge()
            .list_items()
            .unwrap()
            .into_iter()
            .map(|i| {
                if i.id == "1" {
                    with_app_match(&i, &an_app_match())
                } else {
                    i
                }
            })
            .collect();

        assert_eq!(
            cache.populate_with(fetched_a, mark_a).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(
            has_app_match(cache.items().iter().find(|i| i.id == "1").unwrap()),
            "A's own fetch covered the write, so the write must be there after A lands"
        );

        // ...and only now does the older populate write its result back.
        assert_eq!(
            cache.populate_with(fetched_b, mark_b).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(
            has_app_match(cache.items().iter().find(|i| i.id == "1").unwrap()),
            "the older overlapping populate reverted a write it could not have fetched -- \
             the newer populate had pruned the entry the older one still needed"
        );
    }

    #[test]
    fn a_populate_whose_fetch_dropped_a_written_id_the_snapshot_no_longer_has_skips_it() {
        // REVIEW 25'S IMPORTANT: the `else { continue }` arm of
        // `replay_writes`, which called itself unreachable, and which review
        // 23's deletion of the prune made reachable. This test IS the claim
        // now made at that arm, so the claim is checked rather than asserted.
        //
        // The trace, deterministic and with no threads:
        //  1. Populate. B takes the older mark and a fetch that still holds
        //     item 1.
        //  2. The user edits item 1 locally -> a live pending entry for it,
        //     and the snapshot holds it.
        //  3. Item 1 is deleted from ANOTHER client. Populate P, marked after
        //     the write, lands with a fetch that legitimately lacks item 1.
        //     Nothing is replayed (P's own fetch is newer than the write), so
        //     `snapshot.items = items` drops item 1 -- and the pending entry
        //     SURVIVES, because a populate no longer prunes on its own
        //     authority (review 23's Critical).
        //  4. B finally lands. Its mark predates the write, so the entry is
        //     replayed; it is not a delete; and the snapshot has no item 1 to
        //     copy. That is the arm.
        let server = populating_server_with_a_writable_item();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // (1) B: mark and fetch, both before the write.
        let mark_b = cache.epoch();
        let fetched_b = cache.bridge().list_items().unwrap();
        assert!(
            fetched_b.iter().any(|i| i.id == "1"),
            "B's fetch must predate the delete"
        );

        // (2) the local edit.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache.set_app_match(&item, &an_app_match()).unwrap();

        // (3) P: marked after the write, fetching a vault the remote delete
        // has already been applied to. (The mock always replays the same
        // body, so the post-delete fetch is built here rather than requested.)
        let mark_p = cache.epoch();
        let fetched_p: Vec<VaultItem> = cache
            .bridge()
            .list_items()
            .unwrap()
            .into_iter()
            .filter(|i| i.id != "1")
            .collect();
        assert_eq!(
            cache.populate_with(fetched_p, mark_p).unwrap(),
            PopulateOutcome::Populated
        );
        let ids: Vec<String> = cache.items().into_iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            vec!["2".to_string()],
            "P's own fetch is newer than the write, so it must be adopted as it stands -- \
             this is what takes item 1 out of the snapshot while its pending entry lives on"
        );

        // (4) B lands into exactly the state the arm describes.
        assert_eq!(
            cache.populate_with(fetched_b, mark_b).unwrap(),
            PopulateOutcome::Populated,
            "the arm must skip the entry, not fail the populate"
        );

        let after = cache.items();
        let one = after
            .iter()
            .find(|i| i.id == "1")
            .expect("B's own fetch carries item 1, and the arm must leave it exactly as fetched");
        assert!(
            !has_app_match(one),
            "the arm re-applied a local write the snapshot no longer holds -- there is nothing \
             newer to re-apply, and copying from a snapshot that dropped the id is not \
             something this arm can do"
        );
        assert_eq!(after.len(), 2);
        // What is left behind is the RECORDED fetch-vs-fetch staleness
        // deferral -- B's pre-delete fetch keeps item 1 in the snapshot until
        // the next populate -- and not a lost write. Asserted so a future
        // reader does not mistake it for a defect this test failed to catch.
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
        // Note this is per-populate and NOT a prune: the entry stays in the
        // log for any older populate still in flight that needs it (review
        // 23's Critical). What retires it for THIS populate is the comparison
        // `seq > mark`, which is a fact about this fetch alone.
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
    fn the_replay_log_line_says_which_ids_are_items_and_which_are_folders() {
        // Review 25's Minor 5. The two id spaces were concatenated under the
        // words "local write(s)", so the line could not be read without
        // guessing which endpoint each id belonged to.
        assert_eq!(
            replayed_summary(&["a".to_string(), "b".to_string()], &["f1".to_string()]),
            "item(s) a, b; folder(s) f1"
        );
        // ...and neither half is mentioned when it contributed nothing, so a
        // pure-item replay does not read as though a folder was touched.
        assert_eq!(replayed_summary(&["a".to_string()], &[]), "item(s) a");
        assert_eq!(replayed_summary(&[], &["f1".to_string()]), "folder(s) f1");
    }

    #[test]
    fn a_write_against_a_cleared_cache_does_not_resurrect_a_one_item_snapshot() {
        // After `clear()` (idle/locked), the snapshot must stay empty until
        // the next `populate()` -- the bridge call still happens and its
        // result is still returned, but the local snapshot must not gain a
        // one-item "vault" from a stray write while locked.
        let mut server = crate::test_http::server();
        let created_body = r#"{"success":true,"data":{"id":"3","name":"Gamma","fields":[],"type":1}}"#;
        let _c = server
            .mock("POST", "/object/item")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(created_body)
            .create();

        let cache = cache_for(server.url());
        assert!(!cache.is_populated());

        cache.create_item(&NewItem::login("Gamma", "", "", None)).unwrap();

        assert!(cache.items().is_empty(), "a write on a cleared cache resurrected a snapshot");
        assert!(!cache.is_populated());
    }

    /// Populates `cache` from the two standard mock lists, so a write test can
    /// assert on how the snapshot changed rather than on whether it exists.
    fn a_populated_cache(server: &mut crate::test_http::MockServer) -> VaultCache {
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
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache
    }

    #[test]
    fn creating_each_kind_lands_in_the_snapshot() {
        // Every kind routes through the cache, so the vault window's list
        // shows the new item without a re-fetch -- and so no caller has a
        // reason to reach around to `VaultBridge` and leave the snapshot
        // behind.
        for (new_item, id, name) in [
            (NewItem::login("Gamma", "u", "p", None), "3", "Gamma"),
            (NewItem::secure_note("Wifi", "body", None), "4", "Wifi"),
            (NewItem::card("Visa", CardData::default(), None), "5", "Visa"),
            (NewItem::identity("Me", IdentityData::default(), None), "6", "Me"),
            (NewItem::ssh_key("deploy", "PRIV", "PUB", "FP", None), "7", "deploy"),
        ] {
            let mut server = crate::test_http::server();
            let cache = a_populated_cache(&mut server);
            let _c = server
                .mock("POST", "/object/item")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(format!(
                    r#"{{"success":true,"data":{{"id":"{id}","name":"{name}","fields":[]}}}}"#
                ))
                .create();

            let created = cache.create_item(&new_item).unwrap();
            assert_eq!(created.id, id);
            let names: Vec<String> = cache.items().into_iter().map(|i| i.name).collect();
            assert_eq!(names, vec!["Alpha".to_string(), "Beta".to_string(), name.to_string()]);
        }
    }

    #[test]
    fn a_failed_create_leaves_the_snapshot_alone() {
        // The other half of the test above: the snapshot must reflect a
        // create that SUCCEEDED, and only that one. A 500 leaves the list as
        // it was, so the window never shows an item the server does not have.
        let mut server = crate::test_http::server();
        let cache = a_populated_cache(&mut server);
        let _c = server.mock("POST", "/object/item").with_status(500).create();

        assert!(cache.create_item(&NewItem::card("Visa", CardData::default(), None)).is_err());
        assert_eq!(cache.items().len(), 2, "a failed create was pushed into the snapshot");
    }

    #[test]
    fn snapshot_unless_superseded_hands_back_items_and_folders_of_one_era() {
        let cache = seeded_offline_cache();

        let snapshot = cache
            .snapshot_unless_superseded(cache.epoch().era())
            .expect("a populated snapshot in its own era is a vault");
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.folders.len(), 1);
    }

    #[test]
    fn snapshot_unless_superseded_separates_a_superseded_era_from_an_unfilled_snapshot() {
        // REVIEW 26'S MINOR 3. A bare `Option` collapses two answers that
        // want opposite handling: "there is no vault for your era, and
        // fetching one cannot produce one" (a `clear` happened -- the caller
        // must give up) and "nothing has been fetched yet" (a populate is
        // exactly the cure). `picker_ui` paid for that with a whole vault
        // fetch under the spinner before it could answer `VaultLocked`.
        let cache = VaultCache::new(crate::test_vault::unreachable_bridge());

        // A fresh process: era 0, never populated. The era MATCHES, so only
        // the populated flag distinguishes this -- and it must read as
        // "fetch one", not "give up".
        let era_before_any_populate = cache.epoch().era();
        assert_eq!(
            cache.snapshot_unless_superseded(era_before_any_populate).unwrap_err(),
            VaultUnavailable::Unpopulated
        );

        let epoch = cache.epoch();
        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch
            ),
            PopulateOutcome::Populated
        );
        let era = cache.epoch().era();
        cache.clear();
        assert_eq!(
            cache.snapshot_unless_superseded(era).unwrap_err(),
            VaultUnavailable::Superseded,
            "a cleared cache must not report the caller's era as merely unfilled -- populating \
             would refill it for a DIFFERENT vault session"
        );
    }

    #[test]
    fn snapshot_unless_superseded_cannot_hand_back_two_eras_at_once() {
        // REVIEW 26'S IMPORTANT 2. The repair a reader reaches for is a
        // checked ITEMS read plus a bare `cache.folders()`. That is two lock
        // acquisitions with a `clear` window between them, and it LOOKS
        // checked. This test exhibits the tear in that spelling and then
        // shows the combined door cannot produce the same pair.
        //
        // Since review 28's Important 1 there is no items-only door to spell
        // the first half with, so it is spelled as the projection that door
        // WAS (`.map(|s| s.items)`) -- which is the shape any future caller
        // tempted to re-add one would write, and therefore still the tear
        // being exhibited.
        let cache = seeded_offline_cache();
        let era = cache.epoch().era();

        // The two-call spelling, with the `clear` landing in the window it
        // leaves open. Scripted rather than raced, so it is deterministic.
        let torn_items = cache
            .snapshot_unless_superseded(era)
            .map(|snapshot| snapshot.items)
            .expect("the era is still current at this point");
        cache.clear();
        let torn_folders = cache.folders();
        assert!(
            !torn_items.is_empty() && torn_folders.len() != 1,
            "the two-call spelling is supposed to be able to tear -- if it cannot, this test no \
             longer demonstrates anything"
        );

        // One observation, both facts: there is no era for which this hands
        // back the pre-clear items, and no era for which it hands back
        // post-clear folders alongside them.
        assert_eq!(
            cache.snapshot_unless_superseded(era).unwrap_err(),
            VaultUnavailable::Superseded
        );
        assert_eq!(
            cache.snapshot_unless_superseded(cache.epoch().era()).unwrap_err(),
            VaultUnavailable::Unpopulated
        );
    }

    #[test]
    fn a_clear_from_inside_a_response_handler_refuses_both_halves_together() {
        // The same deterministic interleaving the epoch-guard test above
        // uses: the `clear()` fires from inside the mocked folders response
        // handler, so it lands strictly after the populate began fetching and
        // strictly before it tries to write. Neither half of the pair may
        // survive it for the pre-clear era.
        let mut server = crate::test_http::server();
        let cache = std::sync::Arc::new(cache_for(server.url()));
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let cache_for_handler = cache.clone();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |_| {
                cache_for_handler.clear();
                folders_body().as_bytes().to_vec()
            })
            .create();

        let era = cache.epoch().era();
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::DiscardedStale);

        assert_eq!(
            cache.snapshot_unless_superseded(era).unwrap_err(),
            VaultUnavailable::Superseded,
            "a populate discarded by a mid-flight clear must not leave EITHER half readable for \
             the era that asked"
        );
        assert!(cache.items().is_empty());
        assert!(cache.folders().is_empty());
    }

    #[test]
    fn set_app_match_reports_a_write_the_snapshot_could_not_take() {
        // REVIEW 26'S MINOR 2. Reachable with no `clear` at all: a tray Sync
        // is in flight (the "Add app..." handler does not gate on
        // `backend_task_in_progress`), its `populate_with` lands while the
        // user is in `run_picker`, and its fetch legitimately lacks the item.
        // The server took the match; the snapshot did not, and no pending
        // entry was recorded either, so no later populate replays it.
        // Returning a bare `Ok(())` told the caller it had been written
        // through.
        //
        // THE PATH, CORRECTED IN REVIEW 30 (Important 2). This comment used to
        // narrate the interleaving below as "our PUT raced a remote delete",
        // which the same commit's `AppMatchWrite` doc proves cannot produce
        // `ServerOnly`: a delete landing FIRST makes the PUT return `Err`. The
        // delete has to come after the server accepted the write, and what
        // this test actually scripts is the consequence -- a populate whose
        // fetch no longer holds the id writing back BEFORE `set_app_match`'s
        // own `self.lock()`, the statement immediately after the PUT returns.
        // The whole window is that response-return latency. See
        // `AppMatchWrite::ServerOnly`.
        let mut server = crate::test_http::server();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();

        let cache = cache_for(server.url());
        let item = VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert_eq!(
            cache.populate_with(vec![item.clone()], cache.epoch()).unwrap(),
            PopulateOutcome::Populated
        );
        assert_eq!(
            cache.set_app_match(&item, &an_app_match()).unwrap(),
            AppMatchWrite::WroteThrough
        );

        // The sync's populate lands with a fetch that no longer holds the id.
        let mark = cache.epoch();
        assert_eq!(
            cache.populate_with(vec![], mark).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(cache.items().is_empty());

        assert_eq!(
            cache.set_app_match(&item, &an_app_match()).unwrap(),
            AppMatchWrite::ServerOnly,
            "a save the snapshot could not take must not be reported as a write-through -- the \
             caller arms its match engine from that snapshot"
        );
    }

    #[test]
    fn set_app_match_reports_an_unpopulated_cache_as_server_only_too() {
        let mut server = crate::test_http::server();
        let _u = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();
        let cache = cache_for(server.url());
        let item = VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(!cache.is_populated());
        assert_eq!(
            cache.set_app_match(&item, &an_app_match()).unwrap(),
            AppMatchWrite::ServerOnly
        );
    }

    #[test]
    fn the_replay_log_line_carries_an_ordering_fact_of_its_own() {
        // REVIEW 26'S MINOR 1. The line is emitted OUTSIDE the lock (review
        // 25's Minor 4), so two concurrent populates can serialise their
        // snapshot updates in one order and their log lines in the other --
        // and "which populate landed first" is the whole question this line
        // exists to answer in a post-mortem of a lost write. The sequence
        // number is allocated under the lock, so it orders them even when the
        // lines do not.
        let line = replay_log_line(7, &["a".to_string()], &["f1".to_string()]);
        assert!(line.contains("populate #7"), "got: {line}");
        assert!(line.contains("item(s) a; folder(s) f1"), "got: {line}");
        assert!(
            line.to_lowercase().contains("as this populate left it"),
            "the line must not be readable as a claim about the snapshot as it stands now: {line}"
        );
    }

    #[test]
    fn every_populate_that_reaches_the_write_back_gets_a_distinct_sequence_number() {
        let mut server = crate::test_http::server();
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
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        // A discarded populate consumes one too: it reached the lock, and a
        // post-mortem that cannot place it against the ones that landed is
        // missing the populate most likely to explain a lost write.
        let stale = cache.epoch();
        cache.clear();
        assert_eq!(
            cache.populate_with(vec![], stale).unwrap(),
            PopulateOutcome::DiscardedStale
        );
        assert_eq!(
            cache.populate_sequence(),
            3,
            "each populate that took the write-back lock must have taken a number"
        );
    }

    // --- Trash -------------------------------------------------------------

    /// The trash as `bw serve` answers `?trash=true`: one item, carrying a
    /// `deletedDate` and NOT among the two in `items_body`.
    fn trash_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"t1","name":"Old thing","fields":[],"type":1,
             "deletedDate":"2026-07-30T09:15:00.000Z"}
        ]}}"#
    }

    /// A populated cache holding `items_body`'s two live items and one
    /// folder. Same shape as `populated_cache_with_a_filed_item` above, with
    /// one addition that matters: the live-items mock states
    /// `Matcher::Missing`, so it can only answer the *unqualified* list. The
    /// `?trash=true` request each test registers separately therefore cannot
    /// be served by this mock by accident, which is what would let a
    /// `list_trash` that forgot its query silently pass here.
    fn populated_cache(server: &mut mockito::Server) -> VaultCache {
        server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Missing)
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
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        cache
    }

    /// The single trashed item, fetched through the cache.
    fn the_trashed_item(cache: &VaultCache) -> VaultItem {
        cache.list_trash().unwrap().into_iter().next().unwrap()
    }

    #[test]
    fn listing_the_trash_does_not_disturb_the_live_snapshot() {
        // The whole of decision (a) in one assertion: trash is fetched, never
        // stored. If it were merged into `items` the Trash view would leak
        // deleted rows into every live list in the app -- the sidebar, the
        // item list and `app::handle_match`'s autofill candidates all read
        // `items`.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();

        let trashed = cache.list_trash().unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, "t1");

        let live = cache.items();
        assert_eq!(live.len(), 2, "the trash fetch changed the live snapshot: {live:?}");
        assert!(
            !live.iter().any(|i| i.id == "t1"),
            "a trashed item leaked into the live snapshot"
        );
    }

    #[test]
    fn an_on_demand_list_is_handed_back_while_its_era_still_stands() {
        // The half that must keep working. `list_trash_unless_superseded` is
        // a guard, and a guard that refused everything would satisfy the
        // test below on its own.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();

        let era = cache.epoch().era();
        let fetched = cache
            .list_trash_unless_superseded(era)
            .expect("the fetch succeeded")
            .expect("nothing superseded this era");
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].id, "t1");
    }

    #[test]
    fn an_on_demand_list_fetched_across_a_clear_is_discarded() {
        // **The account-A/account-B hole, for the on-demand lists.** The
        // vault window's only guard on these fetches was `load_generation`,
        // which is a SPAWN TAG: it is incremented when a vault load is
        // spawned and `clear` does not touch it. So a Trash fetch outstanding
        // across a lock-and-reauth into a different account came back
        // carrying the generation it left with, matched, and was applied --
        // account B's trashed item names under account A's chrome.
        //
        // `window_era` was introduced (review 29) to close exactly this for
        // the live load, and its doc explicitly refuses to rest on "every
        // production `clear` runs on the main thread". This path re-imposed
        // that reliance until the era check reached it too.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();

        // The era the window captured when it opened, before the vault
        // underneath it was replaced.
        let era = cache.epoch().era();
        cache.clear();

        assert!(
            cache.list_trash_unless_superseded(era).expect("the fetch itself succeeded").is_none(),
            "a trash list fetched against a vault session that has since been cleared \
             was handed back anyway"
        );
        // ...and repopulating for the NEW account does not make the old era
        // applicable again. This is the cross-account case, and it is the
        // whole reason the era rather than a populate count is the question.
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
        assert!(
            cache.list_trash_unless_superseded(era).expect("the fetch itself succeeded").is_none(),
            "a repopulate under a new account revived a superseded era"
        );
        // The control: the era the cache is in NOW is still served, so the
        // refusals above are about the era and not about `clear` having
        // broken the door outright.
        assert!(
            cache
                .list_trash_unless_superseded(cache.epoch().era())
                .expect("the fetch itself succeeded")
                .is_some(),
            "the current era's fetch was refused too -- the guard refuses everything"
        );
    }

    #[test]
    fn a_superseded_on_demand_list_reports_no_error_even_when_the_fetch_failed() {
        // The error describes a vault this window has already left, so
        // surfacing it would put "Trash could not be read" in front of a user
        // whose session was merely replaced. The era check runs after the
        // fetch and swallows both outcomes alike.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(500)
            .create();

        let era = cache.epoch().era();
        cache.clear();
        assert!(
            cache.list_trash_unless_superseded(era).expect("a superseded fetch reports no error").is_none()
        );
    }

    #[test]
    fn a_failed_on_demand_list_in_its_own_era_is_still_an_error() {
        // The control for the test above: swallowing errors is right ONLY
        // because the era moved. In the ordinary case the failure has to
        // reach the band, which is what `AuxLoadError` and the inline notice
        // are for.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(500)
            .create();

        assert!(
            cache.list_trash_unless_superseded(cache.epoch().era()).is_err(),
            "a failed fetch in the current era was reported as a successful non-answer"
        );
    }

    #[test]
    fn the_archive_list_is_guarded_by_the_same_era_check() {
        // The two lists go through one private helper precisely so they
        // cannot come to disagree about when a result is still current --
        // asserted rather than left to the shared call.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _a = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();

        assert!(
            cache
                .list_archive_unless_superseded(cache.epoch().era())
                .expect("the fetch succeeded")
                .is_some(),
            "the current era's archive fetch was refused"
        );

        let era = cache.epoch().era();
        cache.clear();
        assert!(
            cache.list_archive_unless_superseded(era).expect("the fetch itself succeeded").is_none(),
            "an archive list fetched against a cleared vault session was handed back"
        );
    }

    #[test]
    fn a_successful_restore_puts_the_item_into_the_live_snapshot() {
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();
        let restore = server
            .mock("POST", "/restore/item/t1")
            .with_status(200)
            .expect(1)
            .create();

        let item = the_trashed_item(&cache);
        cache.restore_item(&item).unwrap();
        restore.assert();

        let live = cache.items();
        assert_eq!(live.len(), 3, "a restored item did not reach the live snapshot: {live:?}");
        let back = live.iter().find(|i| i.id == "t1").expect("the restored item");
        assert_eq!(back.name, "Old thing");
    }

    #[test]
    fn a_restored_item_no_longer_claims_a_deletion_date() {
        // `deletedDate` rides the catch-all, and the catch-all is serialized
        // on every write this app makes. A restored item left carrying it
        // means the vault window's next ordinary edit PUTs a deletion date at
        // a backend whose handling of one is UNVERIFIED. Dropped at the one
        // place the item crosses from trash to live.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();

        let item = the_trashed_item(&cache);
        assert_eq!(
            crate::vault_bridge::deleted_date(&item),
            Some("2026-07-30T09:15:00.000Z"),
            "the premise: the item arrived from the trash carrying a deletion date"
        );

        cache.restore_item(&item).unwrap();
        let back = cache.items().into_iter().find(|i| i.id == "t1").unwrap();
        assert_eq!(
            crate::vault_bridge::deleted_date(&back),
            None,
            "the live snapshot holds an item that still claims to be deleted"
        );
    }

    #[test]
    fn a_failed_restore_leaves_the_snapshot_alone() {
        // The Trash view will report the failure and keep the row in the
        // trash, so a rejected restore must not come back `Ok` and must not
        // put the item into the live list.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body())
            .create();
        let _r = server.mock("POST", "/restore/item/t1").with_status(500).create();

        let item = the_trashed_item(&cache);
        assert!(cache.restore_item(&item).is_err(), "a rejected restore came back Ok");
        let live = cache.items();
        assert_eq!(live.len(), 2, "a failed restore added the item anyway: {live:?}");
        assert!(!live.iter().any(|i| i.id == "t1"));
    }

    // --- The third stale-token path ----------------------------------------
    //
    // `fba91ff` fixed `update_item`, `move_item_to_folder` and `set_favorite`
    // by adopting the copy the server answered with. `POST /restore/item/{id}`
    // answers with nothing this crate has verified, so restore and unarchive
    // stored the CALLER's copy -- token and all -- and the token in it is the
    // one the item had BEFORE the restore. See `current_revision_of`.

    const TOKEN_BEFORE: &str = "2026-07-30T09:15:00.000Z";
    const TOKEN_AFTER: &str = "2026-08-03T11:47:19.101Z";

    /// The trash, with a revision token on the item -- which the real backend
    /// puts on every item and `trash_body` above simply does not model.
    fn trash_body_with_a_token() -> String {
        format!(
            r#"{{"success":true,"data":{{"data":[
                {{"id":"t1","name":"Old thing","fields":[],"type":1,
                 "deletedDate":"{TOKEN_BEFORE}","revisionDate":"{TOKEN_BEFORE}"}}
            ]}}}}"#
        )
    }

    fn mock_trash_with_a_token(server: &mut mockito::Server) -> mockito::Mock {
        server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(trash_body_with_a_token())
            .create()
    }

    /// `GET /object/item/t1` answering the way a backend mid-settle would:
    /// the NEW token, on an otherwise **pre-restore** copy -- still trashed,
    /// and under a name the caller's copy does not have. Deliberately hostile,
    /// because `current_revision_of` must take one key off this and nothing
    /// else.
    fn mock_read_back_mid_settle(server: &mut mockito::Server) -> mockito::Mock {
        server
            .mock("GET", "/object/item/t1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":
                    {{"id":"t1","name":"NOT the caller's name","fields":[],"type":1,
                     "deletedDate":"{TOKEN_BEFORE}","revisionDate":"{TOKEN_AFTER}"}}
                }}"#
            ))
            .create()
    }

    fn revision_of(item: &VaultItem) -> Option<&str> {
        item.other.get("revisionDate").and_then(|v| v.as_str())
    }

    #[test]
    fn a_restore_leaves_the_snapshot_holding_the_token_the_backend_reports() {
        // THE REVIEWER'S PROBE, resolved. It passed against the old code:
        // after `cache.restore_item` the snapshot still held the PRE-restore
        // `revisionDate`, so the next write of that item was refused with the
        // 400 in `vault_bridge`'s `REVISION_DATE_KEY` -- which is the user's
        // report ("shows as faved in folder but not in original client")
        // exactly, one door along from where `fba91ff` closed it.
        //
        // Reverting `current_revision_of` to `item.clone()` gives
        //     the snapshot kept the pre-restore revision token
        //     left: Some("2026-07-30T09:15:00.000Z")
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = mock_trash_with_a_token(&mut server);
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let _g = mock_read_back_mid_settle(&mut server);

        let item = the_trashed_item(&cache);
        assert_eq!(
            revision_of(&item),
            Some(TOKEN_BEFORE),
            "the premise: the trashed copy the caller holds carries the old token"
        );

        let restored = cache.restore_item(&item).unwrap();

        assert_eq!(
            revision_of(&restored),
            Some(TOKEN_AFTER),
            "the caller was handed the pre-restore revision token"
        );
        let back = cache.items().into_iter().find(|i| i.id == "t1").expect("the restored item");
        assert_eq!(
            revision_of(&back),
            Some(TOKEN_AFTER),
            "the snapshot kept the pre-restore revision token"
        );
    }

    #[test]
    fn the_read_back_contributes_the_token_and_nothing_else() {
        // The positive control for the test above, and the one that pins the
        // decision rather than the outcome: `current_revision_of` could have
        // been "swap in the server's copy", which satisfies the token
        // assertion and reinstates `deletedDate` on a live item -- the exact
        // key `without_deleted_date` exists to keep out, at a backend whose
        // handling of it is unverified.
        //
        // Replacing `with_revision_date_from(item, &server)` with `server`
        // gives
        //     the read-back's whole copy was swapped in, not just its token
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = mock_trash_with_a_token(&mut server);
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let _g = mock_read_back_mid_settle(&mut server);

        let restored = cache.restore_item(&the_trashed_item(&cache)).unwrap();

        assert_eq!(
            restored.name, "Old thing",
            "the read-back's whole copy was swapped in, not just its token"
        );
        assert_eq!(
            crate::vault_bridge::deleted_date(&restored),
            None,
            "a restored item was put back into the live snapshot still claiming a deletion date"
        );
    }

    #[test]
    fn after_a_restore_the_next_write_carries_the_new_token() {
        // The consequence, on the wire. The PUT mock answers ONLY a body
        // carrying the post-restore token; mockito returns 501 for anything
        // else, so a `set_favorite` built from a stale copy comes back `Err`.
        // This is `bw serve`'s optimistic-concurrency check, modelled.
        //
        // Reverting `current_revision_of` to `item.clone()` gives
        //     favouriting a just-restored item was refused
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = mock_trash_with_a_token(&mut server);
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let _g = mock_read_back_mid_settle(&mut server);
        let _p = server
            .mock("PUT", "/object/item/t1")
            .match_body(mockito::Matcher::Regex(format!("revisionDate\":\"{TOKEN_AFTER}")))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":
                    {{"id":"t1","name":"Old thing","fields":[],"type":1,"favorite":true,
                     "revisionDate":"{TOKEN_AFTER}"}}
                }}"#
            ))
            .create();

        let restored = cache.restore_item(&the_trashed_item(&cache)).unwrap();

        assert!(
            cache.set_favorite(&restored, true).is_ok(),
            "favouriting a just-restored item was refused -- the write carried a token the \
             backend had already superseded, which is the reported bug"
        );
    }

    #[test]
    fn the_pre_restore_token_is_not_what_goes_back_on_the_wire() {
        // The other half of the test above: it proves a write is accepted when
        // the mock demands the NEW token, and this proves one is refused when
        // the mock demands the OLD one. Together they say the token on the
        // wire really is the read-back's, rather than that this app now sends
        // some token or none.
        //
        // Reverting `current_revision_of` to `item.clone()` makes this PASS
        // the write and fail here with
        //     the stale pre-restore token is still what this app sends
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = mock_trash_with_a_token(&mut server);
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let _g = mock_read_back_mid_settle(&mut server);
        let _p = server
            .mock("PUT", "/object/item/t1")
            .match_body(mockito::Matcher::Regex(format!("revisionDate\":\"{TOKEN_BEFORE}")))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"t1","name":"Old thing","fields":[]}}"#)
            .create();

        let restored = cache.restore_item(&the_trashed_item(&cache)).unwrap();

        assert!(
            cache.set_favorite(&restored, true).is_err(),
            "the stale pre-restore token is still what this app sends"
        );
    }

    #[test]
    fn a_read_back_that_fails_still_leaves_the_restore_successful() {
        // The write already landed on the server, so a failed read-back cannot
        // turn it into an error -- that would tell the user their restore
        // failed when the item is sitting live in their vault. It falls back
        // to the caller's copy, which is precisely the behaviour this replaced,
        // and says so in the log.
        //
        // Making `current_revision_of`'s `Err` arm return the error instead
        // gives a panic on the `unwrap` below, with the 501 in it.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = mock_trash_with_a_token(&mut server);
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let _g = server.mock("GET", "/object/item/t1").with_status(500).create();

        let restored = cache.restore_item(&the_trashed_item(&cache)).unwrap();

        assert_eq!(revision_of(&restored), Some(TOKEN_BEFORE));
        assert_eq!(
            crate::vault_bridge::deleted_date(&restored),
            None,
            "the fallback must still be the RESTORED shape, not the raw trashed one"
        );
        let back = cache.items().into_iter().find(|i| i.id == "t1").expect("the restored item");
        assert_eq!(back.name, "Old thing", "a failed read-back must not cost the write-through");
    }

    #[test]
    fn a_restore_overrides_the_pending_delete_that_trashed_the_item() {
        // THE ONE PLACE THIS FEATURE CAN BE WRONG, walked deterministically.
        //
        // Trashing an item IS `delete_item`, which records `deleted: true`
        // against its id. `record_write` is last-write-wins per id, so unless
        // the restore records its own entry that delete survives -- and
        // `replay_writes` then strips the id out of EVERY later fetch whose
        // mark predates it, for the rest of the session. The item is live on
        // the server, visibly restored in the UI, and gone again at the next
        // sync, permanently, until a lock.
        //
        // The ordering is the whole test: mark BEFORE the delete, so the
        // replayed window covers both the delete and the restore, which is
        // exactly the window a sync started before the user opened Trash
        // occupies.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _d = server.mock("DELETE", "/object/item/1").with_status(200).create();
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"data":{"data":[
                    {"id":"1","name":"Alpha","fields":[],"type":1,
                     "deletedDate":"2026-07-30T09:15:00.000Z"}
                ]}}"#,
            )
            .create();
        let _r = server.mock("POST", "/restore/item/1").with_status(200).create();

        // A sync that fetched the vault while item 1 was still live.
        let mark = cache.epoch();
        let fetched = cache.items();
        assert!(fetched.iter().any(|i| i.id == "1"), "the fetch must predate the delete");

        cache.delete_item("1").unwrap();
        assert!(!cache.items().iter().any(|i| i.id == "1"), "the delete must have landed");

        let trashed = the_trashed_item(&cache);
        cache.restore_item(&trashed).unwrap();
        assert_eq!(
            cache.epoch().era(),
            mark.era(),
            "a restore must not start a new era -- that would supersede every reader holding one"
        );

        // Now the slow sync lands with its pre-delete fetch.
        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(
            cache.items().iter().any(|i| i.id == "1"),
            "a populate stripped a RESTORED item back out, because the restore left the \
             pending delete in place: {:?}",
            cache.items()
        );
    }

    #[test]
    fn a_successful_purge_survives_a_populate_whose_fetch_predates_it() {
        // The mirror of the test above, and the reason purge records a delete
        // even though a trashed item is normally absent from the live
        // snapshot: what the record has to survive is not the snapshot but a
        // FETCH taken while the item was still live. Without it, the purge is
        // undone in the cache and the vault window shows a row for an item
        // that no longer exists anywhere.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let purge = server
            .mock("DELETE", "/object/item/1")
            .match_query(mockito::Matcher::Exact("permanent=true".into()))
            .with_status(200)
            .expect(1)
            .create();

        let mark = cache.epoch();
        let fetched = cache.items();
        assert!(fetched.iter().any(|i| i.id == "1"), "the fetch must predate the purge");

        cache.purge_item("1").unwrap();
        purge.assert();
        assert!(
            !cache.items().iter().any(|i| i.id == "1"),
            "a purge left the item in the live snapshot"
        );

        assert_eq!(
            cache.populate_with(fetched, mark).unwrap(),
            PopulateOutcome::Populated
        );
        assert!(
            !cache.items().iter().any(|i| i.id == "1"),
            "a populate resurrected a permanently deleted item: {:?}",
            cache.items()
        );
    }

    #[test]
    fn a_failed_purge_leaves_the_snapshot_alone() {
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _p = server
            .mock("DELETE", "/object/item/1")
            .match_query(mockito::Matcher::Exact("permanent=true".into()))
            .with_status(500)
            .create();

        assert!(cache.purge_item("1").is_err(), "a rejected purge came back Ok");
        assert!(
            cache.items().iter().any(|i| i.id == "1"),
            "a failed purge removed the item from the cache anyway"
        );
    }

    // --- Archive -----------------------------------------------------------

    /// The archive as `bw serve` answers `?archived=true`: one item that is
    /// NOT among the two in `items_body` and carries NO `deletedDate` -- an
    /// archived item's keys are an ordinary item's (measured).
    fn archive_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"a1","name":"Put aside","fields":[],"type":1,
             "login":{"username":"u","password":"p"}}
        ]}}"#
    }

    fn mock_archive_list(server: &mut mockito::Server) {
        server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(archive_body())
            .create();
    }

    #[test]
    fn listing_the_archive_does_not_disturb_the_live_snapshot() {
        // The same property the trash list has, asserted for the same reason:
        // an archived item that leaked into `items` would reappear in the
        // item list, the match engine and autofill -- the three consumers
        // whose exclusion this app gets for free precisely BECAUSE the
        // archive is a separate query.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        mock_archive_list(&mut server);

        let archived = cache.list_archive().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "a1");

        let live = cache.items();
        assert_eq!(live.len(), 2, "the archive fetch changed the live snapshot: {live:?}");
        assert!(!live.iter().any(|i| i.id == "a1"), "an archived item leaked into the snapshot");
    }

    #[test]
    fn archiving_an_item_takes_it_out_of_the_live_snapshot() {
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let archive = server.mock("POST", "/archive/item/1").with_status(200).expect(1).create();

        let item = cache.items().into_iter().find(|i| i.id == "1").expect("the premise");
        cache.archive_item(&item).unwrap();
        archive.assert();

        let live = cache.items();
        assert_eq!(live.len(), 1, "archiving did not remove the item: {live:?}");
        assert!(!live.iter().any(|i| i.id == "1"));
        // POSITIVE CONTROL: the OTHER item is untouched, so this cannot pass
        // against an `archive_item` that emptied the snapshot.
        assert!(live.iter().any(|i| i.id == "2"));
    }

    #[test]
    fn a_failed_archive_leaves_the_snapshot_alone() {
        // Re-archiving an already-archived item is a 400 on the live backend,
        // so this path is reachable. Removing the item locally on a write the
        // server refused would hide an item the vault still has.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _a = server.mock("POST", "/archive/item/1").with_status(400).create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(cache.archive_item(&item).is_err(), "a rejected archive came back Ok");
        assert!(
            cache.items().iter().any(|i| i.id == "1"),
            "a failed archive removed the item from the cache anyway"
        );
    }

    #[test]
    fn an_archive_survives_a_populate_whose_fetch_predates_it() {
        // The pending-write log is the whole reason `archive_item` records
        // anything at all. Without it, a sync started before the user
        // archived lands afterwards carrying the item as live, and the
        // archive is silently undone in this process.
        //
        // THE WINDOW THAT HAPPENS IN IS NOT A TIMER. An earlier version of
        // this comment called it "a window the vault window's 30s auto-sync
        // makes ordinary"; there is no 30s auto-sync and there never was.
        // The vault window syncs once, on its first frame (`auto_synced`),
        // and otherwise only when the user clicks the Sync pill -- which
        // `vault_window::mod`'s pill comment states outright. The window is
        // opened by those two events, and by nothing else.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _a = server.mock("POST", "/archive/item/1").with_status(200).create();

        let mark = cache.epoch();
        let fetched = cache.items();
        assert!(fetched.iter().any(|i| i.id == "1"), "the fetch must predate the archive");

        let item = fetched.iter().find(|i| i.id == "1").unwrap().clone();
        cache.archive_item(&item).unwrap();

        assert_eq!(cache.populate_with(fetched, mark).unwrap(), PopulateOutcome::Populated);
        assert!(
            !cache.items().iter().any(|i| i.id == "1"),
            "a populate resurrected an archived item: {:?}",
            cache.items()
        );
    }

    #[test]
    fn unarchiving_puts_the_item_back_and_overrides_the_pending_archive() {
        // Both halves of the round trip, in one walk, because the second is
        // invisible without the first: `archive_item` leaves a
        // `deleted: true` entry in the pending-write log, and if the
        // unarchive does not overwrite it `replay_writes` strips the item out
        // of EVERY later fetch -- the item comes back on the server and stays
        // invisible here for the rest of the session.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _a = server.mock("POST", "/archive/item/1").with_status(200).create();
        let unarchive = server.mock("POST", "/restore/item/1").with_status(200).expect(1).create();

        let mark = cache.epoch();
        let fetched = cache.items();
        let item = fetched.iter().find(|i| i.id == "1").unwrap().clone();

        cache.archive_item(&item).unwrap();
        assert!(!cache.items().iter().any(|i| i.id == "1"), "the archive must have landed");

        cache.unarchive_item(&item).unwrap();
        unarchive.assert();
        assert!(
            cache.items().iter().any(|i| i.id == "1"),
            "an unarchived item did not reach the live snapshot: {:?}",
            cache.items()
        );

        // ...and a fetch that predates the whole sequence no longer has the
        // item stripped out of it.
        assert_eq!(cache.populate_with(fetched, mark).unwrap(), PopulateOutcome::Populated);
        assert!(
            cache.items().iter().any(|i| i.id == "1"),
            "a populate stripped an UNARCHIVED item back out, because the unarchive left the \
             pending archive in place: {:?}",
            cache.items()
        );
    }

    #[test]
    fn a_failed_unarchive_leaves_the_snapshot_alone() {
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        mock_archive_list(&mut server);
        let _u = server.mock("POST", "/restore/item/a1").with_status(500).create();

        let item = cache.list_archive().unwrap().into_iter().next().unwrap();
        assert!(cache.unarchive_item(&item).is_err(), "a rejected unarchive came back Ok");
        assert!(
            !cache.items().iter().any(|i| i.id == "a1"),
            "a failed unarchive put the item into the live snapshot anyway"
        );
    }

    #[test]
    fn an_unarchive_leaves_the_snapshot_holding_the_token_the_backend_reports() {
        // The same defect as the restore's, and the same fix: an unarchive
        // goes down the very same `POST /restore/item/{id}` route (see
        // `VaultBridge::unarchive_item`), so it too stored the caller's copy
        // and the caller's token. Asserted separately rather than trusted to
        // the restore's test, because they are two functions and only one of
        // them was ever fixed by hand.
        //
        // Reverting `current_revision_of` to `item.clone()` gives
        //     the snapshot kept the pre-unarchive revision token
        //     left: Some("2026-07-30T09:15:00.000Z")
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":{{"data":[
                    {{"id":"a1","name":"Put aside","fields":[],"type":1,
                     "revisionDate":"{TOKEN_BEFORE}"}}
                ]}}}}"#
            ))
            .create();
        let _u = server.mock("POST", "/restore/item/a1").with_status(200).create();
        let _g = server
            .mock("GET", "/object/item/a1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":
                    {{"id":"a1","name":"NOT the caller's name","fields":[],"type":1,
                     "revisionDate":"{TOKEN_AFTER}"}}
                }}"#
            ))
            .create();

        let item = cache.list_archive().unwrap().into_iter().next().unwrap();
        assert_eq!(revision_of(&item), Some(TOKEN_BEFORE), "the premise");

        let unarchived = cache.unarchive_item(&item).unwrap();

        assert_eq!(
            revision_of(&unarchived),
            Some(TOKEN_AFTER),
            "the caller was handed the pre-unarchive revision token"
        );
        let back = cache.items().into_iter().find(|i| i.id == "a1").expect("the unarchived item");
        assert_eq!(
            revision_of(&back),
            Some(TOKEN_AFTER),
            "the snapshot kept the pre-unarchive revision token"
        );
        // POSITIVE CONTROL, the same one the restore's has: only the token is
        // taken off the read-back. Swapping the whole copy in would pass every
        // assertion above and rename the user's item.
        assert_eq!(
            back.name, "Put aside",
            "the read-back's whole copy was swapped in, not just its token"
        );
    }

    #[test]
    fn a_401_on_an_archive_call_reaches_the_caller_as_unauthorized() {
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(401)
            .create();
        let _a = server.mock("POST", "/archive/item/1").with_status(401).create();
        let _u = server.mock("POST", "/restore/item/1").with_status(401).create();

        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(matches!(cache.list_archive(), Err(VaultError::Unauthorized)));
        assert!(matches!(cache.archive_item(&item), Err(VaultError::Unauthorized)));
        assert!(matches!(cache.unarchive_item(&item), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn a_401_on_a_trash_call_reaches_the_caller_as_unauthorized() {
        // The cache must not flatten the variant the vault window's re-auth
        // path keys off. Nothing here maps errors -- they ride `?` -- so this
        // pins that nobody adds a `map_err` later.
        let mut server = crate::test_http::server();
        let cache = populated_cache(&mut server);
        let _t = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(401)
            .create();
        let _p = server
            .mock("DELETE", "/object/item/1")
            .match_query(mockito::Matcher::Exact("permanent=true".into()))
            .with_status(401)
            .create();
        let _r = server.mock("POST", "/restore/item/1").with_status(401).create();

        assert!(matches!(cache.list_trash(), Err(VaultError::Unauthorized)));
        assert!(matches!(cache.purge_item("1"), Err(VaultError::Unauthorized)));
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        assert!(matches!(cache.restore_item(&item), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn restoring_into_an_unpopulated_cache_is_reported_as_success_and_writes_nothing() {
        // The self-curing miss, pinned so it stays self-curing: no snapshot to
        // write through to, the vault has the item live, and any populate
        // brings it in. It must NOT be an error -- the server accepted the
        // restore -- and it must not mark the cache populated.
        let mut server = crate::test_http::server();
        let _r = server.mock("POST", "/restore/item/t1").with_status(200).create();
        let cache = cache_for(server.url());
        assert!(!cache.is_populated());

        let item: VaultItem = serde_json::from_str(
            r#"{"id":"t1","name":"Old thing","fields":[],"type":1,
                "deletedDate":"2026-07-30T09:15:00.000Z"}"#,
        )
        .unwrap();
        cache.restore_item(&item).unwrap();

        assert!(!cache.is_populated(), "a restore marked an empty cache populated");
        assert!(cache.items().is_empty(), "a restore seeded an unpopulated snapshot");
    }

    /// A cache seeded with one item carrying one custom field whose value is
    /// the probe string, and a folder mock to satisfy `populate_with`.
    ///
    /// The item is built here rather than parsed from a body so the probe is
    /// the value verbatim -- a JSON round trip would allocate and free several
    /// intermediate copies of it, and those frees are exactly what the watch
    /// would then report on.
    fn cache_with_a_probe_custom_field(
        server: &mut mockito::Server,
        field_name: &str,
        field_value: &str,
    ) -> VaultCache {
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        let seeded = vec![VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![crate::vault_bridge::VaultField {
                name: Some(field_name.to_string()),
                value: Some(zeroize::Zeroizing::new(field_value.to_string())),
                other: serde_json::Map::new(),
            }],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }];
        assert_eq!(cache.populate_with(seeded, cache.epoch()).unwrap(), PopulateOutcome::Populated);
        cache
    }

    /// **A custom field's value does not reach the allocator in the clear.**
    ///
    /// This watches copy **A** of the trace in [`crate::vault_bridge::VaultField::value`]'s
    /// doc: `VaultCache::items` hands every caller a clone of the whole
    /// snapshot, and the vault window holds one open while `fill_from_vault`
    /// and `handle_match` make short-lived ones. Before the type change that
    /// clone's `String` went back to the allocator holding a user-typed
    /// secret -- a hidden (`type: 1`) field's PIN, recovery code or security
    /// answer -- on every drop.
    ///
    /// **It is deliberately not the read-only detail pane.** `detail.rs` has
    /// no non-test read of `item.fields` and `key_sequence::field_palette`
    /// reads field *names* only, so a probe hung on the detail pane would
    /// watch a path this value never travels and report clean while blind.
    /// The snapshot clone is where the value actually goes.
    ///
    /// The cache, the server and the item are all built **before** the watch
    /// is armed, so what the watch sees is only what `items()` itself
    /// allocated and released.
    #[test]
    fn a_custom_field_value_does_not_reach_the_allocator_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // **The instrument is awake, in this thread and in this direction.**
        // Without this line a probe that had gone deaf makes the assertion at
        // the bottom pass by saying nothing -- a clean report from a blind
        // instrument, which is this codebase's signature failure and the one
        // shape the assertion cannot catch in itself.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe cannot see an unwiped custom-field value, so this test proves nothing"
        );

        let mut server = crate::test_http::server();
        let cache = cache_with_a_probe_custom_field(&mut server, "PIN", PROBE);

        // Read back through a borrow, not a clone, so the positive control
        // below does not itself allocate a copy of the probe.
        let mut carried = false;
        let leaked = plaintext_reached_the_allocator(|| {
            let snapshot = cache.items();
            carried = snapshot
                .iter()
                .flat_map(|i| i.fields.iter())
                .any(|f| f.value.as_ref().map(|v| v.as_str()) == Some(PROBE));
            drop(snapshot);
        });

        // Positive control: the clone really did carry the secret, so a
        // `false` above cannot mean "there was nothing there to leak".
        assert!(carried, "the snapshot clone never held the probe value -- nothing was watched");
        assert!(!leaked, "the snapshot clone freed a custom field's value in the clear");
    }

    /// **A custom field's NAME is still a plain `String`.**
    ///
    /// The guard against over-applying the change. A field name is not a
    /// secret -- it is `PIN`, `Recovery code`, `deskwarden:app-match` -- and
    /// wrapping it too would buy nothing while making every `.as_deref()`
    /// read in the crate (including `main.rs`'s) pay for a wipe it does not
    /// need.
    ///
    /// Two assertions, because either alone is weak. The binding pins the
    /// *type*: `Option<String>` will not accept an `Option<Zeroizing<String>>`
    /// and the test stops compiling if the name is wrapped. The probe pins the
    /// *behaviour*: a name equal to the probe string goes back to the
    /// allocator unwiped, which is only true while it is a plain `String`.
    #[test]
    fn a_custom_field_name_is_still_a_plain_string() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe is deaf, so neither assertion below means anything"
        );

        let mut server = crate::test_http::server();
        let cache = cache_with_a_probe_custom_field(&mut server, PROBE, "4821");

        let snapshot = cache.items();
        // The type pin. Annotated on purpose: this is the assertion, and it
        // is checked by the compiler rather than at run time.
        let name: Option<String> = snapshot[0].fields[0].name.clone();
        assert_eq!(name.as_deref(), Some(PROBE), "the fixture's field name is not the probe");
        drop(name);
        drop(snapshot);

        let mut carried = false;
        let leaked = plaintext_reached_the_allocator(|| {
            let snapshot = cache.items();
            carried = snapshot
                .iter()
                .flat_map(|i| i.fields.iter())
                .any(|f| f.name.as_deref() == Some(PROBE));
            drop(snapshot);
        });

        assert!(carried, "the snapshot clone never held the probe name -- nothing was watched");
        assert!(
            leaked,
            "a custom field's NAME is being zeroized: the change over-applied, and every reader \
             of `f.name` now pays for a wipe a field label does not need"
        );
    }
    // -- the optional encrypted file --------------------------------------
    //
    // **No `mockito` anywhere below, deliberately.** Every one of these is
    // about what reaches the *disk*, so the bridge is
    // `test_vault::unreachable_bridge` -- which carries a free assertion:
    // a persist path that went to the network instead of to the snapshot
    // fails visibly. See `test_vault`'s module doc for what pooled mockito
    // servers cost the tests that did not need them.

    use crate::vault_disk_cache::tests::{cache_with_key, temp_dir_for};
    use crate::vault_disk_cache::DiskCacheLoad;

    /// A cache over its own scratch directory, with the Hello step already
    /// satisfied by the substituted key -- which is the state a real session
    /// is in after the startup load or after the toggle was switched on.
    fn cache_with_disk(name: &str, enabled: bool) -> (VaultCache, std::path::PathBuf) {
        let dir = temp_dir_for(name);
        let disk = cache_with_key(&dir);
        let cache = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            disk,
            "fp".to_string(),
            enabled,
        );
        (cache, dir)
    }

    /// Seeds `cache` with the `items_body` vault the way a populate would,
    /// through the one write-back every populate uses.
    ///
    /// It asserts that the populate landed rather than returning the
    /// outcome: a fixture that silently seeded nothing is how a test passes
    /// for the wrong reason, and every caller below wants the same answer.
    fn seed(cache: &VaultCache) {
        let epoch = cache.epoch();
        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch,
            ),
            PopulateOutcome::Populated,
            "the fixture cache did not actually get populated"
        );
    }

    fn cache_file(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("vault-cache.bin")
    }

    #[test]
    fn with_the_setting_off_no_file_is_ever_created() {
        // **Asserted on the filesystem, not on a flag.** "Off by default" has
        // to mean nothing is written, not that a boolean says so -- and the
        // directory is read back whole, so a file under any name counts.
        let (cache, dir) = cache_with_disk("disabled", false);
        seed(&cache);
        let item = cache.items().into_iter().next().unwrap();
        let _ = cache.set_favorite(&item, true);
        let _ = cache.delete_item("1");
        cache.clear();

        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            entries.is_empty(),
            "a file was created with the setting off: {entries:?}"
        );
    }

    #[test]
    fn a_cache_with_no_disk_at_all_is_inert_rather_than_a_panic() {
        // `VaultCache::new` is what every other fixture in this suite builds.
        // Each of these has to be a no-op on it, not an unwrap on `None`.
        let cache = crate::test_vault::cache_with_items(body_list(items_body()));
        assert!(cache.disk_cache_path().is_none());
        assert!(matches!(cache.load_from_disk(), DiskCacheLoad::Absent));
        assert!(cache.loaded_from_disk_at().is_none());
        assert!(cache.forget_disk_copy().is_ok());
        assert!(cache.disable_disk_persistence().is_ok());
        assert!(cache.enable_disk_persistence().is_err());
    }

    #[test]
    fn a_successful_populate_writes_the_file() {
        let (cache, dir) = cache_with_disk("populate-writes", true);
        assert!(!cache_file(&dir).exists());
        seed(&cache);
        assert!(cache_file(&dir).exists());
    }

    #[test]
    fn a_failed_populate_does_not_write_a_file() {
        // The bridge is unreachable, so `populate` fails at its first call.
        let (cache, dir) = cache_with_disk("failed-populate", true);
        assert!(cache.populate().is_err());
        assert!(!cache_file(&dir).exists());
    }

    #[test]
    fn a_populate_discarded_as_stale_leaves_no_file_on_disk() {
        // The era guard, one layer down. A populate whose era was bumped
        // mid-flight is the previous vault session's data; persisting it
        // would write the account being left to the disk of the account
        // arriving, which is the very bug the guard exists to stop.
        let (cache, dir) = cache_with_disk("discarded", true);
        let epoch = cache.epoch();
        cache.clear();
        assert_eq!(
            cache.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch,
            ),
            PopulateOutcome::DiscardedStale
        );
        assert!(!cache.is_populated());
        assert!(
            !cache_file(&dir).exists(),
            "a discarded populate wrote its stale snapshot to disk"
        );
    }

    /// **The one test down here that needs a server**, because a mutation is
    /// by definition a write the backend accepted first. Seeding still costs
    /// no round-trip; only the `DELETE` does.
    /// **One item, out of the file, without the snapshot.**
    ///
    /// The version 2 file's whole purpose from this side: a caller that
    /// needs one password gets that one. Asserted against a cache whose

    /// The same read, before the lock: it works, and it does not disturb the
    /// snapshot. Without this the test above would pass on a cache that could

    #[test]
    fn a_successful_mutation_rewrites_the_file() {
        let dir = temp_dir_for("mutation-rewrites");
        let mut server = crate::test_http::server();
        let _d = server
            .mock("DELETE", "/object/item/1")
            .with_status(200)
            .create();
        let cache = VaultCache::with_disk_cache(
            VaultBridge::new(server.url()),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        seed(&cache);
        cache.delete_item("1").unwrap();

        // Reload the file and confirm the deletion is IN it, rather than
        // checking that its mtime moved: a rewrite of the pre-delete snapshot
        // would pass the weaker assertion.
        let reloaded = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        match reloaded.load_from_disk() {
            DiskCacheLoad::Loaded { items, folders, .. } => {
                let ids: Vec<String> = items.into_iter().map(|i| i.id).collect();
                assert_eq!(ids, vec!["2".to_string()]);
                assert_eq!(folders[0].name, "Work");
            }
            other => panic!("expected a loaded snapshot, got {other:?}"),
        }
    }

    #[test]
    fn a_mutation_the_server_refused_does_not_rewrite_the_file() {
        // Every mutating door does its HTTP first, so an unreachable bridge
        // means the snapshot was never touched -- and `persisting` must not
        // write on that path.
        let (cache, dir) = cache_with_disk("failed-mutation", true);
        seed(&cache);
        let before = std::fs::read(cache_file(&dir)).unwrap();

        assert!(cache.delete_item("1").is_err());
        assert_eq!(cache.items().len(), 2, "a refused delete reached the snapshot");
        assert_eq!(
            std::fs::read(cache_file(&dir)).unwrap(),
            before,
            "a refused delete was persisted anyway"
        );
    }

    #[test]
    fn clear_empties_memory_and_leaves_the_file() {
        // The load-bearing lifecycle rule. Lock and quit both call `clear`;
        // if either deleted the file the feature would stop working, since
        // surviving a restart is the entire point.
        let (cache, dir) = cache_with_disk("clear-keeps-file", true);
        seed(&cache);
        cache.clear();

        assert!(cache.items().is_empty());
        assert!(!cache.is_populated());
        assert!(cache_file(&dir).exists(), "clear() deleted the file");
        assert!(
            cache.loaded_from_disk_at().is_none(),
            "clear() left an age describing a snapshot it had just dropped"
        );
    }

    #[test]
    fn forget_disk_copy_deletes_the_file() {
        let (cache, dir) = cache_with_disk("forget", true);
        seed(&cache);
        assert!(cache_file(&dir).exists());
        cache.forget_disk_copy().unwrap();
        assert!(!cache_file(&dir).exists());
    }

    #[test]
    fn disabling_persistence_deletes_the_file_and_stops_writing() {
        let (cache, dir) = cache_with_disk("disable", true);
        seed(&cache);
        assert!(cache_file(&dir).exists());

        cache.disable_disk_persistence().unwrap();
        assert!(!cache_file(&dir).exists());

        seed(&cache);
        assert!(
            !cache_file(&dir).exists(),
            "a populate after disabling wrote the file back"
        );
        let item = cache.items().into_iter().next().unwrap();
        let _ = cache.set_favorite(&item, true);
        assert!(
            !cache_file(&dir).exists(),
            "a mutation after disabling wrote the file back"
        );
    }

    #[test]
    fn enabling_persistence_writes_the_snapshot_already_in_memory() {
        // Turning the setting on is the Hello prompt plus the first write,
        // with no separate confirmation and no wait for the next sync.
        let (cache, dir) = cache_with_disk("enable", false);
        seed(&cache);
        assert!(!cache_file(&dir).exists());
        cache.enable_disk_persistence().unwrap();
        assert!(cache_file(&dir).exists());
    }

    #[test]
    fn loading_from_disk_populates_the_snapshot_and_records_its_age() {
        let (writer, dir) = cache_with_disk("load-populates", true);
        seed(&writer);

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        assert!(!reader.is_populated());
        assert!(matches!(
            reader.load_from_disk(),
            DiskCacheLoad::Loaded { .. }
        ));
        assert!(reader.is_populated());
        assert_eq!(reader.items().len(), 2);
        assert!(reader.loaded_from_disk_at().is_some());
    }

    #[test]
    fn a_restore_does_not_rewrite_the_file_it_just_read() {
        // If it did, `written_at` would be stamped with the current time at
        // every launch, so the seven-day expiry would never fire and the
        // toolbar pill would report a vault that was always "just written"
        // however old it really was.
        let (writer, dir) = cache_with_disk("restore-no-rewrite", true);
        seed(&writer);
        let before = std::fs::read(cache_file(&dir)).unwrap();

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        assert!(matches!(
            reader.load_from_disk(),
            DiskCacheLoad::Loaded { .. }
        ));
        assert_eq!(
            std::fs::read(cache_file(&dir)).unwrap(),
            before,
            "the restore rewrote the file, so its age is now a lie"
        );
    }

    /// **A cancelled prompt leaves a copy that the app can still see.**
    ///
    /// `load_from_disk` answers `Unavailable` and the file stays put, so
    /// nothing in that outcome distinguishes it from a machine with no copy at
    /// all. `disk_copy_awaiting_key` is what the offline screens ask instead.
    #[test]
    fn a_copy_left_by_a_cancelled_prompt_is_still_reported_as_being_there() {
        let (writer, dir) = cache_with_disk("awaiting-key", true);
        seed(&writer);

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            crate::vault_disk_cache::tests::cache_that_declines_hello(&dir),
            "fp".to_string(),
            true,
        );
        assert!(matches!(
            reader.load_from_disk(),
            DiskCacheLoad::Unavailable(_)
        ));
        assert!(!reader.is_populated());
        assert!(
            reader.loaded_from_disk_at().is_none(),
            "an age was recorded for a file nothing read"
        );
        assert!(
            reader.disk_copy_awaiting_key(),
            "the copy is on the disk and the app cannot tell, so the offline screens would \
             tell the user it does not exist"
        );
    }

    /// The same file, with the setting off: inert, and offered to nobody.
    #[test]
    fn a_disabled_cache_offers_no_copy_even_when_a_declined_one_is_there() {
        let (writer, dir) = cache_with_disk("awaiting-key-off", true);
        seed(&writer);

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            crate::vault_disk_cache::tests::cache_that_declines_hello(&dir),
            "fp".to_string(),
            false,
        );
        assert!(matches!(reader.load_from_disk(), DiskCacheLoad::Absent));
        assert!(
            !reader.disk_copy_awaiting_key(),
            "a cache the user turned off offered its leftover file anyway"
        );
    }

    /// **Opening the copy on request does not rewrite it**, which is the same
    /// rule `a_restore_does_not_rewrite_the_file_it_just_read` pins for the
    /// startup path -- asserted again here because `open_disk_copy` is a
    /// second entry into it and a second chance to get it wrong.
    #[test]
    fn opening_the_copy_on_request_still_does_not_stamp_the_file() {
        let (writer, dir) = cache_with_disk("open-no-rewrite", true);
        seed(&writer);
        let before = std::fs::read(cache_file(&dir)).unwrap();

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        assert!(matches!(
            reader.open_disk_copy(),
            DiskCacheLoad::Loaded { .. }
        ));
        assert!(reader.is_populated());
        assert_eq!(
            std::fs::read(cache_file(&dir)).unwrap(),
            before,
            "opening the copy at the user's request rewrote it, so its age is now a lie"
        );
        assert!(reader.loaded_from_disk_at().is_some());
    }

    #[test]
    fn a_rejected_load_leaves_the_cache_unpopulated_and_records_no_age() {
        let (cache, _dir) = cache_with_disk("load-absent", true);
        assert!(matches!(cache.load_from_disk(), DiskCacheLoad::Absent));
        assert!(!cache.is_populated());
        assert!(cache.loaded_from_disk_at().is_none());
    }

    #[test]
    fn a_disabled_cache_reads_nothing_even_when_a_file_is_there() {
        // Somebody turned the setting off by hand in settings.json while a
        // file from a previous enablement was still on disk. Nothing may be
        // read from it.
        let (writer, dir) = cache_with_disk("disabled-read", true);
        seed(&writer);
        assert!(cache_file(&dir).exists());

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            false,
        );
        assert!(matches!(reader.load_from_disk(), DiskCacheLoad::Absent));
        assert!(!reader.is_populated());
    }

    #[test]
    fn a_backend_populate_clears_the_from_disk_age() {
        // Once real data has arrived in this session the snapshot is no
        // longer "loaded from a file written N hours ago", and the toolbar
        // pill must stop saying so.
        let (writer, dir) = cache_with_disk("age-cleared", true);
        seed(&writer);

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        reader.load_from_disk();
        assert!(reader.loaded_from_disk_at().is_some());
        seed(&reader);
        assert!(reader.loaded_from_disk_at().is_none());
    }

    #[test]
    fn a_restore_discarded_as_stale_populates_nothing_and_records_no_age() {
        // The era guard covers the disk path too, because the restore goes
        // through the same write-back: a `clear` between the epoch capture
        // and the write-back means a different vault session, and a locked
        // vault must not be repopulated from a file.
        let (writer, dir) = cache_with_disk("restore-stale", true);
        seed(&writer);

        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&dir),
            "fp".to_string(),
            true,
        );
        // The same shape the guard sees in production: an epoch captured
        // before the read, and a `clear` in between.
        let epoch = reader.epoch();
        reader.clear();
        let vault = match cache_with_key(&dir).load("fp") {
            DiskCacheLoad::Loaded { items, folders, .. } => VaultSnapshot { items, folders },
            other => panic!("expected a loaded snapshot, got {other:?}"),
        };
        assert_eq!(
            reader.populate_with_vault(vault, epoch),
            PopulateOutcome::DiscardedStale
        );
        assert!(!reader.is_populated());
        assert!(reader.loaded_from_disk_at().is_none());
    }

    #[test]
    fn a_repoint_moves_the_file_and_the_fingerprint_together() {
        let (cache, first) = cache_with_disk("repoint-first", true);
        seed(&cache);
        assert!(cache_file(&first).exists());
        let before = std::fs::read(cache_file(&first)).unwrap();

        let second = temp_dir_for("repoint-second");
        cache.repoint_disk_cache(&second, "fp-b".to_string());
        assert!(cache.loaded_from_disk_at().is_none());
        seed(&cache);
        assert!(cache_file(&second).exists());
        assert_eq!(
            std::fs::read(cache_file(&first)).unwrap(),
            before,
            "a write after the switch reached the account being left"
        );

        // And the new file is keyed to the new account: read back under the
        // old fingerprint it is refused, not accepted.
        let stale = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(&second),
            "fp".to_string(),
            true,
        );
        assert!(matches!(
            stale.load_from_disk(),
            DiskCacheLoad::Rejected(crate::vault_disk_cache::RejectReason::ForeignAccount)
        ));
    }

    // -- reconciliation: what the backend's answer does to a restore --------
    //
    // A cache-first launch (`main`'s third startup arm) comes up off
    // `load_from_disk` and starts `bw serve` behind the tray. Some seconds
    // later that backend answers, and its answer is written over a snapshot
    // that came out of a file written hours ago. The claim the design rests
    // on is that this needs no new rules: `load_from_disk` restores through
    // `write_back_at_epoch` and so does every populate, so the second one
    // simply supersedes the first the way any two populates do.
    //
    // That is a claim about shared code, and shared code is exactly what
    // drifts. These hold it as behaviour instead: the same restore, four
    // different backend answers, each asserted on what is left in the
    // snapshot AND on what is left of the from-disk age.
    //
    // No `mockito` and no fetch anywhere: `populate_with_vault` is the
    // fetching populates' own write-back with the round trips removed (see
    // its doc), so writing the backend's answer through it exercises the
    // identical code with the network's flakiness taken out of the
    // observation.

    /// A cache over `dir` restored from a file `seed` already wrote, with a
    /// bridge that can never answer -- the state `main` is in the moment the
    /// cache-first arm runs, before `bw serve` exists.
    ///
    /// It asserts the restore landed and that the age is set, rather than
    /// returning them: every caller below is about what happens *to* a
    /// restore, and a fixture that quietly restored nothing would let them
    /// all pass against an empty cache.
    fn restored_from_disk(dir: &std::path::Path) -> VaultCache {
        let reader = VaultCache::with_disk_cache(
            crate::test_vault::unreachable_bridge(),
            cache_with_key(dir),
            "fp".to_string(),
            true,
        );
        match reader.load_from_disk() {
            DiskCacheLoad::Loaded { .. } => {}
            other => panic!("the fixture did not restore from disk: {other:?}"),
        }
        assert_eq!(reader.items().len(), 2, "control: the restore is the two-item fixture");
        assert!(
            reader.loaded_from_disk_at().is_some(),
            "control: the restore did not record a from-disk age, so the assertions below \
             about that age clearing would pass against a restore that never set one"
        );
        reader
    }

    /// **Edited elsewhere.** Item `1` was renamed on another device. The
    /// backend's answer is newer truth than a file written before the edit,
    /// so it wins outright -- there is no merge, no "keep the local name",
    /// and nothing to prompt about. The user sees the new name the moment
    /// the sync lands, which for a tray launch is a few seconds after the
    /// tray appeared.
    ///
    /// **And the pill goes out.** `Source::Backend` clears
    /// `loaded_from_disk_at`, which is what the toolbar's "Loaded from
    /// cache" age pill reads. That is the honest thing: the vault on screen
    /// is no longer the disk copy. A launch whose pill stayed up after a
    /// successful reconcile would be telling the user their data was hours
    /// old when it had just been refreshed.
    #[test]
    fn a_backend_answer_that_edited_an_item_elsewhere_wins_over_the_restore() {
        let (writer, dir) = cache_with_disk("reconcile-edited", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);
        assert_eq!(restored.items()[0].name, "Alpha", "control: the file's own name");

        // The vault as `bw sync` would have listed it: item 1 renamed.
        let mut fresh = body_list::<VaultItem>(items_body());
        fresh[0].name = "Alpha (renamed on the phone)".to_string();

        let epoch = restored.epoch();
        assert_eq!(
            restored.populate_with_vault(
                VaultSnapshot { items: fresh, folders: body_list(folders_body()) },
                epoch,
            ),
            PopulateOutcome::Populated
        );

        assert_eq!(
            restored.items()[0].name,
            "Alpha (renamed on the phone)",
            "the backend's answer did not land over the restored snapshot; a cache-first \
             launch would show the file's stale copy for the rest of the session, with the \
             sync it just ran reporting success"
        );
        assert!(
            restored.loaded_from_disk_at().is_none(),
            "the from-disk age survived a successful reconcile, so the toolbar pill would \
             still read \"Loaded from cache\" over a vault that has just been refreshed"
        );
    }

    /// **Deleted elsewhere.** The one case where "the backend wins" has teeth:
    /// the answer is a SHORTER list, and the item that is gone is gone by
    /// virtue of not being in it. Nothing has to notice a deletion for it to
    /// take effect -- `write_back_at_epoch` replaces `items` wholesale rather
    /// than merging by id -- which is why an item deleted on another device
    /// cannot survive in a restored snapshot as a fill that types a dead
    /// password.
    #[test]
    fn a_backend_answer_that_deleted_an_item_elsewhere_removes_it_from_the_restore() {
        let (writer, dir) = cache_with_disk("reconcile-deleted", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);

        let fresh: Vec<VaultItem> = body_list::<VaultItem>(items_body())
            .into_iter()
            .filter(|i| i.id != "1")
            .collect();
        assert_eq!(fresh.len(), 1, "control: the fixture answer really dropped one item");

        let epoch = restored.epoch();
        assert_eq!(
            restored.populate_with_vault(
                VaultSnapshot { items: fresh, folders: body_list(folders_body()) },
                epoch,
            ),
            PopulateOutcome::Populated,
            "control: the reconcile was discarded, so the id check below would pass for a \
             reason that has nothing to do with the deletion"
        );

        let ids: Vec<String> = restored.items().into_iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            vec!["2".to_string()],
            "an item deleted on another device survived the reconcile. Autofill reads this \
             snapshot, so it would go on offering -- and typing -- a credential the vault \
             no longer has"
        );
    }

    /// **Emptied elsewhere**, which is the case a merge would get wrong and
    /// this deliberately does not.
    ///
    /// An empty answer is indistinguishable, in the data, from a vault whose
    /// every item was deleted -- and it IS that, when it is one. So it is
    /// applied: the snapshot empties, and `is_populated` stays true, because
    /// "populated with nothing" is a different state from "never populated"
    /// and only the second means the window should still be waiting.
    ///
    /// The alternative -- refusing an empty answer as implausible -- would
    /// mean a user who emptied their vault on purpose kept seeing it, and
    /// filling from it, on this machine forever.
    #[test]
    fn a_backend_answer_that_emptied_the_vault_empties_the_restore() {
        let (writer, dir) = cache_with_disk("reconcile-emptied", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);

        let epoch = restored.epoch();
        assert_eq!(
            restored
                .populate_with_vault(VaultSnapshot { items: Vec::new(), folders: Vec::new() }, epoch),
            PopulateOutcome::Populated,
            "control: an empty answer was DISCARDED rather than applied, so the emptiness \
             below would be the era guard's doing and not the reconcile's"
        );

        assert!(
            restored.items().is_empty(),
            "a vault emptied on another device did not empty here; the restore's items \
             outlived the account that held them"
        );
        assert!(
            restored.is_populated(),
            "the emptied vault reads as NEVER populated, which is the state the vault \
             window paints a spinner for -- an account with no items would wait forever \
             for a load that already happened"
        );
        assert!(restored.loaded_from_disk_at().is_none());
    }

    /// **And the era rule still bites**, which is the half that says the two
    /// populates really are the same mechanism rather than merely ordered.
    ///
    /// A cache-first launch's reconcile runs on a worker thread while the
    /// main thread can lock the vault or switch account -- both `clear` --
    /// so this is not hypothetical here in the way it is for the inert epoch
    /// captures elsewhere in this file. A `clear` between the restore and the
    /// answer begins a new era, and the answer is discarded rather than
    /// repopulating a vault the user just locked.
    #[test]
    fn a_backend_answer_that_lands_after_a_lock_is_discarded_not_written_over_the_restore() {
        let (writer, dir) = cache_with_disk("reconcile-locked", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);

        // The worker captured its epoch before its fetch, as `spawn_sync`
        // does; the main thread locked while that fetch was in flight.
        let epoch = restored.epoch();
        restored.clear();

        assert_eq!(
            restored.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch,
            ),
            PopulateOutcome::DiscardedStale,
            "the reconcile repopulated a vault that had been locked underneath it"
        );
        assert!(!restored.is_populated());
        assert!(
            restored.loaded_from_disk_at().is_none(),
            "a discarded reconcile left a from-disk age behind, so the next window would \
             claim to be showing a disk copy of an empty, locked vault"
        );
    }

    /// **A cache-first launch arms autofill without rewriting the file or
    /// clearing its age.**
    ///
    /// This is `main`'s third startup arm, reduced to the one statement it
    /// consists of. The arm cannot be executed from a test -- `fn main` opens
    /// real windows and never returns -- but what it must not do is a
    /// property of the cache rather than of `main`: reading `items()` back
    /// out to seed the match engine has to leave the file and the age exactly
    /// as `load_from_disk` left them.
    ///
    /// The negative is what makes it worth writing. Had the arm gone through
    /// `arm_autofill_and_seed_cache` -- the obvious thing, and the thing six
    /// other startup sites do -- the cache would have been re-written at
    /// `Source::Backend`, which clears `loaded_from_disk_at` and re-persists.
    /// The vault window would then show no "Loaded from cache" pill and, if
    /// it did, a `written_at` of *now*: a three-hour-old copy presenting
    /// itself as current, on a launch where nothing was fetched at all. So
    /// both halves are asserted, and the file is compared BYTE FOR BYTE
    /// rather than by mtime -- a rewrite of identical plaintext produces
    /// different ciphertext and a fresh `written_at`, and would pass a
    /// weaker check.
    #[test]
    fn a_cache_first_launch_arms_autofill_without_rewriting_the_file() {
        let (writer, dir) = cache_with_disk("cache-first-arms", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);

        let before_bytes = std::fs::read(cache_file(&dir)).unwrap();
        let before_age = restored
            .loaded_from_disk_at()
            .expect("the fixture asserted this is set");

        // `main`'s cache-first arm, in everything that touches the cache:
        // read the restored items back out to build match entries from.
        let items = restored.items();
        assert!(!items.is_empty(), "control: there is something to arm autofill from");

        assert_eq!(
            std::fs::read(cache_file(&dir)).unwrap(),
            before_bytes,
            "arming the match engine rewrote the encrypted file. A launch that fetched \
             nothing has just stamped a fresh `written_at` into it, so the age line will \
             read \"0 minutes old\" over data that is hours old -- and the next launch \
             inherits the lie"
        );
        assert_eq!(
            restored.loaded_from_disk_at(),
            Some(before_age),
            "arming the match engine cleared or moved the from-disk age. That age is what \
             the toolbar's \"Loaded from cache\" pill is; a cache-first launch that lost it \
             would present the disk copy as a live vault"
        );
    }

    /// **What every write does when the backend never comes up at all**, which
    /// is the state a cache-first launch is in for its whole first seconds
    /// and can be in for its whole life -- offline, `bw` broken, the start
    /// wedged past `BACKEND_OP_TIMEOUT`.
    ///
    /// The tray is up, the hotkey is claimed and autofill is filling, all
    /// from the snapshot. Every WRITE, though, is an HTTP round trip to a
    /// backend that is not there. What matters is that they REFUSE rather
    /// than appear to work: each door does its HTTP first and records the
    /// write afterwards, so a refusal returns `Err`, leaves the snapshot
    /// untouched, leaves the file untouched, and -- the part with the longest
    /// tail -- records no pending write, so the reconcile that lands minutes
    /// later has no phantom edit to replay over the backend's answer.
    ///
    /// A silent no-op would be worse than a refusal in exactly the way the
    /// owner said: the user renames an item, sees the rename, and it is gone
    /// at the next launch with nothing having reported anything.
    ///
    /// One door of each shape is exercised -- an edit, a delete, a favourite
    /// and a folder move -- because they are four separate `bridge` calls and
    /// the ordering property is per-door.
    #[test]
    fn with_no_backend_every_write_refuses_and_leaves_the_restore_untouched() {
        let (writer, dir) = cache_with_disk("no-backend-writes", true);
        seed(&writer);
        let restored = restored_from_disk(&dir);

        let before_items = restored.items();
        let before_bytes = std::fs::read(cache_file(&dir)).unwrap();
        let mut edited = before_items[0].clone();
        edited.name = "renamed while offline".to_string();

        assert!(restored.update_item(&edited).is_err(), "an edit reported success offline");
        assert!(restored.delete_item("2").is_err(), "a delete reported success offline");
        assert!(
            restored.set_favorite(&before_items[0], true).is_err(),
            "a favourite reported success offline"
        );
        assert!(
            restored.move_item_to_folder(&before_items[0], Some("f1")).is_err(),
            "a move reported success offline"
        );

        // By id AND name: an id-only comparison would miss the edit, and the
        // edit is the write with the longest tail (`update_item` is the one
        // that PUTs the whole item back).
        let shape = |items: &[VaultItem]| -> Vec<(String, String)> {
            items.iter().map(|i| (i.id.clone(), i.name.clone())).collect()
        };
        assert_eq!(
            shape(&restored.items()),
            shape(&before_items),
            "a refused write reached the snapshot anyway. Autofill reads this, so the user \
             would be typing a password the vault does not have -- and the vault window \
             would show an edit that no server ever accepted"
        );
        assert_eq!(
            std::fs::read(cache_file(&dir)).unwrap(),
            before_bytes,
            "a refused write was persisted, so the next launch restores an edit that never \
             happened and presents it as the vault"
        );

        // The longest tail: nothing was recorded as a pending local write, so
        // when the backend finally does come up its answer is adopted whole
        // rather than having a refused edit replayed back over it.
        let epoch = restored.epoch();
        assert_eq!(
            restored.populate_with_vault(
                VaultSnapshot {
                    items: body_list(items_body()),
                    folders: body_list(folders_body()),
                },
                epoch,
            ),
            PopulateOutcome::Populated,
            "control: the reconcile did not land, so the name below is the restore's own \
             and says nothing about replay"
        );
        assert_eq!(
            restored.items()[0].name,
            "Alpha",
            "a write that was REFUSED was replayed over the backend's answer when the \
             backend finally came up, resurrecting an edit the server never accepted"
        );
    }
}

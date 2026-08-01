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
use crate::vault_bridge::{
    with_app_match, with_folder, without_deleted_date, Folder, NewLoginItem, VaultBridge,
    VaultError, VaultItem,
};
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
    /// `populate_with_at_epoch`). Only `clear` empties them.
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
    /// See [`Self::populate_with_at_epoch`] for the mechanism and for why it
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
            return Ok(PopulateOutcome::DiscardedStale);
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
    pub fn set_app_match(
        &self,
        item: &VaultItem,
        m: &AppMatch,
    ) -> Result<AppMatchWrite, VaultError> {
        self.bridge.set_app_match(item, m)?;
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
                snapshot.items[at] = with_app_match(item, m);
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
    pub fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<(), VaultError> {
        // Bridge call BEFORE `self.lock()`, like every other write here: no
        // lock may be held across HTTP.
        self.bridge.move_item_to_folder(item, folder_id)?;
        // Built from the caller's item rather than reconstructed from the
        // snapshot's copy, so the snapshot holds exactly what was sent -- the
        // same rule `set_app_match` follows with `with_app_match`.
        let moved = with_folder(item, folder_id);
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "moved vault item {} to folder {:?} but the cache holds no snapshot to write it \
                 through to; the vault has it and any populate will bring it in",
                item.id,
                folder_id
            );
            return Ok(());
        }
        match snapshot.items.iter().position(|i| i.id == item.id) {
            Some(at) => {
                snapshot.items[at] = moved;
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

    /// The vault's trash, fetched fresh every time.
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
    pub fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_trash()
    }

    /// Takes `item` out of the trash and puts it back in the live snapshot.
    ///
    /// Takes the whole trashed item, not just its id, for the reason
    /// [`Self::move_item_to_folder`] does: the snapshot then holds what the
    /// caller had rather than a reconstruction, and `POST /restore/item/{id}`
    /// returns nothing this crate has verified the shape of.
    ///
    /// **[`without_deleted_date`] is load-bearing, not tidying.** The item the
    /// caller holds came from [`Self::list_trash`] and carries `deletedDate`
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
    /// Returns `Result<(), VaultError>` rather than an [`AppMatchWrite`]-style
    /// outcome, on the same test that doc sets (the two misses must demand
    /// identical caller behaviour):
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
    pub fn restore_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        // Bridge call BEFORE `self.lock()`, like every other write here: no
        // lock may be held across HTTP.
        self.bridge.restore_item(&item.id)?;
        let restored = without_deleted_date(item);
        let mut snapshot = self.lock();
        if !snapshot.populated {
            drop(snapshot);
            log::warn!(
                "restored vault item {} from the trash but the cache holds no snapshot to write \
                 it through to; the vault has it live and any populate will bring it in",
                item.id
            );
            return Ok(());
        }
        match snapshot.items.iter().position(|i| i.id == item.id) {
            // A restored item is by definition absent from the live snapshot,
            // so `push` is the ordinary arm and `Some` is the unusual one --
            // reachable when an older populate's fetch still carried the item
            // as live. Replacing rather than pushing there is what keeps the
            // snapshot from holding the same id twice.
            Some(at) => snapshot.items[at] = restored,
            None => snapshot.items.push(restored),
        }
        // UNCONDITIONAL, unlike the `position`-guarded writes above: both arms
        // changed the snapshot, and this entry has a stale `deleted: true` to
        // overwrite. See the doc.
        snapshot.note_item_write(&item.id, false);
        Ok(())
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
    /// [`Self::list_trash`] for why that is the point rather than an omission.
    pub fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.purge_item(id)?;
        let mut snapshot = self.lock();
        if snapshot.populated {
            snapshot.items.retain(|i| i.id != id);
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
        let mut server = mockito::Server::new();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = server.mock("PUT", "/object/item/2").with_status(200).create();

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
        let mut server = mockito::Server::new();
        let cache = populated_cache_with_a_filed_item(&mut server);
        // The mock matches ONLY a body that states `folderId` as present and
        // null, so this also pins that the cache routes through
        // `VaultBridge::move_item_to_folder` and not through `update_item` --
        // the latter omits the key and would get a 501 here.
        let _u = server
            .mock("PUT", "/object/item/1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "Alpha",
                "type": 1,
                "fields": [],
                "favorite": false,
                "folderId": null,
            })))
            .with_status(200)
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
        let cache = populated_cache_with_a_filed_item(&mut server);
        let _u = server.mock("PUT", "/object/item/1").with_status(200).create();

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

    #[test]
    fn snapshot_unless_superseded_hands_back_items_and_folders_of_one_era() {
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

        // A fresh process: era 0, never populated. The era MATCHES, so only
        // the populated flag distinguishes this -- and it must read as
        // "fetch one", not "give up".
        let era_before_any_populate = cache.epoch().era();
        assert_eq!(
            cache.snapshot_unless_superseded(era_before_any_populate).unwrap_err(),
            VaultUnavailable::Unpopulated
        );

        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = server.mock("PUT", "/object/item/1").with_status(200).create();

        let cache = cache_for(server.url());
        let item = VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
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
        let mut server = mockito::Server::new();
        let _u = server.mock("PUT", "/object/item/1").with_status(200).create();
        let cache = cache_for(server.url());
        let item = VaultItem {
            id: "1".to_string(),
            name: "Alpha".to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
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
        let mut server = mockito::Server::new();
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
    fn a_successful_restore_puts_the_item_into_the_live_snapshot() {
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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

    #[test]
    fn a_401_on_a_trash_call_reaches_the_caller_as_unauthorized() {
        // The cache must not flatten the variant the vault window's re-auth
        // path keys off. Nothing here maps errors -- they ride `?` -- so this
        // pins that nobody adds a `map_err` later.
        let mut server = mockito::Server::new();
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
        let mut server = mockito::Server::new();
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
}

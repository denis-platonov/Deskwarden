//! The whole-vault breach scan: what it checks, what it costs, and the state
//! machine the Preferences page draws.
//!
//! # The consent question, answered out loud
//!
//! Breach checking is opt-in and off by default, and `PRIVACY.md` says making
//! that call on the user's behalf "is not the developer's decision to make".
//! `vault_window::password_health` therefore performs no lookup in either
//! state of the setting, and says so in its footer.
//!
//! This module does not undo that. What it adds is a **button**, and the rule
//! it works to is:
//!
//! > **The button always scans. The setting governs everything automatic.**
//!
//! Pressing "Scan all passwords now" is the user initiating the request in
//! the same breath as consenting to it -- the identical argument that settled
//! the manual update check ([`crate::update_panel::UpdatePanel::begin_check`],
//! which is deliberately not gated on `Settings::check_for_updates`). A
//! button that silently did nothing because of a pill on another page is the
//! "control that refuses to be clicked" this codebase keeps having to delete,
//! and it is worse here than elsewhere: the user would have asked, in the
//! plainest possible way, and been ignored without being told.
//!
//! What `Settings::check_breaches` still governs is everything this app does
//! *by itself*: the per-item badge on the detail pane, which fires because
//! the user opened an item rather than because they asked about it. Nothing
//! in this module runs on open, on unlock, or on a timer -- see
//! [`ScanPanel::begin_scan`], which has exactly one caller and it is a click.
//!
//! **The page says all of this in words**, because a rule the user has to
//! infer from behaviour is not a rule they have agreed to. See
//! [`SCAN_CONSENT_NOTE`].
//!
//! # Distinct passwords, not items
//!
//! A 1600-item vault is not 1600 requests. [`plan_for`] groups the vault by
//! password first -- the same SHA-256 grouping
//! `vault_window::password_health::report_for` already does, for the same
//! reason and with the same wiping -- and asks about each **distinct**
//! password once. A vault with heavy reuse can be a couple of hundred
//! lookups, and `BreachCache` makes a repeat within one session free.
//!
//! The two numbers are both reported, because "checked 128" on its own is
//! unanswerable: the history and the page say how many distinct passwords
//! were checked *and* how many items those covered.
//!
//! # Failures are first-class and are never folded into anything
//!
//! [`check_prefix`] fails per prefix -- no network, a 503, a body that did
//! not parse. A run that reported "checked 60, found 3" while 40 lookups
//! failed would be a lie the user goes on trusting: they would read the
//! absence of a finding as a clean result for passwords nobody managed to
//! ask about.
//!
//! So: each prefix is retried [`RETRIES`] times before it is given up on, the
//! give-up is **counted**, the count is on screen while the scan runs and
//! after it finishes, it is written to the history, and every password behind
//! a failed lookup shows on the health pane as *unknown* rather than being
//! left out. "Not shown" reading as "safe" is the failure that matters here.
//!
//! # A good citizen of a free public API
//!
//! [`MAX_IN_FLIGHT`] requests at a time, and no more, however large the vault
//! is. Have I Been Pwned's range API is free, unauthenticated and run at
//! somebody else's expense; a scan that opened three hundred sockets would be
//! indistinguishable from an attack on it, and the user would be the one
//! rate-limited. The retries back off rather than hammering.
//!
//! # Where the results live, and when they die
//!
//! In this process, in [`results`], and nowhere else. **They are not
//! persisted.** `scan_history.json` records five counts per run and never
//! which item was found -- see [`crate::scan_history`], where the reason is
//! spelled out -- and that separation is the whole design: the record is
//! durable and says nothing about your items; the per-item findings are
//! useful and die with the session.
//!
//! [`clear`] is called on vault lock, beside `BreachCache::clear`, for the
//! reason that one exists: "item X is breached" is a claim about this user's
//! vault, and a locked vault must not still be answering questions about it.

use crate::breach::{check_prefix, BaseUrl, BreachCheck, BreachStatus, Prefix};
use crate::scan_history::ScanRecord;
use crate::vault_bridge::VaultItem;
use crate::vault_window::password_health::password_of;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;
use zeroize::Zeroizing;

/// How many range lookups may be outstanding at once. See the module docs on
/// being a good citizen: this is a hard ceiling, not a ratio of vault size.
pub const MAX_IN_FLIGHT: usize = 4;

/// How many times a failed prefix is retried before it is given up on and
/// **counted as a failure**. Three attempts in total.
///
/// Two rather than "until it works": a scan that retried forever would hang
/// on an offline machine with a progress bar that never moves, which is the
/// dishonest version of a failure.
pub const RETRIES: u32 = 2;

/// How long a worker waits after its first failed attempt. The second wait is
/// twice this. Small enough that a flaky connection recovers inside one scan,
/// large enough that a rate-limited API is not being hammered.
pub const RETRY_BACKOFF: Duration = Duration::from_millis(400);

/// A worker's pause between finishing one lookup and taking the next.
///
/// With [`MAX_IN_FLIGHT`] workers this is roughly forty requests a second at
/// the ceiling, which is polite for an unauthenticated free API and still
/// clears a two-hundred-password vault in a few seconds.
pub const REQUEST_SPACING: Duration = Duration::from_millis(100);

/// How long the page should wait before drawing again while a scan is out.
///
/// Named here for [`crate::breach::BREACH_POLL_INTERVAL`]'s reason: a channel
/// is not egui input, so nothing about an answer arriving wakes the UI.
pub const SCAN_POLL_INTERVAL: Duration = Duration::from_millis(120);

/// The sentence the Preferences page prints under the button, in both states
/// of the setting.
///
/// **It says the rule rather than leaving the user to discover it.** See the
/// module docs: a button whose behaviour contradicts a pill two rows above it
/// needs to say so where both are on screen at once.
pub const SCAN_CONSENT_NOTE: &str =
    "This button always checks, whatever the setting above says: pressing it is you asking for \
     the check. The setting governs what Deskwarden does on its own -- the breach badge on an \
     item you open -- and nothing here runs on its own, ever.";

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// One distinct password, the item ids it is used by, and the two halves of
/// its hash.
///
/// `ScanTarget` rather than `Target`, and the prefix is load-bearing:
/// `crate::debug_leak_guard` matches type names across the whole crate, so a
/// second `Target` here made `foreground.rs`'s unrelated one -- which does
/// derive `Debug` -- look like a type that could print a password.
///
/// **The password itself is not in here.** It is hashed in [`plan_for`], on
/// the UI thread, and only the [`Prefix`] and the already-`Zeroizing` suffix
/// travel any further -- exactly the boundary `breach::spawn_check` documents
/// and for exactly the same reason. There is deliberately no `Debug`: the
/// suffix narrows a password to one candidate, and
/// `crate::debug_leak_guard` is the reason nothing in this crate derives one
/// over a secret.
struct ScanTarget {
    prefix: Prefix,
    suffix: Zeroizing<String>,
    /// Every item this password is on. **Ids, never names**: an id is already
    /// on screen elsewhere and is what a finding row is resolved by, and a
    /// name in a structure that outlives a frame is one step from a name in a
    /// file.
    item_ids: Vec<String>,
}

/// What a scan is going to do, decided before anything is spawned.
///
/// Held only for the length of a run: [`ScanPanel`] moves it into the worker
/// pool and does not keep a copy. When the last worker finishes, every
/// `Zeroizing` suffix in it has been dropped and wiped.
pub struct ScanPlan {
    targets: Vec<ScanTarget>,
    items_covered: usize,
}

impl ScanPlan {
    /// How many range lookups this scan will make, at most -- one per distinct
    /// password. The number the page counts up to.
    pub fn distinct_passwords(&self) -> usize {
        self.targets.len()
    }

    /// How many vault items those passwords are on.
    pub fn items_covered(&self) -> usize {
        self.items_covered
    }

    /// Whether there is nothing to do. Its own question, because "no
    /// passwords in this vault" and "scanned and found nothing" are different
    /// results and must not be drawn alike -- the same distinction
    /// `password_health::Summary::NothingToCheck` exists for.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// Group the vault by password and hash each distinct one.
///
/// The grouping is [`vault_window::password_health::report_for`]'s, moved
/// nowhere and copied nowhere in spirit: SHA-256 digests in `Zeroizing`
/// buffers, in one local `Vec` that dies with this function, an **index**
/// sort so the sort's own move buffer never receives a digest that nothing
/// wipes, and one linear pass to cut the sorted order into runs. See that
/// module's header for why each of those is not an implementation detail.
///
/// [`password_of`] is that module's gate, borrowed rather than re-decided, so
/// the set of items a scan covers is exactly the set the report is over.
///
/// Vault order is preserved within a group and between groups, so two runs
/// over one vault plan the same work in the same order.
pub fn plan_for(items: &[VaultItem]) -> ScanPlan {
    // (digest, index into `items`). The only password-derived value here, and
    // it dies at the end of this function.
    let mut keyed: Vec<(Zeroizing<[u8; 32]>, usize)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(password) = password_of(item) else { continue };
        let mut digest = Zeroizing::new([0u8; 32]);
        digest.copy_from_slice(Sha256::digest(password.as_bytes()).as_slice());
        keyed.push((digest, index));
    }
    let items_covered = keyed.len();

    // The INDICES are sorted, not `keyed`. See `report_for`: a `Vec::sort_by`
    // moves its elements through a merge buffer, and a digest copied into
    // that buffer is a copy `Zeroizing`'s drop never runs on.
    let mut order: Vec<usize> = (0..keyed.len()).collect();
    order.sort_by(|a, b| {
        keyed[*a]
            .0
            .as_slice()
            .cmp(keyed[*b].0.as_slice())
            .then(keyed[*a].1.cmp(&keyed[*b].1))
    });

    let mut groups: Vec<(usize, Vec<String>)> = Vec::new();
    let mut run_start = 0usize;
    while run_start < order.len() {
        let mut run_end = run_start + 1;
        while run_end < order.len()
            && keyed[order[run_end]].0.as_slice() == keyed[order[run_start]].0.as_slice()
        {
            run_end += 1;
        }
        // **A run of one IS a target here**, unlike in the reuse report where
        // a password used once is not a finding. A password used once is
        // still a password that can be in a breach.
        let first = keyed[order[run_start]].1;
        let ids = order[run_start..run_end]
            .iter()
            .map(|slot| items[keyed[*slot].1].id.clone())
            .collect();
        groups.push((first, ids));
        run_start = run_end;
    }

    // Back into vault order, so the plan is walked the way the vault reads.
    groups.sort_by_key(|(first, _)| *first);

    let targets = groups
        .into_iter()
        .map(|(first, item_ids)| {
            // Hashed HERE, on the caller's thread, before anything is
            // spawned. `split_hash` borrows the password out of the item and
            // makes no heap copy of it; what comes back is the five
            // characters that may leave this machine and the thirty-five
            // that may not, already `Zeroizing`.
            let password = password_of(&items[first]).unwrap_or_default();
            let (prefix, suffix) = crate::breach::split_hash(password);
            ScanTarget { prefix, suffix, item_ids }
        })
        .collect();

    ScanPlan { targets, items_covered }
}

// ---------------------------------------------------------------------------
// The results
// ---------------------------------------------------------------------------

/// What the last scan found, per vault item.
///
/// **In this process only.** Never written to disk; see the module docs and
/// [`crate::scan_history`], which records counts and nothing else.
///
/// A `BreachStatus` per id, and every id a scan asked about is in here --
/// including the ones whose lookup failed, which are
/// [`BreachStatus::Unavailable`]. That is what lets the health pane show a
/// password it could not check as *unknown* instead of omitting it, and the
/// omission is the failure that matters: "not shown" reads as "safe".
///
/// `Debug` is derived, and safely: an id and a count are both already on
/// screen, and `BreachStatus` carries no hash, suffix or password.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanResults {
    by_item: HashMap<String, BreachStatus>,
}

impl ScanResults {
    /// What the last scan said about this item, or `None` if no scan has
    /// covered it.
    ///
    /// `None` and `Some(Unavailable)` are different answers and must not be
    /// collapsed: the first is "nobody has asked", the second is "we asked
    /// and could not find out". A surface that treated them alike would draw
    /// a failed lookup as an unscanned item, which is the softer and wronger
    /// of the two.
    pub fn status_of(&self, item_id: &str) -> Option<BreachStatus> {
        self.by_item.get(item_id).copied()
    }

    /// Whether any scan result is held at all. The page's "no scan yet"
    /// state reads this.
    pub fn is_empty(&self) -> bool {
        self.by_item.is_empty()
    }

    /// How many items are covered.
    pub fn len(&self) -> usize {
        self.by_item.len()
    }

    /// Records `status` against every id in `item_ids`.
    ///
    /// Stored verbatim. An `Unavailable` becomes a stored `Unavailable` and
    /// is never softened into `Safe` on the way in -- the one answer this
    /// feature must never invent.
    /// **`pub`, and named distinctly so a guard can find it.**
    ///
    /// A scan is not the only thing that has to be able to build a
    /// `ScanResults`: `password_health`'s tests assert against the answers
    /// its pane displays, and `examples/ui_preview` photographs a report with
    /// findings on it. Both must go through the SAME function a scan uses --
    /// a hand-assembled fixture would go on passing after this shape changed
    /// -- and `examples/` is a separate crate, so `pub(crate)` is not enough.
    ///
    /// The cost is real and is bounded by a rule rather than by hope:
    /// `only_a_scan_writes_a_finding_in_production` walks this crate's
    /// production halves and fails if anything outside this file calls it. A
    /// finding invented by a draw site would be a badge with nothing behind
    /// it, which is this project's most-repeated defect.
    ///
    /// It is not a door to the network either way. A `ScanResults` has no
    /// channel, no agent and no URL; writing an answer into one is not the
    /// same as getting one.
    pub fn set_status(&mut self, item_ids: &[String], status: BreachStatus) {
        for id in item_ids {
            self.by_item.insert(id.clone(), status);
        }
    }
}

/// The process-wide store the health pane reads.
///
/// A global for [`crate::update_panel::UpdateEnv`]'s reason: the scan is
/// started from Preferences, which is a blocking window in one shell and a
/// modal in another, and the pane that displays the findings is in a third
/// place. Threading a handle through all three would be the parameter list
/// two call sites away from the thing that uses it. The cost -- a global is
/// invisible at the call site -- is paid down by there being exactly three
/// functions that touch it, all here.
fn store() -> &'static RwLock<ScanResults> {
    static RESULTS: OnceLock<RwLock<ScanResults>> = OnceLock::new();
    RESULTS.get_or_init(|| RwLock::new(ScanResults::default()))
}

/// A copy of what the last scan found. Cheap enough per frame: one clone of a
/// map of ids to a `Copy` enum, over a vault the same window already holds
/// whole.
///
/// A poisoned lock reads as **empty**, not as a panic and not as stale data:
/// the only writer is this module, the surfaces render "no scan yet"
/// perfectly well, and a `Safe` invented out of a lock error would be the one
/// unforgivable answer.
pub fn results() -> ScanResults {
    store().read().map(|r| r.clone()).unwrap_or_default()
}

/// Publishes a finished scan's findings.
fn publish(results: ScanResults) {
    if let Ok(mut guard) = store().write() {
        *guard = results;
    }
}

/// Forgets everything the last scan found. **Called on vault lock**, beside
/// `BreachCache::clear`, and for that function's reason: "item X is breached"
/// is a claim about this user's vault, and a locked vault must not still be
/// answering questions about it.
pub fn clear() {
    publish(ScanResults::default());
}

// ---------------------------------------------------------------------------
// The environment
// ---------------------------------------------------------------------------

/// What a scan needs from the process it runs in.
///
/// Installed once by `main.rs` rather than passed down, for the reason
/// [`crate::update_panel::UpdateEnv`] gives: it is a fact about the process,
/// fixed at startup and identical at both shells of the Preferences window.
///
/// The cost is the same one, and is stated the same way: anything that fails
/// to install it gets [`ScanStage::Unavailable`] -- a state the page renders
/// honestly -- rather than a button that silently does nothing.
pub struct ScanEnv {
    /// The vault, as the process currently holds it. A closure because the
    /// scan is started from a window that does not own the vault, and because
    /// a snapshot taken at install time would scan a vault the user has since
    /// edited.
    pub items: Arc<dyn Fn() -> Vec<VaultItem> + Send + Sync>,
    /// The one range lookup a worker makes, injected exactly as
    /// [`crate::breach::BreachCache`]'s is. `main.rs` wires [`live_check`];
    /// no test constructs an env at all.
    pub check: BreachCheck,
    /// Where `scan_history.json` is written, or `None` on a platform with no
    /// resolvable config directory -- in which case the scan runs and is not
    /// recorded, which the page says out loud.
    pub history_path: Option<PathBuf>,
}

static ENV: OnceLock<ScanEnv> = OnceLock::new();

/// Installs the process-wide [`ScanEnv`]. `false` if one was already
/// installed, in which case the new one is dropped and the first stands.
///
/// Called once, early in `main.rs`. **Never by a test**: a `OnceLock` in a
/// shared test process is exactly the hazard of a second test finding a stale
/// one, which is why every test in this module drives the state machine
/// through [`ScanPanel::apply`] instead and touches neither this nor the
/// network.
pub fn install_env(env: ScanEnv) -> bool {
    ENV.set(env).is_ok()
}

/// The installed environment, or `None` where nothing installed one -- the
/// screenshot example (`examples/ui_preview`), and every test.
pub fn env() -> Option<&'static ScanEnv> {
    ENV.get()
}

/// The production lookup: the real Have I Been Pwned range API.
///
/// **The only place in this crate outside `breach.rs` that reaches the
/// network for a password**, and it is named at exactly one call site --
/// `main.rs`'s [`install_env`]. No test constructs it.
pub fn live_check() -> BreachCheck {
    Arc::new(|prefix: &Prefix, suffix: &Zeroizing<String>| {
        check_prefix(
            &BaseUrl::production(),
            prefix,
            suffix,
            &crate::breach::build_agent(),
        )
    })
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// Where a scan is.
///
/// One enum rather than a set of flags, for the reason
/// [`crate::update_panel::UpdateStage`] is one: the states are exclusive, and
/// independent booleans can describe a situation none of them means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanStage {
    /// Nothing asked for yet this session. The page offers the button and
    /// claims nothing about the vault.
    Idle,
    /// A scan is in flight.
    ///
    /// **All four numbers, always.** `found` and `failed` are on screen while
    /// the scan runs, not only at the end: a run that will finish with forty
    /// failures should not look clean for the first thirty seconds of it.
    Running {
        done: usize,
        total: usize,
        found: usize,
        failed: usize,
    },
    /// A scan finished. Carries the record that was written to the history --
    /// the same five numbers, so the panel and the file cannot disagree about
    /// what just happened.
    Finished(ScanRecord),
    /// The vault has no passwords to check at all. Its own state, and not
    /// `Finished` with zeros: "nothing to check" and "checked everything and
    /// found nothing" are different results, and drawing them alike is the
    /// mistake `password_health::Summary::NothingToCheck` exists to avoid.
    NothingToScan,
    /// No [`ScanEnv`] was installed, so this build cannot scan. Not reachable
    /// in the shipped app, and rendered honestly rather than papered over.
    Unavailable,
}

/// What a worker reports.
enum ScanMsg {
    /// One target resolved, after every retry it was going to get. `Pending`
    /// is never sent -- a worker either has an answer or has given up.
    Answer {
        item_ids: Vec<String>,
        status: BreachStatus,
    },
}

/// The Preferences page's scan section, and the only owner of the channel its
/// workers report on.
pub struct ScanPanel {
    stage: ScanStage,
    /// `Some` exactly while a worker may still report. Dropping it is what
    /// makes [`Self::is_busy`] answerable without a second flag to keep in
    /// step -- and it is also what stops a scan started before the window
    /// closed from landing anywhere afterwards.
    rx: Option<Receiver<ScanMsg>>,
    /// Accumulated as answers arrive. Published in one go when the run
    /// finishes, so the health pane never shows a half-scanned vault as
    /// though it were a scanned one.
    pending_results: ScanResults,
    /// The plan's second number, kept for the record written at the end.
    items_covered: usize,
    /// When the run started, UTC. Not read from the clock at the end alone,
    /// because a record's timestamp is "when it finished" and this is only
    /// here so a run with no clock available still produces a monotone
    /// history.
    finished_at: i64,
}

impl Default for ScanPanel {
    fn default() -> Self {
        Self {
            stage: ScanStage::Idle,
            rx: None,
            pending_results: ScanResults::default(),
            items_covered: 0,
            finished_at: 0,
        }
    }
}

impl ScanPanel {
    /// A panel parked in a stage and wired to nothing.
    ///
    /// For `examples/ui_preview`, which renders every state of this page
    /// without a network, a thread or a clock. It **cannot start work**:
    /// there is no receiver, so nothing can arrive, and the flow refuses to
    /// begin without a [`ScanEnv`] that only `main.rs` installs.
    pub fn parked(stage: ScanStage) -> Self {
        Self { stage, ..Self::default() }
    }

    pub fn stage(&self) -> &ScanStage {
        &self.stage
    }

    /// True while a worker may still report. The page uses it to keep asking
    /// for frames: an egui window repaints on input, and a progress line
    /// nobody is typing over would otherwise move only when the mouse does.
    pub fn is_busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Drains everything that has arrived, without ever blocking. Returns
    /// whether the stage changed.
    ///
    /// **This is where the side effects are**, and [`Self::apply`] is where
    /// they are not: when the last answer lands, this writes the history file
    /// and publishes the findings. Keeping the transition arithmetic in a
    /// pure function is what lets the tests below hand this state machine
    /// sequences a real run would produce -- and sequences a real run would
    /// not -- without a socket, a file or a spawn anywhere.
    pub fn pump(&mut self, now_unix_millis: i64) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = self.rx.as_ref() else { return changed };
            match rx.try_recv() {
                Ok(msg) => changed |= self.apply(msg, now_unix_millis),
                Err(TryRecvError::Empty) => return changed,
                // The workers are all gone. Not an error: their outcomes have
                // already been applied, so the receiver is simply retired.
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    return changed;
                }
            }
        }
    }

    /// One message's effect on the stage, and the whole of it. Pure.
    fn apply(&mut self, msg: ScanMsg, now_unix_millis: i64) -> bool {
        let ScanMsg::Answer { item_ids, status } = msg;
        let ScanStage::Running { done, total, found, failed } = self.stage else {
            // An answer that arrives after the run it belongs to is dropped
            // rather than reopening a finished scan -- the same rule
            // `UpdatePanel::apply` applies to a late `Progress`.
            return false;
        };
        self.pending_results.set_status(&item_ids, status);
        let next = ScanStage::Running {
            done: done + 1,
            total,
            found: found + usize::from(matches!(status, BreachStatus::Breached(_))),
            // **`Unavailable` is the ONLY thing counted as a failure**, and
            // it is counted here rather than inferred later from a gap
            // between `done` and `found + safe`. A count derived by
            // subtraction is a count that goes quietly wrong when a fourth
            // status is added.
            failed: failed + usize::from(status == BreachStatus::Unavailable),
        };
        self.stage = next;
        if let ScanStage::Running { done, total, found, failed } = self.stage {
            if done >= total {
                self.finished_at = now_unix_millis;
                let record = ScanRecord {
                    finished_at_unix_millis: now_unix_millis,
                    passwords_checked: total as u32,
                    items_covered: self.items_covered as u32,
                    found: found as u32,
                    failed: failed as u32,
                };
                // **Published in one go**, so the health pane never shows a
                // half-scanned vault as though the scan had covered it.
                publish(std::mem::take(&mut self.pending_results));
                if let Some(path) = env().and_then(|e| e.history_path.as_deref()) {
                    // A history that could not be written is not a scan that
                    // did not happen. The findings are already published; the
                    // record is the durable half and its loss costs five
                    // numbers.
                    let _ = crate::scan_history::append(path, record);
                }
                self.stage = ScanStage::Finished(record);
            }
        }
        true
    }

    /// Starts a scan. **The only thing that starts one, and its only caller
    /// is a click.**
    ///
    /// Nothing here consults `Settings::check_breaches`; see the module docs
    /// for why, and [`SCAN_CONSENT_NOTE`] for where the user is told.
    ///
    /// A no-op while one is already running: two concurrent scans would race
    /// to be the one whose findings are published.
    pub fn begin_scan(&mut self, now_unix_millis: i64) {
        if self.is_busy() {
            return;
        }
        let Some(env) = env() else {
            self.stage = ScanStage::Unavailable;
            return;
        };
        let items = (env.items)();
        let plan = plan_for(&items);
        if plan.is_empty() {
            self.stage = ScanStage::NothingToScan;
            return;
        }
        let total = plan.distinct_passwords();
        self.items_covered = plan.items_covered();
        self.pending_results = ScanResults::default();
        self.finished_at = now_unix_millis;

        let (tx, rx) = mpsc::channel();
        // A shared queue rather than one thread per target: the ceiling on
        // outstanding requests is the whole politeness policy, and a
        // thread-per-target design has no ceiling at all.
        let queue = Arc::new(Mutex::new(plan.targets));
        for _ in 0..MAX_IN_FLIGHT.min(total) {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let check = Arc::clone(&env.check);
            std::thread::spawn(move || loop {
                // The lock is held only across the pop. A worker that held it
                // across its request would make `MAX_IN_FLIGHT` mean one.
                let Some(target) = queue.lock().ok().and_then(|mut q| q.pop()) else {
                    return;
                };
                let status = resolve(&check, &target.prefix, &target.suffix);
                // The receiver is gone if the window closed mid-scan. That is
                // a normal way for this to end, not an error, and it is also
                // what stops the answer landing in a later session.
                if tx
                    .send(ScanMsg::Answer { item_ids: target.item_ids, status })
                    .is_err()
                {
                    return;
                }
                std::thread::sleep(REQUEST_SPACING);
            });
        }
        // The panel's own clone is dropped, so the channel disconnects when
        // the last worker exits and `pump` can retire the receiver.
        drop(tx);
        self.rx = Some(rx);
        self.stage = ScanStage::Running { done: 0, total, found: 0, failed: 0 };
    }
}

/// One target, with retries. Returns the answer, or [`BreachStatus::Unavailable`]
/// once every attempt has been spent.
///
/// **A retry is only for `Unavailable`.** `Safe` and `Breached` are answers;
/// asking again would be asking a question that has been answered, at the
/// expense of an API somebody else pays for.
fn resolve(check: &BreachCheck, prefix: &Prefix, suffix: &Zeroizing<String>) -> BreachStatus {
    for attempt in 0..=RETRIES {
        let status = check(prefix, suffix);
        if status != BreachStatus::Unavailable {
            return status;
        }
        if attempt < RETRIES {
            // Backs off rather than hammering: the commonest cause of a
            // failure here is the far end asking to be left alone.
            std::thread::sleep(RETRY_BACKOFF * (attempt + 1));
        }
    }
    BreachStatus::Unavailable
}

/// The one line a finished or running scan says about itself.
///
/// Pure, so the wording can be asserted without a rendered frame, and so the
/// four numbers are put into words in exactly one place -- the page, the
/// history list and the tests all read this.
///
/// **The failure count is in the sentence whenever it is non-zero, and it is
/// the last thing said**, because it is the qualifier on everything before
/// it. A sentence that ended on "3 found" with forty failures unmentioned is
/// the lie this whole feature is arranged against.
pub fn outcome_wording(record: &ScanRecord) -> String {
    let passwords = plural(record.passwords_checked, "password", "passwords");
    let items = plural(record.items_covered, "item", "items");
    let head = format!(
        "Checked {} {passwords} across {} {items}.",
        record.passwords_checked, record.items_covered
    );
    let found = match record.found {
        0 => " None was found in a breach.".to_string(),
        1 => " 1 was found in a known breach.".to_string(),
        n => format!(" {n} were found in known breaches."),
    };
    let failed = match record.failed {
        0 => String::new(),
        1 => " 1 could not be checked, so nothing is known about it.".to_string(),
        n => format!(" {n} could not be checked, so nothing is known about them."),
    };
    format!("{head}{found}{failed}")
}

/// The progress line, while a scan is running.
pub fn progress_wording(done: usize, total: usize, found: usize, failed: usize) -> String {
    let mut text = format!("Checked {done} of {total}. {found} found so far");
    if failed > 0 {
        text.push_str(&format!(", {failed} could not be checked"));
    }
    text.push('.');
    text
}

fn plural(count: u32, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::LoginData;

    /// **No test in this module installs a `ScanEnv`, constructs
    /// `live_check`, or opens a socket.** The state machine is driven through
    /// `apply`, which is why it is a separate function from `pump`.
    /// An item with nothing on it -- the shape `password_health`'s own
    /// fixtures use, spelled out field by field because `VaultItem` has no
    /// `Default` and deliberately so.
    fn bare(id: &str, item_type: i64) -> VaultItem {
        VaultItem {
            id: id.into(),
            name: format!("item {id}"),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(item_type),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn login(id: &str, password: Option<&str>) -> VaultItem {
        let mut item = bare(id, 1);
        item.login = Some(LoginData {
            username: Some("user".into()),
            password: password.map(|p| Zeroizing::new(p.to_string())),
            totp: None,
            uris: vec![],
            other: serde_json::Map::new(),
        });
        item
    }

    /// A card: no `login` object at all, so nothing about it is a password.
    fn card(id: &str) -> VaultItem {
        bare(id, 3)
    }

    fn answer(ids: &[&str], status: BreachStatus) -> ScanMsg {
        ScanMsg::Answer {
            item_ids: ids.iter().map(|s| s.to_string()).collect(),
            status,
        }
    }

    /// **Held by every test that drives a panel to completion.**
    ///
    /// Finishing a run publishes into the one process-wide store, and the
    /// tests in this binary run in parallel -- so two of them finishing at
    /// once would each read the other's findings. A lock is the honest fix:
    /// the store really is shared, and hiding that behind a `cfg(test)` seam
    /// would be testing a different program from the one that ships.
    static STORE: Mutex<()> = Mutex::new(());

    /// Takes the store lock, ignoring poisoning: a panicking test has already
    /// failed, and a poisoned lock must not turn one failure into every later
    /// test failing behind it.
    fn store_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
        // Emptied on the way IN, not on the way out: a test that panicked
        // holding this lock would otherwise leave its findings behind for the
        // next one to read as its own.
        clear();
        guard
    }

    fn running(total: usize, items_covered: usize) -> ScanPanel {
        ScanPanel {
            stage: ScanStage::Running { done: 0, total, found: 0, failed: 0 },
            items_covered,
            ..ScanPanel::default()
        }
    }

    // -- the plan ----------------------------------------------------------

    /// **The whole cost argument, as a test.** Four items, two passwords.
    #[test]
    fn the_plan_is_one_lookup_per_distinct_password_not_per_item() {
        let items = vec![
            login("a", Some("reused-one")),
            login("b", Some("only-here")),
            login("c", Some("reused-one")),
            login("d", Some("reused-one")),
        ];
        let plan = plan_for(&items);
        assert_eq!(
            plan.distinct_passwords(),
            2,
            "a vault with heavy reuse would be scanned item by item, which is the request \
             volume this design exists to avoid"
        );
        assert_eq!(plan.items_covered(), 4);
    }

    /// A password used once is still a password that can be in a breach --
    /// the opposite of the reuse report, where a run of one is not a finding.
    #[test]
    fn a_password_used_once_is_still_scanned() {
        let plan = plan_for(&[login("a", Some("unique"))]);
        assert_eq!(plan.distinct_passwords(), 1);
        assert_eq!(plan.items_covered(), 1);
    }

    /// The gate is `password_health::password_of`, borrowed rather than
    /// re-decided, so a scan covers exactly the set that report is over.
    #[test]
    fn items_with_no_password_are_not_scanned() {
        let items = vec![
            card("card"),
            login("empty", Some("")),
            login("absent", None),
            login("real", Some("something")),
        ];
        let plan = plan_for(&items);
        assert_eq!(plan.distinct_passwords(), 1);
        assert_eq!(plan.items_covered(), 1, "a card was counted as a password to check");
    }

    #[test]
    fn a_vault_with_no_passwords_plans_nothing() {
        assert!(plan_for(&[card("a"), card("b")]).is_empty());
        assert!(plan_for(&[]).is_empty());
    }

    /// Two runs over one vault plan the same work in the same order, so a
    /// scan is reproducible and a progress count means the same thing twice.
    #[test]
    fn the_plan_is_in_vault_order_and_is_stable() {
        let items = vec![
            login("a", Some("zzz")),
            login("b", Some("aaa")),
            login("c", Some("zzz")),
        ];
        let first: Vec<Vec<String>> =
            plan_for(&items).targets.iter().map(|t| t.item_ids.clone()).collect();
        let second: Vec<Vec<String>> =
            plan_for(&items).targets.iter().map(|t| t.item_ids.clone()).collect();
        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![vec!["a".to_string(), "c".to_string()], vec!["b".to_string()]],
            "the plan is not in vault order, so the same vault scans in two orders"
        );
    }

    // -- the state machine -------------------------------------------------

    #[test]
    fn a_run_counts_up_and_finishes_on_the_last_answer() {
        let _guard = store_lock();
        let mut panel = running(2, 3);
        assert!(panel.apply(answer(&["a"], BreachStatus::Safe), 1_000));
        assert_eq!(
            *panel.stage(),
            ScanStage::Running { done: 1, total: 2, found: 0, failed: 0 }
        );
        panel.apply(answer(&["b", "c"], BreachStatus::Breached(9)), 2_000);
        let ScanStage::Finished(record) = panel.stage().clone() else {
            panic!("the last answer did not finish the run: {:?}", panel.stage());
        };
        assert_eq!(record.passwords_checked, 2);
        assert_eq!(record.items_covered, 3);
        assert_eq!(record.found, 1);
        assert_eq!(record.failed, 0);
        assert_eq!(record.finished_at_unix_millis, 2_000, "the timestamp is when it finished");
    }

    /// **The lie this feature is arranged against.** A run where most lookups
    /// failed must not report a small number of findings as though the rest
    /// were clean.
    #[test]
    fn a_failed_lookup_is_counted_as_a_failure_and_never_as_safe() {
        let _guard = store_lock();
        let mut panel = running(3, 3);
        panel.apply(answer(&["a"], BreachStatus::Unavailable), 1);
        panel.apply(answer(&["b"], BreachStatus::Unavailable), 2);
        panel.apply(answer(&["c"], BreachStatus::Breached(4)), 3);
        let ScanStage::Finished(record) = panel.stage().clone() else { panic!() };
        assert_eq!(record.failed, 2);
        assert_eq!(record.found, 1);
        assert!(!record.is_complete());
        let words = outcome_wording(&record);
        assert!(
            words.contains("2 could not be checked"),
            "the failures are not in the sentence: {words:?}"
        );
    }

    /// A failed lookup is stored as `Unavailable`, per item, and is NOT
    /// omitted -- which is what lets the health pane show it as unknown.
    /// "Not shown" reading as "safe" is the failure that matters here.
    #[test]
    fn every_item_a_scan_asked_about_ends_up_in_the_results_including_the_failures() {
        let _guard = store_lock();
        let mut panel = running(2, 3);
        panel.apply(answer(&["a", "b"], BreachStatus::Unavailable), 1);
        // Mid-run the findings are NOT yet in the store the pane reads: a
        // half-scanned vault must not be drawn as though it had been scanned.
        assert!(results().is_empty(), "a partial scan was published");
        panel.apply(answer(&["c"], BreachStatus::Safe), 2);

        // Published on completion, in one go.
        let published = results();
        assert_eq!(published.len(), 3);
        assert_eq!(published.status_of("a"), Some(BreachStatus::Unavailable));
        assert_eq!(published.status_of("b"), Some(BreachStatus::Unavailable));
        assert_eq!(published.status_of("c"), Some(BreachStatus::Safe));
        assert_eq!(published.status_of("never-scanned"), None);

        // And a lock forgets them: "item X is breached" is a claim about this
        // user's vault, and a locked vault must not still be answering it.
        clear();
        assert!(results().is_empty(), "the findings survived a lock");
    }

    /// `None` and `Some(Unavailable)` are different answers: "nobody asked"
    /// against "we asked and could not find out".
    #[test]
    fn an_unscanned_item_and_a_failed_one_are_not_the_same_answer() {
        let mut results = ScanResults::default();
        results.set_status(&["asked".to_string()], BreachStatus::Unavailable);
        assert_eq!(results.status_of("asked"), Some(BreachStatus::Unavailable));
        assert_eq!(results.status_of("not-asked"), None);
    }

    /// An answer arriving after the run it belongs to must not reopen a
    /// finished scan.
    #[test]
    fn a_late_answer_is_dropped_rather_than_reopening_a_finished_run() {
        let _guard = store_lock();
        let mut panel = running(1, 1);
        panel.apply(answer(&["a"], BreachStatus::Safe), 10);
        let finished = panel.stage().clone();
        assert!(!panel.apply(answer(&["b"], BreachStatus::Breached(1)), 20));
        assert_eq!(*panel.stage(), finished);
        clear();
    }

    #[test]
    fn an_idle_panel_ignores_an_answer() {
        let mut panel = ScanPanel::default();
        assert!(!panel.apply(answer(&["a"], BreachStatus::Safe), 1));
        assert_eq!(*panel.stage(), ScanStage::Idle);
    }

    #[test]
    fn a_parked_panel_can_never_start_work() {
        let panel = ScanPanel::parked(ScanStage::Running {
            done: 3,
            total: 10,
            found: 1,
            failed: 2,
        });
        assert!(!panel.is_busy(), "a parked panel has no receiver, so nothing can arrive");
    }

    /// Without an installed environment the button reports that it cannot
    /// scan, rather than doing nothing.
    #[test]
    fn with_no_environment_the_button_says_so_rather_than_doing_nothing() {
        let mut panel = ScanPanel::default();
        panel.begin_scan(0);
        assert_eq!(
            *panel.stage(),
            ScanStage::Unavailable,
            "a button that silently does nothing is the control this design exists to delete"
        );
        assert!(!panel.is_busy());
    }

    // -- the retry policy --------------------------------------------------

    /// Retries are for `Unavailable` only, and there are exactly
    /// `RETRIES + 1` attempts.
    #[test]
    fn a_failing_lookup_is_retried_and_then_given_up_on_visibly() {
        let calls = Arc::new(Mutex::new(0usize));
        let seen = Arc::clone(&calls);
        let check: BreachCheck = Arc::new(move |_, _| {
            *seen.lock().unwrap() += 1;
            BreachStatus::Unavailable
        });
        let (prefix, suffix) = crate::breach::split_hash("anything");
        assert_eq!(resolve(&check, &prefix, &suffix), BreachStatus::Unavailable);
        assert_eq!(
            *calls.lock().unwrap(),
            RETRIES as usize + 1,
            "the number of attempts is not RETRIES + 1"
        );
    }

    #[test]
    fn a_lookup_that_recovers_on_a_retry_reports_the_answer() {
        let calls = Arc::new(Mutex::new(0usize));
        let seen = Arc::clone(&calls);
        let check: BreachCheck = Arc::new(move |_, _| {
            let mut n = seen.lock().unwrap();
            *n += 1;
            if *n == 1 {
                BreachStatus::Unavailable
            } else {
                BreachStatus::Breached(7)
            }
        });
        let (prefix, suffix) = crate::breach::split_hash("anything");
        assert_eq!(resolve(&check, &prefix, &suffix), BreachStatus::Breached(7));
        assert_eq!(*calls.lock().unwrap(), 2);
    }

    /// **An answer is not retried.** Asking again about a `Safe` costs the
    /// free API a request for a question that has been answered.
    #[test]
    fn an_answered_lookup_is_asked_exactly_once() {
        for status in [BreachStatus::Safe, BreachStatus::Breached(3)] {
            let calls = Arc::new(Mutex::new(0usize));
            let seen = Arc::clone(&calls);
            let check: BreachCheck = Arc::new(move |_, _| {
                *seen.lock().unwrap() += 1;
                status
            });
            let (prefix, suffix) = crate::breach::split_hash("anything");
            assert_eq!(resolve(&check, &prefix, &suffix), status);
            assert_eq!(*calls.lock().unwrap(), 1, "{status:?} was asked about more than once");
        }
    }

    // -- the wording -------------------------------------------------------

    #[test]
    fn the_outcome_sentence_names_both_numbers_and_ends_on_the_failures() {
        let record = ScanRecord {
            finished_at_unix_millis: 0,
            passwords_checked: 128,
            items_covered: 1_600,
            found: 3,
            failed: 40,
        };
        let words = outcome_wording(&record);
        assert!(words.contains("128 passwords"), "{words:?}");
        assert!(words.contains("1600 items"), "{words:?}");
        assert!(words.contains("3 were found"), "{words:?}");
        assert!(
            words.ends_with("40 could not be checked, so nothing is known about them."),
            "the failures have to be the last thing said, because they qualify everything \
             before them: {words:?}"
        );
    }

    #[test]
    fn a_clean_run_says_so_and_mentions_no_failures() {
        let record = ScanRecord {
            finished_at_unix_millis: 0,
            passwords_checked: 1,
            items_covered: 1,
            found: 0,
            failed: 0,
        };
        let words = outcome_wording(&record);
        assert!(words.contains("1 password across 1 item"), "{words:?}");
        assert!(words.contains("None was found"), "{words:?}");
        assert!(!words.contains("could not be checked"), "{words:?}");
    }

    #[test]
    fn the_progress_line_reports_failures_as_they_happen() {
        assert_eq!(progress_wording(4, 10, 1, 0), "Checked 4 of 10. 1 found so far.");
        assert_eq!(
            progress_wording(4, 10, 1, 2),
            "Checked 4 of 10. 1 found so far, 2 could not be checked.",
            "a run that will end with failures must not look clean while it runs"
        );
    }

    // -- the consent rule --------------------------------------------------

    /// **The decision, pinned as text on the page.** The button always
    /// scans; the setting governs what the app does on its own. A rule the
    /// user has to infer from behaviour is not a rule they agreed to.
    #[test]
    fn the_page_says_that_the_button_ignores_the_setting() {
        assert!(SCAN_CONSENT_NOTE.contains("always checks"), "{SCAN_CONSENT_NOTE:?}");
        assert!(
            SCAN_CONSENT_NOTE.contains("on its own"),
            "the note has to say what the setting DOES govern, or it reads as the setting being \
             pointless: {SCAN_CONSENT_NOTE:?}"
        );
    }

    /// `begin_scan` is the only thing that starts a scan, and nothing schedules
    /// it. A source walk, because "nothing runs on a timer" is a claim about
    /// call sites and not about this function.
    #[test]
    fn nothing_in_this_crate_starts_a_scan_except_a_click() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&root, &mut files);
        assert!(files.len() > 40, "the walk found only {} files", files.len());
        let mut sites = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("source is readable");
            let production = match text.find("#[cfg(test)]") {
                Some(cut) => &text[..cut],
                None => &text[..],
            };
            let n = production.matches(concat!(".begin_", "scan(")).count();
            if n > 0 {
                sites.push((path.file_name().unwrap().to_string_lossy().to_string(), n));
            }
        }
        assert_eq!(
            sites,
            vec![("prefs_ui.rs".to_string(), 1)],
            "a scan is started from somewhere other than the one button on the Preferences \
             page. Opening a report is not consent to hundreds of outbound requests, and \
             neither is unlocking, nor a timer firing"
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// **Only a scan writes a finding in production.**
    ///
    /// `ScanResults::set_status` is `pub` because the paint tests and the
    /// screenshot example -- a separate crate -- have to build the answers
    /// the health pane displays, through the same function a scan uses. This
    /// is the rule that bounds that: nothing outside this file may write a
    /// finding in production code.
    ///
    /// A finding invented at a draw site would be a badge with nothing behind
    /// it, which is this project's most-repeated defect and the one thing a
    /// screen about breached passwords must not do.
    #[test]
    fn only_a_scan_writes_a_finding_in_production() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&root, &mut files);
        assert!(files.len() > 40, "the walk found only {} files", files.len());
        let needle = concat!(".set_", "status(");
        let mut writers = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("source is readable");
            let production = match text.find("#[cfg(test)]") {
                Some(cut) => &text[..cut],
                None => &text[..],
            };
            let n = production.matches(needle).count();
            if n > 0 {
                writers.push((path.file_name().unwrap().to_string_lossy().to_string(), n));
            }
        }
        assert_eq!(
            writers,
            vec![("breach_scan.rs".to_string(), 1)],
            "a finding is written outside the scan that produced it. A badge with nothing              behind it is worse than no badge"
        );
    }

    /// **Nothing in this module logs.** `breach.rs` makes the same
    /// file-scoped claim about itself, and this file is the one that knows
    /// which items came back breached.
    #[test]
    fn the_scan_module_never_logs() {
        let source = include_str!("breach_scan.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        for needle in ["log::", "info!", "warn!", "debug!", "error!", "trace!", "println!", "dbg!"]
        {
            assert!(
                !production.contains(needle),
                "production `breach_scan.rs` writes `{needle}`, and this is the file that knows \
                 which of the user's items came back breached"
            );
        }
    }
}

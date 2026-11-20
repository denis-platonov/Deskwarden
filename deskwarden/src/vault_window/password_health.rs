//! The vault window's **Password health** screen: which of the passwords in
//! this vault are reused, and which are weak. Computed here, on this machine,
//! from the snapshot the window already holds. **Nothing in this file opens a
//! socket, spawns a process or writes a log line.**
//!
//! # Where it lives, and why it is not in Preferences
//!
//! Preferences holds settings. This is data *about the vault's contents*, and
//! a finding the user cannot click through to the item is a dead end -- so it
//! is a **page** of the vault window (spec A 4, like Settings and the
//! Generator): a title, the tabs, and white cards of findings, each with a
//! Change password button. A row opens its item and the button opens that
//! item's editor; [`draw_password_health`] reports which as a
//! [`HealthAction`] and the window applies it through the same door the
//! Shared page's Open record uses. (Through 0.15 this was a column beside a
//! standing detail pane.)
//!
//! It is a **screen**, not a [`super::sidebar::SidebarFilter`], for the same
//! reason `Sends` is not one. A `SidebarFilter` is a per-item predicate
//! (`scope_contains(&self, item)`) evaluated against one item at a time, and
//! **reuse is not a property of an item** -- it is a property of a pair of
//! them. There is no predicate over a single `VaultItem` that can answer it.
//! So: its own page, with group captions that are allowed to be a different
//! height from a row.
//!
//! # The weakness rule, and why it is this one
//!
//! **A password is weak here exactly when [`crate::password_strength::rate`]
//! says [`Strength::Weak`].** That is: fewer than 8 characters, or 8 to 11
//! characters drawn from fewer than three of the four character types.
//!
//! It is deliberately not a rule of this module's own. The detail pane
//! already prints "Strength: Weak" beside the very same password, from that
//! very same function; a second rule here would let this screen list a
//! password the pane calls Fair, or stay silent about one the pane calls
//! Weak, and the user would have no way to tell which of the two was lying.
//! One rule, two surfaces.
//!
//! **There is no score, and that is the point.** A user cannot act on
//! "43/100". What they are shown instead is the two facts that produced the
//! verdict -- the character count and the character types actually present,
//! e.g. *"9 characters, lowercase letters and digits"* -- which names the
//! thing to change. [`crate::password_strength::CharClasses`] is where those
//! types are decided, and `rate` counts the same value, so the explanation
//! cannot drift from the rating it explains.
//!
//! What this rule deliberately does NOT flag: a long single-class password
//! ("horsehorsehorsehorse"). Twenty lowercase characters is a wider search
//! space than eight mixed ones, and flagging it would be crying wolf at the
//! passphrase style this app's own generator can produce.
//!
//! # Items with no password are excluded, not counted as weak
//!
//! A card reported as "weak password" is a bug, not a finding. The gate is
//! [`password_of`]: an item is considered only if it has a `login` object
//! carrying a **non-empty** password. Cards, secure notes, identities, SSH
//! keys, and logins whose password field is empty or absent are all outside
//! the report entirely -- they are not weak, they are not safe, they are not
//! anything, because there is nothing to say about a password that does not
//! exist. See [`HealthReport::with_password`], which is the count the empty
//! state reads.
//!
//! # The grouping key: what it is, and exactly how long it lives
//!
//! Reuse is exact, not heuristic: two items are reused together iff their
//! passwords are byte-for-byte equal. Comparing passwords means holding
//! them, so the comparison is done on **SHA-256 digests, never on the
//! plaintext**.
//!
//! * The key is `Zeroizing<[u8; 32]>` -- the SHA-256 of the password, in a
//!   buffer that is wiped when it drops.
//! * It lives in one local `Vec` inside [`report_for`] and **nowhere else**.
//!   It is not returned, not stored in [`HealthReport`], not put in any
//!   struct that outlives the call, and not cached between frames. When
//!   `report_for` returns, every key has been dropped and wiped.
//! * **There is deliberately no `HashMap<String, Vec<Item>>` of live
//!   passwords.** That is the obvious implementation and it is the one thing
//!   this module must not do: it would hold every password in this vault, in
//!   plaintext, on the heap, for the length of the computation, and free them
//!   unwiped.
//! * The `Vec` is never sorted in place -- an *index* vector is sorted
//!   instead (see [`report_for`]) -- so the sort's own move buffer never
//!   receives a copy of a digest that nothing would wipe.
//!
//! A SHA-256 digest of a password is not the password, but it is a rainbow
//! table's index and it narrows the password to one candidate. It is treated
//! as a secret for that reason: it is wiped, and **it is never formatted,
//! logged, or put in a `Debug`**. Nothing in this file logs at all.
//!
//! # Cost on a large vault
//!
//! [`report_for`] is **O(n log n)** in `n`, the number of items carrying a
//! password: one SHA-256 of a short string per item, then one sort of `n`
//! indices comparing 32 bytes each, then one linear walk over the sorted
//! order to cut it into runs. It is explicitly **not** quadratic -- the naive
//! "compare every password against every other" is O(n^2) comparisons over
//! plaintext, which is both the slow shape and the unsafe one, and it is
//! ruled out by construction here because no two plaintexts are ever compared
//! at all.
//!
//! # Breach data, when the user has not asked for it
//!
//! Breach checking (`crate::breach`, Have I Been Pwned's k-anonymity range
//! API) is **opt-in and off by default**, and `PRIVACY.md` says making that
//! call on the user's behalf "is not the developer's decision to make".
//!
//! So this screen does not make it. It performs **no** breach lookup, in
//! either state of the setting -- not when it is off, and not when it is on.
//! Opening a report over a 1600-item vault is not consent to 1600 outbound
//! requests, and the setting the user actually agreed to governs the
//! per-password badge on the detail pane, one item at a time, when they look
//! at it. What this screen does instead is **say so out loud**: a footer line
//! that names the state of the setting and where the setting is, so
//! "breached" being absent from this report is never mistaken for "none of
//! your passwords are breached". See [`breach_note`].

use super::item_list;
use super::sidebar;
use crate::password_strength::{rate, CharClasses, Strength};
use crate::theme;
use crate::vault_bridge::VaultItem;
use crate::kit::card as kit_card;
use crate::kit::controls::{self, ButtonKind};
use crate::kit::tokens::{self, Role};
use eframe::egui::{self, Margin};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The rail row that opens this screen. Spelled once, so the sidebar and the
/// tests that press it cannot drift apart -- exactly as
/// [`sidebar::SENDS_ROW_LABEL`] is.
pub const HEALTH_ROW_LABEL: &str = "Password health";

/// The pane's own title, at the top of the column.
pub const HEALTH_PANE_TITLE: &str = "Password health";

/// Where the breach setting lives, named the way the user will find it.
///
/// The row's label is spelled here as its own literal because `prefs_ui`'s
/// `BREACH_LABEL` is private to that module. `the_breach_setting_is_named_
/// where_it_actually_is` reads `prefs_ui.rs` off disk and fails if the two
/// ever disagree, so renaming the preference and leaving this sentence
/// behind reds the suite rather than sending the user hunting.
///
/// **The PAGE is checked too, and it was the half that went stale.** The
/// setting moved from General to its own Breaches page -- beside the scan
/// button it does not govern -- and a sentence still naming General would
/// have sent the reader to a page that exists and does not have it on, which
/// is the most confusing of the available wrong answers.
pub const BREACH_SETTING_LOCATION: &str =
    "Preferences > Breaches > \"Check passwords against known breaches\"";

/// One item a finding is about: enough to draw its row and to select it, and
/// **nothing else**.
///
/// Deliberately not a `VaultItem` and not a borrow of one. Both the id and
/// the name are already on screen elsewhere in this window and neither is a
/// secret; carrying the item itself would put a `Zeroizing` password inside
/// the report, which is the long-lived plaintext this module exists to avoid.
/// `Debug` is derived for exactly that reason -- there is nothing here that
/// came from a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundItem {
    pub id: String,
    pub name: String,
}

/// A set of two or more items sharing one password.
///
/// Never one item: a group of one is not reuse, and [`report_for`] does not
/// emit one -- see `a_password_used_once_is_not_a_group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseGroup {
    /// In vault order, so the group reads the same way twice running.
    pub items: Vec<FoundItem>,
}

/// One item whose password [`rate`] calls [`Strength::Weak`], with the two
/// facts that made it so.
///
/// `chars` and `classes` are what the row actually says. They are carried
/// rather than recomputed at the draw site because recomputing means holding
/// the password again, at a second place, for the length of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeakItem {
    pub item: FoundItem,
    /// Characters, not bytes: a password of five emoji is five characters,
    /// and telling the user it is twenty would be telling them something
    /// they cannot check.
    pub chars: usize,
    pub classes: CharClasses,
}

/// A set of items whose passwords are in a breach the same number of times.
///
/// # Grouped by COUNT, and what that does and does not mean
///
/// The count is what Have I Been Pwned reports for a password, so two items
/// in one group either share a password or are two different passwords that
/// happen to have been leaked the same number of times -- and the second is
/// vanishingly unlikely at any count above a handful.
///
/// **This screen does not leave that as an inference.** It already knows the
/// answer: the reuse grouping above is exact, over SHA-256 digests, so
/// [`one_password`](Self::one_password) says whether these items really are
/// one password rather than "almost certainly". A user looking at three rows
/// under one count needs to know whether that is three passwords to change or
/// one, and guessing on their behalf -- either way -- is the thing worth not
/// doing.
///
/// It cross-cuts the reuse section deliberately, and the pane says so: a
/// reused password that is also breached is the most urgent thing in the
/// vault, and appearing in one list only would bury exactly that case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreachGroup {
    /// How many times the password was seen. Never 0 -- see
    /// [`crate::breach::parse_range_body`].
    pub count: u64,
    /// In vault order, so the group reads the same way twice running.
    pub items: Vec<FoundItem>,
    /// Whether these items are **known** to share one password, from the
    /// reuse grouping rather than from the count. `false` on a group of one,
    /// and on the rare group whose members' counts agree by coincidence.
    pub one_password: bool,
}

/// Everything this screen says about one vault, computed once.
///
/// **No `Zeroizing` anywhere in it, by construction**, which is what makes it
/// safe to hold for the length of a frame and safe to derive `Debug` on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HealthReport {
    /// Groups of items sharing a password, ordered by where the group's first
    /// member sits in the vault.
    pub reused: Vec<ReuseGroup>,
    /// Items whose password is weak, in vault order.
    pub weak: Vec<WeakItem>,
    /// Items whose password a scan found in a breach, **by count,
    /// descending** -- the worst first, because that is the reading order a
    /// user wants and because a list sorted by vault position buries the
    /// 40,000-hit password under three that were seen twice.
    ///
    /// Empty when no scan has run. That is not the same as "nothing was
    /// found", and [`Self::scanned`] is what tells them apart.
    /// D5: passwords unchanged for a year or more.
    pub old: Vec<OldItem>,
    pub breached: Vec<BreachGroup>,
    /// Items a scan **asked about and could not find out**, in vault order.
    ///
    /// **Listed, never omitted.** A password nobody could check is not a
    /// password that is fine, and "not shown" reading as "safe" is the
    /// failure that matters on this screen. See
    /// [`crate::breach::BreachStatus`], which has no `Default` for the same
    /// reason.
    pub unknown: Vec<FoundItem>,
    /// Whether a scan has covered any of these items at all.
    ///
    /// The difference between "no breached passwords" and "nobody has
    /// looked", which are opposite claims that an empty list cannot tell
    /// apart. The pane draws two different sentences from this.
    pub scanned: bool,
    /// How many items carried a non-empty password at all -- the denominator
    /// this report is over.
    ///
    /// **The empty state reads this, and that is why it exists.** "No reused
    /// passwords" over a vault of 300 logins is a result; the same words over
    /// a vault of nothing but cards is a non-answer wearing a result's
    /// clothes, and the two must not look alike. See [`Summary`].
    pub with_password: usize,
}

impl HealthReport {
    /// How many distinct items have at least one finding against them -- the
    /// number on the rail's badge.
    ///
    /// Distinct, because a password can be both reused and weak and one item
    /// counted twice would put a badge on the rail that no list on this
    /// screen adds up to.
    pub fn flagged_items(&self) -> usize {
        let mut ids: Vec<&str> = self
            .reused
            .iter()
            .flat_map(|group| group.items.iter())
            .chain(self.weak.iter().map(|weak| &weak.item))
            .chain(self.old.iter().map(|old| &old.item))
            // **Breached and unknown items count too.** The badge is "how
            // many items on this screen need attention", and a breached
            // password that the rail did not count is a finding the user has
            // no reason to go and look for. `unknown` is on the list for the
            // harder version of the same reason: an item nobody could check
            // is an item with an open question against it, and leaving it out
            // of the badge is the "not shown reads as safe" failure moved up
            // one level.
            .chain(self.breached.iter().flat_map(|group| group.items.iter()))
            .chain(self.unknown.iter())
            .map(|found| found.id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// The findings under one filter, as the count the pane's empty line
    /// reads. Pure, so "this filter has nothing in it" can be asserted
    /// without a rendered frame.
    pub fn shown_under(&self, filter: HealthFilter) -> usize {
        match filter {
            HealthFilter::All => self.flagged_items(),
            HealthFilter::Reused => self.reused.iter().map(|g| g.items.len()).sum(),
            HealthFilter::Weak => self.weak.len(),
            HealthFilter::Old => self.old.len(),
            HealthFilter::Breached => {
                self.breached.iter().map(|g| g.items.len()).sum::<usize>() + self.unknown.len()
            }
        }
    }
}

/// Which findings the pane is showing.
///
/// **Only findings are ever listed**, under every filter -- see the module
/// docs. The filter narrows a list of problems; it never turns this screen
/// into an inventory of the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthFilter {
    All,
    Reused,
    Weak,
    /// D5: unchanged for [`OLD_DAYS`] or more.
    Old,
    Breached,
}

impl HealthFilter {
    /// Left to right, in the order the chips are drawn.
    ///
    /// `All` first because it is where the screen opens; `Breached` last
    /// because it is the one that depends on something having been run, and a
    /// chip that is often empty does not belong where the eye lands first.
    pub const ALL: [HealthFilter; 5] = [
        HealthFilter::All,
        HealthFilter::Reused,
        HealthFilter::Weak,
        HealthFilter::Old,
        HealthFilter::Breached,
    ];

    /// The chip's word.
    pub fn label(self) -> &'static str {
        match self {
            HealthFilter::All => "All",
            HealthFilter::Reused => "Reused",
            HealthFilter::Weak => "Weak",
            HealthFilter::Old => "Old",
            HealthFilter::Breached => "Breached",
        }
    }

    /// Whether this filter shows the reuse section.
    fn shows_reused(self) -> bool {
        matches!(self, HealthFilter::All | HealthFilter::Reused)
    }

    fn shows_weak(self) -> bool {
        matches!(self, HealthFilter::All | HealthFilter::Weak)
    }

    fn shows_old(self) -> bool {
        matches!(self, HealthFilter::All | HealthFilter::Old)
    }

    fn shows_breached(self) -> bool {
        matches!(self, HealthFilter::All | HealthFilter::Breached)
    }
}

/// The password this item is judged on, or `None` if it has none.
///
/// **The single gate keeping cards, notes, SSH keys, identities and
/// password-less logins out of the report.** An empty string is `None` here
/// and not a zero-length password: a login whose password field was never
/// filled in has nothing to be weak about, and reporting it would be the
/// "a card is weak" bug in a different hat.
///
/// Returns a borrow. It does not clone, and must not be made to: a clone is a
/// second plaintext copy on the heap, and the only reason this function
/// exists is so that there is exactly one place that decides what counts.
///
/// `pub(crate)` rather than private, and only for [`crate::breach_scan`],
/// which plans a scan over **the same set of items this report is over**. A
/// second "does this item have a password" rule over there would let a scan
/// check an item this report does not list, or skip one it does, and the user
/// would have no way to tell which of the two was lying -- exactly the
/// argument the module docs make for having one weakness rule and two
/// surfaces.
pub(crate) fn password_of(item: &VaultItem) -> Option<&str> {
    item.login
        .as_ref()
        .and_then(|login| login.password.as_ref())
        .map(|password| password.as_str())
        .filter(|password| !password.is_empty())
}

/// The whole report, over the live vault.
///
/// See the module docs for the grouping key's lifetime and for the
/// complexity. In short: one SHA-256 per password into a local `Vec` of
/// `Zeroizing` digests, an index sort, and one linear pass. Nothing here
/// keeps a plaintext password anywhere but in the borrow it reads.
pub fn report_for(items: &[VaultItem]) -> HealthReport {
    report_with_scan(items, &crate::breach_scan::ScanResults::default())
}

/// The report, plus whatever the last scan found.
///
/// # This still makes NO lookup, and the signature is what says so
///
/// `scan` is a **map of answers**, not a handle: `ScanResults` has no channel,
/// no agent, no URL and no way to ask anything. There is no argument to this
/// function through which a request could be made, and there is no `if` in it
/// that a future reader could flip -- which is the point. The module docs
/// argue that opening a report is not consent to hundreds of outbound
/// requests; that argument is now enforced by the type of this parameter
/// rather than by a branch somebody could invert.
///
/// [`report_for`] is the same call with an empty map, kept so the dozens of
/// call sites and tests that have nothing to say about breaches do not have
/// to name one.
///
/// # Where the answers come from
///
/// A scan the user pressed a button for, in
/// `Preferences > Breaches`, held in this process only and dropped on lock.
/// `crate::breach_scan` is where all of that lives.
pub fn report_with_scan(
    items: &[VaultItem],
    scan: &crate::breach_scan::ScanResults,
) -> HealthReport {
    // (digest, index into `items`). The ONLY place a password-derived value
    // lives, and it dies at the end of this function -- see the module docs.
    let mut keyed: Vec<(Zeroizing<[u8; 32]>, usize)> = Vec::new();
    let mut weak = Vec::new();

    for (index, item) in items.iter().enumerate() {
        let Some(password) = password_of(item) else {
            // Not weak, not safe, not counted. See `password_of`.
            continue;
        };
        // Wiped on drop, and filled by copy so the `GenericArray` `Sha256`
        // returns is the only unwiped copy -- it is stack-resident and 32
        // bytes, exactly as the SHA-1 module documents for its own digest.
        let mut digest = Zeroizing::new([0u8; 32]);
        digest.copy_from_slice(Sha256::digest(password.as_bytes()).as_slice());
        keyed.push((digest, index));

        if rate(password) == Strength::Weak {
            weak.push(WeakItem {
                item: found(item),
                chars: password.chars().count(),
                classes: CharClasses::of(password),
            });
        }
    }
    let with_password = keyed.len();

    // **The indices are sorted, not `keyed` itself.** `Vec::sort_by` moves
    // its elements through a merge buffer, and a digest copied into that
    // buffer is a copy nothing wipes -- `Zeroizing`'s drop runs on the
    // element, not on the scratch space it was moved through. Sorting a
    // `Vec<usize>` and looking the digests up leaves every digest where it
    // was allocated, so each is wiped exactly once, on drop, from the one
    // place it ever lived.
    let mut order: Vec<usize> = (0..keyed.len()).collect();
    order.sort_by(|a, b| {
        keyed[*a]
            .0
            .as_slice()
            .cmp(keyed[*b].0.as_slice())
            // Ties broken by vault position, so a group's members come out in
            // the order the vault holds them and two runs of this function
            // over one vault cannot disagree about it.
            .then(keyed[*a].1.cmp(&keyed[*b].1))
    });

    let mut reused = Vec::new();
    let mut run_start = 0usize;
    while run_start < order.len() {
        let mut run_end = run_start + 1;
        while run_end < order.len()
            && keyed[order[run_end]].0.as_slice() == keyed[order[run_start]].0.as_slice()
        {
            run_end += 1;
        }
        // A run of one is a password used once, which is not a finding.
        if run_end - run_start >= 2 {
            reused.push(ReuseGroup {
                items: order[run_start..run_end]
                    .iter()
                    .map(|slot| found(&items[keyed[*slot].1]))
                    .collect(),
            });
        }
        run_start = run_end;
    }

    // Sorted by digest until now, which is an order with no meaning to a
    // reader. Re-ordered by where each group's first member sits in the
    // vault, so the screen reads top-to-bottom the way the vault does. The
    // key is the member's index, which the tie-break above already made the
    // smallest one in the group.
    reused.sort_by_key(|group| {
        group
            .items
            .first()
            .and_then(|first| items.iter().position(|item| item.id == first.id))
            .unwrap_or(usize::MAX)
    });

    // ------------------------------------------------------------------
    // The breach half. Reads `scan` and nothing else -- see this function's
    // doc for why that is a type-level guarantee rather than a convention.
    // ------------------------------------------------------------------
    let mut by_count: Vec<(u64, Vec<FoundItem>)> = Vec::new();
    let mut unknown = Vec::new();
    for item in items {
        if password_of(item).is_none() {
            // Outside the report entirely, exactly as it is for weakness and
            // reuse. A card is not unknown; there is nothing to know.
            continue;
        }
        match scan.status_of(&item.id) {
            Some(crate::breach::BreachStatus::Breached(count)) => {
                match by_count.iter_mut().find(|(seen, _)| *seen == count) {
                    Some((_, group)) => group.push(found(item)),
                    None => by_count.push((count, vec![found(item)])),
                }
            }
            // **Asked, and no answer.** Listed rather than dropped: see
            // `HealthReport::unknown`.
            Some(crate::breach::BreachStatus::Unavailable) => unknown.push(found(item)),
            // `Pending` cannot reach here -- a scan reports only settled
            // answers -- and `None` means this item was not part of any scan,
            // which is not a finding about it. Neither is `Safe`, which is
            // the whole reason this screen lists findings only.
            _ => {}
        }
    }
    // Worst first. A list in vault order buries the 40,000-hit password under
    // three that were seen twice.
    by_count.sort_by_key(|a| std::cmp::Reverse(a.0));

    // **Whether a group really is one password, from the reuse digests rather
    // than from the count.** See `BreachGroup`: the count alone cannot tell
    // "one password on three items" from "three passwords leaked the same
    // number of times", and this screen already knows the answer.
    let breached: Vec<BreachGroup> = by_count
        .into_iter()
        .map(|(count, group)| {
            let ids: std::collections::BTreeSet<&str> =
                group.iter().map(|f| f.id.as_str()).collect();
            let one_password = ids.len() > 1
                && reused.iter().any(|reuse| {
                    let members: std::collections::BTreeSet<&str> =
                        reuse.items.iter().map(|f| f.id.as_str()).collect();
                    ids.iter().all(|id| members.contains(id))
                });
            BreachGroup { count, items: group, one_password }
        })
        .collect();

    HealthReport {
        reused,
        weak,
        breached,
        unknown,
        old: old_items(items, super::detail::today_days()),
        scanned: !scan.is_empty(),
        with_password,
    }
}

/// D5's threshold.
pub const OLD_DAYS: i64 = 365;
pub const OLD_HEADING: &str = "OLD";

/// A password unchanged for a year or more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldItem {
    pub item: FoundItem,
    pub days: i64,
}

/// How many days ago `item`'s password last changed: `passwordRevisionDate`,
/// else the newest password-history entry, else `creationDate` -- the read
/// pane's own rule (`revisionDate` moves on any edit, so it is never asked).
/// `None` when none of them can be read.
pub fn password_age_days(item: &VaultItem, today: i64) -> Option<i64> {
    let login = item.login.as_ref()?;
    if let Some(set) = login.other.get("passwordRevisionDate").and_then(|v| v.as_str()) {
        return Some(today - super::detail::date_days(set)?);
    }
    // The newest history date, whichever order the entries come in; only the
    // dates are read, never the previous passwords. History with no
    // readable date is an unknown age, not the item's creation.
    if let Some(history) = item.other.get("passwordHistory").and_then(|v| v.as_array()).filter(|h| !h.is_empty()) {
        let newest = history
            .iter()
            .filter_map(|entry| entry.get("lastUsedDate")?.as_str())
            .filter_map(super::detail::date_days)
            .max()?;
        return Some(today - newest);
    }
    let created = item.other.get("creationDate").and_then(|v| v.as_str())?;
    Some(today - super::detail::date_days(created)?)
}

/// D5: every item with a password unchanged for [`OLD_DAYS`] or more. An
/// item whose date cannot be read is never counted.
pub fn old_items(items: &[VaultItem], today: i64) -> Vec<OldItem> {
    items
        .iter()
        .filter(|item| password_of(item).is_some())
        .filter_map(|item| {
            let days = password_age_days(item, today)?;
            (days >= OLD_DAYS).then(|| OldItem { item: found(item), days })
        })
        .collect()
}

/// An old finding's line.
pub fn old_detail(old: &OldItem) -> String {
    format!("Not changed in {} days", old.days)
}

/// The non-secret half of an item: what a finding row draws and clicks.
fn found(item: &VaultItem) -> FoundItem {
    FoundItem { id: item.id.clone(), name: item.name.clone() }
}

/// The sentence under a weak item: the two facts that produced the verdict.
///
/// "9 characters, lowercase letters and digits" -- something to act on. Never
/// a score; see the module docs.
pub fn weak_detail(weak: &WeakItem) -> String {
    let unit = if weak.chars == 1 { "character" } else { "characters" };
    let names = weak.classes.names();
    let classes = match names.len() {
        // Unreachable for a real finding -- an empty password is excluded by
        // `password_of` long before it gets here -- but stated rather than
        // left to produce a sentence with a hole in it.
        0 => "no characters at all".to_string(),
        1 => format!("{} only", names[0]),
        _ => {
            let (last, rest) = names.split_last().expect("len >= 2 in this arm");
            format!("{} and {}", rest.join(", "), last)
        }
    };
    format!("{} {unit}, {classes}", weak.chars)
}

/// The heading over one reuse group.
pub fn reuse_group_heading(group: &ReuseGroup) -> String {
    format!("One password, {} items", group.items.len())
}

/// The detail line under one reused row: the OTHER items on its password,
/// by name, in the group's order. Weak and breached rows each say why they
/// are listed; a reused row without this was a bare name under a caption,
/// and the caption does not say which rows go with which once a list runs
/// past the screen.
pub fn reuse_detail(group: &ReuseGroup, at: usize) -> String {
    let others: Vec<&str> = group
        .items
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, item)| finding_name(&item.name))
        .collect();
    format!("Also used by {}", others.join(", "))
}

/// What the top of the screen says, before any list.
///
/// Three states, and **the clean one is a result, not an absence.** A blank
/// panel and a grey "Nothing here yet" both read as a failed load, and "no
/// reused passwords" is the answer the user came for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    /// No item in this vault carries a password at all -- so there is nothing
    /// to report, and saying "no reused passwords" would be claiming a result
    /// this app has not got. Its own state, deliberately.
    NothingToCheck,
    /// Passwords were checked and none of them is reused or weak.
    AllClear,
    /// Findings, with the number of distinct items they are about.
    Findings(usize),
}

/// Decides between the three. Pure, so the empty state can be asserted
/// without a rendered frame.
pub fn summary_of(report: &HealthReport) -> Summary {
    if report.with_password == 0 {
        return Summary::NothingToCheck;
    }
    match report.flagged_items() {
        0 => Summary::AllClear,
        flagged => Summary::Findings(flagged),
    }
}

impl Summary {
    /// The headline. One wording per state, here rather than three literals
    /// at the draw site, so the states can be asserted without a frame.
    pub fn headline(self) -> String {
        match self {
            Summary::NothingToCheck => "No saved passwords to check".to_string(),
            Summary::AllClear => "No reused or weak passwords".to_string(),
            Summary::Findings(1) => "1 item needs attention".to_string(),
            Summary::Findings(n) => format!("{n} items need attention"),
        }
    }

    /// The line under the headline. Always present: the headline alone does
    /// not say what it was measured over, and "no reused passwords" over a
    /// vault the app read three logins out of is a much smaller claim than it
    /// looks.
    pub fn detail(self, report: &HealthReport) -> String {
        let n = report.with_password;
        let noun = if n == 1 { "password" } else { "passwords" };
        match self {
            Summary::NothingToCheck => {
                "Cards, notes, SSH keys and logins with an empty password are not checked."
                    .to_string()
            }
            Summary::AllClear => format!("Checked {n} {noun}."),
            Summary::Findings(_) => format!("Out of {n} {noun} checked."),
        }
    }
}

/// The footer that says where the breach findings on this screen came from,
/// and what they are not.
///
/// # This screen still performs NO lookup, in either state of the setting
///
/// That has not changed and is not softened: see the module docs and
/// [`report_with_scan`], whose `scan` parameter is a map of answers with no
/// way to ask anything. What has changed is that there is now something to
/// display -- a scan the user pressed a button for -- so the footer's job is
/// no longer to explain an absence but to say **when** the findings above it
/// were gathered and that they are a snapshot, not live.
///
/// **Both states of the pill still say something**, and the wording is about
/// the pill rather than about this screen, because the pill governs the
/// per-item badge and not the scan. Saying "breach checking is off" over a
/// list of breach findings would be a contradiction the user would have to
/// resolve; saying which of the two things the pill decides is not.
pub fn breach_note(breach_checking_on: bool) -> String {
    let source = format!(
        "Breach findings come from the last scan you ran, not from opening this screen \u{2014} \
         nothing here checks anything. Run one in {SCAN_LOCATION}."
    );
    if breach_checking_on {
        format!(
            "{source} The setting in {BREACH_SETTING_LOCATION} is on, which checks one password \
             at a time on an item's own pane."
        )
    } else {
        format!(
            "{source} The setting in {BREACH_SETTING_LOCATION} is off, so nothing is checked \
             automatically when you open an item."
        )
    }
}

/// What a row under [`UNKNOWN_HEADING`] says about itself.
///
/// Per row as well as in the band's note, because a row is what gets clicked
/// through to an item and the pane behind it will show an ordinary password
/// with no badge on it. "Unknown" has to travel with the row.
pub const UNKNOWN_ROW_DETAIL: &str = "Not checked \u{2014} the lookup did not complete.";

/// Where the chip row keeps its selection. One `Id`, named once, so the draw
/// and [`show_filter`] cannot disagree about which slot they are using.
fn filter_id() -> egui::Id {
    egui::Id::new("password_health_filter")
}

/// Selects `filter`, as pressing its chip would.
///
/// For `examples/ui_preview`, which has to photograph each cut of one report
/// and has no second frame to click on -- a chip's rect is not known until
/// after the frame that draws it.
///
/// **It writes the pane's own storage, not a second copy of it.** There is
/// one slot, under [`filter_id`], and the chip writes the same one; a
/// screenshot taken after this call is the screenshot a user pressing that
/// chip would get, or it is nothing.
pub fn show_filter(ctx: &egui::Context, filter: HealthFilter) {
    let index = HealthFilter::ALL.iter().position(|f| *f == filter).unwrap_or(0);
    ctx.data_mut(|d| d.insert_temp(filter_id(), index));
}

/// The two section bands, spelled once so the tests that look for them and
/// the pane that paints them cannot drift.
pub const REUSED_HEADING: &str = "REUSED";
pub const WEAK_HEADING: &str = "WEAK";
/// The breach section's band. **"Found in breaches", not "BREACHED"**: the
/// other two bands name a property of the password, and this one names
/// something that happened to it somewhere else.
pub const BREACHED_HEADING: &str = "FOUND IN BREACHES";
/// The band over the passwords a scan asked about and could not settle.
pub const UNKNOWN_HEADING: &str = "COULD NOT BE CHECKED";
/// The line under that band, said once rather than per row.
///
/// **It says what the rows are NOT.** A list under a grey heading reads as a
/// list of lesser problems; these are items with no answer at all, and the
/// sentence has to rule out the reading that they came back clean.
pub const UNKNOWN_NOTE: &str =
    "The check for these did not complete, so nothing is known about them either way. Run the \
     scan again.";
/// What the pane says when the active filter has nothing in it.
///
/// **A result, not a blank space** -- `Summary`'s argument exactly. Four of
/// these rather than one, because "no reused passwords" and "nothing has been
/// scanned" are different answers and a single wording would have to be vague
/// enough to cover both.
pub fn nothing_under(filter: HealthFilter, report: &HealthReport) -> String {
    match filter {
        HealthFilter::All => "No reused, weak or breached passwords.".to_string(),
        HealthFilter::Reused => "No password in this vault is used twice.".to_string(),
        HealthFilter::Weak => "No password in this vault is weak.".to_string(),
        HealthFilter::Old => "No password in this vault is a year old.".to_string(),
        // **The one filter whose empty state has two meanings**, and they are
        // opposite claims: nothing was found, or nothing was looked at. An
        // empty list cannot tell them apart, so the sentence does.
        HealthFilter::Breached if !report.scanned => format!(
            "No scan has been run, so nothing here has been compared against a breach list. \
             The scan is in {SCAN_LOCATION}."
        ),
        HealthFilter::Breached => {
            "None of the passwords the last scan checked was found in a breach.".to_string()
        }
    }
}

/// Where the scan button lives, named the way the user will find it.
///
/// Spelled here as its own literal for [`BREACH_SETTING_LOCATION`]'s reason,
/// and pinned against `prefs_ui.rs` by the same test.
pub const SCAN_LOCATION: &str = "Preferences > Breaches > \"Scan all passwords now\"";

/// The heading over one breach group.
///
/// **Says whether these rows are one password or several**, from the reuse
/// digests rather than from the count -- see [`BreachGroup`]. Three rows
/// under one number is a different job of work depending on the answer, and
/// this screen already knows it.
pub fn breach_group_heading(group: &BreachGroup) -> String {
    let times = crate::breach::breach_count_phrase(group.count);
    match (group.items.len(), group.one_password) {
        (1, _) => times,
        (n, true) => format!("{times} \u{2014} one password, {n} items"),
        // Same count, different passwords. Rare above a handful, and stated
        // as the fact it is rather than as a guess either way.
        (n, false) => format!("{times} \u{2014} {n} items, different passwords"),
    }
}

/// What the page asks the window to do, once a frame at most.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthAction {
    /// Nothing was pressed.
    None,
    /// A finding's row: leave the report and show that item.
    Open(String),
    /// A finding's Change password: the same, straight into the editor.
    ChangePassword(String),
}

/// The page's own measurements (spec A 4): the column is capped at 760, the
/// page pads 32 / 40, a row is 16 / 22 around a 44 tile.
const COLUMN_MAX: f32 = 760.0;
const PAGE_PAD_X: f32 = 40.0;
const PAGE_PAD_Y: f32 = 32.0;
const ROW_PAD_X: f32 = 22.0;
const ROW_PAD_Y: f32 = 16.0;
const TILE: f32 = 44.0;
const ROW_GAP_X: f32 = 14.0;
const ROW_HEIGHT: f32 = ROW_PAD_Y * 2.0 + TILE;
const CARD_RADIUS: u8 = tokens::R_CARD;
/// Below this much room a row drops its button and the whole row is the click.
const BUTTON_FROM: f32 = 440.0;
/// What a Change password button says.
pub const CHANGE_PASSWORD_LABEL: &str = "Change password";

const TITLE: Role = Role { size: 30.0, face: theme::EXTRABOLD, tracking_em: -0.03 };
const ROW_NAME: Role = Role { size: 16.0, face: theme::BOLD, tracking_em: 0.0 };
const ROW_REASON: Role = Role { size: 14.0, face: theme::SEMIBOLD, tracking_em: 0.0 };
const NOTE: Role = Role { size: 14.0, face: theme::REGULAR, tracking_em: 0.0 };
const SUBLINE: Role = Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 };

/// The reason colours (spec A 4): weak and breached red, reused amber, old
/// blue.
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xb7, 0x79, 0x1f);

/// One line of the list card.
struct Line<'a> {
    id: &'a str,
    name: &'a str,
    reason: String,
    colour: egui::Color32,
    /// Offer Change password: not for an item whose check did not complete.
    change: bool,
}

/// The Password health page (spec A 4), drawn into the whole body.
///
/// Takes the report rather than the items, so every decision on screen has
/// already been made by [`report_for`] and this function only paints. A
/// finding's row opens that item; its button opens the item's editor -- both
/// are reported, not performed, so the window applies them through the one
/// door the Shared page's Open record uses.
pub fn draw_password_health(ui: &mut egui::Ui, report: &HealthReport, breach_checking_on: bool) -> HealthAction {
    let mut action = HealthAction::None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let room = ui.available_rect_before_wrap();
        let width = (room.width() - PAGE_PAD_X * 2.0).clamp(1.0, COLUMN_MAX);
        let left = room.left() + ((room.width() - width) / 2.0).max(0.0);
        let column = egui::Rect::from_min_size(egui::pos2(left, room.top() + PAGE_PAD_Y), egui::vec2(width, 0.0));
        let used = ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            action = page(ui, report, breach_checking_on);
        });
        // The page's bottom padding, and the scroll area's content height.
        ui.allocate_space(egui::vec2(room.width(), used.response.rect.bottom() - room.top() + PAGE_PAD_Y));
    });
    action
}

fn page(ui: &mut egui::Ui, report: &HealthReport, breach_checking_on: bool) -> HealthAction {
    let p = tokens::palette(ui.ctx());
    let mut action = HealthAction::None;

    ui.label(tokens::job(TITLE, HEALTH_PANE_TITLE, p.ink));
    ui.add_space(8.0);
    let summary = summary_of(report);
    let headline_ink = match summary {
        Summary::Findings(_) => p.error,
        Summary::AllClear | Summary::NothingToCheck => p.ink,
    };
    ui.add(egui::Label::new(tokens::job(Role { size: 17.0, face: theme::BOLD, tracking_em: 0.0 }, &summary.headline(), headline_ink)).wrap());
    ui.add_space(2.0);
    ui.add(egui::Label::new(tokens::job(SUBLINE, &summary.detail(report), p.text_faint)).wrap());
    ui.add_space(20.0);

    let filter = draw_filters(ui);
    ui.add_space(20.0);

    let put = |found: Option<HealthAction>, action: &mut HealthAction| {
        if let Some(found) = found {
            *action = found;
        }
    };

    if filter.shows_reused() && !report.reused.is_empty() {
        section(ui, REUSED_HEADING);
        let cards: Vec<(String, Vec<Line<'_>>)> = report
            .reused
            .iter()
            .map(|group| {
                let lines = group
                    .items
                    .iter()
                    .enumerate()
                    .map(|(at, item)| Line { id: &item.id, name: &item.name, reason: reuse_detail(group, at), colour: AMBER, change: true })
                    .collect();
                (reuse_group_heading(group), lines)
            })
            .collect();
        for (caption, lines) in &cards {
            let found = list_card(ui, Some(caption), lines);
            put(found, &mut action);
        }
    }
    if filter.shows_weak() && !report.weak.is_empty() {
        section(ui, WEAK_HEADING);
        let lines: Vec<Line<'_>> = report
            .weak
            .iter()
            .map(|weak| Line { id: &weak.item.id, name: &weak.item.name, reason: weak_detail(weak), colour: p.error, change: true })
            .collect();
        let found = list_card(ui, None, &lines);
        put(found, &mut action);
    }
    if filter.shows_old() && !report.old.is_empty() {
        section(ui, OLD_HEADING);
        let lines: Vec<Line<'_>> = report
            .old
            .iter()
            .map(|old| Line { id: &old.item.id, name: &old.item.name, reason: old_detail(old), colour: p.blue_deep, change: true })
            .collect();
        let found = list_card(ui, None, &lines);
        put(found, &mut action);
    }
    if filter.shows_breached() {
        if !report.breached.is_empty() {
            section(ui, BREACHED_HEADING);
            for group in &report.breached {
                let phrase = crate::breach::breach_phrase(group.count);
                let lines: Vec<Line<'_>> = group
                    .items
                    .iter()
                    .map(|item| Line { id: &item.id, name: &item.name, reason: phrase.clone(), colour: p.error, change: true })
                    .collect();
                let caption = breach_group_heading(group);
                let found = list_card(ui, Some(&caption), &lines);
                put(found, &mut action);
            }
        }
        // Always drawn when there are any, under both filters that show
        // breach findings: an item nobody could check is not an item that is
        // fine, and leaving it off the list is the one failure this section
        // exists to avoid.
        if !report.unknown.is_empty() {
            section(ui, UNKNOWN_HEADING);
            let lines: Vec<Line<'_>> = report
                .unknown
                .iter()
                .map(|item| Line { id: &item.id, name: &item.name, reason: UNKNOWN_ROW_DETAIL.to_string(), colour: p.text_faint, change: false })
                .collect();
            let found = list_card(ui, Some(UNKNOWN_NOTE), &lines);
            put(found, &mut action);
        }
    }

    // An empty filter says so: a page that stopped after the tabs reads as a
    // screen that failed to load, and "nothing is wrong" is the answer the
    // user came for.
    if report.shown_under(filter) == 0 {
        kit_card::card(ui, CARD_RADIUS, Margin::symmetric(ROW_PAD_X as i8, 22), |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(tokens::job(Role { size: 16.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, &nothing_under(filter, report), p.text_muted)).wrap());
        });
    }

    ui.add_space(14.0);
    ui.add(egui::Label::new(tokens::job(NOTE, &breach_note(breach_checking_on), p.text_label)).wrap());
    action
}

/// "REUSED" / "WEAK": tracked capitals over a card.
fn section(ui: &mut egui::Ui, text: &str) {
    let p = tokens::palette(ui.ctx());
    kit_card::section_label(ui, text, p.text_ghost);
    ui.add_space(10.0);
}

/// One white card of findings, with an optional caption row at its head.
/// Answers what was pressed on one of its rows.
fn list_card(ui: &mut egui::Ui, caption: Option<&str>, lines: &[Line<'_>]) -> Option<HealthAction> {
    let p = tokens::palette(ui.ctx());
    let mut found = None;
    kit_card::card(ui, CARD_RADIUS, Margin::ZERO, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        if let Some(caption) = caption {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(ROW_PAD_X);
                ui.add(egui::Label::new(tokens::job(Role { size: 13.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, caption, p.text_faint)).wrap());
            });
            ui.add_space(12.0);
            kit_card::row_rule(ui);
        }
        for (at, line) in lines.iter().enumerate() {
            if at > 0 {
                kit_card::row_rule(ui);
            }
            if let Some(hit) = finding_row(ui, line) {
                found = Some(hit);
            }
        }
    });
    ui.add_space(16.0);
    found
}

/// What a finding row calls an item that has no name of its own.
///
/// **A finding that does not say WHICH item is unactionable.** The user
/// cannot go and change a password they cannot find. In square brackets and in
/// the page's own words, so it cannot be mistaken for an item really called
/// this.
pub const UNNAMED_ITEM: &str = "[No name]";

/// The name a finding row paints for `name`: the item's own, or
/// [`UNNAMED_ITEM`] when it has none. Whitespace counts as none.
fn finding_name(name: &str) -> &str {
    if name.trim().is_empty() {
        UNNAMED_ITEM
    } else {
        name
    }
}

/// One finding: a 44 tile, the name over a coloured reason, and the primary
/// Change password button. The row opens the item; the button its editor.
fn finding_row(ui: &mut egui::Ui, line: &Line<'_>) -> Option<HealthAction> {
    let p = tokens::palette(ui.ctx());
    let width = ui.available_width();
    let (rect, row) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if row.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, p.list_bg);
    }

    let tile = egui::Rect::from_min_size(egui::pos2(rect.left() + ROW_PAD_X, rect.center().y - TILE / 2.0), egui::vec2(TILE, TILE));
    // `new_child`, not `scope_builder`: a scope moves the parent's cursor to
    // its own bottom, which is 16 points ABOVE the row's, and the next row
    // would start on top of this one.
    let mut tile_ui = ui.new_child(egui::UiBuilder::new().max_rect(tile));
    item_list::row_tile(&mut tile_ui, None, crate::vault_bridge::ItemKind::Login, finding_name(line.name), TILE, false);

    let show_button = line.change && width >= BUTTON_FROM;
    let button_width = controls::button_width(ui.ctx(), ButtonKind::Primary, controls::ROW, CHANGE_PASSWORD_LABEL, false);
    let text_left = tile.right() + ROW_GAP_X;
    let text_right = rect.right() - ROW_PAD_X - if show_button { button_width + ROW_GAP_X } else { 0.0 };
    let room = (text_right - text_left).max(1.0);

    let name = ui.fonts_mut(|f| f.layout_job(tokens::job(ROW_NAME, finding_name(line.name), p.ink)));
    let reason = ui.fonts_mut(|f| f.layout_job(tokens::job(ROW_REASON, &line.reason, line.colour)));
    let name = truncate(ui, name, room);
    let reason = truncate(ui, reason, room);
    let gap = 3.0;
    let block = name.size().y + gap + reason.size().y;
    let top = rect.center().y - block / 2.0;
    ui.painter().galley(egui::pos2(text_left, top), name.clone(), p.ink);
    ui.painter().galley(egui::pos2(text_left, top + name.size().y + gap), reason, line.colour);

    let mut hit = row.clicked().then(|| HealthAction::Open(line.id.to_string()));
    if show_button {
        let slot = egui::Rect::from_min_size(
            egui::pos2(rect.right() - ROW_PAD_X - button_width, rect.center().y - controls::ROW.height / 2.0),
            egui::vec2(button_width, controls::ROW.height),
        );
        let mut slot_ui = ui.new_child(egui::UiBuilder::new().max_rect(slot));
        if controls::button(&mut slot_ui, ButtonKind::Primary, controls::ROW, CHANGE_PASSWORD_LABEL, None).clicked() {
            hit = Some(HealthAction::ChangePassword(line.id.to_string()));
        }
    }
    hit
}

/// `galley` cut to `room` with an ellipsis, as one line.
fn truncate(ui: &egui::Ui, galley: std::sync::Arc<egui::Galley>, room: f32) -> std::sync::Arc<egui::Galley> {
    if galley.size().x <= room {
        return galley;
    }
    let mut job = galley.job.as_ref().clone();
    job.wrap = egui::text::TextWrapping { max_width: room, max_rows: 1, break_anywhere: true, overflow_character: Some('\u{2026}') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The tabs, and the currently selected filter.
///
/// # The selection lives in egui's memory, not in the window's state
///
/// Deliberately. The vault window owns the vault, the selection and the
/// screen; which of five cuts of ONE report is on screen is not a fact about
/// the vault, it does not survive the window, and nothing outside this file
/// reads it. It resets to `All` when the window closes, which is the right
/// default: the screen opens on everything that is wrong.
fn draw_filters(ui: &mut egui::Ui) -> HealthFilter {
    let id = filter_id();
    let mut chosen = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(id))
        .and_then(|i| HealthFilter::ALL.get(i).copied())
        .unwrap_or(HealthFilter::All);
    let labels: Vec<&str> = HealthFilter::ALL.iter().map(|f| f.label()).collect();
    let at = HealthFilter::ALL.iter().position(|f| *f == chosen).unwrap_or(0);
    if let Some(picked) = controls::segmented(ui, &labels, at, controls::SEG_TABS) {
        chosen = HealthFilter::ALL[picked];
        ui.ctx().data_mut(|d| d.insert_temp(id, picked));
    }
    chosen
}

/// The rail row this screen must leave selected behind it.
///
/// **This is not decoration.** The detail pane resolves `selected_id` against
/// `list_for(filter.source(), ..)`, so with Trash or Archive selected behind
/// this screen a clicked finding -- which is always a LIVE vault item --
/// would be looked up in a list that by construction does not hold it, and
/// the pane would sit on "Select an item." for a row the user just clicked.
/// `vault_window::mod` calls this when the row is pressed; here is where
/// "a live-vault row" is decided, once.
///
/// A live-vault row is kept as it is: the user's own scope is theirs, and
/// leaving this screen should put them back where they were.
pub fn opening_filter(current: &sidebar::SidebarFilter) -> sidebar::SidebarFilter {
    match current.source() {
        sidebar::FilterSource::LiveVault => current.clone(),
        // All items, and not the row that was selected: every finding on this
        // screen is a live item, so the scope behind the report is the whole
        // live vault.
        sidebar::FilterSource::Trash | sidebar::FilterSource::Archive => {
            sidebar::SidebarFilter::All
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::LoginData;

    /// A login carrying `password`, or -- for `None` -- a login object with
    /// no password field at all.
    fn login(id: &str, name: &str, password: Option<&str>) -> VaultItem {
        let mut item = bare(id, name);
        item.item_type = Some(1);
        item.login = Some(LoginData {
            username: Some("user".into()),
            password: password.map(|p| Zeroizing::new(p.to_string())),
            totp: None,
            uris: vec![],
            other: serde_json::Map::new(),
        });
        item
    }

    /// An item with no `login` object whatsoever -- what a card, a secure
    /// note, an identity or an SSH key looks like to this module.
    fn bare(id: &str, name: &str) -> VaultItem {
        VaultItem {
            id: id.into(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(3),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// A password `rate` calls `Strength::Strong`, so a vault built out of
    /// these has no weak finding to confuse a reuse assertion with.
    const STRONG_A: &str = "Tr0ub4dor&3xtraLong!";
    const STRONG_B: &str = "Qu1etHarbour#Lantern9";
    const STRONG_C: &str = "V3lvet*Meridian~Fold4";

    /// The names of the items in each reuse group, in order -- what the pane
    /// actually lists.
    fn group_names(report: &HealthReport) -> Vec<Vec<&str>> {
        report
            .reused
            .iter()
            .map(|group| group.items.iter().map(|i| i.name.as_str()).collect())
            .collect()
    }

    fn weak_names(report: &HealthReport) -> Vec<&str> {
        report.weak.iter().map(|w| w.item.name.as_str()).collect()
    }

    // ==================================================================
    // Reuse: found where it is there, and absent where it is not
    // ==================================================================

    /// The headline claim, and its counterpart directly below it. **Both go
    /// through `report_for`**, so neither is a test of a fixture.
    #[test]
    fn two_items_sharing_a_password_are_reported_as_one_group() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        assert_eq!(group_names(&report), vec![vec!["Bank", "Forum"]]);
        assert_eq!(report.with_password, 3);
        assert!(report.weak.is_empty(), "these three are all strong: {:?}", weak_names(&report));
    }

    /// The negative half of the pair, from the same function: change one
    /// password and the group must disappear entirely, not merely shrink.
    #[test]
    fn a_vault_with_no_reuse_yields_no_groups_at_all() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_C)),
        ];
        let report = report_for(&items);
        assert!(
            report.reused.is_empty(),
            "three different passwords produced reuse groups: {:?}",
            group_names(&report)
        );
        assert_eq!(report.with_password, 3, "the control: all three were actually looked at");
        assert_eq!(summary_of(&report), Summary::AllClear);
    }

    #[test]
    fn a_password_used_once_is_not_a_group() {
        let report = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert!(report.reused.is_empty());
        assert_eq!(report.with_password, 1);
    }

    #[test]
    fn three_items_on_one_password_are_one_group_of_three_and_not_three_pairs() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
            login("c", "Forum", Some(STRONG_A)),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "{:?}", group_names(&report));
        assert_eq!(group_names(&report), vec![vec!["Bank", "Shop", "Forum"]]);
        assert_eq!(reuse_group_heading(&report.reused[0]), "One password, 3 items");
        // Each row names the others on its password, never itself.
        assert_eq!(reuse_detail(&report.reused[0], 0), "Also used by Shop, Forum");
        assert_eq!(reuse_detail(&report.reused[0], 1), "Also used by Bank, Forum");
        assert_eq!(reuse_detail(&report.reused[0], 2), "Also used by Bank, Shop");
    }

    /// Two independent groups, and the exact count -- an implementation that
    /// merged every reused password into one bucket, or that emitted a group
    /// per pair, both fail here and neither fails an `is_empty()` check.
    #[test]
    fn two_separate_reuses_are_two_groups_in_vault_order() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
            login("c", "Forum", Some(STRONG_A)),
            login("d", "Mail", Some(STRONG_B)),
        ];
        let report = report_for(&items);
        assert_eq!(
            group_names(&report),
            vec![vec!["Bank", "Forum"], vec!["Shop", "Mail"]],
            "groups must come out ordered by their first member's position in the vault, so \
             the screen reads the way the vault does"
        );
    }

    /// The grouping is byte-exact, not a similarity heuristic: two passwords
    /// differing in one character's case are two passwords.
    #[test]
    fn a_one_character_difference_is_not_reuse() {
        let items = vec![
            login("a", "Bank", Some("Tr0ub4dor&3xtraLong!")),
            login("b", "Shop", Some("Tr0ub4dor&3xtraLong?")),
        ];
        assert!(report_for(&items).reused.is_empty());
    }

    // ==================================================================
    // Items with no password: excluded, not counted as weak
    // ==================================================================

    /// **A card reported as "weak password" is a bug.** Asserted with a real
    /// weak login sitting in the same vault, so the test cannot pass by the
    /// weak list being empty for some unrelated reason.
    #[test]
    fn items_without_a_password_are_excluded_rather_than_counted_as_weak() {
        let items = vec![
            bare("card", "Visa"),
            bare("note", "Recovery codes"),
            login("empty", "Half-filled login", Some("")),
            login("absent", "Passwordless login", None),
            login("weak", "Old forum", Some("abc")),
        ];
        let report = report_for(&items);
        assert_eq!(
            weak_names(&report),
            vec!["Old forum"],
            "only the item that actually has a password may be reported"
        );
        assert_eq!(
            report.with_password, 1,
            "four of these five have nothing to check, so the denominator is 1"
        );
        assert!(report.reused.is_empty(), "two empty passwords are not a shared password");
    }

    /// The specific trap: two items with an EMPTY password would group
    /// together on any implementation that hashed before it filtered, and
    /// would be reported as a shared password the user cannot find.
    #[test]
    fn two_empty_passwords_are_not_a_reuse_group() {
        let items = vec![
            login("a", "One", Some("")),
            login("b", "Two", Some("")),
            login("c", "Three", None),
        ];
        let report = report_for(&items);
        assert!(report.reused.is_empty(), "{:?}", group_names(&report));
        assert_eq!(report.with_password, 0);
        assert_eq!(summary_of(&report), Summary::NothingToCheck);
    }

    // ==================================================================
    // Weakness: exactly what `rate` says, and nothing of its own
    // ==================================================================

    /// **One rule, two surfaces.** Walked over a table that contains both
    /// verdicts, so a `report_for` that flagged everything and one that
    /// flagged nothing both fail.
    #[test]
    fn the_weak_list_is_exactly_the_passwords_rate_calls_weak() {
        let passwords = [
            "abc",                    // < 8: Weak
            "abcdefgh",               // 8, one class: Weak
            "Ab1!efgh",               // 8, four classes: Fair
            "horsehorsehorse",        // 15, one class: Fair
            "Tr0ub4dor&3xtraLong!",   // Strong
        ];
        let items: Vec<VaultItem> = passwords
            .iter()
            .enumerate()
            .map(|(n, p)| login(&n.to_string(), &n.to_string(), Some(p)))
            .collect();
        let report = report_for(&items);

        let expected: Vec<String> = passwords
            .iter()
            .enumerate()
            .filter(|(_, p)| rate(p) == Strength::Weak)
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(expected.len(), 2, "control: the table must contain both verdicts");
        assert_eq!(weak_names(&report), expected.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            report.with_password,
            passwords.len(),
            "control: every password in the table was actually reached"
        );
    }

    /// The two facts carried with the finding are the password's own, not a
    /// default: a wrong-item lookup would give the wrong length here.
    #[test]
    fn a_weak_finding_carries_that_password_s_own_length_and_classes() {
        let items = vec![
            login("a", "Strong", Some("Tr0ub4dor&3xtraLong!")),
            login("b", "Old forum", Some("hunter22")),
        ];
        let report = report_for(&items);
        assert_eq!(report.weak.len(), 1);
        let weak = &report.weak[0];
        assert_eq!(weak.item.name, "Old forum");
        assert_eq!(weak.chars, 8);
        assert_eq!(weak.classes, CharClasses::of("hunter22"));
        assert_eq!(weak_detail(weak), "8 characters, lowercase letters and digits");
    }

    /// Characters, not bytes. `"pa\u{df}wort"` is 7 characters and 8 bytes;
    /// telling the user it is 8 would be telling them something they cannot
    /// count on screen.
    #[test]
    fn the_length_reported_is_characters_and_not_bytes() {
        let password = "pa\u{df}wort";
        assert_eq!(password.len(), 8, "control: this string really is 8 bytes");
        let report = report_for(&[login("a", "German", Some(password))]);
        assert_eq!(report.weak.len(), 1, "7 characters is under 8, so this is weak");
        assert_eq!(report.weak[0].chars, 7);
    }

    /// Every branch of the sentence, including the one-class case the "12
    /// characters, letters only" example is about.
    #[test]
    fn the_weak_sentence_names_the_classes_actually_present() {
        let one = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 12,
            classes: CharClasses::of("abcdefghijkl"),
        };
        assert_eq!(weak_detail(&one), "12 characters, lowercase letters only");

        let three = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 9,
            classes: CharClasses::of("aB1"),
        };
        assert_eq!(
            weak_detail(&three),
            "9 characters, lowercase letters, uppercase letters and digits"
        );

        let single_char = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 1,
            classes: CharClasses::of("7"),
        };
        assert_eq!(weak_detail(&single_char), "1 character, digits only");
    }

    /// **No score, stated as a test.** A "43/100" would be a number the user
    /// cannot act on, and this is the assertion that keeps one from being
    /// added to the row's own sentence later.
    #[test]
    fn the_weak_sentence_carries_no_score() {
        let weak = WeakItem {
            item: FoundItem { id: "1".into(), name: "n".into() },
            chars: 6,
            classes: CharClasses::of("abc123"),
        };
        let sentence = weak_detail(&weak);
        assert!(sentence.contains("6 characters"), "the actionable fact is missing: {sentence}");
        for banned in ["/100", "score", "Score", "%"] {
            assert!(!sentence.contains(banned), "{sentence:?} carries a score");
        }
    }

    // ==================================================================
    // The badge, and the three summary states
    // ==================================================================

    /// An item that is both reused and weak is ONE item needing attention.
    /// The counter-assertion is the test: the two lists really do both hold
    /// it, so a naive `reused + weak` would say 3 here.
    #[test]
    fn an_item_that_is_both_reused_and_weak_is_counted_once() {
        let items = vec![
            login("a", "One", Some("abc")),
            login("b", "Two", Some("abc")),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1);
        assert_eq!(report.reused[0].items.len(), 2);
        assert_eq!(report.weak.len(), 2, "the premise: both are also weak");
        assert_eq!(report.flagged_items(), 2);
        assert_eq!(summary_of(&report), Summary::Findings(2));
        assert_eq!(Summary::Findings(2).headline(), "2 items need attention");
    }

    /// The three states, each from a vault that really produces it, and each
    /// with a headline distinct from the other two -- so a `summary_of` wired
    /// to a constant fails.
    #[test]
    fn the_three_summary_states_come_from_three_different_vaults() {
        let nothing = report_for(&[bare("card", "Visa")]);
        let clear = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ]);
        let findings = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
        ]);

        assert_eq!(summary_of(&nothing), Summary::NothingToCheck);
        assert_eq!(summary_of(&clear), Summary::AllClear);
        assert_eq!(summary_of(&findings), Summary::Findings(2));

        let headlines = [
            summary_of(&nothing).headline(),
            summary_of(&clear).headline(),
            summary_of(&findings).headline(),
        ];
        let mut distinct = headlines.to_vec();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 3, "two states say the same thing: {headlines:?}");
    }

    /// **The good empty state is a result, not an absence.** It says what was
    /// checked, and it does not use the item list's grey "nothing here"
    /// wording -- which is the shape of a load that failed.
    #[test]
    fn the_all_clear_state_reports_what_it_checked() {
        let report = report_for(&[
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ]);
        let summary = summary_of(&report);
        assert_eq!(summary.headline(), "No reused or weak passwords");
        assert_eq!(summary.detail(&report), "Checked 2 passwords.");
        for banned in ["Nothing here yet", "Loading", "No items"] {
            assert!(!summary.headline().contains(banned));
            assert!(!summary.detail(&report).contains(banned));
        }
    }

    /// "No reused passwords" over a vault with no passwords in it is a
    /// non-answer wearing a result's clothes. The two must not say the same
    /// thing, which is why `NothingToCheck` exists at all.
    #[test]
    fn an_unanswerable_vault_does_not_claim_a_clean_bill_of_health() {
        let nothing = report_for(&[bare("card", "Visa")]);
        let clear = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert_ne!(summary_of(&nothing).headline(), summary_of(&clear).headline());
        assert_eq!(summary_of(&nothing).headline(), "No saved passwords to check");
        assert!(summary_of(&nothing).detail(&nothing).contains("Cards"));
    }

    #[test]
    fn the_singular_wordings_are_singular() {
        let one_finding = report_for(&[
            login("a", "One", Some("abc")),
            login("b", "Two", Some(STRONG_A)),
        ]);
        assert_eq!(summary_of(&one_finding), Summary::Findings(1));
        assert_eq!(summary_of(&one_finding).headline(), "1 item needs attention");

        let one_password = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        assert_eq!(summary_of(&one_password).detail(&one_password), "Checked 1 password.");
    }

    // ==================================================================
    // Secrets: what the report holds, and what it does not
    // ==================================================================

    /// **The report is `Debug`-safe because it holds nothing derived from a
    /// password.** Constructed with a needle nothing else in this crate could
    /// produce, and asserted against the whole formatted report -- which is
    /// the check `debug_leak_guard`'s source scan cannot make.
    #[test]
    fn the_report_cannot_print_a_password_or_its_digest() {
        const NEEDLE: &str = "correct-horse-battery-staple-NEEDLE";
        let items = vec![
            login("a", "Bank", Some(NEEDLE)),
            login("b", "Shop", Some(NEEDLE)),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "control: the needle really was reused");

        let printed = format!("{report:?}");
        assert!(
            printed.contains("Bank"),
            "control: the report prints something at all, so the assertion below is not \
             passing over an empty string: {printed}"
        );
        assert!(!printed.contains(NEEDLE), "the password reached a formatter: {printed}");
        // The digest, in the two spellings a leak would take.
        let digest = Sha256::digest(NEEDLE.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!printed.to_lowercase().contains(&hex), "the digest reached a formatter");
        assert!(
            !printed.contains(&format!("{}", digest[0])),
            "a byte of the digest reached a formatter -- the report is carrying the key"
        );
    }

    /// **No plaintext-keyed index, and no grouping key at all, survives the
    /// call.** A source pin, because the thing being asserted is structural:
    /// the digests must live in `report_for` and nowhere else.
    #[test]
    fn no_type_in_this_module_carries_a_password_or_a_digest() {
        let source = own_production();
        // The four declared types are the report's whole surface.
        for (name, body) in declared_type_bodies(&source) {
            // `" password:"` with the leading space, so the honest
            // `with_password` COUNT on the report is not mistaken for a
            // field holding one. A field literally named `password`
            // still fails.
            for banned in [
                concat!("Zeroiz", "ing"),
                "[u8; 32]",
                " password:",
                "digest",
                "Sha256",
            ] {
                assert!(
                    !body.contains(banned),
                    "`{name}` mentions {banned:?} in its body. Nothing the report hands back \
                     may carry a password or a value derived from one: the grouping key lives \
                     inside `report_for` and dies there. See the module docs"
                );
            }
        }
        assert!(
            source.contains(concat!("Zeroiz", "ing<[u8; 32]>")),
            "control: the digest key is not in this file at all, so the scan above is vacuous"
        );
        assert!(
            declared_type_bodies(&source).iter().any(|(name, _)| name == "WeakItem"),
            "control: the parser missed a type, so the loop above ran over the wrong set"
        );
    }

    /// **Nothing in this file logs, and nothing in it reaches the network.**
    /// File-scoped, exactly as `breach::the_breach_module_never_logs` is, and
    /// worth stating for the same reason: this is the one module that holds
    /// every password in the vault in its hands at once.
    ///
    /// The needles are the bare macro tokens rather than fully-qualified
    /// paths, because `use log::warn;` is the ordinary spelling and a
    /// path-only guard never sees it.
    #[test]
    fn this_module_neither_logs_nor_opens_anything() {
        let source = own_production();
        for banned in [
            concat!("log", "::"),
            concat!("warn", "!"),
            concat!("info", "!"),
            concat!("debug", "!"),
            concat!("error", "!"),
            concat!("trace", "!"),
            concat!("println", "!"),
            concat!("eprintln", "!"),
            concat!("dbg", "!"),
            concat!("http", "_agent"),
            concat!("ureq", ""),
            concat!("Command", "::new"),
            concat!("std::fs", ""),
            // **The four doors out of `crate::breach`, named one by one
            // instead of banning the module.**
            //
            // The old needle was `breach::`, which was right while this
            // screen displayed nothing from that module. It now displays
            // breach findings -- from a scan the user pressed a button for,
            // handed in as DATA -- so it legitimately names
            // `breach::BreachStatus` and `breach::breach_phrase`, an enum and
            // a formatter, neither of which can ask anything.
            //
            // A module ban would have had to be deleted wholesale to allow
            // that, which would have left NO rule at all. These four are the
            // ones that reach the network or hash a password, and each is
            // banned by name:
            //
            //  * `check_prefix` and `build_agent` are the request.
            //  * `BreachCache` is the request with a map in front of it.
            //  * `split_hash` is the only way to build a `Prefix`, so a call
            //    site with no `Prefix` cannot address the range API at all.
            concat!("check_", "prefix"),
            concat!("build_", "agent"),
            concat!("Breach", "Cache"),
            concat!("split_", "hash"),
        ] {
            assert!(
                !source.contains(banned),
                "`password_health.rs` spells {banned:?} in its production half. This module \
                 sees every password in the vault; it does not log, it does not spawn, and it \
                 does not make a request -- see the module docs on breach data"
            );
        }
        assert!(
            source.contains("fn report_for"),
            "control: the production half was not read, so every needle above matched nothing"
        );
        // The positive control on the four names above: they are real items
        // in `crate::breach`, so banning them means something. A needle that
        // had drifted would ban nothing and this guard would be vacuous.
        let breach = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/breach.rs"),
        )
        .expect("breach.rs is readable");
        for real in ["fn check_prefix", "fn build_agent", "struct BreachCache", "fn split_hash"] {
            assert!(
                breach.contains(real),
                "`breach.rs` no longer declares `{real}`, so the ban on it above is a ban on \
                 nothing"
            );
        }
    }

    /// This file's production half -- everything above the `#[cfg(test)]`
    /// module this test is in -- **with its comments removed**.
    ///
    /// The strip is not tidiness. This module documents heavily and by name:
    /// its own docs discuss `crate::breach`, say the word "password" in
    /// almost every paragraph, and quote the very needles the two guards
    /// below search for. Over raw text those guards would fire on their own
    /// prose, and the obvious fix -- dropping the needles that collide -- is
    /// the fix that guts them. `a_needle_in_prose_is_not_code_here` is the
    /// control that this really strips.
    fn own_production() -> String {
        code_without_comments(&own_source())
    }

    /// The same production half with its comments left in, for the one test
    /// that wants to look at the file as written.
    fn own_source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/vault_window/password_health.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        let marker = "#[cfg(test)]";
        let cut = text
            .find(marker)
            .unwrap_or_else(|| panic!("no {marker} in {path:?}, so the split found nothing"));
        text[..cut].to_string()
    }

    /// `text` with everything from each `//` to end of line removed. Catches
    /// `//`, `///` and `//!` in one rule, which is every comment form this
    /// file uses.
    fn code_without_comments(text: &str) -> String {
        text.lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The control for [`code_without_comments`]: without it both guards
    /// below pass on their own doc comments and assert nothing about the
    /// code at all.
    #[test]
    fn a_needle_in_prose_is_not_code_here() {
        let stripped = code_without_comments(concat!(
            "//! see `crate::breach` and log::warn!\n",
            "/// a password\n",
            "let x = 1; // brea", "ch::check_prefix\n",
        ));
        assert!(!stripped.contains("breach"), "prose survived the strip: {stripped:?}");
        assert!(!stripped.contains("password"), "prose survived the strip: {stripped:?}");
        assert!(stripped.contains("let x = 1;"), "the strip ate real code: {stripped:?}");
        assert!(
            own_source().contains("crate::breach"),
            "control: this module's own docs no longer mention `crate::breach`, so the \
             strip above is not being exercised by the guards that use it"
        );
    }

    /// Each `pub struct` in `source`, as (name, the text between its braces).
    fn declared_type_bodies(source: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let head = "\npub struct ";
        let mut from = 0usize;
        while let Some(at) = source[from..].find(head) {
            let start = from + at + head.len();
            let name: String = source[start..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let Some(open) = source[start..].find('{').map(|o| start + o) else {
                break;
            };
            let close = crate::below_cut::match_brace(source, open);
            out.push((name, source[open..=close].to_string()));
            from = close;
        }
        assert!(
            out.len() >= 3,
            "control: only {} `pub struct` declarations were parsed out of this module, which \
             is not its surface: {:?}",
            out.len(),
            out.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        out
    }

    // ==================================================================
    // Breach findings
    // ==================================================================

    /// A `ScanResults` holding exactly these answers.
    ///
    /// **Built through `breach_scan`'s own recorder**, not by reaching into a
    /// private map: a fixture assembled by hand here would go on passing
    /// after a scan stopped producing this shape. `ScanResults` cannot ask
    /// anything -- it has no channel, no agent and no URL -- so nothing in
    /// this file touches the network by writing into one.
    fn scan_of(
        answers: &[(&str, crate::breach::BreachStatus)],
    ) -> crate::breach_scan::ScanResults {
        let mut out = crate::breach_scan::ScanResults::default();
        for (id, status) in answers {
            out.set_status(&[(*id).to_string()], *status);
        }
        out
    }

    /// **Grouped by count, worst first.** A list in vault order buries the
    /// 40,000-hit password under three that were seen twice.
    #[test]
    fn breach_findings_are_grouped_by_count_worst_first() {
        let items = vec![
            login("a", "Small", Some(STRONG_A)),
            login("b", "Huge", Some(STRONG_B)),
            login("c", "Middling", Some(STRONG_C)),
        ];
        let report = report_with_scan(
            &items,
            &scan_of(&[
                ("a", crate::breach::BreachStatus::Breached(2)),
                ("b", crate::breach::BreachStatus::Breached(40_231)),
                ("c", crate::breach::BreachStatus::Breached(90)),
            ]),
        );
        assert_eq!(
            report.breached.iter().map(|g| g.count).collect::<Vec<_>>(),
            vec![40_231, 90, 2],
            "the groups are not worst-first"
        );
        assert!(report.scanned);
    }

    /// **Whether two items in one count group are one password is a FACT
    /// here, not a guess.** The count alone cannot tell them apart; the reuse
    /// digests can, and this screen has already computed them.
    #[test]
    fn a_group_says_whether_its_items_are_one_password() {
        let shared = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
        ];
        let report = report_with_scan(
            &shared,
            &scan_of(&[
                ("a", crate::breach::BreachStatus::Breached(7)),
                ("b", crate::breach::BreachStatus::Breached(7)),
            ]),
        );
        assert_eq!(report.breached.len(), 1);
        assert!(
            report.breached[0].one_password,
            "two items on ONE password were reported as two passwords"
        );
        assert!(breach_group_heading(&report.breached[0]).contains("one password"));

        // The coincidence: two DIFFERENT passwords with the same count. Rare
        // above a handful, and stated as the fact it is rather than guessed.
        let distinct = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ];
        let report = report_with_scan(
            &distinct,
            &scan_of(&[
                ("a", crate::breach::BreachStatus::Breached(7)),
                ("b", crate::breach::BreachStatus::Breached(7)),
            ]),
        );
        assert_eq!(report.breached.len(), 1);
        assert!(
            !report.breached[0].one_password,
            "two different passwords sharing a count were reported as one password"
        );
        assert!(breach_group_heading(&report.breached[0]).contains("different passwords"));

        // A group of one claims neither way.
        let single = report_with_scan(
            &[login("a", "Bank", Some(STRONG_A))],
            &scan_of(&[("a", crate::breach::BreachStatus::Breached(7))]),
        );
        assert!(!single.breached[0].one_password);
        let heading = breach_group_heading(&single.breached[0]);
        assert!(
            !heading.contains("password"),
            "a group of one made a claim about reuse: {heading}"
        );
    }

    /// Each row carries `breach_phrase`, which is the thing to DO about it.
    #[test]
    fn a_breach_row_carries_the_phrase_and_the_heading_carries_the_count() {
        let items = vec![login("a", "Bank", Some(STRONG_A))];
        let report = report_with_scan(
            &items,
            &scan_of(&[("a", crate::breach::BreachStatus::Breached(40_231))]),
        );
        let ctx = styled_context();
        let mut selected = None;
        let frame = painted(&ctx, &report, &mut selected, false, None);
        let strings = texts(&frame);
        assert!(strings.contains(&BREACHED_HEADING), "got {strings:?}");
        assert!(
            strings.contains(&crate::breach::breach_phrase(40_231).as_str()),
            "the row does not carry its breach phrase: {strings:?}"
        );
        assert!(
            strings.iter().any(|t| t.contains("40,231")),
            "the count is not grouped by thousands anywhere on screen: {strings:?}"
        );
    }

    /// **A password that could not be checked is LISTED, never omitted.**
    /// "Not shown" reading as "safe" is the failure that matters here.
    #[test]
    fn a_password_that_could_not_be_checked_shows_as_unknown() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ];
        let report = report_with_scan(
            &items,
            &scan_of(&[
                ("a", crate::breach::BreachStatus::Unavailable),
                ("b", crate::breach::BreachStatus::Safe),
            ]),
        );
        assert_eq!(
            report.unknown.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["Bank"],
            "the item nobody could check is not on the list, so its absence reads as safe"
        );
        assert!(report.breached.is_empty());
        // And it counts toward the rail's badge, for the same reason.
        assert_eq!(report.flagged_items(), 1);

        let ctx = styled_context();
        let mut selected = None;
        let frame = painted(&ctx, &report, &mut selected, false, None);
        let strings = texts(&frame);
        assert!(strings.contains(&UNKNOWN_HEADING), "got {strings:?}");
        assert!(strings.contains(&"Bank"), "got {strings:?}");
        assert!(strings.contains(&UNKNOWN_ROW_DETAIL), "got {strings:?}");
        assert!(
            !strings.contains(&"Shop"),
            "a password that came back SAFE was listed; only findings belong on this screen"
        );
    }

    /// Only findings are listed, in every state: a vault of clean logins
    /// paints no rows at all, and says so.
    #[test]
    fn a_clean_scan_lists_nothing_and_says_so() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_B)),
        ];
        let report = report_with_scan(
            &items,
            &scan_of(&[
                ("a", crate::breach::BreachStatus::Safe),
                ("b", crate::breach::BreachStatus::Safe),
            ]),
        );
        assert!(report.breached.is_empty() && report.unknown.is_empty());
        assert_eq!(report.flagged_items(), 0);
        let ctx = styled_context();
        let mut selected = None;
        let frame = painted(&ctx, &report, &mut selected, false, None);
        assert!(
            texts(&frame).contains(&nothing_under(HealthFilter::All, &report).as_str()),
            "got {:?}",
            texts(&frame)
        );
    }

    /// **"Nothing was found" and "nobody looked" are opposite claims**, and
    /// an empty list cannot tell them apart -- so the sentence does.
    #[test]
    fn an_unscanned_vault_and_a_clean_scan_say_different_things() {
        let items = vec![login("a", "Bank", Some(STRONG_A))];
        let unscanned = report_for(&items);
        let clean =
            report_with_scan(&items, &scan_of(&[("a", crate::breach::BreachStatus::Safe)]));
        assert!(!unscanned.scanned && clean.scanned);
        let never = nothing_under(HealthFilter::Breached, &unscanned);
        let ran = nothing_under(HealthFilter::Breached, &clean);
        assert_ne!(never, ran);
        assert!(never.contains("No scan has been run"), "{never}");
        assert!(
            never.contains(SCAN_LOCATION),
            "the empty state does not say where the scan is: {never}"
        );
        assert!(ran.contains("None of the passwords"), "{ran}");
    }

    /// Items with no password stay outside the report, exactly as they do for
    /// reuse and weakness. A card is not "unknown"; there is nothing to know.
    #[test]
    fn an_item_with_no_password_is_never_unknown() {
        let items = vec![bare("card", "A card"), login("empty", "Blank", Some(""))];
        let report = report_with_scan(
            &items,
            &scan_of(&[
                ("card", crate::breach::BreachStatus::Unavailable),
                ("empty", crate::breach::BreachStatus::Unavailable),
            ]),
        );
        assert!(report.unknown.is_empty(), "got {:?}", report.unknown);
    }

    // -- the filters -------------------------------------------------------

    /// Each filter narrows to its own findings and `All` is the union.
    /// Asserted on the pure counter, so the four cuts are checked without
    /// four rendered frames.
    #[test]
    fn each_filter_shows_its_own_findings_and_all_shows_every_one() {
        let items = vec![
            login("a", "Bank", Some(STRONG_A)),
            login("b", "Shop", Some(STRONG_A)),
            login("c", "Weak", Some("abc")),
            login("d", "Breached", Some(STRONG_B)),
            login("e", "Unknown", Some(STRONG_C)),
        ];
        let report = report_with_scan(
            &items,
            &scan_of(&[
                ("d", crate::breach::BreachStatus::Breached(9)),
                ("e", crate::breach::BreachStatus::Unavailable),
            ]),
        );
        assert_eq!(report.shown_under(HealthFilter::Reused), 2);
        assert_eq!(report.shown_under(HealthFilter::Weak), 1);
        assert_eq!(
            report.shown_under(HealthFilter::Breached),
            2,
            "the breached filter shows the found AND the unchecked"
        );
        assert_eq!(
            report.shown_under(HealthFilter::All),
            5,
            "`All` is every distinct flagged item"
        );
    }

    /// The chips are painted, clicking one narrows the list, and a filter
    /// with nothing in it says so rather than leaving a blank pane.
    #[test]
    fn clicking_a_filter_chip_narrows_the_list() {
        let items = vec![
            login("a", "Reused One", Some(STRONG_A)),
            login("b", "Reused Two", Some(STRONG_A)),
            login("c", "Weak One", Some("abc")),
        ];
        let report = report_for(&items);
        let ctx = styled_context();
        let mut selected = None;

        let first = painted(&ctx, &report, &mut selected, false, None);
        for chip in HealthFilter::ALL {
            assert!(
                texts(&first).contains(&chip.label()),
                "chip {:?} is missing: {:?}",
                chip.label(),
                texts(&first)
            );
        }
        assert!(texts(&first).contains(&"Reused One"));
        assert!(texts(&first).contains(&"Weak One"));

        // Press "Weak".
        let at = locate(&first, HealthFilter::Weak.label());
        let narrowed = painted(&ctx, &report, &mut selected, false, Some(at));
        assert!(texts(&narrowed).contains(&"Weak One"), "got {:?}", texts(&narrowed));
        assert!(
            !texts(&narrowed).contains(&"Reused One"),
            "the Weak filter still lists the reuse findings: {:?}",
            texts(&narrowed)
        );
        assert!(!texts(&narrowed).contains(&REUSED_HEADING));

        // Press "Breached", which has nothing in it under this vault.
        let at = locate(&narrowed, HealthFilter::Breached.label());
        let empty = painted(&ctx, &report, &mut selected, false, Some(at));
        assert!(!texts(&empty).contains(&"Weak One"), "got {:?}", texts(&empty));
        assert!(
            texts(&empty).contains(&nothing_under(HealthFilter::Breached, &report).as_str()),
            "an empty filter drew nothing at all, which reads as a screen that failed to load: \
             {:?}",
            texts(&empty)
        );
    }

    // ==================================================================
    // Breach data being off
    // ==================================================================

    /// Both states, and both differ -- a `breach_note` that returned one
    /// string whatever the setting said would pass any single-arm check.
    #[test]
    fn the_breach_note_says_which_state_the_setting_is_in() {
        let off = breach_note(false);
        let on = breach_note(true);
        assert_ne!(off, on, "the note says the same thing whether or not the setting is on");
        assert!(off.contains("is off"), "{off}");
        assert!(on.contains("is on"), "{on}");
        for note in [&off, &on] {
            assert!(
                note.contains(BREACH_SETTING_LOCATION),
                "the note does not say where the setting is: {note}"
            );
            // **Both states say where the findings came from**, which is the
            // half that matters now that there ARE findings: a list of
            // breached items over a footer that did not say they came from a
            // scan the user ran would read as this screen having checked
            // them.
            assert!(
                note.contains(SCAN_LOCATION),
                "the note does not say where the scan is: {note}"
            );
            assert!(
                note.contains("nothing here checks anything"),
                "the note stopped saying that this screen makes no lookup: {note}"
            );
        }
    }

    /// **The literal is pinned against the real preference row.** This module
    /// cannot reference `prefs_ui::BREACH_LABEL` -- it is private there -- so
    /// the sentence spells the label out, and this reads that file off disk
    /// to make renaming it a red suite rather than a sentence pointing at a
    /// row that no longer exists.
    #[test]
    fn the_breach_setting_is_named_where_it_actually_is() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prefs_ui.rs");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
        let label = BREACH_SETTING_LOCATION
            .rsplit_once('"')
            .and_then(|(head, _)| head.rsplit_once('"').map(|(_, label)| label.to_string()))
            .expect("the location string quotes the row's label");
        assert_eq!(label, "Check passwords against known breaches", "control: the parse");
        assert!(
            text.contains(&format!("{label:?}")),
            "`prefs_ui.rs` has no row labelled {label:?} any more, so \
             `BREACH_SETTING_LOCATION` sends the user to a preference that is not there"
        );

        // **And the PAGE, not only the row.** The page name is the half that
        // went stale when the setting moved, and a wrong page name is worse
        // than a wrong row name: the reader arrives somewhere real, does not
        // find the switch, and concludes the app is describing a feature it
        // does not have.
        let page = BREACH_SETTING_LOCATION
            .split(" > ")
            .nth(1)
            .expect("the location string names a page between two chevrons");
        assert_eq!(page, "Breaches", "control: the parse");
        assert!(
            text.contains(&format!("=> {page:?},")),
            "`prefs_ui.rs` has no nav section labelled {page:?}, so \
             `BREACH_SETTING_LOCATION` names a page that is not in that window"
        );
    }

    /// **This screen makes no lookup in EITHER state**, which is the claim
    /// the note's on-state wording rests on. Driven, not merely read: a full
    /// report over a vault of real passwords, with the setting on.
    #[test]
    fn a_report_performs_no_breach_lookup_even_when_the_setting_is_on() {
        let items = vec![
            login("a", "Bank", Some("password")),
            login("b", "Shop", Some("password")),
        ];
        let report = report_for(&items);
        assert_eq!(report.reused.len(), 1, "control: the report really did run");
        // Neither entry point takes a network handle, an agent or a setting.
        // `report_with_scan`'s only breach-shaped parameter is a
        // `ScanResults` -- a map of answers with no channel, no URL and no
        // way to ask anything -- so there is no argument through which a
        // lookup could be made and no branch a future reader could flip. The
        // source pin above is the other half of this claim.
        let scanned = report_with_scan(&items, &crate::breach_scan::ScanResults::default());
        assert_eq!(scanned, report, "an empty scan is exactly the plain report");
        assert!(!scanned.scanned, "an empty map is not a scan that ran");
        assert!(breach_note(true).contains("nothing here checks anything"));
    }

    // ==================================================================
    // The rail row it is entered from
    // ==================================================================

    #[test]
    fn opening_from_a_live_row_keeps_that_row_and_from_an_out_of_vault_row_does_not() {
        assert_eq!(
            opening_filter(&sidebar::SidebarFilter::Logins),
            sidebar::SidebarFilter::Logins,
            "a live-vault row is the user's own scope and is left alone"
        );
        assert_eq!(opening_filter(&sidebar::SidebarFilter::Trash), sidebar::SidebarFilter::All);
        assert_eq!(opening_filter(&sidebar::SidebarFilter::Archive), sidebar::SidebarFilter::All);
        assert_eq!(
            opening_filter(&sidebar::SidebarFilter::Folder("f1".into())),
            sidebar::SidebarFilter::Folder("f1".into())
        );
    }

    // ==================================================================
    // The page, driven
    // ==================================================================

    use crate::kit::testing::{click_at, ctx, frame, Painted};

    const PAGE: egui::Vec2 = egui::vec2(1000.0, 800.0);

    // The helpers the breach and filter tests were written against, now over
    // the page: one frame, every string it painted with the rect it is in.
    fn styled_context() -> egui::Context {
        ctx()
    }

    fn painted(
        ctx: &egui::Context,
        report: &HealthReport,
        _selected: &mut Option<String>,
        breach_on: bool,
        click_at_pos: Option<egui::Pos2>,
    ) -> Vec<(String, egui::Rect)> {
        let mut action = HealthAction::None;
        let events = click_at_pos.map(click_at).unwrap_or_default();
        let shown = page_frame(ctx, report, breach_on, events, &mut action);
        shown.texts.into_iter().map(|(text, _, rect)| (text, rect)).collect()
    }

    fn locate(painted: &[(String, egui::Rect)], needle: &str) -> egui::Pos2 {
        painted
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| panic!("the page painted no {needle:?}: {painted:?}"))
    }

    fn texts(painted: &[(String, egui::Rect)]) -> Vec<&str> {
        painted.iter().map(|(text, _)| text.as_str()).collect()
    }

    fn page_frame(
        ctx: &egui::Context,
        report: &HealthReport,
        breach_on: bool,
        events: Vec<egui::Event>,
        action: &mut HealthAction,
    ) -> Painted {
        frame(ctx, 0.0, PAGE, events, |ui| {
            let got = draw_password_health(ui, report, breach_on);
            if got != HealthAction::None {
                *action = got;
            }
        })
    }

    /// Frames until the layout has settled, then the last one.
    fn settled(ctx: &egui::Context, report: &HealthReport, breach_on: bool) -> Painted {
        let mut action = HealthAction::None;
        let mut painted = page_frame(ctx, report, breach_on, vec![], &mut action);
        for _ in 0..3 {
            painted = page_frame(ctx, report, breach_on, vec![], &mut action);
        }
        painted
    }

    fn pair_report() -> HealthReport {
        report_for(&[
            login("id-bank", "Bank", Some(STRONG_A)),
            login("id-forum", "Forum", Some(STRONG_A)),
            login("id-wiki", "Old wiki", Some("abc")),
        ])
    }

    #[test]
    fn the_page_paints_the_title_the_tabs_the_groups_and_the_weak_rows() {
        let ctx = ctx();
        let shown = settled(&ctx, &pair_report(), false);
        let texts: Vec<&str> = shown.texts.iter().map(|(t, _, _)| t.as_str()).collect();
        for expected in [
            HEALTH_PANE_TITLE,
            "3 items need attention",
            "All",
            "Reused",
            "Weak",
            "Old",
            "Breached",
            "REUSED",
            "One password, 2 items",
            "Bank",
            "Forum",
            "WEAK",
            "Old wiki",
            "3 characters, lowercase letters only",
        ] {
            assert!(texts.contains(&expected), "the page never painted {expected:?}: {texts:?}");
        }
        assert_eq!(
            texts.iter().filter(|t| **t == CHANGE_PASSWORD_LABEL).count(),
            3,
            "every finding row carries the Change password button: {texts:?}"
        );
    }

    /// A finding you cannot click through to the item is a dead end.
    #[test]
    fn a_row_opens_its_item_and_the_button_opens_the_editor() {
        let ctx = ctx();
        let report = pair_report();
        let shown = settled(&ctx, &report, false);
        let forum = shown.texts.iter().find(|(t, _, _)| t == "Forum").expect("Forum row").2.center();
        let mut action = HealthAction::None;
        let _ = page_frame(&ctx, &report, false, click_at(forum), &mut action);
        assert_eq!(action, HealthAction::Open("id-forum".to_string()), "the row opens its item");

        let shown = settled(&ctx, &report, false);
        // The wiki's button is the last one painted.
        let button = shown
            .texts
            .iter()
            .filter(|(t, _, _)| t == CHANGE_PASSWORD_LABEL)
            .map(|(_, _, r)| r.center())
            .next_back()
            .expect("a button");
        let mut action = HealthAction::None;
        let _ = page_frame(&ctx, &report, false, click_at(button), &mut action);
        assert_eq!(action, HealthAction::ChangePassword("id-wiki".to_string()));
    }

    #[test]
    fn a_click_off_every_row_asks_for_nothing() {
        let ctx = ctx();
        let report = pair_report();
        let mut action = HealthAction::None;
        let _ = page_frame(&ctx, &report, false, click_at(egui::pos2(10.0, 10.0)), &mut action);
        assert_eq!(action, HealthAction::None);
    }

    #[test]
    fn the_empty_state_paints_a_result_and_no_empty_sections() {
        let ctx = ctx();
        let report = report_for(&[login("a", "Bank", Some(STRONG_A)), login("b", "Shop", Some(STRONG_B))]);
        let shown = settled(&ctx, &report, false);
        let texts: Vec<&str> = shown.texts.iter().map(|(t, _, _)| t.as_str()).collect();
        assert!(texts.contains(&"No reused or weak passwords"), "{texts:?}");
        assert!(texts.contains(&"Checked 2 passwords."), "{texts:?}");
        assert!(texts.iter().any(|t| t.starts_with("No reused, weak or breached")), "{texts:?}");
        assert!(!texts.contains(&"REUSED"), "an empty REUSED band was painted: {texts:?}");
        assert!(!texts.contains(&"WEAK"), "an empty WEAK band was painted: {texts:?}");
        assert!(!texts.contains(&CHANGE_PASSWORD_LABEL), "a button with nothing to change: {texts:?}");
    }

    /// The breach footer is on screen in both states of the setting.
    #[test]
    fn the_breach_footer_is_painted_in_both_states() {
        let ctx = ctx();
        let report = report_for(&[login("a", "Bank", Some(STRONG_A))]);
        for on in [false, true] {
            let shown = settled(&ctx, &report, on);
            let joined = shown.texts.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>().join(" ");
            assert!(joined.contains("known breaches"), "with the setting {on}, no word about where it is: {joined}");
        }
    }

    #[test]
    fn an_item_with_no_name_is_still_a_row_you_can_find() {
        let ctx = ctx();
        let report = report_for(&[login("id", "  ", Some("abc12345"))]);
        let shown = settled(&ctx, &report, false);
        assert!(shown.text(UNNAMED_ITEM).is_some(), "the nameless finding painted no name");
        let named = report_for(&[login("id", "Bank", Some("abc12345"))]);
        let shown = settled(&ctx, &named, false);
        assert!(shown.text(UNNAMED_ITEM).is_none(), "a named item got the placeholder");
    }

    #[test]
    fn a_long_name_is_ellipsised_inside_its_row_and_clear_of_the_button() {
        let ctx = ctx();
        let long = "Visual Studio App Center | iOS, Android, Xamarin & React Native App Development and more besides";
        let report = report_for(&[login("id", long, Some("abc12345"))]);
        let shown = settled(&ctx, &report, false);
        let (drawn, _, rect) = shown.texts.iter().find(|(t, _, _)| t.starts_with("Visual")).expect("the name");
        assert!(drawn.ends_with('\u{2026}'), "the long name was not cut: {drawn:?}");
        let (_, _, button) = shown.texts.iter().find(|(t, _, _)| t == CHANGE_PASSWORD_LABEL).expect("the button");
        assert!(rect.right() <= button.left(), "the name runs under the button");
    }

    /// The column is capped at 760 and centred, as the design has it.
    #[test]
    fn the_column_is_capped_and_centred() {
        let ctx = ctx();
        let shown = settled(&ctx, &pair_report(), false);
        let title = shown.text(HEALTH_PANE_TITLE).expect("the title").2;
        let expected_left = (PAGE.x - COLUMN_MAX) / 2.0;
        assert!((title.left() - expected_left).abs() < 3.0, "the title starts at {}, not {expected_left}", title.left());
    }

    /// Rows in one card are one row apart (a row's tile must not move the
    /// cursor back up into the row, which stacked them 16 points too close).
    #[test]
    fn rows_in_a_card_are_a_row_and_a_rule_apart() {
        let ctx = ctx();
        let shown = settled(&ctx, &pair_report(), false);
        let tops: Vec<f32> = shown
            .texts
            .iter()
            .filter(|(t, _, _)| t == CHANGE_PASSWORD_LABEL)
            .map(|(_, _, r)| r.center().y)
            .collect();
        assert!(tops.len() >= 2, "{tops:?}");
        assert!((tops[1] - tops[0] - (ROW_HEIGHT + 1.0)).abs() < 1.5, "the rows are {} apart, not a row and its rule", tops[1] - tops[0]);
    }

    /// The reason colours: weak red, reused amber.
    #[test]
    fn a_reason_is_in_its_categorys_colour() {
        let ctx = ctx();
        let shown = settled(&ctx, &pair_report(), false);
        let p = crate::kit::tokens::palette(&ctx);
        let colour_of = |needle: &str| shown.text(needle).unwrap_or_else(|| panic!("no {needle}")).1;
        assert_eq!(colour_of("Also used by"), AMBER);
        assert_eq!(colour_of("3 characters"), p.error);
    }
}

#[cfg(test)]
mod old_password_tests {
    //! D5.
    use super::*;

    fn login(extra: serde_json::Value) -> VaultItem {
        let mut base = serde_json::json!({
            "id": "1", "name": "Ledgerline", "type": 1,
            "login": { "username": "a", "password": "correct-horse-battery-staple-7", "uris": [] }
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                if k == "passwordRevisionDate" {
                    base["login"].as_object_mut().unwrap().insert(k.clone(), v.clone());
                } else {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
        serde_json::from_value(base).unwrap()
    }

    fn today() -> i64 {
        super::super::detail::date_days("2026-11-19").unwrap()
    }

    #[test]
    fn a_password_unchanged_for_a_year_is_old() {
        let old = login(serde_json::json!({ "passwordRevisionDate": "2025-10-01T00:00:00Z", "revisionDate": "2026-11-18T00:00:00Z" }));
        let found = old_items(&[old], today());
        assert_eq!(found.len(), 1, "an edit last week does not make an old password new");
        assert_eq!(old_detail(&found[0]), "Not changed in 414 days");
        let fresh = login(serde_json::json!({ "passwordRevisionDate": "2026-06-01T00:00:00Z" }));
        assert!(old_items(&[fresh], today()).is_empty());
        let created = login(serde_json::json!({ "creationDate": "2024-01-01T00:00:00Z" }));
        assert_eq!(old_items(&[created], today()).len(), 1, "no change since it was created");
    }

    #[test]
    fn the_newest_history_date_counts_whatever_the_order() {
        let changed = login(serde_json::json!({
            "creationDate": "2020-01-01T00:00:00Z",
            "passwordHistory": [
                { "password": "first-one", "lastUsedDate": "2021-01-01T00:00:00Z" },
                { "password": "second-one", "lastUsedDate": "2026-10-01T00:00:00Z" }
            ]
        }));
        assert!(old_items(&[changed], today()).is_empty(), "changed last month, oldest-first history");
        let undated = login(serde_json::json!({
            "creationDate": "2020-01-01T00:00:00Z",
            "passwordHistory": [{ "password": "x" }]
        }));
        assert!(old_items(&[undated], today()).is_empty(), "changed at an unknown time is not old");
    }

    #[test]
    fn an_item_with_no_password_date_is_not_old() {
        let unknown = login(serde_json::json!({ "revisionDate": "2020-01-01T00:00:00Z" }));
        assert!(old_items(&[unknown], today()).is_empty(), "revisionDate is never asked");
    }

    #[test]
    fn old_counts_in_the_findings() {
        let report = HealthReport {
            old: vec![OldItem { item: FoundItem { id: "1".into(), name: "x".into() }, days: 400 }],
            ..HealthReport::default()
        };
        assert_eq!(report.flagged_items(), 1);
        assert_eq!(report.shown_under(HealthFilter::Old), 1);
        assert_eq!(HealthFilter::Old.label(), "Old");
    }
}

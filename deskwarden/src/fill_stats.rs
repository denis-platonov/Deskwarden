//! Local-only fill-count analytics for the vault window's detail pane
//! ("Filled 41 times"). Deliberately never touches the vault: this is
//! per-device usage trivia, not data worth a sync round-trip or a write on
//! every single autofill.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone)]
pub struct FillStats {
    path: PathBuf,
}

/// **What a fill attempt actually did**, as opposed to what dispatching it
/// returned.
///
/// The sequence path performs its typing on a thread (see
/// [`crate::injector::SendInputFiller::fill_sequence`]), so the value that
/// comes back to the UI thread means "started", not "typed". This is the
/// value that means "typed", and it arrives later, from the thread that
/// knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillOutcome {
    /// Every keystroke the fill planned was performed.
    Typed,
    /// Typing began and stopped early: the target window stopped being in
    /// front part-way through, or a keystroke failed. Some prefix of the
    /// sequence reached the window; *which* prefix is not knowable from here,
    /// and deliberately is not represented -- see [`counts_as_a_fill`].
    Partial,
    /// Not one keystroke was performed. A sequence refused before it started
    /// (an unresolvable `{S:Missing}`, an unknown token, a modifier before
    /// text, over the 60s bound, or another sequence already typing), or a
    /// default fill that failed.
    NotTyped,
}

/// **The whole of the "did that count as a fill?" decision, as a pure
/// function.**
///
/// Called from the typing thread, once it knows. Nothing here touches a file,
/// a window or a clock, so every branch is reachable from a unit test -- which
/// is the point: the arm this replaces (`Ok(()) => fill_stats.record_fill(..)`)
/// fired when a thread had been *spawned*, and could be deleted outright with
/// the suite staying green.
///
/// # A partial sequence does not count
///
/// Three of five steps typed and then the user alt-tabbed: the username went
/// in, the password did not. That is **not** a fill, for two reasons. The
/// count is shown to the user as "Filled N times" and read as "N logins this
/// item completed", and the number that drives the picker's most-recently-used
/// ranking should favour the items that actually work in the window in front.
/// And a partial fill is one the user has to *redo* -- the retry records its
/// own outcome, so counting the abort as well would score one login twice.
///
/// The opposite choice is defensible ("the item was used"), which is exactly
/// why the answer lives in one named function with its own tests rather than
/// being implied by which arm of a `match` a call sits in.
pub fn counts_as_a_fill(outcome: FillOutcome) -> bool {
    match outcome {
        FillOutcome::Typed => true,
        FillOutcome::Partial => false,
        FillOutcome::NotTyped => false,
    }
}

impl FillStats {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Increments `item_id`'s count and persists immediately. Best-effort:
    /// a failure to read or write the file is not the caller's problem --
    /// analytics that silently don't update this one time is a much smaller
    /// deal than a failed autofill.
    pub fn record_fill(&self, item_id: &str) {
        let mut counts = self.load();
        *counts.entry(item_id.to_string()).or_insert(0) += 1;
        let _ = self.save(&counts);
    }

    pub fn count(&self, item_id: &str) -> u32 {
        self.load().get(item_id).copied().unwrap_or(0)
    }

    fn load(&self) -> HashMap<String, u32> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, counts: &HashMap<String, u32>) -> std::io::Result<()> {
        let json = serde_json::to_string(counts)?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn unique_path(label: &str) -> PathBuf {
        temp_dir().join(format!("deskwarden-test-fill-stats-{label}-{}.json", std::process::id()))
    }

    // -- counts_as_a_fill, the pure decision --------------------------------

    /// **A fill that typed the whole sequence counts.** The positive control:
    /// without it, `counts_as_a_fill` could return `false` unconditionally and
    /// every negative test below would still pass.
    #[test]
    fn a_completed_fill_counts() {
        assert!(counts_as_a_fill(FillOutcome::Typed));
    }

    /// **A sequence abandoned part-way does not count.**
    ///
    /// The case the whole threaded design exists to handle: the user alt-tabs
    /// after the username and before the password. The item was not filled,
    /// the user must do it again, and the retry will record its own outcome --
    /// counting this too would score one login twice and float an item that
    /// half-worked above items that work.
    #[test]
    fn a_sequence_abandoned_part_way_does_not_count() {
        assert!(!counts_as_a_fill(FillOutcome::Partial));
    }

    /// **A sequence that refused before the first keystroke does not count.**
    /// An unresolvable `{S:PIN}`, an unknown token, a modifier before text, a
    /// plan over the 60s bound, or another sequence already typing: nothing
    /// reached the window, so nothing was filled.
    #[test]
    fn a_fill_that_typed_nothing_does_not_count() {
        assert!(!counts_as_a_fill(FillOutcome::NotTyped));
    }

    /// The three answers, together, in one place: exactly one outcome counts.
    /// Widening the decision to any second variant fails here as well as in
    /// the test that names that variant.
    #[test]
    fn exactly_one_outcome_counts_as_a_fill() {
        let counted: Vec<FillOutcome> =
            [FillOutcome::Typed, FillOutcome::Partial, FillOutcome::NotTyped]
                .into_iter()
                .filter(|o| counts_as_a_fill(*o))
                .collect();
        assert_eq!(counted, vec![FillOutcome::Typed], "the fill/no-fill line moved");
    }

    // -- the store ----------------------------------------------------------

    #[test]
    fn a_fresh_item_has_zero_fills() {
        let stats = FillStats::new(unique_path("fresh"));
        assert_eq!(stats.count("item-1"), 0);
    }

    #[test]
    fn recording_a_fill_increments_and_persists() {
        let path = unique_path("increment");
        let stats = FillStats::new(path.clone());
        stats.record_fill("item-1");
        stats.record_fill("item-1");
        stats.record_fill("item-2");

        assert_eq!(stats.count("item-1"), 2);
        assert_eq!(stats.count("item-2"), 1);

        // A fresh handle to the same path sees the persisted counts.
        let reopened = FillStats::new(path.clone());
        assert_eq!(reopened.count("item-1"), 2);

        std::fs::remove_file(&path).ok();
    }
}

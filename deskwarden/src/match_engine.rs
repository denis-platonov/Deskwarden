use crate::app_match::AppMatch;
use crate::window_watch::is_host_process;
use std::collections::HashMap;

pub struct MatchEngine {
    by_process: HashMap<String, (String, AppMatch)>,
    unmatchable_hosts: Vec<(String, String)>,
}

impl MatchEngine {
    pub fn new() -> Self {
        Self { by_process: HashMap::new(), unmatchable_hosts: Vec::new() }
    }

    /// Rebuilds the lookup table, **dropping every entry whose process is a
    /// window host** ([`crate::window_watch::is_host_process`]).
    ///
    /// This is the repair path for matches already sitting in the user's
    /// vault. Before the foreground watcher learned to attribute a hosted
    /// window, saving a match for a Store app recorded
    /// `ApplicationFrameHost.exe` -- the reported bug: one entry that fires on
    /// every Store app the user focuses, which is not "too eager", it is
    /// wrong by construction. Such an entry cannot be *narrowed* into a
    /// correct one, because the name carries no information about which app
    /// was meant.
    ///
    /// **Dropped here, and nowhere else.** This app does not rewrite the
    /// user's vault behind their back, so the field stays exactly as they
    /// saved it; what changes is that autofill stops acting on it. The
    /// dropped entries are kept in [`Self::unmatchable_hosts`] so the "Add
    /// app..." flow can tell the user which item is affected and let them
    /// replace it themselves.
    pub fn rebuild(&mut self, entries: &[(String, AppMatch)]) {
        self.by_process = entries
            .iter()
            .filter(|(_, m)| !is_host_process(&m.process))
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();
        self.unmatchable_hosts = entries
            .iter()
            .filter(|(_, m)| is_host_process(&m.process))
            .map(|(item_id, m)| (item_id.clone(), m.process.clone()))
            .collect();

        // Logged here rather than at the four `rebuild` call sites in `main`,
        // because there are four of them and a warning that only three carry
        // is a warning that goes missing on the fourth path.
        for (item_id, process) in &self.unmatchable_hosts {
            log::warn!(
                "ignoring the app match on vault item {item_id}: {process} owns the top-level \
                 window for every Microsoft Store app, so this match would fire on all of them. \
                 The vault is unchanged -- re-add the app from \"Add app...\" to replace it"
            );
        }
    }

    /// The `(item_id, process)` pairs [`Self::rebuild`] refused to load
    /// because their process is a window host, in the order they arrived.
    ///
    /// Empty in the ordinary case. Non-empty means the user has a stored
    /// match that this app is deliberately ignoring, and they have not been
    /// told anything about it yet -- so every caller of this is a place that
    /// tells them.
    pub fn unmatchable_hosts(&self) -> &[(String, String)] {
        &self.unmatchable_hosts
    }

    /// Drops every match, so nothing can be looked up until a rebuild.
    ///
    /// Pairs with `VaultCache::clear`: an empty cache and a populated engine
    /// are an inconsistent pair (review 13's Minor 3). Left populated after
    /// a lock the user then declined to unlock, a matched process still
    /// raises the autofill prompt, and the fill then finds nothing --
    /// `handle_match` looks the item up in the now-empty cache, misses, and
    /// falls through to a `bridge.get_item` with an id belonging to an
    /// account the app is no longer signed into. Clearing both together
    /// means a locked app is simply inert, which is what "locked" should
    /// look like.
    pub fn clear(&mut self) {
        self.by_process.clear();
        self.unmatchable_hosts.clear();
    }

    pub fn lookup(&self, exe_name: &str) -> Option<(&str, &AppMatch)> {
        self.by_process
            .get(&exe_name.to_lowercase())
            .map(|(id, m)| (id.as_str(), m))
    }
}

impl Default for MatchEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::{AppMatch, TriggerMode};

    fn entry(item_id: &str, process: &str, trigger: TriggerMode) -> (String, AppMatch) {
        (item_id.to_string(), AppMatch { process: process.to_string(), trigger })
    }

    #[test]
    fn empty_engine_matches_nothing() {
        let engine = MatchEngine::new();
        assert!(engine.lookup("anything.exe").is_none());
    }

    #[test]
    fn matches_exact_process_name() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Prompt)]);

        let (id, m) = engine.lookup("RockstarGamesLauncher.exe").unwrap();
        assert_eq!(id, "1");
        assert_eq!(m.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn clear_drops_every_match_so_a_locked_app_is_inert() {
        // Review 13's Minor 3: after lock recovery is dismissed the cache is
        // empty, so the engine must be too -- otherwise a matched process
        // still raises the autofill prompt for an account the app is no
        // longer signed into, and the fill can only fail.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Prompt)]);

        engine.clear();

        assert!(engine.lookup("RockstarGamesLauncher.exe").is_none());
    }

    #[test]
    fn matches_case_insensitively() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Auto)]);

        assert!(engine.lookup("rockstargameslauncher.EXE").is_some());
    }

    #[test]
    fn returns_none_for_unrelated_process() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);

        assert!(engine.lookup("notepad.exe").is_none());
    }

    /// The user's stored match: `ApplicationFrameHost.exe`, saved when they
    /// pointed at KeepSolid (a Store app), which then fired on Speedtest --
    /// and on every other Store app -- because that one exe owns the window
    /// for all of them.
    #[test]
    fn a_match_stored_against_the_frame_host_is_not_loaded() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt)]);

        // Deleting the `filter` in `rebuild` gives
        //     "the frame host is still matched, so every Store app fills this item"
        assert!(
            engine.lookup("ApplicationFrameHost.exe").is_none(),
            "the frame host is still matched, so every Store app fills this item"
        );
    }

    #[test]
    fn dropping_a_host_entry_does_not_drop_the_good_ones_beside_it() {
        // The positive control for the test above, and the mutation it kills:
        // a `rebuild` that filtered out everything (or simply stopped
        // building the table) would satisfy "the host is not matched" while
        // making the whole feature inert. Inverting the filter's sense gives
        //     "a real app stopped being matched"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
        ]);

        assert!(
            engine.lookup("Ledgerline.exe").is_some(),
            "a real app stopped being matched"
        );
        assert!(engine.lookup("ApplicationFrameHost.exe").is_none());
    }

    #[test]
    fn a_dropped_host_entry_is_reported_so_the_user_can_be_told_which_item_it_is() {
        // Nothing is rewritten in the vault, so the ONLY way the user ever
        // learns their match went quiet is this list. Returning an empty
        // slice from `unmatchable_hosts` gives
        //     left: []  right: [("keepsolid", "ApplicationFrameHost.exe")]
        let mut engine = MatchEngine::new();
        engine.rebuild(&[
            entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt),
            entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto),
        ]);

        assert_eq!(
            engine.unmatchable_hosts(),
            [("keepsolid".to_string(), "ApplicationFrameHost.exe".to_string())]
        );
    }

    #[test]
    fn an_ordinary_vault_reports_nothing_unmatchable() {
        // Paired with the test above: a `unmatchable_hosts` that reported
        // every entry, or one that reported a fixed value, fails here.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("ledgerline", "Ledgerline.exe", TriggerMode::Auto)]);

        assert!(engine.unmatchable_hosts().is_empty());
    }

    #[test]
    fn a_rebuild_without_host_entries_clears_a_previous_report() {
        // `unmatchable_hosts` is rebuilt, not appended to: once the user has
        // replaced the bad match, the picker must stop telling them about it.
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt)]);
        assert_eq!(engine.unmatchable_hosts().len(), 1, "precondition");

        engine.rebuild(&[entry("keepsolid", "KeepSolid.exe", TriggerMode::Prompt)]);

        assert!(engine.unmatchable_hosts().is_empty());
        assert!(
            engine.lookup("KeepSolid.exe").is_some(),
            "and the replacement must actually be live"
        );
    }

    #[test]
    fn clear_drops_the_unmatchable_report_too() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt)]);
        engine.clear();
        assert!(engine.unmatchable_hosts().is_empty());
    }

    #[test]
    fn rebuild_replaces_previous_entries() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);
        engine.rebuild(&[entry("2", "notepad.exe", TriggerMode::Auto)]);

        assert!(engine.lookup("mabl.exe").is_none());
        assert!(engine.lookup("notepad.exe").is_some());
    }
}

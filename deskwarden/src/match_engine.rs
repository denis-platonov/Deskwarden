use crate::app_match::AppMatch;
use crate::window_watch::is_host_process;
use std::collections::HashMap;

pub struct MatchEngine {
    by_process: HashMap<String, (String, AppMatch)>,
}

impl MatchEngine {
    pub fn new() -> Self {
        Self { by_process: HashMap::new() }
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
    /// saved it; what changes is that autofill stops acting on it.
    ///
    /// **The user is told by `picker_ui::existing_host_match_notice`, not by
    /// anything this type hands back.** A `Vec` of the dropped pairs and an
    /// `unmatchable_hosts()` accessor lived here for exactly that purpose and
    /// never acquired a caller -- complete, correct and unreachable, which a
    /// `pub` function in a lib crate produces no warning for. `MatchEngine` is
    /// owned by `main`'s event loop and the picker is a separate window with
    /// no reference to it, so the report had nowhere to go. The picker derives
    /// the same fact where it is actually usable: straight off the target
    /// item's own `deskwarden:app-match` field, on the screen where Save
    /// replaces it. Deleting the accessor therefore cost nothing but its own
    /// tests. The `log::warn!` below is what remains, and it is a trace for
    /// the developer, not a surface for the user.
    ///
    /// So a user whose match went quiet is told **when they next open "Add
    /// app..." on that item**, and not at the moment it goes quiet. Closing
    /// that gap needs a channel out of `main`'s loop -- a tray balloon, say --
    /// which is a decision about `main.rs`, not about this file.
    pub fn rebuild(&mut self, entries: &[(String, AppMatch)]) {
        self.by_process = entries
            .iter()
            .filter(|(_, m)| !is_host_process(&m.process))
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();

        // Logged here rather than at the four `rebuild` call sites in `main`,
        // because there are four of them and a warning that only three carry
        // is a warning that goes missing on the fourth path.
        for (item_id, m) in entries.iter().filter(|(_, m)| is_host_process(&m.process)) {
            let process = &m.process;
            log::warn!(
                "ignoring the app match on vault item {item_id}: {process} owns the top-level \
                 window for every Microsoft Store app, so this match would fire on all of them. \
                 The vault is unchanged -- re-add the app from \"Add app...\" to replace it"
            );
        }
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
        (item_id.to_string(), AppMatch::for_process(process, trigger))
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
    fn replacing_a_host_entry_with_a_real_one_makes_the_replacement_live() {
        // What the deleted `unmatchable_hosts` report used to be asserted
        // through, said in terms of the only thing `rebuild` actually changes:
        // the lookup table. Deleting the `self.by_process = ...` assignment
        // (so a rebuild keeps the previous table) gives
        //     "the replacement must actually be live"
        // and inverting the filter's sense gives
        //     "the bad entry survived its own replacement"
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("keepsolid", "ApplicationFrameHost.exe", TriggerMode::Prompt)]);
        assert!(
            engine.lookup("ApplicationFrameHost.exe").is_none(),
            "precondition: the host entry was never loaded"
        );

        engine.rebuild(&[entry("keepsolid", "KeepSolid.exe", TriggerMode::Prompt)]);

        assert!(
            engine.lookup("KeepSolid.exe").is_some(),
            "the replacement must actually be live"
        );
        assert!(
            engine.lookup("ApplicationFrameHost.exe").is_none(),
            "the bad entry survived its own replacement"
        );
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

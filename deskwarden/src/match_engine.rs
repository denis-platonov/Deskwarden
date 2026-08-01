use crate::app_match::AppMatch;
use std::collections::HashMap;

pub struct MatchEngine {
    by_process: HashMap<String, (String, AppMatch)>,
}

impl MatchEngine {
    pub fn new() -> Self {
        Self { by_process: HashMap::new() }
    }

    pub fn rebuild(&mut self, entries: &[(String, AppMatch)]) {
        self.by_process = entries
            .iter()
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();
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

    #[test]
    fn rebuild_replaces_previous_entries() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);
        engine.rebuild(&[entry("2", "notepad.exe", TriggerMode::Auto)]);

        assert!(engine.lookup("mabl.exe").is_none());
        assert!(engine.lookup("notepad.exe").is_some());
    }
}

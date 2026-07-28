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

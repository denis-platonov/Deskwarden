//! Local-only fill-count analytics for the vault window's detail pane
//! ("Filled 41 times"). Deliberately never touches the vault: this is
//! per-device usage trivia, not data worth a sync round-trip or a write on
//! every single autofill.

use std::collections::HashMap;
use std::path::PathBuf;

pub struct FillStats {
    path: PathBuf,
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

//! User preferences, persisted as `settings.json` in the config directory.
//!
//! Follows `fill_stats`'s pattern: plain serde over a small struct, with
//! every read falling back to defaults. A settings file is never a reason
//! the app cannot start, so a missing, partial, or corrupt file is a
//! silent fall-back rather than an error.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Auto-lock timeout used when the stored value is absent. Matches the
/// constant this replaces in `vault_window`, which was marked "hardcoded
/// until the 3e preferences window exists".
const DEFAULT_AUTO_LOCK_MINUTES: u64 = 15;

/// Floor applied to `auto_lock_minutes` by `auto_lock_timeout`, regardless of
/// what's stored on disk.
///
/// `0` is the natural value to hand-write into `settings.json` for "never
/// lock", but `auto_lock_timeout`'s result feeds `vault_window::run`'s
/// `last_activity.elapsed() >= auto_lock` check, where a zero-length timeout
/// is true on the very first frame -- the window would close itself with
/// `locked = true` before the user can do anything, on every single open,
/// forcing a fresh master-password re-auth each time. There's no "never
/// lock" mode in this app (see the design spec), so a zero or corrupt value
/// is clamped up to the shortest lock period that's still usable rather than
/// being treated as meaningful.
const MIN_AUTO_LOCK_MINUTES: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether `bw serve` stays running while the vault is unlocked.
    ///
    /// `true` (the default) is today's behaviour: everything is instant and
    /// the backend holds ~111 MB at idle. `false` runs it only while the
    /// vault window is open; reads come from `VaultCache` either way, so
    /// autofill is unaffected.
    pub keep_backend_running: bool,
    /// Idle minutes before the vault window locks itself.
    pub auto_lock_minutes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_backend_running: true,
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    pub fn auto_lock_timeout(&self) -> Duration {
        // `.max(MIN_AUTO_LOCK_MINUTES)` first, so the floor applies to the
        // stored value itself rather than to a possibly-already-overflowed
        // product. `saturating_mul` handles the separate case of an absurdly
        // large stored value (a corrupt or hand-edited file): plain `* 60`
        // would overflow `u64` and panic in a debug build (or silently wrap
        // to a tiny duration in release), where saturating to
        // `Duration::from_secs(u64::MAX)` -- effectively forever -- is a far
        // safer failure mode for a *lock* timeout to have.
        let minutes = self.auto_lock_minutes.max(MIN_AUTO_LOCK_MINUTES);
        Duration::from_secs(minutes.saturating_mul(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = temp_dir().join(format!("deskwarden-settings-test-{name}.json"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn the_default_preserves_todays_behaviour() {
        let s = Settings::default();
        assert!(s.keep_backend_running);
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(15 * 60));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let path = temp_path("round-trip");
        let written = Settings {
            keep_backend_running: false,
            auto_lock_minutes: 5,
        };
        written.save(&path).unwrap();
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        assert_eq!(Settings::load(&temp_path("absent")), Settings::default());
    }

    #[test]
    fn a_partial_file_keeps_defaults_for_absent_fields() {
        // `#[serde(default)]` on the struct is what makes this work: a file
        // written by an older build must not fail to parse once a field is
        // added.
        let path = temp_path("partial");
        std::fs::write(&path, r#"{"keep_backend_running": false}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, DEFAULT_AUTO_LOCK_MINUTES);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_malformed_file_yields_defaults_rather_than_failing() {
        let path = temp_path("malformed");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zero_minutes_clamps_to_the_minimum_instead_of_locking_instantly() {
        // The regression this guards: `0 * 60 == 0`, and a zero-length
        // timeout is already elapsed on the vault window's very first frame,
        // closing it immediately with `locked = true` and forcing a fresh
        // re-auth on every open.
        let s = Settings { auto_lock_minutes: 0, ..Settings::default() };
        assert_eq!(
            s.auto_lock_timeout(),
            Duration::from_secs(MIN_AUTO_LOCK_MINUTES * 60)
        );
    }

    #[test]
    fn a_normal_value_is_used_as_is() {
        let s = Settings { auto_lock_minutes: 5, ..Settings::default() };
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(5 * 60));
    }

    #[test]
    fn an_absurd_value_saturates_instead_of_overflowing() {
        // A hand-edited (or corrupted) settings.json could contain anything
        // that fits in a u64; `* 60` on the largest of those would overflow
        // rather than produce a meaningful timeout.
        let s = Settings { auto_lock_minutes: u64::MAX, ..Settings::default() };
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(u64::MAX));
    }
}

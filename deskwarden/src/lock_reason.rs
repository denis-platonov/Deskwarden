//! **Why the vault is locked right now**, for the line the lock screen prints
//! under its title.
//!
//! The screen used to describe the *policy* ("Locks after 15 minutes of no
//! activity") whatever had actually locked the vault, which is wrong the moment
//! the user pressed Ctrl+L or Windows went to sleep. The cause is known at the
//! source -- the idle timer, the lock button, the session-change message -- so
//! the source records it here and the lock screen reads it back.
//!
//! # How it crosses the process boundary
//!
//! The lock screen is drawn by a `--ui` process; the lock may have been made by
//! the daemon (Win+L, sleep, the tray) or by that very process (idle, Ctrl+L,
//! an expired session). The two already share the config directory, which is
//! how the search term and the landing reach a new window, so the reason is one
//! more small file there: `lock-reason.json`, written atomically, **read and
//! not taken** (the lock screen may be closed and reopened while the vault
//! stays locked), and forgotten when the vault is unlocked and when the daemon
//! starts. It carries a word and a number of minutes; no secret.
//!
//! Everything here is local state. Nothing reaches the server.

use std::io;
use std::path::{Path, PathBuf};

use crate::away_lock::AwayEvent;

/// Why the vault was locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockReason {
    /// The idle timer fired after this many minutes -- the period that was in
    /// force when it did, not whatever the setting says now.
    Idle { minutes: u64 },
    /// The user asked: the tray's Lock, Ctrl+L, the vault window's lock row or
    /// the account menu's.
    User,
    /// Windows locked the session (Win+L, a session switch or a disconnect).
    WindowsLocked,
    /// The PC went to sleep.
    Sleep,
    /// The server rejected the session, so there was nothing left to keep
    /// open.
    SessionExpired,
}

impl LockReason {
    fn word(self) -> &'static str {
        match self {
            LockReason::Idle { .. } => "idle",
            LockReason::User => "user",
            LockReason::WindowsLocked => "windows",
            LockReason::Sleep => "sleep",
            LockReason::SessionExpired => "expired",
        }
    }

    fn from_parts(word: &str, minutes: Option<u64>) -> Option<Self> {
        Some(match word {
            "idle" => LockReason::Idle { minutes: minutes? },
            "user" => LockReason::User,
            "windows" => LockReason::WindowsLocked,
            "sleep" => LockReason::Sleep,
            "expired" => LockReason::SessionExpired,
            _ => return None,
        })
    }

    /// The reason for an idle lock after `timeout`: whole minutes, never zero
    /// (the settings clamp the period to at least one).
    #[must_use]
    pub fn idle_after(timeout: std::time::Duration) -> Self {
        LockReason::Idle { minutes: (timeout.as_secs() / 60).max(1) }
    }

    /// The lock a Windows report made.
    #[must_use]
    pub fn from_away(event: AwayEvent) -> Self {
        match event {
            AwayEvent::WorkstationLocked => LockReason::WindowsLocked,
            AwayEvent::Suspending => LockReason::Sleep,
        }
    }
}

/// The line under "Your vault is locked".
///
/// `None` is a lock nobody recorded -- the app just started with no key in
/// memory -- and answers with the policy, which is still true and useful:
/// `policy_minutes` is the auto-lock period, `None` for off.
#[must_use]
pub fn line(reason: Option<LockReason>, policy_minutes: Option<u64>) -> String {
    use crate::i18n;
    match reason {
        Some(LockReason::Idle { minutes }) => i18n::tn("auth.locked_idle", minutes, &[]),
        Some(LockReason::User) => i18n::t("auth.locked_user").to_string(),
        Some(LockReason::WindowsLocked) => i18n::t("auth.locked_windows").to_string(),
        Some(LockReason::Sleep) => i18n::t("auth.locked_sleep").to_string(),
        Some(LockReason::SessionExpired) => i18n::t("auth.locked_expired").to_string(),
        None => match policy_minutes {
            Some(minutes) => i18n::tn("auth.lock_after", minutes, &[]),
            None => i18n::t("auth.lock_auto_off").to_string(),
        },
    }
}

/// Where the reason lives, in the config directory.
#[must_use]
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("lock-reason.json")
}

/// Leave the reason, atomically (temp-then-rename).
pub fn write(path: &Path, reason: LockReason) -> io::Result<()> {
    let minutes = match reason {
        LockReason::Idle { minutes } => Some(minutes),
        _ => None,
    };
    let json = serde_json::json!({ "reason": reason.word(), "minutes": minutes }).to_string();
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

/// The recorded reason. Absent, unreadable and unknown are all `None`, which
/// the lock screen answers with the policy line.
#[must_use]
pub fn read(path: &Path) -> Option<LockReason> {
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    LockReason::from_parts(value.get("reason")?.as_str()?, value.get("minutes").and_then(|m| m.as_u64()))
}

/// Forget the reason: the vault is open again, or the app has just started.
pub fn forget(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {}: {e}", path.display());
        }
    }
}

/// Record a lock from a process that has no `config_dir` in hand (the vault
/// window). Best effort: a lock whose reason could not be written is still a
/// lock, and the screen falls back to the policy line.
pub fn note(reason: LockReason) {
    let Some(dir) = crate::settings::config_dir() else { return };
    if let Err(e) = write(&path(&dir), reason) {
        log::warn!("could not record why the vault locked ({reason:?}): {e}");
    }
}

/// [`read`] from the settings directory, for the lock screen.
#[must_use]
pub fn current() -> Option<LockReason> {
    read(&path(&crate::settings::config_dir()?))
}

/// [`forget`] in the settings directory, for the unlock paths that have no
/// `config_dir` in hand.
pub fn forget_current() {
    if let Some(dir) = crate::settings::config_dir() {
        forget(&path(&dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deskwarden-lock-reason-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        path(&dir)
    }

    #[test]
    fn every_reason_survives_the_file() {
        let p = scratch("round-trip");
        for reason in [
            LockReason::Idle { minutes: 37 },
            LockReason::User,
            LockReason::WindowsLocked,
            LockReason::Sleep,
            LockReason::SessionExpired,
        ] {
            write(&p, reason).unwrap();
            assert_eq!(read(&p), Some(reason));
        }
        forget(&p);
        assert_eq!(read(&p), None, "forgotten means no reason");
        forget(&p);
    }

    #[test]
    fn a_damaged_or_unknown_file_is_no_reason() {
        let p = scratch("damaged");
        for text in ["", "not json", r#"{"reason":"mystery"}"#, r#"{"reason":"idle"}"#, "{}"] {
            std::fs::write(&p, text).unwrap();
            assert_eq!(read(&p), None, "{text:?}");
        }
        forget(&p);
    }

    #[test]
    fn windows_events_map_to_their_own_reasons() {
        assert_eq!(LockReason::from_away(AwayEvent::WorkstationLocked), LockReason::WindowsLocked);
        assert_eq!(LockReason::from_away(AwayEvent::Suspending), LockReason::Sleep);
    }

    #[test]
    fn the_idle_period_is_whole_minutes_and_never_zero() {
        assert_eq!(LockReason::idle_after(Duration::from_secs(15 * 60)), LockReason::Idle { minutes: 15 });
        assert_eq!(LockReason::idle_after(Duration::from_secs(1)), LockReason::Idle { minutes: 1 });
    }

    #[test]
    fn each_reason_reads_as_itself_and_a_missing_one_as_the_policy() {
        assert_eq!(line(Some(LockReason::Idle { minutes: 1 }), Some(15)), "Locked after 1 minute of no activity");
        assert_eq!(line(Some(LockReason::Idle { minutes: 7 }), None), "Locked after 7 minutes of no activity");
        assert_eq!(line(Some(LockReason::User), Some(15)), "You locked the vault");
        assert_eq!(line(Some(LockReason::WindowsLocked), None), "Locked when Windows was locked");
        assert_eq!(line(Some(LockReason::Sleep), None), "Locked when the PC went to sleep");
        assert_eq!(line(Some(LockReason::SessionExpired), None), "Locked because your session ended");
        assert_eq!(line(None, Some(15)), "Locks after 15 minutes of no activity");
        assert_eq!(line(None, None), "Auto-lock is off");
    }
}

//! File-backed logging setup.
//!
//! `nodewarden-native` runs as a background tray app with no console attached,
//! so `env_logger`'s default stderr target is invisible to the user (and to
//! anyone diagnosing a failure). Everything is routed to `nodewarden.log` in
//! the same config directory `session_store` already uses, which is the one
//! directory we know exists and is writable per-user.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// Name of the log file created inside the config directory.
pub const LOG_FILE_NAME: &str = "nodewarden.log";

/// Name of the single retained previous log.
pub const OLD_LOG_FILE_NAME: &str = "nodewarden.log.old";

/// Size at which the log is rotated on startup.
///
/// This is a tray app that can stay up for weeks, and at `RUST_LOG=debug` it
/// logs a line for every suppressed foreground event -- i.e. roughly every
/// window switch the user makes. Appending forever with no cap would grow the
/// file without bound.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Returns the path the log file will be written to for a given config dir.
pub fn log_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LOG_FILE_NAME)
}

/// Returns the path the previous log is rotated to.
pub fn old_log_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(OLD_LOG_FILE_NAME)
}

/// Rotates `path` to `old_path` if it is at least `max_bytes` long.
///
/// Deliberately minimal: one generation, checked once at startup rather than
/// on every write. That is enough to bound disk use at roughly `2 *
/// max_bytes` without pulling in a rotating-logger dependency or paying a
/// size check per log line, and a run that manages to exceed the cap within a
/// single session still has the whole of *this* run's log intact.
///
/// Returns whether a rotation happened. A missing or unreadable log is not an
/// error -- there is simply nothing to rotate.
pub fn rotate_if_oversized(path: &Path, old_path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    let len = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(_) => return Ok(false),
    };

    if len < max_bytes {
        return Ok(false);
    }

    // `rename` replaces an existing destination on Windows only via
    // `MoveFileEx`; `std::fs::rename` does handle that (it passes
    // `MOVEFILE_REPLACE_EXISTING`), but an open handle on the old file would
    // still make it fail, so a failure here must not stop the app from
    // logging.
    std::fs::rename(path, old_path)?;
    Ok(true)
}

/// Initialises the global logger, appending to `<config_dir>/nodewarden.log`.
///
/// Rotates the log first if the previous runs left it oversized, so a
/// long-lived tray app can't grow it without bound. The default level is
/// `info`; it can be overridden at runtime with the standard `RUST_LOG`
/// environment variable. Returns the path actually being logged to, or `Err`
/// if the log file could not be opened (in which case logging falls back to
/// stderr so the process still starts).
pub fn init(config_dir: &Path) -> Result<PathBuf, String> {
    let path = log_file_path(config_dir);

    // Before opening for append, and before any logger exists to report it
    // through -- a failed rotation is not a reason to start without logging,
    // so it's swallowed here and the oversized file simply keeps growing.
    let _ = rotate_if_oversized(&path, &old_log_file_path(config_dir), MAX_LOG_BYTES);

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("could not open log file {}: {e}", path.display()));

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));

    match file {
        Ok(file) => {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
            // `try_init` rather than `init`: examples and tests may have
            // already installed a logger, and a double-install shouldn't be
            // fatal for a background app.
            let _ = builder.try_init();
            Ok(path)
        }
        Err(e) => {
            let _ = builder.try_init();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_path_lives_in_the_config_dir() {
        let path = log_file_path(Path::new("C:\\cfg"));
        assert!(path.ends_with(LOG_FILE_NAME));
        assert_eq!(path.parent(), Some(Path::new("C:\\cfg")));
    }

    #[test]
    fn old_log_file_path_lives_beside_the_log() {
        let dir = Path::new("C:\\cfg");
        assert_eq!(old_log_file_path(dir).parent(), log_file_path(dir).parent());
        assert_ne!(old_log_file_path(dir), log_file_path(dir));
    }

    /// A private temp dir that cleans itself up, so these tests don't need a
    /// dev-dependency just to get a scratch directory.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "nodewarden-logging-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_small_log_is_left_alone() {
        let dir = TempDir::new("small");
        let path = log_file_path(&dir.0);
        let old = old_log_file_path(&dir.0);
        std::fs::write(&path, b"still tiny").unwrap();

        assert!(!rotate_if_oversized(&path, &old, 1024).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"still tiny");
        assert!(!old.exists());
    }

    #[test]
    fn an_oversized_log_is_rotated_and_starts_fresh() {
        let dir = TempDir::new("oversized");
        let path = log_file_path(&dir.0);
        let old = old_log_file_path(&dir.0);
        std::fs::write(&path, vec![b'x'; 2048]).unwrap();

        assert!(rotate_if_oversized(&path, &old, 1024).unwrap());
        assert!(!path.exists(), "the oversized log should have been moved");
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 2048);
    }

    #[test]
    fn rotating_again_replaces_the_previous_generation() {
        // Only one old generation is kept, which is what bounds total disk
        // use at roughly twice the cap.
        let dir = TempDir::new("twice");
        let path = log_file_path(&dir.0);
        let old = old_log_file_path(&dir.0);

        std::fs::write(&path, vec![b'a'; 2048]).unwrap();
        assert!(rotate_if_oversized(&path, &old, 1024).unwrap());
        std::fs::write(&path, vec![b'b'; 4096]).unwrap();
        assert!(rotate_if_oversized(&path, &old, 1024).unwrap());

        assert_eq!(std::fs::metadata(&old).unwrap().len(), 4096);
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_log_is_not_an_error_to_rotate() {
        let dir = TempDir::new("missing");
        let path = log_file_path(&dir.0);
        let old = old_log_file_path(&dir.0);
        assert!(!rotate_if_oversized(&path, &old, 1).unwrap());
    }

    #[test]
    fn init_rotates_an_oversized_log_before_appending() {
        let dir = TempDir::new("init");
        let path = log_file_path(&dir.0);
        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();

        // `init` may not install the logger (another test may have got there
        // first) but the rotation happens regardless -- that's the point of
        // doing it before `try_init`.
        let returned = init(&dir.0).unwrap();

        assert_eq!(returned, path);
        assert!(old_log_file_path(&dir.0).exists());
        assert!(
            std::fs::metadata(&path).unwrap().len() < MAX_LOG_BYTES,
            "init should have started a fresh log"
        );
    }
}

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

/// Returns the path the log file will be written to for a given config dir.
pub fn log_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(LOG_FILE_NAME)
}

/// Initialises the global logger, appending to `<config_dir>/nodewarden.log`.
///
/// The default level is `info`; it can be overridden at runtime with the
/// standard `RUST_LOG` environment variable. Returns the path actually being
/// logged to, or `Err` if the log file could not be opened (in which case
/// logging falls back to stderr so the process still starts).
pub fn init(config_dir: &Path) -> Result<PathBuf, String> {
    let path = log_file_path(config_dir);

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
}

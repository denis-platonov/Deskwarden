//! Resolves — once — the absolute path to the Bitwarden CLI (`bw.exe`), and
//! hands that single verified path to every call site that spawns it.
//!
//! Two separate jobs live here, and both matter:
//!
//! * **Resolution** ([`resolve_bw_exe`]): find `bw.exe` by absolute path,
//!   never as a bare `bw` left to `CreateProcess`'s search order.
//! * **Single-verification caching** ([`remember_verified_bw_exe`] /
//!   [`bw_command`]): the path is resolved and signature-checked exactly once,
//!   at startup, and every later spawn reuses *that* result rather than
//!   re-resolving. Re-resolving at each call site would mean the master
//!   password (`login_ui::run_bw_with_password`) could reach a binary that
//!   appeared on disk *after* startup verified a different one — a
//!   `<install dir>\bin\bw.exe` planted mid-session, say, which the resolver
//!   prefers over the `PATH` entry startup actually verified.

use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Tells `CreateProcess` not to allocate a console for the child. `bw.exe` is
/// a console-subsystem program; spawned plainly from this GUI-subsystem app
/// (deskwarden has no console of its own to inherit), Windows briefly flashes
/// a new one into existence for it on every single call otherwise.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The one `bw.exe` this process has verified, populated by `main`'s startup
/// check (see [`remember_verified_bw_exe`]).
///
/// A `OnceLock` rather than a plain static: "verified once, then never
/// re-resolved" is precisely the invariant, and `OnceLock::set` enforces it —
/// a second attempt to install a different path is refused rather than
/// silently winning.
static VERIFIED_BW_EXE: OnceLock<PathBuf> = OnceLock::new();

/// Records the `bw.exe` that startup resolved *and* verified as
/// Bitwarden-signed. Called once, from `main`, before anything spawns the CLI.
///
/// Deliberately idempotent-and-first-wins: a later call cannot replace an
/// already-verified path, so no code path can downgrade the process to a
/// binary that was never checked.
pub fn remember_verified_bw_exe(path: PathBuf) {
    if VERIFIED_BW_EXE.set(path).is_err() {
        log::warn!(
            "the verified bw.exe path was already set; ignoring the later attempt to replace it"
        );
    }
}

/// The verified `bw.exe`, or `None` if startup verification never ran (the
/// examples and unit tests, which don't go through `main`) or never passed.
pub fn verified_bw_exe() -> Option<&'static Path> {
    VERIFIED_BW_EXE.get().map(PathBuf::as_path)
}

/// Builds a `Command` for the single verified `bw.exe`.
///
/// Every spawn of the CLI in this crate goes through here, so there is exactly
/// one place where "which binary gets the master password?" is answered, and
/// that answer is the one startup checked the signature of.
pub fn bw_command() -> Result<Command, String> {
    match verified_bw_exe() {
        Some(path) => {
            let mut cmd = Command::new(path);
            cmd.creation_flags(CREATE_NO_WINDOW);
            Ok(cmd)
        }
        None => Err(
            "no verified Bitwarden CLI: the startup check that resolves bw.exe and confirms it \
             is Bitwarden-signed has not run (or did not pass), and deskwarden will not spawn an \
             unverified `bw`"
                .to_string(),
        ),
    }
}

/// Resolves `bw.exe`, preferring the location deskwarden's own installer
/// places it (`<install dir>\bin\bw.exe`, added to the user `PATH` by
/// `installer/bootstrap-bw.ps1`), falling back to a manual `PATH` search that
/// explicitly excludes deskwarden's own directory.
///
/// Deliberately never a bare `bw`: `CreateProcess`'s search order checks the
/// calling executable's own directory *before* `PATH`, and deskwarden
/// installs per-user into `%LOCALAPPDATA%\Deskwarden` -- a user-writable
/// directory, no privilege escalation needed to drop a file there. A `bw.exe`
/// planted directly beside `deskwarden.exe` would otherwise be preferred over
/// the real CLI, and would receive the user's master password
/// (`login_ui::run_bw_with_password`) and session token. Same class of bug as
/// `signature::powershell_path`, fixed the same way: name the binary
/// absolutely instead of trusting ambient search order. Unlike that fix,
/// pinning the path alone isn't the whole story here -- see
/// `main::TRUSTED_BW_SIGNER_ORGANIZATIONS`, which verifies whatever this
/// resolves to is actually signed by Bitwarden, since the installer's own
/// `bin` directory is itself inside the same user-writable install tree.
///
/// Returns `None` only when deskwarden's own directory cannot be determined
/// (`current_exe()` failing, which on Windows essentially cannot happen). That
/// is reported to the caller rather than papered over with a bare `"bw.exe"`:
/// a bare name would reopen the exact search-order hole this module exists to
/// close, so "I don't know where I am" has to be an explicit, handled failure
/// instead of a silent downgrade. When the directory *is* known but no
/// `bw.exe` exists anywhere, this still returns the expected install path:
/// a missing absolute path fails closed with a plain "not found" I/O error.
pub fn resolve_bw_exe() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let path_var = std::env::var_os("Path")
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    resolve_bw_exe_with(exe_dir.as_deref(), &path_var)
}

fn install_bin_candidate(exe_dir: &Path) -> PathBuf {
    exe_dir.join("bin").join("bw.exe")
}

/// A directory path reduced to a form two spellings of the same directory
/// agree on.
///
/// `canonicalize` is the real answer (it resolves `..`, 8.3 short names,
/// symlinks and junctions), but it only works on paths that exist, so a
/// `PATH` entry naming a deleted directory falls back to a textual
/// normalization: trailing separators stripped, so `C:\dir\` and `C:\dir`
/// compare equal.
fn normalize_for_compare(path: &Path) -> String {
    let normalized = std::fs::canonicalize(path).unwrap_or_else(|_| {
        PathBuf::from(
            path.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_string(),
        )
    });
    normalized.to_string_lossy().to_lowercase()
}

/// True if `a` and `b` name the same directory.
///
/// A raw string comparison is not enough: the exclusion below is a security
/// boundary, and `C:\...\deskwarden\` versus `C:\...\deskwarden` (or a `..`
/// hop, or an 8.3 short name) would otherwise slip straight past it.
fn same_directory(a: &Path, b: &Path) -> bool {
    normalize_for_compare(a) == normalize_for_compare(b)
}

fn resolve_bw_exe_with(exe_dir: Option<&Path>, path_var: &OsStr) -> Option<PathBuf> {
    let exe_dir = exe_dir?;

    let candidate = install_bin_candidate(exe_dir);
    if candidate.exists() {
        return Some(candidate);
    }

    for entry in std::env::split_paths(path_var) {
        if entry.as_os_str().is_empty() {
            continue;
        }

        // Relative `PATH` entries are skipped outright rather than compared.
        // A relative entry resolves against the *process's* current working
        // directory, which for a shortcut-launched app is routinely the
        // install directory itself -- so a bare `.` on `PATH` is a perfectly
        // ordinary way to end up loading `bw.exe` from beside
        // `deskwarden.exe`, the one thing this function exists to prevent.
        // Since a relative entry cannot be reliably compared against an
        // absolute app directory anyway, skipping is both the safe default
        // and the honest one.
        if entry.is_relative() {
            log::debug!(
                "skipping relative PATH entry {:?} while looking for bw.exe",
                entry
            );
            continue;
        }

        // The whole point of this function: never let deskwarden's own
        // directory win a `bw.exe` search, whether it's on `PATH` by
        // coincidence or (as `CreateProcess` would otherwise arrange) by
        // being checked ahead of everything else.
        if same_directory(&entry, exe_dir) {
            continue;
        }

        let candidate = entry.join("bw.exe");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    Some(install_bin_candidate(exe_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch directory, same `temp_dir()` + nanos pattern
    /// `updater`/`session_store`/`logging`'s tests already use (no `tempfile`
    /// dev-dependency in this crate).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-bw-path-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn prefers_the_installer_bin_directory_when_bw_exe_is_there() {
        let exe_dir = scratch_dir("prefers-bin");
        touch(&install_bin_candidate(&exe_dir));

        let resolved = resolve_bw_exe_with(Some(&exe_dir), OsStr::new(""));

        assert_eq!(resolved, Some(install_bin_candidate(&exe_dir)));
        std::fs::remove_dir_all(&exe_dir).ok();
    }

    #[test]
    fn falls_back_to_path_when_the_bin_directory_has_no_bw_exe() {
        let exe_dir = scratch_dir("falls-back-exe");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let legit_dir = scratch_dir("falls-back-legit");
        touch(&legit_dir.join("bw.exe"));

        let path_var = std::env::join_paths([&legit_dir]).unwrap();
        let resolved = resolve_bw_exe_with(Some(&exe_dir), &path_var);

        assert_eq!(resolved, Some(legit_dir.join("bw.exe")));
        std::fs::remove_dir_all(&exe_dir).ok();
        std::fs::remove_dir_all(&legit_dir).ok();
    }

    #[test]
    fn never_prefers_a_bw_exe_planted_directly_beside_the_app() {
        // The regression this whole module exists for: an attacker (same
        // user, no privilege escalation) drops `bw.exe` directly in
        // deskwarden's own directory -- not its `bin` subdirectory -- and
        // lists that directory on `PATH` ahead of the real CLI. Without the
        // exclusion, `CreateProcess` (and this manual search, if it didn't
        // skip `exe_dir`) would prefer it and hand it the master password.
        let exe_dir = scratch_dir("planted-exe");
        touch(&exe_dir.join("bw.exe")); // the attacker's plant
        let legit_dir = scratch_dir("planted-legit");
        touch(&legit_dir.join("bw.exe")); // the real CLI, elsewhere on PATH

        let path_var = std::env::join_paths([&exe_dir, &legit_dir]).unwrap();
        let resolved = resolve_bw_exe_with(Some(&exe_dir), &path_var);

        assert_eq!(resolved, Some(legit_dir.join("bw.exe")));
        std::fs::remove_dir_all(&exe_dir).ok();
        std::fs::remove_dir_all(&legit_dir).ok();
    }

    #[test]
    fn a_trailing_separator_does_not_smuggle_the_app_directory_past_the_exclusion() {
        // A raw string compare would see `...\planted-sep\` and
        // `...\planted-sep` as different directories and happily prefer the
        // planted binary.
        let exe_dir = scratch_dir("planted-sep");
        touch(&exe_dir.join("bw.exe"));
        let legit_dir = scratch_dir("planted-sep-legit");
        touch(&legit_dir.join("bw.exe"));

        let with_separator = PathBuf::from(format!("{}\\", exe_dir.display()));
        let path_var = std::env::join_paths([&with_separator, &legit_dir]).unwrap();
        let resolved = resolve_bw_exe_with(Some(&exe_dir), &path_var);

        assert_eq!(resolved, Some(legit_dir.join("bw.exe")));
        std::fs::remove_dir_all(&exe_dir).ok();
        std::fs::remove_dir_all(&legit_dir).ok();
    }

    #[test]
    fn a_relative_path_entry_pointing_at_the_app_directory_is_skipped() {
        // The realistic version of the bypass: a shortcut-launched app whose
        // "Start in" is its own install directory, plus a bare `.` on `PATH`.
        // `.` then *is* deskwarden's own directory, but a textual comparison
        // against the absolute `exe_dir` can never see that -- so relative
        // entries are skipped outright.
        //
        // The current directory is set for real here (rather than asserting
        // the skip abstractly) so the test fails if the skip is ever removed
        // in favour of comparison alone. It is restored immediately, and no
        // other test in this crate depends on the process's working
        // directory.
        let exe_dir = scratch_dir("planted-relative");
        touch(&exe_dir.join("bw.exe")); // the plant, reachable as `.\bw.exe`
        let legit_dir = scratch_dir("planted-relative-legit");
        touch(&legit_dir.join("bw.exe"));

        let previous_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&exe_dir).unwrap();
        let path_var = std::env::join_paths([Path::new("."), legit_dir.as_path()]).unwrap();
        let resolved = resolve_bw_exe_with(Some(&exe_dir), &path_var);
        std::env::set_current_dir(&previous_cwd).unwrap();

        assert_eq!(
            resolved,
            Some(legit_dir.join("bw.exe")),
            "a `.` PATH entry resolving to deskwarden's own directory was not skipped"
        );
        std::fs::remove_dir_all(&exe_dir).ok();
        std::fs::remove_dir_all(&legit_dir).ok();
    }

    #[test]
    fn fails_closed_to_the_expected_bin_path_when_bw_exe_is_nowhere() {
        let exe_dir = scratch_dir("fails-closed");
        std::fs::create_dir_all(&exe_dir).unwrap();

        let resolved = resolve_bw_exe_with(Some(&exe_dir), OsStr::new(""));

        // Not found anywhere, but still the specific expected path -- never a
        // bare name that would reopen the search-order hole.
        assert_eq!(resolved, Some(install_bin_candidate(&exe_dir)));
        assert!(!resolved.unwrap().exists());
        std::fs::remove_dir_all(&exe_dir).ok();
    }

    #[test]
    fn reports_failure_rather_than_a_bare_name_when_the_exe_directory_is_unknown() {
        // The old behaviour here was `PathBuf::from("bw.exe")`, which handed
        // `CreateProcess` a bare name and reopened the very search-order hole
        // this module closes. "I don't know where I am" is now the caller's
        // problem to handle explicitly.
        assert_eq!(resolve_bw_exe_with(None, OsStr::new("")), None);
    }

    #[test]
    fn the_verified_path_is_recorded_once_and_read_back_by_every_call_site() {
        // The verify-once/resolve-many hole: `main` verifies one path, and
        // every later spawn must reuse *that* result rather than re-resolving
        // against a filesystem that may have changed since.
        let first = PathBuf::from(r"C:\deskwarden-test\first\bw.exe");
        remember_verified_bw_exe(first.clone());
        remember_verified_bw_exe(PathBuf::from(r"C:\deskwarden-test\second\bw.exe"));

        assert_eq!(verified_bw_exe(), Some(first.as_path()));
        assert!(bw_command().is_ok());
    }
}

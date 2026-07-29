//! Resolves the absolute path to the Bitwarden CLI (`bw.exe`).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Resolves `bw.exe`, preferring the location deskwarden's own installer
/// places it (`<install dir>\bin\bw.exe`, added to the user `PATH` by
/// `installer/bootstrap-bw.ps1`), falling back to a manual `PATH` search that
/// explicitly excludes deskwarden's own directory.
///
/// Deliberately never a bare `bw`: `CreateProcess`'s search order checks the
/// calling executable's own directory *before* `PATH`, and deskwarden
/// installs per-user into `%LOCALAPPDATA%\deskwarden` -- a user-writable
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
/// If nothing is found anywhere, this still returns the expected install
/// path (even though it doesn't exist) rather than falling through to a bare
/// name: a missing absolute path fails closed with a plain "not found" I/O
/// error, instead of silently reintroducing the search-order hole this
/// exists to close.
pub fn resolve_bw_exe() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let path_var = std::env::var_os("Path").or_else(|| std::env::var_os("PATH")).unwrap_or_default();
    resolve_bw_exe_with(exe_dir.as_deref(), &path_var)
}

fn install_bin_candidate(exe_dir: &Path) -> PathBuf {
    exe_dir.join("bin").join("bw.exe")
}

fn paths_equal_ignore_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

fn resolve_bw_exe_with(exe_dir: Option<&Path>, path_var: &OsStr) -> PathBuf {
    if let Some(dir) = exe_dir {
        let candidate = install_bin_candidate(dir);
        if candidate.exists() {
            return candidate;
        }
    }

    for entry in std::env::split_paths(path_var) {
        // The whole point of this function: never let deskwarden's own
        // directory win a `bw.exe` search, whether it's on `PATH` by
        // coincidence or (as `CreateProcess` would otherwise arrange) by
        // being checked ahead of everything else.
        if exe_dir.is_some_and(|dir| paths_equal_ignore_case(&entry, dir)) {
            continue;
        }
        let candidate = entry.join("bw.exe");
        if candidate.exists() {
            return candidate;
        }
    }

    exe_dir.map(install_bin_candidate).unwrap_or_else(|| PathBuf::from("bw.exe"))
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

        assert_eq!(resolved, install_bin_candidate(&exe_dir));
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

        assert_eq!(resolved, legit_dir.join("bw.exe"));
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

        assert_eq!(resolved, legit_dir.join("bw.exe"));
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
        assert_eq!(resolved, install_bin_candidate(&exe_dir));
        assert!(!resolved.exists());
        std::fs::remove_dir_all(&exe_dir).ok();
    }

    #[test]
    fn falls_back_to_a_bare_name_only_when_the_exe_directory_is_unknown() {
        let resolved = resolve_bw_exe_with(None, OsStr::new(""));
        assert_eq!(resolved, PathBuf::from("bw.exe"));
    }
}

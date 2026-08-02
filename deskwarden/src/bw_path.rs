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
/// Public because anything that re-sets a spawned command's creation flags
/// has to OR this back in: `Command::creation_flags` *replaces* the value
/// it holds, so a later call silently drops this one (see
/// `job_object::spawn_in_job`).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

/// The `bitwarden-cli` directory the CLI looks for *beside its own
/// executable* — its `relativeDataDir`.
///
/// The CLI resolves its profile directory in this order:
///
/// ```ts
/// if (fs.existsSync(relativeDataDir)) { p = relativeDataDir; }    // FIRST
/// else if (process.env.BITWARDENCLI_APPDATA_DIR) { ... }
/// ```
///
/// so while this directory exists, `BITWARDENCLI_APPDATA_DIR` is **ignored
/// entirely** — see [`multi_account_from`] for why that is fatal to multiple
/// accounts rather than merely inconvenient.
///
/// Joined onto the directory of `bw.exe` itself, never deskwarden's own
/// directory and never the working directory: the CLI computes it from
/// `__dirname`, and deskwarden's installer puts `bw.exe` one level down in
/// `bin\`, so the two are genuinely different directories here.
pub fn relative_data_dir(bw_exe: &Path) -> Option<PathBuf> {
    bw_exe.parent().map(|dir| dir.join("bitwarden-cli"))
}

/// Whether deskwarden may offer more than one account at all.
///
/// Not a `bool`, because the two ways of saying "no" need different
/// explanations and the user can only act on one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAccountAvailability {
    Available,
    /// A `bitwarden-cli` directory exists beside `bw.exe`, so the CLI ignores
    /// `BITWARDENCLI_APPDATA_DIR` and every account would share one profile.
    BlockedByPortableProfile { relative_data_dir: PathBuf },
    /// Startup verification never produced a path, so whether the trap above
    /// is present cannot be checked at all.
    BlockedByUnknownCliPath,
}

/// The decision, as a pure function of the two facts it needs.
///
/// Split out from [`multi_account_availability`] so it can be asserted in both
/// directions without a filesystem: "blocked" and "available" are each one
/// mutation apart from the other, and only a test that pins both can tell a
/// working check from a check that always says no.
///
/// `relative == None` blocks. "We do not know where the CLI is" must never be
/// read as "there is no portable profile beside it" — the whole hazard is a
/// directory we would then never look for, and the failure it causes is silent
/// state-mixing rather than an error.
pub fn multi_account_from(
    relative: Option<PathBuf>,
    relative_exists: bool,
) -> MultiAccountAvailability {
    match relative {
        None => MultiAccountAvailability::BlockedByUnknownCliPath,
        Some(dir) if relative_exists => MultiAccountAvailability::BlockedByPortableProfile {
            relative_data_dir: dir,
        },
        Some(_) => MultiAccountAvailability::Available,
    }
}

/// The impure half: probes the real filesystem for the `bw.exe` it is given.
///
/// Takes the executable as an argument rather than reading
/// [`verified_bw_exe`] itself so the `.exists()` call — the part a pure
/// function cannot cover — is reachable from a test that plants a real
/// directory. A test that rebuilt this expression itself would leave the
/// production one unexercised.
pub fn multi_account_availability_from_exe(bw_exe: Option<&Path>) -> MultiAccountAvailability {
    let relative = bw_exe.and_then(relative_data_dir);
    let exists = relative.as_ref().is_some_and(|dir| dir.exists());
    multi_account_from(relative, exists)
}

/// Whether this process may offer multiple accounts, against the one verified
/// `bw.exe`.
pub fn multi_account_availability() -> MultiAccountAvailability {
    multi_account_availability_from_exe(verified_bw_exe())
}

impl MultiAccountAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Why multiple accounts are unavailable, or `None` when they are not.
    ///
    /// The portable-profile message names the directory: it is the one thing
    /// the user can actually do something about, and without it the message is
    /// unactionable.
    pub fn explanation(&self) -> Option<String> {
        match self {
            Self::Available => None,
            Self::BlockedByPortableProfile { relative_data_dir } => Some(format!(
                "The Bitwarden CLI is using the profile directory beside itself:\n{}\n\n\
                 While that directory exists the CLI ignores the per-account directory \
                 Deskwarden would point it at, so every account would share one profile. \
                 Deskwarden is staying a single-account app rather than mixing two accounts' \
                 state together.\n\nRemove or rename that directory to enable multiple \
                 accounts.",
                relative_data_dir.display()
            )),
            Self::BlockedByUnknownCliPath => Some(
                "Deskwarden could not work out where the Bitwarden CLI is, so it cannot check \
                 whether the CLI is using a profile directory beside itself. Multiple accounts \
                 are unavailable."
                    .to_string(),
            ),
        }
    }
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

    #[test]
    fn the_relative_data_dir_is_bitwarden_cli_beside_the_exe() {
        assert_eq!(
            relative_data_dir(Path::new(
                r"C:\Users\me\AppData\Local\Deskwarden\bin\bw.exe"
            )),
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Local\Deskwarden\bin\bitwarden-cli"
            )),
        );
        // Not the app directory one level up, and not the CWD: the CLI joins
        // it onto the directory of its OWN executable, and deskwarden's
        // installer puts `bw.exe` in `bin\` -- so looking one level up would
        // check a directory the CLI never consults and miss the real trap.
        assert_ne!(
            relative_data_dir(Path::new(r"C:\a\bin\bw.exe")),
            Some(PathBuf::from(r"C:\a\bitwarden-cli")),
        );
    }

    #[test]
    fn a_bitwarden_cli_directory_beside_the_exe_blocks_multi_account() {
        let dir = PathBuf::from(r"C:\a\bin\bitwarden-cli");
        assert_eq!(
            multi_account_from(Some(dir.clone()), true),
            MultiAccountAvailability::BlockedByPortableProfile {
                relative_data_dir: dir
            },
            "BITWARDENCLI_APPDATA_DIR is IGNORED when this directory exists, so every \
             account would silently share one profile"
        );
    }

    #[test]
    fn no_such_directory_means_multi_account_is_available() {
        // The positive control. Without it, `multi_account_from` returning
        // Blocked unconditionally passes the test above.
        assert_eq!(
            multi_account_from(Some(PathBuf::from(r"C:\a\bin\bitwarden-cli")), false),
            MultiAccountAvailability::Available,
        );
    }

    #[test]
    fn an_unknown_cli_path_blocks_rather_than_assuming_it_is_fine() {
        // `verified_bw_exe()` is `None` in examples and unit tests, and would
        // be `None` if startup verification had not run. "We do not know where
        // the CLI is" cannot be read as "there is no portable profile beside
        // it".
        assert_eq!(
            multi_account_from(None, false),
            MultiAccountAvailability::BlockedByUnknownCliPath,
        );
        assert_eq!(
            multi_account_from(None, true),
            MultiAccountAvailability::BlockedByUnknownCliPath,
        );
    }

    #[test]
    fn the_live_probe_sees_a_directory_planted_beside_a_real_exe() {
        // Drives the production impure half -- not a reconstruction of it --
        // against a directory that really exists, so the `.exists()` call
        // itself is exercised rather than only the decision it feeds. Both
        // orders are asserted around one `create_dir_all`, so a probe wired to
        // a constant fails whichever constant it is.
        let dir = scratch_dir("relative-data-dir");
        let exe = dir.join("bw.exe");
        touch(&exe);
        let portable = dir.join("bitwarden-cli");

        assert_eq!(
            multi_account_availability_from_exe(Some(&exe)),
            MultiAccountAvailability::Available,
            "nothing has been planted beside {} yet",
            exe.display()
        );

        std::fs::create_dir_all(&portable).unwrap();
        assert_eq!(
            multi_account_availability_from_exe(Some(&exe)),
            MultiAccountAvailability::BlockedByPortableProfile {
                relative_data_dir: portable
            },
        );

        assert_eq!(
            multi_account_availability_from_exe(None),
            MultiAccountAvailability::BlockedByUnknownCliPath,
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multi_account_availability_asks_the_verified_exe_and_nothing_else() {
        // WIRING, not a decision. `multi_account_availability()` is what every
        // later consumer calls; if it stopped consulting `verified_bw_exe()`
        // -- or answered from a constant -- every test above would stay green
        // while the trap went undetected in the running app.
        //
        // First-wins, so this only guarantees *some* path is recorded, which
        // is all this test needs: the assertion is agreement with the
        // production probe applied to whatever that path is.
        remember_verified_bw_exe(PathBuf::from(r"C:\deskwarden-test\first\bw.exe"));
        let exe = verified_bw_exe().expect("a verified path was just recorded");

        assert_eq!(
            multi_account_availability(),
            multi_account_availability_from_exe(Some(exe)),
            "multi_account_availability() did not answer for the verified bw.exe ({})",
            exe.display()
        );
        assert_ne!(
            multi_account_availability(),
            MultiAccountAvailability::BlockedByUnknownCliPath,
            "the CLI path IS known here, so the unknown-path state is wrong"
        );
    }

    #[test]
    fn the_availability_probe_is_wired_to_the_pure_decision_it_documents() {
        // The other half of the wiring, and unavoidably a source guard: the
        // expressions below cannot be observed from outside the two functions.
        // Needles are `concat!`-split so none can match its own declaration
        // here, and single-line so a CRLF checkout does not turn them into
        // false passes. Each is a *required* needle, so the assertion is
        // itself the proof that it matches live code.
        let source = include_str!("bw_path.rs");
        for required in [
            concat!("multi_account_availability_from_exe(", "verified_bw_exe())"),
            concat!("bw_exe.and_then(", "relative_data_dir)"),
            concat!("dir.", "exists()"),
            concat!("multi_account_from(", "relative, exists)"),
        ] {
            assert!(
                source.contains(required),
                "`{required}` is gone: the availability probe no longer reaches the \
                 filesystem check and the pure decision through the documented path"
            );
        }
    }

    #[test]
    fn only_the_blocked_variants_explain_themselves_and_they_name_the_directory() {
        assert_eq!(MultiAccountAvailability::Available.explanation(), None);
        assert!(MultiAccountAvailability::Available.is_available());

        let dir = PathBuf::from(r"C:\a\bin\bitwarden-cli");
        let blocked = MultiAccountAvailability::BlockedByPortableProfile {
            relative_data_dir: dir.clone(),
        };
        assert!(!blocked.is_available());
        let text = blocked.explanation().expect("a blocked state must say why");
        assert!(
            text.contains(r"C:\a\bin\bitwarden-cli"),
            "the one directory the user has to go and delete is not in the message: {text}"
        );

        assert!(!MultiAccountAvailability::BlockedByUnknownCliPath.is_available());
        assert!(MultiAccountAvailability::BlockedByUnknownCliPath
            .explanation()
            .is_some());
    }
}

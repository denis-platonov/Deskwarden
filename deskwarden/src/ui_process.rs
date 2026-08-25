//! **How the daemon starts a window in a process of its own, and how that
//! window's answer gets home.**
//!
//! The vault window costs ~50 MB that closing it does not return: the OpenGL
//! driver's committed arenas survive window destruction and are reclaimed
//! only at process exit. So the window runs in a process that exits --
//! `deskwarden.exe --ui vault` -- and the daemon stays at ~10 MB with no GL
//! driver mapped at all. See
//! `docs/superpowers/specs/2026-08-23-daemon-and-ui-processes-design.md`.
//!
//! Two things live here, and they are the two halves of that boundary.
//!
//! # Out, on the command line: a mode and a surface, and nothing else
//!
//! A Windows command line is readable by any process on the machine, so
//! **nothing secret may appear in one**. [`UiSpawnPlan`] is a value rather
//! than a `Command` precisely so a test can read every argument the daemon is
//! about to pass and assert that the list is exactly two words long. The UI
//! process reads what it needs itself: the DPAPI-wrapped session token
//! unwraps under the same user's credentials, `settings.json` names the
//! active account, and `bw_serve::BW_SERVE_PORT` is a compile-time constant
//! in the same binary.
//!
//! # Back, from the child: a small file, plus the exit code
//!
//! [`UiVaultResult`] is the vault window's six daemon-actionable outcomes.
//! The daemon has to act on every one of them -- a lock tears down
//! `bw serve` and clears the match engine, a switch re-points the CLI's data
//! directory and re-authenticates -- and a UI process can do none of that.
//!
//! **Why a file and not an exit code alone.** Two of the six carry a payload:
//! `switch_to` is an [`AccountId`] and `edited_settings` is a whole
//! [`Settings`]. Neither fits in a 32-bit status. So the full result is
//! serialised to one small JSON file in the config directory the daemon
//! already owns, written once by the child as its last act and read once by
//! the daemon after the child is gone -- which is why it needs no locking, no
//! protocol version and no DACL reasoning. It carries no secret: no token, no
//! password, no vault item. An account id and a settings block are already
//! sitting in `settings.json` in the same directory.
//!
//! **Why the exit code as well.** `locked` is the one outcome whose loss is a
//! security defect rather than an inconvenience -- a Lock button that does
//! not lock. A disk that is full, a file that cannot be created, a UI process
//! killed between locking and writing: each of those loses the file and none
//! of them loses the exit status. So the four boolean outcomes ride the exit
//! code too and the daemon takes the union. The two payload fields have no
//! such backup and their loss is recoverable by the user repeating the
//! action.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::accounts::AccountId;
use crate::settings::Settings;

/// The daemon's `deskwarden.exe --ui <surface>`, as a value.
///
/// **Its most important field is the one that names an absence.**
/// `joins_the_daemons_job` is `false` and there is no code path that sets it
/// otherwise -- but it is a readable field rather than a missing call,
/// because "this child is not assigned to the kill-on-close job object" is a
/// property a reader has to be able to *see*. Every other child this app
/// spawns goes through [`crate::job_object::spawn_in_job`] and joins the job;
/// a maintainer routing this spawn through the same helper by reflex would be
/// making a daemon restart -- an update, a crash, a manual quit -- close the
/// user's open vault window, and nothing in the diff would have said so.
///
/// This is the shape [`crate::vault_window`]'s
/// `the_launcher_spawns_the_plan_verbatim_and_out_of_any_job` established: the
/// plan is built by a pure function, asserted over by a test that starts no
/// process, and handed to the spawn verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSpawnPlan {
    /// `std::env::current_exe()`. One binary, two modes -- so there is no
    /// second file to keep version-matched and nothing a half-applied update
    /// can leave mismatched.
    pub program: PathBuf,
    /// Exactly the mode flag and the surface name. Asserted, not assumed --
    /// see `no_argument_of_a_ui_spawn_could_be_a_secret`.
    pub args: Vec<String>,
    /// **Always `false`.** See this type's doc; it is spelled out so that it
    /// can be read and tested rather than inferred from a call that is not
    /// there.
    pub joins_the_daemons_job: bool,
}

/// The mode flag, spelled once. `main.rs`'s `UI_FLAG` is the reader; this is
/// the writer, and `the_flag_the_daemon_writes_is_the_flag_the_app_reads`
/// reconciles them.
pub const UI_FLAG: &str = "--ui";

impl UiSpawnPlan {
    /// The plan for one surface.
    ///
    /// `surface_arg` is the word `main.rs`'s `Surface::as_arg` produces, and
    /// it is the *only* thing that varies between surfaces. Taking it as a
    /// `&str` rather than importing the enum keeps this module free of the
    /// binary's own types, which is what lets it be tested without one.
    pub fn for_surface(program: PathBuf, surface_arg: &str) -> Self {
        Self {
            program,
            args: vec![UI_FLAG.to_string(), surface_arg.to_string()],
            // Not a default, not an omission: the one line this whole module
            // exists to make visible.
            joins_the_daemons_job: false,
        }
    }
}

/// `ERROR_ACCESS_DENIED`, returned by `CreateProcess` when
/// `CREATE_BREAKAWAY_FROM_JOB` is asked for and the containing job forbids it.
const ACCESS_DENIED_ERRNO: i32 = 5;

/// Try to start the child *outside* any job this process is itself in, and
/// fall back to inside if the job forbids breaking out.
///
/// **Generic over the spawn so a test can be the spawn.** The three decisions
/// here -- that the first attempt asks for breakaway, that `ACCESS_DENIED`
/// and only `ACCESS_DENIED` earns a retry, and that the retry asks without --
/// are the entire value of the flag and none of them is visible in a
/// source-text needle. This is deliberately the same policy
/// `vault_window::launch_with` uses for launching a user's app, for the same
/// reason: a child that inherits Deskwarden's containing job dies when
/// Deskwarden does.
///
/// The daemon's own `KillOnCloseJob` is a different matter and is not
/// involved at all: this child is never assigned to it. Breakaway is about a
/// job somebody *else* put Deskwarden in.
pub fn spawn_out_of_any_job<T>(
    mut spawn: impl FnMut(bool) -> io::Result<T>,
) -> io::Result<T> {
    match spawn(true) {
        Ok(child) => Ok(child),
        Err(e) if e.raw_os_error() == Some(ACCESS_DENIED_ERRNO) => {
            log::warn!(
                "breakaway from the containing job was refused; starting the UI process \
                 inside it. It will not survive this process being killed by whoever owns \
                 that job"
            );
            spawn(false)
        }
        Err(e) => Err(e),
    }
}

/// **The vault window's six daemon-actionable outcomes**, as they cross the
/// process boundary.
///
/// A near-copy of [`crate::vault_window::VaultWindowResult`] minus
/// `account_details`, which is deliberately not carried: it is a warm-cache
/// optimisation whose absence costs one `bw status` spawn on the next open --
/// already the documented cost of a window closed before its fetch returned
/// -- and carrying it would put the user's email address in a file for no
/// benefit the daemon cannot re-earn.
///
/// **Nothing here is a secret.** No session token, no master password, no
/// vault item. `switch_to` is an account id and `edited_settings` is the
/// preferences block; both already live in `settings.json` beside this file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiVaultResult {
    /// The user pressed Lock, or the auto-lock timer fired. **The one field
    /// whose loss would be a security defect** -- see this module's doc for
    /// why it also rides the exit code.
    pub locked: bool,
    /// A write in the window failed with `Unauthorized`; the session behind
    /// `bw serve` is gone. The daemon runs the same recovery a lock does.
    pub needs_reauth: bool,
    /// What the titlebar gear's modal was left holding. The daemon persists
    /// it and reconciles the tray and the backend policy from it.
    pub edited_settings: Option<Settings>,
    /// The account the switcher asked to move to.
    pub switch_to: Option<AccountId>,
    /// The account menu asked to add one.
    pub add_account: bool,
    /// The account menu asked to remove the active one.
    pub remove_account: bool,
}

impl UiVaultResult {
    /// `locked`.
    pub const EXIT_LOCKED: i32 = 1;
    /// `needs_reauth`.
    pub const EXIT_NEEDS_REAUTH: i32 = 2;
    /// `add_account`.
    pub const EXIT_ADD_ACCOUNT: i32 = 4;
    /// `remove_account`.
    pub const EXIT_REMOVE_ACCOUNT: i32 = 8;

    /// The four booleans, packed into the process's exit status.
    ///
    /// A bitfield rather than one code per outcome because they are not
    /// mutually exclusive: a window can be locked *and* have asked for a
    /// re-auth, and a scheme that had to pick one would drop the other.
    ///
    /// Zero means "the window was closed and nothing needs doing", which is
    /// also what a successful ordinary exit means -- correctly, because those
    /// are the same event.
    pub fn exit_code(&self) -> i32 {
        (if self.locked { Self::EXIT_LOCKED } else { 0 })
            | (if self.needs_reauth { Self::EXIT_NEEDS_REAUTH } else { 0 })
            | (if self.add_account { Self::EXIT_ADD_ACCOUNT } else { 0 })
            | (if self.remove_account { Self::EXIT_REMOVE_ACCOUNT } else { 0 })
    }

    /// The four booleans, read back out of an exit status.
    ///
    /// **Only the booleans.** `switch_to` and `edited_settings` cannot be
    /// reconstructed from a status word and are left empty here; the file is
    /// the only carrier for those, and [`UiVaultResult::union`] is where the
    /// two sources are put back together.
    pub fn from_exit_code(code: i32) -> Self {
        Self {
            locked: code & Self::EXIT_LOCKED != 0,
            needs_reauth: code & Self::EXIT_NEEDS_REAUTH != 0,
            add_account: code & Self::EXIT_ADD_ACCOUNT != 0,
            remove_account: code & Self::EXIT_REMOVE_ACCOUNT != 0,
            edited_settings: None,
            switch_to: None,
        }
    }

    /// The result the daemon acts on: the file if there was one, with the
    /// exit code's booleans OR'd in.
    ///
    /// **`or`, never `and`.** A missing file must not be able to turn a lock
    /// off. The whole point of carrying the booleans twice is that either
    /// carrier alone is enough to lock, and neither can veto the other.
    ///
    /// A UI process that crashed hard -- a panic, a `TerminateProcess` -- has
    /// an exit code that is not one of ours. That is why the caller passes
    /// only codes it recognises; see `main.rs`'s `ui_vault_session`.
    pub fn union(from_file: Option<Self>, from_exit_code: Self) -> Self {
        let file = from_file.unwrap_or_default();
        Self {
            locked: file.locked || from_exit_code.locked,
            needs_reauth: file.needs_reauth || from_exit_code.needs_reauth,
            add_account: file.add_account || from_exit_code.add_account,
            remove_account: file.remove_account || from_exit_code.remove_account,
            // No exit-code carrier exists for either; the file is all there is.
            edited_settings: file.edited_settings,
            switch_to: file.switch_to,
        }
    }
}

/// Where a UI process leaves its result.
///
/// **Named by the child's own process id**, so a stale file from a UI process
/// that was killed cannot be read as this one's answer, and two UI processes
/// (once there is more than one surface) cannot write over each other. The
/// daemon knows the id from the `Child` it spawned; the child knows its own.
pub fn result_path(config_dir: &Path, pid: u32) -> PathBuf {
    config_dir.join(format!("ui-result-{pid}.json"))
}

/// Write the result, from the UI process, as its last act before exiting.
///
/// Failure is reported to the caller rather than swallowed, because the
/// caller's response to it is not "ignore": it is to make sure the exit code
/// still carries what it can.
pub fn write_result(path: &Path, result: &UiVaultResult) -> io::Result<()> {
    let json = serde_json::to_string_pretty(result).map_err(io::Error::other)?;
    std::fs::write(path, json)
}

/// Read the result, from the daemon, after the child has exited.
///
/// `None` for every failure -- absent, unreadable, unparseable -- and each is
/// the same thing from the daemon's side: this window left no payload, so act
/// on the exit code alone. Logged rather than silent, because a file that is
/// consistently unparseable is a bug that would otherwise present as
/// "settings edited in the vault window sometimes do not stick".
pub fn read_result(path: &Path) -> Option<UiVaultResult> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<UiVaultResult>(&text) {
            Ok(result) => Some(result),
            Err(e) => {
                log::warn!("the UI process's result file at {} did not parse ({e})", path.display());
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("could not read the UI process's result file at {}: {e}", path.display());
            None
        }
    }
}

/// Delete the result file once it has been read.
///
/// Best effort. A leftover is inert -- the next UI process has a different
/// process id and writes a different name -- but the config directory is the
/// user's and should not accumulate.
pub fn forget_result(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {} after reading it: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_argument_of_a_ui_spawn_could_be_a_secret() {
        let plan = UiSpawnPlan::for_surface(PathBuf::from(r"C:\Program Files\x\deskwarden.exe"), "vault");
        assert_eq!(
            plan.args,
            vec!["--ui".to_string(), "vault".to_string()],
            "a UI spawn's command line is a mode and a surface and nothing else. Any third \
             argument is a value somebody decided to pass, and a Windows command line is \
             readable by every process on the machine"
        );
    }

    #[test]
    fn the_ui_child_is_not_a_member_of_the_daemons_kill_on_close_job() {
        let plan = UiSpawnPlan::for_surface(PathBuf::from("deskwarden.exe"), "vault");
        assert!(
            !plan.joins_the_daemons_job,
            "the UI process must outlive a daemon restart. Assigned to the kill-on-close job, \
             an update or a crash would close the user's open vault window"
        );
    }

    #[test]
    fn the_plan_names_the_program_it_was_given_rather_than_deriving_one() {
        let exe = PathBuf::from(r"D:\somewhere else\deskwarden.exe");
        assert_eq!(UiSpawnPlan::for_surface(exe.clone(), "vault").program, exe);
    }

    #[test]
    fn the_spawn_asks_for_breakaway_first_and_retries_without_it_only_on_access_denied() {
        let mut asked = Vec::new();
        let out = spawn_out_of_any_job(|breakaway| {
            asked.push(breakaway);
            if breakaway {
                Err(io::Error::from_raw_os_error(ACCESS_DENIED_ERRNO))
            } else {
                Ok(())
            }
        });
        assert!(out.is_ok());
        assert_eq!(
            asked,
            vec![true, false],
            "breakaway must be attempted first and abandoned only when the job refuses it"
        );
    }

    #[test]
    fn any_other_spawn_failure_is_reported_rather_than_retried() {
        let mut attempts = 0;
        let out: io::Result<()> = spawn_out_of_any_job(|_| {
            attempts += 1;
            Err(io::Error::from_raw_os_error(2))
        });
        assert!(out.is_err());
        assert_eq!(
            attempts, 1,
            "a missing executable is not a job problem; retrying without breakaway would \
             fail identically and hide the real error behind the wrong one"
        );
    }

    #[test]
    fn a_locked_window_is_still_locked_when_the_result_file_is_lost() {
        let locked = UiVaultResult { locked: true, ..Default::default() };
        let recovered = UiVaultResult::union(None, UiVaultResult::from_exit_code(locked.exit_code()));
        assert!(
            recovered.locked,
            "a Lock that does not lock is worse than no split at all; the exit code is the \
             carrier that survives a failed file write"
        );
    }

    #[test]
    fn the_file_cannot_be_used_to_turn_a_lock_off() {
        let file = UiVaultResult { locked: false, ..Default::default() };
        let union =
            UiVaultResult::union(Some(file), UiVaultResult::from_exit_code(UiVaultResult::EXIT_LOCKED));
        assert!(union.locked, "the two carriers are OR'd; neither may veto the other");
    }

    #[test]
    fn every_boolean_outcome_survives_the_exit_code_on_its_own() {
        for (label, result) in [
            ("locked", UiVaultResult { locked: true, ..Default::default() }),
            ("needs_reauth", UiVaultResult { needs_reauth: true, ..Default::default() }),
            ("add_account", UiVaultResult { add_account: true, ..Default::default() }),
            ("remove_account", UiVaultResult { remove_account: true, ..Default::default() }),
        ] {
            let round_tripped = UiVaultResult::from_exit_code(result.exit_code());
            assert_eq!(round_tripped, result, "{label} did not survive the exit code");
        }
    }

    #[test]
    fn the_codes_are_distinct_bits_so_two_outcomes_do_not_collide() {
        let both = UiVaultResult { locked: true, remove_account: true, ..Default::default() };
        let read_back = UiVaultResult::from_exit_code(both.exit_code());
        assert!(read_back.locked && read_back.remove_account);
        assert!(!read_back.needs_reauth && !read_back.add_account);
    }

    #[test]
    fn a_window_that_asked_for_nothing_exits_zero() {
        assert_eq!(UiVaultResult::default().exit_code(), 0);
    }

    #[test]
    fn the_payload_fields_survive_the_file_because_no_exit_code_can_carry_them() {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-ui-result-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = result_path(&dir, 4242);

        let written = UiVaultResult {
            switch_to: Some(AccountId::generate()),
            edited_settings: Some(Settings::default()),
            ..Default::default()
        };
        write_result(&path, &written).expect("write");

        let read = read_result(&path).expect("the file should parse");
        assert_eq!(read.switch_to, written.switch_to);
        assert_eq!(read.edited_settings, written.edited_settings);

        forget_result(&path);
        assert!(read_result(&path).is_none(), "the file is deleted once it has been read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_result_file_that_is_not_there_is_not_an_error() {
        let path = std::env::temp_dir().join("deskwarden-no-such-ui-result-file.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_result(&path), None);
    }

    #[test]
    fn the_result_file_is_named_by_the_process_that_wrote_it() {
        let a = result_path(Path::new("C:/cfg"), 100);
        let b = result_path(Path::new("C:/cfg"), 101);
        assert_ne!(
            a, b,
            "two UI processes must not write over each other, and a stale file from a killed \
             one must not be read as a live one's answer"
        );
    }

    /// The serialised form is inspected as text, not as a struct, because
    /// what matters is what a reader of the file on disk can see.
    #[test]
    fn nothing_secret_is_written_to_the_result_file() {
        let result = UiVaultResult {
            locked: true,
            needs_reauth: true,
            edited_settings: Some(Settings::default()),
            switch_to: Some(AccountId::generate()),
            add_account: true,
            remove_account: true,
        };
        let json = serde_json::to_string(&result).expect("serialise");
        let lowered = json.to_lowercase();
        for forbidden in ["session", "token", "password", "master", "bw_session"] {
            assert!(
                !lowered.contains(forbidden),
                "the result file mentions {forbidden:?}: {json}. This file is the whole of \
                 what crosses the process boundary and it must stay free of secrets"
            );
        }
    }
}

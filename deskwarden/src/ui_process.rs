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

/// **What the daemon does with one `try_wait` answer**, decided where a test
/// can watch it.
///
/// The daemon's loop asks the open UI child once per iteration and must never
/// block on it: a loop that blocks is a `CTRL+ALT+B` that sits on the hotkey
/// channel until the window closes and then fills the wrong window, which is
/// the defect this whole step exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reap {
    /// Still running. The slot stays occupied and the one-window rule keeps
    /// holding.
    Keep,
    /// The child is gone, and this is what it exited with -- `None` when
    /// Windows gave no code at all.
    Take { code: Option<i32> },
}

/// `try_wait`'s three answers, mapped onto the two things the daemon can do.
///
/// **An `Err` reaps.** `try_wait` fails when the handle is no longer a child
/// this process can wait on -- it was already reaped, or the handle is
/// invalid. Keeping the slot on that answer would leave the daemon believing
/// a window is open forever, and under the one-window rule that is an *Open
/// Vault* that can never open another one again for the life of the process.
/// So the failure is treated as "gone", with no exit code, which is exactly
/// what a killed child means anyway.
///
/// Taking `Result<Option<Option<i32>>, ()>`-shaped data rather than a
/// `std::process::Child` is what makes this reachable at all: an
/// `ExitStatus` cannot be constructed in a test on Windows, and a `Child`
/// cannot be had without starting a process.
pub fn reap_step(answer: Result<Option<Option<i32>>, ()>) -> Reap {
    match answer {
        Ok(None) => Reap::Keep,
        Ok(Some(code)) => Reap::Take { code },
        Err(()) => Reap::Take { code: None },
    }
}

/// **One window per surface**, decided where a test can watch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiOpenDecision {
    /// Nothing is open for this surface; start one.
    Spawn,
    /// One is open but HIDDEN, because `keep_ui_loaded` kept its process
    /// resident after a plain close. Show that one; it cannot be raised,
    /// because a hidden viewport has no window to raise.
    ShowTheHiddenOne { pid: u32 },
    /// One is already open. Bring *that* one forward; do not start a second.
    FocusTheOpenOne { pid: u32 },
}

/// Whether a request for a surface starts a process or focuses the one that
/// is already there.
///
/// With the daemon's loop live while a window is open, the tray's *Open
/// Vault* is clickable again the moment the first one appears -- which the
/// blocking version made impossible and therefore never had to answer. Two
/// vault windows on the same vault is two things that must agree, on the
/// surface that edits it.
///
/// `already_open` is the child's process id, which is the daemon's whole
/// record that a window exists: it is what the spawn returned, it is what the
/// result file is named by, and it is what the focus below is aimed at. There
/// is no second registry to disagree with it.
/// `hidden` is whether that open window has hidden itself after a plain
/// close. It is a second argument rather than a third state of
/// `already_open` because the pid means the same thing either way -- the
/// process exists and is ours -- and only what to DO with it differs.
pub fn open_decision(already_open: Option<u32>, hidden: bool) -> UiOpenDecision {
    match (already_open, hidden) {
        (Some(pid), true) => UiOpenDecision::ShowTheHiddenOne { pid },
        (Some(pid), false) => UiOpenDecision::FocusTheOpenOne { pid },
        (None, _) => UiOpenDecision::Spawn,
    }
}

/// **What a daemon that is going away for good does with the window it has
/// open**, decided where a test can watch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Farewell {
    /// No UI process is open; there is nothing to do.
    NothingOpen,
    /// End this process before exiting.
    CloseIt { pid: u32 },
}

/// **Why the open UI window is being closed**, which is not the same question
/// as why the daemon is going away.
///
/// [`DaemonExit`] answers the second and converts into this; it keeps its own
/// name because the quit path genuinely is asking a daemon-lifecycle question.
/// The third reason is not a daemon lifecycle event at all -- the daemon is
/// running, will keep running, and has just been told by Windows that the
/// person using it left the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhyClose {
    /// The tray's *Quit*: the user asked for the app to be gone. The quit
    /// handler has just killed `bw serve` and cleared the vault cache, the
    /// breach results and the clipboard, precisely so that nothing decrypted
    /// outlives the moment the user said to go away -- and a vault window left
    /// running is a process still showing that user's entire decrypted vault,
    /// on screen, with no app behind it and no auto-lock timer that means
    /// anything any more.
    DaemonIsQuitting,
    /// The process is ending and expects to be back -- an update swapping the
    /// binary, or a crash. **The window stays up.** This is the same
    /// distinction [`UiSpawnPlan::joins_the_daemons_job`] is about, drawn one
    /// level up: the daemon comes back, brings `bw serve` up on the same port,
    /// and the window's next request succeeds.
    DaemonIsRestarting,
    /// Windows reported that the user walked away -- Win+L, a session switch,
    /// or a suspend -- **and `away_lock::locks_the_vault` said that locks this
    /// vault**. That gate is why this arm carries no preference of its own: a
    /// value of this variant cannot exist unless the user's own auto-lock
    /// setting already answered yes.
    ///
    /// The daemon is not going anywhere. What is going away is the decrypted
    /// vault, and the largest piece of it is in another process.
    TheUserWalkedAway,
}

impl From<DaemonExit> for WhyClose {
    fn from(exit: DaemonExit) -> Self {
        match exit {
            DaemonExit::UserQuit => WhyClose::DaemonIsQuitting,
            DaemonExit::Restart => WhyClose::DaemonIsRestarting,
        }
    }
}

/// Whether the daemon closes the UI window it has open.
///
/// **Two of the three reasons close it and one does not**, and that
/// distinction is the whole content of this function. A daemon *restart* must
/// not close the user's window, because the daemon comes back and recovery is
/// a retry rather than a handshake. A **Quit** is not a restart: nothing comes
/// back. A **workstation lock** is not a restart either, and is the one reason
/// here that leaves the daemon alive -- what ends is the decrypted vault, and
/// this process is holding the visible copy of it.
///
/// The cost of both closing arms is an edit in progress in that window. It is
/// the same cost the window's own idle auto-lock already charges, which closes
/// the viewport without asking (`vault_window::idle_frame` -> `IdleFrame::Lock`).
pub fn farewell_to_an_open_window(reason: WhyClose, open: Option<u32>) -> Farewell {
    match (reason, open) {
        (WhyClose::DaemonIsQuitting | WhyClose::TheUserWalkedAway, Some(pid)) => {
            Farewell::CloseIt { pid }
        }
        // Matched rather than caught by a wildcard so that a fourth reason is
        // a compile error here -- the one place that has to weigh it -- rather
        // than a silent inheritance of "leave it alone".
        (WhyClose::DaemonIsRestarting, _) | (_, None) => Farewell::NothingOpen,
    }
}

/// Why the daemon is going away. See [`WhyClose`], which this converts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExit {
    /// The tray's *Quit*: the user asked for the app to be gone.
    UserQuit,
    /// The process is ending and expects to be back -- an update swapping the
    /// binary, or a crash. The window stays up.
    Restart,
}

/// **Why a UI process could not open its window at all**, widened from one
/// number into a value the daemon can act on.
///
/// `main.rs`'s `UI_COULD_NOT_START` is a single exit code standing in for
/// five unrelated failures, and the daemon's whole reaction to it was one
/// `log::warn!`. To the user that is a tray click that did nothing --
/// reported, on a shipped build, as "the main UI does not open". A window
/// that cannot open has to say so, and it has to say WHICH thing is missing,
/// because the five have five different remedies: one is a restart that
/// re-downloads the CLI, one is a sign-in, and one is a preference to turn
/// back off.
///
/// So the child names the cause on the way out and the daemon reads it back.
/// The codes start at [`Self::FIRST_EXIT_CODE`] and run upwards, **above**
/// `UI_COULD_NOT_START` (64) and `UI_LAUNCH_REFUSED` (65) and far above the
/// 0-15 the [`UiVaultResult`] bitfield uses, for the reason those two are
/// where they are: exit 3 read as a result is `locked | needs_reauth`, and a
/// child that never drew a frame must not be able to tear `bw serve` down.
/// `main`'s `the_could_not_start_codes_cannot_be_mistaken_for_an_outcome`
/// walks [`Self::ALL`] and holds that.
///
/// The existing codes are untouched. `UI_COULD_NOT_START` stays 64 and stays
/// meaningful: it is what a child exits with when it has no more specific
/// answer, and the daemon still handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiStartFailure {
    /// No per-user config directory, or it could not be created. Nothing
    /// about this account was ever read.
    NoConfigDirectory,
    /// `bw.exe` could not be resolved, or is not where it was resolved to.
    NoBwExe,
    /// No saved session for this account. Signing in belongs to the daemon,
    /// so the child cannot fix this itself.
    NoSessionToken,
    /// **No longer produced by anything, and kept so the codes below it do
    /// not move.**
    ///
    /// It meant: this account is served by the built-in client, which reads
    /// the vault with a master key kept in `userkey.bin`, and there is no such
    /// file. That was a refusal only because a UI process could not ask for a
    /// master password. It can now -- a direct-REST child with no stored key
    /// opens the sign-in card and derives one -- so the condition this named
    /// is the ordinary first frame of a window rather than a reason not to
    /// open it.
    ///
    /// Retained rather than deleted because [`Self::ALL`] is what
    /// [`Self::exit_code`] indexes: removing a variant would silently
    /// renumber `NoServerUrl`, and exit codes are the one thing here a
    /// half-updated shortcut or a stale log line can still be read against.
    /// `the_start_failure_codes_are_stable` holds the numbering.
    NoStoredVaultKey,
    /// Served by the built-in client, but no server address is recorded, so
    /// there is nothing to read the vault from.
    NoServerUrl,
}

impl UiStartFailure {
    /// Every variant, in code order. Public so the daemon's range guard can
    /// walk them rather than trusting a hand-written list that a sixth
    /// variant would silently fall out of.
    pub const ALL: [Self; 5] = [
        Self::NoConfigDirectory,
        Self::NoBwExe,
        Self::NoSessionToken,
        Self::NoStoredVaultKey,
        Self::NoServerUrl,
    ];

    /// 66. One past `UI_LAUNCH_REFUSED`, which is one past
    /// `UI_COULD_NOT_START`.
    pub const FIRST_EXIT_CODE: i32 = 66;

    /// What the child exits with.
    #[must_use]
    pub fn exit_code(self) -> i32 {
        let at = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::FIRST_EXIT_CODE + at as i32
    }

    /// The inverse, and `None` for every code that is not one of these --
    /// including 64, 65, a panic's 101, and the whole result bitfield.
    #[must_use]
    pub fn from_exit_code(code: i32) -> Option<Self> {
        let at = usize::try_from(code - Self::FIRST_EXIT_CODE).ok()?;
        Self::ALL.get(at).copied()
    }

    /// **What goes on the screen.** Not a log line and not "something went
    /// wrong": it names what is missing and the one thing the user can do
    /// about it, which is the same rule `prefs_ui` holds its copy to.
    ///
    /// The two built-in-client sentences end with the preference that caused
    /// them, because that is the change the user made and the change they can
    /// undo.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NoConfigDirectory => {
                "Deskwarden could not open the vault window: it could not read or create its \
                 settings folder. Check that your user profile folder is available, then try \
                 again."
            }
            // **"Install Deskwarden again to put it back" was false**, and
            // had been since 0.15.1: the installer stopped bundling and
            // stopped downloading the CLI, so a re-install produces no
            // `bw.exe` at all and the user is sent round a loop. Deskwarden
            // fetches it itself now -- `bw_acquire::acquire_if_needed`, from
            // the sign-in card's Submit and from `main`'s own
            // `recover_a_missing_cli_with`, which offers on the next launch
            // that tries to start the vault service. That offer is the
            // remedy this names, because it is the one the user can reach
            // without signing out first.
            Self::NoBwExe => {
                "Deskwarden could not open the vault window: the Bitwarden CLI (bw.exe) that \
                 serves this account's vault is not on this PC. Restart Deskwarden and it \
                 will offer to download and install it for you."
            }
            Self::NoSessionToken => {
                "Deskwarden could not open the vault window: there is no saved session for \
                 this account. Sign in again from the Deskwarden tray icon."
            }
            Self::NoStoredVaultKey => {
                "Deskwarden could not open the vault window. This account is set to use \
                 Deskwarden's built-in client, but no vault key is stored on this PC for it, \
                 and the window has no way to ask for your master password. Sign in through \
                 Deskwarden once to store one, or turn the built-in client off in Preferences."
            }
            Self::NoServerUrl => {
                "Deskwarden could not open the vault window. This account is set to use \
                 Deskwarden's built-in client, but no server address is recorded for it, so \
                 there is nothing to read the vault from. Sign in through Deskwarden again \
                 with your server address, or turn the built-in client off in Preferences."
            }
        }
    }

    /// The same fact for the log file, in one clause.
    #[must_use]
    pub fn log_line(self) -> &'static str {
        match self {
            Self::NoConfigDirectory => "there is no readable config directory",
            Self::NoBwExe => "bw.exe could not be resolved",
            Self::NoSessionToken => "there is no saved session token",
            Self::NoStoredVaultKey => {
                "this account is served by the built-in client and no vault key is stored for it"
            }
            Self::NoServerUrl => {
                "this account is served by the built-in client and has no server URL"
            }
        }
    }
}

/// **Whether a direct-REST account gives a UI process anything to open a
/// window on.**
///
/// One pure function with two callers that must never disagree: the UI child,
/// which uses it to pick the exit code it dies with, and the daemon, which
/// asks it BEFORE spawning so that it never starts a child whose only possible
/// act is to exit.
///
/// # It used to ask about the stored key, and it deliberately no longer does
///
/// The old rule was "no `userkey.bin`, no window", because a UI process could
/// not ask for a master password -- so a direct-REST account with no stored
/// key was a window the child could not fill, and the daemon opened it
/// **itself**, which is precisely how the daemon came to hold the OpenGL
/// driver for the rest of its life.
///
/// A direct-REST child signs in now. A missing key is the ordinary opening
/// state of that window rather than a reason to refuse it: the card derives
/// one, `child_sign_in_backend_env`'s sink writes it to the same per-account
/// `userkey.bin` the daemon reads, and the window carries on into the vault.
///
/// # What still refuses, and why it is not the same question
///
/// A missing server URL. There is nothing to sign in *to*, so no card can
/// help -- this is an account record that is incomplete rather than one that
/// is merely locked, and no amount of typing a password fixes it.
///
/// `choose` cannot answer `DirectRest` without a self-hosted URL, so in
/// practice this arm is unreachable through an `Account`; it is answered
/// anyway because the child reads the URL as an `Option` off a record the
/// daemon may have rewritten since, and "unreachable" is not a thing a
/// process boundary lets either side assume.
#[must_use]
pub fn direct_rest_start_failure(a_server_url_is_recorded: bool) -> Option<UiStartFailure> {
    if !a_server_url_is_recorded {
        return Some(UiStartFailure::NoServerUrl);
    }
    None
}

/// **The vault window's seven daemon-actionable outcomes**, as they cross the
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
/// `signed_in` is the newest field and holds to the same bar: an email, a
/// server address and a backend answer, which are the three things
/// `settings.json` is *made of*. The master password that produced them never
/// leaves the child's address space, and the key it derived goes to that
/// account's own `userkey.bin` rather than through here.
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
    /// **What a direct-REST sign-in in this child established**, for the
    /// daemon to adopt into `settings.json`.
    ///
    /// `Some` only when this child actually drew a sign-in card and the user
    /// completed it. A window that opened straight onto the vault -- which is
    /// every launch with a usable stored key -- leaves this `None`.
    ///
    /// **Direct-REST only, and that is a scope fact rather than a shape
    /// one.** A `bw serve` sign-in still happens in the daemon, because the
    /// token it produces is what the daemon must start `bw serve` WITH -- and
    /// a carrier that only arrives when this process exits cannot serve a
    /// backend the window needs before it can draw. See this module's doc.
    ///
    /// A payload field, so like `switch_to` and `edited_settings` it has no
    /// exit-code backup and its loss is recoverable: `userkey.bin` is already
    /// written by the time this is, so a child killed between the two leaves a
    /// working sign-in whose *address* the daemon relearns on its next settle.
    /// That is the same cost a stale email already carries, and it is why this
    /// is not worth a second carrier.
    pub signed_in: Option<crate::login_ui::SignedInIdentity>,
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
    /// **Only the booleans.** `switch_to`, `edited_settings` and `signed_in`
    /// cannot be reconstructed from a status word and are left empty here; the
    /// file is the only carrier for those, and [`UiVaultResult::union`] is
    /// where the two sources are put back together.
    pub fn from_exit_code(code: i32) -> Self {
        Self {
            locked: code & Self::EXIT_LOCKED != 0,
            needs_reauth: code & Self::EXIT_NEEDS_REAUTH != 0,
            add_account: code & Self::EXIT_ADD_ACCOUNT != 0,
            remove_account: code & Self::EXIT_REMOVE_ACCOUNT != 0,
            edited_settings: None,
            switch_to: None,
            signed_in: None,
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
            // No exit-code carrier exists for any of the three; the file is
            // all there is.
            edited_settings: file.edited_settings,
            switch_to: file.switch_to,
            signed_in: file.signed_in,
        }
    }
}

/// Where a UI process leaves its result.
///
/// **Named by the child's own process id**, so a stale file from a UI process
/// that was killed cannot be read as this one's answer, and two UI processes
/// (once there is more than one surface) cannot write over each other. The
/// What a closing vault window does with its process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// Hide the viewport and stay resident, ready to be shown again.
    Hide,
    /// End the process, which is how every result gets home.
    Exit,
}

/// **Whether this close hides the window or ends its process.**
///
/// Every field of [`UiVaultResult`] is something the daemon acts on, and a
/// window that hid while holding one it had not delivered would be a lock,
/// a switch or a settings edit that silently never happened.
///
/// **Five of the six can only travel by this process exiting.** They are
/// each a *reason the window closed* -- the daemon's answer to every one
/// is `resettle_session` or an account settle, and it needs the window
/// gone to run either. So they still force an exit, whatever else is true.
///
/// **`edited_settings` is the exception, and it always was the odd one
/// out**: its own doc says it is "not a reason the window closed -- the
/// window closed for whatever it closed for, and this rides along". It now
/// has a second route home -- `vault_window::HideHooks::deliver_settings`, a
/// file and a doorbell the daemon reads while this process is still alive --
/// and `settings_delivered` says whether that route was taken. It is a
/// parameter rather than a field of the result because it is a fact about
/// the CHANNEL, not about the window's outcome: the result crossing the
/// process boundary is unchanged, and the daemon never sees this flag.
///
/// **This still does not mirror `vault_follow_up`'s `Done`.** That
/// function does not read `edited_settings` at all, because by the time it
/// is consulted the daemon has applied it. Hiding on `Done` alone would
/// hide an UNDELIVERED edit, which is the defect this whole rule exists to
/// prevent. `the_hide_rule_is_stricter_than_done` in `main.rs` holds the
/// two together over every combination of the six fields, at both values
/// of this flag.
#[must_use]
pub fn on_close(keep_loaded: bool, result: &UiVaultResult, settings_delivered: bool) -> OnClose {
    let nothing_to_report = !result.locked
        && !result.needs_reauth
        && !result.add_account
        && !result.remove_account
        && result.switch_to.is_none()
        && (result.edited_settings.is_none() || settings_delivered);
    if keep_loaded && nothing_to_report {
        OnClose::Hide
    } else {
        OnClose::Exit
    }
}

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

/// **Where a resident UI process leaves a preferences edit for the daemon
/// to pick up, without exiting to deliver it.**
///
/// Named by pid for [`result_path`]'s reasons, and a DIFFERENT name from
/// it: that file is the child's last act and is deleted by the daemon
/// after the reap, so sharing it would be a truncating write racing a
/// delete across two processes.
pub fn edited_settings_path(config_dir: &Path, pid: u32) -> PathBuf {
    config_dir.join(format!("ui-settings-{pid}.json"))
}

/// Write a preferences edit for the daemon, **atomically**.
///
/// Temp-then-rename rather than `fs::write`, because unlike the result
/// file this one is read while both processes are alive. `fs::write`
/// truncates before it writes, so a daemon that polled on a timer could
/// read an empty file. Production never polls on a timer -- it reads only
/// after the doorbell, which is set after this returns -- and the rename
/// is the belt to that braces: a second delivery over the first cannot be
/// observed half-applied either.
pub fn write_edited_settings(path: &Path, settings: &Settings) -> io::Result<()> {
    let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    // `fs::rename` is `MoveFileEx` with `REPLACE_EXISTING` on Windows, so
    // this lands over an earlier delivery rather than failing on it.
    std::fs::rename(&temp, path)
}

/// Read a preferences edit, from the daemon, after the doorbell rang.
///
/// `None` for every failure -- absent, unreadable, unparseable -- and each
/// is the same thing from the daemon's side: nothing to apply. Logged
/// rather than silent, for [`read_result`]'s reason: a consistently
/// unparseable file presents to the user as "settings changed in the vault
/// window sometimes do not stick".
#[must_use]
pub fn read_edited_settings(path: &Path) -> Option<Settings> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => Some(settings),
            Err(e) => {
                log::warn!(
                    "a UI process's settings delivery at {} did not parse ({e})",
                    path.display()
                );
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!(
                "could not read a UI process's settings delivery at {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// **Where a UI process leaves the sign-in it just completed, so the daemon
/// can start `bw serve` while that window is still on screen.**
///
/// The third per-pid file, and a DIFFERENT name from the other two for
/// [`edited_settings_path`]'s reason and one more. [`result_path`] is the
/// child's last act; this one is written in the MIDDLE of the child's life,
/// while it waits on the spinner -- so sharing a name with the result would
/// be a write racing the daemon's post-reap read, and sharing one with the
/// settings delivery would have a preferences edit and a sign-in overwriting
/// each other in a window that does both.
///
/// # What is in it, and why that is allowed to be in a file
///
/// A [`crate::login_ui::SignedInIdentity`]: an account id, an email, a
/// server URL, and what the backend-choice modal answered. Every one of
/// those is already written into `settings.json` in the clear, in this same
/// directory -- which is the bar this module's own doc sets, and the same
/// bar [`UiVaultResult::signed_in`] already clears carrying this exact type
/// home by the other route.
///
/// **The session token is NOT in it, and that is the design and not an
/// omission.** The child writes the token into that account's own DPAPI
/// `session.bin` through [`crate::session_store::SessionStore`] before it
/// rings, and the daemon loads it back out of that same store. So the secret
/// goes only where it already lives, wrapped under the user's credentials,
/// and never appears in a file the daemon has to read as plain JSON. The
/// master password appears nowhere at all: not here, not in an argument, not
/// in an environment variable.
pub fn signed_in_path(config_dir: &Path, pid: u32) -> PathBuf {
    config_dir.join(format!("ui-signin-{pid}.json"))
}

/// Write the sign-in a UI process just completed, **atomically**.
///
/// Temp-then-rename for [`write_edited_settings`]'s reason and with more at
/// stake: this file is read while both processes are alive, and the daemon
/// acts on it by starting a subprocess. A half-written file that parsed
/// would be a `bw serve` started for the wrong account.
pub fn write_signed_in(
    path: &Path,
    identity: &crate::login_ui::SignedInIdentity,
) -> io::Result<()> {
    let json = serde_json::to_string_pretty(identity).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

/// Read the sign-in a UI process delivered, from the daemon, after the
/// doorbell rang.
///
/// `None` for every failure -- absent, unreadable, unparseable -- and each is
/// the same thing from the daemon's side: there is no sign-in here to act on,
/// so nothing is started and the child is not told the backend is ready. It
/// will give up on its own deadline and say so, which is the honest outcome;
/// starting `bw serve` off a file that did not parse would be starting it for
/// an account nobody named.
#[must_use]
pub fn read_signed_in(path: &Path) -> Option<crate::login_ui::SignedInIdentity> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<crate::login_ui::SignedInIdentity>(&text) {
            Ok(identity) => Some(identity),
            Err(e) => {
                log::warn!(
                    "a UI process's sign-in delivery at {} did not parse ({e}); no backend \
                     will be started for it",
                    path.display()
                );
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!(
                "could not read a UI process's sign-in delivery at {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Delete a sign-in delivery once it has been acted on. Best effort, for
/// [`forget_result`]'s reason.
pub fn forget_signed_in(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {} after acting on it: {e}", path.display());
        }
    }
}

/// Delete a delivery once it has been applied. Best effort, for
/// [`forget_result`]'s reason.
pub fn forget_edited_settings(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {} after applying it: {e}", path.display());
        }
    }
}

/// **Where the daemon leaves the search term a UI process is to open with.**
///
/// The first channel in this module that runs DAEMON to CHILD; every other
/// one runs the other way. It exists because the overlay's *Search vault*
/// button is the one door into this window that carries a value, and the
/// command line -- which every process on the machine can read -- is the one
/// place that value may not go. That rule is why this route used to keep the
/// window in the daemon and pay the OpenGL driver for it.
///
/// **Not named by pid, unlike every other file here**, and that is forced
/// rather than chosen: the child must read this at startup, and the pid does
/// not exist until the spawn has already happened. A pid-named file would
/// have to be written after `CreateProcess` returned, which is a race the
/// child would lose. One fixed name is safe because
/// [`UiOpenDecision`](crate::ui_process::UiOpenDecision) allows exactly one
/// vault process at a time, so there is never a second reader to confuse.
///
/// **The daemon writes it on EVERY vault spawn**, with an empty term when
/// there is no search -- see [`write_initial_search`]. That is what makes a
/// stale file impossible: the only way to leave one behind is for a child to
/// die between the spawn and its own first read, and the next spawn
/// overwrites it before a window exists to be seeded by it.
///
/// **It carries no secret.** The term is an app's display name or a vault
/// item id -- the same class of thing `settings.json` and the disk cache
/// already hold in the clear in this very directory. The objection was never
/// to the value; it was to the command line, which is world-readable, and
/// this directory is not.
#[must_use]
pub fn initial_search_path(config_dir: &Path) -> PathBuf {
    config_dir.join("ui-initial-search.json")
}

/// Leave the term for the child that is about to be spawned, **atomically**.
///
/// Temp-then-rename for [`write_edited_settings`]'s reason. The ordering that
/// makes this safe with no doorbell is the spawn itself: this returns before
/// `CreateProcess` is called, so the file is on disk before the process that
/// reads it exists.
///
/// An empty term is written rather than skipped, so that the file the child
/// finds always belongs to the child's own launch. Failure is reported, and
/// the caller's answer is to spawn anyway: a window that opens with an empty
/// box is the behaviour every other door already has.
pub fn write_initial_search(path: &Path, term: &str) -> io::Result<()> {
    let json = serde_json::to_string(term).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

/// Read the term, from the child, at startup -- and **delete it in the same
/// breath**.
///
/// Taken rather than read, so that one write seeds one window. A `keep_ui_loaded`
/// process outlives its first window, and a term left on disk would come back
/// the next time that process was asked to show itself -- which is the search
/// the user ran ten minutes ago reappearing in a box they opened for something
/// else.
///
/// Empty answers `None`: "no search" and "search for nothing" are the same
/// window, and the empty string is what [`write_initial_search`] writes when
/// there is no term. Every failure -- absent, unreadable, unparseable -- is
/// also `None`, which is the ordinary window.
#[must_use]
pub fn take_initial_search(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok();
    // Deleted whether or not it parsed: an unreadable file left in place
    // would be re-read by every window this process opens.
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {} after reading it: {e}", path.display());
        }
    }
    let term = serde_json::from_str::<String>(&text?)
        .map_err(|e| {
            log::warn!("the initial search term left for this window did not parse ({e})");
        })
        .ok()?;
    (!term.is_empty()).then_some(term)
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
    /// A hidden window is SHOWN, not spawned and not raised. Raising is
    /// what `FocusTheOpenOne` does and it cannot work here: there is no
    /// window on screen for `raise_process` to bring forward.
    #[test]
    fn a_hidden_window_is_shown() {
        assert_eq!(
            open_decision(Some(77), true),
            UiOpenDecision::ShowTheHiddenOne { pid: 77 }
        );
    }

    /// A visible window is still focused, and still nothing is spawned.
    /// Two vault windows on one vault is two editors of the same records.
    #[test]
    fn a_visible_window_is_still_focused() {
        assert_eq!(open_decision(Some(77), false), UiOpenDecision::FocusTheOpenOne { pid: 77 });
    }

    /// No window at all is a spawn whatever `hidden` says -- there is
    /// nothing to hide. The `true` arm is not reachable in production, and
    /// is asserted so that a future caller passing a stale flag cannot
    /// turn "no window" into "show the window that is not there".
    #[test]
    fn no_window_is_still_a_spawn() {
        assert_eq!(open_decision(None, false), UiOpenDecision::Spawn);
        assert_eq!(open_decision(None, true), UiOpenDecision::Spawn);
    }
    /// The one case that hides: the setting is on and the user just
    /// closed the window with nothing to report.
    #[test]
    fn a_plain_close_hides_when_the_setting_is_on() {
        assert_eq!(on_close(true, &UiVaultResult::default(), true), OnClose::Hide);
    }

    /// **Off means today's behaviour, exactly.** With the setting off no
    /// result hides, or the setting would not be a setting.
    #[test]
    fn nothing_hides_when_the_setting_is_off() {
        assert_eq!(on_close(false, &UiVaultResult::default(), true), OnClose::Exit);
    }

    /// **Every outcome the daemon acts on exits**, so the result file, the
    /// reap and the resettle all keep working unchanged. Each is asserted
    /// by name rather than in a loop: a loop that built the wrong value
    /// would pass five times and prove nothing.
    #[test]
    fn every_outcome_the_daemon_acts_on_exits() {
        let locked = UiVaultResult { locked: true, ..Default::default() };
        assert_eq!(on_close(true, &locked, true), OnClose::Exit, "a lock must reach the daemon");

        let reauth = UiVaultResult { needs_reauth: true, ..Default::default() };
        assert_eq!(on_close(true, &reauth, true), OnClose::Exit, "a re-auth must reach the daemon");

        let switch =
            UiVaultResult { switch_to: Some(AccountId::generate()), ..Default::default() };
        assert_eq!(on_close(true, &switch, true), OnClose::Exit, "a switch must reach the daemon");

        let add = UiVaultResult { add_account: true, ..Default::default() };
        assert_eq!(on_close(true, &add, true), OnClose::Exit, "an add must reach the daemon");

        let remove = UiVaultResult { remove_account: true, ..Default::default() };
        assert_eq!(on_close(true, &remove, true), OnClose::Exit, "a remove must reach the daemon");
    }

    /// **An UNDELIVERED preferences edit still exits, which is the old rule
    /// exactly.**
    ///
    /// The only route `edited_settings` ever had to the daemon was this
    /// process ending. A window that hid holding an edit nobody had taken
    /// would withhold the estate copy, `apply_disk_cache_change`,
    /// `persist_preferences` and the clipboard re-install -- all four.
    #[test]
    fn an_undelivered_preferences_edit_still_exits() {
        let geared =
            UiVaultResult { edited_settings: Some(Settings::default()), ..Default::default() };
        assert_eq!(
            on_close(true, &geared, false),
            OnClose::Exit,
            "the window hid holding an edit no daemon had taken, so it never arrived"
        );
    }

    /// **A DELIVERED preferences edit hides, and this is the whole defect.**
    ///
    /// `edited_settings` stays `Some` for the rest of a window's life, so
    /// under the old rule one visit to the gear made every later close an
    /// exit -- and the gear is where *Open the vault instantly* lives. The
    /// user turned the setting on and was rewarded with a cold start and a
    /// Windows Hello prompt on the very next open.
    ///
    /// It may hide now because the edit is no longer being withheld: the
    /// daemon took it over the live channel while this window was still up.
    #[test]
    fn a_delivered_preferences_edit_hides() {
        let geared =
            UiVaultResult { edited_settings: Some(Settings::default()), ..Default::default() };
        assert_eq!(
            on_close(true, &geared, true),
            OnClose::Hide,
            "a visit to Preferences still ends the process, which is the reported defect"
        );
    }

    /// **Delivery relaxes ONE field and no other.** A locked window exits
    /// however delivered its settings are: a lock is a reason the window
    /// closed and the daemon's whole response to it is a teardown this
    /// process has to be gone for.
    #[test]
    fn delivering_settings_does_not_excuse_any_other_outcome() {
        for (what, result) in [
            ("a lock", UiVaultResult { locked: true, ..Default::default() }),
            ("a re-auth", UiVaultResult { needs_reauth: true, ..Default::default() }),
            ("an add", UiVaultResult { add_account: true, ..Default::default() }),
            ("a remove", UiVaultResult { remove_account: true, ..Default::default() }),
            (
                "a switch",
                UiVaultResult { switch_to: Some(AccountId::generate()), ..Default::default() },
            ),
        ] {
            let with_settings = UiVaultResult {
                edited_settings: Some(Settings::default()),
                ..result.clone()
            };
            assert_eq!(
                on_close(true, &result, true),
                OnClose::Exit,
                "{what} must reach the daemon"
            );
            assert_eq!(
                on_close(true, &with_settings, true),
                OnClose::Exit,
                "{what} hid because the settings beside it had been delivered"
            );
        }
        // Control: the empty result DOES hide under the same flag, so the
        // loop above is not passing because nothing ever hides.
        assert_eq!(on_close(true, &UiVaultResult::default(), true), OnClose::Hide);
    }
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
    fn a_running_child_is_left_alone() {
        assert_eq!(reap_step(Ok(None)), Reap::Keep);
    }

    #[test]
    fn an_exited_child_is_reaped_with_its_code() {
        assert_eq!(reap_step(Ok(Some(Some(9)))), Reap::Take { code: Some(9) });
    }

    #[test]
    fn a_child_that_cannot_be_waited_on_is_still_reaped() {
        assert_eq!(
            reap_step(Err(())),
            Reap::Take { code: None },
            "a handle that can no longer be waited on means the window is gone. Kept, the              daemon believes one is open forever and the one-window rule then refuses every              later Open Vault for the life of the process"
        );
    }

    #[test]
    fn a_child_killed_without_a_code_is_reaped_too() {
        assert_eq!(reap_step(Ok(Some(None))), Reap::Take { code: None });
    }

    #[test]
    fn a_surface_with_nothing_open_spawns() {
        assert_eq!(open_decision(None, false), UiOpenDecision::Spawn);
    }

    #[test]
    fn a_surface_that_is_already_open_is_focused_rather_than_opened_again() {
        assert_eq!(
            open_decision(Some(4242), false),
            UiOpenDecision::FocusTheOpenOne { pid: 4242 },
            "two vault windows on one vault is two editors of the same records; the second              request brings the first window forward"
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

    /// A scratch directory, in this module's own idiom -- this crate has no
    /// `tempfile` dev-dependency, so `%TEMP%` keyed by pid and line is what
    /// every filesystem test here uses.
    fn scratch(tag: &str, line: u32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-ui-settings-{tag}-{}-{line}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// **A different file from the result**, and that is the point: the result
    /// file is written once on the way out and deleted by the daemon after the
    /// reap. A live channel sharing it would be a truncating write racing a
    /// delete.
    #[test]
    fn the_live_settings_file_is_not_the_result_file() {
        let dir = Path::new(r"C:\config");
        assert_ne!(edited_settings_path(dir, 77), result_path(dir, 77));
        assert_ne!(edited_settings_path(dir, 77), edited_settings_path(dir, 78));
        assert!(
            edited_settings_path(dir, 77).to_string_lossy().contains("77"),
            "not named by pid, so a dead window's file could be read as a live one's"
        );
    }

    /// **Three per-pid files, all different**, and the sign-in delivery is
    /// the third.
    ///
    /// Sharing the result file's name would be a mid-life write racing the
    /// daemon's post-reap delete; sharing the settings delivery's would have
    /// a window that both signs in and edits preferences overwriting one
    /// with the other -- and the loser would be a `bw serve` that never
    /// started, behind a spinner that never ended.
    #[test]
    fn the_sign_in_delivery_is_its_own_file() {
        let dir = Path::new(r"C:\config");
        assert_ne!(signed_in_path(dir, 77), result_path(dir, 77));
        assert_ne!(signed_in_path(dir, 77), edited_settings_path(dir, 77));
        assert_ne!(signed_in_path(dir, 77), signed_in_path(dir, 78));
        assert!(
            signed_in_path(dir, 77).to_string_lossy().contains("77"),
            "not named by pid, so a dead window's sign-in could start a backend for a live \
             one's account"
        );
    }

    /// **The sign-in round trip, and it carries no credential.**
    ///
    /// The round trip is the obvious half. The half worth pinning is the
    /// other one: the bytes that actually land on disk are searched for the
    /// token and the password, because the whole licence for putting this
    /// file in the config directory is that it holds nothing a
    /// `settings.json` does not already hold in the clear.
    #[test]
    fn a_sign_in_delivery_round_trips_and_holds_no_secret() {
        let dir = scratch("signin", line!());
        let path = signed_in_path(&dir, 77);

        assert!(
            read_signed_in(&path).is_none(),
            "control: read something before anything was written"
        );

        let identity = crate::login_ui::SignedInIdentity {
            account: Some(AccountId::generate()),
            user_email: Some("someone@example.com".to_string()),
            server_url: Some("https://vault.example.com".to_string()),
            use_official_bw_crypto: Some(false),
        };
        write_signed_in(&path, &identity).expect("the write should succeed");
        assert_eq!(read_signed_in(&path).as_ref(), Some(&identity));

        // **The rename really is the landing.** A truncate-in-place
        // implementation passes the round trip above and still leaves a
        // window in which the daemon reads an empty file.
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the temp file survived the write, so the rename is not what lands"
        );

        // **Nothing secret is in the bytes.** These two values are what the
        // child holds at the moment it writes this file; neither may appear.
        let raw = std::fs::read_to_string(&path).expect("the file should be readable");
        for secret in ["the-session-token", "correct-horse-battery-staple"] {
            assert!(
                !raw.contains(secret),
                "the sign-in delivery contains {secret:?}. The token goes to the account's \
                 DPAPI `session.bin` and the password goes nowhere at all: {raw}"
            );
        }

        forget_signed_in(&path);
        assert!(
            read_signed_in(&path).is_none(),
            "the delivery survived being forgotten, so a second pass would start `bw serve` \
             again for a sign-in already acted on"
        );
    }

    /// **A reader never sees half a file**, because the write lands by rename.
    ///
    /// Asserted by writing a SECOND, different settings over the first and
    /// reading back: a truncate-in-place implementation passes the round trip
    /// but leaves a window in which the file is empty. The temp file is
    /// asserted gone, which is the observable consequence of the rename
    /// actually being the landing.
    #[test]
    fn an_edited_settings_file_lands_whole_and_leaves_no_temp_behind() {
        let dir = scratch("whole", line!());
        let path = edited_settings_path(&dir, 77);

        assert!(
            read_edited_settings(&path).is_none(),
            "control: read something before anything was written"
        );

        let first = Settings::default();
        write_edited_settings(&path, &first).expect("the write should succeed");
        assert_eq!(read_edited_settings(&path).as_ref(), Some(&first));

        let second = Settings { check_breaches: !first.check_breaches, ..first.clone() };
        write_edited_settings(&path, &second).expect("the second write should succeed");
        assert_eq!(
            read_edited_settings(&path).as_ref(),
            Some(&second),
            "the second delivery did not replace the first"
        );

        let strays: Vec<_> = std::fs::read_dir(&dir)
            .expect("readable")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "the write left temp files behind: {strays:?}");

        forget_edited_settings(&path);
        assert!(
            read_edited_settings(&path).is_none(),
            "the daemon deleted the file and can still read it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The one channel that runs daemon-to-child, over real files.**
    ///
    /// The term the overlay's *Search vault* carries used to have nowhere to
    /// go but a command line, which is why that door kept its window in the
    /// daemon and paid the OpenGL driver for it. What is asserted here is the
    /// property that makes the file a replacement rather than a second copy
    /// of the same problem: it is TAKEN, so one write seeds exactly one
    /// window.
    ///
    /// The second `take` is the whole test. A read that left the file behind
    /// would pass a plain round trip and still put the user's ten-minute-old
    /// search back in the box every time a `keep_ui_loaded` process was asked
    /// to show itself.
    #[test]
    fn an_initial_search_seeds_one_window_and_is_then_gone() {
        let dir = scratch("initial-search", line!());
        let path = initial_search_path(&dir);

        assert!(
            take_initial_search(&path).is_none(),
            "control: a term came back before anything was written, so the assertions below \
             would be about a file that is never read"
        );

        write_initial_search(&path, "example.com").expect("the write should succeed");
        assert_eq!(
            take_initial_search(&path).as_deref(),
            Some("example.com"),
            "the term the daemon left was not the term the child read"
        );
        assert!(
            take_initial_search(&path).is_none(),
            "the term survived being taken, so every later window this process opens is \
             seeded with a search the user ran once and did not ask for again"
        );

        // **An empty write is how a launch with no search says so**, and it
        // is what makes a stale file impossible: the daemon writes on EVERY
        // spawn. If empty read back as `Some("")` the next window would open
        // filtered to nothing rather than showing the vault.
        write_initial_search(&path, "term").expect("writable");
        write_initial_search(&path, "").expect("writable");
        assert!(
            take_initial_search(&path).is_none(),
            "an empty term read back as a search, so a spawn with nothing to look for \
             would overwrite nothing and inherit the previous window's term"
        );

        // The rename really is the landing, asserted the way
        // `an_edited_settings_file_lands_whole_and_leaves_no_temp_behind`
        // asserts it.
        write_initial_search(&path, "second").expect("writable");
        let strays: Vec<_> = std::fs::read_dir(&dir)
            .expect("readable")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "the write left temp files behind: {strays:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A term that did not parse is the ordinary window, and **the file is
    /// still gone**.
    ///
    /// Deleting only on the success path is the tempting shape and it is the
    /// wrong one: an unreadable file left in place is re-read, and re-fails,
    /// for every window this process ever opens.
    #[test]
    fn an_unparseable_initial_search_is_ignored_and_still_consumed() {
        let dir = scratch("initial-search-bad", line!());
        let path = initial_search_path(&dir);
        std::fs::write(&path, "{ not a json string").expect("writable");

        assert!(
            take_initial_search(&path).is_none(),
            "an unparseable term was handed to a window"
        );
        assert!(
            !path.exists(),
            "the unparseable term is still on disk, so every window this process opens will \
             read it again and fail again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **It is not named by pid, and that is forced rather than careless.**
    ///
    /// The child reads this as it starts, and the pid does not exist until
    /// the spawn has already happened -- so a pid-named file would have to be
    /// written after `CreateProcess` returned, which is a race the child
    /// loses. Pinned because "name it by pid like the others" is exactly the
    /// tidying reflex that would reintroduce it, and the failure it would
    /// cause is a search box that is empty for no reason anybody could see.
    #[test]
    fn the_initial_search_is_not_named_by_a_process_id() {
        let dir = scratch("initial-search-name", line!());
        assert_eq!(
            initial_search_path(&dir),
            initial_search_path(&dir),
            "the name is not stable, so the child cannot know which file to open"
        );
        assert_ne!(
            initial_search_path(&dir),
            result_path(&dir, 77),
            "the term shares a name with the result file, which the daemon deletes after a \
             reap -- so the delete would race the child's own read"
        );
        assert_ne!(
            initial_search_path(&dir),
            edited_settings_path(&dir, 77),
            "the term shares a name with the settings delivery"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unparseable is `None` and a log line, never a panic -- the same answer
    /// `read_result` gives, and for the same reason: the daemon's response to
    /// a file it cannot read is to act on nothing, and the user's response is
    /// to change the setting again.
    #[test]
    fn an_unparseable_delivery_is_ignored_rather_than_fatal() {
        let dir = scratch("unparseable", line!());
        let path = edited_settings_path(&dir, 77);
        std::fs::write(&path, "{ not json").expect("writable");
        assert!(read_edited_settings(&path).is_none());

        // Control: the same path with real content reads back, so the
        // assertion above is not passing because the path is wrong.
        write_edited_settings(&path, &Settings::default()).expect("writable");
        assert!(
            read_edited_settings(&path).is_some(),
            "control: a valid file did not read either"
        );
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

    /// **The exit code each start failure dies with, written out by hand.**
    ///
    /// `exit_code` indexes [`UiStartFailure::ALL`], so the numbers are a
    /// property of that array's ORDER rather than of any variant -- which
    /// means deleting or reordering a variant silently renumbers every one
    /// after it, with nothing failing to say so. That is not hypothetical
    /// here: `NoStoredVaultKey` stopped being produced when the direct-REST
    /// sign-in moved into the child, and deleting it as dead would have
    /// quietly moved `NoServerUrl` from 69 to 68.
    ///
    /// The literals are the point. A test that recomputed them from `ALL`
    /// would agree with any renumbering it was handed, which is the house's
    /// own "a test that passes because it never reached the thing it names".
    #[test]
    fn the_start_failure_codes_are_stable() {
        for (failure, code) in [
            (UiStartFailure::NoConfigDirectory, 66),
            (UiStartFailure::NoBwExe, 67),
            (UiStartFailure::NoSessionToken, 68),
            (UiStartFailure::NoStoredVaultKey, 69),
            (UiStartFailure::NoServerUrl, 70),
        ] {
            assert_eq!(
                failure.exit_code(),
                code,
                "{failure:?} changed the code it exits with. These are read back by the \
                 daemon to pick the sentence it shows the user, so a renumbering turns one \
                 failure into another's message"
            );
            assert_eq!(
                UiStartFailure::from_exit_code(code),
                Some(failure),
                "{code} no longer reads back as {failure:?}"
            );
        }
        assert_eq!(
            UiStartFailure::ALL.len(),
            5,
            "a sixth start failure was added without being given a code above; the four \
             lines above are the whole of what a reader can check the numbering against"
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
            // **Populated, because an empty `signed_in` would make this test
            // pass by never reaching the field it exists to check.** This is
            // the newest thing to cross the boundary and the only one born of
            // a master password, so it is the one whose serialised form most
            // needs looking at. Every value here is a plausible real one.
            signed_in: Some(crate::login_ui::SignedInIdentity {
                account: Some(AccountId::generate()),
                user_email: Some("someone@example.com".to_string()),
                server_url: Some("https://vault.example.com".to_string()),
                use_official_bw_crypto: Some(false),
            }),
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

    /// **A workstation lock closes the window, and it is the same decision
    /// function that says so.**
    ///
    /// This arm is only ever reached downstream of
    /// `away_lock::locks_the_vault`, which is what makes it correct for this
    /// function to have no opinion about `auto_lock`: by the time a
    /// `TheUserWalkedAway` exists, the gate has already been passed.
    #[test]
    fn walking_away_closes_the_vault_window() {
        assert_eq!(
            farewell_to_an_open_window(WhyClose::TheUserWalkedAway, Some(4242)),
            Farewell::CloseIt { pid: 4242 },
            "a decrypted vault rendered on screen must not survive the moment its owner              locked the workstation and left; the daemon's own cache and bw serve are torn              down in the same breath and this process is the only thing left holding one"
        );
        assert_eq!(
            farewell_to_an_open_window(WhyClose::TheUserWalkedAway, None),
            Farewell::NothingOpen,
            "no window open is nothing to close -- the ordinary tray-only state this whole              feature was originally written for"
        );
    }

    /// The positive control on the test above: adding the third arm must not
    /// have flattened the distinction the first two arms exist to draw. A
    /// `match` that had degenerated into `Some(pid) => CloseIt` would pass
    /// every assertion above and fail exactly this one.
    #[test]
    fn a_restart_still_leaves_the_window_alone_now_that_a_third_reason_exists() {
        assert_eq!(
            farewell_to_an_open_window(WhyClose::DaemonIsRestarting, Some(4242)),
            Farewell::NothingOpen,
            "an update or a crash must still not close the user's window mid-edit"
        );
        assert_eq!(
            farewell_to_an_open_window(WhyClose::DaemonIsQuitting, Some(4242)),
            Farewell::CloseIt { pid: 4242 },
            "control: the quit arm the two callers below rely on is unchanged"
        );
    }

    /// The two existing call sites pass a `DaemonExit`. The conversion must be
    /// the identity they were relying on, or this refactor silently changes
    /// what quitting does.
    #[test]
    fn the_daemon_exit_reasons_convert_to_the_same_decisions_they_made_before() {
        assert_eq!(
            farewell_to_an_open_window(DaemonExit::UserQuit.into(), Some(7)),
            Farewell::CloseIt { pid: 7 }
        );
        assert_eq!(
            farewell_to_an_open_window(DaemonExit::Restart.into(), Some(7)),
            Farewell::NothingOpen
        );
        assert_eq!(WhyClose::from(DaemonExit::UserQuit), WhyClose::DaemonIsQuitting);
        assert_eq!(WhyClose::from(DaemonExit::Restart), WhyClose::DaemonIsRestarting);
    }
}

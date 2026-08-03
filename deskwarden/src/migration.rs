//! Migrates the pre-existing single-account Bitwarden CLI profile into
//! `accounts/<id>/`.
//!
//! **The one part of this app that can destroy a vault profile.** Everything
//! here is arranged around four rules, and each of them is a test:
//!
//! 1. **Copy, verify, repoint, delete. Never a move, never a rename of the
//!    source.** An interrupted `fs::rename` — or one that half-completes across
//!    a device boundary — leaves an encrypted profile in two incomplete halves
//!    and neither is usable. A copy leaves the original whole no matter when it
//!    is interrupted, so the worst outcome of any crash is wasted disk. The
//!    *only* rename here is [`incoming_path`] → [`accounts::data_dir_for`],
//!    within one directory on one volume, where neither side is the user's
//!    data.
//! 2. **Idempotent and resumable, with a marker whose lifecycle is defined.**
//!    The app can be killed at any instruction. The half-migrated state has to
//!    be *detectable* rather than inferred, so [`resume_action`] is a total
//!    function over [`Observed`] — everything the next startup can see.
//! 3. **Verified before the original is removed.** "Verified" is
//!    [`verification_passed`]: `bw status` run against the *new* directory
//!    parses to `Locked` or `Unlocked` **and** reports the same `userEmail` the
//!    source reported before the copy began. A file count or a byte total would
//!    pass for a copy with a truncated `data.json`; the CLI refusing to
//!    recognise the account is exactly the failure that must be caught while
//!    the old copy still exists.
//! 4. **Rollback on any failure.** Because nothing of the user's is deleted
//!    before rule 3 passes, rollback is always "delete what we created, clear
//!    the marker" — it can never need to *restore* anything.
//! 5. **No on-disk state leaves a real vault directory that nothing
//!    references.** The marker covers the migration; it does not cover the
//!    *account list*, which `main` writes only after [`migrate`] has returned.
//!    So the window between "the marker is gone" and "`settings.json` names
//!    the account" is a window in which the whole vault sits in
//!    `accounts/<id>/` with nothing pointing at it, and one failed settings
//!    write is enough to make that window permanent. [`Observed`] therefore
//!    carries [`account_dirs_with_profile`](Observed::account_dirs_with_profile)
//!    and [`resume_action`] answers
//!    [`AdoptUnclaimedAccount`](ResumeAction::AdoptUnclaimedAccount) —
//!    recovering the state however it arose rather than narrowing the window
//!    that produces it.
//!
//! ## Which directory is the source
//!
//! Not `bw_command_in(None)`. That form sets no `BITWARDENCLI_APPDATA_DIR` on
//! the child, which is the only spelling the CLI reads as "use your default" —
//! but it does **not** `env_remove` the variable, so a child *inherits* one the
//! user's own environment already set. "The CLI's default directory" is
//! therefore not a thing this process can ask for and be sure of, and
//! migrating from — or verifying against — an inherited directory is precisely
//! the class of mistake rule 3 exists to catch.
//!
//! So the source is *named*, by [`migration_source_from`], which reproduces the
//! CLI's own resolution order for the two cases that can reach us, and every
//! `bw status` this module runs is `Some(dir)`. See
//! `the_source_is_named_explicitly_and_never_left_to_an_inherited_variable`.
//!
//! ## Layout
//!
//! ```text
//! <config_dir>\accounts\<id>.migrating.json     the marker
//! <config_dir>\accounts\<id>.incoming\          the staging copy
//! <config_dir>\accounts\<id>\                   the finished account
//! ```
//!
//! The first two can never collide with an account directory: an
//! [`AccountId`] is 32 lowercase hex characters, so no id contains a `.`.
//! Asserted in `the_marker_and_staging_names_can_never_be_an_account_directory`.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::accounts::{self, Account, AccountId};
use crate::bw_path::{self, MultiAccountAvailability, BW_DATA_DIR_ENV};
use crate::login_ui::{BwStatus, BwStatusDetails};

/// The suffix that turns an account id into its marker file name.
const MARKER_SUFFIX: &str = ".migrating.json";

/// The suffix that turns an account id into its staging directory name.
const INCOMING_SUFFIX: &str = ".incoming";

/// The profile file whose presence means "there is something here to migrate".
const PROFILE_FILE: &str = "data.json";

/// How far a single [`migrate`] call may reassess before giving up.
///
/// Every non-terminal [`ResumeAction`] deletes a file or a directory, so the
/// loop cannot legitimately repeat a state; the bound only exists so a future
/// action that forgot to make progress hangs a startup for microseconds
/// instead of forever.
const MAX_REASSESSMENTS: usize = 8;

// ---------------------------------------------------------------- the marker

/// How far a migration got before it was interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The marker is on disk and a copy into `<id>.incoming` may be part done.
    /// Nothing outside `<id>.incoming` has been touched.
    Copying,
    /// The staging copy is complete and its promotion to `<id>` has been
    /// *ordered*: the marker is written and flushed **before** the rename, not
    /// after.
    ///
    /// That ordering is deliberate. Renaming first and recording it second
    /// leaves a window whose observable state is "marker says `Copying`, no
    /// `.incoming`, `<id>` exists" — which resumes by clearing the marker and
    /// starting again under a *fresh* id, stranding the promoted directory
    /// forever. Recording first leaves "marker says `Promoted`, `.incoming`
    /// exists, `<id>` does not", which resumes by discarding the staging copy
    /// and converging cleanly. Either way the source is whole and still
    /// authoritative, and verification has not passed yet.
    Promoted,
}

/// The on-disk record of a migration in flight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub id: AccountId,
    /// The directory being copied FROM, recorded rather than re-derived: a
    /// resume must delete exactly what this run was reading, even if
    /// `%APPDATA%` (or an inherited `BITWARDENCLI_APPDATA_DIR`) has changed
    /// underneath us.
    pub source: PathBuf,
    /// What `bw status` reported for `source` before the copy started — the
    /// value verification compares against, so a RESUMED verification asks the
    /// same question the original one would have.
    pub source_email: Option<String>,
    pub stage: Stage,
}

/// `<config_dir>\accounts\<id>.migrating.json`.
pub fn marker_path(config_dir: &Path, id: &AccountId) -> PathBuf {
    accounts::accounts_root(config_dir).join(format!("{id}{MARKER_SUFFIX}"))
}

/// `<config_dir>\accounts\<id>.incoming` — the staging copy.
pub fn incoming_path(config_dir: &Path, id: &AccountId) -> PathBuf {
    accounts::accounts_root(config_dir).join(format!("{id}{INCOMING_SUFFIX}"))
}

/// Writes the marker durably.
///
/// `sync_all` and not merely `write_all`: the whole value of the marker is
/// that it survives a power loss the next instruction, and a buffered write
/// that never reached the platter describes a migration nobody can see.
fn write_marker(config_dir: &Path, marker: &Marker) -> std::io::Result<()> {
    let path = marker_path(config_dir, &marker.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(marker).map_err(std::io::Error::other)?;
    let mut file = std::fs::File::create(&path)?;
    file.write_all(&json)?;
    file.sync_all()?;
    Ok(())
}

/// Why a marker on disk could not be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerReadError {
    /// More than one migration marker. Two ids claim to be mid-migration and
    /// nothing here can say which `<id>` is authoritative.
    Ambiguous(Vec<PathBuf>),
    /// A marker that will not parse, or whose recorded id disagrees with its
    /// own file name. Its stage is unknown, so whether `<id>` has been promoted
    /// is unknown, so deleting anything on the strength of it is unsafe.
    Unreadable { path: PathBuf, why: String },
}

/// The single migration marker under `<config_dir>\accounts`, if any.
///
/// Scans rather than deriving a path, because on a resume the id is not known
/// until the marker is read. Entries that are not ours — anything whose stem
/// is not a valid [`AccountId`] — are ignored rather than treated as damage.
pub fn read_marker(config_dir: &Path) -> Result<Option<Marker>, MarkerReadError> {
    let root = accounts::accounts_root(config_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(None); // no accounts directory yet: nothing is in flight
    };

    let mut found: Vec<(AccountId, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(MARKER_SUFFIX) else {
            continue;
        };
        let Some(id) = AccountId::parse(stem) else {
            continue; // not a marker this app wrote
        };
        found.push((id, entry.path()));
    }

    if found.len() > 1 {
        found.sort_by(|a, b| a.1.cmp(&b.1));
        return Err(MarkerReadError::Ambiguous(
            found.into_iter().map(|(_, p)| p).collect(),
        ));
    }
    let Some((id, path)) = found.pop() else {
        return Ok(None);
    };

    let text = std::fs::read_to_string(&path).map_err(|e| MarkerReadError::Unreadable {
        path: path.clone(),
        why: e.to_string(),
    })?;
    let marker: Marker =
        serde_json::from_str(&text).map_err(|e| MarkerReadError::Unreadable {
            path: path.clone(),
            why: e.to_string(),
        })?;
    if marker.id != id {
        return Err(MarkerReadError::Unreadable {
            path,
            why: format!(
                "the marker names account {} but its file name names {id}; which directory it \
                 describes cannot be established",
                marker.id
            ),
        });
    }
    Ok(Some(marker))
}

// ------------------------------------------------------- the state machine

/// Everything the next startup can see, gathered once so the decision is pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub marker: Option<Marker>,
    pub incoming_exists: bool,
    pub final_exists: bool,
    pub source_has_data_json: bool,
    pub accounts_already_configured: bool,
    /// Every `<config_dir>\accounts\<id>\` that holds a [`PROFILE_FILE`],
    /// sorted by id — the directories that contain a real, encrypted vault
    /// profile whatever anything else on disk says.
    ///
    /// Without this the state machine has no way to see a vault directory that
    /// nothing references, and "nothing references it" is a *reachable* state:
    /// [`finish`] deletes the source and then the marker, and the account list
    /// is not written until `main` gets the answer back. A power loss, a kill,
    /// or one failed `settings.json` write in that window leaves the whole
    /// vault in `accounts/<id>/` with no marker and no entry — and a
    /// [`resume_action`] that could not see it answered [`DoNothing`], which
    /// made the next startup mint a fresh id beside it and present as never
    /// signed in. See `a_completed_migration_whose_account_list_was_never_
    /// written_is_adopted_not_reminted`.
    ///
    /// [`DoNothing`]: ResumeAction::DoNothing
    pub account_dirs_with_profile: Vec<AccountId>,
    pub multi_account_available: bool,
    pub backend_port_in_use: bool,
}

/// The ids in [`Observed::account_dirs_with_profile`], read off the disk.
///
/// `<id>.incoming` and `<id>.migrating.json` cannot appear here: an
/// [`AccountId`] is 32 lowercase hex characters, so neither name parses.
fn account_dirs_with_profile(config_dir: &Path) -> Vec<AccountId> {
    let root = accounts::accounts_root(config_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new(); // no accounts directory yet: nothing is claimed
    };
    let mut found: Vec<AccountId> = entries
        .flatten()
        .filter_map(|entry| {
            let id = AccountId::parse(&entry.file_name().to_string_lossy())?;
            accounts::data_dir_for(config_dir, &id)
                .join(PROFILE_FILE)
                .is_file()
                .then_some(id)
        })
        .collect();
    found.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    found
}

/// What to do about what was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAction {
    /// Migration already happened, or there is nothing to migrate.
    DoNothing,
    /// No marker, no account list, a source profile is present: begin.
    StartFresh,
    /// Delete `<id>.incoming` and reassess. A staging copy is untrustworthy
    /// whatever stage the marker claims — at `Copying` it may be half written,
    /// and at `Promoted` it is a directory we cannot account for. The source is
    /// untouched either way.
    DiscardPartialCopyAndRetry,
    /// A marker at `Promoted` with `<id>` present. Re-run verification and, on
    /// success, finish (move the session token, delete the source, delete the
    /// marker).
    VerifyAndFinish,
    /// A marker that describes nothing on disk. Clear it and reassess.
    ClearStaleMarkerAndReassess,
    /// No marker, no account list, and `accounts/<id>/` holds a real profile:
    /// **adopt that id** rather than minting a new one beside it.
    ///
    /// This is a migration that finished — copied, verified, source deleted,
    /// marker deleted — and whose account list never reached `settings.json`.
    /// Nothing here deletes or copies anything, so unlike every other action
    /// that touches the user's data it needs no verification gate: `data.json`
    /// being there is the whole of the evidence, and the alternative is
    /// minting a fresh id whose empty directory presents as "never signed in"
    /// while the real vault sits beside it under a name nothing uses.
    AdoptUnclaimedAccount(AccountId),
    /// Refuse to migrate on this launch and run as a single-account app.
    Refuse { reason: String },
}

/// The reason [`resume_action`] gives when the `relativeDataDir` trap is
/// present. Replaced by [`MultiAccountAvailability::explanation`] before it
/// reaches the user; kept here so the pure function is self-explaining.
const REFUSE_UNAVAILABLE: &str =
    "multiple accounts are unavailable on this machine, so the profile directory Deskwarden \
     would verify against is not the one the CLI would read";

/// The reason [`resume_action`] gives when something still holds the backend
/// port.
const REFUSE_PORT: &str =
    "a Bitwarden backend is still listening on the local port, so it is reading and writing the \
     profile that would be copied; the copy could capture a torn data.json";

/// The reason [`resume_action`] gives when there is no account list and *more
/// than one* account directory holds a profile.
///
/// One of them can be adopted; two cannot, because nothing on disk says which
/// is the account this app was on, and guessing would present one real vault
/// and hide the other. Refusing deletes nothing and names both in the log.
const REFUSE_AMBIGUOUS_UNCLAIMED: &str =
    "more than one account directory holds a Bitwarden profile and Deskwarden's account list is \
     empty, so which of them this app was signed in to cannot be established";

/// The whole decision, as a total function over what was observed.
///
/// `Refuse` outranks every other state, **including a marker at `Promoted`**.
/// With the `relativeDataDir` trap present the CLI ignores
/// `BITWARDENCLI_APPDATA_DIR` entirely, so a verification run against `<id>`
/// would really be reading the *portable* profile: it could pass while proving
/// nothing, and the source would then be deleted on the strength of it.
/// Refusing leaves the marker and both directories exactly as they are, to be
/// resumed on a launch where the trap is gone.
pub fn resume_action(observed: &Observed) -> ResumeAction {
    if !observed.multi_account_available {
        return ResumeAction::Refuse {
            reason: REFUSE_UNAVAILABLE.to_string(),
        };
    }
    if observed.backend_port_in_use {
        return ResumeAction::Refuse {
            reason: REFUSE_PORT.to_string(),
        };
    }

    match observed.marker.as_ref().map(|m| m.stage) {
        // No migration in flight. An account list means it already happened on
        // an earlier launch; no source profile means there is nothing to move.
        //
        // With an account list, every directory is already spoken for and
        // this is a launch after the migration. Without one, a directory
        // holding a profile is a vault nothing references, and adopting it
        // outranks `StartFresh`: the pair "unclaimed directory AND a source
        // still present" is a `finish` whose source deletion failed and whose
        // marker removal then succeeded, so migrating again would put a second
        // copy of the same vault under a second id.
        None => {
            if observed.accounts_already_configured {
                ResumeAction::DoNothing
            } else {
                match observed.account_dirs_with_profile.as_slice() {
                    [] if observed.source_has_data_json => ResumeAction::StartFresh,
                    [] => ResumeAction::DoNothing,
                    [only] => ResumeAction::AdoptUnclaimedAccount(only.clone()),
                    _ => ResumeAction::Refuse {
                        reason: REFUSE_AMBIGUOUS_UNCLAIMED.to_string(),
                    },
                }
            }
        }

        // Nothing outside `<id>.incoming` was touched, so the staging copy is
        // the only thing to clean up.
        Some(Stage::Copying) => {
            if observed.incoming_exists {
                ResumeAction::DiscardPartialCopyAndRetry
            } else {
                ResumeAction::ClearStaleMarkerAndReassess
            }
        }

        // `<id>` may be the promoted copy. It is NOT yet trusted: verification
        // has not passed, and the source is still authoritative.
        //
        // `<id>` present with no source is still `VerifyAndFinish`, and it is
        // idempotent: verification runs against `<id>`, the source deletion is
        // a no-op, the marker is removed. That is exactly the state a crash
        // between "delete source" and "delete marker" produces, and reading it
        // as an error would restart a migration that had already succeeded.
        Some(Stage::Promoted) => {
            if observed.incoming_exists {
                ResumeAction::DiscardPartialCopyAndRetry
            } else if observed.final_exists {
                ResumeAction::VerifyAndFinish
            } else {
                ResumeAction::ClearStaleMarkerAndReassess
            }
        }
    }
}

// ------------------------------------------------------------- the source

/// Which directory the pre-existing profile actually lives in.
///
/// Reproduces the CLI's own resolution order for the two cases that can reach
/// here — `BITWARDENCLI_APPDATA_DIR` if it is set and non-empty, otherwise
/// `%APPDATA%\Bitwarden CLI`. (`relativeDataDir` outranks both, but its
/// presence is a `Refuse`, so migration never has to model it.)
///
/// Empty is treated as unset because the CLI's own check is
/// `else if (process.env.BITWARDENCLI_APPDATA_DIR)`, and the empty string is
/// falsy in JavaScript.
///
/// This exists because `bw_command_in(None)` does not `env_remove`: a child
/// inherits whatever the user's environment already set, so "wherever the CLI
/// defaults to" is not something this process can ask for and be certain of.
/// Naming the directory makes the source read deterministic — and makes it the
/// same directory the copy is taken from, which is the only way the
/// before/after email comparison means anything.
pub fn migration_source_from(
    env_value: Option<&OsStr>,
    appdata_default: Option<PathBuf>,
) -> Option<PathBuf> {
    match env_value {
        Some(raw) if !raw.is_empty() => Some(PathBuf::from(raw)),
        _ => appdata_default,
    }
}

/// [`migration_source_from`] against this process's real environment.
pub fn migration_source() -> Option<PathBuf> {
    migration_source_from(
        std::env::var_os(BW_DATA_DIR_ENV).as_deref(),
        bw_path::cli_default_data_dir(),
    )
}

// --------------------------------------------------------------- the copy

/// Recursive copy. `fs::copy` per file, `create_dir_all` per directory, and
/// **never** a rename or a move of anything under `src`: the source is the
/// user's only copy of an encrypted vault profile until verification passes.
///
/// Reparse points (junctions and symlinks) are an error rather than something
/// to follow. One inside a CLI profile does not happen by itself, and copying
/// *through* it would silently pull in whatever it points at — possibly the
/// account directory being copied into. Refusing is a rollback, and rollback
/// deletes nothing of the user's.
///
/// There is deliberately **no free-space precheck**. `std` exposes no portable
/// free-space query, and a precheck would be the wrong mechanism anyway: a copy
/// that runs out of space returns `Err`, which is a rollback, and rollback at
/// that point deletes only our own staging directory.
/// The reparse-point decision, split from the walk so it can be asserted in
/// both directions without depending on the machine running the test having
/// the privilege to create a link.
///
/// On Windows `FileType::is_symlink` is true for a symbolic link *and* for a
/// junction (a mount-point reparse point), which is the form an unprivileged
/// user can actually create — so both are refused by the one check.
fn refuse_reparse_point(path: &Path, is_link: bool) -> std::io::Result<()> {
    if is_link {
        return Err(std::io::Error::other(format!(
            "{} is a link, not a real file or directory; refusing to copy through it",
            path.display()
        )));
    }
    Ok(())
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<u64> {
    std::fs::create_dir_all(dst)?;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        refuse_reparse_point(&path, meta.file_type().is_symlink())?;
        if meta.is_dir() {
            bytes += copy_dir_all(&path, &dst.join(entry.file_name()))?;
        } else {
            bytes += std::fs::copy(&path, dst.join(entry.file_name()))?;
        }
    }
    Ok(bytes)
}

// ------------------------------------------------------- the verification

/// Whether the CLI recognised the *same* account in the new directory.
///
/// The concrete meaning of "verified", and the gate on every deletion of the
/// user's data in this module. A file count, a byte total, or "`data.json`
/// exists" all pass for a truncated copy.
///
/// A source that reported no email cannot be verified at all: there is no
/// question to ask of the copy, so there is no check that could ever justify
/// deleting the original.
pub fn verification_passed(details: &BwStatusDetails, source_email: &Option<String>) -> bool {
    let recognised = matches!(details.status, BwStatus::Locked | BwStatus::Unlocked);
    match (&details.user_email, source_email) {
        (Some(copied), Some(original)) => recognised && copied == original,
        _ => false,
    }
}

// ------------------------------------------------------------ the outcome

/// What a migration attempt left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationState {
    /// A first install, or a migration that already happened: no profile to
    /// migrate.
    NothingToMigrate,
    /// Done, on this launch or an earlier one.
    Completed {
        account: Account,
        hello_needs_reenrolment: bool,
    },
    /// Not done, and the pre-existing profile is intact and untouched. The app
    /// runs exactly as it does today, against the CLI's own directory.
    Blocked { reason: String },
}

/// The points in [`migrate`]'s body whose *order* is the safety property.
///
/// Two orderings carry rules 2 and 3, and neither leaves a trace in the end
/// state of a successful run — so only an ordering assertion can catch either
/// being inverted:
///
/// * a crash after the first byte is copied and before the marker is durable
///   leaves an UNLABELLED partial copy that [`resume_action`] cannot see;
/// * deleting the source before verification is the data-loss bug this whole
///   module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// The marker reached the platter (`sync_all` returned), not merely a
    /// buffer.
    MarkerFlushed,
    CopyStarted,
    CopyFinished,
    /// `<id>.incoming` was renamed to `<id>`.
    Promoted,
    /// The CLI answered for the new directory with the source's own email.
    Verified,
    /// An account directory nothing referenced was taken over rather than
    /// left orphaned beside a freshly minted id.
    Adopted,
    SessionMoved,
    HelloDeleted,
    SourceDeleted,
    MarkerRemoved,
}

/// The whole effectful migration, with its impure dependencies injected so the
/// tests drive the real composition without a Bitwarden CLI.
///
/// `source` is a parameter rather than a call to [`migration_source`] inside
/// the body for two reasons, and the second one is not negotiable: a resumed
/// migration must read the directory its *marker* recorded, and a test must be
/// able to run the whole composition without the real
/// `%APPDATA%\Bitwarden CLI` — the one directory this code must never be
/// pointed at by accident. The live caller passes
/// `migration_source().as_deref()`.
///
/// `status`'s live caller is `|dir| login_ui::check_bw_status_details_in(dir)`
/// (Task 7). It is always called with `Some(dir)`; see the module docs.
#[allow(clippy::too_many_arguments)]
pub fn migrate(
    config_dir: &Path,
    source: Option<&Path>,
    availability: &MultiAccountAvailability,
    accounts_already_configured: bool,
    status: impl Fn(Option<&Path>) -> BwStatusDetails,
    port_in_use: impl Fn() -> bool,
) -> MigrationState {
    migrate_with_probe(
        config_dir,
        source,
        availability,
        accounts_already_configured,
        status,
        port_in_use,
        &mut |_| {},
    )
}

/// [`migrate`]'s body with an event sink, so the orderings above can be
/// asserted. `migrate` is this with a no-op sink; there is no second copy of
/// the logic.
#[allow(clippy::too_many_arguments)]
pub fn migrate_with_probe(
    config_dir: &Path,
    source: Option<&Path>,
    availability: &MultiAccountAvailability,
    accounts_already_configured: bool,
    status: impl Fn(Option<&Path>) -> BwStatusDetails,
    port_in_use: impl Fn() -> bool,
    probe: &mut dyn FnMut(Probe),
) -> MigrationState {
    for _ in 0..MAX_REASSESSMENTS {
        let marker = match read_marker(config_dir) {
            Ok(marker) => marker,
            Err(e) => {
                // Neither deletes nor migrates: the source is whole, and a
                // marker whose stage cannot be read leaves "has `<id>` been
                // promoted?" unanswerable.
                let reason = match e {
                    MarkerReadError::Ambiguous(paths) => format!(
                        "two migrations are recorded as in flight ({}); Deskwarden will not \
                         guess which account directory is authoritative",
                        paths
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    MarkerReadError::Unreadable { path, why } => format!(
                        "the migration marker {} cannot be read ({why}), so how far the last \
                         attempt got is unknown",
                        path.display()
                    ),
                };
                log::warn!("migration blocked: {reason}");
                return MigrationState::Blocked { reason };
            }
        };

        // On a resume the marker's recorded source is authoritative: this run
        // must delete exactly what the interrupted one was reading.
        let effective_source: Option<PathBuf> = marker
            .as_ref()
            .map(|m| m.source.clone())
            .or_else(|| source.map(Path::to_path_buf));

        let observed = Observed {
            incoming_exists: marker
                .as_ref()
                .is_some_and(|m| incoming_path(config_dir, &m.id).exists()),
            final_exists: marker
                .as_ref()
                .is_some_and(|m| accounts::data_dir_for(config_dir, &m.id).exists()),
            source_has_data_json: effective_source
                .as_ref()
                .is_some_and(|s| s.join(PROFILE_FILE).is_file()),
            marker,
            accounts_already_configured,
            account_dirs_with_profile: account_dirs_with_profile(config_dir),
            multi_account_available: availability.is_available(),
            backend_port_in_use: port_in_use(),
        };

        match resume_action(&observed) {
            ResumeAction::Refuse { reason } => {
                // The availability explanation names the directory the user has
                // to go and remove, which the pure decision cannot know.
                let reason = availability.explanation().unwrap_or(reason);
                log::warn!("not migrating the Bitwarden profile: {reason}");
                return MigrationState::Blocked { reason };
            }

            ResumeAction::DoNothing => return MigrationState::NothingToMigrate,

            ResumeAction::StartFresh => {
                let Some(source) = effective_source else {
                    // Unreachable: `StartFresh` requires `source_has_data_json`,
                    // which requires a source. Stated rather than unwrapped.
                    return MigrationState::Blocked {
                        reason: "no source profile directory to migrate from".to_string(),
                    };
                };
                return start_fresh(config_dir, &source, &status, probe);
            }

            ResumeAction::AdoptUnclaimedAccount(id) => {
                // Deletes nothing, copies nothing, renames nothing. `status`
                // is asked only for the LABEL, and a CLI that cannot answer
                // costs a blank address in the switcher -- never the adoption
                // itself, because refusing to adopt is precisely how the
                // directory stays orphaned.
                let dir = accounts::data_dir_for(config_dir, &id);
                log::warn!(
                    "adopting {}: it holds a Bitwarden profile and nothing in settings.json \
                     names it, which is what a migration that finished before its account list \
                     was written leaves behind",
                    dir.display()
                );
                let details = status(Some(&dir));
                probe(Probe::Adopted);
                return MigrationState::Completed {
                    account: Account {
                        id,
                        email: details.user_email.unwrap_or_default(),
                        server_url: details.server_url,
                    },
                    // The blob this launch could have re-enrolled was deleted
                    // by the run that migrated; there is nothing to warn about
                    // a second time.
                    hello_needs_reenrolment: false,
                };
            }

            ResumeAction::VerifyAndFinish => {
                let marker = observed
                    .marker
                    .expect("VerifyAndFinish is only reachable with a marker");
                return finish(config_dir, &marker, &status, probe);
            }

            ResumeAction::DiscardPartialCopyAndRetry => {
                let marker = observed
                    .marker
                    .expect("DiscardPartialCopyAndRetry is only reachable with a marker");
                let incoming = incoming_path(config_dir, &marker.id);
                log::warn!(
                    "discarding an interrupted migration's staging copy at {}",
                    incoming.display()
                );
                if let Err(e) = std::fs::remove_dir_all(&incoming) {
                    return MigrationState::Blocked {
                        reason: format!(
                            "an interrupted migration left {} behind and it cannot be removed \
                             ({e}); Deskwarden will not copy over it",
                            incoming.display()
                        ),
                    };
                }
                continue;
            }

            ResumeAction::ClearStaleMarkerAndReassess => {
                let marker = observed
                    .marker
                    .expect("ClearStaleMarkerAndReassess is only reachable with a marker");
                let path = marker_path(config_dir, &marker.id);
                if observed.final_exists {
                    // Only reachable for a `Copying` marker, which never
                    // promotes anything -- so this directory cannot be
                    // explained. It is left alone rather than deleted: nothing
                    // here knows it is ours.
                    log::warn!(
                        "an unexplained account directory {} outlived a migration that never \
                         promoted anything; leaving it alone",
                        accounts::data_dir_for(config_dir, &marker.id).display()
                    );
                }
                log::warn!("clearing a stale migration marker at {}", path.display());
                if let Err(e) = std::fs::remove_file(&path) {
                    return MigrationState::Blocked {
                        reason: format!(
                            "a stale migration marker at {} cannot be removed ({e})",
                            path.display()
                        ),
                    };
                }
                continue;
            }
        }
    }

    MigrationState::Blocked {
        reason: "the migration state on disk did not settle; nothing was copied or deleted"
            .to_string(),
    }
}

/// Rule 4, in one place: delete only what *we* made, and clear the marker.
///
/// Never touches the source. It cannot need to — nothing of the user's has
/// been deleted at any point this is reachable from.
fn rollback(config_dir: &Path, id: &AccountId, why: &str) -> MigrationState {
    log::error!("rolling back the Bitwarden profile migration: {why}");
    let _ = std::fs::remove_dir_all(incoming_path(config_dir, id));
    let _ = std::fs::remove_dir_all(accounts::data_dir_for(config_dir, id));
    let _ = std::fs::remove_file(marker_path(config_dir, id));
    MigrationState::Blocked {
        reason: why.to_string(),
    }
}

/// Copy, promote, verify, finish — the first-run path.
fn start_fresh(
    config_dir: &Path,
    source: &Path,
    status: &impl Fn(Option<&Path>) -> BwStatusDetails,
    probe: &mut dyn FnMut(Probe),
) -> MigrationState {
    let id = AccountId::generate();

    // Whose profile this is, asked of the SOURCE and asked before anything is
    // copied. Without it there is no question to ask of the copy, so there is
    // no check that could ever justify deleting the original.
    let source_details = status(Some(source));
    let source_email = source_details.user_email.clone();
    if source_email.is_none() {
        return MigrationState::Blocked {
            reason: format!(
                "the Bitwarden CLI did not say which account {} holds, so a copy of it could \
                 never be verified and the original could never be safely removed",
                source.display()
            ),
        };
    }

    let marker = Marker {
        id: id.clone(),
        source: source.to_path_buf(),
        source_email,
        stage: Stage::Copying,
    };
    if let Err(e) = write_marker(config_dir, &marker) {
        return MigrationState::Blocked {
            reason: format!(
                "the migration marker could not be written ({e}); an unlabelled copy is worse \
                 than no copy, so nothing was copied"
            ),
        };
    }
    probe(Probe::MarkerFlushed);

    let incoming = incoming_path(config_dir, &id);
    probe(Probe::CopyStarted);
    if let Err(e) = copy_dir_all(source, &incoming) {
        return rollback(
            config_dir,
            &id,
            &format!(
                "copying {} to {} failed: {e}",
                source.display(),
                incoming.display()
            ),
        );
    }
    probe(Probe::CopyFinished);

    // Recorded before the rename, not after: see `Stage::Promoted`.
    let promoted = Marker {
        stage: Stage::Promoted,
        ..marker
    };
    if let Err(e) = write_marker(config_dir, &promoted) {
        return rollback(
            config_dir,
            &id,
            &format!("the migration marker could not be advanced ({e})"),
        );
    }

    let final_dir = accounts::data_dir_for(config_dir, &id);
    if let Err(e) = std::fs::rename(&incoming, &final_dir) {
        return rollback(
            config_dir,
            &id,
            &format!(
                "promoting {} to {} failed: {e}",
                incoming.display(),
                final_dir.display()
            ),
        );
    }
    probe(Probe::Promoted);

    finish(config_dir, &promoted, status, probe)
}

/// Verify `<id>`, and only then remove anything of the user's.
///
/// Reached both from [`start_fresh`] and from a resume, so a migration
/// interrupted after promotion asks exactly the question the original run would
/// have — the marker carries the source's email precisely so it can.
fn finish(
    config_dir: &Path,
    marker: &Marker,
    status: &impl Fn(Option<&Path>) -> BwStatusDetails,
    probe: &mut dyn FnMut(Probe),
) -> MigrationState {
    let final_dir = accounts::data_dir_for(config_dir, &marker.id);
    let details = status(Some(&final_dir));

    if !verification_passed(&details, &marker.source_email) {
        return rollback(
            config_dir,
            &marker.id,
            &format!(
                "the Bitwarden CLI did not recognise the copied profile in {} as {} (it reported \
                 {:?}/{:?}); the original profile has been left exactly as it was",
                final_dir.display(),
                marker.source_email.as_deref().unwrap_or("<unknown>"),
                details.status,
                details.user_email
            ),
        );
    }
    probe(Probe::Verified);

    // Everything past this point is deleting or moving the user's data, and
    // every one of it is downstream of the assertion above.

    // The session token: `bw` is still unlocked under it, and not carrying it
    // over would demand a master password on the very launch that migrated.
    let old_session = config_dir.join("session.bin");
    if old_session.is_file() {
        let new_session = accounts::session_path_for(config_dir, &marker.id);
        match std::fs::copy(&old_session, &new_session) {
            Ok(_) => {
                if let Err(e) = std::fs::remove_file(&old_session) {
                    log::warn!(
                        "the migrated session token was copied but {} could not be removed: {e}",
                        old_session.display()
                    );
                }
                probe(Probe::SessionMoved);
            }
            Err(e) => log::warn!(
                "the session token could not be carried into the migrated account ({e}); the \
                 master password will be asked for again"
            ),
        }
    }

    // The Hello blob is DELETED, never copied: every account's KDF suffix is
    // non-empty now (`accounts::hello_kdf_suffix_for`), so this blob can never
    // be opened again by anyone. Leaving it would be a file holding a sealed
    // master password that nothing can ever use and nothing will ever clean up.
    let old_hello = config_dir.join("hello.bin");
    let hello_needs_reenrolment = old_hello.is_file();
    if hello_needs_reenrolment {
        match std::fs::remove_file(&old_hello) {
            Ok(()) => probe(Probe::HelloDeleted),
            Err(e) => log::warn!(
                "the pre-migration Windows Hello blob at {} could not be removed ({e}); it can \
                 no longer be opened by anything, but it is still on disk",
                old_hello.display()
            ),
        }
        log::warn!(
            "Windows Hello quick unlock has to be set up again for the migrated account: the \
             sealed key is derived per account and the pre-migration blob cannot be reused"
        );
    }

    // The source, last. A missing one is a no-op, which is what makes a crash
    // between this and the marker removal resumable rather than fatal.
    if marker.source.exists() {
        if accounts::accounts_root(config_dir).starts_with(&marker.source) {
            // Refuses to delete a directory that contains the accounts tree.
            // Unreachable with a real CLI profile as the source, and cheap
            // insurance against a hand-edited marker.
            log::error!(
                "refusing to remove the migration source {}: the accounts directory lives \
                 inside it",
                marker.source.display()
            );
        } else if let Err(e) = std::fs::remove_dir_all(&marker.source) {
            log::warn!(
                "the migrated profile at {} could not be removed ({e}); it is no longer used",
                marker.source.display()
            );
        }
    }
    probe(Probe::SourceDeleted);

    // The marker goes LAST. If it went first, a crash here would leave a
    // finished migration looking like one that never started.
    let path = marker_path(config_dir, &marker.id);
    if let Err(e) = std::fs::remove_file(&path) {
        log::warn!(
            "the migration marker at {} could not be removed ({e}); the next launch will \
             re-verify and clear it",
            path.display()
        );
    }
    probe(Probe::MarkerRemoved);

    let email = details
        .user_email
        .or_else(|| marker.source_email.clone())
        .unwrap_or_default();
    MigrationState::Completed {
        account: Account {
            id: marker.id.clone(),
            email,
            server_url: details.server_url,
        },
        hello_needs_reenrolment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";

    fn id(s: &str) -> AccountId {
        AccountId::parse(s).unwrap_or_else(|| panic!("{s:?} should be a valid id"))
    }

    /// A unique scratch directory. Same `temp_dir()` + nanos pattern the rest
    /// of the crate's tests use (no `tempfile` dev-dependency here).
    ///
    /// **Every path any test in this module touches is under one of these.**
    /// Nothing here may reach the real `%APPDATA%\Bitwarden CLI`; a test that
    /// deleted a real profile is the exact failure this module exists to
    /// prevent.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-migration-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn details(status: BwStatus, email: Option<&str>) -> BwStatusDetails {
        BwStatusDetails {
            status,
            user_email: email.map(str::to_string),
            server_url: None,
        }
    }

    fn locked_as(email: &str) -> BwStatusDetails {
        details(BwStatus::Locked, Some(email))
    }

    fn unauthenticated() -> BwStatusDetails {
        details(BwStatus::Unauthenticated, None)
    }

    fn available() -> MultiAccountAvailability {
        MultiAccountAvailability::Available
    }

    fn observed(marker: Option<Stage>, incoming: bool, fin: bool, src: bool) -> Observed {
        Observed {
            marker: marker.map(|stage| Marker {
                id: id(A),
                source: PathBuf::from(r"C:\Users\me\AppData\Roaming\Bitwarden CLI"),
                source_email: Some("me@example.com".into()),
                stage,
            }),
            incoming_exists: incoming,
            final_exists: fin,
            source_has_data_json: src,
            accounts_already_configured: false,
            account_dirs_with_profile: Vec::new(),
            multi_account_available: true,
            backend_port_in_use: false,
        }
    }

    /// A config directory and a planted "pre-existing CLI profile", both under
    /// one scratch root. Returns `(config_dir, source)`.
    fn planted_profile(tag: &str) -> (PathBuf, PathBuf) {
        let root = scratch_dir(tag);
        let cfg = root.join("cfg");
        let source = root.join("Bitwarden CLI");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join(PROFILE_FILE), b"{\"profile\":1}").unwrap();
        std::fs::write(source.join("nested").join("blob.bin"), b"\x00\x01\x02").unwrap();
        (cfg, source)
    }

    fn dir_entries(dir: PathBuf) -> Vec<String> {
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = read
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // ---------------------------------------------------------------- 4.1

    #[test]
    fn the_resume_table_is_exactly_this() {
        // Absolute expectations for every reachable observation, so this
        // passes for exactly one implementation. A `_ => DoNothing` catch-all
        // is the mutation this is written to reject: it silently converts
        // every half-migrated state into "pretend it is fine", which is
        // precisely the outcome that must not be reachable.
        use ResumeAction::*;

        assert_eq!(resume_action(&observed(None, false, false, false)), DoNothing);
        assert_eq!(resume_action(&observed(None, false, false, true)), StartFresh);
        assert_eq!(
            resume_action(&Observed {
                accounts_already_configured: true,
                ..observed(None, false, false, true)
            }),
            DoNothing,
            "an account list already exists; migration happened on an earlier launch"
        );

        assert_eq!(
            resume_action(&observed(Some(Stage::Copying), true, false, true)),
            DiscardPartialCopyAndRetry
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Copying), true, true, true)),
            DiscardPartialCopyAndRetry
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Copying), false, false, true)),
            ClearStaleMarkerAndReassess
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Copying), false, true, true)),
            ClearStaleMarkerAndReassess,
            "a Copying marker never promoted anything, so `<id>` is not ours to trust"
        );

        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), false, true, true)),
            VerifyAndFinish
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), false, true, false)),
            VerifyAndFinish,
            "a crash between deleting the source and deleting the marker must FINISH, not \
             restart -- restarting would find no source and give up on a migration that had \
             already succeeded"
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), false, false, true)),
            ClearStaleMarkerAndReassess
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), true, true, true)),
            DiscardPartialCopyAndRetry,
            "an `.incoming` that outlived promotion is a directory we cannot account for"
        );
        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), true, false, true)),
            DiscardPartialCopyAndRetry
        );

        // Every remaining combination is covered too, so "total function" is
        // asserted rather than asserted-about-ten-rows. The point is that no
        // observation falls through to a default.
        for stage in [None, Some(Stage::Copying), Some(Stage::Promoted)] {
            for incoming in [false, true] {
                for fin in [false, true] {
                    for src in [false, true] {
                        let action = resume_action(&observed(stage, incoming, fin, src));
                        assert!(
                            !matches!(action, Refuse { .. }),
                            "{stage:?}/{incoming}/{fin}/{src} refused with nothing to refuse over"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_marker_free_observation_ignores_the_staging_directories_entirely() {
        // Without this, an implementation that keyed off `<id>.incoming`
        // existing rather than off the marker would pass the table above --
        // and would then "resume" a migration no marker describes.
        use ResumeAction::*;
        assert_eq!(resume_action(&observed(None, true, true, true)), StartFresh);
        assert_eq!(resume_action(&observed(None, true, true, false)), DoNothing);
    }

    #[test]
    fn a_blocked_availability_refuses_even_a_promoted_migration() {
        // With `relativeDataDir` present the CLI IGNORES our env var, so a
        // verification run against the new directory would really be reading
        // the portable profile: it could pass while proving nothing, and the
        // source would then be deleted on the strength of it.
        let o = Observed {
            multi_account_available: false,
            ..observed(Some(Stage::Promoted), false, true, true)
        };
        assert!(
            matches!(resume_action(&o), ResumeAction::Refuse { .. }),
            "got {:?}",
            resume_action(&o)
        );
        // Positive control: the same observation migrates when the trap is
        // absent.
        assert_eq!(
            resume_action(&observed(Some(Stage::Promoted), false, true, true)),
            ResumeAction::VerifyAndFinish
        );
        // And it outranks every other state, not just this one.
        for stage in [None, Some(Stage::Copying), Some(Stage::Promoted)] {
            for incoming in [false, true] {
                for fin in [false, true] {
                    let o = Observed {
                        multi_account_available: false,
                        ..observed(stage, incoming, fin, true)
                    };
                    assert!(
                        matches!(resume_action(&o), ResumeAction::Refuse { .. }),
                        "{stage:?}/{incoming}/{fin} did not refuse"
                    );
                }
            }
        }
    }

    #[test]
    fn a_backend_still_holding_the_port_refuses_rather_than_copying_a_live_profile() {
        // An orphaned `bw serve` from an unclean exit is reading and writing
        // the source profile right now. Copying it would capture a torn
        // `data.json`, and verification might well pass on it.
        let o = Observed {
            backend_port_in_use: true,
            ..observed(None, false, false, true)
        };
        assert!(matches!(resume_action(&o), ResumeAction::Refuse { .. }));
        assert_eq!(
            resume_action(&observed(None, false, false, true)),
            ResumeAction::StartFresh
        );
    }

    #[test]
    fn each_refusal_says_which_of_the_two_reasons_it_is() {
        // One `Refuse` reason for both would leave the user (and the log) with
        // no way to tell a directory they must delete from a process they must
        // close.
        let trapped = Observed {
            multi_account_available: false,
            ..observed(None, false, false, true)
        };
        let held = Observed {
            backend_port_in_use: true,
            ..observed(None, false, false, true)
        };
        let (ResumeAction::Refuse { reason: a }, ResumeAction::Refuse { reason: b }) =
            (resume_action(&trapped), resume_action(&held))
        else {
            panic!("both of these must refuse");
        };
        assert_ne!(a, b);
        assert!(a.contains("multiple accounts"), "{a}");
        assert!(b.contains("port"), "{b}");
    }

    // ---------------------------------------------------------------- 4.2

    #[test]
    fn the_marker_and_staging_names_can_never_be_an_account_directory() {
        let cfg = Path::new(r"C:\cfg");
        let a = id(A);
        let (marker, incoming, finished) = (
            marker_path(cfg, &a),
            incoming_path(cfg, &a),
            accounts::data_dir_for(cfg, &a),
        );
        assert_eq!(
            marker,
            PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef.migrating.json")
        );
        assert_eq!(
            incoming,
            PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef.incoming")
        );
        assert_ne!(marker, finished);
        assert_ne!(incoming, finished);
        assert_ne!(marker, incoming);

        // The property that makes it so: an id is 32 hex characters, so it can
        // never contain a `.`, so `<id>.incoming` can never BE an `<id>`.
        assert!(AccountId::parse("0123456789abcdef0123456789abc.in").is_none());
        for name in [&marker, &incoming] {
            let leaf = name.file_name().unwrap().to_string_lossy().into_owned();
            assert!(leaf.contains('.'), "{leaf} could be parsed as an account id");
            assert!(AccountId::parse(&leaf).is_none(), "{leaf} parsed as an id");
        }
        // Positive control on the same parser, so the two `is_none`s above are
        // not vacuous.
        assert!(AccountId::parse(finished.file_name().unwrap().to_str().unwrap()).is_some());
    }

    #[test]
    fn a_marker_round_trips_and_a_foreign_file_in_the_accounts_root_is_ignored() {
        let cfg = scratch_dir("marker-round-trip");
        let a = id(A);
        let marker = Marker {
            id: a.clone(),
            source: PathBuf::from(r"C:\Users\me\AppData\Roaming\Bitwarden CLI"),
            source_email: Some("me@example.com".into()),
            stage: Stage::Copying,
        };
        write_marker(&cfg, &marker).unwrap();
        assert_eq!(read_marker(&cfg), Ok(Some(marker.clone())));

        // Things that are not ours are not damage.
        std::fs::write(
            accounts::accounts_root(&cfg).join("notes.migrating.json"),
            b"{}",
        )
        .unwrap();
        std::fs::create_dir_all(accounts::data_dir_for(&cfg, &a)).unwrap();
        assert_eq!(read_marker(&cfg), Ok(Some(marker)));

        // A second real marker is ambiguous, not "pick one".
        let other = id("fedcba9876543210fedcba9876543210");
        write_marker(
            &cfg,
            &Marker {
                id: other,
                source: PathBuf::from(r"C:\elsewhere"),
                source_email: None,
                stage: Stage::Promoted,
            },
        )
        .unwrap();
        assert!(matches!(
            read_marker(&cfg),
            Err(MarkerReadError::Ambiguous(_))
        ));

        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn a_marker_whose_contents_disagree_with_its_name_is_unreadable_rather_than_believed() {
        // A marker is what says whether `<id>` has been promoted. Believing a
        // file that names a different account would point verification -- and
        // then a `remove_dir_all` -- at the wrong directory.
        let cfg = scratch_dir("marker-mismatch");
        std::fs::create_dir_all(accounts::accounts_root(&cfg)).unwrap();
        let lying = Marker {
            id: id("fedcba9876543210fedcba9876543210"),
            source: PathBuf::from(r"C:\src"),
            source_email: None,
            stage: Stage::Promoted,
        };
        std::fs::write(
            marker_path(&cfg, &id(A)),
            serde_json::to_vec(&lying).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            read_marker(&cfg),
            Err(MarkerReadError::Unreadable { .. })
        ));

        // Positive control: the same file under its own name reads back fine.
        std::fs::remove_file(marker_path(&cfg, &id(A))).unwrap();
        write_marker(&cfg, &lying).unwrap();
        assert_eq!(read_marker(&cfg), Ok(Some(lying)));

        let _ = std::fs::remove_dir_all(&cfg);
    }

    // ---------------------------------------------------------------- 4.3

    #[test]
    fn the_copy_leaves_the_source_completely_intact() {
        // THE single most important assertion in this module. `fs::rename` on
        // the source -- the "obvious" implementation, and one line shorter --
        // passes every other test here and destroys the user's profile the
        // first time it is interrupted.
        let root = scratch_dir("copy-intact");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join(PROFILE_FILE), b"{\"profile\":1}").unwrap();
        std::fs::write(src.join("nested").join("blob.bin"), b"\x00\x01\x02").unwrap();

        let dst = root.join("dst");
        let bytes = copy_dir_all(&src, &dst).unwrap();

        assert_eq!(std::fs::read(src.join(PROFILE_FILE)).unwrap(), b"{\"profile\":1}");
        assert_eq!(
            std::fs::read(src.join("nested").join("blob.bin")).unwrap(),
            b"\x00\x01\x02"
        );
        assert_eq!(std::fs::read(dst.join(PROFILE_FILE)).unwrap(), b"{\"profile\":1}");
        assert_eq!(
            std::fs::read(dst.join("nested").join("blob.bin")).unwrap(),
            b"\x00\x01\x02"
        );
        assert_eq!(bytes, 16, "the byte total is the source's, not a guess");
        assert!(src.exists(), "the source directory itself was moved away");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reparse_point_in_the_source_fails_the_copy_rather_than_being_followed() {
        // A junction or symlink inside a CLI profile is not something that
        // happens by itself. Copying THROUGH it would silently pull in
        // whatever it points at -- possibly the account directory we are
        // copying into. Refusing is a rollback, and rollback deletes nothing
        // of the user's.
        let root = scratch_dir("copy-reparse");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(PROFILE_FILE), b"{}").unwrap();

        // Positive control FIRST, so a machine without the privilege to make a
        // link still proves the copy works at all before it skips.
        assert!(copy_dir_all(&src, &root.join("control")).is_ok());

        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let link = src.join("link");
        assert!(
            plant_reparse_point(&link, &outside),
            "neither a symlink nor a junction could be created at {}, so this machine cannot \
             exercise the guard at all",
            link.display()
        );
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "the planted reparse point is not seen as a link, so the guard below would be \
             untested even though something was planted"
        );

        let err = copy_dir_all(&src, &root.join("dst"))
            .expect_err("a link in the source must fail the copy");
        assert!(err.to_string().contains("refusing to copy through it"), "{err}");
        // ...and the destination never received whatever it pointed at.
        assert!(!root.join("dst").join("link").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Creates a directory reparse point at `link` pointing at `target`.
    ///
    /// A symbolic link first, and a **junction** as the fallback: creating a
    /// symlink needs `SeCreateSymbolicLinkPrivilege` (or Developer Mode) and
    /// routinely fails on an ordinary account, while `mklink /J` needs no
    /// privilege at all. Without the fallback this test silently skipped on
    /// this machine — verified by mutation: deleting the guard in
    /// `copy_dir_all` left the whole suite green.
    fn plant_reparse_point(link: &Path, target: &Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
            .join("cmd.exe");
        std::process::Command::new(shell)
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn the_reparse_decision_answers_both_ways_and_says_which_path_it_refused() {
        // The pure half, so the direction of the check is pinned even on a
        // machine where nothing can be planted: a guard that cannot tell a
        // link from a plain file is one character from a guard that refuses
        // every copy.
        let path = Path::new(r"C:\cfg\accounts\aaaa.incoming\link");
        assert!(refuse_reparse_point(path, false).is_ok());
        let err = refuse_reparse_point(path, true).expect_err("a link must be refused");
        assert!(err.to_string().contains(r"C:\cfg\accounts\aaaa.incoming\link"), "{err}");
        assert!(err.to_string().contains("refusing to copy through it"), "{err}");
    }

    // ---------------------------------------------------------------- 4.4

    #[test]
    fn verification_requires_the_cli_to_recognise_the_same_account_in_the_new_directory() {
        // The concrete meaning of "verified". A file count, a byte total, or
        // "data.json exists" all pass for a truncated copy; `bw` refusing to
        // recognise the account is the failure that has to be caught while the
        // original still exists.
        let src = Some("me@example.com".to_string());
        assert!(verification_passed(
            &details(BwStatus::Locked, Some("me@example.com")),
            &src
        ));
        assert!(verification_passed(
            &details(BwStatus::Unlocked, Some("me@example.com")),
            &src
        ));

        // Every failure the copy can produce:
        assert!(
            !verification_passed(&details(BwStatus::Unauthenticated, None), &src),
            "an empty profile reports Unauthenticated -- the copy landed nothing"
        );
        assert!(
            !verification_passed(&details(BwStatus::Unauthenticated, Some("me@example.com")), &src),
            "the right email under a status that means `bw` cannot open this profile"
        );
        assert!(
            !verification_passed(&details(BwStatus::Locked, Some("someone@else")), &src),
            "a DIFFERENT account answered -- the env var was ignored and this is another profile"
        );
        assert!(
            !verification_passed(&details(BwStatus::Locked, None), &src),
            "the CLI could not say whose profile this is"
        );
    }

    #[test]
    fn a_source_that_reports_no_email_cannot_be_verified_and_is_not_migrated() {
        // If we could not establish whose profile the source is, there is no
        // question to ask of the copy -- so there is no check that could ever
        // justify deleting the original.
        assert!(!verification_passed(
            &details(BwStatus::Locked, Some("me@example.com")),
            &None
        ));
        assert!(!verification_passed(&details(BwStatus::Locked, None), &None));
    }

    // -------------------------------------------------------- the source

    #[test]
    fn the_migration_source_follows_the_variable_the_cli_itself_would_read() {
        // `bw_command_in(None)` does not `env_remove`, so a child INHERITS a
        // `BITWARDENCLI_APPDATA_DIR` the user's own environment already set --
        // and then "the CLI's default directory" is not what
        // `cli_default_data_dir()` computed. Migrating from, or verifying
        // against, the wrong directory is exactly what rule 3 exists to catch,
        // so the source is named rather than defaulted.
        let default = Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\Bitwarden CLI"));
        assert_eq!(
            migration_source_from(None, default.clone()),
            default,
            "with nothing in the environment the CLI uses %APPDATA%"
        );
        assert_eq!(
            migration_source_from(Some(OsStr::new(r"D:\portable\profile")), default.clone()),
            Some(PathBuf::from(r"D:\portable\profile")),
            "an inherited variable IS the directory the CLI reads, so it is the source"
        );
        assert_eq!(
            migration_source_from(Some(OsStr::new("")), default.clone()),
            default,
            "the CLI's own check is falsy for an empty string, so empty means default"
        );
        assert_eq!(
            migration_source_from(None, None),
            None,
            "no %APPDATA% and no variable: there is no source to name"
        );
    }

    #[test]
    fn the_live_migration_source_reads_the_real_variable_and_the_real_appdata() {
        // WIRING. The pure function above can be perfect while
        // `migration_source()` answers from something else entirely.
        assert_eq!(
            migration_source(),
            migration_source_from(
                std::env::var_os(BW_DATA_DIR_ENV).as_deref(),
                bw_path::cli_default_data_dir()
            )
        );
        if std::env::var_os(BW_DATA_DIR_ENV).is_none() && std::env::var_os("APPDATA").is_some() {
            assert_eq!(migration_source(), bw_path::cli_default_data_dir());
            assert!(migration_source().is_some(), "the equality is not two Nones");
        }
    }

    /// Records which directory each `bw status` was asked about.
    fn recording_status<'a>(
        log: &'a std::cell::RefCell<Vec<Option<PathBuf>>>,
        answer: impl Fn(Option<&Path>) -> BwStatusDetails + 'a,
    ) -> impl Fn(Option<&Path>) -> BwStatusDetails + 'a {
        move |dir| {
            log.borrow_mut().push(dir.map(Path::to_path_buf));
            answer(dir)
        }
    }

    #[test]
    fn the_source_is_named_explicitly_and_never_left_to_an_inherited_variable() {
        // The wiring for the gap above, over the real composition: every
        // `bw status` migration runs names its directory. A `status(None)`
        // anywhere here would ask "whatever the child inherits", which is the
        // user's environment and not necessarily the profile being copied --
        // and the before/after email comparison would then be comparing two
        // readings of a directory nobody chose.
        let (cfg, source) = planted_profile("named-source");
        let asked = std::cell::RefCell::new(Vec::new());
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            recording_status(&asked, |_| locked_as("me@example.com")),
            || false,
        );
        let MigrationState::Completed { account, .. } = state else {
            panic!("{state:?}")
        };

        let asked = asked.into_inner();
        assert!(
            asked.iter().all(Option::is_some),
            "a `bw status` was left to an inherited BITWARDENCLI_APPDATA_DIR: {asked:?}"
        );
        assert_eq!(
            asked.first().cloned().flatten(),
            Some(source.clone()),
            "the first question was not asked of the profile actually being copied"
        );
        assert!(
            asked.contains(&Some(accounts::data_dir_for(&cfg, &account.id))),
            "the copy was never asked about: {asked:?}"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    // ---------------------------------------------------------------- 4.5

    #[test]
    fn a_successful_migration_copies_verifies_repoints_and_only_then_deletes() {
        let (cfg, source) = planted_profile("migrate-ok");
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_dir| locked_as("me@example.com"),
            || false,
        );

        let MigrationState::Completed { account, .. } = state else {
            panic!("{state:?}")
        };
        let landed = accounts::data_dir_for(&cfg, &account.id);
        assert!(
            landed.join(PROFILE_FILE).exists(),
            "the profile did not land"
        );
        assert_eq!(
            std::fs::read(landed.join("nested").join("blob.bin")).unwrap(),
            b"\x00\x01\x02",
            "the copy was shallow"
        );
        assert_eq!(account.email, "me@example.com");
        assert!(!source.exists(), "the source was not removed after a verified copy");
        assert!(
            !marker_path(&cfg, &account.id).exists(),
            "the marker outlived the migration"
        );
        assert!(
            !incoming_path(&cfg, &account.id).exists(),
            "the staging directory was left behind"
        );
        assert_eq!(
            dir_entries(accounts::accounts_root(&cfg)),
            vec![account.id.to_string()],
            "the accounts root holds the account and nothing else"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_migration_whose_copy_fails_verification_deletes_nothing_of_the_users() {
        // The whole point of rule 3. `bw` cannot open the copy; the original
        // is the user's only working profile and must still be there.
        let (cfg, source) = planted_profile("migrate-unverified");
        let source_for_match = source.clone();
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            move |dir| {
                if dir == Some(source_for_match.as_path()) {
                    locked_as("me@example.com") // the source
                } else {
                    unauthenticated() // the copy is unusable
                }
            },
            || false,
        );

        assert!(matches!(state, MigrationState::Blocked { .. }), "{state:?}");
        assert!(
            source.join(PROFILE_FILE).exists(),
            "THE USER'S PROFILE WAS DELETED"
        );
        assert_eq!(
            std::fs::read(source.join("nested").join("blob.bin")).unwrap(),
            b"\x00\x01\x02"
        );
        // And nothing half-made is left to be mistaken for an account.
        assert!(
            dir_entries(accounts::accounts_root(&cfg)).is_empty(),
            "left behind: {:?}",
            dir_entries(accounts::accounts_root(&cfg))
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_copy_that_answers_for_a_different_account_is_a_rollback_not_a_success() {
        // The `relativeDataDir`-shaped failure seen from the inside: the env
        // var was ignored and some OTHER profile answered. Verification is the
        // only thing that can tell this from success.
        let (cfg, source) = planted_profile("migrate-wrong-account");
        let source_for_match = source.clone();
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            move |dir| {
                if dir == Some(source_for_match.as_path()) {
                    locked_as("me@example.com")
                } else {
                    locked_as("someone@else.example")
                }
            },
            || false,
        );

        let MigrationState::Blocked { reason } = state else {
            panic!("a different account answered and it was accepted: {state:?}")
        };
        assert!(reason.contains("me@example.com"), "{reason}");
        assert!(source.join(PROFILE_FILE).exists(), "THE USER'S PROFILE WAS DELETED");
        assert!(dir_entries(accounts::accounts_root(&cfg)).is_empty());
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_source_the_cli_cannot_identify_is_never_copied_at_all() {
        let (cfg, source) = planted_profile("migrate-anonymous");
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| details(BwStatus::Locked, None),
            || false,
        );
        assert!(matches!(state, MigrationState::Blocked { .. }), "{state:?}");
        assert!(source.join(PROFILE_FILE).exists());
        assert!(
            dir_entries(accounts::accounts_root(&cfg)).is_empty(),
            "a copy was made that could never have been verified"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_blocked_availability_migrates_nothing_and_leaves_the_profile_alone() {
        // WIRING for the `Refuse` row, through `migrate` rather than through
        // the pure decision, and the message is the one the user can act on.
        let (cfg, source) = planted_profile("migrate-trapped");
        let trap = PathBuf::from(r"C:\a\bin\bitwarden-cli");
        let state = migrate(
            &cfg,
            Some(&source),
            &MultiAccountAvailability::BlockedByPortableProfile {
                relative_data_dir: trap.clone(),
            },
            false,
            |_| panic!("`bw status` must not be run at all when migration is refused"),
            || false,
        );
        let MigrationState::Blocked { reason } = state else {
            panic!("{state:?}")
        };
        assert!(
            reason.contains(&trap.display().to_string()),
            "the directory the user has to remove is not in the message: {reason}"
        );
        assert!(source.join(PROFILE_FILE).exists());
        assert!(dir_entries(accounts::accounts_root(&cfg)).is_empty());

        // Positive control: the same planted profile migrates when it is not
        // trapped.
        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        );
        assert!(matches!(state, MigrationState::Completed { .. }), "{state:?}");
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_migration_is_idempotent_across_a_kill_at_every_stage() {
        // Drives the resume path for real: reproduce on disk exactly what each
        // stage leaves behind, then run `migrate` again and require it to
        // converge on the same end state.
        for stage in [Stage::Copying, Stage::Promoted] {
            let (cfg, source) = planted_profile(&format!("migrate-resume-{stage:?}"));
            let a = AccountId::generate();
            write_marker(
                &cfg,
                &Marker {
                    id: a.clone(),
                    source: source.clone(),
                    source_email: Some("me@example.com".into()),
                    stage,
                },
            )
            .unwrap();
            match stage {
                Stage::Copying => {
                    copy_dir_all(&source, &incoming_path(&cfg, &a)).unwrap();
                    // A torn copy: the file is there, the contents are not.
                    std::fs::write(incoming_path(&cfg, &a).join(PROFILE_FILE), b"").unwrap();
                }
                Stage::Promoted => {
                    copy_dir_all(&source, &accounts::data_dir_for(&cfg, &a)).unwrap();
                }
            }

            let state = migrate(
                &cfg,
                Some(&source),
                &available(),
                false,
                |_| locked_as("me@example.com"),
                || false,
            );

            let MigrationState::Completed { account, .. } = state else {
                panic!("{stage:?}: {state:?}")
            };
            assert!(accounts::data_dir_for(&cfg, &account.id)
                .join(PROFILE_FILE)
                .exists());
            assert!(
                !source.exists(),
                "{stage:?}: the source survived a completed migration"
            );
            assert!(!marker_path(&cfg, &account.id).exists());
            assert!(!incoming_path(&cfg, &account.id).exists());
            assert_eq!(
                dir_entries(accounts::accounts_root(&cfg)),
                vec![account.id.to_string()],
                "{stage:?}: something was left in the accounts root"
            );
            // A resumed `Promoted` keeps the id the marker named -- restarting
            // with a fresh id would strand the already-promoted directory
            // forever.
            if stage == Stage::Promoted {
                assert_eq!(account.id, a, "a resumed promotion invented a new account id");
                assert_eq!(
                    std::fs::read(accounts::data_dir_for(&cfg, &a).join(PROFILE_FILE)).unwrap(),
                    b"{\"profile\":1}"
                );
            }
            let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
        }
    }

    #[test]
    fn a_promoted_marker_whose_source_is_already_gone_finishes_instead_of_restarting() {
        // The state a crash between "delete source" and "delete marker"
        // produces. Reading it as an error would restart a migration that had
        // already succeeded -- and it would restart it with no source, so it
        // would give up and strand the promoted directory.
        let (cfg, source) = planted_profile("migrate-resume-no-source");
        let a = AccountId::generate();
        copy_dir_all(&source, &accounts::data_dir_for(&cfg, &a)).unwrap();
        write_marker(
            &cfg,
            &Marker {
                id: a.clone(),
                source: source.clone(),
                source_email: Some("me@example.com".into()),
                stage: Stage::Promoted,
            },
        )
        .unwrap();
        std::fs::remove_dir_all(&source).unwrap();

        let state = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        );

        let MigrationState::Completed { account, .. } = state else {
            panic!("{state:?}")
        };
        assert_eq!(account.id, a);
        assert!(!marker_path(&cfg, &a).exists());
        assert!(accounts::data_dir_for(&cfg, &a).join(PROFILE_FILE).exists());
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn running_migrate_twice_in_a_row_changes_nothing_the_second_time() {
        let (cfg, source) = planted_profile("migrate-twice");
        let MigrationState::Completed { account, .. } = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        ) else {
            panic!()
        };
        let landed = accounts::data_dir_for(&cfg, &account.id);
        let before = std::fs::read(landed.join(PROFILE_FILE)).unwrap();

        // Second launch: the account list now exists.
        let second = migrate(
            &cfg,
            Some(&source),
            &available(),
            true,
            |_| panic!("nothing should be asked of the CLI on a launch with an account list"),
            || false,
        );
        assert_eq!(second, MigrationState::NothingToMigrate);
        assert_eq!(std::fs::read(landed.join(PROFILE_FILE)).unwrap(), before);
        assert_eq!(
            dir_entries(accounts::accounts_root(&cfg)),
            vec![account.id.to_string()]
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    /// **Rule 5, end to end.** The migration finishes — source deleted, marker
    /// deleted — and `settings.json` is then never written: a power loss, a
    /// kill, or a single failed write (disk full, an AV lock, a roaming
    /// profile) between [`migrate`] returning and `Settings::persist_accounts`
    /// landing. Nothing simulated but the omission itself.
    ///
    /// The next launch therefore sees `accounts/<id>/` holding the whole vault,
    /// no marker, no account entry, and no `%APPDATA%\Bitwarden CLI` to
    /// re-resolve from. It must land on **that** id.
    ///
    /// Fails without the fix: delete the `[only] =>
    /// AdoptUnclaimedAccount(...)` arm in [`resume_action`] (or make
    /// [`Observed::account_dirs_with_profile`] always empty) and the second
    /// launch answers `NothingToMigrate`, `resolve_startup` mints a fresh id,
    /// and `assert_eq!(second_id, first_id)` reports two different ids with
    /// `data.json` still sitting under the first.
    #[test]
    fn a_completed_migration_whose_account_list_was_never_written_is_adopted_not_reminted() {
        let (cfg, source) = planted_profile("migrate-orphan");
        let MigrationState::Completed { account: first, .. } = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        ) else {
            panic!("the first launch must migrate")
        };

        // The state the crash leaves: everything `finish` did, and nothing
        // `main` would have done afterwards.
        assert!(
            !source.exists(),
            "control: the migration really did delete the source, so there is nothing left to \
             re-resolve from"
        );
        assert!(
            !marker_path(&cfg, &first.id).exists(),
            "control: the marker really is gone, so no resume can see the migration"
        );
        assert!(
            accounts::data_dir_for(&cfg, &first.id)
                .join(PROFILE_FILE)
                .is_file(),
            "control: the vault really is in the account directory"
        );

        // The second launch, with the empty account list the failed write left.
        let second = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        );
        let MigrationState::Completed {
            account: ref adopted,
            hello_needs_reenrolment,
        } = second
        else {
            panic!("the second launch abandoned the vault: {second:?}")
        };
        assert_eq!(
            adopted.id, first.id,
            "the second launch is on a different account from the one holding the vault; \
             {} still exists and nothing names it",
            accounts::data_dir_for(&cfg, &first.id).display()
        );
        assert_eq!(
            adopted.email, "me@example.com",
            "the adopted account is unlabelled, so the switcher shows a blank row"
        );
        assert!(
            !hello_needs_reenrolment,
            "the Hello notice was raised a second time for a blob the first launch deleted"
        );

        // And the state `main` actually runs on: `resolve_startup` must point
        // at that directory rather than mint one beside it.
        let startup = accounts::resolve_startup(&[], None, &second);
        let accounts::StartupAccounts::Ready { active, .. } = startup else {
            panic!("{startup:?}")
        };
        assert_eq!(active.id, first.id);
        assert_eq!(
            dir_entries(accounts::accounts_root(&cfg)),
            vec![first.id.to_string()],
            "a second account directory was minted beside the one holding the vault"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    /// The same recovery asked of the pure decision, plus the two neighbours
    /// that must NOT adopt.
    ///
    /// Fails without the fix: the first assertion is `DoNothing`.
    #[test]
    fn an_unclaimed_account_directory_is_adopted_only_with_no_account_list() {
        use ResumeAction::*;
        const B: &str = "fedcba9876543210fedcba9876543210";

        let orphaned = Observed {
            account_dirs_with_profile: vec![id(A)],
            ..observed(None, false, false, false)
        };
        assert_eq!(
            resume_action(&orphaned),
            AdoptUnclaimedAccount(id(A)),
            "the vault in accounts/<id> is left for a freshly minted id to sit beside"
        );

        // A source that survived a `finish` whose deletion failed does not
        // make this a fresh migration: copying again would put a second copy
        // of the same vault under a second id.
        assert_eq!(
            resume_action(&Observed {
                account_dirs_with_profile: vec![id(A)],
                ..observed(None, false, false, true)
            }),
            AdoptUnclaimedAccount(id(A))
        );
        // Positive control for that: with no such directory the same
        // observation still starts a migration.
        assert_eq!(
            resume_action(&observed(None, false, false, true)),
            StartFresh,
            "control: a source with nothing adopted is still a first migration"
        );

        // An account list means every directory is already spoken for.
        assert_eq!(
            resume_action(&Observed {
                accounts_already_configured: true,
                account_dirs_with_profile: vec![id(A)],
                ..observed(None, false, false, false)
            }),
            DoNothing
        );
        // A marker outranks it: that state has a defined resume of its own,
        // and it is the one that still holds the source.
        assert_eq!(
            resume_action(&Observed {
                account_dirs_with_profile: vec![id(A)],
                ..observed(Some(Stage::Promoted), false, true, true)
            }),
            VerifyAndFinish
        );

        // Two unclaimed vaults cannot be told apart, so neither is guessed at.
        let two = Observed {
            account_dirs_with_profile: vec![id(A), id(B)],
            ..observed(None, false, false, false)
        };
        let Refuse { reason } = resume_action(&two) else {
            panic!("two unclaimed vaults were resolved to one: {:?}", resume_action(&two))
        };
        assert!(reason.contains("more than one"), "{reason}");
    }

    /// The scan behind that field: an account directory counts only when it
    /// holds a profile, and the staging names can never be mistaken for one.
    ///
    /// Fails without the fix: the function does not exist. With it, drop the
    /// `PROFILE_FILE` check and the empty directory is reported.
    #[test]
    fn only_an_account_directory_holding_a_profile_counts_as_unclaimed() {
        let root = scratch_dir("unclaimed-scan");
        let cfg = root.join("cfg");
        let a = id(A);
        assert!(
            account_dirs_with_profile(&cfg).is_empty(),
            "a config directory with no accounts root reported something"
        );

        // An empty directory is `prepare_new_account`'s, mid sign-in. There is
        // no vault in it, so adopting it would make an entry naming a profile
        // that is not there -- an account permanently signed out.
        std::fs::create_dir_all(accounts::data_dir_for(&cfg, &a)).unwrap();
        assert!(account_dirs_with_profile(&cfg).is_empty());

        // Neither staging name parses as an id, so neither can be adopted.
        std::fs::create_dir_all(incoming_path(&cfg, &a)).unwrap();
        std::fs::write(incoming_path(&cfg, &a).join(PROFILE_FILE), b"{}").unwrap();
        std::fs::write(marker_path(&cfg, &a), b"{}").unwrap();
        assert!(
            account_dirs_with_profile(&cfg).is_empty(),
            "a staging directory was offered up as an account to adopt"
        );

        // Positive control: the real thing is found.
        std::fs::write(accounts::data_dir_for(&cfg, &a).join(PROFILE_FILE), b"{}").unwrap();
        assert_eq!(account_dirs_with_profile(&cfg), vec![a]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_first_install_with_no_profile_anywhere_is_not_an_error() {
        let root = scratch_dir("migrate-first-install");
        let cfg = root.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let state = migrate(
            &cfg,
            Some(&root.join("Bitwarden CLI")), // never created
            &available(),
            false,
            |_| panic!("there is nothing to ask about"),
            || false,
        );
        assert_eq!(state, MigrationState::NothingToMigrate);
        // And with no source path at all (no %APPDATA%).
        assert_eq!(
            migrate(&cfg, None, &available(), false, |_| panic!(), || false),
            MigrationState::NothingToMigrate
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------------------------------------------------------------- 4.6

    #[test]
    fn the_marker_is_durable_before_copying_and_the_source_dies_after_verification() {
        // Two orderings, and they are the whole of rules 2 and 3:
        //  * a crash after the first byte and before the marker lands leaves
        //    an UNLABELLED partial copy that the resume table cannot see --
        //    `ClearStaleMarkerAndReassess` never runs for it and
        //    `<id>.incoming` sits there forever;
        //  * deleting the source before verification is the data-loss bug this
        //    entire module exists to prevent, and it leaves NO trace in the end
        //    state of a successful run, so only an ordering assertion can catch
        //    it.
        let (cfg, source) = planted_profile("migrate-ordering");
        std::fs::write(cfg.join("session.bin"), b"dpapi-wrapped").unwrap();
        std::fs::write(cfg.join("hello.bin"), b"sealed").unwrap();

        let order = std::cell::RefCell::new(Vec::new());
        let state = migrate_with_probe(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
            &mut |event| order.borrow_mut().push(event),
        );
        assert!(matches!(state, MigrationState::Completed { .. }), "{state:?}");

        let order = order.into_inner();
        let at = |e: Probe| {
            order
                .iter()
                .position(|x| *x == e)
                .unwrap_or_else(|| panic!("no {e:?}: {order:?}"))
        };
        assert!(
            at(Probe::MarkerFlushed) < at(Probe::CopyStarted),
            "copying began before the marker was durable: {order:?}"
        );
        assert!(
            at(Probe::CopyFinished) < at(Probe::Promoted),
            "the staging copy was promoted before it finished: {order:?}"
        );
        assert!(
            at(Probe::Verified) < at(Probe::SourceDeleted),
            "the source was deleted before verification passed: {order:?}"
        );
        assert!(
            at(Probe::Verified) < at(Probe::SessionMoved),
            "the session token was moved out of the shared directory before the copy was \
             verified: {order:?}"
        );
        assert!(
            at(Probe::Verified) < at(Probe::HelloDeleted),
            "the Hello blob was deleted before the copy was verified: {order:?}"
        );
        assert!(
            at(Probe::SourceDeleted) < at(Probe::MarkerRemoved),
            "the marker went first, so a crash here would restart a finished migration: {order:?}"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_failed_verification_never_reaches_a_single_destructive_step() {
        // The positive control's negative twin, over the same sink: on the run
        // that must delete nothing, none of the deleting events happen at all.
        let (cfg, source) = planted_profile("migrate-ordering-failed");
        std::fs::write(cfg.join("session.bin"), b"dpapi-wrapped").unwrap();
        std::fs::write(cfg.join("hello.bin"), b"sealed").unwrap();

        let order = std::cell::RefCell::new(Vec::new());
        let source_for_match = source.clone();
        let state = migrate_with_probe(
            &cfg,
            Some(&source),
            &available(),
            false,
            move |dir| {
                if dir == Some(source_for_match.as_path()) {
                    locked_as("me@example.com") // the source identifies itself
                } else {
                    unauthenticated() // ...and the copy cannot be opened
                }
            },
            || false,
            &mut |event| order.borrow_mut().push(event),
        );
        assert!(matches!(state, MigrationState::Blocked { .. }), "{state:?}");
        let order = order.into_inner();
        // It really did get as far as having something to verify -- otherwise
        // the absences below would be trivially true.
        assert!(
            order.contains(&Probe::CopyFinished) && order.contains(&Probe::Promoted),
            "the run never reached verification at all: {order:?}"
        );
        for forbidden in [
            Probe::Verified,
            Probe::SessionMoved,
            Probe::HelloDeleted,
            Probe::SourceDeleted,
        ] {
            assert!(
                !order.contains(&forbidden),
                "{forbidden:?} happened on a run that verified nothing: {order:?}"
            );
        }
        assert!(source.join(PROFILE_FILE).exists());
        assert_eq!(std::fs::read(cfg.join("hello.bin")).unwrap(), b"sealed");
        assert_eq!(std::fs::read(cfg.join("session.bin")).unwrap(), b"dpapi-wrapped");
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn the_marker_is_flushed_to_the_platter_and_not_merely_written() {
        // Unavoidably a source guard: a buffered write and a synced one are
        // indistinguishable from outside the process, and the difference is
        // whether a marker survives the power loss it exists for. The
        // ordering test above proves `MarkerFlushed` precedes the copy; only
        // this proves the event means what it says.
        //
        // Needles are `concat!`-split so none can match its own declaration
        // here, and single-line so a CRLF checkout cannot turn them into false
        // passes. Each is a *required* needle, so the assertion is itself the
        // proof that it matches live code.
        let source = include_str!("migration.rs");
        for required in [
            concat!("file.", "sync_all()?;"),
            concat!("probe(Probe::", "MarkerFlushed);"),
        ] {
            assert!(
                source.contains(required),
                "`{required}` is gone: the marker is no longer made durable before the copy \
                 it labels"
            );
        }
        // Positive control on `contains` over this same text, so the two
        // assertions above are discriminating rather than trivially true.
        assert!(!source.contains(concat!("file.", "sync_none()")));
    }

    // ---------------------------------------------------------------- 4.7

    #[test]
    fn migration_reports_that_hello_needs_reenrolling_only_when_it_was_enrolled() {
        let (cfg, source) = planted_profile("migrate-hello");
        std::fs::write(cfg.join("hello.bin"), b"sealed").unwrap();
        let MigrationState::Completed {
            account,
            hello_needs_reenrolment,
        } = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        ) else {
            panic!()
        };
        assert!(
            hello_needs_reenrolment,
            "the user is not told their quick unlock stopped working"
        );
        assert!(
            !cfg.join("hello.bin").exists(),
            "a sealed master password that NOTHING can open was left on disk"
        );
        assert!(
            !accounts::hello_blob_path_for(&cfg, &account.id).exists(),
            "the unopenable blob was copied into the account instead of being deleted"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());

        // Positive control: a user who never enrolled is told nothing.
        let (cfg2, source2) = planted_profile("migrate-no-hello");
        let MigrationState::Completed {
            hello_needs_reenrolment,
            ..
        } = migrate(
            &cfg2,
            Some(&source2),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        ) else {
            panic!()
        };
        assert!(!hello_needs_reenrolment);
        let _ = std::fs::remove_dir_all(cfg2.parent().unwrap());
    }

    #[test]
    fn the_session_token_moves_with_the_account_so_the_first_launch_needs_no_password() {
        // `bw` is still unlocked under this token; not carrying it over would
        // demand a master password on the very launch that migrated, which
        // reads to the user as the migration having lost something.
        let (cfg, source) = planted_profile("migrate-session");
        std::fs::write(cfg.join("session.bin"), b"dpapi-wrapped").unwrap();
        let MigrationState::Completed { account, .. } = migrate(
            &cfg,
            Some(&source),
            &available(),
            false,
            |_| locked_as("me@example.com"),
            || false,
        ) else {
            panic!()
        };
        assert_eq!(
            std::fs::read(accounts::session_path_for(&cfg, &account.id)).unwrap(),
            b"dpapi-wrapped"
        );
        assert!(
            !cfg.join("session.bin").exists(),
            "the token was left in the shared directory too"
        );
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }
}

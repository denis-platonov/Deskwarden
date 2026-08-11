//! Exports the vault to a file, by spawning the Bitwarden CLI beside the
//! running `bw serve`.
//!
//! **Why a spawn at all, when everything else in this crate talks to the
//! server.** `bw serve` has no export endpoint: export is a CLI-only verb.
//! There is no request `vault_bridge` could send that would produce a vault
//! archive, so this one operation runs the `bw` binary directly — the same
//! binary `bw_path::bw_command` already guarantees is the signature-checked
//! one, never a `Command::new` built here on the bare binary name.
//!
//! # Only `encrypted_json`, and why the other formats are not a setting
//!
//! `bw export` offers `csv`, `json`, `encrypted_json` and `zip`. This module
//! offers exactly one of them, and that is a decision about Windows rather
//! than a missing feature.
//!
//! A plaintext vault file written on Windows is **not recoverable**. It
//! survives its own deletion — the bytes stay in unallocated clusters, and on
//! a stock install the obvious destinations make further copies before the
//! user has finished reading the confirmation dialog: Desktop and Documents
//! are OneDrive-synced by default, Volume Shadow Copy snapshots them, and the
//! search indexer reads the contents into its own database. "Are you sure?"
//! does not make any of that reversible; by the time the answer is "no" the
//! file exists in four places the user cannot enumerate, let alone erase.
//!
//! `encrypted_json` run **without** `--password` is encrypted under the
//! account's own encryption key. It restores only into that same account, and
//! — the part that matters here — it introduces **zero new secrets**: nothing
//! is placed on argv, nothing in an environment variable, nothing that would
//! need [`zeroize`](https://docs.rs/zeroize) on the way out. The session
//! token this module already has is the only secret in play, and it travels
//! in `BW_SESSION` exactly as every other spawn in this crate sends it.
//!
//! `--organizationid` is likewise absent: an organization export is a
//! different consent conversation (it exports data the user may administer
//! but does not own), and v1 does not have that conversation.
//!
//! # Why password-protected export is out of v1
//!
//! Verified against the installed CLI (`bw 2026.7.0`) rather than assumed.
//! `bw login` and `bw unlock` both accept `--passwordenv <var>` and
//! `--passwordfile <path>`, and `login_ui::run_bw_with_password` uses
//! `--passwordenv` deliberately so the master password never appears in the
//! process list.
//!
//! **`bw export` has no equivalent.** Its whole option list is `--output`,
//! `--format`, `--password [password]`, `--organizationid`. The password is
//! an argv value (the CLI's own examples show it as a bare positional,
//! `bw export myPassword321 --format json`), and on Windows any process
//! running as the same user can read another process's command line. So a
//! password-protected export would take a secret the user typed and publish
//! it to every process on the desktop for the lifetime of the spawn — to buy
//! an export that is *already* encrypted without it. That is a strictly worse
//! trade, and it is why the option is not offered rather than merely not
//! implemented.
//!
//! # The `.dw-partial` staging name
//!
//! The CLI writes to `--output` as it goes. If it dies halfway, the chosen
//! path would otherwise hold a truncated archive with the name of a good one.
//! So the spawn is always pointed at `<destination>.dw-partial`, and only a
//! [`ExportOutcome::Written`] classification may promote it to the name the
//! user picked. The promotion itself is the runner's job, not this module's.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::bw_path::bw_command;
use crate::job_object::KillOnCloseJob;

/// The one export format this crate offers. See the module docs.
pub const EXPORT_FORMAT: &str = "encrypted_json";

/// The suffix the in-progress file carries until it is known to be complete.
pub const PARTIAL_SUFFIX: &str = ".dw-partial";

/// Markers that say the head of the file is an *encrypted* Bitwarden export
/// envelope, rather than a plaintext one or a CLI error message that happened
/// to land in the output file.
///
/// Both are literal keys the `encrypted_json` writer emits near the start of
/// the document; a `csv` or plaintext `json` export contains neither.
const ENVELOPE_MARKERS: [&str; 2] = ["\"encrypted\"", "encKeyValidation_DO_NOT_EDIT"];

/// Words the CLI uses when the session it was handed is no longer good. Kept
/// separate from generic failure so the UI can say "unlock and try again"
/// instead of showing the raw stderr.
const SESSION_VOCABULARY: [&str; 5] = [
    "not logged in",
    "vault is locked",
    "session key",
    "invalid session",
    "mac failed",
];

/// What the user asked for: one full **file** path, not a directory.
///
/// The CLI's `--output` accepts either, and given a directory it invents its
/// own timestamped name — which would leave this crate unable to say what it
/// wrote, and unable to stage through a `.dw-partial` sibling. So the caller
/// resolves a file name first and this type only ever carries one.
pub struct ExportRequest {
    pub destination: PathBuf,
}

/// The decided shape of one export. Nothing here has touched the disk.
///
/// **The fields are private and [`plan_export`] is the only way to make one.**
/// That is not tidiness. `run_export` promotes `partial_path` over
/// `final_path` on success and *deletes* `partial_path` on every other
/// outcome, so a call site able to write its own struct could hand over
/// `ExportPlan { final_path: x, partial_path: x }` -- and one failed export
/// would then delete the user's previous backup outright, with every test in
/// this module still green. `send::SendInvocation` is closed the same way and
/// for the same reason; this type was the one that was still open, and the
/// call site that would have done it (the export command's wiring) is not
/// written yet, which is exactly why closing it costs nothing today.
///
/// The suffix invariant is **enforced rather than assumed**: no constructor
/// accepts a staging path, so `partial_path` can only ever be `final_path`
/// with [`PARTIAL_SUFFIX`] appended.
#[derive(Debug, PartialEq, Eq)]
pub struct ExportPlan {
    final_path: PathBuf,
    partial_path: PathBuf,
    format: &'static str,
}

impl ExportPlan {
    /// The one constructor. Private, and it **derives** the staging path
    /// instead of taking one, so the two paths cannot be made equal.
    fn staged(final_path: PathBuf) -> Self {
        let mut partial = final_path.clone().into_os_string();
        partial.push(PARTIAL_SUFFIX);
        Self {
            final_path,
            partial_path: PathBuf::from(partial),
            format: EXPORT_FORMAT,
        }
    }

    /// Where the finished archive belongs, if it turns out to be finished.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Where the CLI is actually pointed: [`Self::final_path`] +
    /// [`PARTIAL_SUFFIX`]. Never equal to it.
    pub fn partial_path(&self) -> &Path {
        &self.partial_path
    }

    /// Always [`EXPORT_FORMAT`]. Read off the plan rather than off the
    /// constant at the call site so the argv builder cannot drift from what
    /// was planned.
    pub fn format(&self) -> &'static str {
        self.format
    }
}

/// Why a destination was refused before anything ran.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportRefusal {
    /// The path names no containing directory at all.
    NoDirectory,
    /// The path lands in (or under) deskwarden's own config directory.
    ///
    /// This is not hypothetical tidiness: a shell save dialog will happily
    /// write a vault archive into `%APPDATA%\Deskwarden` if the user clicks
    /// through it, where it sits beside `settings.json` looking like part of
    /// the app's own state — backed up, synced and eventually restored with
    /// it, and deleted by an uninstaller that has no idea it is holding the
    /// user's only copy of their vault.
    IntoConfigDir,
    /// The path names a directory, or ends in a separator, and so carries no
    /// file name for the archive.
    EmptyFileName,
}

/// Decides where an export would go. **Pure**: no filesystem call, no spawn.
///
/// `config_dir` is passed in rather than read from the environment so the
/// refusal can be exercised against a `TempDir` and never against the real
/// `%APPDATA%\Deskwarden`.
pub fn plan_export(req: &ExportRequest, config_dir: &Path) -> Result<ExportPlan, ExportRefusal> {
    // A trailing separator is checked textually, BEFORE `file_name`, because
    // `Path::file_name` normalizes it away: `C:\Backups\` answers `Backups`,
    // which is a directory the user picked and not a file name they chose.
    // Left to `file_name` alone this module would plan `C:\Backups\` as its
    // own output file.
    let spelled = req.destination.to_string_lossy();
    if spelled.ends_with('\\') || spelled.ends_with('/') {
        return Err(ExportRefusal::EmptyFileName);
    }
    if req.destination.file_name().unwrap_or_default().is_empty() {
        return Err(ExportRefusal::EmptyFileName);
    }

    let parent = match req.destination.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        // Either no parent at all (a bare root, or a bare file name with
        // nowhere to put it) or an empty one. Both mean the same thing to a
        // spawn: there is no directory to write into.
        _ => return Err(ExportRefusal::NoDirectory),
    };

    if is_within(parent, config_dir) {
        return Err(ExportRefusal::IntoConfigDir);
    }

    Ok(ExportPlan::staged(req.destination.clone()))
}

/// `true` if `candidate` is `root` or sits underneath it.
///
/// Compared component by component, case-insensitively, because Windows path
/// comparison is case-insensitive and `%APPDATA%` reaches this function in
/// whatever case the shell dialog handed back. Deliberately textual and
/// therefore pure: it does not canonicalize, so it does not follow a junction
/// or resolve `..`. That makes it a floor rather than a proof — it catches
/// the case the refusal exists for (a save dialog pointed straight at the
/// config directory) without a filesystem call in a pure function.
fn is_within(candidate: &Path, root: &Path) -> bool {
    let fold = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let (candidate, root) = (fold(candidate), fold(root));
    if root.is_empty() {
        return false;
    }
    candidate.len() >= root.len() && candidate[..root.len()] == root[..]
}

/// The exact argv the CLI would be given, minus the program itself.
///
/// **Pure**, and public, so the wiring is observable in a test without
/// spawning anything: the format that reaches the process and the file it is
/// pointed at are both readable from this list.
pub fn export_args(plan: &ExportPlan) -> Vec<String> {
    vec![
        "export".to_string(),
        "--format".to_string(),
        plan.format().to_string(),
        "--output".to_string(),
        plan.partial_path().to_string_lossy().into_owned(),
    ]
}

/// Everything observed about one finished spawn, with no judgement applied.
pub struct RawExport {
    pub exit_ok: bool,
    pub stderr: String,
    /// `Ok(len)` if the partial file could be stat'd, `Err(why)` if **the
    /// check itself** failed.
    ///
    /// The distinction is the whole reason this is a `Result` and not a
    /// `u64`: `Err` is emphatically **not** `Ok(0)`. `Ok(0)` says the file
    /// is there and empty; `Err` says nobody knows what is there. Collapsing
    /// the two would let "the stat failed" be reported to the user with the
    /// same words as a good export.
    pub written: Result<u64, String>,
    /// The first bytes of the partial file, for the envelope check.
    pub head: Vec<u8>,
}

/// The verdict on one export.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    /// The archive is there, non-empty, and looks like an encrypted envelope.
    Written,
    /// The session is stale; the user needs to unlock and retry.
    SessionInvalid,
    /// The CLI failed and said why.
    Failed(String),
    /// The CLI claimed success and **this crate could not confirm it**.
    ///
    /// This variant exists for one purpose: to make a silent success
    /// impossible. A stat that failed, an empty file, or a head that is not
    /// an encrypted envelope all land here rather than in [`Self::Written`],
    /// because the house rule is that *could not check* must never render as
    /// success. There may well be a perfectly good archive on disk in this
    /// state — the point is that nothing here has established that, so
    /// nothing here may say so.
    Unconfirmed,
}

/// Turns one observation into one verdict. **Pure**: the entire decision.
pub fn classify(raw: &RawExport) -> ExportOutcome {
    if !raw.exit_ok {
        let lowered = raw.stderr.to_lowercase();
        if SESSION_VOCABULARY.iter().any(|w| lowered.contains(w)) {
            return ExportOutcome::SessionInvalid;
        }
        return ExportOutcome::Failed(raw.stderr.trim().to_string());
    }

    // Exit 0 from here down. Every arm below is a way for a *successful*
    // exit to still not be a confirmed archive.
    let Ok(len) = &raw.written else {
        // The check failed. Not zero bytes — unknown bytes.
        return ExportOutcome::Unconfirmed;
    };
    if *len == 0 {
        return ExportOutcome::Unconfirmed;
    }
    if !looks_like_an_encrypted_envelope(&raw.head) {
        return ExportOutcome::Unconfirmed;
    }
    ExportOutcome::Written
}

/// `true` if these bytes open like an `encrypted_json` export.
///
/// Guards against the CLI exiting 0 having written something that is not the
/// format that was asked for — a plaintext export, or an error document.
fn looks_like_an_encrypted_envelope(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    ENVELOPE_MARKERS.iter().any(|m| text.contains(m))
}

/// The `Command` that would run this export, built but not spawned.
///
/// Goes through [`bw_command`] and nothing else. That function hands back the
/// one `bw.exe` startup resolved *and* signature-checked, and refuses if that
/// check never passed; a `Command::new` built here on the bare binary name
/// would hand the session token to whatever `CreateProcess`'s search order
/// turned up, bypassing the
/// entire chain described in `bw_path`.
///
/// The session travels in `BW_SESSION`, never on argv — the same choice
/// `bw_serve::run_bw_sync` makes, and for the same reason the module docs
/// give for the export password: on Windows a sibling process can read this
/// process's command line, and it cannot read this process's environment.
pub fn export_command(plan: &ExportPlan, session_token: &str) -> Result<Command, String> {
    let mut cmd = bw_command()?;
    cmd.args(export_args(plan));
    cmd.env("BW_SESSION", session_token);
    Ok(cmd)
}

/// How long a `.dw-partial` must have sat untouched before the sweep treats
/// it as leftover rather than as an export somebody is still running.
///
/// Generous on purpose. The cost of waiting too long is one encrypted file of
/// litter; the cost of being too eager is deleting the output of an export
/// that is still writing it, which turns a working backup into a failure the
/// user cannot explain.
pub const STALE_PARTIAL_AGE: Duration = Duration::from_secs(10 * 60);

/// How many bytes of the partial are read for the envelope check. The markers
/// `looks_like_an_encrypted_envelope` searches for are in the first object of
/// the document, well inside this.
const HEAD_BYTES: usize = 512;

/// Runs one planned export and reports what was seen, judging nothing.
///
/// A boxed closure rather than a trait so the UI can hold one in a struct
/// field and a test can hand over a fake that writes bytes instead of
/// starting a process. `Send + Sync` because the export runs on a worker
/// thread while the window keeps painting.
pub type ExportRunner = std::sync::Arc<dyn Fn(&ExportPlan, &str) -> RawExport + Send + Sync>;

/// The runner that really spawns the CLI.
///
/// The job object is taken by `Arc` because it is `main.rs`'s, shared with
/// the thread that starts `bw serve`, and it must outlive every child it
/// owns. `Arc<Option<..>>` rather than `Option<Arc<..>>` so the "no job at
/// all" case is the one `main.rs` already holds and not a second shape
/// invented here -- **and so that the absence is expressed once, at the top,
/// where job creation genuinely can fail, rather than silently at the spawn.**
///
/// Membership of the job is not a nicety. The child is a process holding an
/// unlocked vault, and kill-on-close is what guarantees it dies with this
/// process however this process dies -- a panic, a `process::exit` that skips
/// every destructor, or a Task Manager kill. Without it a crash mid-export
/// leaves a `bw` running with the session token in its environment.
///
/// **This is a named struct and not a bare closure, and that is the fix for a
/// review finding rather than a preference.** The guarantee above used to be
/// held by a source pin over the `spawn_in_job` call alone. A pin over a call
/// says nothing about the value flowing into it: replacing the body with
/// `run_cli(plan, session, job.as_ref().as_ref().filter(|_| false))` left the
/// pinned line word-perfect, every test in the crate green and no warning
/// emitted -- and every export child spawned outside the job. The job is now
/// a value on a type, [`CliExportRunner::job`] is the single place the spawn
/// reads it from, and
/// [`the_runner_spawns_into_the_very_job_it_was_constructed_with`] asserts
/// that the job reaching that point is the one the constructor was handed.
pub struct CliExportRunner {
    job: Arc<Option<KillOnCloseJob>>,
}

impl CliExportRunner {
    pub fn new(job: Arc<Option<KillOnCloseJob>>) -> Self {
        Self { job }
    }

    /// The job the child will be assigned to, or `None` when this process
    /// never managed to create one.
    ///
    /// **The only place [`Self::run`] reads the job from**, which is what
    /// makes a test of this accessor a test of what the spawn actually gets.
    pub fn job(&self) -> Option<&KillOnCloseJob> {
        self.job.as_ref().as_ref()
    }

    fn run(&self, plan: &ExportPlan, session: &str) -> RawExport {
        run_cli(plan, session, self.job())
    }

    /// Boxes this runner into the seam [`run_export`] takes.
    pub fn into_runner(self) -> ExportRunner {
        Arc::new(move |plan, session| self.run(plan, session))
    }
}

/// The production [`ExportRunner`], for the one call site that wires it up.
pub fn real_runner(job: Arc<Option<KillOnCloseJob>>) -> ExportRunner {
    CliExportRunner::new(job).into_runner()
}

/// One spawn, start to finish, with nothing judged and nothing renamed.
///
/// Both output streams are captured and stdin is `null`: an inherited stream
/// would attach the child to a console this app does not own, and `bw export`
/// has nothing to read from stdin. The spawn goes through
/// [`crate::job_object::spawn_in_job`], which spawns suspended, assigns the
/// child to the job and only then resumes it -- and which re-ORs
/// `CREATE_NO_WINDOW`, because `creation_flags` **replaces** the flags a
/// command is holding rather than adding to them, so the flag
/// [`bw_command`] already set would otherwise be dropped and a console window
/// would flash on screen.
fn run_cli(plan: &ExportPlan, session: &str, job: Option<&KillOnCloseJob>) -> RawExport {
    let mut command = match export_command(plan, session) {
        Ok(command) => command,
        Err(why) => return nothing_ran(why),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = match crate::job_object::spawn_in_job(job, command) {
        Ok(child) => child,
        Err(e) => {
            return nothing_ran(format!(
                "Bitwarden's command-line tool could not be started ({e}). Nothing was exported."
            ))
        }
    };

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => {
            // The child was started, so something may well be on disk -- but
            // this side never learned how it ended. Reported as a failure
            // with its own words; `run_export` deletes the partial either
            // way, which is the safe direction.
            return nothing_ran(format!(
                "Bitwarden's command-line tool was started but could not be waited on ({e})."
            ));
        }
    };

    // Observed BEFORE anything is renamed, and off the partial path, because
    // the partial is the only file this crate has written anything to yet.
    let (written, head) = observe_partial(plan);
    RawExport {
        exit_ok: output.status.success(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        written,
        head,
    }
}

/// The observation for a run that produced no process at all.
///
/// `written` is `Err`, not `Ok(0)`: nothing looked, so nothing knows. It is
/// only read by `classify` on a zero exit anyway, and this is not one.
fn nothing_ran(why: String) -> RawExport {
    RawExport {
        exit_ok: false,
        stderr: why,
        written: Err("no export process ran, so nothing was checked".to_string()),
        head: Vec::new(),
    }
}

/// Stats and samples **the partial**, never the final path.
///
/// Public, and taking the whole plan rather than a bare path, so the choice
/// of which of the two files is measured is a fact a test can read back: a
/// version that looked at `final_path` would answer with the bytes of
/// whatever was already there -- a previous good backup, most of the time --
/// and so would confirm an export that never happened.
///
/// The size is a `Result` all the way out, and deliberately not flattened to
/// `Ok(0)` on error. `Ok(0)` says the file is there and empty; `Err` says
/// nobody knows. [`classify`] routes the second to [`ExportOutcome::Unconfirmed`],
/// and collapsing them here would make that arm unreachable and let a failed
/// check render as a success one layer above.
pub fn observe_partial(plan: &ExportPlan) -> (Result<u64, String>, Vec<u8>) {
    let path = plan.partial_path();
    let written = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("could not check {}: {e}", path.display()));
    (written, read_head(path))
}

/// The first [`HEAD_BYTES`] of `path`, or nothing if it cannot be read.
///
/// An unreadable head is empty rather than an error because `classify`
/// already refuses to confirm a head that is not an envelope, and an empty
/// one is not; there is no outcome an error here could reach that the empty
/// vector does not already reach.
fn read_head(path: &Path) -> Vec<u8> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; HEAD_BYTES];
    let mut filled = 0;
    // Read in a loop: one `read` may legally return fewer bytes than asked
    // for, and a short first chunk would otherwise truncate the head to
    // before the markers.
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => break,
        }
    }
    buf.truncate(filled);
    buf
}

/// Runs one export and leaves the disk in exactly one of two states: the
/// archive under the name the user picked, or no file of this crate's making.
///
/// The order is the whole of it.
///
/// 1. **Sweep** stale `.dw-partial` files out of the destination directory.
///    An app killed mid-export leaves one behind; it is encrypted, so this is
///    litter rather than exposure, but it is litter in a folder the user
///    chose. Same shape as [`crate::updater::cleanup_stale_downloads`].
/// 1b. **Delete this export's own staging file**, unconditionally, whatever
///    its age. The sweep above only takes partials older than
///    [`STALE_PARTIAL_AGE`], so it cannot be relied on for this one.
/// 2. **Run**, through the injected runner, which is pointed at
///    `plan.partial_path()` and never at `plan.final_path()`.
/// 3. **Judge**, with [`classify`] and nothing else.
/// 4. **Promote only on [`ExportOutcome::Written`]**, by renaming the partial
///    over the final path. Every other verdict deletes the partial instead.
///
/// Step 4 is the reason the staging name exists. Pointing the CLI straight at
/// the chosen path would mean a failed or half-finished export overwriting
/// the previous good backup with a truncated file carrying its name -- the
/// user would have destroyed their only copy by trying to make a second one,
/// and nothing would say so. So the final path is touched on exactly one
/// path through this function, and that path is the one where the bytes have
/// been stat'd and read and found to open like an encrypted envelope.
///
/// A rename that itself fails is a failure, and it also deletes the partial:
/// a `.dw-partial` left beside the user's backups is a file they did not ask
/// for and cannot identify, and the export it holds was never promoted, so
/// nothing is lost by removing it.
pub fn run_export(plan: &ExportPlan, session: &str, runner: &ExportRunner) -> ExportOutcome {
    if let Some(dir) = plan.partial_path().parent() {
        let swept = sweep_stale_partials(dir, SystemTime::now(), STALE_PARTIAL_AGE);
        if swept > 0 {
            log::info!("removed {swept} stale export partial(s) from {}", dir.display());
        }
    }

    // The staging file this run is about to use is removed BEFORE the runner
    // is invoked, and the sweep above is emphatically not enough on its own:
    // it only takes partials that have sat for `STALE_PARTIAL_AGE`, so a
    // leftover from a crash five minutes ago survives it. If the CLI then
    // exits 0 having written nothing -- which is exactly the case
    // `ExportOutcome::Unconfirmed` exists for -- `observe_partial` would stat
    // and sample THAT file, `classify` would find a valid envelope head, and
    // step 4 would rename an earlier run's half-finished archive over the
    // backup the user already had. After this line the only bytes at
    // `partial_path` are bytes this run's runner put there.
    discard_partial(plan);

    let raw = runner(plan, session);
    let outcome = classify(&raw);

    if outcome != ExportOutcome::Written {
        discard_partial(plan);
        return outcome;
    }

    match std::fs::rename(plan.partial_path(), plan.final_path()) {
        Ok(()) => ExportOutcome::Written,
        Err(e) => {
            let why = format!(
                "The export was written but could not be renamed to {} ({e}).",
                plan.final_path().display()
            );
            discard_partial(plan);
            ExportOutcome::Failed(why)
        }
    }
}

/// Removes the staged file, best effort.
///
/// A failure to delete is logged and not reported: the export already failed
/// and the user is about to be told so, and "and also a temporary file could
/// not be removed" is not information they can act on.
fn discard_partial(plan: &ExportPlan) {
    match std::fs::remove_file(plan.partial_path()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "could not delete the staged export {}: {e}",
            plan.partial_path().display()
        ),
    }
}

/// Deletes `.dw-partial` files in `dir` that have sat untouched for at least
/// `older_than`. Answers how many were removed.
///
/// `now` is a parameter rather than a call to the clock so the threshold can
/// be exercised on both sides without a test sleeping for ten minutes.
///
/// What it deletes: regular files whose name ends in [`PARTIAL_SUFFIX`] and
/// whose last-modified time is at least `older_than` in the past.
///
/// What it leaves, every one of them on purpose:
/// * anything else in the directory -- the user's own files live here
/// * a partial younger than `older_than`, which may be an export in progress
/// * a *directory* whose name happens to end in the suffix
/// * an entry whose modification time cannot be read, or which claims to be
///   from the future (a clock change, or a file copied from another machine):
///   an age that cannot be established is not an old age
/// * a file named exactly `.dw-partial` with nothing before it, which this
///   crate never creates -- the suffix is always appended to a chosen name
///
/// Best effort throughout: an unreadable directory answers zero, and a file
/// that will not delete is logged and skipped. Neither is worth failing an
/// export the user asked for.
pub fn sweep_stale_partials(dir: &Path, now: SystemTime, older_than: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(PARTIAL_SUFFIX) || name.len() == PARTIAL_SUFFIX.len() {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_file() => {}
            _ => continue,
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        // `duration_since` is an `Err` when `modified` is after `now`, which
        // is the future-dated case above: skipped, not treated as age zero
        // and not treated as infinitely old.
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < older_than {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => log::warn!(
                "could not delete the stale export partial {}: {e}",
                entry.path().display()
            ),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private temp dir that cleans itself up, so these tests don't need a
    /// dev-dependency just to get a scratch directory. Same idiom as
    /// `logging.rs`'s tests. `plan_export` is pure and never looks at the
    /// disk, so this exists only to make the config-directory refusal read
    /// against a real, disposable path rather than the user's actual
    /// `%APPDATA%\Deskwarden`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "deskwarden-vault-export-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a scratch directory under the system temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn plan_for(path: &str) -> Result<ExportPlan, ExportRefusal> {
        plan_export(
            &ExportRequest {
                destination: PathBuf::from(path),
            },
            Path::new("C:\\Users\\nobody\\AppData\\Roaming\\Deskwarden"),
        )
    }

    #[test]
    fn the_staging_file_is_never_the_users_own_file_for_any_destination() {
        // REVIEW FINDING. `ExportPlan` used to have three `pub` fields and
        // `run_export` takes one directly, so a call site could have written
        // `ExportPlan { final_path: x, partial_path: x, .. }` -- and then a
        // failed export would delete the user's previous backup outright,
        // because `discard_partial` removes `partial_path` and nothing
        // checked the two were different. The fields are private now and
        // `staged` DERIVES the second from the first, so the equal-paths plan
        // is unbuildable rather than merely untested; this holds the
        // invariant that makes the constructor's derivation correct.
        let names = [
            "vault.json",
            "vault.json.dw-partial",
            "no-extension",
            "two.dots.json",
            "\u{4e2d}\u{6587}.json",
            "a name with spaces.json",
        ];
        let mut ran = 0usize;
        for name in names {
            let plan = plan_for(&format!("C:\\Users\\nobody\\Backups\\{name}"))
                .expect("a plain path plans");
            assert_ne!(
                plan.partial_path(),
                plan.final_path(),
                "{name}: the staging file IS the user's own file, so a failed export would \
                 delete the backup it was meant to replace"
            );
            assert_eq!(
                plan.partial_path().to_string_lossy(),
                format!("{}{PARTIAL_SUFFIX}", plan.final_path().to_string_lossy()),
                "{name}: the staging path is not the destination plus the suffix"
            );
            assert_eq!(
                plan.partial_path().parent(),
                plan.final_path().parent(),
                "{name}: the promotion must be a rename within one directory, not a copy \
                 across volumes"
            );
            ran += 1;
        }
        assert_eq!(ran, names.len(), "control: every name was planned");
    }

    #[test]
    fn a_full_file_path_plans_a_partial_beside_it() {
        let plan = plan_for("C:\\Users\\nobody\\Backups\\vault.json").expect("a plain path plans");
        assert_eq!(
            plan.final_path(),
            PathBuf::from("C:\\Users\\nobody\\Backups\\vault.json")
        );
        assert_eq!(
            plan.partial_path(),
            PathBuf::from("C:\\Users\\nobody\\Backups\\vault.json.dw-partial"),
            "the partial is the final path with the suffix appended -- same directory, so the \
             promotion is a rename within one volume and not a copy"
        );
        assert_eq!(plan.format(), "encrypted_json");
    }

    #[test]
    fn a_path_with_no_directory_is_refused() {
        assert_eq!(plan_for("vault.json"), Err(ExportRefusal::NoDirectory));
    }

    #[test]
    fn a_path_with_no_file_name_is_refused() {
        assert_eq!(
            plan_for("C:\\Users\\nobody\\Backups\\"),
            Err(ExportRefusal::EmptyFileName),
            "a trailing separator names a directory, and the CLI would invent its own file name \
             inside it -- which this crate could then neither stage nor promote"
        );
        assert_eq!(plan_for(""), Err(ExportRefusal::EmptyFileName));
    }

    #[test]
    fn the_config_directory_is_refused_and_a_sibling_of_it_is_not() {
        let temp = TempDir::new("configdir");
        let config = temp.path().join("Deskwarden");

        // Directly in it.
        let inside = ExportRequest {
            destination: config.join("vault.json"),
        };
        assert_eq!(
            plan_export(&inside, &config),
            Err(ExportRefusal::IntoConfigDir)
        );

        // Under it, one level down.
        let deeper = ExportRequest {
            destination: config.join("sub").join("vault.json"),
        };
        assert_eq!(
            plan_export(&deeper, &config),
            Err(ExportRefusal::IntoConfigDir)
        );

        // Case-insensitively in it, because Windows paths are.
        let shouted = ExportRequest {
            destination: PathBuf::from(config.to_string_lossy().to_uppercase()).join("vault.json"),
        };
        assert_eq!(
            plan_export(&shouted, &config),
            Err(ExportRefusal::IntoConfigDir)
        );

        // Control, and the one that stops the refusal being "refuse
        // everything": a SIBLING whose name merely starts with the config
        // directory's is fine, and so is an unrelated directory.
        let sibling = ExportRequest {
            destination: temp.path().join("DeskwardenBackups").join("vault.json"),
        };
        assert!(
            plan_export(&sibling, &config).is_ok(),
            "`DeskwardenBackups` is not inside `Deskwarden`; a textual prefix test would say it \
             was"
        );
        let elsewhere = ExportRequest {
            destination: temp.path().join("vault.json"),
        };
        assert!(plan_export(&elsewhere, &config).is_ok());
    }

    #[test]
    fn every_refusal_variant_is_reachable() {
        // A closed check that the three refusals above are the three the enum
        // declares -- a fourth variant added without a test here fails.
        let seen = [
            plan_for("vault.json").unwrap_err(),
            plan_for("").unwrap_err(),
            plan_export(
                &ExportRequest {
                    destination: PathBuf::from("C:\\cfg\\vault.json"),
                },
                Path::new("C:\\cfg"),
            )
            .unwrap_err(),
        ];
        assert!(seen.contains(&ExportRefusal::NoDirectory));
        assert!(seen.contains(&ExportRefusal::EmptyFileName));
        assert!(seen.contains(&ExportRefusal::IntoConfigDir));
    }

    /// A head that a real `encrypted_json` export starts with.
    fn envelope_head() -> Vec<u8> {
        br#"{"encrypted":true,"passwordProtected":false,"encKeyValidation_DO_NOT_EDIT":"2.abc"#
            .to_vec()
    }

    #[test]
    fn classify_decides_every_row_of_the_table() {
        let rows: Vec<(&str, RawExport, ExportOutcome)> = vec![
            (
                "exit 0, bytes on disk, encrypted envelope",
                RawExport {
                    exit_ok: true,
                    stderr: String::new(),
                    written: Ok(41_233),
                    head: envelope_head(),
                },
                ExportOutcome::Written,
            ),
            (
                "exit 0 but the file is empty",
                RawExport {
                    exit_ok: true,
                    stderr: String::new(),
                    written: Ok(0),
                    head: Vec::new(),
                },
                ExportOutcome::Unconfirmed,
            ),
            (
                "exit 0 and the SIZE CHECK ITSELF failed -- unknown, not zero, and never success",
                RawExport {
                    exit_ok: true,
                    stderr: String::new(),
                    written: Err("access denied reading the partial".to_string()),
                    head: envelope_head(),
                },
                ExportOutcome::Unconfirmed,
            ),
            (
                "exit 0 but the head is a PLAINTEXT export, not an encrypted envelope",
                RawExport {
                    exit_ok: true,
                    stderr: String::new(),
                    written: Ok(41_233),
                    head: br#"{"folders":[],"items":[{"name":"Bank","login":{"password":"hunter2"#
                        .to_vec(),
                },
                ExportOutcome::Unconfirmed,
            ),
            (
                "exit 0 but the head is a CSV, not JSON at all",
                RawExport {
                    exit_ok: true,
                    stderr: String::new(),
                    written: Ok(900),
                    head: b"folder,favorite,type,name,notes,login_uri,login_username".to_vec(),
                },
                ExportOutcome::Unconfirmed,
            ),
            (
                "the session went stale",
                RawExport {
                    exit_ok: false,
                    stderr: "You are not logged in.".to_string(),
                    written: Err("no such file".to_string()),
                    head: Vec::new(),
                },
                ExportOutcome::SessionInvalid,
            ),
            (
                "the vault relocked under us",
                RawExport {
                    exit_ok: false,
                    stderr: "Vault is locked.".to_string(),
                    written: Err("no such file".to_string()),
                    head: Vec::new(),
                },
                ExportOutcome::SessionInvalid,
            ),
            (
                "an ordinary failure keeps its own words",
                RawExport {
                    exit_ok: false,
                    stderr: "  EACCES: permission denied, open 'D:\\x.json'\n".to_string(),
                    written: Err("no such file".to_string()),
                    head: Vec::new(),
                },
                ExportOutcome::Failed("EACCES: permission denied, open 'D:\\x.json'".to_string()),
            ),
        ];

        // Positive control: the table is real and every row is decided, so a
        // `classify` that answered one thing forever could not pass by the
        // loop simply never running.
        assert!(
            rows.len() >= 8,
            "the table lost rows; every arm of `classify` needs one"
        );
        let mut ran = 0usize;
        for (why, raw, expected) in &rows {
            assert_eq!(classify(raw), *expected, "row: {why}");
            ran += 1;
        }
        assert_eq!(ran, rows.len(), "control: every row of the table ran");

        // And the outcomes are not all the same value, which a loop over a
        // table can otherwise hide.
        assert!(rows.iter().any(|(.., o)| *o == ExportOutcome::Written));
        assert!(rows.iter().any(|(.., o)| *o == ExportOutcome::Unconfirmed));
        assert!(rows
            .iter()
            .any(|(.., o)| *o == ExportOutcome::SessionInvalid));
        assert!(rows
            .iter()
            .any(|(.., o)| matches!(o, ExportOutcome::Failed(_))));
    }

    #[test]
    fn a_failed_size_check_is_not_a_zero_size() {
        // Stated on its own as well as in the table, because it is the one
        // arm whose whole purpose is to differ from the arm beside it: the
        // same exit code and the same head, and the ONLY difference is that
        // one knows the size and the other does not.
        let known = RawExport {
            exit_ok: true,
            stderr: String::new(),
            written: Ok(41_233),
            head: envelope_head(),
        };
        let unknown = RawExport {
            written: Err("the stat failed".to_string()),
            ..RawExport {
                exit_ok: true,
                stderr: String::new(),
                written: Ok(41_233),
                head: envelope_head(),
            }
        };
        assert_eq!(classify(&known), ExportOutcome::Written);
        assert_eq!(
            classify(&unknown),
            ExportOutcome::Unconfirmed,
            "an export whose size could not be checked must never be reported as written"
        );
    }

    fn a_plan() -> ExportPlan {
        plan_for("C:\\Users\\nobody\\Backups\\vault.json").expect("the fixture path plans")
    }

    #[test]
    fn the_args_carry_the_one_format_and_the_partial_file() {
        let args = export_args(&a_plan());
        assert_eq!(
            args,
            vec![
                "export",
                "--format",
                "encrypted_json",
                "--output",
                "C:\\Users\\nobody\\Backups\\vault.json.dw-partial",
            ]
        );
        assert!(
            !args.iter().any(|a| a == "--password"),
            "a password on argv is readable by every process on the desktop; `bw export` has no \
             --passwordenv, which is why this format is offered without one"
        );
        assert!(!args.iter().any(|a| a == "--organizationid"));
        assert!(
            !args.iter().any(|a| a == "csv" || a == "json" || a == "zip"),
            "no plaintext format may reach the CLI"
        );
    }

    /// The wiring pin. Every assertion below reads the **built `Command`** --
    /// `get_program`, `get_args`, `get_envs` -- and not the values handed to
    /// `export_command`. A version that accepted the plan and the token and
    /// then dropped either on the floor passes a test that checks its inputs
    /// and fails this one. Nothing is spawned.
    ///
    /// **The crate's `#[global_allocator]` probe deliberately does NOT back
    /// this up, and the reason is worth recording so nobody adds it later.**
    /// It backs up `send.rs`'s equivalent because there the secret has a
    /// `Zeroizing` home and reaching argv means reaching a plain
    /// `Vec<String>` that is freed in the clear. Here BOTH destinations are
    /// plain: `Command` stores its environment values in an ordinary
    /// `OsString` with no zeroizing anywhere, so the CORRECT implementation
    /// -- session in `BW_SESSION` and nowhere else -- hands the token back
    /// to the allocator in the clear exactly as the wrong one would. A probe
    /// would fire on both and so separate neither; it would be a second net
    /// with the same hole. The argv assertion above is the net that
    /// discriminates, which is why it reads `get_args()` on the built
    /// command rather than trusting that the token was merely handed over.
    #[test]
    fn the_built_command_carries_the_plan_on_argv_and_the_session_only_in_the_environment() {
        const TOKEN: &str = "a-fake-session-token-Zm9vYmFy";
        let plan = a_plan();

        // Unit tests never run `main`'s startup check, so `bw_command` would
        // otherwise refuse and leave every assertion below unreached. Record
        // a fake verified path first -- the same idiom `bw_path`'s and
        // `login_ui`'s tests already use, and safe to repeat because
        // `remember_verified_bw_exe` is first-wins: whichever test gets there
        // first, the recorded path is a `...\bw.exe` and no real binary is
        // involved. Nothing here spawns anything.
        crate::bw_path::remember_verified_bw_exe(PathBuf::from(r"C:\deskwarden-test\first\bw.exe"));
        let cmd = export_command(&plan, TOKEN)
            .expect("a verified path was just recorded, so `bw_command` must hand back a command");

        let program = cmd.get_program().to_string_lossy().to_lowercase();
        assert!(
            program.ends_with("bw.exe") && program.contains('\\'),
            "the program must be the absolute, signature-checked path `bw_command` hands back, \
             not a bare `bw` left to CreateProcess's search order: {program}"
        );

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--format" && w[1] == "encrypted_json"),
            "the built command must ask for encrypted_json: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--output" && Path::new(&w[1]) == plan.partial_path()),
            "the built command must be pointed at the PLAN's partial path, not at the final \
             path and not at a path it invented: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains(TOKEN)),
            "the session token reached argv, where any same-user process can read it: {args:?}"
        );

        let session: Vec<String> = cmd
            .get_envs()
            .filter(|(k, _)| k.to_string_lossy() == "BW_SESSION")
            .map(|(_, v)| v.unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            session,
            vec![TOKEN.to_string()],
            "BW_SESSION must be set exactly once, to the token that was passed in -- a command \
             that accepted the token and never used it would otherwise pass"
        );
    }

    // -----------------------------------------------------------------
    // The runner and the promotion.
    // -----------------------------------------------------------------

    /// A plan pointed inside `dir`, with a config directory that is nowhere
    /// near it so the refusal cannot interfere.
    fn plan_in(dir: &Path, name: &str) -> ExportPlan {
        plan_export(
            &ExportRequest {
                destination: dir.join(name),
            },
            Path::new("C:\\a-config-directory-far-from-any-temp-dir"),
        )
        .expect("a file inside a temp dir plans")
    }

    /// A runner that writes `body` where the plan says the CLI would, then
    /// observes it exactly as `run_cli` does. It starts no process.
    fn writing_runner(body: &'static [u8], exit_ok: bool, stderr: &'static str) -> ExportRunner {
        Arc::new(move |plan: &ExportPlan, _session: &str| {
            std::fs::write(plan.partial_path(), body).expect("the fake runner stages its own file");
            let (written, head) = observe_partial(plan);
            RawExport {
                exit_ok,
                stderr: stderr.to_string(),
                written,
                head,
            }
        })
    }

    /// A runner that writes nothing at all, as a CLI that died before opening
    /// its output file would leave things.
    fn barren_runner(exit_ok: bool, stderr: &'static str) -> ExportRunner {
        Arc::new(move |plan: &ExportPlan, _session: &str| {
            let (written, head) = observe_partial(plan);
            RawExport {
                exit_ok,
                stderr: stderr.to_string(),
                written,
                head,
            }
        })
    }

    fn envelope_body() -> &'static [u8] {
        br#"{"encrypted":true,"encKeyValidation_DO_NOT_EDIT":"2.abcdef","items":[]}"#
    }

    #[test]
    fn a_confirmed_export_is_promoted_to_the_name_the_user_picked() {
        let temp = TempDir::new("promote");
        let plan = plan_in(temp.path(), "vault.json");

        let outcome = run_export(&plan, "session", &writing_runner(envelope_body(), true, ""));

        assert_eq!(outcome, ExportOutcome::Written);
        assert_eq!(
            std::fs::read(plan.final_path()).expect("the archive is under the chosen name"),
            envelope_body(),
            "the promotion must move the staged bytes, not write something of its own"
        );
        assert!(
            !plan.partial_path().exists(),
            "the staging file must not survive a successful export"
        );
    }

    #[test]
    fn no_failure_leaves_a_partial_behind_and_none_of_them_touches_a_previous_backup() {
        // THE test of this module. Each row is a way for an export to not be
        // confirmed; in each, a previous good backup is already sitting under
        // the name the user picked, and it must come out byte-identical.
        const PREVIOUS: &[u8] = b"a previous, perfectly good encrypted backup";

        let rows: Vec<(&str, ExportRunner, ExportOutcome)> = vec![
            (
                "the CLI failed and said why",
                barren_runner(false, "EACCES: permission denied"),
                ExportOutcome::Failed("EACCES: permission denied".to_string()),
            ),
            (
                "the session went stale",
                barren_runner(false, "Vault is locked."),
                ExportOutcome::SessionInvalid,
            ),
            (
                "exit 0 and an EMPTY file -- a truncated export must never take the name",
                writing_runner(b"", true, ""),
                ExportOutcome::Unconfirmed,
            ),
            (
                "exit 0 and a PLAINTEXT document where an envelope was asked for",
                writing_runner(br#"{"items":[{"login":{"password":"hunter2"}}]}"#, true, ""),
                ExportOutcome::Unconfirmed,
            ),
            (
                "exit 0 and NO file at all, so the size check itself fails",
                barren_runner(true, ""),
                ExportOutcome::Unconfirmed,
            ),
        ];

        assert_eq!(rows.len(), 5, "the table lost rows");
        let mut ran = 0usize;
        for (why, runner, expected) in &rows {
            let temp = TempDir::new("failpath");
            let plan = plan_in(temp.path(), "vault.json");
            std::fs::write(plan.final_path(), PREVIOUS).expect("the previous backup is placed");

            assert_eq!(run_export(&plan, "session", runner), *expected, "row: {why}");

            // Absence, asserted rather than assumed.
            assert!(
                !plan.partial_path().exists(),
                "row: {why} -- the staging file was left in the user's chosen folder"
            );
            assert_eq!(
                std::fs::read(plan.final_path()).expect("the previous backup is still there"),
                PREVIOUS,
                "row: {why} -- A FAILED EXPORT OVERWROTE THE USER'S PREVIOUS BACKUP"
            );
            ran += 1;
        }
        assert_eq!(ran, rows.len(), "control: every row ran");
    }

    #[test]
    fn a_failure_with_no_previous_backup_leaves_the_chosen_name_unused() {
        // The other half of the row above: when nothing was there before,
        // nothing must be there after either -- not an empty file, not a
        // truncated one. A caller that then reports "exported to vault.json"
        // would otherwise be pointing at a file that does exist.
        let temp = TempDir::new("nothingbefore");
        let plan = plan_in(temp.path(), "vault.json");
        assert!(!plan.final_path().exists(), "precondition");

        let outcome = run_export(&plan, "session", &writing_runner(b"truncated", true, ""));

        assert_eq!(outcome, ExportOutcome::Unconfirmed);
        assert!(
            !plan.final_path().exists(),
            "the chosen name must not exist after an export that was never confirmed"
        );
        assert!(!plan.partial_path().exists());
    }

    #[test]
    fn a_promotion_that_cannot_happen_is_a_failure_and_still_clears_the_staging_file() {
        // A directory standing where the file belongs: the rename cannot
        // succeed, and the interesting part is what is left behind.
        let temp = TempDir::new("renamefail");
        let plan = plan_in(temp.path(), "occupied.json");
        std::fs::create_dir(plan.final_path()).expect("a directory takes the chosen name");

        let outcome = run_export(&plan, "session", &writing_runner(envelope_body(), true, ""));

        assert!(
            matches!(outcome, ExportOutcome::Failed(_)),
            "a promotion that did not happen must not be reported as Written: {outcome:?}"
        );
        assert!(
            !plan.partial_path().exists(),
            "the staging file must not be left beside the user's backups"
        );
        assert!(
            plan.final_path().is_dir(),
            "and whatever was already at the chosen name is untouched"
        );
    }

    #[test]
    fn the_observation_reads_the_staged_file_and_not_the_chosen_one() {
        // The mutation this exists for reads the head and the size off
        // `final_path`. With a previous backup sitting there, that version
        // answers with a perfectly good envelope for an export that wrote
        // nothing -- and `classify` would then confirm it.
        let temp = TempDir::new("whichfile");
        let plan = plan_in(temp.path(), "vault.json");
        std::fs::write(plan.final_path(), envelope_body()).expect("the previous backup");
        std::fs::write(plan.partial_path(), b"xy").expect("the staged file");

        let (written, head) = observe_partial(&plan);
        assert_eq!(written, Ok(2), "the size is the staged file's");
        assert_eq!(head, b"xy".to_vec(), "the head is the staged file's");

        // And with no staged file, the size check FAILS rather than reading
        // the one that is there.
        std::fs::remove_file(plan.partial_path()).expect("remove the staged file");
        let (written, head) = observe_partial(&plan);
        assert!(
            written.is_err(),
            "a missing staged file must be an unknown size, not the previous backup's size"
        );
        assert!(head.is_empty());
    }

    #[test]
    fn a_head_longer_than_the_sample_is_truncated_and_a_short_one_is_not() {
        let temp = TempDir::new("headlen");
        let plan = plan_in(temp.path(), "vault.json");
        let big = vec![b'z'; HEAD_BYTES * 3];
        std::fs::write(plan.partial_path(), &big).expect("write");
        let (written, head) = observe_partial(&plan);
        assert_eq!(written, Ok(big.len() as u64));
        assert_eq!(head.len(), HEAD_BYTES);

        std::fs::write(plan.partial_path(), b"abc").expect("write");
        let (_, head) = observe_partial(&plan);
        assert_eq!(head, b"abc".to_vec(), "a short file is not padded out");
    }

    // -----------------------------------------------------------------
    // The sweep.
    // -----------------------------------------------------------------

    fn backdate(path: &Path, by: Duration) {
        let when = SystemTime::now() - by;
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open for set_modified");
        file.set_modified(when).expect("backdate the file");
    }

    #[test]
    fn the_sweep_removes_old_partials_and_leaves_everything_else_alone() {
        let temp = TempDir::new("sweep");
        let dir = temp.path();

        let stale = dir.join("vault.json.dw-partial");
        let also_stale = dir.join("something-else.json.dw-partial");
        let fresh = dir.join("in-progress.json.dw-partial");
        let unrelated = dir.join("taxes.pdf");
        let old_unrelated = dir.join("last-years-backup.json");
        let bare = dir.join(".dw-partial");
        let dir_partial = dir.join("a-folder.dw-partial");

        for p in [&stale, &also_stale, &fresh, &unrelated, &old_unrelated, &bare] {
            std::fs::write(p, b"contents").expect("write the fixture");
        }
        std::fs::create_dir(&dir_partial).expect("a directory wearing the suffix");
        for p in [&stale, &also_stale, &unrelated, &old_unrelated, &bare] {
            backdate(p, Duration::from_secs(60 * 60));
        }

        let removed = sweep_stale_partials(dir, SystemTime::now(), STALE_PARTIAL_AGE);

        assert_eq!(removed, 2, "exactly the two aged partials");
        assert!(!stale.exists());
        assert!(!also_stale.exists());
        // The four absences that matter, each asserted, because "it did not
        // delete this" is invisible unless something says so.
        assert!(fresh.exists(), "a partial younger than the threshold may be an export still \
                                 running; deleting it would break a backup in progress");
        assert!(unrelated.exists(), "the user's own files live in this folder");
        assert!(old_unrelated.exists(), "age alone is not a reason; the suffix is");
        assert!(bare.exists(), "this crate never creates a bare suffix, so it is not ours");
        assert!(dir_partial.is_dir(), "a directory is never removed by a file sweep");
    }

    #[test]
    fn a_future_dated_partial_is_left_alone() {
        // A clock change or a file copied from another machine. Its age
        // cannot be established, and an age that cannot be established is
        // not an old age.
        let temp = TempDir::new("future");
        let ahead = temp.path().join("vault.json.dw-partial");
        std::fs::write(&ahead, b"contents").expect("write");
        backdate(&ahead, Duration::ZERO);
        let long_ago = SystemTime::now() - Duration::from_secs(60 * 60 * 24);

        assert_eq!(
            sweep_stale_partials(temp.path(), long_ago, STALE_PARTIAL_AGE),
            0
        );
        assert!(ahead.exists());
    }

    #[test]
    fn the_sweep_answers_zero_for_a_directory_that_is_not_there() {
        let temp = TempDir::new("nodir");
        let missing = temp.path().join("no-such-folder");
        assert_eq!(
            sweep_stale_partials(&missing, SystemTime::now(), STALE_PARTIAL_AGE),
            0,
            "a destination that vanished is not worth failing an export over"
        );
    }

    #[test]
    fn a_fresh_leftover_partial_is_not_promoted_by_an_export_that_wrote_nothing() {
        // REVIEW FINDING, in the shape the reviewer demonstrated it: a
        // `.dw-partial` left by a crash MINUTES ago is younger than
        // `STALE_PARTIAL_AGE`, so the sweep leaves it. It holds a valid
        // envelope head. The CLI then exits 0 without writing anything.
        // Before the unconditional delete, `observe_partial` stat'd and
        // sampled the leftover, `classify` answered `Written`, and the
        // promotion renamed a STRANGER'S half-finished archive over the
        // backup the user already had -- reported as a success.
        const PREVIOUS: &[u8] = b"the user's previous, perfectly good backup";
        let temp = TempDir::new("freshleftover");
        let plan = plan_in(temp.path(), "vault.json");
        std::fs::write(plan.final_path(), PREVIOUS).expect("the previous backup is placed");
        std::fs::write(plan.partial_path(), envelope_body()).expect("the crash leftover");
        // Young enough that the sweep must not take it, and the sweep is not
        // what this test is about.
        backdate(plan.partial_path(), Duration::from_secs(60));

        let outcome = run_export(&plan, "session", &barren_runner(true, ""));

        assert_eq!(
            outcome,
            ExportOutcome::Unconfirmed,
            "an export that wrote nothing was confirmed off an earlier run's leftover file"
        );
        assert_eq!(
            std::fs::read(plan.final_path()).expect("the previous backup is still there"),
            PREVIOUS,
            "A LEFTOVER PARTIAL WAS PROMOTED OVER THE USER'S BACKUP by an export that wrote \
             nothing at all"
        );
        assert!(
            !plan.partial_path().exists(),
            "the leftover survived the run as well as being read from it"
        );
    }

    #[test]
    fn a_fresh_leftover_partial_does_not_survive_into_a_real_export() {
        // The other direction, so the delete above cannot be satisfied by a
        // `run_export` that refuses everything: with a runner that really
        // writes, the export still succeeds and the bytes under the chosen
        // name are THIS run's, not the leftover's.
        const LEFTOVER: &[u8] =
            br#"{"encrypted":true,"encKeyValidation_DO_NOT_EDIT":"2.stale","items":[1]}"#;
        let temp = TempDir::new("leftoverthenreal");
        let plan = plan_in(temp.path(), "vault.json");
        std::fs::write(plan.partial_path(), LEFTOVER).expect("the crash leftover");
        backdate(plan.partial_path(), Duration::from_secs(60));

        assert_eq!(
            run_export(&plan, "session", &writing_runner(envelope_body(), true, "")),
            ExportOutcome::Written
        );
        assert_eq!(
            std::fs::read(plan.final_path()).expect("the archive is under the chosen name"),
            envelope_body(),
            "the promoted bytes are the leftover's, not this run's"
        );
    }

    #[test]
    fn the_runner_spawns_into_the_very_job_it_was_constructed_with() {
        // THE CRITICAL FINDING, held behaviourally rather than by a pin.
        //
        // Job membership itself cannot be observed without a real process,
        // and no test here may start one -- so the old guarantee was a source
        // pin over the `spawn_in_job` call. That pin was defeated by starving
        // its argument: `job.as_ref().as_ref().filter(|_| false)` in the
        // constructor left the pinned line untouched, the whole suite green
        // and no warning emitted, while every `bw` child -- a process holding
        // an unlocked vault -- was spawned outside the kill-on-close job and
        // could outlive a panic, a `process::exit` or a Task Manager kill.
        //
        // What is asserted is the VALUE: the job reaching the single accessor
        // the spawn reads from is, by pointer identity, the one the
        // constructor was handed. A starved constructor cannot satisfy it,
        // and neither can one that quietly substitutes a job of its own.
        //
        // `KillOnCloseJob::new` creates a kernel handle. It starts no
        // process, touches no file, opens no socket, and the handle is
        // dropped with nothing assigned to it.
        let job = KillOnCloseJob::new().expect("a job object is a handle, not a process");
        let held = Arc::new(Some(job));
        let given: &KillOnCloseJob = held.as_ref().as_ref().expect("the fixture holds one");

        let runner = CliExportRunner::new(Arc::clone(&held));
        let reaching = runner.job().expect(
            "the runner threw the job away between its constructor and the spawn, so an \
             export child holding an unlocked vault would not die with this process",
        );
        assert!(
            std::ptr::eq(given, reaching),
            "the job the spawn reads is not the job the runner was constructed with"
        );

        // And the absence is still expressible -- once, at the top, because
        // `KillOnCloseJob::new` can genuinely fail and `main.rs` holds the
        // `Arc<Option<..>>` that says so.
        assert!(
            CliExportRunner::new(Arc::new(None)).job().is_none(),
            "control: `no job at all` is still representable, so the assertion above is about \
             a job that was really there rather than about a type that cannot be empty"
        );
    }

    #[test]
    fn run_export_really_sweeps_on_the_way_in() {
        // An absence with a witness. Nothing in the outcome of an export says
        // whether the sweep ran, so a leftover from an earlier crash is put
        // in the folder first and its disappearance is the evidence.
        let temp = TempDir::new("sweepwired");
        let leftover = temp.path().join("an-earlier-crash.json.dw-partial");
        std::fs::write(&leftover, b"leftover").expect("write");
        backdate(&leftover, Duration::from_secs(60 * 60));

        let plan = plan_in(temp.path(), "vault.json");
        assert_eq!(
            run_export(&plan, "session", &writing_runner(envelope_body(), true, "")),
            ExportOutcome::Written
        );

        assert!(
            !leftover.exists(),
            "run_export never swept the destination directory"
        );
        assert!(plan.final_path().exists(), "control: the export itself still worked");
    }

    // -----------------------------------------------------------------
    // Source pins.
    //
    // Everything below asserts about this module's own text rather than its
    // behaviour, and each says why. The common reason: they are facts about
    // a CHILD PROCESS, and no test in this crate may spawn one. A fake
    // runner starts nothing, so job membership and the creation flags are
    // not observable from any test that could run here. Rather than write a
    // test that restates the source and calls itself behavioural, they are
    // labelled pins.
    // -----------------------------------------------------------------

    /// This file's own text, **above the test module only**, so a needle
    /// spelled out in a test below cannot satisfy the pin looking for it.
    fn code_under_test() -> String {
        // Normalised to LF once, so a needle written with `\n` matches this
        // file whether it is stored CRLF or LF.
        let whole = include_str!("vault_export.rs").replace("\r\n", "\n");
        let code = whole.split("#[cfg(test)]").next().unwrap().to_string();
        assert!(
            code.len() < whole.len(),
            "the test module marker was not found; the split did nothing"
        );
        code
    }

    #[test]
    fn the_source_pin_search_can_tell_present_from_absent() {
        // The positive control every pin below depends on. Without it, a
        // `code_under_test` that answered an empty string would make each
        // `!contains` pin pass while asserting nothing whatsoever.
        let code = code_under_test();
        assert!(code.contains("pub fn real_runner(job: Arc<Option<KillOnCloseJob>>)"));
        assert!(!code.contains("no such line appears anywhere in this module"));
        // And the split really did drop the tests: this function's own name
        // is below the marker.
        assert!(!code.contains("the_source_pin_search_can_tell_present_from_absent"));
    }

    #[test]
    fn the_export_child_is_spawned_into_the_kill_on_close_job() {
        // PIN. The child holds an unlocked vault. Job membership is what
        // makes it die with this process however this process dies, and it
        // is a property of a real process: a fake runner has none.
        let code = code_under_test();
        assert_eq!(
            code.matches("crate::job_object::spawn_in_job(job, command)").count(),
            1,
            "the export child is no longer spawned into the job exactly once, so a crash or a \
             Task Manager kill could leave a `bw` running with the session token in its \
             environment. Counted rather than merely required, because a second spawn added \
             beside the pinned one satisfies a presence-only needle by construction"
        );
        assert_eq!(
            code.matches("run_cli(plan, session, self.job())").count(),
            1,
            "the runner no longer passes its OWN job to the spawn. This is the pin that the \
             finding was about: pinning the `spawn_in_job` line above says nothing about the \
             value flowing into it, and the behavioural assertion in \
             `the_runner_spawns_into_the_very_job_it_was_constructed_with` is what holds the \
             value. This says which line carries it"
        );
        assert!(
            !code.contains(".spawn()"),
            "a direct spawn bypasses the job entirely"
        );
    }

    #[test]
    fn the_job_spawn_still_re_applies_the_no_window_flag() {
        // PIN, and it reads ANOTHER file: `creation_flags` REPLACES the flags
        // a command holds rather than adding to them, so `spawn_in_job`
        // setting CREATE_SUSPENDED would silently drop the CREATE_NO_WINDOW
        // that `bw_command` set -- and every export would flash a console
        // window. Nothing this module owns can hold that guarantee, and no
        // test here may spawn the process that would show the symptom.
        let job_source = include_str!("job_object.rs").replace("\r\n", "\n");
        assert!(
            job_source.contains("pub fn spawn_in_job("),
            "control: the file being searched is the one that defines the spawn"
        );
        assert!(
            job_source.contains("creation_flags(crate::bw_path::CREATE_NO_WINDOW"),
            "spawn_in_job stopped re-applying CREATE_NO_WINDOW, so every export would flash a \
             console window on screen"
        );
    }

    #[test]
    fn the_streams_are_captured_and_stdin_is_closed() {
        // PIN. An inherited stream attaches the child to a console this app
        // does not own; there is no way to observe a child's inherited
        // handles from a test that never starts one.
        let code = code_under_test();
        for needle in [
            ".stdin(Stdio::null())",
            ".stdout(Stdio::piped())",
            ".stderr(Stdio::piped())",
        ] {
            assert!(code.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn the_real_runner_points_the_cli_at_the_partial_through_export_command() {
        // PIN on the wiring, not on the behaviour: `export_command` is
        // already tested for real above (argv, program, BW_SESSION), and this
        // is the line that says the runner uses it rather than assembling an
        // invocation of its own. A runner that built its own would be caught
        // by `bw_path`'s crate-wide spawn guard too -- this pin says which
        // line it is.
        let code = code_under_test();
        assert!(code.contains("match export_command(plan, session)"));
    }
}

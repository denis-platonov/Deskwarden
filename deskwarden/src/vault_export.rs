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

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bw_path::bw_command;

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
#[derive(Debug, PartialEq, Eq)]
pub struct ExportPlan {
    /// Where the finished archive belongs, if it turns out to be finished.
    pub final_path: PathBuf,
    /// Where the CLI is actually pointed: `final_path` + [`PARTIAL_SUFFIX`].
    pub partial_path: PathBuf,
    /// Always [`EXPORT_FORMAT`]. A field rather than a constant at the call
    /// site so the argv builder cannot drift from what was planned.
    pub format: &'static str,
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

    let mut partial = req.destination.clone().into_os_string();
    partial.push(PARTIAL_SUFFIX);

    Ok(ExportPlan {
        final_path: req.destination.clone(),
        partial_path: PathBuf::from(partial),
        format: EXPORT_FORMAT,
    })
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
        plan.format.to_string(),
        "--output".to_string(),
        plan.partial_path.to_string_lossy().into_owned(),
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
    fn a_full_file_path_plans_a_partial_beside_it() {
        let plan = plan_for("C:\\Users\\nobody\\Backups\\vault.json").expect("a plain path plans");
        assert_eq!(
            plan.final_path,
            PathBuf::from("C:\\Users\\nobody\\Backups\\vault.json")
        );
        assert_eq!(
            plan.partial_path,
            PathBuf::from("C:\\Users\\nobody\\Backups\\vault.json.dw-partial"),
            "the partial is the final path with the suffix appended -- same directory, so the \
             promotion is a rename within one volume and not a copy"
        );
        assert_eq!(plan.format, "encrypted_json");
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
                .any(|w| w[0] == "--output" && Path::new(&w[1]) == plan.partial_path),
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
}

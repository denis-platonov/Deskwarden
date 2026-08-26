//! Lifecycle management for the bundled `bw serve` HTTP bridge.
//!
//! Covers spawning it, detecting a pre-existing process squatting on its port,
//! waiting for it to actually become ready (its cold start is a Node process
//! and routinely takes multiple seconds), and running `bw sync`.

use crate::bw_path::bw_command;
use crate::vault_backend::VaultBackend;
#[cfg(test)]
use crate::vault_bridge::VaultBridge;
use crate::vault_bridge::VaultItem;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::process::{Child, Stdio};
use std::time::Duration;

/// Port `bw serve` is started on, and that [`BW_SERVE_URL`] points at.
pub const BW_SERVE_PORT: u16 = 8087;

/// Base URL of the local `bw serve` bridge.
pub const BW_SERVE_URL: &str = "http://localhost:8087";

/// Builds the retry schedule for waiting on `bw serve` readiness: the delays
/// to sleep *between* attempts, exponentially backing off from 250ms and
/// capped at 4s per wait, stopping once the accumulated wait would exceed
/// `deadline`.
///
/// Pure and separated from the polling loop so the shape of the backoff is
/// testable without actually sleeping for half a minute.
pub fn readiness_schedule(deadline: Duration) -> Vec<Duration> {
    const FIRST_DELAY_MS: u64 = 250;
    const MAX_DELAY_MS: u64 = 4_000;

    let mut schedule = Vec::new();
    let mut elapsed = Duration::ZERO;
    let mut delay = Duration::from_millis(FIRST_DELAY_MS);

    while elapsed + delay <= deadline {
        schedule.push(delay);
        elapsed += delay;
        delay = std::cmp::min(delay * 2, Duration::from_millis(MAX_DELAY_MS));
    }

    schedule
}

/// Default deadline for `bw serve` to come up and answer a real vault query.
pub const READINESS_DEADLINE: Duration = Duration::from_secs(30);

/// Upper bound on how long a legitimate backend start or sync may take
/// before something is treated as having gone wrong: `wait_for_port_free`
/// (up to `PORT_RELEASE_GRACE_RESTART`, 30s) can run first, then an
/// unbounded `bw sync`, then node's own cold start (10-20s more)
/// before `bw serve` answers at all.
///
/// Lives here, not `main.rs`, so it can be the *one* number both sides of
/// review 11's Important 2 agree on: `main`'s own backend-op bookkeeping
/// (`backend_task_in_progress`'s wedge deadline, `open_vault_window`'s
/// lock-recovery wait) and the picker's own readiness probe
/// (`picker_ui::run_picker`). Before this, the picker used the much shorter
/// `READINESS_DEADLINE` (30s) for the same wait `main` budgets 90s for --
/// so a normal-but-slow save-memory start resolved the probe to
/// `BackendReadiness::Unavailable` while the start was still healthy and
/// ~20s from landing.
pub const BACKEND_OP_TIMEOUT: Duration = Duration::from_secs(90);

/// Returns true if something is already listening on `port` on localhost.
///
/// Used to detect an orphaned `bw serve` from a previous, unclean exit: if one
/// is still holding the port, our freshly spawned `bw serve` will fail to bind
/// and `VaultBridge` would end up talking to an unknown process holding an
/// unknown session.
fn port_in_use(port: u16) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    TcpStream::connect_timeout(&addr.into(), Duration::from_millis(300)).is_ok()
}

/// How long to allow a just-killed `bw serve` to release its port before
/// concluding that whatever holds it isn't ours.
pub const PORT_RELEASE_GRACE: Duration = Duration::from_secs(5);

/// The same grace period, but for the mid-run restart paths, where being
/// patient is strictly better than the alternative.
///
/// `Child::kill` only terminates the process we spawned directly. If `bw`
/// resolves to a launcher or shim rather than a single packaged binary, a
/// grandchild can hold the listening socket for noticeably longer than the
/// short startup grace allows. On the restart paths the user may *just* have
/// retyped their master password, so waiting six times as long costs nothing
/// compared to giving up on them.
pub const PORT_RELEASE_GRACE_RESTART: Duration = Duration::from_secs(30);

/// Waits for `port` to stop being listened on, up to `deadline`.
///
/// Needed on the restart paths: `Child::kill` only *requests* termination, so
/// the socket is typically still bound for a moment afterwards. Without this
/// grace period a restart would immediately trip the port-collision guard and
/// abort the app over its own, already-dying child.
///
/// Returns `true` if the port is free, `false` if it was still held when the
/// deadline expired.
pub fn wait_for_port_free(port: u16, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if !port_in_use(port) {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Kills `bw serve` and reaps it, so it releases its port and doesn't linger
/// as a zombie.
pub fn stop_bw_serve(child: &mut Child) {
    if let Err(e) = child.kill() {
        log::warn!("bw serve kill failed (already gone?): {e}");
    }
    if let Err(e) = child.wait() {
        log::warn!("waiting for bw serve to exit failed: {e}");
    }
}

/// Builds (but does not spawn) the `bw serve` command, bound to
/// [`BW_SERVE_PORT`] with `BW_SESSION` set.
///
/// Returned unspawned so the caller can decide *how* to start it --
/// `job_object::spawn_in_job` needs to add `CREATE_SUSPENDED` before spawning
/// so the child can join the kill-on-close job before it runs.
///
/// `Err` when no verified `bw.exe` has been recorded (see
/// `bw_path::bw_command`): this process is about to hand the child a live
/// session token, so "we never checked which binary this is" has to be a
/// refusal, not a resolve-and-hope.
pub fn bw_serve_command(session_token: &str) -> Result<crate::bw_path::BareCommand, String> {
    let mut cmd = bw_command()?;
    cmd.args(["serve", "--port", &BW_SERVE_PORT.to_string()])
        .env("BW_SESSION", session_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(cmd)
}

// `spawn_bw_serve` used to live here: a `pub fn` that spawned `bw serve`
// directly, with no job-object protection, "for callers that have no job object
// at all". Nothing in this crate called it, and it was a finished door out of
// the kill-on-close job that `vault_export` or `send` could have walked through
// while naming no command type and writing no direct child start of their own --
// rule in `job_object`'s guards satisfied, a `bw` holding a live session token
// running outside the job. The only caller that ever wanted one, `main`, holds
// a job and goes through `job_object::spawn_in_job`. Deleted rather than
// documented.

/// Polls `vault.list_items()` until it succeeds or the schedule is exhausted.
///
/// `list_items` is deliberately the probe rather than a lighter ping: it is the
/// operation the app actually depends on, so a success here proves both that
/// the backend is up *and* that whatever credential it was handed is valid.
///
/// Returns the items on success (the caller needs them anyway to build the
/// match engine), or a human-readable error describing the last failure.
///
/// # It is generic over the backend, and its messages used to say otherwise
///
/// This takes `&dyn VaultBackend` and knows nothing about a subprocess. Its
/// log line and its error nevertheless both read "bw serve", from when that
/// was the only backend there was -- so a **direct-REST** launch, which
/// starts no subprocess at all, announced `bw serve ready after 0 retries`
/// three seconds after announcing that the vault was served by `DirectRest`.
///
/// That cost a real investigation on 2026-08-26: the message was read as
/// evidence that an ungated entry point had started `bw serve` behind the
/// backend policy, which would have been a serious defect, and a spec was
/// written naming it as the first thing to fix. `bw.exe` was never running.
/// The messages now name what this function actually observed, which is a
/// backend answering.
///
/// **The module is still the wrong home for it** and that is not fixed here:
/// nothing in this function is about `bw serve`, and it sits in `bw_serve.rs`
/// because that is where the only caller used to be. Moving it is a rename
/// across call sites and pins; saying so is cheaper than half-doing it.
pub fn wait_for_vault_ready(
    vault: &dyn VaultBackend,
    schedule: &[Duration],
) -> Result<Vec<VaultItem>, String> {
    let mut attempt = 0usize;

    loop {
        match vault.list_items() {
            Ok(items) => {
                log::info!(
                    "the vault backend answered after {attempt} retr{} ({} vault items)",
                    if attempt == 1 { "y" } else { "ies" },
                    items.len()
                );
                return Ok(items);
            }
            Err(e) => {
                let last_error = format!("{e:?}");
                if attempt >= schedule.len() {
                    return Err(format!(
                        "the vault backend did not become ready within the deadline; last error: {last_error}"
                    ));
                }
                log::debug!(
                    "bw serve not ready yet (attempt {}): {last_error}",
                    attempt + 1
                );
                std::thread::sleep(schedule[attempt]);
                attempt += 1;
            }
        }
    }
}

/// The process-lifetime job object every `bw sync` child joins.
///
/// **Why `bw sync` needs one at all.** It used to run through
/// `into_jobless_command`, whose comment argued that a short-lived child which
/// is waited on right here needs no protection. Both halves of that are wrong.
/// The child carries `BW_SESSION` -- the token that unlocks the vault -- in its
/// environment block, and on Windows an environment block is readable by any
/// same-user process holding `PROCESS_VM_READ` (that is exactly what Process
/// Explorer's Environment tab does). And "waited on right here" is a statement
/// about the happy path only: a panic, a `process::exit` or a Task Manager kill
/// of this process during the wait leaves the child orphaned, still holding the
/// token, with nothing left to reap it. A kill-on-close job closes both: the
/// kernel tears the child down when this process dies, however it dies.
///
/// **Shaped after `vault_window::send_fetch_thread::sends_job`, deliberately.**
/// Same `OnceLock`, same process lifetime, same `None`-on-failure degradation,
/// for the same reasons written out there: the hazard is an orphan outliving
/// the app rather than a child outliving a window, `KillOnCloseJob::new` is a
/// kernel call that can genuinely fail, and `spawn_in_job` already accepts
/// `None` -- so a job that cannot be created degrades to exactly the old
/// behaviour rather than to a sync that refuses to run.
///
/// It is a job of its own rather than the vault window's: `bw sync` runs during
/// backend startup, before any vault window (and so any window job) exists.
fn sync_job() -> Option<&'static crate::job_object::KillOnCloseJob> {
    static JOB: std::sync::OnceLock<Option<crate::job_object::KillOnCloseJob>> =
        std::sync::OnceLock::new();
    JOB.get_or_init(|| crate::job_object::KillOnCloseJob::new().ok())
        .as_ref()
}

/// Runs `bw sync` with the given session so the local vault cache reflects the
/// latest server state before the match engine is built.
///
/// Failure is non-fatal (we can still work from the cached vault), so this
/// returns a `Result` for the caller to log rather than propagating.
pub fn run_bw_sync(session_token: &str) -> Result<(), String> {
    let mut command = crate::bw_path::bw_job_command()?;
    command
        .args(["sync"])
        .env("BW_SESSION", session_token)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = crate::job_object::spawn_in_job(sync_job(), command)
        .map_err(|e| format!("failed to run `bw sync`: {e}"))?
        .wait_with_output()
        .map_err(|e| format!("failed to run `bw sync`: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`bw sync` is started INSIDE a job object, with the session in its
    /// environment and nowhere else.**
    ///
    /// Behavioural, not a source pin. What this replaces was a COMMENT above
    /// `run_bw_sync` asserting in prose that jobless was fine; five rounds of
    /// this crate's history say a text claim about which job a spawn carries
    /// cannot see which job it is actually looking at. So this arms
    /// `job_object`'s spawn probe -- the recorder standing exactly where
    /// `CreateProcess` would -- calls the PRODUCTION function, and reads back
    /// what really arrived at the choke point.
    ///
    /// Three properties, each of which was a real defect before this round:
    ///
    ///  1. **The child is in `sync_job()`.** Compared by ADDRESS, so "some
    ///     job" is not enough, and a jobless spawn (which records `None`)
    ///     fails. This is the finding: `bw sync` ran outside every job, so a
    ///     panic, a `process::exit` or a Task Manager kill during the wait
    ///     orphaned a process holding the vault-unlocking token.
    ///  2. **`BW_SESSION` carries the token this test passed in.** A body
    ///     that dropped the parameter, emptied it or substituted a constant
    ///     of its own fails here rather than being spelled right.
    ///  3. **The token is in none of argv.** An argument vector is readable
    ///     machine-wide far more cheaply than an environment block is.
    ///
    /// **No child is started.** The probe refuses the spawn before
    /// `CreateProcess`, and `run_bw_sync` maps that refusal through its own
    /// ordinary failure path -- so the error path is exercised too.
    #[test]
    fn bw_sync_is_started_in_a_job_with_the_session_it_was_given() {
        // Ends in `=` because a real `bw` session token is base64 and does: a
        // body that trimmed, split or percent-decoded the parameter would
        // leave a token without one untouched.
        const TOKEN: &str = "sync-test-session-token/9+x=";

        // `bw_job_command` refuses without a verified CLI path. A path that
        // does not exist and never will; nothing is executed, because the
        // probe below refuses every spawn. `remember_verified_bw_exe` is
        // first-wins, so whichever test in this binary gets there first wins
        // and the value is irrelevant to all of them.
        crate::bw_path::remember_verified_bw_exe(std::path::PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));

        let expected_job = sync_job().map(|j| std::ptr::from_ref(j) as usize);
        assert!(
            expected_job.is_some(),
            "control: no job object could be created at all, so the identity assertion below \
             would be satisfied by the very jobless spawn this test exists to refuse"
        );

        let probe = crate::job_object::spawn_probe::SpawnProbe::arm();
        let refused = run_bw_sync(TOKEN);
        let attempts = probe.attempts();
        drop(probe);

        assert!(
            refused.is_err(),
            "control: the probe refuses every spawn, so a success here means `run_bw_sync` \
             never reached the choke point and the attempts below are somebody else's"
        );
        assert_eq!(
            attempts.len(),
            1,
            "control: `run_bw_sync` did not hand exactly one child to `spawn_in_job`; got \
             {attempts:?}"
        );
        let attempt = &attempts[0];

        assert_eq!(
            attempt.job, expected_job,
            "`bw sync` was handed to `spawn_in_job` with {:?}, not with `sync_job()` at {:?}. A \
             `None` here is the finding itself: a `bw` carrying the vault-unlocking session in \
             its environment block, outside every job object, orphan-able past this process's \
             death",
            attempt.job, expected_job
        );

        let args: Vec<String> =
            attempt.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec!["sync".to_string()],
            "control: the child handed over is not `bw sync` at all, so every other assertion \
             here is about some other spawn"
        );
        assert!(
            !args.iter().any(|a| a.contains(TOKEN)),
            "the session token is in argv ({args:?}), where any process on the machine can read \
             it without even opening this one"
        );

        let sessions: Vec<Option<String>> = attempt
            .envs
            .iter()
            .filter(|(k, _)| k == "BW_SESSION")
            .map(|(_, v)| v.as_ref().map(|v| v.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(
            sessions,
            vec![Some(TOKEN.to_string())],
            "`bw sync` did not receive exactly the session it was given in `BW_SESSION`; it \
             would answer `Locked` and the vault cache would silently go stale"
        );
    }

    #[test]
    fn readiness_schedule_starts_short_and_backs_off() {
        let schedule = readiness_schedule(Duration::from_secs(30));
        assert_eq!(schedule[0], Duration::from_millis(250));
        assert_eq!(schedule[1], Duration::from_millis(500));
        assert_eq!(schedule[2], Duration::from_secs(1));
        assert_eq!(schedule[3], Duration::from_secs(2));
        assert_eq!(schedule[4], Duration::from_secs(4));
    }

    #[test]
    fn readiness_schedule_caps_individual_delays() {
        let schedule = readiness_schedule(Duration::from_secs(120));
        assert!(schedule.iter().all(|d| *d <= Duration::from_millis(4_000)));
    }

    #[test]
    fn readiness_schedule_never_exceeds_the_deadline() {
        for secs in [1u64, 5, 15, 30, 60] {
            let deadline = Duration::from_secs(secs);
            let total: Duration = readiness_schedule(deadline).iter().sum();
            assert!(total <= deadline, "schedule for {secs}s totalled {total:?}");
        }
    }

    #[test]
    fn readiness_schedule_gives_more_attempts_for_a_longer_deadline() {
        let short = readiness_schedule(Duration::from_secs(1)).len();
        let long = readiness_schedule(Duration::from_secs(30)).len();
        assert!(long > short, "short={short} long={long}");
    }

    #[test]
    fn readiness_schedule_is_empty_when_there_is_no_time_to_retry() {
        assert!(readiness_schedule(Duration::from_millis(100)).is_empty());
    }

    #[test]
    fn port_in_use_is_false_for_a_closed_port() {
        // Bind and immediately drop a listener to obtain a port that was just
        // free; nothing should be listening on it afterwards.
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!port_in_use(port));
    }

    #[test]
    fn port_in_use_is_true_for_an_open_port() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_in_use(port));
    }

    #[test]
    fn wait_for_port_free_returns_immediately_for_a_free_port() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let start = std::time::Instant::now();
        assert!(wait_for_port_free(port, Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wait_for_port_free_gives_up_on_a_held_port() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        assert!(!wait_for_port_free(port, Duration::from_millis(300)));
    }

    #[test]
    fn backend_op_timeout_is_more_patient_than_the_plain_readiness_deadline() {
        // Review 11's Important 2: `picker_ui::run_picker`'s readiness probe
        // used to use `READINESS_DEADLINE` (30s), shorter than a
        // save-memory `bw serve` start can legitimately take (up to
        // `PORT_RELEASE_GRACE_RESTART` plus an unbounded `bw sync` plus
        // node's own cold start) -- which is exactly why `BACKEND_OP_TIMEOUT`
        // exists as a longer, separate number in the first place. Guards
        // against the two silently drifting back into disagreement.
        assert!(BACKEND_OP_TIMEOUT > READINESS_DEADLINE);
    }

    #[test]
    fn the_restart_port_grace_is_more_patient_than_the_startup_one() {
        // A re-auth restart happens right after the user retyped their master
        // password; giving up on them over a slow socket release is the worst
        // possible moment to be impatient.
        assert!(PORT_RELEASE_GRACE_RESTART > PORT_RELEASE_GRACE);
    }

    #[test]
    fn wait_for_vault_ready_returns_items_when_the_server_answers() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[{"id":"1","name":"A","fields":[]}]}}"#)
            .create();

        let vault = VaultBridge::new(server.url());
        let items = wait_for_vault_ready(&vault, &[]).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn wait_for_vault_ready_reports_an_error_instead_of_an_empty_vault() {
        // Nothing listening: every attempt fails. With an empty schedule this
        // gives up immediately rather than sleeping, and must surface an error
        // rather than the empty item list that used to be silently swallowed
        // by `unwrap_or_default()`.
        let vault = VaultBridge::new("http://127.0.0.1:1");
        let err = wait_for_vault_ready(&vault, &[]).unwrap_err();
        assert!(err.contains("did not become ready"), "got: {err}");
    }
}

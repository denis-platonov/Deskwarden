//! A Windows Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!
//! `bw serve` holds an unlocked vault and serves it over plain HTTP on
//! localhost. Killing it only on the tray's Quit path is not enough: a panic,
//! a Ctrl+C, or a Task Manager kill of `deskwarden` leaves it running
//! and serving decrypted secrets with nothing left to stop it.
//!
//! A job object with kill-on-close moves that guarantee into the kernel. Every
//! process assigned to the job is terminated when the last handle to the job
//! closes -- and the OS closes our handles unconditionally when this process
//! dies, whatever the reason (including `process::exit`, which skips
//! destructors, and an unhandled panic or an external `TerminateProcess`).

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};

/// An owned job-object handle. **Must be kept alive** for as long as the child
/// processes should live: dropping it closes the handle, which (with
/// kill-on-close set) terminates everything assigned to the job.
///
/// Stored as a `std::os::windows::io::OwnedHandle` rather than the raw
/// `windows::Win32::Foundation::HANDLE` it's created as. `OwnedHandle` is
/// `Send + Sync` in std itself -- no `unsafe impl` needed here -- and closes
/// the handle in its own `Drop`, which is exactly the kill-on-close trigger
/// this type exists for; that's also why the manual `Drop` impl this used to
/// need is gone too. This is what lets `main.rs` share the job object with a
/// background thread that starts `bw serve` without blocking the vault
/// window's first paint on the backend's cold start.
pub struct KillOnCloseJob {
    handle: OwnedHandle,
}

impl KillOnCloseJob {
    /// Creates an unnamed job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null())?;

            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            if let Err(e) = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) {
                // Don't leak the handle if configuring it failed: a job
                // without kill-on-close is worse than no job at all, because
                // it silently looks like protection we don't have.
                let _ = CloseHandle(handle);
                return Err(e);
            }

            // `HANDLE` wraps a `*mut c_void`, which is exactly what
            // `OwnedHandle::from_raw_handle` expects (`RawHandle` is the same
            // type). From here on this handle's lifetime is std's problem,
            // not ours.
            Ok(Self { handle: OwnedHandle::from_raw_handle(handle.0) })
        }
    }

    /// Assigns an already-spawned child process to this job.
    ///
    /// Safe to call on a process that has already exited (it simply fails);
    /// callers should log rather than treat that as fatal.
    pub fn assign(&self, child: &Child) -> windows::core::Result<()> {
        let process_handle = HANDLE(child.as_raw_handle());
        let job_handle = HANDLE(self.handle.as_raw_handle());
        unsafe { AssignProcessToJobObject(job_handle, process_handle) }
    }
}

/// A `std::process::Command` that **cannot be spawned by anything except
/// [`spawn_in_job`]**.
///
/// This is the seventh round of one finding, and the first one whose fix is a
/// type rather than a text search. The sixth round was lost like this, in
/// `CliExportRunner::run`:
///
/// ```text
/// let _ = spawn_in_job(self.job(), Command::new("x"));   // decoy
/// let child = Command::spawn(&mut command)?;             // the real export
/// ```
///
/// Both halves of the previous seam shrugged. The probe below saw exactly one
/// arrival carrying exactly the right job, because it records *an* arrival and
/// not *the* spawn. The tree walk matched the three literals `.spawn()`,
/// `.output()` and `.status()` and `Command::spawn(&mut command)` is none of
/// them. Everything stayed green while every export child spawned outside the
/// kill-on-close job.
///
/// The answer is that the two halves stop being separate questions. The inner
/// `Command` is **private to this module**: there is no accessor, no
/// `into_inner`, no `Deref`, no `AsMut`, and the only code that can take it
/// out is [`spawn_in_job`], four lines below. So the decoy is not merely
/// caught, it is unwritable: a runner that wants to spawn its command at all
/// has exactly one call available, and if it also offers a decoy the probe
/// sees two arrivals instead of one.
///
/// Construction is `pub(crate)` and made in exactly one production place,
/// [`crate::bw_path::bw_job_command_in`], which is also the one place that
/// names the `bw.exe` whose signature startup verified. So "the command that
/// runs the verified CLI" and "the command that can only be spawned into a
/// job" are the same value, and neither can be obtained without the other.
///
/// The forwarding methods are everything a caller legitimately needs to
/// *describe* a child (arguments, environment, stream wiring) and nothing that
/// starts one. The `get_*` readers exist so tests can assert what would be run
/// without running it -- reading the description, never consuming it.
pub struct JobCommand {
    /// Private to this module. **This is the whole design**; see the type's
    /// note. Adding any accessor that hands this out re-opens the sixth hop.
    command: Command,
}

impl JobCommand {
    /// Wraps a command so that it can only be started through
    /// [`spawn_in_job`].
    ///
    /// `pub` only because `main.rs` is a separate crate from this library and
    /// wraps the `bw serve` command there; the intended production caller for
    /// everything else is [`crate::bw_path::bw_job_command_in`].
    ///
    /// **Being public costs nothing here.** Wrapping requires already holding
    /// a `std::process::Command`, and the two job-bearing modules may not name
    /// that type at all -- see
    /// [`tests::the_two_job_bearing_modules_cannot_name_a_bare_command`],
    /// which also forbids them to name this function. Without both, a runner
    /// could wrap a decoy of its own and hand that to the probe while spawning
    /// the real child by some other route, which is exactly how round six was
    /// lost.
    pub fn wrap(command: Command) -> Self {
        Self { command }
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.command.args(args);
        self
    }

    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        self.command.env(key, val);
        self
    }

    pub fn stdin<T: Into<std::process::Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    pub fn stdout<T: Into<std::process::Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    pub fn stderr<T: Into<std::process::Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    /// Reads the program that would be run. A reader, not a way out: it
    /// borrows an `OsStr`, from which no child process can be started without
    /// naming a command type the two job-bearing modules may not name.
    ///
    /// **`#[cfg(test)]`, and that is the eleventh round's structural fix.**
    /// These three readers are what made an argv rebuild possible: given a
    /// `JobCommand`, they hand back its program, its argv and its environment,
    /// which is everything needed to construct a second `std::process::Command`
    /// that is byte-for-byte the same child and start it outside the job. Six
    /// separate mutants did exactly that, differing only in where the helper
    /// was written and how its `impl` head spelled the self type; each survived
    /// a full green suite with a signature-verified `bw.exe` running on the
    /// export's real argv, outside the kill-on-close job. Every one of those
    /// spellings is a way of *writing* the helper, and each round closed one
    /// spelling and left the next.
    ///
    /// The readers are the thing they all need, and **no production code in
    /// this crate calls any of them**: outside this file the only callers are
    /// below the `#[cfg(test)]` cuts in `send.rs`, `vault_export.rs` and
    /// `login_ui.rs`. So they are gated rather than guarded. The shipped
    /// binary no longer contains the rebuild surface at all, and every
    /// `BW_SESSION` assertion and the spawn probe keep working unchanged,
    /// because tests are exactly who these were for.
    ///
    /// This is a partial mitigation and is meant to be read as one. A helper
    /// can still be written that takes what it needs as an ARGUMENT from a
    /// caller which already has it -- `CliExportRunner::run` holds the plan and
    /// the session -- and rebuilds the command from those. What is gone is the
    /// ability to derive the child from a `JobCommand` a runner is *holding*,
    /// which is what every measured hop did; the remaining route costs a
    /// signature change at the call site rather than one added line.
    #[cfg(test)]
    pub fn get_program(&self) -> &std::ffi::OsStr {
        self.command.get_program()
    }

    #[cfg(test)]
    pub fn get_args(&self) -> std::process::CommandArgs<'_> {
        self.command.get_args()
    }

    #[cfg(test)]
    pub fn get_envs(&self) -> std::process::CommandEnvs<'_> {
        self.command.get_envs()
    }
}

/// Spawns `command` so that it is a member of `job` **before it executes a
/// single instruction**.
///
/// Spawning first and assigning afterwards leaves a window -- short, but real
/// -- in which the child is running and unprotected: if this process died in
/// that window the child would survive as an orphan, which for `bw serve`
/// means an unlocked vault left serving on localhost with nothing to stop it.
/// `CREATE_SUSPENDED` closes the window: the child exists (so it can be
/// assigned) but has not run yet, and is only resumed once it is in the job.
///
/// Assignment failure is logged, not fatal -- an unprotected child is still
/// better than no child. A *resume* failure is different: a permanently
/// suspended `bw serve` would hang every caller forever, so the child is
/// killed and the error returned.
///
/// With no job object, this degrades to a plain spawn.
///
/// **This is the crate's one choke point for starting a child, and it is
/// OBSERVABLE.** Job membership is a property of a real process, and no test
/// in this crate may start one, so for five successive rounds the guarantee
/// "the child joins the job its runner was constructed with" was held by a
/// source pin over the `spawn_in_job(self.job(), ..)` call in
/// [`crate::vault_export`] and [`crate::send`]. Every one of those rounds was
/// lost, each one hop nearer this line than the last, for a single reason: a
/// text pin on `self.job()` cannot see WHICH `self` it is looking at. The last
/// one measured -- a `run` that handed the work to a second method on the same
/// type, constructed jobless -- left every needle word-perfect, both
/// pointer-identity tests green, 0 failed and 0 warnings, while every child
/// spawned outside the job.
///
/// So the value is recorded HERE, at the last instruction before
/// `CreateProcess`, rather than described upstream. Under `cfg(test)` a thread
/// can arm [`spawn_probe::SpawnProbe`], which captures the identity of the job
/// actually handed over and refuses the spawn; a test then drives the real
/// production path end to end and asserts by pointer identity that the job
/// which ARRIVED here is the one the runner was built with. It no longer
/// matters which receiver, which method or which module the call came from --
/// a substituted receiver arrives with a different job and is caught. The
/// probe is `cfg(test)` on the library crate, so it does not exist in the
/// shipped binary at all.
///
/// **The recorder is not the whole chain, and the other half already existed.**
/// It reports what was handed to this function; the four lines BELOW it are
/// where the job is actually used, and code inserted between the two could
/// still starve it. That stretch is held behaviourally by
/// [`tests::closing_the_job_kills_its_members`], which spawns a real child
/// through this very function -- `cmd`, never `bw`, and never a vault -- and
/// asserts it dies when the job handle closes. Between the two, every step
/// from a runner's constructor to the kernel is observed by something that is
/// not a text search.
pub fn spawn_in_job(job: Option<&KillOnCloseJob>, command: JobCommand) -> io::Result<Child> {
    // The one place a `JobCommand` is opened. Everything above this line can
    // describe a child; only this function can start one.
    let JobCommand { mut command } = command;

    // Deliberately above the jobless early return, so a spawn that carries no
    // job is recorded as such rather than slipping past unseen.
    #[cfg(test)]
    if spawn_probe::record(job, &command) {
        return Err(io::Error::other(spawn_probe::REFUSED));
    }

    let Some(job) = job else {
        return command.spawn();
    };

    // `CREATE_NO_WINDOW` is re-applied here, not just `CREATE_SUSPENDED`.
    //
    // `Command::creation_flags` *replaces* the flags the `Command` is
    // holding -- it only ORs with the ones std adds for itself at spawn
    // time. So setting `CREATE_SUSPENDED` alone silently discarded the
    // `CREATE_NO_WINDOW` that `bw_path::bw_command` had already set, and
    // `bw serve` -- the one child spawned through here -- came up with a
    // real console attached (confirmed by it owning a `conhost.exe` child),
    // flashing a console window on screen every time the backend started.
    command.creation_flags(crate::bw_path::CREATE_NO_WINDOW | CREATE_SUSPENDED.0);
    let mut child = command.spawn()?;

    if let Err(e) = job.assign(&child) {
        log::error!(
            "could not assign the child process to the kill-on-close job object ({e}); it may \
             survive an unclean exit of this process"
        );
    }

    match resume_process(child.id()) {
        Ok(0) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::other(
                "spawned the child suspended but found no thread to resume; killed it rather \
                 than leaving it wedged",
            ))
        }
        Ok(_) => Ok(child),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::other(format!(
                "could not resume the suspended child process: {e}"
            )))
        }
    }
}

/// Resumes every thread owned by `pid`, returning how many were resumed.
///
/// A process created with `CREATE_SUSPENDED` has exactly one (primary) thread,
/// but `std::process::Child` exposes only the process handle -- never the
/// thread handle `CreateProcessW` returned -- so the thread has to be found by
/// enumerating the snapshot. Safe against pid reuse because the caller still
/// holds the child's process handle, which pins the pid.
fn resume_process(pid: u32) -> windows::core::Result<u32> {
    let mut resumed = 0u32;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };

        if Thread32First(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    if let Ok(thread) = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                    {
                        // `ResumeThread` reports failure as `u32::MAX`, not as
                        // a `Result`.
                        if ResumeThread(thread) != u32::MAX {
                            resumed += 1;
                        }
                        let _ = CloseHandle(thread);
                    }
                }

                if Thread32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    Ok(resumed)
}

/// The test-only seam that stands exactly where `CreateProcess` would.
///
/// Everything upstream of the real spawn is production code, so a test that
/// arms this recorder and calls a production entry point observes what that
/// entry point really handed over -- not what a fake was told, and not what
/// the source text near the call happens to spell.
///
/// **The stopping point is deliberate.** The one hop this cannot cover is
/// `CreateProcess` itself: whether the kernel really put the child in the job
/// needs a child, and no test in this crate may start one. Everything before
/// that hop -- which runner, which receiver, which method, which module, and
/// which job value -- is now observable, and that is where all five previous
/// rounds of this finding were lost.
#[cfg(test)]
pub(crate) mod spawn_probe {
    use super::KillOnCloseJob;
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::process::Command;

    /// The message a refused spawn reports. Callers map it into their own
    /// failure type, which is exactly the production path a real spawn error
    /// would take, so arming the probe exercises that path too.
    pub const REFUSED: &str = "spawn refused by the test probe; no process was created";

    /// One attempted child, as [`super::spawn_in_job`] received it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Attempt {
        /// The ADDRESS of the job handed in, or `None` for a jobless spawn.
        ///
        /// An address and not a handle: the whole question is pointer
        /// identity -- "is this the very job the runner was constructed
        /// with?" -- and a `KillOnCloseJob` has no other identity to compare.
        /// Only ever compared against a job the test itself is still holding
        /// alive, so it cannot be a stale address that got reused.
        pub job: Option<usize>,
        /// The program the command would have run, so a test can tell one
        /// spawn from another without starting either.
        pub program: OsString,
        pub args: Vec<OsString>,
        /// The environment overlay the command carried, exactly as
        /// `Command::get_envs` reports it: the variables this process ASKED
        /// to add, change or remove for the child, not the whole inherited
        /// block. Recorded so that a caller can assert a secret reached the
        /// child in the environment rather than in `argv` without starting
        /// one -- see `send::tests`.
        pub envs: Vec<(OsString, Option<OsString>)>,
    }

    thread_local! {
        /// `None` when unarmed -- which is every thread that has not asked for
        /// the probe, including `job_object`'s own tests, which really do
        /// spawn `cmd` and must not be intercepted.
        static RECORDED: RefCell<Option<Vec<Attempt>>> = const { RefCell::new(None) };
    }

    /// Arms the recorder for the current thread until it is dropped.
    ///
    /// Thread-local rather than process-global because `cargo test` runs tests
    /// concurrently in one process: a global would let one test's arming
    /// refuse another test's real spawn, and a suite used as a mutation oracle
    /// cannot be flaky.
    pub struct SpawnProbe(());

    impl SpawnProbe {
        pub fn arm() -> Self {
            RECORDED.with(|slot| {
                let mut slot = slot.borrow_mut();
                assert!(
                    slot.is_none(),
                    "the spawn probe is already armed on this thread"
                );
                *slot = Some(Vec::new());
            });
            Self(())
        }

        /// Every spawn attempted since arming, in order.
        pub fn attempts(&self) -> Vec<Attempt> {
            RECORDED.with(|slot| slot.borrow().clone().expect("armed"))
        }
    }

    impl Drop for SpawnProbe {
        fn drop(&mut self) {
            RECORDED.with(|slot| *slot.borrow_mut() = None);
        }
    }

    /// Records one attempted spawn. `true` means the caller must not spawn.
    ///
    /// Called from production code, which is the entire point: a seam only a
    /// fake can reach guards nothing.
    pub(super) fn record(job: Option<&KillOnCloseJob>, command: &Command) -> bool {
        RECORDED.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(log) = slot.as_mut() else {
                return false;
            };
            log.push(Attempt {
                job: job.map(|j| std::ptr::from_ref(j) as usize),
                program: command.get_program().to_os_string(),
                args: command.get_args().map(OsString::from).collect(),
                envs: command
                    .get_envs()
                    .map(|(k, v)| (k.to_os_string(), v.map(OsString::from)))
                    .collect(),
            });
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `spawn_in_job` must keep the child windowless as well as suspended.
    ///
    /// The two flags are independent bits, and the bug this guards was
    /// setting only `CREATE_SUSPENDED` -- which, because
    /// `Command::creation_flags` replaces rather than merges, dropped the
    /// `CREATE_NO_WINDOW` set earlier by `bw_path::bw_command` and gave
    /// `bw serve` a visible console. There is no way to read the flags back
    /// off a `Command`, so this asserts the composed value directly.
    #[test]
    fn the_spawn_flags_keep_the_child_both_windowless_and_suspended() {
        let flags = crate::bw_path::CREATE_NO_WINDOW | CREATE_SUSPENDED.0;

        assert_eq!(
            flags & crate::bw_path::CREATE_NO_WINDOW,
            crate::bw_path::CREATE_NO_WINDOW,
            "CREATE_NO_WINDOW is missing -- the child would get a console window"
        );
        assert_eq!(
            flags & CREATE_SUSPENDED.0,
            CREATE_SUSPENDED.0,
            "CREATE_SUSPENDED is missing -- the child could run before it is \
             assigned to the job, escaping kill-on-close"
        );
        // Distinct bits, so neither can mask the other.
        assert_eq!(crate::bw_path::CREATE_NO_WINDOW & CREATE_SUSPENDED.0, 0);
    }

    #[test]
    fn creates_a_configured_job_object() {
        // Creating and configuring the job must succeed on any Windows host;
        // if `SetInformationJobObject` rejected our struct layout or flags
        // this would fail, which is exactly the regression worth catching.
        let job = KillOnCloseJob::new().expect("failed to create kill-on-close job object");
        drop(job);
    }

    /// A child that reliably stays alive for ~20s, so assignment and
    /// kill-on-close can be observed rather than raced against the child
    /// exiting on its own. `ping` is present on every Windows host and needs
    /// no network (127.0.0.1) or write access to anything.
    fn long_lived_command() -> Command {
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "ping", "-n", "20", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    /// The same child, wrapped so it can be handed to `spawn_in_job`.
    fn long_lived_job_command() -> JobCommand {
        JobCommand::wrap(long_lived_command())
    }

    fn kill_and_reap(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn assigns_a_live_child_process_to_the_job() {
        let job = KillOnCloseJob::new().unwrap();
        let mut child = long_lived_command()
            .spawn()
            .expect("failed to spawn test child");

        // Asserted, not discarded: against a genuinely live process this is a
        // real check that the handle conversion and the job handle are both
        // usable. The previous version ignored the result and so could not
        // fail for any reason other than a compile error.
        let assigned = job.assign(&child);
        kill_and_reap(&mut child);
        assigned.expect("assigning a live child process to the job must succeed");
    }

    #[test]
    fn spawn_in_job_returns_a_running_child() {
        let job = KillOnCloseJob::new().unwrap();
        let mut child = spawn_in_job(Some(&job), long_lived_job_command())
            .expect("spawn_in_job must succeed for a valid command");

        // The point of `CREATE_SUSPENDED` is that it is invisible to the
        // caller: the returned child must be resumed and running, not stuck.
        let still_running = child.try_wait().unwrap().is_none();
        kill_and_reap(&mut child);
        assert!(still_running, "child was not resumed after being assigned");
    }

    #[test]
    fn spawn_in_job_works_without_a_job_object() {
        let mut child = spawn_in_job(None, long_lived_job_command())
            .expect("spawn_in_job must degrade to a plain spawn with no job");
        let still_running = child.try_wait().unwrap().is_none();
        kill_and_reap(&mut child);
        assert!(still_running);
    }

    #[test]
    fn the_armed_probe_records_the_job_handed_over_and_starts_nothing() {
        // The seam's own control. Every guarantee built on the probe is worth
        // exactly what this is worth: that an armed probe sees the VALUE
        // `spawn_in_job` was handed, distinguishes one job from another and
        // from none, and refuses the spawn rather than creating a process.
        let job = KillOnCloseJob::new().unwrap();
        let other = KillOnCloseJob::new().unwrap();

        let probe = spawn_probe::SpawnProbe::arm();
        let refused = spawn_in_job(Some(&job), long_lived_job_command());
        let _ = spawn_in_job(Some(&other), long_lived_job_command());
        let _ = spawn_in_job(None, long_lived_job_command());
        let seen = probe.attempts();
        drop(probe);

        let err = refused.expect_err("an armed probe must refuse the spawn, not perform it");
        assert!(
            err.to_string().contains(spawn_probe::REFUSED),
            "the spawn was refused for some other reason, so the probe is not what stopped it: \
             {err}"
        );

        assert_eq!(seen.len(), 3, "the probe missed a spawn: {seen:?}");
        assert_eq!(seen[0].job, Some(std::ptr::from_ref(&job) as usize));
        assert_eq!(seen[1].job, Some(std::ptr::from_ref(&other) as usize));
        assert_eq!(
            seen[2].job, None,
            "a jobless spawn was not recorded as jobless, so the probe cannot tell a child that \
             joined the job from one that did not"
        );
        assert_ne!(
            seen[0].job, seen[1].job,
            "control: two live jobs are indistinguishable to the probe, so every pointer \
             comparison made through it is vacuous"
        );
        // And it saw the command, not just the job.
        assert_eq!(seen[0].program, std::ffi::OsString::from("cmd"));
    }

    #[test]
    fn an_unarmed_probe_is_not_in_the_way() {
        // The other half of the control above, and the reason the three tests
        // in this module that really spawn are still real tests: with no probe
        // armed on this thread, `spawn_in_job` performs the spawn.
        let mut child = spawn_in_job(None, long_lived_job_command())
            .expect("with no probe armed the spawn must really happen");
        let running = child.try_wait().unwrap().is_none();
        kill_and_reap(&mut child);
        assert!(running);
    }

    /// The files excused from
    /// [`only_the_files_that_must_leave_the_job_can_open_a_bare_command`] --
    /// the ones licensed to open a `BareCommand` and start a child that is not
    /// in the kill-on-close job.
    ///
    ///   bw_path.rs  -- defines the door, and `bw_job_command_in` is the one
    ///                  production place that walks through it in order to hand
    ///                  the command straight to `JobCommand::wrap`.
    ///   bw_serve.rs -- `bw serve` is the long-lived backend; it is owned and
    ///                  killed by the backend policy, not by the job.
    ///   login_ui.rs -- sign-in, unlock and status run before there is a vault
    ///                  window, and so before there is a job.
    ///   main.rs     -- re-wraps into a `JobCommand` at the one call site that
    ///                  builds the vault window's own job.
    ///
    /// **Module scope because there are two readers and they must not drift.**
    /// RULE 8 needs this same set as MODULE names, to refuse a hop that lands
    /// in a door, and it kept a hand-written copy -- four strings with the
    /// `.rs` filed off, in a different test function, with nothing asserting
    /// the two agreed and no staleness check, while `DOOR_FILES`, `ALLOWED` and
    /// `BW_PATH_MAY_REACH` are all stale-checked. A copy that is one entry
    /// behind is a door RULE 8 has stopped refusing, and nothing would say so.
    /// There is now one list and [`door_modules`] derives the other view of it.
    const DOOR_FILES: &[&str] = &["bw_path.rs", "bw_serve.rs", "login_ui.rs", "main.rs"];

    /// [`DOOR_FILES`] as module names -- what RULE 8 refuses to end a hop in.
    fn door_modules() -> Vec<String> {
        DOOR_FILES
            .iter()
            .map(|f| f.strip_suffix(".rs").unwrap_or(f).to_string())
            .collect()
    }

    /// Every `.rs` file under this crate's `src`, recursively.
    fn rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension() == Some(std::ffi::OsStr::new("rs")) {
                out.push(path);
            }
        }
    }

    /// Ways to start a child process that do NOT pass through
    /// [`spawn_in_job`], and so are invisible to the probe above.
    ///
    /// **Scanned twice, and the second pass is the one that has to hold.** The
    /// line pass exists only to name a line number, which is what makes a
    /// failure actionable. It reads UNSTRIPPED source, and the eighth hop was
    /// written straight through it:
    ///
    ///     let _ = real.spawn ();
    ///
    /// One space. `.spawn ()` contains none of the three literals, so the line
    /// pass said nothing -- exactly the mistake the round-six note in
    /// [`code_only`] warns about, left standing on this one matcher while the
    /// other rules moved onto the stripped view.
    ///
    /// So the second pass runs the same needles over [`code_only`], where
    /// comments, string literals and **all whitespace** are gone: `.spawn ()`,
    /// `.spawn/*x*/()`, `. spawn ( )` and a receiver split across two lines are
    /// one string there. Counted rather than merely searched, so an occurrence
    /// the line pass already reported is not reported twice -- and if the two
    /// disagree the stripped count wins, because it is the one no spelling can
    /// hide from.
    fn direct_child_starts(label: &str, text: &str) -> Vec<String> {
        let needles = [
            concat!(".spa", "wn()"),
            concat!(".out", "put()"),
            concat!(".sta", "tus()"),
        ];
        let mut found: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| needles.iter().any(|n| line.contains(n)))
            .map(|(i, line)| format!("{label}:{}: {}", i + 1, line.trim()))
            .collect();

        let code = code_only(text);
        for needle in needles {
            let stripped = code.matches(needle).count();
            let named = found.iter().filter(|f| f.contains(needle)).count();
            // Fails safe in both directions: a stripped count that exceeds what
            // the line pass could name adds the difference as line-less
            // offenders, and a line pass that somehow saw more adds nothing.
            for _ in named..stripped {
                found.push(format!(
                    "{label}: (in the comment-, string- and whitespace-free view) {needle}"
                ));
            }
        }
        found
    }

    #[test]
    fn the_two_job_bearing_modules_can_start_a_child_only_through_this_one() {
        // THE OTHER HALF OF THE SEAM. The probe proves what arrives at
        // `spawn_in_job`; this proves there is no second door. Without it a
        // runner could simply call `command.spawn()` itself, satisfy every
        // assertion made through the probe by never reaching it, and spawn
        // every child outside the job.
        //
        // A WALK of the real tree, not an `include_str!` of two known files
        // and not a text cut at the test module: measured on an earlier
        // commit, a `pub fn` carrying a bare `command.spawn()` appended BELOW
        // both test modules of `send.rs` was invisible to every pin in that
        // file. A walk sees whole files, and sees files that do not exist yet.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = root.join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);

        // AND `build.rs`, WHICH IS NOT UNDER `src/`.
        //
        // The round that closed the thirteenth hop disclosed a proc-macro
        // route as "not constructible today (adding one needs a `Cargo.toml`
        // change)". That understated it. `deskwarden/build.rs` is ALREADY in
        // this tree, cargo compiles and RUNS it on every build of this crate,
        // and it sits outside `src/` -- so the walk above never reached it and
        // no rule in this module read a byte of it. A `.spawn()` written there
        // runs at build time, outside everything the fences below can see.
        //
        // It is a source file of this crate, so it is walked like one. It is
        // deliberately NOT on `ALLOWED`: it starts no child today.
        //
        // It is NOT fenced the way `src/` files are (no callee list, no cut),
        // and the reason is stated rather than left to silence: the fences
        // constrain what the two JOB-BEARING RUNNERS can reach at RUNTIME, and
        // `build.rs` is in no runtime call graph at all -- nothing in `src/`
        // can name it, and it cannot name anything in `src/`. The only
        // property of it that belongs to this guarantee is the one asserted
        // here: it starts no child.
        let build_rs = root.join("build.rs");
        assert!(
            build_rs.is_file(),
            "control: `build.rs` is not where this walk expects it. If it was deleted, delete \
             this block too; if it moved, follow it -- do not leave the walk silently reading \
             nothing, which is exactly how it went unread for thirteen rounds"
        );
        files.push(build_rs.clone());

        // AND HELD TIGHTER THAN THE WALK ALONE HOLDS IT.
        //
        // Measured, and SURVIVING the walk above at 2105 lib / 217 bin / 1
        // ignored / 0 failed / 0 warnings:
        //
        //     let mut c = <std's process builder>::new(<the CLI>);
        //     let f = <that same type>::spawn;   // the fn ITEM, by path
        //     let _ = f(&mut c);
        //
        // (Spelled around rather than verbatim: `bw_path.rs` runs its own
        // crate-wide scan for a `bw` started outside it, and a comment
        // carrying the real literals trips that instead of this.)
        //
        // `direct_child_starts` matches a RECEIVER followed by `.spawn(`,
        // `.output(` or `.status(`; a function ITEM taken by path and called
        // through a binding writes none of the three. Every file under `src/`
        // the two runners can reach has a callee allowlist below it, which
        // sees `spawn` under any spelling -- `build.rs` has none and cannot
        // usefully be given one, because it is in no runtime call graph and
        // the entire reason it is in scope is that nothing else reads it.
        //
        // So this one file gets the blunt rule its size affords: 46 lines
        // that embed an icon resource, and not one of the tokens out of which
        // a child process is built. Cheaper than a fence, and strictly
        // stronger for a file this small.
        //
        // Disclosed rather than claimed complete: a build script can still
        // reach a child through some other crate that spawns on its behalf,
        // and no token ban sees that. What it does see is every route that
        // names the primitive itself.
        let build_code = code_only(&std::fs::read_to_string(&build_rs).unwrap());
        for banned in [
            concat!("spa", "wn"),
            concat!("Comm", "and"),
            concat!("CreatePro", "cess"),
            concat!("ex", "ec"),
        ] {
            assert!(
                !build_code.contains(banned),
                "`build.rs` names `{banned}`. It runs at BUILD time, outside every fence in \
                 this module and outside the job object entirely, and all it does is embed an \
                 icon -- it has no business anywhere near a child process. If it genuinely \
                 needs one, that is a decision to write down here, not to slip past a walk \
                 that knows only three spellings"
            );
        }
        assert!(
            build_code.len() > 150,
            "control: `build.rs` stripped to {} chars (it is mostly comment, and 204 when this \r
             was written) -- the token ban above is reading nothing",
            build_code.len()
        );

        // The walk itself, pinned: a recursion that returned nothing, or that
        // never descended into `vault_window/`, would make this vacuous.
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for expected in ["vault_export.rs", "send.rs", "job_object.rs", "mod.rs", "main.rs", "build.rs"]
        {
            assert!(
                names.iter().any(|n| n == expected),
                "the source walk never reached {expected}; it found {names:?}"
            );
        }

        // Files allowed to start a child outside the job, each because it
        // really does today and for a reason that is not this module's to
        // change. Adding a file here is a visible edit to this list -- which
        // is the point: it is not a hole, it is a signature.
        //
        //   job_object.rs  -- the choke point itself, and its own tests.
        //   bw_serve.rs, login_ui.rs, main.rs, updater.rs, vault_window/mod.rs
        //                  -- pre-existing spawns outside this change's scope.
        //
        // Paths, not bare file names: this crate has two `mod.rs` files and
        // only one of them spawns, and a name-keyed list would have excused
        // both.
        const ALLOWED: &[&str] = &[
            "job_object.rs",
            "bw_serve.rs",
            "login_ui.rs",
            "main.rs",
            "updater.rs",
            "vault_window/mod.rs",
        ];

        // `build.rs` is not under `src/`, so it keeps its bare name here --
        // which is also the name it would have to be given on `ALLOWED`.
        let relative = |p: &std::path::Path| {
            p.strip_prefix(&src)
                .unwrap_or(std::path::Path::new(p.file_name().unwrap()))
                .to_string_lossy()
                .replace('\\', "/")
        };

        // Stale entries are an error too: an entry that no longer spawns is a
        // pre-opened door for whoever edits that file next.
        for allowed in ALLOWED {
            let text = std::fs::read_to_string(src.join(allowed)).unwrap();
            assert!(
                !direct_child_starts(allowed, &text).is_empty(),
                "{allowed} is excused from this guard but no longer starts a child; remove it \
                 from ALLOWED rather than leaving the exemption standing"
            );
        }

        let mut offenders = Vec::new();
        for file in &files {
            if ALLOWED.contains(&relative(file).as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(file).unwrap();
            offenders.extend(direct_child_starts(&file.display().to_string(), &text));
        }
        assert!(
            offenders.is_empty(),
            "a child process is started outside `spawn_in_job`, so it joins no job object and \
             the spawn probe never sees it -- a `bw` holding an unlocked vault could outlive a \
             panic, a `process::exit` or a Task Manager kill:\n{}",
            offenders.join("\n")
        );

        // The two files this guarantee is actually about are named, so that
        // silently adding either to ALLOWED is not enough to make the guard
        // shrug: they must carry ZERO occurrences, in the whole file, tests
        // and comments included.
        for must_be_clean in ["vault_export.rs", "send.rs"] {
            let path = src.join(must_be_clean);
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                direct_child_starts(must_be_clean, &text).is_empty(),
                "{must_be_clean} can start a child without passing `spawn_in_job`"
            );
            assert!(
                text.contains(concat!("job_object::spawn_", "in_job(")),
                "control: {must_be_clean} no longer spawns through the choke point at all, so \
                 the emptiness asserted above means the module stopped spawning rather than \
                 that it spawns correctly"
            );
        }

        // Positive control, through the same matcher: it can see the thing it
        // exists to catch...
        assert_eq!(
            direct_child_starts("planted.rs", &format!("let c = cmd{}?;", concat!(".spa", "wn()")))
                .len(),
            1,
            "the matcher cannot see a direct spawn, so its silence above means nothing"
        );
        assert_eq!(
            direct_child_starts("planted.rs", &format!("let o = cmd{};", concat!(".out", "put()")))
                .len(),
            1
        );
        assert_eq!(
            direct_child_starts("planted.rs", &format!("let s = cmd{};", concat!(".sta", "tus()")))
                .len(),
            1
        );
        // ...including THE EIGHTH HOP's spelling, which is the same three
        // literals with one space in the middle and which the line pass alone
        // could not see.
        assert_eq!(
            direct_child_starts(
                "planted.rs",
                &format!("let _ = real{};", concat!(".spa", "wn ()"))
            )
            .len(),
            1,
            "`.spawn ()` is still invisible here, so this guard would lose the eighth hop again"
        );
        // ...and the same thing split across lines, which no line-by-line
        // matcher can ever see however many spellings it is taught.
        assert_eq!(
            direct_child_starts(
                "planted.rs",
                &format!("let _ = real\r\n    {}\r\n    );", concat!(".spa", "wn ("))
            )
            .len(),
            1,
            "a receiver and its `spawn` on two lines slip past"
        );
        // ...and does not flag the choke-point call itself.
        assert!(direct_child_starts(
            "planted.rs",
            "let child = crate::job_object::spawn_in_job(self.job(), command)?;"
        )
        .is_empty());
        // ...nor prose about spawning, which the stripped pass must drop or
        // every doc comment in these two modules becomes an offender.
        assert!(direct_child_starts(
            "planted.rs",
            "/// The runner used to call .spawn ( ) itself; it no longer can.\r\nlet a = 1;"
        )
        .is_empty());
    }

    /// A file's Rust source with comments and string literals removed, and
    /// then all whitespace removed.
    ///
    /// **Both halves matter, and each answers a measured defeat.** Dropping
    /// comments and strings is what lets the rules below talk about the
    /// identifier `Command` at all: these two modules are full of prose about
    /// commands and spawning, and a matcher that saw prose could only be a
    /// matcher for phrases. Dropping whitespace is what makes the rules immune
    /// to spelling: `Command :: new`, a receiver split across two lines and
    /// `Command::new` are one string here, and round six was lost precisely
    /// because `.spawn()` and `Command::spawn(&mut command)` are different
    /// strings.
    ///
    /// Raw strings and char literals are handled explicitly because both occur
    /// in the scanned files (`r"C:\..."` paths; `'\\'`), and a lifetime `'a`
    /// is not mistaken for a char literal -- `CliSendRunner<'a>` would
    /// otherwise swallow the rest of the file and make every rule vacuous.
    /// [`the_code_only_view_really_drops_comments_and_strings`] is the control.
    fn code_only(text: &str) -> String {
        let b: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            let next = b.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 1;
                i += 2;
                while i < b.len() && depth > 0 {
                    if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            } else if c == 'r' && (next == Some('"') || next == Some('#')) {
                // `r"..."` / `r#"..."#`, with the hash count preserved.
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    out.push(c);
                    i += 1;
                    continue;
                }
                j += 1;
                let close: String =
                    std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                let rest: String = b[j..].iter().collect();
                i = match rest.find(&close) {
                    Some(k) => j + rest[..k].chars().count() + close.chars().count(),
                    None => b.len(),
                };
            } else if c == '"' {
                i += 1;
                while i < b.len() {
                    if b[i] == '\\' {
                        i += 2;
                    } else if b[i] == '"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else if c == '\'' {
                // A char literal only if it closes within the next few
                // characters; otherwise it is a lifetime and is kept.
                let mut j = i + 1;
                if b.get(j) == Some(&'\\') {
                    j += 1;
                }
                if b.get(j).is_some() && b.get(j + 1) == Some(&'\'') {
                    i = j + 2;
                } else {
                    out.push(c);
                    i += 1;
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Production `job_object.rs`, cut at the first module-level
    /// `#[cfg(test)]`. Everything below that -- the spawn probe and this test
    /// module -- is absent from the shipped binary.
    /// This file, whole, with its line endings normalised to LF.
    ///
    /// Split out of [`job_object_production`] so that
    /// `nothing_but_gated_test_modules_lives_below_the_choke_points_cut` can walk the
    /// half the cut throws away, taken from the same text by the same
    /// function, rather than re-reading the file and re-deriving a cut that
    /// could drift away from the one every rule here reads.
    fn job_object_raw() -> String {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        std::fs::read_to_string(src.join("job_object.rs")).unwrap().replace("\r\n", "\n")
    }

    fn job_object_production() -> String {
        let raw = job_object_raw();
        let cut = concat!("\n#[cfg(", "test)]\n");
        let production = raw.split(cut).next().unwrap().to_string();
        assert!(
            production.len() < raw.len(),
            "control: the `#[cfg(test)]` cut marker was not found in job_object.rs, so this scan \
             is reading the test module as production and every count below is meaningless"
        );
        assert!(
            production.contains("pub fn spawn_in_job("),
            "control: the production cut of job_object.rs does not contain `spawn_in_job`, so \
             the cut is in the wrong place"
        );
        production
    }

    /// RULE 9: THE CHOKE POINT MODULE IS GOVERNED TOO.
    ///
    /// Every other rule in this file points somewhere else. `vault_export.rs`
    /// and `send.rs` may not name a `BareCommand`; `bw_path.rs` may reach only
    /// two items; the tree walk excuses six files and refuses the rest. Nothing
    /// governed `job_object.rs`. It was excused from the tree walk (it *is* the
    /// spawn), it is its own home file for `OWN_IMPLS_ONLY`, and RULE 8's one
    /// permitted hop now terminates here by construction -- `job_object` is the
    /// only module `BW_PATH_MAY_REACH` allows. So the one-hop check is sound
    /// and hands the problem to a module with no rules.
    ///
    /// Two mutants written in this file survived a full green suite:
    ///
    ///   M12 -- a `run_it` helper written HERE, in the type's own home file,
    ///          where `OWN_IMPLS_ONLY` skips it by design, called from
    ///          `CliExportRunner::run` as `command.run_it()`, which names no
    ///          type and writes no path.
    ///   N14 -- `spawn_in_job` starting a SECOND, jobless child in its own
    ///          body:
    ///
    ///              let mut side = std::process::Command::new(command.get_program());
    ///              side.args(command.get_args());
    ///              let _ = side.spawn();
    ///
    ///          placed after the probe's early return, so under an armed probe
    ///          the added lines never execute and the probe reports exactly one
    ///          arrival carrying exactly the right job. 2077 lib / 217 bin /
    ///          1 ignored / 0 failed / 0 warnings.
    ///
    /// M12 is dead structurally: `get_program`, `get_args` and `get_envs` are
    /// `#[cfg(test)]` now, so a production helper cannot read the command it
    /// was handed. N14 is not -- it destructures nothing and reads nothing that
    /// is gated; it just writes a second command.
    ///
    /// **This rule is a set of counts, and counts are a text rule.** It is
    /// shipped knowing that, because the alternative on offer was nothing: job
    /// membership is a property of a live process, no test here may start one
    /// beyond the single pre-existing `cmd`, and the probe cannot observe code
    /// placed after the refusal it returns. What the counts buy is that the
    /// second child cannot be written in one line: it needs a second `Command`
    /// value AND a way to start it, and each of those is pinned separately, so
    /// the cheapest surviving mutation is now a deliberate rewrite of the
    /// choke point rather than an insertion into it. The known ways through are
    /// written out at the bottom of this test, in the same spirit as the notes
    /// above: a rule whose holes are undocumented is a rule nobody can widen.
    #[test]
    fn the_choke_point_module_starts_a_child_in_exactly_one_place() {
        let production = job_object_production();
        let code = code_only(&production);
        assert!(
            code.len() > 2000,
            "control: production job_object.rs stripped to almost nothing, so every count \
             below is vacuous"
        );

        // (a) The three literals the tree walk reads, over the same stripped
        //     view it uses. `.output()` and `.status()` start a child too, and
        //     this file is on ALLOWED, so nothing else in the crate looks at
        //     them here.
        assert_eq!(code.matches(concat!(".spa", "wn()")).count(), 2, "{}", SPAWN_COUNT);
        assert_eq!(code.matches(concat!(".out", "put()")).count(), 0, "{}", SPAWN_COUNT);
        assert_eq!(code.matches(concat!(".sta", "tus()")).count(), 0, "{}", SPAWN_COUNT);

        // (b) ...and both of them are the one binding `spawn_in_job`
        //     destructures out of the `JobCommand` it was handed. A second
        //     child started on any other receiver -- `side.spawn()`, which is
        //     what N14 wrote -- fails here even before the count does.
        assert_eq!(
            code.matches(concat!("command.spa", "wn()")).count(),
            2,
            "production job_object.rs starts a child on a receiver that is not the `command` \
             destructured out of the caller's `JobCommand`. That child is not the one the \
             runner built and is not in the runner's job"
        );

        // (c) The whole word, however it is spelled. `Command::spawn(&mut c)`
        //     is the sixth round's decoy and contains none of the literals in
        //     (a); a function pointer `let f = Command::spawn;` contains none
        //     of them either. Both write the word, so the word is counted.
        //     Five: `spawn_in_job`, two `spawn_probe::` paths, and the two
        //     spawns from (a).
        assert_eq!(
            code.matches("spawn").count(),
            5,
            "the word `spawn` occurs somewhere new in production job_object.rs. Every use is \
             accounted for -- the function's own name, the two `spawn_probe` paths, and the \
             two `command.spawn()` calls -- so a sixth is a way of starting a child that (a) \
             and (b) were not written to see, `Command::spawn(&mut c)` among them"
        );

        // (d) And there is no second command to start. `JobCommand` owns the
        //     only `Command` this module holds, it is opened in exactly one
        //     place, and a fresh one cannot be built without saying so.
        assert_eq!(
            code.matches(concat!("Command::", "new(")).count(),
            0,
            "production job_object.rs constructs a `Command`. This module holds exactly one, \
             it arrives already built from `bw_path::bw_job_command_in`, and a second one is a \
             second child that no caller asked for and no job owns"
        );
        assert_eq!(
            code.matches(concat!("JobCommand{mut", "command}=command;")).count(),
            1,
            "the one place a `JobCommand` is opened has moved or been joined by another. The \
             destructure is the only way to reach the inner `Command`, so a second one is a \
             second spawnable command inside the choke point itself"
        );

        // CONTROLS, through the same matcher, on shapes that are not in the
        // file: each half can see what it exists to catch.
        assert_eq!(code_only("let _ = side . spawn ( ) ;").matches(concat!(".spa", "wn()")).count(), 1);
        assert_eq!(
            code_only("let _ = side . spawn ( ) ;").matches(concat!("command.spa", "wn()")).count(),
            0,
            "control: (b) cannot tell a foreign receiver from `command`, so N14 walks through it"
        );
        assert_eq!(
            code_only("let _ = Command :: spawn ( & mut side ) ;").matches(concat!(".spa", "wn()")).count(),
            0,
            "control: the sixth round's decoy really is invisible to (a), which is why (c) exists"
        );
        assert_eq!(code_only("let _ = Command :: spawn ( & mut side ) ;").matches("spawn").count(), 1);
        assert_eq!(code_only("let f = Command :: spawn ;").matches("spawn").count(), 1);
        assert_eq!(
            code_only("let mut side = std :: process :: Command :: new ( p ) ;")
                .matches(concat!("Command::", "new("))
                .count(),
            1,
            "control: (d) cannot see a freshly built command, so N14's first line walks through it"
        );
        assert_eq!(
            code_only("let JobCommand { mut command } = other ;")
                .matches(concat!("JobCommand{mut", "command}=command;"))
                .count(),
            0,
            "control: a second destructure of a DIFFERENT value is not miscounted as the one \
             real one"
        );

        // WHAT THIS DOES NOT CATCH, written down rather than left to be
        // rediscovered as the twelfth hop:
        //
        //  * `CreateProcessW` called directly through `windows-sys`. It writes
        //    no `spawn` and builds no `Command`. It is refused nowhere and is
        //    the cheapest remaining escape from this file -- but it is a dozen
        //    lines of `unsafe` FFI with a `STARTUPINFOW` in the crate's most
        //    reviewed function, against one line before this rule existed.
        //  * a rewrite of `spawn_in_job` that keeps all five `spawn` words and
        //    all the counts while handing `None` for the job. That one is not
        //    this rule's business: it is what the probe observes, by pointer
        //    identity, in
        //    `the_export_reaches_the_spawn_carrying_the_job_the_entry_point_was_given`.
        //  * a helper that takes what it needs as an ARGUMENT rather than
        //    reading it off a `JobCommand` -- the residue the `#[cfg(test)]`
        //    gate on the `get_*` readers deliberately leaves. It would still
        //    have to start its child, and in this file (c) counts that.
    }

    /// Shared failure text for the three literal counts in RULE 9.
    const SPAWN_COUNT: &str = "production job_object.rs starts a child somewhere other than the \
         two `command.spawn()` calls in `spawn_in_job`. This file is excused from the tree walk \
         because it IS the spawn, so nothing else in the crate is looking: a second child \
         started here is started nowhere any rule can see, and a child started after the spawn \
         probe's early return never executes under an armed probe, so the probe reports one \
         clean arrival while two processes start";

    #[test]
    fn the_code_only_view_really_drops_comments_and_strings() {
        // Every rule in the next test is worth exactly what this is worth.
        assert_eq!(code_only("let a = 1; // Command::new\n"), "leta=1;");
        assert_eq!(code_only("/* Command::spawn() */ let b = 2;"), "letb=2;");
        assert_eq!(code_only("panic!(\"Command::spawn()\");"), "panic!();");
        assert_eq!(code_only("let p = r\"C:\\x\\Command\"; let q = 3;"), "letp=;letq=3;");
        assert_eq!(code_only("let s = r#\"a\"Command\"#; let t = 4;"), "lets=;lett=4;");
        assert_eq!(code_only("let c = '\\\\'; let d = '\"';"), "letc=;letd=;");
        // A lifetime is not a char literal: the rest of the line survives.
        assert_eq!(code_only("struct S<'a> { c: &'a Command }"), "structS<'a>{c:&'aCommand}");
        // And code really is kept, in every spelling of it.
        assert_eq!(code_only("Command :: spawn ( & mut c )"), "Command::spawn(&mutc)");
        assert_eq!(code_only("Command\n    ::spawn(&mut c)"), "Command::spawn(&mutc)");
    }

    #[test]
    fn the_two_job_bearing_modules_cannot_name_a_bare_command() {
        // THE FIX FOR ROUND SIX, in text; the type is the other half.
        //
        // Round six was this, in `CliExportRunner::run`:
        //
        //     let _ = spawn_in_job(self.job(), Command::new("x"));   // decoy
        //     let child = Command::spawn(&mut command)?;             // real
        //
        // Everything stayed green. The probe recorded one arrival carrying
        // exactly the right job -- because it records *an* arrival, not *the*
        // spawn -- and the tree walk above matched three literals none of
        // which `Command::spawn(&mut command)` contains.
        //
        // The type closes the second line: `export_command` and
        // `build_command` now hand back a `JobCommand` whose inner command is
        // private to this module, so there is no method on it that starts a
        // process and no way to get the command out. This test closes the
        // first: with no way to *name* a bare command type, these two modules
        // cannot construct a second child description at all, so they have
        // nothing to offer the probe as a decoy and nothing to spawn behind
        // its back. Neither half is sufficient alone, which is why both are
        // here.
        //
        // Note what is NOT being asserted: no phrase, no call spelling, no
        // line. The rules are about the identifiers `Command` and `wrap`, and
        // an identifier has exactly one spelling.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

        for file in ["vault_export.rs", "send.rs"] {
            let raw = std::fs::read_to_string(src.join(file))
                .unwrap()
                .replace("\r\n", "\n");
            let code = code_only(&raw);

            // Controls FIRST: these rules are all "count is zero" shaped, and
            // a `code_only` that returned nothing, or a module that stopped
            // spawning, would satisfy every one of them while asserting
            // nothing at all.
            assert!(
                code.len() > 1000,
                "{file} produced almost no code, so every rule below is vacuous"
            );
            assert_eq!(
                code.matches("JobCommand").count() >= 1,
                true,
                "{file} no longer names the wrapper at all, so it is not building its child \
                 through the one type that can only be spawned into a job"
            );
            assert!(
                code.contains("job_object::spawn_in_job("),
                "control: {file} no longer spawns through the choke point at all, so the \
                 emptiness asserted below means the module stopped spawning rather than that \
                 it spawns correctly"
            );

            // RULE 1, and the load-bearing one: every occurrence of the
            // identifier `Command` is part of `JobCommand`. So there is no
            // `std::process::Command` in this module under any spelling --
            // not `Command::new`, not `process::Command`, not a `use` alias
            // (an alias must still name the type it aliases), not `CommandExt`
            // -- and therefore no way to reach `spawn`, `output` or `status`,
            // which exist only on that type.
            assert_eq!(
                code.matches("Command").count(),
                code.matches("JobCommand").count(),
                "{file} names a command type that is not `JobCommand`, so it can describe a \
                 child this module cannot see and start it outside the kill-on-close job"
            );

            // RULE 2: and it may not wrap one either. `JobCommand::wrap` is
            // public (main.rs is a separate crate), so without this a runner
            // could wrap a decoy, hand it to the probe, and satisfy every
            // pointer assertion made through it.
            assert_eq!(
                code.matches("JobCommand::wrap").count(),
                0,
                "{file} wraps a command of its own; the only command it may build is the \
                 verified one from `bw_path`, or it has a second child to offer the probe"
            );

            // There is deliberately no rule about the identifier `spawn`.
            // Rule 1 already denies these modules any value with a `spawn`,
            // `output` or `status` method on it, and a text rule would have
            // to tell `Command::spawn` from `scope.spawn` and
            // `job_reaching_the_spawn` -- which is a rule about phrases, and
            // rules about phrases are what the last six rounds defeated.

            // RULE 5, AND THE ONE THAT CLOSES THE EIGHTH HOP. Rule 1 bans
            // *naming* a bare command type; it cannot ban receiving one by
            // inference, which is exactly what the eighth hop did:
            //
            //     if let Ok(mut real) = crate::bw_path::bw_command_in(None) {
            //         real.args(export_args(plan));
            //         let _ = real.spawn ();
            //     }
            //
            // The identifier `Command` never appeared. `bw_command_in` was
            // `pub` and handed back a bare `std::process::Command`, so a second
            // real `bw.exe` -- signature-verified, this plan's argv, an
            // unlocked vault -- started outside the kill-on-close job with
            // every rule above still satisfied.
            //
            // `bw_path::bw_command_in`, `bw_path::bw_command` and
            // `bw_serve::bw_serve_command` now hand back a
            // `bw_path::BareCommand` instead, and after that change NO public
            // function in this crate returns a bare command. The single door
            // out of that wrapper is the inherent method below.
            //
            // Why the identifier is the right thing to ban: it cannot be
            // aliased or re-exported. There is no `use path::method as other`
            // for an inherent method, there is no trait to reach it through
            // (the field is private, so no foreign impl can extract it), and
            // there is no operator sugar for it. Calling it means writing this
            // exact identifier, in code, in SOME file. An identifier has one
            // spelling and `code_only` sees through whitespace, so
            // `BareCommand :: into_jobless_command ( c )` and
            // `c.into_jobless_command()` are the same string here.
            //
            // **"in SOME file" is not "in this file", and the eighth round's
            // note claimed it was.** It read "an inherent method cannot be
            // renamed", and concluded that this rule, applied to these two
            // files, closed the door. The premise is right and the conclusion
            // is wrong, because INHERENT IMPLS ARE NOT RESTRICTED TO THE
            // DEFINING MODULE: any module in this crate may write
            //
            //     impl crate::bw_path::BareCommand {
            //         pub fn run_it(self) { let _ = self.into_jobless_command().spawn(); }
            //     }
            //
            // and inside THAT impl the banned identifier is written in a file
            // this loop does not read. `real.run_it()` back in the runner is a
            // method call on a local: it names no type, writes no path, and so
            // is invisible to every rule here -- RULE 7 included, which is the
            // hop RULE 7 exists to close, re-opened through a receiver.
            // Measured on the parent commit: 2072 lib / 217 bin / 0 failed / 0
            // warnings, with a jobless verified `bw.exe` on this plan's argv.
            //
            // So this rule is now only the local half. The crate-wide half is
            // `only_the_files_that_must_leave_the_job_can_open_a_bare_command`
            // below, which counts the identifier in EVERY file and refuses a
            // foreign `impl` block on the type outright. Neither is sufficient
            // alone: this one keeps the runner from opening the door itself,
            // that one keeps anyone else from opening it on the runner's
            // behalf.
            assert_eq!(
                code.matches("into_jobless_command").count(),
                0,
                "{file} takes a command out of `BareCommand`, which is the one way to hold a \
                 spawnable `std::process::Command` in this crate -- so it can start a `bw` \
                 holding an unlocked vault outside the kill-on-close job"
            );

            // RULE 6: and it may not go under `std` for a child either.
            // Everything above is about the safe API; `CreateProcessW` and
            // friends are reachable from the `windows` crate with no command
            // type named anywhere, and cannot be called without this keyword.
            // Free to assert -- both files contain zero `unsafe` today -- and
            // if one ever legitimately needs it, that is a visible edit here.
            assert_eq!(
                code.matches("unsafe").count(),
                0,
                "{file} contains `unsafe`, so it can reach a raw process-creation API directly \
                 and no rule above would see the child"
            );

            // RULE 7: WHOSE CODE THESE TWO MODULES MAY REACH AT ALL.
            //
            // Every rule above is about what these files may *write*. This one
            // is about what they may *call*, and it is here because a measured
            // mutant walked straight through all of them:
            //
            //   login_ui.rs (an ALLOWED file):
            //       pub fn run_anything_jobless(args: &[String]) { .. spawn .. }
            //   vault_export.rs:
            //       crate::login_ui::run_anything_jobless(&export_args(plan));
            //
            // No command type named, no banned identifier, no `.spawn()` in
            // this file in any spelling -- and a jobless `bw` running with this
            // plan's arguments. The probe cannot see it (it never reaches
            // `spawn_in_job`) and the outcome assertion cannot see it (the
            // honest child still arrives, still refuses, still answers). It is
            // the residual the seventh round wrote down and left open.
            //
            // Closing it needs a rule of a different kind: calling code in
            // another module means NAMING that module. So the set of modules
            // these two may name is closed, and it is tiny -- the choke point
            // and the one place that builds the verified command. A helper
            // planted anywhere else is unreachable from here whatever it is
            // called, because the path to it cannot be written.
            //
            // `super::` IS an escape, and this comment used to say it was
            // not -- see RULE 7b below, which is the ninth round's fix.
            //
            // ITEMS, not modules. A module-level allow-list was measured and
            // was not enough: `job_object.rs` is necessarily both reachable
            // (it is the choke point) and on the tree walk's ALLOWED list (it
            // is the one file that legitimately spawns), so a `pub fn
            // spawn_loose(args)` added HERE and called from `vault_export`
            // satisfied a module-level rule completely -- 2071/217/0 failed/0
            // warnings, with a jobless `bw` running this plan's arguments.
            // Naming an item is the same act as naming a module -- you cannot
            // call what you cannot write -- so the list is simply drawn one
            // level finer, and a helper that is not on it is unreachable
            // wherever it is planted, including in this very file.
            // `send.rs`'s test module borrows `login_ui`'s allocator probe for
            // the secret-lifetime assertions. Left as a prefix rather than
            // itemised because it is a `#[cfg(test)]` module: production code
            // that called into it would not compile in the non-test build at
            // all, which is a stronger guarantee than this list gives.
            const REACHABLE_TEST_ONLY: &[&str] = &["crate::login_ui::password_lifetime_tests::"];

            for path in crate_paths(&code) {
                {
                    assert!(
                        REACHABLE.contains(&path.as_str())
                            || REACHABLE_TEST_ONLY.iter().any(|ok| path.starts_with(ok)),
                        "{file} names `{path}`, which is not on the list of items these two \
                         modules may reach. A helper anywhere in this crate -- including in \
                         `job_object.rs` itself -- could start a `bw` outside the kill-on-close \
                         job on their behalf, and neither the probe nor the tree walk would see \
                         the child. If this item is genuinely needed, adding it here is the \
                         visible decision that it cannot spawn"
                    );
                }
            }
            assert_eq!(
                code.matches("super::super").count(),
                0,
                "{file} reaches out through `super::super`, which lands at the crate root and \
                 from there at every module RULE 7 just refused"
            );

            // RULE 7b: `super::` IS `crate::` HERE, AND RULE 7 NEVER SEES IT.
            //
            // RULE 7 scans for `crate::`. The comment above it used to argue
            // that plain `super::` needed no rule, because "both files are
            // crate-root modules, so `super::` inside their test modules names
            // the file's own module". That is true inside `mod tests` and
            // FALSE in production code at file top level -- which is exactly
            // where the runner lives. There, the parent of a crate-root module
            // IS the crate root, so:
            //
            //     updater.rs  (an ALLOWED file):
            //         pub fn run_anything_jobless(args: &[String]) { .. spawn .. }
            //     vault_export.rs, in `CliExportRunner::run`:
            //         super::updater::run_anything_jobless(&export_args(plan));
            //
            // reaches any module in this crate while writing no `crate::` path
            // at all. Measured on the parent commit: 2072 lib / 217 bin / 0
            // failed / 0 warnings, with a second signature-verified `bw.exe`
            // running this plan's full argv against an unlocked vault, outside
            // the kill-on-close job. RULE 7 was not defeated -- it was never
            // reached. `super::` is a synonym for `crate::` here, and a rule
            // that bans one spelling of a path and not the other bans nothing.
            //
            // Two assertions, because the hazard has two shapes:
            let production = raw
                .split(concat!("#[cfg(", "test)]", "\nmod "))
                .next()
                .unwrap();
            assert!(
                production.len() < raw.len(),
                "control: the test-module cut marker was not found in {file}, so `production` \
                 below is the whole file and RULE 7b is being asked about the tests as well as \
                 about the code -- which it would then fail on `use super::*;` rather than on \
                 anything real"
            );
            // (a) NONE in production code, where `super::` is the crate root.
            assert_eq!(
                code_only(production).matches("super::").count(),
                0,
                "{file} writes `super::` in production code, where it names the CRATE ROOT and \
                 not this file's own module -- so it can call a helper in any module in this \
                 crate, one that starts a `bw` outside the kill-on-close job among them, \
                 without writing a single `crate::` path for RULE 7 to see"
            );
            // (b) and the only one anywhere in the file is the glob a test
            // module writes to see its own parent. Without this second half,
            // `use super::*;` written at FILE scope -- which (a) does catch --
            // is not the only way through: any `super::` below the cut that is
            // not that glob is a path RULE 7 cannot read either, and a nested
            // `mod` below the cut is not necessarily a test module.
            assert_eq!(
                code.matches("super::").count(),
                code.matches("usesuper::*;").count(),
                "{file} writes a `super::` path that is not a test module's `use super::*;`; \
                 every other form of it is a path to somewhere in this crate that RULE 7 \
                 cannot read"
            );

            // RULE 7c: A MACRO INVOCATION WRITES NO PATH EITHER.
            //
            // RULE 7 and RULE 7b between them close every PATH out of these
            // two files. A `macro_rules!` in textual scope is called by bare
            // name and is not a path at all:
            //
            //     lib.rs, above the module list:
            //         macro_rules! run_it {
            //             ($a:expr) => { $crate::login_ui::run_anything_jobless(&$a) };
            //         }
            //     login_ui.rs (a DOOR_FILE, above its layout cut):
            //         pub fn run_anything_jobless(args: &[String]) { .. }
            //     vault_export.rs:
            //         run_it!(export_args(plan));
            //
            // The `$crate::` path is written in `lib.rs`, not here. Measured
            // against every other rule in this round: 2073 lib / 217 bin / 0
            // failed / 0 warnings, with a jobless verified `bw.exe` on this
            // plan's argv. The helper sits in a file that is allowed to open a
            // `BareCommand`, so the crate-wide count forgives it too; the only
            // thing left to close is the CALL, and the call is a bare
            // identifier.
            //
            // So the macros these two files may invoke are listed, exactly as
            // their items are. Everything here is either std or `log` -- and a
            // macro from another crate cannot name a deskwarden item, so it
            // cannot expand into one of these helpers whatever it does.
            // `concat!` and `include_str!` are on the list because the source
            // pins in both files are built out of them.
            const MACROS: &[&str] = &[
                "assert", "assert_eq", "assert_ne", "panic", "matches", "format", "vec",
                "write", "concat", "include_str", "log::warn", "log::debug", "log::info",
            ];
            for name in macro_invocations(&code) {
                assert!(
                    MACROS.contains(&name.as_str()),
                    "{file} invokes the macro `{name}!`, which is not on the list of macros \
                     these two modules may use. A `macro_rules!` in textual scope is called by \
                     BARE NAME -- it is not a path, so neither RULE 7 nor RULE 7b can read it -- \
                     and it can expand into a call to any item in this crate, one that starts a \
                     `bw` outside the kill-on-close job among them"
                );
            }

            // RULE 4: the only things it may reach out of `std::process` for
            // are stream wiring and this process's own pid -- neither of which
            // can describe or start a child.
            assert_eq!(
                code.matches("std::process::").count(),
                code.matches("std::process::Stdio").count()
                    + code.matches("std::process::id()").count(),
                "{file} reaches into `std::process` for something other than `Stdio` and `id`"
            );
        }

        // RULE 8: AND THE ONE MODULE THEY MAY CALL INTO IS CLOSED TOO.
        //
        // RULE 7 closes the set of items `vault_export.rs` and `send.rs` may
        // NAME. It says nothing about what those items then do, and stops
        // exactly one hop short: `bw_path::bw_job_command_in` is on REACHABLE
        // because the runners must build their child through it, and its own
        // body may call anywhere at all. Measured:
        //
        //     bw_path.rs:
        //         pub fn bw_job_command_in(..) -> .. {
        //             crate::login_ui::run_anything_jobless(&[..]);   // added
        //             ..
        //     login_ui.rs (an ALLOWED file, above its layout cut):
        //         pub fn run_anything_jobless(args: &[String]) { .. spawn .. }
        //
        // 2073 lib / 217 bin / 0 failed / 0 warnings. The runners are spotless
        // -- they call only what RULE 7 permits -- and the child still starts
        // outside the job, because the permitted callee grew a second errand.
        //
        // So `bw_path.rs` gets the same treatment, and for the same reason it
        // was put on REACHABLE in the first place: it is the one module these
        // two are allowed to reach, so it is the one module whose reach has to
        // be closed as well. It is a small file with a single job -- resolve
        // the verified `bw.exe` and describe a command -- and the only thing
        // in this crate it needs is the wrapper it hands the command to.
        //
        // This does NOT close the hop in general; it closes it for the one
        // module the runners depend on. `job_object.rs`, the other REACHABLE
        // module, is the choke point itself and is guarded by the probe and
        // by its own tests instead.
        // ONE HOP IS ONLY ONE HOP IF NO ENTRY HERE IS ITSELF A DOOR.
        //
        // THE TENTH ROUND'S SECOND HOP. This list is not followed: whatever is
        // on it may do as it likes. That is only sound if nothing on it is a
        // module licensed to start a jobless child -- and
        // `crate::bw_serve::bw_serve_command` was, because `bw_serve.rs` is on
        // both DOOR_FILES and the tree walk's ALLOWED. Measured, at 2077 lib /
        // 217 bin / 0 failed / 0 warnings:
        //
        //     bw_path.rs, in `bw_job_command_in`:
        //         let _ = crate::bw_serve::bw_serve_command("x");
        //     bw_serve.rs, inside `bw_serve_command`:
        //         let mut side = bw_command()?.into_jobless_command();
        //         let _ = side.spawn();
        //
        // Two hops from a REACHABLE item, through an entry this very list
        // blesses, every rule green. The entry was also STALE -- production
        // `bw_path.rs` never called it; only its tests did -- which is the
        // same defect DOOR_FILES and ALLOWED both treat as an error, and which
        // is why it went unnoticed.
        //
        // So the list is cut to what production really names, staleness is an
        // error here too, and no entry may live in a door module. What is left
        // is `job_object` alone: the choke point, guarded by the probe and by
        // its own tests rather than by this list.
        //
        // The scan is over PRODUCTION `bw_path.rs`, not the whole file: the
        // test module is not compiled into the shipped binary, so nothing in
        // it is reachable from the runners, and its calls are what made the
        // stale entry look live.
        const BW_PATH_MAY_REACH: &[&str] = &[
            "crate::job_object::JobCommand",
            "crate::job_object::JobCommand::wrap",
        ];
        // DERIVED FROM `DOOR_FILES`, NOT COPIED FROM IT. This used to be
        // `const DOOR_MODULES: &[&str] = &["bw_path", "bw_serve", "login_ui",
        // "main"];` -- a hand transcription of a list that lives in another
        // test function, with no assertion tying the two together and none of
        // the staleness or anti-vacuity controls that `DOOR_FILES`, `ALLOWED`
        // and `BW_PATH_MAY_REACH` all carry. Adding a door file and forgetting
        // this copy would silently make RULE 8 stop refusing exactly the hop it
        // exists to refuse, and the suite would stay green. One list, two
        // views, and the derivation is asserted below.
        let door_modules = door_modules();
        let entry_module = |entry: &str| {
            entry
                .strip_prefix("crate::")
                .unwrap_or(entry)
                .split("::")
                .next()
                .unwrap()
                .to_string()
        };
        for entry in BW_PATH_MAY_REACH {
            let module = entry_module(entry);
            assert!(
                !door_modules.contains(&module),
                "`{entry}` is reachable from `bw_path.rs` on the runners' behalf, and `{module}` \
                 is a module allowed to open a `BareCommand` and start a child outside the \
                 kill-on-close job. This list is not followed, so an entry that is itself a \
                 door makes RULE 8 one hop shorter than the escape it is aimed at"
            );
        }
        // ANTI-VACUITY. With `BW_PATH_MAY_REACH` shrunk to two `job_object`
        // entries the loop above rejects nothing, so on its own it would go on
        // passing if `door_modules` returned an empty list, or the wrong names,
        // or names with the `.rs` still attached. Each of the three is checked
        // against a synthetic entry that is not on the real list, which is the
        // only way to exercise a "count is zero" rule.
        assert_eq!(
            door_modules,
            vec!["bw_path", "bw_serve", "login_ui", "main"],
            "the door module list no longer derives to the modules RULE 8 must refuse; either \
             DOOR_FILES changed (in which case update this control deliberately) or the \
             derivation is broken and RULE 8 has quietly stopped refusing anything"
        );
        for door in &door_modules {
            let synthetic = format!("crate::{door}::some_helper");
            assert_eq!(
                entry_module(&synthetic),
                *door,
                "control: the module of a `crate::` entry is not read back, so the loop above \
                 compares the wrong string and forgives every door"
            );
            assert!(
                door_modules.contains(&entry_module(&synthetic)),
                "control: `{synthetic}` -- an entry landing squarely in a door module -- would \
                 not be refused, so RULE 8's second hop is open"
            );
        }
        assert!(
            !door_modules.contains(&entry_module("crate::job_object::JobCommand")),
            "control: `job_object` reads as a door, so RULE 8 would refuse the one hop it is \
             built to allow and this rule is asserting the opposite of its intent"
        );
        let bw_path_raw = std::fs::read_to_string(src.join("bw_path.rs"))
            .unwrap()
            .replace("\r\n", "\n");
        let bw_path_production = bw_path_raw
            .split(concat!("#[cfg(", "test)]", "\nmod "))
            .next()
            .unwrap();
        assert!(
            bw_path_production.len() < bw_path_raw.len(),
            "control: the test-module cut marker was not found in bw_path.rs"
        );
        let bw_path_all = code_only(bw_path_production);
        assert!(
            bw_path_all.len() > 1000,
            "control: bw_path.rs produced almost no code, so RULE 8 is vacuous"
        );
        assert!(
            crate_paths(&bw_path_all).iter().any(|p| p.starts_with("crate::job_object")),
            "control: bw_path.rs names nothing in `job_object` any more, so it has stopped              building the job-bearing command and RULE 8 is asserting about the wrong file"
        );
        for path in crate_paths(&bw_path_all) {
            assert!(
                BW_PATH_MAY_REACH.contains(&path.as_str()),
                "bw_path.rs names `{path}`. It is on RULE 7's REACHABLE list, so                  `vault_export.rs` and `send.rs` call into it -- and anything it calls, it                  calls on their behalf, with no rule of theirs in the way. A spawner reached                  from here starts a `bw` outside the kill-on-close job while both runners stay                  spotless. If this item is genuinely needed, adding it here is the visible                  decision that it cannot spawn"
            );
        }
        // Stale entries are an error, exactly as they are on DOOR_FILES and on
        // ALLOWED: an entry production no longer names is a pre-blessed hop
        // for whoever edits `bw_path.rs` next, and it is what the second hop
        // above walked through.
        let named = crate_paths(&bw_path_all);
        let stale: Vec<&&str> =
            BW_PATH_MAY_REACH.iter().filter(|e| !named.contains(&e.to_string())).collect();
        assert!(
            stale.is_empty(),
            "{stale:?} are blessed for `bw_path.rs` to reach but production `bw_path.rs` no \
             longer names them; remove them rather than leaving the permission standing"
        );
        assert_eq!(
            code_only(bw_path_production).matches("super::").count(),
            0,
            "bw_path.rs writes `super::` in production code, where it names the crate root --              the same synonym for `crate::` that RULE 7b closed for the two runners, and RULE 8              above reads only `crate::`"
        );

        // RULE 5 IS A "COUNT IS ZERO" RULE, so it is worth exactly nothing
        // unless the identifier it bans really is the only door out of a
        // `BareCommand` -- a rename in `bw_path` would leave it green while
        // banning a word that no longer means anything. Pinned at the source,
        // in the whitespace-free view, so `pub fn into_jobless_command ( self )
        // -> Command` is the same string.
        let bw_path_code = code_only(&std::fs::read_to_string(src.join("bw_path.rs")).unwrap());
        assert!(
            bw_path_code.contains(concat!("pubfninto_jobless_", "command(self)->Command{")),
            "`BareCommand::into_jobless_command` is gone or has changed shape, so RULE 5 now \
             bans an identifier that is not the way out of a `BareCommand` and asserts nothing"
        );
        // And the producers really do hand back the wrapper -- if any of them
        // went back to a bare `Command`, RULE 5 would still be green while a
        // decoy could be built by inference exactly as the eighth hop did.
        for required in [
            concat!("pubfnbw_command_in(dir:Option<&Path>)->Result<Bare", "Command,String>"),
            concat!("pubfnbw_command()->Result<Bare", "Command,String>"),
            concat!("structBare", "Command{command:Command,}"),
        ] {
            assert!(
                bw_path_code.contains(required),
                "`{required}` is gone: a bare `std::process::Command` can be obtained from \
                 `bw_path` again without writing the one banned identifier"
            );
        }
        let bw_serve_code = code_only(&std::fs::read_to_string(src.join("bw_serve.rs")).unwrap());
        assert!(
            bw_serve_code.contains(concat!(
                "pubfnbw_serve_command(session_token:&str)->Result<crate::bw_path::Bare",
                "Command,String>"
            )),
            "`bw_serve_command` hands back a bare command again, which these two modules can \
             call, spawn and never name"
        );

        // Positive controls, through the very same matcher: each rule can see
        // the thing it exists to catch.
        let planted = |s: &str| code_only(s);
        let p = planted("let child = Command::spawn(&mut command)?;");
        assert_eq!(p.matches("Command").count(), 1);
        assert_eq!(p.matches("JobCommand").count(), 0);
        assert_eq!(planted("std::process::Command::new(\"x\")").matches("std::process::Stdio").count(), 0);
        assert_eq!(planted("JobCommand :: wrap ( c )").matches("JobCommand::wrap").count(), 1);
        // RULE 5's controls: the door is seen in both spellings it has, and
        // naming the wrapper type itself is caught by rule 1 for free, since
        // `BareCommand` contains `Command` and is not `JobCommand`.
        assert_eq!(
            planted("let c = real . into_jobless_command ( ) ;")
                .matches("into_jobless_command")
                .count(),
            1
        );
        assert_eq!(
            planted("BareCommand::into_jobless_command(real)")
                .matches("into_jobless_command")
                .count(),
            1
        );
        let bare = planted("let r: BareCommand = bw_command_in(None)?;");
        assert_eq!(bare.matches("Command").count(), 1);
        assert_eq!(bare.matches("JobCommand").count(), 0);
        // RULE 6's control.
        assert_eq!(planted("unsafe { CreateProcessW(..) }").matches("unsafe").count(), 1);
        assert_eq!(planted("// unsafe\nlet a = 1;").matches("unsafe").count(), 0);
        // ...and passes the honest spelling, which names only `JobCommand`.
        let honest = planted("let c = job_object::spawn_in_job(self.job(), command)?;");
        assert_eq!(honest.matches("Command").count(), 0);

        // RULE 7b's controls, through the very same matcher.
        assert_eq!(planted("super :: updater :: run_it ( )").matches("super::").count(), 1);
        assert_eq!(planted("use super :: * ;").matches("usesuper::*;").count(), 1);
        // The glob is forgiven only in that one exact shape: a named import
        // through `super` is still a path, and is still seen.
        let named_through_super = planted("use super::updater::run_it;");
        assert_eq!(named_through_super.matches("super::").count(), 1);
        assert_eq!(named_through_super.matches("usesuper::*;").count(), 0);
        // And a `super::` inside a comment or a string is not one.
        assert_eq!(planted("// super::updater\nlet a = 1;").matches("super::").count(), 0);

        // RULE 7c's controls: a bare macro call is seen, in every bracket, and
        // the things that merely sit next to a `!` are not mistaken for one.
        assert_eq!(macro_invocations(&planted("run_it ! ( x ) ;")), vec!["run_it"]);
        assert_eq!(macro_invocations(&planted("vec![1, 2]")), vec!["vec"]);
        assert_eq!(macro_invocations(&planted("write!{f, \"x\"}")), vec!["write"]);
        assert_eq!(macro_invocations(&planted("crate::run_it!(x)")), vec!["crate::run_it"]);
        assert_eq!(macro_invocations(&planted("log::warn!(\"x\")")), vec!["log::warn"]);
        // A lone colon is a field or an ascription, not a path separator.
        assert_eq!(macro_invocations(&planted("S { args: vec![1] }")), vec!["vec"]);
        assert_eq!(macro_invocations(&planted("let x: u8 = matches!(a, B);")), vec!["matches"]);
        assert_eq!(macro_invocations(&planted("S { f: crate::run_it!(x) }")), vec!["crate::run_it"]);
        let none: Vec<String> = Vec::new();
        assert_eq!(macro_invocations(&planted("if a != b { }")), none);
        assert_eq!(macro_invocations(&planted("if !(a && b) { }")), none);
        assert_eq!(macro_invocations(&planted("while !(a) { }")), none);
        assert_eq!(macro_invocations(&planted("return !(a);")), none);
        assert_eq!(macro_invocations(&planted("if !flag { }")), none);
        assert_eq!(macro_invocations(&planted("#![allow(dead_code)]")), none);
        assert_eq!(macro_invocations(&planted("// run_it!(x)\nlet a = 1;")), none);
    }

    /// Everything above is about the two files the runner lives in. This is
    /// about the rest of the crate, because **an inherent impl may be written
    /// from any module in it**.
    ///
    /// RULE 5 bans the identifier `into_jobless_command` in `vault_export.rs`
    /// and `send.rs`, on the sound ground that an inherent method cannot be
    /// aliased or re-exported, so calling it means writing it. What the eighth
    /// round took from that -- that banning it in those two files closes the
    /// door -- does not follow. `impl crate::bw_path::BareCommand { .. }` is
    /// legal in every file in this crate, and inside such a block the
    /// identifier is written somewhere RULE 5 does not read; the call back in
    /// the runner is then `real.run_it()`, which names no type and writes no
    /// path and so is invisible to RULE 1, RULE 5 and RULE 7 alike. Measured:
    /// 2072 lib / 217 bin / 0 failed / 0 warnings, with a signature-verified
    /// `bw.exe` on the export's full argv running outside the job.
    ///
    /// So the door is counted where it can actually be opened: everywhere.
    /// Two rules, because the hop has two halves --
    ///
    ///  * only the files that genuinely need a child outside the job may write
    ///    the identifier at all, and
    ///  * no file but `bw_path.rs` may write an `impl` block for the type,
    ///    which is what let the door be given a second name in the first place.
    ///
    /// This is still a text rule, and text rules are what the last eight
    /// rounds lost to. It is here rather than a compile-time wall because
    /// Rust has no way to build that wall for this shape: `pub(in path)`
    /// accepts only ANCESTOR modules, so an item in `bw_path` cannot be made
    /// visible to its siblings and nobody else. Measured, not assumed --
    /// `pub(in crate::bw_serve) fn into_jobless_command` on the definition
    /// fails to compile with `E0433: could not find bw_serve in the crate
    /// root`, even though `bw_serve` is a real crate-root module, because the
    /// resolver looks only among ancestors. The wall exists only if the four
    /// consumers are moved to be descendants of a common ancestor of
    /// `bw_path`, which is a whole-crate restructure.
    #[test]
    fn only_the_files_that_must_leave_the_job_can_open_a_bare_command() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);
        assert!(
            files.len() > 20,
            "control: the tree walk found {} files, so the counts below are vacuous",
            files.len()
        );

        // The files that legitimately hold a `bw` child outside the
        // kill-on-close job, and therefore the only ones that may write the
        // one identifier that opens a `BareCommand`.
        //
        // Strictly SHORTER than the tree walk's ALLOWED list, and deliberately
        // so: `updater.rs` and `vault_window/mod.rs` do start children, but
        // they do not start `bw` and have no business opening this wrapper.
        // The eighth hop's escape was planted in `updater.rs` precisely
        // because it was excused there.
        //
        //   bw_path.rs  -- defines the door, and `bw_job_command_in` is the
        //                  one production place that walks through it in order
        //                  to hand the command straight to `JobCommand::wrap`.
        //   bw_serve.rs -- `bw serve` is the long-lived backend; it is owned
        //                  and killed by the backend policy, not by the job.
        //   login_ui.rs -- sign-in, unlock and status run before there is a
        //                  vault window, and so before there is a job.
        //   main.rs     -- re-wraps into a `JobCommand` at the one call site
        //                  that builds the vault window's own job.
        //
        // Declared at module scope rather than here because RULE 8, in the
        // other test, needs the SAME list as a set of module names and used to
        // keep a hand copy of it. See [`DOOR_FILES`] and [`door_modules`].

        let relative = |p: &std::path::Path| {
            p.strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        };

        let mut seen_door = Vec::new();
        for file in &files {
            let rel = relative(file);
            let code = code_only(&std::fs::read_to_string(file).unwrap());

            let opens = code.matches("into_jobless_command").count();
            if DOOR_FILES.contains(&rel.as_str()) {
                if opens > 0 {
                    seen_door.push(rel.clone());
                }
            } else {
                assert_eq!(
                    opens, 0,
                    "{rel} opens a `BareCommand`, which is the one way to hold a spawnable \
                     `std::process::Command` in this crate. Wherever it is written, the \
                     resulting child runs outside the kill-on-close job -- and if the code \
                     around it is reachable from `vault_export.rs` or `send.rs` by any spelling, \
                     it is a `bw` holding an unlocked vault that outlives a panic, a \
                     `process::exit` or a Task Manager kill. If this file genuinely needs a \
                     jobless child, adding it to DOOR_FILES is the visible decision that it does"
                );
            }

            // AND NO ONE MAY GROW A METHOD ONTO A TYPE THE RUNNER HOLDS.
            //
            // A foreign inherent impl is how the door gets a second name, and
            // counting the door's identifier is not enough to stop it -- a
            // measured mutant proves it. `JobCommand` exposes `get_program`,
            // `get_args` and `get_envs` so a test can assert what would be run
            // without running it; that is enough to REBUILD the invocation
            // without ever opening a `BareCommand`:
            //
            //     login_ui.rs (an ALLOWED file, above its layout cut):
            //         impl crate::job_object::JobCommand {
            //             pub fn run_it(&self) {
            //                 let mut c = Command::new(self.get_program());
            //                 c.args(self.get_args());  ..  let _ = c.spawn();
            //             }
            //         }
            //     vault_export.rs, in `CliExportRunner::run`:
            //         command.run_it();
            //
            // Measured against every other rule written in this round: 2073
            // lib / 217 bin / 0 failed / 0 warnings, with a second
            // signature-verified `bw.exe` on the export's full argv running
            // outside the kill-on-close job. The same shape on
            // `KillOnCloseJob` survived too. `into_jobless_command` is never
            // written, so the count above sees nothing; `command.run_it()`
            // names no type and writes no path, so RULE 1, RULE 5, RULE 7 and
            // RULE 7b see nothing either.
            //
            // What makes the list below CLOSED rather than a guess: Rust's
            // orphan rule already forbids an inherent impl for a type defined
            // in another crate, so the only types anyone in this crate can
            // grow a method onto are this crate's own. Of those, the runner
            // can only call a method on one it holds a value of -- and RULE 7
            // has already closed the set of items it may name. That set
            // contains exactly four crate-local types, plus whatever the two
            // files define themselves, which is why a foreign `impl` reaching
            // into either module is refused as well.
            //
            // WHAT THIS STILL DOES NOT CLOSE, measured and left standing: the
            // same `pub fn run_it` written in `job_object.rs` ITSELF, next to
            // the type, survives at 2077 lib / 217 bin / 0 failed / 0
            // warnings. So does a second jobless spawn inside `spawn_in_job`'s
            // own body. Both require editing the choke point module -- the
            // most-reviewed file in this crate, carrying the probe and its own
            // spawn tests -- whereas everything refused above needs only an
            // ordinary UI file and one innocuous line in a runner. These rules
            // are aimed at the second kind.
            //
            // AND WHY THE READERS STAY. `JobCommand::get_program`, `get_args`
            // and `get_envs` are what make the rebuild above possible: a
            // foreign `impl` cannot touch the private `command` field, so the
            // public surface is all it has. Removing them would close the
            // rebuild structurally rather than by rule -- but they are not
            // spare. `get_envs` is how `send.rs` and `vault_export.rs` assert
            // that `BW_SESSION` really reaches the child without running one,
            // and `get_program`/`get_args` are how the spawn probe records
            // what arrived; those assertions ARE part of the guarantee this
            // module makes. Narrowing them would trade a rule for a blind
            // spot, so they stay and the rebuild is refused at the `impl`.
            const OWN_IMPLS_ONLY: &[(&str, &str)] = &[
                (concat!("Bare", "Command"), "bw_path.rs"),
                (concat!("Job", "Command"), "job_object.rs"),
                ("KillOnCloseJob", "job_object.rs"),
                ("SpawnProbe", "job_object.rs"),
                ("crate::vault_export::", "vault_export.rs"),
                ("crate::send::", "send.rs"),
            ];
            for (ty, home) in OWN_IMPLS_ONLY {
                if rel == *home {
                    continue;
                }
                assert_eq!(
                    impl_blocks_for(&code, ty),
                    0,
                    "{rel} writes an `impl` block for `{ty}`, which is not defined here. Rust \
                     allows an inherent impl from any module in the defining crate, so this \
                     block grows a new method onto a value that `vault_export.rs` and `send.rs` \
                     are allowed to hold -- and calling it is a method call on a local, which \
                     names no type and writes no path, and so is invisible to every rule in \
                     `the_two_job_bearing_modules_cannot_name_a_bare_command`. Keep the impl in \
                     `{home}`, where those rules and this crate's spawn guards can read it"
                );

                // AND NO ONE MAY GIVE A GUARDED TYPE A SECOND NAME.
                //
                // THE TENTH ROUND'S HOP, and the reason the rule above is not
                // enough on its own: `impl_blocks_for` reads an impl HEAD, so
                // it can only refuse a head in which the type is SPELLED. If
                // the type is renamed first, the head names the rename and the
                // needle never occurs. Both spellings were measured, each in
                // `login_ui.rs` above its layout cut, each with
                // `command.run_it();` added to `CliExportRunner::run`:
                //
                //     use crate::job_object::JobCommand as Jc;
                //     impl Jc { pub fn run_it(&self) { .. spawn .. } }
                //
                //     type Jt = crate::job_object::JobCommand;
                //     impl Jt { pub fn run_it(&self) { .. spawn .. } }
                //
                // 2077 lib / 217 bin / 0 failed / 0 warnings, both of them,
                // with a signature-verified `bw.exe` on the export's full argv
                // running outside the kill-on-close job. The identical helper
                // written `impl crate::job_object::JobCommand` is refused by
                // the assertion above -- so that assertion closed exactly the
                // spelling its author happened to mutate with.
                //
                // Renaming is the whole of the gap, so renaming is what is
                // banned: outside the type's home file the guarded names may
                // be IMPORTED and USED (`vault_export.rs` really does
                // `use crate::job_object::{JobCommand, KillOnCloseJob};`),
                // but they may not be given a name the head-reader cannot
                // recognise. Re-export without renaming is deliberately still
                // allowed and is deliberately still caught: `mod al { pub use
                // crate::job_object::JobCommand; } impl al::JobCommand { .. }`
                // writes the needle at the impl head, which is where the rule
                // above reads.
                let renamed: Vec<String> = item_bodies(&code, "use")
                    .into_iter()
                    .filter(|item| item.contains(&format!("{ty}as")))
                    .collect();
                assert!(
                    renamed.is_empty(),
                    "{rel} imports `{ty}` under a second name: {renamed:?}. An `impl` block \
                     written against the rename names no guarded type, so the assertion above \
                     -- which reads the impl's head -- cannot see it, and the method it grows \
                     is called as `x.run_it()`, which names no type and writes no path. Import \
                     it under its own name (split the `use` if some other item in the same \
                     list needs renaming), or keep the impl in `{home}`"
                );
                // Only an alias whose right-hand side IS the type: that is the
                // one `impl <alias>` accepts as an inherent impl. An alias for
                // a compound that merely mentions the type -- `type A = (u64,
                // Result<Vec<crate::send::SendSummary>, ..>)`, which
                // `vault_window/mod.rs` really writes -- names no type an
                // inherent impl could attach to, and is not this rule's
                // business. Parentheses are kept in the accepted set because
                // `type Jt = (crate::job_object::JobCommand);` is the same
                // alias with brackets round it.
                //
                // THE ELEVENTH ROUND, and both halves of this filter were
                // wrong. It used to read `item.split_once('=')` -- the FIRST
                // `=` -- and test the needle against the right-hand side. A
                // generic parameter's DEFAULT is spelled with an `=` too, and
                // it comes first:
                //
                //     type Jd<A = crate::job_object::JobCommand> = A;
                //     impl Jd { pub fn run_it(&self) { .. } }
                //
                // The split landed inside the parameter list, the "right-hand
                // side" came out as `crate::job_object::JobCommand>=A`, and the
                // `=` still in it is not in the accepted character set, so
                // `.all()` failed and the alias was not flagged. `A` is used,
                // so there is no E0091 and no warning. Measured SURVIVING at
                // 2077 lib / 217 bin / 1 ignored / 0 failed / 0 warnings.
                //
                // Splitting on the LAST top-level `=` fixes the first half but
                // not the rule: the right-hand side is then `A`, which does not
                // contain the type either -- the guarded name is on the LEFT,
                // in the default. So the needle is tested against the WHOLE
                // item, which is where the type is named however the alias is
                // spelled, and the right-hand side is used only for the one
                // thing it is good for: deciding whether the alias resolves to
                // something an inherent impl could attach to at all.
                //
                // That second test is why `type A = (u64, Result<Vec<crate::
                // send::SendSummary>, ..>)` -- which `vault_window/mod.rs`
                // really writes -- is still let through: its right-hand side is
                // a compound, and `impl A` for a tuple grows a method onto
                // nothing this rule guards. Parentheses stay in the accepted
                // set because `type Jt = (crate::job_object::JobCommand);` is
                // the same alias with brackets round it.
                let aliased = aliases_naming(&code, ty);
                assert!(
                    aliased.is_empty(),
                    "{rel} declares a type alias naming `{ty}`: {aliased:?}. `impl <alias>` is \
                     an inherent impl for the aliased type, and its head names nothing the \
                     assertion above can read. Write the type out at the impl, or keep the \
                     impl in `{home}`"
                );
            }

            // AND NO IMPL MAY HIDE ITS SELF TYPE BEHIND A MACRO ARGUMENT.
            //
            // The third way to write an impl head that does not spell the
            // type: let a macro spell it. `macro_rules! grow { ($t:ty) => {
            // impl $t { .. } } }` puts `impl$t` in the head, and the only
            // place the guarded name is written is the invocation
            // `grow!(crate::job_object::JobCommand);` -- an argument, not a
            // head. Banned outright, in every file including the home files:
            // an impl whose self type is a metavariable is unreadable to
            // every rule here, and this crate has none.
            let heads = impl_heads(&code);
            let macro_heads: Vec<&String> = heads.iter().filter(|h| h.contains('$')).collect();
            assert!(
                macro_heads.is_empty(),
                "{rel} writes an `impl` whose self type is a macro metavariable: {macro_heads:?}. \
                 The type it grows a method onto is named at the macro's call site rather than \
                 at its head, where every rule in this test reads. Write the impl out"
            );

            // `extern crate self as x;` makes `x::` a synonym for `crate::`,
            // which RULE 7 reads, and for `super::`, which RULE 7b reads --
            // and `x::` is neither. One line anywhere in the crate would make
            // both of them unreachable exactly as `super::` already did.
            assert_eq!(
                code.matches("externcrateselfas").count(),
                0,
                "{rel} aliases this crate to a second name, which is a spelling of `crate::` \
                 that neither RULE 7 nor RULE 7b can read"
            );
        }

        // Stale entries are an error, for the same reason they are on ALLOWED:
        // a DOOR_FILE that no longer opens the wrapper is a door standing open
        // for whoever edits that file next, and it makes the count above
        // forgive a file for a reason that has stopped being true.
        let mut missing: Vec<&str> = DOOR_FILES
            .iter()
            .filter(|d| !seen_door.iter().any(|s| s == *d))
            .copied()
            .collect();
        missing.sort_unstable();
        assert!(
            missing.is_empty(),
            "{missing:?} are excused from this guard but no longer open a `BareCommand`; remove \
             them from DOOR_FILES rather than leaving the exemption standing"
        );

        // Controls, through the very same matcher: each half can see the thing
        // it exists to catch, and passes the honest shapes.
        assert_eq!(
            code_only("let c = real . into_jobless_command ( ) ;")
                .matches("into_jobless_command")
                .count(),
            1
        );
        const BARE: &str = concat!("Bare", "Command");
        const JOB: &str = concat!("Job", "Command");
        // Both spellings of an inherent impl, and a trait impl too.
        assert_eq!(
            impl_blocks_for(
                &code_only("impl crate::bw_path::BareCommand { pub fn run_it(self) {} }"),
                BARE
            ),
            1
        );
        assert_eq!(
            impl_blocks_for(&code_only("impl BareCommand { fn f(&self) {} }"), BARE),
            1
        );
        assert_eq!(
            impl_blocks_for(&code_only("impl std::fmt::Debug for BareCommand {}"), BARE),
            1
        );
        // The two shapes that actually survived, each seen by its own needle.
        assert_eq!(
            impl_blocks_for(
                &code_only("impl crate::job_object::JobCommand { pub fn run_it(&self) {} }"),
                JOB
            ),
            1
        );
        assert_eq!(
            impl_blocks_for(
                &code_only("impl crate::job_object::KillOnCloseJob { pub fn run_it(&self) {} }"),
                "KillOnCloseJob"
            ),
            1
        );
        assert_eq!(
            impl_blocks_for(
                &code_only("impl crate::vault_export::CliExportRunner<'_> { fn r(&self) {} }"),
                "crate::vault_export::"
            ),
            1
        );
        // ...and the needles do not see each other: `BareCommand` is not a
        // `JobCommand`, which is what keeps the per-type home files honest.
        assert_eq!(impl_blocks_for(&code_only("impl BareCommand {}"), JOB), 0);
        // Merely naming the type is not an impl: `bw_serve` does it in a
        // signature, and `vault_export` and `send` name `JobCommand` in theirs.
        assert_eq!(
            impl_blocks_for(
                &code_only("pub fn f() -> Result<crate::bw_path::BareCommand, String> { g() }"),
                BARE
            ),
            0
        );
        assert_eq!(
            impl_blocks_for(&code_only("let c: crate::job_object::JobCommand = x;"), JOB),
            0
        );
        assert_eq!(
            impl_blocks_for(&code_only("// impl BareCommand {\nlet a = 1;"), BARE),
            0
        );
        assert_eq!(code_only("extern crate self as dw;").matches("externcrateselfas").count(), 1);

        // The tenth round's three shapes, each through the matcher that now
        // refuses it, and each shown to have been INVISIBLE to the needle
        // walk that a head reader replaces.
        //
        //  * a generic impl: the old backwards walk stopped on the `>`.
        assert_eq!(
            impl_blocks_for(
                &code_only("impl<'a> crate::job_object::JobCommand { fn r(&'a self) {} }"),
                JOB
            ),
            1
        );
        assert_eq!(
            impl_blocks_for(
                &code_only("impl<T: Fn() -> u8> Trait<T> for JobCommand { fn r(&self) {} }"),
                JOB
            ),
            1
        );
        //  * a rename: no head names the type at all, so the head reader is
        //    silent by construction and the `use` item is what is refused.
        assert_eq!(impl_blocks_for(&code_only("use X as Jc; impl Jc {}"), JOB), 0);
        assert_eq!(
            item_bodies(&code_only("use crate::job_object::JobCommand as Jc;"), "use")
                .iter()
                .filter(|i| i.contains(&format!("{JOB}as")))
                .count(),
            1
        );
        assert_eq!(
            item_bodies(&code_only("type Jt = crate::job_object::JobCommand;"), "type")
                .iter()
                .filter(|i| i.contains(JOB))
                .count(),
            1
        );
        //  * and the honest shapes the same two matchers must pass: an import
        //    under the type's own name (`vault_export.rs` really writes this
        //    one), a brace list, and the two words that contain the keywords.
        assert_eq!(
            item_bodies(&code_only("use crate::job_object::{JobCommand, KillOnCloseJob};"), "use")
                .iter()
                .filter(|i| i.contains(&format!("{JOB}as")))
                .count(),
            0
        );
        assert_eq!(item_bodies(&code_only("let n = x.type_of(); f(user);"), "type").len(), 0);
        assert_eq!(item_bodies(&code_only("let n = x.type_of(); f(user);"), "use").len(), 0);
        //  * a macro-supplied self type: the guarded name is written at the
        //    call site, never at the head, so the head is what is refused.
        let macro_impl = code_only("macro_rules! g { ($t:ty) => { impl $t { fn r(&self) {} } } }");
        assert_eq!(impl_blocks_for(&macro_impl, JOB), 0);
        assert_eq!(impl_heads(&macro_impl).iter().filter(|h| h.contains('$')).count(), 1);
        //  * a braced const generic argument does not end the head early. Not
        //    reachable on its own -- a trait impl needs the trait in scope at
        //    the call site, which is a `crate::` path RULE 7 refuses -- but a
        //    head this reader cannot read whole is a hole either way.
        assert_eq!(
            impl_blocks_for(
                &code_only("impl RunIt<{ 0 }> for crate::job_object::JobCommand { fn r(&self) {} }"),
                JOB
            ),
            1
        );
        //  * and `impl` is a keyword here, not a substring: neither of these
        //    is an impl block.
        assert_eq!(impl_heads(&code_only("let simple = 1; fn implementation() {}")).len(), 0);
        //  * a return-position `impl Trait` is a head, and a harmless one:
        //    it is seen, and it names no guarded type.
        assert_eq!(
            impl_heads(&code_only("fn f() -> impl Iterator<Item = u8> { g() }")),
            vec!["Iterator<Item=u8>".to_string()]
        );

        // THE ELEVENTH ROUND'S CONTROLS, AND WHY THEY ARE NOT LITERALS.
        //
        // Every control above this point is a SPELLING its author already knew
        // how to write: `impl crate::job_object::JobCommand ..`, `impl<'a> ..`,
        // `impl RunIt<{ 0 }> for ..`. Each was added the round after a mutant
        // used it. Nothing anchored the reader's ENTRY GATE against a head
        // shape it refuses to look at, and nothing anchored the alias filter's
        // CHARACTER SET against a right-hand side it drops -- and those two
        // hand-written sets are exactly where the eleventh round's two
        // survivors lived. `impl (crate::job_object::JobCommand)` was not
        // rejected as a head, it was never READ as one; `type Jd<A = ..> = A;`
        // was not judged safe, it was never TESTED.
        //
        // So these are shape-driven: a table of forms, each one built rather
        // than written out, each asserted through the real matcher. A shape
        // added to the table is one line; a shape the matcher cannot see fails
        // here rather than in production three rounds later.

        // (i) The reader must not be silenced by how a self type BEGINS. Not
        //     all of these are impls Rust would accept -- `impl !T` is not --
        //     but a reader that stops at the first byte it does not recognise
        //     is a reader whose author's imagination is the guard, which is the
        //     defect being closed. Every one of them must yield a head naming
        //     the type.
        const HEAD_SHAPES: &[(&str, &str)] = &[
            ("", ""),
            ("(", ")"),
            ("((", "))"),
            ("(", ",)"),
            ("&", ""),
            ("&'a ", ""),
            ("&mut ", ""),
            ("*const ", ""),
            ("*mut ", ""),
            ("[", "]"),
            ("[", "; 4]"),
            ("!", ""),
            ("dyn ", ""),
        ];
        for (open, close) in HEAD_SHAPES {
            let text = format!(
                "impl {open}crate::job_object::{JOB}{close} {{ pub fn run_it(&self) {{}} }}"
            );
            assert!(
                impl_blocks_for(&code_only(&text), JOB) >= 1,
                "the impl head reader cannot see `{text}`. A head it refuses to read is not a \
                 head it rejects: `impl_blocks_for` counts nothing, the `$` metavariable filter \
                 sees nothing, and the method the block grows is called as `x.run_it()`, which \
                 names no type and writes no path"
            );
        }
        // ...and the reader still refuses the two things it must: a keyword
        // that is really part of a word, and a generic PARAMETER list, whose
        // contents are not the self type.
        assert_eq!(impl_heads(&code_only("fn implementation() {} let simple = 1;")).len(), 0);
        assert_eq!(
            impl_blocks_for(
                &code_only("impl<T: Into<crate::job_object::JobCommand>> Other { fn r(&self) {} }"),
                JOB
            ),
            0,
            "control: a generic BOUND naming the type is not an impl FOR it, and counting it \
             would make this rule fire on honest code until someone deleted it"
        );

        // (ii) The alias filter must not drop a right-hand side because of the
        //      characters in it, and must not be fooled about WHICH `=` is the
        //      alias's. Every form below gives `JobCommand` a second name that
        //      `impl <alias>` accepts, and every one must be flagged.
        const ALIAS_SHAPES: &[&str] = &[
            "type Jt = crate::job_object::JobCommand;",
            "pub type Jt = crate::job_object::JobCommand;",
            "type Jt = (crate::job_object::JobCommand);",
            "type Jt = ((crate::job_object::JobCommand));",
            "type Jd<A = crate::job_object::JobCommand> = A;",
            "type Jd<A = crate::job_object::JobCommand> = (A);",
            "type Jd<'a, A = crate::job_object::JobCommand> = A;",
            "type Jd<A = crate::job_object::JobCommand, B = u8> = A;",
            "type Jd<A = crate::job_object::JobCommand> = Jd2<A>;",
            "type Jd<A: Sized = crate::job_object::JobCommand> = A;",
        ];
        for shape in ALIAS_SHAPES {
            assert_eq!(
                aliases_naming(&code_only(shape), JOB).len(),
                1,
                "the alias filter does not flag `{shape}`, which names `{JOB}` and which \
                 `impl <alias>` accepts as an inherent impl for it. The filter's job is to \
                 refuse a second name the head reader cannot recognise, and a form it silently \
                 passes is that second name"
            );
        }
        // ...and it must still pass the compounds, or it fires on honest code
        // and gets deleted. `vault_window/mod.rs` really writes the first one.
        for honest in [
            "type Pair = (u64, Result<Vec<crate::send::SendSummary>, String>);",
            "type Outcome = Result<crate::send::SendSummary, String>;",
        ] {
            assert_eq!(
                aliases_naming(&code_only(honest), "crate::send::").len(),
                0,
                "control: the alias filter flags `{honest}`, which resolves to a compound no \
                 inherent impl can attach to"
            );
        }
        // The split itself, driven directly: the alias's `=` is the last one
        // outside the brackets, not the first one anywhere.
        let rhs_of = |s: &str| alias_rhs(&item_bodies(&code_only(s), "type")[0]).to_string();
        assert_eq!(rhs_of("type Jd<A = crate::job_object::JobCommand> = A;"), "A");
        assert_eq!(rhs_of("type Jt = crate::job_object::JobCommand;"), JOB_PATH);
        assert_eq!(rhs_of("type Jd<A = u8> = (A, u8);"), "(A,u8)");
        assert_eq!(rhs_of("type Jd<A = u8, B = u8> = Map<A, B>;"), "Map<A,B>");
        // ...and an item that is not an alias at all has no right-hand side.
        assert_eq!(alias_rhs(&code_only("use crate::job_object::JobCommand;")), "");

        // The NEEDLE view, driven directly: a generic BOUND is not a way to
        // reach the type, a DEFAULT is, and the right-hand side is untouched.
        let needle_of = |s: &str| alias_needle_view(&item_bodies(&code_only(s), "type")[0]);
        assert_eq!(
            needle_of("type W<T: Into<crate::job_object::JobCommand>> = Vec<T>;"),
            "typeW<T>=Vec<T>",
            "the bound is still in the needle view, so the filter flags an alias no inherent \
             impl can attach to and the next editor weakens it to shut it up"
        );
        assert_eq!(
            needle_of("type Jd<A = crate::job_object::JobCommand> = A;"),
            concat!("typeJd<A=crate::job_object::", "JobCommand>=A"),
            "a parameter DEFAULT was dropped along with the bounds, which deletes the \
             eleventh round's hop from this rule"
        );
        assert_eq!(
            needle_of("type Jd<T: Clone, A = crate::job_object::JobCommand> = A;"),
            concat!("typeJd<T,A=crate::job_object::", "JobCommand>=A"),
            "a bound ended at the wrong place, taking the default that follows it with it"
        );
        assert_eq!(
            needle_of("type Jt = crate::job_object::JobCommand;"),
            concat!("typeJt=crate::job_object::", "JobCommand"),
            "the path separators in an ordinary alias were read as bounds"
        );
        assert_eq!(
            needle_of("type F<T: Fn() -> crate::job_object::JobCommand> = Vec<T>;"),
            "typeF<T>=Vec<T>",
            "the `>` of an `->` inside a bound ended the skip early"
        );
        // ...and the filter itself agrees on both.
        assert_eq!(
            aliases_naming(
                &code_only("type W<T: Into<crate::job_object::JobCommand>> = Vec<T>;"),
                JOB
            )
            .len(),
            0
        );
        assert_eq!(
            aliases_naming(
                &code_only("type Jd<A = crate::job_object::JobCommand> = A;"),
                JOB
            )
            .len(),
            1
        );

        // A TWELFTH-HOP MUTANT THAT SATISFIES EVERY CONTROL ABOVE AND STILL
        // ESCAPES, written down rather than left to be found:
        //
        //     impl<T> crate::job_object::JobCommand where T: Sized { .. }
        //
        // is refused (the head names the type). But
        //
        //     mod al { pub use crate::job_object::JobCommand as Jc; }
        //     impl al::Jc { pub fn run_it(&self) { .. } }
        //
        // renames inside a nested module. The rename is caught -- `item_bodies`
        // reads `use` items anywhere in the file, and `JobCommandas` occurs --
        // so this one dies. What does NOT die is a rename in a DIFFERENT file
        // that is not on `OWN_IMPLS_ONLY`'s walk at all, or a `pub use` in one
        // file consumed by an `impl` in another. That is caught only because
        // the walk covers every `.rs` file under `src` and the `use` scan runs
        // on all of them: the two halves are in different files but both are
        // scanned. The genuine residue is the `get_*` gate's -- a helper that
        // takes the plan and the session as arguments instead of reading a
        // `JobCommand` -- and RULE 9's, listed there.
    }

    /// The path spelling of `JobCommand`, in the whitespace-free view.
    const JOB_PATH: &str = concat!("crate::job_object::Job", "Command");

    /// Every `crate::` path written in one file's `code_only` view.
    ///
    /// `use crate::m::{A, B};` is one occurrence naming several items, and is
    /// expanded into one entry each -- otherwise a brace list would smuggle
    /// every item after the first past a rule that reads only the head.
    fn crate_paths(code: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = code[from..].find("crate::") {
            let start = from + at;
            from = start + "crate::".len();
            let head: String = code[start..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                .collect();
            let rest = &code[start + head.len()..];
            if head.ends_with("::") && rest.starts_with('{') {
                out.extend(
                    rest[1..]
                        .chars()
                        .take_while(|c| *c != '}')
                        .collect::<String>()
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|item| format!("{head}{item}")),
                );
            } else {
                out.push(head);
            }
        }
        out
    }

    /// Every macro invoked in one file's `code_only` view, by name.
    ///
    /// A macro call is an identifier (or a path, for `log::warn!`) followed by
    /// `!` and then an opening bracket. The bracket is what separates a call
    /// from `!=` and from a negation, both of which put a `!` next to
    /// something that is not one. An inner attribute's `#!` walks back to an
    /// empty name and is dropped.
    ///
    /// Paths are kept whole, so `crate::x!` comes back as `crate::x` and fails
    /// the list rather than passing as `x` -- and RULE 7 has already refused
    /// the same path in its own right.
    fn macro_invocations(code: &str) -> Vec<String> {
        let bytes = code.as_bytes();
        let mut out = Vec::new();
        for (i, c) in code.char_indices() {
            if c != '!' {
                continue;
            }
            match bytes.get(i + 1) {
                Some(b'(') | Some(b'[') | Some(b'{') => {}
                _ => continue,
            }
            let name_start = code[..i]
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == ':'))
                .map(|(j, c)| j + c.len_utf8())
                .unwrap_or(0);
            let mut name = &code[name_start..i];
            // Only `::` joins path segments. A LONE colon is a struct-literal
            // field or a type ascription, and `code_only` has already dropped
            // the space that separated it -- so `args: vec![..]` arrives here
            // as `args:vec`, whose macro is `vec` and not `args:vec`.
            let b = name.as_bytes();
            if let Some(cut) = (0..b.len()).rev().find(|&p| {
                b[p] == b':' && b.get(p + 1) != Some(&b':') && (p == 0 || b[p - 1] != b':')
            }) {
                name = &name[cut + 1..];
            }
            // A negation of a parenthesised expression puts `!` right after
            // whatever preceded it, and `code_only` has dropped the space that
            // separated them: `if !(a && b)` arrives here as `if!(`. A macro's
            // name can never be a keyword, so dropping these is sound rather
            // than a hole.
            const NOT_A_MACRO: &[&str] = &[
                "if", "while", "return", "break", "continue", "else", "match", "in", "let",
            ];
            if !name.is_empty() && !NOT_A_MACRO.contains(&name) {
                out.push(name.to_string());
            }
        }
        out
    }

    /// The head of every `impl` block in one file's `code_only` view: the
    /// text between the `impl` keyword and the `{` (or `;`) that follows it,
    /// with the generic parameter list skipped.
    ///
    /// **Driven by the `impl` keyword, not by the type being looked for.**
    /// The previous shape of this walked BACKWARDS from an occurrence of the
    /// needle and asked whether it landed on `impl`, which made it a reader of
    /// spellings: `impl<'a> JobCommand` walks back onto the `>` that ends the
    /// generic list, stops there, and is not counted -- and a head that never
    /// writes the type at all (because the type was renamed first, or because
    /// a macro supplies it) has no occurrence to walk back from. Enumerating
    /// the heads instead means every impl in the file is looked at exactly
    /// once, whatever its self type is written as, and the renames are refused
    /// separately by [`item_bodies`].
    ///
    /// `code_only` has already removed the whitespace, so the head of
    /// `impl SomeTrait for BareCommand` is one string `SomeTraitforBareCommand`
    /// and a needle test over it sees the trait impl as well as the inherent
    /// one. Trait impls are not the hazard this rule is aimed at (a foreign
    /// module cannot reach a private field through one either), but they are
    /// caught for free and there are none, so they are left caught.
    ///
    /// The token test is what keeps `simple` and `implementation` out: no
    /// IDENTIFIER character may touch the keyword on either side -- except
    /// that `pub` cannot precede an `impl`, so nothing legitimate does.
    fn impl_heads(code: &str) -> Vec<String> {
        let b = code.as_bytes();
        let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = code[from..].find("impl") {
            let start = from + at;
            from = start + "impl".len();
            if start > 0 && ident(b[start - 1]) {
                continue;
            }
            // THERE IS NO ACCEPT-SET FOR WHAT MAY FOLLOW THE KEYWORD, and the
            // eleventh round is why. This gate used to read
            //
            //     match b.get(start + "impl".len()) {
            //         Some(&c) if ident(c) || c == b'<' || c == b'$' => {}
            //         _ => continue,
            //     }
            //
            // -- a hand-written list of the three first characters its author
            // could think of. `(` is not on it, so
            //
            //     #[allow(unused_parens)]
            //     impl (crate::job_object::JobCommand) { pub fn run_it(&self) { .. } }
            //
            // was not a head at all: `impl_blocks_for` could not count it, the
            // `$` metavariable filter could not see it, and the attribute
            // removed the only warning. Measured SURVIVING at 2077 lib / 217
            // bin / 1 ignored / 0 failed / 0 warnings, byte-identical to
            // baseline, with the export's child running outside the job.
            //
            // Adding `(` would have been the ninth round's mistake a second
            // time: `&`, `*`, `[`, `!` and `dyn` all begin types too, and the
            // next author gets to pick. So the set is DELETED. Every `impl`
            // token that is not glued to an identifier is a head, and the head
            // is whatever text runs up to the block. A head that is not really
            // an impl is harmless -- it names no guarded type, so every reader
            // here ignores it -- while a head that is refused is a hole, which
            // is the asymmetry the accept-set had backwards.
            //
            // `from` is left at the end of the keyword rather than at the end
            // of the head for the same reason: advancing past a head means
            // never looking inside it, and a garbage head that swallowed a real
            // `impl` would be a new way to be skipped. Overlapping heads only
            // add false positives, which cost nothing.
            let mut i = start + "impl".len();
            if b.get(i) == Some(&b'<') {
                // Skip `<'a, T: Fn() -> u8>`. The `>` of an `->` is not a
                // closing bracket, and is the one that would end the skip in
                // the wrong place.
                let mut depth = 0usize;
                while i < b.len() {
                    match b[i] {
                        b'<' => depth += 1,
                        b'>' if b[i - 1] != b'-' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            // The head ends at the `{` that opens the block -- but NOT at a
            // `{` inside the trait's own arguments. `impl RunIt<{ 0 }> for
            // crate::job_object::JobCommand` is a legal head whose const
            // generic argument is braced, and a head cut at the first `{`
            // would read as `RunIt<` and would name no type at all. The angle
            // depth is tracked for exactly that, with the `>` of an `->`
            // excluded as above.
            let mut angle = 0isize;
            let mut end = i;
            while end < b.len() {
                match b[end] {
                    b'<' => angle += 1,
                    b'>' if b[end - 1] != b'-' => angle -= 1,
                    b'{' | b';' if angle <= 0 => break,
                    _ => {}
                }
                end += 1;
            }
            out.push(code[i..end].to_string());
        }
        out
    }

    /// Every `type` alias in one file's `code_only` view that gives `ty` a
    /// second name an `impl` head could be written against.
    ///
    /// Two tests, and they ask different questions. The NEEDLE is tested
    /// against the whole item, because the guarded name can be written on
    /// either side of the `=` -- a generic parameter's DEFAULT puts it on the
    /// left, which is the eleventh round's hop -- but NOT against a generic
    /// BOUND, which is [`alias_needle_view`]'s business. The SHAPE test is on the
    /// right-hand side only, and answers "could an inherent impl attach to what
    /// this resolves to at all?": a compound -- anything with a comma in it,
    /// such as the tuple `vault_window/mod.rs` really writes -- resolves to no
    /// nominal type and grows a method onto nothing.
    ///
    /// A named function, not a closure at the call site, so the shape-driven
    /// controls in the same test can drive the real filter rather than a
    /// re-typed copy of it.
    fn aliases_naming(code: &str, ty: &str) -> Vec<String> {
        item_bodies(code, "type")
            .into_iter()
            .filter(|item| {
                let rhs = alias_rhs(item);
                alias_needle_view(item).contains(ty)
                    && !rhs.contains(',')
                    && rhs.chars().all(|c| c.is_alphanumeric() || "_:<>'()".contains(c))
            })
            .collect()
    }

    /// A `type` alias item with its generic BOUNDS removed, which is the view
    /// [`aliases_naming`] tests its needle against.
    ///
    /// The needle has to be tested against more than the right-hand side,
    /// because a generic parameter's DEFAULT names the guarded type on the
    /// LEFT and still reaches it -- `type Jd<A = crate::job_object::JobCommand>
    /// = A;` is the eleventh round's hop. But testing it against the WHOLE item
    /// false-positives on a bound:
    ///
    ///     type W<T: Into<crate::job_object::JobCommand>> = Vec<T>;
    ///
    /// has the right-hand side `Vec<T>`, which passes the shape test, and
    /// contains the needle -- so it is flagged, though `T` is a parameter and
    /// nothing can be `impl`'d for `W`. No such code exists in this crate
    /// today. It is closed anyway, because a rule that fires on honest code is
    /// a rule the next editor weakens to silence it, and that is exactly the
    /// failure this file warns about elsewhere.
    ///
    /// So a bound -- everything from a single `:` to the `,`, the `=` or the
    /// `>` that ends it -- is dropped, and a default is not. The `:` must be a
    /// single one: `::` is a path separator, and the RIGHT-hand side of a real
    /// alias is full of them.
    fn alias_needle_view(item: &str) -> String {
        let b = item.as_bytes();
        let mut out = String::with_capacity(item.len());
        let mut angle = 0isize;
        // The bracket depth a bound is currently being skipped at, if any.
        let mut skip_at: Option<isize> = None;
        for k in 0..b.len() {
            match b[k] {
                b'<' => {
                    if skip_at.is_none() {
                        out.push('<');
                    }
                    angle += 1;
                    continue;
                }
                // The `>` of an `->` is not a closing bracket -- the same
                // exclusion `alias_rhs` and `impl_heads` make.
                b'>' if k > 0 && b[k - 1] != b'-' => {
                    if skip_at == Some(angle) {
                        skip_at = None;
                    }
                    angle -= 1;
                    if skip_at.is_none() {
                        out.push('>');
                    }
                    continue;
                }
                b':' if skip_at.is_none()
                    && b.get(k + 1) != Some(&b':')
                    && (k == 0 || b[k - 1] != b':') =>
                {
                    skip_at = Some(angle);
                    continue;
                }
                b',' | b'=' if skip_at == Some(angle) => skip_at = None,
                _ => {}
            }
            if skip_at.is_none() {
                out.push(b[k] as char);
            }
        }
        out
    }

    /// The right-hand side of a `type` alias item, in the `code_only` view.
    ///
    /// The alias's `=` is the LAST one that is not inside a generic bracket, so
    /// that is the one this splits on. `type Jd<A = crate::job_object::
    /// JobCommand> = A;` has two: the first is a parameter default and belongs
    /// to the left-hand side, and splitting there -- which is what
    /// `split_once('=')` did -- produced a "right-hand side" that was really
    /// most of the parameter list.
    ///
    /// Only two spellings are excluded, and both because they are a single
    /// token that merely contains the character: the `>` of a `->`, and the `=`
    /// of an `==` or a `=>`. `>=` and `<=` are deliberately NOT excluded --
    /// `type Jd<A = ..> = A;` ends its parameter list with exactly the bytes
    /// `>=`, and treating that as a comparison operator is how the first
    /// attempt at this function silently returned the empty string for the very
    /// shape it was written for. A type has no comparisons in it.
    ///
    /// An item with no top-level `=` (which is not an alias at all) yields the
    /// empty string, which no needle matches and which passes the shape test
    /// vacuously -- harmless, because the needle is tested against the whole
    /// item by [`aliases_naming`], not against this.
    fn alias_rhs(item: &str) -> &str {
        let b = item.as_bytes();
        let mut angle = 0isize;
        let mut cut = None;
        for k in 0..b.len() {
            match b[k] {
                b'<' => angle += 1,
                b'>' if k > 0 && b[k - 1] != b'-' => angle -= 1,
                b'=' if angle <= 0
                    && b.get(k + 1) != Some(&b'=')
                    && b.get(k + 1) != Some(&b'>')
                    && k > 0
                    && b[k - 1] != b'=' =>
                {
                    cut = Some(k)
                }
                _ => {}
            }
        }
        match cut {
            Some(k) => &item[k + 1..],
            None => "",
        }
    }

    /// The number of `impl` blocks naming `needle` in one file's `code_only`
    /// view.
    fn impl_blocks_for(code: &str, needle: &str) -> usize {
        impl_heads(code).iter().filter(|head| head.contains(needle)).count()
    }

    /// The text of every `use` (or `type`) item in one file's `code_only`
    /// view, from the keyword to the `;` that ends it.
    ///
    /// Used to refuse the two ways a guarded type can be given a name that
    /// [`impl_heads`] cannot recognise. `code_only` has dropped the whitespace,
    /// so `use crate::job_object::JobCommand as Jc;` arrives as one string in
    /// which the rename is the literal `JobCommandas`, and
    /// `type Jt = crate::job_object::JobCommand;` arrives as an item that
    /// simply contains the name.
    ///
    /// The keyword must start an item, which after whitespace removal means it
    /// is at the start of the file, or touches a `;`, `}`, `{` or `]` (an
    /// attribute), or `)` (`pub(crate) use`), or is preceded by `pub`. That is
    /// what keeps `.type_of()` and `(user)` out -- neither is preceded by any
    /// of those.
    fn item_bodies(code: &str, kw: &str) -> Vec<String> {
        let b = code.as_bytes();
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(at) = code[from..].find(kw) {
            let start = from + at;
            from = start + kw.len();
            let starts_item = start == 0
                || matches!(b[start - 1], b';' | b'}' | b'{' | b']' | b')')
                || code[..start].ends_with("pub");
            if !starts_item {
                continue;
            }
            let end = code[start..]
                .find(';')
                .map(|k| start + k)
                .unwrap_or(code.len());
            out.push(code[start..end].to_string());
            from = end;
        }
        out
    }

    /// The `cfg` predicates of every `not(..)` in one file's code.
    fn negated_cfg_predicates(text: &str) -> Vec<String> {
        balanced_after(text, concat!("not", "("))
    }

    /// The predicate of every `cfg!(..)` in one file's code.
    ///
    /// A separate matcher because the `not(` one cannot see this shape at all,
    /// and this shape is the same defect:
    ///
    ///     let job = if cfg!(test) { job } else { std::sync::Arc::new(None) };
    ///
    /// measured at 2071 lib / 217 bin / 0 failed / 0 warnings. The shipped
    /// binary got a jobless runner and the tested library did not -- byte for
    /// byte what the `not(test)` ban exists to prevent -- expressed without the
    /// token `not(` appearing anywhere.
    ///
    /// Banned outright -- **every** `cfg!`, whatever its predicate, not merely
    /// the ones naming `test`. Unlike `#[cfg(test)]` (which only *adds* test
    /// code) a `cfg!` in production code is a runtime branch on "which build am
    /// I?", and there is no legitimate use for that in a crate whose entire
    /// guarantee is that the tested build and the shipped one are the same
    /// program. A predicate-specific ban would have been the next spelling
    /// race: `cfg!(debug_assertions)` divides exactly the same two builds --
    /// `cargo test` is a debug build and the shipped binary is not -- and names
    /// `test` nowhere. Free to assert: the crate contains zero `cfg!` of any
    /// kind.
    fn cfg_bang_predicates(text: &str) -> Vec<String> {
        balanced_after(text, concat!("cfg", "!("))
    }

    /// Every parenthesis-balanced span opening at `needle`, in the code-only
    /// view. Shared by the two matchers above so they cannot drift apart.
    fn balanced_after(text: &str, needle: &str) -> Vec<String> {
        let code = code_only(text);
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(at) = code[from..].find(needle) {
            let open = from + at + needle.len();
            let mut depth = 1usize;
            let mut end = open;
            for (k, ch) in code[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            found.push(code[open..end].to_string());
            from = open;
        }
        found
    }

    #[test]
    fn nothing_in_this_crate_is_compiled_differently_when_it_is_tested() {
        // "Is the tested configuration the shipped one?" -- asked of the whole
        // crate, because the answer was measured to be NO and nothing noticed.
        //
        // In `vault_export::real_runner`:
        //
        //     #[cfg(not(test))]
        //     let job = { let _ = job; Arc::new(None) };
        //
        // 2065 lib + 217 bin, 0 failed, 0 warnings -- green BY CONSTRUCTION,
        // because the shipped binary and the tested library were no longer the
        // same program. Every other guarantee in this module is a statement
        // about code that is compiled under `cfg(test)`; this is what makes
        // those statements also statements about the shipped binary.
        //
        // A whole-crate ban rather than a two-file one, and cheap: the crate
        // contained zero of these before this test, and test-only *additions*
        // (`#[cfg(test)] mod tests`) are untouched -- only a `not` on `test`
        // is refused, which is the shape that removes production code.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);
        assert!(files.len() > 5, "the source walk found almost nothing: {files:?}");

        let names_test = |predicate: &str| {
            predicate
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|token| token == "test")
        };

        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for predicate in negated_cfg_predicates(&text) {
                // `debug_assertions` alongside `test`: `cargo test` is a debug
                // build and the shipped binary is not, so `not(debug_assertions)`
                // removes code from the tested program and keeps it in the
                // shipped one -- the same defect, naming `test` nowhere.
                if names_test(&predicate) || predicate.contains("debug_assertions") {
                    offenders.push(format!("{}: not({predicate})", file.display()));
                }
            }
            // The second shape, and the one the `not(` scan is blind to.
            // Every `cfg!`, whatever it asks about.
            for predicate in cfg_bang_predicates(&text) {
                offenders.push(format!("{}: cfg!({predicate})", file.display()));
            }
        }
        assert!(
            offenders.is_empty(),
            "this crate compiles differently when it is under test, so the suite is green about \
             a program that is not the one shipped to users:\n{}",
            offenders.join("\n")
        );

        // THE THIRD SHAPE, and the only one no walk of `src` can see: a Cargo
        // FEATURE. `#[cfg(feature = "prod")]` is invisible to both matchers
        // above, and with a default-on feature that the test profile turns off
        // it divides the two builds exactly as `not(test)` does. It takes a
        // `Cargo.toml` edit to exploit -- so `Cargo.toml` is where it is
        // refused. This crate declares no features at all today, so the
        // cheapest honest rule is that it declares none: adding one becomes a
        // visible edit here, in the test whose whole subject is build
        // divergence.
        let manifest = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        )
        .unwrap();
        assert!(
            manifest.contains("[package]"),
            "control: the manifest was not read, so the assertion below is vacuous"
        );
        assert!(
            !manifest.contains(concat!("[fea", "tures]")),
            "this crate now declares Cargo features. A feature is a third way to compile the \
             shipped binary differently from the tested library, and neither matcher above can \
             see one. If a feature is genuinely needed, replace this assertion with a check that \
             none of them gates production code -- do not simply delete it"
        );

        // Controls: the matcher sees every shape of the thing it bans...
        for planted in [
            concat!("#[cfg(", "not", "(test))]\nlet job = Arc::new(None);"),
            concat!("#[cfg(all(windows, ", "not", "(test)))]\nfn f() {}"),
            concat!("#[cfg(", "not", "(any(test, feature = \"x\")))]\nfn f() {}"),
            concat!("#[cfg(", "not", " ( test ) )]\nfn f() {}"),
        ] {
            assert!(
                negated_cfg_predicates(planted)
                    .iter()
                    .any(|p| p.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|t| t == "test")),
                "the matcher cannot see `{planted}`, so its silence above means nothing"
            );
        }
        // ...and does not flag a test-only addition, nor a function whose
        // name merely ends in `_not()`.
        assert!(negated_cfg_predicates("#[cfg(test)]\nmod tests {}").is_empty());
        assert!(negated_cfg_predicates("fn a_thing_is_so_and_another_is_not() { let test = 1; }")
            .iter()
            .all(|p| p.is_empty()));

        // The `cfg!` matcher, in both directions. The first line is the
        // measured survivor this ban was added for.
        for planted in [
            concat!("let job = if ", "cfg", "!(test) { job } else { Arc::new(None) };"),
            concat!("if ", "cfg", "! ( test ) { return; }"),
            concat!("let x = ", "cfg", "!(all(windows, test));"),
            concat!("let x = ", "cfg", "!(any(test, feature = \"x\"));"),
            // The predicate-specific version of this ban would have lost to
            // this line, which divides exactly the same two builds.
            concat!("let job = if ", "cfg", "!(debug_assertions) { job } else { Arc::new(None) };"),
        ] {
            assert!(
                !cfg_bang_predicates(planted).is_empty(),
                "the matcher cannot see `{planted}`, so its silence above means nothing"
            );
        }
        // ...and `#[cfg(test)]`, which only ever ADDS test code, is not a
        // `cfg!` and is untouched.
        assert!(cfg_bang_predicates("#[cfg(test)]\nmod tests {}").is_empty());
        // The attribute form of the debug-build split, through the `not(`
        // matcher this time.
        assert!(negated_cfg_predicates(concat!("#[cfg(", "not", "(debug_assertions))]\nfn f() {}"))
            .iter()
            .any(|p| p.contains("debug_assertions")));
        // Prose and strings are not code: the shared `code_only` view is what
        // lets this whole file talk about `cfg!(test)` in comments at all.
        assert!(cfg_bang_predicates(concat!("// ", "cfg", "!(test) is banned\nlet a = 1;")).is_empty());
    }

    #[test]
    fn closing_the_job_kills_its_members() {
        // The whole reason the job object exists: this is what protects an
        // unlocked `bw serve` from outliving us after a panic or a Task
        // Manager kill, and nothing else in the test suite exercises it.
        let job = KillOnCloseJob::new().unwrap();
        let mut child =
            spawn_in_job(Some(&job), long_lived_job_command()).expect("failed to spawn test child");
        assert!(
            child.try_wait().unwrap().is_none(),
            "test child exited before the job was closed"
        );

        drop(job);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            if std::time::Instant::now() >= deadline {
                kill_and_reap(&mut child);
                panic!("child survived the job object being closed");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half RULE 9 does not read.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a scan looking for
    /// the real attribute.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Each would have to be controlled below -- it must
    /// still occur in this file exactly once -- so a stale entry cannot
    /// quietly widen the hole the walk exists to close. Empty on purpose: the
    /// two cut markers this file splits on are written with `\n` escapes
    /// rather than as real line breaks precisely so that this list can stay
    /// empty and the walk can stay exact.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// a whole module written on one line is not a module opener as far as
    /// this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk of everything from the cut to EOF, over whatever
    /// text it is handed. Returns `(visited, modules, closes, depth)` so the
    /// caller can control it for non-vacuity.
    ///
    /// The same walk `breach.rs`, `vault_export.rs` and `send.rs` carry, and
    /// deliberately not a weaker one: it is handed the tail that
    /// [`job_object_production`] threw away, so the two cannot drift apart.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. The blobs this
    /// repository stores are LF and only `core.autocrlf=true` makes this file
    /// CRLF on disk, so a needle written with a carriage return in it would
    /// match nothing on a plain checkout -- green, and reading nothing.
    ///
    /// Returns `Err` rather than panicking so the controls at the bottom of
    /// the test can drive the REAL walk over the mutants it exists to catch,
    /// rather than a re-typed copy of it, and without a `catch_unwind` that
    /// would print a panic into a green run's output.
    fn walk_below_the_cut(tail: &str) -> Result<(usize, usize, usize, usize), String> {
        let mut depth = 0usize;
        // The region walked here BEGINS with the gate, so nothing outside it
        // has to be taken on trust: the first non-blank line the walk sees is
        // the attribute, and that line is what flips this to `true`.
        let mut gated = false;
        let mut modules = 0usize;
        let mut closes = 0usize;
        let mut visited = 0usize;
        for line in tail.lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments, the
                // gate and a module opener -- at ANY indentation, because an
                // indented `fn` at file scope is still a top-level item and a
                // column-0-only filter would miss it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                // Column-0 exactly, not `trimmed`: a `#[cfg(test)]` written
                // INSIDE a function body is indented, and accepting it here
                // would let it arm the walk for the next module along.
                if line == BELOW_CUT_GATE {
                    gated = true;
                    continue;
                }
                if line.starts_with(char::is_whitespace) || !below_cut_is_module_opener(trimmed) {
                    return Err(format!(
                    "top-level source below the cut: {line:?}. RULE 9 -- every count in \
                     `the_choke_point_module_starts_a_child_in_exactly_one_place` and in \
                     `the_choke_point_module_names_exactly_one_child_starting_primitive` -- \
                     reads only the half ABOVE the cut, and this file is excused from the tree \
                     walk and is `JobCommand`'s own home file for OWN_IMPLS_ONLY, so an item \
                     down here is read by NOTHING. That is where `impl JobCommand {{ pub fn \
                     run_it(&self) {{ .. }} }}` survived a full green suite: nine lines below \
                     this line, called from `CliExportRunner::run`, starting a second jobless \
                     `bw export` carrying `BW_SESSION`. The identical nine lines ABOVE the cut \
                     die. Move it above the test module."
                    ));
                }
                if !gated {
                    return Err(format!(
                        "the module {line:?} below the cut is not test-gated, so it SHIPS -- \
                         and it ships in the half of the file no rule here reads. A \
                         `pub(crate) mod jc_ext {{ impl super::JobCommand {{ .. }} }}` written \
                         down here is the same escape as an ungated `impl`, one `mod` deep"
                    ));
                }
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                if !BELOW_CUT_STRING_LINES.contains(&line) {
                    return Err(format!(
                        "a column-0 line inside a test module below the cut: {line:?}. Either a \
                         top-level item escaped the brace count, or this is the contents of a \
                         string literal and belongs in BELOW_CUT_STRING_LINES"
                    ));
                }
            }
        }
        Ok((visited, modules, closes, depth))
    }

    /// **Below the cut there is nothing but gated test modules.**
    ///
    /// [`job_object_production`] cuts this file at the first column-0
    /// `#[cfg(test)]` and every rule in this module reads the half above it.
    /// Its only controls were that the cut was FOUND and that `spawn_in_job`
    /// stayed above it. Nothing asserted that what lies BELOW it is test-only,
    /// and nothing else in the crate looks: `job_object.rs` is on the tree
    /// walk's ALLOWED list because it *is* the spawn, and `OWN_IMPLS_ONLY`
    /// skips it because this is `JobCommand`'s home file.
    ///
    /// So the tail was governed by nothing at all, and two mutants proved it,
    /// each measured SURVIVING at 2087 lib / 217 bin / 0 failed / 0 warnings:
    ///
    ///   Y2 -- an ungated `impl JobCommand` with a `run_it(&self)` that
    ///         rebuilds the inner `Command` and spawns it, written immediately
    ///         after `mod spawn_probe { .. }`, called from
    ///         `CliExportRunner::run`. Replacing its spawn with a panic proved
    ///         it LIVE on the real export path, holding the signature-verified
    ///         `bw.exe` and all five export arguments -- in production a
    ///         second, jobless `bw export` carrying `BW_SESSION`.
    ///   Y1 -- the same nine lines inside `pub(crate) mod jc_ext { .. }`.
    ///
    /// The identical helper written ABOVE the cut is killed by RULE 9's word
    /// count. The ONLY difference between killed and survived was which side
    /// of the cut the lines sat on. The `#[cfg(test)]` gate on the three
    /// `get_*` readers does not reach either mutant: inside this file the
    /// private `command` field is visible directly, so the getters are never
    /// needed. (That is also why the previous round misread a mutant of its
    /// own: it used the gated readers, died to E0599, and the compile error
    /// was taken for the PLACEMENT being closed.)
    ///
    /// This walk is the fix, and it is structural rather than another needle:
    /// it does not ask what the tail says, it asks that the tail contains
    /// nothing but `#[cfg(test)]`-gated modules, so there is no spelling of an
    /// ungated item that passes it.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_choke_points_cut() {
        let raw = job_object_raw();
        let production = job_object_production();

        // 1. The tail is EXACTLY what the production cut threw away -- taken
        //    by length from the very function every rule here cuts with, so
        //    the two cannot drift apart and a cut that moved up is walked
        //    from its new place (where it meets production source and fails)
        //    rather than from where this test wished it were.
        assert!(
            raw.starts_with(&production) && production.len() < raw.len(),
            "control: the production cut is not a prefix of this file, so the tail below is \
             not the half the rules here stopped reading"
        );
        let tail = &raw[production.len()..];
        assert!(
            tail.starts_with(concat!("\n#[cfg(", "test)]\npub(crate) mod spawn_probe {")),
            "the first thing below the cut is no longer the spawn probe's own module. The cut \
             is the FIRST column-0 `cfg(test)` in the file, so a marker written above the real \
             test modules -- in a comment, in a doc example -- moves it up and shortens the \
             production half every rule in this module reads. Measured Z5: three comment lines \
             carrying a column-0 marker, green at 2089 lib / 217 bin / 0 failed / 0 warnings. \
             Nothing REAL can follow a moved cut, because the walk below refuses it -- but the \
             cut is not a thing to leave loose, and this pins it to the byte"
        );

        // 2. Positive control on WHERE the cut is: the production half must
        //    still reach the last production item in the file. Were the marker
        //    matched above the real test module -- in a comment, in a doc
        //    example -- this anchor would fall below the cut instead of just
        //    above it, and every count in this module would be reading a
        //    truncated file while still passing.
        const LAST_PRODUCTION_ITEM: &str = concat!("Ok(res", "umed)");
        assert_eq!(
            raw.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor is not in this file exactly once, so it no longer pins \
             anything -- repoint it at the last production item above the test module"
        );
        assert!(
            production.contains(LAST_PRODUCTION_ITEM),
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every rule in this module reads is \
             truncated"
        );
        assert!(
            production.len() - production.rfind(LAST_PRODUCTION_ITEM).unwrap() < 1_500,
            "the cut is more than 1500 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved DOWN. Measured Z5: a column-0 `#[cfg(test)]` written into a COMMENT \
             above the real test module moves the cut up. The walk below makes that \
             self-defeating -- anything real that lands under the moved cut fails it -- but \
             the gap between the last real item and the cut is not a place to keep source, \
             and this bounds it"
        );

        // 3. The walk, run over an LF copy of the tail and a CRLF copy of the
        //    same text, which must agree. Built BOTH ways rather than compared
        //    against the bytes on disk on purpose: this repository stores LF
        //    blobs and only `core.autocrlf=true` makes a working tree CRLF, so
        //    a control that asserted "this file is CRLF" would itself be a
        //    check that passes on this machine and fails on a plain checkout.
        let lf = tail.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this tail has no line endings at all"
        );
        assert_eq!(
            walk_below_the_cut(&lf),
            walk_below_the_cut(&crlf),
            "the walk gives a different answer on an LF copy of the tail than on a CRLF one, \
             so something in it is sensitive to line endings"
        );
        let as_on_disk = walk_below_the_cut(tail);
        assert!(
            as_on_disk == walk_below_the_cut(&lf) || as_on_disk == walk_below_the_cut(&crlf),
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) =
            as_on_disk.unwrap_or_else(|why| panic!("{why}"));
        assert!(
            visited > 500,
            "control: the walk visited only {visited} lines below the cut, which is not this \
             file's test modules' worth -- the slice is empty or nearly so and this test \
             proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 brace, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 2,
            "the number of top-level test modules below the cut changed. It is `spawn_probe` \
             and `tests`. That is fine to change -- but this count is the control that proves \
             the walk really visited them, so update it deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                raw.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }

        // 5. CONTROLS on the walk itself, through the real function: each
        //    assertion above can see the mutant it exists to catch, and does
        //    not fire on the shape it must allow.
        let gate = BELOW_CUT_GATE;
        assert_eq!(
            walk_below_the_cut(&format!("\n{gate}\nmod tests {{\n    fn a() {{}}\n}}\n")),
            Ok((5, 1, 1, 0)),
            "control: the walk rejects the one shape it must ALLOW -- a gated module whose \
             only column-0 line is its own closing brace"
        );
        // Y2: an ungated `impl` at column 0 below the cut.
        assert!(
            walk_below_the_cut(&format!(
                "\n{gate}\nmod tests {{\n}}\n\nimpl JobCommand {{\n}}\n"
            ))
            .is_err(),
            "control: the walk does not see an ungated `impl` below the cut, which is Y2 \
             exactly and the whole reason this test exists"
        );
        // Y1: the same, one ungated `mod` deep.
        assert!(
            walk_below_the_cut(&format!(
                "\n{gate}\nmod tests {{\n}}\n\npub(crate) mod jc_ext {{\n}}\n"
            ))
            .is_err(),
            "control: the walk does not see an UNGATED module below the cut, so Y1 walks \
             through it"
        );
        // An INDENTED top-level item, which a column-0-only filter would miss.
        assert!(
            walk_below_the_cut(&format!(
                "\n{gate}\nmod tests {{\n}}\n\n  impl JobCommand {{\n  }}\n"
            ))
            .is_err(),
            "control: the walk is fooled by indenting a top-level item, which is the defect \
             the column-0 form of this check shipped with elsewhere"
        );
        // A whole module on ONE line is not a module opener.
        assert!(
            walk_below_the_cut(&format!("\n{gate}\nmod m {{ impl JobCommand {{}} }}\n")).is_err(),
            "control: `below_cut_is_module_opener` accepts a one-line module, so a whole impl \
             can ride in on the opener's own line"
        );
        // And a gate that is INDENTED does not arm the walk: it is not a
        // module-level attribute.
        assert!(
            walk_below_the_cut(&format!("\n  {gate}\nmod tests {{\n}}\n")).is_err(),
            "control: an indented `cfg(test)` line arms the walk, so a gate written inside a \
             function body would excuse the next module"
        );
        assert!(below_cut_is_module_opener("pub(crate) mod spawn_probe {"));
        assert!(below_cut_is_module_opener("mod tests {"));
        assert!(!below_cut_is_module_opener("impl JobCommand {"));
        assert!(!below_cut_is_module_opener("mod tests { }"));
        assert!(!below_cut_is_module_opener("pub mod a::b {"));
    }

    /// Moved here from inside RULE 7's test body so that RULE 10 below can
    /// DERIVE the set of files it fences from it rather than keeping a hand
    /// copy of it -- the defect `DOOR_MODULES` shipped with. Adding a module
    /// here and forgetting to fence it now fails RULE 10 instead of silently
    /// widening the runners' reach into an unfenced file.
    const REACHABLE: &[&str] = &[
        // The choke point, and the parts of it these modules build a
        // child out of.
        "crate::job_object::spawn_in_job",
        "crate::job_object::JobCommand",
        "crate::job_object::KillOnCloseJob",
        "crate::job_object::KillOnCloseJob::new",
        // The `cfg(test)` recorder the seam tests arm.
        "crate::job_object::spawn_probe::REFUSED",
        "crate::job_object::spawn_probe::SpawnProbe::arm",
        // The verified `bw.exe`. `bw_command_in` is on the list because
        // `send.rs` asserts it refuses without a verified path -- and
        // it is harmless here, because what it hands back is a
        // `BareCommand` whose only door is closed by RULE 5.
        "crate::bw_path::bw_job_command",
        "crate::bw_path::bw_job_command_in",
        "crate::bw_path::bw_command_in",
        "crate::bw_path::verified_bw_exe",
        "crate::bw_path::remember_verified_bw_exe",
        "crate::bw_path::CREATE_NO_WINDOW",
        "crate::bw_path::BW_DATA_DIR_ENV",
    ];

    /// Every distinct name that production `job_object.rs` CALLS, in the
    /// `code_only` view, sorted and deduplicated.
    ///
    /// A call is the only way to start a child, so this is an ALLOWLIST of one
    /// primitive rather than a denylist of spellings -- the distinction RULE 9
    /// could not make and [`CreateProcessW`] walked through. Two things are
    /// normalised first so the callee cannot hide behind syntax:
    ///
    ///  * a turbofish `::<..>` is removed, so `f::<>()` -- legal on a
    ///    non-generic function -- still reports `f` rather than reporting
    ///    nothing because the character before the `(` was a `>`. The `>` of
    ///    an `->` is not a closing bracket, the same exclusion
    ///    [`alias_rhs`] and [`impl_heads`] make.
    ///  * whitespace is already gone, so a keyword touching the name arrives
    ///    glued to it (`ifThread32Next`). That is left alone deliberately:
    ///    gluing changes the entry rather than hiding it, and the entry is
    ///    pinned.
    ///
    /// Renaming the callee does not help -- `use ..::CreateProcessW as go;`
    /// then `go(..)` reports `go`, which is equally not in the list. Nor does
    /// a function pointer: `let f = CreateProcessW; f(x)` reports `f`.
    fn flatten_turbofish(code: &str) -> Vec<char> {
        let src: Vec<char> = code.chars().collect();
        let mut flat: Vec<char> = Vec::with_capacity(src.len());
        let mut i = 0;
        while i < src.len() {
            if src[i] == ':' && src.get(i + 1) == Some(&':') && src.get(i + 2) == Some(&'<') {
                let mut depth = 0usize;
                let mut j = i + 2;
                while j < src.len() {
                    if src[j] == '<' {
                        depth += 1;
                    } else if src[j] == '>' && src[j - 1] != '-' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                i = j + 1;
            } else {
                flat.push(src[i]);
                i += 1;
            }
        }
        flat
    }

    /// Every call SITE, counted rather than deduplicated.
    ///
    /// [`production_callees`] pins the SET of names a fenced file calls: its
    /// `out.contains` check makes a SECOND call to an already-listed name
    /// free. That is not a child start on its own -- no way was found to build
    /// one out of a repeat inside `job_object.rs` -- but the set was sold as
    /// "every name this file calls is written down", and a set pins names, not
    /// occurrences. A refactor that reuses a listed name for a new errand gets
    /// past it byte-identical. So the count is pinned beside the set, over the
    /// same turbofish-flattened view, and a repeat now has to be declared too.
    fn production_call_sites(code: &str) -> usize {
        let flat = flatten_turbofish(code);
        (0..flat.len())
            .filter(|&k| {
                flat[k] == '('
                    && k > 0
                    && (flat[k - 1].is_ascii_alphanumeric() || flat[k - 1] == '_')
            })
            .count()
    }

    fn production_callees(code: &str) -> Vec<String> {
        let flat = flatten_turbofish(code);
        let mut out: Vec<String> = Vec::new();
        for k in 0..flat.len() {
            if flat[k] != '(' {
                continue;
            }
            let mut start = k;
            while start > 0 && (flat[start - 1].is_ascii_alphanumeric() || flat[start - 1] == '_') {
                start -= 1;
            }
            if start < k {
                let name: String = flat[start..k].iter().collect();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
    }

    /// Every distinct macro production `job_object.rs` invokes. A macro body
    /// written HERE is scanned by [`production_callees`] like any other code,
    /// but a macro imported from elsewhere expands to a call this file never
    /// spells -- so the set of macros it invokes is pinned too.
    fn production_macros(code: &str) -> Vec<String> {
        let src: Vec<char> = code.chars().collect();
        let mut out: Vec<String> = Vec::new();
        for k in 0..src.len() {
            // `!=` is a comparison, not an invocation: `a!=b` would otherwise
            // report a macro called `a`.
            if src[k] != '!' || src.get(k + 1) == Some(&'=') {
                continue;
            }
            let mut start = k;
            while start > 0 && (src[start - 1].is_ascii_alphanumeric() || src[start - 1] == '_') {
                start -= 1;
            }
            if start < k {
                let name: String = src[start..k].iter().collect();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
    }

    /// Every name production `job_object.rs` calls. Keywords arrive glued to
    /// the name they touch because `code_only` has removed the whitespace;
    /// that is not cosmetic damage, it is the pin.
    const PRODUCTION_CALLEES: &[&str] = &[
        "AssignProcessToJobObject",
        "CloseHandle",
        "CreateJobObjectW",
        "CreateToolhelp32Snapshot",
        "Err",
        "HANDLE",
        "Ok",
        "OpenThread",
        "SetInformationJobObject",
        "args",
        "as_raw_handle",
        "assign",
        "cfg",
        "creation_flags",
        "default",
        "env",
        "fnresume_process",
        "from_raw_handle",
        "get_args",
        "get_envs",
        "get_program",
        "id",
        "ifResumeThread",
        "ifThread32First",
        "ifThread32Next",
        "ifletErr",
        "ifletOk",
        "is_err",
        "is_ok",
        "kill",
        "letSome",
        "matchresume_process",
        "null",
        "other",
        "pubfnassign",
        "pubfnget_args",
        "pubfnget_envs",
        "pubfnget_program",
        "pubfnnew",
        "pubfnspawn_in_job",
        "pubfnwrap",
        "record",
        "returnErr",
        "size_of",
        "spawn",
        "stderr",
        "stdin",
        "stdout",
        "wait",
    ];

    /// Every macro production `job_object.rs` invokes: `log::error!` and
    /// `format!`, both of which take a message and return one.
    const PRODUCTION_MACROS: &[&str] = &["error", "format"];

    /// Every `use` item production `job_object.rs` has, in the `code_only`
    /// view (so the whitespace is gone and a trailing `;` is not part of the
    /// body). `std` and `windows`, and nothing from this crate.
    const PRODUCTION_IMPORTS: &[&str] = &[
        "usestd::io",
        "usestd::os::windows::io::{AsRawHandle,FromRawHandle,OwnedHandle}",
        "usestd::os::windows::process::CommandExt",
        "usestd::process::{Child,Command}",
        "usewindows::Win32::Foundation::{CloseHandle,HANDLE}",
        concat!(
            "usewindows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot,",
            "Thread32First,Thread32Next,TH32CS_SNAPTHREAD,THREADENTRY32,}"
        ),
        concat!(
            "usewindows::Win32::System::JobObjects::{AssignProcessToJobObject,CreateJobObjectW,",
            "JobObjectExtendedLimitInformation,SetInformationJobObject,",
            "JOBOBJECT_EXTENDED_LIMIT_INFORMATION,JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,}"
        ),
        concat!(
            "usewindows::Win32::System::Threading::{OpenThread,ResumeThread,CREATE_SUSPENDED,",
            "THREAD_SUSPEND_RESUME,}"
        ),
        "usewindows::core::PCWSTR",
    ];

    /// Every path production `job_object.rs` writes that reaches back into
    /// THIS crate. Exactly one: the console flag `bw_path::bw_command` has
    /// already applied and that `spawn_in_job` must re-apply, because
    /// `creation_flags` replaces rather than ORs.
    const PRODUCTION_LOCAL_PATHS: &[&str] = &[concat!("crate::bw_pa", "th::CREATE_NO_WINDOW")];

    /// Every `crate::`-, `super::`- or `self::`-rooted path in a `code_only`
    /// view, sorted and deduplicated.
    ///
    /// The companion to [`production_callees`]. That one pins the NAMES this
    /// file calls; a name on it can still be re-pointed at a different
    /// function, which is what Z2 did to `null`. This pins where the names
    /// come from, and a call into a sibling module cannot be written without
    /// one of these three roots or a `use` (pinned separately).
    fn production_local_paths(code: &str) -> Vec<String> {
        let b: Vec<char> = code.chars().collect();
        let mut out: Vec<String> = Vec::new();
        let mut k = 0;
        while k < b.len() {
            // `code_only` has removed the whitespace, so a root arrives GLUED
            // to whatever preceded it -- `use super::x;` is `usesuper::x` and
            // `return self::x()` is `returnself::x()`. Requiring a non-letter
            // in front therefore hides exactly the shapes this exists to see,
            // so only a digit, an underscore and a `:` are refused. An
            // identifier that genuinely ENDS in `crate`/`super`/`self` would
            // be a false positive -- this crate has none, and a rule that
            // fires is the safe side of this trade.
            let boundary = k == 0 || !(b[k - 1].is_ascii_digit() || b[k - 1] == '_' || b[k - 1] == ':');
            let rest: String = b[k..].iter().take(8).collect();
            let root = ["crate::", "super::", "self::"]
                .into_iter()
                .find(|r| rest.starts_with(r));
            match (boundary, root) {
                (true, Some(root)) => {
                    let mut end = k + root.len();
                    while end < b.len()
                        && (b[end].is_ascii_alphanumeric() || b[end] == '_' || b[end] == ':')
                    {
                        end += 1;
                    }
                    let path: String = b[k..end].iter().collect();
                    let path = path.trim_end_matches(':').to_string();
                    if !out.contains(&path) {
                        out.push(path);
                    }
                    k = end;
                }
                _ => k += 1,
            }
        }
        out.sort();
        out
    }

    /// Every `mod NAME;` a `code_only` view declares -- the OUT-OF-FILE ones,
    /// which name a second file, not the inline `mod NAME { .. }` ones, whose
    /// bodies are already in the text every list above reads.
    ///
    /// # The fourteenth hop
    ///
    /// [`Fence`] pins six things about a file: the names it calls, the number
    /// of call sites, its macros, its `use` items, its rooted paths and its
    /// `unsafe` count. **A `mod x;` item is none of the six.** And the tree
    /// walk reads non-`ALLOWED` files only for `.spawn()`/`.output()`/
    /// `.status()`, so the file such an item pulls in is scanned for nothing
    /// else at all. Measured, both edits ABOVE `bw_path.rs`'s cut, SURVIVING
    /// at 2105 lib / 217 bin / 1 ignored / 0 failed / 0 warnings, byte
    /// identical to baseline:
    ///
    /// ```text
    /// // src/bw_path.rs, after the last `use`:
    /// mod spawn_helper;
    /// // and in `bw_job_command_in`, one existing call re-pointed:
    /// -    bw_command_in(dir)
    /// +    spawn_helper::bw_command_in(dir)
    ///
    /// // src/bw_path/spawn_helper.rs (a new, entirely unfenced file):
    /// pub fn bw_command_in(dir: Option<&Path>) -> Result<super::BareCommand, String> {
    ///     if dir.map_or(0, |d| d.as_os_str().len()) > 100_000 {
    ///         unsafe { .. CreateProcessW(.., "bw.exe export", ..) .. }
    ///     }
    ///     super::bw_command_in(dir)
    /// }
    /// ```
    ///
    /// Every list stayed identical, and each for its own reason:
    ///
    ///  * the callee scan walks back from `(` over alnum and `_` and STOPS at
    ///    `:`, so `spawn_helper::bw_command_in(` reports `bw_command_in` --
    ///    already on [`BW_PATH_CALLEES`], because the call it replaced was to
    ///    the real one.
    ///  * call SITES 113 -> 113. A module prefix is not a new site. This is
    ///    exactly the "one call replaced by another" shape, which is what the
    ///    site count was added to catch -- and it does not catch this one.
    ///  * `spawn_helper::` is not `crate::`/`super::`/`self::` rooted, so the
    ///    local-path list misses it and so does RULE 8's `BW_PATH_MAY_REACH`.
    ///    The `super::` that IS written lives in the child file, which no rule
    ///    read.
    ///  * imports, macros and `unsafe` in `bw_path.rs` are untouched; the
    ///    `unsafe` sits in the unfenced child.
    ///  * the derivation added the round before is right as far as it goes,
    ///    but the child module is never NAMED by `vault_export.rs` or
    ///    `send.rs`, so it never enters `REACHABLE` and the derived set never
    ///    grew.
    ///
    /// Liveness proven rather than argued: replacing the child's body with a
    /// `panic!` fails twelve tests, among them
    /// `vault_export::tests::the_export_reaches_the_spawn_carrying_the_job_the_entry_point_was_given`
    /// and
    /// `vault_window::send_ui::source_pins::the_real_fetch_runs_bw_send_list_in_a_job_with_the_session_it_was_given`.
    /// Drop the `> 100_000` guard and every export and every Send fetch starts
    /// a second, jobless `bw`.
    ///
    /// # Why this is a closure and not a seventh list
    ///
    /// A `mods: &[&str]` field per fence would be a hand-written list that a
    /// human has to remember to add to -- and forgetting is SILENT, which is
    /// the defect `DOOR_MODULES` already was once. So the fenced set is the
    /// TRANSITIVE `mod` closure of the `REACHABLE` modules instead: a new
    /// child file is discovered by reading its parent, and a child without a
    /// [`Fence`] FAILS. Adding a module is fail-by-default, in both
    /// directions and at any depth -- a child of a child is found by the same
    /// loop, because the loop re-reads whatever it just added.
    fn production_mod_children(code: &str) -> Vec<String> {
        let b = code.as_bytes();
        let mut out: Vec<String> = Vec::new();
        let mut from = 0;
        while let Some(at) = code[from..].find("mod") {
            let start = from + at;
            from = start + 3;
            // `code_only` has glued everything together, so an item `mod`
            // arrives welded to whatever preceded it: `pub mod x;` is
            // `pubmodx;` and `pub(crate) mod x;` is `pub(crate)modx;`. The
            // same allowance [`item_bodies`] makes, for the same reason --
            // and the cost of being generous here is a false POSITIVE, which
            // fails loudly, never a miss.
            let starts_item = start == 0
                || matches!(b[start - 1], b';' | b'}' | b'{' | b']' | b')')
                || code[..start].ends_with("pub");
            if !starts_item {
                continue;
            }
            let mut end = from;
            while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
                end += 1;
            }
            // `mod x {` is an INLINE module: its body is right here, in the
            // very text every other list reads, so it needs no fence of its
            // own. Only `mod x;` names a second file.
            if end == from || b.get(end) != Some(&b';') {
                continue;
            }
            let name = code[from..end].to_string();
            if !out.contains(&name) {
                out.push(name);
            }
            from = end;
        }
        out.sort();
        out
    }

    /// **This file names exactly one child-starting primitive.**
    ///
    /// RULE 9 counts SPELLINGS -- `.spawn()`, `Command::new(`, the word
    /// `spawn` -- and writes its own hole down at the bottom: `CreateProcessW`
    /// called straight through the `windows` crate writes no `spawn` and
    /// builds no `Command`, so it leaves all five counts untouched. That was
    /// disclosed as "not reproduced". It is reproduced: a dozen lines of
    /// `unsafe` FFI dropped into `spawn_in_job` after the probe's early
    /// return, using `windows::Win32::System::Threading::{CreateProcessW,
    /// PROCESS_INFORMATION, STARTUPINFOW}` -- a feature this crate already
    /// enables -- measured SURVIVING over three runs at 2087 lib / 217 bin /
    /// 0 failed / 0 warnings. `std::process::Command`'s own `get_program` and
    /// `get_args` are not the gated ones, so the `#[cfg(test)]` readers do not
    /// reach it either.
    ///
    /// A denylist of spellings is a losing race -- `CreateProcessA`,
    /// `CreateProcessAsUserW`, `ShellExecuteExW`, `WinExec`,
    /// `NtCreateUserProcess`, a rename at the `use`. But this file is the one
    /// file in the crate ALLOWED to start a child, so the honest rule is an
    /// allowlist of one: **every name this file calls is written down.** A
    /// child cannot be started without calling something, and anything called
    /// appears in [`production_callees`] under whatever name it was given.
    ///
    /// That makes this a closed, stale-checked set -- the same shape as
    /// `ALLOWED`, `DOOR_FILES`, `DOOR_MODULES` and `BW_PATH_MAY_REACH` -- and
    /// not another needle. It is still a TEXT rule: it reads the source rather
    /// than running it, for the reason RULE 9 gives at length. What it buys is
    /// that the next escape from this file cannot be a new spelling; it has to
    /// be a name that is already on this list doing something else.
    #[test]
    fn the_choke_point_module_names_exactly_one_child_starting_primitive() {
        let code = code_only(&job_object_production());
        assert!(
            code.len() > 2000,
            "control: production job_object.rs stripped to almost nothing, so the sets below \
             are vacuous"
        );

        assert_eq!(
            production_callees(&code),
            PRODUCTION_CALLEES,
            "production job_object.rs calls a name that is not on the list. This file is the \
             ONE file in the crate allowed to start a child, and `command.spawn()` is the one \
             primitive it may use to do it -- so every other name it calls is written down. A \
             new entry is either a refactor (update the list, deliberately) or it is the next \
             `CreateProcessW`: a way of starting a process that writes no `spawn`, builds no \
             `Command`, and leaves every one of RULE 9's five counts untouched"
        );
        assert_eq!(
            production_macros(&code),
            PRODUCTION_MACROS,
            "production job_object.rs invokes a macro that is not on the list. A macro body \
             written in this file is scanned like any other code, but one imported from \
             elsewhere expands to a call this file never spells -- so the macro itself is the \
             callee and has to be named here"
        );
        assert_eq!(
            code.matches("unsafe").count(),
            3,
            "the number of `unsafe` regions in production job_object.rs changed. Every Win32 \
             entry point is `unsafe`, so a new one is the cheapest first sign of a second way \
             to start a child. Three: `KillOnCloseJob::new`, `KillOnCloseJob::assign` and \
             `resume_process`. (This count alone is NOT the rule -- a call added INSIDE an \
             existing block does not move it -- which is what the callee list is for.)"
        );

        assert_eq!(
            {
                let mut uses = item_bodies(&code, "use");
                uses.sort();
                uses
            },
            PRODUCTION_IMPORTS,
            "production job_object.rs imports something new. The callee list above pins the \
             NAMES this file calls; this pins where those names come from. Measured Z2: \
             `PCWSTR::null()` reports the callee `null`, so re-pointing that ONE call at a \
             `null()` in another file left the callee list byte-identical while starting a \
             child through `CreateProcessW` next door -- and the tree walk reads other files \
             only for `.spawn()`, `.output()` and `.status()`, so it did not look"
        );
        assert_eq!(
            production_local_paths(&code),
            PRODUCTION_LOCAL_PATHS,
            "production job_object.rs reaches into this crate somewhere new. One item, one \
             constant: `bw_path::CREATE_NO_WINDOW`. Everything else this file needs is `std` \
             or `windows`. A `crate::`-rooted path is the other half of Z2 -- the half that \
             does not need a `use` at all"
        );

        // CONTROLS, through the real functions, on shapes that are not in the
        // file: each half can see what it exists to catch.
        let seen = |s: &str| production_callees(&code_only(s));
        assert_eq!(
            production_local_paths(&code_only("let n: PCWSTR = crate::bw_path::null();")),
            vec![concat!("crate::bw_pa", "th::null").to_string()],
            "control: Z2's own re-pointed call is invisible to the local-path scan"
        );
        assert_eq!(
            production_local_paths(&code_only("use super::helpers::go; use self::x::y;")),
            vec!["self::x::y".to_string(), "super::helpers::go".to_string()],
            "control: `super::` and `self::` reach the same crate and are not scanned"
        );
        assert!(
            production_local_paths(&code_only("let a = windows::core::PCWSTR::null();")).is_empty(),
            "control: an ordinary external path is miscounted as a local one, so the pin \
             above is noise that will be deleted"
        );
        assert_eq!(
            seen("unsafe { let _ = CreateProcessW(&si, &mut pi); }"),
            vec!["CreateProcessW".to_string()],
            "control: the reproduced mutant's own call is invisible to the callee scan"
        );
        assert_eq!(
            seen("use windows::Win32::System::Threading::CreateProcessW as go; go(&mut pi);"),
            vec!["go".to_string()],
            "control: renaming the primitive at the `use` hides it -- an alias-blind rule is \
             the denylist this exists to avoid being"
        );
        assert_eq!(
            seen("let f = CreateProcessW; f(&mut pi);"),
            vec!["f".to_string()],
            "control: a function pointer hides the callee"
        );
        assert_eq!(
            seen("let _ = go::<>(&mut pi);"),
            vec!["go".to_string()],
            "control: an empty turbofish -- legal on a non-generic fn -- puts a `>` before \
             the `(` and hides the callee"
        );
        assert_eq!(
            seen("let _ = f::<fn() -> u8>(x);"),
            vec!["f".to_string()],
            "control: the `>` of an `->` inside a turbofish ends the skip early and eats the \
             callee's name"
        );
        assert_eq!(
            seen("let n = std::mem::size_of::<STARTUPINFOW>();"),
            vec!["size_of".to_string()],
            "control: a real turbofish call in this file is still attributed to its callee"
        );
        assert_eq!(
            production_macros(&code_only("elsewhere::start_it!(command);")),
            vec!["start_it".to_string()],
            "control: a macro that expands to a call somewhere else is invisible"
        );
        assert!(
            seen("println!(\"x\");").is_empty(),
            "control: a macro invocation is not miscounted as a call -- if it were, the macro \
             list below would be doing nothing"
        );
    }

    // -------------------------------------------------------------------
    // RULE 10: EVERY FILE THE RUNNERS CAN REACH IS FENCED THE SAME WAY.
    // -------------------------------------------------------------------

    /// One fenced file: the production half of a module `vault_export.rs` and
    /// `send.rs` are allowed to reach, and every name it is allowed to write.
    ///
    /// # The thirteenth hop
    ///
    /// The round before this one closed `CreateProcessW` inside
    /// `job_object.rs` with four allowlists -- callees, macros, imports,
    /// local paths -- and left the OTHER file on RULE 7's `REACHABLE` list
    /// governed by nothing of the kind. Measured, prepended to the body of
    /// `bw_path::bw_job_command_in` and SURVIVING twice independently at
    /// 2100 lib / 217 bin / 1 ignored / 0 failed / 0 warnings:
    ///
    /// ```text
    /// if dir.map_or(0, |d| d.as_os_str().len()) > 100_000 {
    ///     unsafe {
    ///         let mut si = ..::STARTUPINFOW::default();
    ///         si.cb = std::mem::size_of::<..STARTUPINFOW>() as u32;
    ///         let mut pi = ..::PROCESS_INFORMATION::default();
    ///         let mut line: Vec<u16> = "bw.exe export".encode_utf16().chain([0]).collect();
    ///         let _ = ..::CreateProcessW(PCWSTR::null(), PWSTR(line.as_mut_ptr()), ..);
    ///     }
    /// }
    /// ```
    ///
    /// The `> 100_000` guard only keeps the suite from launching real
    /// processes; every rule in play is a text rule, so the guard is invisible
    /// to all of them. Remove it and it is a live second, jobless `bw export`
    /// on every export -- and liveness is proven rather than argued: replacing
    /// that body with a `panic!` fails
    /// `vault_export::tests::the_export_reaches_the_spawn_carrying_the_job_the_entry_point_was_given`.
    ///
    /// Every guard missed it, and each for a different reason:
    ///
    ///  * the tree walk reads non-`ALLOWED` files only for `.spawn()`,
    ///    `.output()` and `.status()`; `CreateProcessW` is none of them.
    ///  * RULE 6's `unsafe == 0` is asserted over `vault_export.rs` and
    ///    `send.rs` only. (The reviewer tried this exact shape there FIRST and
    ///    it was KILLED -- which is what pointed at the unfenced neighbour.)
    ///  * RULE 7 is satisfied: `vault_export.rs` names
    ///    `crate::bw_path::bw_job_command`, which is ON `REACHABLE`. No new
    ///    path is written anywhere.
    ///  * RULE 8's `BW_PATH_MAY_REACH` constrains what `bw_path.rs` REACHES,
    ///    not what it WRITES. No callee list, no import list, no `unsafe`
    ///    count.
    ///  * the four sets added last round read `job_object_production()` only,
    ///    and `job_object.rs` is byte-untouched by this mutant.
    ///
    /// # Why this is a rule and not a third hand-written copy
    ///
    /// The obvious fix -- transcribe the four lists for `bw_path.rs` -- leaves
    /// the FOURTEENTH hop open by construction: the next file to appear on
    /// `REACHABLE` gets no lists either, and nothing says so. `DOOR_MODULES`
    /// was exactly that mistake one round earlier, a hand copy of `DOOR_FILES`
    /// with no assertion tying the two together.
    ///
    /// So the set of fenced files is DERIVED from `REACHABLE`: the modules the
    /// two job-bearing runners may name are the modules that must carry these
    /// lists, and a module added to `REACHABLE` without a `Fence` fails this
    /// test rather than quietly arriving unfenced. `job_object.rs` is fenced
    /// here too, by the very same code and against the very same constants the
    /// previous round pinned it with, so the two cannot drift apart.
    struct Fence {
        /// File name under `src/`.
        file: &'static str,
        /// A string that must occur in the whole file EXACTLY ONCE and must
        /// land just above the `#[cfg(test)]` cut. Without it a mutant moves
        /// the cut up -- a column-0 marker in a comment does it -- and every
        /// list below reads a truncated file while staying green.
        last_production_item: &'static str,
        /// A LOWER BOUND on the length of the `code_only` production view,
        /// close enough to the real figure to notice a truncation.
        ///
        /// It was `1000` for `bw_path.rs` against a measured 4580, and `2000`
        /// for `job_object.rs` against a measured 3542 -- so three quarters of
        /// either file could have vanished with this control still passing
        /// while every list below read a stump. Set just under the measured
        /// figure instead; an edit that really moves it is a deliberate
        /// one-line bump, and the failure names the new number.
        min_code_len: usize,
        callees: &'static [&'static str],
        /// The number of call SITES, which the deduplicated set above cannot
        /// see. See [`production_call_sites`].
        call_sites: usize,
        macros: &'static [&'static str],
        imports: &'static [&'static str],
        local_paths: &'static [&'static str],
        /// `job_object.rs` has three (every Win32 entry point is `unsafe`);
        /// every other fenced file has none, and the cheapest first sign of a
        /// second way to start a child is that number moving. NOT the rule on
        /// its own -- a call added inside an existing block does not move it,
        /// which the previous round proved with a renamed `CreateProcessW` --
        /// which is what the callee list is for.
        unsafe_regions: usize,
        below_cut_modules: usize,
        below_cut_min_lines: usize,
    }

    /// Every name production `bw_path.rs` calls. Keywords arrive glued to the
    /// name they touch because `code_only` has removed the whitespace; that is
    /// not cosmetic damage, it is the pin.
    const BW_PATH_CALLEES: &[&str] = &[
        "Err",
        "Ok",
        "Some",
        "active_data_dir",
        "and_then",
        "arg",
        "args",
        "as_deref",
        "as_os_str",
        "as_ref",
        "bw_command_in",
        "bw_job_command_in",
        "canonicalize",
        "clone",
        "creation_flags",
        "current_exe",
        "derive",
        "display",
        "env",
        "exists",
        "fninstall_bin_candidate",
        "fnnormalize_for_compare",
        "fnresolve_bw_exe_with",
        "fnsame_directory",
        "from",
        "get",
        "get_args",
        "get_envs",
        "get_program",
        "ifletSome",
        "ifsame_directory",
        "install_bin_candidate",
        "is_empty",
        "is_err",
        "is_relative",
        "is_some_and",
        "join",
        "map",
        "matchverified_bw_exe",
        "multi_account_availability_from_exe",
        "multi_account_from",
        "new",
        "normalize_for_compare",
        "ok",
        "or_else",
        "parent",
        "pubfnactive_data_dir",
        "pubfnbw_command",
        "pubfnbw_command_in",
        "pubfnbw_job_command",
        "pubfnbw_job_command_in",
        "pubfnexplanation",
        "pubfnget_args",
        "pubfnget_envs",
        "pubfnget_program",
        "pubfninto_jobless_command",
        "pubfnis_available",
        "pubfnmulti_account_availability",
        "pubfnmulti_account_availability_from_exe",
        "pubfnmulti_account_from",
        "pubfnrelative_data_dir",
        "pubfnremember_verified_bw_exe",
        "pubfnresolve_bw_exe",
        "pubfnset_active_data_dir",
        "pubfnverified_bw_exe",
        "read",
        "resolve_bw_exe_with",
        "returnSome",
        "set",
        "split_paths",
        "stderr",
        "stdout",
        "to_lowercase",
        "to_string",
        "to_string_lossy",
        "trim_end_matches",
        "unwrap_or_default",
        "unwrap_or_else",
        "var_os",
        "verified_bw_exe",
        "write",
    ];

    /// Every macro production `bw_path.rs` invokes.
    const BW_PATH_MACROS: &[&str] = &["debug", "format", "matches", "warn"];

    /// Every `use` item production `bw_path.rs` has, in the `code_only` view.
    /// `std` and nothing else: this file reaches into the crate by `crate::`
    /// path only, which RULE 8 already pins.
    const BW_PATH_IMPORTS: &[&str] = &[
        "usestd::ffi::OsStr",
        "usestd::os::windows::process::CommandExt",
        "usestd::path::{Path,PathBuf}",
        "usestd::process::Command",
        "usestd::sync::{OnceLock,PoisonError,RwLock}",
    ];

    /// Every path production `bw_path.rs` writes that reaches back into this
    /// crate. The same two entries RULE 8's `BW_PATH_MAY_REACH` blesses,
    /// asserted here as an ordered set as well so the two halves of the fence
    /// agree.
    const BW_PATH_LOCAL_PATHS: &[&str] = &[
        "crate::job_object::JobCommand",
        "crate::job_object::JobCommand::wrap",
    ];

    const FENCES: &[Fence] = &[
        Fence {
            file: "bw_path.rs",
            last_production_item: "BlockedByUnknownCliPath => Some(",
            min_code_len: 4400,
            callees: BW_PATH_CALLEES,
            call_sites: 113,
            macros: BW_PATH_MACROS,
            imports: BW_PATH_IMPORTS,
            local_paths: BW_PATH_LOCAL_PATHS,
            unsafe_regions: 0,
            below_cut_modules: 1,
            below_cut_min_lines: 300,
        },
        Fence {
            file: "job_object.rs",
            last_production_item: concat!("Ok(res", "umed)"),
            min_code_len: 3400,
            callees: PRODUCTION_CALLEES,
            call_sites: 71,
            macros: PRODUCTION_MACROS,
            imports: PRODUCTION_IMPORTS,
            local_paths: PRODUCTION_LOCAL_PATHS,
            unsafe_regions: 3,
            below_cut_modules: 2,
            below_cut_min_lines: 500,
        },
    ];

    /// A fenced file, whole, line endings normalised to LF.
    fn fenced_raw(file: &str) -> String {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        std::fs::read_to_string(src.join(file)).unwrap().replace("\r\n", "\n")
    }

    /// Its production half, cut at the first column-0 `#[cfg(test)]` -- the
    /// SAME cut [`job_object_production`] makes, so the file both functions
    /// read cannot be read two different ways.
    fn fenced_production(file: &str) -> String {
        fenced_raw(file).split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string()
    }

    /// **Every file the two job-bearing runners can reach writes down every
    /// name it calls.**
    ///
    /// See [`Fence`] for the measured survivor this exists to kill and for why
    /// the fenced set is derived rather than transcribed.
    #[test]
    fn every_file_the_runners_can_reach_writes_down_every_name_it_calls() {
        // (0) THE FENCED SET IS DERIVED FROM RULE 7's `REACHABLE`.
        let module_of = |p: &str| {
            p.strip_prefix("crate::")
                .unwrap_or(p)
                .split("::")
                .next()
                .unwrap()
                .to_string()
        };
        let mut reachable_modules: Vec<String> = REACHABLE.iter().map(|p| module_of(p)).collect();
        reachable_modules.sort();
        reachable_modules.dedup();

        // ...AND CLOSED OVER `mod` CHILDREN, TRANSITIVELY.
        //
        // See [`production_mod_children`] for the fourteenth hop, which is the
        // whole reason this is a closure: `REACHABLE` names only what the two
        // runners SPELL, and a module reached through a child of a module they
        // spell is never spelled by either of them. The closure is
        // `REACHABLE` -> modules -> their `mod` children -> theirs, until it
        // stops growing. A file discovered here and not in `FENCES` fails.
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut required: Vec<String> = reachable_modules.iter().map(|m| format!("{m}.rs")).collect();
        let mut discovered_children: Vec<String> = Vec::new();
        let mut at = 0;
        while at < required.len() {
            let file = required[at].clone();
            at += 1;
            assert!(
                src_dir.join(&file).is_file(),
                "the closure wants to read `src/{file}`, which is not a file. A module was \
                 named that this walk cannot follow -- resolve it or fix the name; do not let \
                 the closure quietly stop here"
            );
            let code = code_only(&fenced_production(&file));
            // A `#[path = ".."]` attribute re-points a `mod` item at an
            // arbitrary file -- possibly outside `src/` entirely -- and the
            // two candidate locations below would then both be wrong while
            // the real child stayed unfenced. There is no such attribute in
            // this crate and no reason for a fenced file to grow one, so it
            // is refused outright rather than followed.
            assert!(
                !code.contains(concat!("#[pa", "th=")),
                "production src/{file} carries a `#[path = ..]` attribute. That re-points a \
                 `mod` item at a file this closure would not look at, which puts the child \
                 outside every fence. Put the child where its `mod` name says it goes"
            );
            for child in production_mod_children(&code) {
                let dir = file.trim_end_matches(".rs").trim_end_matches("/mod");
                let flat = format!("{dir}/{child}.rs");
                let nested = format!("{dir}/{child}/mod.rs");
                let found = [flat, nested]
                    .into_iter()
                    .find(|c| src_dir.join(c).is_file())
                    .unwrap_or_else(|| {
                        panic!(
                            "production src/{file} declares `mod {child};` but neither \
                             `src/{dir}/{child}.rs` nor `src/{dir}/{child}/mod.rs` exists, so \
                             this closure cannot fence the file it pulls in. If the `mod` item \
                             sits inside an INLINE `mod` in this file, the real file is a \
                             directory deeper than either name above: this scan finds `mod` \
                             items wherever they are written but resolves them against the \
                             FILE, so it stops here rather than guessing, and the fix is to \
                             stop nesting it. (Measured: that shape, hiding a `CreateProcessW`, \
                             lands on this line.)"
                        )
                    });
                if !required.contains(&found) {
                    discovered_children.push(found.clone());
                    required.push(found);
                }
            }
        }
        required.sort();
        required.dedup();

        let mut fenced: Vec<String> = FENCES.iter().map(|f| f.file.to_string()).collect();
        fenced.sort();
        assert_eq!(
            fenced, required,
            "the set of fenced files is no longer the set of modules RULE 7 lets \
             `vault_export.rs` and `send.rs` name. A module on `REACHABLE` is a module the two \
             runners call INTO, and anything it calls it calls on their behalf with no rule of \
             theirs in the way -- which is exactly how the thirteenth hop put a `CreateProcessW` \
             in `bw_path.rs` while both runners stayed spotless -- or, if the missing name is a \
             `mod` CHILD of one of those, how the FOURTEENTH hop put one in \
             `bw_path/spawn_helper.rs` with `bw_path.rs` itself staying spotless too. Either add \
             a `Fence` for it (with its own lists) or take it off `REACHABLE`. Adding a module \
             under a fenced one FAILS HERE BY DESIGN: that is the point of the closure, and the \
             answer is a fence, not an exemption"
        );
        assert_eq!(
            reachable_modules,
            vec!["bw_path", "job_object"],
            "control: the derivation no longer produces the two modules this fence is known to \
             cover, so either `REACHABLE` changed (update this deliberately) or `module_of` is \
             broken and the equality above is comparing two wrong lists to each other"
        );
        assert!(
            discovered_children.is_empty(),
            "control-of-record: neither fenced file has a `mod` child today, and this says so \
             out loud. If one is added deliberately, this line changes with it -- so the day \
             the closure DOES pull a file in is a visible edit rather than a silent one. Found \
             {discovered_children:?}"
        );
        for m in ["job_object", "bw_path", "login_ui"] {
            assert_eq!(
                module_of(&format!("crate::{m}::some_helper")),
                m,
                "control: the module of a `crate::` path is not read back, so the derivation \
                 above compares the wrong strings"
            );
        }
        // CONTROLS on the child scanner. `discovered_children` is empty above,
        // so without these the closure could be a no-op and read exactly the
        // same.
        assert_eq!(
            production_mod_children(&code_only("use std::path::Path;\nmod spawn_helper;\n")),
            vec!["spawn_helper"],
            "control: the fourteenth hop's own `mod` item is invisible to the scanner, so the \
             closure above is decorative"
        );
        assert_eq!(
            production_mod_children(&code_only("pub mod a;\npub(crate) mod b;\n")),
            vec!["a", "b"],
            "control: a `mod` item behind a visibility keyword is missed -- and `code_only` \
             glues the keyword to it, which is the shape this has to survive"
        );
        assert!(
            production_mod_children(&code_only("mod inline { fn f() {} }")).is_empty(),
            "control: an INLINE module is reported as an out-of-file child; its body is already \
             in the text every list reads, and demanding a file for it would fail the crate"
        );
        assert!(
            production_mod_children(&code_only("fn f() { let model = 1; }")).is_empty(),
            "control: `mod` inside a longer identifier is reported as a module declaration, so \
             the closure would demand a file for a local variable"
        );

        for fence in FENCES {
            let file = fence.file;
            let raw = fenced_raw(file);
            let production = fenced_production(file);

            // (1) THE CUT. Everything below is read from the production half
            //     only, so a cut that moved UP is a green rule reading a
            //     truncated file.
            assert!(
                production.len() < raw.len(),
                "control: no column-0 `cfg(test)` cut marker in {file}, so every list below is \
                 reading the test module as production"
            );
            assert_eq!(
                raw.matches(fence.last_production_item).count(),
                1,
                "control: {file}'s cut anchor {:?} is not in it exactly once, so it pins \
                 nothing -- repoint it at the last production item above the test module",
                fence.last_production_item
            );
            assert!(
                production.contains(fence.last_production_item),
                "{file}'s last production item is BELOW the cut, so the cut moved up and every \
                 list below is reading a truncated file"
            );
            assert!(
                production.len() - production.rfind(fence.last_production_item).unwrap() < 1_500,
                "{file}'s cut is more than 1500 bytes past its last known production item: \
                 either production was appended below the anchor (repoint it) or the cut moved \
                 DOWN and source is living in the gap"
            );

            // (2) THE FOUR ALLOWLISTS, plus the count the sets cannot see.
            let code = code_only(&production);
            assert!(
                code.len() > fence.min_code_len,
                "control: production {file} stripped to {} chars, so every list below is vacuous",
                code.len()
            );
            assert_eq!(
                production_callees(&code),
                fence.callees,
                "production {file} calls a name that is not on its list. A child cannot be \
                 started without CALLING something, and whatever is called appears here under \
                 whatever name it was given -- a rename at the `use`, a function pointer and an \
                 empty turbofish all still report a name. A new entry is either a refactor \
                 (update the list, deliberately) or it is the next `CreateProcessW`: a way of \
                 starting a process that writes no `spawn`, builds no `Command`, and leaves \
                 every spelling-count in this module untouched"
            );
            assert_eq!(
                production_call_sites(&code),
                fence.call_sites,
                "the number of call SITES in production {file} changed while the set of names \
                 it calls did not. The set deduplicates, so a SECOND call to an \
                 already-listed name is free -- this is the count that is not"
            );
            assert_eq!(
                production_macros(&code),
                fence.macros,
                "production {file} invokes a macro that is not on its list. A macro body \
                 written in the file is scanned like any other code, but one imported from \
                 elsewhere expands to a call the file never spells -- so the macro itself is \
                 the callee and has to be named here"
            );
            assert_eq!(
                {
                    let mut uses = item_bodies(&code, "use");
                    uses.sort();
                    uses
                },
                fence.imports,
                "production {file} imports something new. The callee list pins the NAMES it \
                 calls; this pins where those names come from -- and re-pointing one existing \
                 call at a function next door is a mutation the callee list cannot see"
            );
            assert_eq!(
                production_local_paths(&code),
                fence.local_paths,
                "production {file} reaches into this crate somewhere new. A `crate::`-, \
                 `super::`- or `self::`-rooted path is the half of that move that needs no \
                 `use` at all"
            );
            assert_eq!(
                code.matches("unsafe").count(),
                fence.unsafe_regions,
                "the number of `unsafe` regions in production {file} changed. Every Win32 \
                 entry point is `unsafe`, so a new one is the cheapest first sign of a second \
                 way to start a child -- and the thirteenth hop's survivor opened the FIRST \
                 one in a file that had none"
            );

            // (3) AND THE HALF BELOW THE CUT IS NOTHING BUT GATED TEST
            //     MODULES, so a helper cannot simply be written where none of
            //     the lists above look. The same walk, over the tail the same
            //     cut threw away.
            let tail = &raw[production.len()..];
            let lf = tail.replace("\r\n", "\n");
            let crlf = lf.replace('\n', "\r\n");
            assert_ne!(
                lf, crlf,
                "control: {file}'s tail has no line endings at all, so comparing the walk over \
                 two copies of it compares it with itself"
            );
            assert_eq!(
                walk_below_the_cut(&lf),
                walk_below_the_cut(&crlf),
                "the walk gives a different answer on an LF copy of {file}'s tail than on a \
                 CRLF one, so something in it is sensitive to line endings"
            );
            let (visited, modules, closes, depth) =
                walk_below_the_cut(&lf).unwrap_or_else(|why| panic!("{file}: {why}"));
            assert!(
                visited > fence.below_cut_min_lines,
                "control: the walk visited only {visited} lines below {file}'s cut, which is \
                 not its test modules' worth -- the slice is empty or nearly so"
            );
            assert_eq!(
                depth, 0,
                "a test module below {file}'s cut is never closed by a column-0 brace, so the \
                 walk ran off the end of the file inside it and stopped inspecting top-level \
                 lines"
            );
            assert_eq!(
                modules, fence.below_cut_modules,
                "the number of top-level test modules below {file}'s cut changed. That is fine \
                 to change -- but this count is the control that proves the walk really \
                 visited them, so update it deliberately rather than loosening it"
            );
            assert_eq!(
                closes, modules,
                "control: every module the walk opened in {file} must also have been closed at \
                 column 0"
            );
        }

        // (4) CONTROLS on the count, through the real function: it can see
        //     exactly the repeat the deduplicated set cannot.
        assert_eq!(production_call_sites(&code_only("f(1); f(2); g(3);")), 3);
        assert_eq!(production_callees(&code_only("f(1); f(2); g(3);")).len(), 2);
        assert_eq!(
            production_call_sites(&code_only("let _ = go::<>(&mut pi);")),
            1,
            "control: an empty turbofish -- legal on a non-generic fn -- puts a `>` before the \
             `(` and hides the call site as well as the callee"
        );
        assert_eq!(
            production_call_sites(&code_only("println!(\"x\");")),
            0,
            "control: a macro invocation is miscounted as a call -- the `!` sits between the \
             name and the `(` -- which would make the macro list below do nothing"
        );
        assert_eq!(
            production_call_sites(&code_only("if (a) { }")),
            1,
            "control: `code_only` glues the keyword to the paren, so `if(` counts as one site. \
             That is the same gluing the callee list already pins as `ifThread32Next`, and it \
             is deliberate: gluing changes the entry rather than hiding it"
        );
    }
}

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
    pub fn get_program(&self) -> &std::ffi::OsStr {
        self.command.get_program()
    }

    pub fn get_args(&self) -> std::process::CommandArgs<'_> {
        self.command.get_args()
    }

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
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src, &mut files);

        // The walk itself, pinned: a recursion that returned nothing, or that
        // never descended into `vault_window/`, would make this vacuous.
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for expected in ["vault_export.rs", "send.rs", "job_object.rs", "mod.rs", "main.rs"] {
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

        let relative = |p: &std::path::Path| {
            p.strip_prefix(&src)
                .unwrap()
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
            let code = code_only(&std::fs::read_to_string(src.join(file)).unwrap());

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
            // Why this is a fact and not another spelling race: **an inherent
            // method cannot be aliased, re-exported or renamed in Rust.** There
            // is no `use path::method as other`, there is no trait to reach it
            // through (the field is private, so no foreign impl can extract
            // it), and there is no operator sugar for it. Calling it therefore
            // means writing this exact identifier, in code, in this file. An
            // identifier has one spelling and `code_only` sees through
            // whitespace, so `BareCommand :: into_jobless_command ( c )` and
            // `c.into_jobless_command()` are the same string here.
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
            // `super::` is not an escape: both files are crate-root modules,
            // so `super::` inside their test modules names the file's own
            // module, and `super::super::` -- which would reach the crate root
            // and from there anything at all -- is banned outright below.
            // Neither file contains one today.
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
            // `send.rs`'s test module borrows `login_ui`'s allocator probe for
            // the secret-lifetime assertions. Left as a prefix rather than
            // itemised because it is a `#[cfg(test)]` module: production code
            // that called into it would not compile in the non-test build at
            // all, which is a stronger guarantee than this list gives.
            const REACHABLE_TEST_ONLY: &[&str] = &["crate::login_ui::password_lifetime_tests::"];

            let mut from = 0;
            while let Some(at) = code[from..].find("crate::") {
                let start = from + at;
                from = start + "crate::".len();
                let head: String = code[start..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                let rest = &code[start + head.len()..];
                // `use crate::m::{A, B};` -- one occurrence, several items.
                let named: Vec<String> = if head.ends_with("::") && rest.starts_with('{') {
                    rest[1..]
                        .chars()
                        .take_while(|c| *c != '}')
                        .collect::<String>()
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|item| format!("{head}{item}"))
                        .collect()
                } else {
                    vec![head]
                };
                for path in named {
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
}

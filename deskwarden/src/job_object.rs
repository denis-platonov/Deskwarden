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
pub fn spawn_in_job(job: Option<&KillOnCloseJob>, mut command: Command) -> io::Result<Child> {
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
        let mut child = spawn_in_job(Some(&job), long_lived_command())
            .expect("spawn_in_job must succeed for a valid command");

        // The point of `CREATE_SUSPENDED` is that it is invisible to the
        // caller: the returned child must be resumed and running, not stuck.
        let still_running = child.try_wait().unwrap().is_none();
        kill_and_reap(&mut child);
        assert!(still_running, "child was not resumed after being assigned");
    }

    #[test]
    fn spawn_in_job_works_without_a_job_object() {
        let mut child = spawn_in_job(None, long_lived_command())
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
        let refused = spawn_in_job(Some(&job), long_lived_command());
        let _ = spawn_in_job(Some(&other), long_lived_command());
        let _ = spawn_in_job(None, long_lived_command());
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
        let mut child = spawn_in_job(None, long_lived_command())
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
    fn direct_child_starts(label: &str, text: &str) -> Vec<String> {
        let needles = [
            concat!(".spa", "wn()"),
            concat!(".out", "put()"),
            concat!(".sta", "tus()"),
        ];
        text.lines()
            .enumerate()
            .filter(|(_, line)| needles.iter().any(|n| line.contains(n)))
            .map(|(i, line)| format!("{label}:{}: {}", i + 1, line.trim()))
            .collect()
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
        // ...and does not flag the choke-point call itself.
        assert!(direct_child_starts(
            "planted.rs",
            "let child = crate::job_object::spawn_in_job(self.job(), command)?;"
        )
        .is_empty());
    }

    #[test]
    fn closing_the_job_kills_its_members() {
        // The whole reason the job object exists: this is what protects an
        // unlocked `bw serve` from outliving us after a panic or a Task
        // Manager kill, and nothing else in the test suite exercises it.
        let job = KillOnCloseJob::new().unwrap();
        let mut child =
            spawn_in_job(Some(&job), long_lived_command()).expect("failed to spawn test child");
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

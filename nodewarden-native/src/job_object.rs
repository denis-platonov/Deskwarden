//! A Windows Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!
//! `bw serve` holds an unlocked vault and serves it over plain HTTP on
//! localhost. Killing it only on the tray's Quit path is not enough: a panic,
//! a Ctrl+C, or a Task Manager kill of `nodewarden-native` leaves it running
//! and serving decrypted secrets with nothing left to stop it.
//!
//! A job object with kill-on-close moves that guarantee into the kernel. Every
//! process assigned to the job is terminated when the last handle to the job
//! closes -- and the OS closes our handles unconditionally when this process
//! dies, whatever the reason (including `process::exit`, which skips
//! destructors, and an unhandled panic or an external `TerminateProcess`).

use std::io;
use std::os::windows::io::AsRawHandle;
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
pub struct KillOnCloseJob {
    handle: HANDLE,
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

            Ok(Self { handle })
        }
    }

    /// Assigns an already-spawned child process to this job.
    ///
    /// Safe to call on a process that has already exited (it simply fails);
    /// callers should log rather than treat that as fatal.
    pub fn assign(&self, child: &Child) -> windows::core::Result<()> {
        let process_handle = HANDLE(child.as_raw_handle());
        unsafe { AssignProcessToJobObject(self.handle, process_handle) }
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
pub fn spawn_in_job(job: Option<&KillOnCloseJob>, mut command: Command) -> io::Result<Child> {
    let Some(job) = job else {
        return command.spawn();
    };

    // OR-ed into whatever std sets internally; it does not replace those.
    command.creation_flags(CREATE_SUSPENDED.0);
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

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // Closing the last handle is what triggers the kill. That's the
        // intended behaviour, not a leak being cleaned up.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

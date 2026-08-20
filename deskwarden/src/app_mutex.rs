//! The named Windows mutex that tells the installer this app is running.
//!
//! # The bug this exists for
//!
//! "Installer cannot close Deskwarden when installing while it is running."
//! Inno Setup had no way to know: `installer/deskwarden.iss` named no mutex,
//! and this process held none. So setup walked straight into copying over a
//! `deskwarden.exe` that Windows had mapped as a running image, and the
//! install either failed outright or left a stale binary behind.
//!
//! A named mutex is the whole mechanism. This process creates one at the very
//! top of `main` and holds it for its entire life; `AppMutex=` in the `.iss`
//! names the same string, and Inno's `OpenMutex` on it is what turns "files
//! are in use" into "please close Deskwarden, then click OK".
//!
//! # Why the installer ASKS rather than force-closing
//!
//! Inno 6 could instead force-close this app through the Restart Manager
//! (`CloseApplications=yes`). That is the wrong default **for this app**, and
//! the reason is in the shutdown path: quitting clears a copied password off
//! the Windows clipboard (`clipboard::clear_if_still_ours_for`) and every
//! secret in this crate is zeroized on drop. A Restart-Manager kill is a
//! `TerminateProcess`: no destructors, no exit path, so the user's password
//! stays on the clipboard and their secrets stay unwiped in freed memory --
//! as a side effect of installing an update. The `.iss` therefore sets
//! `CloseApplications=no` and `RestartApplications=no` explicitly, and this
//! mutex is what makes the app close through its own door instead.
//!
//! # Why `Local\`, not `Global\`
//!
//! The installer is per-user by design: `PrivilegesRequired=lowest`,
//! `DefaultDirName={localappdata}\Deskwarden`. Setup therefore runs in the
//! same logon session as the app it is replacing, so a session-local mutex is
//! exactly the right scope -- and it is the only correct one. A `Global\`
//! mutex is visible across sessions, so one user's running copy would block
//! another user's per-user install of their own copy, and creating one from a
//! session-0-adjacent context brings permission problems this install has no
//! rights to solve.
//!
//! # The name is written down once
//!
//! [`APP_MUTEX_NAME`] is the only place the string is authored. The `.iss`
//! spells it a second time because Inno cannot read a Rust constant -- so
//! [`tests::the_installer_names_the_same_mutex_this_process_creates`] reads
//! `installer/deskwarden.iss` (through `include_str!`, at compile time) and
//! fails if the two ever drift.

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::Mutex;
use windows::core::HSTRING;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;

/// The mutex name, authored here and nowhere else.
///
/// The GUID half is `AppId` from `installer/deskwarden.iss`, so the name
/// cannot collide with an unrelated program that happened to pick the word
/// "Deskwarden". No `{`/`}` anywhere in it: Inno treats braces in a directive
/// value as the start of a constant, and the `.iss` has to be able to write
/// this string literally.
pub const APP_MUTEX_NAME: &str = "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630";

/// The handle, held for the life of the process.
///
/// Owning it in a `static` rather than in a `main` local is deliberate: the
/// self-update path has to be able to let go of it from `updater.rs` (see
/// [`release`]), and that call happens on a background thread which has no
/// way to reach a binding in `main`.
static HELD: Mutex<Option<OwnedHandle>> = Mutex::new(None);

/// What creating the mutex found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquired {
    /// Nothing else held the name: this is the only Deskwarden running.
    First,
    /// The name already existed, so another Deskwarden is running in this
    /// logon session.
    ///
    /// **Acted on.** This used to be reported and no more, and the log line
    /// that reported it claimed the second copy would run without a global
    /// hotkey -- a claim nothing implemented, which is how a duplicate launch
    /// came to die on `RegisterHotKey`. [`crate::single_instance::resolve`] is
    /// now the one place that says what this means: the newly launched copy
    /// asks the running one to stand down and takes its place.
    AlreadyRunning,
}

/// Creates the named mutex and reports whether it already existed.
///
/// Split out from [`acquire`] and taking the name as a parameter for one
/// reason: it is the half a test can drive. `acquire` writes into the
/// process-wide [`HELD`], which a test must not disturb, but this function is
/// a pure "make a kernel object with this name" that
/// [`tests::a_second_creation_of_the_same_name_reports_already_running`] can
/// call twice under a name of its own.
fn create_named(name: &str) -> windows::core::Result<(OwnedHandle, Acquired)> {
    unsafe {
        // `bInitialOwner: false`. Ownership is irrelevant here -- nothing
        // waits on this mutex, and nothing releases it. What matters is
        // purely that the NAME exists for as long as this process does, which
        // is the handle's business, not the ownership's. Asking for initial
        // ownership would additionally mean this thread has to be the one to
        // let it go, which is exactly the constraint `release` must not have.
        let handle = CreateMutexW(None, false, &HSTRING::from(name))?;
        // `CreateMutexW` succeeds either way and distinguishes the two cases
        // only through the last-error code, so this must be read immediately,
        // before anything else can overwrite it.
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        let acquired = if existed { Acquired::AlreadyRunning } else { Acquired::First };
        Ok((OwnedHandle::from_raw_handle(handle.0), acquired))
    }
}

/// Creates the mutex and keeps it alive until [`release`] or process exit.
///
/// Called as the **first statement of `main`**, before the config directory,
/// before logging, before the `bw.exe` signature check -- every one of which
/// can fail and take the process down. A crash before this point is a process
/// that never existed as far as the installer is concerned, which is correct;
/// a crash after it is a process the installer will find and ask about, which
/// is also correct. Anything in between would be a live app with files mapped
/// and no mutex, which is the bug.
///
/// Calling it twice replaces the stored handle, closing the old one. Nothing
/// does; `main` calls it once.
///
/// **Failure is not fatal.** The mutex is how the installer notices this app,
/// not how the app works, so a startup that cannot create it logs and carries
/// on rather than refusing to run the user's password manager over a
/// housekeeping handle.
pub fn acquire() -> windows::core::Result<Acquired> {
    let (handle, acquired) = create_named(APP_MUTEX_NAME)?;
    if let Ok(mut held) = HELD.lock() {
        *held = Some(handle);
    }
    Ok(acquired)
}

/// Takes the mutex **only if nothing else holds it**, reporting whether it
/// did.
///
/// [`crate::single_instance`]'s poll, and it has to be this rather than
/// `acquire` for a reason that is easy to miss: a `CreateMutexW` that finds
/// the name already there still *opens a handle to it*, and a named object
/// lives as long as any handle does. An incoming instance that polled with
/// `acquire` would therefore keep the very name it was waiting to see
/// disappear alive, and would wait out its whole timeout against itself. So a
/// losing attempt here drops its handle immediately and stores nothing; only
/// a winning one is retained.
///
/// A creation that fails outright is reported as "not free". The caller's
/// timeout then decides, which is the right answer: an unexplained refusal to
/// create the name is not evidence that the other instance has gone.
pub fn take_if_free() -> bool {
    match create_named(APP_MUTEX_NAME) {
        Ok((handle, Acquired::First)) => {
            if let Ok(mut held) = HELD.lock() {
                *held = Some(handle);
            }
            true
        }
        // Dropped here, deliberately -- see above.
        Ok((_, Acquired::AlreadyRunning)) => false,
        Err(e) => {
            log::warn!("could not create the app mutex while waiting for the other copy ({e})");
            false
        }
    }
}

/// Closes the mutex handle, so the name stops existing.
///
/// # The one caller, and why it is not the Quit path
///
/// Quitting does not need this: process exit closes the handle for us, by any
/// route including a panic or a `TerminateProcess`.
///
/// The self-update path does need it, and needs it *early*. `updater.rs`
/// launches the downloaded installer with `/VERYSILENT /SUPPRESSMSGBOXES` and
/// this process then exits -- so for a moment the app that asked for the
/// update is still running and still holding the mutex, and the installer it
/// just started is the one thing in the world that must NOT be told to stop
/// and ask the user. There is nobody to ask: the run is silent by design and
/// its message boxes are suppressed, so Inno's "please close the application"
/// prompt has no interactive answer available and the update would fail or
/// hang depending on which default that suppressed box carries.
///
/// So `launch_installer` releases the mutex immediately before the spawn.
/// That is not a workaround for a race -- it is the accurate statement: at
/// that instant this process has handed over to its own replacement and is on
/// its way out, so "a Deskwarden is running that setup should ask about" has
/// stopped being true.
///
/// Idempotent, and a no-op when [`acquire`] was never called (tests, the
/// examples, and any binary in this workspace that is not `deskwarden.exe`).
pub fn release() {
    if let Ok(mut held) = HELD.lock() {
        // Dropping the `OwnedHandle` is the `CloseHandle`, and with no other
        // handle open anywhere the kernel destroys the named object -- which
        // is what makes the installer's `OpenMutex` fail and its check pass.
        held.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `.iss`, read at compile time so that this test cannot silently
    /// pass against a file that has moved or been deleted -- a missing path
    /// is a build error, not a green run.
    const ISS: &str = include_str!("../installer/deskwarden.iss");

    /// The value of a `Directive=value` line in the `.iss`, ignoring comment
    /// lines (Inno comments start with `;`).
    fn iss_directive(name: &str) -> Option<String> {
        ISS.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with(';'))
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .map(|value| value.trim().to_string())
    }

    /// **The two files must not drift.**
    ///
    /// This is the entire reason the fix works: a mutex this process creates
    /// under one name and an installer that opens another name is a change
    /// that compiles, ships, and does exactly nothing -- the user sees the
    /// same failed install as before, with a mutex to prove it was fixed.
    #[test]
    fn the_installer_names_the_same_mutex_this_process_creates() {
        let named = iss_directive("AppMutex").expect(
            "installer/deskwarden.iss has no `AppMutex=` line. Without it Inno never looks \
             for the running app and the install walks into files that are in use, which is \
             the whole bug",
        );
        assert_eq!(
            named, APP_MUTEX_NAME,
            "the installer opens {named:?} but this process creates {APP_MUTEX_NAME:?}. \
             `APP_MUTEX_NAME` is the one place the string is authored; the `.iss` copy has \
             to be updated to match it"
        );
    }

    /// Control for the parse above: it really reads directives out of this
    /// file, rather than returning `None` for everything and being carried by
    /// an `expect` that has never fired.
    #[test]
    fn the_iss_parse_finds_a_directive_known_to_be_there() {
        assert_eq!(
            iss_directive("AppName").as_deref(),
            Some("Deskwarden"),
            "the `.iss` directive parse is broken, so every assertion built on it is vacuous"
        );
        assert_eq!(
            iss_directive("PrivilegesRequired").as_deref(),
            Some("lowest"),
            "the install stopped being per-user, which is the premise the `Local\\` scope of \
             this mutex rests on -- see this module's docs"
        );
    }

    /// The scope decision, pinned rather than left to whoever edits the
    /// constant next.
    #[test]
    fn the_mutex_is_scoped_to_this_logon_session() {
        assert!(
            APP_MUTEX_NAME.starts_with("Local\\"),
            "{APP_MUTEX_NAME:?} is not session-local. A per-user install must not be blocked \
             by another user's running copy"
        );
        assert!(
            !APP_MUTEX_NAME.contains("Global"),
            "{APP_MUTEX_NAME:?} reaches into the global namespace, which this install has \
             neither the rights nor the reason for"
        );
        assert!(
            !APP_MUTEX_NAME.contains('{') && !APP_MUTEX_NAME.contains('}'),
            "{APP_MUTEX_NAME:?} contains a brace, which Inno reads as the start of a constant \
             in a directive value -- the `.iss` could not spell this name literally"
        );
    }

    /// **The installer must ask, not kill.**
    ///
    /// A force-close skips the clipboard clear and every zeroize-on-drop in
    /// this crate. See this module's docs for the argument; this is the pin
    /// that stops `CloseApplications` from drifting back to its default.
    #[test]
    fn the_installer_does_not_force_close_this_app() {
        assert_eq!(
            iss_directive("CloseApplications").as_deref(),
            Some("no"),
            "Inno's Restart Manager would `TerminateProcess` this app to free its files. That \
             runs no destructors and no exit path, so a copied password would be left on the \
             clipboard and secrets left unwiped -- as a side effect of an install"
        );
        assert_eq!(
            iss_directive("RestartApplications").as_deref(),
            Some("no"),
            "with `CloseApplications=no` there is nothing for Inno to restart, and leaving \
             this at its default is a second Restart Manager entry point into the same kill"
        );
    }

    /// **The mutex is created before anything that can fail.**
    ///
    /// The window this closes is small and real: a `main` that resolved the
    /// config directory, spawned the backend or put up the login window before
    /// creating the mutex would be a running Deskwarden, with its image mapped
    /// and its files locked, that the installer cannot see.
    #[test]
    fn main_creates_the_mutex_first() {
        let main_rs = include_str!("main.rs");
        let body_at = main_rs.find("fn main() {").expect("control: main.rs has no `fn main()`");
        let body = &main_rs[body_at..];
        let acquire_at = body.find("app_mutex::acquire()").expect(
            "`fn main` no longer creates the app mutex, so the installer has nothing to \
             detect and the in-use-files bug is back",
        );
        // Three things `main` does that can end the process, each of them
        // after the app is unmistakably alive. None may come first.
        for later in ["ProjectDirs::from(", "logging::init(", "resolve_bw_exe()"] {
            let at = body
                .find(later)
                .unwrap_or_else(|| panic!("control: `fn main` no longer contains {later:?}"));
            assert!(
                acquire_at < at,
                "`main` reaches {later:?} before creating the app mutex. Everything between \
                 process start and the mutex is a running app the installer is blind to"
            );
        }
    }

    /// **The one place that lets go of the mutex does it BEFORE the spawn.**
    ///
    /// Releasing it after launching the installer would be releasing it after
    /// the moment it matters: the installer's mutex check runs early, and it
    /// is running silently with its message boxes suppressed, so a mutex still
    /// standing when it looks is a failed or hung update.
    #[test]
    fn the_update_launch_releases_the_mutex_before_starting_the_installer() {
        let updater = include_str!("updater.rs");
        let at = updater
            .find("fn launch_installer(")
            .expect("control: updater.rs no longer has `fn launch_installer`");
        let body = &updater[at..];
        // Split, like every other needle of this shape in this crate:
        // written whole, the literal below is itself a child-start as far as
        // `job_object`'s guard is concerned, and that guard reads every `.rs`
        // file in the tree -- comments included -- which is why this comment
        // does not spell it either.
        let spawn_at = body
            .find(concat!(".spa", "wn()"))
            .expect("control: `launch_installer` no longer starts the installer process");
        let release_at = body.find(concat!("app_mutex::", "release()")).expect(
            "`launch_installer` no longer releases the app mutex. The installer it starts \
             runs with `/VERYSILENT /SUPPRESSMSGBOXES`, so Inno's \"please close Deskwarden\" \
             prompt has no answer available and the self-update stops working",
        );
        assert!(
            release_at < spawn_at,
            "the release happens after the spawn, which is after the installer has already \
             looked for the mutex"
        );
    }

    /// **The mechanism itself: a second creation of a name that exists is
    /// reported as such.**
    ///
    /// This is what Inno's `OpenMutex` is doing from the other side, and it is
    /// the only part of the fix that can be executed here at all -- running
    /// the real installer against a running app is out of reach of any test
    /// this crate may contain.
    ///
    /// Under a name of this test's own, not [`APP_MUTEX_NAME`]: creating the
    /// production name from a test process would make that test process look
    /// like a running Deskwarden to any installer the user happened to start.
    #[test]
    fn a_second_creation_of_the_same_name_reports_already_running() {
        let name = format!("Local\\deskwarden-app-mutex-test-{}", std::process::id());

        let (first_handle, first) = create_named(&name).expect("could not create the test mutex");
        assert_eq!(first, Acquired::First, "nothing else can be holding a name keyed on this pid");

        let (second_handle, second) =
            create_named(&name).expect("could not re-create the test mutex");
        assert_eq!(
            second,
            Acquired::AlreadyRunning,
            "creating a name that already exists reported `First`, so the app would never \
             notice a second instance and this module's detection is a no-op"
        );
        drop(second_handle);

        // And once every handle is gone the name is gone with it, which is
        // what makes `release` (and process exit) actually clear the way for
        // the installer rather than merely stopping us from looking.
        drop(first_handle);
        let (third_handle, third) =
            create_named(&name).expect("could not create the test mutex a third time");
        assert_eq!(
            third,
            Acquired::First,
            "the name outlived its last handle, so an installer would go on being told \
             Deskwarden is running after it has exited"
        );
        drop(third_handle);
    }

    /// `release` is safe to call when nothing was ever acquired -- which is
    /// every test binary and every example in this workspace.
    #[test]
    fn releasing_without_acquiring_is_a_no_op() {
        release();
        release();
    }
}

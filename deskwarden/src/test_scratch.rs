//! One scratch directory for the whole test suite, and it removes itself.
//!
//! Test-only at the declaration in `lib.rs`, exactly like [`crate::test_http`]
//! and [`crate::test_vault`]. The gate is `#[cfg(any(test, feature =
//! "test-support"))]` rather than a bare `#[cfg(test)]` for `test_http`'s
//! reason: `main.rs` is a SEPARATE CRATE that links the library built WITHOUT
//! `cfg(test)`, so a `cfg(test)` module is invisible to it, and `main.rs` has
//! scratch directories of its own.
//!
//! # What went wrong without this
//!
//! Every test family that needed somewhere to write invented its own scratch
//! directory, and none of them deleted it. A measurement on the author's
//! machine found **26,562 leftover `deskwarden-*` directories in `%TEMP%`,
//! holding 29,940 files and 643.7 MB**, the oldest three weeks old. They came
//! from at least eight test families under two incompatible naming
//! conventions:
//!
//! - `deskwarden-diskcache-test-<tag>-<nanos>` -- namespaced by a wall-clock
//!   nanosecond stamp.
//! - `deskwarden-fill-dispatch-<tag>-<pid>-ThreadId(<n>)-<nanos>` -- namespaced
//!   by process id and thread id.
//!
//! This module is the third convention only in the sense that it REPLACES both:
//! the point is one mechanism, not one more.
//!
//! # Why the name is not just the process id
//!
//! Leftovers are not only untidy here, they have already caused a failure.
//! `scan_history`'s test helper namespaced its scratch file by
//! `std::process::id()` alone. 2,004 of those files accumulated, Windows
//! recycled a pid, and a run loaded a stale record written by a process that
//! had exited days earlier. The test failed for a reason nothing in it
//! mentioned.
//!
//! So the name here is `deskwarden-<tag>-<pid>-<seed>-<n>`, and each part is
//! load-bearing:
//!
//! - `pid` separates concurrently running test processes. Two live processes
//!   cannot share one, which is exactly the guarantee a pid does give.
//! - `n` is an [`AtomicUsize`] counter, so two directories from the SAME
//!   process cannot collide however fast they are made. A nanosecond clock was
//!   the old answer to this and is a weaker one: `SystemTime` is not monotonic,
//!   and its resolution on Windows is coarser than the gap between two
//!   `create_dir_all` calls in a tight loop.
//! - `seed` is a wall-clock stamp taken ONCE per process. The counter restarts
//!   at zero in every process, so `pid` + counter alone would still collide with
//!   a directory left behind by an earlier run that drew the same recycled pid
//!   -- the `scan_history` failure exactly. [`Drop`] normally means no such
//!   directory survives, but [`Drop`] does not run when a process is killed or
//!   aborts, and the whole point of this module is to not depend on the happy
//!   path. `seed` is what makes a recycled pid harmless.
//!
//! # Why this does not sweep away strays
//!
//! It would be easy to have the first [`ScratchDir::new`] of a process delete
//! every `deskwarden-*` directory it finds, and that would be a **race with
//! data loss on the losing side**. `%TEMP%` is shared: the author's own runs,
//! CI, and several agents run this suite at once. A sweep cannot tell a stray
//! from a directory another live process is in the middle of writing, because
//! the only evidence either way is a name it does not own. Deleting the wrong
//! one turns a tidy-up into a failure in an unrelated process, which is
//! strictly worse than the disk usage it saves.
//!
//! The leak is fixed at the source instead. What is already on disk is a
//! one-time manual cleanup, and one that a person can do when no run is live.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Monotonic within the process. See the module doc for why a counter and not
/// a clock.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Taken once per process, so a recycled pid cannot name a directory an
/// earlier run left behind. See the module doc.
fn seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    })
}

/// A directory under [`std::env::temp_dir`] that is **removed when this value
/// is dropped, including when the test that made it panics**.
///
/// That last part is the reason this is a guard and not a `cleanup()` call at
/// the end of a test. Most of the tests that use it assert, and an assertion
/// that fires unwinds past any trailing statement -- so a tidy-up written at
/// the end of the happy path is a tidy-up that runs only when it is least
/// needed. Unwinding runs `Drop`; that is the whole mechanism.
///
/// Derefs to [`Path`], so `scratch.join("stats.json")` and passing `&scratch`
/// where a `&Path` is wanted both work and the call sites read as they did
/// when they held a bare `PathBuf`.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Creates `%TEMP%\deskwarden-<tag>-<pid>-<seed>-<n>`.
    ///
    /// `tag` is the caller's own label and is only ever there to make a
    /// directory recognisable while a test is running -- uniqueness comes from
    /// the three fields after it, never from the tag.
    pub fn new(tag: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "deskwarden-{tag}-{}-{:x}-{n}",
            std::process::id(),
            seed()
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("a writable scratch directory at {path:?}: {e}"));
        Self { path }
    }

    /// The directory itself.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for ScratchDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.path, f)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best-effort and deliberately silent. A panicking `Drop` during an
        // unwind aborts the process, which would replace a legible test
        // failure with one that says nothing -- and this is a tidy-up, not an
        // assertion. On Windows a removal can also lose to a virus scanner or
        // an indexer holding a handle open for a moment; that is a stray
        // directory, not a test result.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard's whole claim, and a control proving the assertion that
    /// checks it is capable of failing.
    ///
    /// Without the control this test passes for a guard that does nothing at
    /// all -- which is the house defect class, "a test that passes because it
    /// never reached the thing it names". The control creates a directory the
    /// same way and does NOT drop a guard over it, and asserts it is still
    /// there. If `exists()` were somehow always false, the control fails.
    #[test]
    fn the_directory_is_gone_after_the_guard_drops() {
        let path = {
            let scratch = ScratchDir::new("guard-drops");
            std::fs::write(scratch.join("a-file"), b"contents").expect("writes");
            assert!(scratch.exists(), "the guard did not create its directory");
            scratch.path().to_path_buf()
        };
        assert!(
            !path.exists(),
            "{path:?} survived the guard's drop, so every test using it still leaks"
        );

        // ---- Positive control: the same directory, no guard dropped over it.
        let leaked = ScratchDir::new("guard-control");
        let leaked_path = leaked.path().to_path_buf();
        std::fs::write(leaked.join("a-file"), b"contents").expect("writes");
        assert!(
            leaked_path.exists(),
            "control: a directory whose guard is still alive was already gone, so the \
             assertion above would hold even if `Drop` did nothing"
        );
        drop(leaked);
        assert!(!leaked_path.exists(), "the control's own guard did not clean up either");
    }

    /// **The path that matters.** Half the tests using this assert, so the
    /// case that must clean up is the failing one -- and a `cleanup()` call
    /// written at the end of a test body is precisely the shape that does not.
    #[test]
    fn a_panicking_test_still_cleans_up_after_itself() {
        let captured: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));

        let sink = captured.clone();
        let previous = std::panic::take_hook();
        // The default hook would print this deliberate panic's backtrace into
        // the test output, where it reads as a real failure.
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(move || {
            let scratch = ScratchDir::new("guard-panics");
            std::fs::write(scratch.join("a-file"), b"contents").expect("writes");
            *sink.lock().expect("not poisoned") = Some(scratch.path().to_path_buf());
            panic!("what an asserting test does");
        });
        std::panic::set_hook(previous);

        assert!(outcome.is_err(), "control: the closure was supposed to panic and did not");
        let path = captured
            .lock()
            .expect("not poisoned")
            .take()
            .expect("control: the closure never got as far as making its directory");
        assert!(
            !path.exists(),
            "{path:?} survived a panic. `Drop` is the only thing that runs while an \
             assertion unwinds, so a guard that does not clean up here does not clean up \
             for any test that fails"
        );
    }

    /// A pid is not enough on its own -- that is what recycled under
    /// `scan_history` -- and neither is a tag. Two guards made back to back
    /// must be two directories.
    #[test]
    fn two_scratch_directories_with_one_tag_do_not_collide() {
        let a = ScratchDir::new("same-tag");
        let b = ScratchDir::new("same-tag");
        assert_ne!(
            a.path(),
            b.path(),
            "two scratch directories share a name, so one test can read another's files \
             and dropping either takes both"
        );
        std::fs::write(a.join("who"), b"a").expect("writes");
        std::fs::write(b.join("who"), b"b").expect("writes");
        assert_eq!(std::fs::read(a.join("who")).expect("reads"), b"a");
        assert_eq!(std::fs::read(b.join("who")).expect("reads"), b"b");
    }

    /// The name carries the process id AND a per-process seed. The seed is
    /// what a recycled pid cannot reproduce, so it is the field the
    /// `scan_history` failure turns on.
    #[test]
    fn the_name_is_namespaced_by_more_than_the_process_id() {
        let scratch = ScratchDir::new("naming");
        let name = scratch
            .path()
            .file_name()
            .expect("a directory name")
            .to_string_lossy()
            .into_owned();
        let pid = std::process::id().to_string();
        assert!(name.starts_with("deskwarden-naming-"), "unexpected name {name:?}");
        assert!(name.contains(&pid), "{name:?} does not carry the process id");
        assert!(
            name.contains(&format!("{:x}", seed())),
            "{name:?} carries no per-process seed, so a recycled pid plus a counter that \
             restarts at zero would reproduce a name an earlier run had"
        );
    }

    /// The guard is not confined to a directory it made empty: a test that
    /// wrote a tree under it is the normal case, and 643.7 MB of the leak was
    /// exactly that.
    #[test]
    fn a_directory_with_contents_under_it_is_removed_whole() {
        let path = {
            let scratch = ScratchDir::new("nested");
            std::fs::create_dir_all(scratch.join("one/two")).expect("creates");
            std::fs::write(scratch.join("one/two/three.bin"), vec![0u8; 4096]).expect("writes");
            scratch.path().to_path_buf()
        };
        assert!(!path.exists(), "{path:?} survived because it was not empty");
    }
}

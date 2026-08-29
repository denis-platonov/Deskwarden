//! The one place a `mockito` server is created, and the gate that keeps only
//! one of them serving at a time.
//!
//! Test-only at the declaration in `lib.rs`, exactly like [`crate::test_vault`]
//! and [`crate::below_cut`], so nothing here can ship. It is not a `cfg(test)`
//! seam: production code does not call it and does not change shape because it
//! exists.
//!
//! # Why this module exists
//!
//! `mockito` 1.7.2's server resets accepted connections when several of them
//! are in flight anywhere in the process. Not refuses -- **resets**: the client
//! connects, writes its request head, and then the read fails with
//! `WSAECONNRESET` (`os error 10054`) before a status line arrives. Every
//! module in this crate that stands up a mock server saw it, on a failing set
//! that changed from run to run: `rest::api`, `rest::backend`, `vault_cache`,
//! `vault_bridge`, `breach`, `updater`, `picker_ui`, `bw_serve`,
//! `app::fill_dispatch_tests`, `vault_window::spawn_vault_load_tests`.
//!
//! It is worth being precise about what that is *not*, because three plausible
//! explanations were measured and all three are wrong:
//!
//! * **Not Windows ephemeral-port churn.** The same client code, at the same
//!   connection rate and the same thread count, against a hand-rolled
//!   [`std::net::TcpListener`] instead of a mock server: **0 failures in 2400
//!   requests**, where `mockito` failed 706. Port recycling cannot tell the two
//!   servers apart.
//! * **Not `ureq` connection pooling.** The failing request is often the first
//!   one a freshly built agent makes, so its pool is empty. Driving the same
//!   server with a raw [`std::net::TcpStream`] and a hand-written request head
//!   -- no `ureq` in the process at all -- reproduces it at the same rate:
//!   1847 answered, **553 reset after the head was written**.
//! * **Not `mockito`'s own server pool.** `Server::new` hands out a recycled
//!   server; `Server::new_with_opts` bypasses the pool entirely. Measured side
//!   by side under identical load: 670/2400 pooled, 764/2400 unpooled. A
//!   long-lived server that is never recycled at all fails just as often
//!   (657/2400).
//!
//! What every measurement *did* track was **how many mock-server requests are
//! in flight at once**, and nothing else:
//!
//! | concurrent test threads | failures |
//! |---|---|
//! | 1 (one long-lived server, sequential) | 1 / 1000 |
//! | 4 | 57 / 400 |
//! | 24 | 706 / 2400 |
//!
//! One server with 24 clients and 24 servers with one client each fail at the
//! same rate, so it is the traffic that matters, not how it is spread over
//! servers. That is why the gate below counts *servers held*, and why its
//! limit is one rather than some larger number that would merely make the
//! failure rarer.
//!
//! # Why a gate and not a retry
//!
//! Retrying a reset request, sleeping, or widening a timeout would turn this
//! suite into one that cannot report a transport failure at all -- and a real
//! `Transport` error is exactly what several tests in `rest::api` and
//! `vault_bridge` assert. The gate removes the condition instead: with one
//! server serving at a time the reset does not happen, and a reset that does
//! happen still fails its test loudly.

use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex};
use std::thread::ThreadId;

/// How many tests may hold a mock server at once.
///
/// One, for the reason in the module docs: at four concurrent threads the
/// reset rate was already 14%. This is not a tuning knob to raise if the suite
/// feels slow -- raising it re-admits the failure in proportion.
const CONCURRENT_SERVERS: usize = 1;

/// Servers currently held, paired with [`RELEASED`].
static HELD: Mutex<usize> = Mutex::new(0);

/// Signalled every time a [`MockServer`] is dropped.
static RELEASED: Condvar = Condvar::new();

thread_local! {
    /// How many servers *this* thread already holds.
    ///
    /// The gate is re-entrant through this counter, and it has to be: two
    /// tests in this crate stand up a second server while holding the first
    /// (`rest::api`'s `the_servers_own_refusals_on_the_archive_routes_are_kept_as_themselves`
    /// takes three), and several more add one to a fixture that already
    /// carries one. Without it those tests would wait on a permit they are
    /// themselves holding and hang the suite.
    static HELD_BY_THIS_THREAD: Cell<usize> = const { Cell::new(0) };
}

/// A `mockito` server, held under the gate described in the module docs.
///
/// Derefs to [`mockito::Server`], so a call site reads exactly as it did when
/// it called `mockito::Server::new` directly: `server.mock(...)`,
/// `server.url()`, `server.socket_address()`.
pub struct MockServer {
    /// Declared first so it drops -- resetting the mocks and returning the
    /// server to `mockito`'s pool -- before [`Drop`] hands the permit on. A
    /// waiting test that got the permit first would otherwise be racing this
    /// server's teardown.
    server: mockito::Server,
    /// The thread that took the permit, so a server dropped on a different
    /// thread than it was built on still returns the permit exactly once
    /// without corrupting that other thread's re-entrancy count.
    owner: ThreadId,
}

/// A `mockito` server, waiting for the gate if another test holds one.
///
/// This is the only place in the crate that calls `mockito::Server::new`, and
/// [`crate::below_cut`]'s source walk is not what enforces that --
/// `only_this_module_builds_a_mock_server` below is.
#[must_use]
pub fn server() -> MockServer {
    let reentrant = HELD_BY_THIS_THREAD.with(|held| {
        let depth = held.get();
        held.set(depth + 1);
        depth > 0
    });

    if !reentrant {
        // `unwrap_or_else(into_inner)` and not `unwrap`: a test that panics
        // while holding a server poisons this mutex, and a poisoned gate would
        // turn one real failure into every later mock-server test failing for
        // a reason that has nothing to do with what it was checking. The guard
        // protects a count, and a count is still readable after a panic.
        let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while *held >= CONCURRENT_SERVERS {
            held = RELEASED.wait(held).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *held += 1;
    }

    MockServer {
        server: mockito::Server::new_with_opts(mockito::ServerOpts::default()),
        owner: std::thread::current().id(),
    }
}

impl Deref for MockServer {
    type Target = mockito::Server;

    fn deref(&self) -> &Self::Target {
        &self.server
    }
}

impl DerefMut for MockServer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.server
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let outermost = if std::thread::current().id() == self.owner {
            HELD_BY_THIS_THREAD.with(|held| {
                let depth = held.get().saturating_sub(1);
                held.set(depth);
                depth == 0
            })
        } else {
            // Moved to another thread and dropped there. The permit is still
            // this server's to return, and the owning thread's count was
            // decided when it handed the server over.
            true
        };

        if outermost {
            let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *held = held.saturating_sub(1);
            drop(held);
            RELEASED.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half of this module that no type can carry: a test that calls
    /// `mockito::Server::new` itself takes no permit, so it serves traffic
    /// beside a test that did take one and re-admits exactly the resets this
    /// module exists to remove. Nothing would go red -- it would just flake
    /// again, somewhere else, next week.
    ///
    /// The needle is SPLIT ACROSS `concat!` ARGUMENTS so it cannot match its
    /// own declaration, and the positive control below is what keeps that
    /// honest. `http_agent.rs` has shipped a dead guard of exactly this shape
    /// before; see its note.
    #[test]
    fn only_this_module_builds_a_mock_server() {
        const NEEDLE: &str = concat!("mockito::Server:", ":new");

        /// Every `.rs` file under `src/`, as (path relative to `src/`,
        /// contents). Walked off disk rather than pulled from a hand-written
        /// list, for the reason `http_agent`'s twin gives: the defect guarded
        /// against is a *future* module standing up its own server, and a
        /// hand-written list is a list that module would not be on.
        fn crate_source_files() -> Vec<(String, String)> {
            fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
                for entry in std::fs::read_dir(dir).expect("src/ is readable").flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(root, &path, out);
                    } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                        let rel = path
                            .strip_prefix(root)
                            .expect("walked from root")
                            .to_string_lossy()
                            .replace('\\', "/");
                        out.push((rel, std::fs::read_to_string(&path).expect("source is UTF-8")));
                    }
                }
            }
            let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
            let mut files = Vec::new();
            walk(root, root, &mut files);
            files
        }

        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());
        let this_file = files
            .iter()
            .find(|(path, _)| path == "test_http.rs")
            .expect("the walk did not reach test_http.rs");
        assert!(
            this_file.1.contains(NEEDLE),
            "needle {NEEDLE:?} no longer matches the one place that is supposed to use it"
        );

        // `main.rs` is exempt, and the exemption is a limit rather than a
        // preference: it is a *different crate* that reaches this one through
        // `deskwarden::`, so a `#[cfg(test)]` module of the lib is not visible
        // to it at all. Its tests also run in their own process, where they
        // cannot contend with the lib's -- which is why the exemption costs
        // this suite nothing. Nothing else belongs on this list.
        let offenders: Vec<&String> = files
            .iter()
            .filter(|(path, text)| {
                path != "test_http.rs" && path != "main.rs" && text.contains(NEEDLE)
            })
            .map(|(path, _)| path)
            .collect();

        assert!(
            offenders.is_empty(),
            "these files build a mock server without taking the gate in `test_http`: \
             {offenders:?}. A server built outside the gate serves traffic beside one that \
             took a permit, which is the `os error 10054` reset this module exists to remove; \
             call `test_http::server()` instead"
        );
    }

    /// The gate really is a gate: a second server cannot be held while the
    /// first is, from another thread.
    ///
    /// Asserted by observing the peak count rather than by timing, so a loaded
    /// machine cannot flake it.
    #[test]
    fn only_one_server_is_held_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let (live, peak) = (Arc::clone(&live), Arc::clone(&peak));
            handles.push(std::thread::spawn(move || {
                for _ in 0..10 {
                    let _server = server();
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    live.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }
        for handle in handles {
            handle.join().expect("no thread panicked");
        }

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "two tests held a mock server at once: the gate is not holding, and the resets \
             it exists to remove come back with it"
        );
    }

    /// A second server taken while the first is still held must not wait on a
    /// permit this thread already owns. Without the re-entrancy counter this
    /// deadlocks, and the suite hangs rather than fails -- which is how the
    /// three-server test in `rest::api` would have found it.
    #[test]
    fn a_second_server_on_one_thread_does_not_wait_for_the_first() {
        let first = server();
        let second = server();
        assert_ne!(
            first.socket_address().port(),
            second.socket_address().port(),
            "two servers held at once must be two servers"
        );
    }
}

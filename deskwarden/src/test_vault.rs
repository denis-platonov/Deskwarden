//! Seeding a [`VaultCache`] for a test **without a backend**.
//!
//! Test-only at the declaration in `lib.rs`, exactly like [`crate::below_cut`],
//! so nothing here can ship and nothing in production can branch on it. It is
//! not a `cfg(test)` seam: production code does not call these and does not
//! change shape because they exist. They are ordinary callers of an ordinary
//! public API -- [`VaultCache::populate_with_vault`] -- which the encrypted
//! disk cache will call for the same reason a fixture does: it holds the whole
//! vault already and has nothing to ask a server for.
//!
//! # Why this module exists
//!
//! Until [`VaultCache::populate_with_vault`], seeding a cache with items
//! required a live HTTP round-trip: `populate_with` fetched the folders
//! itself. So every fixture across the crate that wanted a *populated* cache
//! stood up a `mockito` server for one `GET /list/object/folders` that
//! answered `[]`, and paid for it twice over:
//!
//!  * **mockito 1.7 pools its servers.** `ServerGuard::drop` does not shut the
//!    server down; it resets the mocks and returns the server -- still bound
//!    to its port -- to a process-global pool for the next test. Two tests
//!    that "own" the same port at overlapping moments produce connection
//!    resets and timeouts in each other, which is what the intermittent
//!    `os error 10054` / `10060` failures across `app`, `vault_cache` and
//!    `updater` on one port actually were.
//!  * **It made "this code path does not touch the network" untestable.**
//!    Dropping the guard before the code under test ran was the trick several
//!    fixtures used to assert exactly that; pooling silently took it away,
//!    because the port stays live and a recycled server answers.
//!
//! [`unreachable_bridge`] restores that property and makes it stronger than
//! the trick ever was: the address is dead for the life of the process, not
//! merely dead-if-nobody-recycled-it, so a code path that reaches for the
//! network fails loudly instead of quietly succeeding.

use crate::vault_bridge::{Folder, VaultBridge, VaultItem};
use crate::vault_cache::{PopulateOutcome, VaultCache, VaultSnapshot};

/// The loopback address these fixtures point a bridge at: the discard port,
/// which no service on this machine listens on and which nothing in this test
/// suite ever binds.
///
/// Port 9 is reserved for `discard` (RFC 863) and is not implemented on
/// Windows, so a connection to it is refused immediately rather than hanging
/// -- a failed request here costs microseconds, not a timeout. It is spelled
/// with an explicit `127.0.0.1` rather than `localhost` so that no DNS lookup
/// and no IPv6 fallback can turn a refusal into a delay.
pub const UNREACHABLE_URL: &str = "http://127.0.0.1:9";

/// A [`VaultBridge`] whose every request fails, permanently.
///
/// Hand this to a cache seeded by [`cache_with`] and the resulting fixture
/// carries a real assertion for free: any code under test that reaches for the
/// backend instead of the in-memory snapshot fails visibly.
#[must_use]
pub fn unreachable_bridge() -> VaultBridge {
    VaultBridge::new(UNREACHABLE_URL)
}

/// A populated [`VaultCache`] holding exactly `items` and `folders`, backed by
/// an [`unreachable_bridge`].
///
/// The epoch is captured before the write-back, from a cache that has done
/// nothing since it was built, so the era guard sees the era it wrote in and
/// the populate lands -- which the assertion below checks rather than assumes,
/// because a fixture that silently seeded nothing is how a test passes for the
/// wrong reason.
///
/// # Panics
///
/// If the populate did not land. It cannot fail in a fixture -- nothing else
/// holds this cache -- so a panic here means the fixture itself is broken.
#[must_use]
pub fn cache_with(items: Vec<VaultItem>, folders: Vec<Folder>) -> VaultCache {
    let cache = VaultCache::new(unreachable_bridge());
    let epoch = cache.epoch();
    let outcome = cache.populate_with_vault(VaultSnapshot { items, folders }, epoch);
    assert_eq!(
        outcome,
        PopulateOutcome::Populated,
        "the fixture cache did not actually get populated"
    );
    cache
}

/// [`cache_with`] for the common case of a vault with no folders.
#[must_use]
pub fn cache_with_items(items: Vec<VaultItem>) -> VaultCache {
    cache_with(items, Vec::new())
}

/// A populated [`VaultCache`] whose bridge points at `url` -- for a test that
/// genuinely needs one live HTTP route (a one-time-code fetch, say) but has no
/// reason to make the *seeding* a round-trip too.
///
/// The seeding still makes no request; only what the code under test does
/// afterwards can.
#[must_use]
pub fn cache_at(url: impl Into<String>, items: Vec<VaultItem>, folders: Vec<Folder>) -> VaultCache {
    let cache = VaultCache::new(VaultBridge::new(url));
    let epoch = cache.epoch();
    let outcome = cache.populate_with_vault(VaultSnapshot { items, folders }, epoch);
    assert_eq!(
        outcome,
        PopulateOutcome::Populated,
        "the fixture cache did not actually get populated"
    );
    cache
}

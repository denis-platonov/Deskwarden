//! **What changed in the vault the daemon holds, as a feed any client can
//! follow.**
//!
//! The daemon owns the one decrypted vault (see
//! `docs/superpowers/specs/2026-09-26-daemon-owns-the-vault-design.md`), and
//! the notifications hub runs there and nowhere else. This is how what the hub
//! said -- once the daemon has applied it to its own copy -- reaches a client
//! that holds a copy of its own: a sequence-numbered log, read with a
//! long-poll.
//!
//! # Client-agnostic, and transport-free
//!
//! Nothing here knows about HTTP, a window or a thread. [`ChangeFeed::publish`]
//! appends; [`ChangeFeed::wait_after`] answers a page. The HTTP route that
//! serves a page is `crate::vault_api`'s, and a future transport -- a browser
//! extension's native-messaging host -- would serve the same page the same
//! way.
//!
//! # The vocabulary is the hub's own
//!
//! A [`Change::Vault`] carries a [`crate::rest::notifications::Announcement`]
//! exactly as the daemon's listener read it, and the daemon publishes it only
//! AFTER applying it to its own cache. So a client answers it with the rule it
//! would have used on the push itself -- one record for an announced cipher,
//! a whole reload for what cannot be targeted -- and the record it reads back
//! from the daemon is already the new one. A second vocabulary would be a
//! second place for "which pushes are targetable" to be decided.
//!
//! [`Change::Sends`] is the other kind: a Send was created, changed, opened or
//! revoked. It is a list refetch and never a vault read -- see
//! `HubListener::sends_changed`.
//!
//! # A client that fell behind is told so
//!
//! The log keeps the last [`RETAINED`] changes. A client asking for changes
//! after a sequence number older than that -- or after one this feed never
//! issued, which is what a restarted daemon looks like -- gets a page with
//! `reset` set, and answers it with a whole reload. The feed's
//! [`FeedPage::instance`] is drawn at random when it is made, so a client can
//! also tell a restarted daemon from a quiet one.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::rest::notifications::Announcement;

/// How many changes the log keeps. A client further behind than this reloads
/// the whole vault, which is the answer it would have needed anyway.
pub const RETAINED: usize = 256;

/// The longest one long-poll is held open before it answers "nothing yet".
///
/// Short enough that a client learns the daemon's hub came up or went down
/// within one period (a page carries [`FeedPage::live`]), and long enough that
/// an idle window costs a request every twenty seconds and not more.
pub const LONG_POLL: Duration = Duration::from_secs(20);

/// One thing that changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The vault changed, as the hub announced it. Already applied to the
    /// daemon's copy by the time it is published.
    Vault(Announcement),
    /// A Send changed. A client drops its Sends list and asks again.
    Sends,
}

/// One answer to [`ChangeFeed::wait_after`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedPage {
    /// Drawn at random when the feed was made. A different value than a
    /// client last saw means a different daemon -- a restart -- and is
    /// answered as a [`Self::reset`].
    pub instance: u64,
    /// The sequence number of the last change this page covers. A client
    /// passes it back as its next `after`.
    pub seq: u64,
    /// Whether the daemon's notifications hub is connected right now. A client
    /// polls the vault only while this is false, exactly as it would have
    /// while its own hub was down.
    pub live: bool,
    /// The client is too far behind for the log to say what it missed, and
    /// must reload the whole vault and the Sends list.
    pub reset: bool,
    pub changes: Vec<Change>,
}

struct Log {
    seq: u64,
    changes: VecDeque<(u64, Change)>,
    live: bool,
}

/// The log, and the condition a long-poll waits on.
pub struct ChangeFeed {
    instance: u64,
    log: Mutex<Log>,
    arrived: Condvar,
}

impl ChangeFeed {
    /// An empty feed with a fresh [`FeedPage::instance`].
    #[must_use]
    pub fn new() -> Self {
        let mut bytes = [0u8; 8];
        // A failed draw costs restart detection, not safety: the instance is
        // not a credential. Zero is still a value a client compares.
        let _ = getrandom::getrandom(&mut bytes);
        Self::with_instance(u64::from_le_bytes(bytes))
    }

    /// An empty feed with a chosen instance -- for a test that has to name it.
    #[must_use]
    pub fn with_instance(instance: u64) -> Self {
        Self {
            instance,
            log: Mutex::new(Log { seq: 0, changes: VecDeque::new(), live: false }),
            arrived: Condvar::new(),
        }
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Appends `changes`, in order, and wakes every waiting client.
    pub fn publish(&self, changes: impl IntoIterator<Item = Change>) {
        let mut log = self.log();
        let before = log.seq;
        for change in changes {
            log.seq += 1;
            let seq = log.seq;
            log.changes.push_back((seq, change));
        }
        while log.changes.len() > RETAINED {
            log.changes.pop_front();
        }
        if log.seq != before {
            self.arrived.notify_all();
        }
    }

    /// Records whether the daemon's hub is connected. Wakes waiting clients
    /// only on a change, so a client learns of it now rather than at the end
    /// of its poll.
    pub fn set_live(&self, live: bool) {
        let mut log = self.log();
        if log.live != live {
            log.live = live;
            self.arrived.notify_all();
        }
    }

    /// Whether the daemon's hub is connected, as last recorded.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.log().live
    }

    /// The changes after `after`, waiting up to `wait` for the first one.
    ///
    /// `None` for `after` is a client's first question -- "where are you?" --
    /// and is answered at once, with no changes: what the client already
    /// holds is its own initial load, and nothing before this moment is news.
    ///
    /// A page is answered as soon as there is anything to say: a change, a
    /// `reset`, or a change in [`FeedPage::live`] since the client last saw
    /// it (`live_seen`). Otherwise it waits, and answers an empty page when
    /// `wait` runs out.
    pub fn wait_after(&self, after: Option<u64>, live_seen: bool, wait: Duration) -> FeedPage {
        let until = Instant::now() + wait;
        let mut log = self.log();
        loop {
            let Some(after) = after else {
                return self.page(&log, log.seq, false, Vec::new());
            };
            // Ahead of the log: this feed never issued that number, so the
            // client was following a different daemon.
            if after > log.seq {
                return self.page(&log, log.seq, true, Vec::new());
            }
            // Behind the log: what it missed has been trimmed.
            let oldest = log.changes.front().map_or(log.seq + 1, |(seq, _)| *seq);
            if after + 1 < oldest && after < log.seq {
                return self.page(&log, log.seq, true, Vec::new());
            }
            if after < log.seq || log.live != live_seen {
                let changes = log
                    .changes
                    .iter()
                    .filter(|(seq, _)| *seq > after)
                    .map(|(_, change)| change.clone())
                    .collect();
                return self.page(&log, log.seq, false, changes);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return self.page(&log, log.seq, false, Vec::new());
            }
            log = self
                .arrived
                .wait_timeout(log, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn page(&self, log: &Log, seq: u64, reset: bool, changes: Vec<Change>) -> FeedPage {
        FeedPage { instance: self.instance, seq, live: log.live, reset, changes }
    }
}

impl Default for ChangeFeed {
    fn default() -> Self {
        Self::new()
    }
}

// ---- the wire ------------------------------------------------------------
//
// One JSON shape, written and read here and nowhere else, so the daemon's
// answer and a client's reading of it cannot drift apart. The `kind` names
// are the hub's own concepts, not a window's.

#[derive(Serialize, Deserialize)]
struct WirePage {
    instance: u64,
    seq: u64,
    live: bool,
    reset: bool,
    changes: Vec<WireChange>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum WireChange {
    Item {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    ItemGone {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    VaultAt {
        date: String,
    },
    Whole,
    Sends,
}

impl From<&Change> for WireChange {
    fn from(change: &Change) -> Self {
        match change {
            Change::Vault(Announcement::Item { id, revision }) => {
                WireChange::Item { id: id.clone(), revision: revision.clone() }
            }
            Change::Vault(Announcement::ItemGone { id, revision }) => {
                WireChange::ItemGone { id: id.clone(), revision: revision.clone() }
            }
            Change::Vault(Announcement::VaultAt { date }) => WireChange::VaultAt { date: date.clone() },
            Change::Vault(Announcement::Whole) => WireChange::Whole,
            Change::Sends => WireChange::Sends,
        }
    }
}

impl From<WireChange> for Change {
    fn from(change: WireChange) -> Self {
        match change {
            WireChange::Item { id, revision } => Change::Vault(Announcement::Item { id, revision }),
            WireChange::ItemGone { id, revision } => {
                Change::Vault(Announcement::ItemGone { id, revision })
            }
            WireChange::VaultAt { date } => Change::Vault(Announcement::VaultAt { date }),
            WireChange::Whole => Change::Vault(Announcement::Whole),
            WireChange::Sends => Change::Sends,
        }
    }
}

/// A page as the JSON body the daemon answers `GET /changes` with.
#[must_use]
pub fn page_body(page: &FeedPage) -> String {
    let wire = WirePage {
        instance: page.instance,
        seq: page.seq,
        live: page.live,
        reset: page.reset,
        changes: page.changes.iter().map(WireChange::from).collect(),
    };
    serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_string())
}

/// A page read back out of [`page_body`]'s JSON.
///
/// # Errors
///
/// A body that is not a page -- which a client treats as a failed poll.
pub fn page_from_body(body: &str) -> Result<FeedPage, String> {
    let wire: WirePage =
        serde_json::from_str(body).map_err(|e| format!("not a change-feed page: {e}"))?;
    Ok(FeedPage {
        instance: wire.instance,
        seq: wire.seq,
        live: wire.live,
        reset: wire.reset,
        changes: wire.changes.into_iter().map(Change::from).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Change {
        Change::Vault(Announcement::Item { id: id.to_string(), revision: Some("r".to_string()) })
    }

    /// A client's first question is answered at once, with nothing: what it
    /// holds is its own load, and the history before it is not news.
    #[test]
    fn the_first_question_is_where_are_you_and_it_is_answered_at_once() {
        let feed = ChangeFeed::with_instance(7);
        feed.publish([item("a"), Change::Sends]);
        let page = feed.wait_after(None, false, Duration::from_secs(5));
        assert_eq!(page, FeedPage { instance: 7, seq: 2, live: false, reset: false, changes: vec![] });
    }

    /// Changes after the client's number, in order, and only those.
    #[test]
    fn a_client_gets_what_it_has_not_seen_and_nothing_it_has() {
        let feed = ChangeFeed::with_instance(1);
        feed.publish([item("a")]);
        feed.publish([item("b"), Change::Sends]);
        let page = feed.wait_after(Some(1), false, Duration::ZERO);
        assert_eq!(page.seq, 3);
        assert_eq!(page.changes, vec![item("b"), Change::Sends]);
        assert!(!page.reset);
    }

    /// A quiet feed answers an empty page once the wait runs out, carrying
    /// the same number back so the client loses nothing.
    #[test]
    fn a_quiet_feed_answers_empty_when_the_wait_runs_out() {
        let feed = ChangeFeed::with_instance(1);
        feed.publish([item("a")]);
        let page = feed.wait_after(Some(1), false, Duration::from_millis(30));
        assert_eq!((page.seq, page.changes.len(), page.reset), (1, 0, false));
    }

    /// **A number this feed never issued is a different daemon**, and a
    /// number older than the log is a client that missed too much: both are
    /// told to reload rather than handed a partial history.
    #[test]
    fn a_client_ahead_of_the_log_or_behind_it_is_told_to_reset() {
        let feed = ChangeFeed::with_instance(1);
        feed.publish([item("a")]);
        assert!(feed.wait_after(Some(9), false, Duration::ZERO).reset, "ahead of the log");

        let feed = ChangeFeed::with_instance(1);
        feed.publish((0..RETAINED + 10).map(|n| item(&n.to_string())));
        let page = feed.wait_after(Some(2), false, Duration::ZERO);
        assert!(page.reset, "behind the retained log");
        assert!(page.changes.is_empty(), "a reset carries no partial history");
        // Control: a client inside the retained window is not reset.
        let inside = feed.wait_after(Some(page.seq - 1), false, Duration::ZERO);
        assert!(!inside.reset);
        assert_eq!(inside.changes.len(), 1);
    }

    /// The hub going up or down is itself news, so a client stops -- or
    /// starts -- polling without waiting out a long-poll.
    #[test]
    fn the_hub_coming_up_is_answered_without_waiting() {
        let feed = ChangeFeed::with_instance(1);
        feed.set_live(true);
        let page = feed.wait_after(Some(0), false, Duration::from_secs(5));
        assert!(page.live);
        assert!(page.changes.is_empty());
        // And a client that already knows waits as usual.
        let quiet = feed.wait_after(Some(0), true, Duration::from_millis(20));
        assert!(quiet.live && quiet.changes.is_empty());
    }

    /// **The wire carries every kind of change and reads back as itself.**
    #[test]
    fn a_page_survives_the_wire() {
        let page = FeedPage {
            instance: u64::MAX,
            seq: 42,
            live: true,
            reset: false,
            changes: vec![
                item("a"),
                Change::Vault(Announcement::Item { id: "b".to_string(), revision: None }),
                Change::Vault(Announcement::ItemGone { id: "c".to_string(), revision: None }),
                Change::Vault(Announcement::VaultAt { date: "2026-09-26T00:00:00Z".to_string() }),
                Change::Vault(Announcement::Whole),
                Change::Sends,
            ],
        };
        assert_eq!(page_from_body(&page_body(&page)).expect("reads back"), page);
        assert!(page_from_body("{\"nope\":1}").is_err());
    }
}

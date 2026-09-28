//! **A client's line to the vault the daemon owns** -- where it is, the grant
//! that admits this client, and the backend, the sync and the change feed
//! built on them.
//!
//! See `docs/superpowers/specs/2026-09-26-daemon-owns-the-vault-design.md`.
//! The daemon holds the one decrypted vault and answers `crate::vault_api` on
//! loopback; the vault window is the first client of it.
//!
//! # How a window gets one
//!
//! The daemon writes ONE line of JSON down the child's stdin as it starts it
//! -- `{"url": "http://127.0.0.1:<port>", "grant": "<64 hex>"}` -- and closes
//! the pipe. Never on the command line, which every process on the machine
//! can read (`ui_process`'s `no_argument_of_a_ui_spawn_could_be_a_secret`
//! still holds: the argument vector is the same two words), and never in the
//! environment, which is readable too. A process nobody piped anything to
//! reads end-of-file and has no link, and behaves exactly as it did before
//! this existed.
//!
//! # Installed once, read wherever it is needed
//!
//! The window's backend is built from it directly; two things deeper down
//! read it from here -- the Sync the window's heartbeat and Sync pill run
//! (`vault_window`'s `DirectRestTasks::sync`), and the notifications listener
//! (`HubListener::for_this_process`), which follows the daemon's change feed
//! instead of dialling the hub itself.

use std::sync::OnceLock;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::app_match::AppMatch;
use crate::change_feed::{FeedPage, LONG_POLL};
use crate::vault_backend::VaultBackend;
use crate::vault_bridge::{Folder, GenerateRequest, NewItem, VaultBridge, VaultError, VaultItem};
use crate::vault_cache::VaultSnapshot;

/// The connect bound for every request to the daemon: a loopback handshake,
/// and a daemon that is not answering one in this long is not there.
const CONNECT: Duration = Duration::from_secs(3);

/// How long `POST /sync` may take: the daemon re-checking the server, which
/// is a revision question and, when the vault moved, a whole read.
const SYNC_DEADLINE: Duration = Duration::from_secs(120);

/// Where the daemon's vault is, and this client's grant to it.
///
/// **Not `Debug`**: the grant is a credential, and a link that could be
/// printed would be.
pub struct DaemonLink {
    base_url: String,
    grant: Zeroizing<String>,
    /// What this process does when the daemon stops honouring the grant --
    /// see [`Self::when_refused`]. Nothing, until something says otherwise.
    on_refused: fn(),
}

/// How many polls of the feed in a row may fail without a refusal before the
/// daemon is taken to be gone. With the listener's back-off that is about half
/// a minute in production: long enough for a daemon that is merely busy, and
/// a daemon on this machine that has not answered for that long is not
/// coming back on this port -- a restarted one listens on another.
pub const GONE_AFTER_FAILED_POLLS: u32 = 3;

fn ignore_refusal() {}

#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    url: String,
    grant: String,
}

impl DaemonLink {
    /// The line a daemon writes for a client it starts.
    #[must_use]
    pub fn line(base_url: &str, grant: &str) -> Zeroizing<String> {
        let line = Line { url: base_url.to_string(), grant: grant.to_string() };
        Zeroizing::new(serde_json::to_string(&line).unwrap_or_default() + "\n")
    }

    /// A link out of that line, or `None` for anything that is not one.
    ///
    /// **Loopback or nothing.** The line comes from the process that started
    /// this one, and it is still refused unless it names `127.0.0.1` over
    /// plain HTTP: a link that could name another host is a grant that could
    /// be sent to one.
    #[must_use]
    pub fn from_line(line: &str) -> Option<Self> {
        let Line { url, grant } = serde_json::from_str(line.trim()).ok()?;
        let grant = Zeroizing::new(grant);
        let port = url.strip_prefix("http://127.0.0.1:")?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err() {
            return None;
        }
        if grant.len() != 64 || !grant.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self { base_url: url, grant, on_refused: ignore_refusal })
    }

    /// **What to do when this link is dead** -- the daemon answered the grant
    /// `401`, or has not answered at all for [`GONE_AFTER_FAILED_POLLS`]
    /// polls in a row.
    ///
    /// Both mean the vault this window shows can no longer be read or
    /// written: the daemon locked it, signed out, switched account, or
    /// restarted (an update does), and a restarted daemon neither knows this
    /// grant nor listens on this port. The window must not go on showing a
    /// vault it cannot save, and it must not fall back to reading the server
    /// itself; the vault window's host supplies the one safe answer -- say so,
    /// and close. Noticed by the change-feed listener, which is always polling.
    #[must_use]
    pub fn when_refused(mut self, on_refused: fn()) -> Self {
        self.on_refused = on_refused;
        self
    }

    /// The link this process's parent wrote down its stdin, if it wrote one.
    ///
    /// One line and then end-of-file: the daemon closes the pipe as soon as it
    /// has written. A process started any other way has no stdin at all -- the
    /// binary is a windows-subsystem one -- and reads end-of-file at once.
    #[must_use]
    pub fn from_stdin() -> Option<Self> {
        let mut line = Zeroizing::new(String::new());
        std::io::stdin().read_line(&mut line).ok()?;
        Self::from_line(&line)
    }

    /// The daemon's base URL, for a log line. Not a secret: a port on this
    /// machine.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// This client's vault backend: the daemon, asked in `bw serve`'s own
    /// words.
    #[must_use]
    pub fn backend(&self) -> DaemonBackend {
        DaemonBackend {
            bridge: VaultBridge::with_bearer(self.base_url.clone(), &self.grant),
            directory_url: format!("{}/organizations", self.base_url),
            directory_agent: crate::http_agent::bounded_total_with_bearer(
                CONNECT,
                DIRECTORY_DEADLINE,
                self.grant.clone(),
            ),
        }
    }

    /// Asks the daemon to re-check the server -- the window's Sync, its
    /// heartbeat and its answer to a whole-vault announcement all end here.
    /// Behind the daemon's revision gate, so one row when nothing moved.
    ///
    /// # Errors
    ///
    /// The daemon refused or did not answer, as a sentence for the log and
    /// the Sync pill.
    pub fn sync(&self) -> Result<(), String> {
        let agent = crate::http_agent::bounded_total_with_bearer(CONNECT, SYNC_DEADLINE, self.grant.clone());
        agent
            .post(&format!("{}/sync", self.base_url))
            .call()
            .map(|_| ())
            .map_err(|e| format!("the daemon could not sync the vault: {e}"))
    }

    /// One long-poll of the daemon's change feed, as
    /// [`crate::rest::notifications::HubListener::following`] asks for it.
    #[must_use]
    pub fn feed_poll(&self) -> crate::rest::notifications::FollowPoll {
        // The agent outlives each poll and is bounded above the feed's own
        // long-poll, so a quiet feed answers before the request times out.
        let agent = crate::http_agent::bounded_total_with_bearer(
            CONNECT,
            LONG_POLL + Duration::from_secs(10),
            self.grant.clone(),
        );
        let url = format!("{}/changes", self.base_url);
        let on_refused = self.on_refused;
        let mut failed_in_a_row: u32 = 0;
        Box::new(move |after: Option<u64>, live_seen: bool| -> Result<FeedPage, String> {
            let mut request = agent.get(&url).query("live", if live_seen { "true" } else { "false" });
            if let Some(after) = after {
                request = request.query("after", &after.to_string());
            }
            let page = match request.call() {
                Ok(response) => response
                    .into_string()
                    .map_err(|e| e.to_string())
                    .and_then(|body| crate::change_feed::page_from_body(&body)),
                Err(ureq::Error::Status(401, _)) => {
                    log::warn!("the daemon no longer honours this window's grant");
                    on_refused();
                    return Err("the daemon refused this window's grant".to_string());
                }
                Err(e) => Err(e.to_string()),
            };
            match &page {
                Ok(_) => failed_in_a_row = 0,
                Err(_) => {
                    failed_in_a_row += 1;
                    if failed_in_a_row >= GONE_AFTER_FAILED_POLLS {
                        log::warn!(
                            "the daemon has not answered this window for {failed_in_a_row} polls \
                             in a row; taking it to be gone"
                        );
                        on_refused();
                    }
                }
            }
            page
        })
    }
}

static INSTALLED: OnceLock<DaemonLink> = OnceLock::new();

/// Makes `link` this process's line to the daemon. Once per process; a second
/// install is refused and says so.
pub fn install(link: DaemonLink) -> bool {
    INSTALLED.set(link).is_ok()
}

/// This process's line to the daemon, if it was handed one.
#[must_use]
pub fn installed() -> Option<&'static DaemonLink> {
    INSTALLED.get()
}

/// The window's Sync, through the daemon -- or `None` in a process with no
/// link, whose Sync is what it always was.
#[must_use]
pub fn sync_through_the_daemon() -> Option<Result<(), String>> {
    installed().map(DaemonLink::sync)
}

/// A change-feed poll for this process's listener, or `None` with no link.
#[must_use]
pub fn feed_poll() -> Option<crate::rest::notifications::FollowPoll> {
    installed().map(DaemonLink::feed_poll)
}

/// **The window's vault backend when the daemon owns the vault.**
///
/// A [`VaultBridge`] -- `bw serve`'s client, complete and tested -- pointed at
/// the daemon with this client's grant on every request. Every vault
/// operation goes to the daemon, which answers reads out of the vault it holds
/// and makes writes through its own cache; two are answered here instead, and
/// both say why.
pub struct DaemonBackend {
    bridge: VaultBridge,
    /// `GET /organizations` -- the one route here that is not `bw serve`'s,
    /// so the one the bridge does not know.
    directory_url: String,
    directory_agent: crate::http_agent::TotalBounded,
}

/// How long the sharing directory may take: a loopback answer out of memory.
const DIRECTORY_DEADLINE: Duration = Duration::from_secs(10);

impl DaemonBackend {
    /// **The daemon's sharing directory** -- organisations, collections and
    /// who is in them, as its own whole read built them.
    ///
    /// # Errors
    ///
    /// The daemon did not answer, or answered with something that is not a
    /// directory.
    pub fn directory(&self) -> Result<crate::rest::organizations::Directory, String> {
        let body: serde_json::Value = self
            .directory_agent
            .get(&self.directory_url)
            .call()
            .map_err(|e| e.to_string())?
            .into_json()
            .map_err(|e| e.to_string())?;
        crate::rest::organizations::Directory::from_body(
            body.get("data").cloned().unwrap_or(serde_json::Value::Null),
        )
    }
}

impl VaultBackend for DaemonBackend {
    fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_items()
    }
    fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        self.bridge.get_item(id)
    }
    fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        self.bridge.list_folders()
    }
    /// Two loopback requests, and no server round trip behind either.
    ///
    /// **And the sharing directory with them**, held for this process exactly
    /// as a backend that synced for itself holds the one its sync built -- so
    /// the organisations rail and every "shared with" answer describe the
    /// vault just loaded. A daemon that cannot say leaves the directory as it
    /// was; the vault itself has loaded.
    fn list_vault(&self) -> Result<VaultSnapshot, VaultError> {
        let items = self.bridge.list_items()?;
        let folders = self.bridge.list_folders()?;
        match self.directory() {
            Ok(directory) => crate::rest::organizations::remember(directory),
            Err(why) => log::warn!("the daemon's sharing directory could not be read ({why})"),
        }
        Ok(VaultSnapshot { items, folders })
    }
    fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.bridge.set_app_match(item, m)
    }
    fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        self.bridge.create_folder(name)
    }
    fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        self.bridge.update_folder(id, name)
    }
    fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)
    }
    fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        self.bridge.create_item(new_item)
    }
    fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        self.bridge.update_item(item)
    }
    fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        self.bridge.move_item_to_folder(item, folder_id)
    }
    /// **`true`, and it is the daemon's answer, not `bw serve`'s.** The route
    /// is `bw serve`'s, but what is behind it is the direct-REST backend,
    /// which replaces the whole cipher -- so the explicit `null` folder the
    /// bridge sends for a move out of every folder is honoured. See
    /// `VaultBackend::can_unfile_items`.
    fn can_unfile_items(&self) -> bool {
        true
    }
    fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)
    }
    fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_trash()
    }
    fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        self.bridge.list_archive()
    }
    fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.archive_item(id)
    }
    fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.unarchive_item(id)
    }
    fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.restore_item(id)
    }
    fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.purge_item(id)
    }
    fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        self.bridge.get_totp(id)
    }
    /// **Generated here, not at the daemon.** No server has an endpoint for
    /// it and it reads nothing from the vault, so a round trip would carry a
    /// fresh secret over a socket for no reason. The same generator and the
    /// same failure mapping as the direct-REST backend's --
    /// `rest::backend::generate_here`.
    fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        crate::rest::backend::generate_here(request)
    }
    /// No revision question to ask: the daemon owns the vault and asks the server on the window's behalf, through `POST /sync`.
    fn revision_now(&self) -> Option<i64> {
        None
    }
    fn revision_of_last_read(&self) -> Option<i64> {
        None
    }
    // A window's sharing goes through `SharingClient` to the daemon's engine; it has none of its own.
    fn sharing(&self) -> Option<std::sync::Arc<crate::rest::sharing::Sharing>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRANT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// **What the daemon writes is what a window reads**, and nothing else is.
    #[test]
    fn a_link_line_reads_back_and_nothing_else_does() {
        let line = DaemonLink::line("http://127.0.0.1:50123", GRANT);
        let link = DaemonLink::from_line(&line).expect("the daemon's own line");
        assert_eq!(link.base_url(), "http://127.0.0.1:50123");

        for refused in [
            // Not loopback: a grant that could be sent to another host.
            DaemonLink::line("http://10.0.0.5:50123", GRANT),
            DaemonLink::line("http://localhost:50123", GRANT),
            DaemonLink::line("https://127.0.0.1:50123", GRANT),
            DaemonLink::line("http://127.0.0.1:50123/path", GRANT),
            DaemonLink::line("http://127.0.0.1:99999", GRANT),
            // Not a grant this daemon mints.
            DaemonLink::line("http://127.0.0.1:50123", "short"),
            DaemonLink::line("http://127.0.0.1:50123", &"z".repeat(64)),
            Zeroizing::new(String::new()),
            Zeroizing::new("not json\n".to_string()),
        ] {
            assert!(DaemonLink::from_line(&refused).is_none(), "{:?} was accepted", refused.as_str());
        }
    }

    /// Links built by the refusal tests, and how many times each was refused.
    /// Thread-local-free on purpose: the feed follower calls the hook on its
    /// own thread, so these are statics -- one per test, so the two cannot
    /// read each other's count.
    static REVOKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static GONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn count_revoked() {
        REVOKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn count_gone() {
        GONE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    const FAST: crate::rest::notifications::Timing = crate::rest::notifications::Timing {
        connect: Duration::from_secs(3),
        poll: Duration::from_millis(20),
        ping_every: Duration::from_millis(150),
        silence: Duration::from_millis(600),
        no_source_wait: Duration::from_millis(20),
        retry_floor: Duration::from_millis(20),
        retry_ceiling: Duration::from_millis(100),
        settle: Duration::ZERO,
    };

    /// Thirty seconds, because a connection refused on Windows is not
    /// immediate -- the stack retries the SYN, so each failed poll against a
    /// closed port costs about a second -- and the full suite runs these
    /// beside five thousand others.
    fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < until {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never happened: {what}");
    }

    fn a_live_daemon() -> (crate::test_http::MockServer, crate::vault_host::VaultHost) {
        let (server, backend) = crate::rest::backend::tests::signed_in_with_no_sync_route();
        let api = std::sync::Arc::new(
            crate::vault_api::VaultApi::new(
                std::sync::Arc::new(crate::vault_cache::VaultCache::new(backend)),
                std::sync::Arc::new(crate::change_feed::ChangeFeed::new()),
            )
            .with_long_poll(Duration::from_millis(100)),
        );
        (server, crate::vault_host::VaultHost::start(api).expect("a loopback port"))
    }

    fn engineering() -> std::sync::Arc<crate::rest::organizations::Directory> {
        std::sync::Arc::new(
            crate::rest::organizations::Directory::from_body(serde_json::json!({
                "organisations": [{ "id": "org-1", "name": "Engineering", "enabled": true }],
                "collections": [{ "id": "c-1", "organizationId": "org-1", "name": "Shared" }],
                "rosters": [{ "organizationId": "org-1", "members": [
                    { "id": "u-1", "email": "owner@example.invalid", "name": null,
                      "role": "owner", "confirmed": true, "collections": [] }
                ] }]
            }))
            .expect("a directory"),
        )
    }

    /// **The organisations rail of a window reading the daemon's vault is
    /// the daemon's**: the directory its own whole read built, over the API,
    /// whole -- organisations, collections and who is in them.
    #[test]
    fn a_linked_window_is_handed_the_daemons_sharing_directory() {
        let (_server, backend) = crate::rest::backend::tests::signed_in_with_no_sync_route();
        let api = std::sync::Arc::new(
            crate::vault_api::VaultApi::new(
                std::sync::Arc::new(crate::vault_cache::VaultCache::new(backend)),
                std::sync::Arc::new(crate::change_feed::ChangeFeed::new()),
            )
            .with_directory(engineering),
        );
        let host = crate::vault_host::VaultHost::start(api).expect("a loopback port");
        let link = DaemonLink::from_line(&host.link_line_for("window")).expect("the daemon's line");
        let directory = link.backend().directory().expect("the daemon's directory");
        assert_eq!(&directory, engineering().as_ref());
        assert_eq!(directory.organisations()[0].label(), "Engineering");
        assert_eq!(directory.collections_of("org-1")[0].label(), "Shared");
        // And only for a client the daemon granted.
        host.api().revoke("window");
        assert!(link.backend().directory().is_err(), "an ungranted client read the directory");
    }

    /// **A grant the daemon stops honouring closes the window's link** -- a
    /// lock, a sign-out, an account switch -- and the window is told once the
    /// feed is refused, not left showing a vault it cannot save.
    #[test]
    fn a_refused_grant_is_reported_to_the_window() {
        let (_server, host) = a_live_daemon();
        let link = DaemonLink::from_line(&host.link_line_for("window"))
            .expect("the daemon's line")
            .when_refused(count_revoked);
        let _listener = crate::rest::notifications::HubListener::following(link.feed_poll(), FAST);
        // Several polls answered while the grant is good, and no refusal.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(REVOKED.load(std::sync::atomic::Ordering::SeqCst), 0, "refused while honoured");
        host.api().revoke("window");
        eventually("the refusal reaches the window", || {
            REVOKED.load(std::sync::atomic::Ordering::SeqCst) > 0
        });
    }

    /// **A daemon that is gone is reported too** -- a restart, which listens on
    /// another port and would answer this one with nothing at all.
    ///
    /// # What "gone" looks like, and why this is not a dropped `VaultHost`
    ///
    /// A daemon goes away by its process ending, and the OS then closes
    /// everything it had: the listening socket (a connect is refused) and every
    /// accepted connection (a pooled one is reset). That is what the window
    /// meets, and it is what this builds: a port that was bound and let go, so
    /// nothing listens on it.
    ///
    /// This test used to drop a live `VaultHost` in-process instead, and that
    /// is NOT the same event. `tiny_http` accepts connections on a thread of
    /// its own and keeps each accepted connection on a thread of its own; the
    /// host's workers stop, but a connection accepted in the instant before
    /// the listener closed -- here, the window's first poll, made
    /// microseconds after the drop -- is queued for workers that will never
    /// take it, and is neither answered nor closed. The poll then waited out
    /// its agent's whole thirty seconds, and the test's deadline with it: in
    /// every full run once the suite grew, never when it ran alone, because
    /// only under load did the poll land inside that window. Measured: the
    /// poll that failed had taken 30.01 s and ended in `os error 10060`.
    #[test]
    fn a_daemon_that_went_away_is_reported_to_the_window() {
        let port = {
            let bound = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            bound.local_addr().expect("its address").port()
        };
        let line = DaemonLink::line(&format!("http://127.0.0.1:{port}"), &"a".repeat(64));
        let link = DaemonLink::from_line(&line).expect("a line").when_refused(count_gone);
        let listener = crate::rest::notifications::HubListener::following(link.feed_poll(), FAST);
        eventually("the daemon's absence reaches the window", || {
            GONE.load(std::sync::atomic::Ordering::SeqCst) > 0
        });
        assert!(!listener.is_live(), "a window whose daemon is gone still calls itself live");
    }

    /// The daemon's backend can un-file an item, because what answers behind
    /// the `bw serve` route is the direct-REST backend.
    #[test]
    fn the_daemons_backend_can_unfile() {
        let link = DaemonLink::from_line(&DaemonLink::line("http://127.0.0.1:1", GRANT)).expect("link");
        assert!(link.backend().can_unfile_items());
    }
}

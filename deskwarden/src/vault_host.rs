//! **The daemon's door to the vault it owns**: `crate::vault_api` on a
//! loopback port, and the worker that keeps the vault current from the
//! notifications hub.
//!
//! See `docs/superpowers/specs/2026-09-26-daemon-owns-the-vault-design.md`.
//! Everything worth being wrong about is in `vault_api` (what may be asked,
//! and what is answered) and `change_feed` (what changed); this is the part
//! that cannot be a pure function -- a socket, a few threads -- kept thin so
//! it can be read in one go.
//!
//! # Loopback, an ephemeral port, and a grant on every request
//!
//! The listener binds `127.0.0.1:0` through `service_host::listen_addr`,
//! whose loopback literal is pinned there, so there is one definition of
//! where this app listens. The port is whatever Windows hands out, and a
//! client learns it only from the daemon that started it (`crate::daemon_link`).
//! A connection from anything else on this machine reaches `vault_api`, which
//! answers `401` with no body to a request that does not carry a grant.
//!
//! # A small pool, because a long-poll holds a worker
//!
//! [`WORKERS`] threads take requests off one listener. `GET /changes` holds
//! its worker for up to `change_feed::LONG_POLL`, so a single worker would
//! make every read wait behind a quiet feed.
//!
//! # The hub lives here now
//!
//! [`HubWorker`] owns the daemon's `HubListener`. Each settled batch is
//! applied to the daemon's cache with `notifications::targeted_changes` --
//! one record per announced cipher, a revision-gated reload for the rest --
//! and only THEN published to the feed, so a client that reads a record back
//! reads the new one. Send pushes are published as `Change::Sends` and touch
//! no vault at all.
//!
//! **The sharing pass rides the same thread** (spec 11.5): after an applied
//! batch, after the hub's own populate, for a new vault era, and on
//! [`SHARING_TIMER`] while someone is waiting -- `VaultApi::sharing_pass`
//! publishes `Change::Sharing` when the view moved. A thread of its own
//! would be one more for `job_object`'s census to count and one more lock
//! order to get right, for work that is at most a few requests a tick.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::change_feed::{Change, ChangeFeed};
use crate::rest::notifications::HubListener;
use crate::rest::sharing::Trigger;
use crate::vault_api::{ApiRequest, VaultApi};

/// How many requests the daemon answers at once. One long-poll per open
/// client plus room for its reads; two clients is the most this app has.
pub const WORKERS: usize = 4;

/// The largest request body read. A create or an edit is one item's JSON --
/// kilobytes -- and a body past this is refused unread rather than buffered.
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// The daemon's listener: its address, the API behind it, and the workers
/// answering it.
pub struct VaultHost {
    base_url: String,
    api: Arc<VaultApi>,
    server: Arc<tiny_http::Server>,
    stop: Arc<AtomicBool>,
}

impl VaultHost {
    /// Binds loopback and starts [`WORKERS`] workers answering `api`.
    ///
    /// # Errors
    ///
    /// The port could not be bound.
    pub fn start(api: Arc<VaultApi>) -> std::io::Result<Self> {
        let server = tiny_http::Server::http(crate::service_host::listen_addr(0))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| std::io::Error::other("the listener has no IP address"))?;
        let server = Arc::new(server);
        let stop = Arc::new(AtomicBool::new(false));
        for _ in 0..WORKERS {
            let server = Arc::clone(&server);
            let api = Arc::clone(&api);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("vault-host".to_string())
                .spawn(move || serve(&server, &api, &stop))?;
        }
        Ok(Self { base_url: format!("http://127.0.0.1:{port}"), api, server, stop })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn api(&self) -> &Arc<VaultApi> {
        &self.api
    }

    /// **A new client's line**: this listener's address and a fresh grant for
    /// `holder`, as the one line `crate::daemon_link` reads.
    #[must_use]
    pub fn link_line_for(&self, holder: &str) -> Zeroizing<String> {
        let grant = self.api.grant(holder);
        crate::daemon_link::DaemonLink::line(&self.base_url, grant.expose())
    }
}

/// **Stops the workers; it does not close what `tiny_http` already accepted.**
///
/// `tiny_http` owns the accepting thread and one thread per accepted
/// connection, and exposes no way to close either from here. The listener
/// closes once the last worker lets go of the server; a connection accepted
/// before that -- or a keep-alive one a client still holds -- stays open, and
/// a request on it is queued for workers that are gone, so it is neither
/// answered nor refused until the client's own timeout. The daemon never
/// drops its host (it is installed for the life of the process), and a daemon
/// that goes away does so by exiting, which closes every one of those sockets
/// at once -- so this matters only to a test, which must not use a dropped
/// host to stand for a daemon that has gone. See `daemon_link`'s
/// `a_daemon_that_went_away_is_reported_to_the_window`.
impl Drop for VaultHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for _ in 0..WORKERS {
            self.server.unblock();
        }
    }
}

/// One worker: take a request, answer it, until told to stop.
fn serve(server: &tiny_http::Server, api: &VaultApi, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        let mut request = match server.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            // An accept error is the listener's, not this request's; the
            // loop carries on for the next one.
            Err(_) => continue,
        };
        let method = request.method().as_str().to_string();
        let target = request.url().to_string();
        // Read once and never logged: this header IS the credential.
        let authorization = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Authorization"))
            .map(|header| Zeroizing::new(header.value.as_str().to_string()));
        let oversized = request.body_length().is_some_and(|length| length as u64 > MAX_BODY);
        // Sized to the declared length before the read: a `Vec` that grew
        // while reading would free each shorter copy of the body unwiped, and
        // `POST /sharing/invite-code`'s body is the master password. A body
        // with no length (chunked) still grows; no client of ours sends one.
        let declared = if oversized { 0 } else { request.body_length().unwrap_or(0) };
        let mut body = Zeroizing::new(Vec::with_capacity(declared));
        if !oversized {
            use std::io::Read as _;
            let _ = request.as_reader().take(MAX_BODY + 1).read_to_end(&mut body);
        }
        let reply = if oversized || body.len() as u64 > MAX_BODY {
            crate::vault_api::ApiResponse { status: 413, body: String::new() }
        } else {
            api.answer(&ApiRequest {
                method: &method,
                target: &target,
                authorization: authorization.as_deref().map(String::as_str),
                body: &body,
            })
        };
        log::debug!("vault host: {method} {} -> {}", target.split('?').next().unwrap_or(""), reply.status);
        let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
            .expect("a literal header that cannot fail to parse");
        let _ = request.respond(
            tiny_http::Response::from_string(reply.body)
                .with_status_code(reply.status)
                .with_header(header),
        );
    }
}

// ---- the published host -------------------------------------------------

static HOST: OnceLock<VaultHost> = OnceLock::new();

/// Makes `host` this daemon's listener, for the code that starts a window and
/// hands it a line. Once per process.
pub fn install_env(host: VaultHost) -> bool {
    HOST.set(host).is_ok()
}

/// A line for a client the daemon is starting, or `None` in a process with no
/// listener -- whose client then reads the vault the way it always did.
#[must_use]
pub fn link_line_for(holder: &str) -> Option<Zeroizing<String>> {
    HOST.get().map(|host| host.link_line_for(holder))
}

/// Takes `holder`'s grant away -- the daemon reaped or killed its process.
pub fn revoke(holder: &str) {
    if let Some(host) = HOST.get() {
        host.api().revoke(holder);
    }
}

// ---- the hub, in the daemon --------------------------------------------

/// How often the worker asks the listener whether a batch has settled. The
/// listener's own settle (two seconds in production) is the real grain; this
/// is only how promptly it is noticed.
const HUB_TICK: Duration = Duration::from_millis(250);

/// The daemon's notifications worker. Dropping it stops the thread within one
/// tick; the listener it owns closes its connection as it goes.
pub struct HubWorker {
    stop: Arc<AtomicBool>,
}

impl Drop for HubWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl HubWorker {
    /// Starts the worker over `listener`, applying to the vault `api` answers
    /// out of and publishing to its feed.
    #[must_use]
    pub fn start(listener: HubListener, api: Arc<VaultApi>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("vault-hub".to_string())
            .spawn(move || {
                let mut sharing = SharingTick { era: None, next: Instant::now() + SHARING_TIMER };
                while !thread_stop.load(Ordering::SeqCst) {
                    hub_tick(&listener, &api, Instant::now());
                    sharing_tick(&api, &mut sharing, Instant::now());
                    std::thread::sleep(HUB_TICK);
                }
            })
            .map_err(|e| log::warn!("the daemon's notifications worker could not start: {e}"))
            .ok();
        Self { stop }
    }
}

/// **One look at the hub**: what it says is applied to the daemon's vault
/// first, and published second.
///
/// * The hub's `live` is published every tick, so a client polls exactly
///   while pushes cannot reach it.
/// * A settled batch of Send pushes is one `Change::Sends`.
/// * A settled vault batch is applied -- one record per announced cipher when
///   `targeted_changes` says it can be, otherwise a revision-gated reload --
///   and then published as it arrived. The server's revision is read (one
///   row) BEFORE a one-record apply and noted with the API, so a later Sync
///   knows the copy stands at it -- see `VaultApi::sync`.
/// * **A vault that is not loaded takes nothing.** A lock empties the cache,
///   and a push arriving after it must not be the thing that fills it again:
///   the batch is dropped, and the next unlock loads a vault that has it.
/// * **The sharing pass runs LAST**, after the batch is published. It makes
///   round trips of its own (a session's first probes the server), and run
///   before the publish it held the batch back from every window for their
///   length. Its own `Sharing` change follows the batch on the feed. Pinned by
///   `the_vault_batch_is_published_before_the_sharing_pass_runs`.
fn hub_tick(listener: &HubListener, api: &VaultApi, now: Instant) {
    let cache = api.cache();
    let feed = api.feed();
    feed.set_live(listener.is_live());
    if listener.sends_changed(now) {
        feed.publish([Change::Sends]);
    }
    if !listener.settled(now) {
        return;
    }
    let announced = listener.take();
    if !cache.is_populated() {
        log::info!(
            "the notifications hub announced {} change(s) while the vault is not loaded; \
             nothing to apply them to",
            announced.len()
        );
        return;
    }
    let era = cache.epoch().era();
    let applied = match crate::rest::notifications::targeted_changes(listener.is_live(), &announced) {
        Some(changes) => {
            let before = cache.bridge().revision_now();
            match cache.apply_announced(&changes, era) {
                Ok(fetched) => {
                    log::info!(
                        "the daemon applied {} announced change(s) with {fetched} item read(s) and \
                         no vault sync",
                        changes.len()
                    );
                    api.note_applied(era, before);
                    Some(Trigger::Batch)
                }
                Err(why) => {
                    log::info!("announced changes need the whole vault after all: {why}");
                    None
                }
            }
        }
        None => None,
    };
    // Which pass the batch calls for, run once it is published: a one-record
    // apply is a `Batch`, a reload a `WholeRead`, a failed reload none.
    let pass = applied.or_else(|| match cache.populate() {
        Ok(_) => {
            api.forget_applied();
            Some(Trigger::WholeRead)
        }
        Err(e) => {
            log::warn!("the daemon could not re-read the vault after an announcement: {e:?}");
            None
        }
    });
    feed.publish(announced.into_iter().map(Change::Vault));
    if let Some(trigger) = pass {
        api.sharing_pass(trigger);
    }
}

/// **The sharing timer** (spec 11.5, trigger 4): how often the daemon asks
/// the server about people still waiting, while the vault is unlocked and
/// only while somebody is. NodeWarden pushes an accept to the sharer from
/// slice 1, so this is the backstop, not the path.
pub const SHARING_TIMER: Duration = Duration::from_secs(600);

/// The hub worker's memory for the sharing pass: which vault era it last
/// ran for (a new era is an unlock or a switch: a whole read), and when the
/// timer is next due.
struct SharingTick {
    era: Option<crate::vault_cache::VaultEra>,
    next: Instant,
}

/// Runs the pass for a new era and for the timer; called on the hub worker's
/// own thread, once per tick.
fn sharing_tick(api: &VaultApi, tick: &mut SharingTick, now: Instant) {
    let cache = api.cache();
    if !cache.is_populated() {
        tick.era = None;
        return;
    }
    let era = cache.epoch().era();
    if tick.era != Some(era) {
        tick.era = Some(era);
        tick.next = now + SHARING_TIMER;
        api.sharing_pass(Trigger::WholeRead);
        return;
    }
    if now >= tick.next {
        tick.next = now + SHARING_TIMER;
        if api.sharing_wants_timer() {
            api.sharing_pass(Trigger::Timer);
        }
    }
}

/// **Whether the daemon's vault changed under its match engine** since the
/// last time this was asked -- the daemon's loop asks once per pass.
///
/// The hub worker publishes one batch per settled burst, so however many
/// pushes a burst held, this answers `true` once for it: one rebuild. A pass
/// that finds several batches waiting rebuilds once for all of them.
pub struct VaultChangeWatch {
    seen: u64,
}

impl VaultChangeWatch {
    /// A watch that has seen everything published so far.
    #[must_use]
    pub fn new(feed: &ChangeFeed) -> Self {
        Self { seen: feed.vault_batches() }
    }

    /// `true` once for every run of vault batches published since the last
    /// `true`.
    pub fn due(&mut self, feed: &ChangeFeed) -> bool {
        let now = feed.vault_batches();
        let due = now != self.seen;
        self.seen = now;
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_feed::{ChangeFeed, LONG_POLL};
    use crate::daemon_link::DaemonLink;
    use crate::rest::backend::tests::{declare_the_per_id_cipher_route, fixture_cipher_for, signed_in_with_no_sync_route, sync_payload};
    use crate::rest::notifications::{tests::a_listener_the_test_can_push_to, Announcement, Timing};
    use crate::vault_cache::{PopulateOutcome, VaultCache};

    /// A timing for a listener that follows a feed in a test: no settle, and
    /// retries measured in milliseconds.
    const FAST: Timing = Timing {
        connect: Duration::from_secs(3),
        poll: Duration::from_millis(20),
        ping_every: Duration::from_millis(150),
        silence: Duration::from_millis(600),
        no_source_wait: Duration::from_millis(20),
        retry_floor: Duration::from_millis(20),
        retry_ceiling: Duration::from_millis(200),
        settle: Duration::ZERO,
    };

    fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never happened: {what}");
    }

    /// **The whole shape, on real sockets, counted on the server.**
    ///
    /// The daemon: the fixture account over the real `RestBackend`, the
    /// startup probe, the slot, the cache, the readiness wait and the seeding
    /// populate -- `main.rs`'s launch, in its order. Then the listener on a
    /// real loopback port, and a window: a `VaultCache` over the real
    /// `DaemonBackend`, doing what a window does on open -- its load, its
    /// sync-on-open (`POST /sync`), and the forced reload that follows.
    ///
    /// Then an edit from the window, and a push the daemon hears.
    ///
    /// The server's counts are the assertions:
    ///
    /// * **one** `GET /api/sync` for the whole launch, the daemon's;
    /// * **none** more for the window's edit, which is the one `PUT` it makes;
    /// * **none** more for a push reaching the window -- the daemon reads the
    ///   one record, and the window reads it back from the daemon.
    #[test]
    fn a_launch_with_a_window_reads_the_vault_once_and_an_edit_and_a_push_read_it_not_at_all() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect_at_least(0)
            .create();
        server
            .mock("GET", "/api/accounts/profile")
            .with_body(
                serde_json::json!({
                    "key": serde_json::from_str::<serde_json::Value>(&sync_payload())
                        .expect("json")["profile"]["key"],
                })
                .to_string(),
            )
            .expect(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        declare_the_per_id_cipher_route(&mut server);
        let put = server
            .mock("PUT", "/api/ciphers/live-1")
            .with_body_from_request(|_| fixture_cipher_for("/api/ciphers/live-1").into_bytes())
            .expect(1)
            .create();

        // ---- the daemon's launch ------------------------------------------
        backend.verify_stored_login().expect("the probe");
        let slot: crate::vault_backend::SharedLateBoundBackend =
            Arc::new(crate::vault_backend::LateBoundBackend::empty());
        slot.adopt(Box::new(backend));
        let daemon = Arc::new(VaultCache::new(Arc::clone(&slot)));
        let epoch = daemon.epoch();
        let items = crate::bw_serve::wait_for_vault_ready(daemon.bridge(), &[]).expect("the wait");
        assert_eq!(daemon.populate_with(items, epoch).expect("seeded"), PopulateOutcome::Populated);

        let feed = Arc::new(ChangeFeed::with_instance(3));
        let api = Arc::new(
            VaultApi::new(Arc::clone(&daemon), Arc::clone(&feed)).with_long_poll(Duration::from_millis(200)),
        );
        let host = VaultHost::start(Arc::clone(&api)).expect("a loopback port");
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.go_live(true);
        let _worker = HubWorker::start(hub, Arc::clone(&api));

        // ---- the window's open --------------------------------------------
        let link = DaemonLink::from_line(&host.link_line_for("window")).expect("the daemon's line");
        let window = VaultCache::new(link.backend());
        assert_eq!(window.populate().expect("the initial load"), PopulateOutcome::Populated);
        link.sync().expect("the sync-on-open, answered by the daemon");
        assert_eq!(window.populate().expect("the forced reload"), PopulateOutcome::Populated);
        assert_eq!(window.items().len(), 1, "the window shows the daemon's vault");
        assert_eq!(window.folders().len(), 1);

        // ---- an edit from the window --------------------------------------
        let mut edited = window.items().remove(0);
        edited.name = "Renamed in the window".to_string();
        let saved = window.update_item(&edited).expect("the edit, through the daemon");
        assert_eq!(saved.id, "live-1");
        put.assert();

        // ---- a push, heard by the daemon ----------------------------------
        let listener = HubListener::following(link.feed_poll(), FAST);
        eventually("the window follows the daemon's feed", || listener.is_live());
        pushes.hear_vault(Announcement::Item { id: "live-1".to_string(), revision: None });
        eventually("the push reaches the window", || listener.settled(Instant::now()));
        let announced = listener.take();
        assert_eq!(announced, [Announcement::Item { id: "live-1".to_string(), revision: None }]);
        let changes = crate::rest::notifications::targeted_changes(true, &announced)
            .expect("a cipher push is targetable");
        window
            .apply_announced(&changes, window.epoch().era())
            .expect("applied from the daemon");

        sync.assert();
        drop(host);
    }

    /// **A window that has to sign in first still costs one read, and it is
    /// the daemon's.**
    ///
    /// The first launch, or the one after a sign-out: the daemon has no key,
    /// so it starts the window with a grant and an empty vault. The window
    /// takes the master password, stores the key, puts the DAEMON's backend
    /// in its slot, and rings; the daemon adopts the stored key -- the profile
    /// check -- and makes the one whole read. `main.rs` does those steps in
    /// two processes and no test can run it, so they are made here in its
    /// order, and `main.rs`'s pins hold that it still takes them.
    #[test]
    fn a_sign_in_in_the_window_costs_one_read_and_it_is_the_daemons() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect_at_least(0)
            .create();
        server
            .mock("GET", "/api/accounts/profile")
            .with_body(
                serde_json::json!({
                    "key": serde_json::from_str::<serde_json::Value>(&sync_payload())
                        .expect("json")["profile"]["key"],
                })
                .to_string(),
            )
            .expect(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();

        // The daemon at launch: no key, so an empty slot and an empty vault.
        let daemon_slot: crate::vault_backend::SharedLateBoundBackend =
            Arc::new(crate::vault_backend::LateBoundBackend::empty());
        let daemon = Arc::new(VaultCache::new(Arc::clone(&daemon_slot)));
        let api = Arc::new(
            VaultApi::new(Arc::clone(&daemon), Arc::new(ChangeFeed::with_instance(4)))
                .with_long_poll(Duration::from_millis(100)),
        );
        let host = VaultHost::start(Arc::clone(&api)).expect("a loopback port");
        // The window it starts, with its line minted then.
        let link = DaemonLink::from_line(&host.link_line_for("window")).expect("the daemon's line");
        let window_slot: crate::vault_backend::SharedLateBoundBackend =
            Arc::new(crate::vault_backend::LateBoundBackend::empty());
        let window = VaultCache::new(Arc::clone(&window_slot));

        // The window's sign-in succeeds: its slot takes the daemon's backend.
        window_slot.adopt(Box::new(link.backend()));
        assert!(
            crate::bw_serve::wait_for_vault_ready(window.bridge(), &[]).is_err(),
            "the window read a vault before the daemon had loaded one"
        );

        // The ring: the daemon adopts the stored key and makes the one read.
        backend.verify_stored_login().expect("the key the window just stored");
        daemon_slot.adopt(Box::new(backend));
        let epoch = daemon.epoch();
        let items = daemon.bridge().list_items().expect("the daemon's read");
        assert_eq!(daemon.populate_with(items, epoch).expect("seeded"), PopulateOutcome::Populated);

        // The window's readiness wait now passes, and it loads from the daemon.
        crate::bw_serve::wait_for_vault_ready(window.bridge(), &[]).expect("ready");
        assert_eq!(window.populate().expect("the window's load"), PopulateOutcome::Populated);
        assert_eq!(window.items().len(), 1);

        sync.assert();
    }

    /// **A Send push reaches a client as a Send change, and the daemon's vault
    /// is not touched.**
    #[test]
    fn a_send_push_is_published_to_the_feed_and_reads_nothing() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        let api = an_api_over(VaultCache::new(backend));
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.hear_one();
        pushes.hear_one();
        hub_tick(&hub, &api, Instant::now());
        let page = api.feed().wait_after(Some(0), false, Duration::ZERO);
        assert_eq!(page.changes, vec![Change::Sends], "two Send pushes, one change");
        sync.assert();
    }

    /// **A push after a lock fills nothing.** The cache a lock emptied must be
    /// filled by the next unlock, not by whatever the hub says next -- and the
    /// batch is not published either, because nothing was applied.
    #[test]
    fn a_push_to_a_locked_daemon_is_dropped() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        let api = an_api_over(VaultCache::new(backend));
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.hear_vault(Announcement::Whole);
        hub_tick(&hub, &api, Instant::now());
        assert!(!api.cache().is_populated(), "a push re-filled a locked vault");
        assert!(api.feed().wait_after(Some(0), false, Duration::ZERO).changes.is_empty());
        sync.assert();
    }

    /// **A push that changes what an item matches is armed in the daemon's
    /// engine without a reload** -- and a burst is one rebuild.
    ///
    /// The engine matches applications, by the app-match field an item
    /// carries (URIs are the browser's, and autofill reads those straight
    /// from the cache the worker already updated). So the push here gives
    /// `live-1` an app match it did not have when the vault was read; the
    /// daemon's loop, asking `VaultChangeWatch` as it does on every pass,
    /// rebuilds from the snapshot, and the engine then answers for that app.
    /// One sync in all: the vault read. The pushes cost a record each.
    #[test]
    fn a_push_that_changes_an_app_match_is_armed_without_a_reload() {
        use crate::app_match::{AppMatch, TriggerMode, APP_MATCH_FIELD_NAME};
        use crate::match_engine::MatchEngine;
        use crate::window_watch::ForegroundEvent;

        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect_at_least(0)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        declare_the_per_id_cipher_route(&mut server);
        let matched = AppMatch::for_process("Speedtest.exe", TriggerMode::Hotkey).to_field_value();
        let record = server
            .mock("GET", "/api/ciphers/live-1")
            .with_body(crate::rest::backend::tests::fixture_cipher_with_field(
                "live-1",
                "A live item",
                APP_MATCH_FIELD_NAME,
                &matched,
            ))
            // One read per announcement, as `apply_announced` has always
            // made them: the burst below is two pushes of one item.
            .expect(2)
            .create();

        let api = an_api_over(VaultCache::new(backend));
        assert_eq!(api.cache().populate().expect("the vault read"), PopulateOutcome::Populated);
        let mut watch = VaultChangeWatch::new(api.feed());
        let mut engine = MatchEngine::new();
        engine.rebuild(&crate::app::match_entries(&api.cache().items()));
        let speedtest = ForegroundEvent {
            hwnd: 1,
            pid: 1,
            exe_name: "Speedtest.exe".to_string(),
            title: String::new(),
        };
        assert!(engine.lookup(&speedtest).is_none(), "control: nothing matched it yet");

        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.go_live(true);
        pushes.hear_vault(Announcement::Item { id: "live-1".to_string(), revision: None });
        pushes.hear_vault(Announcement::Item { id: "live-1".to_string(), revision: None });
        hub_tick(&hub, &api, Instant::now());

        // The daemon's loop, one pass.
        assert!(watch.due(api.feed()), "the loop was not told the vault changed");
        engine.rebuild(&crate::app::match_entries(&api.cache().items()));
        assert_eq!(
            engine.lookup(&speedtest).map(|(id, _)| id.to_string()),
            Some("live-1".to_string()),
            "the push's app match is not armed"
        );
        assert!(!watch.due(api.feed()), "one burst, but the loop was told twice");

        sync.assert();
        record.assert();
    }

    /// **A window hears the vault batch before the sharing pass runs.** The
    /// pass makes round trips of its own -- this one, the session's first,
    /// probes the server -- and a batch published after them reached the
    /// window only once they were done. Published first, the batch is not
    /// held up, and the pass's own `Sharing` change follows it.
    #[test]
    fn the_vault_batch_is_published_before_the_sharing_pass_runs() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(sync_payload()).expect(1).create();
        declare_the_per_id_cipher_route(&mut server);
        server
            .mock("GET", "/api/accounts/profile")
            .with_body(crate::rest::backend::tests::sharing_profile(serde_json::json!([])).to_string())
            .create();
        server
            .mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let api = an_api_over(VaultCache::new(backend));
        assert_eq!(api.cache().populate().expect("the vault read"), PopulateOutcome::Populated);
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.go_live(true);
        let pushed = Announcement::Item { id: "live-1".to_string(), revision: None };
        pushes.hear_vault(pushed.clone());
        hub_tick(&hub, &api, Instant::now());
        let changes = api.feed().wait_after(Some(0), false, Duration::ZERO).changes;
        assert_eq!(changes.first(), Some(&Change::Vault(pushed)), "{changes:?}");
        assert_eq!(changes.last(), Some(&Change::Sharing), "the pass's change comes after the batch: {changes:?}");
    }

    fn an_api_over(cache: VaultCache) -> VaultApi {
        VaultApi::new(Arc::new(cache), Arc::new(ChangeFeed::with_instance(1)))
    }

    /// The listener binds loopback and nothing else, through the one address
    /// this app listens on.
    #[test]
    fn the_host_listens_on_loopback_only() {
        let (_server, backend) = signed_in_with_no_sync_route();
        let api = Arc::new(VaultApi::new(Arc::new(VaultCache::new(backend)), Arc::new(ChangeFeed::new())));
        let host = VaultHost::start(api).expect("a loopback port");
        assert!(host.base_url().starts_with("http://127.0.0.1:"), "{}", host.base_url());
        // And a request without a grant is refused with nothing to read.
        let agent = crate::http_agent::bounded_total(Duration::from_secs(3), LONG_POLL);
        match agent.get(&format!("{}/list/object/items", host.base_url())).call() {
            Err(ureq::Error::Status(401, response)) => {
                assert_eq!(response.into_string().unwrap_or_default(), "");
            }
            other => panic!("an ungranted request was answered (ok = {})", other.is_ok()),
        }
    }

    /// **A window reads the sharing view from the daemon, over the real
    /// loopback listener**, and hears `Change::Sharing` on its feed.
    #[test]
    fn a_window_reads_the_sharing_view_and_hears_its_change_through_the_daemon() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(sync_payload()).expect_at_least(0).create();
        server.mock("GET", "/api/accounts/profile").with_body(crate::rest::backend::tests::sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        let daemon = Arc::new(VaultCache::new(backend));
        assert_eq!(daemon.populate().expect("the load"), PopulateOutcome::Populated);
        let api = Arc::new(VaultApi::new(Arc::clone(&daemon), Arc::new(ChangeFeed::with_instance(5))).with_long_poll(Duration::from_millis(200)));
        let host = VaultHost::start(Arc::clone(&api)).expect("a loopback port");
        let link = DaemonLink::from_line(&host.link_line_for("window")).expect("the line");
        let listener = crate::rest::notifications::HubListener::following(link.feed_poll(), FAST);
        // The listener's first poll is "where are you?", answered with the
        // feed's current number: a change published before it lands would be
        // history to it and never heard. The hub going live is news it hears
        // only once it follows, so waiting for that orders the two.
        api.feed().set_live(true);
        eventually("the window follows the feed", || listener.is_live());

        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        eventually("the window heard the sharing change", || listener.sharing_changed(Instant::now()));
        let view = link.sharing().view().expect("the view");
        assert!(view.available);
        drop(host);
    }

    /// **An accepted person is confirmed by the daemon with no window**
    /// (spec 11.5 triggers 1-2): a push reaches the hub worker, the worker's
    /// populate is a whole read, and the pass after it confirms -- all on the
    /// daemon's own thread, with no client connected.
    #[test]
    fn an_accepted_person_is_confirmed_by_the_daemon_with_no_window() {
        use crate::rest::backend::tests::{FIXTURE_ORG, FIXTURE_USER_ID, person_collection, roster_row, sharing_org_row, sharing_profile};
        let (mut server, backend) = signed_in_with_no_sync_route();
        // The org cached as made here, as on the PC that shares from it: a
        // whole read holds only an org made here or adopted by the user
        // (official-path spec §3.1, fix round 1), and the pass confirms only
        // in a held org.
        let dir = crate::test_scratch::ScratchDir::new("vault-host-confirm");
        let state = dir.path().join("sharing_state.json");
        crate::sharing_state::update(&state, |s| {
            let me = s.account_mut(FIXTURE_USER_ID);
            me.sharing_org_id = Some(FIXTURE_ORG.into());
            me.sharing_org_created = true;
        })
        .expect("seeded");
        backend
            .sharing_engine()
            .set_up(crate::rest::sharing::SharingSetup { state_path: Some(state), ..crate::rest::sharing::SharingSetup::none() });
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        let profile = sharing_profile(serde_json::json!([sharing_org_row()]));
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(serde_json::json!({
                "object": "sync", "profile": profile.clone(), "folders": [], "ciphers": [],
                "collections": [person_collection("c-anna", "anna@example.com")]
            }).to_string())
            .expect(1)
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] }).to_string())
            .expect(1)
            .create();
        server.mock("GET", "/api/accounts/profile").with_body(profile.to_string()).expect_at_least(0).create();
        let key = serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string();
        server.mock("GET", "/api/users/user-1/public-key").with_body(key.clone()).expect_at_least(0).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(key).expect(1).create();
        let confirm = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .with_body("")
            .expect(1)
            .create();

        let cache = VaultCache::new(backend);
        assert_eq!(cache.populate().expect("the daemon's load"), PopulateOutcome::Populated);
        let api = an_api_over(cache);
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.go_live(true);
        pushes.hear_vault(Announcement::Whole);
        // `now` far enough ahead that the burst reads as settled.
        hub_tick(&hub, &api, Instant::now() + Duration::from_secs(60));
        confirm.assert();
        let page = api.feed().wait_after(Some(0), false, Duration::ZERO);
        assert!(page.changes.contains(&Change::Sharing), "{:?}", page.changes);
    }

    /// **A plaintext collection name never confirms anyone** (review of Task
    /// 12, spec 11.5 rule 2): a hostile server lists a collection named
    /// `mallory@example.com` in plaintext -- a name it can write without the
    /// org key -- beside an accepted row whose key is its own. The whole read
    /// files the name, the card may show it, and the daemon's pass neither
    /// reads mallory's key nor confirms her.
    #[test]
    fn a_plaintext_collection_name_never_confirms_anyone() {
        use crate::rest::backend::tests::{FIXTURE_ORG, roster_row, sharing_org_row, sharing_profile};
        let (mut server, backend) = signed_in_with_no_sync_route();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        let profile = sharing_profile(serde_json::json!([sharing_org_row()]));
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(serde_json::json!({
                "object": "sync", "profile": profile.clone(), "folders": [], "ciphers": [],
                "collections": [{ "id": "c-mal", "organizationId": FIXTURE_ORG, "name": "mallory@example.com", "object": "collection" }]
            }).to_string())
            .expect(1)
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(serde_json::json!({ "data": [roster_row("mal-id", Some("mal-id"), "mallory@example.com", 1, &["c-mal"])] }).to_string())
            .expect_at_least(0)
            .create();
        server.mock("GET", "/api/accounts/profile").with_body(profile.to_string()).expect_at_least(0).create();
        let key = serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string();
        server.mock("GET", "/api/users/user-1/public-key").with_body(key.clone()).expect_at_least(0).create();
        let mallorys_key = server.mock("GET", "/api/users/mal-id/public-key").with_body(key).expect(0).create();
        let confirm = server.mock("POST", crate::test_http::Matcher::Any).expect(0).create();

        let cache = VaultCache::new(backend);
        assert_eq!(cache.populate().expect("the daemon's load"), PopulateOutcome::Populated);
        let api = an_api_over(cache);
        api.sharing_pass(Trigger::WholeRead);
        api.sharing_pass(Trigger::Timer);
        mallorys_key.assert();
        confirm.assert();
        assert!(!api.sharing_wants_timer(), "nobody the pass may confirm is waiting");
    }
}

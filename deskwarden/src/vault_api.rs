//! **The vault the daemon holds, as questions any client may ask** -- a
//! request in, a status and a JSON body out.
//!
//! `docs/superpowers/specs/2026-09-26-daemon-owns-the-vault-design.md` is the
//! design. In one line: the daemon owns the one decrypted vault, and every
//! other process that wants it -- the vault window today, a browser
//! extension's host one day -- asks here.
//!
//! # `bw serve`'s routes, answered out of the daemon's cache
//!
//! The routes and the envelope are `bw serve`'s own
//! (`{"success":true,"data":...}`, a list one level deeper), because
//! [`crate::vault_bridge::VaultBridge`] is already a complete, tested client
//! of exactly that, and a second protocol would be a second place for a write
//! to be wrong. Reads are answered from the daemon's
//! [`crate::vault_cache::VaultCache`] with no server round trip; writes call
//! that same cache's writes, which reach the server and then the snapshot --
//! so the daemon's copy is the single truth and a client's copy is updated
//! from the server's own answer, as it always was.
//!
//! Two routes `bw serve` never had: **`GET /changes`**, the long-poll feed of
//! [`crate::change_feed`], and **`POST /sync`** answered by the daemon
//! re-checking the server (behind the revision gate, so one row when nothing
//! moved) rather than by any client reading it.
//!
//! # Transport-free, and client-agnostic
//!
//! Nothing here binds, reads a socket or knows which client is asking. The
//! loopback HTTP listener is `crate::vault_host`'s; a native-messaging host
//! would build the same [`ApiRequest`] out of a stdio frame. And nothing here
//! is the window's: a client's whole standing is the grant it presents.
//!
//! # Grants
//!
//! Every request carries `Authorization: Bearer <grant>`, and the only grants
//! are the ones [`VaultApi::grant`] minted -- per client, per launch. The API
//! keys of the user-facing local service (`crate::service_keys`) are a
//! different door with a different switch, and are not accepted here.
//!
//! * Only a SHA-256 of each grant is kept, compared in constant time with
//!   `service_token`'s one comparison.
//! * **A grant is bound to the vault era it was minted in.** Lock, logout and
//!   account switch all `clear` the daemon's cache, which starts a new era, so
//!   every outstanding grant dies at that instant -- without any of those
//!   paths having to remember to revoke anything.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::change_feed::Change;
use crate::change_feed::{ChangeFeed, LONG_POLL};
use crate::rest::sharing::{Sharing, SharingError, SharingOutcome, Trigger};
use crate::service_token::{self, Token};
use crate::vault_bridge::{NewItem, RelayedItem, VaultError, VaultItem};
use crate::vault_cache::{PopulateOutcome, VaultCache, VaultEra, VaultUnavailable};

/// One request, as any transport hands it over.
#[derive(Debug, Clone, Copy)]
pub struct ApiRequest<'a> {
    pub method: &'a str,
    /// The path and the query string, as the request line carried them.
    pub target: &'a str,
    /// The `Authorization` header's value. Never logged.
    pub authorization: Option<&'a str>,
    pub body: &'a [u8],
}

/// One answer: a status and a JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

impl ApiResponse {
    fn ok(data: serde_json::Value) -> Self {
        Self { status: 200, body: serde_json::json!({ "success": true, "data": data }).to_string() }
    }

    fn list(data: serde_json::Value) -> Self {
        Self::ok(serde_json::json!({ "object": "list", "data": data }))
    }

    fn done() -> Self {
        Self { status: 200, body: "{\"success\":true}".to_string() }
    }

    fn refused(status: u16, message: &str) -> Self {
        Self {
            status,
            body: serde_json::json!({ "success": false, "message": message }).to_string(),
        }
    }

    /// **No body for a client with no grant.** It learns that it was refused,
    /// and nothing about what a grant would have got it.
    fn unauthenticated() -> Self {
        Self { status: 401, body: String::new() }
    }
}

/// What a daemon-side failure tells the client, as a status its own
/// [`crate::vault_bridge`] reads back into the same [`VaultError`] kind.
///
/// * a refusal stays a refusal (`400` -> [`VaultError::Http`]);
/// * a server that did not serve stays one (`503` -> `Unreachable`);
/// * a dead session is `401` -> [`VaultError::Unauthorized`], which is the
///   client's cue to hand the vault back to whoever signs in -- the daemon.
fn failed(error: &VaultError) -> ApiResponse {
    match error {
        VaultError::Unauthorized => ApiResponse::refused(401, "the vault's own session has ended"),
        VaultError::Unreachable(_) => ApiResponse::refused(503, "the vault server did not answer"),
        VaultError::Unsupported { why, .. } => ApiResponse::refused(501, why),
        VaultError::Http(_) => ApiResponse::refused(400, "the vault server refused the request"),
        VaultError::Parse(_) => ApiResponse::refused(502, "the vault server's answer was unreadable"),
    }
}

/// A sharing refusal as an answer. Spec 11.3: `Unsupported` -> 501 with why, a
/// refused step -> 400 with the sentence to show, the rest as `failed`.
// Its callers are the write routes, which arrive with the flows (Tasks 9-13).
#[allow(dead_code)]
fn sharing_failed(error: &SharingError) -> ApiResponse {
    match error {
        SharingError::Unavailable(reason) => ApiResponse::refused(501, reason),
        SharingError::Refused(sentence) => ApiResponse::refused(400, sentence),
        SharingError::Vault(e) => failed(e),
    }
}

/// The daemon's vault is not loaded yet, or its session was cleared under the
/// request. `503`, which a client's readiness wait retries through its own
/// spinner.
fn not_ready() -> ApiResponse {
    ApiResponse::refused(503, "the vault is not loaded yet")
}

fn to_value<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

struct Grant {
    holder: String,
    digest: String,
    era: VaultEra,
}

fn digest_of(token: &str) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(token.as_bytes()) {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The API over one daemon's vault.
pub struct VaultApi {
    cache: Arc<VaultCache>,
    feed: Arc<ChangeFeed>,
    grants: Mutex<Vec<Grant>>,
    long_poll: Duration,
    /// The server revision the copy stands at after the hub worker's last
    /// one-record-at-a-time apply, in the era it was applied in. See
    /// [`Self::sync`].
    applied: Mutex<Option<(VaultEra, i64)>>,
    /// Where the sharing directory comes from:
    /// `rest::organizations::current`, which the daemon's backend fills on
    /// every whole read. A seam only because that is a process-wide slot a
    /// test cannot hold still.
    directory: fn() -> Arc<crate::rest::organizations::Directory>,
}

impl VaultApi {
    #[must_use]
    pub fn new(cache: Arc<VaultCache>, feed: Arc<ChangeFeed>) -> Self {
        Self {
            cache,
            feed,
            grants: Mutex::new(Vec::new()),
            long_poll: LONG_POLL,
            applied: Mutex::new(None),
            directory: crate::rest::organizations::current,
        }
    }

    /// The same API reading its sharing directory from `directory`, for a
    /// test.
    #[must_use]
    pub fn with_directory(mut self, directory: fn() -> Arc<crate::rest::organizations::Directory>) -> Self {
        self.directory = directory;
        self
    }

    /// The vault this API answers out of -- for the daemon's hub worker, which
    /// applies announcements to it.
    #[must_use]
    pub fn cache(&self) -> &Arc<VaultCache> {
        &self.cache
    }

    /// The same API with a shorter long-poll, for a test that must not wait
    /// twenty seconds for a quiet feed.
    #[must_use]
    pub fn with_long_poll(mut self, long_poll: Duration) -> Self {
        self.long_poll = long_poll;
        self
    }

    /// The feed this API serves, for the daemon's hub worker to publish into.
    #[must_use]
    pub fn feed(&self) -> &Arc<ChangeFeed> {
        &self.feed
    }

    fn grants(&self) -> std::sync::MutexGuard<'_, Vec<Grant>> {
        self.grants.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// **A new grant for `holder`**, bound to the vault era it is minted in.
    ///
    /// `holder` names the client for [`Self::revoke`] and nothing else -- the
    /// window's process id, today. A holder that already had a grant loses it:
    /// one client, one credential.
    #[must_use]
    pub fn grant(&self, holder: &str) -> Token {
        self.grant_with(holder, service_token::os_random)
    }

    /// [`Self::grant`] with the randomness supplied, for a test.
    #[must_use]
    pub fn grant_with(&self, holder: &str, random: fn() -> [u8; 32]) -> Token {
        let token = service_token::mint(random);
        let era = self.cache.epoch().era();
        let mut grants = self.grants();
        grants.retain(|grant| grant.holder != holder);
        grants.push(Grant { holder: holder.to_string(), digest: digest_of(token.expose()), era });
        token
    }

    /// Takes `holder`'s grant away. Its next request is refused.
    pub fn revoke(&self, holder: &str) {
        self.grants().retain(|grant| grant.holder != holder);
    }

    /// Whether `authorization` carries a grant that is live in the current
    /// era. Every stored grant is compared, whatever the first one said, so
    /// the time this takes says nothing about which one matched.
    fn admits(&self, authorization: Option<&str>) -> bool {
        let Some(presented) = service_token::bearer_of(authorization) else {
            return false;
        };
        let presented = digest_of(presented);
        let era = self.cache.epoch().era();
        let mut admitted = false;
        for grant in self.grants().iter() {
            let same = service_token::hashes_match(&grant.digest, &presented);
            admitted |= same && grant.era == era;
        }
        admitted
    }

    /// **One request, answered.**
    pub fn answer(&self, request: &ApiRequest<'_>) -> ApiResponse {
        if !self.admits(request.authorization) {
            return ApiResponse::unauthenticated();
        }
        let (path, query) = request.target.split_once('?').unwrap_or((request.target, ""));
        let query = Query(query);
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        match (request.method, segments.as_slice()) {
            ("GET", ["status"]) => ApiResponse {
                status: 200,
                body: crate::service_body::status_body(!self.cache.is_populated()),
            },
            ("GET", ["list", "object", "items"]) => self.list_items(&query),
            ("GET", ["list", "object", "folders"]) => self.list_folders(),
            ("GET", ["object", "item", id]) => self.get_item(id),
            ("POST", ["object", "item"]) => self.create_item(request.body),
            ("PUT", ["object", "item", id]) => self.update_item(id, request.body),
            ("DELETE", ["object", "item", id]) => {
                let done = if query.is("permanent", "true") {
                    self.cache.purge_item(id)
                } else {
                    self.cache.delete_item(id)
                };
                done.map_or_else(|e| failed(&e), |()| ApiResponse::done())
            }
            ("POST", ["restore", "item", id]) => self.restore_item(id),
            ("POST", ["archive", "item", id]) => self.archive_item(id),
            ("GET", ["object", "totp", id]) => match self.cache.bridge().get_totp(id) {
                Ok(Some(code)) => ApiResponse::ok(serde_json::json!({ "object": "string", "data": code })),
                // `bw serve`'s own answer for an item with no seed, and the
                // one `VaultBridge::get_totp` reads as "no code".
                Ok(None) => ApiResponse::refused(400, "this item has no one-time code"),
                Err(e) => failed(&e),
            },
            ("POST", ["object", "folder"]) => match folder_name(request.body) {
                Some(name) => self
                    .cache
                    .create_folder(&name)
                    .map_or_else(|e| failed(&e), |folder| ApiResponse::ok(to_value(&folder))),
                None => ApiResponse::refused(400, "a folder needs a name"),
            },
            ("PUT", ["object", "folder", id]) => match folder_name(request.body) {
                Some(name) => self
                    .cache
                    .update_folder(id, &name)
                    .map_or_else(|e| failed(&e), |folder| ApiResponse::ok(to_value(&folder))),
                None => ApiResponse::refused(400, "a folder needs a name"),
            },
            ("DELETE", ["object", "folder", id]) => {
                self.cache.delete_folder(id).map_or_else(|e| failed(&e), |()| ApiResponse::done())
            }
            ("POST", ["sync"]) => self.sync(),
            // **The sharing directory the daemon's own whole read built**:
            // organisations, collection names, who is in them. A client that
            // reads the vault here makes no sync of its own, so this is the
            // only way it can draw the same organisations rail -- and answer
            // "who else can see this item" -- as one that does.
            ("GET", ["organizations"]) => ApiResponse::ok((self.directory)().to_body()),
            ("GET", ["changes"]) => {
                let after = query.get("after").and_then(|after| after.parse().ok());
                let page = self.feed.wait_after(after, query.is("live", "true"), self.long_poll);
                ApiResponse { status: 200, body: crate::change_feed::page_body(&page) }
            }
            ("GET", ["sharing"]) => self.sharing_view(),
            (_, ["status" | "sync" | "changes" | "organizations"])
            | (_, ["list", "object", "items" | "folders"])
            | (_, ["object", "item" | "folder", ..])
            | (_, ["restore" | "archive", "item", _])
            | (_, ["object", "totp", _])
            | (_, ["sharing", ..]) => ApiResponse::refused(405, "that method is not answered here"),
            _ => ApiResponse::refused(404, "no such route"),
        }
    }

    /// **A client asking the daemon to make sure its vault is current** --
    /// the Sync pill, a window's sync-on-open, its polling timer.
    ///
    /// The server IS asked, every time, and it is asked the cheap question
    /// first: the account's revision, one row. If it names the revision the
    /// daemon's copy stands at, the copy is current and nothing more is read.
    /// Only a revision that moved is followed by the read that picks the
    /// change up. Never a blind whole read.
    ///
    /// # Which revision the copy stands at
    ///
    /// The last whole read's, as the backend filed it -- unless the hub worker
    /// has since applied announced changes one record at a time, which moved
    /// the server's revision without a whole read. Then it is the revision the
    /// worker read BEFORE applying them ([`Self::note_applied`]): never newer
    /// than the copy, so a change landing in between is seen as a move and
    /// read, rather than filed as already held. A whole read made here
    /// forgets that note, because the backend's own is newer.
    fn sync(&self) -> ApiResponse {
        let synced =
            ApiResponse::ok(serde_json::json!({ "object": "message", "title": "Syncing complete." }));
        let era = self.cache.epoch().era();
        if self.cache.is_populated() {
            if let Some(now) = self.cache.bridge().revision_now() {
                let held = self
                    .applied_in(era)
                    .or_else(|| self.cache.bridge().revision_of_last_read());
                if held == Some(now) {
                    return synced;
                }
            }
        }
        match self.cache.populate() {
            Ok(PopulateOutcome::Populated) => {
                self.forget_applied();
                self.sharing_pass(Trigger::WholeRead);
                synced
            }
            Ok(PopulateOutcome::DiscardedStale) => not_ready(),
            Err(e) => failed(&e),
        }
    }

    fn applied(&self) -> std::sync::MutexGuard<'_, Option<(VaultEra, i64)>> {
        self.applied.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn applied_in(&self, era: VaultEra) -> Option<i64> {
        self.applied().filter(|(at, _)| *at == era).map(|(_, revision)| revision)
    }

    /// **The hub worker applied announced changes to the copy**, having read
    /// the server's revision as `revision` just before it did. See
    /// [`Self::sync`] for what this is read by, and why "before".
    pub fn note_applied(&self, era: VaultEra, revision: Option<i64>) {
        *self.applied() = revision.map(|revision| (era, revision));
    }

    /// A whole read superseded any note: the backend's own revision is the
    /// copy's now.
    pub fn forget_applied(&self) {
        *self.applied() = None;
    }

    // ---- personal sharing ---------------------------------------------------

    /// The engine, or the response that says why there is none: a vault not
    /// loaded yet is 503, a backend without one (`bw serve`) is 501.
    // Its callers are the write routes, which arrive with the flows (Tasks 9-13).
    #[allow(dead_code)]
    fn engine(&self) -> Result<Arc<Sharing>, ApiResponse> {
        if !self.cache.is_populated() {
            return Err(not_ready());
        }
        self.cache
            .bridge()
            .sharing()
            .ok_or_else(|| ApiResponse::refused(501, "Sharing needs direct sync with a self-hosted server."))
    }

    /// `GET /sharing` -- answered from the engine's held view and the daemon's
    /// items, with no server round trip (spec 11.3). Never an error: "not
    /// loaded" and "not here" are views too.
    fn sharing_view(&self) -> ApiResponse {
        if !self.cache.is_populated() {
            return ApiResponse::ok(to_value(&crate::rest::sharing::SharingView::unavailable(
                crate::rest::sharing::REASON_NOT_LOADED,
            )));
        }
        match self.cache.bridge().sharing() {
            Some(engine) => ApiResponse::ok(to_value(&engine.view(&self.cache.items()))),
            None => ApiResponse::ok(to_value(&crate::rest::sharing::SharingView::unavailable(
                crate::rest::sharing::REASON_BW_SERVE,
            ))),
        }
    }

    /// **What a sharing write changed, into the daemon's copy and onto the
    /// feed** -- removals first, then the new copies, in one batch, then
    /// `sharing`. A window reads each announced item back from this daemon,
    /// which already holds it; nothing reaches the server.
    fn publish_outcome(&self, outcome: &SharingOutcome) {
        self.cache.apply_written_elsewhere(&outcome.upserted, &outcome.removed);
        let mut changes: Vec<Change> = outcome
            .removed
            .iter()
            .map(|id| Change::Vault(crate::rest::notifications::Announcement::ItemGone { id: id.clone(), revision: None }))
            .collect();
        changes.extend(outcome.upserted.iter().map(|item| {
            Change::Vault(crate::rest::notifications::Announcement::Item {
                id: item.id.clone(),
                revision: item.other.get("revisionDate").and_then(serde_json::Value::as_str).map(str::to_string),
            })
        }));
        changes.push(Change::Sharing);
        self.feed.publish(changes);
    }

    /// **One sharing pass** -- the triggers of spec 11.5 call this: a whole
    /// read, an applied hub batch, the timer. It is the daemon's own work; no
    /// window needs to be open. A failure is logged and the next trigger tries
    /// again.
    pub fn sharing_pass(&self, trigger: Trigger) {
        if !self.cache.is_populated() {
            return;
        }
        let Some(engine) = self.cache.bridge().sharing() else {
            return;
        };
        match engine.refresh(trigger) {
            Ok(pass) => {
                if !pass.outcome.is_empty() {
                    self.publish_outcome(&pass.outcome);
                } else if pass.changed {
                    self.feed.publish([Change::Sharing]);
                }
            }
            Err(e) => log::info!("the sharing pass ({trigger:?}) did not complete: {e}"),
        }
    }

    /// Whether the ten-minute timer has anything to do.
    #[must_use]
    pub fn sharing_wants_timer(&self) -> bool {
        self.cache.is_populated() && self.cache.bridge().sharing().is_some_and(|engine| engine.wants_timer())
    }

    fn list_items(&self, query: &Query<'_>) -> ApiResponse {
        let era = self.cache.epoch().era();
        // The two lists the cache deliberately does not hold -- see
        // `VaultCache::list_trash_unless_superseded` -- are passed through to
        // the daemon's backend, era-guarded as the window always had them.
        let aside = if query.is("trash", "true") {
            Some(self.cache.list_trash_unless_superseded(era))
        } else if query.is("archived", "true") {
            Some(self.cache.list_archive_unless_superseded(era))
        } else {
            None
        };
        match aside {
            Some(Ok(Some(items))) => ApiResponse::list(to_value(&items)),
            Some(Ok(None)) => not_ready(),
            Some(Err(e)) => failed(&e),
            None => match self.cache.snapshot_unless_superseded(era) {
                Ok(snapshot) => ApiResponse::list(to_value(&snapshot.items)),
                Err(VaultUnavailable::Unpopulated | VaultUnavailable::Superseded) => not_ready(),
            },
        }
    }

    fn list_folders(&self) -> ApiResponse {
        match self.cache.snapshot_unless_superseded(self.cache.epoch().era()) {
            Ok(snapshot) => ApiResponse::list(to_value(&snapshot.folders)),
            Err(_) => not_ready(),
        }
    }

    /// The daemon's copy when it holds one; otherwise the server's, by id --
    /// a trashed or archived item is not in the live snapshot and is still an
    /// item its id may be asked about.
    fn item(&self, id: &str) -> Result<VaultItem, VaultError> {
        match self.cache.get_by_id(id) {
            Some(item) => Ok(item),
            None => self.cache.bridge().get_item(id),
        }
    }

    fn get_item(&self, id: &str) -> ApiResponse {
        self.item(id).map_or_else(|e| failed(&e), |item| ApiResponse::ok(to_value(&item)))
    }

    fn create_item(&self, body: &[u8]) -> ApiResponse {
        let payload = match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(payload) => payload,
            Err(_) => return ApiResponse::refused(400, "a create body must be JSON"),
        };
        match RelayedItem::from_payload(payload) {
            Ok(relayed) => self
                .cache
                .create_item(&NewItem::Relayed(relayed))
                .map_or_else(|e| failed(&e), |item| ApiResponse::ok(to_value(&item))),
            Err(why) => ApiResponse::refused(400, why),
        }
    }

    /// An edit: the item as the client means it to be. Every write a client
    /// makes to an existing item -- an edit, a favourite, a move, an app
    /// match -- arrives as this, as it did at `bw serve`.
    fn update_item(&self, id: &str, body: &[u8]) -> ApiResponse {
        let item: VaultItem = match serde_json::from_slice(body) {
            Ok(item) => item,
            Err(_) => return ApiResponse::refused(400, "an edit body must be an item"),
        };
        if item.id != id {
            return ApiResponse::refused(400, "an edit body must be the item its path names");
        }
        self.cache
            .update_item(&item)
            .map_or_else(|e| failed(&e), |saved| ApiResponse::ok(to_value(&saved)))
    }

    /// **`bw serve` answers two operations on this one route**, and so does
    /// this: it un-archives an archived item and un-trashes a trashed one. The
    /// item is read by id -- one record -- to tell which, because the cache
    /// holds neither list.
    fn restore_item(&self, id: &str) -> ApiResponse {
        let item = match self.cache.bridge().get_item(id) {
            Ok(item) => item,
            Err(e) => return failed(&e),
        };
        let archived = item.other.get("archivedDate").is_some_and(|date| !date.is_null());
        let done = if archived {
            self.cache.unarchive_item(&item)
        } else {
            self.cache.restore_item(&item)
        };
        done.map_or_else(|e| failed(&e), |_| ApiResponse::done())
    }

    fn archive_item(&self, id: &str) -> ApiResponse {
        match self.item(id) {
            Ok(item) => {
                self.cache.archive_item(&item).map_or_else(|e| failed(&e), |()| ApiResponse::done())
            }
            Err(e) => failed(&e),
        }
    }
}

fn folder_name(body: &[u8]) -> Option<String> {
    let body: serde_json::Value = serde_json::from_slice(body).ok()?;
    body.get("name")?.as_str().map(str::to_string)
}

/// A query string, read as `key=value` pairs. The keys this API reads are
/// ASCII words and the values ASCII numbers and booleans, so nothing is
/// percent-decoded: a value that needed it is not one of ours.
struct Query<'a>(&'a str);

impl Query<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    fn is(&self, key: &str, value: &str) -> bool {
        self.get(key) == Some(value)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rest::backend::tests::{declare_the_per_id_cipher_route, signed_in_with_no_sync_route, sync_payload};
    use crate::test_http::MockServer;

    fn fixed() -> [u8; 32] {
        [9u8; 32]
    }

    /// A daemon's vault over the fixture account, populated by ONE counted
    /// sync, and an API over it with one grant.
    pub(crate) fn a_daemon() -> (MockServer, Arc<VaultCache>, VaultApi, String) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect_at_least(0)
            .create();
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect_at_least(0)
            .create();
        declare_the_per_id_cipher_route(&mut server);
        let cache = Arc::new(VaultCache::new(backend));
        assert_eq!(cache.populate().expect("the daemon's own load"), PopulateOutcome::Populated);
        let api = VaultApi::new(Arc::clone(&cache), Arc::new(ChangeFeed::with_instance(1)))
            .with_long_poll(Duration::from_millis(20));
        let token = api.grant_with("window", fixed).expose().to_string();
        (server, cache, api, token)
    }

    fn ask(api: &VaultApi, method: &str, target: &str, token: Option<&str>, body: &str) -> ApiResponse {
        let header = token.map(|token| format!("Bearer {token}"));
        api.answer(&ApiRequest {
            method,
            target,
            authorization: header.as_deref(),
            body: body.as_bytes(),
        })
    }

    /// **No grant, no answer** -- and no body saying what a grant would have
    /// got. A wrong grant, a missing one and an API key's `Bearer` shape are
    /// the same refusal.
    #[test]
    fn a_request_without_a_live_grant_is_refused_with_nothing() {
        let (_server, _cache, api, token) = a_daemon();
        for presented in [None, Some("not-the-grant"), Some("")] {
            let answer = ask(&api, "GET", "/list/object/items", presented, "");
            assert_eq!(answer, ApiResponse { status: 401, body: String::new() }, "{presented:?}");
        }
        // Control: the grant itself is admitted.
        assert_eq!(ask(&api, "GET", "/list/object/items", Some(&token), "").status, 200);
        // A revoked grant is refused like any other.
        api.revoke("window");
        assert_eq!(ask(&api, "GET", "/list/object/items", Some(&token), "").status, 401);
    }

    /// **A lock ends every grant at once**, with no revoke anywhere: the
    /// cache's `clear` starts a new era, and a grant is bound to its own.
    #[test]
    fn a_cleared_vault_ends_every_grant_minted_before_it() {
        let (_server, cache, api, token) = a_daemon();
        assert_eq!(ask(&api, "GET", "/list/object/folders", Some(&token), "").status, 200);
        cache.clear();
        assert_eq!(ask(&api, "GET", "/list/object/folders", Some(&token), "").status, 401);
        // A grant minted in the new era works, and answers "not loaded yet"
        // rather than an empty vault.
        let fresh = api.grant_with("window", fixed).expose().to_string();
        assert_eq!(ask(&api, "GET", "/list/object/items", Some(&fresh), "").status, 503);
    }

    /// **The routes are `bw serve`'s, answered from the daemon's copy**, in
    /// the envelope `VaultBridge` reads -- asserted by reading them back
    /// through it in `crate::vault_host`'s tests; here, the shapes.
    #[test]
    fn reads_are_answered_from_the_daemons_copy_in_bw_serves_envelope() {
        let (_server, _cache, api, token) = a_daemon();
        let items: serde_json::Value =
            serde_json::from_str(&ask(&api, "GET", "/list/object/items", Some(&token), "").body)
                .expect("json");
        assert_eq!(items["success"], true);
        assert_eq!(items["data"]["data"][0]["id"], "live-1");
        let item: serde_json::Value =
            serde_json::from_str(&ask(&api, "GET", "/object/item/live-1", Some(&token), "").body)
                .expect("json");
        assert_eq!(item["data"]["name"], "A live item");
        let folders: serde_json::Value =
            serde_json::from_str(&ask(&api, "GET", "/list/object/folders", Some(&token), "").body)
                .expect("json");
        assert_eq!(folders["data"]["data"][0]["name"], "Work");
    }

    /// **A Sync with nothing changed on the server: one revision question,
    /// and no whole read** -- whether or not the daemon's hub is live.
    #[test]
    fn a_sync_with_nothing_changed_asks_one_revision_and_reads_nothing() {
        let (mut server, _cache, api, token) = a_daemon();
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        let revision = server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000000")
            .expect(1)
            .create();
        api.feed().set_live(true);
        assert_eq!(ask(&api, "POST", "/sync", Some(&token), "").status, 200);
        revision.assert();
        sync.assert();
    }

    /// **A revision that moved is followed by the read**, and only then.
    #[test]
    fn a_sync_after_the_server_moved_reads_the_vault_once() {
        let (mut server, cache, api, token) = a_daemon();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000999")
            .expect_at_least(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(sync_payload())
            .expect(1)
            .create();
        assert_eq!(ask(&api, "POST", "/sync", Some(&token), "").status, 200);
        sync.assert();
        assert_eq!(cache.items().len(), 1, "the read was applied");
    }

    /// **Changes the hub applied one record at a time are the copy's**: a Sync
    /// after them asks the revision and finds it held, where the backend's own
    /// last read would have called it moved.
    #[test]
    fn a_sync_after_the_hub_applied_changes_reads_nothing() {
        let (mut server, cache, api, token) = a_daemon();
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        server
            .mock("GET", "/api/accounts/revision-date")
            .with_body("1700000000555")
            .expect_at_least(1)
            .create();
        api.note_applied(cache.epoch().era(), Some(1_700_000_000_555));
        assert_eq!(ask(&api, "POST", "/sync", Some(&token), "").status, 200);
        sync.assert();
        // A note from another era is not the copy's.
        cache.clear();
        assert_eq!(api.applied_in(cache.epoch().era()), None);
    }

    /// Nothing but the routes above, and a wrong method on one of them is a
    /// `405` rather than a `404`.
    #[test]
    fn an_unknown_route_is_404_and_a_wrong_method_is_405() {
        let (_server, _cache, api, token) = a_daemon();
        assert_eq!(ask(&api, "GET", "/object/send", Some(&token), "").status, 404);
        assert_eq!(ask(&api, "PATCH", "/object/item/live-1", Some(&token), "").status, 405);
        assert_eq!(ask(&api, "GET", "/sync", Some(&token), "").status, 405);
    }

    /// **A relayed create is checked against `NewItem`'s rule**: every
    /// payload this app's own constructors build is accepted and relayed
    /// byte for byte, and one with two type objects -- or none -- is not.
    #[test]
    fn a_relayed_create_is_this_apps_own_payload_or_nothing() {
        for built in [
            NewItem::login("L", "u", "p", Some("f1".to_string())),
            NewItem::secure_note("N", "body", None),
            NewItem::card("C", crate::vault_bridge::CardData::default(), None),
            NewItem::identity("I", crate::vault_bridge::IdentityData::default(), None),
            NewItem::ssh_key("S", "private", "public", "fingerprint", None),
        ] {
            let payload = built.to_payload();
            let relayed = RelayedItem::from_payload(payload.clone())
                .unwrap_or_else(|why| panic!("{built:?} was refused: {why}"));
            let relayed = NewItem::Relayed(relayed);
            assert_eq!(relayed.to_payload(), payload, "{built:?} was not relayed verbatim");
            assert_eq!(relayed.name(), built.name());
        }
        let two = serde_json::json!({
            "name": "x", "type": 1, "folderId": null, "login": {}, "card": {}
        });
        assert!(RelayedItem::from_payload(two).is_err(), "two type objects");
        let none = serde_json::json!({ "name": "x", "type": 1, "folderId": null });
        assert!(RelayedItem::from_payload(none).is_err(), "no type object");
        let mismatched = serde_json::json!({ "name": "x", "type": 3, "login": {} });
        assert!(RelayedItem::from_payload(mismatched).is_err(), "a type object of another type");
        assert!(RelayedItem::from_payload(serde_json::json!([1])).is_err(), "not an object");
    }

    /// And the relayed payload's secrets do not reach a formatter.
    #[test]
    fn a_relayed_create_prints_no_secret() {
        let relayed = RelayedItem::from_payload(
            NewItem::login("L", "u", "hunter2-secret", None).to_payload(),
        )
        .expect("a login");
        let printed = format!("{:?}", NewItem::Relayed(relayed));
        assert!(!printed.contains("hunter2-secret"), "{printed}");
        assert!(printed.contains("NewItem::Relayed"), "{printed}");
    }

    /// **`GET /sharing` answers from the daemon's held view**, in the
    /// envelope, and a first pass is what turns it on.
    #[test]
    fn the_sharing_view_is_answered_from_the_daemon_and_a_pass_publishes_it() {
        let (mut server, cache, api, token) = a_daemon();
        cache.bridge().sharing().expect("a REST daemon has an engine").set_up(crate::rest::sharing::SharingSetup::none());
        let before = ask(&api, "GET", "/sharing", Some(&token), "");
        assert_eq!(before.status, 200);
        let body: serde_json::Value = serde_json::from_str(&before.body).expect("json");
        assert_eq!(body["data"]["available"], false);
        assert_eq!(body["data"]["reason"], crate::rest::sharing::REASON_NOT_LOADED);

        server.mock("GET", "/api/accounts/profile").with_body(crate::rest::backend::tests::sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        let page = api.feed().wait_after(Some(0), false, Duration::ZERO);
        assert!(page.changes.contains(&crate::change_feed::Change::Sharing), "{:?}", page.changes);
        let after: serde_json::Value = serde_json::from_str(&ask(&api, "GET", "/sharing", Some(&token), "").body).expect("json");
        assert_eq!(after["data"]["available"], true);
        assert_eq!(after["data"]["dialect"], "nodewarden");
    }

    /// **`bw serve` answers "unavailable" and makes no request** (spec 7.4).
    #[test]
    fn bw_serve_answers_sharing_unavailable_and_makes_no_request() {
        let mut server = crate::test_http::server();
        let nothing = server.mock("GET", crate::test_http::Matcher::Any).expect(0).create();
        // Seeded with no request: the `bw serve` bridge's first request must
        // be one the sharing code made, and there must be none.
        let cache = Arc::new(crate::test_vault::cache_at(server.url(), Vec::new(), Vec::new()));
        let api = VaultApi::new(Arc::clone(&cache), Arc::new(ChangeFeed::with_instance(1)));
        let token = api.grant_with("window", fixed).expose().to_string();
        let answer: serde_json::Value = serde_json::from_str(&ask(&api, "GET", "/sharing", Some(&token), "").body).expect("json");
        assert_eq!(answer["data"]["available"], false);
        assert_eq!(answer["data"]["reason"], crate::rest::sharing::REASON_BW_SERVE);
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        nothing.assert();
    }

    /// A wrong method on a sharing route is 405, not 404.
    #[test]
    fn a_wrong_method_on_a_sharing_route_is_refused_as_a_method() {
        let (_server, _cache, api, token) = a_daemon();
        assert_eq!(ask(&api, "DELETE", "/sharing", Some(&token), "").status, 405);
    }
}

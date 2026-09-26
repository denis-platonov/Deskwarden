//! **The server telling this app that the vault changed**, rather than this
//! app asking.
//!
//! Bitwarden's own clients hold a websocket open to the server's
//! notifications hub, `/notifications/hub`, and the server pushes a message
//! down it whenever an item, a folder or the account changes -- which is why
//! an item deleted in the browser extension vanishes from the desktop app a
//! second later. The owner, of this app: "removed second website in [the
//! Bitwarden] app and it doesn't get updated automatically?" -- and then "do
//! the right way". This is that.
//!
//! # Only on the built-in client
//!
//! The hub authenticates the upgrade request with the account's **server**
//! access token. On the `bw serve` backend that token belongs to `bw`, which
//! never hands it over, so there is nothing to connect with; that backend
//! keeps the window's polling heartbeat and nothing else. On the direct-REST
//! backend the token is this process's own -- [`crate::rest::backend::RestBackend`]
//! holds the session -- and [`publish`] is how a backend makes it reachable
//! from here without this module holding a vault key or a second session.
//!
//! # The protocol, and how little of it is used
//!
//! The hub speaks ASP.NET SignalR. This client asks for the **JSON** hub
//! protocol, which every server this app targets answers -- ASP.NET registers
//! it by default, and NodeWarden (the owner's server) implements it by name --
//! and reads exactly four things out of it: the handshake answer, the
//! server's keep-alive ping, a close, and a `ReceiveMessage` invocation's
//! `Type`. That type is Bitwarden's `PushType`; [`notice_for`] lists which
//! ones mean "the vault on screen may be stale".
//!
//! **A message never carries the change itself** that this app would apply:
//! a notice is only ever a reason to run the window's ordinary sync, through
//! the same path the Sync pill takes. Nothing a server sends here is
//! decrypted, trusted as vault content, or written anywhere.
//!
//! A server that answers the handshake and then talks MessagePack -- binary
//! frames this client cannot read -- is [`Ended::Unsupported`], logged once,
//! and left alone: a push channel that cannot be read would otherwise stand
//! the window's polling down while delivering nothing.
//!
//! # The bearer goes in a header, never in the URL
//!
//! Browsers cannot set headers on a websocket upgrade, so Bitwarden's web
//! clients put the token in `?access_token=`. A native client can, and does
//! here: a URL is what proxies, server logs and error trackers keep. NodeWarden
//! refuses a token in the URL outright for exactly that reason, and accepts
//! the header.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::http::{HeaderValue, Uri, header::AUTHORIZATION};
use tungstenite::{Message, handshake::HandshakeError};
use zeroize::Zeroizing;

use crate::rest::api::{Authenticated, RestClient};

/// Where the hub lives, under the server root.
const HUB_PATH: &str = "/notifications/hub";

/// SignalR's record separator: every JSON message ends with one, and one
/// websocket frame may carry several.
const RECORD_SEPARATOR: char = '\u{1e}';

/// The first thing a SignalR client sends: which hub protocol it speaks.
const HANDSHAKE: &str = "{\"protocol\":\"json\",\"version\":1}\u{1e}";

/// SignalR's keep-alive, `type` 6. Sent on [`Timing::ping_every`] so that a
/// server which only ever ANSWERS pings -- NodeWarden's hub hibernates and
/// sends nothing unprompted -- still gives this client something to hear.
const PING: &str = "{\"type\":6}\u{1e}";

// ---- what a message means --------------------------------------------------

/// What one hub message is, as far as this client cares.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HubMessage {
    /// The answer to [`HANDSHAKE`]: `{}`, or `{"error": ..}` on a refusal.
    HandshakeAck { error: Option<String> },
    /// A `ReceiveMessage` whose type means the vault on screen may be stale,
    /// with that type -- for the log line, which is how "did the server say
    /// anything?" is answered without a debugger.
    VaultChanged(i64),
    /// Anything else the server may say: a keep-alive, an invocation of a
    /// type this app has no use for (a Send, a login-with-device request),
    /// a completion.
    Ignored,
    /// `type` 7: the server is ending the connection.
    Close,
    /// Not SignalR JSON at all.
    Unreadable,
}

/// The messages in one websocket frame's text, in order.
fn read_messages(text: &str) -> Vec<HubMessage> {
    text.split(RECORD_SEPARATOR)
        .filter(|frame| !frame.trim().is_empty())
        .map(read_one)
        .collect()
}

fn read_one(frame: &str) -> HubMessage {
    let Ok(serde_json::Value::Object(message)) = serde_json::from_str(frame) else {
        return HubMessage::Unreadable;
    };
    let Some(kind) = message.get("type") else {
        // The handshake answer is the one message with no `type`.
        let error = message.get("error").map(|e| e.as_str().unwrap_or("refused").to_string());
        return HubMessage::HandshakeAck { error };
    };
    match kind.as_i64() {
        Some(1) => {
            let target = message.get("target").and_then(serde_json::Value::as_str);
            let push_type = message
                .get("arguments")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("Type").or_else(|| a.get("type")))
                .and_then(serde_json::Value::as_i64);
            match (target, push_type) {
                (Some("ReceiveMessage"), Some(push_type)) if notice_for(push_type) => {
                    HubMessage::VaultChanged(push_type)
                }
                _ => HubMessage::Ignored,
            }
        }
        Some(7) => HubMessage::Close,
        Some(_) => HubMessage::Ignored,
        None => HubMessage::Unreadable,
    }
}

/// **Which of Bitwarden's `PushType`s mean "sync"**, by number.
///
/// * `0`-`10` -- every cipher and folder change, a whole-vault resync, an
///   organisation's keys, the account's settings (equivalent domains).
/// * `11`, `LogOut` -- the server has ended this device's session. A sync is
///   the right answer to that too: it fails with the 401 the window already
///   turns into a sign-in, rather than this module inventing a second way to
///   lock.
/// * `17`-`19` -- an organisation joined, left, or its collection settings
///   changed: what is visible here moved.
///
/// Not the Sends (`12`-`14`): the Sends screen fetches its own list and a
/// vault sync would not refresh it, so a Send notice would buy a whole-vault
/// download for nothing -- on the owner's server, about 3,400 database rows.
/// Not a login-with-device request (`15`, `16`), which this app does not
/// offer.
fn notice_for(push_type: i64) -> bool {
    matches!(push_type, 0..=11 | 17..=19)
}

/// The hub's websocket URL for a server root, or `None` when the root is not
/// an `http(s)` URL. A root with a path -- a server mounted under
/// `https://host/vault` -- keeps it, as every other route in
/// [`crate::rest::api`] does.
fn hub_url(base_url: &str) -> Option<String> {
    let base = base_url.trim_end_matches('/');
    let rest = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return None;
    };
    Some(format!("{rest}{HUB_PATH}"))
}

// ---- where the bearer comes from -------------------------------------------

/// One connection's worth of credentials.
pub struct Ticket {
    /// `wss://host/notifications/hub`.
    hub_url: String,
    /// `Bearer <access token>`, wiped on drop.
    bearer: Zeroizing<String>,
    /// Which [`publish`] this came from, so a connection taken under one
    /// account can notice that the process has moved to another.
    generation: u64,
}

/// Why there is no [`Ticket`] right now.
pub enum NoTicket {
    /// No backend has published a session -- the `bw serve` backend, or a
    /// window still on its sign-in card. Costs nothing to ask again.
    NoSource,
    /// There is a session and renewing it failed. Asked again only after a
    /// back-off, because each ask is a request to the token endpoint.
    Failed(String),
}

/// What the published session is reached through.
struct Source {
    client: RestClient,
    /// Weak: the backend owns its session, and a backend that has been
    /// dropped -- a lock, an account switch -- must not be kept alive by a
    /// listener that happened to be holding the handle.
    state: Weak<Mutex<Authenticated>>,
    generation: u64,
}

static SOURCE: Mutex<Option<Source>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// **Makes a direct-REST backend's session reachable by the hub listener.**
///
/// Called by [`crate::rest::backend::RestBackend::new`], and nowhere else:
/// that constructor's own doc calls it the one boundary that is exactly "a
/// different logged-in account", which is what replacing this slot means. A
/// listener connected under the previous one sees the generation move and
/// reconnects under this one.
pub(crate) fn publish(client: &RestClient, state: &Arc<Mutex<Authenticated>>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    *SOURCE.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(Source { client: client.clone(), state: Arc::downgrade(state), generation });
}

/// A ticket from whatever was last [`publish`]ed. `renew` forces a token
/// refresh -- asked for after the hub refused the one it was given.
fn published_ticket(renew: bool) -> Result<Ticket, NoTicket> {
    let (client, state, generation) = {
        let slot = SOURCE.lock().unwrap_or_else(PoisonError::into_inner);
        let source = slot.as_ref().ok_or(NoTicket::NoSource)?;
        let state = source.state.upgrade().ok_or(NoTicket::NoSource)?;
        (source.client.clone(), state, source.generation)
    };
    let hub_url = hub_url(client.base_url())
        .ok_or_else(|| NoTicket::Failed("the server URL is not http(s)".to_string()))?;
    let mut authenticated = state.lock().unwrap_or_else(PoisonError::into_inner);
    let bearer = client
        .hub_bearer(&mut authenticated.session, renew)
        .map_err(|e| NoTicket::Failed(format!("{e:?}")))?;
    Ok(Ticket { hub_url, bearer, generation })
}

fn published_generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

// ---- the listener ----------------------------------------------------------

/// What the listener needs from outside it. Production's is
/// [`HubListener::for_this_process`]; a test hands closures.
///
/// Nothing here wakes the window when a notice lands, deliberately: the
/// vault window already runs a frame every half second whatever happens, and
/// a notice waits [`Timing::settle`] before it is acted on anyway.
pub struct Feed {
    pub tickets: Box<dyn FnMut(bool) -> Result<Ticket, NoTicket> + Send>,
    /// Whether a connection taken under `ticket` should stay up.
    pub still_current: Box<dyn Fn(&Ticket) -> bool + Send>,
}

/// Every wait the listener makes, as one value so a test can shrink them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// TCP connect, the TLS and websocket handshakes, and SignalR's.
    pub connect: Duration,
    /// How long one read blocks: the grain at which a stop is noticed and a
    /// ping is sent.
    pub poll: Duration,
    /// How often [`PING`] is sent.
    pub ping_every: Duration,
    /// How long without hearing anything a connection is taken to be dead.
    /// SignalR's own client uses 30 seconds against a 15-second ping.
    pub silence: Duration,
    /// Between asks while nothing has been published.
    pub no_source_wait: Duration,
    /// The first wait after a failed connection; doubled per failure.
    pub retry_floor: Duration,
    /// The longest wait after a failed connection.
    pub retry_ceiling: Duration,
    /// How long after the LAST notice a burst is taken to have ended. A
    /// bulk edit in another client arrives as one notice per item; this
    /// turns it into one sync.
    pub settle: Duration,
}

impl Timing {
    pub const PRODUCTION: Self = Self {
        connect: Duration::from_secs(10),
        poll: Duration::from_millis(500),
        ping_every: Duration::from_secs(15),
        silence: Duration::from_secs(45),
        no_source_wait: Duration::from_secs(2),
        retry_floor: Duration::from_secs(5),
        retry_ceiling: Duration::from_secs(5 * 60),
        settle: Duration::from_secs(2),
    };
}

/// What the window reads, written by the listener thread.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// Connected, and the hub has answered the SignalR handshake.
    live: AtomicBool,
    /// Every notice ever received, plus one per REconnect -- a connection
    /// that was down may have missed some, and a sync is what Bitwarden's
    /// own clients do on reconnecting for the same reason.
    notices: AtomicU64,
    last_notice: Mutex<Option<Instant>>,
}

impl Shared {
    fn notice(&self) {
        *self.last_notice.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        self.notices.fetch_add(1, Ordering::SeqCst);
    }
}

/// **The open vault window's connection to the hub**, on a thread of its
/// own. Dropping it asks the thread to stop; the thread notices within one
/// [`Timing::poll`] (or one connect timeout, if it is mid-connect) and sends
/// the server a close.
pub struct HubListener {
    shared: Arc<Shared>,
    settle: Duration,
}

impl HubListener {
    /// The production listener: tickets from [`publish`]. Starts a thread
    /// even when nothing is published yet --
    /// a window that opens on its sign-in card publishes a backend a minute
    /// later -- and that thread costs one mutex read per
    /// [`Timing::no_source_wait`] until something is.
    pub fn for_this_process() -> Self {
        Self::start(
            Feed {
                tickets: Box::new(published_ticket),
                still_current: Box::new(|ticket| ticket.generation == published_generation()),
            },
            Timing::PRODUCTION,
        )
    }

    pub fn start(feed: Feed, timing: Timing) -> Self {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("notifications-hub".to_string())
            .spawn(move || listen(feed, timing, &thread_shared))
            .map_err(|e| log::warn!("the notifications hub thread could not start: {e}"))
            .ok();
        Self { shared, settle: timing.settle }
    }

    /// Whether the hub is connected and handshaken right now. While it is,
    /// the window's polling stands down: there is nothing for it to find
    /// that the hub would not have said.
    pub fn is_live(&self) -> bool {
        self.shared.live.load(Ordering::SeqCst)
    }

    /// **Notices the window has not acted on**, once they have settled:
    /// `Some(count)` when more than `seen` have arrived and the last of them
    /// is [`Timing::settle`] old. The window records `count` as seen when it
    /// starts the sync, so a notice landing between this read and that one
    /// is still counted next frame rather than lost.
    pub fn pending(&self, seen: u64, now: Instant) -> Option<u64> {
        let count = self.shared.notices.load(Ordering::SeqCst);
        if count <= seen {
            return None;
        }
        let last = *self.shared.last_notice.lock().unwrap_or_else(PoisonError::into_inner);
        let settled = last.is_none_or(|at| now.saturating_duration_since(at) >= self.settle);
        settled.then_some(count)
    }
}

impl Drop for HubListener {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.live.store(false, Ordering::SeqCst);
    }
}

/// How one connection ended.
#[derive(Debug)]
enum Ended {
    Stopped,
    /// The process moved to another account's session.
    Superseded,
    /// The hub refused the bearer.
    Unauthorized,
    /// The hub is not there, or speaks something this client cannot read.
    Unsupported(String),
    /// Never got as far as a working connection.
    Failed(String),
    /// Was working, and stopped.
    Dropped(String),
}

/// The thread body: connect, listen, and on any ending decide how long to
/// wait before the next attempt.
///
/// **A failing server costs one attempt per back-off**, doubling from
/// [`Timing::retry_floor`] to [`Timing::retry_ceiling`], and a refused bearer
/// is renewed exactly once before it is treated as a failure like any other
/// -- the token endpoint is not asked in a loop. That is the lesson of the
/// periodic refresh this app once had, which retried a failing start with
/// nothing between attempts.
fn listen(mut feed: Feed, timing: Timing, shared: &Shared) {
    let mut failures: u32 = 0;
    let mut renew = false;
    let mut connected_before = false;
    while !shared.stop.load(Ordering::SeqCst) {
        let ticket = match (feed.tickets)(renew) {
            Ok(ticket) => ticket,
            Err(NoTicket::NoSource) => {
                renew = false;
                pause(shared, timing.no_source_wait);
                continue;
            }
            Err(NoTicket::Failed(why)) => {
                log::warn!("the notifications hub has no usable session: {why}");
                renew = false;
                failures = failures.saturating_add(1);
                pause(shared, backoff(timing, failures));
                continue;
            }
        };
        let renewed = renew;
        renew = false;
        let ended = one_connection(&ticket, &feed, timing, shared, &mut connected_before);
        shared.live.store(false, Ordering::SeqCst);
        match ended {
            Ended::Stopped => return,
            Ended::Superseded => {
                log::info!("the notifications hub is reconnecting for a new session");
                failures = 0;
            }
            Ended::Unauthorized if !renewed => {
                log::info!("the notifications hub refused the access token; renewing it once");
                renew = true;
            }
            Ended::Unsupported(why) => {
                log::info!(
                    "this server has no notifications hub this app can use ({why}); the vault \
                     window keeps itself current by polling instead"
                );
                return;
            }
            Ended::Dropped(why) => {
                log::info!("the notifications hub connection dropped: {why}");
                failures = 0;
                pause(shared, timing.retry_floor);
            }
            Ended::Unauthorized | Ended::Failed(_) => {
                let why = match ended {
                    Ended::Failed(why) => why,
                    _ => "the renewed access token was refused too".to_string(),
                };
                failures = failures.saturating_add(1);
                let wait = backoff(timing, failures);
                log::warn!("the notifications hub could not connect ({why}); retrying in {wait:?}");
                pause(shared, wait);
            }
        }
    }
}

/// The wait after the `failures`-th failure in a row.
fn backoff(timing: Timing, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    timing.retry_floor.saturating_mul(1 << doublings).min(timing.retry_ceiling)
}

/// Sleeps `total`, waking early for a stop.
fn pause(shared: &Shared, total: Duration) {
    let until = Instant::now() + total;
    while !shared.stop.load(Ordering::SeqCst) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

fn one_connection(
    ticket: &Ticket,
    feed: &Feed,
    timing: Timing,
    shared: &Shared,
    connected_before: &mut bool,
) -> Ended {
    let Ok(uri) = ticket.hub_url.parse::<Uri>() else {
        return Ended::Unsupported("the hub URL does not parse".to_string());
    };
    let stream = match connect(&uri, timing) {
        Ok(stream) => stream,
        Err(why) => return Ended::Failed(why),
    };
    let mut request = match uri.into_client_request() {
        Ok(request) => request,
        Err(e) => return Ended::Unsupported(e.to_string()),
    };
    // The one copy of the credential this module cannot wipe: `http` keeps
    // header values in its own `Bytes`. Marked sensitive so its `Debug`
    // prints nothing, and dropped with the request as soon as the handshake
    // is written.
    let Ok(mut bearer) = HeaderValue::from_str(ticket.bearer.as_str()) else {
        return Ended::Failed("the access token is not a valid header value".to_string());
    };
    bearer.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, bearer);

    let mut socket = match tungstenite::client(request, stream) {
        Ok((socket, _)) => socket,
        Err(HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            // Read off the parts rather than through the `status` method:
            // `job_object`'s census of child-process starts reads any call of
            // that name as `Command::status`, and an exemption for this file
            // would be one more rule-shaped hole in a scan that has none.
            let status = (*response).into_parts().0.status.as_u16();
            return match status {
                401 | 403 => Ended::Unauthorized,
                404 | 405 | 426 | 501 => Ended::Unsupported(format!("HTTP {status}")),
                _ => Ended::Failed(format!("HTTP {status}")),
            };
        }
        Err(HandshakeError::Failure(e)) => return Ended::Failed(e.to_string()),
        Err(HandshakeError::Interrupted(_)) => {
            return Ended::Failed("the websocket handshake timed out".to_string());
        }
    };
    if let Err(e) = socket.send(Message::text(HANDSHAKE)) {
        return Ended::Failed(e.to_string());
    }
    if let Err(e) = socket.get_mut().tcp().set_read_timeout(Some(timing.poll)) {
        return Ended::Failed(e.to_string());
    }

    let opened = Instant::now();
    let mut acked = false;
    let mut last_heard = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            let _ = socket.close(None);
            let _ = socket.flush();
            return Ended::Stopped;
        }
        if !(feed.still_current)(ticket) {
            let _ = socket.close(None);
            let _ = socket.flush();
            return Ended::Superseded;
        }
        let now = Instant::now();
        if !acked && now.duration_since(opened) >= timing.connect {
            return Ended::Failed("the hub never answered the SignalR handshake".to_string());
        }
        if acked && now.duration_since(last_heard) >= timing.silence {
            return Ended::Dropped(format!("nothing heard for {:?}", timing.silence));
        }
        if acked && now.duration_since(last_ping) >= timing.ping_every {
            if let Err(e) = socket.send(Message::text(PING)) {
                return Ended::Dropped(e.to_string());
            }
            last_ping = now;
        }
        let message = match socket.read() {
            Ok(message) => message,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(e) => return Ended::Dropped(e.to_string()),
        };
        last_heard = Instant::now();
        let text = match &message {
            Message::Text(_) | Message::Binary(_) => match message.to_text() {
                Ok(text) => text,
                Err(_) => return Ended::Unsupported("a binary hub protocol".to_string()),
            },
            Message::Close(_) => return Ended::Dropped("the server closed the connection".to_string()),
            _ => continue,
        };
        for read in read_messages(text) {
            match read {
                HubMessage::HandshakeAck { error: Some(error) } => {
                    return Ended::Unsupported(format!("the handshake was refused: {error}"));
                }
                HubMessage::HandshakeAck { error: None } => {
                    acked = true;
                    shared.live.store(true, Ordering::SeqCst);
                    log::info!("the notifications hub is connected");
                    if *connected_before {
                        // Down for a while: whatever changed meanwhile was
                        // said to nobody.
                        shared.notice();
                    }
                    *connected_before = true;
                }
                HubMessage::VaultChanged(push_type) if acked => {
                    log::info!("the notifications hub says the vault changed (push type {push_type})");
                    shared.notice();
                }
                HubMessage::Close => {
                    return Ended::Dropped("the hub said goodbye".to_string());
                }
                HubMessage::Unreadable => {
                    return Ended::Unsupported("a hub protocol other than JSON".to_string());
                }
                HubMessage::VaultChanged(_) | HubMessage::Ignored => {}
            }
        }
    }
}

// ---- the transport ---------------------------------------------------------

/// A TCP stream, with or without TLS on it.
enum Transport {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Transport {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => tls.get_ref(),
        }
    }
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(buf),
            Self::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

/// The TCP connection, and TLS on it for `wss`, bounded by
/// [`Timing::connect`] for every step that can block.
fn connect(uri: &Uri, timing: Timing) -> Result<Transport, String> {
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err("not a websocket URL".to_string()),
    };
    let host = uri.host().ok_or("the hub URL has no host")?;
    // `Uri::host` keeps an IPv6 literal's brackets; neither the resolver nor
    // a TLS server name wants them.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let addresses = (host, port).to_socket_addrs().map_err(|e| format!("resolving {host}: {e}"))?;
    let mut last_error = format!("{host} resolved to no address");
    let mut tcp = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, timing.connect) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(e) => last_error = format!("connecting to {host}: {e}"),
        }
    }
    let tcp = tcp.ok_or(last_error)?;
    tcp.set_read_timeout(Some(timing.connect)).map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(timing.connect)).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    if !secure {
        return Ok(Transport::Plain(tcp));
    }
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("{host} is not a TLS server name: {e}"))?;
    let connection = rustls::ClientConnection::new(tls_config(), name).map_err(|e| e.to_string())?;
    Ok(Transport::Tls(Box::new(rustls::StreamOwned::new(connection, tcp))))
}

/// The hub's TLS configuration: ring, and Mozilla's roots -- what ureq
/// verifies every other request this app makes against, so the hub is
/// trusted on exactly the terms the API it sits beside is.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config =
            rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        // A websocket upgrade is HTTP/1.1; offering h2 would let a server
        // pick a protocol the upgrade cannot happen over.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(config)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use tungstenite::handshake::server::{ErrorResponse, Request, Response};

    // ---- reading messages ---------------------------------------------------

    /// The four things this client reads, each as a real server sends it:
    /// NodeWarden's PascalCase invocation, ASP.NET's camelCase one, the
    /// keep-alive, the handshake answer both ways, and a close.
    #[test]
    fn each_message_the_hub_sends_is_read_for_what_it_means() {
        let pascal = "{\"type\":1,\"target\":\"ReceiveMessage\",\"arguments\":[{\"ContextId\":null,\"Type\":0,\"Payload\":{}}]}\u{1e}";
        let camel = "{\"type\":1,\"target\":\"ReceiveMessage\",\"arguments\":[{\"contextId\":null,\"type\":9,\"payload\":{}}]}\u{1e}";
        assert_eq!(read_messages(pascal), [HubMessage::VaultChanged(0)]);
        assert_eq!(read_messages(camel), [HubMessage::VaultChanged(9)]);
        assert_eq!(read_messages(PING), [HubMessage::Ignored]);
        assert_eq!(read_messages("{}\u{1e}"), [HubMessage::HandshakeAck { error: None }]);
        assert_eq!(
            read_messages("{\"error\":\"no\"}\u{1e}"),
            [HubMessage::HandshakeAck { error: Some("no".to_string()) }]
        );
        assert_eq!(read_messages("{\"type\":7}\u{1e}"), [HubMessage::Close]);
        // Several records in one frame, in order.
        assert_eq!(
            read_messages(&format!("{{}}\u{1e}{PING}{pascal}")),
            [HubMessage::HandshakeAck { error: None }, HubMessage::Ignored, HubMessage::VaultChanged(0)]
        );
    }

    /// **Which pushes cost a sync.** A Send or a login-with-device request
    /// does not: each would be a whole-vault download that changes nothing
    /// on screen. A log-out does, because the sync's 401 is what signs the
    /// window out.
    #[test]
    fn only_a_push_that_can_change_the_vault_on_screen_asks_for_a_sync() {
        let invocation = |push_type: i64| {
            format!(
                "{{\"type\":1,\"target\":\"ReceiveMessage\",\"arguments\":[{{\"Type\":{push_type}}}]}}\u{1e}"
            )
        };
        for push_type in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 17, 18, 19] {
            assert_eq!(
                read_messages(&invocation(push_type)),
                [HubMessage::VaultChanged(push_type)],
                "{push_type}"
            );
        }
        for push_type in [12, 13, 14, 15, 16, 20, 102] {
            assert_eq!(read_messages(&invocation(push_type)), [HubMessage::Ignored], "{push_type}");
        }
        // Another target entirely is not a vault change, whatever it carries.
        assert_eq!(
            read_messages("{\"type\":1,\"target\":\"AuthRequestResponseRecieved\",\"arguments\":[{\"Type\":0}]}\u{1e}"),
            [HubMessage::Ignored]
        );
    }

    #[test]
    fn a_frame_that_is_not_signalr_json_is_unreadable() {
        assert_eq!(read_messages("\u{2}\u{91}\u{6}"), [HubMessage::Unreadable]);
        assert_eq!(read_messages("[1,2]\u{1e}"), [HubMessage::Unreadable]);
        assert_eq!(read_messages("{\"type\":\"one\"}\u{1e}"), [HubMessage::Unreadable]);
    }

    #[test]
    fn the_hub_url_is_the_server_root_with_a_websocket_scheme() {
        assert_eq!(
            hub_url("https://vault.example.com/").as_deref(),
            Some("wss://vault.example.com/notifications/hub")
        );
        assert_eq!(
            hub_url("http://192.168.1.5:8080").as_deref(),
            Some("ws://192.168.1.5:8080/notifications/hub")
        );
        assert_eq!(
            hub_url("https://host/vault").as_deref(),
            Some("wss://host/vault/notifications/hub")
        );
        assert_eq!(hub_url("ftp://host"), None);
    }

    #[test]
    fn the_back_off_doubles_from_its_floor_to_its_ceiling() {
        let timing = Timing::PRODUCTION;
        assert_eq!(backoff(timing, 1), Duration::from_secs(5));
        assert_eq!(backoff(timing, 2), Duration::from_secs(10));
        assert_eq!(backoff(timing, 4), Duration::from_secs(40));
        assert_eq!(backoff(timing, 7), Duration::from_secs(300));
        assert_eq!(backoff(timing, u32::MAX), Duration::from_secs(300));
    }

    // ---- against a real websocket -------------------------------------------

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

    const TOKEN: &str = "Bearer AT-test";

    /// What the test server saw of one connection.
    #[derive(Debug, Default, Clone)]
    struct Seen {
        authorization: Option<String>,
        uri: String,
        texts: Vec<String>,
    }

    /// A hub on `127.0.0.1` that runs `script` once per connection, in
    /// order, and reports what each connection sent it.
    fn hub(
        scripts: Vec<Box<dyn FnOnce(&mut tungstenite::WebSocket<TcpStream>) + Send>>,
        refuse_first: bool,
    ) -> (String, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut refuse = refuse_first;
            for script in scripts {
                let Ok((stream, _)) = listener.accept() else { return };
                stream.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
                let mut seen = Seen::default();
                let refusing = std::mem::take(&mut refuse);
                let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                    seen.uri = request.uri().to_string();
                    seen.authorization = request
                        .headers()
                        .get(AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    if refusing {
                        let mut refusal = ErrorResponse::new(None);
                        *refusal.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED;
                        return Err(refusal);
                    }
                    Ok(response)
                };
                let Ok(mut socket) = tungstenite::accept_hdr(stream, callback) else {
                    let _ = tx.send(seen);
                    continue;
                };
                // The client's SignalR handshake comes first.
                if let Ok(message) = socket.read() {
                    seen.texts.push(message.to_text().unwrap_or("").to_string());
                }
                script(&mut socket);
                // Whatever else it said before the script finished.
                while let Ok(message) = socket.read() {
                    if let Ok(text) = message.to_text() {
                        seen.texts.push(text.to_string());
                    }
                    if message.is_close() {
                        break;
                    }
                }
                let _ = tx.send(seen);
            }
        });
        (base, rx)
    }

    struct Probe {
        listener: HubListener,
        asks: mpsc::Receiver<bool>,
    }

    fn listen_to(base: &str) -> Probe {
        let url = hub_url(base).expect("an http base");
        let (ask_tx, asks) = mpsc::channel();
        let listener = HubListener::start(
            Feed {
                tickets: Box::new(move |renew| {
                    let _ = ask_tx.send(renew);
                    Ok(Ticket {
                        hub_url: url.clone(),
                        bearer: Zeroizing::new(TOKEN.to_string()),
                        generation: 1,
                    })
                }),
                still_current: Box::new(|_| true),
            },
            FAST,
        );
        Probe { listener, asks }
    }

    fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never happened: {what}");
    }

    fn ack(socket: &mut tungstenite::WebSocket<TcpStream>) {
        // NodeWarden sends its answer as a BINARY frame; so does this.
        socket.send(Message::binary(b"{}\x1e".to_vec())).expect("ack");
    }

    fn push(socket: &mut tungstenite::WebSocket<TcpStream>, push_type: i64) {
        let text = format!(
            "{{\"type\":1,\"target\":\"ReceiveMessage\",\"arguments\":[{{\"ContextId\":null,\"Type\":{push_type},\"Payload\":{{}}}}]}}\u{1e}"
        );
        socket.send(Message::text(text)).expect("push");
    }

    /// **A push reaches the window**: the bearer goes in the header and not
    /// the URL, the SignalR handshake asks for JSON, a cipher update becomes
    /// one pending notice, and dropping the listener closes the connection.
    #[test]
    fn a_pushed_change_becomes_a_pending_notice() {
        let (base, seen) = hub(
            vec![Box::new(|socket| {
                ack(socket);
                std::thread::sleep(Duration::from_millis(50));
                push(socket, 0);
                std::thread::sleep(Duration::from_millis(300));
            })],
            false,
        );
        let probe = listen_to(&base);
        eventually("the hub is live", || probe.listener.is_live());
        eventually("the push is pending", || {
            probe.listener.pending(0, Instant::now()).is_some()
        });
        assert_eq!(probe.listener.pending(0, Instant::now()), Some(1));
        assert_eq!(probe.listener.pending(1, Instant::now()), None, "a seen notice is not pending");
        drop(probe.listener);
        let seen = seen.recv_timeout(Duration::from_secs(5)).expect("the connection report");
        assert_eq!(seen.authorization.as_deref(), Some(TOKEN));
        assert!(!seen.uri.contains("AT-test"), "the token is in the URL: {}", seen.uri);
        assert!(seen.uri.ends_with(HUB_PATH), "{}", seen.uri);
        assert_eq!(seen.texts.first().map(String::as_str), Some(HANDSHAKE));
    }

    /// A push of a kind that does not change the vault is not a notice.
    #[test]
    fn a_send_push_is_not_a_notice() {
        let (base, _seen) = hub(
            vec![Box::new(|socket| {
                ack(socket);
                push(socket, 12);
                std::thread::sleep(Duration::from_millis(300));
            })],
            false,
        );
        let probe = listen_to(&base);
        eventually("the hub is live", || probe.listener.is_live());
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(probe.listener.pending(0, Instant::now()), None);
    }

    /// **The client keeps the connection alive itself**, because a
    /// hibernating hub says nothing unless spoken to: it pings on its
    /// schedule, and a hub that answers keeps one connection up well past
    /// `silence` -- no reconnect, which would have counted as a notice.
    #[test]
    fn the_client_pings_on_its_schedule_and_answered_pings_keep_it_up() {
        let pings = Arc::new(AtomicU64::new(0));
        let heard = Arc::clone(&pings);
        let (base, _seen) = hub(
            vec![Box::new(move |socket| {
                ack(socket);
                // Answer pings the way NodeWarden's auto-response does, for
                // twice `silence`.
                let until = Instant::now() + FAST.silence * 2;
                while Instant::now() < until {
                    if let Ok(message) = socket.read() {
                        if message.to_text().is_ok_and(|t| t == PING) {
                            heard.fetch_add(1, Ordering::SeqCst);
                            let _ = socket.send(Message::text(PING));
                        }
                    }
                }
            })],
            false,
        );
        let probe = listen_to(&base);
        eventually("the hub is live", || probe.listener.is_live());
        std::thread::sleep(FAST.silence + FAST.silence / 2);
        assert!(probe.listener.is_live(), "an answered connection was dropped");
        assert_eq!(probe.listener.pending(0, Instant::now()), None, "it reconnected");
        let pings = pings.load(Ordering::SeqCst);
        assert!(pings >= 4, "only {pings} pings in {:?}", FAST.silence * 3 / 2);
    }

    /// **A refused bearer is renewed exactly once**, and the renewed one is
    /// used.
    #[test]
    fn a_refused_token_is_renewed_once_and_the_connection_retried() {
        let (base, seen) = hub(
            vec![
                Box::new(|_| {}),
                Box::new(|socket| {
                    ack(socket);
                    std::thread::sleep(Duration::from_millis(300));
                }),
            ],
            true,
        );
        let probe = listen_to(&base);
        eventually("the hub is live after renewing", || probe.listener.is_live());
        let asks: Vec<bool> = probe.asks.try_iter().collect();
        assert_eq!(asks[..2], [false, true], "the second ask was not a renewal: {asks:?}");
        let refused = seen.recv_timeout(Duration::from_secs(5)).expect("the refused connection");
        assert_eq!(refused.authorization.as_deref(), Some(TOKEN));
    }

    /// **A reconnect is a notice**: whatever changed while the connection
    /// was down was said to nobody.
    #[test]
    fn a_reconnect_after_a_drop_counts_as_a_notice() {
        let (base, _seen) = hub(
            vec![
                Box::new(|socket| {
                    ack(socket);
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = socket.close(None);
                }),
                Box::new(|socket| {
                    ack(socket);
                    std::thread::sleep(Duration::from_millis(500));
                }),
            ],
            false,
        );
        let probe = listen_to(&base);
        eventually("the reconnect is a notice", || {
            probe.listener.pending(0, Instant::now()).is_some()
        });
        assert!(probe.listener.is_live());
    }

    /// **A silent connection is a dead one**: a hub that stops answering is
    /// left and reconnected, not listened to forever.
    #[test]
    fn a_hub_that_goes_silent_is_reconnected() {
        let (base, _seen) = hub(
            vec![
                Box::new(|socket| {
                    ack(socket);
                    // Hears the pings and never answers; longer than `silence`.
                    let until = Instant::now() + Duration::from_millis(1500);
                    while Instant::now() < until {
                        let _ = socket.read();
                    }
                }),
                Box::new(|socket| {
                    ack(socket);
                    std::thread::sleep(Duration::from_millis(500));
                }),
            ],
            false,
        );
        let probe = listen_to(&base);
        eventually("a second connection", || {
            probe.listener.pending(0, Instant::now()).is_some()
        });
    }

    /// **A hub this client cannot read stands down for good**, so the
    /// window's polling is not silenced by a channel that delivers nothing.
    #[test]
    fn a_messagepack_hub_is_left_alone_and_polling_carries_on() {
        let (base, _seen) = hub(
            vec![Box::new(|socket| {
                ack(socket);
                // A MessagePack ping: length 2, then [6].
                let _ = socket.send(Message::binary(vec![0x02, 0x91, 0x06]));
                std::thread::sleep(Duration::from_millis(300));
            })],
            false,
        );
        let probe = listen_to(&base);
        eventually("the listener gave up", || {
            let asks: Vec<bool> = probe.asks.try_iter().collect();
            !asks.is_empty() && !probe.listener.is_live()
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!probe.listener.is_live());
        assert_eq!(probe.asks.try_iter().count(), 0, "it asked for another ticket");
    }

    /// With nothing published there is nothing to connect with, and asking
    /// again costs nothing -- but it is still asked, because a window that
    /// opened on its sign-in card publishes a session later.
    #[test]
    fn no_source_means_no_connection_until_one_appears() {
        let (tx, asks) = mpsc::channel();
        let listener = HubListener::start(
            Feed {
                tickets: Box::new(move |_| {
                    let _ = tx.send(());
                    Err(NoTicket::NoSource)
                }),
                still_current: Box::new(|_| true),
            },
            FAST,
        );
        std::thread::sleep(Duration::from_millis(150));
        assert!(asks.try_iter().count() >= 2, "it stopped asking");
        assert!(!listener.is_live());
    }

    /// `pending` waits for a burst to settle.
    #[test]
    fn a_burst_of_notices_is_pending_only_once_it_has_settled() {
        let listener = HubListener { shared: Arc::new(Shared::default()), settle: Duration::from_secs(2) };
        listener.shared.notice();
        listener.shared.notice();
        let now = Instant::now();
        assert_eq!(listener.pending(0, now), None, "pending mid-burst");
        assert_eq!(listener.pending(0, now + Duration::from_secs(2)), Some(2));
        listener.shared.stop.store(true, Ordering::SeqCst);
    }
}

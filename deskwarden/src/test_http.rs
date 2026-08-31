//! The mock HTTP server this crate's tests are written against: a hand-rolled
//! [`std::net::TcpListener`] that speaks exactly as much HTTP/1.1 as those
//! tests need, and answers `mockito`'s API so that not one of them had to be
//! rewritten to move.
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
//! It is worth being precise about what that is *not*, because four plausible
//! explanations were measured and all four are wrong:
//!
//! * **Not Windows ephemeral-port churn.** The same client code, at the same
//!   connection rate and the same thread count, against a hand-rolled
//!   [`std::net::TcpListener`] instead of a mock server: **0 failures in 2400
//!   requests**, where `mockito` failed 706. Port recycling cannot tell the two
//!   servers apart. That measurement is the whole design of this module: the
//!   listener that did not fail is the listener below.
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
//! * **Not a version behind.** 1.7.2 is the newest release and is what was
//!   locked while all of the above was measured.
//!
//! What every measurement *did* track was **how many mock-server requests are
//! in flight at once**, and nothing else: 1 failure per 1000 sequential, 57 of
//! 400 at four threads, 706 of 2400 at twenty-four. One server with 24 clients
//! and 24 servers with one client each failed at the same rate, so it is the
//! traffic that matters and not how it is spread over servers.
//!
//! # Why a replacement and not a gate
//!
//! The first fix was a gate: one permit, so only one test held a mock server
//! at a time. It took the `--lib` suite from 70-74 failures to 17-41 -- and
//! stopped there, because the sequential rate is not zero and serialising
//! cannot make it zero. It also cost real wall-clock: every test that touches
//! a server queued behind every other one.
//!
//! Retrying a reset request, sleeping, or widening a timeout was never on the
//! table, for a reason worth writing down: a real `Transport` error is exactly
//! what several tests in `rest::api` and `vault_bridge` assert, and a suite
//! that swallows transport failures cannot report the two regressions this
//! repository found that way. Removing `mockito` removes the condition. A
//! reset that happens anyway still fails its test loudly.
//!
//! # What is implemented, and what is deliberately not
//!
//! The surface below is exactly the one this crate's call sites use, and it
//! copies `mockito` 1.7.2's *semantics*, not just its names -- because the call
//! sites were written against those semantics and a near-miss would have
//! turned a mechanical move into a silent change of what the tests check:
//!
//! * A `mock(method, path)` with no [`Mock::match_query`] matches the path
//!   **and query together** as one string, so `/x` does not match `/x?y=1`.
//!   Adding `match_query` splits them. `mockito`'s `PathAndQueryMatcher`.
//! * Mock selection on a request: of the mocks that match, the first one that
//!   has not yet reached its expected hit count wins; if all of them have, the
//!   **last created** one does. Verbatim from `mockito`'s `handle_request`.
//! * [`Mock::assert`] with no expectation set means **exactly one** hit, not
//!   "at least one". `expect(n)` pins both ends, `expect_at_least(n)` only the
//!   lower one.
//! * An unmatched request is answered **501**, with no body. Tests in
//!   `vault_bridge`, `vault_cache`, `updater` and `rest::backend` name that
//!   status in their comments or assertions: it is the observable difference
//!   between "the client sent the request I expected" and "it sent something
//!   else", and it is load-bearing.
//!
//! Not implemented, because nothing here uses them: `Matcher::Binary`,
//! `Matcher::JsonString`, `Matcher::PartialJsonString`, chunked responses,
//! request-derived statuses and headers, and every `_async` method. A call
//! site that reaches for one gets a compile error, which is the right
//! failure. Request-derived *bodies* are here, because five mocks need them.
//!
//! One deliberate divergence: `mockito` **panics** inside its own runtime when
//! [`Matcher::Json`] is handed a body that is not JSON, which reaches the
//! client as a dead connection. Here a body that will not parse simply does
//! not match, so the request falls through to the 501 above and the test fails
//! saying so. Both fail; only one of them says why.
//!
//! # Connections are reused, and the close is graceful
//!
//! Both halves of that were measured the hard way, and both produced the
//! *identical* `os error 10054` from a listener with no `mockito` in it:
//!
//! | connection handling | close | failures / 240 |
//! |---|---|---|
//! | one request per connection | `shutdown(Both)` | 69 |
//! | one request per connection | `shutdown(Write)`, then drop | 82 |
//! | one request per connection | `shutdown(Write)`, read to EOF | 18 |
//! | **keep-alive** | `shutdown(Write)`, read to EOF | 0 |
//!
//! The close is in [`graceful_close`], which explains its three rows. The
//! remaining 18 were the connection rate itself: one connection per request is
//! one ephemeral port per request, Windows recycles that range through
//! `TIME_WAIT`, and a SYN landing on a port still in `TIME_WAIT` is answered
//! with an RST that the client reads as a reset in the status line. Keeping the
//! connection open removes the churn instead of surviving it: `ureq` pools, so
//! a test that makes twenty requests makes one connection.
//!
//! The cost is a thread parked on each live connection, which is why
//! [`IDLE_BEFORE_HANGUP`] exists.

use std::borrow::Cow;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

/// One request as it arrived, already read to the end of its body.
///
/// `mockito`'s `Request` hands out `Result`s from `body()` and
/// `utf8_lossy_body()` because it reads the body lazily and those accessors
/// can be called before the read. Here the body is read off the socket before
/// a matcher ever sees the request, so there is no state in which it is
/// absent and no `Result` to unwrap. That is the one shape difference the call
/// sites could see, and it cost eight call sites: seven `.expect("a body")`
/// calls and one `.map(..).unwrap_or_default()`. Nothing was weakened by it --
/// `vault_bridge`'s `.expect("a PUT this app makes always carries a body")` was
/// unwrapping a `Result` that was always `Ok`, and an empty body still fails on
/// the very next line, where it is parsed as JSON.
#[derive(Debug, Clone)]
pub struct Request {
    method: String,
    path_and_query: String,
    /// Field names lowercased on the way in, so lookup is a plain comparison.
    /// HTTP field names are case-insensitive and `ureq` does not always spell
    /// them the way the tests do.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    /// The HTTP method, uppercase as it arrived.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The path, excluding the query part.
    #[must_use]
    pub fn path(&self) -> &str {
        self.path_and_query.split('?').next().unwrap_or("")
    }

    /// The path including the query part, exactly as the client wrote it.
    #[must_use]
    pub fn path_and_query(&self) -> &str {
        &self.path_and_query
    }

    /// Every value sent for `name`, which HTTP allows to repeat.
    #[must_use]
    pub fn header(&self, name: &str) -> Vec<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .filter(|(field, _)| *field == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    /// Whether `name` was sent at all.
    #[must_use]
    pub fn has_header(&self, name: &str) -> bool {
        !self.header(name).is_empty()
    }

    /// The request body. Empty when there was none.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The request body as text, with invalid UTF-8 replaced.
    #[must_use]
    pub fn utf8_lossy_body(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The one-line form used in a [`Mock::assert`] failure, so a test that did
    /// not get the request it expected can see what it did get.
    fn summary(&self) -> String {
        format!("{} {}  body: {}", self.method, self.path_and_query, self.utf8_lossy_body())
    }
}

// ---------------------------------------------------------------------------
// matchers
// ---------------------------------------------------------------------------

/// How a path, query, header value or body is compared against what arrived.
///
/// The variants and their meanings are `mockito`'s; see the module docs for
/// the ones that are deliberately absent.
#[derive(Clone, Debug, PartialEq)]
pub enum Matcher {
    /// The value must equal this string exactly.
    Exact(String),
    /// The value must be matched by this regular expression, anywhere in it.
    Regex(String),
    /// The value must parse as JSON equal to this.
    Json(serde_json::Value),
    /// The value must parse as JSON that *contains* this: every key here must
    /// be present with a matching value, and extra keys are allowed.
    PartialJson(serde_json::Value),
    /// The value, read as a URL-encoded form, must carry this field with this
    /// value. Both are given decoded.
    UrlEncoded(String, String),
    /// Every one of these must match.
    AllOf(Vec<Matcher>),
    /// At least one of these must match.
    AnyOf(Vec<Matcher>),
    /// Anything, as long as it is there.
    Any,
    /// Nothing: the header must be absent, or the query/body empty.
    Missing,
}

impl From<&str> for Matcher {
    fn from(value: &str) -> Self {
        Matcher::Exact(value.to_string())
    }
}

impl std::fmt::Display for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Matcher::Exact(value) => write!(f, "{value}"),
            Matcher::Regex(value) => write!(f, "{value} (regex)"),
            Matcher::Json(value) => write!(f, "{value} (json)"),
            Matcher::PartialJson(value) => write!(f, "{value} (partial json)"),
            Matcher::UrlEncoded(field, value) => write!(f, "{field}={value} (urlencoded)"),
            Matcher::AllOf(inner) => write!(f, "({}) (all of)", join(inner)),
            Matcher::AnyOf(inner) => write!(f, "({}) (any of)", join(inner)),
            Matcher::Any => write!(f, "(any)"),
            Matcher::Missing => write!(f, "(missing)"),
        }
    }
}

fn join(matchers: &[Matcher]) -> String {
    matchers.iter().map(Matcher::to_string).collect::<Vec<_>>().join(", ")
}

impl Matcher {
    /// Match against a single string -- a path, a query, a header value or a
    /// body.
    fn matches_value(&self, other: &str) -> bool {
        match self {
            Matcher::Exact(expected) => expected == other,
            // A regex that does not compile is a bug in the test, and there is
            // no answer to it that is not a panic: silently not matching would
            // read as "the client sent the wrong thing".
            Matcher::Regex(expected) => regex::Regex::new(expected)
                .unwrap_or_else(|err| {
                    panic!("the matcher regex {expected:?} does not compile: {err}")
                })
                .is_match(other),
            // A body that is not JSON does not match a JSON matcher. See the
            // module docs: `mockito` panics here, inside its own runtime, which
            // reaches the test as a dead connection rather than as a failed
            // match.
            Matcher::Json(expected) => serde_json::from_str::<serde_json::Value>(other)
                .is_ok_and(|actual| actual == *expected),
            Matcher::PartialJson(expected) => serde_json::from_str::<serde_json::Value>(other)
                .is_ok_and(|actual| json_contains(&actual, expected)),
            Matcher::UrlEncoded(field, value) => form_pairs(other)
                .is_some_and(|pairs| pairs.iter().any(|(f, v)| f == field && v == value)),
            Matcher::AllOf(inner) => inner.iter().all(|m| m.matches_value(other)),
            Matcher::AnyOf(inner) => inner.iter().any(|m| m.matches_value(other)),
            Matcher::Any => true,
            Matcher::Missing => other.is_empty(),
        }
    }

    /// Match against every value sent for one header field.
    ///
    /// [`Matcher::Missing`] is the reason this is not just a loop over
    /// [`Matcher::matches_value`]: it is a statement about the *set* of values
    /// -- that it is empty -- and no single value can answer it. Every other
    /// matcher requires at least one value and must match all of them.
    fn matches_values(&self, values: &[&str]) -> bool {
        match self {
            Matcher::Missing => values.is_empty(),
            Matcher::AllOf(inner) if values.is_empty() => {
                inner.iter().all(|m| m.matches_values(values))
            }
            Matcher::AnyOf(inner) if values.is_empty() => {
                inner.iter().any(|m| m.matches_values(values))
            }
            _ => !values.is_empty() && values.iter().all(|value| self.matches_value(value)),
        }
    }
}

/// Whether `expected` is contained in `actual`: objects by key, arrays by
/// leading position, everything else by equality.
///
/// This is `assert_json_diff`'s `CompareMode::Inclusive`, which is what
/// `mockito` calls for [`Matcher::PartialJson`]. Arrays compare by index and
/// `actual` may be longer -- a shorter one is a miss, because the expected
/// element at that index is simply not there.
fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match (actual, expected) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(expected)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|got| json_contains(got, value))),
        (serde_json::Value::Array(actual), serde_json::Value::Array(expected)) => {
            expected.len() <= actual.len()
                && expected.iter().zip(actual.iter()).all(|(want, got)| json_contains(got, want))
        }
        _ => actual == expected,
    }
}

/// Split a URL-encoded form -- a query string or an
/// `application/x-www-form-urlencoded` body -- into decoded pairs.
///
/// `None` when a percent escape is malformed or decodes to invalid UTF-8,
/// which is a miss rather than a panic for [`Matcher::Json`]'s reason.
fn form_pairs(raw: &str) -> Option<Vec<(String, String)>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    raw.split('&')
        .map(|pair| {
            let mut halves = pair.splitn(2, '=');
            let field = percent_decode(halves.next().unwrap_or(""))?;
            let value = percent_decode(halves.next().unwrap_or(""))?;
            Some((field, value))
        })
        .collect()
}

/// Undo `application/x-www-form-urlencoded` escaping: `+` is a space and `%XX`
/// is a byte.
fn percent_decode(raw: &str) -> Option<String> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => {
                out.push(b' ');
                at += 1;
            }
            b'%' => {
                let high = nibble(*bytes.get(at + 1)?)?;
                let low = nibble(*bytes.get(at + 2)?)?;
                out.push((high << 4) | low);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// How a mock matches the request target.
///
/// `mockito`'s `PathAndQueryMatcher`, and the distinction is not cosmetic: a
/// mock built as `mock("GET", "/list/object/items")` does **not** answer
/// `/list/object/items?trash=true`, which is what lets `vault_cache` tell the
/// trash listing apart from the live one without saying so twice.
#[derive(Clone, Debug)]
enum Target {
    /// Path and query compared as one string.
    Unified(Matcher),
    /// Path and query compared separately, once `match_query` has been called.
    Split(Matcher, Matcher),
}

impl Target {
    fn matches(&self, path_and_query: &str) -> bool {
        match self {
            Target::Unified(matcher) => matcher.matches_value(path_and_query),
            Target::Split(path, query) => {
                let mut parts = path_and_query.splitn(2, '?');
                let got_path = parts.next().unwrap_or("");
                let got_query = parts.next().unwrap_or("");
                path.matches_value(got_path) && query.matches_value(got_query)
            }
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Unified(matcher) => write!(f, "{matcher}"),
            Target::Split(path, query) => write!(f, "{path}?{query}"),
        }
    }
}

// ---------------------------------------------------------------------------
// mocks
// ---------------------------------------------------------------------------

/// What a mock answers with.
///
/// The second variant is not a convenience: `vault_bridge`'s `bw serve` PUT
/// mock, and four more like it, answer with a body derived from the request
/// they were sent -- echoing the item back with a new revision date, which is
/// what the real service does and what the read-back tests are about. A fixed
/// body cannot say that.
#[derive(Clone)]
enum ResponseBody {
    Fixed(Vec<u8>),
    FromRequest(Arc<dyn Fn(&Request) -> Vec<u8> + Send + Sync>),
}

impl ResponseBody {
    fn bytes(&self, request: &Request) -> Vec<u8> {
        match self {
            ResponseBody::Fixed(bytes) => bytes.clone(),
            ResponseBody::FromRequest(build) => build(request),
        }
    }
}

/// A registered mock, as the server holds it.
#[derive(Clone)]
struct Registered {
    id: u64,
    method: String,
    target: Target,
    /// Field names lowercased, paired with what their values must match.
    headers: Vec<(String, Matcher)>,
    body: Matcher,
    /// The escape hatch for what no declarative matcher can say. Defaults to
    /// "yes".
    request: Arc<dyn Fn(&Request) -> bool + Send + Sync>,
    status: usize,
    response_headers: Vec<(String, String)>,
    response_body: ResponseBody,
    hits: usize,
    expected_at_least: Option<usize>,
    expected_at_most: Option<usize>,
}

impl Registered {
    fn matches(&self, request: &Request) -> bool {
        self.method == request.method
            && self.target.matches(&request.path_and_query)
            && self
                .headers
                .iter()
                .all(|(field, matcher)| matcher.matches_values(&request.header(field)))
            && self.body.matches_value(&request.utf8_lossy_body())
            && (self.request)(request)
    }

    /// Whether this mock still owes hits. Drives selection when more than one
    /// mock matches: an unsatisfied mock is preferred over a satisfied one, so
    /// a test that registers two answers for one route gets them in order.
    fn is_missing_hits(&self) -> bool {
        match (self.expected_at_least, self.expected_at_most) {
            (Some(_), Some(at_most)) => self.hits < at_most,
            (Some(at_least), None) => self.hits < at_least,
            (None, Some(at_most)) => self.hits < at_most,
            (None, None) => self.hits < 1,
        }
    }

    fn describe(&self) -> String {
        let mut described = format!("{} {}", self.method, self.target);
        for (field, matcher) in &self.headers {
            described.push_str(&format!("\r\n{field}: {matcher}"));
        }
        if self.body != Matcher::Any {
            described.push_str(&format!("\r\nbody: {}", self.body));
        }
        described
    }
}

/// A mock under construction, and -- after [`Mock::create`] -- the handle used
/// to assert on it or take it away again.
///
/// Every builder method takes `self` and returns it, so a mock reads as one
/// expression, exactly as it did under `mockito`.
///
/// NOT `#[must_use]`, and that is deliberate rather than an oversight:
/// seventy-five call sites in this crate end a chain at `.create()` and drop
/// the handle, because they only ever wanted the route answered and have
/// nothing to assert on later. `mockito`'s `Mock` is not `must_use` either.
pub struct Mock {
    state: Arc<Mutex<State>>,
    inner: Registered,
}

impl Mock {
    /// Require the query part to match, which also stops the path matcher from
    /// seeing it. See [`Target`] for why that distinction matters.
    pub fn match_query<M: Into<Matcher>>(mut self, query: M) -> Self {
        let path = match self.inner.target {
            Target::Unified(path) | Target::Split(path, _) => path,
        };
        self.inner.target = Target::Split(path, query.into());
        self
    }

    /// Require a header. Repeating the same field name adds a second
    /// requirement rather than replacing the first.
    pub fn match_header<M: Into<Matcher>>(mut self, field: &str, value: M) -> Self {
        self.inner.headers.push((field.to_ascii_lowercase(), value.into()));
        self
    }

    /// Require the request body to match.
    pub fn match_body<M: Into<Matcher>>(mut self, body: M) -> Self {
        self.inner.body = body.into();
        self
    }

    /// Require an arbitrary predicate over the whole request, for what the
    /// declarative matchers cannot say -- most often that a field is *absent*
    /// from a body, which no matcher over present values can express.
    pub fn match_request<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&Request) -> bool + Send + Sync + 'static,
    {
        self.inner.request = Arc::new(predicate);
        self
    }

    /// The status to answer with. 200 if never called.
    pub fn with_status(mut self, status: usize) -> Self {
        self.inner.status = status;
        self
    }

    /// Add a response header.
    pub fn with_header(mut self, field: &str, value: &str) -> Self {
        self.inner.response_headers.push((field.to_string(), value.to_string()));
        self
    }

    /// The response body. `Content-Length` is set from it.
    pub fn with_body<B: AsRef<[u8]>>(mut self, body: B) -> Self {
        self.inner.response_body = ResponseBody::Fixed(body.as_ref().to_vec());
        self
    }

    /// The response body, built from the request that asked for it. See
    /// [`ResponseBody`] for why a fixed body is not enough for five of this
    /// crate's mocks.
    pub fn with_body_from_request<F>(mut self, build: F) -> Self
    where
        F: Fn(&Request) -> Vec<u8> + Send + Sync + 'static,
    {
        self.inner.response_body = ResponseBody::FromRequest(Arc::new(build));
        self
    }

    /// Expect exactly `hits` requests, for [`Mock::assert`].
    pub fn expect(mut self, hits: usize) -> Self {
        self.inner.expected_at_least = Some(hits);
        self.inner.expected_at_most = Some(hits);
        self
    }

    /// Expect at least `hits` requests, for [`Mock::assert`]. An upper bound
    /// that this would contradict is dropped, as `mockito` drops it.
    pub fn expect_at_least(mut self, hits: usize) -> Self {
        self.inner.expected_at_least = Some(hits);
        if self.inner.expected_at_most.is_some_and(|at_most| at_most < hits) {
            self.inner.expected_at_most = None;
        }
        self
    }

    /// Register the mock. Until this is called the server has never heard of
    /// it, so a builder chain that forgets it answers nothing -- the same
    /// footgun `mockito` has, kept rather than fixed so the call sites read the
    /// same.
    pub fn create(self) -> Self {
        self.lock().mocks.push(self.inner.clone());
        self
    }

    /// Panic unless this mock was hit as many times as it expects: exactly once
    /// by default, `expect(n)` times if that was set, at least `n` if
    /// [`Mock::expect_at_least`] was.
    #[track_caller]
    pub fn assert(&self) {
        let state = self.lock();
        let hits = self.hits_in(&state);
        if self.satisfied_by(hits) {
            return;
        }
        let wanted = match (self.inner.expected_at_least, self.inner.expected_at_most) {
            (Some(at_least), Some(at_most)) if at_least == at_most => format!("exactly {at_least}"),
            (Some(at_least), Some(at_most)) => format!("between {at_least} and {at_most}"),
            (Some(at_least), None) => format!("at least {at_least}"),
            (None, Some(at_most)) => format!("at most {at_most}"),
            (None, None) => "exactly 1".to_string(),
        };
        // The unmatched requests are the whole point of this message: a mock
        // that was not hit is nearly always a mock whose matcher and the
        // client's request disagree about one character, and the two have to be
        // readable side by side to see which.
        let seen = if state.unmatched.is_empty() {
            "no request went unmatched".to_string()
        } else {
            format!("unmatched requests:\r\n  {}", state.unmatched.join("\r\n  "))
        };
        drop(state);
        panic!(
            "expected {wanted} request(s) to the mock\r\n{}\r\nbut got {hits}; {seen}",
            self.inner.describe()
        );
    }

    /// Whether this mock has been hit as many times as it expects, without
    /// panicking if it has not.
    #[must_use]
    pub fn matched(&self) -> bool {
        let state = self.lock();
        let hits = self.hits_in(&state);
        self.satisfied_by(hits)
    }

    /// Take the mock off the server, so later requests to its route fall
    /// through to whatever else matches -- or to the 501.
    pub fn remove(&self) {
        self.lock().mocks.retain(|mock| mock.id != self.inner.id);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // `unwrap_or_else(into_inner)` and not `unwrap`: a test that panics
        // while a connection thread holds this lock would otherwise turn one
        // real failure into every later mock-server test failing for a reason
        // that has nothing to do with what it was checking.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn hits_in(&self, state: &State) -> usize {
        state.mocks.iter().find(|mock| mock.id == self.inner.id).map_or(0, |mock| mock.hits)
    }

    fn satisfied_by(&self, hits: usize) -> bool {
        match (self.inner.expected_at_least, self.inner.expected_at_most) {
            (Some(at_least), Some(at_most)) => hits >= at_least && hits <= at_most,
            (Some(at_least), None) => hits >= at_least,
            (None, Some(at_most)) => hits <= at_most,
            (None, None) => hits == 1,
        }
    }
}

// ---------------------------------------------------------------------------
// the server
// ---------------------------------------------------------------------------

/// Everything the connection threads and the test thread share.
struct State {
    mocks: Vec<Registered>,
    /// Requests no mock answered, kept for [`Mock::assert`]'s message.
    unmatched: Vec<String>,
    next_id: u64,
}

/// A mock HTTP server on loopback, listening until it is dropped.
///
/// Named `Server` so the fixtures that take one -- `fn granted(server: &mut
/// Server)` and its half-dozen siblings -- read as they did.
pub struct Server {
    state: Arc<Mutex<State>>,
    address: SocketAddr,
    /// Cleared on drop, then the acceptor is woken by a connection to itself so
    /// it can notice.
    accepting: Arc<AtomicBool>,
    acceptor: Option<std::thread::JoinHandle<()>>,
}

/// A mock HTTP server, listening on a loopback port the OS chose.
///
/// This is the only place in the crate that stands up a test HTTP server, and
/// [`crate::below_cut`]'s source walk is not what enforces that --
/// `no_module_in_this_crate_builds_a_mockito_server` below is.
#[must_use]
pub fn server() -> Server {
    let state =
        Arc::new(Mutex::new(State { mocks: Vec::new(), unmatched: Vec::new(), next_id: 0 }));
    // Port 0: the OS picks a free one. Asking for a particular port is what
    // makes a test suite fight itself when two runs overlap.
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port for the mock server");
    let address = listener.local_addr().expect("the bound address");
    let accepting = Arc::new(AtomicBool::new(true));

    let acceptor = {
        let (state, accepting) = (Arc::clone(&state), Arc::clone(&accepting));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if !accepting.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let state = Arc::clone(&state);
                // A thread per connection, and it lives for exactly one
                // request: see the module docs on `Connection: close`. Serving
                // connections inline on this thread was the alternative and is
                // wrong -- a client that connects without sending would wedge
                // the acceptor for every later request to this server.
                std::thread::spawn(move || serve_connection(&stream, &state));
            }
        })
    };

    Server { state, address, accepting, acceptor: Some(acceptor) }
}

/// The name three fixtures in `vault_cache` and `rest::backend` spell when
/// they return a server rather than borrow one. It was a distinct type when
/// this module was a gate -- a guard that held a permit and derefed to the
/// real server -- and there is no permit any more, so it is the server itself.
pub type MockServer = Server;

impl Server {
    /// Begin a mock for `method` and `path`. Nothing is registered until
    /// [`Mock::create`].
    pub fn mock<P: Into<Matcher>>(&mut self, method: &str, path: P) -> Mock {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        drop(state);
        Mock {
            state: Arc::clone(&self.state),
            inner: Registered {
                id,
                method: method.to_ascii_uppercase(),
                target: Target::Unified(path.into()),
                headers: Vec::new(),
                body: Matcher::Any,
                request: Arc::new(|_| true),
                status: 200,
                response_headers: Vec::new(),
                response_body: ResponseBody::Fixed(Vec::new()),
                hits: 0,
                expected_at_least: None,
                expected_at_most: None,
            },
        }
    }

    /// The base URL, with no trailing slash: `http://127.0.0.1:PORT`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// `127.0.0.1:PORT`.
    #[must_use]
    pub fn host_with_port(&self) -> String {
        self.address.to_string()
    }

    /// The address the listener is bound to.
    #[must_use]
    pub fn socket_address(&self) -> SocketAddr {
        self.address
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accepting.store(false, Ordering::SeqCst);
        // `accept` is blocking and there is no portable way to interrupt it, so
        // the wake is a connection to ourselves: the acceptor returns from
        // `accept`, reads the cleared flag and leaves. The connection is
        // dropped without a byte written, which `serve_one` reads as EOF.
        let _ = TcpStream::connect(self.address);
        if let Some(acceptor) = self.acceptor.take() {
            // Not `expect`: this runs during unwinding when a test panics, and
            // a second panic there would replace the failure being reported
            // with a useless one.
            let _ = acceptor.join();
        }
    }
}

/// How long a connection may sit idle between requests before this server
/// hangs up on it.
///
/// A ceiling on how long a thread can be parked on a client that has gone away
/// without closing, and nothing else. It is not a wait that any assertion
/// depends on: every response has already been written when the clock starts,
/// and `ureq` retransmits an idempotent request whose *pooled* connection was
/// closed under it, which is exactly the case this creates. A whole `--lib`
/// run is around three minutes, so a minute of silence on a connection means
/// the agent that owned it is gone.
const IDLE_BEFORE_HANGUP: std::time::Duration = std::time::Duration::from_secs(60);

/// Serve requests on one connection until the client closes it.
///
/// **Keep-alive, and that is the load-bearing decision in this file.** The
/// first draft answered one request per connection and hung up, which is
/// simpler and bounds the thread count for free -- and it failed 18 of 240
/// concurrent requests with `os error 10054`, measured, after the close itself
/// had already been made graceful. One connection per request is one
/// ephemeral port per request, and Windows recycles that range through
/// `TIME_WAIT`; a SYN that lands on a port still in `TIME_WAIT` is answered
/// with an RST, which the client reads as a connection reset in the status
/// line. Reusing the connection removes the churn rather than surviving it:
/// `ureq` pools, so a test that makes twenty requests makes one connection.
fn serve_connection(stream: &TcpStream, state: &Mutex<State>) {
    let _ = stream.set_read_timeout(Some(IDLE_BEFORE_HANGUP));
    let mut reader = BufReader::new(stream);
    while let Some(request) = read_request(&mut reader) {
        let close = serve_one(&request, stream, state);
        if close {
            break;
        }
    }
    graceful_close(stream);
}

/// Answer one request. Returns whether the connection must now be closed.
fn serve_one(request: &Request, stream: &TcpStream, state: &Mutex<State>) -> bool {
    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Selection, verbatim from `mockito`: among the mocks that match, the first
    // one still owing hits, else the last one registered. Registration order is
    // what lets a test lay down a catch-all and then override it.
    let chosen = state
        .mocks
        .iter()
        .position(|mock| mock.matches(request) && mock.is_missing_hits())
        .or_else(|| state.mocks.iter().rposition(|mock| mock.matches(request)));

    // A client that asked to close gets `Connection: close` back and the
    // connection after it; anything else is kept alive, which is what stops
    // this server burning an ephemeral port per request.
    let close = request
        .header("connection")
        .iter()
        .any(|value| value.eq_ignore_ascii_case("close"));

    let answer = match chosen {
        Some(at) => {
            let mock = &mut state.mocks[at];
            mock.hits += 1;
            // Wrapped for the reason [`render`] states: this is a copy of the
            // response body on a thread the test that asked for it does not
            // control, and the crate's allocator probe watches every thread.
            //
            // What this covers is the copy itself. A [`ResponseBody::Fixed`]
            // body is cloned into an exactly-sized buffer, so there is nothing
            // else; a [`ResponseBody::FromRequest`] body is built by a closure
            // this module does not own, and whatever that closure grows through
            // on its way to a `Vec` is the closure's. No such mock carries a
            // probe today, and one that did would be back to the same race.
            let body = Zeroizing::new(mock.response_body.bytes(request));
            render(mock.status, &mock.response_headers, &body, close)
        }
        None => {
            state.unmatched.push(request.summary());
            // 501, with no body, exactly as `mockito` answers an unmatched
            // request. Tests across four modules read this status as "the
            // client did not send what I said it would".
            render(501, &[], &[], close)
        }
    };
    drop(state);

    let mut stream = stream;
    if stream.write_all(&answer).is_err() {
        return true;
    }
    let _ = stream.flush();
    close
}

/// Close a connection the way Winsock requires if the response is to survive
/// it.
///
/// This is where every `os error 10054` in this module was, and it took three
/// measured attempts to get right, so all three are written down:
///
/// * `shutdown(Both)` -- `SD_RECEIVE` as well as `SD_SEND` -- arms the socket
///   to answer anything that arrives afterwards with an RST, and an RST
///   **discards the send buffer**, response included. Measured: 69 of 240
///   concurrent requests failed with the identical symptom this module was
///   written to remove, from a listener that has nothing to do with `mockito`.
/// * `shutdown(Write)` alone, then dropping the socket, is no better: the drop
///   is a `closesocket` while the peer's FIN has not arrived, which Winsock
///   also completes abortively. Measured: 82 of 240.
/// * `shutdown(Write)`, then **read to EOF**, then drop. The read returns 0
///   once the client has seen the response and closed its own side, and by
///   then `closesocket` has nothing to abort. This one.
///
/// The read is not a wait dressed up as a fix: it is the FIN handshake, and it
/// carries a deadline only so that a client which never closes cannot park
/// this thread for the life of the process. Nothing about a test's outcome
/// depends on the deadline being generous -- a response has already been
/// written when this is called.
fn graceful_close(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut discard = [0_u8; 1024];
    let mut stream = stream;
    while matches!(stream.read(&mut discard), Ok(read) if read > 0) {}
}

/// Parse one HTTP/1.1 request, body included. `None` at a clean EOF -- which is
/// what the drop wake and a closed keep-alive connection both look like -- or
/// on a head this server cannot read.
fn read_request(reader: &mut BufReader<&TcpStream>) -> Option<Request> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().split(' ');
    let method = parts.next()?.to_ascii_uppercase();
    let path_and_query = parts.next()?.to_string();

    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (field, value) = line.split_once(':')?;
        headers.push((field.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    let value_of = |name: &str| {
        headers.iter().find(|(field, _)| field == name).map(|(_, value)| value.as_str())
    };
    let chunked =
        value_of("transfer-encoding").is_some_and(|coding| coding.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        read_chunked(reader)?
    } else {
        let length: usize =
            value_of("content-length").and_then(|len| len.parse().ok()).unwrap_or(0);
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body).ok()?;
        body
    };

    Some(Request { method, path_and_query, headers, body })
}

/// Reassemble a `Transfer-Encoding: chunked` body.
///
/// `ureq` sends `Content-Length` for every body this crate gives it, so this
/// path is not exercised by the suite today. It is here because the alternative
/// -- reading a chunked body as zero bytes -- would not fail, it would make
/// every body matcher on such a request quietly compare against nothing.
fn read_chunked(reader: &mut BufReader<&TcpStream>) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let size = usize::from_str_radix(header.trim_end().split(';').next()?.trim(), 16).ok()?;
        if size == 0 {
            // The trailer section, then the final blank line.
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).ok()? == 0 {
                    break;
                }
                if line.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            return Some(body);
        }
        let mut chunk = vec![0_u8; size];
        reader.read_exact(&mut chunk).ok()?;
        body.extend_from_slice(&chunk);
        let mut terminator = [0_u8; 2];
        reader.read_exact(&mut terminator).ok()?;
    }
}

/// Serialise a response.
///
/// `Content-Length` is always present, because it is what lets the client frame
/// the body and read the next response off the same connection.
///
/// # Why the rendered response is [`Zeroizing`], and why the body is reserved
///
/// **A mock body is a test's plaintext, and this server serves it from a thread
/// that test does not control.** The crate's allocator probe
/// ([`crate::login_ui::password_lifetime_tests`]) scans **every** thread's
/// frees while any window is armed, deliberately: a probe verdict that ignored
/// other threads would read clean while blind. So a connection thread that
/// frees a probe-bearing response buffer while a test has a window open puts
/// bytes that are nobody's leak onto that test's global channel.
///
/// That was not hypothetical. `vault_bridge::tests::`
/// `generate_hands_back_a_password_that_does_not_reach_the_allocator_in_the_clear`
/// failed about half of full-suite runs on exactly this: instrumenting the
/// probe's hits by thread showed **two** hits per response from this thread --
/// the body clone in `serve_one` and this rendered buffer -- and under load one
/// of them landed after the client had its answer and the test had opened the
/// window that measures the value alone. The test was right to refuse a verdict
/// it could not honour; the bytes it was refusing over were this server's.
///
/// The answer is at the source rather than in any verdict: this thread does not
/// release a response body in the clear at all. The wipe covers the buffer that
/// is dropped, so the body must not be written into a buffer that then grows --
/// the `reserve` below is the other half of the same fix, not a speed tweak.
fn render(
    status: usize,
    headers: &[(String, String)],
    body: &[u8],
    close: bool,
) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(format!("HTTP/1.1 {status} {}\r\n", reason(status)).into_bytes());
    for (field, value) in headers {
        out.extend_from_slice(format!("{field}: {value}\r\n").as_bytes());
    }
    // Only when the mock did not state one itself: two `Content-Length` headers
    // is a malformed response, and a test that sets one is saying something
    // deliberate about it.
    if !headers.iter().any(|(field, _)| field.eq_ignore_ascii_case("content-length")) {
        out.extend_from_slice(format!("content-length: {}\r\n", body.len()).as_bytes());
    }
    // Only when the client asked for it. A `Connection: close` on every
    // response is what made this server burn an ephemeral port per request --
    // see the module docs' table.
    if close {
        out.extend_from_slice(b"connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
    // Room for the body BEFORE a byte of it is written. Everything above is
    // head, which never carries a test's secret; the moment the body goes in,
    // a `Vec` that has to grow hands its old buffer -- plaintext and all --
    // back to the allocator, and the wipe on THIS buffer's drop cannot reach
    // a buffer that is already gone.
    out.reserve(body.len());
    out.extend_from_slice(body);
    out
}

/// The reason phrase. Clients here read the code and ignore this, but a status
/// line without one is malformed, and every code this crate's mocks actually
/// answer with is named rather than left to the fallback.
fn reason(status: usize) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client these tests drive the server with.
    ///
    /// [`crate::http_agent::bounded_total`] rather than a bare `ureq` agent,
    /// for two reasons and only incidentally because
    /// `http_agent::bare_ureq_calls_are_confined_to_this_module` insists: it is
    /// the agent every production caller in this crate uses, so these tests
    /// exercise the same client as the 209 call sites, and it **pools**, which
    /// is the half of the fix that lives on the client side.
    fn agent() -> crate::http_agent::TotalBounded {
        crate::http_agent::bounded_total(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(30),
        )
    }

    /// Either the answer's REASON PHRASE or the transport failure.
    ///
    /// A transport failure is what this whole module exists to stop producing,
    /// so it is carried as a value rather than unwrapped: the concurrency test
    /// below has to *count* them.
    ///
    /// The phrase and not the code, which reads oddly and is not a preference:
    /// `job_object`'s
    /// `the_two_job_bearing_modules_can_start_a_child_only_through_this_one`
    /// forbids calling `status()` on a receiver in EVERY file of this crate
    /// -- and its line pass reads unstripped source, so this comment may not
    /// spell that call either,
    /// because `std::process::Command::status` starts a child outside
    /// `spawn_in_job` and no substring test can tell that call from
    /// `ureq::Response::status`. That guard is right to be blunt -- the thing
    /// it protects is a `bw` process holding an unlocked vault -- so this
    /// reads `status_text` instead. It costs nothing: [`reason`] below maps
    /// each code to exactly one phrase, so the two carry the same
    /// information, and the assertions now pin `reason` as well.
    fn status_of(result: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
        match result {
            Ok(response) | Err(ureq::Error::Status(_, response)) => {
                Ok(response.status_text().to_string())
            }
            Err(ureq::Error::Transport(transport)) => Err(transport.to_string()),
        }
    }

    /// `Ok("OK")` and friends, so the assertions below read as one line.
    fn phrase(status: usize) -> Result<String, String> {
        Ok(reason(status).to_string())
    }

    /// Make one GET on a throwaway agent.
    fn get(url: &str) -> Result<String, String> {
        status_of(agent().get(url).call())
    }

    /// Make one request with a method and a body, and report the status.
    fn send(method: &str, url: &str, body: &str) -> Result<String, String> {
        let agent = agent();
        let request = match method {
            "POST" => agent.post(url),
            "PUT" => agent.put(url),
            other => panic!("this helper knows POST and PUT, not {other}"),
        };
        status_of(request.send_string(body))
    }

    /// **The measurement this module was written for**, kept as a test.
    ///
    /// Eight threads, each standing up its own server and driving thirty
    /// requests through it: 240 requests with up to eight in flight. Against
    /// `mockito` that shape failed 14% of the time with `os error 10054` --
    /// 57 of 400 at four threads, measured, and this runs at eight. Against
    /// this listener it must be zero, and the count of answered requests is
    /// the positive control that the zero is not the zero of a loop that never
    /// ran.
    ///
    /// Not a timing test and not a sleep anywhere in it: it asserts on counts,
    /// so a loaded machine makes it slower and never makes it flake.
    #[test]
    fn concurrent_traffic_is_answered_without_a_single_transport_failure() {
        const THREADS: usize = 8;
        const REQUESTS: usize = 30;

        let mut handles = Vec::new();
        for _ in 0..THREADS {
            handles.push(std::thread::spawn(|| {
                let mut server = server();
                let mock = server.mock("GET", "/ping").with_body("pong").expect(REQUESTS).create();
                let url = format!("{}/ping", server.url());
                let mut answered = 0_usize;
                let mut failures = Vec::new();
                for _ in 0..REQUESTS {
                    match get(&url) {
                        Ok(answer) if answer == "OK" => answered += 1,
                        Ok(other) => failures.push(format!("status {other}")),
                        Err(transport) => failures.push(transport),
                    }
                }
                mock.assert();
                (answered, failures)
            }));
        }

        let mut answered = 0_usize;
        let mut failures: Vec<String> = Vec::new();
        for handle in handles {
            let (ok, failed) = handle.join().expect("no thread panicked");
            answered += ok;
            failures.extend(failed);
        }

        assert!(
            failures.is_empty(),
            "{} of {} concurrent requests did not get a clean 200: {:?}. That is the \
             `os error 10054` class of failure this module replaced `mockito` to remove; a \
             retry or a sleep here would hide it rather than fix it",
            failures.len(),
            THREADS * REQUESTS,
            failures
        );
        assert_eq!(
            answered,
            THREADS * REQUESTS,
            "control: every request must actually have been made and answered, or the \
             emptiness of the failure list above means nothing"
        );
    }

    /// A request no mock matches is answered **501**, and that is not an
    /// accident of this implementation -- tests in `vault_bridge`,
    /// `vault_cache`, `updater` and `rest::backend` read that status as "the
    /// client sent something other than what I said it would".
    ///
    /// The 200 is the positive control: the same server, one route away.
    #[test]
    fn an_unmatched_request_is_answered_501_and_a_matched_one_is_not() {
        let mut server = server();
        let _matched = server.mock("GET", "/known").with_status(200).create();

        assert_eq!(get(&format!("{}/known", server.url())), phrase(200), "control: the route that exists");
        assert_eq!(
            get(&format!("{}/unknown", server.url())),
            phrase(501),
            "an unmatched route must be a 501, not a 404 and not a hang"
        );
    }

    /// A mock built with a bare path does **not** answer a request that
    /// carries a query, and one built with `match_query` does. That pair is
    /// what lets `vault_cache` tell `?trash=true` apart from the live listing
    /// without either mock having to exclude the other by hand.
    #[test]
    fn a_bare_path_does_not_match_a_query_but_a_split_matcher_does() {
        let mut server = server();
        let bare = server.mock("GET", "/items").with_status(200).expect(1).create();

        assert_eq!(
            get(&format!("{}/items?trash=true", server.url())),
            phrase(501),
            "a path matcher sees the query too, so `/items` must not answer `/items?trash=true`"
        );
        assert_eq!(get(&format!("{}/items", server.url())), phrase(200), "control: without the query it matches");
        bare.assert();

        let split = server
            .mock("GET", "/items")
            .match_query(Matcher::Exact("trash=true".into()))
            .with_status(204)
            .create();
        assert_eq!(
            get(&format!("{}/items?trash=true", server.url())),
            phrase(204),
            "`match_query` splits path from query, so this one must answer"
        );
        split.assert();
    }

    /// `Matcher::Missing` on a query means "there was none", which is a
    /// different statement from `Matcher::Any`.
    #[test]
    fn missing_matches_an_absent_query_and_any_matches_a_present_one() {
        let mut server = server();
        let absent = server
            .mock("GET", "/x")
            .match_query(Matcher::Missing)
            .with_status(200)
            .expect(1)
            .create();
        let present =
            server.mock("GET", "/x").match_query(Matcher::Any).with_status(204).expect(1).create();

        assert_eq!(get(&format!("{}/x", server.url())), phrase(200));
        assert_eq!(get(&format!("{}/x?a=1", server.url())), phrase(204));
        absent.assert();
        present.assert();
    }

    /// The declarative body matchers, each with the body that must not match
    /// it beside the one that must -- a matcher that accepted everything would
    /// pass the positive half alone.
    #[test]
    fn the_body_matchers_accept_what_they_name_and_refuse_what_they_do_not() {
        let cases: Vec<(&str, Matcher, &str, &str)> = vec![
            ("exact", Matcher::Exact("hello".into()), "hello", "hello "),
            ("regex", Matcher::Regex("to[kK]en=[0-9]+".into()), "x&toKen=123", "x&toKen=abc"),
            (
                "json",
                Matcher::Json(serde_json::json!({"a": 1, "b": 2})),
                r#"{"b":2,"a":1}"#,
                r#"{"a":1,"b":2,"c":3}"#,
            ),
            (
                "partial json",
                Matcher::PartialJson(serde_json::json!({"a": 1})),
                r#"{"a":1,"b":2}"#,
                r#"{"a":2,"b":1}"#,
            ),
            (
                "urlencoded",
                Matcher::UrlEncoded("scope".into(), "api offline_access".into()),
                "grant_type=password&scope=api+offline_access",
                "grant_type=password&scope=api",
            ),
            (
                "all of",
                Matcher::AllOf(vec![
                    Matcher::UrlEncoded("a".into(), "1".into()),
                    Matcher::UrlEncoded("b".into(), "2".into()),
                ]),
                "a=1&b=2",
                "a=1&b=3",
            ),
            (
                "any of",
                Matcher::AnyOf(vec![
                    Matcher::Exact("left".into()),
                    Matcher::Exact("right".into()),
                ]),
                "right",
                "middle",
            ),
            ("missing", Matcher::Missing, "", "anything"),
            // A body that is not JSON at all must be a miss, not a panic in
            // the connection thread -- see the module docs' one divergence
            // from `mockito`.
            ("json over junk", Matcher::Json(serde_json::json!({"a": 1})), r#"{"a":1}"#, "not json"),
        ];

        for (name, matcher, accepted, refused) in cases {
            let mut server = server();
            let mock =
                server.mock("POST", "/b").match_body(matcher).with_status(200).expect(1).create();
            let url = format!("{}/b", server.url());
            assert_eq!(send("POST", &url, accepted), phrase(200), "the {name} matcher refused {accepted:?}");
            assert_eq!(send("POST", &url, refused), phrase(501), "the {name} matcher accepted {refused:?}");
            mock.assert();
        }
    }

    /// A header matcher requires the header; `Missing` requires its absence.
    /// `ureq` sends the field name lowercased and the tests spell it
    /// `Authorization`, so the case-insensitive lookup is not cosmetic.
    #[test]
    fn header_matchers_read_the_field_case_insensitively_and_missing_means_absent() {
        let mut server = server();
        let wanted = server
            .mock("GET", "/h")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .expect(1)
            .create();
        let url = format!("{}/h", server.url());

        assert_eq!(
            status_of(agent().get(&url).set("authorization", "Bearer AT-1").call()),
            phrase(200),
            "a header spelled in a different case is the same header"
        );
        assert_eq!(get(&url), phrase(501), "control: without the header the mock must not answer");
        wanted.assert();

        // `super::server` because the binding above shadows the free function
        // for the rest of this body.
        let mut server = super::server();
        let absent = server
            .mock("GET", "/h")
            .match_header("Authorization", Matcher::Missing)
            .with_status(204)
            .expect(1)
            .create();
        let url = format!("{}/h", server.url());
        assert_eq!(get(&url), phrase(204));
        assert_eq!(
            status_of(agent().get(&url).set("Authorization", "Bearer AT-1").call()),
            phrase(501),
            "control: `Missing` must refuse the header when it is there"
        );
        absent.assert();
    }

    /// When two mocks match, the one still owing hits answers first and the
    /// last-created one answers after both are satisfied. `vault_cache`'s
    /// read-back tests depend on exactly this: two answers for one route, in
    /// order.
    #[test]
    fn an_unsatisfied_mock_answers_before_a_satisfied_one_and_the_last_created_answers_after() {
        let mut server = server();
        let first = server.mock("GET", "/s").with_status(200).expect(1).create();
        let second = server.mock("GET", "/s").with_status(204).expect(1).create();
        let url = format!("{}/s", server.url());

        assert_eq!(get(&url), phrase(200), "the first mock still owed a hit");
        assert_eq!(get(&url), phrase(204), "the second still owed one");
        assert_eq!(get(&url), phrase(204), "both are satisfied, so the last created one answers");
        first.assert();
        assert!(
            !second.matched(),
            "the third request went to the last-created mock, taking it to two hits against \
             an `expect(1)`. That is the observation: overflow lands on the last mock rather \
             than on the first or on the 501"
        );
    }

    /// `assert` with no expectation stated means exactly one hit -- not "at
    /// least one". A mock hit twice must fail it, or every `expect_at_least`
    /// in the crate would be saying nothing.
    #[test]
    fn assert_defaults_to_exactly_one_hit() {
        let mut server = server();
        let mock = server.mock("GET", "/once").create();
        let url = format!("{}/once", server.url());

        assert!(!mock.matched(), "control: a mock nothing has hit is not satisfied");
        assert_eq!(get(&url), phrase(200));
        assert!(mock.matched(), "one hit satisfies the default expectation");
        assert_eq!(get(&url), phrase(200));
        assert!(!mock.matched(), "two hits do not: the default is exactly one, not at least one");
    }

    /// `expect(0)` says the route must never be reached, and `expect_at_least`
    /// puts a floor under the count with no ceiling.
    #[test]
    fn expect_pins_both_ends_and_expect_at_least_pins_only_the_floor() {
        let mut server = server();
        let never = server.mock("PUT", "/never").expect(0).create();
        let twice = server.mock("GET", "/many").expect_at_least(2).create();
        let url = format!("{}/many", server.url());

        assert!(never.matched(), "an untouched `expect(0)` is satisfied");
        assert!(!twice.matched(), "control: a floor of two is not met by nothing");
        assert_eq!(get(&url), phrase(200));
        assert!(!twice.matched(), "nor by one");
        assert_eq!(get(&url), phrase(200));
        assert!(twice.matched(), "two meets a floor of two");
        assert_eq!(get(&url), phrase(200));
        assert!(twice.matched(), "and three still does: `expect_at_least` sets no ceiling");

        assert_eq!(send("PUT", &format!("{}/never", server.url()), ""), phrase(200));
        assert!(!never.matched(), "a route that said it would never be reached was reached");
    }

    /// `remove` takes the mock off the server, so its route goes back to being
    /// unmatched. The 200 before it is the control.
    #[test]
    fn a_removed_mock_stops_answering() {
        let mut server = server();
        let mock = server.mock("GET", "/gone").with_status(200).create();
        let url = format!("{}/gone", server.url());

        assert_eq!(get(&url), phrase(200), "control: it answers while it is registered");
        mock.remove();
        assert_eq!(get(&url), phrase(501), "a removed mock must not answer");
    }

    /// Response status, headers and body all reach the client, and the body is
    /// framed by a `Content-Length` this server writes -- a response whose
    /// length is wrong reads as a truncated or a hanging one, not as a failed
    /// assertion.
    #[test]
    fn the_status_headers_and_body_all_arrive() {
        let mut server = server();
        let _mock = server
            .mock("GET", "/full")
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create();

        let response =
            agent().get(&format!("{}/full", server.url())).call().expect("a response");
        assert_eq!(response.status_text(), "Created");
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert_eq!(response.into_string().expect("a body"), r#"{"ok":true}"#);
    }

    /// A body is read to its end before any matcher sees it, whatever its
    /// size. A server that answered from a partially read socket would leave
    /// the rest unread, and closing on unread data is exactly the RST this
    /// module exists to stop producing.
    #[test]
    fn a_large_body_arrives_whole() {
        let mut server = server();
        let body = "x".repeat(300_000);
        let mock = server
            .mock("POST", "/big")
            .match_body(Matcher::Exact(body.clone()))
            .expect(1)
            .create();

        assert_eq!(send("POST", &format!("{}/big", server.url()), &body), phrase(200));
        mock.assert();
    }

    /// A server that is dropped stops listening, and its acceptor thread is
    /// joined rather than left running. Without the join the suite would
    /// accumulate one parked thread per test that ever built a server.
    ///
    /// Observed by the port going dead, which is a fact about the OS rather
    /// than about a timer, so no sleep is involved.
    #[test]
    fn a_dropped_server_stops_answering_on_its_port() {
        let address = {
            let mut server = server();
            let _mock = server.mock("GET", "/alive").with_status(200).create();
            assert_eq!(
                get(&format!("{}/alive", server.url())),
                phrase(200),
                "control: it answers while it is alive"
            );
            server.socket_address()
        };

        assert!(
            get(&format!("http://{address}/alive")).is_err(),
            "the listener outlived its `Server`"
        );
    }

    /// The half of this module that no type can carry: this crate's library
    /// must not stand up a `mockito` server anywhere, because `mockito` is the
    /// defect. One left behind would not go red -- it would just flake again,
    /// somewhere else, next week.
    ///
    /// The needle is SPLIT ACROSS `concat!` ARGUMENTS so it cannot match its
    /// own declaration, and the control below is what keeps that honest.
    /// `http_agent.rs` has shipped a dead guard of exactly this shape before;
    /// see its note.
    ///
    /// **`main.rs` USED TO BE EXEMPT, AND IS NOT ANY MORE.** The exemption was
    /// a limit -- `main.rs` is a *different crate* that reaches this one
    /// through `deskwarden::`, so a `#[cfg(test)]` module of the lib was not
    /// visible to it at all -- and it rested on a claim that was measured
    /// FALSE: that its tests "run in their own process, one at a time against
    /// their own servers, where the concurrency that provokes the reset is not
    /// there". Measured on the machine in this module's docs, a single one of
    /// those tests, alone in a fresh process at `--test-threads=1` with one
    /// server and four requests, failed **5 runs out of 15** with the same
    /// `os error 10054`. `mockito` does not need this crate's concurrency to
    /// reset a connection; it only needs a request. `lib.rs`'s `test-support`
    /// feature is what removed the limit, and with it the exemption.
    ///
    /// So the walk now covers every file, `main.rs` included, and the control
    /// moved with it: with nothing in the tree left to match, the needle is
    /// checked against a string built here instead.
    #[test]
    fn no_module_in_this_crate_builds_a_mockito_server() {
        const NEEDLE: &str = concat!("mockito::Server:", ":new");
        // THE CONTROL, and it has to be synthetic now: the needle used to be
        // proved live against `main.rs`, the last file that matched it. A
        // needle that matches nothing anywhere passes this test forever while
        // asserting nothing, which is exactly what happened to `http_agent`'s
        // dead guard; see its note.
        assert!(
            format!("let mut server = {}::Server{}new();", "mockito", "::").contains(NEEDLE),
            "needle {NEEDLE:?} does not match the call it bans, so this test has been passing \
             over nothing"
        );

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
        assert!(
            files.iter().any(|(path, _)| path == "main.rs"),
            "control: the walk did not reach `main.rs`, the file that used to be exempt here"
        );

        let offenders: Vec<&String> = files
            .iter()
            .filter(|(_, text)| text.contains(NEEDLE))
            .map(|(path, _)| path)
            .collect();

        assert!(
            offenders.is_empty(),
            "these files stand up a `mockito` server: {offenders:?}. `mockito` 1.7.2 resets \
             accepted connections -- the `os error 10054` this module was written to remove, \
             and it does it to a lone sequential test as readily as to a parallel suite. Call \
             `test_http::server()` instead. `mockito` is not a dependency of this crate any \
             more, so such a call does not even compile; this walk is what keeps it from \
             coming back with the dependency"
        );
    }
}

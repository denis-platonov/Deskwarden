//! Breach lookup against Have I Been Pwned's k-anonymity range API.
//!
//! # SHA-1 here is NOT a security control and must not be "upgraded"
//!
//! It is Have I Been Pwned's index: the k-anonymity range API is keyed on
//! SHA-1(password) and nothing else matches it. Swapping in SHA-256 would
//! make every lookup return "not breached" -- a silent, reassuring lie. A
//! future reader who "hardens" this will not get an error; they will get a
//! feature that says every password is fine.
//!
//! # What leaves this machine
//!
//! The password is hashed locally. Only the **first five hex characters** of
//! that hash are ever put on the wire, as the path of
//! `GET /range/{PREFIX5}`. The server answers with every suffix it knows
//! under that prefix -- typically 500-1000 `SUFFIX35:COUNT` lines -- and the
//! remaining 35 characters are matched here, locally. The password, and any
//! part of its hash beyond those five characters, never leaves the machine.
//!
//! Two things hold that up, and neither is a convention:
//!
//! * [`Prefix`] is a newtype with a private field that only [`split_hash`]
//!   can build, and [`check_prefix`] takes a `&Prefix`. A call site that
//!   passed the whole 40-character hex where the prefix belongs does not
//!   compile. Same pattern, same reason, as [`crate::http_agent::TotalBounded`].
//! * `the_request_head_carries_nothing_beyond_the_allowlist` reads the
//!   literal request bytes off a socket and matches them against an exact
//!   allowlist -- request line and every header, by name and by value. A
//!   header nobody anticipated fails closed rather than sailing through.
//!
//! The request also sets `Add-Padding: true`, HIBP's own hardening for this
//! API: the response is bulked out with decoy suffixes so its *size* does not
//! identify the bucket to anyone watching the encrypted stream. The decoys
//! carry a count of zero and [`parse_range_body`] skips them.
//!
//! # What is NOT wiped, stated plainly
//!
//! `sha1 0.10` has no `zeroize` support. `Sha1::digest(password.as_bytes())`
//! copies the plaintext into the hasher's 64-byte block buffer and returns a
//! `GenericArray<u8, 20>` holding the complete hash, and **both are dropped
//! without being wiped**. Both are stack-resident, so no `realloc` hands
//! them back and the allocator probe in
//! `hashing_a_password_does_not_release_it_to_the_allocator` is blind to
//! them by construction -- which is why this paragraph exists instead of a
//! claim that the source is wiped. Closing it needs a hash implementation
//! that zeroizes; nothing in this file can.
//!
//! What *is* guaranteed: no heap copy of the plaintext is made here, and
//! every buffer this module owns -- the digest bytes, the hex, the suffix --
//! is `Zeroizing` at its exact final capacity, so it never reallocs and is
//! wiped on drop.
//!
//! # No line of THIS FILE logs, and that is exactly the claim
//!
//! `the_breach_module_never_logs` reads *this file's* production source and
//! nothing else. So the guarantee is: **no code in `breach.rs` writes a log
//! macro, a print macro, a `dbg!` or a data-carrying panic, and the hex is
//! never formatted into any message.** It is file-scoped, and it is worth
//! saying plainly that a file-scoped guard is not a claim about the hash's
//! whole lifetime -- it cannot be, because a guard that reads one file cannot
//! see another.
//!
//! What narrows the gap, stated exactly: `hex_digest` is **private**, so the
//! whole 40 characters exist as one string only inside this file, and every
//! line that can hold them is a line the guard reads.
//!
//! What the gap still is, stated equally exactly: [`split_hash`] is `pub` and
//! returns the 35-character suffix as a `Zeroizing<String>`. A future caller
//! elsewhere in the crate could hold it, and could log it, and no guard in
//! this file would see that. Today there is no such caller --
//! `the_whole_hash_is_not_reachable_outside_this_module` is only about
//! `hex_digest`, and it is not a claim about the suffix. The honest summary
//! is: this file does not log, and this file is the only place the *complete*
//! hash is assembled.
//!
//! The needles are the bare macro tokens rather than fully-qualified paths:
//! `use log::debug;` is the ordinary spelling and a path-only guard never
//! sees it.

use crate::http_agent::TotalBounded;
use eframe::egui;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::fmt::Write as _;
use std::time::Duration;
use zeroize::Zeroizing;

/// The production endpoint. Pinned by
/// `the_production_endpoint_is_the_https_range_api`; `check_prefix` takes it
/// as a parameter so tests can point it at a local mock instead.
pub const HIBP_RANGE_BASE: &str = "https://api.pwnedpasswords.com/range";

/// How long to wait for a TCP connection to establish before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total-time bound for one range lookup.
///
/// The response is a few tens of kilobytes of text, so *total* elapsed time is
/// the right shape (see [`crate::http_agent`]): a lookup that has not finished
/// by now is a broken path, not a slow one, and a breach badge that arrives
/// after the user has moved on is worth nothing.
const DEADLINE: Duration = Duration::from_secs(20);

/// HIBP asks callers to identify themselves. Nothing else in this crate sets
/// a User-Agent, so this is deliberately the only one.
///
/// It carries the app name and version and **nothing about the user or the
/// password** -- a User-Agent is a header on the same request as the prefix,
/// so anything put here is something that left the machine.
const USER_AGENT: &str = concat!("Deskwarden/", env!("CARGO_PKG_VERSION"));

/// What a breach lookup can say about one password.
///
/// **There is deliberately no `impl Default`.** A `Default` would have to
/// pick one of these, and every candidate is wrong: `unwrap_or_default()` on
/// a missing status would turn "we could not check" into `Safe` -- a green
/// badge on a password nobody looked at -- or into `Breached`, which cries
/// wolf. Callers must name the variant they mean at every site, and a site
/// that has no answer yet says `Pending` or `Unavailable` out loud.
///
/// `Breached` carries a `u64` and nothing else: no hash, no suffix, no
/// password. `Debug` is safe here for exactly that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreachStatus {
    /// Asked, no answer yet.
    Pending,
    /// Answered, and the suffix was not in the range.
    Safe,
    /// Answered, and the suffix was in the range this many times. Never 0 --
    /// see [`parse_range_body`].
    Breached(u64),
    /// Could not be answered: no network, a non-200, an unreadable body, or a
    /// body that did not parse. Explicitly **not** `Safe`.
    Unavailable,
}

/// A response body with no well-formed `SUFFIX35:COUNT` line in it at all.
///
/// Carries a `&'static str` category and never any data: no hash, no suffix,
/// no fragment of the body. `Debug` is safe for that reason -- there is
/// nothing in here that came from the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed(&'static str);

impl Malformed {
    /// A fixed category, for a caller that wants to distinguish the two
    /// reasons. Never contains data.
    pub fn category(&self) -> &'static str {
        self.0
    }
}

/// Length of the hex prefix that is allowed to leave this machine.
const PREFIX_LEN: usize = 5;

/// Length of the hex suffix that is not.
const SUFFIX_LEN: usize = 35;

/// The SHA-1 hash of `password`, as uppercase hex, in a wiped-on-drop buffer.
///
/// **Private, and deliberately so.** This was `pub(crate)` and had no caller
/// outside [`split_hash`]; what it had instead was a use as an oracle. Any
/// module in the crate could ask it for the complete 40 characters and then
/// hand them to something that puts text on a wire. The types in this file
/// exist to make that impossible to spell, and a crate-visible function that
/// returns the whole hash is the plainest way around them. There is now no
/// way to obtain the other 35 characters outside this module except as the
/// `Zeroizing` suffix `split_hash` returns, which only ever gets compared
/// against text that came back.
///
/// `password` is borrowed -- it is the `Zeroizing<String>` that already lives
/// in `LoginData` (`vault_bridge.rs`) and **no heap copy of it is made here**;
/// `Sha1::digest` reads the borrow directly.
///
/// It is not true that no copy at all is made. `sha1 0.10` has no `zeroize`
/// support: the plaintext goes into the hasher's 64-byte block buffer and the
/// hash comes back in a `GenericArray<u8, 20>`, and neither is wiped when it
/// drops. Both are stack-resident, so the allocator probe cannot see them --
/// see the module docs. This function's own buffers are the part it can
/// guarantee, and does.
///
/// The `String` is built at `with_capacity(40)` and exactly 40 characters are
/// pushed into it. That is load-bearing, not tidiness: a `String` that grows
/// reallocs, and `realloc` hands the old block -- holding a partial hash --
/// back to the allocator without wiping it. Pre-reserved capacity means zero
/// reallocs and exactly one wipe, on drop. `the_hex_buffer_never_reallocates`
/// pins the capacity at both ends.
fn hex_digest(password: &str) -> Zeroizing<String> {
    // Stack array, wiped on drop.
    let mut digest = Zeroizing::new([0u8; 20]);
    digest.copy_from_slice(Sha1::digest(password.as_bytes()).as_slice());

    let mut hex = Zeroizing::new(String::with_capacity(PREFIX_LEN + SUFFIX_LEN));
    for byte in digest.iter() {
        // Infallible for a `String`; 40 chars into 40 reserved bytes, so the
        // buffer is never grown.
        let _ = write!(&mut *hex, "{byte:02X}");
    }
    hex
}

/// The five hex characters of a password's SHA-1 that may leave this machine.
///
/// **The field is private and [`split_hash`] is the only thing that builds
/// one.** That is the whole point of the type. `check_prefix` used to take
/// `prefix: &str` next to `suffix: &Zeroizing<String>` -- two bare strings,
/// where a call site that passed the full 40-character hex would have put the
/// entire hash in the URL with no test anywhere going red, because the
/// privacy test builds its own inputs from `split_hash` and never sees a real
/// call site. With this type that call does not compile.
///
/// `crate::http_agent::TotalBounded` is the same pattern, invented in this
/// crate for the same class of failure: state the invariant as a type and the
/// compiler enforces it at every site, including the ones not written yet.
///
/// The inner `String` is deliberately **not** `Zeroizing`. The prefix is the
/// one part of this that is legitimately published, and dressing it up as a
/// secret would misdescribe what k-anonymity actually protects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix(String);

impl Prefix {
    /// The five characters, for interpolation into the range URL.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// SHA-1 the password and split the uppercase hex into the 5 chars that may
/// leave this machine and the 35 that may not.
///
/// This is the **only** constructor of a [`Prefix`] anywhere, and it can only
/// build one out of `hex[..PREFIX_LEN]`. The suffix is `Zeroizing<String>` at
/// exactly its final capacity, for the same reason the hex is.
pub fn split_hash(password: &str) -> (Prefix, Zeroizing<String>) {
    let hex = hex_digest(password);

    let prefix = Prefix(hex[..PREFIX_LEN].to_string());
    let mut suffix = Zeroizing::new(String::with_capacity(SUFFIX_LEN));
    suffix.push_str(&hex[PREFIX_LEN..]);

    (prefix, suffix)
}

/// Look `suffix` up in a `SUFFIX35:COUNT` range body. Pure.
///
/// * `Ok(Some(count))` -- the suffix is in the range, `count` times.
/// * `Ok(None)` -- the body had well-formed lines and this suffix was not
///   among them.
/// * `Err(Malformed)` -- the body had **no** structurally well-formed line at
///   all.
///
/// The third case is why this returns a `Result` rather than an `Option`. An
/// empty 200, an HTML error page served with a 200, a truncated body: all of
/// them contain no matching suffix, and all of them would read as "not
/// breached" if absence were the only signal. `Malformed` means
/// **structurally unparseable** and nothing else.
///
/// # The line that carries OUR suffix is judged before its count is
///
/// Every rule below about skipping a line applies to lines that are *not*
/// ours. A line whose 35 characters match the suffix we asked about is the
/// answer to the question, and if its count cannot be used then we do not
/// have an answer -- so that line returns `Err(Malformed)` and the caller
/// paints `Unavailable`. It must never fall through to `Ok(None)`.
///
/// That distinction is the whole of this function's safety. "Skip the line"
/// and "the password is fine" are the same output whenever any *other* line
/// in the body parsed, and a padded body always has other lines that parse.
/// So a match with an unusable count used to read as `Safe` on a breached
/// password, which is the one answer this module must never invent.
///
/// `Unavailable` rather than a saturating count, and the two are not
/// interchangeable: an overflowing count is all digits, so it is known to be
/// a real and enormous number and `u64::MAX` loses nothing (the advice does
/// not vary by count). A count of `"lots"`, `"-5"` or `""` is not a number at
/// all; saturating it would put a fabricated 18-quintillion figure in front
/// of the user, and choosing any other number would be equally invented.
/// "Could not check" is the true statement, and it is the one that fails
/// closed.
///
/// # A count of zero is a decoy on any line but ours
///
/// `Add-Padding: true` is HIBP's own privacy hardening for this API, and this
/// crate sends it: the response is bulked out with randomly generated decoy
/// suffixes carrying a count of **0**, so that the response size does not
/// identify the bucket to anyone watching the encrypted stream.
///
/// An earlier version returned `Err(Malformed("zero count"))` on the first
/// such line, short-circuiting before it could reach a genuine match. Turning
/// on the natural next privacy step for a privacy feature would therefore
/// have made every lookup a permanent `Unavailable` -- silently. A zero-count
/// line whose suffix is not ours is **skipped**: it counts as a well-formed
/// line, so an all-decoy body is `Safe` (genuinely not breached, which is
/// what it means).
///
/// A zero count on a line carrying **our** suffix is a different thing, and
/// the honest description of it is: *we do not know what it means.* HIBP is
/// not documented to return a real count of zero, and a decoy is generated
/// randomly, so a decoy colliding with our 35 characters is a 16^-35 event --
/// but neither of those is a fact this code can check, and the old comment
/// asserted the first as though it were. So it is treated as unusable, the
/// same as `"abc"`: `Err(Malformed)`, `Unavailable`. That keeps
/// `BreachStatus::Breached(0)` unrepresentable, removes an unverifiable
/// protocol assumption from the safe path, and costs one astronomically rare
/// spurious "couldn't check" instead of one wrong green badge.
///
/// # Duplicate lines report the LARGEST count
///
/// A body should not list the same suffix twice, but if it does, reporting
/// the first one understates the breach by whatever the later lines say.
/// The whole loop runs and the maximum wins.
///
/// # A count too large for `u64` saturates
///
/// `parse::<u64>()` fails on overflow exactly as it fails on `"abc"`, and the
/// two must not be handled alike. An all-digit count is a real breach whose
/// magnitude merely does not fit; letting it `continue` meant that if any
/// *other* line parsed, the answer was `Ok(None)` and the password read as
/// `Safe`. So an all-digit count that overflows saturates to `u64::MAX` -- it
/// is still a breach, and the phrase does not vary by count anyway. A count
/// that is not all digits is not a range line, and the line is skipped:
/// that, and only that, is what "structurally unparseable" means here.
///
/// Matching is ASCII-case-insensitive: the API returns uppercase today, but
/// "not breached" is not something to make contingent on that.
pub fn parse_range_body(body: &str, suffix: &str) -> Result<Option<u64>, Malformed> {
    let mut saw_well_formed_line = false;
    let mut found = None;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((line_suffix, count_text)) = line.split_once(':') else {
            continue;
        };
        if line_suffix.len() != SUFFIX_LEN
            || !line_suffix.bytes().all(|b| b.is_ascii_hexdigit())
        {
            continue;
        }
        let count_text = count_text.trim();

        // **Whose line is this** is decided before the count is judged, and
        // that ordering is the safety property. Every `continue` below means
        // "this line is not evidence"; for a line that is not ours that is
        // true and harmless, and for a line that IS ours it silently turns a
        // breached password into `Ok(None)` -> `Safe` the moment any other
        // line in the body parsed -- which, under padding, is always.
        let is_ours = line_suffix.eq_ignore_ascii_case(suffix);

        // Not digits (empty, signed, hex, prose) -- not a range line.
        if count_text.is_empty() || !count_text.bytes().all(|b| b.is_ascii_digit()) {
            if is_ours {
                return Err(Malformed("unusable count on the matching line"));
            }
            continue;
        }
        // All digits, so the only way parsing can fail is overflow, and an
        // overflowing count is still a breach. See the doc above.
        let count = count_text.parse::<u64>().unwrap_or(u64::MAX);

        // Structurally a range line either way, decoy or not. This is what
        // stops a padded body from reading as "nothing parsed".
        saw_well_formed_line = true;

        if count == 0 {
            if is_ours {
                // Not knowably a decoy and not knowably a count. See the doc.
                return Err(Malformed("zero count on the matching line"));
            }
            // HIBP padding. Skipped rather than matched, so Breached(0) is
            // unrepresentable; counted above, so an all-decoy body is Safe.
            continue;
        }
        if is_ours {
            // The largest, not the first: a duplicate that says more is the
            // one worth reporting.
            found = Some(found.map_or(count, |seen: u64| seen.max(count)));
        }
    }

    if !saw_well_formed_line {
        return Err(Malformed("no well-formed range line"));
    }
    Ok(found)
}

/// Group a count by thousands, so a five- or eight-digit number reads as a
/// magnitude rather than as a wall of digits.
fn group_thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The one sentence shown for a breached password.
///
/// **The advice never varies by count.** "Seen 3 times" and "seen 40,000
/// times" mean the same thing -- the password is on a public list -- and
/// softening the wording for a small number would invite the user to keep it.
/// The number is reported because it is real, not because it grades the risk.
pub fn breach_phrase(count: u64) -> String {
    format!(
        "Found in a known data breach ({} times). Change this password.",
        group_thousands(count)
    )
}

/// Where a range lookup may be addressed -- the other half of [`Prefix`].
///
/// **The field is private and neither constructor accepts caller-supplied
/// text.** That is the entire type.
///
/// [`Prefix`] closed the door on the path *tail*: a call site cannot put the
/// whole 40-character hash where the five-character prefix belongs, because
/// it cannot build a `Prefix` out of one. But `check_prefix` took
/// `base_url: &str` and did `format!("{base_url}/{}", prefix.as_str())`, and
/// `hex_digest` was `pub(crate)` -- so the identical leak was one argument to
/// the left, and it compiled:
///
/// ```text
/// check_prefix(&format!("{HIBP_RANGE_BASE}/{}", hex_digest(pw)), &prefix, ..)
/// ```
///
/// The privacy guard could not see it either. It supplies its own `base_url`
/// and then asserts the request line it computes *from that same value*, so
/// it is strong against a header nobody anticipated and worthless against a
/// path the caller chose. A newtype on one argument and a bare `&str` on the
/// one right next to it is not a closed door; it is a door with the frame
/// left out.
///
/// So the parameter no longer carries text at all. [`BaseUrl::production`]
/// takes **nothing**, and [`BaseUrl::loopback`] takes a `u16` port -- a type
/// that cannot hold a hex character, let alone forty of them. There is no
/// spelling of a call site that smuggles hash material through this argument,
/// and that is a fact the compiler enforces at sites not yet written, exactly
/// as `Prefix` does.
///
/// The inner `String` is not `Zeroizing`, for the same reason `Prefix`'s is
/// not: an endpoint is not a secret. It is constrained because it is the
/// place secrets could be *hidden*, not because it is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl(String);

impl BaseUrl {
    /// The real Have I Been Pwned range API.
    ///
    /// Takes no argument, so there is nothing a caller can put into it. This
    /// and the endpoint pin are the only places the constant is named.
    pub fn production() -> Self {
        BaseUrl(HIBP_RANGE_BASE.to_string())
    }

    /// A mock on `127.0.0.1`, for tests.
    ///
    /// The **port is the only thing a caller supplies**; scheme, host and
    /// path are fixed here. A test cannot point this at another machine and
    /// cannot append a path segment to it, so the one seam this module has
    /// for testing is a seam a `u16` fits through and nothing else does.
    pub fn loopback(port: u16) -> Self {
        BaseUrl(format!("http://127.0.0.1:{port}/range"))
    }

    /// The base, for interpolation into the range URL.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The agent for [`check_prefix`]: one small text response, bounded by total
/// elapsed time. See [`crate::http_agent`] for why the shape is a type.
pub fn build_agent() -> TotalBounded {
    crate::http_agent::bounded_total(CONNECT_TIMEOUT, DEADLINE)
}

/// Ask the range API about `prefix` and match `suffix` locally.
///
/// `base_url` comes first and the agent is injected, the same shape as
/// `updater::check_for_update`, so tests point it at a local mock and no test
/// in this crate ever reaches the real API.
///
/// **Both halves of the URL are newtypes with private fields, and neither can
/// be built out of the hash.** `prefix` is a [`Prefix`], whose only
/// constructor is [`split_hash`]; `base_url` is a [`BaseUrl`], whose two
/// constructors take no argument and a `u16` respectively. So the whole
/// string this function puts on the wire is composed of exactly two values
/// that the compiler already knows cannot carry password material -- not one
/// checked value next to one free-form one. Everything that could identify
/// the password stays in `suffix`, which is only ever compared against text
/// that came back.
///
/// `Add-Padding: true` asks HIBP to bulk the response out with decoy suffixes
/// so its size does not identify the bucket. It is a constant, carries
/// nothing about the user, and is on the allowlist in
/// `the_request_head_carries_nothing_beyond_the_allowlist`;
/// [`parse_range_body`] skips the decoys.
///
/// Every failure -- transport, non-200, unreadable body, unparseable body --
/// collapses to [`BreachStatus::Unavailable`]. None of them collapse to
/// `Safe`; a lookup that did not happen must never paint a green badge.
pub fn check_prefix(
    base_url: &BaseUrl,
    prefix: &Prefix,
    suffix: &Zeroizing<String>,
    agent: &TotalBounded,
) -> BreachStatus {
    let url = format!("{}/{}", base_url.as_str(), prefix.as_str());
    let Ok(response) = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Add-Padding", "true")
        .call()
    else {
        return BreachStatus::Unavailable;
    };
    let Ok(body) = response.into_string() else {
        return BreachStatus::Unavailable;
    };
    match parse_range_body(&body, suffix) {
        Ok(Some(count)) => BreachStatus::Breached(count),
        Ok(None) => BreachStatus::Safe,
        Err(_) => BreachStatus::Unavailable,
    }
}

/// The one call a [`BreachCache`] worker makes, injected rather than hard-wired.
///
/// It is a boxed closure rather than a bare `fn` pointer for one reason: a
/// test's stub has to carry its own invocation counter and its own gate, and a
/// `fn` pointer can capture nothing, so those would have to be statics --
/// shared by every test in the binary, which run in parallel. With this shape
/// each test owns its counter, and "exactly one worker was started" is a
/// measurement rather than a hope.
///
/// The real one is [`live_check`] and it is wired in exactly one place,
/// [`BreachCache::live`]. No test constructs it.
pub type BreachCheck = Arc<dyn Fn(&Prefix, &Zeroizing<String>) -> BreachStatus + Send + Sync>;

/// How long a caller should wait before drawing again while a lookup is out.
///
/// Named once, here, for the same reason `AppIdentityCache::POLL_INTERVAL` is:
/// a channel is not egui input, so nothing about an answer arriving wakes the
/// UI on its own.
pub const BREACH_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The production check: the real range API, over the real network.
///
/// Nothing in the test suite may reach this function. It is private, it is
/// named at one call site ([`BreachCache::live`]), and every test builds its
/// own [`BreachCheck`] instead.
fn live_check(prefix: &Prefix, suffix: &Zeroizing<String>) -> BreachStatus {
    check_prefix(&BaseUrl::production(), prefix, suffix, &build_agent())
}

/// What the cache knows about one prefix.
///
/// `Ready` holds the **resolved [`BreachStatus`] and nothing else** -- never
/// the range body, never the suffix list. A body is ~20 KB of public data that
/// answers exactly one question, and once that question is answered it buys
/// nothing; a second password sharing a prefix is a one-in-a-million event
/// that may pay for a fresh request. Holding the list would also mean holding,
/// in this process, a structure whose whole purpose is to be matched against
/// the user's hashes.
enum Entry {
    /// A worker is out. **Holding this receiver IS the outstanding request:**
    /// dropping the entry drops the channel, and the worker's answer then has
    /// nowhere to land. See [`BreachCache::clear`].
    Pending(mpsc::Receiver<BreachStatus>),
    /// Answered.
    Ready(BreachStatus),
}

/// One breach answer per hash prefix, for as long as the vault is unlocked.
///
/// # Nothing here is persisted, and nothing here has a TTL
///
/// The map dies with the window and [`Self::clear`] empties it on lock. That
/// is not a simplification to be improved on later. "Item X is breached" is a
/// claim about this user's vault and a target the moment it is on disk; a
/// prefix-to-suffix cache on disk would be worse still, recording what the
/// user's passwords hash to and narrowing every one of them by a factor of a
/// million. There is no expiry because there is no lifetime long enough for
/// one to matter: the longest an entry can live is one unlocked session.
///
/// # There is no `Default`, on purpose
///
/// A cache with no check function is a cache that cannot answer, and the
/// obvious filler -- the live check -- is exactly what tests must never get by
/// accident. [`Self::live`] and [`Self::new`] both name what they wire.
pub struct BreachCache {
    /// Keyed on the **prefix**, never on the whole hash. The whole hash is the
    /// thing this module exists to keep off the wire and out of long-lived
    /// storage; a map keyed on it would hold every password's complete SHA-1,
    /// in memory, for the length of the session, to answer a question that is
    /// only ever asked about the first five characters.
    entries: HashMap<String, Entry>,
    check: BreachCheck,
}

impl BreachCache {
    /// The production cache: workers call the real Have I Been Pwned range API.
    ///
    /// The **only** place [`live_check`] is named.
    pub fn live() -> Self {
        Self::new(Arc::new(live_check))
    }

    /// A cache whose workers run `check`. Tests pass a stub; production calls
    /// [`Self::live`].
    pub fn new(check: BreachCheck) -> Self {
        Self { entries: HashMap::new(), check }
    }

    /// The status of `password`, from cache if it is known, otherwise
    /// [`BreachStatus::Pending`] with one worker started for its prefix.
    ///
    /// A pending entry is promoted to ready here, on the UI thread, by
    /// `try_recv`; nothing else moves an answer into the map.
    ///
    /// Every arm that has no answer says `Pending`. None of them says `Safe` --
    /// see [`BreachStatus`].
    pub fn status(&mut self, ctx: &egui::Context, password: &str) -> BreachStatus {
        // **Hashed here, on the UI thread, before any spawn.** The worker is
        // handed the `(Prefix, Zeroizing<String>)` and never the password --
        // see `spawn_check`, where the reason is spelled out at the boundary
        // it protects.
        let (prefix, suffix) = split_hash(password);
        let key = prefix.as_str().to_string();

        if !self.entries.contains_key(&key) {
            // Two statements rather than `entry().or_insert_with()`: the
            // closure would capture `self.check` while the map is already
            // mutably borrowed, and spelling the insert out keeps the
            // "spawn exactly once, and only on a miss" rule visible.
            let rx = spawn_check(&self.check, prefix, suffix);
            self.entries.insert(key, Entry::Pending(rx));
            ctx.request_repaint_after(BREACH_POLL_INTERVAL);
            return BreachStatus::Pending;
        }

        // Read the channel, then write the map: two steps, because the
        // receiver is borrowed out of the very map the promotion writes to.
        let answered = match self.entries.get(&key) {
            Some(Entry::Pending(rx)) => rx.try_recv().ok(),
            _ => None,
        };
        if let Some(answered) = answered {
            // Whatever the worker said, stored verbatim. An `Unavailable`
            // becomes a cached `Unavailable`; it must never be softened into
            // `Safe` on the way in, which would paint a green badge for a
            // lookup that did not happen.
            self.entries.insert(key.clone(), Entry::Ready(answered));
        }

        match self.entries.get(&key) {
            Some(Entry::Ready(answered)) => *answered,
            // Still out. Keep the frames coming: a channel is not egui input,
            // so without this the badge never appears until something else
            // happens to cause a repaint.
            Some(Entry::Pending(_)) => {
                ctx.request_repaint_after(BREACH_POLL_INTERVAL);
                BreachStatus::Pending
            }
            // Not reachable -- the key was present two lines ago and nothing
            // removes entries here. `Pending` rather than a hard stop, because
            // the one answer this must never invent is `Safe`.
            None => BreachStatus::Pending,
        }
    }

    /// Forget everything. Called on vault lock and on window close.
    ///
    /// **This drops pending entries as well as ready ones**, and that is the
    /// security-relevant half. `HashMap::clear` drops every value, so each
    /// `Entry::Pending` drops its receiver, and a worker that was still out at
    /// lock time finds its `send` failing: its answer cannot land in the map a
    /// later session goes on to read. Clearing only the answered entries would
    /// leave a lookup started before the lock able to resolve after it.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Start one worker for one prefix, and hand back the channel its answer will
/// arrive on.
///
/// # The password does not cross this boundary, and must not be made to
///
/// The hash is computed by the caller on the UI thread; only the `Prefix` and
/// the already-`Zeroizing` suffix are moved in here. **Do not "simplify" this
/// by moving `split_hash` into the closure.** A `&str` cannot be moved into a
/// thread, so the version of that refactor which compiles is
/// `password.to_string()` -- a plain `String` holding the plaintext, owned by
/// a worker for the length of a network round trip, and freed *unwiped* onto a
/// heap the allocator hands straight to the next caller. It would compile, it
/// would pass, and it would be the one thing this module is for.
///
/// The channel is bounded at one and the send is allowed to fail: if the vault
/// locked or the window closed while the worker was out, the receiver is gone
/// and there is nobody to tell. Nothing is retried and nothing is joined.
fn spawn_check(
    check: &BreachCheck,
    prefix: Prefix,
    suffix: Zeroizing<String>,
) -> mpsc::Receiver<BreachStatus> {
    let (tx, rx) = mpsc::sync_channel(1);
    let check = Arc::clone(check);
    std::thread::spawn(move || {
        let answered = check(&prefix, &suffix);
        let _ = tx.send(answered);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    /// A realistic range body: 24 lines, CRLF-terminated as the API sends
    /// them, with the real suffix for the password `"password"` sitting in
    /// the middle rather than first or last.
    ///
    /// `the_fixture_is_a_realistic_nonempty_range_body` is the control that
    /// makes every test below mean something.
    const FIXTURE: &str = concat!(
        "1DE5159B964F0EBCBDB7C113C9B09EFDED7:820\r\n",
        "8C365E9E3E4B8EE6A31F19BD55EE5E8E38D:13000\r\n",
        "2F808915F87629F98D9FC07395598527E2B:50984\r\n",
        "9F1A5D133B4751F21F8C334952C42B8CDAE:82046\r\n",
        "7DBEAC16087D1CCA78EC232FD0CCDC10B2B:35796\r\n",
        "A5244143F8AE5B6494A94FC9C4678F30BCD:75379\r\n",
        "0EEA2D248DF6B3929B6F66DF86EFAB3F966:65394\r\n",
        "4E4BC7FF99F0C26327495652280D97F01DF:45222\r\n",
        "45E2CBB3B2DEE2D5A3F3414B1FC7B3FD59F:19839\r\n",
        "FB13D8DA7CDAB41BA21E4BB12877E7C39BA:40937\r\n",
        "461EED3FD9C9CCA49C84444723B456CEEA4:82285\r\n",
        "1E4C9B93F3F0682250B6CF8331B7EE68FD8:10437277\r\n",
        "E92EA352A732EE496AC1778CBA930D818C9:81687\r\n",
        "75468B61FB470019336A227E43E963010C7:14349\r\n",
        "798725AB40C6C74E14BD52DB179CB7F6BDF:76235\r\n",
        "2C9DBA8A7CAC7604DEBA99CF82A77FF403F:73658\r\n",
        "2D14678DAB86621BB49ACB377BB6A0550EC:15229\r\n",
        "D1490F2A66CD208BCDCCF5DAD627E1F9953:88403\r\n",
        "C489A03DDE394D781EAD9D87C868D0B733B:52249\r\n",
        "4E34E5583F75698FF96430EBE1E3FD8F9BA:28272\r\n",
        "A4860485F4FB16A07000823083D787A47C7:3838\r\n",
        "E0A981678F1A5763ACE995F091C5BECFC87:36484\r\n",
        "258F7B93BC00C693FA287E56DDDDCCF4090:40122\r\n",
        "AC6F922D169BA3CD603E0C8C2850DCB5901:34465\r\n",
    );

    /// The full SHA-1 of `"password"`, uppercase.
    ///
    /// Cross-checked against an independent implementation (Python's
    /// `hashlib.sha1`) rather than recalled, and the two structural vectors
    /// below (`""` and `"abc"`) are the ones published with the algorithm
    /// itself in FIPS 180-1, so a wrong constant here cannot agree with a
    /// wrong implementation.
    const PASSWORD_SHA1: &str = "5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8";
    /// The 35 characters of [`PASSWORD_SHA1`] that must never leave the box.
    const PASSWORD_SUFFIX: &str = "1E4C9B93F3F0682250B6CF8331B7EE68FD8";
    /// Its literal count in [`FIXTURE`], asserted rather than `is_some()`d.
    const PASSWORD_COUNT: u64 = 10_437_277;
    /// 35 characters that are not hex at all, so no real body can contain it.
    const ABSENT_SUFFIX: &str = "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ";
    /// A real hex suffix that is absent from [`FIXTURE`] and differs from
    /// [`PASSWORD_SUFFIX`], which is present, in **only the last character**.
    ///
    /// [`ABSENT_SUFFIX`] above is 35 `Z`s: not hex, and different from every
    /// fixture line in every position, which makes it useless as a probe of
    /// the comparison itself. `line_suffix[..20].eq_ignore_ascii_case(
    /// &suffix[..20])` -- which stops comparing 15 of the 35 secret
    /// characters -- passed 1684 green against it. A one-character near miss
    /// at the far end does not.
    const NEAR_MISS_LAST: &str = "1E4C9B93F3F0682250B6CF8331B7EE68FD9";
    /// The same idea at the other end, so a comparison weakened at the *head*
    /// of the suffix is caught as well as one weakened at the tail.
    const NEAR_MISS_FIRST: &str = "2E4C9B93F3F0682250B6CF8331B7EE68FD8";

    /// A padded body, the shape `Add-Padding: true` returns: decoys carrying
    /// a count of **0** before, between and after the real lines, with the
    /// real match for `"password"` among them.
    const PADDED_FIXTURE: &str = concat!(
        "8BB9E9DF6591605A2E953C3B1A7D4749954:0\r\n",
        "DBF36FADC5D2E54085E7D442EA9736987B1:0\r\n",
        "1DE5159B964F0EBCBDB7C113C9B09EFDED7:820\r\n",
        "67B28BB39B66D64A07A859DD8FC13A32AEB:0\r\n",
        "1E4C9B93F3F0682250B6CF8331B7EE68FD8:10437277\r\n",
        "6B461A4C886C496CB8472943E82B677DADB:0\r\n",
        "A5244143F8AE5B6494A94FC9C4678F30BCD:75379\r\n",
        "C775186DB2E3062F0CE6C21FC48C6006596:0\r\n",
        "975E274A80D7350D8162BB04E75C95852DC:0\r\n",
    );
    /// A padded body that is **nothing but** decoys: the shape a genuinely
    /// not-breached password gets back once padding is on.
    const ALL_DECOY_FIXTURE: &str = concat!(
        "7F0B7FD970186A7F3387DC644A6AD3EDF84:0\r\n",
        "B8BA10539061F2DCA511F223E1D90D5B2DB:0\r\n",
        "8BB9E9DF6591605A2E953C3B1A7D4749954:0\r\n",
    );

    /// The prefix every mock below is keyed on. Built by `split_hash` because
    /// that is the only thing in the crate that can build one.
    fn password_prefix() -> Prefix {
        split_hash("password").0
    }

    // ---------------------------------------------------------------- parser

    /// **The control without which half this file is vacuous.** A parser test
    /// that says "this suffix is not in the body" passes just as happily
    /// against an empty fixture, or against one whose lines are all garbage.
    /// So: the fixture is long, the suffix the "present" test looks for really
    /// is in it, and the suffix the "absent" test looks for really is not.
    #[test]
    fn the_fixture_is_a_realistic_nonempty_range_body() {
        let lines: Vec<&str> = FIXTURE.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(lines.len() >= 20, "fixture has only {} lines", lines.len());
        assert!(
            FIXTURE.contains(PASSWORD_SUFFIX),
            "the fixture does not contain the suffix the 'present' test looks for"
        );
        assert!(
            !FIXTURE.contains(ABSENT_SUFFIX),
            "the fixture contains the suffix the 'absent' test assumes is missing"
        );
        assert_eq!(ABSENT_SUFFIX.len(), SUFFIX_LEN, "the fabricated suffix is the wrong length");
        for line in &lines {
            let (s, c) = line.trim().split_once(':').expect("every fixture line is SUFFIX:COUNT");
            assert_eq!(s.len(), SUFFIX_LEN, "fixture line {line:?} has a bad suffix length");
            assert!(c.parse::<u64>().unwrap() > 0, "fixture line {line:?} has a zero count");
        }
    }

    #[test]
    fn a_present_suffix_returns_its_count() {
        assert_eq!(
            parse_range_body(FIXTURE, PASSWORD_SUFFIX),
            Ok(Some(PASSWORD_COUNT)),
            "the count must be the fixture's literal value, not merely 'some number'"
        );
    }

    #[test]
    fn a_known_absent_suffix_returns_none_from_a_nonempty_body() {
        assert_eq!(parse_range_body(FIXTURE, ABSENT_SUFFIX), Ok(None));
    }

    #[test]
    fn an_empty_body_is_malformed_not_safe() {
        let parsed = parse_range_body("", PASSWORD_SUFFIX);
        assert!(parsed.is_err(), "an empty 200 must not read as 'not breached'");
        // And separately: the conversion the caller actually performs.
        assert_eq!(status_for(""), BreachStatus::Unavailable);
    }

    #[test]
    fn a_garbage_body_is_malformed_not_safe() {
        let garbage = "<html><body>Service Unavailable</body></html>";
        let parsed = parse_range_body(garbage, PASSWORD_SUFFIX);
        assert!(parsed.is_err(), "an HTML error page served with a 200 must not read as safe");
        assert_eq!(status_for(garbage), BreachStatus::Unavailable);
    }

    /// The same collapse `check_prefix` performs, over a body rather than a
    /// socket -- so the two halves of the tests above (the `Err`, and what the
    /// `Err` becomes) can both be asserted without a server.
    fn status_for(body: &str) -> BreachStatus {
        match parse_range_body(body, PASSWORD_SUFFIX) {
            Ok(Some(count)) => BreachStatus::Breached(count),
            Ok(None) => BreachStatus::Safe,
            Err(_) => BreachStatus::Unavailable,
        }
    }

    /// **A count of zero on a line that is NOT ours is padding; on a line
    /// that IS ours it is an answer we cannot read.**
    ///
    /// This test has been through both wrong answers. It first asserted that
    /// any zero count was `Err` -- which short-circuited the whole body and
    /// would have made every padded lookup a permanent `Unavailable`. It then
    /// asserted that a zero count on our *own* suffix was `Safe`, on the
    /// stated ground that "HIBP never returns a count of 0". That is not
    /// something this code can check; it is a protocol assumption written in
    /// a comment, sitting directly on the path that paints a green badge.
    ///
    /// So: decoys elsewhere in the body are skipped and an all-decoy body is
    /// `Safe` (covered by `an_all_decoy_body_is_safe_not_unavailable`), and a zero on our own
    /// line is `Unavailable`. `Breached(0)` stays unrepresentable either way,
    /// which is what the original was reaching for.
    #[test]
    fn a_zero_count_on_our_own_line_is_unavailable_not_safe() {
        // Shaped exactly like a range line in every other respect, and
        // carrying the very suffix we are looking for.
        let body = format!("{PASSWORD_SUFFIX}:0\r\n");
        assert!(
            parse_range_body(&body, PASSWORD_SUFFIX).is_err(),
            "a zero count on our own line is not a readable answer"
        );
        assert_eq!(status_for(&body), BreachStatus::Unavailable);
        assert_ne!(
            status_for(&body),
            BreachStatus::Breached(0),
            "Breached(0) must be unrepresentable"
        );
    }

    /// **The dangerous shape, for the zero case: our line, in a body whose
    /// other lines parse.**
    ///
    /// The single-line version above cannot fail through `Ok(None)`, because
    /// with no other well-formed line the body is `Err` anyway. This one can:
    /// every decoy around our line is well-formed, so `saw_well_formed_line`
    /// is already true and a `continue` on our own line resolves to `Ok(None)`
    /// -- `Safe`, on the one line that was about this password.
    #[test]
    fn a_zero_count_on_our_own_line_in_a_mixed_body_is_not_safe() {
        let body = format!(
            "{}{PASSWORD_SUFFIX}:0\r\n{}",
            "8BB9E9DF6591605A2E953C3B1A7D4749954:0\r\n",
            "A5244143F8AE5B6494A94FC9C4678F30BCD:75379\r\n"
        );
        // Control: the surrounding lines really are well-formed on their own,
        // so this body really is the mixed shape and not a degenerate one.
        assert_eq!(
            parse_range_body(&body, NEAR_MISS_LAST),
            Ok(None),
            "control: the other lines parse, so a skip of our line lands on Ok(None)"
        );
        assert_ne!(status_for(&body), BreachStatus::Safe, "our own line read as Safe");
        assert_eq!(status_for(&body), BreachStatus::Unavailable);
    }

    /// **The dangerous shape, for the non-numeric case.**
    ///
    /// `a_non_numeric_count_is_not_a_range_line` builds a body of one line, so
    /// its `Err` comes from "no well-formed line at all" and it would pass
    /// just as green with the matching line skipped. This body has decoys and
    /// a real other line around ours, so the skip is survivable and the answer
    /// is `Ok(None)` -- `Safe` -- unless the matching line is treated as the
    /// answer it is.
    #[test]
    fn a_non_numeric_count_on_our_own_line_in_a_mixed_body_is_not_safe() {
        for count in ["not-a-number", "-5", "+5", "0x20", "", "1 2", "9e9"] {
            let body = format!(
                "{}{PASSWORD_SUFFIX}:{count}\r\n{}",
                "8BB9E9DF6591605A2E953C3B1A7D4749954:0\r\n",
                "A5244143F8AE5B6494A94FC9C4678F30BCD:75379\r\n"
            );
            // Control: with our line taken out of the question the body is a
            // perfectly good `Ok(None)`, so `Safe` is genuinely reachable here
            // and the assertion below is not vacuous.
            assert_eq!(
                parse_range_body(&body, NEAR_MISS_LAST),
                Ok(None),
                "control failed for count {count:?}"
            );
            assert_ne!(
                status_for(&body),
                BreachStatus::Safe,
                "count {count:?} on our own line read as Safe -- a green badge on a password \
                 whose answer we could not read"
            );
            assert_eq!(status_for(&body), BreachStatus::Unavailable, "count {count:?}");
        }
    }

    /// **A body that lists our suffix twice reports the LARGEST count.**
    ///
    /// Reporting the first understates the breach by whatever the later lines
    /// say. The two counts are far apart and in both orders, so a test that
    /// merely happened to see the bigger one first cannot pass.
    #[test]
    fn duplicate_lines_for_our_suffix_report_the_largest_count() {
        let small = 7u64;
        let large = 987_654u64;
        for (a, b) in [(small, large), (large, small)] {
            let body = format!("{PASSWORD_SUFFIX}:{a}\r\n{PASSWORD_SUFFIX}:{b}\r\n");
            assert_eq!(
                parse_range_body(&body, PASSWORD_SUFFIX),
                Ok(Some(large)),
                "duplicate counts {a} then {b} did not report the largest"
            );
        }
        // And a duplicate whose second line is unusable still cannot make the
        // password read as Safe -- it is our line either way.
        let body = format!("{PASSWORD_SUFFIX}:{small}\r\n{PASSWORD_SUFFIX}:abc\r\n");
        assert_ne!(status_for(&body), BreachStatus::Safe);
    }

    /// The padded fixtures really are padded, and the decoys really do sit on
    /// both sides of the real match -- otherwise "a padded body still finds
    /// the match" would be a test about an ordinary body.
    #[test]
    fn the_padded_fixtures_are_shaped_like_hibps_padding() {
        let rows = |body: &'static str| -> Vec<(String, u64)> {
            body.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let (s, c) = l.trim().split_once(':').expect("SUFFIX:COUNT");
                    assert_eq!(s.len(), SUFFIX_LEN, "fixture line {l:?} has a bad suffix length");
                    assert!(s.bytes().all(|b| b.is_ascii_hexdigit()), "{l:?} is not hex");
                    (s.to_string(), c.parse::<u64>().expect("a numeric count"))
                })
                .collect()
        };

        let padded = rows(PADDED_FIXTURE);
        assert!(padded.len() >= 8, "the padded fixture has only {} lines", padded.len());
        let real = padded
            .iter()
            .position(|(s, _)| s == PASSWORD_SUFFIX)
            .expect("the padded fixture contains the real match");
        assert!(
            padded[..real].iter().any(|(_, c)| *c == 0),
            "no decoy before the real match, so a parser that stopped at the first decoy would \
             still find it"
        );
        assert!(
            padded[real + 1..].iter().any(|(_, c)| *c == 0),
            "no decoy after the real match"
        );
        assert!(
            padded.iter().filter(|(_, c)| *c != 0).count() >= 3,
            "the padded fixture has no real lines besides the match"
        );

        let decoys = rows(ALL_DECOY_FIXTURE);
        assert!(decoys.len() >= 3, "the all-decoy fixture has only {} lines", decoys.len());
        assert!(
            decoys.iter().all(|(_, c)| *c == 0),
            "the all-decoy fixture has a non-zero line in it"
        );
        assert!(
            !ALL_DECOY_FIXTURE.contains(PASSWORD_SUFFIX),
            "the all-decoy fixture contains the suffix the Safe assertion assumes is missing"
        );
    }

    #[test]
    fn a_padded_body_still_finds_the_real_match() {
        assert_eq!(
            parse_range_body(PADDED_FIXTURE, PASSWORD_SUFFIX),
            Ok(Some(PASSWORD_COUNT)),
            "the decoys must not hide a genuine match, and must not short-circuit the body"
        );
        assert_eq!(status_for(PADDED_FIXTURE), BreachStatus::Breached(PASSWORD_COUNT));
    }

    #[test]
    fn an_all_decoy_body_is_safe_not_unavailable() {
        assert_eq!(parse_range_body(ALL_DECOY_FIXTURE, PASSWORD_SUFFIX), Ok(None));
        assert_eq!(
            status_for(ALL_DECOY_FIXTURE),
            BreachStatus::Safe,
            "an all-decoy body is what a genuinely not-breached password gets back once padding \
             is on; reading it as Unavailable turns the feature off for exactly those users"
        );
    }

    /// A count too large for `u64` is still a breach.
    ///
    /// The dangerous shape is the one built here: the overflowing line is not
    /// alone, so some *other* line parses, `saw_well_formed_line` is set, and
    /// a `continue` on the overflow produces `Ok(None)` -- `Safe`.
    #[test]
    fn a_count_too_large_for_u64_saturates_rather_than_reading_as_safe() {
        let huge = "9".repeat(30);
        assert!(huge.parse::<u64>().is_err(), "control: {huge} really does overflow u64");
        assert!(huge.bytes().all(|b| b.is_ascii_digit()), "control: it is all digits");

        let body = format!("1DE5159B964F0EBCBDB7C113C9B09EFDED7:820\r\n{PASSWORD_SUFFIX}:{huge}\r\n");
        assert_eq!(parse_range_body(&body, PASSWORD_SUFFIX), Ok(Some(u64::MAX)));
        assert_eq!(status_for(&body), BreachStatus::Breached(u64::MAX));
        assert_ne!(
            status_for(&body),
            BreachStatus::Safe,
            "a count too big to represent is a very breached password, not an unbreached one"
        );
    }

    /// The other half of the same decision: a count that is not digits at all
    /// is structurally not a range line, and skipping it is right.
    #[test]
    fn a_non_numeric_count_is_not_a_range_line() {
        for count in ["not-a-number", "-5", "+5", "0x20", "", "1 2"] {
            let body = format!("{PASSWORD_SUFFIX}:{count}\r\n");
            assert!(
                parse_range_body(&body, PASSWORD_SUFFIX).is_err(),
                "count {count:?} was accepted as a range line"
            );
            assert_eq!(status_for(&body), BreachStatus::Unavailable, "count {count:?}");
        }
    }

    /// The near-miss suffixes really are near misses: 35 hex characters,
    /// absent from the fixture, differing from the present one in exactly one
    /// position -- the last, and the first.
    #[test]
    fn the_near_miss_suffixes_differ_in_exactly_one_character() {
        for (name, near, expected) in [
            ("NEAR_MISS_LAST", NEAR_MISS_LAST, SUFFIX_LEN - 1),
            ("NEAR_MISS_FIRST", NEAR_MISS_FIRST, 0),
        ] {
            assert_eq!(near.len(), SUFFIX_LEN, "{name} is the wrong length");
            assert!(near.bytes().all(|b| b.is_ascii_hexdigit()), "{name} is not hex");
            let differing: Vec<usize> = near
                .bytes()
                .zip(PASSWORD_SUFFIX.bytes())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i)
                .collect();
            assert_eq!(differing, vec![expected], "{name} differs from the real suffix at");
            assert!(!FIXTURE.contains(near), "{name} is in the fixture after all");
        }
    }

    /// A suffix that differs from a present one in a single character is not
    /// a match -- at either end.
    ///
    /// This is the assertion `ABSENT_SUFFIX` could never make. A comparison
    /// that looks at only the first 20 characters, or only the last 20, is
    /// indistinguishable from a correct one when the "absent" probe is 35
    /// `Z`s.
    #[test]
    fn a_one_character_near_miss_is_not_a_match() {
        assert_eq!(parse_range_body(FIXTURE, NEAR_MISS_LAST), Ok(None));
        assert_eq!(parse_range_body(FIXTURE, NEAR_MISS_FIRST), Ok(None));
        assert_eq!(parse_range_body(PADDED_FIXTURE, NEAR_MISS_LAST), Ok(None));
    }

    #[test]
    fn suffix_matching_is_case_insensitive_on_the_wire_form() {
        let lower = PASSWORD_SUFFIX.to_ascii_lowercase();
        assert_ne!(lower, PASSWORD_SUFFIX, "control: the two spellings really do differ");
        assert_eq!(parse_range_body(FIXTURE, &lower), Ok(Some(PASSWORD_COUNT)));

        let lower_body = FIXTURE.to_ascii_lowercase();
        assert_ne!(lower_body, FIXTURE, "control: the two bodies really do differ");
        assert_eq!(parse_range_body(&lower_body, PASSWORD_SUFFIX), Ok(Some(PASSWORD_COUNT)));
    }

    // ------------------------------------------------------------ split_hash

    /// The two vectors published with SHA-1 itself (FIPS 180-1 appendix), plus
    /// the one this module's fixture turns on. Verified against an independent
    /// implementation before being written down.
    ///
    /// This is the test that fails loudly if someone "upgrades" the hash.
    #[test]
    fn split_hash_matches_the_published_sha1_test_vectors() {
        assert_eq!(&*hex_digest("abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
        assert_eq!(&*hex_digest(""), "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709");
        assert_eq!(&*hex_digest("password"), PASSWORD_SHA1);
    }

    #[test]
    fn split_hash_splits_the_vector_at_five_characters() {
        let (prefix, suffix) = split_hash("password");
        assert_eq!(prefix.as_str(), &PASSWORD_SHA1[..5]);
        assert_eq!(&*suffix, PASSWORD_SUFFIX);
        assert_eq!(prefix.as_str().len(), 5);
        assert_eq!(suffix.len(), 35);
        // And the two halves really do reassemble into the vector, so neither
        // assertion above can be satisfied by an overlap or a gap.
        assert_eq!(format!("{prefix}{}", &*suffix), PASSWORD_SHA1);
    }

    #[test]
    fn breach_phrase_groups_the_count_and_gives_the_same_advice_at_every_size() {
        let small = breach_phrase(3);
        let large = breach_phrase(40_000);
        assert!(small.contains("3"), "{small}");
        assert!(large.contains("40,000"), "{large}");
        assert!(breach_phrase(PASSWORD_COUNT).contains("10,437,277"));
        // The advice, with the number stripped out, must be identical.
        let advice = |s: &str| s.split('(').next().unwrap().to_string() + s.split(')').nth(1).unwrap();
        assert_eq!(
            advice(&small),
            advice(&large),
            "the advice must not soften for a small count -- both mean 'change it'"
        );
        assert!(small.contains("Change this password"), "{small}");
    }

    // ---------------------------------------------------------- mockito seam

    #[test]
    fn the_production_endpoint_is_the_https_range_api() {
        assert_eq!(HIBP_RANGE_BASE, "https://api.pwnedpasswords.com/range");
    }

    #[test]
    fn a_present_suffix_over_the_wire_reports_breached() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/range/5BAA6")
            .with_status(200)
            .with_body(FIXTURE)
            .create();

        let suffix = Zeroizing::new(PASSWORD_SUFFIX.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Breached(PASSWORD_COUNT));
    }

    /// The absent suffix here is the **one-character near miss**, not the 35
    /// `Z`s: over the wire as well as in the parser, "not breached" has to
    /// mean all 35 characters were compared.
    #[test]
    fn an_absent_suffix_over_the_wire_reports_safe() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/range/5BAA6")
            .with_status(200)
            .with_body(FIXTURE)
            .create();

        let suffix = Zeroizing::new(NEAR_MISS_LAST.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Safe);
    }

    #[test]
    fn a_descriptive_user_agent_is_sent() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/range/5BAA6")
            .match_header("user-agent", USER_AGENT)
            .with_status(200)
            .with_body(FIXTURE)
            .create();

        let suffix = Zeroizing::new(PASSWORD_SUFFIX.to_string());
        check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        // `assert` fails unless a request matching the header arrived; the
        // status above would be `Unavailable` on a mismatch, so this is the
        // half that actually pins the header.
        m.assert();
        assert!(USER_AGENT.starts_with("Deskwarden/"), "{USER_AGENT}");
        assert!(
            USER_AGENT.len() > "Deskwarden/".len(),
            "the version half of the User-Agent is empty: {USER_AGENT}"
        );
    }

    /// **The request asks for HIBP's padding, and a padded answer still
    /// works.**
    ///
    /// Both halves matter and neither alone is enough. `m.assert()` proves
    /// the header went out; the status proves the decoys it brings back do
    /// not turn a genuine match into `Unavailable`, which is what the old
    /// `Err(Malformed("zero count"))` did to every padded response.
    #[test]
    fn the_request_asks_for_hibps_own_padding_and_survives_the_answer() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/range/5BAA6")
            .match_header("add-padding", "true")
            .with_status(200)
            .with_body(PADDED_FIXTURE)
            .create();

        let suffix = Zeroizing::new(PASSWORD_SUFFIX.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        m.assert();
        assert_eq!(
            status,
            BreachStatus::Breached(PASSWORD_COUNT),
            "the padding this crate asked for turned a real match into something else"
        );
    }

    /// And the same over the wire for a password that is genuinely not in the
    /// range: an all-decoy answer is `Safe`, not `Unavailable`.
    #[test]
    fn an_all_decoy_answer_over_the_wire_reports_safe() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/range/5BAA6")
            .with_status(200)
            .with_body(ALL_DECOY_FIXTURE)
            .create();

        let suffix = Zeroizing::new(PASSWORD_SUFFIX.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Safe);
    }

    /// **The privacy claim, as a test.**
    ///
    /// Read off the literal request head this crate put on a socket -- not off
    /// a mockito matcher, which can only tell you that something you already
    /// named was present.
    ///
    /// The assertion is an **exact allowlist**, not a hunt for the secret. The
    /// request line must be character-for-character
    /// `GET /range/{PREFIX} HTTP/1.1` -- no query string, no fragment, no
    /// extra path segment, no other method -- and every header must be one of
    /// a fixed set, matched on its name *and* on its exact value, each exactly
    /// once, with none missing.
    ///
    /// It **fails closed**: a header that is not on the list fails this test
    /// even if it carries nothing secret at all. That is the whole point. The
    /// previous version of this guard asked "does an 8-character run of the
    /// suffix appear anywhere in the head?", and a leak of *seven* characters
    /// in an invented header matched no 8-window and sailed through green --
    /// prefix plus seven is twelve hex characters, 48 bits of the SHA-1, which
    /// collapses the k-anonymity bucket from ~800 candidates to one. Narrowing
    /// the window is not the fix (a 2-character window false-positives against
    /// the prefix and against ordinary header text); enumerating what is
    /// allowed is.
    ///
    /// Every header this crate sends is a thing that left the machine, so a
    /// new one has to be added to this list deliberately, by someone reading
    /// this comment -- not discovered afterwards.
    #[test]
    fn the_request_head_carries_nothing_beyond_the_allowlist() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{FIXTURE}",
                    FIXTURE.len()
                )
                .as_bytes(),
            );
            let _ = stream.flush();
            String::from_utf8_lossy(&head).into_owned()
        });

        let (prefix, suffix) = split_hash("password");
        let status = check_prefix(
            &BaseUrl::loopback(port),
            &prefix,
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Breached(PASSWORD_COUNT), "the mock did answer");

        let head = server.join().expect("server thread");

        // Controls first: the head really was captured, really is a request
        // head, and really is complete -- otherwise every assertion below is
        // about an empty or truncated string.
        assert!(head.starts_with("GET "), "captured head is not a request head: {head:?}");
        assert!(head.ends_with("\r\n\r\n"), "the captured head is truncated: {head:?}");
        assert!(
            head.contains(prefix.as_str()),
            "the prefix is not in the head, so this is not the request under test: {head:?}"
        );

        let mut lines = head.trim_end_matches("\r\n\r\n").split("\r\n");

        // 1. The request line, exactly. A query string, a second path
        //    segment, a changed method or a changed version all land here.
        let request_line = lines.next().expect("a request head has a request line");
        assert_eq!(
            request_line,
            format!("GET /range/{prefix} HTTP/1.1"),
            "the request line is not exactly `GET /range/{{PREFIX}} HTTP/1.1`"
        );

        // 2. The headers. Name -> the one value that name may carry. `Host`
        //    is built from the port this test itself chose, so a rewritten or
        //    redirected Host fails too.
        let allowed: Vec<(&str, String)> = vec![
            ("host", format!("127.0.0.1:{port}")),
            ("accept", "*/*".to_string()),
            ("user-agent", USER_AGENT.to_string()),
            ("accept-encoding", "gzip".to_string()),
            // HIBP's own padding request. A constant; it says nothing about
            // the user, the password or the hash.
            ("add-padding", "true".to_string()),
        ];

        let mut seen: Vec<String> = Vec::new();
        for line in lines {
            assert!(!line.is_empty(), "a blank line inside the head: {head:?}");
            let Some((name, value)) = line.split_once(':') else {
                panic!("header line is not `Name: value`: {line:?}");
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            let Some((_, expected)) = allowed.iter().find(|(n, _)| *n == name) else {
                panic!(
                    "the request carried a header that is not on the allowlist: {line:?}\n\
                     Full head: {head:?}\n\
                     Everything this crate puts in a request head leaves the machine. If this \
                     header is meant to be sent, add it to the allowlist above and say why."
                );
            };
            assert_eq!(
                value, expected,
                "header {name:?} carried {value:?}, not the one value it is allowed to carry"
            );
            seen.push(name);
        }

        // The loop visited a non-zero, exact number of headers: every allowed
        // name present, none twice, nothing else. A head with no headers at
        // all, or one that smuggles a second copy of an allowed name, is not
        // silently fine.
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "a header name was sent twice: {seen:?}");
        let mut expected_names: Vec<String> =
            allowed.iter().map(|(n, _)| (*n).to_string()).collect();
        expected_names.sort();
        assert_eq!(
            sorted, expected_names,
            "the set of headers sent is not the allowlist: {head:?}"
        );

        // 3. Redundant, and cheap: the whole suffix is not in the head under
        //    either case. This cannot catch a partial leak -- that is what the
        //    allowlist above is for -- but it costs nothing and it is the
        //    claim in its bluntest form.
        let upper = head.to_ascii_uppercase();
        assert!(
            !upper.contains(&*suffix.to_ascii_uppercase()),
            "the whole 35-char suffix went on the wire: {head:?}"
        );
    }

    #[test]
    fn a_500_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(500).with_body(FIXTURE).create();

        let suffix = Zeroizing::new(NEAR_MISS_LAST.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Unavailable, "a 500 must never paint a green badge");
    }

    #[test]
    fn a_404_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(404).with_body("").create();

        let suffix = Zeroizing::new(NEAR_MISS_LAST.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Unavailable);
    }

    #[test]
    fn a_200_with_an_empty_body_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(200).with_body("").create();

        let suffix = Zeroizing::new(NEAR_MISS_LAST.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(server.socket_address().port()),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(
            status,
            BreachStatus::Unavailable,
            "an empty 200 is the shape that most easily reads as 'not breached'"
        );
    }

    /// A closed local port. No DNS lookup, no packet that leaves this machine.
    #[test]
    fn an_unreachable_host_reports_unavailable_not_safe() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let suffix = Zeroizing::new(NEAR_MISS_LAST.to_string());
        let status = check_prefix(
            &BaseUrl::loopback(port),
            &password_prefix(),
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Unavailable);
    }

    // -------------------------------------------------------------- hygiene

    /// The allocator probe, run the way `login_ui`'s own tests run it: the
    /// **control comes first**. A probe that reports clean while blind is this
    /// codebase's signature failure, so before asserting that hashing releases
    /// nothing, prove the instrument fires for a bare `String` holding the
    /// same bytes.
    ///
    /// The password is built and dropped outside the watched region -- the
    /// point is what `hex_digest`/`split_hash` do while it is alive, not what
    /// the test's own scaffolding does on the way out.
    #[test]
    fn hashing_a_password_does_not_release_it_to_the_allocator() {
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: an ordinary String's plaintext went past the allocator unnoticed, so the \
             assertion below is about an instrument that sees nothing"
        );

        let password = Zeroizing::new(
            String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8"),
        );
        let mut carried = None;
        assert!(
            !plaintext_reached_the_allocator(|| {
                carried = Some(split_hash(&password));
            }),
            "hashing the password released a copy of the plaintext to the allocator"
        );
        // The results are real, so the closure above cannot have been
        // optimised into nothing.
        let (prefix, suffix) = carried.expect("split_hash ran");
        assert_eq!(prefix.as_str().len(), 5);
        assert_eq!(suffix.len(), 35);
        assert!(
            !prefix.as_str().contains("des"),
            "control: the prefix is a hash, not the password"
        );
    }

    /// **What this cannot see, stated plainly:** the allocator probe's needle
    /// is `PROBE`, and the SHA-1 of `PROBE` is not `PROBE`, so the probe is
    /// blind to a leaked *hash* buffer. Nothing in this crate can watch that
    /// buffer's reallocation directly. The substitutes are this capacity pin
    /// and the source guard below.
    ///
    /// Capacity exactly 40 after 40 characters is only reachable by
    /// pre-reservation: a `String` grown from empty passes through 8, 16, 32,
    /// 64 -- never 40. So this fails for `String::new()` and for any
    /// `with_capacity` smaller than the hash.
    #[test]
    fn the_hex_buffer_never_reallocates() {
        let fresh = String::with_capacity(40);
        assert_eq!(fresh.capacity(), 40, "control: with_capacity(40) really reserves 40");

        let hex = hex_digest("password");
        assert_eq!(hex.len(), 40, "a SHA-1 is 40 hex characters");
        assert_eq!(
            hex.capacity(),
            40,
            "the hex buffer grew: a realloc handed a block holding a partial hash back to the \
             allocator without wiping it"
        );

        let (_, suffix) = split_hash("password");
        assert_eq!(suffix.len(), 35);
        assert_eq!(suffix.capacity(), 35, "the suffix buffer grew");
    }

    /// This module's own source, read off disk.
    fn this_module_source() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/breach.rs"))
            .expect("breach.rs is readable")
    }

    /// This module's **production code**: everything above the `cfg(test)`
    /// marker, with comment-only lines removed.
    ///
    /// Both halves are necessary and both are load-bearing:
    ///
    /// * Comment lines are dropped because the module docs *name* the things
    ///   the guards forbid ("no `log::`, no print, no debug-dump") -- a guard
    ///   that scans them fires on its own explanation.
    /// * The test module is dropped because the positive control for the
    ///   debug-dump needle has to spell the macro out to prove the needle
    ///   matches the compiler's own spelling of it.
    ///
    /// Neither exclusion weakens what the guards claim: the claim is about
    /// what ships, and a comment ships no bytes to a log file.
    ///
    /// The exclusions are themselves controlled -- see `assert_strip_worked`.
    fn this_module_production_code() -> String {
        let source = this_module_source();
        let end = source
            .find("#[cfg(test)]")
            .expect("breach.rs has a cfg(test) module");
        source[..end]
            .lines()
            .filter(|line| {
                let t = line.trim_start();
                !(t.starts_with("//") || t.starts_with("//!"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The control for [`this_module_production_code`]: prove the strip really
    /// removed something, really kept the code, really cut the tests off, and
    /// -- the part that is easy to forget -- that there is nothing *after* the
    /// test module for it to have missed.
    fn assert_strip_worked(code: &str) {
        let full = this_module_source();
        assert!(code.len() < full.len(), "the strip removed nothing at all");
        assert!(
            code.contains("pub fn check_prefix") && code.contains("pub fn split_hash"),
            "the strip removed the production code it was supposed to keep"
        );
        assert!(
            !code.contains("mod tests"),
            "the strip did not cut the test module off"
        );
        // Not "contains no `//` at all": the endpoint constant is a URL, and
        // that is code. Pin the actual prose instead -- the module doc names
        // every needle the guards forbid, so if that survived the strip the
        // guards would fire on their own explanation.
        assert!(
            full.contains("The hex is never formatted into any"),
            "control: the module doc line this checks for has been reworded"
        );
        assert!(
            !code.contains("The hex is never formatted into any"),
            "the strip left the module doc behind, so the guards can fire on prose"
        );

        // **Nothing lives below the cut.** The strip keeps everything above
        // the first cfg(test) marker, so a production item placed *after* the
        // test module would be invisible to every guard in this file --
        // silently, and green. Inside `mod tests` every item is indented, so
        // the only lines starting at column 0 below the cut are the marker,
        // the module's opening line, and its closing brace. Anything else
        // there is a top-level item that no guard is reading.
        let cut = full.find(&format!("#[cfg({})]", "test")).expect("cfg(test) marker");
        let below: Vec<&str> = full[cut..]
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with(char::is_whitespace))
            .collect();
        assert_eq!(
            below,
            vec![&format!("#[cfg({})]", "test")[..], "mod tests {", "}"],
            "there is top-level source below the cfg(test) marker; the guards in this file do \
             not read it"
        );

        // **The strip is line-based**, and drops only lines whose first
        // non-space characters are `//`. That is exact for this file because
        // this file has no block comments -- an assumption the guards were
        // relying on silently, and which is now enforced. A `/* ... */` block
        // leaves its contents in the "production" text, so the guards fire on
        // prose; a `//` written inside a block comment goes the other way and
        // hides a line from every one of them.
        //
        // What is NOT fixed here, said plainly: a `//` sequence inside a
        // *string literal* would still drop that line, and no line-based
        // strip can tell the two apart. Nothing in this file has one; the
        // guards that matter are absence-guards, whose failure direction from
        // an unstripped comment is to fire rather than to miss; and the real
        // fix is a Rust lexer, which is not worth carrying here.
        for marker in [concat!("/", "*"), concat!("*", "/")] {
            assert!(
                !code.contains(marker),
                "breach.rs production code contains {marker:?}. Every guard in this file reads \
                 a line-based strip that removes only `//` lines, so a block comment is scanned \
                 as though it were code -- and can hide code from the scan"
            );
        }
    }

    /// Every `.rs` file under `src/`, as (path relative to `src/`, contents) --
    /// the same walk `http_agent`'s guard uses, and walked off disk for the
    /// same reason: the defect being guarded against is a *future* module, and
    /// a hand-written list is a list that module would not be on.
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

    /// The hash is only ever built in a wiped-on-drop buffer at its exact
    /// final size.
    ///
    /// Needles are split across `concat!` arguments so they cannot match their
    /// own declaration -- a trap this crate has shipped for real -- and every
    /// one of them is positively controlled: the two that must be present are
    /// asserted present in live code here, and the one that must be absent is
    /// asserted present in a file that legitimately uses it, so a typo cannot
    /// leave the guard passing on a mutant.
    #[test]
    fn the_hash_is_only_ever_built_in_a_zeroizing_buffer() {
        const HEX_BUF: &str = concat!("Zeroizing::new(String::with_", "capacity(PREFIX_LEN");
        const SUFFIX_BUF: &str = concat!("Zeroizing::new(String::with_", "capacity(SUFFIX_LEN))");
        const DIGEST_BUF: &str = concat!("Zeroizing::new([0u8; ", "20])");
        const BARE_STRING: &str = concat!("String::", "new()");

        let source = this_module_production_code();
        assert_strip_worked(&source);

        // Positive controls: these three spellings match live code right now.
        for needle in [HEX_BUF, SUFFIX_BUF, DIGEST_BUF] {
            assert!(
                source.contains(needle),
                "needle {needle:?} no longer matches live code -- either the buffer changed \
                 shape (which is the defect) or this guard has gone blind"
            );
        }
        // Positive control for the *negative* needle: it is a real spelling
        // that really does occur in this crate, just not here.
        let elsewhere = crate_source_files()
            .into_iter()
            .filter(|(path, _)| path != "breach.rs")
            .any(|(_, text)| text.contains(BARE_STRING));
        assert!(
            elsewhere,
            "needle {BARE_STRING:?} occurs nowhere in the crate, so asserting its absence here \
             proves nothing"
        );

        assert!(
            !source.contains(BARE_STRING),
            "breach.rs builds a String without reserving its capacity. A growing String reallocs, \
             and realloc returns the old block -- holding a partial password hash -- unwiped"
        );
    }

    /// Nothing in this module logs, prints, panics with data, or debug-dumps.
    ///
    /// **The needles are the bare macro tokens, not fully-qualified paths.**
    /// The previous version listed `log::debug!` and five siblings, and
    /// `use log::debug;` with `debug!("breach: computed hex {}", &*hex);` --
    /// the most ordinary spelling in Rust -- walked straight past it at 1684
    /// green, putting the complete 40-character hash in the log file. So did
    /// `print!` and `eprint!` (no `ln`), which were not covered at all, and
    /// `panic!`, `tracing::`, and a `write!` to `stderr`.
    ///
    /// Every needle is positively controlled. The macro tokens are controlled
    /// against `stringify!` over a real invocation -- `stringify!` does not
    /// expand its argument, so that is the compiler's own tokenisation of the
    /// name rather than a second guess at it -- and the path needles against
    /// files in this crate that really do use them.
    ///
    /// `format!` and `write!` cannot be banned: the URL, the breach phrase and
    /// the hex buffer are built with them. They are **pinned at an exact
    /// count** instead, so a third one has to be added here deliberately --
    /// and the one `write!` is pinned to its target as well, because the
    /// thing most worth formatting in this file is the hash.
    #[test]
    fn the_breach_module_never_logs() {
        // Bare macro tokens. `print!` is not a substring of `println!`
        // ("printl"), so both are needed; likewise `eprint!`/`eprintln!`.
        const BANNED_MACROS: [&str; 14] = [
            concat!("debu", "g!"),
            concat!("inf", "o!"),
            concat!("war", "n!"),
            concat!("erro", "r!"),
            concat!("trac", "e!"),
            concat!("prin", "t!"),
            concat!("printl", "n!"),
            concat!("eprin", "t!"),
            concat!("eprintl", "n!"),
            concat!("db", "g!"),
            concat!("pani", "c!"),
            concat!("unreachabl", "e!"),
            concat!("tod", "o!"),
            concat!("writel", "n!"),
        ];
        // Paths and sinks that reach a log file or a terminal without any of
        // the macro tokens above appearing verbatim.
        const BANNED_PATHS: [&str; 6] = [
            concat!("log", "::"),
            concat!("tracin", "g::"),
            concat!("slo", "g::"),
            concat!("stder", "r"),
            concat!("stdou", "t"),
            concat!(".expec", "t("),
        ];

        // Positive control 1: each macro needle matches the compiler's own
        // spelling of that macro. `stringify!` does not expand what it is
        // given, so these are token streams, not expansions.
        let spellings = [
            stringify!(debug!(1)),
            stringify!(info!(1)),
            stringify!(warn!(1)),
            stringify!(error!(1)),
            stringify!(trace!(1)),
            stringify!(print!(1)),
            stringify!(println!(1)),
            stringify!(eprint!(1)),
            stringify!(eprintln!(1)),
            stringify!(dbg!(1)),
            stringify!(panic!(1)),
            stringify!(unreachable!(1)),
            stringify!(todo!(1)),
            stringify!(writeln!(1)),
        ];
        assert_eq!(
            spellings.len(),
            BANNED_MACROS.len(),
            "a banned macro has no positive control"
        );
        for (needle, spelling) in BANNED_MACROS.iter().zip(spellings) {
            assert!(
                spelling.contains(needle),
                "needle {needle:?} does not match {spelling:?}, the compiler's own spelling of \
                 that macro -- the needle has gone blind"
            );
        }

        // Positive control 2: the needles match the exact survivor they were
        // written for -- the spelling that passed 1684 green.
        assert!(
            "use log::debug;".contains(BANNED_PATHS[0]),
            "the log-path needle no longer matches `use log::debug;`"
        );
        assert!(
            "    debug!(\"breach: computed hex {}\", &*hex);".contains(BANNED_MACROS[0]),
            "the debug needle no longer matches the survivor it was written for"
        );
        assert!(
            "    print!(\"{}\", &*hex);".contains(BANNED_MACROS[5]),
            "the print needle no longer matches the survivor it was written for"
        );

        // Positive control 3: the path needles match real code elsewhere in
        // this crate. Not all six do -- `slog::` is not a dependency -- so
        // the assertion is on a non-zero, expected count rather than on all.
        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());
        let controlled = BANNED_PATHS
            .iter()
            .filter(|needle| {
                files
                    .iter()
                    .filter(|(path, _)| path != "breach.rs")
                    .any(|(_, text)| text.contains(**needle))
            })
            .count();
        assert!(
            controlled >= 3,
            "only {controlled} of the {} path needles match anything in this crate, so most of \
             them prove nothing by their absence here",
            BANNED_PATHS.len()
        );

        let source = this_module_production_code();
        assert_strip_worked(&source);
        let found: Vec<&str> = BANNED_MACROS
            .into_iter()
            .chain(BANNED_PATHS)
            .filter(|needle| source.contains(needle))
            .collect();
        assert!(
            found.is_empty(),
            "breach.rs logs, prints or panics with data: {found:?}. A log line here is the \
             password's hash in a file on disk, and there is no such thing as a safe one"
        );

        // The two that cannot be banned, pinned instead.
        assert_eq!(
            source.matches(concat!("forma", "t!")).count(),
            3,
            "a new `format!` in breach.rs production code. The three are the range URL, the \
             breach phrase and `BaseUrl::loopback`, and the thing most worth formatting here is \
             the hash -- so a fourth has to be added to this count deliberately"
        );
        // Each of the three pinned to what it may build, so the count cannot
        // be satisfied by a *different* three.
        for target in [
            concat!("format!(\"{}/{}\", base_url.as_str(), ", "prefix.as_str())"),
            concat!("\"Found in a known data breach ({} times).", " Change this password.\","),
            concat!("format!(\"http://127.0.0.1:{port}", "/range\")"),
        ] {
            assert!(
                source.contains(target),
                "the pinned `format!` {target:?} is gone, so the count of 3 is now pinning \
                 something else"
            );
        }
        assert_eq!(
            source.matches(concat!("writ", "e!")).count(),
            1,
            "a new `write!` in breach.rs production code: the hex buffer is the only target"
        );
        assert!(
            source.contains(concat!("write!(&mut *hex, ", "\"{byte:02X}\")")),
            "the one `write!` no longer writes into the hex buffer"
        );
    }

    /// A [`Prefix`] is the first five characters of the hash, always, because
    /// nothing else in the crate can build one.
    ///
    /// The compiler is what enforces this: the field is private, `split_hash`
    /// is the only constructor, and `check_prefix` takes a `&Prefix`. Before
    /// the newtype, `check_prefix(base, prefix: &str, ...)` sat next to
    /// `suffix: &Zeroizing<String>` as two bare strings, and a call site that
    /// passed the full 40-character hex would have put the whole hash in the
    /// URL with nothing red -- the privacy test builds its own inputs from
    /// `split_hash` and never sees a call site. That call no longer compiles.
    /// This test is only the arithmetic.
    #[test]
    fn a_prefix_is_always_the_first_five_characters_of_the_hash() {
        for password in ["", "password", "abc", "correct horse battery staple", "\u{1F510}\u{1F510}"]
        {
            let (prefix, suffix) = split_hash(password);
            let hex = hex_digest(password);
            assert_eq!(prefix.as_str().len(), PREFIX_LEN, "{password:?}");
            assert_eq!(prefix.as_str(), &hex[..PREFIX_LEN], "{password:?}");
            assert_eq!(
                format!("{}{}", prefix.as_str(), &*suffix).as_str(),
                hex.as_str(),
                "prefix and suffix do not reassemble into the hash for {password:?}"
            );
        }
        assert_eq!(
            password_prefix().as_str(),
            &PASSWORD_SHA1[..PREFIX_LEN],
            "every mockito mock in this file is keyed on this path"
        );
        assert_eq!(password_prefix().as_str(), "5BAA6", "and that path is /range/5BAA6");
    }

    /// The private field and the single construction site, source-pinned.
    ///
    /// The compiler enforces the invariant *given* the field stays private
    /// and `split_hash` stays the only constructor. A `pub` on the field, or
    /// a second `Prefix(...)` somewhere that does not slice at `PREFIX_LEN`,
    /// would reopen exactly the hole the type closes -- and the compiler
    /// would have nothing to say about either.
    #[test]
    fn the_prefix_field_is_private_and_built_in_exactly_one_place() {
        const DECL: &str = concat!("pub struct Pre", "fix(String);");
        const CTOR: &str = concat!("Pre", "fix(hex[..PREFIX_LEN].to_string())");
        const NAME: &str = concat!("Pre", "fix(");

        let production = this_module_production_code();
        assert_strip_worked(&production);
        assert!(
            production.contains(DECL),
            "the declaration is no longer {DECL:?}. A `pub` on the tuple field would let any \
             call site in the crate build a Prefix out of the whole hash"
        );
        assert!(
            production.contains(CTOR),
            "the one construction site is no longer {CTOR:?}, so it may no longer be slicing \
             the hash at PREFIX_LEN"
        );
        assert_eq!(
            production.matches(NAME).count(),
            2,
            "a Prefix is constructed somewhere other than split_hash. The two expected \
             occurrences are the declaration and split_hash's own"
        );
    }

    /// **`base_url` was the unclosed half of the door `Prefix` closed.**
    ///
    /// `check_prefix` composes its URL out of exactly two values. The newtype
    /// review closed one of them and left the other a bare `&str`, sitting
    /// next to a `pub(crate) hex_digest`, so this compiled and put the whole
    /// 40-character hash in the path:
    ///
    /// ```text
    /// check_prefix(&format!("{RANGE_BASE}/{}", hex_digest(pw)), &prefix, ..)
    /// ```
    ///
    /// `the_request_head_carries_nothing_beyond_the_allowlist` cannot catch
    /// that and never could: it supplies the base itself and then asserts the
    /// request line it computes *from its own input*, so a path chosen by the
    /// caller is invisible to it by construction. Only the type can close it.
    ///
    /// The compiler does the enforcing, *given* the field stays private and
    /// the constructors keep taking nothing and a `u16`. A `pub` on the
    /// field, or a third constructor that takes a `&str`, reopens the hole
    /// with nothing to say about it -- which is what this test is for.
    #[test]
    fn the_base_url_field_is_private_and_built_from_no_caller_supplied_text() {
        const DECL: &str = concat!("pub struct BaseU", "rl(String);");
        const NAME: &str = concat!("BaseU", "rl(");
        const PARAM: &str = concat!("base_url: &BaseU", "rl,");
        // Split three ways, not two: the endpoint constant's own name is
        // counted in the test half by `no_test_in_this_file_can_reach_the_
        // real_api`, and spelling it whole here would trip that count.
        const PRODUCTION_CTOR: &str = concat!("BaseU", "rl(HIBP_RANGE_", "BASE.to_string())");
        const LOOPBACK_CTOR: &str =
            concat!("BaseU", "rl(format!(\"http://127.0.0.1:{port}/range\"))");

        let production = this_module_production_code();
        assert_strip_worked(&production);

        assert!(
            production.contains(DECL),
            "the declaration is no longer {DECL:?}. A `pub` on the tuple field would let any \
             call site in the crate build a BaseUrl out of the whole hash"
        );
        assert!(
            production.contains(PARAM),
            "check_prefix no longer takes a &BaseUrl, so its URL is composed from caller text \
             again"
        );

        // The two signatures, spelled out. Neither takes anything a caller
        // could put hash material into: one takes no argument at all, and the
        // other takes a u16, which cannot hold a hex character.
        assert!(
            production.contains(concat!("pub fn producti", "on() -> Self")),
            "BaseUrl::production no longer takes zero arguments"
        );
        assert!(
            production.contains(concat!("pub fn loopb", "ack(port: u16) -> Self")),
            "BaseUrl::loopback no longer takes exactly a u16 port -- a &str or a String here is \
             the whole hole reopened"
        );

        // And the bodies: whatever each one builds, it is not something a
        // caller handed it.
        assert!(
            production.contains(PRODUCTION_CTOR),
            "BaseUrl::production no longer builds itself out of the pinned constant"
        );
        assert!(
            production.contains(LOOPBACK_CTOR),
            "BaseUrl::loopback no longer builds a fixed loopback URL out of nothing but the port"
        );

        assert_eq!(
            production.matches(NAME).count(),
            3,
            "a BaseUrl is constructed somewhere other than its two constructors. The three \
             expected occurrences are the declaration, production() and loopback()"
        );
    }

    /// **The other 35 characters cannot be obtained outside this module.**
    ///
    /// `hex_digest` returns the complete 40-character hash and was
    /// `pub(crate)` with no caller outside `split_hash`. What it had instead
    /// of a caller was a use: any module in the crate could ask it for the
    /// whole hash and hand the result to something that writes text. Both
    /// newtypes in this file exist to make that unspellable, and a
    /// crate-visible function returning the hash is the plainest way past
    /// them.
    ///
    /// It is private now. Two halves, because either alone can go blind: the
    /// visibility is read off this file's production source, and the name is
    /// searched for across every other file in `src/` -- because a `pub use`
    /// or a re-export somewhere else would restore it without changing a
    /// character here.
    #[test]
    fn the_whole_hash_is_not_reachable_outside_this_module() {
        const FN_NAME: &str = concat!("hex_dig", "est");

        let production = this_module_production_code();
        assert_strip_worked(&production);

        // Positive control: the needle matches the live declaration.
        assert!(
            production.contains(&format!("fn {FN_NAME}(password: &str)")),
            "needle {FN_NAME:?} no longer matches the live declaration, so this guard is blind"
        );
        for visibility in ["pub fn ", "pub(crate) fn ", "pub(super) fn ", "pub(in "] {
            assert!(
                !production.contains(&format!("{visibility}{FN_NAME}")),
                "{FN_NAME} is {visibility:?} again. It hands out all 40 characters, which is \
                 exactly what Prefix and BaseUrl exist to make unspellable"
            );
        }

        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());
        let this = files
            .iter()
            .find(|(path, _)| path == "breach.rs")
            .expect("the walk did not reach breach.rs");
        assert!(this.1.contains(FN_NAME), "control: the walk's copy of breach.rs is not it");

        let offenders: Vec<&str> = files
            .iter()
            .filter(|(path, _)| path != "breach.rs")
            .filter(|(_, text)| text.contains(FN_NAME))
            .map(|(path, _)| path.as_str())
            .collect();
        assert!(
            offenders.is_empty(),
            "{FN_NAME} is named outside breach.rs: {offenders:?}. Nothing outside this module \
             may hold the whole hash"
        );
    }

    /// SHA-1 lives in this module and nowhere else.
    ///
    /// It is the API's index, not a security primitive, and the way this goes
    /// wrong is not someone deleting it -- it is someone finding it convenient
    /// somewhere it is not an index. The walk is positively controlled twice:
    /// it must have reached `breach.rs`, and `breach.rs` must contain the
    /// needle. A walk that visits zero files passes green otherwise.
    #[test]
    fn sha1_is_confined_to_the_breach_module() {
        const TYPE_NAME: &str = concat!("Sha", "1");
        const CRATE_PATH: &str = concat!("sha", "1::");

        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());
        let this = files
            .iter()
            .find(|(path, _)| path == "breach.rs")
            .expect("the walk did not reach breach.rs");
        assert!(
            this.1.contains(TYPE_NAME) && this.1.contains(CRATE_PATH),
            "needles {TYPE_NAME:?}/{CRATE_PATH:?} no longer match the one module that uses them"
        );

        let mut offenders = Vec::new();
        for (path, text) in &files {
            if path == "breach.rs" {
                continue;
            }
            for needle in [TYPE_NAME, CRATE_PATH] {
                if text.contains(needle) {
                    offenders.push(format!("{path}: {needle}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "SHA-1 escaped the breach module: {offenders:?}. Here it is Have I Been Pwned's \
             index; anywhere else it would be a security primitive, and it is not one"
        );
    }

    /// `BreachStatus` must not gain a `Default` -- in **any** spelling.
    ///
    /// `unwrap_or_default()` on a missing status would silently turn "we could
    /// not check" into `Safe`, a green badge on a password nobody looked at.
    /// It is the single highest-consequence bug this feature can have.
    ///
    /// The guard is a **total, case-insensitive ban on the token**, not a
    /// pattern match on one line. The previous version read exactly
    /// `lines[decl - 1]` and looked for `derive` there. Rust permits more than
    /// one `#[derive]` on an item, so a bare `#[derive(Default)]` stacked
    /// *above* the real derive list was invisible and passed green -- as did
    /// `impl std::default::Default for` and `impl Default  for` with two
    /// spaces. Chasing spellings loses that race by construction.
    ///
    /// The token has no legitimate use anywhere in this module's production
    /// code, and no spelling of the hazard omits it: the derive, the
    /// hand-written impl at any path, the `#[default]` variant attribute and
    /// `unwrap_or_default()` all contain `default` under an ASCII-lowercase
    /// fold. So the guard is: it appears nowhere at all.
    #[test]
    fn breach_status_has_no_default_in_any_spelling() {
        // Folded to lowercase, so one needle covers `Default`, `default` and
        // any mixture. Split across `concat!` so it cannot match itself.
        const DEFAULT: &str = concat!("def", "ault");

        // Positive control 1: the needle matches every real spelling of the
        // hazard, including the two-derive stack and the two-space impl that
        // defeated the previous guard, and the lowercase forms an
        // uppercase-only needle would miss.
        for spelling in [
            "#[derive(Default)]",
            "#[derive(Debug, Default, Clone)]",
            "#[derive(Default)]\r\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
            "impl Default for BreachStatus {",
            "impl Default  for BreachStatus {",
            "impl std::default::Default for BreachStatus {",
            "impl core::default::Default for BreachStatus {",
            "impl ::std::default::Default for BreachStatus {",
            "impl<'a> Default for BreachStatus {",
            "    #[default]\r\n    Safe,",
            "status.unwrap_or_default()",
            "BreachStatus::default()",
        ] {
            assert!(
                spelling.to_ascii_lowercase().contains(DEFAULT),
                "the {DEFAULT:?} needle does not match {spelling:?}, which is a real spelling of \
                 the hazard -- the needle has gone blind"
            );
        }

        // Positive control 2: the needle matches real code somewhere else in
        // this crate, so asserting its absence here is not vacuous.
        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());
        let elsewhere = files
            .iter()
            .filter(|(path, _)| path != "breach.rs")
            .any(|(_, text)| text.to_ascii_lowercase().contains(DEFAULT));
        assert!(
            elsewhere,
            "needle {DEFAULT:?} matches nothing else in this crate, so asserting its absence \
             here proves nothing"
        );

        let production = this_module_production_code().to_ascii_lowercase();
        assert_strip_worked(&this_module_production_code());
        assert!(
            !production.contains(DEFAULT),
            "the token {DEFAULT:?} appears in breach.rs production code. Nothing here needs it, \
             and every form it can take is a Default for BreachStatus -- which makes \
             `unwrap_or_default()` read 'we could not check' as a green badge"
        );
    }
    /// The cache's state machine, driven entirely by injected stubs.
    ///
    /// **No test in here reaches the network.** `BreachCache::live` -- the one
    /// thing that names `live_check`, which is the one thing that names the
    /// production endpoint constant outside the endpoint pin -- is never
    /// constructed below; `no_cache_test_can_reach_the_real_api` is the guard
    /// that says so out loud, on this module's own test source.
    mod cache {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Condvar, Mutex};
        use std::time::Instant;

        /// A context with no fonts and no frame. `status` paints nothing; it
        /// only ever asks for a repaint.
        fn ctx() -> egui::Context {
            egui::Context::default()
        }

        /// A context that is **not** already asking to be repainted.
        ///
        /// A brand-new `Context` is: it has never run a pass, so it wants one.
        /// Measured, not assumed -- the first version of the repaint test used
        /// a fresh context and its own control caught that the assertion was
        /// true before `status` was ever called, which would have made the
        /// test green with the repaint request deleted.
        ///
        /// Empty passes settle it. Each one has no input, draws nothing, and
        /// clears the outstanding request; the loop is bounded and the caller
        /// asserts the result rather than trusting it.
        fn idle_ctx() -> egui::Context {
            let ctx = ctx();
            for _ in 0..8 {
                ctx.begin_pass(egui::RawInput::default());
                let _ = ctx.end_pass();
                if !ctx.has_requested_repaint() {
                    break;
                }
            }
            assert!(
                !ctx.has_requested_repaint(),
                "control: a context that has run eight empty passes still wants a repaint, so \
                 `has_requested_repaint` cannot be used to observe one being asked for"
            );
            ctx
        }

        /// How long a test will wait for a worker thread before calling it a
        /// hang. Generous, because it is only ever reached when something is
        /// actually broken.
        const PATIENCE: Duration = Duration::from_secs(10);

        /// A stub check that counts its invocations and always answers
        /// `answer`. The counter is per-test state, which is the whole reason
        /// [`BreachCheck`] is a boxed closure and not a `fn` pointer.
        fn counting_stub(answer: BreachStatus) -> (BreachCheck, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let mine = Arc::clone(&calls);
            let check: BreachCheck = Arc::new(move |_: &Prefix, _: &Zeroizing<String>| {
                mine.fetch_add(1, Ordering::SeqCst);
                answer
            });
            (check, calls)
        }

        /// A stub that counts its invocation, then **blocks until the gate is
        /// opened**, then answers `Breached(n)` for the n-th invocation.
        ///
        /// Two properties the pending tests need and cannot get any other way:
        /// a worker that is genuinely still out when the assertion runs, and
        /// answers that are distinguishable *by which worker produced them* --
        /// so "the first worker's answer landed after the clear" is a thing a
        /// test can see rather than infer.
        type Gate = Arc<(Mutex<bool>, Condvar)>;

        fn gated_stub() -> (BreachCheck, Arc<AtomicUsize>, Gate) {
            let calls = Arc::new(AtomicUsize::new(0));
            let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
            let mine = Arc::clone(&calls);
            let held = Arc::clone(&gate);
            let check: BreachCheck = Arc::new(move |_: &Prefix, _: &Zeroizing<String>| {
                let nth = mine.fetch_add(1, Ordering::SeqCst) + 1;
                let (lock, cvar) = &*held;
                let mut open = match lock.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                while !*open {
                    open = match cvar.wait(open) {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                }
                BreachStatus::Breached(nth as u64)
            });
            (check, calls, gate)
        }

        fn open(gate: &Gate) {
            let (lock, cvar) = &**gate;
            let mut open = match lock.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *open = true;
            cvar.notify_all();
        }

        /// Wait until the stub has been entered exactly `n` times.
        ///
        /// A worker is a thread, so "one worker was started" is observable a
        /// moment after `status` returns, not during it. This is the only
        /// waiting any test here does, and it is bounded.
        fn wait_for_calls(calls: &AtomicUsize, n: usize) {
            let deadline = Instant::now() + PATIENCE;
            while calls.load(Ordering::SeqCst) < n {
                assert!(
                    Instant::now() < deadline,
                    "the stub was entered {} times, not {n}: no worker was started",
                    calls.load(Ordering::SeqCst)
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Poll `status` until it stops saying `Pending`, which is the
        /// promotion path under test. Bounded; a hang is a failure, not a wait.
        fn promote(cache: &mut BreachCache, ctx: &egui::Context, password: &str) -> BreachStatus {
            let deadline = Instant::now() + PATIENCE;
            loop {
                let answered = cache.status(ctx, password);
                if answered != BreachStatus::Pending {
                    return answered;
                }
                assert!(
                    Instant::now() < deadline,
                    "the answer was never promoted out of Pending"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Whether the one entry for `password`'s prefix is pending.
        fn is_pending(cache: &BreachCache, password: &str) -> bool {
            let (prefix, _) = split_hash(password);
            matches!(cache.entries.get(prefix.as_str()), Some(Entry::Pending(_)))
        }

        fn ready_status(cache: &BreachCache, password: &str) -> Option<BreachStatus> {
            let (prefix, _) = split_hash(password);
            match cache.entries.get(prefix.as_str()) {
                Some(Entry::Ready(answered)) => Some(*answered),
                _ => None,
            }
        }

        const PW: &str = "password";
        const OTHER: &str = "correct horse battery staple";

        #[test]
        fn a_first_lookup_answers_pending_and_starts_exactly_one_worker() {
            let ctx = ctx();
            let (check, calls) = counting_stub(BreachStatus::Safe);
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            wait_for_calls(&calls, 1);
            // Give a second worker every chance to show up before asserting
            // that none did.
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "one lookup started more than one worker"
            );
            assert_eq!(cache.entries.len(), 1, "one lookup made more than one entry");
        }

        #[test]
        fn a_second_lookup_for_the_same_prefix_starts_no_second_worker() {
            let ctx = ctx();
            let (check, calls) = counting_stub(BreachStatus::Safe);
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            wait_for_calls(&calls, 1);
            // Several more frames' worth of asking, exactly as a UI would.
            for _ in 0..5 {
                let _ = cache.status(&ctx, PW);
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "asking again started another worker: the cache is not being consulted"
            );
            assert_eq!(cache.entries.len(), 1);
        }

        #[test]
        fn an_answer_is_promoted_to_ready_and_then_served_from_cache() {
            let ctx = ctx();
            let (check, calls) = counting_stub(BreachStatus::Breached(42));
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            assert!(is_pending(&cache, PW), "the miss did not leave a pending entry");

            assert_eq!(promote(&mut cache, &ctx, PW), BreachStatus::Breached(42));
            assert_eq!(
                ready_status(&cache, PW),
                Some(BreachStatus::Breached(42)),
                "the promotion did not store the answer it just handed out"
            );

            for _ in 0..5 {
                assert_eq!(cache.status(&ctx, PW), BreachStatus::Breached(42));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1, "a cache hit started a worker");
        }

        /// A lookup in flight when the vault locks must not be able to land.
        ///
        /// The two answers are told apart by *which worker* produced them --
        /// the gated stub answers `Breached(n)` for its n-th call -- so a
        /// receiver that survived `clear` shows up as `Breached(1)` where only
        /// `Breached(2)` is correct. Asserting the map is empty would not catch
        /// that on its own: an entry re-inserted from a stale channel is an
        /// entry, and it would look exactly like a fresh one.
        #[test]
        fn clear_drops_pending_entries_too() {
            let ctx = ctx();
            let (check, calls, gate) = gated_stub();
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            wait_for_calls(&calls, 1);
            assert!(
                is_pending(&cache, PW),
                "control: the entry under test is not pending, so this test proves nothing"
            );
            assert_eq!(ready_status(&cache, PW), None, "control: the entry is already ready");

            cache.clear();
            assert!(
                cache.entries.is_empty(),
                "clear left {} entries behind",
                cache.entries.len()
            );

            // The first worker now finishes into a dropped receiver.
            open(&gate);
            std::thread::sleep(Duration::from_millis(20));
            assert!(
                cache.entries.is_empty(),
                "an answer from before the clear landed in the cache after it"
            );

            // A fresh lookup must be a fresh worker, and must get the *second*
            // worker's answer.
            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            wait_for_calls(&calls, 2);
            let answered = promote(&mut cache, &ctx, PW);
            assert_eq!(
                answered,
                BreachStatus::Breached(2),
                "the answer served after the lock came from the worker that was out *during* it"
            );
        }

        #[test]
        fn an_unavailable_answer_is_cached_as_unavailable_and_never_as_safe() {
            let ctx = ctx();
            let (check, calls) = counting_stub(BreachStatus::Unavailable);
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            let answered = promote(&mut cache, &ctx, PW);
            assert_eq!(answered, BreachStatus::Unavailable);
            assert_ne!(
                answered,
                BreachStatus::Safe,
                "a lookup that could not be made was reported as a password nobody found"
            );
            assert_eq!(ready_status(&cache, PW), Some(BreachStatus::Unavailable));

            for _ in 0..5 {
                assert_eq!(cache.status(&ctx, PW), BreachStatus::Unavailable);
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn two_different_prefixes_get_two_workers() {
            // The control the count depends on. If these two passwords shared
            // a prefix, "two workers" would be the defect, not the assertion.
            let (a, _) = split_hash(PW);
            let (b, _) = split_hash(OTHER);
            assert_ne!(
                a.as_str(),
                b.as_str(),
                "the two fixtures share a prefix, so this test cannot mean what it says"
            );

            let ctx = ctx();
            let (check, calls) = counting_stub(BreachStatus::Safe);
            let mut cache = BreachCache::new(check);

            assert_eq!(cache.status(&ctx, PW), BreachStatus::Pending);
            assert_eq!(cache.status(&ctx, OTHER), BreachStatus::Pending);
            wait_for_calls(&calls, 2);
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(calls.load(Ordering::SeqCst), 2);

            let mut keys: Vec<&str> = cache.entries.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            let mut expected = [a.as_str(), b.as_str()];
            expected.sort_unstable();
            assert_eq!(keys, expected.to_vec());
        }

        /// Without this the badge never appears: an answer arriving on a
        /// channel is not egui input, so nothing wakes the UI to read it.
        #[test]
        fn a_pending_entry_asks_for_a_repaint() {
            let (check, calls, gate) = gated_stub();
            let mut cache = BreachCache::new(check);

            // The miss.
            let first = idle_ctx();
            assert_eq!(cache.status(&first, PW), BreachStatus::Pending);
            assert!(
                first.has_requested_repaint(),
                "the frame that started the lookup asked for no follow-up frame"
            );
            wait_for_calls(&calls, 1);

            // A later frame, still pending: this arm needs the request just as
            // much, and it is a different line of code.
            let later = idle_ctx();
            assert!(is_pending(&cache, PW), "control: the entry is no longer pending");
            assert_eq!(cache.status(&later, PW), BreachStatus::Pending);
            assert!(
                later.has_requested_repaint(),
                "a frame that found the lookup still out asked for no follow-up frame"
            );

            // A settled entry has nothing to wait for.
            open(&gate);
            let _ = promote(&mut cache, &ctx(), PW);
            let settled = idle_ctx();
            assert_ne!(cache.status(&settled, PW), BreachStatus::Pending);
            assert!(
                !settled.has_requested_repaint(),
                "a cache hit is still asking for repaints, so the window never idles"
            );
        }

        /// The map is keyed on the five characters that are allowed to leave
        /// this machine, and on nothing longer.
        ///
        /// A map keyed on the whole hash keeps every password's complete SHA-1
        /// alive, in a plain `String`, for the length of the session -- to
        /// answer a question that is only ever asked about the first five
        /// characters. It would also make the cache useless by construction:
        /// no two lookups would ever share a key.
        #[test]
        fn the_cache_key_is_the_prefix_and_never_the_whole_hash() {
            let ctx = ctx();
            let (check, _) = counting_stub(BreachStatus::Safe);
            let mut cache = BreachCache::new(check);
            let _ = cache.status(&ctx, PW);

            let (prefix, suffix) = split_hash(PW);
            let hex = hex_digest(PW);
            assert_eq!(suffix.len(), SUFFIX_LEN, "control: the fixture's suffix is the wrong size");

            let keys: Vec<&str> = cache.entries.keys().map(|k| k.as_str()).collect();
            assert_eq!(keys, vec![prefix.as_str()]);
            for key in &keys {
                assert_eq!(key.len(), PREFIX_LEN, "key {key:?} is not five characters");
                assert!(
                    !key.contains(&*suffix),
                    "key {key:?} carries the 35 characters that must not leave this machine"
                );
                assert_ne!(*key, hex.as_str(), "the cache is keyed on the whole hash");
            }
        }

        /// No test in this module can reach Have I Been Pwned.
        ///
        /// The live check is reachable from exactly one constructor, and a
        /// test that called it would make a real request from the test suite.
        /// This reads the file's own test source rather than trusting the list
        /// above to stay complete, and is controlled twice: the needle must
        /// match the one production site that legitimately uses it, and the
        /// test text must be the text that really contains these tests.
        #[test]
        fn no_test_in_this_file_can_reach_the_real_api() {
            const LIVE: &str = concat!("BreachCache::li", "ve(");
            const ENDPOINT: &str = concat!("HIBP_RANGE_", "BASE");
            // The neighbour the BaseUrl fix created. The live base
            // constructor is public, takes no argument, names the endpoint,
            // and `check_prefix` takes one -- so a test that builds it has a
            // two-line path to api.pwnedpasswords.com that names neither the
            // constant nor the live cache. It is banned here for the same
            // reason both of those are.
            const LIVE_BASE: &str = concat!("BaseUrl::producti", "on(");

            let full = this_module_source();
            let cut = full
                .find(&format!("#[cfg({})]", "test"))
                .unwrap_or(full.len());
            let (production, tests) = full.split_at(cut);

            // Controls: both needles match live production code right now, so
            // asserting their absence from the tests is not an assertion about
            // a typo.
            assert!(
                production.contains(concat!("pub fn li", "ve() -> Self")),
                "control: the live constructor has been renamed, so {LIVE:?} is blind"
            );
            assert!(
                production.contains(ENDPOINT),
                "control: {ENDPOINT:?} no longer names anything in production"
            );
            assert!(
                tests.contains(concat!("fn clear_drops_pending_entries", "_too")),
                "control: the split did not put the cache tests on the test side"
            );

            assert!(
                production.contains(LIVE_BASE),
                "control: {LIVE_BASE:?} no longer names the live base constructor in production"
            );

            assert!(
                !tests.contains(LIVE),
                "a test constructs the live cache, whose workers call the real \
                 api.pwnedpasswords.com"
            );
            assert!(
                !tests.contains(LIVE_BASE),
                "a test builds the production base URL. Handed to check_prefix that is a real \
                 request to api.pwnedpasswords.com from the test suite; every test here uses \
                 BaseUrl::loopback"
            );
            // The endpoint constant is named in exactly one test: the pin,
            // which only compares it to a string literal.
            assert_eq!(
                tests.matches(ENDPOINT).count(),
                1,
                "the production endpoint is named somewhere new in the tests. The one expected \
                 use is `the_production_endpoint_is_the_https_range_api`, which only compares it \
                 to a string literal and never calls anything with it"
            );
        }
    }

}

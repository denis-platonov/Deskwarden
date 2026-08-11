//! Bitwarden **Send** -- the pure half.
//!
//! A Send publishes a secret behind a **public link**. It is the only
//! outbound-publishing action in this app; everything else is local. That
//! makes an accidental Send a real harm, and a Send that cannot be revoked a
//! worse one -- so the feature lands list-and-delete before create, and this
//! module is deliberately the half that can do neither. **Nothing here spawns
//! a process.** [`SendRunner`] is a trait with no production implementation
//! yet; the only implementation in the crate today is the test fake below.
//!
//! ## What was measured about `bw`, and what follows from it
//!
//! `bw serve` has **no Send endpoint** -- Send is CLI-only, so this cannot go
//! through [`crate::vault_bridge`] like every other write. Captured from the
//! installed CLI:
//!
//! ```text
//! bw send create [options] [encodedJson]
//!   encodedJson    JSON object to upload. Can also be piped in through stdin.
//!   --file <path>  file to Send. Can also be specified in parent's JSON.
//!   --text <text>  text to Send. Can also be specified in parent's JSON.
//!   --hidden       text hidden flag. Valid only with the --text option.
//! Note: Options specified in JSON take precedence over command options
//! ```
//!
//! **The consequence is the good one.** The share password, the access cap,
//! the deletion date and the hidden flag are all *JSON* fields, and the JSON
//! can be piped through **stdin**. So no secret ever needs to touch `argv`,
//! where it would be readable by every other process on the machine for as
//! long as `bw` lives. The design had feared it might have to drop the share
//! password entirely; it does not. The command-line flags that would have
//! carried the share password or the recipient e-mail addresses therefore
//! appear **nowhere** in this file, and
//! [`the_two_secret_bearing_flags_appear_nowhere_in_this_file`] pins that
//! over the source text -- because an assertion over invocations only ever
//! covers the plans a test happened to build.
//!
//! ## The seam
//!
//! [`SendInvocation`] is **the whole of what will be handed to `bw`**. Nothing
//! is added after it: step 2's runner takes one and executes exactly it. Its
//! fields are private and the only public way to make one is
//! [`plan_to_invocation`], because a seam a call site can build for itself is
//! not a seam -- `http_agent` is the local precedent, where a bare type let
//! two sites bypass the seam with the whole suite green.
//!
//! ## Time
//!
//! The deletion date and [`expiry_wording`] both need "now", and a
//! `SystemTime::now()` inside them would make every test that reads a
//! deletion date depend on the wall clock. The clock is therefore **injected**
//! ([`SendClock`]); [`FixedClock`] is what the tests use.

use zeroize::Zeroizing;

#[cfg(test)]
/// Present so that
/// [`the_two_secret_bearing_flags_appear_nowhere_in_this_file`] has a positive
/// control: without it, a test asserting only that two needles are *absent*
/// would pass just as happily against an empty string, a misspelt
/// `include_str!` path or a `contains` that had been inverted.
const ARGV_PIN_CONTROL: &str = "argv-pin-control-marker";

/// The three lifetimes a Send may be given, in days. Not a free `u8`: the
/// picker offers exactly these, and [`validate_plan`] refuses anything else,
/// so a caller cannot quietly publish a link that outlives what the user was
/// shown.
pub const DELETE_IN_DAYS_CHOICES: [u8; 3] = [1, 7, 30];

/// The default, and the one the picker starts on.
pub const DEFAULT_DELETE_IN_DAYS: u8 = 7;

/// The longest name that will be accepted. `bw` has no documented limit; this
/// exists so a name pasted out of a document cannot become a multi-kilobyte
/// argument to a JSON builder.
const MAX_NAME_LEN: usize = 200;

/// The longest secret body that will be accepted, in bytes.
const MAX_TEXT_LEN: usize = 100_000;

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// Everything the user chose, and nothing else. Deliberately inert: building
/// one publishes nothing.
///
/// `text` and `password` are [`Zeroizing`] for the same reason
/// `vault_bridge::LoginData::password` is -- the whole point of a Send is that
/// the body is a secret, and a `String` handed back to the allocator with the
/// secret still in it is exactly what the crate's `#[global_allocator]` probe
/// exists to catch. [`the_plans_secret_fields_and_the_built_json_all_wipe`]
/// holds all three buffers against that probe.
///
/// **`Debug` is hand-written below and deliberately not derived.**
/// `vault_bridge.rs`'s `PasswordHistoryEntry` set the precedent, and the
/// reason applies here word for word: a new secret-carrying struct starts
/// without the escape route rather than adding one more. `Zeroizing` is not
/// that escape route's lock -- its own `Debug` forwards straight to the inner
/// value, so a derived one would print the secret body and the share password
/// in full to anyone who wrote `{plan:?}`.
#[derive(Clone)]
pub struct SendPlan {
    pub name: String,
    pub text: Zeroizing<String>,
    pub hidden: bool,
    /// One of [`DELETE_IN_DAYS_CHOICES`]. Defaults to
    /// [`DEFAULT_DELETE_IN_DAYS`] via [`Default`].
    pub delete_in_days: u8,
    pub password: Option<Zeroizing<String>>,
    pub max_access_count: Option<u32>,
}

impl Default for SendPlan {
    fn default() -> Self {
        Self {
            name: String::new(),
            text: Zeroizing::new(String::new()),
            hidden: false,
            delete_in_days: DEFAULT_DELETE_IN_DAYS,
            password: None,
            max_access_count: None,
        }
    }
}

/// A stand-in for a value that must not be printed, carrying only its length.
///
/// A length is safe and is the one thing worth knowing from a log line: it
/// separates "the body is there" from "the body is empty", which is the only
/// question a `Debug` of one of these types can usefully answer.
struct Redacted(usize);

impl std::fmt::Debug for Redacted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} bytes redacted>", self.0)
    }
}

/// Hand-written so the two secrets never reach a formatter.
///
/// `name` is kept: it is the label the user typed to identify the Send to
/// themselves, it is already on screen and in the Sends list, and redacting it
/// would leave a `Debug` that cannot tell two plans apart.
impl std::fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendPlan")
            .field("name", &self.name)
            .field("text", &Redacted(self.text.len()))
            .field("hidden", &self.hidden)
            .field("delete_in_days", &self.delete_in_days)
            .field("password", &self.password.as_ref().map(|p| Redacted(p.len())))
            .field("max_access_count", &self.max_access_count)
            .finish()
    }
}

/// The problem with a plan, phrased for the user, or `None` if there is none.
///
/// A `&'static str` rather than an enum because every one of these is a
/// sentence shown beside the form; there is nothing for a caller to branch on.
pub fn validate_plan(plan: &SendPlan) -> Option<&'static str> {
    if plan.name.trim().is_empty() {
        return Some("Give the Send a name.");
    }
    if plan.name.len() > MAX_NAME_LEN {
        return Some("That name is too long.");
    }
    if plan.text.is_empty() {
        return Some("There is nothing to send.");
    }
    if plan.text.len() > MAX_TEXT_LEN {
        return Some("That is too much text for one Send.");
    }
    if !DELETE_IN_DAYS_CHOICES.contains(&plan.delete_in_days) {
        return Some("Choose how long the link should last: 1, 7 or 30 days.");
    }
    if let Some(password) = &plan.password {
        if password.is_empty() {
            return Some("Either set a password or turn the password off.");
        }
    }
    if plan.max_access_count == Some(0) {
        return Some("A limit of zero views would make the link useless.");
    }
    None
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// "Now", injected. See the module docs: nothing in this file reads the wall
/// clock for itself.
pub trait SendClock {
    /// Milliseconds since the Unix epoch, UTC.
    fn now_unix_millis(&self) -> i64;
}

/// A clock that always answers the same instant. What the tests use, and the
/// reason every assertion about a deletion date in this file is exact rather
/// than approximate.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl SendClock for FixedClock {
    fn now_unix_millis(&self) -> i64 {
        self.0
    }
}

/// The wall clock. **Not used anywhere in this module** -- it exists so that
/// step 2's call site has one obvious thing to pass, rather than inventing its
/// own and drifting from the format the tests pin.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SendClock for SystemClock {
    fn now_unix_millis(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

const MILLIS_PER_DAY: i64 = 86_400_000;

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Civil date from a count of days since 1970-01-01, by Howard Hinnant's
/// `civil_from_days`. Proleptic Gregorian, which is what an ISO 8601 instant
/// means and what `bw` parses.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// Splits a Unix millisecond instant into `(year, month, day, hour, minute,
/// second, millisecond)`, UTC. Floor division throughout, so instants before
/// the epoch do not fold the wrong way.
fn utc_parts(millis: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let days = millis.div_euclid(MILLIS_PER_DAY);
    let rem = millis.rem_euclid(MILLIS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let ms = (rem % 1000) as u32;
    let secs = rem / 1000;
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs / 60) % 60) as u32,
        (secs % 60) as u32,
        ms,
    )
}

/// The instant a Send planned `days` from `now` should be deleted, in the
/// shape `bw send template` emits: `2026-08-18T00:43:17.148Z`.
fn deletion_date(days: u8, now: &dyn SendClock) -> String {
    let (y, mo, d, h, mi, s, ms) =
        utc_parts(now.now_unix_millis() + i64::from(days) * MILLIS_PER_DAY);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// What the form says under the lifetime picker: **the date the link dies**,
/// not only the number of days.
///
/// "7 days" is a duration the user has to do arithmetic on; a date is the
/// thing they can check against a calendar, and this is a publishing action
/// where being wrong about the lifetime is the harm. Both are given.
pub fn expiry_wording(days: u8, now: &dyn SendClock) -> String {
    let (y, mo, d, ..) = utc_parts(now.now_unix_millis() + i64::from(days) * MILLIS_PER_DAY);
    let month = MONTH_NAMES
        .get(mo.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("???");
    let unit = if days == 1 { "day" } else { "days" };
    format!("The link stops working after {days} {unit} -- on {d} {month} {y} (UTC).")
}

// ---------------------------------------------------------------------------
// The invocation
// ---------------------------------------------------------------------------

/// **The whole of what will be handed to `bw`.** Nothing is added after this
/// is built: step 2's runner executes exactly these arguments, feeds exactly
/// this stdin, and sets exactly this session token in the environment.
///
/// The fields are private and there is no public constructor but
/// [`plan_to_invocation`]. That is the point of the type -- see the module
/// docs on the `http_agent` precedent.
///
/// **`Debug` is hand-written below and deliberately not derived**, for the
/// reason [`SendPlan`] gives. The derived one printed
/// `session_token: Some(Zeroizing("THE-SESSION-TOKEN"))` verbatim and a
/// base64 body that decodes to the Send's text and its share password, so a
/// single `log::debug!("{inv:?}")` at a future call site would have put the
/// key to the whole vault and the secret being published into the log file.
#[derive(Clone)]
pub struct SendInvocation {
    /// The arguments after the `bw` executable itself. **Never a secret**;
    /// see [`the_built_invocation_never_carries_a_secret_in_its_arguments`].
    args: Vec<String>,
    /// The base64 of the JSON body, as `bw`'s `create` commands read stdin.
    /// Empty for the invocations that have no body (list, delete).
    stdin_json_b64: Zeroizing<String>,
    /// The session token to place in the environment, or `None` to mean "the
    /// runner uses the session it was configured with". Only the create path
    /// is handed one explicitly, because only it is built from a plan the
    /// caller already holds a session for.
    session_token: Option<Zeroizing<String>>,
}

impl SendInvocation {
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The base64 body to write to `bw`'s stdin, or `""` when there is none.
    pub fn stdin_json_b64(&self) -> &str {
        &self.stdin_json_b64
    }

    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref().map(|s| s.as_str())
    }
}

/// Hand-written for the reason on the type: the session token unlocks the
/// vault and the body is the secret being published, so neither may reach a
/// formatter. The arguments are printed in full -- they are pinned never to
/// carry a secret, and a `Debug` that hid them would say nothing at all.
impl std::fmt::Debug for SendInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendInvocation")
            .field("args", &self.args)
            .field("stdin_json_b64", &Redacted(self.stdin_json_b64.len()))
            .field(
                "session_token",
                &self.session_token.as_ref().map(|t| Redacted(t.len())),
            )
            .finish()
    }
}

/// Byte equality, hand-written rather than derived so that the comparison is
/// over the three fields by name: a field added to [`SendInvocation`] and not
/// added here would make
/// [`create_send_runs_the_invocation_it_was_given_rather_than_one_of_its_own`]
/// blind to it.
impl PartialEq for SendInvocation {
    fn eq(&self, other: &Self) -> bool {
        self.args == other.args
            && *self.stdin_json_b64 == *other.stdin_json_b64
            && self.session_token.as_deref().map(|s| s.as_str())
                == other.session_token.as_deref().map(|s| s.as_str())
    }
}

/// Appends `s` to `out` as a JSON string literal, quotes included.
///
/// Hand-rolled rather than `serde_json::to_string`, and that is deliberate:
/// `serde_json` would allocate a plain `String` for the secret body and hand
/// it back to the allocator unwiped, which is precisely the leak
/// [`the_plans_secret_fields_and_the_built_json_all_wipe`] refuses. This
/// pushes straight into the caller's pre-reserved [`Zeroizing`] buffer and
/// allocates nothing of its own.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // No `format!`: that would allocate a `String` per control
                // character, out of the secret body, outside the wiped buffer.
                out.push_str("\\u00");
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(HEX[(c as usize >> 4) & 0xf] as char);
                out.push(HEX[c as usize & 0xf] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, straight into a pre-reserved buffer.
///
/// The crate has no base64 dependency and this is fourteen lines; adding one
/// to encode a single JSON body would be a larger change than the feature.
fn base64_into(out: &mut String, bytes: &[u8]) {
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
}

/// Turns a plan into the exact invocation `bw` will be given.
///
/// **Every user choice travels in the JSON body**, which goes over stdin. The
/// argument vector is the two literal words `send create` and nothing else, so
/// there is no plan for which a secret can reach `argv`.
///
/// The clock is a parameter and not a `SystemTime::now()` for the reason in
/// the module docs. It is an addition to the shape the design sketched, which
/// had no clock on this function at all and therefore no way to produce a
/// deletion date without reading the wall clock inside it.
pub fn plan_to_invocation(
    plan: &SendPlan,
    session: &str,
    now: &dyn SendClock,
) -> Result<SendInvocation, SendError> {
    if let Some(problem) = validate_plan(plan) {
        return Err(SendError::Rejected(problem.to_string()));
    }

    // Reserved up front, and generously. A `String` that reallocates while
    // the secret is already in it leaves the old buffer behind for the
    // allocator with the plaintext still in it -- the crate's probe watches
    // `realloc` for exactly that, and there is no reason to make it work.
    let mut json = Zeroizing::new(String::with_capacity(
        512 + plan.name.len() * 6
            + plan.text.len() * 6
            + plan.password.as_ref().map_or(0, |p| p.len() * 6),
    ));
    let json_mut: &mut String = &mut json;

    json_mut.push_str("{\"object\":\"send\",\"type\":0,\"name\":");
    push_json_string(json_mut, plan.name.trim());
    json_mut.push_str(",\"notes\":null,\"text\":{\"text\":");
    push_json_string(json_mut, &plan.text);
    json_mut.push_str(",\"hidden\":");
    json_mut.push_str(if plan.hidden { "true" } else { "false" });
    json_mut.push_str("},\"file\":null,\"maxAccessCount\":");
    match plan.max_access_count {
        // No `format!`/`to_string`: this one is not secret, but the buffer is
        // pre-reserved and a temporary would be a second allocation for
        // nothing.
        Some(n) => {
            let mut buf = itoa_u32(n);
            json_mut.push_str(buf.as_str());
            buf.clear();
        }
        None => json_mut.push_str("null"),
    }
    json_mut.push_str(",\"deletionDate\":");
    push_json_string(json_mut, &deletion_date(plan.delete_in_days, now));
    json_mut.push_str(",\"expirationDate\":null,\"password\":");
    match &plan.password {
        Some(p) => push_json_string(json_mut, p),
        None => json_mut.push_str("null"),
    }
    json_mut.push_str(",\"emails\":null,\"disabled\":false,\"hideEmail\":false}");

    let mut b64 = Zeroizing::new(String::with_capacity(4 * (json.len() / 3 + 2)));
    base64_into(&mut b64, json.as_bytes());

    Ok(SendInvocation {
        args: vec!["send".to_string(), "create".to_string()],
        stdin_json_b64: b64,
        session_token: Some(Zeroizing::new(session.to_string())),
    })
}

/// A `u32` rendered without allocating a formatting temporary.
fn itoa_u32(mut n: u32) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut digits = [0u8; 10];
    let mut i = digits.len();
    while n > 0 {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    String::from_utf8_lossy(&digits[i..]).into_owned()
}

/// The invocation that lists every Send. **Private on purpose**: the seam is
/// the type, so a call site outside this module can only reach a
/// [`SendInvocation`] through [`list_sends`] / [`delete_send`] /
/// [`plan_to_invocation`], never by assembling one.
fn list_invocation(session: Option<&str>) -> SendInvocation {
    SendInvocation {
        args: vec!["send".to_string(), "list".to_string()],
        stdin_json_b64: Zeroizing::new(String::new()),
        session_token: session.map(|s| Zeroizing::new(s.to_string())),
    }
}

/// The invocation that revokes one Send. Same privacy, same reason.
fn delete_invocation(id: &str, session: Option<&str>) -> SendInvocation {
    SendInvocation {
        args: vec!["send".to_string(), "delete".to_string(), id.to_string()],
        stdin_json_b64: Zeroizing::new(String::new()),
        session_token: session.map(|s| Zeroizing::new(s.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Results and failures
// ---------------------------------------------------------------------------

/// A Send that was created and whose link was read back. The link is the whole
/// point: a `CreatedSend` the user cannot be shown the URL for is not a
/// success, which is why [`SendError::CreatedButUnreadable`] exists rather
/// than an `access_url: Option<String>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSend {
    pub id: String,
    pub name: String,
    pub access_url: String,
    pub deletion_date: String,
}

/// One row of the Sends screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendSummary {
    pub id: String,
    pub name: String,
    pub access_url: String,
    pub deletion_date: String,
    /// File Sends are listed and can be revoked, but this app cannot create
    /// one; the screen says so rather than pretending they are the same thing.
    pub is_file: bool,
}

/// What `bw` said, verbatim. The runner's whole output; no interpretation.
#[derive(Debug, Clone, Default)]
pub struct RawOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// There is no `bw` this app is willing to run. Carries the reason from
    /// [`crate::bw_path`] / [`crate::signature`].
    NoVerifiedCli(String),
    /// The vault is locked or the session token is stale.
    Locked,
    /// `bw` could not reach the server.
    Offline,
    /// The server, or this module's own validation, refused the request and
    /// said why. The string is the sentence to show.
    Rejected(String),
    /// A non-zero exit with nothing on either stream.
    FailedSilently,
    /// **The load-bearing arm.** `bw` exited 0 -- so a Send has probably been
    /// created and a public link probably exists -- but its output could not
    /// be read, so this app does not know the link or the id and cannot offer
    /// to revoke it.
    ///
    /// It must render as neither a success nor a clean failure. This is the
    /// crate's "could not check must never render as success" rule applied to
    /// "could not create": the user is told to go and look at the Sends list,
    /// because the alternative is an unrevoked public link nobody knows about.
    CreatedButUnreadable,
    /// `bw` was still running when the deadline passed and was given up on.
    /// **Ambiguous in the same way as [`Self::CreatedButUnreadable`]**: the
    /// request may well have reached the server before the app stopped
    /// waiting.
    ///
    /// Unlike [`Self::CreatedButUnreadable`] this arm is reached from **all
    /// three** operations -- create, list and revoke -- because
    /// [`CliSendRunner::run`] answers it whenever the cap expires, whatever
    /// the invocation was. Its sentence must therefore be true of a revoke
    /// that may not have happened as well as of a create that may have; see
    /// [`the_timeout_message_is_true_of_a_revoke_and_of_a_list_as_well`].
    TimedOut,
    /// `bw` could not be started at all. Unambiguous -- nothing ran.
    SpawnFailed(String),
}

impl SendError {
    /// The sentence to show. **No message may read as a plain success**;
    /// [`no_failure_message_reads_as_a_success`] holds that over every arm.
    pub fn user_message(&self) -> &str {
        match self {
            Self::NoVerifiedCli(why) => why,
            Self::Locked => "The vault is locked. Unlock it and try again.",
            Self::Offline => "Bitwarden could not be reached. Check your connection and try again.",
            Self::Rejected(why) => why,
            Self::FailedSilently => {
                "Bitwarden stopped without saying why. Check your Sends before trying again."
            }
            Self::CreatedButUnreadable => {
                "A Send may have been created, but its link could not be read. \
                 Check your Sends list before trying again -- if it is there, the link is \
                 already public."
            }
            Self::TimedOut => {
                // Reached from create, from list AND from revoke, so it may
                // not claim a Send was created: on a timed-out revoke that is
                // false, and the true worry there -- the link may still be
                // live -- went unsaid.
                "Bitwarden did not answer in time. Check your Sends list before trying again: \
                 a Send may now be public, or one you tried to revoke may still be live."
            }
            Self::SpawnFailed(why) => why,
        }
    }

    /// Whether this failure leaves it **unknown** whether a public link now
    /// exists. The screen must not offer a plain "try again" for these; it
    /// must send the user to the list first.
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::CreatedButUnreadable | Self::TimedOut)
    }
}

/// Turns a finished `bw` run into a [`SendError`].
///
/// **Called only when the run did not yield a readable success.** The exit-0
/// arm is therefore not "it worked"; it is "it exited cleanly and the output
/// could not be read", which is [`SendError::CreatedButUnreadable`] and is the
/// single most important row in this function.
pub fn classify_failure(exit_code: Option<i32>, stdout: &str, stderr: &str) -> SendError {
    if exit_code == Some(0) {
        return SendError::CreatedButUnreadable;
    }
    if exit_code.is_none() {
        return SendError::TimedOut;
    }

    let mut haystack = stderr.to_ascii_lowercase();
    haystack.push('\n');
    haystack.push_str(&stdout.to_ascii_lowercase());

    // Order matters, and this is the order: the two that mean "nothing ran"
    // first, because their messages also mention the vault and the network.
    if exit_code == Some(127)
        || haystack.contains("enoent")
        || haystack.contains("is not recognized as an internal")
        || haystack.contains("command not found")
    {
        return SendError::SpawnFailed(
            "Bitwarden's command-line tool could not be started. Nothing was sent.".to_string(),
        );
    }
    if haystack.contains("signature")
        || haystack.contains("not verified")
        || haystack.contains("unverified")
    {
        return SendError::NoVerifiedCli(
            "Bitwarden's command-line tool could not be verified, so it was not run. \
             Nothing was sent."
                .to_string(),
        );
    }
    if haystack.contains("locked")
        || haystack.contains("not logged in")
        || haystack.contains("session key")
        || haystack.contains("you are not logged in")
    {
        return SendError::Locked;
    }
    if haystack.contains("enotfound")
        || haystack.contains("econnrefused")
        || haystack.contains("etimedout")
        || haystack.contains("getaddrinfo")
        || haystack.contains("network")
    {
        return SendError::Offline;
    }

    let detail = first_useful_line(stderr).or_else(|| first_useful_line(stdout));
    match detail {
        Some(line) => SendError::Rejected(format!("Bitwarden would not do it: {line}")),
        None => SendError::FailedSilently,
    }
}

/// The first non-blank line of a stream, trimmed, or `None` if there is none.
fn first_useful_line(stream: &str) -> Option<String> {
    stream
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Parsing what `bw` said
// ---------------------------------------------------------------------------

/// Unwraps `bw`'s optional `{"success":true,"data":{..}}` envelope. `bw` emits
/// it under `--response` and the bare object otherwise, and which one a given
/// installation produces is not something this module should depend on.
fn unwrap_envelope(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(mut map) if map.contains_key("data") => {
            let data = map.remove("data").unwrap_or(serde_json::Value::Null);
            if map.contains_key("success") || data.is_object() || data.is_array() {
                return data;
            }
            serde_json::Value::Object(map)
        }
        other => other,
    }
}

fn string_field(obj: &serde_json::Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Reads back the Send `bw send create` says it made.
///
/// **Unknown fields are tolerated** -- `bw`'s object carries a dozen keys this
/// app has no use for and a new one must not break create. **A missing `id` or
/// `accessUrl` fails loudly**, because without them the app can neither show
/// the link nor revoke it, and reporting that as a success is the exact harm
/// [`SendError::CreatedButUnreadable`] exists for.
pub fn parse_created_send(stdout: &str) -> Result<CreatedSend, SendError> {
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|_| SendError::CreatedButUnreadable)?;
    let value = unwrap_envelope(value);
    let id = string_field(&value, "id").ok_or(SendError::CreatedButUnreadable)?;
    let access_url = string_field(&value, "accessUrl").ok_or(SendError::CreatedButUnreadable)?;
    Ok(CreatedSend {
        id,
        name: string_field(&value, "name").unwrap_or_default(),
        access_url,
        deletion_date: string_field(&value, "deletionDate").unwrap_or_default(),
    })
}

/// Reads `bw send list`. Same tolerance and the same loudness: a row without
/// an `id` cannot be revoked and a row without an `accessUrl` cannot be shown,
/// so a list containing one is a parse failure rather than a short list.
pub fn parse_send_list(stdout: &str) -> Result<Vec<SendSummary>, SendError> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        SendError::Rejected("Your Sends could not be read.".to_string())
    })?;
    let value = unwrap_envelope(value);
    let rows = value
        .as_array()
        .ok_or_else(|| SendError::Rejected("Your Sends could not be read.".to_string()))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = string_field(row, "id").ok_or_else(|| {
            SendError::Rejected("A Send in the list has no id, so it could not be shown.".to_string())
        })?;
        let access_url = string_field(row, "accessUrl").ok_or_else(|| {
            SendError::Rejected(
                "A Send in the list has no link, so it could not be shown.".to_string(),
            )
        })?;
        let is_file = row.get("type").and_then(serde_json::Value::as_i64) == Some(1)
            || row.get("file").map(serde_json::Value::is_object) == Some(true);
        out.push(SendSummary {
            id,
            name: string_field(row, "name").unwrap_or_default(),
            access_url,
            deletion_date: string_field(row, "deletionDate").unwrap_or_default(),
            is_file,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The runner seam
// ---------------------------------------------------------------------------

/// Runs one [`SendInvocation`] and hands back what `bw` said.
///
/// **There is no production implementation in this crate yet**, and that is
/// the whole point of the step order: with only the fake below, nothing in
/// this app can publish anything. Step 2 adds the real one.
pub trait SendRunner {
    fn run(&self, inv: &SendInvocation) -> Result<RawOutput, SendError>;

    /// The session this runner was configured with, or `None` for one that
    /// was given none.
    ///
    /// **This is how the read paths get their token.** `SendInvocation`'s
    /// fields are private and the type is the whole of what reaches `bw`, so
    /// `list` and `delete` -- which are built here rather than from a plan a
    /// caller already holds a session for -- have to be handed one from
    /// somewhere. Step 1 already wrote down where: `session_token: None`
    /// means "the runner uses the session it was configured with", and this
    /// is that configuration, read at the one moment the invocation is built.
    ///
    /// Defaulted to `None` rather than required, because the fakes that stand
    /// in for `bw` in this crate's tests configure no session and must not be
    /// made to invent one.
    fn session(&self) -> Option<&str> {
        None
    }
}

/// Creates a Send: build the invocation, hand **that** invocation to the
/// runner, read the link back.
///
/// It does not rebuild the invocation, and
/// [`create_send_runs_the_invocation_it_was_given_rather_than_one_of_its_own`]
/// is what holds that -- the named local failure mode for this design is a
/// test that asserts a value was handed *to* a function and never that the
/// function used it.
pub fn create_send<R: SendRunner>(
    runner: &R,
    plan: &SendPlan,
    session: &str,
    now: &dyn SendClock,
) -> Result<CreatedSend, SendError> {
    let invocation = plan_to_invocation(plan, session, now)?;
    let raw = runner.run(&invocation)?;
    if raw.exit_code != Some(0) {
        return Err(classify_failure(raw.exit_code, &raw.stdout, &raw.stderr));
    }
    parse_created_send(&raw.stdout)
}

pub fn list_sends<R: SendRunner>(runner: &R) -> Result<Vec<SendSummary>, SendError> {
    let raw = runner.run(&list_invocation(runner.session()))?;
    if raw.exit_code != Some(0) {
        return Err(classify_failure(raw.exit_code, &raw.stdout, &raw.stderr));
    }
    parse_send_list(&raw.stdout)
}

/// Revokes one Send. **Nothing is parsed back**: `bw send delete` prints a
/// human sentence, and the only question worth asking is whether it exited 0.
pub fn delete_send<R: SendRunner>(runner: &R, id: &str) -> Result<(), SendError> {
    let raw = runner.run(&delete_invocation(id, runner.session()))?;
    if raw.exit_code != Some(0) {
        return Err(classify_failure(raw.exit_code, &raw.stdout, &raw.stderr));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The real runner: one `bw` child per invocation
// ---------------------------------------------------------------------------

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The environment variable the CLI reads its session from.
///
/// **An environment variable and never an argument.** A process's argument
/// vector is readable by any other process on the machine (Task Manager's
/// "Command line" column, `Get-CimInstance Win32_Process`), and the session
/// token unlocks the whole vault. `bw_serve::run_bw_sync` hands the session
/// over exactly this way, and this is the same rule applied to the same
/// secret.
const SESSION_ENV: &str = "BW_SESSION";

/// The wall-clock cap on one CLI child.
///
/// Sixty seconds, not five: creating a Send talks to the server, and a slow
/// link that would have succeeded must not be cut off. When the cap *does*
/// expire the answer is [`SendError::TimedOut`], which
/// [`SendError::is_ambiguous`] reports as ambiguous -- the request may well
/// have reached the server before this app stopped waiting, so a Send may
/// exist and a public link with it. Reporting a timeout as a clean failure
/// would leave an unrevoked link nobody knows about, which is the exact harm
/// this whole module is arranged around.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the wait loop asks whether the child has exited.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What the wait loop should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDecision {
    /// The child exited; collect its output.
    Finished,
    /// Still running, still inside the cap.
    KeepWaiting,
    /// Still running and the cap has passed: kill it and report the ambiguous
    /// [`SendError::TimedOut`].
    GiveUp,
}

/// The one decision the wait loop makes, as a pure function of the two facts
/// it has.
///
/// Pulled out of the loop precisely so that the deadline rule is testable
/// without a process: nothing in this crate may spawn `bw` from a test, so if
/// this lived inline it would be untested. `cap` is a parameter rather than a
/// read of [`SEND_TIMEOUT`] so the boundary can be reached without waiting a
/// minute for it.
///
/// `exited` wins over the clock: a child that finished on the last
/// millisecond produced a result, and calling that a timeout would report an
/// answer this app actually has as an ambiguous non-answer.
pub fn wait_decision(exited: bool, elapsed: Duration, cap: Duration) -> WaitDecision {
    if exited {
        WaitDecision::Finished
    } else if elapsed >= cap {
        WaitDecision::GiveUp
    } else {
        WaitDecision::KeepWaiting
    }
}

/// Turns an exit status plus the two captured streams into a [`RawOutput`].
///
/// Pure, and separate from the spawn for that reason: everything downstream of
/// the child -- [`classify_failure`], [`parse_created_send`],
/// [`parse_send_list`] -- reads only this, so this is where "what the CLI
/// said" is decided, and it can be tested directly instead of through a
/// process.
///
/// `from_utf8_lossy` rather than a hard error: the CLI's own sentence is the
/// only explanation the user will get, and throwing it away because one byte
/// was not UTF-8 would downgrade a readable rejection to
/// [`SendError::FailedSilently`].
///
/// A `None` exit code means the child was killed rather than exited, and
/// [`classify_failure`] already reads that as [`SendError::TimedOut`] -- i.e.
/// ambiguous -- which is why nothing here invents a code to stand in for it.
pub fn raw_output_from(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> RawOutput {
    RawOutput {
        exit_code,
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    }
}

/// Reads a captured pipe to end-of-file. A read error yields what was read so
/// far rather than nothing: a partial message still explains more than an
/// empty one.
fn drain<R: Read>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    buf
}

/// The production [`SendRunner`]: runs one [`SendInvocation`] as a real child
/// of the verified Bitwarden CLI, and reports exactly what it said.
///
/// It adds nothing to the invocation. The arguments, the stdin body and the
/// session are all read off the [`SendInvocation`] it is handed, which is the
/// point of that type being the whole of what reaches the CLI.
struct CliSendRunner<'a> {
    /// The kill-on-close job the child is placed in **before it executes a
    /// single instruction**.
    ///
    /// From the design, verbatim: an orphaned `bw` holding `BW_SESSION` after
    /// deskwarden dies is the same hazard `bw serve` is in the job for.
    ///
    /// Read only through [`Self::job`]. The guarantee used to rest on a
    /// source pin over the `spawn_in_job` call, and a pin over a call says
    /// nothing about the value flowing into it: `Self { job: job.filter(|_|
    /// false), data_dir }` left the pinned line word-perfect, the whole suite
    /// green and no warning emitted, while every `bw` child was spawned
    /// outside the job.
    ///
    /// **Nor does an accessor settle it.** Measured on an earlier commit, a
    /// [`SendRunner::run`] of
    ///
    /// ```text
    /// CliSendRunner::new(None, self.data_dir).run_inner(inv)
    /// ```
    ///
    /// -- the work handed to a second method on the same type, constructed
    /// jobless -- left the pin word-perfect, [`Self::job`] answering honestly,
    /// the pointer-identity test green, 0 failed and 0 warnings, while every
    /// `bw` child spawned outside the job. A pin on `self.job()` cannot see
    /// WHICH `self` it is. What holds this now is
    /// [`the_send_reaches_the_spawn_carrying_the_job_the_runner_was_built_with`],
    /// which drives [`list_sends`] end to end and asserts the job value that
    /// ARRIVED at [`crate::job_object::spawn_in_job`], plus `job_object`'s
    /// tree walk proving this file has no second route to a child process.
    ///
    /// **And it is unreachable in production today.** The one production
    /// construction of this type, in `vault_window`, passes `None` outright:
    /// the vault window holds no [`crate::job_object::KillOnCloseJob`], so
    /// every real `bw send` child already spawns outside any job. That is
    /// deliberate and documented, not an oversight -- but it means the
    /// guarantee above is currently proven only of code no production caller
    /// exercises with a job. When the window is given one, the property must
    /// be re-proven at the call that hands it over.
    job: Option<&'a crate::job_object::KillOnCloseJob>,
    /// The active account's CLI profile directory, passed straight through to
    /// [`crate::bw_path::bw_command_in`] so a Send is created, listed and
    /// revoked against the same account as every other write in this app.
    data_dir: Option<&'a Path>,
    /// The vault session this runner hands to every child it starts, or
    /// `None` for a runner that was given none.
    ///
    /// A `Zeroizing<String>` and not a `&str`, for the same reason
    /// `SendPlan`'s secrets are: the token unlocks the whole vault, and a
    /// plain `String` goes back to the allocator with the token still in it.
    ///
    /// **It reaches the child in the environment and nowhere else.** See
    /// `SESSION_ENV`, and `the_session_the_list_runner_holds_reaches_the_child_and_never_its_argv`,
    /// which reads the overlay that arrived at the spawn rather than the one
    /// this field was set from.
    session: Option<Zeroizing<String>>,
}

impl<'a> CliSendRunner<'a> {
    fn new(
        job: Option<&'a crate::job_object::KillOnCloseJob>,
        data_dir: Option<&'a Path>,
    ) -> Self {
        Self {
            job,
            data_dir,
            session: None,
        }
    }

    /// The same runner, configured with the vault session every child it
    /// starts will inherit.
    ///
    /// **This is what makes the read paths work against a real vault.**
    /// `bw send list` and `bw send delete` are authenticated commands: a
    /// child that inherits no `BW_SESSION` answers "locked", which is exactly
    /// what the Sends screen used to show. `create` never had this problem
    /// because its invocation is built from a plan the caller already holds a
    /// session for.
    fn with_session(
        job: Option<&'a crate::job_object::KillOnCloseJob>,
        data_dir: Option<&'a Path>,
        session: &str,
    ) -> Self {
        Self {
            session: Some(Zeroizing::new(session.to_string())),
            ..Self::new(job, data_dir)
        }
    }

    /// The job the child will be placed in, or `None` when this runner was
    /// given none.
    ///
    /// **The single place [`SendRunner::run`] reads the job from**, which is
    /// what makes an assertion about this accessor an assertion about what
    /// the spawn actually gets. `data_dir` needs no equivalent: it reaches
    /// the built `Command` as an environment variable, so
    /// [`the_profile_directory_the_runner_was_given_reaches_the_child`] can
    /// already read it back without a process. Job membership cannot be read
    /// back that way, and this is the nearest thing to it that is still a
    /// value rather than a line of source text.
    fn job(&self) -> Option<&'a crate::job_object::KillOnCloseJob> {
        self.job
    }

    /// Builds the command for one invocation, **without spawning anything**.
    ///
    /// Separate from [`Self::run`] so that the built object can be inspected
    /// by a test: `get_program`, `get_args` and `get_envs` answer without a
    /// process, so the assertions that matter here -- that the body is not in
    /// argv, that the session is not in argv -- read the thing that would be
    /// executed rather than the inputs the test handed in.
    ///
    /// The command comes from [`crate::bw_path::bw_command_in`] and from
    /// nowhere else: that is the one place that names the executable whose
    /// signature `check_bw_signature` verified at startup, and it refuses
    /// outright when no verified path was recorded. Building one here by hand
    /// would run whatever binary the ambient search order found first.
    fn build_command(
        &self,
        inv: &SendInvocation,
    ) -> Result<crate::job_object::JobCommand, SendError> {
        let mut command = crate::bw_path::bw_job_command_in(self.data_dir)
            .map_err(SendError::NoVerifiedCli)?;
        command.args(inv.args());
        if let Some(token) = inv.session_token() {
            command.env(SESSION_ENV, token);
        }
        // Both pipes captured, and stdin piped because the request body goes
        // in that way. No stream is inherited: a console handed to the child
        // is a console this app does not own.
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(command)
    }
}

impl SendRunner for CliSendRunner<'_> {
    fn session(&self) -> Option<&str> {
        self.session.as_deref().map(|s| s.as_str())
    }

    fn run(&self, inv: &SendInvocation) -> Result<RawOutput, SendError> {
        let command = self.build_command(inv)?;

        // Spawned suspended, put in the job, and only then resumed -- see
        // `job_object::spawn_in_job`, which also re-ORs `CREATE_NO_WINDOW`
        // because `creation_flags` REPLACES the flags a command holds rather
        // than adding to them.
        let mut child = crate::job_object::spawn_in_job(self.job(), command).map_err(|e| {
            SendError::SpawnFailed(format!(
                "Bitwarden's command-line tool could not be started ({e}). Nothing was sent."
            ))
        })?;

        // Write the body and **close the pipe**. The CLI reads its
        // `encodedJson` from stdin to end-of-file, so a write handle left
        // open would keep it reading forever while this side waited for an
        // exit that could only arrive when the sixty-second cap expired --
        // a deadlock that reports itself as an ambiguous timeout. Taking the
        // handle out of the child moves it into this scope, so it is dropped,
        // and the pipe closed, before a single byte is read back.
        let write_error = match child.stdin.take() {
            Some(mut pipe) => {
                let body = inv.stdin_json_b64();
                let outcome = if body.is_empty() {
                    Ok(())
                } else {
                    pipe.write_all(body.as_bytes()).and_then(|()| pipe.flush())
                };
                drop(pipe);
                outcome.err()
            }
            None => None,
        };
        if let Some(e) = write_error {
            // Unambiguous: a body that never arrived in full is not valid
            // JSON, so nothing was created. Kill rather than leave a child
            // waiting on a stream it will never get.
            let _ = child.kill();
            let _ = child.wait();
            return Err(SendError::SpawnFailed(format!(
                "The request could not be handed to Bitwarden's command-line tool ({e}). \
                 Nothing was sent."
            )));
        }

        let out_pipe = child.stdout.take();
        let err_pipe = child.stderr.take();

        std::thread::scope(|scope| {
            // Both pipes are drained concurrently: a child that fills one
            // while this side waits on the other would block forever, and the
            // only symptom would be the sixty-second cap.
            let out_reader = scope.spawn(move || drain(out_pipe));
            let err_reader = scope.spawn(move || drain(err_pipe));

            let started = Instant::now();
            let status = loop {
                // A `try_wait` error is read as "not finished": the cap is
                // then what ends the wait, which is the ambiguous answer and
                // therefore the safe direction.
                let exited = child.try_wait().ok().flatten();
                match wait_decision(exited.is_some(), started.elapsed(), SEND_TIMEOUT) {
                    WaitDecision::Finished => break exited,
                    WaitDecision::KeepWaiting => std::thread::sleep(POLL_INTERVAL),
                    WaitDecision::GiveUp => {
                        // Killing is also what lets the two readers finish:
                        // they are blocked on pipes that close only when the
                        // child does.
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                }
            };

            let stdout = out_reader.join().unwrap_or_default();
            let stderr = err_reader.join().unwrap_or_default();
            match status {
                Some(status) => Ok(raw_output_from(status.code(), &stdout, &stderr)),
                None => Err(SendError::TimedOut),
            }
        })
    }
}

/// **The one way into a real `bw send list` from outside this module.**
///
/// [`CliSendRunner`] is the only [`SendRunner`] in this crate that starts a
/// process, and as of this commit the type and both of its constructors are
/// **private to `crate::send`**. That is a compile-time wall and it replaced
/// a count. The count pinned the number of times the tokens `CliSendRunner`,
/// `CliSendRunner::new` and `CliSendRunner::with_session` appeared; a
/// measured mutant wrote
///
/// ```ignore
/// impl<'a> CliSendRunner<'a> {
///     pub fn warm_cache(session: &str) -> Result<(), SendError> {
///         let runner = Self::new(None, None);
///         let _ = runner.run(&list_invocation(Some(session)))?;
///         Ok(())
///     }
/// }
/// ```
///
/// and called it from the frame closure. `Self` spells none of the pinned
/// tokens, so every count stayed exact: 2109 lib / 217 bin / 0 failed / 0
/// warnings, an unbounded per-frame `bw send list`. The free-function shape
/// of the same mutant, which spells `CliSendRunner::new`, died. **The only
/// difference between the killed and the surviving mutant was the token
/// `Self`** -- a count pins the set of spellings, not the set of routes.
///
/// Privacy pins the route. `pub(in ..)` was not available: it accepts only
/// ANCESTOR modules, and `crate::vault_window::send_fetch_thread` is not an
/// ancestor of `crate::send`, so `pub(in crate::vault_window::send_fetch_thread)`
/// is E0742 here exactly as `pub(in crate::bw_serve)` was E0433 in an earlier
/// round. What IS available is plain module privacy: with no `pub` at all,
/// the type is visible in `crate::send` and its descendants -- which is
/// this module's own tests, and nothing else in the crate. Every spelling
/// `vault_window` could reach for -- `crate::send::CliSendRunner`, a `use`,
/// a `type` alias, a re-export, `Self` inside an `impl` written over there
/// -- is now **E0603**, at compile time, before any test runs.
///
/// **What this does not close, said plainly.** The mutant above was written
/// INSIDE this file, where `CliSendRunner` is still nameable. A new
/// `pub fn` added here could still build one and the frame could still call
/// it. What refuses that is
/// [`this_modules_public_surface_is_exactly_what_it_was`], an EQUALITY over
/// every module-level `pub` item in this file's production half -- not a
/// count of one spelling, so a differently-named helper fails it too.
///
/// The session, the job and the profile directory are all parameters for the
/// same reason they were parameters of the constructor: this function reads
/// no process state and holds none.
pub fn cli_send_list(
    job: Option<&crate::job_object::KillOnCloseJob>,
    data_dir: Option<&Path>,
    session: &str,
) -> Result<Vec<SendSummary>, SendError> {
    list_sends(&CliSendRunner::with_session(job, data_dir, session))
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod runner_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    /// The same instant the pure half's tests use.
    const NOW: FixedClock = FixedClock(1_786_408_997_148);
    /// Invented. No real session token appears in this file.
    const SESSION: &str = "an-invented-session-token-not-a-real-one";
    const SECRET: &str = "correct-horse-battery-staple";
    const SHARE_PASSWORD: &str = "share-pw-9271";

    fn a_plan() -> SendPlan {
        SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new(SECRET.to_string()),
            password: Some(Zeroizing::new(SHARE_PASSWORD.to_string())),
            max_access_count: Some(3),
            ..SendPlan::default()
        }
    }

    /// Records a verified CLI path so [`crate::bw_path::bw_command_in`] will
    /// answer at all, and hands back whichever path won the process-wide
    /// `OnceLock`.
    ///
    /// **Records a path; it does not touch the filesystem and nothing is
    /// spawned.** The path is deliberately the same invented one
    /// `bw_path`'s own tests record, and the assertions below compare against
    /// whatever `verified_bw_exe()` reports rather than against this literal,
    /// so the first-wins lock makes them order-independent.
    fn verified_exe() -> &'static Path {
        crate::bw_path::remember_verified_bw_exe(PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));
        crate::bw_path::verified_bw_exe().expect("a verified path was just recorded")
    }

    fn create_invocation() -> SendInvocation {
        plan_to_invocation(&a_plan(), SESSION, &NOW).expect("the plan is valid")
    }

    fn args_of(command: &crate::job_object::JobCommand) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_built_command_runs_the_verified_executable_and_not_an_ambient_one() {
        let exe = verified_exe();
        assert!(
            exe.is_absolute(),
            "control: the recorded path is relative, so the comparison below would not \
             distinguish an absolute path from a bare binary name"
        );

        let command = CliSendRunner::new(None, None)
            .build_command(&create_invocation())
            .expect("a verified path is recorded, so the command builds");

        // Read off the BUILT object, not off what was passed in.
        assert_eq!(
            Path::new(command.get_program()),
            exe,
            "the command names something other than the executable whose signature startup \
             verified; a bare binary name would be resolved by the ambient search order, which \
             on Windows checks this app's own user-writable directory first"
        );
    }

    #[test]
    fn there_is_no_command_at_all_when_no_verified_executable_was_recorded() {
        // The failure direction, without depending on the OnceLock being
        // empty (another test may have filled it): whatever `bw_command_in`
        // refuses with must arrive as `NoVerifiedCli` and never as a silent
        // fallback to some other binary.
        match crate::bw_path::bw_command_in(None) {
            Ok(command) => assert_eq!(
                Path::new(command.get_program()),
                crate::bw_path::verified_bw_exe().expect("it answered, so a path is recorded"),
                "the builder answered with a program that is not the verified one"
            ),
            Err(why) => assert!(
                !why.is_empty(),
                "the refusal must carry the reason the user is shown"
            ),
        }
    }

    #[test]
    fn the_request_body_reaches_stdin_and_never_the_argument_vector() {
        let _ = verified_exe();
        let inv = create_invocation();
        let body = inv.stdin_json_b64().to_string();
        assert!(
            !body.is_empty(),
            "control: creating a Send really does have a body, so the assertions below are \
             about a body that exists"
        );

        let command = CliSendRunner::new(None, None)
            .build_command(&inv)
            .expect("the command builds");
        let args = args_of(&command);

        // The whole design exists for this line: the argument vector is the
        // two literal words and nothing else.
        assert_eq!(
            args,
            vec!["send".to_string(), "create".to_string()],
            "the argument vector grew past `send create`; every field that could carry a \
             secret goes in through stdin as JSON, and argv is world-readable"
        );
        for arg in &args {
            assert!(
                !arg.contains(&body),
                "the encoded request body is in argv: {arg}"
            );
            assert!(!arg.contains(SECRET), "the shared secret is in argv: {arg}");
            assert!(
                !arg.contains(SHARE_PASSWORD),
                "the share password is in argv: {arg}"
            );
        }
    }

    #[test]
    fn the_session_token_is_handed_over_in_the_environment_and_never_in_argv() {
        let _ = verified_exe();
        let command = CliSendRunner::new(None, None)
            .build_command(&create_invocation())
            .expect("the command builds");

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(
            envs.iter()
                .any(|(k, v)| k == SESSION_ENV && v.as_deref() == Some(SESSION)),
            "the session is not in the child's environment under {SESSION_ENV}; it was {envs:?}"
        );
        for arg in args_of(&command) {
            assert!(
                !arg.contains(SESSION),
                "the session token is in argv, where any process on the machine can read it: \
                 {arg}"
            );
        }
    }

    #[test]
    fn the_invocations_that_carry_no_session_do_not_have_one_invented_for_them() {
        // An invocation whose `session_token` is `None` means "the runner
        // was configured with no session". Honouring that means setting
        // nothing, not reaching for some other source -- and it is what
        // makes `the_session_the_list_runner_holds_reaches_the_child_and_never_its_argv`'s
        // control able to tell a configured token from an invented one.
        let _ = verified_exe();
        for inv in [list_invocation(None), delete_invocation("send-id-1", None)] {
            assert!(inv.session_token().is_none(), "control: {inv:?} has none");
            let command = CliSendRunner::new(None, None)
                .build_command(&inv)
                .expect("the command builds");
            assert!(
                command
                    .get_envs()
                    .all(|(k, _)| k != OsStr::new(SESSION_ENV)),
                "a session was set for an invocation that carries none"
            );
            assert!(
                inv.stdin_json_b64().is_empty(),
                "control: this invocation has no body"
            );
        }
    }

    #[test]
    fn the_profile_directory_the_runner_was_given_reaches_the_child() {
        let _ = verified_exe();
        let dir = PathBuf::from(r"C:\deskwarden-test\profile-b");
        let command = CliSendRunner::new(None, Some(&dir))
            .build_command(&list_invocation(None))
            .expect("the command builds");
        assert!(
            command.get_envs().any(|(k, v)| k
                == OsStr::new(crate::bw_path::BW_DATA_DIR_ENV)
                && v == Some(OsStr::new(dir.as_os_str()))),
            "the account's profile directory did not reach the child, so a Send would be \
             created against whichever account the CLI defaults to"
        );
    }

    #[test]
    fn the_job_the_runner_was_given_is_the_job_the_child_is_spawned_into() {
        // THE CRITICAL FINDING, held behaviourally rather than by a pin.
        //
        // Job membership is a property of a real process and no test in this
        // crate may start one, so the guarantee was a source pin over the
        // `spawn_in_job` call. That pin was defeated by starving its
        // argument: `Self { job: job.filter(|_| false), data_dir }` left the
        // pinned line untouched, every test green and no warning emitted --
        // and a `bw` child holding an unlocked vault was spawned outside the
        // kill-on-close job, free to outlive a panic, a `process::exit` or a
        // Task Manager kill of this process.
        //
        // What is asserted is the VALUE, by pointer identity: the job at the
        // one accessor the spawn reads from is the job the constructor was
        // handed. A starved constructor cannot satisfy this, and neither can
        // one that substitutes a job of its own. It is the same shape as
        // `the_profile_directory_the_runner_was_given_reaches_the_child`,
        // which is what already defeats this trick for `data_dir`.
        //
        // `KillOnCloseJob::new` creates a kernel handle. Nothing is spawned,
        // no file is touched, no socket is opened, and the handle is dropped
        // with nothing assigned to it.
        let job = crate::job_object::KillOnCloseJob::new()
            .expect("a job object is a handle, not a process");

        let runner = CliSendRunner::new(Some(&job), None);
        let reaching = runner.job().expect(
            "the runner threw the job away between its constructor and the spawn, so a `bw` \
             child holding an unlocked vault would not die with this process",
        );
        assert!(
            std::ptr::eq(&job, reaching),
            "the job the spawn reads is not the job the runner was constructed with"
        );

        // Control, and the reason this is not a type that cannot be empty:
        // `KillOnCloseJob::new` can genuinely fail, and the window that lists
        // Sends holds no job at all, so `None` has to stay expressible.
        assert!(
            CliSendRunner::new(None, None).job().is_none(),
            "control: `no job at all` is still representable, so the assertion above is about \
             a job that was really there"
        );
    }

    /// Drives `list_sends` end to end through the production `CliSendRunner`
    /// and answers what arrived at the spawn: the address of the job, or
    /// `None` for a jobless child.
    ///
    /// No process is created. `job_object::spawn_probe` stands where
    /// `CreateProcess` would and refuses, so `run` takes its ordinary
    /// spawn-failed path and `list_sends` returns `SendError::SpawnFailed` --
    /// which is not what is being asserted. What is asserted is the recorded
    /// value.
    fn job_reaching_the_send_spawn(
        job: Option<&crate::job_object::KillOnCloseJob>,
    ) -> Vec<Option<usize>> {
        // `build_command` refuses outright unless startup recorded a
        // signature-verified CLI. First-wins and idempotent, so this is safe
        // however the test order falls out, and the path is a fiction that is
        // never executed.
        crate::bw_path::remember_verified_bw_exe(std::path::PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));

        let runner = CliSendRunner::new(job, None);
        let probe = crate::job_object::spawn_probe::SpawnProbe::arm();
        let outcome = list_sends(&runner);
        let attempts = probe.attempts();
        drop(probe);

        assert!(
            matches!(outcome, Err(SendError::SpawnFailed(_))),
            "the probe did not refuse the spawn, so this test may have started a process: \
             {outcome:?}"
        );
        assert_eq!(
            attempts.len(),
            1,
            "the production Send path did not reach `spawn_in_job` exactly once, so the \
             assertion made on what it carried is about nothing: {attempts:?}"
        );
        // THE RECORDED SPAWN IS THE SEND, and not merely *a* spawn that
        // happened, and the Send's answer CAME FROM IT. Round six of this
        // finding (in `vault_export`) satisfied a probe assertion exactly like
        // the one below with a one-line decoy while the real child spawned by
        // another route; the decoy is now unwritable here, and these checks are
        // what would say so if it ever became writable again -- including via
        // a helper in some file `job_object`'s tree walk excuses, because a
        // child started anywhere else would answer instead of this refusal.
        match &outcome {
            Err(SendError::SpawnFailed(why)) => assert!(
                why.contains(crate::job_object::spawn_probe::REFUSED),
                "the Send failed for some other reason, so its result did not come from the \
                 call the probe recorded: {why}"
            ),
            other => panic!(
                "the probe refused the only spawn this Send may make, yet it reported \
                 {other:?}, so a child was started by a route the probe cannot see"
            ),
        }
        let program = attempts[0].program.to_string_lossy().to_lowercase();
        assert!(
            program.ends_with("bw.exe"),
            "the recorded spawn is not the CLI, so it is not the Send: {program}"
        );
        let args: Vec<String> = attempts[0]
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            list_invocation(None).args(),
            "the recorded spawn does not carry this Send's arguments, so it is not the Send"
        );

        attempts.iter().map(|a| a.job).collect()
    }

    /// The one spawn a read path makes when driven end to end through the
    /// production runner: its argument vector and its environment overlay.
    ///
    /// No process is created: `job_object::spawn_probe` stands where
    /// `CreateProcess` would and refuses, so `run` takes its ordinary
    /// spawn-failed path. The checks here are only that the recorded attempt
    /// really is this Send's -- the caller asserts what it carried.
    #[allow(clippy::type_complexity)]
    fn the_one_spawn(
        op: impl FnOnce() -> Result<(), SendError>,
    ) -> (Vec<String>, Vec<(String, Option<String>)>) {
        crate::bw_path::remember_verified_bw_exe(std::path::PathBuf::from(
            r"C:\deskwarden-testirstw.exe",
        ));
        let probe = crate::job_object::spawn_probe::SpawnProbe::arm();
        let outcome = op();
        let attempts = probe.attempts();
        drop(probe);

        match &outcome {
            Err(SendError::SpawnFailed(why)) => assert!(
                why.contains(crate::job_object::spawn_probe::REFUSED),
                "the read failed for some other reason, so its result did not come from the                  call the probe recorded: {why}"
            ),
            other => panic!(
                "the probe refused the only spawn this read may make, yet it reported                  {other:?}, so a child was started by a route the probe cannot see"
            ),
        }
        assert_eq!(
            attempts.len(),
            1,
            "the production read path did not reach `spawn_in_job` exactly once, so any              assertion about what it carried is about nothing: {attempts:?}"
        );
        let attempt = attempts.into_iter().next().expect("just counted one");
        assert!(
            attempt
                .program
                .to_string_lossy()
                .to_lowercase()
                .ends_with("bw.exe"),
            "the recorded spawn is not the CLI, so it is not the read: {:?}",
            attempt.program
        );
        // Handed back as plain strings rather than as the recorded value
        // itself: `job_object`'s
        // `the_two_job_bearing_modules_cannot_name_a_bare_command` keeps this
        // file from NAMING items in that module, and the point here is what
        // the child would have been given, not the recorder's type.
        (
            attempt
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            attempt
                .envs
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.as_ref().map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn the_session_the_list_runner_holds_reaches_the_child_and_never_its_argv() {
        // THE GAP, held at the spawn itself and not at any accessor.
        //
        // `bw send list` against a real vault answers "locked" unless the
        // child inherits `BW_SESSION`. Asserting that on the invocation, or
        // on `build_command`, would say nothing about what the production
        // path actually hands to `CreateProcess` -- this crate has lost that
        // argument repeatedly -- so `list_sends` is driven end to end through
        // the real `CliSendRunner` and the real `spawn_in_job`, and what is
        // read back is the environment overlay that ARRIVED there.
        //
        // The second half is the other side of the same rule: a process's
        // argument vector is readable by every other process on the machine,
        // so the token that unlocks the whole vault must appear in no element
        // of it. `--session` on the command line would satisfy the first
        // assertion and fail this one.
        let runner = CliSendRunner::with_session(None, None, SESSION);
        let (args, envs) = the_one_spawn(|| list_sends(&runner).map(|_| ()));

        let session_values: Vec<Option<String>> = envs
            .iter()
            .filter(|(k, _)| k == SESSION_ENV)
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(
            session_values,
            vec![Some(SESSION.to_string())],
            "`{SESSION_ENV}` did not arrive at the child set exactly once to the token the              runner was built with, so a real `bw send list` answers `locked`; the overlay              that arrived was {envs:?}"
        );

        for arg in &args {
            assert!(
                !arg.contains(SESSION),
                "the session token is in argv, where any process on the machine can read it:                  {arg}"
            );
        }
        assert!(
            !SESSION.is_empty() && SESSION.len() > 8,
            "control: the token searched for in argv is a real string, so the loop above is              not vacuous"
        );
        assert_eq!(
            args,
            vec!["send".to_string(), "list".to_string()],
            "control: the recorded spawn does not carry this list's arguments, so the argv              check above is about some other command"
        );

        // Control, and the mutant it kills: a runner built with no session is
        // still DISTINGUISHABLE from one that has it. Without this, a
        // `build_command` that set `BW_SESSION` unconditionally out of some
        // other source would pass the assertion above.
        let jobless = CliSendRunner::new(None, None);
        let (_, jobless_envs) = the_one_spawn(|| list_sends(&jobless).map(|_| ()));
        assert!(
            jobless_envs.iter().all(|(k, _)| k != SESSION_ENV),
            "control: a runner configured with no session still set {SESSION_ENV}, so the              assertion above cannot tell the configured token from an invented one:              {jobless_envs:?}"
        );
    }

    #[test]
    fn the_session_the_delete_runner_holds_reaches_the_child_and_never_its_argv() {
        // The same hole existed on the revoke path, and it is the more
        // dangerous of the two: a `bw send delete` that answers "locked"
        // leaves a public link live while the screen reports a failure.
        //
        // The Send's id DOES belong in argv -- it is not a secret and `bw`
        // takes it positionally -- so this asserts the TOKEN's absence
        // specifically rather than "argv is two words".
        const ID: &str = "send-id-to-revoke";
        let runner = CliSendRunner::with_session(None, None, SESSION);
        let (args, envs) = the_one_spawn(|| delete_send(&runner, ID));

        assert_eq!(
            envs.iter()
                .filter(|(k, _)| k == SESSION_ENV)
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>(),
            vec![Some(SESSION.to_string())],
            "`{SESSION_ENV}` did not arrive at the child set exactly once to the token the runner was built with, so a real `bw send delete` answers `locked` and the link stays live: {envs:?}"
        );
        assert_eq!(
            args,
            vec!["send".to_string(), "delete".to_string(), ID.to_string()],
            "control: the recorded spawn is not this revoke"
        );
        for arg in &args {
            assert!(
                !arg.contains(SESSION),
                "the session token is in argv, where any process on the machine can read it: {arg}"
            );
        }
    }

    #[test]
    fn the_send_reaches_the_spawn_carrying_the_job_the_runner_was_built_with() {
        // THE CRITICAL FINDING, held at the spawn itself.
        //
        // `the_job_the_runner_was_given_is_the_job_the_child_is_spawned_into`
        // above interrogates `CliSendRunner::job`, and that is not enough: an
        // accessor answers about the receiver it is called on, and the spawn
        // is free to use a different one. Measured, a `run` of
        // `CliSendRunner::new(None, self.data_dir).run_inner(inv)` left that
        // test green, the `spawn_in_job(self.job(), command)` pin word-perfect,
        // 0 failed and 0 warnings -- while every `bw` child, a process holding
        // the session token, spawned outside the kill-on-close job.
        //
        // So nothing here reads a line of source and nothing here calls an
        // accessor. `list_sends` -- a production entry point -- is driven
        // through the real `CliSendRunner` and the real
        // `crate::job_object::spawn_in_job`, and what is asserted is the job
        // that ARRIVED at that spawn. Another hop, another receiver, another
        // module: whichever route the work takes it arrives here, and the job
        // it arrives with must be this one.
        //
        // `KillOnCloseJob::new` creates a kernel handle: no process, no file,
        // no socket. The probe refuses before `CreateProcess`, so this test
        // starts nothing either.
        let job = crate::job_object::KillOnCloseJob::new()
            .expect("a job object is a handle, not a process");
        let given = std::ptr::from_ref(&job) as usize;

        assert_eq!(
            job_reaching_the_send_spawn(Some(&job)),
            vec![Some(given)],
            "the `bw` child was spawned with a job that is not the one the runner was built \
             with, so a CLI holding the session token would not die with this process"
        );

        // Control: a DIFFERENT job is distinguishable, so the equality above
        // is not satisfied by any job at all.
        let other = crate::job_object::KillOnCloseJob::new()
            .expect("a second handle, still not a process");
        assert_eq!(
            job_reaching_the_send_spawn(Some(&other)),
            vec![Some(std::ptr::from_ref(&other) as usize)]
        );
        assert_ne!(std::ptr::from_ref(&other) as usize, given);

        // Control: "no job at all" is still expressible end to end and is
        // DISTINGUISHABLE from a job -- which is exactly the difference every
        // mutant in this family erased. It is also the shape production uses
        // today: the vault window holds no job.
        assert_eq!(
            job_reaching_the_send_spawn(None),
            vec![None],
            "control: a runner built with no job did not reach the spawn jobless, so the probe \
             cannot tell a child inside the job from one outside it"
        );
    }

    #[test]
    fn debug_printing_an_invocation_shows_neither_the_body_nor_the_session() {
        // A `log::debug!("{inv:?}")` at any future call site is one line
        // away, and the derived `Debug` this replaced printed
        // `session_token: Some(Zeroizing("..."))` verbatim plus a base64 body
        // that decodes to the Send's text and its share password.
        let inv = create_invocation();
        let printed = format!("{inv:?}");

        assert!(
            printed.contains("send") && printed.contains("create"),
            "control: the arguments still print, so this Debug is not empty: {printed}"
        );
        assert!(
            !printed.contains(SESSION),
            "the session token -- the key to the whole vault -- is in the debug output: \
             {printed}"
        );
        let body = inv.stdin_json_b64();
        assert!(body.len() > 32, "control: there is a body to leak");
        assert!(
            !printed.contains(&body[..32]),
            "the encoded request body is in the debug output; it decodes to the secret being \
             published and to the share password: {printed}"
        );

        // **The read invocations carry the token too, now that they are
        // authenticated**, and the same `Debug` has to hide it there. Without
        // this, closing the session gap would have quietly reopened the
        // logging leak the hand-written `Debug` exists for.
        for inv in [
            list_invocation(Some(SESSION)),
            delete_invocation("send-id-1", Some(SESSION)),
        ] {
            let printed = format!("{inv:?}");
            assert!(
                inv.session_token() == Some(SESSION),
                "control: this invocation really is carrying the token: {printed}"
            );
            assert!(
                printed.contains("send"),
                "control: the arguments still print: {printed}"
            );
            assert!(
                !printed.contains(SESSION),
                "the session token -- the key to the whole vault -- is in the debug output of                  a read invocation: {printed}"
            );
        }

        // The plan the invocation was built from, same rule.
        let printed_plan = format!("{:?}", a_plan());
        assert!(
            printed_plan.contains("Wi-Fi password"),
            "control: the plan's Debug still identifies the plan: {printed_plan}"
        );
        assert!(
            !printed_plan.contains(SECRET) && !printed_plan.contains(SHARE_PASSWORD),
            "the plan's debug output carries the secret body or the share password: \
             {printed_plan}"
        );

        // And the control that says WHY the wrappers are not what protects
        // these: `Zeroizing`'s own `Debug` forwards straight to the inner
        // value, so it buys nothing at all against a format string.
        assert!(
            format!("{:?}", Zeroizing::new(SESSION.to_string())).contains(SESSION),
            "control: if `Zeroizing` had started redacting on its own, the assertions above \
             would pass without either hand-written `Debug` doing anything"
        );
    }

    #[test]
    fn the_timeout_message_is_true_of_a_revoke_and_of_a_list_as_well() {
        // `TimedOut` is answered by `CliSendRunner::run` whatever the
        // invocation was, so it reaches the user from all three operations.
        // Its sentence used to say a Send "may have been created anyway",
        // which is false on a timed-out revoke -- where the worry is the
        // opposite one, and was never said.
        struct TimingOut;
        impl SendRunner for TimingOut {
            fn run(&self, _inv: &SendInvocation) -> Result<RawOutput, SendError> {
                Err(SendError::TimedOut)
            }
        }

        assert_eq!(
            delete_send(&TimingOut, "the-id"),
            Err(SendError::TimedOut),
            "a revoke that timed out must reach the user as the ambiguous failure"
        );
        assert_eq!(list_sends(&TimingOut), Err(SendError::TimedOut));
        assert_eq!(
            create_send(&TimingOut, &a_plan(), SESSION, &NOW),
            Err(SendError::TimedOut)
        );

        let message = SendError::TimedOut.user_message().to_ascii_lowercase();
        assert!(
            !message.contains("created"),
            "the timeout sentence claims something was created; it is shown for a revoke and \
             for a list too, where that is simply false: {message:?}"
        );
        assert!(
            message.contains("revoke"),
            "the true worry on a timed-out revoke -- that the public link may still be live \
             -- is still not said: {message:?}"
        );
        assert!(
            message.contains("check your sends list"),
            "an ambiguous failure must send the user to the list rather than offer a plain \
             try again: {message:?}"
        );
    }

    #[test]
    fn the_wait_loop_gives_up_only_once_the_cap_has_passed() {
        let cap = Duration::from_secs(60);
        assert_eq!(
            wait_decision(false, Duration::ZERO, cap),
            WaitDecision::KeepWaiting
        );
        assert_eq!(
            wait_decision(false, Duration::from_secs(59), cap),
            WaitDecision::KeepWaiting
        );
        assert_eq!(
            wait_decision(false, cap - Duration::from_millis(1), cap),
            WaitDecision::KeepWaiting,
            "one millisecond short of the cap is still inside it"
        );
        assert_eq!(wait_decision(false, cap, cap), WaitDecision::GiveUp);
        assert_eq!(
            wait_decision(false, Duration::from_secs(3600), cap),
            WaitDecision::GiveUp
        );
        assert_eq!(
            wait_decision(true, Duration::from_secs(3600), cap),
            WaitDecision::Finished,
            "a child that finished produced a result; calling that a timeout would report an \
             answer this app actually has as an ambiguous non-answer"
        );
        assert_eq!(
            SEND_TIMEOUT,
            Duration::from_secs(60),
            "the cap the loop actually uses is no longer the sixty seconds documented"
        );
        assert!(
            POLL_INTERVAL < SEND_TIMEOUT,
            "control: the loop polls many times inside the cap"
        );
    }

    #[test]
    fn giving_up_on_the_child_is_ambiguous_and_never_a_clean_failure() {
        assert!(
            SendError::TimedOut.is_ambiguous(),
            "a Send may have been created before this app stopped waiting"
        );
        // The other route to the same answer: a killed child has no exit
        // code, and that must not be read as a clean non-zero failure either.
        let killed = raw_output_from(None, b"", b"");
        assert_eq!(
            classify_failure(killed.exit_code, &killed.stdout, &killed.stderr),
            SendError::TimedOut
        );
        assert_ne!(
            killed.exit_code,
            Some(0),
            "a killed child must not read as a clean exit"
        );
    }

    #[test]
    fn the_captured_streams_become_the_output_the_parsers_read() {
        let out = raw_output_from(Some(0), b"{\"id\":\"send-1\"}", b"");
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout, "{\"id\":\"send-1\"}");
        assert_eq!(out.stderr, "");

        // Not UTF-8, and the rest of the sentence survives: it is the only
        // explanation the user will get.
        let lossy = raw_output_from(Some(1), b"", b"could not \xff reach the server");
        assert!(
            lossy.stderr.contains("could not") && lossy.stderr.contains("reach the server"),
            "a single bad byte swallowed the CLI's message: {:?}",
            lossy.stderr
        );
        assert!(
            matches!(
                classify_failure(lossy.exit_code, &lossy.stdout, &lossy.stderr),
                SendError::Rejected(_)
            ),
            "the recovered sentence did not reach the classifier"
        );
    }

    /// **Source pins, not tests -- and each one says why it cannot be a
    /// test.**
    ///
    /// Three properties of [`CliSendRunner::run`] are facts about a process
    /// that exists only once the CLI has actually been spawned: that the child
    /// joins the kill-on-close job, that it comes up with no console
    /// (`CREATE_NO_WINDOW`), and that the command was built by
    /// [`crate::bw_path`] rather than by hand. **No test in this crate may
    /// spawn a process**, and the fake runner the rest of this module's tests
    /// use never starts one -- so a fake proves none of the three. They are
    /// therefore pinned over the source text, in the shape `bw_path.rs:889`
    /// and `login_ui.rs:3530` established: every needle is `concat!`-split so
    /// that it cannot match its own declaration here, every needle is
    /// single-line so that a CRLF checkout cannot turn it into a false pass,
    /// and every needle is *required*, so the assertion is itself the evidence
    /// that it still matches live code.
    ///
    /// Two more are pinned for the same reason: the stdin handle being taken
    /// out of the child and dropped before anything is read (leaving it in
    /// place deadlocks until the cap, which no test can see), and the
    /// give-up branch returning the ambiguous failure.
    /// This file's own text, **above the first test module only**, so a
    /// needle spelled out in a test cannot satisfy the pin looking for it.
    ///
    /// `vault_export.rs` and `file_picker.rs` split this way already; this
    /// file did not, and no needle happened to be duplicated below the cut
    /// today -- which is a fact about today rather than a property. The cut
    /// marker is `#[cfg(test)]` followed by `mod `, because the bare
    /// attribute also sits above `ARGV_PIN_CONTROL` near the top of the file
    /// and splitting there would leave the pins searching sixty lines.
    fn code_under_test() -> String {
        // Normalised to LF once, so a needle written with `\n` matches this
        // file whether it is stored CRLF or LF.
        let whole = include_str!("send.rs").replace("\r\n", "\n");
        let code = whole
            .split(concat!("#[cfg(test)]", "\nmod "))
            .next()
            .unwrap()
            .to_string();
        assert!(
            code.len() < whole.len(),
            "the test module marker was not found; the split did nothing"
        );
        code
    }

    #[test]
    fn the_source_pin_search_can_tell_present_from_absent() {
        // The positive control every pin below depends on. Without it, a
        // `code_under_test` that answered an empty string would make each
        // `!contains` pin pass while asserting nothing whatsoever.
        let code = code_under_test();
        assert!(code.contains("struct CliSendRunner<'a> {"));
        assert!(!code.contains("no such line appears anywhere in this module"));
        // And the split really did drop the tests: this function's own name
        // is below the cut.
        assert!(
            !code.contains("the_source_pin_search_can_tell_present_from_absent"),
            "the cut did not drop the test modules, so a needle spelled in a test would \
             satisfy the pin looking for it"
        );
    }

    #[test]
    fn the_spawn_time_properties_no_test_can_observe_are_pinned_to_the_source() {
        let source = code_under_test();
        for (required, why) in [
            (
                concat!("bw_path::bw_job_command_in(", "self.data_dir)"),
                "the command is no longer built by the one module that names the executable \
                 whose signature startup verified",
            ),
            // The `spawn_in_job(self.job(), command)` needle that used to sit
            // here is DELETED, and its deletion is the point of this change.
            // It was defeated for the fifth time by a `run` that handed the
            // work to `CliSendRunner::new(None, self.data_dir).run_inner(inv)`
            // -- word-perfect needle, honest accessor, 0 failed, 0 warnings,
            // every child outside the job. A pin on `self.job()` cannot see
            // which `self` it is, so a sixth spelling of it would lose to a
            // sixth hop. The property is now held by
            // `the_send_reaches_the_spawn_carrying_the_job_the_runner_was_built_with`,
            // which observes the spawn, and by `job_object`'s tree walk, which
            // proves this file has no other route to a child.
            (
                concat!("child.stdin.", "take()"),
                "the stdin handle is left inside the child, so the pipe stays open, the CLI \
                 keeps reading to a end-of-file that never comes, and the only symptom is the \
                 sixty-second cap",
            ),
            (
                concat!("drop(", "pipe);"),
                "the write handle is no longer closed before the output is read",
            ),
            (
                concat!("None => Err(SendError::", "TimedOut),"),
                "giving up on the child no longer reports the ambiguous failure, so a Send \
                 that may exist would render as a clean failure",
            ),
        ] {
            // Counted, not merely required: a presence-only needle is
            // satisfied by construction by a second, overriding line added
            // beside the pinned one.
            assert_eq!(
                source.matches(required).count(),
                1,
                "`{required}` no longer appears exactly once: {why}"
            );
        }

        // `CREATE_NO_WINDOW` is the third, and it is not in this file at all.
        // `spawn_in_job` re-ORs it because `creation_flags` REPLACES the flags
        // a command holds rather than adding to them, so setting
        // `CREATE_SUSPENDED` alone silently drops it and the child comes up
        // with a real console attached. Pinned where it lives, because this
        // module's correctness depends on it.
        let job_source = include_str!("job_object.rs");
        assert!(
            job_source.contains(concat!(
                "creation_flags(crate::bw_path::CREATE_NO_WINDOW",
                " | CREATE_SUSPENDED.0)"
            )),
            "`spawn_in_job` no longer re-applies CREATE_NO_WINDOW, so every Send would flash a \
             console window on screen"
        );
    }

    #[test]
    fn the_two_secret_bearing_flags_still_appear_nowhere_in_this_file() {
        // The pure half pins this too; it is repeated here because this is the
        // step that builds a real command line, and a flag added to the runner
        // rather than to `plan_to_invocation` would slip past a pin that only
        // ever looked at the invocation builders.
        let source = include_str!("send.rs");
        for forbidden in [concat!("--", "password"), concat!("--", "emails")] {
            assert!(
                !source.contains(forbidden),
                "`{forbidden}` is spelled in this file: every field that carries a secret or a \
                 recipient list goes in as JSON on stdin, never as a flag"
            );
        }
        assert!(
            source.contains(concat!("--", "password").trim_start_matches('-')),
            "control: the needles above are not vacuous -- the bare word is present, so the \
             absence asserted is the absence of the flag spelling"
        );
    }

    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no pin in this module reads.
    //
    // Every pin here reads `code_under_test`, which is this file ABOVE the
    // first test module and nothing else. The two controls it already had
    // prove the search can tell present from absent and that the cut dropped
    // the tests. NEITHER proves the cut does not drop PRODUCTION code, and
    // measured on the commit before this one it did not: a real `pub fn`
    // containing a bare direct spawn appended at the end of this file, below
    // both test modules, gave 2050 lib + 217 bin, 0 failed, 0 warnings --
    // unseen by the pins here and by `bw_path`'s crate-wide spawn guard.
    // Same shape as the control `breach.rs` already carries. (The spawn half
    // of that hazard is now also held from OUTSIDE this file, by
    // `job_object`'s tree walk, which reads whole files and so has no cut to
    // append below; this walk still holds everything else.)
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a search for the
    /// real attribute.
    const CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Empty today, and controlled by the walk.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// Where [`code_under_test`] cuts this file: the FIRST [`CUT_GATE`] that
    /// is immediately followed by a module opener.
    ///
    /// Written as a scan rather than as a `find` of `"#[cfg(test)]\nmod "`
    /// because a bare `\n` needle matches NOTHING in a CRLF working tree, and
    /// this repository stores LF blobs while `core.autocrlf=true` checks them
    /// out as CRLF. A needle that matched nothing here would make the walk
    /// below silently read the wrong region -- or none at all.
    ///
    /// The gate alone is not the cut: this file also carries one near the top,
    /// above `ARGV_PIN_CONTROL`, and cutting there would leave every pin
    /// searching sixty lines of imports.
    fn cut_index(source: &str) -> usize {
        let mut from = 0usize;
        while let Some(hit) = source[from..].find(CUT_GATE) {
            let at = from + hit;
            let rest = &source[at + CUT_GATE.len()..];
            let rest = rest.strip_prefix('\r').unwrap_or(rest);
            if rest.starts_with("\nmod ") {
                return at;
            }
            from = at + CUT_GATE.len();
        }
        panic!("no `cfg(test)` attribute in this file is followed by a module opener");
    }

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Exact rather than a `starts_with`: a whole
    /// module written on one line is not a module opener here and must fail.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let rest = match t.strip_prefix("mod ") {
            Some(rest) => rest,
            None => return false,
        };
        let name = match rest.strip_suffix(" {") {
            Some(name) => name,
            None => return false,
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk from the cut to EOF over whatever text it is handed.
    /// Returns `(visited, modules, closes, depth)` so the caller can control
    /// it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose**, both in [`cut_index`] and here:
    /// `lines()` strips a trailing carriage return, so every comparison is
    /// against the line's real text on a CRLF tree and on an LF one alike.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = cut_index(source);
        let mut depth = 0usize;
        // The walked region BEGINS with the gate, so nothing outside it is
        // taken on trust: the first line seen is the attribute itself.
        let mut gated = false;
        let (mut modules, mut closes, mut visited) = (0usize, 0usize, 0usize);
        for line in source[cut..].lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments,
                // the gate and a module opener -- at ANY indentation, because
                // an indented `fn` at file scope is still a top-level item
                // and a column-0-only filter would walk straight past it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. Every pin in this module reads \
                     only the half ABOVE the cut, so an item down here is read by none of \
                     them: it can spawn a process, reintroduce a flag banned by name, or \
                     duplicate a call site pinned at exactly one -- and the suite stays green. \
                     Move it above the first test module."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and it \
                     ships in the half of the file no pin here reads"
                );
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_pins_cut() {
        let source = include_str!("send.rs");
        let lf = source.replace("\r\n", "\n");

        // 1. The cut this control walks from is the SAME byte
        //    `code_under_test` cuts at, or the walk proves nothing about the
        //    region the pins cannot see.
        let cut = cut_index(&lf);
        assert_eq!(
            &lf[..cut],
            code_under_test().as_str(),
            "this control walks from a different byte than `code_under_test` cuts at, so the \
             region it inspects is not the region the pins are blind to"
        );

        // 2. Positive control on WHERE the cut is: the production half still
        //    reaches the last production item in the file. Were the cut to
        //    move UP -- into a doc comment or a string that happened to spell
        //    a gate followed by a module opener -- this anchor would fall
        //    below it and every pin downstream would be reading nothing.
        const LAST_PRODUCTION_ITEM: &str = concat!("None => Err(SendError::", "TimedOut),");
        assert_eq!(
            lf.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor is not in this file exactly once, so it pins nothing -- \
             repoint it at the last production item above the first test module"
        );
        let anchor = lf.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, so the cut \
             moved up and the production half every pin reads is truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, over an LF copy and a CRLF copy of the same text, which
        //    must agree. Built both ways rather than compared against the
        //    bytes on disk: this repository stores LF blobs and only
        //    `core.autocrlf=true` makes a working tree CRLF, so a control that
        //    asserted "this file is CRLF" would pass here and fail on Linux.
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        assert_eq!(
            walk_below_the_cut(&lf),
            walk_below_the_cut(&crlf),
            "the walk gives a different answer on an LF copy of this file than on a CRLF one"
        );
        let on_disk = walk_below_the_cut(source);
        assert!(
            on_disk == walk_below_the_cut(&lf) || on_disk == walk_below_the_cut(&crlf),
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty and this test proves nothing"
        );
        assert_eq!(
            (modules, closes, depth),
            (2, 2, 0),
            "below the cut there are no longer exactly two opened-and-closed test modules: \
             {modules} opened, {closes} closed, ending at depth {depth}"
        );

        // 5. Control on the walk itself: it really refuses production code
        //    down there. Without this the walk could be a no-op that visits
        //    lines and asserts nothing.
        let with_an_appended_item = format!("{lf}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_appended_item)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test modules, which is \
             the exact mutation it exists to catch"
        );
        // And an INDENTED one, which a column-0-only filter would miss.
        let with_an_indented_item = format!("{lf}\n    struct Sneaked(u8);\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_indented_item)).is_err(),
            "control: the walk accepted an INDENTED top-level item appended below the test \
             modules"
        );
        // And an ungated module, which ships.
        let with_an_ungated_module = format!("{lf}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_ungated_module)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships"
        );
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 2026-08-11T00:43:17.148Z, chosen so that every deletion date in these
    /// tests crosses a month boundary at 30 days and none of them is a
    /// midnight that would hide a time-of-day bug.
    const NOW: FixedClock = FixedClock(1_786_408_997_148);

    fn plan() -> SendPlan {
        SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new("hunter2".to_string()),
            ..SendPlan::default()
        }
    }

    fn b64_decode(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut acc = 0u32;
        let mut bits = 0u32;
        for byte in s.bytes() {
            let six = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => continue,
                _ => panic!("the invocation's body is not base64: {byte:?}"),
            };
            acc = (acc << 6) | u32::from(six);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    /// The invocation's body, decoded back into JSON. **Every differential
    /// assertion below reads this**, not the plan it built the invocation
    /// from: asserting that a value was handed to `plan_to_invocation` says
    /// nothing about whether `plan_to_invocation` used it.
    fn body_of(inv: &SendInvocation) -> serde_json::Value {
        let bytes = b64_decode(inv.stdin_json_b64());
        serde_json::from_slice(&bytes).expect("the body is JSON")
    }

    #[test]
    fn the_base64_round_trips() {
        // Control on the decoder every other test in this file leans on. If
        // this is wrong, every differential assertion below is reading
        // something other than what was built.
        for sample in ["", "a", "ab", "abc", "abcd", "{\"x\":1}", "\u{00e9}\u{4e2d}"] {
            let mut encoded = String::new();
            base64_into(&mut encoded, sample.as_bytes());
            assert_eq!(
                b64_decode(&encoded),
                sample.as_bytes(),
                "base64 round trip failed for {sample:?} (encoded as {encoded:?})"
            );
            assert_eq!(encoded.len() % 4, 0, "base64 is not padded to a multiple of 4");
        }
    }

    // -- 1. the differential invocation tests ------------------------------
    //
    // Two plans differing in EXACTLY one field must produce two DIFFERENT
    // invocations, and the difference must be the field. A field dropped on
    // the floor by `plan_to_invocation` makes the two invocations identical,
    // which the first half catches; a field written into the wrong key makes
    // them differ but wrongly, which the second half catches. Neither half is
    // sufficient alone.

    fn differ(base: &SendPlan, variant: &SendPlan) -> (serde_json::Value, serde_json::Value) {
        let a = plan_to_invocation(base, "sess", &NOW).expect("the base plan is valid");
        let b = plan_to_invocation(variant, "sess", &NOW).expect("the variant plan is valid");
        assert!(
            a != b,
            "two plans differing in one field produced byte-identical invocations, so that \
             field never reached `bw` at all: {:?}",
            a.stdin_json_b64()
        );
        (body_of(&a), body_of(&b))
    }

    #[test]
    fn delete_in_days_reaches_the_built_json() {
        let base = SendPlan { delete_in_days: 7, ..plan() };
        let variant = SendPlan { delete_in_days: 30, ..plan() };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["deletionDate"], "2026-08-18T00:43:17.148Z");
        assert_eq!(b["deletionDate"], "2026-09-10T00:43:17.148Z");
    }

    #[test]
    fn max_access_count_reaches_the_built_json() {
        let base = plan();
        let variant = SendPlan { max_access_count: Some(3), ..plan() };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["maxAccessCount"], serde_json::Value::Null);
        assert_eq!(b["maxAccessCount"], 3);
    }

    #[test]
    fn the_share_password_reaches_the_built_json() {
        let base = plan();
        let variant = SendPlan {
            password: Some(Zeroizing::new("open-sesame".to_string())),
            ..plan()
        };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["password"], serde_json::Value::Null);
        assert_eq!(b["password"], "open-sesame");
    }

    #[test]
    fn the_hidden_flag_reaches_the_built_json() {
        let base = SendPlan { hidden: false, ..plan() };
        let variant = SendPlan { hidden: true, ..plan() };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["text"]["hidden"], false);
        assert_eq!(b["text"]["hidden"], true);
    }

    #[test]
    fn the_name_reaches_the_built_json() {
        let base = plan();
        let variant = SendPlan { name: "Door code".to_string(), ..plan() };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["name"], "Wi-Fi password");
        assert_eq!(b["name"], "Door code");
    }

    #[test]
    fn the_secret_body_reaches_the_built_json_and_is_escaped() {
        let base = plan();
        let variant = SendPlan {
            text: Zeroizing::new("line\none\t\"quoted\"\\ \u{0001}".to_string()),
            ..plan()
        };
        let (a, b) = differ(&base, &variant);
        assert_eq!(a["text"]["text"], "hunter2");
        assert_eq!(b["text"]["text"], "line\none\t\"quoted\"\\ \u{0001}");
    }

    #[test]
    fn the_session_token_reaches_the_invocation_and_not_its_arguments() {
        let inv = plan_to_invocation(&plan(), "sess-abc-123", &NOW).expect("valid");
        assert_eq!(inv.session_token(), Some("sess-abc-123"));
        assert!(
            !inv.args().iter().any(|a| a.contains("sess-abc-123")),
            "the session token reached argv: {:?}",
            inv.args()
        );
    }

    // -- 2. argv hygiene, both ways ----------------------------------------

    #[test]
    fn the_built_invocation_never_carries_a_secret_in_its_arguments() {
        let full = SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new("s3cr3t-body".to_string()),
            hidden: true,
            delete_in_days: 30,
            password: Some(Zeroizing::new("share-pw".to_string())),
            max_access_count: Some(2),
        };
        let inv = plan_to_invocation(&full, "sess", &NOW).expect("valid");

        // Control: the plan really does carry both secrets, so the absences
        // below are about the invocation and not about an empty plan.
        assert_eq!(&*full.text, "s3cr3t-body");
        assert_eq!(full.password.as_deref().map(String::as_str), Some("share-pw"));
        // Control: and both really are in the body, so "not in argv" is a
        // statement about where they went rather than about them vanishing.
        let body = body_of(&inv);
        assert_eq!(body["text"]["text"], "s3cr3t-body");
        assert_eq!(body["password"], "share-pw");

        let argv = inv.args().join(" ");
        assert_eq!(argv, "send create", "the argument vector grew something: {argv:?}");
        for needle in ["s3cr3t-body", "share-pw", "sess"] {
            assert!(
                !argv.contains(needle),
                "{needle:?} reached the command line, where every process on this machine \
                 can read it: {argv:?}"
            );
        }
        for flag in [concat!("--", "password"), concat!("--", "emails")] {
            assert!(
                !argv.contains(flag),
                "{flag:?} is in the argument vector; those two flags put a secret and a \
                 recipient list in argv and this module does not use them"
            );
        }
    }

    /// The source pin. The test above only covers the plans it happens to
    /// build; this covers every plan there could ever be, by saying the two
    /// flags are not written anywhere in this file at all -- not in
    /// production, not in a doc comment a future reader could copy.
    #[test]
    fn the_two_secret_bearing_flags_appear_nowhere_in_this_file() {
        let source = include_str!("send.rs");
        // Positive controls: the file was really read, and `contains` really
        // finds things in it. Without these the assertions below would pass
        // against an empty string or a misspelt path.
        assert!(source.len() > 5_000, "send.rs read as {} bytes", source.len());
        assert!(
            source.contains(ARGV_PIN_CONTROL),
            "the control marker is missing, so this test is not reading this file"
        );

        for flag in [concat!("--", "password"), concat!("--", "emails")] {
            assert_eq!(
                source.matches(flag).count(),
                0,
                "{flag:?} is spelled in send.rs. Both of those flags carry a secret or a \
                 recipient list on the command line, where every process on the machine can \
                 read it. Everything they would carry belongs in the JSON body, which goes \
                 over stdin."
            );
        }
    }

    // -- 3. classify_failure, every arm ------------------------------------

    /// A short name for each arm, so the table below can assert that every one
    /// of the eight was actually produced rather than merely listed.
    fn arm_name(err: &SendError) -> &'static str {
        match err {
            SendError::NoVerifiedCli(_) => "NoVerifiedCli",
            SendError::Locked => "Locked",
            SendError::Offline => "Offline",
            SendError::Rejected(_) => "Rejected",
            SendError::FailedSilently => "FailedSilently",
            SendError::CreatedButUnreadable => "CreatedButUnreadable",
            SendError::TimedOut => "TimedOut",
            SendError::SpawnFailed(_) => "SpawnFailed",
        }
    }

    const ALL_ARMS: [&str; 8] = [
        "NoVerifiedCli",
        "Locked",
        "Offline",
        "Rejected",
        "FailedSilently",
        "CreatedButUnreadable",
        "TimedOut",
        "SpawnFailed",
    ];

    #[test]
    fn every_way_a_bw_run_can_end_is_classified() {
        // (why, exit code, stdout, stderr, expected arm, expected ambiguity)
        let rows: [(&str, Option<i32>, &str, &str, &str, bool); 10] = [
            (
                "exit 0 with output nothing can read: a Send PROBABLY EXISTS",
                Some(0),
                "Send created: see https://... <-- not JSON",
                "",
                "CreatedButUnreadable",
                true,
            ),
            (
                "exit 0 with empty output: same, and just as ambiguous",
                Some(0),
                "",
                "",
                "CreatedButUnreadable",
                true,
            ),
            ("no exit code at all: given up on", None, "", "", "TimedOut", true),
            (
                "the shell could not find bw",
                Some(127),
                "",
                "'bw' is not recognized as an internal or external command",
                "SpawnFailed",
                false,
            ),
            (
                "spawn failed with an errno rather than a shell message",
                Some(1),
                "",
                "Error: spawn bw ENOENT",
                "SpawnFailed",
                false,
            ),
            (
                "the CLI is there but this app will not run it",
                Some(1),
                "",
                "Authenticode signature could not be verified",
                "NoVerifiedCli",
                false,
            ),
            (
                "the vault is locked",
                Some(1),
                "",
                "You are not logged in.",
                "Locked",
                false,
            ),
            (
                "the network is down",
                Some(1),
                "",
                "Error: getaddrinfo ENOTFOUND vault.bitwarden.com",
                "Offline",
                false,
            ),
            (
                "the server said no, and said why",
                Some(1),
                "",
                "Deletion date is in the past.",
                "Rejected",
                false,
            ),
            (
                "a non-zero exit with nothing on either stream",
                Some(1),
                "   \n",
                "\n",
                "FailedSilently",
                false,
            ),
        ];

        assert!(!rows.is_empty(), "control: the table is empty and this test asserts nothing");

        let mut ran = 0usize;
        let mut seen: Vec<&str> = Vec::new();
        for (why, code, stdout, stderr, expected, ambiguous) in rows {
            let err = classify_failure(code, stdout, stderr);
            assert_eq!(arm_name(&err), expected, "{why}: classified as {err:?}");
            assert_eq!(
                err.is_ambiguous(),
                ambiguous,
                "{why}: this failure's ambiguity decides whether the screen may offer a plain \
                 `try again` or must send the user to the Sends list first"
            );
            if !seen.contains(&expected) {
                seen.push(expected);
            }
            ran += 1;
        }
        assert_eq!(ran, rows.len(), "not every row of the table ran");
        seen.sort_unstable();
        let mut want = ALL_ARMS;
        want.sort_unstable();
        assert_eq!(seen, want, "the table does not reach every arm of SendError");
    }

    // -- 4. no failure reads as a success ----------------------------------

    fn one_of_every_arm() -> Vec<SendError> {
        vec![
            SendError::NoVerifiedCli("Bitwarden's tool could not be verified.".to_string()),
            SendError::Locked,
            SendError::Offline,
            SendError::Rejected("Bitwarden would not do it: no.".to_string()),
            SendError::FailedSilently,
            SendError::CreatedButUnreadable,
            SendError::TimedOut,
            SendError::SpawnFailed("Bitwarden's tool could not be started.".to_string()),
        ]
    }

    #[test]
    fn no_failure_message_reads_as_a_success() {
        let arms = one_of_every_arm();
        assert_eq!(arms.len(), ALL_ARMS.len(), "control: not every arm is in the list");

        for err in &arms {
            let message = err.user_message().to_ascii_lowercase();
            assert!(!message.is_empty(), "{err:?} has no message at all");
            if message.contains("created") {
                assert!(
                    message.contains("may") || message.contains("check"),
                    "{err:?} tells the user something was created with no hedge and no \
                     instruction to look: {message:?}. A failure that reads as a success is \
                     how an unrevoked public link goes unnoticed."
                );
            }
        }
    }

    #[test]
    fn only_the_two_failures_that_may_have_published_are_ambiguous() {
        // The ambiguity rule stated positively as well as negatively: it is
        // not enough that the two say `true`; the other six must say `false`,
        // or "ambiguous" degenerates into "a failure happened".
        for err in one_of_every_arm() {
            let expected = matches!(
                err,
                SendError::CreatedButUnreadable | SendError::TimedOut
            );
            assert_eq!(
                err.is_ambiguous(),
                expected,
                "{err:?}: a failure is ambiguous exactly when a public link may exist without \
                 this app knowing its id"
            );
        }
    }

    // -- 5. create_send uses the runner it was given -----------------------

    struct FakeRunner {
        seen: RefCell<Vec<SendInvocation>>,
        answer: RawOutput,
    }

    impl FakeRunner {
        fn answering(answer: RawOutput) -> Self {
            Self { seen: RefCell::new(Vec::new()), answer }
        }
    }

    impl SendRunner for FakeRunner {
        fn run(&self, inv: &SendInvocation) -> Result<RawOutput, SendError> {
            self.seen.borrow_mut().push(inv.clone());
            Ok(self.answer.clone())
        }
    }

    /// A created Send as `bw send create` documents its object. **Constructed
    /// from the documented shape, NOT captured from a real run** -- creating a
    /// real Send would publish a real public link, which nobody has authorised
    /// and which this whole step exists to make impossible. The field names
    /// and the deletion-date format come from `bw send template send.text`,
    /// which was captured; the `id` and `accessUrl` values are invented.
    const CREATED_JSON: &str = r#"{
      "object": "send",
      "id": "9f8b1a2c-0000-4d6e-9c1f-abcdef012345",
      "accessId": "abcdefghijklmnop",
      "accessUrl": "https://send.bitwarden.com/#abcdefghijklmnop/somekeyhere",
      "name": "Wi-Fi password",
      "notes": null,
      "type": 0,
      "text": { "text": "hunter2", "hidden": false },
      "file": null,
      "maxAccessCount": null,
      "accessCount": 0,
      "deletionDate": "2026-08-18T00:43:17.148Z",
      "expirationDate": null,
      "password": null,
      "disabled": false,
      "hideEmail": false,
      "revisionDate": "2026-08-11T00:43:17.148Z",
      "aFieldThisAppHasNeverHeardOf": { "nested": true }
    }"#;

    #[test]
    fn create_send_runs_the_invocation_it_was_given_rather_than_one_of_its_own() {
        let plan = SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new("hunter2".to_string()),
            hidden: true,
            delete_in_days: 30,
            password: Some(Zeroizing::new("share-pw".to_string())),
            max_access_count: Some(4),
        };
        let runner = FakeRunner::answering(RawOutput {
            exit_code: Some(0),
            stdout: CREATED_JSON.to_string(),
            stderr: String::new(),
        });

        let created = create_send(&runner, &plan, "sess-abc", &NOW).expect("the fake exits 0");
        assert_eq!(created.id, "9f8b1a2c-0000-4d6e-9c1f-abcdef012345");

        let seen = runner.seen.borrow();
        assert_eq!(seen.len(), 1, "the runner was called {} times, not once", seen.len());
        let expected = plan_to_invocation(&plan, "sess-abc", &NOW).expect("valid");
        assert!(
            seen[0] == expected,
            "`create_send` ran an invocation of its own making rather than the one \
             `plan_to_invocation` builds, so every guarantee this module's seam offers is \
             about a value that never reached `bw`.\n  ran:      {:?} / {:?}\n  expected: \
             {:?} / {:?}",
            seen[0].args(),
            seen[0].stdin_json_b64(),
            expected.args(),
            expected.stdin_json_b64()
        );
        // And the invocation that actually ran carries the plan, so the
        // equality above is not two identically-empty bodies.
        let body = body_of(&seen[0]);
        assert_eq!(body["text"]["text"], "hunter2");
        assert_eq!(body["password"], "share-pw");
        assert_eq!(body["maxAccessCount"], 4);
        assert_eq!(body["text"]["hidden"], true);
        assert_eq!(body["deletionDate"], "2026-09-10T00:43:17.148Z");
    }

    #[test]
    fn a_clean_exit_with_unreadable_output_is_never_reported_as_a_success() {
        let runner = FakeRunner::answering(RawOutput {
            exit_code: Some(0),
            stdout: "Send created!".to_string(),
            stderr: String::new(),
        });
        let err = create_send(&runner, &plan(), "sess", &NOW)
            .expect_err("unreadable output is not a success");
        assert_eq!(err, SendError::CreatedButUnreadable);
        assert!(err.is_ambiguous());
    }

    #[test]
    fn a_plan_that_does_not_validate_never_reaches_the_runner() {
        let runner = FakeRunner::answering(RawOutput {
            exit_code: Some(0),
            stdout: CREATED_JSON.to_string(),
            stderr: String::new(),
        });
        let bad = SendPlan { name: "   ".to_string(), ..plan() };
        let err = create_send(&runner, &bad, "sess", &NOW).expect_err("a nameless plan is invalid");
        assert_eq!(arm_name(&err), "Rejected");
        assert!(
            runner.seen.borrow().is_empty(),
            "an invalid plan was handed to the runner anyway -- this is the one publishing \
             action in the app and validation is not advisory"
        );
    }

    #[test]
    fn list_and_delete_reach_the_runner_with_the_right_arguments() {
        let runner = FakeRunner::answering(RawOutput {
            exit_code: Some(0),
            stdout: "[]".to_string(),
            stderr: String::new(),
        });
        assert_eq!(list_sends(&runner).expect("an empty list parses"), vec![]);
        delete_send(&runner, "the-id").expect("exit 0 is a delete");
        let seen = runner.seen.borrow();
        assert_eq!(seen[0].args(), ["send", "list"]);
        assert_eq!(seen[1].args(), ["send", "delete", "the-id"]);
        assert_eq!(seen[0].stdin_json_b64(), "", "list sends nothing on stdin");
        assert_eq!(seen[1].stdin_json_b64(), "", "delete sends nothing on stdin");
    }

    #[test]
    fn a_failed_delete_is_a_failure_and_not_a_silent_revocation() {
        let runner = FakeRunner::answering(RawOutput {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "You are not logged in.".to_string(),
        });
        assert_eq!(delete_send(&runner, "the-id"), Err(SendError::Locked));
    }

    // -- 6. the parsers ----------------------------------------------------

    /// **Every fixture in this section is CONSTRUCTED, not captured.** The
    /// field names, the `type` codes and the deletion-date format are taken
    /// from the captured output of `bw send template send.text` and `bw send
    /// --help`; the ids, links and names are invented. No real Send exists to
    /// capture from, because creating one publishes a real public link and
    /// nobody has authorised that.
    const LIST_JSON: &str = r#"[
      {
        "object": "send",
        "id": "aaaa1111-0000-4d6e-9c1f-abcdef012345",
        "accessUrl": "https://send.bitwarden.com/#aaaa/key1",
        "name": "Wi-Fi password",
        "type": 0,
        "text": { "text": "hunter2", "hidden": true },
        "file": null,
        "deletionDate": "2026-08-18T00:43:17.148Z",
        "somethingNew": 42
      },
      {
        "object": "send",
        "id": "bbbb2222-0000-4d6e-9c1f-abcdef012345",
        "accessUrl": "https://send.bitwarden.com/#bbbb/key2",
        "name": "Scan.pdf",
        "type": 1,
        "text": null,
        "file": { "fileName": "Scan.pdf", "size": "1024" },
        "deletionDate": "2026-08-12T00:00:00.000Z"
      }
    ]"#;

    #[test]
    fn a_created_send_is_read_back_with_its_link() {
        let created = parse_created_send(CREATED_JSON).expect("the documented shape parses");
        assert_eq!(
            created,
            CreatedSend {
                id: "9f8b1a2c-0000-4d6e-9c1f-abcdef012345".to_string(),
                name: "Wi-Fi password".to_string(),
                access_url: "https://send.bitwarden.com/#abcdefghijklmnop/somekeyhere"
                    .to_string(),
                deletion_date: "2026-08-18T00:43:17.148Z".to_string(),
            }
        );
    }

    #[test]
    fn the_response_envelope_is_unwrapped_if_it_is_there() {
        let wrapped = format!("{{\"success\":true,\"data\":{CREATED_JSON}}}");
        assert_eq!(
            parse_created_send(&wrapped).expect("the enveloped shape parses"),
            parse_created_send(CREATED_JSON).expect("the bare shape parses"),
            "the two shapes `bw` can print did not read back the same"
        );
    }

    #[test]
    fn a_created_send_without_a_link_is_not_a_success() {
        // Loud, not lenient: an `accessUrl: Option<String>` here would let the
        // app report a Send it cannot show and cannot revoke.
        for (why, stdout) in [
            ("no id", r#"{"accessUrl":"https://x/#a/b","name":"n"}"#),
            ("no link", r#"{"id":"the-id","name":"n"}"#),
            ("an empty link", r#"{"id":"the-id","accessUrl":""}"#),
            ("not JSON at all", "Send created!"),
            ("JSON, but not an object", "[1, 2, 3]"),
        ] {
            assert_eq!(
                parse_created_send(stdout),
                Err(SendError::CreatedButUnreadable),
                "{why}: parsed as a success"
            );
        }
    }

    #[test]
    fn the_sends_list_is_read_including_the_file_sends_this_app_cannot_make() {
        let rows = parse_send_list(LIST_JSON).expect("the documented shape parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            SendSummary {
                id: "aaaa1111-0000-4d6e-9c1f-abcdef012345".to_string(),
                name: "Wi-Fi password".to_string(),
                access_url: "https://send.bitwarden.com/#aaaa/key1".to_string(),
                deletion_date: "2026-08-18T00:43:17.148Z".to_string(),
                is_file: false,
            }
        );
        assert!(rows[1].is_file, "a file Send was listed as a text Send");
        assert_eq!(rows[1].name, "Scan.pdf");
    }

    #[test]
    fn an_unreadable_sends_list_is_an_error_and_not_an_empty_list() {
        // An empty list means "you have no Sends", and a screen that says so
        // when it simply could not read them is the "could not check must
        // never render as success" rule again.
        for (why, stdout) in [
            ("not JSON", "no sends"),
            ("an object rather than a list", r#"{"id":"x"}"#),
            ("a row with no id", r#"[{"accessUrl":"https://x/#a/b"}]"#),
            ("a row with no link", r#"[{"id":"x"}]"#),
        ] {
            assert!(
                parse_send_list(stdout).is_err(),
                "{why}: read back as {:?}",
                parse_send_list(stdout)
            );
        }
        assert_eq!(parse_send_list("[]").expect("an empty list is a list"), vec![]);
    }

    // -- validation, the clock and the wording -----------------------------

    #[test]
    fn a_plan_is_refused_for_each_reason_it_can_be_refused_for() {
        assert_eq!(validate_plan(&plan()), None, "control: the good plan is refused");
        let cases = [
            SendPlan { name: "  ".to_string(), ..plan() },
            SendPlan { name: "n".repeat(MAX_NAME_LEN + 1), ..plan() },
            SendPlan { text: Zeroizing::new(String::new()), ..plan() },
            SendPlan {
                text: Zeroizing::new("x".repeat(MAX_TEXT_LEN + 1)),
                ..plan()
            },
            SendPlan { delete_in_days: 3, ..plan() },
            SendPlan { delete_in_days: 0, ..plan() },
            SendPlan { password: Some(Zeroizing::new(String::new())), ..plan() },
            SendPlan { max_access_count: Some(0), ..plan() },
        ];
        let mut ran = 0;
        for case in &cases {
            let problem = validate_plan(case)
                .unwrap_or_else(|| panic!("this plan was accepted: {:?}", case.name));
            assert!(problem.len() > 10, "the refusal says nothing useful: {problem:?}");
            assert!(
                plan_to_invocation(case, "sess", &NOW).is_err(),
                "a plan validation refuses was still turned into an invocation"
            );
            ran += 1;
        }
        assert_eq!(ran, cases.len());
        for days in DELETE_IN_DAYS_CHOICES {
            assert_eq!(validate_plan(&SendPlan { delete_in_days: days, ..plan() }), None);
        }
    }

    #[test]
    fn the_deletion_date_is_the_injected_now_plus_the_chosen_days() {
        // Control: the clock is read at all. A `deletion_date` that ignored
        // its argument would fail here rather than in a month's time.
        assert_eq!(deletion_date(0, &FixedClock(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(deletion_date(1, &NOW), "2026-08-12T00:43:17.148Z");
        assert_eq!(deletion_date(7, &NOW), "2026-08-18T00:43:17.148Z");
        assert_eq!(deletion_date(30, &NOW), "2026-09-10T00:43:17.148Z");
        // A leap day, crossed both ways, because the civil-date arithmetic is
        // hand-rolled and February is where hand-rolled date code goes wrong.
        assert_eq!(
            deletion_date(1, &FixedClock(1_709_078_400_000)),
            "2024-02-29T00:00:00.000Z"
        );
        assert_eq!(
            deletion_date(1, &FixedClock(1_709_164_800_000)),
            "2024-03-01T00:00:00.000Z"
        );
        assert_eq!(deletion_date(1, &FixedClock(4_107_456_000_000)), "2100-03-01T00:00:00.000Z");
    }

    #[test]
    fn the_expiry_wording_names_the_date_and_not_only_the_number_of_days() {
        let seven = expiry_wording(7, &NOW);
        assert!(seven.contains("7 days"), "{seven:?}");
        assert!(
            seven.contains("18 Aug 2026"),
            "the wording gives a duration but not the date it lands on, which is the thing a \
             user can check: {seven:?}"
        );
        let one = expiry_wording(1, &NOW);
        assert!(one.contains("1 day") && !one.contains("1 days"), "{one:?}");
        assert!(one.contains("12 Aug 2026"), "{one:?}");
        assert!(
            expiry_wording(30, &NOW).contains("10 Sep 2026"),
            "{}",
            expiry_wording(30, &NOW)
        );
        // The wording and the JSON must not disagree about the day; two
        // separate formatters over the same instant is exactly how they would.
        for days in DELETE_IN_DAYS_CHOICES {
            let wording = expiry_wording(days, &NOW);
            let iso = deletion_date(days, &NOW);
            let (y, m, d, ..) =
                utc_parts(NOW.now_unix_millis() + i64::from(days) * MILLIS_PER_DAY);
            assert!(iso.starts_with(&format!("{y:04}-{m:02}-{d:02}")), "{iso:?}");
            assert!(
                wording.contains(&format!("{d} {} {y}", MONTH_NAMES[(m - 1) as usize])),
                "the wording and the deletion date disagree: {wording:?} vs {iso:?}"
            );
        }
    }

    // -- 7. the Zeroizing pin ----------------------------------------------

    /// The crate's `#[global_allocator]` probe, applied to the three buffers
    /// that hold the secret on this path: the plan's `text`, the plan's
    /// `password`, and the JSON body [`plan_to_invocation`] builds out of
    /// them. The last is the one a reviewer would not think of: `text` may be
    /// `Zeroizing` and still be copied verbatim into an ordinary `String` one
    /// line later.
    ///
    /// **The control is asserted first**, as every probe test in this crate
    /// does: an instrument that answers `false` to everything would make the
    /// real assertion below vacuous, and there would be nothing in the output
    /// to say so.
    #[test]
    fn the_plans_secret_fields_and_the_built_json_all_wipe() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // Built before the watch is armed, so the temporaries of building it
        // are not what is measured.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: the allocator probe did not see a plain `String` carrying the probe go \
             back to the allocator, so every verdict below is meaningless"
        );

        // 1. The plan's own fields.
        let text = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let password = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let held = SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new(text),
            password: Some(Zeroizing::new(password)),
            ..SendPlan::default()
        };
        assert!(
            !plaintext_reached_the_allocator(move || drop(held)),
            "a dropped `SendPlan` handed the secret it was carrying back to the allocator in \
             the clear"
        );

        // 2. And the body built out of them. This is the buffer that did not
        //    exist before this module: `plan_to_invocation` reads two
        //    `Zeroizing` fields and writes a third string containing both.
        let text = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let password = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let source = SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new(text),
            password: Some(Zeroizing::new(password)),
            ..SendPlan::default()
        };
        assert!(
            !plaintext_reached_the_allocator(move || {
                let inv = plan_to_invocation(&source, "sess", &NOW).expect("valid");
                drop(inv);
                drop(source);
            }),
            "the JSON body `plan_to_invocation` builds went back to the allocator with the \
             secret still in it. `Zeroizing` on the plan's fields is not enough: the body is \
             a third copy of both of them."
        );
    }
}

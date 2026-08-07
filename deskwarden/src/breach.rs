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
//! `no_part_of_the_hash_beyond_the_prefix_is_ever_sent` is that claim as a
//! test, read off the literal request bytes.
//!
//! # Nothing in this module logs
//!
//! No `log::`, no `println!`, no `dbg!`. The hex is never formatted into any
//! message, and errors carry a `&'static str` category rather than data.
//! `the_breach_module_never_logs` enforces it over this file's source.

use crate::http_agent::TotalBounded;
use sha1::{Digest, Sha1};
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
/// `password` is borrowed -- it is the `Zeroizing<String>` that already lives
/// in `LoginData` (`vault_bridge.rs`) and **no copy of it is made here**;
/// `Sha1::digest` reads the borrow directly.
///
/// The `String` is built at `with_capacity(40)` and exactly 40 characters are
/// pushed into it. That is load-bearing, not tidiness: a `String` that grows
/// reallocs, and `realloc` hands the old block -- holding a partial hash --
/// back to the allocator without wiping it. Pre-reserved capacity means zero
/// reallocs and exactly one wipe, on drop. `the_hex_buffer_never_reallocates`
/// pins the capacity at both ends.
pub(crate) fn hex_digest(password: &str) -> Zeroizing<String> {
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

/// SHA-1 the password and split the uppercase hex into the 5 chars that may
/// leave this machine and the 35 that may not.
///
/// The prefix is a plain `String` on purpose: it is the one part of this that
/// is legitimately published, and dressing it up as a secret would misdescribe
/// what the k-anonymity scheme actually protects. The suffix is
/// `Zeroizing<String>` at exactly its final capacity, for the same reason the
/// hex is.
pub fn split_hash(password: &str) -> (String, Zeroizing<String>) {
    let hex = hex_digest(password);

    let prefix = hex[..PREFIX_LEN].to_string();
    let mut suffix = Zeroizing::new(String::with_capacity(SUFFIX_LEN));
    suffix.push_str(&hex[PREFIX_LEN..]);

    (prefix, suffix)
}

/// Look `suffix` up in a `SUFFIX35:COUNT` range body. Pure.
///
/// * `Ok(Some(count))` -- the suffix is in the range, `count` times.
/// * `Ok(None)` -- the body had well-formed lines and this suffix was not
///   among them.
/// * `Err(Malformed)` -- the body had **no** well-formed line at all, or a
///   line claiming a count of zero.
///
/// The third case is why this returns a `Result` rather than an `Option`. An
/// empty 200, an HTML error page served with a 200, a truncated body: all of
/// them contain no matching suffix, and all of them would read as "not
/// breached" if absence were the only signal. A count of zero is treated the
/// same way -- HIBP never returns one, so a parsed 0 means the line was not
/// what it looked like, and `BreachStatus::Breached(0)` must be
/// unrepresentable rather than merely unlikely.
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
        let Ok(count) = count_text.trim().parse::<u64>() else {
            continue;
        };
        if count == 0 {
            // Shaped like a range line but cannot be one. Refusing the whole
            // body is the conservative reading: something upstream is wrong,
            // and "not breached" is the one answer that must not be guessed.
            return Err(Malformed("zero count"));
        }
        saw_well_formed_line = true;
        if found.is_none() && line_suffix.eq_ignore_ascii_case(suffix) {
            found = Some(count);
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
/// **Only `prefix` is interpolated into the URL.** Everything that could
/// identify the password stays in `suffix`, which is only ever compared
/// against text that came back.
///
/// Every failure -- transport, non-200, unreadable body, unparseable body --
/// collapses to [`BreachStatus::Unavailable`]. None of them collapse to
/// `Safe`; a lookup that did not happen must never paint a green badge.
pub fn check_prefix(
    base_url: &str,
    prefix: &str,
    suffix: &Zeroizing<String>,
    agent: &TotalBounded,
) -> BreachStatus {
    let url = format!("{base_url}/{prefix}");
    let Ok(response) = agent.get(&url).set("User-Agent", USER_AGENT).call() else {
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

    #[test]
    fn a_zero_count_line_is_malformed() {
        // Shaped exactly like a range line in every other respect.
        let body = format!("{PASSWORD_SUFFIX}:0\r\n");
        assert!(
            parse_range_body(&body, PASSWORD_SUFFIX).is_err(),
            "HIBP never returns a count of 0, so Breached(0) must be unrepresentable"
        );
        assert_eq!(status_for(&body), BreachStatus::Unavailable);
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
        assert_eq!(prefix, &PASSWORD_SHA1[..5]);
        assert_eq!(&*suffix, PASSWORD_SUFFIX);
        assert_eq!(prefix.len(), 5);
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
            &format!("{}/range", server.url()),
            "5BAA6",
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Breached(PASSWORD_COUNT));
    }

    #[test]
    fn an_absent_suffix_over_the_wire_reports_safe() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/range/5BAA6")
            .with_status(200)
            .with_body(FIXTURE)
            .create();

        let suffix = Zeroizing::new(ABSENT_SUFFIX.to_string());
        let status = check_prefix(
            &format!("{}/range", server.url()),
            "5BAA6",
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
        check_prefix(&format!("{}/range", server.url()), "5BAA6", &suffix, &build_agent());
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

    /// **The privacy claim, as a test.**
    ///
    /// Read off the literal request head this crate put on a socket -- not off
    /// a matcher, which can only tell you that something you named was
    /// present. The whole head (request line and every header) must contain no
    /// substring of the 35 secret characters longer than a few chars, and the
    /// path must be exactly `/range/{PREFIX}`.
    #[test]
    fn no_part_of_the_hash_beyond_the_prefix_is_ever_sent() {
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
            &format!("http://127.0.0.1:{port}/range"),
            &prefix,
            &suffix,
            &build_agent(),
        );
        assert_eq!(status, BreachStatus::Breached(PASSWORD_COUNT), "the mock did answer");

        let head = server.join().expect("server thread");
        let upper = head.to_ascii_uppercase();

        // Control: the head really was captured and really is a request head,
        // otherwise every "does not contain" below is about an empty string.
        assert!(head.starts_with("GET "), "captured head is not a request head: {head:?}");
        assert!(upper.contains(&prefix), "the prefix should be in the head: {head:?}");

        assert!(
            upper.contains(&format!("GET /RANGE/{prefix} ")),
            "the request line is not exactly /range/{{PREFIX}}: {head:?}"
        );
        assert!(
            !upper.contains(&*suffix.to_ascii_uppercase()),
            "the whole 35-char suffix went on the wire: {head:?}"
        );
        // Nothing beyond the prefix, not merely "not the whole thing": every
        // 8-character window of the suffix must be absent too, which catches a
        // truncated or chunked leak the whole-string check would miss.
        for window in suffix.as_bytes().windows(8) {
            let piece = String::from_utf8(window.to_vec()).unwrap().to_ascii_uppercase();
            assert!(
                !upper.contains(&piece),
                "a fragment of the secret suffix ({piece}) went on the wire: {head:?}"
            );
        }
    }

    #[test]
    fn a_500_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(500).with_body(FIXTURE).create();

        let suffix = Zeroizing::new(ABSENT_SUFFIX.to_string());
        let status =
            check_prefix(&format!("{}/range", server.url()), "5BAA6", &suffix, &build_agent());
        assert_eq!(status, BreachStatus::Unavailable, "a 500 must never paint a green badge");
    }

    #[test]
    fn a_404_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(404).with_body("").create();

        let suffix = Zeroizing::new(ABSENT_SUFFIX.to_string());
        let status =
            check_prefix(&format!("{}/range", server.url()), "5BAA6", &suffix, &build_agent());
        assert_eq!(status, BreachStatus::Unavailable);
    }

    #[test]
    fn a_200_with_an_empty_body_reports_unavailable_not_safe() {
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/range/5BAA6").with_status(200).with_body("").create();

        let suffix = Zeroizing::new(ABSENT_SUFFIX.to_string());
        let status =
            check_prefix(&format!("{}/range", server.url()), "5BAA6", &suffix, &build_agent());
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
        let suffix = Zeroizing::new(ABSENT_SUFFIX.to_string());
        let status = check_prefix(
            &format!("http://127.0.0.1:{port}/range"),
            "5BAA6",
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
        assert_eq!(prefix.len(), 5);
        assert_eq!(suffix.len(), 35);
        assert!(!prefix.contains("des"), "control: the prefix is a hash, not the password");
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
    /// removed something, really kept the code, and really cut the tests off.
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

    /// Nothing in this module logs, prints, or debug-dumps.
    ///
    /// Every needle is positively controlled. The five that occur in live code
    /// are asserted to occur in some *other* file, so a mis-spelled needle
    /// fails here instead of passing vacuously. `dbg!` occurs nowhere in this
    /// crate, so its control is `stringify!` over a real `dbg!` invocation --
    /// the compiler's own spelling of the macro, not a second guess at it.
    #[test]
    fn the_breach_module_never_logs() {
        const LIVE: [&str; 6] = [
            concat!("log:", ":debug!"),
            concat!("log:", ":info!"),
            concat!("log:", ":warn!"),
            concat!("log:", ":error!"),
            concat!("print", "ln!"),
            concat!("eprint", "ln!"),
        ];
        const DBG: &str = concat!("db", "g!");

        let files = crate_source_files();
        assert!(files.len() > 20, "the walk found only {} files; src/ has far more", files.len());

        for needle in LIVE {
            let elsewhere = files
                .iter()
                .filter(|(path, _)| path != "breach.rs")
                .any(|(_, text)| text.contains(needle));
            assert!(
                elsewhere,
                "needle {needle:?} matches nothing in this crate, so asserting its absence from \
                 breach.rs proves nothing"
            );
        }
        assert!(
            stringify!(dbg!(1)).contains(DBG),
            "control: the {DBG:?} needle does not match the compiler's own spelling of the macro"
        );

        let source = this_module_production_code();
        assert_strip_worked(&source);
        let found: Vec<&str> = LIVE
            .into_iter()
            .chain(std::iter::once(DBG))
            .filter(|needle| source.contains(needle))
            .collect();
        assert!(
            found.is_empty(),
            "breach.rs logs or prints: {found:?}. A log line here is the password's hash in a \
             file on disk, and there is no such thing as a safe one"
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

    /// `BreachStatus` must not gain a `Default`. `unwrap_or_default()` on a
    /// missing status would silently turn "we could not check" into a badge,
    /// and that is the one failure mode this feature cannot afford.
    ///
    /// Source-pinned because the type system cannot state the absence of an
    /// impl. Positively controlled against a file that does derive `Default`,
    /// so the needle spellings are known to match real code.
    #[test]
    fn breach_status_has_no_default() {
        const DERIVE: &str = concat!("Def", "ault");
        let source = this_module_source();

        let elsewhere = crate_source_files()
            .into_iter()
            .filter(|(path, _)| path != "breach.rs")
            .any(|(_, text)| text.contains(DERIVE));
        assert!(elsewhere, "needle {DERIVE:?} matches nothing in this crate");

        // The doc comment on `BreachStatus` spells the word out, so count
        // occurrences in code rather than asserting outright absence: the
        // enum's own derive list must not contain it.
        let lines: Vec<&str> = source.lines().collect();
        let decl = lines
            .iter()
            .position(|l| l.contains("pub enum BreachStatus"))
            .expect("BreachStatus is declared in this file");
        assert!(decl > 0, "control: the declaration is not the first line of the file");
        let derive_line = lines[decl - 1].to_string();
        assert!(
            derive_line.contains("derive"),
            "control: the line above `pub enum BreachStatus` is not its derive list: \
             {derive_line:?}"
        );
        assert!(
            !derive_line.contains(DERIVE),
            "BreachStatus derives Default: {derive_line:?}"
        );

        // The derive is only one of the two spellings. A hand-written `impl
        // Default for BreachStatus` produces exactly the same
        // `unwrap_or_default()` hazard and the line-above check cannot see it,
        // so forbid it in the production code too.
        const HAND_WRITTEN: &str = concat!("impl Def", "ault for");
        let production = this_module_production_code();
        assert_strip_worked(&production);
        let elsewhere_impl = crate_source_files()
            .into_iter()
            .filter(|(path, _)| path != "breach.rs")
            .any(|(_, text)| text.contains(HAND_WRITTEN));
        assert!(
            elsewhere_impl,
            "needle {HAND_WRITTEN:?} matches nothing in this crate, so asserting its absence \
             here proves nothing"
        );
        assert!(
            !production.contains(HAND_WRITTEN),
            "breach.rs hand-writes a Default impl -- same hazard as the derive, different spelling"
        );
    }
}

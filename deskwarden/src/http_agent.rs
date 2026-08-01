//! The two shapes of HTTP bound this app needs, as two constructors that
//! cannot be confused for each other.
//!
//! # Why this module exists
//!
//! This app makes two genuinely different kinds of HTTP request:
//!
//! * **Small responses that must come back promptly** -- every `bw serve`
//!   vault call (on the UI thread), the GitHub releases check, a favicon.
//!   What matters is *total* elapsed time: past some point the answer is
//!   worthless however it arrives.
//! * **One multi-megabyte streamed download** -- the installer. What matters
//!   is *progress*: a legitimately slow link may take minutes and must not be
//!   aborted, but a transfer that stops moving is dead and must be.
//!
//! ureq 2.12.1 can express exactly one of these per agent, and the two
//! settings that express them are mutually exclusive in a way its API does not
//! advertise:
//!
//! * `AgentBuilder::timeout()` sets `unit.deadline`, a whole-request deadline
//!   covering the body (`request.rs:122`, `response.rs:574`). It is the *only*
//!   bound that survives connection reuse.
//! * `AgentBuilder::timeout_read()` is applied to the socket only on the
//!   connect path, and only in the `else` arm of `stream.rs:433-437` --
//!   **once `timeout` is set, `unit.deadline` is always `Some` and
//!   `timeout_read` is never applied at all.** `DeadlineStream::fill_buf`
//!   (`stream.rs:85-90`) re-stamps the remaining whole-request deadline before
//!   every body read, so it stays inert for the entire transfer.
//! * Worse, without `timeout` the read timeout does not survive pooling
//!   either: `Stream::reset()` clears it (`set_read_timeout(None)`,
//!   `stream.rs:265`) on the way into the keep-alive pool, and
//!   `connect_socket` returns a pooled connection early (`unit.rs:361-364`)
//!   without re-entering the connect path. That gap shipped in v0.3.0 as a
//!   hard UI hang: the eframe thread parked in `recv` on the `bw serve`
//!   socket, byte-identical stack minutes apart, while `bw serve` answered
//!   fresh connections fine.
//!
//! So "set both and hope" -- which is what this crate did in v0.3.0 and again
//! in the first fix for it -- always leaves one of the two knobs doing
//! nothing, with a doc comment claiming otherwise. Each constructor below sets
//! only the knob that is actually live for its shape, and the dead one is not
//! set at all.

use std::time::Duration;

/// An agent whose requests are bounded by **total elapsed time**.
///
/// For small responses where a late answer is a useless answer. `total` caps
/// the whole request including the body, which is the point: these callers
/// have nothing to stream.
///
/// This is the shape that survives connection reuse, so it is what closes the
/// v0.3.0 pooled-connection hang. Pooling stays enabled -- reuse is a real win
/// for the vault path's once-per-second poll and for icon bursts, and the
/// deadline covers pooled and fresh connections alike.
///
/// `timeout_read` is deliberately **not** set: with `timeout` in force ureq
/// never applies it (see the module docs), so setting it would only produce a
/// knob that reads as protection and is not.
pub fn bounded_total(connect: Duration, total: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout(total)
        .build()
}

/// An agent whose requests are bounded by **time without progress**.
///
/// For streamed transfers where the legitimate duration is unknown and a total
/// cap is the wrong shape: tight enough to catch a stall it would abort a slow
/// download, loose enough not to abort a slow download it is not a bound
/// anyone benefits from. `stall` is the longest gap allowed between successive
/// reads; a slow-but-steady transfer runs as long as it needs.
///
/// `max_idle_connections(0)` is load-bearing, not tuning. It makes
/// `Pool::noop()` true (`pool.rs:92`), so `Pool::add` returns without storing
/// anything (`pool.rs:126-129`), so every request takes the connect path and
/// `timeout_read` is actually applied to the socket. Without it this agent
/// would inherit exactly the v0.3.0 hang -- a reused connection with its read
/// timeout cleared and no deadline to fall back on. Here the hang is
/// impossible by construction rather than bounded after the fact, and pooling
/// buys a one-shot download nothing anyway.
///
/// `timeout` is deliberately **not** set: setting it is what would make
/// `timeout_read` inert, which is the regression this shape exists to undo.
pub fn bounded_stall(connect: Duration, stall: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout_read(stall)
        .max_idle_connections(0)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    /// Reads exactly one request head off `stream`, so the next one can be
    /// read separately -- several tests below turn on two requests arriving
    /// over one socket. Returns false at EOF.
    fn read_head(stream: &mut TcpStream) -> bool {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read(&mut byte).unwrap_or(0) == 1 {
            seen.push(byte[0]);
            if seen.ends_with(b"\r\n\r\n") {
                return true;
            }
        }
        false
    }

    fn write_json_response(stream: &mut TcpStream, body: &str) {
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        let _ = stream.flush();
    }

    /// Regression test for the v0.3.0 UI hang, at the constructor every
    /// total-bounded caller in this crate goes through.
    ///
    /// The trap this deliberately avoids: stalling the *first* request proves
    /// nothing, because a fresh connection does get `timeout_read` applied on
    /// ureq's connect path. The exposure is only on a **reused** connection,
    /// whose read timeout `Stream::reset()` cleared on its way into the
    /// keep-alive pool. So the server answers one request normally (pooling
    /// the socket), then reads the second request off that same socket and
    /// never answers.
    #[test]
    fn a_total_bounded_agent_bounds_a_pooled_connection_that_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut stream);
            // Keep-alive by default in HTTP/1.1, and a Content-Length so ureq
            // knows the body ended and can return the socket to the pool.
            write_json_response(&mut stream, r#"{"ok":true}"#);
            // Second request on the *same* socket: accept it, answer nothing.
            read_head(&mut stream);
            std::thread::sleep(Duration::from_secs(15));
        });

        let agent = bounded_total(Duration::from_secs(1), Duration::from_secs(2));
        let url = format!("http://127.0.0.1:{port}/");

        agent
            .get(&url)
            .call()
            .expect("first request should succeed")
            .into_string()
            .expect("first body should read, so the socket is pooled");

        let started = Instant::now();
        let result = agent.get(&url).call();
        let elapsed = started.elapsed();

        assert!(result.is_err(), "a server that never answers must not look like success");
        // Comfortably above the 2s deadline so a loaded machine can't flake
        // this, and far below the server thread's 15s sleep so an unbounded
        // read cannot possibly sneak under it.
        assert!(
            elapsed < Duration::from_secs(8),
            "second (pooled) request was not bounded by the total deadline: took {elapsed:?}"
        );
    }

    /// The case the first fix for the hang silently broke: headers arrive,
    /// some body arrives, then the transfer stops moving. A *total* bound
    /// generous enough for a real download (600s) cannot detect this at all,
    /// which is why the stall shape exists.
    ///
    /// Note this stalls the connection's *first and only* request on purpose:
    /// unlike the pooled test above, the property under test is that
    /// `timeout_read` is in force at all -- and it is not, on any request, if
    /// the agent also carries a whole-request deadline.
    #[test]
    fn a_stall_bounded_agent_aborts_a_transfer_that_stops_mid_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_head(&mut stream);
            // Promise 1000 bytes, send 10, then go silent while holding the
            // socket open -- a stalled transfer, not a closed one.
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n0123456789");
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(10));
        });

        let agent = bounded_stall(Duration::from_secs(1), Duration::from_secs(1));
        let response = agent
            .get(&format!("http://127.0.0.1:{port}/"))
            .call()
            .expect("headers arrive promptly; only the body stalls");

        let started = Instant::now();
        let mut body = Vec::new();
        let result = response.into_reader().read_to_end(&mut body);
        let elapsed = started.elapsed();

        assert!(result.is_err(), "a transfer that stopped moving must not look like success");
        // Well above the 1s stall bound, well below the server's 10s sleep --
        // the only thing that can return inside this window is the read
        // timeout firing.
        assert!(
            elapsed < Duration::from_secs(5),
            "stalled body read was not bounded by the stall timeout: took {elapsed:?}"
        );
    }

    /// Pins `max_idle_connections(0)`, without which the stall shape is a lie:
    /// a reused connection has had its read timeout cleared by
    /// `Stream::reset()`, and this agent has no whole-request deadline to fall
    /// back on, so the second request would be exactly the v0.3.0 hang.
    ///
    /// Asserted by counting accepts rather than by timing, so it cannot flake:
    /// the client makes two requests, and each must arrive on its own
    /// connection. With pooling left enabled the second request rides the
    /// first socket and the count stays at 1.
    #[test]
    fn a_stall_bounded_agent_never_reuses_a_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepts = Arc::new(AtomicUsize::new(0));

        let counted = Arc::clone(&accepts);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counted.fetch_add(1, Ordering::SeqCst);
                // Serve every request that arrives on this connection, so a
                // pooled second request would be answered too -- the test must
                // distinguish the two cases by connection count, not by one of
                // them failing.
                std::thread::spawn(move || {
                    while read_head(&mut stream) {
                        write_json_response(&mut stream, r#"{"ok":true}"#);
                    }
                });
            }
        });

        let agent = bounded_stall(Duration::from_secs(2), Duration::from_secs(2));
        let url = format!("http://127.0.0.1:{port}/");
        for _ in 0..2 {
            agent.get(&url).call().unwrap().into_string().unwrap();
        }

        // The counter is incremented before the response is written, so by the
        // time the second call has returned a body its connection is counted.
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            2,
            "the second request reused a connection: pooling is not disabled, \
             so its read timeout was cleared and nothing bounds it"
        );
    }
}

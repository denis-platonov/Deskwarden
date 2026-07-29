//! Lifecycle management for the bundled `bw serve` HTTP bridge.
//!
//! Covers spawning it, detecting a pre-existing process squatting on its port,
//! waiting for it to actually become ready (its cold start is a Node process
//! and routinely takes multiple seconds), and running `bw sync`.

use crate::vault_bridge::{VaultBridge, VaultItem};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Port `bw serve` is started on, and that [`BW_SERVE_URL`] points at.
pub const BW_SERVE_PORT: u16 = 8087;

/// Base URL of the local `bw serve` bridge.
pub const BW_SERVE_URL: &str = "http://localhost:8087";

/// Builds the retry schedule for waiting on `bw serve` readiness: the delays
/// to sleep *between* attempts, exponentially backing off from 250ms and
/// capped at 4s per wait, stopping once the accumulated wait would exceed
/// `deadline`.
///
/// Pure and separated from the polling loop so the shape of the backoff is
/// testable without actually sleeping for half a minute.
pub fn readiness_schedule(deadline: Duration) -> Vec<Duration> {
    const FIRST_DELAY_MS: u64 = 250;
    const MAX_DELAY_MS: u64 = 4_000;

    let mut schedule = Vec::new();
    let mut elapsed = Duration::ZERO;
    let mut delay = Duration::from_millis(FIRST_DELAY_MS);

    while elapsed + delay <= deadline {
        schedule.push(delay);
        elapsed += delay;
        delay = std::cmp::min(delay * 2, Duration::from_millis(MAX_DELAY_MS));
    }

    schedule
}

/// Default deadline for `bw serve` to come up and answer a real vault query.
pub const READINESS_DEADLINE: Duration = Duration::from_secs(30);

/// Returns true if something is already listening on `port` on localhost.
///
/// Used to detect an orphaned `bw serve` from a previous, unclean exit: if one
/// is still holding the port, our freshly spawned `bw serve` will fail to bind
/// and `VaultBridge` would end up talking to an unknown process holding an
/// unknown session.
pub fn port_in_use(port: u16) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    TcpStream::connect_timeout(&addr.into(), Duration::from_millis(300)).is_ok()
}

/// Spawns `bw serve` bound to [`BW_SERVE_PORT`] with `BW_SESSION` set.
///
/// Returns the error rather than panicking so the caller can log a diagnosable
/// message (the usual cause is the Bitwarden CLI not being on `PATH`).
pub fn spawn_bw_serve(session_token: &str) -> std::io::Result<Child> {
    Command::new("bw")
        .args(["serve", "--port", &BW_SERVE_PORT.to_string()])
        .env("BW_SESSION", session_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Polls `vault.list_items()` until it succeeds or the schedule is exhausted.
///
/// `list_items` is deliberately the probe rather than a lighter ping: it is the
/// operation the app actually depends on, so a success here proves both that
/// the HTTP server is up *and* that the session it was handed is valid.
///
/// Returns the items on success (the caller needs them anyway to build the
/// match engine), or a human-readable error describing the last failure.
pub fn wait_for_vault_ready(
    vault: &VaultBridge,
    schedule: &[Duration],
) -> Result<Vec<VaultItem>, String> {
    let mut attempt = 0usize;

    loop {
        match vault.list_items() {
            Ok(items) => {
                log::info!(
                    "bw serve ready after {attempt} retr{} ({} vault items)",
                    if attempt == 1 { "y" } else { "ies" },
                    items.len()
                );
                return Ok(items);
            }
            Err(e) => {
                let last_error = format!("{e:?}");
                if attempt >= schedule.len() {
                    return Err(format!(
                        "bw serve did not become ready within the deadline; last error: {last_error}"
                    ));
                }
                log::debug!(
                    "bw serve not ready yet (attempt {}): {last_error}",
                    attempt + 1
                );
                std::thread::sleep(schedule[attempt]);
                attempt += 1;
            }
        }
    }
}

/// Runs `bw sync` with the given session so the local vault cache reflects the
/// latest server state before the match engine is built.
///
/// Failure is non-fatal (we can still work from the cached vault), so this
/// returns a `Result` for the caller to log rather than propagating.
pub fn run_bw_sync(session_token: &str) -> Result<(), String> {
    let output = Command::new("bw")
        .arg("sync")
        .env("BW_SESSION", session_token)
        .output()
        .map_err(|e| format!("failed to run `bw sync`: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_schedule_starts_short_and_backs_off() {
        let schedule = readiness_schedule(Duration::from_secs(30));
        assert_eq!(schedule[0], Duration::from_millis(250));
        assert_eq!(schedule[1], Duration::from_millis(500));
        assert_eq!(schedule[2], Duration::from_secs(1));
        assert_eq!(schedule[3], Duration::from_secs(2));
        assert_eq!(schedule[4], Duration::from_secs(4));
    }

    #[test]
    fn readiness_schedule_caps_individual_delays() {
        let schedule = readiness_schedule(Duration::from_secs(120));
        assert!(schedule
            .iter()
            .all(|d| *d <= Duration::from_millis(4_000)));
    }

    #[test]
    fn readiness_schedule_never_exceeds_the_deadline() {
        for secs in [1u64, 5, 15, 30, 60] {
            let deadline = Duration::from_secs(secs);
            let total: Duration = readiness_schedule(deadline).iter().sum();
            assert!(
                total <= deadline,
                "schedule for {secs}s totalled {total:?}"
            );
        }
    }

    #[test]
    fn readiness_schedule_gives_more_attempts_for_a_longer_deadline() {
        let short = readiness_schedule(Duration::from_secs(1)).len();
        let long = readiness_schedule(Duration::from_secs(30)).len();
        assert!(long > short, "short={short} long={long}");
    }

    #[test]
    fn readiness_schedule_is_empty_when_there_is_no_time_to_retry() {
        assert!(readiness_schedule(Duration::from_millis(100)).is_empty());
    }

    #[test]
    fn port_in_use_is_false_for_a_closed_port() {
        // Bind and immediately drop a listener to obtain a port that was just
        // free; nothing should be listening on it afterwards.
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!port_in_use(port));
    }

    #[test]
    fn port_in_use_is_true_for_an_open_port() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_in_use(port));
    }

    #[test]
    fn wait_for_vault_ready_returns_items_when_the_server_answers() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[{"id":"1","name":"A","fields":[]}]}}"#)
            .create();

        let vault = VaultBridge::new(server.url());
        let items = wait_for_vault_ready(&vault, &[]).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn wait_for_vault_ready_reports_an_error_instead_of_an_empty_vault() {
        // Nothing listening: every attempt fails. With an empty schedule this
        // gives up immediately rather than sleeping, and must surface an error
        // rather than the empty item list that used to be silently swallowed
        // by `unwrap_or_default()`.
        let vault = VaultBridge::new("http://127.0.0.1:1");
        let err = wait_for_vault_ready(&vault, &[]).unwrap_err();
        assert!(err.contains("did not become ready"), "got: {err}");
    }
}

//! The socket, and the small number of decisions that come with owning one.
//!
//! Everything about *who may call what* is [`crate::service_api`], and
//! everything about *what comes back* is [`crate::service_body`]. What is
//! left here is the part that cannot be a pure function -- binding a port and
//! reading requests off it -- kept deliberately thin so that the untestable
//! surface is small enough to read in one go.
//!
//! # Loopback, and not by default
//!
//! [`listen_addr`] returns `127.0.0.1` and there is no parameter that could
//! make it anything else. A configurable bind address would be one settings
//! mistake away from putting a decrypted vault on the network, and nobody
//! needs it: the three consumers are all on this machine by construction.
//!
//! # Two lifetimes, one mechanism
//!
//! `docs/superpowers/specs/2026-08-27-the-local-vault-service-design.md`
//! asks for a service that can run 24/7 or only while an app needs it.
//! [`Mode::Installed`] holds a [`crate::vault_service`] attachment slot for
//! as long as it is installed, and that is the whole difference --
//! `anyone_attached` and `supervise` never learn which mode they are in. A
//! second lifetime scheme would be a second set of exit conditions, and the
//! exit condition is the part that has already proved delicate.

use crate::service_api::Answer;

/// Where the service listens. Loopback, always.
///
/// Port 0 means "any free port", which is the sensible default for a service
/// whose clients are told the port rather than guessing it. A fixed port is
/// available for a script that wants to hard-code one.
#[must_use]
pub fn listen_addr(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], port))
}

/// Whether this service is installed or is running for as long as an app
/// needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Started at logon and expected to outlive every app. Holds one
    /// attachment slot of its own so that "is anybody using the vault?"
    /// answers yes for as long as it is installed.
    Installed,
    /// Started by an app, and exits when the last one lets go. Holds no slot
    /// of its own -- the apps hold theirs, and that is the count.
    ConsumerDriven,
}

/// How many attachment slots this service holds on its own behalf.
///
/// The entire difference between the two lifetimes, expressed as a number so
/// that it is one value rather than a branch in three places.
#[must_use]
pub fn slots_to_hold(mode: Mode) -> usize {
    match mode {
        Mode::Installed => 1,
        Mode::ConsumerDriven => 0,
    }
}

/// The HTTP status for one decided request.
///
/// `401` and `403` are kept apart for the reason
/// [`crate::service_api::Answer::Forbidden`] exists: a legitimate script has
/// to be able to tell "your credential is wrong" from "your credential is
/// right and does not cover this".
#[must_use]
pub fn status_code_for(answer: &Answer) -> u16 {
    match answer {
        Answer::Ok { .. } => 200,
        // The auth exchange has not happened yet at the point this is asked;
        // whether it succeeds is decided by the handler, which answers 200
        // with a session or 401 without one.
        Answer::Authenticate => 200,
        Answer::Unauthenticated => 401,
        Answer::Forbidden => 403,
        Answer::NotFound => 404,
        Answer::MethodNotAllowed => 405,
    }
}

/// The answer to a failed master-password attempt.
///
/// One shape for every failure. A service that said "no such account" for one
/// input and "wrong password" for another would answer a question nobody
/// authenticated has any business asking.
#[must_use]
pub fn failed_auth_status() -> u16 {
    401
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_api::Route;

    /// **Binding anything but loopback would put a decrypted vault on the
    /// network.** There is no parameter that could do it, and this is the
    /// test that says so.
    #[test]
    fn the_listen_address_is_always_loopback() {
        for port in [0u16, 80, 8087, 65535] {
            let addr = listen_addr(port);
            assert!(addr.ip().is_loopback(), "{addr} is not a loopback address");
            assert_eq!(addr.ip().to_string(), "127.0.0.1");
            assert_eq!(addr.port(), port);
        }
    }

    /// 24/7 and consumer-driven differ by one held slot and nothing else.
    #[test]
    fn installed_mode_is_one_permanent_attachment() {
        assert_eq!(slots_to_hold(Mode::Installed), 1);
        assert_eq!(slots_to_hold(Mode::ConsumerDriven), 0);
    }

    /// **A refused credential and a refused scope are different answers.**
    /// Collapsing them would leave the owner debugging a working key that
    /// looks broken.
    #[test]
    fn a_wrong_credential_and_an_insufficient_one_are_told_apart() {
        assert_eq!(status_code_for(&Answer::Unauthenticated), 401);
        assert_eq!(status_code_for(&Answer::Forbidden), 403);
    }

    /// Every answer has a status, and the mapping is asserted rather than
    /// left to be read off the match.
    #[test]
    fn every_answer_maps_to_the_status_it_should() {
        assert_eq!(status_code_for(&Answer::Ok { route: Route::Status, key: 0 }), 200);
        assert_eq!(status_code_for(&Answer::Authenticate), 200);
        assert_eq!(status_code_for(&Answer::NotFound), 404);
        assert_eq!(status_code_for(&Answer::MethodNotAllowed), 405);
    }

    /// A failed sign-in says only that it failed. "No such account" and
    /// "wrong password" are the same answer, because the difference is a
    /// fact about the owner told to somebody who has not authenticated.
    #[test]
    fn a_failed_sign_in_does_not_say_why() {
        assert_eq!(failed_auth_status(), status_code_for(&Answer::Unauthenticated));
    }

    /// The bind address must not become configurable by accident. An absence
    /// cannot be read, so it is pinned.
    #[test]
    fn nothing_here_can_bind_a_non_loopback_address() {
        let source = include_str!("service_host.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        assert!(
            production.contains("([127, 0, 0, 1], port)"),
            "control: the loopback literal is gone, so this pin guards nothing"
        );
        for forbidden in ["0.0.0.0", "[::]", "UNSPECIFIED", "bind_address", "listen_host"] {
            assert!(
                !production.contains(forbidden),
                "`{forbidden}` appears here. This service serves a decrypted vault; the bind address is not a setting."
            );
        }
    }
}

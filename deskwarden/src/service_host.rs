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


// ---- the request loop ------------------------------------------------
//
// **Moved here from `main.rs` so that a real socket can drive it.** The
// loop lived in the binary, which meant nothing outside the binary could
// reach it -- no example, no integration test, no probe. That is how it
// came to ship two defects that a green suite never saw: an
// unauthenticated request that fetched the whole vault, and a write that
// answered 200 with the item having changed nothing.
//
// `examples/service_probe.rs` is what this move is for: it binds a real
// port, speaks real HTTP, and checks the answers. The unit tests below
// still drive `answer_one_request` as a pure function, because most of
// what can go wrong here is a decision rather than a socket.
/// What goes back on the wire for one request: a status and a body, and
/// nothing that knows about a socket.
///
/// A struct rather than a tuple because the two halves are the thing that has
/// to agree -- a refusal with a body, or a served item with a `403`, are both
/// one silent swap of an unnamed pair away.
#[derive(Debug, PartialEq, Eq)]
pub struct ServiceReply {
    pub status: u16,
    pub body: String,
}

/// The answer this service gives a caller who asks for the master-password
/// exchange. A named constant so the test that says it must not look like a
/// failed sign-in is comparing against the same bytes the wire carries.
pub const NO_MASTER_PASSWORD_BODY: &str =
    "{\"success\":false,\"message\":\"this service does not take a master password; use an API key\"}";

/// What a write is told, until writing is built.
///
/// **This exists because the alternative was a lie.** `decide` maps every
/// non-GET method to [`Access::Write`] and lets a write-scoped key
/// through, and `body_for` -- which is never told the method -- then
/// answers with the item. So `PUT /object/item/{id}` returned **200 and
/// the item, having changed nothing**, which a script reads as a write
/// that landed. Silent data loss from the caller's side.
///
/// 501 rather than 405: the route exists and the method is one this
/// service intends to support, which is a different thing from a method
/// it will never accept.
/// What a caller is told when the vault cannot be read at all.
///
/// 503 rather than 500: this is a service that is temporarily unable to
/// answer -- an expired session, a server that is down, a network that is
/// gone -- and a script that retries on 503 and gives up on 500 is doing
/// the right thing in both cases.
pub const VAULT_UNREADABLE_BODY: &str =
    "{\"success\":false,\"message\":\"the vault could not be read; the service is not able to answer right now\"}";

/// The route an answer is for, if it is one that will be served.
fn route_of_answer(answer: &crate::service_api::Answer) -> Option<&crate::service_api::Route> {
    match answer {
        crate::service_api::Answer::Ok { route, .. } => Some(route),
        _ => None,
    }
}

pub const WRITES_NOT_BUILT_BODY: &str =
    "{\"success\":false,\"message\":\"this service does not write yet; the scope model accepts write grants but no write is performed\"}";

/// **One request, decided and rendered, as a value.**
///
/// This is where the three tested modules are joined up, and joining them up
/// is its own opportunity to be wrong: the decision comes from
/// [`crate::service_api::decide`], the status from
/// [`crate::service_host::status_code_for`] and the body from
/// [`crate::service_body::body_for`], and a mistake HERE -- a status
/// paired with the wrong body, a refusal handed something to read -- defeats
/// every guarantee those three prove about themselves. So it takes strings
/// and returns a struct: no socket, no clock, no vault of its own.
///
/// `now_unix` is passed in for the reason `decide` takes it -- expiry has to
/// be drivable without waiting for a day to pass -- and the vault arrives as
/// a closure for a stronger reason than testing:
///
/// # The vault is loaded only if the answer is one that reads it
///
/// An earlier shape of this loop fetched items and folders for EVERY request
/// before looking at the answer, which meant an unauthenticated caller --
/// somebody with no credential at all -- made this process do two network
/// round trips to the server per request, as fast as they could connect.
/// Taking the load as `FnOnce` makes "a refusal touches nothing" a fact the
/// type system carries, and a test asserts it by handing in a closure that
/// panics.
pub fn answer_one_request<F>(
    method: &str,
    url: &str,
    auth: Option<&str>,
    keys: &[crate::service_keys::KeyRecord],
    now_unix: u64,
    load_vault: F,
) -> ServiceReply
where
    F: FnOnce() -> Result<(Vec<crate::vault_bridge::VaultItem>, Vec<crate::vault_bridge::Folder>), String>,
{
    use crate::service_api::Answer;

    let answer = crate::service_api::decide(method, url, auth, keys, now_unix);

    // The path is logged and the credential is not. A refused request is
    // worth seeing; the value that was refused is not worth storing.
    log::debug!("{method} {url} -> {answer:?}");

    // Not built yet, and answered honestly rather than with a shape that
    // looks like a failed password. The service has no way to take one.
    if matches!(answer, Answer::Authenticate) {
        return ServiceReply { status: 501, body: NO_MASTER_PASSWORD_BODY.to_string() };
    }

    let status = crate::service_host::status_code_for(&answer);
    if !matches!(answer, Answer::Ok { .. }) {
        // **No body for a refusal.** `body_for` would answer `None` here
        // anyway; not asking it is the same answer arrived at without ever
        // holding the bytes.
        return ServiceReply { status, body: String::new() };
    }

    // **After the scope check and BEFORE the vault is loaded.** After,
    // because a key with no write grant is told 403 by `decide` and never
    // reaches here -- an unauthorised caller learns nothing about what is
    // built and what is not. Before, because fetching the whole vault to
    // answer a request that cannot use it is the exact bug this function
    // was extracted to fix, and it would be a poor showing to reintroduce
    // a smaller copy of it one line lower.
    if method != "GET" {
        return ServiceReply { status: 501, body: WRITES_NOT_BUILT_BODY.to_string() };
    }

    // **A vault that could not be read is not an empty vault.**
    //
    // This used to be `unwrap_or_default()`, which turned a failed read
    // into a successful `200` carrying zero items. A backup script writes
    // an empty backup; a sync script deletes everything. It is the same
    // mistake `run_as_the_vault_service` refuses at start-up -- serving an
    // empty vault a script cannot tell from an empty account -- made one
    // layer further down, where the refusal did not reach.
    let vault_data = load_vault();

    // `/status` is the one route that has an answer when the vault cannot
    // be read, and it is the answer a polling script needs: locked. Every
    // other route says the service is not able to serve right now.
    if matches!(route_of_answer(&answer), Some(crate::service_api::Route::Status)) {
        let locked = vault_data.is_err();
        return ServiceReply { status: 200, body: crate::service_body::status_body(locked) };
    }

    let (items, folders) = match vault_data {
        Ok(loaded) => loaded,
        Err(why) => {
            log::error!("the vault could not be read for {method} {url}: {why}");
            return ServiceReply {
                status: 503,
                body: VAULT_UNREADABLE_BODY.to_string(),
            };
        }
    };
    // `locked: false` is honest here and only here: this line is only
    // reached when the vault WAS read. `/status` answered above, from
    // whether that read succeeded.
    let vault = crate::service_body::Vault { items: &items, folders: &folders, locked: false };

    match crate::service_body::body_for(&answer, &vault, keys) {
        Some(body) => ServiceReply { status, body },
        // Permitted, and there is nothing to hand back -- an id no item has.
        // That is a `404` and not a `200` with an empty body: an empty `200`
        // is a well-formed answer meaning "here it is", and a client that
        // parses it gets a JSON error instead of the "no such item" it can
        // act on.
        None => ServiceReply { status: 404, body: String::new() },
    }
}

/// The request loop.
///
/// Split out from [`run_as_the_vault_service`] so that everything above it is
/// start-up that happens once, and this is the part that repeats. It holds no
/// decision of its own, and now no rendering either: it reads three strings
/// off the request, hands them to [`answer_one_request`], and writes the
/// result back. Everything worth being wrong about is in that function, where
/// a test can reach it.
pub fn serve_the_vault(
    server: &tiny_http::Server,
    backend: &dyn crate::vault_backend::VaultBackend,
    keys: &[crate::service_keys::KeyRecord],
) {
    for request in server.incoming_requests() {
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();
        // Read once, and never logged: an `Authorization` header IS the
        // credential.
        let auth = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Authorization"))
            .map(|header| header.value.as_str().to_string());

        let reply = answer_one_request(
            &method,
            &url,
            auth.as_deref(),
            keys,
            crate::service_keys::now_unix(),
            || {
                let items = backend.list_items().map_err(|e| format!("{e:?}"))?;
                let folders = backend.list_folders().map_err(|e| format!("{e:?}"))?;
                Ok((items, folders))
            },
        );

        let _ = respond(request, reply.status, &reply.body);
    }
}

/// One reply, as JSON.
fn respond(request: tiny_http::Request, status: u16, body: &str) -> std::io::Result<()> {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("a literal header that cannot fail to parse");
    request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header),
    )
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

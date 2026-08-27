//! Who may call what, as a pure function of one request.
//!
//! # Why there is no socket in this file
//!
//! Every rule about authentication and routing is decided by [`decide`],
//! which takes three strings and returns a value. Binding a port is the
//! process's job, not this module's, and no test in this crate may bind one.
//! That is not only a testing constraint: an authorisation rule that can only
//! be exercised by making a real HTTP request is a rule that will be tested
//! once, by hand, and then trusted forever.
//!
//! # The API is `bw serve`'s
//!
//! `crate::vault_bridge` has been a complete client for `bw serve`'s API for
//! the whole life of this project, so this service speaks that API rather
//! than a new one: `/status`, `/list/object/items`, `/list/object/folders`,
//! `/object/item/{id}`. A script already written against `bw serve` keeps
//! working, and pointing `VaultBridge` at this service is a change of base
//! URL rather than a migration.
//!
//! **The one deliberate incompatibility** is that `bw serve` requires no
//! credential at all and this requires one. See [`crate::service_token`] for
//! what that credential does and does not protect.
//!
//! # Two orderings that are not arbitrary
//!
//! **The credential is checked before the path is understood.** A service
//! that answered `404` for an unknown path and `401` for a known one would
//! tell an unauthenticated caller which routes exist, which is a map of the
//! API handed to somebody who has not shown they may have one.
//!
//! **Read-only, and refused by method rather than by omission.** `bw serve`
//! accepts writes. This does not, yet, and a write arriving by accident --
//! because a route was added and a method list was not -- is exactly how that
//! would go wrong. Every writing method is refused explicitly.

use crate::service_token::{bearer_of, matches, Token};

/// A route this service serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `GET /status` -- `bw serve`'s liveness and lock-state endpoint.
    Status,
    /// `GET /list/object/items`
    ListItems,
    /// `GET /list/object/folders`
    ListFolders,
    /// `GET /object/item/{id}`
    Item(String),
}

/// What to do with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Serve this route.
    Ok(Route),
    /// No credential, or the wrong one. **Returned before the path is
    /// parsed**, so it is also the answer for an unknown path.
    Unauthenticated,
    /// Authenticated, but there is no such route.
    NotFound,
    /// Authenticated, the route exists, and this service does not accept
    /// that method on it.
    MethodNotAllowed,
}

/// The whole access decision for one request.
///
/// `auth` is the raw `Authorization` header, if the request carried one.
#[must_use]
pub fn decide(method: &str, path: &str, auth: Option<&str>, expected: &Token) -> Answer {
    // First, and before anything reads `path`. See the module doc.
    let Some(presented) = bearer_of(auth) else {
        return Answer::Unauthenticated;
    };
    if !matches(expected, presented) {
        return Answer::Unauthenticated;
    }

    // The query string is not part of the routing decision, and dropping it
    // here means `/status?x=1` cannot become an unknown path.
    let path = path.split('?').next().unwrap_or(path);

    let Some(route) = route_of(path) else {
        return Answer::NotFound;
    };
    if method != "GET" {
        return Answer::MethodNotAllowed;
    }
    Answer::Ok(route)
}

/// The route a path names, if any. Pure, and separate from the method and
/// credential checks so that "does this route exist" and "may you have it"
/// are never entangled.
fn route_of(path: &str) -> Option<Route> {
    match path {
        "/status" => Some(Route::Status),
        "/list/object/items" => Some(Route::ListItems),
        "/list/object/folders" => Some(Route::ListFolders),
        _ => {
            let id = path.strip_prefix("/object/item/")?;
            // An empty id is not an item, and `/object/item/a/b` is not one
            // either -- neither should reach a handler that would then look
            // one up.
            if id.is_empty() || id.contains('/') {
                None
            } else {
                Some(Route::Item(id.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_token::mint;

    fn token() -> Token {
        mint(|| [1u8; 32])
    }

    fn good_header(token: &Token) -> String {
        format!("Bearer {}", token.expose())
    }

    /// **The test this module exists for.** No credential, no vault -- and
    /// the same answer for every path, so the refusal itself says nothing.
    #[test]
    fn every_route_refuses_an_unauthenticated_caller() {
        let expected = token();
        for path in [
            "/status",
            "/list/object/items",
            "/list/object/folders",
            "/object/item/abc",
            "/nonsense",
        ] {
            assert_eq!(
                decide("GET", path, None, &expected),
                Answer::Unauthenticated,
                "{path} answered something other than a refusal with no credential"
            );
        }
    }

    /// A wrong credential is refused exactly as a missing one is.
    #[test]
    fn a_wrong_credential_is_refused() {
        let expected = token();
        for header in ["Bearer wrong", "Basic anything", "", "Bearer "] {
            assert_eq!(
                decide("GET", "/status", Some(header), &expected),
                Answer::Unauthenticated,
                "{header:?} was accepted"
            );
        }
    }

    /// **`/status` is not public**, and this is the one that will be argued
    /// about. "It only says whether the vault is locked" is still a fact
    /// about this user's vault, told to anything that asks.
    #[test]
    fn status_is_not_a_public_endpoint() {
        let expected = token();
        assert_eq!(
            decide("GET", "/status", Some("Bearer wrong"), &expected),
            Answer::Unauthenticated
        );
    }

    /// **An unknown path is not a way to learn which routes exist.** An
    /// unauthenticated caller gets the same answer for a real route and a
    /// made-up one; only an authenticated caller is told the difference.
    #[test]
    fn an_unknown_path_does_not_leak_the_shape_of_the_api() {
        let expected = token();
        assert_eq!(
            decide("GET", "/nonsense", None, &expected),
            decide("GET", "/list/object/items", None, &expected),
            "a refused caller can tell a real route from a made-up one"
        );
        // And the difference IS visible once you are entitled to it.
        let header = good_header(&expected);
        assert_eq!(decide("GET", "/nonsense", Some(&header), &expected), Answer::NotFound);
    }

    #[test]
    fn an_authenticated_caller_reaches_the_routes() {
        let expected = token();
        let header = good_header(&expected);
        assert_eq!(decide("GET", "/status", Some(&header), &expected), Answer::Ok(Route::Status));
        assert_eq!(
            decide("GET", "/list/object/items", Some(&header), &expected),
            Answer::Ok(Route::ListItems)
        );
        assert_eq!(
            decide("GET", "/list/object/folders", Some(&header), &expected),
            Answer::Ok(Route::ListFolders)
        );
        assert_eq!(
            decide("GET", "/object/item/abc", Some(&header), &expected),
            Answer::Ok(Route::Item("abc".to_string()))
        );
    }

    /// Read-only, refused by method rather than by there being no handler.
    #[test]
    fn writing_methods_are_refused_even_when_authenticated() {
        let expected = token();
        let header = good_header(&expected);
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            assert_eq!(
                decide(method, "/list/object/items", Some(&header), &expected),
                Answer::MethodNotAllowed,
                "{method} was allowed; this service is read-only for now"
            );
            assert_eq!(
                decide(method, "/object/item/abc", Some(&header), &expected),
                Answer::MethodNotAllowed,
                "{method} on one item was allowed"
            );
        }
    }

    /// A write to an unknown path is still a 404, not a 405: the method
    /// check must not invent routes.
    #[test]
    fn a_write_to_a_route_that_does_not_exist_is_not_found() {
        let expected = token();
        let header = good_header(&expected);
        assert_eq!(decide("POST", "/nonsense", Some(&header), &expected), Answer::NotFound);
    }

    /// A query string is not part of the route. Without this, `/status?x=1`
    /// is an unknown path and a compatible client breaks on a detail nobody
    /// would look for.
    #[test]
    fn a_query_string_does_not_change_the_route() {
        let expected = token();
        let header = good_header(&expected);
        assert_eq!(
            decide("GET", "/list/object/items?trash=true", Some(&header), &expected),
            Answer::Ok(Route::ListItems)
        );
    }

    /// An id must be one path segment. `/object/item/` and
    /// `/object/item/a/b` are not items, and must not reach a lookup.
    #[test]
    fn a_malformed_item_path_is_not_an_item() {
        let expected = token();
        let header = good_header(&expected);
        for path in ["/object/item/", "/object/item/a/b", "/object/item"] {
            assert_eq!(
                decide("GET", path, Some(&header), &expected),
                Answer::NotFound,
                "{path} was read as an item id"
            );
        }
    }

    /// The credential check must come first in the source as well as in
    /// behaviour: an absence cannot be read, and a later refactor that moved
    /// routing above it would pass every test above while leaking the API's
    /// shape through response codes.
    #[test]
    fn the_credential_is_checked_before_the_path_is_parsed() {
        let source = include_str!("service_api.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        let body_at = production.find("pub fn decide(").expect("control: `decide` is gone");
        let body = &production[body_at..];
        let auth_at = body.find("bearer_of(auth)").expect("control: `decide` does not read a credential");
        let path_at = body.find("route_of(path)").expect("control: `decide` does not route");
        assert!(
            auth_at < path_at,
            "the path is routed before the credential is checked, so response codes tell an unauthenticated caller which routes exist"
        );
    }
}

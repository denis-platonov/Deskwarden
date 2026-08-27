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
//! **The scope is checked here, not in a handler.** A handler that checks
//! its own permissions is a handler somebody adds a route without. Putting
//! the check between routing and the answer is what makes a per-item grant
//! real rather than decorative: `/object/item/{id}` is checked against THAT
//! id, in the one place every request passes through.
//!
//! # Writes
//!
//! The method is not a gate; it is an [`Access`]. `GET` asks to read and
//! everything else asks to write, and the scope decides. An earlier version
//! of this module refused every writing method outright -- that was caution,
//! and it was wrong twice over: it made half the scope model decorative, and
//! it would have stopped `VaultBridge` ever pointing here, which is the whole
//! compatibility argument. What was right about it was only the ordering:
//! writes must not exist before scopes do, which is why they arrive in the
//! same change as the check that bounds them.

use crate::service_keys::{permits, Access, KeyRecord, Subject};
use crate::service_token::bearer_of;

/// The one path reachable without a credential. A constant, so the exemption
/// is one string in one place rather than a pattern that could grow.
pub const AUTH_PATH: &str = "/auth";

/// A route this service serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `POST /auth` -- the master password in, a short-lived session out.
    ///
    /// **The only route reachable without a credential**, because it is the
    /// route that issues one. It is a fixed, known path rather than one
    /// discovered by routing, so admitting it costs nothing an
    /// unauthenticated caller did not already know.
    Auth,
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
    /// Handle the master-password exchange. Carries no key index, because
    /// the whole point is that there is not one yet.
    Authenticate,
    /// Serve this route.
    ///
    /// `key` is the index of the record that authorised it, so the code
    /// building the body can narrow what it returns to what that key may
    /// see -- a category-scoped key gets a filtered list rather than a
    /// refusal.
    Ok { route: Route, key: usize },
    /// No credential, an unknown one, or an expired one. **Returned before
    /// the path is parsed**, so it is also the answer for an unknown path.
    Unauthenticated,
    /// A live key that is not allowed to do this.
    ///
    /// **Distinct from [`Answer::Unauthenticated`] on purpose.** A
    /// legitimate script has to be able to learn that it needs a wider
    /// scope; collapsing the two would leave the owner debugging a working
    /// key that looks broken.
    Forbidden,
    /// Authenticated, but there is no such route.
    NotFound,
    /// Authenticated, the route exists, and the method is not one this
    /// service understands at all.
    MethodNotAllowed,
}

/// The whole access decision for one request.
///
/// `auth` is the raw `Authorization` header, if the request carried one, and
/// `now_unix` is the moment to judge expiry against -- passed in rather than
/// read here, so a test can drive the day after a key expires without waiting
/// for it.
#[must_use]
pub fn decide(
    method: &str,
    path: &str,
    auth: Option<&str>,
    keys: &[KeyRecord],
    now_unix: u64,
) -> Answer {
    let bare_path = path.split('?').next().unwrap_or(path);

    // The one exemption, and it is a fixed string rather than a routing
    // decision: `/auth` is where a credential comes FROM, so requiring one
    // would leave no way in. Compared before the credential check for that
    // reason, and compared against a literal so it cannot widen.
    if bare_path == AUTH_PATH {
        return if method == "POST" { Answer::Authenticate } else { Answer::MethodNotAllowed };
    }

    // Now the credential, and still before anything ROUTES `path`.
    let Some(presented) = bearer_of(auth) else {
        return Answer::Unauthenticated;
    };
    let Some(index) = crate::service_keys::find_index(keys, presented, now_unix) else {
        return Answer::Unauthenticated;
    };

    let Some(route) = route_of(bare_path) else {
        return Answer::NotFound;
    };

    let Some(access) = access_of(method) else {
        return Answer::MethodNotAllowed;
    };

    if !allowed(&keys[index], access, &route) {
        return Answer::Forbidden;
    }
    Answer::Ok { route, key: index }
}

/// What a method is asking to do.
///
/// `GET` reads; every method that changes something writes. A method this
/// service has no meaning for at all is `None`, and becomes
/// [`Answer::MethodNotAllowed`] -- which is a different statement from "you
/// may not", and is why the two are not one answer.
fn access_of(method: &str) -> Option<Access> {
    match method {
        "GET" => Some(Access::Read),
        "POST" | "PUT" | "DELETE" | "PATCH" => Some(Access::Write),
        _ => None,
    }
}

/// Whether this key may take this action on this route.
///
/// The subject each route is judged against is the narrowest one the route
/// names, which is the whole point of a per-item grant.
fn allowed(key: &KeyRecord, access: Access, route: &Route) -> bool {
    match route {
        // Never reached: `decide` answers `/auth` before any key is
        // resolved. Present so this match is total, and denying rather than
        // allowing so that a future caller that did reach it gets the safe
        // answer.
        Route::Auth => false,
        // Whether the vault is locked is not vault contents, and every
        // client needs it to work at all. A live key is enough; no scope
        // grants or withholds it. It is still refused to an unauthenticated
        // caller, which is the line that matters.
        Route::Status => true,
        // A list is judged loosely here and narrowed when the body is built:
        // a key with any read grant may ask, and sees only what it may see.
        // Refusing outright would make a category grant useless for the one
        // job it exists for.
        Route::ListItems | Route::ListFolders => {
            access == Access::Read && has_any(key, Access::Read)
        }
        // One item, judged against that id.
        //
        // A CATEGORY grant does not pass here, and that is a real limit
        // rather than an oversight: this function is not given the item, so
        // it cannot know the item's kind, and asking the vault first would
        // mean fetching something the caller may not be allowed to have. A
        // category-scoped key reaches those items through the filtered list
        // instead, which returns the same data.
        Route::Item(id) => permits(key, access, &Subject::Item(id.clone())),
    }
}

/// Whether a key has any grant at all of this access.
fn has_any(key: &KeyRecord, access: Access) -> bool {
    permits(key, access, &Subject::All)
        || key.scopes.iter().any(|scope| scope.access == access)
}

/// The route a path names, if any. Pure, and separate from the method and
/// credential checks so that "does this route exist" and "may you have it"
/// are never entangled.
fn route_of(path: &str) -> Option<Route> {
    match path {
        // Handled in `decide` before the credential check; listed here only
        // so that `route_of` is total over the paths this service knows.
        AUTH_PATH => Some(Route::Auth),
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
    use crate::service_keys::{hash_key, KeyRecord, Scope};
    use crate::vault_bridge::ItemKind;

    const NOW: u64 = 1_700_000_000;
    const KEY: &str = "the-key";

    fn key_with(scopes: Vec<Scope>) -> KeyRecord {
        KeyRecord {
            name: "a key".to_string(),
            hash: hash_key(KEY),
            created_unix: 1,
            expires_unix: None,
            scopes,
        }
    }

    fn scope(subject: Subject, access: Access) -> Scope {
        Scope { subject, access }
    }

    fn reads_everything() -> Vec<KeyRecord> {
        vec![key_with(vec![scope(Subject::All, Access::Read)])]
    }

    fn reads_and_writes_everything() -> Vec<KeyRecord> {
        vec![key_with(vec![
            scope(Subject::All, Access::Read),
            scope(Subject::All, Access::Write),
        ])]
    }

    fn reads_one_item(id: &str) -> Vec<KeyRecord> {
        vec![key_with(vec![scope(Subject::Item(id.to_string()), Access::Read)])]
    }

    fn bearer(value: &str) -> String {
        format!("Bearer {value}")
    }

    fn good() -> String {
        bearer(KEY)
    }

    /// **The test this module exists for.** No credential, no vault -- and
    /// the same answer for every path, so the refusal itself says nothing.
    #[test]
    fn every_route_refuses_an_unauthenticated_caller() {
        let keys = reads_and_writes_everything();
        for path in [
            "/status",
            "/list/object/items",
            "/list/object/folders",
            "/object/item/abc",
            "/nonsense",
        ] {
            assert_eq!(
                decide("GET", path, None, &keys, NOW),
                Answer::Unauthenticated,
                "{path} answered something other than a refusal with no credential"
            );
        }
    }

    #[test]
    fn a_wrong_credential_is_refused() {
        let keys = reads_everything();
        for header in ["Bearer wrong", "Basic anything", "", "Bearer "] {
            assert_eq!(
                decide("GET", "/status", Some(header), &keys, NOW),
                Answer::Unauthenticated,
                "{header:?} was accepted"
            );
        }
    }

    /// **An expired key is refused exactly as an unknown one is**, and this
    /// is checked at the moment of the request.
    #[test]
    fn an_expired_key_is_refused() {
        let mut record = key_with(vec![scope(Subject::All, Access::Read)]);
        record.expires_unix = Some(NOW);
        let keys = vec![record];
        assert_eq!(
            decide("GET", "/status", Some(&good()), &keys, NOW - 1),
            Answer::Ok { route: Route::Status, key: 0 },
            "control: it worked before expiry"
        );
        assert_eq!(
            decide("GET", "/status", Some(&good()), &keys, NOW),
            Answer::Unauthenticated
        );
    }

    /// **`/status` is not public.** A live key is enough for it -- lock state
    /// is not vault contents, and every client needs it -- but an
    /// unauthenticated caller is told nothing.
    #[test]
    fn status_is_not_a_public_endpoint() {
        let keys = reads_everything();
        assert_eq!(
            decide("GET", "/status", Some("Bearer wrong"), &keys, NOW),
            Answer::Unauthenticated
        );
    }

    /// **An unknown path is not a way to learn which routes exist.**
    #[test]
    fn an_unknown_path_does_not_leak_the_shape_of_the_api() {
        let keys = reads_everything();
        assert_eq!(
            decide("GET", "/nonsense", None, &keys, NOW),
            decide("GET", "/list/object/items", None, &keys, NOW),
            "a refused caller can tell a real route from a made-up one"
        );
        assert_eq!(
            decide("GET", "/nonsense", Some(&good()), &keys, NOW),
            Answer::NotFound
        );
    }

    #[test]
    fn an_authorised_caller_reaches_the_routes() {
        let keys = reads_everything();
        let header = good();
        assert_eq!(
            decide("GET", "/status", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::Status, key: 0 }
        );
        assert_eq!(
            decide("GET", "/list/object/items", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::ListItems, key: 0 }
        );
        assert_eq!(
            decide("GET", "/list/object/folders", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::ListFolders, key: 0 }
        );
        assert_eq!(
            decide("GET", "/object/item/abc", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::Item("abc".to_string()), key: 0 }
        );
    }

    /// **The test per-item grants exist for.** Checked in `decide`, not in a
    /// handler -- a handler that checks its own permissions is one somebody
    /// adds a route without.
    #[test]
    fn a_key_scoped_to_one_item_cannot_read_a_different_one() {
        let keys = reads_one_item("allowed-id");
        let header = good();
        assert_eq!(
            decide("GET", "/object/item/allowed-id", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::Item("allowed-id".to_string()), key: 0 },
            "control: the key cannot read the item it was granted"
        );
        assert_eq!(
            decide("GET", "/object/item/other-id", Some(&header), &keys, NOW),
            Answer::Forbidden
        );
    }

    /// A read-only key may not write, and the refusal is a SCOPE refusal
    /// rather than a blanket method ban.
    #[test]
    fn a_read_only_key_cannot_write() {
        let keys = reads_everything();
        let header = good();
        assert_eq!(
            decide("GET", "/object/item/abc", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::Item("abc".to_string()), key: 0 },
            "control: the key cannot read either"
        );
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            assert_eq!(
                decide(method, "/object/item/abc", Some(&header), &keys, NOW),
                Answer::Forbidden,
                "{method} was allowed for a read-only key"
            );
        }
    }

    /// **And a write-scoped key may.** Without this, the test above passes
    /// on a service that simply refuses every write -- which is exactly the
    /// thing this change replaces.
    #[test]
    fn a_write_scoped_key_can_write() {
        let keys = reads_and_writes_everything();
        let header = good();
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            assert_eq!(
                decide(method, "/object/item/abc", Some(&header), &keys, NOW),
                Answer::Ok { route: Route::Item("abc".to_string()), key: 0 },
                "{method} was refused for a key scoped to write everything"
            );
        }
    }

    /// A method this service has no meaning for is a different statement
    /// from "you may not", and stays a different answer.
    #[test]
    fn an_unknown_method_is_not_a_scope_refusal() {
        let keys = reads_and_writes_everything();
        assert_eq!(
            decide("TRACE", "/object/item/abc", Some(&good()), &keys, NOW),
            Answer::MethodNotAllowed
        );
    }

    /// A key with no scopes at all is refused everything except `/status`,
    /// which needs only a live key.
    #[test]
    fn a_key_with_no_scopes_reaches_nothing_that_touches_the_vault() {
        let keys = vec![key_with(vec![])];
        let header = good();
        assert_eq!(
            decide("GET", "/list/object/items", Some(&header), &keys, NOW),
            Answer::Forbidden
        );
        assert_eq!(
            decide("GET", "/object/item/abc", Some(&header), &keys, NOW),
            Answer::Forbidden
        );
        assert_eq!(
            decide("GET", "/status", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::Status, key: 0 }
        );
    }

    /// A category-scoped key may ask for the list -- it is narrowed when the
    /// body is built -- but may not fetch an arbitrary item by id.
    #[test]
    fn a_category_key_may_list_but_not_fetch_an_arbitrary_item() {
        let keys = vec![key_with(vec![scope(
            Subject::Category(ItemKind::Login),
            Access::Read,
        )])];
        let header = good();
        assert_eq!(
            decide("GET", "/list/object/items", Some(&header), &keys, NOW),
            Answer::Ok { route: Route::ListItems, key: 0 }
        );
        assert_eq!(
            decide("GET", "/object/item/abc", Some(&header), &keys, NOW),
            Answer::Forbidden
        );
    }

    /// **A refused scope is not reported as a bad credential.** A legitimate
    /// script has to be able to learn it needs a wider scope.
    #[test]
    fn a_refused_scope_is_not_reported_as_a_bad_credential() {
        let keys = reads_one_item("allowed-id");
        assert_eq!(
            decide("GET", "/object/item/other", Some(&bearer("wrong")), &keys, NOW),
            Answer::Unauthenticated
        );
        assert_eq!(
            decide("GET", "/object/item/other", Some(&good()), &keys, NOW),
            Answer::Forbidden
        );
    }

    /// The answer says WHICH key authorised it, so the body can be narrowed
    /// to what that key may see.
    #[test]
    fn the_answer_names_the_key_that_authorised_it() {
        let mut keys = reads_everything();
        keys.insert(0, key_with(vec![]));
        keys[0].hash = hash_key("a-different-key");
        assert_eq!(
            decide("GET", "/status", Some(&good()), &keys, NOW),
            Answer::Ok { route: Route::Status, key: 1 },
            "the wrong key was credited with the request"
        );
    }

    /// A query string is not part of the route.
    #[test]
    fn a_query_string_does_not_change_the_route() {
        let keys = reads_everything();
        assert_eq!(
            decide("GET", "/list/object/items?trash=true", Some(&good()), &keys, NOW),
            Answer::Ok { route: Route::ListItems, key: 0 }
        );
    }

    /// An id must be one path segment.
    #[test]
    fn a_malformed_item_path_is_not_an_item() {
        let keys = reads_everything();
        for path in ["/object/item/", "/object/item/a/b", "/object/item"] {
            assert_eq!(
                decide("GET", path, Some(&good()), &keys, NOW),
                Answer::NotFound,
                "{path} was read as an item id"
            );
        }
    }

    /// **`/auth` is the only way in without a credential**, because it is
    /// where a credential comes from.
    #[test]
    fn the_auth_route_is_reachable_without_a_credential() {
        let keys = reads_everything();
        assert_eq!(decide("POST", "/auth", None, &keys, NOW), Answer::Authenticate);
    }

    /// And it is the ONLY one. This is the test that would catch an
    /// exemption that grew.
    #[test]
    fn no_other_route_is_reachable_without_a_credential() {
        let keys = reads_and_writes_everything();
        for path in [
            "/status",
            "/list/object/items",
            "/list/object/folders",
            "/object/item/abc",
            "/nonsense",
            "/auth/extra",
            "/Auth",
            "/auth/",
        ] {
            assert_eq!(
                decide("POST", path, None, &keys, NOW),
                Answer::Unauthenticated,
                "{path} was reachable without a credential"
            );
        }
    }

    /// The master password goes in a body, so `/auth` is POST only. A GET is
    /// a method refusal rather than a credential refusal -- the route is
    /// public, so there is nothing to hide about it.
    #[test]
    fn the_auth_route_is_post_only() {
        let keys = reads_everything();
        for method in ["GET", "PUT", "DELETE", "PATCH", "TRACE"] {
            assert_eq!(
                decide(method, "/auth", None, &keys, NOW),
                Answer::MethodNotAllowed,
                "{method} /auth was accepted"
            );
        }
    }

    /// A query string does not smuggle a request past the exemption, and
    /// does not stop a legitimate one either.
    #[test]
    fn the_auth_exemption_is_not_confused_by_a_query_string() {
        let keys = reads_everything();
        assert_eq!(decide("POST", "/auth?x=1", None, &keys, NOW), Answer::Authenticate);
        assert_eq!(
            decide("POST", "/status?x=/auth", None, &keys, NOW),
            Answer::Unauthenticated
        );
    }

    /// Presenting a perfectly good key to `/auth` still authenticates rather
    /// than erroring: the route does not depend on there being no key.
    #[test]
    fn a_caller_that_already_has_a_key_may_still_use_the_auth_route() {
        let keys = reads_everything();
        assert_eq!(decide("POST", "/auth", Some(&good()), &keys, NOW), Answer::Authenticate);
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
        let auth_at =
            body.find("bearer_of(auth)").expect("control: `decide` does not read a credential");
        let path_at = body.find("route_of(bare_path)").expect("control: `decide` does not route");
        assert!(
            auth_at < path_at,
            "the path is routed before the credential is checked, so response codes tell an unauthenticated caller which routes exist"
        );

        // `/auth` IS compared before the credential, deliberately -- it is
        // where a credential comes from. The exemption must stay exactly one
        // comparison against one constant: anything richer before the
        // credential check is routing, and routing there is the leak this
        // pin exists to stop.
        let before = &body[..auth_at];
        assert_eq!(
            before.matches("bare_path ==").count(),
            1,
            "the public-route exemption is no longer a single comparison; whatever grew there is routing an unauthenticated caller's path"
        );
        assert!(
            before.contains("AUTH_PATH"),
            "the exemption compares against something other than the AUTH_PATH constant"
        );
        assert!(
            !before.contains("route_of"),
            "`route_of` runs before the credential is checked"
        );
    }

    /// And the scope check must come before the answer, in `decide` rather
    /// than anywhere downstream.
    #[test]
    fn the_scope_is_checked_inside_decide() {
        let source = include_str!("service_api.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        let body_at = production.find("pub fn decide(").expect("control: `decide` is gone");
        let body = &production[body_at..];
        let end = body.find("\n}").expect("control: could not find the end of `decide`");
        let body = &body[..end];
        assert!(
            body.contains("allowed("),
            "`decide` no longer checks the scope; a per-item grant is decorative unless it is checked in the one place every request passes through"
        );
    }
}

//! What comes back, and what a key is allowed to see of it.
//!
//! [`crate::service_api`] decides who may ask. This decides what the answer
//! contains, and the two are separate because they fail differently: a
//! routing mistake refuses a legitimate caller, and a body mistake hands a
//! caller something they were never granted.
//!
//! # The envelope is `bw serve`'s
//!
//! `{"success":true,"data":{...}}`, with a list nested one deeper as
//! `{"success":true,"data":{"data":[...]}}`. That is not a style choice --
//! [`crate::vault_bridge`] already parses exactly this, and every script
//! written against `bw serve` expects it. A test asserts our own client can
//! read our own body, because "compatible" is a claim that decays silently.
//!
//! # The list is filtered, not refused
//!
//! A key scoped to one category may ask for the whole list and receives only
//! the items it may see. Refusing outright would make a category grant
//! useless for the one job it exists for -- a backup script that reads
//! Logins -- and returning everything would make the grant a lie.
//!
//! This is why [`crate::service_api::Answer::Ok`] carries which key
//! authorised the request: the filter needs it, and reaching for "the
//! current key" from somewhere else would be a second source of truth.
//!
//! # Nothing is built for a caller who was refused
//!
//! [`body_for`] answers `None` for every refusal. A body assembled and then
//! discarded is a body that exists in this process's memory for no reason,
//! and one `if` away from being sent.

use crate::service_api::{Answer, Route};
use crate::service_keys::{permits, Access, KeyRecord, Subject};
use crate::vault_bridge::{Folder, ItemKind, VaultItem};

/// Whether this key may see this item.
///
/// Three ways to be allowed and no fourth: the whole vault, this item's
/// category, or this item by id. The category is computed from the item
/// rather than trusted from anywhere, which is what
/// [`crate::service_api`] could not do -- it has the id and not the item,
/// and that asymmetry is why single-item fetches are refused there for a
/// category-scoped key while the list is filtered here.
#[must_use]
pub fn may_see(key: &KeyRecord, item: &VaultItem) -> bool {
    permits(key, Access::Read, &Subject::All)
        || permits(key, Access::Read, &Subject::Category(ItemKind::of(item)))
        || permits(key, Access::Read, &Subject::Item(item.id.clone()))
}

/// `GET /list/object/items`, narrowed to what `key` may see.
#[must_use]
pub fn list_items_body(items: &[VaultItem], key: &KeyRecord) -> String {
    let visible: Vec<&VaultItem> = items.iter().filter(|item| may_see(key, item)).collect();
    envelope_list(&visible)
}

/// `GET /list/object/folders`.
///
/// Folders are not items and carry no secret -- a name and an id. They are
/// not filtered by item scope, because there is no item scope that could
/// sensibly apply to one, and withholding a folder name from a caller who
/// can see items inside it would be a shape with no meaning.
#[must_use]
pub fn list_folders_body(folders: &[Folder]) -> String {
    envelope_list(&folders.iter().collect::<Vec<_>>())
}

/// `GET /object/item/{id}`.
#[must_use]
pub fn item_body(item: &VaultItem) -> String {
    envelope(item)
}

/// `GET /status`.
///
/// Lock state and nothing else. Deliberately not the account's email or the
/// server URL: a live key is enough to reach this route, and it should not
/// be a way to learn who the owner is.
#[must_use]
pub fn status_body(locked: bool) -> String {
    let status = if locked { "locked" } else { "unlocked" };
    envelope(&serde_json::json!({ "status": status }))
}

/// The body for one decided request, or `None` when there is not one.
///
/// **`None` for every refusal.** The caller sends a status code and no body,
/// which is what stops a refusal from carrying anything.
#[must_use]
pub fn body_for(answer: &Answer, vault: &Vault<'_>, keys: &[KeyRecord]) -> Option<String> {
    let Answer::Ok { route, key } = answer else {
        return None;
    };
    let key = keys.get(*key)?;
    match route {
        Route::Status => Some(status_body(vault.locked)),
        Route::ListItems => Some(list_items_body(vault.items, key)),
        Route::ListFolders => Some(list_folders_body(vault.folders)),
        Route::Item(id) => {
            let item = vault.items.iter().find(|item| &item.id == id)?;
            // Checked again here, and not because `service_api` is not
            // trusted: it judged an ID, and this judges the ITEM, which is
            // the only place the item's category is known. A per-item grant
            // passes both; a category grant is refused above and would pass
            // here, and the narrower answer is the one that holds.
            if may_see(key, item) {
                Some(item_body(item))
            } else {
                None
            }
        }
        // `service_api` answers `/auth` with `Answer::Authenticate`, which
        // is not `Ok`, so this is unreachable. Denying rather than serving,
        // so an unreachable arm that becomes reachable fails closed.
        Route::Auth => None,
    }
}

/// What the service knows right now, borrowed for one request.
///
/// A struct rather than four parameters so that adding a fifth thing the
/// service can answer about does not silently reorder two `&[T]` arguments
/// at every call site.
pub struct Vault<'a> {
    pub items: &'a [VaultItem],
    pub folders: &'a [Folder],
    pub locked: bool,
}

fn envelope<T: serde::Serialize>(data: &T) -> String {
    serde_json::json!({ "success": true, "data": data }).to_string()
}

fn envelope_list<T: serde::Serialize>(data: &[T]) -> String {
    serde_json::json!({ "success": true, "data": { "data": data } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_keys::{hash_key, Scope};

    fn key_with(scopes: Vec<Scope>) -> KeyRecord {
        KeyRecord {
            name: "a key".to_string(),
            hash: hash_key("the-key"),
            created_unix: 1,
            expires_unix: None,
            scopes,
        }
    }

    fn read(subject: Subject) -> Scope {
        Scope { subject, access: Access::Read }
    }

    fn item(id: &str, name: &str, kind: i64, password: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "type": kind,
            "login": { "username": "someone@example.com", "password": password,
                       "uris": [{ "uri": "https://example.com" }] },
        }))
        .expect("the fixture must parse as a VaultItem")
    }

    fn a_login() -> VaultItem {
        item("login-1", "Example", 1, "hunter2")
    }

    fn a_card() -> VaultItem {
        item("card-1", "Bank", 3, "unused")
    }

    /// **Compatibility is the requirement, so it is asserted rather than
    /// assumed:** the shape `VaultBridge` already parses is the shape this
    /// answers with. Without this, "drop-in for `bw serve`" is a claim in a
    /// comment.
    #[test]
    fn our_own_client_can_read_our_own_list_body() {
        let items = [a_login()];
        let body = list_items_body(&items, &key_with(vec![read(Subject::All)]));
        let parsed: serde_json::Value =
            serde_json::from_str(&body).expect("our own list body is not valid JSON");
        assert_eq!(parsed["success"], true);
        let listed = parsed["data"]["data"].as_array().expect("bw serve nests a list one deeper");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], "login-1");
        assert_eq!(listed[0]["login"]["password"], "hunter2");
    }

    /// And the single-item envelope is one level shallower, as `bw serve`'s
    /// is. Getting this wrong would break every client in a way no type
    /// checks.
    #[test]
    fn the_single_item_envelope_is_not_nested_like_a_list() {
        let body = item_body(&a_login());
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(parsed["data"]["id"], "login-1");
        assert!(parsed["data"]["data"].is_null(), "a single item is nested like a list");
    }

    /// **The filter is the point of a category grant.**
    #[test]
    fn a_category_key_sees_only_its_category() {
        let key = key_with(vec![read(Subject::Category(ItemKind::Login))]);
        let items = [a_login(), a_card()];
        let body = list_items_body(&items, &key);
        assert!(body.contains("login-1"), "control: the granted item is missing");
        assert!(!body.contains("card-1"), "a card leaked to a key scoped to logins");
        assert!(!body.contains("Bank"), "a card's name leaked to a key scoped to logins");
    }

    /// A per-item key sees one item, and the list is how it gets there.
    #[test]
    fn an_item_key_sees_only_its_item() {
        let key = key_with(vec![read(Subject::Item("login-1".to_string()))]);
        let items = [a_login(), a_card()];
        let body = list_items_body(&items, &key);
        assert!(body.contains("login-1"));
        assert!(!body.contains("card-1"));
    }

    /// A key with no scopes sees an empty list rather than everything. The
    /// filter must default to excluding.
    #[test]
    fn a_key_with_no_scopes_sees_an_empty_list() {
        let body = list_items_body(&[a_login(), a_card()], &key_with(vec![]));
        assert!(!body.contains("login-1"), "an unscoped key was served the vault");
        assert!(!body.contains("card-1"));
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(
            parsed["data"]["data"].as_array().expect("still a list").len(),
            0,
            "an empty result must still be a well-formed list, not a missing field"
        );
    }

    /// Control for the three tests above: a key that may see everything
    /// does. Without this they all pass on a filter that excludes always.
    #[test]
    fn a_key_scoped_to_everything_sees_everything() {
        let body = list_items_body(&[a_login(), a_card()], &key_with(vec![read(Subject::All)]));
        assert!(body.contains("login-1"));
        assert!(body.contains("card-1"));
    }

    /// **No body is built for a caller who was refused.**
    #[test]
    fn nothing_is_built_for_a_refusal() {
        let items = [a_login()];
        let keys = [key_with(vec![read(Subject::All)])];
        let vault = Vault { items: &items, folders: &[], locked: false };
        for answer in [
            Answer::Unauthenticated,
            Answer::Forbidden,
            Answer::NotFound,
            Answer::MethodNotAllowed,
            Answer::Authenticate,
        ] {
            assert_eq!(
                body_for(&answer, &vault, &keys),
                None,
                "{answer:?} produced a body"
            );
        }
    }

    /// Control: an `Ok` does produce one, or the test above proves nothing.
    #[test]
    fn a_permitted_request_does_get_a_body() {
        let items = [a_login()];
        let keys = [key_with(vec![read(Subject::All)])];
        let vault = Vault { items: &items, folders: &[], locked: false };
        let answer = Answer::Ok { route: Route::ListItems, key: 0 };
        assert!(body_for(&answer, &vault, &keys).is_some());
    }

    /// The single-item route is judged against the ITEM, which is where its
    /// category is known -- so a grant that named a different item does not
    /// get served one it was never given.
    #[test]
    fn a_single_item_body_is_checked_against_the_item_itself() {
        let items = [a_login(), a_card()];
        let keys = [key_with(vec![read(Subject::Item("login-1".to_string()))])];
        let vault = Vault { items: &items, folders: &[], locked: false };
        assert!(
            body_for(&Answer::Ok { route: Route::Item("login-1".into()), key: 0 }, &vault, &keys)
                .is_some(),
            "control: the granted item was withheld"
        );
        assert_eq!(
            body_for(&Answer::Ok { route: Route::Item("card-1".into()), key: 0 }, &vault, &keys),
            None,
            "an item outside the grant was served"
        );
    }

    /// An id nobody has is not a body, and not a panic.
    #[test]
    fn an_unknown_item_id_yields_no_body() {
        let items = [a_login()];
        let keys = [key_with(vec![read(Subject::All)])];
        let vault = Vault { items: &items, folders: &[], locked: false };
        assert_eq!(
            body_for(&Answer::Ok { route: Route::Item("nope".into()), key: 0 }, &vault, &keys),
            None
        );
    }

    /// A key index that does not exist is not a panic either. `Answer`
    /// carries an index, and an index is only as good as the slice it is
    /// used against.
    #[test]
    fn a_key_index_out_of_range_yields_no_body() {
        let items = [a_login()];
        let vault = Vault { items: &items, folders: &[], locked: false };
        assert_eq!(
            body_for(&Answer::Ok { route: Route::ListItems, key: 7 }, &vault, &[]),
            None
        );
    }

    /// `/status` says whether the vault is locked and nothing else -- not
    /// who the owner is, and not where their server lives.
    #[test]
    fn status_says_only_whether_the_vault_is_locked() {
        for (locked, expected) in [(true, "locked"), (false, "unlocked")] {
            let body = status_body(locked);
            let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
            assert_eq!(parsed["data"]["status"], expected);
        }
        let body = status_body(false);
        for forbidden in ["@", "http", "email", "url", "server"] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` appears in the status body: {body}"
            );
        }
    }
}

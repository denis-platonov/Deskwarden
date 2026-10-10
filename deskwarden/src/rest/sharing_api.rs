//! **The sharing routes**, one typed function each -- Bitwarden's own, the
//! official column of spec §7.3, which every server answers
//! (`docs/superpowers/specs/2026-10-08-official-path-sharing-design.md`).
//! One NodeWarden route remains, and says so: the administrator's
//! registration code. There is no invitation list -- no official server has
//! one -- and an invitation is accepted from its link ([`accept_invite`]).
//!
//! No policy lives here. Which route to call, in what order, and what a
//! refusal means for the user is `rest::sharing`'s. What lives here is the
//! shape of each request and each answer, and the handful of answers that
//! mean success in disguise: a 404 on a delete, "already a member", "already
//! confirmed" -- the two-PC race of spec §13, which converges by treating
//! the loser's refusal as the winner's result.
//!
//! Every request goes through [`RestClient::send_sharing`], and
//! [`SharingRequest`] -- the only thing that door accepts -- is constructed
//! nowhere but this file.

use serde_json::{Value, json};

use crate::rest::api::{RestClient, RestError, Session, is_url_path_safe};
use crate::rest::crypto::{EncString, spki_from_base64};
use crate::rest::write::MappedCipher;

/// The four verbs these routes use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// One sharing request. **No `Debug`**: a body can hold a member's email and
/// an invitation token, and nothing needs to print one.
///
/// **Its body's strings are wiped when it is dropped** (fix round 1, M3).
/// Two bodies carry a secret in the clear -- an invitation's token
/// ([`accept_invite`]) and the administrator's master password hash
/// ([`registration_invite`]) -- and a `serde_json::Value` frees its strings
/// unwiped. Every body is wiped rather than those two, so a third cannot be
/// added that forgets to. What `ureq`'s `send_json` serialises the body into
/// on the way to the socket this crate does not own and cannot wipe -- the
/// limit `http_agent::bounded_total_with_bearer` states for the grant.
pub struct SharingRequest {
    method: SharingMethod,
    path: String,
    body: Option<Value>,
}

impl Drop for SharingRequest {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            use zeroize::Zeroize;
            match value {
                Value::String(text) => text.zeroize(),
                Value::Array(items) => items.iter_mut().for_each(wipe),
                Value::Object(fields) => fields.values_mut().for_each(wipe),
                Value::Null | Value::Bool(_) | Value::Number(_) => {}
            }
        }
        if let Some(body) = self.body.as_mut() {
            wipe(body);
        }
    }
}

impl SharingRequest {
    fn new(method: SharingMethod, path: String, body: Option<Value>) -> Self {
        Self { method, path, body }
    }

    pub(crate) fn method(&self) -> SharingMethod {
        self.method
    }

    /// The path under the server root, starting with `/api/`.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn body(&self) -> Option<&Value> {
        self.body.as_ref()
    }
}

/// A server id checked for use in a path -- [`is_url_path_safe`]'s rule,
/// the same one every other id in `rest::api` is held to.
fn safe(id: &str) -> Result<&str, RestError> {
    if is_url_path_safe(id) { Ok(id) } else { Err(RestError::UnsafeId) }
}

/// The server's sentence, lower-cased, when it refused.
fn refusal(error: &RestError) -> Option<String> {
    match error {
        RestError::Rejected { error, description } => Some(format!("{error} {description}").to_lowercase()),
        _ => None,
    }
}

/// `{"object":"list","data":[..]}` or a bare array, as `rest::organizations`
/// reads its envelopes.
fn rows(answer: &Value) -> Vec<Value> {
    match answer.get("data") {
        Some(Value::Array(rows)) => rows.clone(),
        _ => answer.as_array().cloned().unwrap_or_default(),
    }
}

fn text_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn send(client: &RestClient, session: &mut Session, request: &SharingRequest) -> Result<Value, RestError> {
    client.send_sharing(session, request)
}

// ---- answers ----------------------------------------------------------------

/// An account's id and public key: what a confirm wraps the org key to, and
/// what a pin records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountKey {
    pub user_id: String,
    /// SPKI DER.
    pub public_key: Vec<u8>,
}

/// A collection as the org's own listing answers it: the name still ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCollection {
    pub id: String,
    pub name: String,
}

/// A NodeWarden registration code, as the registration-invite route answers
/// it (NodeWarden's `toInviteResponse`, the shape `/api/admin/invites` uses
/// too).
///
/// **A hand-written `Debug` that prints neither the code nor the link**: the
/// code registers an account on the server, and the link carries it in its
/// fragment -- the `access_url` leak `crate::debug_leak_guard` opens with,
/// in a plain `String` its seed step cannot see. `Serialize` stays: the
/// daemon's answer to the window is the one place the code is meant to go.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCode {
    pub code: String,
    pub link: Option<String>,
    pub expires_at: Option<String>,
}

impl std::fmt::Debug for InviteCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteCode")
            .field("code", &"<redacted>")
            .field("link", &self.link.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The official create body, spec §9.5.
pub struct NewOrganization {
    pub name: String,
    pub billing_email: String,
    /// The org key wrapped to the owner's own public key (type 4).
    pub key: EncString,
    /// SPKI DER, base64.
    pub public_key: String,
    /// PKCS#8 under the org key (type 2).
    pub encrypted_private_key: EncString,
    /// The first person's email under the org key (type 2).
    pub collection_name: EncString,
}

// ---- the routes ---------------------------------------------------------------

/// `GET /api/users/{userId}/public-key` -- the key every confirm wraps to.
/// `None`: no such user, or a user who has never opened an app that made
/// keys.
pub fn user_public_key(
    client: &RestClient,
    session: &mut Session,
    user_id: &str,
) -> Result<Option<Vec<u8>>, RestError> {
    let path = format!("/api/users/{}/public-key", safe(user_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Get, path, None)) {
        Ok(answer) => match text_at(&answer, "publicKey").or_else(|| text_at(&answer, "key")) {
            Some(key) => Ok(Some(spki_from_base64(&key).map_err(|_| RestError::Parse("a public key"))?)),
            None => Ok(None),
        },
        Err(RestError::Status(404)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET /api/config` -- the server's own name, `server.name`. Vaultwarden
/// and NodeWarden fill it; official Bitwarden sends `null`, which is `None`
/// (official-path spec §3.4). Read once a session, for one sentence.
pub fn server_name(client: &RestClient, session: &mut Session) -> Result<Option<String>, RestError> {
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Get, "/api/config".into(), None))?;
    Ok(answer.get("server").and_then(|server| text_at(server, "name")))
}

/// `POST /api/organizations`, the official body. Returns the new org's id.
pub fn create_organization(
    client: &RestClient,
    session: &mut Session,
    body: &NewOrganization,
) -> Result<String, RestError> {
    let json = json!({
        "name": body.name,
        "billingEmail": body.billing_email,
        "planType": 0,
        "key": body.key.to_string(),
        "keys": {
            "publicKey": body.public_key,
            "encryptedPrivateKey": body.encrypted_private_key.to_string(),
        },
        "collectionName": body.collection_name.to_string(),
    });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, "/api/organizations".into(), Some(json)))?;
    text_at(&answer, "id").ok_or(RestError::Parse("the new organisation's id"))
}

/// `GET /api/organizations/{id}/collections` -- every collection of the org,
/// for its manager. Names are ciphertext.
pub fn organization_collections(
    client: &RestClient,
    session: &mut Session,
    org: &str,
) -> Result<Vec<RawCollection>, RestError> {
    let path = format!("/api/organizations/{}/collections", safe(org)?);
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Get, path, None))?;
    Ok(rows(&answer)
        .iter()
        .filter_map(|row| Some(RawCollection { id: text_at(row, "id")?, name: text_at(row, "name")? }))
        .collect())
}

/// `POST /api/organizations/{id}/collections` with the sharer as its one
/// manager (spec §5.2, §7.3). Returns the collection's id.
pub fn create_collection(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    name: &EncString,
    manager_ou_id: &str,
) -> Result<String, RestError> {
    let path = format!("/api/organizations/{}/collections", safe(org)?);
    let json = json!({
        "name": name.to_string(),
        "externalId": null,
        "groups": [],
        "users": [{ "id": manager_ou_id, "readOnly": false, "hidePasswords": false, "manage": true }],
    });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json)))?;
    text_at(&answer, "id").ok_or(RestError::Parse("the new collection's id"))
}

/// `DELETE /api/organizations/{id}/collections/{cid}`. Already gone is done.
pub fn delete_collection(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    collection: &str,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/collections/{}", safe(org)?, safe(collection)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `GET /api/organizations/{id}/collections/{cid}/items` -- the id of every
/// cipher filed in one collection, **trashed and archived ones included**:
/// NodeWarden reads its `collection_items` links, not a vault view, and the
/// owner (who can reach every collection) is allowed. The daemon's own copy
/// holds no trash or archive, so this is the only list of what a collection
/// really holds. Answered `{data: [{id, collectionId}]}`.
///
/// **An answer that is not a list, or a row with no id, is an error --
/// never fewer ids.** `forget` deletes the collection once every listed
/// record has stopped; a record missed here would stay in the org under a
/// collection that no longer exists, shared with nobody and never coming
/// home. `organizations::map_members`'s rule, for the same reason.
pub fn collection_items(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    collection: &str,
) -> Result<Vec<String>, RestError> {
    let path = format!("/api/organizations/{}/collections/{}/items", safe(org)?, safe(collection)?);
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Get, path, None))?;
    let listed = match answer.get("data") {
        Some(Value::Array(rows)) => rows.clone(),
        _ => answer.as_array().cloned().ok_or(RestError::Parse("the collection's items"))?,
    };
    listed.iter().map(|row| text_at(row, "id").ok_or(RestError::Parse("an item id in the collection"))).collect()
}

/// `POST /api/organizations/{id}/users/invite` -- Bitwarden's invite, with
/// the official body of the official-path spec §3.2: one email, the User
/// type, read-only access to the person's own collection, no groups, no
/// custom permissions, no Secrets Manager.
///
/// **Answered with an empty 200 on every server**, and the answer says
/// nothing: not the new org-user id, not whether the address has an account,
/// not whether the server accepted it at once (Vaultwarden and NodeWarden
/// with mail off do, for an existing account). The caller reads the roster
/// after it for all three. An address already in the org is skipped by the
/// server, silently, so a two-PC race is a success here too.
pub fn invite_user(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
    collection_id: &str,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/invite", safe(org)?);
    let json = json!({
        "emails": [email],
        "type": 2,
        "collections": [{ "id": collection_id, "readOnly": true, "hidePasswords": false, "manage": false }],
        "groups": [],
        "permissions": null,
        "accessSecretsManager": false,
    });
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json))).map(|_| ())
}

/// `POST /api/organizations/{id}/users/{ouId}/confirm {key}`. "Already
/// confirmed" is another PC's confirm, and a success.
pub fn confirm_member(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    ou_id: &str,
    key: &EncString,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/confirm", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json!({ "key": key.to_string() })))) {
        Ok(_) => Ok(()),
        Err(e) if refusal(&e).is_some_and(|s| s.contains("already confirmed")) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations/{id}/users/{ouId}/reinvite` -- a new token and a
/// new 7 days for a pending invitation.
pub fn reinvite(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/reinvite", safe(org)?, safe(ou_id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, None)).map(|_| ())
}

/// `DELETE /api/organizations/{id}/users/{ouId}` -- a member or an invitation.
/// Already gone is done.
pub fn remove_user(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `DELETE /api/ciphers/{id}` -- Bitwarden's permanent delete, the second
/// half of moving a record back (base spec §7.3: "`POST /api/ciphers`
/// (personal) then `DELETE /api/ciphers/{orgId}`" on every server).
///
/// **The same route as `RestClient::hard_delete_cipher`**, sent through
/// sharing's one door and with sharing's 404 rule below. Both used to send
/// NodeWarden's own `DELETE /api/ciphers/{id}/delete`: official servers and
/// Vaultwarden have no such route, and the 404 they answered was read here
/// as "already gone", so the org copy stayed in the org, filed nowhere.
/// NodeWarden's bare route was a soft delete for a live cipher until it was
/// made Bitwarden's.
///
/// A 404 is done: the org copy is already gone. Matched on the status
/// itself -- a 403 is not "gone", and must stay an error so the move back is
/// retried rather than dropped.
pub fn delete_cipher(client: &RestClient, session: &mut Session, id: &str) -> Result<(), RestError> {
    let path = format!("/api/ciphers/{}", safe(id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `PUT /api/ciphers/{id}/share {cipher, collectionIds}` -- a personal record
/// into the org, id unchanged. Returns the server's copy (still ciphertext).
pub fn share_cipher(
    client: &RestClient,
    session: &mut Session,
    id: &str,
    cipher: &MappedCipher,
    collection_ids: &[String],
) -> Result<Value, RestError> {
    let path = format!("/api/ciphers/{}/share", safe(id)?);
    let json = json!({ "cipher": cipher.body().clone(), "collectionIds": collection_ids });
    send(client, session, &SharingRequest::new(SharingMethod::Put, path, Some(json)))
}

/// `PUT /api/ciphers/{id}/collections {collectionIds}` -- which people a
/// shared record reaches. Returns the server's copy.
pub fn set_cipher_collections(
    client: &RestClient,
    session: &mut Session,
    id: &str,
    collection_ids: &[String],
) -> Result<Value, RestError> {
    let path = format!("/api/ciphers/{}/collections", safe(id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Put, path, Some(json!({ "collectionIds": collection_ids }))))
}

/// `POST /api/organizations/{orgId}/users/{ouId}/accept {token}` -- Bitwarden's
/// accept, with the token an invitation link carries (official-path spec
/// §4.1). The server checks the token, and that the invitation's email is
/// the signed-in account's. `resetPasswordKey` is not sent: a sharing org has
/// no account-recovery policy that would ask for one. There is no decline --
/// Bitwarden has none; an invitation nobody answers expires. The body's copy
/// of the token is wiped when the request drops, at the end of the send
/// ([`SharingRequest`]'s `Drop`).
pub fn accept_invite(client: &RestClient, session: &mut Session, org: &str, ou_id: &str, token: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/accept", safe(org)?, safe(ou_id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json!({ "token": token })))).map(|_| ())
}

/// `POST /api/organizations/{id}/leave` -- the recipient's "stop receiving".
pub fn leave_organization(client: &RestClient, session: &mut Session, org: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/leave", safe(org)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, None)).map(|_| ())
}

/// `POST /api/organizations/{id}/users/{ouId}/registration-invite
/// {masterPasswordHash}` -- a registration code for ONE pending invitation,
/// for a NodeWarden administrator sharing with somebody who has no account
/// (owner decision Q3 (b)). Answered 201 with NodeWarden's invite shape.
///
/// **This route and not `/api/admin/invites`**, which mints a code for
/// anyone, for as long as it is asked to last. NodeWarden's per-invitation
/// route (`handleCreateRegistrationInviteForOrgUser`) ties the code to the
/// invitation instead: it expires with it, and the server refuses it for an
/// invitation whose address already has an account ("This person already has
/// an account"), for an expired one, for anyone but a confirmed manager of
/// the org who is also an active server admin (403), and without the right
/// master-password hash ("Invalid password"). The hash is in the body, never
/// a query.
pub fn registration_invite(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    ou_id: &str,
    master_password_hash: &str,
) -> Result<InviteCode, RestError> {
    let path = format!("/api/organizations/{}/users/{}/registration-invite", safe(org)?, safe(ou_id)?);
    let json = json!({ "masterPasswordHash": master_password_hash });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json)))?;
    Ok(InviteCode {
        code: text_at(&answer, "code").ok_or(RestError::Parse("the invite code"))?,
        link: text_at(&answer, "inviteLink"),
        expires_at: text_at(&answer, "expiresAt"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::Device;
    use crate::test_http::{Matcher, MockServer};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    fn granted(server: &mut MockServer) -> (RestClient, Session) {
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let session = client.password_grant("a@b.c", "HASH", &device).expect("the grant");
        (client, session)
    }

    fn an_enc() -> EncString {
        "4.AAAA".parse().expect("a type-4 string")
    }

    /// **The accept is Bitwarden's**: the link's org and org-user ids in the
    /// path, the token in the body and nowhere else; an id that is not
    /// path-safe never reaches the wire.
    #[test]
    fn an_accept_posts_the_links_token_to_the_official_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let accepted = server
            .mock("POST", format!("/api/organizations/{ORG}/users/ou-9/accept").as_str())
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" })))
            .with_body("")
            .expect(1)
            .create();
        accept_invite(&client, &mut session, ORG, "ou-9", "tok-1").expect("accepted");
        accepted.assert();
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(accept_invite(&client, &mut session, ORG, "../x", "t"), Err(RestError::UnsafeId));
        any.assert();
    }

    /// **The organisation create body is the official one** (spec 9.5),
    /// which NodeWarden accepts from slice 1: plaintext name, the org key
    /// wrapped to the owner (type 4), the key pair, and the first person's
    /// collection name.
    #[test]
    fn an_organisation_is_created_with_the_official_body_and_its_id_comes_back() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let created = server
            .mock("POST", "/api/organizations")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": "Shared by denis@example.com",
                "billingEmail": "denis@example.com",
                "planType": 0,
                "key": "4.AAAA",
                "keys": { "publicKey": "PUB", "encryptedPrivateKey": "4.AAAA" },
                "collectionName": "4.AAAA"
            })))
            .with_body(r#"{"id":"org-new","object":"organization"}"#)
            .expect(1)
            .create();
        let body = NewOrganization {
            name: "Shared by denis@example.com".into(),
            billing_email: "denis@example.com".into(),
            key: an_enc(),
            public_key: "PUB".into(),
            encrypted_private_key: an_enc(),
            collection_name: an_enc(),
        };
        assert_eq!(create_organization(&client, &mut session, &body).expect("created"), "org-new");
        created.assert();
    }

    /// **The sharer manages every person collection**, in the create body
    /// itself -- spec 5.2: official servers refuse a collection nobody
    /// manages, and show an owner only collections assigned to them.
    #[test]
    fn a_person_collection_is_created_with_the_sharer_as_its_manager() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let created = server
            .mock("POST", format!("/api/organizations/{ORG}/collections").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": "4.AAAA",
                "users": [{ "id": "user-1", "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body(r#"{"id":"c-new","object":"collection"}"#)
            .expect(1)
            .create();
        assert_eq!(create_collection(&client, &mut session, ORG, &an_enc(), "user-1").expect("made"), "c-new");
        created.assert();
    }

    /// **The invite is Bitwarden's, body and all** (official-path spec §3.2):
    /// one email, the User type, read-only access to one collection, and the
    /// rest of `OrganizationUserInviteRequestModel` spelled out. Answered with
    /// an empty 200 on every server, which is a success.
    #[test]
    fn an_invite_is_the_official_body_on_the_official_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let invited = server
            .mock("POST", format!("/api/organizations/{ORG}/users/invite").as_str())
            .match_body(Matcher::Json(serde_json::json!({
                "emails": ["anna@example.com"],
                "type": 2,
                "collections": [{ "id": "c-anna", "readOnly": true, "hidePasswords": false, "manage": false }],
                "groups": [],
                "permissions": null,
                "accessSecretsManager": false
            })))
            .with_body("")
            .expect(1)
            .create();
        invite_user(&client, &mut session, ORG, "anna@example.com", "c-anna").expect("invited");
        invited.assert();
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(invite_user(&client, &mut session, "../x", "anna@example.com", "c"), Err(RestError::UnsafeId));
        any.assert();
    }

    /// **A record leaves the org by Bitwarden's permanent delete**,
    /// `DELETE /api/ciphers/{id}` -- not NodeWarden's `/delete` suffix, which
    /// official servers and Vaultwarden do not have. Already gone is done; a
    /// refusal is not.
    #[test]
    fn a_purge_is_the_official_delete_and_a_404_is_done() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let deleted = server.mock("DELETE", "/api/ciphers/c1").with_status(204).expect(1).create();
        delete_cipher(&client, &mut session, "c1").expect("deleted");
        deleted.assert();
        server.mock("DELETE", "/api/ciphers/gone").with_status(404).create();
        delete_cipher(&client, &mut session, "gone").expect("already gone is done");
        server.mock("DELETE", "/api/ciphers/theirs").with_status(403).create();
        assert_eq!(delete_cipher(&client, &mut session, "theirs"), Err(RestError::Status(403)));
    }

    /// "Already confirmed" and a removal already made are the two-PC race
    /// (spec 13): success, not an error.
    #[test]
    fn a_race_another_pc_already_won_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("POST", format!("/api/organizations/{ORG}/users/anna-id/confirm").as_str())
            .with_status(400)
            .with_body(r#"{"error":"Already confirmed","error_description":"Already confirmed"}"#)
            .create();
        confirm_member(&client, &mut session, ORG, "anna-id", &an_enc()).expect("a race is a success");
        server.mock("DELETE", format!("/api/organizations/{ORG}/users/gone").as_str()).with_status(404).create();
        remove_user(&client, &mut session, ORG, "gone").expect("a 404 on a delete is a success");
    }

    /// `/config`'s `server.name` -- Vaultwarden's and NodeWarden's own name;
    /// official Bitwarden sends `null`, which is no name.
    #[test]
    fn the_servers_own_name_is_read_from_config() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("GET", "/api/config")
            .with_body(r#"{"object":"config","server":{"name":"Vaultwarden","url":"https://github.com/dani-garcia/vaultwarden"}}"#)
            .expect(1)
            .create();
        assert_eq!(server_name(&client, &mut session).expect("read").as_deref(), Some("Vaultwarden"));
        let mut official = crate::test_http::server();
        let (client, mut session) = granted(&mut official);
        official.mock("GET", "/api/config").with_body(r#"{"object":"config","server":null}"#).create();
        assert_eq!(server_name(&client, &mut session).expect("read"), None);
    }

    /// Any other refusal keeps the server's own sentence, whichever key it
    /// wrote it under.
    #[test]
    fn a_refusal_keeps_the_servers_sentence() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", "/api/ciphers/c1/collections")
            .with_status(400)
            .with_body(r#"{"message":"You do not have permission to edit this item"}"#)
            .create();
        let err = set_cipher_collections(&client, &mut session, "c1", &["a".into()]).expect_err("refused");
        assert_eq!(
            err,
            RestError::Rejected { error: String::new(), description: "You do not have permission to edit this item".into() }
        );
    }

    /// **The share body is `{cipher, collectionIds}`**, and the cipher is a
    /// mapped one -- nothing here can put a hand-built cipher on the wire.
    #[test]
    fn a_share_puts_the_mapped_cipher_and_the_collections_to_the_share_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let cipher = crate::rest::write::tests::a_mapped_cipher();
        let shared = server
            .mock("PUT", "/api/ciphers/c1/share")
            .match_body(Matcher::Json(serde_json::json!({ "cipher": cipher.body().clone(), "collectionIds": ["c-anna"] })))
            .with_body(r#"{"id":"c1","object":"cipherDetails"}"#)
            .expect(1)
            .create();
        let answer = share_cipher(&client, &mut session, "c1", &cipher, &["c-anna".into()]).expect("shared");
        shared.assert();
        assert_eq!(answer["id"], "c1");
    }

    /// An empty 200 -- NodeWarden's confirm, leave and reinvite -- is `Null`
    /// and a success, never "the answer was not JSON".
    #[test]
    fn an_empty_success_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server.mock("POST", format!("/api/organizations/{ORG}/leave").as_str()).with_body("").create();
        leave_organization(&client, &mut session, ORG).expect("left");
        server.mock("POST", format!("/api/organizations/{ORG}/users/ou-1/reinvite").as_str()).with_body("").create();
        reinvite(&client, &mut session, ORG, "ou-1").expect("reinvited");
    }

    /// **A collection's items are its ids, and only a list is one**: an
    /// answer that is not a list, or a row without an id, is an error rather
    /// than fewer ids -- `forget` deletes the collection after stopping what
    /// this listed.
    #[test]
    fn a_collections_items_are_read_as_ids_and_anything_else_is_an_error() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let path = format!("/api/organizations/{ORG}/collections/c-1/items");
        let listed = server
            .mock("GET", path.as_str())
            .match_header("Authorization", "Bearer AT-1")
            .with_body(r#"{"object":"list","data":[{"id":"s1","collectionId":"c-1","object":"collectionItem"},{"id":"s2","collectionId":"c-1"}],"continuationToken":null}"#)
            .expect(1)
            .create();
        assert_eq!(collection_items(&client, &mut session, ORG, "c-1").expect("a list"), ["s1", "s2"]);
        listed.assert();
        for body in [r#"{"message":"Not found"}"#, r#"{"data":[{"collectionId":"c-1"}]}"#] {
            server.mock("GET", path.as_str()).with_body(body).expect(1).create();
            assert!(matches!(collection_items(&client, &mut session, ORG, "c-1"), Err(RestError::Parse(_))), "{body}");
        }
        server.mock("GET", path.as_str()).with_body(r#"{"data":[]}"#).expect(1).create();
        assert!(collection_items(&client, &mut session, ORG, "c-1").expect("an empty list").is_empty());
    }

    /// An id that is not path-safe is refused before any request is made.
    #[test]
    fn an_unsafe_id_never_reaches_the_wire() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(leave_organization(&client, &mut session, "../x"), Err(RestError::UnsafeId));
        any.assert();
    }

    /// **The registration code a NodeWarden admin can mint, for ONE pending
    /// invitation** (owner decision Q3 (b)): the invitation's own route, the
    /// master-password hash in the body and never a query, and the answer
    /// read as NodeWarden's `toInviteResponse` writes it. An id that is not
    /// path-safe never reaches the wire.
    #[test]
    fn a_registration_invite_is_minted_for_one_invitation_with_the_hash_in_the_body() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let minted = server
            .mock("POST", format!("/api/organizations/{ORG}/users/inv-1/registration-invite").as_str())
            .match_body(Matcher::Json(serde_json::json!({ "masterPasswordHash": "HASH==" })))
            .with_status(201)
            .with_body(r#"{"code":"abc123","status":"active","createdBy":"user-1","usedBy":null,"createdAt":"2026-09-28T00:00:00.000Z","updatedAt":"2026-09-28T00:00:00.000Z","expiresAt":"2026-10-04T00:00:00.000Z","inviteLink":"https://vault.example/#/register?invite=abc123","object":"invite"}"#)
            .expect(1)
            .create();
        let code = registration_invite(&client, &mut session, ORG, "inv-1", "HASH==").expect("minted");
        minted.assert();
        assert_eq!(code.code, "abc123");
        assert_eq!(code.link.as_deref(), Some("https://vault.example/#/register?invite=abc123"));
        assert_eq!(code.expires_at.as_deref(), Some("2026-10-04T00:00:00.000Z"));

        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert!(matches!(registration_invite(&client, &mut session, "../x", "inv-1", "HASH=="), Err(RestError::UnsafeId)));
        assert!(matches!(registration_invite(&client, &mut session, ORG, "a/b", "HASH=="), Err(RestError::UnsafeId)));
        any.assert();
    }

    /// **An invite code never prints through `Debug`**: it is a credential
    /// for registering an account on the server, and the link carries it too.
    #[test]
    fn an_invite_codes_debug_never_prints_the_code_or_the_link() {
        let code = InviteCode {
            code: "c0de-secret".into(),
            link: Some("https://vault.example/#/register?invite=c0de-secret".into()),
            expires_at: Some("2026-10-04T00:00:00.000Z".into()),
        };
        let printed = format!("{code:?}");
        assert!(!printed.contains("c0de-secret"), "{printed}");
        assert!(printed.contains("2026-10-04"), "{printed}");
    }
}

//! **NodeWarden's sharing routes**, one typed function each -- the NodeWarden
//! column of spec §7.3, with the request and answer shapes of §10 and of the
//! routes NodeWarden already has (`/members/*`, `/invitations/*`,
//! `/admin/invites`).
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
use zeroize::Zeroizing;

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
pub struct SharingRequest {
    method: SharingMethod,
    path: String,
    body: Option<Value>,
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

/// Percent-encodes a query value. An email carries `@` and may carry `+`,
/// and a `+` left bare is a space to the server.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A server without this route: its whole answer is "not here".
fn route_absent(error: &RestError) -> bool {
    matches!(error, RestError::Status(404 | 405 | 501))
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

/// An invitation addressed to this account (`GET /invitations/mine`).
///
/// **No `Debug`**, because the token is what accepts it.
#[derive(Clone)]
pub struct IncomingInvitation {
    pub id: String,
    pub organization_id: String,
    /// Plaintext on every server; for a sharing org, `Shared by <email>`.
    pub organization_name: Option<String>,
    pub token: Zeroizing<String>,
    pub expires_at: Option<String>,
}

/// An account's id and public key, looked up by email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountKey {
    pub user_id: String,
    /// SPKI DER.
    pub public_key: Vec<u8>,
}

/// Which branch NodeWarden's invite took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteOutcome {
    /// An existing account became a member at once (direct membership).
    Member,
    /// No account: a pending invitation, with its own id.
    Invitation { id: String },
    /// Already a member or already invited -- another PC got there first.
    AlreadyThere,
}

/// A collection as the org's own listing answers it: the name still ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCollection {
    pub id: String,
    pub name: String,
}

/// A NodeWarden registration code, as `/api/admin/invites` answers it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCode {
    pub code: String,
    pub link: Option<String>,
    pub expires_at: Option<String>,
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

/// `GET /api/organizations/invitations/mine` -- NodeWarden's, and the dialect
/// probe's question. `None` is a server without the route (official,
/// Vaultwarden), which is an answer and not an error.
pub fn my_invitations(
    client: &RestClient,
    session: &mut Session,
) -> Result<Option<Vec<IncomingInvitation>>, RestError> {
    let request = SharingRequest::new(SharingMethod::Get, "/api/organizations/invitations/mine".into(), None);
    match send(client, session, &request) {
        Ok(answer) => Ok(Some(
            rows(&answer)
                .iter()
                .filter_map(|row| {
                    Some(IncomingInvitation {
                        id: text_at(row, "id")?,
                        organization_id: text_at(row, "organizationId")?,
                        organization_name: text_at(row, "organizationName"),
                        token: Zeroizing::new(text_at(row, "token")?),
                        expires_at: text_at(row, "expirationDate"),
                    })
                })
                .collect(),
        )),
        Err(e) if route_absent(&e) || matches!(e, RestError::Rejected { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET /api/users/{userId}/public-key` -- slice 1's second probe, and the
/// key every confirm wraps to. `None`: no such route, no such user, or a user
/// who has never opened an app that made keys.
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

/// What `GET /api/users/{userId}/public-key` said, for the dialect probe's
/// second question -- "are the slice-1 routes here?" (spec 7.1).
///
/// **Not [`user_public_key`]'s `Option`**, which folds three answers into
/// `None` because a confirm only wants the key. The probe needs the
/// difference: a 200 with no key in it is the route answering, and caching it
/// as "server too old" for the session would turn sharing off on a server
/// that has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// 200: the route is there, whether or not the answer carried a key.
    Answered { key: bool },
    /// 404: no such route -- OR, on NodeWarden, whose handler answers 404
    /// "User public key not found" for an account with no key pair, the route
    /// and a keyless account. The status alone cannot tell them apart; the
    /// caller decides with what it knows about the account.
    NotFound,
    /// 405 or 501: no such route, unambiguously ([`route_absent`]'s others).
    Absent,
}

/// `GET /api/users/{userId}/public-key`, answered as a [`KeyRoute`].
pub fn key_route(client: &RestClient, session: &mut Session, user_id: &str) -> Result<KeyRoute, RestError> {
    let path = format!("/api/users/{}/public-key", safe(user_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Get, path, None)) {
        Ok(answer) => Ok(KeyRoute::Answered {
            key: text_at(&answer, "publicKey").or_else(|| text_at(&answer, "key")).is_some(),
        }),
        Err(RestError::Status(404)) => Ok(KeyRoute::NotFound),
        Err(e) if route_absent(&e) => Ok(KeyRoute::Absent),
        Err(e) => Err(e),
    }
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

/// `GET /api/organizations/{id}/members/public-key?email=` -- whether the
/// account exists, and its key (spec §7.2, the direct-add accelerator).
pub fn member_key_by_email(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
) -> Result<Option<AccountKey>, RestError> {
    let path = format!("/api/organizations/{}/members/public-key?email={}", safe(org)?, query_value(email));
    match send(client, session, &SharingRequest::new(SharingMethod::Get, path, None)) {
        Ok(answer) => {
            let (Some(user_id), Some(key)) = (text_at(&answer, "userId"), text_at(&answer, "publicKey")) else {
                return Ok(None);
            };
            let public_key = spki_from_base64(&key).map_err(|_| RestError::Parse("a public key"))?;
            Ok(Some(AccountKey { user_id, public_key }))
        }
        Err(RestError::Status(404)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations/{id}/members/invite`: read-only access to one
/// collection, with the org key already wrapped when the account exists.
pub fn invite_member(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
    collection_id: &str,
    key: Option<&EncString>,
) -> Result<InviteOutcome, RestError> {
    let path = format!("/api/organizations/{}/members/invite", safe(org)?);
    let mut json = json!({
        "email": email,
        "role": "member",
        "collections": [{ "id": collection_id, "readOnly": true, "hidePasswords": false, "manage": false }],
    });
    if let (Some(key), Some(object)) = (key, json.as_object_mut()) {
        object.insert("encryptedOrgKey".into(), Value::String(key.to_string()));
    }
    match send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json))) {
        Ok(answer) => Ok(match answer.get("object").and_then(Value::as_str) {
            Some("organizationInvitation") => {
                InviteOutcome::Invitation { id: text_at(&answer, "id").unwrap_or_default() }
            }
            _ => InviteOutcome::Member,
        }),
        Err(e) if refusal(&e).is_some_and(|s| s.contains("already a member") || s.contains("already been invited")) => {
            Ok(InviteOutcome::AlreadyThere)
        }
        Err(e) => Err(e),
    }
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

/// `POST /api/organizations/invitations/accept {token}`.
pub fn accept_invitation(client: &RestClient, session: &mut Session, token: &str) -> Result<(), RestError> {
    let request = SharingRequest::new(
        SharingMethod::Post,
        "/api/organizations/invitations/accept".into(),
        Some(json!({ "token": token })),
    );
    send(client, session, &request).map(|_| ())
}

/// `POST /api/organizations/invitations/decline {token}`.
pub fn decline_invitation(client: &RestClient, session: &mut Session, token: &str) -> Result<(), RestError> {
    let request = SharingRequest::new(
        SharingMethod::Post,
        "/api/organizations/invitations/decline".into(),
        Some(json!({ "token": token })),
    );
    send(client, session, &request).map(|_| ())
}

/// `POST /api/organizations/{id}/leave` -- the recipient's "stop receiving".
pub fn leave_organization(client: &RestClient, session: &mut Session, org: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/leave", safe(org)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, None)).map(|_| ())
}

/// `POST /api/admin/invites {masterPasswordHash, expiresInHours}` -- a
/// registration code, for a NodeWarden administrator sharing with somebody
/// who has no account (owner decision Q3 (b)). Seven days, matching the org
/// invitation's own expiry.
pub fn mint_invite_code(
    client: &RestClient,
    session: &mut Session,
    master_password_hash: &str,
) -> Result<InviteCode, RestError> {
    let json = json!({ "masterPasswordHash": master_password_hash, "expiresInHours": 168 });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, "/api/admin/invites".into(), Some(json)))?;
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

    /// **The invitation list, and what its absence means.** A 200 list is
    /// NodeWarden; a 404 is a server without the route -- the dialect probe's
    /// whole question -- and is `None`, not an error.
    #[test]
    fn my_invitations_are_read_and_a_missing_route_is_none() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let listed = server
            .mock("GET", "/api/organizations/invitations/mine")
            .match_header("Authorization", "Bearer AT-1")
            .with_body(serde_json::json!({ "object": "list", "data": [{
                "id": "inv-1", "organizationId": ORG, "organizationName": "Shared by denis@example.com",
                "token": "tok-1", "expirationDate": "2026-10-04T00:00:00.000Z", "object": "organizationInvitation"
            }]}).to_string())
            .expect(1)
            .create();
        let got = my_invitations(&client, &mut session).expect("a list").expect("NodeWarden");
        listed.assert();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(got[0].token.as_str(), "tok-1");
        assert_eq!(got[0].expires_at.as_deref(), Some("2026-10-04T00:00:00.000Z"));

        let mut official = crate::test_http::server();
        let (client, mut session) = granted(&mut official);
        official.mock("GET", "/api/organizations/invitations/mine").with_status(404).create();
        assert!(my_invitations(&client, &mut session).expect("not an error").is_none());
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

    /// Direct membership (spec 7.2): the key lookup answers both "does the
    /// account exist" and "what is its key"; a 404 is "no account".
    #[test]
    fn an_accounts_key_is_looked_up_by_email_and_a_404_is_no_account() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let spki = crate::rest::crypto::tests::fixture_public_spki();
        let found = server
            .mock("GET", format!("/api/organizations/{ORG}/members/public-key?email=anna%40example.com").as_str())
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&spki) }).to_string())
            .expect(1)
            .create();
        let key = member_key_by_email(&client, &mut session, ORG, "anna@example.com").expect("ok").expect("found");
        found.assert();
        assert_eq!(key, AccountKey { user_id: "anna-id".into(), public_key: spki });

        server
            .mock("GET", format!("/api/organizations/{ORG}/members/public-key?email=nobody%40example.com").as_str())
            .with_status(404)
            .create();
        assert!(member_key_by_email(&client, &mut session, ORG, "nobody@example.com").expect("ok").is_none());
    }

    /// **The probe's reading of the own-key route keeps apart what
    /// `user_public_key` folds together**: a 200 is the route whether or not
    /// it carried a key, a 404 is ambiguous and says so, and a 405 or 501 is
    /// no route at all.
    #[test]
    fn the_key_route_tells_an_answer_without_a_key_from_no_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let spki = crate::rest::crypto::tests::fixture_public_spki();
        server
            .mock("GET", "/api/users/keyed/public-key")
            .with_body(serde_json::json!({ "userId": "keyed", "publicKey": crate::rest::crypto::tests::base64(&spki) }).to_string())
            .create();
        server.mock("GET", "/api/users/keyless/public-key").with_body(r#"{"userId":"keyless","publicKey":null}"#).create();
        server.mock("GET", "/api/users/missing/public-key").with_status(404).create();
        server.mock("GET", "/api/users/refused/public-key").with_status(405).create();
        server.mock("GET", "/api/users/unbuilt/public-key").with_status(501).create();
        let asked = |id: &str, session: &mut Session| key_route(&client, session, id).expect("an answer");
        assert_eq!(asked("keyed", &mut session), KeyRoute::Answered { key: true });
        assert_eq!(asked("keyless", &mut session), KeyRoute::Answered { key: false });
        assert_eq!(asked("missing", &mut session), KeyRoute::NotFound);
        assert_eq!(asked("refused", &mut session), KeyRoute::Absent);
        assert_eq!(asked("unbuilt", &mut session), KeyRoute::Absent);
    }

    /// NodeWarden's invite: read-only access to the one collection, and the
    /// org key already wrapped for an existing account. The answer's `object`
    /// says which of the two branches the server took.
    #[test]
    fn an_invite_carries_read_only_access_and_says_which_branch_the_server_took() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let member = server
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({
                "email": "anna@example.com",
                "role": "member",
                "collections": [{ "id": "c-anna", "readOnly": true, "hidePasswords": false, "manage": false }],
                "encryptedOrgKey": "4.AAAA"
            })))
            .with_body(r#"{"id":"anna-id","status":1,"object":"organizationMember"}"#)
            .expect(1)
            .create();
        let outcome = invite_member(&client, &mut session, ORG, "anna@example.com", "c-anna", Some(&an_enc())).expect("invited");
        member.assert();
        assert_eq!(outcome, InviteOutcome::Member);

        let mut other = crate::test_http::server();
        let (client, mut session) = granted(&mut other);
        other
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .match_request(|request| !request.utf8_lossy_body().contains("encryptedOrgKey"))
            .with_body(r#"{"id":"inv-9","status":0,"object":"organizationInvitation"}"#)
            .create();
        assert_eq!(
            invite_member(&client, &mut session, ORG, "carol@example.com", "c-carol", None).expect("invited"),
            InviteOutcome::Invitation { id: "inv-9".into() }
        );
    }

    /// "Already a member" and "already confirmed" are the two-PC race (spec
    /// 13): success, not an error.
    #[test]
    fn a_race_another_pc_already_won_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .with_status(400)
            .with_body(r#"{"error":"User is already a member","error_description":"User is already a member"}"#)
            .create();
        assert_eq!(
            invite_member(&client, &mut session, ORG, "anna@example.com", "c", None).expect("a race"),
            InviteOutcome::AlreadyThere
        );
        server
            .mock("POST", format!("/api/organizations/{ORG}/users/anna-id/confirm").as_str())
            .with_status(400)
            .with_body(r#"{"error":"Already confirmed","error_description":"Already confirmed"}"#)
            .create();
        confirm_member(&client, &mut session, ORG, "anna-id", &an_enc()).expect("a race is a success");
        server.mock("DELETE", format!("/api/organizations/{ORG}/users/gone").as_str()).with_status(404).create();
        remove_user(&client, &mut session, ORG, "gone").expect("a 404 on a delete is a success");
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

    /// **The registration code a NodeWarden admin can mint** (owner decision
    /// Q3 (b)): the master-password hash goes in the body, never a query.
    #[test]
    fn an_invite_code_is_minted_with_the_password_hash_in_the_body() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let minted = server
            .mock("POST", "/api/admin/invites")
            .match_body(Matcher::PartialJson(serde_json::json!({ "masterPasswordHash": "HASH==", "expiresInHours": 168 })))
            .with_status(201)
            .with_body(r#"{"code":"abc123","inviteLink":"https://vault.example/#/register?invite=abc123","expiresAt":"2026-10-04T00:00:00Z","object":"invite"}"#)
            .expect(1)
            .create();
        let code = mint_invite_code(&client, &mut session, "HASH==").expect("minted");
        minted.assert();
        assert_eq!(code.code, "abc123");
        assert_eq!(code.link.as_deref(), Some("https://vault.example/#/register?invite=abc123"));
    }
}

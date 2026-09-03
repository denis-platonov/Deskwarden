//! Sends over REST: the half `crate::send` does not have.
//!
//! **`crate::send` is not edited by this module and does not know it
//! exists.** The two meet at the result types -- `SendPlan`, `CreatedSend`,
//! `SendSummary`, `SendError` -- and the branch between them is in
//! `vault_window`'s `real_send_*` helpers. See
//! `docs/superpowers/specs/2026-08-30-sends-without-the-cli-design.md` for
//! why the `SendRunner` trait is not widened to admit this path: it is an
//! argv and a stdin body, and a runner behind it would have to parse its own
//! request back out of base64 and then synthesise a CLI-shaped answer for
//! this app's own parser.
//!
//! **Text Sends only.** `type` is `0` and `file` is `null`, which is what
//! `send::plan_to_invocation` already sends to the CLI -- so this is parity
//! with the other backend and not a subtraction from it.

use crate::rest::api::{
    GrantAbsence, MappedSendAccess, RestClient, RestError, SendGrantRefusal, Session,
};
use crate::rest::crypto::{decrypt, encrypt, CryptoError, EncString};
use crate::rest::send_crypto::{access_url, SendKey};
use crate::rest::sync::VaultKeys;
use crate::send::{validate_plan, SendClock, SendError, SendPlan, SendSummary};
use serde_json::{json, Value};

/// A server-ready Send body, and the key it was built with.
///
/// The key is kept because the caller needs it **after** the request: the
/// access URL is assembled from the server's `accessId` and this key, and the
/// key never comes back from the server in a form anyone but the account
/// holder can open. Dropping it and re-reading it from the response would be
/// a second unwrap of a value this process already has.
pub struct MappedSend {
    body: Value,
    key: SendKey,
}

impl MappedSend {
    pub(crate) fn body(&self) -> &Value {
        &self.body
    }

    pub(crate) fn key(&self) -> &SendKey {
        &self.key
    }
}

/// One plan, as the body of `POST /api/sends`.
///
/// The plan is validated first, with [`validate_plan`] -- the composer's own
/// rules, so a plan refused here is refused in the same sentence the form
/// would have used, and no key is generated for a request that will not be
/// made.
pub fn encrypt_plan(
    plan: &SendPlan,
    keys: &VaultKeys,
    now: &dyn SendClock,
) -> Result<MappedSend, SendError> {
    if let Some(problem) = validate_plan(plan) {
        return Err(SendError::Rejected(problem.to_string()));
    }
    build(plan, keys, now).map_err(crypto_failed)
}

/// The half that can fail cryptographically, split out so the `map_err` above
/// is one line and every `?` below is the same kind of failure.
fn build(plan: &SendPlan, keys: &VaultKeys, now: &dyn SendClock) -> Result<MappedSend, CryptoError> {
    let key = SendKey::fresh()?;
    let cipher_key = key.cipher_key()?;
    let body = json!({
        "type": 0,
        "name": encrypt(&cipher_key, plan.name.trim().as_bytes())?.to_string(),
        "notes": Value::Null,
        "key": key.wrapped_under(keys.user())?.to_string(),
        "text": {
            "text": encrypt(&cipher_key, plan.text.as_bytes())?.to_string(),
            "hidden": plan.hidden,
        },
        "file": Value::Null,
        "maxAccessCount": plan.max_access_count,
        "deletionDate": crate::send::deletion_date(plan.delete_in_days, now),
        "expirationDate": Value::Null,
        "password": plan.password.as_ref().map(|p| key.password_hash(p).to_string()),
        "emails": Value::Null,
        "disabled": false,
        "hideEmail": false,
    });
    Ok(MappedSend { body, key })
}

/// Every cryptographic failure on this path means the same thing to the user
/// and the same thing about the world: **nothing was sent.**
///
/// It is not [`SendError::is_ambiguous`], and that is the point of mapping it
/// here rather than at the call site: no request has been made, so no link
/// can exist, and sending the user to check their Sends list would be
/// alarming and wrong.
fn crypto_failed(_: CryptoError) -> SendError {
    SendError::Rejected("The Send could not be encrypted on this PC, so nothing was sent.".to_string())
}

// ---- the server's answers, in the screen's own vocabulary --------------------

/// Whether a failed operation could have left a **public link** behind.
///
/// Not a boolean at the call site: this is the distinction the whole module
/// is arranged around -- see [`SendError::is_ambiguous`] -- and a bare `true`
/// three lines from a `?` is how it gets passed the wrong way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ambiguity {
    /// A create: the request may have been served before the answer was lost.
    Ambiguous,
    /// A list or a revoke: a failure here published nothing.
    Safe,
}

/// One [`RestError`], in the vocabulary the Sends screen already speaks.
pub fn map_error(error: RestError, ambiguity: Ambiguity) -> SendError {
    match error {
        RestError::Transport(_) if ambiguity == Ambiguity::Ambiguous => SendError::TimedOut,
        RestError::Transport(_) => SendError::Offline,
        RestError::Unauthorized | RestError::NoRefreshToken => SendError::Locked,
        RestError::Rejected { ref description, .. } if !description.is_empty() => {
            SendError::Rejected(format!("Bitwarden would not do it: {description}"))
        }
        RestError::Parse(_) if ambiguity == Ambiguity::Ambiguous => SendError::CreatedButUnreadable,
        other => SendError::Rejected(format!("Bitwarden would not do it: {other}")),
    }
}

/// A field the answer must carry, or the row is not showable.
fn field<'a>(row: &'a Value, name: &str) -> Result<&'a str, SendError> {
    row.get(name).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| {
        SendError::Rejected(format!("Bitwarden's answer carried no `{name}` for this Send."))
    })
}

/// One server row, as a row of the Sends screen.
///
/// A row missing what a revoke or a link needs is a **failure**, not a short
/// list: `send::parse_send_list`'s rule, applied to the other backend so the
/// two screens cannot disagree about what is showable.
fn summary_from(row: &Value, keys: &VaultKeys, base: &str) -> Result<SendSummary, SendError> {
    let id = field(row, "id")?.to_string();
    let access_id = field(row, "accessId")?;
    let wrapped: EncString = field(row, "key")?
        .parse()
        .map_err(|_| SendError::Rejected("A Send's key could not be read.".to_string()))?;
    let key = SendKey::from_wrapped(&wrapped, keys.user())
        .map_err(|_| SendError::Rejected("A Send's key could not be unwrapped.".to_string()))?;

    let name = match row.get("name").and_then(Value::as_str) {
        // A Send with no name at all is shown as one rather than refused: the
        // id and the link are what a revoke needs, and both are present.
        None | Some("") => String::new(),
        Some(text) => {
            let sealed: EncString = text
                .parse()
                .map_err(|_| SendError::Rejected("A Send's name could not be read.".to_string()))?;
            let plain = decrypt(&key.cipher_key().map_err(|_| encryption_unreadable())?, &sealed)
                .map_err(|_| encryption_unreadable())?;
            String::from_utf8(plain.to_vec()).map_err(|_| encryption_unreadable())?
        }
    };

    Ok(SendSummary {
        id,
        name,
        access_url: access_url(base, access_id, &key),
        deletion_date: row
            .get("deletionDate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // Bitwarden's `SendType`: 0 text, 1 file. Anything this client does
        // not recognise is treated as "not a text Send" so the screen says
        // it was made somewhere else rather than offering to edit it.
        is_file: row.get("type").and_then(Value::as_i64).unwrap_or(0) != 0,
    })
}

fn encryption_unreadable() -> SendError {
    SendError::Rejected("A Send on the server could not be decrypted with this vault's key.".to_string())
}

/// `POST /api/sends`, then the link.
///
/// An answer without an `id` or an `accessId` is
/// [`SendError::CreatedButUnreadable`], verbatim the rule
/// `send::parse_created_send` applies: the Send exists and its link cannot be
/// shown, which is the one failure this module refuses to call either a
/// success or a clean failure.
pub fn create(
    client: &RestClient,
    session: &mut Session,
    keys: &VaultKeys,
    plan: &SendPlan,
    now: &dyn SendClock,
) -> Result<crate::send::CreatedSend, SendError> {
    let mapped = encrypt_plan(plan, keys, now)?;
    let answer = client
        .create_send(session, &mapped)
        .map_err(|e| map_error(e, Ambiguity::Ambiguous))?;

    let (Some(id), Some(access_id)) = (
        answer.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()),
        answer.get("accessId").and_then(Value::as_str).filter(|s| !s.is_empty()),
    ) else {
        return Err(SendError::CreatedButUnreadable);
    };

    Ok(crate::send::CreatedSend {
        id: id.to_string(),
        name: plan.name.trim().to_string(),
        access_url: access_url(client.base_url(), access_id, mapped.key()),
        deletion_date: answer
            .get("deletionDate")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| crate::send::deletion_date(plan.delete_in_days, now)),
    })
}

/// `GET /api/sends`, decrypted into the rows the screen shows.
pub fn list(
    client: &RestClient,
    session: &mut Session,
    keys: &VaultKeys,
) -> Result<Vec<SendSummary>, SendError> {
    let answer = client.fetch_sends(session).map_err(|e| map_error(e, Ambiguity::Safe))?;
    // `data` is the paged envelope Bitwarden's list routes use; a server that
    // answered a bare array is read too, rather than reported as empty.
    let rows = answer
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| answer.as_array())
        .ok_or_else(|| {
            SendError::Rejected("Bitwarden's answer was not a list of Sends.".to_string())
        })?;
    let base = client.base_url();
    rows.iter().map(|row| summary_from(row, keys, base)).collect()
}

/// `DELETE /api/sends/{id}`. **No key is needed**: a revoke names a record.
pub fn delete(client: &RestClient, session: &mut Session, id: &str) -> Result<(), SendError> {
    client.delete_send(session, id).map_err(|e| map_error(e, Ambiguity::Safe))
}

/// `pub` for the same reason [`crate::rest::crypto::tests`] is: `rest::api`
/// needs a `MappedSend` fixture, and a `MappedSend` has no constructor but
/// [`encrypt_plan`] -- which is the whole point of the type. Sharing the
/// plan fixture is a narrower door than a second constructor.
// ---- the three the vault window calls ---------------------------------------

/// The client and the live session for the account this process is serving.
///
/// Assembled per operation out of process facts rather than held: an account
/// switch replaces both, and a handle taken a frame earlier is a handle to
/// somebody else's vault. A missing credential is [`SendError::Locked`],
/// whose existing sentence is already the right one.
fn active_account() -> Result<(RestClient, crate::rest::api::Authenticated), SendError> {
    let login = crate::backend_policy::direct_rest_login().ok_or(SendError::Locked)?;
    let read = crate::backend_policy::direct_rest_credentials().ok_or(SendError::Locked)?;
    let authenticated = read().ok_or(SendError::Locked)?;
    Ok((RestClient::new(login.server_url), authenticated))
}

/// The user key, unwrapped from a fresh `/api/sync`.
///
/// **`sync_refreshing`, and the plain `sync` here was the whole of bug 2.**
/// The credential this runs on comes off disk through
/// [`crate::user_key_store::UserKeyStore::load`], whose own doc says what it
/// hands back: a [`crate::rest::api::Session`] that "holds no access token at
/// all and is already expired by construction", so that the first
/// authenticated request refreshes first. This was the one call in the Sends
/// path that could not refresh -- it took `&Session` -- so it went out with an
/// empty bearer, the server answered 401, `map_error` turned that into
/// [`SendError::Locked`], and the screen said the vault was locked on an
/// account whose vault was on screen behind it. Every other Send route
/// (`create_send`, `fetch_sends`, `delete_send`) already went through
/// `RestClient::refreshing`; this one did not, which is why a revoke worked
/// -- it needs no key and so never comes here -- and a list never could.
///
/// One extra round trip per create and per list, and the reason it is paid
/// rather than cached: [`VaultKeys::unwrap_from`] is the one place in this
/// crate that knows how to do this, and a second copy of the key held beside
/// the backend's own would be a second thing to invalidate on a lock. A
/// revoke pays nothing -- it needs no key at all.
fn vault_keys(
    client: &RestClient,
    authenticated: &mut crate::rest::api::Authenticated,
    ambiguity: Ambiguity,
) -> Result<VaultKeys, SendError> {
    let response = client
        .sync_refreshing(&mut authenticated.session)
        .map_err(|e| map_error(e, ambiguity))?;
    let profile = response
        .profile
        .as_ref()
        .ok_or_else(|| SendError::Rejected("Bitwarden's answer carried no profile.".to_string()))?;
    let (keys, _) = VaultKeys::unwrap_from(&authenticated.master_key, profile)
        .map_err(|_| SendError::Locked)?;
    Ok(keys)
}

/// Publishes one text Send for the active direct-REST account.
pub fn create_on_active_account(
    plan: &SendPlan,
    now: &dyn SendClock,
) -> Result<crate::send::CreatedSend, SendError> {
    // Validated before anything reaches the network, so a refused plan costs
    // no round trip and answers in the composer's own sentence.
    if let Some(problem) = validate_plan(plan) {
        return Err(SendError::Rejected(problem.to_string()));
    }
    let (client, mut authenticated) = active_account()?;
    // **`Safe`, not `Ambiguous`.** This is the sync that happens BEFORE the
    // create; a failure here published nothing, and reporting it as
    // `TimedOut` would send the user to hunt for a link that cannot exist.
    let keys = vault_keys(&client, &mut authenticated, Ambiguity::Safe)?;
    create(&client, &mut authenticated.session, &keys, plan, now)
}

/// Every Send this account has, as the rows the screen shows.
pub fn list_on_active_account() -> Result<Vec<SendSummary>, SendError> {
    let (client, mut authenticated) = active_account()?;
    let keys = vault_keys(&client, &mut authenticated, Ambiguity::Safe)?;
    list(&client, &mut authenticated.session, &keys)
}

/// Revokes one Send. **No sync and no key**: a revoke names a record.
pub fn delete_on_active_account(id: &str) -> Result<(), SendError> {
    let (client, mut authenticated) = active_account()?;
    delete(&client, &mut authenticated.session, id)
}

// ---- receiving a Send from a link -------------------------------------------
//
// The design is `docs/superpowers/specs/2026-08-30-receiving-a-send-design.md`,
// and its §1 and §2 are the investigation that makes the shape below
// non-obvious. In short: there is no stable anonymous-access route. The
// bearer route arrived in server `v2026.1.1`; the anonymous one was REMOVED in
// `v2026.8.0`; they overlapped for seven releases, and the official client
// probes for neither.

/// The whole receive, both routes, one answer.
///
/// # Why the token route is tried first
///
/// **The order is forced, not aesthetic.** The legacy route has no clean "I am
/// not here" signal: a server that has removed it answers `404`, and a server
/// that still has it answers `404` for a Send that is disabled, expired, past
/// its deletion date or out of accesses. Same status code, opposite meanings.
/// A client that probed the old route first would tell a user with a perfectly
/// good link that their Send is gone.
///
/// The grant, by contrast, has one: `unsupported_grant_type` is Duende's own
/// answer and is the only thing that string can mean, and a token endpoint is
/// something every Bitwarden server has. So the grant is asked first, and
/// **only** [`SendGrantRefusal::GrantAbsent`] falls through. Every other
/// answer -- password required, password invalid, e-mail required, send gone
/// -- means the grant exists and is talking about this Send.
///
/// The probe costs one extra request, and only against old servers.
///
/// # Errors
///
/// [`SendError::Rejected`] with the sentence to show, or [`SendError::Offline`]
/// for a transport failure. **Never [`SendError::TimedOut`]**: see
/// [`Ambiguity`] -- a receive publishes nothing, so there is no link for a
/// failure here to have left behind.
pub fn receive(
    client: &RestClient,
    link: &crate::rest::send_link::SendLink,
    password: Option<&str>,
) -> Result<zeroize::Zeroizing<String>, SendError> {
    // Hashed once, here, and handed to whichever route answers: it is the same
    // value in both eras (`SendKey::password_hash`), and deriving it twice
    // would be two hundred thousand PBKDF2 iterations for one request.
    let hash = password.map(|p| link.key().password_hash(p));

    let answer = match client.mint_send_access_token(link.access_id(), hash.as_deref().map(String::as_str))
    {
        Ok(token) => client
            .access_send_with_token(&token)
            .map_err(|e| map_error(e, Ambiguity::Safe))?,
        // **The one refusal that may fall back**, and it carries which of its
        // two causes fired because that decides what a `404` from the old
        // route means. See [`GrantAbsence`].
        Err(SendGrantRefusal::GrantAbsent(absence)) => {
            access_legacy(client, link, password, absence)?
        }
        Err(SendGrantRefusal::PasswordRequired) => return Err(needs_password()),
        Err(SendGrantRefusal::PasswordInvalid) => return Err(wrong_password()),
        Err(SendGrantRefusal::EmailRequired) => return Err(email_gated()),
        Err(SendGrantRefusal::SendGone) => return Err(gone()),
        Err(SendGrantRefusal::Other(e)) => return Err(map_error(e, Ambiguity::Safe)),
    };

    text_from(&answer, link.key())
}

/// The legacy anonymous route, and its answers in the same vocabulary.
///
/// `absence` is what the token endpoint did, and it is the whole of how a
/// `404` here is read -- the one place in this module where the meaning of a
/// status code depends on an earlier request.
fn access_legacy(
    client: &RestClient,
    link: &crate::rest::send_link::SendLink,
    password: Option<&str>,
    absence: GrantAbsence,
) -> Result<Value, SendError> {
    let access = MappedSendAccess::for_key(link.key(), password);
    match client.access_send_anonymously(link.access_id(), &access) {
        Ok(answer) => Ok(answer),
        // `401` on this route is the server asking for the share password --
        // `SendAccessResult.PasswordRequired`, which the controller throws an
        // `UnauthorizedAccessException` for. It is NOT the vault being locked,
        // which is what `map_error` would have made of it.
        Err(RestError::Unauthorized) => Err(needs_password()),
        // `404` is `SendAccessResult.Denied` on a server that has this route,
        // and "no such route" on one that does not. Which it is was decided
        // before this request was made.
        Err(RestError::Status(404)) => match absence {
            GrantAbsence::GrantUnknownToTokenEndpoint => Err(gone()),
            GrantAbsence::NoTokenEndpoint => Err(neither_route()),
        },
        // `400 "Invalid password."` -- the arm the server takes a deliberate
        // two-second delay before, which is why the receive deadline is sized
        // the way it is.
        Err(RestError::Rejected { ref description, .. })
            if description.to_lowercase().contains("invalid password") =>
        {
            Err(wrong_password())
        }
        // Every other `400` on this route is "could not locate send", which
        // the controller throws a `BadRequestException` for. It means the same
        // thing to the user as a denial.
        Err(RestError::Rejected { .. }) => Err(gone()),
        Err(other) => Err(map_error(other, Ambiguity::Safe)),
    }
}

/// One `SendAccessResponseModel`, as the text it carries.
///
/// **One parser for both eras, and that is deliberate.** The class is the same
/// on both routes -- the bearer route returns `new SendAccessResponseModel(send)`
/// exactly as the anonymous one did -- and two parsers for one shape is how
/// the two come to disagree about what a readable Send is.
fn text_from(
    answer: &Value,
    key: &SendKey,
) -> Result<zeroize::Zeroizing<String>, SendError> {
    // Bitwarden's `SendType`: 0 text, 1 file. `summary_from`'s rule, and the
    // same default: an absent `type` is read as a text Send rather than
    // refused, because that is what every text Send this app makes carries.
    if answer.get("type").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return Err(file_send());
    }
    let sealed = answer
        .get("text")
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            SendError::Rejected("That Send carried no text this app could read.".to_string())
        })?;
    let sealed: EncString = sealed
        .parse()
        .map_err(|_| SendError::Rejected("That Send's text could not be read.".to_string()))?;
    let cipher_key = key.cipher_key().map_err(|_| link_key_wrong())?;
    let plain = decrypt(&cipher_key, &sealed).map_err(|_| link_key_wrong())?;
    let text = String::from_utf8(plain.to_vec()).map_err(|_| link_key_wrong())?;
    Ok(zeroize::Zeroizing::new(text))
}

// ---- the sentences, each its own function so none can quietly become another

/// The share password is needed and was not given.
fn needs_password() -> SendError {
    SendError::Rejected("That Send is protected by a share password.".to_string())
}

/// The share password was given and was wrong.
fn wrong_password() -> SendError {
    SendError::Rejected("That share password is not right.".to_string())
}

/// The Send is not there to be read, for any of the reasons that amount to the
/// same thing to whoever holds the link.
fn gone() -> SendError {
    SendError::Rejected(
        "That Send is no longer available. It may have expired, been revoked, or reached the \
         number of times it could be opened."
            .to_string(),
    )
}

/// **An e-mail-gated Send, refused by name.**
///
/// It must not be reachable from [`gone`]'s sentence: this link is *live*, and
/// a user told it was dead would stop asking the sender for a working one.
fn email_gated() -> SendError {
    SendError::Rejected(
        "That Send asks the recipient to prove an e-mail address, which Deskwarden cannot do \
         yet. Open it in a browser instead."
            .to_string(),
    )
}

/// A file Send, refused by name rather than decrypted into nonsense.
fn file_send() -> SendError {
    SendError::Rejected(
        "That is a file Send, which Deskwarden cannot download yet. Open it in a browser \
         instead."
            .to_string(),
    )
}

/// **The server has neither route.** Not the link's fault, and the sentence
/// must not blame it -- see the design's §6, which is the case this app could
/// not settle from Bitwarden's source at all.
fn neither_route() -> SendError {
    SendError::Rejected(
        "This server does not offer Send links to this app. Open the link in a browser \
         instead."
            .to_string(),
    )
}

/// The answer arrived and the key in the link does not open it.
fn link_key_wrong() -> SendError {
    SendError::Rejected(
        "That Send could not be decrypted with the key in its link. Copy the whole link and \
         try again."
            .to_string(),
    )
}

/// Reads one Send from a link, for the active direct-REST account.
///
/// **No `VaultKeys` and no `/api/sync`, and no credential of any kind.** The
/// key is in the link -- that is what a Send *is* -- so unlike
/// [`create_on_active_account`] and [`list_on_active_account`] this pays no
/// sync round trip, and unlike all three of them it never reads
/// `backend_policy::direct_rest_credentials`. It needs the account only to
/// know which server is ours, which is the comparison
/// [`crate::rest::send_link::parse`] makes and refuses on.
pub fn receive_on_active_account(
    url: &str,
    password: Option<&str>,
) -> Result<zeroize::Zeroizing<String>, SendError> {
    let login = crate::backend_policy::direct_rest_login().ok_or(SendError::Locked)?;
    let client = RestClient::new(login.server_url);
    // Parsed against the client's own normalised base URL rather than the raw
    // configured string, so a trailing slash cannot make a link foreign.
    let link = crate::rest::send_link::parse(url, client.base_url())?;
    receive(&client, &link, password)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::send::FixedClock;
    use zeroize::Zeroizing;

    const NOW: FixedClock = FixedClock(1_786_408_997_148);

    pub fn keys() -> VaultKeys {
        crate::rest::sync::tests::keys_from_user(&[9u8; 64])
    }

    pub fn a_plan() -> SendPlan {
        SendPlan {
            name: "Wi-Fi password".to_string(),
            text: Zeroizing::new("correct-horse-battery-staple".to_string()),
            hidden: true,
            delete_in_days: 7,
            password: Some(Zeroizing::new("share-pw-9271".to_string())),
            max_access_count: Some(3),
        }
    }

    /// A text row the server could have sent, for the mappers below.
    fn text_row(keys: &VaultKeys) -> Value {
        let key = SendKey::from_bytes([5u8; 16]);
        let cipher_key = key.cipher_key().expect("derives");
        json!({
            "id": "send-1",
            "accessId": "acc-1",
            "name": encrypt(&cipher_key, b"Wi-Fi password").expect("encrypts").to_string(),
            "key": key.wrapped_under(keys.user()).expect("wraps").to_string(),
            "deletionDate": "2026-09-06T00:43:17.148Z",
            "type": 0,
        })
    }

    /// **The secret is in the body only as ciphertext.** The positive
    /// control is on the next line: the ciphertext field must be present and
    /// non-empty, so a body that was never built cannot pass the absence
    /// assertion.
    #[test]
    fn the_body_carries_the_text_encrypted_and_nowhere_in_the_clear() {
        let mapped = encrypt_plan(&a_plan(), &keys(), &NOW).expect("the plan maps");
        let body = mapped.body().to_string();

        assert!(
            !body.contains("correct-horse-battery-staple"),
            "the Send's body reached the request in the clear"
        );
        assert!(
            !body.contains("share-pw-9271"),
            "the share password reached the request in the clear"
        );
        assert!(
            !body.contains("Wi-Fi password"),
            "the name reached the request in the clear -- Bitwarden encrypts it"
        );

        let text = mapped.body()["text"]["text"].as_str().expect("a text field");
        assert!(text.starts_with("2."), "the text is not an AES-CBC-HMAC EncString: {text}");
        let name = mapped.body()["name"].as_str().expect("a name field");
        assert!(name.starts_with("2."), "the name is not an EncString: {name}");
    }

    /// Every non-secret field the server needs, and the two that say what
    /// kind of Send this is.
    #[test]
    fn the_body_says_it_is_a_text_send_that_expires_when_the_form_said() {
        let mapped = encrypt_plan(&a_plan(), &keys(), &NOW).expect("the plan maps");
        let body = mapped.body();

        assert_eq!(body["type"], 0, "a text Send is type 0");
        assert!(body["file"].is_null(), "a text Send carries no file object");
        assert_eq!(body["text"]["hidden"], true, "the hidden flag did not travel");
        assert_eq!(body["maxAccessCount"], 3);
        assert_eq!(body["disabled"], false);
        assert_eq!(
            body["deletionDate"].as_str().expect("a deletion date"),
            crate::send::deletion_date(7, &NOW),
            "the two backends must stamp the same instant in the same format"
        );
        assert!(
            body["key"].as_str().expect("a key").starts_with("2."),
            "the send key is not wrapped under the user key"
        );
        assert_eq!(
            body["password"].as_str().expect("a password hash").len(),
            44,
            "the password is not a 32-byte PBKDF2 digest"
        );
    }

    /// A plan with no password and no view cap sends explicit nulls, not
    /// absent keys -- the shape `send::plan_to_invocation` already sends, so
    /// the two backends put the same record on the same server.
    #[test]
    fn a_bare_plan_sends_nulls_rather_than_missing_keys() {
        let plan = SendPlan { password: None, max_access_count: None, ..a_plan() };
        let mapped = encrypt_plan(&plan, &keys(), &NOW).expect("the plan maps");
        assert!(mapped.body()["password"].is_null());
        assert!(mapped.body()["maxAccessCount"].is_null());
        // The control: the fields that DO have values still have them.
        assert!(mapped.body()["name"].is_string());
    }

    /// **Validation happens before any key is generated.** A refused plan
    /// must not consume a CSPRNG draw and must answer in the words the
    /// composer already shows.
    #[test]
    fn a_plan_the_composer_would_refuse_is_refused_here_too() {
        let empty = SendPlan { name: "  ".to_string(), ..a_plan() };
        assert_eq!(
            encrypt_plan(&empty, &keys(), &NOW).map(|_| ()),
            Err(SendError::Rejected("Give the Send a name.".to_string()))
        );
        // The control: the same plan with a name maps.
        assert!(encrypt_plan(&a_plan(), &keys(), &NOW).is_ok());
    }

    /// A row the server sends becomes a row the Sends screen can show: a
    /// decrypted name, and a link that carries the key.
    #[test]
    fn a_server_row_becomes_a_summary_with_a_working_link() {
        let keys = keys();
        let key = SendKey::from_bytes([5u8; 16]);
        let summary =
            summary_from(&text_row(&keys), &keys, "https://vault.example.com").expect("the row maps");
        assert_eq!(summary.name, "Wi-Fi password", "the name was not decrypted");
        assert_eq!(summary.id, "send-1");
        assert!(!summary.is_file, "type 0 is a text Send");
        assert_eq!(
            summary.access_url,
            format!("https://vault.example.com/#/send/acc-1/{}", key.fragment())
        );
    }

    /// **A file Send is listed, not hidden.** It cannot be created here, and
    /// the screen says so -- but a Send made on another client must still be
    /// revocable from this one, which is the whole reason `is_file` exists.
    #[test]
    fn a_file_send_is_listed_and_flagged() {
        let keys = keys();
        let mut row = text_row(&keys);
        row["type"] = json!(1);
        let summary = summary_from(&row, &keys, "https://vault.example.com").expect("maps");
        assert!(summary.is_file, "a type 1 Send is a file Send");
        // The control: the same mapper reports false for the text row.
        assert!(!summary_from(&text_row(&keys), &keys, "https://x").expect("maps").is_file);
    }

    /// A row missing what a revoke or a link needs is a **failure**, not a
    /// short list -- `send::parse_send_list`'s rule, applied to the other
    /// backend so the two screens cannot disagree about what is showable.
    #[test]
    fn a_row_without_an_access_id_is_a_failure_and_not_a_skip() {
        let keys = keys();
        let mut row = text_row(&keys);
        row.as_object_mut().expect("an object").remove("accessId");
        assert!(summary_from(&row, &keys, "https://x").is_err());
        // The control: with it back, the same row maps.
        assert!(summary_from(&text_row(&keys), &keys, "https://x").is_ok());
    }

    /// A row whose key belongs to somebody else's vault is a failure and not
    /// a blank name: a link built from a key this vault cannot unwrap opens
    /// nothing.
    #[test]
    fn a_row_this_vault_cannot_unwrap_is_a_failure() {
        let mine = keys();
        let theirs = crate::rest::sync::tests::keys_from_user(&[8u8; 64]);
        assert!(summary_from(&text_row(&theirs), &mine, "https://x").is_err());
        // The control: the same row under its own vault's key maps.
        assert!(summary_from(&text_row(&theirs), &theirs, "https://x").is_ok());
    }

    /// **A transport failure on a create is ambiguous.** The request may have
    /// reached the server, so a link may exist -- which is exactly what
    /// `SendError::TimedOut` means and what `is_ambiguous` gates the screen
    /// on. Reporting it as `Offline` would offer a plain "try again" over a
    /// link nobody knows about.
    #[test]
    fn a_create_that_never_got_an_answer_is_ambiguous() {
        assert_eq!(
            map_error(RestError::Transport("connection reset".to_string()), Ambiguity::Ambiguous),
            SendError::TimedOut
        );
        // The control: the same failure on a LIST is unambiguous -- a list
        // that did not happen created nothing.
        assert_eq!(
            map_error(RestError::Transport("connection reset".to_string()), Ambiguity::Safe),
            SendError::Offline
        );
        assert_eq!(map_error(RestError::Unauthorized, Ambiguity::Safe), SendError::Locked);
    }

    // ---- receiving a Send from a link ---------------------------------------

    const ACCESS_ID: &str = "an-invented-access-id";
    const SHARED_TEXT: &str = "the-invented-text-inside-the-send";

    /// The 16 bytes an invented link carries. Never a real key.
    fn a_link_key() -> SendKey {
        SendKey::from_bytes([6u8; 16])
    }

    /// A whole invented link on `base`, in the shape `access_url` builds.
    fn a_link(base: &str) -> String {
        access_url(base, ACCESS_ID, &a_link_key())
    }

    /// **One `SendAccessResponseModel`, used by BOTH routes' mocks.**
    ///
    /// One fixture rather than two is what makes
    /// [`both_routes_are_read_by_the_same_parser`] mean anything: the class is
    /// the same in both eras, so a test that fed each route its own fixture
    /// could not notice the two parsers drifting apart.
    fn a_send_answer() -> String {
        let cipher_key = a_link_key().cipher_key().expect("derives");
        let text = encrypt(&cipher_key, SHARED_TEXT.as_bytes()).expect("encrypts").to_string();
        let name = encrypt(&cipher_key, b"An invented Send").expect("encrypts").to_string();
        serde_json::json!({
            "id": ACCESS_ID,
            "type": 0,
            "name": name,
            "text": { "text": text, "hidden": true },
            "file": Value::Null,
            "expirationDate": Value::Null,
            "creatorIdentifier": Value::Null,
        })
        .to_string()
    }

    /// Mocks a grant that mints a token.
    fn mint_ok(server: &mut crate::test_http::Server) {
        server
            .mock("POST", "/identity/connect/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"SEND-AT-1","expires_in":300}"#)
            .create();
    }

    /// Mocks a grant that refuses with a named `send_access_error_type`.
    fn mint_refuses(server: &mut crate::test_http::Server, body: &str) {
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();
    }

    /// Mocks an OLD server: the token endpoint exists and does not know the
    /// grant. This is the answer -- and the ONLY 400 -- that may fall back.
    fn mint_is_unknown(server: &mut crate::test_http::Server) {
        mint_refuses(server, r#"{"error":"unsupported_grant_type"}"#);
    }

    /// **The happy path on a NEW server**: the grant, then the bare access
    /// path, then the text.
    ///
    /// The legacy route is mocked with `.expect(0)` so a probe that called it
    /// anyway fails HERE rather than silently costing a request against every
    /// modern server.
    #[test]
    fn a_server_that_mints_a_token_is_never_asked_the_old_route() {
        let mut server = crate::test_http::server();
        mint_ok(&mut server);
        let token_route = server
            .mock("POST", "/api/sends/access")
            .match_header("Authorization", "Bearer SEND-AT-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .expect(1)
            .create();
        let legacy = server
            .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .expect(0)
            .create();

        let client = RestClient::new(server.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("the link parses");
        let text = receive(&client, &link, None).expect("the Send is read");

        assert_eq!(&*text, SHARED_TEXT, "the text did not come out of the token route");
        token_route.assert();
        legacy.assert();
    }

    /// **The happy path on an OLD server**: the grant answers
    /// `unsupported_grant_type`, the legacy route answers, and the text that
    /// comes out is IDENTICAL to the one the new-server test got from the same
    /// fixture.
    ///
    /// That equality is the whole user-facing claim of the design's §2.4 --
    /// the same Send, the same text, the same screen, on either server -- and
    /// it is asserted rather than described.
    #[test]
    fn an_old_server_yields_the_same_text_through_the_fallback() {
        let mut server = crate::test_http::server();
        mint_is_unknown(&mut server);
        let legacy = server
            .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .match_header("Send-Id", ACCESS_ID)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("the link parses");
        let text = receive(&client, &link, None).expect("the Send is read");

        assert_eq!(&*text, SHARED_TEXT, "the fallback produced different text");
        legacy.assert();
    }

    /// **One parser for both eras.** The same fixture through both routes must
    /// produce byte-identical output -- the guard against the two paths
    /// growing separate parsers, which is how they would come to disagree
    /// about what a readable Send is.
    #[test]
    fn both_routes_are_read_by_the_same_parser() {
        let mut new_server = crate::test_http::server();
        mint_ok(&mut new_server);
        new_server
            .mock("POST", "/api/sends/access")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .create();
        let new_client = RestClient::new(new_server.url());
        let new_link =
            crate::rest::send_link::parse(&a_link(new_client.base_url()), new_client.base_url())
                .expect("parses");
        let through_token = receive(&new_client, &new_link, None).expect("the token route reads");

        let mut old_server = crate::test_http::server();
        mint_is_unknown(&mut old_server);
        old_server
            .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .create();
        let old_client = RestClient::new(old_server.url());
        let old_link =
            crate::rest::send_link::parse(&a_link(old_client.base_url()), old_client.base_url())
                .expect("parses");
        let through_legacy = receive(&old_client, &old_link, None).expect("the legacy route reads");

        assert_eq!(*through_token, *through_legacy, "the two routes read the same answer apart");
        // The control: the shared answer is not empty, so this is not two
        // parsers agreeing that a Send contains nothing.
        assert_eq!(&*through_token, SHARED_TEXT);
    }

    /// **A server with neither route is refused by name.**
    ///
    /// `404` at identity AND `404` at the legacy path is not "this Send is
    /// gone" -- it is "this server does not offer Send links to this app", and
    /// the sentence says so. This is the design's §6 case, which no Bitwarden
    /// source could settle.
    ///
    /// Two controls. The same fixture with the legacy route PRESENT succeeds,
    /// so this is about the server and not about the fixture. And an OLD
    /// server -- one whose token endpoint answered `unsupported_grant_type`,
    /// so it has an identity server -- gets the "gone" sentence for the same
    /// `404`, which is the distinction the whole arm exists for.
    #[test]
    fn a_server_that_speaks_neither_route_says_so_rather_than_blaming_the_link() {
        let mut neither = crate::test_http::server();
        neither.mock("POST", "/identity/connect/token").with_status(404).create();
        neither
            .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .with_status(404)
            .create();
        let client = RestClient::new(neither.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("parses");
        let refusal = receive(&client, &link, None).expect_err("neither route answers");

        assert_eq!(refusal, neither_route(), "a server with no routes blamed the link");
        assert_ne!(
            refusal,
            gone(),
            "a server with no Send routes was reported as a dead link, which sends the user to \
             ask for a new link that will fail in exactly the same way"
        );

        // **Control one:** the same fixture, on a server that HAS the legacy
        // route, is read.
        let mut has_legacy = crate::test_http::server();
        has_legacy.mock("POST", "/identity/connect/token").with_status(404).create();
        has_legacy
            .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .create();
        let client = RestClient::new(has_legacy.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("parses");
        assert_eq!(
            &*receive(&client, &link, None).expect("the legacy route reads"),
            SHARED_TEXT,
            "control: the fixture cannot be read even where the route exists"
        );

        // **Control two, and the reason `GrantAbsence` is not a bare flag:**
        // the SAME `404` from the legacy route means "gone" when the token
        // endpoint answered rather than 404'd.
        let mut old = crate::test_http::server();
        mint_is_unknown(&mut old);
        old.mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
            .with_status(404)
            .create();
        let client = RestClient::new(old.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("parses");
        assert_eq!(
            receive(&client, &link, None).expect_err("the Send is denied"),
            gone(),
            "an expired Send on an OLD server was reported as a server without Send links"
        );
    }

    /// **The password path, on both routes, from the same two inputs.**
    ///
    /// `401` (legacy) and `password_hash_b64_required` (grant) both mean
    /// "ask"; a wrong password on either says "wrong password" and never
    /// "gone". Four cases, one table -- and the two sentences are asserted
    /// unequal, so a later edit cannot quietly collapse them.
    #[test]
    fn a_password_protected_send_asks_once_and_names_a_wrong_password_on_both_routes() {
        assert_ne!(needs_password(), wrong_password(), "the two password sentences have merged");
        assert_ne!(wrong_password(), gone(), "a wrong password reads as a dead link");

        // The grant's two.
        for (body, expected) in [
            (r#"{"send_access_error_type":"password_hash_b64_required"}"#, needs_password()),
            (r#"{"send_access_error_type":"password_hash_b64_invalid"}"#, wrong_password()),
        ] {
            let mut server = crate::test_http::server();
            mint_refuses(&mut server, body);
            // `.expect(0)`: none of these may reach the old route. They all
            // mean the grant exists and is talking about this Send.
            let legacy = server
                .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
                .with_status(200)
                .with_body(a_send_answer())
                .expect(0)
                .create();
            let client = RestClient::new(server.url());
            let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
                .expect("parses");
            assert_eq!(
                receive(&client, &link, Some("an-invented-share-password"))
                    .expect_err("the grant refused"),
                expected,
                "{body} did not produce its own sentence"
            );
            legacy.assert();
        }

        // The legacy route's two, reached through an old server.
        for (status, body, expected) in [
            (401, String::new(), needs_password()),
            (400, r#"{"message":"Invalid password."}"#.to_string(), wrong_password()),
        ] {
            let mut server = crate::test_http::server();
            mint_is_unknown(&mut server);
            server
                .mock("POST", format!("/api/sends/access/{ACCESS_ID}").as_str())
                .with_status(status)
                .with_header("content-type", "application/json")
                .with_body(body.clone())
                .create();
            let client = RestClient::new(server.url());
            let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
                .expect("parses");
            assert_eq!(
                receive(&client, &link, Some("an-invented-share-password"))
                    .expect_err("the legacy route refused"),
                expected,
                "a {status} on the legacy route did not produce its own sentence"
            );
        }

        // **The control for all four:** the same fixture with a password that
        // the server accepts is read, so none of the above is passing because
        // a password-carrying receive always fails.
        let mut server = crate::test_http::server();
        mint_ok(&mut server);
        server
            .mock("POST", "/api/sends/access")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_send_answer())
            .create();
        let client = RestClient::new(server.url());
        let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
            .expect("parses");
        assert_eq!(
            &*receive(&client, &link, Some("an-invented-share-password")).expect("reads"),
            SHARED_TEXT
        );
    }

    /// **An e-mail-gated Send is not reported as a dead link.**
    ///
    /// Asserted as an inequality between the two sentences, so a later edit
    /// cannot quietly collapse them: this link is LIVE, and a user told it was
    /// dead stops asking the sender for a working one.
    #[test]
    fn an_email_gated_send_is_not_reported_as_a_dead_link() {
        for code in ["email_required", "email_and_otp_required"] {
            let mut server = crate::test_http::server();
            mint_refuses(&mut server, &format!(r#"{{"send_access_error_type":"{code}"}}"#));
            let client = RestClient::new(server.url());
            let link = crate::rest::send_link::parse(&a_link(client.base_url()), client.base_url())
                .expect("parses");
            let refusal = receive(&client, &link, None).expect_err("an e-mail-gated Send");
            assert_eq!(refusal, email_gated(), "{code} did not get the e-mail sentence");
            assert_ne!(refusal, gone(), "{code} was reported as a dead link");
        }
        assert_ne!(email_gated(), gone(), "the two sentences have merged");
        assert!(
            email_gated().user_message().contains("e-mail"),
            "the refusal does not say what it is asking for"
        );
    }

    /// A `type` that is not 0 is refused **by name** -- a file Send -- and not
    /// decrypted into nonsense.
    #[test]
    fn a_file_send_is_refused_in_its_own_words() {
        let mut answer: Value = serde_json::from_str(&a_send_answer()).expect("the fixture");
        answer["type"] = json!(1);
        assert_eq!(
            text_from(&answer, &a_link_key()).expect_err("a file Send"),
            file_send(),
            "a file Send was not refused by name"
        );
        assert_ne!(file_send(), gone(), "a file Send reads as a dead link");

        // The control: the SAME fixture as type 0 is read, so this is about
        // the type and not about the fixture.
        let text = serde_json::from_str(&a_send_answer()).expect("the fixture");
        assert_eq!(&*text_from(&text, &a_link_key()).expect("a text Send"), SHARED_TEXT);
    }

    /// **A receive can never report a link it might have published.**
    ///
    /// A receive publishes nothing, so `Ambiguity::Ambiguous` must be
    /// unreachable here: a transport failure is `Offline` and never
    /// `TimedOut`, whose sentence tells the reader to go and check a Sends
    /// list that is not theirs.
    ///
    /// Control: the same mapper WITH `Ambiguous` does give `TimedOut`, so this
    /// is about this call site and not about a mapper that lost the arm.
    #[test]
    fn a_receive_can_never_report_a_link_it_might_have_published() {
        // Port 1 on loopback: nothing listens, so this is a transport failure
        // and no request is answered.
        let client = RestClient::new("http://127.0.0.1:1");
        let link = crate::rest::send_link::parse(&a_link("http://127.0.0.1:1"), "http://127.0.0.1:1")
            .expect("parses");
        let failure = receive(&client, &link, None).expect_err("nothing is listening");
        assert_eq!(failure, SendError::Offline, "an unreachable server was not reported as one");
        assert_ne!(
            failure,
            SendError::TimedOut,
            "a receive claimed a Send may now be public. It publishes nothing"
        );
        assert!(!failure.is_ambiguous(), "a receive reported an ambiguous outcome");

        // The control: the mapper still HAS the ambiguous arm, so the
        // assertions above are about this call site.
        assert_eq!(
            map_error(RestError::Transport("connection reset".to_string()), Ambiguity::Ambiguous),
            SendError::TimedOut,
            "control: the mapper no longer produces `TimedOut` at all"
        );
    }

    /// A Send whose text the link's key does not open is a named failure and
    /// not empty text -- and it is not the "gone" sentence either.
    #[test]
    fn a_send_the_links_key_cannot_open_is_a_failure_and_not_empty_text() {
        let answer: Value = serde_json::from_str(&a_send_answer()).expect("the fixture");
        let someone_elses = SendKey::from_bytes([7u8; 16]);
        assert_eq!(
            text_from(&answer, &someone_elses).expect_err("the wrong key"),
            link_key_wrong()
        );
        // The control: the right key reads the same answer.
        assert_eq!(&*text_from(&answer, &a_link_key()).expect("the right key"), SHARED_TEXT);
    }

    /// **Every sentence this path can show is distinct.**
    ///
    /// Written as one test over the whole set rather than as pairs, because
    /// the failure being guarded against is two of them being made equal by an
    /// edit that was only trying to reword one -- and a pairwise test only
    /// covers the pairs somebody thought of.
    #[test]
    fn no_two_receive_refusals_say_the_same_thing() {
        let all = [
            ("needs_password", needs_password()),
            ("wrong_password", wrong_password()),
            ("gone", gone()),
            ("email_gated", email_gated()),
            ("file_send", file_send()),
            ("neither_route", neither_route()),
            ("link_key_wrong", link_key_wrong()),
        ];
        for (i, (name, left)) in all.iter().enumerate() {
            assert!(
                !left.user_message().is_empty(),
                "control: {name} has no sentence at all, so every comparison below is between \
                 empty strings"
            );
            for (other, right) in &all[i + 1..] {
                assert_ne!(left, right, "{name} and {other} say the same thing");
            }
        }
    }
}

/// **A stored credential really lists this account's Sends.**
///
/// The owner's second report: `could not list this account's Sends: Locked`,
/// one minute after 1,668 items of the same account's vault had been drawn on
/// screen from the same server.
///
/// # What was wrong, and it was not the installed environment
///
/// The obvious suspects were `backend_policy`'s pairing -- an environment
/// whose `direct` or `credentials` had gone missing after the sign-in. They
/// had not. `send_fetch_thread::real_send_list` only reaches
/// [`list_on_active_account`] at all when `backend_policy::selected()` answers
/// `DirectRest`, and `install_env` refuses that choice unless BOTH halves are
/// present -- so reaching this `Locked` is itself proof that the environment
/// was intact and paired.
///
/// The credential was the difference. The one a Send runs on comes off disk
/// through [`crate::user_key_store::UserKeyStore::load`], and that function's
/// own doc says what it hands back: a session that "holds no access token at
/// all and is already expired by construction", so that the first
/// authenticated request refreshes first. [`vault_keys`] was the one call in
/// the Sends path that could not refresh -- `RestClient::sync` takes
/// `&Session` -- so it went out with an empty bearer, took a 401, and
/// `map_error` turned that into [`SendError::Locked`]. Every other Send route
/// already went through `RestClient::refreshing`, which is why a revoke (no
/// key, so it never comes here) worked and a list never could.
///
/// # Why these use the STORE and not a hand-built session
///
/// Because a hand-built `Authenticated` carrying a live access token passes on
/// both sides of the fix, which is this crate's named defect class: a test
/// that passes because it never reached the thing it names. The bug is a
/// property of the credential production really reads, so these write one with
/// `UserKeyStore::save` and read it back through a
/// `backend_policy::Credentials` closure that is the same
/// `move || key_store.load()` that `main`'s `settle_the_vault_backend` and its
/// `child_process_backend_env` both install. The mock server and the scratch
/// directory are the only substitutes.
#[cfg(test)]
mod a_stored_credential_can_list_sends {
    use super::tests::keys;
    use super::*;
    use crate::rest::crypto::{MasterKey, MASTER_KEY_LEN};
    use crate::user_key_store::UserKeyStore;

    /// The 64 bytes behind `tests::keys`, so the profile served below unwraps
    /// to the very key the Send row was sealed under.
    const USER_KEY_BYTES: [u8; 64] = [9u8; 64];

    /// The master key the stored credential carries, and the one the profile
    /// is sealed to.
    fn master() -> MasterKey {
        MasterKey::from_bytes([0xA5; MASTER_KEY_LEN])
    }

    /// A `userkey.bin` in a scratch directory, written by the store's own
    /// `save` -- so what `load` returns is the real expired-by-construction
    /// session and not something this test shaped.
    fn a_stored_credential(dir: &std::path::Path) -> UserKeyStore {
        let store = UserKeyStore::new(dir.join("userkey.bin"));
        let saved = store
            .save(&crate::rest::api::Authenticated {
                session: crate::rest::api::Session::from_refresh_token(zeroize::Zeroizing::new(
                    "a-stored-refresh-token".to_string(),
                )),
                master_key: master(),
            })
            .expect("the scratch directory is writable");
        assert!(saved, "control: nothing was stored, so `load` would answer `None`");
        store
    }

    /// The `profile.key` a server sends: the user key of `tests::keys`, sealed
    /// under the stretched master key of [`master`].
    fn protected_user_key() -> String {
        crate::rest::crypto::tests::seal(&master().stretch(), &USER_KEY_BYTES)
    }

    /// One text Send as the server sends it, named so the assertion below can
    /// tell a decrypted row from an absent one.
    fn a_row(keys: &VaultKeys) -> Value {
        let key = SendKey::from_bytes([5u8; 16]);
        let cipher_key = key.cipher_key().expect("derives");
        json!({
            "id": "send-1",
            "accessId": "acc-1",
            "name": encrypt(&cipher_key, b"Wi-Fi password").expect("encrypts").to_string(),
            "key": key.wrapped_under(keys.user()).expect("wraps").to_string(),
            "deletionDate": "2026-09-06T00:43:17.148Z",
            "type": 0,
        })
    }

    /// Installs a `DirectRest` environment for `server_url` whose credential
    /// reader is `store`, runs `action`, and puts the environment back.
    fn with_the_stored_credential<T>(
        server_url: &str,
        store: UserKeyStore,
        action: impl FnOnce() -> T,
    ) -> T {
        fn never(
            _server_url: &str,
            _email: &str,
            _device_id: &str,
            _password: &[u8],
        ) -> Result<crate::rest::api::LoginOutcome, String> {
            Err("this fixture never logs in".to_string())
        }
        let _guard = crate::backend_policy::tests::hold_the_env_lock();
        assert!(
            crate::backend_policy::install_env(crate::backend_policy::BackendEnv {
                choice: crate::backend_policy::VaultBackendChoice::DirectRest,
                credentials: Some(std::sync::Arc::new(move || store.load())),
                direct: Some(crate::login_ui::DirectRestLogin {
                    server_url: server_url.to_string(),
                    email: "someone@example.com".to_string(),
                    device_id: "00000000-0000-0000-0000-000000000000".to_string(),
                    second_factor: crate::login_ui::SecondFactorSeam {
                        start: never,
                        ..crate::login_ui::PRODUCTION_SECOND_FACTOR
                    },
                    prompt: None,
                    adopt: std::sync::Arc::new(|_authenticated| {}),
                }),
            }),
            "the fixture environment was refused, so this test would have measured the \
             default backend rather than the one it names"
        );
        let answer = action();
        crate::backend_policy::uninstall_env();
        answer
    }

    /// **The whole path, end to end, on a credential that came off disk.**
    #[test]
    fn listing_sends_on_a_stored_credential_returns_the_rows() {
        let dir = crate::test_scratch::ScratchDir::new("sends-stored-credential");
        let store = a_stored_credential(dir.path());
        let mut server = crate::test_http::server();

        let refresh = server
            .mock("POST", "/identity/connect/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"a-fresh-access-token","expires_in":3600}"#)
            .expect(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync")
            .match_query("excludeDomains=true")
            .match_header("authorization", "Bearer a-fresh-access-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({ "profile": { "key": protected_user_key() } }).to_string())
            .expect(1)
            .create();
        let sends = server
            .mock("GET", "/api/sends")
            .match_header("authorization", "Bearer a-fresh-access-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({ "data": [a_row(&keys())] }).to_string())
            .expect(1)
            .create();

        let url = server.url();
        let listed = with_the_stored_credential(&url, store, list_on_active_account);

        let rows = listed.expect("a stored credential could not list this account's Sends");
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["Wi-Fi password"],
            "the rows came back but were not the ones the server sent, decrypted"
        );
        refresh.assert();
        sync.assert();
        sends.assert();
    }

    /// The control that says the test above is about the refresh and not about
    /// the mock server: with nothing stored, the reader answers `None` and the
    /// path refuses before any request at all.
    #[test]
    fn a_signed_out_account_is_locked_without_asking_the_server() {
        let dir = crate::test_scratch::ScratchDir::new("sends-no-credential");
        let store = UserKeyStore::new(dir.path().join("userkey.bin"));
        let mut server = crate::test_http::server();
        let nothing = server
            .mock("POST", "/identity/connect/token")
            .with_status(500)
            .expect(0)
            .create();

        let url = server.url();
        let listed = with_the_stored_credential(&url, store, list_on_active_account);

        assert_eq!(listed.map(|_| ()), Err(SendError::Locked));
        nothing.assert();
    }
}

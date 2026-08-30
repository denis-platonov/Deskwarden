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

use crate::rest::api::{RestClient, RestError, Session};
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
/// One extra round trip per create and per list, and the reason it is paid
/// rather than cached: [`VaultKeys::unwrap_from`] is the one place in this
/// crate that knows how to do this, and a second copy of the key held beside
/// the backend's own would be a second thing to invalidate on a lock. A
/// revoke pays nothing -- it needs no key at all.
fn vault_keys(
    client: &RestClient,
    authenticated: &crate::rest::api::Authenticated,
    ambiguity: Ambiguity,
) -> Result<VaultKeys, SendError> {
    let response =
        client.sync(&authenticated.session).map_err(|e| map_error(e, ambiguity))?;
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
    let keys = vault_keys(&client, &authenticated, Ambiguity::Safe)?;
    create(&client, &mut authenticated.session, &keys, plan, now)
}

/// Every Send this account has, as the rows the screen shows.
pub fn list_on_active_account() -> Result<Vec<SendSummary>, SendError> {
    let (client, mut authenticated) = active_account()?;
    let keys = vault_keys(&client, &authenticated, Ambiguity::Safe)?;
    list(&client, &mut authenticated.session, &keys)
}

/// Revokes one Send. **No sync and no key**: a revoke names a record.
pub fn delete_on_active_account(id: &str) -> Result<(), SendError> {
    let (client, mut authenticated) = active_account()?;
    delete(&client, &mut authenticated.session, id)
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
}

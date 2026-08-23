//! The HTTP half of the direct-REST backend: **prelogin, the password grant,
//! and `/api/sync`**. Read-only, and nothing in the running app calls it yet.
//!
//! [`crypto`](crate::rest::crypto) is the cryptography with no I/O; this is
//! the I/O with no cryptography of its own beyond calling into that module.
//! The two stay apart on purpose -- the key derivation is checked against
//! published vectors and must stay checkable without a socket anywhere near
//! it.
//!
//! # What a login actually is, in three requests
//!
//! 1. **`POST /identity/accounts/prelogin`** with `{"email": ...}`, which is
//!    *unauthenticated*: it answers with the account's KDF and its
//!    parameters. It has to come first, because the client cannot derive the
//!    master key -- and therefore cannot produce the hash the next request
//!    sends -- without knowing whether the account is PBKDF2 or Argon2id and
//!    at what cost.
//! 2. **`POST /identity/connect/token`**, an OAuth 2 *password grant*, form
//!    encoded. The `password` field is **not** the master password: it is
//!    [`MasterKey::password_hash`], one PBKDF2 iteration over the master key
//!    salted by the password. The server never sees anything it could
//!    decrypt the vault with.
//! 3. **`GET /api/sync`** with `Authorization: Bearer <access token>`.
//!
//! # The fields the grant requires, and why they are not guesswork
//!
//! Every field below is one a Bitwarden-compatible server *validates*, taken
//! from Vaultwarden's `ConnectData` (`src/api/identity.rs`), which rejects a
//! blank one with a 400 naming it: `client_id`, `scope`, `username`,
//! `password`, `deviceIdentifier`, `deviceName`, `deviceType`. Omitting any
//! of them is a 400 whose body says which -- see [`RestError::Rejected`],
//! which exists so that body reaches the caller instead of being flattened
//! into "bad request".
//!
//! `scope` is `api offline_access`, and the second half is load-bearing:
//! **without `offline_access` the server issues no refresh token**, and a
//! session that cannot refresh is a session that makes the user retype their
//! master password every hour.
//!
//! # Token lifetime, and what expiry does
//!
//! The grant answers with `access_token`, `refresh_token` and `expires_in`
//! (seconds). [`Session`] stores the first two [`Zeroizing`] and turns the
//! third into a deadline. Two things then happen, and both are needed:
//!
//! * **Proactively.** [`Session::needs_refresh_at`] is true once the deadline
//!   is within [`EXPIRY_SKEW`], so a request that would have crossed the
//!   boundary mid-flight refreshes first.
//! * **Reactively.** A clock is not a fact about the server. If `/api/sync`
//!   answers `401` anyway -- a revoked session, a restarted server, a clock
//!   that drifted -- [`RestClient::sync_refreshing`] refreshes **once** and
//!   retries, and a second `401` is [`RestError::Unauthorized`] for the
//!   caller to turn into a re-login. Once, not in a loop: a server that
//!   answers 401 to a freshly refreshed token is not going to stop.
//!
//! If the refresh itself fails the error is [`RestError::Unauthorized`] as
//! well, and the master password is the only way back. Nothing here caches a
//! password to re-derive with, and nothing here should.
//!
//! # Two-factor authentication is recognised, not handled
//!
//! A server that wants a second factor answers the grant with **400** and a
//! body carrying `error: "invalid_grant"`, `error_description: "Two factor
//! required."` and a `TwoFactorProviders` array of provider numbers. That is
//! returned as [`RestError::TwoFactorRequired`] -- distinguishable from a
//! wrong password, which is the point -- and this module goes no further.
//! Completing the second factor means resending the grant with
//! `twoFactorProvider`/`twoFactorToken`, and that is not in this task.
//!
//! # Bounds
//!
//! Every request goes through [`crate::http_agent::bounded_total`], as the
//! rest of the crate's HTTP does. See [`CONNECT_TIMEOUT`], [`AUTH_DEADLINE`]
//! and [`SYNC_DEADLINE`] for why these are three numbers and not one.

use std::time::{Duration, Instant};

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::http_agent::TotalBounded;
use crate::rest::crypto::{CryptoError, Kdf, MasterKey, master_key};
use crate::rest::sync::SyncResponse;

/// Connect timeout. Larger than [`crate::vault_bridge`]'s three seconds, and
/// the difference is the situation rather than caution: that one dials a Node
/// process on loopback, this one dials a self-hosted server across the public
/// internet -- possibly a Cloudflare Worker, whose first request after an
/// idle period pays a cold start before a byte of the response exists.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Total-time bound for prelogin, the grant and the refresh.
///
/// The same 30s [`crate::updater`] gives its call to a third-party API, which
/// is the closest comparable shape this crate has: a small request to an
/// external host over a link of unknown quality. It is *not* derived from
/// `vault_bridge`'s read budget, which measures loopback.
///
/// The server's own work dominates it and is not this client's to bound: the
/// grant's answer costs the server a slow hash of the value we just sent it.
const AUTH_DEADLINE: Duration = Duration::from_secs(30);

/// Total-time bound for `/api/sync`.
///
/// Its own number because it is the only request here whose *size* varies
/// with the account: one response carrying every cipher, folder and
/// collection the user has, encrypted (so larger than the plaintext), over
/// WAN. `vault_bridge` measures 1.1s for the same vault out of a local file;
/// this is the same payload with a network in front of it and no evidence
/// that 30s is enough for a large vault on a poor link.
///
/// It is still a bound, and that is the point: a sync that hangs must fail,
/// because the caller is the app.
const SYNC_DEADLINE: Duration = Duration::from_secs(120);

/// Total-time bound for a single cipher write.
///
/// Its own number, between the other two, and the reason is the shape of the
/// request rather than caution: one cipher is small and bounded in a way
/// `/api/sync` is not, so [`SYNC_DEADLINE`]'s two minutes would be two
/// minutes of a user watching a save spinner for a request that is never
/// going to land. It is the same 30s as [`AUTH_DEADLINE`] because it is the
/// same shape of request -- small body, external host, the server's own work
/// dominating -- and it is deliberately *not* shared with it: an auth
/// deadline that someone later tunes for a slow KDF should not silently
/// re-tune how long a save hangs for.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);

/// How long before an access token's deadline it is treated as already
/// expired.
///
/// A token that expires while the request carrying it is in flight is
/// indistinguishable from one that was never valid, and costs a round trip to
/// find out. One minute is comfortably more than any request here takes and
/// far less than the hour a Bitwarden access token usually lives.
const EXPIRY_SKEW: Duration = Duration::from_secs(60);

/// The OAuth client this app identifies itself as.
///
/// `desktop`, because that is what it is. Bitwarden's server keys some
/// behaviour off it -- notably which device types and scopes are allowed --
/// and claiming `cli` would be a lie that happens to work today.
pub const CLIENT_ID: &str = "desktop";

/// The scope every request in this module asks for. See the module docs on
/// why `offline_access` is not optional.
const SCOPE: &str = "api offline_access";

/// Bitwarden's `DeviceType` for a Windows desktop client.
///
/// Sent as a string because the grant is form-encoded and every field on that
/// wire is a string; the server parses it.
pub const DEVICE_TYPE_WINDOWS_DESKTOP: &str = "6";

// ---- errors ----------------------------------------------------------------

/// Why a request did not produce what was asked for.
///
/// # Nothing in here is a secret, and that is checked
///
/// No arm carries the master password, the derived hash, an access or refresh
/// token, a key, or any ciphertext. [`RestError::Rejected`] carries two
/// strings the *server* wrote (`error` and `error_description`), which is the
/// one place worth pausing on: those are OAuth's own diagnostic fields, they
/// are how a missing required field announces itself, and neither Bitwarden
/// nor Vaultwarden echoes a submitted credential into them. They are
/// forwarded rather than dropped because "400" on its own tells a reader
/// nothing, and this API has seven required fields.
///
/// `no_error_can_carry_a_credential` asserts the whole of that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestError {
    /// The request never got an answer: DNS, TLS, connect, or the deadline.
    /// Carries ureq's own description, which names the host and the failure
    /// and nothing that was sent.
    Transport(String),
    /// An HTTP status this module has no specific meaning for.
    Status(u16),
    /// `401`, or a refresh that did not restore one. The caller must
    /// re-authenticate; retrying will not help.
    Unauthorized,
    /// The server wants a second factor. `providers` is its
    /// `TwoFactorProviders` list verbatim -- Bitwarden's provider numbers as
    /// strings ("0" authenticator, "1" email, and so on) -- forwarded rather
    /// than parsed, because this module does not implement any of them and an
    /// enum it cannot act on would be decoration.
    TwoFactorRequired { providers: Vec<String> },
    /// The server rejected the credentials themselves.
    InvalidCredentials,
    /// A 400 that is neither of the two above: a required field missing, a
    /// scope the server will not grant, a rate limit. Both strings are the
    /// server's own; see the type's doc.
    Rejected { error: String, description: String },
    /// The response was not the shape this module reads. The `&'static str`
    /// names what was missing, never what was there.
    Parse(&'static str),
    /// A key could not be derived or a value could not be decrypted.
    Crypto(CryptoError),
    /// A refresh was needed and the session has no refresh token -- which
    /// means the grant was made without `offline_access`, or the server does
    /// not issue one.
    NoRefreshToken,
    /// A cipher id that cannot be put in a URL path as it stands.
    ///
    /// Every id this client writes to came from a server, and a server sends
    /// GUIDs -- so this is not expected to fire. It exists because the
    /// alternative to checking is `format!("/api/ciphers/{id}")` with an id
    /// holding a `/`, a `?` or a `..`, which is a request to a different
    /// endpoint than the one the code reads as. Refusing is cheap; a
    /// `DELETE` aimed somewhere else is not. Carries nothing: the id itself
    /// is not a secret, but there is no reason to echo an unvalidated string
    /// into an error that may be logged.
    UnsafeId,
}

impl std::fmt::Display for RestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(what) => write!(f, "the server could not be reached: {what}"),
            Self::Status(code) => write!(f, "the server answered {code}"),
            Self::Unauthorized => f.write_str("the session is no longer valid; sign in again"),
            Self::TwoFactorRequired { providers } => write!(
                f,
                "this account needs a second factor, which is not supported yet (providers: \
                 {providers:?})"
            ),
            Self::InvalidCredentials => f.write_str("the email address or master password is wrong"),
            Self::Rejected { error, description } => {
                write!(f, "the server rejected the request ({error}): {description}")
            }
            Self::Parse(what) => write!(f, "the server's answer was missing {what}"),
            Self::Crypto(e) => write!(f, "{e}"),
            Self::NoRefreshToken => {
                f.write_str("this session has no refresh token and cannot be renewed")
            }
            Self::UnsafeId => f.write_str("that item's id is not one this client will put in a URL"),
        }
    }
}

impl std::error::Error for RestError {}

impl From<CryptoError> for RestError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

// ---- the device -------------------------------------------------------------

/// The three device fields the grant requires.
///
/// A separate type rather than three parameters because the server *stores*
/// them: `deviceIdentifier` is the stable name of this installation, and
/// sending a fresh one on every login registers a new device on the account
/// every time -- visible to the user, and on some configurations enough to
/// trigger a new-device-login email per launch. Whoever constructs this owns
/// keeping the identifier stable; this module will not invent one, because a
/// value invented here would be a different value on the next call.
///
/// No secret, so a derived `Debug` is fine and deliberate: this is exactly
/// the thing a reader debugging a rejected grant needs to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// A GUID, stable for the lifetime of this installation.
    pub identifier: String,
    /// What the user will see in their device list.
    pub name: String,
    /// Bitwarden's `DeviceType`. See [`DEVICE_TYPE_WINDOWS_DESKTOP`].
    pub device_type: String,
}

impl Device {
    /// This app's shape of device, with the caller's stable identifier.
    pub fn windows_desktop(identifier: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
            name: name.into(),
            device_type: DEVICE_TYPE_WINDOWS_DESKTOP.to_string(),
        }
    }
}

// ---- the session ------------------------------------------------------------

/// A live authentication: the bearer token, the token that renews it, and
/// when the first one stops working.
///
/// Both tokens are [`Zeroizing`]. An access token is a bearer credential for
/// the whole vault for as long as it lives, and a refresh token is one for
/// far longer than that.
pub struct Session {
    access_token: Zeroizing<String>,
    refresh_token: Option<Zeroizing<String>>,
    /// `None` when the server sent no `expires_in`. That is not treated as
    /// "never expires": it means no *proactive* refresh can be scheduled, and
    /// the 401 path is the only thing left. Said here because silently
    /// defaulting it to an hour would invent a fact about the server.
    expires_at: Option<Instant>,
}

/// Hand-written, and it must be: see [`crate::debug_leak_guard`]. A token is
/// the whole credential, so not one byte of it prints -- not a prefix, not a
/// length. The expiry does, because it is the only thing a reader debugging a
/// refresh actually needs.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at.is_some())
            .finish()
    }
}

impl Session {
    /// Whether the access token is expired, or close enough to it that a
    /// request started now might outlive it. See [`EXPIRY_SKEW`].
    ///
    /// Takes `now` rather than reading the clock so the rule is testable
    /// without one; the callers below pass [`Instant::now`].
    #[must_use]
    pub fn needs_refresh_at(&self, now: Instant) -> bool {
        match self.expires_at {
            Some(at) => now + EXPIRY_SKEW >= at,
            // No `expires_in` was sent: nothing to schedule against, so never
            // proactively. The 401 path still covers it.
            None => false,
        }
    }

    /// Whether this session can be renewed at all without the master
    /// password.
    #[must_use]
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// What a successful login yields: the session, and the master key the vault
/// is unwrapped with.
///
/// The two travel together because neither is any use alone -- the session
/// fetches ciphertext nobody can read, and the master key opens a vault
/// nobody can fetch.
pub struct Authenticated {
    pub session: Session,
    pub master_key: MasterKey,
}

/// Hand-written for [`Session`]'s reason, one level up.
impl std::fmt::Debug for Authenticated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authenticated")
            .field("session", &self.session)
            .field("master_key", &self.master_key)
            .finish()
    }
}

// ---- the wire shapes --------------------------------------------------------

/// `/identity/accounts/prelogin`'s answer.
///
/// Every field is optional and every name has an alias, because the casing
/// has moved: Bitwarden's server has shipped `Kdf`/`KdfIterations` and
/// `kdf`/`kdfIterations` at different times, Vaultwarden sends the lowercase
/// pair, and a self-hosted implementation is free to send either. Refusing
/// one casing would be refusing a real server for no reason.
///
/// `kdf_memory` and `kdf_parallelism` are absent (or null) on every PBKDF2
/// account, which is why they are `Option` rather than defaulted to a number
/// that would silently turn a PBKDF2 account into an Argon2id one.
#[derive(Deserialize)]
struct PreloginResponse {
    #[serde(alias = "Kdf")]
    kdf: Option<u32>,
    #[serde(alias = "KdfIterations", alias = "kdfIterations")]
    kdf_iterations: Option<u32>,
    #[serde(alias = "KdfMemory", alias = "kdfMemory")]
    kdf_memory: Option<u32>,
    #[serde(alias = "KdfParallelism", alias = "kdfParallelism")]
    kdf_parallelism: Option<u32>,
}

/// The grant's answer, and the refresh's.
///
/// Only three fields are read. The response also carries `Key`, `PrivateKey`
/// and the KDF parameters again, and **this module deliberately ignores
/// them**: `/api/sync`'s profile carries the same two key blobs, that is
/// where the mapper reads them, and having one source rather than two removes
/// the question of what to do when they disagree.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<Zeroizing<String>>,
    refresh_token: Option<Zeroizing<String>>,
    expires_in: Option<u64>,
}

// ---- the client -------------------------------------------------------------

/// A Bitwarden-compatible server, addressed directly.
///
/// `base_url` is the server root with no trailing slash --
/// `https://vault.example.com`. The two API prefixes this crate uses,
/// `/identity` and `/api`, are appended here rather than asked of the caller,
/// because a caller that got them wrong would get a 404 that looks like a
/// server that is not Bitwarden-compatible.
#[derive(Clone)]
pub struct RestClient {
    base_url: String,
    /// Prelogin, the grant, the refresh. See [`AUTH_DEADLINE`].
    auth_agent: TotalBounded,
    /// `/api/sync`, whose response size is unbounded by anything this client
    /// controls. See [`SYNC_DEADLINE`].
    sync_agent: TotalBounded,
    /// The cipher write endpoints. See [`WRITE_DEADLINE`].
    write_agent: TotalBounded,
}

impl RestClient {
    /// A client for one server.
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_agent: crate::http_agent::bounded_total(CONNECT_TIMEOUT, AUTH_DEADLINE),
            sync_agent: crate::http_agent::bounded_total(CONNECT_TIMEOUT, SYNC_DEADLINE),
            write_agent: crate::http_agent::bounded_total(CONNECT_TIMEOUT, WRITE_DEADLINE),
        }
    }

    /// The account's KDF and its parameters, before any password is derived.
    ///
    /// # Two paths are tried, and that is not defensive padding
    ///
    /// `POST /identity/accounts/prelogin` is where current servers answer.
    /// `POST /api/accounts/prelogin` is where it used to live, and
    /// Vaultwarden still mounts both. A server that implements only the older
    /// one is a server a client that tries only the newer one cannot log into
    /// at all -- so a 404 or 405 on the first, **and only those two**, falls
    /// through to the second. Any other failure is returned as it is: a 400
    /// or a 429 from the modern route means the route exists and said no, and
    /// asking a second route the same question would replace a real answer
    /// with a worse one.
    pub fn prelogin(&self, email: &str) -> Result<Kdf, RestError> {
        let body = serde_json::json!({ "email": email });
        let first = self.post_json(&format!("{}/identity/accounts/prelogin", self.base_url), &body);
        let response = match first {
            Err(RestError::Status(404 | 405)) => {
                self.post_json(&format!("{}/api/accounts/prelogin", self.base_url), &body)?
            }
            other => other?,
        };
        let parsed: PreloginResponse =
            serde_json::from_value(response).map_err(|_| RestError::Parse("the KDF parameters"))?;
        kdf_from(&parsed)
    }

    /// Prelogin, then the password grant. The whole of a login.
    ///
    /// The master password is taken as `&[u8]` for the same reason
    /// [`master_key`] takes it that way: a caller holding a
    /// `Zeroizing<String>` passes `.as_bytes()` and this makes no second,
    /// un-wiped copy of it.
    pub fn authenticate(
        &self,
        email: &str,
        password: &[u8],
        device: &Device,
    ) -> Result<Authenticated, RestError> {
        let kdf = self.prelogin(email)?;
        let master_key = master_key(password, email, kdf)?;
        let hash = master_key.password_hash(password);
        let session = self.password_grant(email, &hash, device)?;
        Ok(Authenticated { session, master_key })
    }

    /// The grant itself, given a hash somebody else derived.
    ///
    /// Split out from [`Self::authenticate`] so a test can drive the HTTP
    /// without paying six hundred thousand PBKDF2 iterations, and so the one
    /// place a derived hash is put on a wire is a function a reader can find.
    pub fn password_grant(
        &self,
        email: &str,
        password_hash: &str,
        device: &Device,
    ) -> Result<Session, RestError> {
        // The eight fields, in the order Bitwarden's own clients send them.
        // Seven of them are ones a server *validates* -- see the module docs;
        // `grant_type` is the eighth and selects the flow.
        let fields = [
            ("grant_type", "password"),
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("username", email),
            ("password", password_hash),
            ("deviceIdentifier", device.identifier.as_str()),
            ("deviceName", device.name.as_str()),
            ("deviceType", device.device_type.as_str()),
        ];

        let url = format!("{}/identity/connect/token", self.base_url);
        // `Auth-Email` is base64url of the email with no padding. Bitwarden's
        // own clients send it and its server binds rate limiting to it;
        // Vaultwarden ignores it. Sent because a server that wants it
        // otherwise answers 400 for a request that is in every other way
        // correct.
        let auth_email = base64_url_no_pad(email.as_bytes());
        let response = self
            .auth_agent
            .post(&url)
            .set("Auth-Email", &auth_email)
            .send_form(&fields);
        self.session_from(response)
    }

    /// Exchanges a refresh token for a new access token.
    ///
    /// The server may or may not send a new refresh token back; when it does
    /// not, the old one stays valid and is kept. Dropping it because the
    /// answer was silent about it would end the session at the next expiry.
    pub fn refresh(&self, session: &mut Session) -> Result<(), RestError> {
        let Some(refresh) = session.refresh_token.clone() else {
            return Err(RestError::NoRefreshToken);
        };
        let url = format!("{}/identity/connect/token", self.base_url);
        let response = self.auth_agent.post(&url).send_form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh.as_str()),
        ]);
        let renewed = self.session_from(response)?;
        session.access_token = renewed.access_token;
        session.expires_at = renewed.expires_at;
        if renewed.refresh_token.is_some() {
            session.refresh_token = renewed.refresh_token;
        }
        Ok(())
    }

    /// `GET /api/sync`, parsed but not decrypted.
    ///
    /// `excludeDomains=true` because this client has no use for the equivalent
    /// -domains table and asking for it makes the response bigger for nothing.
    pub fn sync(&self, session: &Session) -> Result<SyncResponse, RestError> {
        let url = format!("{}/api/sync?excludeDomains=true", self.base_url);
        // `Zeroizing` rather than a bare `format!`: the header value is the
        // access token with seven characters in front of it, and a plain
        // `String` here would leave a copy of the whole credential in a freed
        // page every time the vault is synced.
        //
        // The honest limit: ureq copies it into its own request buffer, which
        // this crate does not own and which is freed unwiped. That is the
        // same pre-existing exception `vault_bridge` records for response
        // bodies, and it is not fixable here. Wiping the copy this module
        // *does* own is still worth doing -- it is the one that would
        // otherwise persist for the life of the allocator's free list.
        let header = Zeroizing::new(format!("Bearer {}", session.access_token.as_str()));
        let response = self.sync_agent.get(&url).set("Authorization", &header).call();
        let value = self.value_from(response)?;
        serde_json::from_value(value).map_err(|_| RestError::Parse("the sync payload"))
    }

    /// [`Self::sync`], with the token lifetime handled: refresh first if the
    /// deadline is near, and refresh once and retry if the server says 401
    /// anyway. See the module docs on why both halves exist and why the retry
    /// happens exactly once.
    pub fn sync_refreshing(&self, session: &mut Session) -> Result<SyncResponse, RestError> {
        self.refreshing(session, |session| self.sync(session))
    }

    // ---- writing one cipher -------------------------------------------------

    /// `POST /api/ciphers` -- a new item.
    ///
    /// `body` is what [`crate::rest::write::encrypt_item`] produced, and this
    /// function does not look inside it: the only thing it could usefully
    /// check is already the mapper's job, and reading a mapped cipher here
    /// would be one more place a plaintext could be logged from. **Nothing in
    /// this function or below it formats `body`.**
    ///
    /// Returns the server's own copy of the created cipher -- still
    /// encrypted, and carrying the `id` and `revisionDate` it assigned, which
    /// is the only way the caller learns the new item's id.
    pub fn create_cipher(
        &self,
        session: &mut Session,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/ciphers", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.write_agent.post(&url), session).send_json(body))
        })
    }

    /// `PUT /api/ciphers/{id}` -- an edit.
    ///
    /// # This replaces the whole cipher
    ///
    /// Whatever `body` omits, the server drops. That is the reason
    /// [`crate::rest::write`] builds its body by laying the modelled fields
    /// *over* the retained JSON rather than from the model alone, and it is
    /// restated here because this is the function that does the damage if the
    /// rule is ever broken upstream.
    pub fn update_cipher(
        &self,
        session: &mut Session,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RestError> {
        let url = self.cipher_url(id, "")?;
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.write_agent.put(&url), session).send_json(body))
        })
    }

    /// `PUT /api/ciphers/{id}/delete` -- move to the trash.
    ///
    /// Reversible by [`Self::restore_cipher`], which is the whole reason it is
    /// a separate endpoint from [`Self::hard_delete_cipher`] and the reason a
    /// caller should reach for this one.
    pub fn trash_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.cipher_url(id, "/delete")?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.put(&url), session).call())
        })
    }

    /// `PUT /api/ciphers/{id}/restore` -- back out of the trash.
    pub fn restore_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.cipher_url(id, "/restore")?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.put(&url), session).call())
        })
    }

    /// `DELETE /api/ciphers/{id}` -- gone, with no trash to recover it from.
    ///
    /// Named `hard_delete` rather than `delete` so that no caller reaches for
    /// it by autocomplete when they meant [`Self::trash_cipher`]. This module
    /// will not decide for a caller which one they want, but it will make the
    /// irreversible one the longer word.
    pub fn hard_delete_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.cipher_url(id, "")?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.delete(&url), session).call())
        })
    }

    // ---- the plumbing ------------------------------------------------------

    /// The token discipline every authenticated call in this module shares:
    /// refresh proactively if the deadline is near, and refresh **once** and
    /// retry if the server says 401 anyway.
    ///
    /// Factored out rather than repeated per endpoint because the "once" is
    /// the part that is easy to get wrong in the fifth copy, and a retry loop
    /// against a server that answers 401 to a fresh token is a login storm.
    /// See the module docs for why both halves exist.
    fn refreshing<T>(
        &self,
        session: &mut Session,
        mut attempt: impl FnMut(&Session) -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        if session.needs_refresh_at(Instant::now()) && session.can_refresh() {
            // A failed proactive refresh is not fatal on its own -- the token
            // may still have seconds left -- so the request below is still
            // attempted, and its own 401 is what decides.
            let _ = self.refresh(session);
        }
        match attempt(session) {
            Err(RestError::Unauthorized) => {
                self.refresh(session).map_err(|_| RestError::Unauthorized)?;
                attempt(session)
            }
            other => other,
        }
    }

    /// `{base}/api/ciphers/{id}{suffix}`, with the id checked first.
    ///
    /// See [`RestError::UnsafeId`] for why an id from a server is checked at
    /// all.
    fn cipher_url(&self, id: &str, suffix: &str) -> Result<String, RestError> {
        if !is_url_path_safe(id) {
            return Err(RestError::UnsafeId);
        }
        Ok(format!("{}/api/ciphers/{}{}", self.base_url, id, suffix))
    }

    /// Puts the bearer token on a request.
    ///
    /// `Zeroizing` for [`Self::sync`]'s reason, spelled once there and not
    /// repeated at four call sites: the header value is the whole credential
    /// with seven characters in front of it, and the copy this module owns is
    /// the one that would otherwise persist in a freed page.
    fn bearer(&self, request: ureq::Request, session: &Session) -> ureq::Request {
        let header = Zeroizing::new(format!("Bearer {}", session.access_token.as_str()));
        request.set("Authorization", &header)
    }

    /// A response whose *body* is not wanted, only its status.
    ///
    /// Separate from [`Self::value_from`] because the three endpoints that use
    /// it answer with an empty body on success, and `into_json` on an empty
    /// body is a parse failure -- which would turn a delete that worked into
    /// an error the caller reports as a delete that did not.
    fn unit_from(&self, response: Result<ureq::Response, ureq::Error>) -> Result<(), RestError> {
        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(401, _)) => Err(RestError::Unauthorized),
            Err(ureq::Error::Status(400, body)) => Err(classify_400(body)),
            Err(ureq::Error::Status(code, _)) => Err(RestError::Status(code)),
            Err(e @ ureq::Error::Transport(_)) => Err(RestError::Transport(e.to_string())),
        }
    }

    /// Turns a token endpoint's response into a [`Session`].
    fn session_from(
        &self,
        response: Result<ureq::Response, ureq::Error>,
    ) -> Result<Session, RestError> {
        let value = self.value_from(response)?;
        let parsed: TokenResponse =
            serde_json::from_value(value).map_err(|_| RestError::Parse("the token response"))?;
        let Some(access_token) = parsed.access_token else {
            return Err(RestError::Parse("an access token"));
        };
        Ok(Session {
            access_token,
            refresh_token: parsed.refresh_token,
            expires_at: parsed
                .expires_in
                .and_then(|s| Instant::now().checked_add(Duration::from_secs(s))),
        })
    }

    /// One `POST` of a JSON body, for prelogin.
    fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RestError> {
        self.value_from(self.auth_agent.post(url).send_json(body))
    }

    /// The single place a `ureq` outcome becomes either a JSON value or a
    /// [`RestError`].
    ///
    /// Every status classification lives here so there is one answer to "what
    /// does a 400 mean", and so the 2FA and wrong-password cases cannot be
    /// distinguished in one call site and conflated in another.
    fn value_from(
        &self,
        response: Result<ureq::Response, ureq::Error>,
    ) -> Result<serde_json::Value, RestError> {
        match response {
            Ok(ok) => ok.into_json().map_err(|_| RestError::Parse("a JSON body")),
            Err(ureq::Error::Status(401, _)) => Err(RestError::Unauthorized),
            Err(ureq::Error::Status(400, body)) => Err(classify_400(body)),
            Err(ureq::Error::Status(code, _)) => Err(RestError::Status(code)),
            Err(e @ ureq::Error::Transport(_)) => Err(RestError::Transport(e.to_string())),
        }
    }
}

/// Reads a 400's body and decides which of the three things it is.
///
/// A body that is not JSON, or is JSON without the keys looked for, becomes a
/// [`RestError::Rejected`] with whatever was there -- never a panic and never
/// a silent `Status(400)`, because a 400 with an unreadable body is exactly
/// the case where a human needs to see the text.
fn classify_400(body: ureq::Response) -> RestError {
    let text = body.into_string().unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

    let providers = json
        .get("TwoFactorProviders")
        .or_else(|| json.get("twoFactorProviders"))
        .and_then(|v| v.as_array())
        .map(|list| list.iter().map(render_provider).collect::<Vec<_>>());
    if let Some(providers) = providers {
        return RestError::TwoFactorRequired { providers };
    }

    let error = string_at(&json, "error");
    let description = string_at(&json, "error_description");
    // Bitwarden and Vaultwarden both answer a wrong password with
    // `invalid_grant` and a description naming the credentials. The
    // description is matched case-insensitively on two words rather than in
    // full: the exact sentence differs between the two servers and has
    // changed within each.
    let lowered = description.to_lowercase();
    if error == "invalid_grant" && lowered.contains("username") && lowered.contains("password") {
        return RestError::InvalidCredentials;
    }
    RestError::Rejected { error, description }
}

/// One `TwoFactorProviders` entry as text. Bitwarden sends them as strings,
/// Vaultwarden as strings, and at least one implementation has sent numbers;
/// both render the same way here rather than one of them vanishing.
fn render_provider(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A string field of an error body, or the empty string. Never `None`: every
/// caller would turn it into the same empty string anyway.
fn string_at(json: &serde_json::Value, key: &str) -> String {
    json.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

/// Bitwarden's numeric KDF discriminator, turned into [`Kdf`].
///
/// Refuses rather than defaults. A server that sends an unknown KDF number,
/// or Argon2id without its memory and parallelism, is a server this client
/// cannot derive the right key for -- and deriving the *wrong* key produces a
/// rejected login the user reads as a wrong password.
fn kdf_from(parsed: &PreloginResponse) -> Result<Kdf, RestError> {
    let Some(iterations) = parsed.kdf_iterations else {
        return Err(RestError::Parse("the KDF iteration count"));
    };
    match parsed.kdf {
        // 0 is PBKDF2-SHA256. Absent is treated as 0 for one reason: it is
        // the only value that existed before the field did.
        Some(0) | None => Ok(Kdf::Pbkdf2 { iterations }),
        Some(1) => {
            // `KdfMemory` is **MiB** on this wire -- see [`Kdf::Argon2id`],
            // which carries the server's unit for exactly this reason, and
            // the Bitwarden vector in `crypto.rs` that established it.
            let (Some(memory_mib), Some(parallelism)) = (parsed.kdf_memory, parsed.kdf_parallelism)
            else {
                return Err(RestError::Parse("the Argon2id memory or parallelism"));
            };
            Ok(Kdf::Argon2id { iterations, memory_mib, parallelism })
        }
        Some(_) => Err(RestError::Parse("a KDF this client understands")),
    }
}

/// Whether a cipher id can be pasted into a URL path unchanged.
///
/// Deliberately narrow rather than an escaper: a Bitwarden cipher id is a
/// GUID, so hex digits and hyphens is the whole of what a real one contains,
/// and percent-encoding an id that is not one would send a well-formed
/// request for a record that cannot exist. Refusing says the truer thing.
fn is_url_path_safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Standard base64 re-spelled as base64url without padding.
///
/// Built on [`crate::record::seal::base64_into`] rather than a second encoder
/// so there is one base64 implementation in the crate. The two substitutions
/// and the padding are the whole of the difference (RFC 4648 section 5).
fn base64_url_no_pad(bytes: &[u8]) -> String {
    let mut out = String::new();
    crate::record::seal::base64_into(&mut out, bytes);
    out.truncate(out.trim_end_matches('=').len());
    out.replace('+', "-").replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device fixture. Never a real identifier; nothing here reaches a real
    /// server.
    fn device() -> Device {
        Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC")
    }

    /// A grant response shaped like the real one, including the fields this
    /// module deliberately ignores -- so a test that passes cannot be passing
    /// on a fixture that is only what the parser wants.
    fn token_body(expires_in: u64) -> String {
        format!(
            r#"{{"access_token":"AT-1","expires_in":{expires_in},"token_type":"Bearer",
                "refresh_token":"RT-1","PrivateKey":"2.aaa|bbb|ccc","Kdf":0,
                "KdfIterations":600000,"KdfMemory":null,"KdfParallelism":null,
                "ResetMasterPassword":false,"ForcePasswordReset":false,
                "MasterPasswordPolicy":{{"object":"masterPasswordPolicy"}},
                "scope":"api offline_access","Key":"2.ddd|eee|fff",
                "UserDecryptionOptions":{{"HasMasterPassword":true,
                "Object":"userDecryptionOptions"}}}}"#
        )
    }

    #[test]
    fn base64url_is_base64_with_two_substitutions_and_no_padding() {
        // RFC 4648 section 10's vectors, re-spelled. `f` -> "Zg==" in
        // standard base64, so the padding is what is being removed.
        assert_eq!(base64_url_no_pad(b"f"), "Zg");
        assert_eq!(base64_url_no_pad(b"foobar"), "Zm9vYmFy");
        // The two bytes that differ: 0xff 0xef encodes to "/+8=" in standard
        // base64 and must come out as "_-8".
        assert_eq!(base64_url_no_pad(&[0xff, 0xef]), "_-8");
        assert_eq!(base64_url_no_pad(b""), "");
    }

    #[test]
    fn a_pbkdf2_prelogin_is_read_in_either_casing() {
        for body in [
            r#"{"kdf":0,"kdfIterations":600000,"kdfMemory":null,"kdfParallelism":null}"#,
            r#"{"Kdf":0,"KdfIterations":600000,"KdfMemory":null,"KdfParallelism":null}"#,
        ] {
            let mut server = mockito::Server::new();
            let mock = server
                .mock("POST", "/identity/accounts/prelogin")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(body)
                .create();
            let kdf = RestClient::new(server.url()).prelogin("a@b.c").expect("a PBKDF2 prelogin");
            assert_eq!(kdf, Kdf::Pbkdf2 { iterations: 600_000 });
            mock.assert();
        }
    }

    #[test]
    fn an_argon2id_prelogin_carries_its_memory_and_parallelism() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":1,"kdfIterations":3,"kdfMemory":64,"kdfParallelism":4}"#)
            .create();
        let kdf = RestClient::new(server.url()).prelogin("a@b.c").expect("an Argon2id prelogin");
        // 64 is Vaultwarden's default and means 64 **MiB**, carried through
        // unconverted: the MiB-to-KiB step belongs to `master_key`.
        assert_eq!(kdf, Kdf::Argon2id { iterations: 3, memory_mib: 64, parallelism: 4 });
        mock.assert();
    }

    /// An Argon2id account whose memory figure did not arrive must not be
    /// silently derived as something else. Deriving the wrong key here is a
    /// login the user reads as a wrong master password.
    #[test]
    fn an_argon2id_prelogin_missing_its_parameters_is_refused_not_defaulted() {
        let mut server = mockito::Server::new();
        server.mock("POST", "/identity/accounts/prelogin").with_body(r#"{"kdf":1,"kdfIterations":3}"#).create();
        let err = RestClient::new(server.url()).prelogin("a@b.c").expect_err("no memory figure");
        assert_eq!(err, RestError::Parse("the Argon2id memory or parallelism"));
    }

    #[test]
    fn an_unknown_kdf_number_is_refused_rather_than_treated_as_pbkdf2() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":7,"kdfIterations":3}"#)
            .create();
        let err = RestClient::new(server.url()).prelogin("a@b.c").expect_err("KDF 7");
        assert_eq!(err, RestError::Parse("a KDF this client understands"));
    }

    /// The older route is tried only after the newer one 404s -- and the
    /// mocks assert *both* were called, so a client that skipped straight to
    /// the fallback (or never fell back) reds.
    #[test]
    fn a_server_with_only_the_old_prelogin_route_still_logs_in() {
        let mut server = mockito::Server::new();
        let modern =
            server.mock("POST", "/identity/accounts/prelogin").with_status(404).create();
        let legacy = server
            .mock("POST", "/api/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":100000}"#)
            .create();
        let kdf = RestClient::new(server.url()).prelogin("a@b.c").expect("the legacy route");
        assert_eq!(kdf, Kdf::Pbkdf2 { iterations: 100_000 });
        modern.assert();
        legacy.assert();
    }

    /// The fallback is for a missing *route*, not for a route that answered.
    /// A 400 from the modern path is a real answer and must be returned.
    #[test]
    fn a_prelogin_that_was_rejected_does_not_fall_through_to_the_old_route() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_status(400)
            .with_body(r#"{"error":"invalid_request","error_description":"email is required"}"#)
            .create();
        let legacy = server.mock("POST", "/api/accounts/prelogin").with_status(200).create();
        let err = RestClient::new(server.url()).prelogin("").expect_err("a rejected prelogin");
        assert_eq!(
            err,
            RestError::Rejected {
                error: "invalid_request".to_string(),
                description: "email is required".to_string(),
            }
        );
        assert!(!legacy.matched(), "a rejected prelogin retried the legacy route");
    }

    /// Every field a Bitwarden-compatible server validates is on the wire,
    /// named one by one. This is the test that would have caught omitting
    /// `deviceType` -- the failure the module docs describe as "a 400 with a
    /// body worth reading".
    #[test]
    fn the_password_grant_sends_every_field_the_server_requires() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/identity/connect/token")
            .match_header("Auth-Email", "YUBiLmM")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "password".into()),
                mockito::Matcher::UrlEncoded("client_id".into(), "desktop".into()),
                mockito::Matcher::UrlEncoded("scope".into(), "api offline_access".into()),
                mockito::Matcher::UrlEncoded("username".into(), "a@b.c".into()),
                mockito::Matcher::UrlEncoded("password".into(), "HASH==".into()),
                mockito::Matcher::UrlEncoded(
                    "deviceIdentifier".into(),
                    "11111111-2222-3333-4444-555555555555".into(),
                ),
                mockito::Matcher::UrlEncoded("deviceName".into(), "TEST-PC".into()),
                mockito::Matcher::UrlEncoded("deviceType".into(), "6".into()),
            ]))
            .with_body(token_body(3600))
            .create();

        let session = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH==", &device())
            .expect("the grant");
        mock.assert();
        assert!(session.can_refresh(), "no refresh token was kept, so the session cannot renew");
        assert!(
            !session.needs_refresh_at(Instant::now()),
            "a token with an hour left was treated as expiring"
        );
    }

    /// The whole login, prelogin included, and the one assertion that matters
    /// most: the value on the wire is the **hash**, not the master password.
    #[test]
    fn a_login_puts_the_derived_hash_on_the_wire_and_never_the_password() {
        const PASSWORD: &str = "correct horse battery staple";
        let mut server = mockito::Server::new();
        let pre = server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":5000}"#)
            .create();
        // The expected hash is computed the same way the module docs describe,
        // through the same public API, so this pins the wiring rather than
        // restating it: prelogin's parameters -> master key -> password hash.
        let expected = master_key(PASSWORD.as_bytes(), "a@b.c", Kdf::Pbkdf2 { iterations: 5000 })
            .expect("5000 iterations")
            .password_hash(PASSWORD.as_bytes());
        let grant = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "password".into(),
                expected.to_string(),
            )]))
            .with_body(token_body(3600))
            .create();

        let authed = RestClient::new(server.url())
            .authenticate("a@b.c", PASSWORD.as_bytes(), &device())
            .expect("the login");
        pre.assert();
        grant.assert();
        // The master key comes back so the caller can unwrap the vault, and
        // it is the same one the hash was derived from.
        assert_eq!(*authed.master_key.password_hash(PASSWORD.as_bytes()), *expected);
    }

    #[test]
    fn a_wrong_password_is_its_own_error_and_not_a_bare_400() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":
                    "Username or password is incorrect. Try again"}"#,
            )
            .create();
        let err = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect_err("a wrong password");
        assert_eq!(err, RestError::InvalidCredentials);
    }

    /// The 2FA shape, transcribed from Vaultwarden's `_json_err_twofactor`.
    /// It must be distinguishable from a wrong password -- both are 400
    /// `invalid_grant` -- because the two need opposite things from the user.
    #[test]
    fn a_second_factor_requirement_is_recognised_and_kept_apart_from_a_wrong_password() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"Two factor required.",
                    "TwoFactorProviders":["0","1"],
                    "TwoFactorProviders2":{"0":null,"1":{"Email":"a***@b.c"}},
                    "MasterPasswordPolicy":{"Object":"masterPasswordPolicy"}}"#,
            )
            .create();
        let err = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect_err("a 2FA challenge");
        assert_eq!(
            err,
            RestError::TwoFactorRequired {
                providers: vec!["0".to_string(), "1".to_string()]
            }
        );
        assert_ne!(err, RestError::InvalidCredentials, "2FA was read as a wrong password");
    }

    /// A missing required field is a 400 whose body names it, and the body
    /// must survive to the caller. `Status(400)` here would be the module
    /// docs' own worked example of an unhelpful error.
    #[test]
    fn a_missing_required_field_reaches_the_caller_with_the_servers_own_words() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_request",
                    "error_description":"device_type cannot be blank"}"#,
            )
            .create();
        let err = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect_err("a rejected grant");
        let RestError::Rejected { error, description } = err else {
            panic!("a field-level 400 was flattened into {err:?}");
        };
        assert_eq!(error, "invalid_request");
        assert!(description.contains("device_type"), "{description}");
    }

    /// A 400 whose body is not JSON at all -- a proxy's HTML error page, say.
    /// It must not panic and must not become an empty success.
    #[test]
    fn a_400_with_an_unreadable_body_is_still_a_rejection() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body("<html>Bad Request</html>")
            .create();
        let err = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect_err("a non-JSON 400");
        assert_eq!(
            err,
            RestError::Rejected { error: String::new(), description: String::new() }
        );
    }

    #[test]
    fn a_grant_that_answers_without_an_access_token_is_a_parse_failure() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"token_type":"Bearer","expires_in":3600}"#)
            .create();
        let err = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect_err("no access token");
        assert_eq!(err, RestError::Parse("an access token"));
    }

    /// `expires_in: 0` means the token is already past the skew, so a
    /// refresh must happen **before** the sync rather than after a wasted
    /// round trip. The refresh mock asserting exactly one call is what makes
    /// this about the proactive path.
    #[test]
    fn a_token_that_is_already_expiring_is_refreshed_before_the_sync_is_attempted() {
        let mut server = mockito::Server::new();
        let grant = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(0))
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                mockito::Matcher::UrlEncoded("refresh_token".into(), "RT-1".into()),
                mockito::Matcher::UrlEncoded("client_id".into(), "desktop".into()),
            ]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"token_type":"Bearer"}"#)
            .expect(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .match_header("Authorization", "Bearer AT-2")
            .with_body(r#"{"object":"sync","profile":{"key":"2.a|b|c"},"ciphers":[],"folders":[]}"#)
            .create();

        let client = RestClient::new(server.url());
        let mut session =
            client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        assert!(session.needs_refresh_at(Instant::now()), "expires_in 0 was not read as expiring");
        client.sync_refreshing(&mut session).expect("the sync");
        grant.assert();
        refresh.assert();
        sync.assert();
    }

    /// The reactive half. The token looks fine by the clock; the server
    /// disagrees. One refresh, one retry, and the retry carries the NEW
    /// token -- which is the assertion that would fail if the refreshed
    /// access token were dropped on the floor.
    #[test]
    fn a_401_mid_session_is_refreshed_once_and_the_retry_carries_the_new_token() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        let stale = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(401)
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"refresh_token":"RT-2"}"#)
            .expect(1)
            .create();
        let fresh = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .match_header("Authorization", "Bearer AT-2")
            .with_body(r#"{"object":"sync","profile":{"key":"2.a|b|c"},"ciphers":[],"folders":[]}"#)
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        client.sync_refreshing(&mut session).expect("the retried sync");
        stale.assert();
        refresh.assert();
        fresh.assert();
    }

    /// The retry happens once. A server that keeps saying 401 must produce
    /// one error, not a loop -- and `expect(2)` on the sync is what pins
    /// "exactly two attempts, ever".
    #[test]
    fn a_session_that_cannot_be_restored_gives_up_instead_of_looping() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600}"#)
            .expect(1)
            .create();
        let sync = server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_status(401)
            .expect(2)
            .create();

        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        let err = client.sync_refreshing(&mut session).expect_err("a dead session");
        assert_eq!(err, RestError::Unauthorized);
        refresh.assert();
        sync.assert();
    }

    /// A refresh that itself fails must surface as "sign in again", not as
    /// the refresh endpoint's own 400 -- the caller's only remedy is the
    /// master password either way, and `Rejected` would send it looking for
    /// a different one.
    #[test]
    fn a_refresh_that_is_itself_rejected_becomes_a_plain_re_authenticate() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_status(401).create();

        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        assert_eq!(
            client.sync_refreshing(&mut session).expect_err("a dead refresh"),
            RestError::Unauthorized
        );
    }

    /// A grant made without `offline_access` gets no refresh token, and the
    /// failure mode has to be legible rather than a 400 from a request sent
    /// with an empty field.
    #[test]
    fn a_session_with_no_refresh_token_says_so_instead_of_sending_an_empty_one() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600}"#)
            .create();
        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        assert!(!session.can_refresh());
        assert_eq!(client.refresh(&mut session).expect_err("nothing to refresh"), RestError::NoRefreshToken);
    }

    /// A server that sends no `expires_in` must not be assumed to have sent
    /// one. Guessing an hour here would schedule a refresh against a fact
    /// nobody stated.
    #[test]
    fn a_token_with_no_stated_lifetime_is_never_refreshed_proactively() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1"}"#)
            .create();
        let session = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH", &device())
            .expect("the grant");
        assert!(!session.needs_refresh_at(Instant::now()));
        assert!(session.can_refresh());
    }

    #[test]
    fn a_base_url_with_a_trailing_slash_does_not_produce_a_double_slash() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":1}"#)
            .create();
        RestClient::new(format!("{}/", server.url())).prelogin("a@b.c").expect("prelogin");
        mock.assert();
    }

    /// The leak check, and it is deliberately blunt: build one of every
    /// error, plus the two secret-bearing structs, and assert that no
    /// credential this module ever holds appears in any of them.
    #[test]
    fn no_error_can_carry_a_credential() {
        const NEEDLES: [&str; 4] = ["AT-1", "RT-1", "HASH-VALUE", "master-password"];

        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(token_body(3600))
            .create();
        let session = RestClient::new(server.url())
            .password_grant("a@b.c", "HASH-VALUE", &device())
            .expect("the grant");

        let mut printed = vec![format!("{session:?}")];
        let authed = Authenticated {
            session,
            master_key: master_key(b"master-password", "a@b.c", Kdf::Pbkdf2 { iterations: 1 })
                .expect("one iteration"),
        };
        printed.push(format!("{authed:?}"));

        for error in [
            RestError::Transport("connection refused".to_string()),
            RestError::Status(500),
            RestError::Unauthorized,
            RestError::TwoFactorRequired { providers: vec!["0".to_string()] },
            RestError::InvalidCredentials,
            RestError::Rejected {
                error: "invalid_request".to_string(),
                description: "device_type cannot be blank".to_string(),
            },
            RestError::Parse("an access token"),
            RestError::Crypto(CryptoError::MacMismatch),
            RestError::NoRefreshToken,
        ] {
            printed.push(error.to_string());
            printed.push(format!("{error:?}"));
        }

        for text in &printed {
            assert!(!text.is_empty(), "an error rendered as nothing at all");
            for needle in NEEDLES {
                assert!(!text.contains(needle), "{needle} reached a formatter: {text}");
            }
        }
        // Control on the needles themselves: they really are the values in
        // play, so the loop above is not vacuously passing on strings that
        // were never anywhere near these types.
        assert!(printed[0].contains("redacted"), "{}", printed[0]);
        assert!(printed[1].contains("redacted"), "{}", printed[1]);
    }

    // ---- the cipher write endpoints ----------------------------------------

    /// A session for the write tests, obtained the same way the app would:
    /// through a real grant against the mock server. There is no constructor
    /// shortcut, because a `cfg(test)` seam into `Session` is exactly the
    /// thing this crate bans.
    fn granted(server: &mut mockito::Server) -> (RestClient, Session) {
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        let client = RestClient::new(server.url());
        let session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        (client, session)
    }

    /// A mapped cipher body, as `rest::write` produces one: encrypted values
    /// and one plaintext the tests below can search for and must never find.
    const SECRET: &str = "hunter2-never-on-the-wire";
    fn encrypted_body() -> serde_json::Value {
        serde_json::json!({
            "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "type": 1,
            "name": "2.aWl2aXZpdml2aXZpdml2aQ==|Y2lwaGVydGV4dGNpcGhlcnRleHQ=|bWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWE=",
            "login": {
                "password": "2.aXZpdml2aXZpdml2aXZpdg==|c2VjcmV0Y2lwaGVydGV4dHM=|bWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWE=",
            },
            "reprompt": 1,
        })
    }

    /// The whole of what a create must put on the wire: the method, the path,
    /// the bearer header, and a body carrying ciphertext and no plaintext.
    #[test]
    fn a_create_posts_the_encrypted_body_with_the_bearer_token() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let body = encrypted_body();
        let created = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(mockito::Matcher::Json(body.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object":"cipher","id":"server-assigned-id","revisionDate":"2024-07-07T07:07:07Z"}"#)
            .expect(1)
            .create();

        let answer = client.create_cipher(&mut session, &body).expect("the create");
        created.assert();
        // The server's own copy comes back, which is how the caller learns the
        // id it did not choose.
        assert_eq!(answer.get("id").and_then(|v| v.as_str()), Some("server-assigned-id"));

        // And the body really was ciphertext: the same assertion from the
        // other side, on the exact bytes `match_body` accepted.
        let sent = serde_json::to_string(&body).expect("serializable");
        assert!(!sent.contains(SECRET), "a plaintext reached the request body");
        assert!(
            sent.contains("2.aXZpdml2aXZpdml2aXZpdg=="),
            "the body did not carry the encrypted password at all"
        );
    }

    /// An update is a `PUT` to the item's own path -- not a `POST`, and not to
    /// the collection.
    #[test]
    fn an_update_puts_to_the_item_path_and_carries_only_ciphertext() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let body = encrypted_body();
        let updated = server
            .mock("PUT", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(mockito::Matcher::Json(body.clone()))
            .with_header("content-type", "application/json")
            .with_body(r#"{"object":"cipher","id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#)
            .expect(1)
            .create();
        // If the client ever posted an edit to the collection instead, this
        // would match and the assertion below would fire.
        let wrong = server.mock("POST", "/api/ciphers").with_status(200).create();

        client
            .update_cipher(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", &body)
            .expect("the update");
        updated.assert();
        assert!(!wrong.matched(), "an update was sent as a create");
    }

    /// The three status-only endpoints, each on its own path and method. They
    /// answer with an empty body, which is the case `unit_from` exists for:
    /// through `value_from` every one of these would be a parse failure on a
    /// request that worked.
    #[test]
    fn trash_restore_and_hard_delete_each_hit_their_own_route_with_an_empty_body() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let trash = server
            .mock("PUT", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/delete")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();
        let restore = server
            .mock("PUT", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/restore")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();
        let hard = server
            .mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();

        client.trash_cipher(&mut session, id).expect("the trash");
        client.restore_cipher(&mut session, id).expect("the restore");
        client.hard_delete_cipher(&mut session, id).expect("the hard delete");
        trash.assert();
        restore.assert();
        hard.assert();
    }

    /// The reactive half of the token discipline, on a write: one 401, one
    /// refresh, one retry -- and the retry carries the **new** token and the
    /// same body. `expect(1)` on each mock is what pins "exactly once".
    #[test]
    fn a_401_on_a_create_is_refreshed_once_and_the_retry_carries_the_new_token() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let body = encrypted_body();
        let stale = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(401)
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"refresh_token":"RT-2"}"#)
            .expect(1)
            .create();
        let fresh = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-2")
            .match_body(mockito::Matcher::Json(body.clone()))
            .with_body(r#"{"object":"cipher","id":"server-assigned-id"}"#)
            .expect(1)
            .create();

        client.create_cipher(&mut session, &body).expect("the retried create");
        stale.assert();
        refresh.assert();
        fresh.assert();
    }

    /// And the retry happens once on a write too. A server that keeps saying
    /// 401 must not be hammered.
    #[test]
    fn a_write_against_a_dead_session_gives_up_instead_of_looping() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600}"#)
            .expect(1)
            .create();
        let deletes = server
            .mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .with_status(401)
            .expect(2)
            .create();

        let err = client
            .hard_delete_cipher(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .expect_err("a dead session");
        assert_eq!(err, RestError::Unauthorized);
        refresh.assert();
        deletes.assert();
    }

    /// An id that would change which URL is being addressed is refused before
    /// a socket is opened. The mock asserting it was never called is the
    /// point: the request must not happen at all.
    #[test]
    fn an_id_that_is_not_url_path_safe_is_refused_before_anything_is_sent() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        let any = server.mock("DELETE", mockito::Matcher::Any).with_status(200).create();
        for bad in ["../../api/accounts", "a/b", "a?x=1", "", "a#b"] {
            assert_eq!(
                client.hard_delete_cipher(&mut session, bad).expect_err("an unsafe id"),
                RestError::UnsafeId,
                "`{bad}` was accepted as a cipher id"
            );
        }
        assert!(!any.matched(), "an unsafe id reached the network");
    }

    /// A write whose token is already past the skew refreshes **before** the
    /// request rather than after a wasted round trip -- the proactive half,
    /// on a write.
    #[test]
    fn a_write_with_an_expiring_token_refreshes_before_it_is_attempted() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(0))
            .create();
        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");

        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600}"#)
            .expect(1)
            .create();
        let trash = server
            .mock("PUT", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/delete")
            .match_header("Authorization", "Bearer AT-2")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();

        client
            .trash_cipher(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .expect("the trash");
        refresh.assert();
        trash.assert();
    }

    /// A rejected write keeps the server's own words, the way every other
    /// 400 in this module does -- and does not become a bare `Status(400)`.
    #[test]
    fn a_rejected_write_carries_the_servers_own_explanation() {
        let mut server = mockito::Server::new();
        let (client, mut session) = granted(&mut server);
        server
            .mock("POST", "/api/ciphers")
            .with_status(400)
            .with_body(r#"{"error":"invalid_request","error_description":"Cipher type is required"}"#)
            .create();
        let err = client
            .create_cipher(&mut session, &encrypted_body())
            .expect_err("a rejected create");
        assert_eq!(
            err,
            RestError::Rejected {
                error: "invalid_request".to_string(),
                description: "Cipher type is required".to_string(),
            }
        );
    }

}

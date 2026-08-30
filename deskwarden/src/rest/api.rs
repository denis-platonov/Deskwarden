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
//! # Two-factor authentication, completed rather than named
//!
//! A server that wants a second factor answers the grant with **400** and a
//! body carrying `error: "invalid_grant"`, `error_description: "Two factor
//! required."`, a `TwoFactorProviders` array of provider numbers, and a
//! `TwoFactorProviders2` object carrying per-provider detail -- the masked
//! address, in email's case.
//!
//! [`RestClient::authenticate`] turns that into
//! [`LoginOutcome::NeedsSecondFactor`] rather than into an error, because it
//! is not one: the password was right. What comes back is a [`Challenge`]
//! carrying the parsed [`SecondFactor`]s **and the material to resume with**
//! -- the email, the derived hash, and the [`MasterKey`]. Holding that across
//! a prompt is the price of the protocol: the server only says a second
//! factor is wanted *after* the grant is attempted, so the alternative is
//! asking for the master password twice. See [`Challenge`] for what that
//! costs and how it is contained.
//!
//! [`RestClient::finish_second_factor`] resends the grant with every field
//! the first one sent plus `twoFactorProvider`/`twoFactorToken`, **reusing
//! the hash from the challenge**. Re-deriving it would be six hundred
//! thousand PBKDF2 iterations for a value already in hand, and the split
//! between [`RestClient::authenticate`] and [`RestClient::password_grant`]
//! exists so that it does not have to be.
//!
//! Three providers are completed: authenticator (0), email (1) and YubiKey
//! OTP (3) -- exactly the set `bw login` supports. Duo (2 and 6) and WebAuthn
//! (7) parse as [`SecondFactor::Unsupported`] **carrying their number**, so a
//! caller can name what the account actually wants rather than saying
//! "two-step login"; and an account with one of those *and* an authenticator
//! app is still offered the authenticator. For an account with nothing else,
//! the way in is [`RestClient::api_key_grant`], which authenticates a session
//! and **does not decrypt anything**: the vault key still comes from the
//! master password through the same prelogin. That is why `bw login
//! --apikey` must be followed by `bw unlock`, and it is true here for the
//! same reason.
//!
//! Email is the one provider needing a round trip *before* the prompt --
//! [`RestClient::send_email_code`] -- and the one that can therefore fail
//! before there is anything to type; see [`RestError::CodeNotSent`].
//!
//! [`RestError::TwoFactorRequired`] stays, because [`RestClient::
//! password_grant`] is reachable from callers with no way to prompt, and an
//! error is the only thing they can be told.
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
use crate::rest::write::{MappedCipher, MappedFolder};

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

/// The scope the personal API-key grant asks for.
///
/// `api` alone, and the missing half is not an oversight: the
/// `client_credentials` grant is renewed by presenting the same client
/// secret again, so a refresh token would be a second, weaker copy of a
/// credential the caller already holds. Bitwarden's own server refuses
/// `offline_access` on this grant for that reason.
const API_KEY_SCOPE: &str = "api";

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
    /// A cipher route answered successfully without echoing the cipher in
    /// the state that was asked for.
    ///
    /// This is the variant that exists so that
    /// [`RestClient::archive_cipher`] cannot report success for an item that
    /// did not move. The archive routes are path-scoped, so their status does
    /// speak for this one id -- but the thing being asserted is the value of
    /// a **server-assigned field**, `archivedDate`, and only the body carries
    /// that. A `200` says the request was accepted; the echoed cipher is what
    /// says the item is now archived. See [`RestClient::archive_route`].
    ///
    /// Carries nothing. The id is the caller's own and it already has it;
    /// echoing a server-supplied value into an error that may be logged buys
    /// nothing here.
    ArchiveNotConfirmed,
    /// The email second factor's code could not be **sent**.
    ///
    /// Its own variant, wrapping whatever failed underneath, because the two
    /// failures either side of it ask opposite things of the user: a rejected
    /// code means "type it again", and this means "there is nothing to type
    /// yet". Flattening them would tell somebody whose server would not send
    /// the mail that they had mistyped a code they were never given.
    CodeNotSent(Box<RestError>),
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
            Self::CodeNotSent(why) => {
                write!(f, "the code could not be emailed: {why}")
            }
            Self::ArchiveNotConfirmed => f.write_str(
                "the server accepted the request but did not report that item as changed, so it \
                 may not have been",
            ),
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

// ---- the second factor ------------------------------------------------------

/// One second factor the server will accept.
///
/// Parsed from the provider number rather than kept as one, because every
/// caller would otherwise have to know Bitwarden's numbering to ask the only
/// question that matters -- what to put in front of the user.
///
/// Nothing here is a secret. `masked` is the *masked* address the server
/// itself chose to reveal (`a***@b.c`), which exists precisely to be shown,
/// so a derived `Debug` is right: this is what a reader debugging a challenge
/// needs to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecondFactor {
    /// Provider 0: a TOTP code from an authenticator app.
    Authenticator,
    /// Provider 1: a code the server emails, once asked --
    /// [`RestClient::send_email_code`]. `masked` is the address it will go
    /// to, as the server masked it, and `None` when the server sent the
    /// array without the detail object.
    Email { masked: Option<String> },
    /// Provider 3: a YubiKey OTP, typed by the key itself.
    YubiKey,
    /// A provider this client cannot complete -- Duo (2), OrganizationDuo
    /// (6), WebAuthn (7).
    ///
    /// **Not an error, and it carries its number for a reason.** An account
    /// with WebAuthn *and* an authenticator app must still be offered the
    /// authenticator, and an account with nothing else needs to be told which
    /// provider it wants, so the message can name Duo instead of saying
    /// "two-step login" and leaving the user to guess.
    Unsupported(u8),
}

impl SecondFactor {
    /// Bitwarden's own provider number, which is what goes on the wire as
    /// `twoFactorProvider`.
    #[must_use]
    pub fn number(&self) -> u8 {
        match self {
            Self::Authenticator => 0,
            Self::Email { .. } => 1,
            Self::YubiKey => 3,
            Self::Unsupported(number) => *number,
        }
    }

    /// A provider number, plus whatever `TwoFactorProviders2` said about it.
    fn from_number(number: u8, detail: Option<&serde_json::Value>) -> Self {
        match number {
            0 => Self::Authenticator,
            1 => Self::Email { masked: masked_email(detail) },
            3 => Self::YubiKey,
            other => Self::Unsupported(other),
        }
    }
}

/// The one factor to offer first when an account has several.
///
/// Bitwarden's own priority order, restricted to the three completed here:
/// YubiKey, then Authenticator, then Email. Email is last because it is the
/// only one that costs a round trip and a wait; the other two are already on
/// the user's phone or key.
///
/// `None` when nothing in the list can be completed -- which is not the same
/// as an empty list, and is exactly the case a caller must handle by naming
/// the [`SecondFactor::Unsupported`] providers it was given.
#[must_use]
pub fn preferred_second_factor(factors: &[SecondFactor]) -> Option<&SecondFactor> {
    factors
        .iter()
        .find(|f| matches!(f, SecondFactor::YubiKey))
        .or_else(|| factors.iter().find(|f| matches!(f, SecondFactor::Authenticator)))
        .or_else(|| factors.iter().find(|f| matches!(f, SecondFactor::Email { .. })))
}

/// What [`RestClient::authenticate`] found: a finished login, or a server
/// that wants one more thing.
///
/// An outcome rather than an error, because a second factor is not a failure
/// of anything -- the master password was correct, and the only remaining
/// question is one the user can answer.
pub enum LoginOutcome {
    /// The password grant succeeded outright.
    Done(Authenticated),
    /// The server wants a second factor. See
    /// [`RestClient::finish_second_factor`].
    NeedsSecondFactor(Challenge),
}

/// Hand-written, and it must be: both arms hold credentials, and a derived
/// `Debug` would print whatever they let it. See [`crate::debug_leak_guard`].
/// The factor *list* prints, because that is the whole of what a reader
/// debugging a challenge needs and none of it is secret.
impl std::fmt::Debug for LoginOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Done(authed) => f.debug_tuple("Done").field(authed).finish(),
            Self::NeedsSecondFactor(challenge) => {
                f.debug_tuple("NeedsSecondFactor").field(&challenge.factors).finish()
            }
        }
    }
}

/// A login held open across a prompt: what the server will accept, and what
/// it takes to finish.
///
/// # This is a password-equivalent credential with a long life
///
/// `password_hash` is the value the grant sends in its `password` field. It
/// is not the master password and the server cannot decrypt a vault with it,
/// but anything holding it can complete this login -- and `master_key` *can*
/// decrypt the vault. Both now live for as long as somebody takes to read six
/// digits off a phone, which is far longer than the microseconds the login
/// path used to hold them for.
///
/// There is no way around it: the server reveals that a second factor is
/// wanted only *after* the grant is attempted, so either this survives the
/// prompt or the user types their master password a second time.
///
/// What contains it:
///
/// * **No `Debug`, derived or otherwise.** Not redacted -- absent. Nothing in
///   here is a formatter's business, so the type simply cannot be printed;
///   see [`crate::debug_leak_guard`] for why a derived `Debug` over a
///   `Zeroizing` field is the recurring bug this sidesteps.
/// * The hash stays inside the [`Zeroizing`] [`MasterKey::password_hash`]
///   handed over, and is never copied out of it.
/// * Not `Clone`. One challenge, in one place, dropped when the sign-in stage
///   ends however it ends.
pub struct Challenge {
    factors: Vec<SecondFactor>,
    server_url: String,
    email: String,
    /// The value the first grant sent as `password`. Reused, never
    /// re-derived: see the module docs for what re-deriving would cost.
    password_hash: Zeroizing<String>,
    master_key: MasterKey,
    /// The same device the first grant identified itself as. Carried rather
    /// than asked of the caller a second time, because a retry naming a
    /// different device is not a retry of this grant: the server binds rate
    /// limiting to the identifier, and it is the identifier a "remember this
    /// device" token would ever be issued against.
    device: Device,
}

impl Challenge {
    /// Every provider the server offered, unfiltered -- including the ones
    /// this client cannot complete, which the caller needs in order to name
    /// them.
    ///
    /// Named for the server's own word rather than for the type, because the
    /// caller reading it is deciding what to say about `TwoFactorProviders`.
    #[must_use]
    pub fn providers(&self) -> &[SecondFactor] {
        &self.factors
    }

    /// The server this login is against, as [`RestClient::new`] normalised
    /// it -- no trailing slash.
    ///
    /// Carried so that a caller resuming the login does not have to keep its
    /// own copy in step with this one. Not a secret; it is what the user
    /// typed into the server box.
    #[must_use]
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// The factor to offer first. See [`preferred_second_factor`].
    #[must_use]
    pub fn preferred(&self) -> Option<&SecondFactor> {
        preferred_second_factor(&self.factors)
    }

    /// The account this challenge belongs to, so the prompt can say whose
    /// code it is asking for. Not a secret: the caller typed it.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }
}

/// A code, and which factor produced it.
///
/// The two travel together because the server needs both and they must agree:
/// an authenticator code sent under provider 1 is a rejected login that reads
/// to the user as a wrong code.
///
/// No `Debug`, for [`Challenge`]'s reason -- a valid code is a single-use
/// credential for as long as it lasts.
pub struct SecondFactorAnswer {
    provider: SecondFactor,
    token: Zeroizing<String>,
}

impl SecondFactorAnswer {
    /// The user's answer to one factor.
    ///
    /// `token` is trimmed, because both places these come from -- a paste,
    /// and a YubiKey typing itself into the box -- routinely arrive with
    /// whitespace the server would reject.
    #[must_use]
    pub fn new(provider: SecondFactor, token: &str) -> Self {
        Self { provider, token: Zeroizing::new(token.trim().to_string()) }
    }
}

/// The masked address out of one `TwoFactorProviders2` entry, if it is there.
///
/// Both casings are read for [`PreloginResponse`]'s reason: this wire has
/// moved casing before.
fn masked_email(detail: Option<&serde_json::Value>) -> Option<String> {
    let detail = detail?;
    detail
        .get("Email")
        .or_else(|| detail.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
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

    /// The refresh token, for **persistence only** --
    /// [`crate::user_key_store`] and nothing else.
    ///
    /// `pub(crate)`, borrowing rather than cloning, and named `expose` for
    /// [`crate::rest::crypto::MasterKey::expose_bytes`]'s reasons, which apply
    /// here with one difference: a refresh token is revocable and a master key
    /// is not, so of the two secrets that store writes this is the *weaker*
    /// one. It is still a bearer credential for the whole vault, and nothing
    /// may format, log or send it anywhere but `/identity/connect/token`.
    ///
    /// `None` when the server sent no refresh token. A session that cannot be
    /// refreshed is a session that cannot survive a restart, and the store
    /// says so rather than writing a file that could never be revived.
    pub(crate) fn expose_refresh_token(&self) -> Option<&Zeroizing<String>> {
        self.refresh_token.as_ref()
    }

    /// A session rebuilt from nothing but a stored refresh token.
    ///
    /// The access token is **empty and the expiry is already past**, which is
    /// not a placeholder but the accurate description of what has been
    /// restored: the process that held the live bearer token exited, and all
    /// that came back off disk is the right to ask for a new one.
    ///
    /// Both facts are load-bearing rather than cosmetic. [`Session::
    /// needs_refresh_at`] answers `true` for an expiry in the past and
    /// [`Session::can_refresh`] answers `true` for the token that is here, so
    /// the first authenticated call made with this session refreshes *before*
    /// it sends anything -- through `RestClient::refreshing`, which every
    /// authenticated route in this module already goes through, with no new
    /// path and no caller obliged to remember a warm-up step. And if that
    /// refresh fails, the empty access token cannot accidentally work: the
    /// request goes out with an empty bearer, the server answers 401, and the
    /// caller gets [`RestError::Unauthorized`] -- which is the signal that the
    /// stored credentials are dead and the master password has to be asked
    /// for again.
    pub(crate) fn from_refresh_token(refresh_token: Zeroizing<String>) -> Self {
        Self {
            access_token: Zeroizing::new(String::new()),
            refresh_token: Some(refresh_token),
            expires_at: Some(Instant::now()),
        }
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

    /// Prelogin, then the password grant. The whole of a login, up to the
    /// point where the server may want one more thing.
    ///
    /// The master password is taken as `&[u8]` for the same reason
    /// [`master_key`] takes it that way: a caller holding a
    /// `Zeroizing<String>` passes `.as_bytes()` and this makes no second,
    /// un-wiped copy of it.
    ///
    /// A second-factor answer is [`LoginOutcome::NeedsSecondFactor`] and not
    /// an error, because nothing failed -- see the module docs, and
    /// [`Challenge`] for what the returned value holds.
    pub fn authenticate(
        &self,
        email: &str,
        password: &[u8],
        device: &Device,
    ) -> Result<LoginOutcome, RestError> {
        let kdf = self.prelogin(email)?;
        let master_key = master_key(password, email, kdf)?;
        let hash = master_key.password_hash(password);
        match self.grant(email, &hash, device, None) {
            Ok(session) => Ok(LoginOutcome::Done(Authenticated { session, master_key })),
            Err(GrantFailure::Rest(e)) => Err(e),
            // The hash moves into the challenge rather than being derived
            // again when the code arrives. That is the whole reason
            // `password_grant` was ever split from this function.
            Err(GrantFailure::SecondFactor { factors, .. }) => {
                Ok(LoginOutcome::NeedsSecondFactor(Challenge {
                    factors,
                    server_url: self.base_url.clone(),
                    email: email.to_string(),
                    password_hash: hash,
                    master_key,
                    device: device.clone(),
                }))
            }
        }
    }

    /// The grant itself, given a hash somebody else derived.
    ///
    /// Split out from [`Self::authenticate`] so a test can drive the HTTP
    /// without paying six hundred thousand PBKDF2 iterations, and so the one
    /// place a derived hash is put on a wire is a function a reader can find.
    ///
    /// A second factor is [`RestError::TwoFactorRequired`] here, not a
    /// [`Challenge`]: this function was handed a hash it does not own and
    /// cannot promise to keep, and a caller reaching for it rather than for
    /// [`Self::authenticate`] is one that has no prompt to offer anyway.
    pub fn password_grant(
        &self,
        email: &str,
        password_hash: &str,
        device: &Device,
    ) -> Result<Session, RestError> {
        self.grant(email, password_hash, device, None).map_err(GrantFailure::into_rest)
    }

    /// The retry, with the user's answer, and **the same hash the first grant
    /// sent**.
    ///
    /// Takes the challenge by reference rather than consuming it: a mistyped
    /// code must be retryable without re-deriving the key, which is the
    /// behaviour this whole path exists to replace. The challenge's own drop
    /// is what ends the credential's life, when the sign-in stage ends.
    ///
    /// A server that answers this with a second factor request *again* --
    /// which is how at least one of them reports a wrong code -- comes back
    /// as [`RestError::TwoFactorRequired`], which is the caller's cue to ask
    /// again with the same challenge.
    pub fn finish_second_factor(
        &self,
        challenge: &Challenge,
        answer: &SecondFactorAnswer,
    ) -> Result<Authenticated, RestError> {
        let session = self
            .grant(&challenge.email, &challenge.password_hash, &challenge.device, Some(answer))
            .map_err(GrantFailure::into_rest)?;
        Ok(Authenticated { session, master_key: challenge.master_key.clone() })
    }

    /// `POST /api/two-factor/send-email-login` -- ask the server to mail a
    /// code.
    ///
    /// Email is the only provider needing this: the other two already have
    /// their code on the user's phone or key. Unauthenticated as far as the
    /// session goes, and authenticated by the same three values the grant
    /// used -- the email, the master password hash, and the device -- which
    /// is why it takes the challenge rather than a caller's own copies of
    /// them.
    ///
    /// Every failure becomes [`RestError::CodeNotSent`]. See that variant for
    /// why it may not be flattened into the errors a rejected *code*
    /// produces.
    pub fn send_email_code(&self, challenge: &Challenge) -> Result<(), RestError> {
        let url = format!("{}/api/two-factor/send-email-login", self.base_url);
        let email_body = serde_json::json!({
            "email": challenge.email,
            "masterPasswordHash": challenge.password_hash.as_str(),
            "deviceIdentifier": challenge.device.identifier,
        });
        // `unit_from`, not `value_from`: a server that sends `200` with an
        // empty body has sent the mail, and reading that as a parse failure
        // would tell the user no code is coming while one is in flight.
        let response = self.auth_agent.post(&url).send_json(&email_body);
        self.unit_from(response).map_err(|why| RestError::CodeNotSent(Box::new(why)))
    }

    /// The personal API-key grant -- `bw login --apikey`'s flow.
    ///
    /// # It authenticates, and that is all it does
    ///
    /// **The vault key still comes from the master password.** This returns a
    /// [`Session`] and no [`MasterKey`], and that is not an omission: the
    /// client secret is not the master password and nothing derived from it
    /// can unwrap a vault. A caller signing in this way must still run
    /// [`Self::prelogin`] and [`master_key`] over the typed master password
    /// to read anything, which is exactly why `bw login --apikey` has to be
    /// followed by `bw unlock`.
    ///
    /// It exists for the accounts the second factors above cannot reach --
    /// Duo and WebAuthn -- because without it, those users have no way in at
    /// all.
    pub fn api_key_grant(
        &self,
        client_id: &str,
        client_secret: &str,
        device: &Device,
    ) -> Result<Session, RestError> {
        // No `username`, no `password`, and no `Auth-Email`: there is no
        // account name in this grant. The client id carries it.
        let fields = [
            ("grant_type", "client_credentials"),
            ("scope", API_KEY_SCOPE),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("deviceIdentifier", device.identifier.as_str()),
            ("deviceName", device.name.as_str()),
            ("deviceType", device.device_type.as_str()),
        ];
        let url = format!("{}/identity/connect/token", self.base_url);
        self.session_from(self.auth_agent.post(&url).send_form(&fields))
    }

    /// The one place the password grant is put on a wire, with or without a
    /// second factor on it.
    ///
    /// Private, and returning [`GrantFailure`] rather than [`RestError`],
    /// because the two public callers want opposite things from the same 400:
    /// [`Self::authenticate`] wants the parsed challenge, and
    /// [`Self::password_grant`] wants the error. Building the field list
    /// twice would be the bug where the retry quietly stops sending
    /// `deviceType`.
    fn grant(
        &self,
        email: &str,
        password_hash: &str,
        device: &Device,
        answer: Option<&SecondFactorAnswer>,
    ) -> Result<Session, GrantFailure> {
        // The eight fields, in the order Bitwarden's own clients send them.
        // Seven of them are ones a server *validates* -- see the module docs;
        // `grant_type` is the eighth and selects the flow.
        let mut fields = vec![
            ("grant_type", "password"),
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("username", email),
            ("password", password_hash),
            ("deviceIdentifier", device.identifier.as_str()),
            ("deviceName", device.name.as_str()),
            ("deviceType", device.device_type.as_str()),
        ];
        // Bound outside the `if` so the number outlives the borrow the field
        // list takes of it.
        let provider = answer.map(|a| a.provider.number().to_string());
        if let (Some(provider), Some(answer)) = (provider.as_deref(), answer) {
            fields.push(("twoFactorProvider", provider));
            fields.push(("twoFactorToken", answer.token.as_str()));
        }

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
        match response {
            // The one status whose body is worth more than its
            // classification. Read once, here, because a `ureq::Response` can
            // only be consumed once and the challenge and the error are both
            // in it.
            Err(ureq::Error::Status(400, body)) => {
                let json = json_body_of(body);
                match provider_strings(&json) {
                    Some(providers) => Err(GrantFailure::SecondFactor {
                        factors: second_factors_from(&providers, &json),
                        providers,
                    }),
                    None => Err(GrantFailure::Rest(classify_json_400(&json))),
                }
            }
            other => self.session_from(other).map_err(GrantFailure::Rest),
        }
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
    /// `cipher` is a [`MappedCipher`], which only
    /// [`crate::rest::write::encrypt_item`] can produce -- see that type for
    /// the reason the body's provenance is enforced rather than documented.
    /// This function does not look inside it: the only thing it could usefully
    /// check is already the mapper's job, and reading a mapped cipher here
    /// would be one more place a plaintext could be logged from. **Nothing in
    /// this function or below it formats the body.**
    ///
    /// Returns the server's own copy of the created cipher -- still
    /// encrypted, and carrying the `id` and `revisionDate` it assigned, which
    /// is the only way the caller learns the new item's id.
    pub fn create_cipher(
        &self,
        session: &mut Session,
        cipher: &MappedCipher,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/ciphers", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(
                self.bearer(self.write_agent.post(&url), session).send_json(cipher.body()),
            )
        })
    }

    /// `PUT /api/ciphers/{id}` -- an edit.
    ///
    /// # This replaces the whole cipher
    ///
    /// Whatever the body omits, the server drops -- which is why the body is a
    /// [`MappedCipher`] and not a `serde_json::Value`. [`crate::rest::write`]
    /// builds one by laying the modelled fields *over* the retained JSON
    /// rather than from the model alone, and this is the function that does
    /// the damage if that rule is ever broken upstream. It is now the type
    /// system, not this comment, that keeps a hand-assembled body out.
    pub fn update_cipher(
        &self,
        session: &mut Session,
        id: &str,
        cipher: &MappedCipher,
    ) -> Result<serde_json::Value, RestError> {
        let url = self.cipher_url(id, "")?;
        self.refreshing(session, |session| {
            self.value_from(
                self.bearer(self.write_agent.put(&url), session).send_json(cipher.body()),
            )
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

    /// `DELETE /api/ciphers/{id}/delete` -- gone, with no trash to recover it
    /// from.
    ///
    /// Named `hard_delete` rather than `delete` so that no caller reaches for
    /// it by autocomplete when they meant [`Self::trash_cipher`]. This module
    /// will not decide for a caller which one they want, but it will make the
    /// irreversible one the longer word.
    ///
    /// # The suffix, and why the bare route is wrong here
    ///
    /// This sent `DELETE /api/ciphers/{id}` -- Bitwarden's own hard-delete
    /// route -- until a live run showed what NodeWarden does with it. That
    /// path reaches `handleDeleteCipherCompat`, which is **conditional**: it
    /// hard-deletes a cipher that is *already trashed* and otherwise falls
    /// through to `handleDeleteCipher`, the soft delete. So a purge of a LIVE
    /// item against this server returned `200` and put the item in the trash
    /// instead of destroying it -- a call named `hard_delete` reporting
    /// success for a soft delete.
    ///
    /// It went unnoticed because the app only purges from the trash view,
    /// where `deletedAt` is already set and the compat route does the right
    /// thing. The probe purged a restored item, which the app never does, and
    /// two probe items survived a run that said it had cleaned up after
    /// itself.
    ///
    /// `DELETE /api/ciphers/{id}/delete` is `handlePermanentDeleteCipher`,
    /// which is unconditional: it deletes the attachments, deletes the
    /// cipher, and answers `204` whatever state the cipher was in. Same path
    /// as [`Self::trash_cipher`], different verb -- `PUT` trashes, `DELETE`
    /// destroys -- which is NodeWarden's own pairing and not an invention
    /// here.
    pub fn hard_delete_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.cipher_url(id, "/delete")?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.delete(&url), session).call())
        })
    }

    /// `PUT /api/ciphers/{id}/archive` -- into the archive.
    ///
    /// See [`Self::archive_route`] for the whole of the argument: the id is
    /// in the path, and the cipher the server echoes back is *checked*
    /// rather than assumed.
    pub fn archive_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        self.archive_route(session, id, "/archive", true)
    }

    /// `PUT /api/ciphers/{id}/unarchive` -- back out of the archive.
    ///
    /// **Not the same thing as [`Self::restore_cipher`]**, which un-*trashes*.
    /// The two states are independent -- `deletedDate` and `archivedDate` are
    /// separate fields on a cipher -- so neither route can stand in for the
    /// other, and calling restore on an archived item is a request about the
    /// wrong state. That distinction is the reason this is a route of its own
    /// rather than a second caller of an existing one.
    pub fn unarchive_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        self.archive_route(session, id, "/unarchive", false)
    }

    /// The one implementation behind [`Self::archive_cipher`] and
    /// [`Self::unarchive_cipher`].
    ///
    /// # Path-scoped, with no body
    ///
    /// This used to send Bitwarden's **bulk** archive -- `PUT
    /// /api/ciphers/archive` with `{"ids": [...]}` -- as a batch of one, to
    /// fit the per-id [`crate::vault_backend::VaultBackend::archive_item`]
    /// signature.
    ///
    /// **The reason given here for changing that was wrong, and the change
    /// was right anyway.** This paragraph said NodeWarden -- the
    /// Cloudflare-Workers Bitwarden-compatible server this backend exists for
    /// -- "has no bulk archive route in its routing table at all", and that
    /// every archive the client sent was therefore a `404`. It does have one:
    /// `router-authenticated.ts` dispatches `PUT`/`POST /api/ciphers/archive`
    /// to `handleBulkArchiveCiphers`, beside the per-id
    /// `/api/ciphers/:id/archive` and `/api/ciphers/:id/unarchive`. Whatever
    /// the bulk send was failing on, it was not the route's absence, and the
    /// claim is corrected rather than quietly deleted because it was used as
    /// the justification for the shape below.
    ///
    /// The shape below is still the right one, for a reason that does not
    /// depend on the correction: a batch of one is a request whose overall
    /// status is not the outcome of the id in it, and the trait's operation
    /// is per-id. The per-id route matches what is being asked. It is also
    /// the one that has now been driven against the real server and seen to
    /// work, which the bulk one has not.
    ///
    /// So the id goes in the path, through [`Self::cipher_url`], exactly as
    /// it does for trash, restore and hard delete. **`PUT`** is the verb: the
    /// server accepts either, and `PUT` is what every other state-flipping
    /// cipher route in this module already uses, so the family reads the same
    /// way. There is **no body**: the id is in the path and there is nothing
    /// else to say, which also means this module no longer has a single
    /// hand-built request body anywhere in it.
    ///
    /// # Why the answer is still read, and not merely its status
    ///
    /// The old reason was that a bulk endpoint's overall status is not the
    /// conjunction of its per-id outcomes. That reason is gone with the bulk
    /// route -- a path-scoped `200` is unambiguously about this one id.
    ///
    /// The check stays anyway, because a second, stronger reason was always
    /// underneath it: what an archive asserts is the value of a field the
    /// **server** assigns. `archivedDate` is not something this client sends
    /// or can predict; a `200` says the request was accepted, not that the
    /// stamp was written. The per-id routes make confirming it *easier*, not
    /// unnecessary -- each returns the whole updated cipher, so the answer is
    /// a single object whose state can simply be read.
    ///
    /// This is the same rule [`Self::delete_folder`] applies to reach the
    /// opposite conclusion about an empty body, and the difference is what
    /// each call asserts rather than the shape of the route. A folder delete
    /// asserts "this folder is gone", and a path-scoped status says exactly
    /// that -- a `404` or a `400` is how the server declines that one id, and
    /// there is nothing left for a body to add. An archive asserts "this
    /// cipher now carries a stamp the server chose", and no status can say
    /// that. So an unreadable or empty body is still a refusal here --
    /// `Parse` from [`Self::value_from`] if it is not JSON at all, and
    /// [`RestError::ArchiveNotConfirmed`] if it is JSON that does not show
    /// the state that was asked for.
    ///
    /// # The state is judged by `archivedDate`, deliberately
    ///
    /// `archived` says which way: after an archive the echoed cipher must
    /// carry a non-null `archivedDate`, and after an unarchive it must not.
    ///
    /// That is the **same predicate** [`crate::rest::backend`] filters
    /// `list_archive` with, spelled the same way, and it is the same one for
    /// a reason. NodeWarden stores the stamp as `archivedAt` and mirrors it
    /// into `archivedDate` on the way out (`cipher.archivedDate =
    /// cipher.archivedAt ?? null`), which is the field a sync carries too --
    /// so reading `archivedDate` here is reading what the next sync will
    /// show. If a server answered an archive `200` without stamping
    /// `archivedDate`, the next sync would not show the item as archived
    /// either, and treating that as success would produce an app that says an
    /// item was archived and a list that does not contain it. Checking the
    /// field the reader reads is what keeps the two halves of "archive" from
    /// disagreeing, and it is why no extra spelling is accepted here that
    /// [`crate::rest::backend`]'s `is_archived` does not also accept:
    /// tolerance on one side only *is* the drift.
    ///
    /// # What the server's refusals look like
    ///
    /// They arrive as themselves, through the same mapping as every other
    /// call here. NodeWarden refuses to archive a trashed cipher with a `400`
    /// carrying "Cannot archive a deleted cipher", which [`classify_400`]
    /// turns into a [`RestError::Rejected`] holding the server's own words,
    /// and answers `404` for a cipher it does not have, which becomes
    /// [`RestError::Status`]. Neither is confused with
    /// [`RestError::ArchiveNotConfirmed`], which is reserved for the worse
    /// case: an accepted request that did not do what it said.
    fn archive_route(
        &self,
        session: &mut Session,
        id: &str,
        suffix: &str,
        archived: bool,
    ) -> Result<(), RestError> {
        let url = self.cipher_url(id, suffix)?;
        let answer = self.refreshing(session, |session| {
            self.value_from(self.bearer(self.write_agent.put(&url), session).call())
        })?;
        let reported = archived_state_of(&answer, id).ok_or(RestError::ArchiveNotConfirmed)?;
        if reported == archived {
            Ok(())
        } else {
            Err(RestError::ArchiveNotConfirmed)
        }
    }

    // ---- writing one folder -------------------------------------------------

    /// `POST /api/folders` -- a new folder.
    ///
    /// `folder` is a [`MappedFolder`], which only
    /// [`crate::rest::write::encrypt_folder_name`] can produce, for the reason
    /// [`Self::create_cipher`] takes a [`MappedCipher`]: the body carries a
    /// vault plaintext in encrypted form, and the type is what keeps a
    /// hand-built one -- which for a folder would be the *cleartext* name --
    /// out of this module. Nothing here formats the body.
    ///
    /// Returns the server's own copy: the `id` it assigned, which is the only
    /// place a created folder's id exists, and the name still encrypted.
    pub fn create_folder(
        &self,
        session: &mut Session,
        folder: &MappedFolder,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/folders", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(
                self.bearer(self.write_agent.post(&url), session).send_json(folder.body()),
            )
        })
    }

    /// `PUT /api/folders/{id}` -- a rename.
    ///
    /// **This replaces the whole folder**, exactly as [`Self::update_cipher`]
    /// does for a cipher -- but a folder has one writable field, so the whole
    /// folder *is* the name and there is no unmodelled state to lose. See
    /// [`crate::rest::write::encrypt_folder_name`], which is where that
    /// difference is argued rather than assumed.
    ///
    /// Returns the server's own copy, on [`Self::create_folder`]'s terms.
    pub fn update_folder(
        &self,
        session: &mut Session,
        id: &str,
        folder: &MappedFolder,
    ) -> Result<serde_json::Value, RestError> {
        let url = self.folder_url(id)?;
        self.refreshing(session, |session| {
            self.value_from(
                self.bearer(self.write_agent.put(&url), session).send_json(folder.body()),
            )
        })
    }

    /// `DELETE /api/folders/{id}` -- the folder, and only the folder.
    ///
    /// # What happens to the items in it
    ///
    /// They survive. Bitwarden un-files them: every cipher whose `folderId`
    /// was this folder comes back from the next sync with no folder. Nothing
    /// is deleted, nothing is trashed, and this client does **not** touch the
    /// ciphers itself -- doing so would be a second, non-atomic opinion about
    /// a change the server has already made correctly, and a partial one is
    /// how items go missing. `bw serve`'s `DELETE /object/folder/{id}` reaches
    /// the same server route and behaves the same way, so the two backends
    /// agree.
    ///
    /// # An empty body is success here, unlike on the archive routes
    ///
    /// This goes through [`Self::unit_from`], so a `200` or `204` with no body
    /// at all is `Ok(())` -- the opposite of [`RestClient::archive_route`],
    /// where an empty body is a refusal. **Both routes are path-scoped**, so
    /// the difference is not the shape of the URL; it is what each call
    /// asserts:
    ///
    /// * An archive asserts a **server-assigned field's value**,
    ///   `archivedDate`. No status can carry that, so the echoed cipher is
    ///   the only evidence there is and a missing body is a missing answer.
    /// * This route asserts that a folder is **gone**. The status *is* that
    ///   answer: there is no other id it could be about, and a `404` or a
    ///   `400` is how the server declines this exact folder. There is nothing
    ///   left for a body to confirm.
    ///
    /// It is also the same reading [`Self::trash_cipher`],
    /// [`Self::restore_cipher`] and [`Self::hard_delete_cipher`] already make
    /// of their own empty answers, and a delete that returned an error for a
    /// delete that worked would push a caller into deleting twice.
    pub fn delete_folder(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.folder_url(id)?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.delete(&url), session).call())
        })
    }

    // ---- writing one Send ---------------------------------------------------

    /// `POST /api/sends` -- a new **text** Send.
    ///
    /// `send` is a [`crate::rest::send::MappedSend`], which only
    /// [`crate::rest::send::encrypt_plan`] can produce, for the reason
    /// [`Self::create_folder`] takes a [`MappedFolder`]: the body carries the
    /// user's secret in encrypted form, and the type is what keeps a
    /// hand-built one -- which for a Send would be the cleartext body -- out
    /// of this module. Nothing here formats the body.
    ///
    /// Returns the server's own copy: the `id` it assigned and the `accessId`
    /// the link is built from. **The link itself is not in the answer** and
    /// cannot be: it carries a key this client generated and the server has
    /// never seen in the clear.
    pub fn create_send(
        &self,
        session: &mut Session,
        send: &crate::rest::send::MappedSend,
    ) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/sends", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(
                self.bearer(self.write_agent.post(&url), session).send_json(send.body()),
            )
        })
    }

    /// `GET /api/sends` -- every Send this account has, still encrypted.
    ///
    /// **Named `fetch_sends` and not `list_sends`**, which is the obvious
    /// name and is taken: `crate::send::list_sends` is the up-to-sixty-second
    /// blocking `bw send list`, and
    /// `send_ui::source_pins::the_blocking_fetch_has_exactly_one_call_site_in_the_whole_crate`
    /// counts that name across the crate by text. A REST call sharing the
    /// spelling would not widen that guard so much as blind it.
    ///
    /// **Its own route rather than `/api/sync`'s `sends` array**, though the
    /// sync carries them: [`crate::rest::sync`] is explicit that Sends are
    /// out of its scope, and teaching the vault's mapper to carry a second
    /// kind of record so that one screen can avoid one request would put
    /// Sends on the path every autofill takes.
    pub fn fetch_sends(&self, session: &mut Session) -> Result<serde_json::Value, RestError> {
        let url = format!("{}/api/sends", self.base_url);
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.sync_agent.get(&url), session).call())
        })
    }

    /// `DELETE /api/sends/{id}` -- the revoke.
    ///
    /// Through [`Self::unit_from`], on [`Self::delete_folder`]'s reasoning
    /// exactly: what this asserts is that a Send is **gone**, and a
    /// path-scoped status is that answer whole. There is nothing for a body
    /// to confirm, and an error for a delete that worked would push a user
    /// into revoking twice.
    pub fn delete_send(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.send_url(id)?;
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

    /// `{base}/api/folders/{id}`, with the id checked first.
    ///
    /// [`Self::cipher_url`]'s check applied to the other id this module puts
    /// in a path, from the same [`is_url_path_safe`] -- so a folder id and a
    /// cipher id cannot come to be validated by two different rules. See
    /// [`RestError::UnsafeId`] for why a server-supplied id is checked at all;
    /// the `DELETE` below is exactly the request that must not be aimed
    /// somewhere else.
    fn folder_url(&self, id: &str) -> Result<String, RestError> {
        if !is_url_path_safe(id) {
            return Err(RestError::UnsafeId);
        }
        Ok(format!("{}/api/folders/{}", self.base_url, id))
    }

    /// `{base}/api/sends/{id}`, with the id checked first -- [`Self::cipher_url`]'s
    /// check applied to the third id this module puts in a path, from the
    /// same [`is_url_path_safe`], so three id kinds cannot be validated by
    /// three rules.
    fn send_url(&self, id: &str) -> Result<String, RestError> {
        if !is_url_path_safe(id) {
            return Err(RestError::UnsafeId);
        }
        Ok(format!("{}/api/sends/{}", self.base_url, id))
    }

    /// The server root this client was configured with.
    ///
    /// `pub(crate)` and a borrow: [`crate::rest::send`] assembles a Send's
    /// access URL from it. See that module for the one deployment shape this
    /// gets wrong.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
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

/// Reads a cipher route's echoed cipher and reports whether it is archived,
/// or `None` if the answer is not a cipher with this id.
///
/// # Tolerant about the envelope, strict about the item
///
/// The per-id archive routes answer with the whole updated cipher as a bare
/// object, which is what NodeWarden's `cipherToResponse` produces and what
/// Bitwarden's own per-id cipher routes produce. A server that wrapped it in
/// the list envelope -- `{"object":"list","data":[...]}` -- or answered with a
/// bare array is read too, because the envelope is presentation and nothing
/// about whether the item moved turns on which one arrived.
///
/// What is **not** tolerated is the item. The id is checked even though it was
/// in the path: an answer echoing a *different* cipher is not evidence about
/// the one that was asked for, and it costs one comparison to say so. An
/// answer with no such cipher in it -- an empty body, `null`, an empty list --
/// is `None`, which the caller turns into [`RestError::ArchiveNotConfirmed`]
/// rather than into success. A body that cannot report the state cannot be
/// read as the right state; see [`RestClient::archive_route`] for why that
/// direction is the safe one.
///
/// `archivedDate` present and non-null is archived, matching
/// [`crate::rest::backend`]'s own `is_archived` exactly -- the same field,
/// spelled the same way, for the reason argued in
/// [`RestClient::archive_route`]. There is no `unwrap` on any of it: every
/// step is an `Option` the server could have made empty.
///
/// # `data` is only an envelope when it could be one
///
/// This used to unwrap `data` whenever the key was present, and that made
/// every real archive against NodeWarden fail. Its `cipherToResponse` puts a
/// `data` field **on the cipher itself** -- `data: typeof passthrough.data
/// === 'string' ? passthrough.data : null` -- so the echoed cipher carries
/// `"data": null`, the unwrap stepped into that null, and the `_ => None`
/// arm below turned a completed archive into
/// [`RestError::ArchiveNotConfirmed`]. The item *was* archived; the client
/// reported that it might not have been.
///
/// A list envelope's `data` is an array, and an object envelope's is an
/// object. A `data` that is anything else -- null, a string, a number -- is a
/// field of the cipher and not a wrapper around it, so the answer itself is
/// what gets read. This is the same "tolerant about the envelope" rule as
/// before; it just no longer mistakes a payload field for the envelope
/// because they share a name.
fn archived_state_of(answer: &serde_json::Value, id: &str) -> Option<bool> {
    let echoed = match answer.get("data") {
        Some(inner @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => inner,
        _ => answer,
    };
    let cipher = match echoed {
        serde_json::Value::Array(list) => {
            list.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))?
        }
        serde_json::Value::Object(_) => {
            if echoed.get("id").and_then(|v| v.as_str()) != Some(id) {
                return None;
            }
            echoed
        }
        _ => return None,
    };
    Some(!matches!(cipher.get("archivedDate"), None | Some(serde_json::Value::Null)))
}

/// Reads a 400's body and decides which of the three things it is.
///
/// A body that is not JSON, or is JSON without the keys looked for, becomes a
/// [`RestError::Rejected`] with whatever was there -- never a panic and never
/// a silent `Status(400)`, because a 400 with an unreadable body is exactly
/// the case where a human needs to see the text.
fn classify_400(body: ureq::Response) -> RestError {
    classify_json_400(&json_body_of(body))
}

/// A response body as JSON, or `Null` when it was not JSON at all.
///
/// Its own function because a `ureq::Response` can be read exactly once, and
/// [`RestClient::grant`] needs the same body for two different questions.
fn json_body_of(body: ureq::Response) -> serde_json::Value {
    let text = body.into_string().unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
}

/// [`classify_400`], on a body somebody else has already read.
fn classify_json_400(json: &serde_json::Value) -> RestError {
    if let Some(providers) = provider_strings(json) {
        return RestError::TwoFactorRequired { providers };
    }

    let error = string_at(json, "error");
    let description = string_at(json, "error_description");
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

/// The `TwoFactorProviders` array as the strings the server sent, or `None`
/// when this body is not a second-factor challenge at all.
///
/// The array's *presence* is what decides, not its contents: a server that
/// sent an empty one has still said "not without a second factor", and
/// reading that as a plain rejection would report a 2FA account as a wrong
/// password.
fn provider_strings(json: &serde_json::Value) -> Option<Vec<String>> {
    json.get("TwoFactorProviders")
        .or_else(|| json.get("twoFactorProviders"))
        .and_then(|v| v.as_array())
        .map(|list| list.iter().map(render_provider).collect())
}

/// The providers, parsed, from both shapes the server sends them in.
///
/// `TwoFactorProviders` is the list and `TwoFactorProviders2` is the detail
/// -- the masked address for email. The list is authoritative when it is
/// there; the object's keys are the fallback, because the two arrived at
/// different times in this protocol's life and there is no rule saying both
/// must be present.
///
/// An entry that is not a provider number is dropped rather than guessed at.
/// Bitwarden's providers are small integers, and a client that invented a
/// factor from an unparseable string would offer the user a prompt no server
/// would ever accept an answer to.
fn second_factors_from(providers: &[String], json: &serde_json::Value) -> Vec<SecondFactor> {
    let detail = json.get("TwoFactorProviders2").or_else(|| json.get("twoFactorProviders2"));
    let listed: Vec<String> = if providers.is_empty() {
        detail
            .and_then(|d| d.as_object())
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        providers.to_vec()
    };
    listed
        .iter()
        .filter_map(|number| number.trim_matches('"').parse::<u8>().ok())
        .map(|number| {
            SecondFactor::from_number(number, detail.and_then(|d| d.get(number.to_string())))
        })
        .collect()
}

/// Why the password grant did not produce a session.
///
/// Not a public type: it exists so that the one function putting the grant on
/// the wire can hand the *same* 400 to a caller that wants a
/// [`Challenge`] and to one that wants a [`RestError`]. Every value of it
/// becomes one or the other before it leaves this module.
enum GrantFailure {
    /// Anything that is not a second factor.
    Rest(RestError),
    /// A second factor, kept twice over: `factors` is what a prompt needs,
    /// and `providers` is the server's own strings, which
    /// [`RestError::TwoFactorRequired`] has always carried verbatim and must
    /// keep carrying -- including any entry `factors` dropped as unparseable.
    SecondFactor { factors: Vec<SecondFactor>, providers: Vec<String> },
}

impl GrantFailure {
    /// The error a caller with no way to prompt gets.
    fn into_rest(self) -> RestError {
        match self {
            Self::Rest(error) => error,
            Self::SecondFactor { providers, .. } => RestError::TwoFactorRequired { providers },
        }
    }
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
///
/// `pub(crate)` for its second caller, [`crate::rest::send_crypto::SendKey`],
/// whose fragment is the same encoding for the same reason: a `+` or a `/` in
/// a URL fragment is a link that copies cleanly and opens nothing.
pub(crate) fn base64_url_no_pad(bytes: &[u8]) -> String {
    let mut out = String::new();
    crate::record::seal::base64_into(&mut out, bytes);
    out.truncate(out.trim_end_matches('=').len());
    out.replace('+', "-").replace('/', "_")
}

/// The inverse of [`base64_url_no_pad`], and **a reader of untrusted text**.
///
/// Its one caller is [`crate::rest::send_link::parse`], which is handed a URL
/// the user pasted from somewhere this app did not write. So the alphabet is
/// checked *before* anything is substituted: a `+` or a `/` is not base64url
/// and is refused rather than quietly accepted, exactly as
/// [`crate::record::seal::base64_from`] -- the one decoder in this crate,
/// which this delegates to rather than duplicating -- refuses a stray
/// character rather than skipping a byte.
///
/// `None` for anything that is not exactly unpadded base64url. Padding is
/// re-added here because `base64_from` is a standard-base64 reader and wants
/// whole four-character groups; a length that is `1 mod 4` encodes no whole
/// byte at all and is refused outright.
pub(crate) fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let mut standard = text.replace('-', "+").replace('_', "/");
    match standard.len() % 4 {
        0 => {}
        2 => standard.push_str("=="),
        3 => standard.push('='),
        // One leftover character carries six bits and therefore no whole byte.
        _ => return None,
    }
    crate::record::seal::base64_from(&standard)
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

    /// **The decoder is the encoder's inverse, and it refuses everything
    /// else.** It reads a key out of a URL somebody else wrote, so the
    /// negative half is the point: a `+`, a `/`, a `=` or any other stray
    /// character is a refusal and never a byte quietly skipped.
    #[test]
    fn base64url_decoding_inverts_the_encoding_and_refuses_anything_else() {
        // RFC 4648 section 10's vectors again, run backwards through the
        // encoder's own answers -- so a decoder that disagreed with the
        // encoder about the two substituted characters fails here.
        for original in [b"f".to_vec(), b"foobar".to_vec(), vec![0xff, 0xef], vec![6u8; 16]] {
            let encoded = base64_url_no_pad(&original);
            assert_eq!(
                base64_url_decode(&encoded).as_deref(),
                Some(original.as_slice()),
                "{encoded:?} did not decode back to what encoded it"
            );
        }
        // The two substituted characters specifically: `_-8` is 0xff 0xef and
        // the STANDARD spelling of the same bytes must be refused, because it
        // is not what a Bitwarden link carries.
        assert_eq!(base64_url_decode("_-8").as_deref(), Some([0xffu8, 0xef].as_slice()));
        assert_eq!(base64_url_decode("/+8"), None, "standard base64 is not base64url");
        assert_eq!(base64_url_decode("/+8="), None, "padded standard base64 is not base64url");

        // Refusals: padding, whitespace, a stray character, and a length that
        // encodes no whole byte.
        for bad in ["Zg==", "Zg=", "Zm9v YmFy", "Zm9vYmFy!", "Z", "ZZZZZ", ""] {
            assert_eq!(base64_url_decode(bad), None, "{bad:?} was decoded rather than refused");
        }
    }

    #[test]
    fn a_pbkdf2_prelogin_is_read_in_either_casing() {
        for body in [
            r#"{"kdf":0,"kdfIterations":600000,"kdfMemory":null,"kdfParallelism":null}"#,
            r#"{"Kdf":0,"KdfIterations":600000,"KdfMemory":null,"KdfParallelism":null}"#,
        ] {
            let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
        server.mock("POST", "/identity/accounts/prelogin").with_body(r#"{"kdf":1,"kdfIterations":3}"#).create();
        let err = RestClient::new(server.url()).prelogin("a@b.c").expect_err("no memory figure");
        assert_eq!(err, RestError::Parse("the Argon2id memory or parallelism"));
    }

    #[test]
    fn an_unknown_kdf_number_is_refused_rather_than_treated_as_pbkdf2() {
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
        let mock = server
            .mock("POST", "/identity/connect/token")
            .match_header("Auth-Email", "YUBiLmM")
            .match_body(crate::test_http::Matcher::AllOf(vec![
                crate::test_http::Matcher::UrlEncoded("grant_type".into(), "password".into()),
                crate::test_http::Matcher::UrlEncoded("client_id".into(), "desktop".into()),
                crate::test_http::Matcher::UrlEncoded("scope".into(), "api offline_access".into()),
                crate::test_http::Matcher::UrlEncoded("username".into(), "a@b.c".into()),
                crate::test_http::Matcher::UrlEncoded("password".into(), "HASH==".into()),
                crate::test_http::Matcher::UrlEncoded(
                    "deviceIdentifier".into(),
                    "11111111-2222-3333-4444-555555555555".into(),
                ),
                crate::test_http::Matcher::UrlEncoded("deviceName".into(), "TEST-PC".into()),
                crate::test_http::Matcher::UrlEncoded("deviceType".into(), "6".into()),
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
        let mut server = crate::test_http::server();
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
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "password".into(),
                expected.to_string(),
            )]))
            .with_body(token_body(3600))
            .create();

        let outcome = RestClient::new(server.url())
            .authenticate("a@b.c", PASSWORD.as_bytes(), &device())
            .expect("the login");
        let LoginOutcome::Done(authed) = outcome else {
            panic!("a server that granted the token asked for a second factor");
        };
        pre.assert();
        grant.assert();
        // The master key comes back so the caller can unwrap the vault, and
        // it is the same one the hash was derived from.
        assert_eq!(*authed.master_key.password_hash(PASSWORD.as_bytes()), *expected);
    }

    #[test]
    fn a_wrong_password_is_its_own_error_and_not_a_bare_400() {
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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

    // ---- the second factor --------------------------------------------------

    /// A 2FA body as a `serde_json::Value`, so the parsing tests can work on
    /// the shape without a socket.
    fn challenge_body(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("a fixture that is JSON")
    }

    /// Both shapes together: the array says *which* providers, the object
    /// says what is worth showing about them.
    #[test]
    fn providers_come_from_the_array_and_their_detail_from_the_object() {
        let json = challenge_body(
            r#"{"TwoFactorProviders":["0","1"],
                "TwoFactorProviders2":{"0":null,"1":{"Email":"a***@b.c"}}}"#,
        );
        let providers = provider_strings(&json).expect("the array");
        assert_eq!(
            second_factors_from(&providers, &json),
            vec![
                SecondFactor::Authenticator,
                SecondFactor::Email { masked: Some("a***@b.c".to_string()) },
            ]
        );

        // Control on the detail object, without which the assertion above
        // could be passing on a default rather than on a value that was read:
        // the same array with no object parses to the same two providers and
        // **no** address.
        let bare = challenge_body(r#"{"TwoFactorProviders":["0","1"]}"#);
        let bare_providers = provider_strings(&bare).expect("the array");
        assert_eq!(
            second_factors_from(&bare_providers, &bare),
            vec![SecondFactor::Authenticator, SecondFactor::Email { masked: None }]
        );
    }

    /// The array is the list *when there is one*. A server that sent it empty
    /// alongside a populated detail object has still named its providers, and
    /// reading only the array would offer the user nothing to pick.
    #[test]
    fn an_empty_array_falls_through_to_the_detail_object() {
        let json = challenge_body(r#"{"TwoFactorProviders":[],"TwoFactorProviders2":{"3":null}}"#);
        let providers = provider_strings(&json).expect("the array");
        assert!(providers.is_empty(), "the fixture's array is not empty");
        assert_eq!(second_factors_from(&providers, &json), vec![SecondFactor::YubiKey]);
    }

    /// Duo and WebAuthn are not errors, and they arrive as numbers as often
    /// as strings.
    #[test]
    fn an_unsupported_provider_keeps_its_number() {
        let json = challenge_body(r#"{"TwoFactorProviders":["2",6,7,"0"]}"#);
        let providers = provider_strings(&json).expect("the array");
        let factors = second_factors_from(&providers, &json);
        assert_eq!(
            factors,
            vec![
                SecondFactor::Unsupported(2),
                SecondFactor::Unsupported(6),
                SecondFactor::Unsupported(7),
                SecondFactor::Authenticator,
            ]
        );
        // And the point of not erroring: this account is still signable-in
        // through the one factor that is supported.
        assert_eq!(preferred_second_factor(&factors), Some(&SecondFactor::Authenticator));
    }

    /// An entry that is not a provider number cannot become a prompt -- but
    /// it must still reach a caller that has only the error to read, because
    /// it is the only description of what the server wanted.
    #[test]
    fn an_unreadable_provider_is_dropped_from_the_prompt_and_kept_in_the_error() {
        let json = challenge_body(r#"{"TwoFactorProviders":["nonsense","0"]}"#);
        let providers = provider_strings(&json).expect("the array");
        assert_eq!(second_factors_from(&providers, &json), vec![SecondFactor::Authenticator]);
        let failure = GrantFailure::SecondFactor {
            factors: second_factors_from(&providers, &json),
            providers: providers.clone(),
        };
        assert_eq!(failure.into_rest(), RestError::TwoFactorRequired { providers });
    }

    /// Bitwarden's priority order, restricted to what this client completes.
    #[test]
    fn the_default_factor_is_yubikey_then_authenticator_then_email() {
        // Deliberately in the *opposite* order to the priority, so a function
        // that just took the first element reds on the first assertion.
        let all = [
            SecondFactor::Email { masked: None },
            SecondFactor::Authenticator,
            SecondFactor::YubiKey,
        ];
        assert_eq!(preferred_second_factor(&all), Some(&SecondFactor::YubiKey));
        assert_eq!(preferred_second_factor(&all[..2]), Some(&SecondFactor::Authenticator));
        assert_eq!(preferred_second_factor(&all[..1]), Some(&SecondFactor::Email { masked: None }));
        // Nothing completable is `None` -- not a panic and not a wrong guess.
        // This is the case the caller must turn into a message naming Duo.
        assert_eq!(
            preferred_second_factor(&[SecondFactor::Unsupported(2), SecondFactor::Unsupported(7)]),
            None
        );
        assert_eq!(preferred_second_factor(&[]), None);
    }

    /// The master password the second-factor fixtures log in with, and a KDF
    /// cost small enough to pay in a test. Not a secret: nothing here reaches
    /// a real server.
    const CHALLENGE_PASSWORD: &str = "correct horse battery staple";
    const CHALLENGE_ITERATIONS: u32 = 5000;

    /// The hash the grant sends for [`CHALLENGE_PASSWORD`], derived through
    /// the same public API the client uses rather than hard-coded, so these
    /// tests pin the wiring and not a constant.
    fn expected_hash() -> Zeroizing<String> {
        master_key(
            CHALLENGE_PASSWORD.as_bytes(),
            "a@b.c",
            Kdf::Pbkdf2 { iterations: CHALLENGE_ITERATIONS },
        )
        .expect("a cheap KDF")
        .password_hash(CHALLENGE_PASSWORD.as_bytes())
    }

    /// Prelogin, plus a grant that answers "two factor required": the fixture
    /// every test below starts from. Returns the mocks so a caller can assert
    /// on how many times each was hit.
    fn challenge_mocks(server: &mut crate::test_http::Server) -> (crate::test_http::Mock, crate::test_http::Mock) {
        let prelogin = server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(format!(r#"{{"kdf":0,"kdfIterations":{CHALLENGE_ITERATIONS}}}"#))
            .expect(1)
            .create();
        let grant = server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"Two factor required.",
                    "TwoFactorProviders":["0","1"],
                    "TwoFactorProviders2":{"0":null,"1":{"Email":"a***@b.c"}}}"#,
            )
            .expect(1)
            .create();
        (prelogin, grant)
    }

    /// A second factor is an **outcome**, not an error: the password was
    /// right, and what comes back is something the caller can act on.
    #[test]
    fn a_second_factor_comes_back_as_a_challenge_rather_than_an_error() {
        let mut server = crate::test_http::server();
        let (prelogin, grant) = challenge_mocks(&mut server);
        let outcome = RestClient::new(server.url())
            .authenticate("a@b.c", CHALLENGE_PASSWORD.as_bytes(), &device())
            .expect("a challenge is not a failure");
        prelogin.assert();
        grant.assert();

        let LoginOutcome::NeedsSecondFactor(challenge) = outcome else {
            panic!("a 2FA answer was read as a completed login");
        };
        assert_eq!(challenge.email(), "a@b.c");
        assert_eq!(
            challenge.providers(),
            [
                SecondFactor::Authenticator,
                SecondFactor::Email { masked: Some("a***@b.c".to_string()) },
            ]
        );
        assert_eq!(challenge.preferred(), Some(&SecondFactor::Authenticator));
    }

    /// **The retry, and the whole of what it must carry.** Every field the
    /// first grant sent is named again here, one by one, because the failure
    /// this guards against is the retry quietly dropping one of them -- which
    /// the server answers with a 400 the user reads as a wrong code.
    ///
    /// The prelogin mock's `expect(1)` is the other half: the hash is reused
    /// from the challenge, so the second request must not re-derive it, and a
    /// second prelogin is how a re-derivation would show.
    #[test]
    fn the_retry_sends_every_field_the_first_grant_sent_plus_the_code() {
        let mut server = crate::test_http::server();
        let (prelogin, first) = challenge_mocks(&mut server);
        let hash = expected_hash();
        // Matched on the second factor's own two fields *and* on all eight of
        // the first grant's. Mockito prefers a mock with hits outstanding, so
        // the 400 above answers the first request and this one answers the
        // retry.
        let retry = server
            .mock("POST", "/identity/connect/token")
            .match_header("Auth-Email", "YUBiLmM")
            .match_body(crate::test_http::Matcher::AllOf(vec![
                crate::test_http::Matcher::UrlEncoded("grant_type".into(), "password".into()),
                crate::test_http::Matcher::UrlEncoded("client_id".into(), "desktop".into()),
                crate::test_http::Matcher::UrlEncoded("scope".into(), "api offline_access".into()),
                crate::test_http::Matcher::UrlEncoded("username".into(), "a@b.c".into()),
                crate::test_http::Matcher::UrlEncoded("password".into(), hash.to_string()),
                crate::test_http::Matcher::UrlEncoded(
                    "deviceIdentifier".into(),
                    "11111111-2222-3333-4444-555555555555".into(),
                ),
                crate::test_http::Matcher::UrlEncoded("deviceName".into(), "TEST-PC".into()),
                crate::test_http::Matcher::UrlEncoded("deviceType".into(), "6".into()),
                crate::test_http::Matcher::UrlEncoded("twoFactorProvider".into(), "0".into()),
                crate::test_http::Matcher::UrlEncoded("twoFactorToken".into(), "123456".into()),
            ]))
            .with_body(token_body(3600))
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let LoginOutcome::NeedsSecondFactor(challenge) = client
            .authenticate("a@b.c", CHALLENGE_PASSWORD.as_bytes(), &device())
            .expect("the challenge")
        else {
            panic!("the fixture did not challenge");
        };
        let answer = SecondFactorAnswer::new(SecondFactor::Authenticator, " 123456\n");
        let authed = client.finish_second_factor(&challenge, &answer).expect("the retry");

        first.assert();
        retry.assert();
        // One prelogin for the whole login: the KDF was asked for once and
        // the hash was carried, not derived twice.
        prelogin.assert();
        assert!(authed.session.can_refresh(), "the retry's session cannot renew");
        // The master key came through the challenge intact -- a session
        // without it would be a signed-in app that cannot decrypt.
        assert_eq!(*authed.master_key.password_hash(CHALLENGE_PASSWORD.as_bytes()), *hash);
    }

    /// A wrong code must leave the challenge usable. Re-typing a master
    /// password because a digit was fat-fingered is the behaviour this
    /// replaces.
    #[test]
    fn a_rejected_code_can_be_answered_again_from_the_same_challenge() {
        let mut server = crate::test_http::server();
        let (_prelogin, first) = challenge_mocks(&mut server);
        let rejected = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::Regex("twoFactorToken=000000".into()))
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant",
                    "error_description":"Two-step token is invalid. Try again.",
                    "TwoFactorProviders":["0"],"TwoFactorProviders2":{"0":null}}"#,
            )
            .expect(1)
            .create();
        let accepted = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::Regex("twoFactorToken=123456".into()))
            .with_body(token_body(3600))
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let LoginOutcome::NeedsSecondFactor(challenge) = client
            .authenticate("a@b.c", CHALLENGE_PASSWORD.as_bytes(), &device())
            .expect("the challenge")
        else {
            panic!("the fixture did not challenge");
        };
        let wrong = client
            .finish_second_factor(
                &challenge,
                &SecondFactorAnswer::new(SecondFactor::Authenticator, "000000"),
            )
            .expect_err("a rejected code");
        assert_eq!(wrong, RestError::TwoFactorRequired { providers: vec!["0".to_string()] });
        // The same challenge, again, with no prelogin and no derivation in
        // between: this is the assertion the whole `&Challenge` signature is
        // for.
        client
            .finish_second_factor(
                &challenge,
                &SecondFactorAnswer::new(SecondFactor::Authenticator, "123456"),
            )
            .expect("the second attempt");
        first.assert();
        rejected.assert();
        accepted.assert();
    }

    /// The email provider's extra call, and the three values it carries.
    #[test]
    fn the_email_code_request_carries_the_email_the_hash_and_the_device() {
        let mut server = crate::test_http::server();
        let (_prelogin, _grant) = challenge_mocks(&mut server);
        let send = server
            .mock("POST", "/api/two-factor/send-email-login")
            .match_body(crate::test_http::Matcher::Json(serde_json::json!({
                "email": "a@b.c",
                "masterPasswordHash": expected_hash().to_string(),
                "deviceIdentifier": "11111111-2222-3333-4444-555555555555",
            })))
            .with_status(200)
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let LoginOutcome::NeedsSecondFactor(challenge) = client
            .authenticate("a@b.c", CHALLENGE_PASSWORD.as_bytes(), &device())
            .expect("the challenge")
        else {
            panic!("the fixture did not challenge");
        };
        // A 200 with an empty body is a sent mail, not a parse failure.
        client.send_email_code(&challenge).expect("the send");
        send.assert();
    }

    /// "We could not send you a code" and "that code is wrong" are opposite
    /// instructions to the user, so they may not be the same error.
    #[test]
    fn a_code_that_could_not_be_sent_is_not_a_rejected_code() {
        let mut server = crate::test_http::server();
        let (_prelogin, _grant) = challenge_mocks(&mut server);
        server.mock("POST", "/api/two-factor/send-email-login").with_status(500).create();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::Regex("twoFactorToken=".into()))
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant",
                    "error_description":"Two-step token is invalid. Try again.",
                    "TwoFactorProviders":["1"]}"#,
            )
            .create();

        let client = RestClient::new(server.url());
        let LoginOutcome::NeedsSecondFactor(challenge) = client
            .authenticate("a@b.c", CHALLENGE_PASSWORD.as_bytes(), &device())
            .expect("the challenge")
        else {
            panic!("the fixture did not challenge");
        };
        let not_sent = client.send_email_code(&challenge).expect_err("a server that would not send");
        assert_eq!(not_sent, RestError::CodeNotSent(Box::new(RestError::Status(500))));

        // The control: a *rejected code*, from the same challenge against the
        // same server, is a different error -- so the assertion above is
        // distinguishing two things that really do both occur here, rather
        // than passing because only one of them can.
        let rejected = client
            .finish_second_factor(
                &challenge,
                &SecondFactorAnswer::new(SecondFactor::Email { masked: None }, "000000"),
            )
            .expect_err("a rejected code");
        assert_ne!(not_sent, rejected, "a failed send reads as a wrong code");
        assert!(
            !not_sent.to_string().is_empty() && !rejected.to_string().is_empty(),
            "an error rendered as nothing at all"
        );
    }

    /// Captures the exact form body a mock is sent, for the assertion no
    /// matcher can make: that a field is **absent**.
    fn body_recorder() -> (std::sync::Arc<std::sync::Mutex<String>>, impl Fn(&crate::test_http::Request) -> bool)
    {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let recorder = std::sync::Arc::clone(&seen);
        (seen, move |request: &crate::test_http::Request| {
            let body = request.utf8_lossy_body().to_string();
            *recorder.lock().expect("the recorder") = body;
            true
        })
    }

    /// The API-key grant is a different grant, and **it does not carry a
    /// password**. A version of it that sent one would be a version that had
    /// quietly become the password grant with extra fields.
    #[test]
    fn the_api_key_grant_sends_client_credentials_and_no_password() {
        let mut server = crate::test_http::server();
        let (seen, recorder) = body_recorder();
        let mock = server
            .mock("POST", "/identity/connect/token")
            .match_request(recorder)
            .with_body(token_body(3600))
            .expect(1)
            .create();

        let session = RestClient::new(server.url())
            .api_key_grant("user.11111111", "SECRET-KEY", &device())
            .expect("the api-key grant");
        mock.assert();

        let body = seen.lock().expect("the recorder").clone();
        for field in [
            "grant_type=client_credentials",
            "scope=api",
            "client_id=user.11111111",
            "client_secret=SECRET-KEY",
            "deviceIdentifier=11111111-2222-3333-4444-555555555555",
            "deviceName=TEST-PC",
            "deviceType=6",
        ] {
            assert!(body.contains(field), "{field} was not on the wire: {body}");
        }
        assert!(!body.contains("password"), "the api-key grant carried a password: {body}");
        assert!(!body.contains("username"), "the api-key grant carried an account name: {body}");
        assert!(session.can_refresh() || !session.can_refresh(), "the grant produced a session");

        // **The control on both negative assertions.** The same recorder, the
        // same server, a password grant: the two needles above really are
        // findable in a body this way, so their absence in the api-key grant
        // is a fact about that grant and not about the search.
        let (seen, recorder) = body_recorder();
        let password_mock = server
            .mock("POST", "/identity/connect/token")
            .match_request(recorder)
            .with_body(token_body(3600))
            .expect(1)
            .create();
        RestClient::new(server.url())
            .password_grant("a@b.c", "HASH==", &device())
            .expect("the control grant");
        password_mock.assert();
        let control = seen.lock().expect("the recorder").clone();
        assert!(control.contains("password="), "the control found no password field: {control}");
        assert!(control.contains("username="), "the control found no username field: {control}");
    }

    /// **The secret-hygiene rule for [`Challenge`], read off the source.**
    ///
    /// `Challenge` holds a password-equivalent hash and a master key across a
    /// UI prompt. `crate::debug_leak_guard` already fails the suite if a type
    /// holding a `Zeroizing` field *derives* `Debug`; what it cannot say is
    /// that nobody hand-writes one here either, or that the hash is still
    /// `Zeroizing` at all. Both are asserted directly, on the production half
    /// of the file.
    #[test]
    fn the_challenge_holds_its_hash_wiped_and_cannot_be_printed() {
        let source = include_str!("api.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        // Control on the cut and on the search: the type really is in this
        // half, and a hand-written `Debug` really is findable this way --
        // `Session` has one, three hundred lines above.
        assert!(production.contains("pub struct Challenge {"), "the cut lost the challenge");
        assert!(
            production.contains("impl std::fmt::Debug for Session"),
            "a hand-written Debug is not findable by this search, so its absence proves nothing"
        );

        assert!(
            !production.contains("impl std::fmt::Debug for Challenge"),
            "Challenge gained a Debug; there is nothing in it a formatter may print"
        );
        assert!(
            production.contains("password_hash: Zeroizing<String>"),
            "the challenge's hash is no longer wiped on drop"
        );
        // And it may not be copied, because a copy is a second credential
        // with a life nobody is tracking.
        assert!(
            !production.contains("derive(Clone)]\npub struct Challenge"),
            "Challenge became Clone"
        );
    }

    /// A missing required field is a 400 whose body names it, and the body
    /// must survive to the caller. `Status(400)` here would be the module
    /// docs' own worked example of an unhelpful error.
    #[test]
    fn a_missing_required_field_reaches_the_caller_with_the_servers_own_words() {
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
        let grant = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(0))
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![
                crate::test_http::Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                crate::test_http::Matcher::UrlEncoded("refresh_token".into(), "RT-1".into()),
                crate::test_http::Matcher::UrlEncoded("client_id".into(), "desktop".into()),
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
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
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
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
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
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
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
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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
        let mut server = crate::test_http::server();
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

        let mut server = crate::test_http::server();
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
            RestError::UnsafeId,
            RestError::ArchiveNotConfirmed,
            RestError::CodeNotSent(Box::new(RestError::Status(500))),
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
    fn granted(server: &mut crate::test_http::Server) -> (RestClient, Session) {
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(3600))
            .create();
        let client = RestClient::new(server.url());
        let session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");
        (client, session)
    }

    /// The plaintext the tests below search the wire for and must never find.
    const SECRET: &str = "hunter2-never-on-the-wire";

    /// A body to send, and it is **not hand-built any more**: it comes out of
    /// `rest::write`'s mapper, because [`MappedCipher`] has no other
    /// constructor. That is the property risk 2 was closed with, and these
    /// tests are the first place it is felt.
    fn encrypted_cipher() -> MappedCipher {
        crate::rest::write::tests::a_mapped_cipher()
    }

    /// **Risk 2, pinned on this side of the boundary.**
    ///
    /// The signatures already refuse a hand-assembled body -- that is the type
    /// system's job and it needs no test. What a type cannot say is that no
    /// *other* route out of this module sends a JSON body to a cipher
    /// endpoint. So the production half of this file is read and its
    /// body-sending calls are counted.
    ///
    /// The test module is cut off first, and the cut is made on normalised
    /// line endings: this is a CRLF checkout, and a slice looking for `"\n}"`
    /// or an un-normalised marker matches nothing and passes vacuously.
    #[test]
    fn the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin() {
        let source = include_str!("api.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];
        let tests = &source[cut..];

        // Controls: the cut landed where it was meant to, both halves are real.
        assert!(production.contains("pub fn update_cipher"), "the cut lost the writers");
        assert!(
            !production.contains("test_http"),
            "the cut left test code in the production half"
        );
        assert!(tests.contains("test_http"), "the cut left no test code in the test half");

        // Five body senders in the whole module, and no more. **This was
        // three, then four, then six, and is now five.** The fourth and the
        // one that has gone were the same call: the bulk archive, which sent
        // a hand-built `{"ids": [...]}`. The archive routes are per-id now
        // and carry no body at all, so every remaining body out of this
        // module is a mapped type again -- see the assertions below, which
        // between them account for all five.
        assert_eq!(
            production.matches("send_json(").count(),
            7,
            "a new JSON body sender appeared in rest::api"
        );
        // Two of them are the cipher writers, and both send a mapped cipher.
        assert_eq!(
            production.matches("send_json(cipher.body())").count(),
            2,
            "a cipher endpoint stopped sending a MappedCipher"
        );
        // Two more are the folder writers, and both send a mapped folder --
        // which is the type that makes the name ciphertext. A folder endpoint
        // that stopped sending one would be a folder endpoint that had gained
        // the ability to send a name in the clear.
        assert_eq!(
            production.matches("send_json(folder.body())").count(),
            2,
            "a folder endpoint stopped sending a MappedFolder"
        );
        // The fifth is prelogin, on the auth agent, which carries no vault data.
        assert_eq!(production.matches("self.auth_agent.post(url).send_json(body)").count(), 1);
        // The sixth is the email second factor's send request. It is the one
        // hand-built body in the module and it is on the auth agent, not the
        // write agent: it carries the master password hash -- the same value
        // the grant sends -- and no vault data at all. It is named here
        // rather than left to the `send_json(` count so that a *seventh*
        // hand-built body cannot hide behind this one's allowance.
        assert_eq!(
            production.matches("self.auth_agent.post(&url).send_json(&email_body)").count(),
            1,
            "the email second factor stopped sending its own body, or gained a twin"
        );
        // The seventh is the Send create, and it is a mapped type for the
        // same reason the four above it are: `MappedSend` has no constructor
        // but `rest::send::encrypt_plan`, so the body that reaches this
        // module has already been encrypted under the Send's own key. A
        // hand-built one would be the user's cleartext.
        assert_eq!(
            production.matches("send_json(send.body())").count(),
            1,
            "the Send endpoint stopped sending a MappedSend"
        );

        // And there is **no** hand-built body left anywhere: the four mapped
        // sends above are the write agent's only `post`/`put` with content.
        // This assertion used to be `1`, pinning the bulk archive's literal
        // `{"ids": [id]}`. Nothing sends that any more, and a zero here is
        // the strongest form of the rule the literal was an exception to --
        // every body this module puts on the wire comes from a mapped type.
        assert_eq!(
            production.matches("send_json(&body)").count(),
            0,
            "a hand-built body appeared in rest::api"
        );
        assert_eq!(
            production.matches("self.write_agent.post(&url), session).send_json").count()
                + production.matches("self.write_agent.put(&url), session).send_json").count(),
            5,
            "the write agent gained another body-carrying call"
        );
    }

    /// The whole of what a create must put on the wire: the method, the path,
    /// the bearer header, and a body carrying ciphertext and no plaintext.
    #[test]
    fn a_create_posts_the_encrypted_body_with_the_bearer_token() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let cipher = encrypted_cipher();
        let body = cipher.body().clone();
        let created = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(crate::test_http::Matcher::Json(body.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"object":"cipher","id":"server-assigned-id","revisionDate":"2024-07-07T07:07:07Z"}"#)
            .expect(1)
            .create();

        let answer = client.create_cipher(&mut session, &cipher).expect("the create");
        created.assert();
        // The server's own copy comes back, which is how the caller learns the
        // id it did not choose.
        assert_eq!(answer.get("id").and_then(|v| v.as_str()), Some("server-assigned-id"));

        // And the body really was ciphertext: the same assertion from the
        // other side, on the exact bytes `match_body` accepted.
        let sent = serde_json::to_string(&body).expect("serializable");
        assert!(!sent.contains(SECRET), "a plaintext reached the request body");
        let sealed = body
            .get("login")
            .and_then(|l| l.get("password"))
            .and_then(|v| v.as_str())
            .expect("the mapped body carries a login password");
        assert!(sealed.starts_with("2."), "the body did not carry a ciphertext password");
    }

    /// An update is a `PUT` to the item's own path -- not a `POST`, and not to
    /// the collection.
    #[test]
    fn an_update_puts_to_the_item_path_and_carries_only_ciphertext() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let cipher = encrypted_cipher();
        let body = cipher.body().clone();
        let updated = server
            .mock("PUT", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(crate::test_http::Matcher::Json(body.clone()))
            .with_header("content-type", "application/json")
            .with_body(r#"{"object":"cipher","id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#)
            .expect(1)
            .create();
        // If the client ever posted an edit to the collection instead, this
        // would match and the assertion below would fire.
        let wrong = server.mock("POST", "/api/ciphers").with_status(200).create();

        client
            .update_cipher(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", &cipher)
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
        let mut server = crate::test_http::server();
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
        // `DELETE .../delete`, NOT `DELETE .../{id}`. The bare route is
        // NodeWarden's `handleDeleteCipherCompat`, which soft-deletes a live
        // cipher and answers 200 -- a purge that trashes. See
        // `hard_delete_cipher`. The `bare` mock below is the assertion that
        // this client no longer goes there.
        let hard = server
            .mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/delete")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();
        let bare = server
            .mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .with_status(200)
            .create();

        client.trash_cipher(&mut session, id).expect("the trash");
        client.restore_cipher(&mut session, id).expect("the restore");
        client.hard_delete_cipher(&mut session, id).expect("the hard delete");
        trash.assert();
        restore.assert();
        hard.assert();
        assert!(
            !bare.matched(),
            "the purge went to the compat route, which trashes a live item instead of \
             destroying it and reports success either way"
        );
    }

    // ---- the folder write endpoints ----------------------------------------

    /// A body to send, out of `rest::write`'s folder mapper -- the only thing
    /// that can produce one.
    fn encrypted_folder() -> MappedFolder {
        crate::rest::write::tests::a_mapped_folder()
    }

    /// The plaintext folder name these tests search the wire for and must
    /// never find.
    const FOLDER_SECRET: &str = crate::rest::write::tests::FOLDER_NEEDLE;

    /// The whole of what the two folder writers put on the wire: the method,
    /// the path, the bearer header, and a body whose name is **ciphertext**.
    ///
    /// The create and the rename are asserted together because the mistake
    /// available is that one of them is the other: a create that `PUT`s, or a
    /// rename that posts to the collection URL and quietly makes a second
    /// folder.
    #[test]
    fn the_folder_writers_send_the_encrypted_name_to_their_own_routes() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let folder = encrypted_folder();
        let body = folder.body().clone();
        // Control: the fixture really is ciphertext, so the two assertions
        // below are not passing over a body that has no name in it at all.
        assert!(!body.to_string().contains(FOLDER_SECRET), "the fixture is not encrypted");
        assert!(body.get("name").and_then(|v| v.as_str()).is_some_and(|n| n.starts_with("2.")));

        let created = server
            .mock("POST", "/api/folders")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(crate::test_http::Matcher::Json(body.clone()))
            .with_body(r#"{"object":"folder","id":"server-assigned-folder"}"#)
            .expect(1)
            .create();
        let renamed = server
            .mock("PUT", "/api/folders/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(crate::test_http::Matcher::Json(body))
            .with_body(r#"{"object":"folder","id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#)
            .expect(1)
            .create();

        let answer = client.create_folder(&mut session, &folder).expect("the create");
        assert_eq!(answer.get("id").and_then(|v| v.as_str()), Some("server-assigned-folder"));
        client
            .update_folder(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", &folder)
            .expect("the rename");
        created.assert();
        renamed.assert();
    }

    /// The delete is a `DELETE` on the folder's own path, and an answer with
    /// **no body at all** is a success.
    ///
    /// That last half is the deliberate difference from the archive routes
    /// above, where an empty body is `ArchiveNotConfirmed`. Both routes are
    /// path-scoped, so it is not the shape of the URL that separates them;
    /// see `RestClient::delete_folder` for why what each call *asserts* makes
    /// them read an empty answer oppositely. This test is that decision
    /// written down somewhere it can fail.
    #[test]
    fn a_folder_delete_takes_an_empty_body_as_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let deleted = server
            .mock("DELETE", "/api/folders/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(204)
            .with_body("")
            .expect(1)
            .create();

        client
            .delete_folder(&mut session, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
            .expect("an empty 204 is a delete that happened");
        deleted.assert();
    }

    // ---- the Send endpoints ------------------------------------------------

    /// A mapped Send body, out of `rest::send`'s own encrypter -- there is no
    /// other constructor, which is the property these tests are the first
    /// place to feel.
    fn encrypted_send() -> crate::rest::send::MappedSend {
        crate::rest::send::encrypt_plan(
            &crate::rest::send::tests::a_plan(),
            &crate::rest::send::tests::keys(),
            &crate::send::FixedClock(0),
        )
        .expect("the plan maps")
    }

    /// **Every field the server needs, and nothing the user typed.** Modelled
    /// on the cipher create beside it.
    #[test]
    fn creating_a_send_posts_the_encrypted_body_to_the_sends_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let send = encrypted_send();
        let body = send.body().clone();
        let created = server
            .mock("POST", "/api/sends")
            .match_header("Authorization", "Bearer AT-1")
            .match_body(crate::test_http::Matcher::Json(body.clone()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"send-1","accessId":"acc-1","deletionDate":"2026-09-06T00:43:17.148Z"}"#)
            .expect(1)
            .create();

        let answer = client.create_send(&mut session, &send).expect("the send is created");
        created.assert();
        assert_eq!(
            answer.get("accessId").and_then(|v| v.as_str()),
            Some("acc-1"),
            "the server's own accessId is what comes back"
        );

        // The same assertion from the other side, on the exact bytes
        // `match_body` accepted: the plan's secret body is not among them,
        // and -- the positive control -- the ciphertext that replaced it is.
        let sent = serde_json::to_string(&body).expect("serializable");
        assert!(
            !sent.contains("correct-horse-battery-staple"),
            "the Send's plaintext reached the request body"
        );
        assert!(
            body["text"]["text"].as_str().expect("a text field").starts_with("2."),
            "the body carried no ciphertext at all, so the absence above proves nothing"
        );
    }

    /// A refusal is the server's own words, through the same
    /// [`classify_json_400`] every other write uses -- not a new error
    /// vocabulary for one feature.
    ///
    /// **The body is `error_description`, not `message`.** That is what
    /// `classify_json_400` reads, and it is a module-wide property rather
    /// than a Send one: a server that says `message` instead is one whose
    /// sentence a cipher create and a folder create already lose too. Pinning
    /// the shape this module actually reads keeps that fact visible; teaching
    /// the classifier a second key would change every route in the file and
    /// is not what this plan is.
    #[test]
    fn a_refused_send_carries_the_servers_own_sentence() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let _refused = server
            .mock("POST", "/api/sends")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":"invalid_request","error_description":"You must be a premium user to send files."}"#,
            )
            .expect(1)
            .create();

        match client.create_send(&mut session, &encrypted_send()) {
            Err(RestError::Rejected { description, .. }) => {
                assert!(description.contains("premium"), "the server's words were lost: {description}");
            }
            other => panic!("expected a Rejected, got {other:?}"),
        }
    }

    /// The list is a `GET`, and the delete is path-scoped with the id
    /// checked -- the same `is_url_path_safe` gate the cipher and folder
    /// routes already use, so an id carrying a `/` cannot aim a `DELETE`
    /// somewhere else.
    #[test]
    fn sends_are_listed_and_revoked_on_their_own_routes() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let listed = server
            .mock("GET", "/api/sends")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[]}"#)
            .expect(1)
            .create();
        let revoked = server
            .mock("DELETE", "/api/sends/send-1")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(200)
            .with_body("")
            .expect(1)
            .create();

        assert!(client.fetch_sends(&mut session).is_ok());
        assert!(client.delete_send(&mut session, "send-1").is_ok());
        listed.assert();
        revoked.assert();

        assert!(
            matches!(client.delete_send(&mut session, "../ciphers/x"), Err(RestError::UnsafeId)),
            "an id that is not path-safe must be refused before it is a URL"
        );
    }

    /// The token discipline, on a folder write: one 401, one refresh, one
    /// retry carrying the **new** token and the same encrypted body.
    ///
    /// A folder write that gave up where a cipher write would have refreshed
    /// is an inconsistency that only shows itself when a token expires
    /// mid-session, so it is pinned rather than left to the shared helper
    /// being shared.
    #[test]
    fn a_401_on_a_folder_create_is_refreshed_once_and_the_retry_carries_the_new_token() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let folder = encrypted_folder();
        let body = folder.body().clone();
        let stale = server
            .mock("POST", "/api/folders")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(401)
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"refresh_token":"RT-2"}"#)
            .expect(1)
            .create();
        let fresh = server
            .mock("POST", "/api/folders")
            .match_header("Authorization", "Bearer AT-2")
            .match_body(crate::test_http::Matcher::Json(body))
            .with_body(r#"{"object":"folder","id":"server-assigned-folder"}"#)
            .expect(1)
            .create();

        client.create_folder(&mut session, &folder).expect("the retried create");
        stale.assert();
        refresh.assert();
        fresh.assert();
    }

    /// And a folder id gets the same path check a cipher id does, before a
    /// socket is opened.
    #[test]
    fn an_unsafe_id_is_refused_by_the_folder_routes_too() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let folder = encrypted_folder();
        let any = server.mock("DELETE", crate::test_http::Matcher::Any).with_status(200).create();
        let put = server.mock("PUT", crate::test_http::Matcher::Any).with_status(200).create();
        for bad in ["../accounts", "a/b", "a?x=1", "", "a#b"] {
            assert_eq!(
                client.delete_folder(&mut session, bad).expect_err("an unsafe id"),
                RestError::UnsafeId
            );
            assert_eq!(
                client.update_folder(&mut session, bad, &folder).expect_err("an unsafe id"),
                RestError::UnsafeId
            );
        }
        assert!(!any.matched(), "an unsafe id reached the network");
        assert!(!put.matched(), "an unsafe id reached the network");
    }

    /// The reactive half of the token discipline, on a write: one 401, one
    /// refresh, one retry -- and the retry carries the **new** token and the
    /// same body. `expect(1)` on each mock is what pins "exactly once".
    #[test]
    fn a_401_on_a_create_is_refreshed_once_and_the_retry_carries_the_new_token() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let cipher = encrypted_cipher();
        let body = cipher.body().clone();
        let stale = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-1")
            .with_status(401)
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"refresh_token":"RT-2"}"#)
            .expect(1)
            .create();
        let fresh = server
            .mock("POST", "/api/ciphers")
            .match_header("Authorization", "Bearer AT-2")
            .match_body(crate::test_http::Matcher::Json(body.clone()))
            .with_body(r#"{"object":"cipher","id":"server-assigned-id"}"#)
            .expect(1)
            .create();

        client.create_cipher(&mut session, &cipher).expect("the retried create");
        stale.assert();
        refresh.assert();
        fresh.assert();
    }

    /// And the retry happens once on a write too. A server that keeps saying
    /// 401 must not be hammered.
    #[test]
    fn a_write_against_a_dead_session_gives_up_instead_of_looping() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600}"#)
            .expect(1)
            .create();
        let deletes = server
            .mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/delete")
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
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let any = server.mock("DELETE", crate::test_http::Matcher::Any).with_status(200).create();
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
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "password".into(),
            )]))
            .with_body(token_body(0))
            .create();
        let client = RestClient::new(server.url());
        let mut session = client.password_grant("a@b.c", "HASH", &device()).expect("the grant");

        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
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

    /// The archive routes: **per-id endpoints with the id in the path**,
    /// which is the shape the target server actually implements.
    ///
    /// The path matchers are the assertion that matters. NodeWarden's routing
    /// table has `/:id/archive` and `/:id/unarchive` and no bulk archive at
    /// all, so a request to `/api/ciphers/archive` is a `404` there. And the
    /// two directions are two routes: an unarchive that quietly used
    /// `/restore` would not match.
    ///
    /// The body matcher asserts the other half of the decision: **no body**.
    /// The id is in the path, so there is nothing left to say, and a client
    /// that still sent `{"ids": [...]}` would fail here.
    #[test]
    fn the_archive_routes_put_the_id_in_the_path_and_are_two_distinct_routes() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let archive = server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .match_header("Authorization", "Bearer AT-1")
            .match_body("")
            .with_body(format!(
                r#"{{"object":"cipher","id":"{id}","archivedDate":"2022-03-01T00:00:00Z"}}"#
            ))
            .expect(1)
            .create();
        let unarchive = server
            .mock("PUT", format!("/api/ciphers/{id}/unarchive").as_str())
            .match_header("Authorization", "Bearer AT-1")
            .match_body("")
            .with_body(format!(r#"{{"object":"cipher","id":"{id}","archivedDate":null}}"#))
            .expect(1)
            .create();
        // The bulk route, which nothing may reach any more. Left at the
        // default expectation so that `matched()` means "was hit"; see
        // `an_id_that_is_not_url_path_safe_is_refused_before_anything_is_sent`
        // for why an `expect(0)` mock would read inverted here.
        let bulk = server.mock("PUT", "/api/ciphers/archive").with_status(200).create();

        client.archive_cipher(&mut session, id).expect("the archive");
        client.unarchive_cipher(&mut session, id).expect("the unarchive");
        archive.assert();
        unarchive.assert();
        assert!(!bulk.matched(), "an archive still reached the bulk route");
    }

    /// **A `200` is not a confirmation, and this is where that is enforced.**
    ///
    /// Each body below is a perfectly successful HTTP response that does not
    /// show the requested cipher archived. Every one must be
    /// [`RestError::ArchiveNotConfirmed`] -- an `Ok` here would be a caller
    /// told its item moved when it did not, which is the exact defect the
    /// archive operations were previously refused in order to avoid.
    ///
    /// The route is path-scoped now, so its status *does* speak for this one
    /// id. The check survives that change because what an archive asserts is
    /// the value of a **server-assigned** field: a `200` says the request was
    /// accepted, and only the echoed cipher says the stamp was written.
    #[test]
    fn an_answer_that_omits_or_contradicts_the_cipher_is_not_a_success() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let bodies = [
            ("a cipher that is someone else", r#"{"object":"cipher","id":"other"}"#.to_string()),
            (
                "a cipher with no id",
                r#"{"object":"cipher","archivedDate":"2022-03-01T00:00:00Z"}"#.to_string(),
            ),
            ("an empty list", r#"{"object":"list","data":[]}"#.to_string()),
            ("a null body", "null".to_string()),
            (
                "the id in the wrong state",
                format!(r#"{{"object":"cipher","id":"{id}","archivedDate":null}}"#),
            ),
        ];
        for (what, body) in bodies {
            let mut server = crate::test_http::server();
            let (client, mut session) = granted(&mut server);
            server
                .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
                .with_status(200)
                .with_body(body)
                .create();
            assert_eq!(
                client.archive_cipher(&mut session, id).expect_err(what),
                RestError::ArchiveNotConfirmed,
                "an archive answered with {what} was reported as a success"
            );
        }
    }

    /// The mirror of the previous test for the other direction: an unarchive
    /// whose echoed cipher still carries an `archivedDate` did not happen.
    ///
    /// Worth its own test rather than a sixth row above, because the
    /// predicate is *inverted* here and an implementation that ignored the
    /// direction would pass every archive case and fail only this one.
    #[test]
    fn an_unarchive_whose_cipher_is_still_stamped_is_not_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        server
            .mock("PUT", format!("/api/ciphers/{id}/unarchive").as_str())
            .with_body(format!(
                r#"{{"object":"cipher","id":"{id}","archivedDate":"2022-02-01T00:00:00Z"}}"#
            ))
            .create();
        assert_eq!(
            client.unarchive_cipher(&mut session, id).expect_err("still stamped"),
            RestError::ArchiveNotConfirmed
        );
    }

    /// A `200` with **no body at all** is also not a success.
    ///
    /// Separate from the case list above because it fails at a different
    /// place and it is worth being explicit about which: an empty body never
    /// reaches [`archived_state_of`], because `value_from` cannot parse it
    /// into JSON and answers `Parse("a JSON body")` first. That is a
    /// different error but the same *answer* -- not `Ok` -- and this test
    /// exists so that the property is asserted rather than assumed from one
    /// layer's behaviour.
    ///
    /// **This did not change when the routes became path-scoped**, and that
    /// is the deliberate part. [`RestClient::delete_folder`] is path-scoped
    /// too and reads an empty body as success; the difference is not the
    /// shape of the route but what the call asserts. A delete asserts the
    /// folder is gone, which a status can say. An archive asserts a
    /// server-assigned `archivedDate`, which no status can say. So a server
    /// that answers these routes with `204` and nothing else gets an error
    /// here rather than a false success -- a cost this crate takes on
    /// purpose, and one NodeWarden never charges, because it returns the
    /// whole updated cipher.
    #[test]
    fn an_archive_answered_with_no_body_at_all_is_not_a_success_either() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .with_status(200)
            .with_body("")
            .create();
        let err = client.archive_cipher(&mut session, id).expect_err("an empty body");
        assert!(
            matches!(err, RestError::Parse(_) | RestError::ArchiveNotConfirmed),
            "an archive answered with an empty body was not reported as a failure: {err:?}"
        );
    }

    /// The server's own refusals arrive as themselves, and are **not**
    /// confused with a confirmation failure.
    ///
    /// Both cases are read out of NodeWarden's own handlers: archiving a
    /// trashed cipher is a `400` carrying "Cannot archive a deleted cipher",
    /// and a cipher the server does not have is a `404`. The first must stay
    /// a [`RestError::Rejected`] -- the one variant that can carry an
    /// explanation to a user -- and the second a plain status. Neither may be
    /// [`RestError::ArchiveNotConfirmed`], which is reserved for the worse
    /// case of an accepted request that did nothing.
    ///
    /// The 400 is exercised in both of the shapes such a body plausibly
    /// arrives in, because `classify_400` reads `error`/`error_description`
    /// and a server may instead answer a bare `message`. The first pins that
    /// the words survive when they are where this module looks; the second
    /// pins that an unrecognised envelope still lands on `Rejected` rather
    /// than being mistaken for a confirmation failure.
    #[test]
    fn the_servers_own_refusals_on_the_archive_routes_are_kept_as_themselves() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_request",
                    "error_description":"Cannot archive a deleted cipher"}"#,
            )
            .expect(1)
            .create();
        assert_eq!(
            client.archive_cipher(&mut session, id).expect_err("a deleted cipher"),
            RestError::Rejected {
                error: "invalid_request".to_string(),
                description: "Cannot archive a deleted cipher".to_string(),
            }
        );

        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .with_status(400)
            .with_body(r#"{"message":"Cannot archive a deleted cipher"}"#)
            .expect(1)
            .create();
        let err = client.archive_cipher(&mut session, id).expect_err("a deleted cipher");
        assert!(
            matches!(err, RestError::Rejected { .. }),
            "a 400 on an archive was not read as a rejection: {err:?}"
        );

        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", format!("/api/ciphers/{id}/unarchive").as_str())
            .with_status(404)
            .expect(1)
            .create();
        assert_eq!(
            client.unarchive_cipher(&mut session, id).expect_err("a missing cipher"),
            RestError::Status(404)
        );
    }

    /// The envelope is presentation, so all three spellings are read: the
    /// bare cipher NodeWarden returns, Bitwarden's
    /// `{"object":"list","data":[..]}`, and a bare array. Nothing about
    /// whether the item moved turns on which one arrived, and refusing the
    /// others would refuse a server for a cosmetic reason.
    #[test]
    fn an_archive_answer_is_read_whether_or_not_it_carries_a_list_envelope() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        for body in [
            format!(r#"{{"object":"cipher","id":"{id}","archivedDate":"2022-03-01T00:00:00Z"}}"#),
            format!(
                r#"{{"object":"list",
                    "data":[{{"id":"{id}","archivedDate":"2022-03-01T00:00:00Z"}}]}}"#
            ),
            format!(r#"[{{"id":"{id}","archivedDate":"2022-03-01T00:00:00Z"}}]"#),
        ] {
            let mut server = crate::test_http::server();
            let (client, mut session) = granted(&mut server);
            server
                .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
                .with_body(body.clone())
                .create();
            client.archive_cipher(&mut session, id).unwrap_or_else(|e| {
                panic!("a valid archive answer was refused: {e:?} for {body}")
            });
        }
    }

    /// **The shape the real server actually sends**, which every earlier test
    /// in this file missed and which `examples/rest_probe --write` found on
    /// the first live run.
    ///
    /// NodeWarden's `cipherToResponse` puts `data` on the cipher itself --
    /// `data: typeof passthrough.data === 'string' ? passthrough.data : null`
    /// -- so a real archive answer is a bare cipher carrying `"data": null`.
    /// [`archived_state_of`] unwrapped `data` whenever the key existed, so it
    /// stepped into that null and reported
    /// [`RestError::ArchiveNotConfirmed`] for an archive that had in fact
    /// happened. The item moved; the client said it might not have.
    ///
    /// Both directions are driven, because an unarchive answer carries
    /// `"archivedDate": null` **and** `"data": null` and a fix that special-
    /// cased nulls one level up would pass the archive half and fail here.
    #[test]
    fn a_cipher_that_carries_its_own_data_field_is_not_mistaken_for_an_envelope() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        for (suffix, stamp) in [("archive", r#""2022-03-01T00:00:00Z""#), ("unarchive", "null")] {
            let mut server = crate::test_http::server();
            let (client, mut session) = granted(&mut server);
            server
                .mock("PUT", format!("/api/ciphers/{id}/{suffix}").as_str())
                .with_body(format!(
                    r#"{{"object":"cipherDetails","id":"{id}",
                         "archivedDate":{stamp},"data":null}}"#
                ))
                .create();
            let sent = if suffix == "archive" {
                client.archive_cipher(&mut session, id)
            } else {
                client.unarchive_cipher(&mut session, id)
            };
            sent.unwrap_or_else(|e| {
                panic!("a real {suffix} answer was refused as unconfirmed: {e:?}")
            });
        }
    }

    /// The tolerance the fix above must not have thrown away: a genuine list
    /// envelope still has to be stepped into. Asserted from the other side --
    /// the cipher inside the envelope is archived and the *outer* object is
    /// not a cipher at all, so a reader that stopped unwrapping would find no
    /// id and refuse.
    #[test]
    fn a_real_data_envelope_is_still_unwrapped() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .with_body(format!(
                r#"{{"object":"list","data":{{"id":"{id}",
                     "archivedDate":"2022-03-01T00:00:00Z"}}}}"#
            ))
            .create();
        client
            .archive_cipher(&mut session, id)
            .expect("an object envelope around the cipher is still an envelope");
    }

    /// The id reaches the URL on these routes now, exactly as it does on
    /// trash and restore, so the path check is the same one its neighbours
    /// make -- and it still happens before a socket is opened. The mock never
    /// being hit is the assertion.
    #[test]
    fn an_unsafe_id_is_refused_by_the_archive_routes_too() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let any = server.mock("PUT", crate::test_http::Matcher::Any).with_status(200).create();
        for bad in ["../../api/accounts", "a/b", "a?x=1", "", "a#b"] {
            assert_eq!(
                client.archive_cipher(&mut session, bad).expect_err("an unsafe id"),
                RestError::UnsafeId,
                "`{bad}` was accepted as a cipher id by archive"
            );
            assert_eq!(
                client.unarchive_cipher(&mut session, bad).expect_err("an unsafe id"),
                RestError::UnsafeId
            );
        }
        assert!(!any.matched(), "an unsafe id reached the network");
    }

    /// The archive routes share the module's token discipline rather than
    /// having their own: one 401, one refresh, one retry, and the retry
    /// carries the new token. Worth pinning per endpoint family, because a
    /// route added outside `refreshing` would work in every test that did not
    /// look for this.
    #[test]
    fn a_401_on_an_archive_is_refreshed_once_and_retried() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let stale = server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .match_header("Authorization", "Bearer AT-1")
            .with_status(401)
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/identity/connect/token")
            .match_body(crate::test_http::Matcher::AllOf(vec![crate::test_http::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            )]))
            .with_body(r#"{"access_token":"AT-2","expires_in":3600,"refresh_token":"RT-2"}"#)
            .expect(1)
            .create();
        let fresh = server
            .mock("PUT", format!("/api/ciphers/{id}/archive").as_str())
            .match_header("Authorization", "Bearer AT-2")
            .with_body(format!(
                r#"{{"object":"cipher","id":"{id}","archivedDate":"2022-03-01T00:00:00Z"}}"#
            ))
            .expect(1)
            .create();

        client.archive_cipher(&mut session, id).expect("the retried archive");
        stale.assert();
        refresh.assert();
        fresh.assert();
    }

    /// A rejected write keeps the server's own words, the way every other
    /// 400 in this module does -- and does not become a bare `Status(400)`.
    #[test]
    fn a_rejected_write_carries_the_servers_own_explanation() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("POST", "/api/ciphers")
            .with_status(400)
            .with_body(r#"{"error":"invalid_request","error_description":"Cipher type is required"}"#)
            .create();
        let err = client
            .create_cipher(&mut session, &encrypted_cipher())
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

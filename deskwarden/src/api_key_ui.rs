//! **Signing in with a personal API key** -- the way in for the accounts whose
//! second factor this app cannot complete (Duo, WebAuthn).
//!
//! Two stages, because they are two different things: the key pair
//! authenticates ([`crate::rest::api::RestClient::api_key_grant`]) and the
//! master password decrypts. Both, always. See
//! `docs/superpowers/specs/2026-08-29-signing-in-with-an-api-key-design.md`.
//!
//! # The client secret
//!
//! It is a permanent, password-free login to the account. It is handled here
//! exactly as the master password is: [`Zeroizing`], wiped on [`Drop`], on a
//! struct with no `Debug`, never formatted, never logged, never in an error
//! string, and **never written to disk**. What survives a restart is the
//! session token, by the same route every other session token takes.
//! `the_client_secret_is_handled_like_a_password` reads this file's own source
//! and asserts all of that.
//!
//! # Why every public name here is prefixed
//!
//! `crate::debug_leak_guard` matches type names **crate-wide and by bare
//! name**, so two types called `Refusal` in two modules are conflated and both
//! are flagged. This module holds three secrets, so it is exactly the module
//! that must not lose a real flag to a name collision: `ApiKeyStep`,
//! `ApiKeyRefusal`, `ApiKeyAccount`, `ApiKeyCommand`, `ApiKeyReport` and
//! `ApiKeyAction` are all spelled long for that reason, and `Refusal`,
//! `Account`, `Step`, `Command` and `Report` are every one of them already
//! taken elsewhere in this crate.

use crate::rest::api::RestError;
use zeroize::{Zeroize, Zeroizing};

/// Which of the two things the user is being asked for.
///
/// A step and not a bool, because the failures are asymmetric and the type is
/// where that asymmetry is written down: a rejected key pair returns to
/// [`ApiKeyStep::KeyPair`], a rejected password returns to
/// [`ApiKeyStep::MasterPassword`] and does **not** repeat stage 1, because
/// nothing about stage 1 failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyStep {
    /// `client_id` and `client_secret`, exchanged for a session.
    KeyPair,
    /// The master password, which is what actually decrypts the vault.
    MasterPassword,
}

/// Everything the API-key sign-in is, as state.
///
/// **No `Debug`, derived or otherwise.** Two of the three fields are
/// credentials and the third names an account; nothing in here is a
/// formatter's business. `crate::debug_leak_guard` fails the suite for a
/// derived `Debug` over a [`Zeroizing`] field, and
/// `the_client_secret_is_handled_like_a_password` fails it for a hand-written
/// one.
pub struct ApiKeyForm {
    /// The key's public half. Not a secret -- it is `user.<guid>` and the
    /// server treats it as a username -- but it is still an account
    /// identifier, so nothing formats it either.
    pub client_id: String,
    /// **The key's secret half: a permanent, password-free login to this
    /// account.** It does not expire, it is not covered by the second factor
    /// it exists to bypass, and it is never persisted. See the module docs.
    pub secret: Zeroizing<String>,
    /// The master password. Stage 2's whole subject, and the only thing that
    /// can decrypt the vault.
    pub password: Zeroizing<String>,
    pub step: ApiKeyStep,
    /// The inline message under the fields, or `None`. See [`message_for`].
    pub error: Option<String>,
    /// True while a grant or an unlock is in flight: the buttons ghost,
    /// exactly as the sign-in card's do.
    pub busy: bool,
}

/// Three fields' worth of credential, on a struct move-captured by a frame
/// closure that lives as long as the window. `login_ui::LoginForm` states the
/// same rule for the same reason.
impl Drop for ApiKeyForm {
    fn drop(&mut self) {
        // `Zeroizing` wipes `secret` and `password` itself. The id and the
        // message are not credentials, and are wiped anyway rather than
        // released to the allocator naming an account.
        self.client_id.zeroize();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
    }
}

impl Default for ApiKeyForm {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyForm {
    pub fn new() -> Self {
        Self {
            client_id: String::new(),
            secret: Zeroizing::new(String::new()),
            password: Zeroizing::new(String::new()),
            step: ApiKeyStep::KeyPair,
            error: None,
            busy: false,
        }
    }

    /// Whether stage 1 has something to send. Trimmed, because both of these
    /// arrive by paste from a web page and routinely carry whitespace the
    /// server would reject.
    pub fn key_pair_ready(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.secret.trim().is_empty()
    }

    /// Whether stage 2 has something to try. Trimmed only for the emptiness
    /// check -- a master password's own leading space is part of it and is
    /// **not** trimmed when it is sent.
    pub fn password_ready(&self) -> bool {
        !self.password.trim().is_empty()
    }
}

/// Why the API-key sign-in stopped.
///
/// Three variants and not one `String`, because the *behaviour* differs and
/// not only the wording: each names a different stage to return to and a
/// different set of fields to keep. A `String` error would put that decision
/// in the caller, where nothing tests it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyRefusal {
    /// The grant said no: the id or the secret is wrong, or the key has been
    /// rotated in the web vault. **Both fields are kept.**
    KeyPairRejected,
    /// The session is good and the key is not the problem: the master password
    /// did not unwrap this account's user key. Stage 1 is not repeated.
    PasswordRejected,
    /// Neither call got an answer at all. **Not a rejected credential** --
    /// nothing the user typed was refused. The same distinction
    /// [`crate::rest::api::RestError::CodeNotSent`] makes for the email code.
    Unreachable,
}

/// One line the user can act on, per refusal.
///
/// `&'static str` and not a formatted `String`, deliberately: a formatted
/// message is a place a credential can be interpolated into, and these three
/// are the only strings this feature ever shows a user about a failure.
pub fn message_for(refusal: ApiKeyRefusal) -> &'static str {
    match refusal {
        ApiKeyRefusal::KeyPairRejected => {
            "That API key wasn't accepted. Check the client id and client secret \u{2014} and \
             if the key has been rotated in the web vault, create a new one under Account \
             settings \u{2192} Security \u{2192} Keys."
        }
        ApiKeyRefusal::PasswordRejected => {
            "That master password didn't unlock this account. The key pair is fine \u{2014} \
             only the password needs retyping."
        }
        ApiKeyRefusal::Unreachable => {
            "Couldn't reach the server. Check your connection \u{2014} and the server URL, if \
             this is a self-hosted account."
        }
    }
}

impl ApiKeyForm {
    /// Applies a refusal to the form: the message, the stage to return to, and
    /// the fields to clear.
    ///
    /// **The whole of the design's error-state section is this one `match`**,
    /// which is why it is here and not spread across the draw function and the
    /// worker.
    pub fn refused(&mut self, refusal: ApiKeyRefusal) {
        self.busy = false;
        self.error = Some(message_for(refusal).to_string());
        match refusal {
            // Back to stage 1, holding both fields: the user is likelier to
            // have mistyped the short id than the pasted secret, and they can
            // see both to tell.
            ApiKeyRefusal::KeyPairRejected => {
                self.step = ApiKeyStep::KeyPair;
                self.password.zeroize();
            }
            // Stage 2 only. The session minted by stage 1 is still good and is
            // still held by the worker.
            ApiKeyRefusal::PasswordRejected => {
                self.step = ApiKeyStep::MasterPassword;
                self.password.zeroize();
            }
            // Nothing the user typed was refused, so nothing is cleared and
            // nothing moves. The button comes back and they press it again.
            ApiKeyRefusal::Unreachable => {}
        }
    }
}

/// What a failed [`crate::rest::api::RestClient::api_key_grant`] means to the
/// user.
///
/// Everything the server actively refused is the key pair's fault, because the
/// key pair is the only thing this call sends that a user can get wrong: there
/// is no username and no password in a `client_credentials` grant. Everything
/// else -- no answer, or an answer this client cannot read -- is
/// [`ApiKeyRefusal::Unreachable`], which asks the user to check their
/// connection rather than a secret that may be perfectly correct.
///
/// **No arm formats the error.** `RestError`'s `Display` carries a status and
/// a route, but this function's whole output is an [`ApiKeyRefusal`], so
/// nothing the server said can reach a message on the way past.
pub fn grant_refusal(error: &RestError) -> ApiKeyRefusal {
    match error {
        RestError::Transport(_) | RestError::Parse(_) => ApiKeyRefusal::Unreachable,
        _ => ApiKeyRefusal::KeyPairRejected,
    }
}

/// What a failed stage 2 means to the user.
///
/// Stage 2 is prelogin, then a derivation that cannot fail on a wrong
/// password, then a sync, then unwrapping the user key. **The unwrap is the
/// verification**, and a [`RestError::Crypto`] out of it is the only thing in
/// this whole path that means "that master password was wrong".
///
/// A [`RestError::Unauthorized`] here is the session dying, not the password
/// failing -- so it returns [`ApiKeyRefusal::KeyPairRejected`], which is where
/// a new session comes from.
pub fn unlock_refusal(error: &RestError) -> ApiKeyRefusal {
    match error {
        RestError::Crypto(_) => ApiKeyRefusal::PasswordRejected,
        RestError::Transport(_) | RestError::Parse(_) => ApiKeyRefusal::Unreachable,
        RestError::Unauthorized => ApiKeyRefusal::KeyPairRejected,
        _ => ApiKeyRefusal::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A rejected key pair keeps BOTH fields.** Retyping a 64-character
    /// secret because the id had a typo is the behaviour this design exists to
    /// avoid.
    #[test]
    fn the_form_starts_on_the_key_pair_and_knows_when_each_stage_is_answerable() {
        let mut form = ApiKeyForm::new();
        assert_eq!(form.step, ApiKeyStep::KeyPair, "the key pair comes first");
        assert!(!form.key_pair_ready(), "control: an empty form is not submittable");

        form.client_id.push_str("user.9f3c");
        assert!(
            !form.key_pair_ready(),
            "an id without a secret is half a credential, not a submittable one"
        );
        form.secret.push_str("  b7d2ecc  ");
        assert!(
            form.key_pair_ready(),
            "both fields present -- and the whitespace a paste brings must not block it"
        );

        assert!(!form.password_ready(), "control: no password has been typed");
        form.password.push_str("   ");
        assert!(
            !form.password_ready(),
            "whitespace is not a master password; submitting it would spend a round trip              to be told so"
        );
        form.password.push_str("hunter2");
        assert!(form.password_ready());
    }

    /// The password stage never carries the key pair back into view, and the
    /// key-pair stage never shows a password box. Two stages, not one screen
    /// with three fields -- the design's reason is diagnosis.
    #[test]
    fn the_two_steps_are_ordered_and_distinct() {
        assert_ne!(ApiKeyStep::KeyPair, ApiKeyStep::MasterPassword);
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = ApiKeyStep::MasterPassword;
        assert_eq!(
            form.client_id, "user.9f3c",
            "the id is still held: stage 2 failing must not have to re-ask for it"
        );
        assert_eq!(
            form.secret.as_str(),
            "b7d2ecc",
            "control: the secret survives the step change too, so the 'stage 1 is not              repeated' rule is about the STEP and not about lost fields"
        );
    }
    /// The three failures say three different things. A shared "That didn't
    /// work" would tell a user whose Wi-Fi dropped to check a secret that is
    /// correct.
    #[test]
    fn the_three_refusals_do_not_share_a_message() {
        let key_pair = message_for(ApiKeyRefusal::KeyPairRejected);
        let password = message_for(ApiKeyRefusal::PasswordRejected);
        let unreachable = message_for(ApiKeyRefusal::Unreachable);

        assert!(
            key_pair.contains("API key") || key_pair.contains("client secret"),
            "the key-pair failure must name the thing that was refused; got {key_pair:?}"
        );
        assert!(
            key_pair.contains("rotated") || key_pair.contains("web vault"),
            "a rotated key is the commonest cause and the only one with a fix the user can              act on; got {key_pair:?}"
        );
        assert!(password.contains("master password"), "got {password:?}");
        assert!(
            !password.contains("API key"),
            "the password failure must not send the user back to a key that worked;              got {password:?}"
        );
        assert!(
            unreachable.contains("reach") || unreachable.contains("connection"),
            "got {unreachable:?}"
        );
        assert!(
            !unreachable.contains("wrong") && !unreachable.contains("didn't work"),
            "an unreachable server must not read as a rejected credential; got {unreachable:?}"
        );

        let all = [key_pair, password, unreachable];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "two refusals share one message");
            }
        }
    }

    /// **A rejected key pair returns to stage 1 with BOTH fields kept.** This
    /// is the behaviour the design exists to produce.
    #[test]
    fn a_rejected_key_pair_keeps_both_fields() {
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = ApiKeyStep::MasterPassword;
        form.busy = true;

        form.refused(ApiKeyRefusal::KeyPairRejected);

        assert_eq!(form.step, ApiKeyStep::KeyPair, "the key pair is what failed");
        assert_eq!(form.client_id, "user.9f3c", "the id is kept");
        assert_eq!(
            form.secret.as_str(),
            "b7d2ecc",
            "and so is the secret -- retyping 64 characters because the id had a typo is              exactly what this design refuses to charge for"
        );
        assert!(!form.busy, "the buttons come back");
        assert_eq!(
            form.error.as_deref(),
            Some(message_for(ApiKeyRefusal::KeyPairRejected))
        );
    }

    /// **A rejected password returns to stage 2 only.** Stage 1 is not
    /// repeated, because nothing about it failed -- the session is good.
    #[test]
    fn a_rejected_password_does_not_reask_for_the_key_pair() {
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = ApiKeyStep::MasterPassword;
        form.password.push_str("wrong-one");
        form.busy = true;

        form.refused(ApiKeyRefusal::PasswordRejected);

        assert_eq!(
            form.step,
            ApiKeyStep::MasterPassword,
            "the key pair worked; sending the user back to it would be a lie about what failed"
        );
        assert!(form.password.is_empty(), "the wrong password is gone from the box");
        assert_eq!(
            form.client_id, "user.9f3c",
            "control: the key pair is still held, so a retry needs no re-entry"
        );
        assert_eq!(
            form.error.as_deref(),
            Some(message_for(ApiKeyRefusal::PasswordRejected))
        );
    }

    /// An unreachable server changed nothing about what the user typed, so it
    /// clears nothing and moves nothing.
    #[test]
    fn an_unreachable_server_touches_no_field_and_no_step() {
        for step in [ApiKeyStep::KeyPair, ApiKeyStep::MasterPassword] {
            let mut form = ApiKeyForm::new();
            form.client_id.push_str("user.9f3c");
            form.secret.push_str("b7d2ecc");
            form.password.push_str("hunter2");
            form.step = step;
            form.busy = true;

            form.refused(ApiKeyRefusal::Unreachable);

            assert_eq!(form.step, step, "an unreachable server is not a wrong stage");
            assert_eq!(form.client_id, "user.9f3c");
            assert_eq!(form.secret.as_str(), "b7d2ecc");
            assert_eq!(
                form.password.as_str(),
                "hunter2",
                "nothing the user typed was refused, so nothing they typed is thrown away"
            );
            assert!(!form.busy, "control: the refusal was applied at all");
        }
    }
    /// Stage 1: every way the grant can say no is the key pair's fault, and
    /// the one way it can say nothing is not.
    #[test]
    fn a_refused_grant_is_the_key_pairs_fault_and_a_dead_socket_is_not() {
        use crate::rest::api::RestError;

        for refused in [
            RestError::Unauthorized,
            RestError::InvalidCredentials,
            RestError::Rejected {
                error: "invalid_client".to_string(),
                description: "client_secret is invalid".to_string(),
            },
            RestError::Status(403),
        ] {
            assert_eq!(
                grant_refusal(&refused),
                ApiKeyRefusal::KeyPairRejected,
                "{refused:?} is the server refusing this key pair"
            );
        }

        assert_eq!(
            grant_refusal(&RestError::Transport("dns error".to_string())),
            ApiKeyRefusal::Unreachable,
            "a transport failure is not a wrong secret, and telling the user it was would \
             send them to rotate a key that is fine"
        );
        assert_eq!(
            grant_refusal(&RestError::Parse("the access token")),
            ApiKeyRefusal::Unreachable,
            "an answer this client cannot read is not a credential the user can fix"
        );
    }

    /// Stage 2: the ONLY thing that means "wrong master password" is a crypto
    /// failure unwrapping the user key. There is no grant here to reject it.
    #[test]
    fn only_a_crypto_failure_means_the_master_password_was_wrong() {
        use crate::rest::api::RestError;
        use crate::rest::crypto::CryptoError;

        // The real one. `unwrap_user_key` -> `decrypt` answers `MacMismatch`
        // for a key that is not this account's, which `rest/crypto.rs`'s own
        // wrong-password test already pins.
        assert_eq!(
            unlock_refusal(&RestError::Crypto(CryptoError::MacMismatch)),
            ApiKeyRefusal::PasswordRejected,
            "a user key that will not unwrap IS the wrong-password signal: `master_key` \
             derives a key from any bytes and never fails on one"
        );
        assert_eq!(
            unlock_refusal(&RestError::Crypto(CryptoError::KeyLength { expected: 64, got: 32 })),
            ApiKeyRefusal::PasswordRejected,
            "control: the arm is about `Crypto` and not about one variant of it"
        );
        assert_eq!(
            unlock_refusal(&RestError::Transport("connection reset".to_string())),
            ApiKeyRefusal::Unreachable
        );
        // **The session died, not the password.** A 401 on stage 2 is the
        // token minted by stage 1 having been revoked or expired, so the
        // honest place to send the user is back to the key pair.
        assert_eq!(
            unlock_refusal(&RestError::Unauthorized),
            ApiKeyRefusal::KeyPairRejected,
            "a 401 at stage 2 is a dead session, and a dead session is re-minted from the \
             key pair -- not from the master password"
        );
        // Positive control on the whole function: it does not answer
        // `PasswordRejected` to everything.
        assert_ne!(
            unlock_refusal(&RestError::Transport("x".to_string())),
            ApiKeyRefusal::PasswordRejected,
            "control: unlock_refusal discriminates at all"
        );
    }
}

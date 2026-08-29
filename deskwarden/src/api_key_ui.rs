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
}

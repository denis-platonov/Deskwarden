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

use crate::rest::api::{Authenticated, Device, RestClient, RestError, Session};
use crate::theme;
use eframe::egui::{self, RichText};
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

/// The three things both stages need that the user did not type into this
/// card: they came from the sign-in card, which asked for them first.
///
/// A struct rather than three parameters threaded through four signatures,
/// and no secret in it -- the server URL and the email are what the user typed
/// into the card, and the device id is a stable installation GUID. A derived
/// `Debug` is fine and deliberate: this is what a reader debugging a rejected
/// grant needs to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAccount {
    /// The server the account is on, as the sign-in card has it.
    pub server_url: String,
    /// The account's address. **Stage 2 needs it and stage 1 does not** --
    /// there is no username in a `client_credentials` grant, but the email is
    /// the KDF's salt, so the master password cannot be derived without it.
    pub email: String,
    /// The per-installation device identifier. The same value the password
    /// sign-in sends, so this does not register a second device.
    pub device_id: String,
}

/// The two `rest::` calls this feature needs, as `fn` pointers.
///
/// A struct and not two aliases: a test that faked the grant but let the
/// unlock reach the network would be a test of half a sign-in. A `fn`-pointer
/// struct and not a `cfg(test)` seam, which this crate bans crate-wide, and
/// not a trait object, because there is exactly one production value.
///
/// **Neither signature carries the secret anywhere it can be formatted.** The
/// secret is a `&str` argument, borrowed for one call, and every `Err` here is
/// a `RestError` -- which piece 1 documents as carrying no credential, and
/// which this module reduces to an [`ApiKeyRefusal`] before anything sees it
/// anyway.
#[derive(Clone, Copy)]
pub struct ApiKeySeam {
    /// **Stage 1.** `grant_type=client_credentials`, `scope=api`, the three
    /// device fields. Yields a session and no master key -- see
    /// [`crate::rest::api::RestClient::api_key_grant`] for why that asymmetry
    /// is the whole reason this feature has two stages.
    pub grant:
        fn(&ApiKeyAccount, client_id: &str, client_secret: &str) -> Result<Session, RestError>,
    /// **Stage 2.** Prelogin, derive, sync, unwrap. **Borrows** the session
    /// rather than consuming it: a mistyped master password must be retryable
    /// against the session stage 1 already minted, which is the whole reason
    /// the design does not repeat stage 1. Returns only the master key; the
    /// caller owns the session and pairs the two.
    ///
    /// It was written by-value first, to match [`Authenticated`]'s own field,
    /// and that shape does not merely mis-report a retry -- it **hangs the
    /// worker**. The failed attempt consumed the session, so the retry found
    /// none, answered `KeyPairRejected`, and the loop went back to blocking on
    /// a `recv()` for a key pair the user had no reason to re-enter.
    pub unlock: fn(
        &ApiKeyAccount,
        &Session,
        password: &[u8],
    ) -> Result<crate::rest::crypto::MasterKey, RestError>,
}

/// [`ApiKeySeam::grant`] as production performs it.
///
/// The client secret is borrowed, passed straight through, and never bound to
/// a local -- there is nothing here for a `Drop` to have to wipe.
pub fn grant_direct_rest(
    account: &ApiKeyAccount,
    client_id: &str,
    client_secret: &str,
) -> Result<Session, RestError> {
    let device = Device::windows_desktop(&account.device_id, DEVICE_NAME);
    RestClient::new(&account.server_url).api_key_grant(client_id, client_secret, &device)
}

/// [`ApiKeySeam::unlock`] as production performs it -- **and the master
/// password's only verification.**
///
/// `master_key` cannot fail on a wrong password: it is a KDF, and it derives
/// *a* key from any bytes at all. So the four steps here are not four steps of
/// setup with a check at the end; the last one **is** the check. A wrong
/// password produces a key that will not unwrap this account's protected user
/// key, `unwrap_user_key` answers [`crate::rest::crypto::CryptoError`], and
/// [`unlock_refusal`] turns that into [`ApiKeyRefusal::PasswordRejected`].
///
/// Skipping it would produce an app that is signed in and cannot read
/// anything -- which the design names as worse than being refused, because it
/// looks like success.
///
/// The unwrapped key is dropped on the spot. It is not what this function
/// returns: `rest::sync::VaultKeys::unwrap_from` derives it again from the
/// same [`crate::rest::crypto::MasterKey`] when the vault is actually read,
/// and returning a second copy would be a second key with a life nobody is
/// tracking.
pub fn unlock_direct_rest(
    account: &ApiKeyAccount,
    session: &Session,
    password: &[u8],
) -> Result<crate::rest::crypto::MasterKey, RestError> {
    let client = RestClient::new(&account.server_url);
    let kdf = client.prelogin(&account.email)?;
    let master_key = crate::rest::crypto::master_key(password, &account.email, kdf)?;

    let synced = client.sync(session)?;
    let profile = synced.profile.ok_or(RestError::Parse("the account profile"))?;
    let protected = profile.key.as_deref().ok_or(RestError::Parse("the protected user key"))?;
    let protected: crate::rest::crypto::EncString = protected.parse()?;
    // The verification. The returned key is deliberately dropped here.
    let _ = crate::rest::crypto::unwrap_user_key(&master_key.stretch(), &protected)?;

    Ok(master_key)
}

/// What the user's device list calls this app. The same value
/// `login_ui::DEVICE_NAME` uses, written again rather than imported, because
/// that constant is private to a 11 600-line module this one owes no
/// dependency.
const DEVICE_NAME: &str = "Deskwarden";

/// The seam's one production value, written in exactly one place.
pub const PRODUCTION_API_KEY: ApiKeySeam =
    ApiKeySeam { grant: grant_direct_rest, unlock: unlock_direct_rest };

/// **What the window tells the worker.**
///
/// No `Debug`, for [`ApiKeyForm`]'s reason: two of the three variants carry a
/// credential, and a derived `Debug` would print whatever they let it.
pub enum ApiKeyCommand {
    /// Stage 1. The secret is moved in and dropped when the arm handling it
    /// ends, wiping itself on the way.
    KeyPair { client_id: String, secret: Zeroizing<String> },
    /// Stage 2, against the session stage 1 minted.
    MasterPassword(Zeroizing<String>),
    /// The user backed out or closed the window. The worker drops the session
    /// and returns.
    Abandon,
}

/// **What the worker tells the window.**
///
/// A variant and not a `String`, so the window cannot be handed a message
/// built out of a `RestError` this module has not vetted -- and so the only
/// strings this feature ever shows about a failure are [`message_for`]'s
/// three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyReport {
    /// Stage 1 passed. The card moves to the master password.
    KeyPairAccepted,
    /// One of the three. See [`ApiKeyForm::refused`] for what each does.
    Refused(ApiKeyRefusal),
}

/// **The API-key sign-in, from the worker thread's side.**
///
/// Blocks on the window's commands while holding the [`Session`] stage 1
/// minted, and returns an [`Authenticated`] or nothing. Holding the session is
/// what makes a mistyped master password cheap: stage 1 is not repeated,
/// because nothing about it failed.
///
/// **It blocks a detached thread on a human**, for the code stage's reason and
/// with the code stage's bounds: an [`ApiKeyCommand::Abandon`] and a
/// disconnected channel both return, and returning drops the session.
///
/// **Nothing here is persisted and nothing here is logged with a value in
/// it.** The two `log::warn!` lines carry a stage name and the `RestError`,
/// whose `Display` piece 1 pins as carrying no credential; the refusal the
/// window sees is an enum.
pub fn run_api_key_sign_in(
    seam: &ApiKeySeam,
    account: &ApiKeyAccount,
    commands: &std::sync::mpsc::Receiver<ApiKeyCommand>,
    report: &std::sync::mpsc::Sender<ApiKeyReport>,
) -> Option<Authenticated> {
    // The one session, minted at most once per accepted key pair. `None` is
    // "stage 1 has not passed yet", which is also what makes a stray
    // `MasterPassword` answerable rather than a panic.
    let mut session: Option<Session> = None;

    // `recv()` and not `try_recv()`: this thread has nothing else to do, and a
    // poll loop would spin for as long as it takes somebody to find their web
    // vault. `Err` is a disconnected channel, which is a closed window.
    while let Ok(command) = commands.recv() {
        match command {
            ApiKeyCommand::Abandon => return None,
            ApiKeyCommand::KeyPair { client_id, secret } => {
                match (seam.grant)(account, client_id.trim(), secret.trim()) {
                    Ok(granted) => {
                        session = Some(granted);
                        let _ = report.send(ApiKeyReport::KeyPairAccepted);
                    }
                    Err(e) => {
                        // The route and the status reach the log; the window
                        // gets a variant. Neither half of the key pair is in
                        // either.
                        log::warn!("the API-key grant was not accepted: {e}");
                        let _ = report.send(ApiKeyReport::Refused(grant_refusal(&e)));
                    }
                }
            }
            ApiKeyCommand::MasterPassword(password) => {
                // No session means stage 1 has not passed. Answering
                // `KeyPairRejected` is the true statement: what is missing is
                // the key pair.
                let Some(held) = session.as_ref() else {
                    let _ = report.send(ApiKeyReport::Refused(ApiKeyRefusal::KeyPairRejected));
                    continue;
                };
                match (seam.unlock)(account, held, password.as_bytes()) {
                    Ok(master_key) => {
                        // The session is handed on with the key it pairs with;
                        // `take` is what guarantees there is exactly one.
                        let session = session.take()?;
                        return Some(Authenticated { session, master_key });
                    }
                    Err(e) => {
                        let refusal = unlock_refusal(&e);
                        log::warn!("the API-key sign-in could not unlock the vault: {e}");
                        // A dead session cannot be retried against; a wrong
                        // password can, and does -- the session stays. This is
                        // the line the whole two-stage shape rests on: drop it
                        // and the next password finds nothing to try against.
                        if refusal == ApiKeyRefusal::KeyPairRejected {
                            session = None;
                        }
                        let _ = report.send(ApiKeyReport::Refused(refusal));
                    }
                }
            }
        }
    }
    None
}

pub const KEY_PAIR_TITLE: &str = "Sign in with an API key";
pub const KEY_PAIR_HINT: &str = "Create a personal API key under Account settings \u{2192} \
                                 Security \u{2192} Keys in the Bitwarden web vault, then paste \
                                 both halves here.";
pub const CLIENT_ID_LABEL: &str = "Client ID";
pub const CLIENT_SECRET_LABEL: &str = "Client secret";
pub const PASSWORD_TITLE: &str = "Master password";
pub const PASSWORD_HINT: &str = "The API key signed you in. Your master password is what \
                                 unlocks the vault \u{2014} the key cannot decrypt it.";
pub const PASSWORD_LABEL: &str = "Master password";
pub const CONTINUE_LABEL: &str = "Continue";
pub const BACK_LABEL: &str = "Back";
/// The label on both routes in: the sign-in card's link, and the button on
/// `second_factor_ui`'s unsupported-only card. One constant, so the two places
/// cannot drift into naming the same destination differently.
pub const USE_API_KEY_LABEL: &str = "Sign in with an API key";

/// What the user asked the API-key stage to do this frame. `None` from
/// [`draw`] is the ordinary case: they are still typing.
///
/// No `Debug`: [`ApiKeyAction::Submit`] means "send what is in the form", and
/// while it carries nothing itself, giving this a `Debug` is one refactor away
/// from giving it a field.
pub enum ApiKeyAction {
    /// Submit whatever the current [`ApiKeyStep`] is asking for. The caller
    /// reads `form.step` and builds the [`ApiKeyCommand`] -- the card does not,
    /// because building it would mean copying the secret out of the buffer that
    /// owns it.
    Submit,
    /// Back to the sign-in card.
    Back,
}

/// Draws the API-key stage. Pure view: the caller owns the [`ApiKeyForm`] and
/// performs the channel sends for whatever comes back, exactly as
/// `login_ui::draw_login_window` and its caller are split.
///
/// **`&mut *form.secret` and not a copy.** [`Zeroizing`] is `DerefMut`, so the
/// text edit writes straight into the buffer that owns the secret and there is
/// never a second `String` holding it. That is not full containment and is not
/// claimed as such: `TextEdit` grows the buffer as the user types, and a
/// reallocation copies the old bytes to a new allocation and frees the old one
/// **unwiped**. The `Drop` covers the final buffer, not every intermediate.
/// `login_ui::LoginForm` has had exactly this exposure for the master password
/// since it was written; the real fix is a fixed-capacity buffer, and it
/// belongs to neither of them alone.
pub fn draw(ui: &mut egui::Ui, form: &mut ApiKeyForm) -> Option<ApiKeyAction> {
    let mut asked = None;
    let ready = match form.step {
        ApiKeyStep::KeyPair => form.key_pair_ready(),
        ApiKeyStep::MasterPassword => form.password_ready(),
    };

    match form.step {
        ApiKeyStep::KeyPair => {
            ui.label(RichText::new(KEY_PAIR_TITLE).size(17.0));
            ui.add_space(6.0);
            ui.label(RichText::new(KEY_PAIR_HINT).size(12.0).color(theme::TEXT_MUTED));
            ui.add_space(14.0);

            ui.label(RichText::new(CLIENT_ID_LABEL).size(12.0));
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut form.client_id).desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);

            ui.label(RichText::new(CLIENT_SECRET_LABEL).size(12.0));
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut *form.secret)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
        ApiKeyStep::MasterPassword => {
            ui.label(RichText::new(PASSWORD_TITLE).size(17.0));
            ui.add_space(6.0);
            ui.label(RichText::new(PASSWORD_HINT).size(12.0).color(theme::TEXT_MUTED));
            ui.add_space(14.0);

            ui.label(RichText::new(PASSWORD_LABEL).size(12.0));
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut *form.password)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
    }

    if let Some(error) = form.error.as_deref() {
        ui.add_space(6.0);
        ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
    }

    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(!form.busy && ready, egui::Button::new(CONTINUE_LABEL)).clicked() {
            asked = Some(ApiKeyAction::Submit);
        }
        if ui.button(BACK_LABEL).clicked() {
            asked = Some(ApiKeyAction::Back);
        }
    });

    asked
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
    /// A grant answer and an unlock answer with no server behind either.
    fn fake_session() -> crate::rest::api::Session {
        crate::rest::api::Session::from_refresh_token(zeroize::Zeroizing::new(
            "not-a-real-refresh-token".to_string(),
        ))
    }

    fn fake_master_key() -> crate::rest::crypto::MasterKey {
        crate::rest::crypto::MasterKey::from_bytes([0x5A; crate::rest::crypto::MASTER_KEY_LEN])
    }

    fn test_account() -> ApiKeyAccount {
        ApiKeyAccount {
            server_url: "https://vault.example.test".to_string(),
            email: "a@b.c".to_string(),
            device_id: "11111111-2222-3333-4444-555555555555".to_string(),
        }
    }

    /// **The production seam is wired to the real calls and not to a stub
    /// somebody left in while testing.** A source pin, because the value is a
    /// `const` of two function pointers and no runtime assertion can say which
    /// functions they are.
    #[test]
    fn the_production_seam_calls_the_real_grant_and_the_real_unwrap() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let body = source
            .split_once(concat!("pub const PRODUCTION_API_", "KEY: ApiKeySeam"))
            .expect("the production seam must exist")
            .1
            .split_once(";\n")
            .expect("the const must be terminated")
            .0;
        assert!(body.len() > 30, "control: the seam's body is not empty");
        assert!(
            body.contains("grant_direct_rest") && body.contains("unlock_direct_rest"),
            "the seam must be wired to this module's own two functions; got {body:?}"
        );

        // And those two functions must call what they claim to. `api_key_grant`
        // is piece 1's whole contribution and this feature is its only caller.
        assert!(
            source.contains(concat!(".api_key", "_grant(")),
            "nothing in this module calls the grant this feature exists to call"
        );
        assert!(
            source.contains("unwrap_user_key("),
            "stage 2 must actually unwrap the user key -- that unwrap IS the master \
             password's verification, and without it a wrong password signs the user in to \
             a vault they cannot read"
        );
        // Positive control on both searches: a needle this file really does
        // spell this way is findable, so the two above mean something.
        assert!(
            source.contains(concat!(".prelogin", "(")),
            "control: a rest::api call is findable by this search at all"
        );
    }

    /// **The seam's two calls happen in order, and stage 2 does not re-run
    /// stage 1.** The count is the assertion: a design that re-granted on
    /// every password attempt would charge a Duo user a round trip and a new
    /// device registration per typo.
    #[test]
    fn a_password_retry_reuses_the_session_stage_one_minted() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static GRANTS: AtomicUsize = AtomicUsize::new(0);
        static UNLOCKS: AtomicUsize = AtomicUsize::new(0);
        GRANTS.store(0, Ordering::SeqCst);
        UNLOCKS.store(0, Ordering::SeqCst);

        let seam = ApiKeySeam {
            grant: |_account, _id, _secret| {
                GRANTS.fetch_add(1, Ordering::SeqCst);
                Ok(fake_session())
            },
            unlock: |_account, _session: &crate::rest::api::Session, password| {
                let n = UNLOCKS.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    assert_eq!(password, b"wrong-one", "control: the first password arrives");
                    Err(RestError::Crypto(crate::rest::crypto::CryptoError::MacMismatch))
                } else {
                    assert_eq!(password, b"hunter2", "the SECOND password is the one used");
                    Ok(fake_master_key())
                }
            },
        };

        let session = (seam.grant)(&test_account(), "user.9f3c", "b7d2ecc").expect("stage 1");
        let first = (seam.unlock)(&test_account(), &session, b"wrong-one");
        let refusal = unlock_refusal(&first.expect_err("the first password is wrong"));
        assert_eq!(refusal, ApiKeyRefusal::PasswordRejected);

        // The retry: a NEW session is not minted, because stage 1 did not
        // fail. This is the shape the worker loop enforces; here it is the
        // seam's own contract being stated.
        // The SAME session, not a new one: the borrow is what makes this
        // possible at all, and the by-value shape could not express it.
        assert!((seam.unlock)(&test_account(), &session, b"hunter2").is_ok());
        assert_eq!(
            UNLOCKS.load(Ordering::SeqCst),
            2,
            "control: both passwords reached the unlock"
        );
        assert_eq!(
            GRANTS.load(Ordering::SeqCst),
            1,
            "control: the fake grant is reachable, and the retry did not reach it again"
        );
    }
    /// **A wrong master password is retried against the SAME session.** The
    /// key pair is used once, to mint a session, and dropped -- the design's
    /// own words -- so a password typo costs one sync and not a second grant.
    #[test]
    fn a_wrong_password_is_retried_without_a_second_grant() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;
        static GRANTS: AtomicUsize = AtomicUsize::new(0);
        static UNLOCKS: AtomicUsize = AtomicUsize::new(0);
        GRANTS.store(0, Ordering::SeqCst);
        UNLOCKS.store(0, Ordering::SeqCst);

        let seam = ApiKeySeam {
            grant: |_, id, secret| {
                GRANTS.fetch_add(1, Ordering::SeqCst);
                assert_eq!(id, "user.9f3c");
                assert_eq!(secret, "b7d2ecc", "the secret reaches the grant untouched");
                Ok(fake_session())
            },
            unlock: |_, _session, _password| {
                if UNLOCKS.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(RestError::Crypto(crate::rest::crypto::CryptoError::MacMismatch))
                } else {
                    Ok(fake_master_key())
                }
            },
        };

        let (tx, rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        tx.send(ApiKeyCommand::KeyPair {
            client_id: "user.9f3c".to_string(),
            secret: zeroize::Zeroizing::new("b7d2ecc".to_string()),
        })
        .unwrap();
        tx.send(ApiKeyCommand::MasterPassword(zeroize::Zeroizing::new(
            "wrong-one".to_string(),
        )))
        .unwrap();
        tx.send(ApiKeyCommand::MasterPassword(zeroize::Zeroizing::new(
            "hunter2".to_string(),
        )))
        .unwrap();

        let outcome = run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx);

        assert!(outcome.is_some(), "the second password signs the user in");
        assert_eq!(UNLOCKS.load(Ordering::SeqCst), 2, "control: both passwords were tried");
        assert_eq!(
            GRANTS.load(Ordering::SeqCst),
            1,
            "a wrong password must NOT re-grant: stage 1 did not fail, and a second grant \
             would register a device and spend a round trip per typo"
        );
        assert_eq!(
            report_rx.try_iter().collect::<Vec<_>>(),
            vec![
                ApiKeyReport::KeyPairAccepted,
                ApiKeyReport::Refused(ApiKeyRefusal::PasswordRejected),
            ],
            "the window was told stage 1 passed, then told the PASSWORD was what failed"
        );
    }

    /// **A rejected key pair does not advance.** The loop stays on stage 1 and
    /// the next thing it will accept is another key pair -- a
    /// `MasterPassword` arriving now has no session to try against.
    #[test]
    fn a_rejected_key_pair_leaves_the_loop_on_stage_one() {
        use std::sync::mpsc;
        let seam = ApiKeySeam {
            grant: |_, _, _| Err(RestError::Unauthorized),
            unlock: |_, _, _| panic!("nothing was granted, so nothing may be unlocked"),
        };
        let (tx, rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        tx.send(ApiKeyCommand::KeyPair {
            client_id: "user.9f3c".to_string(),
            secret: zeroize::Zeroizing::new("wrong".to_string()),
        })
        .unwrap();
        // Arrives while there is no session. It must be ignored rather than
        // panicking the worker on the one screen a blocked user is looking at.
        tx.send(ApiKeyCommand::MasterPassword(zeroize::Zeroizing::new(
            "hunter2".to_string(),
        )))
        .unwrap();
        tx.send(ApiKeyCommand::Abandon).unwrap();

        assert!(run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx).is_none());
        assert_eq!(
            report_rx.try_iter().collect::<Vec<_>>(),
            vec![
                ApiKeyReport::Refused(ApiKeyRefusal::KeyPairRejected),
                ApiKeyReport::Refused(ApiKeyRefusal::KeyPairRejected),
            ],
            "the stray password is answered as 'the key pair is what is missing', which is \
             true, and not as a rejected password, which would be a lie"
        );
    }

    /// A closed command channel -- the window went away -- ends the loop and
    /// drops the session, rather than blocking a detached thread forever.
    #[test]
    fn a_closed_window_ends_the_loop() {
        use std::sync::mpsc;
        let seam = ApiKeySeam {
            grant: |_, _, _| panic!("nothing was sent, so nothing may be granted"),
            unlock: |_, _, _| panic!("nothing was sent, so nothing may be unlocked"),
        };
        let (tx, rx) = mpsc::channel::<ApiKeyCommand>();
        let (report_tx, _report_rx) = mpsc::channel();
        drop(tx);
        assert!(run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx).is_none());
    }

    /// **Nothing in this module writes the key pair to disk.** The design's
    /// deliberate refusal, read off the source: a stored `client_secret` is a
    /// permanent password-free login sitting on disk.
    #[test]
    fn the_key_pair_is_never_persisted() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        for forbidden in
            ["SessionStore", "user_key_store", "std::fs::", "fs::write", "File::create", "settings::"]
        {
            assert!(
                !production.contains(forbidden),
                "this module reached for `{forbidden}`; the client secret is not persisted \
                 and the session token goes to the store the `adopt` sink already owns"
            );
        }
        // Positive controls: the cut really did keep the production half, and
        // these needles really are findable in this crate when they are there.
        assert!(
            production.contains("pub fn run_api_key_sign_in"),
            "the cut lost the production half, so the absences above prove nothing"
        );
        assert!(
            include_str!("session_store.rs").contains("std::fs::write"),
            "the needle has drifted -- persistence is no longer spelled this way, so its \
             absence above means nothing"
        );
    }
    /// **The client secret's hygiene rule, read off the source.**
    ///
    /// It is a permanent, password-free login to the account, and the design
    /// gives it "the master password's handling": `Zeroizing`, no `Debug`,
    /// never logged, never in an error string. Three of those four are
    /// properties of the *text of this file* and no runtime value can
    /// demonstrate them, which is why this test reads the file.
    #[test]
    fn the_client_secret_is_handled_like_a_password() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        // Control on the cut: the subject really is in this half.
        assert!(
            production.contains("pub struct ApiKeyForm {"),
            "the cut lost the form, so every absence below proves nothing"
        );

        // 1. Still wiped on drop.
        assert!(
            production.contains("pub secret: Zeroizing<String>"),
            "the client secret is no longer wiped on drop"
        );
        assert!(
            production.contains("pub password: Zeroizing<String>"),
            "control: the master password beside it is wiped the same way, so the needle \
             above is the shape this file really uses"
        );

        // 2. Not printable -- neither derived nor hand-written.
        assert!(
            !production.contains("impl std::fmt::Debug for ApiKeyForm"),
            "ApiKeyForm gained a Debug; there is nothing in it a formatter may print"
        );
        assert!(
            !production.contains("impl std::fmt::Debug for ApiKeyCommand"),
            "ApiKeyCommand gained a Debug; two of its variants carry a credential"
        );
        // Control on that search technique: a `derive(..)]\npub enum` really is
        // findable this way, so the two absences above are about Debug and not
        // about the needle being unspellable.
        assert!(
            production.contains("derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum ApiKeyRefusal"),
            "the derive-then-item search no longer matches anything, so the assertions \
             above prove nothing"
        );

        // 3. Never formatted, never logged, never in an error string.
        //
        // Every production line that so much as names the secret, checked one
        // at a time -- a whole-file search would be satisfied by the wrong
        // line.
        let touching: Vec<&str> = production
            .lines()
            .filter(|line| {
                let t = line.trim_start();
                !t.starts_with("//") && !t.starts_with("///")
            })
            .filter(|line| line.contains("secret") || line.contains("client_secret"))
            .collect();
        assert!(
            touching.len() >= 4,
            "control: the scan found only {} lines naming the secret, which is fewer than \
             this module has -- the filter is wrong and the loop below is vacuous: {touching:?}",
            touching.len()
        );
        for line in &touching {
            for forbidden in ["log::", "format!", "println!", "eprintln!", "{secret", "to_string()"]
            {
                assert!(
                    !line.contains(forbidden),
                    "a line naming the client secret also reaches for `{forbidden}`; the \
                     secret is never formatted, logged, or put in an error string. Line: \
                     {line:?}"
                );
            }
        }
        // Control on THAT loop: the same scan over a line that does log finds
        // it, so the absence above is about this file and not about the needle.
        assert!(
            production.contains("log::warn!"),
            "this module logs nothing at all, so 'the secret is never logged' is a claim \
             about a file with no logging in it"
        );
    }
    const WINDOW: egui::Vec2 = egui::vec2(420.0, 620.0);

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
            ..Default::default()
        }
    }

    /// A context with `theme::apply`'s fonts actually live -- a font set
    /// registered during a frame only becomes usable at the start of the next
    /// one, so the throwaway frames are load-bearing. `second_factor_ui` and
    /// `login_ui` keep their own copies of this for the same reason: it is six
    /// lines, and the alternative is a `pub(crate)` promotion across a file
    /// this module owes no dependency.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            // Everything else is geometry this module does not assert on.
            _ => {}
        }
    }

    /// Every string the card paints in one frame.
    fn painted(form: &mut ApiKeyForm) -> Vec<String> {
        let ctx = styled_context();
        let output = ctx.run_ui(raw_input(), |ui| {
            let _ = draw(ui, form);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    fn says(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// **The two stages are two screens.** The key-pair screen has no master
    /// password box, and the password screen has no secret box -- which is the
    /// design's second reason for the split: "it keeps a long-lived credential
    /// and the master password off the screen at the same moment."
    #[test]
    fn each_stage_shows_only_its_own_fields() {
        let mut form = ApiKeyForm::new();
        let stage_one = painted(&mut form);
        assert!(
            says(&stage_one, CLIENT_ID_LABEL),
            "control: the key-pair card painted its own labels; got {stage_one:?}"
        );
        assert!(says(&stage_one, CLIENT_SECRET_LABEL), "got {stage_one:?}");
        assert!(
            !says(&stage_one, PASSWORD_LABEL),
            "the master password is not on the same screen as the client secret; \
             got {stage_one:?}"
        );

        let mut form = ApiKeyForm::new();
        form.step = ApiKeyStep::MasterPassword;
        let stage_two = painted(&mut form);
        assert!(
            says(&stage_two, PASSWORD_LABEL),
            "control: the password card painted its own label; got {stage_two:?}"
        );
        assert!(
            !says(&stage_two, CLIENT_SECRET_LABEL),
            "the secret is done with; leaving it on screen is a long-lived credential \
             sitting in a text box for no reason. got {stage_two:?}"
        );
    }

    /// **The password screen says why it is asking.** A user who just typed a
    /// key that "signs them in" and is then asked for a password will read it
    /// as a failure unless the card says otherwise.
    #[test]
    fn the_password_stage_explains_why_the_key_was_not_enough() {
        let mut form = ApiKeyForm::new();
        form.step = ApiKeyStep::MasterPassword;
        let texts = painted(&mut form);
        assert!(
            says(&texts, "unlock") || says(&texts, "decrypt"),
            "the hint must say what the password is FOR, which is the vault's contents; \
             got {texts:?}"
        );
        assert!(
            !says(&texts, "wrong") && !says(&texts, "failed"),
            "nothing failed -- the key pair was accepted; got {texts:?}"
        );
    }

    /// A refusal is painted where the user is looking, on the stage it sent
    /// them to.
    #[test]
    fn the_refusal_is_painted_on_the_stage_it_returns_to() {
        let mut form = ApiKeyForm::new();
        form.step = ApiKeyStep::MasterPassword;
        form.refused(ApiKeyRefusal::PasswordRejected);
        let texts = painted(&mut form);
        assert!(
            says(&texts, message_for(ApiKeyRefusal::PasswordRejected)),
            "got {texts:?}"
        );
        assert!(
            says(&texts, PASSWORD_LABEL),
            "control: it is painted on the PASSWORD stage, which is where that refusal \
             returns to; got {texts:?}"
        );

        let mut form = ApiKeyForm::new();
        form.refused(ApiKeyRefusal::KeyPairRejected);
        let texts = painted(&mut form);
        assert!(
            says(&texts, message_for(ApiKeyRefusal::KeyPairRejected)),
            "got {texts:?}"
        );
        assert!(
            says(&texts, CLIENT_SECRET_LABEL),
            "a rejected key pair returns to the key-pair stage with both fields on screen; \
             got {texts:?}"
        );

        // Control on the whole harness: a form with no refusal paints neither
        // message, so the two presences above are about `refused` and not
        // about a card that paints every string it knows.
        let mut clean = ApiKeyForm::new();
        let texts = painted(&mut clean);
        assert!(
            !says(&texts, message_for(ApiKeyRefusal::KeyPairRejected))
                && !says(&texts, message_for(ApiKeyRefusal::PasswordRejected)),
            "control: a card with nothing wrong paints no refusal; got {texts:?}"
        );
    }
}

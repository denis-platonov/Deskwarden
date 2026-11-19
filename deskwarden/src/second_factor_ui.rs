//! The second-factor prompt: the stage between the sign-in card and the
//! spinner.
//!
//! Nothing in this module has ever seen a `Challenge`. See
//! `login_ui::SecondFactorRequest` for what does, and why it stays on the
//! worker thread.

use crate::i18n;
use crate::rest::api::SecondFactor;
use crate::theme;
use eframe::egui;
use zeroize::Zeroize;

/// Bitwarden's own priority order, restricted to what this app can complete.
///
/// A `const` list rather than an `Ord` on `SecondFactor`, because the order is
/// this prompt's opinion about which box to put the cursor in -- not a fact
/// about the enum, which `rest::api` owns and which has no reason to know that
/// a YubiKey is faster to reach for than an emailed code.
const PRIORITY: [fn(&SecondFactor) -> bool; 3] = [
    |f| matches!(f, SecondFactor::YubiKey),
    |f| matches!(f, SecondFactor::Authenticator),
    |f| matches!(f, SecondFactor::Email { .. }),
];

/// Whether this prompt can complete the factor at all. `Unsupported` is not an
/// error here -- see [`unsupported_only_message`] -- it is simply not something
/// the user can be asked to type.
pub fn is_supported(factor: &SecondFactor) -> bool {
    PRIORITY.iter().any(|matches| matches(factor))
}

/// The factor the prompt opens on: the highest-priority SUPPORTED one, or
/// `None` when the account offers nothing this app can complete.
///
/// It agrees with [`crate::rest::api::preferred_second_factor`] and is
/// deliberately not a call to it: which box the cursor lands in is a
/// presentation decision, and the day the card wants a different default is
/// not a day `rest::api` should have to be edited.
pub fn preferred(offered: &[SecondFactor]) -> Option<SecondFactor> {
    PRIORITY
        .iter()
        .find_map(|matches| offered.iter().find(|f| matches(f)))
        .cloned()
}

/// Everything the code stage is, as state.
///
/// **There is no `Challenge` in here and there must never be one.** The
/// challenge carries the derived password hash and the master key, and this
/// struct is owned by a frame closure that outlives the stage. What this holds
/// is a provider list, six digits and a message.
pub struct Prompt {
    offered: Vec<SecondFactor>,
    chosen: Option<SecondFactor>,
    /// The digits the user is typing. Public because the text edit writes
    /// straight into it; wiped on [`Drop`] and on every provider switch.
    pub code: String,
    /// The inline message under the box, or `None`. See [`message_for`].
    pub error: Option<String>,
    /// Set once an email code has actually been sent, so the card can say so
    /// rather than leaving the user wondering whether the button did anything.
    pub email_sent: bool,
    /// True while a send or an answer is in flight: the buttons ghost, exactly
    /// as the sign-in card's do while a login is running.
    pub busy: bool,
}

/// The code is not password-equivalent, but it is a live credential for the
/// thirty seconds it exists, and this struct is move-captured by a frame
/// closure that lives as long as the window. Wiping costs one call on a
/// six-byte string. `login_ui::LoginForm` states the same rule for the same
/// reason.
impl Drop for Prompt {
    fn drop(&mut self) {
        self.code.zeroize();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
    }
}

impl Prompt {
    pub fn new(offered: Vec<SecondFactor>) -> Self {
        let chosen = preferred(&offered);
        Self {
            offered,
            chosen,
            code: String::new(),
            error: None,
            email_sent: false,
            busy: false,
        }
    }

    /// Everything the server said this account has, unsupported entries
    /// included. [`unsupported_only_message`] reads this list to name Duo by
    /// name.
    pub fn offered(&self) -> &[SecondFactor] {
        &self.offered
    }

    /// The subset the user may actually pick between. A one-element answer is
    /// what makes the provider switch disappear.
    pub fn supported(&self) -> Vec<SecondFactor> {
        self.offered.iter().filter(|f| is_supported(f)).cloned().collect()
    }

    pub fn chosen(&self) -> Option<SecondFactor> {
        self.chosen.clone()
    }

    /// Switches provider, and **clears the code and the error with it**: both
    /// were about the factor the user just navigated away from, and a stale
    /// "That code didn't work" under a freshly-switched provider is a message
    /// about nothing.
    pub fn choose(&mut self, factor: SecondFactor) {
        self.chosen = Some(factor);
        self.code.zeroize();
        self.code.clear();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
        self.error = None;
        self.email_sent = false;
    }
}

/// The heading over the code box: the factor, named the way the user's own
/// Bitwarden security settings name it.
pub fn factor_title(factor: &SecondFactor) -> &'static str {
    match factor {
        SecondFactor::Authenticator => i18n::t("second_factor.title_authenticator"),
        SecondFactor::Email { .. } => i18n::t("second_factor.title_email"),
        SecondFactor::YubiKey => "YubiKey",
        SecondFactor::Unsupported(_) => i18n::t("second_factor.title_generic"),
    }
}

/// The line under the heading: where the code is, in the one sentence it takes
/// to say it.
///
/// Returns `String` rather than `&'static str` because the Email arm carries
/// the masked address the server sent -- which is the only reason this line
/// earns its space for a user with more than one mailbox.
pub fn factor_hint(factor: &SecondFactor) -> String {
    match factor {
        SecondFactor::Authenticator => {
            i18n::t("second_factor.hint_authenticator").to_string()
        }
        SecondFactor::Email { masked: Some(address) } => {
            i18n::tf("second_factor.hint_email_masked", &[("address", address.as_str())])
        }
        SecondFactor::Email { masked: None } => {
            i18n::t("second_factor.hint_email_unmasked")
                .to_string()
        }
        SecondFactor::YubiKey => {
            i18n::t("second_factor.hint_yubikey")
                .to_string()
        }
        SecondFactor::Unsupported(_) => {
            i18n::t("second_factor.hint_unsupported").to_string()
        }
    }
}

pub fn send_code_label() -> &'static str {
    i18n::t("second_factor.send_code")
}
pub fn code_sent_notice() -> &'static str {
    i18n::t("second_factor.code_sent")
}

impl Prompt {
    /// Email is the one factor that needs a call before the user has anything
    /// to type. See the design's "Email is the one that needs a second call".
    pub fn wants_send_button(&self) -> bool {
        matches!(self.chosen, Some(SecondFactor::Email { .. }))
    }
}

/// A provider number, in words the user would recognise from their own
/// Bitwarden security settings.
///
/// `String` rather than `&'static str` so the unknown arm can carry the number
/// it did not recognise. A message reading "an unrecognised method" is
/// unreportable; one reading "an unrecognised method (9)" names the thing to
/// look up.
pub fn provider_name(number: u8) -> String {
    match number {
        2 => i18n::t("second_factor.provider_duo").to_string(),
        6 => i18n::t("second_factor.provider_duo_org").to_string(),
        7 => i18n::t("second_factor.provider_webauthn").to_string(),
        4 => i18n::t("second_factor.provider_u2f").to_string(),
        5 => i18n::t("second_factor.provider_remembered").to_string(),
        8 => i18n::t("second_factor.provider_recovery").to_string(),
        other => i18n::tf("second_factor.provider_unknown", &[("number", &other.to_string())]),
    }
}

/// What an account whose every factor is unsupported is told.
///
/// `None` the instant anything is supported: the spec is explicit that
/// `Unsupported` is not an error, and an account with WebAuthn beside an
/// authenticator app is an ordinary prompt.
///
/// **The two things this message must do**, both of which the message it
/// replaces (`login_ui::friendly_auth_error`'s two-step arm) did not:
///
///  * name the provider, so a Duo user is not left guessing which of their
///    factors Deskwarden means;
///  * name the personal API key, which IS supported
///    ([`crate::rest::api::RestClient::api_key_grant`]) and is the same path
///    `bw login --apikey` gives these users today. Duo and WebAuthn are not
///    reachable from `bw login` either, so without this sentence the message
///    is a dead end rather than a redirection.
pub fn unsupported_only_message(offered: &[SecondFactor]) -> Option<String> {
    if offered.is_empty() || offered.iter().any(is_supported) {
        return None;
    }
    let names: Vec<String> = offered
        .iter()
        .map(|factor| match factor {
            SecondFactor::Unsupported(number) => provider_name(*number),
            // Unreachable given the guard above, and written as a name rather
            // than an `unreachable!()` because the cost of being wrong here is
            // the whole window dying on the one screen a blocked user sees.
            supported => factor_title(supported).to_lowercase(),
        })
        .collect();
    let list = names
        .into_iter()
        .reduce(|so_far, next| {
            i18n::tf("second_factor.list_and", &[("first", &so_far), ("rest", &next)])
        })?;
    Some(i18n::tf("second_factor.unsupported_only", &[("list", &list)]))
}

/// Why the prompt is showing a message.
///
/// Four variants and not one `String`, because the *behaviour* differs and not
/// only the wording: a rejected code empties the box, a failed send does not,
/// and an expired challenge ends the stage. A `String` error would put that
/// decision in the caller, where nothing tests it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// The server said no to these digits.
    CodeRejected,
    /// [`crate::rest::api::RestClient::send_email_code`] failed, which
    /// `rest::api` reports as its own [`crate::rest::api::RestError::
    /// CodeNotSent`] precisely so it cannot be confused with this next one.
    /// **Not** a rejected code: the user has nothing to check.
    EmailSendFailed,
    /// The challenge no longer resumes -- the server expired it, or the worker
    /// holding it is gone. There is no hash left to retry with, so this one
    /// ends the stage.
    ChallengeExpired,
    /// Neither the grant nor the send got an answer at all.
    Unreachable,
}

impl Trouble {
    /// Whether the stage can continue. `false` for exactly one variant, and it
    /// is the variant with nothing on the worker to retry against.
    pub fn is_fatal(self) -> bool {
        matches!(self, Trouble::ChallengeExpired)
    }
}

/// One line the user can act on, per trouble.
pub fn message_for(trouble: Trouble) -> &'static str {
    match trouble {
        Trouble::CodeRejected => {
            i18n::t("second_factor.err_code_rejected")
        }
        Trouble::EmailSendFailed => {
            i18n::t("second_factor.err_send_failed")
        }
        Trouble::ChallengeExpired => {
            i18n::t("second_factor.err_expired")
        }
        Trouble::Unreachable => {
            i18n::t("login.err_unreachable")
        }
    }
}

impl Prompt {
    /// Applies a failure to the prompt.
    ///
    /// **`CodeRejected` is the arm this feature exists for**: it empties the
    /// code box and touches nothing else. The master password is not here to
    /// clear -- it is on the worker thread, still holding the derived hash --
    /// which is precisely why a wrong digit no longer costs a re-typed master
    /// password.
    pub fn went_wrong(&mut self, trouble: Trouble) {
        self.busy = false;
        self.error = Some(message_for(trouble).to_string());
        match trouble {
            Trouble::CodeRejected => {
                self.code.zeroize();
                self.code.clear();
            }
            // The box is left exactly as it was: a send that failed may be the
            // SECOND send, and the code from the first one is still good.
            Trouble::EmailSendFailed => self.email_sent = false,
            Trouble::ChallengeExpired | Trouble::Unreachable => {}
        }
    }
}

pub fn continue_label() -> &'static str {
    i18n::t("second_factor.continue")
}
pub fn back_label() -> &'static str {
    i18n::t("second_factor.back")
}

/// What the user asked the code stage to do this frame. `None` from [`draw`]
/// is the ordinary case: the user is still typing.
pub enum PromptAction {
    /// Email only: send me a code.
    Send,
    /// These digits, against the chosen provider.
    ///
    /// No `Debug` on this enum, because this variant carries a live one-time
    /// code and [`crate::rest::api::SecondFactorAnswer`] refuses to print one.
    Submit(crate::rest::api::SecondFactorAnswer),
    /// Back to the master password.
    Back,
    /// **The unsupported-only card's one way forward**: the personal API key
    /// its own message names. Without it that message is a promise this app
    /// does not keep -- it tells a Duo user to sign in with an API key, and
    /// there is nowhere to type one. See [`crate::api_key_ui`].
    UseApiKey,
}

/// Draws the code stage on the sign-in split screen (spec C 5.2): the brand
/// half, and the prompt in the form column. Pure view: the caller owns the
/// [`Prompt`] and performs the channel sends for whatever comes back, exactly
/// as `login_ui::draw_login_window` and its caller are split.
///
/// It also draws the window's caption buttons, because this stage has no title
/// bar of its own and the design has none on this screen.
pub fn draw(ui: &mut egui::Ui, prompt: &mut Prompt) -> Option<PromptAction> {
    let full = ui.max_rect();
    let mut asked = None;
    let caption = crate::kit::auth::split(ui, full, crate::login_ui::CloseControl::Active, |ui| {
        asked = draw_form(ui, prompt);
    });
    crate::kit::auth::apply_caption(ui.ctx(), caption);
    asked
}

/// The big words over the code boxes (the sign-in gallery's "Enter your
/// 6-digit code" for the two numeric methods).
pub fn stage_heading(factor: &SecondFactor) -> &'static str {
    match factor {
        SecondFactor::Authenticator | SecondFactor::Email { .. } => i18n::t("second_factor.heading_code"),
        SecondFactor::YubiKey => i18n::t("second_factor.heading_yubikey"),
        SecondFactor::Unsupported(_) => i18n::t("second_factor.title_generic"),
    }
}

/// What the stage says when Continue is pressed with fewer than six digits.
pub fn short_code() -> &'static str {
    i18n::t("second_factor.short_code")
}
/// The link that moves to the next method the account offers.
pub fn other_method_label() -> &'static str {
    i18n::t("second_factor.other_method")
}

/// The glyph in the stage's tile.
fn tile_glyph(factor: &SecondFactor) -> &'static str {
    match factor {
        SecondFactor::Authenticator => crate::kit::auth::PHONE_CODE,
        SecondFactor::Email { .. } => "M4 4h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM22 7l-10 6L2 7",
        SecondFactor::YubiKey => crate::kit::icon::glyph::PASSKEY,
        SecondFactor::Unsupported(_) => crate::kit::auth::LOCK_TILE,
    }
}

/// True for the methods whose code is six digits.
fn six_digits(factor: &SecondFactor) -> bool {
    matches!(factor, SecondFactor::Authenticator | SecondFactor::Email { .. })
}

/// The next supported method after `chosen`, wrapping, if there is another.
fn next_method(prompt: &Prompt, chosen: &SecondFactor) -> Option<SecondFactor> {
    let supported = prompt.supported();
    let at = supported.iter().position(|f| f == chosen)?;
    let next = &supported[(at + 1) % supported.len()];
    (next != chosen).then(|| next.clone())
}

/// The prompt itself, in the form column: the tile, the heading, the code
/// (six boxes, or one field for a YubiKey), its error, Continue, and the
/// links.
fn draw_form(ui: &mut egui::Ui, prompt: &mut Prompt) -> Option<PromptAction> {
    use crate::kit::auth::{self, FORM_GAP};
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 0.0;

    if let Some(message) = unsupported_only_message(prompt.offered()) {
        auth::stage_tile(ui, tile_glyph(&SecondFactor::Unsupported(0)), false);
        ui.add_space(20.0);
        auth::stage_title(ui, stage_heading(&SecondFactor::Unsupported(0)));
        ui.add_space(6.0);
        auth::subtitle(ui, &message);
        ui.add_space(20.0);
        // The message above names the API key and where to create it. This is
        // the button that makes it true.
        if auth::submit_button(ui, crate::api_key_ui::use_api_key_label(), false, width).clicked() {
            return Some(PromptAction::UseApiKey);
        }
        ui.add_space(20.0);
        return auth::link_big(ui, back_label()).clicked().then_some(PromptAction::Back);
    }

    let Some(chosen) = prompt.chosen() else {
        // An empty provider list. Nothing to ask for, and nothing to
        // apologise for either -- `unsupported_only_message` answers `None`
        // for the empty offer, so this arm exists and must say something.
        auth::field_error(ui, message_for(Trouble::Unreachable));
        ui.add_space(FORM_GAP);
        return auth::link_big(ui, back_label()).clicked().then_some(PromptAction::Back);
    };

    // The gallery's column: 20 between the parts.
    auth::stage_tile(ui, tile_glyph(&chosen), false);
    ui.add_space(20.0);
    auth::stage_title(ui, stage_heading(&chosen));
    ui.add_space(6.0);
    auth::subtitle(ui, &factor_hint(&chosen));
    ui.add_space(20.0);

    let mut asked = None;

    if prompt.wants_send_button() {
        let busy = prompt.busy;
        let send = ui
            .add_enabled_ui(!busy, |ui| {
                crate::kit::controls::button(
                    ui,
                    crate::kit::controls::ButtonKind::Outline,
                    crate::kit::controls::ROW,
                    send_code_label(),
                    None,
                )
            })
            .inner;
        if send.clicked() {
            asked = Some(PromptAction::Send);
        }
        if prompt.email_sent {
            ui.add_space(10.0);
            let p = crate::kit::tokens::palette(ui.ctx());
            ui.add(egui::Label::new(theme::regular(code_sent_notice(), 14.0).color(p.text_muted)).wrap());
        }
        ui.add_space(20.0);
    }

    let entry_id = egui::Id::new("auth-second-factor-code");
    let bad = prompt.error.is_some();
    let entry = if six_digits(&chosen) {
        auth::code_boxes(ui, entry_id, &mut prompt.code, bad, width, !prompt.busy)
    } else {
        ui.add_enabled_ui(!prompt.busy, |ui| {
            auth::field_text(ui, entry_id, &mut prompt.code, "", width, bad, None)
        })
        .inner
    };
    // A YubiKey types its code and presses Enter itself, which is why the
    // focus goes here on entry and why Enter submits.
    if !prompt.busy {
        entry.request_focus();
    }
    let entered = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

    if let Some(error) = prompt.error.as_deref() {
        ui.add_space(8.0);
        auth::field_error(ui, error);
    }

    ui.add_space(20.0);
    let label = if prompt.busy { crate::login_ui::checking_label() } else { continue_label() };
    let go = auth::submit_button(ui, label, prompt.busy, width).clicked();
    if (go || entered) && !prompt.busy && asked.is_none() {
        let code = prompt.code.trim().to_string();
        if code.is_empty() || (six_digits(&chosen) && code.len() < 6) {
            prompt.error = Some(short_code().to_string());
        } else {
            asked = Some(PromptAction::Submit(crate::rest::api::SecondFactorAnswer::new(
                chosen.clone(),
                &code,
            )));
        }
    }

    // Back always; the other method only when the account has one.
    ui.add_space(20.0);
    let other = next_method(prompt, &chosen);
    let labels: Vec<&str> =
        if other.is_some() { vec![back_label(), other_method_label()] } else { vec![back_label()] };
    match auth::link_row(ui, &labels) {
        Some(0) => asked = Some(PromptAction::Back),
        Some(_) => {
            if let Some(factor) = other {
                prompt.choose(factor);
            }
        }
        None => {}
    }

    asked
}

impl Prompt {
    /// **A code was asked for.** Says so, and deliberately does NOT ghost the
    /// card.
    ///
    /// A send has no success report -- the worker answers only
    /// [`Trouble::EmailSendFailed`], because a mail that left is not news --
    /// so a card that went [`Prompt::busy`] here would have nothing to bring
    /// it back, and the user would be left looking at a code box they could no
    /// longer type into. The answer is the one control that DOES have both
    /// answers: [`Prompt::busy`] is set when a code is submitted, and either a
    /// trouble clears it or the sign-in ends the stage.
    pub fn sent_a_code(&mut self) {
        self.email_sent = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::SecondFactor;

    /// **The default lands on the highest-priority SUPPORTED factor**, in
    /// Bitwarden's own order: YubiKey, then Authenticator, then Email.
    ///
    /// The `Unsupported` entries are the control: an account with WebAuthn
    /// and an authenticator app must open on the authenticator, and a
    /// `preferred` that simply returned the first element would pass every
    /// single-provider case and fail exactly this one.
    #[test]
    fn the_prompt_opens_on_the_best_supported_factor() {
        let webauthn_and_totp = [SecondFactor::Unsupported(7), SecondFactor::Authenticator];
        assert_eq!(
            preferred(&webauthn_and_totp),
            Some(SecondFactor::Authenticator),
            "an account with WebAuthn and an authenticator app must open on the authenticator"
        );

        let all_three = [
            SecondFactor::Email { masked: None },
            SecondFactor::Authenticator,
            SecondFactor::YubiKey,
        ];
        assert_eq!(
            preferred(&all_three),
            Some(SecondFactor::YubiKey),
            "YubiKey outranks the authenticator, which outranks email"
        );

        let email_only = [SecondFactor::Email { masked: Some("a***@b.c".to_string()) }];
        assert_eq!(
            preferred(&email_only),
            Some(SecondFactor::Email { masked: Some("a***@b.c".to_string()) }),
            "the masked address must survive the choice -- it is what the card shows"
        );

        assert_eq!(
            preferred(&[SecondFactor::Unsupported(2), SecondFactor::Unsupported(7)]),
            None,
            "Duo and WebAuthn are not a choice this prompt can offer"
        );
    }

    /// The prompt keeps the offered list whole -- including the unsupported
    /// entries, which the unsupported-only message names -- but only ever
    /// *chooses* a supported one.
    #[test]
    fn the_prompt_remembers_what_was_offered_and_what_it_picked() {
        let prompt = Prompt::new(vec![
            SecondFactor::Unsupported(2),
            SecondFactor::Email { masked: Some("a***@b.c".to_string()) },
            SecondFactor::Authenticator,
        ]);
        assert_eq!(prompt.offered().len(), 3, "control: the whole offer is kept");
        assert_eq!(prompt.supported().len(), 2, "Duo is not offered as a choice");
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Authenticator),
            "the authenticator outranks email"
        );

        let mut prompt = prompt;
        prompt.code.push_str("123456");
        prompt.error = Some("wrong".to_string());
        prompt.choose(SecondFactor::Email { masked: Some("a***@b.c".to_string()) });
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Email { masked: Some("a***@b.c".to_string()) })
        );
        assert!(
            prompt.code.is_empty(),
            "switching provider must clear a code typed for the OTHER provider"
        );
        assert_eq!(
            prompt.error, None,
            "and must clear an error that was about the other provider"
        );
    }

    /// The card names the factor the user is being asked for, and the hint
    /// tells them where to look for the code. "Enter your code" would be true
    /// of all three and useful for none.
    #[test]
    fn each_factor_is_named_and_says_where_the_code_comes_from() {
        assert_eq!(factor_title(&SecondFactor::Authenticator), "Authenticator app");
        assert!(
            factor_hint(&SecondFactor::Authenticator).contains("authenticator app"),
            "got {:?}",
            factor_hint(&SecondFactor::Authenticator)
        );

        assert_eq!(factor_title(&SecondFactor::YubiKey), "YubiKey");
        assert!(
            factor_hint(&SecondFactor::YubiKey).contains("touch"),
            "a YubiKey hint that does not mention touching the key describes nothing the \
             user can do; got {:?}",
            factor_hint(&SecondFactor::YubiKey)
        );

        // The masked address is the whole value of the Email arm's hint: it is
        // how a user with two mailboxes knows which one to open.
        let masked = SecondFactor::Email { masked: Some("a***@b.c".to_string()) };
        assert!(
            factor_hint(&masked).contains("a***@b.c"),
            "got {:?}",
            factor_hint(&masked)
        );
        let unmasked = SecondFactor::Email { masked: None };
        assert!(
            !factor_hint(&unmasked).contains("None") && !factor_hint(&unmasked).is_empty(),
            "an address-less email hint must still be a sentence, not a debug-printed \
             Option; got {:?}",
            factor_hint(&unmasked)
        );
    }

    /// **Send code appears for Email and for nothing else.** The other two
    /// factors need no round trip -- the code is already on the phone or the
    /// key -- and a button that sends nothing is this project's most-repeated
    /// defect.
    #[test]
    fn only_email_offers_to_send_a_code() {
        let email = Prompt::new(vec![SecondFactor::Email { masked: None }]);
        assert!(
            email.wants_send_button(),
            "control: the Email prompt is the one that must have the button"
        );
        for factor in [SecondFactor::Authenticator, SecondFactor::YubiKey] {
            let prompt = Prompt::new(vec![factor.clone()]);
            assert!(
                !prompt.wants_send_button(),
                "{factor:?} needs no round trip and must not offer one"
            );
        }
    }


    /// **The message names the provider.** "This account uses two-step login"
    /// is what `login_ui` said before this work and it is what sent people to
    /// a terminal to find out which factor it meant.
    #[test]
    fn an_unsupported_only_account_is_told_which_provider_and_what_to_do() {
        let duo = unsupported_only_message(&[SecondFactor::Unsupported(2)])
            .expect("control: an all-unsupported account must get a message");
        assert!(duo.contains("Duo"), "got {duo:?}");
        assert!(
            duo.contains("API key"),
            "the personal API key is the ONE path these users have; a message without it \
             is a dead end. got {duo:?}"
        );
        assert!(
            !duo.contains("bw login") && !duo.contains("terminal"),
            "this is the message that replaces the terminal instruction; got {duo:?}"
        );

        let webauthn = unsupported_only_message(&[SecondFactor::Unsupported(7)])
            .expect("WebAuthn-only is the other real case");
        assert!(
            webauthn.contains("security key") || webauthn.contains("WebAuthn"),
            "got {webauthn:?}"
        );
        assert!(
            !webauthn.contains("Duo"),
            "the message must name THIS account's provider, not a list of every one; \
             got {webauthn:?}"
        );

        let both = unsupported_only_message(&[
            SecondFactor::Unsupported(2),
            SecondFactor::Unsupported(7),
        ])
        .expect("two unsupported providers is still an unsupported-only account");
        assert!(both.contains("Duo") && both.contains("security key"), "got {both:?}");
    }

    /// **The message is absent the moment anything is supported.** An account
    /// with WebAuthn AND an authenticator app must be offered the
    /// authenticator, not apologised to.
    #[test]
    fn a_supportable_account_gets_no_apology() {
        assert_eq!(
            unsupported_only_message(&[
                SecondFactor::Unsupported(7),
                SecondFactor::Authenticator
            ]),
            None,
            "WebAuthn beside an authenticator app is an ordinary prompt"
        );
        // Positive control: the same call with the authenticator removed does
        // produce a message, so the `None` above is about supportability and
        // not about the function having stopped working.
        assert!(
            unsupported_only_message(&[SecondFactor::Unsupported(7)]).is_some(),
            "control: the unsupported-only case still produces a message"
        );
        assert_eq!(
            unsupported_only_message(&[]),
            None,
            "an empty offer is not a Duo account -- it is a server answer nobody can act on"
        );
    }

    /// Every number the spec's table names, so an unfamiliar one degrades to
    /// something reportable rather than to a bare integer in a sentence.
    #[test]
    fn every_known_provider_number_has_a_name() {
        assert_eq!(provider_name(2), "Duo");
        assert_eq!(provider_name(6), "Duo (organization)");
        assert_eq!(provider_name(7), "a security key (WebAuthn)");
        assert_eq!(provider_name(4), "a U2F security key");
        assert!(
            provider_name(99).contains("99"),
            "an unknown provider must carry its number so a bug report can name it; got {:?}",
            provider_name(99)
        );
    }


    /// The three failures say three different things. A shared "That didn't
    /// work" would tell a user whose code never arrived to check the code they
    /// do not have.
    #[test]
    fn the_three_troubles_do_not_share_a_message() {
        let rejected = message_for(Trouble::CodeRejected);
        let send_failed = message_for(Trouble::EmailSendFailed);
        let expired = message_for(Trouble::ChallengeExpired);

        assert!(rejected.contains("code"), "got {rejected:?}");
        assert!(
            send_failed.contains("send") || send_failed.contains("sent"),
            "the email failure must be about SENDING, not about a code the user never \
             received; got {send_failed:?}"
        );
        assert!(
            !send_failed.contains("didn't work") && !send_failed.contains("Check it"),
            "the email failure must not read as a rejected code; got {send_failed:?}"
        );
        assert!(
            expired.contains("master password") || expired.contains("again"),
            "an expired challenge means starting over, and the message has to say so; \
             got {expired:?}"
        );

        let all = [rejected, send_failed, expired, message_for(Trouble::Unreachable)];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "two troubles share one message");
            }
        }
    }

    /// **A rejected code clears the box and nothing else.** This is the whole
    /// behaviour this feature exists to produce: the master password lives on
    /// the worker thread and is not asked for again, and the provider the user
    /// picked is still picked.
    #[test]
    fn a_rejected_code_clears_the_code_and_keeps_the_provider() {
        let mut prompt = Prompt::new(vec![
            SecondFactor::Authenticator,
            SecondFactor::Email { masked: None },
        ]);
        prompt.choose(SecondFactor::Email { masked: None });
        prompt.email_sent = true;
        prompt.code.push_str("123457");
        prompt.busy = true;

        prompt.went_wrong(Trouble::CodeRejected);

        assert!(prompt.code.is_empty(), "the fat-fingered digits are gone");
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Email { masked: None }),
            "the user's provider choice survives a wrong code"
        );
        assert!(
            prompt.email_sent,
            "the code that was sent is still valid -- re-typing it must not require \
             sending a second one"
        );
        assert!(!prompt.busy, "the buttons come back");
        assert_eq!(prompt.error.as_deref(), Some(message_for(Trouble::CodeRejected)));
    }

    /// A failed SEND leaves the box alone: there may be nothing in it, and if
    /// there is, it is a code from an earlier send that is still good.
    #[test]
    fn a_failed_send_leaves_the_box_alone() {
        let mut prompt = Prompt::new(vec![SecondFactor::Email { masked: None }]);
        prompt.code.push_str("123456");
        prompt.busy = true;

        prompt.went_wrong(Trouble::EmailSendFailed);

        assert_eq!(prompt.code, "123456", "a send failure is not a code failure");
        assert!(!prompt.email_sent, "nothing was sent, so nothing may claim it was");
        assert!(!prompt.busy);
        assert_eq!(
            prompt.error.as_deref(),
            Some(message_for(Trouble::EmailSendFailed))
        );
    }

    /// Only one trouble ends the stage, and it is the one with nothing left on
    /// the worker to retry against.
    #[test]
    fn only_an_expired_challenge_is_fatal() {
        assert!(
            Trouble::ChallengeExpired.is_fatal(),
            "control: the fatal variant is the expired challenge"
        );
        for survivable in [Trouble::CodeRejected, Trouble::EmailSendFailed, Trouble::Unreachable] {
            assert!(
                !survivable.is_fatal(),
                "{survivable:?} still has a challenge on the worker to try again against"
            );
        }
    }


    /// A context with `theme::apply`'s fonts actually live -- a font set
    /// registered during a frame only becomes usable at the start of the next
    /// one, so the throwaway frames are load-bearing. `login_ui`'s paint tests
    /// keep their own copy of this; it is six lines, and the alternative is
    /// making three helpers `pub(crate)` across a 10 000-line file so that one
    /// card can be painted.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(470.0, 588.0),
            )),
            ..Default::default()
        }
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

    /// Every string the card paints once it has risen into place: the screen
    /// fades in over half a second, and a fully transparent frame paints
    /// nothing at all, so the harness runs a dozen frames 0.1 s apart and
    /// reads the last.
    fn painted(prompt: &mut Prompt) -> Vec<String> {
        let ctx = styled_context();
        let mut texts = Vec::new();
        for i in 0..12 {
            let mut input = raw_input();
            input.time = Some(i as f64 * 0.1);
            let output = ctx.run_ui(input, |ui| {
                let _ = draw(ui, prompt);
            });
            texts.clear();
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut texts);
            }
        }
        texts
    }

    fn says(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// The Email card offers to send; the authenticator card does not, and the
    /// authenticator card is the positive control that proves the harness is
    /// painting anything at all.
    #[test]
    fn the_send_button_is_painted_for_email_only() {
        let mut email = Prompt::new(vec![SecondFactor::Email {
            masked: Some("a***@b.c".to_string()),
        }]);
        let email_texts = painted(&mut email);
        assert!(
            says(&email_texts, "a***@b.c"),
            "control: the card painted its own hint; got {email_texts:?}"
        );
        assert!(says(&email_texts, send_code_label()), "got {email_texts:?}");

        let mut totp = Prompt::new(vec![SecondFactor::Authenticator]);
        let totp_texts = painted(&mut totp);
        assert!(
            says(&totp_texts, "authenticator app"),
            "control: the authenticator card painted its own hint; got {totp_texts:?}"
        );
        assert!(!says(&totp_texts, send_code_label()), "got {totp_texts:?}");
    }

    /// The provider switch appears only when there is something to switch to.
    #[test]
    fn the_provider_switch_appears_only_with_more_than_one_factor() {
        let mut two = Prompt::new(vec![SecondFactor::Authenticator, SecondFactor::YubiKey]);
        let two_texts = painted(&mut two);
        assert!(says(&two_texts, other_method_label()), "got {two_texts:?}");

        let mut one = Prompt::new(vec![SecondFactor::Authenticator]);
        let one_texts = painted(&mut one);
        assert!(
            says(&one_texts, stage_heading(&SecondFactor::Authenticator)),
            "control: the single-factor card painted its heading; got {one_texts:?}"
        );
        assert!(!says(&one_texts, other_method_label()), "got {one_texts:?}");
    }

    /// An unsupported-only account gets the message and NO code box: a box
    /// that cannot be submitted is worse than no box.
    #[test]
    fn an_unsupported_only_card_asks_for_nothing() {
        let mut duo = Prompt::new(vec![SecondFactor::Unsupported(2)]);
        let texts = painted(&mut duo);
        assert!(says(&texts, "Duo"), "got {texts:?}");
        assert!(says(&texts, "API key"), "got {texts:?}");
        assert!(!says(&texts, continue_label()), "there is nothing to submit; got {texts:?}");
        // Control: a card that CAN be answered does paint the box, so the
        // absence above is about this account and not about a `draw` that
        // stopped drawing.
        let mut totp = Prompt::new(vec![SecondFactor::Authenticator]);
        assert!(says(&painted(&mut totp), continue_label()), "control: the box is painted at all");
    }

    /// The error line is painted under the box, and it is the one the trouble
    /// named -- not a generic apology.
    #[test]
    fn the_message_from_a_wrong_code_is_painted() {
        let mut prompt = Prompt::new(vec![SecondFactor::Authenticator]);
        let before = painted(&mut prompt);
        assert!(
            !says(&before, message_for(Trouble::CodeRejected)),
            "control: a prompt with no trouble paints no trouble; got {before:?}"
        );
        prompt.went_wrong(Trouble::CodeRejected);
        let after = painted(&mut prompt);
        assert!(says(&after, message_for(Trouble::CodeRejected)), "got {after:?}");
    }


    /// **Asking for a code must not ghost the box the code goes in.** There is
    /// no "the mail left" report -- the worker speaks only in troubles -- so
    /// anything set here has nothing to unset it.
    #[test]
    fn sending_a_code_leaves_the_card_usable() {
        let mut prompt = Prompt::new(vec![SecondFactor::Email { masked: None }]);
        prompt.sent_a_code();
        assert!(prompt.email_sent, "control: the notice is what this call is for");
        assert!(
            !prompt.busy,
            "a card ghosted by a send can only be un-ghosted by a failure, so a send that \
             worked would leave the user unable to type the code it delivered"
        );

        // The control for the other direction: submitting an answer IS a state
        // with an answer coming, and that one may ghost.
        prompt.busy = true;
        prompt.went_wrong(Trouble::CodeRejected);
        assert!(!prompt.busy, "and the answer's failure brings the card back");
    }

    /// **The unsupported-only card now leads somewhere.** Piece 2's own
    /// self-review named this: "If the API-key path does not land, Task 3's
    /// message is a promise this app does not keep." This is the button that
    /// keeps it.
    #[test]
    fn the_unsupported_only_card_offers_the_api_key_it_names() {
        let mut duo = Prompt::new(vec![SecondFactor::Unsupported(2)]);
        let texts = painted(&mut duo);
        assert!(
            says(&texts, "API key"),
            "control: the message this button belongs to is still painted; got {texts:?}"
        );
        assert!(
            says(&texts, crate::api_key_ui::use_api_key_label()),
            "the message tells the user to sign in with an API key and there must be a \
             way to; got {texts:?}"
        );

        // Control on the other direction: a card that CAN be answered does not
        // offer the escape hatch, so the presence above is about this account
        // and not about a button painted on every card.
        let mut totp = Prompt::new(vec![SecondFactor::Authenticator]);
        let texts = painted(&mut totp);
        assert!(
            !says(&texts, crate::api_key_ui::use_api_key_label()),
            "an account with an authenticator app should use it; got {texts:?}"
        );
    }

    // ---- the code stage in the sign-in gallery's layout (slice 15a) --------

    const STAGE: egui::Vec2 = egui::vec2(1280.0, 820.0);

    fn stage_frames(prompt: &mut Prompt) -> (egui::Context, crate::kit::testing::Painted) {
        let ctx = crate::kit::testing::ctx();
        let mut painted = crate::kit::testing::Painted::default();
        for i in 0..8 {
            painted = crate::kit::testing::frame(&ctx, f64::from(i) * 0.2, STAGE, vec![], |ui| {
                let _ = draw(ui, prompt);
            });
        }
        (ctx, painted)
    }

    fn press_continue(prompt: &mut Prompt) -> Option<PromptAction> {
        let (ctx, painted) = stage_frames(prompt);
        let at = painted.text(continue_label()).unwrap().2.center();
        let mut asked = None;
        crate::kit::testing::frame(&ctx, 1.6, STAGE, crate::kit::testing::click_at(at), |ui| {
            asked = draw(ui, prompt);
        });
        asked
    }

    #[test]
    fn the_numeric_methods_share_one_heading_and_a_yubikey_has_its_own() {
        assert_eq!(stage_heading(&SecondFactor::Authenticator), "Enter your 6-digit code");
        assert_eq!(stage_heading(&SecondFactor::Email { masked: None }), "Enter your 6-digit code");
        assert_eq!(stage_heading(&SecondFactor::YubiKey), "Touch your YubiKey");
    }

    #[test]
    fn the_stage_draws_six_boxes_and_the_two_links_for_an_account_with_two_methods() {
        let mut prompt = Prompt::new(vec![
            SecondFactor::Authenticator,
            SecondFactor::Email { masked: Some("a***@b.c".to_string()) },
        ]);
        prompt.code = "482".to_string();
        let (_, painted) = stage_frames(&mut prompt);
        assert_eq!(painted.rects.iter().filter(|r| r.rect.height() == 64.0).count(), 6);
        for word in ["Enter your 6-digit code", continue_label(), back_label(), other_method_label()] {
            assert!(painted.text(word).is_some(), "{word}");
        }
    }

    #[test]
    fn a_one_method_account_has_no_other_method_link() {
        let mut prompt = Prompt::new(vec![SecondFactor::Authenticator]);
        let (_, painted) = stage_frames(&mut prompt);
        assert!(painted.text(other_method_label()).is_none());
        assert!(painted.text(back_label()).is_some());
    }

    #[test]
    fn the_other_method_link_moves_on_and_wraps_round() {
        let prompt = Prompt::new(vec![
            SecondFactor::Authenticator,
            SecondFactor::Email { masked: None },
            SecondFactor::YubiKey,
        ]);
        assert_eq!(next_method(&prompt, &SecondFactor::Authenticator), Some(SecondFactor::Email { masked: None }));
        assert_eq!(next_method(&prompt, &SecondFactor::YubiKey), Some(SecondFactor::Authenticator));
        let single = Prompt::new(vec![SecondFactor::Authenticator]);
        assert_eq!(next_method(&single, &SecondFactor::Authenticator), None);
    }

    #[test]
    fn continue_with_a_short_code_says_so_instead_of_sending_it() {
        let mut prompt = Prompt::new(vec![SecondFactor::Authenticator]);
        prompt.code = "482".to_string();
        assert!(press_continue(&mut prompt).is_none(), "nothing is sent");
        assert_eq!(prompt.error.as_deref(), Some(short_code()));
    }

    #[test]
    fn six_digits_are_sent_when_continue_is_pressed() {
        let mut prompt = Prompt::new(vec![SecondFactor::Authenticator]);
        prompt.code = "482913".to_string();
        assert!(matches!(press_continue(&mut prompt), Some(PromptAction::Submit(_))));
    }

    #[test]
    fn a_busy_stage_reads_checking() {
        let mut prompt = Prompt::new(vec![SecondFactor::Authenticator]);
        prompt.code = "482913".to_string();
        prompt.busy = true;
        let (_, painted) = stage_frames(&mut prompt);
        assert!(painted.text(crate::login_ui::checking_label()).is_some());
    }
}

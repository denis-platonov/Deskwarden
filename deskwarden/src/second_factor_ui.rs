//! The second-factor prompt: the stage between the sign-in card and the
//! spinner.
//!
//! Nothing in this module has ever seen a `Challenge`. See
//! `login_ui::SecondFactorRequest` for what does, and why it stays on the
//! worker thread.

use crate::rest::api::SecondFactor;
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
        SecondFactor::Authenticator => "Authenticator app",
        SecondFactor::Email { .. } => "Email code",
        SecondFactor::YubiKey => "YubiKey",
        SecondFactor::Unsupported(_) => "Two-step login",
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
            "Open your authenticator app and enter the 6-digit code it shows for this \
             account."
                .to_string()
        }
        SecondFactor::Email { masked: Some(address) } => format!(
            "Send a code to {address}, then enter it here. Codes expire after a few \
             minutes."
        ),
        SecondFactor::Email { masked: None } => {
            "Send a code to the email address on this account, then enter it here. Codes \
             expire after a few minutes."
                .to_string()
        }
        SecondFactor::YubiKey => {
            "Plug in your YubiKey, put the cursor in the box below and touch the key. It \
             types the code for you."
                .to_string()
        }
        SecondFactor::Unsupported(_) => {
            "Deskwarden cannot complete this kind of two-step login.".to_string()
        }
    }
}

pub const SEND_CODE_LABEL: &str = "Send code";
pub const CODE_SENT_NOTICE: &str =
    "Code sent. Check your email \u{2014} it may take a moment to arrive.";

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
        2 => "Duo".to_string(),
        6 => "Duo (organization)".to_string(),
        7 => "a security key (WebAuthn)".to_string(),
        4 => "a U2F security key".to_string(),
        5 => "a remembered device".to_string(),
        8 => "a recovery code".to_string(),
        other => format!("an unrecognised two-step method ({other})"),
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
    let list = match names.as_slice() {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {}", rest.join(" and ")),
        [] => return None,
    };
    Some(format!(
        "This account's two-step login uses {list}, which Deskwarden cannot complete. Use a \
         personal API key instead: create one under Account settings \u{2192} Security \u{2192} \
         Keys in the Bitwarden web vault, then sign in here with it. You will still be asked \
         for your master password \u{2014} the API key signs you in, the password unlocks the \
         vault."
    ))
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

}

//! The words of an autofill refusal, said in the language in force.
//!
//! The injector writes its refusals in English: they are log lines as much as
//! sentences for a person, and `notice_card` already splits them on an English
//! prefix. The card that shows one therefore words it here, at the edge, by
//! reading it whole against the English catalog text of the keys below (see
//! [`crate::i18n::edge`]); a sentence this file does not know is shown as it
//! arrived. The English never changes -- only the card's copy is translated.

use crate::i18n;

/// Refusals with nothing in them to fill.
const FIXED: &[&str] = &["autofill.err_already_typing", "autofill.err_nothing"];

/// Refusals with a `{hole}` the injector filled.
const TEMPLATED: &[&str] = &[
    "autofill.err_needs",
    "autofill.err_unsupported",
    "autofill.err_dangling",
    "autofill.err_untypable",
    "autofill.err_too_long",
    "autofill.err_stopped_by",
    "autofill.err_stopped",
    "autofill.err_not_foreground",
];

/// The holes that are phrases of ours: what a sequence needs or uses.
fn phrase(name: &str, value: &str) -> String {
    if name != "what" {
        return value.to_string();
    }
    match value {
        "a username" => i18n::t("autofill.what_username").to_string(),
        "a password" => i18n::t("autofill.what_password").to_string(),
        "a one-time code" => i18n::t("autofill.what_totp").to_string(),
        other => {
            if let Some(field) = other.strip_prefix("a field called ") {
                return i18n::tf("autofill.what_field", &[("name", field)]);
            }
            if let Some(group) = other.strip_prefix("a ").and_then(|rest| rest.strip_suffix(" group")) {
                return i18n::tf("autofill.what_group", &[("group", group)]);
            }
            other.to_string()
        }
    }
}

/// A refusal's sentence in the language in force, or `None` if it is not one
/// this file knows (the card then shows it as it arrived).
pub fn localize(message: &str) -> Option<String> {
    i18n::edge::localize(FIXED, TEMPLATED, message, phrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::sequence::Refusal;

    /// Every refusal the planner can make, and the two the injector makes
    /// beside it, is one this file words: a new one without a key is English
    /// in every language, and this is the test that says so.
    #[test]
    fn every_refusal_the_injector_writes_is_one_the_card_can_word() {
        let all = [
            Refusal::Unresolved("a username".into()),
            Refusal::Unresolved("a password".into()),
            Refusal::Unresolved("a one-time code".into()),
            Refusal::Unresolved("a field called PIN".into()),
            Refusal::Unsupported("a ( group".into()),
            Refusal::Unsupported("{UNKNOWN}".into()),
            Refusal::DanglingModifier("Ctrl".into()),
            Refusal::UntypableKey("TAB".into()),
            Refusal::TooLong(std::time::Duration::from_secs(90)),
            Refusal::Nothing,
        ];
        for refusal in all {
            let english = refusal.message();
            let said = localize(&english).unwrap_or_else(|| panic!("no key words {english:?}"));
            assert_eq!(said, english, "in English the card's words are the injector's words");
        }
        let typing = crate::injector::ALREADY_TYPING;
        assert_eq!(localize(typing).as_deref(), Some(typing));
        let stopped = "window 7 is no longer in front (after 2 of 5 steps). Nothing further was typed.";
        assert_eq!(localize(stopped).as_deref(), Some(stopped));
        let taken = "window \u{201c}mabl\u{201d} (mabl.exe) is no longer in front; \u{201c}Slack\u{201d} (slack.exe) is (after 2 of 5 steps). Nothing further was typed.";
        assert_eq!(localize(taken).as_deref(), Some(taken), "the sentence naming what took over is worded too");
        assert_eq!(localize("something this file does not know"), None);
    }

    /// A phrase inside a sentence is worded as the phrase it is, not copied.
    #[test]
    fn a_field_the_sequence_names_keeps_its_name() {
        let said = localize("the auto-type sequence needs a field called PIN, and this item has none").expect("known");
        assert!(said.contains("PIN"), "{said}");
    }
}

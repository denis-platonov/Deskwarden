//! The words of a Send, said in the language in force.
//!
//! `crate::send` and `crate::rest::send` stay English: they are reachable
//! from `job_object`'s fenced runners, so they may not name `i18n`, and what
//! they produce is also what the log says. Their sentences are therefore
//! worded HERE, at the edge, by reading each one whole against the English
//! catalog text of the keys listed below -- the rule `login_ui::error_spot`
//! already follows -- and a sentence this file does not know (the line `bw`
//! printed, an error from the OS) is shown as it arrived. Logic never reads
//! the result: it is only ever painted.
//!
//! Also here: the lifetime picker's labels and the sentence under it, which
//! used to be `send::lifetime_label` and `send::expiry_wording` and need the
//! catalog for their plurals.

use crate::i18n;
use crate::local_time::{self, LocalOffset};
use crate::send::{deletion_instant, SendClock, SendError, SendLifetime};

/// Sentences with nothing in them to fill. Each key's English is the exact
/// sentence the producing module writes.
const FIXED: &[&str] = &[
    "sends.err_not_started",
    "sends.err_not_verified",
    "sends.err_list_unreadable",
    "sends.err_list_no_id",
    "sends.err_list_no_link",
    "sends.err_not_a_send",
    "sends.err_read_back",
    "sends.err_extend_unconfirmed",
    "sends.err_name_required",
    "sends.err_name_long",
    "sends.err_nothing_to_send",
    "sends.err_text_long",
    "sends.err_lifetime_choices",
    "sends.err_date_passed",
    "sends.err_date_far",
    "sends.err_empty_password",
    "sends.err_zero_views",
    "sends.err_encrypt_failed",
    "sends.err_unconfirmed_live",
    "sends.err_no_profile",
    "sends.err_key_unreadable",
    "sends.err_key_unwrapped",
    "sends.err_name_unreadable",
    "sends.err_undecryptable",
    "sends.err_not_a_list",
    "sends.err_link_no_text",
    "sends.err_link_text_unreadable",
    "sends.err_link_protected",
    "sends.err_link_wrong_password",
    "sends.err_link_gone",
    "sends.err_link_email_gated",
    "sends.err_link_file_send",
    "sends.err_link_no_route",
    "sends.err_link_key_wrong",
    "sends.err_link_missing_parts",
    "sends.err_link_key_unreadable",
    "sends.err_link_key_length",
    "sends.err_link_no_key",
];

/// Sentences with a `{hole}` in them, which the producer fills.
const TEMPLATED: &[&str] = &[
    "sends.err_refused",
    "sends.err_answer_no_field",
    "sends.err_field_unreadable",
    "sends.err_started_with",
    "sends.err_handed_over",
    "sends.err_link_no_server",
    "sends.err_link_other_server",
];

/// A producer's `&'static str` sentence in the language in force; one this
/// file does not know is returned as it is.
pub fn refusal(sentence: &'static str) -> &'static str {
    match i18n::edge::fixed_key(FIXED, sentence) {
        Some(key) => i18n::t(key),
        None => sentence,
    }
}

/// The one hole that is itself a phrase of ours.
fn phrase(name: &str, value: &str) -> String {
    if name != "what" {
        return value.to_string();
    }
    match value {
        "deletion date" => i18n::t("sends.what_deletion_date").to_string(),
        "expiration date" => i18n::t("sends.what_expiration_date").to_string(),
        _ => value.to_string(),
    }
}

/// A producer's sentence in the language in force, or `None` if it is not one
/// this file knows.
pub fn localize(message: &str) -> Option<String> {
    i18n::edge::localize(FIXED, TEMPLATED, message, phrase)
}

impl SendError {
    /// The sentence to show, in the language in force. **No message may read
    /// as a plain success**; `no_failure_message_reads_as_a_success` holds that
    /// over every arm.
    pub fn user_message(&self) -> String {
        match self {
            Self::Locked => i18n::t("sends.err_locked").to_string(),
            Self::Offline => i18n::t("sends.err_offline").to_string(),
            Self::FailedSilently => i18n::t("sends.err_silent").to_string(),
            Self::CreatedButUnreadable => i18n::t("sends.err_created_unreadable").to_string(),
            // Reached from create, from list AND from revoke, so it may not
            // claim a Send was created: on a timed-out revoke that is false,
            // and the true worry there -- the link may still be live -- went
            // unsaid.
            Self::TimedOut => i18n::t("sends.err_timed_out").to_string(),
            Self::NoVerifiedCli(why) | Self::Rejected(why) | Self::SpawnFailed(why) => {
                localize(why).unwrap_or_else(|| why.clone())
            }
        }
    }
}

// ---- the lifetime picker -------------------------------------------------

/// The row label for the choice that has no fixed value, while no date has
/// been picked. The ellipsis is the app's promise that pressing it opens
/// something rather than answering the question.
pub fn pick_a_date_label() -> &'static str {
    i18n::t("sends.pick_a_date_label")
}

/// What [`SendLifetime::Never`] is called, everywhere it is named. One word,
/// and no qualifier after it: it is the right answer to sharing a household
/// credential with someone who will want it again next month.
pub fn never_label() -> &'static str {
    i18n::t("sends.never_label")
}

/// The sentence for [`SendLifetime::Never`] under the picker. Deletion is
/// named as the thing that ends the link, and no place is named to do it.
pub fn never_wording() -> &'static str {
    i18n::t("sends.never_wording")
}

/// A lifetime as the picker names it: `1 hour`, `7 days`, `3 months`, `Never`,
/// or the picked day itself. A whole number of days says days; the day is the
/// user's own (`zone`), because the stored instant is UTC.
pub fn lifetime_label(lifetime: SendLifetime, zone: &dyn LocalOffset) -> String {
    match lifetime {
        SendLifetime::Hours(hours) => {
            if hours % 24 == 0 && hours != 0 {
                i18n::tn("sends.lifetime_days", u64::from(hours / 24), &[])
            } else {
                i18n::tn("sends.lifetime_hours", u64::from(hours), &[])
            }
        }
        SendLifetime::Months(months) => i18n::tn("sends.lifetime_months", u64::from(months), &[]),
        SendLifetime::Until(at) => local_time::format_day(local_time::local_parts(at, zone)),
        SendLifetime::Never => never_label().to_string(),
    }
}

/// What the form says under the lifetime picker: **the date the link dies**,
/// in the user's own timezone, and for a sub-day lifetime the clock time too.
pub fn expiry_wording(lifetime: SendLifetime, now: &dyn SendClock, zone: &dyn LocalOffset) -> String {
    let Some(expires_at) = deletion_instant(lifetime, now) else {
        return never_wording().to_string();
    };
    let parts = local_time::local_parts(expires_at, zone);
    match lifetime {
        SendLifetime::Until(_) => {
            let day = local_time::format_day(parts);
            i18n::tf("sends.expiry_until_end_of", &[("day", &day)])
        }
        _ => {
            let sub_day = matches!(lifetime, SendLifetime::Hours(hours) if hours < 24);
            let when = if sub_day { local_time::format_day_time(parts) } else { local_time::format_day(parts) };
            i18n::tf(
                "sends.expiry_stops_after",
                &[("after", &lifetime_label(lifetime, zone)), ("when", &when)],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Rust string literal's value: `\"`, `\\`, `\n` and the `\`-newline
    /// continuation, which is all the producers use.
    fn unescape(source: &str) -> String {
        let mut out = String::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('\n') => {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
                Some(other) => out.push(other),
                None => {}
            }
        }
        out
    }

    /// Every literal handed straight to `Rejected(`, `SpawnFailed(` or
    /// `NoVerifiedCli(` (also through `format!(` and the `rejected(` helper)
    /// in the production half of a source file.
    fn literals_handed_to_errors(source: &str) -> Vec<String> {
        let source = source.replace("\r\n", "\n");
        let production = source.split("#[cfg(test)]\nmod ").next().unwrap_or(&source);
        let mut found = Vec::new();
        for opener in ["Rejected(", "SpawnFailed(", "NoVerifiedCli(", "rejected("] {
            let mut from = 0;
            while let Some(at) = production[from..].find(opener) {
                let mut rest = production[from + at + opener.len()..].trim_start();
                if let Some(inner) = rest.strip_prefix("&format!(").or_else(|| rest.strip_prefix("format!(")) {
                    rest = inner.trim_start();
                }
                if let Some(body) = rest.strip_prefix('"') {
                    let mut end = 0;
                    let bytes = body.as_bytes();
                    while end < bytes.len() && bytes[end] != b'"' {
                        end += if bytes[end] == b'\\' { 2 } else { 1 };
                    }
                    found.push(unescape(&body[..end]));
                }
                from += at + opener.len();
            }
        }
        found
    }

    /// Every sentence a producer hands to `SendError` is one this file words:
    /// a sentence added in `send.rs` or `rest/send.rs` without a key is
    /// English in every language, and this is the test that says so.
    #[test]
    fn every_sentence_the_producers_write_is_one_the_screen_can_word() {
        let mut checked = 0;
        let mut unknown: Vec<String> = Vec::new();
        for (file, source) in [
            ("send.rs", include_str!("../../send.rs")),
            ("rest/send.rs", include_str!("../../rest/send.rs")),
            ("rest/send_link.rs", include_str!("../../rest/send_link.rs")),
        ] {
            for sentence in literals_handed_to_errors(source) {
                // A template's holes stand for anything.
                let mut probe = String::new();
                let mut inside = false;
                for c in sentence.chars() {
                    match c {
                        '{' => {
                            inside = true;
                            probe.push('X');
                        }
                        '}' => inside = false,
                        _ if !inside => probe.push(c),
                        _ => {}
                    }
                }
                if localize(&probe).is_none() && localize(&sentence).is_none() {
                    unknown.push(format!("{file}: {sentence:?}"));
                }
                checked += 1;
            }
        }
        assert!(unknown.is_empty(), "no key in send_ui/words.rs words these sentences, so they are English in every language:
{}", unknown.join("
"));
        assert!(checked > 30, "control: the scan found only {checked} sentences, so it is not looking at the producers");
    }

    /// Every key in the two tables exists in English, and its English is what
    /// the producer writes -- the matching is by that text.
    #[test]
    fn the_tables_name_real_catalog_sentences_and_a_sentence_matches_only_its_own_key() {
        for key in FIXED.iter().chain(TEMPLATED) {
            let text = i18n::edge::english(key);
            assert_ne!(text, *key, "{key} is not in the English catalog");
        }
        for key in FIXED {
            assert_eq!(i18n::edge::fixed_key(FIXED, i18n::edge::english(key)), Some(*key), "{key} is read as another sentence");
        }
    }

    #[test]
    fn a_templated_sentence_keeps_what_the_producer_filled_in() {
        let said = localize("Bitwarden would not do it: Not found.").expect("the refusal template");
        assert_eq!(said, "Bitwarden would not do it: Not found.");
        let said = localize("This Send's deletion date could not be read, so nothing was changed.")
            .expect("the field template");
        assert_eq!(said, "This Send's deletion date could not be read, so nothing was changed.");
        let said = localize(
            "That Send link is on a.example, but this account is on b.example. Deskwarden will not send your share \
             password to a server you did not configure.",
        )
        .expect("the server template");
        assert!(said.contains("a.example") && said.contains("b.example"), "{said}");
        // What the words file does not know is shown as it arrived.
        assert_eq!(localize("something the OS said"), None);
        assert_eq!(SendError::Rejected("something the OS said".into()).user_message(), "something the OS said");
    }

    /// Plural labels agree with the number, in English, as `send.rs` always
    /// said them.
    #[test]
    fn lifetime_labels_say_one_hour_and_seven_days() {
        struct Utc;
        impl LocalOffset for Utc {
            fn offset_millis_at(&self, _utc_millis: i64) -> i64 {
                0
            }
        }
        assert_eq!(lifetime_label(SendLifetime::Hours(1), &Utc), "1 hour");
        assert_eq!(lifetime_label(SendLifetime::Hours(24 * 7), &Utc), "7 days");
        assert_eq!(lifetime_label(SendLifetime::Hours(24), &Utc), "1 day");
        assert_eq!(lifetime_label(SendLifetime::Months(1), &Utc), "1 month");
        assert_eq!(lifetime_label(SendLifetime::Months(3), &Utc), "3 months");
        assert_eq!(lifetime_label(SendLifetime::Never, &Utc), "Never");
    }

    const NOW: crate::send::FixedClock = crate::send::FixedClock(1_786_408_997_148);

    /// The refusal for a lifetime that is not on the picker's row.
    fn choices_sentence() -> &'static str {
        crate::send::validate_access(SendLifetime::Hours(3), None, None, &NOW).expect("an off-row lifetime is refused")
    }

    use crate::send::{deletion_date, validate_access, LIFETIME_CHOICES};

    /// **The refusal names every lifetime the picker offers**, which is what
    /// makes a `&'static str` safe to hand-write beside an array.
    ///
    /// The old sentence said "1, 7 or 30 days" and was correct for exactly as
    /// long as the picker had three cells of days. This holds the sentence
    /// against [`LIFETIME_CHOICES`] through [`lifetime_label`] -- the same
    /// function that labels the rows -- so a choice added, removed or renamed
    /// reds this rather than leaving a refusal that names a row the user is
    /// not looking at.
    ///
    /// The picked-date row has no constant to label, so the sentence is held
    /// against the words the picker actually puts on it instead.
    #[test]
    fn the_lifetime_refusal_names_every_choice_the_picker_offers() {
        let utc = crate::local_time::FixedOffset(0);
        let sentence = choices_sentence();
        // Control: the needles are real. Without this an empty `choices`
        // would satisfy the loop below and say nothing.
        assert_eq!(LIFETIME_CHOICES.len(), 9);
        assert_eq!(LIFETIME_CHOICES.iter().filter(|row| row.is_none()).count(), 1);
        for choice in LIFETIME_CHOICES.into_iter().flatten() {
            let label = lifetime_label(choice, &utc);
            assert!(
                sentence.contains(&label),
                "the picker offers {label:?} and the refusal does not name it: \
                 {sentence:?}"
            );
        }
        assert!(
            sentence.contains("a date you pick"),
            "the picker offers a date and the refusal does not name it"
        );
        assert_eq!(
            validate_access(SendLifetime::Hours(3), None, None, &NOW),
            Some(sentence),
            "a lifetime off the row is refused with some other sentence"
        );
    }


    /// **Every choice is named once, in one unit per magnitude.**
    ///
    /// `1 hour` is singular, `24` is a day and not twenty-four hours, and no
    /// two cells in the row carry the same words -- a picker with two cells
    /// reading `1 day` would be a control the user cannot use, and it is
    /// exactly what a `hours % 24` branch gets wrong if it ever loses the
    /// `hours != 0` guard.
    #[test]
    fn every_lifetime_is_named_once_and_in_the_right_unit() {
        let utc = crate::local_time::FixedOffset(0);
        let named: Vec<String> = LIFETIME_CHOICES
            .into_iter()
            .flatten()
            .map(|choice| lifetime_label(choice, &utc))
            .collect();
        assert_eq!(
            named,
            vec![
                "1 hour", "1 day", "7 days", "30 days", "3 months", "6 months", "12 months",
                "Never"
            ]
        );
        let mut unique = named.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), named.len(), "two rows of the picker carry one label");
        // Off the row, so that the plural rules are tested and not merely the
        // eight strings above.
        assert_eq!(lifetime_label(SendLifetime::Hours(2), &utc), "2 hours");
        assert_eq!(lifetime_label(SendLifetime::Hours(23), &utc), "23 hours");
        assert_eq!(lifetime_label(SendLifetime::Hours(48), &utc), "2 days");
        assert_eq!(lifetime_label(SendLifetime::Months(1), &utc), "1 month");
        // **A picked date is named as its LOCAL day.** The instant stored is
        // the end of 14 March in the zone it was picked in; five hours west of
        // Greenwich it is still the 14th, and a label built off the UTC parts
        // would say the 15th.
        let new_york = crate::local_time::FixedOffset(-5 * 3_600_000);
        let picked = SendLifetime::on_local_day(2027, 3, 14, &new_york);
        assert_eq!(lifetime_label(picked, &new_york), "14 Mar 2027");
        // **`Never` is one word and never a date.** The wire carries the year
        // 9999 and no reader of this label may ever see it.
        let never = lifetime_label(SendLifetime::Never, &utc);
        assert_eq!(never, never_label());
        assert!(!never.contains("9999"), "the sentinel leaked into a label: {never:?}");
        assert!(
            !expiry_wording(SendLifetime::Never, &NOW, &utc).contains("9999"),
            "the sentinel leaked into the sentence under the picker"
        );
    }


    #[test]
    fn the_expiry_wording_names_the_date_and_not_only_the_number_of_days() {
        // UTC, so that the dates below read as they always have. The
        // timezone-dependent behaviour has its own tests directly beneath.
        let utc = crate::local_time::FixedOffset(0);
        let seven = expiry_wording(SendLifetime::Hours(24 * 7), &NOW, &utc);
        assert!(seven.contains("7 days"), "{seven:?}");
        assert!(
            seven.contains("18 Aug 2026"),
            "the wording gives a duration but not the date it lands on, which is the thing a \
             user can check: {seven:?}"
        );
        let one = expiry_wording(SendLifetime::Hours(24), &NOW, &utc);
        assert!(one.contains("1 day") && !one.contains("1 days"), "{one:?}");
        assert!(one.contains("12 Aug 2026"), "{one:?}");
        assert!(
            expiry_wording(SendLifetime::Hours(24 * 30), &NOW, &utc).contains("10 Sep 2026"),
            "{}",
            expiry_wording(SendLifetime::Hours(24 * 30), &NOW, &utc)
        );
        // The wording and the JSON must not disagree about the day; two
        // separate formatters over the same instant is exactly how they would.
        // At UTC+0 the two are the same instant AND the same reading, which is
        // the only offset at which this equality is the right assertion --
        // see `the_wording_is_local_while_the_stored_deletion_date_stays_utc`.
        for hours in [1u32, 24, 24 * 7, 24 * 30] {
            let wording = expiry_wording(SendLifetime::Hours(hours), &NOW, &utc);
            let iso = deletion_date(SendLifetime::Hours(hours), &NOW);
            let parts = crate::local_time::civil_parts(
                NOW.now_unix_millis() + i64::from(hours) * crate::local_time::MILLIS_PER_HOUR,
            );
            let (y, m, d) = (parts.year, parts.month, parts.day);
            assert!(iso.starts_with(&format!("{y:04}-{m:02}-{d:02}")), "{iso:?}");
            assert!(
                wording.contains(&format!(
                    "{d} {} {y}",
                    crate::local_time::month_name(m)
                )),
                "the wording and the deletion date disagree: {wording:?} vs {iso:?}"
            );
        }
    }


    /// **An hour-long link is told the TIME it dies, and a day-long one is
    /// not.**
    ///
    /// The sentence's whole argument is that a date is checkable where a
    /// duration is arithmetic -- and for a one-hour Send a bare date is worse
    /// than either, because "on 11 Aug 2026" under a link with forty minutes
    /// to live reads as reassurance. Under a day it names the clock time;
    /// from a day up it does not, because the minute a seven-day link dies is
    /// not something the sender will act on.
    ///
    /// Both halves are asserted. A version that always printed the time would
    /// pass the first and fail the second, and a version that never did would
    /// pass the second and fail the first.
    #[test]
    fn a_sub_day_lifetime_names_the_time_and_a_longer_one_does_not() {
        let utc = crate::local_time::FixedOffset(0);

        let hour = expiry_wording(SendLifetime::Hours(1), &NOW, &utc);
        assert!(hour.contains("1 hour") && !hour.contains("1 hours"), "{hour:?}");
        assert!(
            hour.contains("11 Aug 2026, 01:43"),
            "a one-hour link is not told the clock time it dies at, so the sentence under the \
             picker names a whole day for a lifetime measured in minutes: {hour:?}"
        );

        for hours in [24u32, 24 * 7, 24 * 30] {
            let wording = expiry_wording(SendLifetime::Hours(hours), &NOW, &utc);
            assert!(
                !wording.contains("00:43"),
                "a {hours}-hour link is being given a clock time it has not earned: {wording:?}"
            );
        }
    }


    /// **The defect this sentence carried, as a test.**
    ///
    /// `NOW` plus one day is `2026-08-12T00:43:17.148Z` -- forty-three
    /// minutes past midnight UTC. Five hours west, that is the evening of the
    /// **11th**, so the old sentence ("on 12 Aug 2026 (UTC)") named a day on
    /// which the link would already have been dead for five hours, to a user
    /// standing in New York.
    ///
    /// The offset is injected, so this assertion is exact wherever and
    /// whenever the suite runs -- which is the other half of the rule: no
    /// test in this crate may read the machine's clock or its timezone.
    #[test]
    fn the_expiry_date_is_the_users_own_day_and_not_the_utc_one() {
        let new_york = crate::local_time::FixedOffset(-5 * 3_600_000);
        let wording = expiry_wording(SendLifetime::Hours(24), &NOW, &new_york);
        assert!(
            wording.contains("11 Aug 2026"),
            "just-past-midnight UTC on the 12th is the evening of the 11th at UTC-5, and the \
             day the user can check is theirs: {wording:?}"
        );
        assert!(
            !wording.contains("12 Aug 2026"),
            "the UTC day is still the one being shown: {wording:?}"
        );
    }


    /// **No label in this app says "UTC" to a user**, and this is the one
    /// that used to. A parenthesis naming a timezone is not a fix for a date
    /// in the wrong timezone; it is an instruction to do arithmetic, aimed at
    /// a reader who has no reason to know there is arithmetic to do.
    #[test]
    fn the_expiry_wording_never_names_a_timezone() {
        for offset in [-11, -5, 0, 1, 5, 13] {
            let zone = crate::local_time::FixedOffset(offset * 3_600_000);
            for choice in LIFETIME_CHOICES.into_iter().flatten() {
                let wording = expiry_wording(choice, &NOW, &zone);
                assert!(
                    !wording.contains("UTC") && !wording.contains("GMT"),
                    "{wording:?} names a timezone at offset {offset}"
                );
            }
        }
    }


    /// **Store UTC, display local**, both halves in one assertion: the
    /// sentence moves with the zone and the stored `deletionDate` does not.
    ///
    /// The stored value is what `bw` is handed and what the Bitwarden server
    /// records. Shifting it would not be a display change -- it would move
    /// the moment the link actually dies.
    #[test]
    fn the_wording_is_local_while_the_stored_deletion_date_stays_utc() {
        let iso = deletion_date(SendLifetime::Hours(24), &NOW);
        assert!(iso.ends_with('Z'), "the stored instant is UTC and says so: {iso:?}");
        let mut readings = Vec::new();
        for offset in [-11, -5, 0, 5, 13] {
            let zone = crate::local_time::FixedOffset(offset * 3_600_000);
            readings.push(expiry_wording(SendLifetime::Hours(24), &NOW, &zone));
            assert_eq!(
                deletion_date(SendLifetime::Hours(24), &NOW),
                iso,
                "the stored deletion date moved with the display timezone, which would change \
                 when the link dies rather than how it is described"
            );
        }
        readings.dedup();
        assert!(
            readings.len() > 1,
            "every timezone produced the same sentence, so nothing is being converted: {readings:?}"
        );
    }
}

//! How long ago something happened, in words, for every surface in this
//! window that says so.
//!
//! # Why this is one function and not three
//!
//! There were two, and they were the same defect twice. The toolbar sync
//! pill counted minutes with no ceiling and read "Syncing 1200 minutes ago"
//! after a window stayed open for twenty hours; the detail pane's metadata
//! strip counted days with no ceiling and reads "Updated 700 days ago" for an
//! item nobody has touched in two years. Each one knew exactly one unit and
//! carried its own enumeration of the wordings -- the "two enumerations that
//! must agree" shape this crate keeps losing to. So the wording lives here,
//! once, and `mod.rs`'s `synced_ago_text`, `detail.rs`'s `updated_text` and
//! `detail.rs`'s `history_label` all read it.
//!
//! # The format, and what was rejected
//!
//! **At most two units, largest first, no zero tail**: `45s ago`,
//! `5m ago`, `20h 15m ago`, `3d 4h ago`, `1y 35d ago`.
//!
//!  * **Two units, not five.** `1y 2mo 3d 4h 5m ago` is five significant
//!    figures on a number the reader is using to decide "recently or not".
//!    The second unit is what separates "just over a day" from "nearly two";
//!    a third never changes a reader's mind.
//!  * **No months.** A month is not a fixed length, so `mo` would have to
//!    pick one and be wrong for half the year -- and the obvious short
//!    suffix, `m`, is already minutes. Dropping the unit entirely removes
//!    both problems at once: the ladder is `s` -> `m` -> `h` -> `d` -> `y`
//!    and no two suffixes collide. `1y 35d ago` is longer in characters
//!    than `1y 1mo ago` and shorter in questions.
//!  * **Seconds never pair.** Under a minute reads `45s ago` alone; from a
//!    minute up, seconds are dropped rather than shown beside minutes, so
//!    the pill never flickers a `1m 3s` / `1m 4s` digit at the user once a
//!    second. The pairing starts at hours, where the second unit is
//!    genuinely load-bearing.
//!
//! # What "a year" means here
//!
//! 365 days, flat. This is a relative-time label on a UI strip, not a
//! calendar computation: the reader of "1y 35d ago" is deciding whether an
//! item is stale, and a leap day inside that answer is not a difference
//! anybody acts on. Stated so nobody has to reverse-engineer it from the
//! constant.

use std::time::Duration;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
/// See the module doc: a flat 365 days, deliberately.
const YEAR: u64 = 365 * DAY;

/// `elapsed` as a relative-time phrase, suffix included: `"45s ago"`,
/// `"20h 15m ago"`, `"3d 4h ago"`, `"1y 35d ago"`.
///
/// The " ago" is part of this function's output rather than each caller's
/// `format!`, so the three surfaces cannot drift on the suffix either.
///
/// **A pure function of a `Duration` and nothing else** -- no clock, no
/// locale, no state -- which is what lets the boundaries be tested directly
/// instead of through a window. Callers that want a floor other than a
/// second ("just now", "today") impose it themselves, because that choice is
/// about what their data means and not about how a duration is spelled: see
/// each call site.
pub fn ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= YEAR {
        pair(secs / YEAR, "y", (secs % YEAR) / DAY, "d")
    } else if secs >= DAY {
        pair(secs / DAY, "d", (secs % DAY) / HOUR, "h")
    } else if secs >= HOUR {
        pair(secs / HOUR, "h", (secs % HOUR) / MINUTE, "m")
    } else if secs >= MINUTE {
        // Deliberately no seconds tail -- see the module doc.
        pair(secs / MINUTE, "m", 0, "s")
    } else {
        pair(secs, "s", 0, "")
    }
}

/// `"3d 4h ago"`, or `"3d ago"` when the smaller unit is zero.
///
/// The zero drop is not cosmetic: "1y 0d ago" invites the reader to wonder
/// what happened to the days, and every duration that lands exactly on a
/// unit boundary would carry one.
fn pair(large: u64, large_unit: &str, small: u64, small_unit: &str) -> String {
    if small == 0 {
        format!("{large}{large_unit} ago")
    } else {
        format!("{large}{large_unit} {small}{small_unit} ago")
    }
}

/// `ago` for a caller whose data is a whole number of **days** and carries no
/// finer granularity than that.
///
/// `detail.rs` derives its numbers from `days_since`, which parses only the
/// `YYYY-MM-DD` prefix of a `revisionDate`. It genuinely does not know the
/// hour, so this exists to stop a caller from inventing one: hand it days and
/// it hands back a phrase whose smallest unit can only ever be a day
/// (`"3d ago"`, `"1y 35d ago"`) because the hours remainder of a whole number
/// of days is always zero. Negative and zero days are the caller's business
/// -- both mean "today" to the surfaces that ask, and both say so in their
/// own words.
pub fn ago_days(days: u64) -> String {
    ago(Duration::from_secs(days * DAY))
}

#[cfg(test)]
mod tests {
    use super::{ago, ago_days};
    use std::time::Duration;

    fn at(secs: u64) -> String {
        ago(Duration::from_secs(secs))
    }

    #[test]
    fn under_a_minute_is_seconds_alone() {
        assert_eq!(at(0), "0s ago");
        assert_eq!(at(1), "1s ago");
        assert_eq!(at(45), "45s ago");
    }

    /// The first boundary, both sides of it.
    #[test]
    fn fifty_nine_seconds_is_seconds_and_sixty_is_a_minute() {
        assert_eq!(at(59), "59s ago");
        assert_eq!(at(60), "1m ago");
        assert_eq!(at(61), "1m ago", "the seconds tail is dropped, not carried");
    }

    #[test]
    fn minutes_never_show_a_seconds_tail() {
        assert_eq!(at(125), "2m ago");
        assert_eq!(at(45 * 60 + 59), "45m ago");
    }

    /// 59m/60m: the minute-to-hour rollover, and the point at which the
    /// second unit switches on.
    #[test]
    fn fifty_nine_minutes_is_minutes_and_sixty_is_an_hour() {
        assert_eq!(at(59 * 60), "59m ago");
        assert_eq!(at(3599), "59m ago");
        assert_eq!(at(3600), "1h ago");
        assert_eq!(at(3600 + 59), "1h ago", "under a minute past the hour adds nothing");
        assert_eq!(at(3600 + 60), "1h 1m ago");
    }

    /// The defect this module was written for: twenty hours used to read
    /// "1200 min ago".
    #[test]
    fn twenty_hours_reads_in_hours_and_minutes() {
        assert_eq!(at(20 * 3600 + 15 * 60), "20h 15m ago");
        assert_eq!(at(1200 * 60), "20h ago");
    }

    /// 23h/24h: the hour-to-day rollover.
    #[test]
    fn twenty_three_hours_is_hours_and_twenty_four_is_a_day() {
        assert_eq!(at(23 * 3600 + 59 * 60 + 59), "23h 59m ago");
        assert_eq!(at(86_399), "23h 59m ago");
        assert_eq!(at(86_400), "1d ago");
        assert_eq!(at(86_400 + 3599), "1d ago", "under an hour past the day adds nothing");
        assert_eq!(at(86_400 + 3600), "1d 1h ago");
        assert_eq!(at(3 * 86_400 + 4 * 3600), "3d 4h ago");
    }

    /// The largest rollover: 364d/365d. This is the one with no unit above
    /// it, so an off-by-one here has nowhere to be caught downstream.
    #[test]
    fn three_hundred_sixty_four_days_is_days_and_three_hundred_sixty_five_is_a_year() {
        assert_eq!(at(364 * 86_400), "364d ago");
        assert_eq!(at(365 * 86_400 - 1), "364d 23h ago");
        assert_eq!(at(365 * 86_400), "1y ago");
        assert_eq!(at(365 * 86_400 + 86_399), "1y ago", "under a day past the year adds nothing");
        assert_eq!(at(366 * 86_400), "1y 1d ago");
    }

    /// The second defect's number: 700 days used to read "700 days ago".
    #[test]
    fn seven_hundred_days_reads_in_years_and_days() {
        assert_eq!(ago_days(700), "1y 335d ago");
    }

    #[test]
    fn nothing_ever_names_more_than_two_units() {
        for secs in [0, 59, 60, 3599, 3600, 86_399, 86_400, 365 * 86_400, 4000 * 86_400] {
            let text = at(secs);
            let stripped = text.strip_suffix(" ago").unwrap_or_else(|| {
                panic!("{secs}s produced {text:?}, which does not end in \" ago\"")
            });
            assert!(
                stripped.split(' ').count() <= 2,
                "{secs}s produced {text:?}, which names more than two units"
            );
        }
    }

    /// No two suffixes collide, which is the whole reason months are gone.
    /// Read off the output rather than off the constants, so a suffix changed
    /// in `ago` and not here is a red.
    #[test]
    fn every_suffix_is_distinct() {
        let suffixes: Vec<char> = [1u64, 60, 3600, 86_400, 365 * 86_400]
            .iter()
            .map(|secs| {
                let text = at(*secs);
                text.strip_suffix(" ago")
                    .and_then(|s| s.chars().last())
                    .expect("a suffix character")
            })
            .collect();
        assert_eq!(suffixes, vec!['s', 'm', 'h', 'd', 'y'], "the unit ladder changed");
        let mut sorted = suffixes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), suffixes.len(), "two units share a suffix: {suffixes:?}");
    }

    /// [`ago_days`]'s claim: a whole number of days can never produce an
    /// hours tail, so a day-granularity caller cannot accidentally show a
    /// precision its data does not carry.
    #[test]
    fn a_whole_number_of_days_never_grows_an_hours_tail() {
        for days in [0u64, 1, 2, 30, 364, 365, 366, 700, 4000] {
            let text = ago_days(days);
            assert!(
                !text.contains('h'),
                "{days} whole days produced {text:?}, which claims an hour it does not know"
            );
        }
    }

    #[test]
    fn ago_days_agrees_with_ago_over_the_same_span() {
        for days in [0u64, 1, 3, 364, 365, 700] {
            assert_eq!(ago_days(days), ago(Duration::from_secs(days * 86_400)));
        }
    }
}

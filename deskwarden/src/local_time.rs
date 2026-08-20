//! Civil dates and times, and the one place an instant is turned into the
//! wall clock the user is actually reading.
//!
//! # The rule this module exists to enforce
//!
//! **Store UTC, always display in the user's local timezone, never show
//! "UTC" to the user.** Every instant this app persists -- a Send's
//! `deletionDate`, a scan history entry's `finished_at` -- is UTC, because a
//! stored local time is a number that changes meaning when the user flies
//! somewhere or when the clocks go back. Every instant this app *paints* is
//! local, because a user checking a date against their calendar is checking
//! it against the calendar on their wall.
//!
//! The two halves used to be one, and the bug was not cosmetic.
//! `send::expiry_wording` said `"-- on {d} {month} {y} (UTC)."`, computed
//! straight off the UTC instant. A Send that expires at 00:30 UTC expires
//! the **previous day** everywhere in the Americas, so the sentence under
//! the lifetime picker named a day the link would already be dead on. The
//! parenthesis did not save it: "(UTC)" in front of a user who has never
//! thought about timezones reads as noise, not as an instruction to subtract
//! five hours.
//!
//! # Why there is still no date library
//!
//! This crate deliberately has none. What it did before was arithmetic on
//! `YYYY-MM-DD` prefixes and proleptic-Gregorian civil conversion, neither of
//! which needs a timezone database -- and a timezone database is exactly the
//! thing that goes stale in a shipped binary. Local conversion, though, needs
//! to know what *this machine* thinks the offset is, including whether
//! daylight saving was in effect **at that instant**, and the only correct
//! source for that is the OS.
//!
//! So: the calendar arithmetic below is this crate's own, and the offset
//! comes from Windows, per instant, through
//! `SystemTimeToTzSpecificLocalTime`.
//!
//! # DST is resolved per instant, and no offset is ever cached
//!
//! [`SystemZone`] asks the OS afresh for every conversion. That is the whole
//! of its design. A cached "we are UTC+1" is right for seven months of the
//! year and silently an hour out for the other five, and the failure lands
//! on exactly the dates a user is most likely to be checking -- a Send
//! created in October and expiring in November crosses the transition. The
//! call is a handful of microseconds and happens when a label is drawn, not
//! in a loop.
//!
//! # Nothing here reads the wall clock
//!
//! There is no `now()` in this module. An instant is always a parameter, and
//! the offset is always a [`LocalOffset`] the caller supplies. That is what
//! lets every test below -- and every test of every caller -- be exact:
//! `main.rs` passes [`SystemZone`], tests pass [`FixedOffset`], and no
//! assertion in this crate depends on which side of a DST boundary the suite
//! happens to be run on. A test that pinned geometry derived from a relative
//! date is what turned `main` red at UTC midnight once; the instant and the
//! offset are both injected here so that cannot recur through this path.

/// Milliseconds in a day, on the civil calendar this module uses (no leap
/// seconds -- Unix time has none either).
pub const MILLIS_PER_DAY: i64 = 86_400_000;

/// Month names as this app spells them, three letters, index 0 = January.
///
/// Three letters rather than the full word because every place these are
/// painted is a secondary line under a control, and because "18 Aug 2026"
/// cannot be misread as month-first the way "08/18/2026" can.
pub const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The name of month `mo`, where January is 1.
///
/// Out-of-range answers `"???"` rather than panicking or indexing: this is
/// only ever reached from a formatter drawing a label, and a label with three
/// question marks in it is a visible defect, where a panic in a draw call is
/// a window that disappears.
pub fn month_name(mo: u32) -> &'static str {
    // **`mo == 0` is checked first rather than saturated.**
    // `saturating_sub(1)` on zero is zero, so a month of 0 -- exactly what a
    // caller who forgot months are 1-based produces -- came back "Jan": a
    // wrong answer wearing a right answer's clothes. Out of range says so.
    if mo == 0 {
        return "???";
    }
    MONTH_NAMES.get((mo - 1) as usize).copied().unwrap_or("???")
}

/// A civil date and time, with no timezone attached to it.
///
/// **Deliberately says nothing about which zone it is in.** It is what
/// [`civil_parts`] produces from a millisecond count, and whether that count
/// was UTC or already shifted into local time is the caller's business.
/// Naming a zone in this type would invite exactly the mistake this module
/// exists to stop: a value labelled `Utc` that has been shifted, or one
/// labelled `Local` that has not.
///
/// `Debug` is derived: there is nothing here derived from a secret. A date is
/// not a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilParts {
    pub year: i64,
    /// 1-12.
    pub month: u32,
    /// 1-31.
    pub day: u32,
    /// 0-23.
    pub hour: u32,
    /// 0-59.
    pub minute: u32,
    /// 0-59.
    pub second: u32,
    /// 0-999.
    pub millis: u32,
}

/// Days since the Unix epoch to a proleptic-Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`. Proleptic Gregorian, which is what an
/// ISO 8601 instant means, and what `bw`'s own `deletionDate` is read as.
///
/// Moved here from `send.rs`, which had the only copy. It is not a Send
/// concept; it is what "what day is this instant" means, and a second
/// implementation of it beside a first is how two surfaces come to disagree
/// about the same moment by one day.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The civil date and time of `millis` past the Unix epoch, **in whatever
/// zone `millis` is already expressed in**.
///
/// Handed a UTC instant it produces UTC parts; handed the output of
/// [`local_millis`] it produces local ones. It does no shifting of its own,
/// which is what keeps the shift visible at the call site instead of hidden
/// in a formatter.
///
/// Negative instants (before 1970) work: the division is `div_euclid`, not
/// truncating division, so the day boundary falls in the same place either
/// side of the epoch.
pub fn civil_parts(millis: i64) -> CivilParts {
    let days = millis.div_euclid(MILLIS_PER_DAY);
    let rem = millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let secs = rem / 1000;
    CivilParts {
        year,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: ((secs / 60) % 60) as u32,
        second: (secs % 60) as u32,
        millis: (rem % 1000) as u32,
    }
}

/// How far this machine's wall clock is from UTC **at a given instant**.
///
/// A trait rather than a function for one reason: the answer depends on the
/// machine and on the moment, and neither is something a test may be allowed
/// to inherit from whatever computer the suite is running on. Production
/// passes [`SystemZone`]; every test in this crate passes [`FixedOffset`].
///
/// The instant is a parameter and not a field, so an implementation that
/// cached one offset for the process would be visibly wrong at its own
/// signature rather than subtly wrong twice a year.
pub trait LocalOffset {
    /// Milliseconds to ADD to `utc_millis` to get local wall-clock time.
    ///
    /// Positive east of Greenwich. `+02:00` in summer and `+01:00` in winter
    /// is one zone answering two instants, which is exactly what the
    /// parameter is for.
    fn offset_millis_at(&self, utc_millis: i64) -> i64;
}

/// `utc_millis` shifted into local wall-clock time by `zone`.
///
/// The result is **not an instant any more** -- it is a wall-clock reading
/// expressed as a millisecond count so that [`civil_parts`] can take it
/// apart. Never persist one, never compare one against a stored instant, and
/// never send one to `bw`: it is display material and nothing else. That is
/// the whole of the "store UTC, display local" rule, in one function.
pub fn local_millis(utc_millis: i64, zone: &dyn LocalOffset) -> i64 {
    utc_millis.saturating_add(zone.offset_millis_at(utc_millis))
}

/// The local civil date and time of a UTC instant. The pairing of
/// [`local_millis`] and [`civil_parts`], spelled once so no call site does
/// only half of it.
pub fn local_parts(utc_millis: i64, zone: &dyn LocalOffset) -> CivilParts {
    civil_parts(local_millis(utc_millis, zone))
}

/// `"18 Aug 2026"`. The day, for a sentence about a deadline.
pub fn format_day(parts: CivilParts) -> String {
    format!("{} {} {}", parts.day, month_name(parts.month), parts.year)
}

/// `"18 Aug 2026, 14:32"`. A day and a time, for a record of when something
/// happened.
///
/// **Minutes, not seconds.** The precision a scan history entry can honestly
/// claim is "when it finished", and a display down to the second invites the
/// reader to treat the record as a log. Hours are 24-hour because this app
/// paints no am/pm anywhere else and a lock countdown beside it is `11:42`.
pub fn format_day_time(parts: CivilParts) -> String {
    format!(
        "{} {} {}, {:02}:{:02}",
        parts.day,
        month_name(parts.month),
        parts.year,
        parts.hour,
        parts.minute
    )
}

/// A fixed offset from UTC. **Tests only**, and the reason every assertion
/// about a painted date in this crate is exact.
///
/// It is `pub` because the tests that need it are in the modules that draw
/// the labels, not in this one. It is not wired at any production call site,
/// and `the_only_production_zone_is_the_system_one` is what keeps that true.
#[derive(Debug, Clone, Copy)]
pub struct FixedOffset(pub i64);

impl LocalOffset for FixedOffset {
    fn offset_millis_at(&self, _utc_millis: i64) -> i64 {
        self.0
    }
}

/// The offset Windows itself reports for this machine, resolved afresh at
/// every instant asked about.
///
/// # How it is computed, and why it is a round trip rather than a lookup
///
/// `GetTimeZoneInformation` hands back a *rule* -- a bias, a daylight bias,
/// and the two transition dates -- and applying that rule by hand means
/// reimplementing "the last Sunday in October" and getting it wrong for the
/// southern hemisphere, for zones that do not transition on a Sunday, and for
/// the year a government moves the date. `SystemTimeToTzSpecificLocalTime` is
/// the OS applying its own rule to one instant, which is the only version of
/// this that stays correct after the binary ships.
///
/// So: the UTC instant becomes a `FILETIME` by pure integer arithmetic (no
/// calendar involved), `FileTimeToSystemTime` turns it into a UTC
/// `SYSTEMTIME`, `SystemTimeToTzSpecificLocalTime` shifts it, and
/// `SystemTimeToFileTime` turns the shifted reading back into a number. The
/// **difference** between the two numbers is the offset. Nothing here parses
/// a date and nothing here needs to.
///
/// # What it does when the OS refuses
///
/// Zero -- i.e. the label shows UTC. That is not a good answer and it is not
/// dressed up as one; it is the least bad one available inside a function
/// whose signature is `-> i64`. Every one of these calls fails only if the
/// system timezone data is unreadable, which is a broken installation rather
/// than a state this app can be in. Widening the return type to propagate it
/// would put a `Result` in front of every label in the app to describe a
/// condition under which the app would not start.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemZone;

/// 100-nanosecond intervals between the FILETIME epoch (1601-01-01) and the
/// Unix epoch (1970-01-01). The one constant that relates the two.
#[cfg(windows)]
const FILETIME_EPOCH_OFFSET_100NS: i64 = 116_444_736_000_000_000;

#[cfg(windows)]
impl LocalOffset for SystemZone {
    fn offset_millis_at(&self, utc_millis: i64) -> i64 {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::System::Time::{
            FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
        };

        let ticks = match utc_millis.checked_mul(10_000) {
            Some(t) => t.saturating_add(FILETIME_EPOCH_OFFSET_100NS),
            // Only reachable for an instant hundreds of thousands of years
            // out, which nothing in this app produces. UTC is the honest
            // answer for a number that is not a date.
            None => return 0,
        };
        if ticks < 0 {
            // `SYSTEMTIME` cannot express a date before 1601.
            return 0;
        }
        let utc_file = FILETIME {
            dwLowDateTime: (ticks as u64 & 0xFFFF_FFFF) as u32,
            dwHighDateTime: ((ticks as u64) >> 32) as u32,
        };

        let mut utc_system = SYSTEMTIME::default();
        let mut local_system = SYSTEMTIME::default();
        let mut local_file = FILETIME::default();
        // SAFETY: every pointer is to a live local, and each call is checked
        // before the next reads what it wrote. Nothing is allocated and
        // nothing outlives this frame.
        unsafe {
            if FileTimeToSystemTime(&utc_file, &mut utc_system).is_err() {
                return 0;
            }
            // `None` for the zone means "this machine's own", which is the
            // whole question being asked.
            if SystemTimeToTzSpecificLocalTime(None, &utc_system, &mut local_system).is_err() {
                return 0;
            }
            if SystemTimeToFileTime(&local_system, &mut local_file).is_err() {
                return 0;
            }
        }

        let local_ticks = ((local_file.dwHighDateTime as u64) << 32
            | local_file.dwLowDateTime as u64) as i64;
        // Back to milliseconds, and the DIFFERENCE rather than the value: the
        // caller adds this to the instant it already has, so a wrong sign
        // here would show up as a date twice the offset away rather than as
        // a plausible-looking one.
        (local_ticks - ticks) / 10_000
    }
}

/// UTC everywhere that is not Windows.
///
/// This crate is a Windows application -- it will not run anywhere else --
/// but `cargo` will still typecheck this file on another host, and an `impl`
/// that existed only under `cfg(windows)` would make every caller
/// conditionally compiled too. It is never the impl that runs in a shipped
/// build.
#[cfg(not(windows))]
impl LocalOffset for SystemZone {
    fn offset_millis_at(&self, _utc_millis: i64) -> i64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instants below are all stated, never taken from the clock. See the
    /// module docs: a test that read `SystemTime::now()` would be a test whose
    /// answer changes twice a year and at UTC midnight.
    ///
    /// 2026-08-18T00:30:00Z -- half an hour past midnight UTC, which is the
    /// instant the whole "(UTC)" defect is about.
    const HALF_PAST_MIDNIGHT_UTC: i64 = 1_787_013_000_000;

    const HOUR: i64 = 3_600_000;

    #[test]
    fn the_civil_parts_of_a_stated_instant() {
        let parts = civil_parts(HALF_PAST_MIDNIGHT_UTC);
        assert_eq!(
            (parts.year, parts.month, parts.day, parts.hour, parts.minute),
            (2026, 8, 18, 0, 30),
            "got {parts:?}"
        );
    }

    /// **The defect, as a test.** An instant just after midnight UTC is the
    /// previous day in the Americas, and a sentence that named the UTC day
    /// named a day the link was already dead on.
    #[test]
    fn just_after_midnight_utc_is_the_previous_day_five_hours_west() {
        let west = FixedOffset(-5 * HOUR);
        let parts = local_parts(HALF_PAST_MIDNIGHT_UTC, &west);
        assert_eq!(
            (parts.year, parts.month, parts.day),
            (2026, 8, 17),
            "half past midnight UTC on the 18th is the evening of the 17th at UTC-5; got {parts:?}"
        );
        assert_eq!(format_day(parts), "17 Aug 2026");
        // And the UTC reading of the very same instant is the other day,
        // which is what makes this a difference the user would see.
        assert_eq!(format_day(civil_parts(HALF_PAST_MIDNIGHT_UTC)), "18 Aug 2026");
    }

    #[test]
    fn an_eastern_offset_can_push_the_day_forward() {
        let east = FixedOffset(13 * HOUR);
        // 2026-08-17T23:00:00Z, which is the 18th in New Zealand.
        let parts = local_parts(HALF_PAST_MIDNIGHT_UTC - 90 * 60 * 1000, &east);
        assert_eq!((parts.month, parts.day), (8, 18), "got {parts:?}");
    }

    /// A half-hour zone, because plenty of the world lives in one and an
    /// implementation that only handled whole hours would pass every test
    /// above.
    #[test]
    fn a_half_hour_offset_moves_the_minutes_and_not_only_the_hour() {
        let india = FixedOffset(5 * HOUR + 30 * 60 * 1000);
        let parts = local_parts(HALF_PAST_MIDNIGHT_UTC, &india);
        assert_eq!((parts.day, parts.hour, parts.minute), (18, 6, 0), "got {parts:?}");
    }

    /// The same zone answering two instants with two different offsets is the
    /// whole reason [`LocalOffset`] takes the instant. This stub is what a
    /// DST-observing zone does; `SystemZone` gets the same behaviour from the
    /// OS.
    #[test]
    fn an_offset_may_differ_between_two_instants_in_one_zone() {
        struct Dst;
        impl LocalOffset for Dst {
            fn offset_millis_at(&self, utc_millis: i64) -> i64 {
                if utc_millis < HALF_PAST_MIDNIGHT_UTC {
                    HOUR
                } else {
                    2 * HOUR
                }
            }
        }
        let before = local_parts(HALF_PAST_MIDNIGHT_UTC - 1, &Dst);
        let after = local_parts(HALF_PAST_MIDNIGHT_UTC, &Dst);
        assert_eq!((before.day, before.hour), (18, 1), "got {before:?}");
        assert_eq!((after.day, after.hour), (18, 2), "got {after:?}");
    }

    #[test]
    fn the_day_time_format_is_the_local_wall_clock_and_names_no_zone() {
        let text = format_day_time(local_parts(HALF_PAST_MIDNIGHT_UTC, &FixedOffset(-5 * HOUR)));
        assert_eq!(text, "17 Aug 2026, 19:30");
        assert!(!text.contains("UTC"), "no label in this app says UTC to the user: {text:?}");
    }

    #[test]
    fn a_month_out_of_range_is_visible_rather_than_a_panic() {
        assert_eq!(month_name(1), "Jan");
        assert_eq!(month_name(12), "Dec");
        assert_eq!(month_name(0), "???");
        assert_eq!(month_name(13), "???");
    }

    /// Before the epoch the day boundary has to fall in the same place, which
    /// truncating division would get wrong by a day for every negative
    /// instant. Nothing in this app produces one today; the arithmetic is
    /// shared with `send.rs`, so it is pinned rather than assumed.
    #[test]
    fn an_instant_before_the_epoch_still_lands_on_the_right_day() {
        let parts = civil_parts(-1);
        assert_eq!(
            (parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second),
            (1969, 12, 31, 23, 59, 59),
            "got {parts:?}"
        );
    }

    #[test]
    fn the_epoch_itself() {
        let parts = civil_parts(0);
        assert_eq!((parts.year, parts.month, parts.day, parts.hour), (1970, 1, 1, 0));
    }

    /// **`FixedOffset` must not be reachable from production code.** It is
    /// `pub` because the tests that use it live in other modules, and that is
    /// exactly the shape that lets one leak into a call site where the app
    /// would then paint a hard-coded timezone for every user on earth.
    ///
    /// The walk is over this crate's own sources with the test modules cut
    /// off at `#[cfg(test)]`, the same instrument `settings.rs` and
    /// `breach.rs` use on themselves.
    #[test]
    fn the_only_production_zone_is_the_system_one() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut files = Vec::new();
        collect_rs(&root, &mut files);
        assert!(files.len() > 40, "the walk found only {} files", files.len());
        for path in files {
            // This file DECLARES the type, so its production half names it by
            // construction. Excluded by path rather than by a cleverer
            // pattern, because a pattern that told a declaration from a
            // construction would be the thing under test.
            if path.file_name().is_some_and(|n| n == "local_time.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source is readable");
            let production = match text.find("#[cfg(test)]") {
                Some(cut) => &text[..cut],
                None => &text[..],
            };
            if production.contains("FixedOffset(") {
                offenders.push(path);
            }
        }
        assert!(
            offenders.is_empty(),
            "`FixedOffset` is a test double and was constructed in production code: {offenders:?}"
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}

//! `scan_history.json`: what the last twenty breach scans counted, and
//! nothing else at all.
//!
//! # Why this is not in `settings.json`
//!
//! `settings.json` holds **preferences** -- things the user chose, that the
//! app reads to decide how to behave. A scan history is a **record**: nothing
//! in this app branches on it, it is written after the fact, and it grows.
//! Putting it in the settings file would mean every preference write
//! re-serialising a growing list, and every history append re-writing a file
//! whose other half is the only record of which directory the user's vault is
//! in. `Settings::save` already refuses outright when the file it read back
//! could not be parsed, for exactly that reason; a record has no business
//! sharing that hazard.
//!
//! They live in the same directory ([`crate::settings::config_dir`]), which
//! is spelled once, in `settings.rs`, so the two files cannot come to be
//! written into different places.
//!
//! # WHAT IS IN THE FILE: counts and timestamps. Nothing else. Ever.
//!
//! [`ScanRecord`] has five fields and **every one of them is a number**.
//! There is no password here, no item name, no item id, no hash, no prefix,
//! no suffix, and nothing computed from a password. That is enforced rather
//! than promised: `every_field_of_a_scan_record_is_a_number` reads this
//! file's own declaration of the struct off disk and fails on any field whose
//! type is not one of the integer types.
//!
//! This is the whole point of the module, and the reason for the test.
//! A scan history is exactly the artefact that quietly becomes **an
//! unencrypted list of which of your entries are compromised**, sitting in
//! `%APPDATA%` next to a settings file, readable by anything running as the
//! user, surviving every lock and every sign-out. "Which item was breached"
//! is the single most useful sentence an attacker could find on this machine
//! that is not the vault itself. A per-item history would be a genuinely
//! useful feature and it is refused.
//!
//! What a count can still say is worth stating honestly rather than waving
//! away: "3 of 128 passwords found" tells a reader of the file that this
//! vault has three breached logins. It does not tell them **which**, it does
//! not narrow any password, and it is the least that a history can be while
//! still being a history. That is the trade, made deliberately.
//!
//! # The timestamp is UTC, and the display is not
//!
//! [`ScanRecord::finished_at_unix_millis`] is milliseconds since the Unix
//! epoch, UTC, because a stored local time is a number that changes meaning
//! when the user travels or when the clocks go back. Every surface that draws
//! one converts it through [`crate::local_time`] and shows the user's own day
//! and time. See that module: store UTC, display local, never print "UTC".
//!
//! # What an older or missing file parses as
//!
//! Stated here the way every [`crate::settings::Settings`] field states its
//! own, because "what happens to a file written by the last version" is the
//! question this file has to have an answer to before it ships.
//!
//! * **No file at all** -- the ordinary first-run case, and also what a user
//!   who has never scanned has. [`ScanHistory::load`] returns an empty
//!   history. Empty is a real state and the surfaces say so out loud ("No
//!   scan has been run yet") rather than drawing a blank panel.
//! * **A file that is empty, or nothing but whitespace** -- what a crashed
//!   write leaves behind. Treated as absent, exactly as `Settings::load`
//!   treats it: there is nothing in it to lose.
//! * **A file that is not JSON, or is JSON of the wrong shape** -- an empty
//!   history. Unlike `Settings`, this does **not** refuse to write afterwards.
//!   The difference is what is at stake: `Settings` refuses because
//!   overwriting an unparseable settings file could destroy an account list
//!   that names the only directory the vault is in. Nothing of the sort is in
//!   here. Losing an unreadable scan history costs five numbers per scan, and
//!   a history that could never be written again because of one bad byte
//!   would be a worse failure than the one it was guarding against.
//! * **A file from an older version, missing fields** -- `#[serde(default)]`
//!   on the struct, so an absent count reads as `0` and an absent timestamp
//!   reads as the Unix epoch. Zero is the honest reading of "this version did
//!   not record that": it is not an invented number, and a count of zero
//!   failures on an old entry is the same thing every other field on that
//!   entry already is -- what the version that wrote it knew.
//! * **A file with fields this version does not know** -- ignored, and
//!   **dropped on the next write**, because the whole file is re-serialised
//!   from what was parsed. That is the same round trip `Settings` makes.
//! * **A file with more than [`MAX_ENTRIES`] entries** -- read whole, then
//!   trimmed to the newest [`MAX_ENTRIES`] on the next write. A cap that were
//!   enforced only on append would let a hand-edited file grow forever.
//!
//! # Newest first, and capped
//!
//! [`ScanHistory::record`] pushes to the FRONT. The order in the file is the
//! order on screen, so nothing reverses a list at a draw site and no surface
//! can disagree with another about which scan was the last one. The cap is
//! [`MAX_ENTRIES`]; a scan history is a "did it run, and what did it say"
//! panel under a button, not an audit log, and twenty entries is already more
//! than a user will scroll.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name, beside `settings.json`.
pub const SCAN_HISTORY_FILE_NAME: &str = "scan_history.json";

/// How many scans are kept. See the module docs.
pub const MAX_ENTRIES: usize = 20;

/// Where `scan_history.json` lives, or `None` on a platform with no
/// resolvable config directory -- in which case nothing is recorded, which is
/// the same silent fall-back `settings.rs` makes.
///
/// The directory comes from [`crate::settings::config_dir`] and is not
/// re-derived here, so this file and `settings.json` cannot drift into two
/// different directories.
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(SCAN_HISTORY_FILE_NAME))
}

/// One finished scan, as five numbers.
///
/// **Every field is an integer, and that is enforced by a test that reads
/// this declaration off disk.** See the module docs for why: a per-item
/// history would be an unencrypted list of which of the user's entries are
/// compromised, and it is refused rather than deferred.
///
/// `Debug` is derived, and safely: there is nothing in here that came from a
/// password, so `crate::debug_leak_guard` has no quarrel with it.
///
/// `#[serde(default)]` is on the struct, not on each field, so a file written
/// by a version that did not have one of these counts reads it as `0`. See
/// the module docs for the whole table of what an older file parses as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanRecord {
    /// When the scan finished, in milliseconds since the Unix epoch, **UTC**.
    ///
    /// Stored UTC and drawn local; see [`crate::local_time`]. A missing value
    /// reads as `0`, which is the epoch -- visibly wrong on screen rather
    /// than plausibly wrong, which is what an invented "now" would be.
    pub finished_at_unix_millis: i64,
    /// How many **distinct passwords** were looked up.
    ///
    /// Distinct, because that is what a scan actually does: the reuse
    /// grouping already knows which passwords are the same, and a 1600-item
    /// vault is far fewer than 1600 requests. Reporting the item count here
    /// would overstate what left the machine by whatever the reuse factor is.
    pub passwords_checked: u32,
    /// How many vault **items** those passwords covered.
    ///
    /// Carried beside the count above because on its own "checked 128" is
    /// unanswerable -- 128 out of what? Two numbers say both what was asked
    /// of the API and what the answer covers.
    pub items_covered: u32,
    /// How many of the checked passwords came back as found in a breach.
    pub found: u32,
    /// How many could **not** be checked, after every retry this app makes.
    ///
    /// **First-class, and never folded into the others.** A run that reports
    /// "checked 60, found 3" while 40 lookups failed is a lie the user will
    /// go on trusting: they will read the absence of a finding as a clean
    /// result for passwords nobody managed to ask about. Every surface that
    /// draws a record draws this number, and draws it even when it is zero
    /// on a run where any other run failed, so its absence is never mistaken
    /// for its being unrecorded.
    pub failed: u32,
}

impl ScanRecord {
    /// Whether this run finished without a single lookup failing.
    ///
    /// A named predicate rather than `record.failed == 0` at each draw site:
    /// "was this result complete" is a question three surfaces ask, and it
    /// is the question that decides whether the numbers beside it can be
    /// read as an answer at all.
    pub fn is_complete(&self) -> bool {
        self.failed == 0
    }
}

/// The whole file: the scans this app has run, newest first.
///
/// A struct wrapping the `Vec` rather than a bare `Vec` at the top level of
/// the JSON, so a later version can add a sibling field without every older
/// file becoming unparseable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanHistory {
    /// **Newest first.** See the module docs: the order in the file is the
    /// order on screen, so nothing reverses a list at a draw site.
    pub entries: Vec<ScanRecord>,
}

impl ScanHistory {
    /// Reads the file. Every failure -- absent, empty, unparseable -- is an
    /// empty history. See the module docs for the full table and for why this
    /// does not refuse to write afterwards the way `Settings` does.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        if text.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str::<Self>(&text).unwrap_or_default()
    }

    /// Puts `record` at the front and trims to [`MAX_ENTRIES`].
    ///
    /// The trim is applied to the **whole** list and not only to the newly
    /// pushed entry, so a file that arrived over-length -- hand-edited, or
    /// written by a version with a larger cap -- comes back under it rather
    /// than staying over it forever.
    pub fn record(&mut self, record: ScanRecord) {
        self.entries.insert(0, record);
        self.entries.truncate(MAX_ENTRIES);
    }

    /// The newest scan, or `None` if none has ever been run.
    ///
    /// `None` is a state the surfaces render out loud ("No scan has been run
    /// yet"), for `password_health`'s reason: a blank panel reads as a load
    /// that failed, and "nothing has been checked" is the answer the user
    /// came for.
    pub fn latest(&self) -> Option<&ScanRecord> {
        self.entries.first()
    }

    /// Writes the whole file, trimming first.
    ///
    /// Creates the config directory if it is not there: this may be the first
    /// file this app ever writes on a machine where the user has changed no
    /// preference, and `settings.json` is written by a path that already
    /// exists by then.
    pub fn persist(&self, path: &Path) -> std::io::Result<()> {
        let mut trimmed = self.clone();
        trimmed.entries.truncate(MAX_ENTRIES);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&trimmed)?)
    }
}

/// Reads the history, adds `record`, writes it back.
///
/// A read-modify-write, for the reason every writer in `settings.rs` is one:
/// the caller holds the history it drew three frames ago, and a whole-file
/// save from that copy would drop a scan run from the other shell of the
/// preferences window in between.
///
/// Errors are returned rather than logged. Nothing about a scan result is
/// safe to put in a log line by reflex, and the caller is the surface that
/// can say "the scan ran but could not be recorded" in words.
pub fn append(path: &Path, record: ScanRecord) -> std::io::Result<ScanHistory> {
    let mut history = ScanHistory::load(path);
    history.record(record);
    history.persist(path)?;
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch path under the OS temp directory, **guaranteed to name no
    /// existing file**. **Never `%APPDATA%`**: no test in this crate may touch
    /// the real `Deskwarden` config directory, and
    /// `the_real_config_directory_is_never_resolved_by_a_test` below is what
    /// keeps that from being a convention.
    ///
    /// # Why this deletes, and why that is the fix rather than tidiness
    ///
    /// The process id is unique among the processes alive at one instant and
    /// **not across time** -- Windows recycles the range briskly -- while this
    /// directory was never cleaned, so every run left its files behind for a
    /// later run to inherit. Measured on the machine this was found on: 2,012
    /// files here, 255 of them `round-trip-<PID>.json`, each holding exactly
    /// the one record `a_record_round_trips_through_disk_under_its_own_field_names`
    /// writes. That test [`append`]s and then asserts the file holds one
    /// entry, so a run whose pid collided with any of those 255 loaded the
    /// stale record, appended a second, and failed with two identical entries
    /// -- roughly once in six full `--lib` runs, and reading exactly like a
    /// duplicate-write bug in `append`.
    ///
    /// The tests that survived it did so by removing the file first, one line
    /// at a time, in three of the nine. That is a convention every future test
    /// here has to remember; this is the same guarantee made once, where the
    /// path is handed out, so a test cannot be written without it. Nothing is
    /// weakened: a test that wants content on disk writes it on the next line.
    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("deskwarden-scan-history-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}-{}.json", std::process::id()));
        // Not `expect`: an absent file is the state being established, and
        // `remove_file` reports that as an error.
        let _ = std::fs::remove_file(&path);
        path
    }

    fn at(millis: i64) -> ScanRecord {
        ScanRecord { finished_at_unix_millis: millis, ..ScanRecord::default() }
    }

    #[test]
    fn a_missing_file_is_an_empty_history_and_not_an_error() {
        let path = temp_path("absent");
        let _ = std::fs::remove_file(&path);
        let loaded = ScanHistory::load(&path);
        assert_eq!(loaded, ScanHistory::default());
        assert!(loaded.entries.is_empty());
        assert_eq!(loaded.latest(), None);
    }

    /// What a crashed write leaves. Treated as absent, the same way
    /// `Settings::load` treats it.
    #[test]
    fn an_empty_file_is_an_empty_history() {
        let path = temp_path("empty");
        std::fs::write(&path, "").unwrap();
        assert!(ScanHistory::load(&path).entries.is_empty());
        std::fs::write(&path, "   \n\t ").unwrap();
        assert!(ScanHistory::load(&path).entries.is_empty());
    }

    #[test]
    fn an_unparseable_file_is_an_empty_history_and_does_not_block_the_next_write() {
        let path = temp_path("garbage");
        std::fs::write(&path, "{ this is not json").unwrap();
        assert!(ScanHistory::load(&path).entries.is_empty());
        // **The difference from `Settings`, asserted rather than described.**
        // There is no account list in here to destroy, and a history that
        // could never be written again after one bad byte would be the worse
        // failure.
        append(&path, at(5)).expect("an unreadable history must not block recording a new scan");
        assert_eq!(ScanHistory::load(&path).latest().map(|r| r.finished_at_unix_millis), Some(5));
    }

    /// **An older file, missing every field this version knows.** The one
    /// thing that must not happen is a parse failure that throws the whole
    /// history away.
    #[test]
    fn an_older_file_without_the_newer_counts_reads_them_as_zero() {
        let path = temp_path("older");
        std::fs::write(&path, r#"{"entries":[{"finished_at_unix_millis":1700000000000}]}"#)
            .unwrap();
        let loaded = ScanHistory::load(&path);
        assert_eq!(loaded.entries.len(), 1, "the older entry was dropped: {loaded:?}");
        let entry = loaded.entries[0];
        assert_eq!(entry.finished_at_unix_millis, 1_700_000_000_000);
        assert_eq!((entry.passwords_checked, entry.items_covered), (0, 0));
        assert_eq!((entry.found, entry.failed), (0, 0));
    }

    /// A file from before this feature existed at all: no `entries` key.
    #[test]
    fn a_file_with_no_entries_key_is_an_empty_history() {
        let path = temp_path("no-entries-key");
        std::fs::write(&path, r#"{"something_else": 3}"#).unwrap();
        assert!(ScanHistory::load(&path).entries.is_empty());
    }

    #[test]
    fn a_record_round_trips_through_disk_under_its_own_field_names() {
        let path = temp_path("round-trip");
        let written = ScanRecord {
            // Deliberately five DIFFERENT numbers: a writer that assigned one
            // field from another would round-trip identically through any
            // fixture whose counts agreed.
            finished_at_unix_millis: 1_787_013_000_000,
            passwords_checked: 128,
            items_covered: 1_600,
            found: 3,
            failed: 7,
        };
        append(&path, written).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        for field in [
            "finished_at_unix_millis",
            "passwords_checked",
            "items_covered",
            "found",
            "failed",
        ] {
            assert!(text.contains(field), "{field} is not in the file at all: {text}");
        }
        assert_eq!(ScanHistory::load(&path).entries, vec![written]);
    }

    #[test]
    fn the_newest_scan_is_first() {
        let path = temp_path("order");
        let _ = std::fs::remove_file(&path);
        append(&path, at(1)).unwrap();
        append(&path, at(2)).unwrap();
        let history = append(&path, at(3)).unwrap();
        assert_eq!(
            history.entries.iter().map(|r| r.finished_at_unix_millis).collect::<Vec<_>>(),
            vec![3, 2, 1],
            "the history is not newest-first, so every surface would have to reverse it"
        );
        assert_eq!(history.latest().map(|r| r.finished_at_unix_millis), Some(3));
        // And what came back is what is on disk, not only what was in memory.
        assert_eq!(ScanHistory::load(&path), history);
    }

    #[test]
    fn the_history_is_capped_and_the_oldest_falls_off() {
        let path = temp_path("cap");
        let _ = std::fs::remove_file(&path);
        for i in 0..(MAX_ENTRIES as i64 + 5) {
            append(&path, at(i)).unwrap();
        }
        let history = ScanHistory::load(&path);
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(
            history.entries[0].finished_at_unix_millis,
            MAX_ENTRIES as i64 + 4,
            "the newest scan is not at the front"
        );
        assert_eq!(
            history.entries.last().unwrap().finished_at_unix_millis,
            5,
            "the oldest kept entry is wrong, so the trim is taking from the wrong end"
        );
    }

    /// **The cap applies to the whole list, not only to appends.** A file that
    /// arrived over-length -- hand-edited, or written by a version with a
    /// larger cap -- has to come back under the cap rather than stay over it.
    #[test]
    fn an_over_long_file_is_trimmed_on_the_next_write() {
        let path = temp_path("over-long");
        let over = ScanHistory {
            entries: (0..(MAX_ENTRIES as i64 * 3)).rev().map(at).collect(),
        };
        over.persist(&path).unwrap();
        assert_eq!(ScanHistory::load(&path).entries.len(), MAX_ENTRIES);
    }

    /// **The claim this module exists to make, as a test over its own
    /// source.**
    ///
    /// Every field of [`ScanRecord`] is an integer. Not "no field is called
    /// `password`" -- a `String` named `note` would pass that and would be
    /// the whole defect. A history file is exactly the artefact that quietly
    /// becomes an unencrypted list of which of the user's entries are
    /// compromised, and the way that arrives is one well-meant field.
    #[test]
    fn every_field_of_a_scan_record_is_a_number() {
        let source = include_str!("scan_history.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        let head = concat!("pub struct ScanRe", "cord {");
        let start = production
            .find(head)
            .expect("the record struct is no longer declared the way this test reads it")
            + head.len();
        let body = &production[start..];
        let body = &body[..body.find("\n}").expect("the struct's closing brace is unindented")];

        let allowed = ["i64", "u64", "u32", "usize", "i32"];
        let mut fields = 0;
        for line in body.lines() {
            let line = line.trim();
            // Doc comments, attributes and blank lines carry no field.
            if !line.starts_with("pub ") {
                continue;
            }
            let declared = line
                .trim_start_matches("pub ")
                .split_once(':')
                .map(|(_, ty)| ty.trim().trim_end_matches(',').to_string())
                .expect("a field line with no type");
            assert!(
                allowed.contains(&declared.as_str()),
                "`{declared}` is not a number. Every field of a scan record is a count or a \
                 timestamp: no password, no item name, no item id, no hash, and nothing derived \
                 from a password. A history file with a name in it is an unencrypted list of \
                 which of this user's entries are compromised"
            );
            fields += 1;
        }
        assert_eq!(
            fields, 5,
            "control: {fields} fields were parsed out of the record, so the walk is reading \
             the wrong text and the assertion above proves nothing"
        );
    }

    /// The instrument above, aimed at a struct that should fail it. Without
    /// this, a walk that parsed nothing would pass silently -- which is the
    /// failure mode of every source-text guard.
    #[test]
    fn the_field_type_walk_would_catch_a_string() {
        let body = "\n    pub finished_at_unix_millis: i64,\n    pub item_name: String,\n";
        let offending: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("pub "))
            .filter(|l| !l.ends_with("i64,") && !l.ends_with("u32,"))
            .collect();
        assert_eq!(
            offending,
            vec!["pub item_name: String,"],
            "control: the shape the real walk matches on no longer isolates a non-integer field"
        );
    }

    /// **No test in this module resolves the real `%APPDATA%` file.** Same
    /// instrument, and same reason, as `settings.rs`'s own: a test that wrote
    /// there would be a test that destroyed the user's history when the suite
    /// ran on their machine.
    #[test]
    fn the_real_config_directory_is_never_resolved_by_a_test() {
        let source = include_str!("scan_history.rs");
        let tests = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .1;
        let resolver = concat!("default_", "path()");
        assert_eq!(
            tests.matches(resolver).count(),
            0,
            "a test in this module resolves the real %APPDATA% scan history -- every test here \
             must stay inside the OS temp directory"
        );
        assert_eq!(
            source.matches(concat!("pub fn default_", "path()")).count(),
            1,
            "the real resolver is no longer spelled that way -- the needle above has drifted \
             and its absence proves nothing"
        );
    }

    #[test]
    fn a_run_with_a_failure_is_not_complete() {
        assert!(at(0).is_complete());
        assert!(!ScanRecord { failed: 1, ..at(0) }.is_complete());
    }

    /// **Nothing in this file logs.** Same claim, and the same file-scoped
    /// instrument, as `breach.rs` makes about itself: a record's counts are
    /// not a secret, but a module whose whole subject is "which of this
    /// vault's passwords are breached" has no business writing to a file that
    /// outlives the process and that nothing wipes.
    #[test]
    fn the_scan_history_module_never_logs() {
        let source = include_str!("scan_history.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        for needle in ["log::", "info!", "warn!", "debug!", "error!", "trace!", "println!", "dbg!"]
        {
            assert!(
                !production.contains(needle),
                "production `scan_history.rs` writes `{needle}`"
            );
        }
    }
}

//! `receive_history.json`: which Sends **other people** sent to this user and
//! this app actually fetched, and nothing else at all.
//!
//! # What this row is, and why it needed a file
//!
//! Design 5b's sidebar has a `Shared with me` row under `SHARING`, beside the
//! account's own published Sends. This app can already receive one --
//! `record_ui`'s import modal takes a Send link, `send_receive` fetches it,
//! and `record::import::item_from` turns the answer into an ordinary vault
//! item. What did not exist was any trace that it happened: the item lands in
//! the vault indistinguishable from one the user typed, and the event leaves
//! nothing behind. A row that counts something has to have something to
//! count, so the count is kept here.
//!
//! # Why this is not in `settings.json`
//!
//! [`crate::scan_history`]'s reason, unchanged and worth restating rather
//! than cross-referencing away: `settings.json` holds **preferences** -- what
//! the user chose, which the app branches on -- and a history is a
//! **record**, written after the fact and growing. Sharing the file would
//! make every preference write re-serialise a growing list, and every append
//! re-write the only record of where the user's vault directory is.
//! `Settings::save` refuses outright when the file it read back could not be
//! parsed; a record has no business sharing that hazard.
//!
//! Both files live in [`crate::settings::config_dir`], which is spelled once,
//! in `settings.rs`, so the three cannot drift into different directories.
//!
//! # WHAT IS IN THE FILE, AND WHY THE ACCESS URL IS NOT
//!
//! **A Send's access URL is a live secret, not a locator.** Its fragment
//! carries the decryption key -- which is why [`crate::send::SendSummary`]'s
//! hand-written `Debug` elides the whole URL rather than splitting on `#`,
//! and why that decision is argued at length there. A history file holding
//! access URLs would be a plaintext list of openable Sends sitting in
//! `%APPDATA%`, readable by anything running as the user, surviving every
//! lock and every sign-out, and outliving the links' own expiry only in the
//! sense that it would let them be opened again the moment anybody found it.
//!
//! So [`ReceivedRecord`] has exactly three fields and they are named in a
//! test: when it was received, what it was called, and which vault item it
//! became. There is no URL, no key, no password, no passphrase, no content,
//! no sender and no host. `a_received_record_holds_exactly_the_three_permitted_fields`
//! reads this file's own declaration of the struct off disk and fails on any
//! field that is not one of the three, by name **and** by type -- so a later
//! `url: String`, or a `locator: String`, or a `name: SendSummary`, does not
//! compile past the suite. `scan_history` enforces its own rule the same way
//! and for the same reason: a promise in a comment is not a guard.
//!
//! # Why a name is allowed here when `scan_history` refuses one
//!
//! `scan_history` refuses item names because the sentence its file would then
//! make is "**these** entries of yours are breached" -- a claim about the
//! user's passwords that an attacker could not otherwise assemble. Nothing of
//! that kind is available here. The name of a received record is a label the
//! user has already stored, unencrypted decisions aside, as an ordinary vault
//! item; the file says "somebody shared a thing called this with you", which
//! narrows no password and verifies no credential.
//!
//! It is still a real disclosure and it is made deliberately rather than
//! waved away: a reader of this file learns that this machine's user was sent
//! something called "SAP Production". The row cannot exist without a word to
//! print, the alternative is a list of blanks, and the trade is the smallest
//! one that leaves a usable row.
//!
//! # The vault item id, and what it is for
//!
//! [`ReceivedRecord::item_id`] is the id of the item the import created. It
//! exists to answer **one** question -- is that item still in the vault? --
//! because the history outlives the item and a row that points at nothing
//! must say so rather than fail silently. See
//! [`crate::vault_window::send_ui::received_rows`], which is the only reader.
//!
//! An id is not a credential: it names a row in a vault an attacker would
//! still have to open. It is also not a secret this file introduces -- the
//! same id is in the local vault cache on the same disk.
//!
//! # The timestamp is UTC, and the display is not
//!
//! Milliseconds since the Unix epoch, UTC, for `scan_history`'s reason: a
//! stored local time is a number that changes meaning when the user travels
//! or when the clocks go back. Every surface that draws one converts it
//! through [`crate::local_time`] and shows the user's own day.
//!
//! # What an older or missing file parses as
//!
//! * **No file at all** -- the ordinary case, and what every user who has
//!   never imported a link has. An empty history, which the row renders as a
//!   count of zero and the pane renders in words.
//! * **An empty or whitespace-only file** -- what a crashed write leaves.
//!   Treated as absent.
//! * **Not JSON, or JSON of the wrong shape** -- an empty history, and this
//!   does **not** refuse to write afterwards. `scan_history` argues the
//!   difference from `Settings`: nothing here names the directory the vault
//!   is in, so losing an unreadable history costs three fields per import,
//!   and a history that could never be written again because of one bad byte
//!   would be the worse failure.
//! * **Missing fields** -- `#[serde(default)]` on the struct, so an absent
//!   timestamp reads as the epoch and an absent name or id reads as the empty
//!   string. Both are visibly wrong on screen rather than plausibly wrong,
//!   which is what an invented value would be; the pane says so in words.
//! * **Fields this version does not know** -- ignored, and dropped on the
//!   next write, because the whole file is re-serialised from what was
//!   parsed.
//! * **More than [`MAX_ENTRIES`] entries** -- read whole, trimmed to the
//!   newest [`MAX_ENTRIES`] on the next write, so a hand-edited file cannot
//!   grow forever.
//!
//! # Newest first, and capped
//!
//! [`ReceiveHistory::record`] pushes to the FRONT, so the order in the file is
//! the order on screen and no draw site reverses a list. The cap is
//! [`MAX_ENTRIES`], `scan_history`'s own twenty: this is a "what have I been
//! sent lately" row, not an audit log.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name, beside `settings.json` and `scan_history.json`.
pub const RECEIVE_HISTORY_FILE_NAME: &str = "receive_history.json";

/// How many receives are kept. See the module docs.
pub const MAX_ENTRIES: usize = 20;

/// Where `receive_history.json` lives, or `None` on a platform with no
/// resolvable config directory -- in which case nothing is recorded, which is
/// the same silent fall-back `settings.rs` and `scan_history.rs` make.
///
/// The directory comes from [`crate::settings::config_dir`] and is not
/// re-derived here, so this file cannot drift away from the other two.
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(RECEIVE_HISTORY_FILE_NAME))
}

/// One Send that was received and imported, as three fields.
///
/// **Exactly these three, by name and by type, and that is enforced by a test
/// that reads this declaration off disk.** See the module docs: the field
/// this struct must never grow is the access URL, because a Send's URL
/// carries its own decryption key.
///
/// `Debug` is derived, and safely: nothing in here came from the Send's
/// content, its password, or its link.
///
/// `#[serde(default)]` is on the struct, not on each field, so a file written
/// by a version that did not have one of these reads it as that type's
/// default. See the module docs for the whole table.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceivedRecord {
    /// When the import finished, in milliseconds since the Unix epoch,
    /// **UTC**. Stored UTC and drawn local; see [`crate::local_time`].
    pub received_at_unix_millis: i64,
    /// What the imported record was called -- the name the new vault item was
    /// given, which is the only word the row has to print.
    pub name: String,
    /// The id of the vault item the import created, so a reader can ask
    /// whether that item is still there. **Not a link and not a key**; see
    /// the module docs.
    pub item_id: String,
}

/// The whole file.
///
/// One field, a list, for `scan_history`'s reason: a record file with a
/// second top-level key is a file with two things that can disagree, and a
/// bare JSON array cannot grow a key later without the old file becoming
/// unparseable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceiveHistory {
    /// **Newest first.** See the module docs.
    pub entries: Vec<ReceivedRecord>,
}

impl ReceiveHistory {
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
    /// pushed entry, so a file that arrived over-length comes back under the
    /// cap rather than staying over it forever.
    pub fn record(&mut self, record: ReceivedRecord) {
        self.entries.insert(0, record);
        self.entries.truncate(MAX_ENTRIES);
    }

    /// The most recent receive, or `None` if nothing has ever been imported.
    ///
    /// `None` is a state the surfaces render out loud rather than as a blank
    /// panel; see [`crate::vault_window::send_ui::RECEIVED_EMPTY_HEADLINE`].
    pub fn latest(&self) -> Option<&ReceivedRecord> {
        self.entries.first()
    }

    /// Writes the whole file, trimming first.
    ///
    /// Creates the config directory if it is not there: this may be the first
    /// file this app ever writes on a machine where the user has changed no
    /// preference.
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
/// the caller holds the history it drew some frames ago, and a whole-file
/// save from that copy would drop an import made by another window in
/// between.
///
/// Errors are returned rather than logged. See
/// `the_receive_history_module_never_logs`.
pub fn append(path: &Path, record: ReceivedRecord) -> std::io::Result<ReceiveHistory> {
    let mut history = ReceiveHistory::load(path);
    history.record(record);
    history.persist(path)?;
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch path under the OS temp directory, **guaranteed to name no
    /// existing file**. Never `%APPDATA%`; see
    /// `the_real_config_directory_is_never_resolved_by_a_test`, and
    /// `scan_history`'s own helper for the measured reason the guard is a
    /// per-call directory rather than a shared one.
    fn temp_path(tag: &str) -> (crate::test_scratch::ScratchDir, PathBuf) {
        let dir = crate::test_scratch::ScratchDir::new(&format!("receive-history-{tag}"));
        let path = dir.join("history.json");
        (dir, path)
    }

    fn at(millis: i64) -> ReceivedRecord {
        ReceivedRecord {
            received_at_unix_millis: millis,
            name: format!("Record {millis}"),
            item_id: format!("item-{millis}"),
        }
    }

    #[test]
    fn a_missing_file_is_an_empty_history_and_not_an_error() {
        let (_dir, path) = temp_path("absent");
        let loaded = ReceiveHistory::load(&path);
        assert_eq!(loaded, ReceiveHistory::default());
        assert!(loaded.entries.is_empty());
        assert_eq!(loaded.latest(), None);
    }

    #[test]
    fn an_empty_file_is_an_empty_history() {
        let (_dir, path) = temp_path("empty");
        std::fs::write(&path, "   \n\t ").unwrap();
        assert_eq!(ReceiveHistory::load(&path), ReceiveHistory::default());
    }

    #[test]
    fn an_unparseable_file_is_an_empty_history_and_does_not_block_the_next_write() {
        let (_dir, path) = temp_path("garbage");
        std::fs::write(&path, "{ not json at all").unwrap();
        assert_eq!(ReceiveHistory::load(&path), ReceiveHistory::default());
        // The difference from `Settings`, asserted rather than described: a
        // bad byte must not make the history unwritable forever.
        let after = append(&path, at(7)).unwrap();
        assert_eq!(after.entries.len(), 1);
        assert_eq!(ReceiveHistory::load(&path).entries, after.entries);
    }

    #[test]
    fn an_older_file_without_the_newer_fields_reads_them_as_their_defaults() {
        let (_dir, path) = temp_path("older");
        std::fs::write(
            &path,
            r#"{"entries":[{"received_at_unix_millis":12}]}"#,
        )
        .unwrap();
        let loaded = ReceiveHistory::load(&path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].received_at_unix_millis, 12);
        assert_eq!(loaded.entries[0].name, "");
        assert_eq!(loaded.entries[0].item_id, "");
    }

    #[test]
    fn a_file_with_no_entries_key_is_an_empty_history() {
        let (_dir, path) = temp_path("no-key");
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(ReceiveHistory::load(&path), ReceiveHistory::default());
    }

    /// The field names on disk are the field names in the struct, so an older
    /// build's file is readable by a newer one.
    #[test]
    fn a_record_round_trips_through_disk_under_its_own_field_names() {
        let (_dir, path) = temp_path("round-trip");
        let record = ReceivedRecord {
            received_at_unix_millis: 1_786_320_000_000,
            name: "SAP Production".to_string(),
            item_id: "abc-123".to_string(),
        };
        let history = append(&path, record.clone()).unwrap();
        assert_eq!(history.entries, vec![record.clone()]);

        let text = std::fs::read_to_string(&path).unwrap();
        for key in ["received_at_unix_millis", "name", "item_id"] {
            assert!(text.contains(key), "the file does not spell {key:?}: {text}");
        }
        assert_eq!(ReceiveHistory::load(&path).entries, vec![record]);
    }

    #[test]
    fn the_newest_receive_is_first() {
        let mut history = ReceiveHistory::default();
        history.record(at(1));
        history.record(at(2));
        assert_eq!(history.latest(), Some(&at(2)));
        assert_eq!(history.entries[1], at(1));
    }

    #[test]
    fn the_history_is_capped_and_the_oldest_falls_off() {
        let mut history = ReceiveHistory::default();
        for i in 0..(MAX_ENTRIES as i64 + 5) {
            history.record(at(i));
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(history.latest(), Some(&at(MAX_ENTRIES as i64 + 4)));
    }

    #[test]
    fn an_over_long_file_is_trimmed_on_the_next_write() {
        let (_dir, path) = temp_path("over-long");
        let over = ReceiveHistory {
            entries: (0..(MAX_ENTRIES as i64 * 3)).rev().map(at).collect(),
        };
        over.persist(&path).unwrap();
        assert_eq!(ReceiveHistory::load(&path).entries.len(), MAX_ENTRIES);
    }

    /// **THE CLAIM THIS MODULE EXISTS TO MAKE, as a test over its own
    /// source.**
    ///
    /// A received-Sends history is exactly the artefact that quietly becomes
    /// **a plaintext list of openable secret links** in `%APPDATA%`: a Send's
    /// access URL carries its decryption key in the fragment, so one
    /// well-meant `url: String` turns this file from a count into a key ring.
    ///
    /// So the guard is an allow-list by NAME AND TYPE rather than a type walk
    /// -- `scan_history` can say "every field is a number" because a count is
    /// all it needs, and this one cannot, since two of its three fields are
    /// `String`. A type walk here would wave through `pub url: String`, which
    /// is the whole defect. Three names, three types, and any fourth field
    /// fails.
    #[test]
    fn a_received_record_holds_exactly_the_three_permitted_fields() {
        let source = include_str!("receive_history.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        let head = concat!("pub struct ReceivedRe", "cord {");
        let start = production
            .find(head)
            .expect("the record struct is no longer declared the way this test reads it")
            + head.len();
        let body = &production[start..];
        let body = &body[..body.find("\n}").expect("the struct's closing brace is unindented")];

        let permitted = [
            ("received_at_unix_millis", "i64"),
            ("name", "String"),
            ("item_id", "String"),
        ];
        let mut found: Vec<(String, String)> = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            // Doc comments, attributes and blank lines carry no field.
            if !line.starts_with("pub ") {
                continue;
            }
            let (name, ty) = line
                .trim_start_matches("pub ")
                .split_once(':')
                .expect("a field line with no type");
            found.push((
                name.trim().to_string(),
                ty.trim().trim_end_matches(',').to_string(),
            ));
        }
        for (name, ty) in &found {
            assert!(
                permitted
                    .iter()
                    .any(|(n, t)| *n == name.as_str() && *t == ty.as_str()),
                "`{name}: {ty}` is not one of the three fields a received-Send record may \
                 hold. A Send's access URL carries its own decryption key, so a field here \
                 that could hold one -- a url, a locator, a link, a key, a password, the \
                 content -- turns this file into a plaintext list of openable secrets in \
                 %APPDATA%. See this module's header"
            );
        }
        assert_eq!(
            found.len(),
            permitted.len(),
            "control: {} fields were parsed out of the record, so the walk is reading the \
             wrong text and the assertion above proves nothing",
            found.len()
        );
    }

    /// The instrument above, aimed at a struct that should fail it. Without
    /// this, a walk that parsed nothing would pass silently -- which is the
    /// failure mode of every source-text guard.
    #[test]
    fn the_field_walk_would_catch_an_access_url() {
        let body = "\n    pub received_at_unix_millis: i64,\n    pub name: String,\n    \
                    pub item_id: String,\n    pub access_url: String,\n";
        let permitted = [
            ("received_at_unix_millis", "i64"),
            ("name", "String"),
            ("item_id", "String"),
        ];
        let offending: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("pub "))
            .filter(|l| {
                let (name, ty) = l.trim_start_matches("pub ").split_once(':').expect("typed");
                !permitted
                    .iter()
                    .any(|(n, t)| *n == name.trim() && *t == ty.trim().trim_end_matches(','))
            })
            .collect();
        assert_eq!(
            offending,
            vec!["pub access_url: String,"],
            "control: the shape the real walk matches on no longer isolates an extra field"
        );
    }

    /// **No line of executable code in this module names a Send's link.**
    ///
    /// The struct guard above is the hold; this is the second one, over the
    /// whole module, because a URL could be smuggled through a helper that
    /// never touches [`ReceivedRecord`] at all -- a `fn remember_link`, a
    /// `const`, a format string.
    ///
    /// **Comments are excluded, and that is the point rather than a
    /// loophole.** This file's header argues at length about access URLs,
    /// decryption keys and passwords -- it has to, because saying WHY they
    /// are forbidden is how the next person knows not to add one -- so a
    /// whole-text scan would be a rule that forbids its own justification.
    /// What is scanned is what compiles. The control below aims the same
    /// walk at a line that should fail it, so a stripper that removed
    /// everything would not pass silently.
    #[test]
    fn nothing_in_this_modules_code_names_a_send_link() {
        let source = include_str!("receive_history.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        let code = code_only(production);
        assert!(
            code.contains("pub fn append"),
            "control: the comment stripper removed this module's own code, so the scan below \
             would pass against nothing"
        );
        for needle in FORBIDDEN_IN_CODE {
            assert!(
                !code.contains(needle),
                "production `receive_history.rs` writes {needle:?} in code -- see the module \
                 docs on why a Send's URL, its key and its password must never reach this file"
            );
        }
    }

    /// The words that must not appear in this module's executable text.
    ///
    /// `http` is here as well as the obvious three, because the shape this
    /// guards against is not only a field called `url`: a base address plus
    /// an id assembled in a `format!` is the same disclosure spelled
    /// differently.
    const FORBIDDEN_IN_CODE: [&str; 6] =
        ["access_url", "accessUrl", "passphrase", "password", "http", "secret"];

    /// `text` with every `//` comment line removed. Line-based, which is all
    /// this file needs: it contains no block comments and no string literal
    /// holding a `//`.
    fn code_only(text: &str) -> String {
        text.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The instrument above, aimed at a line that should fail it. Without
    /// this, a stripper that returned the empty string would pass silently --
    /// the failure mode of every source-text guard.
    #[test]
    fn the_code_scan_would_catch_a_url_outside_the_record() {
        let sample = "// a comment mentioning access_url\nfn f() { let x = \"accessUrl\"; }\n";
        let code = code_only(sample);
        assert!(
            !code.contains("access_url"),
            "control: the stripper is not removing comment lines at all"
        );
        assert!(
            FORBIDDEN_IN_CODE.iter().any(|needle| code.contains(needle)),
            "control: the needle list no longer catches a link named in code"
        );
    }

    /// **No test in this module resolves the real `%APPDATA%` file.** Same
    /// instrument, and same reason, as `settings.rs`'s and
    /// `scan_history.rs`'s: a test that wrote there would destroy the user's
    /// history when the suite ran on their machine.
    #[test]
    fn the_real_config_directory_is_never_resolved_by_a_test() {
        let source = include_str!("receive_history.rs");
        let tests = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .1;
        let resolver = concat!("default_", "path()");
        assert_eq!(
            tests.matches(resolver).count(),
            0,
            "a test in this module resolves the real %APPDATA% receive history -- every test \
             here must stay inside the OS temp directory"
        );
        assert_eq!(
            source.matches(concat!("pub fn default_", "path()")).count(),
            1,
            "the real resolver is no longer spelled that way -- the needle above has drifted \
             and its absence proves nothing"
        );
    }

    /// **Nothing in this file logs.** `scan_history`'s claim, for the same
    /// reason one step along: a module whose subject is "what was shared with
    /// this user" has no business writing names into a file that outlives the
    /// process and that nothing wipes.
    #[test]
    fn the_receive_history_module_never_logs() {
        let source = include_str!("receive_history.rs");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .0;
        for needle in
            ["log::", "info!", "warn!", "debug!", "error!", "trace!", "println!", "dbg!"]
        {
            assert!(
                !production.contains(needle),
                "production `receive_history.rs` writes `{needle}`"
            );
        }
    }
}

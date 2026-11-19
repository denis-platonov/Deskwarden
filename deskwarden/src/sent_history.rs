//! `sent_history.json`: the Sends **this user** made from a record in this
//! app -- who each one went to, which record it came from, and which fields
//! travelled -- and nothing that opens one.
//!
//! # Why this file exists
//!
//! A Bitwarden Send stores none of this. It has a name, a link, dates and a
//! view count; it has no recipient, and it does not say which vault item it
//! was made from or what of it went. Design 5b's Sent screen shows all three
//! (`to m.reyes`, `Open record`, `WHAT WAS SHARED`), and the owner asked for
//! a fourth thing only a record of recipients can give: "if sent once - we
//! should save the email\recepient and suggest those from the list next
//! time, person can also click x in the list to delete a person but if
//! records shared - we ask like these are shared, deleting will remove all
//! shares yes\no". So it is kept here, on this PC, as `receive_history.json`
//! keeps the other direction.
//!
//! # WHAT IS IN THE FILE, AND WHAT IS NOT
//!
//! **Never the link.** A Send's access URL carries its decryption key in its
//! fragment -- see `receive_history`'s module docs, whose argument is the same
//! and is not repeated. An entry is keyed on the Send's server **id**, which
//! names it to the server (so it can be deleted) and opens nothing.
//!
//! What IS here, and is a real disclosure made deliberately: the recipient's
//! email address, the record's name, and the names of the fields that went
//! (`Username`, `Password`, ...) -- never their values. A reader of this file
//! learns that this user sent something called "SAP Production" to
//! m.reyes@ledgerline.com. That is the address book the owner asked for; it
//! is also why a person can be forgotten ([`SentHistory::forget`]), which
//! removes every entry naming them.
//!
//! # Newest first, and capped
//!
//! [`MAX_ENTRIES`] entries, newest first, trimmed on every write -- a record
//! file that only grows is a file nobody asked to keep forever. A person whose
//! last Send has aged out of the cap drops out of the suggestions with it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name, inside [`crate::settings::config_dir`].
pub const SENT_HISTORY_FILE_NAME: &str = "sent_history.json";

/// How many Sends are remembered. See the module docs.
pub const MAX_ENTRIES: usize = 500;

/// Where `sent_history.json` lives, or `None` with no config directory -- in
/// which case nothing is recorded, as `receive_history` falls back.
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(SENT_HISTORY_FILE_NAME))
}

/// What an entry records: a Send (a link) or a share (a person). Spec 11.7:
/// both feed the one address book the Send composer and the Share dialog
/// suggest from. `Send` by default, so a file from before shares reads
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SentKind {
    #[default]
    Send,
    Share,
}

/// One Send this app made from a record.
///
/// `#[serde(default)]` on the struct, so a file from a version without one of
/// these fields reads it as that type's default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SentRecord {
    /// A Send or a share. For a share, `send_id` is empty and `fields` is
    /// empty: a person gets the whole record, and there is no link.
    pub kind: SentKind,
    /// When the Send was made, in milliseconds since the Unix epoch, **UTC**.
    pub sent_at_unix_millis: i64,
    /// The Send's server id -- what names it to the server. **Not its link**;
    /// see the module docs.
    pub send_id: String,
    /// Who it went to, as typed, or empty when nobody was named.
    pub recipient: String,
    /// The vault item it was made from.
    pub item_id: String,
    /// That item's name when it was sent.
    pub name: String,
    /// The names of the fields that travelled -- labels, never values.
    pub fields: Vec<String>,
}

/// Somebody this user has sent to, for the composer's suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Their address, as first typed.
    pub email: String,
    /// When they were last sent something, UTC milliseconds.
    pub last_sent_unix_millis: i64,
    /// The Send ids this file holds for them, newest first.
    pub send_ids: Vec<String>,
    /// How many of this person's entries are Sends, and how many shares --
    /// what the suggestion's count says ("3 sent", "2 shared").
    pub sent: usize,
    pub shared: usize,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SentHistory {
    /// **Newest first.**
    pub entries: Vec<SentRecord>,
}

/// Addresses are compared without case or surrounding space: `M.Reyes@…` and
/// `m.reyes@…` are one person.
fn same_person(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl SentHistory {
    /// Reads the file. Every failure -- absent, empty, unparseable -- is an
    /// empty history, as `receive_history` reads its own.
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
    pub fn record(&mut self, record: SentRecord) {
        self.entries.insert(0, record);
        self.entries.truncate(MAX_ENTRIES);
    }

    /// The entry for one Send, by its server id.
    pub fn for_send(&self, send_id: &str) -> Option<&SentRecord> {
        self.entries.iter().find(|entry| !send_id.is_empty() && entry.send_id == send_id)
    }

    /// **Everyone this user has sent to, most recently sent first**, one row
    /// per person however many Sends they had.
    pub fn people(&self) -> Vec<Person> {
        let mut people: Vec<Person> = Vec::new();
        for entry in &self.entries {
            if entry.recipient.trim().is_empty() {
                continue;
            }
            let is_send = entry.kind == SentKind::Send;
            match people.iter_mut().find(|p| same_person(&p.email, &entry.recipient)) {
                Some(person) => {
                    if is_send && !entry.send_id.is_empty() {
                        person.send_ids.push(entry.send_id.clone());
                    }
                    person.sent += usize::from(is_send);
                    person.shared += usize::from(!is_send);
                    person.last_sent_unix_millis =
                        person.last_sent_unix_millis.max(entry.sent_at_unix_millis);
                }
                None => people.push(Person {
                    email: entry.recipient.trim().to_string(),
                    last_sent_unix_millis: entry.sent_at_unix_millis,
                    send_ids: if is_send && !entry.send_id.is_empty() { vec![entry.send_id.clone()] } else { Vec::new() },
                    sent: usize::from(is_send),
                    shared: usize::from(!is_send),
                }),
            }
        }
        people.sort_by_key(|a| std::cmp::Reverse(a.last_sent_unix_millis));
        people
    }

    /// The people whose address contains `typed`, for the suggestions under
    /// the Recipient box. Empty `typed` suggests everyone.
    pub fn suggestions(&self, typed: &str) -> Vec<Person> {
        let typed = typed.trim().to_ascii_lowercase();
        self.people()
            .into_iter()
            .filter(|p| typed.is_empty() || p.email.to_ascii_lowercase().contains(&typed))
            // Suggesting exactly what is typed is suggesting nothing.
            .filter(|p| !same_person(&p.email, &typed))
            .collect()
    }

    /// **Forgets a person**: every entry naming them goes. Returns what went,
    /// so the caller can delete those Sends from the server first when the
    /// person still has live ones -- see the module docs.
    pub fn forget(&mut self, email: &str) -> Vec<SentRecord> {
        let (gone, kept) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|entry| same_person(&entry.recipient, email));
        self.entries = kept;
        gone
    }

    /// Drops the entry for a Send that no longer exists on the server, so a
    /// deleted Send stops counting as a share.
    pub fn drop_send(&mut self, send_id: &str) {
        self.entries.retain(|entry| entry.send_id != send_id);
    }

    /// Every entry for `old` now names `new`: a shared record moved back to
    /// the personal vault takes a new id (spec 9.4, Q9).
    pub fn rekey_item(&mut self, old: &str, new: &str) {
        for entry in &mut self.entries {
            if entry.item_id == old {
                entry.item_id = new.to_string();
            }
        }
    }

    /// Writes the whole file, trimming first, creating the directory if this
    /// is the first thing this app ever wrote there.
    pub fn persist(&self, path: &Path) -> std::io::Result<()> {
        let mut trimmed = self.clone();
        trimmed.entries.truncate(MAX_ENTRIES);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&trimmed)?)
    }
}

/// Reads the history, applies `change`, writes it back -- a read-modify-write
/// for `receive_history::append`'s reason: another window may have written in
/// between. Returns the history as written.
pub fn update(path: &Path, change: impl FnOnce(&mut SentHistory)) -> std::io::Result<SentHistory> {
    let mut history = SentHistory::load(path);
    change(&mut history);
    history.persist(path)?;
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> (crate::test_scratch::ScratchDir, PathBuf) {
        let dir = crate::test_scratch::ScratchDir::new(&format!("sent-history-{tag}"));
        let path = dir.join("sent.json");
        (dir, path)
    }

    fn sent(at: i64, id: &str, to: &str) -> SentRecord {
        SentRecord {
            kind: SentKind::Send,
            sent_at_unix_millis: at,
            send_id: id.to_string(),
            recipient: to.to_string(),
            item_id: format!("item-{id}"),
            name: format!("Record {id}"),
            fields: vec!["Username".to_string(), "Password".to_string()],
        }
    }

    /// **Newest first, one row per person, case and space ignored**, and an
    /// entry with nobody named is nobody.
    #[test]
    fn people_are_listed_once_each_most_recently_sent_first() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "m.reyes@ledgerline.com"));
        history.record(sent(2, "b", "j.abara@vantage.io"));
        history.record(sent(3, "c", " M.Reyes@Ledgerline.com "));
        history.record(sent(4, "d", ""));
        let people = history.people();
        assert_eq!(people.len(), 2, "{people:?}");
        assert_eq!(people[0].email, "M.Reyes@Ledgerline.com");
        assert_eq!(people[0].last_sent_unix_millis, 3);
        assert_eq!(people[0].send_ids, ["c", "a"]);
        assert_eq!(people[1].email, "j.abara@vantage.io");
    }

    #[test]
    fn suggestions_narrow_to_what_is_typed_and_never_echo_it() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "m.reyes@ledgerline.com"));
        history.record(sent(2, "b", "m.renard@vantage.io"));
        history.record(sent(3, "c", "j.abara@vantage.io"));
        let emails = |typed: &str| {
            history.suggestions(typed).into_iter().map(|p| p.email).collect::<Vec<_>>()
        };
        assert_eq!(emails("m.re"), ["m.renard@vantage.io", "m.reyes@ledgerline.com"]);
        assert_eq!(emails("VANTAGE"), ["j.abara@vantage.io", "m.renard@vantage.io"]);
        assert_eq!(emails("").len(), 3);
        assert!(emails("m.reyes@ledgerline.com").is_empty(), "the typed address was suggested");
    }

    /// **Forgetting a person removes every entry naming them and hands them
    /// back**, so their live Sends can be deleted from the server first.
    #[test]
    fn forgetting_a_person_removes_and_returns_all_their_sends() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "m.reyes@ledgerline.com"));
        history.record(sent(2, "b", "j.abara@vantage.io"));
        history.record(sent(3, "c", "M.REYES@ledgerline.com"));
        let gone = history.forget("m.reyes@ledgerline.com");
        assert_eq!(gone.iter().map(|e| e.send_id.as_str()).collect::<Vec<_>>(), ["c", "a"]);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.people().len(), 1);
    }

    #[test]
    fn a_send_is_found_by_its_server_id_and_can_be_dropped() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "x@y.z"));
        assert_eq!(history.for_send("a").map(|e| e.name.as_str()), Some("Record a"));
        assert!(history.for_send("").is_none(), "an empty id matched something");
        history.drop_send("a");
        assert!(history.for_send("a").is_none());
    }

    /// Round-trips through the file, trims to the cap, and reads a missing,
    /// empty or broken file as empty.
    #[test]
    fn the_file_round_trips_is_capped_and_forgives_a_broken_one() {
        let (_dir, path) = temp_path("roundtrip");
        assert_eq!(SentHistory::load(&path), SentHistory::default(), "a missing file is empty");
        let written = update(&path, |h| {
            for n in 0..(MAX_ENTRIES + 5) {
                h.record(sent(n as i64, &n.to_string(), "p@q.r"));
            }
        })
        .expect("writes");
        assert_eq!(written.entries.len(), MAX_ENTRIES);
        let read = SentHistory::load(&path);
        assert_eq!(read, written);
        assert_eq!(read.entries[0].send_id, (MAX_ENTRIES + 4).to_string(), "not newest first");
        std::fs::write(&path, "{ not json").expect("writes");
        assert_eq!(SentHistory::load(&path), SentHistory::default());
    }

    /// **Nothing that opens a Send is a field of an entry.** Read off this
    /// file's own declaration, as `receive_history` guards its own: a field
    /// named for a link, a URL or a key does not get past the suite.
    #[test]
    fn an_entry_holds_nothing_that_opens_a_send() {
        let source = include_str!("sent_history.rs");
        let head = concat!("pub struct SentRe", "cord {");
        let at = source.find(head).expect("the struct is declared");
        let body = &source[at..at + source[at..].find("\n}").expect("the struct ends")];
        for banned in ["url", "link", "key", "password", "secret", "value"] {
            let field = format!("pub {banned}");
            assert!(!body.contains(&field), "SentRecord grew a `{banned}` field");
        }
    }

    /// **A share is a person in the address book too** (spec 11.7), counted
    /// separately so the suggestion can say "2 shared" or "3 sent", and a file
    /// from before shares reads as all Sends.
    #[test]
    fn shares_and_sends_feed_one_address_book_with_their_own_counts() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "anna@example.com"));
        history.record(SentRecord { kind: SentKind::Share, item_id: "r1".into(), recipient: "Anna@example.com".into(), sent_at_unix_millis: 2, ..SentRecord::default() });
        history.record(SentRecord { kind: SentKind::Share, item_id: "r2".into(), recipient: "anna@example.com".into(), sent_at_unix_millis: 3, ..SentRecord::default() });
        let people = history.people();
        assert_eq!(people.len(), 1);
        assert_eq!((people[0].sent, people[0].shared), (1, 2));
        assert_eq!(people[0].send_ids, ["a"], "a share has no Send id");
        let old: SentHistory = serde_json::from_str(r#"{"entries":[{"send_id":"x","recipient":"b@c.d"}]}"#).expect("an old file");
        assert_eq!(old.entries[0].kind, SentKind::Send);
    }

    #[test]
    fn a_record_moved_back_keeps_its_entries_under_its_new_id() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "x@y.z"));
        history.entries[0].item_id = "old".into();
        history.rekey_item("old", "new");
        assert_eq!(history.entries[0].item_id, "new");
    }
}

//! Slice 9: reading another password manager's export into Bitwarden items.
//!
//! # What this module is, and is not
//!
//! It turns the bytes of an export file into [`ImportItem`]s, decides which of
//! them the vault already holds ([`plan`]), and creates the rest one at a
//! time through the same create path every other write in this app uses
//! ([`run`]). It does NOT encrypt anything and it does NOT call
//! `POST /ciphers/import`: see the slice plan's Rulings for why creating
//! through the existing path is the Bitwarden-compatible choice with the least
//! new crypto.
//!
//! # No `csv` crate
//!
//! The export formats are small, flat, comma-separated tables. The reader
//! below is [`parse_csv`], about sixty lines, handling quotes, doubled quotes,
//! embedded newlines, a byte-order mark and CRLF. A crate would have added a
//! dependency (and a `cargo deny` surface) to parse untrusted files this app
//! has to treat as secrets anyway; this reader hands every cell back as a
//! `Zeroizing<String>` so the secret is wiped when the row is dropped, which a
//! general-purpose crate would not.
//!
//! # Secrets
//!
//! Nothing here logs a cell. Every secret-bearing value ends in a
//! [`VaultItem`], whose `Debug` already elides them, and the file's own text is
//! held in a `Zeroizing` buffer by the caller.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use zeroize::Zeroizing;

use crate::vault_bridge::{
    Folder, ItemKind, LoginData, NewItem, RelayedItem, UriEntry, VaultError, VaultField,
    VaultItem,
};

/// The most a file may be before it is refused unread. A real export of tens of
/// thousands of logins is a few megabytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the file came from: the six tiles of the source step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportSource {
    Bitwarden,
    OnePassword,
    Keeper,
    LastPass,
    Browser,
    GenericCsv,
}

impl ImportSource {
    /// The tiles, in the spec's order.
    pub const ALL: [ImportSource; 6] = [
        ImportSource::Bitwarden,
        ImportSource::OnePassword,
        ImportSource::Keeper,
        ImportSource::LastPass,
        ImportSource::Browser,
        ImportSource::GenericCsv,
    ];

    /// The tile's label, which is also the name the file step quotes.
    pub fn label(self) -> &'static str {
        match self {
            ImportSource::Bitwarden => "Bitwarden",
            ImportSource::OnePassword => "1Password",
            ImportSource::Keeper => "Keeper",
            ImportSource::LastPass => "LastPass",
            ImportSource::Browser => crate::i18n::t("import.source_browser"),
            ImportSource::GenericCsv => crate::i18n::t("import.source_csv"),
        }
    }

    /// The monogram on the tile.
    pub fn initials(self) -> &'static str {
        match self {
            ImportSource::Bitwarden => "BW",
            ImportSource::OnePassword => "1P",
            ImportSource::Keeper => "KE",
            ImportSource::LastPass => "LP",
            ImportSource::Browser => "CR",
            ImportSource::GenericCsv => "CSV",
        }
    }
}

/// Why a file could not be read, in words for the file step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Nothing in it.
    Empty,
    /// Bigger than [`MAX_FILE_BYTES`].
    TooLarge,
    /// An encrypted Bitwarden export: it needs its password and a decrypt
    /// step this wizard does not have.
    Encrypted,
    /// Not the format the user said it was.
    NotThisFormat,
    /// Readable, but it held nothing this app can import.
    NothingToImport,
    /// Could not be read from disk.
    Unreadable,
}

impl ParseError {
    pub fn sentence(self) -> &'static str {
        match self {
            ParseError::Empty => crate::i18n::t("import.err_empty"),
            ParseError::TooLarge => crate::i18n::t("import.err_too_large"),
            ParseError::Encrypted => crate::i18n::t("import.err_encrypted"),
            ParseError::NotThisFormat => crate::i18n::t("import.err_not_this_format"),
            ParseError::NothingToImport => crate::i18n::t("import.err_nothing"),
            ParseError::Unreadable => crate::i18n::t("import.err_unreadable"),
        }
    }
}

/// One entry read from a file: the item as it will be created, and the name of
/// the folder it belongs in.
pub struct ImportItem {
    /// `id` is empty: nothing in it came from this vault.
    pub item: VaultItem,
    pub folder: Option<String>,
}

/// Everything a file held.
pub struct Parsed {
    pub items: Vec<ImportItem>,
    /// Entries the file had that this wizard cannot make into an item (a trashed
    /// item, a type Bitwarden does not have, a row with no usable data).
    pub left_out: usize,
}

/// Reads `bytes` as `source`'s export.
pub fn parse(source: ImportSource, bytes: &[u8]) -> Result<Parsed, ParseError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => Zeroizing::new(text.trim_start_matches('\u{feff}').to_string()),
        Err(_) => return Err(ParseError::NotThisFormat),
    };
    if text.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let parsed = match source {
        ImportSource::Bitwarden => {
            if text.trim_start().starts_with('{') {
                parse_bitwarden_json(&text)?
            } else {
                parse_bitwarden_csv(&text)?
            }
        }
        ImportSource::OnePassword => parse_mapped(&text, MapperKind::OnePassword)?,
        ImportSource::LastPass => parse_lastpass(&text)?,
        ImportSource::Browser => parse_mapped(&text, MapperKind::Browser)?,
        ImportSource::GenericCsv => parse_mapped(&text, MapperKind::Generic)?,
        ImportSource::Keeper => parse_keeper(&text)?,
    };
    if parsed.items.is_empty() {
        return Err(ParseError::NothingToImport);
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// The table reader
// ---------------------------------------------------------------------------

type Row = Vec<Zeroizing<String>>;

/// RFC 4180: comma-separated, fields optionally quoted with `"`, a doubled
/// quote inside a quoted field is one quote, a quoted field may hold commas and
/// line breaks, rows end at CRLF, LF or CR. Rows with no characters in any cell
/// are dropped.
pub fn parse_csv(text: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut row: Row = Vec::new();
    let mut cell = Zeroizing::new(String::new());
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = text.chars().peekable();
    let end_row = |row: &mut Row, cell: &mut Zeroizing<String>, rows: &mut Vec<Row>| {
        row.push(std::mem::replace(cell, Zeroizing::new(String::new())));
        let empty = row.len() == 1 && row[0].is_empty();
        let finished = std::mem::take(row);
        if !empty {
            rows.push(finished);
        }
    };
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            ',' => {
                row.push(std::mem::replace(&mut cell, Zeroizing::new(String::new())));
                was_quoted = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_row(&mut row, &mut cell, &mut rows);
                was_quoted = false;
            }
            '\n' => {
                end_row(&mut row, &mut cell, &mut rows);
                was_quoted = false;
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() || was_quoted {
        end_row(&mut row, &mut cell, &mut rows);
    }
    rows
}

/// A table with a header row, read by column name (case-insensitive).
struct Table {
    header: Vec<String>,
    rows: Vec<Row>,
}

impl Table {
    fn read(text: &str) -> Result<Self, ParseError> {
        let mut rows = parse_csv(text);
        if rows.is_empty() {
            return Err(ParseError::Empty);
        }
        let header = rows.remove(0).iter().map(|c| c.trim().to_lowercase()).collect();
        Ok(Self { header, rows })
    }

    fn column(&self, names: &[&str]) -> Option<usize> {
        names.iter().find_map(|name| self.header.iter().position(|h| h == name))
    }

    fn has(&self, names: &[&str]) -> bool {
        self.column(names).is_some()
    }
}

/// The first of `names` that is a column of `table`, read from `row`; `None`
/// when the column is absent or the cell blank.
fn cell<'a>(table: &Table, row: &'a Row, names: &[&str]) -> Option<&'a str> {
    let at = table.column(names)?;
    row.get(at).map(|c| c.as_str()).filter(|c| !c.trim().is_empty())
}

// ---------------------------------------------------------------------------
// Building items
// ---------------------------------------------------------------------------

fn secret(value: Option<&str>) -> Option<Zeroizing<String>> {
    value.map(|v| Zeroizing::new(v.to_string()))
}

/// What a row knows, before it becomes an item.
#[derive(Default)]
struct Draft {
    name: Option<String>,
    username: Option<String>,
    password: Option<Zeroizing<String>>,
    totp: Option<Zeroizing<String>>,
    uris: Vec<String>,
    notes: Option<Zeroizing<String>>,
    folder: Option<String>,
    favorite: bool,
    fields: Vec<(String, Zeroizing<String>)>,
}

/// Hand-written, and eliding everything: a draft is a row of somebody's
/// passwords.
impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Draft { .. }")
    }
}

impl Draft {
    fn login(self) -> Option<ImportItem> {
        let nothing = self.username.is_none()
            && self.password.is_none()
            && self.totp.is_none()
            && self.uris.is_empty()
            && self.notes.is_none()
            && self.name.is_none();
        if nothing {
            return None;
        }
        let name = self.name.clone().unwrap_or_else(|| {
            self.uris.first().map(|u| host_of(u)).filter(|h| !h.is_empty()).unwrap_or_else(|| "--".to_string())
        });
        let login = LoginData {
            username: self.username,
            password: self.password,
            totp: self.totp,
            uris: self.uris.into_iter().map(|uri| UriEntry { uri: Some(uri), other: Default::default() }).collect(),
            other: Default::default(),
        };
        let fields = self
            .fields
            .into_iter()
            .map(|(name, value)| VaultField { name: Some(name), value: Some(value), other: text_field() })
            .collect();
        Some(ImportItem {
            item: VaultItem {
                id: String::new(),
                name,
                fields,
                login: Some(login),
                card: None,
                identity: None,
                ssh_key: None,
                notes: self.notes,
                item_type: Some(1),
                folder_id: None,
                favorite: self.favorite,
                other: Default::default(),
            },
            folder: self.folder,
        })
    }

    fn note(self) -> ImportItem {
        let mut other = serde_json::Map::new();
        other.insert("secureNote".into(), serde_json::json!({ "type": 0 }));
        ImportItem {
            item: VaultItem {
                id: String::new(),
                name: self.name.unwrap_or_else(|| "--".to_string()),
                fields: Vec::new(),
                login: None,
                card: None,
                identity: None,
                ssh_key: None,
                notes: self.notes,
                item_type: Some(2),
                folder_id: None,
                favorite: self.favorite,
                other,
            },
            folder: self.folder,
        }
    }
}

/// `{"type": 0}`: the key Bitwarden's custom fields carry for a plain-text one.
fn text_field() -> serde_json::Map<String, serde_json::Value> {
    let mut other = serde_json::Map::new();
    other.insert("type".into(), serde_json::json!(0));
    other
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// A Keeper / LastPass folder path (`a\b`) as Bitwarden spells nesting (`a/b`).
fn folder_path(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split(['\\', '/']).map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

/// The host of a URL or a bare address: no scheme, no `www.`, no path, lower
/// case. Used for names and for telling duplicates.
pub fn host_of(uri: &str) -> String {
    let lower = uri.trim().to_lowercase();
    let rest = lower.split_once("://").map(|(_, r)| r).unwrap_or(&lower);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn is_true(value: Option<&str>) -> bool {
    matches!(value.map(|v| v.trim().to_lowercase()).as_deref(), Some("1" | "true" | "yes" | "y"))
}

// ---------------------------------------------------------------------------
// Bitwarden
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct BitwardenFile {
    #[serde(default)]
    encrypted: bool,
    #[serde(default)]
    folders: Vec<Folder>,
    #[serde(default)]
    items: Vec<VaultItem>,
}

/// Keys a Bitwarden export carries on an item that belong to the vault it came
/// from and must not be sent to a create.
const FOREIGN_ITEM_KEYS: [&str; 14] = [
    "organizationId",
    "collectionIds",
    "revisionDate",
    "creationDate",
    "deletedDate",
    "archivedDate",
    "attachments",
    "passwordHistory",
    "object",
    "edit",
    "viewPassword",
    "permissions",
    "key",
    "organizationUseTotp",
];

fn parse_bitwarden_json(text: &str) -> Result<Parsed, ParseError> {
    let file: BitwardenFile = serde_json::from_str(text).map_err(|_| ParseError::NotThisFormat)?;
    if file.encrypted {
        return Err(ParseError::Encrypted);
    }
    let names: HashMap<&str, &str> = file.folders.iter().map(|f| (f.id.as_str(), f.name.as_str())).collect();
    let mut items = Vec::new();
    let mut left_out = 0;
    for mut item in file.items {
        let trashed = item.other.get("deletedDate").is_some_and(|d| !d.is_null());
        let kind = ItemKind::of(&item);
        if trashed || matches!(kind, ItemKind::Unknown(_)) {
            left_out += 1;
            continue;
        }
        let folder = item.folder_id.as_deref().and_then(|id| names.get(id)).map(|n| n.to_string());
        item.id = String::new();
        item.folder_id = None;
        for key in FOREIGN_ITEM_KEYS {
            item.other.remove(key);
        }
        if let Some(login) = item.login.as_mut() {
            login.other.remove("passwordRevisionDate");
            login.other.remove("fido2Credentials");
            for uri in &mut login.uris {
                uri.other.remove("uriChecksum");
            }
        }
        items.push(ImportItem { item, folder });
    }
    Ok(Parsed { items, left_out })
}

fn parse_bitwarden_csv(text: &str) -> Result<Parsed, ParseError> {
    let table = Table::read(text)?;
    if !table.has(&["login_username", "login_uri", "login_password"]) || !table.has(&["name"]) {
        return Err(ParseError::NotThisFormat);
    }
    let mut items = Vec::new();
    let mut left_out = 0;
    for row in &table.rows {
        let kind = cell(&table, row, &["type"]).map(|t| t.trim().to_lowercase());
        let draft = Draft {
            name: trimmed(cell(&table, row, &["name"])),
            notes: secret(cell(&table, row, &["notes"])),
            folder: cell(&table, row, &["folder"]).and_then(folder_path),
            favorite: is_true(cell(&table, row, &["favorite"])),
            username: trimmed(cell(&table, row, &["login_username"])),
            password: secret(cell(&table, row, &["login_password"])),
            totp: secret(cell(&table, row, &["login_totp"]).map(str::trim)),
            uris: cell(&table, row, &["login_uri"])
                .map(|u| u.lines().map(str::trim).filter(|u| !u.is_empty()).map(str::to_string).collect())
                .unwrap_or_default(),
            fields: cell(&table, row, &["fields"]).map(parse_fields).unwrap_or_default(),
        };
        match kind.as_deref() {
            Some("login") | None => match draft.login() {
                Some(item) => items.push(item),
                None => left_out += 1,
            },
            Some("note") => items.push(draft.note()),
            Some(_) => left_out += 1,
        }
    }
    Ok(Parsed { items, left_out })
}

/// Bitwarden's CSV `fields` column: one `name: value` per line.
fn parse_fields(text: &str) -> Vec<(String, Zeroizing<String>)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(": ")?;
            Some((name.trim().to_string(), Zeroizing::new(value.to_string())))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1Password, Chrome/Edge and any other CSV with named columns
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MapperKind {
    OnePassword,
    Browser,
    Generic,
}

const NAME_COLUMNS: [&str; 4] = ["title", "name", "account", "site name"];
const URL_COLUMNS: [&str; 8] = ["url", "website", "web site", "login_uri", "login url", "site", "uri", "address"];
const USER_COLUMNS: [&str; 6] = ["username", "login_username", "login username", "user", "login", "email"];
const PASSWORD_COLUMNS: [&str; 3] = ["password", "login_password", "pass"];
const TOTP_COLUMNS: [&str; 6] = ["otpauth", "one-time password", "totp", "login_totp", "otp", "one time password"];
const NOTES_COLUMNS: [&str; 5] = ["notes", "note", "notesplain", "extra", "comments"];
const FOLDER_COLUMNS: [&str; 3] = ["folder", "grouping", "group"];

fn parse_mapped(text: &str, kind: MapperKind) -> Result<Parsed, ParseError> {
    let table = Table::read(text)?;
    // The file must at least name something to identify an entry and something
    // to sign in with; a table of anything else is not a password export.
    let identifies = table.has(&NAME_COLUMNS) || table.has(&URL_COLUMNS);
    let signs_in = table.has(&USER_COLUMNS) || table.has(&PASSWORD_COLUMNS);
    if !identifies || !signs_in {
        return Err(ParseError::NotThisFormat);
    }
    let mut items = Vec::new();
    let mut left_out = 0;
    for row in &table.rows {
        let entry_type = cell(&table, row, &["type"]).map(|t| t.trim().to_lowercase());
        let draft = Draft {
            name: trimmed(cell(&table, row, &NAME_COLUMNS)),
            username: trimmed(cell(&table, row, &USER_COLUMNS)),
            password: secret(cell(&table, row, &PASSWORD_COLUMNS)),
            totp: secret(cell(&table, row, &TOTP_COLUMNS).map(str::trim)),
            uris: trimmed(cell(&table, row, &URL_COLUMNS)).into_iter().collect(),
            notes: secret(cell(&table, row, &NOTES_COLUMNS)),
            folder: if kind == MapperKind::Browser {
                None
            } else {
                cell(&table, row, &FOLDER_COLUMNS).and_then(folder_path)
            },
            favorite: kind == MapperKind::OnePassword && is_true(cell(&table, row, &["favorite"])),
            fields: Vec::new(),
        };
        // 1Password's `Type` column names the entry's kind; anything that is
        // not a login and has no password is kept as a secure note rather than
        // as a login with nothing to sign in with.
        let note_only = kind == MapperKind::OnePassword
            && entry_type.as_deref().is_some_and(|t| t.contains("note"))
            && draft.password.is_none();
        if note_only {
            items.push(draft.note());
            continue;
        }
        match draft.login() {
            Some(item) => items.push(item),
            None => left_out += 1,
        }
    }
    Ok(Parsed { items, left_out })
}

// ---------------------------------------------------------------------------
// LastPass: `url,username,password,totp,extra,name,grouping,fav`
// ---------------------------------------------------------------------------

/// LastPass marks a secure note by giving it this address.
const LASTPASS_NOTE_URL: &str = "http://sn";

fn parse_lastpass(text: &str) -> Result<Parsed, ParseError> {
    let table = Table::read(text)?;
    if !table.has(&["url"]) || !table.has(&["grouping"]) || !table.has(&["extra"]) {
        return Err(ParseError::NotThisFormat);
    }
    let mut items = Vec::new();
    let mut left_out = 0;
    for row in &table.rows {
        let url = cell(&table, row, &["url"]).map(str::trim);
        let draft = Draft {
            name: trimmed(cell(&table, row, &["name"])),
            username: trimmed(cell(&table, row, &["username"])),
            password: secret(cell(&table, row, &["password"])),
            totp: secret(cell(&table, row, &["totp"]).map(str::trim)),
            uris: url.filter(|u| *u != LASTPASS_NOTE_URL).map(str::to_string).into_iter().collect(),
            notes: secret(cell(&table, row, &["extra"])),
            folder: cell(&table, row, &["grouping"]).and_then(folder_path),
            favorite: is_true(cell(&table, row, &["fav"])),
            fields: Vec::new(),
        };
        if url == Some(LASTPASS_NOTE_URL) {
            items.push(draft.note());
        } else {
            match draft.login() {
                Some(item) => items.push(item),
                None => left_out += 1,
            }
        }
    }
    Ok(Parsed { items, left_out })
}

// ---------------------------------------------------------------------------
// Keeper: no header; folder, title, login, password, url, notes, shared
// folder, then name/value pairs of custom fields
// ---------------------------------------------------------------------------

/// A Keeper custom field carrying a one-time-password seed is named this.
const KEEPER_TOTP_FIELD: &str = "TFC:Keeper";

fn parse_keeper(text: &str) -> Result<Parsed, ParseError> {
    let rows = parse_csv(text);
    // At least the seven fixed columns; a Bitwarden or Chrome file has a header
    // row of fewer or differently shaped columns, and would otherwise import as
    // an entry named "name".
    if rows.first().is_none_or(|r| r.len() < 6) {
        return Err(ParseError::NotThisFormat);
    }
    let mut items = Vec::new();
    let mut left_out = 0;
    for row in &rows {
        let get = |at: usize| row.get(at).map(|c| c.as_str()).filter(|c| !c.trim().is_empty());
        let mut totp = None;
        let mut fields = Vec::new();
        let mut at = 7;
        while at + 1 < row.len() {
            let (name, value) = (row[at].trim(), &row[at + 1]);
            if !name.is_empty() && !value.is_empty() {
                if name == KEEPER_TOTP_FIELD {
                    totp = Some(Zeroizing::new(value.to_string()));
                } else {
                    fields.push((name.to_string(), Zeroizing::new(value.to_string())));
                }
            }
            at += 2;
        }
        let draft = Draft {
            folder: get(0).and_then(folder_path),
            name: trimmed(get(1)),
            username: trimmed(get(2)),
            password: secret(get(3)),
            uris: trimmed(get(4)).into_iter().collect(),
            notes: secret(get(5)),
            totp,
            favorite: false,
            fields,
        };
        match draft.login() {
            Some(item) => items.push(item),
            None => left_out += 1,
        }
    }
    Ok(Parsed { items, left_out })
}

// ---------------------------------------------------------------------------
// Duplicates
// ---------------------------------------------------------------------------

/// What an item is, for telling two apart: its type, name, user and host,
/// lower-cased. The secret is compared separately, so it is never part of a
/// key that sits in a map.
fn identity_of(item: &VaultItem) -> String {
    let kind = item.item_type.unwrap_or(1);
    let user = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
    let host = item
        .login
        .as_ref()
        .and_then(|l| l.uris.first())
        .and_then(|u| u.uri.as_deref())
        .map(host_of)
        .unwrap_or_default();
    format!("{kind}\u{1f}{}\u{1f}{}\u{1f}{host}", item.name.trim().to_lowercase(), user.trim().to_lowercase())
}

/// The one secret that makes two items the same thing: a login's password, a
/// note's body, a card's number, an SSH key's private half.
fn secret_of(item: &VaultItem) -> Zeroizing<String> {
    let text = item
        .login
        .as_ref()
        .and_then(|l| l.password.as_ref())
        .or(item.card.as_ref().and_then(|c| c.number.as_ref()))
        .or(item.ssh_key.as_ref().and_then(|k| k.private_key.as_ref()))
        .or(item.notes.as_ref());
    Zeroizing::new(text.map(|t| t.to_string()).unwrap_or_default())
}

/// What will be created and what is left alone.
pub struct Plan {
    pub to_create: Vec<ImportItem>,
    /// Items the vault already holds, or the file held twice.
    pub duplicates: usize,
    /// [`Parsed::left_out`].
    pub left_out: usize,
}

/// Drops from `parsed` every item that `existing` (the vault) or an earlier
/// item of the file already is. Same type, name, user and site AND the same
/// secret: two logins for one site with different passwords are two logins.
pub fn plan(parsed: Parsed, existing: &[VaultItem]) -> Plan {
    let mut seen: HashMap<String, Vec<Zeroizing<String>>> = HashMap::new();
    for item in existing {
        seen.entry(identity_of(item)).or_default().push(secret_of(item));
    }
    let mut to_create = Vec::new();
    let mut duplicates = 0;
    for entry in parsed.items {
        let key = identity_of(&entry.item);
        let secret = secret_of(&entry.item);
        let known = seen.entry(key).or_default();
        if known.iter().any(|s| **s == *secret) {
            duplicates += 1;
        } else {
            known.push(secret);
            to_create.push(entry);
        }
    }
    Plan { to_create, duplicates, left_out: parsed.left_out }
}

// ---------------------------------------------------------------------------
// Creating
// ---------------------------------------------------------------------------

/// The create body for `entry`, filed under `folder_id`.
pub fn new_item_for(entry: &ImportItem, folder_id: Option<&str>) -> Result<NewItem, &'static str> {
    let mut value = serde_json::to_value(&entry.item).map_err(|_| "an item could not be written out")?;
    let Some(object) = value.as_object_mut() else {
        return Err("an item could not be written out");
    };
    object.remove("id");
    object.remove("folderId");
    object.insert(
        "folderId".to_string(),
        match folder_id {
            Some(id) => serde_json::Value::String(id.to_string()),
            None => serde_json::Value::Null,
        },
    );
    RelayedItem::from_payload(value).map(NewItem::Relayed)
}

/// How an import ended.
#[derive(Debug, Default)]
pub struct Report {
    pub imported: usize,
    pub failed: usize,
    pub duplicates: usize,
    pub left_out: usize,
    pub stopped_early: bool,
    /// The first error a write answered, so the window can notice a signed-out
    /// session. Its text carries no secret.
    pub first_error: Option<VaultError>,
}

/// Where the writes go: the vault's create calls, as closures so a test can
/// supply its own.
pub struct Sinks<'a> {
    pub existing_folders: Vec<Folder>,
    pub create_folder: &'a mut dyn FnMut(&str) -> Result<Folder, VaultError>,
    pub create_item: &'a mut dyn FnMut(&NewItem) -> Result<(), VaultError>,
}

/// Failures in a row after which the import stops: a server that is down is
/// not asked about every remaining item.
pub const GIVE_UP_AFTER: usize = 3;

/// Creates the folders the plan needs, then every item, counting each one that
/// lands in `progress`. Stops when `cancel` is set (after the item in hand) or
/// after [`GIVE_UP_AFTER`] failures in a row.
pub fn run(plan: Plan, sinks: Sinks<'_>, progress: &AtomicUsize, cancel: &AtomicBool) -> Report {
    let Sinks { existing_folders, create_folder, create_item } = sinks;
    let mut report = Report { duplicates: plan.duplicates, left_out: plan.left_out, ..Report::default() };
    let mut folders: HashMap<String, String> =
        existing_folders.into_iter().map(|f| (f.name, f.id)).collect();
    let mut in_a_row = 0;
    for entry in &plan.to_create {
        if cancel.load(Ordering::Relaxed) {
            report.stopped_early = true;
            break;
        }
        let folder_id = match entry.folder.as_deref() {
            None => None,
            Some(name) => match folders.get(name) {
                Some(id) => Some(id.clone()),
                None => match create_folder(name) {
                    Ok(folder) => {
                        folders.insert(name.to_string(), folder.id.clone());
                        Some(folder.id)
                    }
                    // A folder that cannot be made files the item nowhere
                    // rather than losing it.
                    Err(_) => None,
                },
            },
        };
        let outcome = new_item_for(entry, folder_id.as_deref())
            .map_err(|e| VaultError::Parse(e.to_string()))
            .and_then(|new_item| create_item(&new_item));
        match outcome {
            Ok(()) => {
                report.imported += 1;
                in_a_row = 0;
                progress.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                report.failed += 1;
                in_a_row += 1;
                if report.first_error.is_none() {
                    report.first_error = Some(e);
                }
                if in_a_row >= GIVE_UP_AFTER {
                    report.stopped_early = true;
                    break;
                }
            }
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(rows: &[Row]) -> Vec<Vec<&str>> {
        rows.iter().map(|r| r.iter().map(|c| c.as_str()).collect()).collect()
    }

    fn items(source: ImportSource, text: &str) -> Parsed {
        parse(source, text.as_bytes()).expect("the file parses")
    }

    fn login_of(entry: &ImportItem) -> &LoginData {
        entry.item.login.as_ref().expect("a login")
    }

    // -- the table reader --------------------------------------------------

    #[test]
    fn the_reader_handles_quotes_commas_newlines_and_doubled_quotes() {
        let rows = parse_csv("a,\"b,c\",\"say \"\"hi\"\"\"\r\n\"line1\nline2\",,z\n");
        assert_eq!(cells(&rows), vec![vec!["a", "b,c", "say \"hi\""], vec!["line1\nline2", "", "z"]]);
    }

    #[test]
    fn the_reader_keeps_a_last_row_with_no_line_ending_and_drops_blank_lines() {
        let rows = parse_csv("a,b\n\n\nc,d");
        assert_eq!(cells(&rows), vec![vec!["a", "b"], vec!["c", "d"]]);
    }

    #[test]
    fn a_quoted_empty_last_cell_is_a_cell() {
        assert_eq!(cells(&parse_csv("a,\"\"")), vec![vec!["a", ""]]);
    }

    #[test]
    fn a_byte_order_mark_does_not_end_up_in_the_first_column_name() {
        let parsed = parse(
            ImportSource::Browser,
            "\u{feff}name,url,username,password\nGitHub,https://github.com,me,pw\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.items.len(), 1);
    }

    // -- each source -------------------------------------------------------

    #[test]
    fn chrome_rows_become_logins_with_their_address() {
        let parsed = items(
            ImportSource::Browser,
            "name,url,username,password,note\nGitHub,https://github.com/login,me,hunter2,two factor\n",
        );
        let entry = &parsed.items[0];
        assert_eq!(entry.item.name, "GitHub");
        assert_eq!(login_of(entry).username.as_deref(), Some("me"));
        assert_eq!(login_of(entry).password.as_ref().map(|p| p.as_str()), Some("hunter2"));
        assert_eq!(login_of(entry).uris[0].uri.as_deref(), Some("https://github.com/login"));
        assert_eq!(entry.item.notes.as_ref().map(|n| n.as_str()), Some("two factor"));
        assert_eq!(entry.folder, None);
    }

    #[test]
    fn a_chrome_row_with_no_name_is_named_for_its_site() {
        let parsed = items(ImportSource::Browser, "name,url,username,password\n,https://www.example.com/x,me,pw\n");
        assert_eq!(parsed.items[0].item.name, "example.com");
    }

    #[test]
    fn a_table_with_no_credential_columns_is_not_a_password_export() {
        assert_eq!(
            parse(ImportSource::GenericCsv, b"city,population\nOslo,700000\n").err(),
            Some(ParseError::NotThisFormat)
        );
    }

    #[test]
    fn the_generic_reader_finds_columns_by_common_names() {
        let parsed = items(
            ImportSource::GenericCsv,
            "Account,Login URL,Login Username,Login Password,Folder\nBank,https://bank.test,ann,pw1,Money\n",
        );
        let entry = &parsed.items[0];
        assert_eq!(entry.item.name, "Bank");
        assert_eq!(login_of(entry).username.as_deref(), Some("ann"));
        assert_eq!(entry.folder.as_deref(), Some("Money"));
    }

    #[test]
    fn one_password_reads_otp_favourite_and_secure_notes() {
        let parsed = items(
            ImportSource::OnePassword,
            "Title,Url,Username,Password,OTPAuth,Favorite,Archived,Tags,Notes,Type\n\
             Mail,https://mail.test,me,pw,otpauth://totp/x?secret=ABC,true,false,,hello,Login\n\
             Wifi code,,,,,false,false,,the code is 42,Secure Note\n",
        );
        assert_eq!(parsed.items.len(), 2);
        let mail = &parsed.items[0];
        assert!(mail.item.favorite);
        assert_eq!(login_of(mail).totp.as_ref().map(|t| t.as_str()), Some("otpauth://totp/x?secret=ABC"));
        let note = &parsed.items[1];
        assert_eq!(note.item.item_type, Some(2));
        assert!(note.item.login.is_none());
        assert_eq!(note.item.notes.as_ref().map(|n| n.as_str()), Some("the code is 42"));
    }

    #[test]
    fn lastpass_notes_are_secure_notes_and_groups_are_nested_folders() {
        let parsed = items(
            ImportSource::LastPass,
            "url,username,password,totp,extra,name,grouping,fav\n\
             https://a.test,u,p,,,Site A,Work\\Clients,1\n\
             http://sn,,,,secret words,A note,,0\n",
        );
        let site = &parsed.items[0];
        assert_eq!(site.folder.as_deref(), Some("Work/Clients"));
        assert!(site.item.favorite);
        let note = &parsed.items[1];
        assert_eq!(note.item.item_type, Some(2));
        assert_eq!(note.item.notes.as_ref().map(|n| n.as_str()), Some("secret words"));
        assert!(note.item.login.is_none(), "the placeholder address is not an address");
    }

    #[test]
    fn keeper_has_no_header_and_its_totp_field_becomes_the_login_totp() {
        let parsed = items(
            ImportSource::Keeper,
            "Personal\\Banks,My bank,ann,pw,https://bank.test,n,,Pin,1234,TFC:Keeper,otpauth://totp/b?secret=Z\n",
        );
        let entry = &parsed.items[0];
        assert_eq!(entry.folder.as_deref(), Some("Personal/Banks"));
        assert_eq!(entry.item.name, "My bank");
        assert_eq!(login_of(entry).totp.as_ref().map(|t| t.as_str()), Some("otpauth://totp/b?secret=Z"));
        assert_eq!(entry.item.fields.len(), 1);
        assert_eq!(entry.item.fields[0].name.as_deref(), Some("Pin"));
    }

    #[test]
    fn bitwarden_csv_reads_logins_and_notes_and_custom_fields() {
        let parsed = items(
            ImportSource::Bitwarden,
            "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
             Work,1,login,Git,,\"Pin: 9\nOther: x\",0,\"https://g.test\nhttps://h.test\",me,pw,\n\
             ,0,note,Memo,remember,,0,,,,\n\
             ,0,card,Visa,,,0,,,,\n",
        );
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.left_out, 1, "a card row is left out and counted");
        let login = &parsed.items[0];
        assert_eq!(login.folder.as_deref(), Some("Work"));
        assert_eq!(login_of(login).uris.len(), 2);
        assert_eq!(login.item.fields.len(), 2);
        assert_eq!(parsed.items[1].item.item_type, Some(2));
    }

    #[test]
    fn bitwarden_json_keeps_every_type_and_files_by_folder_name() {
        let parsed = items(
            ImportSource::Bitwarden,
            r#"{"encrypted":false,"folders":[{"id":"f1","name":"Money"}],"items":[
              {"id":"1","organizationId":null,"folderId":"f1","type":1,"reprompt":0,"name":"Bank","notes":null,"favorite":true,
               "login":{"username":"u","password":"p","totp":null,"uris":[{"match":null,"uri":"https://b.test"}]},
               "collectionIds":null,"revisionDate":"2026-01-01T00:00:00Z"},
              {"id":"2","folderId":null,"type":3,"name":"Visa","card":{"cardholderName":"A","number":"4111","code":"123","expMonth":"1","expYear":"2030","brand":"Visa"}},
              {"id":"3","folderId":null,"type":4,"name":"Me","identity":{"firstName":"A"}},
              {"id":"4","folderId":null,"type":2,"name":"Memo","notes":"x","secureNote":{"type":0}},
              {"id":"5","folderId":null,"type":1,"name":"Old","deletedDate":"2026-01-01T00:00:00Z","login":{"username":"x"}},
              {"id":"6","folderId":null,"type":9,"name":"Future"}
            ]}"#,
        );
        assert_eq!(parsed.items.len(), 4);
        assert_eq!(parsed.left_out, 2, "a trashed item and a type this app does not know");
        let bank = &parsed.items[0];
        assert_eq!(bank.folder.as_deref(), Some("Money"));
        assert!(bank.item.id.is_empty());
        assert!(bank.item.other.get("revisionDate").is_none(), "the old vault's bookkeeping is gone");
        assert!(bank.item.favorite);
        assert_eq!(parsed.items[1].item.card.as_ref().and_then(|c| c.number.as_ref()).map(|n| n.as_str()), Some("4111"));
        assert!(parsed.items[2].item.identity.is_some());
    }

    #[test]
    fn an_encrypted_bitwarden_export_is_refused_with_a_reason() {
        let refused = parse(ImportSource::Bitwarden, br#"{"encrypted":true,"encKeyValidation_DO_NOT_EDIT":"x","data":"y"}"#);
        assert_eq!(refused.err(), Some(ParseError::Encrypted));
    }

    #[test]
    fn a_file_of_the_wrong_kind_is_refused() {
        assert_eq!(parse(ImportSource::Bitwarden, b"{not json").err(), Some(ParseError::NotThisFormat));
        assert_eq!(parse(ImportSource::LastPass, b"name,url,username,password\nA,b,c,d\n").err(), Some(ParseError::NotThisFormat));
        assert_eq!(parse(ImportSource::Browser, b"   \n").err(), Some(ParseError::Empty));
        assert_eq!(parse(ImportSource::Browser, &[0xff, 0xfe, 0x00]).err(), Some(ParseError::NotThisFormat));
        assert_eq!(parse(ImportSource::Browser, b"name,url,username,password\n").err(), Some(ParseError::NothingToImport));
    }

    // -- duplicates --------------------------------------------------------

    fn existing(name: &str, user: &str, uri: &str, password: &str) -> VaultItem {
        let parsed = items(
            ImportSource::Browser,
            &format!("name,url,username,password\n{name},{uri},{user},{password}\n"),
        );
        let mut item = parsed.items.into_iter().next().unwrap().item;
        item.id = "existing".into();
        item
    }

    #[test]
    fn what_the_vault_already_holds_is_skipped() {
        let parsed = items(
            ImportSource::Browser,
            "name,url,username,password\nGitHub,https://www.github.com/login,Me,pw\nNew,https://n.test,me,pw\n",
        );
        let vault = [existing("github", "me", "https://github.com", "pw")];
        let plan = plan(parsed, &vault);
        assert_eq!(plan.duplicates, 1);
        assert_eq!(plan.to_create.len(), 1);
        assert_eq!(plan.to_create[0].item.name, "New");
    }

    #[test]
    fn the_same_site_and_user_with_another_password_is_not_a_duplicate() {
        let parsed = items(ImportSource::Browser, "name,url,username,password\nGitHub,https://github.com,me,new-pw\n");
        let vault = [existing("GitHub", "me", "https://github.com", "old-pw")];
        let plan = plan(parsed, &vault);
        assert_eq!(plan.duplicates, 0);
        assert_eq!(plan.to_create.len(), 1);
    }

    #[test]
    fn a_file_that_says_the_same_thing_twice_imports_it_once() {
        let parsed = items(
            ImportSource::Browser,
            "name,url,username,password\nA,https://a.test,u,p\nA,https://a.test,u,p\nA,https://a.test,u,other\n",
        );
        let plan = plan(parsed, &[]);
        assert_eq!(plan.duplicates, 1);
        assert_eq!(plan.to_create.len(), 2);
    }

    // -- creating ----------------------------------------------------------

    fn one_login(folder: Option<&str>) -> ImportItem {
        let mut entry = items(ImportSource::Browser, "name,url,username,password\nA,https://a.test,u,p\n").items.remove(0);
        entry.folder = folder.map(str::to_string);
        entry
    }

    #[test]
    fn the_create_body_is_a_login_in_the_chosen_folder_with_no_old_identity() {
        let body = new_item_for(&one_login(None), Some("f9")).unwrap().to_payload();
        assert_eq!(body["type"], 1);
        assert_eq!(body["folderId"], "f9");
        assert_eq!(body["login"]["username"], "u");
        assert_eq!(body["login"]["uris"][0]["uri"], "https://a.test");
        assert!(body.get("id").is_none());
        let unfiled = new_item_for(&one_login(None), None).unwrap().to_payload();
        assert!(unfiled["folderId"].is_null());
    }

    #[test]
    fn a_secure_note_body_carries_its_discriminator() {
        let parsed = items(ImportSource::Bitwarden, "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n,0,note,Memo,remember,,0,,,,\n");
        let body = new_item_for(&parsed.items[0], None).unwrap().to_payload();
        assert_eq!(body["type"], 2);
        assert_eq!(body["secureNote"]["type"], 0);
        assert_eq!(body["notes"], "remember");
    }

    #[test]
    fn a_card_and_an_ssh_key_from_bitwarden_json_make_valid_create_bodies() {
        let parsed = items(
            ImportSource::Bitwarden,
            r#"{"items":[{"id":"2","type":3,"name":"Visa","card":{"number":"4111"}},
                         {"id":"3","type":5,"name":"Key","sshKey":{"privateKey":"k","publicKey":"p","keyFingerprint":"f"}}]}"#,
        );
        for entry in &parsed.items {
            assert!(new_item_for(entry, None).is_ok());
        }
    }

    fn plan_of(entries: Vec<ImportItem>) -> Plan {
        Plan { to_create: entries, duplicates: 2, left_out: 1 }
    }

    #[test]
    fn running_creates_missing_folders_once_and_reuses_existing_ones() {
        let mut made = Vec::new();
        let mut created = Vec::new();
        let progress = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let report = run(
            plan_of(vec![one_login(Some("New")), one_login(Some("New")), one_login(Some("Old")), one_login(None)]),
            Sinks {
                existing_folders: vec![Folder { id: "old-id".into(), name: "Old".into(), other: Default::default() }],
                create_folder: &mut |name| {
                    made.push(name.to_string());
                    Ok(Folder { id: "new-id".into(), name: name.into(), other: Default::default() })
                },
                create_item: &mut |item| {
                    created.push(item.to_payload()["folderId"].clone());
                    Ok(())
                },
            },
            &progress,
            &cancel,
        );
        assert_eq!(made, vec!["New"]);
        assert_eq!(created, vec![serde_json::json!("new-id"), serde_json::json!("new-id"), serde_json::json!("old-id"), serde_json::Value::Null]);
        assert_eq!((report.imported, report.failed, report.duplicates, report.left_out), (4, 0, 2, 1));
        assert_eq!(progress.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn three_failures_in_a_row_stop_the_import_and_a_success_resets_the_count() {
        let progress = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let report = run(
            plan_of((0..8).map(|_| one_login(None)).collect()),
            Sinks {
                existing_folders: vec![],
                create_folder: &mut |_| unreachable!("no folders"),
                create_item: &mut |_| {
                    calls += 1;
                    // ok, fail, fail, ok, fail, fail, fail -> stop.
                    if matches!(calls, 1 | 4) { Ok(()) } else { Err(VaultError::Http("no".into())) }
                },
            },
            &progress,
            &cancel,
        );
        assert_eq!(calls, 7);
        assert_eq!((report.imported, report.failed), (2, 5));
        assert!(report.stopped_early);
        assert!(report.first_error.is_some());
    }

    #[test]
    fn cancelling_stops_before_the_next_item() {
        let progress = AtomicUsize::new(0);
        let cancel = AtomicBool::new(false);
        let report = run(
            plan_of((0..5).map(|_| one_login(None)).collect()),
            Sinks {
                existing_folders: vec![],
                create_folder: &mut |_| unreachable!("no folders"),
                create_item: &mut |_| {
                    cancel.store(true, Ordering::Relaxed);
                    Ok(())
                },
            },
            &progress,
            &cancel,
        );
        assert_eq!(report.imported, 1);
        assert!(report.stopped_early);
    }
}

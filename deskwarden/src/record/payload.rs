//! The [`Record`] type, its versioned JSON writer, and a strict reader.
//!
//! # Absence is meaningful
//!
//! A field the sender did not tick is **absent from the JSON**, not present
//! and empty. The two are not the same thing on the far side: an empty string
//! imports as a username of `""`, which renders as a blank the recipient never
//! chose and which, on a collision replace, overwrites something real with
//! nothing. `Option::None` is the wire representation of "not sent", and
//! `absence_is_meaningful_and_is_written_as_absence` is what holds it.
//!
//! # The reader is strict because the payload is untrusted
//!
//! This JSON was written by someone else and arrived over the network.
//! **Unknown fields are refused, not ignored** — ignoring them is exactly how a
//! payload written for a later version imports silently and wrongly, with a
//! future `"totp_plain"` dropped on the floor and the user told the import
//! succeeded. The size cap is enforced **before** parsing, so a hostile payload
//! is not walked at all. And it is a refusal, never a truncation: importing a
//! prefix of a record is importing a record nobody wrote.
//!
//! # Nothing here is an instruction
//!
//! `notes` is text to store. It is not a key sequence, a path or a URL, and no
//! code in the shipping half of this module hands any field to the crate's
//! sequence parser, to a shell, or to a browser — pinned, as an absence, by
//! `a_notes_field_that_looks_like_a_key_sequence_is_stored_as_text`.

use crate::record::seal::{self, SealedSeed};
use zeroize::Zeroizing;

/// What the payload calls itself. A payload that does not say what it is can
/// only fail to parse; one that does can be refused politely by name.
pub const RECORD_FORMAT: &str = "deskwarden.record";

/// The payload layout this build writes and the only one it reads.
pub const RECORD_VERSION: u32 = 1;

/// The largest payload this reader will look at, in bytes.
///
/// **A refusal, not a truncation.** A Send is fetched whole into memory, and a
/// multi-megabyte "record" is not a record.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// One credential record, as it travels.
///
/// Every optional field is `Option` for the reason in the module docs, and the
/// seed is [`SealedSeed`] rather than a `String` **by construction**: there is
/// no arm of this type in which a bare seed can be carried, so "we sent the
/// seed unsealed" is not a bug that can be written here.
#[derive(Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub username: Option<String>,
    pub password: Option<Zeroizing<String>>,
    pub uri: Option<String>,
    pub notes: Option<String>,
    /// Sealed by `seal.rs`, never the bare seed. `None` when not sent.
    pub totp_sealed: Option<SealedSeed>,
    /// Advisory only. RFC 3339. A vault item does not expire; this is
    /// staleness information about the record and must never be presented as
    /// an expiry that will be enforced.
    pub not_after: Option<String>,
}

/// Redacting, in the house style of `SendSummary` and `CreatedSend`.
///
/// A `Record` holds a password and a sealed seed, and a `{record:?}` in a log
/// line is how both would leave the process. **Which fields are present is
/// shown; no value is** — that is the same thing the import surface is allowed
/// to show the user before creating anything.
impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn present<T>(field: &Option<T>) -> &'static str {
            if field.is_some() {
                "<present, redacted>"
            } else {
                "<absent>"
            }
        }
        f.debug_struct("Record")
            .field("name", &format_args!("<{} chars redacted>", self.name.chars().count()))
            .field("username", &present(&self.username))
            .field("password", &present(&self.password))
            .field("uri", &present(&self.uri))
            .field("notes", &present(&self.notes))
            .field("totp_sealed", &present(&self.totp_sealed))
            .field("not_after", &present(&self.not_after))
            .finish()
    }
}

/// Why a payload was refused. Each arm is a sentence the surface can render;
/// a refusal that renders as a generic failure teaches the user to retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordRefusal {
    NotOurFormat,
    UnsupportedVersion(u32),
    UnknownField(String),
    MissingName,
    Malformed(&'static str),
    TooLarge,
}

impl RecordRefusal {
    /// A sentence naming the reason, for the import surface.
    pub fn sentence(&self) -> String {
        match self {
            Self::NotOurFormat => {
                "This Send does not contain a Deskwarden record.".to_string()
            }
            Self::UnsupportedVersion(v) => format!(
                "This record was written in format version {v}, which this version of \
                 Deskwarden cannot read."
            ),
            Self::UnknownField(key) => format!(
                "This record carries a field this version does not know ({key}), so importing \
                 it could quietly drop part of it."
            ),
            Self::MissingName => "This record has no name.".to_string(),
            Self::Malformed(what) => format!("This record could not be read: {what}."),
            Self::TooLarge => format!(
                "This Send is larger than a record can be ({} KiB), so it was not read.",
                MAX_PAYLOAD_BYTES / 1024
            ),
        }
    }
}

/// Writes `record` as the versioned JSON that travels in the Send.
///
/// **Hand-rolled rather than `serde_json::to_string`**, and deliberately:
/// `serde_json` would allocate an ordinary `String` for a body containing the
/// password and hand it back to the allocator unwiped. This pushes into one
/// pre-reserved [`Zeroizing`] buffer and allocates nothing else. The buffer is
/// reserved generously up front, because a `String` that reallocates while the
/// secret is already in it leaves the old block behind with the plaintext
/// still in it.
pub fn write_json(record: &Record) -> Zeroizing<String> {
    let sealed_len = record.totp_sealed.as_ref().map_or(0, |s| s.ciphertext.len() * 2 + 128);
    let mut json = Zeroizing::new(String::with_capacity(
        256 + record.name.len() * 6
            + record.username.as_ref().map_or(0, |s| s.len() * 6)
            + record.password.as_ref().map_or(0, |s| s.len() * 6)
            + record.uri.as_ref().map_or(0, |s| s.len() * 6)
            + record.notes.as_ref().map_or(0, |s| s.len() * 6)
            + record.not_after.as_ref().map_or(0, |s| s.len() * 6)
            + sealed_len,
    ));
    let out: &mut String = &mut json;

    out.push_str("{\"format\":");
    push_json_string(out, RECORD_FORMAT);
    out.push_str(",\"version\":");
    push_u32(out, RECORD_VERSION);
    out.push_str(",\"name\":");
    push_json_string(out, &record.name);

    // Each of these emits the key only when the value is there. The `if let`
    // is the whole of "absence is meaningful": there is no `else` arm writing
    // an empty string, and adding one would be the defect.
    for (key, value) in [
        ("username", record.username.as_deref()),
        ("password", record.password.as_deref().map(String::as_str)),
        ("uri", record.uri.as_deref()),
        ("notes", record.notes.as_deref()),
        ("not_after", record.not_after.as_deref()),
    ] {
        if let Some(value) = value {
            out.push_str(",\"");
            out.push_str(key);
            out.push_str("\":");
            push_json_string(out, value);
        }
    }

    if let Some(sealed) = &record.totp_sealed {
        out.push_str(",\"totp_sealed\":{\"v\":");
        push_u32(out, seal::SEAL_VERSION);
        out.push_str(",\"kdf\":");
        push_json_string(out, seal::SEAL_KDF);
        out.push_str(",\"salt\":\"");
        seal::base64_into(out, &sealed.salt);
        out.push_str("\",\"nonce\":\"");
        seal::base64_into(out, &sealed.nonce);
        out.push_str("\",\"ct\":\"");
        seal::base64_into(out, &sealed.ciphertext);
        out.push_str("\"}");
    }

    out.push('}');
    json
}

/// Appends `s` to `out` as a JSON string literal, quotes included.
///
/// A twin of `send.rs`'s function of the same name, which is private there.
/// Same reason for existing: no `format!`, no intermediate `String`, nothing
/// allocated outside the caller's wiped buffer.
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(HEX[(c as usize >> 4) & 0xf] as char);
                out.push(HEX[c as usize & 0xf] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Digits straight into the caller's buffer. Not `to_string`, for the reason
/// [`push_json_string`] is not `format!`.
fn push_u32(out: &mut String, mut n: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}

/// Reads a payload, strictly. See the module docs for why every refusal here
/// is a refusal rather than a best effort.
pub fn read_json(text: &str) -> Result<Record, RecordRefusal> {
    // **Before parsing, and that is the point.** Checking the size after
    // `serde_json` has walked the document would have already done the work
    // this cap exists to refuse.
    if text.len() > MAX_PAYLOAD_BYTES {
        return Err(RecordRefusal::TooLarge);
    }

    let value: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|_| RecordRefusal::Malformed("it is not JSON"))?;
    let serde_json::Value::Object(mut map) = value else {
        return Err(RecordRefusal::Malformed("it is not a JSON object"));
    };

    match map.remove("format") {
        Some(serde_json::Value::String(s)) if s == RECORD_FORMAT => {}
        _ => return Err(RecordRefusal::NotOurFormat),
    }
    let version = match map.remove("version") {
        Some(serde_json::Value::Number(n)) if n.is_u64() => n.as_u64().unwrap_or(u64::MAX),
        _ => return Err(RecordRefusal::Malformed("it does not say which version it is")),
    };
    if version != u64::from(RECORD_VERSION) {
        return Err(RecordRefusal::UnsupportedVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }

    let name = match map.remove("name") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s,
        Some(serde_json::Value::String(_)) | None => return Err(RecordRefusal::MissingName),
        Some(_) => return Err(RecordRefusal::Malformed("its name is not text")),
    };

    let username = take_string(&mut map, "username")?;
    // Moved out of the parsed document rather than cloned out of it, so the
    // one `String` `serde_json` allocated for the password is the one that
    // gets wiped. A `.as_str().to_string()` here would leave the original
    // behind inside the `Value` for the allocator to hand on unwiped.
    let password = take_string(&mut map, "password")?.map(Zeroizing::new);
    let uri = take_string(&mut map, "uri")?;
    let notes = take_string(&mut map, "notes")?;
    let not_after = take_string(&mut map, "not_after")?;
    let totp_sealed = match map.remove("totp_sealed") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(read_sealed(&value)?),
    };

    // Every key this version knows has been removed above, so whatever is
    // left is a key this version does NOT know -- and it is refused rather
    // than ignored. See the module docs.
    if let Some(key) = map.keys().next() {
        return Err(RecordRefusal::UnknownField(key.clone()));
    }

    Ok(Record { name, username, password, uri, notes, totp_sealed, not_after })
}

/// Takes an optional string field out of the document by value.
///
/// An explicit `null` reads as absent; anything that is not text is a refusal
/// rather than a coercion, because `"username": 7` is not a username.
fn take_string(
    map: &mut serde_json::Map<String, serde_json::Value>,
    key: &'static str,
) -> Result<Option<String>, RecordRefusal> {
    match map.remove(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(RecordRefusal::Malformed("one of its fields is not text")),
    }
}

/// Reads a sealed seed. Its own version and KDF name are checked here, so a
/// future KDF change is a refusal rather than a wrong key derived in silence
/// from the right passphrase.
fn read_sealed(value: &serde_json::Value) -> Result<SealedSeed, RecordRefusal> {
    let serde_json::Value::Object(map) = value else {
        return Err(RecordRefusal::Malformed("its sealed seed is not an object"));
    };
    match map.get("v").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(seal::SEAL_VERSION) => {}
        _ => return Err(RecordRefusal::Malformed("its sealed seed is of an unknown version")),
    }
    match map.get("kdf").and_then(serde_json::Value::as_str) {
        Some(kdf) if kdf == seal::SEAL_KDF => {}
        _ => return Err(RecordRefusal::Malformed("its sealed seed uses an unknown key derivation")),
    }
    // Unknown keys inside the seal are refused too, on the same reasoning as
    // the outer document: five known keys, and anything else means this
    // payload was written for a reader that is not us.
    const KNOWN: [&str; 5] = ["v", "kdf", "salt", "nonce", "ct"];
    if let Some(key) = map.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(RecordRefusal::UnknownField(key.clone()));
    }

    let bytes = |key: &str| -> Result<Vec<u8>, RecordRefusal> {
        map.get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(seal::base64_from)
            .ok_or(RecordRefusal::Malformed("its sealed seed is not readable"))
    };
    let salt: [u8; 16] = bytes("salt")?
        .try_into()
        .map_err(|_| RecordRefusal::Malformed("its sealed seed has the wrong salt length"))?;
    let nonce: [u8; 12] = bytes("nonce")?
        .try_into()
        .map_err(|_| RecordRefusal::Malformed("its sealed seed has the wrong nonce length"))?;
    let ciphertext = bytes("ct")?;
    if ciphertext.is_empty() {
        return Err(RecordRefusal::Malformed("its sealed seed is empty"));
    }
    Ok(SealedSeed { salt, nonce, ciphertext })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(name: &str) -> Record {
        Record {
            name: name.to_string(),
            username: None,
            password: None,
            uri: None,
            notes: None,
            totp_sealed: None,
            not_after: None,
        }
    }

    #[test]
    fn absence_is_meaningful_and_is_written_as_absence() {
        // The spec: "an unticked field is absent, not empty". An empty string
        // would import as a username of "", silently rendering as a blank the
        // user did not choose.
        let record = Record { username: Some("dplatonov".to_string()), ..bare("SAP Production") };
        let json = write_json(&record);
        assert!(json.contains("\"username\":\"dplatonov\""), "{}", json.as_str());
        assert!(
            !json.contains("\"password\""),
            "an unsent field must be ABSENT from the JSON, not present-and-empty: {}",
            json.as_str()
        );
        // Control: the writer really does emit a password when there is one,
        // so the assertion above is about absence and not about a writer that
        // never writes passwords at all.
        let with_password =
            Record { password: Some(Zeroizing::new("hunter2".into())), ..record.clone() };
        assert!(write_json(&with_password).contains("\"password\":\"hunter2\""));
        // And the same control on every other optional field, so "absent"
        // cannot be the writer's answer to everything.
        for (json, needle) in [
            (write_json(&Record { uri: Some("https://x".into()), ..record.clone() }), "\"uri\""),
            (write_json(&Record { notes: Some("n".into()), ..record.clone() }), "\"notes\""),
            (
                write_json(&Record { not_after: Some("2026-09-01T00:00:00Z".into()), ..record }),
                "\"not_after\"",
            ),
        ] {
            assert!(json.contains(needle), "the writer never emits {needle}: {}", json.as_str());
        }
    }

    #[test]
    fn the_envelope_names_the_format_and_the_version() {
        // A payload that does not say what it is cannot be refused politely by
        // a future reader; it can only fail to parse.
        let json = write_json(&bare("x"));
        assert!(json.contains("\"format\":\"deskwarden.record\""), "{}", json.as_str());
        assert!(json.contains("\"version\":1"), "{}", json.as_str());
    }

    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        // Ignoring unknown keys is how a payload written for a LATER version
        // imports silently and wrongly here: a future `"totp_plain"` would be
        // dropped on the floor and the user told the import succeeded.
        let json = r#"{"format":"deskwarden.record","version":1,"name":"x","surprise":true}"#;
        assert_eq!(
            read_json(json),
            Err(RecordRefusal::UnknownField("surprise".to_string()))
        );
        let future = r#"{"format":"deskwarden.record","version":1,"name":"x","totp_plain":"JBSW"}"#;
        assert_eq!(
            read_json(future),
            Err(RecordRefusal::UnknownField("totp_plain".to_string())),
            "the exact shape this refusal exists for"
        );
    }

    #[test]
    fn a_payload_from_somewhere_else_is_refused_by_name() {
        assert_eq!(
            read_json(r#"{"format":"something.else","version":1,"name":"x"}"#),
            Err(RecordRefusal::NotOurFormat)
        );
        assert_eq!(
            read_json(r#"{"format":"deskwarden.record","version":99,"name":"x"}"#),
            Err(RecordRefusal::UnsupportedVersion(99))
        );
        assert_eq!(
            read_json(r#"{"version":1,"name":"x"}"#),
            Err(RecordRefusal::NotOurFormat),
            "a payload with no format at all is not ours either"
        );
        assert_eq!(
            read_json(r#"{"format":"deskwarden.record","version":1}"#),
            Err(RecordRefusal::MissingName)
        );
        assert_eq!(read_json("not json at all"), Err(RecordRefusal::Malformed("it is not JSON")));
        assert_eq!(
            read_json(r#"["a"]"#),
            Err(RecordRefusal::Malformed("it is not a JSON object"))
        );
    }

    #[test]
    fn a_record_survives_a_round_trip_with_every_field_set() {
        // The control that makes the refusal tests meaningful: a reader that
        // refused EVERYTHING would pass all of them.
        let original = Record {
            name: "SAP Production".to_string(),
            username: Some("dplatonov".to_string()),
            password: Some(Zeroizing::new("hunter2".to_string())),
            uri: Some("https://sap.example".to_string()),
            notes: Some("line one\nline two \"quoted\"\ttabbed".to_string()),
            totp_sealed: Some(seal::seal("JBSWY3DPEHPK3PXP", "pw")),
            not_after: Some("2026-09-01T00:00:00Z".to_string()),
        };
        let back = read_json(&write_json(&original)).expect("round trip");
        assert_eq!(back.name, original.name);
        assert_eq!(back.username, original.username);
        assert_eq!(back.password.as_deref(), original.password.as_deref());
        assert_eq!(back.uri, original.uri);
        assert_eq!(back.notes, original.notes);
        assert_eq!(back.not_after, original.not_after);
        // The seal survives byte-for-byte, and still opens -- a `SealedSeed`
        // that round-tripped as equal but no longer decrypted would be a
        // base64 bug this equality alone would not catch.
        assert_eq!(back.totp_sealed, original.totp_sealed);
        let opened = seal::unseal(back.totp_sealed.as_ref().expect("sealed"), "pw").expect("opens");
        assert_eq!(opened.as_str(), "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn an_oversized_payload_is_refused_before_it_is_parsed() {
        // A Send is fetched into memory. A multi-megabyte "record" is not a
        // record; refusing early keeps a hostile payload from being walked.
        let huge = format!(
            r#"{{"format":"deskwarden.record","version":1,"name":"{}"}}"#,
            "x".repeat(200_000)
        );
        assert_eq!(read_json(&huge), Err(RecordRefusal::TooLarge));
        // Control: the same document just under the cap is read, so `TooLarge`
        // is about the size and not about a reader that refuses long names.
        let big = format!(
            r#"{{"format":"deskwarden.record","version":1,"name":"{}"}}"#,
            "x".repeat(1_000)
        );
        assert!(big.len() < MAX_PAYLOAD_BYTES);
        assert_eq!(read_json(&big).expect("under the cap").name.len(), 1_000);
    }

    #[test]
    fn a_sealed_seed_of_an_unknown_shape_is_refused() {
        let good = write_json(&Record {
            totp_sealed: Some(seal::seal("JBSWY3DPEHPK3PXP", "pw")),
            ..bare("x")
        });
        // Control: the good one reads.
        assert!(read_json(&good).expect("reads").totp_sealed.is_some());

        let bumped = good.replace("\"v\":1", "\"v\":2");
        assert_eq!(
            read_json(&bumped),
            Err(RecordRefusal::Malformed("its sealed seed is of an unknown version"))
        );
        let other_kdf = good.replace("argon2id", "sha256");
        assert_eq!(
            read_json(&other_kdf),
            Err(RecordRefusal::Malformed("its sealed seed uses an unknown key derivation"))
        );
        let extra = good.replace("\"kdf\"", "\"extra\":1,\"kdf\"");
        assert_eq!(read_json(&extra), Err(RecordRefusal::UnknownField("extra".to_string())));
    }

    #[test]
    fn a_notes_field_that_looks_like_a_key_sequence_is_stored_as_text() {
        // "No field is an instruction", made checkable. A sender can write
        // anything into `notes`; what must never happen is this module -- or
        // anything reading a `Record` -- treating that text as a sequence to
        // type into whatever window happens to be in front.
        const NOTES: &str = "recovery: {PASSWORD}{TAB}{TOTP}{ENTER}";
        let record = Record { notes: Some(NOTES.to_string()), ..bare("x") };
        let back = read_json(&write_json(&record)).expect("round trip");
        assert_eq!(
            back.notes.as_deref(),
            Some(NOTES),
            "the notes came back changed, so something interpreted them"
        );

        // Control: the needle really IS a sequence a parser would interpret,
        // so "stored as text" is a claim about this path and not about a
        // string that happens to be inert.
        let tokens = crate::key_sequence::parse(NOTES);
        assert!(
            tokens.iter().any(|t| matches!(
                t,
                crate::key_sequence::Token::Field(crate::key_sequence::FieldRef::Password)
            )),
            "the control string is not one `key_sequence` interprets: {tokens:?}"
        );

        // And the shipping half of this module never hands anything to
        // `key_sequence` at all -- a source pin, because the property is an
        // absence and no round trip can show an absence.
        let whole = include_str!("payload.rs").replace("\r\n", "\n");
        let code = whole.split(concat!("#[cfg(test)]", "\nmod ")).next().unwrap();
        assert!(
            code.len() < whole.len(),
            "the test-module marker was not found, so the pin below reads the whole file and \
             would fail on this very test's own text"
        );
        assert!(
            code.contains("pub fn read_json"),
            "control: the shipping half was not found, so the absence below is vacuous"
        );
        assert!(
            !code.contains(concat!("key_sequence", "::")),
            "the payload module reached for `key_sequence`. No field of a record written by \
             somebody else is a sequence to type."
        );
    }

    #[test]
    fn a_refusal_renders_as_a_sentence_that_names_the_reason() {
        for refusal in [
            RecordRefusal::NotOurFormat,
            RecordRefusal::UnsupportedVersion(99),
            RecordRefusal::UnknownField("surprise".to_string()),
            RecordRefusal::MissingName,
            RecordRefusal::Malformed("it is not JSON"),
            RecordRefusal::TooLarge,
        ] {
            let sentence = refusal.sentence();
            assert!(sentence.ends_with('.'), "{refusal:?} -> {sentence:?}");
            assert!(sentence.len() > 20, "{refusal:?} -> {sentence:?}");
        }
        // The two that carry a value must SAY it, or the sentence is generic
        // in the way this method exists to prevent.
        assert!(RecordRefusal::UnsupportedVersion(99).sentence().contains("99"));
        assert!(RecordRefusal::UnknownField("surprise".into()).sentence().contains("surprise"));
    }

    #[test]
    fn the_debug_of_a_record_prints_which_fields_are_present_and_no_value() {
        let record = Record {
            username: Some("dplatonov".to_string()),
            password: Some(Zeroizing::new("hunter2".to_string())),
            ..bare("SAP Production")
        };
        let shown = format!("{record:?}");
        assert!(!shown.contains("hunter2"), "the password reached a log line: {shown}");
        assert!(!shown.contains("dplatonov"), "the username reached a log line: {shown}");
        assert!(!shown.contains("SAP"), "the name reached a log line: {shown}");
        // Control: it says which fields are there, which is what the import
        // surface is allowed to show.
        assert!(shown.contains("password: \"<present, redacted>\""), "{shown}");
        assert!(shown.contains("uri: \"<absent>\""), "{shown}");
    }

    /// The allocator probe over the writer's buffer.
    ///
    /// **The control is asserted first**, as every probe test in this crate
    /// does. `write_json` is exactly the place `serde_json::to_string` would
    /// have been the obvious choice and would have been wrong.
    ///
    /// Shown able to fail before it was trusted: returning
    /// `Zeroizing::new(json.to_string())` out of an ordinary `String` builder
    /// reds the second assertion.
    #[test]
    fn the_written_payload_does_not_reach_the_allocator_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        let bare_probe = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare_probe)),
            "control: the allocator probe did not see a plain `String` carrying the probe go \
             back to the allocator, so the verdict below is meaningless"
        );

        let password = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        let record = Record { password: Some(Zeroizing::new(password)), ..bare("SAP Production") };
        assert!(
            !plaintext_reached_the_allocator(move || {
                let json = write_json(&record);
                assert!(json.contains(PROBE), "control: the password really is in the payload");
                drop(json);
                drop(record);
            }),
            "the JSON payload went back to the allocator with the password still in it"
        );
    }
}

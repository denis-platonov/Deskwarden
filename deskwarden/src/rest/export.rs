//! One `encrypted_json` vault export, built here instead of by `bw export`.
//!
//! `crate::vault_export` is not edited by this module and does not know it
//! exists beyond one branch. The two meet where every other REST/CLI pair in
//! this crate meets -- at a `backend_policy::selected()` read inside the one
//! helper that produces the production runner -- exactly as
//! `vault_window`'s `real_send_*` helpers do.
//!
//! # What is produced, and where the shape came from
//!
//! **The format is `bw export --format encrypted_json` with no
//! `--password`**: the account-key-encrypted individual-vault export. It is
//! the only format `crate::vault_export` has ever offered, and that module's
//! own docs give the Windows reasons why.
//!
//! The shape below was **read off the installed CLI**, not invented and not
//! remembered. `bw.exe` (2026.7.0, the signature-checked binary at
//! `bw_path`'s own location) embeds the TypeScript sources of
//! `IndividualVaultExportService` and of every
//! `libs/common/src/models/export/*.export.ts` DTO as literal strings; the
//! document below is a transcription of `getEncryptedExport` and of the
//! `build` methods it calls. The same code was independently read from
//! `bitwarden/clients` at
//! `libs/tools/export-vault-core/src/services/individual-vault-export.service.ts`
//! and the two agree.
//!
//! ```text
//! {
//!   "encrypted": true,
//!   "encKeyValidation_DO_NOT_EDIT": "2.<iv>|<ct>|<mac>",
//!   "folders": [ { "id": .., "name": <EncString> } ],
//!   "items":   [ { .. CipherWithIdExport .. } ]
//! }
//! ```
//!
//! # Why this needs no new cryptography, and it really does not
//!
//! `getEncryptedExport` reads the **domain** objects -- `folders$` and
//! `cipherService.getAll`, not the `View` streams -- and every DTO field is
//! filled by `safeGetString`, which for an `EncString` hands back
//! `.encryptedString` verbatim. Nothing is decrypted and nothing is
//! re-encrypted, **including a cipher that carries its own key**:
//! `CipherExport.build` does `if ("key" in o) { this.key = o.key
//! ?.encryptedString }`, so the wrapped cipher key rides through and the
//! item's fields stay under it. (The *plaintext* `json` export is the one
//! that does `delete cipher.key`, which is how you can tell the two apart.)
//!
//! So this module is a **projection of the ciphertext the server already
//! sent**. The one and only encryption it performs is
//! `encKeyValidation_DO_NOT_EDIT`, which the CLI builds as
//! `encryptService.encryptString(Utils.newGuid(), userKey)` -- a fresh random
//! GUID under the user key, whose plaintext is never read again. The
//! importer's only use of it is
//! `await this.encryptService.decryptString(encKeyValidation, key)` inside a
//! `try`/`catch`: it proves the file belongs to the key, and nothing else.
//!
//! **Nothing in the produced document is vault plaintext.** That is not a
//! convenience, it is the security property that lets this path match the
//! CLI's: `bw export` wrote an encrypted archive without this process ever
//! holding the vault in the clear, and so does this.
//!
//! # What is left out, matching the CLI item for item
//!
//! * **Trashed items** -- `deletedDate != null` is filtered before the
//!   forEach.
//! * **Organisation-owned items** -- `if (c.organizationId != null) return;`.
//!   An organisation export is `--organizationid`, a different document and a
//!   different consent conversation, and `vault_export` does not offer it on
//!   either backend.
//! * **Attachments** -- `encrypted_json` carries none on either backend. Only
//!   `--format zip` does, and that format is *decrypted*.
//! * **`collectionIds`** -- forced to `null` by the CLI on every exported
//!   item, so forced to `null` here.
//! * **Folders with no id.**
//!
//! One filter is **not** matched, and it is written here rather than left to
//! be discovered: the CLI also drops ciphers rejected by
//! `restrictedItemTypesService`, which is an *organisation policy* ("members
//! may not store cards", and so on) delivered on a config endpoint this crate
//! does not read. A vault under such a policy would export one or more items
//! here that `bw export` would have omitted. Nothing is added that the user
//! does not own; the file is a superset, never a subset.
//!
//! # Key order
//!
//! Alphabetical, because `serde_json`'s `Map` is a `BTreeMap` in this crate's
//! feature set, where the CLI's is insertion-ordered by a `build` method
//! whose comment says it exists "so that we can control order of JSON
//! stringify for pretty print". That is a difference in the *pretty print*
//! and in nothing else: `BitwardenEncryptedJsonImporter` parses the document
//! into DTOs by field name, so no reader can observe it. The indentation is
//! two spaces, which is `JSON.stringify(jsonDoc, null, "  ")`.

use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::{encrypt, CryptoError};
use crate::rest::sync::{SyncResponse, VaultKeys};

/// The key whose presence tells an importer this file is key-checked, and one
/// of the two markers `vault_export::classify` looks for in the head of the
/// written file. Spelled once, here, because it is a wire name.
pub const VALIDATION_KEY: &str = "encKeyValidation_DO_NOT_EDIT";

/// The fields of `CipherWithIdExport` that are taken from the wire cipher
/// verbatim, whatever their value -- `null` included.
///
/// `collectionIds` is deliberately absent: the CLI *overwrites* it with
/// `null` rather than carrying the server's, so it is written separately
/// below and cannot be picked up from the wire by accident.
const ITEM_KEYS: [&str; 11] = [
    "id",
    "organizationId",
    "folderId",
    "type",
    "reprompt",
    "name",
    "notes",
    "favorite",
    "creationDate",
    "revisionDate",
    "archivedDate",
];

/// `CipherExport`'s one field that exists only when the cipher has it: the
/// wrapped per-cipher key. Absent, rather than `null`, when there is none --
/// `JSON.stringify` drops an `undefined` and the CLI never assigns it.
const ITEM_OPTIONAL_KEYS: [&str; 1] = ["key"];

/// `LoginExport`'s own fields. Whitelisted rather than passed through because
/// the wire login also carries `passwordRevisionDate`, which no export DTO
/// has.
const LOGIN_KEYS: [&str; 3] = ["username", "password", "totp"];

/// `LoginUriExport`.
const URI_KEYS: [&str; 3] = ["uri", "uriChecksum", "match"];

/// `FieldExport`.
const FIELD_KEYS: [&str; 4] = ["name", "value", "type", "linkedId"];

/// `PasswordHistoryExport`.
const HISTORY_KEYS: [&str; 2] = ["password", "lastUsedDate"];

/// `CardExport`.
const CARD_KEYS: [&str; 6] =
    ["cardholderName", "brand", "number", "expMonth", "expYear", "code"];

/// `IdentityExport`.
const IDENTITY_KEYS: [&str; 18] = [
    "title",
    "firstName",
    "middleName",
    "lastName",
    "address1",
    "address2",
    "address3",
    "city",
    "state",
    "postalCode",
    "country",
    "company",
    "email",
    "phone",
    "ssn",
    "username",
    "passportNumber",
    "licenseNumber",
];

/// `SshKeyExport`.
const SSH_KEY_KEYS: [&str; 3] = ["privateKey", "publicKey", "keyFingerprint"];

/// `SecureNoteExport`, which is one number and is never encrypted.
const SECURE_NOTE_KEYS: [&str; 1] = ["type"];

/// The type-specific sub-objects, and how each is carried.
///
/// The first five are whitelisted against the DTO read out of the CLI. The
/// last three -- `bankAccount`, `driversLicense`, `passport`, all new in the
/// 2026 item types -- are carried **whole**, because their DTO field lists
/// were not established from a primary source and a guessed whitelist would
/// silently drop a field the user typed. Carrying the server's object is the
/// direction that cannot lose data; the cost is that a field Bitwarden does
/// not model would ride along, which an importer ignores.
const TYPED_SECTIONS: [(&str, Option<&[&str]>); 8] = [
    ("login", None), // handled by `export_login`, which has a nested array
    ("secureNote", Some(&SECURE_NOTE_KEYS)),
    ("card", Some(&CARD_KEYS)),
    ("identity", Some(&IDENTITY_KEYS)),
    ("sshKey", Some(&SSH_KEY_KEYS)),
    ("bankAccount", None),
    ("driversLicense", None),
    ("passport", None),
];

/// Why an export could not be produced.
///
/// Two variants and not a string, because the two mean different things to
/// the user: one is "unlock and try again", the other is "it did not work and
/// here is why". `vault_export::ExportOutcome` already draws the same line.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportProblem {
    /// There is no usable session for the active account: no credentials, no
    /// login, or the server refused the token.
    Locked,
    /// Anything else, with the sentence to show.
    Failed(String),
}

impl ExportProblem {
    /// One [`RestError`], in the vocabulary above.
    fn from_rest(error: RestError) -> Self {
        match error {
            RestError::Unauthorized | RestError::NoRefreshToken => Self::Locked,
            RestError::Transport(_) => Self::Failed(
                "Bitwarden's server could not be reached, so nothing was exported.".to_string(),
            ),
            other => Self::Failed(format!("Bitwarden would not do it: {other}")),
        }
    }
}

// ---- the document ------------------------------------------------------------

/// The whole `encrypted_json` document for one sync payload. **Pure**: no
/// I/O, no clock, and the validation plaintext is a parameter.
///
/// `validation_plaintext` is what the CLI spells `Utils.newGuid()`. It is a
/// parameter rather than generated here for one reason and it is not
/// testability alone: a function that generates its own randomness cannot be
/// asserted to have *used* it. [`export_document`] is the caller that mints a
/// real GUID, and it is one line.
///
/// # Errors
///
/// Only [`CryptoError`], and only from the single `encrypt` call. Every other
/// value in the document is copied.
pub fn encrypted_json(
    response: &SyncResponse,
    keys: &VaultKeys,
    validation_plaintext: &str,
) -> Result<Zeroizing<String>, CryptoError> {
    let validation = encrypt(keys.user(), validation_plaintext.as_bytes())?;

    let mut doc = Map::new();
    doc.insert("encrypted".to_string(), Value::Bool(true));
    doc.insert(VALIDATION_KEY.to_string(), Value::String(validation.to_string()));
    doc.insert(
        "folders".to_string(),
        Value::Array(response.folders.iter().filter_map(export_folder).collect()),
    );
    doc.insert(
        "items".to_string(),
        Value::Array(response.ciphers.iter().filter_map(export_item).collect()),
    );

    let text = serde_json::to_string_pretty(&Value::Object(doc))
        .map_err(|_| CryptoError::Malformed("the export document could not be serialized"))?;
    Ok(Zeroizing::new(text))
}

/// One `FolderWithIdExport`: an id and a name, and nothing else at all -- not
/// even `revisionDate`, which the wire folder carries and the DTO does not.
///
/// `None` for a value that is not an object, or whose `id` is missing or
/// empty. That is the CLI's `if (!f.id) { return; }`, which also skips the
/// empty string.
fn export_folder(raw: &Value) -> Option<Value> {
    let object = raw.as_object()?;
    let id = object.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
    let mut out = Map::new();
    out.insert("id".to_string(), Value::String(id.to_string()));
    out.insert(
        "name".to_string(),
        object.get("name").cloned().unwrap_or_else(|| Value::String(String::new())),
    );
    Some(Value::Object(out))
}

/// One `CipherWithIdExport`, or `None` for a cipher this export does not
/// carry.
///
/// The three refusals are the CLI's, in its order: not an object or no `id`
/// (there is nothing to write), in the trash, or owned by an organisation.
fn export_item(raw: &Value) -> Option<Value> {
    let object = raw.as_object()?;
    object.get("id").and_then(Value::as_str).filter(|id| !id.is_empty())?;
    if !object.get("deletedDate").unwrap_or(&Value::Null).is_null() {
        return None;
    }
    if !object.get("organizationId").unwrap_or(&Value::Null).is_null() {
        return None;
    }

    let mut out = Map::new();
    for key in ITEM_KEYS {
        out.insert(key.to_string(), object.get(key).cloned().unwrap_or(Value::Null));
    }
    for key in ITEM_OPTIONAL_KEYS {
        if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
            out.insert(key.to_string(), value.clone());
        }
    }
    // Overwritten, never carried: `cipher.collectionIds = null`.
    out.insert("collectionIds".to_string(), Value::Null);

    if let Some(fields) = object.get("fields").and_then(Value::as_array) {
        out.insert("fields".to_string(), project_each(fields, &FIELD_KEYS));
    }
    if let Some(history) = object.get("passwordHistory").and_then(Value::as_array) {
        out.insert("passwordHistory".to_string(), project_each(history, &HISTORY_KEYS));
    }

    for (name, keys) in TYPED_SECTIONS {
        let Some(section) = object.get(name).filter(|v| !v.is_null()) else {
            continue;
        };
        let carried = match (name, keys) {
            ("login", _) => export_login(section),
            (_, Some(keys)) => project(section, keys),
            (_, None) => section.clone(),
        };
        out.insert(name.to_string(), carried);
    }

    Some(Value::Object(out))
}

/// `LoginExport`: three strings, the URI array, and `fido2Credentials` whole.
///
/// The passkey array is carried rather than projected for
/// [`TYPED_SECTIONS`]'s reason -- nothing in this crate decrypts a passkey,
/// so nothing here is entitled to decide which of its thirteen fields matter.
fn export_login(raw: &Value) -> Value {
    let mut out = match project(raw, &LOGIN_KEYS) {
        Value::Object(map) => map,
        // `project` only ever answers an object; this arm is unreachable and
        // is an empty login rather than a panic on a server's value.
        _ => Map::new(),
    };
    if let Some(uris) = raw.get("uris").and_then(Value::as_array) {
        out.insert("uris".to_string(), project_each(uris, &URI_KEYS));
    }
    if let Some(passkeys) = raw.get("fido2Credentials").filter(|v| !v.is_null()) {
        out.insert("fido2Credentials".to_string(), passkeys.clone());
    }
    Value::Object(out)
}

/// The named keys of `raw`, present ones only, as a fresh object. A value
/// that is not an object answers an empty object rather than failing: a
/// section the server sent as a string is a section with no fields, and the
/// alternative is refusing to export the whole item over it.
fn project(raw: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    if let Some(object) = raw.as_object() {
        for key in keys {
            if let Some(value) = object.get(*key) {
                out.insert((*key).to_string(), value.clone());
            }
        }
    }
    Value::Object(out)
}

/// [`project`] over an array, keeping its order and its length.
fn project_each(raw: &[Value], keys: &[&str]) -> Value {
    Value::Array(raw.iter().map(|element| project(element, keys)).collect())
}

/// A fresh random GUID in the 8-4-4-4-12 spelling, version 4, variant 1 --
/// what `Utils.newGuid()` produces.
///
/// The bytes come from the same CSPRNG `crypto::encrypt` takes its IV from.
/// A failure is reported rather than papered over with a counter: the
/// plaintext here is discarded, but a predictable one would be a known
/// plaintext for the one field an offline attacker would start on.
fn new_guid() -> Result<String, CryptoError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| CryptoError::Rng)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

// ---- the one the vault window reaches ----------------------------------------

/// The client and the live session for the account this process is serving.
///
/// Assembled per operation out of process facts rather than held, for
/// `rest::send::active_account`'s reason: an account switch replaces both.
fn active_account() -> Result<(RestClient, Authenticated), ExportProblem> {
    let login = crate::backend_policy::direct_rest_login().ok_or(ExportProblem::Locked)?;
    let read = crate::backend_policy::direct_rest_credentials().ok_or(ExportProblem::Locked)?;
    let authenticated = read().ok_or(ExportProblem::Locked)?;
    Ok((RestClient::new(login.server_url), authenticated))
}

/// One export document for the active direct-REST account.
///
/// **One `GET /api/sync` and nothing else.** No request body is sent, so
/// `rest::api`'s `the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin`
/// census is untouched by this module; the sync is the same call
/// `rest::send`'s list and create already make for the same reason -- it is
/// where `VaultKeys::unwrap_from` gets the profile.
pub fn export_document() -> Result<Zeroizing<String>, ExportProblem> {
    let (client, mut authenticated) = active_account()?;
    let response =
        client.sync_refreshing(&mut authenticated.session).map_err(ExportProblem::from_rest)?;
    let profile = response.profile.as_ref().ok_or_else(|| {
        ExportProblem::Failed("Bitwarden's answer carried no profile.".to_string())
    })?;
    let (keys, _) = VaultKeys::unwrap_from(&authenticated.master_key, profile)
        .map_err(|_| ExportProblem::Locked)?;
    let guid = new_guid().map_err(|_| {
        ExportProblem::Failed("This PC's random number generator refused.".to_string())
    })?;
    encrypted_json(&response, &keys, &guid).map_err(|_| {
        ExportProblem::Failed("The export could not be built on this PC.".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::crypto::{decrypt, EncString, SymmetricKey};
    use serde_json::json;

    fn keys() -> VaultKeys {
        crate::rest::sync::tests::keys_from_user(&[7u8; 64])
    }

    fn user_key() -> SymmetricKey {
        crate::rest::crypto::tests::key_from_64(&[7u8; 64])
    }

    /// A sync payload with one personal login, one org item, one trashed
    /// item and one folder.
    fn a_sync() -> SyncResponse {
        serde_json::from_value(json!({
            "profile": { "key": "2.x|y|z" },
            "folders": [
                { "id": "f-1", "name": "2.folder|name|mac", "revisionDate": "2026-01-01T00:00:00Z" },
                { "id": "", "name": "2.nameless|f|m" },
            ],
            "ciphers": [
                {
                    "id": "c-1",
                    "organizationId": null,
                    "folderId": "f-1",
                    "type": 1,
                    "reprompt": 0,
                    "name": "2.name|iv|mac",
                    "notes": null,
                    "favorite": true,
                    "key": "2.cipherkey|iv|mac",
                    "creationDate": "2026-01-01T00:00:00Z",
                    "revisionDate": "2026-02-02T00:00:00Z",
                    "deletedDate": null,
                    "archivedDate": null,
                    "collectionIds": ["should-not-survive"],
                    "attachments": [{ "id": "a-1", "fileName": "2.f|i|m" }],
                    "viewPassword": true,
                    "login": {
                        "username": "2.user|iv|mac",
                        "password": "2.pass|iv|mac",
                        "totp": null,
                        "passwordRevisionDate": "2026-02-02T00:00:00Z",
                        "uris": [
                            { "uri": "2.uri|iv|mac", "uriChecksum": "2.sum|iv|mac", "match": null,
                              "somethingElse": 1 }
                        ],
                        "fido2Credentials": [{ "credentialId": "2.cid|iv|mac" }]
                    },
                    "fields": [
                        { "name": "2.fn|i|m", "value": "2.fv|i|m", "type": 1, "linkedId": null,
                          "extra": true }
                    ],
                    "passwordHistory": [
                        { "password": "2.old|i|m", "lastUsedDate": "2026-01-05T00:00:00Z",
                          "extra": 9 }
                    ]
                },
                {
                    "id": "c-org",
                    "organizationId": "org-1",
                    "type": 1,
                    "name": "2.org|iv|mac",
                    "deletedDate": null
                },
                {
                    "id": "c-trash",
                    "organizationId": null,
                    "type": 1,
                    "name": "2.trash|iv|mac",
                    "deletedDate": "2026-03-03T00:00:00Z"
                }
            ]
        }))
        .expect("the fixture parses")
    }

    fn document() -> Value {
        let text = encrypted_json(&a_sync(), &keys(), "guid-1").expect("the document builds");
        serde_json::from_str(&text).expect("the document is JSON")
    }

    #[test]
    fn the_envelope_is_the_four_keys_the_cli_writes() {
        let doc = document();
        let object = doc.as_object().expect("an object");
        let mut names: Vec<&str> = object.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["encKeyValidation_DO_NOT_EDIT", "encrypted", "folders", "items"],
            "the export envelope is no longer the document `getEncryptedExport` writes"
        );
        assert_eq!(object["encrypted"], Value::Bool(true), "`encrypted` must be the literal true");
        // Control: `passwordProtected` belongs to the OTHER format, and a
        // document claiming it without a salt and a KDF would be a lie about
        // what protects the file.
        assert!(!object.contains_key("passwordProtected"), "this is not a password export");
    }

    #[test]
    fn the_validation_field_is_the_named_plaintext_under_the_user_key() {
        let doc = document();
        let enc: EncString = doc[VALIDATION_KEY]
            .as_str()
            .expect("a string")
            .parse()
            .expect("an EncString");
        let opened = decrypt(&user_key(), &enc).expect("the user key opens it");
        assert_eq!(&*opened, b"guid-1", "the validation field is not what was handed in");
        // Control: it really is encrypted, not the plaintext written through.
        assert_ne!(
            doc[VALIDATION_KEY].as_str().expect("a string"),
            "guid-1",
            "the validation plaintext reached the file in the clear"
        );
    }

    #[test]
    fn a_fresh_guid_is_a_version_four_guid_and_two_of_them_differ() {
        let one = new_guid().expect("the CSPRNG answers");
        let two = new_guid().expect("the CSPRNG answers");
        assert_ne!(one, two, "two fresh GUIDs collided");
        assert_eq!(one.len(), 36, "{one} is not 8-4-4-4-12");
        let parts: Vec<&str> = one.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12],
            "{one} is not 8-4-4-4-12"
        );
        assert!(one.starts_with(|c: char| c.is_ascii_hexdigit()), "{one} is not hex");
        assert_eq!(&parts[2][..1], "4", "{one} does not carry the version nibble");
        assert!(matches!(&parts[3][..1], "8" | "9" | "a" | "b"), "{one} has no variant nibble");
    }

    #[test]
    fn only_the_personal_untrashed_items_are_carried() {
        let doc = document();
        let items = doc["items"].as_array().expect("an array");
        let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().expect("an id")).collect();
        assert_eq!(ids, ["c-1"], "the export carried an item the CLI would have dropped");
        // Controls: both excluded ciphers really were in the payload, so the
        // assertion above is not passing over an empty fixture.
        let source = a_sync();
        assert_eq!(source.ciphers.len(), 3, "the fixture lost the items it exists to exclude");
        assert!(
            source.ciphers.iter().any(|c| c["id"] == json!("c-org")),
            "the fixture has no organisation cipher to exclude"
        );
        assert!(
            source.ciphers.iter().any(|c| c["id"] == json!("c-trash")),
            "the fixture has no trashed cipher to exclude"
        );
    }

    #[test]
    fn every_carried_value_is_the_servers_own_ciphertext() {
        let doc = document();
        let item = &doc["items"][0];
        assert_eq!(item["name"], json!("2.name|iv|mac"), "the name was not carried verbatim");
        assert_eq!(
            item["key"],
            json!("2.cipherkey|iv|mac"),
            "the wrapped cipher key was not carried, so the item would not open"
        );
        assert_eq!(item["login"]["password"], json!("2.pass|iv|mac"));
        assert_eq!(item["login"]["uris"][0]["uri"], json!("2.uri|iv|mac"));
        assert_eq!(item["login"]["uris"][0]["uriChecksum"], json!("2.sum|iv|mac"));
        assert_eq!(item["fields"][0]["value"], json!("2.fv|i|m"));
        assert_eq!(item["passwordHistory"][0]["password"], json!("2.old|i|m"));
        assert_eq!(item["login"]["fido2Credentials"][0]["credentialId"], json!("2.cid|iv|mac"));
        assert_eq!(item["favorite"], json!(true));
        assert_eq!(item["folderId"], json!("f-1"));
        assert_eq!(item["type"], json!(1));
    }

    #[test]
    fn what_the_export_dto_does_not_have_does_not_reach_the_file() {
        let doc = document();
        let item = &doc["items"][0];
        assert!(item.get("attachments").is_none(), "an attachment list reached an export");
        assert!(item.get("viewPassword").is_none(), "a wire-only field reached an export");
        assert!(
            item["login"].get("passwordRevisionDate").is_none(),
            "`LoginExport` has no passwordRevisionDate"
        );
        assert!(item["login"]["uris"][0].get("somethingElse").is_none());
        assert!(item["fields"][0].get("extra").is_none());
        assert!(item["passwordHistory"][0].get("extra").is_none());
        assert_eq!(
            item["collectionIds"],
            Value::Null,
            "`collectionIds` must be the CLI's forced null, not the server's list"
        );
        // Control: those keys really were on the wire, so the absences above
        // are absences this code produced.
        let source = a_sync();
        let raw = &source.ciphers[0];
        assert!(raw.get("attachments").is_some(), "the fixture carried no attachment to drop");
        assert!(raw.get("viewPassword").is_some());
        assert!(raw["login"].get("passwordRevisionDate").is_some());
        assert_eq!(raw["collectionIds"], json!(["should-not-survive"]));
    }

    #[test]
    fn a_folder_is_an_id_and_a_name_and_nothing_else() {
        let doc = document();
        let folders = doc["folders"].as_array().expect("an array");
        assert_eq!(folders.len(), 1, "the id-less folder was not dropped");
        let mut names: Vec<&str> =
            folders[0].as_object().expect("an object").keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["id", "name"], "`FolderWithIdExport` has exactly two fields");
        assert_eq!(folders[0]["name"], json!("2.folder|name|mac"));
        // Control: the wire folder carried a third field, so "nothing else"
        // is a fact about this mapper.
        assert!(
            a_sync().folders[0].get("revisionDate").is_some(),
            "the fixture folder carried nothing to drop"
        );
    }

    #[test]
    fn the_document_is_two_space_pretty_printed_like_json_stringify() {
        let text = encrypted_json(&a_sync(), &keys(), "guid-1").expect("builds");
        assert!(text.starts_with("{\n  \""), "the document is not two-space pretty printed");
        // Control: the same document with no indentation would fail the line
        // above, and this proves the two are genuinely different renderings.
        let compact = serde_json::to_string(
            &serde_json::from_str::<Value>(&text).expect("re-parses"),
        )
        .expect("re-serializes");
        assert!(!compact.starts_with("{\n  \""), "the control is not a different rendering");
    }

    #[test]
    fn a_typed_section_is_carried_for_its_own_type() {
        let sync: SyncResponse = serde_json::from_value(json!({
            "profile": { "key": "2.x|y|z" },
            "folders": [],
            "ciphers": [{
                "id": "c-card", "organizationId": null, "type": 3, "deletedDate": null,
                "name": "2.card|i|m",
                "card": { "number": "2.num|i|m", "code": "2.cvv|i|m", "brand": "2.b|i|m",
                          "cardholderName": null, "expMonth": null, "expYear": null }
            }]
        }))
        .expect("parses");
        let text = encrypted_json(&sync, &keys(), "g").expect("builds");
        let doc: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(doc["items"][0]["card"]["number"], json!("2.num|i|m"));
        assert_eq!(doc["items"][0]["card"]["code"], json!("2.cvv|i|m"));
        // Control: the login section is absent, so `card` was chosen and not
        // merely everything-copied.
        assert!(doc["items"][0].get("login").is_none(), "a card item gained a login section");
    }

    /// **Every value in the document came off the wire, bar exactly one.**
    ///
    /// This is the plaintext property stated as something a test can see: if
    /// this module ever decrypted a field, or re-encrypted one, or invented
    /// a value, the new string would not appear in the payload the server
    /// sent. The single permitted exception is the validation field, and the
    /// control below is that its value really is such a string -- so a check
    /// that could not tell a novel value from a carried one would fail here.
    #[test]
    fn the_only_value_this_module_mints_is_the_validation_field() {
        let sync = a_sync();
        let wire = serde_json::to_string(&json!({
            "folders": sync.folders,
            "ciphers": sync.ciphers,
        }))
        .expect("the wire re-serializes");
        let doc = document();
        let validation = doc[VALIDATION_KEY].as_str().expect("a string").to_string();

        let mut seen = 0usize;
        let mut strings = Vec::new();
        collect_strings(&doc["items"], &mut strings);
        collect_strings(&doc["folders"], &mut strings);
        for value in &strings {
            seen += 1;
            assert!(
                wire.contains(value.as_str()),
                "`{value}` is in the export but was never on the wire"
            );
        }
        assert!(seen >= 10, "only {seen} strings were checked; the walk found nothing");

        // Control: the one minted value would fail the very assertion above,
        // which is what makes the loop a check rather than a formality.
        assert!(
            !wire.contains(validation.as_str()),
            "control: the validation field looks like a carried value"
        );
    }

    /// Every string leaf under `value`, in no particular order.
    fn collect_strings(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(text) => into.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|v| collect_strings(v, into)),
            Value::Object(map) => map.values().for_each(|v| collect_strings(v, into)),
            _ => {}
        }
    }
}

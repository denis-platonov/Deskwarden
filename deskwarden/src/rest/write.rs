//! The inverse of [`crate::rest::sync`]: a [`VaultItem`] turned back into the
//! wire cipher shape, with every secret field re-encrypted.
//!
//! # The one rule this file exists to keep
//!
//! **`PUT /api/ciphers/{id}` replaces the whole cipher.** Whatever the body
//! does not carry, the server does not keep. A Bitwarden cipher has many more
//! fields than this crate models -- attachments, `passwordHistory`,
//! `reprompt`, `collectionIds`, `fido2Credentials`, and whatever the next
//! server version adds -- and a mapper that emitted only the modelled fields
//! would delete all of them from the user's real vault on the first edit,
//! irreversibly and silently.
//!
//! So this mapper never *builds* a cipher. It **starts from
//! [`VaultItem::other`]** -- the byte-for-byte remainder `sync` kept when it
//! decrypted -- and lays the modelled fields back over it. The same holds one
//! level down, for [`LoginData::other`], [`UriEntry::other`],
//! [`VaultField::other`], [`CardData::other`], [`IdentityData::other`] and
//! [`SshKeyData::other`]. `an_unmodelled_field_survives_a_decrypt_encrypt_round_trip`
//! is the acceptance test for the whole module, and it asserts on keys this
//! crate has no concept of.
//!
//! # Which key each field goes under
//!
//! Exactly the one `sync` read it with, worked out by the same code
//! ([`CipherKeys::for_cipher`]): the cipher's own key if it carries one,
//! otherwise its organisation's, otherwise the user key. The wrapped
//! `cipher.key` itself is *not* re-wrapped -- it rides through `other`
//! untouched, which is both cheaper and the only way to be sure it still
//! opens what it opened.
//!
//! # Absent, empty, and the difference between them
//!
//! A modelled field that is `None` is **removed**, not written as an
//! encrypted empty string; a modelled field that is `Some("")` is encrypted,
//! and comes back as `Some("")`. Those are different states on the wire and
//! the distinction only survives if it is preserved deliberately, so it is.
//! ([`VaultItem::name`] is not optional and is always written.)
//!
//! # The two fields `sync` decrypts *in place* inside a catch-all
//!
//! `passwordHistory[].password` and `login.uri` are decrypted into
//! `other` rather than into a typed field, so they -- alone among the
//! retained JSON -- have to be re-encrypted on the way out. They are also the
//! one place where "re-encrypt what is there" could go wrong: if the original
//! decryption *failed*, `sync` recorded the failure and left the **ciphertext**
//! in place, and encrypting that again would store a doubly-wrapped value
//! that nothing can ever read.
//!
//! [`reencrypt_in_place`] therefore leaves any value that still parses as an
//! [`EncString`] exactly as it found it. That is a heuristic and it is named
//! as one, but it is a safe one in both directions: a real plaintext that
//! happened to be `2.<16 bytes b64>|<whole blocks b64>|<32 bytes b64>` is not
//! a value a human has ever typed, and the failure mode of guessing wrong the
//! other way -- double-encrypting a password history entry -- is data the user
//! cannot recover.
//!
//! # No plaintext is logged, ever
//!
//! Nothing here has a `Debug` of its own; the value it returns is a
//! `serde_json::Value` full of ciphertext, and
//! [`EncString`]'s `Display` -- which this module calls, because that is how a
//! ciphertext is written down -- prints the wire form. **Do not log a mapped
//! cipher.** The intermediate plaintexts are borrowed from the `Zeroizing`
//! fields they already live in; this module makes no owned plaintext copy of
//! any of them.

use serde_json::Value;
use zeroize::Zeroizing;

use crate::rest::crypto::{CryptoError, EncString, SymmetricKey, encrypt};
use crate::rest::sync::{CipherKeys, VaultKeys};
use crate::vault_bridge::{
    CardData, IdentityData, LoginData, SshKeyData, UriEntry, VaultField, VaultItem,
};

/// A `serde_json` object, spelled once -- as in [`crate::rest::sync`].
type Object = serde_json::Map<String, Value>;

/// One [`VaultItem`], as a server-ready cipher body.
///
/// Suitable as the body of both `POST /api/ciphers` and
/// `PUT /api/ciphers/{id}`. The `id` is written into the body as well as the
/// path because the API's own model carries it and Bitwarden's clients send
/// it; on a create the caller is expected to hold an item whose `id` is empty,
/// and an empty `id` is omitted rather than sent as `""`.
///
/// # Errors
///
/// [`CryptoError`] if the cipher's key cannot be worked out (an organisation
/// this session has no key for, or a `cipher.key` that does not unwrap), or
/// if any encryption fails. Never partial: on an error nothing has been sent
/// anywhere, because nothing here sends.
pub fn encrypt_item(item: &VaultItem, keys: &VaultKeys) -> Result<Value, CryptoError> {
    // The retained JSON is the *base*, and everything below lays over it.
    // This ordering is the whole requirement of this file: build from the
    // remainder, never from the model.
    let mut out = item.other.clone();

    // `organizationId` and the wrapped `key` both live in the remainder, so
    // the key is resolved from the base rather than from anything this
    // function has already changed.
    let cipher_keys = CipherKeys::for_cipher(keys, &out)?;
    let key = cipher_keys.key();

    reencrypt_password_history(key, &mut out)?;

    if item.id.is_empty() {
        out.remove("id");
    } else {
        out.insert("id".to_string(), Value::String(item.id.clone()));
    }
    put_text(&mut out, "name", Some(item.name.as_str()), key)?;
    put_text(&mut out, "notes", item.notes.as_deref().map(String::as_str), key)?;

    match item.item_type {
        Some(t) => out.insert("type".to_string(), Value::from(t)),
        None => out.remove("type"),
    };
    match &item.folder_id {
        Some(id) => out.insert("folderId".to_string(), Value::String(id.clone())),
        None => out.remove("folderId"),
    };
    out.insert("favorite".to_string(), Value::Bool(item.favorite));

    // `fields` is a `Vec` with no absent state on [`VaultItem`], so an empty
    // one is written as an empty array rather than guessed back into `null`.
    // The two mean the same thing to the server; inventing a `null` would be
    // claiming to know which one arrived.
    let mut fields = Vec::with_capacity(item.fields.len());
    for field in &item.fields {
        fields.push(Value::Object(encrypt_field(key, field)?));
    }
    out.insert("fields".to_string(), Value::Array(fields));

    put_object(&mut out, "login", item.login.as_ref(), |l| encrypt_login(key, l))?;
    put_object(&mut out, "card", item.card.as_ref(), |c| encrypt_card(key, c))?;
    put_object(&mut out, "identity", item.identity.as_ref(), |i| encrypt_identity(key, i))?;
    put_object(&mut out, "sshKey", item.ssh_key.as_ref(), |s| encrypt_ssh_key(key, s))?;

    Ok(Value::Object(out))
}

/// Writes one optional type object, or removes it.
///
/// Removal rather than `"login": null`: a full-state PUT carrying an explicit
/// null has told the server the object is gone, which is the same hazard
/// [`VaultItem::login`]'s own `skip_serializing_if` exists for.
fn put_object<T>(
    out: &mut Object,
    wire: &str,
    value: Option<&T>,
    build: impl FnOnce(&T) -> Result<Object, CryptoError>,
) -> Result<(), CryptoError> {
    match value {
        Some(v) => {
            out.insert(wire.to_string(), Value::Object(build(v)?));
        }
        None => {
            out.remove(wire);
        }
    }
    Ok(())
}

/// Encrypts one optional string into `out[wire]`, or removes the key.
///
/// The `None` arm is the "absent stays absent" half of the module docs, and
/// it *removes* rather than leaving whatever was there: a modelled field is
/// modelled, so the model is the authority on it.
fn put_text(
    out: &mut Object,
    wire: &str,
    plain: Option<&str>,
    key: &SymmetricKey,
) -> Result<(), CryptoError> {
    match plain {
        Some(text) => {
            // `encrypt` draws a fresh IV per call, which is correct and is
            // not to be hoisted: reusing one IV across two fields under one
            // key leaks whether their first blocks are equal.
            out.insert(wire.to_string(), Value::String(encrypt(key, text.as_bytes())?.to_string()));
        }
        None => {
            out.remove(wire);
        }
    }
    Ok(())
}

/// One custom field: label and value both encrypted, `type` and anything else
/// carried through.
fn encrypt_field(key: &SymmetricKey, field: &VaultField) -> Result<Object, CryptoError> {
    let mut out = field.other.clone();
    put_text(&mut out, "name", field.name.as_deref(), key)?;
    put_text(&mut out, "value", field.value.as_deref().map(String::as_str), key)?;
    Ok(out)
}

fn encrypt_login(key: &SymmetricKey, login: &LoginData) -> Result<Object, CryptoError> {
    let mut out = login.other.clone();
    // The API's back-compat duplicate of `uris[0].uri`, which `sync`
    // decrypted in place. See the module docs on why this is not simply
    // re-encrypted unconditionally.
    reencrypt_in_place(key, &mut out, "uri")?;

    put_text(&mut out, "username", login.username.as_deref(), key)?;
    put_text(&mut out, "password", login.password.as_deref().map(String::as_str), key)?;
    put_text(&mut out, "totp", login.totp.as_deref().map(String::as_str), key)?;

    let mut uris = Vec::with_capacity(login.uris.len());
    for entry in &login.uris {
        uris.push(Value::Object(encrypt_uri(key, entry)?));
    }
    out.insert("uris".to_string(), Value::Array(uris));
    Ok(out)
}

/// One URI entry. `match` -- a number here, a string over `bw serve` -- rides
/// `other` in whatever form it arrived, which is the point of it being there.
fn encrypt_uri(key: &SymmetricKey, entry: &UriEntry) -> Result<Object, CryptoError> {
    let mut out = entry.other.clone();
    put_text(&mut out, "uri", entry.uri.as_deref(), key)?;
    Ok(out)
}

fn encrypt_card(key: &SymmetricKey, card: &CardData) -> Result<Object, CryptoError> {
    let mut out = card.other.clone();
    put_text(&mut out, "cardholderName", card.cardholder_name.as_deref(), key)?;
    put_text(&mut out, "brand", card.brand.as_deref(), key)?;
    put_text(&mut out, "number", card.number.as_deref().map(String::as_str), key)?;
    put_text(&mut out, "expMonth", card.exp_month.as_deref(), key)?;
    put_text(&mut out, "expYear", card.exp_year.as_deref(), key)?;
    put_text(&mut out, "code", card.code.as_deref().map(String::as_str), key)?;
    Ok(out)
}

/// The eighteen identity fields, spelled out for
/// [`crate::rest::sync`]'s stated reason: this is where a wire name and a
/// struct field have to line up eighteen times, and a reader checking that
/// `postal_code` goes to `postalCode` should be able to see it.
fn encrypt_identity(key: &SymmetricKey, id: &IdentityData) -> Result<Object, CryptoError> {
    let mut out = id.other.clone();
    for (wire, value) in [
        ("title", &id.title),
        ("firstName", &id.first_name),
        ("middleName", &id.middle_name),
        ("lastName", &id.last_name),
        ("address1", &id.address1),
        ("address2", &id.address2),
        ("address3", &id.address3),
        ("city", &id.city),
        ("state", &id.state),
        ("postalCode", &id.postal_code),
        ("country", &id.country),
        ("company", &id.company),
        ("email", &id.email),
        ("phone", &id.phone),
        ("ssn", &id.ssn),
        ("username", &id.username),
        ("passportNumber", &id.passport_number),
        ("licenseNumber", &id.license_number),
    ] {
        put_text(&mut out, wire, value.as_deref(), key)?;
    }
    Ok(out)
}

fn encrypt_ssh_key(key: &SymmetricKey, ssh: &SshKeyData) -> Result<Object, CryptoError> {
    let mut out = ssh.other.clone();
    put_text(&mut out, "privateKey", ssh.private_key.as_deref().map(String::as_str), key)?;
    put_text(&mut out, "publicKey", ssh.public_key.as_deref(), key)?;
    put_text(&mut out, "keyFingerprint", ssh.key_fingerprint.as_deref(), key)?;
    Ok(out)
}

/// Re-encrypts `passwordHistory[].password`, which `sync` decrypted in place.
///
/// `lastUsedDate` is plaintext on this wire, was left alone on the way in, and
/// is left alone here.
fn reencrypt_password_history(key: &SymmetricKey, out: &mut Object) -> Result<(), CryptoError> {
    let Some(entries) = out.get_mut("passwordHistory").and_then(|v| v.as_array_mut()) else {
        return Ok(());
    };
    for entry in entries.iter_mut() {
        let Some(object) = entry.as_object_mut() else { continue };
        reencrypt_in_place(key, object, "password")?;
    }
    Ok(())
}

/// Encrypts `object[wire]` back into ciphertext -- unless it already is
/// ciphertext.
///
/// See the module docs for why the [`EncString`] parse is the guard and why
/// erring in this direction is the safe one. A value that is absent or is not
/// a JSON string is left alone: there is nothing to encrypt and inventing
/// something would be worse.
fn reencrypt_in_place(
    key: &SymmetricKey,
    object: &mut Object,
    wire: &str,
) -> Result<(), CryptoError> {
    let Some(text) = object.get(wire).and_then(|v| v.as_str()) else { return Ok(()) };
    if text.parse::<EncString>().is_ok() {
        // Still ciphertext: this field never decrypted on the way in, and
        // encrypting it again would bury it.
        return Ok(());
    }
    // Borrowed into a wiped buffer rather than handed to `encrypt` straight
    // out of the `Value`, so that the one owned copy this function makes of a
    // historical password is one that wipes itself.
    let plain = Zeroizing::new(text.to_string());
    let sealed = encrypt(key, plain.as_bytes())?.to_string();
    object.insert(wire.to_string(), Value::String(sealed));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::crypto::Kdf;
    use crate::rest::crypto::master_key;
    use crate::rest::crypto::tests::{key_from_64, seal};
    use crate::rest::sync::{SyncResponse, decrypt_vault};

    /// The 64 bytes of a deterministic fixture key. Same construction as
    /// `sync`'s own fixtures, so the two files' fixtures are the same keys.
    fn key_bytes(seed: u8) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(u8::try_from(i % 251).expect("under 251"));
        }
        bytes
    }

    fn key(seed: u8) -> SymmetricKey {
        key_from_64(&key_bytes(seed))
    }

    fn enc(key: &SymmetricKey, plain: &str) -> String {
        seal(key, plain.as_bytes())
    }

    /// A master key and the `profile.key` blob that protects [`key(1)`], built
    /// through the real arrangement so a test can drive [`decrypt_vault`] from
    /// the top and get a [`VaultKeys`] the honest way.
    fn account() -> (crate::rest::crypto::MasterKey, String) {
        let master =
            master_key(b"master", "fixture@example.invalid", Kdf::Pbkdf2 { iterations: 1 })
                .expect("one iteration");
        let protected = seal(&master.stretch(), &key_bytes(1));
        (master, protected)
    }

    /// Decrypts a one-cipher sync and hands back the item plus the keys, so
    /// every test below round-trips through the *real* read path rather than
    /// hand-building a `VaultItem`.
    fn round_trip_in(cipher: Value) -> (VaultItem, VaultKeys) {
        let (master, protected) = account();
        let payload = serde_json::json!({
            "profile": { "key": protected },
            "ciphers": [cipher],
            "folders": [],
        });
        let response: SyncResponse =
            serde_json::from_value(payload).expect("the fixture parses as a sync");
        let profile = response.profile.as_ref().expect("a profile");
        let (keys, key_failures) =
            VaultKeys::unwrap_from(&master, profile).expect("the user key unwraps");
        assert!(key_failures.is_empty(), "{key_failures:?}");
        let vault = decrypt_vault(&response, &master).expect("the fixture decrypts");
        assert!(vault.failures.is_empty(), "the fixture did not decrypt cleanly: {:?}", vault.failures);
        let item = vault.items.into_iter().next().expect("one item");
        (item, keys)
    }

    /// A cipher carrying a deliberate mix of things this crate models and
    /// things it does not.
    fn cipher_with_unmodelled_fields() -> Value {
        let k = key(1);
        serde_json::json!({
            "object": "cipherDetails",
            "id": "11111111-1111-1111-1111-111111111111",
            "organizationId": null,
            "folderId": null,
            "type": 1,
            "name": enc(&k, "Example"),
            "notes": null,
            "favorite": false,
            "fields": null,
            "login": {
                "username": enc(&k, "user@example.invalid"),
                "password": enc(&k, "hunter2"),
                "totp": null,
                "uris": [{ "uri": enc(&k, "https://example.invalid"), "match": null }],
                "uri": enc(&k, "https://example.invalid"),
                "fido2Credentials": [],
                "passwordRevisionDate": "2024-01-01T00:00:00.000000Z",
                "autofillOnPageLoad": true,
            },
            "card": null,
            "identity": null,
            "sshKey": null,
            "secureNote": null,
            // Everything from here down is a key this crate has no concept
            // of, and the whole point of the acceptance test.
            "reprompt": 1,
            "attachments": [{
                "id": "abc123",
                "fileName": "2.aWl2aXZpdml2aXZpdml2aQ==|Y2lwaGVydGV4dGNpcGhlcnRleHQ=|bWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWE=",
                "key": "2.aWl2aXZpdml2aXZpdml2aQ==|Y2lwaGVydGV4dGNpcGhlcnRleHQ=|bWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWE=",
                "size": "12345",
                // A different host from the login's URI on purpose: the
                // no-plaintext-on-the-wire assertion below searches the whole
                // body, and an attachment URL *is* plaintext (it is the
                // server's own, and rides through untouched), so sharing a
                // host would make that assertion fire on a legitimate value.
                "url": "https://files.invalid/attachments/abc123",
            }],
            "collectionIds": [],
            "edit": true,
            "viewPassword": true,
            "permissions": { "delete": true, "restore": true },
            "organizationUseTotp": false,
            "archivedDate": null,
            "deletedDate": null,
            "creationDate": "2023-05-05T05:05:05.000000Z",
            "revisionDate": "2024-06-06T06:06:06.000000Z",
            // The one that matters most: a field no version of this crate has
            // ever heard of.
            "someFutureFieldNobodyHasShippedYet": { "nested": [1, 2, {"deep": "value"}] },
        })
    }

    /// **The acceptance test for this module.**
    ///
    /// A wire cipher carrying fields this crate does not model goes through
    /// the real decrypt and back out through the mapper, and every unmodelled
    /// key must come back byte-identical. If this fails, an edit in the
    /// running app deletes those fields from the user's real vault.
    #[test]
    fn an_unmodelled_field_survives_a_decrypt_encrypt_round_trip() {
        let original = cipher_with_unmodelled_fields();
        let (item, keys) = round_trip_in(original.clone());
        let written = encrypt_item(&item, &keys).expect("the item maps");

        let before = original.as_object().expect("an object");
        let after = written.as_object().expect("an object");

        // Every key this crate models is *expected* to be rewritten (it is
        // freshly encrypted, with a fresh IV). Everything else must be
        // untouched, and it is named here so that adding a modelled field
        // without thinking about it fails this test rather than passing it.
        const MODELLED: &[&str] = &[
            "id", "name", "fields", "login", "card", "identity", "sshKey", "notes", "type",
            "folderId", "favorite",
        ];
        // `sync` drops the four `null` type objects on the way in, with its
        // own recorded reason; they carry no information and are the one
        // documented non-survival.
        const DROPPED_NULLS: &[&str] = &["card", "identity", "sshKey", "secureNote"];

        for (name, value) in before {
            if MODELLED.contains(&name.as_str()) || DROPPED_NULLS.contains(&name.as_str()) {
                continue;
            }
            let kept = after.get(name).unwrap_or_else(|| {
                panic!("the unmodelled key `{name}` was deleted by the mapper")
            });
            assert_eq!(kept, value, "the unmodelled key `{name}` was rewritten");
        }

        // And the guard on the guard: if the fixture had no unmodelled keys
        // in it, the loop above would pass vacuously.
        assert!(
            after.contains_key("someFutureFieldNobodyHasShippedYet"),
            "the fixture lost its own control field"
        );
        assert!(after.contains_key("attachments"), "attachments did not survive");
        assert!(after.contains_key("reprompt"), "reprompt did not survive");
    }

    /// The same for the nested catch-alls: an unmodelled key *inside*
    /// `login`, inside a URI entry and inside a custom field.
    #[test]
    fn unmodelled_keys_inside_login_uris_and_fields_survive_too() {
        let k = key(1);
        let cipher = serde_json::json!({
            "id": "22222222-2222-2222-2222-222222222222",
            "type": 1,
            "name": enc(&k, "Nested"),
            "favorite": false,
            "fields": [{
                "name": enc(&k, "Recovery code"),
                "value": enc(&k, "123456"),
                "type": 1,
                "linkedId": null,
                "unknownFieldKey": "kept",
            }],
            "login": {
                "username": enc(&k, "u"),
                "uris": [{
                    "uri": enc(&k, "https://a.invalid"),
                    "match": 3,
                    "uriChecksum": "abc",
                    "unknownUriKey": [7],
                }],
                "autofillOnPageLoad": null,
                "unknownLoginKey": {"a": 1},
            },
        });
        let (item, keys) = round_trip_in(cipher);
        let written = encrypt_item(&item, &keys).expect("the item maps");

        let login = written.get("login").and_then(|v| v.as_object()).expect("a login");
        assert_eq!(login.get("unknownLoginKey"), Some(&serde_json::json!({"a": 1})));
        assert_eq!(login.get("autofillOnPageLoad"), Some(&Value::Null));

        let uri = login
            .get("uris")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_object())
            .expect("a uri entry");
        assert_eq!(uri.get("match"), Some(&serde_json::json!(3)));
        assert_eq!(uri.get("uriChecksum"), Some(&serde_json::json!("abc")));
        assert_eq!(uri.get("unknownUriKey"), Some(&serde_json::json!([7])));

        let field = written
            .get("fields")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_object())
            .expect("a field");
        assert_eq!(field.get("type"), Some(&serde_json::json!(1)));
        assert_eq!(field.get("unknownFieldKey"), Some(&serde_json::json!("kept")));
    }

    /// The mapped body decrypts back to exactly what went in -- so the
    /// preservation above is not being bought by writing plaintext.
    #[test]
    fn every_modelled_secret_is_written_as_ciphertext_and_reads_back_the_same() {
        let (item, keys) = round_trip_in(cipher_with_unmodelled_fields());
        let written = encrypt_item(&item, &keys).expect("the item maps");
        let rendered = serde_json::to_string(&written).expect("serializable");
        for plaintext in ["Example", "user@example.invalid", "hunter2", "https://example.invalid"] {
            assert!(
                !rendered.contains(plaintext),
                "the plaintext `{plaintext}` was written to the wire body"
            );
        }

        // Round-trip the mapped body back through the read path.
        let (again, _) = round_trip_in(written);
        assert_eq!(again.name, "Example");
        let login = again.login.as_ref().expect("a login");
        assert_eq!(login.username.as_deref(), Some("user@example.invalid"));
        assert_eq!(login.password.as_deref().map(String::as_str), Some("hunter2"));
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri.as_deref(), Some("https://example.invalid"));
        assert_eq!(login.other.get("uri").and_then(|v| v.as_str()), Some("https://example.invalid"));
    }

    /// Two fields under one key must not share an IV. The check is on the
    /// rendered wire strings because that is where an IV is visible.
    #[test]
    fn no_two_encrypted_fields_share_an_iv() {
        let (item, keys) = round_trip_in(cipher_with_unmodelled_fields());
        let written = encrypt_item(&item, &keys).expect("the item maps");
        let login = written.get("login").and_then(|v| v.as_object()).expect("a login");
        let ivs: Vec<&str> = [
            written.get("name").and_then(|v| v.as_str()),
            login.get("username").and_then(|v| v.as_str()),
            login.get("password").and_then(|v| v.as_str()),
        ]
        .into_iter()
        .map(|v| v.expect("present").split('|').next().expect("an iv part"))
        .collect();
        assert_ne!(ivs[0], ivs[1]);
        assert_ne!(ivs[1], ivs[2]);
        assert_ne!(ivs[0], ivs[2]);
    }

    /// Absent stays absent; empty stays empty. Both are asserted on the same
    /// item so neither can be satisfied by the other's rule.
    #[test]
    fn an_absent_field_stays_absent_and_an_empty_one_stays_empty() {
        let k = key(1);
        let cipher = serde_json::json!({
            "id": "33333333-3333-3333-3333-333333333333",
            "type": 1,
            "name": enc(&k, "Blanks"),
            "favorite": false,
            "login": {
                // Present and empty.
                "username": enc(&k, ""),
                // Absent entirely -- no key at all, not a null.
                "uris": [],
            },
        });
        let (item, keys) = round_trip_in(cipher);
        assert_eq!(item.login.as_ref().expect("a login").username.as_deref(), Some(""));
        assert!(item.login.as_ref().expect("a login").password.is_none());

        let written = encrypt_item(&item, &keys).expect("the item maps");
        let login = written.get("login").and_then(|v| v.as_object()).expect("a login");
        assert!(!login.contains_key("password"), "an absent password became a written one");
        assert!(!login.contains_key("totp"), "an absent totp became a written one");
        assert!(!written.as_object().expect("an object").contains_key("notes"));

        let username = login.get("username").and_then(|v| v.as_str()).expect("a username");
        assert!(username.starts_with("2."), "an empty username was written as plaintext");
        // And it is still empty on the way back, not absent.
        let (again, _) = round_trip_in(written);
        assert_eq!(again.login.as_ref().expect("a login").username.as_deref(), Some(""));
    }

    /// `passwordHistory[].password` is decrypted into the catch-all on the
    /// way in, so it must be re-encrypted on the way out -- and `lastUsedDate`,
    /// which is plaintext on this wire, must not be.
    #[test]
    fn password_history_is_re_encrypted_and_its_dates_are_not() {
        let k = key(1);
        let cipher = serde_json::json!({
            "id": "44444444-4444-4444-4444-444444444444",
            "type": 1,
            "name": enc(&k, "History"),
            "favorite": false,
            "passwordHistory": [
                { "password": enc(&k, "old-one"), "lastUsedDate": "2024-01-01T00:00:00.000Z" },
            ],
        });
        let (item, keys) = round_trip_in(cipher);
        let written = encrypt_item(&item, &keys).expect("the item maps");
        let entry = written
            .get("passwordHistory")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_object())
            .expect("one history entry");
        let sealed = entry.get("password").and_then(|v| v.as_str()).expect("a password");
        assert!(sealed.starts_with("2."), "a historical password was written as plaintext");
        assert!(!sealed.contains("old-one"));
        assert_eq!(
            entry.get("lastUsedDate").and_then(|v| v.as_str()),
            Some("2024-01-01T00:00:00.000Z")
        );

        let (again, _) = round_trip_in(written);
        assert_eq!(
            again
                .other
                .get("passwordHistory")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|e| e.get("password"))
                .and_then(|v| v.as_str()),
            Some("old-one")
        );
    }

    /// A field whose decryption *failed* is left in `other` as ciphertext, and
    /// must not be encrypted a second time. See the module docs: guessing
    /// wrong here is unrecoverable for the user.
    #[test]
    fn a_history_entry_that_never_decrypted_is_not_encrypted_twice() {
        let k = key(1);
        // Sealed under a key this vault does not have, so the MAC fails.
        let foreign = enc(&key(9), "unreadable");
        let cipher = serde_json::json!({
            "id": "55555555-5555-5555-5555-555555555555",
            "type": 1,
            "name": enc(&k, "Broken history"),
            "favorite": false,
            "passwordHistory": [{ "password": foreign, "lastUsedDate": null }],
        });

        let (master, protected) = account();
        let payload = serde_json::json!({
            "profile": { "key": protected },
            "ciphers": [cipher.clone()],
            "folders": [],
        });
        let response: SyncResponse = serde_json::from_value(payload).expect("parses");
        let profile = response.profile.as_ref().expect("a profile");
        let (keys, _) = VaultKeys::unwrap_from(&master, profile).expect("the user key");
        let vault = decrypt_vault(&response, &master).expect("decrypts");
        assert_eq!(vault.failures.len(), 1, "the fixture was supposed to fail one field");
        let item = vault.items.into_iter().next().expect("one item");

        let written = encrypt_item(&item, &keys).expect("the item maps");
        let kept = written
            .get("passwordHistory")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("password"))
            .and_then(|v| v.as_str())
            .expect("a password");
        assert_eq!(
            kept,
            cipher.get("passwordHistory").and_then(|v| v.as_array()).expect("array")[0]
                .get("password")
                .and_then(|v| v.as_str())
                .expect("the original"),
            "an undecryptable history entry was encrypted a second time"
        );
    }

    /// A cipher with its own `key` decrypts and re-encrypts under *that* key,
    /// and the wrapped key itself rides through untouched.
    #[test]
    fn a_cipher_with_its_own_key_is_written_back_under_that_key() {
        let user = key(1);
        let own_bytes = key_bytes(7);
        let own = key(7);
        let wrapped = seal(&user, &own_bytes);
        let cipher = serde_json::json!({
            "id": "66666666-6666-6666-6666-666666666666",
            "type": 1,
            "key": wrapped,
            "name": enc(&own, "Own key"),
            "favorite": false,
            "login": { "password": enc(&own, "secret") },
        });
        let (item, keys) = round_trip_in(cipher);
        assert_eq!(item.name, "Own key");

        let written = encrypt_item(&item, &keys).expect("the item maps");
        assert_eq!(
            written.get("key").and_then(|v| v.as_str()),
            Some(wrapped.as_str()),
            "the wrapped cipher key was rewritten instead of carried through"
        );
        // The name must open under the cipher's own key and not the user key.
        let name = written.get("name").and_then(|v| v.as_str()).expect("a name");
        let parsed = name.parse::<EncString>().expect("an EncString");
        assert!(
            crate::rest::crypto::decrypt(&user, &parsed).is_err(),
            "the field was written under the user key instead of the cipher's own"
        );
        assert_eq!(
            &*crate::rest::crypto::decrypt(&own, &parsed).expect("opens under the cipher key"),
            b"Own key"
        );
    }

    /// An item whose organisation this session has no key for cannot be
    /// written, and says so rather than writing under the wrong key.
    #[test]
    fn an_item_in_an_unknown_organisation_is_refused_rather_than_mis_keyed() {
        let (item, keys) = round_trip_in(cipher_with_unmodelled_fields());
        let mut orphan = item;
        orphan
            .other
            .insert("organizationId".to_string(), Value::String("no-such-org".to_string()));
        let err = encrypt_item(&orphan, &keys).expect_err("no key for that organisation");
        assert!(matches!(err, CryptoError::Malformed(_)), "{err:?}");
    }

    /// A create has no id yet, and an empty one must not be sent as `""`.
    #[test]
    fn an_item_with_no_id_omits_it_rather_than_sending_an_empty_one() {
        let (item, keys) = round_trip_in(cipher_with_unmodelled_fields());
        let fresh = VaultItem { id: String::new(), ..item };
        let written = encrypt_item(&fresh, &keys).expect("the item maps");
        assert!(!written.as_object().expect("an object").contains_key("id"));
    }
}

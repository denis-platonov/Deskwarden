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
//! **This is not decided by looking at the value.** An earlier version of
//! [`reencrypt_in_place`] left alone anything that still parsed as an
//! [`crate::rest::crypto::EncString`], and that was wrong in the one direction that matters: a
//! *real* secret whose plaintext happens to have `EncString` shape -- a value
//! beginning `2.` with two `|` separators and base64-looking parts, which a
//! password generator can produce and a user can paste -- would have been
//! skipped and **written to the server in the clear**. The two failure
//! directions are not symmetric. A double-encrypted history entry is visible
//! and recoverable; an exposed password is neither.
//!
//! So the write path is *told*, not left to infer.
//! [`crate::rest::sync::DecryptedItem`] carries the paths `sync` recorded a
//! decryption failure for -- which is exactly the set of values whose
//! ciphertext is still in place -- and [`reencrypt_in_place`] consults that
//! record and nothing else. An item whose record is empty (one this crate
//! composed itself, via
//! [`DecryptedItem::newly_composed`](crate::rest::sync::DecryptedItem::newly_composed))
//! has every in-place value encrypted, which is the direction that cannot
//! leak.
//!
//! # No plaintext is logged, ever
//!
//! Nothing here has a `Debug` of its own; the value it returns is a
//! `serde_json::Value` full of ciphertext, and
//! [`crate::rest::crypto::EncString`]'s `Display` -- which this module calls, because that is how a
//! ciphertext is written down -- prints the wire form. **Do not log a mapped
//! cipher.** The intermediate plaintexts are borrowed from the `Zeroizing`
//! fields they already live in; this module makes no owned plaintext copy of
//! any of them.

use serde_json::Value;
use zeroize::Zeroizing;

use crate::rest::crypto::{CryptoError, SymmetricKey, encrypt};
use crate::rest::sync::{CipherKeys, DecryptedItem, VaultKeys};
use crate::vault_bridge::{
    CardData, IdentityData, LoginData, SshKeyData, UriEntry, VaultField, VaultItem,
};

/// A `serde_json` object, spelled once -- as in [`crate::rest::sync`].
type Object = serde_json::Map<String, Value>;

/// A cipher body **this module produced**, and the only thing
/// [`crate::rest::api`] will send to `POST /api/ciphers` or
/// `PUT /api/ciphers/{id}`.
///
/// # Why this is a type and not a `serde_json::Value`
///
/// `PUT /api/ciphers/{id}` **replaces the whole cipher**: whatever the body
/// does not carry, the server does not keep. A body assembled from the fields
/// this crate models would therefore delete every field it does not model --
/// attachments, `passwordHistory`, `fido2Credentials`, and whatever the next
/// server version adds -- from the user's real vault, irreversibly and
/// silently, on the first edit.
///
/// [`encrypt_item`] is the one function that builds a body the right way
/// (over [`VaultItem::other`], never from the model), and while the write
/// endpoints took a bare `Value` nothing said so except a doc comment. The
/// field below is private and the only construction of this type in the crate
/// is inside [`encrypt_item`], so "this body came from the mapper" is now
/// something the compiler checks rather than something a reviewer remembers.
/// `mapped_cipher_is_constructed_only_by_the_mapper` pins the half a private
/// field cannot state on its own.
///
/// # No `Debug`, deliberately
///
/// This holds a whole cipher's worth of ciphertext, written with
/// [`crate::rest::crypto::EncString`]'s `Display` -- which prints the wire
/// form. Nothing should ever format one, so nothing can.
pub struct MappedCipher {
    body: Value,
}

impl MappedCipher {
    /// The JSON to send. `pub(crate)` and returning a borrow: a caller can
    /// hand it to `ureq`, which is the entire intended use, and cannot get an
    /// owned `Value` back out to edit and re-send.
    pub(crate) fn body(&self) -> &Value {
        &self.body
    }
}

/// One [`VaultItem`], as a server-ready cipher body.
///
/// Suitable as the body of both `POST /api/ciphers` and
/// `PUT /api/ciphers/{id}`. The `id` is written into the body as well as the
/// path because the API's own model carries it and Bitwarden's clients send
/// it; on a create the caller is expected to hold an item whose `id` is empty,
/// and an empty `id` is omitted rather than sent as `""`.
///
/// Takes a [`DecryptedItem`] rather than a bare [`VaultItem`] because the two
/// in-place values (`login.uri`, `passwordHistory[].password`) can only be
/// handled correctly by an inverse that knows which of them actually
/// decrypted. See the module docs.
///
/// # Errors
///
/// [`CryptoError`] if the cipher's key cannot be worked out (an organisation
/// this session has no key for, or a `cipher.key` that does not unwrap), or
/// if any encryption fails. Never partial: on an error nothing has been sent
/// anywhere, because nothing here sends.
pub fn encrypt_item(
    decrypted: &DecryptedItem,
    keys: &VaultKeys,
) -> Result<MappedCipher, CryptoError> {
    let item: &VaultItem = &decrypted.item;
    // The retained JSON is the *base*, and everything below lays over it.
    // This ordering is the whole requirement of this file: build from the
    // remainder, never from the model.
    let mut out = item.other.clone();

    // `organizationId` and the wrapped `key` both live in the remainder, so
    // the key is resolved from the base rather than from anything this
    // function has already changed.
    let cipher_keys = CipherKeys::for_cipher(keys, &out)?;
    let key = cipher_keys.key();

    reencrypt_password_history(key, &mut out, decrypted)?;

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

    put_object(&mut out, "login", item.login.as_ref(), |l| encrypt_login(key, l, decrypted))?;
    put_object(&mut out, "card", item.card.as_ref(), |c| encrypt_card(key, c))?;
    put_object(&mut out, "identity", item.identity.as_ref(), |i| encrypt_identity(key, i))?;
    put_object(&mut out, "sshKey", item.ssh_key.as_ref(), |s| encrypt_ssh_key(key, s))?;

    Ok(MappedCipher { body: Value::Object(out) })
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

fn encrypt_login(
    key: &SymmetricKey,
    login: &LoginData,
    decrypted: &DecryptedItem,
) -> Result<Object, CryptoError> {
    let mut out = login.other.clone();
    // The API's back-compat duplicate of `uris[0].uri`, which `sync`
    // decrypted in place. See the module docs on why this is not simply
    // re-encrypted unconditionally. The path is `sync`'s own spelling for
    // this field, and the two have to stay in step -- `the_two_in_place_paths_
    // are_spelled_the_same_on_both_sides` is what makes a rename fail.
    reencrypt_in_place(key, &mut out, "uri", "login.uri", decrypted)?;

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
fn reencrypt_password_history(
    key: &SymmetricKey,
    out: &mut Object,
    decrypted: &DecryptedItem,
) -> Result<(), CryptoError> {
    let Some(entries) = out.get_mut("passwordHistory").and_then(|v| v.as_array_mut()) else {
        return Ok(());
    };
    for (i, entry) in entries.iter_mut().enumerate() {
        let Some(object) = entry.as_object_mut() else { continue };
        // The index is part of the path because `sync` recorded it that way:
        // one broken entry in a history of five must not exempt the other
        // four from being re-encrypted.
        let path = format!("passwordHistory[{i}].password");
        reencrypt_in_place(key, object, "password", &path, decrypted)?;
    }
    Ok(())
}

/// Encrypts `object[wire]` back into ciphertext -- unless [`crate::rest::sync`]
/// recorded that `path` never decrypted, in which case what is there is still
/// the server's own ciphertext and is left exactly as it was found.
///
/// # The guard is a recorded fact, not a look at the value
///
/// It would be easy, and it was once done, to ask instead whether the value
/// still parses as an [`crate::rest::crypto::EncString`]. That reads the
/// *secret* to decide how to treat the secret, and it gets the answer wrong
/// for a plaintext that happens to have `EncString` shape -- by writing it to
/// the server unencrypted. `path` comes from
/// [`DecryptedItem::is_still_encrypted`], which answers from what the decrypt
/// pass actually did.
///
/// A path the record does not mention is encrypted. That is the safe default
/// in both the case it is meant for (a value that decrypted) and the case it
/// is not (an item this crate composed, or a record that has gone stale): the
/// worst outcome is a doubly-wrapped value the user can see and fix.
///
/// A value that is absent or is not a JSON string is left alone: there is
/// nothing to encrypt and inventing something would be worse.
fn reencrypt_in_place(
    key: &SymmetricKey,
    object: &mut Object,
    wire: &str,
    path: &str,
    decrypted: &DecryptedItem,
) -> Result<(), CryptoError> {
    let Some(text) = object.get(wire).and_then(|v| v.as_str()) else { return Ok(()) };
    if decrypted.is_still_encrypted(path) {
        // This field never decrypted on the way in, so the ciphertext is
        // still sitting here and encrypting it again would bury it.
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
pub(crate) mod tests {
    use super::*;
    use crate::rest::crypto::EncString;
    use crate::rest::crypto::Kdf;
    use crate::rest::crypto::master_key;
    use crate::rest::crypto::tests::{key_from_64, seal};
    use crate::rest::sync::{SyncResponse, decrypt_vault};

    /// The mapped body as JSON, for the assertions below. [`MappedCipher`]
    /// deliberately hands out only a borrow, and every test here wants to
    /// index into it or feed it back through the read path.
    fn mapped(item: &DecryptedItem, keys: &VaultKeys) -> Value {
        encrypt_item(item, keys).expect("the item maps").body().clone()
    }

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
    fn round_trip_in(cipher: Value) -> (DecryptedItem, VaultKeys) {
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

    /// A real [`MappedCipher`] for [`crate::rest::api`]'s tests.
    ///
    /// It exists because those tests *cannot* hand-build one -- which is the
    /// whole point of the type -- and building one the honest way means going
    /// through the mapper. `pub(crate)` on a `#[cfg(test)]` module, in the
    /// shape `crypto::tests` already established for its key helpers.
    ///
    /// The id matches the path `api`'s update test uses, and the password is
    /// the plaintext those tests search the wire for and must never find.
    pub(crate) fn a_mapped_cipher() -> MappedCipher {
        let k = key(1);
        let cipher = serde_json::json!({
            "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "type": 1,
            "name": enc(&k, "Example"),
            "favorite": false,
            "reprompt": 1,
            "login": { "password": enc(&k, "hunter2-never-on-the-wire") },
        });
        let (item, keys) = round_trip_in(cipher);
        encrypt_item(&item, &keys).expect("the fixture maps")
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
        let written = mapped(&item, &keys);

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
        let written = mapped(&item, &keys);

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
        let written = mapped(&item, &keys);
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
        let written = mapped(&item, &keys);
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

        let written = mapped(&item, &keys);
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
        let written = mapped(&item, &keys);
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

        let written = mapped(&item, &keys);
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

    /// **Risk 1, and the exact defect this file was hardened against.**
    ///
    /// A real secret whose *plaintext* has `EncString` shape. The old guard
    /// asked whether the value parsed as an `EncString` and, for this one,
    /// answered "still ciphertext, leave it" -- writing the user's previous
    /// password and their URI to the server in the clear.
    ///
    /// Both in-place paths are exercised in one item so neither can be
    /// satisfied by the other's rule, and the first assertion is the control
    /// that makes the rest mean something: the plaintext really does parse.
    #[test]
    fn a_plaintext_that_looks_like_an_enc_string_is_encrypted_and_not_passed_through() {
        // A well-formed type-2 EncString by shape. As a *plaintext* it is a
        // password a generator could produce; as a guard it was indistinguishable
        // from ciphertext, which was the bug.
        const SHAPED: &str = "2.aXZpdml2aXZpdml2aXZpdg==|Y2lwaGVydGV4dGNpcGhlcnRleHRjaXBoZXJ0ZXh0MzI=|bWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWFjbWE=";
        assert!(
            SHAPED.parse::<EncString>().is_ok(),
            "the fixture does not have EncString shape, so this test proves nothing"
        );

        let k = key(1);
        let cipher = serde_json::json!({
            "id": "77777777-7777-7777-7777-777777777777",
            "type": 1,
            "name": enc(&k, "Shaped"),
            "favorite": false,
            "login": { "uri": enc(&k, SHAPED), "uris": [] },
            "passwordHistory": [{ "password": enc(&k, SHAPED), "lastUsedDate": null }],
        });
        let (item, keys) = round_trip_in(cipher);
        // The read path really did decrypt both -- otherwise the write path
        // would be right to leave them alone and this test would be asserting
        // on the wrong thing.
        assert_eq!(
            item.login.as_ref().and_then(|l| l.other.get("uri")).and_then(|v| v.as_str()),
            Some(SHAPED)
        );

        let written = mapped(&item, &keys);
        let rendered = serde_json::to_string(&written).expect("serializable");
        assert!(
            !rendered.contains(SHAPED),
            "a plaintext with EncString shape was written to the wire unencrypted"
        );

        // And it is not merely mangled: it comes back byte-identical.
        let (again, _) = round_trip_in(written);
        assert_eq!(
            again.login.as_ref().and_then(|l| l.other.get("uri")).and_then(|v| v.as_str()),
            Some(SHAPED)
        );
        assert_eq!(
            again
                .other
                .get("passwordHistory")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|e| e.get("password"))
                .and_then(|v| v.as_str()),
            Some(SHAPED)
        );
    }

    /// The record travels by *path string*, and the two sides spell those
    /// strings independently. A rename on either side would silently turn the
    /// guard off -- back into writing a plaintext -- so it is pinned.
    ///
    /// Read through normalised line endings, because this is a CRLF checkout.
    #[test]
    fn the_two_in_place_paths_are_spelled_the_same_on_both_sides() {
        let sync_source = include_str!("sync.rs").replace("\r\n", "\n");
        let write_source = include_str!("write.rs").replace("\r\n", "\n");
        // Control: the sources were really read.
        assert!(sync_source.contains("fn decrypt_password_history"), "sync.rs was not read");
        assert!(write_source.contains("fn reencrypt_in_place"), "write.rs was not read");

        // `login.uri`: recorded by `sync`'s `fail`, queried by `write`.
        assert!(sync_source.contains(r#"fail("login.uri", why)"#), "sync renamed the login.uri path");
        assert!(
            write_source.contains(r#""uri", "login.uri", decrypted"#),
            "write renamed the login.uri path"
        );

        // `passwordHistory[i].password`: the same `format!` on both sides.
        let history = "passwordHistory[{i}].password";
        assert_eq!(
            sync_source.matches(history).count(),
            1,
            "sync no longer records the history path exactly once"
        );
        assert_eq!(
            write_source.matches(history).count(),
            2,
            "write no longer queries the history path (once in code, once in this pin)"
        );
    }

    /// **Risk 2's other half.** A private field says "nobody outside this
    /// module builds one"; it does not say "nobody *inside* it does". The
    /// claim `MappedCipher`'s doc makes -- that the only construction in the
    /// crate is inside `encrypt_item` -- is pinned here.
    ///
    /// A WALK of `src/`, not an `include_str!` of one file: a second
    /// construction added in any other module is exactly the thing that would
    /// re-open the hole, and a pin that only reads this file would not see it.
    /// Line endings are normalised first -- this is a CRLF checkout.
    #[test]
    fn mapped_cipher_is_constructed_only_by_the_mapper() {
        // Built rather than written, so this test's own source does not
        // contain the needle it counts.
        let needle = format!("MappedCipher{}{{ body", " ");
        let absent = format!("MappedCipher{}{{ payload", " ");

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rust_sources(&src, &mut files);
        // Control: the walk found a real tree, including this file.
        assert!(files.len() > 20, "the source walk found only {} files", files.len());

        let mut sites = Vec::new();
        for path in &files {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("{} is readable", path.display()))
                .replace("\r\n", "\n");
            assert_eq!(text.matches(&absent).count(), 0, "the control needle matched");
            for _ in 0..text.matches(&needle).count() {
                sites.push(path.clone());
            }
        }
        assert_eq!(
            sites.len(),
            1,
            "a mapped cipher is constructed somewhere other than the mapper: {sites:?}"
        );
        assert!(sites[0].ends_with("write.rs"), "{:?}", sites[0]);

        // And that one site is inside `encrypt_item`, not merely in this file.
        let this = include_str!("write.rs").replace("\r\n", "\n");
        let start = this.find("pub fn encrypt_item(").expect("encrypt_item is in this file");
        let body = &this[start..];
        let end = body.find("\n}\n").expect("encrypt_item has an end");
        assert!(
            body[..end].contains(&needle),
            "the one construction is not inside encrypt_item"
        );
    }

    /// Every `.rs` file under `dir`, recursively.
    fn collect_rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
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

        let written = mapped(&item, &keys);
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
            .item
            .other
            .insert("organizationId".to_string(), Value::String("no-such-org".to_string()));
        // `expect_err` is not available here, and that is the point:
        // [`MappedCipher`] has no `Debug`, so an unexpected `Ok` cannot be
        // printed -- which is exactly the property that keeps a mapped cipher
        // out of a panic message.
        let Err(err) = encrypt_item(&orphan, &keys) else {
            panic!("an item in an unknown organisation was mapped anyway");
        };
        assert!(matches!(err, CryptoError::Malformed(_)), "{err:?}");
    }

    /// A create has no id yet, and an empty one must not be sent as `""`.
    #[test]
    fn an_item_with_no_id_omits_it_rather_than_sending_an_empty_one() {
        let (item, keys) = round_trip_in(cipher_with_unmodelled_fields());
        let fresh = DecryptedItem::newly_composed(VaultItem { id: String::new(), ..item.item });
        let written = mapped(&fresh, &keys);
        assert!(!written.as_object().expect("an object").contains_key("id"));
    }
}

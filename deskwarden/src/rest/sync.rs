//! `/api/sync`'s payload: parsed, decrypted, and mapped into the
//! [`VaultItem`] shapes the rest of the app already depends on.
//!
//! # The one sentence that explains the whole file
//!
//! **`bw serve` hands over plaintext; `/api/sync` hands over ciphertext.**
//! Every field of a cipher that could hold user data -- the *name*, the
//! notes, the username, every URI, every card and identity field, every
//! custom field's label as well as its value -- arrives as an
//! [`EncString`](crate::rest::crypto::EncString). The `bw` CLI decrypted them
//! before this app ever saw them. Nothing else does now.
//!
//! That is the difference the brief asked about, and it is not the only one.
//! See [`the payload comparison`](self#how-the-rest-payload-differs-from-bw-serves).
//!
//! # The key hierarchy, and the trap in the middle of it
//!
//! 1. The master key comes from the login ([`crate::rest::api`]).
//! 2. Stretched, it unwraps `profile.key` -- the **user key**.
//! 3. `profile.privateKey` is an `EncString` under the user key; inside is
//!    PKCS#8 DER. That private key unwraps each `profile.organizations[].key`
//!    into an **organisation key**.
//! 4. A cipher decrypts under its organisation's key if it has an
//!    `organizationId`, else under the user key.
//! 5. **And then there is `cipher.key`.** A cipher may carry its own wrapped
//!    symmetric key, and when it does, *every field of that cipher decrypts
//!    under that key instead* -- the key from step 4 only unwraps it. This is
//!    the trap: a client that ignores `cipher.key` decrypts nothing on such
//!    an item and, worse, may show a MAC failure on an item that is
//!    perfectly fine. It is handled in [`CipherKeys::for_cipher`].
//!
//! # What a failure to decrypt does
//!
//! **One bad field does not lose the item.** [`DecryptedVault::failures`]
//! records which field of which cipher failed, by *name* -- never its
//! ciphertext, never a key -- and the item is still produced with that field
//! absent. The alternative, refusing the whole sync, means one corrupt cipher
//! hides an entire vault; the alternative to *that*, silently blanking it,
//! means a user cannot tell an empty field from a broken one.
//!
//! An item whose *name* cannot be decrypted is the one exception the mapper
//! makes visible, because a nameless row is unusable: the name becomes the
//! empty string and a failure is recorded, so the caller can decide.
//!
//! # How the REST payload differs from `bw serve`'s
//!
//! `vault_bridge`'s capture test
//! `a_real_shaped_item_round_trips_with_every_observed_key` records every key
//! a real item carries over the CLI's local HTTP. Against `/api/sync`'s
//! `cipherDetails` (Vaultwarden `src/db/models/cipher.rs`, `to_json`):
//!
//! * **`object` is `"cipherDetails"`, not `"item"`.**
//! * **All five type objects are present, four of them `null`.** `bw serve`
//!   sends only the one that applies. A `"card": null` on a login is
//!   *dropped* by this mapper rather than carried into
//!   [`VaultItem::other`] -- see [`strip_null_type_objects`] -- because
//!   `VaultItem`'s own `skip_serializing_if` would not re-emit them and a
//!   round-trip assertion would fail on a difference that means nothing.
//! * **Keys `bw serve` never sends**: `edit`, `viewPassword`, `permissions`,
//!   `organizationUseTotp`, `archivedDate`, and `deletedDate` on items that
//!   are not deleted (`bw serve` omits it; the API sends `null`).
//! * **`login.uri`** -- a back-compat duplicate of `uris[0].uri`, encrypted,
//!   which this mapper decrypts along with the array so the two do not
//!   disagree.
//! * **`login.uris[].match`** is a number or `null` here; the CLI has been
//!   observed sending it as a string. `UriEntry::other` carries whichever
//!   arrives, unchanged.
//! * **`passwordHistory[].password` is encrypted**; `lastUsedDate` is not.
//! * **`attachments[]`** carry an encrypted `fileName` and their own `key`.
//!   Not decrypted here -- see [`DecryptedVault`]'s doc on what is unmapped.
//! * **`key`** on an item means two entirely different things on the two
//!   wires. Over `bw serve` the captured item carried `"key":"K"` and this
//!   crate treats it as opaque; over the API it is the per-cipher key
//!   described above.
//!
//! # Zeroizing
//!
//! Every decrypted value that lands in a modelled field goes straight into
//! the `Zeroizing<String>` that field already is; no plaintext `String`
//! intermediate is created for them. There is exactly one exception and it is
//! named at [`decrypt_password_history`], with the reason.

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::rest::crypto::{
    CryptoError, EncString, MasterKey, SymmetricKey, decrypt, unwrap_org_key, unwrap_user_key,
};
use crate::vault_bridge::{
    CardData, Folder, IdentityData, LoginData, SshKeyData, UriEntry, VaultField, VaultItem,
};

/// A `serde_json` object, spelled once.
type Object = serde_json::Map<String, serde_json::Value>;

// ---- the wire shapes --------------------------------------------------------

/// `/api/sync`'s top level.
///
/// Everything is optional. This is deliberate and it is the difference
/// between a client that works against one implementation and a client that
/// works: a Bitwarden-compatible server is free to omit a section it does not
/// implement (a self-hosted server without organisations sends no
/// `collections`, or sends `null`), and a field that must be present is a
/// field whose absence is a crash rather than a diagnosis.
#[derive(Debug, Deserialize)]
pub struct SyncResponse {
    #[serde(default)]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub ciphers: Vec<serde_json::Value>,
    #[serde(default)]
    pub folders: Vec<serde_json::Value>,
}

/// The account, and the two key blobs the whole vault hangs off.
///
/// `Debug` is derived and that is safe: both fields are ciphertext, and
/// [`crate::debug_leak_guard`] would flag this type if either were
/// `Zeroizing`. Printing a wrapped key is printing the thing an offline
/// attack targets, so they are `Option<String>` rather than parsed
/// `EncString` here only because parsing is the mapper's job; nothing logs
/// this type.
#[derive(Debug, Deserialize)]
pub struct Profile {
    /// The user's symmetric key, wrapped under the stretched master key.
    #[serde(default)]
    pub key: Option<String>,
    /// The RSA private key (PKCS#8 DER), wrapped under the user key. Absent
    /// on an account that has never been in an organisation.
    #[serde(rename = "privateKey", default)]
    pub private_key: Option<String>,
    #[serde(default)]
    pub organizations: Vec<Organization>,
}

/// One organisation the user is a confirmed member of.
#[derive(Debug, Deserialize)]
pub struct Organization {
    pub id: String,
    /// The organisation's symmetric key, RSA-OAEP-wrapped to the user's
    /// public key. A type-4 `EncString`.
    #[serde(default)]
    pub key: Option<String>,
}

// ---- what comes out ---------------------------------------------------------

/// One field of one cipher that could not be decrypted.
///
/// Both strings are *names*: the cipher's id (already public -- it is a GUID
/// the server assigned and it appears in URLs) and the JSON path of the
/// field. Neither is derived from a plaintext, a ciphertext or a key, which
/// is [`CryptoError`]'s own rule applied one level up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptFailure {
    pub cipher_id: String,
    pub field: String,
    pub why: CryptoError,
}

impl std::fmt::Display for DecryptFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cipher {}: {} could not be decrypted ({})", self.cipher_id, self.field, self.why)
    }
}

/// A whole sync, decrypted.
///
/// # What is deliberately not mapped
///
/// Named here rather than discovered later, because an unmapped field is a
/// field the user loses on the first write this crate ever does:
///
/// * **Attachments.** `attachments[].fileName` is an `EncString` and each
///   attachment has its own `key`. They ride [`VaultItem::other`] encrypted.
///   Downloading an attachment is not in this read path, and decrypting a
///   filename this crate cannot then fetch would be decoration. **A write
///   path must handle this before it round-trips an item with attachments.**
/// * **`login.fido2Credentials`.** Passkeys, with several encrypted fields
///   of their own. Carried encrypted, for the same reason: nothing in this
///   app uses a passkey.
/// * **Collections and organisation membership beyond the key.** The app has
///   no concept of a collection.
/// * **Sends**, explicitly out of scope.
///
/// Everything else on the wire is either mapped into a typed field or ridden
/// through [`VaultItem::other`] byte-for-byte.
pub struct DecryptedVault {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
    /// Empty on a healthy sync. Non-empty means some field of some item is
    /// missing from `items` and the caller should say so rather than pretend
    /// the vault is complete.
    pub failures: Vec<DecryptFailure>,
}

/// Hand-written: [`VaultItem`] and [`Folder`] carry secrets and refuse a
/// derived `Debug` themselves, but printing a whole vault's worth of them is
/// not something any log line should do even redacted. Counts only.
impl std::fmt::Debug for DecryptedVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecryptedVault")
            .field("items", &self.items.len())
            .field("folders", &self.folders.len())
            .field("failures", &self.failures)
            .finish()
    }
}

// ---- the keys ---------------------------------------------------------------

/// Every key a sync's ciphers might need, worked out once.
///
/// Built rather than looked up per cipher because the RSA unwrap of an
/// organisation key is the single most expensive operation in this file, and
/// a vault with a thousand ciphers in one organisation would otherwise do it
/// a thousand times.
pub struct VaultKeys {
    user: SymmetricKey,
    /// `(organisation id, its key)`. A `Vec` rather than a map: a user is in
    /// a handful of organisations at most, and a linear scan over four
    /// entries is not worth a hash.
    orgs: Vec<(String, SymmetricKey)>,
}

/// Hand-written for the reason every key type in this crate hand-writes one.
impl std::fmt::Debug for VaultKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultKeys")
            .field("user", &"<redacted>")
            .field("organisations", &self.orgs.len())
            .finish()
    }
}

impl VaultKeys {
    /// Unwraps the user key, and every organisation key reachable from it.
    ///
    /// # An organisation that cannot be unwrapped is skipped, not fatal
    ///
    /// The user key is fatal: without it there is no vault. An organisation
    /// key is not -- a member whose RSA key does not open one organisation
    /// can still read every personal item and every *other* organisation, and
    /// failing the whole sync would hide all of them. The ciphers of the
    /// skipped organisation then fail per field, and say so, which is the
    /// behaviour this module already has for a bad field.
    pub fn unwrap_from(
        master_key: &MasterKey,
        profile: &Profile,
    ) -> Result<(Self, Vec<DecryptFailure>), CryptoError> {
        let Some(protected) = profile.key.as_deref() else {
            return Err(CryptoError::Malformed("the profile carries no protected user key"));
        };
        let user = unwrap_user_key(&master_key.stretch(), &protected.parse::<EncString>()?)?;

        let mut failures = Vec::new();
        let mut orgs = Vec::new();
        if !profile.organizations.is_empty() {
            // The private key is only needed if there is an organisation to
            // use it on, so an account without one is not held to having a
            // readable RSA key it never uses.
            match private_key_der(&user, profile) {
                Ok(der) => {
                    for org in &profile.organizations {
                        match unwrap_one_org(&der, org) {
                            Ok(key) => orgs.push((org.id.clone(), key)),
                            Err(why) => failures.push(DecryptFailure {
                                cipher_id: org.id.clone(),
                                field: "organizations[].key".to_string(),
                                why,
                            }),
                        }
                    }
                }
                Err(why) => failures.push(DecryptFailure {
                    cipher_id: String::new(),
                    field: "profile.privateKey".to_string(),
                    why,
                }),
            }
        }
        Ok((Self { user, orgs }, failures))
    }

    /// The key a cipher's *own* key is wrapped under, or that its fields are
    /// wrapped under when it has none.
    fn owner_of(&self, organization_id: Option<&str>) -> Option<&SymmetricKey> {
        match organization_id {
            None => Some(&self.user),
            Some(id) => self.orgs.iter().find(|(known, _)| known == id).map(|(_, key)| key),
        }
    }
}

/// The RSA private key's DER, decrypted under the user key.
fn private_key_der(
    user: &SymmetricKey,
    profile: &Profile,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let Some(wrapped) = profile.private_key.as_deref() else {
        return Err(CryptoError::Malformed("the profile carries no private key"));
    };
    decrypt(user, &wrapped.parse::<EncString>()?)
}

/// One organisation key, RSA-unwrapped.
fn unwrap_one_org(der: &[u8], org: &Organization) -> Result<SymmetricKey, CryptoError> {
    let Some(wrapped) = org.key.as_deref() else {
        return Err(CryptoError::Malformed("the organisation carries no key"));
    };
    unwrap_org_key(der, &wrapped.parse::<EncString>()?)
}

/// The key one cipher's fields actually decrypt under.
///
/// A newtype over a borrowed-or-owned key rather than a bare reference,
/// because a per-cipher key is *created* here and lives only as long as the
/// cipher is being mapped.
pub(crate) enum CipherKeys<'k> {
    Shared(&'k SymmetricKey),
    Own(SymmetricKey),
}

impl CipherKeys<'_> {
    pub(crate) fn key(&self) -> &SymmetricKey {
        match self {
            Self::Shared(key) => key,
            Self::Own(key) => key,
        }
    }

    /// Works out which key a cipher's fields are under. See the module docs'
    /// step 5 for why this exists and what ignoring it would cost.
    pub(crate) fn for_cipher<'k>(
        keys: &'k VaultKeys,
        cipher: &Object,
    ) -> Result<CipherKeys<'k>, CryptoError> {
        let org = cipher.get("organizationId").and_then(|v| v.as_str());
        let Some(owner) = keys.owner_of(org) else {
            return Err(CryptoError::Malformed("no key for this cipher's organisation"));
        };
        match cipher.get("key").and_then(|v| v.as_str()) {
            // `unwrap_user_key` is named for its commonest caller but is
            // exactly the operation wanted here: decrypt, then split 64 bytes
            // into an encryption key and a MAC key. A per-cipher key has the
            // same shape as a user key because it is the same kind of thing.
            Some(wrapped) => {
                Ok(CipherKeys::Own(unwrap_user_key(owner, &wrapped.parse::<EncString>()?)?))
            }
            None => Ok(CipherKeys::Shared(owner)),
        }
    }
}

// ---- the mapping ------------------------------------------------------------

/// Decrypts a whole sync into the app's shapes.
pub fn decrypt_vault(
    response: &SyncResponse,
    master_key: &MasterKey,
) -> Result<DecryptedVault, CryptoError> {
    let Some(profile) = response.profile.as_ref() else {
        return Err(CryptoError::Malformed("the sync payload carries no profile"));
    };
    let (keys, mut failures) = VaultKeys::unwrap_from(master_key, profile)?;

    let mut items = Vec::with_capacity(response.ciphers.len());
    for raw in &response.ciphers {
        if let Some(item) = map_cipher(raw, &keys, &mut failures) {
            items.push(item);
        }
    }

    let mut folders = Vec::with_capacity(response.folders.len());
    for raw in &response.folders {
        if let Some(folder) = map_folder(raw, &keys.user, &mut failures) {
            folders.push(folder);
        }
    }

    Ok(DecryptedVault { items, folders, failures })
}

/// One cipher.
///
/// Returns `None` only for a cipher that is not a JSON object or has no `id`
/// -- there is nothing to produce, and an item with a synthesised id would be
/// an item this app could later write back over the wrong record.
fn map_cipher(
    raw: &serde_json::Value,
    keys: &VaultKeys,
    failures: &mut Vec<DecryptFailure>,
) -> Option<VaultItem> {
    let cipher = raw.as_object()?;
    let id = cipher.get("id").and_then(|v| v.as_str())?.to_string();

    let cipher_keys = match CipherKeys::for_cipher(keys, cipher) {
        Ok(k) => k,
        Err(why) => {
            failures.push(DecryptFailure { cipher_id: id, field: "key".to_string(), why });
            return None;
        }
    };
    let key = cipher_keys.key();
    let mut fail = |field: &str, why: CryptoError| {
        failures.push(DecryptFailure {
            cipher_id: id.clone(),
            field: field.to_string(),
            why,
        });
    };

    // `VaultItem::name` is a plain `String` and not `Zeroizing`, which is the
    // shape the rest of the app already has: a name is what is on screen in
    // the list, in the tray and in the overlay. The decryption still happens
    // into a wiped buffer and only the copy that has to live in the struct
    // outlives it.
    let name =
        text(key, cipher.get("name"), "name", &mut fail).map(|n| n.to_string()).unwrap_or_default();
    let notes = text(key, cipher.get("notes"), "notes", &mut fail);

    let fields = cipher
        .get("fields")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, f)| map_field(key, f, i, &mut fail))
                .collect()
        })
        .unwrap_or_default();

    let login = cipher.get("login").and_then(|v| v.as_object()).map(|o| map_login(key, o, &mut fail));
    let card = cipher.get("card").and_then(|v| v.as_object()).map(|o| map_card(key, o, &mut fail));
    let identity =
        cipher.get("identity").and_then(|v| v.as_object()).map(|o| map_identity(key, o, &mut fail));
    let ssh_key =
        cipher.get("sshKey").and_then(|v| v.as_object()).map(|o| map_ssh_key(key, o, &mut fail));

    let item_type = cipher.get("type").and_then(serde_json::Value::as_i64);
    let folder_id =
        cipher.get("folderId").and_then(|v| v.as_str()).map(std::string::ToString::to_string);
    let favorite = cipher.get("favorite").and_then(serde_json::Value::as_bool).unwrap_or(false);

    let mut other = cipher.clone();
    for modelled in MODELLED_TOP_LEVEL_KEYS {
        other.remove(*modelled);
    }
    strip_null_type_objects(&mut other);
    decrypt_password_history(key, &mut other, &mut fail);

    Some(VaultItem {
        id,
        name,
        fields,
        login,
        card,
        identity,
        ssh_key,
        notes,
        item_type,
        folder_id,
        favorite,
        other,
    })
}

/// The top-level keys [`VaultItem`] models as typed fields, and which
/// therefore must **not** also appear in its catch-all.
///
/// A duplicate would serialize twice and `bw` would take the last one, which
/// is the hazard `VaultItem::other`'s own doc records. Spelled as one list so
/// adding a modelled field and forgetting to remove it here is one edit
/// rather than two -- and `every_modelled_key_is_stripped_from_the_catch_all`
/// checks the list against the struct.
const MODELLED_TOP_LEVEL_KEYS: &[&str] =
    &["id", "name", "fields", "login", "card", "identity", "sshKey", "notes", "type", "folderId", "favorite"];

/// Drops the four `null` type objects `/api/sync` sends on every cipher.
///
/// `bw serve` sends only the one that applies, so carrying `"card": null` on
/// a login into the catch-all would make a REST-sourced item a different
/// shape from a CLI-sourced one for no information at all. Only `null` is
/// dropped: a `"card": {...}` on an item that also has a login is data, and
/// would survive (through the typed field).
fn strip_null_type_objects(other: &mut Object) {
    for key in ["login", "card", "identity", "sshKey", "secureNote"] {
        if other.get(key).is_some_and(serde_json::Value::is_null) {
            other.remove(key);
        }
    }
}

/// Decrypts `passwordHistory[].password` in place.
///
/// # The one recorded zeroize exception in this file
///
/// A historical password is a secret, and this is the only decrypted secret
/// that lands in a `serde_json::Value` -- an ordinary `String`, freed
/// unwiped -- rather than in a `Zeroizing<String>`. It is done anyway, and
/// the alternative is worse in a way worth being explicit about: leaving it
/// encrypted means [`crate::vault_bridge::password_history`] hands the detail
/// pane a base64 blob and calls it the user's previous password. A wrong
/// secret shown as a right one is a worse failure than a right one in a page
/// that is not wiped -- and the same page already holds every other value on
/// its way out of ureq's response buffer, which `vault_bridge` records as a
/// pre-existing, unfixable-here exception.
///
/// `lastUsedDate` is plaintext on this wire and is left exactly as it
/// arrived.
fn decrypt_password_history(
    key: &SymmetricKey,
    other: &mut Object,
    fail: &mut impl FnMut(&str, CryptoError),
) {
    let Some(entries) = other.get_mut("passwordHistory").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for (i, entry) in entries.iter_mut().enumerate() {
        let Some(object) = entry.as_object_mut() else { continue };
        let Some(cipher_text) = object.get("password").and_then(|v| v.as_str()) else { continue };
        match plaintext(key, cipher_text) {
            Ok(plain) => {
                object.insert("password".to_string(), serde_json::Value::String(plain.to_string()));
            }
            Err(why) => fail(&format!("passwordHistory[{i}].password"), why),
        }
    }
}

/// One custom field. `type` and anything else ride [`VaultField::other`].
fn map_field(
    key: &SymmetricKey,
    raw: &serde_json::Value,
    index: usize,
    fail: &mut impl FnMut(&str, CryptoError),
) -> Option<VaultField> {
    let field = raw.as_object()?;
    // The label is encrypted too, which is the thing most easily forgotten:
    // a user's custom field is called "Recovery code", and that is data.
    let name = text(key, field.get("name"), &format!("fields[{index}].name"), fail);
    let value = text(key, field.get("value"), &format!("fields[{index}].value"), fail);
    let mut other = field.clone();
    other.remove("name");
    other.remove("value");
    Some(VaultField { name: name.map(|n| n.to_string()), value, other })
}

fn map_login(
    key: &SymmetricKey,
    login: &Object,
    fail: &mut impl FnMut(&str, CryptoError),
) -> LoginData {
    let uris = login
        .get("uris")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(i, u)| map_uri(key, u, i, fail))
                .collect()
        })
        .unwrap_or_default();

    let mut other = login.clone();
    for modelled in ["username", "password", "totp", "uris"] {
        other.remove(modelled);
    }
    // `login.uri` is the API's back-compat duplicate of `uris[0].uri` and is
    // encrypted like the rest. Decrypted in place so the two do not disagree;
    // it stays on the catch-all because `LoginData` does not model it.
    if let Some(text) = other.get("uri").and_then(|v| v.as_str()) {
        match plaintext(key, text) {
            Ok(plain) => {
                other.insert("uri".to_string(), serde_json::Value::String(plain.to_string()));
            }
            Err(why) => fail("login.uri", why),
        }
    }

    LoginData {
        username: text(key, login.get("username"), "login.username", fail).map(|u| u.to_string()),
        password: text(key, login.get("password"), "login.password", fail),
        totp: text(key, login.get("totp"), "login.totp", fail),
        uris,
        other,
    }
}

fn map_uri(
    key: &SymmetricKey,
    raw: &serde_json::Value,
    index: usize,
    fail: &mut impl FnMut(&str, CryptoError),
) -> Option<UriEntry> {
    let entry = raw.as_object()?;
    let uri = text(key, entry.get("uri"), &format!("login.uris[{index}].uri"), fail);
    let mut other = entry.clone();
    other.remove("uri");
    Some(UriEntry { uri: uri.map(|u| u.to_string()), other })
}

fn map_card(key: &SymmetricKey, card: &Object, fail: &mut impl FnMut(&str, CryptoError)) -> CardData {
    let mut other = card.clone();
    for modelled in ["cardholderName", "brand", "number", "expMonth", "expYear", "code"] {
        other.remove(modelled);
    }
    CardData {
        cardholder_name: text(key, card.get("cardholderName"), "card.cardholderName", fail)
            .map(|v| v.to_string()),
        brand: text(key, card.get("brand"), "card.brand", fail).map(|v| v.to_string()),
        number: text(key, card.get("number"), "card.number", fail),
        exp_month: text(key, card.get("expMonth"), "card.expMonth", fail).map(|v| v.to_string()),
        exp_year: text(key, card.get("expYear"), "card.expYear", fail).map(|v| v.to_string()),
        code: text(key, card.get("code"), "card.code", fail),
        other,
    }
}

/// The eighteen identity fields.
///
/// A macro would be shorter and is not used: this is the one place where the
/// wire name and the struct field have to line up eighteen times, and a
/// reader checking that `postalCode` goes to `postal_code` should be able to
/// see it rather than expand a macro in their head.
fn map_identity(
    key: &SymmetricKey,
    id: &Object,
    fail: &mut impl FnMut(&str, CryptoError),
) -> IdentityData {
    let mut get = |wire: &str| {
        text(key, id.get(wire), &format!("identity.{wire}"), fail).map(|v| v.to_string())
    };
    let mapped = IdentityData {
        title: get("title"),
        first_name: get("firstName"),
        middle_name: get("middleName"),
        last_name: get("lastName"),
        address1: get("address1"),
        address2: get("address2"),
        address3: get("address3"),
        city: get("city"),
        state: get("state"),
        postal_code: get("postalCode"),
        country: get("country"),
        company: get("company"),
        email: get("email"),
        phone: get("phone"),
        ssn: get("ssn"),
        username: get("username"),
        passport_number: get("passportNumber"),
        license_number: get("licenseNumber"),
        other: Object::new(),
    };
    let mut other = id.clone();
    for modelled in IDENTITY_WIRE_KEYS {
        other.remove(*modelled);
    }
    IdentityData { other, ..mapped }
}

/// The wire names of every field [`IdentityData`] models, so the catch-all
/// does not duplicate them. Checked against the struct by
/// `every_identity_field_is_mapped_and_stripped`.
const IDENTITY_WIRE_KEYS: &[&str] = &[
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

fn map_ssh_key(
    key: &SymmetricKey,
    ssh: &Object,
    fail: &mut impl FnMut(&str, CryptoError),
) -> SshKeyData {
    let mut other = ssh.clone();
    for modelled in ["privateKey", "publicKey", "keyFingerprint"] {
        other.remove(modelled);
    }
    SshKeyData {
        private_key: text(key, ssh.get("privateKey"), "sshKey.privateKey", fail),
        public_key: text(key, ssh.get("publicKey"), "sshKey.publicKey", fail).map(|v| v.to_string()),
        key_fingerprint: text(key, ssh.get("keyFingerprint"), "sshKey.keyFingerprint", fail)
            .map(|v| v.to_string()),
        other,
    }
}

/// One folder. Only the name is encrypted.
fn map_folder(
    raw: &serde_json::Value,
    user: &SymmetricKey,
    failures: &mut Vec<DecryptFailure>,
) -> Option<Folder> {
    let folder = raw.as_object()?;
    let id = folder.get("id").and_then(|v| v.as_str())?.to_string();
    let mut fail = |field: &str, why: CryptoError| {
        failures.push(DecryptFailure {
            cipher_id: id.clone(),
            field: field.to_string(),
            why,
        });
    };
    let name = text(user, folder.get("name"), "name", &mut fail).unwrap_or_default();
    let mut other = folder.clone();
    other.remove("id");
    other.remove("name");
    Some(Folder { id, name: name.to_string(), other })
}

// ---- the two primitives every mapper above is built from --------------------

/// Decrypts one optional JSON string field.
///
/// `None` in, `None` out -- an absent field and a `null` field are both
/// absent, which is what every `skip_serializing_if` on [`VaultItem`] already
/// assumes. A decryption failure is *also* `None`, but it is recorded first,
/// which is the difference between a field that is empty and one that is
/// broken.
fn text(
    key: &SymmetricKey,
    value: Option<&serde_json::Value>,
    field: &str,
    fail: &mut impl FnMut(&str, CryptoError),
) -> Option<Zeroizing<String>> {
    let raw = value?.as_str()?;
    match plaintext(key, raw) {
        Ok(plain) => Some(plain),
        Err(why) => {
            fail(field, why);
            None
        }
    }
}

/// Parse, decrypt, and interpret as UTF-8, wiping every intermediate.
///
/// Invalid UTF-8 after a MAC that verified is a corrupt store rather than an
/// attack -- the same reasoning [`CryptoError::Padding`] carries -- and it is
/// reported as `Malformed` rather than lossily replaced, because a password
/// with a replacement character in it is a password that will not work and
/// will not look wrong.
fn plaintext(key: &SymmetricKey, raw: &str) -> Result<Zeroizing<String>, CryptoError> {
    let bytes = decrypt(key, &raw.parse::<EncString>()?)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| CryptoError::Malformed("the plaintext is not UTF-8"))?;
    Ok(Zeroizing::new(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::crypto::master_key;
    use crate::rest::crypto::tests::{
        ORG_KEY_PRIVATE_PKCS8_DER, ORG_KEY_WRAPPED_OAEP_SHA1, base64, hex, key_from_64, seal,
    };
    use crate::rest::crypto::Kdf;

    /// A deterministic 64-byte symmetric key. Different `seed`s give
    /// different keys, which is all any fixture here needs.
    fn key(seed: u8) -> SymmetricKey {
        let mut bytes = [0u8; 64];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(u8::try_from(i % 251).expect("under 251"));
        }
        key_from_64(&bytes)
    }

    /// The 64 bytes behind [`key`], for sealing a key *as* a payload.
    fn key_bytes(seed: u8) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = seed.wrapping_mul(31).wrapping_add(u8::try_from(i % 251).expect("under 251"));
        }
        bytes
    }

    /// Seals text. A thin wrapper so the fixtures below read as data rather
    /// than as byte handling.
    fn enc(key: &SymmetricKey, plain: &str) -> String {
        seal(key, plain.as_bytes())
    }

    /// A master key, the user key it protects, and the `profile.key` blob
    /// that ties the two together -- built through the real arrangement
    /// (stretch, then seal the 64 bytes) so a test can drive
    /// [`decrypt_vault`] from the top.
    fn account() -> (MasterKey, SymmetricKey, String) {
        let master =
            master_key(b"master", "fixture@example.invalid", Kdf::Pbkdf2 { iterations: 1 })
                .expect("one iteration");
        let protected = seal(&master.stretch(), &key_bytes(1));
        (master, key(1), protected)
    }

    /// Control on the fixtures themselves. Without it every assertion below
    /// could be passing over ciphertext nothing can read, or over a "key"
    /// that opens everything.
    #[test]
    fn the_fixture_helpers_produce_something_only_the_right_key_opens() {
        let k = key(3);
        let sealed = enc(&k, "hello, world");
        assert!(sealed.starts_with("2."), "{sealed}");
        assert_eq!(*plaintext(&k, &sealed).expect("decrypts"), "hello, world");
        assert!(!sealed.contains("hello"), "the plaintext is in the wrapper: {sealed}");
        assert!(plaintext(&key(4), &sealed).is_err(), "a different key opened it");
    }

    /// A sync payload shaped like the real one: every key Vaultwarden's
    /// `cipherDetails` carries, including the four `null` type objects and
    /// the keys `bw serve` never sends.
    fn sync_fixture(k: &SymmetricKey, protected_user_key: &str) -> serde_json::Value {
        serde_json::json!({
            "object": "sync",
            "profile": { "key": protected_user_key, "privateKey": null, "organizations": [] },
            "folders": [{
                "id": "f1", "name": enc(k, "Work"), "object": "folder",
                "revisionDate": "2020-01-01T00:00:00.000000Z"
            }],
            "ciphers": [{
                "object": "cipherDetails",
                "id": "c1",
                "type": 1,
                "creationDate": "2020-01-01T00:00:00.000000Z",
                "revisionDate": "2021-01-01T00:00:00.000000Z",
                "deletedDate": null,
                "archivedDate": null,
                "reprompt": 1,
                "organizationId": null,
                "key": null,
                "attachments": null,
                "organizationUseTotp": true,
                "collectionIds": [],
                "name": enc(k, "Site"),
                "notes": enc(k, "a note"),
                "fields": [{
                    "name": enc(k, "Recovery code"),
                    "value": enc(k, "s3cret"),
                    "type": 1,
                    "linkedId": null
                }],
                "passwordHistory": [{
                    "password": enc(k, "old-pass"),
                    "lastUsedDate": "2020-06-01T00:00:00.000000Z"
                }],
                "login": {
                    "username": enc(k, "u@example.com"),
                    "password": enc(k, "p4ssw0rd"),
                    "totp": enc(k, "JBSWY3DPEHPK3PXP"),
                    "uri": enc(k, "https://example.com"),
                    "uris": [{ "uri": enc(k, "https://example.com"), "match": null }],
                    "fido2Credentials": [],
                    "passwordRevisionDate": null
                },
                "secureNote": null,
                "card": null,
                "identity": null,
                "sshKey": null,
                "folderId": "f1",
                "favorite": true,
                "edit": true,
                "viewPassword": true,
                "permissions": { "delete": true, "restore": true }
            }]
        })
    }

    fn vault_of(payload: serde_json::Value, master: &MasterKey) -> DecryptedVault {
        let response: SyncResponse = serde_json::from_value(payload).expect("the sync shape");
        decrypt_vault(&response, master).expect("the vault")
    }

    #[test]
    fn a_real_shaped_cipher_decrypts_into_every_modelled_field() {
        let (master, user, protected) = account();
        let vault = vault_of(sync_fixture(&user, &protected), &master);

        assert_eq!(vault.failures, Vec::new(), "a healthy payload produced failures");
        let item = &vault.items[0];
        assert_eq!(item.id, "c1");
        assert_eq!(item.name, "Site");
        assert_eq!(item.notes.as_deref().map(String::as_str), Some("a note"));
        assert_eq!(item.item_type, Some(1));
        assert_eq!(item.folder_id.as_deref(), Some("f1"));
        assert!(item.favorite);

        let login = item.login.as_ref().expect("a login object");
        assert_eq!(login.username.as_deref(), Some("u@example.com"));
        assert_eq!(login.password.as_deref().map(String::as_str), Some("p4ssw0rd"));
        assert_eq!(login.totp.as_deref().map(String::as_str), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(login.uris[0].uri.as_deref(), Some("https://example.com"));
        // `uris[].match` is unmodelled and must ride the entry's own catch-all.
        assert!(login.uris[0].other.contains_key("match"));
        // The API's back-compat duplicate is decrypted too, and stays put.
        assert_eq!(login.other["uri"], serde_json::json!("https://example.com"));

        // The custom field's LABEL is encrypted on this wire, which is the
        // half most easily forgotten.
        assert_eq!(item.fields[0].name.as_deref(), Some("Recovery code"));
        assert_eq!(item.fields[0].value.as_deref().map(String::as_str), Some("s3cret"));
        assert_eq!(item.fields[0].other["type"], serde_json::json!(1));

        assert_eq!(vault.folders[0].name, "Work");
        assert!(vault.folders[0].other.contains_key("revisionDate"));
    }

    /// The keys this app already reads out of `other` must be there and must
    /// mean what they meant over `bw serve`. Named one by one: an assertion
    /// that `other` is non-empty would pass on the wrong keys entirely.
    #[test]
    fn every_unmodelled_key_survives_onto_the_catch_all() {
        let (master, user, protected) = account();
        let vault = vault_of(sync_fixture(&user, &protected), &master);
        let item = &vault.items[0];

        for key in [
            "object",
            "creationDate",
            "revisionDate",
            "deletedDate",
            "archivedDate",
            "reprompt",
            "organizationId",
            "key",
            "attachments",
            "organizationUseTotp",
            "collectionIds",
            "passwordHistory",
            "edit",
            "viewPassword",
            "permissions",
        ] {
            assert!(item.other.contains_key(key), "{key} was dropped on the way through");
        }
        // The app's existing readers of `other` still work on a REST item --
        // which is the whole point of routing them through `other` at all.
        assert!(crate::vault_bridge::reprompt_protected(item));
        assert_eq!(crate::vault_bridge::deleted_date(item), None);
        let history = crate::vault_bridge::password_history(item);
        assert_eq!(history.len(), 1);
        assert_eq!(*history[0].password, "old-pass");
        assert_eq!(history[0].last_used_date.as_deref(), Some("2020-06-01T00:00:00.000000Z"));
    }

    /// A modelled field must appear once, not twice. A key left on the
    /// catch-all *and* in a typed field serializes twice, and `bw` takes the
    /// last one -- the hazard [`VaultItem::other`]'s own doc records.
    #[test]
    fn no_modelled_key_is_duplicated_onto_the_catch_all() {
        let (master, user, protected) = account();
        let vault = vault_of(sync_fixture(&user, &protected), &master);
        let item = &vault.items[0];

        for key in MODELLED_TOP_LEVEL_KEYS {
            assert!(!item.other.contains_key(*key), "{key} is both modelled and on the catch-all");
        }
        for key in ["username", "password", "totp", "uris"] {
            let login = item.login.as_ref().expect("a login");
            assert!(!login.other.contains_key(key), "login.{key} is duplicated");
        }
        // Serializing must not emit a duplicate JSON key either, which is the
        // failure the check above exists to prevent rather than describe.
        // `serde_json`'s object is a map, so a duplicate would appear as one
        // key too many rather than as invalid JSON: the round trip is what
        // catches it.
        let text = serde_json::to_string(item).expect("serializes");
        let back: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(back["name"], serde_json::json!("Site"));
        assert_eq!(back["notes"], serde_json::json!("a note"));
        assert_eq!(back["fields"][0]["name"], serde_json::json!("Recovery code"));
    }

    /// A decrypted item must round-trip through serde unchanged, exactly as
    /// `vault_bridge`'s own capture test demands of a CLI-sourced one. This
    /// is the assertion that a dropped key would fail.
    #[test]
    fn a_decrypted_item_round_trips_through_serde_unchanged() {
        let (master, user, protected) = account();
        let vault = vault_of(sync_fixture(&user, &protected), &master);
        let before = serde_json::to_value(&vault.items[0]).expect("serializes");
        let again: VaultItem = serde_json::from_value(before.clone()).expect("deserializes");
        assert_eq!(before, serde_json::to_value(&again).expect("serializes"));

        // The live control `vault_bridge`'s capture test uses: name the
        // unmodelled keys, so "everything survived" means something.
        let object = before.as_object().expect("an object");
        for key in ["object", "creationDate", "revisionDate", "reprompt", "passwordHistory"] {
            assert!(object.contains_key(key), "{key} did not reach the serialized form");
        }
    }

    /// The `null` type objects the API sends and the CLI does not. They must
    /// not reach `other`, or a REST item and a CLI item of the same content
    /// are different shapes.
    #[test]
    fn the_apis_null_type_objects_do_not_reach_the_catch_all() {
        let (master, user, protected) = account();
        let vault = vault_of(sync_fixture(&user, &protected), &master);
        let item = &vault.items[0];
        for key in ["card", "identity", "sshKey", "secureNote"] {
            assert!(!item.other.contains_key(key), "a null {key} was carried through");
        }
        assert!(item.card.is_none());
        assert!(item.identity.is_none());
        assert!(item.ssh_key.is_none());
    }

    /// A secure note's `secureNote` object is a real discriminator when it is
    /// not null, and must survive: it is the only thing that distinguishes
    /// the type on the wire besides `type`.
    #[test]
    fn a_secure_notes_discriminator_object_survives_but_its_body_is_the_notes_field() {
        let (master, user, protected) = account();
        let vault = vault_of(
            serde_json::json!({
                "profile": { "key": protected },
                "ciphers": [{
                    "id": "n1", "type": 2, "name": enc(&user, "Note"),
                    "notes": enc(&user, "the body"),
                    "secureNote": { "type": 0 }
                }]
            }),
            &master,
        );
        let item = &vault.items[0];
        assert_eq!(item.notes.as_deref().map(String::as_str), Some("the body"));
        assert_eq!(item.other["secureNote"], serde_json::json!({ "type": 0 }));
    }

    /// A cipher with its own `key`: every field is under it, and the vault
    /// key only unwraps it. The control is the second half -- the same
    /// ciphertext under the user key must fail -- so this cannot be passing
    /// on a payload where either key would have worked.
    #[test]
    fn a_cipher_with_its_own_key_decrypts_under_that_key_and_not_the_users() {
        let (master, user, protected) = account();
        let per_cipher = key(9);

        let vault = vault_of(
            serde_json::json!({
                "profile": { "key": protected },
                "folders": [],
                "ciphers": [{
                    "id": "c1", "type": 1,
                    "key": seal(&user, &key_bytes(9)),
                    "name": enc(&per_cipher, "Under its own key"),
                    "login": { "password": enc(&per_cipher, "p") }
                }]
            }),
            &master,
        );
        assert_eq!(vault.failures, Vec::new());
        assert_eq!(vault.items[0].name, "Under its own key");
        assert_eq!(
            vault.items[0].login.as_ref().and_then(|l| l.password.as_deref()).map(String::as_str),
            Some("p")
        );

        // The control: that ciphertext under the user key is unreadable, so
        // `cipher.key` really is load-bearing here.
        assert!(plaintext(&user, &enc(&per_cipher, "Under its own key")).is_err());
    }

    /// One unreadable field loses that field and nothing else. This is the
    /// property that decides whether a single corrupt cipher hides a vault.
    #[test]
    fn one_undecryptable_field_is_recorded_and_the_rest_of_the_item_survives() {
        let (master, user, protected) = account();
        let mut payload = sync_fixture(&user, &protected);
        // A well-formed type-2 EncString whose MAC does not verify.
        payload["ciphers"][0]["notes"] = serde_json::json!(enc(&key(200), "a note"));
        let vault = vault_of(payload, &master);

        let item = &vault.items[0];
        assert_eq!(item.name, "Site", "an unrelated field was lost with the broken one");
        assert!(item.notes.is_none(), "a field that failed to decrypt was not left absent");
        assert_eq!(
            vault.failures,
            vec![DecryptFailure {
                cipher_id: "c1".to_string(),
                field: "notes".to_string(),
                why: CryptoError::MacMismatch,
            }]
        );
    }

    /// The failure must name the field and nothing else. A message carrying
    /// the ciphertext hands whoever reads a log the thing an offline attack
    /// runs against.
    #[test]
    fn a_failure_names_the_field_and_never_the_ciphertext_or_the_plaintext() {
        let (master, user, protected) = account();
        let mut payload = sync_fixture(&user, &protected);
        let bad = enc(&key(200), "a note");
        payload["ciphers"][0]["notes"] = serde_json::json!(bad.clone());
        let vault = vault_of(payload, &master);

        for rendered in [vault.failures[0].to_string(), format!("{:?}", vault.failures[0])] {
            assert!(rendered.contains("notes"), "{rendered}");
            assert!(rendered.contains("c1"), "{rendered}");
            assert!(!rendered.contains(&bad), "the ciphertext reached the error: {rendered}");
            assert!(!rendered.contains("a note"), "a plaintext reached the error: {rendered}");
        }
        // And the vault's own Debug prints counts, not contents.
        let printed = format!("{vault:?}");
        assert!(printed.contains("items: 1"), "{printed}");
        assert!(!printed.contains("p4ssw0rd"), "{printed}");
        // `VaultKeys` too: it holds the user key.
        let keys = VaultKeys {
            user: key(1),
            orgs: vec![("org1".to_string(), key(2))],
        };
        let printed = format!("{keys:?}");
        assert!(printed.contains("redacted"), "{printed}");
        assert!(printed.contains("organisations: 1"), "{printed}");
    }

    /// A plaintext that is not UTF-8 after a MAC that verified is a corrupt
    /// store, and must be refused rather than lossily replaced -- a password
    /// with a replacement character in it will not work and will not look
    /// wrong.
    #[test]
    fn a_decrypted_value_that_is_not_utf8_is_refused_rather_than_mangled() {
        let (master, user, protected) = account();
        let mut payload = sync_fixture(&user, &protected);
        payload["ciphers"][0]["notes"] = serde_json::json!(seal(&user, &[0xff, 0xfe, 0xfd]));
        let vault = vault_of(payload, &master);
        assert!(vault.items[0].notes.is_none());
        assert_eq!(
            vault.failures[0].why,
            CryptoError::Malformed("the plaintext is not UTF-8")
        );
    }

    /// A missing profile key is fatal and must say so rather than producing
    /// an empty vault, which a caller would render as "you have no items".
    #[test]
    fn a_sync_without_a_protected_user_key_is_an_error_not_an_empty_vault() {
        let (master, _, _) = account();
        let response: SyncResponse = serde_json::from_value(
            serde_json::json!({ "profile": {}, "ciphers": [], "folders": [] }),
        )
        .expect("the sync shape");
        assert_eq!(
            decrypt_vault(&response, &master).map(|v| v.items.len()),
            Err(CryptoError::Malformed("the profile carries no protected user key"))
        );
    }

    /// A wrong master password produces a MAC failure on the user key, which
    /// must be one clean error and not a vault of broken items.
    #[test]
    fn the_wrong_master_password_fails_at_the_user_key_and_goes_no_further() {
        let (_, user, protected) = account();
        let wrong =
            master_key(b"not the master", "fixture@example.invalid", Kdf::Pbkdf2 { iterations: 1 })
                .expect("one iteration");
        let response: SyncResponse =
            serde_json::from_value(sync_fixture(&user, &protected)).expect("the sync shape");
        assert_eq!(
            decrypt_vault(&response, &wrong).map(|v| v.items.len()),
            Err(CryptoError::MacMismatch)
        );
    }

    /// A server that omits a whole section must parse, not fail. This is the
    /// tolerance a subset implementation needs: `collections`, `sends`,
    /// `policies` and `domains` are all absent here, and so are `ciphers` and
    /// `folders` themselves.
    #[test]
    fn a_sync_payload_missing_whole_sections_still_parses() {
        let (master, _, protected) = account();
        let response: SyncResponse =
            serde_json::from_value(serde_json::json!({ "profile": { "key": protected } }))
                .expect("a payload with no ciphers or folders key at all");
        let vault = decrypt_vault(&response, &master).expect("the vault");
        assert!(vault.items.is_empty());
        assert!(vault.folders.is_empty());
        assert!(vault.failures.is_empty());
    }

    /// A card, an identity and an SSH key, each with an unmodelled nested key
    /// that must survive -- the rule `CardData`'s and `UriEntry`'s own
    /// `other` fields exist for, applied to a REST payload.
    #[test]
    fn the_other_three_item_types_map_and_keep_their_unmodelled_nested_keys() {
        let (master, user, protected) = account();
        let vault = vault_of(
            serde_json::json!({
                "profile": { "key": protected },
                "folders": [],
                "ciphers": [
                    {
                        "id": "card1", "type": 3, "name": enc(&user, "Visa"),
                        "card": {
                            "cardholderName": enc(&user, "John Doe"),
                            "brand": enc(&user, "Visa"),
                            "number": enc(&user, "4242424242424242"),
                            "expMonth": enc(&user, "04"),
                            "expYear": enc(&user, "2030"),
                            "code": enc(&user, "123"),
                            "somethingNew": { "deep": true }
                        }
                    },
                    {
                        "id": "id1", "type": 4, "name": enc(&user, "Me"),
                        "identity": {
                            "firstName": enc(&user, "Ada"),
                            "lastName": enc(&user, "Lovelace"),
                            "postalCode": enc(&user, "SW1A"),
                            "passportNumber": enc(&user, "P1"),
                            "somethingNew": 1
                        }
                    },
                    {
                        "id": "ssh1", "type": 5, "name": enc(&user, "Key"),
                        "sshKey": {
                            "privateKey": enc(&user, "-----BEGIN-----"),
                            "publicKey": enc(&user, "ssh-ed25519 AAAA"),
                            "keyFingerprint": enc(&user, "SHA256:abc"),
                            "somethingNew": null
                        }
                    }
                ]
            }),
            &master,
        );
        assert_eq!(vault.failures, Vec::new());

        let card = vault.items[0].card.as_ref().expect("a card");
        assert_eq!(card.cardholder_name.as_deref(), Some("John Doe"));
        assert_eq!(card.number.as_deref().map(String::as_str), Some("4242424242424242"));
        // The zero padding is a string on both wires and must stay one.
        assert_eq!(card.exp_month.as_deref(), Some("04"));
        assert_eq!(card.code.as_deref().map(String::as_str), Some("123"));
        assert_eq!(card.other["somethingNew"], serde_json::json!({ "deep": true }));

        let identity = vault.items[1].identity.as_ref().expect("an identity");
        assert_eq!(identity.first_name.as_deref(), Some("Ada"));
        assert_eq!(identity.postal_code.as_deref(), Some("SW1A"));
        assert_eq!(identity.passport_number.as_deref(), Some("P1"));
        assert_eq!(identity.other["somethingNew"], serde_json::json!(1));
        // An identity field the server did not send must stay absent rather
        // than becoming an empty string on the next write.
        assert!(identity.middle_name.is_none());

        let ssh = vault.items[2].ssh_key.as_ref().expect("an ssh key");
        assert_eq!(ssh.private_key.as_deref().map(String::as_str), Some("-----BEGIN-----"));
        assert_eq!(ssh.public_key.as_deref(), Some("ssh-ed25519 AAAA"));
        assert_eq!(ssh.key_fingerprint.as_deref(), Some("SHA256:abc"));
        assert!(ssh.other.contains_key("somethingNew"));
    }

    /// The two constant lists above are the kind of thing that rots: a field
    /// added to the struct and forgotten here becomes a duplicated JSON key.
    /// Checked against `vault_bridge.rs`'s own source rather than a second
    /// hand-written list.
    #[test]
    fn the_modelled_key_lists_match_the_structs_they_describe() {
        let source = include_str!("../vault_bridge.rs");
        // Positive control: the parse really read `vault_bridge.rs`, so every
        // check below is not vacuously passing on an empty string.
        assert!(source.contains("pub struct IdentityData"), "the source was not found");

        for wire in IDENTITY_WIRE_KEYS {
            let snake = to_snake(wire);
            assert!(
                source.contains(&format!("rename = \"{wire}\""))
                    || source.contains(&format!("pub {snake}: Option<String>")),
                "{wire} is stripped from the identity catch-all but is not a field of \
                 IdentityData"
            );
        }
        // `IdentityData` models eighteen fields plus `other`; a nineteenth
        // added without touching this list would be silently duplicated.
        assert_eq!(IDENTITY_WIRE_KEYS.len(), 18);
        assert_eq!(MODELLED_TOP_LEVEL_KEYS.len(), 11);
    }

    /// `postalCode` -> `postal_code`, for the check above only.
    fn to_snake(camel: &str) -> String {
        let mut out = String::new();
        for c in camel.chars() {
            if c.is_ascii_uppercase() {
                out.push('_');
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// An organisation cipher, decrypted through the RSA-wrapped
    /// organisation key.
    ///
    /// # This one has real external ground truth, and it is the only one here
    /// that does
    ///
    /// The private key and the wrapped organisation key are **OpenSSL's**,
    /// transcribed in `crypto.rs` -- and OpenSSL's plaintext for that
    /// ciphertext is the 64 bytes `00 01 .. 3f`, which is exactly the shape
    /// an organisation key has. So the whole chain below except the outermost
    /// AES layer is checked against an implementation that is not this one.
    ///
    /// **What it is still not**: an organisation cipher from a real server.
    /// The account this backend was written for runs a self-hosted server
    /// that does not implement organisations at all, so no live payload will
    /// ever exercise this path, and that gap is named rather than implied.
    #[test]
    fn an_organisation_cipher_decrypts_through_the_rsa_wrapped_org_key() {
        let (master, user, protected) = account();
        // The org key OpenSSL's ciphertext actually contains.
        let mut org_bytes = [0u8; 64];
        for (i, b) in org_bytes.iter_mut().enumerate() {
            *b = u8::try_from(i).expect("under 64");
        }
        let org_key = key_from_64(&org_bytes);

        let vault = vault_of(
            serde_json::json!({
                "profile": {
                    "key": protected,
                    "privateKey": seal(&user, &hex(ORG_KEY_PRIVATE_PKCS8_DER)),
                    "organizations": [{
                        "id": "org1",
                        "key": format!("4.{}", base64(&hex(ORG_KEY_WRAPPED_OAEP_SHA1)))
                    }]
                },
                "folders": [],
                "ciphers": [{
                    "id": "oc1", "type": 1, "organizationId": "org1",
                    "name": enc(&org_key, "Shared login"),
                    "login": { "password": enc(&org_key, "shared") }
                }]
            }),
            &master,
        );
        assert_eq!(vault.failures, Vec::new());
        assert_eq!(vault.items[0].name, "Shared login");
        assert_eq!(
            vault.items[0].login.as_ref().and_then(|l| l.password.as_deref()).map(String::as_str),
            Some("shared")
        );
        // Control: the user key does not open an organisation cipher, so the
        // RSA hop above is really being taken.
        assert!(plaintext(&user, &enc(&org_key, "Shared login")).is_err());
    }

    /// An organisation whose key will not unwrap must not take the personal
    /// vault down with it.
    ///
    /// Both ways it can go wrong are covered, and they are recorded at
    /// **different** field names on purpose -- "the RSA key itself is
    /// unreadable" and "one organisation's key does not open" are different
    /// problems with different remedies, and a single message for both would
    /// send a reader looking in the wrong place.
    #[test]
    fn an_unreadable_organisation_key_is_recorded_and_personal_items_still_load() {
        let (master, user, protected) = account();
        let personal = serde_json::json!([{ "id": "p1", "type": 1, "name": enc(&user, "Personal") }]);

        // (a) The private key decrypts but is not a key: the failure is the
        //     organisation's, because that is where the unwrap is attempted.
        let vault = vault_of(
            serde_json::json!({
                "profile": {
                    "key": protected,
                    "privateKey": seal(&user, b"not a PKCS#8 key at all"),
                    "organizations": [{ "id": "org1", "key": "4.AAAA" }]
                },
                "ciphers": personal.clone()
            }),
            &master,
        );
        assert_eq!(vault.items.len(), 1);
        assert_eq!(vault.items[0].name, "Personal");
        assert_eq!(vault.failures.len(), 1);
        assert_eq!(vault.failures[0].field, "organizations[].key");
        assert_eq!(vault.failures[0].cipher_id, "org1");

        // (b) The private key does not decrypt at all -- wrapped under
        //     something that is not the user key. One failure, named at the
        //     private key, and no per-organisation noise on top of it.
        let vault = vault_of(
            serde_json::json!({
                "profile": {
                    "key": protected,
                    "privateKey": seal(&key(77), b"whatever"),
                    "organizations": [{ "id": "org1", "key": "4.AAAA" }]
                },
                "ciphers": personal
            }),
            &master,
        );
        assert_eq!(vault.items.len(), 1);
        assert_eq!(vault.failures.len(), 1);
        assert_eq!(vault.failures[0].field, "profile.privateKey");
        assert_eq!(vault.failures[0].why, CryptoError::MacMismatch);
    }

    /// A cipher belonging to an organisation whose key never arrived is
    /// dropped with a recorded reason -- not emitted with an empty name,
    /// which would look like a real but blank item.
    #[test]
    fn a_cipher_of_an_unknown_organisation_is_dropped_with_a_reason() {
        let (master, user, protected) = account();
        let vault = vault_of(
            serde_json::json!({
                "profile": { "key": protected },
                "folders": [],
                "ciphers": [{ "id": "oc1", "type": 1, "organizationId": "ghost",
                              "name": enc(&user, "Unreachable") }]
            }),
            &master,
        );
        assert!(vault.items.is_empty());
        assert_eq!(vault.failures.len(), 1);
        assert_eq!(vault.failures[0].cipher_id, "oc1");
        assert_eq!(vault.failures[0].field, "key");
    }

    /// A cipher with no `id` has nothing this app could ever write back to,
    /// and must be dropped rather than given a synthesised one.
    #[test]
    fn a_cipher_with_no_id_is_dropped_rather_than_given_one() {
        let (master, user, protected) = account();
        let vault = vault_of(
            serde_json::json!({
                "profile": { "key": protected },
                "ciphers": [{ "type": 1, "name": enc(&user, "No id") }, "not an object"]
            }),
            &master,
        );
        assert!(vault.items.is_empty());
    }
}

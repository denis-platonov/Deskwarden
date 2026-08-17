//! A whole credential record, written into a Bitwarden Send and read back out
//! of one.
//!
//! Two halves, both pure and both free of I/O:
//!
//!  * [`payload`] — the [`Record`] type, its **versioned** JSON writer, and a
//!    deliberately strict reader. The writer is hand-rolled into a
//!    [`Zeroizing`](zeroize::Zeroizing) buffer rather than derived through
//!    `serde_json::to_string`, which would allocate the secret body into an
//!    ordinary `String` and hand it back to the allocator unwiped.
//!  * [`seal`] — passphrase sealing of the TOTP seed, and only the seed.
//!
//! **Why the seed is sealed twice over and nothing else is.** A Send's content
//! is protected by the fragment key, and that key is *in the link*: whoever has
//! the link has the content. For a username and a password that is the bargain
//! already accepted by sending them, because both can be rotated. A TOTP seed
//! cannot — "rotating" it means re-enrolling the second factor with the
//! service, which this app can neither do nor offer — so "whoever has the link"
//! is too weak a gate for it. The passphrase layer makes the link alone
//! insufficient, **but only if the passphrase travels out of band.**
//!
//! **Everything in a payload is data.** No field here is a command, a path, a
//! URL to fetch, or a key sequence to type. `notes` is text to store. Nothing
//! in this module interprets, opens or runs anything, and
//! `payload::tests::a_notes_field_that_looks_like_a_key_sequence_is_stored_as_text`
//! is that rule made checkable rather than merely stated.

pub mod payload;
pub mod seal;

pub use payload::{
    read_json, write_json, Record, RecordRefusal, MAX_PAYLOAD_BYTES, RECORD_FORMAT, RECORD_VERSION,
};
pub use seal::{seal, unseal, SealFailed, SealedSeed};

use crate::vault_bridge::VaultItem;

/// Which of a vault item's fields the sender ticked.
///
/// Every field defaults to **not sent**. That is the safe direction: a field
/// added here later is absent from every existing caller's record until
/// somebody deliberately ticks it, rather than silently joining the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecordSelection {
    pub username: bool,
    pub password: bool,
    pub uri: bool,
    pub notes: bool,
    pub totp: bool,
}

/// The TOTP seed as it is handed to [`record_from`] — **seed and passphrase
/// together, or neither.**
///
/// This is the shape of the module's load-bearing rule rather than a branch
/// inside it. The failure to prevent is "the user ticked TOTP, set no
/// passphrase, and we sent the seed anyway", and the way to prevent it is not
/// to check for it: it is to leave no arm of this type in which a seed exists
/// without the passphrase it will be sealed under. There is no
/// `Sealed { seed }`, so a caller cannot pass a seed and forget the
/// passphrase — the only way to have a seed with nothing to seal it under is
/// [`TotpToSend::None`], which carries no seed at all.
///
/// The two live together in one arm for the same reason a `SealedSeed` and not
/// a `String` sits in [`Record::totp_sealed`]: an unsealed seed is not a state
/// the type system should let anybody reach.
///
/// **An empty passphrase is still a passphrase here.** Refusing one is the
/// send surface's job (it disables the button), because this function returns
/// a `Record` and has no vocabulary for a refusal; making one up would put the
/// decision in two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpToSend<'a> {
    /// No seed travels. Either the sender did not tick TOTP, or there is no
    /// passphrase to seal one under.
    None,
    /// A seed, and the passphrase it will be sealed under. Both, always.
    Sealed { seed: &'a str, passphrase: &'a str },
}

/// Builds the record that will travel, from a vault item and what the sender
/// ticked.
///
/// Pure: no I/O, no clock, no vault. The only expensive thing it can do is one
/// Argon2id derivation, and only on the [`TotpToSend::Sealed`] arm.
///
/// **A ticked-but-empty field is absent, not empty.** `Some("")` on the wire
/// imports as a username of `""`, which renders as a blank the recipient never
/// chose and, on a collision replace, overwrites something real with nothing.
/// See `payload.rs`'s module docs: absence is the representation of "not
/// sent", and an empty value is not a value the sender meant to send.
pub fn record_from(
    item: &VaultItem,
    sel: &RecordSelection,
    totp: TotpToSend<'_>,
    not_after: Option<String>,
) -> Record {
    let login = item.login.as_ref();
    let pick = |ticked: bool, value: Option<&str>| -> Option<String> {
        if !ticked {
            return None;
        }
        value.filter(|v| !v.is_empty()).map(str::to_string)
    };

    Record {
        name: item.name.clone(),
        username: pick(sel.username, login.and_then(|l| l.username.as_deref())),
        password: if sel.password {
            login
                .and_then(|l| l.password.as_deref())
                .filter(|v| !v.is_empty())
                .map(|v| zeroize::Zeroizing::new(v.to_string()))
        } else {
            None
        },
        uri: pick(sel.uri, login.and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref())),
        notes: pick(sel.notes, item.notes.as_deref().map(String::as_str)),
        // The tick and the seed are two separate conditions and both must
        // hold. Neither one alone can put a seed on the wire.
        totp_sealed: match (sel.totp, totp) {
            (true, TotpToSend::Sealed { seed, passphrase }) => Some(seal::seal(seed, passphrase)),
            (true, TotpToSend::None) | (false, _) => None,
        },
        not_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A login item carrying a username, a password, a URI **and** a seed.
    ///
    /// It carries all four so that every "this did not reach the record"
    /// assertion below is about the selection rather than about an empty
    /// fixture. Built through `serde_json` rather than a struct literal so it
    /// stays compiling as `VaultItem` gains fields — the same route
    /// `app.rs`'s `login_item` takes.
    fn item_with(username: &str, password: &str, uri: &str, seed: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": "item-1",
            "name": "SAP Production",
            "type": 1,
            "notes": "a note the sender wrote",
            "login": {
                "username": username,
                "password": password,
                "totp": seed,
                "uris": [{ "uri": uri }],
            },
        }))
        .expect("the fixture is a literal in this file and must parse as a VaultItem")
    }

    #[test]
    fn an_unticked_field_never_reaches_the_record() {
        let item = item_with("dplatonov", "hunter2", "https://sap.example", "JBSWY3DPEHPK3PXP");
        let sel = RecordSelection { username: true, ..RecordSelection::default() };
        let record = record_from(&item, &sel, TotpToSend::None, None);

        // Positively, first: the ticked field and the name really are there,
        // so the absences below are not the absences of an empty record.
        assert_eq!(record.name, "SAP Production");
        assert_eq!(record.username.as_deref(), Some("dplatonov"));

        assert!(record.password.is_none(), "an unticked password reached the record");
        assert!(record.uri.is_none(), "an unticked uri reached the record");
        assert!(record.notes.is_none(), "unticked notes reached the record");
        assert!(record.totp_sealed.is_none(), "an unticked seed reached the record");

        // Control: the item really does carry all four, so `is_none` above is
        // about the selection and not about a fixture with nothing in it.
        let login = item.login.as_ref().expect("the fixture has a login object");
        assert_eq!(login.password.as_deref().map(String::as_str), Some("hunter2"));
        assert_eq!(login.uris.first().and_then(|u| u.uri.as_deref()), Some("https://sap.example"));
        assert_eq!(login.totp.as_deref().map(String::as_str), Some("JBSWY3DPEHPK3PXP"));
        assert!(item.notes.is_some());

        // And the same control at the wire, where it matters: none of the
        // unticked values are anywhere in what would be sent.
        let json = write_json(&record);
        let json = json.as_str();
        assert!(json.contains("\"username\":\"dplatonov\""), "{json}");
        for unticked in ["hunter2", "https://sap.example", "a note the sender wrote"] {
            assert!(!json.contains(unticked), "an unticked value reached the payload: {json}");
        }
    }

    #[test]
    fn ticking_totp_without_a_passphrase_cannot_produce_a_bare_seed() {
        // The load-bearing one. A seed must never be written unsealed, and the
        // failure mode to prevent is "user ticked TOTP, no passphrase set, we
        // sent it anyway". `TotpToSend` is what makes the second half of that
        // sentence unwritable: with no passphrase there is no arm to put the
        // seed in, so the sender's tick has nothing to act on.
        const SEED: &str = "JBSWY3DPEHPK3PXP";
        let item = item_with("dplatonov", "hunter2", "https://sap.example", SEED);
        let sel = RecordSelection { totp: true, ..RecordSelection::default() };

        let record = record_from(&item, &sel, TotpToSend::None, None);
        assert!(
            record.totp_sealed.is_none(),
            "a seed was included with no passphrase to seal it under"
        );
        // Asserted on the rendered JSON, not just the struct: the struct
        // cannot hold a bare seed by construction, so only the payload can
        // show that no other field carried it out instead.
        let json = write_json(&record);
        let json = json.as_str();
        assert!(!json.contains(SEED), "the bare seed reached the payload: {json}");
        // Positively: it is a real record of this item, not an empty one that
        // trivially contains no seed.
        assert!(json.contains("\"name\":\"SAP Production\""), "{json}");

        // The control that makes all of the above mean something: with a
        // passphrase, this same call DOES seal the seed. One Argon2id
        // derivation, ~0.7 s in debug -- the only one in this module.
        let sealed = record_from(
            &item,
            &sel,
            TotpToSend::Sealed { seed: SEED, passphrase: "correct horse battery staple" },
            None,
        );
        let sealed_seed =
            sealed.totp_sealed.as_ref().expect("a seed WITH a passphrase must be sealed and sent");
        assert_eq!(
            seal::unseal(sealed_seed, "correct horse battery staple").as_deref().map(String::as_str),
            Ok(SEED),
            "the sealed seed is not the item's seed"
        );
        let sealed_json = write_json(&sealed);
        let sealed_json = sealed_json.as_str();
        assert!(sealed_json.contains("\"totp_sealed\""), "{sealed_json}");
        assert!(
            !sealed_json.contains(SEED),
            "even the sealing path wrote the seed in the clear: {sealed_json}"
        );
    }

    #[test]
    fn a_ticked_but_empty_field_is_absent_rather_than_empty() {
        // `Some("")` on the wire imports as a username of "", which renders as
        // a blank the recipient never chose and, on a collision replace,
        // overwrites something real with nothing.
        let item = item_with("", "", "", "");
        let sel = RecordSelection {
            username: true,
            password: true,
            uri: true,
            notes: false,
            totp: false,
        };
        let record = record_from(&item, &sel, TotpToSend::None, None);
        assert!(record.username.is_none(), "an empty username was sent as an empty string");
        assert!(record.password.is_none());
        assert!(record.uri.is_none());
        let json = write_json(&record);
        let json = json.as_str();
        assert!(!json.contains("\"username\""), "{json}");

        // Control: ticked and non-empty, the very same call does send them.
        let full = item_with("dplatonov", "hunter2", "https://sap.example", "");
        let record = record_from(&full, &sel, TotpToSend::None, None);
        assert_eq!(record.username.as_deref(), Some("dplatonov"));
        assert_eq!(record.password.as_deref().map(String::as_str), Some("hunter2"));
        assert_eq!(record.uri.as_deref(), Some("https://sap.example"));
    }

    #[test]
    fn a_seed_without_a_passphrase_is_unexpressible_and_not_merely_guarded() {
        // The property this module's shape is for is a compile-time one, and
        // no runtime call can demonstrate a call that does not compile. So it
        // is pinned at the source, the way this crate pins its other absences:
        // if `record_from` ever takes a seed and a passphrase as two separate
        // arguments again, a caller can pass the first and forget the second,
        // and this reds.
        let whole = include_str!("mod.rs").replace("\r\n", "\n");
        let code = whole.split(concat!("#[cfg(test)]", "\nmod ")).next().unwrap();
        assert!(
            code.len() < whole.len(),
            "the test-module marker was not found, so the pins below read this test's own text"
        );

        let signature = code
            .split_once("pub fn record_from(")
            .expect("control: `record_from` was not found, so the pins below are vacuous")
            .1
            .split_once(") -> Record {")
            .expect("`record_from`'s signature did not end where expected")
            .0;
        assert!(
            signature.contains("totp: TotpToSend<"),
            "the seed no longer arrives as one `TotpToSend` value: {signature}"
        );
        assert!(
            !signature.contains("seed") && !signature.contains("passphrase"),
            "`record_from` takes a seed or a passphrase as a parameter of its own, so a caller \
             can pass one and forget the other: {signature}"
        );

        let variants = code
            .split_once("pub enum TotpToSend<'a> {")
            .expect("control: `TotpToSend` was not found, so the pin below is vacuous")
            .1
            .split_once("\n}")
            .expect("`TotpToSend`'s body did not end where expected")
            .0;
        // Declaration lines only: the doc comments on the arms discuss the
        // seed in prose, and prose is not what carries one.
        let declarations =
            variants.lines().filter(|l| !l.trim_start().starts_with("///") && l.contains("seed"));
        assert!(
            declarations.clone().count() > 0,
            "control: no arm of `TotpToSend` declares a seed at all, so the pin below is vacuous"
        );
        for line in declarations {
            assert!(
                line.contains("passphrase"),
                "a `TotpToSend` variant carries a seed without the passphrase to seal it \
                 under, which is the one state this type exists to make unreachable: {line}"
            );
        }
    }
}

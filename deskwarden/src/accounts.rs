//! The account model and every path derived from it.
//!
//! One account is one Bitwarden login. Each lives in its own directory under
//! `<config_dir>\accounts\<account-id>\`, holds its own `session.bin` and
//! `hello.bin`, and is reached by pointing the Bitwarden CLI's
//! `BITWARDENCLI_APPDATA_DIR` at that directory.
//!
//! **All accounts are symmetric.** There is no "the first one is special"
//! variant and no `AccountLocation` enum: the pre-existing profile is migrated
//! into this layout like any other. The pre-migration state is the *absence*
//! of an account list — a startup condition that ends the moment migration
//! succeeds — not a kind of account. So [`data_dir_for`] returns a plain
//! `PathBuf`; there is no account whose directory is "wherever the CLI would
//! have put it".
//!
//! **The id is opaque and generated, never derived.** [`AccountId::generate`]
//! takes no arguments, so an id cannot be a function of the email — the
//! directory name must not disclose whose vault it is to anyone who lists
//! `%APPDATA%`. And an id becomes a directory name that later code will
//! `remove_dir_all`, so [`AccountId::parse`] is the only way to build one from
//! untrusted text (a hand-edited `settings.json`), and it accepts exactly 32
//! lowercase hex characters. That single rule is what makes `..`, an absolute
//! path, a separator of either flavour, and a reserved Windows device name all
//! unrepresentable rather than merely unlikely.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

/// The number of characters in an account id: 16 random bytes, hex-encoded.
const ID_LEN: usize = 32;

/// An opaque per-account identifier, and the name of that account's directory.
///
/// The inner `String` is private and the only two ways to obtain one are
/// [`AccountId::generate`] and [`AccountId::parse`], so no value of this type
/// can name anything but a single 32-character leaf directory. Deserialization
/// goes through `parse` as well (see the hand-written `Deserialize` impl below
/// — a derived one on a transparent newtype would accept whatever string was
/// in the file).
///
/// Deliberately **no** `#[serde(transparent)]`: serde's derive already
/// serializes a newtype as its inner value, so the attribute is a no-op here.
/// It was removed after a mutation run showed it could be deleted with the
/// whole suite still green — a decoration a later reader would have taken for
/// a load-bearing guarantee. What actually holds the wire format is
/// `an_id_serializes_as_a_bare_string_so_settings_json_stays_readable`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct AccountId(String);

impl AccountId {
    /// A fresh random id: 16 bytes from the OS CSPRNG, lowercase hex.
    ///
    /// Takes no arguments *by design*. An id derived from the email — even
    /// hashed — would let anyone who can list the accounts directory confirm
    /// a guess at which account is enrolled, and the spec requires the
    /// directory name disclose nothing about whose vault it is.
    pub fn generate() -> Self {
        use std::fmt::Write as _;

        let mut bytes = [0u8; ID_LEN / 2];
        getrandom::getrandom(&mut bytes)
            .expect("the OS must be able to produce 16 random bytes for an account id");
        let mut hex = String::with_capacity(ID_LEN);
        for b in bytes {
            // Writing to a `String` is infallible; there is no error to handle.
            let _ = write!(hex, "{b:02x}");
        }
        Self(hex)
    }

    /// Parses an id that came from somewhere untrusted — `settings.json`, a
    /// migration marker, a directory listing.
    ///
    /// Accepts exactly 32 lowercase hex characters and nothing else. That is
    /// deliberately far narrower than "a valid filename": this string is
    /// joined onto the accounts root and the result is created, written into,
    /// and eventually `remove_dir_all`'d. `..`, `../evil`, `..\evil`, `C:\`,
    /// `CON`, `NUL`, `COM1`, a trailing dot or space — every one of them is
    /// rejected by the same rule, so there is no list of special cases to keep
    /// in sync with Windows.
    pub fn parse(raw: &str) -> Option<Self> {
        let ok = raw.len() == ID_LEN && raw.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
        ok.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AccountId {
    /// Hand-written rather than derived so a stored id goes through
    /// [`AccountId::parse`]. `#[derive(Deserialize)]` on a transparent newtype
    /// would take any string at all, which would mean a hand-edited
    /// `settings.json` could name a directory outside the accounts root.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        AccountId::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{raw:?} is not an account id: an id is exactly 32 lowercase hex characters, \
                 because it names a directory Deskwarden creates and deletes"
            ))
        })
    }
}

/// One configured Bitwarden account.
///
/// The data directory is **not** a field: it is derived from the id by
/// [`data_dir_for`] every time it is needed. Persisting it would let a
/// hand-edited or stale `settings.json` point an account at a directory the
/// app never created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub email: String,
    /// A self-hosted server URL, or `None` for bitwarden.com.
    pub server_url: Option<String>,
}

/// `<config_dir>\accounts` — the one directory every account lives under.
pub fn accounts_root(config_dir: &Path) -> PathBuf {
    config_dir.join("accounts")
}

/// The account's CLI profile directory: what `BITWARDENCLI_APPDATA_DIR` is
/// pointed at while this account is active.
pub fn data_dir_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    accounts_root(config_dir).join(id.as_str())
}

/// The account's DPAPI-wrapped session token.
///
/// Built from [`data_dir_for`], not from `config_dir` directly, so the layout
/// has exactly one definition. Before this feature the file lived directly in
/// `config_dir`; if any account's copy resolved back there, every other
/// account would find, overwrite and delete it.
pub fn session_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    data_dir_for(config_dir, id).join("session.bin")
}

/// The account's Windows Hello quick-unlock blob. Same reasoning as
/// [`session_path_for`].
pub fn hello_blob_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    data_dir_for(config_dir, id).join("hello.bin")
}

/// Mixed into `hello`'s existing domain-separation label so **one** Windows
/// Hello credential seals a distinct key per account.
///
/// One shared credential is not an optimisation, it is the constraint:
/// `KeyCredentialManager::RequestCreateAsync(ReplaceExisting)` rotates the
/// credential and would destroy every *other* account's enrolment, so it is
/// banned and the accounts are separated by this label instead.
///
/// Never empty, for every account including the first. An empty suffix would
/// reproduce the derivation used before this feature existed, which would mean
/// a `hello.bin` left over from before the migration could still be opened —
/// under whichever account happened to have the empty suffix. Quick unlock is
/// therefore re-enrolled per account after migration, which is why the
/// migration deletes the pre-migration blob and tells the user so.
pub fn hello_kdf_suffix_for(id: &AccountId) -> Vec<u8> {
    let mut suffix = b" account ".to_vec();
    suffix.extend_from_slice(id.as_str().as_bytes());
    suffix
}

/// The configured account with this id, if any.
pub fn account_for<'a>(accounts: &'a [Account], id: &AccountId) -> Option<&'a Account> {
    accounts.iter().find(|a| &a.id == id)
}

/// Which account becomes active when `removed` is deleted: the first survivor
/// in configured order, or `None` when it was the last one.
///
/// Never the account being removed — that is the whole point. Returning it
/// would leave the app pointed at a directory that is about to be deleted.
pub fn next_active_after_removal<'a>(
    accounts: &'a [Account],
    removed: &AccountId,
) -> Option<&'a Account> {
    accounts.iter().find(|a| &a.id != removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn id(s: &str) -> AccountId {
        AccountId::parse(s).unwrap_or_else(|| panic!("{s:?} should be a valid id"))
    }

    fn account(raw: &str) -> Account {
        Account {
            id: id(raw),
            email: "me@example.com".to_string(),
            server_url: None,
        }
    }

    // ---------------------------------------------------------------- 2.1

    #[test]
    fn a_generated_id_is_thirty_two_lowercase_hex_characters_and_not_an_email() {
        let id = AccountId::generate();
        assert_eq!(id.as_str().len(), 32, "got {id}");
        assert!(
            id.as_str()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "got {id}"
        );
        assert!(
            !id.as_str().contains('@'),
            "the directory name must not disclose whose vault it is, got {id}"
        );
        // And it survives its own validator, so a generated id can always be
        // written to settings.json and read back.
        assert_eq!(AccountId::parse(id.as_str()).as_ref(), Some(&id));
    }

    #[test]
    fn generated_ids_are_random_rather_than_a_constant_or_a_counter() {
        // `generate()` takes no arguments, so the type system already rules
        // out an id derived from the email. What it does not rule out is a
        // constant (every account would share one directory) or a zero-padded
        // counter (ids enumerable, and two installs colliding). Sixteen draws:
        // all distinct kills the constant, and a varying FIRST character kills
        // `format!("{n:032x}")`, whose leading character is always '0'.
        let ids: Vec<AccountId> = (0..16).map(|_| AccountId::generate()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "generate() repeated itself: {ids:?}");

        let first_chars: std::collections::BTreeSet<char> =
            ids.iter().filter_map(|i| i.as_str().chars().next()).collect();
        assert!(
            first_chars.len() > 1,
            "every generated id starts with the same character, which is what a zero-padded \
             counter looks like: {ids:?}"
        );
    }

    #[test]
    fn two_generated_ids_differ() {
        assert_ne!(AccountId::generate(), AccountId::generate());
    }

    #[test]
    fn parse_rejects_anything_that_could_escape_the_accounts_directory() {
        // This id becomes a directory name, and the migration (and account
        // removal) will `remove_dir_all` a path built from it. A traversal, a
        // separator, a drive-absolute path or a reserved device name reaching
        // `data_dir_for` would put -- or DELETE -- something somewhere else
        // entirely.
        let bad = [
            // traversal, in both separator flavours
            "..",
            "../evil",
            r"..\evil",
            "../../../../windows/system32",
            r"..\..\..\..\windows\system32",
            // separators inside an otherwise plausible id
            "0123456789abcdef/0123456789abcde",
            r"0123456789abcdef\0123456789abcde",
            // absolute paths
            r"C:\evil",
            "C:/evil",
            r"\\server\share",
            "/etc/passwd",
            // reserved Windows device names, with and without extensions
            "CON",
            "con",
            "NUL",
            "nul",
            "COM1",
            "LPT1",
            "aux.txt",
            // trailing dot / space, which Windows silently strips
            "0123456789abcdef0123456789abcde.",
            "0123456789abcdef0123456789abcde ",
            " 0123456789abcdef0123456789abcde",
            // empty, and the current directory
            "",
            ".",
            // right shape, wrong alphabet or length
            "abc",
            "a@b.c",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdefg123456789abcdef",
            // 32 characters, but not 32 bytes of hex
            "0123456789abcdef0123456789abcd\u{00e9}",
        ];
        for raw in bad {
            assert!(AccountId::parse(raw).is_none(), "accepted {raw:?}");
        }
        // Positive controls on the same function: it is not simply returning
        // `None` for everything.
        assert!(AccountId::parse(A).is_some());
        assert!(AccountId::parse(&"0".repeat(32)).is_some());
        assert!(AccountId::parse(&"f".repeat(32)).is_some());
        assert!(AccountId::parse(AccountId::generate().as_str()).is_some());
    }

    #[test]
    fn a_hand_edited_settings_id_that_escapes_the_directory_does_not_deserialize() {
        // The wiring, not the decision: `parse` can be perfect and a DERIVED
        // `Deserialize` on a transparent newtype would still let this through.
        for raw in [r#""..""#, r#""../..""#, r#""CON""#, r#""""#, r#""C:\\evil""#] {
            assert!(
                serde_json::from_str::<AccountId>(raw).is_err(),
                "deserialized {raw}"
            );
        }
        assert_eq!(
            serde_json::from_str::<AccountId>(&format!("\"{A}\"")).unwrap(),
            id(A)
        );
        // And the rejection says what is wrong, so a user who hand-edited the
        // file can fix it.
        let err = serde_json::from_str::<AccountId>(r#""..""#).unwrap_err();
        assert!(err.to_string().contains("32"), "got: {err}");
    }

    #[test]
    fn an_id_serializes_as_a_bare_string_so_settings_json_stays_readable() {
        // Pins the wire format Task 5's settings file will hold: a bare JSON
        // string. Verified failable by mutation -- a `Serialize` that emits
        // `{"value":"..."}`, or one that "normalises" the id to uppercase,
        // both fail here and in the round-trip below. (It does NOT pin
        // `#[serde(transparent)]`; that attribute is a no-op for a newtype and
        // has been removed rather than left looking load-bearing.)
        assert_eq!(serde_json::to_string(&id(A)).unwrap(), format!("\"{A}\""));
        assert_eq!(
            serde_json::from_str::<AccountId>(&serde_json::to_string(&id(A)).unwrap()).unwrap(),
            id(A)
        );
    }

    #[test]
    fn an_account_round_trips_through_the_exact_json_settings_will_hold() {
        let stored = Account {
            id: id(A),
            email: "me@example.com".to_string(),
            server_url: Some("https://vault.example.com".to_string()),
        };
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            format!(
                "{{\"id\":\"{A}\",\"email\":\"me@example.com\",\
                 \"server_url\":\"https://vault.example.com\"}}"
            )
        );
        assert_eq!(
            serde_json::from_str::<Account>(&serde_json::to_string(&stored).unwrap()).unwrap(),
            stored
        );
        // A self-hosted URL is optional, not absent-meaning-empty.
        assert_eq!(
            serde_json::to_string(&account(A)).unwrap(),
            format!("{{\"id\":\"{A}\",\"email\":\"me@example.com\",\"server_url\":null}}")
        );
        // And a stored account carrying an escaping id is rejected as a whole,
        // not silently loaded with a dangerous directory name.
        assert!(serde_json::from_str::<Account>(
            r#"{"id":"../..","email":"me@example.com","server_url":null}"#
        )
        .is_err());
    }

    // ---------------------------------------------------------------- 2.2

    #[test]
    fn an_accounts_paths_all_live_under_its_own_directory() {
        // Literal expectations throughout: building the expected path with the
        // same `join` chain the production code uses would pass for any layout
        // at all.
        let cfg = Path::new(r"C:\cfg");
        let a = id(A);
        assert_eq!(accounts_root(cfg), PathBuf::from(r"C:\cfg\accounts"));
        assert_eq!(
            data_dir_for(cfg, &a),
            PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            session_path_for(cfg, &a),
            PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef\session.bin")
        );
        assert_eq!(
            hello_blob_path_for(cfg, &a),
            PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef\hello.bin")
        );
        // Component-wise too, so a layout with the right string but an extra
        // level (`accounts\0123...\0123...\session.bin`) cannot pass.
        assert_eq!(
            data_dir_for(cfg, &a)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "C:".to_string(),
                "\\".to_string(),
                "cfg".to_string(),
                "accounts".to_string(),
                A.to_string()
            ]
        );
    }

    #[test]
    fn the_secret_files_are_children_of_the_account_directory_and_nothing_else() {
        // Pins the composition: both blob paths are `data_dir_for(..)` plus one
        // leaf. A second copy of the layout expression inside
        // `session_path_for` would drift the first time the layout changed.
        let cfg = Path::new(r"C:\cfg");
        for a in [id(A), id(B), AccountId::generate()] {
            let dir = data_dir_for(cfg, &a);
            assert_eq!(session_path_for(cfg, &a).parent(), Some(dir.as_path()));
            assert_eq!(hello_blob_path_for(cfg, &a).parent(), Some(dir.as_path()));
            assert_eq!(
                session_path_for(cfg, &a).file_name(),
                Some(std::ffi::OsStr::new("session.bin"))
            );
            assert_eq!(
                hello_blob_path_for(cfg, &a).file_name(),
                Some(std::ffi::OsStr::new("hello.bin"))
            );
            assert_eq!(dir.file_name(), Some(std::ffi::OsStr::new(a.as_str())));
        }
    }

    #[test]
    fn no_secret_of_any_account_lands_in_the_shared_config_directory() {
        // The pre-migration app kept `session.bin` and `hello.bin` directly in
        // `config_dir`. After migration nothing does -- if one account's blob
        // resolved back to the shared directory it would be found (and
        // deleted, and overwritten) by every other account.
        let cfg = Path::new(r"C:\cfg");
        for raw in [A, B, &"0".repeat(32), &"f".repeat(32)] {
            let a = id(raw);
            assert_ne!(session_path_for(cfg, &a), PathBuf::from(r"C:\cfg\session.bin"));
            assert_ne!(hello_blob_path_for(cfg, &a), PathBuf::from(r"C:\cfg\hello.bin"));
            assert!(session_path_for(cfg, &a).starts_with(accounts_root(cfg)));
            assert!(hello_blob_path_for(cfg, &a).starts_with(accounts_root(cfg)));
            assert!(data_dir_for(cfg, &a).starts_with(accounts_root(cfg)));
            // Positive control on `starts_with` itself, which answers `true`
            // for a prefix that is a whole component: the shared config
            // directory is NOT under the accounts root, so the assertions
            // above are discriminating rather than vacuous.
            assert!(!PathBuf::from(r"C:\cfg\session.bin").starts_with(accounts_root(cfg)));
        }
    }

    #[test]
    fn every_account_directory_is_a_single_leaf_under_the_accounts_root() {
        // The security property, stated over the values that can actually
        // exist: an id can only be produced by `parse` or `generate`, and for
        // every such id the account directory is exactly one component below
        // the accounts root. There is no id for which it escapes, because
        // there is no id containing a separator or a dot.
        let cfg = Path::new(r"C:\cfg");
        let ids: Vec<AccountId> = [A, B, &"0".repeat(32), &"f".repeat(32)]
            .iter()
            .map(|s| id(s))
            .chain((0..4).map(|_| AccountId::generate()))
            .collect();
        for a in &ids {
            let dir = data_dir_for(cfg, a);
            let rest: Vec<_> = dir
                .strip_prefix(accounts_root(cfg))
                .expect("outside the accounts root")
                .components()
                .collect();
            assert_eq!(rest.len(), 1, "{dir:?} is not a single leaf");
            assert!(
                matches!(rest[0], std::path::Component::Normal(_)),
                "{dir:?} ends in a traversal or a root component"
            );
        }
    }

    #[test]
    fn no_two_accounts_share_a_session_or_hello_path() {
        let cfg = Path::new(r"C:\cfg");
        let ids = [id(A), id(B), id(&"0".repeat(32)), id(&"f".repeat(32))];
        let mut paths: Vec<PathBuf> = Vec::new();
        for a in &ids {
            paths.push(session_path_for(cfg, a));
            paths.push(hello_blob_path_for(cfg, a));
            paths.push(data_dir_for(cfg, a));
        }
        let count = paths.len();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), count, "two accounts share a path: {paths:?}");
    }

    #[test]
    fn two_config_directories_never_produce_the_same_account_path() {
        // The other half of the collision property: the config directory is
        // part of every derived path, so a portable install and a roaming one
        // cannot write over each other.
        let a = id(A);
        assert_ne!(
            data_dir_for(Path::new(r"C:\cfg"), &a),
            data_dir_for(Path::new(r"D:\other"), &a)
        );
        assert_eq!(
            data_dir_for(Path::new(r"D:\other"), &a),
            PathBuf::from(r"D:\other\accounts\0123456789abcdef0123456789abcdef")
        );
    }

    // ---------------------------------------------------------------- 2.3

    #[test]
    fn the_kdf_suffix_is_the_label_and_the_id_exactly() {
        // A literal expectation, not a reconstruction of the expression: this
        // suffix is baked into a key derivation, so changing it silently
        // invalidates every enrolled quick unlock.
        assert_eq!(
            hello_kdf_suffix_for(&id(A)),
            b" account 0123456789abcdef0123456789abcdef".to_vec()
        );
    }

    #[test]
    fn two_accounts_get_different_kdf_suffixes_and_none_is_empty() {
        let a = id(A);
        let b = id(B);
        assert_ne!(hello_kdf_suffix_for(&a), hello_kdf_suffix_for(&b));
        // Absolute, not incidental: an empty suffix would reproduce the
        // pre-migration derivation, so a stale hello.bin left behind by a
        // failed migration would silently open under the migrated account's
        // identity.
        assert!(!hello_kdf_suffix_for(&a).is_empty());
        assert!(!hello_kdf_suffix_for(&b).is_empty());
        assert!(!hello_kdf_suffix_for(&AccountId::generate()).is_empty());
        // And it carries the id, so it cannot be a constant.
        assert!(hello_kdf_suffix_for(&a).ends_with(a.as_str().as_bytes()));
        assert!(hello_kdf_suffix_for(&b).ends_with(b.as_str().as_bytes()));
    }

    #[test]
    fn one_accounts_kdf_suffix_is_never_a_prefix_of_anothers() {
        // Suffixes are concatenated into a hash input. If one were a prefix of
        // another, two accounts could collide under a naive concatenation.
        // Fixed-length ids after a fixed label make that impossible; asserted
        // rather than assumed.
        let ids = [id(A), id(B), id(&"0".repeat(32)), id(&"f".repeat(32))];
        for a in &ids {
            for b in &ids {
                if a == b {
                    continue;
                }
                let (x, y) = (hello_kdf_suffix_for(a), hello_kdf_suffix_for(b));
                assert_eq!(x.len(), y.len());
                assert!(!x.starts_with(&y), "{a} and {b} collide");
            }
        }
    }

    // ---------------------------------------------------------------- 2.4

    #[test]
    fn account_for_finds_by_id_and_misses_cleanly() {
        let list = vec![account(A), account(&"a".repeat(32))];
        assert_eq!(account_for(&list, &id(A)).map(|a| a.id.clone()), Some(id(A)));
        assert_eq!(
            account_for(&list, &id(&"a".repeat(32)))
                .map(|a| a.id.clone()),
            Some(id(&"a".repeat(32))),
            "only the first entry is ever found"
        );
        assert!(account_for(&list, &id(&"9".repeat(32))).is_none());
        assert!(account_for(&[], &AccountId::generate()).is_none());
    }

    #[test]
    fn removing_the_active_account_falls_to_the_first_survivor_and_never_to_itself() {
        let a = account(A);
        let b = account(B);
        let c = account(&"a".repeat(32));
        let list = vec![a.clone(), b.clone(), c.clone()];
        assert_eq!(
            next_active_after_removal(&list, &a.id).map(|x| x.id.clone()),
            Some(b.id.clone()),
            "the first survivor in configured order"
        );
        assert_eq!(
            next_active_after_removal(&list, &b.id).map(|x| x.id.clone()),
            Some(a.id.clone()),
            "removing a later account keeps the earlier one, rather than always \
             answering with index 1"
        );
        assert_eq!(
            next_active_after_removal(&list, &c.id).map(|x| x.id.clone()),
            Some(a.id.clone())
        );
        assert!(
            next_active_after_removal(&[a.clone()], &a.id).is_none(),
            "the last account"
        );
        assert!(next_active_after_removal(&[], &a.id).is_none());
        // Whatever it returns, it is never the account about to be deleted --
        // that directory is going away.
        for removed in [&a.id, &b.id, &c.id] {
            assert_ne!(
                next_active_after_removal(&list, removed).map(|x| &x.id),
                Some(removed)
            );
        }
    }
}

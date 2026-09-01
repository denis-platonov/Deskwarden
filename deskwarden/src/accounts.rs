//! The account model and every path derived from it.
//!
//! One account is one Bitwarden login. Each lives in its own directory under
//! `<config_dir>\accounts\<account-id>\`, holds its own `session.bin` and
//! `hello.bin`, and is reached by pointing the Bitwarden CLI's
//! `BITWARDENCLI_APPDATA_DIR` at that directory.
//!
//! **All accounts are symmetric.** There is no "the first one is special"
//! variant and no `AccountLocation` enum: the first account is minted and
//! signed in to exactly like the second. Deskwarden never adopts the profile
//! the Bitwarden CLI may already hold under `%APPDATA%\Bitwarden CLI`; that
//! directory is left where it is, and a first run is simply an account list
//! with nothing in it yet. So [`data_dir_for`] returns a plain `PathBuf`;
//! there is no account whose directory is "wherever the CLI would have put
//! it".
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
    /// directory listing.
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
    /// Whether this account's vault goes through the official `bw` CLI rather
    /// than through this crate's built-in client.
    ///
    /// **Per account, and that is the feature rather than an implementation
    /// detail.** The owner's case is "two profiles on the same machine --
    /// official via CLI and not", which a single value in [`crate::settings`]
    /// cannot express at all: one machine, two accounts, two backends.
    /// [`crate::backend_policy::choose`] takes this and
    /// [`Self::server_url`] and answers for *one* account, and every caller
    /// spends it against the account it is actually serving.
    ///
    /// **`true` for any record already on disk, and that is what makes the
    /// upgrade safe.** This struct has no container `#[serde(default)]` --
    /// unlike [`crate::settings::Settings`], which does -- so a missing key
    /// is answered by this field's own `default` and by nothing else. An
    /// `Account` in a `settings.json` was written before this field existed,
    /// therefore it is an account that has been served by `bw serve` all
    /// along, therefore `true` is not a guess about it but a description of
    /// it. No migration runs and no existing account changes backend.
    ///
    /// That is precisely the reasoning a default on
    /// [`crate::settings::Settings`] could **not** support: a `Settings` with
    /// no key is indistinguishable from a fresh install, so a `false` there
    /// would silently move an existing self-hoster off the backend that holds
    /// their vault. Here the record's existence *is* the evidence.
    ///
    /// Set explicitly at every mint site ([`prepare_new_account`],
    /// [`resolve_startup`]'s first-run arm, and the API-key path) from the
    /// server the account is being created against, so a *new* account never
    /// relies on this default.
    #[serde(default = "served_by_the_official_cli_by_default")]
    pub use_official_bw_crypto: bool,
}

/// `true`, for [`Account::use_official_bw_crypto`]'s `serde(default)`.
///
/// Named for what it means rather than for what it returns: a free
/// `default_true` would read as a serde idiom, and the thing worth saying at
/// the call site is *which* backend an account with no stored answer gets.
/// See that field for why the answer is `true` and why it is safe.
fn served_by_the_official_cli_by_default() -> bool {
    true
}

/// One switchable account, as an [`AccountsState`], for the windows' tests.
///
/// **Here rather than in the window that wants it**, because
/// `no_window_answers_may_i_switch_for_itself` forbids `vault_window/mod.rs`
/// and four of its siblings from naming `MultiAccountAvailability` at all --
/// and that guard is right: whether this process may switch accounts is this
/// module's answer and nothing else's. A test fixture that named the enum in
/// one of those files would weaken a guard rather than satisfy it, so the
/// fixture lives on this side of the line and the window's test asks for a
/// state rather than deciding what one should say.
#[cfg(test)]
pub(crate) fn one_available_account(email: &str) -> AccountsState {
    let account = Account {
        id: AccountId::generate(),
        email: email.to_string(),
        server_url: None,
        use_official_bw_crypto: true,
    };
    let active = account.id.clone();
    AccountsState::new(
        crate::bw_path::MultiAccountAvailability::Available,
        vec![account],
        active,
    )
    .expect("one account is one account")
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

/// The account's DPAPI-wrapped **master key and refresh token**, used only by
/// the direct-REST backend. Same reasoning as [`session_path_for`], and it
/// matters more here: see [`crate::user_key_store`] for why a store that
/// resolved its own path could put one account's vault key in another
/// account's slot.
///
/// Inside the account directory, so [`delete_account_dir`] removes it with
/// everything else the account owns. There is no pre-accounts layout to
/// migrate from -- this file has never existed anywhere but here.
pub fn user_key_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    data_dir_for(config_dir, id).join("userkey.bin")
}

// ------------------------------------------- every secret one account owns

/// How long one of this app's [`AccountSecret`]s is supposed to outlive the
/// app's use of the account it belongs to.
///
/// The distinction exists because "clear this account's state" is not one
/// question. Switching away from an account is not the user withdrawing the
/// Windows Hello enrolment they opted into, and it is not saying the encrypted
/// vault copy may never be read again -- both of those are meant to survive a
/// switch, which is the whole reason they are enrolled per account. But it
/// *is* saying that this process has no further business holding the
/// credentials it signed in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretScope {
    /// Credentials this app derived or was handed in order to talk to the
    /// server **as this process, right now**: the `bw` session token and the
    /// direct-REST master key. Nothing the user asked to keep; the app keeps
    /// them only so that being signed in survives a restart. The moment the
    /// app stops being on this account they are a secret held for no reason,
    /// so they are dropped on a switch away and on a log out.
    ThisAppsSignIn,
    /// Something the **user opted into** and can withdraw: the Hello blob and
    /// the encrypted vault copy. Survives a switch away, precisely so that
    /// switching back does not re-ask; dropped on a log out and with the
    /// account itself.
    UserOptIn,
}

/// One kind of secret this app writes inside an account's directory.
///
/// # Why this is an enum and not three `remove_file` calls
///
/// Before it existed, an account switch deleted `session.bin` by name and knew
/// nothing else. That was harmless while `session.bin` was the only credential
/// -- and the day [`user_key_path_for`] started being written it became a
/// **leaked non-expiring vault key for an account the user switched away
/// from**: unlike a session token, a master key does not expire and cannot be
/// revoked (see [`crate::user_key_store`]).
///
/// The defect was not the missing line. It was that there was nowhere a fourth
/// secret could be added that would *make* the switch consider it. So every
/// per-account secret is a variant here, every variant must answer
/// [`AccountSecret::scope`] through a `match` with no wildcard arm, and
/// [`clear_sign_in_secrets`] is derived from that answer rather than from a
/// list somebody has to remember to extend. Adding a variant without
/// classifying it does not compile; classifying it is the whole of the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountSecret {
    /// The DPAPI-wrapped `bw` session token; see [`session_path_for`].
    SessionToken,
    /// The DPAPI-wrapped master key and refresh token; see
    /// [`user_key_path_for`].
    UserKey,
    /// The Windows Hello quick-unlock blob; see [`hello_blob_path_for`].
    HelloBlob,
    /// The encrypted offline copy of the vault, `vault_disk_cache`'s file and
    /// its temporary twin. The one variant that names two files.
    VaultCopy,
}

impl AccountSecret {
    /// Every variant. `ALL` and the `match` in [`Self::scope`] are the two
    /// places a new secret has to be added, and both fail loudly: this array
    /// is length-checked against the variant count in the tests, and the match
    /// has no wildcard.
    pub const ALL: [AccountSecret; 4] = [
        AccountSecret::SessionToken,
        AccountSecret::UserKey,
        AccountSecret::HelloBlob,
        AccountSecret::VaultCopy,
    ];

    /// Whether this survives the app moving off the account -- see
    /// [`SecretScope`]. Exhaustive on purpose.
    #[must_use]
    pub fn scope(self) -> SecretScope {
        match self {
            AccountSecret::SessionToken | AccountSecret::UserKey => SecretScope::ThisAppsSignIn,
            AccountSecret::HelloBlob | AccountSecret::VaultCopy => SecretScope::UserOptIn,
        }
    }

    /// The files this secret occupies, inside `data_dir_for(config_dir, id)`.
    ///
    /// Built by *calling* the per-secret path functions above rather than by
    /// repeating their leaf names, so that this cannot come to disagree with
    /// them -- the failure mode would be a clear that deletes a file nothing
    /// writes while the real one stays on disk.
    #[must_use]
    pub fn paths_for(self, config_dir: &Path, id: &AccountId) -> Vec<PathBuf> {
        match self {
            AccountSecret::SessionToken => vec![session_path_for(config_dir, id)],
            AccountSecret::UserKey => vec![user_key_path_for(config_dir, id)],
            AccountSecret::HelloBlob => vec![hello_blob_path_for(config_dir, id)],
            AccountSecret::VaultCopy => {
                let dir = data_dir_for(config_dir, id);
                vec![
                    dir.join(crate::vault_disk_cache::FILE_NAME),
                    dir.join(crate::vault_disk_cache::TMP_FILE_NAME),
                ]
            }
        }
    }
}

/// Deletes every [`SecretScope::ThisAppsSignIn`] secret of `id`.
///
/// The one call the app makes when it stops being signed in as an account it
/// is not deleting: switching away from it, and logging it out. It is
/// deliberately **not** "delete this account's secrets" -- the Hello
/// enrolment and the encrypted vault copy are the user's, and a switch must
/// not silently withdraw them (a log out drops those too, by its own calls, in
/// the same breath).
///
/// Infallible, like [`discard_prepared_account`] and for its reason: both
/// callers are past the point where anything can be offered instead, and a
/// caller pushed into ignoring a `Result` is a caller that will. A file that
/// is already gone is not logged at all -- that is the state being asked for.
pub fn clear_sign_in_secrets(config_dir: &Path, id: &AccountId) {
    for secret in AccountSecret::ALL {
        if secret.scope() != SecretScope::ThisAppsSignIn {
            continue;
        }
        for path in secret.paths_for(config_dir, id) {
            match std::fs::remove_file(&path) {
                Ok(()) => log::info!("discarded {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // The path and the OS error, never anything that was in the
                // file: these are the two files this app guards hardest.
                Err(e) => log::warn!("could not discard {}: {e}", path.display()),
            }
        }
    }
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
/// a `hello.bin` left over from that version — one still sitting in
/// `<config_dir>` — could be opened under whichever account happened to have
/// the empty suffix. Quick unlock is therefore enrolled per account, which is
/// why the first-run login window says the earlier enrolment no longer applies
/// (see [`login_ui::FIRST_RUN_NOTICE`](crate::login_ui::FIRST_RUN_NOTICE)).
pub fn hello_kdf_suffix_for(id: &AccountId) -> Vec<u8> {
    let mut suffix = b" account ".to_vec();
    suffix.extend_from_slice(id.as_str().as_bytes());
    suffix
}

/// The configured account with this id, if any.
pub fn account_for<'a>(accounts: &'a [Account], id: &AccountId) -> Option<&'a Account> {
    accounts.iter().find(|a| &a.id == id)
}

/// How an account is named in anything a user reads — a message box, a log
/// line, or a row in the vault window's account switcher.
///
/// An account minted by [`resolve_startup`] on a first install, or by
/// [`prepare_new_account`], carries an **empty** email until a sign-in fills it
/// in. `"" could not be removed` names nothing at all, and a switcher row
/// showing it is a blank strip of menu the user is invited to click. The id is
/// not friendly, but it is the directory name under `accounts\` and so is the
/// one thing that always distinguishes this account from every other.
///
/// In this module rather than in `main.rs`, where it was written for
/// `remove_account`'s messages, because the vault window needs the same answer
/// and is in the library half of this crate: two spellings of "what do we call
/// an account with no email" is one of them drifting.
pub fn account_label(account: &Account) -> &str {
    if account.email.is_empty() {
        account.id.as_str()
    } else {
        &account.email
    }
}

/// Records what the app has just learned about **this account's identity**,
/// and answers whether it changed anything.
///
/// The hole this closes: an account minted by [`resolve_startup`] on a first
/// install carries an empty email, and nothing filled it in afterwards — so
/// [`account_label`] fell back to the id and every menu naming the account
/// named a 32-character hash instead. The sign-in that follows the mint is the
/// moment the address becomes knowable, and
/// [`login_ui::identity_after_sign_in`](crate::login_ui::identity_after_sign_in)
/// is what knows it: the address is read off the box the user just submitted,
/// on the `bw serve` backend and on the direct-REST one alike. It used to come
/// from a `bw status` spawn that followed the sign-in — later than the form,
/// and on a direct-REST account a question put to a CLI that has never seen
/// this account at all.
///
/// **`None` means "nothing was learned", never "there is nobody".** An unlock
/// draws no email box, so it has nothing to offer here; and on the `bw serve`
/// backend a `bw status` is still run — to decide what the login card opens
/// knowing, not to name the account
/// ([`check_bw_status_details_in`](crate::login_ui::check_bw_status_details_in))
/// — and it answers `null` for both fields when the CLI is logged out or
/// could not be spawned at all. Letting either of those erase an address
/// already on disk would put the hash back in the menu for a locked vault.
///
/// A *different* non-empty answer does win, and has to: `bw login` replaces
/// whatever profile it is pointed at, so a directory can genuinely change
/// hands. A stale email is a menu row naming the wrong person.
/// Whether `account` is a mint that no sign-in has completed yet.
///
/// A blank address *and* no server: that is what [`prepare_new_account`] and
/// [`resolve_startup`]'s first-run arm produce, and nothing else in this app
/// writes a record in that state. Both halves are required -- an account with
/// an address has signed in, and an account with a server has been pointed at
/// one -- because this predicate gates the one write that is allowed to
/// choose an account's backend, and widening it would let that write reach an
/// established account.
///
/// **`pub(crate)` for one other caller, and it is the same question.**
/// [`crate::login_ui::login_card_source`] asks whether the login card may read
/// its identity off the record instead of spawning `bw status`, and the answer
/// turns on exactly this: a mint has nothing on its record to read, and an
/// established account has a CLI that may know something the record does not.
/// Duplicating the two-field test beside that call site would be a second
/// opinion of what a fresh account looks like, drifting from this one -- the
/// same defect the doc above is written against.
pub(crate) fn is_a_fresh_mint(account: &Account) -> bool {
    account.email.is_empty() && account.server_url.is_none()
}

/// Which backend an account will be on once a sign-in against `server_url`
/// completes -- asked *before* the sign-in, by the window that has to decide
/// whether this sign-in needs the `bw` CLI downloaded first.
///
/// **One rule, two callers, and the second one exists because the first is
/// too late.** [`learn_account_details`] writes this value, but it runs
/// *after* a successful sign-in -- and the CLI has to be on disk *before* the
/// sign-in, because on the `bw serve` backend the CLI is what performs it. So
/// the sign-in window asks the same question in advance, and asks it through
/// this function rather than through a second copy of the reasoning:
/// `crate::bw_acquire::this_sign_in_needs_the_cli` spends the answer, and a
/// `debug_assert` in `learn_account_details` holds the two to the same
/// answer.
///
/// **The typed server, not the stored one, and that is the whole point for a
/// new account.** A mint carries `server_url: None` and the `true`
/// placeholder right up until the sign-in it is waiting for, so asking the
/// *record* would say "official, fetch the CLI" for every new account
/// including a self-hosted one -- which is the download the built-in client
/// exists to avoid. `server_url` here is the address in the form the user is
/// about to submit.
///
/// **An established account answers from its record**, and the typed address
/// is ignored: it has a backend already serving it, and `learn_account_details`
/// will not move it either.
///
/// **`chosen_this_sign_in` is the user's own answer, and it beats BOTH of
/// the answers below.** The sign-in window asks a self-hoster which client
/// should open their vault (`bw_acquire::CliSetupState::Choosing`) on every
/// sign-in, established account included, because the owner's rule is
/// "always ... prompt which one to use (self-hosted) when login" and the
/// silent derivation was the defect. `None` means nothing was asked, which is
/// every caller that is not that modal: a `bw status` answer being learned, a
/// preview, a test -- and every sign-in to an official Bitwarden server,
/// where there is no choice to put.
///
/// **It reaches the established arm too, and a sign-in is the one moment
/// that is safe.** The stored record is otherwise final: this function is
/// what `learn_account_details` and `main`'s added-account path both write
/// from, and the alternative -- Preferences -- has to ask for a restart and a
/// fresh sign-in precisely to reach the state a sign-in is already in. The
/// backend is re-settled from this value a moment later
/// (`login_ui::direct_login_for_this_sign_in` ->
/// `backend_policy::resettle_for` -> `main`'s `settle_the_vault_backend`),
/// and that settlement is what clears or writes `userkey.bin`, so the key
/// hygiene rides along rather than being something this caller must remember.
#[must_use]
pub fn official_cli_after_sign_in(
    account: Option<&Account>,
    server_url: Option<&str>,
    chosen_this_sign_in: Option<bool>,
) -> bool {
    if let Some(chosen) = chosen_this_sign_in {
        return chosen;
    }
    match account {
        Some(account) if !is_a_fresh_mint(account) => account.use_official_bw_crypto,
        // `None` is a host with no account record at all, which is a sign-in
        // that is about to mint one; it takes the same answer as a mint.
        _ => !crate::backend_policy::is_self_hosted(server_url),
    }
}

/// `chosen_this_sign_in` is what the sign-in window's choice modal answered,
/// threaded here so the value this function writes is the user's own answer
/// rather than the derivation made in their absence. `None` everywhere the
/// question was never put -- see [`official_cli_after_sign_in`], which is
/// where the two are reconciled, so the precedence lives in ONE function and
/// this one cannot disagree with the gate that showed the modal.
pub fn learn_account_details(
    account: &mut Account,
    email: Option<&str>,
    server_url: Option<&str>,
    chosen_this_sign_in: Option<bool>,
) -> bool {
    // **The one moment an account's backend is decided, and it is decided
    // once.**
    //
    // A blank record -- no address and no server -- is a mint from
    // [`prepare_new_account`] or [`resolve_startup`]'s first-run arm that no
    // sign-in has completed yet. This call is that sign-in, so it is the
    // first and only moment at which the server this account exists to talk
    // to is known, which is exactly what
    // [`crate::backend_policy::choose`] needs and what
    // [`Account::use_official_bw_crypto`] must therefore hold.
    //
    // **Read before anything below writes**, because both halves of the test
    // are fields this function is about to fill in.
    //
    // Guarding on "was blank" rather than deriving unconditionally is the
    // whole safety argument, and it is the same one the field's `serde`
    // default rests on: an account that already carries an address or a
    // server has been signed in before, therefore it has a backend already
    // serving it, therefore this function must not re-open the question. `bw
    // login` can genuinely move a directory to a different server -- see the
    // note above about a stale email -- and on an established account that
    // moves the address without moving the vault off the backend that holds
    // it. `settle_the_vault_backend` handles a server that has moved to a
    // cloud host on its own, by clearing the stored key and returning to
    // `bw serve`; it does not need this function to rewrite the preference to
    // get there.
    let completing_a_fresh_mint = is_a_fresh_mint(account);
    let mut changed = false;
    if let Some(email) = email.filter(|e| !e.is_empty()) {
        if account.email != email {
            account.email = email.to_string();
            changed = true;
        }
    }
    if let Some(server_url) = server_url.filter(|s| !s.is_empty()) {
        if account.server_url.as_deref() != Some(server_url) {
            account.server_url = Some(server_url.to_string());
            changed = true;
        }
    }
    // **An answered sign-in writes its answer whatever the account's age.**
    //
    // This is the one place the guard above is deliberately not consulted.
    // "Was blank" is the right test for a DERIVATION, because a derivation is
    // a guess and re-guessing an established account's backend from a `bw
    // login` that only moved its address would move a vault nobody asked to
    // move. It is the wrong test for an ANSWER: the user was shown which
    // client this account is on and pressed the other one, on the sign-in
    // that re-settles the backend and re-authenticates, which is the only
    // moment the change is free. Refusing it here would leave the modal
    // painting a choice it cannot keep.
    if let Some(chosen) = chosen_this_sign_in {
        if account.use_official_bw_crypto != chosen {
            account.use_official_bw_crypto = chosen;
            changed = true;
        }
    } else if completing_a_fresh_mint {
        // The same function the sign-in window's CLI gate asks, so the modal
        // that window decides to show and the backend this write settles on
        // are one answer and not two. See `official_cli_after_sign_in`.
        // `!is_self_hosted` rather than `choose`: this is the *input* that
        // function takes, and computing it from the function it feeds would
        // be circular. A positively self-hosted server gets the built-in
        // client; bitwarden.com, and any address whose host cannot be read,
        // gets the official CLI -- the same "unknown counts as official"
        // direction `is_self_hosted` documents, reached rather than repeated.
        //
        // A sign-in to bitwarden.com leaves `server_url` as `None` and lands
        // here with `true`, which is both the value the mint already carried
        // and the right one; the assignment is unconditional so that the two
        // arms cannot drift apart.
        //
        // Reached only when nothing was answered -- the arm above has the
        // answered case -- so this is the derivation exactly as it always
        // was, and `None` is passed to the assertion below to say so.
        let official = !crate::backend_policy::is_self_hosted(account.server_url.as_deref());
        debug_assert_eq!(
            official,
            official_cli_after_sign_in(None, account.server_url.as_deref(), None),
            "the sign-in gate and this write must agree about the backend a new account gets"
        );
        if account.use_official_bw_crypto != official {
            account.use_official_bw_crypto = official;
            changed = true;
        }
    }
    changed
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

/// Creates an account's data directory if it is not there yet, and hands back
/// the path.
///
/// Exists because [`session_path_for`] and [`hello_blob_path_for`] both name
/// files *inside* it and neither writer creates it:
/// [`SessionStore::new`](crate::session_store::SessionStore::new) is explicit
/// that "its parent directory must already exist — the account directory is
/// created when the account is". [`prepare_new_account`] creates the directory
/// for an account added mid-session; an account [`resolve_startup`] mints has
/// none, and without this call the very first `store.save` fails with "the
/// system cannot find the path specified" — logged, survivable, and invisible
/// except as a master-password prompt on every launch forever.
///
/// Idempotent, so startup can call it unconditionally rather than deciding
/// which of the two cases it is in.
pub fn ensure_account_dir(config_dir: &Path, id: &AccountId) -> std::io::Result<PathBuf> {
    let dir = data_dir_for(config_dir, id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Mints an account and creates its directory, ready for a sign-in that has
/// not happened yet.
///
/// The email is empty *by construction*, not by oversight: nobody has signed
/// in, so there is nothing to record. The sign-in that follows fills it in
/// from the address typed into the login card's own email box
/// ([`login_ui::identity_after_sign_in`](crate::login_ui::identity_after_sign_in)),
/// which is the single writer for both backends and knows the answer strictly
/// before any process could report it. Left empty, the account is a blank row
/// in the switcher that the user cannot tell from any other.
///
/// **The CLI is still asked something on this path, and it is a different
/// question.** An account minted here has `server_url: None`, and
/// [`backend_policy::choose`](crate::backend_policy::choose) cannot answer
/// `DirectRest` without a positively self-hosted URL — so a fresh account's
/// login card is always the `bw serve` one, which runs `bw status` in this
/// directory
/// ([`login_ui::check_bw_status_details_in`](crate::login_ui::check_bw_status_details_in))
/// to decide what the card opens showing. That spawn reports whether the
/// CLI's own profile is logged in. It is not what names the account, and on a
/// direct-REST account it does not run at all.
///
/// The directory is created with `create_dir` rather than `create_dir_all`, so
/// an id whose directory already exists is an **error** rather than a silent
/// adoption. [`discard_prepared_account`] deletes the whole directory when the
/// sign-in is abandoned, and deleting a directory this call did not create
/// would take a working account's vault with it. Unreachable in practice --
/// the id is 128 bits from the OS CSPRNG -- and it is the consequence of being
/// wrong, not the odds, that decides the shape here.
pub fn prepare_new_account(config_dir: &Path) -> Result<Account, String> {
    let root = accounts_root(config_dir);
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("could not create {}: {e}", root.display()))?;
    let id = AccountId::generate();
    let dir = data_dir_for(config_dir, &id);
    std::fs::create_dir(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    log::info!("prepared a new account directory at {}", dir.display());
    Ok(Account {
        id,
        email: String::new(),
        server_url: None,
        // Nothing has been signed in against this directory yet, so the
        // server -- the only thing that can answer this -- is not known.
        // `true` is the value that means "no decision has been taken": it is
        // what every account on this machine had before the field existed, so
        // a mint abandoned between here and the sign-in leaves a record that
        // reads exactly like an old one rather than like an opt-in nobody
        // made. `learn_account_details` derives the real answer from the
        // server the sign-in is actually against, and it is the only writer
        // that ever moves this field for a new account.
        use_official_bw_crypto: true,
    })
}

/// Undoes [`prepare_new_account`]: deletes the account's directory and
/// everything an abandoned sign-in left in it.
///
/// Deleting the *directory* rather than a list of files by name is the point.
/// A sign-in that got as far as ticking "Use Windows Hello" leaves a
/// `hello.bin` sealing a master password for an account that is about to stop
/// existing, and a `bw login` that succeeded before the switch failed leaves a
/// whole CLI profile. What must not survive is an account directory nothing
/// names -- or, worse, an entry naming a directory with no profile in it,
/// which presents as an account that is permanently signed out.
///
/// Infallible by design: this runs on a path where something has already gone
/// wrong and there is no second recovery to offer, so a failure is logged and
/// the caller gets on with restoring the account the user was already using.
pub fn discard_prepared_account(config_dir: &Path, id: &AccountId) {
    match delete_account_dir(config_dir, id) {
        Ok(()) => log::info!(
            "discarded the prepared account directory {}",
            data_dir_for(config_dir, id).display()
        ),
        Err(reason) => log::warn!("{reason}"),
    }
}

/// Deletes an account's whole directory — its CLI profile, its `session.bin`
/// and its `hello.bin` — and is the **only** place in this module that runs
/// `remove_dir_all` on a path built from an account id.
///
/// One implementation with two callers rather than one each:
/// [`discard_prepared_account`] undoing an abandoned sign-in, and the account
/// removal in `main`, which is the same deletion asked for on purpose. A second
/// copy would be a second guard, and the guard is the whole point of the
/// function.
///
/// The `starts_with` check is belt and braces over [`AccountId::parse`], which
/// already makes `..` and an absolute path unrepresentable. It is here because
/// of what being wrong costs: a path that escaped the accounts root would take
/// `settings.json`, the log, and *every other account's* profile with it.
/// Refusing returns `Err` and deletes nothing.
///
/// A directory that is already gone is `Ok`: "there is no such directory" is
/// the goal state, however we got there.
pub fn delete_account_dir(config_dir: &Path, id: &AccountId) -> Result<(), String> {
    let dir = data_dir_for(config_dir, id);
    let root = accounts_root(config_dir);
    if !dir.starts_with(&root) {
        return Err(format!(
            "refusing to delete {}: it is not under {}",
            dir.display(),
            root.display()
        ));
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {}: {e}", dir.display())),
    }
}

// -------------------------------------------------- what startup resolves to

/// What this launch is pointed at.
///
/// Exactly two shapes, because `main` has exactly two: an app pointed at one
/// account's directory, and an app with no account of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAccounts {
    /// The normal case: at least one account, one of them active.
    Ready {
        active: Account,
        accounts: Vec<Account>,
        /// Whether the resolution changed something `settings.json` has to be
        /// told about — a freshly minted account, or an active id that named
        /// nobody.
        needs_persist: bool,
        /// Whether `active` was **minted by this resolution** rather than
        /// resumed from the stored list.
        ///
        /// Which is the same thing as "nobody has ever signed in to this
        /// account", because minting only creates a directory. The login
        /// window uses it to say why it is asking
        /// ([`login_ui::FIRST_RUN_NOTICE`](crate::login_ui::FIRST_RUN_NOTICE)):
        /// on the first launch under the per-account layout the user is asked
        /// for a master password with nothing on screen to explain it, which
        /// reads as a bug rather than as setup.
        first_run: bool,
    },
    /// `settings.json` is there and could not be read, so the app runs with no
    /// account of its own: no `BITWARDENCLI_APPDATA_DIR` is set and
    /// `<config_dir>\session.bin` is the token store.
    ///
    /// **The only state in which the app has no [`Account`] at all**, and the
    /// reason it exists is that a failed parse yields an *empty* account list
    /// — indistinguishable, to [`resolve_startup`], from a machine with no
    /// accounts yet. Minting on that would point the CLI at an empty
    /// directory while the user's own account sat in `accounts/<old-id>/`, and
    /// [`Settings::save`](crate::settings::Settings::save) refuses to write
    /// over an unreadable file, so the fresh id would not even be recorded:
    /// the next launch would mint another one, and the one after that another.
    ///
    /// Nothing is deleted here and nothing is written; `reason` is what
    /// [`AccountsState::blocked_reason`] would have reported, and `main` puts
    /// it on screen because this state repeats identically on every launch
    /// until the file is fixed or removed.
    NoAccountList { reason: String },
}

/// Which account this launch runs as, given what `settings.json` holds.
///
/// Pure: every effect belongs to the caller, so this can be driven through
/// every branch without a `%APPDATA%` anywhere near it.
///
/// There are three answers and the whole function is the difference between
/// them:
///
/// * **Accounts stored.** Resume the active one — or, if the stored active id
///   names none of them (a hand-edited `settings.json`, or a removal that
///   crashed between two writes), the first configured account, rewriting the
///   active id. Never no active account: that would leave the app with no
///   directory to point the CLI at.
/// * **No accounts stored, and the list was readable.** A first run: mint one
///   account, mark it `first_run`, and let the login window ask for a master
///   password. Deskwarden does not import the profile the Bitwarden CLI may
///   already have under `%APPDATA%` — it is left exactly where it is,
///   untouched — because the vault lives on Bitwarden's servers and that
///   directory is a local cache plus a session token. Re-signing in
///   reconstructs it; copying it does not gain the user anything a sign-in
///   does not.
/// * **No accounts stored because the list could not be read.**
///   [`StartupAccounts::NoAccountList`], which mints nothing. See that
///   variant.
pub fn resolve_startup(
    stored: &[Account],
    stored_active: Option<&AccountId>,
    unreadable_reason: Option<&str>,
) -> StartupAccounts {
    let mut accounts = stored.to_vec();
    let mut needs_persist = false;
    let mut first_run = false;

    if accounts.is_empty() {
        if let Some(reason) = unreadable_reason {
            return StartupAccounts::NoAccountList {
                reason: reason.to_string(),
            };
        }
        accounts.push(Account {
            id: AccountId::generate(),
            // Not known yet: nobody has signed in. Filled in by whoever
            // completes a sign-in against this directory.
            email: String::new(),
            server_url: None,
            // Not known yet either, and for the same reason -- the server is
            // learned from the sign-in this mint has not had. See
            // `prepare_new_account` for why the placeholder is `true`.
            use_official_bw_crypto: true,
        });
        needs_persist = true;
        first_run = true;
    }

    // The minted account when there is one, else the stored active id, else
    // the first configured account.
    let active = first_run
        .then(|| accounts.first())
        .flatten()
        .or_else(|| stored_active.and_then(|id| account_for(&accounts, id)))
        .or_else(|| accounts.first())
        .expect("the account list is non-empty by construction above")
        .clone();
    if Some(&active.id) != stored_active {
        needs_persist = true;
    }

    StartupAccounts::Ready {
        active,
        accounts,
        needs_persist,
        first_run,
    }
}

// ------------------------------------------- may this process switch at all?

/// The one door for "may I offer another account, and which one am I on?".
///
/// One reason a switch may be unavailable, and every UI entry point needs the
/// same answer to it:
/// [`MultiAccountAvailability`](crate::bw_path::MultiAccountAvailability) — a
/// `bitwarden-cli` directory beside `bw.exe` makes the CLI ignore
/// `BITWARDENCLI_APPDATA_DIR`, so every account would silently share one
/// profile; and "we do not know where the CLI is" is the same refusal, because
/// the trap cannot be ruled out.
///
/// It is asked here and nowhere else. A window that answered it for itself
/// would offer a switch that shares a profile — which reports no error, so it
/// is not visible in an end state. There *were* two reasons, the second being
/// a profile migration that had been refused; that feature is gone and the
/// door is not, because the remaining refusal still has to reach every window
/// through one value.
///
/// [`switchable`](Self::switchable) is a *field*, computed once in
/// [`new`](Self::new), rather than a filter a caller could be tempted to
/// rebuild from [`all`](Self::all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsState {
    accounts: Vec<Account>,
    active: Account,
    switchable: Vec<Account>,
    blocked_reason: Option<String>,
}

impl AccountsState {
    /// `None` when `accounts` is empty, which is the *only* way this can fail.
    ///
    /// An account list with no accounts is not a state this app has:
    /// [`resolve_startup`] either resumes a stored account or mints one, and
    /// its one account-less answer
    /// ([`StartupAccounts::NoAccountList`]) builds no `AccountsState` at all.
    /// Making that unrepresentable is what lets [`active`](Self::active)
    /// return `&Account` instead of an `Option` every caller would have to
    /// unwrap somewhere.
    ///
    /// `active` naming an id that is not in `accounts` falls back to the first
    /// configured account rather than being refused. `settings.json` is a
    /// user-editable file and a removal that crashed mid-write leaves exactly
    /// this state; the alternative — no active account — would leave the app
    /// with no directory to point the CLI at.
    pub fn new(
        availability: crate::bw_path::MultiAccountAvailability,
        accounts: Vec<Account>,
        active: AccountId,
    ) -> Option<Self> {
        let active = account_for(&accounts, &active)
            .or_else(|| accounts.first())?
            .clone();

        let blocked_reason = availability.explanation();
        let switchable = switch_targets(&accounts, &active, blocked_reason.is_some());

        Some(Self {
            accounts,
            active,
            switchable,
            blocked_reason,
        })
    }

    /// The same state, built from the one thing [`new`](Self::new) distils its
    /// input down to — for the tests of a window that is **banned from naming
    /// that input**.
    ///
    /// `no_window_answers_may_i_switch_for_itself` forbids
    /// `vault_window/mod.rs` from containing the string
    /// `MultiAccountAvailability` anywhere at all, tests included, and that ban
    /// is the point of this type. But the account switcher in that window still
    /// has to be handed a blocked state and an available one by its own tests,
    /// and a hand-built struct literal there would be a second `AccountsState`
    /// with its own idea of what `switchable` means.
    ///
    /// So this takes exactly the `Option<String>` that
    /// [`new`](Self::new) derives and computes everything else the same way it
    /// does — including `switchable`, through the one [`switch_targets`],
    /// rather than filtering the list a second time.
    /// `the_test_constructor_agrees_with_the_real_one` pins the two together.
    #[cfg(test)]
    pub fn from_blocked_reason(
        accounts: Vec<Account>,
        active: AccountId,
        blocked_reason: Option<String>,
    ) -> Option<Self> {
        let active = account_for(&accounts, &active)
            .or_else(|| accounts.first())?
            .clone();
        let switchable = switch_targets(&accounts, &active, blocked_reason.is_some());
        Some(Self {
            accounts,
            active,
            switchable,
            blocked_reason,
        })
    }

    /// The account this process is pointed at.
    pub fn active(&self) -> &Account {
        &self.active
    }

    /// Every configured account, including the active one — the list is still
    /// *shown* when switching is refused; it is switching that is refused.
    pub fn all(&self) -> &[Account] {
        &self.accounts
    }

    /// The accounts a user may switch **to** right now. Empty when
    /// multi-account is blocked, whatever [`all`](Self::all) holds.
    pub fn switchable(&self) -> &[Account] {
        &self.switchable
    }

    /// Whether another account may be added. The same block: an account added
    /// now would share the one profile.
    pub fn can_add(&self) -> bool {
        self.blocked_reason.is_none()
    }

    /// Why switching and adding are unavailable, in the words the user can act
    /// on, or `None` when they are available.
    pub fn blocked_reason(&self) -> Option<&str> {
        self.blocked_reason.as_deref()
    }

    /// Whether the account this process is on could be removed at all.
    ///
    /// Two refusals, both of them `remove_account`'s own, and both answered
    /// here so that no menu has to spell either for itself: **the last account
    /// cannot be removed** (there would be nowhere coherent for the app to
    /// land, and no directory left to point the CLI at), and a **blocked**
    /// state cannot remove anything (the app cannot reach the survivor it
    /// would have to settle onto first, and it will not delete a profile it
    /// cannot reach). Both collapse to "there is at least one switchable
    /// account", because that is exactly the survivor
    /// [`next_active_after_removal`] picks and exactly what `remove_account`
    /// then re-checks against [`switchable`](Self::switchable).
    ///
    /// Asked by both menus that offer a removal — the tray's Accounts submenu
    /// and the vault window's account menu. A menu item that can only fail is
    /// worse than one that is not there, and two spellings of the rule is one
    /// of them offering it anyway.
    pub fn can_remove_active(&self) -> bool {
        !self.switchable.is_empty()
    }

    /// Fills the active account's email and server URL in from what the
    /// sign-in established about it (see [`learn_account_details`], which is
    /// where the source of that answer is written down), and answers whether
    /// anything changed — so the caller writes `settings.json` only when
    /// there is something new to write.
    ///
    /// Applied to [`active`](Self::active) **and** to every entry of
    /// [`all`](Self::all) carrying that id, because those are two copies of one
    /// account and `all()` is what gets persisted. A hand-edited file can hold
    /// the id twice, hence `filter` rather than `find`.
    ///
    /// [`switchable`](Self::switchable) is deliberately not recomputed: it never
    /// contains the active id (see [`switch_targets`]), so no row it holds can
    /// be the one this just rewrote, and rebuilding it here would be a second
    /// place deciding what a switch target is.
    /// **No backend choice reaches here, and that is deliberate.** This
    /// learns what a `bw status` (or a sign-in's own report) said about an
    /// account's address and server; a backend choice is not one of those
    /// facts, and its only writer is the added-account path, which builds a
    /// finished [`Account`] rather than filling in a blank one. Threading
    /// `None` through both calls keeps this function unable to move a
    /// backend by accident. See [`set_active_backend_preference`] for the
    /// path that does move one, on purpose.
    ///
    /// [`set_active_backend_preference`]: Self::set_active_backend_preference
    pub fn learn_active_details(&mut self, email: Option<&str>, server_url: Option<&str>) -> bool {
        let mut changed = learn_account_details(&mut self.active, email, server_url, None);
        let id = self.active.id.clone();
        for account in self.accounts.iter_mut().filter(|a| a.id == id) {
            changed |= learn_account_details(account, email, server_url, None);
        }
        changed
    }

    /// Records that the user has chosen a backend for the active account,
    /// answering whether anything moved.
    ///
    /// **The one writer that may change an established account's backend**,
    /// and it is deliberately not [`learn_account_details`]: that function
    /// derives the value from a server and refuses to touch an account that
    /// has signed in before, precisely so a re-sign-in cannot move a vault
    /// off the backend holding it. This is the other direction -- the user in
    /// Preferences, saying so on purpose -- so it takes the value rather than
    /// deriving one, and it applies to an established account by design.
    ///
    /// Written to [`active`](Self::active) **and** to every entry of
    /// [`all`](Self::all) carrying that id, for the reason
    /// [`learn_active_details`](Self::learn_active_details) gives: those are
    /// two copies of one account and `all()` is what gets persisted. A
    /// caller that updated only `active` would show the change for this
    /// session and lose it on the next launch.
    pub fn set_active_backend_preference(&mut self, use_official_bw_crypto: bool) -> bool {
        let mut changed = self.active.use_official_bw_crypto != use_official_bw_crypto;
        self.active.use_official_bw_crypto = use_official_bw_crypto;
        let id = self.active.id.clone();
        for account in self.accounts.iter_mut().filter(|a| a.id == id) {
            changed |= account.use_official_bw_crypto != use_official_bw_crypto;
            account.use_official_bw_crypto = use_official_bw_crypto;
        }
        changed
    }

    /// Records the account this process has just moved onto, appending it to
    /// the list if it is not there yet.
    ///
    /// This is how an *added* account reaches the state every window reads.
    /// It is a mutation rather than a rebuild because [`new`](Self::new)'s
    /// input — the CLI's availability — is not re-derivable here, and
    /// re-deriving it would be a second reading of the fact this type exists
    /// to be the single answer to. It is not changed by adding an account:
    /// whether switching is blocked was settled at startup, so it is
    /// *carried*, and a blocked state that adopts one still offers no switch
    /// targets.
    pub fn adopt(&mut self, account: Account) {
        if account_for(&self.accounts, &account.id).is_none() {
            self.accounts.push(account.clone());
        }
        self.active = account;
        self.switchable =
            switch_targets(&self.accounts, &self.active, self.blocked_reason.is_some());
    }

    /// Drops `removed` from the list, and answers whether it did.
    ///
    /// The counterpart of [`adopt`](Self::adopt), and a mutation for the same
    /// reason: [`new`](Self::new)'s two inputs are not re-derivable at the point
    /// of a removal, and neither is *changed* by one.
    ///
    /// **It refuses to remove the account this process is on**, which is the
    /// one rule this type needs to keep the two invariants it is built around.
    /// The active account is always in the list, so refusing the active one is
    /// also what keeps the list from ever becoming empty — and an empty
    /// `AccountsState` is exactly the state [`new`](Self::new) returns `None`
    /// rather than represent, because [`active`](Self::active) hands out an
    /// `&Account` and there would be none to hand out. The caller that is
    /// removing the *active* account settles onto the survivor first (see
    /// [`next_active_after_removal`]) and [`adopt`](Self::adopt)s it, which
    /// makes the account it is deleting inactive before it gets here.
    ///
    /// `false` for an id this state does not hold, so a double removal is a
    /// no-op rather than a silent success that persists a list nobody changed.
    pub fn forget(&mut self, removed: &AccountId) -> bool {
        if &self.active.id == removed {
            log::error!(
                "refusing to forget the active account: the app would be left with no account \
                 to point the CLI at"
            );
            return false;
        }
        let before = self.accounts.len();
        self.accounts.retain(|a| &a.id != removed);
        if self.accounts.len() == before {
            return false;
        }
        self.switchable =
            switch_targets(&self.accounts, &self.active, self.blocked_reason.is_some());
        true
    }
}

/// The accounts a user may switch **to**, given the whole list and the one
/// they are on.
///
/// One definition, used by [`AccountsState::new`] and [`AccountsState::adopt`]
/// alike. A second copy would be a second rule, and the two would first
/// disagree exactly where it matters least visibly: an account added at
/// runtime is the only one that ever reaches `adopt`.
fn switch_targets(accounts: &[Account], active: &Account, blocked: bool) -> Vec<Account> {
    let mut targets: Vec<Account> = Vec::new();
    if blocked {
        return targets;
    }
    for account in accounts {
        // Never the active account: "switch to where you already are" still
        // tears the backend down and demands a master password. Never a repeat
        // of one already offered either — two entries for one id are two doors
        // onto one directory, and a hand-edited file can contain them.
        let already = targets.iter().any(|a: &Account| a.id == account.id);
        if account.id != active.id && !already {
            targets.push(account.clone());
        }
    }
    targets
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
            use_official_bw_crypto: true,
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
        // This id becomes a directory name, and account removal will
        // `remove_dir_all` a path built from it. A traversal, a
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
            use_official_bw_crypto: false,
        };
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            format!(
                "{{\"id\":\"{A}\",\"email\":\"me@example.com\",\
                 \"server_url\":\"https://vault.example.com\",\
                 \"use_official_bw_crypto\":false}}"
            )
        );
        assert_eq!(
            serde_json::from_str::<Account>(&serde_json::to_string(&stored).unwrap()).unwrap(),
            stored
        );
        // A self-hosted URL is optional, not absent-meaning-empty.
        assert_eq!(
            serde_json::to_string(&account(A)).unwrap(),
            format!(
                "{{\"id\":\"{A}\",\"email\":\"me@example.com\",\"server_url\":null,\
                 \"use_official_bw_crypto\":true}}"
            )
        );
        // **A record written before the backend became a per-account choice.**
        // The key is absent, and it must read as `true` -- the official CLI,
        // which is what every account on this machine was served by before the
        // field existed. `Account` deliberately carries no container
        // `#[serde(default)]`, so this is that one field's own default and
        // nothing else; see `Account::use_official_bw_crypto` for why the
        // existence of the record is what makes `true` a description rather
        // than a guess.
        let older = serde_json::from_str::<Account>(&format!(
            "{{\"id\":\"{A}\",\"email\":\"me@example.com\",\
             \"server_url\":\"https://vault.example.com\"}}"
        ))
        .expect("a record written before the field existed must still load");
        assert!(
            older.use_official_bw_crypto,
            "an account stored before this field existed must stay on the official CLI"
        );
        // And a stored account carrying an escaping id is rejected as a whole,
        // not silently loaded with a dangerous directory name.
        assert!(serde_json::from_str::<Account>(
            r#"{"id":"../..","email":"me@example.com","server_url":null}"#
        )
        .is_err());
    }

    // ---- the backend an account gets, and when it is decided --------------

    /// **The upgrade regression, and it is the one that would cost a user
    /// their vault.**
    ///
    /// An existing self-hosted account is a record in a `settings.json`
    /// written before this field existed: it has a server, it has an address,
    /// and it has no `use_official_bw_crypto` key. That user's vault is held
    /// by `bw serve`, and their machine may have no `userkey.bin` at all --
    /// the direct path has never run for them. If the upgrade moved them to
    /// the built-in client, their next launch would drop the cached `bw`
    /// session and ask for the master password against a backend that has
    /// never held their vault.
    ///
    /// So this walks the whole way from the bytes on disk to the backend:
    /// parse the record as it really is on disk, and spend it on the same
    /// `choose` that `settle_the_vault_backend` spends. Asserting the field
    /// alone would be the vacuous version -- it would pass while `choose`
    /// answered anything at all.
    #[test]
    fn an_existing_self_hosted_account_stays_on_bw_serve_across_the_upgrade() {
        use crate::backend_policy::{VaultBackendChoice, choose, is_self_hosted};

        // Byte for byte what a pre-0.9.0 `settings.json` holds for one
        // account: no `use_official_bw_crypto` key anywhere in it.
        let on_disk = format!(
            "{{\"id\":\"{A}\",\"email\":\"me@example.com\",\
             \"server_url\":\"https://vault.example.com\"}}"
        );
        assert!(
            !on_disk.contains("use_official_bw_crypto"),
            "control: this fixture is supposed to be a record with the key ABSENT, and it \
             carries one -- the test would prove nothing about an upgrading user"
        );
        let existing: Account =
            serde_json::from_str(&on_disk).expect("a record written by an older build must load");

        // The control for the other half of `choose`: this really is the
        // self-hosted case, so a `BwServe` answer below can only come from the
        // preference and not from the server test falling through.
        assert!(
            is_self_hosted(existing.server_url.as_deref()),
            "control: the fixture is not self-hosted, so this test is not about the \
             account it claims to be about"
        );

        assert_eq!(
            choose(existing.server_url.as_deref(), existing.use_official_bw_crypto),
            VaultBackendChoice::BwServe,
            "an existing self-hosted account was moved off `bw serve` by the upgrade. Its \
             vault is held by the CLI and it may have no stored vault key at all, so this \
             user's next launch asks for a master password against a backend that has \
             never held their vault."
        );
    }

    /// The other direction, and the reason the test above is not simply
    /// "everything is `BwServe`": a NEW self-hosted account gets the built-in
    /// client, from the same `choose`.
    ///
    /// The account is built the way production builds one -- a mint, then the
    /// sign-in that teaches it its server -- rather than by setting the field
    /// directly, so this fails if `learn_account_details` stops deriving.
    #[test]
    fn a_new_self_hosted_account_gets_the_built_in_client() {
        use crate::backend_policy::{VaultBackendChoice, choose};

        let mut minted = Account {
            id: id(A),
            email: String::new(),
            server_url: None,
            use_official_bw_crypto: true,
        };
        assert_eq!(
            choose(minted.server_url.as_deref(), minted.use_official_bw_crypto),
            VaultBackendChoice::BwServe,
            "control: a mint that has not signed in yet is on `bw serve`, so the change \
             below is the sign-in's doing and not the fixture's"
        );

        assert!(
            learn_account_details(
                &mut minted,
                Some("me@example.com"),
                Some("https://vault.example.com"),
                None,
            ),
            "the sign-in taught the account nothing"
        );

        assert!(
            !minted.use_official_bw_crypto,
            "a new self-hosted account was left on the official CLI"
        );
        assert_eq!(
            choose(minted.server_url.as_deref(), minted.use_official_bw_crypto),
            VaultBackendChoice::DirectRest,
            "a new self-hosted account did not get the built-in client"
        );
    }

    /// **The sign-in window's answer lands on `use_official_bw_crypto`, on a
    /// mint AND on an established account, in both directions.**
    ///
    /// The point of this test is that it asserts the FIELD and not the copy.
    /// A modal can paint two immaculate paragraphs and two correct buttons
    /// and still write nothing -- that is this repo's own defect class, "a
    /// test that passes because it never reached the thing it names", and a
    /// suite that checked `CliSetupState::body()` would have been fully green
    /// against it.
    ///
    /// Four cases, because the two that matter are the ones the derivation
    /// gets wrong: an established CLI account answering "built-in", and a
    /// self-hosted mint answering "the CLI". Each is paired with the
    /// derivation's own answer so a build that ignored the parameter fails
    /// on exactly half of them rather than passing on the half that agrees.
    #[test]
    fn the_sign_ins_answer_is_what_lands_on_the_account() {
        use crate::backend_policy::{VaultBackendChoice, choose};
        let established = |official| Account {
            id: AccountId::generate(),
            email: "me@example.eu".to_string(),
            server_url: Some("https://vault.example.eu".to_string()),
            use_official_bw_crypto: official,
        };
        let self_hosted = Some("https://vault.example.eu");

        for (mut account, chosen, want, what) in [
            (established(true), Some(false), false, "an established CLI account chose built-in"),
            (established(false), Some(true), true, "an established built-in account chose the CLI"),
            (a_mint(), Some(true), true, "a new self-hosted account chose the CLI"),
            (a_mint(), Some(false), false, "a new self-hosted account chose built-in"),
            // The unanswered pair, unchanged: the established account keeps
            // what it had, and the mint takes the derivation. These are the
            // control -- without them a build that wrote `chosen` in
            // unconditionally, clobbering every account a `bw status` touched,
            // would pass the four above.
            (established(true), None, true, "an unasked established CLI account"),
            (a_mint(), None, false, "an unasked new self-hosted account"),
        ] {
            learn_account_details(&mut account, Some("me@example.eu"), self_hosted, chosen);
            assert_eq!(
                account.use_official_bw_crypto, want,
                "{what}: the record says {} instead. The modal collected an answer and the \
                 account did not keep it, so the next launch settles onto the client the \
                 user did not pick",
                account.use_official_bw_crypto
            );
            // And the answer is spent, not merely stored: `choose` is what
            // actually selects a backend at startup, so this is the claim
            // the user experiences.
            assert_eq!(
                choose(account.server_url.as_deref(), account.use_official_bw_crypto),
                if want { VaultBackendChoice::BwServe } else { VaultBackendChoice::DirectRest },
                "{what}: the recorded preference does not select the backend it names"
            );
        }
    }

    /// A blank record, the shape `prepare_new_account` and `resolve_startup`
    /// mint one in. A helper because the test above needs a fresh one per
    /// case and `learn_account_details` mutates it.
    fn a_mint() -> Account {
        Account {
            id: AccountId::generate(),
            email: String::new(),
            server_url: None,
            use_official_bw_crypto: true,
        }
    }

    /// A new account on bitwarden.com gets `bw serve`, which is what keeps the
    /// CLI acquisition modal firing for the servers that need it.
    ///
    /// `server_url` stays `None` for the official cloud -- that is what `None`
    /// means here -- so this also pins that the derivation reads "no server"
    /// as official rather than as unknown-and-therefore-direct.
    #[test]
    fn a_new_bitwarden_com_account_stays_on_the_official_cli() {
        use crate::backend_policy::{VaultBackendChoice, choose};

        let mut minted = Account {
            id: id(A),
            email: String::new(),
            server_url: None,
            use_official_bw_crypto: true,
        };
        learn_account_details(&mut minted, Some("me@example.com"), None, None);

        assert!(minted.use_official_bw_crypto, "a bitwarden.com account left the official CLI");
        assert_eq!(
            choose(minted.server_url.as_deref(), minted.use_official_bw_crypto),
            VaultBackendChoice::BwServe
        );
    }

    /// **A re-sign-in must not re-open the question on an established
    /// account**, which is the guard that keeps the upgrade test above true
    /// for anything the user does after the upgrade.
    ///
    /// Both directions, because the damage runs both ways: a self-hoster on
    /// `bw serve` must not be moved to the built-in client by signing in
    /// again, and one who has chosen the built-in client must not be dragged
    /// back onto the CLI.
    #[test]
    fn signing_in_again_does_not_move_an_established_account_between_backends() {
        let mut on_the_cli = Account {
            id: id(A),
            email: "me@example.com".to_string(),
            server_url: Some("https://vault.example.com".to_string()),
            use_official_bw_crypto: true,
        };
        learn_account_details(
            &mut on_the_cli,
            Some("me@example.com"),
            Some("https://vault.example.com"),
            None,
        );
        assert!(
            on_the_cli.use_official_bw_crypto,
            "signing in again moved an established self-hosted account off `bw serve`, which \
             is the upgrade regression arriving one launch late"
        );

        let mut on_the_built_in_client =
            Account { use_official_bw_crypto: false, ..on_the_cli.clone() };
        learn_account_details(
            &mut on_the_built_in_client,
            Some("me@example.com"),
            Some("https://vault.example.com"),
            None,
        );
        assert!(
            !on_the_built_in_client.use_official_bw_crypto,
            "signing in again dragged an account back onto the official CLI"
        );
    }

    /// **The owner's case: two accounts, one machine, one backend each.**
    ///
    /// Nothing in the suite covered this before, because it could not be
    /// expressed -- the preference was one value per `settings.json`. It is
    /// spent through `choose` for each account rather than by reading the two
    /// fields back, so it fails if the decision ever stops being per-account.
    #[test]
    fn two_accounts_on_one_machine_keep_their_own_backends() {
        use crate::backend_policy::{VaultBackendChoice, choose};

        let official = Account {
            id: id(A),
            email: "me@example.com".to_string(),
            server_url: None,
            use_official_bw_crypto: true,
        };
        let self_hosted = Account {
            id: id(B),
            email: "me@example.com".to_string(),
            server_url: Some("https://vault.example.com".to_string()),
            use_official_bw_crypto: false,
        };

        let mut state = AccountsState::new(
            crate::bw_path::MultiAccountAvailability::Available,
            vec![official.clone(), self_hosted.clone()],
            official.id.clone(),
        )
        .expect("two accounts is a valid state");

        let backend_of = |a: &Account| choose(a.server_url.as_deref(), a.use_official_bw_crypto);

        assert_eq!(backend_of(state.active()), VaultBackendChoice::BwServe);
        // The switch, through `adopt` -- the one mutation every switch goes
        // through, and the same one `main`'s `switch_to_account` performs.
        state.adopt(self_hosted.clone());
        assert_eq!(
            backend_of(state.active()),
            VaultBackendChoice::DirectRest,
            "after switching to the self-hosted account the app is still on `bw serve`, so \
             the backend is not following the account"
        );
        // And back, so the two answers above are telling the accounts apart
        // rather than reporting one constant twice.
        state.adopt(official.clone());
        assert_eq!(backend_of(state.active()), VaultBackendChoice::BwServe);
    }

    /// Preferences writes the backend choice into the account, and into every
    /// copy of it that gets persisted.
    #[test]
    fn choosing_a_backend_in_preferences_writes_it_to_the_active_account() {
        let mut state = AccountsState::new(
            crate::bw_path::MultiAccountAvailability::Available,
            vec![account(A), account(B)],
            id(A),
        )
        .expect("two accounts is a valid state");
        assert!(state.active().use_official_bw_crypto, "control: the fixture starts on the CLI");

        assert!(state.set_active_backend_preference(false), "the write reported no change");
        assert!(!state.active().use_official_bw_crypto);
        assert!(
            !account_for(state.all(), &id(A)).expect("the account is in the list").use_official_bw_crypto,
            "the choice reached `active` but not the list that gets persisted, so it is lost \
             on the next launch"
        );
        assert!(
            account_for(state.all(), &id(B)).expect("the other account").use_official_bw_crypto,
            "choosing a backend for one account changed the other one's"
        );
        assert!(
            !state.set_active_backend_preference(false),
            "writing the same value again reported a change, so every visit to Preferences \
             rewrites settings.json"
        );
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
            assert_eq!(user_key_path_for(cfg, &a).parent(), Some(dir.as_path()));
            assert_eq!(
                user_key_path_for(cfg, &a).file_name(),
                Some(std::ffi::OsStr::new("userkey.bin"))
            );
            assert_eq!(dir.file_name(), Some(std::ffi::OsStr::new(a.as_str())));
        }
    }

    #[test]
    fn no_secret_of_any_account_lands_in_the_shared_config_directory() {
        // The single-account app kept `session.bin` and `hello.bin` directly
        // in `config_dir`. Nothing does now -- if one account's blob resolved
        // back to the shared directory it would be found (and deleted, and
        // overwritten) by every other account.
        let cfg = Path::new(r"C:\cfg");
        for raw in [A, B, &"0".repeat(32), &"f".repeat(32)] {
            let a = id(raw);
            assert_ne!(session_path_for(cfg, &a), PathBuf::from(r"C:\cfg\session.bin"));
            assert_ne!(hello_blob_path_for(cfg, &a), PathBuf::from(r"C:\cfg\hello.bin"));
            assert!(session_path_for(cfg, &a).starts_with(accounts_root(cfg)));
            assert!(hello_blob_path_for(cfg, &a).starts_with(accounts_root(cfg)));
            // The master key is the strongest of the three -- unlike a session
            // token it does not expire -- so the rule that keeps one account's
            // secret out of the shared directory is asserted for it too rather
            // than left to be inferred from the other two.
            assert_ne!(user_key_path_for(cfg, &a), PathBuf::from(r"C:\cfg\userkey.bin"));
            assert!(user_key_path_for(cfg, &a).starts_with(accounts_root(cfg)));
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
        // single-account derivation, so a stale hello.bin still sitting in
        // `config_dir` would silently open under this account's identity.
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

    // --------------------------------------------------------------- 10

    mod accounts_state {
        use super::*;
        use crate::bw_path::MultiAccountAvailability;

        fn a() -> Account {
            account(A)
        }

        fn b() -> Account {
            account(B)
        }

        fn c() -> Account {
            account(&"a".repeat(32))
        }

        fn trap() -> MultiAccountAvailability {
            MultiAccountAvailability::BlockedByPortableProfile {
                relative_data_dir: PathBuf::from(r"C:\a\bin\bitwarden-cli"),
            }
        }

        fn state(
            availability: MultiAccountAvailability,
            accounts: Vec<Account>,
            active: &AccountId,
        ) -> AccountsState {
            AccountsState::new(availability, accounts, active.clone())
                .expect("these accounts are not empty")
        }

        fn switch_ids(state: &AccountsState) -> Vec<AccountId> {
            state.switchable().iter().map(|x| x.id.clone()).collect()
        }

        /// [`AccountsState::from_blocked_reason`] is what `vault_window`'s
        /// switcher tests build their states with, because that file may not
        /// name either of [`AccountsState::new`]'s inputs. A constructor only
        /// tests use is a constructor that can quietly stop matching the one
        /// production uses — and every switcher assertion over there would
        /// keep passing against it.
        ///
        /// Both states, not just the blocked one: an available state that
        /// offered no targets would make every "the switcher offers nothing"
        /// assertion in that file pass for the wrong reason.
        #[test]
        fn the_test_constructor_agrees_with_the_real_one() {
            let real_open = state(MultiAccountAvailability::Available, vec![a(), b()], &a().id);
            let open = AccountsState::from_blocked_reason(vec![a(), b()], a().id, None)
                .expect("these accounts are not empty");
            assert_eq!(open, real_open);
            assert_eq!(switch_ids(&open), vec![b().id], "control: it offers b");

            let real_blocked = state(trap(), vec![a(), b()], &a().id);
            let blocked = AccountsState::from_blocked_reason(
                vec![a(), b()],
                a().id,
                trap().explanation(),
            )
            .expect("these accounts are not empty");
            assert_eq!(blocked, real_blocked);
            assert!(
                blocked.switchable().is_empty(),
                "control: the blocked one really does offer nothing"
            );
            assert_ne!(
                open, blocked,
                "control: the two states this constructor is asked for are different states"
            );
        }

        #[test]
        fn a_blocked_availability_offers_no_switch_targets_and_no_add() {
            let s = state(trap(), vec![a(), b()], &a().id);
            assert!(
                s.switchable().is_empty(),
                "a switch was offered while the CLI ignores our env var: {:?}",
                switch_ids(&s)
            );
            assert!(!s.can_add(), "an account was addable into one shared profile");
            let why = s.blocked_reason().expect("the refusal must say why");
            assert!(
                why.contains(r"C:\a\bin\bitwarden-cli"),
                "the message must name the directory the user can remove, got: {why}"
            );
            assert_eq!(
                s.all().len(),
                2,
                "the list is still shown -- it is SWITCHING that is refused"
            );
            assert_eq!(s.active().id, a().id);
        }

        #[test]
        fn an_unknown_cli_path_refuses_a_switch_exactly_as_the_portable_profile_does() {
            // The variant the plan's own tests never name. An implementation
            // that matched on `BlockedByPortableProfile` alone would pass every
            // one of them and switch freely on the machine where the trap
            // cannot be CHECKED for -- which is the state this variant exists
            // to report, and the one where a wrong answer is unfalsifiable.
            let s = state(
                MultiAccountAvailability::BlockedByUnknownCliPath,
                vec![a(), b()],
                &a().id,
            );
            assert!(
                s.switchable().is_empty(),
                "a switch was offered without knowing where the CLI reads its profile from"
            );
            assert!(!s.can_add());
            assert!(s.blocked_reason().is_some());
            assert_eq!(s.all().len(), 2);
        }

        #[test]
        fn an_available_state_offers_every_account_except_the_active_one() {
            // The positive control for both tests above, and the rule in its
            // own right: "switch to the account you are already on" is a no-op
            // that would still tear the backend down and demand a master
            // password.
            let s = state(MultiAccountAvailability::Available, vec![a(), b(), c()], &b().id);
            assert_eq!(
                switch_ids(&s),
                vec![a().id, c().id],
                "in configured order, and without the active account"
            );
            assert!(s.can_add());
            assert_eq!(s.blocked_reason(), None);
            assert_eq!(s.active().id, b().id);
            assert_eq!(s.all().len(), 3);
        }

        #[test]
        fn an_active_id_naming_no_stored_account_falls_back_to_the_first() {
            // Reachable: `settings.json` is a user-editable file, and a removal
            // that crashed between rewriting the list and rewriting
            // `active_account` leaves exactly this. The fallback must be a
            // REAL account -- an active account that is not in the list would
            // point the CLI at a directory nothing created, and would then also
            // appear in its own switch targets.
            let ghost = id(&"9".repeat(32));
            let s = state(MultiAccountAvailability::Available, vec![b(), a()], &ghost);
            assert_eq!(s.active().id, b().id, "the first configured account");
            assert_eq!(
                switch_ids(&s),
                vec![a().id],
                "the fallback active account must not also be offered as a target"
            );
            assert!(
                !switch_ids(&s).contains(&ghost),
                "an id that names no account was offered as somewhere to switch to"
            );
            assert!(s.can_add());
        }

        #[test]
        fn an_empty_account_list_is_not_an_accounts_state_at_all() {
            // `StartupAccounts::NoAccountList` is a startup condition rather
            // than an `AccountsState` holding nothing. If this constructed,
            // `active()` would have to invent an account or panic -- and an
            // invented one points the CLI at an empty directory, which presents
            // as "signed out" with the real vault untouched a few directories
            // away.
            for availability in [
                crate::bw_path::MultiAccountAvailability::Available,
                MultiAccountAvailability::BlockedByUnknownCliPath,
                trap(),
            ] {
                assert!(
                    AccountsState::new(availability.clone(), vec![], a().id).is_none(),
                    "{availability:?} built a state with no accounts"
                );
                // The positive control on the same call: one account and the
                // same availability does build.
                assert!(
                    AccountsState::new(availability.clone(), vec![a()], a().id).is_some(),
                    "{availability:?} refused a perfectly good single account"
                );
            }
        }

        #[test]
        fn a_single_account_has_nowhere_to_switch_to_but_may_still_be_added_to() {
            let s = state(MultiAccountAvailability::Available, vec![a()], &a().id);
            assert!(s.switchable().is_empty());
            assert!(
                s.can_add(),
                "the only way a second account ever arrives is barred"
            );
            assert_eq!(s.blocked_reason(), None);
            assert_eq!(s.all().len(), 1);
        }

        #[test]
        fn a_duplicated_id_is_offered_once_rather_than_as_two_doors_onto_one_directory() {
            // A hand-edited `settings.json` again. Two entries with one id are
            // two menu items that switch to the same data directory.
            let s = state(
                crate::bw_path::MultiAccountAvailability::Available,
                vec![a(), b(), b(), a()],
                &a().id,
            );
            assert_eq!(switch_ids(&s), vec![b().id]);
            assert_eq!(s.all().len(), 4, "what is stored is still reported as stored");
        }

        #[test]
        fn every_combination_of_the_block_and_the_list_obeys_one_rule() {
            // The whole decision table, so no single combination can be
            // special-cased into working while another silently is not. Three
            // availabilities x four account lists, each with an active account
            // that is in the list and one that is not.
            let availabilities = [
                (MultiAccountAvailability::Available, true),
                (MultiAccountAvailability::BlockedByUnknownCliPath, false),
                (trap(), false),
            ];
            let lists = [
                vec![a()],
                vec![a(), b()],
                vec![a(), b(), c()],
                vec![b(), c()], // the active id names none of these
            ];

            let mut ever_offered = 0usize;
            for (availability, allowed) in &availabilities {
                for list in &lists {
                    let s = state(availability.clone(), list.clone(), &a().id);
                    let allowed = *allowed;
                    let label = format!("{availability:?} / {list:?}");

                    assert_eq!(s.can_add(), allowed, "can_add disagrees for {label}");
                    assert_eq!(
                        s.blocked_reason().is_none(),
                        allowed,
                        "blocked_reason disagrees for {label}"
                    );
                    assert_eq!(s.all(), &list[..], "all() rewrote the stored list for {label}");

                    // The active account is always one of the stored ones,
                    // and is never a switch target.
                    assert!(
                        list.iter().any(|x| x.id == s.active().id),
                        "the active account is not in the list for {label}"
                    );
                    assert!(
                        !switch_ids(&s).contains(&s.active().id),
                        "the active account was offered as a target for {label}"
                    );

                    let expected: Vec<AccountId> = if allowed {
                        list.iter()
                            .map(|x| x.id.clone())
                            .filter(|x| x != &s.active().id)
                            .collect()
                    } else {
                        vec![]
                    };
                    assert_eq!(switch_ids(&s), expected, "switchable disagrees for {label}");
                    ever_offered += s.switchable().len();
                }
            }
            // The positive control over the whole table: "refused" is not the
            // answer everywhere, which is what a `switchable()` that returned
            // an empty slice unconditionally would look like.
            assert!(
                ever_offered > 0,
                "no combination in the whole table offered a single switch target"
            );
            // And the unblocked corner really does depend on the account list
            // rather than on the availability alone.
            let one = state(MultiAccountAvailability::Available, vec![a()], &a().id);
            let two = state(MultiAccountAvailability::Available, vec![a(), b()], &a().id);
            assert!(one.switchable().is_empty());
            assert_eq!(switch_ids(&two), vec![b().id]);
        }

        #[test]
        fn an_unavailable_cli_always_explains_itself() {
            // `AccountsState` decides on `explanation()` being `Some`, so
            // "blocked" and "has something to say" have to be the same set. If
            // a variant ever explained nothing, the door would silently swing
            // open for it -- which is exactly how `BlockedByUnknownCliPath`
            // would have got through.
            for availability in [
                MultiAccountAvailability::BlockedByUnknownCliPath,
                trap(),
            ] {
                assert!(!availability.is_available(), "{availability:?}");
                assert!(
                    availability.explanation().is_some(),
                    "{availability:?} blocks multi-account but says nothing about it"
                );
            }
            assert!(MultiAccountAvailability::Available.is_available());
            assert_eq!(MultiAccountAvailability::Available.explanation(), None);
        }

        /// Both menus that offer "Remove this account" ask this, and both would
        /// otherwise offer an item that can only fail: `remove_account` refuses
        /// the last account (nowhere to land) and refuses while blocked (it
        /// cannot reach the survivor). The whole table, so neither refusal can
        /// be special-cased into working while the other silently is not.
        #[test]
        fn the_active_account_is_removable_only_when_there_is_a_survivor_to_reach() {
            for availability in [
                MultiAccountAvailability::BlockedByUnknownCliPath,
                trap(),
            ] {
                assert!(
                    !state(availability.clone(), vec![a(), b(), c()], &a().id).can_remove_active(),
                    "{availability:?} offered a removal it cannot settle away from first"
                );
            }
            assert!(
                !state(MultiAccountAvailability::Available, vec![a()], &a().id)
                    .can_remove_active(),
                "the only account was offered for removal, which leaves the app with no \
                 profile to point the CLI at"
            );
            // The positive controls, on the same call: two accounts and nothing
            // blocking really is removable, from either end of the list.
            assert!(
                state(MultiAccountAvailability::Available, vec![a(), b()], &a().id)
                    .can_remove_active()
            );
            assert!(
                state(MultiAccountAvailability::Available, vec![a(), b()], &b().id)
                    .can_remove_active()
            );
            // And it agrees with the survivor `remove_account` would actually
            // pick, rather than being a second rule that happens to line up.
            for list in [vec![a()], vec![a(), b()], vec![a(), b(), c()]] {
                let s = state(MultiAccountAvailability::Available, list.clone(), &a().id);
                assert_eq!(
                    s.can_remove_active(),
                    next_active_after_removal(&list, &a().id).is_some(),
                    "for {list:?}"
                );
            }
        }

        /// The bug behind the "random hash": an account minted on a first
        /// install has no email, so `account_label` names it by its directory.
        #[test]
        fn a_first_run_account_learns_its_address_and_stops_being_named_by_its_id() {
            let blank = Account {
                id: id(A),
                email: String::new(),
                server_url: None,
                use_official_bw_crypto: true,
            };
            let mut s = state(
                crate::bw_path::MultiAccountAvailability::Available,
                vec![blank.clone(), b()],
                &blank.id,
            );
            assert_eq!(
                account_label(s.active()),
                A,
                "control: before it learns anything it really is named by its id"
            );

            assert!(s.learn_active_details(
                Some("ana@example.com"),
                Some("https://vault.example.eu")
            ));
            assert_eq!(account_label(s.active()), "ana@example.com");
            assert_eq!(
                s.active().server_url.as_deref(),
                Some("https://vault.example.eu")
            );
            // What gets PERSISTED is `all()`, so the copy in there has to have
            // learned it too -- otherwise the next launch reads the hash back.
            assert_eq!(s.all()[0].email, "ana@example.com");
            assert_eq!(s.all()[1], b(), "the other account was rewritten as well");
            // Nothing new to say the second time, so nothing to write.
            assert!(!s.learn_active_details(
                Some("ana@example.com"),
                Some("https://vault.example.eu")
            ));
        }

        /// `bw status` answers `null` for both fields when the CLI is logged
        /// out or could not be spawned at all. Treating that as an answer would
        /// erase an address already on disk and put the hash back.
        #[test]
        fn a_silent_bw_status_never_unlearns_an_address_the_app_already_had() {
            let mut s = state(MultiAccountAvailability::Available, vec![a(), b()], &a().id);
            assert!(!s.learn_active_details(None, None));
            assert!(!s.learn_active_details(Some(""), Some("")));
            assert_eq!(s.active().email, "me@example.com");
            assert_eq!(account_label(s.active()), "me@example.com");
            // The positive control on the same state: a real answer still lands.
            assert!(s.learn_active_details(Some("someone@example.com"), None));
            assert_eq!(s.active().email, "someone@example.com");
            assert_eq!(
                s.active().server_url, None,
                "a server URL was invented from an answer that did not carry one"
            );
        }

        /// `bw login` replaces whatever profile it is pointed at, so a
        /// directory really can change hands. Keeping the old address there
        /// would name the wrong person in every menu.
        #[test]
        fn a_directory_that_changed_hands_is_relearned_rather_than_left_stale() {
            let mut s = state(MultiAccountAvailability::Available, vec![a(), b()], &a().id);
            assert!(s.learn_active_details(Some("someone-else@example.com"), None));
            assert_eq!(s.active().email, "someone-else@example.com");
            assert_eq!(s.all()[0].email, "someone-else@example.com");
            assert_eq!(
                s.all()[1].email,
                "me@example.com",
                "the OTHER account's address was rewritten by the active one's status"
            );
            assert_eq!(
                switch_ids(&s),
                vec![b().id],
                "learning an address changed what the app offers to switch to"
            );
        }

        /// A hand-edited `settings.json` can list one id twice. `all()` is what
        /// gets written back, so both copies have to learn -- a stale one would
        /// come back as the account's address on the next launch, and which of
        /// the two wins is `account_for`'s "first entry only".
        #[test]
        fn a_duplicated_active_id_learns_in_every_copy_that_gets_persisted() {
            let blank = Account {
                id: id(A),
                email: String::new(),
                server_url: None,
                use_official_bw_crypto: true,
            };
            let mut s = state(
                crate::bw_path::MultiAccountAvailability::Available,
                vec![blank.clone(), b(), blank.clone()],
                &blank.id,
            );
            assert!(s.learn_active_details(Some("ana@example.com"), None));
            assert_eq!(s.all()[0].email, "ana@example.com");
            assert_eq!(
                s.all()[2].email,
                "ana@example.com",
                "the second copy of the active account kept the empty email that put its id \
                 in the menu"
            );
            assert_eq!(s.all()[1].email, "me@example.com", "control: not everything");
        }

        /// The files that must ask [`AccountsState`] rather than answer for
        /// themselves. `main.rs` is deliberately not among them: it is where
        /// the availability is produced, so it is the one place that
        /// legitimately names it.
        ///
        /// Read off disk rather than with `include_str!` so this list can name
        /// files in subdirectories, and so a file that stops existing is a
        /// failure rather than a compile error nobody reads.
        const MUST_NOT_DECIDE_FOR_THEMSELVES: [&str; 6] = [
            "tray.rs",
            "login_ui.rs",
            "picker_ui.rs",
            "prefs_ui.rs",
            "vault_window/mod.rs",
            "vault_window/sidebar.rs",
        ];

        #[test]
        fn no_window_answers_may_i_switch_for_itself() {
            // A gate nothing asks is the same as no gate. A window that asked
            // `multi_account_availability()` for itself would answer it at a
            // different moment from startup -- a `bitwarden-cli` directory that
            // appeared since, or a `bw.exe` that has not been verified in this
            // process -- and would then offer a switch that silently shares one
            // profile between two identities. Nothing reports an error when
            // that happens, so it is not visible in an end state.
            //
            // NEEDLES SPLIT ACROSS `concat!` ARGUMENTS, DELIBERATELY: written
            // whole, each would match its own declaration in this file, and
            // the positive control below would pass against an empty scan.
            // Single-line for the same reason every other guard here is: this
            // module is LF and the files it reads are not necessarily.
            let banned = [
                concat!("MultiAccount", "Availability"),
                concat!("multi_account_", "availability"),
            ];
            // `CARGO_MANIFEST_DIR`, not `file!()`: the latter is relative to
            // the package root and would depend on the test binary's working
            // directory.
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
            let mut scanned = 0usize;
            for name in MUST_NOT_DECIDE_FOR_THEMSELVES {
                let path = dir.join(name);
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
                scanned += 1;
                for needle in banned {
                    assert!(
                        !source.contains(needle),
                        "{name} names `{needle}` itself. Whether this process may switch \
                         accounts is `AccountsState`'s answer and nothing else's: a window \
                         that reads that fact a second time will disagree with this one, and \
                         the disagreement is silent -- a switch that shares one profile \
                         between two identities."
                    );
                }
            }
            // A literal, not `MUST_NOT_DECIDE_FOR_THEMSELVES.len()`: a test
            // that re-derives its expectation from the constant under test
            // passes for every value that constant could hold, including an
            // empty list -- which is what deleting this guard looks like from
            // the outside.
            assert_eq!(scanned, 6, "the scan did not read every file it names");
            // Positive controls, on the same needles and the same reader: they
            // ARE spelled that way, and they DO appear in the file that is
            // allowed to name them.
            let mine = std::fs::read_to_string(dir.join("accounts.rs"))
                .expect("cannot read this module's own source");
            for needle in banned {
                assert!(
                    mine.contains(needle),
                    "the needle `{needle}` is not spelled that way in accounts.rs, so the scan \
                     above proves nothing"
                );
            }
        }
    }

    // ---------------------------------------------------------------- 11.1

    mod startup_resolution {
        use super::*;

        fn ready(r: StartupAccounts) -> (Account, Vec<Account>, bool, bool) {
            match r {
                StartupAccounts::Ready {
                    active,
                    accounts,
                    needs_persist,
                    first_run,
                } => (active, accounts, needs_persist, first_run),
                other => panic!("{other:?}"),
            }
        }

        #[test]
        fn the_stored_active_account_is_resumed_on_a_later_launch() {
            let (a, b) = (account(A), account(B));
            let (active, accounts, needs_persist, first_run) =
                ready(resolve_startup(&[a.clone(), b.clone()], Some(&b.id), None));
            assert_eq!(
                active.id, b.id,
                "a restart must resume the account that was last active"
            );
            assert_eq!(accounts.len(), 2, "and must not drop the others");
            assert!(!needs_persist, "nothing changed, so nothing is rewritten");
            assert!(
                !first_run,
                "a resumed account has been signed in to; saying otherwise puts the first-run \
                 notice in front of the user on every launch forever"
            );
        }

        #[test]
        fn an_active_id_naming_no_stored_account_falls_back_to_the_first() {
            // A hand-edited settings.json, or an account removed by a build
            // that crashed mid-write. Falling through to "no active account"
            // would leave the app with nothing to point the CLI at.
            let a = account(A);
            let ghost = AccountId::parse(&"9".repeat(32)).unwrap();
            let (active, _, needs_persist, first_run) =
                ready(resolve_startup(&[a.clone()], Some(&ghost), None));
            assert_eq!(active.id, a.id);
            assert!(
                needs_persist,
                "the dangling active id must be corrected on disk"
            );
            assert!(
                !first_run,
                "the account was stored, so it is not this launch's doing"
            );
        }

        #[test]
        fn a_first_run_mints_exactly_one_account_and_says_it_is_new() {
            // No account list, and nothing to import: Deskwarden does not
            // touch whatever the Bitwarden CLI may already hold under
            // `%APPDATA%`. It mints one directory and the login window asks
            // for a master password. `first_run` is what tells the window to
            // say why.
            let (active, accounts, needs_persist, first_run) =
                ready(resolve_startup(&[], None, None));
            assert_eq!(
                accounts.len(),
                1,
                "a first run must mint exactly one account, got {accounts:?}"
            );
            assert!(needs_persist, "the minted account must reach settings.json");
            assert_eq!(
                accounts[0].id, active.id,
                "the minted account is the one that becomes active"
            );
            assert!(
                accounts[0].email.is_empty(),
                "nobody has signed in yet, so there is no address to claim"
            );
            assert!(first_run, "the minted account has never been signed in to");
            // And the id really is minted rather than a constant: two first
            // runs are two different directories.
            let (second, ..) = ready(resolve_startup(&[], None, None));
            assert_ne!(second.id, active.id);
        }

        #[test]
        fn an_unreadable_account_list_mints_nothing_at_all() {
            // `Settings::load` answers a failed parse with an EMPTY account
            // list, which is indistinguishable here from a first run. Minting
            // on it would point the CLI at an empty directory while the user's
            // account sat in `accounts/<old-id>/` -- and `Settings::save`
            // refuses to write over a file it could not read, so the fresh id
            // would not even be recorded: the next launch would mint another,
            // and the one after that another.
            let r = resolve_startup(&[], None, Some("settings.json could not be read"));
            let StartupAccounts::NoAccountList { reason } = r else {
                panic!("an unreadable account list minted an account: {r:?}")
            };
            assert!(reason.contains("could not be read"), "{reason}");
            // The control, on the same call with the same empty list: a
            // READABLE empty list does mint. Without this the refusal above
            // could be "never mint anything", which is the app never starting.
            let (_, accounts, ..) = ready(resolve_startup(&[], None, None));
            assert_eq!(accounts.len(), 1);
        }

        #[test]
        fn a_stored_account_outranks_an_unreadable_list() {
            // Unreachable through `Settings::load` -- a failed parse yields no
            // accounts -- and spelled out anyway, because the alternative
            // ordering (refuse whenever the flag is set) would throw away a
            // perfectly good account list and take the app account-less.
            let a = account(A);
            let (active, _, _, first_run) = ready(resolve_startup(
                &[a.clone()],
                Some(&a.id),
                Some("settings.json could not be read"),
            ));
            assert_eq!(active.id, a.id);
            assert!(!first_run);
        }

        #[test]
        fn ensure_account_dir_creates_the_directory_a_session_token_needs_and_repeats_safely() {
            // `SessionStore::new`'s contract is that the parent directory
            // already exists; nothing else on the first-install path creates
            // it. Scratch directory under %TEMP% -- never the real config
            // directory.
            let root = std::env::temp_dir().join(format!(
                "deskwarden-ensure-dir-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let id = AccountId::generate();
            assert!(
                !data_dir_for(&root, &id).is_dir(),
                "control: the directory is not there before the call"
            );

            let made = ensure_account_dir(&root, &id).expect("the directory must be created");
            assert_eq!(made, data_dir_for(&root, &id));
            assert!(made.is_dir());

            // Idempotent, and it does not clear what is already inside: a
            // second launch calls this before reading the session token.
            std::fs::write(session_path_for(&root, &id), b"wrapped").unwrap();
            ensure_account_dir(&root, &id).expect("the second call must succeed too");
            assert!(
                session_path_for(&root, &id).is_file(),
                "the second call threw away the account's session token"
            );

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    // ---------------------------------------------------------------- 12

    mod preparing_an_account {
        use super::*;

        /// A scratch config directory under `%TEMP%`. **Never** the real one:
        /// these tests call `remove_dir_all`.
        struct Scratch(PathBuf);

        impl Scratch {
            fn new(tag: &str) -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "deskwarden-prepare-{tag}-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                Self(dir)
            }

            fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn names_under(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }

        #[test]
        fn a_prepared_account_gets_a_real_directory_and_names_nobody_yet() {
            let cfg = Scratch::new("made");
            assert!(
                !accounts_root(cfg.path()).exists(),
                "control: the accounts root is not there before the call, so the assertions \
                 below are about what this created"
            );

            let first = prepare_new_account(cfg.path()).expect("a new account must be preparable");

            // The directory must exist BEFORE anything writes into it:
            // `SessionStore::new` will not create its own parent, so without
            // this the very first `store.save` fails and the account presents
            // as a master-password prompt on every launch, forever.
            assert!(
                data_dir_for(cfg.path(), &first.id).is_dir(),
                "the account has no directory to hold its session token"
            );
            assert!(
                session_path_for(cfg.path(), &first.id)
                    .parent()
                    .is_some_and(Path::is_dir),
                "the session token's parent directory does not exist"
            );
            // Empty by construction: nobody has signed in, so there is no
            // address to record and the sign-in is what fills it in.
            assert_eq!(first.email, "");
            assert_eq!(first.server_url, None);

            // Two prepares are two accounts, not one directory shared.
            let second = prepare_new_account(cfg.path()).expect("a second one too");
            assert_ne!(first.id, second.id);
            assert_eq!(
                names_under(&accounts_root(cfg.path())),
                {
                    let mut both = vec![first.id.to_string(), second.id.to_string()];
                    both.sort();
                    both
                },
                "the two prepared accounts are not two directories"
            );
        }

        #[test]
        fn a_prepare_that_cannot_create_the_directory_reports_it_rather_than_inventing_an_account()
        {
            // A file where the accounts root should be: the same shape as a
            // read-only or otherwise unwritable config directory. An `Account`
            // returned here would be persisted and switched to, and the switch
            // would point the CLI at a directory that does not exist.
            let cfg = Scratch::new("blocked");
            std::fs::write(accounts_root(cfg.path()), b"not a directory").unwrap();

            let e = prepare_new_account(cfg.path())
                .expect_err("an account was minted with nowhere to put it");
            assert!(
                e.contains("accounts"),
                "the failure must name the path that could not be made, got: {e}"
            );
            assert!(
                accounts_root(cfg.path()).is_file(),
                "control: the obstruction is still a file, so the call really did fail on it"
            );
        }

        #[test]
        fn discarding_a_prepared_account_takes_its_secrets_and_only_its_own() {
            let cfg = Scratch::new("discard");
            let doomed = prepare_new_account(cfg.path()).expect("prepared");
            let keeper = prepare_new_account(cfg.path()).expect("prepared");
            // What an abandoned sign-in can leave behind: a CLI profile, a
            // session token, and -- if the user ticked "Use Windows Hello"
            // before the switch failed -- a blob sealing a master password for
            // an account that is about to stop existing.
            for id in [&doomed.id, &keeper.id] {
                std::fs::write(session_path_for(cfg.path(), id), b"wrapped").unwrap();
                std::fs::write(hello_blob_path_for(cfg.path(), id), b"sealed").unwrap();
                std::fs::write(user_key_path_for(cfg.path(), id), b"wrapped-key").unwrap();
                std::fs::create_dir_all(data_dir_for(cfg.path(), id).join("bw")).unwrap();
            }

            discard_prepared_account(cfg.path(), &doomed.id);

            assert!(!data_dir_for(cfg.path(), &doomed.id).exists());
            assert!(!session_path_for(cfg.path(), &doomed.id).exists());
            assert!(!hello_blob_path_for(cfg.path(), &doomed.id).exists());
            assert!(!user_key_path_for(cfg.path(), &doomed.id).exists());
            // The positive controls, without which every assertion above
            // passes against a call that deleted the whole accounts root:
            assert!(
                data_dir_for(cfg.path(), &keeper.id).is_dir(),
                "the OTHER account's profile went with it"
            );
            assert!(
                hello_blob_path_for(cfg.path(), &keeper.id).exists(),
                "the other account's sealed master password went with it"
            );
            assert!(
                accounts_root(cfg.path()).is_dir(),
                "the accounts root was deleted"
            );
            assert!(cfg.path().is_dir(), "the config directory was deleted");

            // Idempotent: a discard runs on a path where something already
            // went wrong, and "already gone" is the goal state.
            discard_prepared_account(cfg.path(), &doomed.id);
            assert!(!data_dir_for(cfg.path(), &doomed.id).exists());
        }

        #[test]
        fn a_deletion_that_fails_says_so_rather_than_reporting_the_account_gone() {
            // `discard_prepared_account` can only log; the account removal in
            // `main` has a user in front of it and a `settings.json` write
            // behind it, so it needs to be able to tell "the profile is gone"
            // from "the profile is still sitting there".
            let cfg = Scratch::new("delete-fails");
            let obstructed = AccountId::generate();
            std::fs::create_dir_all(accounts_root(cfg.path())).unwrap();
            // A file where the account's directory should be: an undeletable
            // directory (a `bw` still holding data.json open) has the same
            // shape and cannot be arranged deterministically in a test.
            std::fs::write(data_dir_for(cfg.path(), &obstructed), b"not a directory").unwrap();

            let e = delete_account_dir(cfg.path(), &obstructed)
                .expect_err("a deletion that did not happen was reported as done");
            assert!(
                e.contains(obstructed.as_str()),
                "the failure must name the path that survived, got: {e}"
            );
            assert!(
                data_dir_for(cfg.path(), &obstructed).is_file(),
                "control: the obstruction is still there, so the call really did fail on it"
            );

            // The positive controls on the same function: a real account
            // directory IS deleted and reported as such, and a second call for
            // one already gone is success rather than a failure the caller
            // would have to explain to the user.
            let real = prepare_new_account(cfg.path()).expect("prepared");
            std::fs::write(hello_blob_path_for(cfg.path(), &real.id), b"sealed").unwrap();
            assert_eq!(delete_account_dir(cfg.path(), &real.id), Ok(()));
            assert!(!data_dir_for(cfg.path(), &real.id).exists());
            assert_eq!(delete_account_dir(cfg.path(), &real.id), Ok(()));
        }
    }

    mod adopting_an_account {
        use super::*;
        use crate::bw_path::MultiAccountAvailability;

        fn state(
            availability: MultiAccountAvailability,
            list: Vec<Account>,
            active: &AccountId,
        ) -> AccountsState {
            AccountsState::new(availability, list, active.clone())
                .expect("these accounts are not empty")
        }

        fn switch_ids(state: &AccountsState) -> Vec<AccountId> {
            state.switchable().iter().map(|x| x.id.clone()).collect()
        }

        #[test]
        fn an_adopted_account_becomes_active_and_the_one_it_left_becomes_a_target() {
            let a = account(A);
            let mut s = state(MultiAccountAvailability::Available, vec![a.clone()], &a.id);
            assert!(
                s.switchable().is_empty(),
                "control: there was nowhere to switch to before the add"
            );

            let added = Account {
                id: id(B),
                email: "new@example.com".to_string(),
                server_url: Some("https://vault.example.com".to_string()),
                use_official_bw_crypto: true,
            };
            s.adopt(added.clone());

            assert_eq!(s.active(), &added, "the add did not settle onto the account it added");
            assert_eq!(s.all().len(), 2);
            assert_eq!(
                s.all().last(), Some(&added),
                "the added account is not in the list the switcher shows"
            );
            assert_eq!(
                switch_ids(&s),
                vec![a.id.clone()],
                "the account the user came from is not offered as somewhere to go back to"
            );
            assert!(
                !switch_ids(&s).contains(&added.id),
                "the account just switched onto is offered as somewhere to switch to"
            );
            assert!(s.can_add(), "a second add is barred by the first");
        }

        #[test]
        fn adopting_the_account_already_active_neither_duplicates_it_nor_offers_it() {
            // Not reachable from an add -- the id is fresh -- and exactly what
            // a resumed switch would do. Appending would put two menu entries
            // on one directory.
            let (a, b) = (account(A), account(B));
            let mut s = state(
                crate::bw_path::MultiAccountAvailability::Available,
                vec![a.clone(), b.clone()],
                &a.id,
            );
            s.adopt(b.clone());
            assert_eq!(s.all().len(), 2, "the account was added a second time");
            assert_eq!(s.active().id, b.id);
            assert_eq!(switch_ids(&s), vec![a.id]);
        }

        #[test]
        fn adopting_does_not_unblock_a_state_that_was_blocked() {
            // `adopt` carries the two facts rather than re-deriving them, and
            // this is what that has to mean: an add cannot talk its way past
            // the `relativeDataDir` trap by mutating the state afterwards.
            let (a, b) = (account(A), account(B));
            let mut s = state(
                MultiAccountAvailability::BlockedByUnknownCliPath,
                vec![a.clone()],
                &a.id,
            );
            assert!(!s.can_add());
            s.adopt(b.clone());
            assert!(
                !s.can_add(),
                "a blocked state was unblocked by adopting an account into it"
            );
            assert!(
                s.switchable().is_empty(),
                "a blocked state offered a switch target after an adopt: {:?}",
                switch_ids(&s)
            );
            assert!(s.blocked_reason().is_some());
            // The positive control on the same call: the unblocked state DOES
            // offer one, so the emptiness above is the block rather than
            // `adopt` never computing targets at all.
            let mut open = state(MultiAccountAvailability::Available, vec![a.clone()], &a.id);
            open.adopt(b.clone());
            assert_eq!(switch_ids(&open), vec![a.id]);
        }
    }

    // ---------------------------------------------------------------- 13

    mod forgetting_an_account {
        use super::*;
        use crate::bw_path::MultiAccountAvailability;

        fn state(list: Vec<Account>, active: &AccountId) -> AccountsState {
            AccountsState::new(MultiAccountAvailability::Available, list, active.clone())
                .expect("these accounts are not empty")
        }

        fn ids(accounts: &[Account]) -> Vec<AccountId> {
            accounts.iter().map(|x| x.id.clone()).collect()
        }

        #[test]
        fn a_forgotten_account_leaves_the_list_and_the_switch_targets_together() {
            // Two answers, and a removal that updated only one of them would
            // leave the switcher offering a door onto a directory that has
            // been deleted -- which points the CLI at nothing and reads as a
            // brand-new sign-in.
            let (a, b, c) = (account(A), account(B), account(&"a".repeat(32)));
            let mut s = state(vec![a.clone(), b.clone(), c.clone()], &a.id);
            assert_eq!(
                ids(s.switchable()),
                vec![b.id.clone(), c.id.clone()],
                "control: both of the others were offered before the removal"
            );

            assert!(s.forget(&b.id), "the removal reported that it did nothing");

            assert_eq!(ids(s.all()), vec![a.id.clone(), c.id.clone()]);
            assert_eq!(
                ids(s.switchable()),
                vec![c.id.clone()],
                "a removed account is still offered as a switch target"
            );
            assert_eq!(s.active().id, a.id, "the removal moved the active account");
        }

        #[test]
        fn a_state_refuses_to_forget_the_account_it_is_on() {
            // The invariant the type is built around: `active()` hands out an
            // `&Account`, so there has to BE one. Removing the active account
            // is done by settling onto the survivor first and adopting it --
            // by which point the account being deleted is not the active one.
            let (a, b) = (account(A), account(B));
            let mut s = state(vec![a.clone(), b.clone()], &a.id);

            assert!(!s.forget(&a.id), "the active account was forgotten");
            assert_eq!(ids(s.all()), vec![a.id.clone(), b.id.clone()]);
            assert_eq!(s.active().id, a.id);

            // Positive control on the same state and the same call: the OTHER
            // account really can be forgotten, so the refusal above is the
            // active-account rule rather than a `forget` that never removes
            // anything.
            assert!(s.forget(&b.id));
            assert_eq!(ids(s.all()), vec![a.id.clone()]);

            // And the last account is still the active one, so the list can
            // never be emptied by this call however many times it is made.
            assert!(!s.forget(&a.id));
            assert_eq!(ids(s.all()), vec![a.id]);
        }

        #[test]
        fn forgetting_an_account_this_state_never_held_changes_nothing_and_says_so() {
            // The second click on "Remove", or a removal retried after a
            // `settings.json` write failed. `true` here would persist a list
            // nobody changed.
            let (a, b) = (account(A), account(B));
            let mut s = state(vec![a.clone(), b.clone()], &a.id);
            let before = s.clone();

            assert!(!s.forget(&id(&"9".repeat(32))));
            assert_eq!(s, before);

            // Positive control: an id this state DOES hold is removed by the
            // same call, so the no-op above is about the id rather than about
            // a `forget` that is inert.
            assert!(s.forget(&b.id));
            assert_ne!(s, before);
        }
    }

    // ------------------------------------------------ every account secret

    /// The switch bug, and the shape that keeps it from coming back.
    ///
    /// An account switch used to delete `session.bin` by name and nothing
    /// else. These tests are written against [`AccountSecret::ALL`] rather
    /// than against a list of file names, so a fifth secret added later is
    /// covered by them the moment its variant exists -- which is the whole
    /// point of the enum.
    mod account_secrets {
        use super::*;

        /// A scratch config directory under `%TEMP%`. **Never** the real one:
        /// these tests delete files.
        struct Scratch(PathBuf);

        impl Scratch {
            fn new(tag: &str) -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "deskwarden-secrets-{tag}-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                Self(dir)
            }
            fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        /// Writes a recognisable byte into every file every secret occupies,
        /// so a clear can be checked file by file.
        fn write_every_secret(cfg: &Path, id: &AccountId) -> Vec<PathBuf> {
            ensure_account_dir(cfg, id).unwrap();
            let mut all = Vec::new();
            for secret in AccountSecret::ALL {
                for path in secret.paths_for(cfg, id) {
                    std::fs::write(&path, b"wrapped").unwrap();
                    all.push(path);
                }
            }
            all
        }

        #[test]
        fn all_lists_every_variant_exactly_once() {
            // Cheap, and it is the assertion that catches a variant added to
            // the enum but not to `ALL` -- the one mistake the exhaustive
            // `match` in `scope` cannot catch on its own.
            let mut seen = AccountSecret::ALL.to_vec();
            let count = seen.len();
            seen.dedup();
            assert_eq!(seen.len(), count, "ALL repeats a variant");
            assert_eq!(count, 4, "a variant was added or removed without ALL");
        }

        #[test]
        fn every_secret_lives_in_its_own_files_inside_the_account_directory() {
            let cfg = Path::new(r"C:\cfg");
            let a = id(A);
            let dir = data_dir_for(cfg, &a);
            let mut every: Vec<PathBuf> = Vec::new();
            for secret in AccountSecret::ALL {
                let paths = secret.paths_for(cfg, &a);
                assert!(!paths.is_empty(), "{secret:?} names no file");
                for path in &paths {
                    assert_eq!(
                        path.parent(),
                        Some(dir.as_path()),
                        "{secret:?} escapes {dir:?}"
                    );
                }
                every.extend(paths);
            }
            let count = every.len();
            every.sort();
            every.dedup();
            assert_eq!(every.len(), count, "two secrets share a file: {every:?}");
        }

        /// `paths_for` must be the same paths the named functions give, or a
        /// clear would delete a file nothing writes while the real one stays.
        #[test]
        fn paths_for_agrees_with_the_named_path_functions() {
            let cfg = Path::new(r"C:\cfg");
            let a = id(A);
            assert_eq!(
                AccountSecret::SessionToken.paths_for(cfg, &a),
                vec![session_path_for(cfg, &a)]
            );
            assert_eq!(
                AccountSecret::UserKey.paths_for(cfg, &a),
                vec![user_key_path_for(cfg, &a)]
            );
            assert_eq!(
                AccountSecret::HelloBlob.paths_for(cfg, &a),
                vec![hello_blob_path_for(cfg, &a)]
            );
        }

        /// The master key is classified with the session token and not with
        /// the user's opt-ins. Stated on its own because getting this one
        /// wrong is the bug this whole module exists for.
        #[test]
        fn the_master_key_is_a_sign_in_secret_and_so_is_the_session_token() {
            assert_eq!(AccountSecret::UserKey.scope(), SecretScope::ThisAppsSignIn);
            assert_eq!(
                AccountSecret::SessionToken.scope(),
                SecretScope::ThisAppsSignIn
            );
            // The other half, so the classification is discriminating rather
            // than "everything is a sign-in secret": a switch that withdrew
            // the user's Hello enrolment would be its own defect.
            assert_eq!(AccountSecret::HelloBlob.scope(), SecretScope::UserOptIn);
            assert_eq!(AccountSecret::VaultCopy.scope(), SecretScope::UserOptIn);
        }

        /// **The pin.** Every file of every secret exists; the clear runs;
        /// each file is asserted against its own variant's scope. A future
        /// third sign-in secret that `clear_sign_in_secrets` does not remove
        /// fails here without anybody editing this test.
        #[test]
        fn clearing_removes_exactly_the_sign_in_secrets_and_leaves_the_opt_ins() {
            let cfg = Scratch::new("clear");
            let a = AccountId::generate();
            write_every_secret(cfg.path(), &a);
            // Something in the directory that is not a secret at all: the CLI
            // profile. A clear that took the directory would sign the account
            // out of `bw` as well.
            let profile = data_dir_for(cfg.path(), &a).join("data.json");
            std::fs::write(&profile, b"the CLI profile").unwrap();

            clear_sign_in_secrets(cfg.path(), &a);

            for secret in AccountSecret::ALL {
                for path in secret.paths_for(cfg.path(), &a) {
                    match secret.scope() {
                        SecretScope::ThisAppsSignIn => assert!(
                            !path.exists(),
                            "{secret:?} survived the clear at {}",
                            path.display()
                        ),
                        SecretScope::UserOptIn => assert!(
                            path.exists(),
                            "{secret:?} was withdrawn at {}",
                            path.display()
                        ),
                    }
                }
            }
            assert!(profile.exists(), "the clear took the CLI profile with it");
            assert!(data_dir_for(cfg.path(), &a).is_dir());
        }

        /// The regression in one line: whatever else it does, a clear must
        /// take `userkey.bin`. A master key does not expire, so leaving it is
        /// leaving a key to a vault the app is no longer on.
        #[test]
        fn clearing_always_takes_the_stored_master_key() {
            let cfg = Scratch::new("key");
            let a = AccountId::generate();
            write_every_secret(cfg.path(), &a);

            clear_sign_in_secrets(cfg.path(), &a);

            assert!(!user_key_path_for(cfg.path(), &a).exists());
            assert!(!session_path_for(cfg.path(), &a).exists());
        }

        #[test]
        fn clearing_an_account_that_has_nothing_stored_is_quiet_and_repeatable() {
            let cfg = Scratch::new("empty");
            let a = AccountId::generate();
            ensure_account_dir(cfg.path(), &a).unwrap();
            clear_sign_in_secrets(cfg.path(), &a);
            clear_sign_in_secrets(cfg.path(), &a);
            assert!(data_dir_for(cfg.path(), &a).is_dir());

            // And for an account whose directory was never created either.
            clear_sign_in_secrets(cfg.path(), &AccountId::generate());
        }

        /// One account's clear may not reach into another's. The property the
        /// per-account layout exists for, asserted at the clear.
        #[test]
        fn clearing_one_account_leaves_every_other_accounts_secrets_alone() {
            let cfg = Scratch::new("neighbour");
            let (a, b) = (AccountId::generate(), AccountId::generate());
            write_every_secret(cfg.path(), &a);
            let bs = write_every_secret(cfg.path(), &b);

            clear_sign_in_secrets(cfg.path(), &a);

            assert!(!user_key_path_for(cfg.path(), &a).exists(), "control");
            for path in bs {
                assert!(path.exists(), "{} was taken by a's clear", path.display());
            }
        }

        /// Removing the account is the other end of the scale: *everything*
        /// goes, opt-ins included. Derived from `ALL` for the same reason.
        #[test]
        fn deleting_the_account_directory_takes_every_secret_of_every_scope() {
            let cfg = Scratch::new("delete");
            let a = AccountId::generate();
            let every = write_every_secret(cfg.path(), &a);

            delete_account_dir(cfg.path(), &a).expect("delete");

            for path in every {
                assert!(!path.exists(), "{} survived removal", path.display());
            }
            assert!(!data_dir_for(cfg.path(), &a).exists());
        }
    }
}

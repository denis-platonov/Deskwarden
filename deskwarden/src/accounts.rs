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
/// created when the account is". For a migrated account the copy created it;
/// for an account this app mints (a first install, where there was no profile
/// to migrate) nothing has, and without this call the very first `store.save`
/// fails with "the system cannot find the path specified" — logged, survivable,
/// and invisible except as a master-password prompt on every launch forever.
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
/// in, so there is nothing to record. Whoever completes the sign-in fills it
/// in by asking the CLI about **this** directory
/// ([`login_ui::check_bw_status_details_in`](crate::login_ui::check_bw_status_details_in)),
/// which is the only source that knows. Left empty, the account is a blank row
/// in the switcher that the user cannot tell from any other.
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
/// `settings.json`, the log, and *every other account's* migrated profile with
/// it. Refusing returns `Err` and deletes nothing.
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

/// What this launch is pointed at, once the migration has had its turn.
///
/// Exactly two shapes, because `main` has exactly two: an app pointed at an
/// account directory, and *today's* app pointed at whatever profile the CLI
/// would use by itself. The second is a fallback to existing behaviour, not a
/// second implementation of anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAccounts {
    /// The normal case: at least one account, one of them active.
    Ready {
        active: Account,
        accounts: Vec<Account>,
        /// Whether the resolution changed something `settings.json` has to be
        /// told about — a migrated or freshly minted account, or an active id
        /// that named nobody.
        needs_persist: bool,
    },
    /// Migration did not produce an account list. The app sets no
    /// `BITWARDENCLI_APPDATA_DIR` at all and reads `<config_dir>\session.bin`,
    /// and `reason` is what [`AccountsState::blocked_reason`] reports wherever
    /// a switch would have been.
    ///
    /// **What that leaves depends on whether a migration ever ran, and only
    /// one of the two is "today's app".** On a machine that never migrated,
    /// the CLI's own directory still holds the vault and this really is the
    /// pre-multi-account behaviour. On one that migrated and is now blocked
    /// — a `bitwarden-cli` directory that appeared beside `bw.exe` since, or
    /// an unreadable `settings.json` — that directory and
    /// `<config_dir>\session.bin` were both deleted by the migration, so what
    /// the user meets is a **signed-out** app asking for a master password.
    /// Nothing is lost either way (the vault is in `accounts/<id>/` and this
    /// state deletes nothing), but the second case is a sign-in, not a
    /// continuation, and saying otherwise sends whoever reads it looking for
    /// a directory that is not there.
    ///
    /// **This is the only state in which the app has no [`Account`] at all**,
    /// and it is a startup condition rather than an account variant — see this
    /// module's header on why `AccountLocation` does not exist.
    Unmigrated { reason: String },
}

/// The ids a stored account list claims — what
/// [`migration::resume_action`](crate::migration::resume_action) subtracts the
/// directories on disk from.
///
/// Here rather than spelled out at the call site for the reason
/// `nothing_offers_an_account_without_going_through_accounts_state` exists:
/// `settings.accounts.iter()` in `main.rs` is banned, because in every *other*
/// place it is the raw list bypassing [`AccountsState`]'s four refusals. This
/// one read is legitimate and cannot go through `AccountsState` — it runs
/// *before* the migration whose answer `AccountsState` is built from — so it
/// lives beside [`resolve_startup`], the other function that is given the same
/// raw list for the same reason.
pub fn claimed_ids(stored: &[Account]) -> Vec<AccountId> {
    stored.iter().map(|a| a.id.clone()).collect()
}

/// Which account this launch runs as, given what is stored and what the
/// migration just did.
///
/// Pure, and deliberately given the migration's *answer* rather than the
/// config directory: every effect belongs to the caller, so this can be driven
/// through every branch without a `%APPDATA%` anywhere near it.
///
/// The one decision worth spelling out is which migration states may mint an
/// account and which may not:
///
/// * [`Blocked`](crate::migration::MigrationState::Blocked) with nothing
///   stored means the user's vault is somewhere this launch did not put an
///   account on. Usually that is the pre-existing profile, still sitting where
///   it always was and untouched. It is **not** always: a resumed migration
///   whose re-verification failed after an earlier run had already verified
///   and deleted the source is `Blocked` too, and there the only copy is the
///   one `migration::rollback` kept at `accounts/<id>` — which the next launch
///   adopts, the marker having been cleared. Either way the answer is the
///   same, and it is the answer that matters here: inventing an account would
///   point the CLI at an **empty** directory, and the app would present as
///   signed out while the real vault sat a few directories away — the symptom
///   a user reports as "the update deleted my vault".
/// * [`Blocked`](crate::migration::MigrationState::Blocked) with accounts
///   already stored is the opposite case and takes the opposite answer:
///   migration ran on some earlier launch and a `bitwarden-cli` directory has
///   appeared beside `bw.exe` since. The app is still `Ready` and its state,
///   `session.bin` and Windows Hello label all name the stored active
///   account; [`AccountsState`] is what refuses the *switch*.
///
///   **Under `BlockedByPortableProfile` the CLI does not read the directory
///   this points at.** That is the whole of what the block means: a
///   `bitwarden-cli` directory beside `bw.exe` makes `bw` ignore
///   `BITWARDENCLI_APPDATA_DIR` and read the portable profile instead, so the
///   vault the user sees is that one while every name the app shows is
///   account X's. It is still the right answer — refusing to switch is what
///   keeps one profile from being served under several identities, and
///   nothing here deletes anything — but `Ready` here means "pointed at",
///   not "reading from", and the two are the same only when the block is one
///   of the other kinds.
/// * [`NothingToMigrate`](crate::migration::MigrationState::NothingToMigrate)
///   with nothing stored is a new machine, not a failure. It gets one account
///   directory to sign in to.
pub fn resolve_startup(
    stored: &[Account],
    stored_active: Option<&AccountId>,
    migration: &crate::migration::MigrationState,
) -> StartupAccounts {
    use crate::migration::MigrationState;

    let mut accounts = stored.to_vec();
    let mut needs_persist = false;
    // The account this resolution is *introducing*, which is the one that then
    // becomes active. `None` when nothing new arrived, in which case the
    // stored active account is resumed and nothing is rewritten.
    let mut introduced: Option<AccountId> = None;

    if let MigrationState::Completed { account, .. } = migration {
        // Only when it is not already there. `Completed` is reported by the
        // launch that migrated, and a resumed `VerifyAndFinish` can report it
        // again after the list was already written; appending twice would put
        // two entries on one directory, and re-activating would silently drag
        // the user off whichever account they had switched to.
        if account_for(&accounts, &account.id).is_none() {
            introduced = Some(account.id.clone());
            accounts.push(account.clone());
            needs_persist = true;
        }
    }

    if accounts.is_empty() {
        match migration {
            MigrationState::Blocked { reason } => {
                return StartupAccounts::Unmigrated {
                    reason: reason.clone(),
                };
            }
            // `Completed` cannot reach here — it pushed above — but it is
            // spelled out rather than caught by a wildcard so that a later
            // variant has to be thought about instead of silently minting an
            // account.
            MigrationState::NothingToMigrate | MigrationState::Completed { .. } => {
                let fresh = Account {
                    id: AccountId::generate(),
                    // Not known yet: nobody has signed in. Filled in by
                    // whoever completes a sign-in against this directory.
                    email: String::new(),
                    server_url: None,
                };
                introduced = Some(fresh.id.clone());
                accounts.push(fresh);
                needs_persist = true;
            }
        }
    }

    let active = introduced
        .as_ref()
        .or(stored_active)
        .and_then(|id| account_for(&accounts, id))
        // A stored active id naming no stored account is a hand-edited
        // `settings.json`, or a removal that crashed between the two writes.
        // Falling through to "no active account" would leave the app with no
        // directory to point the CLI at at all.
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
    }
}

// ------------------------------------------- may this process switch at all?

/// The one door for "may I offer another account, and which one am I on?".
///
/// Two *independent* reasons a switch may be unavailable, and every UI entry
/// point needs the same answer to both:
///
/// * [`MultiAccountAvailability`](crate::bw_path::MultiAccountAvailability) —
///   a `bitwarden-cli` directory beside `bw.exe` makes the CLI ignore
///   `BITWARDENCLI_APPDATA_DIR`, so every account would silently share one
///   profile; and "we do not know where the CLI is" is the same refusal,
///   because the trap cannot be ruled out.
/// * [`MigrationState`](crate::migration::MigrationState) — a migration that
///   was refused or could not be verified means the account directories do not
///   hold what the account list says they do.
///
/// They are combined here and nowhere else. A window that asked one of them
/// and not the other would offer a switch that shares a profile, or one into a
/// directory that was never populated — neither of which reports an error, so
/// neither is visible in an end state.
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
    hello_needs_reenrolment: bool,
}

impl AccountsState {
    /// `None` when `accounts` is empty, which is the *only* way this can fail.
    ///
    /// An account list with no accounts is not a state this app has: before
    /// migration produces one there is no `Account` at all, and that is a
    /// startup condition (`StartupAccounts::Unmigrated`, Task 11) rather than
    /// an `AccountsState` with nothing in it. Making that unrepresentable is
    /// what lets [`active`](Self::active) return `&Account` instead of an
    /// `Option` every caller would have to unwrap somewhere.
    ///
    /// `active` naming an id that is not in `accounts` falls back to the first
    /// configured account rather than being refused. `settings.json` is a
    /// user-editable file and a removal that crashed mid-write leaves exactly
    /// this state; the alternative — no active account — would leave the app
    /// with no directory to point the CLI at.
    pub fn new(
        availability: crate::bw_path::MultiAccountAvailability,
        migration: crate::migration::MigrationState,
        accounts: Vec<Account>,
        active: AccountId,
    ) -> Option<Self> {
        use crate::migration::MigrationState;

        let active = account_for(&accounts, &active)
            .or_else(|| accounts.first())?
            .clone();

        // The CLI's refusal outranks the migration's: with the trap present a
        // migration is refused *because of* it, and the availability
        // explanation is the one that names the directory the user can go and
        // remove.
        let blocked_reason = match (availability.explanation(), &migration) {
            (Some(why), _) => Some(why),
            (None, MigrationState::Blocked { reason }) => Some(reason.clone()),
            // `NothingToMigrate` is not "not yet migrated": it is what every
            // launch after the first one reports, because an existing account
            // list is itself the reason there is nothing to do. Treating it as
            // pending would refuse every switch from the second launch onward.
            (None, MigrationState::Completed { .. } | MigrationState::NothingToMigrate) => None,
        };

        let switchable = switch_targets(&accounts, &active, blocked_reason.is_some());

        let hello_needs_reenrolment = matches!(
            migration,
            MigrationState::Completed {
                hello_needs_reenrolment: true,
                ..
            }
        );

        Some(Self {
            accounts,
            active,
            switchable,
            blocked_reason,
            hello_needs_reenrolment,
        })
    }

    /// The same state, built from the one thing [`new`](Self::new) distils its
    /// two inputs down to — for the tests of a window that is **banned from
    /// naming those inputs**.
    ///
    /// `no_window_answers_may_i_switch_for_itself` forbids
    /// `vault_window/mod.rs` from containing the strings
    /// `MultiAccountAvailability` or `MigrationState` anywhere at all, tests
    /// included, and that ban is the point of this type. But the account
    /// switcher in that window still has to be handed a blocked state and an
    /// available one by its own tests, and a hand-built struct literal there
    /// would be a second `AccountsState` with its own idea of what
    /// `switchable` means.
    ///
    /// So this takes exactly the `Option<String>` that
    /// [`new`](Self::new)'s `match` produces and computes everything else the
    /// same way it does — including `switchable`, through the one
    /// [`switch_targets`], rather than filtering the list a second time.
    /// `the_test_constructor_agrees_with_the_real_one` pins the two together.
    ///
    /// `hello_needs_reenrolment` is `false`: it is a fact about the migration
    /// and no switcher reads it.
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
            hello_needs_reenrolment: false,
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

    /// The accounts a user may switch **to** right now. Empty when multi-account
    /// is blocked or the migration is, whatever [`all`](Self::all) holds.
    pub fn switchable(&self) -> &[Account] {
        &self.switchable
    }

    /// Whether another account may be added. The same two blocks: an account
    /// added now would share the one profile, or land beside a migration that
    /// has not run.
    pub fn can_add(&self) -> bool {
        self.blocked_reason.is_none()
    }

    /// Why switching and adding are unavailable, in the words the user can act
    /// on, or `None` when they are available.
    pub fn blocked_reason(&self) -> Option<&str> {
        self.blocked_reason.as_deref()
    }

    /// Whether the migration deleted a Windows Hello enrolment that has to be
    /// set up again. Carried through here because the panel that shows it
    /// reads this state and not the migration's own return value.
    pub fn hello_needs_reenrolment(&self) -> bool {
        self.hello_needs_reenrolment
    }

    /// Records the account this process has just moved onto, appending it to
    /// the list if it is not there yet.
    ///
    /// This is how an *added* account reaches the state every window reads.
    /// It is a mutation rather than a rebuild because [`new`](Self::new)'s two
    /// inputs — the CLI's availability and the migration's outcome — are not
    /// re-derivable here, and re-deriving them would be a second reading of
    /// the two facts this type exists to be the single answer to. Neither is
    /// changed by adding an account: whether switching is blocked was settled
    /// at startup, so it is *carried*, and a blocked state that adopts one
    /// still offers no switch targets.
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

    // --------------------------------------------------------------- 10

    mod accounts_state {
        use super::*;
        use crate::bw_path::MultiAccountAvailability;
        use crate::migration::MigrationState;

        fn a() -> Account {
            account(A)
        }

        fn b() -> Account {
            account(B)
        }

        fn c() -> Account {
            account(&"a".repeat(32))
        }

        fn completed(of: &Account) -> MigrationState {
            MigrationState::Completed {
                account: of.clone(),
                hello_needs_reenrolment: false,
            }
        }

        fn trap() -> MultiAccountAvailability {
            MultiAccountAvailability::BlockedByPortableProfile {
                relative_data_dir: PathBuf::from(r"C:\a\bin\bitwarden-cli"),
            }
        }

        fn state(
            availability: MultiAccountAvailability,
            migration: MigrationState,
            accounts: Vec<Account>,
            active: &AccountId,
        ) -> AccountsState {
            AccountsState::new(availability, migration, accounts, active.clone())
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
            let real_open = state(
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![a(), b()],
                &a().id,
            );
            let open = AccountsState::from_blocked_reason(vec![a(), b()], a().id, None)
                .expect("these accounts are not empty");
            assert_eq!(open, real_open);
            assert_eq!(switch_ids(&open), vec![b().id], "control: it offers b");

            let real_blocked = state(trap(), completed(&a()), vec![a(), b()], &a().id);
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
            let s = state(trap(), completed(&a()), vec![a(), b()], &a().id);
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
                completed(&a()),
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
        fn a_blocked_migration_offers_no_switch_targets_even_when_the_cli_is_fine() {
            // The second, independent reason. A half-migrated or unmigrated
            // profile means the account directories do not hold what the list
            // says they do.
            let s = state(
                MultiAccountAvailability::Available,
                MigrationState::Blocked {
                    reason: "the copy could not be verified".into(),
                },
                vec![a(), b()],
                &a().id,
            );
            assert!(s.switchable().is_empty());
            assert!(!s.can_add());
            assert!(
                s.blocked_reason()
                    .is_some_and(|why| why.contains("could not be verified")),
                "the migration's own reason must reach the user, got {:?}",
                s.blocked_reason()
            );
        }

        #[test]
        fn an_available_migrated_state_offers_every_account_except_the_active_one() {
            // The positive control for both tests above, and the rule in its
            // own right: "switch to the account you are already on" is a no-op
            // that would still tear the backend down and demand a master
            // password.
            let s = state(
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![a(), b(), c()],
                &b().id,
            );
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
        fn a_later_launch_with_nothing_to_migrate_still_offers_a_switch() {
            // `NothingToMigrate` is what `migrate` returns on EVERY launch
            // after the first: `resume_action` answers `DoNothing` as soon as
            // `accounts_already_configured` is true. Reading "migration has not
            // completed" as "the state is not `Completed`" would therefore
            // refuse every switch from the second launch onward -- a feature
            // that works exactly once, on the launch that migrated, and is
            // inert forever after. No end state distinguishes that from a
            // correctly blocked app.
            let s = state(
                MultiAccountAvailability::Available,
                MigrationState::NothingToMigrate,
                vec![a(), b()],
                &a().id,
            );
            assert_eq!(switch_ids(&s), vec![b().id]);
            assert!(s.can_add());
            assert_eq!(s.blocked_reason(), None);
        }

        #[test]
        fn the_hello_notice_survives_into_the_state_the_login_window_reads() {
            // WIRING for Task 7's panel line: a `hello_needs_reenrolment` that
            // the migration computes and nothing carries forward is a notice
            // the user never sees, and quick unlock silently stops working.
            let loud = state(
                MultiAccountAvailability::Available,
                MigrationState::Completed {
                    account: a(),
                    hello_needs_reenrolment: true,
                },
                vec![a()],
                &a().id,
            );
            assert!(loud.hello_needs_reenrolment());
            let quiet = state(
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![a()],
                &a().id,
            );
            assert!(
                !quiet.hello_needs_reenrolment(),
                "a state that always says yes would put the notice in front of every user"
            );
            // And it is not a synonym for either block: a blocked state with
            // the flag set still reports it, and an unblocked one without it
            // still does not.
            let blocked_and_loud = state(
                trap(),
                MigrationState::Completed {
                    account: a(),
                    hello_needs_reenrolment: true,
                },
                vec![a()],
                &a().id,
            );
            assert!(blocked_and_loud.hello_needs_reenrolment());
            let unmigrated = state(
                MultiAccountAvailability::Available,
                MigrationState::NothingToMigrate,
                vec![a()],
                &a().id,
            );
            assert!(!unmigrated.hello_needs_reenrolment());
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
            let s = state(
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![b(), a()],
                &ghost,
            );
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
            // The pre-migration app has no `Account`, and that is a startup
            // condition rather than an `AccountsState` holding nothing. If this
            // constructed, `active()` would have to invent an account or panic
            // -- and an invented one points the CLI at an empty directory,
            // which presents as "signed out" with the real vault untouched a
            // few directories away.
            for availability in [
                MultiAccountAvailability::Available,
                MultiAccountAvailability::BlockedByUnknownCliPath,
                trap(),
            ] {
                for migration in [
                    MigrationState::NothingToMigrate,
                    completed(&a()),
                    MigrationState::Blocked {
                        reason: "nope".into(),
                    },
                ] {
                    assert!(
                        AccountsState::new(
                            availability.clone(),
                            migration.clone(),
                            vec![],
                            a().id
                        )
                        .is_none(),
                        "{availability:?} + {migration:?} built a state with no accounts"
                    );
                    // The positive control on the same call: one account and
                    // the same two inputs does build.
                    assert!(
                        AccountsState::new(
                            availability.clone(),
                            migration.clone(),
                            vec![a()],
                            a().id
                        )
                        .is_some(),
                        "{availability:?} + {migration:?} refused a perfectly good single account"
                    );
                }
            }
        }

        #[test]
        fn a_single_account_has_nowhere_to_switch_to_but_may_still_be_added_to() {
            let s = state(
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![a()],
                &a().id,
            );
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
                MultiAccountAvailability::Available,
                completed(&a()),
                vec![a(), b(), b(), a()],
                &a().id,
            );
            assert_eq!(switch_ids(&s), vec![b().id]);
            assert_eq!(s.all().len(), 4, "what is stored is still reported as stored");
        }

        #[test]
        fn every_combination_of_the_two_blocks_obeys_one_rule() {
            // The whole decision table, so no single combination can be
            // special-cased into working while another silently is not. Three
            // availabilities x three migration states x four account lists,
            // each with an active account that is in the list and one that is
            // not.
            let unblocked_migrations = [MigrationState::NothingToMigrate, completed(&a())];
            let availabilities = [
                (MultiAccountAvailability::Available, true),
                (MultiAccountAvailability::BlockedByUnknownCliPath, false),
                (trap(), false),
            ];
            let migrations = [
                (MigrationState::NothingToMigrate, true),
                (completed(&a()), true),
                (
                    MigrationState::Blocked {
                        reason: "the copy could not be verified".into(),
                    },
                    false,
                ),
            ];
            let lists = [
                vec![a()],
                vec![a(), b()],
                vec![a(), b(), c()],
                vec![b(), c()], // the active id names none of these
            ];

            let mut ever_offered = 0usize;
            for (availability, cli_ok) in &availabilities {
                for (migration, migration_ok) in &migrations {
                    for list in &lists {
                        let s = state(
                            availability.clone(),
                            migration.clone(),
                            list.clone(),
                            &a().id,
                        );
                        let allowed = *cli_ok && *migration_ok;
                        let label = format!("{availability:?} / {migration:?} / {list:?}");

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
            }
            // The positive control over the whole table: "refused" is not the
            // answer everywhere, which is what a `switchable()` that returned
            // an empty slice unconditionally would look like.
            assert!(
                ever_offered > 0,
                "no combination in the whole table offered a single switch target"
            );
            // And the unblocked corner really does depend on the account list
            // rather than on the state alone.
            for migration in unblocked_migrations {
                let one = state(
                    MultiAccountAvailability::Available,
                    migration.clone(),
                    vec![a()],
                    &a().id,
                );
                let two = state(
                    MultiAccountAvailability::Available,
                    migration,
                    vec![a(), b()],
                    &a().id,
                );
                assert!(one.switchable().is_empty());
                assert_eq!(switch_ids(&two), vec![b().id]);
            }
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

        /// The files that must ask [`AccountsState`] rather than answer for
        /// themselves. `main.rs` is deliberately not among them: it is where
        /// the two inputs are produced (Task 11), so it is the one place that
        /// legitimately names both.
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
            // A gate nothing asks is the same as no gate. There is no
            // production caller yet -- Task 11 wires the first one -- so what
            // can be pinned today is the other half: that the two facts are
            // combined HERE and are not reachable anywhere a second, weaker
            // combination could grow. A window that asked
            // `multi_account_availability()` and not the migration would offer
            // a switch into a directory that was never populated; one that
            // asked the migration and not the CLI would offer a switch that
            // shares a profile. Both are silent.
            //
            // NEEDLES SPLIT ACROSS `concat!` ARGUMENTS, DELIBERATELY: written
            // whole, each would match its own declaration in this file, and
            // the positive control below would pass against an empty scan.
            // Single-line for the same reason every other guard here is: this
            // module is LF and the files it reads are not necessarily.
            let banned = [
                concat!("MultiAccount", "Availability"),
                concat!("multi_account_", "availability"),
                concat!("Migration", "State"),
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
                         that combines those two facts a second time will disagree with this \
                         one, and the disagreement is silent -- a switch that shares a profile, \
                         or one into a directory the migration never populated."
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
        use crate::migration::MigrationState;

        fn completed(account: &Account) -> MigrationState {
            MigrationState::Completed {
                account: account.clone(),
                hello_needs_reenrolment: false,
            }
        }

        #[test]
        fn a_completed_migration_on_this_launch_becomes_the_active_account() {
            let migrated = account(A);
            let r = resolve_startup(
                &[],
                None,
                &MigrationState::Completed {
                    account: migrated.clone(),
                    hello_needs_reenrolment: true,
                },
            );
            let StartupAccounts::Ready {
                active,
                accounts,
                needs_persist,
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(active.id, migrated.id);
            assert_eq!(accounts.len(), 1);
            assert!(
                needs_persist,
                "the migrated account must be written before the next launch"
            );
        }

        #[test]
        fn the_stored_active_account_is_resumed_on_a_later_launch() {
            let (a, b) = (account(A), account(B));
            let r = resolve_startup(&[a.clone(), b.clone()], Some(&b.id), &completed(&a));
            let StartupAccounts::Ready {
                active,
                accounts,
                needs_persist,
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(
                active.id, b.id,
                "a restart must resume the account that was last active"
            );
            assert_eq!(accounts.len(), 2, "and must not drop the others");
            assert!(!needs_persist, "nothing changed, so nothing is rewritten");
        }

        #[test]
        fn an_active_id_naming_no_stored_account_falls_back_to_the_first() {
            // A hand-edited settings.json, or an account removed by a build
            // that crashed mid-write. Falling through to "no active account"
            // would leave the app with nothing to point the CLI at.
            let a = account(A);
            let ghost = AccountId::parse(&"9".repeat(32)).unwrap();
            let r = resolve_startup(&[a.clone()], Some(&ghost), &completed(&a));
            let StartupAccounts::Ready {
                active,
                needs_persist,
                ..
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(active.id, a.id);
            assert!(
                needs_persist,
                "the dangling active id must be corrected on disk"
            );
        }

        #[test]
        fn a_blocked_migration_leaves_the_app_unmigrated_rather_than_inventing_an_account() {
            // The failure path that keeps the app working. Inventing an
            // `Account` here would point the CLI at an EMPTY directory and
            // present as "signed out", while the real profile sat untouched a
            // few directories away -- the exact symptom a user would report as
            // "the update deleted my vault".
            let r = resolve_startup(
                &[],
                None,
                &MigrationState::Blocked {
                    reason: "the copy could not be verified".into(),
                },
            );
            let StartupAccounts::Unmigrated { reason } = r else {
                panic!("{r:?}")
            };
            assert!(reason.contains("could not be verified"));
        }

        #[test]
        fn a_first_install_gets_one_fresh_account_rather_than_running_unmigrated() {
            // `NothingToMigrate` is not a failure: there was no profile
            // because this is a new machine. Give it an account directory and
            // let the user sign in there.
            let r = resolve_startup(&[], None, &MigrationState::NothingToMigrate);
            let StartupAccounts::Ready {
                active,
                accounts,
                needs_persist,
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(accounts.len(), 1);
            assert!(needs_persist);
            assert_eq!(
                accounts[0].id, active.id,
                "the minted account is the one that becomes active"
            );
        }

        #[test]
        fn a_block_that_appears_after_the_migration_still_resumes_the_migrated_account() {
            // Not in the plan, and the inverse of the test above it. A
            // `bitwarden-cli` directory appearing beside `bw.exe` AFTER the
            // profile was migrated blocks the migration report on every later
            // launch -- and reading that as `Unmigrated` would point the CLI
            // at its own default profile while the vault sits in
            // `accounts/<id>/`. That is the same "signed out on upgrade"
            // symptom the empty-list case exists to avoid, arrived at from the
            // opposite direction: refusing to migrate is safe, refusing to
            // RESUME is not.
            let a = account(A);
            let r = resolve_startup(
                &[a.clone()],
                Some(&a.id),
                &MigrationState::Blocked {
                    reason: "a bitwarden-cli directory sits beside bw.exe".into(),
                },
            );
            let StartupAccounts::Ready {
                active,
                needs_persist,
                ..
            } = r
            else {
                panic!("a migrated account was dropped when a later launch was blocked: {r:?}")
            };
            assert_eq!(active.id, a.id);
            assert!(!needs_persist);
        }

        #[test]
        fn a_completed_migration_reported_twice_neither_duplicates_nor_re_activates() {
            // A crash between deleting the source and clearing the marker
            // leaves `VerifyAndFinish`, so a LATER launch can report
            // `Completed` for an account that is already stored -- and by then
            // the user may have added and switched to a second one. Appending
            // again would put two menu entries on one directory; re-activating
            // would silently drag them back.
            let (a, b) = (account(A), account(B));
            let r = resolve_startup(&[a.clone(), b.clone()], Some(&b.id), &completed(&a));
            let StartupAccounts::Ready {
                active,
                accounts,
                needs_persist,
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(accounts.len(), 2, "the migrated account was added twice");
            assert_eq!(active.id, b.id, "the user was dragged back off their pick");
            assert!(!needs_persist);
            // Positive control on the same call shape: an account the list
            // does NOT hold really is added and really does become active.
            let fresh = account(&"c".repeat(32));
            let r = resolve_startup(&[a.clone(), b.clone()], Some(&b.id), &completed(&fresh));
            let StartupAccounts::Ready {
                active, accounts, ..
            } = r
            else {
                panic!("{r:?}")
            };
            assert_eq!(accounts.len(), 3);
            assert_eq!(active.id, fresh.id);
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
                std::fs::create_dir_all(data_dir_for(cfg.path(), id).join("bw")).unwrap();
            }

            discard_prepared_account(cfg.path(), &doomed.id);

            assert!(!data_dir_for(cfg.path(), &doomed.id).exists());
            assert!(!session_path_for(cfg.path(), &doomed.id).exists());
            assert!(!hello_blob_path_for(cfg.path(), &doomed.id).exists());
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
        use crate::migration::MigrationState;

        fn state(
            availability: MultiAccountAvailability,
            list: Vec<Account>,
            active: &AccountId,
        ) -> AccountsState {
            AccountsState::new(
                availability,
                MigrationState::NothingToMigrate,
                list,
                active.clone(),
            )
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
                MultiAccountAvailability::Available,
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
        use crate::migration::MigrationState;

        fn state(list: Vec<Account>, active: &AccountId) -> AccountsState {
            AccountsState::new(
                MultiAccountAvailability::Available,
                MigrationState::NothingToMigrate,
                list,
                active.clone(),
            )
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
}

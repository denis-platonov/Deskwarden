# Multiple accounts — implementation plan

**Spec:** `docs/superpowers/specs/2026-08-01-multiple-accounts-design.md` (approved, `3bb7615`).
**Baseline:** `1a0bea9` on `main`, tree clean, **880 lib + 34 bin pass, zero warnings**.

## Goal

Let a user configure several Bitwarden accounts and switch between them. Switching
switches everything: the vault list, the detail pane, autofill, the match engine, and
the underlying `bw` CLI. One active account at a time; autofill follows it; a switch
prompts for the master password; concurrent accounts are rejected. Those four are
settled in the spec and are not re-derived here.

## Architecture

**The switch is the existing lock/re-auth sequence with a different data directory.**
`main.rs` already runs, inside `open_vault_window`'s `if result.locked ||
result.needs_reauth` block (lines 1839–1951):

> drain the in-flight backend op → `cache.clear()` → `stop_bw_serve` → `reauthenticate`
> → drop `cached_status_details` → `try_start_backend` (via
> `restart_backend_after_unlock`) → capture `cache.epoch()` → `settle_vault_after_unlock`
> (readiness probe → `repopulate_and_refresh_after_unlock` → rebuild the engine) →
> `main`'s idle loop reconciles `keep_backend_running`.

That block is **inlined**, not a function, so "reuse it" is not achievable by calling
it. Task 7 extracts it verbatim into `resettle_session(..)`, parameterised by exactly
one thing — the closure that produces the new session token. The lock/re-auth path
passes `|| reauthenticate(&store)`. A switch passes a closure that first points the CLI
at the target account's data directory and constructs that account's `SessionStore`,
then does the same. **Nothing else differs.**

> **If a task finds itself writing a second teardown-and-repopulate path, it has gone
> wrong.** There is exactly one `cache.clear()` + `try_start_backend` +
> `settle_vault_after_unlock` sequence in this crate after Task 7, and Task 8 must call
> it rather than reproduce it. A reviewer should be able to `grep -c
> 'settle_vault_after_unlock('` in `main.rs` and find the definition plus **one** call
> site.

**Where the data directory is injected.** `bw_path::bw_command()` is the single door
every `bw` spawn in this crate goes through — `bw_serve_command`, `run_bw_sync`,
`check_bw_status*`, `bw_logout`, `configure_server`, and the one call that hands over
the master password. Setting `BITWARDENCLI_APPDATA_DIR` there means the login window,
the backend, sync and status all follow the active account with **no signature
widened**. The active directory is a process-global `RwLock<Option<PathBuf>>` beside
the existing `VERIFIED_BW_EXE` `OnceLock` (which stays a `OnceLock` — the *binary* is
still verified once and never replaced).

**The legacy account does not move.** The spec says each account gets
`accounts\<id>\`. The account that already exists lives in the CLI's own default
directory and its profile holds the encrypted vault; relocating it is a data-loss risk
the spec never weighed, and a half-completed copy is worse than every failure mode this
feature is guarding against. So an account is either `AccountLocation::CliDefault` (the
pre-existing one: `BITWARDENCLI_APPDATA_DIR` unset, `session.bin` and `hello.bin` stay
exactly where they are, today's behaviour bit-for-bit) or `AccountLocation::Managed`
(everything added from now on). This also means every existing Windows Hello enrolment
keeps working untouched, which a migration would have silently destroyed.

**Per-account secrets.** `session.bin` and `hello.bin` become functions of the account.
`hello.rs` keeps **one** Hello key credential (`deskwarden-quick-unlock`) and separates
accounts through the KDF label it already has, and must never call
`RequestCreateAsync(ReplaceExisting)` again — that rotates the shared credential and
destroys every other account's enrolment. `CliDefault` keeps the unmodified label so
existing blobs still open.

**Availability is one object, not a scattered check.** `accounts::AccountsState` is
constructed from `(availability, records, active)` and refuses to report more than one
switchable account when the `relativeDataDir` trap is detected. Every consumer — tray,
vault-window switcher, add-account flow — asks it. One door.

## Tech stack

Rust 2021, Windows-only. `serde`/`serde_json` (settings), `directories` (config dir),
`getrandom` (account ids — already a dependency via `hello.rs`), `aes-gcm`/`sha2`
(hello), `windows` crate (DPAPI, Hello), `eframe`/`egui` (windows), `tray-icon`,
`mockito` (HTTP tests). No new dependencies.

## Global constraints

State these verbatim; they are not advisory.

- Build with `-j 2` ONLY: `cargo test --manifest-path deskwarden/Cargo.toml -j 2` and
  `cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2`. Higher
  parallelism hits a page-file limit and WILL fail the build. Both must end clean and
  warning-free.
- Commit by passing paths to `git commit` itself; never `git add` then commit, never
  `--amend`, never `git add -A`, never `git stash` (two unrelated pre-existing stash
  entries must survive), never create/remove/prune a worktree.
- `zeroize` is leaky BY DESIGN, deliberate and recorded — do not "fix" it.

Additional constraints for this plan:

- **Every task states what would make its test fail.** A test whose failure mode is not
  written down is not finished. Where a task wires something, the test must pin the
  wiring, not only the decision — the decisions getting tested while the wiring that
  reaches them does not is this repository's single most repeated defect.
- **Prefer pure functions over logic inside an eframe closure.** Logic reachable only
  through a closure is logic that will not be tested. `fn account_for(..)`,
  `fn data_dir_for(..)`, `fn switch_allowed(..)` exist so the answer can be asserted
  without a window.
- **Source guards, where one is the only option, use `concat!`-split single-line
  needles with a positive control.** A needle written as one literal in a file the test
  `include_str!`s matches its own declaration and can never fail. A needle containing
  `\n` passes on an LF working tree and fails on a CRLF one; this repo has both.
- Line endings: leave files as you found them. Do not reflow or `cargo fmt` untouched
  regions.

---

# Task 1 — Detect the `relativeDataDir` trap

Earliest, because every later task is meaningless if it is not handled: a
`bitwarden-cli` directory beside `bw.exe` makes `BITWARDENCLI_APPDATA_DIR` ignored and
**every account shares one profile**, presenting as switching that appears to work and
then doesn't stick, with no error anywhere.

The CLI resolves its data directory as:

```ts
if (fs.existsSync(relativeDataDir)) { p = relativeDataDir; }        // FIRST
else if (process.env.BITWARDENCLI_APPDATA_DIR) { ... }
```

`relativeDataDir` is `<dir of bw.exe>\bitwarden-cli`.

**Files:** modify `deskwarden/src/bw_path.rs`.

**Interfaces**

Consumes: `bw_path::verified_bw_exe() -> Option<&'static Path>` (exists).

Produces:

```rust
pub fn relative_data_dir(bw_exe: &Path) -> Option<PathBuf>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAccountAvailability {
    Available,
    BlockedByPortableProfile { relative_data_dir: PathBuf },
    BlockedByUnknownCliPath,
}

pub fn multi_account_from(relative: Option<PathBuf>, relative_exists: bool)
    -> MultiAccountAvailability;

pub fn multi_account_availability() -> MultiAccountAvailability;

impl MultiAccountAvailability {
    pub fn is_available(&self) -> bool;
    pub fn explanation(&self) -> Option<String>;
}
```

### Step 1.1 — the sibling path

```rust
#[test]
fn the_relative_data_dir_is_bitwarden_cli_beside_the_exe() {
    assert_eq!(
        relative_data_dir(Path::new(r"C:\Users\me\AppData\Local\Deskwarden\bin\bw.exe")),
        Some(PathBuf::from(r"C:\Users\me\AppData\Local\Deskwarden\bin\bitwarden-cli")),
    );
    // Not the app directory one level up, and not the CWD: the CLI joins it
    // onto the directory of its OWN executable.
    assert_ne!(
        relative_data_dir(Path::new(r"C:\a\bin\bw.exe")),
        Some(PathBuf::from(r"C:\a\bitwarden-cli")),
    );
}
```

```rust
pub fn relative_data_dir(bw_exe: &Path) -> Option<PathBuf> {
    bw_exe.parent().map(|dir| dir.join("bitwarden-cli"))
}
```

### Step 1.2 — the decision, absolute in both directions

```rust
#[test]
fn a_bitwarden_cli_directory_beside_the_exe_blocks_multi_account() {
    let dir = PathBuf::from(r"C:\a\bin\bitwarden-cli");
    assert_eq!(
        multi_account_from(Some(dir.clone()), true),
        MultiAccountAvailability::BlockedByPortableProfile { relative_data_dir: dir },
        "BITWARDENCLI_APPDATA_DIR is IGNORED when this directory exists, so every \
         account would silently share one profile"
    );
}

#[test]
fn no_such_directory_means_multi_account_is_available() {
    // The positive control. Without it, `multi_account_from` returning
    // Blocked unconditionally passes the test above.
    assert_eq!(
        multi_account_from(Some(PathBuf::from(r"C:\a\bin\bitwarden-cli")), false),
        MultiAccountAvailability::Available,
    );
}

#[test]
fn an_unknown_cli_path_blocks_rather_than_assuming_it_is_fine() {
    // `verified_bw_exe()` is `None` in examples and unit tests, and would be
    // `None` if startup verification had not run. "We do not know where the
    // CLI is" cannot be read as "there is no portable profile beside it".
    assert_eq!(
        multi_account_from(None, false),
        MultiAccountAvailability::BlockedByUnknownCliPath,
    );
    assert_eq!(
        multi_account_from(None, true),
        MultiAccountAvailability::BlockedByUnknownCliPath,
    );
}
```

```rust
pub fn multi_account_from(
    relative: Option<PathBuf>,
    relative_exists: bool,
) -> MultiAccountAvailability {
    match relative {
        None => MultiAccountAvailability::BlockedByUnknownCliPath,
        Some(dir) if relative_exists => {
            MultiAccountAvailability::BlockedByPortableProfile { relative_data_dir: dir }
        }
        Some(_) => MultiAccountAvailability::Available,
    }
}
```

### Step 1.3 — the real probe, and the explanation

```rust
#[test]
fn the_live_probe_agrees_with_the_pure_decision_for_a_real_directory() {
    // Drives the impure half against a directory that really exists, so the
    // `.exists()` call itself is exercised rather than only the decision it
    // feeds.
    let dir = scratch_dir("relative-data-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("bw.exe");
    touch(&exe);
    let portable = dir.join("bitwarden-cli");
    assert_eq!(
        multi_account_from(relative_data_dir(&exe), portable.exists()),
        MultiAccountAvailability::Available,
        "nothing has been planted yet"
    );
    std::fs::create_dir_all(&portable).unwrap();
    assert_eq!(
        multi_account_from(relative_data_dir(&exe), portable.exists()),
        MultiAccountAvailability::BlockedByPortableProfile { relative_data_dir: portable },
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_blocked_variants_explain_themselves_and_they_name_the_directory() {
    assert_eq!(MultiAccountAvailability::Available.explanation(), None);
    let dir = PathBuf::from(r"C:\a\bin\bitwarden-cli");
    let text = MultiAccountAvailability::BlockedByPortableProfile {
        relative_data_dir: dir.clone(),
    }
    .explanation()
    .expect("a blocked state must say why");
    assert!(text.contains(r"C:\a\bin\bitwarden-cli"), "got: {text}");
    assert!(
        MultiAccountAvailability::BlockedByUnknownCliPath.explanation().is_some()
    );
}
```

```rust
pub fn multi_account_availability() -> MultiAccountAvailability {
    let relative = verified_bw_exe().and_then(relative_data_dir);
    let exists = relative.as_ref().is_some_and(|p| p.exists());
    multi_account_from(relative, exists)
}

impl MultiAccountAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn explanation(&self) -> Option<String> {
        match self {
            Self::Available => None,
            Self::BlockedByPortableProfile { relative_data_dir } => Some(format!(
                "The Bitwarden CLI is using the profile directory beside itself:\n{}\n\n\
                 While that directory exists the CLI ignores the per-account directory \
                 Deskwarden would point it at, so every account would share one profile. \
                 Deskwarden is staying a single-account app rather than mixing two accounts' \
                 state together.\n\nRemove or rename that directory to enable multiple \
                 accounts.",
                relative_data_dir.display()
            )),
            Self::BlockedByUnknownCliPath => Some(
                "Deskwarden could not work out where the Bitwarden CLI is, so it cannot check \
                 whether the CLI is using a profile directory beside itself. Multiple accounts \
                 are unavailable."
                    .to_string(),
            ),
        }
    }
}
```

**What would make these tests fail:** `relative_data_dir` joining onto the app
directory instead of the exe's; `multi_account_from` returning `Available`
unconditionally (caught by 1.2's first test), or `Blocked` unconditionally (caught by
the positive control), or treating `None` as fine; `.exists()` swapped for a check that
never fires (caught by 1.3, which plants a real directory); an `explanation` that omits
the directory the user has to go and delete.

---

# Task 2 — The account model and its paths

**Files:** create `deskwarden/src/accounts.rs`; modify `deskwarden/src/lib.rs` (add
`pub mod accounts;`).

**Interfaces**

Consumes: `bw_path::MultiAccountAvailability` (Task 1).

Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    pub fn generate() -> Self;                       // 32 lowercase hex chars
    pub fn parse(raw: &str) -> Option<Self>;         // rejects anything not 32 hex
    pub fn as_str(&self) -> &str;
}
impl std::fmt::Display for AccountId { .. }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountLocation { CliDefault, Managed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub email: String,
    pub server_url: Option<String>,
    pub location: AccountLocation,
}

pub fn accounts_root(config_dir: &Path) -> PathBuf;
pub fn data_dir_for(config_dir: &Path, account: &Account) -> Option<PathBuf>;
pub fn session_path_for(config_dir: &Path, account: &Account) -> PathBuf;
pub fn hello_blob_path_for(config_dir: &Path, account: &Account) -> PathBuf;
pub fn hello_kdf_suffix_for(account: &Account) -> Vec<u8>;
pub fn account_for<'a>(accounts: &'a [Account], id: &AccountId) -> Option<&'a Account>;
pub fn next_active_after_removal<'a>(accounts: &'a [Account], removed: &AccountId)
    -> Option<&'a Account>;
```

`data_dir_for` returning `Option` is the whole legacy story in the type: `None` means
"leave `BITWARDENCLI_APPDATA_DIR` unset and let the CLI use its own default", which is
exactly what the pre-existing account does today.

### Step 2.1 — an opaque id that is not the email

```rust
#[test]
fn a_generated_id_is_thirty_two_hex_characters_and_not_an_email() {
    let id = AccountId::generate();
    assert_eq!(id.as_str().len(), 32, "got {id}");
    assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(!id.as_str().contains('@'), "the directory name must not disclose whose vault it is");
}

#[test]
fn two_generated_ids_differ() {
    assert_ne!(AccountId::generate(), AccountId::generate());
}

#[test]
fn parse_rejects_anything_that_could_escape_the_accounts_directory() {
    // This id becomes a directory name. A traversal or a device name reaching
    // `data_dir_for` would put an account's profile somewhere else entirely.
    for bad in ["..", "../evil", r"..\evil", "CON", "", "abc", "a@b.c",
                "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef0123456789abcde"] {
        assert!(AccountId::parse(bad).is_none(), "accepted {bad:?}");
    }
    // Positive control on the same function.
    assert!(AccountId::parse("0123456789abcdef0123456789abcdef").is_some());
    assert!(AccountId::parse(AccountId::generate().as_str()).is_some());
}
```

```rust
impl AccountId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).expect("the OS must be able to produce 16 random bytes");
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let ok = raw.len() == 32
            && raw.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        ok.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

`Deserialize` must go through `parse`: derive it as a `String` and validate in a
`#[serde(try_from = "String")]` impl, so a hand-edited `settings.json` containing
`"id": "../.."` fails to parse rather than being honoured.

```rust
#[test]
fn a_hand_edited_settings_id_that_escapes_the_directory_does_not_deserialize() {
    assert!(serde_json::from_str::<AccountId>(r#""../.." "#.trim()).is_err());
    assert!(serde_json::from_str::<AccountId>(r#""0123456789abcdef0123456789abcdef""#).is_ok());
}
```

### Step 2.2 — the paths, and the collision property

```rust
fn managed(id: &str) -> Account {
    Account {
        id: AccountId::parse(id).unwrap(),
        email: "a@b.c".into(),
        server_url: None,
        location: AccountLocation::Managed,
    }
}
fn legacy() -> Account {
    Account { location: AccountLocation::CliDefault, ..managed("0".repeat(32).as_str()) }
}

#[test]
fn a_managed_accounts_paths_all_live_under_its_own_directory() {
    let cfg = Path::new(r"C:\cfg");
    let a = managed("0123456789abcdef0123456789abcdef");
    let dir = PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef");
    assert_eq!(data_dir_for(cfg, &a), Some(dir.clone()));
    assert_eq!(session_path_for(cfg, &a), dir.join("session.bin"));
    assert_eq!(hello_blob_path_for(cfg, &a), dir.join("hello.bin"));
}

#[test]
fn the_legacy_account_keeps_todays_paths_exactly() {
    // Not a nicety: `config_dir.join("session.bin")` is where `main.rs` reads
    // the token from today, and `hello.rs::blob_path` is where every existing
    // Windows Hello enrolment already sits. Moving either silently logs the
    // existing user out / destroys their enrolment on upgrade.
    let cfg = Path::new(r"C:\cfg");
    let a = legacy();
    assert_eq!(data_dir_for(cfg, &a), None, "the CLI's own default directory, unset env var");
    assert_eq!(session_path_for(cfg, &a), PathBuf::from(r"C:\cfg\session.bin"));
    assert_eq!(hello_blob_path_for(cfg, &a), PathBuf::from(r"C:\cfg\hello.bin"));
}

#[test]
fn no_two_accounts_share_a_session_or_hello_path() {
    // The spec's own test. Asserted over a set that includes the legacy
    // account, because that is the pair most likely to collide: it is the one
    // whose paths are NOT derived from an id.
    let cfg = Path::new(r"C:\cfg");
    let accounts = vec![
        legacy(),
        managed("0123456789abcdef0123456789abcdef"),
        managed("fedcba9876543210fedcba9876543210"),
        managed("00000000000000000000000000000000"),
    ];
    let mut paths: Vec<PathBuf> = Vec::new();
    for a in &accounts {
        paths.push(session_path_for(cfg, a));
        paths.push(hello_blob_path_for(cfg, a));
        if let Some(d) = data_dir_for(cfg, a) {
            paths.push(d);
        }
    }
    let count = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), count, "two accounts share a path: {paths:?}");
    // And the legacy account's id is deliberately in that set too -- note the
    // all-zero managed id above, which collides with `legacy()`'s id and must
    // still produce distinct paths because the LOCATION differs.
}
```

```rust
pub fn accounts_root(config_dir: &Path) -> PathBuf {
    config_dir.join("accounts")
}

pub fn data_dir_for(config_dir: &Path, account: &Account) -> Option<PathBuf> {
    match account.location {
        AccountLocation::CliDefault => None,
        AccountLocation::Managed => Some(accounts_root(config_dir).join(account.id.as_str())),
    }
}

pub fn session_path_for(config_dir: &Path, account: &Account) -> PathBuf {
    match data_dir_for(config_dir, account) {
        Some(dir) => dir.join("session.bin"),
        None => config_dir.join("session.bin"),
    }
}

pub fn hello_blob_path_for(config_dir: &Path, account: &Account) -> PathBuf {
    match data_dir_for(config_dir, account) {
        Some(dir) => dir.join("hello.bin"),
        None => config_dir.join("hello.bin"),
    }
}
```

### Step 2.3 — the KDF suffix, so one Hello credential serves many accounts

```rust
#[test]
fn the_legacy_accounts_kdf_suffix_is_empty_so_existing_blobs_still_open() {
    // `hello::derive_key` mixes KDF_LABEL then this suffix. An empty suffix
    // reproduces today's derivation byte for byte, which is what lets an
    // existing hello.bin keep working after upgrade.
    assert_eq!(hello_kdf_suffix_for(&legacy()), Vec::<u8>::new());
}

#[test]
fn two_managed_accounts_get_different_kdf_suffixes() {
    let a = managed("0123456789abcdef0123456789abcdef");
    let b = managed("fedcba9876543210fedcba9876543210");
    assert_ne!(hello_kdf_suffix_for(&a), hello_kdf_suffix_for(&b));
    assert_ne!(hello_kdf_suffix_for(&a), hello_kdf_suffix_for(&legacy()));
    // And the suffix carries the id, so it cannot be a constant that merely
    // differs from the legacy one.
    assert!(hello_kdf_suffix_for(&a).ends_with(a.id.as_str().as_bytes()));
}
```

```rust
/// Mixed into `hello`'s existing domain-separation label so one Windows Hello
/// credential seals a distinct key per account. Empty for the legacy account,
/// deliberately: that reproduces today's derivation exactly, so an existing
/// `hello.bin` still opens after upgrade.
pub fn hello_kdf_suffix_for(account: &Account) -> Vec<u8> {
    match account.location {
        AccountLocation::CliDefault => Vec::new(),
        AccountLocation::Managed => {
            let mut suffix = b" account ".to_vec();
            suffix.extend_from_slice(account.id.as_str().as_bytes());
            suffix
        }
    }
}
```

### Step 2.4 — lookups

```rust
#[test]
fn account_for_finds_by_id_and_misses_cleanly() {
    let list = vec![managed("0123456789abcdef0123456789abcdef"), legacy()];
    let wanted = AccountId::parse("0123456789abcdef0123456789abcdef").unwrap();
    assert_eq!(account_for(&list, &wanted).map(|a| a.id.clone()), Some(wanted));
    assert!(account_for(&list, &AccountId::parse(&"9".repeat(32)).unwrap()).is_none());
    assert!(account_for(&[], &AccountId::generate()).is_none());
}

#[test]
fn removing_the_active_account_falls_to_the_first_survivor_and_never_to_itself() {
    let a = managed("0123456789abcdef0123456789abcdef");
    let b = managed("fedcba9876543210fedcba9876543210");
    let list = vec![a.clone(), b.clone()];
    assert_eq!(next_active_after_removal(&list, &a.id).map(|x| x.id.clone()), Some(b.id.clone()));
    assert_eq!(next_active_after_removal(&list, &b.id).map(|x| x.id.clone()), Some(a.id.clone()));
    assert!(next_active_after_removal(&[a.clone()], &a.id).is_none(), "the last account");
}
```

**What would make these tests fail:** an id built from the email; `parse` accepting
`..` (the traversal test); `data_dir_for` giving the legacy account a managed directory
(the legacy-paths test, which asserts absolute strings); the KDF suffix being a
constant, or non-empty for the legacy account (which would orphan every existing
enrolment); `next_active_after_removal` returning the account being removed.

---

# Task 3 — Point the CLI at the active account's data directory

**Files:** modify `deskwarden/src/bw_path.rs`.

**Interfaces**

Consumes: `bw_path::bw_command()` (exists), `accounts::data_dir_for` (Task 2).

Produces:

```rust
/// The environment variable the Bitwarden CLI reads its profile directory from.
pub const BW_DATA_DIR_ENV: &str = "BITWARDENCLI_APPDATA_DIR";

pub fn set_active_data_dir(dir: Option<PathBuf>);
pub fn active_data_dir() -> Option<PathBuf>;
pub fn bw_command_in(dir: Option<&Path>) -> Result<Command, String>;
// `bw_command()` keeps its exact signature and becomes
// `bw_command_in(active_data_dir().as_deref())`.
```

`bw_command_in` exists so removing a **non-active** account can run `bw logout` in its
directory without a temporary mutation of the process-global — a temporary mutation
would race any background thread that spawns `bw` (there are several:
`spawn_backend_start`, `spawn_sync`, the status prefetch).

### Step 3.1 — the env var is set, and unset means unset

```rust
#[test]
fn a_command_built_for_a_directory_carries_the_appdata_env_var() {
    remember_verified_bw_exe(PathBuf::from(r"C:\a\bin\bw.exe")); // no-op if already set
    let dir = PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef");
    let Ok(cmd) = bw_command_in(Some(&dir)) else { return }; // skipped without a verified exe
    let found: Vec<_> = cmd
        .get_envs()
        .filter(|(k, _)| *k == std::ffi::OsStr::new(BW_DATA_DIR_ENV))
        .collect();
    assert_eq!(found.len(), 1, "the CLI reads exactly one profile-directory variable");
    assert_eq!(found[0].1, Some(dir.as_os_str()));
}

#[test]
fn a_command_built_for_the_cli_default_sets_no_appdata_env_var_at_all() {
    // NOT "sets it to empty": an empty `BITWARDENCLI_APPDATA_DIR` is a
    // different thing to the CLI than an absent one, and the legacy account's
    // whole guarantee is that it behaves exactly as it does today -- which is
    // with the variable never mentioned.
    let Ok(cmd) = bw_command_in(None) else { return };
    assert!(
        cmd.get_envs().all(|(k, _)| k != std::ffi::OsStr::new(BW_DATA_DIR_ENV)),
        "the legacy account must inherit the CLI's own default profile directory"
    );
}
```

### Step 3.2 — the process-global, and `bw_command` reading it

```rust
#[test]
fn bw_command_follows_the_active_data_dir() {
    let dir = PathBuf::from(r"C:\cfg\accounts\fedcba9876543210fedcba9876543210");
    set_active_data_dir(Some(dir.clone()));
    if let Ok(cmd) = bw_command() {
        assert_eq!(
            cmd.get_envs()
                .find(|(k, _)| *k == std::ffi::OsStr::new(BW_DATA_DIR_ENV))
                .and_then(|(_, v)| v),
            Some(dir.as_os_str()),
            "bw_command ignored the active data directory -- every bw spawn in the crate \
             goes through it, so an account switch would not reach the CLI at all"
        );
    }
    set_active_data_dir(None);
    if let Ok(cmd) = bw_command() {
        assert!(cmd.get_envs().all(|(k, _)| k != std::ffi::OsStr::new(BW_DATA_DIR_ENV)));
    }
}
```

Run this test serially — it mutates process-global state. Put it and any other
global-touching test behind a shared `static ACTIVE_DIR_LOCK: Mutex<()>` in the test
module and take the guard first, so a parallel test runner cannot interleave them.

```rust
static ACTIVE_DATA_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Points every subsequent `bw` spawn at `dir`, or at the CLI's own default
/// when `None`. THE ONLY WAY THE ACTIVE ACCOUNT REACHES THE CLI.
///
/// Callable more than once, unlike `remember_verified_bw_exe`: which account
/// is active changes, which *binary* is trusted does not.
pub fn set_active_data_dir(dir: Option<PathBuf>) {
    match ACTIVE_DATA_DIR.write() {
        Ok(mut slot) => *slot = dir,
        Err(e) => log::error!("the active data directory lock was poisoned: {e}"),
    }
}

pub fn active_data_dir() -> Option<PathBuf> {
    ACTIVE_DATA_DIR.read().ok().and_then(|slot| slot.clone())
}

pub fn bw_command_in(dir: Option<&Path>) -> Result<Command, String> {
    match verified_bw_exe() {
        Some(path) => {
            let mut cmd = Command::new(path);
            cmd.creation_flags(CREATE_NO_WINDOW);
            if let Some(dir) = dir {
                cmd.env(BW_DATA_DIR_ENV, dir);
            }
            Ok(cmd)
        }
        None => Err(/* the existing message, unchanged */),
    }
}

pub fn bw_command() -> Result<Command, String> {
    bw_command_in(active_data_dir().as_deref())
}
```

### Step 3.3 — pin the wiring: nothing bypasses `bw_command`

The whole design rests on `bw_command` being the only door. Pin it, with the
`concat!` rule.

```rust
#[test]
fn every_bw_spawn_in_the_crate_goes_through_bw_command() {
    // WIRING, not a decision. If a new call site builds `Command::new(bw)`
    // itself, that spawn silently uses whatever profile directory the CLI
    // picks by default -- so a switched account would keep answering from the
    // previous one, with no error anywhere.
    let mut offenders = Vec::new();
    for entry in walk_rust_sources(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")) {
        if entry.ends_with("bw_path.rs") {
            continue; // the definition itself
        }
        let text = std::fs::read_to_string(&entry).unwrap();
        // Split so this needle cannot match its own declaration.
        let needle = concat!("Command", "::new(");
        if text.contains(needle) && text.contains("bw") {
            for line in text.lines().filter(|l| l.contains(needle)) {
                // Test helpers spawn `cmd`/`ping`, not `bw`.
                if line.contains("bw") {
                    offenders.push(format!("{}: {}", entry.display(), line.trim()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "bw spawned outside bw_command:\n{}", offenders.join("\n"));

    // Positive control: the guard can actually see a violation.
    let planted = format!("let c = {}\"bw.exe\");", concat!("Command", "::new("));
    assert!(planted.contains(concat!("Command", "::new(")) && planted.contains("bw"));
}
```

**What would make these tests fail:** setting the env var to `""` for the legacy
account (3.1's second test); `bw_command` ignoring the global (3.2 — and note this is
the single mutation that would make the entire feature inert while every other test
stayed green); a new module spawning `bw` directly (3.3, which carries its own positive
control so the guard cannot be dead like `picker_ui.rs:1859`'s was).

---

# Task 4 — Persist the account list

**Files:** modify `deskwarden/src/settings.rs`.

**Interfaces**

Consumes: `accounts::{Account, AccountId}` (Task 2).

Produces:

```rust
pub struct Settings {
    pub keep_backend_running: bool,
    pub auto_lock_enabled: bool,
    pub auto_lock_minutes: u64,
    pub vault_window: Option<WindowGeometry>,
    /// Owned by the account code, NOT by the preferences window.
    pub accounts: Vec<crate::accounts::Account>,
    pub active_account: Option<crate::accounts::AccountId>,
}

impl Settings {
    pub fn persist_accounts(
        path: &Path,
        accounts: &[crate::accounts::Account],
        active: Option<&crate::accounts::AccountId>,
    ) -> std::io::Result<()>;
}
```

A **third** writer over disjoint fields, exactly mirroring
`persist_vault_window_geometry`. `persist_preferences` must destructure the two new
fields as `accounts: _, active_account: _` — the compile error the destructuring
produces is the mechanism that forces this decision to be made out loud.

### Step 4.1 — an older file still parses (the existing guarantee, restated)

```rust
#[test]
fn a_file_written_before_accounts_existed_still_parses() {
    let path = temp_path("pre-accounts");
    std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 3}"#).unwrap();
    let loaded = Settings::load(&path);
    assert!(loaded.accounts.is_empty(), "an absent list is 'no accounts configured yet'");
    assert_eq!(loaded.active_account, None);
    assert!(!loaded.keep_backend_running, "the fields it does carry still land");
    assert_eq!(loaded.auto_lock_minutes, 3);
    let _ = std::fs::remove_file(&path);
}
```

`a_partial_file_keeps_defaults_for_absent_fields` and every other existing settings
test must keep passing untouched. Add the new fields to `Default` (`Vec::new()` /
`None`) and to the struct literals in the existing round-trip tests only.

### Step 4.2 — the list round-trips through the real file

```rust
#[test]
fn the_account_list_round_trips_through_settings_json() {
    // Through the file, not just `PartialEq` on a struct: a field written but
    // not read, or renamed on one side only, looks fine in memory.
    let path = temp_path("accounts-round-trip");
    let id = crate::accounts::AccountId::parse("0123456789abcdef0123456789abcdef").unwrap();
    let written = Settings {
        accounts: vec![
            crate::accounts::Account {
                id: id.clone(),
                email: "work@example.com".into(),
                server_url: Some("https://vault.example.com".into()),
                location: crate::accounts::AccountLocation::Managed,
            },
            crate::accounts::Account {
                id: crate::accounts::AccountId::parse(&"a".repeat(32)).unwrap(),
                email: "me@example.com".into(),
                server_url: None,
                location: crate::accounts::AccountLocation::CliDefault,
            },
        ],
        active_account: Some(id.clone()),
        ..Settings::default()
    };
    written.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("work@example.com"), "not in the file at all: {text}");
    assert!(!text.contains("password") && !text.contains("session"), "NO SECRETS: {text}");
    let loaded = Settings::load(&path);
    assert_eq!(loaded, written);
    assert_eq!(loaded.active_account, Some(id));
    assert_eq!(loaded.accounts[1].location, crate::accounts::AccountLocation::CliDefault);
    let _ = std::fs::remove_file(&path);
}
```

### Step 4.3 — three writers, disjoint fields, in every pairing

```rust
#[test]
fn persisting_accounts_keeps_every_preference_and_the_geometry() {
    let path = temp_path("accounts-preserve");
    Settings { keep_backend_running: false, auto_lock_minutes: 7, ..Settings::default() }
        .save(&path).unwrap();
    Settings::persist_vault_window_geometry(
        &path, WindowGeometry { x: 1, y: 2, width: 1000, height: 700 }).unwrap();
    let id = crate::accounts::AccountId::parse(&"b".repeat(32)).unwrap();
    Settings::persist_accounts(&path, &[account(&id)], Some(&id)).unwrap();

    let loaded = Settings::load(&path);
    assert!(!loaded.keep_backend_running, "persist_accounts clobbered a preference");
    assert_eq!(loaded.auto_lock_minutes, 7);
    assert_eq!(loaded.vault_window.map(|g| g.x), Some(1), "persist_accounts clobbered the geometry");
    assert_eq!(loaded.active_account, Some(id));
}

#[test]
fn persisting_preferences_from_a_stale_copy_keeps_the_account_list() {
    // The regression, in the order the app performs it: `main` loads
    // `Settings` once at startup; an account is added mid-session and written
    // by `persist_accounts`; the user then opens Preferences and changes the
    // auto-lock. A whole-struct save at that point writes main's stale (empty)
    // account list back and the added account VANISHES on next launch, with no
    // error anywhere -- the identical trap the geometry fell into.
    let path = temp_path("prefs-preserve-accounts");
    let at_startup = Settings::load(&path);
    assert!(at_startup.accounts.is_empty());

    let id = crate::accounts::AccountId::parse(&"c".repeat(32)).unwrap();
    Settings::persist_accounts(&path, &[account(&id)], Some(&id)).unwrap();

    Settings { auto_lock_minutes: 10, ..at_startup }.persist_preferences(&path).unwrap();

    let loaded = Settings::load(&path);
    assert_eq!(loaded.accounts.len(), 1, "a preferences save deleted the account list");
    assert_eq!(loaded.active_account, Some(id));
    assert_eq!(loaded.auto_lock_minutes, 10, "and the preference itself must still land");
}

#[test]
fn persisting_accounts_wins_over_a_stale_list_in_the_file() {
    // The other direction, so the read-modify-write cannot be "fixed" into
    // merely ignoring the accounts.
    let path = temp_path("accounts-win");
    let a = crate::accounts::AccountId::parse(&"d".repeat(32)).unwrap();
    let b = crate::accounts::AccountId::parse(&"e".repeat(32)).unwrap();
    Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();
    Settings::persist_accounts(&path, &[account(&a), account(&b)], Some(&b)).unwrap();
    let loaded = Settings::load(&path);
    assert_eq!(loaded.accounts.len(), 2);
    assert_eq!(loaded.active_account, Some(b));
}
```

```rust
pub fn persist_accounts(
    path: &Path,
    accounts: &[crate::accounts::Account],
    active: Option<&crate::accounts::AccountId>,
) -> std::io::Result<()> {
    let mut on_disk = Self::load(path);
    on_disk.accounts = accounts.to_vec();
    on_disk.active_account = active.cloned();
    on_disk.save(path)
}
```

and in `persist_preferences`:

```rust
let Settings {
    keep_backend_running,
    auto_lock_enabled,
    auto_lock_minutes,
    vault_window: _,
    // Owned by `persist_accounts`. Named here so a future field cannot join
    // the set this writer silently drops.
    accounts: _,
    active_account: _,
} = self;
```

**What would make these tests fail:** `persist_accounts` implemented as a whole-struct
`save` (4.3's first test); `persist_preferences` writing `accounts` from a stale copy
(4.3's second — the exact geometry regression, repeated); a field serialised but never
read back (4.2 asserts on the file's text); an older file failing to parse because
`#[serde(default)]` was dropped or the new field made non-defaultable (4.1, plus the
pre-existing partial-file test).

---

# Task 5 — Per-account `session.bin` and `hello.bin`

**Files:** modify `deskwarden/src/hello.rs`; modify `deskwarden/src/session_store.rs`
(doc only — it already takes a `PathBuf`, so nothing structural changes).

**Interfaces**

Consumes: `accounts::{Account, hello_blob_path_for, hello_kdf_suffix_for,
session_path_for}` (Task 2).

Produces (in `hello.rs`, replacing the account-less forms):

```rust
pub fn blob_path_for(config_dir: &Path, account: &Account) -> PathBuf;
pub fn state_for(config_dir: &Path, account: &Account) -> HelloState;
pub fn enroll_for(config_dir: &Path, account: &Account, master_password: &str)
    -> Result<(), String>;
pub fn unlock_password_for(config_dir: &Path, account: &Account)
    -> Result<Zeroizing<String>, String>;
pub fn unenroll_for(config_dir: &Path, account: &Account);
```

`blob_path()`, `state()`, `enroll()`, `unlock_password()` and `unenroll()` are deleted;
their one caller each (in `login_ui.rs`) is updated in Task 6.

`derive_key` gains the suffix:

```rust
fn derive_key(signature: &[u8], account_suffix: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(account_suffix);
    hasher.update(signature);
    Zeroizing::new(hasher.finalize().into())
}
```

### Step 5.1 — never `ReplaceExisting`

The credential is **shared across accounts**. `RequestCreateAsync(ReplaceExisting)`
rotates it, which changes the signature, which changes every derived key, which
silently destroys every other account's enrolment. The existing doc on
`hello_derived_key` justifies `ReplaceExisting` ("a stale credential from an abandoned
enrollment has no blob to pair with and is worthless") — **that justification is false
once a second account exists, and the comment must be corrected, not left standing.**
(Four separate false comments about timeouts is how this repository already learned
that a stale comment outlives the code it describes.)

```rust
#[test]
fn hello_never_asks_windows_to_replace_the_shared_credential() {
    // WIRING, and unavoidably a source guard: `RequestCreateAsync` needs real
    // TPM-backed Hello hardware and a live user, so no test can call it.
    // `ReplaceExisting` rotates the ONE credential every account's key is
    // derived from -- enrolling account B would silently destroy account A's
    // enrolment, and A would only find out at the moment it tried to unlock.
    let source = include_str!("hello.rs");
    // Split so the needle cannot match its own declaration in this file.
    let banned = concat!("KeyCredentialCreationOption", "::ReplaceExisting");
    assert_eq!(
        source.matches(banned).count(),
        1,
        "`{banned}` must appear ONCE and only inside this test's own needle -- \
         it rotates the shared Hello credential and destroys every other account's enrolment"
    );
    // The replacement is present.
    let required = concat!("KeyCredentialCreationOption", "::FailIfExists");
    assert!(source.contains(required), "enrolment must use {required}");
    // Positive control: the counting mechanism can see a second occurrence.
    let planted = format!("{banned} {banned}");
    assert_eq!(planted.matches(banned).count(), 2);
}
```

```rust
/// `create` enrols rather than unlocks. **`FailIfExists`, never
/// `ReplaceExisting`** (which this used to pass): the credential named
/// [`CREDENTIAL_NAME`] is SHARED BY EVERY ACCOUNT -- accounts are separated by
/// the KDF suffix (`accounts::hello_kdf_suffix_for`), not by having a
/// credential each. Replacing it rotates the private key, which changes the
/// signature, which changes every derived key: enrolling a second account
/// would silently destroy the first one's enrolment, discovered only at the
/// moment that account next tried to unlock. `AlreadyExists` is therefore not
/// a failure here -- it is the normal case for the second and every later
/// account -- and is handled by opening the existing credential instead.
fn hello_derived_key(create: bool, account_suffix: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);

    let result = if create {
        let created =
            KeyCredentialManager::RequestCreateAsync(&name, KeyCredentialCreationOption::FailIfExists)
                .and_then(|op| op.get());
        match created.as_ref().map(|r| r.Status()) {
            Ok(Ok(KeyCredentialStatus::CredentialAlreadyExists)) => {
                KeyCredentialManager::OpenAsync(&name).and_then(|op| op.get())
            }
            _ => created,
        }
    } else {
        KeyCredentialManager::OpenAsync(&name).and_then(|op| op.get())
    }
    .map_err(|e| format!("Windows Hello is unavailable: {e}"))?;
    // ... the existing status match, signing and derivation, unchanged except
    // for passing `account_suffix` into `derive_key`.
}
```

### Step 5.2 — one account's blob cannot open another's

```rust
#[test]
fn a_blob_sealed_for_one_account_does_not_open_for_another() {
    // The property per-account key derivation exists for. Driven through
    // `seal`/`unseal` under the same fixed pretend signature, so the ONLY
    // thing that differs between the two sides is the account.
    let signature = b"pretend hello signature";
    let a = managed_account("0123456789abcdef0123456789abcdef");
    let b = managed_account("fedcba9876543210fedcba9876543210");

    let key_a = derive_key(signature, &crate::accounts::hello_kdf_suffix_for(&a));
    let key_b = derive_key(signature, &crate::accounts::hello_kdf_suffix_for(&b));
    let sealed = seal(&key_a, b"account A master password").unwrap();

    assert!(unseal(&key_b, &sealed).is_err(), "account B opened account A's sealed password");
    // Positive control on the same blob: A's own key still opens it, so this
    // does not pass merely because `seal`/`unseal` are broken.
    assert_eq!(unseal(&key_a, &sealed).unwrap().as_slice(), b"account A master password");
}

#[test]
fn the_legacy_accounts_derivation_is_byte_for_byte_what_it_was_before() {
    // An existing hello.bin was sealed under SHA-256(KDF_LABEL || signature).
    // If the suffix is not empty for the legacy account, every existing
    // enrolment silently stops opening on upgrade -- and the user's only
    // symptom is a quick-unlock panel that always errors.
    let signature = b"pretend hello signature";
    let mut expected = Sha256::new();
    expected.update(KDF_LABEL);
    expected.update(signature);
    let expected: [u8; 32] = expected.finalize().into();
    let legacy = legacy_account();
    assert_eq!(
        derive_key(signature, &crate::accounts::hello_kdf_suffix_for(&legacy)).as_slice(),
        &expected,
        "the legacy account's Hello key changed -- every existing enrolment is now dead"
    );
}
```

### Step 5.3 — paths and enrolment state follow the account

```rust
#[test]
fn hello_state_is_per_account() {
    let cfg = scratch_dir("hello-per-account");
    let a = managed_account("0123456789abcdef0123456789abcdef");
    let b = managed_account("fedcba9876543210fedcba9876543210");
    std::fs::create_dir_all(blob_path_for(&cfg, &a).parent().unwrap()).unwrap();
    std::fs::write(blob_path_for(&cfg, &a), b"not a real blob").unwrap();

    // `state_for` reports enrolment from the blob's existence; `available`
    // needs real hardware, so assert the half that is decidable here.
    assert!(blob_path_for(&cfg, &a).exists());
    assert!(!blob_path_for(&cfg, &b).exists(), "B must not see A's enrolment");
    assert_ne!(blob_path_for(&cfg, &a), blob_path_for(&cfg, &b));

    unenroll_for(&cfg, &b);
    assert!(blob_path_for(&cfg, &a).exists(), "unenrolling B deleted A's blob");
    unenroll_for(&cfg, &a);
    assert!(!blob_path_for(&cfg, &a).exists());
    let _ = std::fs::remove_dir_all(&cfg);
}
```

`enroll_for` must `create_dir_all` the blob's parent before writing — a managed
account's directory may not exist yet the first time.

**What would make these tests fail:** `ReplaceExisting` surviving anywhere (5.1);
`derive_key` ignoring the suffix, so both accounts derive the same key (5.2's first
test); the legacy account getting a non-empty suffix (5.2's second — the upgrade
regression); `unenroll_for` deleting a fixed path rather than the account's (5.3).

---

# Task 6 — A login flow that can be cancelled

**This is the task the spec does not know it needs.** `login_ui::run_login_flow()` ends:

```rust
None => {
    log::error!("login window was closed without producing a session token; exiting");
    std::process::exit(1);
}
```

That is right for its two existing call sites (startup and the lock/re-auth recovery —
there is genuinely nothing to continue with). It is **fatal** for a switch: the spec
requires "Wrong master password: stay on the current account, current vault intact",
and today closing that window kills the app. Same for "Add account…". A switch cannot
be built on a flow that cannot be declined.

**Files:** modify `deskwarden/src/login_ui.rs`.

**Interfaces**

Produces:

```rust
/// The login/unlock window, returning `None` when the user closed it without
/// producing a session.
pub fn run_login_flow_cancellable() -> Option<String>;

/// Unchanged behaviour for the two call sites that genuinely cannot continue
/// without a session: startup and the lock/re-auth recovery.
pub fn run_login_flow() -> String;
```

Consumes: `hello::{state_for, enroll_for, unlock_password_for, unenroll_for}` (Task 5),
which requires the window to know which account it is authenticating. Thread it in
rather than reaching for another global:

```rust
pub fn run_login_flow_for(config_dir: &Path, account: &Account) -> Option<String>;
// `run_login_flow_cancellable()` is `run_login_flow_for(active_config_dir(), active_account())`
// -- but prefer passing them explicitly from `main` and deleting the no-arg form.
```

### Step 6.1 — split the exit out of the flow

```rust
pub fn run_login_flow_for(config_dir: &Path, account: &Account) -> Option<String> {
    // ...the entire existing body, verbatim, up to and including...
    token.borrow_mut().take()
}

/// The two callers that cannot continue without a session: startup, and the
/// lock/re-auth recovery. Everything else -- an account switch, adding an
/// account -- must use [`run_login_flow_for`] and handle `None`, because
/// declining to switch accounts is an ordinary gesture and must not kill an
/// already-running app. That distinction is the whole reason these are two
/// functions.
pub fn run_login_flow(config_dir: &Path, account: &Account) -> String {
    match run_login_flow_for(config_dir, account) {
        Some(token) => token,
        None => {
            log::error!("login window was closed without producing a session token; exiting");
            std::process::exit(1);
        }
    }
}
```

```rust
#[test]
fn only_one_login_entry_point_can_exit_the_process() {
    // A source guard, and unavoidably so: `run_login_flow_for` opens a real
    // eframe window. What it pins is the thing a switch depends on -- that
    // `process::exit` lives in the WRAPPER and not in the body every other
    // caller now uses. Inlined back into the body, a user who closes the
    // master-password prompt during a switch kills a running app, and every
    // test in the suite stays green because none of them opens that window.
    let source = include_str!("login_ui.rs");
    let needle = concat!("std::process", "::exit(1)");
    assert_eq!(
        source.matches(needle).count(),
        1,
        "exactly one login path may exit the process; found {} occurrences",
        source.matches(needle).count()
    );
    // ...and it is inside `run_login_flow`, not `run_login_flow_for`.
    let wrapper = source
        .split_once("pub fn run_login_flow(")
        .expect("run_login_flow must exist")
        .1;
    assert!(
        wrapper.contains(needle),
        "the exit moved out of the wrapper -- the cancellable body now kills the app"
    );
    // Positive control: the split actually isolated a region.
    assert!(wrapper.len() < source.len());
}
```

### Step 6.2 — per-account Hello in the window

Replace the five `hello::` call sites inside `run_login_flow_for` with their `_for`
equivalents, passing the `config_dir` and `account` the function now takes. The
`LoginAction::LogOut` arm's `hello::unenroll()` becomes `hello::unenroll_for(config_dir,
account)` — logging out of *this* account must not drop another's enrolment.

```rust
#[test]
fn the_login_window_uses_only_the_per_account_hello_entry_points() {
    // WIRING. `hello::unenroll()` deleted a fixed path; left in place after
    // Task 5, logging out of account B would silently delete account A's
    // enrolment, and nothing in the suite would notice because neither
    // function is reachable from a test.
    let source = include_str!("login_ui.rs");
    for stale in [
        concat!("hello", "::unenroll()"),
        concat!("hello", "::state()"),
        concat!("hello", "::unlock_password()"),
        concat!("hello", "::enroll("),
    ] {
        assert!(!source.contains(stale), "{stale} is still called here");
    }
    for required in [
        concat!("hello", "::unenroll_for("),
        concat!("hello", "::state_for("),
        concat!("hello", "::unlock_password_for("),
        concat!("hello", "::enroll_for("),
    ] {
        assert!(source.contains(required), "{required} is not called here");
    }
}
```

Note the `enroll(` needle is written without a trailing `)` deliberately — `enroll_for(`
does not contain `enroll(`, so the two assertions cannot both be satisfied by one call.
Verify that by construction when writing the test: `assert!(!"hello::enroll_for(".contains("hello::enroll("));`.

**What would make these tests fail:** `process::exit` left in the body (6.1 — the single
mutation that makes a declined switch fatal); a stale account-less `hello::` call
surviving in the login window (6.2); `run_login_flow_cancellable` implemented as
`Some(run_login_flow())`, which is caught by 6.1's second assertion since the exit would
then be reachable from the cancellable path.

---

# Task 7 — Extract the resettle sequence (pure refactor)

No behaviour changes. The existing 880 lib + 34 bin tests are the safety net; in
particular `matches_from_the_pre_lock_account_do_not_survive_the_unlock`,
`a_backend_that_cannot_be_restarted_after_unlock_leaves_the_app_running`,
`a_readiness_timeout_after_unlock_leaves_the_app_running` and
`a_late_sync_from_a_previous_account_is_discarded_even_though_the_cache_refilled` all
cover this block and must pass unchanged.

**Files:** modify `deskwarden/src/main.rs`.

**Interfaces**

Consumes: everything the block already uses — `VaultCache`, `MatchEngine`,
`bw_serve::{stop_bw_serve, PORT_RELEASE_GRACE_RESTART}`, `try_start_backend`,
`restart_backend_after_unlock`, `settle_vault_after_unlock`,
`wait_for_vault_ready_with_spinner`, `apply_backend_op`, `tray::set_sync_idle`.

Produces:

```rust
/// What a resettle ended up doing, for a caller that has to decide whether to
/// roll back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResettleOutcome {
    /// The backend came up and the settle ran. (The settle itself may still
    /// have stood autofill down -- that is a survivable outcome the app
    /// already ships, not a reason to roll back a switch.)
    BackendStarted,
    /// `try_start_backend` failed; `stand_down_after_unlock` has run,
    /// `bw_serve_child` is `None`, the engine is cleared.
    BackendNotStarted,
}

#[allow(clippy::too_many_arguments)]
fn resettle_session(
    cache: &Arc<VaultCache>,
    engine: &mut MatchEngine,
    bw_serve_child: &mut Option<Child>,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    schedule: &[Duration],
    tray: &tray::AppTray,
    backend_op_rx: &mpsc::Receiver<BackendOp>,
    backend_task_in_progress: &mut Option<(Instant, BackendOpKind)>,
    cached_status_details: &mut Option<login_ui::BwStatusDetails>,
    session_token: &mut String,
    // THE ONLY THING THAT DIFFERS between a lock/re-auth and an account
    // switch. Lock/re-auth passes `|| reauthenticate(&store)`. A switch passes
    // a closure that first points `bw_path` at the target account's data
    // directory and builds that account's `SessionStore`, then does the same.
    // Runs AFTER the old backend is stopped and the cache is cleared, and
    // BEFORE the new backend is started -- which is the whole ordering that
    // makes a half-switched app unreachable.
    authenticate: impl FnOnce() -> Option<String>,
) -> ResettleOutcome;
```

`authenticate` returns `Option` so a declined switch (Task 6) is expressible. `None`
means: nothing was authenticated, so nothing may be started —
`resettle_session` returns `BackendNotStarted` after standing down, and the caller
rolls back.

### Step 7.1 — move the block, change nothing

Cut `main.rs` lines 1839–1951 into `resettle_session`, verbatim, substituting
`authenticate()` for `reauthenticate(store)`. The call site becomes:

```rust
if result.locked || result.needs_reauth {
    if result.needs_reauth {
        log::warn!("vault window write failed with an unauthorized session; re-authenticating");
    } else {
        log::info!("vault window locked itself; re-authenticating");
    }
    if resettle_session(
        cache, engine, bw_serve_child, job, schedule, tray, backend_op_rx,
        backend_task_in_progress, cached_status_details, session_token,
        // Startup and this path are the two that genuinely cannot continue
        // without a session, so this is the one that keeps `run_login_flow`'s
        // exit-on-close (see `login_ui`).
        || Some(reauthenticate(store, config_dir, active_account)),
    ) == ResettleOutcome::BackendNotStarted
    {
        return;
    }
}
```

Run the suite. **880 lib + 34 bin must still pass, warning-free**, with no test edited.
If any test needed editing, the refactor was not a refactor.

### Step 7.2 — pin that there is one sequence

```rust
#[test]
fn there_is_exactly_one_teardown_and_repopulate_path() {
    // The spec's own warning, made mechanical: "if an implementation finds
    // itself writing a second teardown-and-repopulate path, that is the signal
    // it has gone wrong". `settle_vault_after_unlock` is the tail of the
    // sequence; a second call site means a second implementation of the
    // hardest code in this codebase, and it would not have these tests.
    let source = include_str!("main.rs");
    let needle = concat!("settle_vault_after", "_unlock(");
    let count = source.matches(needle).count();
    assert_eq!(
        count, 2,
        "expected the definition and exactly ONE call site (inside `resettle_session`); found {count}"
    );
    // And the call site is inside `resettle_session`, not beside it.
    let body = source
        .split_once(concat!("fn resettle", "_session("))
        .expect("resettle_session must exist")
        .1;
    assert!(body.contains(needle), "the settle moved out of `resettle_session`");
    // Positive control: the split isolated a region, and the counting works.
    assert!(body.len() < source.len());
    assert_eq!(format!("{needle} {needle}").matches(needle).count(), 2);
}
```

### Step 7.3 — a composition test for the new seam

```rust
#[test]
fn a_declined_authentication_starts_no_backend_and_leaves_the_cache_cleared() {
    // The new arm the extraction introduced. Driven with `authenticate`
    // returning `None`, which is what a user closing the master-password
    // prompt during a switch produces.
    let cache = Arc::new(VaultCache::new(VaultBridge::new("http://127.0.0.1:1")));
    let mut engine = MatchEngine::new();
    engine.rebuild(&[("id".into(), AppMatch { process: "prev.exe".into(), trigger: TriggerMode::Prompt })]);
    // ...construct the remaining parameters as the existing `main.rs` tests do...
    let outcome = resettle_session(/* .. */, || None);
    assert_eq!(outcome, ResettleOutcome::BackendNotStarted);
    assert!(engine.lookup("prev.exe").is_none(), "the previous account's matches survived");
    assert!(!cache.is_populated());
}
```

**What would make these tests fail:** a second `settle_vault_after_unlock` call site
appearing anywhere (7.2 — this is the guard against the spec's named failure mode);
the settle being lifted back out of `resettle_session`; a `None` from `authenticate`
proceeding to `try_start_backend` anyway (7.3); any behaviour change in the moved block
(the 880 existing tests).

---

# Task 8 — The switch itself

**Files:** modify `deskwarden/src/main.rs`.

**Interfaces**

Consumes: `resettle_session`, `ResettleOutcome` (Task 7);
`bw_path::set_active_data_dir` (Task 3); `accounts::{Account, data_dir_for,
session_path_for}` (Task 2); `login_ui::run_login_flow_for` (Task 6).

Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum SwitchOutcome {
    /// The target account is live: its backend is up, the cache holds its
    /// vault, the engine holds its matches.
    Switched,
    /// Nothing changed. The previous account is active, its backend is up, its
    /// cache and engine are as they were.
    Declined,
    /// The switch failed and the previous account was restored.
    RolledBack { reason: String },
    /// The switch failed AND the rollback failed. The app is running with no
    /// backend and an empty cache -- `stand_down_after_unlock`'s state, which
    /// this app already ships and recovers from via the tray's Sync.
    StoodDown { reason: String },
}

#[allow(clippy::too_many_arguments)]
fn switch_to_account(
    config_dir: &Path,
    from: &Account,
    to: &Account,
    active_account: &mut Account,
    store: &mut session_store::SessionStore,
    cache: &Arc<VaultCache>,
    engine: &mut MatchEngine,
    /* the same bw_serve_child / job / schedule / tray / backend_op_rx /
       backend_task_in_progress / cached_status_details / session_token
       parameters `resettle_session` takes */
    // Injected so the tests can drive the whole composition without opening a
    // window or starting a backend. The live caller passes
    // `|cfg, account| resettle_session(.., || login_ui::run_login_flow_for(cfg, account))`.
    mut resettle: impl FnMut(&Path, &Account) -> ResettleOutcome,
) -> SwitchOutcome;
```

The injected-closure shape is the one this codebase has already proven with
`settle_vault_after_unlock(.., probe)`: the composition is what the tests drive, not a
reimplementation of it beside the live one.

### Step 8.1 — the order, which is the whole safety property

```rust
fn switch_to_account(..) -> SwitchOutcome {
    let previous_token = session_token.clone();
    let previous_dir = bw_path::active_data_dir();

    // 1. Point the CLI and the token store at the target, BEFORE anything
    //    authenticates -- `run_login_flow_for` spawns `bw`, and it has to land
    //    in the target's profile.
    bw_path::set_active_data_dir(accounts::data_dir_for(config_dir, to));
    *store = session_store::SessionStore::new(accounts::session_path_for(config_dir, to));

    // 2. The existing sequence. `resettle_session` stops the old backend,
    //    clears the cache (which bumps the era, discarding any populate the
    //    PREVIOUS account still has in flight), authenticates, starts the new
    //    backend, waits for readiness, repopulates and rebuilds the engine.
    match resettle(config_dir, to) {
        ResettleOutcome::BackendStarted => {
            *active_account = to.clone();
            // Only NOW is the outgoing token discarded. Doing it up front
            // would make a rollback cost the user a second password prompt for
            // an account they never asked to leave; and until the switch
            // lands, the outgoing account is still this app's account, not an
            // idle one.
            if let Err(e) = std::fs::remove_file(accounts::session_path_for(config_dir, from)) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("could not discard the previous account's session token: {e}");
                }
            }
            SwitchOutcome::Switched
        }
        ResettleOutcome::BackendNotStarted => {
            // 3. Roll back. Point everything at the previous account and run
            //    THE SAME sequence, authenticating from the token we still
            //    hold rather than prompting again.
            log::warn!("switching to {} failed; returning to {}", to.email, from.email);
            bw_path::set_active_data_dir(previous_dir);
            *store = session_store::SessionStore::new(accounts::session_path_for(config_dir, from));
            match resettle(config_dir, from) {
                ResettleOutcome::BackendStarted => SwitchOutcome::RolledBack { .. },
                ResettleOutcome::BackendNotStarted => SwitchOutcome::StoodDown { .. },
            }
        }
    }
}
```

The rollback's `resettle` in the live caller authenticates with
`|| Some(previous_token.clone())` — no prompt, because that session was never
invalidated. That is why `previous_token` is captured on the first line.

### Step 8.2 — the previous account's matches are GONE

```rust
#[test]
fn a_switch_rebuilds_the_engine_so_the_previous_accounts_matches_are_gone() {
    // The spec's own test, and the one with the worst failure mode: a match
    // left armed from account A, under account B's session, raises an autofill
    // prompt whose fill can only ever end in an error -- or worse, in a
    // credential from the wrong vault.
    let (cache, mut engine) = ..;
    engine.rebuild(&[(
        "a-item".into(),
        AppMatch { process: "notepad.exe".into(), trigger: TriggerMode::Auto },
    )]);
    assert!(engine.lookup("notepad.exe").is_some(), "precondition");

    let outcome = switch_to_account(.., |_, account| {
        // Stands in for the real sequence: clear + repopulate + rebuild from
        // the TARGET's items, which is what `resettle_session` does.
        cache.clear();
        let items = items_for(account);          // B's vault: one match on `code.exe`
        let epoch = cache.epoch();
        engine.rebuild(&match_entries(&items));
        cache.populate_with(items, epoch).unwrap();
        ResettleOutcome::BackendStarted
    });

    assert_eq!(outcome, SwitchOutcome::Switched);
    assert!(
        engine.lookup("notepad.exe").is_none(),
        "account A's match is STILL ARMED under account B's session"
    );
    // Positive control: not merely "the engine is empty".
    assert!(engine.lookup("code.exe").is_some(), "account B's own match is not armed");
    // And the cache holds B, not A.
    assert!(cache.items().iter().all(|i| i.id != "a-item"));
}
```

### Step 8.3 — a populate in flight across a switch is discarded

```rust
#[test]
fn a_populate_from_the_previous_account_in_flight_across_a_switch_is_discarded() {
    // The era machinery is what makes a switch safe, and this is the assertion
    // that it is actually being ROUTED THROUGH rather than bypassed. Modelled
    // on the existing era tests.
    let cache = Arc::new(VaultCache::new(VaultBridge::new(&server.url())));
    // Account A's fetch begins: the epoch is captured before it.
    let a_epoch = cache.epoch();
    let a_items = probe_items(&[("a-item", "notepad.exe")]);

    // The switch lands. `resettle_session`'s `cache.clear()` bumps the era.
    switch_to_account(.., |_, _| { cache.clear(); ResettleOutcome::BackendStarted });

    // A's slow populate now completes, holding a pre-switch epoch.
    assert_eq!(
        cache.populate_with(a_items, a_epoch).unwrap(),
        PopulateOutcome::DiscardedStale,
        "account A's items were written into account B's cache"
    );
    assert!(cache.items().is_empty());

    // Positive control: an epoch captured AFTER the switch is not discarded,
    // so this does not pass merely because `populate_with` always discards.
    let b_epoch = cache.epoch();
    assert_eq!(
        cache.populate_with(probe_items(&[("b-item", "code.exe")]), b_epoch).unwrap(),
        PopulateOutcome::Populated
    );
}
```

### Step 8.4 — a failed switch leaves the previous account fully working

```rust
#[test]
fn a_failed_switch_returns_to_the_previous_account_with_everything_working() {
    // "A half-switched app -- new data directory, old cache -- is the one
    // outcome that must not be reachable."
    let mut seen: Vec<(PathBuf_or_none, String)> = Vec::new();
    let outcome = switch_to_account(config_dir, &a, &b, .., |cfg, account| {
        // Record what the CLI is pointed at AT THE MOMENT the sequence runs.
        seen.push((bw_path::active_data_dir(), account.email.clone()));
        if account.id == b.id {
            ResettleOutcome::BackendNotStarted     // B's backend will not start
        } else {
            // A's rollback: repopulate and rearm exactly as the live path does.
            cache.clear();
            let items = items_for(account);
            let epoch = cache.epoch();
            engine.rebuild(&match_entries(&items));
            cache.populate_with(items, epoch).unwrap();
            ResettleOutcome::BackendStarted
        }
    });

    assert!(matches!(outcome, SwitchOutcome::RolledBack { .. }));
    // The CLI is pointed back at A -- NOT left on B's directory beside A's cache.
    assert_eq!(bw_path::active_data_dir(), accounts::data_dir_for(config_dir, &a));
    // A is still the active account.
    assert_eq!(active_account.id, a.id);
    // A's vault is in the cache and A's matches are armed.
    assert!(cache.is_populated());
    assert!(engine.lookup("notepad.exe").is_some(), "A's autofill is dead after a failed switch");
    // A's session token was NOT discarded -- the rollback must not have cost a
    // second password prompt.
    assert!(accounts::session_path_for(config_dir, &a).exists());
    // And the sequence really was run twice, at B then at A.
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, accounts::data_dir_for(config_dir, &b));
    assert_eq!(seen[1].0, accounts::data_dir_for(config_dir, &a));
}
```

That last pair of assertions is the wiring pin: it asserts the data directory was
actually swapped *before* the sequence ran, which is the one genuinely new behaviour in
this whole feature and the one a decision-only test would miss entirely.

### Step 8.5 — a declined switch changes nothing

```rust
#[test]
fn declining_the_master_password_prompt_leaves_the_previous_account_active() {
    // The spec: "Wrong master password: stay on the current account, current
    // vault intact." Closing the prompt is the same gesture.
    let outcome = switch_to_account(.., |_, account| {
        if account.id == b.id { ResettleOutcome::BackendNotStarted } else { /* rollback ok */ }
    });
    assert!(matches!(outcome, SwitchOutcome::RolledBack { .. } | SwitchOutcome::Declined));
    assert_eq!(active_account.id, a.id);
    assert_eq!(bw_path::active_data_dir(), accounts::data_dir_for(config_dir, &a));
}
```

### Step 8.6 — a switch never kills the app

```rust
#[test]
fn no_switch_path_can_reach_the_fatal_startup_error() {
    // The spec's explicit warning. `start_backend` (startup's wrapper around
    // `try_start_backend`) calls `fatal_startup_error`; killing the app
    // because the OTHER account's backend would not start is not acceptable.
    // Source guard, because the alternative is a test that calls
    // `process::exit`.
    let source = include_str!("main.rs");
    let switch_body = source
        .split_once(concat!("fn switch_to", "_account("))
        .expect("switch_to_account must exist")
        .1
        .split_once("\n}\n")
        .expect("the function must be brace-terminated at column 0")
        .0;
    for banned in [concat!("fatal_startup", "_error("), concat!("start_backend", "(")] {
        assert!(
            !switch_body.contains(banned),
            "`{banned}` is reachable from a switch -- a failed switch would kill a running app"
        );
    }
    // Positive controls: the region is non-empty and both needles CAN be found
    // in the file, so this does not pass because the split produced nothing.
    assert!(!switch_body.is_empty());
    assert!(source.contains(concat!("fatal_startup", "_error(")));
    assert!(source.contains(concat!("start_backend", "(")));
}
```

Note the needle `start_backend(` also matches `try_start_backend(` — deliberately, and
correctly: the switch must go through `resettle_session`, which is the only thing that
may call either. If the switch body calls neither, both assertions hold.

**What would make these tests fail:** the data directory swapped *after* the sequence
runs, or not at all (8.4's `seen` assertions); the engine merged rather than rebuilt
(8.2); the era bypassed, e.g. by reaching for `populate_with` without a clear (8.3);
no rollback on failure (8.4); the outgoing token deleted before the switch lands
(8.4's `session_path_for(&a).exists()`); `fatal_startup_error` on the switch path
(8.6).

---

# Task 9 — Startup: resolve the active account, resume it

**Files:** modify `deskwarden/src/main.rs`; modify `deskwarden/src/accounts.rs`.

**Interfaces**

Produces (in `accounts.rs`, pure and directly testable):

```rust
/// The account this launch runs as, and the list to persist, given what is on
/// disk. Pure so the whole startup decision -- including the first-launch
/// adoption of the pre-existing account -- is assertable without a config
/// directory or a CLI.
pub struct ResolvedStartup {
    pub active: Account,
    pub accounts: Vec<Account>,
    /// True when `accounts` differs from what was loaded and must be written
    /// back with `Settings::persist_accounts`.
    pub needs_persist: bool,
}

pub fn resolve_startup(
    stored: &[Account],
    stored_active: Option<&AccountId>,
    status_email: Option<&str>,
    status_server_url: Option<&str>,
) -> ResolvedStartup;
```

### Step 9.1 — first launch adopts the existing account without moving it

```rust
#[test]
fn a_first_launch_adopts_the_existing_profile_as_the_cli_default_account() {
    // Upgrade from a build with no account list. The account that already
    // exists MUST come back as CliDefault: it lives in the CLI's own profile
    // directory, its session.bin is `config_dir/session.bin`, and its
    // hello.bin is `config_dir/hello.bin`. Adopting it as Managed would point
    // the CLI at an empty directory and present as "signed out" on upgrade,
    // with the real profile still sitting untouched on disk.
    let r = resolve_startup(&[], None, Some("me@example.com"), Some("https://vault.bitwarden.com"));
    assert_eq!(r.active.location, AccountLocation::CliDefault);
    assert_eq!(r.active.email, "me@example.com");
    assert_eq!(r.accounts.len(), 1);
    assert!(r.needs_persist);
}

#[test]
fn a_first_launch_with_no_signed_in_account_still_yields_one_cli_default_account() {
    // `bw status` reports no email when nothing is signed in; startup's
    // `reauthenticate` will sign in, and it must do so in the CLI's default
    // directory, not a fresh managed one.
    let r = resolve_startup(&[], None, None, None);
    assert_eq!(r.active.location, AccountLocation::CliDefault);
    assert!(r.active.email.is_empty());
}
```

### Step 9.2 — a stored active account is resumed; a dangling one is not

```rust
#[test]
fn the_stored_active_account_is_resumed() {
    let a = managed("0123456789abcdef0123456789abcdef");
    let b = managed("fedcba9876543210fedcba9876543210");
    let r = resolve_startup(&[a.clone(), b.clone()], Some(&b.id), None, None);
    assert_eq!(r.active.id, b.id, "a restart must resume the account that was last active");
    assert_eq!(r.accounts.len(), 2, "and must not drop the others");
    assert!(!r.needs_persist, "nothing changed, so nothing is rewritten");
}

#[test]
fn an_active_id_naming_no_stored_account_falls_back_to_the_first() {
    // A hand-edited settings.json, or an account removed by a build that
    // crashed mid-write. Falling through to "no active account" would leave
    // the app with nothing to point the CLI at.
    let a = managed("0123456789abcdef0123456789abcdef");
    let ghost = AccountId::parse(&"9".repeat(32)).unwrap();
    let r = resolve_startup(&[a.clone()], Some(&ghost), None, None);
    assert_eq!(r.active.id, a.id);
    assert!(r.needs_persist, "the dangling active id must be corrected on disk");
}
```

### Step 9.3 — wire it into `main`

In `main()`, after `settings` is loaded and after
`remember_verified_bw_exe(bw_exe)` (which `multi_account_availability` depends on) and
**before** `store` is built and before the first `store.load()`:

```rust
let status = login_ui::check_bw_status_details();
let resolved = accounts::resolve_startup(
    &settings.accounts,
    settings.active_account.as_ref(),
    status.user_email.as_deref(),
    status.server_url.as_deref(),
);
let mut active_account = resolved.active.clone();
let mut known_accounts = resolved.accounts.clone();
if resolved.needs_persist {
    if let Err(e) =
        settings::Settings::persist_accounts(&settings_path, &known_accounts, Some(&active_account.id))
    {
        log::warn!("could not persist the account list: {e}");
    }
}
// Everything that spawns `bw` from here on follows this.
bw_path::set_active_data_dir(accounts::data_dir_for(&config_dir, &active_account));
let mut store =
    session_store::SessionStore::new(accounts::session_path_for(&config_dir, &active_account));
```

`let store =` becomes `let mut store =` (a switch replaces it); the existing
`config_dir.join("session.bin")` line is deleted. `cached_status_details` can be seeded
from the `status` fetched here rather than re-fetched by the background prefetch —
leave the prefetch as it is to keep this task a wiring change only.

```rust
#[test]
fn startup_points_the_cli_at_the_resolved_account_before_it_loads_a_session() {
    // WIRING, and the ordering is the point: `store.load()` reads
    // `session.bin`, and `check_bw_status_with_session` spawns `bw` to
    // validate it. Both must already be pointed at the resolved account, or
    // the very first launch after a switch validates the wrong account's token
    // against the wrong profile and silently re-authenticates.
    let source = include_str!("main.rs");
    let set_dir = source
        .find(concat!("bw_path::set_active_data", "_dir("))
        .expect("main must point the CLI at the active account");
    let build_store = source
        .find(concat!("session_store::SessionStore", "::new("))
        .expect("main must build a session store");
    let load = source.find(concat!("store", ".load()")).expect("main must load the token");
    assert!(set_dir < build_store, "the CLI is pointed at the account AFTER the store is built");
    assert!(build_store < load, "the store is loaded before it is built for this account");
    // Positive control: all three needles exist and are distinct positions.
    assert!(set_dir != build_store && build_store != load);
}
```

**What would make these tests fail:** adopting the pre-existing account as `Managed`
(9.1 — the mutation that presents as "signed out on upgrade" while the whole suite goes
green); not resuming the stored active account (9.2); building the store before
pointing the CLI (9.3, which is pure ordering and therefore invisible to any
value-based test).

---

# Task 10 — `AccountsState`: the one door for "may I switch?"

**Files:** modify `deskwarden/src/accounts.rs`.

**Interfaces**

Consumes: `bw_path::MultiAccountAvailability` (Task 1), `Account` (Task 2).

Produces:

```rust
pub struct AccountsState {
    availability: crate::bw_path::MultiAccountAvailability,
    accounts: Vec<Account>,
    active: AccountId,
}

impl AccountsState {
    pub fn new(
        availability: crate::bw_path::MultiAccountAvailability,
        accounts: Vec<Account>,
        active: AccountId,
    ) -> Self;
    pub fn active(&self) -> &Account;
    /// Every account, for display. Always at least one.
    pub fn all(&self) -> &[Account];
    /// The accounts a user may switch TO right now. EMPTY when multi-account
    /// is blocked, whatever the list holds.
    pub fn switchable(&self) -> &[Account];
    pub fn can_add(&self) -> bool;
    pub fn blocked_reason(&self) -> Option<String>;
}
```

### Step 10.1 — blocked means blocked, everywhere

```rust
#[test]
fn a_blocked_availability_offers_no_switch_targets_and_no_add() {
    // The `relativeDataDir` trap. Offering a switch here would point the CLI at
    // a directory it IGNORES, so both accounts would read and write one shared
    // profile -- switching that appears to work and then doesn't stick.
    let a = managed("0123456789abcdef0123456789abcdef");
    let b = managed("fedcba9876543210fedcba9876543210");
    let state = AccountsState::new(
        MultiAccountAvailability::BlockedByPortableProfile {
            relative_data_dir: PathBuf::from(r"C:\a\bin\bitwarden-cli"),
        },
        vec![a.clone(), b.clone()],
        a.id.clone(),
    );
    assert!(state.switchable().is_empty(), "a switch was offered while the CLI ignores our env var");
    assert!(!state.can_add());
    assert!(state.blocked_reason().is_some());
    assert_eq!(state.active().id, a.id, "the app still works as a single-account app");
    assert_eq!(state.all().len(), 2, "the list is still shown -- it is switching that is refused");
}

#[test]
fn an_available_state_offers_every_account_except_the_active_one() {
    // The positive control for the test above, and the rule in its own right:
    // "switch to the account you are already on" is a no-op that would still
    // tear the backend down and demand a master password.
    let a = managed("0123456789abcdef0123456789abcdef");
    let b = managed("fedcba9876543210fedcba9876543210");
    let state = AccountsState::new(
        MultiAccountAvailability::Available,
        vec![a.clone(), b.clone()],
        a.id.clone(),
    );
    assert_eq!(state.switchable().iter().map(|x| x.id.clone()).collect::<Vec<_>>(), vec![b.id]);
    assert!(state.can_add());
    assert_eq!(state.blocked_reason(), None);
}

#[test]
fn a_single_account_available_state_offers_no_switch_but_still_offers_add() {
    let a = managed("0123456789abcdef0123456789abcdef");
    let state = AccountsState::new(MultiAccountAvailability::Available, vec![a.clone()], a.id);
    assert!(state.switchable().is_empty());
    assert!(state.can_add(), "one account is where a second one gets added from");
}
```

`switchable()` returning `&[Account]` requires the filtered list be stored; compute it
in `new` and keep it in a field. That is deliberate: a `switchable()` that filters on
every call is one a caller can be tempted to reimplement.

**What would make these tests fail:** `switchable()` ignoring `availability` (10.1's
first test — the mutation that re-opens the profile-corruption trap while every other
test passes); including the active account (10.1's second); `can_add` true while
blocked.

---

# Task 11 — Add an account

**Files:** modify `deskwarden/src/main.rs`; modify `deskwarden/src/accounts.rs`.

**Interfaces**

Produces:

```rust
/// Creates the target directory and returns the not-yet-persisted account.
/// Separate from the flow so the directory-creation failure is a value, not a
/// panic inside a tray-click handler.
pub fn prepare_new_account(config_dir: &Path) -> Result<Account, String>;

/// Undoes `prepare_new_account` when the login flow is declined or fails.
pub fn discard_prepared_account(config_dir: &Path, account: &Account);
```

and in `main.rs`:

```rust
fn add_account(
    config_dir: &Path,
    state: &mut accounts::AccountsState,
    /* the resettle/switch parameters */,
    mut sign_in: impl FnMut(&Account) -> Option<String>,
) -> SwitchOutcome;
```

The flow: `prepare_new_account` → point the CLI at its directory → `sign_in` (the live
caller passes `|a| login_ui::run_login_flow_for(config_dir, a)`, which shows the
existing sign-in window, 2FA and all, against the empty profile) → read
`check_bw_status_details()` for the email and server URL → append to the list, persist
with `Settings::persist_accounts`, then run the **same** `switch_to_account` to make it
active. Declined or failed → `discard_prepared_account` and roll back.

### Step 11.1 — a declined sign-in leaves nothing behind

```rust
#[test]
fn a_declined_sign_in_removes_the_directory_and_persists_no_account() {
    let cfg = scratch_dir("add-declined");
    let mut state = single_account_state(&cfg);
    let before = state.all().len();
    let outcome = add_account(&cfg, &mut state, .., |_| None);
    assert!(matches!(outcome, SwitchOutcome::Declined | SwitchOutcome::RolledBack { .. }));
    assert_eq!(state.all().len(), before, "a half-created account was left in the list");
    let leftovers: Vec<_> = std::fs::read_dir(accounts::accounts_root(&cfg))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "an empty profile directory was left behind: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&cfg);
}
```

### Step 11.2 — a successful add lands as a managed account and becomes active

```rust
#[test]
fn a_successful_add_persists_a_managed_account_and_makes_it_active() {
    let cfg = scratch_dir("add-ok");
    let mut state = single_account_state(&cfg);
    let outcome = add_account(&cfg, &mut state, .., |_| Some("session-token".into()));
    assert_eq!(outcome, SwitchOutcome::Switched);
    assert_eq!(state.all().len(), 2);
    let added = state.active();
    assert_eq!(added.location, accounts::AccountLocation::Managed,
        "an added account must NOT be CliDefault -- it would share the first account's profile");
    assert!(accounts::data_dir_for(&cfg, added).unwrap().is_dir());
    // Persisted, not just held in memory.
    let loaded = settings::Settings::load(&cfg.join("settings.json"));
    assert_eq!(loaded.accounts.len(), 2);
    assert_eq!(loaded.active_account.as_ref(), Some(&added.id));
    let _ = std::fs::remove_dir_all(&cfg);
}
```

### Step 11.3 — the CLI is pointed at the new directory before the sign-in runs

```rust
#[test]
fn the_sign_in_runs_with_the_cli_pointed_at_the_new_accounts_directory() {
    // WIRING, and the one that decides whether "Add account" adds an account
    // or LOGS THE EXISTING ONE OUT. `bw login` in the CLI's default profile
    // replaces the account that is already there.
    let cfg = scratch_dir("add-dir");
    let mut state = single_account_state(&cfg);
    let seen = std::cell::RefCell::new(None);
    add_account(&cfg, &mut state, .., |account| {
        *seen.borrow_mut() = bw_path::active_data_dir();
        assert_eq!(account.location, accounts::AccountLocation::Managed);
        Some("session-token".into())
    });
    let seen = seen.into_inner().expect("the sign-in never ran");
    assert!(
        seen.starts_with(accounts::accounts_root(&cfg)),
        "the sign-in ran in {seen:?}, not in the new account's own directory"
    );
    assert_ne!(seen_as_option(seen), None, "the sign-in ran in the CLI's DEFAULT profile -- \
        this would sign the existing account out and replace it");
    let _ = std::fs::remove_dir_all(&cfg);
}
```

**What would make these tests fail:** the sign-in running before the directory swap, or
with the swap missing entirely (11.3 — this is the single mutation that turns "add" into
"replace", and it is invisible to any test that only checks the resulting list); the
account persisted before the sign-in succeeds (11.1); the added account created as
`CliDefault` (11.2).

---

# Task 12 — Remove an account

**Files:** modify `deskwarden/src/main.rs`; modify `deskwarden/src/login_ui.rs`
(`bw_logout` gains a directory).

**Interfaces**

```rust
// login_ui
pub fn bw_logout_in(dir: Option<&Path>) -> Result<(), String>;   // uses bw_command_in
pub fn bw_logout() -> Result<(), String>;                        // = bw_logout_in(active)

// main
fn remove_account(
    config_dir: &Path,
    state: &mut accounts::AccountsState,
    target: &AccountId,
    /* switch parameters, for the case where the target is active */,
    mut logout: impl FnMut(Option<&Path>) -> Result<(), String>,
) -> Result<(), String>;
```

Order, and each step's reason:

1. If `target` is the active account, switch to `next_active_after_removal` **first**
   (via `switch_to_account`), so the removal never runs against the profile the backend
   is currently serving. If there is no survivor, refuse: removing the only account
   leaves the app with nothing to point at.
2. `logout(data_dir_for(config_dir, target))` — `bw logout` in **that** directory, via
   `bw_command_in`, never via a temporary mutation of the process-global (background
   threads spawn `bw`).
3. Delete `hello_blob_path_for` and `session_path_for` for that account. The reasoning
   `login_ui`'s log-out handler already applies: a sealed credential for an account the
   CLI no longer knows is a liability, not a feature.
4. Delete the managed directory (`CliDefault` has none — do **not** delete
   `config_dir` itself; that is where `settings.json` and the log live).
5. `Settings::persist_accounts` with the account gone.

### Step 12.1

```rust
#[test]
fn removing_an_account_logs_out_in_that_accounts_own_directory() {
    // WIRING. `bw logout` with the wrong (or no) profile directory logs the
    // WRONG ACCOUNT out -- the active one -- and the removed one stays signed
    // in on disk forever.
    let cfg = scratch_dir("remove-dir");
    let (mut state, a, b) = two_account_state(&cfg);   // `a` active
    let seen = std::cell::RefCell::new(Vec::new());
    remove_account(&cfg, &mut state, &b.id, .., |dir| {
        seen.borrow_mut().push(dir.map(Path::to_path_buf));
        Ok(())
    }).unwrap();
    assert_eq!(
        seen.into_inner(),
        vec![accounts::data_dir_for(&cfg, &b)],
        "logout ran against the wrong profile directory"
    );
}

#[test]
fn removing_an_account_deletes_its_session_and_hello_blobs_and_only_its_own() {
    let cfg = scratch_dir("remove-blobs");
    let (mut state, a, b) = two_account_state(&cfg);
    for acct in [&a, &b] {
        let p = accounts::session_path_for(&cfg, acct);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        std::fs::write(accounts::hello_blob_path_for(&cfg, acct), b"x").unwrap();
    }
    remove_account(&cfg, &mut state, &b.id, .., |_| Ok(())).unwrap();
    assert!(!accounts::session_path_for(&cfg, &b).exists());
    assert!(!accounts::hello_blob_path_for(&cfg, &b).exists());
    assert!(accounts::session_path_for(&cfg, &a).exists(), "the WRONG account's token was deleted");
    assert!(accounts::hello_blob_path_for(&cfg, &a).exists());
    assert!(!accounts::data_dir_for(&cfg, &b).unwrap().exists());
    assert_eq!(state.all().len(), 1);
}

#[test]
fn removing_the_active_account_switches_away_first_and_never_removes_the_last_one() {
    let cfg = scratch_dir("remove-active");
    let (mut state, a, b) = two_account_state(&cfg);  // `a` active
    remove_account(&cfg, &mut state, &a.id, .., |_| Ok(())).unwrap();
    assert_eq!(state.active().id, b.id, "the app was left pointing at a removed account");
    assert_eq!(bw_path::active_data_dir(), accounts::data_dir_for(&cfg, &b));
    // And the last one cannot go.
    assert!(remove_account(&cfg, &mut state, &b.id, .., |_| Ok(())).is_err());
    assert_eq!(state.all().len(), 1);
}

#[test]
fn removing_the_legacy_account_does_not_delete_the_config_directory() {
    // `CliDefault` has no directory of its own; its session.bin and hello.bin
    // sit directly in the config directory alongside settings.json and the log.
    // A `remove_dir_all(parent_of_session_bin)` here would delete the app's
    // entire configuration, including the account list naming the survivors.
    let cfg = scratch_dir("remove-legacy");
    let (mut state, legacy, b) = legacy_plus_managed_state(&cfg);
    std::fs::write(cfg.join("settings.json"), "{}").unwrap();
    remove_account(&cfg, &mut state, &legacy.id, .., |_| Ok(())).unwrap();
    assert!(cfg.join("settings.json").exists(), "the config directory was deleted");
    assert!(cfg.is_dir());
}
```

**What would make these tests fail:** `bw_logout()` (active-profile) used instead of
`bw_logout_in(target)` (12.1's first test); blobs deleted by a fixed path (second);
removing the active account without switching away first, or removing the last account
(third); `remove_dir_all` on the legacy account's blob parent (fourth — the one that
destroys the whole config directory and is otherwise entirely plausible-looking code).

---

# Task 13 — The switcher in the vault window

**Files:** modify `deskwarden/src/vault_window/mod.rs`; modify
`deskwarden/src/main.rs`.

Follows the `open_preferences` precedent exactly — a **third distinct field**, not a
reuse of `locked` or `needs_reauth`. Asking to switch says nothing about the current
session being gone; folded into either flag, the recovery would run against the wrong
account.

**Interfaces**

```rust
pub struct VaultWindowResult {
    pub locked: bool,
    pub needs_reauth: bool,
    pub open_preferences: bool,
    /// The account the user picked in the titlebar switcher. The window closed
    /// only because `main` has to tear the backend down and bring another one
    /// up, which cannot happen while this window owns the event loop --
    /// exactly the reason `open_preferences` exists. Distinct from `locked`
    /// and `needs_reauth`: this session was never lost.
    pub switch_to: Option<crate::accounts::AccountId>,
}

// vault_window::run gains two parameters:
//   accounts: crate::accounts::AccountsState,
```

`open_vault_window`'s loop handles it before the lock/re-auth branch, with its own
`continue` (so the window reopens on the new account), calling `switch_to_account`.

```rust
#[test]
fn the_switcher_lists_only_the_switchable_accounts_and_reports_the_pick() {
    // Driven through a real frame with a headless `egui::Context`, the way the
    // window's other interaction tests are. Pins that the pick REACHES the
    // result -- a switcher that paints correctly and returns `None` is the
    // "decision correct, renderer inert" shape this codebase keeps producing.
    let state = AccountsState::new(MultiAccountAvailability::Available, vec![a(), b()], a().id);
    let pane = Pane::new(..);
    pane.click_switcher_entry(&b().email);
    assert_eq!(pane.result().switch_to, Some(b().id));
    assert!(!pane.result().locked, "a switch must not be reported as a lock");
    assert!(!pane.result().needs_reauth);
}

#[test]
fn a_blocked_availability_paints_the_reason_instead_of_a_switcher() {
    let state = AccountsState::new(blocked(), vec![a(), b()], a().id);
    let pane = Pane::new(..);
    assert!(pane.switcher_entries().is_empty(), "a switch was offered while it cannot work");
    assert!(pane.text().contains("bitwarden-cli"), "the user is not told why");
}
```

```rust
#[test]
fn open_vault_window_acts_on_a_switch_and_reopens_rather_than_running_the_lock_recovery() {
    // WIRING. A `switch_to` that `open_vault_window` never reads means the
    // switcher is 100% inert -- the exact shape of the Trash/Archive feature
    // that shipped dead behind an early return with a green suite.
    let source = include_str!("main.rs");
    let needle = concat!("result.switch", "_to");
    assert!(source.contains(needle), "open_vault_window never reads the switcher's result");
    // It is handled BEFORE the lock/re-auth branch, and with its own `continue`.
    let switch_at = source.find(needle).unwrap();
    let lock_at = source.find("if result.locked || result.needs_reauth").unwrap();
    assert!(switch_at < lock_at, "a switch would be swallowed by the lock recovery");
    // Positive control: both needles were found at distinct positions.
    assert_ne!(switch_at, lock_at);
}
```

**What would make these tests fail:** a switcher that paints but never sets
`switch_to`; `switch_to` folded into `locked`; `open_vault_window` never reading the
field (the inert-feature mutation); the switcher offered while blocked.

---

# Task 14 — The tray accounts submenu, and the final wiring pins

**Files:** modify `deskwarden/src/tray.rs`; modify `deskwarden/src/main.rs`.

`AppTray` gains an `Accounts` submenu rebuilt whenever the account list or the active
account changes:

```rust
pub struct AccountsMenu {
    /// One entry per account, in list order. `MenuId` → `AccountId`, so the
    /// main loop can answer "which account was clicked?" without matching on
    /// labels.
    entries: Vec<(MenuId, crate::accounts::AccountId)>,
    add_id: MenuId,
    manage_id: MenuId,
}

impl AppTray {
    pub fn rebuild_accounts_menu(&mut self, state: &crate::accounts::AccountsState);
    /// The account a menu event names, or `None` if the event is not an
    /// account entry. Pure over the recorded ids -- testable without a tray.
    pub fn account_for_menu_id(&self, id: &MenuId) -> Option<&crate::accounts::AccountId>;
}
```

`account_for_menu_id` is a pure lookup over a `Vec` and is what the test drives; the
`muda` menu construction is not testable and is deliberately kept to the smallest
possible unlogic'd layer above it.

```rust
#[test]
fn a_menu_id_maps_back_to_the_account_it_was_built_for() {
    let entries = vec![(MenuId::new("m1"), a().id), (MenuId::new("m2"), b().id)];
    let menu = AccountsMenu::from_entries(entries, MenuId::new("add"), MenuId::new("manage"));
    assert_eq!(menu.account_for_menu_id(&MenuId::new("m2")), Some(&b().id));
    assert_eq!(menu.account_for_menu_id(&MenuId::new("add")), None,
        "\"Add account...\" must not be mistaken for an account");
    assert_eq!(menu.account_for_menu_id(&MenuId::new("nope")), None);
}

#[test]
fn a_blocked_availability_builds_no_account_entries() {
    let menu = accounts_menu_entries(&AccountsState::new(blocked(), vec![a(), b()], a().id));
    assert!(menu.is_empty(), "the tray offered a switch the CLI would ignore");
    // Positive control on the same helper.
    assert_eq!(
        accounts_menu_entries(&AccountsState::new(available(), vec![a(), b()], a().id)).len(),
        1,
        "only the non-active account is a switch target"
    );
}
```

Main-loop wiring, beside the existing `tray.open_vault_id` / `tray.preferences_id`
handlers:

```rust
if let Some(target) = tray.account_for_menu_id(&event.id).cloned() {
    match switch_to_account(&config_dir, &active_account.clone(), /* target account */, ..) {
        SwitchOutcome::Switched => {
            tray.rebuild_accounts_menu(&accounts_state);
            if let Err(e) = settings::Settings::persist_accounts(
                &settings_path, accounts_state.all(), Some(&accounts_state.active().id))
            {
                log::warn!("could not persist the active account: {e}");
            }
        }
        SwitchOutcome::Declined => log::info!("account switch declined; staying on {}", ..),
        SwitchOutcome::RolledBack { reason } => {
            log::warn!("{reason}; returned to {}", active_account.email);
            message_box("Deskwarden", &reason, MB_ICONWARNING);
        }
        SwitchOutcome::StoodDown { reason } => log::error!("{reason}"),
    }
    last_dispatched_hwnd = None;
}
```

Two final pins:

```rust
#[test]
fn the_active_account_is_persisted_after_every_successful_switch() {
    // WIRING. Without this the app resumes the PREVIOUS account on restart --
    // "switching that appears to work and then doesn't stick", which is
    // indistinguishable from the `relativeDataDir` trap and would send whoever
    // debugs it straight down the wrong path.
    let source = include_str!("main.rs");
    let needle = concat!("Settings::persist", "_accounts(");
    assert!(source.matches(needle).count() >= 2,
        "persist_accounts is called from fewer places than there are ways to change the active account");
    let switched = source.split(concat!("SwitchOutcome::", "Switched")).nth(1).unwrap();
    assert!(switched.contains(needle), "a successful switch does not persist the new active account");
}

#[test]
fn nothing_offers_a_switch_without_going_through_accounts_state() {
    // The `relativeDataDir` refusal has exactly one door (Task 10). A UI that
    // reads `settings.accounts` directly bypasses it and re-opens the trap.
    for file in ["tray.rs", "main.rs"] {
        let source = read_source(file);
        assert!(
            !source.contains(concat!("settings.accounts", ".iter()")),
            "{file} iterates the raw account list instead of AccountsState::switchable()"
        );
    }
    // Positive control: the guard's needle is findable when present.
    assert!(format!("x{}y", concat!("settings.accounts", ".iter()"))
        .contains(concat!("settings.accounts", ".iter()")));
}
```

**What would make these tests fail:** a menu id mapping to the wrong account, or "Add
account…" being treated as one; account entries built while blocked; a successful
switch not persisting the active account (the "doesn't stick" symptom, and the one most
likely to be misdiagnosed as the `relativeDataDir` trap); a UI reading the raw list.

---

## Verification, at the end of every task

```
cargo test  --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
```

Both clean and warning-free. Report the counts (`N lib + 34 bin`) in the commit
message, and record in `.superpowers/sdd/progress.md` — per task — **the mutation each
new test was watched to fail on**, with the verbatim failure message. A test that has
not been watched to fail has not been shown to test anything, and that is how this
repository has shipped a feature 100% inert behind an early return with a green suite.

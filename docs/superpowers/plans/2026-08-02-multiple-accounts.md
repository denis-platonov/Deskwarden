# Multiple accounts — implementation plan

**Spec:** `docs/superpowers/specs/2026-08-01-multiple-accounts-design.md` (approved, `3bb7615`).
**Baseline:** `1a0bea9` on `main`, tree clean, **880 lib + 34 bin pass, zero warnings**.

**Revision, 2026-08-03. Task 4 (migration) is REMOVED and the feature shipped without
it.** See Task 4 below for the record. In one line: `%APPDATA%\Bitwarden CLI` is a local
cache plus a session token — the vault lives on Bitwarden's servers — so losing it costs
a sign-in and no data, and every risk `migration.rs` was arranged around was a risk to a
regenerable cache. A first run under the per-account layout now mints an account, points
the CLI at it and lets the login window prompt once. The pre-existing profile is left
exactly where it is: never read, never copied, never deleted.

**Revision, 2026-08-02.** A first draft of this plan proposed leaving the pre-existing
account in the CLI's own default profile directory (`AccountLocation::CliDefault`), so
nothing had to be relocated. **That was put to the user with both costs stated and
overruled: all accounts are symmetric under `accounts/<id>/`, and the pre-existing
profile is migrated.** The user explicitly accepted that Windows Hello quick unlock has
to be re-enrolled as a consequence. Migration was Task 4 — superseded by the 2026-08-03
revision above, which keeps the symmetry and drops the copying.

The plan's other departure from the spec stands unchanged and was not challenged: **the
data directory is derived, never persisted** in `settings.json`.

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
it. Task 8 extracts it verbatim into `resettle_session(..)`, parameterised by exactly
one thing — the closure that produces the new session token. The lock/re-auth path
passes `|| Some(reauthenticate(..))`. A switch passes a closure that first points the
CLI at the target account's data directory and constructs that account's
`SessionStore`, then does the same. **Nothing else differs.**

> **If a task finds itself writing a second teardown-and-repopulate path, it has gone
> wrong.** There is exactly one `cache.clear()` + `try_start_backend` +
> `settle_vault_after_unlock` sequence in this crate after Task 8, and Task 9 must call
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

**All accounts are symmetric.** Every account lives at
`<config_dir>\accounts\<account-id>\`, holds its own `session.bin` and `hello.bin`, and
is reached with `BITWARDENCLI_APPDATA_DIR` pointing at it. There is **no
`AccountLocation` enum** and no "the first one is special" variant — this project has
already had to delete one variant with no members this week, and the overruled draft
would have created another. A machine with no accounts set up is not an account variant;
it is the absence of an account list, which is a *startup* condition that ends the moment
the user signs in once.

**~~Migration copies, verifies, repoints, and only then deletes.~~ (Removed 2026-08-03.)**
There is no migration. `accounts::resolve_startup` answers from `settings.accounts`
alone: accounts present → resume the active one; none → mint one and let the login
window prompt. Its one account-less answer is `StartupAccounts::NoAccountList`, whose
only producer is a `settings.json` that exists and cannot be parsed — where an empty
list cannot be believed.

**Windows Hello.** One key credential (`deskwarden-quick-unlock`), shared, with
accounts separated by the KDF label's account suffix. `RequestCreateAsync(ReplaceExisting)`
is banned outright — it rotates the shared credential and destroys every other
account's enrolment. The per-account suffix is non-empty for every account including
the first, so a `hello.bin` written by a version before this feature cannot be opened by
any account — it is simply never read again, and the first-run login window
**tells the user their quick unlock has to be set up again**
(`login_ui::FIRST_RUN_NOTICE`) rather than letting them discover it when Hello stops
working.

**Availability is one object, not a scattered check.** `accounts::AccountsState` is
constructed from `(availability, records, active)` and reports no switch targets when
the `relativeDataDir` trap is in play. Every consumer — tray, vault-window switcher,
add-account flow — asks it. One door. *(It took a `migration_state` too until
2026-08-03; the door survives the second reason going away, because the first still has
to reach every window through one value.)*

## Tech stack

Rust 2021, Windows-only. `serde`/`serde_json` (settings),
`directories` (config dir), `getrandom` (account ids — already a dependency via
`hello.rs`), `aes-gcm`/`sha2` (hello), `windows` crate (DPAPI, Hello), `eframe`/`egui`
(windows), `tray-icon`, `mockito` (HTTP tests). No new dependencies.

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
  `fn data_dir_for(..)`, `fn resume_action(..)` exist so the answer can be asserted
  without a window and without a filesystem.
- **Source guards, where one is the only option, use `concat!`-split single-line
  needles with a positive control.** A needle written as one literal in a file the test
  `include_str!`s matches its own declaration and can never fail. A needle containing
  `\n` passes on an LF working tree and fails on a CRLF one; this repo has both.
- **~~Task 4 may not delete anything the app did not itself create, until after
  verification has passed.~~** Superseded 2026-08-03, and replaced by something
  stronger: **the app does not delete a Bitwarden profile it did not itself create at
  all.** The only `remove_dir_all` on a profile directory is
  `accounts::delete_account_dir`, reached from an account removal the user asked for and
  from `discard_prepared_account` undoing an add this app started.
- Line endings: leave files as you found them. Do not reflow or `cargo fmt` untouched
  regions.

---

# Task 1 — Detect the `relativeDataDir` trap

> **DONE — shipped as `39f8ef1`, 888 lib (880 + 8) + 34 bin pass.** The interface
> below is transcribed from what actually landed, not from the draft that was
> dispatched. One deliberate improvement the implementer made, recorded here so
> later tasks use it and no reader re-derives it: the impure half is
> `multi_account_availability_from_exe(Option<&Path>)`, taking the executable
> rather than reading `verified_bw_exe()` itself. The draft's live-probe test
> rebuilt that expression at the call site, which would have left the production
> `.exists()` unexercised — the decision tested and the wiring that reaches it
> not, this repo's most repeated defect. Later tasks call
> `multi_account_availability()`; nothing consumes it until Task 10.

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

/// The impure half, taking the exe so a test can drive it against a really
/// planted directory instead of rebuilding the expression at the call site.
pub fn multi_account_availability_from_exe(bw_exe: Option<&Path>) -> MultiAccountAvailability;

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
    assert_eq!(multi_account_from(None, false), MultiAccountAvailability::BlockedByUnknownCliPath);
    assert_eq!(multi_account_from(None, true), MultiAccountAvailability::BlockedByUnknownCliPath);
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
    let text = MultiAccountAvailability::BlockedByPortableProfile { relative_data_dir: dir }
        .explanation()
        .expect("a blocked state must say why");
    assert!(text.contains(r"C:\a\bin\bitwarden-cli"), "got: {text}");
    assert!(MultiAccountAvailability::BlockedByUnknownCliPath.explanation().is_some());
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

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AccountId(String);
// Deserialize is `#[serde(try_from = "String")]` so it goes through `parse`.

impl AccountId {
    pub fn generate() -> Self;                       // 32 lowercase hex chars
    pub fn parse(raw: &str) -> Option<Self>;         // rejects anything not 32 hex
    pub fn as_str(&self) -> &str;
}
impl std::fmt::Display for AccountId { .. }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub email: String,
    pub server_url: Option<String>,
}

pub fn accounts_root(config_dir: &Path) -> PathBuf;
pub fn data_dir_for(config_dir: &Path, id: &AccountId) -> PathBuf;
pub fn session_path_for(config_dir: &Path, id: &AccountId) -> PathBuf;
pub fn hello_blob_path_for(config_dir: &Path, id: &AccountId) -> PathBuf;
pub fn hello_kdf_suffix_for(id: &AccountId) -> Vec<u8>;
pub fn account_for<'a>(accounts: &'a [Account], id: &AccountId) -> Option<&'a Account>;
pub fn next_active_after_removal<'a>(accounts: &'a [Account], removed: &AccountId)
    -> Option<&'a Account>;
```

**There is no `AccountLocation`.** Every account is under `accounts/<id>/`; the
migration (Task 4) is what makes that true of the pre-existing one. The pre-migration
state is the *absence* of an account list, not an account with a special location — see
Task 11's `StartupAccounts::Unmigrated`. An enum variant that exists only to describe
"the one account that predates this feature" is a variant that will have no members the
moment migration lands, and this project has already had to delete one of those.

`data_dir_for` returns `PathBuf`, not `Option<PathBuf>` — there is no account whose
directory is "the CLI's own default". `bw_path::set_active_data_dir` keeps its
`Option<PathBuf>` parameter (Task 3), because `None` is still meaningful *before*
migration and *during* it, but it is not a property of any `Account`.

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
    // This id becomes a directory name, and Task 4 and Task 13 will
    // `remove_dir_all` a path built from it. A traversal or a device name
    // reaching `data_dir_for` would put -- or DELETE -- an account's profile
    // somewhere else entirely.
    for bad in ["..", "../evil", r"..\evil", "CON", "", "abc", "a@b.c",
                "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef0123456789abcde"] {
        assert!(AccountId::parse(bad).is_none(), "accepted {bad:?}");
    }
    // Positive control on the same function.
    assert!(AccountId::parse("0123456789abcdef0123456789abcdef").is_some());
    assert!(AccountId::parse(AccountId::generate().as_str()).is_some());
}

#[test]
fn a_hand_edited_settings_id_that_escapes_the_directory_does_not_deserialize() {
    assert!(serde_json::from_str::<AccountId>(r#""../..""#).is_err());
    assert!(serde_json::from_str::<AccountId>(r#""0123456789abcdef0123456789abcdef""#).is_ok());
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

### Step 2.2 — the paths, and the collision property

```rust
fn id(s: &str) -> AccountId { AccountId::parse(s).unwrap() }

#[test]
fn an_accounts_paths_all_live_under_its_own_directory() {
    let cfg = Path::new(r"C:\cfg");
    let a = id("0123456789abcdef0123456789abcdef");
    let dir = PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef");
    assert_eq!(accounts_root(cfg), PathBuf::from(r"C:\cfg\accounts"));
    assert_eq!(data_dir_for(cfg, &a), dir);
    assert_eq!(session_path_for(cfg, &a), dir.join("session.bin"));
    assert_eq!(hello_blob_path_for(cfg, &a), dir.join("hello.bin"));
}

#[test]
fn no_secret_of_any_account_lands_in_the_shared_config_directory() {
    // The pre-migration app kept `session.bin` and `hello.bin` directly in
    // `config_dir`. After migration nothing does -- if one account's blob
    // resolved back to the shared directory it would be found (and deleted, and
    // overwritten) by every other account.
    let cfg = Path::new(r"C:\cfg");
    for raw in ["0123456789abcdef0123456789abcdef", &"0".repeat(32), &"f".repeat(32)] {
        let a = id(raw);
        assert_ne!(session_path_for(cfg, &a), PathBuf::from(r"C:\cfg\session.bin"));
        assert_ne!(hello_blob_path_for(cfg, &a), PathBuf::from(r"C:\cfg\hello.bin"));
        assert!(session_path_for(cfg, &a).starts_with(accounts_root(cfg)));
        assert!(hello_blob_path_for(cfg, &a).starts_with(accounts_root(cfg)));
    }
}

#[test]
fn no_two_accounts_share_a_session_or_hello_path() {
    // The spec's own test.
    let cfg = Path::new(r"C:\cfg");
    let ids = [id("0123456789abcdef0123456789abcdef"), id("fedcba9876543210fedcba9876543210"),
               id(&"0".repeat(32)), id(&"f".repeat(32))];
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
```

```rust
pub fn accounts_root(config_dir: &Path) -> PathBuf {
    config_dir.join("accounts")
}

pub fn data_dir_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    accounts_root(config_dir).join(id.as_str())
}

pub fn session_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    data_dir_for(config_dir, id).join("session.bin")
}

pub fn hello_blob_path_for(config_dir: &Path, id: &AccountId) -> PathBuf {
    data_dir_for(config_dir, id).join("hello.bin")
}
```

### Step 2.3 — the KDF suffix, now unconditional

**Re-derived after the migration decision.** In the overruled draft the suffix had two
jobs: separate accounts under one shared Hello credential, *and* reproduce today's
derivation exactly for the un-migrated first account so its existing `hello.bin` kept
working. The second job is gone with migration. The first is untouched, and is the
entire reason `RequestCreateAsync(ReplaceExisting)` stays banned: one credential, N
accounts, separated by the label.

So the scheme **survives, and gets simpler**: the suffix is now unconditional — every
account, including the first, gets `b" account " ‖ id`. There is no empty-suffix case,
so there is no "is the empty case reachable?" question and no branch that exists only
to serve a decision that was overruled. The consequence is stated plainly rather than
discovered: **no pre-migration `hello.bin` can be opened by any account**, which is why
Task 4 deletes it and tells the user (Step 4.7).

```rust
#[test]
fn two_accounts_get_different_kdf_suffixes_and_none_is_empty() {
    let a = id("0123456789abcdef0123456789abcdef");
    let b = id("fedcba9876543210fedcba9876543210");
    assert_ne!(hello_kdf_suffix_for(&a), hello_kdf_suffix_for(&b));
    // Absolute, not incidental: an empty suffix would reproduce the
    // pre-migration derivation, so a stale hello.bin left behind by a failed
    // migration would silently open under the migrated account's identity.
    assert!(!hello_kdf_suffix_for(&a).is_empty());
    assert!(!hello_kdf_suffix_for(&b).is_empty());
    // And it carries the id, so it cannot be a constant.
    assert!(hello_kdf_suffix_for(&a).ends_with(a.as_str().as_bytes()));
}
```

```rust
/// Mixed into `hello`'s existing domain-separation label so ONE Windows Hello
/// credential seals a distinct key per account.
///
/// Never empty, for every account including the first. An empty suffix would
/// reproduce the derivation used before this feature existed, which would mean
/// a `hello.bin` left over from before the migration could still be opened --
/// under whichever account happened to have the empty suffix. Quick unlock is
/// re-enrolled per account after migration; see `migration::migrate`, which
/// deletes the pre-migration blob and says so.
pub fn hello_kdf_suffix_for(id: &AccountId) -> Vec<u8> {
    let mut suffix = b" account ".to_vec();
    suffix.extend_from_slice(id.as_str().as_bytes());
    suffix
}
```

### Step 2.4 — lookups

```rust
#[test]
fn account_for_finds_by_id_and_misses_cleanly() {
    let list = vec![account("0123456789abcdef0123456789abcdef"), account(&"a".repeat(32))];
    let wanted = id("0123456789abcdef0123456789abcdef");
    assert_eq!(account_for(&list, &wanted).map(|a| a.id.clone()), Some(wanted));
    assert!(account_for(&list, &id(&"9".repeat(32))).is_none());
    assert!(account_for(&[], &AccountId::generate()).is_none());
}

#[test]
fn removing_the_active_account_falls_to_the_first_survivor_and_never_to_itself() {
    let a = account("0123456789abcdef0123456789abcdef");
    let b = account("fedcba9876543210fedcba9876543210");
    let list = vec![a.clone(), b.clone()];
    assert_eq!(next_active_after_removal(&list, &a.id).map(|x| x.id.clone()), Some(b.id.clone()));
    assert_eq!(next_active_after_removal(&list, &b.id).map(|x| x.id.clone()), Some(a.id.clone()));
    assert!(next_active_after_removal(&[a.clone()], &a.id).is_none(), "the last account");
}
```

**What would make these tests fail:** an id built from the email; `parse` accepting `..`
(the traversal test — which now guards two `remove_dir_all` call sites, not just a
create); any path resolving back into the shared config directory (2.2's second test);
an empty or constant KDF suffix (2.3); `next_active_after_removal` returning the account
being removed.

---

# Task 3 — Point the CLI at a chosen data directory

**Files:** modify `deskwarden/src/bw_path.rs`.

**Interfaces**

```rust
/// The environment variable the Bitwarden CLI reads its profile directory from.
pub const BW_DATA_DIR_ENV: &str = "BITWARDENCLI_APPDATA_DIR";

pub fn set_active_data_dir(dir: Option<PathBuf>);
pub fn active_data_dir() -> Option<PathBuf>;
pub fn bw_command_in(dir: Option<&Path>) -> Result<Command, String>;
// `bw_command()` keeps its exact signature and becomes
// `bw_command_in(active_data_dir().as_deref())`.

/// Where the CLI keeps its profile when neither `relativeDataDir` nor
/// `BITWARDENCLI_APPDATA_DIR` applies: `%APPDATA%\Bitwarden CLI`. Task 4's
/// migration source.
pub fn cli_default_data_dir() -> Option<PathBuf>;
pub fn cli_default_data_dir_from(appdata: Option<&OsStr>) -> Option<PathBuf>;
```

`Option<PathBuf>` on `set_active_data_dir` stays even though every `Account` now has a
real directory: `None` is what the app runs with *before* migration, and Task 4 needs
`bw_command_in(None)` to read the source profile and `bw_command_in(Some(new))` to
verify the copy — without ever touching the global, which background threads read.

### Step 3.1 — the env var is set, and unset means unset

```rust
#[test]
fn a_command_built_for_a_directory_carries_the_appdata_env_var() {
    let dir = PathBuf::from(r"C:\cfg\accounts\0123456789abcdef0123456789abcdef");
    let Ok(cmd) = bw_command_in(Some(&dir)) else { return }; // skipped without a verified exe
    let found: Vec<_> = cmd.get_envs()
        .filter(|(k, _)| *k == std::ffi::OsStr::new(BW_DATA_DIR_ENV)).collect();
    assert_eq!(found.len(), 1, "the CLI reads exactly one profile-directory variable");
    assert_eq!(found[0].1, Some(dir.as_os_str()));
}

#[test]
fn a_command_built_for_the_cli_default_sets_no_appdata_env_var_at_all() {
    // NOT "sets it to empty": an empty `BITWARDENCLI_APPDATA_DIR` is a
    // different thing to the CLI than an absent one. Task 4 reads the SOURCE
    // profile through this exact form, so getting it wrong means migrating
    // from the wrong directory -- or from nothing.
    let Ok(cmd) = bw_command_in(None) else { return };
    assert!(cmd.get_envs().all(|(k, _)| k != std::ffi::OsStr::new(BW_DATA_DIR_ENV)));
}
```

### Step 3.2 — the process-global, and `bw_command` reading it

```rust
#[test]
fn bw_command_follows_the_active_data_dir() {
    let _guard = ACTIVE_DIR_LOCK.lock().unwrap();
    let dir = PathBuf::from(r"C:\cfg\accounts\fedcba9876543210fedcba9876543210");
    set_active_data_dir(Some(dir.clone()));
    if let Ok(cmd) = bw_command() {
        assert_eq!(
            cmd.get_envs().find(|(k, _)| *k == std::ffi::OsStr::new(BW_DATA_DIR_ENV))
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

Every test that touches the process-global takes `ACTIVE_DIR_LOCK` (a `static
Mutex<()>` in the test module) first, so a parallel runner cannot interleave them.

### Step 3.3 — ~~the migration source~~ REMOVED 2026-08-03

> Superseded with Task 4. `bw_path::cli_default_data_dir{,_from}` are deleted: the app
> no longer has code that can name the CLI's own default profile directory, which is
> what "leave the pre-existing profile alone" means in practice.

```rust
#[test]
fn the_cli_default_profile_directory_is_bitwarden_cli_under_appdata() {
    assert_eq!(
        cli_default_data_dir_from(Some(std::ffi::OsStr::new(r"C:\Users\me\AppData\Roaming"))),
        Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\Bitwarden CLI")),
    );
    assert_eq!(cli_default_data_dir_from(None), None, "no APPDATA, no source to migrate from");
}
```

```rust
/// The CLI's own fallback profile location on Windows:
/// `path.join(process.env.APPDATA, "Bitwarden CLI")`.
///
/// **Task 4 treats this as a candidate, never as a fact.** If the directory
/// does not exist, or holds no `data.json`, there is simply nothing to migrate
/// -- the ordinary first-install case, handled as such rather than as an error.
/// Migration never deletes anything on the strength of this path alone; it
/// deletes only after the CLI itself has answered successfully against the copy.
pub fn cli_default_data_dir_from(appdata: Option<&OsStr>) -> Option<PathBuf> {
    appdata.map(|a| Path::new(a).join("Bitwarden CLI"))
}
```

### Step 3.4 — pin the wiring: nothing bypasses `bw_path`

```rust
#[test]
fn every_bw_spawn_in_the_crate_goes_through_bw_path() {
    // WIRING. A new call site building `Command::new(bw)` itself silently uses
    // whatever profile directory the CLI picks by default -- so a switched
    // account would keep answering from the previous one, with no error.
    let mut offenders = Vec::new();
    for entry in walk_rust_sources(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")) {
        if entry.ends_with("bw_path.rs") { continue; }       // the definition itself
        let text = std::fs::read_to_string(&entry).unwrap();
        let needle = concat!("Command", "::new(");
        for line in text.lines().filter(|l| l.contains(needle) && l.contains("bw")) {
            offenders.push(format!("{}: {}", entry.display(), line.trim()));
        }
    }
    assert!(offenders.is_empty(), "bw spawned outside bw_path:\n{}", offenders.join("\n"));
    // Positive control: the guard can actually see a violation.
    let planted = format!("let c = {}\"bw.exe\");", concat!("Command", "::new("));
    assert!(planted.contains(concat!("Command", "::new(")) && planted.contains("bw"));
}
```

**What would make these tests fail:** setting the env var to `""` for the default case
(3.1's second test — which now also breaks migration's source read); `bw_command`
ignoring the global (3.2 — the single mutation that makes the whole feature inert);
`cli_default_data_dir` naming the wrong directory (3.3); a new module spawning `bw`
directly (3.4, which carries its own positive control so the guard cannot be dead).

---

# Task 4 — ~~Migrate the pre-existing profile into `accounts/<id>/`~~ REMOVED 2026-08-03

**This task shipped and was then deleted, in full.** `deskwarden/src/migration.rs` and
every reference to it are gone; what stood here — the four rules, the marker and its
layout, the resume state machine, `copy_dir_all`, `verification_passed`, `rollback` and
the mutation log below them — described code that no longer exists, so it is not kept as
a description of the app. The record of *why* is kept, because the reasoning applies to
anything that might propose it again.

**The premise was wrong.** Everything here was arranged around "this is the one part of
the app that can destroy a vault profile". `%APPDATA%\Bitwarden CLI` is a local cache
plus a session token; the vault itself lives on Bitwarden's servers, and writes reach
them synchronously (verified live: a forced `POST /sync`, which pulls server state over
local, left an edited item intact). Losing that directory costs a sign-in. It costs no
data. Every one of the four Criticals three reviews found in this module — all of them
in the copy/verify/delete/resume machinery, the last a path that deleted the last copy
of a profile — was a risk to a regenerable cache, priced as a risk to a vault.

**The cost/benefit, plainly.** 1,273 production lines and more than half the feature's
test suite, in service of a 57-line switch, to save the user one sign-in. Removed, the
first launch under the per-account layout finds an empty account list, mints
`accounts/<id>/`, points `BITWARDENCLI_APPDATA_DIR` and the `SessionStore` at it, and
the ordinary login window asks once. `login_ui::FIRST_RUN_NOTICE` says why, and says the
Windows Hello enrolment has to be redone, so neither arrives unexplained.

**The pre-existing profile is left alone.** Not read, not copied, not deleted, not
offered for import. `bw_path::cli_default_data_dir` — the only function that could name
that directory — was removed with the module, so the app no longer has code that can
find it.

**What was kept.** Everything the migration was *not*: per-account `session.bin` and
`hello.bin`, the non-empty `hello_kdf_suffix_for`, the ban on
`RequestCreateAsync(ReplaceExisting)`, the `relativeDataDir` detection and the one door
that reports it, and the switch/add/remove wiring pins.

**The one protection that had to be replaced rather than dropped.**
`ResumeAction::AdoptUnclaimedAccount` scanned the accounts root for a directory holding a
profile that `settings.json` did not name, and its last live producer had nothing to do
with migration: `add_account` signed in (writing `accounts/B/data.json`), the switch
landed, and `persist_accounts` failed — leaving B signed in and unreferenced forever.
That is closed by ordering instead of by a scan: the account is written to
`settings.json` **before** the switch is attempted, so a failed write has nothing to undo
and the response is `discard_prepared_account`. Every remaining failure leaves an entry
whose directory is gone — which `ensure_account_dir` and one sign-in repair — and never a
directory nothing names. Pinned by
`a_failed_persist_strands_no_signed_in_profile`.

# Task 5 — Persist the account list

**Files:** modify `deskwarden/src/settings.rs`.

**Interfaces**

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

A **third** writer over disjoint fields, mirroring `persist_vault_window_geometry`.
`persist_preferences` must destructure the two new fields as `accounts: _,
active_account: _` — the compile error the destructuring produces is the mechanism that
forces this decision to be made out loud.

### Step 5.1 — an older file still parses

```rust
#[test]
fn a_file_written_before_accounts_existed_still_parses() {
    let path = temp_path("pre-accounts");
    std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 3}"#).unwrap();
    let loaded = Settings::load(&path);
    assert!(loaded.accounts.is_empty(), "an absent list is 'migration has not run yet'");
    assert_eq!(loaded.active_account, None);
    assert!(!loaded.keep_backend_running, "the fields it does carry still land");
    assert_eq!(loaded.auto_lock_minutes, 3);
    let _ = std::fs::remove_file(&path);
}
```

An empty `accounts` is exactly what Task 4 keys `accounts_already_configured: false`
off, so this is not only a compatibility guarantee — it is the migration trigger.
`a_partial_file_keeps_defaults_for_absent_fields` and every other existing settings test
must keep passing untouched.

### Step 5.2 — the list round-trips through the real file

```rust
#[test]
fn the_account_list_round_trips_through_settings_json() {
    let path = temp_path("accounts-round-trip");
    let a = accounts::AccountId::parse("0123456789abcdef0123456789abcdef").unwrap();
    let written = Settings {
        accounts: vec![
            accounts::Account { id: a.clone(), email: "work@example.com".into(),
                                server_url: Some("https://vault.example.com".into()) },
            accounts::Account { id: accounts::AccountId::parse(&"a".repeat(32)).unwrap(),
                                email: "me@example.com".into(), server_url: None },
        ],
        active_account: Some(a.clone()),
        ..Settings::default()
    };
    written.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("work@example.com"), "not in the file at all: {text}");
    assert!(!text.contains("AppData") && !text.to_lowercase().contains("data_dir"),
        "the DATA DIRECTORY is derived, never stored -- storing it makes a second source \
         of truth that can disagree with the first: {text}");
    assert!(!text.contains("password") && !text.contains("session"), "NO SECRETS: {text}");
    assert_eq!(Settings::load(&path), written);
    let _ = std::fs::remove_file(&path);
}
```

### Step 5.3 — three writers, disjoint fields, in every pairing

```rust
#[test]
fn persisting_accounts_keeps_every_preference_and_the_geometry() {
    let path = temp_path("accounts-preserve");
    Settings { keep_backend_running: false, auto_lock_minutes: 7, ..Settings::default() }
        .save(&path).unwrap();
    Settings::persist_vault_window_geometry(
        &path, WindowGeometry { x: 1, y: 2, width: 1000, height: 700 }).unwrap();
    let a = accounts::AccountId::parse(&"b".repeat(32)).unwrap();
    Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();

    let loaded = Settings::load(&path);
    assert!(!loaded.keep_backend_running, "persist_accounts clobbered a preference");
    assert_eq!(loaded.auto_lock_minutes, 7);
    assert_eq!(loaded.vault_window.map(|g| g.x), Some(1), "persist_accounts clobbered the geometry");
    assert_eq!(loaded.active_account, Some(a));
}

#[test]
fn persisting_preferences_from_a_stale_copy_keeps_the_account_list() {
    // The regression, in the order the app performs it: `main` loads `Settings`
    // once at startup; an account is added mid-session and written by
    // `persist_accounts`; the user then opens Preferences and changes the
    // auto-lock. A whole-struct save writes main's stale (empty) list back and
    // the added account VANISHES on next launch -- and with an empty list, the
    // NEXT startup thinks migration never ran and tries to migrate a source
    // directory that no longer exists. Same trap the geometry fell into, with a
    // far worse blast radius.
    let path = temp_path("prefs-preserve-accounts");
    let at_startup = Settings::load(&path);
    assert!(at_startup.accounts.is_empty());
    let a = accounts::AccountId::parse(&"c".repeat(32)).unwrap();
    Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();

    Settings { auto_lock_minutes: 10, ..at_startup }.persist_preferences(&path).unwrap();

    let loaded = Settings::load(&path);
    assert_eq!(loaded.accounts.len(), 1, "a preferences save deleted the account list");
    assert_eq!(loaded.active_account, Some(a));
    assert_eq!(loaded.auto_lock_minutes, 10, "and the preference itself must still land");
}

#[test]
fn persisting_accounts_wins_over_a_stale_list_in_the_file() {
    // The other direction, so the read-modify-write cannot be "fixed" into
    // merely ignoring the accounts.
    let path = temp_path("accounts-win");
    let (a, b) = (accounts::AccountId::parse(&"d".repeat(32)).unwrap(),
                  accounts::AccountId::parse(&"e".repeat(32)).unwrap());
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

**What would make these tests fail:** `persist_accounts` implemented as a whole-struct
`save`; `persist_preferences` writing `accounts` from a stale copy; a data directory
sneaking into the file (5.2's second assertion); an older file failing to parse.

---

# Task 6 — Per-account `session.bin` and `hello.bin`

**Files:** modify `deskwarden/src/hello.rs`; modify `deskwarden/src/session_store.rs`
(doc only — it already takes a `PathBuf`).

**Interfaces**

```rust
pub fn blob_path_for(config_dir: &Path, id: &AccountId) -> PathBuf;
pub fn state_for(config_dir: &Path, id: &AccountId) -> HelloState;
pub fn enroll_for(config_dir: &Path, id: &AccountId, master_password: &str) -> Result<(), String>;
pub fn unlock_password_for(config_dir: &Path, id: &AccountId) -> Result<Zeroizing<String>, String>;
pub fn unenroll_for(config_dir: &Path, id: &AccountId);
```

`blob_path()`, `state()`, `enroll()`, `unlock_password()` and `unenroll()` are deleted.

```rust
fn derive_key(signature: &[u8], account_suffix: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(KDF_LABEL);
    hasher.update(account_suffix);
    hasher.update(signature);
    Zeroizing::new(hasher.finalize().into())
}
```

### Step 6.1 — never `ReplaceExisting`

The credential is **shared across accounts**. `RequestCreateAsync(ReplaceExisting)`
rotates it, which changes the signature, which changes every derived key, which silently
destroys every other account's enrolment. The existing doc on `hello_derived_key`
justifies `ReplaceExisting` ("a stale credential from an abandoned enrollment has no
blob to pair with and is worthless") — **that justification is false once a second
account exists, and the comment must be corrected, not left standing.**

The correction must also cover the migration path, which is a new way to reach a stale
blob: after Task 4, a `hello.bin` from before the migration is unopenable **by
construction** (the suffix is non-empty for every account), so the old doc's premise —
"a stale credential is worthless, so replacing it is free" — is now wrong in both
halves. It is not free: it is the one action that would break every *other* account. A
stale blob is dealt with by deleting the blob, never by rotating the credential.

```rust
#[test]
fn hello_never_asks_windows_to_replace_the_shared_credential() {
    // WIRING, and unavoidably a source guard: `RequestCreateAsync` needs real
    // TPM-backed Hello hardware and a live user. `ReplaceExisting` rotates the
    // ONE credential every account's key is derived from -- enrolling account B
    // would silently destroy account A's enrolment, and A would find out at the
    // moment it next tried to unlock. Migration makes this worse, not better:
    // it is what leaves stale blobs around in the first place, and the answer to
    // a stale blob is to DELETE IT, never to rotate the credential.
    let source = include_str!("hello.rs");
    let banned = concat!("KeyCredentialCreationOption", "::ReplaceExisting");
    assert_eq!(source.matches(banned).count(), 1,
        "`{banned}` must appear ONCE and only inside this test's own needle");
    let required = concat!("KeyCredentialCreationOption", "::FailIfExists");
    assert!(source.contains(required), "enrolment must use {required}");
    // Positive control: the counting mechanism can see a second occurrence.
    assert_eq!(format!("{banned} {banned}").matches(banned).count(), 2);
}
```

```rust
/// `create` enrols rather than unlocks. **`FailIfExists`, never
/// `ReplaceExisting`** (which this used to pass): the credential named
/// [`CREDENTIAL_NAME`] is SHARED BY EVERY ACCOUNT -- accounts are separated by
/// the KDF suffix (`accounts::hello_kdf_suffix_for`), not by having a credential
/// each. Replacing it rotates the private key, which changes the signature,
/// which changes every derived key: enrolling a second account would silently
/// destroy the first one's enrolment.
///
/// The old justification for `ReplaceExisting` was "a stale credential from an
/// abandoned enrolment has no blob to pair with and is worthless". That is no
/// longer true in either half. A stale BLOB is dealt with by deleting the blob
/// (`unenroll_for`, and `migration::migrate` for the pre-migration one, which no
/// account's suffix can ever open). A stale CREDENTIAL is not worthless -- it is
/// the only credential every other account depends on.
///
/// `AlreadyExists` is therefore the NORMAL case for the second and every later
/// account, and falls through to opening the existing credential.
fn hello_derived_key(create: bool, account_suffix: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let name = HSTRING::from(CREDENTIAL_NAME);
    let result = if create {
        let created = KeyCredentialManager::RequestCreateAsync(
            &name, KeyCredentialCreationOption::FailIfExists).and_then(|op| op.get());
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

### Step 6.2 — one account's blob cannot open another's, and none opens the old way

```rust
#[test]
fn a_blob_sealed_for_one_account_does_not_open_for_another() {
    let signature = b"pretend hello signature";
    let (a, b) = (id("0123456789abcdef0123456789abcdef"), id("fedcba9876543210fedcba9876543210"));
    let key_a = derive_key(signature, &accounts::hello_kdf_suffix_for(&a));
    let key_b = derive_key(signature, &accounts::hello_kdf_suffix_for(&b));
    let sealed = seal(&key_a, b"account A master password").unwrap();
    assert!(unseal(&key_b, &sealed).is_err(), "account B opened account A's sealed password");
    // Positive control on the same blob.
    assert_eq!(unseal(&key_a, &sealed).unwrap().as_slice(), b"account A master password");
}

#[test]
fn no_account_reproduces_the_pre_migration_derivation() {
    // The pre-migration key was SHA-256(KDF_LABEL || signature) -- an empty
    // suffix. If any account reproduced it, a `hello.bin` that a FAILED
    // migration left in the config directory could be opened under that
    // account's identity. This replaces the draft's
    // `the_legacy_accounts_derivation_is_byte_for_byte_what_it_was_before`,
    // whose whole purpose disappeared with the no-migration decision -- the
    // assertion is now the exact opposite of what it used to be.
    let signature = b"pretend hello signature";
    let mut old = Sha256::new();
    old.update(KDF_LABEL);
    old.update(signature);
    let old: [u8; 32] = old.finalize().into();
    for raw in ["0123456789abcdef0123456789abcdef", &"0".repeat(32), &"f".repeat(32)] {
        let key = derive_key(signature, &accounts::hello_kdf_suffix_for(&id(raw)));
        assert_ne!(key.as_slice(), &old, "{raw} derives the pre-migration key");
    }
    // Positive control: the two derivations differ ONLY because of the suffix,
    // so this is testing the suffix and not some unrelated change to the KDF.
    assert_eq!(derive_key(signature, &[]).as_slice(), &old);
}
```

### Step 6.3 — paths and enrolment state follow the account

```rust
#[test]
fn hello_state_is_per_account() {
    let cfg = scratch_dir("hello-per-account");
    let (a, b) = (id("0123456789abcdef0123456789abcdef"), id("fedcba9876543210fedcba9876543210"));
    std::fs::create_dir_all(blob_path_for(&cfg, &a).parent().unwrap()).unwrap();
    std::fs::write(blob_path_for(&cfg, &a), b"not a real blob").unwrap();
    assert!(!blob_path_for(&cfg, &b).exists(), "B must not see A's enrolment");
    assert_ne!(blob_path_for(&cfg, &a), blob_path_for(&cfg, &b));
    unenroll_for(&cfg, &b);
    assert!(blob_path_for(&cfg, &a).exists(), "unenrolling B deleted A's blob");
    unenroll_for(&cfg, &a);
    assert!(!blob_path_for(&cfg, &a).exists());
    let _ = std::fs::remove_dir_all(&cfg);
}
```

`enroll_for` must `create_dir_all` the blob's parent before writing.

**What would make these tests fail:** `ReplaceExisting` surviving anywhere (6.1);
`derive_key` ignoring the suffix (6.2's first test); any account reproducing the
pre-migration derivation (6.2's second — the mutation that makes a leftover blob
openable); `unenroll_for` deleting a fixed path (6.3).

---

# Task 7 — A login flow that can be cancelled, and the Hello notice in it

`login_ui::run_login_flow()` ends with `std::process::exit(1)` when its window is
closed. Right for startup and lock/re-auth; **fatal** for a switch, which the spec
requires be declinable ("wrong master password: stay on the current account").

**Files:** modify `deskwarden/src/login_ui.rs`.

**Interfaces**

```rust
pub fn run_login_flow_for(
    config_dir: &Path,
    account: &Account,
    /// From `MigrationState::Completed`, carried through `AccountsState`.
    /// `true` shows a line beside the enrol checkbox saying quick unlock has to
    /// be set up again -- see Step 4.7. Not a message the window invents: the
    /// panel being SILENTLY ABSENT is exactly the failure to avoid.
    hello_needs_reenrolment: bool,
) -> Option<String>;

/// Unchanged behaviour for the two call sites that genuinely cannot continue
/// without a session: startup and the lock/re-auth recovery.
pub fn run_login_flow(config_dir: &Path, account: &Account, hello_needs_reenrolment: bool) -> String;

pub fn check_bw_status_details_in(dir: Option<&Path>) -> BwStatusDetails;
pub fn bw_logout_in(dir: Option<&Path>) -> Result<(), String>;
```

### Step 7.1 — split the exit out of the flow

```rust
/// The two callers that cannot continue without a session: startup, and the
/// lock/re-auth recovery. Everything else -- an account switch, adding an
/// account -- must use [`run_login_flow_for`] and handle `None`, because
/// declining to switch accounts is an ordinary gesture and must not kill an
/// already-running app. That distinction is the whole reason these are two
/// functions rather than one.
pub fn run_login_flow(config_dir: &Path, account: &Account, notice: bool) -> String {
    match run_login_flow_for(config_dir, account, notice) {
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
    assert_eq!(source.matches(needle).count(), 1);
    let wrapper = source.split_once("pub fn run_login_flow(").expect("must exist").1;
    assert!(wrapper.contains(needle), "the exit moved out of the wrapper");
    assert!(wrapper.len() < source.len(), "positive control: the split isolated a region");
}
```

### Step 7.2 — per-account Hello, and the first-run line

> Amended 2026-08-03. Per-account Hello is unchanged. The line is no longer
> `HELLO_REENROLMENT_NOTICE` carried out of a migration; it is
> `login_ui::FIRST_RUN_NOTICE`, gated on the account `resolve_startup` minted on this
> launch, and it says both why the app is asking for a master password and that quick
> unlock has to be set up again.

Replace the five `hello::` call sites with their `_for` equivalents. The
`LoginAction::LogOut` arm's `hello::unenroll()` becomes `hello::unenroll_for(config_dir,
&account.id)` — logging out of *this* account must not drop another's enrolment.

```rust
#[test]
fn the_login_window_uses_only_the_per_account_hello_entry_points() {
    let source = include_str!("login_ui.rs");
    for stale in [concat!("hello", "::unenroll()"), concat!("hello", "::state()"),
                  concat!("hello", "::unlock_password()"), concat!("hello", "::enroll(")] {
        assert!(!source.contains(stale), "{stale} is still called here");
    }
    for required in [concat!("hello", "::unenroll_for("), concat!("hello", "::state_for("),
                     concat!("hello", "::unlock_password_for("), concat!("hello", "::enroll_for(")] {
        assert!(source.contains(required), "{required} is not called here");
    }
    // The `enroll(` needle must not be satisfiable by `enroll_for(`, or the two
    // halves of this test would contradict each other and one would be inert.
    assert!(!concat!("hello", "::enroll_for(").contains(concat!("hello", "::enroll(")));
}

#[test]
fn the_hello_panel_says_quick_unlock_needs_reenrolling_after_a_migration() {
    // Driven through a real frame with a headless `egui::Context`, the way the
    // window's other tests are. Without this line the user's ONLY signal that
    // their quick unlock stopped working is the panel silently not being there
    // -- indistinguishable from Hello never having been set up. The user
    // accepted re-enrolling; they did not accept finding out this way.
    let pane = login_pane(/* hello_needs_reenrolment */ true);
    assert!(pane.text().to_lowercase().contains("set up again"), "got: {}", pane.text());
    // Positive control: not painted for a user who never enrolled.
    let quiet = login_pane(false);
    assert!(!quiet.text().to_lowercase().contains("set up again"));
}
```

**What would make these tests fail:** `process::exit` left in the body (7.1 — the
mutation that makes a declined switch fatal); a stale account-less `hello::` call
(7.2's first); the re-enrolment line dropped or painted unconditionally (7.2's second).

---

# Task 8 — Extract the resettle sequence (pure refactor)

No behaviour changes. The existing 880 lib + 34 bin tests are the safety net; in
particular `matches_from_the_pre_lock_account_do_not_survive_the_unlock`,
`a_backend_that_cannot_be_restarted_after_unlock_leaves_the_app_running`,
`a_readiness_timeout_after_unlock_leaves_the_app_running` and
`a_late_sync_from_a_previous_account_is_discarded_even_though_the_cache_refilled` all
cover this block and must pass **unedited**.

**Files:** modify `deskwarden/src/main.rs`.

**Interfaces**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResettleOutcome {
    /// The backend came up and the settle ran. (The settle itself may still have
    /// stood autofill down -- a survivable outcome the app already ships, not a
    /// reason to roll back a switch.)
    BackendStarted,
    /// `try_start_backend` failed, or nothing authenticated.
    /// `stand_down_after_unlock` has run, `bw_serve_child` is `None`, the engine
    /// is cleared.
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
    // THE ONLY THING THAT DIFFERS between a lock/re-auth and an account switch.
    // Runs AFTER the old backend is stopped and the cache is cleared, and BEFORE
    // the new backend is started -- which is the whole ordering that makes a
    // half-switched app unreachable.
    authenticate: impl FnOnce() -> Option<String>,
) -> ResettleOutcome;
```

### Step 8.1 — move the block, change nothing

Cut `main.rs` lines 1839–1951 into `resettle_session`, verbatim, substituting
`authenticate()` for `reauthenticate(store)`. Run the suite: **880 lib + 34 bin must
still pass, warning-free, with no test edited.** If any test needed editing, the
refactor was not a refactor.

### Step 8.2 — pin that there is one sequence

```rust
#[test]
fn there_is_exactly_one_teardown_and_repopulate_path() {
    // The spec's own warning, made mechanical: "if an implementation finds
    // itself writing a second teardown-and-repopulate path, that is the signal
    // it has gone wrong". A second call site is a second implementation of the
    // hardest code in this codebase, and it would not have these tests.
    let source = include_str!("main.rs");
    let needle = concat!("settle_vault_after", "_unlock(");
    let count = source.matches(needle).count();
    assert_eq!(count, 2,
        "expected the definition and exactly ONE call site (inside `resettle_session`); found {count}");
    let body = source.split_once(concat!("fn resettle", "_session(")).expect("must exist").1;
    assert!(body.contains(needle), "the settle moved out of `resettle_session`");
    assert!(body.len() < source.len());
    assert_eq!(format!("{needle} {needle}").matches(needle).count(), 2);
}
```

### Step 8.3 — a composition test for the new arm

```rust
#[test]
fn a_declined_authentication_starts_no_backend_and_leaves_the_cache_cleared() {
    let cache = Arc::new(VaultCache::new(VaultBridge::new("http://127.0.0.1:1")));
    let mut engine = MatchEngine::new();
    engine.rebuild(&[("id".into(), AppMatch { process: "prev.exe".into(),
                                              trigger: TriggerMode::Prompt })]);
    let outcome = resettle_session(/* .. */, || None);
    assert_eq!(outcome, ResettleOutcome::BackendNotStarted);
    assert!(engine.lookup("prev.exe").is_none(), "the previous account's matches survived");
    assert!(!cache.is_populated());
}
```

**What would make these tests fail:** a second `settle_vault_after_unlock` call site;
the settle lifted back out; `None` from `authenticate` proceeding to
`try_start_backend`; any behaviour change in the moved block (the 880 existing tests).

---

# Task 9 — The switch itself

**Files:** modify `deskwarden/src/main.rs`.

**Interfaces**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum SwitchOutcome {
    Switched,
    Declined,
    RolledBack { reason: String },
    /// The switch failed AND the rollback failed: `stand_down_after_unlock`'s
    /// state, which this app already ships and recovers from via the tray's Sync.
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
    /* the same parameters `resettle_session` takes */
    // Injected so the tests drive the whole composition without a window or a
    // backend -- the shape `settle_vault_after_unlock(.., probe)` already proved.
    mut resettle: impl FnMut(&Path, &Account) -> ResettleOutcome,
) -> SwitchOutcome;
```

### Step 9.1 — the order, which is the whole safety property

```rust
let previous_token = session_token.clone();
let previous_dir = bw_path::active_data_dir();

// 1. Point the CLI and the token store at the target, BEFORE anything
//    authenticates -- `run_login_flow_for` spawns `bw`, and it has to land in
//    the target's profile.
bw_path::set_active_data_dir(Some(accounts::data_dir_for(config_dir, &to.id)));
*store = session_store::SessionStore::new(accounts::session_path_for(config_dir, &to.id));

// 2. The existing sequence. `resettle_session` stops the old backend, clears the
//    cache (bumping the era, discarding any populate the PREVIOUS account still
//    has in flight), authenticates, starts the new backend, waits for readiness,
//    repopulates and rebuilds the engine.
match resettle(config_dir, to) {
    ResettleOutcome::BackendStarted => {
        *active_account = to.clone();
        // Only NOW is the outgoing token discarded. Doing it up front would make
        // a rollback cost the user a second password prompt for an account they
        // never asked to leave; and until the switch lands, the outgoing account
        // is still this app's account, not an idle one.
        let outgoing = accounts::session_path_for(config_dir, &from.id);
        if let Err(e) = std::fs::remove_file(&outgoing) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("could not discard the previous account's session token: {e}");
            }
        }
        SwitchOutcome::Switched
    }
    ResettleOutcome::BackendNotStarted => {
        // 3. Roll back. Point everything at the previous account and run THE
        //    SAME sequence, authenticating from the token we still hold.
        bw_path::set_active_data_dir(previous_dir);
        *store = session_store::SessionStore::new(
            accounts::session_path_for(config_dir, &from.id));
        match resettle(config_dir, from) {
            ResettleOutcome::BackendStarted => SwitchOutcome::RolledBack { .. },
            ResettleOutcome::BackendNotStarted => SwitchOutcome::StoodDown { .. },
        }
    }
}
```

The rollback's `resettle` in the live caller authenticates with
`|| Some(previous_token.clone())` — no prompt, because that session was never
invalidated. That is why `previous_token` is captured on the first line.

### Step 9.2 — the previous account's matches are GONE

```rust
#[test]
fn a_switch_rebuilds_the_engine_so_the_previous_accounts_matches_are_gone() {
    // The spec's own test, with the worst failure mode: a match left armed from
    // account A, under account B's session, raises an autofill prompt whose fill
    // can only ever end in an error -- or in a credential from the wrong vault.
    engine.rebuild(&[("a-item".into(),
        AppMatch { process: "notepad.exe".into(), trigger: TriggerMode::Auto })]);
    assert!(engine.lookup("notepad.exe").is_some(), "precondition");

    let outcome = switch_to_account(.., |_, account| {
        cache.clear();
        let items = items_for(account);          // B's vault: one match on `code.exe`
        let epoch = cache.epoch();
        engine.rebuild(&match_entries(&items));
        cache.populate_with(items, epoch).unwrap();
        ResettleOutcome::BackendStarted
    });

    assert_eq!(outcome, SwitchOutcome::Switched);
    assert!(engine.lookup("notepad.exe").is_none(),
        "account A's match is STILL ARMED under account B's session");
    // Positive control: not merely "the engine is empty".
    assert!(engine.lookup("code.exe").is_some(), "account B's own match is not armed");
    assert!(cache.items().iter().all(|i| i.id != "a-item"));
}
```

### Step 9.3 — a populate in flight across a switch is discarded

```rust
#[test]
fn a_populate_from_the_previous_account_in_flight_across_a_switch_is_discarded() {
    // The era machinery is what makes a switch safe, and this is the assertion
    // that it is actually being ROUTED THROUGH rather than bypassed.
    let a_epoch = cache.epoch();                 // captured before A's fetch
    let a_items = probe_items(&[("a-item", "notepad.exe")]);

    switch_to_account(.., |_, _| { cache.clear(); ResettleOutcome::BackendStarted });

    assert_eq!(cache.populate_with(a_items, a_epoch).unwrap(),
        PopulateOutcome::DiscardedStale,
        "account A's items were written into account B's cache");
    assert!(cache.items().is_empty());

    // Positive control: an epoch captured AFTER the switch is not discarded, so
    // this does not pass merely because `populate_with` always discards.
    let b_epoch = cache.epoch();
    assert_eq!(cache.populate_with(probe_items(&[("b-item", "code.exe")]), b_epoch).unwrap(),
        PopulateOutcome::Populated);
}
```

### Step 9.4 — a failed switch leaves the previous account fully working

```rust
#[test]
fn a_failed_switch_returns_to_the_previous_account_with_everything_working() {
    // "A half-switched app -- new data directory, old cache -- is the one
    // outcome that must not be reachable."
    let mut seen: Vec<Option<PathBuf>> = Vec::new();
    let outcome = switch_to_account(config_dir, &a, &b, .., |_cfg, account| {
        seen.push(bw_path::active_data_dir());   // AT THE MOMENT the sequence runs
        if account.id == b.id { ResettleOutcome::BackendNotStarted }
        else { /* A's rollback: repopulate and rearm as the live path does */
               ResettleOutcome::BackendStarted }
    });

    assert!(matches!(outcome, SwitchOutcome::RolledBack { .. }));
    assert_eq!(bw_path::active_data_dir(), Some(accounts::data_dir_for(config_dir, &a.id)),
        "the CLI was left on B's directory beside A's cache");
    assert_eq!(active_account.id, a.id);
    assert!(cache.is_populated());
    assert!(engine.lookup("notepad.exe").is_some(), "A's autofill is dead after a failed switch");
    assert!(accounts::session_path_for(config_dir, &a.id).exists(),
        "A's token was discarded, so the rollback cost a second password prompt");
    // The wiring pin: the directory really was swapped BEFORE the sequence ran.
    assert_eq!(seen, vec![Some(accounts::data_dir_for(config_dir, &b.id)),
                          Some(accounts::data_dir_for(config_dir, &a.id))]);
}
```

### Step 9.5 — a switch never kills the app

```rust
#[test]
fn no_switch_path_can_reach_the_fatal_startup_error() {
    // The spec's explicit warning. `start_backend` -- startup's wrapper around
    // `try_start_backend` -- calls `fatal_startup_error`; killing the app
    // because the OTHER account's backend would not start is not acceptable.
    let source = include_str!("main.rs");
    let body = source.split_once(concat!("fn switch_to", "_account(")).expect("must exist").1
        .split_once("\n}\n").expect("brace-terminated at column 0").0;
    for banned in [concat!("fatal_startup", "_error("), concat!("start_backend", "(")] {
        assert!(!body.contains(banned),
            "`{banned}` is reachable from a switch -- a failed switch would kill a running app");
    }
    assert!(!body.is_empty(), "positive control: the region is non-empty");
    assert!(source.contains(concat!("fatal_startup", "_error(")));
    assert!(source.contains(concat!("start_backend", "(")));
}
```

The `start_backend(` needle also matches `try_start_backend(` — deliberately and
correctly: the switch must go through `resettle_session`, which is the only thing that
may call either.

**What would make these tests fail:** the data directory swapped *after* the sequence,
or not at all (9.4's `seen` assertion — the single mutation that makes the feature inert
and is invisible to any end-state check); the engine merged rather than rebuilt (9.2);
the era bypassed (9.3); no rollback (9.4); the outgoing token deleted early (9.4);
`fatal_startup_error` reachable (9.5).

---

# Task 10 — `AccountsState`: the one door for "may I switch?"

> Amended 2026-08-03. `new` takes `(availability, records, active)`: the
> `MigrationState` input and the `hello_needs_reenrolment` field went with Task 4. The
> `relativeDataDir` half — and the rule that every window asks this and nothing else —
> is unchanged.

**Files:** modify `deskwarden/src/accounts.rs`.

**Interfaces**

```rust
pub struct AccountsState { /* private */ }

impl AccountsState {
    pub fn new(
        availability: crate::bw_path::MultiAccountAvailability,
        migration: crate::migration::MigrationState,
        accounts: Vec<Account>,
        active: AccountId,
    ) -> Self;
    pub fn active(&self) -> &Account;
    pub fn all(&self) -> &[Account];
    /// The accounts a user may switch TO right now. EMPTY when multi-account is
    /// blocked OR migration has not completed, whatever the list holds.
    pub fn switchable(&self) -> &[Account];
    pub fn can_add(&self) -> bool;
    pub fn blocked_reason(&self) -> Option<String>;
    pub fn hello_needs_reenrolment(&self) -> bool;
}
```

`AccountsState` composes `MultiAccountAvailability` (Task 1 — **its enum is not
extended**, because that task is already in flight with an implementer) with
`MigrationState` (Task 4). Two independent reasons a switch may be unavailable,
combined in exactly one place, so nothing else has to know about both.

```rust
#[test]
fn a_blocked_availability_offers_no_switch_targets_and_no_add() {
    let state = AccountsState::new(
        MultiAccountAvailability::BlockedByPortableProfile {
            relative_data_dir: PathBuf::from(r"C:\a\bin\bitwarden-cli") },
        completed(&a()), vec![a(), b()], a().id);
    assert!(state.switchable().is_empty(), "a switch was offered while the CLI ignores our env var");
    assert!(!state.can_add());
    assert!(state.blocked_reason().is_some());
    assert_eq!(state.all().len(), 2, "the list is still shown -- it is SWITCHING that is refused");
}

#[test]
fn a_blocked_migration_offers_no_switch_targets_even_when_the_cli_is_fine() {
    // The second, independent reason. A half-migrated or unmigrated profile
    // means the account directories do not hold what the list says they do.
    let state = AccountsState::new(
        MultiAccountAvailability::Available,
        MigrationState::Blocked { reason: "the copy could not be verified".into() },
        vec![a(), b()], a().id);
    assert!(state.switchable().is_empty());
    assert!(!state.can_add());
    assert!(state.blocked_reason().unwrap().contains("could not be verified"));
}

#[test]
fn an_available_migrated_state_offers_every_account_except_the_active_one() {
    // The positive control for both tests above, and the rule in its own right:
    // "switch to the account you are already on" is a no-op that would still
    // tear the backend down and demand a master password.
    let state = AccountsState::new(MultiAccountAvailability::Available, completed(&a()),
                                   vec![a(), b()], a().id);
    assert_eq!(state.switchable().iter().map(|x| x.id.clone()).collect::<Vec<_>>(), vec![b().id]);
    assert!(state.can_add());
    assert_eq!(state.blocked_reason(), None);
}

#[test]
fn the_hello_notice_survives_into_the_state_the_login_window_reads() {
    // WIRING for Task 7's panel line: a `hello_needs_reenrolment` that Task 4
    // computes and nothing carries forward is a notice the user never sees.
    let loud = AccountsState::new(MultiAccountAvailability::Available,
        MigrationState::Completed { account: a(), hello_needs_reenrolment: true },
        vec![a()], a().id);
    assert!(loud.hello_needs_reenrolment());
    let quiet = AccountsState::new(MultiAccountAvailability::Available,
        MigrationState::Completed { account: a(), hello_needs_reenrolment: false },
        vec![a()], a().id);
    assert!(!quiet.hello_needs_reenrolment());
}
```

`switchable()` returns `&[Account]` over a field computed in `new` — deliberately, so a
caller cannot be tempted to reimplement the filter.

**What would make these tests fail:** `switchable()` ignoring either input (the two
mutations that re-open the profile-corruption trap and the half-migration trap, each
invisible to every other test); including the active account; the Hello flag dropped in
transit.

---

# Task 11 — Startup: resolve, point, resume

> Amended 2026-08-03. Step 1 (migrate) is gone. `resolve_startup(stored, stored_active,
> unreadable_reason)` answers from `settings.accounts` alone and mints one account when
> there are none; `StartupAccounts::Unmigrated` is now `NoAccountList { reason }`, whose
> only producer is a `settings.json` that exists and cannot be parsed. Every code block
> below that names `MigrationState` describes deleted code.

**Files:** modify `deskwarden/src/main.rs`; modify `deskwarden/src/accounts.rs`.

**Interfaces**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAccounts {
    /// The normal case: at least one account, one of them active.
    Ready { active: Account, accounts: Vec<Account>, needs_persist: bool },
    /// Migration did not produce an account list. The app runs as a
    /// single-account app against the CLI's own default directory, exactly as it
    /// does today, and `reason` is what `AccountsState` reports wherever a
    /// switch would have been. **This is the only state in which the app has no
    /// `Account` at all**, and it is a startup condition, not an account variant
    /// -- see Task 2 on why `AccountLocation` does not exist.
    Unmigrated { reason: String },
}

pub fn resolve_startup(
    stored: &[Account],
    stored_active: Option<&AccountId>,
    migration: &crate::migration::MigrationState,
) -> StartupAccounts;
```

### Step 11.1 — the resolution

```rust
#[test]
fn a_completed_migration_on_this_launch_becomes_the_active_account() {
    let migrated = account("0123456789abcdef0123456789abcdef");
    let r = resolve_startup(&[], None,
        &MigrationState::Completed { account: migrated.clone(), hello_needs_reenrolment: true });
    let StartupAccounts::Ready { active, accounts, needs_persist } = r else { panic!("{r:?}") };
    assert_eq!(active.id, migrated.id);
    assert_eq!(accounts.len(), 1);
    assert!(needs_persist, "the migrated account must be written before the next launch");
}

#[test]
fn the_stored_active_account_is_resumed_on_a_later_launch() {
    let (a, b) = (account("0123456789abcdef0123456789abcdef"),
                  account("fedcba9876543210fedcba9876543210"));
    let r = resolve_startup(&[a.clone(), b.clone()], Some(&b.id), &completed(&a));
    let StartupAccounts::Ready { active, accounts, needs_persist } = r else { panic!() };
    assert_eq!(active.id, b.id, "a restart must resume the account that was last active");
    assert_eq!(accounts.len(), 2, "and must not drop the others");
    assert!(!needs_persist, "nothing changed, so nothing is rewritten");
}

#[test]
fn an_active_id_naming_no_stored_account_falls_back_to_the_first() {
    // A hand-edited settings.json, or an account removed by a build that crashed
    // mid-write. Falling through to "no active account" would leave the app with
    // nothing to point the CLI at.
    let a = account("0123456789abcdef0123456789abcdef");
    let ghost = AccountId::parse(&"9".repeat(32)).unwrap();
    let r = resolve_startup(&[a.clone()], Some(&ghost), &completed(&a));
    let StartupAccounts::Ready { active, needs_persist, .. } = r else { panic!() };
    assert_eq!(active.id, a.id);
    assert!(needs_persist, "the dangling active id must be corrected on disk");
}

#[test]
fn a_blocked_migration_leaves_the_app_unmigrated_rather_than_inventing_an_account() {
    // The failure path that keeps the app working. Inventing an `Account` here
    // would point the CLI at an EMPTY directory and present as "signed out",
    // while the real profile sat untouched a few directories away -- the exact
    // symptom a user would report as "the update deleted my vault".
    let r = resolve_startup(&[], None,
        &MigrationState::Blocked { reason: "the copy could not be verified".into() });
    let StartupAccounts::Unmigrated { reason } = r else { panic!("{r:?}") };
    assert!(reason.contains("could not be verified"));
}

#[test]
fn a_first_install_gets_one_fresh_account_rather_than_running_unmigrated() {
    // `NothingToMigrate` is not a failure: there was no profile because this is
    // a new machine. Give it an account directory and let the user sign in there.
    let r = resolve_startup(&[], None, &MigrationState::NothingToMigrate);
    let StartupAccounts::Ready { accounts, needs_persist, .. } = r else { panic!("{r:?}") };
    assert_eq!(accounts.len(), 1);
    assert!(needs_persist);
}
```

### Step 11.2 — wire it into `main`

In `main()`, after `remember_verified_bw_exe(bw_exe)` (which
`multi_account_availability` depends on) and **before** `store` is built and before the
first `store.load()`:

```rust
let availability = bw_path::multi_account_availability();
let migration = migration::migrate(
    &config_dir,
    &availability,
    !settings.accounts.is_empty(),
    login_ui::check_bw_status_details_in,
    || bw_serve::port_in_use(bw_serve::BW_SERVE_PORT),
);
if let migration::MigrationState::Completed { hello_needs_reenrolment: true, .. } = &migration {
    // The one moment we know the user is at the machine: they just launched the
    // app. A tray app has no window, and a quick-unlock panel that is silently
    // absent is indistinguishable from Hello never having been set up.
    message_box(
        "Deskwarden",
        "Your Bitwarden profile has been moved so Deskwarden can hold more than one \
         account.\n\nWindows Hello quick unlock has to be set up again -- tick \"Use \
         Windows Hello\" the next time you enter your master password.",
        MB_ICONINFORMATION,
    );
}

let startup = accounts::resolve_startup(
    &settings.accounts, settings.active_account.as_ref(), &migration);
let (active_account, known_accounts) = match &startup {
    accounts::StartupAccounts::Ready { active, accounts, needs_persist } => {
        if *needs_persist {
            if let Err(e) = settings::Settings::persist_accounts(
                &settings_path, accounts, Some(&active.id))
            {
                log::warn!("could not persist the account list: {e}");
            }
        }
        (Some(active.clone()), accounts.clone())
    }
    accounts::StartupAccounts::Unmigrated { reason } => {
        log::warn!("{reason}; running as a single-account app against the CLI's default profile");
        (None, Vec::new())
    }
};

// The two arms below are "today's app" and "the account-aware app". This is a
// FALLBACK to existing behaviour, not a second implementation of anything: the
// switch, the resettle and the cache are untouched by it, and the `Unmigrated`
// arm never reaches them because `AccountsState::switchable()` is empty.
let (session_path, active_dir) = match &active_account {
    Some(a) => (accounts::session_path_for(&config_dir, &a.id),
                Some(accounts::data_dir_for(&config_dir, &a.id))),
    None => (config_dir.join("session.bin"), None),
};
bw_path::set_active_data_dir(active_dir);
let mut store = session_store::SessionStore::new(session_path);
```

`let store =` becomes `let mut store =`; the existing `config_dir.join("session.bin")`
line moves into the `None` arm.

```rust
#[test]
fn startup_migrates_and_points_the_cli_before_it_loads_a_session() {
    // WIRING, and the ordering is the point three times over: migration must
    // finish before the account is resolved (or it resolves against a profile
    // that is about to move); the CLI must be pointed at the account before
    // `store.load()` and before `check_bw_status_with_session` spawns `bw` (or
    // the first launch after a migration validates a token against the wrong
    // profile and silently re-authenticates, which reads as "it lost my login").
    let source = include_str!("main.rs");
    let migrate_at = source.find(concat!("migration::", "migrate(")).expect("no migration");
    let resolve_at = source.find(concat!("resolve_", "startup(")).expect("no resolution");
    let set_dir = source.find(concat!("set_active_data", "_dir(")).expect("no dir");
    let build_store = source.find(concat!("SessionStore", "::new(")).expect("no store");
    let load = source.find(concat!("store", ".load()")).expect("no load");
    assert!(migrate_at < resolve_at, "the account is resolved before migration runs");
    assert!(resolve_at < set_dir);
    assert!(set_dir < build_store, "the CLI is pointed at the account AFTER the store is built");
    assert!(build_store < load);
    // Positive control: five distinct positions were found.
    let mut all = vec![migrate_at, resolve_at, set_dir, build_store, load];
    let n = all.len(); all.sort_unstable(); all.dedup();
    assert_eq!(all.len(), n);
}

#[test]
fn the_hello_notice_is_raised_where_the_migration_completes() {
    // The user accepted re-enrolling. They did not accept finding out by having
    // quick unlock silently stop working.
    let source = include_str!("main.rs");
    let region = source.split_once(concat!("MigrationState::", "Completed")).expect("must exist").1;
    let window = &region[..region.len().min(1200)];
    assert!(window.contains(concat!("message", "_box(")), "no notice is shown on completion");
    assert!(window.to_lowercase().contains("windows hello"), "the notice does not name Hello");
}
```

**What would make these tests fail:** migration running after resolution, or the store
built before the CLI is pointed (11.2's ordering test — pure ordering, invisible to any
value-based test); inventing an `Account` for a blocked migration (11.1's fourth test —
the mutation that presents as "signed out" on upgrade while every other test passes);
the Hello notice dropped (11.2's second).

---

# Task 12 — Add an account

**Files:** modify `deskwarden/src/main.rs`; modify `deskwarden/src/accounts.rs`.

```rust
pub fn prepare_new_account(config_dir: &Path) -> Result<Account, String>;
pub fn discard_prepared_account(config_dir: &Path, id: &AccountId);

fn add_account(
    config_dir: &Path,
    state: &mut accounts::AccountsState,
    /* the switch parameters */,
    mut sign_in: impl FnMut(&Account) -> Option<String>,
) -> SwitchOutcome;
```

Flow: check `state.can_add()` → `prepare_new_account` → point the CLI at its directory
→ `sign_in` (live caller passes `|a| login_ui::run_login_flow_for(config_dir, a,
false)`, the existing sign-in window, 2FA and all, against the empty profile) → read
`check_bw_status_details_in` for the email and server URL → append,
`Settings::persist_accounts`, then run the **same** `switch_to_account` to make it
active. Declined or failed → `discard_prepared_account` and roll back.

```rust
#[test]
fn a_declined_sign_in_removes_the_directory_and_persists_no_account() {
    let before = state.all().len();
    let outcome = add_account(&cfg, &mut state, .., |_| None);
    assert!(matches!(outcome, SwitchOutcome::Declined | SwitchOutcome::RolledBack { .. }));
    assert_eq!(state.all().len(), before, "a half-created account was left in the list");
    assert_eq!(dir_entries(accounts::accounts_root(&cfg)).len(), before,
        "an empty profile directory was left behind");
}

#[test]
fn the_sign_in_runs_with_the_cli_pointed_at_the_new_accounts_directory() {
    // WIRING, and the one that decides whether "Add account" ADDS an account or
    // LOGS THE EXISTING ONE OUT. `bw login` in the wrong profile replaces the
    // account already there -- and after Task 4 that profile is the migrated one.
    let seen = std::cell::RefCell::new(None);
    add_account(&cfg, &mut state, .., |_account| {
        *seen.borrow_mut() = bw_path::active_data_dir();
        Some("session-token".into())
    });
    let seen = seen.into_inner().expect("the sign-in never ran");
    assert_eq!(seen, Some(accounts::data_dir_for(&cfg, &state.active().id)),
        "the sign-in ran in {seen:?}, not in the new account's own directory");
    assert!(seen.is_some(),
        "the sign-in ran in the CLI's DEFAULT profile -- this would sign the existing \
         account out and replace it");
}

#[test]
fn a_successful_add_persists_the_account_and_makes_it_active() {
    let outcome = add_account(&cfg, &mut state, .., |_| Some("session-token".into()));
    assert_eq!(outcome, SwitchOutcome::Switched);
    assert_eq!(state.all().len(), 2);
    let added = state.active();
    assert!(accounts::data_dir_for(&cfg, &added.id).is_dir());
    let loaded = settings::Settings::load(&cfg.join("settings.json"));
    assert_eq!(loaded.accounts.len(), 2, "persisted, not just held in memory");
    assert_eq!(loaded.active_account.as_ref(), Some(&added.id));
}

#[test]
fn add_is_refused_while_accounts_state_says_it_cannot_be_done() {
    // The `relativeDataDir` trap and an unfinished migration both reach here,
    // and adding an account under either would write a profile the app cannot
    // reliably reach again.
    let mut blocked = blocked_state(&cfg);
    assert!(!blocked.can_add());
    assert!(matches!(add_account(&cfg, &mut blocked, .., |_| Some("t".into())),
                     SwitchOutcome::Declined));
    assert!(dir_entries(accounts::accounts_root(&cfg)).is_empty());
}
```

**What would make these tests fail:** the sign-in running before (or without) the
directory swap — the mutation that turns "add" into "replace", invisible to any test
that only checks the resulting list; the account persisted before the sign-in succeeds;
`can_add` not consulted.

---

# Task 13 — Remove an account

**Files:** modify `deskwarden/src/main.rs` (`login_ui::bw_logout_in` lands in Task 7).

```rust
fn remove_account(
    config_dir: &Path,
    state: &mut accounts::AccountsState,
    target: &AccountId,
    /* switch parameters, for the case where the target is active */,
    mut logout: impl FnMut(Option<&Path>) -> Result<(), String>,
) -> Result<(), String>;
```

Order, and each step's reason:

1. If `target` is active, switch to `next_active_after_removal` **first**, so the
   removal never runs against the profile the backend is currently serving. No survivor
   → refuse: removing the only account leaves the app with nothing to point at.
2. `logout(Some(&data_dir_for(config_dir, target)))` — `bw logout` in **that**
   directory, via `bw_command_in`, never a temporary mutation of the process-global
   (background threads spawn `bw`).
3. `remove_dir_all(data_dir_for(config_dir, target))`, which takes `session.bin` and
   `hello.bin` with it — the reasoning `login_ui`'s log-out handler already applies: a
   sealed credential for an account the CLI no longer knows is a liability.
4. `Settings::persist_accounts` with the account gone.

`remove_account` must assert `data_dir_for(..).starts_with(accounts_root(..))` before
calling `remove_dir_all`, and return `Err` rather than delete if it does not —
belt-and-braces over `AccountId::parse`, because this and `migration` are the only two
`remove_dir_all` call sites in the crate that run on a path built from stored data.

```rust
#[test]
fn removing_an_account_logs_out_in_that_accounts_own_directory() {
    // WIRING. `bw logout` with the wrong (or no) profile directory logs the
    // WRONG ACCOUNT out -- the active one -- and the removed one stays signed in
    // on disk forever.
    let seen = std::cell::RefCell::new(Vec::new());
    remove_account(&cfg, &mut state, &b.id, .., |dir| {
        seen.borrow_mut().push(dir.map(Path::to_path_buf)); Ok(()) }).unwrap();
    assert_eq!(seen.into_inner(), vec![Some(accounts::data_dir_for(&cfg, &b.id))]);
}

#[test]
fn removing_an_account_deletes_its_secrets_and_only_its_own() {
    for acct in [&a, &b] { plant_secrets(&cfg, &acct.id); }
    remove_account(&cfg, &mut state, &b.id, .., |_| Ok(())).unwrap();
    assert!(!accounts::session_path_for(&cfg, &b.id).exists());
    assert!(!accounts::hello_blob_path_for(&cfg, &b.id).exists());
    assert!(!accounts::data_dir_for(&cfg, &b.id).exists());
    assert!(accounts::session_path_for(&cfg, &a.id).exists(), "the WRONG account's token went");
    assert!(accounts::hello_blob_path_for(&cfg, &a.id).exists());
    assert_eq!(state.all().len(), 1);
}

#[test]
fn removing_the_active_account_switches_away_first_and_never_removes_the_last_one() {
    remove_account(&cfg, &mut state, &a.id, .., |_| Ok(())).unwrap();   // `a` was active
    assert_eq!(state.active().id, b.id, "the app was left pointing at a removed account");
    assert_eq!(bw_path::active_data_dir(), Some(accounts::data_dir_for(&cfg, &b.id)));
    assert!(remove_account(&cfg, &mut state, &b.id, .., |_| Ok(())).is_err());
    assert_eq!(state.all().len(), 1);
}

#[test]
fn a_removal_never_deletes_above_the_accounts_directory() {
    // `remove_dir_all` on a mis-built path -- an empty id, a `..` that slipped
    // past `parse`, one `parent()` too many -- takes settings.json, the log, and
    // the account list naming the survivors with it. And after Task 4 it would
    // take the OTHER accounts' migrated profiles too.
    std::fs::write(cfg.join("settings.json"), "{}").unwrap();
    remove_account(&cfg, &mut state, &b.id, .., |_| Ok(())).unwrap();
    assert!(cfg.join("settings.json").exists(), "the config directory was deleted");
    assert!(accounts::accounts_root(&cfg).is_dir(), "the accounts root was deleted");
    assert!(accounts::data_dir_for(&cfg, &a.id).is_dir(), "another account's profile went with it");
    assert!(accounts::data_dir_for(&cfg, &b.id).starts_with(accounts::accounts_root(&cfg)));
}
```

**What would make these tests fail:** `bw_logout()` (active profile) instead of
`bw_logout_in(target)`; secrets deleted by a fixed path; removing the active account
without switching away, or removing the last one; a `remove_dir_all` that can escape
`accounts_root`.

---

# Task 14 — The switcher in the vault window

**Files:** modify `deskwarden/src/vault_window/mod.rs`; `deskwarden/src/main.rs`.

Follows the `open_preferences` precedent exactly — a **fourth distinct field**, not a
reuse of `locked` or `needs_reauth`. Asking to switch says nothing about the current
session being gone; folded into either flag, the recovery would run against the wrong
account.

```rust
pub struct VaultWindowResult {
    pub locked: bool,
    pub needs_reauth: bool,
    pub open_preferences: bool,
    /// The account the user picked in the titlebar switcher. The window closed
    /// only because `main` has to tear the backend down and bring another one
    /// up, which cannot happen while this window owns the event loop -- exactly
    /// the reason `open_preferences` exists. Distinct from `locked` and
    /// `needs_reauth`: this session was never lost.
    pub switch_to: Option<crate::accounts::AccountId>,
}
// `vault_window::run` gains one parameter: `accounts: crate::accounts::AccountsState`.
```

```rust
#[test]
fn the_switcher_lists_only_the_switchable_accounts_and_reports_the_pick() {
    // Driven through a real frame with a headless `egui::Context`. Pins that the
    // pick REACHES the result -- a switcher that paints correctly and returns
    // `None` is the "decision correct, renderer inert" shape this codebase keeps
    // producing.
    let pane = vault_pane(available_state());
    pane.click_switcher_entry(&b().email);
    assert_eq!(pane.result().switch_to, Some(b().id));
    assert!(!pane.result().locked, "a switch must not be reported as a lock");
    assert!(!pane.result().needs_reauth);
}

#[test]
fn a_blocked_state_paints_the_reason_instead_of_a_switcher() {
    let pane = vault_pane(blocked_state());
    assert!(pane.switcher_entries().is_empty(), "a switch was offered while it cannot work");
    assert!(pane.text().contains("bitwarden-cli"), "the user is not told why");
}

#[test]
fn open_vault_window_acts_on_a_switch_and_reopens_rather_than_running_the_lock_recovery() {
    // WIRING. A `switch_to` that `open_vault_window` never reads means the
    // switcher is 100% inert -- the exact shape of the Trash/Archive feature
    // that shipped dead behind an early return with a green suite.
    let source = include_str!("main.rs");
    let needle = concat!("result.switch", "_to");
    assert!(source.contains(needle), "open_vault_window never reads the switcher's result");
    let switch_at = source.find(needle).unwrap();
    let lock_at = source.find("if result.locked || result.needs_reauth").unwrap();
    assert!(switch_at < lock_at, "a switch would be swallowed by the lock recovery");
    assert_ne!(switch_at, lock_at, "positive control: two distinct positions");
}
```

**What would make these tests fail:** a switcher that paints but never sets `switch_to`;
`switch_to` folded into `locked`; `open_vault_window` never reading the field; the
switcher offered while blocked.

---

# Task 15 — The tray accounts submenu, and the final wiring pins

**Files:** modify `deskwarden/src/tray.rs`; `deskwarden/src/main.rs`.

```rust
pub struct AccountsMenu {
    /// `MenuId` → `AccountId`, so the main loop answers "which account was
    /// clicked?" without matching on labels.
    entries: Vec<(MenuId, crate::accounts::AccountId)>,
    add_id: MenuId,
    manage_id: MenuId,
}

impl AppTray {
    pub fn rebuild_accounts_menu(&mut self, state: &crate::accounts::AccountsState);
    pub fn account_for_menu_id(&self, id: &MenuId) -> Option<&crate::accounts::AccountId>;
}
```

`account_for_menu_id` is a pure lookup and is what the test drives; the `muda` menu
construction is the smallest possible layer above it.

```rust
#[test]
fn a_menu_id_maps_back_to_the_account_it_was_built_for() {
    let menu = AccountsMenu::from_entries(
        vec![(MenuId::new("m1"), a().id), (MenuId::new("m2"), b().id)],
        MenuId::new("add"), MenuId::new("manage"));
    assert_eq!(menu.account_for_menu_id(&MenuId::new("m2")), Some(&b().id));
    assert_eq!(menu.account_for_menu_id(&MenuId::new("add")), None,
        "\"Add account...\" must not be mistaken for an account");
    assert_eq!(menu.account_for_menu_id(&MenuId::new("nope")), None);
}

#[test]
fn a_blocked_state_builds_no_account_entries() {
    assert!(accounts_menu_entries(&blocked_state()).is_empty(),
        "the tray offered a switch the CLI would ignore");
    // Positive control on the same helper.
    assert_eq!(accounts_menu_entries(&available_state()).len(), 1,
        "only the non-active account is a switch target");
}

#[test]
fn the_active_account_is_persisted_after_every_successful_switch() {
    // WIRING. Without this the app resumes the PREVIOUS account on restart --
    // "switching that appears to work and then doesn't stick", which is
    // indistinguishable from the `relativeDataDir` trap and would send whoever
    // debugs it straight down the wrong path.
    let source = include_str!("main.rs");
    let needle = concat!("Settings::persist", "_accounts(");
    assert!(source.matches(needle).count() >= 2);
    let switched = source.split(concat!("SwitchOutcome::", "Switched")).nth(1).unwrap();
    assert!(switched.contains(needle), "a successful switch does not persist the new active account");
}

#[test]
fn nothing_offers_a_switch_without_going_through_accounts_state() {
    // The `relativeDataDir` refusal and the migration refusal have exactly one
    // door (Task 10). A UI reading `settings.accounts` directly bypasses both.
    for file in ["tray.rs", "main.rs", "vault_window/mod.rs"] {
        let source = read_source(file);
        assert!(!source.contains(concat!("settings.accounts", ".iter()")),
            "{file} iterates the raw account list instead of AccountsState::switchable()");
    }
    assert!(format!("x{}y", concat!("settings.accounts", ".iter()"))
        .contains(concat!("settings.accounts", ".iter()")), "positive control");
}
```

Main-loop wiring, beside the existing `tray.open_vault_id` / `tray.preferences_id`
handlers, dispatching to `switch_to_account` and reporting each `SwitchOutcome` — a
`RolledBack` raises a `message_box`, a `StoodDown` logs at `error`.

**What would make these tests fail:** a menu id mapping to the wrong account, or "Add
account…" treated as one; entries built while blocked; a successful switch not
persisting the active account; a UI reading the raw list.

---

## Verification, at the end of every task

```
cargo test  --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
```

Both clean and warning-free. Report the counts (`N lib + 34 bin`) in the commit message,
and record in `.superpowers/sdd/progress.md` — per task — **the mutation each new test
was watched to fail on**, with the verbatim failure message. A test that has not been
watched to fail has not been shown to test anything, and that is how this repository has
shipped a feature 100% inert behind an early return with a green suite.

~~For **Task 4 specifically**, the mutation log is not optional~~ — Task 4 is removed
(see above) and its mutation table with it, since every test it named is deleted. The
mutations watched for what replaced it are recorded in `.superpowers/sdd/progress.md`
under the 2026-08-03 entry: a startup that mints nothing, a startup that mints twice, an
unreadable account list that mints anyway, a `first_run` that is always on, a
`first_run_account` that never leaves startup, a notice offered to every account, the
add-account persist moved back after the switch, and the discard dropped from its
failure path.



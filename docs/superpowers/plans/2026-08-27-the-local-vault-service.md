# The Local Vault Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A loopback REST endpoint, authenticated, that a program the owner writes can call to read their vault — the third consumer from `2026-08-27-the-local-vault-service-design.md`.

**Architecture:** A third process mode, `deskwarden.exe --service`, wrapping the existing `rest::` client and `CachingBackend`. It claims a `vault_service` attachment slot; 24/7 mode is that slot held permanently. Nothing about `bw serve` changes.

**Tech Stack:** Rust, `windows` 0.58, `tiny_http` (see Task 2), the `vault_service` module on `main`.

## Task 0: The auth decision, taken

**A bearer token, minted by the service, DPAPI-wrapped on disk, compared in constant time.**

Rejected: loopback with no auth. It is what `bw serve` does, and it is the
weakest option — every process on the machine, including one running as
another user that can reach loopback, gets the vault by connecting. "As bad
as the thing we are replacing" is not a standard.

**The limit, stated rather than discovered:** any process running as this user
can read the token file, because DPAPI unwraps under this user's credentials.
So the token stops *other users* and *unprivileged remote reach*, and does not
stop a program already running as the owner. That is the same limit
`session_store` and `user_key_store` already have, and it is the strongest
thing available without a per-client credential the owner would have to
manage.

## Scope: the new capability only

**The daemon and the vault window are NOT migrated to HTTP.** They keep their
current in-process `VaultBackend`. Migrating two working apps is a separate
piece of work with no user-visible benefit on its own, and doing it here would
put the risk of that migration in front of the feature that is actually new.

This plan delivers: a script can read the vault. That is all, and it is the
whole point of the third consumer.

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, spawn `bw`, or **bind a port**. HTTP requests are classified by pure functions; kernel objects and sockets go behind `fn` pointers.
- **No `cfg(test)` seams.** Banned crate-wide.
- **No secret on a command line.** `ui_process`'s rule. The service reads its token from disk; it is never an argument.
- **Loopback only.** A bind to anything but `127.0.0.1` is a bug, and there is a test for it.
- **One target directory**, `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **`--lib` does not run `main.rs`'s tests.** Use `--bin deskwarden` too.
- **CI is the arbiter.** The local suite yields a different failing set each run; CI triggers on `main` and PRs to `main`.
- **`job_object` keeps a ledger** of every `.rs` outside `src/`; `foreground` keeps one of every module. Both will catch new files, as designed.
- **`cargo deny` runs in CI.** A new dependency must pass it; check the licence before adding.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `local-vault-service`.

---

### Task 1: The token

**Files:** Create `deskwarden/src/service_token.rs`; modify `deskwarden/src/lib.rs`, `deskwarden/src/foreground.rs` (classification list)

**Interfaces:**
```rust
pub struct Token(String);                       // never `Debug`-printed
pub fn mint(random: fn() -> [u8; 32]) -> Token;
pub fn matches(expected: &Token, presented: &str) -> bool;   // constant time
pub fn bearer_of(header: Option<&str>) -> Option<&str>;      // pure parse
```

`user_key_store`'s idiom for the file: DPAPI-wrapped, at a path the caller
chooses, so a test drives a temp path and never `%APPDATA%`.

- [x] **Step 1: Write the failing tests**

```rust
#[test]
fn a_wrong_token_is_refused() {
    let expected = mint(|| [7u8; 32]);
    assert!(!matches(&expected, "not-the-token"));
}

#[test]
fn the_right_token_is_accepted() {
    let expected = mint(|| [7u8; 32]);
    let presented = expected.expose_for_test_only_in_this_module();
    assert!(matches(&expected, presented));
}

/// A prefix of the real token must not be accepted, and must not be
/// cheaper to reject than a full-length wrong one.
#[test]
fn a_prefix_of_the_token_is_refused() {
    let expected = mint(|| [7u8; 32]);
    let full = expected.expose_for_test_only_in_this_module().to_string();
    assert!(!matches(&expected, &full[..full.len() - 1]));
}

#[test]
fn only_a_bearer_header_yields_a_token() {
    assert_eq!(bearer_of(Some("Bearer abc")), Some("abc"));
    assert_eq!(bearer_of(Some("Basic abc")), None);
    assert_eq!(bearer_of(Some("Bearerabc")), None);
    assert_eq!(bearer_of(None), None);
}

/// The house guard: a token that reaches a log is a token in a file the
/// user might paste into a bug report.
#[test]
fn the_token_type_does_not_derive_debug() {
    let source = include_str!("service_token.rs");
    let cut = source.find("#[cfg(test)]").expect("control: no test module");
    let production = &source[..cut];
    assert!(production.contains("pub struct Token"), "control: the type is gone");
    assert!(
        !production.contains("#[derive(Debug)]"),
        "`Token` or a neighbour derives Debug; a secret that can be printed will be"
    );
}
```

- [x] **Step 2: Run to verify they fail.** `cargo test --lib -- service_token` — expected: `cannot find function mint`.
- [x] **Step 3: Implement.** `mint` hex-encodes 32 random bytes. `matches` folds a difference over the whole of both strings and compares lengths without an early return. `bearer_of` requires the exact prefix `"Bearer "`.
- [x] **Step 4: Run to verify they pass.** Plus a mutation check: an early return inserted into `matches` made the source pin fail, and removing it made it pass.
- [x] **Step 5: Commit.**

**Note against the plan as written:** the plan proposed a `expose_for_test_only_in_this_module` accessor. That would have been a test-only seam, which is banned crate-wide. `Token::expose` is a single production method with its two legitimate callers named, used by the tests as well.

---

### Task 2: The request decision, without a socket

**Files:** Create `deskwarden/src/service_api.rs`; modify `deskwarden/src/lib.rs`, `deskwarden/src/foreground.rs`

Every rule about who may call what is a pure function of (method, path,
authorization header). The socket is Task 4's problem, and no test binds one.

**Interfaces:**
```rust
pub enum Route { Status, Items, Item(String), Unknown }
pub enum Answer { Ok(Route), Unauthorized, NotFound, MethodNotAllowed }
pub fn decide(method: &str, path: &str, auth: Option<&str>, expected: &Token) -> Answer;
```

- [ ] **Step 1: Write the failing tests**

```rust
/// **The test this module exists for.** No token, no vault.
#[test]
fn every_route_refuses_an_unauthenticated_caller() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    for path in ["/status", "/items", "/items/abc", "/nonsense"] {
        assert_eq!(
            decide("GET", path, None, &expected),
            Answer::Unauthorized,
            "{path} answered something other than 401 with no credential"
        );
    }
}

/// Including `/status`, which is the one that will be argued about.
/// "It only says whether the vault is locked" is still a fact about this
/// user's vault, told to anything that asks.
#[test]
fn status_is_not_a_public_endpoint() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    assert_eq!(decide("GET", "/status", Some("Bearer wrong"), &expected), Answer::Unauthorized);
}

#[test]
fn an_authenticated_caller_reaches_the_routes() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    let header = format!("Bearer {}", expected.expose_for_test_only_in_this_module());
    assert_eq!(decide("GET", "/status", Some(&header), &expected), Answer::Ok(Route::Status));
    assert_eq!(decide("GET", "/items", Some(&header), &expected), Answer::Ok(Route::Items));
    assert_eq!(
        decide("GET", "/items/abc", Some(&header), &expected),
        Answer::Ok(Route::Item("abc".to_string()))
    );
}

/// Read-only for now. A write API is a separate decision and must not
/// arrive by accident.
#[test]
fn writing_methods_are_refused_even_when_authenticated() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    let header = format!("Bearer {}", expected.expose_for_test_only_in_this_module());
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        assert_eq!(
            decide(method, "/items", Some(&header), &expected),
            Answer::MethodNotAllowed,
            "{method} /items was allowed; this service is read-only"
        );
    }
}

/// Auth is checked BEFORE the path is understood, so an unknown path
/// cannot be used to probe which routes exist.
#[test]
fn an_unknown_path_is_not_a_way_to_learn_which_routes_exist() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    assert_eq!(decide("GET", "/nonsense", None, &expected), Answer::Unauthorized);
}
```

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement.** Check the credential first and return `Unauthorized` before parsing the path at all; then method; then route.
- [ ] **Step 4: Run to verify they pass.**
- [ ] **Step 5: Commit.**

---

### Task 3: What a response may contain

**Files:** Modify `deskwarden/src/service_api.rs`

`/items` returns `app::ItemFacts` — the projection that already exists
precisely because it has no field a secret fits in. `/items/{id}` returns one
full item, because fetching a password is the point of the third consumer.

- [ ] **Step 1: Write the failing tests**

```rust
/// The list endpoint is a list of facts, not of items. A script that wants
/// a password asks for one by id, which is one call a reviewer can find.
#[test]
fn the_list_endpoint_carries_no_secret() {
    let body = items_body(&[fact_with_everything_filled()]);
    for forbidden in ["password", "totp", "notes", "card", "identity"] {
        assert!(
            !body.to_lowercase().contains(forbidden),
            "`{forbidden}` appears in the list body; the list is facts only"
        );
    }
}

/// Control: the body is not empty, or the assertions above prove nothing.
#[test]
fn the_list_endpoint_actually_lists() {
    let body = items_body(&[fact_with_everything_filled()]);
    assert!(body.contains("example.com"), "control: nothing was serialised: {body}");
}
```

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** `items_body` over `ItemFacts` and `item_body` over one `VaultItem`.
- [ ] **Step 4: Run to verify they pass.**
- [ ] **Step 5: Commit.**

---

### Task 4: The process

**Files:** Modify `deskwarden/src/main.rs`, `deskwarden/Cargo.toml`

`deskwarden.exe --service`. Binds `127.0.0.1:0` unless a port is configured,
writes its chosen port and its token beside the config, claims a
`vault_service` slot, and serves until nobody is attached.

**Dependency:** `tiny_http` — no async runtime, one thread, small enough to
read. **Check `cargo deny` accepts its licence before writing any code**; if
it does not, hand-rolling HTTP/1.1 for four read-only routes is viable and
should be costed rather than assumed.

- [ ] **Step 1: Write the failing tests**

```rust
/// Binding anything but loopback would put the vault on the network.
#[test]
fn the_listen_address_is_loopback() {
    assert_eq!(listen_addr(0).ip().to_string(), "127.0.0.1");
}

/// 24/7 and consumer-driven are the same mechanism: a held slot.
#[test]
fn installed_mode_is_one_permanent_attachment() {
    assert_eq!(slots_to_hold(Mode::Installed), 1);
    assert_eq!(slots_to_hold(Mode::ConsumerDriven), 0);
}
```

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** the `--service` arm, reusing `vault_service::attach` / `anyone_attached` / `supervise`. No new lifetime logic.
- [ ] **Step 4: Run to verify they pass.**
- [ ] **Step 5: Commit.**

---

### Task 5: Telling the owner it is on

**Files:** Modify `deskwarden/src/prefs_ui.rs`, `deskwarden/src/settings.rs`, `README.md`

A process serving a decrypted vault must be visible in the app that started
it, and the owner must be able to turn it off. Off is the default.

- [ ] **Step 1: Write the failing test** — the setting defaults to off, and a settings file predating it reads as off rather than failing to parse.
- [ ] **Steps 2–5:** red, implement, full suite, commit.

---

## Verification

- [ ] Full suite `--lib` and `--bin deskwarden`; CI as arbiter.
- [ ] `cargo clippy --all-targets` and `cargo deny check licenses advisories` clean.
- [ ] **A live check, and it is the point of the feature:** start the service, `curl` `/items` with no token (expect 401), with a wrong token (401), with the right token (a list with no secrets in it), then `/items/{id}` for one password. Confirm from a second logon session, or state plainly that it was not tested.
- [ ] **Ask before launching Deskwarden.** Starting a build trips `single_instance`'s takeover and kills the owner's running app; that has already happened once here.
- [ ] Say plainly what the token does and does not stop.

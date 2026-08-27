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

## The API is `bw serve`'s, not a new one

**Corrected 2026-08-27, after Task 1.** The first draft of this plan invented
`/items` and `/items/{id}`. That was wrong, and the crate itself says why:
`vault_bridge` is already a complete client for `bw serve`'s API --
`/list/object/items`, `/object/item/{id}`, `/object/item`,
`/restore/item/{id}` -- and has been for the whole life of this project.

Speaking that API instead of a new one buys three things a new one does not:

1. **Scripts already written against `bw serve` keep working**, apart from
   the auth header.
2. **The daemon and the vault window are a base-URL change**, not a
   migration. `VaultBridge::new(base_url)` already takes the address; pointing
   it at this service instead of `bw serve` is the whole of it.
3. **Dropping the `bw` CLI becomes invisible.** This is a drop-in for
   `bw serve`, backed by `rest::` and the encrypted cache rather than by a
   Node binary.

The earlier draft's "the daemon and window are NOT migrated" section is
withdrawn: it was solving a problem the invented API had created. Migration
is still not in *this* plan -- it is a separate, small, testable change --
but it is no longer a rewrite standing in the way.

## What is served is plaintext, and that is the point

This service answers with **decrypted** vault items. The remote Bitwarden API
serves encrypted blobs that only a holder of the master-password-derived key
can read; this one serves what those blobs say, over a loopback socket,
guarded by a bearer token any process running as the owner can read.

That is exactly what `bw serve` is, and exactly why `bw serve` is kept in a
kill-on-close job object today. It is unavoidable if the goal is "a program
the owner writes can read the vault" -- a script cannot derive the key. It is
written here so that it is a decision on the record rather than a property
someone discovers.

**One consequence for the compatible API:** `bw serve` requires no
credential at all. This service requires one, so a script written against
`bw serve` needs a header added. That is a deliberate incompatibility, and
the only one -- see Task 0.

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
pub enum Route { Status, ListItems, Item(String), ListFolders, Unknown }
pub enum Answer { Ok(Route), Unauthorized, NotFound, MethodNotAllowed }
pub fn decide(method: &str, path: &str, auth: Option<&str>, expected: &Token) -> Answer;
// Paths are `bw serve`'s: /status, /list/object/items, /list/object/folders, /object/item/{id}
```

- [ ] **Step 1: Write the failing tests**

```rust
/// **The test this module exists for.** No token, no vault.
#[test]
fn every_route_refuses_an_unauthenticated_caller() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    for path in ["/status", "/list/object/items", "/object/item/abc", "/nonsense"] {
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
    assert_eq!(
        decide("GET", "/list/object/items", Some(&header), &expected),
        Answer::Ok(Route::ListItems)
    );
    assert_eq!(
        decide("GET", "/object/item/abc", Some(&header), &expected),
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
            decide(method, "/list/object/items", Some(&header), &expected),
            Answer::MethodNotAllowed,
            "{method} /list/object/items was allowed; this service is read-only for now"
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

**A conflict the corrected API forces, resolved here rather than discovered
later.** The first draft had `/list/object/items` return `app::ItemFacts` --
the projection with no field a secret fits in -- so that fetching a password
was one call a reviewer could find. **Compatibility makes that impossible:**
`bw serve`'s list returns full items, and `VaultBridge::populate` needs them
that way to fill credentials. A facts-only list would break the crate's own
client and every existing script, to protect a boundary the bearer token has
already been chosen to be.

**Decision: the list is compatible, and returns full items.** The reasons:

- The token is the boundary. A caller that reached this endpoint has already
  presented it; withholding passwords from a caller entitled to fetch each
  one individually is theatre, not a control.
- The alternative protects nothing an attacker cannot get with N more
  requests, while breaking the primary consumer.
- `bw serve` does this today. This is a drop-in for it, and a drop-in that
  answers differently is not one.

What is NOT given up, and must be tested:

- [ ] **Step 1: Write the failing tests**

```rust
/// Compatibility is the requirement, so it is asserted rather than assumed:
/// the shape `VaultBridge` already parses is the shape this answers with.
#[test]
fn the_list_body_is_what_our_own_client_already_parses() {
    let body = list_items_body(&[an_item_with_a_password()]);
    let parsed = crate::vault_bridge::parse_items_response(&body)
        .expect("our own client could not read our own list body");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].login_password().as_deref(), Some("hunter2"));
}

/// Control: an empty body would satisfy nothing above by accident.
#[test]
fn the_list_body_actually_carries_the_item() {
    assert!(list_items_body(&[an_item_with_a_password()]).contains("example.com"));
}

/// **The line that does not move.** Serving secrets to an authenticated
/// caller is the point; serving them to an unauthenticated one is the
/// failure. Task 2 refuses the request; this asserts no body is built on
/// the way to that refusal.
#[test]
fn no_body_is_built_for_a_caller_that_was_refused() {
    let expected = crate::service_token::mint(|| [1u8; 32]);
    assert_eq!(body_for(decide("GET", "/list/object/items", None, &expected)), None);
}
```

- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** `list_items_body`, `item_body` and `body_for`.
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
- [ ] **A live check, and it is the point of the feature:** start the service, `curl` `/list/object/items` with no token (expect 401), with a wrong token (401), with the right token (a list with no secrets in it), then `/object/item/{id}` for one password. Confirm from a second logon session, or state plainly that it was not tested.
- [ ] **Ask before launching Deskwarden.** Starting a build trips `single_instance`'s takeover and kills the owner's running app; that has already happened once here.
- [ ] Say plainly what the token does and does not stop.

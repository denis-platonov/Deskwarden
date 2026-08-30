# Receiving a Send Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user on Deskwarden's built-in client pastes a Send link and reads
the Send. No `bw.exe` runs. It works whether their self-hosted server is old
enough to speak the anonymous route or new enough to have removed it, and the
user is never told which — the sentence they see is about their Send, never
about a route. A user on the official Bitwarden CLI sees exactly what they see
today, `RECEIVE_NEEDS_THE_CLI` included.

**Architecture:** This implements
`docs/superpowers/specs/2026-08-30-receiving-a-send-design.md`, whose §1 and §2
are the investigation this branch exists to have done: the two routes, quoted
from `bitwarden/server` and `bitwarden/clients` at named tags, and the finding
that there is **no stable route and no capability flag** — the anonymous
`POST /api/sends/access/{id}` was removed in server `v2026.8.0`, the bearer
`POST /api/sends/access` arrived in `v2026.1.1`, and the official client
probes for neither. This plan builds the probe the spec's §2.3 specifies:
mint a `send_access` grant first, and fall back to the anonymous route on
`unsupported_grant_type` (or a `404` at the token endpoint) and on nothing
else.

**The cryptography is not designed here and not edited here.**
`deskwarden/src/rest/send_crypto.rs` already carries the derivation, pinned
against Bitwarden's own two `shareable_key.rs` vectors. Receiving runs it in
reverse and adds nothing: `SendKey::from_bytes` → `cipher_key()` →
`crypto::decrypt`. `SendKey::password_hash` is the value **both** routes take,
unchanged.

**Tech Stack:** Rust, `ureq` through `crate::http_agent`, `serde_json`,
`zeroize`, and `crate::test_http` for every request test.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code, in the shape of `ServiceEnv`, `DiskCacheEnv` and `login_ui::SecondFactorSeam`.
- Build with `RUSTFLAGS="-D warnings"`, on the build **and** on `cargo test --no-run`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names". A test asserting a request was *not* made must, in the same test, assert a request that *was*.
- **No test may touch** the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`. Every request goes through `crate::test_http`.
- **No real Send link, id, key or share password appears in this repository.** Every fixture is invented, and named so — the file already does this (`"an-invented-share-password"`).

Additionally, and specific to this branch:

- **Do not edit `deskwarden/src/send.rs`.** `cli_send_receive` and everything under it is the `BwServe` arm and must survive byte for byte, including its three source guards. If a task appears to need a change there, stop and report.
- **Do not edit `deskwarden/src/rest/send_crypto.rs`** except to make an existing item `pub(crate)`. A new derivation, a new constant or a second password hash means the spec was misread.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/rest/send_link.rs` (new) | Pure. A pasted link → origin, access id, `SendKey`. No I/O. |
| `deskwarden/src/rest/api.rs` (modify) | `anon_agent`, `SendAccessToken`, the grant, the two access routes, and the census. |
| `deskwarden/src/rest/send.rs` (modify) | The probe, the classification, the response parse, `receive_on_active_account`. |
| `deskwarden/src/vault_window/send_ui.rs` (modify) | The receive branch; `RECEIVE_NEEDS_THE_CLI` narrows to the `BwServe` arm. |
| `deskwarden/src/rest/mod.rs` (modify) | The "what is missing" list, which is a promise to be current. |

## Interfaces

```rust
// deskwarden/src/rest/send_link.rs — new, pure.

/// One pasted Send link, taken apart. **Not `Debug`**: it holds the key.
pub struct SendLink { origin: String, access_id: String, key: SendKey }

/// Parse, and refuse rather than guess.
///
/// `configured` is the account's own server URL. An origin that is not
/// exactly it is refused by name -- this process must not carry a PBKDF2 hash
/// of the user's typed password to a host chosen by whoever wrote the link.
/// Exact origin match through `favicon::host_from_url`; never a suffix.
pub fn parse(link: &str, configured: &str) -> Result<SendLink, SendError>;

// deskwarden/src/rest/api.rs — new.

/// A `send_access` bearer, minted at identity and worth one request.
/// **Not `Debug`**, by `Challenge`'s rule. No `Clone`, no cache.
pub struct SendAccessToken(/* private */);

/// Why a grant did not produce a token, in the server's own vocabulary.
/// `GrantAbsent` is the ONLY variant that may trigger the legacy fallback.
pub enum SendGrantRefusal {
    GrantAbsent,            // 400 unsupported_grant_type, or 404 at the endpoint
    PasswordRequired,       // password_hash_b64_required
    PasswordInvalid,        // password_hash_b64_invalid
    EmailRequired,          // email_required | email_and_otp_required
    SendGone,               // send_id_invalid
    Other(RestError),
}

impl RestClient {
    /// `POST {base}/identity/connect/token`, form-encoded, on `anon_agent`.
    /// Fields pinned against `SendAccessConstants.TokenRequest`.
    pub fn mint_send_access_token(&self, access_id: &str, password_hash: Option<&str>)
        -> Result<SendAccessToken, SendGrantRefusal>;

    /// `POST {base}/api/sends/access`, empty body, `Authorization: Bearer` the
    /// SEND token. Never a `Session`.
    pub fn access_send_with_token(&self, token: &SendAccessToken) -> Result<Value, RestError>;

    /// `POST {base}/api/sends/access/{access_id}` with `{"password": …}` and a
    /// `Send-Id` header. Anonymous. Never a `Session`.
    pub fn access_send_anonymously(&self, access_id: &str, access: &MappedSendAccess)
        -> Result<Value, RestError>;
}

/// The legacy body, mapped rather than hand-built, so `rest::api`'s census
/// keeps `send_json(&body) == 0`. Its only constructor hashes.
pub struct MappedSendAccess { body: Value }
impl MappedSendAccess { pub(crate) fn for_key(key: &SendKey, password: Option<&str>) -> Self; }

// deskwarden/src/rest/send.rs — new.

/// The whole receive, both paths, one answer.
pub fn receive(client: &RestClient, link: &SendLink, password: Option<&str>)
    -> Result<Zeroizing<String>, SendError>;

/// The vault-window door, matching `crate::send::cli_send_receive`'s signature
/// in everything that matters: no session, and a `Zeroizing<String>` out.
pub fn receive_on_active_account(url: &str, password: Option<&str>)
    -> Result<Zeroizing<String>, SendError>;
```

---

### Task 1: The link, taken apart, with nothing on the wire

**Files:** `deskwarden/src/rest/send_link.rs` (new), `deskwarden/src/rest/mod.rs`, `deskwarden/src/rest/send_crypto.rs`

**Interfaces**

- *Consumes:* `send_crypto::SendKey`, `crate::favicon::host_from_url`, `crate::send::SendError`.
- *Produces:* `SendLink`, `parse`.

The parse is first and is pure, so the trust decision in it is under oath
before any request exists that could be pointed at the wrong host.
`SendKey::from_bytes` becomes `pub(crate)`; nothing else in `send_crypto.rs`
moves.

- [ ] **Step 1: Write the failing tests**

```rust
    /// Both link shapes the official client accepts, because it takes the
    /// LAST TWO fragment segments -- `receive.command.ts`'s `getIdAndKey`.
    /// A parser that split on a fixed `#/send/` prefix passes the first of
    /// these and fails the second, which is why both are here.
    #[test]
    fn the_id_and_key_are_the_last_two_fragment_segments_in_both_link_shapes() { … }

    /// **The refusal that matters.** A link whose origin is not the account's
    /// configured server is refused, and the two hosts are both named in the
    /// sentence -- a refusal that says only "bad link" sends the user looking
    /// at the key.
    ///
    /// Control, in the same test: the identical link on the CONFIGURED origin
    /// parses. Without it this passes for a parser that refuses everything.
    #[test]
    fn a_link_on_a_host_that_is_not_the_configured_server_is_refused_and_the_same_link_on_it_is_not() { … }

    /// Exact origin, never a suffix: `vault.example.com.evil.test` and
    /// `evil-vault.example.com` are both refused against `vault.example.com`.
    /// `backend_policy::is_self_hosted`'s rule, and its stated reason.
    #[test]
    fn a_host_that_merely_contains_the_configured_one_is_a_different_host() { … }

    /// A fragment that is not 16 bytes is refused rather than padded.
    /// Controls: the 22-character key from `SendKey::fragment()` on a known
    /// 16 bytes parses, and round-trips to the same bytes.
    #[test]
    fn a_key_of_any_length_but_sixteen_bytes_is_refused_and_the_right_one_round_trips() { … }

    /// `SendLink` has no `Debug`, checked the way the crate already checks it
    /// for `Challenge`: a source read asserting the derive is absent, with the
    /// positive control that some OTHER derive on the same type is found.
    #[test]
    fn the_parsed_link_cannot_be_written_to_a_log() { … }
```

- [ ] **Step 2: Implement** `send_link.rs`, and declare it in `rest/mod.rs`.
- [ ] **Step 3: Verify** `cargo test -p deskwarden send_link` — all pass, and deliberately break the origin check once to confirm the refusal test reddens.
- [ ] **Step 4: Commit** — `The link, taken apart before anything is sent`

---

### Task 2: The grant, and the one refusal that may fall back

**Files:** `deskwarden/src/rest/api.rs`

**Interfaces**

- *Consumes:* `crate::http_agent::bounded_total`.
- *Produces:* `anon_agent`, `SendAccessToken`, `SendGrantRefusal`, `mint_send_access_token`.

This is the task the whole design turns on. `GrantAbsent` is the fallback
trigger and it must be reachable from **exactly two** server answers.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The form the grant puts on the wire, field by field, against
    /// `SendAccessConstants.TokenRequest` and `receive.command.ts` on
    /// `clients` main. Named here because `client_id` and `scope` are NOT in
    /// the server's constants file and have no other pin.
    #[test]
    fn the_send_access_grant_sends_every_field_the_identity_server_requires() {
        // grant_type=send_access, client_id=send, scope=api.send.access,
        // send_id={access_id}, and password_hash_b64 only when there is one.
    }

    /// **The classification table, every arm.** One mock per
    /// `send_access_error_type`, and the assertion is on the VARIANT, not on
    /// a string. A design that read these as one "refused" is a design that
    /// asks for a password when the Send is gone.
    #[test]
    fn every_named_send_access_error_maps_to_its_own_refusal() {
        // password_hash_b64_required -> PasswordRequired
        // password_hash_b64_invalid  -> PasswordInvalid
        // email_required             -> EmailRequired
        // email_and_otp_required     -> EmailRequired
        // send_id_invalid            -> SendGone
    }

    /// **The fallback trigger is exactly two answers.**
    ///
    /// Positive half: `400 {"error":"unsupported_grant_type"}` and a bare
    /// `404` at `/identity/connect/token` each give `GrantAbsent`.
    /// Negative half, in the SAME test: every arm of the table above, plus a
    /// `500`, plus a transport failure, gives something that is NOT
    /// `GrantAbsent`. Without the negative half this passes for a mapper that
    /// answers `GrantAbsent` to everything -- which would send a user with a
    /// wrong password down the legacy route on a server that has no legacy
    /// route, and answer "this Send is gone".
    #[test]
    fn only_a_server_that_does_not_know_the_grant_may_send_us_to_the_old_route() { … }

    /// A token with no finite `expires_in` is refused, not treated as
    /// eternal. `api.rs`'s existing `expires_in` guard is the precedent and
    /// the reason is `receive.command.ts`'s own comment.
    #[test]
    fn a_grant_answer_with_no_usable_expiry_is_refused() { … }

    /// The password hash reaches the form and nothing else does: no master
    /// password, no session token, no vault data. Control: the hash IS found
    /// in the body, so a test searching an empty body cannot pass.
    #[test]
    fn the_grant_carries_the_send_password_hash_and_no_vault_secret() { … }
```

- [ ] **Step 2: Implement.** Add `anon_agent` beside `auth_agent`/`write_agent` with its own deadline, sized to exceed the server's deliberate 2-second wrong-password delay.
- [ ] **Step 3: Verify.**
- [ ] **Step 4: Commit** — `A token for one Send, and the one answer that means the old route`

---

### Task 3: The two access routes, and a body that is not hand-built

**Files:** `deskwarden/src/rest/api.rs`

**Interfaces**

- *Produces:* `MappedSendAccess`, `access_send_with_token`, `access_send_anonymously`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// `POST /api/sends/access` with the SEND token in the bearer -- and the
    /// user's session nowhere. The mock matches the exact path (no `{id}`
    /// segment) and the exact header.
    #[test]
    fn the_token_route_posts_to_the_bare_access_path_with_the_send_bearer() { … }

    /// `POST /api/sends/access/{accessId}` with `{"password": …}` and the
    /// `Send-Id` header, matching `send-api.service.ts` at `cli-v2025.8.0`.
    /// Two mocks: with a password and without, because `null` and a hash are
    /// different bodies and only one of them is what a server without a
    /// password expects.
    #[test]
    fn the_anonymous_route_posts_the_password_body_to_the_id_path() { … }

    /// **Neither anonymous route may be handed the user's vault token.**
    /// A `Session` is granted in this test and deliberately NOT passed; the
    /// mocks assert `Authorization` is either absent or the send token, and
    /// never `Bearer AT-1`.
    ///
    /// Control, in the same test: a `create_send` against the same server
    /// DOES carry `Bearer AT-1`. A matcher that never fires would otherwise
    /// pass both halves.
    #[test]
    fn the_vault_session_never_reaches_a_request_addressed_by_a_link() { … }
```

- [ ] **Step 2: Implement.** `MappedSendAccess::for_key` is the only constructor and it calls `SendKey::password_hash`; the routes take `&MappedSendAccess`, so no caller can assemble the body.
- [ ] **Step 3: Verify.**
- [ ] **Step 4: Commit** — `Two routes to one answer, and a body no caller can build`

---

### Task 4: The census asserts more

**Files:** `deskwarden/src/rest/api.rs`

`the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin`
(`api.rs:2940`) currently pins `send_json(` at 7, `send_json(&body)` at 0, and
the write agent's body-carrying calls at 5. Every change below is in the
direction of asserting **more**; none is an allowance.

- [ ] **Step 1: Extend the census**

- `send_json(` goes 7 → 8, and the comment gains a named eighth: the legacy
  Send access body, which is a `MappedSendAccess`. Add
  `assert_eq!(production.matches("send_json(access.body())").count(), 1)`.
- `send_json(&body)` **stays 0**. If it does not, `MappedSendAccess` was not
  used and the task is wrong.
- The write agent's count **stays 5**. A receive that moved it was handed a
  session.
- **New: enumerate `send_form(` too.** The census cannot see a form body, and
  a count that cannot see a body can be evaded. Assert the total and name each
  site: the password grant, the API-key grant, the refresh, and the
  send-access grant.

- [ ] **Step 2: Add the anonymous-agent source pin**

```rust
    /// **No anonymous route can reach the user's session.**
    ///
    /// A type could not say this: `bearer` takes a `&Session` and any of the
    /// three new functions could have been given one. So the production half
    /// is read.
    ///
    /// Positive control, and it is the whole point of the test: the same read
    /// asserts `self.bearer(self.write_agent` is NON-zero. A slice that had
    /// been cut wrong, or a needle spelled differently from the source, makes
    /// every count zero -- and the zero assertions below would all pass while
    /// reaching nothing. That is this house's named defect and this is where
    /// it would live.
    #[test]
    fn nothing_on_the_anonymous_agent_is_given_the_vault_session() {
        assert!(production.matches("self.bearer(self.write_agent").count() > 0, "control: …");
        assert_eq!(production.matches("self.bearer(self.anon_agent").count(), 0, "…");
        assert!(production.matches("anon_agent").count() >= 3, "control: …");
    }
```

- [ ] **Step 3: Verify.** Run the census test alone and confirm it reddens when `send_json(` is left at 7.
- [ ] **Step 4: Commit** — `The census learns the fourth agent and the forms`

---

### Task 5: The probe, and the sentence for every way it can end

**Files:** `deskwarden/src/rest/send.rs`

**Interfaces**

- *Produces:* `receive`, the response parse, the refusals.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **The happy path on a NEW server**: grant, then the bare access path,
    /// then the text. Two mocks, both `.expect(1)`, and the legacy path is
    /// mocked to PANIC-loud (`.expect(0)`) so a probe that called it anyway
    /// fails here rather than silently costing a request.
    #[test]
    fn a_server_that_mints_a_token_is_never_asked_the_old_route() { … }

    /// **The happy path on an OLD server**: the grant answers
    /// `unsupported_grant_type`, the legacy route answers, the text comes
    /// out, and the text is IDENTICAL to the one the new-server test got from
    /// the same fixture Send. One assertion, two paths -- which is the whole
    /// user-facing claim of the design's §2.4.
    #[test]
    fn an_old_server_yields_the_same_text_through_the_fallback() { … }

    /// **A server with neither route is refused by name.** `404` at identity
    /// AND `404` at the legacy path is not "this Send is gone"; it is "this
    /// server does not offer Send links to this app", and the sentence says
    /// so. Control: the same fixture with the legacy route present succeeds,
    /// so this is about the server and not about the fixture.
    #[test]
    fn a_server_that_speaks_neither_route_says_so_rather_than_blaming_the_link() { … }

    /// The password path, on both routes, from the same two inputs:
    /// `401` (legacy) and `password_hash_b64_required` (grant) both mean
    /// "ask", and a wrong password on either says "wrong password" and never
    /// "gone". Four cases, one table.
    #[test]
    fn a_password_protected_send_asks_once_and_names_a_wrong_password_on_both_routes() { … }

    /// An email-gated Send is refused with a sentence naming e-mail proof,
    /// and that sentence is NOT the "gone" sentence. Asserted as an
    /// inequality between the two strings, so a later edit cannot quietly
    /// collapse them.
    #[test]
    fn an_email_gated_send_is_not_reported_as_a_dead_link() { … }

    /// A `type` that is not 0 is refused by name -- a file Send -- and not
    /// decrypted into nonsense.
    #[test]
    fn a_file_send_is_refused_in_its_own_words() { … }

    /// **One parser for both eras.** The same `SendAccessResponseModel`
    /// fixture is fed through the token path and the legacy path and must
    /// produce byte-identical output. This is the guard against the two paths
    /// growing separate parsers.
    #[test]
    fn both_routes_are_read_by_the_same_parser() { … }

    /// A transport failure is `Offline` and never `TimedOut`: a receive
    /// publishes nothing, so `Ambiguity::Ambiguous` must be unreachable here.
    /// Control: the same helper with `Ambiguous` DOES give `TimedOut`, so the
    /// test is about this call site and not about a broken mapper.
    #[test]
    fn a_receive_can_never_report_a_link_it_might_have_published() { … }
```

- [ ] **Step 2: Implement** `receive`, plus `receive_on_active_account` built the way `create_on_active_account` is — client and credentials assembled per operation, never held. It needs **no `VaultKeys` and no `/api/sync`**: the key is in the link. Say so in the doc comment, as `delete_on_active_account` says it.
- [ ] **Step 3: Verify.**
- [ ] **Step 4: Commit** — `A Send read from a link, on either server`

---

### Task 6: The window branches, and the sentence narrows

**Files:** `deskwarden/src/vault_window/send_ui.rs`, `deskwarden/src/rest/mod.rs`

`RECEIVE_NEEDS_THE_CLI` (`send_ui.rs:80`) is currently unconditional. It must
survive, unchanged in wording, on the `BwServe` arm — a user on the official
CLI is not affected by this branch — and must become unreachable on
`DirectRest`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The branch, and both halves of it. `DirectRest` reaches
    /// `rest::send::receive_on_active_account`; `BwServe` reaches
    /// `cli_send_receive`. Driven through an `fn`-pointer seam in the shape of
    /// `login_ui::SecondFactorSeam` -- NOT a `cfg(test)` hook, which this
    /// crate bans.
    #[test]
    fn the_receive_goes_to_the_backend_the_policy_chose() { … }

    /// `RECEIVE_NEEDS_THE_CLI` is still the exact string, still says what is
    /// missing, and still names the three operations that keep working -- the
    /// existing guards at `send_ui.rs:2524`, `2529` and `2540` keep their
    /// needles. What is new: it is unreachable on the `DirectRest` arm.
    /// Control: it IS reached on the `BwServe` arm, in the same test.
    #[test]
    fn the_cli_sentence_survives_for_the_cli_and_is_gone_for_the_built_in_client() { … }
```

- [ ] **Step 2: Implement** the seam and the branch. Every source guard in
  `send_ui.rs` that counts mentions of `cli_send_receive` keeps its needle
  list — no needle moves, because `crate::send` is not edited.
- [ ] **Step 3: Update `rest/mod.rs`'s module doc.** Receive leaves the
  "still missing" list; `bw.exe` remains on the machine for **attachments**
  and **organisations**. The doc already says a list of what is missing is a
  promise to keep current, and it already carries the scar of not having been.
- [ ] **Step 4: Verify** the full suite: `cargo test -p deskwarden`.
- [ ] **Step 5: Commit** — `The link opens without the CLI, and the sentence stays for the CLI`

---

### Task 7: The live check, which is the only thing that can settle §6

**Files:** none — a report.

The design's §6 says plainly what code cannot: nothing in Bitwarden's source
says which route the account's own server (a third-party implementation)
answers. This task is to find out and write it down.

- [ ] **Step 1:** Create a text Send on the account, with a password, through
  the app's own create path. Receive it. Record which route answered.
- [ ] **Step 2:** Repeat without a password, and with a link whose max access
  count is exhausted.
- [ ] **Step 3:** Record the finding in the spec's §6 — replacing "this does
  not settle" with what was observed, or, if the server answers neither
  route, leaving the refusal path as the shipped behaviour and saying so.
- [ ] **Step 4: Commit** — `What the server actually answered`

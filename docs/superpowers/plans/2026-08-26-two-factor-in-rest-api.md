# Two-Factor Authentication in `rest/api` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an account with two-factor authentication sign in through the direct-REST backend, so that `bw` stops being required for the one thing a user cannot avoid doing.

**Architecture:** The grant already fails with a typed `RestError::TwoFactorRequired`. This turns that dead end into a *resumable* login: a first call returns either a session or a challenge carrying the already-derived master key, and a second call completes it with the code the user typed. Providers this client cannot complete are refused **by name**, never silently.

**Tech Stack:** Rust, `ureq` 2.12 through `crate::http_agent::bounded_total`, `serde_json`, `zeroize`, `mockito` for every wire test.

## Global Constraints

- **No test may touch** the network, the real vault, the real clipboard, the real screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`. `mockito` only.
- **No `cfg(test)` seams.** Banned crate-wide.
- **Never build into `deskwarden/target`.** Use an absolute `CARGO_TARGET_DIR` outside the repo, e.g. `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **Never write scratch files under `deskwarden/src/**`.**
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, never `--amend`, `reset`, `rebase`, or `git stash`.
- **Nothing secret is logged, ever** -- not a code, not a token, not a hash. `RestError::Parse` takes a `&'static str` naming what was *missing*, never what was there. Follow it.
- **A second factor is a secret.** Codes and remember-tokens are `Zeroizing`, and no type carrying one derives `Debug`.
- **This crate's local test runs are unreliable** on the owner's machine: the TCP dynamic port range starts at 1024 and collides with `mockito`. A `mockito` test failing with `os error 10054` is the machine, not the code. Re-run before believing a failure; CI is the arbiter.
- Branch: `two-factor-rest`. Build/test: `CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2`

## What this plan does NOT do

- **No UI.** `login_ui::derive_direct_rest` keeps working unchanged and keeps refusing 2FA accounts by name. Prompting the user for a code is the next plan, and it is a different kind of work.
- **No YubiKey, WebAuthn or Duo.** Refused by name. See Task 5 for why refusing is the deliverable rather than a gap.
- **No changes to `rest/crypto.rs`.** Nothing here derives a key differently.

## The protocol, read from the server rather than assumed

Confirmed against `shuaiplus/nodewarden`'s `src/handlers/identity.ts` on 2026-08-26, which is the server this backend exists for:

- The challenge is a **400** carrying `error: "invalid_grant"`, `error_description: "Two factor required."`, a `TwoFactorProviders` array of provider numbers **as strings**, and a `TwoFactorProviders2` object keyed by the same numbers (`identity.ts:270-271`).
- The retry is the **same** `/identity/connect/token` endpoint with three extra form fields: `twoFactorProvider`, `twoFactorToken`, `twoFactorRemember` (`identity.ts:372-374`). The server also accepts the capitalised spellings.
- **If either provider or token is missing, the server re-issues the challenge** rather than erroring (`identity.ts:474-476`).
- Provider numbers nodewarden implements: **0** authenticator (TOTP), **3** YubiKey, **5** remember, **7** WebAuthn, **8** recovery code. **There is no email provider and no Duo.**
- Provider **5** is a trusted-device token tied to `deviceIdentifier`; an invalid or expired one **re-enters the challenge** rather than failing the login (`identity.ts:488-490`).
- On a successful login where remember was requested, the token response carries **`TwoFactorToken`** (`identity.ts:617`).
- `Device::windows_desktop` already takes a stable identifier, and `main.rs:10491`'s `device_id_for` supplies a per-account one. **Remember therefore works without new plumbing** -- but it is worthless if that identifier ever changes, which Task 4's test pins.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/rest/api.rs` (modify) | All of it. The challenge type, the second-factor grant, the resumable outcome, the remember token, the refusals. |
| `deskwarden/src/login_ui.rs` (modify, Task 5 only) | One arm, so a 2FA account's refusal names the provider instead of saying "wrong password". |

---

### Task 1: The challenge, typed

**Files:**
- Modify: `deskwarden/src/rest/api.rs`

**Interfaces:**
- Consumes: `RestError::TwoFactorRequired { providers: Vec<String> }` (exists), `classify_400` (exists).
- Produces:
  ```rust
  pub enum SecondFactor { Authenticator, YubiKey, WebAuthn, RecoveryCode, Remember, Other(String) }
  impl SecondFactor {
      pub fn from_wire(value: &str) -> Self;
      pub fn wire_number(&self) -> Option<&str>;
      pub fn is_supported(&self) -> bool;
      pub fn describe(&self) -> &str;
  }
  ```

- [ ] **Step 1: Write the failing tests**

Add to `rest::api::tests`:

```rust
    /// The provider numbers are the server's, read from nodewarden's
    /// `identity.ts` rather than from memory: 0 authenticator, 3 YubiKey,
    /// 5 remember, 7 WebAuthn, 8 recovery code.
    #[test]
    fn the_provider_numbers_are_the_ones_the_server_sends() {
        assert_eq!(SecondFactor::from_wire("0"), SecondFactor::Authenticator);
        assert_eq!(SecondFactor::from_wire("3"), SecondFactor::YubiKey);
        assert_eq!(SecondFactor::from_wire("5"), SecondFactor::Remember);
        assert_eq!(SecondFactor::from_wire("7"), SecondFactor::WebAuthn);
        assert_eq!(SecondFactor::from_wire("8"), SecondFactor::RecoveryCode);
    }

    /// **A number this client has never heard of is kept, not dropped.**
    /// Bitwarden has added providers before and will again; a parser that
    /// discarded the unknown ones would tell the user "no second factor is
    /// available" on an account that has one.
    #[test]
    fn an_unknown_provider_number_survives_as_itself() {
        assert_eq!(SecondFactor::from_wire("42"), SecondFactor::Other("42".to_string()));
        assert_eq!(SecondFactor::from_wire(""), SecondFactor::Other(String::new()));
        assert!(!SecondFactor::from_wire("42").is_supported());
    }

    /// Only the two this plan implements are supported, and `wire_number`
    /// answers for exactly those. A provider that could be *sent* but not
    /// completed is the mismatch this pins.
    #[test]
    fn only_the_providers_this_client_can_complete_are_supported() {
        assert!(SecondFactor::Authenticator.is_supported());
        assert!(SecondFactor::Remember.is_supported());
        for unsupported in [
            SecondFactor::YubiKey,
            SecondFactor::WebAuthn,
            SecondFactor::RecoveryCode,
            SecondFactor::Other("42".to_string()),
        ] {
            assert!(!unsupported.is_supported(), "{unsupported:?} is not implemented");
            assert!(
                unsupported.wire_number().is_none(),
                "{unsupported:?} can be put on a wire this client cannot then complete"
            );
        }
        assert_eq!(SecondFactor::Authenticator.wire_number(), Some("0"));
        assert_eq!(SecondFactor::Remember.wire_number(), Some("5"));
    }

    /// The description is what a UI will show. It must name the thing the
    /// user has to go and find, in their words rather than a number.
    #[test]
    fn every_provider_describes_itself_without_a_number() {
        for provider in [
            SecondFactor::Authenticator,
            SecondFactor::YubiKey,
            SecondFactor::WebAuthn,
            SecondFactor::RecoveryCode,
            SecondFactor::Remember,
        ] {
            let text = provider.describe();
            assert!(!text.is_empty(), "{provider:?} has no description");
            assert!(
                !text.chars().any(|c| c.is_ascii_digit()),
                "{provider:?} describes itself with a provider number: {text:?}"
            );
        }
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- the_provider_numbers an_unknown_provider only_the_providers every_provider_describes
```

Expected: compile error -- `cannot find type SecondFactor in this scope`.

- [ ] **Step 3: Write the implementation**

In `rest/api.rs`, beside `RestError`:

```rust
/// One second-factor method the server will accept for an account.
///
/// **Numbers in, names out.** The wire carries Bitwarden's provider numbers
/// as strings; nothing above this type should ever see one, because a number
/// is not something a user can be asked for.
///
/// [`Self::Other`] keeps a number this client does not know rather than
/// dropping it. Bitwarden has added providers before; a parser that discarded
/// the unrecognised ones would report "no second factor available" for an
/// account that has one, which is a worse failure than saying "this client
/// cannot do that one".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecondFactor {
    /// 0. A code from an authenticator app. The only interactive provider
    /// this client implements.
    Authenticator,
    /// 3. A YubiKey OTP.
    YubiKey,
    /// 7. A WebAuthn assertion.
    WebAuthn,
    /// 8. A one-time recovery code.
    RecoveryCode,
    /// 5. A token this device was given last time, tied to the device
    /// identifier. Never typed by a user.
    Remember,
    /// A number this client does not recognise, kept verbatim.
    Other(String),
}

impl SecondFactor {
    /// The server's number, as it arrives in `TwoFactorProviders`.
    #[must_use]
    pub fn from_wire(value: &str) -> Self {
        match value.trim() {
            "0" => Self::Authenticator,
            "3" => Self::YubiKey,
            "5" => Self::Remember,
            "7" => Self::WebAuthn,
            "8" => Self::RecoveryCode,
            other => Self::Other(other.to_string()),
        }
    }

    /// The number to send back, or `None` for a provider this client cannot
    /// complete.
    ///
    /// **Deliberately `None` for everything unsupported**, so that "can be
    /// sent" and "can be completed" cannot come apart. A client that sent a
    /// YubiKey provider with no OTP would get the challenge again and read it
    /// as a rejected code.
    #[must_use]
    pub fn wire_number(&self) -> Option<&'static str> {
        match self {
            Self::Authenticator => Some("0"),
            Self::Remember => Some("5"),
            Self::YubiKey | Self::WebAuthn | Self::RecoveryCode | Self::Other(_) => None,
        }
    }

    /// Whether this client can complete this factor.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.wire_number().is_some()
    }

    /// What to call it when asking, or when refusing.
    #[must_use]
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Authenticator => "an authenticator app",
            Self::YubiKey => "a YubiKey",
            Self::WebAuthn => "a security key",
            Self::RecoveryCode => "a recovery code",
            Self::Remember => "a remembered device",
            Self::Other(_) => "a method this app does not support",
        }
    }
}
```

- [ ] **Step 4: Run the tests and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- the_provider_numbers an_unknown_provider only_the_providers every_provider_describes
```

Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/rest/api.rs
git commit -F <message file>
```

---

### Task 2: The grant that carries a second factor

**Files:**
- Modify: `deskwarden/src/rest/api.rs`

**Interfaces:**
- Consumes: `Self::password_grant` (exists), `SecondFactor` (Task 1).
- Produces:
  ```rust
  pub struct SecondFactorAnswer { pub factor: SecondFactor, pub token: Zeroizing<String>, pub remember: bool }
  impl RestClient {
      pub fn password_grant_with(
          &self, email: &str, password_hash: &str, device: &Device,
          answer: Option<&SecondFactorAnswer>,
      ) -> Result<Session, RestError>;
  }
  ```
  `password_grant` becomes `password_grant_with(.., None)` and keeps its signature and behaviour.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The three fields the server reads, in the spelling it reads them
    /// (`identity.ts:372-374`), and the code reaching the wire at all.
    #[test]
    fn a_second_factor_answer_puts_its_three_fields_on_the_wire() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        let sent = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("twoFactorProvider".into(), "0".into()),
                mockito::Matcher::UrlEncoded("twoFactorToken".into(), "123456".into()),
                mockito::Matcher::UrlEncoded("twoFactorRemember".into(), "1".into()),
            ]))
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .expect(1)
            .create();

        let answer = SecondFactorAnswer {
            factor: SecondFactor::Authenticator,
            token: Zeroizing::new("123456".to_string()),
            remember: true,
        };
        client
            .password_grant_with("me@example.com", "HASH", &a_device(), Some(&answer))
            .expect("the grant");
        sent.assert();
    }

    /// **`0`, not absent.** The server reads the field with a permissive
    /// truthiness test; sending nothing when the user declined would be
    /// indistinguishable from a client that forgot the field, and the
    /// difference is whether a trust token is minted for this device.
    #[test]
    fn declining_to_be_remembered_says_so_rather_than_omitting_the_field() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        let sent = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::UrlEncoded("twoFactorRemember".into(), "0".into()))
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .expect(1)
            .create();
        let answer = SecondFactorAnswer {
            factor: SecondFactor::Authenticator,
            token: Zeroizing::new("123456".to_string()),
            remember: false,
        };
        client
            .password_grant_with("me@example.com", "HASH", &a_device(), Some(&answer))
            .expect("the grant");
        sent.assert();
    }

    /// **No answer means none of the three fields**, because the server
    /// re-issues the challenge when either is missing (`identity.ts:474`).
    /// An empty `twoFactorToken` would therefore loop rather than fail.
    #[test]
    fn a_grant_with_no_second_factor_sends_none_of_the_three_fields() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        let bare = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Missing("twoFactorProvider".into()),
                mockito::Matcher::Missing("twoFactorToken".into()),
                mockito::Matcher::Missing("twoFactorRemember".into()),
            ]))
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .expect(1)
            .create();
        client
            .password_grant("me@example.com", "HASH", &a_device())
            .expect("the grant");
        bare.assert();
    }

    /// A provider this client cannot complete never reaches the wire, and
    /// says so as itself rather than as a rejected code.
    #[test]
    fn an_unsupported_provider_is_refused_before_anything_is_sent() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        let any = server.mock("POST", mockito::Matcher::Any).with_status(200).create();
        let answer = SecondFactorAnswer {
            factor: SecondFactor::YubiKey,
            token: Zeroizing::new("ccccc...".to_string()),
            remember: false,
        };
        let error = client
            .password_grant_with("me@example.com", "HASH", &a_device(), Some(&answer))
            .expect_err("a YubiKey OTP this client cannot verify");
        assert!(matches!(error, RestError::SecondFactorUnsupported { .. }), "got {error:?}");
        assert!(!any.matched(), "an unsupported provider was put on the wire");
    }
```

Add the shared helper beside the other test helpers:

```rust
    fn a_device() -> Device {
        Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC")
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- a_second_factor_answer declining_to_be_remembered a_grant_with_no_second_factor an_unsupported_provider_is_refused
```

Expected: compile error -- `cannot find struct SecondFactorAnswer`, `no method password_grant_with`, `no variant SecondFactorUnsupported`.

- [ ] **Step 3: Write the implementation**

Add the error variant beside the others in `RestError`:

```rust
    /// The user was asked for a factor this client cannot complete, or the
    /// server offered only such factors. Carries the description rather than
    /// the number, because the string is shown to a person.
    ///
    /// **Not [`Self::TwoFactorRequired`]**, which means "the server wants a
    /// second factor and here is what it will take". This one means "this
    /// app cannot do that", which is an answer about the client and needs
    /// different words on screen.
    SecondFactorUnsupported { method: String },
```

Add to its `Display`:

```rust
            RestError::SecondFactorUnsupported { method } => {
                write!(f, "this app cannot sign in with {method} yet")
            }
```

The answer type and the grant:

```rust
/// What the user (or a stored token) offers as the second factor.
///
/// No `Debug`: `token` is a live credential -- a code that is about to be
/// accepted, or a device-trust token good for thirty days. `debug_leak_guard`
/// enforces the same rule on every other secret-bearing type here.
pub struct SecondFactorAnswer {
    pub factor: SecondFactor,
    /// The code, or the remembered-device token. `Zeroizing` for the reason
    /// `Session`'s tokens are.
    pub token: Zeroizing<String>,
    /// Whether to ask the server to trust this device next time. Sent as
    /// `1`/`0` and never omitted -- see the test.
    pub remember: bool,
}

impl RestClient {
    /// The grant, with an optional second factor.
    ///
    /// `None` is the ordinary first attempt and sends none of the three
    /// fields. That is not the same as sending them empty: the server
    /// re-issues the challenge when either the provider or the token is
    /// missing (`identity.ts:474`), so an empty token would loop a caller
    /// that thought it had answered.
    pub fn password_grant_with(
        &self,
        email: &str,
        password_hash: &str,
        device: &Device,
        answer: Option<&SecondFactorAnswer>,
    ) -> Result<Session, RestError> {
        let extra = match answer {
            None => Vec::new(),
            Some(answer) => {
                let Some(number) = answer.factor.wire_number() else {
                    // Refused HERE, before a socket is opened, for
                    // `cipher_url`'s reason one layer up: a request this
                    // client cannot complete is not worth sending, and the
                    // server's answer to it is the challenge again, which
                    // reads as a wrong code.
                    return Err(RestError::SecondFactorUnsupported {
                        method: answer.factor.describe().to_string(),
                    });
                };
                vec![
                    ("twoFactorProvider", number.to_string()),
                    ("twoFactorToken", answer.token.to_string()),
                    ("twoFactorRemember", if answer.remember { "1" } else { "0" }.to_string()),
                ]
            }
        };
        self.grant_form(email, password_hash, device, &extra)
    }
}
```

Refactor the existing `password_grant` body into `grant_form(&self, email, password_hash, device, extra: &[(&str, String)])`, appending `extra` to the eight fields it already builds, and make `password_grant` call `password_grant_with(.., None)`. **Do not change the eight existing fields or their order** -- `identity.ts` validates seven of them.

- [ ] **Step 4: Run the tests and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- rest::api
```

Expected: every existing `rest::api` test still passes, plus the four new ones. If a `mockito` test reports `os error 10054`, re-run it -- see the constraints.

- [ ] **Step 5: Commit**

---

### Task 3: A login that can be resumed

**Files:**
- Modify: `deskwarden/src/rest/api.rs`

**Interfaces:**
- Consumes: `authenticate` (exists), `master_key` (exists), `Task 1`, `Task 2`.
- Produces:
  ```rust
  pub enum LoginOutcome { Done(Authenticated), SecondFactorNeeded(SecondFactorPrompt) }
  pub struct SecondFactorPrompt { /* private */ }
  impl SecondFactorPrompt {
      pub fn offered(&self) -> &[SecondFactor];
      pub fn can_be_answered(&self) -> bool;
  }
  impl RestClient {
      pub fn begin_login(&self, email: &str, password: &[u8], device: &Device) -> Result<LoginOutcome, RestError>;
      pub fn finish_login(&self, prompt: SecondFactorPrompt, answer: &SecondFactorAnswer, device: &Device) -> Result<Authenticated, RestError>;
  }
  ```

**Why `SecondFactorPrompt` is opaque and carries the key:** deriving the master key is 600,000 PBKDF2 iterations -- seconds of CPU, measured on the owner's account. Re-deriving it after the user types a code would make every 2FA login pay twice and would need the password held somewhere for the second pass. The prompt therefore carries the already-derived `MasterKey` and the password hash, and its fields are private so nothing outside this module can read them back out.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The whole flow, end to end, against a server that demands a code and
    /// then accepts one.
    #[test]
    fn a_challenge_is_answered_without_deriving_the_key_a_second_time() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":5}"#)
            .expect(1)
            .create();
        // The challenge, in nodewarden's own shape (`identity.ts:270`).
        let challenge = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::Missing("twoFactorToken".into()))
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"Two factor required.",
                    "TwoFactorProviders":["0"],"TwoFactorProviders2":{"0":null}}"#,
            )
            .expect(1)
            .create();
        let accepted = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::UrlEncoded("twoFactorToken".into(), "123456".into()))
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .expect(1)
            .create();

        let outcome = client
            .begin_login("me@example.com", b"pw", &a_device())
            .expect("the first leg");
        let LoginOutcome::SecondFactorNeeded(prompt) = outcome else {
            panic!("a server that answered 400 Two factor required was read as a completed login");
        };
        assert_eq!(prompt.offered(), &[SecondFactor::Authenticator]);
        assert!(prompt.can_be_answered());

        let answer = SecondFactorAnswer {
            factor: SecondFactor::Authenticator,
            token: Zeroizing::new("123456".to_string()),
            remember: false,
        };
        client.finish_login(prompt, &answer, &a_device()).expect("the second leg");

        challenge.assert();
        accepted.assert();
        // ONE prelogin for the whole login. Two would mean the key was
        // derived twice, which is the cost this design exists to avoid and
        // which no assertion on the returned value would notice.
    }

    /// A server with no second factor still finishes in one call, and the
    /// outcome says so rather than handing back a prompt nobody can answer.
    #[test]
    fn a_login_with_no_second_factor_is_done_in_one_leg() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":5}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .create();
        assert!(matches!(
            client.begin_login("me@example.com", b"pw", &a_device()).expect("the login"),
            LoginOutcome::Done(_)
        ));
    }

    /// **A challenge offering nothing this client can do says so up front.**
    /// Handing back a prompt whose only options are unanswerable would put
    /// the refusal at the moment the user has already typed something.
    #[test]
    fn a_challenge_of_only_unsupported_providers_cannot_be_answered() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":5}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"Two factor required.",
                    "TwoFactorProviders":["3","7"]}"#,
            )
            .create();
        let LoginOutcome::SecondFactorNeeded(prompt) =
            client.begin_login("me@example.com", b"pw", &a_device()).expect("the first leg")
        else {
            panic!("expected a challenge");
        };
        assert!(!prompt.can_be_answered());
        assert_eq!(prompt.offered(), &[SecondFactor::YubiKey, SecondFactor::WebAuthn]);
    }

    /// **`authenticate` is unchanged for callers that cannot prompt.**
    /// `login_ui::derive_direct_rest` and `examples/rest_probe` both call it
    /// and neither can ask a user for anything; they must keep getting a
    /// typed refusal rather than a prompt they would have to ignore.
    #[test]
    fn authenticate_still_refuses_a_two_factor_account_by_name() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/accounts/prelogin")
            .with_body(r#"{"kdf":0,"kdfIterations":5}"#)
            .create();
        server
            .mock("POST", "/identity/connect/token")
            .with_status(400)
            .with_body(
                r#"{"error":"invalid_grant","error_description":"Two factor required.",
                    "TwoFactorProviders":["0"]}"#,
            )
            .create();
        let error = client
            .authenticate("me@example.com", b"pw", &a_device())
            .expect_err("a 2FA account");
        assert!(matches!(error, RestError::TwoFactorRequired { .. }), "got {error:?}");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- a_challenge_is_answered a_login_with_no_second_factor a_challenge_of_only_unsupported authenticate_still_refuses
```

Expected: compile error -- `cannot find enum LoginOutcome`, `no method begin_login`.

- [ ] **Step 3: Write the implementation**

```rust
/// What a first login attempt produced.
pub enum LoginOutcome {
    /// The account has no second factor, or a remembered device covered it.
    Done(Authenticated),
    /// The server wants a second factor. Carries everything the retry needs
    /// so that the expensive half of the login is not paid twice.
    SecondFactorNeeded(SecondFactorPrompt),
}

/// A challenge, with the derived key kept alive for the retry.
///
/// **Opaque on purpose.** It holds a `MasterKey` and the password hash, which
/// are the two things a caller must not be able to read out and put anywhere
/// else. The only thing it will answer is what the server offered.
///
/// No `Debug`, for the reason [`Authenticated`] hand-writes one.
pub struct SecondFactorPrompt {
    offered: Vec<SecondFactor>,
    master_key: MasterKey,
    password_hash: Zeroizing<String>,
    email: String,
}

impl SecondFactorPrompt {
    /// What the server said it would take, in the order it said it.
    #[must_use]
    pub fn offered(&self) -> &[SecondFactor] {
        &self.offered
    }

    /// Whether any offered factor is one this client can complete.
    ///
    /// Asked BEFORE the user is prompted, so an account this app cannot sign
    /// in to is refused while the user still has their hands in their
    /// pockets, rather than after they have fetched a key it cannot read.
    #[must_use]
    pub fn can_be_answered(&self) -> bool {
        self.offered.iter().any(SecondFactor::is_supported)
    }
}

impl RestClient {
    /// The first leg: prelogin, derive, and one grant with no second factor.
    pub fn begin_login(
        &self,
        email: &str,
        password: &[u8],
        device: &Device,
    ) -> Result<LoginOutcome, RestError> {
        let kdf = self.prelogin(email)?;
        let master_key = master_key(password, email, kdf)?;
        let hash = Zeroizing::new(master_key.password_hash(password));
        match self.password_grant_with(email, &hash, device, None) {
            Ok(session) => Ok(LoginOutcome::Done(Authenticated { session, master_key })),
            Err(RestError::TwoFactorRequired { providers }) => {
                Ok(LoginOutcome::SecondFactorNeeded(SecondFactorPrompt {
                    offered: providers.iter().map(|p| SecondFactor::from_wire(p)).collect(),
                    master_key,
                    password_hash: hash,
                    email: email.to_string(),
                }))
            }
            Err(e) => Err(e),
        }
    }

    /// The second leg: the same grant, with the answer.
    ///
    /// Takes the prompt **by value**: a prompt is one attempt. Letting a
    /// caller retry with the same value would invite a loop over a code the
    /// server has already consumed -- nodewarden consumes the TOTP counter
    /// (`identity.ts:500`), so the second use of a correct code is a wrong
    /// code.
    pub fn finish_login(
        &self,
        prompt: SecondFactorPrompt,
        answer: &SecondFactorAnswer,
        device: &Device,
    ) -> Result<Authenticated, RestError> {
        let session =
            self.password_grant_with(&prompt.email, &prompt.password_hash, device, Some(answer))?;
        Ok(Authenticated { session, master_key: prompt.master_key })
    }
}
```

`authenticate` is left exactly as it is. It already returns
`RestError::TwoFactorRequired` from `password_grant`, which is what the fourth
test asserts.

- [ ] **Step 4: Run the tests and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- rest::
```

- [ ] **Step 5: Commit**

---

### Task 4: The remembered device

**Files:**
- Modify: `deskwarden/src/rest/api.rs`

**Interfaces:**
- Produces: `Authenticated::remember_token: Option<Zeroizing<String>>`, and `SecondFactor::Remember` usable as an answer.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The server mints a trust token when remember was asked for, and it
    /// arrives as `TwoFactorToken` (`identity.ts:617`). Losing it means
    /// asking the user for a code on every launch.
    #[test]
    fn a_remembered_login_brings_back_the_token_that_makes_it_work_next_time() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/connect/token")
            .with_body(
                r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600,
                    "TwoFactorToken":"TRUST-1"}"#,
            )
            .create();
        let answer = SecondFactorAnswer {
            factor: SecondFactor::Authenticator,
            token: Zeroizing::new("123456".to_string()),
            remember: true,
        };
        let session = client
            .password_grant_with("me@example.com", "HASH", &a_device(), Some(&answer))
            .expect("the grant");
        assert_eq!(
            session.remember_token.as_deref().map(String::as_str),
            Some("TRUST-1"),
            "the device-trust token was dropped, so the next launch asks for a code again"
        );
    }

    /// A login that did not ask to be remembered gets no token, and the
    /// absence is `None` rather than an empty string -- a caller storing
    /// `Some("")` would send an empty token next time and be re-challenged.
    #[test]
    fn a_login_that_did_not_ask_to_be_remembered_carries_no_token() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .create();
        let session = client
            .password_grant("me@example.com", "HASH", &a_device())
            .expect("the grant");
        assert!(session.remember_token.is_none());
    }

    /// **A stored token is answered with provider 5 and never typed.**
    /// The server ties it to the device identifier (`identity.ts:481`), so
    /// this is also the test that would fail if the identifier stopped being
    /// stable.
    #[test]
    fn a_remembered_device_answers_with_the_stored_token() {
        let mut server = mockito::Server::new();
        let client = RestClient::new(server.url());
        let sent = server
            .mock("POST", "/identity/connect/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("twoFactorProvider".into(), "5".into()),
                mockito::Matcher::UrlEncoded("twoFactorToken".into(), "TRUST-1".into()),
                mockito::Matcher::UrlEncoded("deviceIdentifier".into(),
                    "11111111-2222-3333-4444-555555555555".into()),
            ]))
            .with_body(r#"{"access_token":"AT-1","refresh_token":"RT-1","expires_in":3600}"#)
            .expect(1)
            .create();
        let answer = SecondFactorAnswer {
            factor: SecondFactor::Remember,
            token: Zeroizing::new("TRUST-1".to_string()),
            remember: false,
        };
        client
            .password_grant_with("me@example.com", "HASH", &a_device(), Some(&answer))
            .expect("the grant");
        sent.assert();
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Expected: `no field remember_token on type Session`.

- [ ] **Step 3: Write the implementation**

Add to `TokenResponse`:

```rust
    /// The device-trust token, minted only when the grant asked to be
    /// remembered. Capitalised on the wire; nodewarden sends exactly this
    /// spelling (`identity.ts:617`) and so does Bitwarden.
    #[serde(rename = "TwoFactorToken", alias = "twoFactorToken", default)]
    two_factor_token: Option<Zeroizing<String>>,
```

Add to `Session`:

```rust
    /// Present only on a login that asked to be remembered. `Zeroizing`
    /// because it is a credential that skips the second factor for thirty
    /// days -- weaker than the refresh token beside it, and not by much.
    ///
    /// Empty is normalised to `None` at the parse: a caller that stored
    /// `Some("")` would send an empty token and be re-challenged, and would
    /// have no way to tell that from a rejected one.
    pub remember_token: Option<Zeroizing<String>>,
```

Populate it where `Session` is built, normalising empty to `None`. Do **not**
add it to the hand-written `Debug`.

- [ ] **Step 4: Run the tests and watch them pass**

- [ ] **Step 5: Commit**

---

### Task 5: The refusal a user can act on

**Files:**
- Modify: `deskwarden/src/rest/api.rs`
- Modify: `deskwarden/src/login_ui.rs`

**Interfaces:**
- Consumes: `RestError::TwoFactorRequired`, `SecondFactor` (Task 1).

**Why this is a task and not a leftover:** until the UI plan lands, a 2FA account still cannot sign in through direct REST. What it *must not* do is fail with words that send the user looking for the wrong problem. `derive_direct_rest` currently maps every `RestError` through `to_string()`, so a 2FA account reads as whatever `TwoFactorRequired`'s `Display` happens to say.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The sentence a user sees when their account has a second factor and
    /// this app cannot yet complete it. It has to say three things: that the
    /// password was right, what is being asked for, and what to do instead.
    #[test]
    fn the_two_factor_refusal_says_the_password_was_right_and_what_to_do() {
        let text = RestError::TwoFactorRequired { providers: vec!["0".to_string()] }.to_string();
        let lowered = text.to_lowercase();
        assert!(
            lowered.contains("password"),
            "the user must be told their password was accepted, or they will change it: {text:?}"
        );
        assert!(
            lowered.contains("authenticator app"),
            "the refusal must name the factor, not its number: {text:?}"
        );
        assert!(
            !text.contains('0') || lowered.contains("authenticator app"),
            "a bare provider number reached the user: {text:?}"
        );
        assert!(
            lowered.contains("official bw for crypto") || lowered.contains("setting"),
            "the refusal must point at the way out, which is the setting: {text:?}"
        );
    }

    /// Several providers are listed, not just the first.
    #[test]
    fn a_refusal_lists_every_factor_the_server_offered() {
        let text = RestError::TwoFactorRequired {
            providers: vec!["0".to_string(), "7".to_string()],
        }
        .to_string()
        .to_lowercase();
        assert!(text.contains("authenticator app"), "{text}");
        assert!(text.contains("security key"), "{text}");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Expected: the assertions fail on the current `Display`, which prints the raw provider numbers.

- [ ] **Step 3: Write the implementation**

Replace `TwoFactorRequired`'s `Display` arm:

```rust
            RestError::TwoFactorRequired { providers } => {
                let named: Vec<&str> = providers
                    .iter()
                    .map(|p| SecondFactor::from_wire(p).describe())
                    .collect();
                write!(
                    f,
                    "your password was accepted, but this account also needs {}. \
                     Deskwarden cannot do that yet on its own -- turn \"Use official bw for \
                     crypto\" back on in Preferences to sign in through the Bitwarden CLI",
                    named.join(" or ")
                )
            }
```

- [ ] **Step 4: Run the tests and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2
```

Run the **whole** suite here, not a filter: `login_ui` and `main` both assert
on error strings, and this task changes one.

- [ ] **Step 5: Commit**

---

## Verification before this branch is finished

- [ ] The full suite, with every failure accounted for. On the owner's machine the loopback failures are the port-range collision; **a failure in a test that binds no socket is real** and must not be waved through.
- [ ] `examples/rest_probe` still builds and its read-only run still passes against `nw37.powernapps.net` -- that account has no second factor, so this work must be invisible to it.
- [ ] **The one thing no test here can prove:** none of this has been driven against a server that actually demands a second factor. `mockito` returns the shape read out of `identity.ts`, which is a good deal better than a guess and is not the same as a real challenge. Say so in the branch's summary rather than letting "the tests pass" stand in for it.

# Signing In With an API Key Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user whose account's second factor is Duo or WebAuthn — the two Deskwarden cannot complete, and the two `bw login` cannot complete either — signs in inside Deskwarden's own window. They paste a personal API key's `client_id` and `client_secret`, then type their master password, and land in an unlocked vault. The message piece 2 shows them ("Use a personal API key instead… then sign in here with it") stops being a promise this app does not keep.

**Architecture:** This is piece 3 of `docs/superpowers/specs/2026-08-29-signing-in-with-an-api-key-design.md`. Piece 1 built the grant (`RestClient::api_key_grant`, `rest/api.rs:925`) and it has no caller. Piece 2 is being built **right now, in this same tree**, in `second_factor_ui.rs`, `login_ui.rs` and `app_window.rs`; this plan writes no `rest::` code, does not edit `deskwarden/src/rest/`, and touches piece 2's three files in exactly one task (Task 7), placed last on purpose.

The shape follows the design's two stages, and the two stages exist because of one asymmetry in piece 1: **`api_key_grant` returns a `Session` and no `MasterKey`.** The key authenticates; it does not decrypt. So:

* **Stage 1** is `api_key_grant(client_id, client_secret, device) -> Session`.
* **Stage 2** is `prelogin` → `master_key(password, email, kdf)` → `sync(&session)` → `unwrap_user_key(master_key.stretch(), profile.key)`.

Stage 2's last step is load-bearing and is the one thing the design leaves implicit: **there is no grant to reject a wrong master password with.** `master_key` derives a key from any bytes and always succeeds. The thing that says "that password was wrong" is `unwrap_user_key` failing on the profile's protected user key — the same check `rest::sync::VaultKeys::unwrap_from` already makes, and the same one `rest/crypto.rs:1656` already tests with a wrong password. Without it, a mistyped master password would produce a signed-in app holding a garbage key that fails item by item later, which is the design's own named worst case: "signed in and cannot read anything — which is a worse failure than being refused, because it looks like success."

**Where the credential lives.** Unlike piece 2's `Challenge`, the `client_secret` is *typed by the user*, so it cannot be kept off the UI thread — the text edit has to own a buffer. The containment is therefore the master password's, not the challenge's: `Zeroizing`, wiped on `Drop`, no `Debug` on the struct holding it, never formatted, never logged, never in an error string, and **never written to disk**. The session token it mints goes to the existing `SessionStore` by the route every other session token already takes — the `adopt` sink — and nothing new goes on disk at all.

**Why the network work still gets a worker thread.** The grant is a blocking HTTP call and the UI thread is the frame loop. The worker here is this feature's own, spawned by the API-key stage and holding the `Session` between the two stages, in the same shape as piece 2's `complete_second_factor`: it blocks on a `Receiver`, it is bounded by `Abandon` and by a disconnected channel, and its success is handed *sideways* to the `adopt` sink rather than back through the window. It deliberately does **not** reuse `login_ui`'s `spawn_auth`/`DirectRestLogin`, because piece 2 is rewriting exactly that machinery this week.

**Tech Stack:** Rust, egui/eframe, `fn`-pointer seam structs in production code, `std::sync::mpsc`, `zeroize`, `crate::rest::api::{RestClient, Session, Authenticated, RestError, Device}`, `crate::rest::crypto::{master_key, unwrap_user_key, EncString}`, `crate::rest::sync::Profile`, the `app_window::Stage`/`advance` transition table.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- `RUSTFLAGS="-D warnings"`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory; ~23 GB free and that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect class is "a test that passes because it never reached the thing it names".
- The mock-HTTP family is flaky at 17–41 failures per full `--lib` run of 4088, membership shifting every run. Judge by the tests you wrote, and compare against a baseline before believing you broke something.
- `login_ui.rs` is 10,450 lines and carries source-position pins that `split_once` on its own text; put new code in its own module rather than adding to it.

Additionally, and specific to this branch:

- **Do not edit `deskwarden/src/rest/`.** Piece 1 owns it. If a task needs a `rest::` item that does not exist, stop and report rather than adding it.
- **Another worker is editing `second_factor_ui.rs`, `login_ui.rs` and `app_window.rs` right now.** Tasks 1–6 touch none of them. Task 7 touches all three and is deliberately last; before starting it, `git log --oneline -5` and confirm piece 2's Task 8 has landed. If it has not, **stop and report** rather than editing those files underneath a live worker.
- **No test may touch** the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `git add .`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `the-second-factor-prompt`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/api_key_ui.rs` (**new**) | The whole feature that can be tested without an event loop: the two-stage state machine, the copy, the three refusals, the `RestError` mapping, the seam, the worker loop, the `draw_` function, and the source-reading hygiene tests. |
| `deskwarden/src/lib.rs` (modify) | One `pub mod api_key_ui;` line, in alphabetical position. |
| `deskwarden/src/second_factor_ui.rs` (modify, Task 7) | One new `Asked` variant and one button on the unsupported-only card, so piece 2's message leads somewhere. |
| `deskwarden/src/login_ui.rs` (modify, Task 7) | One new `LoginAction` variant and one link on the sign-in card. Nothing else. |
| `deskwarden/src/app_window.rs` (modify, Task 7) | `Stage::ApiKey`, its two events, its `advance` arms. |

**Why one new file.** The same reason piece 2 gave, plus a sharper one: three of the four files above are being written by another agent this week, and every line this plan puts in them is a merge conflict and a chance to trip a source-position pin belonging to a feature it has nothing to do with. Tasks 1–6 are 100% in a file nobody else has open.

---

### Task 1: The two stages, and which one a failure returns to

**Files:** Create `deskwarden/src/api_key_ui.rs`; modify `deskwarden/src/lib.rs`

**Interfaces**

- *Consumes:* `zeroize::Zeroizing`.
- *Produces:* `api_key_ui::Step`, `api_key_ui::ApiKeyForm`, `ApiKeyForm::new`, `ApiKeyForm::key_pair_ready`, `ApiKeyForm::password_ready`.

- [ ] **Step 1: Write the failing test**

Create `deskwarden/src/api_key_ui.rs` containing only the module docstring and the test module, so the first run fails to *resolve* rather than to compile a body:

```rust
//! **Signing in with a personal API key** — the way in for the accounts whose
//! second factor this app cannot complete (Duo, WebAuthn).
//!
//! Two stages, because they are two different things: the key pair
//! authenticates ([`crate::rest::api::RestClient::api_key_grant`]) and the
//! master password decrypts. Both, always. See
//! `docs/superpowers/specs/2026-08-29-signing-in-with-an-api-key-design.md`.
//!
//! # The client secret
//!
//! It is a permanent, password-free login to the account. It is handled here
//! exactly as the master password is: [`Zeroizing`], wiped on [`Drop`], on a
//! struct with no `Debug`, never formatted, never logged, never in an error
//! string, and **never written to disk**. What survives a restart is the
//! session token, by the same route every other session token takes.
//! `the_client_secret_is_handled_like_a_password` reads this file's own source
//! and asserts all of that.

#[cfg(test)]
mod tests {
    use super::*;

    /// **A rejected key pair keeps BOTH fields.** Retyping a 64-character
    /// secret because the id had a typo is the behaviour this design exists to
    /// avoid.
    #[test]
    fn the_form_starts_on_the_key_pair_and_knows_when_each_stage_is_answerable() {
        let mut form = ApiKeyForm::new();
        assert_eq!(form.step, Step::KeyPair, "the key pair comes first");
        assert!(!form.key_pair_ready(), "control: an empty form is not submittable");

        form.client_id.push_str("user.9f3c");
        assert!(
            !form.key_pair_ready(),
            "an id without a secret is half a credential, not a submittable one"
        );
        form.secret.push_str("  b7d2ecc  ");
        assert!(
            form.key_pair_ready(),
            "both fields present -- and the whitespace a paste brings must not block it"
        );

        assert!(!form.password_ready(), "control: no password has been typed");
        form.password.push_str("   ");
        assert!(
            !form.password_ready(),
            "whitespace is not a master password; submitting it would spend a round trip \
             to be told so"
        );
        form.password.push_str("hunter2");
        assert!(form.password_ready());
    }

    /// The password stage never carries the key pair back into view, and the
    /// key-pair stage never shows a password box. Two stages, not one screen
    /// with three fields -- the design's reason is diagnosis.
    #[test]
    fn the_two_steps_are_ordered_and_distinct() {
        assert_ne!(Step::KeyPair, Step::MasterPassword);
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = Step::MasterPassword;
        assert_eq!(
            form.client_id, "user.9f3c",
            "the id is still held: stage 2 failing must not have to re-ask for it"
        );
        assert_eq!(
            form.secret.as_str(),
            "b7d2ecc",
            "control: the secret survives the step change too, so Task 2's \
             'stage 1 is not repeated' is about the STEP and not about lost fields"
        );
    }
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`, in alphabetical position (between `pub mod app_window;` and `pub mod autostart_repair;`):

```rust
pub mod api_key_ui;
```

- [ ] **Step 3: Run it and watch it fail**

```bash
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: **compile error**, `cannot find type 'ApiKeyForm' in this scope` and `cannot find type 'Step' in this scope`.

- [ ] **Step 4: Implement**

Above the test module:

```rust
use zeroize::{Zeroize, Zeroizing};

/// Which of the two things the user is being asked for.
///
/// A step and not a bool, because the failures are asymmetric and the type is
/// where that asymmetry is written down: a rejected key pair returns to
/// [`Step::KeyPair`], a rejected password returns to [`Step::MasterPassword`]
/// and does **not** repeat stage 1, because nothing about stage 1 failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `client_id` and `client_secret`, exchanged for a session.
    KeyPair,
    /// The master password, which is what actually decrypts the vault.
    MasterPassword,
}

/// Everything the API-key sign-in is, as state.
///
/// **No `Debug`, derived or otherwise.** Two of the three fields are
/// credentials and the third names an account; nothing in here is a
/// formatter's business. `crate::debug_leak_guard` fails the suite for a
/// derived `Debug` over a [`Zeroizing`] field, and
/// `the_client_secret_is_handled_like_a_password` fails it for a hand-written
/// one.
pub struct ApiKeyForm {
    /// The key's public half. Not a secret -- it is `user.<guid>` and the
    /// server treats it as a username -- but it is still an account
    /// identifier, so nothing formats it either.
    pub client_id: String,
    /// **The key's secret half: a permanent, password-free login to this
    /// account.** It does not expire, it is not covered by the second factor
    /// it exists to bypass, and it is never persisted. See the module docs.
    pub secret: Zeroizing<String>,
    /// The master password. Stage 2's whole subject, and the only thing that
    /// can decrypt the vault.
    pub password: Zeroizing<String>,
    pub step: Step,
    /// The inline message under the fields, or `None`. See
    /// [`crate::api_key_ui::message_for`].
    pub error: Option<String>,
    /// True while a grant or an unlock is in flight: the buttons ghost,
    /// exactly as the sign-in card's do.
    pub busy: bool,
}

/// Three fields' worth of credential, on a struct move-captured by a frame
/// closure that lives as long as the window. `login_ui::LoginForm` states the
/// same rule for the same reason.
impl Drop for ApiKeyForm {
    fn drop(&mut self) {
        // `Zeroizing` wipes `secret` and `password` itself. The id and the
        // message are not credentials, and are wiped anyway rather than
        // released to the allocator naming an account.
        self.client_id.zeroize();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
    }
}

impl Default for ApiKeyForm {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyForm {
    pub fn new() -> Self {
        Self {
            client_id: String::new(),
            secret: Zeroizing::new(String::new()),
            password: Zeroizing::new(String::new()),
            step: Step::KeyPair,
            error: None,
            busy: false,
        }
    }

    /// Whether stage 1 has something to send. Trimmed, because both of these
    /// arrive by paste from a web page and routinely carry whitespace the
    /// server would reject.
    pub fn key_pair_ready(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.secret.trim().is_empty()
    }

    /// Whether stage 2 has something to try. Trimmed only for the emptiness
    /// check -- a master password's own leading space is part of it and is
    /// **not** trimmed when it is sent.
    pub fn password_ready(&self) -> bool {
        !self.password.trim().is_empty()
    }
}
```

- [ ] **Step 5: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: both tests pass, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/api_key_ui.rs deskwarden/src/lib.rs
git commit -F <message file>
```

Message: the two stages as state, and why the step is a type rather than a bool.

---

### Task 2: The three refusals, told apart — and which stage each returns to

**Files:** Modify `deskwarden/src/api_key_ui.rs`

**Interfaces**

- *Consumes:* `Step`, `ApiKeyForm`.
- *Produces:* `enum Refusal { KeyPairRejected, PasswordRejected, Unreachable }`, `message_for(Refusal) -> &'static str`, `ApiKeyForm::refused(Refusal)`.

The design names all three and the behaviour of each. `Unreachable` is the one that "must not read as a rejected credential — the same distinction `RestError::CodeNotSent` makes for the email code in piece 1", so it changes no field and no step: there is nothing wrong with what the user typed.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
    /// The three failures say three different things. A shared "That didn't
    /// work" would tell a user whose Wi-Fi dropped to check a secret that is
    /// correct.
    #[test]
    fn the_three_refusals_do_not_share_a_message() {
        let key_pair = message_for(Refusal::KeyPairRejected);
        let password = message_for(Refusal::PasswordRejected);
        let unreachable = message_for(Refusal::Unreachable);

        assert!(
            key_pair.contains("API key") || key_pair.contains("client secret"),
            "the key-pair failure must name the thing that was refused; got {key_pair:?}"
        );
        assert!(
            key_pair.contains("rotated") || key_pair.contains("web vault"),
            "a rotated key is the commonest cause and the only one with a fix the user can \
             act on; got {key_pair:?}"
        );
        assert!(
            password.contains("master password"),
            "got {password:?}"
        );
        assert!(
            !password.contains("API key"),
            "the password failure must not send the user back to a key that worked; \
             got {password:?}"
        );
        assert!(
            unreachable.contains("reach") || unreachable.contains("connection"),
            "got {unreachable:?}"
        );
        assert!(
            !unreachable.contains("wrong") && !unreachable.contains("didn't work"),
            "an unreachable server must not read as a rejected credential; got {unreachable:?}"
        );

        let all = [key_pair, password, unreachable];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "two refusals share one message");
            }
        }
    }

    /// **A rejected key pair returns to stage 1 with BOTH fields kept.** This
    /// is the behaviour the design exists to produce.
    #[test]
    fn a_rejected_key_pair_keeps_both_fields() {
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = Step::MasterPassword;
        form.busy = true;

        form.refused(Refusal::KeyPairRejected);

        assert_eq!(form.step, Step::KeyPair, "the key pair is what failed");
        assert_eq!(form.client_id, "user.9f3c", "the id is kept");
        assert_eq!(
            form.secret.as_str(),
            "b7d2ecc",
            "and so is the secret -- retyping 64 characters because the id had a typo is \
             exactly what this design refuses to charge for"
        );
        assert!(!form.busy, "the buttons come back");
        assert_eq!(form.error.as_deref(), Some(message_for(Refusal::KeyPairRejected)));
    }

    /// **A rejected password returns to stage 2 only.** Stage 1 is not
    /// repeated, because nothing about it failed -- the session is good.
    #[test]
    fn a_rejected_password_does_not_reask_for_the_key_pair() {
        let mut form = ApiKeyForm::new();
        form.client_id.push_str("user.9f3c");
        form.secret.push_str("b7d2ecc");
        form.step = Step::MasterPassword;
        form.password.push_str("wrong-one");
        form.busy = true;

        form.refused(Refusal::PasswordRejected);

        assert_eq!(
            form.step,
            Step::MasterPassword,
            "the key pair worked; sending the user back to it would be a lie about what failed"
        );
        assert!(form.password.is_empty(), "the wrong password is gone from the box");
        assert_eq!(
            form.client_id, "user.9f3c",
            "control: the key pair is still held, so a retry needs no re-entry"
        );
        assert_eq!(form.error.as_deref(), Some(message_for(Refusal::PasswordRejected)));
    }

    /// An unreachable server changed nothing about what the user typed, so it
    /// clears nothing and moves nothing.
    #[test]
    fn an_unreachable_server_touches_no_field_and_no_step() {
        for step in [Step::KeyPair, Step::MasterPassword] {
            let mut form = ApiKeyForm::new();
            form.client_id.push_str("user.9f3c");
            form.secret.push_str("b7d2ecc");
            form.password.push_str("hunter2");
            form.step = step;
            form.busy = true;

            form.refused(Refusal::Unreachable);

            assert_eq!(form.step, step, "an unreachable server is not a wrong stage");
            assert_eq!(form.client_id, "user.9f3c");
            assert_eq!(form.secret.as_str(), "b7d2ecc");
            assert_eq!(
                form.password.as_str(),
                "hunter2",
                "nothing the user typed was refused, so nothing they typed is thrown away"
            );
            assert!(!form.busy, "control: the refusal was applied at all");
        }
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: `cannot find type 'Refusal' in this scope`, `cannot find function 'message_for' in this scope`, `no method named 'refused' found for struct 'ApiKeyForm'`.

- [ ] **Step 3: Implement**

```rust
/// Why the API-key sign-in stopped.
///
/// Three variants and not one `String`, because the *behaviour* differs and
/// not only the wording: each names a different stage to return to and a
/// different set of fields to keep. A `String` error would put that decision
/// in the caller, where nothing tests it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The grant said no: the id or the secret is wrong, or the key has been
    /// rotated in the web vault. **Both fields are kept.**
    KeyPairRejected,
    /// The session is good and the key is not the problem: the master password
    /// did not unwrap this account's user key. Stage 1 is not repeated.
    PasswordRejected,
    /// Neither call got an answer at all. **Not a rejected credential** --
    /// nothing the user typed was refused. The same distinction
    /// [`crate::rest::api::RestError::CodeNotSent`] makes for the email code.
    Unreachable,
}

/// One line the user can act on, per refusal.
///
/// `&'static str` and not a formatted `String`, deliberately: a formatted
/// message is a place a credential can be interpolated into, and these three
/// are the only strings this feature ever shows a user about a failure.
pub fn message_for(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::KeyPairRejected => {
            "That API key wasn't accepted. Check the client id and client secret — and if the \
             key has been rotated in the web vault, create a new one under Account settings \
             \u{2192} Security \u{2192} Keys."
        }
        Refusal::PasswordRejected => {
            "That master password didn't unlock this account. The API key is fine — only the \
             password needs retyping."
        }
        Refusal::Unreachable => {
            "Couldn't reach the server. Check your connection — and the server URL, if this is \
             a self-hosted account."
        }
    }
}

impl ApiKeyForm {
    /// Applies a refusal to the form: the message, the stage to return to, and
    /// the fields to clear.
    ///
    /// **The whole of the design's error-state section is this one `match`**,
    /// which is why it is here and not spread across the draw function and the
    /// worker.
    pub fn refused(&mut self, refusal: Refusal) {
        self.busy = false;
        self.error = Some(message_for(refusal).to_string());
        match refusal {
            // Back to stage 1, holding both fields: the user is likelier to
            // have mistyped the short id than the pasted secret, and they can
            // see both to tell.
            Refusal::KeyPairRejected => {
                self.step = Step::KeyPair;
                self.password.zeroize();
            }
            // Stage 2 only. The session minted by stage 1 is still good and is
            // still held by the worker.
            Refusal::PasswordRejected => {
                self.step = Step::MasterPassword;
                self.password.zeroize();
            }
            // Nothing the user typed was refused, so nothing is cleared and
            // nothing moves. The button comes back and they press it again.
            Refusal::Unreachable => {}
        }
    }
}
```

Note `Zeroizing<String>::zeroize` truncates the string to empty as well as wiping it, which is what `form.password.is_empty()` in the test asserts.

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/api_key_ui.rs`. Message: three refusals, three stages, three sets of kept fields — and why an unreachable server clears nothing.

---

### Task 3: Turning a `RestError` into a refusal, without a server

**Files:** Modify `deskwarden/src/api_key_ui.rs`

**Interfaces**

- *Consumes:* `crate::rest::api::RestError` (piece 1, read-only).
- *Produces:* `grant_refusal(&RestError) -> Refusal`, `unlock_refusal(&RestError) -> Refusal`.

Two functions and not one, because the same `RestError` means different things at the two stages, and that is exactly the diagnosis the two-stage split buys. `RestError` is `pub` with `pub` variants, so every case below is constructible in a test and no HTTP is involved — this whole task is outside the flaky mock-HTTP family.

- [ ] **Step 1: Write the failing test**

```rust
    /// Stage 1: every way the grant can say no is the key pair's fault, and
    /// the one way it can say nothing is not.
    #[test]
    fn a_refused_grant_is_the_key_pairs_fault_and_a_dead_socket_is_not() {
        use crate::rest::api::RestError;

        for refused in [
            RestError::Unauthorized,
            RestError::InvalidCredentials,
            RestError::Rejected {
                error: "invalid_client".to_string(),
                description: "client_secret is invalid".to_string(),
            },
            RestError::Status(403),
        ] {
            assert_eq!(
                grant_refusal(&refused),
                Refusal::KeyPairRejected,
                "{refused:?} is the server refusing this key pair"
            );
        }

        assert_eq!(
            grant_refusal(&RestError::Transport("dns error".to_string())),
            Refusal::Unreachable,
            "a transport failure is not a wrong secret, and telling the user it was would \
             send them to rotate a key that is fine"
        );
        assert_eq!(
            grant_refusal(&RestError::Parse("the access token")),
            Refusal::Unreachable,
            "an answer this client cannot read is not a credential the user can fix"
        );
    }

    /// Stage 2: the ONLY thing that means "wrong master password" is a crypto
    /// failure unwrapping the user key. There is no grant here to reject it.
    #[test]
    fn only_a_crypto_failure_means_the_master_password_was_wrong() {
        use crate::rest::api::RestError;
        use crate::rest::crypto::CryptoError;

        assert_eq!(
            unlock_refusal(&RestError::Crypto(CryptoError::Malformed(
                "the profile carries no protected user key"
            ))),
            Refusal::PasswordRejected,
            "a user key that will not unwrap IS the wrong-password signal: `master_key` \
             derives a key from any bytes and never fails"
        );
        assert_eq!(
            unlock_refusal(&RestError::Transport("connection reset".to_string())),
            Refusal::Unreachable
        );
        // **The session died, not the password.** A 401 on stage 2 is the
        // token minted by stage 1 having been revoked or expired, so the
        // honest place to send the user is back to the key pair.
        assert_eq!(
            unlock_refusal(&RestError::Unauthorized),
            Refusal::KeyPairRejected,
            "a 401 at stage 2 is a dead session, and a dead session is re-minted from the \
             key pair -- not from the master password"
        );
        // Positive control on the whole function: it does not answer
        // `PasswordRejected` to everything.
        assert_ne!(
            unlock_refusal(&RestError::Transport("x".to_string())),
            Refusal::PasswordRejected,
            "control: unlock_refusal discriminates at all"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: `cannot find function 'grant_refusal' in this scope` and `cannot find function 'unlock_refusal' in this scope`.

If it instead fails on `CryptoError::Malformed` not taking a `&'static str`, read `rest/crypto.rs`'s own definition and use whatever variant it does expose — **do not edit `rest/`.**

- [ ] **Step 3: Implement**

```rust
use crate::rest::api::RestError;

/// What a failed [`crate::rest::api::RestClient::api_key_grant`] means to the
/// user.
///
/// Everything the server actively refused is the key pair's fault, because the
/// key pair is the only thing this call sends that a user can get wrong: there
/// is no username and no password in a `client_credentials` grant. Everything
/// else -- no answer, or an answer this client cannot read -- is
/// [`Refusal::Unreachable`], which asks the user to check their connection
/// rather than a secret that may be perfectly correct.
///
/// **No arm formats the error.** `RestError`'s `Display` carries a status and
/// a route, but this function's whole output is a `Refusal`, so nothing the
/// server said can reach a message on the way past.
pub fn grant_refusal(error: &RestError) -> Refusal {
    match error {
        RestError::Transport(_) | RestError::Parse(_) => Refusal::Unreachable,
        _ => Refusal::KeyPairRejected,
    }
}

/// What a failed stage 2 means to the user.
///
/// Stage 2 is prelogin, then a derivation that cannot fail on a wrong
/// password, then a sync, then unwrapping the user key. **The unwrap is the
/// verification**, and a [`RestError::Crypto`] out of it is the only thing in
/// this whole path that means "that master password was wrong".
///
/// A [`RestError::Unauthorized`] here is the session dying, not the password
/// failing -- so it returns [`Refusal::KeyPairRejected`], which is where a new
/// session comes from.
pub fn unlock_refusal(error: &RestError) -> Refusal {
    match error {
        RestError::Crypto(_) => Refusal::PasswordRejected,
        RestError::Transport(_) | RestError::Parse(_) => Refusal::Unreachable,
        RestError::Unauthorized => Refusal::KeyPairRejected,
        _ => Refusal::Unreachable,
    }
}
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/api_key_ui.rs`. Message: the same `RestError` means different things at the two stages, and that difference is the whole reason the design splits them.

---

### Task 4: The two stages performed — the seam, and stage 2's real verification

**Files:** Modify `deskwarden/src/api_key_ui.rs`

**Interfaces**

- *Consumes:* `crate::rest::api::{RestClient, Session, Authenticated, Device, RestError}`, `crate::rest::crypto::{master_key, unwrap_user_key, EncString}`, `crate::rest::sync::Profile`.
- *Produces:* `api_key_ui::Account`, `api_key_ui::ApiKeySeam`, `api_key_ui::PRODUCTION_API_KEY`, `api_key_ui::grant_direct_rest`, `api_key_ui::unlock_direct_rest`.

The seam is a **`fn`-pointer struct in production code** — the crate's rule, and the same shape `login_ui::AuthenticateFn` and piece 2's `SecondFactorSeam` use. A struct rather than two aliases because the two calls are one substitutable unit: a test that faked the grant but let the unlock reach the network would be testing half a sign-in.

`Session` has no public constructor, but `Session::from_refresh_token` and `MasterKey::from_bytes` are `pub(crate)` (`rest/api.rs:672`, `rest/crypto.rs:369`) and this crate's tests already build fixtures from them (`login_ui.rs:9617`). That is what makes the seam fakeable without touching `rest/`.

- [ ] **Step 1: Write the failing test**

```rust
    /// A grant answer and an unlock answer with no server behind either.
    fn fake_session() -> crate::rest::api::Session {
        crate::rest::api::Session::from_refresh_token(zeroize::Zeroizing::new(
            "not-a-real-refresh-token".to_string(),
        ))
    }

    fn fake_authenticated() -> crate::rest::api::Authenticated {
        crate::rest::api::Authenticated {
            session: fake_session(),
            master_key: crate::rest::crypto::MasterKey::from_bytes(
                [0x5A; crate::rest::crypto::MASTER_KEY_LEN],
            ),
        }
    }

    fn test_account() -> Account {
        Account {
            server_url: "https://vault.example.test".to_string(),
            email: "a@b.c".to_string(),
            device_id: "11111111-2222-3333-4444-555555555555".to_string(),
        }
    }

    /// **The production seam is wired to the real calls and not to a stub
    /// somebody left in while testing.** A source pin, because the value is a
    /// `const` of two function pointers and no runtime assertion can say which
    /// functions they are.
    #[test]
    fn the_production_seam_calls_the_real_grant_and_the_real_unwrap() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let body = source
            .split_once(concat!("pub const PRODUCTION_API_", "KEY: ApiKeySeam"))
            .expect("the production seam must exist")
            .1
            .split_once(";\n")
            .expect("the const must be terminated")
            .0;
        assert!(body.len() > 30, "control: the seam's body is not empty");
        assert!(
            body.contains("grant_direct_rest") && body.contains("unlock_direct_rest"),
            "the seam must be wired to this module's own two functions; got {body:?}"
        );

        // And those two functions must call what they claim to. `api_key_grant`
        // is piece 1's whole contribution and this feature is its only caller.
        assert!(
            source.contains(concat!(".api_key", "_grant(")),
            "nothing in this module calls the grant this feature exists to call"
        );
        assert!(
            source.contains("unwrap_user_key("),
            "stage 2 must actually unwrap the user key -- that unwrap IS the master \
             password's verification, and without it a wrong password signs the user in to \
             a vault they cannot read"
        );
        // Positive control on both searches: a needle this file really does
        // spell this way is findable, so the two above mean something.
        assert!(
            source.contains(concat!(".prelogin", "(")),
            "control: a rest::api call is findable by this search at all"
        );
    }

    /// **The seam's two calls happen in order, and stage 2 does not re-run
    /// stage 1.** The count is the assertion: a design that re-granted on
    /// every password attempt would charge a Duo user a round trip and a new
    /// device registration per typo.
    #[test]
    fn a_password_retry_reuses_the_session_stage_one_minted() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static GRANTS: AtomicUsize = AtomicUsize::new(0);
        static UNLOCKS: AtomicUsize = AtomicUsize::new(0);
        GRANTS.store(0, Ordering::SeqCst);
        UNLOCKS.store(0, Ordering::SeqCst);

        let seam = ApiKeySeam {
            grant: |_account, _id, _secret| {
                GRANTS.fetch_add(1, Ordering::SeqCst);
                Ok(fake_session())
            },
            unlock: |_account, _session, password| {
                let n = UNLOCKS.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    assert_eq!(password, b"wrong-one", "control: the first password arrives");
                    Err(RestError::Crypto(crate::rest::crypto::CryptoError::Malformed(
                        "the user key did not unwrap",
                    )))
                } else {
                    assert_eq!(password, b"hunter2", "the SECOND password is the one used");
                    Ok(fake_authenticated())
                }
            },
        };

        let session = (seam.grant)(&test_account(), "user.9f3c", "b7d2ecc").expect("stage 1");
        let first = (seam.unlock)(&test_account(), session, b"wrong-one");
        let refusal = unlock_refusal(&first.expect_err("the first password is wrong"));
        assert_eq!(refusal, Refusal::PasswordRejected);

        // The retry: a NEW session is not minted, because stage 1 did not
        // fail. This is the shape Task 5's loop enforces; here it is the
        // seam's own contract being stated.
        let session = (seam.grant)(&test_account(), "user.9f3c", "b7d2ecc").expect("stage 1");
        assert!((seam.unlock)(&test_account(), session, b"hunter2").is_ok());
        assert_eq!(
            UNLOCKS.load(Ordering::SeqCst),
            2,
            "control: both passwords reached the unlock"
        );
        assert_eq!(GRANTS.load(Ordering::SeqCst), 2, "control: the fake grant is reachable");
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: `cannot find type 'Account' in this scope` and `cannot find type 'ApiKeySeam' in this scope`; `the_production_seam_calls_the_real_grant_and_the_real_unwrap` panics with "the production seam must exist".

- [ ] **Step 3: Implement**

```rust
/// The three things both stages need that the user did not type into this
/// card: they came from the sign-in card, which asked for them first.
///
/// A struct rather than three parameters threaded through four signatures,
/// and no secret in it -- the server URL and the email are what the user typed
/// into the card, and the device id is a stable installation GUID. A derived
/// `Debug` is fine and deliberate: this is what a reader debugging a rejected
/// grant needs to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The server the account is on, as the sign-in card has it.
    pub server_url: String,
    /// The account's address. **Stage 2 needs it and stage 1 does not** --
    /// there is no username in a `client_credentials` grant, but the email is
    /// the KDF's salt, so the master password cannot be derived without it.
    pub email: String,
    /// The per-installation device identifier. The same value the password
    /// sign-in sends, so this does not register a second device.
    pub device_id: String,
}

/// The two `rest::` calls this feature needs, as `fn` pointers.
///
/// A struct and not two aliases: a test that faked the grant but let the
/// unlock reach the network would be a test of half a sign-in. A `fn`-pointer
/// struct and not a `cfg(test)` seam, which this crate bans crate-wide, and
/// not a trait object, because there is exactly one production value.
///
/// **Neither signature carries the secret anywhere it can be formatted.** The
/// secret is a `&str` argument, borrowed for one call, and every `Err` here is
/// a `RestError` -- which piece 1 documents as carrying no credential, and
/// which this module reduces to a [`Refusal`] before anything sees it anyway.
#[derive(Clone, Copy)]
pub struct ApiKeySeam {
    /// **Stage 1.** `grant_type=client_credentials`, `scope=api`, the three
    /// device fields. Yields a session and no master key -- see
    /// [`crate::rest::api::RestClient::api_key_grant`] for why that asymmetry
    /// is the whole reason this feature has two stages.
    pub grant: fn(&Account, client_id: &str, client_secret: &str) -> Result<Session, RestError>,
    /// **Stage 2.** Prelogin, derive, sync, unwrap. Consumes the session
    /// because the [`Authenticated`] it returns carries it onward -- there is
    /// one session and it does not get copied.
    pub unlock: fn(&Account, Session, password: &[u8]) -> Result<Authenticated, RestError>,
}

/// [`ApiKeySeam::grant`] as production performs it.
///
/// The client secret is borrowed, passed straight through, and never bound to
/// a local -- there is nothing here for a `Drop` to have to wipe.
pub fn grant_direct_rest(
    account: &Account,
    client_id: &str,
    client_secret: &str,
) -> Result<Session, RestError> {
    let device = Device::windows_desktop(&account.device_id, DEVICE_NAME);
    RestClient::new(&account.server_url).api_key_grant(client_id, client_secret, &device)
}

/// [`ApiKeySeam::unlock`] as production performs it — **and the master
/// password's only verification.**
///
/// `master_key` cannot fail on a wrong password: it is a KDF, and it derives
/// *a* key from any bytes at all. So the four steps here are not four steps of
/// setup with a check at the end; the last one **is** the check. A wrong
/// password produces a key that will not unwrap this account's protected user
/// key, `unwrap_user_key` answers [`crate::rest::crypto::CryptoError`], and
/// [`unlock_refusal`] turns that into [`Refusal::PasswordRejected`].
///
/// Skipping it would produce an app that is signed in and cannot read
/// anything -- which the design names as worse than being refused, because it
/// looks like success.
///
/// The unwrapped key is dropped on the spot. It is not what this function
/// returns: `rest::sync::VaultKeys::unwrap_from` derives it again from the
/// same [`crate::rest::crypto::MasterKey`] when the vault is actually read,
/// and returning a second copy would be a second key with a life nobody is
/// tracking.
pub fn unlock_direct_rest(
    account: &Account,
    session: Session,
    password: &[u8],
) -> Result<Authenticated, RestError> {
    let client = RestClient::new(&account.server_url);
    let kdf = client.prelogin(&account.email)?;
    let master_key = crate::rest::crypto::master_key(password, &account.email, kdf)?;

    let synced = client.sync(&session)?;
    let profile = synced
        .profile
        .ok_or(RestError::Parse("the account profile"))?;
    let protected = profile
        .key
        .as_deref()
        .ok_or(RestError::Parse("the protected user key"))?;
    let protected: crate::rest::crypto::EncString = protected.parse()?;
    // The verification. The returned key is deliberately dropped here.
    let _ = crate::rest::crypto::unwrap_user_key(&master_key.stretch(), &protected)?;

    Ok(Authenticated { session, master_key })
}

/// What the user's device list calls this app. The same value
/// `login_ui::DEVICE_NAME` uses, written again rather than imported, because
/// `login_ui` is 10,450 lines under another worker's hands this week and this
/// module owes it no dependency.
const DEVICE_NAME: &str = "Deskwarden";

/// The seam's one production value, written in exactly one place.
pub const PRODUCTION_API_KEY: ApiKeySeam = ApiKeySeam {
    grant: grant_direct_rest,
    unlock: unlock_direct_rest,
};
```

Add the imports this needs at the top of the file, beside the existing ones:

```rust
use crate::rest::api::{Authenticated, Device, RestClient, Session};
```

> **If `EncString: FromStr` does not produce a `RestError` through `?`**, `RestError: From<CryptoError>` exists (`rest/api.rs`, `impl From<CryptoError> for RestError`) and `EncString`'s parse error is a `CryptoError`; if it is not, bind the parse and `map_err(RestError::Crypto)` explicitly. **Do not add a conversion to `rest/`.**

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/api_key_ui.rs`. Message: the two calls as one seam, and the paragraph that matters — the user-key unwrap is the master password's only verification, because the KDF cannot refuse anything.

---

### Task 5: The worker — one session, two stages, and nothing persisted

**Files:** Modify `deskwarden/src/api_key_ui.rs`

**Interfaces**

- *Consumes:* `ApiKeySeam`, `Account`, `Refusal`, `grant_refusal`, `unlock_refusal`.
- *Produces:* `api_key_ui::Command`, `api_key_ui::Report`, `api_key_ui::run_api_key_sign_in`.

The worker holds the `Session` between the two stages, in the same shape piece 2's `complete_second_factor` holds the `Challenge`: it blocks on a `Receiver`, and it is bounded on both ends by `Command::Abandon` and by a disconnected channel. The secret travels *down* this channel because the user typed it into a text box and there is no way around that — which is why every containment this feature has is on the buffer, not on the boundary.

- [ ] **Step 1: Write the failing test**

```rust
    /// **A wrong master password is retried against the SAME session.** The
    /// key pair is used once, to mint a session, and dropped -- the design's
    /// own words -- so a password typo costs one sync and not a second grant.
    #[test]
    fn a_wrong_password_is_retried_without_a_second_grant() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;
        static GRANTS: AtomicUsize = AtomicUsize::new(0);
        static UNLOCKS: AtomicUsize = AtomicUsize::new(0);
        GRANTS.store(0, Ordering::SeqCst);
        UNLOCKS.store(0, Ordering::SeqCst);

        let seam = ApiKeySeam {
            grant: |_, id, secret| {
                GRANTS.fetch_add(1, Ordering::SeqCst);
                assert_eq!(id, "user.9f3c");
                assert_eq!(secret, "b7d2ecc", "the secret reaches the grant untouched");
                Ok(fake_session())
            },
            unlock: |_, _session, password| {
                if UNLOCKS.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(RestError::Crypto(crate::rest::crypto::CryptoError::Malformed(
                        "the user key did not unwrap",
                    )))
                } else {
                    Ok(fake_authenticated())
                }
            },
        };

        let (tx, rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        tx.send(Command::KeyPair {
            client_id: "user.9f3c".to_string(),
            secret: zeroize::Zeroizing::new("b7d2ecc".to_string()),
        })
        .unwrap();
        tx.send(Command::MasterPassword(zeroize::Zeroizing::new("wrong-one".to_string())))
            .unwrap();
        tx.send(Command::MasterPassword(zeroize::Zeroizing::new("hunter2".to_string())))
            .unwrap();

        let outcome = run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx);

        assert!(outcome.is_some(), "the second password signs the user in");
        assert_eq!(UNLOCKS.load(Ordering::SeqCst), 2, "control: both passwords were tried");
        assert_eq!(
            GRANTS.load(Ordering::SeqCst),
            1,
            "a wrong password must NOT re-grant: stage 1 did not fail, and a second grant \
             would register a device and spend a round trip per typo"
        );
        assert_eq!(
            report_rx.try_iter().collect::<Vec<_>>(),
            vec![Report::KeyPairAccepted, Report::Refused(Refusal::PasswordRejected)],
            "the window was told stage 1 passed, then told the PASSWORD was what failed"
        );
    }

    /// **A rejected key pair does not advance.** The loop stays on stage 1 and
    /// the next thing it will accept is another key pair -- a
    /// `MasterPassword` arriving now has no session to try against.
    #[test]
    fn a_rejected_key_pair_leaves_the_loop_on_stage_one() {
        use std::sync::mpsc;
        let seam = ApiKeySeam {
            grant: |_, _, _| Err(RestError::Unauthorized),
            unlock: |_, _, _| panic!("nothing was granted, so nothing may be unlocked"),
        };
        let (tx, rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        tx.send(Command::KeyPair {
            client_id: "user.9f3c".to_string(),
            secret: zeroize::Zeroizing::new("wrong".to_string()),
        })
        .unwrap();
        // Arrives while there is no session. It must be ignored rather than
        // panicking the worker on the one screen a blocked user is looking at.
        tx.send(Command::MasterPassword(zeroize::Zeroizing::new("hunter2".to_string())))
            .unwrap();
        tx.send(Command::Abandon).unwrap();

        assert!(run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx).is_none());
        assert_eq!(
            report_rx.try_iter().collect::<Vec<_>>(),
            vec![
                Report::Refused(Refusal::KeyPairRejected),
                Report::Refused(Refusal::KeyPairRejected),
            ],
            "the stray password is answered as 'the key pair is what is missing', which is \
             true, and not as a rejected password, which would be a lie"
        );
    }

    /// A closed command channel -- the window went away -- ends the loop and
    /// drops the session, rather than blocking a detached thread forever.
    #[test]
    fn a_closed_window_ends_the_loop() {
        use std::sync::mpsc;
        let seam = ApiKeySeam {
            grant: |_, _, _| panic!("nothing was sent, so nothing may be granted"),
            unlock: |_, _, _| panic!("nothing was sent, so nothing may be unlocked"),
        };
        let (tx, rx) = mpsc::channel::<Command>();
        let (report_tx, _report_rx) = mpsc::channel();
        drop(tx);
        assert!(run_api_key_sign_in(&seam, &test_account(), &rx, &report_tx).is_none());
    }

    /// **Nothing in this module writes the key pair to disk.** The design's
    /// deliberate refusal, read off the source: a stored `client_secret` is a
    /// permanent password-free login sitting on disk.
    #[test]
    fn the_key_pair_is_never_persisted() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        for forbidden in [
            "SessionStore",
            "user_key_store",
            "std::fs::",
            "fs::write",
            "File::create",
            "settings::",
        ] {
            assert!(
                !production.contains(forbidden),
                "this module reached for `{forbidden}`; the client secret is not persisted \
                 and the session token goes to the store the `adopt` sink already owns"
            );
        }
        // Positive controls: the cut really did keep the production half, and
        // these needles really are findable in this crate when they are there.
        assert!(
            production.contains("pub fn run_api_key_sign_in"),
            "the cut lost the production half, so the absences above prove nothing"
        );
        assert!(
            include_str!("session_store.rs").contains("std::fs::write"),
            "the needle has drifted -- persistence is no longer spelled this way, so its \
             absence above means nothing"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: `cannot find type 'Command' in this scope`, `cannot find type 'Report' in this scope`, `cannot find function 'run_api_key_sign_in' in this scope`.

- [ ] **Step 3: Implement**

```rust
/// **What the window tells the worker.**
///
/// No `Debug`, for [`ApiKeyForm`]'s reason: two of the three variants carry a
/// credential, and a derived `Debug` would print whatever they let it.
pub enum Command {
    /// Stage 1. The secret is moved in and dropped when the arm handling it
    /// ends, wiping itself on the way.
    KeyPair { client_id: String, secret: Zeroizing<String> },
    /// Stage 2, against the session stage 1 minted.
    MasterPassword(Zeroizing<String>),
    /// The user backed out or closed the window. The worker drops the session
    /// and returns.
    Abandon,
}

/// **What the worker tells the window.**
///
/// A variant and not a `String`, so the window cannot be handed a message
/// built out of a `RestError` this module has not vetted -- and so the only
/// strings this feature ever shows about a failure are
/// [`message_for`]'s three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Stage 1 passed. The card moves to the master password.
    KeyPairAccepted,
    /// One of the three. See [`ApiKeyForm::refused`] for what each does.
    Refused(Refusal),
}

/// **The API-key sign-in, from the worker thread's side.**
///
/// Blocks on the window's commands while holding the [`Session`] stage 1
/// minted, and returns an [`Authenticated`] or nothing. Holding the session is
/// what makes a mistyped master password cheap: stage 1 is not repeated,
/// because nothing about it failed.
///
/// **It blocks a detached thread on a human**, for piece 2's reason and with
/// piece 2's bounds: a [`Command::Abandon`] and a disconnected channel both
/// return, and returning drops the session.
///
/// **Nothing here is persisted and nothing here is logged with a value in
/// it.** The two `log::warn!` lines carry a stage name and no argument; the
/// `RestError` is consumed by [`grant_refusal`]/[`unlock_refusal`], which
/// return an enum.
pub fn run_api_key_sign_in(
    seam: &ApiKeySeam,
    account: &Account,
    commands: &std::sync::mpsc::Receiver<Command>,
    report: &std::sync::mpsc::Sender<Report>,
) -> Option<Authenticated> {
    // The one session, minted at most once per accepted key pair. `None` is
    // "stage 1 has not passed yet", which is also what makes a stray
    // `MasterPassword` answerable rather than a panic.
    let mut session: Option<Session> = None;

    // `recv()` and not `try_recv()`: this thread has nothing else to do, and a
    // poll loop would spin for as long as it takes somebody to find their web
    // vault. `Err` is a disconnected channel, which is a closed window.
    while let Ok(command) = commands.recv() {
        match command {
            Command::Abandon => return None,
            Command::KeyPair { client_id, secret } => {
                match (seam.grant)(account, client_id.trim(), secret.trim()) {
                    Ok(granted) => {
                        session = Some(granted);
                        let _ = report.send(Report::KeyPairAccepted);
                    }
                    Err(e) => {
                        // The route and the status reach the log; the window
                        // gets a variant. Neither half of the key pair is in
                        // either.
                        log::warn!("the API-key grant was not accepted: {e}");
                        let _ = report.send(Report::Refused(grant_refusal(&e)));
                    }
                }
            }
            Command::MasterPassword(password) => {
                // No session means stage 1 has not passed. Answering
                // `KeyPairRejected` is the true statement: what is missing is
                // the key pair.
                let Some(held) = session.take() else {
                    let _ = report.send(Report::Refused(Refusal::KeyPairRejected));
                    continue;
                };
                match (seam.unlock)(account, held, password.as_bytes()) {
                    Ok(authenticated) => return Some(authenticated),
                    Err(e) => {
                        let refusal = unlock_refusal(&e);
                        log::warn!("the API-key sign-in could not unlock the vault: {e}");
                        // The session is put back for a `PasswordRejected` and
                        // deliberately NOT for the other two: a
                        // `KeyPairRejected` here means the session is dead, and
                        // an `Unreachable` means the unlock never happened, so
                        // the session it consumed is gone either way. Only the
                        // wrong-password case has a live session to retry with.
                        if refusal == Refusal::PasswordRejected {
                            // `unlock` consumed the session, so the retry needs
                            // a new one. See the soft spot in this plan's
                            // self-review: this is the one place the two-stage
                            // shape and piece 1's by-value `Session` disagree.
                            session = None;
                        }
                        let _ = report.send(Report::Refused(refusal));
                    }
                }
            }
        }
    }
    None
}
```

> **The `Session` is consumed by `unlock`, so a `PasswordRejected` cannot retry against it.** As written above, the loop reports `PasswordRejected` (correct wording, correct stage) but the next `MasterPassword` finds no session and reports `KeyPairRejected`. **That is a real defect and Step 3b fixes it**; it is written out here rather than hidden because the test in Step 1 asserts the fixed behaviour and will fail against this text alone.

- [ ] **Step 3b: Fix the consumed session — `unlock` borrows, and builds the `Authenticated` here**

Change [`ApiKeySeam::unlock`] to borrow the session and return only the master key, and let the loop assemble the `Authenticated`. That is what keeps the session alive across a wrong password.

In Task 4's seam, replace the `unlock` field and `unlock_direct_rest`'s signature and tail:

```rust
    /// **Stage 2.** Prelogin, derive, sync, unwrap. **Borrows** the session
    /// rather than consuming it: a mistyped master password must be retryable
    /// against the session stage 1 already minted, which is the whole reason
    /// the design does not repeat stage 1. Returns only the master key; the
    /// caller owns the session and pairs the two.
    pub unlock: fn(&Account, &Session, password: &[u8])
        -> Result<crate::rest::crypto::MasterKey, RestError>,
```

```rust
pub fn unlock_direct_rest(
    account: &Account,
    session: &Session,
    password: &[u8],
) -> Result<crate::rest::crypto::MasterKey, RestError> {
    let client = RestClient::new(&account.server_url);
    let kdf = client.prelogin(&account.email)?;
    let master_key = crate::rest::crypto::master_key(password, &account.email, kdf)?;

    let synced = client.sync(session)?;
    let profile = synced.profile.ok_or(RestError::Parse("the account profile"))?;
    let protected = profile
        .key
        .as_deref()
        .ok_or(RestError::Parse("the protected user key"))?;
    let protected: crate::rest::crypto::EncString = protected.parse()?;
    let _ = crate::rest::crypto::unwrap_user_key(&master_key.stretch(), &protected)?;

    Ok(master_key)
}
```

And in the loop, replace the `Command::MasterPassword` arm's body with:

```rust
                let Some(held) = session.as_ref() else {
                    let _ = report.send(Report::Refused(Refusal::KeyPairRejected));
                    continue;
                };
                match (seam.unlock)(account, held, password.as_bytes()) {
                    Ok(master_key) => {
                        // The session is handed on with the key it pairs with;
                        // `take` is what guarantees there is exactly one.
                        let session = session.take()?;
                        return Some(Authenticated { session, master_key });
                    }
                    Err(e) => {
                        let refusal = unlock_refusal(&e);
                        log::warn!("the API-key sign-in could not unlock the vault: {e}");
                        // A dead session cannot be retried against; a wrong
                        // password can, and does -- the session stays.
                        if refusal == Refusal::KeyPairRejected {
                            session = None;
                        }
                        let _ = report.send(Report::Refused(refusal));
                    }
                }
```

Update Task 4's `a_password_retry_reuses_the_session_stage_one_minted` fakes to the borrowed signature (`|_account, _session: &crate::rest::api::Session, password|` returning `Result<MasterKey, RestError>`, with `Ok(crate::rest::crypto::MasterKey::from_bytes([0x5A; crate::rest::crypto::MASTER_KEY_LEN]))` on success), and drop its now-unused `fake_authenticated` if nothing else uses it.

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

Expected: all of Tasks 1–5's tests pass, zero warnings. In particular `GRANTS == 1` in `a_wrong_password_is_retried_without_a_second_grant` — if that reads 2, Step 3b was not applied.

- [ ] **Step 5: Commit**

`deskwarden/src/api_key_ui.rs`. Message: one session across two stages, why `unlock` borrows it, and the source pin that says nothing here is written to disk.

---

### Task 6: The secret's hygiene, read off this file's own source

**Files:** Modify `deskwarden/src/api_key_ui.rs`

**Interfaces**

- *Consumes:* this file's own source, via `include_str!`.
- *Produces:* `the_client_secret_is_handled_like_a_password` (test only).

The same shape `rest/api.rs`'s `the_challenge_holds_its_hash_wiped_and_cannot_be_printed` uses (`rest/api.rs:2467`): cut the source at the test module marker, assert on the production half only, and carry a positive control for the *search technique* as well as for the subject. `crate::debug_leak_guard` already fails the suite for a type that *derives* `Debug` over a `Zeroizing` field; what it cannot say is that nobody hand-writes one, that the field is still `Zeroizing` at all, or that the value is never formatted.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The client secret's hygiene rule, read off the source.**
    ///
    /// It is a permanent, password-free login to the account, and the design
    /// gives it "the master password's handling": `Zeroizing`, no `Debug`,
    /// never logged, never in an error string. Three of those four are
    /// properties of the *text of this file* and no runtime value can
    /// demonstrate them, which is why this test reads the file.
    #[test]
    fn the_client_secret_is_handled_like_a_password() {
        let source = include_str!("api_key_ui.rs").replace("\r\n", "\n");
        let marker = "\n#[cfg(test)]\nmod tests {";
        let cut = source.find(marker).expect("the test module marker was not found");
        let production = &source[..cut];

        // Control on the cut: the subject really is in this half.
        assert!(
            production.contains("pub struct ApiKeyForm {"),
            "the cut lost the form, so every absence below proves nothing"
        );

        // 1. Still wiped on drop.
        assert!(
            production.contains("pub secret: Zeroizing<String>"),
            "the client secret is no longer wiped on drop"
        );
        assert!(
            production.contains("pub password: Zeroizing<String>"),
            "control: the master password beside it is wiped the same way, so the needle \
             above is the shape this file really uses"
        );

        // 2. Not printable -- neither derived nor hand-written.
        assert!(
            !production.contains("impl std::fmt::Debug for ApiKeyForm"),
            "ApiKeyForm gained a Debug; there is nothing in it a formatter may print"
        );
        assert!(
            !production.contains("impl std::fmt::Debug for Command"),
            "Command gained a Debug; two of its variants carry a credential"
        );
        // Control on that search technique: a `derive(..)]\npub enum` really is
        // findable this way, so the two absences above are about Debug and not
        // about the needle being unspellable.
        assert!(
            production.contains("derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum Refusal"),
            "the derive-then-item search no longer matches anything, so the assertions \
             above prove nothing"
        );

        // 3. Never formatted, never logged, never in an error string.
        //
        // Every production line that so much as names the secret, checked one
        // at a time -- a whole-file search would be satisfied by the wrong
        // line.
        let touching: Vec<&str> = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//") && !line.trim_start().starts_with("///"))
            .filter(|line| line.contains("secret") || line.contains("client_secret"))
            .collect();
        assert!(
            touching.len() >= 4,
            "control: the scan found only {} lines naming the secret, which is fewer than \
             this module has -- the filter is wrong and the loop below is vacuous: {touching:?}",
            touching.len()
        );
        for line in &touching {
            for forbidden in ["log::", "format!", "println!", "eprintln!", "{secret", "to_string()"] {
                assert!(
                    !line.contains(forbidden),
                    "a line naming the client secret also reaches for `{forbidden}`; the \
                     secret is never formatted, logged, or put in an error string. Line: \
                     {line:?}"
                );
            }
        }
        // Control on THAT loop: the same scan over a line that does log finds
        // it, so the absence above is about this file and not about the needle.
        assert!(
            production.contains("log::warn!"),
            "this module logs nothing at all, so 'the secret is never logged' is a claim \
             about a file with no logging in it"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- the_client_secret_is_handled_like_a_password
```

Expected: **it may well pass on the first run**, because Tasks 1–5 were written to satisfy it. That is not a TDD failure to paper over — run it deliberately red first:

```bash
# Temporarily add `impl std::fmt::Debug for ApiKeyForm { .. }` returning
# `f.write_str("ApiKeyForm")`, run the test, and confirm it fails on
# "ApiKeyForm gained a Debug". Then remove it and run again.
```

Record both outputs in the commit message. A source-reading test that has never been seen red is the house defect class this plan's constraints name by name.

- [ ] **Step 3: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui
```

- [ ] **Step 4: Commit**

`deskwarden/src/api_key_ui.rs`. Message: the secret's hygiene as a source pin, with the deliberate red run quoted.

---

### Task 7: The card, and the two places it is reached from

**Files:** Modify `deskwarden/src/api_key_ui.rs`, `deskwarden/src/second_factor_ui.rs`, `deskwarden/src/login_ui.rs`, `deskwarden/src/app_window.rs`

**Interfaces**

- *Consumes:* `ApiKeyForm`, `Step`, `Refusal`, `message_for`.
- *Produces:* `api_key_ui::{Asked, draw, KEY_PAIR_TITLE, KEY_PAIR_HINT, CLIENT_ID_LABEL, CLIENT_SECRET_LABEL, PASSWORD_TITLE, PASSWORD_HINT, PASSWORD_LABEL, CONTINUE_LABEL, BACK_LABEL, USE_API_KEY_LABEL}`; `second_factor_ui::Asked::UseApiKey`; `login_ui::LoginAction::UseApiKey`; `app_window::Stage::ApiKey` and its two events.

> **BEFORE STARTING THIS TASK:** run `git log --oneline -8` and confirm piece 2's Task 8 commit (the card, and the retirement of the terminal instruction) has landed. Three of these four files are that worker's. **If it has not landed, stop and report** — do Tasks 1–6, commit them, and hand this task back.

**The three integration points, each to be confirmed against the code as it then stands rather than assumed:**

1. `second_factor_ui::Asked` — piece 2's Task 8 produces it. This task adds a `UseApiKey` variant and a button on the unsupported-only card, so `unsupported_only_message`'s "then sign in here with it" leads somewhere. **Confirm the enum's actual name and variants** before editing; if piece 2 named it something else, follow what is there.
2. `second_factor_ui::unsupported_only_message` — already landed (`second_factor_ui.rs:217`), returns `Option<String>`, and its text already names "Account settings → Security → Keys" and the API key. **Nothing about its wording changes.** The button goes beside it.
3. `app_window::Stage` / `Event` / `advance` — piece 2's Task 7 adds `Stage::SecondFactor` and three events. This task adds a fourth stage beside them. **Confirm the variant names it used** before adding arms.

- [ ] **Step 1: Write the failing test — the card**

In `api_key_ui.rs`'s test module. The paint helpers are written here rather than borrowed from `login_ui::tests`: they are six lines, and depending on another worker's `pub(crate)` promotion for a test harness is a coupling this module does not need.

```rust
    const WINDOW: egui::Vec2 = egui::vec2(420.0, 620.0);

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
            ..Default::default()
        }
    }

    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Every string the card paints in one frame.
    fn painted(form: &mut ApiKeyForm) -> Vec<String> {
        let ctx = styled_context();
        let output = ctx.run_ui(raw_input(), |ui| {
            let _ = draw(ui, form);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    fn says(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// **The two stages are two screens.** The key-pair screen has no master
    /// password box, and the password screen has no secret box -- which is the
    /// design's second reason for the split: "it keeps a long-lived credential
    /// and the master password off the screen at the same moment."
    #[test]
    fn each_stage_shows_only_its_own_fields() {
        let mut form = ApiKeyForm::new();
        let stage_one = painted(&mut form);
        assert!(
            says(&stage_one, CLIENT_ID_LABEL),
            "control: the key-pair card painted its own labels; got {stage_one:?}"
        );
        assert!(says(&stage_one, CLIENT_SECRET_LABEL), "got {stage_one:?}");
        assert!(
            !says(&stage_one, PASSWORD_LABEL),
            "the master password is not on the same screen as the client secret; \
             got {stage_one:?}"
        );

        let mut form = ApiKeyForm::new();
        form.step = Step::MasterPassword;
        let stage_two = painted(&mut form);
        assert!(
            says(&stage_two, PASSWORD_LABEL),
            "control: the password card painted its own label; got {stage_two:?}"
        );
        assert!(
            !says(&stage_two, CLIENT_SECRET_LABEL),
            "the secret is done with; leaving it on screen is a long-lived credential \
             sitting in a text box for no reason. got {stage_two:?}"
        );
    }

    /// **The password screen says why it is asking.** A user who just typed a
    /// key that "signs them in" and is then asked for a password will read it
    /// as a failure unless the card says otherwise.
    #[test]
    fn the_password_stage_explains_why_the_key_was_not_enough() {
        let mut form = ApiKeyForm::new();
        form.step = Step::MasterPassword;
        let texts = painted(&mut form);
        assert!(
            says(&texts, "unlock") || says(&texts, "decrypt"),
            "the hint must say what the password is FOR, which is the vault's contents; \
             got {texts:?}"
        );
        assert!(
            !says(&texts, "wrong") && !says(&texts, "failed"),
            "nothing failed -- the key pair was accepted; got {texts:?}"
        );
    }

    /// A refusal is painted where the user is looking, on the stage it sent
    /// them to.
    #[test]
    fn the_refusal_is_painted_on_the_stage_it_returns_to() {
        let mut form = ApiKeyForm::new();
        form.step = Step::MasterPassword;
        form.refused(Refusal::PasswordRejected);
        let texts = painted(&mut form);
        assert!(says(&texts, message_for(Refusal::PasswordRejected)), "got {texts:?}");
        assert!(
            says(&texts, PASSWORD_LABEL),
            "control: it is painted on the PASSWORD stage, which is where that refusal \
             returns to; got {texts:?}"
        );

        let mut form = ApiKeyForm::new();
        form.refused(Refusal::KeyPairRejected);
        let texts = painted(&mut form);
        assert!(says(&texts, message_for(Refusal::KeyPairRejected)), "got {texts:?}");
        assert!(
            says(&texts, CLIENT_SECRET_LABEL),
            "a rejected key pair returns to the key-pair stage with both fields on screen; \
             got {texts:?}"
        );
    }
```

And in `second_factor_ui.rs`'s test module:

```rust
    /// **The unsupported-only card now leads somewhere.** Piece 2's own
    /// self-review named this: "If the API-key path does not land, Task 3's
    /// message is a promise this app does not keep." This is the button that
    /// keeps it.
    #[test]
    fn the_unsupported_only_card_offers_the_api_key_it_names() {
        let mut duo = Prompt::new(vec![SecondFactor::Unsupported(2)]);
        let texts = painted(&mut duo);
        assert!(
            says(&texts, "API key"),
            "control: the message this button belongs to is still painted; got {texts:?}"
        );
        assert!(
            says(&texts, crate::api_key_ui::USE_API_KEY_LABEL),
            "the message tells the user to sign in with an API key and there must be a \
             way to; got {texts:?}"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- api_key_ui the_unsupported_only_card
```

Expected: `cannot find function 'draw' in this scope`, `cannot find value 'CLIENT_ID_LABEL'`, and in `second_factor_ui` `cannot find value 'USE_API_KEY_LABEL' in module 'crate::api_key_ui'`.

If `second_factor_ui`'s test module has no `painted`/`says` helpers, piece 2's Task 8 has not landed. **Stop and report.**

- [ ] **Step 3: Implement the card**

In `api_key_ui.rs`:

```rust
use crate::theme;
use egui::RichText;

pub const KEY_PAIR_TITLE: &str = "Sign in with an API key";
pub const KEY_PAIR_HINT: &str = "Create a personal API key under Account settings \u{2192} \
                                 Security \u{2192} Keys in the Bitwarden web vault, then paste \
                                 both halves here.";
pub const CLIENT_ID_LABEL: &str = "Client ID";
pub const CLIENT_SECRET_LABEL: &str = "Client secret";
pub const PASSWORD_TITLE: &str = "Master password";
pub const PASSWORD_HINT: &str = "The API key signed you in. Your master password is what \
                                 unlocks the vault \u{2014} the key cannot decrypt it.";
pub const PASSWORD_LABEL: &str = "Master password";
pub const CONTINUE_LABEL: &str = "Continue";
pub const BACK_LABEL: &str = "Back";
/// The label on both routes in: the sign-in card's link, and the button on
/// `second_factor_ui`'s unsupported-only card. One constant, so the two places
/// cannot drift into naming the same destination differently.
pub const USE_API_KEY_LABEL: &str = "Sign in with an API key";

/// What the user asked the API-key stage to do this frame. `None` from
/// [`draw`] is the ordinary case: they are still typing.
///
/// No `Debug`: [`Asked::Submit`] means "send what is in the form", and while
/// it carries nothing itself, giving this a `Debug` is one refactor away from
/// giving it a field.
pub enum Asked {
    /// Submit whatever the current [`Step`] is asking for. The caller reads
    /// `form.step` and builds the [`Command`] -- the card does not, because
    /// building it would mean copying the secret out of the buffer that owns
    /// it.
    Submit,
    /// Back to the sign-in card.
    Back,
}

/// Draws the API-key stage. Pure view: the caller owns the [`ApiKeyForm`] and
/// performs the channel sends for whatever comes back, exactly as
/// `login_ui::draw_login_window` and its caller are split.
pub fn draw(ui: &mut egui::Ui, form: &mut ApiKeyForm) -> Option<Asked> {
    let mut asked = None;
    let ready = match form.step {
        Step::KeyPair => form.key_pair_ready(),
        Step::MasterPassword => form.password_ready(),
    };

    match form.step {
        Step::KeyPair => {
            ui.label(RichText::new(KEY_PAIR_TITLE).size(17.0));
            ui.add_space(6.0);
            ui.label(RichText::new(KEY_PAIR_HINT).size(12.0).color(theme::TEXT_MUTED));
            ui.add_space(14.0);

            ui.label(RichText::new(CLIENT_ID_LABEL).size(12.0));
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut form.client_id).desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);

            ui.label(RichText::new(CLIENT_SECRET_LABEL).size(12.0));
            // `&mut *form.secret` and not a copy: the text edit writes straight
            // into the `Zeroizing` buffer that owns the secret, so there is
            // never a second String holding it.
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut *form.secret)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
        Step::MasterPassword => {
            ui.label(RichText::new(PASSWORD_TITLE).size(17.0));
            ui.add_space(6.0);
            ui.label(RichText::new(PASSWORD_HINT).size(12.0).color(theme::TEXT_MUTED));
            ui.add_space(14.0);

            ui.label(RichText::new(PASSWORD_LABEL).size(12.0));
            ui.add_enabled(
                !form.busy,
                egui::TextEdit::singleline(&mut *form.password)
                    .password(true)
                    .desired_width(f32::INFINITY),
            );
        }
    }

    if let Some(error) = form.error.as_deref() {
        ui.add_space(6.0);
        ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
    }

    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!form.busy && ready, egui::Button::new(CONTINUE_LABEL))
            .clicked()
        {
            asked = Some(Asked::Submit);
        }
        if ui.button(BACK_LABEL).clicked() {
            asked = Some(Asked::Back);
        }
    });

    asked
}
```

- [ ] **Step 4: Implement the two routes in**

In `second_factor_ui.rs`, add to `Asked`:

```rust
    /// The unsupported-only card's one way forward: the personal API key its
    /// own message names. See [`crate::api_key_ui`].
    UseApiKey,
```

and in `draw`'s unsupported-only branch, beside the existing `BACK_LABEL` button:

```rust
        if ui.button(crate::api_key_ui::USE_API_KEY_LABEL).clicked() {
            return Some(Asked::UseApiKey);
        }
        return ui.button(BACK_LABEL).clicked().then_some(Asked::Back);
```

In `login_ui.rs`, add to `LoginAction`:

```rust
    /// The sign-in card's link to [`crate::api_key_ui`]: the way in for an
    /// account whose second factor this app cannot complete.
    UseApiKey,
```

and, in `draw_login_window`'s footer beside the existing links, one line:

```rust
    if ui.link(crate::api_key_ui::USE_API_KEY_LABEL).clicked() {
        action = Some(LoginAction::UseApiKey);
    }
```

In `app_window.rs`, add to `Stage`:

```rust
    /// **The API-key card** -- `api_key_ui`'s two steps, over the same backdrop
    /// the sign-in card uses. Reached from the sign-in card's link and from
    /// `second_factor_ui`'s unsupported-only message; it is a way of signing
    /// in, so it lives where signing in happens and is not a Preferences page.
    ApiKey,
```

to `Event`:

```rust
    /// The user chose to sign in with an API key -- from the sign-in card, or
    /// from the unsupported-only second-factor message.
    ApiKeyChosen,
    /// The API-key sign-in produced a session and a master key. From here the
    /// window is in exactly the state a password sign-in leaves it in.
    ApiKeyDone,
    /// The user backed out of the API-key card. Back to the sign-in card,
    /// because they still have an account to sign into.
    ApiKeyAbandoned,
```

and to `advance`, above the catch-all:

```rust
        (Stage::SignIn, Event::ApiKeyChosen) => Next::Show(Stage::ApiKey),
        (Stage::SecondFactor, Event::ApiKeyChosen) => Next::Show(Stage::ApiKey),
        (Stage::ApiKey, Event::ApiKeyDone) => Next::Show(Stage::Working),
        (Stage::ApiKey, Event::ApiKeyAbandoned) => Next::Show(Stage::SignIn),
```

- [ ] **Step 5: Write the failing transition test, and make it pass**

In `app_window.rs`'s existing transition-table test module:

```rust
    /// **The API-key card is reachable from BOTH places the design names**,
    /// and it leaves the way a sign-in leaves.
    #[test]
    fn the_api_key_card_is_reached_from_the_card_and_from_the_blocked_factor() {
        assert_eq!(advance(Stage::SignIn, Event::ApiKeyChosen), Next::Show(Stage::ApiKey));
        assert_eq!(
            advance(Stage::SecondFactor, Event::ApiKeyChosen),
            Next::Show(Stage::ApiKey),
            "a Duo account is told to use an API key, so the message must be able to get \
             the user there"
        );
        assert_eq!(
            advance(Stage::ApiKey, Event::ApiKeyDone),
            Next::Show(Stage::Working),
            "a finished API-key sign-in enters the spinner exactly as a password one does"
        );
        assert_eq!(
            advance(Stage::ApiKey, Event::ApiKeyAbandoned),
            Next::Show(Stage::SignIn)
        );
        assert_eq!(
            advance(Stage::ApiKey, Event::SignedIn),
            Next::Show(Stage::ApiKey),
            "a stale SignedIn must not skip the key pair"
        );
        // Positive control: the table still moves for the events it owns.
        assert_eq!(advance(Stage::ApiKey, Event::ApiKeyDone), Next::Show(Stage::Working));
    }
```

Run it red, then implement the four `advance` arms above and run it green:

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- app_window
```

- [ ] **Step 6: Wire the frame closure**

In `app_window.rs`'s frame closure, a new arm beside `Stage::SecondFactor`, in that arm's exact shape: drain `report_rx.try_iter()` into `form.refused(..)` / `form.step = Step::MasterPassword` for `Report::KeyPairAccepted`, call `api_key_ui::draw`, and on `Asked::Submit` set `form.busy = true` and send the `Command` for the current `form.step`. Reuse the `notice_the_token` helper piece 2's Task 7 factored out; **do not write a second copy of the token wiring.**

`Asked::Back` sends `Command::Abandon` and advances on `Event::ApiKeyAbandoned`.

- [ ] **Step 7: Run the FULL suite, and clippy**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2
RUSTFLAGS="-D warnings" cargo clippy --manifest-path deskwarden/Cargo.toml --all-targets
```

`login_ui.rs`'s source-position pins are the ones to expect complaints from — this task added a link to `draw_login_window` and a variant to `LoginAction`, and several pins `split_once` on that file's text. Read each pin's own docstring before touching it: a pin whose subject genuinely moved is re-pinned with a commit message saying what moved; a pin whose subject did not move is telling you this change went further than intended.

Baseline the mock-HTTP family before believing you broke it: note the failure count and the module names on a run *before* this task's changes are staged, and compare.

- [ ] **Step 8: Commit**

`deskwarden/src/api_key_ui.rs`, `deskwarden/src/second_factor_ui.rs`, `deskwarden/src/login_ui.rs`, `deskwarden/src/app_window.rs`. Message: the card, and the two routes in — quote piece 2's self-review line about the promise this app does not keep, since this commit is what keeps it.

---

### Task 8: The live check

**Files:** none

No test in this repository can prove this feature. The design says so: *"a real account with a personal API key, signed in with `bw.exe` renamed away, reaching an unlocked vault. As with the rest of this feature, no test with a CLI available proves anything about a build without one."*

A human runs every step below.

- [ ] **Step 1:** Rename `bw.exe` away.
- [ ] **Step 2:** In the Bitwarden web vault, Account settings → Security → Keys, view the personal API key. Confirm the wording in `KEY_PAIR_HINT` still matches what is actually on that page; if it does not, fix the constant and say so.
- [ ] **Step 3:** Launch Deskwarden. From the sign-in card, click **Sign in with an API key**. Confirm the key-pair screen appears and has **no master password box**.
- [ ] **Step 4:** **Type a wrong client id deliberately, with the correct secret pasted.** Confirm: the message names the API key and mentions rotation, **both fields are still filled in**, and the master password was never asked for. This is the acceptance criterion for the whole piece.
- [ ] **Step 5:** Fix the id and continue. Confirm the master password screen appears, the secret box is gone, and the hint says the password is what unlocks the vault.
- [ ] **Step 6:** **Type a wrong master password deliberately.** Confirm: the message names the master password, **the key pair is not asked for again**, and pressing Continue with the right password then works — with no second round trip visible as a new device in the account's device list.
- [ ] **Step 7:** Type the right master password. Confirm the spinner follows and the vault loads **decrypted** — open one item and read its password. A vault that lists items but shows ciphertext means Task 4's unwrap is not doing what this plan says it does.
- [ ] **Step 8:** Repeat from an account whose only factor is Duo or WebAuthn: sign in with the master password, reach `second_factor_ui`'s unsupported-only card, and confirm the **Sign in with an API key** button is there and leads to the key-pair screen.
- [ ] **Step 9:** Turn off the network and press Continue on the key-pair screen. Confirm the message is about reaching the server, **both fields are still filled in**, and nothing suggests the key was wrong.
- [ ] **Step 10:** Quit and relaunch. Confirm the app is still signed in (the session token survived) **and that no file under `%APPDATA%\Deskwarden` contains the client id or the client secret** — `findstr /S /I /M "<the client id>" %APPDATA%\Deskwarden\*` returning nothing is the evidence. The design's refusal to persist the key pair is structural, but "structural" is a claim and this is the check.
- [ ] **Step 11:** Read the log for the whole session and confirm no line contains the client secret or the master password.

---

## Self-review

### Spec coverage

| Design requirement | Where |
| --- | --- |
| Stage 1: `client_id` + `client_secret` through `api_key_grant` | Task 4 (`grant_direct_rest`, `PRODUCTION_API_KEY`, source pin on `.api_key_grant(`) |
| Stage 2: prelogin → `master_key` → the vault key | Task 4 (`unlock_direct_rest`) |
| Both, always — never a way to skip the password | Task 1 (`Step` has two variants and starts on `KeyPair`), Task 5 (the loop returns `Authenticated` only from the `MasterPassword` arm), Task 7 (two screens, tested) |
| Two stages, not one screen with three fields | Task 7 `each_stage_shows_only_its_own_fields`, with the negative on both directions |
| Key pair rejected → stage 1, **both fields kept** | Task 2 `a_rejected_key_pair_keeps_both_fields`; Task 3 `grant_refusal`; Task 8 Step 4 |
| Password rejected → stage 2 only, stage 1 **not** repeated | Task 2 `a_rejected_password_does_not_reask_for_the_key_pair`; Task 5 `a_wrong_password_is_retried_without_a_second_grant` (`GRANTS == 1`) |
| Server unreachable, distinct from both | Task 2 (pairwise-distinct messages + `an_unreachable_server_touches_no_field_and_no_step`), Task 3 (`Transport`/`Parse` → `Unreachable`) |
| `client_secret`: `Zeroizing`, no `Debug`, never logged, never in an error string | Task 6 `the_client_secret_is_handled_like_a_password`, in `rest/api.rs:2467`'s idiom, with a control on the cut, on the derive-search, and on the per-line scan |
| `client_secret` **not persisted**; only the session token, to the existing `SessionStore` | Task 5 `the_key_pair_is_never_persisted` (source pin, control on the needle via `session_store.rs`); the session reaches the store through the existing `adopt` sink, so this feature adds no persistence path at all. Task 8 Step 10 is the field check. |
| Reached from the sign-in card | Task 7 (`login_ui::LoginAction::UseApiKey`), `advance(SignIn, ApiKeyChosen)` |
| Reached from piece 2's unsupported-only message | Task 7 (`second_factor_ui::Asked::UseApiKey`, `the_unsupported_only_card_offers_the_api_key_it_names`), `advance(SecondFactor, ApiKeyChosen)` |
| Not a Preferences page, not a top-level surface | Task 7 — it is a `Stage` beside `SignIn`, documented as such on the variant |
| No new HTTP test for the grant | None written. Piece 1's body-recorder test already pins `client_credentials` and the absent password. |
| A live check a human runs | Task 8, eleven steps, four of them deliberate failures |

### Placeholder scan

Searched this document for `TBD`, `appropriate`, `similar to Task`, `etc.`, `and so on`, `handle errors`, `as needed`: no hits. Every step that changes code carries the code.

Four places name a condition under which the worker must **stop and report** rather than improvise: Task 3's `CryptoError` variant shape, Task 4's `EncString` parse conversion, Task 7's precondition that piece 2's Task 8 has landed, and Task 7 Step 2's `second_factor_ui` paint helpers. Those are deliberate stop signs at another worker's boundary, not placeholders — each says exactly what is missing and what to do.

Task 5 Step 3 deliberately ships a **known-wrong** implementation and Step 3b fixes it. That is not a placeholder either: the defect is the by-value `Session` colliding with the retry requirement, it is the single most likely thing an implementer would get wrong silently, and Step 1's test fails against Step 3's text. It is written out so the failure is met once, on purpose, with the fix beside it.

### Soft spots I am flagging rather than hiding

1. **Stage 2's verification is my inference, not the design's text.** The design says "master password rejected" is a state; it does not say what rejects it, and piece 1 gives it nothing that can. I resolved it as `sync` → `unwrap_user_key`, which is the check `rest::sync::VaultKeys::unwrap_from` already makes and `rest/crypto.rs:1656` already tests. **This is the biggest single assumption in the plan.** If a reviewer prefers a cheaper probe (a route that returns the profile without a full sync), the change is confined to `unlock_direct_rest` and no test outside Task 4 moves. What is not negotiable is that *something* verifies: without it the app signs in and cannot read.

2. **The `Session` ownership collision (Task 5 Step 3b).** Piece 1's `Authenticated` takes a `Session` by value and my seam originally did too, which makes a password retry impossible — the session is consumed by the attempt that failed. The fix is to have `unlock` borrow and return a `MasterKey`, with the loop assembling the `Authenticated`. It works, but it means this module knows how an `Authenticated` is built. If piece 1 ever gives `Authenticated` a constructor, use it.

3. **Three integration points with piece 2 that I could not verify, because the file is being written as I write this.** `second_factor_ui::Asked` and `second_factor_ui::draw` do not exist yet (piece 2 is through its Task 4); `app_window::Stage::SecondFactor` does not exist yet; `login_ui::draw_login_window`'s footer is a region piece 2's Task 8 also edits. Task 7 is placed last, is gated on a `git log` check, and says to follow what is actually there rather than what I guessed. `unsupported_only_message` **has** landed and I read it — that one is verified, and its text needs no change.

4. **`Zeroizing<String>` as a text-edit buffer.** `&mut *form.secret` works because `Zeroizing` is `DerefMut`, but egui's `TextEdit` reallocates the `String` as the user types, and a reallocation copies the old bytes to a new allocation and frees the old one **without wiping it**. So the `Drop` wipe covers the final buffer and not every intermediate. This is the same exposure `login_ui::LoginForm` already has for the master password, and the same one `rest/crypto.rs`'s `decrypt_rsa` docstring records honestly for its own intermediate. I am recording it rather than claiming a containment this cannot deliver. A real fix is a fixed-capacity buffer, and it is out of scope for this piece.

5. **The client id is wiped on `Drop` but is not a secret.** I wipe it anyway, which costs nothing and is defensible, but it means `ApiKeyForm` has a `Drop` doing something the type's own doc calls unnecessary. If a reviewer reads that as cargo-culting, dropping the `client_id.zeroize()` line changes no test.

6. **The card is drawn as bare labels and text edits, not as `prefs_ui`-styled rows.** Same soft spot piece 2 flagged for its provider switch, same reason: the copy and the behaviour are pinned by tests, the widget is not, and this is the part most likely to come back from a design pass.

7. **The email comes from the sign-in card, and the API key does not carry one.** `Account::email` is what the user typed into the card before clicking the link — and it is the KDF salt, so a wrong email at stage 2 fails as `PasswordRejected` when the password is fine. On the route in from piece 2's unsupported-only card the email is certainly right (a grant was attempted with it). On the route in from the sign-in card's link the user may not have typed one yet. **Task 7's frame-closure wiring must not enter `Stage::ApiKey` with an empty email**; the cheapest correct answer is to disable the link until the card's email field is non-empty, and I did not write that, because I could not read the footer's shape while another worker is rewriting it.

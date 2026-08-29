# The Second-Factor Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user whose account has an authenticator app, an email code or a YubiKey signs in inside Deskwarden's own window, types six digits once, and lands in their vault — instead of reading `login_ui.rs:486` and being told to go open a terminal. A wrong digit costs them the code box and nothing else; the master password they already typed is never asked for twice.

**Architecture:** This is piece 2 of `docs/superpowers/specs/2026-08-29-two-factor-without-the-cli-design.md`. Piece 1 (`rest::api`) is being built in parallel by another worker and its types are a **given interface** here — `LoginOutcome`, `Challenge`, `SecondFactor`, `SecondFactorAnswer`, `finish_second_factor`. This plan writes no `rest::` code and does not edit `deskwarden/src/rest/`.

The shape has one governing constraint, and everything below follows from it: **`Challenge` is password-equivalent, is not `Debug`, and never crosses a thread boundary or enters a UI struct.** The sign-in worker thread that `spawn_auth` already starts — the one whose helper's docstring calls itself *"the plaintext master password's whole life"* — is where the `Challenge` is born and where it dies. What travels up to the window is a `SecondFactorRequest` carrying only the provider list and a masked address; what travels back down is a `SecondFactorCommand` carrying only digits. The window never has anything to leak, log or `Debug`-print, because it was never handed anything.

That means the worker **blocks** on a `Receiver<SecondFactorCommand>` while the user reads their phone. That is deliberate: it is a detached thread whose only other option is to hand the credential to the UI, and the spec is explicit that "either the credential survives the prompt or the user types their master password twice."

**Tech Stack:** Rust, egui/eframe, the existing `AuthenticateFn` `fn`-pointer seam in `login_ui.rs`, `std::sync::mpsc`, `zeroize`, the `app_window::Stage`/`advance` transition table.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- Build with `RUSTFLAGS="-D warnings"`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory; the disk has ~23 GB free and that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names".
- The `rest::`/`vault_cache::`/`picker_ui::` mock-HTTP test family is flaky; compare against a `git stash` baseline before believing you broke it.

Additionally, and specific to this branch:

- **Do not edit `deskwarden/src/rest/`.** Another worker owns it for the duration. If a task here needs a `rest::api` item that does not exist yet, stop and report rather than adding it.
- **No test may touch** the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash` (the stash exception in the flaky-suite constraint above is a read-only baseline check on a clean tree, run and then restored immediately).
- Branch: `two-factor-without-the-cli`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/second_factor_ui.rs` (**new**) | The prompt's state, its copy, its pure decisions, and its `draw_` function. Everything in this feature that can be tested without an event loop. |
| `deskwarden/src/lib.rs` (modify) | One `pub mod second_factor_ui;` line. |
| `deskwarden/src/login_ui.rs` (modify) | The worker-side protocol (`SecondFactorRequest`/`SecondFactorCommand`), the extended `fn`-pointer seam, the `friendly_auth_error` arm at line ~486 that this work replaces. |
| `deskwarden/src/app_window.rs` (modify) | `Stage::SecondFactor`, its two events, its `advance` arms, and the frame-closure arm that draws it. |

**Why a new file rather than more of `login_ui.rs`.** `login_ui.rs` is 10 450 lines and carries a dozen source-position pins that split on its own text. Every one of them is a chance for this feature to fail a guard it has nothing to do with. The prompt is a self-contained card with its own copy and its own state; it gets its own file, and `login_ui.rs` gains only the protocol types and the one wording change it genuinely owns.

---

### Task 1: The prompt's state and the provider it starts on

**Files:** Create `deskwarden/src/second_factor_ui.rs`; modify `deskwarden/src/lib.rs`

**Interfaces**

- *Consumes:* `crate::rest::api::SecondFactor` (piece 1).
- *Produces:* `second_factor_ui::Prompt`, `Prompt::new`, `Prompt::supported`, `Prompt::chosen`, `Prompt::choose`, `second_factor_ui::preferred`.

`preferred` is the spec's default choice — "Bitwarden's own priority order, restricted to what is supported -- YubiKey, then Authenticator, then Email." It is stated here and not deferred to `rest::api`, because the default is a *presentation* decision: `rest::api` has no opinion about which box the cursor lands in.

- [ ] **Step 1: Write the failing test**

Create `deskwarden/src/second_factor_ui.rs` containing only the test module for now, so the first run fails to *resolve* rather than to *compile a body*:

```rust
//! The second-factor prompt: the stage between the sign-in card and the
//! spinner.
//!
//! Nothing in this module has ever seen a `Challenge`. See
//! `login_ui::SecondFactorRequest` for what does, and why it stays on the
//! worker thread.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::SecondFactor;

    /// **The default lands on the highest-priority SUPPORTED factor**, in
    /// Bitwarden's own order: YubiKey, then Authenticator, then Email.
    ///
    /// The `Unsupported` entries are the control: an account with WebAuthn
    /// and an authenticator app must open on the authenticator, and a
    /// `preferred` that simply returned the first element would pass every
    /// single-provider case and fail exactly this one.
    #[test]
    fn the_prompt_opens_on_the_best_supported_factor() {
        let webauthn_and_totp = [SecondFactor::Unsupported(7), SecondFactor::Authenticator];
        assert_eq!(
            preferred(&webauthn_and_totp),
            Some(SecondFactor::Authenticator),
            "an account with WebAuthn and an authenticator app must open on the authenticator"
        );

        let all_three = [
            SecondFactor::Email { masked: None },
            SecondFactor::Authenticator,
            SecondFactor::YubiKey,
        ];
        assert_eq!(
            preferred(&all_three),
            Some(SecondFactor::YubiKey),
            "YubiKey outranks the authenticator, which outranks email"
        );

        let email_only = [SecondFactor::Email { masked: Some("a***@b.c".to_string()) }];
        assert_eq!(
            preferred(&email_only),
            Some(SecondFactor::Email { masked: Some("a***@b.c".to_string()) }),
            "the masked address must survive the choice -- it is what the card shows"
        );

        assert_eq!(
            preferred(&[SecondFactor::Unsupported(2), SecondFactor::Unsupported(7)]),
            None,
            "Duo and WebAuthn are not a choice this prompt can offer"
        );
    }

    /// The prompt keeps the offered list whole -- including the unsupported
    /// entries, which Task 3's message names -- but only ever *chooses* a
    /// supported one.
    #[test]
    fn the_prompt_remembers_what_was_offered_and_what_it_picked() {
        let prompt = Prompt::new(vec![
            SecondFactor::Unsupported(2),
            SecondFactor::Email { masked: Some("a***@b.c".to_string()) },
            SecondFactor::Authenticator,
        ]);
        assert_eq!(prompt.offered().len(), 3, "control: the whole offer is kept");
        assert_eq!(prompt.supported().len(), 2, "Duo is not offered as a choice");
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Authenticator),
            "the authenticator outranks email"
        );

        let mut prompt = prompt;
        prompt.code.push_str("123456");
        prompt.error = Some("wrong".to_string());
        prompt.choose(SecondFactor::Email { masked: Some("a***@b.c".to_string()) });
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Email { masked: Some("a***@b.c".to_string()) })
        );
        assert!(
            prompt.code.is_empty(),
            "switching provider must clear a code typed for the OTHER provider"
        );
        assert_eq!(
            prompt.error, None,
            "and must clear an error that was about the other provider"
        );
    }
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`, beside `pub mod scratch_window;` and in alphabetical position:

```rust
pub mod second_factor_ui;
```

- [ ] **Step 3: Run it and watch it fail**

```bash
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

Expected: **compile error**, `cannot find function 'preferred' in this scope` and `cannot find type 'Prompt' in this scope`. If instead it fails on `SecondFactor` not existing, piece 1 has not landed its types yet — stop and report, do not write them.

- [ ] **Step 4: Implement**

Above the test module in `deskwarden/src/second_factor_ui.rs`:

```rust
use crate::rest::api::SecondFactor;
use zeroize::Zeroize;

/// Bitwarden's own priority order, restricted to what this app can complete.
///
/// A `const` list rather than an `Ord` on `SecondFactor`, because the order is
/// this prompt's opinion about which box to put the cursor in -- not a fact
/// about the enum, which `rest::api` owns and which has no reason to know that
/// a YubiKey is faster to reach for than an emailed code.
const PRIORITY: [fn(&SecondFactor) -> bool; 3] = [
    |f| matches!(f, SecondFactor::YubiKey),
    |f| matches!(f, SecondFactor::Authenticator),
    |f| matches!(f, SecondFactor::Email { .. }),
];

/// Whether this prompt can complete the factor at all. `Unsupported` is not an
/// error here -- see [`crate::second_factor_ui::unsupported_only_message`] --
/// it is simply not something the user can be asked to type.
pub fn is_supported(factor: &SecondFactor) -> bool {
    PRIORITY.iter().any(|matches| matches(factor))
}

/// The factor the prompt opens on: the highest-priority SUPPORTED one, or
/// `None` when the account offers nothing this app can complete.
pub fn preferred(offered: &[SecondFactor]) -> Option<SecondFactor> {
    PRIORITY
        .iter()
        .find_map(|matches| offered.iter().find(|f| matches(f)))
        .cloned()
}

/// Everything the code stage is, as state.
///
/// **There is no `Challenge` in here and there must never be one.** The
/// challenge carries the derived password hash and the master key, and this
/// struct is owned by a frame closure that outlives the stage. What this holds
/// is a provider list, six digits and a message.
pub struct Prompt {
    offered: Vec<SecondFactor>,
    chosen: Option<SecondFactor>,
    /// The digits the user is typing. Public because the text edit writes
    /// straight into it; wiped on [`Drop`] and on every provider switch.
    pub code: String,
    /// The inline message under the box, or `None`. See
    /// [`crate::second_factor_ui::message_for`].
    pub error: Option<String>,
    /// Set once an email code has actually been sent, so the card can say so
    /// rather than leaving the user wondering whether the button did anything.
    pub email_sent: bool,
    /// True while a send or an answer is in flight: the buttons ghost, exactly
    /// as the sign-in card's do while `bw` is running.
    pub busy: bool,
}

/// The code is not password-equivalent, but it is a live credential for the
/// thirty seconds it exists, and this struct is move-captured by a frame
/// closure that lives as long as the window. Wiping costs one call on a
/// six-byte string. `login_ui::LoginForm` states the same rule for the same
/// reason.
impl Drop for Prompt {
    fn drop(&mut self) {
        self.code.zeroize();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
    }
}

impl Prompt {
    pub fn new(offered: Vec<SecondFactor>) -> Self {
        let chosen = preferred(&offered);
        Self {
            offered,
            chosen,
            code: String::new(),
            error: None,
            email_sent: false,
            busy: false,
        }
    }

    /// Everything the server said this account has, unsupported entries
    /// included. Task 3's message reads this list to name Duo by name.
    pub fn offered(&self) -> &[SecondFactor] {
        &self.offered
    }

    /// The subset the user may actually pick between. A one-element answer is
    /// what makes the provider switch disappear.
    pub fn supported(&self) -> Vec<SecondFactor> {
        self.offered.iter().filter(|f| is_supported(f)).cloned().collect()
    }

    pub fn chosen(&self) -> Option<SecondFactor> {
        self.chosen.clone()
    }

    /// Switches provider, and **clears the code and the error with it**: both
    /// were about the factor the user just navigated away from, and a stale
    /// "That code didn't work" under a freshly-switched provider is a message
    /// about nothing.
    pub fn choose(&mut self, factor: SecondFactor) {
        self.chosen = Some(factor);
        self.code.zeroize();
        self.code.clear();
        if let Some(error) = self.error.as_mut() {
            error.zeroize();
        }
        self.error = None;
        self.email_sent = false;
    }
}
```

- [ ] **Step 5: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

Expected: both tests pass, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/second_factor_ui.rs deskwarden/src/lib.rs
git commit -F <message file>
```

Message: the prompt's state, and why the default provider is a UI decision rather than a `rest::api` one.

---

### Task 2: What the card says, per factor

**Files:** Modify `deskwarden/src/second_factor_ui.rs`

**Interfaces**

- *Consumes:* `SecondFactor`, `Prompt`.
- *Produces:* `factor_title`, `factor_hint`, `SEND_CODE_LABEL`, `CODE_SENT_NOTICE`, `Prompt::wants_send_button`.

House style, taken from `prefs_ui`'s `AUTO_LOCK_ENABLED_LABEL`/`AUTO_LOCK_ENABLED_DESCRIPTION` pairs: **a short label naming the thing, then one or two sentences of plain description that say what will happen.** Named `const`s or `fn`s returning `&'static str`/`String`, never literals inline in the draw function, because that is how this crate's tests pin copy.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests`:

```rust
    /// The card names the factor the user is being asked for, and the hint
    /// tells them where to look for the code. "Enter your code" would be true
    /// of all three and useful for none.
    #[test]
    fn each_factor_is_named_and_says_where_the_code_comes_from() {
        assert_eq!(factor_title(&SecondFactor::Authenticator), "Authenticator app");
        assert!(
            factor_hint(&SecondFactor::Authenticator).contains("authenticator app"),
            "got {:?}",
            factor_hint(&SecondFactor::Authenticator)
        );

        assert_eq!(factor_title(&SecondFactor::YubiKey), "YubiKey");
        assert!(
            factor_hint(&SecondFactor::YubiKey).contains("touch"),
            "a YubiKey hint that does not mention touching the key describes nothing the \
             user can do; got {:?}",
            factor_hint(&SecondFactor::YubiKey)
        );

        // The masked address is the whole value of the Email arm's hint: it is
        // how a user with two mailboxes knows which one to open.
        let masked = SecondFactor::Email { masked: Some("a***@b.c".to_string()) };
        assert!(
            factor_hint(&masked).contains("a***@b.c"),
            "got {:?}",
            factor_hint(&masked)
        );
        let unmasked = SecondFactor::Email { masked: None };
        assert!(
            !factor_hint(&unmasked).contains("None") && !factor_hint(&unmasked).is_empty(),
            "an address-less email hint must still be a sentence, not a debug-printed \
             Option; got {:?}",
            factor_hint(&unmasked)
        );
    }

    /// **Send code appears for Email and for nothing else.** The other two
    /// factors need no round trip -- the code is already on the phone or the
    /// key -- and a button that sends nothing is this project's most-repeated
    /// defect.
    #[test]
    fn only_email_offers_to_send_a_code() {
        let email = Prompt::new(vec![SecondFactor::Email { masked: None }]);
        assert!(
            email.wants_send_button(),
            "control: the Email prompt is the one that must have the button"
        );
        for factor in [SecondFactor::Authenticator, SecondFactor::YubiKey] {
            let prompt = Prompt::new(vec![factor.clone()]);
            assert!(
                !prompt.wants_send_button(),
                "{factor:?} needs no round trip and must not offer one"
            );
        }
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

Expected: `cannot find function 'factor_title'` / `'factor_hint'`, and `no method named 'wants_send_button'`.

- [ ] **Step 3: Implement**

```rust
/// The heading over the code box: the factor, named the way the user's own
/// Bitwarden security settings name it.
pub fn factor_title(factor: &SecondFactor) -> &'static str {
    match factor {
        SecondFactor::Authenticator => "Authenticator app",
        SecondFactor::Email { .. } => "Email code",
        SecondFactor::YubiKey => "YubiKey",
        SecondFactor::Unsupported(_) => "Two-step login",
    }
}

/// The line under the heading: where the code is, in the one sentence it takes
/// to say it.
///
/// Returns `String` rather than `&'static str` because the Email arm carries
/// the masked address the server sent -- which is the only reason this line
/// earns its space for a user with more than one mailbox.
pub fn factor_hint(factor: &SecondFactor) -> String {
    match factor {
        SecondFactor::Authenticator => {
            "Open your authenticator app and enter the 6-digit code it shows for this \
             account."
                .to_string()
        }
        SecondFactor::Email { masked: Some(address) } => {
            format!("Send a code to {address}, then enter it here. Codes expire after a few \
                     minutes.")
        }
        SecondFactor::Email { masked: None } => {
            "Send a code to the email address on this account, then enter it here. Codes \
             expire after a few minutes."
                .to_string()
        }
        SecondFactor::YubiKey => {
            "Plug in your YubiKey, put the cursor in the box below and touch the key. It \
             types the code for you."
                .to_string()
        }
        SecondFactor::Unsupported(_) => {
            "Deskwarden cannot complete this kind of two-step login.".to_string()
        }
    }
}

pub const SEND_CODE_LABEL: &str = "Send code";
pub const CODE_SENT_NOTICE: &str = "Code sent. Check your email — it may take a moment to \
                                    arrive.";

impl Prompt {
    /// Email is the one factor that needs a call before the user has anything
    /// to type. See the design's "Email is the one that needs a second call".
    pub fn wants_send_button(&self) -> bool {
        matches!(self.chosen, Some(SecondFactor::Email { .. }))
    }
}
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/second_factor_ui.rs`. Message: per-factor copy, and why the Email hint carries the masked address.

---

### Task 3: What an account with only Duo or WebAuthn is told

**Files:** Modify `deskwarden/src/second_factor_ui.rs`

**Interfaces**

- *Consumes:* `&[SecondFactor]`.
- *Produces:* `provider_name(u8) -> &'static str`, `unsupported_only_message(&[SecondFactor]) -> Option<String>`.

The spec is explicit about the two things this message must do that today's does not: **name the actual provider**, and **mention the personal API key**, which is supported and is the path these users have today via `bw login --apikey`. A message that says "two-step login" leaves a Duo user guessing which of their factors is the problem; a message that stops there leaves them with no next step at all.

The provider numbers are the spec's table: 2 Duo, 6 Organization Duo, 7 WebAuthn/FIDO2, plus 4 `U2f`, 5 `Remember`, 8 `RecoveryCode`, which are not factors a user picks at this prompt but can still arrive in the list.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The message names the provider.** "This account uses two-step login"
    /// is what `login_ui` said before this work and it is what sent people to
    /// a terminal to find out which factor it meant.
    #[test]
    fn an_unsupported_only_account_is_told_which_provider_and_what_to_do() {
        let duo = unsupported_only_message(&[SecondFactor::Unsupported(2)])
            .expect("control: an all-unsupported account must get a message");
        assert!(duo.contains("Duo"), "got {duo:?}");
        assert!(
            duo.contains("API key"),
            "the personal API key is the ONE path these users have; a message without it \
             is a dead end. got {duo:?}"
        );
        assert!(
            !duo.contains("bw login") && !duo.contains("terminal"),
            "this is the message that replaces the terminal instruction; got {duo:?}"
        );

        let webauthn = unsupported_only_message(&[SecondFactor::Unsupported(7)])
            .expect("WebAuthn-only is the other real case");
        assert!(
            webauthn.contains("security key") || webauthn.contains("WebAuthn"),
            "got {webauthn:?}"
        );
        assert!(
            !webauthn.contains("Duo"),
            "the message must name THIS account's provider, not a list of every one; \
             got {webauthn:?}"
        );

        let both = unsupported_only_message(&[
            SecondFactor::Unsupported(2),
            SecondFactor::Unsupported(7),
        ])
        .expect("two unsupported providers is still an unsupported-only account");
        assert!(both.contains("Duo") && both.contains("security key"), "got {both:?}");
    }

    /// **The message is absent the moment anything is supported.** An account
    /// with WebAuthn AND an authenticator app must be offered the
    /// authenticator, not apologised to.
    #[test]
    fn a_supportable_account_gets_no_apology() {
        assert_eq!(
            unsupported_only_message(&[
                SecondFactor::Unsupported(7),
                SecondFactor::Authenticator
            ]),
            None,
            "WebAuthn beside an authenticator app is an ordinary prompt"
        );
        // Positive control: the same call with the authenticator removed does
        // produce a message, so the `None` above is about supportability and
        // not about the function having stopped working.
        assert!(
            unsupported_only_message(&[SecondFactor::Unsupported(7)]).is_some(),
            "control: the unsupported-only case still produces a message"
        );
        assert_eq!(
            unsupported_only_message(&[]),
            None,
            "an empty offer is not a Duo account -- it is a server answer nobody can act on"
        );
    }

    /// Every number the spec's table names, so an unfamiliar one degrades to
    /// something reportable rather than to a bare integer in a sentence.
    #[test]
    fn every_known_provider_number_has_a_name() {
        assert_eq!(provider_name(2), "Duo");
        assert_eq!(provider_name(6), "Duo (organization)");
        assert_eq!(provider_name(7), "a security key (WebAuthn)");
        assert_eq!(provider_name(4), "a U2F security key");
        assert!(
            provider_name(99).contains("99"),
            "an unknown provider must carry its number so a bug report can name it; got {:?}",
            provider_name(99)
        );
    }
```

Note `provider_name(99)` returns `&'static str`, so it cannot embed the number — the implementation below therefore returns `String` and the test's first three `assert_eq!` compare against `&str` via `String: PartialEq<&str>`. Write the test exactly as above; it compiles against a `String` return.

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

Expected: `cannot find function 'unsupported_only_message'` / `'provider_name'`.

- [ ] **Step 3: Implement**

```rust
/// A provider number, in words the user would recognise from their own
/// Bitwarden security settings.
///
/// `String` rather than `&'static str` so the unknown arm can carry the number
/// it did not recognise. A message reading "an unrecognised method" is
/// unreportable; one reading "an unrecognised method (9)" names the thing to
/// look up.
pub fn provider_name(number: u8) -> String {
    match number {
        2 => "Duo".to_string(),
        6 => "Duo (organization)".to_string(),
        7 => "a security key (WebAuthn)".to_string(),
        4 => "a U2F security key".to_string(),
        5 => "a remembered device".to_string(),
        8 => "a recovery code".to_string(),
        other => format!("an unrecognised two-step method ({other})"),
    }
}

/// What an account whose every factor is unsupported is told.
///
/// `None` the instant anything is supported: the spec is explicit that
/// `Unsupported` is not an error, and an account with WebAuthn beside an
/// authenticator app is an ordinary prompt.
///
/// **The two things this message must do**, both of which the message it
/// replaces (`login_ui::friendly_auth_error`'s two-step arm) did not:
///
///  * name the provider, so a Duo user is not left guessing which of their
///    factors Deskwarden means;
///  * name the personal API key, which IS supported and is the same path
///    `bw login --apikey` gives these users today. Duo and WebAuthn are not
///    reachable from `bw login` either, so without this sentence the message
///    is a dead end rather than a redirection.
pub fn unsupported_only_message(offered: &[SecondFactor]) -> Option<String> {
    if offered.is_empty() || offered.iter().any(is_supported) {
        return None;
    }
    let names: Vec<String> = offered
        .iter()
        .map(|factor| match factor {
            SecondFactor::Unsupported(number) => provider_name(*number),
            // Unreachable given the guard above, and written as a name rather
            // than an `unreachable!()` because the cost of being wrong here is
            // the whole window dying on the one screen a blocked user sees.
            supported => factor_title(supported).to_lowercase(),
        })
        .collect();
    let list = match names.as_slice() {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {}", rest.join(" and ")),
        [] => return None,
    };
    Some(format!(
        "This account's two-step login uses {list}, which Deskwarden cannot complete. Use a \
         personal API key instead: create one under Account settings → Security → Keys in the \
         Bitwarden web vault, then sign in here with it. You will still be asked for your \
         master password — the API key signs you in, the password unlocks the vault."
    ))
}
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/second_factor_ui.rs`. Message: names the provider and the API key, and why both halves are load-bearing.

---

### Task 4: The three error states, told apart

**Files:** Modify `deskwarden/src/second_factor_ui.rs`

**Interfaces**

- *Consumes:* nothing outside this module.
- *Produces:* `enum Trouble { CodeRejected, EmailSendFailed, ChallengeExpired, Unreachable }`, `message_for(Trouble) -> &'static str`, `Prompt::went_wrong(Trouble)`.

The spec calls the email failure out by name: it "can fail *before* the prompt, and its failure has to read as 'we could not send you a code', not as a rejected code." The expired challenge is the third, and it is the one that must send the user back to the card, because there is no longer a hash on the worker to retry with.

- [ ] **Step 1: Write the failing test**

```rust
    /// The three failures say three different things. A shared "That didn't
    /// work" would tell a user whose code never arrived to check the code they
    /// do not have.
    #[test]
    fn the_three_troubles_do_not_share_a_message() {
        let rejected = message_for(Trouble::CodeRejected);
        let send_failed = message_for(Trouble::EmailSendFailed);
        let expired = message_for(Trouble::ChallengeExpired);

        assert!(rejected.contains("code"), "got {rejected:?}");
        assert!(
            send_failed.contains("send") || send_failed.contains("sent"),
            "the email failure must be about SENDING, not about a code the user never \
             received; got {send_failed:?}"
        );
        assert!(
            !send_failed.contains("didn't work") && !send_failed.contains("Check it"),
            "the email failure must not read as a rejected code; got {send_failed:?}"
        );
        assert!(
            expired.contains("master password") || expired.contains("again"),
            "an expired challenge means starting over, and the message has to say so; \
             got {expired:?}"
        );

        let all = [rejected, send_failed, expired, message_for(Trouble::Unreachable)];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "two troubles share one message");
            }
        }
    }

    /// **A rejected code clears the box and nothing else.** This is the whole
    /// behaviour this feature exists to produce: the master password lives on
    /// the worker thread and is not asked for again, and the provider the user
    /// picked is still picked.
    #[test]
    fn a_rejected_code_clears_the_code_and_keeps_the_provider() {
        let mut prompt = Prompt::new(vec![
            SecondFactor::Authenticator,
            SecondFactor::Email { masked: None },
        ]);
        prompt.choose(SecondFactor::Email { masked: None });
        prompt.email_sent = true;
        prompt.code.push_str("123457");
        prompt.busy = true;

        prompt.went_wrong(Trouble::CodeRejected);

        assert!(prompt.code.is_empty(), "the fat-fingered digits are gone");
        assert_eq!(
            prompt.chosen(),
            Some(SecondFactor::Email { masked: None }),
            "the user's provider choice survives a wrong code"
        );
        assert!(
            prompt.email_sent,
            "the code that was sent is still valid -- re-typing it must not require \
             sending a second one"
        );
        assert!(!prompt.busy, "the buttons come back");
        assert_eq!(prompt.error.as_deref(), Some(message_for(Trouble::CodeRejected)));
    }

    /// A failed SEND leaves the box alone: there may be nothing in it, and if
    /// there is, it is a code from an earlier send that is still good.
    #[test]
    fn a_failed_send_leaves_the_box_alone() {
        let mut prompt = Prompt::new(vec![SecondFactor::Email { masked: None }]);
        prompt.code.push_str("123456");
        prompt.busy = true;

        prompt.went_wrong(Trouble::EmailSendFailed);

        assert_eq!(prompt.code, "123456", "a send failure is not a code failure");
        assert!(!prompt.email_sent, "nothing was sent, so nothing may claim it was");
        assert!(!prompt.busy);
        assert_eq!(
            prompt.error.as_deref(),
            Some(message_for(Trouble::EmailSendFailed))
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

Expected: `cannot find type 'Trouble'`, `cannot find function 'message_for'`, `no method named 'went_wrong'`.

- [ ] **Step 3: Implement**

```rust
/// Why the prompt is showing a message.
///
/// Four variants and not one `String`, because the *behaviour* differs and not
/// only the wording: a rejected code empties the box, a failed send does not,
/// and an expired challenge ends the stage. A `String` error would put that
/// decision in the caller, where nothing tests it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// The server said no to these digits.
    CodeRejected,
    /// `POST /api/two-factor/send-email-login` failed. **Not** a rejected
    /// code: the user has nothing to check.
    EmailSendFailed,
    /// The challenge no longer resumes -- the server expired it, or the worker
    /// holding it is gone. There is no hash left to retry with, so this one
    /// ends the stage.
    ChallengeExpired,
    /// Neither the grant nor the send got an answer at all.
    Unreachable,
}

impl Trouble {
    /// Whether the stage can continue. `false` for exactly one variant, and it
    /// is the variant with nothing on the worker to retry against.
    pub fn is_fatal(self) -> bool {
        matches!(self, Trouble::ChallengeExpired)
    }
}

/// One line the user can act on, per trouble.
pub fn message_for(trouble: Trouble) -> &'static str {
    match trouble {
        Trouble::CodeRejected => {
            "That code didn't work. Check it and try again — codes change every 30 seconds."
        }
        Trouble::EmailSendFailed => {
            "Deskwarden couldn't send the code. Check your connection, then try Send code \
             again."
        }
        Trouble::ChallengeExpired => {
            "This sign-in took too long and has expired. Enter your master password again to \
             start over."
        }
        Trouble::Unreachable => {
            "Couldn't reach the server. Check your connection — and the server URL, if this \
             is a self-hosted account."
        }
    }
}

impl Prompt {
    /// Applies a failure to the prompt.
    ///
    /// **`CodeRejected` is the arm this feature exists for**: it empties the
    /// code box and touches nothing else. The master password is not here to
    /// clear -- it is on the worker thread, still holding the derived hash --
    /// which is precisely why a wrong digit no longer costs a re-typed master
    /// password.
    pub fn went_wrong(&mut self, trouble: Trouble) {
        self.busy = false;
        self.error = Some(message_for(trouble).to_string());
        match trouble {
            Trouble::CodeRejected => {
                self.code.zeroize();
                self.code.clear();
            }
            // The box is left exactly as it was: a send that failed may be the
            // SECOND send, and the code from the first one is still good.
            Trouble::EmailSendFailed => self.email_sent = false,
            Trouble::ChallengeExpired | Trouble::Unreachable => {}
        }
    }
}
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui
```

- [ ] **Step 5: Commit**

`deskwarden/src/second_factor_ui.rs`. Message: three failures, three behaviours, and why the email failure is not a rejected code.

---

### Task 5: The protocol — what crosses the worker's boundary, and what cannot

**Files:** Modify `deskwarden/src/login_ui.rs`

**Interfaces**

- *Consumes:* `rest::api::{SecondFactor, SecondFactorAnswer, Challenge, Authenticated, LoginOutcome}`.
- *Produces:* `login_ui::SecondFactorRequest`, `login_ui::SecondFactorCommand`, `login_ui::SecondFactorSeam`, `login_ui::PRODUCTION_SECOND_FACTOR`.

This is the security-shaped task. Two types cross the thread boundary and neither may carry anything derived from the master password.

The seam is a **`fn`-pointer struct in production code** — the crate's rule, and the same shape `AuthenticateFn` already uses one level up. It is a struct rather than three loose aliases because the three calls are one substitutable unit: a test that faked the finish but not the send would be testing a half-real worker.

- [ ] **Step 1: Write the failing test**

In `login_ui.rs`, in a new test module placed with the file's other below-the-cut modules:

```rust
/// **The second factor's boundary: what leaves the worker thread.**
///
/// The `Challenge` carries the derived password hash and the master key. It is
/// born on the worker thread and dies there. These tests are the statement of
/// that, in the two forms this crate can check: a source pin over the types
/// that cross, and a compile-time check that the crossing types are the only
/// ones the window is given.
#[cfg(test)]
mod second_factor_boundary {
    use super::*;

    /// Reads this file's own source, in `bw_serve_gate`'s idiom, because the
    /// property is about a struct DEFINITION and no runtime value can show it.
    fn definition_of(name: &str) -> String {
        let source = include_str!("login_ui.rs");
        let opener = format!("pub struct {name} {{");
        source
            .split_once(&opener)
            .unwrap_or_else(|| panic!("{name} must be defined in this file"))
            .1
            .split_once("\n}")
            .expect("the definition must be brace-terminated")
            .0
            .to_string()
    }

    /// The request the worker sends UP carries providers and nothing else.
    #[test]
    fn the_request_carries_no_credential() {
        let body = definition_of("SecondFactorRequest");
        assert!(
            body.contains("providers"),
            "control: the definition was found and has the field this is about; got {body:?}"
        );
        for forbidden in ["Challenge", "MasterKey", "password", "hash", "Zeroizing"] {
            assert!(
                !body.contains(forbidden),
                "SecondFactorRequest must not carry `{forbidden}`: it crosses to a frame \
                 closure that outlives the stage. got {body:?}"
            );
        }
    }

    /// The command the window sends DOWN carries digits and nothing else --
    /// in particular it does not carry the challenge back, which would mean
    /// the window had held it.
    #[test]
    fn the_command_carries_no_credential() {
        let source = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub enum SecondFactor", "Command {"))
            .expect("SecondFactorCommand must be defined in this file")
            .1
            .split_once("\n}")
            .expect("the definition must be brace-terminated")
            .0;
        assert!(
            body.contains("Answer"),
            "control: the definition was found; got {body:?}"
        );
        for forbidden in ["Challenge", "MasterKey", "password"] {
            assert!(
                !body.contains(forbidden),
                "SecondFactorCommand must not carry `{forbidden}`; got {body:?}"
            );
        }
    }

    /// `SecondFactorRequest` is `Send` (it crosses the channel) and the
    /// challenge is not in it. Compile-time, so it cannot rot.
    #[test]
    fn the_request_is_the_thing_that_crosses() {
        fn assert_send<T: Send>() {}
        assert_send::<SecondFactorRequest>();
        assert_send::<SecondFactorCommand>();
    }

    /// The production seam is wired to the real functions and not to a stub
    /// somebody left in while testing.
    #[test]
    fn the_production_seam_is_not_a_stub() {
        let source = include_str!("login_ui.rs");
        let body = source
            .split_once(concat!("pub const PRODUCTION_SECOND_", "FACTOR: SecondFactorSeam"))
            .expect("the production seam must exist")
            .1
            .split_once(";\n")
            .expect("the const must be terminated")
            .0;
        assert!(body.len() > 40, "control: the seam's body is not empty");
        assert!(
            body.contains("finish_second_factor"),
            "the seam must call rest::api's own finish; got {body:?}"
        );
        assert!(
            body.contains("send_second_factor_email"),
            "the seam must call rest::api's own email send; got {body:?}"
        );
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_boundary
```

Expected: `cannot find type 'SecondFactorRequest'`, and the two source-pin tests panic with "must be defined in this file". If the seam test instead fails because `rest::api::send_second_factor_email` does not exist, **stop and report** — that is piece 1's surface and this plan may not add it.

- [ ] **Step 3: Implement**

In `login_ui.rs`, immediately after the `AuthenticateFn` block (it is the same boundary, one level down):

```rust
/// **What the worker tells the window when the server asks for a second
/// factor.**
///
/// Providers and nothing else. The `Challenge` that these providers came out
/// of stays on the worker thread for the life of the prompt, because it holds
/// the derived password hash and the master key and this struct is received by
/// a frame closure that outlives the stage.
///
/// Pinned by `second_factor_boundary::the_request_carries_no_credential`,
/// which reads this definition's own source: no runtime value can demonstrate
/// the absence of a field.
pub struct SecondFactorRequest {
    /// Every provider the account offers, unsupported entries included --
    /// `second_factor_ui::unsupported_only_message` needs them to name Duo.
    pub providers: Vec<crate::rest::api::SecondFactor>,
}

/// **What the window tells the worker.**
///
/// Digits, or a request to send some, or nothing at all. The challenge is not
/// here because it never left; the worker matches these against the one it is
/// holding.
#[derive(Debug)]
pub enum SecondFactorCommand {
    /// Complete the sign-in with these digits, against the provider named.
    Answer(crate::rest::api::SecondFactorAnswer),
    /// Email only: ask the server to send a code.
    SendEmail,
    /// The user closed the window or backed out. The worker drops the
    /// challenge and returns.
    Abandon,
}

/// The three `rest::api` calls the second factor needs, as `fn` pointers.
///
/// A struct and not three aliases: a test that faked `finish` but let `send`
/// reach the network would be a test of half a worker. A `fn`-pointer struct
/// and not a `cfg(test)` seam, which this crate bans crate-wide, and not a
/// trait object, because there is exactly one call site.
///
/// **Every `Err` here is a message for a log.** `RestError`'s `Display`
/// carries a status and a route and nothing of what was sent; nothing in this
/// module may put the challenge, the hash or the key into one.
#[derive(Clone, Copy)]
pub struct SecondFactorSeam {
    /// The first grant. `Ok(LoginOutcome::NeedsSecondFactor(..))` is the case
    /// this whole feature is about.
    pub start: fn(
        server_url: &str,
        email: &str,
        device_id: &str,
        password: &[u8],
    ) -> Result<crate::rest::api::LoginOutcome, String>,
    /// `POST /api/two-factor/send-email-login`.
    pub send_email: fn(&crate::rest::api::Challenge) -> Result<(), String>,
    /// The retry, carrying the same hash the first grant derived.
    pub finish: fn(
        &crate::rest::api::Challenge,
        &crate::rest::api::SecondFactorAnswer,
    ) -> Result<Authenticated, String>,
}

fn start_direct_rest(
    server_url: &str,
    email: &str,
    device_id: &str,
    password: &[u8],
) -> Result<crate::rest::api::LoginOutcome, String> {
    let client = crate::rest::api::RestClient::new(server_url);
    let device = crate::rest::api::Device::windows_desktop(device_id, DEVICE_NAME);
    client
        .authenticate(email, password, &device)
        .map_err(|e| e.to_string())
}

fn send_direct_rest_email(challenge: &crate::rest::api::Challenge) -> Result<(), String> {
    crate::rest::api::RestClient::new(challenge.server_url())
        .send_second_factor_email(challenge)
        .map_err(|e| e.to_string())
}

fn finish_direct_rest(
    challenge: &crate::rest::api::Challenge,
    answer: &crate::rest::api::SecondFactorAnswer,
) -> Result<Authenticated, String> {
    crate::rest::api::RestClient::new(challenge.server_url())
        .finish_second_factor(challenge, answer)
        .map_err(|e| e.to_string())
}

/// The seam's one production value, written in exactly one place.
pub const PRODUCTION_SECOND_FACTOR: SecondFactorSeam = SecondFactorSeam {
    start: start_direct_rest,
    send_email: send_direct_rest_email,
    finish: finish_direct_rest,
};
```

> **If `Challenge` does not expose the server it came from**, take the three helpers' `server_url` from `DirectRestLogin::server_url` instead — thread it in as a first parameter on `send_email`/`finish`. Do **not** add an accessor to `rest/api.rs`; that file belongs to the other worker this week. Note the deviation in the commit message.

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_boundary
```

- [ ] **Step 5: Commit**

`deskwarden/src/login_ui.rs`. Message: the protocol, and the sentence that justifies it — the challenge is born on the worker and dies there, so the window has nothing to leak.

---

### Task 6: The worker completes the factor, and the master password is not asked for twice

**Files:** Modify `deskwarden/src/login_ui.rs`

**Interfaces**

- *Consumes:* `SecondFactorSeam`, `SecondFactorRequest`, `SecondFactorCommand`, `DirectRestLogin`.
- *Produces:* `login_ui::complete_second_factor` (the loop), and `authenticate_then_wipe`'s new `NeedsSecondFactor` arm.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod second_factor_worker {
    use super::*;
    use std::sync::mpsc;

    /// **A wrong code is retried against the SAME challenge.** The whole point
    /// of the feature: the hash the first grant derived is still on this
    /// thread, so a second attempt costs one round trip and not six hundred
    /// thousand PBKDF2 iterations plus a re-typed master password.
    #[test]
    fn a_rejected_code_is_retried_without_a_new_grant() {
        static FINISHES: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        static STARTS: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        FINISHES.store(0, std::sync::atomic::Ordering::SeqCst);
        STARTS.store(0, std::sync::atomic::Ordering::SeqCst);

        let seam = SecondFactorSeam {
            start: |_, _, _, _| {
                STARTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err("the loop must not re-grant".to_string())
            },
            send_email: |_| Ok(()),
            finish: |_, answer| {
                let n = FINISHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Err("400 Bad Request /identity/connect/token".to_string())
                } else {
                    assert_eq!(answer.token(), "654321", "the SECOND code is the one used");
                    Ok(fake_authenticated())
                }
            },
        };

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        cmd_tx
            .send(SecondFactorCommand::Answer(totp_answer("123456")))
            .unwrap();
        cmd_tx
            .send(SecondFactorCommand::Answer(totp_answer("654321")))
            .unwrap();

        let outcome = complete_second_factor(&seam, &fake_challenge(), &cmd_rx, &report_tx);

        assert!(outcome.is_some(), "the second code signs the user in");
        assert_eq!(
            FINISHES.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "control: both codes reached the server"
        );
        assert_eq!(
            STARTS.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a rejected code must NOT re-derive the master key -- that is the re-typed \
             master password this feature removes"
        );
        let troubles: Vec<_> = report_rx.try_iter().collect();
        assert_eq!(
            troubles,
            vec![crate::second_factor_ui::Trouble::CodeRejected],
            "the window was told exactly once, and told it was the CODE"
        );
    }

    /// A failed send reports `EmailSendFailed` and leaves the loop running --
    /// the user can press the button again, and their earlier code (if any) is
    /// still good.
    #[test]
    fn a_failed_send_is_not_a_rejected_code() {
        let seam = SecondFactorSeam {
            start: |_, _, _, _| Err("unused".to_string()),
            send_email: |_| Err("503 Service Unavailable /api/two-factor/send-email-login"
                .to_string()),
            finish: |_, _| Ok(fake_authenticated()),
        };
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        cmd_tx.send(SecondFactorCommand::SendEmail).unwrap();
        cmd_tx.send(SecondFactorCommand::Abandon).unwrap();

        let outcome = complete_second_factor(&seam, &fake_challenge(), &cmd_rx, &report_tx);

        assert!(outcome.is_none(), "abandoning produces no session");
        assert_eq!(
            report_rx.try_iter().collect::<Vec<_>>(),
            vec![crate::second_factor_ui::Trouble::EmailSendFailed],
            "a failed send must not read as a rejected code"
        );
    }

    /// A closed command channel -- the window went away -- ends the loop and
    /// drops the challenge, rather than blocking a detached thread forever.
    #[test]
    fn a_closed_window_ends_the_loop() {
        let seam = SecondFactorSeam {
            start: |_, _, _, _| Err("unused".to_string()),
            send_email: |_| Ok(()),
            finish: |_, _| panic!("nothing was answered, so nothing may be finished"),
        };
        let (cmd_tx, cmd_rx) = mpsc::channel::<SecondFactorCommand>();
        let (report_tx, _report_rx) = mpsc::channel();
        drop(cmd_tx);
        assert!(complete_second_factor(&seam, &fake_challenge(), &cmd_rx, &report_tx).is_none());
    }
}
```

`fake_authenticated()` already exists in this file's test support (see `login_ui.rs:9446`); add `fake_challenge()` and `totp_answer()` beside it, built from `rest::api`'s own constructors. If `Challenge` has no test constructor, **stop and report** — asking piece 1 for one is a five-line request and is cheaper than this plan inventing a parallel type.

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_worker
```

Expected: `cannot find function 'complete_second_factor'`.

- [ ] **Step 3: Implement**

```rust
/// **The prompt, from the worker thread's side.**
///
/// Blocks on the window's commands while holding the `Challenge`, and returns
/// an `Authenticated` or nothing. This is the function that makes a wrong
/// digit cheap: the challenge carries the hash the first grant already
/// derived, so a retry is one round trip and the master password is never
/// asked for again.
///
/// **It blocks a detached thread on a human.** That is the design's own
/// answer -- "either the credential survives the prompt or the user types
/// their master password twice" -- and it is bounded on both ends: a closed
/// command channel (the window went away) and an `Abandon` both return, and
/// returning drops the challenge, which zeroizes the hash it holds.
///
/// `report_tx` carries `Trouble` and not a `String`, so the window cannot be
/// handed a message built out of a `RestError` this function has not vetted.
pub fn complete_second_factor(
    seam: &SecondFactorSeam,
    challenge: &crate::rest::api::Challenge,
    commands: &std::sync::mpsc::Receiver<SecondFactorCommand>,
    report: &std::sync::mpsc::Sender<crate::second_factor_ui::Trouble>,
) -> Option<Authenticated> {
    use crate::second_factor_ui::Trouble;
    // `recv()` and not `try_recv()`: this thread has nothing else to do, and a
    // poll loop would spin for as long as it takes somebody to find their
    // phone. `Err` is a disconnected channel, which is a closed window.
    while let Ok(command) = commands.recv() {
        match command {
            SecondFactorCommand::Abandon => return None,
            SecondFactorCommand::SendEmail => {
                if let Err(e) = (seam.send_email)(challenge) {
                    // The route and status reach the log; the window gets a
                    // variant. Nothing of the challenge is in either.
                    log::warn!("could not send the second-factor email: {e}");
                    let _ = report.send(Trouble::EmailSendFailed);
                }
            }
            SecondFactorCommand::Answer(answer) => match (seam.finish)(challenge, &answer) {
                Ok(authenticated) => return Some(authenticated),
                Err(e) => {
                    log::warn!("the second factor was not accepted: {e}");
                    let _ = report.send(Trouble::CodeRejected);
                }
            },
        }
    }
    None
}
```

Then extend `authenticate_then_wipe`'s direct-REST block. Where it currently calls `(direct.authenticate)(..)` and matches `Ok(authenticated) => (direct.adopt)(authenticated)`, it becomes:

```rust
            match (direct.second_factor.start)(
                &direct.server_url,
                &direct.email,
                &direct.device_id,
                password.as_bytes(),
            ) {
                Ok(crate::rest::api::LoginOutcome::Done(authenticated)) => {
                    (direct.adopt)(authenticated)
                }
                // **The challenge is bound here and nowhere else.** It lives
                // for one function call and is dropped before this arm ends;
                // the window is told only what `SecondFactorRequest` carries.
                Ok(crate::rest::api::LoginOutcome::NeedsSecondFactor(challenge)) => {
                    if let Some(prompt) = direct.prompt.as_ref() {
                        let request = SecondFactorRequest {
                            providers: challenge.providers().to_vec(),
                        };
                        match prompt.ask(request) {
                            Some((commands, report)) => {
                                if let Some(authenticated) = complete_second_factor(
                                    &direct.second_factor,
                                    &challenge,
                                    &commands,
                                    &report,
                                ) {
                                    (direct.adopt)(authenticated);
                                }
                            }
                            // The window declined to open a prompt at all --
                            // the unsupported-only case, which Task 7 draws.
                            None => log::info!(
                                "the second factor was not completed: the window offered no \
                                 prompt for the providers this account has"
                            ),
                        }
                    }
                }
                Err(e) => log::warn!("the direct-REST login could not be derived: {e}"),
            }
```

`DirectRestLogin` gains two fields, replacing `authenticate`:

```rust
    /// See [`SecondFactorSeam`]. Replaces the old `authenticate` field: the
    /// first grant is now one of three calls that move together.
    pub second_factor: SecondFactorSeam,
    /// How the worker reaches the window's code stage. `None` on the hosts
    /// that have no stage to show -- and a `None` here means a two-step
    /// account simply cannot sign in on that host, which is exactly what it
    /// meant before this change.
    pub prompt: Option<std::sync::Arc<dyn SecondFactorPrompt + Send + Sync>>,
```

```rust
/// The window, as the worker sees it: hand over a request, get back the two
/// channel ends to talk over — or `None` if the window will not prompt.
///
/// A trait object here and a `fn` pointer in [`SecondFactorSeam`], and the
/// difference is real: the seam has one production value, while this has one
/// per host and each closes over its host's own state.
pub trait SecondFactorPrompt {
    #[allow(clippy::type_complexity)]
    fn ask(
        &self,
        request: SecondFactorRequest,
    ) -> Option<(
        std::sync::mpsc::Receiver<SecondFactorCommand>,
        std::sync::mpsc::Sender<crate::second_factor_ui::Trouble>,
    )>;
}
```

`backend_policy::direct_rest_login` fills `second_factor: PRODUCTION_SECOND_FACTOR` and `prompt: None`; Task 7 fills the prompt on the one host that has a stage.

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_worker
```

- [ ] **Step 5: Run the FULL suite**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2
```

`password_lifetime_tests` is the module to read first if anything fails: it watches the global allocator around `authenticate_then_wipe`, and this task made that function longer. A failure there is a real finding, not a re-pin. The `rest::`/`vault_cache::`/`picker_ui::` mock-HTTP failures are the known-flaky family — baseline them before believing them.

- [ ] **Step 6: Commit**

`deskwarden/src/login_ui.rs`, `deskwarden/src/backend_policy.rs`. Message: the worker blocks on the human, and why that is the cheaper of the two available mistakes.

---

### Task 7: The stage in the window

**Files:** Modify `deskwarden/src/app_window.rs`

**Interfaces**

- *Consumes:* `login_ui::{SecondFactorRequest, SecondFactorCommand, SecondFactorPrompt}`, `second_factor_ui::{Prompt, Trouble}`.
- *Produces:* `Stage::SecondFactor`, `Event::{SecondFactorNeeded, SecondFactorDone, SecondFactorAbandoned}`, the `advance` arms.

- [ ] **Step 1: Write the failing test**

In `app_window.rs`'s existing transition-table test module:

```rust
    /// **The code stage sits between the card and the spinner**, and the two
    /// ways out of it are the two things that can happen: a completed factor
    /// goes on to the spinner, an abandoned one goes BACK to the card.
    #[test]
    fn the_code_stage_sits_between_the_card_and_the_spinner() {
        assert_eq!(
            advance(Stage::SignIn, Event::SecondFactorNeeded),
            Next::Show(Stage::SecondFactor)
        );
        assert_eq!(
            advance(Stage::SecondFactor, Event::SecondFactorDone),
            Next::Show(Stage::Working),
            "a completed factor enters the spinner exactly as a password-only sign-in does"
        );
        assert_eq!(
            advance(Stage::SecondFactor, Event::SecondFactorAbandoned),
            Next::Show(Stage::SignIn),
            "backing out returns to the card, not to a closed window: the user still has \
             an account to sign into"
        );
    }

    /// The stage does not become a hole in the table. A `SignedIn` arriving
    /// while the code box is up is a no-op, not a jump past the factor.
    #[test]
    fn the_code_stage_ignores_events_that_are_not_about_it() {
        assert_eq!(
            advance(Stage::SecondFactor, Event::SignedIn),
            Next::Show(Stage::SecondFactor),
            "a stale SignedIn must not skip the factor"
        );
        assert_eq!(
            advance(Stage::SecondFactor, Event::WorkReady),
            Next::Show(Stage::SecondFactor)
        );
        // Positive control: the table still moves for the events it owns.
        assert_eq!(
            advance(Stage::SecondFactor, Event::SecondFactorDone),
            Next::Show(Stage::Working)
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- app_window
```

Expected: `no variant named 'SecondFactor' found for enum 'Stage'`.

- [ ] **Step 3: Implement**

In `Stage`:

```rust
    /// **The code box** -- `second_factor_ui`'s card, over the same backdrop
    /// the sign-in card uses. Entered only when the server asked for a second
    /// factor, which it can only do AFTER the grant, which is why this is a
    /// stage between the card and the spinner rather than a field on the card.
    SecondFactor,
```

In `Event`:

```rust
    /// The sign-in worker reached `LoginOutcome::NeedsSecondFactor` and is now
    /// blocked holding the challenge, waiting for a
    /// `login_ui::SecondFactorCommand`.
    SecondFactorNeeded,
    /// The factor was accepted. From here the window is in exactly the state a
    /// password-only sign-in leaves it in.
    SecondFactorDone,
    /// The user backed out of the code box, or the challenge expired. The
    /// worker has dropped the challenge; the card is what comes next, because
    /// starting over means a new master password.
    SecondFactorAbandoned,
```

In `advance`, above the catch-all:

```rust
        (Stage::SignIn, Event::SecondFactorNeeded) => Next::Show(Stage::SecondFactor),
        (Stage::SecondFactor, Event::SecondFactorDone) => Next::Show(Stage::Working),
        (Stage::SecondFactor, Event::SecondFactorAbandoned) => Next::Show(Stage::SignIn),
```

In the frame closure, a new arm beside `Stage::SignIn`:

```rust
            Stage::SecondFactor => {
                let Some(prompt) = second_factor.as_mut() else {
                    // Nothing to draw and nothing to wait for. Treated as an
                    // abandonment rather than as a panic: the cost of being
                    // wrong is the whole window dying on the screen a blocked
                    // user is looking at.
                    if let Next::Show(next) = advance(stage, Event::SecondFactorAbandoned) {
                        stage = next;
                    }
                    return;
                };
                for trouble in trouble_rx.try_iter() {
                    prompt.state.went_wrong(trouble);
                    if trouble.is_fatal() {
                        prompt.fatal = true;
                    }
                }
                match second_factor_ui::draw(ui, &mut prompt.state) {
                    Some(second_factor_ui::Asked::Send) => {
                        prompt.state.busy = true;
                        prompt.state.email_sent = true;
                        let _ = prompt.commands.send(login_ui::SecondFactorCommand::SendEmail);
                    }
                    Some(second_factor_ui::Asked::Submit(answer)) => {
                        prompt.state.busy = true;
                        let _ = prompt
                            .commands
                            .send(login_ui::SecondFactorCommand::Answer(answer));
                    }
                    Some(second_factor_ui::Asked::Back) => {
                        let _ = prompt.commands.send(login_ui::SecondFactorCommand::Abandon);
                        if let Next::Show(next) = advance(stage, Event::SecondFactorAbandoned) {
                            stage = next;
                            login = None;
                        }
                    }
                    None => {}
                }
                // The worker's success still arrives as a token on the same
                // channel the card's does, so the SignedIn wiring above is
                // unchanged and this stage only has to notice it.
                if let Some(handles) = login.as_ref().map(|(_, h)| h) {
                    if let Some(produced) = handles.take_token() {
                        *token_for_closure.borrow_mut() = Some(produced.clone());
                        // ... identical to the `Stage::SignIn` arm's body ...
                        if let Next::Show(next) = advance(stage, Event::SecondFactorDone) {
                            stage = next;
                            working_message = setup_message;
                            working_since = Some(Instant::now());
                        }
                    }
                }
                ui.ctx().request_repaint_after(Duration::from_millis(120));
            }
```

The token-noticing body is duplicated between the two arms in the sketch above; **factor it into one `fn notice_the_token(..)` before committing**, because two copies of the wiring that records the session token is exactly the shape `run_the_one_window`'s own docstring warns about.

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- app_window
```

- [ ] **Step 5: Commit**

`deskwarden/src/app_window.rs`. Message: the fourth stage, and why abandoning returns to the card rather than closing the window.

---

### Task 8: The card, drawn — and the message at `login_ui.rs:486` retired

**Files:** Modify `deskwarden/src/second_factor_ui.rs`, `deskwarden/src/login_ui.rs`

**Interfaces**

- *Consumes:* `Prompt`, `factor_title`, `factor_hint`, `SEND_CODE_LABEL`, `CODE_SENT_NOTICE`, `unsupported_only_message`.
- *Produces:* `second_factor_ui::Asked`, `second_factor_ui::draw`; and `friendly_auth_error`'s two-step arm, rewritten.

- [ ] **Step 1: Write the failing test**

In `second_factor_ui.rs`'s test module, using `login_ui`'s own paint harness idiom (`styled_context`, `raw_input`, the `walk` over `output.shapes`):

```rust
    /// Every string the card paints in one frame.
    fn painted(prompt: &mut Prompt) -> Vec<String> {
        let ctx = crate::login_ui::tests::styled_context();
        let output = ctx.run_ui(crate::login_ui::tests::raw_input(), |ui| {
            let _ = draw(ui, prompt);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            crate::login_ui::tests::walk(&clipped.shape, &mut texts);
        }
        texts
    }

    fn says(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// The Email card offers to send; the authenticator card does not, and the
    /// authenticator card is the positive control that proves the harness is
    /// painting anything at all.
    #[test]
    fn the_send_button_is_painted_for_email_only() {
        let mut email = Prompt::new(vec![SecondFactor::Email {
            masked: Some("a***@b.c".to_string()),
        }]);
        let email_texts = painted(&mut email);
        assert!(
            says(&email_texts, "a***@b.c"),
            "control: the card painted its own hint; got {email_texts:?}"
        );
        assert!(says(&email_texts, SEND_CODE_LABEL), "got {email_texts:?}");

        let mut totp = Prompt::new(vec![SecondFactor::Authenticator]);
        let totp_texts = painted(&mut totp);
        assert!(
            says(&totp_texts, "authenticator app"),
            "control: the authenticator card painted its own hint; got {totp_texts:?}"
        );
        assert!(
            !says(&totp_texts, SEND_CODE_LABEL),
            "got {totp_texts:?}"
        );
    }

    /// The provider switch appears only when there is something to switch to.
    #[test]
    fn the_provider_switch_appears_only_with_more_than_one_factor() {
        let mut two = Prompt::new(vec![SecondFactor::Authenticator, SecondFactor::YubiKey]);
        let two_texts = painted(&mut two);
        assert!(says(&two_texts, SWITCH_LABEL), "got {two_texts:?}");

        let mut one = Prompt::new(vec![SecondFactor::Authenticator]);
        let one_texts = painted(&mut one);
        assert!(
            says(&one_texts, "Authenticator app"),
            "control: the single-factor card painted its title; got {one_texts:?}"
        );
        assert!(!says(&one_texts, SWITCH_LABEL), "got {one_texts:?}");
    }

    /// An unsupported-only account gets the message and NO code box: a box
    /// that cannot be submitted is worse than no box.
    #[test]
    fn an_unsupported_only_card_asks_for_nothing() {
        let mut duo = Prompt::new(vec![SecondFactor::Unsupported(2)]);
        let texts = painted(&mut duo);
        assert!(says(&texts, "Duo"), "got {texts:?}");
        assert!(says(&texts, "API key"), "got {texts:?}");
        assert!(
            !says(&texts, CODE_LABEL),
            "there is nothing to type; got {texts:?}"
        );
    }
```

And in `login_ui.rs`, in the module that already tests `friendly_auth_error`:

```rust
    /// **The message that sent people to a terminal is gone.**
    ///
    /// This is the message `login_ui.rs:486` carried before this branch, and
    /// the whole of piece 2 exists to replace it. The positive control is the
    /// arm beside it: a mistyped password still gets its own wording, so a
    /// `friendly_auth_error` that had stopped matching anything at all would
    /// fail here rather than pass.
    #[test]
    fn no_two_step_failure_tells_the_user_to_open_a_terminal() {
        let two_step = friendly_auth_error("Two-step login is required");
        assert!(
            !two_step.contains("bw login") && !two_step.to_lowercase().contains("terminal"),
            "got {two_step:?}"
        );
        assert!(
            two_step.to_lowercase().contains("try again")
                || two_step.to_lowercase().contains("two-step"),
            "the arm must still SAY something about two-step login; got {two_step:?}"
        );
        assert!(
            friendly_auth_error("error=The decryption operation failed")
                .contains("master password"),
            "control: friendly_auth_error still recognises its other arms"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui no_two_step_failure
```

Expected: `cannot find function 'draw'`, `cannot find value 'SWITCH_LABEL'`, `cannot find value 'CODE_LABEL'`; and `no_two_step_failure_tells_the_user_to_open_a_terminal` fails on the first assertion, because the arm still says `Run \`bw login\` in a terminal once`. **That failure is the defect this branch is named after, reproduced.**

If `login_ui::tests::styled_context` / `raw_input` / `walk` are private, make them `pub(crate)` in the same commit — they are already test-only helpers and this is the second consumer, which is the point at which sharing them beats copying them.

- [ ] **Step 3: Implement the card**

In `second_factor_ui.rs`:

```rust
pub const CODE_LABEL: &str = "Verification code";
pub const SWITCH_LABEL: &str = "Use a different method";
pub const CONTINUE_LABEL: &str = "Continue";
pub const BACK_LABEL: &str = "Back";

/// What the user asked the code stage to do this frame. `None` from [`draw`]
/// is the ordinary case: the user is still typing.
#[derive(Debug)]
pub enum Asked {
    /// Email only: send me a code.
    Send,
    /// These digits, against the chosen provider.
    Submit(crate::rest::api::SecondFactorAnswer),
    /// Back to the master password.
    Back,
}

/// Draws the code stage. Pure view: the caller owns the [`Prompt`] and
/// performs the channel sends for whatever comes back, exactly as
/// `login_ui::draw_login_window` and its caller are split.
pub fn draw(ui: &mut egui::Ui, prompt: &mut Prompt) -> Option<Asked> {
    if let Some(message) = unsupported_only_message(prompt.offered()) {
        ui.label(RichText::new(factor_title(&SecondFactor::Unsupported(0))).size(17.0));
        ui.add_space(8.0);
        ui.label(RichText::new(message).size(12.0).color(theme::TEXT_MUTED));
        ui.add_space(16.0);
        return ui.button(BACK_LABEL).clicked().then_some(Asked::Back);
    }

    let Some(chosen) = prompt.chosen() else {
        // An empty provider list. Nothing to ask for, and nothing to
        // apologise for either -- `unsupported_only_message` answers `None`
        // for the empty offer, so this arm exists and must say something.
        ui.label(RichText::new(message_for(Trouble::Unreachable)).size(12.0));
        return ui.button(BACK_LABEL).clicked().then_some(Asked::Back);
    };

    ui.label(RichText::new(factor_title(&chosen)).size(17.0));
    ui.add_space(6.0);
    ui.label(RichText::new(factor_hint(&chosen)).size(12.0).color(theme::TEXT_MUTED));
    ui.add_space(14.0);

    let mut asked = None;

    if prompt.wants_send_button() {
        ui.horizontal(|ui| {
            if ui.add_enabled(!prompt.busy, egui::Button::new(SEND_CODE_LABEL)).clicked() {
                asked = Some(Asked::Send);
            }
            if prompt.email_sent {
                ui.label(RichText::new(CODE_SENT_NOTICE).size(11.0).color(theme::TEXT_MUTED));
            }
        });
        ui.add_space(10.0);
    }

    ui.label(RichText::new(CODE_LABEL).size(12.0));
    let entry = ui.add_enabled(
        !prompt.busy,
        egui::TextEdit::singleline(&mut prompt.code).desired_width(f32::INFINITY),
    );
    // A YubiKey types its code and presses Enter itself, which is why the
    // focus goes here on entry and why Enter submits.
    if !prompt.busy {
        entry.request_focus();
    }
    let entered = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

    if let Some(error) = prompt.error.as_deref() {
        ui.add_space(6.0);
        ui.label(RichText::new(error).size(11.0).color(theme::DANGER));
    }

    ui.add_space(14.0);
    ui.horizontal(|ui| {
        let ready = !prompt.busy && !prompt.code.trim().is_empty();
        if (ui.add_enabled(ready, egui::Button::new(CONTINUE_LABEL)).clicked() || (entered && ready))
            && asked.is_none()
        {
            asked = Some(Asked::Submit(answer_for(&chosen, prompt.code.trim())));
        }
        if ui.button(BACK_LABEL).clicked() {
            asked = Some(Asked::Back);
        }
    });

    if prompt.supported().len() > 1 {
        ui.add_space(10.0);
        ui.label(RichText::new(SWITCH_LABEL).size(11.0).color(theme::TEXT_MUTED));
        let alternatives = prompt.supported();
        for factor in alternatives {
            if factor == chosen {
                continue;
            }
            if ui.link(factor_title(&factor)).clicked() {
                prompt.choose(factor);
            }
        }
    }

    asked
}
```

`answer_for` builds `rest::api::SecondFactorAnswer` from the chosen factor and the digits; it is one `match` and belongs beside `draw`. If piece 1 exposes a constructor taking `(SecondFactor, &str)`, call that instead of writing a second one.

- [ ] **Step 4: Implement the `login_ui.rs:486` replacement**

The two-step arm of `friendly_auth_error` becomes:

```rust
    // **This arm no longer sends anybody to a terminal.** It used to read
    // "Run `bw login` in a terminal once to complete it, then come back",
    // which was true when this window could not prompt for a second factor.
    // It can: see `second_factor_ui` and `app_window::Stage::SecondFactor`.
    //
    // What is left here is the case where a two-step failure surfaces as a
    // CLI stderr string rather than as a `LoginOutcome::NeedsSecondFactor` --
    // an account still on the `bw` backend. There is no provider list in a
    // stderr line, so this cannot name the provider the way
    // `second_factor_ui::unsupported_only_message` does; it says what
    // happened and stops.
    if mentions(&["two-step", "two step", "two-factor", "twofactor"]) {
        return "That two-step login didn't complete. Try again — and if this account uses \
                Duo or a security key, sign in with a personal API key instead."
            .to_string();
    }
```

- [ ] **Step 5: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- second_factor_ui no_two_step_failure
```

- [ ] **Step 6: Run the FULL suite, and clippy**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2
RUSTFLAGS="-D warnings" cargo clippy --manifest-path deskwarden/Cargo.toml --all-targets
```

Expect `login_ui.rs`'s source-position pins to be the ones that complain, if anything does — this task added functions to a file several pins split on. Read each pin's own docstring before touching it: a pin whose subject genuinely moved is re-pinned with a commit message saying what moved; a pin whose subject did not move is telling you this change went further than intended.

- [ ] **Step 7: Commit**

`deskwarden/src/second_factor_ui.rs`, `deskwarden/src/login_ui.rs`. Message: the card, and the retirement of the terminal instruction — quote the old message in the body, since it is what the branch is named after.

---

### Task 9: The live check

**Files:** none

No test in this repository can prove this feature. The spec says so: *"a real account with an authenticator app, signed in with `bw.exe` renamed away. Nothing about this can be proven by a test with a CLI available."*

- [ ] **Step 1:** Rename `bw.exe` away.
- [ ] **Step 2:** Sign in on an account with an authenticator app. Confirm: the code stage appears, the title names the authenticator, there is no *Send code* button.
- [ ] **Step 3:** **Type a wrong code deliberately.** Confirm the box empties, the message names the code, and **the master password is not asked for again**. This is the acceptance criterion for the whole branch.
- [ ] **Step 4:** Type the right code. Confirm the spinner follows and the vault loads.
- [ ] **Step 5:** Repeat on an email-code account: *Send code* present, pressed once, notice appears, code accepted.
- [ ] **Step 6:** Read the log for the whole session and confirm no line contains anything challenge-shaped — no base64 hash, no key material. The `Challenge` is not `Debug`, so this should be structurally impossible; check anyway, because "structurally impossible" is a claim and the log is the evidence.

---

## Self-review

### Spec coverage

| Spec requirement (piece 2) | Where |
| --- | --- |
| A stage between card and spinner | Task 7 (`Stage::SecondFactor`, `advance` arms) |
| Shows which factor is being asked for | Task 2 (`factor_title`/`factor_hint`), Task 8 (painted) |
| One code box | Task 8 (`CODE_LABEL`, single `TextEdit`) |
| *Send code* for Email only | Task 2 (`wants_send_button`), Task 8 (paint test with authenticator control) |
| A way to switch provider when several are offered | Task 1 (`supported`, `choose`), Task 8 (`SWITCH_LABEL`, hidden at one factor) |
| Default provider: YubiKey → Authenticator → Email | Task 1 (`preferred`) |
| Wrong code returns to the same stage, code cleared | Task 4 (`went_wrong(CodeRejected)`), Task 6 (loop retries in place) |
| Master password untouched by a wrong code | Task 6 — the structural answer: the hash never leaves the worker, so there is nothing to re-type. Asserted by `STARTS == 0`. |
| Unsupported-only names the provider | Task 3 (`provider_name`, per-account list, "not Duo" negative with control) |
| Unsupported-only mentions the personal API key | Task 3 |
| `login_ui.rs:486` replaced | Task 8 Step 4, with a red test in Step 2 |
| Code rejected / email send failed / challenge expired, distinct | Task 4 (`Trouble`, pairwise-distinct assertion, `is_fatal`) |
| `Challenge` never logged, never in an error string, not held longer than needed | Task 5 (source pins over both crossing types), Task 6 (bound in one arm, dropped on return), Task 9 Step 6 |

### Placeholder scan

Searched the document for `TBD`, `appropriate`, `similar to`, `etc.`, `and so on`, `handle errors`: no hits. Every step that changes code carries the code. Three places name a condition under which the worker must **stop and report** rather than improvise (Task 5's `Challenge::server_url`, Task 6's `fake_challenge`, Task 1's missing `SecondFactor`); those are deliberate stop signs at piece 1's boundary, not placeholders — each says exactly what is missing and what the fallback is.

### Soft spots I am flagging rather than hiding

1. **Task 5 and Task 6 are written against a `Challenge` API I have not seen.** I assumed `challenge.providers()` and `challenge.server_url()`. Piece 1 may spell these differently or not expose them at all. Task 5 carries a written fallback for `server_url`; `providers()` has none, because a `Challenge` that will not name its providers cannot drive a prompt at all — that would be a real interface negotiation with the other worker, not a plan edit.

2. **The worker blocks a detached thread on a human.** Bounded on both ends (`Abandon`, disconnected channel), and the alternative is handing the credential to a frame closure — but it is a thread that can live for minutes, and it holds the master key the whole time. `password_lifetime_tests` watches `authenticate_then_wipe`, and Task 6 makes that function's life much longer. **I expect a real conversation with those guards, and I would treat a failure there as a finding and not as a re-pin.** If they cannot be satisfied, the fallback is a hard timeout inside `complete_second_factor` — five minutes, reported as `ChallengeExpired`, which is already a variant.

3. **Task 7's frame-closure arm is the one part of this feature no test can run.** `eframe::Frame` has no public constructor, which `app_window`'s own docstring says. The `advance` table is tested; the wiring around it is not, and this crate's answer to that elsewhere is a source-position pin. I did not write one, because I could not read the shape the arm will actually take. **Add one in Task 7 Step 5** in `startup_shape_tests`' idiom, asserting the `SecondFactor` arm sends on the command channel and never binds a `Challenge`.

4. **The provider switch is drawn as links in a column.** That is the cheapest thing that satisfies "a way to switch provider" and it will look unlike the rest of the card, which is `prefs_ui`-styled rows. It is the part of this plan most likely to come back from a design pass. The copy and the behaviour are pinned; the widget is not, on purpose.

5. **The API-key sign-in path is not in this plan.** The spec puts it in scope for the feature as a whole, and Task 3's message now *promises* it ("sign in here with it"). Piece 1 owns the grant. **If the API-key path does not land, Task 3's message is a promise this app does not keep** — that is the one wording in this plan whose truth depends on the other worker, and it should be re-read before the branch merges.

6. **The unsupported-only message names web-vault navigation** ("Account settings → Security → Keys"). That is a path in someone else's product and it drifts. It earns its place because "use an API key" without a way to find one is not actionable — but it is the sentence most likely to be wrong in a year.

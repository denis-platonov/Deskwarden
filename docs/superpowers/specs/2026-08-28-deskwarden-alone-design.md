# Deskwarden Alone

**Running with no `bw.exe` on the machine at all.**

## Why

The comparison table in the README is the argument:

| | Deskwarden | Bitwarden Desktop |
| --- | --- | --- |
| App itself | ~16 MB | 456 MB |
| Full install (app + bundled `bw` CLI) | ~169 MB | 456 MB |

Two of this project's stated reasons for existing are **size** and **not being
Electron**. A 16 MB native app that ships a ~150 MB Node CLI beside itself
answers the second and undercuts the first. And on the configuration this
project is actually built for — a self-hosted server, talked to directly —
the CLI is dead weight: `rest::backend` already does every vault operation,
with a test asserting that none of them refuses.

So the goal is a third row that says **~16 MB, and nothing else**.

## Correction: this document is a supplement, not the plan

**Written 2026-08-28 without checking for prior work, and there was prior
work.** `2026-08-26-dropping-the-bw-cli-design.md` already sets out the whole
subtraction in order, and it names a blocker this document missed entirely:

> **2. Two-factor authentication. This is the blocker.** `rest::api`
> *recognises* 2FA and refuses it by name (`RestError::TwoFactorRequired`)
> rather than completing it. Every account with 2FA enabled -- which is the
> configuration Bitwarden encourages -- cannot sign in without the CLI.

Still true, checked today: `twoFactorProvider`/`twoFactorToken` appear in that
module only inside a comment saying completing the challenge "is not in this
task". So the three CLI call sites this document measures are **necessary and
not sufficient** -- fixing all three leaves any 2FA account unable to sign in,
which for most users is the same as not shipping it.

**The order to follow is the other document's.** What is useful here is the
measurement it does not have: exactly where the CLI is still invoked, which
is its step 3, and the finding that the session token is unused on the
direct-REST path so it needs not producing rather than replacing.

## What stands in the way, measured rather than assumed

Three places require `bw.exe` today, and the third was found only by reading
the startup path rather than reasoning about it.

### 1. Startup refuses without it

`main.rs`, before `settings.json` is read:

```rust
if !bw_exe.exists() {
    fatal_startup_error("Deskwarden needs the Bitwarden CLI (bw.exe) …");
}
check_bw_signature(&bw_exe);
```

Unconditional. A direct-REST account that will never spawn the CLI still
cannot start without it.

### 2. Signing in runs the CLI first, and direct REST is a bolt-on

```rust
let token = run_cli(&password);
if token.is_ok() {
    enroll(&password);
    if let Some(direct) = direct { …derive direct-REST… }
}
```

The CLI is not merely *involved* on the direct path — it is the gate. The
direct-REST derivation only happens **if the CLI sign-in succeeded**, and the
CLI's session token is what the function returns.

### 3. Every launch verifies the cached session by running the CLI

```rust
let cached_session = match store.load() {
    Some(token) => match login_ui::check_bw_status_with_session(Some(&token)) { … }
```

`check_bw_status_with_session` shells out to `bw status`. This runs on every
start, on both backends.

## What does NOT stand in the way

**The session token itself.** Every consumer — `start_backend`,
`try_start_backend`, `run_bw_sync` — is a `bw`-path call that already
short-circuits on `BackendStart::NotSelected` when direct REST is selected. So
on that path the token is not used for anything. It does not need replacing;
it needs not producing.

This was expected to be the hard part and is not, which is the reason to
measure before estimating.

## The shape

**One question, asked once, early: which backend serves this account.**
`backend_policy::choose(server_url, use_official_bw_crypto)` is already a pure
function of two values that live in `settings.json`. Today the answer is
reached after the CLI has been demanded. Moving that read above the gate is
most of the change.

Then each of the three sites becomes conditional on it:

1. **Startup** requires `bw.exe` only on the `BwServe` arm. On `DirectRest` a
   missing CLI is not an error and not a warning — it is the expected state.
   **The signature check stays exactly as strict where it applies**: it is a
   supply-chain control, and "we no longer need `bw`" must not become "we no
   longer check `bw`".
2. **Sign-in** derives direct-REST first and runs the CLI only on the
   `BwServe` arm.
3. **The cached-session check** asks the vault the account actually uses. On
   `DirectRest` that is whether `user_key_store` holds a key that still works
   — which `main` already knows how to answer, because it does exactly that
   when it builds the backend.

## The part to be careful with

Site 2 is inside the function whose doc calls it *"the plaintext master
password's whole life"*, and it carries allocator-watching guards that measure
the password never escapes. The control-flow change is small. The review is
not, and the failure mode is unrecoverable.

Requirements for that site specifically:

- The password stays in `Zeroizing` and the unwind paths stay covered. Adding
  a branch adds a way out of the function, and the existing guard exists
  because that has been got wrong before.
- The allocator guards run on **every** step, not at the end.
- `enroll` (Windows Hello) must keep running on both arms. It is not a `bw`
  thing and must not be lost to a reshuffle.
- A failure to derive direct-REST on the `DirectRest` arm is a **failed
  sign-in**, not a warning. Today it is logged and swallowed, correctly,
  because the CLI had already succeeded and the user was signed in. With no
  CLI there is nothing else that succeeded.

## What this is not

- **Not removing the `bw` backend.** `VaultBackendChoice::BwServe` stays, it
  stays the default, and everything about it keeps working. This is about not
  *requiring* the CLI when it is not the backend.
- **Not a new sign-in flow.** `derive_direct_rest` already exists and is
  already exercised on this path today.
- **Not the two-binaries split**, which is separate and still gated on the
  updater swapping a set atomically.

## Packaging

The installer downloads and verifies a signed `bw.exe`. A self-contained
install skips that. This is the smallest piece of the work and the one with
the clearest test: install, delete nothing, and confirm the app starts with no
CLI present.

**Open question for the implementation plan:** whether that is a second
installer, or one installer with a task the user deselects — the same
mechanism `/MERGETASKS=!autostart` already uses. The second is less to
maintain and less to explain.

## How it will be known to work

The claim is "no `bw.exe` on the machine". So the check is exactly that:
rename `bw.exe`, start the app, sign in, read an item, fill a credential.
Nothing about this can be verified by a test that has a CLI available, which
means the acceptance step is manual and must be done on a machine where the
file is genuinely absent.

## Status

Design. No plan, no code. The auth-path work is the gate: it should be planned
on its own, with the allocator guards named as a step rather than assumed.

# Two-Factor Without the CLI

**Completing a second factor in `rest::api`, so that dropping `bw.exe` locks
nobody out.**

## Why this one first

`2026-08-26-dropping-the-bw-cli-design.md` names this the blocker, and it is
still true: `rest::api` *recognises* a second factor
(`RestError::TwoFactorRequired { providers }`) and refuses it by name rather
than completing it. Its own module doc says so -- "Completing the second
factor means resending the grant with `twoFactorProvider`/`twoFactorToken`,
and that is not in this task."

**And it is not only a `bw` problem.** Today `login_ui.rs:486` answers a
two-step account with:

> "This account uses two-step login, which Deskwarden can't prompt for. Run
> `bw login` in a terminal once to complete it, then come back."

So a 2FA user has a bad time on *both* backends right now. This work fixes a
live hole and removes the blocker in the same change.

## What parity actually means, measured against the CLI

Bitwarden supports five second factors: FIDO2 WebAuthn, authenticator app and
email (free), plus Duo and YubiKey OTP (premium).

**The CLI supports three of them.** Its documentation is explicit that Duo and
WebAuthn/FIDO2 "are not compatible with CLI login", and directs those users to
`bw login --apikey` instead.

That settles the scope, and it settles it more cheaply than expected:

| Provider | Number | In the CLI? | Here |
| --- | --- | --- | --- |
| Authenticator (TOTP) | 0 | yes | **yes** |
| Email | 1 | yes | **yes** |
| YubiKey OTP | 3 | yes | **yes** |
| Duo | 2 | no | no |
| OrganizationDuo | 6 | no | no |
| WebAuthn / FIDO2 | 7 | no | no |
| Personal API key | -- | `--apikey` | **yes** |

A Duo or WebAuthn user cannot sign in through `bw login` today either. What
they *can* do is use a personal API key, so that path is in scope: without it,
dropping the CLI would be a real regression for them, and with it there is no
account `bw` can sign in that Deskwarden cannot.

`U2f` (4), `Remember` (5) and `RecoveryCode` (8) complete the enum and are
not providers a user picks at this prompt.

## The shape

### The seam is already in the right place

`api::authenticate` (`rest/api.rs:531`) is prelogin -> `master_key` ->
`password_hash` -> `password_grant`, and `password_grant`
(`rest/api.rs:548`) already "takes a hash somebody else derived". That split
is what makes this affordable: **the retry reuses the same hash** rather than
paying PBKDF2's six hundred thousand iterations a second time.

### One new outcome, not a new function

```rust
pub enum LoginOutcome {
    Done(Authenticated),
    NeedsSecondFactor(Challenge),
}

pub fn authenticate(&self, email, password, device) -> Result<LoginOutcome, RestError>
pub fn finish_second_factor(&self, challenge: &Challenge, answer: &SecondFactorAnswer)
    -> Result<Authenticated, RestError>
```

`RestError::TwoFactorRequired` stays, because it is still the right answer for
a caller that cannot prompt -- the vault service, say -- but the sign-in path
now gets a `Challenge` it can act on.

### `Challenge` is the delicate part

It carries the parsed providers **and the material needed to resume**: the
email, the derived password hash, and the `MasterKey`. That is a
password-equivalent credential living for as long as somebody takes to read a
code off their phone.

It gets the treatment this crate already gives such things:

- **never `Debug`**, like `service_token::Token` and `service_keys::KeyRecord`;
- the hash in `Zeroizing`, as `master_key` already requires;
- the allocator guards that already watch the sign-in path extended over it --
  the function whose doc calls itself *"the plaintext master password's whole
  life"* now has a longer life to account for, and that is the review this
  change actually needs.

There is no way around holding it. The server only reveals that a second
factor is wanted *after* the grant is attempted, so either the credential
survives the prompt or the user types their master password twice.

### Providers, parsed rather than stringly

The server sends `["0","1","3"]`; `TwoFactorProviders2` carries per-provider
detail (the module's own test fixture shows `{"1":{"Email":"a***@b.c"}}`,
which is the masked address to show the user).

```rust
pub enum SecondFactor { Authenticator, Email { masked: Option<String> }, YubiKey, Unsupported(u8) }
```

`Unsupported` is **not an error**. An account with WebAuthn *and* an
authenticator app must still be offered the authenticator, and an account with
only unsupported providers needs to be told which ones they are -- so the
error message can name Duo instead of saying "two-step login" and leaving the
user to guess.

**Default choice, when several are offered:** Bitwarden's own priority order,
restricted to what is supported -- YubiKey, then Authenticator, then Email.
The user can pick another.

### Email is the one that needs a second call

`POST /api/two-factor/send-email-login`, carrying the email, the master
password hash and the device identifier, before the user has anything to type.
The other two providers need no round trip: the code is already on the user's
phone or key.

This is also the one that can fail *before* the prompt, and its failure has to
read as "we could not send you a code", not as a rejected code.

### The API key is a different grant, and it does not replace the password

`grant_type=client_credentials` with `client_id`/`client_secret` and the same
three device fields. It authenticates the session -- and that is all it does.

**The vault key still comes from the master password**, through the same
prelogin and `master_key`, which is exactly why `bw login --apikey` must be
followed by `bw unlock`. So this path is: API key -> session, master password
-> key, both of them, always. A design that treated the API key as a way to
skip the password would produce a signed-in app that could not decrypt
anything.

### The prompt

A stage in the sign-in flow the app already has: card -> **code** -> spinner ->
vault. It shows which factor is being asked for, a single code box, a *Send
code* action for Email, and a way to switch provider when the account has
several.

A wrong code returns to the same stage with the code cleared and the password
untouched -- re-typing a master password because a digit was fat-fingered is
the behaviour this replaces, not one to reproduce.

## What is deliberately not here

**"Remember this device" (`twoFactorRemember`).** The server would return a
bypass token to store and replay as provider 5. Sign-in is already infrequent
-- the session is cached and Windows Hello unlocks it -- so the UX it buys is
small, measured against storing a credential whose only purpose is to skip a
second factor. It is a clean addition later if the frequency turns out to
annoy.

**Duo and WebAuthn.** Neither is reachable from `bw login` today, so omitting
them is not a regression, and each needs machinery -- an embedded browser
flow, or native platform WebAuthn -- comparable to the whole of this work.

**Recovery codes.** Not a provider at this prompt; Bitwarden uses them through
account recovery, which is a different endpoint and a different design.

## How it will be known to work

**Pure tests**: provider parsing from both `TwoFactorProviders` and
`TwoFactorProviders2`, the priority choice among several, the `Unsupported`
arm carrying its number through.

**HTTP tests, in `mockito`**, following
`the_password_grant_sends_every_field_the_server_requires`: that the retry
carries `twoFactorProvider` and `twoFactorToken` **and every field the first
grant sent**; that the email-send call carries its three; that the API-key
grant sends `client_credentials` and no password.

**A secret-hygiene test**: `Challenge` is not `Debug`, and the hash it holds
is zeroized on drop.

**A live check, which is the only one that settles it**: a real account with
an authenticator app, signed in with `bw.exe` renamed away. Nothing about
this can be proven by a test with a CLI available.

## Sequencing

Two pieces, specified together and built separately, because the risk in them
is different in kind:

1. **`rest::api`** -- the outcome, the parsing, the three grants. Pure and
   `mockito`-testable, no UI, and where a mistake is a security mistake.
2. **The sign-in stage** -- the prompt, the provider switch, the wording. Where
   a mistake is a usability mistake.

## Status

Design, approved 2026-08-29. Sources for the provider numbers and the CLI's
supported set are Bitwarden's own documentation, cited in the commit that adds
this file.

# Signing In With an API Key

**The third piece of two-factor parity: a way in for the accounts whose
second factor Deskwarden cannot complete.**

## Why

`2026-08-29-two-factor-without-the-cli-design.md` established the scope by
measuring against the CLI: Bitwarden supports five second factors, `bw login`
supports three, and the other two -- Duo and WebAuthn/FIDO2 -- are answered by
`bw login --apikey` instead. That document put the API key in scope for the
feature as a whole, and piece 1 built the grant.

**Nothing calls it.** `RestClient::api_key_grant` has no caller outside
`rest/api.rs`, and the plan for piece 2 mentions `client_id` and
`client_secret` zero times. Piece 2's own self-review flagged the
consequence:

> "If the API-key path does not land, Task 3's message is a promise this app
> does not keep."

So piece 2 ships a message telling a Duo user to sign in with a personal API
key, and there is nowhere in the app to type one. That is worse than today's
"run `bw login` in a terminal", which is at least followable. This piece is
what makes that message true.

## The shape: two stages, because they are two different things

**Stage 1 -- the key pair.** `client_id` and `client_secret`, exchanged for a
session through `api_key_grant` (`grant_type=client_credentials`).

**Stage 2 -- the master password.** Prelogin for the KDF parameters, then
`master_key`, which is what actually decrypts the vault.

**What rejects a wrong password, which this design first failed to say.**
`api_key_grant` never sees a password, so it cannot check one, and
`master_key` is a key derivation function -- it turns any bytes into a key
and returns `Err` only for unusable KDF *parameters* (`iterations == 0`).
Neither of the two calls above fails on a wrong password. Left there, stage 2
would always "succeed" and the user would reach a vault that decrypts to
nothing, which is the exact failure this document calls worse than a refusal
because it looks like success.

The rejection is `VaultKeys::unwrap_from` (`rest/sync.rs:487`), which
unwraps the account's protected user key with the derived key and answers
`CryptoError::MacMismatch` when the password was wrong. That is not new
machinery: it is the same check the ordinary password sign-in already makes,
and `rest/crypto.rs:1655` already tests it with a deliberately wrong
password. So stage 2 is prelogin, `master_key`, and **one sync**, and the
sync is the part that says yes or no.

**Both, always.** The API key authenticates and does not decrypt; that is
precisely why `bw login --apikey` must be followed by `bw unlock`. A design
that treated the key as a way to skip the password would produce an app that
is signed in and cannot read anything -- which is a worse failure than being
refused, because it looks like success.

Two stages rather than one screen with three fields, and the reason is
diagnosis: a single submit that can fail for three different reasons gives one
error message that has to hedge. Split, a rejected key pair and a rejected
password are different screens with different words, and the user knows which
of the two things they typed was wrong. It also keeps a long-lived credential
and the master password off the screen at the same moment.

## What is stored, and what is not

**The `client_secret` is not persisted.** What survives a restart is the same
thing that survives one after a password sign-in: the session token, in the
existing `SessionStore` under DPAPI. The key pair is used once, to mint that
session, and dropped.

This is a deliberate refusal. A stored `client_secret` is a permanent,
password-free login to the account -- it does not expire, it is not covered by
the second factor it exists to bypass, and it would sit on disk being exactly
the thing an attacker wants. The cost of not storing it is re-entry when the
session finally dies, which for these users is the same cost `bw login
--apikey` already charges them.

**The secret gets the master password's handling** while it is in memory:
`Zeroizing`, no `Debug`, never logged, never in an error string. It is a
credential of the same weight.

## Where it lives

A stage reached from the sign-in card, and from one other place: the
unsupported-only message piece 2 shows a Duo or WebAuthn account. That message
already tells the user where to create the key ("Account settings → Security →
Keys" in the web vault); this gives it somewhere to lead.

It is **not** a separate top-level surface and **not** a Preferences page. It
is a way of signing in, so it belongs where signing in happens.

## Error states, told apart

* **Key pair rejected** (401 from the grant): the id or the secret is wrong,
  or the key has been rotated in the web vault. Stage 1 again, both fields
  kept -- retyping a 64-character secret because the id had a typo is the
  behaviour this design exists to avoid.
* **Master password rejected**: the session is good and the key is not the
  problem. Stage 2 again; stage 1 is not repeated, because nothing about it
  failed.
* **Server unreachable**: distinct from both, and it must not read as a
  rejected credential -- the same distinction `RestError::CodeNotSent` makes
  for the email code in piece 1.

## What this is not

* **Not a second way to store credentials.** Nothing new goes on disk; the
  session token is the only artefact, in the store that already holds it.
* **Not a replacement for the code prompt.** An account with an authenticator
  app should use it; this is for accounts whose factor cannot be completed.
* **Not Duo or WebAuthn support.** Those remain unimplemented, exactly as they
  are in the CLI. This is the escape hatch, not the factor.

## How it will be known to work

**Pure tests**: the two-stage state machine -- which stage a failure returns
to, that a rejected password does not re-ask for the key pair, that a rejected
key pair keeps both fields.

**Wording tests**: the three errors are pairwise distinct, and the
unsupported-only message from piece 2 now leads somewhere.

**A source-reading test** that the `client_secret` is `Zeroizing`, carries no
`Debug`, and appears in no log or error string -- the same shape piece 1 uses
for `Challenge`.

**The grant itself needs no new HTTP test**: piece 1 already pins that
`api_key_grant` sends `client_credentials` and no password, with a body
recorder and a positive control.

**A live check, which is the only thing that settles it**: a real account with
a personal API key, signed in with `bw.exe` renamed away, reaching an unlocked
vault. As with the rest of this feature, no test with a CLI available proves
anything about a build without one.

## Status

Design, approved 2026-08-29. Piece 3 of three; piece 1 is built, piece 2 is in
progress.

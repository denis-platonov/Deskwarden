# Authenticating to the Local Vault Service

Supersedes Task 0 of `2026-08-27-the-local-vault-service.md`, which chose a
single process-wide bearer token. That was all-or-nothing and immortal: one
value, full access, no expiry, no way to revoke one consumer without breaking
the rest.

## Two ways in, both off by default

**A. The master password.** `POST /auth` takes the master password and
returns a short-lived session token. This is what every other app that guards
a vault does, and it is the path for an interactive tool the owner is sitting
in front of.

**B. Named API keys.** A screen in Preferences mints a key, gives it a name
and an expiry, and grants it a specific set of permissions. This is the path
for a script that runs unattended, and it is the one that makes the service
worth having: a backup script gets read on Logins and nothing else, forever
auditable by name.

**Both are disabled by default, and so is the service.** A default-on local
endpoint serving a decrypted vault would be a change to the security of every
existing install, made on the owner's behalf. The default is off, the
enabling is deliberate, and there is nothing to reach until it is.

## Keys are stored hashed. The plaintext is shown once.

A key file that grants access if read is a second copy of the vault's front
door. So the record stores `SHA-256(key)`, and the key itself is displayed at
creation and never again — the same contract every API-key screen the owner
has ever used already teaches.

**A fast hash, deliberately.** Argon2id and PBKDF2 exist in this crate to
defend *low-entropy* secrets, where an attacker guesses candidate passwords.
An API key is 256 bits of OS randomness; there are no candidates to guess, so
a slow KDF buys nothing and costs a stretch on every single request. This is
the one place in this project where the fast hash is the right answer, and it
is written down because it looks wrong next to `rest::crypto`.

Comparison stays constant time (`service_token::matches`), on the hashes.

## What a key may do

```
Scope   = (Subject, Access)
Subject = All | Category(ItemKind) | Item(id)      // ItemKind already exists
Access  = Read | Write
```

- **Default deny.** A key with an empty scope set can do nothing. An
  unrecognised subject in a stored record denies rather than widens — a
  forward-compatibility rule that fails safe when an older build reads a
  newer file.
- **Write implies nothing about read**, and neither is implied by the other.
  Two flags, no hierarchy to reason about.
- **Expiry is checked per request**, against the clock at that moment, not at
  load. A service running for a week must not honour a key that expired on
  the second day.

## The order a request is decided in

1. **Authenticate.** Wrong or missing credential ends here, before the path
   is read. (Already built — `service_api::decide`.)
2. **Check expiry.** An expired key is refused exactly as an unknown one is.
3. **Route.**
4. **Check scope**, against the route AND its subject — for
   `/object/item/{id}`, against that id.
5. Handle.

Step 4 is the new one, and it is where a per-item grant either works or is
decorative. It cannot live in the handler: a handler that checks its own
permissions is a handler someone will add without one.

## What this does not fix

The limit from `service_token`'s module doc stands, and scoping does not
remove it: **a program already running as the owner** can read the DPAPI-
wrapped key store, or drive the Preferences screen, or read the master
password out of the same places the app does. Scopes bound what a *key*
does; they do not bound what the owner's own session can do. They are for
containing a script and revoking it, not for defending against a machine
that is already compromised.

## Status

Design. The implementation plan is revised in
`2026-08-27-the-local-vault-service.md`; Tasks 1 and 2 as already built are
unaffected — a key's secret is still minted and compared by
`service_token`, and routing is still decided by `service_api`.

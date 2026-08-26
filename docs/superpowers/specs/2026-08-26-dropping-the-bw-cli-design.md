# Dropping the `bw` CLI, for the users who chose not to need it

**Status:** designed, not started. Sequenced follow-on to
`2026-08-24-direct-rest-backend`'s work, and **independent of**
`2026-08-26-two-binaries-design.md` -- that one is about GL contexts, this one
is about a Node subprocess, and neither needs the other.

## The idea, in the owner's words

> if user decides so it removes -- they can use original bw instead of our rest

The setting already exists and already works: *Use official bw for crypto*,
off, and this app talks to the server itself. What it does **not** yet do is
make `bw` unnecessary -- the CLI is still on disk, still required, and still
launched. This document is about closing that gap, and about the one piece
that genuinely blocks it.

The framing above is also the safety argument, and it is why this is
defensible rather than reckless: **it stays a choice.** Anyone who does not
want this app's cryptography, or who needs something the REST backend does not
do, turns the setting back on and gets the official client. Nothing here
removes that door.

## Where `bw` is still required, measured against the code

The vault itself is done. `rest::backend::RestBackend` refuses **none** of the
twenty `VaultBackend` operations -- the refusal list went six, then four, then
three, and is now zero, pinned by an inverted test
(`no_operation_this_backend_offers_refuses_any_more`) that drives every
operation which ever refused and asserts none answers `Unsupported`. Reads and
writes were verified against a real server on 2026-08-26: 1,683 ciphers
decrypted with no failures, and a created item survived `set_app_match`,
archive/unarchive, trash/restore and a hard delete with every field intact.

So the remaining dependencies are four, and only one of them is hard -- plus
one item which is not a dependency at all but belongs on the same list.

**1. Sign-in, status and sign-out.** `login_ui.rs` shells out to the CLI at
`bw_command_in` (lines 113, 344, 365) for `bw status`, the login itself and
`bw logout`. This is the dependency a user cannot avoid, because it is the
first thing they do.

`rest::api::RestClient::authenticate` already does the whole flow -- prelogin,
master key, password hash, OAuth password grant -- and it is proven against a
live server. **This is a wiring job, not missing capability.**

**2. Two-factor authentication. This is the blocker.** `rest::api` *recognises*
2FA and refuses it by name (`RestError::TwoFactorRequired { providers }`)
rather than completing it. Every account with 2FA enabled -- which is the
configuration Bitwarden encourages -- cannot sign in without the CLI. Shipping
a `bw`-free build before this is written would lock those users out entirely,
and a lockout at sign-in is not something a setting can rescue them from,
because the setting lives behind the sign-in.

**3. Sends.** `send.rs` shells out to the CLI (lines 1323, 1405). The REST
module has no Send support at all -- `rest/mod.rs` says so: "no folder write,
no Send, no attachment upload". Sends are a whole feature, not a call. See
step 7.

**4. Attachments.** Not decrypted by `sync` and not creatable, replaceable or
deletable by `write`. They *survive* an edit -- they ride the retained JSON
through byte-for-byte, which is what `Retained` exists for -- but a user who
wants to add or open one needs the CLI or the web vault. See step 6.

**5. Organisations, which are not a `bw` dependency and are listed anyway.**
The direct-REST backend decrypts organisation ciphers; it has simply never
been shown doing it against a real server, and on the owner's server it never
can be. Left off this list it would look settled. See step 5.

**A defect this document originally listed here, and it was not one.** An
earlier draft opened with an alarm: a direct-REST launch logged `bw serve
ready after 0 retries (1666 vault items)` three seconds after announcing
`served by DirectRest`, and this was read as an ungated entry point starting
the subprocess behind `backend_policy`. It was named as the gating first step.

**`bw.exe` was never running.** `bw_serve::wait_for_vault_ready` takes
`&dyn VaultBackend` and is generic over the backend; only its *message* said
`bw serve`, from when that was the only backend there was. On direct REST the
probe called `RestBackend::list_items`, it succeeded on the first try, and the
hardcoded string did the rest. The messages were corrected; nothing else was
wrong.

It is left in this document rather than deleted because the lesson is worth
more than the paragraph: **a log line is evidence about what a function was
told to print, not about what happened.** The `bw_serve_gate` pin was working
the entire time, and half an hour went into doubting it on the strength of a
string.

## The order

Each step is useful on its own, and no step makes the app worse if the one
after it never happens.

1. ~~Find and close the `bw serve` start on the direct-REST path.~~ **Done,
   and it was a mislabelled log line rather than a started subprocess** -- see
   above. "No background process keeps running", which Preferences says on
   screen, was true all along.
2. **Two-factor in `rest/api`.** The real work, and the gate on everything
   below it. Recognising the providers is already done; completing the
   challenge is not.
3. **Point `login_ui` at the REST client** when the account is direct-REST,
   keeping the CLI path for `bw`-selected accounts. Both paths stay, chosen by
   the same `backend_policy::choose` that already decides the vault backend --
   not by a second decision that could disagree with it.
4. **Stop bootstrapping the CLI for new installs** that choose direct-REST.
   `installer/bootstrap-bw.ps1` fetches a Node-based CLI which is by a wide
   margin the largest thing this project puts on a user's disk. This is where
   the prize is.

5. **Organisations**, 6. **attachments**, 7. **Sends** -- the three features
   that would otherwise still send a user back to the CLI. They come after the
   sign-in work because a user who cannot sign in has no use for them, and
   they are ordered among themselves by how much new cryptography each needs.
   Each is set out below.

An earlier draft of this document left these three out, on the reasoning that
they should be decided on their own merits rather than rushed to make a
subtraction possible. That was wrong in a specific way: **a "drop the CLI" plan
that omits the three things people keep the CLI for is not a plan to drop the
CLI.** It is a plan to have two clients forever. They are in scope.

### 5. Organisations

The least new code and the most awkward to prove.

**The code largely exists.** `sync::VaultKeys::unwrap_from` reads the
profile's organisation list, RSA-OAEP-unwraps each organisation key with the
user's private key, and `map_cipher` selects the right key per cipher.
`an_organisation_cipher_decrypts_through_the_rsa_wrapped_org_key` covers it,
and the RSA unwrap underneath has genuine external ground truth -- an OpenSSL
ciphertext -- which is the strongest anchor anything in `rest/crypto.rs` has.

**What is missing is evidence, and it cannot be obtained from the owner's
server.** NodeWarden documents organisations, collections, roles, SSO and SCIM
as *not implemented*; the live probe on 2026-08-26 reported **zero
organisations**, and no run against that account can ever report otherwise. So
this step is not "write the feature" but "verify the feature against a server
that has the feature" -- Vaultwarden, or a free Bitwarden cloud organisation,
either of which means a second test account that is not the owner's.

**What is genuinely absent** is everything *beyond* decryption: collections are
not modelled, and neither is the permission question of whether this user may
edit a given organisation cipher. Writing to an org cipher without knowing that
is how a client produces a 403 the user cannot act on.

### 6. Attachments

Real new cryptography and the first file I/O in this module.

Today attachments **survive** an edit and cannot be *used*: `sync` does not
decrypt them, `write` cannot create, replace or delete one, and they ride the
retained JSON through byte-for-byte -- which is exactly what `Retained` exists
for and is the reason an edit does not destroy them.

What has to be built:

* **Per-attachment keys.** A v2 attachment carries its own key, wrapped under
  the cipher's key or the user's; the file body is encrypted under that, not
  under the user key directly. This is a new key path in `crypto.rs`, not a
  reuse of an existing one.
* **Three routes NodeWarden already has**, so the shapes can be read from its
  handlers rather than guessed: `POST /api/ciphers/:id/attachment/v2` to
  reserve, `POST|PUT /api/ciphers/:id/attachment/:attachmentId` to upload,
  `DELETE` the same path to remove, plus a metadata route.
* **Streaming, or an honest size limit.** Every byte this module handles today
  is a small string. An attachment is a file, and reading one wholly into
  memory to decrypt it is a decision to make deliberately -- with a stated
  ceiling -- rather than by writing the easy version first.
* **A place for the plaintext to land.** Decrypting an attachment means writing
  a decrypted file to disk, which is the first time this app does that. Where
  it goes, who can read it, and when it is deleted are the whole security
  question of this feature, and none of it is answered by anything already
  written.

### 7. Sends

The most new code, and a key hierarchy of its own.

`send.rs` shells out to the CLI (lines 1323, 1405) for the entire feature, and
`rest/mod.rs` states plainly that there is no Send support. NodeWarden has
`handlers/sends.ts`, `sends-public.ts`, `sends-private.ts` and
`sends-shared.ts`, so the routes are readable.

The part that is not a route: **a Send is not encrypted under the user key.**
It has its own randomly generated key, from which the encryption and MAC keys
are derived, and that key travels **in the URL fragment** so that a recipient
who is not a Bitwarden user can open it. That is a different trust model from
everything else in this module -- the whole point is that someone outside the
vault can decrypt it -- and it deserves its own section in whatever plan
implements it rather than being treated as "one more encrypted object".

File Sends inherit every attachment question in step 6 and add expiry, view
counts and optional password protection on top.

## What must not happen: the app deleting `bw`

The obvious reading of "it removes" is that flipping the setting uninstalls the
CLI. **It must not**, and the reason is not caution in general but this case in
particular:

* The user may flip the setting back. They may flip it back *because* they hit
  a Send or an attachment, which is exactly when they least want to be told to
  reinstall something.
* It is a signed third-party binary this app verified
  (`bw CLI ... verified as Bitwarden-signed`), living in a location this app
  chose. Deleting it on a settings change is a destructive act taken on a guess
  about a future choice.
* An organisation account is served by a code path that has never been
  exercised end to end (`rest/mod.rs` says so; the live probe reported zero
  organisations and could not test it). A user who discovers that the hard way
  needs the CLI still there.

**What may happen instead:** new installs that choose direct-REST need never
fetch it, and Preferences may offer removing it as an **explicit action** with
the consequence stated -- naming what stops working, not merely freeing space.
A subtraction the user asks for by name is a different act from one the app
performs on their behalf.

## What this costs

**Two sign-in paths to keep working.** The CLI path cannot be deleted while the
setting exists, so both are live and both need to keep working through account
switches, re-auth and lock. `main.rs` already has the latent bug that the login
flow needs the target account id passed explicitly during a switch
(`2026-08-23`'s amendment records it, unfixed); a second login path lands on
top of that rather than after it.

**Two-factor is a security-sensitive protocol written from scratch.** The vault
cryptography had published vectors to check against and one of them caught a
real unit bug. A 2FA flow has fewer such anchors, and getting it subtly wrong
fails in the direction of a user unable to reach their vault.

**Steps 5 to 7 roughly double this module.** `rest/` is 10,840 lines today,
4,974 of them production, for a feature set that is *reads and item writes*.
Organisations add a permission model, attachments add file I/O and a second key
path, and Sends add a key hierarchy with a different trust model. This is not
an argument against doing them -- a plan to drop the CLI that omits what people
keep the CLI for is not a plan -- but it is the honest size, and it should not
be discovered halfway through.

**A second test account is required and the owner cannot supply it.**
Organisations cannot be exercised against NodeWarden at all. Either a
Vaultwarden instance or a Bitwarden cloud organisation has to exist before step
5 can be verified, and until it does, that step ships on unit tests and an
OpenSSL vector -- which is better than nothing and is not the same as working.

**The support surface widens before it narrows.** Until step 4, a direct-REST
user has the CLI on disk *and* does not use it for the vault, which is one more
state to hold in mind when reading a bug report.

## Testing

* **Nothing here may reach the network in a test.** `mockito` for the wire,
  published vectors where they exist, and `examples/rest_probe` -- which is not
  a test -- for anything only a real server can answer.
* **The two sign-in paths are chosen by `backend_policy::choose` and asserted
  to be**, so a future edit cannot introduce a second, disagreeing decision
  about which client signs in. That is the same rule `bw_serve_gate` enforces
  for the subprocess, applied one layer up.
* **The `bw serve` gate gets the test it evidently lacks.** Whatever started
  the subprocess on the direct-REST path was not caught by the existing pin, so
  closing it means finding out what the pin does not see -- not merely fixing
  the call site.
* **A refusal is never a silent fallback.** A direct-REST account that cannot
  do something must say so by name, as `TwoFactorRequired` already does.
  Quietly shelling out to the CLI to cover a gap would make the setting a lie
  and would put `bw` back on the critical path invisibly.

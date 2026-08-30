# Sends Without the CLI

**Creating, listing and revoking a text Send over REST, so that a direct-REST
account never needs `bw.exe` to publish a link.**

## Why now

`2026-08-26-dropping-the-bw-cli-design.md` lists Sends as step 7 of the
subtraction and calls them "the most new code, and a key hierarchy of its
own". Everything ahead of them has landed: startup, sign-in (including the
second factor and the API key), the cached-session check, and item and folder
writes. `deskwarden/src/send.rs` -- 4 441 lines -- is now one of the last
places in the app that starts a process.

It is also the place where the CLI dependency is most visible to the user.
Every other CLI path had a REST replacement built underneath it before the
user could tell; a Send is the one action where `bw.exe`'s absence would
produce a screen that says the feature is gone.

## What this is not

**This is not a rewrite of `send.rs`.** The module already has the seam:
`pub trait SendRunner` (`send.rs:1053`), with `create_send` (1083),
`list_sends` (1105) and `delete_send` (1147) generic over it, and exactly one
production implementation, `CliSendRunner` (1439). The question this design
answers is which of those pieces the REST path joins, and where.

**It is not file Sends.** See below; that is the largest deliberate omission
and it is stated in the user's words further down.

**It does not touch `bw serve` accounts.** `CliSendRunner`, its two private
constructors, `plan_to_invocation`, `list_invocation`, `delete_invocation`,
`receive_invocation` and the four `cli_send_*` doors keep working, unedited.
The three source guards that wall them in
(`send_ui::source_pins::the_public_surface_of_the_send_module_is_exactly_these_items`,
`every_mention_of_the_blocking_fetch_is_sealed_inside_the_spawning_module`,
and `send_delete_wiring::every_mention_of_the_blocking_delete_is_sealed_inside_its_own_module`)
keep their needle lists, because no needle moves.

## 1. The cryptography, which is the whole risk

A Send is **not** encrypted under the user key. It carries its own key, and
that key is what travels in the URL fragment so that a recipient who has no
Bitwarden account can read the Send. The derivation is public and was read out
of Bitwarden's own source rather than inferred:

* `bitwarden/sdk-internal`, `crates/bitwarden-send/src/send.rs`: the key is
  derived with `derive_shareable_key(key, "send", Some("send"))`, and the
  fields encrypted under it are `name`, `notes`, `text.text` and
  `file.file_name`. The password is `pbkdf2(password, k, SEND_ITERATIONS)`
  with `SEND_ITERATIONS = 100_000` and **the send key itself as the salt**.
* `bitwarden/sdk-internal`, `crates/bitwarden-crypto/src/keys/shareable_key.rs`:
  `derive_shareable_key(secret, name, info)` is
  `HKDF-Expand-SHA256(prk = HMAC-SHA256(key = "bitwarden-" ++ name, msg = secret), info, 64)`,
  and the 64 bytes are `enc_key || mac_key` for AES-256-CBC + HMAC-SHA256.

So, precisely:

```text
k            = 16 random bytes                       (the "send key")
prk          = HMAC-SHA256(key = b"bitwarden-send", msg = k)     -- 32 bytes
okm          = HKDF-Expand-SHA256(prk, info = b"send", 64)
send_enc_key = okm[0..32]
send_mac_key = okm[32..64]

send.key     = EncString(type 2) of k, under the USER key
send.name    = EncString(type 2) of the name, under (send_enc_key, send_mac_key)
send.text.text = same key, the body
send.password  = base64(PBKDF2-HMAC-SHA256(password, salt = k, 100_000, 32))
access URL   = {web vault}/#/send/{accessId}/{base64url(k), unpadded}
```

**What this crate already has.** All of it but the assembly:

| Piece | Where it already lives |
| --- | --- |
| HKDF-Expand-SHA256 from a 32-byte PRK | `MasterKey::stretch` (`crypto.rs:385`) does exactly this shape |
| HMAC-SHA256 | `hmac` 0.12, used by `encrypt_with_iv` (`crypto.rs:771`) |
| PBKDF2-HMAC-SHA256 with a chosen iteration count | `master_key` (`crypto.rs:291`) and `password_hash` (430) |
| AES-256-CBC + HMAC over a `SymmetricKey` | `encrypt` / `decrypt` (`crypto.rs:771` / `629`) |
| `type.iv|ct|mac` parse and format | `EncString` (`crypto.rs:464`) |
| 16 random bytes | `getrandom` 0.2, already a dependency (`accounts.rs:63`) |
| base64url, unpadded | `rest/api.rs:1763`, private today |
| The user key to wrap `k` under | `VaultKeys::user()` (`sync.rs:540`) |

**What must be written.** Four small things, and nothing else:

1. `derive_shareable_key`-for-Sends: the HMAC-then-HKDF above, yielding a
   `SymmetricKey`.
2. A `pub(crate)` way to build a `SymmetricKey` from 64 derived bytes.
   `SymmetricKey::from_64` (`crypto.rs:222`) is private to `crypto.rs` and
   takes a slice; the send derivation needs the same split from an array it
   produced itself.
3. The send-password hash: PBKDF2 with `k` as the salt and 100 000
   iterations.
4. `base64url` promoted from private-in-`api.rs` to `pub(crate)`.

**No new dependency**, and no new primitive.

**Confidence: high, and it has an external anchor.**
`bitwarden-crypto`'s own tests carry vectors for exactly this function, and
they pin both halves of it -- the `"bitwarden-{name}"` HMAC label and the HKDF
`info`:

```text
secret = b"67t9b5g67$%Dh89n", name = "test_key", info = Some("test")
     -> "F9jVQmrACGx9VUPjuzfMYDjr726JtL300Y3Yg+VYUnVQtQ1s8oImJ5xtp1KALC9h2nav04++1LDW4iFD+infng=="

secret = b"&/$%F1a895g67HlX", name = "test_key", info = None
     -> "4PV6+PcmF2w7YHRatvyMcVQtI7zvCyssv/wFWmzjiH6Iv9altjmDkuBD1aagLVaLezbthbSe+ktR+U6qswxNnQ=="
```

Those are not Send vectors -- the name and info differ -- but they pin the
*algorithm*, which is the part that could be silently wrong. The Send
parameters (`"send"`, `"send"`) are then the only thing left to get right, and
they are a two-word literal read from one line of Bitwarden's own Send code.
This is the same standard `crypto.rs` already holds itself to for the master
key ("Verified against Bitwarden's own published test vectors").

**The one thing that is not settled by code**, and is called out rather than
guessed: the **web vault URL** a self-hosted server serves its links from. The
access URL is `{web vault}/#/send/{accessId}/{key}`, and this client knows only
`RestClient::base_url` -- the server root it was configured with. For every
deployment this backend serves (`backend_policy::is_self_hosted` positively
identified, and NodeWarden in particular) the two are the same origin, so the
base URL is used. A split deployment -- API on one host, web vault on another
-- would produce a link with the right key and the wrong host. That is a risk
accepted with its eyes open, not an unknown; see "How it will be known to
work".

## 2. Text Sends yes, file Sends no

**File Sends are deferred, explicitly.**

The honest reason first: **this app cannot create a file Send today either.**
`plan_to_invocation` (`send.rs:520`) writes `"type":0` and `"file":null`
unconditionally, `SendPlan` has no file field, and the composer has no file
picker. `SendSummary::is_file` exists only so the Sends list can *show* a file
Send somebody made elsewhere and say it is not the same thing. So deferring
file Sends is not a regression against the CLI backend -- it is parity with
it.

The reason it stays deferred rather than being folded in: a file Send is a
three-call dance (`POST /api/sends/file/v2` to reserve, then either an Azure
blob PUT or `POST /api/sends/{id}/file/{fileId}` multipart, with
`GET /api/sends/{id}/file/{fileId}` to renew an expired upload URL and a
`DELETE` to roll back a failed one), and it inherits every unanswered question
from step 6 of the CLI-dropping design: streaming or an honest size ceiling,
where a decrypted byte is allowed to land, and what a half-uploaded Send
leaves behind. A rollback path for a partially created public link is exactly
the kind of thing this module is arranged around and exactly the kind of thing
that should not ride along at the end of another feature.

**What a user loses if this ships without file Sends** -- in the words the UI
already has to be able to say, because the Sends list can already contain a
file Send made on another client:

> Deskwarden can send text, not files. This Send holds a file, so you can copy
> its link or delete it here, but it was made somewhere else.

and, on the composer, nothing new at all: there is no file control to explain
the absence of, on either backend.

**Receiving is deferred too**, and this one *is* a loss. `cli_send_receive`
(`send.rs:1749`) is what the record-import path uses to read a Send from its
link. Its REST equivalent is not simply "the same call without a process":
Bitwarden has moved the anonymous access route from
`POST /api/sends/access/{id}` carrying `{"password": <hash>}` to a
send-access-token grant obtained from identity, and this tree cannot read the
target server's handlers to learn which one it speaks. Shipping a guess at an
*unauthenticated* protocol -- the one call in this feature that carries a
decryption key and no session to check it against -- is worse than shipping
three of the four operations. The words for it:

> Reading a Send from a link needs Bitwarden's command-line tool. Publishing,
> listing and revoking your own Sends do not.

## 3. The seam: `SendRunner` survives, unchanged

`SendInvocation` is an argument vector, a base64 stdin body, and two
environment values. It is the right type for a command line and the wrong type
for an HTTP request, and the temptation is to widen it until it is neither.

It is not widened. **`SendRunner`, `SendInvocation`, `plan_to_invocation` and
the three generic functions are edited in no way at all.** The REST path takes
a different route to the same *results*:

```text
                      backend_policy::selected()
                                 |
       BwServe ------------------+------------------ DirectRest
          |                                               |
  send::cli_send_create                         rest::send::create
  send::cli_send_list                           rest::send::list
  send::cli_send_delete                         rest::send::delete
          |                                               |
  SendRunner / SendInvocation                    RestClient / EncString
          |                                               |
          +--------- CreatedSend, SendSummary, SendError -+
```

The shared interface is the **result types**, not the trait: `SendPlan`,
`validate_plan`, the deletion-date arithmetic, `CreatedSend`, `SendSummary`
and `SendError` (with its `is_ambiguous` rule and its user-facing sentences)
are all reused verbatim. That is what `vault_window::send_ui` consumes, and it
is the layer at which the two backends genuinely have the same shape.

Three reasons for this over widening the trait:

1. **A REST runner behind `SendInvocation` would have to parse argv.** It
   would receive `["send", "list"]` and a base64 JSON body and decode them
   back into the request it was built from -- re-materialising the plaintext
   body, which `plan_to_invocation` went to some trouble to keep inside one
   `Zeroizing` buffer, in a second place.
2. **It would have to fake a CLI answer.** `parse_created_send` reads `id` and
   `accessUrl` off `bw`'s stdout. The server returns neither in that form: it
   returns `accessId` and an *encrypted* `name`, and the access URL has to be
   assembled from the key this client generated. A REST runner would be
   synthesising a document for this app's own parser to read back -- the
   definition of distorting one side to fit the other.
3. **The wall.** `CliSendRunner`'s privacy, the public-surface equality and
   the two needle counts are load-bearing and were built by measurement
   against real surviving mutants (`send.rs:1439`'s doc records three of
   them). Adding a second implementation of that trait means widening the
   pinned public surface of `crate::send`. Adding a sibling module means not
   touching it.

What `crate::send` gains is **one visibility change**: `deletion_date` becomes
`pub(crate)` so the REST path stamps the same instant, in the same format,
that the composer's own expiry line was worded from. Nothing else moves.

## 4. Which runner runs where

The choice comes from `backend_policy::choose(server_url, use_official_bw_crypto)`
and from nowhere else -- read through `backend_policy::selected()`, the
installed process fact, exactly as the twelve `bw serve` entry points already
read it.

**The branch is in `vault_window`'s three `real_send_*` helpers**
(`mod.rs:6755`, `8025`, `8684`), not in `send.rs`. Those functions already
read process state on the worker thread rather than capturing it from the
frame -- `bw_path::active_data_dir()` -- for the stated reason that a copy
taken a frame earlier can be stale after an account switch. The backend choice
has exactly that property, so it is read in exactly that place.

A `bw serve` account therefore reaches `crate::send::cli_send_*` on the same
line it reaches today, with the same job object and the same profile
directory. **The CLI path is not edited.**

### Where a REST Send gets its credentials

A Send over REST needs three things a worker thread does not have: the server
URL, a live `Session`, and the **user key** to wrap `k` under (and to unwrap
it again when listing).

`backend_policy` already publishes the first through
`direct_rest_login()`. The other two live in `Authenticated` (session +
`MasterKey`), which today is reachable only from inside
`rest::backend::RestBackend`'s mutex or from the per-account
`user_key_store::UserKeyStore` that `main`'s `adopt` sink writes on every
successful login.

So `BackendEnv` gains one field, paired with `choice` by `install_env` the
same way `direct` already is:

```rust
/// How a worker thread reaches this account's live REST credentials.
/// `Some` exactly when `choice` is `DirectRest`.
pub credentials: Option<Arc<dyn Fn() -> Option<Authenticated> + Send + Sync>>,
```

`main` installs it as a closure over the same `UserKeyStore` that `adopt`
writes -- one source, so a Send cannot be signed with a credential the vault
is not. Two consequences, both deliberate:

* **A missing or unreadable credential is `SendError::Locked`**, whose
  existing sentence is already the right one: "The vault is locked. Unlock it
  and try again."
* **A token refreshed during a Send is not written back to the store.**
  `RestClient::refreshing` will refresh in-memory and the next Send will
  refresh again from the stored token. That is a wasted round trip, not a
  correctness problem, and writing the store from three worker threads to save
  it is a worse trade.

The user key is unwrapped per operation from `RestClient::sync`'s profile via
`VaultKeys::unwrap_from`, which is the one place in this crate that knows how.
A create and a list therefore cost one `/api/sync` each on top of their own
call; a delete costs none, because a revoke needs no key at all.

### The endpoints

Read off Bitwarden's own `SendsController` and its clients' `send-api.service`:

| Operation | Call |
| --- | --- |
| create (text) | `POST /api/sends` |
| list | `GET /api/sends` |
| revoke | `DELETE /api/sends/{id}` |

They are shaped like the folder writes beside them (`create_folder`
`api.rs:1325`): the URL built from `base_url`, the bearer through
`self.bearer(..)`, the body a newtype only the encrypting function can build
(`MappedSend`, after `MappedCipher` and `MappedFolder`), the answer through
`value_from`, and every failure through the existing `RestError` mapping.
`RestError` is then mapped to `SendError` in one place, and it is the place
where the ambiguity rule is preserved: a `Transport` failure on a create is
`SendError::TimedOut` (ambiguous -- the request may have reached the server),
not `Offline`.

## How it will be known to work

**Vectors, first and loudest.** `derive_shareable_key` is asserted against
Bitwarden's two published vectors above, before anything is built on it. A
test that only round-trips this crate's own encryptor against its own
decryptor cannot see a wrong HMAC label, and the wrong label produces a Send
that this app can read and no other client can.

**Pure tests**: the send-key wrapping (`k` encrypted under the user key,
recoverable with it), the password hash's shape and its 100 000 iterations,
the access URL's assembly from an `accessId` and a `k` (and that it is
base64**url** and unpadded -- the standard-base64 mistake produces a link that
silently fails on `+` and `/`), and that a `SendPlan` refused by
`validate_plan` never reaches an HTTP call.

**HTTP tests, in `crate::test_http`** (never `mockito` directly), following
`the_password_grant_sends_every_field_the_server_requires`: that `POST
/api/sends` carries `type: 0`, an encrypted `name`, an encrypted `text.text`,
the wrapped `key`, the `deletionDate` this app computed, and -- when the plan
has one -- `password` and `maxAccessCount`; that the plaintext body appears
nowhere in the request; that `GET /api/sends` decrypts to `SendSummary` rows
with a usable `access_url`; that `DELETE` is path-scoped.

**A negative with a positive control beside it**, per the house rule: the test
that asserts the secret body is absent from the request also asserts the
*ciphertext* is present, so a request that was never built cannot pass it.

**Backend-selection tests**: that `real_send_create` with `DirectRest`
installed spawns no process (`job_object`'s spawn probe is thread-local and
already used this way by
`send_delete_wiring::the_revoke_child_is_spawned_into_the_job_this_window_holds`),
and that with nothing installed -- every test process, `examples/ui_preview` --
it still goes to the CLI.

**A live check, which is the only one that settles it**: publish a
password-protected text Send from Deskwarden against the self-hosted server
with `bw.exe` renamed away, open the link in a browser, read the text back,
then revoke it and see the link die. The access-URL host question above is
settled by that check and by nothing else.

## Sequencing

Three pieces, in this order, because each is worthless without the one before
it and each fails differently:

1. **`rest::send_crypto`** -- the derivation, the wrap, the password hash, the
   URL. Pure, vector-checked, no network. Where a mistake is a *security*
   mistake.
2. **`rest::send` + three `RestClient` methods** -- the wire shapes and the
   error mapping. `test_http`-testable, no UI. Where a mistake is an
   *interoperability* mistake.
3. **The dispatch** -- `BackendEnv::credentials`, `main`'s installation, and
   the three `real_send_*` branches. Where a mistake is a *wiring* mistake,
   and the one that must leave the `bw serve` path byte-identical.

## Status

Design, approved 2026-08-30. The Send key derivation, the PBKDF2 password
parameters, the field list and the endpoint table are cited to Bitwarden's own
published source in the commit that adds this file; the two derivation vectors
are quoted above and are `bitwarden-crypto`'s own.

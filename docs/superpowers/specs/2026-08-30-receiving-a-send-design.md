# Receiving a Send

**Reading a Send from a link over REST, so that a direct-REST account never
needs `bw.exe` at all — and the route it must speak, established from
Bitwarden's own source rather than guessed.**

## Why this was deferred, and what settles it

`2026-08-30-sends-without-the-cli-design.md` shipped create, list and revoke
over REST and left receive on the CLI. Its reason was honest and it was the
right call at the time: the agent could not determine which anonymous-access
route a self-hosted server speaks. `deskwarden/src/send.rs:1765`,
`pub fn cli_send_receive`, is now one of only two remaining code-level `bw`
dependencies.

This document does not guess. Section 1 is the investigation, and every route
in it is quoted from Bitwarden's published server or client source at a named
tag. Section 2 is the version-skew answer, which is the part that decides
whether a design is possible at all. Sections 3 onward are the design.

**The investigation succeeded, and the answer is worse than "one route".**
There is no stable anonymous-access route. There are two, they are not
compatible, and they do not merely differ by path — the newer one is not
anonymous at the HTTP layer at all. The overlap window in which a server
speaks both is roughly seven monthly releases wide and it has already closed
on the newest servers.

---

## 1. The routes, from primary sources

### 1.1 The legacy route — anonymous, one POST

`bitwarden/server`, `src/Api/Tools/Controllers/SendsController.cs`, at tag
`v2026.7.0` (and unchanged in shape back through `v2024.6.2`):

```csharp
[Route("sends")]
public class SendsController : Controller
{
    #region Anonymous endpoints

    [AllowAnonymous]
    [HttpPost("access/{id}")]
    [ProducesResponseType<SendAccessResponseModel>(StatusCodes.Status200OK)]
    [ProducesResponseType(StatusCodes.Status400BadRequest)]
    [ProducesResponseType(StatusCodes.Status401Unauthorized)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public async Task<SendAccessResponseModel> Access(string id, [FromBody] SendAccessRequestModel model)
    {
        var guid = new Guid(CoreHelpers.Base64UrlDecode(id));
        var send = await _sendRepository.GetByIdAsync(guid);
        if (send == null) { throw new BadRequestException("Could not locate send"); }
        ...
        var sendAuthResult = await _sendAuthorizationService.AccessAsync(send, model.Password);
        if (sendAuthResult.Equals(SendAccessResult.PasswordRequired)) { throw new UnauthorizedAccessException(); }
        if (sendAuthResult.Equals(SendAccessResult.PasswordInvalid)) { await Task.Delay(2000); throw new BadRequestException("Invalid password."); }
        if (sendAuthResult.Equals(SendAccessResult.Denied)) { throw new NotFoundException(); }
```

So, exactly:

| | |
| --- | --- |
| Method and path | `POST {base}/api/sends/access/{accessId}` |
| Auth | none — `[AllowAnonymous]` |
| `{accessId}` | the base64url GUID out of the link fragment, decoded server-side by `CoreHelpers.Base64UrlDecode` |
| Body | `SendAccessRequestModel` — `{"password": string \| null}`, `[StringLength(300)]`, and nothing else |
| 200 | `SendAccessResponseModel` |
| 400 | send not found, or password wrong (after a deliberate 2 s delay) |
| 401 | password required and none was sent |
| 404 | denied: disabled, expired, past deletion, max access count exhausted, or an email-gated Send |

The `Send-Id` header the client sets is **not** required — the server-side
check for it is commented out in every tag examined, in both this route and
the file-download one. It is sent anyway; see §5.

`SendAccessResponseModel` (`src/Api/Tools/Models/Response/SendAccessResponseModel.cs`):

```csharp
Id   = CoreHelpers.Base64UrlEncode(send.Id.ToByteArray());
Type = send.Type;                     // 0 text, 1 file
Name = sendData.Name;                 // EncString under the SEND key
Text = new SendTextModel(textData);   // { text: EncString, hidden: bool }
File = new SendFileModel(fileData);
ExpirationDate, CreatorIdentifier
```

Client side, `bitwarden/clients` at `cli-v2025.8.0`,
`libs/common/src/tools/send/services/send-api.service.ts`:

```ts
async postSendAccess(id: string, request: SendAccessRequest, apiUrl?: string) {
  const addSendIdHeader = (headers: Headers) => { headers.set("Send-Id", id); };
  const r = await this.apiService.send("POST", "/sends/access/" + id, request, false, true, apiUrl, ...);
```

and the file variant is the third shape that was half-remembered:
`"/sends/" + send.id + "/access/file/" + send.file.id`. All three remembered
paths are real; they are three different routes, not three guesses at one.

### 1.2 The current route — a bearer token, minted at identity

`bitwarden/server` `main` (and `v2026.8.0`), same file:

```csharp
[Authorize(Policy = Policies.Send)]
[HttpPost("access/")]
public async Task<IActionResult> AccessUsingAuth()
{
    var guid = User.GetSendId();
```

There is **no `[AllowAnonymous]` Send-access route left** in `v2026.8.0`. The
only anonymous endpoint on the controller is
`POST sends/file/validate/azure`, which is an Azure EventGrid webhook.

The token is minted by a custom OAuth grant on the identity server.
`bitwarden/server`,
`src/Identity/IdentityServer/RequestValidators/SendAccess/readme.md`:

> #### All Requests
> - `send_id` - Base64 URL-encoded GUID of the send being accessed
>
> #### Password Protected Sends
> - `password_hash_b64` - client hashed Base64-encoded password.
>
> #### Email OTP Protected Sends
> - `email` … - `otp` …

and the field names are pinned by
`src/Identity/IdentityServer/RequestValidators/SendAccess/SendAccessConstants.cs`
(`SendId = "send_id"`, `ClientB64HashedPassword = "password_hash_b64"`,
`Email`, `Otp`, and the error codes `send_id_invalid`,
`password_hash_b64_required`, `password_hash_b64_invalid`, `email_required`,
`email_and_otp_required`, carried in a `send_access_error_type` field).

Client side, `bitwarden/clients` `main`,
`apps/cli/src/tools/send/commands/receive.command.ts`:

```ts
// Defined in SendAccessConstants.TokenRequest in the server repo.
const fields: Record<string, string> = {
  grant_type: "send_access",
  client_id: "send",
  scope: "api.send.access",
  send_id: sendId,
};
switch (credentials?.kind) {
  case "password": fields.password_hash_b64 = credentials.passwordHashB64; break;
  ...
}
await this.apiService.nativeFetch(new Request(identityUrl + "/connect/token", {
  method: "POST",
  headers: { "Content-Type": "application/x-www-form-urlencoded; charset=utf-8", Accept: "application/json" },
  body: /* form-encoded */ }));
```

and the spend, in `libs/common/src/tools/send/services/send-api.service.ts`:

```ts
async postSendAccess(accessToken: SendAccessToken, apiUrl?: string) {
  const setAuthTokenHeader = (headers: Headers) => {
    headers.set("Authorization", "Bearer " + accessToken.token);
  };
  const r = await this.apiService.send("POST", "/sends/access", null, false, true, apiUrl, setAuthTokenHeader);
```

So: two requests, a form body then an empty body, and the send id travels in
the **token**, not the path.

### 1.3 The one thing that did not move: the password hash

Both routes take the same value. The current CLI:

```ts
private async getUnlockedPassword(password: string, keyArray: Uint8Array) {
  const passwordHash = await this.cryptoFunctionService.pbkdf2(password, keyArray, "sha256", 100000);
  return Utils.fromBufferToB64(passwordHash);
}
```

That is `send_crypto::SendKey::password_hash` in this crate, byte for byte —
PBKDF2-HMAC-SHA256, 100 000 iterations, the 16-byte send key as the salt,
32 bytes out, standard base64. **The crypto in `rest/send_crypto.rs` is not
redesigned by this document and is not touched by it.** Receiving runs the
same derivation in reverse: `SendKey::from_bytes(fragment)` →
`cipher_key()` → `decrypt` of `text.text`.

The link is parsed the same way in both eras:

```ts
private getIdAndKey(url: URL): [string, string] {
  const result = url.hash.slice(1).split("/").slice(-2);
  return [result[0], result[1]];
}
```

— the **last two** `/`-separated segments of the fragment, which is what makes
`#/send/{id}/{key}` and a bare `#{id}/{key}` both work.

---

## 2. Version skew: there is no stable route, and nobody probes

### 2.1 When each route existed

`src/Api/Tools/Controllers/SendsController.cs`, fetched at each tag and
grepped for the two route attributes:

| server tag | `HttpPost("access/{id}")` (anon) | `HttpPost("access/")` (bearer) |
| --- | --- | --- |
| v2025.6.0 | yes | no |
| v2025.9.0 | yes | no |
| v2025.12.0 | yes | no |
| v2026.1.0 | yes | no |
| **v2026.1.1** | yes | **yes** |
| v2026.2.0 … v2026.7.2 | yes | yes |
| **v2026.8.0** | **no** | yes |

The bearer route appeared in `v2026.1.1`. The anonymous route survived beside
it for seven releases and was removed in `v2026.8.0`.

### 2.2 Does the official client probe? No.

The CLI on `clients` `main` is **token-only**. `attemptAccess` calls
`getTokenWithRetry` first and has no branch that ever issues
`POST /sends/access/{id}`; the legacy `SendAccessRequest` import is gone from
the file. The old CLI (`cli-v2025.8.0`) is the mirror image — legacy-only,
with no knowledge of `send_access`. Bitwarden's answer to skew is *"upgrade
the client with the server"*, which works for a first-party client shipped in
lockstep and does not work here: this app must read a link from a server it
does not control and did not ship.

There is also **no capability flag to read**. `GET /api/config` is
unauthenticated (`src/Api/Controllers/ConfigController.cs` has no
`[Authorize]`) and returns `version`, `gitHash` and a `featureStates`
dictionary — but the bearer route is not behind a feature flag in any tag
above, and `version` on a third-party Bitwarden-compatible server is a string
that server chose. Deciding a route from it would be trusting a self-reported
version to be honest about a route it may never have implemented either way.

### 2.3 What this app must therefore do

**Probe, on a discriminator that cannot mean anything else, and prefer the
newer route.**

The order is token-first, and that is not an aesthetic preference — it is
forced by the fact that the legacy route has no clean "I do not exist" signal:

* A server that has removed the legacy route answers `404` for the route.
  A server that still has it answers `404` for a Send that is **disabled,
  expired, past its deletion date, or out of accesses**. Those are the same
  status code with opposite meanings, and a client that read one as the other
  would tell a user their link is dead when the route is dead, or retry an
  expired Send forever.
* A server that has never heard of the `send_access` grant answers
  `POST /identity/connect/token` with `400 unsupported_grant_type` — Duende's
  own answer, and the only thing that string can mean. A token endpoint
  always exists on a Bitwarden server, so a `404` on `/identity/connect/token`
  is likewise unambiguous: this is not a server that has one.

So: try the grant; on `unsupported_grant_type` (or `404` at the token
endpoint) and **only** on those, fall back to the legacy anonymous POST. Every
other identity answer — `send_id_invalid`, `password_hash_b64_required`,
`password_hash_b64_invalid`, `email_required` — means the grant *is*
supported and is talking about this Send, and must never trigger a fallback.

**The cost of the probe is one extra request, and only against old servers.**
On a `v2026.1.1+` server the grant succeeds or fails meaningfully and nothing
is retried. On an older one, one 400 is spent before the legacy POST.

### 2.4 What a user on an older self-hosted server experiences

**Nothing.** The fallback is silent and the wording never mentions a route.
That is the requirement, stated as a property rather than an intention:

* The receive succeeds, with the same text and the same screen, on any server
  from `v2024.x` through `v2026.7.x` (legacy path) and on `v2026.1.1+`
  (token path). The union is every tag examined.
* The password prompt appears in the same place on both paths, because both
  have a distinguishable "password required" answer (`401` on the legacy
  route; `password_hash_b64_required` on the grant) and both take the same
  hash.
* A wrong password says "wrong password" on both (`400 Invalid password.`;
  `password_hash_b64_invalid`).
* A dead link says "this Send is gone" on both (`404`; `send_id_invalid` —
  whose readme says it covers "a Send that exists but is disabled, expired,
  past its deletion date, or has exhausted its max access count, as well as a
  `send_id` with no matching Send").

**The user is told which route was used exactly never.** The only place the
distinction may surface is the log, and only as the fact that a fallback
happened, with no URL, no id and no hash. A sentence in the UI naming a route
version would be this app confessing an internal it fixed for the user.

### 2.5 The two failures this design does not paper over

* **Email-OTP Sends are refused by name, on both paths.** They are a
  `v2026.1.1+` feature (`AuthType.Email`), the legacy route answers `404` for
  them by design (`if (send.AuthType == AuthType.Email …) throw new
  NotFoundException();`), and the token path needs an e-mail round trip and a
  code entry this window has no room for on this branch. The refusal must say
  what it is — "this Send asks the recipient to prove an e-mail address, which
  Deskwarden cannot do yet" — and must not be reachable from the generic
  "gone" sentence, or the user will believe a live link is dead.
* **File Sends stay unreceivable over REST.** They already are: this app
  cannot create one, `SendSummary::is_file` exists to say a row came from
  somewhere else, and the download needs a second route (`access/file/{id}`
  in both eras) plus a stream decrypt. A file link is refused by name, in the
  same sentence shape.

---

## 3. Where the change lives

Nothing in `crate::send` is edited. `cli_send_receive`, `receive_invocation`,
`CliSendRunner` and the three source guards that wall them in keep working
byte for byte — a `bw serve` account is served by `backend_policy::choose`
returning `BwServe`, and that arm is untouched. This is the same discipline
`sends-without-the-cli` used for the other three operations.

| File | What it gains |
| --- | --- |
| `deskwarden/src/rest/send_link.rs` (new) | Pure: parse a link into `(origin, access_id, SendKey)`. No I/O. |
| `deskwarden/src/rest/api.rs` | Three routes: `mint_send_access_token`, `access_send_with_token`, `access_send_anonymously`. A third agent. Census changes. |
| `deskwarden/src/rest/send.rs` | `receive`, the probe, the classification, and `receive_on_active_account`. |
| `deskwarden/src/vault_window/send_ui.rs` | The receive path stops being unconditional CLI; `RECEIVE_NEEDS_THE_CLI` narrows to the `BwServe` arm. |
| `deskwarden/src/rest/mod.rs` | The module doc's "what is missing" list loses receive. |

### 3.1 The link parse is its own module, and is pure

A link carries a **host**. `access_url` in `send_crypto.rs` builds one from
this client's own base URL; parsing one means reading a host the user pasted.
That is a trust decision, not a string split, which is why it is not a private
helper inside `send.rs`:

* If the link's origin matches the account's configured server, use the
  configured base URL. This is the whole of what this branch supports.
* If it does not, **refuse**, and say so. The official CLI prompts
  interactively to override ("Do not proceed if you do not trust …"); this app
  has no such prompt on the Sends screen and inventing a modal for it is a
  separate change. A refusal that names the two hosts is honest and is not a
  silent failure. It is also the safe direction: the alternative is this
  process making an HTTPS request, carrying a PBKDF2 hash of a password the
  user typed, to a host chosen by whoever wrote the link.
* Origin comparison is **exact**, never a suffix or a substring, for the
  reason `backend_policy::is_self_hosted` already states about
  `vault.bitwarden.community`. Reached through `favicon::host_from_url`, a
  fourth caller and not a fourth copy.

The key is the last fragment segment, unpadded base64url, 22 characters, 16
bytes. A fragment that decodes to any other length is refused: a truncated key
produces a link that opens nothing, and `SendKey::from_wrapped` already takes
that position for the wrapped case.

### 3.2 The anonymous request is a third agent, not the write agent

`RestClient` holds `auth_agent` and `write_agent` (`api.rs:755`, `760`). An
anonymous Send access gets `anon_agent`, and the reason is a property the type
system can then hold: **no function that builds an anonymous request may be
able to reach `self.bearer(..)`**, because `bearer` takes the user's
`Session`, and the whole point of this path is that the user's vault token
must never be handed to a request addressed by a link. `receive_invocation`
already makes exactly this decision on the CLI side — `CliSendRunner::new`
rather than `with_session`, so `BW_SESSION` never reaches that child. This is
the same rule at the REST layer.

The bearer used by `access_send_with_token` is the **send access token**, a
different type from `Session` and constructible only from the grant response.
It is not `Debug`, by the rule `Challenge`, `service_token::Token` and
`SendInvocation` follow.

### 3.3 The census in `rest/api.rs` must assert *more*

`the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin`
(`api.rs:2940`) currently pins `send_json(` at 7, `send_json(&body)` at 0 —
"no hand-built body left anywhere" — and the write agent's body-carrying calls
at 5. An anonymous receive interacts with all three, and every interaction
must move in the direction of asserting more, not of adding an allowance:

1. **The legacy route's `{"password": …}` must not be hand-built.** It is a
   mapped type, `MappedSendAccess`, whose only constructor takes a `&SendKey`
   and an `Option<&str>` and calls `SendKey::password_hash`. The census gains
   `assert_eq!(production.matches("send_json(access.body())").count(), 1)`,
   and `send_json(&body)` **stays 0**. The `send_json(` total goes 7 → 8, and
   the accompanying comment names the eighth, as the comment already names the
   other seven.
2. **The grant is a form, not JSON**, so it is invisible to the `send_json`
   count. A count that cannot see a body is a count that can be evaded, so the
   census gains a second, symmetric assertion: the module's `send_form(` sites
   are enumerated too — the password grant, the API-key grant, the refresh,
   and now the send-access grant — with each named. This is new coverage of
   ground the census never covered, which is the "assert more" direction.
3. **The anonymous routes must never carry the user's bearer.** A new source
   pin, beside the census and in the same shape: over the production half,
   every occurrence of `anon_agent` is counted, `self.bearer(self.anon_agent`
   is asserted **zero**, and `Session` is asserted absent from the three
   anonymous function signatures. Its positive control is that
   `self.bearer(self.write_agent` is asserted non-zero in the same test — a
   grep that found nothing anywhere would pass the zero assertion vacuously,
   and that is precisely this house's named defect.
4. **The write agent's count stays 5.** A receive that touched it would be a
   receive that had been given a session.

---

## 4. The flow, end to end

```text
paste a link
  |
  parse (pure)  -> origin, access_id (base64url guid), SendKey
  |               refuse: wrong host / bad key length
  |
  POST {base}/identity/connect/token
      grant_type=send_access&client_id=send&scope=api.send.access&send_id={access_id}
      [&password_hash_b64=... once known]
  |
  +-- 400 unsupported_grant_type, or 404  ---> LEGACY PATH
  |                                            POST {base}/api/sends/access/{access_id}
  |                                            body {"password": hash|null}
  |                                            401 -> ask for the password, retry
  |                                            400 "Invalid password." -> wrong password
  |                                            400 other / 404 -> gone
  |
  +-- 400 password_hash_b64_required ------> ask for the password, retry the grant
  +-- 400 password_hash_b64_invalid ------> wrong password
  +-- 400 email_required / email_and_otp_required -> refuse by name (§2.5)
  +-- 400 send_id_invalid ----------------> gone
  +-- 200 {access_token, expires_in} -----> POST {base}/api/sends/access
                                             Authorization: Bearer {access_token}
                                             (empty body)
  |
  both paths converge on one SendAccessResponseModel
  |
  type != 0 -> refuse by name (file Send)
  cipher_key = SendKey::cipher_key()          [unchanged, already verified]
  text       = decrypt(cipher_key, text.text) -> Zeroizing<String>
```

The two paths converge on **one** response parser, and that is deliberate:
`SendAccessResponseModel` is the same class in both eras — the bearer route
returns `new SendAccessResponseModel(send)` exactly as the anonymous one did.
Two parsers for one shape is how the two come to disagree about what a
readable Send is.

### 4.1 The answer type is `Zeroizing<String>`, and matches the CLI's

`cli_send_receive` returns `Result<Zeroizing<String>, SendError>`
(`send.rs:1765`), and `receive_send` fills it with the child's stdout. The
REST path returns the same type with the decrypted text in it, so
`send_ui`'s receive card is a branch on the backend and not a second screen.
`SendError` gains no variant: "gone", "wrong password" and "not something this
app can read" are all `SendError::Rejected` with their own sentence, which is
what `rest::send::map_error` already does for the other three operations.

### 4.2 Ambiguity is `Safe`, always

`Ambiguity` (`rest/send.rs`) distinguishes "this failure may have published a
link" from "this failure published nothing". A receive publishes nothing and
can never be `Ambiguous`. A `Transport` failure here is `SendError::Offline`,
never `TimedOut`, and a design that let `Ambiguous` reach this path would tell
a recipient to go and check a Sends list that is not theirs.

---

## 5. Small decisions, written down rather than left to the implementation

* **`Send-Id` header:** sent on the legacy route, matching the official
  client, even though every server tag has the check commented out. It costs
  nothing, it is what a server that turns the check on will require, and it
  carries no secret — the id is already in the path.
* **`client_id=send` and `scope=api.send.access`:** sent verbatim. They are
  not in `SendAccessConstants` (which pins only the four request fields), so
  they are pinned in this crate against `receive.command.ts` on `main`, with
  the source named in the doc comment as `crypto.rs` does for its vectors.
* **`expires_in`:** refused if not a finite number. The official CLI's own
  comment says why — "A missing or non-numeric `expires_in` would make
  expiresAt NaN, which isExpired() reads as 'not expired' forever". This crate
  has the same trap in `Session`; `api.rs`'s existing test
  `a server that sends no expires_in must not be assumed to have sent one`
  is the precedent and the new token gets the same treatment.
* **No token cache, ever.** The official CLI declines to cache tokens minted
  against a server the user did not configure, for a reason that applies here
  in full: "a token from it must never be reachable by a later request to
  another server". This design refuses foreign hosts outright, so the risk is
  smaller — but the token is single-purpose, short-lived and worth nothing
  after the one request, so it is dropped and never stored.
* **The 2-second delay** the server takes on a wrong password on the legacy
  route is inside the request. The receive deadline must exceed it with room,
  or a wrong password reads as a timeout. `WRITE_DEADLINE` is the existing
  precedent to size against.

---

## 6. What this does not settle

* **Whether the account's own server implements either route.** The server
  driving this work is a third-party Bitwarden-compatible implementation on
  Cloudflare Workers, and `rest/mod.rs` already instructs the reader to treat
  the API as a subset. Nothing in Bitwarden's source can say what that server
  answers. The probe is built so that the *answer* to this is what the code
  does rather than what the design assumed: a server with neither route
  answers `404` at both, and the honest refusal for that case — "this server
  does not offer Send links to this app" — is a sentence the plan must
  require, not a case the plan may leave to a generic error.
* **A split web-vault/API deployment.** `access_url`'s doc already flags that
  this client assumes one origin for both; the parse in §3.1 inherits that
  assumption and refuses rather than guessing when a link's origin is not the
  configured one.
* **Whether the legacy route will be reachable a year from now.** It was
  removed in `v2026.8.0`. The fallback is for servers that have not moved, and
  it is written as a fallback rather than as a peer so that deleting it later
  is a subtraction and not a redesign.

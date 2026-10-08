# Person sharing on Bitwarden's official path, on every self-hosted server

**Status:** approved by the owner 2026-10-08. This is project 1 of 3:

1. Sharing on the official path, self-hosted servers. This document.
2. A direct bitwarden.com connection.
3. Sharing on bitwarden.com.

It covers both repos:

- Deskwarden (`deskwarden/`).
- NodeWarden (`E:\Personal\nodewarden`, branch `feature/missing-features`).

It amends `2026-09-27-personal-sharing-design.md` (the "base spec"). Where the two disagree, this one wins.

## The owner's rule

**"Has to be always 100% compatible" with Bitwarden.** For this project that means:

1. **One path, on every server.** Deskwarden shares through Bitwarden's own routes, request bodies and invitation links. Nothing else is used, on NodeWarden, Vaultwarden or official self-hosted Bitwarden. The base spec's two NodeWarden accelerators (§7.2) are no longer called:
   - direct add;
   - `GET /invitations/mine`.

   The profile and invitation owner-email fields are no longer read either.
2. **NodeWarden behaves as a Bitwarden server.** Where its answer to an official route differed from Bitwarden's, it changes back. There are two exceptions, both chosen by the owner. Vaultwarden behaves the same way in both cases, so every official client already copes with them (§5).
3. **Release condition.** The official Bitwarden CLI and web vault pass the compatibility checks in §7 against NodeWarden.

## 1. Owner's decisions (2026-10-08)

| Question | Decision |
|---|---|
| What "compatible" covers | Both: one official path in Deskwarden, and NodeWarden matches Bitwarden. |
| bitwarden.com | In scope, as projects 2 and 3. The old rule that Deskwarden's own client never talks to Bitwarden's production servers is reversed there, not here. |
| How a NodeWarden recipient accepts | A server setting. With mail, NodeWarden sends Bitwarden's invitation email. Without mail, it accepts on invite, as Vaultwarden does with mail disabled. |
| Member list (`mini-details`) for plain members | Stays hidden on NodeWarden, as on Vaultwarden. Official Bitwarden shows it. |

## 2. Capabilities, not dialects

Base spec §7.1 detected a *dialect* with `GET /invitations/mine`. That probe, the `REASON_NOT_NODEWARDEN` refusal and the `slice_one` gate are removed. Deskwarden instead learns three facts, each from an official route it already needs, and caches them per session.

| Capability | How it is learned | What it changes |
|---|---|---|
| **Roster readable by members** | A recipient's `GET /organizations/{id}/users/mini-details` answers 200. A 404 or 403 means no. | Whether "Shared by" can be verified (§4.3). The sharer's dialog warns that recipients can see each other (§3.4). |
| **Accepts on invite** | Right after `POST …/users/invite`, the roster read shows the new person with `status: 1` (Accepted) rather than `0` (Invited). | Whether the card waits for an accept or goes straight to confirm. Nothing else branches on it. |
| **Clients can create organisations** | `POST /organizations` succeeds. An official self-hosted server refuses it (base spec §4.3). | Whether the first share creates the org or asks the user to (§3.1). |

The server still needs these routes, which official and Vaultwarden already have and NodeWarden has had since slice 1:

- `GET /users/{id}/public-key`
- `PUT /ciphers/{id}/share`
- `POST …/collections` with `users`
- `GET …/users?includeCollections=true`
- confirm and re-invite

If one of them answers 404, sharing is off with the reason `server-too-old` (unchanged wording, *"Sharing with a person needs a newer server."*). The word "NodeWarden" leaves that sentence.

`bw serve` stays unavailable (`bw-serve`) until project 3.

## 3. Sharer

### 3.1 The sharing organisation

- **Creatable** (NodeWarden, Vaultwarden):
  - created on the first share with the official body (base spec §9.5);
  - named `Shared by <email>` (base spec §5.1).
- **Not creatable** (official self-hosted). The Share dialog says *"Your server needs an organisation for sharing. Create one named "Shared by <your email>" in the web vault, then try again."* Deskwarden adopts it by name and ownership (base spec §5.1). Seat limits are the licence's. When an invite fails for lack of seats, the dialog shows the server's own error text.
- **Recognising the sharer's own org.** It stays as built:
  - the account's role in it is Owner;
  - the cached `sharing_org_created` flag or id;
  - the server's word, never the name alone.

  The owner-email field this used on NodeWarden is replaced by the roster. On every server the sharer has ManageUsers, so the sharer can read `GET …/users` and find their own row with type Owner.

### 3.2 Inviting

`POST /organizations/{id}/users/invite` with:

- `emails: [E]`
- `type: 2`
- `collections: [{id: C(E), readOnly: true, hidePasswords: false, manage: false}]`
- `groups: []`
- `permissions: null`
- `accessSecretsManager: false`

The answer is an empty 200. A roster read follows (base spec §7.3), which finds the new org user and its status.

### 3.3 Confirming

Unchanged from what is built. The daemon confirms every Accepted person on its own:

- it fetches `GET /users/{userId}/public-key`;
- it wraps the org key to that key;
- it calls `POST …/users/{ouId}/confirm`.

A key that changed since a previous confirm for the same email is not auto-trusted (base spec §11.5).

### 3.4 What the card and dialog say

| State | Card |
|---|---|
| Invited (`status 0`) | `Waiting for X to accept`, then `· invited N days ago`. The server's expiry setting is invisible to clients, so no expiry is claimed. |
| Accepted, being confirmed | Same as Invited. It lasts one pass. |
| Accepted, the recipient has no key yet | `Waiting for X to sign in once` (base spec §6.1). |
| Confirmed | `X · Shared` |

- **"Waiting for X to create an account" is removed.** No official route tells a client whether an invited email has an account.
- **Re-invite** is `POST …/users/{ouId}/reinvite` on every server.
- **The roster-visibility warning.** When the server lets members read the roster, the Share dialog shows *"On this server, people you share with can see each other's email addresses."* The sharer cannot learn this from a recipient's probe, so it is decided by `GET /config`:
  - the warning shows unless the server names itself Vaultwarden or NodeWarden;
  - an unnamed server gets the warning, because that direction discloses rather than hides.

  NodeWarden adds `server.name: "NodeWarden"` to `/config` (§5).

## 4. Recipient

### 4.1 Servers that email invitations

This covers official Bitwarden, Vaultwarden with mail, and NodeWarden with mail.

- The recipient clicks the link in the email (the web vault accepts it), or uses **Paste an invite link** under Shared with me.
- Deskwarden parses Bitwarden's link format:
  - the query of `…/#/accept-organization?…` with `organizationId`, `organizationUserId`, `email`, `organizationName` and `token`;
  - the parse is case-insensitive on keys and accepts the fragment or the query.
- It refuses a link whose `email` is not the signed-in account's, with that sentence.
- It then calls `POST /organizations/{organizationId}/users/{organizationUserId}/accept {token}`.
- **No Decline.** Bitwarden has no decline route. An unanswered invite expires. The base spec's Accept / Decline row is NodeWarden-only and is removed.

### 4.2 Servers that accept on invite

This covers Vaultwarden and NodeWarden without mail. There is nothing to do. The record appears once the sharer's daemon confirms. Until then, `profile.organizationsNew` lists the org with no key, and Shared with me shows `Waiting for <org's sharer> to finish sharing` (base spec §6.9).

### 4.3 "Shared by"

- **Roster readable** (official Bitwarden). The sharer is the member with `type: 0` in `mini-details`, and "Shared by" shows that email. It is verified by the server. This replaces NodeWarden's owner-email attestation.
- **Roster hidden** (NodeWarden, Vaultwarden).
  - The label reads `Shared by X · unverified`. X is parsed from the org name, as built.
  - The tooltip reads *"This server does not say who owns the organisation. The name comes from the organisation's own title."*
  - The record stays view-only and is otherwise unchanged.
  - The planted-organisation risk (base spec review) is therefore visible, not silently trusted.
- Every place that today requires an attested owner keeps requiring one. That covers the sharing-org recognition and the Shared with me grouping. Where none can exist, the record is grouped by organisation, shown as unverified.

## 5. NodeWarden: back to Bitwarden's behaviour

| Area | Today | Change |
|---|---|---|
| Invitation delivery | No invitation email. An invitee learns of it only through `GET /invitations/mine`. | A setting, `INVITE_MAIL`, read from the environment. |
| — `INVITE_MAIL` on | — | `POST …/users/invite` sends Bitwarden's invitation email: subject, link format and token lifetime match the official server. It is sent through Cloudflare Email Service. The token is the one the official accept route checks. |
| — `INVITE_MAIL` off (default) | — | Exactly as Vaultwarden with mail disabled. An existing account is `status 1` (Accepted) at invite time. A new email becomes Accepted when that account registers. |
| Role change of a confirmed member | Owner, admin or manager reachable only by remove plus a new invitation (47869c4). | `PUT …/users/{ouId}` changes `type` as the official server does. 47869c4's restriction is removed. Its reason was direct add without consent, which Deskwarden no longer uses. Direct add itself stays limited to `member` (it is a NodeWarden-only route, so no official client depends on it). |
| Bulk user operations | Capped at 15 ids (`maxBulkUserIds`). | Set to the official server's own limit. The plan's first NodeWarden task reads it from Bitwarden's server source. If Bitwarden has no limit, NodeWarden uses the base spec's request-size bound instead of a count. |
| `/config` | `server: null` | `server: {name: "NodeWarden", url}`. Official clients already accept this field: Vaultwarden fills it. |
| `mini-details` for plain members | 404 (managers only) | **Unchanged**, by the owner's decision. It matches Vaultwarden. |
| Direct add, `invitations/mine`, profile and invitation owner-email fields | Present | **Unchanged.** Official clients never call or read them. Deskwarden stops using them. NodeWarden's own web vault may keep using them. |
| SSO consent, the confirmed-manager gates, rate limits | Present | **Unchanged.** Each matches or is stricter than the official server's security behaviour. None changes what a correct official client sees. |

## 6. Deskwarden changes, by module

- **`rest/sharing.rs`**
  - Remove the dialect probe, `DIALECT_NODEWARDEN`, `REASON_NOT_NODEWARDEN` and the `slice_one` probe.
  - Add the three capabilities of §2.
  - Every operation uses base spec §7.3's official column.
  - Direct add and `invitations/mine` calls are deleted, not branched around.
- **Invite-link parsing.** A new pure module, `invite_link.rs`, with its own tests.
- **Shared with me.** It gains **Paste an invite link**. The Accept / Decline row for listed invitations is removed.
- **Attestation.** The owner-email reads are replaced by the roster. The "unverified" label and tooltip are added.
- **Share dialog.** It gains the create-it-yourself sentence (§3.1) and the roster-visibility warning (§3.4).

## 7. Testing

- **NodeWarden**, in its existing test harnesses:
  - `INVITE_MAIL` off: an existing account is Accepted at invite, and a new account becomes Accepted on registration.
  - `INVITE_MAIL` on: the email is composed in Bitwarden's format with the mail call faked, and the token is accepted by `…/users/{ouId}/accept`.
  - A role change through `PUT …/users/{ouId}`.
  - The bulk limit.
  - `/config`'s server name.
- **Deskwarden**:
  - each capability branch, against faked server answers;
  - invite links copied from a real Bitwarden email, plus malformed ones and a wrong email.
- **Compatibility, the release condition.** These run against a local NodeWarden (`wrangler dev`):
  - the official `bw` CLI signs in, syncs, lists org members as the sharer, moves an item into the sharing org, and confirms a member;
  - the official web vault opens the organisation and shows the shared record to the recipient;
  - each check is recorded in the plan's ledger.
- **By hand:**
  - one full share and receive on Vaultwarden, in Docker, with mail off;
  - one on NodeWarden with `INVITE_MAIL` off;
  - one paste-link accept, with `INVITE_MAIL` on, through Cloudflare Email Service's test mode.

## 8. Non-goals

- bitwarden.com (projects 2 and 3).
- A decline action. Bitwarden has none.
- Showing whether an invited email has an account.
- Changing what official Bitwarden or Vaultwarden do. Deskwarden takes them as they are.

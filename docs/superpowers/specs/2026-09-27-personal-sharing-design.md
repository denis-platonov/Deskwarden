# Personal sharing: one record, one person, no copies

**Status:** designed 2026-09-27. Design only; nothing here is built. It covers
Deskwarden (`deskwarden/`) and NodeWarden (`E:\Personal\nodewarden`, branch
`feature/missing-features`). The HTML mock for the surfaces is
`docs/design/6-sharing-mock.html`.

In one line: **Share** on a record files it into a per-person collection of a
hidden organisation that belongs to the sharer. The recipient gets the record
view-only, it autofills for them, and every Bitwarden client that syncs sees
it as an ordinary organisation item.

**Owner's decisions after review (2026-09-27), which override the text
below wherever they differ:**
- **NodeWarden first, official servers later.** Slices 1–3 target NodeWarden
  only. On official Bitwarden and Vaultwarden, Share is not offered yet; it
  offers a Send instead. Official support becomes its own later slice, with
  the limits in section 8: the user creates the org, recipients can see each
  other's emails (the dialog warns), and invites arrive as a pasted link.
- **Every open question in section 16 takes its recommendation.** In
  particular:
  - NodeWarden's direct add (no accept step) stays.
  - For a recipient without an account, Deskwarden says so and, if the sharer
    is the admin, mints an invite code for them.
  - The email lookup is rate-limited.
  - A recipient is not notified when a share stops (v1).
  - A record moved back to the personal vault takes a new id.
  - An emptied person collection is kept.
  - Records with attachments that have no per-item key are refused.
- **Mock points not yet reviewed** (subtitle wording, six header controls,
  group labels in Shared with me): the mock's choices stand until the owner
  says otherwise.

---

## 1. Goals and non-goals

**Goals**

- Share one record with one person by email, from the record, in two clicks.
  The only other step is the recipient accepting once per sharer, and only
  where the server requires it.
- It works on NodeWarden, official self-hosted Bitwarden and Vaultwarden. On
  each, official Bitwarden apps see a shared record through the standard
  `/api/sync` and can read it.
- Recipients can view, copy and autofill. Only the sharer edits.
- Sharing with someone who has no account yet is recorded as *waiting*. It
  lands without the sharer doing anything once they sign up and accept.
- Confirmation is automatic (Keeper-like). The sharer's daemon confirms an
  accepted recipient whenever it runs. The fingerprint phrase can still be
  checked by hand.
- Stopping a share removes exactly that person's access. Stopping the last
  one returns the record to the personal vault.
- All of it goes through the daemon's client-agnostic API, so a future
  browser extension gets sharing with no new protocol.

**Non-goals**

- Sharing on the `bw serve` backend, which covers every bitwarden.com
  account (section 7.4).
- Shared *editing*, groups, multiple owners, or delegating "manage".
- Records with attachments that have no per-cipher key (section 9.3). They
  are refused with a sentence rather than half-moved.
- Showing or managing the sharing organisation as an organisation. It is
  plumbing. Deskwarden hides it everywhere.
- Protecting a shared value from its recipient. View-only is a UI and server
  rule, not cryptography: whoever can autofill a password can read it.

---

## 2. Terminology

| Term | Meaning |
|---|---|
| **Sharer** | The account sharing a record. It owns the sharing org. |
| **Recipient**, **person** | Someone a record is shared with, named by email. |
| **Sharing org** | The one organisation per sharer that Deskwarden creates for sharing. It is hidden from every Deskwarden organisations surface. |
| **Person collection** | One collection in the sharing org per recipient. Its name is the recipient's email, encrypted under the org key. The recipient has read-only access to it and to nothing else. |
| **Share** | A (record, person) pair: the record is filed in that person's collection. |
| **Org user** | Bitwarden's `OrganizationUser`: a membership row with its own id (`ouId`), distinct from the account's user id. |
| **Invite / accept / confirm** | Bitwarden's three membership steps. *Invite* creates the row (status 0). The recipient *accepts* it with a token (status 1). The sharer *confirms* it by handing over the org key wrapped to the recipient's RSA public key (status 2). |
| **Dialect** | Which of the servers' protocols Deskwarden is talking: *NodeWarden* (official routes plus two NodeWarden extensions) or *Official* (Bitwarden and Vaultwarden). See section 7. |
| **Pin** | The SHA-256 of a recipient's public key, recorded locally the first time Deskwarden confirms them (trust on first use). |

---

## 3. What exists today, verified

### 3.1 NodeWarden (`feature/missing-features`)

What the brief said, checked against the code:

| Claim | Verdict |
|---|---|
| `/members/invite` gives an existing account direct membership with a client-supplied `encryptedOrgKey`. A non-user gets a pending invitation with a token and a 7-day expiry. | **Correct.** `handleInviteOrganizationMember`, `src/handlers/organizations.ts:378`. The existing-user branch writes `status: 1` at once, with the key if supplied. The invitation branch writes `token: uuid+uuid`, `expiresAt: now + 7 d`. **No email is sent** for either. |
| `/members/public-key?email=` | **Correct**, manager-only (`:351`). It is also an account-existence oracle for anyone who owns an org (section 12.4). |
| Collections CRUD, `/collections/:id/items` add and remove | **Correct**, manager-only. **But collection names are capped at 100 characters** (`:627`, `:670`). An encrypted name (`2.iv|ct|mac`) of an ordinary email is about 120 characters, so **NodeWarden rejects official-style encrypted collection names today.** |
| Accept, decline, my-invitations routes | **Correct.** `POST /api/organizations/invitations/accept|decline {token}`, `GET /api/organizations/invitations/mine`. The last one returns the token to its invitee. On accept, the member row becomes `status: 1` with `encryptedOrgKey: null`. That is Bitwarden's "accepted, not confirmed" state spelled differently. |
| Ciphers take `organizationId` / `collectionIds` on create and update | **Correct, and too permissive.** `reconcileOrgCipherMembership` (`src/handlers/org-ciphers.ts`) lets **any** accepted member with collection access create, edit, soft-delete, **permanently delete** or **un-org** (`organizationId: null`) an org cipher. There is no read-only concept. `member.collections` is `string[]`, and cipher responses always say `edit: true` (`ciphers.ts:867`). |
| `POST /api/ciphers/:id/share` is a stub | **Correct.** It returns the cipher unchanged (`router-authenticated.ts:382`). There is no `PUT` and no `/collections` route. |
| Can plain members list members? | **Yes, and that breaks decision 5.** `handleGetOrganizationMembers` (`:330`) needs only accepted membership. Every member gets the whole roster: emails, names, roles, collection ids and each member's wrapped org key. |
| Does `/api/sync` give a member only their collections' items? | **Items: yes** (`getAccessibleOrgCiphers`). **Collections: no.** `handleSync` (`src/handlers/sync.ts:149`) lists *every* collection of every org the user belongs to, and the recipient holds the org key that decrypts their names. Each cipher's `collectionIds` also lists every collection it is in, including other recipients' (`ciphers.ts:871`). `GET /organizations/:id/collections` and `GET .../collections/:cid/items` are open to any member too. |

Further findings that matter here:

- **Registration is invite-code-only** after the first user
  (`src/handlers/accounts.ts:251`, `:373`). An admin issues the codes
  (`/api/admin/invites`). A person with no account cannot "just sign up".
  Section 6.5 and open question Q3 cover this.
- **Profile organisations carry no numeric `type` or `status`**
  (`src/utils/profile-response.ts`). Official clients read both. `key` is
  correct: the member's wrapped copy.
- **Deleting the sharer's account promotes a recipient to owner**
  (`transferOrganizationDataOnUserDelete`,
  `src/services/storage-organization-repo.ts:432`). The highest-ranked
  surviving member inherits the org, which for a sharing org is a plain
  recipient. They would then reach every collection.
- **An accept pushes only to the acceptor.** `bumpMembersRevision([userId])`
  on accept (`:905`). The sharer's clients learn nothing until they sync.
- Schema changes are additive `ALTER TABLE` lines in
  `src/services/storage-schema.ts`, with `STORAGE_SCHEMA_VERSION` bumped.
  Tests are `node:test` files in `scripts/*.test.ts`, run with `tsx --test`.

### 3.2 Deskwarden

- `src/rest/organizations.rs` **reads** organisations (from sync),
  collections (from sync, names decrypted under the org key) and rosters (one
  `GET /api/organizations/{id}/members` per org). It is a `Directory` served
  by the daemon as `GET /organizations`. `Member::confirmed` counts any
  non-zero status as confirmed. That is right for counting viewers and wrong
  for confirming: Bitwarden's `1` is *accepted, not confirmed*. Its closing
  doc already lists the four hazards of moving an item into an org. This
  design adopts them.
- `src/rest/sync.rs`: `VaultKeys` holds the user key and every org key it
  could unwrap. `CipherKeys::for_cipher` resolves an item's field key: its own
  key, else its org's, else the user's. `organizationId`, `collectionIds`,
  `edit`, `viewPassword` and `permissions` all ride in `VaultItem::other`.
  `Profile` reads `profile.organizations` only. That list is Confirmed-only
  on official servers and Vaultwarden. `organizationsNew` (Accepted +
  Confirmed) is not read yet, and the recipient's "waiting to finish" state
  needs it (section 6.9).
- `src/rest/crypto.rs`: AES-CBC-HMAC encrypt and decrypt (type 2) and
  **RSA-OAEP-SHA1 decrypt only** (`decrypt_rsa`, `unwrap_org_key`). The
  module says so: "There is no *asymmetric* encryption". **Missing:** RSA
  encrypt to a public key, symmetric key generation, RSA key-pair generation
  and the fingerprint phrase. The `rsa = "0.9"`, `sha1`, `hkdf` and
  `getrandom` crates are all already dependencies.
- `src/rest/write.rs`: `encrypt_item` re-encrypts a `DecryptedItem` under
  whatever key `CipherKeys::for_cipher` names *for the body it is given*. So
  setting `organizationId` in the body before calling it encrypts under the
  org key. That is most of a share. It refuses to re-encrypt values that
  never decrypted, which is the rule a whole-item move needs.
- The `shared` pill (`item_list.rs:2026`) and the SHARING card
  (`detail.rs:4314`, `sharing_card`) are about **live Sends** today: "it
  always means someone can open this right now". A person-share satisfies
  that sentence. The pill keeps its meaning, and the card gains person rows.
- `src/sent_history.rs` holds people: `suggestions`, `forget`, and the
  "these are shared" question (`record_ui::forget_title` / `forget_body`).
  Sharing reuses the same address book.
- `src/backend_policy.rs::choose`: **direct REST only for a positively
  self-hosted server *and* `use_official_bw_crypto == false`**. Otherwise the
  account is on `bw serve`, including a self-hosted server where the user
  kept the default. Sharing is a direct-REST feature (section 7.4).
- The daemon owns the vault (`vault_api.rs`, `vault_host.rs`,
  `change_feed.rs`, `daemon_link.rs`). The feed has two kinds of change today,
  `Vault(Announcement)` and `Sends`. The hub worker applies and publishes.
- Push types handled: `0..=11 | 17..=19` are vault changes, and `12..=14` are
  Sends (`rest/notifications.rs:165`).
- The passphrase wordlist (`assets/wordlist.txt`) has 4,096 words. Bitwarden's
  fingerprint phrase uses the **EFF long list, 7,776 words**, so it cannot be
  reused.

---

## 4. Bitwarden's protocol for this (research)

Confidence: **H** = read in source; **M** = strong recollection or indirect
evidence; **L** = inferred. Paths are in `bitwarden/server` (`server/`),
`dani-garcia/vaultwarden` (`vw/`) and `bitwarden/clients` (`clients/`), on
`main`.

Local copies of every file cited are in the session scratchpad (`r\s`, `r\v`,
`r\c`). They are not part of the repo.

### 4.1 Official server: membership

`server/src/Api/AdminConsole/Controllers/OrganizationUsersController.cs`
unless noted.

**Invite** — `POST /organizations/{orgId}/users/invite` (H)

- Needs ManageUsers: Owner, Admin, or Custom with that permission.
- Body `OrganizationUserInviteRequestModel`:
  - `emails[]` and `type`, required;
  - `collections[{id, readOnly, hidePasswords, manage}]`
    (`SelectionReadOnlyRequestModel`);
  - `groups[]`, `permissions` and `accessSecretsManager`.
- **200 with an empty body.** The new org user's id is not returned. A roster
  read finds it.
- `SaveUsersSendInvitesAsync` makes each row with `UserId = null`, `Email`
  and `Status = Invited`, **without looking up whether the account exists**.
  Existing and new emails are indistinguishable from then on.
- Emails already in the org are skipped silently.

**Accept** — `POST /organizations/{orgId}/users/{ouId}/accept {token, resetPasswordKey}` (H)

- It needs a logged-in user, not org membership.
- `AcceptOrgUserCommand.AcceptOrgUserByEmailTokenAsync` always:
  - validates the token;
  - requires the invite email to equal the user's;
  - requires status Invited.

  It then sets Accepted and `UserId`, and clears `Email`.
- **The token.** `OrgUserInviteTokenable`
  (`src/Core/Auth/Models/Business/Tokenables/`) is a DataProtector token
  carrying org user id and email.
  - Its lifetime is 5 days, overridden by
    `GlobalSettings.OrganizationInviteExpirationHours` (default 120, so
    configurable).
  - **Only the invitation email carries it.**
  - There is no token-free accept in the public controllers. Invite links
    (`POST /organizations/users/invite-link/accept`) exist, but need the org
    feature `UseInviteLinks` and an allowed-domain list.

**Confirm** — `POST /organizations/{orgId}/users/{ouId}/confirm {key, defaultUserCollectionName?}` (H)

- Bulk: `POST …/users/confirm {keys:[{id,key}]}` → `[{id, error}]`.
- It needs ManageUsers, and status Accepted with a `UserId`.

**Public keys** (H)

- `POST …/users/public-keys {ids}` → `[{id, userId, key}]`. The field is
  `key`, not `publicKey`. It needs ManageUsers.
- `GET /users/{id}/public-key` (`src/Api/KeyManagement/Controllers/UsersController.cs`)
  is open to any authenticated user. It returns
  `{object:"userKey", userId, publicKey}`.

**Official auto-confirm** (H) exists as policy `AutomaticUserConfirmation`
(type 18).

- It is still an admin *client* doing the key exchange after a push, for the
  User role only. It removes emergency access org-wide.
- This design does not use it. Deskwarden is that client.

**Who can see the members** (H)

- `GET …/users` checks ManageUsers (or account recovery) and otherwise answers
  404, so a plain User is refused.
- **But `GET …/users/mini-details` is `[Authorize<MemberOrProviderRequirement>]`,
  documented as available to all members.** It returns `id, userId, type,
  status, name, email` for every member. **On an official server a recipient
  can list the other recipients.** See section 8.7.

**Invitations and sync** (H)

- No route lets a user list their own pending invitations. Invited rows have
  no `UserId`.
- `/sync`: `profile.organizations` holds **Confirmed only**.
  `profile.organizationsNew` holds Accepted + Confirmed (`Status IN (1,2)`),
  and clients read it
  (`clients/libs/common/src/models/response/profile.response.ts`).
- `DELETE …/users/{ouId}` needs ManageUsers.
- Re-invite is `POST …/users/{ouId}/reinvite`, which needs ManageUsers (M:
  well known, not re-read for this design).
- `POST /organizations/{id}/leave` is for members. It is blocked for claimed
  accounts and Key Connector users.

### 4.2 Official server: ciphers and collections

`server/src/Api/Vault/Controllers/CiphersController.cs`, `CipherService.cs`,
`src/Api/AdminConsole/Controllers/CollectionsController.cs`.

**Share** — `PUT /ciphers/{id}/share` (POST is an obsolete alias) (H)

- Body `{collectionIds: string[] (≥1), cipher: CipherRequestModel}`, with
  `cipher.organizationId` required.
- It requires the cipher to be the caller's and **personal**: "One or more
  ciphers already belong to an organization.".
- It checks `lastKnownRevisionDate`.
- It refuses attachments when the org has no storage.
- `cipher.key` carries the per-cipher key.
- It answers a `CipherResponseModel`, and the id is unchanged.
- Bulk: `PUT /ciphers/share {collectionIds, ciphers[]}`.

**Collections of a cipher** — `PUT /ciphers/{id}/collections {collectionIds}`
and `/collections_v2` (H)

- The caller must be able to edit the cipher. `v2` also requires
  `ViewPassword` and answers `{unavailable, cipher}`.

**Org to personal: no route** (H)

- Share refuses org ciphers.
- `update_cipher_from_data` / the update path refuses an `organizationId`
  change ("Organization mismatch").
- Clients create a personal copy and delete the org one (M for the client
  workflow).

**Delete** (H)

- `DELETE /ciphers/{id}` needs `Manage || Edit`, or `Manage` when the org
  limits item deletion.
- A read-only member cannot delete.

**Create collection** — `POST /organizations/{orgId}/collections` (H)

- Body `{name: EncString, externalId, groups[], users[{id: ouId, readOnly,
  hidePasswords, manage}]}`.
- **`CreateCollectionCommand` refuses a collection nobody can Manage**,
  unless the org allows admin access to all items. It enforces
  `MaxCollections`.
- Access changes go through `PUT {id}` (full lists) or `POST bulk-access`
  (add only).

**What sync gives a member** (H, `src/Sql/dbo/Vault/Functions/UserCipherDetails.sql`)

- Confirmed orgs only.
- Only ciphers in collections assigned to them, directly or through a group.
- Only those collections.
- Per viewer: `edit = !readOnly`, `viewPassword = !hidePasswords`,
  `manage = manage`.

**Owners see only assigned collections in `/sync`** (H). **The sharer must be
assigned (with Manage) to every person collection**, or their own shared
records vanish from their own vault.

**Collection members** — `GET …/collections/{id}/users` needs read access to
that collection's settings. A read-only member gets 404. (H)

### 4.3 Official server: creating an org

`OrganizationsController.cs`, `OrganizationCreateRequestModel.cs`.

- Body: `name`, `billingEmail`, `planType`, `key` (required),
  `keys{publicKey, encryptedPrivateKey}` (required), `collectionName`,
  `useSecretsManager` (required), and payment fields. (H)
- **Self-hosted cannot create an org from a client.** `POST /organizations`
  and `create-without-payment` are `[SelfHosted(NotSelfHostedOnly = true)]`.
  Self-hosted creates an org only by uploading a cloud-bought license:
  `POST /organizations/licenses/self-hosted`. Only Families and Enterprise
  self-host, and Families is capped at 6 seats. (H)
- **Cloud Free plan**: 2 seats, 2 collections, and one Free org per
  owner/admin. That is one recipient under structure A. (H for the values in
  source; M that production pricing still matches, because it now comes from
  `PricingClient`.)
- Org name length: `[StringLength(50)]` (M).

### 4.4 Client crypto

- **Org key**
  (`clients/libs/legacy-crypto/src/services/legacy-compat-key.service.ts`,
  `makeOrgKey`) (H):
  - a fresh 64-byte AES-256-CBC-HMAC key;
  - **encapsulated to the creator's own RSA public key**, not wrapped under
    the user key;
  - it becomes the create body's `key`.
- **Org key pair** (`makeKeyPair(orgKey)`) (H): RSA-2048. The public key is
  base64, and the private key is wrapped under the org key. The web's
  `generateOrganizationEncryptionData` also encrypts `collectionName` under
  the org key.
- **Confirm** (`default-organization-user.service.ts` `buildConfirmRequest`;
  `apps/cli/src/admin-console/commands/confirm.command.ts`):
  - `encapsulateKeyUnsigned(orgKey, memberPublicKey)` (H);
  - the result is type 4, `Rsa2048_OaepSha1_B64` (M: the SDK body was not
    read; it matches what `profile.organizations[].key` decrypts with in
    Deskwarden today).
- **Share** (`libs/common/src/vault/services/cipher.service.ts`
  `shareWithServer`; `sdk-internal/crates/bitwarden-vault/src/cipher/cipher.rs`
  `move_to_organization`) (H for the code, M for the no-cipher-key detail):
  - it throws for a cipher already in an org;
  - it sets `organization_id` and re-encrypts at encrypt time;
  - with a per-cipher key only that key is re-wrapped; otherwise the fields go
    under the org key.
- **Fingerprint** (`KeyService.getFingerprint`, M, not re-read for this
  design): HKDF-Expand over SHA-256 of the public key, with the user id as
  info, into 5 EFF-long-list words (section 9.6). **Needs a captured vector
  before it ships.**

### 4.5 Vaultwarden

`vw/src/api/core/organizations.rs`, `ciphers.rs`, `src/config.rs`,
`src/db/models/*.rs`.

**Org creation** (H)

- It is allowed unless `ORG_CREATION_USERS` or a SingleOrg policy forbids it.
- `plan_type` is ignored, and there are no seat or collection limits.

**Invite** (H, `send_invite`, Admin+)

- An unknown email fails if `INVITATIONS_ALLOWED=false`. Otherwise a
  placeholder user is created, plus an `Invitation` row when mail is off.
- **With mail disabled, an existing user is Accepted at invite time.** On
  registration, every Invited membership of a placeholder becomes Accepted.
- The token is a JWT (`InviteJwtClaims`), with `INVITATION_EXPIRATION_HOURS`
  defaulting to 120.
- Accept always checks the JWT. The no-mail bypass is the automatic Accepted
  status, not a token-free route.

**Auto-confirm: none** (H). `// AutoConfirm = 18, // Not supported`.
Confirm needs Admin, Accepted status and a key.

**Members** (H)

- `GET …/users` and `mini-details` need Manager+, so **plain members cannot
  list members** (unlike official).
- `public-keys` needs Admin, and its `key` is null for a placeholder user.
- `GET /users/<id>/public-key` answers 404 for a user with no key.

**Ciphers** (H)

- Share (POST = PUT) checks write access to each collection.
- The update path refuses an org change ("Organization mismatch").
- `/collections` needs an editable collection.

**Sync** (H)

- Confirmed memberships and assigned collections only, unless `access_all`.
  Owners and admins see everything.
- `edit = !read_only`, `viewPassword = !hide_passwords`.
- `profile.organizations` is Confirmed and `organizationsNew` is Accepted +
  Confirmed. **Invited is never in the profile**, so there is no invitation
  list.
- Collection members need Manager+. Read-only members cannot delete.

### 4.6 Bitwarden CLI

`clients/apps/cli/src/vault.program.ts`, `program.ts`,
`admin-console/commands/*.ts`.

- **Present** (H):
  - `bw move <id> <orgId> [collectionIds]` (and `bw share`, a deprecated
    alias);
  - `bw confirm org-member <id> --organizationid`, which fetches the public
    key and encapsulates;
  - `bw list org-members` (it needs ManageUsers);
  - `bw list org-collections` and `bw create/edit/delete org-collection`;
  - `bw edit item-collections`.
- **There is no invite, accept or org-create command** (H). `bw serve` exposes
  the same set.

---

## 5. The model: structure A

### 5.1 One sharing org per sharer

- **Name.** `Shared by <sharer email>`, in plaintext. Organisation names
  are plaintext on every server (`rest/sync.rs::Organization::name` says
  why).
  - It is what an official app shows the recipient. That is truthful and
    harmless: they know who shared with them.
  - It carries the sharer's email to Deskwarden's "from …" label on servers
    where the recipient cannot see the member list.
  - Official probably caps org names at 50 characters (section 4.3, M).
    Deskwarden creates orgs only on NodeWarden and Vaultwarden, but it keeps
    names within 50 anyway: an email longer than 40 is cut to 39 characters
    plus `…`. The label then reads `from denis.platonov@a-very-long-dom…`,
    which is honest.
  - NodeWarden's 100-character cap does not bind.
- **Recognising the org.**
  - Deskwarden recognises a sharing org by that name: the exact prefix
    `Shared by `, then something that parses as an email, optionally ending
    in `…`.
  - It is *the sharer's own* when the account's org role is Owner (`type 0`,
    or NodeWarden `role: "owner"`) and the email is the account's.
  - The chosen org id is cached per account in `settings.json`
    (`sharing_org_id`). The cache wins while that org still exists and the
    account still owns it, so a later rename in the web vault does not strand
    the shares.
  - If several orgs match, the oldest (`creationDate`) wins and the others are
    left alone.
- **Why a name marker and not local settings alone.** Local settings answer
  only for the sharer, and only on one PC. The recipient's Deskwarden must
  recognise *someone else's* sharing org, to hide it from the rail and to
  label "from …". So must the sharer's second PC. The name is the only place
  every client can read. The server-side `identifier` field is SSO-only on
  official servers, and a collection `externalId` would be plaintext per
  collection.
- **Created on the first share** on NodeWarden and Vaultwarden, with the
  account as Owner. The org key is 64 random bytes, wrapped to the owner. The
  org also gets an RSA key pair, as official clients generate one. Section
  9.1 has the body.
- **Adopted, never created, on official self-hosted.** Only a license upload
  makes an org there (section 4.3). Deskwarden asks the user to create one
  named `Shared by <email>` in the web vault, and then adopts it.

### 5.2 One collection per person

- **Name**: the recipient's email, lower-cased and trimmed, encrypted under
  the org key (type 2), as Bitwarden encrypts every collection name.
  - The sharer's web vault shows it as a readable person, which is useful
    when managing there.
  - The recipient sees only their own collection, named with their own email.
  - A server cannot forge a name that decrypts: it has no org key, and the MAC
    fails. So **the email ↔ collection mapping was written by a holder of the
    org key**, and the auto-confirm rule leans on that (section 11.5).
  - The holders are the sharer, and the recipients where they may create
    collections. Collection creation is manager-only on NodeWarden and
    Vaultwarden. On official servers it depends on the org's "limit collection
    creation" setting, which the sharer's web vault should have on (Q7).
- **Access**:
  - The recipient's org user gets
    `{readOnly: true, hidePasswords: false, manage: false}` on this collection
    and on nothing else.
  - `hidePasswords` stays false because autofill needs the password. Hiding
    it would be a UI promise with no force (section 12.3).
  - **The sharer's own org user gets `{readOnly: false, hidePasswords: false,
    manage: true}` on every person collection.** It is set in the create body
    itself. Official servers refuse a collection nobody manages, and show an
    owner only the collections assigned to them (section 4.2). Without it the
    sharer's own shared records would vanish from their vault.
- **Mapping.** Person ↔ collection is resolved by the decrypted name, case
  folded. Whether that person can *currently read* it is resolved by the
  roster: which org user holds access to which collection. Two collections
  decrypting to one email: the oldest is the person's and the others are
  merged into it on the next write, by re-filing their items and deleting the
  empty collection.
- **Kept after the last share.** A person whose last shared record is stopped
  keeps their membership and their empty collection. Sharing with them again
  is then one call, with no re-invite and no re-accept. The person is removed
  only by **Forget** (the ✕ in the suggestions, behind the existing "these
  are shared" question). That removes their membership and deletes their
  collection.

### 5.3 A record shared with several people

It is one org cipher with `collectionIds = [C(a), C(b), …]`, and no copies.
Each recipient's sync shows the same cipher once (section 10.2 filters
`collectionIds` per viewer).

### 5.4 What is kept locally, and why

`sharing_state.json` in `settings::config_dir()`, per account. No secrets,
no values, no link.

```json
{
  "account": "<user id>",
  "pins": { "m.reyes@ledgerline.com": { "sha256": "<hex of SPKI DER>", "userId": "<id>", "at": 1790000000000 } },
  "moves": { "<org cipher id>": { "folderId": "<personal folder id or null>", "favorite": false } },
  "unfinished": [ { "kind": "purgeOrgCopy", "orgCipherId": "<id>", "personalId": "<id>" } ]
}
```

- **`pins`**: trust on first use for auto-confirm (section 11.5).
- **`moves`**: the folder and favourite a record had before it left the
  personal vault. Org ciphers carry neither on NodeWarden, and carry them per
  user on official. They are restored when it comes back.
- **`unfinished`**: the second half of a two-step move that failed (section
  13). It is retried on every sharing pass.

Everything else is read from the server on each pass: who is invited, the
states, and which records are in which collections. Two PCs never disagree
about shares.

---

## 6. User flows

### 6.1 Per-person states (sharer side)

| State | How Deskwarden knows | SHARING card row | Actions |
|---|---|---|---|
| **Shared** | Org user confirmed (official `status 2`; NodeWarden `status 1` with a key) and has access to C(E) | `m.reyes@… · Shared` | Stop sharing · Show fingerprint |
| **Waiting to accept** | Org user `status 0` (official), or `status 1` with no key and confirm not yet run | `Waiting for m.reyes@… to accept` | Stop sharing · Re-invite (official) |
| **Waiting for an account** | NodeWarden only: a pending invitation row and no account for the email. Official servers cannot tell this apart from the previous state (section 4.1). | `Waiting for m.reyes@… to create an account (expires in 5 days)` | Re-invite · Stop sharing |
| **Waiting for a first sign-in** | Accepted, but the account has no public key yet (a Vaultwarden placeholder, or an account never opened in a Bitwarden app). The key fetch answers 404 or `key: null`. | `Waiting for m.reyes@… to sign in once` | Stop sharing |
| **Invite expired** | NodeWarden only: `expiresAt` in the past. Elsewhere the Waiting row says `invited N days ago` (section 8.5). | `m.reyes@…'s invite expired` | Re-invite · Stop sharing |
| **Key changed** | Accepted, but the public key differs from the pin | `m.reyes@…'s key changed. Check the fingerprint before sharing.` | Show fingerprint · Share anyway · Stop sharing |
| **No longer receiving** | C(E) holds the record, but no org user for E has access (they left, or were removed elsewhere) | `m.reyes@… is no longer receiving this` | Re-invite · Stop sharing |

The pill and rail count a record as **shared** only when at least one person
is in state *Shared*. That keeps the pill's promise.

### 6.2 Sharer: first share ever (no org yet)

1. On a record, press **Share** (beside the envelope). The dialog shows one
   email field with `sent_history` suggestions.
2. Deskwarden checks that the record can move (section 9.3). If not, the
   dialog says why and offers nothing further.
3. `POST /sharing/item/{id} {"email"}` goes to the daemon. The daemon:
   1. creates the sharing org (section 9.1);
   2. creates C(E) (section 9.2);
   3. invites E with read-only access to C(E) (section 7);
   4. moves the record into the org, filed in C(E) (section 9.3);
   5. records the address in `sent_history` as a person (a new
      `SentRecord.kind`, section 11.7);
   6. publishes `Change::Sharing` and `Change::Vault(Item{id})`.
4. The card shows E in whatever state the server reports. For an existing
   NodeWarden user that is **Shared** immediately.

The order is chosen so that a failure leaves the record untouched. Nothing
moves the record until org, collection and invite exist. A failure after the
invite leaves an invited person with an empty collection. That is harmless,
and **Retry** finishes it.

### 6.3 Sharer: org exists, new person

As 6.2 without step 3.1. If the record is already in the org (shared with
someone else), step 3.4 becomes
`PUT /api/ciphers/{id}/collections {collectionIds: current ∪ {C(E)}}`.

### 6.4 Sharer: existing person, another record

As 6.3 without the invite. It is one or two calls, and the record is visible
to them at their next sync (a push on every server here).

### 6.5 Sharer: the person has no account

- **NodeWarden**
  - `/members/invite` makes a pending invitation.
  - The card says *Waiting for X to create an account (expires in N days)*.
    N comes from the invitation's `expirationDate`.
  - They need a registration invite code from the server admin (Q3).
  - After registering, they accept, in NodeWarden's web vault or in
    Deskwarden (6.9). The sharer's daemon then confirms (6.7) and the record
    lands.
- **Official**
  - `/users/invite` sends the invitation email. It needs SMTP on
    self-hosted; without mail the token never reaches anyone.
  - They register through the email's link, and the web vault accepts as part
    of registration.
  - The card says *Waiting for X to accept*, because the server does not say
    whether an account exists (section 4.1).
- **Vaultwarden with mail enabled**: the same as official.
- **Vaultwarden with mail disabled** (common for personal instances)
  - An existing account is **Accepted at invite time**. The sharer's daemon
    confirms in the same pass, so there is no accept step at all: as
    Keeper-like as NodeWarden.
  - A new email gets a placeholder account. It becomes Accepted when they
    register, and then confirmed on the next pass.
- **Re-invite**
  - NodeWarden: a new token and a new 7 days.
  - Official: `POST …/users/{ouId}/reinvite`, a new email and a new token
    lifetime.

### 6.6 Sharer: accepted, awaiting confirm

This state is transient. The next sharing pass confirms. Triggers are listed
in section 11.5. Until then the card says *Waiting for X to accept*. A
distinct "accepted" wording would be noise that lasts seconds on NodeWarden
and at most one poll interval on official servers.

### 6.7 Sharer: auto-confirm

This is the daemon's job. Section 11.5 has the rule, and section 12 its
cost. It needs no window.

### 6.8 Sharer: stop sharing, and the last person

- **Stop sharing X on record R**
  - If R stays in other collections:
    `PUT /api/ciphers/{id}/collections {collectionIds: current − C(X)}`. R
    disappears from X's next sync, and nothing else changes.
  - If C(X) was R's last collection, R goes back to the personal vault
    (section 9.4). Its id changes. The daemon publishes `ItemGone(old)` and
    `Item(new)` and re-keys the id-keyed local files (`sent_history`,
    `fill_stats`, `fill_recall`) from old to new.
- **Forget X** (the ✕ on a suggestion)
  - The existing question lists the records shared with X as well as the
    Sends.
  - **Yes** stops every share with X (as above), removes their org user
    (`DELETE …/users/{ouId}`, or on NodeWarden `DELETE /members/{userId}` or
    revoke the invitation), and deletes C(X).

### 6.9 Recipient: receiving

- **NodeWarden, existing account**
  - Nothing to do. The record appears at the next sync: view-only, marked
    `from denis@…`, and listed under **Shared with me**.
  - Consent: this is NodeWarden's existing direct-membership rule, not a
    Bitwarden one (Q1).
- **NodeWarden, invited while without an account**
  - After registering, Deskwarden reads `GET
    /api/organizations/invitations/mine`.
  - It shows a row at the top of **Shared with me**:
    `denis@… wants to share with you — Accept · Decline`.
  - Accept posts the token. The sharer's daemon confirms. The record arrives
    on the push that confirm produces.
- **Official, and Vaultwarden with mail**
  - The invitation arrives by email. Neither server lets an invitee list
    pending invitations (sections 4.1, 4.5), so Deskwarden cannot show them on
    its own.
  - Deskwarden offers **Paste an invite link** under Shared with me. It parses
    `organizationId`, `organizationUserId`, `token` and `email` from the
    link's query, and then accepts with the same row.
  - Accepting in the web vault works equally well.
- **Vaultwarden without mail**: nothing to accept. The invite made them
  Accepted already (section 6.5).
- **Accepted, awaiting confirm**
  - This state is visible only through `profile.organizationsNew`, where it
    has no key. `profile.organizations` lists Confirmed only (section 4.1).
  - Deskwarden starts reading `organizationsNew`, and shows `Waiting for
    denis@… to finish sharing` for such an org, and nothing else.
- **Owner stopped sharing / forgot them**: the record leaves their vault at
  the next sync. Deskwarden shows no notice (Q6).
- **Stop receiving from X**: a quiet action on the "from" row. It leaves the
  org (`POST /api/organizations/{id}/leave`), and every record from X goes.
- **View-only in Deskwarden**
  - Items whose `other.edit == false` get no Edit, Trash, Archive, Move or
    app-match writes. The detail pane says `Shared by denis@… · view only`.
  - App matches live inside the item, so a recipient cannot bind a desktop
    app to a received record (Q8).
  - The server enforces the same (section 10.3). Deskwarden's rule is
    courtesy, not security.
- **Hidden org**: sharing orgs are left out of the ORGANISATIONS rail
  section, the org filters and the `Audience` count. Their items are filtered
  under **Shared with me** instead.

### 6.10 Rail

- **Sent** lists the sharer's Sends *and* the records they share with people.
  Each record row reads `to m.reyes@…, +2`.
- **Shared with me** lists imported Sends *and* records from others' sharing
  orgs (`from denis@…`), with pending invitations on top.
- Both counts come from the daemon's `GET /sharing` (section 11.3), so the
  rail is right in a linked window with no sync of its own.

---

## 7. Dialects: which calls, on which server

### 7.1 Detecting the dialect

The daemon probes once per session, after the first whole read:
`GET /api/organizations/invitations/mine`.

- A 200 with a list means **NodeWarden**.
- Anything else (404, 400 or 405) means **Official**.

A second probe checks for the slice 1 routes on NodeWarden:
`GET /api/users/{own userId}/public-key`. Official and Vaultwarden already
have this route. On NodeWarden a 404 means **sharing is off**, with the reason
`server-too-old`. Before slice 1, NodeWarden cannot keep a recipient
read-only (section 12.3).

Both results are cached for the session. `/api/config` cannot decide the
dialect: NodeWarden sends `server: null`, as official servers do.

It can separate Vaultwarden from Bitwarden *within* the official dialect,
which matters for one sentence. Vaultwarden's `/api/config` names itself in
`server.name` (M). A Vaultwarden hides the roster from recipients, and a
Bitwarden server does not (section 8.7). An unnamed server is treated as
Bitwarden, the direction that discloses rather than hides.

### 7.2 One core, two NodeWarden accelerators

The core is Bitwarden's routes, on every server. NodeWarden gains them in
slice 1 (section 10). On NodeWarden, Deskwarden additionally uses the two
things NodeWarden has that official servers lack:

1. **Direct membership for an existing account.**
   - `GET …/members/public-key?email=` answers both whether the account
     exists and its key.
   - `POST /api/organizations/{id}/members/invite` with the key already
     wrapped goes in one call, with no accept and no confirm. It is
     Keeper-like, and it is today's NodeWarden behaviour.
   - The key is pinned at that moment, exactly as a first auto-confirm pins
     it (section 11.5).
2. **`GET /invitations/mine`**, so a recipient can see and accept an
   invitation without an email.

That is "Deskwarden implements both flows" (decision 1), expressed as a
single code path with two branches, not two parallel implementations.

### 7.3 Operation table

| Operation | NodeWarden dialect | Official dialect (Bitwarden, Vaultwarden) |
|---|---|---|
| Create sharing org | `POST /api/organizations` (official body, slice 1) | Vaultwarden: `POST /api/organizations` (9.5). **Official self-hosted: impossible from a client**; adopt a licensed org (section 5.1). |
| Create person collection | `POST /api/organizations/{id}/collections {name, users:[{id: sharerOuId, readOnly:false, hidePasswords:false, manage:true}]}` (NodeWarden learns `users` in slice 1) | same |
| Does E have an account; their key | `GET /api/organizations/{id}/members/public-key?email=E` | not knowable before accept |
| Invite E | `POST …/members/invite {email, role:"member", collections:[{id, readOnly:true, hidePasswords:false, manage:false}], encryptedOrgKey?}` | `POST …/users/invite {emails:[E], type:2, collections:[{id, readOnly:true, hidePasswords:false, manage:false}], groups:[], permissions:null, accessSecretsManager:false}`. Empty 200, so a roster read follows. |
| List people and states | `GET …/users?includeCollections=true` (slice 1; includes invitations) | same (the sharer has ManageUsers) |
| Accept (recipient) | `POST /api/organizations/invitations/accept {token}` | `POST …/users/{ouId}/accept {token}` |
| Recipient key | `GET /api/users/{userId}/public-key` → `publicKey` (slice 1) | same; bulk `POST …/users/public-keys` answers the field as `key` |
| Confirm | `POST …/users/{ouId}/confirm {key}` (slice 1) | same |
| Re-invite | `POST …/users/{ouId}/reinvite` (slice 1) | same |
| Move record in | `PUT /api/ciphers/{id}/share {cipher, collectionIds}` (slice 1) | same |
| Change a record's people | `PUT /api/ciphers/{id}/collections {collectionIds}` (slice 1) | same |
| Move record back | `POST /api/ciphers` (personal) then `DELETE /api/ciphers/{orgId}` | same |
| Remove person | `DELETE …/users/{ouId}` (slice 1; also removes invitations) | same |
| Leave (recipient) | `POST /api/organizations/{id}/leave` (slice 1) | same |

### 7.4 The `bw serve` path

Every bitwarden.com account is on `bw serve`, and so is any self-hosted
account that kept `use_official_bw_crypto`.

- **`bw` cannot invite or accept**, and it cannot create an org (section
  4.6). It can move an item into an org collection (`bw move`) and confirm an
  org member (`bw confirm org-member`). That is half a flow, and the half that
  needs the other half first.
- **On bitwarden.com the model does not fit.** The Free org is 2 seats and 2
  collections: structure A gives one recipient. Families costs money. Creating
  paid orgs from a desktop app is out of scope.

**Decision: sharing is not offered on `bw serve`.**

- The Share control is drawn disabled, with the tooltip `Sharing needs direct
  sync with a self-hosted server`.
- The daemon's `/sharing` routes answer `{"available": false, "reason":
  "bw-serve"}`.
- Recipient-side reading still works: shared records from others are ordinary
  org items to `bw`. The "from" label and the hidden-org rule apply to
  whatever `bw serve` lists, because they read only the org name.

---

## 8. Things the decisions meet in the protocol

These are places where a fixed decision holds only partially on some server.
Each is stated with what this design does about it.

1. **"Waiting for X to create an account" is NodeWarden-only.**
   - Official invites look the same whether or not an account exists: the
     org user has no `userId` until accept (section 4.1).
   - The card says *Waiting for X to accept* there, and on Vaultwarden.
2. **"X wants to share with you — Accept / Decline" is NodeWarden-only as a
   list.**
   - Neither official nor Vaultwarden lets an invitee list invitations.
   - Deskwarden offers a pasted invite link instead. Decline is not an
     official operation: an unaccepted invite just expires.
3. **"Recipients must never see the org or collection names"**
   - This holds *in Deskwarden*.
   - An official app shows the recipient the org name (`Shared by denis@…`)
     and their own collection (their own email). The names are chosen so that
     what is visible is harmless.
4. **"First share creates the org" does not hold on official self-hosted.**
   - An org there exists only through a license bought in the cloud (section
     4.3). Deskwarden cannot create one.
   - It says `Your server needs an organisation for sharing. Create one named
     "Shared by <email>" in the web vault, then try again`, and it adopts an
     org of that name.
   - A Families license caps the org at 6 seats: **five recipients**.
   - Vaultwarden and NodeWarden create freely.
5. **"The invite expires after 7 days"**
   - That is NodeWarden's rule. Official and Vaultwarden tokens last 120
     hours by default, and both are configurable (sections 4.1, 4.5).
   - The card uses the server's own date where it has one (NodeWarden). On
     the others it says "invited N days ago", with no expiry claim, because
     the server's setting is not visible to a client.
6. **"Confirmation is automatic … trusting the server-supplied public key"**
   - Kept, with one guard that decision 2 does not forbid: a key that
     *changed* since a previous confirm for the same email is not auto-trusted
     (section 11.5).
   - The first confirm is fully automatic, as decided.
7. **"Recipients must never see each other" does not hold on an official
   Bitwarden server.**
   - `GET /organizations/{id}/users/mini-details` is open to every member. It
     returns every member's name and email (section 4.1). Any recipient using
     any client can list everyone the sharer shares with.
   - No request Deskwarden makes can prevent it. Only a different structure
     can: one org per (sharer, recipient) pair. Official self-hosted cannot
     afford that anyway, because every org is a license.
   - **Decision needed (Q2).** This design recommends shipping structure A on
     official servers with this one line in the Share dialog: `On this server,
     people you share with can see each other's email addresses.`
   - It holds on NodeWarden (slice 1, section 10.2) and on Vaultwarden
     (Manager+ only, section 4.5).
8. **"Structure A" must keep the sharer assigned to every collection.**
   - An official owner sees only assigned collections in their own sync, and
     a collection nobody manages is refused (section 4.2).
   - So the sharer is a *Manage* member of every person collection (section
     5.2). Recipients never see that assignment: it is in the roster, which
     they cannot read (except official, item 7).

---

## 9. Crypto

### 9.1 Creating the org

- `orgKey` = 64 bytes from `getrandom`: 32 bytes of encryption key and 32 of
  MAC key.
- `key` = `orgKey` RSA-OAEP-SHA1-encrypted to **the sharer's own public
  key**, as a type 4 `EncString` (`4.<b64>`). This is what official clients
  send (section 4.4). The owner then reads it back through
  `profile.organizations[].key` exactly like any other org.
- Org key pair: RSA-2048, e = 65537.
  - `keys.publicKey` = SPKI DER, base64.
  - `keys.encryptedPrivateKey` = PKCS#8 DER encrypted under `orgKey` (type 2).
  - Official's create model requires the pair (section 4.3). Vaultwarden
    and NodeWarden store it.
- The body is in section 9.5.

### 9.2 Collection names

`encrypt(orgKey, email_lowercased)` gives a type 2 `EncString`. Reading a
name back is the existing `organizations::collection_name`.

### 9.3 Moving a record into the org

Given a `DecryptedItem` held by the daemon:

1. **Refuse the move** if any of these holds:
   - the item has attachments and no per-cipher `key`. The attachment keys are
     wrapped under the user key, and this crate does not decrypt attachments;
   - any field failed to decrypt (`DecryptedItem`'s failure record is not
     empty). Re-encrypting it would bury it;
   - the item is in the trash or archived.

   The dialog says which one, in a sentence.
2. **Per-cipher key present** → only the key moves.
   - Unwrap `cipher.key` with the user key and re-wrap it under `orgKey`.
   - The fields and the attachments are untouched: they are under the cipher
     key, which did not change.
   - This is the cheap and safe path.
3. **No per-cipher key** → every field moves.
   - Set `organizationId` in `other`.
   - Run the existing `write::encrypt_item`. It resolves the org key through
     `CipherKeys::for_cipher` and re-encrypts every modelled field and the two
     in-place ones.
   - A per-cipher key is **not** introduced here. That is a behaviour change
     for other clients and belongs in its own design.
4. Send `PUT /api/ciphers/{id}/share`. The id is unchanged.

### 9.4 Moving it back

1. Build the personal body, the inverse of 9.3:
   - with a cipher key, re-wrap the key under the user key;
   - otherwise clear `organizationId` and run `encrypt_item`.
   - Restore `folderId` and `favorite` from `moves`.
   - Strip `collectionIds`, `edit`, `viewPassword` and `permissions`.
2. `POST /api/ciphers` creates it and gives a new id.
3. `DELETE /api/ciphers/{orgId}` removes the org copy permanently. Only the
   owner can do this, which the sharer is.

A failure after step 2 leaves two copies. Section 13 covers the recovery.

### 9.5 Confirming a member

- `key` = `orgKey` RSA-OAEP-SHA1-encrypted to the member's public key (SPKI
  DER, base64 from the server), as type 4.
- New in `rest/crypto.rs`:

```rust
pub fn encrypt_rsa(public_key_spki_der: &[u8], plain: &[u8]) -> Result<EncString, CryptoError>; // RSA-OAEP-SHA1, type 4
pub fn generate_symmetric_key() -> Result<SymmetricKey, CryptoError>;                            // 64 OS-random bytes
pub fn generate_org_key_pair(org: &SymmetricKey) -> Result<(String /*spki b64*/, EncString /*pkcs8 under org*/), CryptoError>;
pub fn fingerprint_phrase(material: &str, public_key_spki_der: &[u8], words: &EffLongList) -> Result<String, CryptoError>;
```

- `encrypt_rsa` uses `rsa::RsaPublicKey::from_public_key_der` and
  `Oaep::new::<sha1::Sha1>()`, with the OS RNG. That RNG is
  `rand_core::OsRng`: `rand_core 0.6` is already in the tree under `rsa`, and
  becomes a direct dependency with its `getrandom` feature. That adds no new
  crate to the lock file.

Org creation body (official shape, which NodeWarden also accepts from slice
1):

```json
{
  "name": "Shared by denis@napps.pw",
  "billingEmail": "denis@napps.pw",
  "planType": 0,
  "key": "4.<orgKey wrapped to the sharer's public key>",
  "keys": { "publicKey": "<spki b64>", "encryptedPrivateKey": "2.<pkcs8 under orgKey>" },
  "collectionName": "2.<first recipient email, encrypted under orgKey>"
}
```

- `collectionName` is the **first recipient's** email, encrypted. The org is
  only ever created during a first share, so that email is known.
  - Where the server creates this collection (Vaultwarden, M), it *is* C(E):
    the mapping is by name, so nothing special-cases it.
  - Where the server ignores the field (NodeWarden today), the next step
    creates C(E).
  - Either way, the next step also sets the sharer's Manage access.
- Vaultwarden ignores `planType` (section 4.5).

### 9.6 Fingerprint phrase

Bitwarden's phrase (section 4.4; the construction is M until a captured vector pins it):

1. `fp = HKDF-Expand(PRK = SHA-256(publicKey DER), info = userId, L = 32, SHA-256)`.
2. Read `fp` as a big-endian integer.
3. Take 5 words by repeated `mod 7776` / `div 7776` into the EFF long list.
4. Join them with `-`.

The recipient's own app shows the same phrase under Settings → Account →
Fingerprint phrase. That is what makes it comparable.

- The EFF long list ships beside the executable as
  `assets/eff_large_wordlist.txt`, as `wordlist.txt` does, and is
  installer-pinned by the same kind of test.
- It is loaded only when **Show fingerprint** is pressed.

---

## 10. NodeWarden server work

All in `E:\Personal\nodewarden`. Every response is Bitwarden-shaped
(`object`, camelCase). Ids are UUIDs.

### 10.1 Official membership routes (slice 1)

NodeWarden keys members by `userId` and invitations by their own id. The
official `ouId` is therefore **the member's `userId`** for members and **the
invitation `id`** for invitations. Both are UUIDs from separate generators,
and every route below accepts either.

```
GET  /api/organizations/{orgId}/users?includeCollections=true      manager+
  200 { object:"list", continuationToken:null, data:[
        { object:"organizationUserUserDetails", id, userId|null, email, name|null,
          type: 0|1|2|3|4,              // owner/admin/user/manager/custom
          status: 0|1|2,                // invitation=0; member no key=1; member with key=2
          collections:[{id, readOnly, hidePasswords, manage}],
          accessAll:false, externalId:null, twoFactorEnabled:false,
          resetPasswordEnrolled:false, usesKeyConnector:false, hasMasterPassword:true,
          invitationExpirationDate: "…"|null }   // NodeWarden extension, invitations only
      ]}

POST /api/organizations/{orgId}/users/invite                         manager+
  { emails:[string], type:number, collections:[{id,readOnly,hidePasswords,manage}],
    groups:[], permissions:null|{}, accessSecretsManager:false }
  200 (empty body, as official). Official semantics: always a status-0 invitation,
  even for an existing account; no direct membership on this route.

POST /api/organizations/{orgId}/users/{ouId}/reinvite                manager+
  200. New token, expiry reset to 7 days.

POST /api/organizations/{orgId}/users/{ouId}/accept                  the invitee
  { token }  200. Same rules as /invitations/accept (email must match).

POST /api/organizations/{orgId}/users/{ouId}/confirm                 manager+
  { key }    200. Sets encryptedOrgKey; member must be status 1 with no key
             (400 "User not accepted" / "Already confirmed" otherwise).
POST /api/organizations/{orgId}/users/confirm                        manager+
  { keys:[{id,key}] }  200 { object:"list", data:[{id, error:""}] }

POST /api/organizations/{orgId}/users/public-keys                    manager+
  { ids:[ouId] }  200 { object:"list", data:[{id, userId, key}] }

DELETE /api/organizations/{orgId}/users/{ouId}                       manager+
  204. Member row or invitation. Owner cannot be removed.

GET  /api/organizations/{orgId}/users/mini-details                 manager+  (official: any member;
  200 list of {id, userId, type, status, name, email}                  deliberately Vaultwarden's rule)

POST /api/organizations/{orgId}/collections                          manager+  (extended)
  { name, externalId?, users?:[{id: ouId, readOnly, hidePasswords, manage}], groups?:[] }
  users[] is applied to those members' access lists in the same request.

POST /api/organizations/{orgId}/leave                                any member but the owner
  200. Deletes the caller's own membership.

GET  /api/users/{userId}/public-key                                  any authenticated user
  200 { object:"userKey", userId, publicKey }
```

The existing custom routes stay (`/members/*`, `/invitations/*`). Deskwarden
still uses `/members/invite` and `/invitations/mine` (section 7.2).
`/members/invite` accepts collection entries as objects as well as strings.

**Pushes:** accept, confirm, invite, remove and leave bump the revision of the
affected member **and of every manager+ of the org**. The sharer's daemon
therefore hears an accept at once and auto-confirms without waiting for a
poll.

### 10.2 Privacy fixes (slice 1)

| Surface | Today | Fix |
|---|---|---|
| `GET /organizations/{id}/members`, `/users`, `/users/mini-details` | any member (`/members`); new routes | manager+ only; plain members get `404`, as the rest of this file does. This is Vaultwarden's rule, and stricter than official (section 4.1). |
| `/api/sync` `collections[]` | every collection of every org | only `getAccessibleCollectionsForUser` |
| `GET /organizations/{id}/collections` | any member | managers: all; members: their accessible ones only |
| `GET …/collections/{cid}/items` | any member | manager+, or a member with access to `cid` |
| cipher `collectionIds` in `/api/sync`, `GET /api/ciphers/{id}`, `/details` | all | intersected with the viewer's accessible collections (managers: unchanged) |
| profile `organizations[].users` | `[]` | unchanged (already empty) |
| account deletion successor | highest-ranked member, even a plain one | only `manager`+ may inherit; with none, the org and its ciphers are deleted, as for an org of one |

### 10.3 Read-only collections (slice 1)

- **Storage.** `organization_members.collections` becomes a JSON array of
  `{id, readOnly, hidePasswords, manage}`. A bare string reads as
  `{id, readOnly:false, hidePasswords:false, manage:false}`, so old rows keep
  their meaning. The same goes for `organization_invitations.collections`. No
  new column, and no `STORAGE_SCHEMA_VERSION` change: the column is already
  JSON text.
- **`resolveCipherAccess` gains a `canEdit` flag.** It is true when the member
  can access all collections (manager+), or holds any of the cipher's
  collections with `readOnly: false`.
- **Writes need `canEdit`.** That covers update, partial, delete (soft and
  hard), restore, archive, attachments (create, upload, delete, metadata),
  `share`, `collections` and bulk move. Without it the answer is
  `403 "You do not have permission to edit this item"`.
- **Creating an org cipher needs a writable collection** in
  `validateOrgCollectionsForCipher`.
- **An update may not change `organizationId`**, as on official:
  `400 "Organization mismatch. Re-sync if you recently moved this item to an
  organization."`. That closes the un-org path in section 3.1. Moving in is
  `/share`, and moving out is create + delete.
- **Cipher responses** say, per viewer:
  - `edit = canEdit`;
  - `viewPassword = !hidePasswords` (the most permissive of the viewer's
    collections that hold it);
  - `permissions = {delete: canEdit, restore: canEdit}`.

  They are computed in `handleSync` and `handleGetCipher`, not stored.
- **Collection name limit**: 100 → 1000 characters (official
  `EncryptedStringLength(1000)`), so encrypted names fit.

### 10.4 `PUT /api/ciphers/{id}/share` (slice 1)

This replaces the stub. `POST` is kept as an alias.

```
PUT /api/ciphers/{id}/share
  { cipher: <CipherRequestModel with organizationId set, fields under the org key
             (or cipher key re-wrapped under it), lastKnownRevisionDate>,
    collectionIds: [string, ...] }                    // non-empty
  200 <cipherDetails>
```

Rules:

- The caller owns the cipher and it is **personal**. An org cipher answers
  `400 "Cipher is already in an organization"`.
- The caller is manager+ in `cipher.organizationId`, or holds a writable
  collection there.
- Every id in `collectionIds` belongs to that org and is writable by the
  caller.
- `lastKnownRevisionDate` must not be stale, by the same rule as update.
- Attachments without a cipher key are refused with `400 "This item has
  attachments that must be re-shared by the client"`. Deskwarden never sends
  one (section 9.3), and an official client re-shares them first.
- Folder and favourite are cleared, as `setOrgCipherFields` already does.
  `userId` is kept as the author, which is NodeWarden's existing model.
- Pushes go to the author and to every member who can now see it.

```
PUT /api/ciphers/{id}/collections          (POST alias; also /collections_v2)
  { collectionIds: [string, ...] }
  200 <cipherDetails>   (v2: { object:"optionalCipherDetails", unavailable:bool, cipher })
```

- The caller has `canEdit` (or is manager+). The ids belong to the cipher's
  org and are writable by the caller.
- It reconciles `collection_items` and pushes to members who gained or lost
  it.

### 10.5 Profile organisations (slice 1)

`organizationForMember` adds:

- `type` (0–4 from `role`);
- `status` (2 when the member holds a key, else 1);
- `userId`;
- `hasPublicAndPrivateKeys` (`!!org.publicKey`);
- `usePolicies: true` and `useTotp` (already present);
- `selfHost: true`.

The two lists split as they do on official servers:

- `organizations` holds members **with a key** (Confirmed);
- `organizationsNew` holds every accepted member, keyed or not.

Today both carry the same list, so an unconfirmed member's official app sees
an org with `key: null` in the list it decrypts. Official clients read `type`
and `status` to decide whether an org is usable.
A test pins the shape against an official `ProfileOrganizationResponseModel`
capture. Org creation accepts `key` and `keys.{publicKey,encryptedPrivateKey}`
as aliases of `encryptedOrgKey`, `publicKey` and `privateKey`.

---

## 11. Deskwarden architecture

### 11.1 Modules

| Module | New or changed | Holds |
|---|---|---|
| `rest/crypto.rs` | changed | `encrypt_rsa`, `generate_symmetric_key`, `generate_org_key_pair`, `fingerprint_phrase` (section 9.5) |
| `rest/sharing_api.rs` | new | `RestClient` methods for every route in section 7.3, typed request and response structs, and the dialect probe. No policy. |
| `rest/sharing.rs` | new | **The sharing engine**: `Sharing::share`, `stop`, `forget`, `reinvite`, `confirm_pass`, `accept`, `decline`, `leave`, `fingerprint`. It builds the read model (`SharingView`) from sync data, the roster and `sharing_state.json`, and moves ciphers (9.3, 9.4) through `write.rs`. It runs in the daemon, inside `RestBackend`'s lock discipline (one `Session` borrow per call). |
| `rest/organizations.rs` | changed | `Member` gains `ou_id`, `status: MemberStatus {Invited, Accepted, Confirmed}` (the existing `confirmed` becomes derived) and `collections` with flags. The roster route is `/users?includeCollections=true` first, then `/members` on 404, so one roster serves both dialects. Sharing orgs are marked (`Organisation::sharing: Option<SharingOrg {sharer_email, mine}>`), and `visible_organisations()` leaves them out (11.6). `profile.organizationsNew` is read too, for accepted-but-unconfirmed orgs. |
| `sharing_state.rs` | new | `sharing_state.json` (section 5.4): load, persist and atomic update, in `sent_history.rs`'s own shape. |
| `vault_backend.rs` | changed | `fn sharing(&self) -> Option<&dyn SharingBackend> { None }`. `RestBackend` returns its engine. `bw serve`, disk-cache and daemon backends forward or return `None`. |
| `vault_api.rs` | changed | The `/sharing` routes (11.3). |
| `change_feed.rs` | changed | `Change::Sharing` (11.4). |
| `vault_host.rs` | changed | The hub worker calls `confirm_pass` after an applied batch or populate. A sharing timer runs in the same worker thread (11.5). |
| `daemon_link.rs` | changed | `SharingClient`: typed calls to the `/sharing` routes for the window. |
| `vault_bridge.rs` | changed | `may_edit(item)` (`other.edit != false`), `shared_from(item, &Directory)`. |
| `vault_window/share_ui.rs` | new | The Share dialog: one email field and `sent_history` suggestions, with the same ✕ and the same forget question as the Send composer. |
| `vault_window/detail.rs` | changed | The Share control beside the envelope. SHARING card person rows under the Active Send row. The view-only header line and disabled edit controls when `!may_edit`. |
| `vault_window/item_list.rs` | changed | The `shared` pill when any person is *Shared*. A `from denis@…` subtitle for received records. |
| `vault_window/sidebar.rs` | changed | Sent and Shared with me counts include records. The ORGANISATIONS section skips sharing orgs. Pending invitation rows. |
| `sent_history.rs` | changed | `SentRecord.kind: Send | Share` (default `Send`, so old files read unchanged). People from either kind feed the suggestions. `forget` returns both. |

### 11.2 Why the engine lives in `RestBackend`

- Every sharing step needs the session, the vault keys and the private key
  (to read `profile.organizations[].key` and to re-wrap). `RestBackend` holds
  these, and nothing else should.
- The engine returns the items it changed, and `VaultCache` applies them the
  way it applies any write (write-through). The daemon's snapshot therefore
  stays the single truth.

### 11.3 Daemon API routes

Bodies are JSON with the `{"success":…}` envelope, as the other routes use.
Nothing personal goes in a query string.

```
GET  /sharing
  200 { success:true, data:{
        available: bool, reason: null|"bw-serve"|"server-too-old"|"needs-org"|"not-loaded",
        dialect: "nodewarden"|"official",
        rosterVisibleToRecipients: bool,     // true on an official server (section 8.7)
        people: [{ email, state: "shared"|"waitingAccept"|"waitingAccount"|"waitingSignIn"|"expired"|"keyChanged"|"gone",
                   expiresAt: string|null, invitedAt: string|null }],
        records: [{ itemId, people: [email] }],             // sharer side
        received: [{ itemId, from: email }],                // recipient side
        incoming: [{ id, from: email, expiresAt: string|null }]   // pending invitations
      }}
  Answered from the daemon's held view; no server round trip.

POST /sharing/item/{id}            { email }  -> 200 { data: <record entry>, item: <item> }
POST /sharing/item/{id}/stop       { email }  -> 200 { data: <record entry>|null, item: <item>, movedTo: id|null }
POST /sharing/person/reinvite      { email }  -> 200
POST /sharing/person/forget        { email }  -> 200 { stopped: [itemId] }
POST /sharing/person/confirm       { email }  -> 200          // "Share anyway" after a key change
POST /sharing/person/fingerprint   { email }  -> 200 { data: { phrase } }
POST /sharing/incoming/{id}/accept            -> 200
POST /sharing/incoming/{id}/decline           -> 200          // NodeWarden only; 501 otherwise
POST /sharing/incoming/paste       { link }   -> 200 { data: <incoming entry> }   // official invite link
POST /sharing/from/{orgId}/leave              -> 200
```

**Errors** map through the existing `failed()`:

- `Unsupported` → 501 with `why`;
- a refused step → 400 with the sentence to show;
- the vault not loaded yet → 503.

### 11.4 Change feed

- A new variant `Change::Sharing`, on the wire as `{"kind":"sharing"}`,
  means *the sharing view changed; re-read `GET /sharing`*. It is published
  after every engine write and after every `confirm_pass` that changed
  anything.
- Item movements use the existing kinds. A share or collection change is
  `Item{id}`. A move back is `ItemGone{old}` then `Item{new}`, in one batch.
- **Today an unknown kind fails the whole page.** `page_from_body`
  deserialises into a closed `WireChange` enum. A failed page is a failed
  poll, and three in a row close the window.
  - That is safe today: a window and its daemon are always one build, because
    an update restarts the daemon and so ends every grant.
  - A browser extension would not be one build with the daemon. Slice 2 adds
    a `#[serde(other)] Unknown` arm that is ignored, and a test that a page
    with an unknown kind still parses.

### 11.5 Auto-confirm

This is `Sharing::confirm_pass`, run by the daemon only.

**Triggers:**

1. after every whole read the daemon makes (unlock, `POST /sync`, the hub's
   populate);
2. after every applied hub batch. On NodeWarden an accept now pushes to the
   sharer (section 10.1);
3. after the engine's own share, re-invite or accept;
4. a **sharing timer** every 10 minutes while the vault is unlocked **and**
   the account has a sharing org with an Invited or Accepted person. Official
   servers push nothing on accept, so this is the only trigger there. Only the
   timer costs a request of its own.

**Rule, per org user of the sharer's own sharing org:**

1. Skip unless it is Accepted: official `status 1`, or NodeWarden `status 1`
   with no key.
2. Skip unless its email has a person collection C(E) in this org.
   - The name decrypts under the org key, so the server alone cannot invent
     it (section 5.2).
   - A server cannot slip a stranger into the confirm queue. It would need a
     recipient's help, *and* a server that lets members create collections.
   - That second condition never holds on NodeWarden or Vaultwarden (Q7).
3. Fetch the public key: `GET /api/users/{userId}/public-key`, or
   `POST …/users/public-keys` for several at once.
4. Compute `sha256(spki)`.
   - **No pin for E**: pin it and confirm.
   - **Same pin**: confirm.
   - **Different pin**: do not confirm. E becomes `keyChanged`, and the card
     offers **Show fingerprint** and **Share anyway** (`POST
     /sharing/person/confirm`).
5. `POST …/users/{ouId}/confirm {key: encrypt_rsa(spki, orgKey)}`.
6. Publish `Change::Sharing`.

**Cost:**

- Triggers 1–3 reuse the roster the directory already fetched on that read,
  so they cost nothing extra.
- A confirm is 2 requests per newly accepted person.
- The timer is 1 request per 10 minutes, and only while someone is waiting.

**The daemon, not the window.** It runs whether or not a window is open. That
is "whenever it runs" in decision 2. A sharer with Deskwarden installed on two
PCs may race two confirms. The second gets `400 already confirmed`, which is
treated as success.

### 11.6 Sharing orgs and the existing directory

- `Directory::organisations()` keeps returning all orgs, because `audience_of`
  must still resolve a shared item.
- A new `Directory::visible_organisations()` omits sharing orgs, and the rail
  uses it.
- `Audience::Shared` for an item in a sharing org becomes a new variant
  `Audience::SharedWithPeople { people }` on the sharer's side, and
  `Audience::SharedWithMe { from }` on the recipient's. The "Also visible to
  14 people in Engineering" copy never mentions a sharing org.

### 11.7 Suggestions and Forget

- A share records `SentRecord { kind: Share, recipient, item_id, name,
  fields: [] }`. The Share dialog therefore suggests everyone ever sent *or*
  shared to, with the same ✕.
- `forget` asks the existing question with records and Sends in one list.
  The answer drives `POST /sharing/person/forget` as well as deleting the
  Sends.

---

## 12. Security

### 12.1 What trusting the server's public key means

- Auto-confirm wraps the org key to whatever public key the server returns for
  E.
- A malicious or compromised server can return **its own** key for E. It then
  unwraps the org key and decrypts **every record in the sharing org**, which
  under structure A means every record the sharer shares with anyone. The org
  key is shared across all people; that is the cost of "no copies".
- With manual confirmation and a compared fingerprint, the attack fails.
  Decision 2 accepts the risk for convenience, as Keeper does.

What this design adds without adding a prompt:

- **Pins**: a key swap for someone *already* confirmed is caught (`keyChanged`).
  Only the first confirm is blind.
- **Collection-name authentication**: the server cannot get a confirm for an
  email the sharer never shared with, because C(E) must decrypt under the org
  key.
- **Show fingerprint** on every person row, for anyone who wants to check.

### 12.2 A hostile server under structure A, beyond key substitution

- **It can grant any org user access to more collections.** Collection access
  is enforced by the server, not by cryptography: every member holds the one
  org key. Recipient A could then read records shared only with B.
  Per-recipient isolation is a server promise. The alternative, one org per
  (sharer, recipient) pair, costs an org, an invite and an accept per person,
  and NodeWarden's 5-owned-orgs limit alone rules it out.
- **It learns the sharing graph**: who shares which record ids with whom. It
  also learns the sharer's email from the org name. It already knows both
  from the membership table, so the name adds nothing.
- **It cannot read a record** without an org key. It cannot forge a record
  either: every field is MAC'd under a key it does not hold.
- **It can drop or withhold shares** (denial), and it can show a recipient an
  org named `Shared by boss@company.com` that boss@ never created. The "from"
  label is the org's claim.
  - On NodeWarden, an **existing** user is added without consent (section
    6.9). Any account holder can therefore make items appear in anyone's
    vault, labelled with any email they own.
  - View-only limits the damage: nothing autofills into a site by itself, but
    a record for `bank.com` with a look-alike URL is a phishing aid. Q1 asks
    whether NodeWarden's direct add should require acceptance.

### 12.3 What view-only relies on

- **The server enforcing `readOnly`.** Official and Vaultwarden do. NodeWarden
  does from slice 1 (section 10.3). Before slice 1, a NodeWarden recipient
  could edit, delete, un-org or create items in the sharer's org.
  **Sharing is not switched on in Deskwarden against a NodeWarden without the
  slice 1 routes.** The second probe checks for them (section 7.1).
- **Not cryptography.** The recipient holds the org key and can read every
  field of what they can reach. `hidePasswords` would only hide it in
  well-behaved apps, and autofill needs it, so it stays false.
- **Deskwarden's own UI rule** (`may_edit`) only stops accidental edits.

### 12.4 Other exposures

- **`/members/public-key?email=`** tells any org owner whether an email has an
  account on the server. Anyone can create an org, so that means anyone.
  Deskwarden needs this for the direct add. The invite route already leaks the
  same fact through its response shape. It is recorded rather than fixed
  (Q4).
- **`sharing_state.json`** holds emails and key hashes, no secrets.
  `sent_history.json` already holds the same class of data, and `forget`
  removes a person from both.
- **An official server shows every recipient the whole roster** through
  `mini-details` (section 8.7). That is the honest server's behaviour, not an
  attack. The Share dialog says so on that dialect.
- **The daemon API** adds no new credential. `/sharing` sits behind the same
  per-client grant and era rule as every other route.

---

## 13. Error handling

| Failure | Where | What the user sees | State left |
|---|---|---|---|
| Record can't move (attachments without a cipher key, undecryptable field, trashed) | before any call | Dialog sentence: `This record has attachments. Share it from the Bitwarden web vault.` and so on | nothing |
| No org can be created (official self-hosted), or `ORG_CREATION_USERS` forbids it (Vaultwarden) | 3.1 | `Your server needs an organisation for sharing…` (section 8.4) | nothing |
| Seat limit (a Families license: 6) | 3.3 | `Your organisation has no free seats. Remove someone with Forget, or add seats.` | org, collection |
| Org limit (NodeWarden: 5 owned) | 3.1 | `You own too many organisations on this server to start sharing.` | nothing |
| Collection create fails | 3.2 | `Couldn't prepare sharing with X. Try again.` | org may exist (reused next time) |
| Invite refused (already a member, bad email, 403) | 3.3 | The server's sentence. "Already a member" is treated as success. | org, collection |
| Share (`PUT /share`) fails | 3.4 | `Couldn't share the record. It's still in your vault.` with **Retry** | person invited, record untouched |
| Stale revision on `/share` | 3.4 | Silent: re-read the one item and retry once, then fail as above | as above |
| Stop sharing: `/collections` fails | stop | `Couldn't stop sharing. X can still see it.` with **Retry** | unchanged |
| Move back: create fails | 9.4 step 2 | `Couldn't move the record back. It's still shared with nobody.` | org copy filed nowhere. Next pass: retry the move |
| Move back: delete fails after create | 9.4 step 3 | nothing; logged | `unfinished: purgeOrgCopy`, retried every pass; the org copy is invisible to recipients (no collections) |
| Confirm fails (key missing: the user never logged in to an app that made keys) | confirm pass | Row: `Waiting for X to sign in once to a Bitwarden app` | retried every pass |
| Key changed | confirm pass | `keyChanged` row (section 6.1) | not confirmed |
| Server unreachable / 401 | any | Existing `VaultError` handling: the daemon hands 401 to sign-in | unchanged |
| Two PCs race | any | none: "already confirmed", "already in collection" and 404 on a delete of something gone count as success | converges |

Every engine write is **idempotent by construction**. It reads current server
state first (org exists? collection exists? member exists? item already in
collection?) and does only the missing steps. **Retry** is therefore the same
call.

---

## 14. Testing

### 14.1 Deskwarden: request-counting mock-server tests

These use the codebase's `mockito` style, as in `vault_api.rs` tests
(`a_daemon()`) and `vault_host.rs` (`a_launch_with_a_window_…`). The real
`RestBackend`, `VaultApi` and listener run against a mock server, and each
test asserts **the exact requests made**.

- `a_first_share_creates_the_org_the_collection_the_invite_and_moves_once`:
  `POST /organizations` ×1, `POST …/collections` ×1, invite ×1, `PUT
  /ciphers/{id}/share` ×1, **no** `/api/sync`.
- `a_share_to_an_existing_person_is_one_collections_call`.
- `a_share_that_fails_at_the_move_leaves_the_record_personal`: the mock
  answers 500 on `/share`; the item in the daemon's cache is unchanged.
- `stopping_the_last_person_creates_then_deletes_and_rekeys_local_files`.
- `a_half_finished_move_back_is_finished_on_the_next_pass`.
- `an_accepted_person_is_confirmed_by_the_daemon_with_no_window`: this runs
  through `HubWorker`, and a push triggers it.
- `an_accept_on_an_official_server_is_confirmed_by_the_timer_not_a_sync`.
- `a_changed_public_key_is_not_confirmed`.
- `a_member_whose_email_has_no_collection_is_never_confirmed`: a
  server-injected accept.
- `the_confirm_pass_reuses_the_roster_the_sync_fetched`: zero extra roster
  requests after a whole read.
- `the_dialect_probe_is_made_once_per_session`.
- `bw_serve_answers_sharing_unavailable_and_makes_no_request`.
- `every_person_collection_is_created_with_the_sharer_as_manager`.
- `an_official_invite_is_followed_by_exactly_one_roster_read`.
- `an_accepted_org_in_organizations_new_draws_waiting_to_finish`.
- `an_old_nodewarden_without_slice_one_turns_sharing_off`.
- `the_first_share_adopts_a_collection_the_server_made_from_collection_name`.
- `a_window_learns_of_a_share_through_the_feed_and_reads_nothing`.
- `sharing_orgs_are_absent_from_the_organisations_rail`.
- `the_roster_reads_users_then_falls_back_to_members`.

### 14.2 Crypto round trips

- `encrypt_rsa` → `decrypt_rsa` with the existing OpenSSL-generated PKCS#8
  fixture key. Also `encrypt_rsa` against the fixture's public half, decrypted
  by **OpenSSL** (a recorded vector, as `rsa_oaep_sha1_opens_a_ciphertext_openssl_produced` does).
- `generate_org_key_pair`: the private key decrypts under the org key and
  opens what the public key wraps.
- Share and move-back round trip on the item: user key → org key → user key.
  Every modelled field, `other` untouched, and
  `an_unmodelled_field_survives_…` extended to the move. Both variants run:
  with a cipher key (only `key` changes) and without one.
- `fingerprint_phrase` against a vector captured from a real account:
  userId, public key and the phrase the web vault shows. It is a one-off
  manual capture, recorded like the crypto module's existing
  still-unpinned list until captured.
- The collection name round trip.

### 14.3 NodeWarden

These use the `node:test` files under `scripts/`, pure where possible, as
`organizations.test.ts` is today.

- **Access**
  - `canEdit` truth table: role × collection flags.
  - A read-only member is refused on each write route (update, delete, hard
    delete, restore, share, collections, attachments), and allowed to read.
  - An update changing `organizationId` is refused.
- **Privacy**
  - A plain member gets 404 on `/members` and `/users`.
  - Sync `collections[]` holds only accessible ones.
  - Cipher `collectionIds` are intersected per viewer.
- **Share**
  - A personal cipher moves with the id kept.
  - An org cipher is refused.
  - Foreign collection ids are refused.
  - A stale revision is refused.
  - Attachments without a key are refused.
- **Membership**
  - invite → accept → confirm on the official routes: statuses 0 → 1 → 2 as
    `/users` reports them.
  - Confirm before accept is refused.
  - Reinvite rotates the token.
  - Invitations appear in `/users` with `userId: null`.
- **Pushes**: accept bumps the managers' revisions.
- **Collections**: create with `users[]` grants that access; `mini-details`
  is manager-only.
- **Successor**: a sharer's account deletion does not promote a plain member.
- **Profile**: `type` and `status` are present and numeric, and `key` is the
  member's wrapped key (the existing test extended).
- **Limits**: a 600-character encrypted collection name is accepted.

### 14.4 One manual end-to-end per server

Share from Deskwarden, then read it in the **official** desktop app as the
recipient, on:

- NodeWarden (dev);
- Vaultwarden (docker);
- official self-hosted, with a license, if one is available (Q5).

Check that the recipient's app shows the record, cannot edit it, and shows
only the sharing org and its own collection.

---

## 15. Rollout

Each slice ships on its own and leaves every client working.

1. **NodeWarden: official routes, privacy and view-only** (section 10).
   - Deskwarden unchanged.
   - Its roster read gets a fallback `/users` → `/members`, and it stops
     counting a status-1-no-key member as confirmed. Those two lines can ship
     with slice 2.
   - Ships alone because it fixes the leaks in section 3.1 whether or not
     anyone shares.
2. **Owner share and stop, existing NodeWarden users.**
   - Crypto additions, the engine for create org, collection, direct add,
     `/share`, `/collections` and move back.
   - `/sharing` routes, the feed kind, the Share dialog, person rows
     (*Shared* only) and the pill.
   - Enabled only when the probe says *NodeWarden with `/users`*.
3. **Pending, official dialect and auto-confirm.**
   - Invitations for non-users, the waiting and expiry rows, and Re-invite.
   - The official invite/accept/confirm routes, the dialect probe, the confirm
     pass with pins and the timer, and Show fingerprint (EFF list asset).
   - The adopt-a-licensed-org path and the roster disclosure line on official
     servers, and Forget.
   - Vaultwarden and official self-hosted become supported here. Vaultwarden
     without mail is the easy case: no accept step exists there.
4. **Recipient side polish.**
   - "from …" labels, Shared with me records, and incoming invitation rows
     (NodeWarden list, official pasted link).
   - Reading `profile.organizationsNew`, for the *waiting to finish* row.
   - Leave, the view-only header and `may_edit` gating, and sharing orgs
     hidden in every filter.
   - This can ship before 3 for NodeWarden recipients, because a slice 2
     share already reaches them.

---

## 16. Open questions for the owner

Each has a recommendation. None blocks slice 1.

- **Q1. Consent on NodeWarden.**
  - Direct membership adds records to an existing account's vault with no
    accept. Any account holder can do it to any other, under any "from" email
    they own (section 12.2).
  - Keep it (Keeper-like, and NodeWarden's registration is invite-only, so
    every account is known to its admin)? Or make NodeWarden's direct add
    require an accept, shown in Deskwarden as `X wants to share with you`?
  - *Recommendation: keep it for now; revisit if the server ever opens
    registration.*
- **Q2. Official Bitwarden servers expose the roster to recipients**
  (section 8.7).
  - Ship there with the disclosure line in the Share dialog, or offer sharing
    only on NodeWarden and Vaultwarden?
  - *Recommendation: ship with the line.*
- **Q3. People with no NodeWarden account need an admin's invite code to
  register.** Three options:
  - (a) Deskwarden says so and stops there;
  - (b) when the sharer is the server's admin, Deskwarden also mints a
    registration code (`/api/admin/invites`) and shows it to pass along;
  - (c) NodeWarden treats an unexpired org invitation as permission to
    register that email. Any org owner could then admit people, so it would
    sit behind an admin switch that defaults to off.

  *Recommendation: (a) + (b).*
- **Q4.** `/members/public-key?email=` tells any org owner whether an address
  has an account (section 12.4). Accept it, rate-limit it, or remove it?
  Removing it would end the direct add.
  - *Recommendation: rate-limit.*
- **Q5.** Is a Families or Enterprise license available for a manual
  end-to-end test on official self-hosted?
  - If not, official self-hosted ships as "built to the source, not run
    against a live server", and says so in the release notes.
- **Q6.** Should a recipient be told when a record stops being shared with
  them? That needs a local memory of received ids.
  - *Recommendation: not in v1.*
- **Q7.** On official servers, whether members may create collections is an
  org setting. Deskwarden has no confident route to set it (section 5.2).
  - Ask the sharer to check "limit collection creation to owners and admins"
    in the adopt message?
  - *Recommendation: yes.*
- **Q8.** A recipient cannot bind a desktop app to a received record, because
  app matches live in the item and the item is read-only.
  - Keep per-recipient app matches in a local file (its own design), or leave
    it for v1, where website autofill still works?
  - *Recommendation: leave it for v1.*
- **Q9.** Moving a record back to the personal vault changes its id (create,
  then delete; there is no official route). Local files are re-keyed.
  - Acceptable, or add a NodeWarden-only `unshare` route that keeps the id,
    at the cost of a second code path?
  - *Recommendation: accept the id change.*
- **Q10.** When a person's last shared record is stopped, their membership and
  empty collection are kept, so the next share is instant (section 5.2). Their
  official apps keep showing an empty `Shared by denis@…` org until **Forget**.
  - Acceptable, or remove the membership then, and re-invite next time?
  - *Recommendation: keep.*
- **Q11.** Records with attachments and no per-cipher key are refused
  (section 9.3). Is that acceptable for v1, rather than implementing
  attachment re-keying?
  - *Recommendation: yes.*

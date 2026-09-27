# Personal Sharing, Slice 1: NodeWarden Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give NodeWarden the server half of personal sharing (spec slice 1): Bitwarden's official membership routes, read-only collections that the server enforces, a real `PUT /api/ciphers/{id}/share` and `/collections`, per-viewer privacy, profile organisations official clients can use, and the owner-decided server items (rate-limited email lookup, direct add kept, admin-minted registration code, 1000-character collection names, no plain-member successor).

**Architecture:** Collection access becomes `{id, readOnly, hidePasswords, manage}` entries inside the existing JSON column, read through one pure module (`org-collection-access.ts`). Every per-viewer answer (`edit`, `viewPassword`, `collectionIds`, may-write) is computed from those entries by one function and applied in `resolveCipherAccess`, `/api/sync` and the cipher response. The official `/organizations/{orgId}/users/...` routes live in a new handler file on top of NodeWarden's existing member and invitation tables, where an official `organizationUserId` is a member's user id or an invitation's id. Tests drive the real router against the real runtime schema on `node:sqlite`, through a small D1 adapter.

**Tech Stack:** TypeScript, Cloudflare Workers, D1 (SQLite), `node:test` via `tsx --test`, pnpm.

**Spec:** `E:\Personal\node-bitwarden\docs\superpowers\specs\2026-09-27-personal-sharing-design.md`, sections 3.1, 8, 10, 12, 14.3, 15, plus the "Owner's decisions after review" block, which overrides the rest.

## Global Constraints

- **Repo:** `E:\Personal\nodewarden`, branch `feature/missing-features`. Every path below is relative to that repo. Run every command from its root.
- **pnpm only.** Never `npm` or `yarn`. Typecheck: `pnpm exec tsc --noEmit` (clean means no output and exit 0). It checks `src/` and `shared/` only; test files are not typechecked.
- **Tests:** `node:test` files in `scripts/`, run by `tsx --test`. Every sharing test file is `scripts/sharing-*.test.ts` and runs through `pnpm run test:sharing` (added in Task 1), which preloads `./scripts/cloudflare-workers-stub.mjs`.
- **No deploy, no push.** No task runs `pnpm run deploy`, `wrangler deploy`, `wrangler d1 ... --remote`, or `git push`.
- **One commit per task**, with explicit paths (never `git add -A`, never `--amend`). Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The commands below pass it as a second `-m`, which works in Git Bash and PowerShell alike.
- **LF line endings.** The files touched here are LF; keep them LF.
- **Do not touch `webapp/`.** NodeWarden's web vault must keep working unchanged (see Task 3 for the one rule that bends the spec to keep it working).
- **No schema change.** No new column, no new table, and `STORAGE_SCHEMA_VERSION` stays `2026-09-08-attachment-user-id` (spec 10.3: "the column is already JSON text").
- **Collection access entries** are `{id, readOnly, hidePasswords, manage}`. A bare string id reads as `{id, readOnly:false, hidePasswords:false, manage:false}`, so old rows keep their meaning. Rows are written back as objects.
- **`GET /api/organizations/{id}/members` keeps answering collection ids as strings.** NodeWarden's web vault and Deskwarden's `rest/organizations.rs` both read strings there. Flags are only on the official `/users` route.
- **Bitwarden shapes:** every response has `object` and camelCase keys; ids are UUIDs. Route patterns only match hex ids, so every id a test puts in a URL is a UUID.
- **An official `organizationUserId` (ouId)** is the member's `userId` for a member and the invitation's `id` for an invitation (spec 10.1). Every `/users/{ouId}` route accepts either.
- **Manager-only routes answer a plain member `404 "Not found"`**, as the rest of `src/handlers/organizations.ts` does.
- **Organization invitations last 7 days.**
- **Collection names:** at most **1000** characters (official `EncryptedStringLength(1000)`).
- **Exact messages** (clients and tests match them):
  - `403 "You do not have permission to edit this item"`
  - `400 "Organization mismatch. Re-sync if you recently moved this item to an organization."`
  - `400 "Cipher is already in an organization"`
  - `400 "This item has attachments that must be re-shared by the client"`
  - confirm: `400 "User not accepted"` / `400 "Already confirmed"`
- **Pushes (spec 10.1):** accept, confirm, invite, remove and leave bump the revision of the affected member **and of every manager+** of the organization. In tests a push is observed as the user's `user_revisions.revision_date` moving off `2000-01-01T00:00:00.000Z`.
- **NodeWarden's custom routes stay** (`/members/*`, `/invitations/*`), including the direct add for an existing account (owner decision).

## File Structure

| File | New? | Responsibility |
|---|---|---|
| `src/services/org-collection-access.ts` | new | Pure: parse and normalise collection access entries, merge updates, and compute one viewer's access to one cipher (`canSee`, `canEdit`, `viewPassword`, visible collection ids). No database, no pushes. |
| `src/services/org-user-model.ts` | new | Pure mappings between NodeWarden rows and Bitwarden's `OrganizationUserType` / `OrganizationUserStatusType`, invitation token and expiry, and `resolveOrgUser` (ouId to member or invitation). Imports only the repo, so `profile-response.ts` can use it without pulling in the notifications hub. |
| `src/handlers/organization-users.ts` | new | The official `/api/organizations/{orgId}/users/...` routes, `/leave`, `GET /api/users/{id}/public-key`, and the admin-only registration code. |
| `src/handlers/cipher-sharing.ts` | new | `PUT/POST /api/ciphers/{id}/share` and `PUT/POST /api/ciphers/{id}/collections` (and `_v2`). |
| `src/types/index.ts` | changed | `CollectionAccess`; member and invitation `collections` become `CollectionAccess[]`. |
| `src/services/storage-organization-repo.ts` | changed | Row mappers normalise access entries; `getOrganizationManagerIds`; account deletion never promotes a plain member. |
| `src/services/storage-org-access.ts` | changed | `resolveCipherAccess` gains `canEdit`, `viewPassword`, `visibleCollectionIds`; `resolveEditableCipher`; creating into a collection needs write access. |
| `src/handlers/organizations.ts` | changed | Privacy gates, collection `users[]` and name length, email-lookup budget, org-create aliases, shared accept/remove/revoke helpers, manager pushes. |
| `src/handlers/ciphers.ts` | changed | Per-viewer fields in `cipherToResponse`; `mergeCipherUpdateBody` extracted from update; write routes need `canEdit`; the organization-change rule. |
| `src/handlers/attachments.ts` | changed | Attachment writes need `canEdit`. |
| `src/handlers/org-ciphers.ts` | changed | `notifyCipherAudience`: push only to members who can see a cipher. |
| `src/handlers/sync.ts` | changed | Per-viewer cipher fields; only reachable collections. |
| `src/handlers/scim.ts` | changed | Reads and writes access entries. |
| `src/handlers/admin.ts` | changed | Exports its re-auth and invite helpers for the registration-code route. |
| `src/utils/profile-response.ts` | changed | Numeric `type`/`status` and the official fields; `organizations` (Confirmed) vs `organizationsNew` (Accepted + Confirmed). |
| `src/config/limits.ts` | changed | The email-lookup budget. |
| `src/router-authenticated.ts` | changed | Wires every new route. |
| `scripts/support/sharing-harness.ts` | new | Real schema on `node:sqlite` behind a D1 adapter, seed helpers, and `call()` through `handleAuthenticatedRoute`. |
| `scripts/sharing-*.test.ts` | new | One file per task. |
| `scripts/organizations.test.ts` | changed | One expectation follows the new stored shape. |
| `package.json` | changed | `test:sharing`. |

## Why the harness looks the way it does

Handlers call D1, the Cache API, the notifications Durable Object and `cloudflare:workers`. The harness runs the **runtime schema** (`SCHEMA_STATEMENTS`) on an in-memory `node:sqlite` database and wraps it in the part of D1 the repos use (`prepare`, `bind`, `first`, `all`, `run`, `batch`, `exec`). The Durable Object namespace is a stub whose `fetch` succeeds, because `notifyUserUpdate` catches its own failures and a push is observable as the revision date `bumpUserRevisions` writes to `user_revisions`. The Cache API is a no-op, which also disables the `/api/sync` response cache. This was prototyped against this branch before the plan was written: the router loads under `tsx --test --import ./scripts/cloudflare-workers-stub.mjs` and serves `POST /api/organizations` and `GET /api/sync` from the fake. Loading the router takes about five seconds per test file.

---

### Task 1: Collection access entries, and the test harness

**Files:**
- Create: `src/services/org-collection-access.ts`
- Create: `scripts/support/sharing-harness.ts`
- Create: `scripts/sharing-collection-access.test.ts`
- Modify: `src/types/index.ts` (`OrganizationMember`, `OrganizationInvitation`, new `CollectionAccess`)
- Modify: `src/services/storage-organization-repo.ts` (row mappers, `getAccessibleCollectionsForUser`, `deleteCollection`)
- Modify: `src/services/storage-org-access.ts` (one membership test in `resolveCipherAccess`)
- Modify: `src/handlers/organizations.ts` (`toMemberResponse`, `toInvitationResponse`, `/members/invite`, `PUT /members/:userId`)
- Modify: `src/handlers/scim.ts` (`groupMemberExternalIds`, `applyGroupMembers`)
- Modify: `scripts/organizations.test.ts` (one expectation)
- Modify: `package.json` (`test:sharing`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `src/types/index.ts`: `interface CollectionAccess { id: string; readOnly: boolean; hidePasswords: boolean; manage: boolean }`; `OrganizationMember.collections: CollectionAccess[]`; `OrganizationInvitation.collections: CollectionAccess[]`.
  - `src/services/org-collection-access.ts`:
    - `normalizeCollectionAccess(entry: unknown): CollectionAccess | null`
    - `normalizeCollectionAccessList(value: unknown): CollectionAccess[]`
    - `collectionAccessIds(list: readonly CollectionAccess[]): string[]`
    - `writableCollectionIds(list: readonly CollectionAccess[]): string[]`
    - `applyCollectionAccessUpdate(existing: readonly CollectionAccess[], incoming: unknown): CollectionAccess[]`
    - `withCollectionAccess(list: readonly CollectionAccess[], access: CollectionAccess): CollectionAccess[]`
  - `scripts/support/sharing-harness.ts`: `createHarness()`, `seedUser`, `seedOrg`, `seedMember`, `seedCollection`, `seedCipher`, `seedAttachment`, `seedInvitation`, `call`, `revisionOf`, `wasBumped`, `resetRevisions`, `memberCollectionsRaw`, `invitationCollectionsRaw`, `cipherData`, `count`, `fullAccess`, `readOnlyAccess`, `emailOf`, and the id constants `U`, `ORG`, `ORG2`, `COL_A`, `COL_B`, `COL_C`, `COL_X`, `CIPHER`, `PERSONAL`, `ALICE_PERSONAL`, `INV_CAROL`, `ATT`, `ENC`, `T0`, `OLD_REVISION`. Every later task uses these exact names.

- [ ] **Step 1: Write the harness**

Create `scripts/support/sharing-harness.ts`:

```ts
// The sharing tests drive the real router against a real SQLite database: the
// runtime schema from src/services/storage-schema.ts, applied to node:sqlite and
// wrapped in the slice of D1's API the repos use. Three things are stubbed:
//
// - the notifications Durable Object namespace: its fetch succeeds and records
//   nothing. A push is observed instead as the revision date that
//   bumpUserRevisions writes to user_revisions, which the database keeps;
// - the Cache API: a no-op, which also turns off the /api/sync response cache;
// - `cloudflare:workers`: preloaded by ./scripts/cloudflare-workers-stub.mjs.
//
// Route patterns only match hex ids, so every id a test puts in a URL is a UUID.
import { DatabaseSync } from 'node:sqlite';
import { SCHEMA_STATEMENTS } from '../../src/services/storage-schema';
import { handleAuthenticatedRoute } from '../../src/router-authenticated';
import { getUserById } from '../../src/services/storage-user-repo';
import type { Env } from '../../src/types';

/** A well-formed type-2 EncString. The server never decrypts, it only shape-checks. */
export const ENC = '2.aXY=|Y3Q=|bWFj';
export const T0 = '2026-01-01T00:00:00.000Z';
/** Every seeded user starts at this revision; a push moves it. */
export const OLD_REVISION = '2000-01-01T00:00:00.000Z';

export const U = {
  owner: 'aaaaaaaa-0000-4000-8000-000000000001',
  alice: 'aaaaaaaa-0000-4000-8000-000000000002',
  bob: 'aaaaaaaa-0000-4000-8000-000000000003',
  carol: 'aaaaaaaa-0000-4000-8000-000000000004',
  dave: 'aaaaaaaa-0000-4000-8000-000000000005',
  erin: 'aaaaaaaa-0000-4000-8000-000000000006',
} as const;
export type Person = keyof typeof U;
export const emailOf = (person: Person): string => `${person}@example.test`;

export const ORG = 'bbbbbbbb-0000-4000-8000-000000000001';
export const ORG2 = 'bbbbbbbb-0000-4000-8000-000000000002';
export const COL_A = 'cccccccc-0000-4000-8000-00000000000a';
export const COL_B = 'cccccccc-0000-4000-8000-00000000000b';
export const COL_C = 'cccccccc-0000-4000-8000-00000000000c';
export const COL_X = 'cccccccc-0000-4000-8000-0000000000ff';
export const CIPHER = 'dddddddd-0000-4000-8000-000000000001';
export const PERSONAL = 'dddddddd-0000-4000-8000-000000000002';
export const ALICE_PERSONAL = 'dddddddd-0000-4000-8000-000000000003';
export const INV_CAROL = 'eeeeeeee-0000-4000-8000-000000000001';
export const ATT = 'ffffffff-0000-4000-8000-000000000001';

export const fullAccess = (id: string) => ({ id, readOnly: false, hidePasswords: false, manage: false });
export const readOnlyAccess = (id: string) => ({ id, readOnly: true, hidePasswords: false, manage: false });

export interface Harness {
  db: DatabaseSync;
  env: Env;
}

type Row = Record<string, unknown>;

// D1 binds booleans as 1/0; node:sqlite refuses them.
function bindable(values: unknown[]): any[] {
  return values.map((value) => (typeof value === 'boolean' ? (value ? 1 : 0) : value));
}

function statement(db: DatabaseSync, sql: string, args: any[]): any {
  return {
    bind: (...next: unknown[]) => statement(db, sql, bindable(next)),
    async first(column?: string) {
      const row = db.prepare(sql).get(...args) as Row | undefined;
      if (!row) return null;
      const plain: Row = { ...row };
      return column ? (plain[column] ?? null) : plain;
    },
    async all() {
      const rows = db.prepare(sql).all(...args) as Row[];
      return { results: rows.map((row) => ({ ...row })), success: true, meta: {} };
    },
    async run() {
      const info = db.prepare(sql).run(...args);
      return { success: true, meta: { changes: Number(info.changes), last_row_id: Number(info.lastInsertRowid) } };
    },
  };
}

function asD1(db: DatabaseSync): any {
  return {
    prepare: (sql: string) => statement(db, sql, []),
    async batch(statements: any[]) {
      const results = [];
      for (const stmt of statements) results.push(await stmt.run());
      return results;
    },
    async exec(sql: string) {
      db.exec(sql);
      return { count: 1, duration: 0 };
    },
  };
}

function stubCacheApi(): void {
  const cache = {
    match: async () => undefined,
    put: async () => undefined,
    delete: async () => false,
  };
  (globalThis as any).caches = { default: cache, open: async () => cache };
}

export function createHarness(): Harness {
  const db = new DatabaseSync(':memory:');
  db.exec('PRAGMA foreign_keys = ON');
  // Mirrors executeSchemaStatement: ALTER TABLE ADD COLUMN fails once the column exists.
  for (const sql of SCHEMA_STATEMENTS) {
    try {
      db.exec(sql);
    } catch (error) {
      const message = String(error instanceof Error ? error.message : error).toLowerCase();
      if (message.includes('already exists') || message.includes('duplicate column name')) continue;
      throw new Error(`schema statement failed: ${sql}\n${message}`);
    }
  }
  stubCacheApi();
  const env = {
    DB: asD1(db),
    JWT_SECRET: 'sharing-tests-jwt-secret-0123456789abcdef',
    NOTIFICATIONS_HUB: {
      idFromName: (name: string) => name,
      get: () => ({ fetch: async () => new Response(null, { status: 204 }) }),
    },
  } as unknown as Env;
  return { db, env };
}

export function seedUser(
  h: Harness,
  person: Person,
  options: { role?: 'admin' | 'user'; publicKey?: string | null } = {}
): string {
  const id = U[person];
  const publicKey = options.publicKey === undefined ? `PUBKEY-${person}` : options.publicKey;
  // A raw client hash with no server prefix is a legacy row: verifyPassword
  // compares it directly, so a test "knows" the master password hash-<person>.
  h.db.prepare(
    'INSERT INTO users(id, email, name, master_password_hash, key, private_key, public_key, kdf_type, kdf_iterations, ' +
    "security_stamp, role, status, created_at, updated_at) VALUES(?, ?, ?, ?, ?, ?, ?, 0, 600000, ?, ?, 'active', ?, ?)"
  ).run(id, emailOf(person), person, `hash-${person}`, ENC, ENC, publicKey, `stamp-${person}`, options.role ?? 'user', T0, T0);
  h.db.prepare('INSERT INTO user_revisions(user_id, revision_date) VALUES(?, ?)').run(id, OLD_REVISION);
  return id;
}

export function seedMember(
  h: Harness,
  orgId: string,
  userId: string,
  options: { role?: string; status?: number; collections?: unknown[]; key?: string | null } = {}
): void {
  h.db.prepare(
    'INSERT INTO organization_members(organization_id, user_id, status, role, permissions, collections, encrypted_org_key, created_at, updated_at) ' +
    'VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?)'
  ).run(
    orgId,
    userId,
    options.status ?? 1,
    options.role ?? 'member',
    '{}',
    JSON.stringify(options.collections ?? []),
    options.key === undefined ? `ORGKEY-${userId}` : options.key,
    T0,
    T0
  );
}

/** An organization and its owner's membership (confirmed: it holds a key). */
export function seedOrg(
  h: Harness,
  orgId: string,
  ownerId: string,
  options: { name?: string; publicKey?: string | null; privateKey?: string | null } = {}
): void {
  h.db.prepare(
    'INSERT INTO organizations(id, name, enabled, use_totp, owner_id, billing_email, public_key, private_key, identifier, created_at, updated_at) ' +
    'VALUES(?, ?, 1, 0, ?, NULL, ?, ?, NULL, ?, ?)'
  ).run(orgId, options.name ?? 'Shared by owner@example.test', ownerId, options.publicKey ?? null, options.privateKey ?? null, T0, T0);
  seedMember(h, orgId, ownerId, { role: 'owner' });
}

export function seedCollection(h: Harness, orgId: string, id: string, name: string = ENC): void {
  h.db.prepare('INSERT INTO collections(id, organization_id, name, external_id, created_at, updated_at) VALUES(?, ?, ?, NULL, ?, ?)')
    .run(id, orgId, name, T0, T0);
}

export function seedCipher(
  h: Harness,
  id: string,
  userId: string,
  options: { organizationId?: string | null; collectionIds?: string[]; key?: string | null; updatedAt?: string } = {}
): void {
  const organizationId = options.organizationId ?? null;
  const collectionIds = options.collectionIds ?? [];
  const data = {
    id,
    type: 1,
    name: ENC,
    notes: null,
    favorite: false,
    login: { username: ENC, password: ENC, uris: null },
    fields: null,
    passwordHistory: null,
    reprompt: 0,
    key: options.key ?? null,
    organizationId,
    collectionIds,
  };
  h.db.prepare(
    'INSERT INTO ciphers(id, user_id, type, folder_id, name, notes, favorite, data, reprompt, key, created_at, updated_at, archived_at, deleted_at) ' +
    'VALUES(?, ?, 1, NULL, ?, NULL, 0, ?, 0, ?, ?, ?, NULL, NULL)'
  ).run(id, userId, ENC, JSON.stringify(data), options.key ?? null, T0, options.updatedAt ?? T0);
  for (const collectionId of collectionIds) {
    h.db.prepare('INSERT INTO collection_items(collection_id, cipher_id) VALUES(?, ?)').run(collectionId, id);
  }
}

export function seedAttachment(h: Harness, id: string, cipherId: string, options: { key?: string | null } = {}): void {
  h.db.prepare(
    'INSERT INTO attachments(id, cipher_id, user_id, file_name, size, size_name, key) ' +
    'VALUES(?, ?, (SELECT user_id FROM ciphers WHERE id = ?), ?, 10, ?, ?)'
  ).run(id, cipherId, cipherId, ENC, '10 Bytes', options.key === undefined ? ENC : options.key);
}

export function seedInvitation(
  h: Harness,
  orgId: string,
  id: string,
  email: string,
  options: { invitedBy: string; collections?: unknown[]; role?: string; token?: string; expiresAt?: string }
): void {
  h.db.prepare(
    'INSERT INTO organization_invitations(id, organization_id, email, role, permissions, collections, invited_by, status, token, expires_at, created_at, updated_at) ' +
    'VALUES(?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)'
  ).run(
    id,
    orgId,
    email,
    options.role ?? 'member',
    '{}',
    JSON.stringify(options.collections ?? []),
    options.invitedBy,
    options.token ?? `token-${id}`,
    options.expiresAt ?? new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString(),
    T0,
    T0
  );
}

export function revisionOf(h: Harness, userId: string): string {
  const row = h.db.prepare('SELECT revision_date FROM user_revisions WHERE user_id = ?').get(userId) as Row | undefined;
  return String(row?.revision_date ?? '');
}

export function wasBumped(h: Harness, userId: string): boolean {
  return revisionOf(h, userId) !== OLD_REVISION;
}

export function resetRevisions(h: Harness): void {
  h.db.prepare('UPDATE user_revisions SET revision_date = ?').run(OLD_REVISION);
}

export function memberCollectionsRaw(h: Harness, orgId: string, userId: string): unknown[] {
  const row = h.db.prepare('SELECT collections FROM organization_members WHERE organization_id = ? AND user_id = ?').get(orgId, userId) as Row;
  return JSON.parse(String(row.collections));
}

export function invitationCollectionsRaw(h: Harness, invitationId: string): unknown[] {
  const row = h.db.prepare('SELECT collections FROM organization_invitations WHERE id = ?').get(invitationId) as Row;
  return JSON.parse(String(row.collections));
}

export function cipherData(h: Harness, cipherId: string): any {
  const row = h.db.prepare('SELECT data FROM ciphers WHERE id = ?').get(cipherId) as Row | undefined;
  return row ? JSON.parse(String(row.data)) : null;
}

export function count(h: Harness, sql: string, ...args: unknown[]): number {
  const row = h.db.prepare(sql).get(...(args as any[])) as Row;
  return Number(Object.values(row)[0]);
}

export async function call(
  h: Harness,
  person: Person,
  method: string,
  path: string,
  body?: unknown
): Promise<{ status: number; body: any; headers: Headers }> {
  const url = new URL(path, 'https://vault.example.test');
  const init: RequestInit = { method, headers: { 'Content-Type': 'application/json' } };
  if (body !== undefined) init.body = JSON.stringify(body);
  const request = new Request(url.toString(), init);
  const userId = U[person];
  const currentUser = await getUserById(h.env.DB, userId);
  if (!currentUser) throw new Error(`no seeded user for ${person}`);
  const response = await handleAuthenticatedRoute(request, h.env, userId, currentUser, url.pathname, method);
  if (!response) return { status: 404, body: null, headers: new Headers() };
  const text = await response.text();
  let parsed: any = null;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = text;
    }
  }
  return { status: response.status, body: parsed, headers: response.headers };
}
```

- [ ] **Step 2: Add the test script**

In `package.json`, add this line directly after the `"test:organizations"` line:

```json
    "test:sharing": "tsx --test --import ./scripts/cloudflare-workers-stub.mjs \"scripts/sharing-*.test.ts\"",
```

- [ ] **Step 3: Write the failing tests**

Create `scripts/sharing-collection-access.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applyCollectionAccessUpdate,
  collectionAccessIds,
  normalizeCollectionAccessList,
  writableCollectionIds,
} from '../src/services/org-collection-access';
import {
  CIPHER, COL_A, COL_B, ORG, U, call, createHarness, emailOf, memberCollectionsRaw,
  readOnlyAccess, seedCipher, seedCollection, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// A collection access entry is what Bitwarden calls a SelectionReadOnly:
// {id, readOnly, hidePasswords, manage}. Rows written before this change hold
// bare id strings, and those must keep meaning what they always meant.

test('a bare id reads as full access and an object keeps its flags', () => {
  assert.deepEqual(
    normalizeCollectionAccessList([
      'c1',
      { id: 'c2', readOnly: true, hidePasswords: false, manage: false },
      { Id: 'c3', ReadOnly: true, HidePasswords: true, Manage: true },
    ]),
    [
      { id: 'c1', readOnly: false, hidePasswords: false, manage: false },
      { id: 'c2', readOnly: true, hidePasswords: false, manage: false },
      { id: 'c3', readOnly: true, hidePasswords: true, manage: true },
    ]
  );
});

test('junk is dropped and a repeated id keeps the more permissive access', () => {
  assert.deepEqual(
    normalizeCollectionAccessList([null, 7, '', '  ', {}, { id: '' }, { id: 'c1', readOnly: true }, { id: 'c1', readOnly: false, manage: true }]),
    [{ id: 'c1', readOnly: false, hidePasswords: false, manage: true }]
  );
  assert.deepEqual(normalizeCollectionAccessList('c1'), []);
  assert.deepEqual(normalizeCollectionAccessList(undefined), []);
});

test('an update in bare ids keeps the flags each id already had', () => {
  // The regression this guards: an old client that only knows ids must not be
  // able to turn a read-only person into an editor by saving the member.
  const existing = [{ id: 'c1', readOnly: true, hidePasswords: false, manage: false }];
  assert.deepEqual(applyCollectionAccessUpdate(existing, ['c1', 'c2']), [
    { id: 'c1', readOnly: true, hidePasswords: false, manage: false },
    { id: 'c2', readOnly: false, hidePasswords: false, manage: false },
  ]);
  assert.deepEqual(applyCollectionAccessUpdate(existing, [{ id: 'c1', readOnly: false }]), [
    { id: 'c1', readOnly: false, hidePasswords: false, manage: false },
  ]);
  assert.deepEqual(applyCollectionAccessUpdate(existing, 'not a list'), []);
});

test('ids and writable ids', () => {
  const list = [
    { id: 'a', readOnly: true, hidePasswords: false, manage: false },
    { id: 'b', readOnly: false, hidePasswords: false, manage: false },
  ];
  assert.deepEqual(collectionAccessIds(list), ['a', 'b']);
  assert.deepEqual(writableCollectionIds(list), ['b']);
});

test('a member row written as bare ids still reaches its collection', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedMember(h, ORG, U.alice, { collections: [COL_A] });
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });

  assert.equal((await call(h, 'alice', 'GET', `/api/ciphers/${CIPHER}`)).status, 200);
  const roster = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/members`);
  const alice = roster.body.data.find((member: any) => member.id === U.alice);
  // The NodeWarden roster keeps answering ids: its web vault and Deskwarden read strings.
  assert.deepEqual(alice.collections, [COL_A]);
});

test('the NodeWarden invite route stores collection flags and still answers ids', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);

  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/members/invite`, {
    email: emailOf('alice'),
    role: 'member',
    encryptedOrgKey: '4.d3JhcHBlZA==',
    collections: [readOnlyAccess(COL_A)],
  });
  assert.equal(res.status, 200);
  assert.deepEqual(res.body.collections, [COL_A]);
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.alice), [readOnlyAccess(COL_A)]);
});

test('saving a member by collection ids does not make a read-only member writable', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });

  const res = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/members/${U.alice}`, { collections: [COL_A, COL_B] });
  assert.equal(res.status, 200);
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.alice), [
    readOnlyAccess(COL_A),
    { id: COL_B, readOnly: false, hidePasswords: false, manage: false },
  ]);
});
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL. The file does not load: `Cannot find module` for `../src/services/org-collection-access` (`ERR_MODULE_NOT_FOUND`), reported as `✖ scripts/sharing-collection-access.test.ts`.

- [ ] **Step 5: Add the type**

In `src/types/index.ts`, directly above `export interface OrganizationMember {`, add:

```ts
// One member's (or invitation's) access to one collection: Bitwarden's
// SelectionReadOnlyRequestModel. Stored as JSON in the `collections` column.
export interface CollectionAccess {
  id: string;
  readOnly: boolean;
  hidePasswords: boolean;
  manage: boolean;
}

```

In `export interface OrganizationMember`, replace `  collections: string[];` with `  collections: CollectionAccess[];`. In `export interface OrganizationInvitation`, make the same replacement. Leave `OrganizationMemberResponse.collections: string[]` as it is.

- [ ] **Step 6: Write the module**

Create `src/services/org-collection-access.ts`:

```ts
import type { CollectionAccess } from '../types';

// Collection access, as Bitwarden models it: one entry per collection a member
// (or a pending invitation) holds, with readOnly / hidePasswords / manage.
//
// The `collections` column held bare id strings before read-only collections
// existed. A bare id reads as full access, which is what it always meant, so old
// rows keep their meaning without a migration. Rows are written back as objects.

const FULL_ACCESS = { readOnly: false, hidePasswords: false, manage: false } as const;

function flag(raw: Record<string, unknown>, camel: string, pascal: string): boolean {
  return raw[camel] === true || raw[pascal] === true;
}

export function normalizeCollectionAccess(entry: unknown): CollectionAccess | null {
  if (typeof entry === 'string') {
    const id = entry.trim();
    return id ? { id, ...FULL_ACCESS } : null;
  }
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return null;
  const raw = entry as Record<string, unknown>;
  const id = String(raw.id ?? raw.Id ?? '').trim();
  if (!id) return null;
  return {
    id,
    readOnly: flag(raw, 'readOnly', 'ReadOnly'),
    hidePasswords: flag(raw, 'hidePasswords', 'HidePasswords'),
    manage: flag(raw, 'manage', 'Manage'),
  };
}

// Two entries for one collection merge to the more permissive of the two, the
// way official servers combine direct and group access.
function mergeAccess(a: CollectionAccess, b: CollectionAccess): CollectionAccess {
  return {
    id: a.id,
    readOnly: a.readOnly && b.readOnly,
    hidePasswords: a.hidePasswords && b.hidePasswords,
    manage: a.manage || b.manage,
  };
}

export function normalizeCollectionAccessList(value: unknown): CollectionAccess[] {
  if (!Array.isArray(value)) return [];
  const byId = new Map<string, CollectionAccess>();
  for (const entry of value) {
    const access = normalizeCollectionAccess(entry);
    if (!access) continue;
    const previous = byId.get(access.id);
    byId.set(access.id, previous ? mergeAccess(previous, access) : access);
  }
  return [...byId.values()];
}

export function collectionAccessIds(list: readonly CollectionAccess[]): string[] {
  return list.map((entry) => entry.id);
}

export function writableCollectionIds(list: readonly CollectionAccess[]): string[] {
  return list.filter((entry) => !entry.readOnly).map((entry) => entry.id);
}

// A client-supplied collection list applied to a member's current one. A bare
// id keeps the flags that id already had, so a client that only knows ids (the
// web vault's member editor) cannot silently turn a read-only person into an
// editor. An object sets its flags as given.
export function applyCollectionAccessUpdate(existing: readonly CollectionAccess[], incoming: unknown): CollectionAccess[] {
  if (!Array.isArray(incoming)) return [];
  const current = new Map(existing.map((entry) => [entry.id, entry]));
  const next = new Map<string, CollectionAccess>();
  for (const entry of incoming) {
    if (typeof entry === 'string') {
      const id = entry.trim();
      if (!id) continue;
      next.set(id, current.get(id) ?? { id, ...FULL_ACCESS });
      continue;
    }
    const access = normalizeCollectionAccess(entry);
    if (access) next.set(access.id, access);
  }
  return [...next.values()];
}

// The list with `access` replacing any entry for the same collection.
export function withCollectionAccess(list: readonly CollectionAccess[], access: CollectionAccess): CollectionAccess[] {
  return [...list.filter((entry) => entry.id !== access.id), access];
}
```

- [ ] **Step 7: Normalise on read in the repo**

In `src/services/storage-organization-repo.ts`:

Add after the two existing imports:

```ts
import { normalizeCollectionAccessList } from './org-collection-access';
```

In `mapMemberRow` and in `mapInvitationRow`, replace

```ts
    collections: row.collections ? JSON.parse(row.collections) : [],
```

with

```ts
    collections: normalizeCollectionAccessList(row.collections ? JSON.parse(row.collections) : []),
```

In `getAccessibleCollectionsForUser`, replace

```ts
      for (const collectionId of membership.collections || []) accessible.add(collectionId);
```

with

```ts
      for (const entry of membership.collections || []) accessible.add(entry.id);
```

In `deleteCollection`, replace

```ts
    const collections = member.collections || [];
    if (!collections.includes(id)) continue;
    member.collections = collections.filter((collectionId) => collectionId !== id);
```

with

```ts
    const collections = member.collections || [];
    if (!collections.some((entry) => entry.id === id)) continue;
    member.collections = collections.filter((entry) => entry.id !== id);
```

- [ ] **Step 8: Follow the new shape in the access check**

In `src/services/storage-org-access.ts`, replace

```ts
    cipherCollectionIds.length > 0 && cipherCollectionIds.some((id) => member.collections.includes(id));
```

with

```ts
    cipherCollectionIds.length > 0 && cipherCollectionIds.some((id) => member.collections.some((entry) => entry.id === id));
```

(Task 2 rewrites this function; this keeps the tree compiling in between.)

- [ ] **Step 9: Follow the new shape in the organization handlers**

In `src/handlers/organizations.ts`:

Add after the `import { LIMITS } from '../config/limits';` line:

```ts
import { applyCollectionAccessUpdate, collectionAccessIds, normalizeCollectionAccessList } from '../services/org-collection-access';
```

In `toMemberResponse`, replace `    collections: member.collections || [],` with:

```ts
    // Ids only: NodeWarden's web vault and Deskwarden read strings here. The
    // flags are on the official /users route.
    collections: collectionAccessIds(member.collections || []),
```

In `toInvitationResponse`, replace `    collections: invitation.collections || [],` with:

```ts
    collections: collectionAccessIds(invitation.collections || []),
```

In `handleInviteOrganizationMember`, replace

```ts
  const collections = Array.isArray(body?.collections) ? body.collections.map(String).filter(Boolean) : [];
```

with

```ts
  // Entries may be bare ids (full access, as before) or {id, readOnly, hidePasswords, manage}.
  const collections = normalizeCollectionAccessList(body?.collections);
```

In `handleUpdateOrganizationMember`, replace

```ts
    targetMember.collections = Array.isArray(body.collections) ? body.collections.map(String).filter(Boolean) : [];
```

with

```ts
    targetMember.collections = applyCollectionAccessUpdate(targetMember.collections || [], body.collections);
```

- [ ] **Step 10: Follow the new shape in SCIM**

In `src/handlers/scim.ts`, add after `import { LIMITS } from '../config/limits';`:

```ts
import { withCollectionAccess } from '../services/org-collection-access';
```

In `groupMemberExternalIds`, replace

```ts
    if (!(member.collections || []).includes(collectionId)) continue;
```

with

```ts
    if (!(member.collections || []).some((entry) => entry.id === collectionId)) continue;
```

In `applyGroupMembers`, replace

```ts
    let collections = member.collections || [];
    if (target) {
      if (!collections.includes(collectionId)) collections = [...collections, collectionId];
    } else {
      collections = collections.filter((collection) => collection !== collectionId);
    }
    if (collections.length !== (member.collections || []).length) {
```

with

```ts
    const current = member.collections || [];
    let collections = current;
    if (target) {
      if (!current.some((entry) => entry.id === collectionId)) {
        // A SCIM group grants what it always granted: full access.
        collections = withCollectionAccess(current, { id: collectionId, readOnly: false, hidePasswords: false, manage: false });
      }
    } else {
      collections = current.filter((entry) => entry.id !== collectionId);
    }
    if (collections.length !== current.length) {
```

- [ ] **Step 11: Follow the stored shape in the existing org test**

In `scripts/organizations.test.ts`, in the test `deleting a collection strips it from every member access list`, replace

```ts
  assert.deepEqual(JSON.parse(String(saved[0][5])), ['col-2']);
```

with

```ts
  // Access entries are written back as objects; the bare 'col-2' read as full access.
  assert.deepEqual(JSON.parse(String(saved[0][5])), [{ id: 'col-2', readOnly: false, hidePasswords: false, manage: false }]);
```

- [ ] **Step 12: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 7`, `ℹ pass 7`, `ℹ fail 0`.

Run: `pnpm run test:organizations`
Expected: `ℹ tests 11`, `ℹ pass 11`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 13: Commit**

```bash
git add src/services/org-collection-access.ts src/types/index.ts src/services/storage-organization-repo.ts src/services/storage-org-access.ts src/handlers/organizations.ts src/handlers/scim.ts scripts/support/sharing-harness.ts scripts/sharing-collection-access.test.ts scripts/organizations.test.ts package.json
git commit -m "A collection grants read-only access as well as full access" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Per-viewer access: `edit`, `viewPassword` and `collectionIds`

**Files:**
- Modify: `src/services/org-collection-access.ts` (add `computeCipherViewerAccess`)
- Modify: `src/services/storage-org-access.ts` (`CipherAccessResult`, `resolveCipherAccess`)
- Modify: `src/handlers/ciphers.ts` (`CipherViewer`, `cipherToResponse`, `viewerFromAccess`, `viewerForMember`, `handleGetCipher`)
- Modify: `src/handlers/sync.ts` (per-viewer fields)
- Test: `scripts/sharing-viewer-access.test.ts`

**Interfaces:**
- Consumes: `CollectionAccess` (Task 1); the harness (Task 1).
- Produces:
  - `org-collection-access.ts`: `interface CipherViewerAccess { canSee: boolean; canEdit: boolean; viewPassword: boolean; visibleCollectionIds: string[] }` and `computeCipherViewerAccess(viewer: { allCollections: boolean; collections: readonly CollectionAccess[] }, cipherCollectionIds: readonly string[]): CipherViewerAccess`.
  - `storage-org-access.ts`: `CipherAccessResult` becomes `{ cipher; member; organizationId; canManageCollections: boolean; canEdit: boolean; viewPassword: boolean; visibleCollectionIds: string[] }`. The unused `accessibleCollectionIds` is dropped (nothing reads it, and it cost a query per organization). A personal cipher's owner gets `canEdit: true, viewPassword: true, visibleCollectionIds: []`.
  - `ciphers.ts`: `interface CipherViewer { edit: boolean; viewPassword: boolean; collectionIds: string[] }`; `CipherResponseOptions.viewer?: CipherViewer`; `viewerFromAccess(access: CipherAccessResult): CipherViewer`; `viewerForMember(cipher: Cipher, memberships: ReadonlyMap<string, OrganizationMember>): CipherViewer | undefined`.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-viewer-access.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { computeCipherViewerAccess } from '../src/services/org-collection-access';
import {
  CIPHER, COL_A, COL_B, ORG, U, call, createHarness, fullAccess, readOnlyAccess,
  seedCipher, seedCollection, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// What one person may do with one item is computed from the collections they
// hold it through, never stored on the item: the same record is view-only for
// a recipient and editable for its sharer.

const hidden = (id: string) => ({ id, readOnly: true, hidePasswords: true, manage: false });

test('a manager can edit every item, whatever the collection flags say', () => {
  assert.deepEqual(computeCipherViewerAccess({ allCollections: true, collections: [] }, ['c1']), {
    canSee: true, canEdit: true, viewPassword: true, visibleCollectionIds: ['c1'],
  });
});

test('a read-only collection lets a member see the item but not edit it', () => {
  assert.deepEqual(computeCipherViewerAccess({ allCollections: false, collections: [readOnlyAccess('c1')] }, ['c1']), {
    canSee: true, canEdit: false, viewPassword: true, visibleCollectionIds: ['c1'],
  });
});

test('one writable collection among several makes the item editable', () => {
  const access = computeCipherViewerAccess(
    { allCollections: false, collections: [readOnlyAccess('c1'), fullAccess('c2')] },
    ['c1', 'c2']
  );
  assert.equal(access.canEdit, true);
  assert.deepEqual(access.visibleCollectionIds, ['c1', 'c2']);
});

test('passwords are hidden only when every collection holding the item hides them', () => {
  assert.equal(computeCipherViewerAccess({ allCollections: false, collections: [hidden('c1')] }, ['c1']).viewPassword, false);
  assert.equal(
    computeCipherViewerAccess({ allCollections: false, collections: [hidden('c1'), readOnlyAccess('c2')] }, ['c1', 'c2']).viewPassword,
    true
  );
});

test('an item in none of the member collections is not visible', () => {
  assert.deepEqual(computeCipherViewerAccess({ allCollections: false, collections: [fullAccess('c9')] }, ['c1']), {
    canSee: false, canEdit: false, viewPassword: false, visibleCollectionIds: [],
  });
});

test("a member is told only their own collections, never another person's", () => {
  assert.deepEqual(
    computeCipherViewerAccess({ allCollections: false, collections: [readOnlyAccess('c1')] }, ['c1', 'c2']).visibleCollectionIds,
    ['c1']
  );
});

function sharedWithTwo() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });
  seedMember(h, ORG, U.bob, { collections: [readOnlyAccess(COL_B)] });
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A, COL_B] });
  return h;
}

test('a recipient reads the item as view-only, in their own collection only', async () => {
  const h = sharedWithTwo();
  const res = await call(h, 'alice', 'GET', `/api/ciphers/${CIPHER}`);
  assert.equal(res.status, 200);
  assert.equal(res.body.edit, false);
  assert.equal(res.body.viewPassword, true);
  assert.deepEqual(res.body.permissions, { delete: false, restore: false });
  assert.deepEqual(res.body.collectionIds, [COL_A]);
});

test('the sharer reads the same item as editable, in every collection', async () => {
  const h = sharedWithTwo();
  const res = await call(h, 'owner', 'GET', `/api/ciphers/${CIPHER}/details`);
  assert.equal(res.status, 200);
  assert.equal(res.body.edit, true);
  assert.deepEqual(res.body.permissions, { delete: true, restore: true });
  assert.deepEqual(res.body.collectionIds, [COL_A, COL_B]);
});

test('sync answers each reader with their own permissions and collections', async () => {
  const h = sharedWithTwo();
  const find = (body: any) => body.ciphers.find((cipher: any) => cipher.id === CIPHER);

  const alice = find((await call(h, 'alice', 'GET', '/api/sync')).body);
  assert.equal(alice.edit, false);
  assert.deepEqual(alice.collectionIds, [COL_A]);

  const bob = find((await call(h, 'bob', 'GET', '/api/sync')).body);
  assert.equal(bob.edit, false);
  assert.deepEqual(bob.collectionIds, [COL_B]);

  const owner = find((await call(h, 'owner', 'GET', '/api/sync')).body);
  assert.equal(owner.edit, true);
  assert.deepEqual(owner.collectionIds, [COL_A, COL_B]);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL. `scripts/sharing-viewer-access.test.ts` does not load: `does not provide an export named 'computeCipherViewerAccess'`. The 7 Task 1 tests still pass.

- [ ] **Step 3: Add the computation**

Append to `src/services/org-collection-access.ts`:

```ts
export interface CipherViewerAccess {
  canSee: boolean;
  canEdit: boolean;
  viewPassword: boolean;
  visibleCollectionIds: string[];
}

// One viewer's access to one organization cipher, from the collections they
// hold it through (Bitwarden's UserCipherDetails rules):
// - a manager or above reaches every collection and may edit anything;
// - otherwise the item is visible through the collections the viewer holds
//   that contain it, editable when any of those is writable, and its password
//   is viewable unless every one of them hides passwords;
// - the viewer is told only those collections, never the other people's.
export function computeCipherViewerAccess(
  viewer: { allCollections: boolean; collections: readonly CollectionAccess[] },
  cipherCollectionIds: readonly string[]
): CipherViewerAccess {
  if (viewer.allCollections) {
    return { canSee: true, canEdit: true, viewPassword: true, visibleCollectionIds: [...cipherCollectionIds] };
  }
  const held = new Map(viewer.collections.map((entry) => [entry.id, entry]));
  const holding = cipherCollectionIds.filter((id) => held.has(id));
  if (!holding.length) {
    return { canSee: false, canEdit: false, viewPassword: false, visibleCollectionIds: [] };
  }
  const entries = holding.map((id) => held.get(id)!);
  return {
    canSee: true,
    canEdit: entries.some((entry) => !entry.readOnly),
    viewPassword: entries.some((entry) => !entry.hidePasswords),
    visibleCollectionIds: holding,
  };
}
```

- [ ] **Step 4: Carry it through `resolveCipherAccess`**

In `src/services/storage-org-access.ts`, add after the existing imports:

```ts
import { computeCipherViewerAccess } from './org-collection-access';
```

Replace everything from `export interface CipherAccessResult {` to the closing brace of `resolveCipherAccess` with:

```ts
export interface CipherAccessResult {
  cipher: Cipher;
  member: OrganizationMember | null;
  organizationId: string | null;
  canManageCollections: boolean;
  /** May change the item: update, delete, restore, attachments, collections. */
  canEdit: boolean;
  viewPassword: boolean;
  /** The item's collections this viewer holds; every one of them for a manager. */
  visibleCollectionIds: string[];
}

export async function resolveCipherAccess(db: D1Database, userId: string, cipherId: string): Promise<CipherAccessResult | null> {
  const cipher = await getCipher(db, cipherId);
  if (!cipher) return null;

  const { organizationId, collectionIds } = readOrgCipherFields(cipher);

  // Personal cipher: only the owner may access it, and fully.
  if (!organizationId) {
    if (cipher.userId !== userId) return null;
    return {
      cipher,
      member: null,
      organizationId: null,
      canManageCollections: false,
      canEdit: true,
      viewPassword: true,
      visibleCollectionIds: [],
    };
  }

  // Organization cipher: the user must be an accepted member holding at least
  // one of its collections. An unassigned org cipher belongs to no collection,
  // so only members with all-collection access may see it.
  const member = await getOrganizationMember(db, organizationId, userId);
  if (!member || member.status !== 1) return null;

  const allCollections = canAccessAllCollections(member);
  const viewer = computeCipherViewerAccess({ allCollections, collections: member.collections }, collectionIds);
  if (!viewer.canSee) return null;

  return {
    cipher,
    member,
    organizationId,
    canManageCollections: allCollections,
    canEdit: viewer.canEdit,
    viewPassword: viewer.viewPassword,
    visibleCollectionIds: viewer.visibleCollectionIds,
  };
}
```

(`readOrgCipherFields` is declared later in the same file; function declarations are hoisted.)

- [ ] **Step 5: Let the response carry a viewer**

In `src/handlers/ciphers.ts`:

In the `import { Env, Cipher, ..., PasswordHistory, } from '../types';` list, add `OrganizationMember,` after `PasswordHistory,`.

Replace

```ts
import { resolveCipherAccess, isOrgCipher } from '../services/storage-org-access';
```

with

```ts
import { resolveCipherAccess, isOrgCipher, type CipherAccessResult } from '../services/storage-org-access';
import { canAccessAllCollections } from '../services/storage-organization-repo';
import { computeCipherViewerAccess } from '../services/org-collection-access';
```

Replace

```ts
export interface CipherResponseOptions {
  preserveRepairableUris?: boolean;
  validFolderIds?: ReadonlySet<string>;
}
```

with

```ts
// Who is reading, for an organization item: the fields that differ per reader.
export interface CipherViewer {
  edit: boolean;
  viewPassword: boolean;
  collectionIds: string[];
}

export interface CipherResponseOptions {
  preserveRepairableUris?: boolean;
  validFolderIds?: ReadonlySet<string>;
  /** When set, edit / viewPassword / permissions / collectionIds are this reader's. */
  viewer?: CipherViewer;
}

export function viewerFromAccess(access: CipherAccessResult): CipherViewer {
  return { edit: access.canEdit, viewPassword: access.viewPassword, collectionIds: access.visibleCollectionIds };
}

// The reader's view of an organization item in a bulk answer such as /api/sync,
// from their membership. Undefined for a personal item.
export function viewerForMember(cipher: Cipher, memberships: ReadonlyMap<string, OrganizationMember>): CipherViewer | undefined {
  const { organizationId, collectionIds } = readOrgCipherFields(cipher);
  if (!organizationId) return undefined;
  const member = memberships.get(organizationId);
  const access = member
    ? computeCipherViewerAccess({ allCollections: canAccessAllCollections(member), collections: member.collections }, collectionIds)
    : null;
  // Reached through a collection link the item's own record does not list:
  // readable, never editable, and in no collection this reader is told about.
  if (!access || !access.canSee) return { edit: false, viewPassword: true, collectionIds: [] };
  return { edit: access.canEdit, viewPassword: access.viewPassword, collectionIds: access.visibleCollectionIds };
}
```

In `cipherToResponse`, replace

```ts
  const responsePermissions = buildCipherPermissions(passthrough);
```

with

```ts
  const viewer = options.viewer;
  const responsePermissions = viewer
    ? { delete: viewer.edit, restore: viewer.edit }
    : buildCipherPermissions(passthrough);
```

and replace

```ts
    edit: readBooleanOrFallback((passthrough as any).edit, true),
    viewPassword: readBooleanOrFallback((passthrough as any).viewPassword, true),
    permissions: responsePermissions,
    object: 'cipherDetails',
    collectionIds: Array.isArray((passthrough as any).collectionIds) ? (passthrough as any).collectionIds : [],
```

with

```ts
    edit: viewer ? viewer.edit : readBooleanOrFallback((passthrough as any).edit, true),
    viewPassword: viewer ? viewer.viewPassword : readBooleanOrFallback((passthrough as any).viewPassword, true),
    permissions: responsePermissions,
    object: 'cipherDetails',
    collectionIds: viewer
      ? [...viewer.collectionIds]
      : (Array.isArray((passthrough as any).collectionIds) ? (passthrough as any).collectionIds : []),
```

Replace the whole `handleGetCipher` function (from `// GET /api/ciphers/:id` to its closing brace) with:

```ts
// GET /api/ciphers/:id
export async function handleGetCipher(request: Request, env: Env, userId: string, id: string): Promise<Response> {
  const storage = new StorageService(env.DB);
  const access = await resolveCipherAccess(env.DB, userId, id);

  if (!access) {
    return errorResponse('Cipher not found', 404);
  }
  const cipher = access.cipher;

  const [attachments, folders] = await Promise.all([
    storage.getAttachmentsByCipher(cipher.id),
    storage.getAllFolders(userId),
  ]);
  // A shared org row may carry another member's folder id; never echo it back.
  // edit / viewPassword / collectionIds are this reader's own.
  const responseOptions = {
    ...cipherResponseOptionsForRequest(request),
    validFolderIds: new Set(folders.map((folder) => folder.id)),
    viewer: viewerFromAccess(access),
  };
  return jsonResponse(
    cipherToResponse(cipher, attachments, responseOptions)
  );
}
```

- [ ] **Step 6: Per-viewer fields in `/api/sync`**

In `src/handlers/sync.ts`:

Replace

```ts
import { cipherToResponse, isCipherResponseSyncCompatible, shouldPreserveRepairableCipherUris } from './ciphers';
```

with

```ts
import { cipherToResponse, isCipherResponseSyncCompatible, shouldPreserveRepairableCipherUris, viewerForMember } from './ciphers';
```

Replace `import { getOrganizationsForUser } from '../services/storage-organization-repo';` with:

```ts
import { getOrganizationsForUser, getOrganizationMembershipsForUser } from '../services/storage-organization-repo';
```

Replace

```ts
  const [ciphers, folders, sends, domainSettings, organizations, orgCiphers] = await Promise.all([
    storage.getAllCiphers(userId),
    storage.getAllFolders(userId),
    excludeSends ? Promise.resolve([]) : storage.getAllSends(userId),
    excludeDomains ? Promise.resolve(null) : storage.getUserDomainSettings(userId),
    getOrganizationsForUser(env.DB, userId),
    getAccessibleOrgCiphers(env.DB, userId),
  ]);
```

with

```ts
  const [ciphers, folders, sends, domainSettings, organizations, orgCiphers, memberships] = await Promise.all([
    storage.getAllCiphers(userId),
    storage.getAllFolders(userId),
    excludeSends ? Promise.resolve([]) : storage.getAllSends(userId),
    excludeDomains ? Promise.resolve(null) : storage.getUserDomainSettings(userId),
    getOrganizationsForUser(env.DB, userId),
    getAccessibleOrgCiphers(env.DB, userId),
    getOrganizationMembershipsForUser(env.DB, userId),
  ]);
  const membershipByOrg = new Map(memberships.map((membership) => [membership.organizationId, membership]));
```

Replace

```ts
    const response = cipherToResponse(cipher, mergedAttachments.get(cipher.id) || [], { preserveRepairableUris, validFolderIds });
```

with

```ts
    // edit, viewPassword and collectionIds are this reader's, from the
    // collections they hold the item through.
    const viewer = viewerForMember(cipher, membershipByOrg);
    const response = cipherToResponse(cipher, mergedAttachments.get(cipher.id) || [], { preserveRepairableUris, validFolderIds, viewer });
```

- [ ] **Step 7: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 16`, `ℹ pass 16`, `ℹ fail 0`.

Run: `pnpm run test:organizations`
Expected: `ℹ pass 11`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 8: Commit**

```bash
git add src/services/org-collection-access.ts src/services/storage-org-access.ts src/handlers/ciphers.ts src/handlers/sync.ts scripts/sharing-viewer-access.test.ts
git commit -m "An organization item says what this reader may do with it" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The server enforces read-only

**Files:**
- Modify: `src/services/storage-org-access.ts` (`CIPHER_EDIT_FORBIDDEN`, `resolveEditableCipher`, `validateOrgCollectionsForCipher`)
- Modify: `src/handlers/ciphers.ts` (`mergeCipherUpdateBody`, the organization-change rule, `handleUpdateCipher`, delete/restore handlers, `applyBulkOrgCipherOp`)
- Modify: `src/handlers/attachments.ts` (attachment writes)
- Test: `scripts/sharing-read-only.test.ts`

**Interfaces:**
- Consumes: `CipherAccessResult.canEdit` / `canManageCollections` (Task 2); `writableCollectionIds` (Task 1).
- Produces:
  - `storage-org-access.ts`: `CIPHER_EDIT_FORBIDDEN = 'You do not have permission to edit this item'`; `resolveEditableCipher(db: D1Database, userId: string, cipherId: string): Promise<{ ok: true; access: CipherAccessResult } | { ok: false; status: 403 | 404; error: string }>`. `validateOrgCollectionsForCipher` now requires every named collection to be **writable** by a non-manager.
  - `ciphers.ts` (exported): `STALE_CIPHER_MESSAGE`, `ORGANIZATION_MISMATCH_MESSAGE`, `isStaleCipherUpdate(existingUpdatedAt: string, clientRevisionDate: string | null): boolean`, `readCipherRevisionDate(source: any): string | null`, `mergeCipherUpdateBody(existingCipher: Cipher, cipherData: any, options: { method: string; preserveRevisionDate: boolean }): { ok: true; cipher: Cipher } | { ok: false; error: string }`.

**The one rule that bends the spec.** Spec 10.3 says an update may not change `organizationId` at all, as on official servers. NodeWarden's own web vault moves an existing item into an organization, and out again, through `PUT /api/ciphers/{id}` (`webapp/src/lib/api/vault.ts` puts `organizationId` on the update body, and `VaultEditor.tsx` offers the organization select on existing items). Refusing every change would break that. So: the **author of a personal item** may file it into an organization through update (the collection write checks still apply), and **a manager or above** may take an item out; **every other change is refused** with the official sentence. That closes what the spec needs closed (a recipient taking a shared record out of the sharer's organization, or anyone moving an item between organizations) and keeps the web vault working.

**`PUT /api/ciphers/{id}/partial` is not gated,** although spec 10.3 lists "partial". For an organization item it is already a no-op that answers the cipher unchanged (folder and favourite are personal, and org rows carry neither). Refusing it would make an official client's favourite click fail for a recipient, which it does not on official servers, where folder and favourite are per user.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-read-only.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ALICE_PERSONAL, ATT, CIPHER, COL_A, ENC, ORG, PERSONAL, T0, U, call, cipherData, count,
  createHarness, fullAccess, readOnlyAccess, seedAttachment, seedCipher, seedCollection,
  seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';

// Before this, any member who could see an organization item could edit it,
// trash it, purge it or take it out of the organization. A recipient of a
// shared record must be able to read it and nothing else. Deskwarden's own
// view-only rule is a courtesy; this is the rule.

const FORBIDDEN = 'You do not have permission to edit this item';
const MISMATCH = 'Organization mismatch. Re-sync if you recently moved this item to an organization.';

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'erin');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] }); // a recipient
  seedMember(h, ORG, U.erin, { collections: [fullAccess(COL_A)] }); // an editor, not a manager
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  seedAttachment(h, ATT, CIPHER);
  return h;
}

const updateBody = (extra: Record<string, unknown> = {}) => ({
  type: 1,
  name: ENC,
  notes: null,
  login: { username: ENC, password: ENC },
  organizationId: ORG,
  collectionIds: [COL_A],
  lastKnownRevisionDate: T0,
  ...extra,
});

test('a read-only member can read the item', async () => {
  const h = fixture();
  assert.equal((await call(h, 'alice', 'GET', `/api/ciphers/${CIPHER}`)).status, 200);
});

test('a read-only member is refused every write to the item', async () => {
  const h = fixture();
  const writes: Array<[string, string, unknown?]> = [
    ['PUT', `/api/ciphers/${CIPHER}`, updateBody()],
    ['POST', `/api/ciphers/${CIPHER}`, updateBody()],
    ['PUT', `/api/ciphers/${CIPHER}/delete`],
    ['DELETE', `/api/ciphers/${CIPHER}`],
    ['DELETE', `/api/ciphers/${CIPHER}/delete`],
    ['PUT', `/api/ciphers/${CIPHER}/restore`],
    ['POST', `/api/ciphers/${CIPHER}/attachment/v2`, { fileName: ENC, key: ENC, fileSize: 1 }],
    ['POST', `/api/ciphers/${CIPHER}/attachment/${ATT}`],
    ['PUT', `/api/ciphers/${CIPHER}/attachment/${ATT}/metadata`, { fileName: ENC }],
    ['POST', `/api/ciphers/${CIPHER}/attachment/${ATT}/delete`],
    ['DELETE', `/api/ciphers/${CIPHER}/attachment/${ATT}`],
  ];
  for (const [method, path, body] of writes) {
    const res = await call(h, 'alice', method, path, body);
    assert.equal(res.status, 403, `${method} ${path} answered ${res.status}`);
    assert.equal(res.body.error, FORBIDDEN, `${method} ${path}`);
  }
  // Nothing moved.
  assert.equal(cipherData(h, CIPHER).organizationId, ORG);
  assert.equal(count(h, 'SELECT COUNT(*) FROM ciphers WHERE id = ? AND deleted_at IS NULL', CIPHER), 1);
  assert.equal(count(h, 'SELECT COUNT(*) FROM attachments WHERE id = ?', ATT), 1);
});

test('a bulk delete or purge skips an item the member may only read', async () => {
  const h = fixture();
  assert.equal((await call(h, 'alice', 'POST', '/api/ciphers/delete', { ids: [CIPHER] })).status, 204);
  assert.equal((await call(h, 'alice', 'POST', '/api/ciphers/delete-permanent', { ids: [CIPHER] })).status, 204);
  assert.equal(count(h, 'SELECT COUNT(*) FROM ciphers WHERE id = ? AND deleted_at IS NULL', CIPHER), 1);
});

test('a member with a writable collection can still edit', async () => {
  const h = fixture();
  assert.equal((await call(h, 'erin', 'PUT', `/api/ciphers/${CIPHER}`, updateBody())).status, 200);
});

test('a read-only collection cannot receive a new item', async () => {
  const h = fixture();
  const newItem = { type: 1, name: ENC, login: { username: ENC, password: ENC }, organizationId: ORG, collectionIds: [COL_A] };
  const refused = await call(h, 'alice', 'POST', '/api/ciphers', newItem);
  assert.equal(refused.status, 400);
  assert.equal(refused.body.error, 'You do not have access to one of the selected collections');
  assert.equal((await call(h, 'erin', 'POST', '/api/ciphers', newItem)).status, 200);
});

test('an update may not take an item out of its organization, or into another', async () => {
  const h = fixture();
  for (const organizationId of [null, 'bbbbbbbb-0000-4000-8000-0000000000ee']) {
    const res = await call(h, 'erin', 'PUT', `/api/ciphers/${CIPHER}`, updateBody({ organizationId, collectionIds: [] }));
    assert.equal(res.status, 400);
    assert.equal(res.body.error, MISMATCH);
  }
  assert.equal(cipherData(h, CIPHER).organizationId, ORG);
});

test("the web vault's own moves through update still work", async () => {
  const h = fixture();
  // A manager takes an item out; the people who could see it are told.
  const out = await call(h, 'owner', 'PUT', `/api/ciphers/${CIPHER}`, updateBody({ organizationId: null, collectionIds: [] }));
  assert.equal(out.status, 200);
  assert.equal(cipherData(h, CIPHER).organizationId, null);
  assert.equal(count(h, 'SELECT COUNT(*) FROM collection_items WHERE cipher_id = ?', CIPHER), 0);
  assert.equal(wasBumped(h, U.alice), true);

  // The author files a personal item into a collection they may write.
  seedCipher(h, PERSONAL, U.erin);
  assert.equal((await call(h, 'erin', 'PUT', `/api/ciphers/${PERSONAL}`, updateBody())).status, 200);
  assert.equal(cipherData(h, PERSONAL).organizationId, ORG);

  // Not into one they may only read.
  seedCipher(h, ALICE_PERSONAL, U.alice);
  const refused = await call(h, 'alice', 'PUT', `/api/ciphers/${ALICE_PERSONAL}`, updateBody());
  assert.equal(refused.status, 400);
  assert.equal(refused.body.error, 'You do not have access to one of the selected collections');
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL in `sharing-read-only.test.ts`. `a read-only member is refused every write to the item` fails with `PUT /api/ciphers/... answered 200`; `a bulk delete or purge skips ...`, `a read-only collection cannot receive a new item` (`200 !== 400`), `an update may not take an item out ...` and `the web vault's own moves ...` (the last refusal answers 200) fail too. The 16 earlier tests pass.

- [ ] **Step 3: Gate writes in the access layer**

In `src/services/storage-org-access.ts`:

Replace `import { computeCipherViewerAccess } from './org-collection-access';` with:

```ts
import { computeCipherViewerAccess, writableCollectionIds } from './org-collection-access';
```

Add directly after the closing brace of `resolveCipherAccess`:

```ts
export const CIPHER_EDIT_FORBIDDEN = 'You do not have permission to edit this item';

export type EditableCipherResult =
  | { ok: true; access: CipherAccessResult }
  | { ok: false; status: 403 | 404; error: string };

// Access for a write: seeing the item is not enough, the viewer must be able to
// edit it (a manager, or a member holding a writable collection that contains
// it). A recipient of a shared record holds it read-only.
export async function resolveEditableCipher(db: D1Database, userId: string, cipherId: string): Promise<EditableCipherResult> {
  const access = await resolveCipherAccess(db, userId, cipherId);
  if (!access) return { ok: false, status: 404, error: 'Cipher not found' };
  if (!access.canEdit) return { ok: false, status: 403, error: CIPHER_EDIT_FORBIDDEN };
  return { ok: true, access };
}
```

Replace the whole `validateOrgCollectionsForCipher` function with:

```ts
export async function validateOrgCollectionsForCipher(
  db: D1Database,
  userId: string,
  organizationId: string | null,
  collectionIds: string[] | null | undefined
): Promise<{ ok: boolean; error?: string; member?: OrganizationMember | null }> {
  if (!organizationId) return { ok: true };
  const member = await getOrganizationMember(db, organizationId, userId);
  if (!member || member.status !== 1) {
    return { ok: false, error: 'Organization not found' };
  }
  const allCollections = canAccessAllCollections(member);
  if (!collectionIds || collectionIds.length === 0) {
    if (allCollections) return { ok: true, member };
    return { ok: false, error: 'A collection is required for organization ciphers' };
  }
  // A collection must belong to this organization AND be writable by the user:
  // membership in a second organization must not leak a cipher across the
  // boundary, and a read-only collection must not receive items.
  const orgCollectionIds = new Set((await getCollectionsForOrganization(db, organizationId)).map((collection) => collection.id));
  const writable = allCollections ? orgCollectionIds : new Set(writableCollectionIds(member.collections));
  for (const collectionId of collectionIds) {
    if (!orgCollectionIds.has(collectionId)) {
      return { ok: false, error: 'A selected collection does not belong to this organization' };
    }
    if (!writable.has(collectionId)) {
      return { ok: false, error: 'You do not have access to one of the selected collections' };
    }
  }
  return { ok: true, member };
}
```

- [ ] **Step 4: Extract the update merge**

In `src/handlers/ciphers.ts`:

Replace

```ts
import { resolveCipherAccess, isOrgCipher, type CipherAccessResult } from '../services/storage-org-access';
```

with

```ts
import { resolveCipherAccess, resolveEditableCipher, isOrgCipher, type CipherAccessResult } from '../services/storage-org-access';
```

Change `function readCipherRevisionDate(source: any): string | null {` to `export function readCipherRevisionDate(source: any): string | null {`, and `function isStaleCipherUpdate(existingUpdatedAt: string, clientRevisionDate: string | null): boolean {` to `export function isStaleCipherUpdate(existingUpdatedAt: string, clientRevisionDate: string | null): boolean {`.

Insert the following directly above the line `// GET /api/ciphers` (it must sit above the first `export async function`, outside every handler body that `scripts/notification-pushes.test.ts` slices):

```ts
export const STALE_CIPHER_MESSAGE = 'The client copy of this cipher is out of date. Resync the client and try again.';
export const ORGANIZATION_MISMATCH_MESSAGE = 'Organization mismatch. Re-sync if you recently moved this item to an organization.';

// A whole CipherRequestModel folded onto the stored cipher. Unknown and future
// client fields pass through; server-owned fields keep their stored values.
// PUT /api/ciphers/:id and PUT /api/ciphers/:id/share both send one, so both
// merge it here. Answers the merged cipher, or the sentence for a 400.
export function mergeCipherUpdateBody(
  existingCipher: Cipher,
  cipherData: any,
  options: { method: string; preserveRevisionDate: boolean }
): { ok: true; cipher: Cipher } | { ok: false; error: string } {
  const incomingFolderId = readCipherProp<string | null>(cipherData, ['folderId', 'FolderId']);
  const incomingKey = readCipherProp<string | null>(cipherData, ['key', 'Key']);
  const incomingLogin = readCipherProp<CipherLogin | null>(cipherData, ['login', 'Login']);
  const incomingCard = readCipherProp<CipherCard | null>(cipherData, ['card', 'Card']);
  const incomingIdentity = readCipherProp<CipherIdentity | null>(cipherData, ['identity', 'Identity']);
  const incomingSecureNote = readCipherProp<CipherSecureNote | null>(cipherData, ['secureNote', 'SecureNote']);
  const incomingSshKey = readCipherProp<CipherSshKey | null>(cipherData, ['sshKey', 'SshKey']);
  const incomingBankAccount = readCipherProp<CipherBankAccount | null>(cipherData, ['bankAccount', 'BankAccount']);
  const incomingDriversLicense = readCipherProp<CipherDriversLicense | null>(cipherData, ['driversLicense', 'DriversLicense']);
  const incomingPassport = readCipherProp<CipherPassport | null>(cipherData, ['passport', 'Passport']);
  const incomingPasswordHistory = readCipherProp<PasswordHistory[] | null>(cipherData, ['passwordHistory', 'PasswordHistory']);

  if (incomingKey.present && !shouldAcceptCipherKey(incomingKey.value)) {
    return { ok: false, error: 'Cipher key encryption is not supported by this server. Resync the client and try again.' };
  }

  const nextType = Number(cipherData.type) || existingCipher.type;

  // Opaque passthrough: merge existing stored data with ALL incoming client fields.
  // Unknown/future fields from the client are preserved; server-controlled fields are protected.
  const { preserveRevisionDate: _preserveRevisionDate, PreserveRevisionDate: _pascalPreserveRevisionDate, ...cipherDataWithoutFlags } = cipherData;
  const cipher: Cipher = {
    ...existingCipher,   // start with all existing stored data (including unknowns)
    ...cipherDataWithoutFlags, // overlay all client data (including new/unknown fields)
    // Server-controlled fields (never from client)
    id: existingCipher.id,
    userId: existingCipher.userId,
    type: nextType,
    favorite: cipherData.favorite ?? existingCipher.favorite,
    reprompt: cipherData.reprompt ?? existingCipher.reprompt,
    createdAt: existingCipher.createdAt,
    updatedAt: options.preserveRevisionDate ? existingCipher.updatedAt : new Date().toISOString(),
    archivedAt: readCipherArchivedAt(cipherData, existingCipher.archivedAt ?? null),
    deletedAt: existingCipher.deletedAt,
  };
  if (incomingFolderId.present) {
    cipher.folderId = normalizeOptionalId(incomingFolderId.value);
  }
  if (incomingKey.present) {
    const normalizedIncomingKey = normalizeCipherKeyForStorage(incomingKey.value);
    cipher.key = normalizedIncomingKey || normalizeCipherKeyForStorage(existingCipher.key);
  } else {
    cipher.key = normalizeCipherKeyForStorage(existingCipher.key);
  }
  cipher.login = nextType === 1 ? (incomingLogin.present ? (incomingLogin.value ?? null) : (existingCipher.login ?? null)) : null;
  cipher.secureNote = nextType === 2 ? (incomingSecureNote.present ? (incomingSecureNote.value ?? null) : (existingCipher.secureNote ?? null)) : null;
  cipher.card = nextType === 3 ? (incomingCard.present ? (incomingCard.value ?? null) : (existingCipher.card ?? null)) : null;
  cipher.identity = nextType === 4 ? (incomingIdentity.present ? (incomingIdentity.value ?? null) : (existingCipher.identity ?? null)) : null;
  cipher.sshKey = nextType === 5 ? (incomingSshKey.present ? (incomingSshKey.value ?? null) : (existingCipher.sshKey ?? null)) : null;
  cipher.bankAccount = nextType === 6 ? (incomingBankAccount.present ? (incomingBankAccount.value ?? null) : ((existingCipher as any).bankAccount ?? null)) : null;
  cipher.driversLicense = nextType === 7 ? (incomingDriversLicense.present ? (incomingDriversLicense.value ?? null) : ((existingCipher as any).driversLicense ?? null)) : null;
  cipher.passport = nextType === 8 ? (incomingPassport.present ? (incomingPassport.value ?? null) : ((existingCipher as any).passport ?? null)) : null;
  if (incomingPasswordHistory.present) {
    cipher.passwordHistory = incomingPasswordHistory.value ?? null;
  }

  // Custom fields deletion compatibility:
  // - Accept both camelCase "fields" and PascalCase "Fields".
  // - For full update (PUT/POST on this endpoint), missing fields means cleared fields.
  //   This prevents stale custom fields from being resurrected by merge fallback.
  const incomingFields = getAliasedProp(cipherData, ['fields', 'Fields']);
  if (incomingFields.present) {
    cipher.fields = incomingFields.value ?? null;
  } else if (options.method === 'PUT' || options.method === 'POST') {
    cipher.fields = null;
  }
  normalizeCipherForStorage(cipher);
  const compatibilityError = validateCipherEncryptedFieldsForCompatibility(cipher);
  if (compatibilityError) return { ok: false, error: compatibilityError };
  return { ok: true, cipher };
}

// Moving an item between organizations, or out of one, is not an edit.
// Official servers refuse any organizationId change here (moving in is /share,
// moving out is create + delete). NodeWarden keeps the two moves its own web
// vault makes through update: the author filing a personal item into an
// organization (reconcileOrgCipherMembership still checks the collections),
// and a manager taking an item out. Anything else -- a recipient taking a
// shared record out of the sharer's organization, or any move between
// organizations -- is refused with the official sentence.
function organizationChangeError(access: CipherAccessResult, cipherData: any): string | null {
  const incoming = readCipherProp<string | null>(cipherData, ['organizationId', 'OrganizationId']);
  if (!incoming.present) return null;
  const next = normalizeOptionalId(incoming.value);
  const current = access.organizationId;
  if (next === current) return null;
  // Access to a personal item is its author's alone.
  if (!current) return null;
  if (!next && access.canManageCollections) return null;
  return ORGANIZATION_MISMATCH_MESSAGE;
}

```

- [ ] **Step 5: Rewrite the update handler on top of it**

Replace the whole `handleUpdateCipher` function (from `// PUT /api/ciphers/:id` to its closing brace) with:

```ts
// PUT /api/ciphers/:id
export async function handleUpdateCipher(request: Request, env: Env, userId: string, id: string): Promise<Response> {
  const storage = new StorageService(env.DB);
  const editable = await resolveEditableCipher(env.DB, userId, id);
  if (!editable.ok) {
    return errorResponse(editable.error, editable.status);
  }
  const access = editable.access;
  const existingCipher = access.cipher;
  const previousOrganizationId = access.organizationId;

  let body: any;
  try {
    body = await request.json();
  } catch {
    return errorResponse('Invalid JSON', 400);
  }

  // Handle nested cipher object
  // Android client sends PascalCase "Cipher" for organization ciphers
  const cipherData = body.Cipher || body.cipher || body;
  const incomingRevisionDate = readCipherRevisionDate(cipherData);
  const hasAttachmentMigrationMetadata = hasIncomingAttachmentMetadata(cipherData);
  const preserveRevisionDate =
    shouldPreserveRepairableCipherUris(request)
    && (body.preserveRevisionDate === true || cipherData.preserveRevisionDate === true);

  const organizationError = organizationChangeError(access, cipherData);
  if (organizationError) return errorResponse(organizationError, 400);

  if (!hasAttachmentMigrationMetadata && isStaleCipherUpdate(existingCipher.updatedAt, incomingRevisionDate)) {
    return errorResponse(STALE_CIPHER_MESSAGE, 400);
  }

  const merged = mergeCipherUpdateBody(existingCipher, cipherData, { method: request.method, preserveRevisionDate });
  if (!merged.ok) return errorResponse(merged.error, 400);
  const cipher = merged.cipher;

  // Prevent referencing a folder owned by another user.
  if (cipher.folderId) {
    const folderOk = await verifyFolderOwnership(storage, cipher.folderId, userId);
    if (!folderOk) return errorResponse('Folder not found', 404);
  }

  // Reconcile organization/collection membership for org ciphers.
  const orgReconcile = await reconcileOrgCipherMembership(env.DB, userId, cipher);
  if (!orgReconcile.ok) {
    return errorResponse(orgReconcile.error || 'Organization access denied', 400);
  }

  await syncIncomingAttachmentMetadata(storage, cipher.id, cipherData);
  await storage.saveCipher(cipher);
  const revisionDate = await storage.updateRevisionDate(userId);
  notifyCipherUpdateForRequest(request, env, cipher, revisionDate);
  await notifyOrgCipherChange(request, env, cipher);
  if (previousOrganizationId && previousOrganizationId !== readOrgCipherFields(cipher).organizationId) {
    // The item left its organization: everyone who could see it must drop it.
    await notifyOrgCipherMembers(env, previousOrganizationId, readActingDeviceIdentifier(request));
  }
  const attachments = await storage.getAttachmentsByCipher(cipher.id);
  const responseOptions = cipherResponseOptionsForRequest(request);

  return jsonResponse(
    cipherToResponse(cipher, attachments, responseOptions)
  );
}
```

- [ ] **Step 6: Gate delete and restore**

In each of `handleDeleteCipher`, `handleDeleteCipherCompat`, `handlePermanentDeleteCipher` and `handleRestoreCipher`, replace

```ts
  const cipher = await loadCipherWithAccess(storage, env.DB, userId, id);

  if (!cipher) {
    return errorResponse('Cipher not found', 404);
  }
```

with

```ts
  const editable = await resolveEditableCipher(env.DB, userId, id);
  if (!editable.ok) {
    return errorResponse(editable.error, editable.status);
  }
  const cipher = editable.access.cipher;
```

(`loadCipherWithAccess` stays; `handlePartialUpdateCipher` still reads through it.)

In `applyBulkOrgCipherOp`, replace

```ts
    if (!access || !access.organizationId) continue;
```

with

```ts
    // Seeing an item is not enough to trash or purge it.
    if (!access || !access.organizationId || !access.canEdit) continue;
```

- [ ] **Step 7: Gate attachment writes**

In `src/handlers/attachments.ts`:

Replace `import { resolveCipherAccess, readOrgCipherFields } from '../services/storage-org-access';` with:

```ts
import { resolveCipherAccess, resolveEditableCipher, readOrgCipherFields } from '../services/storage-org-access';
```

Add directly after the closing brace of `loadAttachmentCipher`:

```ts
// Changing an attachment changes the item, so it needs edit rights, not just
// sight: a recipient of a shared record holds it read-only.
async function loadEditableAttachmentCipher(
  env: Env,
  userId: string,
  cipherId: string
): Promise<{ ok: true; cipher: Cipher } | { ok: false; response: Response }> {
  const editable = await resolveEditableCipher(env.DB, userId, cipherId);
  if (!editable.ok) return { ok: false, response: errorResponse(editable.error, editable.status) };
  return { ok: true, cipher: editable.access.cipher };
}
```

In each of `handleCreateAttachment`, `handleUploadAttachment`, `handleUpdateAttachmentMetadata` and `handleDeleteAttachment` (**not** `handleGetAttachment`, which only reads), replace the first

```ts
  const cipher = await loadAttachmentCipher(env, userId, cipherId);
  if (!cipher) {
    return errorResponse('Cipher not found', 404);
  }
```

with

```ts
  const loaded = await loadEditableAttachmentCipher(env, userId, cipherId);
  if (!loaded.ok) {
    return loaded.response;
  }
  const cipher = loaded.cipher;
```

In `handlePublicUploadAttachment`, replace

```ts
  const cipher = await loadAttachmentCipher(env, claims.userId, cipherId);
  if (!cipher) {
    return errorResponse('Cipher not found', 404);
  }
```

with

```ts
  const loaded = await loadEditableAttachmentCipher(env, claims.userId, cipherId);
  if (!loaded.ok) {
    return loaded.response;
  }
  const cipher = loaded.cipher;
```

The later `const updatedCipher = await loadAttachmentCipher(env, userId, cipherId);` lines stay: they read the item back for the response.

- [ ] **Step 8: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 23`, `ℹ pass 23`, `ℹ fail 0`.

Run: `pnpm run test:notification-pushes`
Expected: `ℹ fail 0` (each handler still sends its own push and no whole-vault push).

Run: `pnpm run test:organizations`
Expected: `ℹ pass 11`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 9: Commit**

```bash
git add src/services/storage-org-access.ts src/handlers/ciphers.ts src/handlers/attachments.ts scripts/sharing-read-only.test.ts
git commit -m "A read-only member can read an organization item and change nothing" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Privacy: a member sees only their own collections, and no roster

**Files:**
- Modify: `src/handlers/organizations.ts` (`handleGetOrganizationMembers`, `handleGetCollections`, `handleGetCollectionItems`)
- Modify: `src/handlers/sync.ts` (`collections[]`)
- Test: `scripts/sharing-privacy.test.ts`

**Interfaces:**
- Consumes: the harness (Task 1); `canAccessAllCollections`, `getAccessibleCollectionsForUser` (existing, in `storage-organization-repo.ts`).
- Produces: behaviour only. `GET /organizations/{id}/members` is manager+ (plain members 404). `GET /organizations/{id}/collections` answers a member only their collections. `GET /organizations/{id}/collections/{cid}/items` answers a member only for a collection they hold. `/api/sync` `collections[]` holds only reachable collections. (Cipher `collectionIds` were made per-viewer in Task 2.)

Spec 10.2's table, row by row: `/members` here, `/users` and `/users/mini-details` in Task 5; sync collections, org collections and collection items here; cipher `collectionIds` in Task 2; profile `users` is already `[]`; the account-deletion successor in Task 11.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-privacy.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  CIPHER, COL_A, COL_B, ORG, U, call, createHarness, readOnlyAccess,
  seedCipher, seedCollection, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// In a sharing organization each collection is named for one person (the
// name decrypts, under the org key every member holds, to that person's
// email), and the roster is everyone the sharer shares with. A recipient must
// see neither: only their own collection, and no member list.

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });
  seedMember(h, ORG, U.bob, { collections: [readOnlyAccess(COL_B)] });
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  return h;
}

test('a plain member cannot read the member list; a manager can', async () => {
  const h = fixture();
  assert.equal((await call(h, 'alice', 'GET', `/api/organizations/${ORG}/members`)).status, 404);
  const owner = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/members`);
  assert.equal(owner.status, 200);
  assert.equal(owner.body.data.length, 3);
});

test('sync lists only the collections this member can reach', async () => {
  const h = fixture();
  const alice = await call(h, 'alice', 'GET', '/api/sync');
  assert.deepEqual(alice.body.collections.map((c: any) => c.id), [COL_A]);
  const owner = await call(h, 'owner', 'GET', '/api/sync');
  assert.deepEqual(owner.body.collections.map((c: any) => c.id).sort(), [COL_A, COL_B].sort());
});

test("an organization's collection list is filtered for a member", async () => {
  const h = fixture();
  const alice = await call(h, 'alice', 'GET', `/api/organizations/${ORG}/collections`);
  assert.equal(alice.status, 200);
  assert.deepEqual(alice.body.data.map((c: any) => c.id), [COL_A]);
  const owner = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/collections`);
  assert.deepEqual(owner.body.data.map((c: any) => c.id).sort(), [COL_A, COL_B].sort());
});

test("a member cannot list the items of someone else's collection", async () => {
  const h = fixture();
  assert.equal((await call(h, 'alice', 'GET', `/api/organizations/${ORG}/collections/${COL_B}/items`)).status, 404);
  const own = await call(h, 'alice', 'GET', `/api/organizations/${ORG}/collections/${COL_A}/items`);
  assert.equal(own.status, 200);
  assert.deepEqual(own.body.data.map((item: any) => item.id), [CIPHER]);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 4 failures in `sharing-privacy.test.ts` (`200 !== 404` for the roster and items; the collection lists hold both ids). The 23 earlier tests pass.

- [ ] **Step 3: Gate the organization routes**

In `src/handlers/organizations.ts`, in the import list from `'../services/storage-organization-repo'`, add `canAccessAllCollections,` after `orgRoleRank,`.

In `handleGetOrganizationMembers`, replace

```ts
  const member = await requireMember(env, orgId, userId);
  if (!member) return errorResponse('Not found', 404);

  const members = await getOrganizationMembers(env.DB, orgId);
```

with

```ts
  // Managers only, as on Vaultwarden. In a sharing organization a plain member
  // is a person something was shared with, and the roster is everyone else.
  const member = await requireMember(env, orgId, userId);
  if (!member || !isOrgManager(member)) return errorResponse('Not found', 404);

  const members = await getOrganizationMembers(env.DB, orgId);
```

Replace the whole `handleGetCollections` function (from `// GET /api/organizations/:id/collections` to its closing brace) with:

```ts
// GET /api/organizations/:id/collections
export async function handleGetCollections(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  const member = await requireMember(env, orgId, userId);
  if (!member) return errorResponse('Not found', 404);

  const collections = await getCollectionsForOrganization(env.DB, orgId);
  // A member sees only the collections they hold. Every member holds the key
  // that decrypts collection names, and in a sharing organization each name is
  // one person's email.
  const reachable = canAccessAllCollections(member)
    ? collections
    : collections.filter((collection) => member.collections.some((entry) => entry.id === collection.id));
  return jsonResponse({
    data: reachable.map(toCollectionResponse),
    object: 'list',
    continuationToken: null,
  });
}
```

In `handleGetCollectionItems`, replace

```ts
  const collection = await getCollection(env.DB, collectionId);
  if (!collection || collection.organizationId !== orgId) return errorResponse('Not found', 404);

  const items = await getCollectionItems(env.DB, collectionId);
```

with

```ts
  const collection = await getCollection(env.DB, collectionId);
  if (!collection || collection.organizationId !== orgId) return errorResponse('Not found', 404);
  if (!canAccessAllCollections(member) && !member.collections.some((entry) => entry.id === collectionId)) {
    return errorResponse('Not found', 404);
  }

  const items = await getCollectionItems(env.DB, collectionId);
```

- [ ] **Step 4: Sync lists only reachable collections**

In `src/handlers/sync.ts`:

Replace `import { getCollectionsForOrganization } from '../services/storage-organization-repo';` with:

```ts
import { getAccessibleCollectionsForUser } from '../services/storage-organization-repo';
```

Replace

```ts
  const [ciphers, folders, sends, domainSettings, organizations, orgCiphers, memberships] = await Promise.all([
```

with

```ts
  const [ciphers, folders, sends, domainSettings, organizations, orgCiphers, memberships, accessibleCollections] = await Promise.all([
```

and, in the same `Promise.all` list, replace

```ts
    getOrganizationMembershipsForUser(env.DB, userId),
  ]);
```

with

```ts
    getOrganizationMembershipsForUser(env.DB, userId),
    getAccessibleCollectionsForUser(env.DB, userId),
  ]);
```

Replace this block:

```ts
  const collectionResponses: CollectionResponse[] = [];
  const policyResponses: PolicyResponse[] = [];
  for (const org of organizations) {
    const [collections, policies] = await Promise.all([
      getCollectionsForOrganization(env.DB, org.id),
      getPoliciesForOrganization(env.DB, org.id),
    ]);
    for (const collection of collections) {
      collectionResponses.push({
        id: collection.id,
        organizationId: collection.organizationId,
        name: collection.name,
        externalId: collection.externalId,
        creationDate: collection.createdAt,
        revisionDate: collection.updatedAt,
        object: 'collection',
      });
    }
    for (const policy of policies) {
```

with

```ts
  // Only the collections this user can reach. Every member holds the org key
  // that decrypts collection names, and in a sharing organization each name is
  // one person's email: the whole list would be the sharer's address book.
  const collectionResponses: CollectionResponse[] = accessibleCollections.map((collection) => ({
    id: collection.id,
    organizationId: collection.organizationId,
    name: collection.name,
    externalId: collection.externalId,
    creationDate: collection.createdAt,
    revisionDate: collection.updatedAt,
    object: 'collection' as const,
  }));
  const policyResponses: PolicyResponse[] = [];
  for (const org of organizations) {
    const policies = await getPoliciesForOrganization(env.DB, org.id);
    for (const policy of policies) {
```

(The policy loop body and the two closing braces after it stay as they are.)

- [ ] **Step 5: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 27`, `ℹ pass 27`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add src/handlers/organizations.ts src/handlers/sync.ts scripts/sharing-privacy.test.ts
git commit -m "A member sees their own collections and not the organization's roster" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Official roster, invite, re-invite and remove

**Files:**
- Create: `src/services/org-user-model.ts`
- Create: `src/handlers/organization-users.ts`
- Modify: `src/services/storage-organization-repo.ts` (`getOrganizationManagerIds`)
- Modify: `src/handlers/organizations.ts` (`bumpOrgMemberAndManagers`, `writeOrgAudit` exported, `removeOrganizationMember`, `revokeOrganizationInvitation`)
- Modify: `src/router-authenticated.ts`
- Test: `scripts/sharing-org-users.test.ts`

**Interfaces:**
- Consumes: `normalizeCollectionAccessList` (Task 1), `CollectionAccess` (Task 1).
- Produces:
  - `org-user-model.ts`: `ORG_INVITATION_TTL_MS`; `orgUserTypeForRole(role: unknown): number` (owner 0, admin 1, member 2, manager 3, custom 4; unknown 2); `orgRoleForUserType(type: unknown): string | null`; `orgUserStatusForMember(member: { encryptedOrgKey: string | null }): 1 | 2`; `newInvitationToken(): string`; `invitationExpiry(nowMs?: number): string`; `type OrgUserRef = { kind: 'member'; member: OrganizationMember } | { kind: 'invitation'; invitation: OrganizationInvitation }`; `resolveOrgUser(db: D1Database, orgId: string, ouId: string): Promise<OrgUserRef | null>` (accepted members and pending invitations only).
  - `storage-organization-repo.ts`: `getOrganizationManagerIds(db: D1Database, organizationId: string): Promise<string[]>`.
  - `organizations.ts` (exported): `writeOrgAudit(env, actorUserId, action, targetType, targetId, metadata)`; `bumpOrgMemberAndManagers(env: Env, orgId: string, memberUserIds: Array<string | null | undefined>, contextId?: string | null): Promise<void>`; `removeOrganizationMember(request: Request, env: Env, actor: OrganizationMember, orgId: string, target: OrganizationMember): Promise<Response>`; `revokeOrganizationInvitation(request: Request, env: Env, actorUserId: string, orgId: string, invitation: OrganizationInvitation): Promise<Response>`.
  - `organization-users.ts`: `handleListOrganizationUsers`, `handleListOrganizationUserMiniDetails`, `handleInviteOrganizationUsers`, `handleReinviteOrganizationUser`, `handleRemoveOrganizationUser`, plus the private helpers `requireManager(env, orgId, userId): Promise<OrganizationMember | null>` and `readJson(request): Promise<any | null>` that Tasks 6 and 12 reuse.
  - Routes: `GET /api/organizations/{orgId}/users[?includeCollections=true]`, `GET .../users/mini-details`, `POST .../users/invite`, `POST .../users/{ouId}/reinvite`, `DELETE .../users/{ouId}`.

`/users` lists accepted members (status 1 without a key, 2 with one) and pending invitations (status 0, `userId: null`, with `invitationExpirationDate`, a NodeWarden extension). Member rows with status 0 (SCIM-staged or revoked) are left out: they grant nothing, and SCIM stages one beside an invitation for the same email, which would list one person twice.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-org-users.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  COL_A, COL_B, COL_X, INV_CAROL, ORG, U, call, count, createHarness, emailOf,
  invitationCollectionsRaw, readOnlyAccess, resetRevisions, seedCollection, seedInvitation,
  seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';

// Bitwarden's own membership routes, on NodeWarden's tables. Deskwarden reads
// the roster from /users on every server, so the statuses must mean what
// official ones mean: 0 invited, 1 accepted (no key yet), 2 confirmed.

const CAROL_EXPIRY = new Date(Date.now() + 3 * 24 * 60 * 60 * 1000).toISOString();

function rosterFixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedUser(h, 'dave');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] }); // confirmed
  seedMember(h, ORG, U.bob, { collections: [readOnlyAccess(COL_B)], key: null }); // accepted, not confirmed
  seedInvitation(h, ORG, INV_CAROL, emailOf('carol'), {
    invitedBy: U.owner,
    collections: [readOnlyAccess(COL_A)],
    expiresAt: CAROL_EXPIRY,
  });
  return h;
}

test('the roster reports members, states and collection flags as official servers do', async () => {
  const h = rosterFixture();
  const res = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users?includeCollections=true`);
  assert.equal(res.status, 200);
  assert.equal(res.body.object, 'list');
  assert.equal(res.body.data.length, 4);
  const byEmail = new Map<string, any>(res.body.data.map((row: any) => [row.email, row]));

  const owner = byEmail.get(emailOf('owner'));
  assert.equal(owner.object, 'organizationUserUserDetails');
  assert.equal(owner.id, U.owner);
  assert.equal(owner.userId, U.owner);
  assert.equal(owner.type, 0);
  assert.equal(owner.status, 2);

  const alice = byEmail.get(emailOf('alice'));
  assert.equal(alice.type, 2);
  assert.equal(alice.status, 2);
  assert.deepEqual(alice.collections, [readOnlyAccess(COL_A)]);
  assert.equal(alice.invitationExpirationDate, null);

  assert.equal(byEmail.get(emailOf('bob')).status, 1);

  const carol = byEmail.get(emailOf('carol'));
  assert.equal(carol.id, INV_CAROL);
  assert.equal(carol.userId, null);
  assert.equal(carol.status, 0);
  assert.equal(carol.invitationExpirationDate, CAROL_EXPIRY);
  assert.deepEqual(carol.collections, [readOnlyAccess(COL_A)]);
});

test('without includeCollections the roster carries no collections', async () => {
  const h = rosterFixture();
  const res = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users`);
  assert.equal(res.status, 200);
  for (const row of res.body.data) assert.deepEqual(row.collections, []);
});

test('a plain member can read neither the roster nor the mini roster', async () => {
  const h = rosterFixture();
  assert.equal((await call(h, 'alice', 'GET', `/api/organizations/${ORG}/users`)).status, 404);
  assert.equal((await call(h, 'alice', 'GET', `/api/organizations/${ORG}/users/mini-details`)).status, 404);

  const mini = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users/mini-details`);
  assert.equal(mini.status, 200);
  assert.equal(mini.body.data.length, 4);
  for (const row of mini.body.data) {
    assert.equal(row.object, 'organizationUserUserMiniDetails');
    assert.deepEqual(Object.keys(row).sort(), ['email', 'id', 'name', 'object', 'status', 'type', 'userId']);
  }
});

test('an official invite always makes an invitation, skips people already in, and tells managers', async () => {
  const h = rosterFixture();
  resetRevisions(h);
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: [emailOf('dave'), emailOf('alice'), 'Frank@Example.test', emailOf('carol')],
    type: 2,
    collections: [readOnlyAccess(COL_A)],
    groups: [],
    permissions: null,
    accessSecretsManager: false,
  });
  assert.equal(res.status, 200);
  assert.equal(res.body, null);

  const emails = (h.db.prepare('SELECT email FROM organization_invitations WHERE organization_id = ? ORDER BY email').all(ORG) as any[])
    .map((row) => row.email);
  // dave has an account and still gets an invitation (official semantics:
  // no direct membership on this route); alice is a member and carol already
  // holds a live invitation, so both are skipped.
  assert.deepEqual(emails, ['carol@example.test', 'dave@example.test', 'frank@example.test']);
  const dave = h.db.prepare('SELECT id FROM organization_invitations WHERE email = ?').get(emailOf('dave')) as any;
  assert.deepEqual(invitationCollectionsRaw(h, dave.id), [readOnlyAccess(COL_A)]);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE user_id = ?', U.dave), 0);

  assert.equal(wasBumped(h, U.owner), true);
  assert.equal(wasBumped(h, U.dave), true);
  assert.equal(wasBumped(h, U.alice), false);

  const mine = await call(h, 'dave', 'GET', '/api/organizations/invitations/mine');
  assert.equal(mine.body.data.length, 1);
});

test('an invite is refused a bad type, a bad email, no email, a foreign collection or a role above the inviter', async () => {
  const h = rosterFixture();
  seedUser(h, 'erin');
  seedMember(h, ORG, U.erin, { role: 'manager' });
  const invite = (person: 'owner' | 'erin', body: Record<string, unknown>) =>
    call(h, person, 'POST', `/api/organizations/${ORG}/users/invite`, { emails: [emailOf('dave')], type: 2, collections: [], ...body });

  const cases: Array<[Promise<any>, number, string]> = [
    [invite('owner', { type: 9 }), 400, 'A valid member type is required'],
    [invite('owner', { type: undefined }), 400, 'A valid member type is required'],
    [invite('owner', { emails: [] }), 400, 'At least one email is required'],
    [invite('owner', { emails: ['not-an-email'] }), 400, 'Valid email is required'],
    [invite('owner', { collections: [readOnlyAccess(COL_X)] }), 400, 'A selected collection does not belong to this organization'],
    [invite('erin', { type: 1 }), 403, 'You cannot grant a role above your own'],
  ];
  for (const [pending, status, message] of cases) {
    const res = await pending;
    assert.equal(res.status, status, message);
    assert.equal(res.body.error, message);
  }
});

test('a re-invite rotates the token and restarts the seven days', async () => {
  const h = rosterFixture();
  h.db.prepare('UPDATE organization_invitations SET token = ?, expires_at = ? WHERE id = ?')
    .run('old-token', '2026-01-02T00:00:00.000Z', INV_CAROL);

  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${INV_CAROL}/reinvite`);
  assert.equal(res.status, 200);
  const row = h.db.prepare('SELECT token, expires_at FROM organization_invitations WHERE id = ?').get(INV_CAROL) as any;
  assert.notEqual(row.token, 'old-token');
  assert.ok(Date.parse(row.expires_at) > Date.now() + 6 * 24 * 60 * 60 * 1000, row.expires_at);

  const member = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${U.alice}/reinvite`);
  assert.equal(member.status, 400);
  assert.equal(member.body.error, 'User is not in an invited state');
});

test('remove takes an invitation or a member, never the owner, and only for managers', async () => {
  const h = rosterFixture();
  assert.equal((await call(h, 'alice', 'DELETE', `/api/organizations/${ORG}/users/${U.bob}`)).status, 404);

  assert.equal((await call(h, 'owner', 'DELETE', `/api/organizations/${ORG}/users/${INV_CAROL}`)).status, 204);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE id = ?', INV_CAROL), 0);

  resetRevisions(h);
  assert.equal((await call(h, 'owner', 'DELETE', `/api/organizations/${ORG}/users/${U.bob}`)).status, 204);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND user_id = ?', ORG, U.bob), 0);
  assert.equal(wasBumped(h, U.bob), true);

  const owner = await call(h, 'owner', 'DELETE', `/api/organizations/${ORG}/users/${U.owner}`);
  assert.equal(owner.status, 403);
  assert.equal(owner.body.error, 'The organization owner cannot be removed');
  assert.equal((await call(h, 'owner', 'DELETE', `/api/organizations/${ORG}/users/aaaaaaaa-0000-4000-8000-0000000000ff`)).status, 404);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 7 failures in `sharing-org-users.test.ts` (the routes do not exist: `/users` answers `404 Not found`, so `404 !== 200` and similar). The 27 earlier tests pass.

- [ ] **Step 3: Add the manager lookup to the repo**

In `src/services/storage-organization-repo.ts`, add directly after `getOrganizationMemberIds`:

```ts
// The accepted owners, admins and managers: who hears about every membership change.
export async function getOrganizationManagerIds(db: D1Database, organizationId: string): Promise<string[]> {
  const res = await db
    .prepare("SELECT user_id FROM organization_members WHERE organization_id = ? AND status = 1 AND role IN ('owner', 'admin', 'manager')")
    .bind(organizationId)
    .all<{ user_id: string }>();
  return (res.results || []).map((r) => r.user_id);
}
```

- [ ] **Step 4: Write the model module**

Create `src/services/org-user-model.ts`:

```ts
import type { OrganizationInvitation, OrganizationMember } from '../types';
import { generateUUID } from '../utils/uuid';
import { getOrganizationInvitation, getOrganizationMember } from './storage-organization-repo';

// NodeWarden's membership rows in Bitwarden's terms. Kept free of the
// notifications hub so profile-response.ts can use it and still load in a
// plain `tsx --test` run.

// Bitwarden's OrganizationUserType, as official clients read it.
const ROLE_TO_TYPE: Record<string, number> = { owner: 0, admin: 1, member: 2, manager: 3, custom: 4 };
const TYPE_TO_ROLE = ['owner', 'admin', 'member', 'manager', 'custom'] as const;

export const ORG_INVITATION_TTL_MS = 7 * 24 * 60 * 60 * 1000;

export function orgUserTypeForRole(role: unknown): number {
  return ROLE_TO_TYPE[String(role || '').trim().toLowerCase()] ?? 2;
}

export function orgRoleForUserType(type: unknown): string | null {
  if (type === null || type === undefined || type === '') return null;
  const value = Number(type);
  return Number.isInteger(value) && value >= 0 && value < TYPE_TO_ROLE.length ? TYPE_TO_ROLE[value] : null;
}

// Bitwarden's OrganizationUserStatusType for an accepted NodeWarden member:
// Confirmed (2) once it holds its wrapped copy of the organization key,
// Accepted (1) until then. An invitation is Invited (0).
export function orgUserStatusForMember(member: Pick<OrganizationMember, 'encryptedOrgKey'>): 1 | 2 {
  return member.encryptedOrgKey ? 2 : 1;
}

export function newInvitationToken(): string {
  return generateUUID() + generateUUID();
}

export function invitationExpiry(nowMs: number = Date.now()): string {
  return new Date(nowMs + ORG_INVITATION_TTL_MS).toISOString();
}

export type OrgUserRef =
  | { kind: 'member'; member: OrganizationMember }
  | { kind: 'invitation'; invitation: OrganizationInvitation };

// An official organizationUserId (ouId). NodeWarden keys members by user id and
// invitations by their own id, so an ouId is one or the other; both are UUIDs
// from separate generators. Only accepted members and pending invitations
// resolve: a status-0 member row (SCIM-staged or revoked) grants nothing.
export async function resolveOrgUser(db: D1Database, orgId: string, ouId: string): Promise<OrgUserRef | null> {
  const member = await getOrganizationMember(db, orgId, ouId);
  if (member && member.status === 1) return { kind: 'member', member };
  const invitation = await getOrganizationInvitation(db, ouId);
  if (invitation && invitation.organizationId === orgId && invitation.status === 0) return { kind: 'invitation', invitation };
  return null;
}
```

- [ ] **Step 5: Share the push, audit, remove and revoke logic in `organizations.ts`**

In `src/handlers/organizations.ts`:

In the import list from `'../services/storage-organization-repo'`, add `getOrganizationManagerIds,` after `getOrganizationMemberIds,`.

Change `async function writeOrgAudit(` to `export async function writeOrgAudit(`.

Add directly after the closing brace of `bumpMembersRevision`:

```ts
// A membership change reaches the person it is about and every manager, so a
// sharer's clients hear an accept at once instead of at their next poll.
export async function bumpOrgMemberAndManagers(
  env: Env,
  orgId: string,
  memberUserIds: Array<string | null | undefined>,
  contextId?: string | null
): Promise<void> {
  const managerIds = await getOrganizationManagerIds(env.DB, orgId);
  const memberIds = memberUserIds.filter((id): id is string => !!id);
  await bumpMembersRevision(env, [...managerIds, ...memberIds], contextId);
}
```

Replace the whole `handleRemoveOrganizationMember` function (from `// DELETE /api/organizations/:id/members/:userId` to its closing brace) with:

```ts
// Drops one accepted member. Shared by DELETE /members/:userId and the
// official DELETE /users/:ouId.
export async function removeOrganizationMember(
  request: Request,
  env: Env,
  actor: OrganizationMember,
  orgId: string,
  target: OrganizationMember
): Promise<Response> {
  if (target.role === 'owner') return errorResponse('The organization owner cannot be removed', 403);
  if (actor.userId === target.userId) return errorResponse('You cannot remove yourself', 403);
  // Removal is also rank-gated: a manager must not delete an admin.
  if (orgRoleRank(target.role) > orgRoleRank(actor.role)) {
    return errorResponse('You cannot remove a member with a higher role than your own', 403);
  }

  // Org ciphers stay with the organization; only the membership row is dropped.
  await deleteOrganizationMember(env.DB, orgId, target.userId);
  await bumpOrgMemberAndManagers(env, orgId, [target.userId], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, actor.userId, 'organization.member.remove', 'organization', orgId, {
    memberId: target.userId,
    organizationId: orgId,
  });

  return new Response(null, { status: 204 });
}

// DELETE /api/organizations/:id/members/:userId
export async function handleRemoveOrganizationMember(request: Request, env: Env, userId: string, orgId: string, memberUserId: string): Promise<Response> {
  const member = await requireMember(env, orgId, userId);
  if (!member || !isOrgManager(member)) return errorResponse('Not found', 404);

  const targetMember = await getOrganizationMember(env.DB, orgId, memberUserId);
  if (!targetMember) return errorResponse('Not found', 404);

  return removeOrganizationMember(request, env, member, orgId, targetMember);
}
```

Replace the whole `handleRevokeOrganizationInvitation` function (from `// DELETE /api/organizations/:id/invitations/:invitationId` to its closing brace) with:

```ts
// Drops one pending invitation. Shared by DELETE /invitations/:id and the
// official DELETE /users/:ouId.
export async function revokeOrganizationInvitation(
  request: Request,
  env: Env,
  actorUserId: string,
  orgId: string,
  invitation: OrganizationInvitation
): Promise<Response> {
  await deleteOrganizationInvitation(env.DB, invitation.id);
  const invitee = await getUser(env.DB, invitation.email);
  await bumpOrgMemberAndManagers(env, orgId, [invitee?.id], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, actorUserId, 'organization.revokeInvitation', 'organization', orgId, {
    email: invitation.email,
    organizationId: orgId,
  });

  return new Response(null, { status: 204 });
}

// DELETE /api/organizations/:id/invitations/:invitationId
export async function handleRevokeOrganizationInvitation(request: Request, env: Env, userId: string, orgId: string, invitationId: string): Promise<Response> {
  const member = await requireMember(env, orgId, userId);
  if (!member || !isOrgManager(member)) return errorResponse('Not found', 404);

  const inv = await getOrganizationInvitation(env.DB, invitationId);
  if (!inv || inv.organizationId !== orgId) return errorResponse('Not found', 404);

  return revokeOrganizationInvitation(request, env, userId, orgId, inv);
}
```

- [ ] **Step 6: Write the official routes**

Create `src/handlers/organization-users.ts`:

```ts
import type { CollectionAccess, Env, OrganizationInvitation, OrganizationMember } from '../types';
import { errorResponse, jsonResponse } from '../utils/response';
import { generateUUID } from '../utils/uuid';
import { readActingDeviceIdentifier } from '../utils/device';
import { getUser, getUsersByIds } from '../services/storage-user-repo';
import {
  canAssignOrgRole,
  deleteOrganizationInvitation,
  getCollectionsForOrganization,
  getOrganizationInvitations,
  getOrganizationMember,
  getOrganizationMembers,
  isOrgManager,
  saveOrganizationInvitation,
} from '../services/storage-organization-repo';
import { normalizeCollectionAccessList } from '../services/org-collection-access';
import {
  invitationExpiry,
  newInvitationToken,
  orgRoleForUserType,
  orgUserStatusForMember,
  orgUserTypeForRole,
  resolveOrgUser,
} from '../services/org-user-model';
import {
  bumpOrgMemberAndManagers,
  removeOrganizationMember,
  revokeOrganizationInvitation,
  writeOrgAudit,
} from './organizations';

// Bitwarden's official organization-user routes (/organizations/{orgId}/users/...),
// on NodeWarden's member and invitation tables. An official organizationUserId
// (ouId) is a member's user id or an invitation's own id; see resolveOrgUser.
//
// Every route here except accept and leave is for managers. A plain member gets
// 404, as everywhere else in the organization routes: in a sharing organization
// a plain member is a person something was shared with, and the roster is
// everyone else the sharer shares with.

const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

async function requireManager(env: Env, orgId: string, userId: string): Promise<OrganizationMember | null> {
  const member = await getOrganizationMember(env.DB, orgId, userId);
  return member && isOrgManager(member) ? member : null;
}

async function readJson(request: Request): Promise<any | null> {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

interface OrgUserRow {
  id: string;
  userId: string | null;
  email: string;
  name: string | null;
  role: string;
  status: number;
  collections: CollectionAccess[];
  invitationExpirationDate: string | null;
}

// Accepted members, then pending invitations (expired ones too: the sharer's
// client shows them as expired).
async function loadOrgUserRows(env: Env, orgId: string): Promise<OrgUserRow[]> {
  const [members, invitations] = await Promise.all([
    getOrganizationMembers(env.DB, orgId),
    getOrganizationInvitations(env.DB, orgId),
  ]);
  const accepted = members.filter((member) => member.status === 1);
  const users = await getUsersByIds(env.DB, accepted.map((member) => member.userId));
  const rows: OrgUserRow[] = [];
  for (const member of accepted) {
    const user = users.get(member.userId);
    if (!user) continue;
    rows.push({
      id: member.userId,
      userId: member.userId,
      email: user.email,
      name: user.name,
      role: member.role,
      status: orgUserStatusForMember(member),
      collections: member.collections,
      invitationExpirationDate: null,
    });
  }
  for (const invitation of invitations) {
    if (invitation.status !== 0) continue;
    rows.push({
      id: invitation.id,
      userId: null,
      email: invitation.email,
      name: null,
      role: invitation.role,
      status: 0,
      collections: invitation.collections,
      invitationExpirationDate: invitation.expiresAt,
    });
  }
  return rows;
}

function toOrgUserDetails(row: OrgUserRow, includeCollections: boolean): Record<string, unknown> {
  return {
    object: 'organizationUserUserDetails',
    id: row.id,
    userId: row.userId,
    email: row.email,
    name: row.name,
    type: orgUserTypeForRole(row.role),
    status: row.status,
    collections: includeCollections
      ? row.collections.map(({ id, readOnly, hidePasswords, manage }) => ({ id, readOnly, hidePasswords, manage }))
      : [],
    groups: [],
    permissions: {},
    accessAll: false,
    externalId: null,
    twoFactorEnabled: false,
    resetPasswordEnrolled: false,
    usesKeyConnector: false,
    hasMasterPassword: true,
    // NodeWarden extension, invitations only: when the invite stops working.
    invitationExpirationDate: row.invitationExpirationDate,
  };
}

// GET /api/organizations/:orgId/users[?includeCollections=true]
export async function handleListOrganizationUsers(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const includeCollections = /^(1|true)$/i.test(new URL(request.url).searchParams.get('includeCollections') || '');
  const rows = await loadOrgUserRows(env, orgId);
  return jsonResponse({
    object: 'list',
    data: rows.map((row) => toOrgUserDetails(row, includeCollections)),
    continuationToken: null,
  });
}

// GET /api/organizations/:orgId/users/mini-details
// Official servers answer any member here, so a recipient can list every other
// recipient. NodeWarden answers managers only, as Vaultwarden does.
export async function handleListOrganizationUserMiniDetails(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  void request;
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const rows = await loadOrgUserRows(env, orgId);
  return jsonResponse({
    object: 'list',
    data: rows.map((row) => ({
      object: 'organizationUserUserMiniDetails',
      id: row.id,
      userId: row.userId,
      type: orgUserTypeForRole(row.role),
      status: row.status,
      name: row.name,
      email: row.email,
    })),
    continuationToken: null,
  });
}

// POST /api/organizations/:orgId/users/invite
// Official semantics: always a status-0 invitation, even for an existing
// account, and an empty 200. People already in the organization, or holding an
// invitation that still works, are skipped silently. NodeWarden's direct add
// (no accept step) stays on POST /members/invite.
export async function handleInviteOrganizationUsers(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  const actor = await requireManager(env, orgId, userId);
  if (!actor) return errorResponse('Not found', 404);

  const body = await readJson(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);

  const emails: string[] = Array.isArray(body.emails)
    ? Array.from(new Set(body.emails.map((email: unknown) => String(email || '').trim().toLowerCase()).filter(Boolean))) as string[]
    : [];
  if (!emails.length) return errorResponse('At least one email is required', 400);
  if (emails.some((email) => !EMAIL_PATTERN.test(email))) return errorResponse('Valid email is required', 400);

  const role = orgRoleForUserType(body.type);
  if (!role) return errorResponse('A valid member type is required', 400);
  if (!canAssignOrgRole(actor, role)) return errorResponse('You cannot grant a role above your own', 403);

  const collections = normalizeCollectionAccessList(body.collections);
  const orgCollectionIds = new Set((await getCollectionsForOrganization(env.DB, orgId)).map((collection) => collection.id));
  if (collections.some((entry) => !orgCollectionIds.has(entry.id))) {
    return errorResponse('A selected collection does not belong to this organization', 400);
  }

  const rows = await loadOrgUserRows(env, orgId);
  const nowMs = Date.now();
  const now = new Date(nowMs).toISOString();
  const invitees: Array<string | null> = [];
  for (const email of emails) {
    const existing = rows.filter((row) => row.email.toLowerCase() === email);
    const alreadyIn = existing.some(
      (row) => row.userId !== null || (row.invitationExpirationDate !== null && Date.parse(row.invitationExpirationDate) >= nowMs)
    );
    if (alreadyIn) continue;
    // An expired invitation for the same address is replaced, not duplicated.
    for (const row of existing) await deleteOrganizationInvitation(env.DB, row.id);

    const invitation: OrganizationInvitation = {
      id: generateUUID(),
      organizationId: orgId,
      email,
      role,
      permissions: {},
      collections,
      invitedBy: userId,
      status: 0,
      token: newInvitationToken(),
      expiresAt: invitationExpiry(nowMs),
      createdAt: now,
      updatedAt: now,
    };
    await saveOrganizationInvitation(env.DB, invitation);
    await writeOrgAudit(env, userId, 'organization.invite', 'organization', orgId, {
      email,
      role,
      organizationId: orgId,
      collectionCount: collections.length,
    });
    invitees.push((await getUser(env.DB, email))?.id ?? null);
  }

  await bumpOrgMemberAndManagers(env, orgId, invitees, readActingDeviceIdentifier(request));
  return new Response(null, { status: 200 });
}

// POST /api/organizations/:orgId/users/:ouId/reinvite
export async function handleReinviteOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return errorResponse('Not found', 404);
  if (target.kind !== 'invitation') return errorResponse('User is not in an invited state', 400);

  const invitation = target.invitation;
  const nowMs = Date.now();
  invitation.token = newInvitationToken();
  invitation.expiresAt = invitationExpiry(nowMs);
  invitation.updatedAt = new Date(nowMs).toISOString();
  await saveOrganizationInvitation(env.DB, invitation);
  await writeOrgAudit(env, userId, 'organization.reinvite', 'organization', orgId, {
    email: invitation.email,
    organizationId: orgId,
  });
  const invitee = await getUser(env.DB, invitation.email);
  await bumpOrgMemberAndManagers(env, orgId, [invitee?.id], readActingDeviceIdentifier(request));
  return new Response(null, { status: 200 });
}

// DELETE /api/organizations/:orgId/users/:ouId -- a member or an invitation.
export async function handleRemoveOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  const actor = await requireManager(env, orgId, userId);
  if (!actor) return errorResponse('Not found', 404);
  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return errorResponse('Not found', 404);
  if (target.kind === 'invitation') return revokeOrganizationInvitation(request, env, userId, orgId, target.invitation);
  return removeOrganizationMember(request, env, actor, orgId, target.member);
}
```

- [ ] **Step 7: Wire the routes**

In `src/router-authenticated.ts`, add after the `import { ... } from './handlers/organizations';` block:

```ts
import {
  handleInviteOrganizationUsers,
  handleListOrganizationUserMiniDetails,
  handleListOrganizationUsers,
  handleReinviteOrganizationUser,
  handleRemoveOrganizationUser,
} from './handlers/organization-users';
```

Inside the `if (orgMatch) {` block, directly above `    if (subPath === '/members') {`, add:

```ts
    // Bitwarden's official membership routes. ouId is a member's user id or an
    // invitation's id; see resolveOrgUser.
    if (subPath === '/users') {
      if (method === 'GET') return handleListOrganizationUsers(request, env, userId, orgId);
      return errorResponse('Method not allowed', 405);
    }

    if (subPath === '/users/mini-details' && method === 'GET') {
      return handleListOrganizationUserMiniDetails(request, env, userId, orgId);
    }

    if (subPath === '/users/invite' && method === 'POST') {
      return handleInviteOrganizationUsers(request, env, userId, orgId);
    }

    const orgUserMatch = subPath.match(/^\/users\/([a-f0-9-]+)(\/[a-z-]+)?$/i);
    if (orgUserMatch) {
      const ouId = orgUserMatch[1];
      const action = orgUserMatch[2] || '';
      if (action === '' && method === 'DELETE') return handleRemoveOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '/reinvite' && method === 'POST') return handleReinviteOrganizationUser(request, env, userId, orgId, ouId);
      return errorResponse('Method not allowed', 405);
    }

```

- [ ] **Step 8: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 34`, `ℹ pass 34`, `ℹ fail 0`.

Run: `pnpm run test:organizations`
Expected: `ℹ pass 11`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 9: Commit**

```bash
git add src/services/org-user-model.ts src/handlers/organization-users.ts src/services/storage-organization-repo.ts src/handlers/organizations.ts src/router-authenticated.ts scripts/sharing-org-users.test.ts
git commit -m "NodeWarden answers Bitwarden's roster, invite, re-invite and remove routes" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Accept, confirm, public keys and leave, with manager pushes

**Files:**
- Modify: `src/handlers/organizations.ts` (`acceptOrganizationInvitation` extracted; manager pushes on accept, decline and the direct add)
- Modify: `src/handlers/organization-users.ts` (accept, confirm, bulk confirm, public keys, leave, user public key)
- Modify: `src/router-authenticated.ts`
- Test: `scripts/sharing-membership.test.ts`

**Interfaces:**
- Consumes: `resolveOrgUser`, `newInvitationToken`, `invitationExpiry` (Task 5); `bumpOrgMemberAndManagers`, `writeOrgAudit` (Task 5); `requireManager`, `readJson` (Task 5, private to `organization-users.ts`).
- Produces:
  - `organizations.ts`: `acceptOrganizationInvitation(request: Request, env: Env, userId: string, invitation: OrganizationInvitation): Promise<Response>`.
  - `organization-users.ts`: `handleAcceptOrganizationUser`, `handleConfirmOrganizationUser`, `handleBulkConfirmOrganizationUsers`, `handleOrganizationUserPublicKeys`, `handleLeaveOrganization`, `handleGetUserPublicKey`.
  - Routes: `POST .../users/{ouId}/accept {token}`, `POST .../users/{ouId}/confirm {key}`, `POST .../users/confirm {keys:[{id,key}]}` → `{object:"list", data:[{id, error}]}`, `POST .../users/public-keys {ids}` → `{object:"list", data:[{id, userId, key}]}`, `POST /api/organizations/{orgId}/leave`, `GET /api/users/{id}/public-key` → `{object:"userKey", userId, publicKey}`.

`GET /api/users/{id}/public-key` is also Deskwarden's probe for slice 1 (spec 7.1): on a NodeWarden without it, sharing stays off.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-membership.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  COL_A, INV_CAROL, ORG, ORG2, U, call, count, createHarness, emailOf, memberCollectionsRaw,
  readOnlyAccess, resetRevisions, seedCollection, seedInvitation, seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';

// Invite -> accept -> confirm, on Bitwarden's routes. Confirm is the sharer's
// client wrapping the org key to the recipient's public key; the server only
// stores it. An accept must reach the sharer at once so their daemon can
// confirm without waiting for a poll.

const INV_DAVE = 'eeeeeeee-0000-4000-8000-000000000002';

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedUser(h, 'dave');
  seedUser(h, 'erin');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] }); // confirmed
  seedMember(h, ORG, U.bob, { key: null }); // accepted, not confirmed
  seedMember(h, ORG, U.erin, { role: 'admin' }); // a second manager
  seedInvitation(h, ORG, INV_CAROL, emailOf('carol'), { invitedBy: U.owner, token: 'carol-token' });
  return h;
}

test('invite, accept and confirm on the official routes move a person 0 -> 1 -> 2', async () => {
  const h = fixture();
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: [emailOf('dave')], type: 2, collections: [readOnlyAccess(COL_A)],
  });
  const invite = h.db.prepare('SELECT id, token FROM organization_invitations WHERE email = ?').get(emailOf('dave')) as any;
  const daveRow = async () => (await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users`)).body.data
    .find((row: any) => row.email === emailOf('dave'));

  const invited = await daveRow();
  assert.equal(invited.status, 0);
  assert.equal(invited.id, invite.id);

  const accepted = await call(h, 'dave', 'POST', `/api/organizations/${ORG}/users/${invite.id}/accept`, { token: invite.token, resetPasswordKey: null });
  assert.equal(accepted.status, 200);
  const afterAccept = await daveRow();
  assert.equal(afterAccept.status, 1);
  assert.equal(afterAccept.id, U.dave);
  assert.equal(afterAccept.userId, U.dave);
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.dave), [readOnlyAccess(COL_A)]);

  const confirmed = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${U.dave}/confirm`, { key: '4.d3JhcHBlZA==' });
  assert.equal(confirmed.status, 200);
  assert.equal((await daveRow()).status, 2);
  const keys = await call(h, 'dave', 'GET', `/api/organizations/${ORG}/keys`);
  assert.equal(keys.body.key, '4.d3JhcHBlZA==');
});

test('confirm is refused before accept, twice, without a key, and to a plain member', async () => {
  const h = fixture();
  const confirm = (person: 'owner' | 'alice', ouId: string, key: string) =>
    call(h, person, 'POST', `/api/organizations/${ORG}/users/${ouId}/confirm`, { key });

  const early = await confirm('owner', INV_CAROL, '4.aw==');
  assert.equal(early.status, 400);
  assert.equal(early.body.error, 'User not accepted');

  const again = await confirm('owner', U.alice, '4.aw==');
  assert.equal(again.status, 400);
  assert.equal(again.body.error, 'Already confirmed');

  const junk = await confirm('owner', U.bob, 'not a key');
  assert.equal(junk.status, 400);
  assert.equal(junk.body.error, 'Key must be an encrypted string');

  assert.equal((await confirm('alice', U.bob, '4.aw==')).status, 404);
});

test('accept needs the right token, the right account and the right organization', async () => {
  const h = fixture();
  const wrongToken = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${INV_CAROL}/accept`, { token: 'nope' });
  assert.equal(wrongToken.status, 400);
  assert.equal(wrongToken.body.error, 'Invitation token is invalid');

  const wrongAccount = await call(h, 'alice', 'POST', `/api/organizations/${ORG}/users/${INV_CAROL}/accept`, { token: 'carol-token' });
  assert.equal(wrongAccount.status, 403);
  assert.equal(wrongAccount.body.error, 'Invitation was issued to a different account');

  const wrongOrg = await call(h, 'alice', 'POST', `/api/organizations/${ORG2}/users/${INV_CAROL}/accept`, { token: 'carol-token' });
  assert.equal(wrongOrg.status, 404);
});

test('an accept tells every manager and the person, and nobody else', async () => {
  const h = fixture();
  seedInvitation(h, ORG, INV_DAVE, emailOf('dave'), { invitedBy: U.owner, token: 'dave-token' });
  resetRevisions(h);
  const res = await call(h, 'dave', 'POST', `/api/organizations/${ORG}/users/${INV_DAVE}/accept`, { token: 'dave-token' });
  assert.equal(res.status, 200);
  assert.equal(wasBumped(h, U.owner), true);
  assert.equal(wasBumped(h, U.erin), true);
  assert.equal(wasBumped(h, U.dave), true);
  assert.equal(wasBumped(h, U.alice), false);
  assert.equal(wasBumped(h, U.bob), false);
});

test('a bulk confirm answers per person', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/confirm`, {
    keys: [
      { id: U.bob, key: '4.Ym9i' },
      { id: U.alice, key: '4.YQ==' },
      { id: INV_CAROL, key: '4.Yw==' },
    ],
  });
  assert.equal(res.status, 200);
  assert.equal(res.body.object, 'list');
  assert.deepEqual(res.body.data.map((row: any) => [row.id, row.error]), [
    [U.bob, ''],
    [U.alice, 'Already confirmed'],
    [INV_CAROL, 'User not accepted'],
  ]);
  const bob = h.db.prepare('SELECT encrypted_org_key FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.bob) as any;
  assert.equal(bob.encrypted_org_key, '4.Ym9i');
});

test('public keys are answered for members only, and only to managers', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/public-keys`, {
    ids: [U.alice, INV_CAROL, 'aaaaaaaa-0000-4000-8000-0000000000ff'],
  });
  assert.equal(res.status, 200);
  assert.deepEqual(res.body.data, [
    { object: 'organizationUserPublicKeyResponseModel', id: U.alice, userId: U.alice, key: 'PUBKEY-alice' },
  ]);
  assert.equal((await call(h, 'alice', 'POST', `/api/organizations/${ORG}/users/public-keys`, { ids: [U.bob] })).status, 404);
});

test('a member can leave; the owner cannot; a stranger is not found', async () => {
  const h = fixture();
  resetRevisions(h);
  assert.equal((await call(h, 'alice', 'POST', `/api/organizations/${ORG}/leave`)).status, 200);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND user_id = ?', ORG, U.alice), 0);
  assert.equal(wasBumped(h, U.owner), true);
  assert.equal(wasBumped(h, U.alice), true);

  const owner = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/leave`);
  assert.equal(owner.status, 400);
  assert.equal(owner.body.error, 'The organization owner cannot leave');
  assert.equal((await call(h, 'dave', 'POST', `/api/organizations/${ORG}/leave`)).status, 404);
});

test("any signed-in user can read an account's public key by id", async () => {
  const h = fixture();
  seedUser(h, 'carol', { publicKey: null });
  const res = await call(h, 'dave', 'GET', `/api/users/${U.alice}/public-key`);
  assert.equal(res.status, 200);
  assert.deepEqual(res.body, { object: 'userKey', userId: U.alice, publicKey: 'PUBKEY-alice' });
  assert.equal((await call(h, 'dave', 'GET', `/api/users/${U.carol}/public-key`)).status, 404);
  assert.equal((await call(h, 'dave', 'GET', '/api/users/aaaaaaaa-0000-4000-8000-0000000000ff/public-key')).status, 404);
});

test("NodeWarden's own accept and decline also tell the managers", async () => {
  const h = fixture();
  seedInvitation(h, ORG, INV_DAVE, emailOf('dave'), { invitedBy: U.owner, token: 'dave-token' });
  resetRevisions(h);
  assert.equal((await call(h, 'dave', 'POST', '/api/organizations/invitations/accept', { token: 'dave-token' })).status, 200);
  assert.equal(wasBumped(h, U.owner), true);
  assert.equal(wasBumped(h, U.erin), true);

  seedUser(h, 'carol');
  resetRevisions(h);
  assert.equal((await call(h, 'carol', 'POST', '/api/organizations/invitations/decline', { token: 'carol-token' })).status, 204);
  assert.equal(wasBumped(h, U.owner), true);
});

test("the direct add of an existing account tells the managers too", async () => {
  const h = fixture();
  resetRevisions(h);
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/members/invite`, {
    email: emailOf('dave'), role: 'member', encryptedOrgKey: '4.a2V5', collections: [readOnlyAccess(COL_A)],
  });
  assert.equal(res.status, 200);
  assert.equal(wasBumped(h, U.dave), true);
  assert.equal(wasBumped(h, U.erin), true);
  assert.equal(wasBumped(h, U.alice), false);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL in `sharing-membership.test.ts`: the new routes answer `405 Method not allowed` or `404`, and the push tests fail on `wasBumped(h, U.owner)` / `wasBumped(h, U.erin)` being `false`. The 34 earlier tests pass.

- [ ] **Step 3: Extract the accept and add the manager pushes**

In `src/handlers/organizations.ts`:

Add after the `import { applyCollectionAccessUpdate, ... } from '../services/org-collection-access';` line:

```ts
import { invitationExpiry, newInvitationToken } from '../services/org-user-model';
```

In `handleInviteOrganizationMember`, replace

```ts
    await saveOrganizationMember(env.DB, newMember);
    await bumpMembersRevision(env, [targetUser.id], readActingDeviceIdentifier(request));
```

with

```ts
    await saveOrganizationMember(env.DB, newMember);
    await bumpOrgMemberAndManagers(env, orgId, [targetUser.id], readActingDeviceIdentifier(request));
```

and replace

```ts
    token: generateUUID() + generateUUID(),
    expiresAt: new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString(),
```

with

```ts
    token: newInvitationToken(),
    expiresAt: invitationExpiry(),
```

and replace

```ts
  await saveOrganizationInvitation(env.DB, invitation);
  await writeOrgAudit(env, userId, 'organization.invite', 'organization', orgId, {
```

with

```ts
  await saveOrganizationInvitation(env.DB, invitation);
  await bumpOrgMemberAndManagers(env, orgId, [], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, userId, 'organization.invite', 'organization', orgId, {
```

Replace the whole `handleAcceptOrganizationInvitation` function (from `// POST /api/organizations/invitations/accept` to its closing brace) with:

```ts
// Turns a pending invitation into an accepted, not yet confirmed, membership for
// userId. Shared by NodeWarden's route (token only) and the official one (ouId +
// token). Every manager hears of it at once, so a sharer's daemon can confirm
// without waiting for a poll.
export async function acceptOrganizationInvitation(
  request: Request,
  env: Env,
  userId: string,
  invitation: OrganizationInvitation
): Promise<Response> {
  if (invitation.status !== 0) return errorResponse('Invitation is no longer pending', 400);
  if (new Date(invitation.expiresAt).getTime() < Date.now()) return errorResponse('Invitation has expired', 400);

  const user = await getUserById(env.DB, userId);
  if (!user) return errorResponse('Not found', 404);
  if (user.email.toLowerCase() !== invitation.email.toLowerCase()) {
    return errorResponse('Invitation was issued to a different account', 403);
  }

  const now = new Date().toISOString();
  const existingMember = await getOrganizationMember(env.DB, invitation.organizationId, userId);
  if (existingMember) {
    if (existingMember.status === 1) {
      await deleteOrganizationInvitation(env.DB, invitation.id);
      return jsonResponse({ success: true });
    }
    existingMember.status = 1;
    existingMember.role = invitation.role;
    existingMember.collections = invitation.collections || [];
    existingMember.permissions = invitation.permissions || {};
    existingMember.updatedAt = now;
    await saveOrganizationMember(env.DB, existingMember);
  } else {
    const newMember: OrganizationMember = {
      organizationId: invitation.organizationId,
      userId,
      status: 1,
      role: invitation.role,
      permissions: invitation.permissions || {},
      collections: invitation.collections || [],
      encryptedOrgKey: null,
      createdAt: now,
      updatedAt: now,
    };
    await saveOrganizationMember(env.DB, newMember);
  }

  await deleteOrganizationInvitation(env.DB, invitation.id);
  await bumpOrgMemberAndManagers(env, invitation.organizationId, [userId], readActingDeviceIdentifier(request));

  const ssoEnabled = await getSsoConfig(env.DB, invitation.organizationId);
  return jsonResponse({
    success: true,
    organizationId: invitation.organizationId,
    role: invitation.role,
    ssoEnabled: !!ssoEnabled?.enabled,
  });
}

// POST /api/organizations/invitations/accept
// The token travels in the body, not the path: request URLs land in access logs.
export async function handleAcceptOrganizationInvitation(request: Request, env: Env, userId: string): Promise<Response> {
  let body: any;
  try {
    body = await request.json();
  } catch {
    return errorResponse('Invalid JSON', 400);
  }

  const token = String(body?.token || '').trim();
  if (!token) return errorResponse('Invitation token is required', 400);

  const invitation = await getOrganizationInvitationByToken(env.DB, token);
  if (!invitation) return errorResponse('Invitation not found', 404);

  return acceptOrganizationInvitation(request, env, userId, invitation);
}
```

In `handleDeclineOrganizationInvitation`, replace

```ts
  await deleteOrganizationInvitation(env.DB, invitation.id);
  await writeOrgAudit(env, userId, 'organization.declineInvitation', 'organization', invitation.organizationId, {
```

with

```ts
  await deleteOrganizationInvitation(env.DB, invitation.id);
  await bumpOrgMemberAndManagers(env, invitation.organizationId, [userId], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, userId, 'organization.declineInvitation', 'organization', invitation.organizationId, {
```

- [ ] **Step 4: Add the routes' handlers**

In `src/handlers/organization-users.ts`, replace the import block (every `import` line at the top) with:

```ts
import type { CollectionAccess, Env, OrganizationInvitation, OrganizationMember } from '../types';
import { errorResponse, jsonResponse } from '../utils/response';
import { generateUUID } from '../utils/uuid';
import { readActingDeviceIdentifier } from '../utils/device';
import { getUser, getUserById, getUsersByIds } from '../services/storage-user-repo';
import {
  canAssignOrgRole,
  deleteOrganizationInvitation,
  deleteOrganizationMember,
  getCollectionsForOrganization,
  getOrganizationInvitation,
  getOrganizationInvitations,
  getOrganizationMember,
  getOrganizationMembers,
  isOrgManager,
  saveOrganizationInvitation,
  saveOrganizationMember,
} from '../services/storage-organization-repo';
import { normalizeCollectionAccessList } from '../services/org-collection-access';
import {
  invitationExpiry,
  newInvitationToken,
  orgRoleForUserType,
  orgUserStatusForMember,
  orgUserTypeForRole,
  resolveOrgUser,
} from '../services/org-user-model';
import {
  acceptOrganizationInvitation,
  bumpOrgMemberAndManagers,
  removeOrganizationMember,
  revokeOrganizationInvitation,
  writeOrgAudit,
} from './organizations';
```

Append to the end of the file:

```ts
function sameToken(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

// POST /api/organizations/:orgId/users/:ouId/accept   { token, resetPasswordKey? }
// The invitee, signed in; no membership needed. Same rules as
// /invitations/accept: the token must match and the invite's email must be theirs.
export async function handleAcceptOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  const body = await readJson(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);
  const token = String(body.token || '').trim();
  if (!token) return errorResponse('Invitation token is required', 400);

  const invitation = await getOrganizationInvitation(env.DB, ouId);
  if (!invitation || invitation.organizationId !== orgId) return errorResponse('Invitation not found', 404);
  if (!sameToken(invitation.token, token)) return errorResponse('Invitation token is invalid', 400);
  return acceptOrganizationInvitation(request, env, userId, invitation);
}

const ENC_STRING_PATTERN = /^\d+\.\S+$/;

type ConfirmResult = { ok: true; userId: string } | { ok: false; status: number; error: string };

// Stores the org key the manager's client wrapped to the member's public key.
// Only an accepted member without a key can be confirmed.
async function confirmOrgUser(env: Env, orgId: string, ouId: string, key: string): Promise<ConfirmResult> {
  if (!ENC_STRING_PATTERN.test(key)) return { ok: false, status: 400, error: 'Key must be an encrypted string' };
  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return { ok: false, status: 404, error: 'Not found' };
  if (target.kind === 'invitation') return { ok: false, status: 400, error: 'User not accepted' };
  const member = target.member;
  if (member.encryptedOrgKey) return { ok: false, status: 400, error: 'Already confirmed' };
  member.encryptedOrgKey = key;
  member.updatedAt = new Date().toISOString();
  await saveOrganizationMember(env.DB, member);
  return { ok: true, userId: member.userId };
}

// POST /api/organizations/:orgId/users/:ouId/confirm   { key }
export async function handleConfirmOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const body = await readJson(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);

  const result = await confirmOrgUser(env, orgId, ouId, String(body.key || '').trim());
  if (!result.ok) return errorResponse(result.error, result.status);
  await bumpOrgMemberAndManagers(env, orgId, [result.userId], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, userId, 'organization.member.confirm', 'organization', orgId, {
    memberId: result.userId,
    organizationId: orgId,
  });
  return new Response(null, { status: 200 });
}

// POST /api/organizations/:orgId/users/confirm   { keys: [{ id, key }] }
export async function handleBulkConfirmOrganizationUsers(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const body = await readJson(request);
  if (!body || !Array.isArray(body.keys)) return errorResponse('keys array is required', 400);

  const data: Record<string, unknown>[] = [];
  const confirmed: string[] = [];
  for (const entry of body.keys) {
    const id = String(entry?.id || '').trim();
    const result = await confirmOrgUser(env, orgId, id, String(entry?.key || '').trim());
    if (result.ok) confirmed.push(result.userId);
    data.push({ object: 'OrganizationBulkConfirmResponseModel', id, error: result.ok ? '' : result.error });
  }
  if (confirmed.length) {
    await bumpOrgMemberAndManagers(env, orgId, confirmed, readActingDeviceIdentifier(request));
    await writeOrgAudit(env, userId, 'organization.member.confirm', 'organization', orgId, {
      count: confirmed.length,
      organizationId: orgId,
    });
  }
  return jsonResponse({ object: 'list', data, continuationToken: null });
}

// POST /api/organizations/:orgId/users/public-keys   { ids: [ouId] }
// The field is `key`, as on official servers. Invitations have no account and are skipped.
export async function handleOrganizationUserPublicKeys(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const body = await readJson(request);
  if (!body || !Array.isArray(body.ids)) return errorResponse('ids array is required', 400);

  const data: Record<string, unknown>[] = [];
  for (const raw of body.ids) {
    const id = String(raw || '').trim();
    const target = await resolveOrgUser(env.DB, orgId, id);
    if (!target || target.kind !== 'member') continue;
    const user = await getUserById(env.DB, target.member.userId);
    if (!user) continue;
    data.push({ object: 'organizationUserPublicKeyResponseModel', id, userId: user.id, key: user.publicKey ?? null });
  }
  return jsonResponse({ object: 'list', data, continuationToken: null });
}

// POST /api/organizations/:orgId/leave
// Any accepted member but the owner drops their own membership. For a
// recipient this is "stop receiving from this person".
export async function handleLeaveOrganization(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  const member = await getOrganizationMember(env.DB, orgId, userId);
  if (!member || member.status !== 1) return errorResponse('Not found', 404);
  if (member.role === 'owner') return errorResponse('The organization owner cannot leave', 400);

  await deleteOrganizationMember(env.DB, orgId, userId);
  await bumpOrgMemberAndManagers(env, orgId, [userId], readActingDeviceIdentifier(request));
  await writeOrgAudit(env, userId, 'organization.leave', 'organization', orgId, { organizationId: orgId });
  return new Response(null, { status: 200 });
}

// GET /api/users/:id/public-key
// Any signed-in user, as on official servers. It is keyed by an opaque id, so it
// tells nobody whether an email has an account.
export async function handleGetUserPublicKey(request: Request, env: Env, userId: string, targetUserId: string): Promise<Response> {
  void request;
  void userId;
  const target = await getUserById(env.DB, targetUserId);
  if (!target || !target.publicKey) return errorResponse('User public key not found', 404);
  return jsonResponse({ object: 'userKey', userId: target.id, publicKey: target.publicKey });
}
```

- [ ] **Step 5: Wire the routes**

In `src/router-authenticated.ts`, replace the `import { ... } from './handlers/organization-users';` block with:

```ts
import {
  handleAcceptOrganizationUser,
  handleBulkConfirmOrganizationUsers,
  handleConfirmOrganizationUser,
  handleGetUserPublicKey,
  handleInviteOrganizationUsers,
  handleLeaveOrganization,
  handleListOrganizationUserMiniDetails,
  handleListOrganizationUsers,
  handleOrganizationUserPublicKeys,
  handleReinviteOrganizationUser,
  handleRemoveOrganizationUser,
} from './handlers/organization-users';
```

Directly above `  if (path === '/api/organizations') {`, add:

```ts
  const userPublicKeyMatch = path.match(/^\/api\/users\/([a-f0-9-]+)\/public-key$/i);
  if (userPublicKeyMatch && method === 'GET') {
    return handleGetUserPublicKey(request, env, userId, userPublicKeyMatch[1]);
  }

```

Inside the `if (orgMatch) {` block, directly above `    const orgUserMatch = subPath.match(`, add:

```ts
    if (subPath === '/users/confirm' && method === 'POST') {
      return handleBulkConfirmOrganizationUsers(request, env, userId, orgId);
    }

    if (subPath === '/users/public-keys' && method === 'POST') {
      return handleOrganizationUserPublicKeys(request, env, userId, orgId);
    }

    if (subPath === '/leave' && method === 'POST') {
      return handleLeaveOrganization(request, env, userId, orgId);
    }

```

and replace

```ts
      if (action === '/reinvite' && method === 'POST') return handleReinviteOrganizationUser(request, env, userId, orgId, ouId);
```

with

```ts
      if (action === '/reinvite' && method === 'POST') return handleReinviteOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '/accept' && method === 'POST') return handleAcceptOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '/confirm' && method === 'POST') return handleConfirmOrganizationUser(request, env, userId, orgId, ouId);
```

- [ ] **Step 6: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 44`, `ℹ pass 44`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 7: Commit**

```bash
git add src/handlers/organizations.ts src/handlers/organization-users.ts src/router-authenticated.ts scripts/sharing-membership.test.ts
git commit -m "An accept reaches the sharer at once, and Bitwarden's confirm routes store the key" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Person collections: `users[]` on create, and names up to 1000 characters

**Files:**
- Modify: `src/handlers/organizations.ts` (`handleCreateCollection`, `handleUpdateCollection`)
- Test: `scripts/sharing-collections.test.ts`

**Interfaces:**
- Consumes: `resolveOrgUser`, `OrgUserRef` (Task 5); `normalizeCollectionAccessList`, `withCollectionAccess` (Task 1).
- Produces: `POST /api/organizations/{orgId}/collections` accepts `{ name, externalId?, users?: [{id: ouId, readOnly, hidePasswords, manage}], groups? }` and applies `users[]` to those members' (or invitations') access lists in the same request. `groups` is accepted and ignored (NodeWarden has no groups). Collection names may be up to 1000 characters on create and update.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-collections.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ENC, INV_CAROL, ORG, U, call, count, createHarness, emailOf, invitationCollectionsRaw,
  memberCollectionsRaw, readOnlyAccess, seedInvitation, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// A person collection is named with the recipient's email encrypted under the
// org key (2.iv|ct|mac, about 120 characters), and is created with its access
// already granted: the recipient read-only, the sharer with Manage.

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'dave');
  seedOrg(h, ORG, U.owner);
  seedMember(h, ORG, U.alice);
  seedInvitation(h, ORG, INV_CAROL, emailOf('carol'), { invitedBy: U.owner });
  return h;
}

test('an encrypted collection name fits, up to the official 1000 characters', async () => {
  const h = fixture();
  const created = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/collections`, { name: 'x'.repeat(600) });
  assert.equal(created.status, 200);
  const tooLong = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/collections`, { name: 'x'.repeat(1001) });
  assert.equal(tooLong.status, 400);
  assert.equal(tooLong.body.error, 'Collection name too long');
  const renamed = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/collections/${created.body.id}`, { name: 'y'.repeat(1000) });
  assert.equal(renamed.status, 200);
});

test('a new collection grants its users their access in the same request', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/collections`, {
    name: ENC,
    externalId: null,
    groups: [],
    users: [
      { id: U.owner, readOnly: false, hidePasswords: false, manage: true },
      { id: U.alice, readOnly: true, hidePasswords: false, manage: false },
      { id: INV_CAROL, readOnly: true, hidePasswords: false, manage: false },
    ],
  });
  assert.equal(res.status, 200);
  const id = res.body.id;
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.owner), [{ id, readOnly: false, hidePasswords: false, manage: true }]);
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.alice), [readOnlyAccess(id)]);
  assert.deepEqual(invitationCollectionsRaw(h, INV_CAROL), [readOnlyAccess(id)]);

  const seen = await call(h, 'alice', 'GET', `/api/organizations/${ORG}/collections`);
  assert.deepEqual(seen.body.data.map((c: any) => c.id), [id]);
});

test('a grant to someone outside the organization creates nothing', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/collections`, {
    name: ENC,
    users: [{ id: U.dave, readOnly: true, hidePasswords: false, manage: false }],
  });
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'A selected member does not belong to this organization');
  assert.equal(count(h, 'SELECT COUNT(*) FROM collections WHERE organization_id = ?', ORG), 0);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 3 failures in `sharing-collections.test.ts` (`400 !== 200` for the 600-character name, the grants are not applied, and the outsider grant answers 200). The 44 earlier tests pass.

- [ ] **Step 3: Implement**

In `src/handlers/organizations.ts`:

In the `import type { Env, User, Organization, ... } from '../types';` line, add `CollectionAccess` to the list.

Replace

```ts
import { applyCollectionAccessUpdate, collectionAccessIds, normalizeCollectionAccessList } from '../services/org-collection-access';
import { invitationExpiry, newInvitationToken } from '../services/org-user-model';
```

with

```ts
import {
  applyCollectionAccessUpdate,
  collectionAccessIds,
  normalizeCollectionAccessList,
  withCollectionAccess,
} from '../services/org-collection-access';
import { invitationExpiry, newInvitationToken, resolveOrgUser, type OrgUserRef } from '../services/org-user-model';
```

Replace the whole `handleCreateCollection` function (from `// POST /api/organizations/:id/collections` to its closing brace) with:

```ts
// Official EncryptedStringLength(1000). An encrypted name (2.iv|ct|mac) of an
// ordinary email is about 120 characters, which the old limit of 100 refused.
const MAX_COLLECTION_NAME_LENGTH = 1000;

// POST /api/organizations/:id/collections
// Takes Bitwarden's `users: [{id: ouId, readOnly, hidePasswords, manage}]` and
// grants that access in the same request, so a sharer's person collection never
// exists without its person, or without the sharer's own Manage access.
// `groups` is accepted and ignored: NodeWarden has no groups.
export async function handleCreateCollection(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  const member = await requireMember(env, orgId, userId);
  if (!member || !isOrgManager(member)) return errorResponse('Not found', 404);

  let body: any;
  try {
    body = await request.json();
  } catch {
    return errorResponse('Invalid JSON', 400);
  }

  const name = String(body?.name || '').trim();
  if (!name) return errorResponse('Collection name is required', 400);
  if (name.length > MAX_COLLECTION_NAME_LENGTH) return errorResponse('Collection name too long', 400);

  // Every grant must name someone in this organization before anything is written.
  const grants: Array<{ target: OrgUserRef; access: Omit<CollectionAccess, 'id'> }> = [];
  for (const grant of normalizeCollectionAccessList(body?.users)) {
    const target = await resolveOrgUser(env.DB, orgId, grant.id);
    if (!target) return errorResponse('A selected member does not belong to this organization', 400);
    grants.push({ target, access: { readOnly: grant.readOnly, hidePasswords: grant.hidePasswords, manage: grant.manage } });
  }

  const now = new Date().toISOString();
  const collection: Collection = {
    id: generateUUID(),
    organizationId: orgId,
    name,
    externalId: body?.externalId ? String(body.externalId).trim() : null,
    createdAt: now,
    updatedAt: now,
  };

  await saveCollection(env.DB, collection);

  for (const { target, access } of grants) {
    const entry: CollectionAccess = { id: collection.id, ...access };
    if (target.kind === 'member') {
      target.member.collections = withCollectionAccess(target.member.collections, entry);
      target.member.updatedAt = now;
      await saveOrganizationMember(env.DB, target.member);
    } else {
      target.invitation.collections = withCollectionAccess(target.invitation.collections, entry);
      target.invitation.updatedAt = now;
      await saveOrganizationInvitation(env.DB, target.invitation);
    }
  }

  const memberIds = await getOrganizationMemberIds(env.DB, orgId);
  await bumpMembersRevision(env, memberIds, readActingDeviceIdentifier(request));
  await writeOrgAudit(env, userId, 'collection.create', 'collection', collection.id, {
    name,
    organizationId: orgId,
    collectionId: collection.id,
  });

  return jsonResponse(toCollectionResponse(collection));
}
```

In `handleUpdateCollection`, replace

```ts
    if (name.length > 100) return errorResponse('Collection name too long', 400);
```

with

```ts
    if (name.length > MAX_COLLECTION_NAME_LENGTH) return errorResponse('Collection name too long', 400);
```

- [ ] **Step 4: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 47`, `ℹ pass 47`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/organizations.ts scripts/sharing-collections.test.ts
git commit -m "A collection is created with its people, and an encrypted name fits" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: A budget on the email lookup

**Files:**
- Modify: `src/config/limits.ts` (`rateLimit.memberLookupRequestsPerMinute`, `rateLimit.memberLookupRequestsPerHour`)
- Modify: `src/handlers/organizations.ts` (`enforceMemberLookupBudget`, `handleGetOrganizationMemberPublicKeyByEmail`)
- Test: `scripts/sharing-member-lookup.test.ts`

**Interfaces:**
- Consumes: `RateLimitService.consumeStrictBudgetWithWindow` (existing, D1-backed fixed windows).
- Produces: `GET /api/organizations/{id}/members/public-key?email=` answers `429` with `Retry-After` once the caller has made 10 lookups in the current minute or 60 in the current hour. The budget is per account (the caller is always signed in). A plain member still gets `404` and spends nothing.

Owner decision Q4: rate-limit it rather than accept or remove it. Removing it would end the direct add.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-member-lookup.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { LIMITS } from '../src/config/limits';
import { RateLimitService } from '../src/services/ratelimit';
import { ORG, ORG2, U, call, createHarness, emailOf, seedOrg, seedUser } from './support/sharing-harness';

// members/public-key?email= tells an organization owner whether an address has
// an account here, and anyone may own an organization. The budget keeps it from
// becoming a way to walk a list of addresses.

function freshHarness() {
  // The budget table is created once per process and remembered in a static;
  // every test here starts a fresh database, so forget it.
  (RateLimitService as any).strictBudgetTableReady = false;
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'erin');
  seedUser(h, 'dave');
  seedOrg(h, ORG, U.owner);
  seedOrg(h, ORG2, U.erin);
  return h;
}

const lookup = (h: ReturnType<typeof createHarness>, person: 'owner' | 'erin', orgId: string) =>
  call(h, person, 'GET', `/api/organizations/${orgId}/members/public-key?email=${encodeURIComponent(emailOf('dave'))}`);

// Fixed windows are aligned to the clock: pin it mid-minute so a burst cannot
// straddle two windows and pass by luck.
const MID_MINUTE = Date.parse('2026-09-27T10:00:30.000Z');

test('the email lookup answers up to the per-minute budget, then 429', async (t) => {
  t.mock.method(Date, 'now', () => MID_MINUTE);
  const h = freshHarness();
  for (let i = 0; i < LIMITS.rateLimit.memberLookupRequestsPerMinute; i++) {
    const res = await lookup(h, 'owner', ORG);
    assert.equal(res.status, 200, `lookup ${i + 1}`);
    assert.equal(res.body.userId, U.dave);
  }
  const limited = await lookup(h, 'owner', ORG);
  assert.equal(limited.status, 429);
  assert.ok(Number(limited.headers.get('Retry-After')) > 0);
});

test('the budget is per account', async (t) => {
  t.mock.method(Date, 'now', () => MID_MINUTE);
  const h = freshHarness();
  for (let i = 0; i <= LIMITS.rateLimit.memberLookupRequestsPerMinute; i++) await lookup(h, 'owner', ORG);
  assert.equal((await lookup(h, 'owner', ORG)).status, 429);
  assert.equal((await lookup(h, 'erin', ORG2)).status, 200);
});

test('the hourly budget holds across minutes', async (t) => {
  let now = MID_MINUTE;
  t.mock.method(Date, 'now', () => now);
  const h = freshHarness();
  const perMinute = LIMITS.rateLimit.memberLookupRequestsPerMinute;
  const perHour = LIMITS.rateLimit.memberLookupRequestsPerHour;
  let answered = 0;
  while (answered < perHour) {
    for (let i = 0; i < perMinute && answered < perHour; i++) {
      assert.equal((await lookup(h, 'owner', ORG)).status, 200, `lookup ${answered + 1}`);
      answered++;
    }
    now += 60_000;
  }
  assert.equal((await lookup(h, 'owner', ORG)).status, 429);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 3 failures in `sharing-member-lookup.test.ts`. `LIMITS.rateLimit.memberLookupRequestsPerMinute` is `undefined`, so the first loop never runs and the "limited" call answers `200 !== 429`. The 47 earlier tests pass.

- [ ] **Step 3: Add the limits**

In `src/config/limits.ts`, inside `rateLimit: {`, directly after the `authRequestRequestsPerMinute: 5,` line, add:

```ts
    // Organization member lookup by email (members/public-key?email=) per account per minute.
    // It tells an organization owner whether an address has an account here.
    // 按邮箱查询组织成员公钥：每账户每分钟配额（可据此判断邮箱是否已注册）。
    memberLookupRequestsPerMinute: 10,
    // The same lookup per account per hour.
    // 同一查询：每账户每小时配额。
    memberLookupRequestsPerHour: 60,
```

- [ ] **Step 4: Spend the budget in the handler**

In `src/handlers/organizations.ts`, add after `import { LIMITS } from '../config/limits';`:

```ts
import { RateLimitService } from '../services/ratelimit';
```

Replace the whole `handleGetOrganizationMemberPublicKeyByEmail` function (from `// GET /api/organizations/:id/members/public-key?email=...` to its closing brace) with:

```ts
// members/public-key?email= answers whether an address has an account here.
// Anyone may own an organization, so anyone may ask; this keeps it from
// becoming a way to walk a list of addresses. Per account, because the caller
// is always signed in.
async function enforceMemberLookupBudget(env: Env, userId: string): Promise<Response | null> {
  const rateLimit = new RateLimitService(env.DB);
  const windows: Array<[string, number, number]> = [
    [`org-member-lookup:${userId}:minute`, LIMITS.rateLimit.memberLookupRequestsPerMinute, 60],
    [`org-member-lookup:${userId}:hour`, LIMITS.rateLimit.memberLookupRequestsPerHour, 60 * 60],
  ];
  for (const [key, max, windowSeconds] of windows) {
    const budget = await rateLimit.consumeStrictBudgetWithWindow(key, max, windowSeconds);
    if (!budget.allowed) {
      const retryAfter = budget.retryAfterSeconds || windowSeconds;
      return new Response(
        JSON.stringify({
          error: 'Too many requests',
          error_description: `Rate limit exceeded. Try again in ${retryAfter} seconds.`,
        }),
        {
          status: 429,
          headers: {
            'Content-Type': 'application/json',
            'Retry-After': String(retryAfter),
            'X-RateLimit-Remaining': '0',
          },
        }
      );
    }
  }
  return null;
}

// GET /api/organizations/:id/members/public-key?email=...
// Whether an address has an account, and its key: what lets a sharer add an
// existing account directly (NodeWarden's direct add, kept by decision).
export async function handleGetOrganizationMemberPublicKeyByEmail(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  const member = await requireMember(env, orgId, userId);
  if (!member || !isOrgManager(member)) return errorResponse('Not found', 404);

  const limited = await enforceMemberLookupBudget(env, userId);
  if (limited) return limited;

  const url = new URL(request.url);
  const email = String(url.searchParams.get('email') || '').trim().toLowerCase();
  if (!email) return errorResponse('email query parameter is required', 400);

  const target = await getUser(env.DB, email);
  if (!target || !target.publicKey) return errorResponse('Member public key not found', 404);

  return jsonResponse({ userId: target.id, publicKey: target.publicKey });
}
```

- [ ] **Step 5: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 50`, `ℹ pass 50`, `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add src/config/limits.ts src/handlers/organizations.ts scripts/sharing-member-lookup.test.ts
git commit -m "Looking up whether an email has an account is rate-limited" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `PUT /api/ciphers/{id}/share` and `/collections`

**Files:**
- Create: `src/handlers/cipher-sharing.ts`
- Modify: `src/handlers/ciphers.ts` (export `cipherResponseOptionsForRequest` and `notifyCipherUpdateForRequest`)
- Modify: `src/handlers/org-ciphers.ts` (`getCipherAudienceUserIds`, `notifyCipherAudience`)
- Modify: `src/router-authenticated.ts` (replace the `/share` stub; add `/collections`, `/collections_v2`)
- Test: `scripts/sharing-share.test.ts`

**Interfaces:**
- Consumes: `mergeCipherUpdateBody`, `isStaleCipherUpdate`, `readCipherRevisionDate`, `STALE_CIPHER_MESSAGE` (Task 3); `resolveEditableCipher`, `validateOrgCollectionsForCipher` with write checks (Task 3); `viewerFromAccess` (Task 2).
- Produces:
  - `org-ciphers.ts`: `getCipherAudienceUserIds(db: D1Database, organizationId: string, collectionIds: readonly string[]): Promise<string[]>`; `notifyCipherAudience(env: Env, organizationId: string, collectionIds: readonly string[], exceptUserId: string, contextId?: string | null): Promise<void>`.
  - `cipher-sharing.ts`: `handleShareCipher(request, env, userId, cipherId)`; `handleUpdateCipherCollections(request, env, userId, cipherId, v2: boolean)`.
  - Routes: `PUT|POST /api/ciphers/{id}/share` `{cipher, collectionIds}` → `cipherDetails` (id unchanged); `PUT|POST /api/ciphers/{id}/collections` `{collectionIds}` → `cipherDetails`; `PUT|POST /api/ciphers/{id}/collections_v2` → `{object:"optionalCipherDetails", unavailable, cipher}`.

Rules (spec 10.4). Share: the caller owns the cipher and it is personal (`400 "Cipher is already in an organization"` otherwise; someone else's is `404`); every collection belongs to `cipher.organizationId` and is writable by the caller; `lastKnownRevisionDate` must not be stale; a record with attachments and no cipher key is refused; folder and favourite are cleared; `userId` stays the author; pushes go to the author (single-cipher push) and to every member who can now see it. Collections: the caller needs `canEdit`; the ids belong to the cipher's organization; a non-manager rewrites only the collections they hold (the others are kept) and needs write access to each one added or removed; the push goes to everyone who gained or lost the item.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-share.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ALICE_PERSONAL, ATT, CIPHER, COL_A, COL_B, COL_C, COL_X, ENC, ORG, PERSONAL, T0, U, call, count,
  createHarness, fullAccess, readOnlyAccess, resetRevisions, seedAttachment, seedCipher, seedCollection,
  seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';

// Share moves a personal record into the sharer's organization, filed in a
// person's collection, keeping its id. /collections then adds or removes
// people. Both are Bitwarden's routes; the stub this replaces returned the
// cipher unchanged.

const SHARED_AB = 'dddddddd-0000-4000-8000-0000000000ab';
const FORBIDDEN = 'You do not have permission to edit this item';

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedUser(h, 'erin');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedCollection(h, ORG, COL_C);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });
  seedMember(h, ORG, U.bob, { collections: [readOnlyAccess(COL_B)] });
  seedMember(h, ORG, U.erin, { collections: [fullAccess(COL_A), fullAccess(COL_C)] }); // an editor, not a manager
  seedCipher(h, PERSONAL, U.owner);
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  return h;
}

const shareBody = (collectionIds: string[], extra: Record<string, unknown> = {}) => ({
  collectionIds,
  cipher: {
    type: 1,
    name: ENC,
    notes: null,
    login: { username: ENC, password: ENC },
    organizationId: ORG,
    folderId: null,
    favorite: true,
    lastKnownRevisionDate: T0,
    ...extra,
  },
});

test('a personal record moves into the organization and keeps its id', async () => {
  const h = fixture();
  resetRevisions(h);
  const res = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A]));
  assert.equal(res.status, 200);
  assert.equal(res.body.id, PERSONAL);
  assert.equal(res.body.organizationId, ORG);
  assert.deepEqual(res.body.collectionIds, [COL_A]);
  assert.equal(res.body.folderId, null);
  assert.equal(res.body.favorite, false);
  assert.equal(res.body.edit, true);
  assert.equal(count(h, 'SELECT COUNT(*) FROM collection_items WHERE cipher_id = ? AND collection_id = ?', PERSONAL, COL_A), 1);
  assert.equal((h.db.prepare('SELECT user_id FROM ciphers WHERE id = ?').get(PERSONAL) as any).user_id, U.owner);

  const seen = await call(h, 'alice', 'GET', `/api/ciphers/${PERSONAL}`);
  assert.equal(seen.status, 200);
  assert.equal(seen.body.edit, false);
  // Pushed to the person it was shared with, not to everyone in the organization.
  assert.equal(wasBumped(h, U.alice), true);
  assert.equal(wasBumped(h, U.bob), false);
});

test('POST is kept as an alias', async () => {
  const h = fixture();
  assert.equal((await call(h, 'owner', 'POST', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A]))).status, 200);
});

test('a record already in an organization is refused', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'PUT', `/api/ciphers/${CIPHER}/share`, shareBody([COL_B]));
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'Cipher is already in an organization');
});

test("someone else's record is not found", async () => {
  const h = fixture();
  assert.equal((await call(h, 'alice', 'PUT', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A]))).status, 404);
});

test('a foreign collection, no collection or no organization is refused', async () => {
  const h = fixture();
  const cases: Array<[unknown, string]> = [
    [shareBody([COL_X]), 'A selected collection does not belong to this organization'],
    [shareBody([]), 'At least one collection is required'],
    [shareBody([COL_A], { organizationId: null }), 'Organization is required'],
  ];
  for (const [body, message] of cases) {
    const res = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/share`, body);
    assert.equal(res.status, 400, message);
    assert.equal(res.body.error, message);
  }
  assert.equal(count(h, 'SELECT COUNT(*) FROM collection_items WHERE cipher_id = ?', PERSONAL), 0);
});

test('a read-only collection cannot receive a shared record', async () => {
  const h = fixture();
  seedCipher(h, ALICE_PERSONAL, U.alice);
  const res = await call(h, 'alice', 'PUT', `/api/ciphers/${ALICE_PERSONAL}/share`, shareBody([COL_A]));
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'You do not have access to one of the selected collections');
});

test('a stale copy is refused', async () => {
  const h = fixture();
  h.db.prepare('UPDATE ciphers SET updated_at = ? WHERE id = ?').run('2026-03-01T00:00:00.000Z', PERSONAL);
  const res = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A]));
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'The client copy of this cipher is out of date. Resync the client and try again.');
});

test('attachments need a cipher key to move', async () => {
  const h = fixture();
  seedAttachment(h, ATT, PERSONAL);
  const refused = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A]));
  assert.equal(refused.status, 400);
  assert.equal(refused.body.error, 'This item has attachments that must be re-shared by the client');
  const moved = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/share`, shareBody([COL_A], { key: ENC }));
  assert.equal(moved.status, 200);
});

test('collections add and remove people, and tell exactly those people', async () => {
  const h = fixture();
  resetRevisions(h);
  const both = await call(h, 'owner', 'PUT', `/api/ciphers/${CIPHER}/collections`, { collectionIds: [COL_A, COL_B] });
  assert.equal(both.status, 200);
  assert.deepEqual(both.body.collectionIds, [COL_A, COL_B]);
  assert.equal((await call(h, 'bob', 'GET', `/api/ciphers/${CIPHER}`)).status, 200);
  assert.equal(wasBumped(h, U.bob), true);

  resetRevisions(h);
  const onlyB = await call(h, 'owner', 'POST', `/api/ciphers/${CIPHER}/collections`, { collectionIds: [COL_B] });
  assert.equal(onlyB.status, 200);
  assert.equal((await call(h, 'alice', 'GET', `/api/ciphers/${CIPHER}`)).status, 404);
  assert.equal(wasBumped(h, U.alice), true);
});

test('a read-only member may not change collections', async () => {
  const h = fixture();
  const res = await call(h, 'alice', 'PUT', `/api/ciphers/${CIPHER}/collections`, { collectionIds: [COL_A] });
  assert.equal(res.status, 403);
  assert.equal(res.body.error, FORBIDDEN);
});

test('collections_v2 answers optionalCipherDetails', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'PUT', `/api/ciphers/${CIPHER}/collections_v2`, { collectionIds: [COL_A] });
  assert.equal(res.status, 200);
  assert.equal(res.body.object, 'optionalCipherDetails');
  assert.equal(res.body.unavailable, false);
  assert.equal(res.body.cipher.id, CIPHER);
});

test('an editor rewrites only the collections they hold, and only writable ones', async () => {
  const h = fixture();
  seedCipher(h, SHARED_AB, U.owner, { organizationId: ORG, collectionIds: [COL_A, COL_B] });
  const res = await call(h, 'erin', 'PUT', `/api/ciphers/${SHARED_AB}/collections`, { collectionIds: [COL_C] });
  assert.equal(res.status, 200);
  // COL_B is not erin's: it is kept. COL_A is erin's and writable: removed. COL_C added.
  const stored = (h.db.prepare('SELECT collection_id FROM collection_items WHERE cipher_id = ? ORDER BY collection_id').all(SHARED_AB) as any[])
    .map((row) => row.collection_id);
  assert.deepEqual(stored, [COL_B, COL_C]);
  assert.deepEqual(res.body.collectionIds, [COL_C]);

  // Adding a collection erin does not hold (bob's) needs write access to it.
  const refused = await call(h, 'erin', 'PUT', `/api/ciphers/${CIPHER}/collections`, { collectionIds: [COL_A, COL_B] });
  assert.equal(refused.status, 400);
  assert.equal(refused.body.error, 'You do not have access to one of the selected collections');
});

test('a personal record has no collections to change', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'PUT', `/api/ciphers/${PERSONAL}/collections`, { collectionIds: [COL_A] });
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'Cipher is not in an organization');
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL in `sharing-share.test.ts`. `PUT .../share` answers `404` (no route; only the POST stub exists, and it returns the cipher unchanged), and `/collections` answers `404`. The 50 earlier tests pass.

- [ ] **Step 3: Export two helpers from `ciphers.ts`**

In `src/handlers/ciphers.ts`, change `function cipherResponseOptionsForRequest(request: Request): CipherResponseOptions {` to `export function cipherResponseOptionsForRequest(request: Request): CipherResponseOptions {`, and `function notifyCipherUpdateForRequest(` to `export function notifyCipherUpdateForRequest(`.

- [ ] **Step 4: Push to the people who can see an item**

In `src/handlers/org-ciphers.ts`, replace

```ts
import { getOrganizationMemberIds } from '../services/storage-organization-repo';
```

with

```ts
import { canAccessAllCollections, getOrganizationMemberIds, getOrganizationMembers } from '../services/storage-organization-repo';
```

and append:

```ts
// Who can see an item filed in these collections: every manager, and every
// member holding one of them. A share or a collection change pushes to these
// people only, not to the whole organization: in a sharing organization the
// rest are other people the sharer shares with, and each push makes their
// clients download the whole vault for nothing.
export async function getCipherAudienceUserIds(
  db: D1Database,
  organizationId: string,
  collectionIds: readonly string[]
): Promise<string[]> {
  const wanted = new Set(collectionIds);
  const members = await getOrganizationMembers(db, organizationId);
  return members
    .filter((member) => member.status === 1
      && (canAccessAllCollections(member) || member.collections.some((entry) => wanted.has(entry.id))))
    .map((member) => member.userId);
}

// The acting user is left out: they get the single-cipher push instead.
export async function notifyCipherAudience(
  env: Env,
  organizationId: string,
  collectionIds: readonly string[],
  exceptUserId: string,
  contextId?: string | null
): Promise<void> {
  const audience = await getCipherAudienceUserIds(env.DB, organizationId, collectionIds);
  await bumpUserRevisions(env, audience.filter((id) => id !== exceptUserId), contextId);
}
```

- [ ] **Step 5: Write the routes**

Create `src/handlers/cipher-sharing.ts`:

```ts
import type { Env } from '../types';
import { StorageService } from '../services/storage';
import { errorResponse, jsonResponse } from '../utils/response';
import { readActingDeviceIdentifier } from '../utils/device';
import {
  readOrgCipherFields,
  resolveCipherAccess,
  resolveEditableCipher,
  setOrgCipherFields,
  validateOrgCollectionsForCipher,
} from '../services/storage-org-access';
import {
  addCollectionItem,
  canAccessAllCollections,
  getCollectionsForOrganization,
  removeCollectionItem,
} from '../services/storage-organization-repo';
import {
  STALE_CIPHER_MESSAGE,
  cipherResponseOptionsForRequest,
  cipherToResponse,
  isStaleCipherUpdate,
  mergeCipherUpdateBody,
  notifyCipherUpdateForRequest,
  readCipherRevisionDate,
  viewerFromAccess,
} from './ciphers';
import { notifyCipherAudience } from './org-ciphers';

// Moving a record into an organization, and changing which of its collections
// hold it: Bitwarden's routes, which Deskwarden's sharing is built on.
//
// A share keeps the cipher's id and its author (userId). NodeWarden's model is
// that an organization cipher belongs to whoever wrote it, and the sharer stays
// that author.

function uniqueIds(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return Array.from(new Set(value.map((id) => String(id || '').trim()).filter(Boolean)));
}

async function readBody(request: Request): Promise<any | null> {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

// PUT /api/ciphers/:id/share   (POST is the obsolete alias)
// { cipher: CipherRequestModel with organizationId set and its fields, or its
//   cipher key, re-encrypted under the org key; collectionIds: at least one }
export async function handleShareCipher(request: Request, env: Env, userId: string, cipherId: string): Promise<Response> {
  const storage = new StorageService(env.DB);
  const access = await resolveCipherAccess(env.DB, userId, cipherId);
  if (!access) return errorResponse('Cipher not found', 404);
  // Only its author's personal record can be shared. An organization record
  // changes people through /collections instead.
  if (access.organizationId || readOrgCipherFields(access.cipher).collectionIds.length > 0) {
    return errorResponse('Cipher is already in an organization', 400);
  }

  const body = await readBody(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);
  const cipherData = body.cipher ?? body.Cipher;
  if (!cipherData || typeof cipherData !== 'object') return errorResponse('Cipher is required', 400);

  const organizationId = String(cipherData.organizationId ?? cipherData.OrganizationId ?? '').trim();
  if (!organizationId) return errorResponse('Organization is required', 400);
  const collectionIds = uniqueIds(body.collectionIds ?? body.CollectionIds);
  if (!collectionIds.length) return errorResponse('At least one collection is required', 400);

  // Every collection is this organization's and writable by the caller: a
  // manager, or a member holding it writable.
  const validation = await validateOrgCollectionsForCipher(env.DB, userId, organizationId, collectionIds);
  if (!validation.ok) return errorResponse(validation.error || 'Organization access denied', 400);

  const existing = access.cipher;
  if (isStaleCipherUpdate(existing.updatedAt, readCipherRevisionDate(cipherData))) {
    return errorResponse(STALE_CIPHER_MESSAGE, 400);
  }

  const merged = mergeCipherUpdateBody(existing, cipherData, { method: 'PUT', preserveRevisionDate: false });
  if (!merged.ok) return errorResponse(merged.error, 400);
  const cipher = merged.cipher;

  // Without a cipher key, each attachment's key is wrapped under the personal
  // key. An official client re-shares every attachment first; this server has
  // no route for that, so the record is refused whole rather than moved with
  // files nobody in the organization can open.
  const attachments = await storage.getAttachmentsByCipher(cipher.id);
  if (attachments.length > 0 && !cipher.key) {
    return errorResponse('This item has attachments that must be re-shared by the client', 400);
  }

  // Also clears folder, favourite and archive: personal state on a shared row.
  setOrgCipherFields(cipher, organizationId, collectionIds);
  for (const collectionId of collectionIds) {
    await addCollectionItem(env.DB, collectionId, cipher.id);
  }
  await storage.saveCipher(cipher);

  const revisionDate = await storage.updateRevisionDate(userId);
  notifyCipherUpdateForRequest(request, env, cipher, revisionDate);
  await notifyCipherAudience(env, organizationId, collectionIds, userId, readActingDeviceIdentifier(request));

  const after = await resolveCipherAccess(env.DB, userId, cipher.id);
  return jsonResponse(cipherToResponse(cipher, attachments, {
    ...cipherResponseOptionsForRequest(request),
    viewer: after ? viewerFromAccess(after) : undefined,
  }));
}

// PUT /api/ciphers/:id/collections      (POST alias)  -> cipherDetails
// PUT /api/ciphers/:id/collections_v2   (POST alias)  -> optionalCipherDetails
// { collectionIds: [...] }
export async function handleUpdateCipherCollections(
  request: Request,
  env: Env,
  userId: string,
  cipherId: string,
  v2: boolean
): Promise<Response> {
  const storage = new StorageService(env.DB);
  const editable = await resolveEditableCipher(env.DB, userId, cipherId);
  if (!editable.ok) return errorResponse(editable.error, editable.status);
  const access = editable.access;
  const organizationId = access.organizationId;
  const member = access.member;
  if (!organizationId || !member) return errorResponse('Cipher is not in an organization', 400);

  const body = await readBody(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);
  const rawIds = body.collectionIds ?? body.CollectionIds;
  if (!Array.isArray(rawIds)) return errorResponse('collectionIds array is required', 400);
  const requested = uniqueIds(rawIds);

  const orgCollectionIds = new Set((await getCollectionsForOrganization(env.DB, organizationId)).map((collection) => collection.id));
  if (requested.some((id) => !orgCollectionIds.has(id))) {
    return errorResponse('A selected collection does not belong to this organization', 400);
  }

  const current = readOrgCipherFields(access.cipher).collectionIds;
  let target: string[];
  if (canAccessAllCollections(member)) {
    target = requested;
  } else {
    // A member rewrites only the collections they hold; the item stays in the
    // ones they cannot see. Adding or removing one needs write access to it.
    const held = new Set(member.collections.map((entry) => entry.id));
    const writable = new Set(member.collections.filter((entry) => !entry.readOnly).map((entry) => entry.id));
    const currentSet = new Set(current);
    const requestedSet = new Set(requested);
    const added = requested.filter((id) => !currentSet.has(id));
    const removed = current.filter((id) => held.has(id) && !requestedSet.has(id));
    if ([...added, ...removed].some((id) => !writable.has(id))) {
      return errorResponse('You do not have access to one of the selected collections', 400);
    }
    target = Array.from(new Set([...current.filter((id) => !held.has(id)), ...requested]));
    if (!target.length) return errorResponse('A collection is required for organization ciphers', 400);
  }

  const before = new Set(current);
  const after = new Set(target);
  for (const id of current) {
    if (!after.has(id)) await removeCollectionItem(env.DB, id, cipherId);
  }
  for (const id of target) {
    if (!before.has(id)) await addCollectionItem(env.DB, id, cipherId);
  }

  const cipher = access.cipher;
  setOrgCipherFields(cipher, organizationId, target);
  cipher.updatedAt = new Date().toISOString();
  await storage.saveCipher(cipher);

  const revisionDate = await storage.updateRevisionDate(userId);
  notifyCipherUpdateForRequest(request, env, cipher, revisionDate);
  // Everyone who gained the item and everyone who lost it.
  await notifyCipherAudience(env, organizationId, Array.from(new Set([...current, ...target])), userId, readActingDeviceIdentifier(request));

  const [stillSeen, attachments] = await Promise.all([
    resolveCipherAccess(env.DB, userId, cipherId),
    storage.getAttachmentsByCipher(cipherId),
  ]);
  const viewer = stillSeen ? viewerFromAccess(stillSeen) : { edit: false, viewPassword: true, collectionIds: [] };
  const response = cipherToResponse(cipher, attachments, { ...cipherResponseOptionsForRequest(request), viewer });
  if (v2) {
    return jsonResponse({ object: 'optionalCipherDetails', unavailable: !stillSeen, cipher: stillSeen ? response : null });
  }
  return jsonResponse(response);
}
```

- [ ] **Step 6: Wire the routes**

In `src/router-authenticated.ts`, add after the `import { handleSync } from './handlers/sync';` line:

```ts
import { handleShareCipher, handleUpdateCipherCollections } from './handlers/cipher-sharing';
```

Replace

```ts
    if (subPath === '/share' && method === 'POST') return handleGetCipher(request, env, userId, cipherId);
```

with

```ts
    if (subPath === '/share' && (method === 'PUT' || method === 'POST')) return handleShareCipher(request, env, userId, cipherId);
    if ((subPath === '/collections' || subPath === '/collections_v2') && (method === 'PUT' || method === 'POST')) {
      return handleUpdateCipherCollections(request, env, userId, cipherId, subPath === '/collections_v2');
    }
```

- [ ] **Step 7: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 63`, `ℹ pass 63`, `ℹ fail 0`.

Run: `pnpm run test:notification-pushes`
Expected: `ℹ fail 0`.

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 8: Commit**

```bash
git add src/handlers/cipher-sharing.ts src/handlers/ciphers.ts src/handlers/org-ciphers.ts src/router-authenticated.ts scripts/sharing-share.test.ts
git commit -m "Share moves a record into an organization, and /collections changes who has it" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Profile organisations official clients can use, and the official create body

**Files:**
- Modify: `src/utils/profile-response.ts` (`organizationForMember`, `buildProfileResponse`)
- Modify: `src/handlers/organizations.ts` (`handleCreateOrganization` aliases, `firstNonEmptyString`)
- Test: `scripts/sharing-profile.test.ts`

**Interfaces:**
- Consumes: `orgUserTypeForRole`, `orgUserStatusForMember` (Task 5; `org-user-model.ts` does not import the notifications hub, so `pnpm run test:organizations`, which runs without the workers stub, still loads).
- Produces:
  - `organizationForMember(org, membership: { userId?: string; role?: string; permissions?: unknown; encryptedOrgKey?: string | null } | undefined)` adds `type` (0–4), `status` (2 with a key, else 1), `userId`, `organizationUserId`, `hasPublicAndPrivateKeys`, `usePolicies: true`, `selfHost: true`.
  - `profile.organizations` holds Confirmed memberships only; `profile.organizationsNew` holds every accepted one.
  - `POST /api/organizations` accepts `key` as `encryptedOrgKey`, `keys.publicKey` as `publicKey` and `keys.encryptedPrivateKey` as `privateKey`. `collectionName`, `planType` and the payment fields are ignored (Deskwarden then creates the person collection itself, spec 9.5).

`hasPublicAndPrivateKeys` follows the official model (`PublicKey != null && PrivateKey != null`) rather than spec 10.5's `!!org.publicKey`; for an organization created by Deskwarden both are set, so the two agree.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-profile.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { organizationForMember } from '../src/utils/profile-response';
import {
  ENC, ORG, T0, U, call, createHarness, emailOf, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// Official clients read `type` and `status` to decide whether an organization is
// usable, and decrypt with every entry of `organizations`. An unconfirmed
// member's org (no key yet) belongs in `organizationsNew` only.

const ORG_ROW = {
  id: 'org-1',
  name: 'Shared by owner@example.test',
  enabled: true,
  useTotp: false,
  ownerId: 'user-1',
  billingEmail: null,
  publicKey: 'ORG-PUBLIC-SPKI',
  privateKey: '2.cHJpdg==|eA==|eQ==',
  identifier: null,
  createdAt: T0,
  updatedAt: T0,
} as any;

test('a profile organization carries the fields official clients read, numbers where they expect numbers', () => {
  const seen = organizationForMember(ORG_ROW, { userId: 'user-1', role: 'member', permissions: {}, encryptedOrgKey: '4.a2V5' });
  // Names from ProfileOrganizationResponseModel and BaseProfileOrganizationResponseModel (bitwarden/server).
  for (const key of [
    'id', 'name', 'enabled', 'identifier', 'key', 'userId', 'organizationUserId', 'status', 'type',
    'permissions', 'usePolicies', 'useTotp', 'selfHost', 'hasPublicAndPrivateKeys',
  ]) {
    assert.ok(Object.prototype.hasOwnProperty.call(seen, key), `missing ${key}`);
  }
  assert.equal(seen.type, 2);
  assert.equal(seen.status, 2);
  assert.equal(seen.userId, 'user-1');
  assert.equal(seen.organizationUserId, 'user-1');
  assert.equal(seen.hasPublicAndPrivateKeys, true);
  assert.equal(seen.usePolicies, true);
  assert.equal(seen.selfHost, true);

  const roles: Array<[string, number]> = [['owner', 0], ['admin', 1], ['member', 2], ['manager', 3], ['custom', 4]];
  for (const [role, type] of roles) {
    assert.equal(organizationForMember(ORG_ROW, { role, encryptedOrgKey: '4.a2V5' }).type, type, role);
  }
  assert.equal(organizationForMember(ORG_ROW, { role: 'member', encryptedOrgKey: null }).status, 1);
  assert.equal(organizationForMember({ ...ORG_ROW, privateKey: null }, { role: 'member' }).hasPublicAndPrivateKeys, false);
});

test('organizations holds confirmed memberships; organizationsNew holds every accepted one', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedOrg(h, ORG, U.owner);
  seedMember(h, ORG, U.alice); // confirmed
  seedMember(h, ORG, U.bob, { key: null }); // accepted, not confirmed

  const bob = (await call(h, 'bob', 'GET', '/api/sync')).body.profile;
  assert.deepEqual(bob.organizations, []);
  assert.equal(bob.organizationsNew.length, 1);
  assert.equal(bob.organizationsNew[0].id, ORG);
  assert.equal(bob.organizationsNew[0].status, 1);
  assert.equal(bob.organizationsNew[0].type, 2);
  assert.equal(bob.organizationsNew[0].key, null);

  const alice = (await call(h, 'alice', 'GET', '/api/sync')).body.profile;
  assert.equal(alice.organizations.length, 1);
  assert.equal(alice.organizations[0].status, 2);
  assert.equal(alice.organizations[0].key, `ORGKEY-${U.alice}`);
  assert.equal(alice.organizationsNew.length, 1);
});

test('an organization created with the official body keeps its keys', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  const res = await call(h, 'owner', 'POST', '/api/organizations', {
    name: 'Shared by owner@example.test',
    billingEmail: emailOf('owner'),
    planType: 0,
    key: '4.b3JnLWtleQ==',
    keys: { publicKey: 'ORG-PUBLIC-SPKI', encryptedPrivateKey: '2.cHJpdg==|eA==|eQ==' },
    collectionName: ENC,
  });
  assert.equal(res.status, 200);

  const keys = await call(h, 'owner', 'GET', `/api/organizations/${res.body.id}/keys`);
  assert.equal(keys.body.key, '4.b3JnLWtleQ==');
  assert.equal(keys.body.publicKey, 'ORG-PUBLIC-SPKI');
  assert.equal(keys.body.privateKey, '2.cHJpdg==|eA==|eQ==');

  const profile = (await call(h, 'owner', 'GET', '/api/sync')).body.profile;
  const org = profile.organizations.find((candidate: any) => candidate.id === res.body.id);
  assert.equal(org.type, 0);
  assert.equal(org.status, 2);
  assert.equal(org.hasPublicAndPrivateKeys, true);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 3 failures in `sharing-profile.test.ts` (`missing userId`; `bob.organizations` holds the keyless org; `keys.body.key` is `null`). The 63 earlier tests pass.

- [ ] **Step 3: The profile organization**

In `src/utils/profile-response.ts`, add after the existing imports:

```ts
import { orgUserStatusForMember, orgUserTypeForRole } from '../services/org-user-model';
```

Replace the function `organizationForMember`, from the line `export function organizationForMember(` to its closing brace (keep the `/** ... */` comment above it), with:

```ts
export function organizationForMember(
  org: NonNullable<Awaited<ReturnType<typeof getOrganizationsForUser>>[number]>,
  membership: { userId?: string; role?: string; permissions?: unknown; encryptedOrgKey?: string | null } | undefined,
): Record<string, unknown> {
  const base = toProfileOrganization(org);
  const role = membership?.role ?? 'member';
  // The member's own RSA-wrapped copy of the organization's symmetric key,
  // in the field every Bitwarden client reads it from. `encryptedOrgKey` is
  // kept beside it rather than instead of it -- that is the spelling this
  // server has always sent, so anything already reading it keeps working.
  const wrapped = membership?.encryptedOrgKey ?? null;
  const userId = membership?.userId ?? null;
  return {
    ...base,
    // Members decrypt with the symmetric org key; only admins get the org
    // private key.
    privateKey: role === 'owner' || role === 'admin' ? base.privateKey : null,
    role,
    permissions: membership?.permissions ?? {},
    key: wrapped,
    encryptedOrgKey: wrapped,
    users: [],
    // What official clients read to decide whether an organization is usable:
    // Bitwarden's OrganizationUserType and OrganizationUserStatusType.
    // Confirmed (2) means this member holds the wrapped key.
    type: orgUserTypeForRole(role),
    status: orgUserStatusForMember({ encryptedOrgKey: wrapped }),
    userId,
    // NodeWarden's organizationUserId for a member is their user id.
    organizationUserId: userId,
    hasPublicAndPrivateKeys: !!org.publicKey && !!org.privateKey,
    usePolicies: true,
    selfHost: true,
  };
}
```

In `buildProfileResponse`, replace

```ts
  let organizations: any[] = [];
  if (env?.DB) {
```

with

```ts
  let organizations: any[] = [];
  let organizationsNew: any[] = [];
  if (env?.DB) {
```

replace

```ts
      organizations = orgs
        .filter((org) => org.enabled)
        .map((org) => organizationForMember(org, memberByOrg.get(org.id)));
    } catch (error) {
      console.error('Failed to load organizations for profile:', error);
      organizations = [];
    }
```

with

```ts
      organizationsNew = orgs
        .filter((org) => org.enabled)
        .map((org) => organizationForMember(org, memberByOrg.get(org.id)));
      // As on official servers: `organizations` is Confirmed only -- the list a
      // client decrypts with -- and `organizationsNew` is every accepted
      // membership, keyed or not.
      organizations = organizationsNew.filter((org) => org.status === 2);
    } catch (error) {
      console.error('Failed to load organizations for profile:', error);
      organizations = [];
      organizationsNew = [];
    }
```

and replace

```ts
    premiumFromOrganization: organizations.length > 0,
```

with

```ts
    premiumFromOrganization: organizationsNew.length > 0,
```

and replace

```ts
    organizationsNew: organizations,
```

with

```ts
    organizationsNew,
```

- [ ] **Step 4: The official create body**

In `src/handlers/organizations.ts`, add directly after the closing brace of `normalizeRole`:

```ts
function firstNonEmptyString(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === 'string' && value) return value;
  }
  return null;
}
```

In `handleCreateOrganization`, replace

```ts
  const now = new Date().toISOString();
  const orgId = generateUUID();
  const org: Organization = {
```

with

```ts
  // The official create body names the keys differently: `key` is the org key
  // wrapped to the creator, `keys` is the org's own key pair.
  const officialKeys = body?.keys && typeof body.keys === 'object' ? body.keys : {};

  const now = new Date().toISOString();
  const orgId = generateUUID();
  const org: Organization = {
```

replace

```ts
    publicKey: typeof body?.publicKey === 'string' && body.publicKey ? body.publicKey : null,
    privateKey: typeof body?.privateKey === 'string' && body.privateKey ? body.privateKey : null,
```

with

```ts
    publicKey: firstNonEmptyString(body?.publicKey, officialKeys.publicKey),
    privateKey: firstNonEmptyString(body?.privateKey, officialKeys.encryptedPrivateKey),
```

and replace

```ts
    collections: [],
    encryptedOrgKey: typeof body?.encryptedOrgKey === 'string' && body.encryptedOrgKey ? body.encryptedOrgKey : null,
```

with

```ts
    collections: [],
    encryptedOrgKey: firstNonEmptyString(body?.encryptedOrgKey, body?.key),
```

- [ ] **Step 5: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 66`, `ℹ pass 66`, `ℹ fail 0`.

Run: `pnpm run test:organizations`
Expected: `ℹ pass 11`, `ℹ fail 0` (the existing profile test still holds: `key` is the member's wrapped key).

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add src/utils/profile-response.ts src/handlers/organizations.ts scripts/sharing-profile.test.ts
git commit -m "A profile organization says its type and status, and lists unconfirmed ones apart" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: A recipient never inherits the sharer's organization

**Files:**
- Modify: `src/services/storage-organization-repo.ts` (`transferOrganizationDataOnUserDelete`)
- Test: `scripts/sharing-successor.test.ts`

**Interfaces:**
- Consumes: `canAccessAllCollections`, `orgRoleRank` (same file).
- Produces: `transferOrganizationDataOnUserDelete(db, userId): Promise<string[]>` keeps its signature and caller (`handleAdminDeleteUser`). An owned organization passes only to an accepted owner, admin or manager. With none, its items are handed to the account being deleted and the organization is deleted, so the admin handler's attachment purge and the account cascade that follow remove the items and their files, "as for an org of one" (spec 10.2). Every other member is returned as affected, so they resync.

This also fixes an adjacent defect the new branch depends on. `attachments.user_id` is a copy of the cipher's owner, and the admin handler purges files by it right after this function runs. The existing transfer reassigned `ciphers.user_id` without that copy, so a handed-over item's files were purged. Both directions now re-derive the copy.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-successor.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { transferOrganizationDataOnUserDelete } from '../src/services/storage-organization-repo';
import { getAttachmentsByUserId } from '../src/services/storage-attachment-repo';
import {
  ALICE_PERSONAL, ATT, CIPHER, COL_A, ORG, U, count, createHarness, readOnlyAccess,
  seedAttachment, seedCipher, seedCollection, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// Deleting the sharer's account used to hand the sharing organization to the
// highest-ranked survivor -- a plain recipient -- who would then reach every
// collection: every record shared with anyone.

const CIPHER_BY_ALICE = 'dddddddd-0000-4000-8000-0000000000a1';

test("a plain recipient never inherits the sharer's organization", async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  seedCipher(h, CIPHER_BY_ALICE, U.alice, { organizationId: ORG, collectionIds: [COL_A] });
  seedCipher(h, ALICE_PERSONAL, U.alice);
  seedAttachment(h, ATT, CIPHER_BY_ALICE);

  const affected = await transferOrganizationDataOnUserDelete(h.env.DB, U.owner);
  assert.deepEqual(affected, [U.alice]);
  // The purge that follows reads attachments by the deleted account: it now
  // finds the organization item's file, so the file goes with the item.
  assert.deepEqual([...(await getAttachmentsByUserId(h.env.DB, U.owner)).keys()], [CIPHER_BY_ALICE]);

  h.db.prepare('DELETE FROM users WHERE id = ?').run(U.owner);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organizations'), 0);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE user_id = ?', U.alice), 0);
  const left = (h.db.prepare('SELECT id FROM ciphers ORDER BY id').all() as any[]).map((row) => row.id);
  assert.deepEqual(left, [ALICE_PERSONAL]);
});

test('a manager still inherits, and the items handed over keep their files', async () => {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'erin');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] });
  seedMember(h, ORG, U.erin, { role: 'admin' });
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  seedAttachment(h, ATT, CIPHER);

  const affected = await transferOrganizationDataOnUserDelete(h.env.DB, U.owner);
  assert.deepEqual([...affected].sort(), [U.alice, U.erin].sort());
  assert.equal((h.db.prepare('SELECT owner_id FROM organizations WHERE id = ?').get(ORG) as any).owner_id, U.erin);
  const role = (userId: string) =>
    (h.db.prepare('SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, userId) as any).role;
  assert.equal(role(U.erin), 'owner');
  assert.equal(role(U.alice), 'member');
  assert.equal((h.db.prepare('SELECT user_id FROM ciphers WHERE id = ?').get(CIPHER) as any).user_id, U.erin);
  assert.equal((h.db.prepare('SELECT user_id FROM attachments WHERE id = ?').get(ATT) as any).user_id, U.erin);
  assert.equal((await getAttachmentsByUserId(h.env.DB, U.owner)).size, 0);

  h.db.prepare('DELETE FROM users WHERE id = ?').run(U.owner);
  assert.equal(count(h, 'SELECT COUNT(*) FROM ciphers WHERE id = ?', CIPHER), 1);
  assert.equal(count(h, 'SELECT COUNT(*) FROM attachments WHERE id = ?', ATT), 1);
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 2 failures in `sharing-successor.test.ts`. In the first, alice is promoted, so `affected` matches but the purge finds nothing: the attachment keys are `[]`, expected `[CIPHER_BY_ALICE]`. In the second, the attachment's `user_id` is still the owner's (`aaaaaaaa-...-000000000001`), expected erin's (`...-000000000006`). The 66 earlier tests pass.

- [ ] **Step 3: Implement**

In `src/services/storage-organization-repo.ts`, replace the whole `transferOrganizationDataOnUserDelete` function, with the comment block above it, from `// Both \`organizations.owner_id\` and \`ciphers.user_id\` cascade from \`users\`, so` to the function's closing brace, with:

```ts
// Both `organizations.owner_id` and `ciphers.user_id` cascade from `users`, so
// deleting an account would otherwise destroy an organization it owns and every
// org cipher it authored. Hand that data to a surviving manager first. Returns
// the member ids whose sync state was affected.
export async function transferOrganizationDataOnUserDelete(db: D1Database, userId: string): Promise<string[]> {
  const affected = new Set<string>();
  const now = new Date().toISOString();

  // 1. An organization owned by the account passes to the highest-ranked
  //    remaining member who already reaches every collection: an owner, admin
  //    or manager. A plain member never inherits. In a sharing organization a
  //    plain member is a person something was shared with, and ownership would
  //    hand them every record shared with anyone else. With no such member the
  //    organization goes, items and all, as an organization of one does.
  const owned = await db
    .prepare('SELECT id FROM organizations WHERE owner_id = ?')
    .bind(userId)
    .all<{ id: string }>();
  for (const row of owned.results || []) {
    const others = (await getOrganizationMembers(db, row.id)).filter(
      (member) => member.userId !== userId && member.status === 1
    );
    const successors = others.filter((member) => canAccessAllCollections(member));
    successors.sort((a, b) => orgRoleRank(b.role) - orgRoleRank(a.role) || a.createdAt.localeCompare(b.createdAt));
    const successor = successors[0];
    if (!successor) {
      // Hand the organization's items to the account being deleted, so the
      // attachment purge and the account cascade that follow take them, and
      // their files, with it.
      await db
        .prepare(
          `UPDATE ciphers SET user_id = ?, updated_at = ?
           WHERE user_id <> ? AND (
             id IN (
               SELECT ci.cipher_id FROM collection_items ci
               JOIN collections c ON c.id = ci.collection_id
               WHERE c.organization_id = ?
             )
             OR (json_valid(data) AND json_extract(data, '$.organizationId') = ?)
           )`
        )
        .bind(userId, now, userId, row.id, row.id)
        .run();
      await deleteOrganization(db, row.id);
      for (const member of others) affected.add(member.userId);
      continue;
    }
    await db
      .prepare('UPDATE organizations SET owner_id = ?, updated_at = ? WHERE id = ?')
      .bind(successor.userId, now, row.id)
      .run();
    successor.role = 'owner';
    successor.updatedAt = now;
    await saveOrganizationMember(db, successor);
    for (const member of others) affected.add(member.userId);
  }

  // 2. Org ciphers the account authored are reassigned to the (possibly new) owner
  //    so they survive the cascade.
  const memberships = await db
    .prepare('SELECT organization_id FROM organization_members WHERE user_id = ?')
    .bind(userId)
    .all<{ organization_id: string }>();
  for (const row of memberships.results || []) {
    const org = await getOrganization(db, row.organization_id);
    if (!org || org.ownerId === userId) continue;
    // Match both collection-linked org ciphers AND unassigned ones (admins may
    // create org ciphers with no collection) — the latter would otherwise
    // cascade-delete with the account.
    await db
      .prepare(
        `UPDATE ciphers SET user_id = ?, updated_at = ?
         WHERE user_id = ? AND (
           id IN (
             SELECT ci.cipher_id FROM collection_items ci
             JOIN collections c ON c.id = ci.collection_id
             WHERE c.organization_id = ?
           )
           OR (json_valid(data) AND json_extract(data, '$.organizationId') = ?)
         )`
      )
      .bind(org.ownerId, now, userId, org.id, org.id)
      .run();
    for (const member of await getOrganizationMemberIds(db, org.id)) affected.add(member);
  }

  // 3. Attachments carry a copy of their cipher's owner, and the purge that
  //    follows (handleAdminDeleteUser) reads that copy. Re-derive it for every
  //    item whose owner just changed, in either direction, or a handed-over
  //    item loses its files and a deleted one keeps them.
  await db
    .prepare(
      'UPDATE attachments SET user_id = (SELECT c.user_id FROM ciphers c WHERE c.id = attachments.cipher_id) ' +
      'WHERE user_id = ? OR cipher_id IN (SELECT id FROM ciphers WHERE user_id = ?)'
    )
    .bind(userId, userId)
    .run();

  affected.delete(userId);
  return Array.from(affected);
}
```

- [ ] **Step 4: Run the tests and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 68`, `ℹ pass 68`, `ℹ fail 0`.

Run: `pnpm run test:organizations`
Expected: `ℹ pass 11`, `ℹ fail 0`. (Its mock-database tests still hold: the admin survivor is chosen, and an org with no other member is deleted with `affected` empty.)

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add src/services/storage-organization-repo.ts scripts/sharing-successor.test.ts
git commit -m "Deleting a sharer never makes a recipient the owner of what they were shown" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: An admin sharer mints a registration code for a person with no account

**Files:**
- Modify: `src/handlers/admin.ts` (export `requireMasterPasswordHash`, `randomHex`, `toInviteResponse`)
- Modify: `src/handlers/organization-users.ts` (`handleCreateRegistrationInviteForOrgUser`)
- Modify: `src/router-authenticated.ts`
- Test: `scripts/sharing-registration-invite.test.ts`

**Interfaces:**
- Consumes: `resolveOrgUser` (Task 5); `requireManager`, `readJson`, `writeOrgAudit` (Task 5).
- Produces: `POST /api/organizations/{orgId}/users/{ouId}/registration-invite` `{ masterPasswordHash }` → `201` with the same body as `POST /api/admin/invites` (`{object:"invite", code, status, createdBy, usedBy, createdAt, updatedAt, expiresAt, inviteLink}`).

Registration after the first user is invite-code-only, and only the server admin issues codes (spec 3.1, Q3). Owner decision: when the sharer is the admin, mint the code for them. The route is scoped to one pending invitation of an organization the caller manages, refuses when an account already exists for that email, and expires the code with the invitation. It asks for the master password exactly as `POST /api/admin/invites` does, so it opens no path around that re-authentication.

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-registration-invite.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  INV_CAROL, ORG, U, call, count, createHarness, emailOf, seedInvitation, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// Someone invited to share before they have an account cannot register without
// a code from the server's admin. When the sharer is that admin, they can mint
// one here and pass it along.

const INV_DAVE = 'eeeeeeee-0000-4000-8000-000000000002';
const CAROL_EXPIRY = new Date(Date.now() + 5 * 24 * 60 * 60 * 1000).toISOString();

function fixture(ownerRole: 'admin' | 'user' = 'admin') {
  const h = createHarness();
  seedUser(h, 'owner', { role: ownerRole });
  seedUser(h, 'alice');
  seedUser(h, 'dave');
  seedOrg(h, ORG, U.owner);
  seedMember(h, ORG, U.alice);
  seedInvitation(h, ORG, INV_CAROL, emailOf('carol'), { invitedBy: U.owner, expiresAt: CAROL_EXPIRY });
  seedInvitation(h, ORG, INV_DAVE, emailOf('dave'), { invitedBy: U.owner });
  return h;
}

const mint = (h: ReturnType<typeof createHarness>, person: 'owner' | 'alice', ouId: string, masterPasswordHash: string) =>
  call(h, person, 'POST', `/api/organizations/${ORG}/users/${ouId}/registration-invite`, { masterPasswordHash });

test('an admin sharer mints a registration code that expires with the invitation', async () => {
  const h = fixture();
  const res = await mint(h, 'owner', INV_CAROL, 'hash-owner');
  assert.equal(res.status, 201);
  assert.equal(res.body.object, 'invite');
  assert.match(res.body.code, /^[0-9a-f]{40}$/);
  assert.equal(res.body.expiresAt, CAROL_EXPIRY);
  assert.ok(String(res.body.inviteLink).endsWith(`/?invite=${res.body.code}`));

  const row = h.db.prepare('SELECT status, created_by, expires_at FROM invites WHERE code = ?').get(res.body.code) as any;
  assert.equal(row.status, 'active');
  assert.equal(row.created_by, U.owner);
  assert.equal(row.expires_at, CAROL_EXPIRY);
});

test('only a server admin, with the master password, for someone who has no account', async () => {
  const h = fixture();
  const cases: Array<[Promise<any>, number, string | null]> = [
    [mint(h, 'owner', INV_CAROL, 'wrong'), 400, 'Invalid password'],
    [mint(h, 'owner', INV_DAVE, 'hash-owner'), 400, 'This person already has an account'],
    [mint(h, 'owner', U.alice, 'hash-owner'), 400, 'This person already has an account'],
    [mint(h, 'alice', INV_CAROL, 'hash-alice'), 404, null],
  ];
  for (const [pending, status, message] of cases) {
    const res = await pending;
    assert.equal(res.status, status, message ?? 'plain member');
    if (message) assert.equal(res.body.error, message);
  }
  assert.equal(count(h, 'SELECT COUNT(*) FROM invites'), 0);

  const notAdmin = fixture('user');
  const refused = await mint(notAdmin, 'owner', INV_CAROL, 'hash-owner');
  assert.equal(refused.status, 403);
  assert.equal(refused.body.error, 'Only a server administrator can create registration invites');
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing`
Expected: FAIL: 2 failures in `sharing-registration-invite.test.ts` (the route answers `405 Method not allowed`, so `405 !== 201`). The 68 earlier tests pass.

- [ ] **Step 3: Export the admin helpers**

In `src/handlers/admin.ts`, change `async function requireMasterPasswordHash(` to `export async function requireMasterPasswordHash(`, `function randomHex(bytes: number): string {` to `export function randomHex(bytes: number): string {`, and `function toInviteResponse(request: Request, invite: Invite): Record<string, unknown> {` to `export function toInviteResponse(request: Request, invite: Invite): Record<string, unknown> {`.

- [ ] **Step 4: Add the handler**

In `src/handlers/organization-users.ts`:

Replace `import type { CollectionAccess, Env, OrganizationInvitation, OrganizationMember } from '../types';` with:

```ts
import type { CollectionAccess, Env, Invite, OrganizationInvitation, OrganizationMember, User } from '../types';
import { StorageService } from '../services/storage';
import { randomHex, requireMasterPasswordHash, toInviteResponse } from './admin';
```

Append to the end of the file:

```ts
// POST /api/organizations/:orgId/users/:ouId/registration-invite   { masterPasswordHash }
// Registration here is by invite code only, and only the server admin issues
// codes. A person invited to an organization before they have an account can
// do nothing until they register. When the sharer IS the admin, this mints the
// code for one pending invitation, expiring with it. It asks for the master
// password exactly as POST /api/admin/invites does.
export async function handleCreateRegistrationInviteForOrgUser(
  request: Request,
  env: Env,
  currentUser: User,
  orgId: string,
  ouId: string
): Promise<Response> {
  if (!(await requireManager(env, orgId, currentUser.id))) return errorResponse('Not found', 404);
  if (currentUser.role !== 'admin' || currentUser.status !== 'active') {
    return errorResponse('Only a server administrator can create registration invites', 403);
  }

  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return errorResponse('Not found', 404);
  if (target.kind !== 'invitation' || (await getUser(env.DB, target.invitation.email))) {
    return errorResponse('This person already has an account', 400);
  }
  const invitation = target.invitation;
  if (Date.parse(invitation.expiresAt) < Date.now()) return errorResponse('Invitation has expired', 400);

  const body = (await readJson(request)) ?? {};
  const passwordError = await requireMasterPasswordHash(env, currentUser, body.masterPasswordHash);
  if (passwordError) return passwordError;

  const now = new Date().toISOString();
  const invite: Invite = {
    code: randomHex(20),
    createdBy: currentUser.id,
    usedBy: null,
    expiresAt: invitation.expiresAt,
    status: 'active',
    createdAt: now,
    updatedAt: now,
  };
  await new StorageService(env.DB).createInvite(invite);
  await writeOrgAudit(env, currentUser.id, 'organization.registrationInvite.create', 'organization', orgId, {
    email: invitation.email,
    organizationId: orgId,
  });
  return jsonResponse(toInviteResponse(request, invite), 201);
}
```

- [ ] **Step 5: Wire the route**

In `src/router-authenticated.ts`, add `handleCreateRegistrationInviteForOrgUser,` to the `import { ... } from './handlers/organization-users';` list (after `handleConfirmOrganizationUser,`), and replace

```ts
      if (action === '/confirm' && method === 'POST') return handleConfirmOrganizationUser(request, env, userId, orgId, ouId);
```

with

```ts
      if (action === '/confirm' && method === 'POST') return handleConfirmOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '/registration-invite' && method === 'POST') {
        return handleCreateRegistrationInviteForOrgUser(request, env, currentUser, orgId, ouId);
      }
```

- [ ] **Step 6: Run every suite and the typecheck**

Run: `pnpm run test:sharing`
Expected: `ℹ tests 70`, `ℹ pass 70`, `ℹ fail 0`.

Run each of these; every one ends with `ℹ fail 0`:
- `pnpm run test:organizations`
- `pnpm run test:notification-pushes`
- `pnpm run test:schema`
- `pnpm run test:sends`
- `pnpm run test:backup-blob-keys`
- `pnpm run test:config-compatibility`

Run: `pnpm exec tsc --noEmit`
Expected: no output, exit 0.

- [ ] **Step 7: Commit**

```bash
git add src/handlers/admin.ts src/handlers/organization-users.ts src/router-authenticated.ts scripts/sharing-registration-invite.test.ts
git commit -m "An admin who shares can mint the registration code a new person needs" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage

| Spec item | Task |
|---|---|
| 10.1 `GET /users?includeCollections`, statuses 0/1/2, `invitationExpirationDate` | 5 |
| 10.1 `POST /users/invite` (always an invitation, empty 200) | 5 |
| 10.1 `POST /users/{ouId}/reinvite` (new token, 7 days) | 5 |
| 10.1 `POST /users/{ouId}/accept` | 6 |
| 10.1 `POST /users/{ouId}/confirm`, bulk `POST /users/confirm` | 6 |
| 10.1 `POST /users/public-keys` (`key` field) | 6 |
| 10.1 `DELETE /users/{ouId}` (member or invitation; not the owner) | 5 |
| 10.1 `GET /users/mini-details` manager+ | 5 |
| 10.1 `POST /collections` with `users[]` | 7 |
| 10.1 `POST /organizations/{id}/leave` | 6 |
| 10.1 `GET /api/users/{id}/public-key` (also Deskwarden's slice-1 probe, 7.1) | 6 |
| 10.1 `/members/invite` takes collection objects | 1 |
| 10.1 pushes to the member and every manager on accept, confirm, invite, remove, leave | 5, 6 |
| 10.2 `/members`, `/users`, `mini-details` manager+ | 4, 5 |
| 10.2 sync `collections[]`, org collections, collection items | 4 |
| 10.2 cipher `collectionIds` per viewer | 2 |
| 10.2 account-deletion successor manager+ only; else delete org and items | 11 |
| 10.3 storage of `{id, readOnly, hidePasswords, manage}`, bare string compatible, no schema change | 1 |
| 10.3 `canEdit`; writes need it; create needs a writable collection | 2, 3 |
| 10.3 update may not change `organizationId` | 3 (web-vault exception; see Task 3) |
| 10.3 per-viewer `edit`, `viewPassword`, `permissions` | 2 |
| 10.3 collection names up to 1000 | 7 |
| 10.4 `PUT /share` (+POST alias) and `/collections` (+`_v2`) | 9 |
| 10.5 profile `type`, `status`, `userId`, `hasPublicAndPrivateKeys`, `usePolicies`, `selfHost`; `organizations` vs `organizationsNew`; create-body aliases | 10 |
| Owner decision: rate-limit `members/public-key?email=` (Q4) | 8 |
| Owner decision: keep the direct add (Q1) | unchanged route; pushes added in 6 |
| Owner decision: admin mints a registration code (Q3 b) | 12 |
| 3.1 "An accept pushes only to the acceptor" | 6 |
| 14.3 test list | 1–12 (each bullet maps to a named test above) |

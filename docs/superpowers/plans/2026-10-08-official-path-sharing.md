# Person Sharing on Bitwarden's Official Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deskwarden shares a record with a person through Bitwarden's own routes, bodies and invitation links on every self-hosted server, and NodeWarden answers those routes as a Bitwarden server does, so the official `bw` CLI and web vault work against it.

**Architecture:** Two repos, NodeWarden first.
- **NodeWarden** gains `/config`'s `server` name, Bitwarden's bulk rules, the official `GET`/`PUT …/users/{ouId}` and Bitwarden's `DELETE /ciphers/{id}`. It loses the 47869c4 role-raise refusal. A new `INVITE_MAIL` setting chooses between Vaultwarden's mail-off behaviour (accept on invite, and on registration; the default) and Bitwarden's invitation email, sent through Cloudflare Email Service.
- **Deskwarden**'s sharing engine (`rest/sharing.rs`) stops asking which server it is talking to. It learns *capabilities* from official routes it needs anyway, invites through `POST …/users/invite`, finds its own org by the roster, and accepts a pasted Bitwarden invite link.
  - The recipient's "Shared by" is verified by the roster where members may read it. Elsewhere it is marked unverified.
  - Every NodeWarden-only call on the sharing path goes: direct add, `members/public-key?email=`, `invitations/mine`, `invitations/accept|decline`, the `ownerEmail` attestation, and the `/members` roster fallback.

**Tech Stack:**
- Deskwarden: Rust 2021, egui 0.35, `ureq` through `RestClient`, `url` 2 (already a direct dependency), `zeroize`.
- NodeWarden: TypeScript on Cloudflare Workers, D1, `tsx --test` over a real `node:sqlite` harness, and Cloudflare Email Service's `send_email` binding.

**Specs:** `docs/superpowers/specs/2026-10-08-official-path-sharing-design.md` (this project), amending `docs/superpowers/specs/2026-09-27-personal-sharing-design.md` (the "base spec"). Where the code or Bitwarden's source disagrees with either, see "Decisions this plan takes" below.

## Global Constraints

These apply to every task and every step. A task's requirements include this section.

### Both repos

- **Order.** NodeWarden tasks NW1–NW4 land first, because Deskwarden's compatibility check (DW5) runs against them. DW1–DW5 follow, in order. Each task is one commit and leaves its repo green.
- **Finding each edit.**
  - Line numbers are as of Deskwarden `f2eecc8` (`E:\Personal\node-bitwarden`, `main`) and NodeWarden `47869c4` (`E:\Personal\nodewarden`, `feature/missing-features`). They shift as earlier tasks land.
  - Every replacement quotes its exact old text, and that text occurs exactly once in its file. Find each edit by that text, not by line number.
  - The Edit tool's `old_string` is the quoted block with the code-fence lines removed.
- **Commits.**
  - One commit per task, naming the paths explicitly (`git add <paths>`). Never `git add -A`.
  - The subject is one plain sentence in the repo's style. Examples: Deskwarden "A card's bank and billing ZIP show on its details screen"; NodeWarden "An existing account is made an owner, admin or manager only by accepting an invitation". The body says what changed and why, wrapped at 72.
  - Both repos end the message with:
    ```
    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
    ```
    NodeWarden's own log ends its messages the same way (`git log -3` on `47869c4`).
  - **Don't push.** No release build, no version change, no CHANGELOG change, in either repo.
- **Never log an invite token, an invite link, or a link's query.** No `log::` line in Deskwarden and no `console.*` call in NodeWarden may carry one. Tests in NW4 and DW3 hold this.

### NodeWarden (`E:\Personal\nodewarden`, branch `feature/missing-features`)

- Run every command from `E:\Personal\nodewarden` in Git Bash. Use `pnpm`, never `npm` or `yarn`.
- **LF line endings.** Every `.ts`, `.toml` and `.md` file here is LF.
  - The Edit tool can silently write CRLF. After every step that edits a file, run both checks:
    - `git diff --stat` shows only the lines you meant to change. A whole-file rewrite means CR crept in.
    - `tr -cd '\r' < <file> | wc -c` prints `0` for each file you touched.
  - If a check fails, repair the file with `python -c "import sys; p=sys.argv[1]; b=open(p,'rb').read(); open(p,'wb').write(b.replace(b'\r\n', b'\n'))" <file>` and run both checks again.
  - `src/config/limits.ts` starts with a UTF-8 byte-order mark (`EF BB BF`). Keep it.
- **Verify per task.** The last step of every NodeWarden task runs all of these, and all must pass before the commit:
  1. `pnpm run test:sharing`
  2. `pnpm run test:organizations`
  3. `pnpm run test:notification-pushes`
  4. `pnpm run test:sends`
  5. `pnpm run test:sso`
  6. `pnpm run test:webapp`
  7. `pnpm run test:config-compatibility`
  8. `pnpm exec tsc --noEmit`, printing nothing.
- **Tests live in the existing harness.** `scripts/support/sharing-harness.ts` drives the real authenticated router against `node:sqlite` with the runtime schema. New sharing tests are `scripts/sharing-*.test.ts` files, so `test:sharing`'s glob runs them.

### Deskwarden (`E:\Personal\node-bitwarden\deskwarden`, branch `main`)

- Run every command from `E:\Personal\node-bitwarden\deskwarden` in Git Bash, unless a step says otherwise.
- **CRLF line endings.**
  - Every `.rs` file in `deskwarden/` is CRLF (`.gitattributes`: `text eol=crlf`).
  - Edit `.rs` files only with the Edit tool, or with the Python scripts below, written through the Write tool, which read and write bytes and keep `\r\n`.
  - **Never** use bash heredocs, `sed -i` or `cat >` on a `.rs` file.
  - A new `.rs` file is written with the Write tool, then normalised with `--fix`.
  - After every step that touches `.rs` files, the checker must print `0` for each file touched.
  - Save this checker once, as `$TEMP/dw_crlf.py`, with the Write tool:
    ```python
    # dw_crlf.py -- report lone LF bytes per file; with --fix, normalise to CRLF first.
    import sys
    fix = "--fix" in sys.argv
    for path in [a for a in sys.argv[1:] if a != "--fix"]:
        data = open(path, "rb").read()
        if fix:
            data = data.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
            open(path, "wb").write(data)
        print(path, data.count(b"\n") - data.count(b"\r\n"))
    ```
    Usage: `python "$TEMP/dw_crlf.py" src/app.rs` checks. `python "$TEMP/dw_crlf.py" --fix src/invite_link.rs` normalises a new file.
  - **The block-replacement script.** Many steps replace or delete a whole run of lines (a test, a block of functions). Save this once, as `$TEMP/dw_cut.py`, with the Write tool:
    ```python
    # dw_cut.py FILE START END [REPLACEMENT]
    # Replaces the lines of a CRLF file from the one containing START (inclusive)
    # up to the one containing END (exclusive). START and END must each match
    # exactly one line. With REPLACEMENT, that file's text (any line endings)
    # goes in their place; without it, the lines are deleted.
    import sys
    path, start, end = sys.argv[1], sys.argv[2], sys.argv[3]
    insert = b""
    if len(sys.argv) > 4:
        insert = open(sys.argv[4], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    lines = open(path, "rb").read().split(b"\r\n")
    def only(needle):
        hits = [i for i, line in enumerate(lines) if needle.encode("utf-8") in line]
        if len(hits) != 1:
            sys.exit(f"{needle!r}: {len(hits)} lines match in {path}, expected 1")
        return hits[0]
    a, b = only(start), only(end)
    if b <= a:
        sys.exit("END comes before START")
    body = insert.split(b"\r\n") if insert else []
    if body and body[-1] == b"":
        body = body[:-1]
    lines[a:b] = body
    open(path, "wb").write(b"\r\n".join(lines))
    print(path, "replaced lines", a + 1, "to", b, "with", len(body))
    ```
    - A replacement's text is written with the Write tool to a file under `$TEMP/dw/` (for example `$TEMP/dw/dw1-tests-a.txt`), never into the `.rs` file directly.
    - A replacement that stands where a test or function stood **ends with one empty line**, so the blank line before END survives. Every replacement in this plan is written that way.
    - **The `END` marker line is not replaced.** It is the first line of the item that follows.
- **Source pins are heavy. When one goes red, read its message.** Then either fix the code, or update the pin while keeping its intent. Never delete a pin, and never widen one past what the change needs.
  - **Pins this plan moves on purpose:**
    - **`OPENS_NO_WINDOW`** in `src/foreground.rs` (`foreground::tests::every_module_in_this_crate_is_classified_as_opening_windows_or_not`). This is the module classification. DW3 adds `invite_link` to `lib.rs`: it gets an entry with a comment, and the array length goes 89 → 90.
    - **The README test badge**, `job_object::tests::the_readme_test_count_is_not_a_boast`.
      - The badge says `6000+`, and the guard requires `claimed <= actual < claimed + 200`.
      - At `f2eecc8` there are **6135** `#[test]`s under `src/` (`grep -r --include=*.rs -o "#\[test\]" src | wc -l`).
      - Each task states its own net change. DW4's last step checks the total.
      - If the total reaches 6200, the badge moves to `6100+`: in `README.md`, replace `tests-6000%2B` with `tests-6100%2B`. The guard then holds for 6100–6299.
  - **Pins this plan must keep green without editing them:**
    - the thread census: no new `std::thread::spawn`. A pasted link is sent through the existing `share_ui::spawn_sharing_call`.
    - the frame census;
    - `debug_leak_guard`. `invite_link::InviteLink` holds the token in a `Zeroizing<String>`, so it has a **hand-written** `Debug` that prints no token, and no new type that holds it derives `Debug`.
    - the `job_object` manifest pin. `Cargo.toml` does not change: `url` is already a direct dependency, and `zeroize` already is.
    - the `job_object` file list: no Rust file is added outside `src/`;
    - `OPENS_NO_WINDOW`, except for DW3's one entry above.
- **Verify per task.** The last step of every Deskwarden task runs all four commands, and all four must pass before the commit:
  1. `cargo test --lib -j 2`
  2. `cargo test --bins -j 2`. `--lib` never runs `main.rs`'s tests.
  3. `cargo check --all-targets -j 2`, with **0 warnings**.
  4. `RUSTFLAGS="-D warnings" cargo test --no-run -j 2`.
  - Use a private target directory, because the shared one can serve stale test binaries: `export CARGO_TARGET_DIR="$TEMP/dw-sharing-target"` in the shell you run cargo from.
  - A cold `cargo check` takes about two minutes. A cold test build takes several, and the lib suite runs for about two minutes once it is built.
- **House rules the crate enforces by test.**
  - No `#[cfg(not(test))]` and no `cfg!`.
  - Below a file's first `#[cfg(test)]` there are only `#[cfg(test)] mod … { }` blocks.
  - Comments follow the codebase's dense, explanatory voice. They say *why*, name the alternative that was rejected, and point at the pinning test.

## Researched facts (each read in source on 2026-10-08)

1. **Bitwarden has no count limit on bulk organization-user ids.**
   - `OrganizationUserBulkRequestModel` has `[Required, MinLength(1)] public IEnumerable<Guid> Ids`.
     - Source: `https://raw.githubusercontent.com/bitwarden/server/main/src/Api/AdminConsole/Models/Request/Organizations/OrganizationUserRequestModels.cs`, lines 140–148.
   - `OrganizationUserBulkConfirmRequestModel` has `[Required] Keys`, with no count (lines 82–95).
   - In `src/Api/AdminConsole/Controllers/OrganizationUsersController.cs`, the bulk `reinvite`, `send-invite`, `confirm`, `public-keys`, remove, `delete-account`, `revoke` and `restore` actions pass `model.Ids` straight through, with no count check.
   - None of the commands under `src/Core/AdminConsole/OrganizationFeatures/OrganizationUsers/` counts them either.
   - **The invite is capped.** `OrganizationUserInviteRequestModel.Emails` carries `[StrictEmailAddressList]`, which refuses more than 20 emails ("You can only submit up to 20 emails at a time.") and any email over 256 characters ("Email #N is longer than 256 characters.").
     - Source: `https://raw.githubusercontent.com/bitwarden/server/main/src/Core/Utilities/StrictEmailAddressListAttribute.cs`, lines 15–36.
2. **Bitwarden's invitation link and subject.**
   - **The link.** `HandlebarsMailService.BuildInvitationUrl` builds it.
     - Source: `https://raw.githubusercontent.com/bitwarden/server/main/src/Core/Platform/Mail/HandlebarsMailService.cs`, lines 875–893.
     - Format: `{Vault}/#/accept-organization?organizationId={guid}&organizationUserId={guid}&email={enc}&organizationName={enc}&token={enc}&initOrganization={True|False}&orgUserHasExistingUser={True|False}[&orgSsoIdentifier=…]`.
   - **The path is in the fragment.** `VaultWithHash => $"{Vault}/#"`, at `src/Core/Settings/GlobalSettings.cs` line 169. There is no slash before `?`.
     - Vaultwarden's own link differs. It is `…/#/accept-organization/?…`, with `email` first (`vaultwarden/src/mail.rs`, `send_invite`, lines 282–339).
   - **Encoding.**
     - `email`, `organizationName` and `token` go through `WebUtility.UrlEncode`: space becomes `+`, and the hex is uppercase.
     - The GUIDs are written plain, in lowercase.
     - The booleans are C#'s `True`/`False`.
     - The official web vault's `DirectOrganizationInvite.fromUrlParams` requires all seven keys, checks both ids with `isId`, and compares the booleans case-insensitively.
     - Source: `bitwarden/clients`, `libs/common/src/auth/organization-invite/models/direct-organization-invite.ts`.
   - **Subjects.** `GetUpdatedInviteTemplateInfo` (HandlebarsMailService.cs lines 800–833) chooses the subject by plan and by whether an account exists:
     - Free plan: "You have been invited to a Bitwarden vault" (has an account) or "You have been invited to Bitwarden Password Manager" (no account).
     - Teams, Enterprise and Families: "{OrganizationName} invited you to their Bitwarden vault" or "{OrganizationName} set up a Bitwarden account for you".
   - **Token lifetime.** `OrganizationInviteExpirationHours = 120` (GlobalSettings.cs line 52), used by `OrgUserInviteTokenableFactory`.
     - The token is opaque to every client: a DataProtector blob prefixed `BwOrgUserInviteToken_`.
   - **Accept.** `POST organizations/{orgId}/users/{organizationUserId}/accept`, with body `{token, resetPasswordKey}`. It checks the token, then that the email matches the signed-in account's.
3. **`/config`'s `server`.**
   - Vaultwarden sends `"server": { "name": "Vaultwarden", "url": "https://github.com/dani-garcia/vaultwarden" }`.
     - Source: `https://raw.githubusercontent.com/dani-garcia/vaultwarden/main/src/api/core/mod.rs`, `fn config()`, lines 210–263.
   - Bitwarden declares `ServerConfigResponseModel Server { Name, Url }` and never sets it, so it is `null` there.
     - Source: `https://raw.githubusercontent.com/bitwarden/server/main/src/Api/Models/Response/ConfigResponseModel.cs`, lines 20 and 59–63.
4. **Vaultwarden with mail disabled.** Source: `https://raw.githubusercontent.com/dani-garcia/vaultwarden/main/src/api/core/organizations.rs`.
   - **`send_invite`** (lines 991–1098).
     - An address with an account that has a password is `Accepted` at once ("automatically accept existing users if mail is disabled", lines 1054–1061).
     - An account with no password, or no account at all, stays `Invited`. A password-less user stub and an `Invitation` row are made for it (lines 1031–1047).
   - **`reinvite_member_impl`** (lines 1191–1232), with mail off:
     - It takes the `Invitation`.
     - It sets an existing account with a password to `Accepted`.
     - It does not fail.
   - **Registration.** `accounts.rs` `register`, lines 351–377, runs `Invitation::take` and then `Membership::accept_user_invitations` (`src/db/models/organization.rs` lines 845–855). That turns every `Invited` membership of the new account into `Accepted`.
     - The `Invitation` row exists only when mail is off, so this is the mail-off path.
     - Mail-off invitations carry no expiry.
5. **Clients the release check uses.**
   - `bw confirm org-member` calls `GET /organizations/{orgId}/users/{id}`, then `GET /users/{userId}/public-key`, then `POST …/users/{id}/confirm`.
     - Sources: `bitwarden/clients`, `apps/cli/src/admin-console/commands/confirm.command.ts` lines 69–85, and `libs/admin-console/src/common/organization-user/services/default-organization-user-api.service.ts` (`getOrganizationUser`).
   - The web vault edits a member with `PUT /organizations/{orgId}/users/{id}`, which answers with an empty body (`putOrganizationUser`, same file).
6. **NodeWarden today** (evidence for the gaps below).
   - **It sends no email at all.** Every mail-backed route answers `unsupportedResponse('Email delivery is not supported by this server.')`:
     - `src/router-public.ts` lines 286–307, the `publicMailBackedPaths`;
     - `src/router-authenticated.ts` lines 181–208.
   - **No route answers `GET` or `PUT /organizations/{org}/users/{ouId}`.** `src/router-authenticated.ts` lines 553–565 answer DELETE, reinvite, accept, confirm and registration-invite, and 405 to anything else.
   - **`DELETE /api/ciphers/{id}` soft-deletes a live cipher.** `handleDeleteCipherCompat`, `src/handlers/ciphers.ts` lines 1345–1375, puts it in the trash. Bitwarden's `[HttpDelete("{id}")] Delete` (`src/Api/Vault/Controllers/CiphersController.cs` line 1096) deletes permanently.
   - Organization invitations last 7 days (`ORG_INVITATION_TTL_MS`, `src/services/org-user-model.ts` line 13).
   - Organizations carry no `planType` anywhere in `src/`, so an official client reads them as Free (0).

## Decisions this plan takes (where the spec is silent, contradicts itself, or meets the code)

Each one is implemented as stated, and each is repeated in the final report.

1. **No count cap on bulk ids; a body bound instead (fact 1).**
   - The base spec names no "request-size bound", so this plan sets one: `LIMITS.organizations.maxBulkBodyBytes = 64 KiB` for `POST …/users/confirm` and `…/users/public-keys`.
   - Both routes now read every member and user they need in a fixed number of queries, whatever the count. Confirm writes in one D1 batch.
   - So 64 KiB (about 1,600 ids, or 150 confirm keys) stays inside the Workers Free plan's 50 queries. That is the reason `30f64d5` lowered the count to 15.
   - `POST …/users/invite` takes Bitwarden's own caps: 20 emails, 256 characters each.
2. **`INVITE_MAIL` needs a new mail sender (fact 6).** The brief expected NodeWarden to already send registration email. It does not, so there is no sender to reuse.
   - NW4 adds the first one: Cloudflare Email Service's Workers binding `[[send_email]] name = "EMAIL"`, with `INVITE_MAIL = "1"` and `INVITE_MAIL_FROM` set as vars.
   - The binding and vars ship commented out in `wrangler.toml`. `INVITE_MAIL` is off unless set, which is the spec's default.
3. **The invitation email's subject is Bitwarden's Free-plan pair (fact 2):** "You have been invited to a Bitwarden vault" for an existing account, "You have been invited to Bitwarden Password Manager" for a new one.
   - NodeWarden organizations report no plan, and official clients read that as Free.
   - The From name is "NodeWarden". The address is `INVITE_MAIL_FROM`.
4. **Token lifetime with `INVITE_MAIL` on is Bitwarden's 120 hours. With it off it stays NodeWarden's 7 days.**
   - The registration that accepts a mail-off invitation ignores its expiry, as Vaultwarden's expiry-less `Invitation` does (fact 4).
5. **NodeWarden gains `GET` and `PUT /organizations/{org}/users/{ouId}` (fact 5).** Without `GET`, the release check's `bw confirm org-member` fails. `PUT` is the spec's "role change through `PUT …/users/{ouId}`", which today has no route at all.
6. **NodeWarden's `DELETE /api/ciphers/{id}` becomes Bitwarden's permanent delete (fact 6).**
   - The spec's §5 table does not list it. But base spec §7.3 moves a record back with `DELETE /api/ciphers/{orgId}` on every server, and NodeWarden's soft delete would leave the org copy in the sharer's trash.
   - Today Deskwarden avoids that with NodeWarden's own `DELETE /api/ciphers/{id}/delete`, which official and Vaultwarden do not have. On them, the 404 it gets back is read as "already gone", and the org copy stays.
   - `bw delete item <id> --permanent` on a live item also only trashed it on NodeWarden.
   - **Owner to confirm.**
7. **Deskwarden's general trash purge still uses `DELETE /api/ciphers/{id}/delete`** (`RestClient::hard_delete_cipher`). It is not on the sharing path, so it is out of scope. It is flagged in the report.
8. **The invite code (`…/users/{ouId}/registration-invite`) stays.** It is NodeWarden-only, but it is account administration, not the share path.
   - It is offered only to an account whose profile says `role: "admin"`. Only NodeWarden sends that, so on other servers it is never offered and never called.
   - Its band now opens "Waiting for X to accept", not "to create an account".
   - **Owner to confirm.**
9. **"Mine" without `ownerEmail` (spec §3.1).** An org is this account's own sharing org when:
   - it is owned by the account and named for it; and
   - either this client created it (the cached `sharing_org_created` id), or the roster lists **no other account in the owner role**.
   - A roster that cannot be read refuses a write ("Deskwarden couldn't read who you share with"). It does not adopt the org, and it does not create a second one.
   - The sharer's own org-user id, which the person-collection create names, is read off the same roster: the row whose `userId` is the account's. Today's code sends the *user* id, which is right on NodeWarden only.
10. **`server-too-old` is learned, not probed.**
    - Sharing is off for the session when a route it needs answers 405 or 501, or the sharer's own org roster (`…/users?includeCollections=true`) answers 404.
    - A 404 anywhere else is a missing record, not a missing route.
11. **Accepted at invite goes straight to confirm.** When the roster read after an invite shows the person Accepted (spec §2's "Accepts on invite"), the share runs the confirm pass at once. So an existing NodeWarden or Vaultwarden account is `Shared` when the dialog closes.
12. **States.**
    - `PersonState::WaitingAccount` becomes `Invited` (official status 0), and `Expired` is removed, because no expiry is claimed. `ShareResult::WaitingAccount` becomes `Invited`.
    - The card says "Waiting to accept · invited N days ago" for both Invited and Accepted. N comes from the local sent history, as today's "since" does. A re-invite does not move it.
    - The ten-minute timer still skips an invitation whose `invitationExpirationDate` (a NodeWarden extension) has passed.
13. **The roster-visibility warning (spec §3.4)** reads `GET /api/config` once per session. The warning shows unless `server.name` is `Vaultwarden` or `NodeWarden`, case-insensitively.
14. **Paste an invite link.**
    - It accepts Bitwarden's format, Vaultwarden's (`/#/accept-organization/?`), and a query outside the fragment.
    - Keys are case-insensitive. Values are form-decoded. Both ids must be GUIDs, as `isId` requires.
    - The token travels as a `Zeroizing<String>` from the box to the daemon, which sends it once.
    - A link for another email is refused with "This invite link is for {email}. You're signed in as {account}."
15. **The NodeWarden web vault does not route `#/accept-organization`.** That is out of scope.
    - With `INVITE_MAIL` on, a NodeWarden recipient accepts through Deskwarden's Paste an invite link, or through NodeWarden's own Organizations page, which still lists `invitations/mine`.
    - Flagged for a follow-up.
16. **The rail's dot on Shared with me** now means "a sharer is being waited on" (`waiting`), since there are no invitation rows.
17. **Unverified "Shared by" (spec §4.3).**
    - A received sharing org whose roster cannot be read is drawn `Shared by X · unverified · view-only`. X is the org name's claim, with the spec's tooltip. Its records are grouped by organisation, never merged with another org that claims the same sharer.
    - An org named for *this* account that it does not own is not a sharing org at all.
    - With the roster readable, the sharer is the one confirmed owner. With two owners, or none, the record stays unverified.
18. **The web-vault half of the release check** serves `bw_web_builds` v2026.7.0 (released 2026-08-05) through `wrangler dev --assets`. That build is the official `bitwarden/clients` web vault with Vaultwarden's small patches. It is the build self-hosters run.
19. **Release check CLI:** `@bitwarden/cli@2026.9.0` (published 2026-09-17), run with `pnpm dlx`.
20. **"The member with `type: 0`" (spec §4.3) is read as "the one confirmed owner".** An organisation may have two owners. Picking either one would name a sharer the server never singled out, so with two owners, or none, the record stays unverified. Received rosters are asked once per whole read, and only until a server answers a member 404 or 403. That answer ends the asking for the session, because no organisation on that server will answer otherwise (DW4).
21. **"Cloudflare Email Service's test mode" (spec §7) is `wrangler dev`'s local simulation of the `send_email` binding.** It logs the message's From, To and Subject, and saves its body to a local file without sending. Source: `https://developers.cloudflare.com/email-service/local-development/sending/`. So L12 needs no onboarded domain. A real send, with `remote = true` on the binding, is an optional extra for an owner with one.

## File Structure

| File | New / changed | Responsibility |
|---|---|---|
| **NodeWarden** | | |
| `src/config-response.ts` | changed (NW1) | `server: {name, url}` |
| `src/config/limits.ts` | changed (NW1) | `maxBulkBodyBytes`, `maxInviteEmails`, `maxInviteEmailLength` replace `maxBulkUserIds` |
| `src/services/storage-organization-repo.ts` | changed (NW1) | `saveOrganizationMembers` (one D1 batch) |
| `src/handlers/organization-users.ts` | changed (NW1–NW4) | bulk rules; `GET`/`PUT …/users/{ouId}`; accept on invite and on registration; the invitation email |
| `src/handlers/organizations.ts` | changed (NW2) | 47869c4's raise refusal removed; `updateOrganizationMember` shared |
| `src/handlers/ciphers.ts` | changed (NW2) | `DELETE /ciphers/{id}` is permanent |
| `src/router-authenticated.ts` | changed (NW2) | the two new routes |
| `src/services/org-user-model.ts` | changed (NW4) | `invitationExpiry` takes a lifetime |
| `src/services/invite-mail.ts` | **new** (NW3, NW4) | `inviteMailEnabled`, lifetime, Bitwarden's link, subject, compose, send |
| `src/handlers/accounts.ts` | changed (NW3) | registration accepts mail-off invitations |
| `src/types/index.ts` | changed (NW3, NW4) | `Env.INVITE_MAIL`, `INVITE_MAIL_FROM`, `EMAIL`; `InviteMailSender` |
| `wrangler.toml` | changed (NW4) | commented `send_email` binding and vars |
| `scripts/support/sharing-harness.ts` | changed (NW3, NW4) | `inviteMailOn(h)` |
| `scripts/config-compatibility.test.ts`, `scripts/sharing-membership.test.ts`, `scripts/sharing-org-users.test.ts`, `scripts/sharing-confirmed-manager.test.ts`, `scripts/sharing-profile.test.ts`, `scripts/sharing-sso-takeover.test.ts` | changed | as each task says |
| `scripts/sharing-invite-accept.test.ts` | **new** (NW3) | mail-off accept |
| `scripts/sharing-invite-mail.test.ts` | **new** (NW4) | the email, faked |
| **Deskwarden** (`deskwarden/`) | | |
| `src/rest/sharing.rs` | changed (DW1–DW4) | capabilities; official invite; mine by roster; confirm at invite; states; paste accept; unverified sharers |
| `src/rest/sharing_api.rs` | changed (DW1–DW3) | `invite_user`, `delete_cipher`, `server_name`, `accept_invite`; NodeWarden routes removed |
| `src/rest/api.rs` | changed (DW1, DW4) | roster on `/users` only; `fetch_organization_mini_details` |
| `src/rest/organizations.rs` | changed (DW4) | `SharingOrg.verified`, `sharing_mark` without `ownerEmail`, `Directory::attest_received` |
| `src/rest/sync.rs` | changed (DW4) | `Organization.owner_email` removed |
| `src/rest/backend.rs` | changed (DW4) | asks the received rosters after `Directory::load`; fixture without `ownerEmail` |
| `src/sharing_state.rs` | changed (DW4) | `sharing_org_created`'s doc says what the roster rule does |
| `src/invite_link.rs` | **new** (DW3) | the pure link parser |
| `src/lib.rs`, `src/foreground.rs` | changed (DW3) | `pub mod invite_link;` and its classification |
| `src/vault_api.rs` | changed (DW1, DW3) | `POST /sharing/incoming/paste`; accept/decline routes removed |
| `src/daemon_link.rs` | changed (DW3) | `SharingClient::accept_link`; `accept`/`decline` removed |
| `src/vault_window/share_ui.rs` | changed (DW2–DW4) | states, wording, warning; the paste row; unverified rows |
| `src/vault_window/send_ui.rs` | changed (DW3) | Shared with me lists the paste row and no invitations |
| `src/vault_window/mod.rs` | changed (DW2, DW3) | `share_rows` call; paste wiring; invitation wiring removed |
| `src/vault_window/detail.rs` | changed (DW4) | `· unverified` and its tooltip |
| `src/vault_window/detail_edit.rs`, `src/vault_window/item_list.rs` | changed (DW4) | `Audience::SharedWithMe.verified`; `ReceivedEntry` fields |
| `README.md` (repo root) | changed only if DW4's count passes 6199 | the test badge |
| `docs/superpowers/plans/2026-10-08-official-path-sharing.md` | changed (DW5) | this plan's compatibility ledger, filled in |

---

## NodeWarden tasks

### Task NW1: `/config` names the server, and bulk calls take Bitwarden's limits

**Files:**
- Modify: `E:\Personal\nodewarden\src\config-response.ts:16` (`server: null,`)
- Modify: `E:\Personal\nodewarden\src\config\limits.ts:171-176` (the `maxBulkUserIds` block)
- Modify: `E:\Personal\nodewarden\src\services\storage-organization-repo.ts:353-355` (after `saveOrganizationMember`)
- Modify: `E:\Personal\nodewarden\src\handlers\organization-users.ts:1-40` (imports), `:176-190` (invite's email checks), `:329-376` (bulk confirm, public keys)
- Test: `E:\Personal\nodewarden\scripts\config-compatibility.test.ts`, `E:\Personal\nodewarden\scripts\sharing-membership.test.ts:178-187` and `:221-235`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces:
  - `buildConfigResponse(origin).server: { name: 'NodeWarden'; url: 'https://github.com/shuaiplus/NodeWarden' }`. DW2 reads `server.name`.
  - `LIMITS.organizations.maxBulkBodyBytes: number` (65536), `maxInviteEmails: 20` and `maxInviteEmailLength: 256`. `maxBulkUserIds` is removed.
  - `saveOrganizationMembers(db: D1Database, members: OrganizationMember[]): Promise<void>` in `storage-organization-repo.ts`.

- [ ] **Step 1: Write the failing tests**

Append to `scripts/config-compatibility.test.ts`:

```ts

// Vaultwarden names itself here ({name: "Vaultwarden", url}); official
// Bitwarden leaves it null. Clients read it to tell servers apart: Deskwarden
// decides from it whether recipients can see each other on the roster.
test('config names the server as Vaultwarden does', () => {
  const body = buildConfigResponse('https://vault.example.test');

  assert.deepEqual(body.server, { name: 'NodeWarden', url: 'https://github.com/shuaiplus/NodeWarden' });
});
```

In `scripts/sharing-membership.test.ts`, replace the whole test that begins:

```ts
test('a bulk confirm and a public-keys request both refuse an oversized array', async () => {
```

(through its closing `});`) with:

```ts
test('bulk calls take more ids than the old cap of 15, and an oversized body is refused whole', async () => {
  const h = fixture();
  // Sixteen ids nobody holds, then bob: Bitwarden counts none of them.
  const strangers = Array.from({ length: 16 }, (_, i) => `aaaaaaaa-0000-4000-8000-${String(100 + i).padStart(12, '0')}`);
  const publicKeys = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/public-keys`, { ids: [...strangers, U.alice] });
  assert.equal(publicKeys.status, 200);
  assert.deepEqual(publicKeys.body.data.map((row: any) => row.id), [U.alice]);

  const keys = [...strangers.map((id) => ({ id, key: '4.aw==' })), { id: U.bob, key: '4.Ym9i' }];
  const confirmed = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/confirm`, { keys });
  assert.equal(confirmed.status, 200);
  assert.equal(confirmed.body.data.length, 17);
  assert.equal(confirmed.body.data[0].error, 'Not found');
  assert.equal(confirmed.body.data[16].error, '');
  const bob = h.db.prepare('SELECT encrypted_org_key FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.bob) as any;
  assert.equal(bob.encrypted_org_key, '4.Ym9i');

  // One byte past the bound: refused before anything is read or written.
  const padding = 'x'.repeat(LIMITS.organizations.maxBulkBodyBytes);
  const huge = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/public-keys`, { ids: [padding] });
  assert.equal(huge.status, 413);
  const hugeConfirm = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/confirm`, { keys: [{ id: U.carol, key: `4.${padding}` }] });
  assert.equal(hugeConfirm.status, 413);
});
```

Replace the whole test that begins:

```ts
test('the bulk limit fits a Workers Free request: 50 D1 queries at about 3 per id', async () => {
```

(through its closing `});`) with:

```ts
// The Workers Free plan allows 50 D1 queries per request (the reason 30f64d5
// capped the count at 15). With no count cap, the routes must read in a fixed
// number of queries whatever the count -- measured here at the body bound.
test('a bulk call at the body bound stays inside 50 D1 queries', async () => {
  const h = fixture();
  const seen: string[] = [];
  const db = h.env.DB as any;
  const prepare = db.prepare.bind(db);
  db.prepare = (sql: string) => {
    seen.push(sql);
    return prepare(sql);
  };
  const id = (i: number) => `aaaaaaaa-0000-4000-8000-${String(i).padStart(12, '0')}`;
  // `"<36 characters>",` is 39 bytes: this many ids fill most of the bound.
  const many = Array.from({ length: Math.floor(LIMITS.organizations.maxBulkBodyBytes / 39) - 10 }, (_, i) => id(1000 + i));
  const publicKeys = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/public-keys`, { ids: [...many, U.alice] });
  assert.equal(publicKeys.status, 200);
  assert.ok(many.length > 1500, String(many.length));
  assert.ok(seen.length < 50, `${seen.length} queries for ${many.length} ids`);

  seen.length = 0;
  const keys = Array.from({ length: 150 }, (_, i) => ({ id: id(5000 + i), key: '4.aw==' }));
  const confirmed = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/confirm`, { keys: [...keys, { id: U.bob, key: '4.Ym9i' }] });
  assert.equal(confirmed.status, 200);
  assert.ok(seen.length < 50, `${seen.length} queries for 151 keys`);
});

test('an invite takes at most 20 emails of at most 256 characters, as Bitwarden does', async () => {
  const h = fixture();
  const emails = Array.from({ length: 21 }, (_, i) => `person${i}@example.test`);
  const many = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails, type: 2 });
  assert.equal(many.status, 400);
  assert.equal(many.body.error, 'You can only submit up to 20 emails at a time.');
  const long = `${'a'.repeat(250)}@example.test`;
  const tooLong = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: [emailOf('dave'), long], type: 2 });
  assert.equal(tooLong.status, 400);
  assert.equal(tooLong.body.error, 'Email #2 is longer than 256 characters.');
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE email = ?', emailOf('dave')), 0);
});
```

Check the line endings of the two test files: `git diff --stat` and `tr -cd '\r' < scripts/sharing-membership.test.ts | wc -c` (expect `0`), and the same for `scripts/config-compatibility.test.ts`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:config-compatibility`
Expected: FAIL. `config names the server as Vaultwarden does` reports `actual: null`.

Run: `pnpm run test:sharing 2>&1 | grep -E "^not ok|^# (pass|fail)"`
Expected: three failures:
- `bulk calls take more ids…` gets a 400 for 17 ids;
- `a bulk call at the body bound…` gets a 400;
- `an invite takes at most 20 emails…` gets a 200.

`tsc` would also flag `maxBulkBodyBytes`, but `tsx` does not type-check.

- [ ] **Step 3: Name the server in `/config`**

In `src/config-response.ts`, replace:

```ts
    server: null,
```

with:

```ts
    // Bitwarden's ServerConfigResponseModel {name, url}. The official server
    // leaves it null; Vaultwarden fills it with its own name, and clients read
    // it to tell servers apart -- Deskwarden decides from it whether people a
    // record is shared with can see each other on the roster (they cannot
    // here: mini-details is managers only).
    server: { name: 'NodeWarden', url: 'https://github.com/shuaiplus/NodeWarden' },
```

- [ ] **Step 4: Replace the count cap with Bitwarden's rules**

In `src/config/limits.ts`, replace:

```ts
    // Max ids accepted in one call to the bulk-confirm and public-keys routes.
    // Each id costs about 3 D1 queries, and the Workers Free plan allows 50
    // queries per request, so 15 ids (~45 queries) is the most one call fits.
    // 批量确认与批量公钥查询接口单次请求允许的最大 id 数量。每个 id 约需 3 次 D1 查询，
    // Workers 免费版每个请求最多 50 次查询，因此上限为 15。
    maxBulkUserIds: 15,
```

with:

```ts
    // Bitwarden puts no count on a bulk organization-user call's ids
    // (OrganizationUserBulkRequestModel: [Required, MinLength(1)] Ids, and no
    // count in OrganizationUsersController), so neither does NodeWarden. The
    // bound is the body instead: the bulk-confirm and public-keys routes read
    // what they need in a fixed number of D1 queries and write in one batch,
    // so 64 KiB (about 1,600 ids, or 150 confirm keys) stays inside the
    // Workers Free plan's 50 queries per request.
    // 批量确认与批量公钥接口不限制 id 数量（与 Bitwarden 一致），改为限制请求体大小（64 KiB），
    // 并以固定次数的查询完成，从而符合 Workers 免费版每请求 50 次查询的限制。
    maxBulkBodyBytes: 64 * 1024,
    // Bitwarden's StrictEmailAddressListAttribute: at most 20 emails per
    // invite, each at most 256 characters.
    // 单次邀请最多 20 个邮箱，每个邮箱最长 256 个字符（与 Bitwarden 一致）。
    maxInviteEmails: 20,
    maxInviteEmailLength: 256,
```

Check: `head -c 3 src/config/limits.ts | od -An -tx1` still prints ` ef bb bf`.

- [ ] **Step 5: Add the batched member write**

In `src/services/storage-organization-repo.ts`, replace:

```ts
export async function saveOrganizationMember(db: D1Database, member: OrganizationMember): Promise<void> {
  await organizationMemberUpsertStatement(db, member).run();
}
```

with:

```ts
export async function saveOrganizationMember(db: D1Database, member: OrganizationMember): Promise<void> {
  await organizationMemberUpsertStatement(db, member).run();
}

// Several members in one D1 batch: one round trip whatever the count, and all
// or nothing, since D1 runs a batch as one transaction. The bulk confirm's
// write, which must not cost a query per id.
export async function saveOrganizationMembers(db: D1Database, members: OrganizationMember[]): Promise<void> {
  if (!members.length) return;
  await db.batch(members.map((member) => organizationMemberUpsertStatement(db, member)));
}
```

- [ ] **Step 6: Bound the bulk routes by their body, and read in fixed queries**

In `src/handlers/organization-users.ts`, replace the first line:

```ts
import type { CollectionAccess, Env, Invite, OrganizationInvitation, User } from '../types';
```

with:

```ts
import type { CollectionAccess, Env, Invite, OrganizationInvitation, OrganizationMember, User } from '../types';
```

Replace:

```ts
  getOrganizationMembers,
  saveOrganizationInvitation,
  saveOrganizationMember,
} from '../services/storage-organization-repo';
```

with:

```ts
  getOrganizationMembers,
  saveOrganizationInvitation,
  saveOrganizationMember,
  saveOrganizationMembers,
} from '../services/storage-organization-repo';
```

Replace:

```ts
async function readJson(request: Request): Promise<any | null> {
  try {
    return await request.json();
  } catch {
    return null;
  }
}
```

with:

```ts
async function readJson(request: Request): Promise<any | null> {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

// A bulk route's body, bounded by LIMITS.organizations.maxBulkBodyBytes rather
// than by a count of ids (Bitwarden has no count): the bytes are measured
// before the JSON is parsed. `body` is null for JSON that does not parse.
async function readBulkJson(request: Request): Promise<{ body: any | null } | { refused: Response }> {
  const text = await request.text();
  if (new TextEncoder().encode(text).byteLength > LIMITS.organizations.maxBulkBodyBytes) {
    return { refused: errorResponse('Request body too large', 413) };
  }
  try {
    return { body: JSON.parse(text) };
  } catch {
    return { body: null };
  }
}
```

In `handleInviteOrganizationUsers`, replace:

```ts
  if (!emails.length) return errorResponse('At least one email is required', 400);
  if (emails.some((email) => !EMAIL_PATTERN.test(email))) return errorResponse('Valid email is required', 400);
```

with:

```ts
  if (!emails.length) return errorResponse('At least one email is required', 400);
  // Bitwarden's StrictEmailAddressListAttribute, in its words and order.
  if (body.emails.length > LIMITS.organizations.maxInviteEmails) {
    return errorResponse(`You can only submit up to ${LIMITS.organizations.maxInviteEmails} emails at a time.`, 400);
  }
  const tooLong = emails.findIndex((email) => email.length > LIMITS.organizations.maxInviteEmailLength);
  if (tooLong >= 0) {
    return errorResponse(`Email #${tooLong + 1} is longer than ${LIMITS.organizations.maxInviteEmailLength} characters.`, 400);
  }
  if (emails.some((email) => !EMAIL_PATTERN.test(email))) return errorResponse('Valid email is required', 400);
```

Replace the whole of `handleBulkConfirmOrganizationUsers` and `handleOrganizationUserPublicKeys`, from:

```ts
// POST /api/organizations/:orgId/users/confirm   { keys: [{ id, key }] }
```

through the closing `}` of `handleOrganizationUserPublicKeys` (the line before `// POST /api/organizations/:orgId/leave`), with:

```ts
// POST /api/organizations/:orgId/users/confirm   { keys: [{ id, key }] }
// Any number of keys, as on official servers; the body is what is bounded.
// Every member and invitation is read once and every confirm written in one
// batch, so the cost does not grow with the count. Each entry answers as the
// single confirm would: a bad key, Not found, User not accepted (an
// invitation), Already confirmed -- or '' for done.
export async function handleBulkConfirmOrganizationUsers(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  if (!(await requireConfirmedManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const read = await readBulkJson(request);
  if ('refused' in read) return read.refused;
  const body = read.body;
  if (!body || !Array.isArray(body.keys)) return errorResponse('keys array is required', 400);

  const [members, invitations] = await Promise.all([
    getOrganizationMembers(env.DB, orgId),
    getOrganizationInvitations(env.DB, orgId),
  ]);
  const accepted = new Map(members.filter((member) => member.status === 1).map((member) => [member.userId, member]));
  const invited = new Set(invitations.filter((invitation) => invitation.status === 0).map((invitation) => invitation.id));
  const now = new Date().toISOString();
  const data: Record<string, unknown>[] = [];
  const changed = new Map<string, OrganizationMember>();
  for (const entry of body.keys) {
    const id = String(entry?.id || '').trim();
    const key = String(entry?.key || '').trim();
    const member = accepted.get(id);
    let error = '';
    if (!ENC_STRING_PATTERN.test(key)) error = 'Key must be an encrypted string';
    else if (!member) error = invited.has(id) ? 'User not accepted' : 'Not found';
    else if (member.encryptedOrgKey) error = 'Already confirmed';
    else {
      member.encryptedOrgKey = key;
      member.updatedAt = now;
      changed.set(member.userId, member);
    }
    data.push({ object: 'OrganizationBulkConfirmResponseModel', id, error });
  }
  if (changed.size) {
    await saveOrganizationMembers(env.DB, [...changed.values()]);
    await bumpOrgMemberAndManagers(env, orgId, [...changed.keys()], readActingDeviceIdentifier(request));
    await writeOrgAudit(env, userId, 'organization.member.confirm', 'organization', orgId, {
      count: changed.size,
      organizationId: orgId,
    });
  }
  return jsonResponse({ object: 'list', data, continuationToken: null });
}

// POST /api/organizations/:orgId/users/public-keys   { ids: [ouId] }
// The field is `key`, as on official servers. Invitations have no account and
// are skipped. Any number of ids; the members are read once and the users in
// getUsersByIds' chunks, so the cost does not grow with ids nobody holds.
export async function handleOrganizationUserPublicKeys(request: Request, env: Env, userId: string, orgId: string): Promise<Response> {
  if (!(await requireConfirmedManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const read = await readBulkJson(request);
  if ('refused' in read) return read.refused;
  const body = read.body;
  if (!body || !Array.isArray(body.ids)) return errorResponse('ids array is required', 400);

  const ids: string[] = body.ids.map((raw: unknown) => String(raw || '').trim());
  const accepted = new Set(
    (await getOrganizationMembers(env.DB, orgId)).filter((member) => member.status === 1).map((member) => member.userId)
  );
  const users = await getUsersByIds(env.DB, ids.filter((id) => accepted.has(id)));
  const data: Record<string, unknown>[] = [];
  for (const id of ids) {
    const user = accepted.has(id) ? users.get(id) : undefined;
    if (!user) continue;
    data.push({ object: 'organizationUserPublicKeyResponseModel', id, userId: user.id, key: user.publicKey ?? null });
  }
  return jsonResponse({ object: 'list', data, continuationToken: null });
}
```

`confirmOrgUser` is still used by the single confirm. `getUserById` is still used by `handleGetUserPublicKey`. `getUsersByIds` and `getOrganizationInvitations` are already imported.

Run: `grep -rn "maxBulkUserIds" src scripts`
Expected: no output.

Run the LF checks on all four edited `src` files.

- [ ] **Step 7: Run the tests to see them pass**

Run: `pnpm run test:config-compatibility`
Expected: PASS, 2 tests.

Run: `pnpm run test:sharing 2>&1 | grep -E "^# (pass|fail)"`
Expected: `# fail 0`.

- [ ] **Step 8: Verify the whole NodeWarden suite**

Run each of the eight commands in Global Constraints › NodeWarden › Verify per task.
Expected: every `test:*` script ends `# fail 0`, and `pnpm exec tsc --noEmit` prints nothing.

- [ ] **Step 9: Commit**

```bash
git add src/config-response.ts src/config/limits.ts src/services/storage-organization-repo.ts src/handlers/organization-users.ts scripts/config-compatibility.test.ts scripts/sharing-membership.test.ts
git commit -F - <<'EOF'
/config names the server, and bulk member calls take Bitwarden's limits

/config's server was null. It is now {name: "NodeWarden", url}, the
shape Vaultwarden fills, so a client can tell this server apart.

The bulk confirm and public-keys routes refused more than 15 ids.
Bitwarden counts none (OrganizationUserBulkRequestModel has only
MinLength(1)), so an official client confirming 20 people failed here.
The routes now read every member and invitation once and confirm in one
D1 batch, so their cost does not grow with the count. They are bounded
by a 64 KiB body instead, which stays inside the Workers Free plan's 50
queries per request -- the reason the cap was 15.

The official invite takes Bitwarden's own caps: 20 emails, 256
characters each, refused in Bitwarden's words.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

`git commit -F -` reads the message from the heredoc. It writes no source file.

Test count: NodeWarden tests are not counted by any badge.

---

### Task NW2: The official member routes, a role change without consent, and a permanent delete

**Files:**
- Modify: `E:\Personal\nodewarden\src\handlers\organizations.ts:528-540` (`roleNeedsAcceptance`, `RAISE_NEEDS_ACCEPTANCE`), `:649-726` (`handleUpdateOrganizationMember`)
- Modify: `E:\Personal\nodewarden\src\handlers\organization-users.ts` (two new handlers, after `handleRemoveOrganizationUser`; imports)
- Modify: `E:\Personal\nodewarden\src\router-authenticated.ts:553-565` (the `orgUserMatch` block), and its import of `./handlers/organization-users`
- Modify: `E:\Personal\nodewarden\src\handlers\ciphers.ts:1345-1375` (`handleDeleteCipherCompat`)
- Test: `E:\Personal\nodewarden\scripts\sharing-confirmed-manager.test.ts:102-116`, and a new `E:\Personal\nodewarden\scripts\sharing-org-user-details.test.ts`

**Interfaces:**
- Consumes: nothing from NW1.
- Produces:
  - `GET /api/organizations/{orgId}/users/{ouId}`: managers only, otherwise 404. Answers `{object: 'organizationUserDetails', id, userId, email, name, type, status, collections: [{id, readOnly, hidePasswords, manage}], groups: [], permissions: {}, …}`.
  - `PUT /api/organizations/{orgId}/users/{ouId}` with `{type?, collections?, permissions?}`: confirmed managers only. Answers an empty 200.
  - `DELETE /api/ciphers/{id}`: always permanent, answering 204.
  - `export async function updateOrganizationMember(request: Request, env: Env, actor: OrganizationMember, orgId: string, memberUserId: string, body: { role?: unknown; collections?: unknown; permissions?: unknown; encryptedOrgKey?: unknown }): Promise<Response>` in `organizations.ts`.

- [ ] **Step 1: Write the failing tests**

In `scripts/sharing-confirmed-manager.test.ts`, replace the whole test that begins:

```ts
test('member update cannot raise anyone to admin or owner; lowering and manager still work', async () => {
```

(through its closing `});`) with:

```ts
// 47869c4 refused a raise to admin or owner because a direct add could make
// someone an owner without consent, and a client treated an org it owned and
// that was named for it as its own sharing org. Deskwarden no longer uses
// direct add and checks the roster for another owner, so the official
// behaviour is back: a confirmed manager changes a member's role. Direct add
// itself stays member-only (the test above).
test('member update raises and lowers roles as the official server does', async () => {
  for (const role of ['owner', 'admin', 'manager']) {
    const h = fixture();
    const res = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/members/${U.carol}`, { role });
    assert.equal(res.status, 200, role);
    assert.equal(memberRow(h, U.carol).role, role, role);
  }
  const h = fixture();
  assert.equal((await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/members/${U.erin}`, { role: 'member' })).status, 200);
  assert.equal(memberRow(h, U.erin).role, 'member');
});
```

Create `scripts/sharing-org-user-details.test.ts` with the Write tool:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  CIPHER, COL_A, COL_B, INV_CAROL, ORG, U, call, count, createHarness, emailOf, invitationCollectionsRaw,
  memberCollectionsRaw, readOnlyAccess, seedCipher, seedCollection, seedInvitation, seedMember, seedOrg, seedUser,
} from './support/sharing-harness';

// The two official routes on one org user that official clients call and
// NodeWarden did not answer:
//   GET /organizations/{orgId}/users/{id}  -- `bw confirm org-member` reads the
//       member's userId here before it fetches their key (clients,
//       apps/cli/src/admin-console/commands/confirm.command.ts);
//   PUT /organizations/{orgId}/users/{id}  -- the web vault's member edit
//       (putOrganizationUser), the official role change.

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedUser(h, 'bob');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  seedCollection(h, ORG, COL_B);
  seedMember(h, ORG, U.alice, { collections: [readOnlyAccess(COL_A)] }); // confirmed member
  seedMember(h, ORG, U.bob, { key: null }); // accepted, not confirmed
  seedInvitation(h, ORG, INV_CAROL, emailOf('carol'), { invitedBy: U.owner, collections: [readOnlyAccess(COL_A)] });
  return h;
}

test('GET one org user answers its details for a manager, a member or an invitation', async () => {
  const h = fixture();
  const bob = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users/${U.bob}`);
  assert.equal(bob.status, 200);
  assert.equal(bob.body.object, 'organizationUserDetails');
  assert.equal(bob.body.id, U.bob);
  assert.equal(bob.body.userId, U.bob);
  assert.equal(bob.body.status, 1);
  assert.equal(bob.body.type, 2);

  const carol = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users/${INV_CAROL}?`);
  assert.equal(carol.status, 200);
  assert.equal(carol.body.userId, null);
  assert.equal(carol.body.status, 0);
  assert.deepEqual(carol.body.collections, [readOnlyAccess(COL_A)]);

  assert.equal((await call(h, 'alice', 'GET', `/api/organizations/${ORG}/users/${U.bob}`)).status, 404);
  assert.equal((await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users/${CIPHER}`)).status, 404);
});

test('PUT one org user changes a member\'s type and collections, and answers empty', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/users/${U.alice}`, {
    type: 1,
    collections: [readOnlyAccess(COL_B)],
    groups: [],
    permissions: null,
    accessSecretsManager: false,
  });
  assert.equal(res.status, 200);
  assert.equal(res.body, null);
  const row = h.db.prepare('SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.alice) as any;
  assert.equal(row.role, 'admin');
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.alice), [readOnlyAccess(COL_B)]);

  const owner = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/users/${U.bob}`, { type: 0 });
  assert.equal(owner.status, 200);
  const bob = h.db.prepare('SELECT role FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.bob) as any;
  assert.equal(bob.role, 'owner', 'a raise to owner is the official behaviour again');
});

test('PUT one org user changes an invitation, and refuses what the member route refuses', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/users/${INV_CAROL}`, { type: 3, collections: [readOnlyAccess(COL_B)] });
  assert.equal(res.status, 200);
  const invitation = h.db.prepare('SELECT role FROM organization_invitations WHERE id = ?').get(INV_CAROL) as any;
  assert.equal(invitation.role, 'manager');
  assert.deepEqual(invitationCollectionsRaw(h, INV_CAROL), [readOnlyAccess(COL_B)]);

  assert.equal((await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/users/${U.alice}`, { type: 9 })).status, 400);
  assert.equal((await call(h, 'alice', 'PUT', `/api/organizations/${ORG}/users/${U.bob}`, { type: 1 })).status, 404);
  const self = await call(h, 'owner', 'PUT', `/api/organizations/${ORG}/users/${U.owner}`, { type: 2 });
  assert.equal(self.status, 403);
  assert.equal(count(h, "SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND user_id = ? AND role = 'owner'", ORG, U.owner), 1);
});

test('DELETE on a live cipher deletes it permanently, as Bitwarden does', async () => {
  const h = fixture();
  seedCipher(h, CIPHER, U.owner, { organizationId: ORG, collectionIds: [COL_A] });
  const res = await call(h, 'owner', 'DELETE', `/api/ciphers/${CIPHER}`);
  assert.equal(res.status, 204);
  assert.equal(count(h, 'SELECT COUNT(*) FROM ciphers WHERE id = ?', CIPHER), 0, 'the cipher went to the trash instead');
});
```

Check the LF endings of both test files.

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing 2>&1 | grep -E "^not ok|^# (pass|fail)"`
Expected: failures for:
- `member update raises and lowers roles…` (400 "needs their acceptance");
- `GET one org user…` (405);
- `PUT one org user changes a member's…` (405);
- `PUT one org user changes an invitation…` (405);
- `DELETE on a live cipher…` (the row is still there).

- [ ] **Step 3: Remove the raise refusal and share the member update**

In `src/handlers/organizations.ts`, replace:

```ts
// Raising an existing member to one of these through a member update is
// refused: the person never agreed to it. See handleUpdateOrganizationMember.
const RAISE_NEEDS_ACCEPTANCE =
  'Making someone an admin or owner needs their acceptance. Remove them and invite them with that role.';

```

with nothing (delete those five lines, the blank line after them included).

Replace the whole of `handleUpdateOrganizationMember`, from:

```ts
// PUT /api/organizations/:id/members/:userId
export async function handleUpdateOrganizationMember(request: Request, env: Env, userId: string, orgId: string, memberUserId: string): Promise<Response> {
```

through its closing `}` (the line before `// Drops one accepted member. Shared by DELETE /members/:userId and the`), with:

```ts
// PUT /api/organizations/:id/members/:userId
export async function handleUpdateOrganizationMember(request: Request, env: Env, userId: string, orgId: string, memberUserId: string): Promise<Response> {
  const member = await requireConfirmedManager(env, orgId, userId);
  if (!member) return errorResponse('Not found', 404);

  let body: any;
  try {
    body = await request.json();
  } catch {
    return errorResponse('Invalid JSON', 400);
  }
  return updateOrganizationMember(request, env, member, orgId, memberUserId, body ?? {});
}

// One accepted member's role, collections, permissions or key, changed by a
// confirmed manager `actor`. Shared by PUT /members/:userId (body.role a
// word) and the official PUT /users/:ouId (organization-users.ts, which turns
// Bitwarden's `type` into the word first).
//
// A raise to admin or owner is allowed again, as on official servers. 47869c4
// refused it because a client treated an org it owned and that was named for
// it as its own sharing org; Deskwarden now also requires that no OTHER account
// holds the owner role there, so a raise cannot plant one. Direct add stays
// member-only (roleNeedsAcceptance).
export async function updateOrganizationMember(
  request: Request,
  env: Env,
  actor: OrganizationMember,
  orgId: string,
  memberUserId: string,
  body: { role?: unknown; collections?: unknown; permissions?: unknown; encryptedOrgKey?: unknown }
): Promise<Response> {
  const member = actor;
  const targetMember = await getOrganizationMember(env.DB, orgId, memberUserId);
  if (!targetMember) return errorResponse('Not found', 404);

  // An actor may only modify members at or below their own rank: a manager must
  // not demote an admin or rewrite their collections/permissions/org key.
  if (orgRoleRank(targetMember.role) > orgRoleRank(member.role)) {
    return errorResponse('You cannot manage a member with a higher role than your own', 403);
  }

  // Only the owner may change another owner's role, and only the owner may demote the owner.
  if (targetMember.role === 'owner' && member.role !== 'owner') return errorResponse('Only the organization owner can manage owners', 403);
  if (targetMember.role === 'owner' && body.role !== undefined && body.role !== 'owner' && member.userId !== memberUserId) {
    return errorResponse('Only the organization owner can change the owner role', 403);
  }

  const now = new Date().toISOString();
  if (body.role !== undefined) {
    const nextRole = normalizeRole(body.role);
    // No self-promotion, and no granting a role above your own.
    if (member.userId === memberUserId && nextRole !== targetMember.role) {
      return errorResponse('You cannot change your own role', 403);
    }
    if (!canAssignOrgRole(member, nextRole)) {
      return errorResponse('You cannot grant a role above your own', 403);
    }
    targetMember.role = nextRole;
  }
  if (body.collections !== undefined) {
    const nextCollections = applyCollectionAccessUpdate(targetMember.collections || [], body.collections);
    if (await hasForeignCollection(env, orgId, nextCollections)) return errorResponse(FOREIGN_COLLECTION_ERROR, 400);
    targetMember.collections = nextCollections;
  }
  if (body.permissions !== undefined && (body.permissions === null || typeof body.permissions === 'object')) {
    // A manager's write can neither set nor erase the account holder's own
    // SSO-link consent: the body's ssoLinkedAt is ignored and the stored one
    // carries over exactly. null counts as {}.
    targetMember.permissions = keepSsoConsent(targetMember.permissions, body.permissions);
  }
  if (typeof body.encryptedOrgKey === 'string' && body.encryptedOrgKey) {
    // Matches the official confirm route: a member who already holds a
    // wrapped org key stays confirmed with that key. Without this, any
    // manager at or below the target's rank could re-key an already-
    // confirmed member -- exactly the key-planting this route must not allow.
    if (targetMember.encryptedOrgKey) return errorResponse('Already confirmed', 400);
    targetMember.encryptedOrgKey = body.encryptedOrgKey;
  }
  targetMember.updatedAt = now;

  await saveOrganizationMember(env.DB, targetMember);
  await bumpMembersRevision(env, [targetMember.userId], readActingDeviceIdentifier(request));
  const targetUser = await getUserById(env.DB, memberUserId);
  await writeOrgAudit(env, member.userId, 'organization.member.update', 'organization', orgId, {
    memberId: memberUserId,
    role: targetMember.role,
    organizationId: orgId,
  });

  if (!targetUser) return errorResponse('Not found', 404);
  return jsonResponse(toMemberResponse(targetMember, targetUser));
}
```

The audit's actor was `userId` before. It is now `member.userId`, the same account, because `requireConfirmedManager` returns the caller's own membership.

The comment above `roleNeedsAcceptance` (lines 528–532) still says why direct add is member-only. Leave it.

- [ ] **Step 4: Add the official GET and PUT on one org user**

In `src/handlers/organization-users.ts`, replace:

```ts
import {
  FOREIGN_COLLECTION_ERROR,
  acceptOrganizationInvitation,
  bumpOrgMemberAndManagers,
  removeOrganizationMember,
  requireConfirmedManager,
  requireManager,
  revokeOrganizationInvitation,
  writeOrgAudit,
} from './organizations';
```

with:

```ts
import {
  FOREIGN_COLLECTION_ERROR,
  acceptOrganizationInvitation,
  bumpOrgMemberAndManagers,
  removeOrganizationMember,
  requireConfirmedManager,
  requireManager,
  revokeOrganizationInvitation,
  updateOrganizationMember,
  writeOrgAudit,
} from './organizations';
```

Replace:

```ts
function sameToken(a: string, b: string): boolean {
```

with:

```ts
// GET /api/organizations/:orgId/users/:ouId
// Bitwarden's OrganizationUserDetailsResponseModel, for one member or one
// pending invitation, with its collections. `bw confirm org-member` reads
// `userId` here before it fetches the key it wraps the org key to. Managers
// only, as the list is.
export async function handleGetOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  void request;
  if (!(await requireManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const row = (await loadOrgUserRows(env, orgId)).find((candidate) => candidate.id === ouId);
  if (!row) return errorResponse('Not found', 404);
  return jsonResponse({ ...toOrgUserDetails(row, true), object: 'organizationUserDetails' });
}

// PUT /api/organizations/:orgId/users/:ouId
//   { type, collections, groups, permissions, accessSecretsManager }
// The official member edit: Bitwarden's `type` becomes NodeWarden's role word,
// and a member goes through the same rules as PUT /members/:userId
// (updateOrganizationMember). A pending invitation's role and collections are
// changed in place. Answered with an empty 200, as on official servers.
export async function handleUpdateOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  const actor = await requireConfirmedManager(env, orgId, userId);
  if (!actor) return errorResponse('Not found', 404);
  const body = await readJson(request);
  if (!body || typeof body !== 'object') return errorResponse('Invalid JSON', 400);
  const role = body.type === undefined ? undefined : orgRoleForUserType(body.type);
  if (body.type !== undefined && !role) return errorResponse('A valid member type is required', 400);

  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return errorResponse('Not found', 404);
  if (target.kind === 'member') {
    const answer = await updateOrganizationMember(request, env, actor, orgId, target.member.userId, {
      role,
      collections: body.collections,
      permissions: body.permissions,
    });
    return answer.status === 200 ? new Response(null, { status: 200 }) : answer;
  }

  const invitation = target.invitation;
  if (role !== undefined) {
    if (!canAssignOrgRole(actor, role)) return errorResponse('You cannot grant a role above your own', 403);
    invitation.role = role;
  }
  if (body.collections !== undefined) {
    const collections = normalizeCollectionAccessList(body.collections);
    const orgCollectionIds = new Set((await getCollectionsForOrganization(env.DB, orgId)).map((collection) => collection.id));
    if (collections.some((entry) => !orgCollectionIds.has(entry.id))) return errorResponse(FOREIGN_COLLECTION_ERROR, 400);
    invitation.collections = collections;
  }
  invitation.updatedAt = new Date().toISOString();
  await saveOrganizationInvitation(env.DB, invitation);
  await writeOrgAudit(env, userId, 'organization.invitation.update', 'organization', orgId, {
    email: invitation.email,
    role: invitation.role,
    organizationId: orgId,
  });
  return new Response(null, { status: 200 });
}

function sameToken(a: string, b: string): boolean {
```

`orgRoleForUserType`, `resolveOrgUser`, `canAssignOrgRole`, `normalizeCollectionAccessList`, `getCollectionsForOrganization` and `saveOrganizationInvitation` are already imported there.

- [ ] **Step 5: Route them**

In `src/router-authenticated.ts`, find the import list that names `handleRemoveOrganizationUser` from `'./handlers/organization-users'`, and add the two names beside it:

```ts
  handleGetOrganizationUser,
  handleUpdateOrganizationUser,
```

Then replace:

```ts
      if (action === '' && method === 'DELETE') return handleRemoveOrganizationUser(request, env, userId, orgId, ouId);
```

with:

```ts
      if (action === '' && method === 'GET') return handleGetOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '' && method === 'PUT') return handleUpdateOrganizationUser(request, env, userId, orgId, ouId);
      if (action === '' && method === 'DELETE') return handleRemoveOrganizationUser(request, env, userId, orgId, ouId);
```

- [ ] **Step 6: Make `DELETE /api/ciphers/{id}` permanent**

In `src/handlers/ciphers.ts`, replace:

```ts
// DELETE /api/ciphers/:id (compat mode)
// Bitwarden clients may call DELETE on a trashed item to purge it permanently.
// For compatibility:
// - If item is active -> soft delete.
// - If item is already soft-deleted -> hard delete.
export async function handleDeleteCipherCompat(request: Request, env: Env, userId: string, id: string): Promise<Response> {
  const storage = new StorageService(env.DB);
  const editable = await resolveEditableCipher(env.DB, userId, id);
  if (!editable.ok) {
    return errorResponse(editable.error, editable.status);
  }
  const cipher = editable.access.cipher;

  if (cipher.deletedAt) {
    await deleteAllAttachmentsForCipher(env, id);
    // Access resolved above; owner-scoped delete would no-op for a non-author member.
    await storage.deleteCipherById(id);
    const revisionDate = await storage.updateRevisionDate(userId);
    notifyCipherDeleteForRequest(request, env, cipher, revisionDate);
    await notifyOrgCipherChange(request, env, cipher);
    await writeCipherAudit(storage, request, userId, 'cipher.delete.permanent', {
      id,
      type: cipher.type,
      folderId: cipher.folderId ?? null,
      compat: true,
    });
    return new Response(null, { status: 204 });
  }

  return softDeleteCipher(request, env, userId, editable.access);
}
```

with:

```ts
// DELETE /api/ciphers/:id
// Bitwarden's [HttpDelete("{id}")] Delete: permanent, live or trashed. It used
// to soft-delete a live item ("compat mode"), so `bw delete item --permanent`
// only trashed it here, and a sharer's client moving a record back out of its
// sharing org left the org copy in the trash. Moving to the trash is
// PUT /ciphers/:id/delete, as on official servers.
export async function handleDeleteCipherCompat(request: Request, env: Env, userId: string, id: string): Promise<Response> {
  const storage = new StorageService(env.DB);
  const editable = await resolveEditableCipher(env.DB, userId, id);
  if (!editable.ok) {
    return errorResponse(editable.error, editable.status);
  }
  const cipher = editable.access.cipher;

  await deleteAllAttachmentsForCipher(env, id);
  // Access resolved above; owner-scoped delete would no-op for a non-author member.
  await storage.deleteCipherById(id);
  const revisionDate = await storage.updateRevisionDate(userId);
  notifyCipherDeleteForRequest(request, env, cipher, revisionDate);
  await notifyOrgCipherChange(request, env, cipher);
  await writeCipherAudit(storage, request, userId, 'cipher.delete.permanent', {
    id,
    type: cipher.type,
    folderId: cipher.folderId ?? null,
    compat: true,
  });
  return new Response(null, { status: 204 });
}
```

If `pnpm exec tsc --noEmit` then reports `softDeleteCipher` as unused, check `grep -n "softDeleteCipher(" src/handlers/ciphers.ts`. `handleDeleteCipher` (the `PUT …/delete` route) also calls it, so it stays.

Run the LF checks on the four edited `src` files.

- [ ] **Step 6b: NodeWarden's own web vault trashes with the trash route, not DELETE**

**This step is not optional.** NodeWarden's web vault moves an item to the trash by calling the route that Step 6 makes permanent:
- `webapp/src/lib/api/vault.ts`, `deleteCipher`, about line 1502: `DELETE /api/ciphers/{id}`;
- called from `webapp/src/hooks/useVaultSendActions.ts`, about line 674, as "delete item" (to trash).

Left as it is, the web vault's ordinary Delete would destroy the item with no trash. Bitwarden's trash route is `PUT /api/ciphers/{id}/delete`. NodeWarden routes it to `handleDeleteCipher` → `softDeleteCipher` (`src/router-authenticated.ts` line 392), which answers the cipher as JSON. So the function's return type does not change.

In `webapp/src/lib/api/vault.ts`, replace:

```ts
export async function deleteCipher(authedFetch: AuthedFetch, cipherId: string): Promise<Cipher> {
  const resp = await authedFetch(`/api/ciphers/${encodeURIComponent(cipherId)}`, { method: 'DELETE' });
```

with:

```ts
// Bitwarden's trash route. `DELETE /api/ciphers/{id}` is Bitwarden's
// PERMANENT delete, which NodeWarden matches since this change: calling it
// here would destroy the item instead of moving it to the trash.
export async function deleteCipher(authedFetch: AuthedFetch, cipherId: string): Promise<Cipher> {
  const resp = await authedFetch(`/api/ciphers/${encodeURIComponent(cipherId)}/delete`, { method: 'PUT' });
```

Then:
- Check that no other web vault code sends a bare `DELETE` to a cipher. Run `grep -rn "api/ciphers/\${" webapp/src | grep "DELETE"`. The only hits must be `permanentDeleteCipher` (`…/delete`, DELETE) and attachment deletes (`…/attachment/…`).
- Run `pnpm run test:webapp` and expect it to pass. If the webapp tests mock the trash call by method and path, update the mock to `PUT …/delete` in the same commit.
- Run the LF check on `webapp/src/lib/api/vault.ts`.

**Deploy note for DW5 and the release:** the server and the web vault ship in the same Worker deploy. A browser tab left open across the deploy still runs the old web vault, whose Delete would now be permanent. The release notes must tell the owner to reload any open NodeWarden web vault tabs after deploying.

- [ ] **Step 7: Run the tests to see them pass**

Run: `pnpm run test:sharing 2>&1 | grep -E "^# (pass|fail)"`
Expected: `# fail 0`.

Run: `pnpm run test:notification-pushes`
Expected: PASS. `handleDeleteCipherCompat` still sends `notifyCipherDeleteForRequest` and no whole-vault push.

- [ ] **Step 8: Verify the whole NodeWarden suite**

Run each of the eight commands in Global Constraints › NodeWarden › Verify per task.
Expected: `# fail 0` everywhere, and `tsc` silent.

- [ ] **Step 9: Commit**

```bash
git add src/handlers/organizations.ts src/handlers/organization-users.ts src/router-authenticated.ts src/handlers/ciphers.ts scripts/sharing-confirmed-manager.test.ts scripts/sharing-org-user-details.test.ts webapp/src/lib/api/vault.ts
git commit -F - <<'EOF'
One org user can be read and edited on the official routes, and DELETE on a cipher is permanent

GET and PUT /organizations/{orgId}/users/{id} answered 405. The first
is what `bw confirm org-member` reads before it confirms anyone; the
second is the official web vault's member edit. Both are answered now,
for a member or a pending invitation, managers only; PUT turns
Bitwarden's type into the role and answers empty.

A member update no longer refuses a raise to admin or owner (reverts
that half of 47869c4). The refusal guarded clients that took an org
they owned and that was named for them as their own sharing org;
Deskwarden now also requires that no other account is an owner there.
Direct add of an existing account stays member-only.

DELETE /api/ciphers/{id} soft-deleted a live cipher. It now deletes
permanently, as Bitwarden's does; trash is PUT /ciphers/{id}/delete.
NodeWarden's own web vault trashed through that DELETE, so it now uses
PUT /ciphers/{id}/delete too; otherwise its Delete would destroy items.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task NW3: `INVITE_MAIL` off — accept on invite and on registration, as Vaultwarden does

**Files:**
- Create: `E:\Personal\nodewarden\src\services\invite-mail.ts`
- Modify: `E:\Personal\nodewarden\src\types\index.ts:2-31` (`Env`)
- Modify: `E:\Personal\nodewarden\src\handlers\organization-users.ts:176-260` (invite, reinvite), plus a new exported helper
- Modify: `E:\Personal\nodewarden\src\handlers\accounts.ts:1-19` (imports), `:394-414` (after the invite-path registration)
- Modify: `E:\Personal\nodewarden\scripts\support\sharing-harness.ts` (append `inviteMailOn`)
- Test: create `E:\Personal\nodewarden\scripts\sharing-invite-accept.test.ts`; modify `scripts/sharing-org-users.test.ts:1-7,85-114`, `scripts/sharing-membership.test.ts:34-36`, `scripts/sharing-profile.test.ts:127-143`, `scripts/sharing-sso-takeover.test.ts:170-180`

**Interfaces:**
- Consumes: nothing from NW1 or NW2.
- Produces:
  - `inviteMailEnabled(env: Env): boolean` in `src/services/invite-mail.ts`.
  - `Env.INVITE_MAIL?: string`.
  - `acceptInvitationsOnRegistration(env: Env, userId: string, email: string): Promise<void>` in `organization-users.ts`.
  - `inviteMailOn(h: Harness): void` in the harness. NW4 changes it to return the sent mail.
  - Behaviour: with `INVITE_MAIL` unset, `POST …/users/invite` leaves an existing account with a master password at roster `status: 1`. DW1 reads that as "Accepts on invite".

- [ ] **Step 1: Write the failing tests**

Create `scripts/sharing-invite-accept.test.ts` with the Write tool:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { handleRegister } from '../src/handlers/accounts';
import {
  COL_A, ENC, ORG, T0, U, call, count, createHarness, emailOf, inviteMailOn, readOnlyAccess,
  seedCollection, seedInvitation, seedOrg, seedUser,
} from './support/sharing-harness';

// INVITE_MAIL off -- the default -- is Vaultwarden with mail disabled
// (vaultwarden/src/api/core/organizations.rs send_invite, lines 1054-1061;
// accounts.rs register, lines 351-377): there is no token to deliver, so an
// existing account is Accepted at invite, a new address becomes Accepted when
// it registers, and the inviter's client confirms either.

const INV_NEW = 'eeeeeeee-0000-4000-8000-0000000000a1';

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  return h;
}

async function rosterRow(h: ReturnType<typeof fixture>, email: string): Promise<any> {
  const roster = await call(h, 'owner', 'GET', `/api/organizations/${ORG}/users?includeCollections=true`);
  return roster.body.data.find((row: any) => row.email === email);
}

function register(h: ReturnType<typeof fixture>, email: string, code: string): Promise<Response> {
  h.db.prepare(
    "INSERT INTO invites(code, created_by, used_by, expires_at, status, created_at, updated_at) VALUES(?, ?, NULL, ?, 'active', ?, ?)"
  ).run(code, U.owner, '2999-01-01T00:00:00.000Z', T0, T0);
  return handleRegister(
    new Request('https://vault.example.test/api/accounts/register', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        email,
        masterPasswordHash: 'hash-newcomer',
        key: ENC,
        kdf: 0,
        kdfIterations: 600000,
        inviteCode: code,
        keys: { publicKey: 'PUBKEY-newcomer', encryptedPrivateKey: ENC },
      }),
    }),
    h.env
  );
}

test('INVITE_MAIL off: an existing account is Accepted at invite, with no invitation and no key', async () => {
  const h = fixture();
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: [emailOf('alice')], type: 2, collections: [readOnlyAccess(COL_A)], groups: [], permissions: null, accessSecretsManager: false,
  });
  assert.equal(res.status, 200);
  assert.equal(res.body, null);
  const row = await rosterRow(h, emailOf('alice'));
  assert.equal(row.status, 1);
  assert.equal(row.id, U.alice);
  assert.equal(row.userId, U.alice);
  assert.deepEqual(row.collections, [readOnlyAccess(COL_A)]);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE email = ?', emailOf('alice')), 0);

  // The inviter's client confirms it: the official confirm takes an Accepted member.
  const confirmed = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${U.alice}/confirm`, { key: '4.d3JhcHBlZA==' });
  assert.equal(confirmed.status, 200);
  assert.equal((await rosterRow(h, emailOf('alice'))).status, 2);
});

test('INVITE_MAIL off: a new address is invited, and is Accepted when it registers', async () => {
  const h = fixture();
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: ['newcomer@example.test'], type: 2, collections: [readOnlyAccess(COL_A)],
  });
  assert.equal((await rosterRow(h, 'newcomer@example.test')).status, 0);

  const registered = await register(h, 'Newcomer@Example.test', 'code-newcomer');
  assert.equal(registered.status, 200, await registered.clone().text());
  const user = h.db.prepare('SELECT id FROM users WHERE email = ?').get('newcomer@example.test') as any;
  const row = await rosterRow(h, 'newcomer@example.test');
  assert.equal(row.status, 1);
  assert.equal(row.userId, user.id);
  assert.deepEqual(row.collections, [readOnlyAccess(COL_A)]);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE email = ?', 'newcomer@example.test'), 0);
});

test('INVITE_MAIL off: registration accepts an invitation whatever its expiry, as a mail-off one never expires', async () => {
  const h = fixture();
  seedInvitation(h, ORG, INV_NEW, 'late@example.test', { invitedBy: U.owner, expiresAt: '2001-01-01T00:00:00.000Z' });
  assert.equal((await register(h, 'late@example.test', 'code-late')).status, 200);
  assert.equal((await rosterRow(h, 'late@example.test')).status, 1);
});

test('INVITE_MAIL off: a re-invite of an address that has an account by now makes it Accepted', async () => {
  const h = fixture();
  seedInvitation(h, ORG, INV_NEW, emailOf('alice'), { invitedBy: U.owner, collections: [readOnlyAccess(COL_A)] });
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${INV_NEW}/reinvite`);
  assert.equal(res.status, 200);
  const row = await rosterRow(h, emailOf('alice'));
  assert.equal(row.status, 1);
  assert.equal(row.userId, U.alice);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE id = ?', INV_NEW), 0);
});

test('INVITE_MAIL on: an existing account still gets a status-0 invitation, and registration accepts nothing', async () => {
  const h = fixture();
  inviteMailOn(h);
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: [emailOf('alice')], type: 2 });
  assert.equal((await rosterRow(h, emailOf('alice'))).status, 0);

  seedInvitation(h, ORG, INV_NEW, 'mailed@example.test', { invitedBy: U.owner });
  assert.equal((await register(h, 'mailed@example.test', 'code-mailed')).status, 200);
  assert.equal((await rosterRow(h, 'mailed@example.test')).status, 0, 'with mail on, the emailed token is the way in');
});
```

In `scripts/support/sharing-harness.ts`, append at the end of the file:

```ts

/**
 * Turns INVITE_MAIL on for this harness. Off -- the default, with the variable
 * unset -- an invited account is accepted at once (organization-users.ts).
 */
export function inviteMailOn(h: Harness): void {
  (h.env as any).INVITE_MAIL = '1';
}
```

Now update the four existing tests that assumed an existing account always got a status-0 invitation.

1. In `scripts/sharing-org-users.test.ts`, replace the import block:

```ts
import {
  COL_A, COL_B, COL_X, INV_CAROL, ORG, ORG2, U, call, count, createHarness, emailOf,
  invitationCollectionsRaw, readOnlyAccess, resetRevisions, seedCollection, seedInvitation,
  seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';
```

with:

```ts
import {
  COL_A, COL_B, COL_X, INV_CAROL, ORG, ORG2, U, call, count, createHarness, emailOf,
  invitationCollectionsRaw, inviteMailOn, memberCollectionsRaw, readOnlyAccess, resetRevisions, seedCollection, seedInvitation,
  seedMember, seedOrg, seedUser, wasBumped,
} from './support/sharing-harness';
```

Then replace the whole test that begins:

```ts
test('an official invite always makes an invitation, skips people already in, and tells managers', async () => {
```

(through its closing `});`) with:

```ts
test('with INVITE_MAIL off an invite accepts an existing account at once, invites a new address, skips people already in, and tells managers', async () => {
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
  // dave has an account: Accepted at once, as Vaultwarden does with mail
  // disabled. frank has none and is invited; alice is a member and carol
  // already holds a live invitation, so both are skipped.
  assert.deepEqual(emails, ['carol@example.test', 'frank@example.test']);
  const dave = h.db.prepare('SELECT status, encrypted_org_key FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.dave) as any;
  assert.equal(dave.status, 1);
  assert.equal(dave.encrypted_org_key, null);
  assert.deepEqual(memberCollectionsRaw(h, ORG, U.dave), [readOnlyAccess(COL_A)]);

  assert.equal(wasBumped(h, U.owner), true);
  assert.equal(wasBumped(h, U.dave), true);
  assert.equal(wasBumped(h, U.alice), false);

  const mine = await call(h, 'dave', 'GET', '/api/organizations/invitations/mine');
  assert.equal(mine.body.data.length, 0);
});

test('with INVITE_MAIL on an official invite always makes an invitation, even for an existing account', async () => {
  const h = rosterFixture();
  inviteMailOn(h);
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: [emailOf('dave'), emailOf('alice'), 'Frank@Example.test', emailOf('carol')],
    type: 2,
    collections: [readOnlyAccess(COL_A)],
  });
  assert.equal(res.status, 200);
  const emails = (h.db.prepare('SELECT email FROM organization_invitations WHERE organization_id = ? ORDER BY email').all(ORG) as any[])
    .map((row) => row.email);
  assert.deepEqual(emails, ['carol@example.test', 'dave@example.test', 'frank@example.test']);
  const dave = h.db.prepare('SELECT id FROM organization_invitations WHERE email = ?').get(emailOf('dave')) as any;
  assert.deepEqual(invitationCollectionsRaw(h, dave.id), [readOnlyAccess(COL_A)]);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE user_id = ?', U.dave), 0);
  const mine = await call(h, 'dave', 'GET', '/api/organizations/invitations/mine');
  assert.equal(mine.body.data.length, 1);
});
```

2. In `scripts/sharing-membership.test.ts`, add `inviteMailOn` to the harness import. Replace:

```ts
  COL_A, INV_CAROL, ORG, ORG2, U, call, count, createHarness, emailOf, memberCollectionsRaw,
```

with:

```ts
  COL_A, INV_CAROL, ORG, ORG2, U, call, count, createHarness, emailOf, inviteMailOn, memberCollectionsRaw,
```

Then replace:

```ts
test('invite, accept and confirm on the official routes move a person 0 -> 1 -> 2', async () => {
  const h = fixture();
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
```

with:

```ts
test('invite, accept and confirm on the official routes move a person 0 -> 1 -> 2', async () => {
  const h = fixture();
  // With mail on, as on an official server: the accept is the invitee's.
  // (With it off, dave would be Accepted at invite -- sharing-invite-accept.)
  inviteMailOn(h);
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
```

3. In `scripts/sharing-profile.test.ts`, replace:

```ts
  const roster = await call(h, 'owner', 'GET', `/api/organizations/${orgId}/users`);
  const aliceRow = roster.body.data.find((row: any) => row.email === emailOf('alice'));
  assert.ok(aliceRow, 'alice should now be listed on the roster');
  assert.equal(aliceRow.status, 0); // invited, no account-linked membership yet

  // Seed alice directly as an accepted (unconfirmed) member to exercise confirm.
  seedMember(h, orgId, U.alice, { key: null });
  const confirm = await call(h, 'owner', 'POST', `/api/organizations/${orgId}/users/${U.alice}/confirm`, {
```

with:

```ts
  const roster = await call(h, 'owner', 'GET', `/api/organizations/${orgId}/users`);
  const aliceRow = roster.body.data.find((row: any) => row.email === emailOf('alice'));
  assert.ok(aliceRow, 'alice should now be listed on the roster');
  // INVITE_MAIL off: alice has an account, so she is Accepted at invite.
  assert.equal(aliceRow.status, 1);

  const confirm = await call(h, 'owner', 'POST', `/api/organizations/${orgId}/users/${U.alice}/confirm`, {
```

If `seedMember` is now unused in that file, `grep -n "seedMember" scripts/sharing-profile.test.ts` shows only the import. Then remove it from the import list (`tsc` does not flag unused imports, but keep the list honest).

4. In `scripts/sharing-sso-takeover.test.ts`, replace:

```ts
test('/users/invite: an official (status-0) invitation does not confer SSO login eligibility', async () => {
  const h = fixture();
  await seedSsoConfig(h, ORG2);
  const invited = await call(h, 'dave', 'POST', `/api/organizations/${ORG2}/users/invite`, { emails: [emailOf('carol')], type: 2 });
  assert.equal(invited.status, 200);
```

with:

```ts
// With INVITE_MAIL off, carol (who has an account) is Accepted at invite, as
// Vaultwarden does with mail disabled. Membership is still not SSO consent:
// checkSsoEligibility requires the member's own Link (ssoLinkedAt).
test('/users/invite: a membership accepted at invite does not confer SSO login eligibility', async () => {
  const h = fixture();
  await seedSsoConfig(h, ORG2);
  const invited = await call(h, 'dave', 'POST', `/api/organizations/${ORG2}/users/invite`, { emails: [emailOf('carol')], type: 2 });
  assert.equal(invited.status, 200);
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_members WHERE organization_id = ? AND user_id = ? AND status = 1', ORG2, U.carol), 1);
  assert.equal(memberPermissions(h, ORG2, U.carol).ssoLinkedAt, undefined);
```

Check the LF endings of every file touched in this step.

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing 2>&1 | grep -E "^not ok|^# (pass|fail)"`
Expected:
- the four `INVITE_MAIL off` tests fail (status 0, not 1);
- the rewritten `with INVITE_MAIL off an invite accepts…` fails;
- `sharing-profile`'s official-body test fails (status 0);
- the SSO test fails its new member-count line.

- [ ] **Step 3: Write the setting's module**

Create `src/services/invite-mail.ts` with the Write tool:

```ts
import type { Env } from '../types';

// INVITE_MAIL: how an invited person learns of an organization invitation.
//
// Off -- the default, with the variable unset -- is Vaultwarden with mail
// disabled (vaultwarden/src/api/core/organizations.rs send_invite): there is
// no token to deliver, so an existing account is Accepted at invite and a new
// address becomes Accepted when it registers. The inviter's client confirms
// either. On, NodeWarden sends Bitwarden's invitation email instead (the
// rest of this module, NW4).
export function inviteMailEnabled(env: Env): boolean {
  return /^(1|true|on|yes)$/i.test(String(env.INVITE_MAIL ?? '').trim());
}
```

In `src/types/index.ts`, replace:

```ts
  YUBICO_VALIDATION_URLS?: string;
  'globalSettings__yubico__validationUrls'?: string;
}
```

with:

```ts
  YUBICO_VALIDATION_URLS?: string;
  'globalSettings__yubico__validationUrls'?: string;
  // "1" turns on Bitwarden's organization invitation email. Unset (the
  // default), an invited account is accepted at once, as Vaultwarden does
  // with mail disabled. See services/invite-mail.ts.
  INVITE_MAIL?: string;
}
```

- [ ] **Step 4: Accept on invite, on re-invite and on registration**

In `src/handlers/organization-users.ts`, replace:

```ts
  getOrganizationInvitations,
  getOrganizationMember,
```

with:

```ts
  getOrganizationInvitations,
  getOrganizationInvitationsForEmail,
  getOrganizationMember,
```

Replace:

```ts
import { normalizeCollectionAccessList } from '../services/org-collection-access';
```

with:

```ts
import { normalizeCollectionAccessList } from '../services/org-collection-access';
import { stripSsoConsent } from '../services/sso-consent';
import { inviteMailEnabled } from '../services/invite-mail';
```

In `handleInviteOrganizationUsers`, replace:

```ts
    // An expired invitation for the same address is replaced, not duplicated.
    for (const row of existing) await deleteOrganizationInvitation(env.DB, row.id);

    const invitation: OrganizationInvitation = {
```

with:

```ts
    // An expired invitation for the same address is replaced, not duplicated.
    for (const row of existing) await deleteOrganizationInvitation(env.DB, row.id);

    const account = await getUser(env.DB, email);
    // INVITE_MAIL off: an account that has a master password is Accepted now,
    // with no key -- Vaultwarden's "automatically accept existing users if mail
    // is disabled". There is no email to carry a token, and the inviter's
    // client confirms an Accepted member on its own. Anyone else gets the
    // invitation below, which their registration accepts
    // (acceptInvitationsOnRegistration).
    if (account?.masterPasswordHash && !inviteMailEnabled(env)) {
      const member = await getOrganizationMember(env.DB, orgId, account.id);
      await saveOrganizationMember(env.DB, {
        organizationId: orgId,
        userId: account.id,
        status: 1,
        role,
        permissions: {},
        collections,
        encryptedOrgKey: null,
        createdAt: member?.createdAt ?? now,
        updatedAt: now,
      });
      await writeOrgAudit(env, userId, 'organization.invite.accepted', 'organization', orgId, {
        email,
        role,
        organizationId: orgId,
        collectionCount: collections.length,
      });
      invitees.push(account.id);
      continue;
    }

    const invitation: OrganizationInvitation = {
```

Further down the same loop, replace:

```ts
    invitees.push((await getUser(env.DB, email))?.id ?? null);
```

with:

```ts
    invitees.push(account?.id ?? null);
```

Replace the whole of `handleReinviteOrganizationUser`, from:

```ts
// POST /api/organizations/:orgId/users/:ouId/reinvite
```

through its closing `}` (the line before `// DELETE /api/organizations/:orgId/users/:ouId -- a member or an invitation.`), with:

```ts
// POST /api/organizations/:orgId/users/:ouId/reinvite
// INVITE_MAIL off and the address has an account with a master password by
// now: it is Accepted, and the invitation is spent -- Vaultwarden's
// reinvite_member_impl with mail disabled. Otherwise a new token and a new
// expiry.
export async function handleReinviteOrganizationUser(request: Request, env: Env, userId: string, orgId: string, ouId: string): Promise<Response> {
  if (!(await requireConfirmedManager(env, orgId, userId))) return errorResponse('Not found', 404);
  const target = await resolveOrgUser(env.DB, orgId, ouId);
  if (!target) return errorResponse('Not found', 404);
  if (target.kind !== 'invitation') return errorResponse('User is not in an invited state', 400);

  const invitation = target.invitation;
  const invitee = await getUser(env.DB, invitation.email);
  if (invitee?.masterPasswordHash && !inviteMailEnabled(env)) {
    await acceptInvitationAs(env, invitation, invitee.id);
    await writeOrgAudit(env, userId, 'organization.invite.accepted', 'organization', orgId, {
      email: invitation.email,
      organizationId: orgId,
    });
    await bumpOrgMemberAndManagers(env, orgId, [invitee.id], readActingDeviceIdentifier(request));
    return new Response(null, { status: 200 });
  }

  const nowMs = Date.now();
  invitation.token = newInvitationToken();
  invitation.expiresAt = invitationExpiry(nowMs);
  invitation.updatedAt = new Date(nowMs).toISOString();
  await saveOrganizationInvitation(env.DB, invitation);
  await writeOrgAudit(env, userId, 'organization.reinvite', 'organization', orgId, {
    email: invitation.email,
    organizationId: orgId,
  });
  await bumpOrgMemberAndManagers(env, orgId, [invitee?.id], readActingDeviceIdentifier(request));
  return new Response(null, { status: 200 });
}

// An invitation turned into an Accepted membership of `userId`, with no key:
// its role and collections, and permissions without SSO consent (consent is
// the member's own Link, never an inviter's). A membership already accepted
// is left as it is. The invitation is spent either way.
async function acceptInvitationAs(env: Env, invitation: OrganizationInvitation, userId: string): Promise<void> {
  const now = new Date().toISOString();
  const existing = await getOrganizationMember(env.DB, invitation.organizationId, userId);
  if (!existing || existing.status !== 1) {
    await saveOrganizationMember(env.DB, {
      organizationId: invitation.organizationId,
      userId,
      status: 1,
      role: invitation.role,
      permissions: stripSsoConsent(invitation.permissions),
      collections: invitation.collections || [],
      encryptedOrgKey: null,
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
    });
  }
  await deleteOrganizationInvitation(env.DB, invitation.id);
}

// INVITE_MAIL off: every invitation waiting for a new account's address
// becomes an Accepted membership when the account registers -- Vaultwarden's
// Membership::accept_user_invitations. Expired ones too: a mail-off
// invitation is Vaultwarden's expiry-less Invitation row, and an address
// that registers is the person the inviter named. With mail on, the emailed
// token is the way in, and nothing is accepted here.
export async function acceptInvitationsOnRegistration(env: Env, userId: string, email: string): Promise<void> {
  if (inviteMailEnabled(env)) return;
  const pending = (await getOrganizationInvitationsForEmail(env.DB, email)).filter((invitation) => invitation.status === 0);
  for (const invitation of pending) {
    if (!(await getOrganization(env.DB, invitation.organizationId))) continue;
    await acceptInvitationAs(env, invitation, userId);
    await writeOrgAudit(env, userId, 'organization.invite.accepted', 'organization', invitation.organizationId, {
      email,
      organizationId: invitation.organizationId,
      onRegistration: true,
    });
    await bumpOrgMemberAndManagers(env, invitation.organizationId, [userId], null);
  }
}
```

In `src/handlers/accounts.ts`, replace:

```ts
import { buildProfileResponse } from '../utils/profile-response';
```

with:

```ts
import { buildProfileResponse } from '../utils/profile-response';
import { acceptInvitationsOnRegistration } from './organization-users';
```

Then replace:

```ts
  await writeAuditEvent(storage, {
    actorUserId: user.id,
    action: 'user.register.invite',
```

with:

```ts
  // INVITE_MAIL off: the organization invitations waiting for this address
  // become Accepted now (acceptInvitationsOnRegistration), as Vaultwarden's
  // registration does with mail disabled.
  await acceptInvitationsOnRegistration(env, user.id, user.email);

  await writeAuditEvent(storage, {
    actorUserId: user.id,
    action: 'user.register.invite',
```

The first-admin registration path returns earlier and is left alone: no organization can have invited an address before the server's first account exists.

Run the LF checks on every file touched.

- [ ] **Step 5: Run the tests to see them pass**

Run: `pnpm run test:sharing 2>&1 | grep -E "^# (pass|fail)"`
Expected: `# fail 0`.

Run: `pnpm run test:sso 2>&1 | grep -E "^# (pass|fail)"`
Expected: `# fail 0`. That script runs only `sso-id-token.test.ts`; the SSO takeover file runs in `test:sharing`, which passed above.

- [ ] **Step 6: Verify the whole NodeWarden suite**

Run each of the eight commands in Global Constraints › NodeWarden › Verify per task.
Expected: `# fail 0` everywhere, and `tsc` silent.

- [ ] **Step 7: Commit**

```bash
git add src/services/invite-mail.ts src/types/index.ts src/handlers/organization-users.ts src/handlers/accounts.ts scripts/support/sharing-harness.ts scripts/sharing-invite-accept.test.ts scripts/sharing-org-users.test.ts scripts/sharing-membership.test.ts scripts/sharing-profile.test.ts scripts/sharing-sso-takeover.test.ts
git commit -F - <<'EOF'
An invited account is accepted at once when invitation email is off, as on Vaultwarden

NodeWarden sends no email, so an official invitation's token reached
nobody: an invitee could join only through NodeWarden's own
invitations list, which official clients do not read. With the new
INVITE_MAIL setting off -- the default -- NodeWarden does what
Vaultwarden does with mail disabled. An existing account with a master
password is Accepted at invite. A new address keeps its invitation and
is Accepted when it registers, expired or not, since a mail-off
invitation is Vaultwarden's expiry-less one. A re-invite of an address
that has an account by now accepts it. The inviter's client confirms an
Accepted member on its own.

Acceptance copies the invitation's role and collections and never SSO
consent, which stays the member's own Link.

With INVITE_MAIL on, every invite stays a status-0 invitation; the
email that carries its token comes in the next change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task NW4: `INVITE_MAIL` on — Bitwarden's invitation email, through Cloudflare Email Service

**Files:**
- Modify: `E:\Personal\nodewarden\src\services\invite-mail.ts` (append)
- Modify: `E:\Personal\nodewarden\src\services\org-user-model.ts:36-38` (`invitationExpiry`)
- Modify: `E:\Personal\nodewarden\src\types\index.ts` (`Env`, and a new `InviteMailSender`)
- Modify: `E:\Personal\nodewarden\src\handlers\organization-users.ts` (invite and reinvite send the email)
- Modify: `E:\Personal\nodewarden\wrangler.toml` (append, commented)
- Modify: `E:\Personal\nodewarden\scripts\support\sharing-harness.ts` (`inviteMailOn` installs a fake sender)
- Test: create `E:\Personal\nodewarden\scripts\sharing-invite-mail.test.ts`

**Interfaces:**
- Consumes:
  - `inviteMailEnabled(env)` and `Env.INVITE_MAIL` (NW3);
  - `handleAcceptOrganizationUser` on `POST …/users/{ouId}/accept` (exists);
  - `invitationExpiry`, `newInvitationToken` (exist).
- Produces:
  - in `src/services/invite-mail.ts`: `INVITE_MAIL_TOKEN_TTL_MS: number` (120 hours), `invitationLifetimeMs(env: Env): number`, `dotnetUrlEncode(value: string): string`, `invitationUrl(origin: string, invitation: {organizationId: string; id: string; email: string; token: string}, organizationName: string, hasAccount: boolean): string`, `invitationSubject(hasAccount: boolean): string`, `composeInvitationMail(…): InvitationMail` and `sendInvitationMail(…): Promise<void>`;
  - `Env.INVITE_MAIL_FROM?: string`, `Env.EMAIL?: InviteMailSender`;
  - `inviteMailOn(h: Harness): InvitationMail[]`, which returns the mail the fake sender received;
  - the link format DW3's parser is tested against: `https://<origin>/#/accept-organization?organizationId=<guid>&organizationUserId=<guid>&email=<enc>&organizationName=<enc>&token=<enc>&initOrganization=False&orgUserHasExistingUser=True|False`.

- [ ] **Step 1: Write the failing tests**

Change the harness helper. In `scripts/support/sharing-harness.ts`, replace:

```ts
/**
 * Turns INVITE_MAIL on for this harness. Off -- the default, with the variable
 * unset -- an invited account is accepted at once (organization-users.ts).
 */
export function inviteMailOn(h: Harness): void {
  (h.env as any).INVITE_MAIL = '1';
}
```

with:

```ts
/**
 * Turns INVITE_MAIL on for this harness, with a fake Cloudflare Email Service
 * binding: what would be sent is kept in the returned array, and nothing
 * leaves the process. Off -- the default, with the variable unset -- an
 * invited account is accepted at once (organization-users.ts).
 */
export function inviteMailOn(h: Harness): any[] {
  const sent: any[] = [];
  const env = h.env as any;
  env.INVITE_MAIL = '1';
  env.INVITE_MAIL_FROM = 'vault@example.test';
  env.EMAIL = {
    send: async (message: unknown) => {
      sent.push(message);
      return { messageId: `fake-${sent.length}` };
    },
  };
  return sent;
}
```

Create `scripts/sharing-invite-mail.test.ts` with the Write tool:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dotnetUrlEncode, invitationSubject } from '../src/services/invite-mail';
import {
  COL_A, ORG, U, call, count, createHarness, emailOf, inviteMailOn, readOnlyAccess, seedCollection, seedOrg, seedUser,
} from './support/sharing-harness';

// INVITE_MAIL on: Bitwarden's invitation email (HandlebarsMailService
// BuildInvitationUrl, bitwarden/server main lines 875-893), through a faked
// Cloudflare Email Service binding. The link's own query is what the official
// accept route takes.

function fixture() {
  const h = createHarness();
  seedUser(h, 'owner');
  seedUser(h, 'alice');
  seedOrg(h, ORG, U.owner);
  seedCollection(h, ORG, COL_A);
  return h;
}

function linkIn(text: string): URL {
  const found = text.match(/https:\/\/\S+/);
  assert.ok(found, text);
  return new URL(found[0]);
}

test('an invite sends Bitwarden\'s email, and its link\'s token is what the official accept route takes', async () => {
  const h = fixture();
  const sent = inviteMailOn(h);
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, {
    emails: [emailOf('alice')], type: 2, collections: [readOnlyAccess(COL_A)],
  });
  assert.equal(res.status, 200);
  assert.equal(sent.length, 1);
  const mail = sent[0];
  assert.equal(mail.to, emailOf('alice'));
  assert.deepEqual(mail.from, { email: 'vault@example.test', name: 'NodeWarden' });
  assert.equal(mail.subject, 'You have been invited to a Bitwarden vault');

  const invitation = h.db.prepare('SELECT id, token, expires_at FROM organization_invitations WHERE email = ?').get(emailOf('alice')) as any;
  const expected =
    `https://vault.example.test/#/accept-organization?organizationId=${ORG}&organizationUserId=${invitation.id}` +
    `&email=alice%40example.test&organizationName=Shared+by+owner%40example.test&token=${invitation.token}` +
    '&initOrganization=False&orgUserHasExistingUser=True';
  assert.ok(mail.text.includes(expected), mail.text);
  assert.ok(mail.html.includes(expected.replace(/&/g, '&amp;')), mail.html);

  // Bitwarden's lifetime, OrganizationInviteExpirationHours = 120.
  const lifetime = Date.parse(invitation.expires_at) - Date.now();
  assert.ok(lifetime > 119 * 3_600_000 && lifetime <= 120 * 3_600_000, String(lifetime));

  const link = linkIn(mail.text);
  assert.equal(link.pathname, '/');
  const query = new URLSearchParams(link.hash.slice(link.hash.indexOf('?') + 1));
  const accepted = await call(h, 'alice', 'POST', `/api/organizations/${query.get('organizationId')}/users/${query.get('organizationUserId')}/accept`, {
    token: query.get('token'),
    resetPasswordKey: null,
  });
  assert.equal(accepted.status, 200);
  const row = h.db.prepare('SELECT status FROM organization_members WHERE organization_id = ? AND user_id = ?').get(ORG, U.alice) as any;
  assert.equal(row.status, 1);
});

test('a new address gets the no-account subject and orgUserHasExistingUser=False', async () => {
  const h = fixture();
  const sent = inviteMailOn(h);
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: ['newcomer@example.test'], type: 2 });
  assert.equal(sent[0].subject, 'You have been invited to Bitwarden Password Manager');
  assert.ok(sent[0].text.includes('&orgUserHasExistingUser=False'), sent[0].text);
});

test('a re-invite sends a fresh email with a fresh token and a fresh 120 hours', async () => {
  const h = fixture();
  const sent = inviteMailOn(h);
  await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: ['newcomer@example.test'], type: 2 });
  const first = h.db.prepare('SELECT id, token FROM organization_invitations WHERE email = ?').get('newcomer@example.test') as any;
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/${first.id}/reinvite`);
  assert.equal(res.status, 200);
  const second = h.db.prepare('SELECT token, expires_at FROM organization_invitations WHERE id = ?').get(first.id) as any;
  assert.notEqual(second.token, first.token);
  assert.equal(sent.length, 2);
  assert.ok(sent[1].text.includes(`token=${second.token}`), sent[1].text);
  assert.ok(!sent[1].text.includes(first.token), 'the old token was sent again');
});

test('an email that cannot be sent leaves no invitation behind, and the inviter is told', async () => {
  const h = fixture();
  inviteMailOn(h);
  (h.env as any).EMAIL = { send: async () => { throw new Error('domain not onboarded'); } };
  const res = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: ['newcomer@example.test'], type: 2 });
  assert.equal(res.status, 400);
  assert.equal(res.body.error, 'The invitation email could not be sent');
  assert.equal(count(h, 'SELECT COUNT(*) FROM organization_invitations WHERE email = ?', 'newcomer@example.test'), 0);

  // On, but not configured: the same refusal, never a silent invitation.
  delete (h.env as any).EMAIL;
  const unset = await call(h, 'owner', 'POST', `/api/organizations/${ORG}/users/invite`, { emails: ['newcomer@example.test'], type: 2 });
  assert.equal(unset.status, 400);
});

test('values are encoded as .NET\'s WebUtility.UrlEncode does, and the subjects are Bitwarden\'s Free-plan pair', () => {
  assert.equal(dotnetUrlEncode("Shared by a+b@x.io ~'!*()"), 'Shared+by+a%2Bb%40x.io+%7E%27!*()');
  assert.equal(dotnetUrlEncode('BwOrgUserInviteToken_CfDJ8/Ab+=='), 'BwOrgUserInviteToken_CfDJ8%2FAb%2B%3D%3D');
  assert.equal(invitationSubject(true), 'You have been invited to a Bitwarden vault');
  assert.equal(invitationSubject(false), 'You have been invited to Bitwarden Password Manager');
});
```

Check the LF endings of both files.

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm run test:sharing 2>&1 | grep -E "^not ok|^# (pass|fail)"`
Expected: `sharing-invite-mail.test.ts` fails to load, because `dotnetUrlEncode` and `invitationSubject` are not exported. Every other file still passes.

- [ ] **Step 3: Write the email module**

In `src/types/index.ts`, replace:

```ts
  // "1" turns on Bitwarden's organization invitation email. Unset (the
  // default), an invited account is accepted at once, as Vaultwarden does
  // with mail disabled. See services/invite-mail.ts.
  INVITE_MAIL?: string;
}
```

with:

```ts
  // "1" turns on Bitwarden's organization invitation email. Unset (the
  // default), an invited account is accepted at once, as Vaultwarden does
  // with mail disabled. See services/invite-mail.ts.
  INVITE_MAIL?: string;
  // The invitation email's From address: an address on a domain onboarded to
  // Cloudflare Email Sending (`wrangler email sending enable <domain>`).
  INVITE_MAIL_FROM?: string;
  // Cloudflare Email Service's Workers binding: [[send_email]] name = "EMAIL".
  EMAIL?: InviteMailSender;
}

// The one call NodeWarden makes on Cloudflare Email Service's Workers binding
// (env.EMAIL.send), narrowed to what it sends: one recipient, a From with a
// display name, both an HTML and a text body.
export interface InviteMailSender {
  send(message: { to: string; from: { email: string; name: string }; subject: string; html: string; text: string }): Promise<unknown>;
}
```

In `src/services/org-user-model.ts`, replace:

```ts
export function invitationExpiry(nowMs: number = Date.now()): string {
  return new Date(nowMs + ORG_INVITATION_TTL_MS).toISOString();
}
```

with:

```ts
export function invitationExpiry(nowMs: number = Date.now(), lifetimeMs: number = ORG_INVITATION_TTL_MS): string {
  return new Date(nowMs + lifetimeMs).toISOString();
}
```

Replace the whole of `src/services/invite-mail.ts` with the Write tool (it was NW3's six-line module):

```ts
import type { Env, OrganizationInvitation } from '../types';
import { ORG_INVITATION_TTL_MS } from './org-user-model';

// INVITE_MAIL: how an invited person learns of an organization invitation.
//
// Off -- the default, with the variable unset -- is Vaultwarden with mail
// disabled (vaultwarden/src/api/core/organizations.rs send_invite): there is
// no token to deliver, so an existing account is Accepted at invite and a new
// address becomes Accepted when it registers. The inviter's client confirms
// either.
//
// On, NodeWarden sends Bitwarden's invitation email through Cloudflare Email
// Service's Workers binding (env.EMAIL), and the person accepts with its
// link's token on the official route, POST .../users/{ouId}/accept.
//
// The link is Bitwarden's own: HandlebarsMailService.BuildInvitationUrl
// (bitwarden/server main, src/Core/Platform/Mail/HandlebarsMailService.cs,
// lines 875-893) writes
//   {vault}/#/accept-organization?organizationId=..&organizationUserId=..
//   &email=..&organizationName=..&token=..&initOrganization=..
//   &orgUserHasExistingUser=..
// with email, organizationName and token through WebUtility.UrlEncode and the
// two booleans as C# prints them. The official web vault
// (DirectOrganizationInvite.fromUrlParams) and Deskwarden's invite_link.rs
// read exactly that. Nothing here logs the link or the token.

/** Bitwarden's invitation lifetime: OrganizationInviteExpirationHours = 120 (GlobalSettings.cs line 52). */
export const INVITE_MAIL_TOKEN_TTL_MS = 120 * 60 * 60 * 1000;

export function inviteMailEnabled(env: Env): boolean {
  return /^(1|true|on|yes)$/i.test(String(env.INVITE_MAIL ?? '').trim());
}

/** How long an invitation made now lasts: Bitwarden's 120 hours with mail, NodeWarden's 7 days without. */
export function invitationLifetimeMs(env: Env): number {
  return inviteMailEnabled(env) ? INVITE_MAIL_TOKEN_TTL_MS : ORG_INVITATION_TTL_MS;
}

/**
 * .NET's WebUtility.UrlEncode: letters, digits and `-_.!*()` stay, a space is
 * `+`, everything else is `%XX` in uppercase over UTF-8. encodeURIComponent
 * leaves `~` and `'` as they are too, so those two are encoded here.
 */
export function dotnetUrlEncode(value: string): string {
  return encodeURIComponent(value)
    .replace(/[~']/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)
    .replace(/%20/g, '+');
}

/** The invitation link, Bitwarden's format exactly; `origin` is this server's, which is its web vault's. */
export function invitationUrl(
  origin: string,
  invitation: Pick<OrganizationInvitation, 'organizationId' | 'id' | 'email' | 'token'>,
  organizationName: string,
  hasAccount: boolean
): string {
  const query = [
    `organizationId=${invitation.organizationId}`,
    `organizationUserId=${invitation.id}`,
    `email=${dotnetUrlEncode(invitation.email)}`,
    `organizationName=${dotnetUrlEncode(organizationName)}`,
    `token=${dotnetUrlEncode(invitation.token)}`,
    'initOrganization=False',
    `orgUserHasExistingUser=${hasAccount ? 'True' : 'False'}`,
  ];
  return `${origin.replace(/\/+$/, '')}/#/accept-organization?${query.join('&')}`;
}

/**
 * GetUpdatedInviteTemplateInfo's Free-plan pair (HandlebarsMailService.cs
 * lines 800-833). NodeWarden organizations report no planType, which an
 * official client reads as Free (0), so these are the ones that match.
 */
export function invitationSubject(hasAccount: boolean): string {
  return hasAccount ? 'You have been invited to a Bitwarden vault' : 'You have been invited to Bitwarden Password Manager';
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

export interface InvitationMail {
  to: string;
  from: { email: string; name: string };
  subject: string;
  html: string;
  text: string;
}

export function composeInvitationMail(
  env: Env,
  origin: string,
  invitation: Pick<OrganizationInvitation, 'organizationId' | 'id' | 'email' | 'token'>,
  organizationName: string,
  hasAccount: boolean
): InvitationMail {
  const url = invitationUrl(origin, invitation, organizationName, hasAccount);
  const action = hasAccount ? 'Accept invitation' : 'Finish account setup';
  const lead = `${organizationName} invited you to join their organization.`;
  const lasts = 'The invitation expires in 5 days.';
  return {
    to: invitation.email,
    from: { email: String(env.INVITE_MAIL_FROM ?? '').trim(), name: 'NodeWarden' },
    subject: invitationSubject(hasAccount),
    text: `${lead}\n\n${action}: ${url}\n\n${lasts}\n`,
    html: `<p>${escapeHtml(lead)}</p><p><a href="${escapeHtml(url)}">${action}</a></p><p>${lasts}</p>`,
  };
}

/**
 * Sends one invitation email. Throws when the binding or the From address is
 * missing, or when the service refuses: the caller undoes the invitation, so
 * an invitation whose token reached nobody is never left behind.
 */
export async function sendInvitationMail(
  env: Env,
  origin: string,
  invitation: Pick<OrganizationInvitation, 'organizationId' | 'id' | 'email' | 'token'>,
  organizationName: string,
  hasAccount: boolean
): Promise<void> {
  if (!env.EMAIL || !String(env.INVITE_MAIL_FROM ?? '').trim()) {
    throw new Error('INVITE_MAIL is on, but the EMAIL binding or INVITE_MAIL_FROM is not set');
  }
  await env.EMAIL.send(composeInvitationMail(env, origin, invitation, organizationName, hasAccount));
}
```

- [ ] **Step 4: Send it from invite and re-invite**

In `src/handlers/organization-users.ts`, replace:

```ts
import { inviteMailEnabled } from '../services/invite-mail';
```

with:

```ts
import { invitationLifetimeMs, inviteMailEnabled, sendInvitationMail } from '../services/invite-mail';
```

In `handleInviteOrganizationUsers`, replace:

```ts
  const rows = await loadOrgUserRows(env, orgId);
  const nowMs = Date.now();
```

with:

```ts
  const rows = await loadOrgUserRows(env, orgId);
  const nowMs = Date.now();
  const organizationName = (await getOrganization(env.DB, orgId))?.name ?? '';
  const origin = new URL(request.url).origin;
```

Replace:

```ts
      token: newInvitationToken(),
      expiresAt: invitationExpiry(nowMs),
      createdAt: now,
      updatedAt: now,
    };
    await saveOrganizationInvitation(env.DB, invitation);
```

with:

```ts
      token: newInvitationToken(),
      expiresAt: invitationExpiry(nowMs, invitationLifetimeMs(env)),
      createdAt: now,
      updatedAt: now,
    };
    await saveOrganizationInvitation(env.DB, invitation);
    if (inviteMailEnabled(env)) {
      try {
        await sendInvitationMail(env, origin, invitation, organizationName, !!account);
      } catch (error) {
        // The token reached nobody: the invitation goes, and the inviter is
        // told. The error names no token and no link.
        console.error('An invitation email could not be sent:', error instanceof Error ? error.message : String(error));
        await deleteOrganizationInvitation(env.DB, invitation.id);
        return errorResponse('The invitation email could not be sent', 400);
      }
    }
```

In `handleReinviteOrganizationUser`, replace:

```ts
  const nowMs = Date.now();
  invitation.token = newInvitationToken();
  invitation.expiresAt = invitationExpiry(nowMs);
  invitation.updatedAt = new Date(nowMs).toISOString();
  await saveOrganizationInvitation(env.DB, invitation);
```

with:

```ts
  const nowMs = Date.now();
  const previous = { token: invitation.token, expiresAt: invitation.expiresAt, updatedAt: invitation.updatedAt };
  invitation.token = newInvitationToken();
  invitation.expiresAt = invitationExpiry(nowMs, invitationLifetimeMs(env));
  invitation.updatedAt = new Date(nowMs).toISOString();
  await saveOrganizationInvitation(env.DB, invitation);
  if (inviteMailEnabled(env)) {
    try {
      const organizationName = (await getOrganization(env.DB, orgId))?.name ?? '';
      await sendInvitationMail(env, new URL(request.url).origin, invitation, organizationName, !!invitee);
    } catch (error) {
      // The new token reached nobody: the old one stands, as before the call.
      console.error('An invitation email could not be sent:', error instanceof Error ? error.message : String(error));
      Object.assign(invitation, previous);
      await saveOrganizationInvitation(env.DB, invitation);
      return errorResponse('The invitation email could not be sent', 400);
    }
  }
```

- [ ] **Step 5: Document the binding in `wrangler.toml`**

Append to `wrangler.toml`:

```toml

# Organization invitation email (INVITE_MAIL; see src/services/invite-mail.ts).
# Off unless INVITE_MAIL is set: an invited account is then accepted at once,
# as Vaultwarden does with mail disabled. To turn it on, onboard a domain with
# `pnpm exec wrangler email sending enable <domain>`, then uncomment:
#
# [vars]
# INVITE_MAIL = "1"
# INVITE_MAIL_FROM = "vault@<domain>"
#
# [[send_email]]
# name = "EMAIL"
```

Run the LF checks on every file touched.

- [ ] **Step 6: Run the tests to see them pass**

Run: `pnpm run test:sharing 2>&1 | grep -E "^# (pass|fail)"`
Expected: `# fail 0`. NW3's `INVITE_MAIL on` tests now also send through the fake sender, and still pass.

Run: `grep -rn "console\.\(log\|info\|warn\|error\)" src/services/invite-mail.ts src/handlers/organization-users.ts`
Expected: only the two `An invitation email could not be sent:` lines. Each carries the error's message alone, never a token or a link.

- [ ] **Step 7: Verify the whole NodeWarden suite**

Run each of the eight commands in Global Constraints › NodeWarden › Verify per task.
Expected: `# fail 0` everywhere, and `tsc` silent.

- [ ] **Step 8: Commit**

```bash
git add src/services/invite-mail.ts src/services/org-user-model.ts src/types/index.ts src/handlers/organization-users.ts wrangler.toml scripts/support/sharing-harness.ts scripts/sharing-invite-mail.test.ts
git commit -F - <<'EOF'
With invitation email on, an invite sends Bitwarden's invitation email

INVITE_MAIL=1 makes POST /organizations/{id}/users/invite and
/reinvite send the invitation email official servers send, through
Cloudflare Email Service's Workers binding (EMAIL) from
INVITE_MAIL_FROM. NodeWarden sent no email before, so this is its
first sender. The binding and the vars are documented, commented out,
in wrangler.toml.

The link is Bitwarden's byte for byte where Bitwarden's is fixed:
{vault}/#/accept-organization?organizationId&organizationUserId&email&
organizationName&token&initOrganization&orgUserHasExistingUser, the
values WebUtility.UrlEncode'd and the booleans as C# prints them, so
the official web vault and Deskwarden's paste-a-link read it. The
subjects are Bitwarden's Free-plan pair, since NodeWarden
organizations report no plan. The token is the invitation's own, which
POST .../users/{ouId}/accept checks, and lasts Bitwarden's 120 hours.

An email that cannot be sent removes the new invitation, or restores
the old token on a re-invite, and the inviter is told. Nothing logs
the link or the token.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Deskwarden tasks

All paths below are under `E:\Personal\node-bitwarden\deskwarden` unless they say otherwise.

### Task DW1: Capabilities replace dialects, and the sharer shares on Bitwarden's routes

**Files:**
- Modify: `src/rest/sharing_api.rs:1-15` (module doc), `:66-78` (`query_value`), `:136-153` (`AccountKey` doc, `InviteOutcome`), `:254-286` (`KeyRoute`, `key_route`), `:385-438` (`member_key_by_email`, `invite_member`), `:464-472` (after `remove_user`), and tests `:665-771`
- Modify: `src/rest/sharing.rs`:
  - `:28-60` (module doc, imports, reasons);
  - `:149-162` (`SharingView`);
  - `:444-471` (`Dialect`, `Probe`, `Account`);
  - `:557-585` (`Held`);
  - `:638-654` (after `call`);
  - `:746-756` (`view` head);
  - `:824-876` (`ensure_probe`, `require_available`);
  - `:909-916` (`refresh` head);
  - `:962-989` (`Ctx`);
  - `:1115-1205` (`org_is_mine`, `my_org_id`, `forget_cached_org`);
  - `:1216-1225` (`fetch_roster`);
  - `:1244-1265` (`context_if_any`);
  - `:1296-1321` (`ensure_collection`);
  - `:1347-1414` (`ensure_member`);
  - `:1546-1653` (`share`);
  - `:1845-1852` (`move_back`'s orphan delete);
  - `:1959-1973` (`purge_org_copy`);
  - `:2303-2334` (`reinvite`);
  - `:2797` (`mint_invite_code`'s `my_org_id`);
  - `:694-704` (`note_directory`);
  - and tests from `:2936`.
- Modify: `src/rest/api.rs:1475-1522` (`fetch_organization_members`)
- Modify: `src/rest/organizations.rs` tests `:1560-1565`, `:1832-1855`
- Modify: `src/vault_api.rs:1102` (test), `src/vault_window/mod.rs:41915` (test), `src/vault_window/share_ui.rs:3149,3158` (test)

**Interfaces:**
- Consumes (NodeWarden, for DW5): roster `status: 1` right after an invite when `INVITE_MAIL` is off (NW3); `DELETE /api/ciphers/{id}` permanent (NW2).
- Produces:
  - In `crate::rest::sharing_api`:
    - `pub fn invite_user(client: &RestClient, session: &mut Session, org: &str, email: &str, collection_id: &str) -> Result<(), RestError>`
    - `pub fn delete_cipher(client: &RestClient, session: &mut Session, id: &str) -> Result<(), RestError>`. A 404 is `Ok`.
    - Removed: `member_key_by_email`, `invite_member`, `InviteOutcome`, `KeyRoute`, `key_route`, `query_value`.
  - In `crate::rest::sharing`:
    - `pub(crate) struct Capabilities { pub(crate) loaded: bool, pub(crate) too_old: bool, pub(crate) roster_readable: Option<bool>, pub(crate) accepts_on_invite: Option<bool>, pub(crate) creates_orgs: Option<bool>, pub(crate) server_name: Option<String> }`. It derives `Debug, Clone, Default, PartialEq, Eq`. DW2 fills `creates_orgs` and `server_name`; DW4 fills `roster_readable`.
    - `Held.capabilities: Capabilities` replaces `Held.probe`.
    - `Sharing::ensure_loaded(&self) -> Result<(), SharingError>` replaces `ensure_probe`.
    - `Sharing::route<T>(&self, request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>) -> Result<T, SharingError>`.
    - `Sharing::my_org_id(&self, profile: &Profile, account: &Account) -> Result<Option<String>, SharingError>`. It was `Option<String>`.
    - `Ctx.roster: Vec<Member>`, `Ctx.own_ou_id: Option<String>`, and `Ctx::member(&self, email: &str) -> Option<&Member>`.
    - `Sharing::ensure_member(&self, ctx: &mut Ctx, email: &str, collection: &str) -> Result<ShareResult, SharingError>`. It now takes `&mut Ctx`.
    - `fn roster_says_mine(roster: &[Member], account_id: &str) -> bool` and `fn own_row<'a>(roster: &'a [Member], account_id: &str) -> Option<&'a Member>`. DW4 reads the first.
    - Removed: `REASON_NOT_NODEWARDEN`, `DIALECT_NODEWARDEN`, `Dialect`, `Probe`, `SharingView::dialect`, `org_is_mine`.
- Test count: −4 (6135 → 6131).

- [ ] **Step 1: Save the two scripts**

If you have not already, save `$TEMP/dw_crlf.py` and `$TEMP/dw_cut.py` exactly as Global Constraints gives them, with the Write tool. Then `mkdir -p "$TEMP/dw"`.

- [ ] **Step 2: Write the failing tests in `sharing_api.rs`**

Write `$TEMP/dw/dw1-api-tests.txt` with the Write tool:

```rust
    /// **The invite is Bitwarden's, body and all** (official-path spec §3.2):
    /// one email, the User type, read-only access to one collection, and the
    /// rest of `OrganizationUserInviteRequestModel` spelled out. Answered with
    /// an empty 200 on every server, which is a success.
    #[test]
    fn an_invite_is_the_official_body_on_the_official_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let invited = server
            .mock("POST", format!("/api/organizations/{ORG}/users/invite").as_str())
            .match_body(Matcher::Json(serde_json::json!({
                "emails": ["anna@example.com"],
                "type": 2,
                "collections": [{ "id": "c-anna", "readOnly": true, "hidePasswords": false, "manage": false }],
                "groups": [],
                "permissions": null,
                "accessSecretsManager": false
            })))
            .with_body("")
            .expect(1)
            .create();
        invite_user(&client, &mut session, ORG, "anna@example.com", "c-anna").expect("invited");
        invited.assert();
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(invite_user(&client, &mut session, "../x", "anna@example.com", "c"), Err(RestError::UnsafeId));
        any.assert();
    }

    /// **A record leaves the org by Bitwarden's permanent delete**,
    /// `DELETE /api/ciphers/{id}` -- not NodeWarden's `/delete` suffix, which
    /// official servers and Vaultwarden do not have. Already gone is done; a
    /// refusal is not.
    #[test]
    fn a_purge_is_the_official_delete_and_a_404_is_done() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let deleted = server.mock("DELETE", "/api/ciphers/c1").with_status(204).expect(1).create();
        delete_cipher(&client, &mut session, "c1").expect("deleted");
        deleted.assert();
        server.mock("DELETE", "/api/ciphers/gone").with_status(404).create();
        delete_cipher(&client, &mut session, "gone").expect("already gone is done");
        server.mock("DELETE", "/api/ciphers/theirs").with_status(403).create();
        assert_eq!(delete_cipher(&client, &mut session, "theirs"), Err(RestError::Status(403)));
    }

```

Then run:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '    /// Direct membership (spec 7.2): the key lookup answers both "does the' '    /// "Already a member" and "already confirmed" are the two-PC race (spec' "$TEMP/dw/dw1-api-tests.txt"
```

Expected: one line, `src/rest/sharing_api.rs replaced lines … to … with …`. Any other output means a marker did not match exactly one line: stop and compare it with the file. This deletes `an_accounts_key_is_looked_up_by_email_and_a_404_is_no_account`, `the_key_route_tells_an_answer_without_a_key_from_no_route` and `an_invite_carries_read_only_access_and_says_which_branch_the_server_took`.

With the Edit tool, in the same file, replace:

```rust
    /// "Already a member" and "already confirmed" are the two-PC race (spec
    /// 13): success, not an error.
    #[test]
    fn a_race_another_pc_already_won_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .with_status(400)
            .with_body(r#"{"error":"User is already a member","error_description":"User is already a member"}"#)
            .create();
        assert_eq!(
            invite_member(&client, &mut session, ORG, "anna@example.com", "c", None).expect("a race"),
            InviteOutcome::AlreadyThere
        );
        server
```

with:

```rust
    /// "Already confirmed" and a removal already made are the two-PC race
    /// (spec 13): success, not an error.
    #[test]
    fn a_race_another_pc_already_won_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing_api.rs`
Expected: `src/rest/sharing_api.rs 0`

- [ ] **Step 3: Run them to see them fail**

Run: `cargo test --lib -j 2 rest::sharing_api 2>&1 | tail -5`
Expected: a compile error, ``cannot find function `invite_user` in this scope`` and ``cannot find function `delete_cipher` ``.

- [ ] **Step 4: Replace the NodeWarden-only routes with the official ones**

In `src/rest/sharing_api.rs`, replace:

```rust
//! **NodeWarden's sharing routes**, one typed function each -- the NodeWarden
//! column of spec §7.3, with the request and answer shapes of §10 and of the
//! routes NodeWarden already has (`/members/*`, `/invitations/*`,
//! `/users/{ouId}/registration-invite`).
```

with:

```rust
//! **The sharing routes**, one typed function each -- Bitwarden's own, the
//! official column of spec §7.3, which every server answers
//! (`docs/superpowers/specs/2026-10-08-official-path-sharing-design.md`).
//! Two NodeWarden routes remain, each said where it is: the recipient's
//! invitation list (until the paste-a-link accept replaces it) and the
//! administrator's registration code.
```

Delete `query_value` (nothing else calls it once `member_key_by_email` goes). Replace:

```rust
/// Percent-encodes a query value. An email carries `@` and may carry `+`,
/// and a `+` left bare is a space to the server.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

```

with nothing.

Replace:

```rust
/// An account's id and public key, looked up by email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountKey {
    pub user_id: String,
    /// SPKI DER.
    pub public_key: Vec<u8>,
}

/// Which branch NodeWarden's invite took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InviteOutcome {
    /// An existing account became a member at once (direct membership).
    Member,
    /// No account: a pending invitation, with its own id.
    Invitation { id: String },
    /// Already a member or already invited -- another PC got there first.
    AlreadyThere,
}
```

with:

```rust
/// An account's id and public key: what a confirm wraps the org key to, and
/// what a pin records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountKey {
    pub user_id: String,
    /// SPKI DER.
    pub public_key: Vec<u8>,
}
```

Delete `KeyRoute` and `key_route`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '/// What `GET /api/users/{userId}/public-key` said, for the dialect probe'"'"'s' '/// `POST /api/organizations`, the official body. Returns the new org'"'"'s id.'
```

Expected: one `… replaced lines … with 0` line.

Write `$TEMP/dw/dw1-api-invite.txt`:

```rust
/// `POST /api/organizations/{id}/users/invite` -- Bitwarden's invite, with
/// the official body of the official-path spec §3.2: one email, the User
/// type, read-only access to the person's own collection, no groups, no
/// custom permissions, no Secrets Manager.
///
/// **Answered with an empty 200 on every server**, and the answer says
/// nothing: not the new org-user id, not whether the address has an account,
/// not whether the server accepted it at once (Vaultwarden and NodeWarden
/// with mail off do, for an existing account). The caller reads the roster
/// after it for all three. An address already in the org is skipped by the
/// server, silently, so a two-PC race is a success here too.
pub fn invite_user(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
    collection_id: &str,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/invite", safe(org)?);
    let json = json!({
        "emails": [email],
        "type": 2,
        "collections": [{ "id": collection_id, "readOnly": true, "hidePasswords": false, "manage": false }],
        "groups": [],
        "permissions": null,
        "accessSecretsManager": false,
    });
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json))).map(|_| ())
}

```

Then run:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '/// `GET /api/organizations/{id}/members/public-key?email=` -- whether the' '/// `POST /api/organizations/{id}/users/{ouId}/confirm {key}`. "Already' "$TEMP/dw/dw1-api-invite.txt"
```

Expected: one `… replaced lines …` line.

Replace:

```rust
/// `DELETE /api/organizations/{id}/users/{ouId}` -- a member or an invitation.
/// Already gone is done.
pub fn remove_user(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}
```

with:

```rust
/// `DELETE /api/organizations/{id}/users/{ouId}` -- a member or an invitation.
/// Already gone is done.
pub fn remove_user(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `DELETE /api/ciphers/{id}` -- Bitwarden's permanent delete, the second
/// half of moving a record back (base spec §7.3: "`POST /api/ciphers`
/// (personal) then `DELETE /api/ciphers/{orgId}`" on every server).
///
/// **Not `RestClient::hard_delete_cipher`**, which sends NodeWarden's own
/// `DELETE /api/ciphers/{id}/delete`: official servers and Vaultwarden have
/// no such route, and the 404 they answered was read as "already gone", so
/// the org copy stayed in the org, filed nowhere. NodeWarden's bare route was
/// a soft delete for a live cipher until it was made Bitwarden's.
///
/// A 404 is done: the org copy is already gone. Matched on the status
/// itself -- a 403 is not "gone", and must stay an error so the move back is
/// retried rather than dropped.
pub fn delete_cipher(client: &RestClient, session: &mut Session, id: &str) -> Result<(), RestError> {
    let path = format!("/api/ciphers/{}", safe(id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing_api.rs`
Expected: `src/rest/sharing_api.rs 0`

- [ ] **Step 5: Read the roster on `/users` only**

In `src/rest/api.rs`, replace:

```rust
    /// # `/users` first, `/members` when it is not there
    ///
    /// `/users?includeCollections=true` is the official route (stock
    /// Bitwarden, and NodeWarden from slice 1): it answers invitations
    /// alongside accepted and confirmed rows, and each collection entry
    /// carries its own `readOnly` flag. `/members` is NodeWarden's older
    /// route, from before slice 1 -- its rows ARE the account (no `userId`)
    /// and it has no invitations at all. A 404, a 405 (method not allowed --
    /// some proxies answer a missing route that way) or a 501 (not
    /// implemented) from the first is read as "this server does not have
    /// `/users`" and falls back; any other failure -- 403 from a member who
    /// may not read the roster, a transport error, a body that is not a list
    /// -- is *not* retried on the second spelling, because the caller treats
    /// it identically to a 404 anyway: both become
    /// [`crate::rest::organizations::Roster::Unknown`], a state the UI
    /// already has to draw for "not fetched yet", and a needless second round
    /// trip would only slow that down.
```

with:

```rust
    /// # `/users`, and only `/users`
    ///
    /// `/users?includeCollections=true` is the official route (stock
    /// Bitwarden, Vaultwarden, and NodeWarden from slice 1): it answers
    /// invitations alongside accepted and confirmed rows, and each collection
    /// entry carries its own `readOnly` flag. NodeWarden's older `/members`
    /// is no longer tried when it fails: sharing is on Bitwarden's routes
    /// only (`docs/superpowers/specs/2026-10-08-official-path-sharing-design.md`),
    /// and a server without `/users` is one sharing calls too old. Every
    /// failure -- 404, 403 from a member who may not read the roster, a
    /// transport error -- becomes
    /// [`crate::rest::organizations::Roster::Unknown`] for the directory, a
    /// state the UI already draws for "not fetched yet".
```

Then replace:

```rust
        // `/users?includeCollections=true` first: the official route, which
        // NodeWarden answers from slice 1 with invitations and per-collection
        // read-only flags. `/members` only when that is not there (404, 405, or
        // the 501 a server without it may send) -- an older NodeWarden.
        let users = self.organization_url(id, "/users?includeCollections=true")?;
        let answer = self.refreshing(session, |session| {
            self.value_from(self.bearer(self.sync_agent.get(&users), session).call())
        });
        match answer {
            Err(RestError::Status(404 | 405 | 501)) => {
                let url = self.organization_url(id, "/members")?;
                self.refreshing(session, |session| {
                    self.value_from(self.bearer(self.sync_agent.get(&url), session).call())
                })
            }
            other => other,
        }
```

with:

```rust
        let users = self.organization_url(id, "/users?includeCollections=true")?;
        self.refreshing(session, |session| {
            self.value_from(self.bearer(self.sync_agent.get(&users), session).call())
        })
```

In `src/rest/organizations.rs`, the degrade test asked for `/members` and got there only through the fallback. Replace:

```rust
        let members = server
            .mock("GET", format!("/api/organizations/{ORG}/members").as_str())
            .with_status(404)
```

with:

```rust
        let members = server
            .mock("GET", format!("/api/organizations/{ORG}/users?includeCollections=true").as_str())
            .with_status(404)
```

Replace the last test of the file:

```rust
    /// **The roster route: `/users` first, `/members` when it is not there.**
    #[test]
    fn the_roster_reads_users_then_falls_back_to_members() {
        let mut server = crate::test_http::server();
        server.mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let mut session = client.password_grant("a@b.c", "HASH", &device).expect("grant");
        let users = server
            .mock("GET", format!("/api/organizations/{ORG}/users?includeCollections=true").as_str())
            .with_status(404)
            .expect(1)
            .create();
        let members = server
            .mock("GET", format!("/api/organizations/{ORG}/members").as_str())
            .with_body(r#"{"data":[]}"#)
            .expect(1)
            .create();
        client.fetch_organization_members(&mut session, ORG).expect("the fallback answered");
        users.assert();
        members.assert();
    }
```

with:

```rust
    /// **The roster route is `/users` and nothing else**: NodeWarden's older
    /// `/members` is never asked, even when `/users` is missing -- sharing is
    /// on Bitwarden's routes only, and that server is one it calls too old.
    #[test]
    fn the_roster_reads_users_and_never_the_old_members_route() {
        let mut server = crate::test_http::server();
        server.mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let mut session = client.password_grant("a@b.c", "HASH", &device).expect("grant");
        let users = server
            .mock("GET", format!("/api/organizations/{ORG}/users?includeCollections=true").as_str())
            .with_status(404)
            .expect(1)
            .create();
        let members = server.mock("GET", format!("/api/organizations/{ORG}/members").as_str()).expect(0).create();
        assert_eq!(client.fetch_organization_members(&mut session, ORG), Err(RestError::Status(404)));
        users.assert();
        members.assert();
    }
```

If `RestError` is not in scope in that test module, `cargo check` says so. Then add `use crate::rest::api::RestError;` at the top of the module's `use` lines.

Run: `python "$TEMP/dw_crlf.py" src/rest/api.rs src/rest/organizations.rs`
Expected: both `0`.

- [ ] **Step 6: The engine's doc, imports and reasons**

In `src/rest/sharing.rs`, replace:

```rust
//! # NodeWarden only, in this build
//!
//! The owner's decision: slices 2 and 3 target NodeWarden. The probe tells
//! the dialects apart (spec 7.1), and on anything but NodeWarden with the
//! slice-1 routes the view says `available: false` and every write refuses.
```

with:

```rust
//! # One path, on every server
//!
//! `docs/superpowers/specs/2026-10-08-official-path-sharing-design.md`
//! amends the design: NodeWarden, Vaultwarden and official Bitwarden are all
//! shared on through Bitwarden's own routes and bodies, and nothing asks
//! which server this is. What differs between them is learned as
//! [`Capabilities`], each from an official route sharing needs anyway: a
//! route answering as missing turns sharing off (`server-too-old`), and a
//! roster read after an invite says whether the server accepted at once.
//! The recipient's invitation list is the one NodeWarden route left, until
//! the paste-a-link accept replaces it.
```

Replace:

```rust
use crate::rest::sharing_api::{self, IncomingInvitation, KeyRoute};
```

with:

```rust
use crate::rest::sharing_api::{self, IncomingInvitation};
```

Replace:

```rust
/// Why sharing is not available. Spec 11.3's reasons, plus `not-nodewarden`:
/// the owner's decision leaves official servers for a later slice, and the
/// spec's list had no word for that.
pub const REASON_BW_SERVE: &str = "bw-serve";
pub const REASON_NOT_NODEWARDEN: &str = "not-nodewarden";
pub const REASON_TOO_OLD: &str = "server-too-old";
pub const REASON_NOT_LOADED: &str = "not-loaded";

/// The one dialect this build shares on.
pub const DIALECT_NODEWARDEN: &str = "nodewarden";
```

with:

```rust
/// Why sharing is not available (spec 11.3). `server-too-old` is a route
/// sharing needs answering as missing (official-path spec §2). There is no
/// "not NodeWarden" any more: every server shares on the one path.
pub const REASON_BW_SERVE: &str = "bw-serve";
pub const REASON_TOO_OLD: &str = "server-too-old";
pub const REASON_NOT_LOADED: &str = "not-loaded";
```

In `SharingView`, replace:

```rust
    pub available: bool,
    pub reason: Option<String>,
    pub dialect: Option<String>,
```

with:

```rust
    pub available: bool,
    pub reason: Option<String>,
```

- [ ] **Step 7: `Capabilities` replaces `Dialect` and `Probe`**

Replace:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    NodeWarden,
    Official,
}

/// The probe's answer, once per engine (spec 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probe {
    pub(crate) dialect: Dialect,
    /// NodeWarden answers `GET /api/users/{id}/public-key`: the slice-1 routes
    /// (read-only collections, `/share`, `/users`) are there.
    pub(crate) slice_one: bool,
}
```

with:

```rust
/// **What this server does, as sharing has learned it this session**
/// (official-path spec §2) -- in place of a dialect probe. Each fact comes
/// from an official route sharing needs anyway; none is asked for its own
/// sake, and none names a server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    /// The first pass has read the account: the view may say "available".
    pub(crate) loaded: bool,
    /// A route sharing needs answered as missing -- 405 or 501 from one of
    /// them ([`Sharing::route`]), or 404 from the sharer's own org roster
    /// ([`Sharing::fetch_roster`]). Sharing is off with [`REASON_TOO_OLD`]
    /// until the next sign-in.
    pub(crate) too_old: bool,
    /// A recipient's `GET …/users/mini-details` answered 200 (`Some(true)`) or
    /// 403/404 (`Some(false)`). Whether "Shared by" can be verified.
    pub(crate) roster_readable: Option<bool>,
    /// The roster read after the last invite showed the person Accepted
    /// (`Some(true)`, Vaultwarden and NodeWarden with mail off) or Invited
    /// (`Some(false)`). A share then goes straight to confirm, or waits.
    pub(crate) accepts_on_invite: Option<bool>,
    /// `POST /organizations` was refused (`Some(false)`, official
    /// self-hosted): the user creates the sharing org in the web vault.
    pub(crate) creates_orgs: Option<bool>,
    /// `GET /config`'s `server.name`, read once. Decides the
    /// roster-visibility warning.
    pub(crate) server_name: Option<String>,
}
```

In `Account`, replace:

```rust
    /// The profile carries a private key, so the account has a key pair and
    /// the server holds its public half. What lets the probe read a 404 from
    /// the own-key route as "no route" -- see [`Sharing::ensure_probe`].
    pub(crate) keyed: bool,
```

with:

```rust
    /// The profile carries a private key, so the account has a key pair and
    /// the server holds its public half. Every owner's write asks this before
    /// its first key is touched -- see [`NO_KEY_PAIR`].
    pub(crate) keyed: bool,
```

In `Held`, replace:

```rust
pub(crate) struct Held {
    pub(crate) probe: Option<Probe>,
```

with:

```rust
pub(crate) struct Held {
    pub(crate) capabilities: Capabilities,
```

and in its `Debug`, replace:

```rust
            .field("probe", &self.probe)
```

with:

```rust
            .field("capabilities", &self.capabilities)
```

- [ ] **Step 8: `route`, the view, loading and the pass**

After `call`, add `route`. Replace:

```rust
    /// `GET /api/accounts/profile`, and what it says about the account.
    pub(crate) fn profile(&self) -> Result<Profile, SharingError> {
```

with:

```rust
    /// [`Self::call`], for a route sharing needs (official-path spec §2): a
    /// 405 or 501 is the server saying it has no such route, so sharing is
    /// off for the session with [`REASON_TOO_OLD`] and the answer is
    /// [`SharingError::Unavailable`]. Only the routes that are sharing's
    /// learn it this way -- a `/config` or a cipher read answering 405 says
    /// nothing about sharing, and goes through [`Self::call`].
    ///
    /// A 404 is not read here: from `/share` or `/confirm` it is a record or
    /// a person that has gone. The one 404 that means "no route" is the
    /// sharer's own roster's, which [`Self::fetch_roster`] reads.
    pub(crate) fn route<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let answer = {
            let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            request(&self.client, &mut guard)
        };
        match answer {
            Ok(value) => Ok(value),
            Err(RestError::Status(405 | 501)) => {
                self.held().capabilities.too_old = true;
                Err(SharingError::Unavailable(REASON_TOO_OLD))
            }
            Err(RestError::Rejected { description, .. }) if !description.is_empty() => Err(SharingError::Refused(description)),
            Err(other) => Err(SharingError::Vault(crate::rest::backend::rest_error(other))),
        }
    }

    /// `GET /api/accounts/profile`, and what it says about the account.
    pub(crate) fn profile(&self) -> Result<Profile, SharingError> {
```

In `view`, replace:

```rust
        let held = self.held();
        let Some(probe) = held.probe else {
            return SharingView::unavailable(REASON_NOT_LOADED);
        };
        if probe.dialect != Dialect::NodeWarden {
            return SharingView::unavailable(REASON_NOT_NODEWARDEN);
        }
        if !probe.slice_one {
            return SharingView::unavailable(REASON_TOO_OLD);
        }
        let now = now_millis();
        let mut view = SharingView {
            available: true,
            dialect: Some(DIALECT_NODEWARDEN.to_string()),
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
```

with:

```rust
        let held = self.held();
        if !held.capabilities.loaded {
            return SharingView::unavailable(REASON_NOT_LOADED);
        }
        if held.capabilities.too_old {
            return SharingView::unavailable(REASON_TOO_OLD);
        }
        let now = now_millis();
        let mut view = SharingView {
            available: true,
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
```

Replace the whole of `ensure_probe` and `require_available`, from:

```rust
    /// **The probe**, once per engine: `GET /invitations/mine` tells
```

through the closing `}` of `require_available` (the line before `    /// The account's id, from the last profile, or read now.`), using `dw_cut`. Write `$TEMP/dw/dw1-loaded.txt`:

```rust
    /// **The first pass's one question**, once per engine: who the account
    /// is. Sharing is "not loaded" until it has an answer, and is then
    /// available on every server -- unless a route it needs answers as
    /// missing ([`Self::route`], [`Self::fetch_roster`]), which turns it off
    /// with [`REASON_TOO_OLD`] for the session.
    ///
    /// This replaces the dialect probe (official-path spec §2). The probe
    /// asked NodeWarden's `/invitations/mine` to tell the servers apart and
    /// turned sharing off everywhere else; nothing asks which server this is
    /// now. The profile is usually already held -- the whole read that called
    /// [`Self::note_directory`] filed it -- so a first pass costs no request.
    /// A transport failure is not remembered: the next pass asks again.
    pub(crate) fn ensure_loaded(&self) -> Result<(), SharingError> {
        if self.held().capabilities.loaded {
            return Ok(());
        }
        self.account_id()?;
        self.held().capabilities.loaded = true;
        Ok(())
    }

    /// Refuses unless sharing is loaded and no route it needs has answered
    /// as missing.
    pub(crate) fn require_available(&self) -> Result<(), SharingError> {
        self.ensure_loaded()?;
        if self.held().capabilities.too_old {
            return Err(SharingError::Unavailable(REASON_TOO_OLD));
        }
        Ok(())
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **The probe**, once per engine: `GET /invitations/mine` tells' '    /// The account'"'"'s id, from the last profile, or read now.' "$TEMP/dw/dw1-loaded.txt"
```

Expected: one `src/rest/sharing.rs replaced lines …` line.

In `refresh`, replace:

```rust
    /// **One pass** (spec 11.5): the probe, then the incoming invitations on a
    /// whole read or the timer, then every move back whose delete failed
```

with:

```rust
    /// **One pass** (spec 11.5): the account, then the incoming invitations on a
    /// whole read or the timer, then every move back whose delete failed
```

and replace:

```rust
    /// # Errors
    ///
    /// The probe could not be made; a later pass tries again.
    pub fn refresh(&self, trigger: Trigger) -> Result<SharingPass, SharingError> {
        let _one = self.one_at_a_time();
        let had_probe = self.held().probe.is_some();
        let probe = self.ensure_probe()?;
        let mut pass = SharingPass { changed: !had_probe, ..SharingPass::default() };
        if probe.dialect != Dialect::NodeWarden || !probe.slice_one {
            return Ok(pass);
        }
        if had_probe && matches!(trigger, Trigger::WholeRead | Trigger::Timer) {
```

with:

```rust
    /// # Errors
    ///
    /// The account could not be read; a later pass tries again.
    pub fn refresh(&self, trigger: Trigger) -> Result<SharingPass, SharingError> {
        let _one = self.one_at_a_time();
        let was_loaded = self.held().capabilities.loaded;
        self.ensure_loaded()?;
        let mut pass = SharingPass { changed: !was_loaded, ..SharingPass::default() };
        if self.held().capabilities.too_old {
            return Ok(pass);
        }
        if matches!(trigger, Trigger::WholeRead | Trigger::Timer) {
```

- [ ] **Step 9: `Ctx` carries the roster and the sharer's own org-user id**

Replace:

```rust
pub(crate) struct Ctx {
    pub(crate) account: Account,
    pub(crate) keys: Arc<VaultKeys>,
    pub(crate) org_id: String,
    pub(crate) org_key: SymmetricKey,
    pub(crate) collections: Vec<PersonCollection>,
}

impl Ctx {
```

with:

```rust
pub(crate) struct Ctx {
    pub(crate) account: Account,
    pub(crate) keys: Arc<VaultKeys>,
    pub(crate) org_id: String,
    pub(crate) org_key: SymmetricKey,
    pub(crate) collections: Vec<PersonCollection>,
    /// The org's roster, read at the start of the write (spec 13) and again
    /// after an invite, which is how the write learns the new person's
    /// org-user id and status (the invite answers nothing).
    pub(crate) roster: Vec<Member>,
    /// **This account's own org-user id** in that roster ([`own_row`]): what
    /// a person collection names as its manager. On NodeWarden it is the
    /// user id; on an official server and on Vaultwarden it is not, which is
    /// why it is read and never assumed. `None` when the roster does not list
    /// the account -- such a write cannot create a collection.
    pub(crate) own_ou_id: Option<String>,
}

impl Ctx {
    /// One person's row in [`Self::roster`], by email in any case.
    pub(crate) fn member(&self, email: &str) -> Option<&Member> {
        let email = normalise_email(email);
        self.roster.iter().find(|m| normalise_email(&m.email) == email)
    }

```

Run: `grep -n "Ctx {" src/rest/sharing.rs`
Expected: the `struct Ctx {` line, and the two constructions in `context_if_any` and `share`, which Steps 11 and 14 rewrite.

- [ ] **Step 10: "Mine" by the roster, not by `ownerEmail`**

Write `$TEMP/dw/dw1-mine.txt`:

```rust
/// **Owned by this account and named for it** -- what makes an org a
/// CANDIDATE for this account's own sharing org (spec 5.1). Not enough on its
/// own: another account can create "Shared by <me>" and make this account an
/// owner of it. [`Sharing::my_org_id`] asks the roster too.
fn owned_and_named(org: &crate::rest::sync::Organization, account_email: &str) -> bool {
    crate::rest::organizations::sharing_mark(org.name.as_deref().and_then(sharer_of), None, org.is_owner(), account_email)
        .is_some_and(|mark| mark.mine)
}

/// **The roster's word that a candidate org is this account's own**
/// (official-path spec §3.1): no OTHER account holds the owner role there.
/// It replaces NodeWarden's `ownerEmail` attestation, which no other server
/// sends. A planted org -- someone else's "Shared by <me>" that made this
/// account an owner -- still lists its maker as an owner, and fails here. A
/// pending invitation in the owner role counts as another owner too: it has
/// no account behind it yet, so it cannot be this one.
fn roster_says_mine(roster: &[Member], account_id: &str) -> bool {
    !roster.iter().any(|m| m.role == "owner" && m.user_id.as_deref() != Some(account_id))
}

/// This account's own row in a roster -- see [`Ctx::own_ou_id`].
fn own_row<'a>(roster: &'a [Member], account_id: &str) -> Option<&'a Member> {
    roster.iter().find(|m| m.user_id.as_deref() == Some(account_id))
}

impl Sharing {
    /// **This account's own sharing org among the profile's** -- the cached
    /// id while it still passes, else the first candidate that passes
    /// (official-path spec §3.1). A candidate is owned and named for this
    /// account ([`owned_and_named`]); it passes when this client CREATED it
    /// (the cached `sharing_org_created` id) or when its roster names no
    /// other owner ([`roster_says_mine`]), read fresh.
    ///
    /// **A roster that cannot be read is a refusal, not a "no"**
    /// ([`ROSTER_UNREADABLE`]): read as "not mine", a share would create a
    /// second sharing org beside the real one.
    ///
    /// **A cached id that fails is dropped** from `sharing_state.json`, with
    /// a plain warning naming nothing. **Only an id the profile LISTS can
    /// fail.** One it does not list is kept: the profile may be older than
    /// the id (`a_cached_id_the_profile_does_not_list_yet_is_kept`).
    ///
    /// # Errors
    ///
    /// The roster of a candidate could not be read.
    pub(crate) fn my_org_id(&self, profile: &Profile, account: &Account) -> Result<Option<String>, SharingError> {
        self.choose_org(profile, account, &mut |id| self.attested_mine(id, &account.id))
    }

    /// [`Self::my_org_id`] with the roster question asked by `attest` -- a
    /// fresh read for a write, the whole read's own rosters for
    /// [`Self::note_directory`].
    fn choose_org(
        &self,
        profile: &Profile,
        account: &Account,
        attest: &mut dyn FnMut(&str) -> Result<bool, SharingError>,
    ) -> Result<Option<String>, SharingError> {
        let (cached, created) = self.setup().state_path.map_or((None, false), |path| {
            let me = crate::sharing_state::SharingState::load(&path).account(&account.id);
            (me.sharing_org_id, me.sharing_org_created)
        });
        if let Some(cached) = cached.as_deref() {
            if let Some(listed) = profile.organizations.iter().find(|o| o.id == cached) {
                if owned_and_named(listed, &account.email) && (created || attest(cached)?) {
                    return Ok(Some(cached.to_string()));
                }
                self.forget_cached_org(&account.id, cached);
            }
        }
        for org in profile.organizations.iter().filter(|o| Some(o.id.as_str()) != cached.as_deref()) {
            if owned_and_named(org, &account.email) && attest(&org.id)? {
                return Ok(Some(org.id.clone()));
            }
        }
        Ok(None)
    }

    /// One candidate's roster, read now, and its word ([`roster_says_mine`]).
    /// Through [`Self::call`], not [`Self::route`]: this is a question about
    /// an org this account may not even be the sharer of, and its failure
    /// says nothing about the server's routes.
    fn attested_mine(&self, org: &str, account_id: &str) -> Result<bool, SharingError> {
        let answer = self.call(|client, auth| client.fetch_organization_members(&mut auth.session, org))?;
        let roster = crate::rest::organizations::map_members(&answer)
            .ok_or_else(|| SharingError::Refused(ROSTER_UNREADABLE.to_string()))?;
        Ok(roster_says_mine(&roster, account_id))
    }

    /// Drops a cached sharing org id that failed [`Self::my_org_id`] -- only
    /// if the file still holds that id, so a share that chose another in the
    /// meantime is not undone.
    fn forget_cached_org(&self, account: &str, id: &str) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        log::warn!("the cached sharing organisation is not this account's own, so it is no longer used");
        let dropped = crate::sharing_state::update(&path, |state| {
            let me = state.account_mut(account);
            if me.sharing_org_id.as_deref() == Some(id) {
                me.sharing_org_id = None;
                me.sharing_org_created = false;
            }
        });
        if let Err(e) = dropped {
            log::warn!("the cached sharing organisation could not be dropped: {e}");
        }
    }

```

Then run:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '/// **Whether `org` is this account'"'"'s OWN sharing org** -- the one question' '    /// The org'"'"'s person collections, decrypted where they are ciphertext.' "$TEMP/dw/dw1-mine.txt"
```

Expected: one `… replaced lines …` line. This deletes `org_is_mine` and the old `my_org_id` and `forget_cached_org`, and opens the `impl Sharing {` that `collections_of` was in.

In `note_directory`, replace:

```rust
        let account = self.held().account.clone();
        let chosen = profile.zip(account).and_then(|(profile, account)| self.my_org_id(profile, &account));
```

with:

```rust
        let account = self.held().account.clone();
        let previous_id = self.held().my_org.as_ref().map(|o| o.id.clone());
        // The roster this whole read already fetched is the attestation
        // ([`Self::choose_org`]); an org whose roster it could not fetch keeps
        // the word it had when it was last held. No request is made here.
        let chosen = profile.zip(account).and_then(|(profile, account)| {
            self.choose_org(profile, &account, &mut |id| {
                Ok(match directory.roster_of(id) {
                    Roster::Known(members) => roster_says_mine(members, &account.id),
                    Roster::Unknown => previous_id.as_deref() == Some(id),
                })
            })
            .ok()
            .flatten()
        });
```

Also replace the comment above it:

```rust
        // **The writes' own rule** ([`Self::my_org_id`]), so the confirm pass
        // never works on an org the writes would refuse: a planted org owned
        // and named for this account, attested to someone else, is not held
        // as ours. No profile, no attestation to check: nothing is held.
```

with:

```rust
        // **The writes' own rule** ([`Self::my_org_id`]), so the confirm pass
        // never works on an org the writes would refuse: a planted org owned
        // and named for this account, whose roster names another owner, is
        // not held as ours. No profile, nothing to check: nothing is held.
```

In `mint_invite_code`, replace:

```rust
        let Some(org) = self.my_org_id(&profile, &account) else {
```

with:

```rust
        let Some(org) = self.my_org_id(&profile, &account)? else {
```

- [ ] **Step 11: The roster read that learns `too_old`, and the write context**

Replace:

```rust
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.call(|client, auth| client.fetch_organization_members(&mut auth.session, org))?;
        crate::rest::organizations::map_members(&answer).ok_or_else(|| SharingError::Refused(ROSTER_UNREADABLE.to_string()))
    }
```

with:

```rust
    ///
    /// **The sharer's own org only**, so a 404 is the route missing, not a
    /// member refused: every server gives the owner ManageUsers. It turns
    /// sharing off as [`Self::route`]'s 405 and 501 do (official-path spec
    /// §2) -- an older NodeWarden had no `/users`.
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.route(|client, auth| match client.fetch_organization_members(&mut auth.session, org) {
            Err(RestError::Status(404)) => Err(RestError::Status(501)),
            other => other,
        })?;
        crate::rest::organizations::map_members(&answer).ok_or_else(|| SharingError::Refused(ROSTER_UNREADABLE.to_string()))
    }
```

and the doc line above it:

```rust
    /// The org's roster, fresh: `/users`, or `/members` on an older server.
```

with:

```rust
    /// The org's roster, fresh, from `/users?includeCollections=true`.
```

In `context_if_any`, replace:

```rust
        let Some(org_id) = self.my_org_id(&profile, &account) else {
            return Ok(None);
        };
        let account = keyed_account(Some(account))?;
        let keys = self.keys_from(&profile)?;
        let org_key = keys
            .organization(&org_id)
            .cloned()
            .ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let collections = self.collections_of(&org_id, &org_key)?;
        Ok(Some(Ctx { account, keys, org_id, org_key, collections }))
```

with:

```rust
        let Some(org_id) = self.my_org_id(&profile, &account)? else {
            return Ok(None);
        };
        let account = keyed_account(Some(account))?;
        let keys = self.keys_from(&profile)?;
        let org_key = keys
            .organization(&org_id)
            .cloned()
            .ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let collections = self.collections_of(&org_id, &org_key)?;
        let roster = self.fetch_roster(&org_id)?;
        let own_ou_id = own_row(&roster, &account.id).map(|row| row.id.clone());
        Ok(Some(Ctx { account, keys, org_id, org_key, collections, roster, own_ou_id }))
```

- [ ] **Step 12: The collection names the sharer's own org-user id**

Write `$TEMP/dw/dw1-collection.txt`:

```rust
    /// C(E), found by decrypted name or created with this account as its
    /// manager (spec 5.2). Returns its id.
    ///
    /// **Only an AUTHENTICATED name is found** -- rule 2 for a grant (final
    /// review, I1). What comes back is the collection the person is invited
    /// to and the record is filed into, so a plaintext name -- which the
    /// server can write -- must not be it: a server that listed `anna@...` in
    /// plaintext over a collection someone else already reaches would be
    /// handed the record. Beside a plaintext twin, the engine makes its own.
    ///
    /// The manager is named by this account's own ORG-USER id
    /// ([`Ctx::own_ou_id`]), read off the roster. It used to be the user id,
    /// which is NodeWarden's org-user id and nobody else's: an official
    /// server or Vaultwarden would have answered "no such user".
    fn ensure_collection(&self, ctx: &mut Ctx, email: &str) -> Result<String, SharingError> {
        if let Some(id) = ctx.authenticated_collection_for(email) {
            return Ok(id.to_string());
        }
        let manager = ctx.own_ou_id.clone().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let name = crate::rest::crypto::encrypt(&ctx.org_key, email.as_bytes()).map_err(crypto)?;
        let id = self
            .route(|client, auth| sharing_api::create_collection(client, &mut auth.session, &ctx.org_id, &name, &manager))
            .map_err(|e| e.or_say(&format!("Couldn't prepare sharing with {email}. Try again.")))?;
        // Authenticated: this engine wrote the name, under the org key.
        ctx.collections.push(PersonCollection { id: id.clone(), email: email.to_string(), authenticated: true });
        Ok(id)
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// C(E), found by decrypted name or created with this account as its' '    /// Whether `spki` may be trusted for `email`: no pin yet, or the same one.' "$TEMP/dw/dw1-collection.txt"
```

Expected: one `… replaced lines …` line.

- [ ] **Step 13: Membership by Bitwarden's invite**

Write `$TEMP/dw/dw1-member.txt`:

```rust
    /// **The person's membership, with access to C(E)** (official-path spec
    /// §3.2): nothing if they have it; otherwise Bitwarden's invite, then a
    /// roster read -- the invite answers nothing, and the roster says the new
    /// org-user id and whether the server accepted at once
    /// ([`Capabilities::accepts_on_invite`]). A member without access to
    /// their own collection is removed and invited again (decision 11).
    ///
    /// No key is touched here. NodeWarden's direct add -- the org key wrapped
    /// to whatever key the server served for the address, at invite time --
    /// is gone with the route; a key is wrapped only by the confirm pass,
    /// under its pin rule (spec 11.5).
    pub(crate) fn ensure_member(&self, ctx: &mut Ctx, email: &str, collection: &str) -> Result<ShareResult, SharingError> {
        let refused = format!("Couldn't invite {email}.");
        if let Some(member) = ctx.member(email).cloned() {
            if member.has_access(collection) {
                return Ok(share_result_of(&member));
            }
            let ou = member.id.clone();
            self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &ou))
                .map_err(|e| e.or_say(&refused))?;
        }
        self.route(|client, auth| sharing_api::invite_user(client, &mut auth.session, &ctx.org_id, email, collection))
            .map_err(|e| e.or_say(&refused))?;
        ctx.roster = self.fetch_roster(&ctx.org_id)?;
        let invited = ctx.member(email).cloned().ok_or_else(|| SharingError::Refused(refused.clone()))?;
        self.held().capabilities.accepts_on_invite = Some(invited.status != MemberStatus::Invited);
        Ok(share_result_of(&invited))
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **The person'"'"'s membership, with access to C(E)** (spec 7.2): nothing if' '    /// **Files a record into C(E)** (spec 9.3). Already in the org: one' "$TEMP/dw/dw1-member.txt"
```

Expected: one `… replaced lines …` line.

Then add `share_result_of` beside `person_state`. Replace:

```rust
/// Spec 6.1's table, as a function of the roster row, the collection and
/// what the confirm pass learned.
```

with:

```rust
/// What a share or a re-invite ends as, read off the person's roster row.
fn share_result_of(member: &Member) -> ShareResult {
    match member.status {
        MemberStatus::Confirmed => ShareResult::Shared,
        MemberStatus::Accepted => ShareResult::WaitingAccept,
        MemberStatus::Invited => ShareResult::WaitingAccount,
    }
}

/// Spec 6.1's table, as a function of the roster row, the collection and
/// what the confirm pass learned.
```

The `filing` calls go through `route` too. In `file_once`, replace:

```rust
            return answered(
                self.call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids)),
            );
        }
        let cipher = self.moved_body(ctx, record).map_err(Filing::Failed)?;
        let ids = vec![collection.to_string()];
        answered(self.call(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids)))
```

with:

```rust
            return answered(
                self.route(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids)),
            );
        }
        let cipher = self.moved_body(ctx, record).map_err(Filing::Failed)?;
        let ids = vec![collection.to_string()];
        answered(self.route(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids)))
```

- [ ] **Step 14: The share, which confirms at once when the server accepted at once**

Write `$TEMP/dw/dw1-share.txt`:

```rust
    /// **Share one record with one person** (spec 6.2-6.5, official-path
    /// spec §3). Ordered so a failure leaves the record untouched: nothing
    /// moves it until org, collection and membership exist. Every step reads
    /// what is there first, so a Retry is the same call.
    ///
    /// **Accepted at invite goes straight to confirm** (official-path spec
    /// §2, "Accepts on invite"). Vaultwarden, and NodeWarden with invitation
    /// email off, make an existing account Accepted the moment it is
    /// invited; the confirm pass then runs here, under its pin rule, so the
    /// dialog can say "Shared" rather than "waiting" for a person who will
    /// never be asked anything. A server that leaves an invitation waits for
    /// the person to accept, and the daemon confirms on a later pass.
    ///
    /// # Errors
    ///
    /// [`SharingError::Refused`] with the sentence to show, or the backend's
    /// own failure.
    pub fn share(&self, item_id: &str, email: &str) -> Result<ShareOutcome, SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let email = normalise_email(email);
        if !plausible_email(&email) {
            return Err(SharingError::Refused(NOT_AN_EMAIL.to_string()));
        }
        let profile = self.profile()?;
        let account = self.held().account.clone();
        if account.as_ref().is_some_and(|account| email == normalise_email(&account.email)) {
            return Err(SharingError::Refused(YOURSELF.to_string()));
        }
        let account = keyed_account(account)?;
        // **An org copy whose move back is pending is not shared again.** Its
        // personal copy exists, and the next pass unfiles everyone and
        // deletes it -- so a person added here would be told "shared" and
        // then silently lose it. Refused, not finished first: the record to
        // share is the personal copy, which the user shares from their vault.
        if self.pending_move(&account.id, item_id).is_some() {
            return Err(SharingError::Refused(COMING_HOME.to_string()));
        }
        // **Pins that cannot be read end the share here**, before an org, a
        // collection or a membership is written: the confirm a share may run
        // straight after its invite is a pin decision, and the state write
        // below would be refused anyway. A file that did not parse is set
        // aside by the check, so the Retry the sentence asks for starts from
        // a fresh one.
        self.pins_for_a_write()?;
        let mut keys = self.keys_from(&profile)?;
        let (record, failures) = self.fetch_record(item_id, &keys)?;
        // The record's own refusals first -- the trash, an attachment, a part
        // that did not decrypt -- judged against its own org, so a record
        // that cannot move is refused before the sharing org is looked for
        // (which reads a roster). The org check follows, against ours.
        refuse_unmovable(&record, &failures, organization_of(&record.item))?;
        let existing = self.my_org_id(&profile, &account)?;
        refuse_unmovable(&record, &failures, existing.as_deref())?;

        let (org_id, org_key, collections, created) = match existing {
            Some(org_id) => {
                let org_key = keys.organization(&org_id).cloned().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections, false)
            }
            None => {
                let (org_id, org_key) = self.create_org(&profile, &account, &keys, &email)?;
                keys = Arc::new(keys.with_organization(&org_id, org_key.clone()));
                // Into the backend's key cache too, which `keys_from` filled:
                // the moved record comes back under this org, and a read the
                // backend makes before its next whole read (a hub push's
                // one-record apply) needs the key the profile does not name
                // yet.
                *self.keys.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&keys));
                // A server that made C(E) from `collectionName` (spec 9.5) is
                // adopted here by name, like any other collection.
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections, true)
            }
        };
        if let Some(path) = self.setup().state_path {
            let (account_id, org) = (account.id.clone(), org_id.clone());
            let was_personal = organization_of(&record.item).is_none();
            let moved = crate::sharing_state::Move { folder_id: record.item.folder_id.clone(), favorite: record.item.favorite };
            let written = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(&account_id);
                // Made here now, or made here before under this same id: such
                // an id is this account's without a roster read
                // (`Sharing::choose_org`).
                me.sharing_org_created = created || (me.sharing_org_created && me.sharing_org_id.as_deref() == Some(org.as_str()));
                me.sharing_org_id = Some(org);
                if was_personal {
                    me.moves.insert(record.item.id.clone(), moved);
                }
            });
            match written {
                Ok(_) => {}
                // Unreadable since the check above (another process wrote
                // it): refused as the check refuses, with nothing moved.
                Err(e) if crate::sharing_state::unreadable(&e).is_some() => return Err(state_refused(&e)),
                Err(e) => log::warn!("the sharing state could not be written before a share: {e}"),
            }
        }
        let roster = self.fetch_roster(&org_id)?;
        let own_ou_id = own_row(&roster, &account.id).map(|row| row.id.clone());
        let mut ctx = Ctx { account, keys, org_id, org_key, collections, roster, own_ou_id };
        let collection = self.ensure_collection(&mut ctx, &email)?;
        let mut result = self.ensure_member(&mut ctx, &email, &collection)?;
        let written = self.file_into(&ctx, &record, &collection)?;
        if let Some(path) = self.setup().sent_path {
            let entry = crate::sent_history::SentRecord {
                kind: crate::sent_history::SentKind::Share,
                sent_at_unix_millis: now_millis(),
                recipient: email.clone(),
                item_id: written.id.clone(),
                name: written.name.clone(),
                ..crate::sent_history::SentRecord::default()
            };
            if let Err(e) = crate::sent_history::update(&path, |history| history.record(entry)) {
                log::warn!("a share was made but could not be added to the sent history: {e}");
            }
        }
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a share: {e}");
        }
        if result == ShareResult::WaitingAccept {
            if let Err(e) = self.confirm_pass(Trigger::Own) {
                log::info!("the confirm after a share stopped early: {e}");
            }
            let confirmed = self
                .held()
                .my_org
                .as_ref()
                .and_then(|org| org.member(&email))
                .is_some_and(|member| member.status == MemberStatus::Confirmed);
            if confirmed {
                result = ShareResult::Shared;
            }
        }
        Ok(ShareOutcome { result, changed: SharingOutcome { upserted: vec![written], removed: Vec::new() } })
    }
}

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **Share one record with one person** (spec 6.2-6.5). Ordered so a' '// ---- stopping ----' "$TEMP/dw/dw1-share.txt"
```

Expected: one `… replaced lines …` line.

- [ ] **Step 15: The move back purges with the official delete, and re-invite uses the context**

Replace:

```rust
                if let Some(made) = answer.get("id").and_then(serde_json::Value::as_str).filter(|id| !id.is_empty() && *id != old) {
                    if let Err(e) = self.call(|client, auth| client.hard_delete_cipher(&mut auth.session, made)) {
```

with:

```rust
                if let Some(made) = answer.get("id").and_then(serde_json::Value::as_str).filter(|id| !id.is_empty() && *id != old) {
                    if let Err(e) = self.call(|client, auth| sharing_api::delete_cipher(client, &mut auth.session, made)) {
```

Replace:

```rust
    /// `DELETE /api/ciphers/{id}/delete` -- NodeWarden's unconditional
    /// permanent delete; the bare route only trashes (see
    /// `RestClient::hard_delete_cipher`).
    ///
    /// **A 404 is done**: the org copy is already gone. Matched on the status
    /// itself, not on the [`VaultError::Http`] it maps to -- that one is every
    /// 4xx, and reading a 403 as "gone" would drop the retry and leave the
    /// copy in the org, filed nowhere, for good. A 5xx or a lost connection
    /// stays an error, and so pending.
    fn purge_org_copy(&self, id: &str) -> Result<(), SharingError> {
        self.call(|client, auth| match client.hard_delete_cipher(&mut auth.session, id) {
            Err(RestError::Status(404)) => Ok(()),
            other => other,
        })
    }
```

with:

```rust
    /// `DELETE /api/ciphers/{id}` -- Bitwarden's permanent delete, on every
    /// server ([`sharing_api::delete_cipher`] says why it is not NodeWarden's
    /// `/delete` suffix any more).
    ///
    /// **A 404 is done**: the org copy is already gone. `delete_cipher`
    /// matches the status itself, not the [`VaultError::Http`] it maps to --
    /// that one is every 4xx, and reading a 403 as "gone" would drop the
    /// retry and leave the copy in the org, filed nowhere, for good. A 5xx
    /// or a lost connection stays an error, and so pending.
    fn purge_org_copy(&self, id: &str) -> Result<(), SharingError> {
        self.call(|client, auth| sharing_api::delete_cipher(client, &mut auth.session, id))
    }
```

In `reinvite`, replace:

```rust
        let refused = format!("Couldn't re-invite {email}.");
        let roster = self.fetch_roster(&ctx.org_id)?;
        let result = match roster.iter().find(|m| normalise_email(&m.email) == email) {
            Some(member) if member.status == MemberStatus::Invited => {
                let ou = member.id.clone();
                self.call(|client, auth| sharing_api::reinvite(client, &mut auth.session, &ctx.org_id, &ou))
                    .map_err(|e| e.or_say(&refused))?;
                ShareResult::WaitingAccount
            }
            _ => {
                let collection = self.ensure_collection(&mut ctx, &email)?;
                self.ensure_member(&ctx, &email, &collection)?
            }
        };
```

with:

```rust
        let refused = format!("Couldn't re-invite {email}.");
        let result = match ctx.member(&email).cloned() {
            Some(member) if member.status == MemberStatus::Invited => {
                let ou = member.id.clone();
                self.route(|client, auth| sharing_api::reinvite(client, &mut auth.session, &ctx.org_id, &ou))
                    .map_err(|e| e.or_say(&refused))?;
                ShareResult::WaitingAccount
            }
            _ => {
                let collection = self.ensure_collection(&mut ctx, &email)?;
                self.ensure_member(&mut ctx, &email, &collection)?
            }
        };
```

Also correct `reinvite`'s doc. Replace:

```rust
    /// **Re-invite** (spec 6.1's Re-invite action): a pending invitation is
    /// renewed -- a new token and a new 7 days, expired or not; anyone else
    /// without access to their collection is invited again the way a share
    /// would ([`Self::ensure_member`]: the direct add when they have an
    /// account, an invitation when not). Someone who still has access is left
    /// as they are, and the answer says which of the three they are.
```

with:

```rust
    /// **Re-invite** (spec 6.1's Re-invite action): a pending invitation is
    /// renewed -- `POST …/users/{ouId}/reinvite` on every server (official-path
    /// spec §3.4), a new token and a new lifetime, expired or not; anyone else
    /// without access to their collection is invited again the way a share
    /// would ([`Self::ensure_member`]). Someone who still has access is left
    /// as they are, and the answer says which of the three they are.
```

and, in the same doc, replace:

```rust
    /// on the card, and a plaintext name is one the server can write. Its
    /// second branch is a direct add that wraps the org key to whatever key
    /// the server serves for that address and pins it on first use, so a
    /// hostile server could list `mallory@...` in plaintext and have one
    /// click hand her the org key. Refused before anything is read: no
    /// roster, no key, no invite (`reinvite_refuses_a_plaintext_collection_name`).
```

with:

```rust
    /// on the card, and a plaintext name is one the server can write. Its
    /// second branch invites the address into the org, and on a server that
    /// accepts at once the confirm pass would then hand her the org key, so
    /// a hostile server could list `mallory@...` in plaintext and have one
    /// click make her a recipient. Refused before anything is written: no
    /// key, no invite (`reinvite_refuses_a_plaintext_collection_name`).
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs`
Expected: `src/rest/sharing.rs 0`

Run: `cargo check -j 2 2>&1 | grep -E "^(error|warning)" | sort | uniq -c`
Expected: no `error` in the library. (The test module still names the old items, but `cargo check` does not build tests.) If a `warning: unused` names `route_absent`, `refusal` or `text_at` in `sharing_api.rs`: `route_absent` is still used by `my_invitations`, and `refusal` and `text_at` by `confirm_member` and others. Any other unused item named is one this step left behind. Delete it.

- [ ] **Step 15b: "Delete forever" purges with Bitwarden's route too (owner, 2026-10-08)**

The owner decided this project also moves Deskwarden's general purge onto Bitwarden's route. It covers "empty trash" and "delete forever", not just sharing's move back.
- **Today:** `RestClient::hard_delete_cipher` (`deskwarden/src/rest/api.rs` lines 1633–1669) sends NodeWarden-only `DELETE /api/ciphers/{id}/delete`. Its doc explains why: NodeWarden's bare `DELETE` used to trash a live item.
- **What changed:** NW2 makes the bare route permanent, as on Bitwarden.
- **Why it matters:** the suffix route does not exist on Vaultwarden or official Bitwarden, so "delete forever" fails there today.

First the test. In `deskwarden/src/rest/api.rs`, in the test about lines 4590–4620 (the one ending in `"the purge went to the compat route, …"`), swap the two mocks' roles:
- `hard` becomes `server.mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")` with `.expect(1)`.
- The other mock is renamed `suffix` and becomes `server.mock("DELETE", "/api/ciphers/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/delete").with_status(200).create()`.
- The final assertion becomes:

```rust
        assert!(
            !suffix.matched(),
            "the purge went to NodeWarden's `/delete` suffix, a route Vaultwarden and \
             Bitwarden do not have; Bitwarden's permanent delete is the bare DELETE"
        );
```

- Replace the comment above the mocks with:

```rust
        // `DELETE .../{id}`, Bitwarden's permanent delete -- on every server,
        // NodeWarden included since it matches Bitwarden (its NW2). NOT
        // NodeWarden's old `DELETE .../delete` suffix, which no other server
        // has. The `suffix` mock below is the assertion that this client no
        // longer goes there.
```

Run: `cargo test --lib -j 2 rest::api` with `CARGO_TARGET_DIR` as in Global Constraints.
Expected: that test FAILS, because `hard` is not matched.

Then the code. Replace the doc and body of `hard_delete_cipher` (lines 1633–1669) with:

```rust
    /// `DELETE /api/ciphers/{id}` -- Bitwarden's permanent delete: gone, with
    /// no trash to recover it from.
    ///
    /// Named `hard_delete` rather than `delete` so that no caller reaches for
    /// it by autocomplete when they meant [`Self::trash_cipher`]. This module
    /// will not decide for a caller which one they want, but it will make the
    /// irreversible one the longer word.
    ///
    /// # Why the bare route, and why it used to be the suffix
    ///
    /// For a while this sent NodeWarden's `DELETE /api/ciphers/{id}/delete`,
    /// because NodeWarden's bare `DELETE` soft-deleted a LIVE item -- a call
    /// named `hard_delete` reporting success for a trash. NodeWarden now
    /// answers the bare route exactly as Bitwarden does (permanent, live or
    /// trashed), and the suffix route exists on no other server, so a purge
    /// against Vaultwarden or official Bitwarden failed outright. One route,
    /// Bitwarden's, on every server -- the owner's "100% compatible" rule.
    pub fn hard_delete_cipher(&self, session: &mut Session, id: &str) -> Result<(), RestError> {
        let url = self.cipher_url(id, "")?;
        self.refreshing(session, |session| {
            self.unit_from(self.bearer(self.write_agent.delete(&url), session).call())
        })
    }
```

- Check `cipher_url(id, "")` gives the bare `{base}/api/ciphers/{id}`. `grep -n "fn cipher_url" deskwarden/src/rest/api.rs` shows a suffix appended verbatim, and other callers already pass `""` (`grep -n 'cipher_url(id, "")'`).
- If `sharing_api::delete_cipher` (Step 15) now does the same thing, leave it; it carries sharing's 404 rule. Its doc may say it matches `hard_delete_cipher`.

Run: `cargo test --lib -j 2 rest::api`.
Expected: PASS.

Check: `grep -rn "/delete\")" deskwarden/src/rest/api.rs` lists no `DELETE` built on the `/delete` suffix. `trash_cipher`'s `PUT …/delete` is Bitwarden's trash route and stays.

**Release dependency, for DW5:** this purge is permanent only once NodeWarden carries NW2. Against an older NodeWarden, the bare route trashes a live item instead. That is safe, since nothing is lost, but it is wrong. Deploy NodeWarden before releasing this Deskwarden build, as the release order already requires.

Add `deskwarden/src/rest/api.rs` to Step 19's `git add` if it is not there (it is).

- [ ] **Step 16: Rewrite the engine's tests for the one path**

Every test below is replaced whole with `dw_cut`. Write each replacement to `$TEMP/dw/` first. All of them use the existing helpers (`on_nodewarden`, `roster`, `roster_row`, `with_roster`, `with_collections`, `echo`, `anna_key`, `person_collection`, `plaintext_collection`, `cache_org`, `cached`, `offline`, `authentic`, `member`).

(a) The probe tests. Write `$TEMP/dw/dw1-t-loaded.txt`:

```rust
    /// **The first pass reads the account, once, and turns sharing on** --
    /// on any server: nothing asks which one it is (official-path spec §2).
    /// A batch pass asks nothing at all.
    #[test]
    fn the_first_pass_reads_the_account_once_and_turns_sharing_on() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        let profile = server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect(1).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_status(404).expect_at_least(0).create();
        let key = server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        engine.refresh(Trigger::Batch).expect("a batch pass");
        profile.assert();
        key.assert();
        assert!(engine.view(&[]).available);
        assert!(engine.held().capabilities.loaded);
    }

    /// **A route sharing needs that answers as missing turns sharing off for
    /// the session** (official-path spec §2): the sharer's own roster
    /// answering 404 -- every server gives the owner ManageUsers, so that is
    /// no route -- or any of sharing's routes answering 405 or 501.
    #[test]
    fn a_missing_route_turns_sharing_off_for_the_session() {
        for status in [404, 405, 501] {
            let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
            server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).with_status(status).create();
            assert!(engine.view(&[]).available, "{status}: available before");
            assert!(matches!(engine.fetch_roster(FIXTURE_ORG), Err(SharingError::Unavailable(REASON_TOO_OLD))), "{status}");
            let view = engine.view(&[]);
            assert!(!view.available, "{status}");
            assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD), "{status}");
            assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::Unavailable(REASON_TOO_OLD))), "{status}: refused");
        }
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **The dialect probe is made once per session** (spec 7.1): the' '    /// **`Held`'"'"'s `Debug` never prints an invitation token** -- the reason it' "$TEMP/dw/dw1-t-loaded.txt"
```

This replaces `the_dialect_probe_is_made_once_per_session`, `probed`, `keyless_profile`, `an_old_nodewarden_without_slice_one_turns_sharing_off`, `an_own_key_answer_without_a_key_is_still_slice_one` and `a_keyless_accounts_404_is_not_read_as_an_old_server`.

`keyless_profile` is still used by `a_keyless_account_is_refused_with_a_sentence_about_keys`. Write `$TEMP/dw/dw1-t-official.txt`, which brings it back beside the official-server test:

```rust
    /// The fixture profile with no key pair: `privateKey` null.
    fn keyless_profile() -> serde_json::Value {
        let mut profile = sharing_profile(serde_json::json!([]));
        profile["privateKey"] = serde_json::Value::Null;
        profile
    }

    /// **An official server shares on the same path**: one that has no
    /// NodeWarden invitation list (404) is not told sharing is off, and its
    /// view is available after the first pass.
    #[test]
    fn an_official_server_shares_on_the_same_routes() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_status(404).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        let view = engine.view(&[]);
        assert!(view.available, "{view:?}");
        assert_eq!(view.reason, None);
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// An official server has no invitation list: sharing is not offered.' '    use crate::test_http::{Matcher, MockServer};' "$TEMP/dw/dw1-t-official.txt"
```

In the keyless test, the probe wording goes. Replace:

```rust
    /// **A keyless account is refused on its keys, before anything is read
    /// or written.** The probe leaves sharing on for it -- NodeWarden's 404
    /// for its own key is no evidence of an old server (see
    /// `a_keyless_accounts_404_is_not_read_as_an_old_server`) -- so this is
    /// where it is stopped: with no private key there is no org key it could
    /// wrap to itself or unwrap, and a sentence about the server's age or a
    /// failed organisation would send the user looking in the wrong place.
```

with:

```rust
    /// **A keyless account is refused on its keys, before anything is read
    /// or written.** Sharing is on for it -- nothing about the server is
    /// wrong -- so this is where it is stopped: with no private key there is
    /// no org key it could wrap to itself or unwrap, and a sentence about the
    /// server's age or a failed organisation would send the user looking in
    /// the wrong place.
```

and replace:

```rust
        engine.refresh(Trigger::WholeRead).expect("probed");
        assert!(engine.view(&[]).available, "control: the probe left sharing on for a keyless account");
```

with:

```rust
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        assert!(engine.view(&[]).available, "control: sharing is on for a keyless account");
```

(b) `held.probe` in two view tests. Use the Edit tool with **replace_all**, replacing:

```rust
            held.probe = Some(Probe { dialect: Dialect::NodeWarden, slice_one: true });
```

with:

```rust
            held.capabilities.loaded = true;
```

Expected: 2 replacements.

Replace the doc of the not-loaded test:

```rust
    /// Before the probe has answered, sharing is "not loaded", never "on".
```

with:

```rust
    /// Before the first pass has read the account, sharing is "not loaded",
    /// never "on".
```

(c) The first share. Write `$TEMP/dw/dw1-t-first.txt`:

```rust
    /// **The first share ever** (spec 6.2, official-path spec §3): org,
    /// collection, Bitwarden's invite, move -- and, because this server
    /// accepted the person at invite, the confirm at once. The collection
    /// names the sharer by its own ORG-USER id off the roster, which is not
    /// its user id here, as on an official server.
    #[test]
    fn a_first_share_creates_the_org_the_collection_the_invite_and_moves_once() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([]));
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        let org = server
            .mock("POST", "/api/organizations")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": format!("Shared by {FIXTURE_EMAIL}"),
                "billingEmail": FIXTURE_EMAIL,
                "keys": { "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }
            })))
            .with_body(r#"{"id":"org-new","object":"organization"}"#)
            .expect(1)
            .create();
        // The org's collections: empty when the org is new, then the one the
        // share made -- with the name the share sent, so it decrypts under the
        // new org key (which the test cannot know) and is authenticated, as
        // the confirm pass's rule 2 requires.
        let made_name = Arc::new(Mutex::new(String::new()));
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).expect(1).create();
        let read_back = Arc::clone(&made_name);
        server
            .mock("GET", "/api/organizations/org-new/collections")
            .with_body_from_request(move |_| {
                let name = read_back.lock().expect("unpoisoned").clone();
                serde_json::json!({ "data": [{ "id": "c-anna", "organizationId": "org-new", "name": name }] }).to_string().into_bytes()
            })
            .create();
        let owner = || roster_row("owner-ou", Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]);
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[owner()]))
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[owner(), roster_row("anna-ou", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]))
            .create();
        let keep = Arc::clone(&made_name);
        let collection = server
            .mock("POST", "/api/organizations/org-new/collections")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "users": [{ "id": "owner-ou", "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body_from_request(move |request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                *keep.lock().expect("unpoisoned") = body["name"].as_str().unwrap_or_default().to_string();
                br#"{"id":"c-anna","object":"collection"}"#.to_vec()
            })
            .expect(1)
            .create();
        let invite = server
            .mock("POST", "/api/organizations/org-new/users/invite")
            .match_body(Matcher::PartialJson(serde_json::json!({ "emails": ["anna@example.com"], "type": 2 })))
            .with_body("")
            .expect(1)
            .create();
        let share = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_body(Matcher::PartialJson(serde_json::json!({ "collectionIds": ["c-anna"], "cipher": { "organizationId": "org-new" } })))
            .with_body_from_request(echo("p1", Some("org-new"), &["c-anna"]))
            .expect(1)
            .create();
        let key = server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server
            .mock("POST", "/api/organizations/org-new/users/anna-ou/confirm")
            .match_request(|request| request.utf8_lossy_body().contains("\"key\":\"4."))
            .with_body("")
            .expect(1)
            .create();
        let direct = server.mock("POST", "/api/organizations/org-new/members/invite").expect(0).create();
        let lookup = server.mock("GET", Matcher::Regex("members/public-key".into())).expect(0).create();

        let outcome = engine.share("p1", " Anna@Example.com ").expect("shared");
        org.assert();
        collection.assert();
        invite.assert();
        share.assert();
        key.assert();
        confirm.assert();
        direct.assert();
        lookup.assert();
        sync.assert();
        assert_eq!(outcome.result, ShareResult::Shared, "accepted at invite, confirmed at once");
        assert_eq!(engine.held().capabilities.accepts_on_invite, Some(true));
        assert_eq!(outcome.changed.upserted.len(), 1);
        let moved = &outcome.changed.upserted[0];
        assert_eq!(moved.name, "Acme VPN", "the moved record decrypts under the new org key");
        assert_eq!(organization_of(moved), Some("org-new"));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.sharing_org_id.as_deref(), Some("org-new"));
        assert_eq!(state.moves.get("p1").and_then(|m| m.folder_id.as_deref()), Some("f1"));
        assert!(state.pin_for("anna@example.com").is_some(), "the confirm pins the key it wrapped to");
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].kind, crate::sent_history::SentKind::Share);
        assert_eq!(sent.entries[0].recipient, "anna@example.com");
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **The first share ever** (spec 6.2): org, collection, direct add,' '    /// **An existing person, another record already in the org** (spec 6.3,' "$TEMP/dw/dw1-t-first.txt"
```

(d) A person the server leaves Invited. Write `$TEMP/dw/dw1-t-invited.txt`:

```rust
    /// **A server that leaves an invitation** (official Bitwarden, or any
    /// server with invitation email): the share files the record and says
    /// so, and nothing is confirmed until the person accepts.
    #[test]
    fn a_share_the_server_leaves_invited_waits_for_the_accept() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-carol", "carol@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(2)
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("inv-1", None, "carol@example.com", 0, &["c-carol"]),
            ]))
            .create();
        let invite = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        let confirms = server.mock("POST", Matcher::Regex("/confirm$".into())).expect(0).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-carol"])).create();
        let outcome = engine.share("p1", "carol@example.com").expect("shared");
        invite.assert();
        confirms.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccount);
        assert_eq!(engine.held().capabilities.accepts_on_invite, Some(false));
    }

```

The first roster answer is read twice, by `my_org_id`'s attestation and by the write's own read. The second answer comes after the invite.

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A person with no account** (spec 6.5): a pending invitation, no key' '    /// **A failure at the move leaves the record personal** (spec 6.2, 13),' "$TEMP/dw/dw1-t-invited.txt"
```

(e) A changed key. Write `$TEMP/dw/dw1-t-pin.txt`:

```rust
    /// **A key that differs from the pin is not wrapped to** (spec 12.1): the
    /// person is accepted at invite, the confirm at once finds their key
    /// changed and does not confirm, the card will say so, and the pin is not
    /// moved to the new key.
    #[test]
    fn a_key_that_differs_from_the_pin_is_not_wrapped_to() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let state = dir.join("sharing_state.json");
        let old = crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 };
        crate::sharing_state::update(&state, |s| s.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", old.clone())).expect("pinned");
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(2)
            .create();
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        let invite = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).expect(0).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared");
        invite.assert();
        confirm.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccept);
        assert!(engine.held().key_changed.contains("anna@example.com"));
        let pinned = crate::sharing_state::SharingState::load(&state).account(FIXTURE_USER_ID);
        assert_eq!(pinned.pin_for("anna@example.com"), Some(&old), "the pin moved to a key nobody confirmed");
    }

```

`with_roster` adds a second mock for the same path with `expect_at_least(0)`. It answers once the first, `expect(2)`, mock is spent. That is the read after the invite and every read after it.

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A key that differs from the pin is not wrapped to** (spec 12.1): the' '    /// **The trash, another organisation, and a field that did not decrypt**' "$TEMP/dw/dw1-t-pin.txt"
```

In `what_cannot_move_is_refused_with_its_own_sentence`, the share now reads the sharing org's roster before it judges the org. Replace:

```rust
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row(), team]));
        let mut trashed = personal_cipher("t1", "Old");
```

with:

```rust
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row(), team]));
        with_roster(&mut server, &[]);
        let mut trashed = personal_cipher("t1", "Old");
```

(f) Re-invite. Write `$TEMP/dw/dw1-t-reinvite.txt`:

```rust
    /// **Re-invite** (official-path spec §3.4): a pending invitation is
    /// renewed with `POST …/users/{ouId}/reinvite`; a person who is gone is
    /// invited again with Bitwarden's invite.
    #[test]
    fn reinviting_renews_an_invitation_and_invites_a_gone_person_again() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-carol", "carol@example.com"), person_collection("c-gus", "gus@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"])]))
            .expect(5)
            .create();
        with_roster(
            &mut server,
            &[roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]), roster_row("inv-gus", None, "gus@example.com", 0, &["c-gus"])],
        );
        let renewed = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/inv-carol/reinvite").as_str()).with_body("").expect(1).create();
        assert_eq!(engine.reinvite("carol@example.com").expect("renewed"), ShareResult::WaitingAccount);
        renewed.assert();

        let invited = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({ "emails": ["gus@example.com"] })))
            .with_body("")
            .expect(1)
            .create();
        assert_eq!(engine.reinvite("gus@example.com").expect("invited"), ShareResult::WaitingAccount);
        invited.assert();
    }

```

Mock selection in `test_http` follows `mockito`: of the mocks that match, the first one still owed hits answers; once all are satisfied, the last one created does.
- Carol's re-invite reads the roster three times: `my_org_id`'s attestation, the write's context, and the reload after it.
- Gus's reads it twice before his invite (reads 4 and 5, still the first mock, where he is absent).
- After the invite come read 6, and the reload as read 7. Both fall to `with_roster`'s mock, where he is listed.

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **Re-invite** (spec 6.5): a pending invitation gets a new token and 7' '    /// `GET .../collections/{collection}/items` answers `ids`: what the' "$TEMP/dw/dw1-t-reinvite.txt"
```

(g) A short key, and an account with no key yet. Write `$TEMP/dw/dw1-t-short.txt`:

```rust
    /// **A key under 2048 bits is never wrapped to** (final review, item 6):
    /// the server chooses which key an account answers with, and the org key
    /// wrapped to a short one is not a secret. The person is invited --
    /// nothing about the address is wrong -- but the confirm at once refuses
    /// the key, so nobody is confirmed and nothing is pinned.
    #[test]
    fn a_share_to_a_short_key_confirms_nobody() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(2)
            .create();
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        let short = {
            use rsa::pkcs8::EncodePublicKey;
            let private = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).expect("a 1024-bit key");
            private.to_public_key().to_public_key_der().expect("spki").as_bytes().to_vec()
        };
        server
            .mock("GET", "/api/users/anna-id/public-key")
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&short) }).to_string())
            .create();
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let confirm = server.mock("POST", Matcher::Regex("/confirm$".into())).expect(0).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared, not confirmed");
        confirm.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccept);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("anna@example.com").is_none(), "a refused key is not pinned");
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A direct add to a key under 2048 bits is refused before the member' '    /// **An expired invitation gives the timer nothing to do** (final' "$TEMP/dw/dw1-t-short.txt"
```

Write `$TEMP/dw/dw1-t-keyless.txt`:

```rust
    /// **An account with no key yet is waiting to accept, not to create an
    /// account**: the server accepted it at invite, the confirm at once finds
    /// no key, and the view says it waits for a first sign-in.
    #[test]
    fn an_accepted_account_with_no_key_waits_for_a_first_sign_in() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-kim", "kim@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(2)
            .create();
        with_roster(&mut server, &[roster_row("kim-ou", Some("kim-id"), "kim@example.com", 1, &["c-kim"])]);
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        server.mock("GET", "/api/users/kim-id/public-key").with_status(404).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-kim"])).create();
        let outcome = engine.share("p1", "kim@example.com").expect("shared");
        assert_eq!(outcome.result, ShareResult::WaitingAccept, "an existing account is never told to create one");
        assert_eq!(engine.view(&[]).person("kim@example.com").map(|p| p.state), Some(PersonState::WaitingSignIn));
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A keyless account added directly is waiting to accept, not to' '    // ---- final re-review, CRITICAL: an org is mine only on the server'"'"'s word ----' "$TEMP/dw/dw1-t-keyless.txt"
```

(h) "Mine" by the roster. Write `$TEMP/dw/dw1-t-mine.txt`:

```rust
    // ---- an org is mine only on the roster's word (official-path spec §3.1) ----

    /// A roster row of another account in the OWNER role -- the planted
    /// shape: Mallory made "Shared by <me>" and added me as an owner.
    fn other_owner() -> serde_json::Value {
        let mut row = roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", 2, &[]);
        row["type"] = serde_json::json!(0);
        row
    }

    fn cache_org(dir: &crate::test_scratch::ScratchDir, created: bool) {
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            let me = state.account_mut(FIXTURE_USER_ID);
            me.sharing_org_id = Some(FIXTURE_ORG.into());
            me.sharing_org_created = created;
        })
        .expect("seeded");
    }

    fn cached(dir: &crate::test_scratch::ScratchDir) -> (Option<String>, bool) {
        let me = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        (me.sharing_org_id, me.sharing_org_created)
    }

    /// **A planted org -- owned by this account and named for it, but with
    /// another owner on its roster -- is never this account's sharing org**
    /// (final re-review, CRITICAL, now on the roster's word). The share does
    /// not file into it, and a fresh org of my own is created instead.
    /// Nothing is written to the planted org.
    #[test]
    fn a_planted_org_with_another_owner_is_not_used_and_a_fresh_one_is_made() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), other_owner()]))
            .create();
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let account = engine.held().account.clone().expect("an account");
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer"), None);

        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        let planted_collections = server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str()).expect(0).create();
        let planted_writes = server.mock("POST", Matcher::Regex(format!("^/api/organizations/{FIXTURE_ORG}/"))).expect(0).create();
        let filed = server.mock("PUT", "/api/ciphers/p1/share").match_request(|r| r.utf8_lossy_body().contains(FIXTURE_ORG)).expect(0).create();
        let fresh = server.mock("POST", "/api/organizations").with_body(r#"{"id":"org-new","object":"organization"}"#).expect(1).create();
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).create();
        // What follows the create is not this test's business; the create is.
        let _ = engine.share("p1", "anna@example.com");
        fresh.assert();
        planted_collections.assert();
        planted_writes.assert();
        filed.assert();
        assert_eq!(cached(&dir), (Some("org-new".into()), true), "the fresh org is remembered as made here");
    }

    /// **A whole read holds no planted org for the confirm pass** -- the
    /// roster that read fetched names another owner.
    #[test]
    fn a_whole_read_does_not_hold_a_planted_org() {
        let (_server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let mut directory = crate::rest::organizations::Directory::from_sync(
            &serde_json::from_value(serde_json::json!({ "profile": sharing_profile(serde_json::json!([sharing_org_row()])), "ciphers": [], "folders": [], "collections": [] })).expect("sync"),
            &engine.keys_from(&profile).expect("keys"),
        );
        directory.set_roster(FIXTURE_ORG, &serde_json::json!({ "data": [roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), other_owner()] }));
        engine.note_directory(&directory, Some(&profile));
        assert!(engine.held().my_org.is_none(), "a planted org is not held as ours");
        directory.set_roster(FIXTURE_ORG, &serde_json::json!({ "data": [roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])] }));
        engine.note_directory(&directory, Some(&profile));
        assert_eq!(engine.held().my_org.as_ref().map(|o| o.id.as_str()), Some(FIXTURE_ORG), "control: alone as owner, it is ours");
    }

    /// **A cached id the roster disowns is dropped** -- unless this client
    /// made it: an org this client created cannot have been planted.
    #[test]
    fn a_cached_id_the_roster_disowns_is_dropped_unless_made_here() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([]));
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), other_owner()]))
            .create();
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let account = engine.held().account.clone().expect("an account");
        cache_org(&dir, false);
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer"), None);
        assert_eq!(cached(&dir), (None, false), "the cached id is gone");
        cache_org(&dir, true);
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG), "the id this client made");
    }

    /// **A genuine org is used** -- owned, named for this account, and alone
    /// as owner on its roster -- found with or without a cache; and **a
    /// roster that cannot be read refuses rather than answering "not mine"**,
    /// which would have a share make a second org beside the real one.
    #[test]
    fn a_genuine_org_is_adopted_on_the_roster_and_an_unreadable_roster_refuses() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([]));
        let account = engine.held().account.clone().expect("an account");
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let path = format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true");
        server.mock("GET", path.as_str()).with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])])).expect(2).create();
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG), "adopted on the roster's word");
        cache_org(&dir, false);
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG), "a cached id the roster owns");
        assert_eq!(cached(&dir).0.as_deref(), Some(FIXTURE_ORG), "kept");

        server.mock("GET", path.as_str()).with_body(r#"{"message":"Not found"}"#).create();
        assert!(matches!(engine.my_org_id(&profile, &account), Err(SharingError::Refused(s)) if s == ROSTER_UNREADABLE));
        assert_eq!(cached(&dir).0.as_deref(), Some(FIXTURE_ORG), "an unreadable roster drops nothing");
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    // ---- final re-review, CRITICAL: an org is mine only on the server'"'"'s word ----' '    /// **An owner, admin or manager invitation is not a personal share**' "$TEMP/dw/dw1-t-mine.txt"
```

In `a_cached_id_the_profile_does_not_list_yet_is_kept`, replace:

```rust
        assert_eq!(engine.my_org_id(&stale, &account), None, "not the answer this time");
```

with:

```rust
        assert_eq!(engine.my_org_id(&stale, &account).expect("an answer"), None, "not the answer this time");
```

(i) The plaintext re-invite and the plaintext twin. Write `$TEMP/dw/dw1-t-plain.txt`:

```rust
    /// **Re-invite is refused for a person known only from a plaintext
    /// name, before anything is written** (final review, I1). Its second
    /// branch invites the address, and on a server that accepts at once the
    /// confirm pass would hand her the org key; a hostile server that listed
    /// `mallory@...` in plaintext would have one click make her a recipient.
    /// So: no invite, no key looked up, no pin.
    #[test]
    fn reinvite_refuses_a_plaintext_collection_name() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[plaintext_collection("c-mal", "mallory@example.com")]);
        with_roster(&mut server, &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]);
        let key = server.mock("GET", Matcher::Regex("/public-key$".into())).expect(0).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        match engine.reinvite("Mallory@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, not_vouched_for("mallory@example.com")),
            other => panic!("a plaintext name was re-invited: {other:?}"),
        }
        key.assert();
        writes.assert();
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("mallory@example.com").is_none(), "nothing was pinned");
    }

    /// **A share beside a plaintext twin files into a collection the engine
    /// names itself** (final review, I1): the plaintext `c-plain` is neither
    /// the collection anna is invited to nor the one the record lands in.
    #[test]
    fn a_share_beside_a_plaintext_twin_makes_its_own_collection() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[plaintext_collection("c-plain", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(2)
            .create();
        with_roster(
            &mut server,
            &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), roster_row("inv-1", None, "anna@example.com", 0, &["c-anna"])],
        );
        let made = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(r#"{"id":"c-anna","object":"collection"}"#)
            .expect(1)
            .create();
        let invite = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str())
            .match_request(|request| request.utf8_lossy_body().contains("c-anna") && !request.utf8_lossy_body().contains("c-plain"))
            .with_body("")
            .expect(1)
            .create();
        let moved = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_request(|request| request.utf8_lossy_body().contains("c-anna") && !request.utf8_lossy_body().contains("c-plain"))
            .with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"]))
            .expect(1)
            .create();
        engine.share("p1", "anna@example.com").expect("shared");
        made.assert();
        invite.assert();
        moved.assert();
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **Re-invite is refused for a person known only from a plaintext' '    /// **A stop takes the person off every collection named for them**, a' "$TEMP/dw/dw1-t-plain.txt"
```

(j) Delete the direct-add pins test. Direct add is gone; the share's own refusal over unreadable pins (`a_share_over_unreadable_pins_is_refused_before_anything_is_written`) still holds that rule.

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A direct add refuses while the pins cannot be read** (review of Task' '    /// **A share over pins that cannot be read writes nothing**: refused'
```

(k) The purge's route moved. Write `$TEMP/dw_delete_paths.py` with the Write tool:

```python
# Rewrites the move-back DELETE mocks in rest/sharing.rs's tests from
# NodeWarden's /api/ciphers/{id}/delete to Bitwarden's /api/ciphers/{id}.
# Only lines that mock a DELETE of a cipher are touched; CRLF is kept.
import sys
path = sys.argv[1]
lines = open(path, "rb").read().split(b"\r\n")
changed = 0
for i, line in enumerate(lines):
    if b'mock("DELETE"' in line and b"/api/ciphers/" in line and b"/delete" in line:
        new = line.replace(b'/delete")', b'")').replace(b'/delete"', b'"')
        if new != line:
            lines[i] = new
            changed += 1
open(path, "wb").write(b"\r\n".join(lines))
print(path, "rewrote", changed, "DELETE mocks")
```

Run: `python "$TEMP/dw_delete_paths.py" src/rest/sharing.rs`
Expected: `src/rest/sharing.rs rewrote 17 DELETE mocks`.

(l) The two view-wiring tests outside the engine. In `src/vault_api.rs`, replace:

```rust
        assert_eq!(after["data"]["dialect"], "nodewarden");
```

with:

```rust
        assert!(after["data"].get("dialect").is_none(), "there is no dialect any more: {after}");
```

In `src/vault_window/mod.rs`, replace:

```rust
        let off = SharingView::unavailable(crate::rest::sharing::REASON_NOT_NODEWARDEN);
```

with:

```rust
        let off = SharingView::unavailable(crate::rest::sharing::REASON_BW_SERVE);
```

In `src/vault_window/share_ui.rs`'s `the_share_control_is_decided_by_the_views_reason`, replace:

```rust
        use crate::rest::sharing::{REASON_BW_SERVE, REASON_NOT_LOADED, REASON_NOT_NODEWARDEN, REASON_TOO_OLD};
```

with:

```rust
        use crate::rest::sharing::{REASON_BW_SERVE, REASON_NOT_LOADED, REASON_TOO_OLD};
```

and replace:

```rust
        for reason in [REASON_NOT_LOADED, REASON_BW_SERVE, REASON_NOT_NODEWARDEN, "some-later-reason"] {
```

with:

```rust
        // "not-nodewarden" is a reason an older daemon could still answer
        // with: like any reason this window does not name, it draws nothing.
        for reason in [REASON_NOT_LOADED, REASON_BW_SERVE, "not-nodewarden", "some-later-reason"] {
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/vault_api.rs src/vault_window/mod.rs src/vault_window/share_ui.rs`
Expected: four lines, each `0`.

- [ ] **Step 17: Run the sharing tests**

Run: `cargo test --lib -j 2 rest:: 2>&1 | tail -15`
Expected: `test result: ok.`

If a share test fails with a `Vault(…)` error instead of its sentence, it is a test on `sharing_org_row()` with no roster mock. The share now reads the sharing org's roster (`my_org_id`). Add `with_roster(&mut server, &[]);` after its `on_nodewarden(…)` line, as Step 16 (e) did for `what_cannot_move_is_refused_with_its_own_sentence`. Do not add any other kind of change.

Run: `grep -rn "members/invite\|members/public-key\|REASON_NOT_NODEWARDEN\|DIALECT_NODEWARDEN\|Dialect\|slice_one\|ensure_probe\|key_route" src --include=*.rs; grep -n "hard_delete_cipher" src/rest/sharing.rs src/rest/sharing_api.rs`
Expected: only `src/rest/api.rs` lines 2704 and 2710. That test logs a made-up URL to show its query is redacted, and it names no route the engine calls.

- [ ] **Step 18: Verify the four commands**

Run each of the four commands in Global Constraints › Deskwarden › Verify per task.
Expected: all pass; `cargo check --all-targets -j 2` prints no `warning:`.

Run: `grep -r --include=*.rs -o "#\[test\]" src | wc -l`
Expected: `6131`.

- [ ] **Step 19: Commit**

```bash
cd /e/Personal/node-bitwarden
git add deskwarden/src/rest/sharing.rs deskwarden/src/rest/sharing_api.rs deskwarden/src/rest/api.rs deskwarden/src/rest/organizations.rs deskwarden/src/vault_api.rs deskwarden/src/vault_window/mod.rs deskwarden/src/vault_window/share_ui.rs
git commit -F - <<'EOF'
Sharing invites, files and moves back on Bitwarden's routes on every server

The sharing engine probed for NodeWarden with /invitations/mine and
turned sharing off everywhere else. It now asks nothing about the
server: sharing is on once the account is read, and off with
server-too-old only when a route it needs answers as missing (405 or
501, or a 404 from the sharer's own roster).

A person is invited with Bitwarden's POST .../users/invite and the
official body, never NodeWarden's direct add or its public-key lookup
by email. The roster read after the invite says whether the server
accepted at once; when it did -- Vaultwarden, NodeWarden with mail off
-- the confirm pass runs straight away, under its pin rule, so the
dialog says Shared.

The sharer's own org is recognised by the roster instead of
NodeWarden's ownerEmail: owned, named for the account, and either made
here or with no other owner on its roster. The person collection now
names the sharer by its own org-user id from that roster, which is not
the user id on official servers or Vaultwarden. A record leaves the org
by Bitwarden's DELETE /api/ciphers/{id}, and the roster is read from
/users only.

"Delete forever" also purges with Bitwarden's bare DELETE /api/ciphers/{id}
instead of NodeWarden's /delete suffix, which no other server has.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task DW2: What the sharer's card and dialog say

**Files:**
- Modify: `src/rest/sharing.rs`:
  - `:62-80` (`PersonState`), `:83-103` (`PersonEntry.expires_at` doc), `:149-162` (`SharingView` doc), `:261-271` (`ShareResult`);
  - the `view`'s `now` and `person_state` call;
  - `ensure_loaded`;
  - `create_org` (`:1271-1294` at `f2eecc8`);
  - `person_state`;
  - tests `every_person_state_is_read_off_the_roster_and_the_records_off_their_collections`, `a_pending_invitation_says_when_it_expires_and_then_that_it_has`.
- Modify: `src/rest/sharing_api.rs` (new `server_name`, and a test)
- Modify: `src/vault_window/share_ui.rs`:
  - `:47-99` (words);
  - `:131-162` (`PersonLine`);
  - `:164-246` (lines, `short_state`);
  - `:248-266` (`share_control` doc);
  - `:1340-1361` (`no_account_band`);
  - `:1468-1483` (the dialog's warning);
  - `:2462-2500` (`share_rows`);
  - tests `:3094-3140`, `:3515-3532`, `:3660-3702`, `:3960-3990`, `:4116-4142`, `:4400-4435`.
- Modify: `src/vault_window/mod.rs:3364-3374` (`share_rows` call), `:9317-9320` (`permit_reinvite` doc), `:41288` (test)
- Modify: `src/vault_window/detail.rs:715-717` (`Reinvite` doc), `:20468-20473` (test)
- Modify: `src/daemon_link.rs:643,725` (tests)

**Interfaces:**
- Consumes: `Capabilities` and `Sharing::ensure_loaded` (DW1); `/config`'s `server.name` (NW1).
- Produces:
  - `PersonState::Invited` (replaces `WaitingAccount`; serialised `"invited"`). `PersonState::Expired` is removed.
  - `ShareResult::Invited` (replaces `WaitingAccount`; serialised `"invited"`).
  - `pub fn needs_org(email: &str) -> String` in `rest::sharing`.
  - `pub fn server_name(client: &RestClient, session: &mut Session) -> Result<Option<String>, RestError>` in `sharing_api`.
  - `SharingView::roster_visible_to_recipients` is now computed: false only when `/config` names the server Vaultwarden or NodeWarden.
  - `pub(crate) const ROSTER_VISIBLE_WARNING: &str` in `share_ui`.
  - `pub(crate) fn short_state(state: PersonState) -> &'static str`.
  - `share_rows(view, sources, history, zone)`, with no `now_ms`.
  - `ShareDialog::invited` (was `waiting_account`).
- Test count: +4 (6131 → 6135).

- [ ] **Step 1: Write the failing engine tests**

Write `$TEMP/dw/dw2-engine-tests.txt`:

```rust
    /// **A server that will not create the sharing org** (official
    /// self-hosted, official-path spec §3.1): the share asks the user to make
    /// it in the web vault, by its name, keeps the server's own words after
    /// that, remembers the answer, and writes nothing else.
    #[test]
    fn a_server_that_will_not_create_the_org_asks_the_user_to_make_it() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("POST", "/api/organizations")
            .with_status(400)
            .with_body(r#"{"message":"Only allowed when not self hosted."}"#)
            .expect(1)
            .create();
        let invites = server.mock("POST", Matcher::Regex("/users/invite$".into())).expect(0).create();
        let moved = server.mock("PUT", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => {
                assert!(sentence.starts_with(&needs_org(FIXTURE_EMAIL)), "{sentence}");
                assert!(sentence.ends_with("Only allowed when not self hosted."), "{sentence}");
            }
            other => panic!("expected the create-it-yourself sentence, got {other:?}"),
        }
        invites.assert();
        moved.assert();
        assert_eq!(engine.held().capabilities.creates_orgs, Some(false));
        assert_eq!(
            needs_org("Denis@Example.com"),
            "Your server needs an organisation for sharing. Create one named \"Shared by denis@example.com\" in the web vault, then try again."
        );
    }

    /// **The roster-visibility warning follows the server's own name**
    /// (official-path spec §3.4): Vaultwarden and NodeWarden keep the roster
    /// to managers and get none; official Bitwarden, any other name, and a
    /// server that names itself nothing are warned about -- the direction
    /// that discloses rather than hides.
    #[test]
    fn the_roster_warning_follows_the_servers_own_name() {
        let engine = offline();
        let visible = |name: Option<&str>| {
            {
                let mut held = engine.held();
                held.capabilities.loaded = true;
                held.capabilities.server_name = name.map(str::to_string);
            }
            engine.view(&[]).roster_visible_to_recipients
        };
        assert!(!visible(Some("Vaultwarden")));
        assert!(!visible(Some("NodeWarden")));
        assert!(!visible(Some("nodewarden")), "case does not matter");
        assert!(visible(Some("Bitwarden")));
        assert!(visible(None), "an unnamed server warns");
    }

    /// **The first pass reads the server's name once**, from `/config`.
    #[test]
    fn the_first_pass_reads_the_servers_name_once() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        let config = server
            .mock("GET", "/api/config")
            .with_body(r#"{"object":"config","server":{"name":"NodeWarden","url":"https://github.com/shuaiplus/NodeWarden"}}"#)
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        config.assert();
        assert_eq!(engine.held().capabilities.server_name.as_deref(), Some("NodeWarden"));
        assert!(!engine.view(&[]).roster_visible_to_recipients);
    }

```

Insert them before the test that follows the official-server test:

```bash
python - "$TEMP/dw/dw2-engine-tests.txt" <<'EOF'
import sys
path = "src/rest/sharing.rs"
marker = "    use crate::test_http::{Matcher, MockServer};".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, marker + b"\r\n\r\n" + add.rstrip(b"\r\n"), 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

This heredoc feeds Python on stdin; it writes the `.rs` file as bytes, keeping CRLF.

Then the two existing tests about expiry. In `every_person_state_is_read_off_the_roster_and_the_records_off_their_collections`, replace:

```rust
        assert_eq!(state("carol@x.io"), Some(PersonState::WaitingAccount));
        assert_eq!(state("dan@x.io"), Some(PersonState::Expired));
```

with:

```rust
        assert_eq!(state("carol@x.io"), Some(PersonState::Invited));
        assert_eq!(state("dan@x.io"), Some(PersonState::Invited), "no expiry is claimed (official-path spec §3.4)");
```

Write `$TEMP/dw/dw2-t-invited.txt`:

```rust
    /// **A pending invitation is Invited whatever its expiry** (official-path
    /// spec §3.4): official servers do not show a client the expiry, so no
    /// card claims one -- and a NodeWarden that does name one is read the same.
    #[test]
    fn a_pending_invitation_is_invited_whatever_its_expiry() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-carol", "carol@example.com"), authentic("c-dan", "dan@example.com")],
            roster: Vec::new(),
        });
        let mut carol = roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]);
        carol["invitationExpirationDate"] = serde_json::json!("2999-01-01T00:00:00.000Z");
        let mut dan = roster_row("inv-dan", None, "dan@example.com", 0, &["c-dan"]);
        dan["invitationExpirationDate"] = serde_json::json!("2001-01-01T00:00:00.000Z");
        with_roster(&mut server, &[carol, dan]);
        engine.refresh(Trigger::Timer).expect("a pass");
        let view = engine.view(&[]);
        assert_eq!(view.person("carol@example.com").map(|p| p.state), Some(PersonState::Invited));
        assert_eq!(view.person("dan@example.com").map(|p| p.state), Some(PersonState::Invited));
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A pending invitation'"'"'s expiry reaches the view** through a real' '    /// The fixture account on NodeWarden, owning the sharing org, as the' "$TEMP/dw/dw2-t-invited.txt"
```

Write the `sharing_api` test, `$TEMP/dw/dw2-api-test.txt`:

```rust
    /// `/config`'s `server.name` -- Vaultwarden's and NodeWarden's own name;
    /// official Bitwarden sends `null`, which is no name.
    #[test]
    fn the_servers_own_name_is_read_from_config() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("GET", "/api/config")
            .with_body(r#"{"object":"config","server":{"name":"Vaultwarden","url":"https://github.com/dani-garcia/vaultwarden"}}"#)
            .expect(1)
            .create();
        assert_eq!(server_name(&client, &mut session).expect("read").as_deref(), Some("Vaultwarden"));
        let mut official = crate::test_http::server();
        let (client, mut session) = granted(&mut official);
        official.mock("GET", "/api/config").with_body(r#"{"object":"config","server":null}"#).create();
        assert_eq!(server_name(&client, &mut session).expect("read"), None);
    }

```

Insert it before the test `a_refusal_keeps_the_servers_sentence`:

```bash
python - "$TEMP/dw/dw2-api-test.txt" <<'EOF'
import sys
path = "src/rest/sharing_api.rs"
marker = "    /// Any other refusal keeps the server's own sentence, whichever key it".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, add + marker, 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/rest/sharing_api.rs`
Expected: both `0`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --lib -j 2 rest::sharing 2>&1 | tail -5`
Expected: compile errors for `needs_org`, `server_name` and `PersonState::Invited`.

- [ ] **Step 3: Rename the two states and the dialog field everywhere**

Write `$TEMP/dw_rename.py` with the Write tool:

```python
# Renames the "no account" state and result to "invited", across the crate,
# keeping CRLF. Prints how many replacements each file took.
import pathlib
pairs = [
    (b"PersonState::WaitingAccount", b"PersonState::Invited"),
    (b"ShareResult::WaitingAccount", b"ShareResult::Invited"),
    (b'"result":"waitingAccount"', b'"result":"invited"'),
]
share_ui_only = [
    (b"no_account_band", b"invited_band"),
    (b"no_account_block", b"invited_block"),
    (b"waiting_account_toast", b"invited_toast"),
    (b"waiting_account", b"invited"),
]
for path in sorted(pathlib.Path("src").rglob("*.rs")):
    data = path.read_bytes()
    count = 0
    for old, new in pairs + (share_ui_only if path.as_posix() == "src/vault_window/share_ui.rs" else []):
        count += data.count(old)
        data = data.replace(old, new)
    if count:
        path.write_bytes(data)
        print(path.as_posix(), count)
```

Run: `python "$TEMP/dw_rename.py"`
Expected:
- `src/daemon_link.rs 4`;
- `src/rest/sharing.rs` with a count of at least 5;
- `src/vault_window/detail.rs 1`;
- `src/vault_window/mod.rs 1`;
- `src/vault_window/share_ui.rs` with a count of at least 20.

Then the two variant definitions. In `src/rest/sharing.rs`, replace:

```rust
    /// NodeWarden: a pending invitation, no account for the email.
    WaitingAccount,
    /// Accepted, but the account has no public key yet.
    WaitingSignIn,
    /// The invitation's expiry has passed.
    Expired,
```

with:

```rust
    /// A pending invitation (official status 0). No server tells a client
    /// whether the address has an account, so nothing here claims either,
    /// and no expiry is claimed (official-path spec §3.4).
    Invited,
    /// Accepted, but the account has no public key yet.
    WaitingSignIn,
```

Replace:

```rust
    /// A member without the key yet: the confirm pass will finish it.
    WaitingAccept,
    /// No account: a pending invitation.
    WaitingAccount,
```

with:

```rust
    /// A member without the key yet: the confirm pass will finish it.
    WaitingAccept,
    /// The server left a pending invitation: the person accepts, then the
    /// confirm pass finishes it.
    Invited,
```

- [ ] **Step 4: The engine's state, name and create**

Replace:

```rust
    /// An invitation's expiry, ISO 8601.
    pub expires_at: Option<String>,
```

with:

```rust
    /// NodeWarden's invitation expiry, ISO 8601, when the server names one.
    /// Not drawn: no expiry is claimed (official-path spec §3.4), because
    /// official servers do not show it. The ten-minute timer still reads it.
    pub expires_at: Option<String>,
```

Replace:

```rust
    /// Always false on NodeWarden (slice 1 makes rosters manager-only).
    pub roster_visible_to_recipients: bool,
```

with:

```rust
    /// People a record is shared with can list each other (official
    /// Bitwarden's `mini-details` is open to every member). Decided by
    /// `/config`'s server name (official-path spec §3.4): false only for
    /// Vaultwarden and NodeWarden, which keep the roster to managers; an
    /// unnamed server is warned about, the direction that discloses.
    pub roster_visible_to_recipients: bool,
```

In `view`, replace:

```rust
        let now = now_millis();
        let mut view = SharingView {
            available: true,
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
            ..SharingView::default()
        };
```

with:

```rust
        let hides_roster = held
            .capabilities
            .server_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("vaultwarden") || name.eq_ignore_ascii_case("nodewarden"));
        let mut view = SharingView {
            available: true,
            roster_visible_to_recipients: !hides_roster,
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
            ..SharingView::default()
        };
```

and replace:

```rust
                    state: person_state(member, collection_id, &held, now),
```

with:

```rust
                    state: person_state(member, collection_id, &held),
```

Replace `person_state`:

```rust
fn person_state(member: Option<&Member>, collection_id: &str, held: &Held, now: i64) -> PersonState {
    let Some(member) = member else {
        return PersonState::Gone;
    };
    let email = normalise_email(&member.email);
    match member.status {
        MemberStatus::Invited => {
            if invitation_ran_out(member, now) { PersonState::Expired } else { PersonState::Invited }
        }
```

with:

```rust
fn person_state(member: Option<&Member>, collection_id: &str, held: &Held) -> PersonState {
    let Some(member) = member else {
        return PersonState::Gone;
    };
    let email = normalise_email(&member.email);
    match member.status {
        MemberStatus::Invited => PersonState::Invited,
```

(The rename in Step 3 already turned the old `PersonState::WaitingAccount` in that arm into `Invited`.)

Replace the doc of `invitation_ran_out`:

```rust
/// A pending invitation whose expiry has passed -- the card's `Expired`, and
/// what [`Sharing::wants_timer`] leaves out. A member past the invitation, or
/// one with no expiry the server named, is not expired.
```

with:

```rust
/// A pending invitation whose expiry has passed -- what [`Sharing::wants_timer`]
/// leaves out, since nothing it reads can move it. Not shown on the card (no
/// expiry is claimed). A member past the invitation, or one with no expiry
/// the server named, is not expired.
```

Replace the whole of `ensure_loaded` (DW1's), from:

```rust
    /// **The first pass's one question**, once per engine: who the account
```

through its closing `}` (the line before `    /// Refuses unless sharing is loaded and no route it needs has answered`). Write `$TEMP/dw/dw2-loaded.txt`:

```rust
    /// **The first pass's two questions**, once per engine: who the account
    /// is, and what the server calls itself. Sharing is "not loaded" until
    /// the first has an answer, and is then available on every server --
    /// unless a route it needs answers as missing ([`Self::route`],
    /// [`Self::fetch_roster`]), which turns it off with [`REASON_TOO_OLD`] for
    /// the session.
    ///
    /// This replaces the dialect probe (official-path spec §2). The name is
    /// `/config`'s `server.name`, read for one sentence -- the
    /// roster-visibility warning ([`SharingView::roster_visible_to_recipients`])
    /// -- and nothing else branches on it. A `/config` that does not answer
    /// leaves the server unnamed, which warns. A failure to read the account
    /// is not remembered: the next pass asks again.
    pub(crate) fn ensure_loaded(&self) -> Result<(), SharingError> {
        if self.held().capabilities.loaded {
            return Ok(());
        }
        self.account_id()?;
        let name = match self.call(|client, auth| sharing_api::server_name(client, &mut auth.session)) {
            Ok(name) => name,
            Err(e) => {
                log::info!("the server's /config could not be read for its name: {e}");
                None
            }
        };
        let mut held = self.held();
        held.capabilities.server_name = name;
        held.capabilities.loaded = true;
        Ok(())
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **The first pass'"'"'s one question**, once per engine: who the account' '    /// Refuses unless sharing is loaded and no route it needs has answered' "$TEMP/dw/dw2-loaded.txt"
```

In `create_org`, replace:

```rust
        let id = self
            .call(|client, auth| sharing_api::create_organization(client, &mut auth.session, &body))
            .map_err(|e| e.or_say(ORG_FAILED))?;
        Ok((id, org_key))
```

with:

```rust
        // **Refused, not unreachable**, is the server saying it will not make
        // one: an official self-hosted server creates an organisation only
        // from a license ([SelfHosted(NotSelfHostedOnly = true)], base spec
        // §4.3). The user makes it in the web vault and the next share adopts
        // it (official-path spec §3.1); the server's own words follow.
        let id = self
            .call(|client, auth| sharing_api::create_organization(client, &mut auth.session, &body))
            .map_err(|e| {
                if matches!(e, SharingError::Refused(_) | SharingError::Vault(VaultError::Http(_))) {
                    self.held().capabilities.creates_orgs = Some(false);
                    e.or_say(&needs_org(&account.email))
                } else {
                    e.or_say(ORG_FAILED)
                }
            })?;
        self.held().capabilities.creates_orgs = Some(true);
        Ok((id, org_key))
```

Add `needs_org` after `not_vouched_for`. Replace:

```rust
/// What a share ended as: the words the dialog chooses between (mock 6a).
```

with:

```rust
/// The Share dialog's sentence when the server will not create the sharing
/// org (official-path spec §3.1) -- an official self-hosted server, where an
/// organisation comes only from a license. The user makes it in the web
/// vault under this name, and the next share adopts it by name and
/// ownership.
#[must_use]
pub fn needs_org(email: &str) -> String {
    format!(
        "Your server needs an organisation for sharing. Create one named \"{}\" in the web vault, then try again.",
        crate::rest::organizations::sharing_org_name(email)
    )
}

/// What a share ended as: the words the dialog chooses between (mock 6a).
```

In `src/rest/sharing_api.rs`, after `user_public_key`, add `server_name`. Replace:

```rust
/// `POST /api/organizations`, the official body. Returns the new org's id.
```

with:

```rust
/// `GET /api/config` -- the server's own name, `server.name`. Vaultwarden
/// and NodeWarden fill it; official Bitwarden sends `null`, which is `None`
/// (official-path spec §3.4). Read once a session, for one sentence.
pub fn server_name(client: &RestClient, session: &mut Session) -> Result<Option<String>, RestError> {
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Get, "/api/config".into(), None))?;
    Ok(answer.get("server").and_then(|server| text_at(server, "name")))
}

/// `POST /api/organizations`, the official body. Returns the new org's id.
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/rest/sharing_api.rs`
Expected: both `0`.

- [ ] **Step 5: The card's and the dialog's words**

In `src/vault_window/share_ui.rs`, replace:

```rust
pub(crate) const SHARING_UNAVAILABLE: &str = "Sharing with a person needs a NodeWarden server.";
/// The greyed Share control's hover on a NodeWarden too old to share
/// (`server-too-old`, spec 7.1).
pub(crate) const SHARE_TOO_OLD_TOOLTIP: &str = "Sharing with a person needs a newer NodeWarden server.";
```

with:

```rust
pub(crate) const SHARING_UNAVAILABLE: &str = "Sharing with a person needs direct sync with a self-hosted server.";
/// The greyed Share control's hover on a server too old to share
/// (`server-too-old`, official-path spec §2): a route sharing needs answered
/// as missing. Any server can be updated, so the sentence names none.
pub(crate) const SHARE_TOO_OLD_TOOLTIP: &str = "Sharing with a person needs a newer server.";
/// The Share dialog's warning where people a record is shared with can list
/// each other (official-path spec §3.4) --
/// [`SharingView::roster_visible_to_recipients`].
pub(crate) const ROSTER_VISIBLE_WARNING: &str = "On this server, people you share with can see each other's email addresses.";
```

Replace:

```rust
/// A share that left an invitation for an address with no account, said as a
/// toast when no dialog is open to say it (mock 6a's dialog line, without
/// the expiry the report does not carry).
pub(crate) fn invited_toast(email: &str) -> String {
    format!("Waiting for {email} to create an account.")
}
```

with:

```rust
/// A share the server left as a pending invitation, said as a toast when no
/// dialog is open to say it. It claims nothing about an account
/// (official-path spec §3.4).
pub(crate) fn invited_toast(email: &str) -> String {
    format!("Waiting for {email} to accept.")
}
```

Replace in `PersonLine`:

```rust
            PersonState::WaitingAccept | PersonState::WaitingSignIn => theme::SharePillKind::Invited,
            PersonState::Invited => theme::SharePillKind::NoAccount,
            PersonState::Expired => theme::SharePillKind::Expired,
```

with:

```rust
            PersonState::WaitingAccept | PersonState::WaitingSignIn | PersonState::Invited => theme::SharePillKind::Invited,
```

and:

```rust
    /// Re-invite is offered where a fresh invitation helps (spec 6.1) -- and
    /// never for an unverified person, whose Re-invite the engine refuses
    /// anyway (`Sharing::reinvite`): it can wrap the org key to them.
    pub(crate) fn reinvite(&self) -> bool {
        self.authenticated && matches!(self.state, PersonState::Invited | PersonState::Expired | PersonState::Gone)
    }
```

with:

```rust
    /// Re-invite is offered where a fresh invitation helps -- a pending one,
    /// renewed with `POST …/users/{ouId}/reinvite` on every server, or a
    /// person who is gone (official-path spec §3.4) -- and never for an
    /// unverified person, whose Re-invite the engine refuses anyway
    /// (`Sharing::reinvite`): it can make them a recipient.
    pub(crate) fn reinvite(&self) -> bool {
        self.authenticated && matches!(self.state, PersonState::Invited | PersonState::Gone)
    }
```

Replace:

```rust
fn days_words(days: i64) -> String {
    if days == 1 { "1 day left".to_string() } else { format!("{days} days left") }
}
```

with:

```rust
/// "invited 3 days ago" -- how long a person has been waiting to accept
/// (official-path spec §3.4). From this PC's own record of the share, as
/// "since" is ([`since`]): no server says when it invited anyone. No expiry
/// is claimed, because the server's setting is invisible to a client.
fn invited_ago(at: i64, now_ms: i64) -> String {
    match (now_ms - at).max(0) / MILLIS_PER_DAY {
        0 => "invited today".to_string(),
        1 => "invited 1 day ago".to_string(),
        days => format!("invited {days} days ago"),
    }
}
```

In `person_lines`, replace:

```rust
                PersonState::WaitingAccept => match since(history, item_id, &person.email) {
                    Some(at) => format!("Waiting to accept \u{b7} invited {}", day(at, zone)),
                    None => "Waiting to accept".to_string(),
                },
                PersonState::Invited => match days_left(person.expires_at.as_deref(), now_ms) {
                    Some(days) => format!("Waiting to create an account \u{b7} {}", days_words(days)),
                    None => "Waiting to create an account".to_string(),
                },
                PersonState::Expired => match person.expires_at.as_deref().and_then(crate::send::parse_iso_utc_millis) {
                    Some(at) => format!("The invite ran out on {} \u{b7} they never created an account", day(at, zone)),
                    None => "The invite ran out \u{b7} they never created an account".to_string(),
                },
```

with:

```rust
                // Invited and Accepted read alike (official-path spec §3.4):
                // the accept is the person's, and an Accepted one is
                // confirmed on the next pass.
                PersonState::WaitingAccept | PersonState::Invited => match since(history, item_id, &person.email) {
                    Some(at) => format!("Waiting to accept \u{b7} {}", invited_ago(at, now_ms)),
                    None => "Waiting to accept".to_string(),
                },
```

Replace `short_state`:

```rust
pub(crate) fn short_state(state: PersonState, expires_at: Option<&str>, now_ms: i64) -> String {
    match state {
        PersonState::Shared => "Shared".to_string(),
        PersonState::WaitingAccept => "Waiting to accept".to_string(),
        PersonState::Invited => days_left(expires_at, now_ms).map_or_else(|| "No account yet".to_string(), days_words),
        PersonState::Expired => "Invite expired".to_string(),
        PersonState::KeyChanged => "Key changed".to_string(),
        PersonState::WaitingSignIn => "Waiting for a first sign-in".to_string(),
        PersonState::Gone => "Not receiving".to_string(),
    }
}
```

with:

```rust
pub(crate) fn short_state(state: PersonState) -> &'static str {
    match state {
        PersonState::Shared => "Shared",
        PersonState::WaitingAccept | PersonState::Invited => "Waiting to accept",
        PersonState::KeyChanged => "Key changed",
        PersonState::WaitingSignIn => "Waiting for a first sign-in",
        PersonState::Gone => "Not receiving",
    }
}
```

In `share_control`'s doc, replace:

```rust
/// - `server-too-old`: greyed, with [`SHARE_TOO_OLD_TOOLTIP`] -- NodeWarden,
///   so sharing is a thing this server will do once updated.
/// - `bw-serve`, `not-nodewarden`, `not-loaded`, any other reason, no view
///   yet, no line to the daemon, a received record: **nothing**. It used to
///   offer a Send on every server but NodeWarden; the controller's call is
```

with:

```rust
/// - `server-too-old`: greyed, with [`SHARE_TOO_OLD_TOOLTIP`] -- sharing is
///   a thing this server will do once updated.
/// - `bw-serve`, `not-loaded`, any other reason, no view yet, no line to the
///   daemon, a received record: **nothing**. It used to offer a Send where
///   sharing was not available; the controller's call is
```

Replace `share_rows`' signature and its one use of `now_ms`:

```rust
    history: &SentHistory,
    now_ms: i64,
    zone: &dyn LocalOffset,
) -> Vec<ShareRow> {
```

with:

```rust
    history: &SentHistory,
    zone: &dyn LocalOffset,
) -> Vec<ShareRow> {
```

and:

```rust
                    subtitle: format!(
                        "to {} \u{b7} {}",
                        person.email,
                        short_state(person.state, person.expires_at.as_deref(), now_ms)
                    ),
```

with:

```rust
                    subtitle: format!("to {} \u{b7} {}", person.email, short_state(person.state)),
```

Replace `no_account_band` (renamed `invited_band` in Step 3):

```rust
/// **The no-account band's two sentences** -- the bold line and the one
/// under it.
///
/// **NodeWarden sends no email** (plan decision 4), so this says what really
/// happens next: the person needs an invite code to sign up, and the band
/// never says an invitation was emailed, which is mock 6a's wording for the
/// official server's flow. An administrator is told they can make that code
/// here; anybody else, who makes it.
fn invited_band(email: &str, record: &str, admin: bool) -> (String, String) {
    let head = format!("Waiting for {email} to create an account \u{b7} the invitation lasts 7 days.");
    let who = if admin {
        "They need an invite code to sign up. You are this server's administrator, so you can \
         create one below and give it to them."
    } else {
        "They need an invite code from your server's administrator to sign up."
    };
    let body = format!(
        "{who} {record} reaches their vault after they sign up and accept, and you can re-invite \
         them from the record."
    );
    (head, body)
}
```

with:

```rust
/// **The invited band's two sentences** -- the bold line and the one under
/// it -- for a share the server left as a pending invitation.
///
/// It claims nothing about an account: no server tells a client whether an
/// address has one (official-path spec §3.4), and no expiry is claimed
/// either. Only NodeWarden's administrator -- the one account
/// [`SharingView::can_mint_invites`] is ever true for -- is told about the
/// invite code a newcomer there needs, and that they can make it here.
fn invited_band(email: &str, record: &str, admin: bool) -> (String, String) {
    let head = format!("Waiting for {email} to accept.");
    let code = if admin {
        "If they have no account on this server yet, they need an invite code to sign up. You are \
         this server's administrator, so you can create one below and give it to them. "
    } else {
        ""
    };
    let body = format!("{code}{record} reaches their vault once they accept, and you can re-invite them from the record.");
    (head, body)
}
```

Replace the band block's doc:

```rust
/// **6a's no-account band**, between the body and the footer: the amber of
/// `totp_add`'s 6c band rather than `theme::form_card_caution`'s red, and an
/// info mark rather than a warning, because nothing went wrong -- the share
/// went through and is waiting for somebody to sign up.
```

with:

```rust
/// **The invited band** (6a's fourth state), between the body and the footer:
/// the amber of `totp_add`'s 6c band rather than `theme::form_card_caution`'s
/// red, and an info mark rather than a warning, because nothing went wrong --
/// the share went through and is waiting for the person to accept.
```

Add the warning to the dialog. Replace:

```rust
                    ui.add(egui::Label::new(access_job(&dialog.item_name, width)).wrap());
                });
                // The last attempt's sentence -- the engine's refusal (a
```

with:

```rust
                    ui.add(egui::Label::new(access_job(&dialog.item_name, width)).wrap());
                });
                // **The roster-visibility warning** (official-path spec §3.4):
                // where members may read the roster, everyone a record is
                // shared with can list everyone else, and no request this app
                // makes can stop it. Decided by the server's own name
                // (`SharingView::roster_visible_to_recipients`).
                if view.is_some_and(|v| v.roster_visible_to_recipients) {
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(egui::RichText::new(ROSTER_VISIBLE_WARNING).size(12.0).color(theme::TEXT_MUTED)).wrap());
                }
                // The last attempt's sentence -- the engine's refusal (a
```

In `src/vault_window/mod.rs`, replace:

```rust
            &sent_history,
            crate::send::SendClock::now_unix_millis(&crate::send::SystemClock),
            &crate::local_time::SystemZone,
        );
        // **Design 6d's FROM PEOPLE**, built beside them for the same reason:
```

with:

```rust
            &sent_history,
            &crate::local_time::SystemZone,
        );
        // **Design 6d's FROM PEOPLE**, built beside them for the same reason:
```

Replace in `permit_reinvite`'s doc:

```rust
/// For a person who never got in -- no account yet, an invite that ran out
/// -- a fresh invitation hands nothing new over: they were already meant to
```

with:

```rust
/// For a person who never got in -- an invitation still waiting -- a fresh
/// invitation hands nothing new over: they were already meant to
```

In `src/vault_window/detail.rs`, replace:

```rust
    /// A fresh invitation for a person whose invite ran out, or who has no
    /// account yet (spec 6.1).
    Reinvite(String),
```

with:

```rust
    /// A fresh invitation for a person still invited, or one who is gone
    /// (official-path spec §3.4).
    Reinvite(String),
```

Run: `python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/mod.rs src/vault_window/detail.rs`
Expected: three `0`s.

Run: `cargo check -j 2 2>&1 | grep -E "^(error|warning)"`
Expected: no output. If `days_left` is reported unused, that is wrong for this task: `incoming_rows` still calls it until DW3. Check that the `incoming_rows` code is unchanged.

- [ ] **Step 6: Bring the window's tests to the new words**

`share_ui.rs`, `each_persons_line_says_where_they_stand_in_the_mocks_words`: write `$TEMP/dw/dw2-t-lines.txt`:

```rust
    /// **Every status line**, in its words (mock 6e, and official-path spec
    /// §3.4 for the waits): Invited and Accepted both "Waiting to accept",
    /// with how long ago where this PC shared it; nothing about an account
    /// or an expiry.
    #[test]
    fn each_persons_line_says_where_they_stand_in_the_mocks_words() {
        let zone = crate::local_time::FixedOffset(0);
        let v = view(
            vec![
                ("anna@x.io", PersonState::Shared, None),
                ("bob@x.io", PersonState::WaitingAccept, None),
                ("carol@x.io", PersonState::Invited, Some("2999-01-01T00:00:00Z")),
                ("dan@x.io", PersonState::Invited, Some("2000-01-01T00:00:00Z")),
                ("eve@x.io", PersonState::KeyChanged, None),
                ("fay@x.io", PersonState::WaitingSignIn, None),
                ("gus@x.io", PersonState::Gone, None),
            ],
            vec![("r1", vec!["anna@x.io", "bob@x.io", "carol@x.io", "dan@x.io", "eve@x.io", "fay@x.io", "gus@x.io"])],
        );
        let mut history = crate::sent_history::SentHistory::default();
        for (who, ago) in [("anna@x.io", 7 * DAY), ("carol@x.io", 6 * DAY), ("bob@x.io", DAY)] {
            history.record(crate::sent_history::SentRecord {
                kind: crate::sent_history::SentKind::Share,
                item_id: "r1".into(),
                recipient: who.into(),
                sent_at_unix_millis: NOW - ago,
                ..Default::default()
            });
        }
        let lines = person_lines(Some(&v), "r1", &history, NOW, &zone);
        let line = |email: &str| lines.iter().find(|l| l.email == email).map(|l| l.line.clone()).expect("a line");
        assert_eq!(line("anna@x.io"), "Shared \u{b7} since 14 Sep");
        assert_eq!(line("bob@x.io"), "Waiting to accept \u{b7} invited 1 day ago");
        assert_eq!(line("carol@x.io"), "Waiting to accept \u{b7} invited 6 days ago");
        assert_eq!(line("dan@x.io"), "Waiting to accept", "no record of the share here, and no expiry claimed");
        assert_eq!(line("eve@x.io"), "Their key changed. Check the fingerprint before sharing.");
        assert_eq!(line("fay@x.io"), "Waiting for them to sign in once to a Bitwarden app");
        assert_eq!(line("gus@x.io"), "No longer receiving this");
        for said in lines.iter().map(|l| l.line.to_lowercase()) {
            assert!(!said.contains("account") && !said.contains("expire") && !said.contains("ran out"), "{said}");
        }
        let pill = |email: &str| lines.iter().find(|l| l.email == email).map(PersonLine::pill).expect("a pill");
        assert_eq!(pill("anna@x.io"), theme::SharePillKind::Shared);
        assert_eq!(pill("carol@x.io"), theme::SharePillKind::Invited);
        assert!(lines.iter().find(|l| l.email == "anna@x.io").is_some_and(PersonLine::fingerprint));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.fingerprint()), "no key held yet (mock 6b)");
        assert!(lines.iter().find(|l| l.email == "dan@x.io").is_some_and(PersonLine::reinvite));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.reinvite()), "an Accepted person has nothing to re-accept");
        assert!(lines.iter().find(|l| l.email == "eve@x.io").is_some_and(PersonLine::share_anyway));
        assert_eq!(short_state(PersonState::Shared), "Shared");
        assert_eq!(short_state(PersonState::Invited), "Waiting to accept");
    }

```

```bash
python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '    /// **Every status line of mock 6e**, in its words.' '    /// **The Share control is decided by the view'"'"'s reason** (final review,' "$TEMP/dw/dw2-t-lines.txt"
```

Then these single-line edits, each with the Edit tool, in `share_ui.rs`:

1. In `a_share_answer_with_the_dialog_closed_still_says_where_it_stands`, replace:

   ```rust
           assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Waiting for carol@x.io to create an account."));
   ```

   with:

   ```rust
           assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Waiting for carol@x.io to accept."));
   ```

2. In `a_person_without_an_account_is_said_and_an_admin_can_make_an_invite_code`, replace:

   ```rust
           assert!(has("Waiting for carol@example.com to create an account"), "{texts:?}");
   ```

   with:

   ```rust
           assert!(has("Waiting for carol@example.com to accept."), "{texts:?}");
   ```

3. Replace the band test whole. Write `$TEMP/dw/dw2-t-band.txt`:

   ```rust
       /// **The invited band claims no account either way** (official-path spec
       /// §3.4) and never says an email went out: an administrator is told
       /// about the invite code a newcomer to this server needs; anyone else is
       /// told only that the person has to accept.
       #[test]
       fn the_invited_band_claims_no_account_and_no_email() {
           for admin in [false, true] {
               let (head, body) = invited_band("carol@example.com", "Acme VPN", admin);
               let all = format!("{head} {body}").to_lowercase();
               assert!(!all.contains("email") && !all.contains("e-mail"), "{all}");
               assert!(!all.contains("create an account") && !all.contains("7 days"), "{all}");
               assert_eq!(head, "Waiting for carol@example.com to accept.");
               assert!(body.contains("Acme VPN"), "{body}");
               assert_eq!(all.contains("invite code"), admin, "{all}");
           }
       }

   ```

   ```bash
   python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '    /// **NodeWarden sends no email** (plan decision 4), so the band never' '    /// **An invite code, once made, is on the band with a Copy** -- and the' "$TEMP/dw/dw2-t-band.txt"
   ```

4. In `lines()`, replace:

   ```rust
               PersonLine { email: "carol@x.io".into(), state: PersonState::Invited, line: "Waiting to create an account \u{b7} 6 days left".into(), authenticated: true },
   ```

   with:

   ```rust
               PersonLine { email: "carol@x.io".into(), state: PersonState::Invited, line: "Waiting to accept \u{b7} invited 6 days ago".into(), authenticated: true },
   ```

5. In `the_people_block_draws_each_person_with_their_controls`, replace:

   ```rust
           assert_eq!(count("No account yet"), 1);
   ```

   with:

   ```rust
           assert_eq!(count("Invited"), 1, "carol's pill");
   ```

6. Replace `an_expired_invite_can_be_renewed` whole. Write `$TEMP/dw/dw2-t-renew.txt`:

   ```rust
       /// **An invited person can be re-invited**: Re-invite and Stop sharing
       /// stay, no fingerprint is offered, and Re-invite reports the person.
       #[test]
       fn an_invited_person_can_be_reinvited() {
           use super::super::detail::DetailAction;
           let people_lines = vec![PersonLine {
               email: "carol@x.io".into(),
               state: PersonState::Invited,
               line: "Waiting to accept \u{b7} invited 6 days ago".into(),
               authenticated: true,
           }];
           let people = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
           let (_, texts) = block_frame(people, Vec::new());
           assert!(texts.iter().any(|(t, _)| t == "Invited"));
           assert!(!texts.iter().any(|(t, _)| t == SHOW_FINGERPRINT), "no key is held for an invitation");
           let reinvite = centre_of(&texts, REINVITE_LABEL);
           assert_eq!(block_frame(people, click(reinvite)).0, DetailAction::Reinvite("carol@x.io".into()));
       }

   ```

   ```bash
   python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '    /// **Mock 6b, invite expired**: Re-invite and Stop sharing stay, and' '    /// **A person known only from a plaintext name is shown, marked, and' "$TEMP/dw/dw2-t-renew.txt"
   ```

7. The four `share_rows` calls lose `NOW`. Write `$TEMP/dw_share_rows.py`:

   ```python
   # Drops the `now_ms` argument from share_rows(...) calls in share_ui's tests.
   path = "src/vault_window/share_ui.rs"
   data = open(path, "rb").read()
   old = b", NOW, &crate::local_time::FixedOffset(0))"
   lines = data.split(b"\r\n")
   n = 0
   for i, line in enumerate(lines):
       if b"share_rows(" in line and old in line:
           lines[i] = line.replace(old, b", &crate::local_time::FixedOffset(0))")
           n += 1
   open(path, "wb").write(b"\r\n".join(lines))
   print("share_rows calls changed:", n)
   ```

   Run: `python "$TEMP/dw_share_rows.py"`
   Expected: `share_rows calls changed: 4`.

`src/vault_window/detail.rs`, test `at_the_narrowest_pane_a_persons_controls_stay_on_the_card`: replace:

```rust
            line: "Waiting to create an account \u{b7} 6 days left".to_string(),
```

with:

```rust
            line: "Waiting to accept \u{b7} invited 6 days ago".to_string(),
```

`src/vault_window/mod.rs`, the `permit_reinvite` test: replace:

```rust
        for waiting in [PersonState::Invited, PersonState::Expired] {
```

with:

```rust
        for waiting in [PersonState::Invited] {
```

Run: `python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/detail.rs src/vault_window/mod.rs`
Expected: three `0`s.

Run: `grep -rn "create an account\|Expired\b\|days_words\|WaitingAccount" src --include=*.rs | grep -v "^src/theme.rs"`
Expected: no output. `theme::SharePillKind` keeps its `NoAccount` and `Expired` styles. They are public, so they raise no warning, and `theme`'s own test still draws them. They are no longer used by any card.

- [ ] **Step 7: Add the dialog-warning test**

In `share_ui.rs`, after `a_person_without_an_account_is_said_and_an_admin_can_make_an_invite_code`, add this test. Write `$TEMP/dw/dw2-t-warning.txt`:

```rust
    /// **The Share dialog warns where recipients can see each other**
    /// (official-path spec §3.4), and only there.
    #[test]
    fn the_share_dialog_warns_where_recipients_can_see_each_other() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let official = SharingView { available: true, roster_visible_to_recipients: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&official), Vec::new());
        assert!(texts.iter().any(|(t, _)| t == ROSTER_VISIBLE_WARNING), "{texts:?}");
        let hidden = SharingView { available: true, roster_visible_to_recipients: false, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&hidden), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == ROSTER_VISIBLE_WARNING), "{texts:?}");
    }

```

```bash
python - "$TEMP/dw/dw2-t-warning.txt" <<'EOF'
import sys
path = "src/vault_window/share_ui.rs"
marker = "    /// **An administrator is offered a code only for somebody with no".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, add + marker, 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

Run: `python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs`
Expected: `0`.

- [ ] **Step 8: Run the tests to see them pass**

Run: `cargo test --lib -j 2 sharing 2>&1 | tail -5`
Expected: `test result: ok.`

Run: `cargo test --lib -j 2 share_ui 2>&1 | tail -5`
Expected: `test result: ok.`

- [ ] **Step 9: Verify the four commands**

Run each of the four commands in Global Constraints › Deskwarden › Verify per task.
Expected: all pass, 0 warnings.

Run: `grep -r --include=*.rs -o "#\[test\]" src | wc -l`
Expected: `6135`.

- [ ] **Step 10: Commit**

```bash
cd /e/Personal/node-bitwarden
git add deskwarden/src/rest/sharing.rs deskwarden/src/rest/sharing_api.rs deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/mod.rs deskwarden/src/vault_window/detail.rs deskwarden/src/daemon_link.rs
git commit -F - <<'EOF'
The sharing card says waiting to accept, and claims no account and no expiry

A pending invitation was "Waiting to create an account · 6 days left",
which only NodeWarden could know; no official route says whether an
invited address has an account, and the server's invitation lifetime
is invisible to a client. Invited and Accepted now both read "Waiting
to accept · invited N days ago", from this PC's own record of the
share, and the expired state is gone. Re-invite is offered for an
invitation and for a person who is gone.

The Share dialog warns "On this server, people you share with can see
each other's email addresses." unless /config names the server
Vaultwarden or NodeWarden, which keep the roster to managers; the name
is read once per session. A server that will not create the sharing
org -- official self-hosted -- is answered with "Create one named
"Shared by <email>" in the web vault, then try again." The
server-too-old hover no longer names NodeWarden.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task DW3: Paste an invite link, and no invitation list

**Files:**
- Create: `src/invite_link.rs`
- Modify: `src/lib.rs:79` (after `pub mod injector;`), `src/foreground.rs:1423` and `:1535` (`OPENS_NO_WINDOW`)
- Modify: `src/rest/sharing_api.rs`:
  - module doc, imports;
  - `IncomingInvitation` (`:112-134`), `my_invitations` (`:205-233`), `route_absent` (`:80-83`), `accept_invitation`, `decline_invitation` (`:500-518`);
  - tests `my_invitations_are_read_and_a_missing_route_is_none`.
- Modify: `src/rest/sharing.rs`:
  - module doc;
  - `IncomingEntry` (`:121-134`), `SharingView.incoming`;
  - `Held` (`:544-585`);
  - `view`'s incoming block (`:802-815`);
  - `refresh`;
  - `is_personal_share` (`:1100-1113`);
  - the recipient section (`:2624-2707`);
  - tests: `held_debug_never_prints_an_invitation_token`, `a_failed_invitations_read_does_not_skip_a_half_finished_move`, `an_elevated_role_invitation_is_not_listed_or_accepted_as_a_share`, and `invited` through `a_recipient_declines_an_invitation_by_its_token`.
- Modify: `src/vault_api.rs:335-336` (routes), `:643-662` (`incoming`), `:837-844` (body type), test `:1243-1270`
- Modify: `src/daemon_link.rs:412-439` (`accept`, `decline`), `:455-457` (`INVITATION_UNUSABLE`), test `:672-695`
- Modify: `src/vault_window/share_ui.rs`:
  - `:171-174` (`days_left`);
  - `:473-494` (`SharingPane` notes, `pin_notice_to`);
  - `:698-853` (calls and reports);
  - `:1170-1176` (a comment);
  - `:2533-2587` (`PeopleRows`, `PeopleAsk`);
  - `:2720-3009` (the invitation block);
  - tests `:3332`, `:3361`, `:3419-3432`, `:4513-4615`, `:4653-4710`, `:4727-4741`.
- Modify: `src/vault_window/send_ui.rs`:
  - `:1903-1913` (note);
  - `:1976-1978` (comment);
  - `:2050-2113` (Shared with me's list);
  - `:2905-2916` (detail dispatch);
  - `:3599-3731` (`draw_invitation_pane`);
  - test `:9424-9481`.
- Modify: `src/vault_window/mod.rs:3383-3392`, `:3454`, `:4502-4528`, `:6690-6713`
- Modify: `src/vault_host.rs:723,770,820,870` (test mocks)

**Interfaces:**
- Consumes:
  - NodeWarden's invitation link format (NW4);
  - Bitwarden's `POST …/users/{ouId}/accept {token}`;
  - `Sharing::require_available` and `Capabilities` (DW1).
- Produces:
  - In `crate::invite_link`:
    - `pub struct InviteLink { pub organization_id: String, pub organization_user_id: String, pub email: String, pub organization_name: Option<String>, pub token: Zeroizing<String> }`, with a hand-written `Debug`;
    - `pub enum LinkError { NotAnInviteLink, Missing(&'static str), NotAnId(&'static str) }` and `impl LinkError { pub fn sentence(&self) -> &'static str }`;
    - `pub fn parse(text: &str) -> Result<InviteLink, LinkError>`.
  - `sharing_api::accept_invite(client: &RestClient, session: &mut Session, org: &str, ou_id: &str, token: &str) -> Result<(), RestError>`.
  - In `crate::rest::sharing`: `Sharing::accept_link(&self, link: &str) -> Result<(), SharingError>`, `pub const LINK_ACCEPT_FAILED: &str`, and `pub fn link_for_someone_else(link_email: &str, account_email: &str) -> String`.
  - The daemon route `POST /sharing/incoming/paste {link}`, and `SharingClient::accept_link(&self, link: &str) -> Result<(), String>`.
  - In `share_ui`:
    - `SharingCall::AcceptLink(Zeroizing<String>)`, `SharingReport::LinkAccepted(Result<(), String>)` (the old `Done`, renamed), and `PeopleAsk::AcceptLink`;
    - `PeopleRows.paste: Option<Zeroizing<String>>` and `SharingPane.paste: Zeroizing<String>`;
    - `draw_paste_and_waiting(ui, people, width)` and `paste_and_waiting_height(people, gap) -> f32`;
    - `PASTE_HINT`, `PASTE_ACCEPT_LABEL`, `LINK_ACCEPTED`, `PASTE_ROW_HEIGHT`.
  - Removed:
    - `Sharing::accept`, `Sharing::decline`, `INVITATION_GONE`;
    - `IncomingEntry`, `SharingView::incoming`;
    - `sharing_api::{IncomingInvitation, my_invitations, accept_invitation, decline_invitation, route_absent}`;
    - `SharingClient::{accept, decline}`;
    - `IncomingRow` and everything in the invitation block;
    - `SharingPane::pin_notice_to`.
- Test count:
  - `invite_link` +9;
  - `sharing.rs` −6 +3;
  - `sharing_api` −1 +1;
  - `share_ui` −9 +4;
  - `send_ui` ±0;
  - `daemon_link` ±0;
  - `foreground` ±0;
  - total +1 (6135 → 6136).

- [ ] **Step 1: Write the parser's tests**

Create `src/invite_link.rs` with the Write tool, holding only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ORG: &str = "6f8a3c2e-1b4d-4e5f-9a7b-0c1d2e3f4a5b";
    const OU: &str = "a1b2c3d4-e5f6-4789-8abc-def012345678";

    /// **A link exactly as Bitwarden's server writes it**
    /// (`HandlebarsMailService.BuildInvitationUrl`): the query in the
    /// fragment, `email` and `organizationName` through .NET's
    /// `WebUtility.UrlEncode` (a space is `+`), a DataProtector token, the
    /// booleans as C# prints them.
    #[test]
    fn a_link_from_a_bitwarden_email_is_read() {
        let link = format!(
            "https://vault.bitwarden.com/#/accept-organization?organizationId={ORG}&organizationUserId={OU}\
             &email=Anna%40Example.com&organizationName=Shared+by+denis%40example.com\
             &token=BwOrgUserInviteToken_CfDJ8Kx-Q9vR3pZ_e2A7hT0wLmN1yF4uB6cD8gH0jK2lM4nP6qS8tU0vW2xY4zA6bC8dE0fG2hI4\
             &initOrganization=False&orgUserHasExistingUser=True"
        );
        let read = parse(&link).expect("a Bitwarden link");
        assert_eq!(read.organization_id, ORG);
        assert_eq!(read.organization_user_id, OU);
        assert_eq!(read.email, "anna@example.com", "lower-cased");
        assert_eq!(read.organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(
            read.token.as_str(),
            "BwOrgUserInviteToken_CfDJ8Kx-Q9vR3pZ_e2A7hT0wLmN1yF4uB6cD8gH0jK2lM4nP6qS8tU0vW2xY4zA6bC8dE0fG2hI4"
        );
    }

    /// **Vaultwarden's link** (`mail.rs` `send_invite`): a `/` before the `?`,
    /// `email` first, `%20` rather than `+`, a JWT token, and the extra keys
    /// it adds.
    #[test]
    fn a_link_from_a_vaultwarden_email_is_read() {
        let link = format!(
            "https://vw.example.org/#/accept-organization/?email=anna%40example.com&organizationName=Shared%20by%20denis%40example.com\
             &organizationId={ORG}&organizationUserId={OU}&token=eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln\
             &initOrganization=false&orgUserHasExistingUser=true"
        );
        let read = parse(&link).expect("a Vaultwarden link");
        assert_eq!(read.organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(read.token.as_str(), "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln");
        assert_eq!(read.email, "anna@example.com");
    }

    /// **NodeWarden's link** (`invite-mail.ts` `invitationUrl`): Bitwarden's
    /// format, with NodeWarden's own token.
    #[test]
    fn a_link_from_a_nodewarden_email_is_read() {
        let token = "0f1e2d3c-4b5a-4968-8776-655443322110a9b8c7d6-e5f4-4321-8fed-cba987654321";
        let link = format!(
            "https://nw.example.net/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=anna%40example.com\
             &organizationName=Shared+by+denis%40example.com&token={token}&initOrganization=False&orgUserHasExistingUser=False"
        );
        assert_eq!(parse(&link).expect("a NodeWarden link").token.as_str(), token);
    }

    /// **Keys in any case, and a query outside the fragment**: a link a mail
    /// client rewrote, or one copied from a web vault that routes by path.
    #[test]
    fn keys_in_any_case_and_a_query_outside_the_fragment_are_read() {
        let link = format!(
            "https://vault.example.com/accept-organization?ORGANIZATIONID={}&OrganizationUserId={OU}&EMAIL=anna%40example.com&TOKEN=t-1",
            ORG.to_uppercase()
        );
        let read = parse(&link).expect("read");
        assert_eq!(read.organization_id, ORG, "a GUID is compared lower-cased");
        assert_eq!(read.organization_name, None, "the name is not required");
        assert_eq!(read.token.as_str(), "t-1");
    }

    /// **What a paste carries around the link** -- spaces, a newline, angle
    /// brackets some mail clients add -- is not part of it.
    #[test]
    fn a_pasted_link_is_trimmed() {
        let link = format!("  <https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=a%40b.c&token=t>\r\n");
        assert_eq!(parse(&link).expect("read").email, "a@b.c");
    }

    /// **Not an invite link**: text, another vault page, an
    /// `accept-organization` with no query.
    #[test]
    fn what_is_not_an_invite_link_is_refused() {
        for text in ["", "hello", "https://vault.example/#/vault?organizationId=x", "https://vault.example/#/accept-organization"] {
            assert!(matches!(parse(text), Err(LinkError::NotAnInviteLink)), "{text:?}");
        }
        assert_eq!(
            LinkError::NotAnInviteLink.sentence(),
            "That isn't an organisation invite link. Paste the whole link from the invitation email."
        );
    }

    /// **A link cut short** is refused for what it lacks.
    #[test]
    fn a_link_missing_a_part_is_refused() {
        let whole = format!("https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=a%40b.c&token=t");
        for (cut, part) in [("&token=t", "token"), ("&email=a%40b.c", "email")] {
            assert_eq!(parse(&whole.replace(cut, "")).err(), Some(LinkError::Missing(part)), "{part}");
        }
        assert_eq!(parse(&whole.replace("a%40b.c", "nobody")).err(), Some(LinkError::Missing("email")), "an email has an @");
        assert_eq!(parse(&whole.replace("token=t", "token=")).err(), Some(LinkError::Missing("token")));
        assert_eq!(
            LinkError::Missing("token").sentence(),
            "That invite link is incomplete. Paste the whole link from the invitation email."
        );
    }

    /// **Both ids must be GUIDs**, as the official web vault requires
    /// (`DirectOrganizationInvite.fromUrlParams`'s `isId`) -- and as a path
    /// segment needs: nothing else reaches a route.
    #[test]
    fn ids_that_are_not_guids_are_refused() {
        let with = |org: &str, ou: &str| format!("https://v.example/#/accept-organization?organizationId={org}&organizationUserId={ou}&email=a%40b.c&token=t");
        assert_eq!(parse(&with("../x", OU)).err(), Some(LinkError::NotAnId("organizationId")));
        assert_eq!(parse(&with(ORG, "a1b2c3d4")).err(), Some(LinkError::NotAnId("organizationUserId")));
        assert_eq!(parse(&with(ORG, "a1b2c3d4-e5f6-4789-8abc-def01234567z")).err(), Some(LinkError::NotAnId("organizationUserId")));
    }

    /// **A link's `Debug` prints no token, no email and no id** -- the token
    /// accepts the invitation, and nothing that debugs a link needs the rest.
    #[test]
    fn a_links_debug_prints_no_token_and_no_address() {
        let link = format!(
            "https://v.example/#/accept-organization?organizationId={ORG}&organizationUserId={OU}&email=anna%40example.com&token=TOKEN-NEEDLE-31b"
        );
        let printed = format!("{:?}", parse(&link).expect("read"));
        for secret in ["TOKEN-NEEDLE-31b", "anna@example.com", ORG, OU] {
            assert!(!printed.contains(secret), "{printed}");
        }
    }
}
```

Run: `python "$TEMP/dw_crlf.py" --fix src/invite_link.rs`
Expected: `src/invite_link.rs 0`.

In `src/lib.rs`, replace:

```rust
pub mod injector;
```

with:

```rust
pub mod injector;
/// A pasted organisation invite link -- Bitwarden's, Vaultwarden's or
/// NodeWarden's -- read into the ids and token Bitwarden's accept route takes.
pub mod invite_link;
```

In `src/foreground.rs`, replace:

```rust
        const OPENS_NO_WINDOW: [&str; 89] = [
```

with:

```rust
        const OPENS_NO_WINDOW: [&str; 90] = [
```

and replace:

```rust
            "injector",
            // The chosen-icon custom field: its shape, its parse, its size
```

with:

```rust
            "injector",
            // A pasted invite link, read: text in, two ids, an email and a
            // token out. It draws nothing -- the paste box is Shared with me's,
            // in `vault_window::share_ui`.
            "invite_link",
            // The chosen-icon custom field: its shape, its parse, its size
```

Run: `python "$TEMP/dw_crlf.py" src/lib.rs src/foreground.rs`
Expected: both `0`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --lib -j 2 invite_link 2>&1 | tail -5`
Expected: compile errors, ``cannot find function `parse` `` and ``cannot find type `LinkError` ``.

- [ ] **Step 3: Write the parser**

Add the module's code ABOVE its test module. Write `$TEMP/dw/dw3-invite-link.txt`:

```rust
//! **An organisation invite link, read** (official-path spec §4.1) -- what
//! Shared with me's Paste an invite link hands the daemon, which accepts with
//! it on Bitwarden's own route, `POST /organizations/{organizationId}/users/
//! {organizationUserId}/accept {token}`.
//!
//! # The format
//!
//! Bitwarden's server writes the link in `HandlebarsMailService.
//! BuildInvitationUrl` (bitwarden/server main, src/Core/Platform/Mail/
//! HandlebarsMailService.cs, lines 875-893):
//!
//! `{vault}/#/accept-organization?organizationId=…&organizationUserId=…&email=…&organizationName=…&token=…&initOrganization=True|False&orgUserHasExistingUser=True|False`
//!
//! -- the query inside the FRAGMENT; `email`, `organizationName` and `token`
//! through .NET's `WebUtility.UrlEncode`, where a space is `+`. NodeWarden
//! writes the same. Vaultwarden writes `…/#/accept-organization/?…` with
//! `email` first and `%20` for a space. So this reads the query wherever it
//! stands -- the fragment's, or the URL's own -- under a path that names
//! `accept-organization`, with keys in any case and values form-decoded.
//! Both ids must be GUIDs, as the official web vault's
//! `DirectOrganizationInvite.fromUrlParams` requires, which also keeps any
//! other text out of a route's path.
//!
//! # The token
//!
//! It is what accepts the invitation, so it is held in a `Zeroizing<String>`,
//! and [`InviteLink`]'s `Debug` prints no part of it -- nor the email or the
//! ids, which nothing that debugs a link needs. Nothing here logs.
//!
//! Pure: no I/O, no clock, no window.

use zeroize::Zeroizing;

/// One invite link, read. See the module docs.
pub struct InviteLink {
    /// A GUID, lower-cased.
    pub organization_id: String,
    /// The invitation's org-user id: a GUID, lower-cased.
    pub organization_user_id: String,
    /// The address invited, trimmed and lower-cased -- what the daemon
    /// compares with the signed-in account's before it asks the server.
    pub email: String,
    /// The organisation's name, as the link carries it. Plaintext anyone
    /// could have set: shown as a claim, never trusted.
    pub organization_name: Option<String>,
    pub token: Zeroizing<String>,
}

/// **Hand-written**: the token accepts the invitation, and the email and the
/// ids say whom it is for. Only the organisation's name -- the one part a
/// person reading a log could want, and plaintext on every server -- is shown.
impl std::fmt::Debug for InviteLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteLink").field("organization_name", &self.organization_name).finish_non_exhaustive()
    }
}

/// Why a pasted text is not a link this can accept with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// No `accept-organization` path with a query: some other text or page.
    NotAnInviteLink,
    /// The link lacks this key, or it is empty (an email without an `@`
    /// counts as missing).
    Missing(&'static str),
    /// This id is not a GUID.
    NotAnId(&'static str),
}

impl LinkError {
    /// The sentence Shared with me shows. A link cut short and a link with a
    /// bad id read alike: either way, the fix is to paste the whole link.
    #[must_use]
    pub fn sentence(&self) -> &'static str {
        match self {
            Self::NotAnInviteLink => {
                "That isn't an organisation invite link. Paste the whole link from the invitation email."
            }
            Self::Missing(_) | Self::NotAnId(_) => {
                "That invite link is incomplete. Paste the whole link from the invitation email."
            }
        }
    }
}

/// A GUID as Bitwarden writes one: 8-4-4-4-12 hex digits.
fn is_guid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// **Reads a pasted invite link.** See the module docs for the formats.
///
/// # Errors
///
/// [`LinkError`]: not an invite link, a part missing, or an id that is not a
/// GUID.
pub fn parse(text: &str) -> Result<InviteLink, LinkError> {
    let text = text.trim().trim_start_matches('<').trim_end_matches('>').trim();
    let (outside, fragment) = match text.split_once('#') {
        Some((outside, fragment)) => (outside, Some(fragment)),
        None => (text, None),
    };
    // The fragment's query first (Bitwarden, Vaultwarden, NodeWarden), else
    // the URL's own.
    let (path, query) = match fragment.and_then(|fragment| fragment.split_once('?')) {
        Some(found) => found,
        None => outside.split_once('?').ok_or(LinkError::NotAnInviteLink)?,
    };
    if !path.to_ascii_lowercase().contains("accept-organization") {
        return Err(LinkError::NotAnInviteLink);
    }
    let (mut organization_id, mut organization_user_id, mut email, mut organization_name, mut token) = (None, None, None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "organizationid" => organization_id = Some(value.to_ascii_lowercase()),
            "organizationuserid" => organization_user_id = Some(value.to_ascii_lowercase()),
            "email" => email = Some(value.to_lowercase()),
            "organizationname" => organization_name = Some(value.to_string()).filter(|name| !name.is_empty()),
            "token" => token = Some(Zeroizing::new(value.to_string())),
            _ => {}
        }
    }
    let organization_id = organization_id.filter(|id| !id.is_empty()).ok_or(LinkError::Missing("organizationId"))?;
    let organization_user_id = organization_user_id.filter(|id| !id.is_empty()).ok_or(LinkError::Missing("organizationUserId"))?;
    let email = email.filter(|email| email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty()));
    let email = email.ok_or(LinkError::Missing("email"))?;
    let token = token.filter(|token| !token.is_empty()).ok_or(LinkError::Missing("token"))?;
    if !is_guid(&organization_id) {
        return Err(LinkError::NotAnId("organizationId"));
    }
    if !is_guid(&organization_user_id) {
        return Err(LinkError::NotAnId("organizationUserId"));
    }
    Ok(InviteLink { organization_id, organization_user_id, email, organization_name, token })
}

```

Then prepend it to the file:

```bash
python - "$TEMP/dw/dw3-invite-link.txt" <<'EOF'
import sys
path = "src/invite_link.rs"
head = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
body = open(path, "rb").read()
assert body.startswith(b"#[cfg(test)]"), "the file should hold only its tests so far"
open(path, "wb").write(head + body)
print("prepended")
EOF
python "$TEMP/dw_crlf.py" src/invite_link.rs
```

Expected: `prepended`, then `src/invite_link.rs 0`.

`a_link_missing_a_part_is_refused` checks `Missing("token")` when the token is cut, and `Missing("email")` when the email is. The checks run in the order the code reads them: `organizationId`, `organizationUserId`, `email`, then `token`. A link that lacks both email and token therefore reports the email.

- [ ] **Step 4: Run the parser's tests**

Run: `cargo test --lib -j 2 invite_link 2>&1 | tail -5`
Expected: `test result: ok. 9 passed`.

Run: `cargo test --lib -j 2 every_module_in_this_crate_is_classified_as_opening_windows_or_not`
Expected: PASS.

- [ ] **Step 5: The official accept route, and no invitation list**

In `src/rest/sharing_api.rs`, replace:

```rust
//! Two NodeWarden routes remain, each said where it is: the recipient's
//! invitation list (until the paste-a-link accept replaces it) and the
//! administrator's registration code.
```

with:

```rust
//! One NodeWarden route remains, and says so: the administrator's
//! registration code. There is no invitation list -- no official server has
//! one -- and an invitation is accepted from its link ([`accept_invite`]).
```

Replace:

```rust
use serde_json::{Value, json};
use zeroize::Zeroizing;
```

with:

```rust
use serde_json::{Value, json};
```

Delete `route_absent`. Replace:

```rust
/// A server without this route: its whole answer is "not here".
fn route_absent(error: &RestError) -> bool {
    matches!(error, RestError::Status(404 | 405 | 501))
}

```

with nothing.

Delete `IncomingInvitation`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '/// An invitation addressed to this account (`GET /invitations/mine`).' '/// An account'"'"'s id and public key: what a confirm wraps the org key to, and'
```

Delete `my_invitations`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '/// `GET /api/organizations/invitations/mine` -- NodeWarden'"'"'s, and the dialect' '/// `GET /api/users/{userId}/public-key` -- slice 1'"'"'s second probe, and the'
```

Replace `accept_invitation` and `decline_invitation`:

```rust
/// `POST /api/organizations/invitations/accept {token}`.
pub fn accept_invitation(client: &RestClient, session: &mut Session, token: &str) -> Result<(), RestError> {
    let request = SharingRequest::new(
        SharingMethod::Post,
        "/api/organizations/invitations/accept".into(),
        Some(json!({ "token": token })),
    );
    send(client, session, &request).map(|_| ())
}

/// `POST /api/organizations/invitations/decline {token}`.
pub fn decline_invitation(client: &RestClient, session: &mut Session, token: &str) -> Result<(), RestError> {
    let request = SharingRequest::new(
        SharingMethod::Post,
        "/api/organizations/invitations/decline".into(),
        Some(json!({ "token": token })),
    );
    send(client, session, &request).map(|_| ())
}
```

with:

```rust
/// `POST /api/organizations/{orgId}/users/{ouId}/accept {token}` -- Bitwarden's
/// accept, with the token an invitation link carries (official-path spec
/// §4.1). The server checks the token, and that the invitation's email is
/// the signed-in account's. `resetPasswordKey` is not sent: a sharing org has
/// no account-recovery policy that would ask for one. There is no decline --
/// Bitwarden has none; an invitation nobody answers expires.
pub fn accept_invite(client: &RestClient, session: &mut Session, org: &str, ou_id: &str, token: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/accept", safe(org)?, safe(ou_id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json!({ "token": token })))).map(|_| ())
}
```

Also fix the doc of `user_public_key`, which named the probe. Replace:

```rust
/// `GET /api/users/{userId}/public-key` -- slice 1's second probe, and the
/// key every confirm wraps to. `None`: no such route, no such user, or a user
/// who has never opened an app that made keys.
```

with:

```rust
/// `GET /api/users/{userId}/public-key` -- the key every confirm wraps to.
/// `None`: no such user, or a user who has never opened an app that made
/// keys.
```

Replace the test `my_invitations_are_read_and_a_missing_route_is_none`. Write `$TEMP/dw/dw3-api-test.txt`:

```rust
    /// **The accept is Bitwarden's**: the link's org and org-user ids in the
    /// path, the token in the body and nowhere else; an id that is not
    /// path-safe never reaches the wire.
    #[test]
    fn an_accept_posts_the_links_token_to_the_official_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let accepted = server
            .mock("POST", format!("/api/organizations/{ORG}/users/ou-9/accept").as_str())
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" })))
            .with_body("")
            .expect(1)
            .create();
        accept_invite(&client, &mut session, ORG, "ou-9", "tok-1").expect("accepted");
        accepted.assert();
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(accept_invite(&client, &mut session, ORG, "../x", "t"), Err(RestError::UnsafeId));
        any.assert();
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing_api.rs '    /// **The invitation list, and what its absence means.** A 200 list is' '    /// **The organisation create body is the official one** (spec 9.5),' "$TEMP/dw/dw3-api-test.txt"
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing_api.rs`
Expected: `0`.

- [ ] **Step 6: The engine accepts a pasted link, and holds no invitations**

In `src/rest/sharing.rs`, replace:

```rust
//! The recipient's invitation list is the one NodeWarden route left, until
//! the paste-a-link accept replaces it.
```

with:

```rust
//! There is no invitation list -- no official server lets an invitee list
//! one -- and an invitation is accepted from its pasted link
//! ([`Sharing::accept_link`]) or in the web vault.
```

Replace:

```rust
use crate::rest::sharing_api::{self, IncomingInvitation};
```

with:

```rust
use crate::rest::sharing_api;
```

Delete `IncomingEntry`. Replace:

```rust
/// An invitation addressed to this account, from a sharer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEntry {
    pub id: String,
    /// **Who sent it, as the SERVER attests** -- the inviting org's
    /// `ownerEmail`, and never its plaintext "Shared by" name, which anyone
    /// can set (final review, concern 1). `None` when the server does not
    /// say: the window then shows a neutral "Someone on this server", not a
    /// name-only claim.
    #[serde(default)]
    pub from: Option<String>,
    pub expires_at: Option<String>,
}

```

with nothing. In `SharingView`, replace:

```rust
    pub received: Vec<ReceivedEntry>,
    pub incoming: Vec<IncomingEntry>,
    pub waiting: Vec<WaitingEntry>,
```

with:

```rust
    pub received: Vec<ReceivedEntry>,
    pub waiting: Vec<WaitingEntry>,
```

In `Held`, replace its doc, from:

```rust
/// What the engine holds between passes. See the module docs.
///
/// **A hand-written `Debug`, and it must have one** -- not for a caller, but
```

through the closing `}` of `impl std::fmt::Debug for Held` (the line before `/// **The engine.** See the module docs.`). Write `$TEMP/dw/dw3-held.txt`:

```rust
/// What the engine holds between passes. See the module docs.
///
/// **A hand-written `Debug`.** It was `crate::debug_leak_guard`'s barrier
/// while `Held` kept invitation tokens; it holds no secret now (there is no
/// invitation list), and the impl stays so that a field added later is
/// printed on purpose or not at all.
#[derive(Clone, Default)]
pub(crate) struct Held {
    pub(crate) capabilities: Capabilities,
    pub(crate) account: Option<Account>,
    pub(crate) my_org: Option<MyOrg>,
    /// Other people's sharing orgs: `(org id, sharer)`.
    pub(crate) others: Vec<(String, String)>,
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
    /// Emails whose key differs from the pin.
    pub(crate) key_changed: BTreeSet<String>,
    /// Emails whose account has no public key yet.
    pub(crate) no_key: BTreeSet<String>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("capabilities", &self.capabilities)
            .field("account", &self.account)
            .field("my_org", &self.my_org)
            .field("others", &self.others)
            .field("waiting", &self.waiting)
            .field("key_changed", &self.key_changed)
            .field("no_key", &self.no_key)
            .finish()
    }
}

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '/// What the engine holds between passes. See the module docs.' '/// **The engine.** See the module docs.' "$TEMP/dw/dw3-held.txt"
```

In `view`, replace:

```rust
        view.incoming = held
            .incoming
            .iter()
            // The NAME says only that this is a personal-sharing invitation
            // (a "Shared by" org); WHO sent it is the server's attested
            // owner, whatever the name claims -- a forged "Shared by boss@"
            // is shown as its real owner's -- or nobody when it does not say.
            .filter(|invitation| is_personal_share(invitation))
            .map(|invitation| IncomingEntry {
                id: invitation.id.clone(),
                from: invitation.owner_email.clone(),
                expires_at: invitation.expires_at.clone(),
            })
            .collect();
        view.waiting = held
```

with:

```rust
        view.waiting = held
```

and replace `view`'s doc paragraph:

```rust
    /// Built under the `held` lock rather than from a clone of [`Held`]: a
    /// clone would copy every incoming invitation's `Zeroizing` token only to
    /// read its id and drop it, and nothing here takes another lock or makes
    /// a request, so the guard is short.
```

with:

```rust
    /// Built under the `held` lock rather than from a clone of [`Held`]:
    /// nothing here takes another lock or makes a request, so the guard is
    /// short, and a clone would copy the roster to read it once.
```

Replace the whole of `refresh` (DW1's), from its doc's first line:

```rust
    /// **One pass** (spec 11.5): the account, then the incoming invitations on a
```

through its closing `}` (the line before `    /// Whether the ten-minute timer has anything to do: someone in this`). Write `$TEMP/dw/dw3-refresh.txt`:

```rust
    /// **One pass** (spec 11.5): the account, then every move back whose
    /// delete failed ([`Self::finish_unfinished`]) -- on every trigger, since
    /// each is one file read when nothing is pending -- and last the confirm
    /// pass ([`Self::confirm_pass`]), whose failure is logged: the finished
    /// moves above must still reach the daemon.
    ///
    /// There is no invitation list to read (official-path spec §4.1): no
    /// official server lets an invitee list their invitations, and
    /// NodeWarden's `/invitations/mine` is not called. An invitation is
    /// accepted from its link ([`Self::accept_link`]) or in the web vault.
    ///
    /// # Errors
    ///
    /// The account could not be read; a later pass tries again.
    pub fn refresh(&self, trigger: Trigger) -> Result<SharingPass, SharingError> {
        let _one = self.one_at_a_time();
        let was_loaded = self.held().capabilities.loaded;
        self.ensure_loaded()?;
        let mut pass = SharingPass { changed: !was_loaded, ..SharingPass::default() };
        if self.held().capabilities.too_old {
            return Ok(pass);
        }
        pass.outcome = self.finish_unfinished();
        pass.changed |= !pass.outcome.is_empty();
        match self.confirm_pass(trigger) {
            Ok(confirmed) => pass.changed |= confirmed,
            // A confirm that could not be made this time is tried again on the
            // next trigger; the pass itself did what it could, and its
            // finished moves above must still reach the daemon.
            Err(e) => log::info!("the confirm pass stopped early: {e}"),
        }
        Ok(pass)
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **One pass** (spec 11.5): the account, then the incoming invitations on a' '    /// Whether the ten-minute timer has anything to do: someone in this' "$TEMP/dw/dw3-refresh.txt"
```

Delete `is_personal_share` (its doc and the function) with `dw_cut`, keeping the section comment above it. Write `$TEMP/dw/dw3-owner.txt` holding the one comment line and an empty line after it:

```rust
// ---- the owner's writes -----------------------------------------------------------

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '// ---- the owner'"'"'s writes ----' '/// **Owned by this account and named for it** -- what makes an org a' "$TEMP/dw/dw3-owner.txt"
```

Replace the recipient section's start, from `/// An invitation id this account no longer holds: answered already (here or` up to `    /// **Stop receiving from a sharer** (spec 6.9): leave their sharing org,`. Write `$TEMP/dw/dw3-recipient.txt`:

```rust
/// A leave asked of an organisation that is not someone else's sharing org --
/// this account's own (whose owner NodeWarden will not let leave anyway), a
/// real organisation, or one this account has already left.
pub const NOT_A_SHARER: &str = "That is not someone sharing with you.";
/// A pasted invite link the server would not accept; its own words follow
/// (`SharingError::or_say`).
pub const LINK_ACCEPT_FAILED: &str = "Couldn't accept the invitation.";

/// A pasted invite link addressed to another account (official-path spec
/// §4.1). The server would refuse it too; this says so in words, before a
/// request is made.
#[must_use]
pub fn link_for_someone_else(link_email: &str, account_email: &str) -> String {
    format!("This invite link is for {link_email}. You're signed in as {account_email}.")
}

impl Sharing {
    /// **Accept a pasted invite link** (official-path spec §4.1): read it
    /// ([`crate::invite_link::parse`]), refuse it in words when its email is
    /// not this account's, and otherwise send its token to Bitwarden's own
    /// accept route. The sharer's daemon confirms, and the records arrive on
    /// the push or the poll that confirm produces.
    ///
    /// **The org is filed as waiting at once**, under the name's claim, so
    /// Shared with me says "Waiting for X to finish sharing" the moment the
    /// accept lands rather than after the next whole read -- which replaces
    /// this entry with the directory's own word when it comes. A name that
    /// claims no sharer files nothing.
    ///
    /// The link is the caller's; nothing here logs it or keeps it, and the
    /// token leaves this process only in the accept's body.
    ///
    /// # Errors
    ///
    /// The link is not one ([`crate::invite_link::LinkError::sentence`]), it
    /// is for someone else ([`link_for_someone_else`]), or the server refused
    /// -- its sentence after [`LINK_ACCEPT_FAILED`].
    pub fn accept_link(&self, link: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let invite = crate::invite_link::parse(link).map_err(|e| SharingError::Refused(e.sentence().to_string()))?;
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(LINK_ACCEPT_FAILED.to_string()))?;
        let mine = normalise_email(&account.email);
        if invite.email != mine {
            return Err(SharingError::Refused(link_for_someone_else(&invite.email, &mine)));
        }
        self.call(|client, auth| {
            sharing_api::accept_invite(client, &mut auth.session, &invite.organization_id, &invite.organization_user_id, &invite.token)
        })
        .map_err(|e| e.or_say(LINK_ACCEPT_FAILED))?;
        if let Some(from) = invite.organization_name.as_deref().and_then(sharer_of) {
            let mut held = self.held();
            if !held.waiting.iter().any(|(org, _)| *org == invite.organization_id) {
                held.waiting.push((invite.organization_id.clone(), from.to_string()));
            }
        }
        Ok(())
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '/// An invitation id this account no longer holds: answered already (here or' '    /// **Stop receiving from a sharer** (spec 6.9): leave their sharing org,' "$TEMP/dw/dw3-recipient.txt"
```

The block removed `INVITATION_GONE`, `token_of`, `accept` and `decline`, and re-opened `impl Sharing {` for `leave`.

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs`
Expected: `0`.

- [ ] **Step 7: The engine's recipient tests**

Delete `held_debug_never_prints_an_invitation_token`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **`Held`'"'"'s `Debug` never prints an invitation token** -- the reason it' '    /// The fixture profile with no key pair: `privateKey` null.'
```

Delete `a_failed_invitations_read_does_not_skip_a_half_finished_move`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **A failed invitations read does not skip a half-finished move**: the' '    // ---- follow-up: a pending move back, from every side ----'
```

Delete `an_elevated_role_invitation_is_not_listed_or_accepted_as_a_share`:

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    /// **An owner, admin or manager invitation is not a personal share**' '    /// **A cached id the profile does not list YET is kept** (final'
```

Replace `invited` through `a_recipient_declines_an_invitation_by_its_token`. Write `$TEMP/dw/dw3-t-accept.txt`:

```rust
    /// A Bitwarden invite link for `email`, to the fixture-free org `org-guid`.
    fn invite_link_for(email: &str) -> String {
        format!(
            "https://vault.example/#/accept-organization?organizationId=0f0e0d0c-0b0a-4009-8807-060504030201\
             &organizationUserId=a1b2c3d4-e5f6-4789-8abc-def012345678&email={}\
             &organizationName=Shared+by+denis%40example.com&token=tok-1&initOrganization=False&orgUserHasExistingUser=True",
            email.replace('@', "%40")
        )
    }

    /// **A pasted link is accepted on Bitwarden's own route** (official-path
    /// spec §4.1), and the sharer is waited on at once -- under the name's
    /// claim, until a whole read says more.
    #[test]
    fn a_pasted_link_is_accepted_on_the_official_route() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let accepted = server
            .mock("POST", "/api/organizations/0f0e0d0c-0b0a-4009-8807-060504030201/users/a1b2c3d4-e5f6-4789-8abc-def012345678/accept")
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" })))
            .with_body("")
            .expect(1)
            .create();
        let listed = server.mock("GET", Matcher::Regex("invitations".into())).expect(0).create();
        engine.accept_link(&invite_link_for(FIXTURE_EMAIL)).expect("accepted");
        accepted.assert();
        listed.assert();
        assert_eq!(
            engine.view(&[]).waiting,
            vec![WaitingEntry { organization_id: "0f0e0d0c-0b0a-4009-8807-060504030201".into(), from: "denis@example.com".into() }]
        );
    }

    /// **A link for another account is refused in words, before any
    /// request** -- the server would refuse it too, in its own.
    #[test]
    fn a_link_for_someone_else_is_refused_before_any_request() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let posts = server.mock("POST", Matcher::Any).expect(0).create();
        match engine.accept_link(&invite_link_for("someone@else.example")) {
            Err(SharingError::Refused(sentence)) => {
                assert_eq!(sentence, link_for_someone_else("someone@else.example", &FIXTURE_EMAIL.to_lowercase()));
            }
            other => panic!("a link for someone else was accepted: {other:?}"),
        }
        posts.assert();
        assert!(engine.view(&[]).waiting.is_empty());
    }

    /// **What is not a link is refused with the parser's sentence**, and a
    /// server's refusal keeps its own words after the engine's.
    #[test]
    fn a_bad_link_and_a_refused_accept_say_why() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        match engine.accept_link("https://vault.example/#/vault") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, crate::invite_link::LinkError::NotAnInviteLink.sentence()),
            other => panic!("not a link was accepted: {other:?}"),
        }
        server
            .mock("POST", Matcher::Regex("/accept$".into()))
            .with_status(400)
            .with_body(r#"{"message":"Invalid token."}"#)
            .create();
        match engine.accept_link(&invite_link_for(FIXTURE_EMAIL)) {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, format!("{LINK_ACCEPT_FAILED} The server said: Invalid token.")),
            other => panic!("a refused accept succeeded: {other:?}"),
        }
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/sharing.rs '    fn invited(server: &mut MockServer) {' '    /// **Stop receiving from X** (spec 6.9): leave the sharer'"'"'s org, and every' "$TEMP/dw/dw3-t-accept.txt"
```

Clear the old invitation-list mocks that remain in tests. Write `$TEMP/dw_mocks.py`:

```python
# Deletes single-line test mocks of NodeWarden's invitation list, which
# nothing calls any more. Only whole lines that are one such statement go,
# and none bound to a name (a `let` would leave its uses dangling).
import sys
for path in sys.argv[1:]:
    lines = open(path, "rb").read().split(b"\r\n")
    keep = [
        l for l in lines
        if not (
            b'.mock("GET", "/api/organizations/invitations/mine")' in l
            and l.rstrip().endswith(b".create();")
            and not l.strip().startswith(b"let ")
        )
    ]
    open(path, "wb").write(b"\r\n".join(keep))
    print(path, "removed", len(lines) - len(keep))
```

Run: `python "$TEMP/dw_mocks.py" src/rest/sharing.rs src/vault_api.rs src/vault_host.rs`
Expected: three lines with these counts. If a count differs, read the `grep` below before going on.
- `src/rest/sharing.rs`: at least 6.
- `src/vault_api.rs`: 2.
- `src/vault_host.rs`: 4.

Run: `grep -rn "invitations/mine\|IncomingEntry\|IncomingInvitation\|INVITATION_GONE\|is_personal_share\|token_of\|\.incoming\b" src/rest src/vault_api.rs src/vault_host.rs`
Expected: no output. (`vault_api.rs`'s recipient test still mentions `INVITATION_GONE`; Step 8 rewrites it, so do Step 8 before you expect this to be clean.)

- [ ] **Step 8: The daemon's paste route, and the window's client**

In `src/vault_api.rs`, replace:

```rust
            ("POST", ["sharing", "incoming", id, "accept"]) => self.incoming(id, true),
            ("POST", ["sharing", "incoming", id, "decline"]) => self.incoming(id, false),
```

with:

```rust
            ("POST", ["sharing", "incoming", "paste"]) => self.accept_link(request.body),
```

Replace the whole of `incoming`:

```rust
    /// `POST /sharing/incoming/{id}/accept|decline` (spec 11.3) -- the
    /// recipient's answer to an invitation, by the id `GET /sharing` listed it
    /// under. **The token that answers it is the engine's** (`Sharing::
    /// accept`): the window never held it, so no body carries one. Nothing in
    /// the vault moves yet -- an accept's records arrive once the sharer's
    /// daemon confirms -- so only `sharing` is announced.
    fn incoming(&self, id: &str, accept: bool) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let done = if accept { engine.accept(id) } else { engine.decline(id) };
        match done {
            Ok(()) => {
                self.feed.publish([Change::Sharing]);
                ApiResponse::done()
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

with:

```rust
    /// `POST /sharing/incoming/paste {link}` (spec 11.3; official-path spec
    /// §4.1) -- Paste an invite link. The link's token accepts the
    /// invitation, so it is read straight into [`PasteBody`]'s `Zeroizing`
    /// -- not through a `serde_json::Value`, whose own `String` copy would be
    /// dropped unwiped -- and wiped when this returns; the host's buffer it
    /// came from is a `Zeroizing` too (`vault_host::serve`). Nothing here
    /// logs it. Nothing in the vault moves yet -- the records arrive once the
    /// sharer's daemon confirms -- so only `sharing` is announced.
    fn accept_link(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let asked: Option<PasteBody> = serde_json::from_slice(body).ok();
        let Some(asked) = asked.filter(|asked| !asked.link.trim().is_empty()) else {
            return ApiResponse::refused(400, "Paste the invite link first.");
        };
        match engine.accept_link(&asked.link) {
            Ok(()) => {
                self.feed.publish([Change::Sharing]);
                ApiResponse::done()
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

Add the body type. Replace:

```rust
/// A body's `keySha256`: exactly 64 hex digits, the form
```

with:

```rust
/// `POST /sharing/incoming/paste`'s body: the pasted link, wiped on drop. A
/// missing `link` reads as empty, which the route refuses in words.
#[derive(serde::Deserialize)]
struct PasteBody {
    #[serde(default)]
    link: zeroize::Zeroizing<String>,
}

/// A body's `keySha256`: exactly 64 hex digits, the form
```

In `stopping_receiving_takes_the_sharers_records_out_and_announces_them`, the old invitation mock line is gone after Step 7. Replace:

```rust
        server
            .mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect_at_least(0)
            .create();
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);

        let answers = server.mock("POST", "/api/organizations/invitations/accept").expect(0).create();
        for route in ["/sharing/incoming/inv-9/accept", "/sharing/incoming/inv-9/decline"] {
            let answer = ask(&api, "POST", route, Some(&token), "{}");
            assert_eq!(answer.status, 400, "{route}");
            let said: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
            assert_eq!(said["message"], crate::rest::sharing::INVITATION_GONE, "{route}");
        }
        answers.assert();
```

with:

```rust
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);

        // The listed-invitation routes are gone: an invitation is accepted
        // from its link, and there is no decline (official-path spec §4.1).
        for route in ["/sharing/incoming/inv-9/accept", "/sharing/incoming/inv-9/decline"] {
            let answer = ask(&api, "POST", route, Some(&token), "{}");
            assert_eq!(answer.status, 405, "{route}: {}", answer.body);
        }
        // A pasted link for another account is refused in words, before any
        // request reaches the server.
        let accepts = server.mock("POST", crate::test_http::Matcher::Regex("/accept$".into())).expect(0).create();
        let link = "https://vault.example/#/accept-organization?organizationId=11111111-1111-4111-8111-111111111111\
                    &organizationUserId=22222222-2222-4222-8222-222222222222&email=someone%40else.example&token=t-1";
        let answer = ask(&api, "POST", "/sharing/incoming/paste", Some(&token), &serde_json::json!({ "link": link }).to_string());
        assert_eq!(answer.status, 400, "{}", answer.body);
        accepts.assert();
        let empty = ask(&api, "POST", "/sharing/incoming/paste", Some(&token), "{}");
        assert_eq!(empty.status, 400, "{}", empty.body);
```

Also change that test's doc first lines:

```rust
    /// **The recipient's three routes** (spec 11.3): an answer to an
    /// invitation this daemon does not hold is the engine's sentence, never a
    /// request; and Stop receiving leaves the sharer's org, takes every
```

to:

```rust
    /// **The recipient's routes** (spec 11.3): the listed-invitation answers
    /// are gone, a pasted link for someone else is refused before any
    /// request; and Stop receiving leaves the sharer's org, takes every
```

In `src/daemon_link.rs`, replace `accept` and `decline`, from:

```rust
    /// `POST /sharing/incoming/{id}/accept`. The id is the one `GET /sharing`
```

through the closing `}` of `decline` (the line before `    /// \`POST /sharing/from/{orgId}/leave\` -- stop receiving from a sharer.`). Write `$TEMP/dw/dw3-client.txt`:

```rust
    /// `POST /sharing/incoming/paste {link}` -- Paste an invite link
    /// (official-path spec §4.1).
    ///
    /// **Not through [`Self::post`]**, for [`Self::invite_code`]'s reason: the
    /// link's token accepts the invitation, and a `serde_json::Value` would
    /// hold its own copy and drop it unwiped. The body is written from the
    /// borrowed link straight into a `Zeroizing` buffer sized up front and
    /// sent as bytes. The caller's own `link` is the caller's to wipe.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn accept_link(&self, link: &str) -> Result<(), String> {
        #[derive(serde::Serialize)]
        struct Body<'a> {
            link: &'a str,
        }
        // JSON escapes a byte into at most six (`\u00XX`); the rest is the
        // braces, the key and the quotes.
        let mut body = Zeroizing::new(Vec::with_capacity(6 * link.len() + 16));
        serde_json::to_writer(&mut *body, &Body { link }).map_err(|_| "The invite link could not be sent.".to_string())?;
        sharing_answer(
            self.agent
                .post(&format!("{}/sharing/incoming/paste", self.base_url))
                .set("Content-Type", "application/json")
                .send_bytes(&body),
        )
        .map(|_| ())
    }

```

```bash
python "$TEMP/dw_cut.py" src/daemon_link.rs '    /// `POST /sharing/incoming/{id}/accept`. The id is the one `GET /sharing`' '    /// `POST /sharing/from/{orgId}/leave` -- stop receiving from a sharer.' "$TEMP/dw/dw3-client.txt"
```

Delete the constant. Replace:

```rust
/// An invitation whose id could not be put in a route's path -- never one the
/// server issued, so nothing to answer.
const INVITATION_UNUSABLE: &str = "That invitation can't be answered.";

```

with nothing.

Replace the test `the_recipients_answers_refuse_an_id_that_is_not_path_safe_before_any_request`. Write `$TEMP/dw/dw3-t-client.txt`:

```rust
    /// **An organisation id goes in Stop receiving's path**, so `leave`
    /// refuses an unsafe one before any request; **a pasted link goes in the
    /// paste route's body**, whole, and nowhere else.
    #[test]
    fn stop_receiving_refuses_an_unsafe_id_and_a_link_rides_in_the_body() {
        let mut server = crate::test_http::server();
        let any = server.mock("POST", crate::test_http::Matcher::Any).expect(0).create();
        let client = SharingClient::new(&server.url(), &Zeroizing::new(GRANT.to_string()));
        for unsafe_id in ["../x", "a/b", "", "a?b=c", "a%2Fb"] {
            assert!(client.leave(unsafe_id).is_err(), "{unsafe_id:?}");
        }
        any.assert();
        let ok = r#"{"success":true,"data":{}}"#;
        let link = "https://vault.example/#/accept-organization?organizationId=x&token=t+1";
        let pasted = server
            .mock("POST", "/sharing/incoming/paste")
            .match_body(crate::test_http::Matcher::Json(serde_json::json!({ "link": link })))
            .with_body(ok)
            .expect(1)
            .create();
        let left = server.mock("POST", "/sharing/from/o-1/leave").with_body(ok).expect(1).create();
        assert_eq!(client.accept_link(link), Ok(()));
        assert_eq!(client.leave("o-1"), Ok(()));
        pasted.assert();
        left.assert();
    }

```

```bash
python "$TEMP/dw_cut.py" src/daemon_link.rs '    /// **An invitation or organisation id goes in the path too**, so' '    /// **`forget` and `reinvite` name the person in the body**, never the' "$TEMP/dw/dw3-t-client.txt"
```

Run: `python "$TEMP/dw_crlf.py" src/vault_api.rs src/daemon_link.rs src/vault_host.rs`
Expected: three `0`s.

- [ ] **Step 9: The window: Paste an invite link replaces the invitation rows**

**9a. `share_ui.rs`: the block.** Write `$TEMP/dw/dw3-paste.txt`:

```rust
// ---- Shared with me: Paste an invite link, and the sharers being waited on ------------
//
// **There are no invitation rows any more** (official-path spec §4.1). No
// official server lets an invitee list their invitations, and NodeWarden's
// list is not read: an invitation arrives as a link -- in an email, or a
// message from the sharer -- and is accepted by pasting it here, or in the web
// vault, which accepts the same link. There is no Decline: Bitwarden has
// none, and an invitation nobody answers expires.

/// The paste box's hint, and its button.
pub(crate) const PASTE_HINT: &str = "Paste an invite link";
pub(crate) const PASTE_ACCEPT_LABEL: &str = "Accept";
/// A pasted link was accepted. The records arrive once the sharer's daemon
/// confirms; the waiting line under the box says so meanwhile.
pub(crate) const LINK_ACCEPTED: &str = "Invitation accepted. The records arrive once the person sharing them finishes.";
/// The paste row: one `theme::inline_field` high.
pub(crate) const PASTE_ROW_HEIGHT: f32 = theme::BUTTON_HEIGHT;
const PASTE_BUTTON_GAP: f32 = 8.0;
/// One [`waiting_line`]: 12pt words in a band as tall as a group eyebrow's
/// row pair, inset as the eyebrows are.
pub(crate) const WAITING_LINE_HEIGHT: f32 = 24.0;
const WAITING_LINE_SIZE: f32 = 12.0;

/// "Waiting for denis@… to finish sharing" (spec 6.9): accepted, not yet
/// confirmed. The sharer's daemon confirms on its next pass; until then the
/// org shows this account nothing, and this line is why.
pub(crate) fn waiting_line(from: &str) -> String {
    format!("Waiting for {from} to finish sharing")
}

/// One line of `text`, ended with an ellipsis where it would pass `width` --
/// an address too long for the row is cut, never wrapped under the buttons.
fn one_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    painter.layout_job(job)
}

/// **Shared with me's top**: Paste an invite link, where the window offers
/// it ([`PeopleRows::paste`]) -- the box, and Accept beside it -- then a
/// quiet [`waiting_line`] for every sharer this account accepted and is
/// waiting on. Draws nothing when there is neither.
///
/// Accept is greyed, and deaf, while the box is empty or a sharing write is
/// out ([`PeopleRows::busy`]): [`request`] would only refuse a second write
/// with "still running". A press is reported as [`PeopleAsk::AcceptLink`];
/// the window takes the box's text and sends it.
pub(crate) fn draw_paste_and_waiting(ui: &mut egui::Ui, people: &mut PeopleRows<'_>, width: f32) {
    if let Some(buffer) = people.paste.as_mut() {
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, PASTE_ROW_HEIGHT), egui::Sense::hover());
        let button_width = theme::small_button_width(ui.painter(), PASTE_ACCEPT_LABEL);
        let field = egui::Rect::from_min_size(row.min, egui::vec2((width - button_width - PASTE_BUTTON_GAP).max(0.0), PASTE_ROW_HEIGHT));
        let mut field_ui = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let _ = theme::inline_field(&mut field_ui, buffer, PASTE_HINT, field.width(), false);
        let button = egui::Rect::from_min_size(
            egui::pos2(field.right() + PASTE_BUTTON_GAP, row.center().y - theme::SMALL_BUTTON_HEIGHT / 2.0),
            egui::vec2(button_width, theme::SMALL_BUTTON_HEIGHT),
        );
        let enabled = !people.busy && !buffer.trim().is_empty();
        if theme::small_action_button(ui, button, PASTE_ACCEPT_LABEL, theme::ActionTone::Primary, enabled).clicked() {
            people.asked = Some(PeopleAsk::AcceptLink);
        }
    }
    for from in people.waiting {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, WAITING_LINE_HEIGHT), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let galley = one_line(
            &painter,
            &waiting_line(from),
            egui::FontId::new(WAITING_LINE_SIZE, egui::FontFamily::Proportional),
            theme::TEXT_FAINT,
            width - GROUP_EYEBROW_INSET * 2.0,
        );
        let y = rect.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(rect.left() + GROUP_EYEBROW_INSET, y), galley, theme::TEXT_FAINT);
    }
}

/// The height [`draw_paste_and_waiting`] takes, with the list's `gap` BETWEEN
/// its rows -- `n` rows, `n - 1` gaps, and nothing for none. The gap joining
/// them to rows under them is the caller's: only it knows whether there are
/// any (`send_ui`'s `rows_area`, which counts gaps the same way).
pub(crate) fn paste_and_waiting_height(people: &PeopleRows<'_>, row_gap: f32) -> f32 {
    let paste = usize::from(people.paste.is_some());
    let rows = paste + people.waiting.len();
    paste as f32 * PASTE_ROW_HEIGHT + people.waiting.len() as f32 * WAITING_LINE_HEIGHT + rows.saturating_sub(1) as f32 * row_gap
}

```

```bash
python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '// ---- the recipient'"'"'s invitations (mock 6d, invite pending) ----' '/// The leave question'"'"'s two answers.' "$TEMP/dw/dw3-paste.txt"
```

**9b. `PeopleRows` and `PeopleAsk`.** Replace:

```rust
    /// **Shared with me's invitations** (mock 6d, invite pending): the rows
    /// [`draw_incoming_rows`] puts first in the list, each with Accept and
    /// Decline, and the pane [`draw_invitation_detail`] draws for one.
    pub(crate) incoming: &'a [IncomingRow],
    /// **The sharers this account accepted and who have not finished yet**
    /// -- the view's `waiting`, by the sharer's address. Each is one quiet
    /// [`waiting_line`] under the invitations: an accepted sharing org shows
    /// its member nothing until the sharer's daemon confirms them, and
    /// without the line an Accept would look like it did nothing.
    pub(crate) waiting: &'a [String],
    /// **A sharing write is out** ([`SharingPane::is_busy`]): Accept and
    /// Decline are drawn greyed and sense nothing, rather than being pressed
    /// only for [`request`] to refuse them with [`BUSY`].
    pub(crate) busy: bool,
}
```

with:

```rust
    /// **Paste an invite link's box** (official-path spec §4.1), lent by the
    /// window for the frame ([`SharingPane::paste`]) and handed back after
    /// it; `None` where sharing is not available, and the row is not drawn.
    /// [`draw_paste_and_waiting`] puts it first in Shared with me.
    pub(crate) paste: Option<Zeroizing<String>>,
    /// **The sharers this account accepted and who have not finished yet**
    /// -- the view's `waiting`, by the sharer's address. Each is one quiet
    /// [`waiting_line`] under the paste box: an accepted sharing org shows
    /// its member nothing until the sharer's daemon confirms them, and
    /// without the line an accept would look like it did nothing.
    pub(crate) waiting: &'a [String],
    /// **A sharing write is out** ([`SharingPane::is_busy`]): Paste's Accept
    /// is drawn greyed and senses nothing, rather than being pressed only for
    /// [`request`] to refuse it with [`BUSY`].
    pub(crate) busy: bool,
}
```

Replace in `PeopleRows::none`:

```rust
            received: &[],
            incoming: &[],
            waiting: &[],
```

with:

```rust
            received: &[],
            paste: None,
            waiting: &[],
```

Replace:

```rust
/// A press in the Sends pane's sharing half. `Stop`'s keys are
/// [`share_key`]s; `Accept`'s and `Decline`'s are INVITATION IDS -- the
/// view's, which the daemon answers by the token it holds.
```

with:

```rust
/// A press in the Sends pane's sharing half. `Stop`'s keys are
/// [`share_key`]s.
```

and replace:

```rust
    /// An invitation's Accept (mock 6d), in its row or its pane.
    Accept(String),
    /// Its Decline. One press, like Accept: a decline adds nothing and takes
    /// nothing away, and the sharer can invite again.
    Decline(String),
}
```

with:

```rust
    /// Paste an invite link's Accept: the window sends the box's text.
    AcceptLink,
}
```

**9c. The calls and reports.** Replace:

```rust
/// `View`, and the Share dialog's three (Share, Forget, InviteCode), are the
/// window's and the dialog's; the rest are sent by the cards: Stop, Reinvite,
/// Fingerprint and Confirm by the SHARING card and Sent's share detail;
/// Accept and Decline by Shared with me's invitations; Leave by a received
/// record's Stop receiving.
```

with:

```rust
/// `View`, and the Share dialog's three (Share, Forget, InviteCode), are the
/// window's and the dialog's; the rest are sent by the cards: Stop, Reinvite,
/// Fingerprint and Confirm by the SHARING card and Sent's share detail;
/// AcceptLink by Shared with me's Paste an invite link; Leave by a received
/// record's Stop receiving. AcceptLink's link, like InviteCode's password,
/// stays `Zeroizing` until the call is dropped at the end of [`run`].
```

Replace:

```rust
    InviteCode { email: String, password: Zeroizing<String> },
    Accept(String),
    Decline(String),
```

with:

```rust
    InviteCode { email: String, password: Zeroizing<String> },
    /// Paste an invite link's Accept: the link as pasted, token and all.
    AcceptLink(Zeroizing<String>),
```

Replace:

```rust
            Self::Accept(_) | Self::Decline(_) => SharingReport::Done(Err(err)),
```

with:

```rust
            Self::AcceptLink(_) => SharingReport::LinkAccepted(Err(err)),
```

Replace:

```rust
    Left { from: String, result: Result<(), String> },
    Done(Result<(), String>),
```

with:

```rust
    Left { from: String, result: Result<(), String> },
    /// Paste an invite link's answer.
    LinkAccepted(Result<(), String>),
```

Replace:

```rust
            Self::Done(r) => ("Done", outcome(r)),
```

with:

```rust
            Self::LinkAccepted(r) => ("LinkAccepted", outcome(r)),
```

Replace:

```rust
        SharingCall::Accept(id) => SharingReport::Done(client.accept(&id)),
        SharingCall::Decline(id) => SharingReport::Done(client.decline(&id)),
```

with:

```rust
        // `link` is dropped -- and so wiped -- when this arm ends.
        SharingCall::AcceptLink(link) => SharingReport::LinkAccepted(client.accept_link(&link)),
```

Replace:

```rust
        SharingReport::Done(result) => {
            if let Err(sentence) = result {
                fail(pane, sentence);
            }
        }
```

with:

```rust
        // **A pasted link was accepted**: the records arrive once the sharer
        // finishes, and Shared with me's waiting line says so; the toast says
        // it now. A refusal is the engine's sentence -- not a link, someone
        // else's, the server's no -- said where the user is.
        SharingReport::LinkAccepted(result) => match result {
            Ok(()) => toast(pane, LINK_ACCEPTED.to_string()),
            Err(sentence) => fail(pane, sentence),
        },
```

Replace:

```rust
    // put up is a different one and stays.
    // An Accept's or Decline's refusal pinned to its invitation
    // (`SharingPane::pin_notice_to`) is spent the same way, tag and all.
    if pane.notice.as_deref() == Some(BUSY) {
```

with:

```rust
    // put up is a different one and stays.
    if pane.notice.as_deref() == Some(BUSY) {
```

**9d. `SharingPane`.** Replace:

```rust
    // **`has_incoming` was here**, and the rail's dot read it. The dot reads
    // the frame's own `incoming_rows` now -- the rows the list draws -- so
    // the badge = list rule holds for it as for the counts.

    /// A write holds the one slot ([`request`]): what greys the invitations'
    /// Accept and Decline ([`PeopleRows::busy`]).
    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }

    /// **A refused write's sentence, pinned to the thing it was asked on** --
    /// an invitation's key, for an Accept or a Decline [`request`] would not
    /// start. `request` says a refusal about no record in particular, and on
    /// Shared with me that stands only on the invitation pane on screen; one
    /// asked from a row while something else is picked would be said
    /// nowhere. Pinned, it stands on that invitation's pane when it is shown
    /// and goes to a toast otherwise ([`route_stray_notice`]).
    pub(crate) fn pin_notice_to(&mut self, record: &str) {
        if self.notice.is_some() {
            self.notice_record = Some(record.to_string());
        }
    }
```

with:

```rust
    // **`has_incoming` was here**, and the rail's dot read it. The dot reads
    // the frame's own waiting lines now -- what the list draws -- so the
    // badge = list rule holds for it as for the counts.

    /// A write holds the one slot ([`request`]): what greys Paste an invite
    /// link's Accept ([`PeopleRows::busy`]).
    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }
```

Add the field. Replace:

```rust
    /// The Share dialog, while it is open.
    pub(crate) dialog: Option<ShareDialog>,
```

with:

```rust
    /// The Share dialog, while it is open.
    pub(crate) dialog: Option<ShareDialog>,
    /// **Paste an invite link's text**, between frames: lent to the list as
    /// [`PeopleRows::paste`] while it is drawn, and taken whole when Accept is
    /// pressed. Wiped on drop -- its token accepts the invitation.
    pub(crate) paste: Zeroizing<String>,
```

Delete `days_left`, which only the invitation rows used. Replace:

```rust
/// Whole days until an ISO instant, rounded up; `None` without one.
fn days_left(expires_at: Option<&str>, now_ms: i64) -> Option<i64> {
    let at = crate::send::parse_iso_utc_millis(expires_at?)?;
    Some(((at - now_ms).max(0) + MILLIS_PER_DAY - 1) / MILLIS_PER_DAY)
}

```

with nothing.

**9e. `share_ui.rs` tests.** Rename the generic report in two tests with the Edit tool and **replace_all**, replacing:

```rust
SharingReport::Done(
```

with:

```rust
SharingReport::LinkAccepted(
```

Expected: 2 replacements, in `a_failed_writes_sentence_outlives_the_view_read_it_causes` and `a_reports_debug_never_carries_the_engines_sentence`.

In `the_rail_counts_come_from_the_view`, replace:

```rust
        let received = |pane: &SharingPane| received_share_rows(pane.view.as_ref(), &items).len();
        let invited = |pane: &SharingPane| !incoming_rows(pane.view.as_ref(), NOW).is_empty();
        assert_eq!((received(&pane), invited(&pane)), (0, false));
```

with:

```rust
        let received = |pane: &SharingPane| received_share_rows(pane.view.as_ref(), &items).len();
        assert_eq!(received(&pane), 0);
```

and:

```rust
        v.incoming.push(crate::rest::sharing::IncomingEntry { id: "i1".into(), from: Some("e@x.io".into()), expires_at: None });
        pane.view = Some(v);
        assert_eq!((received(&pane), invited(&pane)), (1, true));
```

with:

```rust
        pane.view = Some(v);
        assert_eq!(received(&pane), 1);
```

Write `$TEMP/dw/dw3-t-paste.txt`:

```rust
    /// **Paste an invite link** (official-path spec §4.1): Accept is greyed
    /// and deaf while the box is empty or a write is out, and a press with a
    /// link in the box asks once.
    #[test]
    fn the_paste_row_asks_to_accept_the_link_it_holds() {
        let mut people = PeopleRows { paste: Some(Zeroizing::new(String::new())), ..PeopleRows::none() };
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        let accept = texts.iter().find(|(t, _)| t == PASTE_ACCEPT_LABEL).map(|(_, r)| r.center()).expect("Accept");
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, None, "an empty box asks nothing");

        people.paste = Some(Zeroizing::new("https://vault.example/#/accept-organization?token=t".to_string()));
        people.busy = true;
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, None, "a write is out");
        people.busy = false;
        let _ = ui_frame(400.0, click(accept), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(people.asked, Some(PeopleAsk::AcceptLink));
    }

    /// **An accepted sharer who has not finished is one quiet line** -- and
    /// with no box and nobody waited on, nothing is drawn at all.
    #[test]
    fn a_sharer_being_waited_on_is_one_line_and_nothing_is_drawn_without_one() {
        let waiting = ["denis@x.io".to_string()];
        let mut people = PeopleRows { waiting: &waiting, ..PeopleRows::none() };
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert_eq!(texts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), ["Waiting for denis@x.io to finish sharing"]);
        let mut people = PeopleRows::none();
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_paste_and_waiting(ui, &mut people, 376.0));
        assert!(texts.is_empty(), "{texts:?}");
        assert!(paste_and_waiting_height(&PeopleRows::none(), 6.0).abs() < f32::EPSILON);
    }

```

```bash
python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '    /// **An invitation the server does not attest names nobody** (final' '    /// **"Stop receiving from X?"** names the sharer and what goes, and' "$TEMP/dw/dw3-t-paste.txt"
```

This replaces `an_unattested_invitation_names_nobody`, `an_invitation_is_the_first_row_and_accepting_it_is_one_press`, `declining_is_one_press_and_the_cards_click_picks_it_without_taking_the_buttons`, the old `a_sharer_being_waited_on_is_one_line_and_nothing_is_drawn_without_one` and `incoming_rows_carry_the_views_id_and_the_expiry_in_words`.

Write `$TEMP/dw/dw3-t-report.txt`:

```rust
    /// **A pasted link's answer**: accepted, a toast saying the records are on
    /// their way; refused, the engine's sentence -- and the write slot is
    /// free either way. A panicked worker reports as the paste.
    #[test]
    fn a_pasted_links_answer_is_said_and_frees_the_slot() {
        let mut pane = SharingPane { busy: true, ..SharingPane::default() };
        apply(&mut pane, SharingReport::LinkAccepted(Ok(())), 0.0);
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some(LINK_ACCEPTED));
        assert!(!pane.is_busy());
        pane.busy = true;
        apply(&mut pane, SharingCall::AcceptLink(Zeroizing::new("x".into())).failed(PANICKED), 1.0);
        assert_eq!(pane.notice.as_deref(), Some(PANICKED));
        assert!(!pane.is_busy());
    }

```

```bash
python "$TEMP/dw_cut.py" src/vault_window/share_ui.rs '    /// **While a sharing write is out, Accept and Decline are greyed and' '    /// **A finished Stop receiving says so, by name**; a failed one says' "$TEMP/dw/dw3-t-report.txt"
```

This replaces `a_busy_slot_greys_accept_and_decline_and_a_press_asks_nothing`, `a_refused_answer_stands_on_its_invitation_or_goes_to_a_toast` and `a_busy_refusal_pinned_to_an_invitation_is_spent_when_the_slot_frees`.

The last test of the module, `the_invitations_height_counts_the_gaps_between_its_rows`, is replaced with the Edit tool. Replace:

```rust
    /// **The invitations' measured height counts the gaps BETWEEN its rows**:
    /// `n` rows, `n - 1` gaps -- the gap under them is the list's to add.
    #[test]
    fn the_invitations_height_counts_the_gaps_between_its_rows() {
        let incoming = [IncomingRow { key: "invite:i".into(), id: "i".into(), from: Some("d@x.io".into()), expires: None }];
        let waiting = ["e@x.io".to_string()];
        let one = PeopleRows { incoming: &incoming, ..PeopleRows::none() };
        assert!((incoming_rows_height(&one, 6.0) - INVITE_ROW_HEIGHT).abs() < f32::EPSILON);
        let two = PeopleRows { incoming: &incoming, waiting: &waiting, ..PeopleRows::none() };
        assert!((incoming_rows_height(&two, 6.0) - (INVITE_ROW_HEIGHT + WAITING_LINE_HEIGHT + 6.0)).abs() < f32::EPSILON);
        assert!(
            (INVITE_ROW_HEIGHT - (11.0 + 40.0 + 10.0 + theme::SMALL_BUTTON_HEIGHT + 11.0)).abs() < f32::EPSILON,
            "6d's `.invite`: padding, the avatar, the gap and a `.btn.sm`"
        );
    }
```

with:

```rust
    /// **The paste row's and the waiting lines' measured height counts the
    /// gaps BETWEEN its rows**: `n` rows, `n - 1` gaps -- the gap under them
    /// is the list's to add.
    #[test]
    fn the_paste_and_waiting_height_counts_the_gaps_between_its_rows() {
        let waiting = ["e@x.io".to_string()];
        let one = PeopleRows { paste: Some(Zeroizing::new(String::new())), ..PeopleRows::none() };
        assert!((paste_and_waiting_height(&one, 6.0) - PASTE_ROW_HEIGHT).abs() < f32::EPSILON);
        let two = PeopleRows { paste: Some(Zeroizing::new(String::new())), waiting: &waiting, ..PeopleRows::none() };
        assert!((paste_and_waiting_height(&two, 6.0) - (PASTE_ROW_HEIGHT + WAITING_LINE_HEIGHT + 6.0)).abs() < f32::EPSILON);
        assert!((PASTE_ROW_HEIGHT - theme::BUTTON_HEIGHT).abs() < f32::EPSILON, "one inline field high");
    }
```

**9f. `send_ui.rs`.** Replace:

```rust
    // And on `Shared with me`, once a record from a person is listed, 6d's
    // own note replaces 5b's for the same reason: the list holds two kinds
    // now, and the note says what each is. An invitation, or a sharer being
    // waited on, counts: 6d's invite-pending list wears that note too, and
    // it is what says where an accepted invitation's records will arrive.
    let gloss = if !shares.is_empty() {
        SHARES_SUBTEXT
    } else if matches!(view, SendView::Received(_))
        && !(people.received.is_empty() && people.incoming.is_empty() && people.waiting.is_empty())
    {
```

with:

```rust
    // And on `Shared with me`, once a record from a person is listed, 6d's
    // own note replaces 5b's for the same reason: the list holds two kinds
    // now, and the note says what each is. A sharer being waited on counts:
    // the note is what says where an accepted invitation's records arrive.
    let gloss = if !shares.is_empty() {
        SHARES_SUBTEXT
    } else if matches!(view, SendView::Received(_)) && !(people.received.is_empty() && people.waiting.is_empty()) {
```

Replace:

```rust
    // `extra` is the height of rows of another shape among them -- 6d's
    // invitation cards and waiting lines, each with its gap after it
    // (`share_ui::incoming_rows_height`) -- for the same reason.
```

with:

```rust
    // `extra` is the height of rows of another shape among them -- Paste an
    // invite link's row and the waiting lines, each with its gap after it
    // (`share_ui::paste_and_waiting_height`) -- for the same reason.
```

Write `$TEMP/dw/dw3-received.txt`:

```rust
        // **Mock 6d: FROM PEOPLE, then IMPORTED FROM SENDS.** The records
        // people share with this account first -- live, and theirs -- then
        // the imports, which are the user's own copies. The eyebrows are
        // drawn only where there are two groups to tell apart: an account
        // nobody shares with keeps the list it always had, and one with no
        // imports gets no eyebrow over nothing.
        //
        // **Paste an invite link is the first row** (official-path spec
        // §4.1), where sharing is available, and the accepted-but-unfinished
        // sharers' waiting lines follow it: both are about what is ON ITS
        // WAY, above what is here. Nothing is picked for the user any more:
        // there is no invitation pane to open beside the list.
        SendView::Received(rows) => {
            let leads = people.paste.is_some() || !people.waiting.is_empty();
            if rows.is_empty() && people.received.is_empty() && people.waiting.is_empty() {
                // Nothing listed: the box, if offered, over the empty
                // screen's words -- which are still true.
                if people.paste.is_some() {
                    ui.add_space(LIST_PAD);
                    egui::Frame::new()
                        .inner_margin(Margin { left: LIST_PAD as i8, right: 0, top: 0, bottom: 0 })
                        .show(ui, |ui| super::share_ui::draw_paste_and_waiting(ui, people, row_width));
                }
                headline(ui, RECEIVED_EMPTY_HEADLINE, RECEIVED_EMPTY_DETAIL, theme::INK);
            } else {
                let received = people.received.len();
                // 6d's IMPORTED FROM SENDS stands over the imports whenever
                // anything from people -- records, a sharer being waited on --
                // is listed above them.
                let from_people = received > 0 || !people.waiting.is_empty();
                let two_groups = !rows.is_empty() && from_people;
                let eyebrows = usize::from(received > 0) + usize::from(two_groups);
                // The paste row's and the waiting lines' own height and the
                // gaps between them, and the one gap joining them to whatever
                // is listed under them.
                let others = rows.len() + received + eyebrows;
                let lead = super::share_ui::paste_and_waiting_height(people, LIST_ROW_GAP)
                    + if others > 0 && leads { LIST_ROW_GAP } else { 0.0 };
                rows_area(ui, rows.len() + received, eyebrows, lead, &mut |ui| {
                    super::share_ui::draw_paste_and_waiting(ui, people, row_width);
                    if let Some(open) = super::share_ui::draw_received_group(ui, people, row_width, ROW_HEIGHT, selected, records) {
                        action = Some(open);
                    }
                    if two_groups {
                        super::share_ui::group_eyebrow(ui, row_width, super::share_ui::FROM_SENDS_EYEBROW);
                    }
                    for row in rows.iter() {
                        draw_received_row(ui, row, row_width, selected);
                    }
                });
            }
        }
```

```bash
python "$TEMP/dw_cut.py" src/vault_window/send_ui.rs '        // **Mock 6d: FROM PEOPLE, then IMPORTED FROM SENDS.** The records' '        SendView::Mine(scope) => match state {' "$TEMP/dw/dw3-received.txt"
```

Replace in the detail dispatch:

```rust
        SendView::Received(rows) => {
            // **An invitation key opens the invitation's own pane** (6d) --
            // while that invitation is still listed. One answered a moment
            // ago names nothing now and falls through to "nothing picked".
            if let Some(key) = selected
                .as_deref()
                .filter(|key| key.starts_with(super::share_ui::INVITE_KEY_PREFIX))
                .filter(|key| people.incoming.iter().any(|row| row.key == *key))
            {
                let key = key.to_string();
                super::share_ui::draw_invitation_detail(ui, people, &key);
                return action;
            }
            let picked =
```

with:

```rust
        SendView::Received(rows) => {
            let picked =
```

Delete `draw_invitation_pane`:

```bash
python "$TEMP/dw_cut.py" src/vault_window/send_ui.rs '/// **An invitation'"'"'s own pane** (mock 6d, invite pending), drawn for' '/// What one row of a fact card shows in its value column.'
```

Replace the test `an_invitation_leads_shared_with_me_and_its_pane_opens_beside_it`. Write `$TEMP/dw/dw3-t-send.txt`:

```rust
    /// **Paste an invite link leads Shared with me** (official-path spec
    /// §4.1), above FROM PEOPLE; the strip counts records and not the box;
    /// nothing is picked for the user; with nothing listed but the box, the
    /// empty screen's words stand under it; and a sharer being waited on
    /// alone is a list with its line in it, never the empty screen.
    #[test]
    fn paste_an_invite_link_leads_shared_with_me() {
        use crate::vault_window::share_ui::{self as sharing, PeopleRows, ReceivedShareRow};
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![a_received_record(NOW - 3_600_000, "SAP Production", "item-here")],
        };
        let rows = received_rows(&history, &[vault_item("item-here")], &FixedClock(NOW), &UTC);
        let from_people = [ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into() }];
        let mut people = PeopleRows { received: &from_people, paste: Some(zeroize::Zeroizing::new(String::new())), ..PeopleRows::none() };
        let (painted, selected) = paint_received_people(&rows, &mut people, None);
        assert_eq!(selected, None, "nothing is picked for the user");
        let top = |label: &str| painted.rect_of(label).unwrap_or_else(|| panic!("no {label:?} in {:?}", painted.text)).top();
        assert!(top(sharing::PASTE_ACCEPT_LABEL) < top(sharing::FROM_PEOPLE_EYEBROW));
        assert!(painted.has(&format!("2 {RECORDS_NOUN}")), "the box is not a record: {:?}", painted.text);

        let mut box_only = PeopleRows { paste: Some(zeroize::Zeroizing::new(String::new())), ..PeopleRows::none() };
        let (alone, _) = paint_received_people(&[], &mut box_only, None);
        assert!(alone.has(sharing::PASTE_ACCEPT_LABEL), "{:?}", alone.text);
        assert!(alone.has(RECEIVED_EMPTY_HEADLINE), "{:?}", alone.text);

        let waiting = ["denis@x.io".to_string()];
        let mut only_waiting = PeopleRows { waiting: &waiting, ..PeopleRows::none() };
        let (painted, _) = paint_received_people(&[], &mut only_waiting, None);
        assert!(painted.has(&sharing::waiting_line("denis@x.io")), "{:?}", painted.text);
        assert!(!painted.has(RECEIVED_EMPTY_HEADLINE), "{:?}", painted.text);
    }

```

```bash
python "$TEMP/dw_cut.py" src/vault_window/send_ui.rs '    /// **Mock 6d, invite pending**: the invitation is the list'"'"'s first row,' '    /// **The activity sentence says what happened and what is left, and never' "$TEMP/dw/dw3-t-send.txt"
```

**9g. `mod.rs`.** Replace:

```rust
        // **Design 6d's invitations, and the sharers being waited on**, built
        // beside them for the same reason. The rail's dot on Shared with me
        // is read off `incoming_rows` too -- the badge = list rule: a dot
        // with no invitation row under it cannot happen.
        let incoming_rows = share_ui::incoming_rows(
            sharing.view.as_ref(),
            crate::send::SendClock::now_unix_millis(&crate::send::SystemClock),
        );
        let waiting_from: Vec<String> =
```

with:

```rust
        // **The sharers being waited on** (spec 6.9), built beside them for
        // the same reason. The rail's dot on Shared with me is read off them
        // too -- the badge = list rule: a dot with no line under it cannot
        // happen. There are no invitation rows: an invitation is accepted
        // from its link (Paste an invite link) or in the web vault.
        let waiting_from: Vec<String> =
```

Replace:

```rust
                    invitation_waiting: !incoming_rows.is_empty(),
```

with:

```rust
                    invitation_waiting: !waiting_from.is_empty(),
```

Replace:

```rust
        // **On Shared with me the same rule holds for an invitation**, by its
        // key: an Accept or a Decline is asked as a write about the
        // invitation on screen (`request_on` below), so its failure stands at
        // the foot of that invitation's pane, and one about anything else --
        // a record, another invitation -- goes to a toast.
        let sent_share_record = selected_send.as_deref().and_then(share_ui::parse_share_key).map(|(item_id, _)| item_id);
        let shown_invitation =
            selected_send.as_deref().filter(|key| key.starts_with(share_ui::INVITE_KEY_PREFIX)).map(str::to_string);
        let notice_about = if on_received { shown_invitation } else { sent_share_record };
```

with:

```rust
        // **On Shared with me nothing on screen is a record a write was
        // about** -- Paste an invite link names none -- so every sentence
        // there is a toast.
        let sent_share_record = selected_send.as_deref().and_then(share_ui::parse_share_key).map(|(item_id, _)| item_id);
        let notice_about = if on_received { None } else { sent_share_record };
```

Replace:

```rust
            received: &received_share_rows,
            incoming: &incoming_rows,
            waiting: &waiting_from,
            busy: sharing.is_busy(),
        };
```

with:

```rust
            received: &received_share_rows,
            // The box's text is the pane's between frames; the list borrows
            // it for this one and hands it back below. Offered only where
            // sharing is available: a link accepted elsewhere is refused.
            paste: sharing.view.as_ref().is_some_and(|view| view.available).then(|| std::mem::take(&mut sharing.paste)),
            waiting: &waiting_from,
            busy: sharing.is_busy(),
        };
```

Replace:

```rust
        // **What the Sent detail asked of sharing** (design 6c): the same
```

with:

```rust
        // The paste box's text goes back to the pane, whatever was pressed.
        if let Some(text) = people_rows.paste.take() {
            sharing.paste = text;
        }
        // **What the Sent detail asked of sharing** (design 6c): the same
```

Replace the two invitation arms:

```rust
            // **An invitation's answer** (6d), from its row or its pane: one
            // press, by the view's id -- the token is the daemon's. Asked
            // about the invitation's own key, so a refusal stands on its pane
            // (`notice_about` above) and not on whatever record is shown next.
            //
            // A request that is REFUSED -- no line to the daemon, a write
            // already out -- has its sentence pinned to the invitation too
            // (`pin_notice_to`): said on its pane when that is on screen, and
            // as a toast when the press came from a row beside another pick.
            Some(share_ui::PeopleAsk::Accept(id)) => {
                let key = format!("{}{id}", share_ui::INVITE_KEY_PREFIX);
                let call = share_ui::SharingCall::Accept(id);
                if !share_ui::request_on(&mut sharing, &key, sharing_client.as_ref(), ui.ctx(), &sharing_tx, call) {
                    sharing.pin_notice_to(&key);
                }
            }
            Some(share_ui::PeopleAsk::Decline(id)) => {
                let key = format!("{}{id}", share_ui::INVITE_KEY_PREFIX);
                let call = share_ui::SharingCall::Decline(id);
                if !share_ui::request_on(&mut sharing, &key, sharing_client.as_ref(), ui.ctx(), &sharing_tx, call) {
                    sharing.pin_notice_to(&key);
                }
            }
```

with:

```rust
            // **Paste an invite link's Accept** (official-path spec §4.1): the
            // box's text, taken whole -- the box empties as the link goes, and
            // the token is wiped when the call ends. A refused request (a
            // write already out) has said why, as a toast on this screen.
            Some(share_ui::PeopleAsk::AcceptLink) => {
                let link = std::mem::take(&mut sharing.paste);
                let _ = share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::AcceptLink(link));
            }
```

Run: `python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/send_ui.rs src/vault_window/mod.rs`
Expected: three `0`s.

Run: `cargo check -j 2 2>&1 | grep -E "^(error|warning)"`
Expected: no output.
- If a `warning: … never used` names a `send_ui.rs` item, check with `grep -n "<name>(" src/vault_window/send_ui.rs` that `draw_invitation_pane` was its only caller, and delete it.
- If it names anything else, the warning is not this step's to silence. Read it again.

- [ ] **Step 10: Run the tests**

Run: `cargo test --lib -j 2 sharing 2>&1 | tail -5`, then `cargo test --lib -j 2 share_ui`, `cargo test --lib -j 2 send_ui`, `cargo test --lib -j 2 vault_api`, `cargo test --lib -j 2 daemon_link`, `cargo test --lib -j 2 invite_link`.
Expected: each `test result: ok.`

Run: `grep -rn "IncomingRow\|incoming_rows\|draw_invitation\|INVITE_KEY_PREFIX\|PeopleAsk::Accept(\|PeopleAsk::Decline\|SharingCall::Accept(\|SharingCall::Decline\|pin_notice_to\|invitations/mine\|INVITATION_GONE\|IncomingEntry" src --include=*.rs`
Expected: no output.

- [ ] **Step 11: Verify the four commands**

Run each of the four commands in Global Constraints › Deskwarden › Verify per task.
Expected: all pass, 0 warnings.

Run: `grep -r --include=*.rs -o "#\[test\]" src | wc -l`
Expected: `6136`.

- [ ] **Step 12: Commit**

```bash
cd /e/Personal/node-bitwarden
git add deskwarden/src/invite_link.rs deskwarden/src/lib.rs deskwarden/src/foreground.rs deskwarden/src/rest/sharing.rs deskwarden/src/rest/sharing_api.rs deskwarden/src/vault_api.rs deskwarden/src/daemon_link.rs deskwarden/src/vault_host.rs deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/send_ui.rs deskwarden/src/vault_window/mod.rs
git commit -F - <<'EOF'
An invitation is accepted by pasting its link, and Shared with me lists no invitations

The recipient's invitations came from NodeWarden's /invitations/mine,
which no official server has, and were answered with NodeWarden's own
accept and decline. Shared with me now offers Paste an invite link
instead: the daemon reads the link -- Bitwarden's, Vaultwarden's or
NodeWarden's format, the query in the fragment or the URL, keys in any
case, both ids GUIDs -- refuses one for another account in words, and
accepts with POST /organizations/{orgId}/users/{ouId}/accept {token}.
The sharer's daemon then confirms as before, and Shared with me says it
is waiting for them meanwhile.

There is no Decline: Bitwarden has none, and an invitation nobody
answers expires. The invitation rows, their pane and their routes are
gone. The link's token stays Zeroizing from the box to the request, and
nothing logs it; the new invite_link module's Debug prints none of it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task DW4: "Shared by" from the roster, or marked unverified

**Files:**
- Modify: `src/rest/sync.rs:255-266` (`Organization.owner_email`), test `:2183-2186`
- Modify: `src/rest/organizations.rs`:
  - `:89` (imports);
  - `:170-219` (`sharing_mark`, `SharingOrg`);
  - `:418-423` (`Audience::SharedWithMe`);
  - `:608-619` (`audience_of`);
  - `:656-662` (`from_sync`'s `mark`);
  - `:763-766` (`load`'s comment), plus a new `Directory::attest_received`;
  - `:998-1008`, `:1059-1077` (the wire);
  - tests `:1727-1830`.
- Modify: `src/rest/api.rs` (new `fetch_organization_mini_details`, after `fetch_organization_members`)
- Modify: `src/rest/backend.rs:655-669` (`synced`), `:4171-4175` (fixture)
- Modify: `src/rest/sharing.rs`:
  - `ReceivedEntry`, `WaitingEntry`, `SharingView` helpers;
  - `Held`;
  - `note_directory`;
  - `view`;
  - `owned_and_named`;
  - `accept_link`, `leave`;
  - new `roster_readable` / `note_roster_readable`;
  - tests.
- Modify: `src/sharing_state.rs:89-95` (doc)
- Modify: `src/vault_window/share_ui.rs`:
  - `ReceivedShareRow` (`:2612-2629`), `received_share_rows` (`:2631-2651`), `draw_received_group` (`:2705`);
  - `people_sharing` (`:1254`);
  - tests.
- Modify: `src/vault_window/detail.rs:1030-1049` (words), `:3797-3824` (subtitle), `:6384` (`PeopleSharing`), test harness `:10739`, `:11181`, `:11205`, `:11297`, and a test
- Modify: `src/vault_window/detail_edit.rs:10272`, `src/vault_window/item_list.rs:2171` and test `:4397`, `src/vault_window/send_ui.rs` tests

**Interfaces:**
- Consumes:
  - `Capabilities::roster_readable` and `roster_says_mine` (DW1);
  - `Held.waiting` and `accept_link` (DW3);
  - NodeWarden and Vaultwarden answering a member's `mini-details` with 404 (NodeWarden: `organization-users.ts:152-169`).
- Produces:
  - `SharingOrg { sharer: String, mine: bool, verified: bool }`.
  - `pub fn sharing_mark(claimed: Option<&str>, is_owner: bool, account_email: &str) -> Option<SharingOrg>`, which no longer takes `owner_email`.
  - `Directory::attest_received(&mut self, client: &RestClient, session: &mut Session) -> Option<bool>`.
  - `RestClient::fetch_organization_mini_details(&self, session: &mut Session, id: &str) -> Result<serde_json::Value, RestError>`.
  - `Audience::SharedWithMe { from: String, verified: bool }`.
  - `ReceivedEntry { item_id, from, organization_id: String, verified: bool }` and `WaitingEntry { organization_id, from, verified: bool }`.
  - `SharingView::received_verified(&self, item_id: &str) -> bool`.
  - `Sharing::roster_readable(&self) -> Option<bool>` and `Sharing::note_roster_readable(&self, learned: Option<bool>)`.
  - `Held.unverified: BTreeSet<String>`.
  - In `share_ui`: `ReceivedShareRow.verified: bool` and `received_subtitle(row: &ReceivedShareRow) -> String`.
  - In `detail`: `PeopleSharing.view_only_unverified: bool`, `shared_by_line(from: &str, unverified: bool) -> String` and `UNVERIFIED_SHARER_TOOLTIP: &str`.
  - Removed: `Organization::owner_email`.
- Test count: +6 (6136 → 6142).

- [ ] **Step 1: Write the failing tests**

`organizations.rs`: replace the `sharing_mark` test. Write `$TEMP/dw/dw4-t-mark.txt`:

```rust
    /// **A "Shared by" name is a claim** (official-path spec §4.3): anyone can
    /// create an org named "Shared by boss@..." and add anyone. Received, it
    /// is a sharing org from the name's sharer, marked unverified; owned and
    /// named for this account, it is this account's candidate; named for this
    /// account and not owned by it, or owned and named for someone else, it
    /// is no sharing org at all.
    #[test]
    fn a_shared_by_name_is_a_claim_until_a_roster_says_otherwise() {
        let me = "denis@example.com";
        let mark = |name: &str, is_owner: bool| sharing_mark(sharer_of(name), is_owner, me);
        assert_eq!(
            mark("Shared by Boss@Corp.example", false),
            Some(SharingOrg { sharer: "boss@corp.example".into(), mine: false, verified: false }),
            "received: the name's claim, unverified"
        );
        assert_eq!(mark("Shared by denis@example.com", true), Some(SharingOrg { sharer: me.into(), mine: true, verified: true }), "my own");
        assert_eq!(mark("Shared by denis@example.com", false), None, "named for me and owned by someone else: planted, not a share");
        assert_eq!(mark("Shared by anna@example.com", true), None, "my org named for someone else is no share");
        assert_eq!(mark("Engineering", false), None, "no sharing name, no sharing org");
        let long = "denis.platonov@a-very-long-domain-name-for-testing.example";
        let cut = sharing_org_name(long);
        assert!(cut.ends_with('\u{2026}'), "{cut}");
        assert_eq!(sharing_mark(sharer_of(&cut), true, long).map(|s| s.sharer), Some(long.to_string()), "mine: the whole address, not the cut name");
        assert_eq!(sharer_of("Engineering"), None);
        assert_eq!(sharer_of("Shared by nobody"), None, "no `@`, not an email");

        // Through a whole read: a received name-only org is a sharing org,
        // unverified; one named for me that I do not own is an ordinary org.
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "email": me,
                "organizations": [
                    { "id": "claimed", "name": "Shared by boss@corp.example", "type": 2, "key": "4.zzz" },
                    { "id": "planted", "name": "Shared by denis@example.com", "type": 2, "key": "4.zzz" }
                ],
                "organizationsNew": [
                    { "id": "waiting", "name": "Shared by boss@corp.example", "type": 2, "status": 1 }
                ]
            },
            "ciphers": [], "folders": [], "collections": []
        })).expect("parses");
        let directory = Directory::from_sync(&response, &keys());
        assert_eq!(
            directory.audience_of(&item(Some("claimed"), &[])),
            Audience::SharedWithMe { from: "boss@corp.example".to_string(), verified: false }
        );
        assert!(directory.sharing_orgs().iter().all(|o| o.id != "planted"), "a planted org is not a sharing org");
        assert!(directory.visible_organisations().iter().any(|o| o.id == "planted"), "it is shown as the organisation it is");
        assert_eq!(
            directory.waiting().iter().map(|o| o.sharing.clone()).collect::<Vec<_>>(),
            [Some(SharingOrg { sharer: "boss@corp.example".into(), mine: false, verified: false })]
        );
    }

    /// **A received sharing org's sharer is the server's word where it gives
    /// one** (official-path spec §4.3). Official Bitwarden lets a member read
    /// `mini-details`, and the sharer becomes the org's one confirmed owner --
    /// a forged "Shared by boss@" says who really owns it -- marked verified;
    /// two owners leave the name's claim. A server that answers a member 404
    /// ends the asking, and its orgs keep the claim, unverified.
    #[test]
    fn a_received_orgs_sharer_is_read_off_a_roster_a_member_may_read() {
        let mut server = crate::test_http::server();
        server.mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let mut session = client.password_grant("a@b.c", "HASH", &device).expect("grant");
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": { "key": "2.aaa|bbb|ccc", "email": "denis@example.com", "organizations": [
                { "id": "forged", "name": "Shared by boss@corp.example", "type": 2, "key": "4.zzz" },
                { "id": "shared", "name": "Shared by anna@example.com", "type": 2, "key": "4.zzz" }
            ]},
            "ciphers": [], "folders": [], "collections": []
        })).expect("parses");
        let row = |id: &str, email: &str, kind: i64| json!({ "id": id, "userId": id, "email": email, "type": kind, "status": 2 });
        server
            .mock("GET", "/api/organizations/forged/users/mini-details")
            .with_body(json!({ "data": [row("m", "Mallory@Evil.example", 0), row("d", "denis@example.com", 2)] }).to_string())
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/shared/users/mini-details")
            .with_body(json!({ "data": [row("a", "anna@example.com", 0), row("o", "other@example.com", 0)] }).to_string())
            .expect(1)
            .create();
        let mut directory = Directory::from_sync(&response, &keys());
        assert_eq!(directory.attest_received(&client, &mut session), Some(true));
        let sharing = |directory: &Directory, id: &str| directory.organisations().iter().find(|o| o.id == id).and_then(|o| o.sharing.clone());
        assert_eq!(
            sharing(&directory, "forged"),
            Some(SharingOrg { sharer: "mallory@evil.example".into(), mine: false, verified: true }),
            "the real owner, on the server's word"
        );
        assert_eq!(
            sharing(&directory, "shared"),
            Some(SharingOrg { sharer: "anna@example.com".into(), mine: false, verified: false }),
            "two owners: still only the name's claim"
        );

        let mut hidden = Directory::from_sync(&response, &keys());
        server.mock("GET", "/api/organizations/forged/users/mini-details").with_status(404).expect(1).create();
        let asked_again = server.mock("GET", "/api/organizations/shared/users/mini-details").expect(0).create();
        assert_eq!(hidden.attest_received(&client, &mut session), Some(false));
        asked_again.assert();
        assert_eq!(sharing(&hidden, "forged").map(|s| s.verified), Some(false));
    }

```

```bash
python "$TEMP/dw_cut.py" src/rest/organizations.rs '    /// **A "Shared by" name is believed only when the server attests it**' '    /// **The roster route is `/users` and nothing else**: NodeWarden'"'"'s older' "$TEMP/dw/dw4-t-mark.txt"
```

In the visible-organisations test (`sharing_orgs_are_left_out_of_the_visible_organisations_and_answer_as_people`), replace:

```rust
                    { "id": "theirs", "name": "Shared by anna@example.com", "role": "member", "key": "4.zzz", "ownerEmail": "anna@example.com" }
                ],
                "organizationsNew": [
                    { "id": "pending", "name": "Shared by carol@example.com", "role": "member", "status": 1, "ownerEmail": "carol@example.com" }
```

with:

```rust
                    { "id": "theirs", "name": "Shared by anna@example.com", "role": "member", "key": "4.zzz" }
                ],
                "organizationsNew": [
                    { "id": "pending", "name": "Shared by carol@example.com", "role": "member", "status": 1 }
```

and replace:

```rust
            Audience::SharedWithMe { from: "anna@example.com".to_string() }
        );
        assert!(directory.audience_of(&item(Some("theirs"), &[])).is_shared());
```

with:

```rust
            Audience::SharedWithMe { from: "anna@example.com".to_string(), verified: false }
        );
        assert!(directory.audience_of(&item(Some("theirs"), &[])).is_shared());
```

`sharing.rs`: write `$TEMP/dw/dw4-t-engine.txt`:

```rust
    /// **A received record says whether its sharer is the server's word**
    /// (official-path spec §4.3), and carries its organisation, which Shared
    /// with me groups by; a sharer being waited on is marked the same way.
    #[test]
    fn received_records_and_waiting_sharers_say_whether_the_sharer_is_verified() {
        let engine = offline();
        {
            let mut held = engine.held();
            held.capabilities.loaded = true;
            held.others = vec![("org-anna".into(), "anna@x.io".into()), ("org-boss".into(), "boss@x.io".into())];
            held.waiting = vec![("org-carol".into(), "carol@x.io".into())];
            held.unverified = ["org-boss".to_string(), "org-carol".to_string()].into_iter().collect();
        }
        let items = [an_item("r1", "org-anna", &[]), an_item("r2", "org-boss", &[])];
        let view = engine.view(&items);
        assert_eq!(
            view.received,
            vec![
                ReceivedEntry { item_id: "r1".into(), from: "anna@x.io".into(), organization_id: "org-anna".into(), verified: true },
                ReceivedEntry { item_id: "r2".into(), from: "boss@x.io".into(), organization_id: "org-boss".into(), verified: false },
            ]
        );
        assert!(view.received_verified("r1"));
        assert!(!view.received_verified("r2"));
        assert!(!view.received_verified("nothing"));
        assert_eq!(view.waiting, vec![WaitingEntry { organization_id: "org-carol".into(), from: "carol@x.io".into(), verified: false }]);
    }

    /// **A whole read files which received orgs are unverified** -- the
    /// directory's own mark -- for the view to say so.
    #[test]
    fn a_whole_read_files_the_unverified_sharers() {
        let (_server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        // `sharing_profile` (backend.rs's fixture) lists it in both
        // `organizations` and `organizationsNew`, as a confirmed member.
        let body = sharing_profile(serde_json::json!([
            { "id": "org-boss", "name": "Shared by boss@x.io", "type": 2, "status": 2, "enabled": true }
        ]));
        let profile: Profile = serde_json::from_value(body.clone()).expect("profile");
        let response: crate::rest::sync::SyncResponse =
            serde_json::from_value(serde_json::json!({ "profile": body, "ciphers": [], "folders": [], "collections": [] })).expect("sync");
        let directory = crate::rest::organizations::Directory::from_sync(&response, &engine.keys_from(&profile).expect("keys"));
        engine.note_directory(&directory, Some(&profile));
        let held = engine.held();
        assert_eq!(held.others, vec![("org-boss".to_string(), "boss@x.io".to_string())]);
        assert!(held.unverified.contains("org-boss"));
    }

    /// **What a whole read learns about rosters is kept for the session**:
    /// a read that learned nothing changes nothing.
    #[test]
    fn what_a_whole_read_learns_about_rosters_is_kept_for_the_session() {
        let engine = offline();
        assert_eq!(engine.roster_readable(), None);
        engine.note_roster_readable(None);
        assert_eq!(engine.roster_readable(), None, "nothing learned changes nothing");
        engine.note_roster_readable(Some(false));
        engine.note_roster_readable(None);
        assert_eq!(engine.roster_readable(), Some(false));
    }

```

Insert them before the stop-receiving test:

```bash
python - "$TEMP/dw/dw4-t-engine.txt" <<'EOF'
import sys
path = "src/rest/sharing.rs"
marker = "    /// **Stop receiving from X** (spec 6.9): leave the sharer's org, and every".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, add + marker, 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

`share_ui.rs`: write `$TEMP/dw/dw4-t-rows.txt`:

```rust
    /// **An unverified sharer says so, and is grouped by organisation**
    /// (official-path spec §4.3): two orgs that claim one sharer stay two
    /// groups, in the order each first appears, never merged under a name
    /// neither has proved.
    #[test]
    fn received_rows_are_grouped_by_organisation_and_say_when_the_sharer_is_unverified() {
        let mut v = SharingView { available: true, ..SharingView::default() };
        let entry = |item: &str, org: &str, verified: bool| crate::rest::sharing::ReceivedEntry {
            item_id: item.into(),
            from: "boss@x.io".into(),
            organization_id: org.into(),
            verified,
        };
        v.received = vec![entry("a", "org-1", false), entry("b", "org-2", true), entry("c", "org-1", false)];
        let items: Vec<VaultItem> =
            ["a", "b", "c"].iter().map(|id| serde_json::from_value(serde_json::json!({ "id": id, "name": id })).expect("item")).collect();
        let rows = received_share_rows(Some(&v), &items);
        assert_eq!(rows.iter().map(|r| r.item_id.as_str()).collect::<Vec<_>>(), ["a", "c", "b"]);
        assert_eq!(received_subtitle(&rows[0]), "from boss@x.io \u{b7} unverified \u{b7} view-only");
        assert_eq!(received_subtitle(&rows[2]), "from boss@x.io \u{b7} view-only");
    }

```

```bash
python - "$TEMP/dw/dw4-t-rows.txt" <<'EOF'
import sys
path = "src/vault_window/share_ui.rs"
marker = "    /// **\"Stop receiving from X?\"** names the sharer and what goes, and".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, add + marker, 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

`detail.rs`: write `$TEMP/dw/dw4-t-detail.txt`:

```rust
    /// **An unverified sharer is said in the subtitle** (official-path spec
    /// §4.3), with the spec's tooltip words for why; a verified one is not.
    #[test]
    fn an_unverified_sharer_is_marked_in_the_subtitle() {
        let item = a_login();
        let mut pane = Pane::new().view_only("boss@corp.example").unverified();
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(
            frame.texts.iter().any(|(t, _)| t.contains("Shared by boss@corp.example \u{b7} unverified \u{b7} view-only")),
            "{:?}",
            frame.texts
        );
        assert_eq!(shared_by_line("d@x.io", false), "Shared by d@x.io \u{b7} view-only");
        assert_eq!(
            UNVERIFIED_SHARER_TOOLTIP,
            "This server does not say who owns the organisation. The name comes from the organisation's own title."
        );
    }

```

```bash
python - "$TEMP/dw/dw4-t-detail.txt" <<'EOF'
import sys
path = "src/vault_window/detail.rs"
marker = "    /// **Mock 6d's \u22ee, open**: the one line saying why, and not one entry".encode()
add = open(sys.argv[1], "rb").read().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
data = open(path, "rb").read()
assert data.count(marker) == 1, "marker not unique"
data = data.replace(marker, add + marker, 1)
open(path, "wb").write(data)
print("inserted")
EOF
```

Run: `python "$TEMP/dw_crlf.py" src/rest/organizations.rs src/rest/sharing.rs src/vault_window/share_ui.rs src/vault_window/detail.rs`
Expected: four `0`s.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --lib -j 2 rest::organizations 2>&1 | tail -5`
Expected: compile errors. `sharing_mark` takes four arguments, `SharingOrg` has no field `verified`, and `attest_received`, `received_subtitle`, `shared_by_line` and `unverified` do not exist.

- [ ] **Step 3: No `ownerEmail`; a sharing org carries whether it is verified**

In `src/rest/sync.rs`, replace:

```rust
    /// 0 invited, 1 accepted, 2 confirmed.
    #[serde(default)]
    pub status: Option<i64>,
    /// **The owner's email, as the SERVER attests it** -- NodeWarden's
    /// `ownerEmail`, read off its own roster (the org's accepted owner), which
    /// no member can write. `None` from a server that does not send it.
    ///
    /// It is what a sharing org's "Shared by <email>" NAME is checked
    /// against ([`crate::rest::organizations::sharing_mark`]): anyone can
    /// create an organisation with that name and add anyone to it, so the
    /// name alone is a claim, and a recipient shown "Shared by boss@..."
    /// must be shown it because the server says boss@ owns it.
    #[serde(rename = "ownerEmail", default)]
    pub owner_email: Option<String>,
}
```

with:

```rust
    /// 0 invited, 1 accepted, 2 confirmed.
    #[serde(default)]
    pub status: Option<i64>,
    // NodeWarden's `ownerEmail` is not read (official-path spec §1): no other
    // server sends it. Who owns an organisation is the roster's to say --
    // `organizations::Directory::attest_received` for a received one, and
    // `Sharing::my_org_id` for this account's own.
}
```

and in its test, replace:

```rust
        assert!(profile.organizations[0].owner_email.is_none(), "a server that does not attest says nothing");
        let attested: Organization =
            serde_json::from_value(serde_json::json!({ "id": "o3", "ownerEmail": "anna@example.com" })).expect("org");
        assert_eq!(attested.owner_email.as_deref(), Some("anna@example.com"));
```

with:

```rust
        // NodeWarden's `ownerEmail` is not read: an organisation that sends it
        // still parses, and the field goes nowhere.
        let _with_owner: Organization =
            serde_json::from_value(serde_json::json!({ "id": "o3", "ownerEmail": "anna@example.com" })).expect("org");
```

In `src/rest/organizations.rs`, replace:

```rust
use crate::rest::api::{RestClient, Session};
```

with:

```rust
use crate::rest::api::{RestClient, RestError, Session};
```

Replace `sharing_mark` and `SharingOrg`, from:

```rust
/// **Whether an organisation is a sharing org, and whose** -- the name's
/// claim checked against the server's attestation (controller decision,
/// final review).
```

through the closing `}` of `pub struct SharingOrg` (the line before `/// One organisation this account is a member of.`). Write `$TEMP/dw/dw4-mark.txt`:

```rust
/// **Whether an organisation is a sharing org, and whose** -- the name's
/// claim, before any roster has spoken (official-path spec §4.3).
///
/// - **This account's own candidate**: it owns the org and the name is its
///   own address. Whether it is really this account's -- not one someone
///   else made and added it to as an owner -- is the sharing engine's roster
///   check (`Sharing::my_org_id`); the directory only marks the candidate.
/// - **One this account owns, named for someone else**: no sharing org.
///   Nobody shares it with this account; it is drawn as the organisation it
///   is.
/// - **Named for this account and not owned by it**: no sharing org either.
///   A "share from me" that I do not own is the planted shape, and it is
///   drawn as the ordinary organisation it provably is.
/// - **Anyone else's "Shared by X"**: a sharing org received from X,
///   **unverified** -- the name is plaintext anyone can set.
///   [`Directory::attest_received`] puts the org's one owner in X's place,
///   verified, where the server lets a member read the roster.
///
/// This replaces NodeWarden's `ownerEmail` attestation, which no other
/// server sends. `claimed` is [`sharer_of`] of the name; `None` is no sharing
/// name at all.
#[must_use]
pub fn sharing_mark(claimed: Option<&str>, is_owner: bool, account_email: &str) -> Option<SharingOrg> {
    let claimed = claimed?;
    let account = normalise_email(account_email);
    let names_me = !account.is_empty() && sharer_is(claimed, &account);
    match (is_owner, names_me) {
        (true, true) => Some(SharingOrg { sharer: account, mine: true, verified: true }),
        (false, false) => Some(SharingOrg { sharer: normalise_email(claimed), mine: false, verified: false }),
        _ => None,
    }
}

/// A sharing organisation, as the directory marks one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharingOrg {
    /// The sharer's email: the org's one owner where a roster said so, else
    /// the name's claim (possibly cut with `…`). This account's own, for its
    /// own org.
    pub sharer: String,
    /// This account owns it and is the sharer it names.
    pub mine: bool,
    /// **The sharer is the server's word** -- the one confirmed owner on a
    /// roster a member may read -- and not only the name's claim. Always
    /// true for this account's own. `false` is drawn "Shared by X ·
    /// unverified" (official-path spec §4.3).
    pub verified: bool,
}
```

```bash
python "$TEMP/dw_cut.py" src/rest/organizations.rs '/// **Whether an organisation is a sharing org, and whose** -- the name'"'"'s' '/// One organisation this account is a member of.' "$TEMP/dw/dw4-mark.txt"
```

Replace:

```rust
    /// In someone else's sharing org: who shared it.
    SharedWithMe { from: String },
```

with:

```rust
    /// In someone else's sharing org: who shared it, and whether that is the
    /// server's word or only the org name's claim ([`SharingOrg::verified`]).
    SharedWithMe { from: String, verified: bool },
```

Replace:

```rust
                Audience::SharedWithMe { from: sharing.sharer.clone() }
```

with:

```rust
                Audience::SharedWithMe { from: sharing.sharer.clone(), verified: sharing.verified }
```

Replace in `from_sync`:

```rust
        // Whether `org` is a sharing organisation, and whose: its name's
        // claim checked against the server's `ownerEmail` (`sharing_mark`).
        // Shared between the confirmed organisations below and
        // `organizationsNew`'s not-yet-confirmed ones, since both carry both.
        let mark = |org: &crate::rest::sync::Organization| {
            sharing_mark(org.name.as_deref().and_then(sharer_of), org.owner_email.as_deref(), org.is_owner(), &account_email)
        };
```

with:

```rust
        // Whether `org` is a sharing organisation, and whose: its name's
        // claim (`sharing_mark`), which `attest_received` may replace with
        // the roster's word. Shared between the confirmed organisations below
        // and `organizationsNew`'s not-yet-confirmed ones.
        let mark = |org: &crate::rest::sync::Organization| {
            sharing_mark(org.name.as_deref().and_then(sharer_of), org.is_owner(), &account_email)
        };
```

Replace in `load`:

```rust
        // **Not other people's sharing orgs.** A recipient is a plain member,
        // and NodeWarden answers a plain member's roster request with 404 on
        // both routes -- two requests per org per sync for nothing. Their
        // rosters are also none of this account's business (spec 10.2).
```

with:

```rust
        // **Not other people's sharing orgs.** A recipient is a plain member,
        // who may not read `/users` on any server -- a request per org per
        // sync for nothing. Who shares them is asked of `mini-details`
        // instead, where the server allows it (`Self::attest_received`).
```

Add `attest_received` after `load`. Replace:

```rust
        for id in wanted {
            if let Ok(answer) = client.fetch_organization_members(session, &id) {
                directory.set_roster(&id, &answer);
            }
        }
        directory
    }
}
```

with:

```rust
        for id in wanted {
            if let Ok(answer) = client.fetch_organization_members(session, &id) {
                directory.set_roster(&id, &answer);
            }
        }
        directory
    }

    /// **Who shares each received sharing org, on the server's word where it
    /// gives one** (official-path spec §4.3).
    ///
    /// Official Bitwarden lets every member read `GET …/users/mini-details`
    /// (`[Authorize<MemberOrProviderRequirement>]`). There the sharer is the
    /// org's one confirmed owner -- whatever the name claims, so a forged
    /// "Shared by boss@" says who really owns it -- and the org is marked
    /// verified. Two owners, or none, leave the name's claim, unverified.
    ///
    /// Vaultwarden and NodeWarden answer a plain member 404 (or 403). The
    /// first such answer ends the asking: no other org on that server will
    /// answer differently, and each would cost a request on every whole
    /// read. Their orgs keep the name's claim, unverified.
    ///
    /// Returns what was learned for the session: `Some(true)` a roster was
    /// read, `Some(false)` members may not read one here, `None` nothing to
    /// ask or every ask failed some other way (a transport error says
    /// nothing about the server's rule).
    pub fn attest_received(&mut self, client: &RestClient, session: &mut Session) -> Option<bool> {
        let received: Vec<String> = self
            .organisations
            .iter()
            .filter(|o| o.sharing.as_ref().is_some_and(|s| !s.mine))
            .map(|o| o.id.clone())
            .collect();
        let mut learned = None;
        for id in received {
            match client.fetch_organization_mini_details(session, &id) {
                Ok(answer) => {
                    learned = Some(true);
                    let Some(members) = map_members(&answer) else {
                        continue;
                    };
                    let owners: Vec<&Member> = members
                        .iter()
                        .filter(|m| m.role == "owner" && m.status == MemberStatus::Confirmed && !m.email.trim().is_empty())
                        .collect();
                    if let ([owner], Some(org)) = (owners.as_slice(), self.organisations.iter_mut().find(|o| o.id == id)) {
                        org.sharing = Some(SharingOrg { sharer: normalise_email(&owner.email), mine: false, verified: true });
                    }
                }
                Err(RestError::Status(403 | 404)) => return Some(false),
                Err(_) => {}
            }
        }
        learned
    }
}
```

In the wire types, replace:

```rust
    #[serde(default)]
    sharer: Option<String>,
    #[serde(default)]
    mine: bool,
}
```

with:

```rust
    #[serde(default)]
    sharer: Option<String>,
    #[serde(default)]
    mine: bool,
    /// [`SharingOrg::verified`]. Absent in a body from before it existed, and
    /// read as `false`: unverified is the safe default.
    #[serde(default)]
    verified: bool,
}
```

Replace:

```rust
        sharer: o.sharing.as_ref().map(|s| s.sharer.clone()),
        mine: o.sharing.as_ref().is_some_and(|s| s.mine),
    }
```

with:

```rust
        sharer: o.sharing.as_ref().map(|s| s.sharer.clone()),
        mine: o.sharing.as_ref().is_some_and(|s| s.mine),
        verified: o.sharing.as_ref().is_some_and(|s| s.verified),
    }
```

and:

```rust
        sharing: o.sharer.map(|sharer| SharingOrg { sharer, mine: o.mine }),
```

with:

```rust
        sharing: o.sharer.map(|sharer| SharingOrg { sharer, mine: o.mine, verified: o.verified }),
```

In `src/rest/api.rs`, replace:

```rust
    /// **The one door every sharing route goes through.**
```

with:

```rust
    /// `GET /api/organizations/{id}/users/mini-details` -- every member's id,
    /// account, type, status, name and email.
    ///
    /// Official Bitwarden answers any member
    /// (`[Authorize<MemberOrProviderRequirement>]`). Vaultwarden and
    /// NodeWarden answer managers only, and a plain member gets 404 (or 403).
    /// Read only for a received sharing org's sharer
    /// ([`crate::rest::organizations::Directory::attest_received`]), on
    /// `sync_agent` for [`Self::fetch_organization_members`]' reason.
    pub fn fetch_organization_mini_details(&self, session: &mut Session, id: &str) -> Result<serde_json::Value, RestError> {
        let url = self.organization_url(id, "/users/mini-details")?;
        self.refreshing(session, |session| self.value_from(self.bearer(self.sync_agent.get(&url), session).call()))
    }

    /// **The one door every sharing route goes through.**
```

In `src/rest/backend.rs`, first read what the engine knows **before** the session lock is taken, so that the engine's lock and the session's are never held together. Replace:

```rust
        let mut state = self.locked();
        let response = self.client.sync_refreshing(&mut state.session).map_err(rest_error)?;
```

with:

```rust
        // Read before the session lock: the sharing engine's lock is never
        // taken while the session's is held.
        let ask_received_rosters = self.sharing.roster_readable() != Some(false);
        let mut state = self.locked();
        let response = self.client.sync_refreshing(&mut state.session).map_err(rest_error)?;
```

Then replace:

```rust
        let directory = crate::rest::organizations::Directory::load(
            &self.client,
            &mut state.session,
            &response,
            &keys,
        );
        drop(state);
```

with:

```rust
        let mut directory = crate::rest::organizations::Directory::load(
            &self.client,
            &mut state.session,
            &response,
            &keys,
        );
        // **Who shares each received record, on the server's word where it
        // gives one** (official-path spec §4.3): the received sharing orgs'
        // `mini-details`, asked until the server says members may not read
        // them. It never says otherwise this session, so the engine keeps the
        // answer and later whole reads ask nothing.
        let roster_readable = if ask_received_rosters {
            directory.attest_received(&self.client, &mut state.session)
        } else {
            None
        };
        drop(state);
        self.sharing.note_roster_readable(roster_readable);
```

and in the fixture, replace:

```rust
            "name": format!("Shared by {EMAIL}"),
            // A NodeWarden that attests the owner -- this account.
            "ownerEmail": EMAIL,
            "role": "owner",
```

with:

```rust
            "name": format!("Shared by {EMAIL}"),
            "role": "owner",
```

In `src/sharing_state.rs`, replace:

```rust
    /// **This client CREATED `sharing_org_id`** -- set by the share that made
    /// it, kept while the id stays the same, cleared when another is chosen.
    /// On a server that does not attest an org's owner (no `ownerEmail`) it
    /// is the only thing that lets the cached id be used: an org merely OWNED
    /// and NAMED for this account may be one someone else made and added it
    /// to as owner (`Sharing::my_org_id`). `serde(default)`: a file from
    /// before it reads as "not created here", the safe side.
```

with:

```rust
    /// **This client CREATED `sharing_org_id`** -- set by the share that made
    /// it, kept while the id stays the same, cleared when another is chosen.
    /// Such an id is this account's without a roster read: an org merely
    /// OWNED and NAMED for this account may be one someone else made and
    /// added it to as an owner, and is used only when its roster names no
    /// other owner (`Sharing::my_org_id`). `serde(default)`: a file from
    /// before it reads as "not created here", the safe side.
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sync.rs src/rest/organizations.rs src/rest/api.rs src/rest/backend.rs src/sharing_state.rs`
Expected: five `0`s.

- [ ] **Step 4: The engine carries the mark to the view**

In `src/rest/sharing.rs`, replace:

```rust
/// A record someone shares with this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedEntry {
    pub item_id: String,
    pub from: String,
}
```

with:

```rust
/// A record someone shares with this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedEntry {
    pub item_id: String,
    pub from: String,
    /// The sharing org the record is in -- what Shared with me groups by,
    /// so two orgs that claim one sharer are never merged (official-path
    /// spec §4.3).
    #[serde(default)]
    pub organization_id: String,
    /// The sharer is the server's word, not only the org name's claim
    /// ([`crate::rest::organizations::SharingOrg::verified`]).
    /// `serde(default)` reads an answer without it as unverified.
    #[serde(default)]
    pub verified: bool,
}
```

Replace:

```rust
pub struct WaitingEntry {
    pub organization_id: String,
    pub from: String,
}
```

with:

```rust
pub struct WaitingEntry {
    pub organization_id: String,
    pub from: String,
    /// As [`ReceivedEntry::verified`].
    #[serde(default)]
    pub verified: bool,
}
```

Add the helper after `received_from`. Replace:

```rust
    /// Who shared a received record.
    #[must_use]
    pub fn received_from(&self, item_id: &str) -> Option<&str> {
        self.received.iter().find(|r| r.item_id == item_id).map(|r| r.from.as_str())
    }
```

with:

```rust
    /// Who shared a received record.
    #[must_use]
    pub fn received_from(&self, item_id: &str) -> Option<&str> {
        self.received.iter().find(|r| r.item_id == item_id).map(|r| r.from.as_str())
    }

    /// Whether a received record's sharer is the server's word -- `false`
    /// for the org name's claim alone, and for a record not received at all.
    #[must_use]
    pub fn received_verified(&self, item_id: &str) -> bool {
        self.received.iter().any(|r| r.item_id == item_id && r.verified)
    }
```

In `Held` (DW3's version), replace:

```rust
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
```

with:

```rust
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
    /// The ids of `others` and `waiting` whose sharer is only the org name's
    /// claim (official-path spec §4.3).
    pub(crate) unverified: BTreeSet<String>,
```

and in its `Debug`, replace:

```rust
            .field("waiting", &self.waiting)
```

with:

```rust
            .field("waiting", &self.waiting)
            .field("unverified", &self.unverified)
```

In `note_directory`, replace:

```rust
        held.waiting = directory
            .waiting()
            .iter()
            .filter_map(|o| o.sharing.as_ref().map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
    }
```

with:

```rust
        held.waiting = directory
            .waiting()
            .iter()
            .filter_map(|o| o.sharing.as_ref().map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
        held.unverified = directory
            .sharing_orgs()
            .into_iter()
            .chain(directory.waiting().iter())
            .filter(|o| o.sharing.as_ref().is_some_and(|s| !s.mine && !s.verified))
            .map(|o| o.id.clone())
            .collect();
    }

    /// Whether members may read an organisation's roster on this server, as
    /// far as this session has learned ([`Capabilities::roster_readable`]).
    /// `RestBackend::synced` asks it before asking the received rosters.
    #[must_use]
    pub fn roster_readable(&self) -> Option<bool> {
        self.held().capabilities.roster_readable
    }

    /// What a whole read learned about it
    /// (`organizations::Directory::attest_received`). `None` learned nothing
    /// and changes nothing.
    pub fn note_roster_readable(&self, learned: Option<bool>) {
        if learned.is_some() {
            self.held().capabilities.roster_readable = learned;
        }
    }
```

In `view`, replace:

```rust
                if let Some((_, sharer)) = held.others.iter().find(|(id, _)| id == org) {
                    view.received.push(ReceivedEntry { item_id: item.id.clone(), from: sharer.clone() });
                }
```

with:

```rust
                if let Some((_, sharer)) = held.others.iter().find(|(id, _)| id == org) {
                    view.received.push(ReceivedEntry {
                        item_id: item.id.clone(),
                        from: sharer.clone(),
                        organization_id: org.to_string(),
                        verified: !held.unverified.contains(org),
                    });
                }
```

and:

```rust
            .map(|(organization_id, from)| WaitingEntry { organization_id: organization_id.clone(), from: from.clone() })
```

with:

```rust
            .map(|(organization_id, from)| WaitingEntry {
                organization_id: organization_id.clone(),
                from: from.clone(),
                verified: !held.unverified.contains(organization_id),
            })
```

In `owned_and_named`, replace:

```rust
    crate::rest::organizations::sharing_mark(org.name.as_deref().and_then(sharer_of), None, org.is_owner(), account_email)
        .is_some_and(|mark| mark.mine)
```

with:

```rust
    crate::rest::organizations::sharing_mark(org.name.as_deref().and_then(sharer_of), org.is_owner(), account_email)
        .is_some_and(|mark| mark.mine)
```

In `accept_link`, replace:

```rust
            if !held.waiting.iter().any(|(org, _)| *org == invite.organization_id) {
                held.waiting.push((invite.organization_id.clone(), from.to_string()));
            }
```

with:

```rust
            if !held.waiting.iter().any(|(org, _)| *org == invite.organization_id) {
                held.waiting.push((invite.organization_id.clone(), from.to_string()));
                held.unverified.insert(invite.organization_id.clone());
            }
```

In `leave`, replace:

```rust
            held.others.retain(|(id, _)| id != organization_id);
            held.waiting.retain(|(id, _)| id != organization_id);
```

with:

```rust
            held.others.retain(|(id, _)| id != organization_id);
            held.waiting.retain(|(id, _)| id != organization_id);
            held.unverified.remove(organization_id);
```

DW3's accept test names the waiting sharer from the link's name, so it is unverified. In `a_pasted_link_is_accepted_on_the_official_route`, replace:

```rust
            vec![WaitingEntry { organization_id: "0f0e0d0c-0b0a-4009-8807-060504030201".into(), from: "denis@example.com".into() }]
```

with:

```rust
            vec![WaitingEntry {
                organization_id: "0f0e0d0c-0b0a-4009-8807-060504030201".into(),
                from: "denis@example.com".into(),
                verified: false,
            }]
```

Run: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs`
Expected: `0`.

- [ ] **Step 5: The window says "unverified", and groups by organisation**

In `src/vault_window/share_ui.rs`, replace `ReceivedShareRow`'s `from` field and its doc:

```rust
    /// Who shares it, as the view names them. **For display only**: the
    /// view's sharer is read off the sharing org's name, which its owner
    /// wrote in plaintext and could have spelled as anybody. The view offers
    /// no authenticated alternative for a record received (the collection
    /// under it is the SHARER's, keyed by an org key only they vouch for), so
    /// this is shown as the claim it is and decides nothing.
    pub(crate) from: String,
}
```

with:

```rust
    /// Who shares it, as the view names them. **For display only**: the org's
    /// one owner where the server let a member read the roster, else the org
    /// name's claim, which anyone could have spelled as anybody -- and then
    /// [`Self::verified`] is false and the row says so. It decides nothing.
    pub(crate) from: String,
    /// The sharer is the server's word (official-path spec §4.3); `false`
    /// draws "unverified" in the row's subtitle ([`received_subtitle`]).
    pub(crate) verified: bool,
}

/// A received row's subtitle: who shares it and that it is view-only, with
/// `unverified` between where the sharer is only the organisation name's
/// claim (official-path spec §4.3).
pub(crate) fn received_subtitle(row: &ReceivedShareRow) -> String {
    if row.verified {
        format!("from {} \u{b7} view-only", row.from)
    } else {
        format!("from {} \u{b7} unverified \u{b7} view-only", row.from)
    }
}
```

Replace the first line of `received_share_rows`' doc:

```rust
/// **The received records the window holds, in the view's order.** An entry
```

with:

```rust
/// **The received records the window holds, grouped by organisation, in the
/// view's order within each.** An entry
```

Then replace its body:

```rust
    view.received
        .iter()
        .filter_map(|r| {
            let item = items.iter().find(|i| i.id == r.item_id)?;
            Some(ReceivedShareRow {
                key: format!("{RECEIVED_KEY_PREFIX}{}", item.id),
                item_id: item.id.clone(),
                name: item.name.clone(),
                from: r.from.clone(),
            })
        })
        .collect()
}
```

with:

```rust
    // **Grouped by organisation** (official-path spec §4.3), in the order each
    // first appears: two orgs that claim one sharer are two groups, never
    // merged under a name neither has proved. The sort is stable, so the
    // view's order holds within a group.
    let mut groups: Vec<&str> = Vec::new();
    let mut rows: Vec<(usize, ReceivedShareRow)> = Vec::new();
    for r in &view.received {
        let Some(item) = items.iter().find(|i| i.id == r.item_id) else {
            continue;
        };
        let group = match groups.iter().position(|g| *g == r.organization_id) {
            Some(at) => at,
            None => {
                groups.push(&r.organization_id);
                groups.len() - 1
            }
        };
        rows.push((
            group,
            ReceivedShareRow {
                key: format!("{RECEIVED_KEY_PREFIX}{}", item.id),
                item_id: item.id.clone(),
                name: item.name.clone(),
                from: r.from.clone(),
                verified: r.verified,
            },
        ));
    }
    rows.sort_by_key(|(group, _)| *group);
    rows.into_iter().map(|(_, row)| row).collect()
}
```

In `draw_received_group`, replace:

```rust
                subtitle: &format!("from {} \u{b7} view-only", row.from),
```

with:

```rust
                subtitle: &received_subtitle(row),
```

In `people_sharing`, replace:

```rust
        view_only_from: pane.view.as_ref().and_then(|v| v.received_from(&item.id)),
    }
```

with:

```rust
        view_only_from: pane.view.as_ref().and_then(|v| v.received_from(&item.id)),
        view_only_unverified: pane.view.as_ref().is_some_and(|v| v.received_from(&item.id).is_some() && !v.received_verified(&item.id)),
    }
```

DW2 renamed `ShareDialog::waiting_account` to `invited`, but left its doc saying otherwise. Replace:

```rust
    /// A share that left an invitation for an address with no account.
    pub(crate) invited: Option<String>,
```

with:

```rust
    /// A share that left the person invited, not yet accepted: on official
    /// Bitwarden and with invitation email on, they accept from the email's
    /// link; with no account yet, after they register (official-path spec
    /// §4.1).
    pub(crate) invited: Option<String>,
```

In `src/vault_window/detail.rs`, add the words. Replace:

```rust
/// The received record's kebab entry that leaves its sharer's org.
```

with:

```rust
/// **The received record's subtitle run** (mock 6d): who shares it and that
/// it is view-only -- with `unverified` between where the sharer is only the
/// organisation name's claim (official-path spec §4.3).
#[must_use]
pub fn shared_by_line(from: &str, unverified: bool) -> String {
    if unverified {
        format!("Shared by {from} \u{b7} unverified \u{b7} view-only")
    } else {
        format!("Shared by {from} \u{b7} view-only")
    }
}

/// The hover on an unverified "Shared by" -- the official-path spec §4.3's
/// own words: the planted-organisation risk is shown, not silently trusted.
pub const UNVERIFIED_SHARER_TOOLTIP: &str =
    "This server does not say who owns the organisation. The name comes from the organisation's own title.";

/// The received record's kebab entry that leaves its sharer's org.
```

Replace the subtitle block:

```rust
                        if let Some(from) = people.view_only_from {
                            ui.horizontal(|ui| {
```

with:

```rust
                        if let Some(from) = people.view_only_from {
                            let line = ui.horizontal(|ui| {
```

and:

```rust
                                subtitle_text_in(
                                    ui,
                                    subtitle_folder_id(&item.id),
                                    &format!("Shared by {from} \u{b7} view-only"),
                                    theme::TEXT_MUTED,
                                );
                            });
                            return;
```

with:

```rust
                                subtitle_text_in(
                                    ui,
                                    subtitle_folder_id(&item.id),
                                    &shared_by_line(from, people.view_only_unverified),
                                    theme::TEXT_MUTED,
                                );
                            });
                            if people.view_only_unverified {
                                let _ = line.response.on_hover_text(UNVERIFIED_SHARER_TOOLTIP);
                            }
                            return;
```

In `PeopleSharing`, replace:

```rust
    pub view_only_from: Option<&'a str>,
}
```

with:

```rust
    pub view_only_from: Option<&'a str>,
    /// That sharer is only the organisation name's claim (official-path spec
    /// §4.3): the subtitle says `unverified`, with [`UNVERIFIED_SHARER_TOOLTIP`].
    pub view_only_unverified: bool,
}
```

The test harness. Replace:

```rust
        /// [`Pane::view_only`]), which is every record the user owns.
        view_only_from: Option<String>,
    }
```

with:

```rust
        /// [`Pane::view_only`]), which is every record the user owns.
        view_only_from: Option<String>,
        /// That sharer is unverified ([`Pane::unverified`]).
        view_only_unverified: bool,
    }
```

Replace:

```rust
                sharing_notice: None,
                view_only_from: None,
            }
        }
```

with:

```rust
                sharing_notice: None,
                view_only_from: None,
                view_only_unverified: false,
            }
        }
```

Replace:

```rust
        fn view_only(mut self, from: &str) -> Self {
            self.view_only_from = Some(from.to_string());
            self
        }
```

with:

```rust
        fn view_only(mut self, from: &str) -> Self {
            self.view_only_from = Some(from.to_string());
            self
        }

        /// The same pane, with that sharer only the org name's claim.
        fn unverified(mut self) -> Self {
            self.view_only_unverified = true;
            self
        }
```

Replace:

```rust
                            view_only_from: self.view_only_from.as_deref(),
                            ..PeopleSharing::default()
```

with:

```rust
                            view_only_from: self.view_only_from.as_deref(),
                            view_only_unverified: self.view_only_unverified,
                            ..PeopleSharing::default()
```

In `src/vault_window/detail_edit.rs`, replace:

```rust
        Audience::SharedWithMe { from } => format!("Shared by {from}"),
```

with:

```rust
        Audience::SharedWithMe { from, verified: true } => format!("Shared by {from}"),
        Audience::SharedWithMe { from, verified: false } => format!("Shared by {from} \u{b7} unverified"),
```

In `src/vault_window/item_list.rs`, replace:

```rust
            received: view.received.iter().map(|r| (r.item_id.clone(), r.from.clone())).collect(),
```

with:

```rust
            // An unverified sharer says so here too (official-path spec
            // §4.3): the row's "from" is the org name's claim alone.
            received: view
                .received
                .iter()
                .map(|r| (r.item_id.clone(), if r.verified { r.from.clone() } else { format!("{} \u{b7} unverified", r.from) }))
                .collect(),
```

Bring the test literals to the new fields. Write `$TEMP/dw_literals.py`:

```python
# Adds DW4's new fields to the test literals of ReceivedEntry and
# ReceivedShareRow that predate them. Each literal gets an organisation and
# `verified: true`, which keeps every existing test reading exactly as it did.
import re
paths = [
    "src/vault_window/share_ui.rs",
    "src/vault_window/item_list.rs",
    "src/vault_window/send_ui.rs",
]
entry = re.compile(rb"ReceivedEntry \{ item_id: ([^,{}]+), from: ([^,{}]+) \}")
row = re.compile(rb"(ReceivedShareRow \{ key: [^{}]+?, from: [^,{}]+?) \}")
for path in paths:
    data = open(path, "rb").read()
    data, a = entry.subn(rb'ReceivedEntry { item_id: \1, from: \2, organization_id: "org-x".into(), verified: true }', data)
    data, b = row.subn(rb"\1, verified: true }", data)
    open(path, "wb").write(data)
    print(path, a, "entries,", b, "rows")
```

Run: `python "$TEMP/dw_literals.py"`
Expected (the counts at f2eecc8):
- `src/vault_window/share_ui.rs 4 entries, 2 rows` (entries at about lines 3165, 3429, 4503, 4504; rows at about 4465, 4508);
- `src/vault_window/item_list.rs 1 entries, 0 rows`;
- `src/vault_window/send_ui.rs 0 entries, 2 rows`.

An earlier task that deleted one of those tests makes its count one lower, which is fine. What matters is that none is left in the old shape:

Run: `grep -rn "ReceivedEntry { item_id: [^}]*from: [^,}]* }\|ReceivedShareRow { key: [^}]*from: [^,}]* }" src`
Expected: no output.

The production `ReceivedShareRow` is built over several lines, so the one-line pattern never reaches it.

Run: `python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/detail.rs src/vault_window/detail_edit.rs src/vault_window/item_list.rs src/vault_window/send_ui.rs`
Expected: five `0`s.

Run: `grep -rn 'owner_email\|"ownerEmail"' src --include=*.rs`
Expected: one line, `src/rest/sync.rs`'s test with `"ownerEmail": "anna@example.com"`, which shows the field is ignored. Docs that name `ownerEmail` in backticks, to say it is no longer read, do not match. Any other line is a reader DW1, DW3 or this task missed. Remove it.

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib -j 2 rest:: 2>&1 | tail -5`, then `cargo test --lib -j 2 vault_window 2>&1 | tail -5`.
Expected: `test result: ok.` both times.

A whole read now asks `mini-details` of every received sharing org. On a test server with no mock for it, the request gets the 501 answer, which `attest_received` ignores. No test at f2eecc8 has an `expect(0)` catch-all over another person's organisation (`grep -rn 'Matcher::Regex(.*organizations' src | grep -v FIXTURE_ORG` prints nothing), so none should fail for this. If one does, it is because that test counted every request. Give that test's server `server.mock("GET", Matcher::Regex("/users/mini-details$".into())).with_status(404).expect_at_least(0).create();` and say why in a one-line comment.

- [ ] **Step 7: Verify the four commands, and the README badge**

Run each of the four commands in Global Constraints › Deskwarden › Verify per task.
Expected: all pass, 0 warnings.

Run: `grep -r --include=*.rs -o "#\[test\]" src | wc -l`
Expected: `6142`. That is under 6200, so the `6000+` badge holds.

Run: `cargo test --lib -j 2 the_readme_test_count_is_not_a_boast`
Expected: PASS.

If the count printed above differs from 6142, the tasks before this one added or removed a different number. Then follow Global Constraints › README badge: the badge moves to `6100+` only if the count is 6200 or more.

- [ ] **Step 8: Commit**

```bash
cd /e/Personal/node-bitwarden
git add deskwarden/src/rest/sync.rs deskwarden/src/rest/organizations.rs deskwarden/src/rest/api.rs deskwarden/src/rest/backend.rs deskwarden/src/rest/sharing.rs deskwarden/src/sharing_state.rs deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/detail_edit.rs deskwarden/src/vault_window/item_list.rs deskwarden/src/vault_window/send_ui.rs
git commit -F - <<'EOF'
"Shared by" is the roster's word where a member may read it, and says unverified where not

A received record's sharer was believed only when NodeWarden attested
the organisation's owner (ownerEmail), which no other server sends, so
on official Bitwarden and Vaultwarden a received record named nobody.
ownerEmail is no longer read.

Where the server lets a member read GET .../users/mini-details --
official Bitwarden -- the sharer is the organisation's one confirmed
owner, whatever its name claims. Where it does not -- Vaultwarden,
NodeWarden -- the sharer is the name's claim and the record says
"Shared by X · unverified · view-only", with the hover "This server
does not say who owns the organisation. The name comes from the
organisation's own title." The first refusal ends the asking for the
session. Shared with me groups received records by organisation, so
two organisations that claim one sharer are never merged.

An organisation named for this account that it does not own is no
longer a share from anyone; it is drawn as the organisation it is.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task DW5 (final): The compatibility release check

This task changes no source file. It runs the official `bw` CLI and the official web vault against a local NodeWarden, and the hand checks of spec §7. It records every result in this plan's ledger (below), and commits the ledger.

**Files:**
- Modify: `E:\Personal\node-bitwarden\docs\superpowers\plans\2026-10-08-official-path-sharing.md` (the Ledger section at its end)
- Create (not committed): `E:\Personal\nodewarden\.dev.vars`, which `.gitignore` excludes

**Interfaces:**
- Consumes: everything in NW1–NW4 and DW1–DW4.
- Produces: a filled ledger. The release condition holds only when every row reads PASS.

**Permissions.** Each of these is a download, so ask the user first. Name the file, its source and its size.
- `@bitwarden/cli@2026.9.0`, from the npm registry through `pnpm dlx`.
- `bw_web_v2026.7.0.tar.gz`, from `https://github.com/dani-garcia/bw_web_builds/releases/tag/v2026.7.0`.
- The `vaultwarden/server:1.37.3` Docker image.

Do not download any of them without a clear yes. A row whose download was refused is recorded as NOT RUN, with the reason.

**Accounts.** Every account below is a local test account on `127.0.0.1`, made in Step 2.
- Their master passwords are test values you generate. Keep them in `E:\Personal\nodewarden\.dev.vars`, which is gitignored, as `BW_SHARER_PW`, `BW_RECIPIENT_PW` and `BW_THIRD_PW`.
- Never write them into the ledger or a commit.
- Never use a real password.

- [ ] **Step 1: Start NodeWarden locally, with invitation email off**

```bash
cd /e/Personal/nodewarden
git log -1 --format=%s
pnpm install --frozen-lockfile
pnpm run build
python -c "import secrets; print('JWT_SECRET=' + secrets.token_hex(32))" > .dev.vars
pnpm exec wrangler dev -c wrangler.toml --local --ip 127.0.0.1 --port 8787
```

Run the last command in the background (`run_in_background`). Then:

Run: `curl -s http://127.0.0.1:8787/api/config | python -c "import json,sys; c=json.load(sys.stdin); print(c['server'])"`
Expected: `{'name': 'NodeWarden', 'url': 'https://github.com/shuaiplus/NodeWarden'}`

Record ledger row L1.

- [ ] **Step 2: Make the three test accounts**

1. In a browser, open `http://127.0.0.1:8787`.
2. Register `sharer@example.test`. The first account on a fresh server is its administrator, so it needs no invite code.
3. In the admin page, make two invite codes.
4. Register `recipient@example.test` and `third@example.test` with them.
5. Sign in to each account once in that web vault, so each has its key pair.
6. Append the three generated master passwords to `.dev.vars`, as `BW_SHARER_PW=…`, `BW_RECIPIENT_PW=…` and `BW_THIRD_PW=…`.

- [ ] **Step 3: Share from Deskwarden, the way a user would**

1. Build and run Deskwarden from `E:\Personal\node-bitwarden\deskwarden` (`cargo run -j 2`). Sign in as `sharer@example.test`, with the server `http://127.0.0.1:8787` and direct sync (not `bw serve`).
2. Create a login named `compat-share-1`. Press **Share** and share it with `recipient@example.test`.
   - Expected: the toast says "Shared with recipient@example.test. It is in their vault now." The SHARING card says `recipient@example.test · Shared`.
   - With `INVITE_MAIL` off, NodeWarden accepted the recipient at invite, and the daemon confirmed straight away.
3. Share `compat-share-1` with `third@example.test`.
   - Expected: `Shared` again.
4. In the Share dialog, check that no roster-visibility warning is drawn, because the server names itself NodeWarden.

Record ledger rows L2 and L3.

- [ ] **Step 4: The official CLI, as the sharer**

```bash
cd /e/Personal/nodewarden
set -a; . ./.dev.vars; set +a
export BITWARDENCLI_APPDATA_DIR="$TEMP/bw-compat"
bw() { pnpm dlx @bitwarden/cli@2026.9.0 "$@"; }
bw config server http://127.0.0.1:8787
```

Expected: `Saved setting `config`.`

```bash
export BW_PASSWORD="$BW_SHARER_PW"
export BW_SESSION="$(bw login sharer@example.test --passwordenv BW_PASSWORD --raw)"
bw sync
```

Expected: `Syncing complete.`. Record L4.

```bash
bw list organizations | python -c "import json,sys; print([o['name'] for o in json.load(sys.stdin)])"
ORG="$(bw list organizations | python -c "import json,sys; print([o['id'] for o in json.load(sys.stdin) if o['name']=='Shared by sharer@example.test'][0])")"
bw list org-members --organizationid "$ORG" | python -c "import json,sys; [print(m['email'], m['status'], m['type']) for m in json.load(sys.stdin)]"
```

Expected:
- `['Shared by sharer@example.test']`;
- then three lines, in any order:
  - `sharer@example.test 2 0`
  - `recipient@example.test 2 2`
  - `third@example.test 2 2`

Record L5.

```bash
COL="$(bw list org-collections --organizationid "$ORG" | python -c "import json,sys; print([c['id'] for c in json.load(sys.stdin) if c['name']=='recipient@example.test'][0])")"
ITEM="$(bw get template item | python -c "import json,sys; t=json.load(sys.stdin); t['name']='compat-move-1'; t['login']={'username':'u','password':'p'}; print(json.dumps(t))" | bw encode | bw create item | python -c "import json,sys; print(json.load(sys.stdin)['id'])")"
printf '["%s"]' "$COL" | bw encode | xargs bw move "$ITEM" "$ORG" | python -c "import json,sys; i=json.load(sys.stdin); print(i['organizationId'], i['collectionIds'])"
```

Expected: `<ORG id> ['<COL id>']`. The CLI moved a personal item into the sharing org with `PUT /ciphers/{id}/share`. Record L6.

- [ ] **Step 5: The official CLI confirms a member**

1. Quit Deskwarden. Then make `third@example.test` an accepted-but-unconfirmed member again, directly in the local database:

   ```bash
   pnpm exec wrangler d1 execute nodewarden-db --local --command "UPDATE organization_members SET encrypted_org_key = NULL WHERE user_id = (SELECT id FROM users WHERE email = 'third@example.test')"
   ```

   This is a test-only edit of the local D1. NodeWarden pushes nothing for it, so no daemon confirms ahead of the CLI.
2. Run:

   ```bash
   bw sync
   OU="$(bw list org-members --organizationid "$ORG" | python -c "import json,sys; print([m['id'] for m in json.load(sys.stdin) if m['email']=='third@example.test'][0])")"
   bw list org-members --organizationid "$ORG" | python -c "import json,sys; print([m['status'] for m in json.load(sys.stdin) if m['email']=='third@example.test'])"
   bw confirm org-member "$OU" --organizationid "$ORG"; echo "exit $?"
   bw list org-members --organizationid "$ORG" | python -c "import json,sys; print([m['status'] for m in json.load(sys.stdin) if m['email']=='third@example.test'])"
   ```

   Expected:
   - `[1]`;
   - then `exit 0`. The CLI read `GET /organizations/{orgId}/users/{id}` (NW2), the key and the confirm;
   - then `[2]`.

Record L7.

- [ ] **Step 6: The official CLI, as the recipient**

```bash
bw logout
export BW_PASSWORD="$BW_RECIPIENT_PW"
export BW_SESSION="$(bw login recipient@example.test --passwordenv BW_PASSWORD --raw)"
bw sync
bw list items --search compat- | python -c "import json,sys; [print(i['name'], i['organizationId'] is not None, i.get('edit')) for i in json.load(sys.stdin)]"
```

Expected, two lines in any order:
- `compat-share-1 True False`
- `compat-move-1 True False`

They are view-only, through the recipient's read-only collection. Record L8.

- [ ] **Step 7: The official web vault, as the recipient**

1. With the user's yes, download `bw_web_v2026.7.0.tar.gz` into a new empty directory, `$TEMP/bw-web-dl`.
2. Check it against the release's `sha256sums.txt`, and extract it there.
3. Stop the `wrangler dev` from Step 1, and start it again serving that web vault in place of NodeWarden's own:

   ```bash
   pnpm exec wrangler dev -c wrangler.toml --local --ip 127.0.0.1 --port 8787 --assets "$TEMP/bw-web-dl/web-vault"
   ```

4. In a browser, open `http://127.0.0.1:8787` and sign in as `recipient@example.test`.
   - Expected: the vault shows the organisation "Shared by sharer@example.test", and the items `compat-share-1` and `compat-move-1` in it, view-only.

Record L9. Stop this `wrangler dev`, and start Step 1's again for Step 8.

- [ ] **Step 8: The hand checks of spec §7**

1. **Vaultwarden, mail off, in Docker** (with the user's yes for the image):

   ```bash
   docker run -d --name vw-compat -e SIGNUPS_ALLOWED=true -e DOMAIN=http://127.0.0.1:8090 -p 127.0.0.1:8090:80 vaultwarden/server:1.37.3
   curl -s http://127.0.0.1:8090/api/config | python -c "import json,sys; print(json.load(sys.stdin)['server']['name'])"
   ```

   Expected: `Vaultwarden`.
   - Register `vw-sharer@example.test` and `vw-recipient@example.test` in its web vault.
   - In Deskwarden, sign in as the sharer against `http://127.0.0.1:8090`. Share a record with the recipient: Vaultwarden creates the org from the client and accepts the recipient at invite. The card must reach `Shared`.
   - Sign in as the recipient in Deskwarden. The record is under Shared with me as `from vw-sharer@example.test · unverified · view-only`, and its detail shows the unverified tooltip.
   - Record L10. Then `docker rm -f vw-compat`.
2. **NodeWarden with `INVITE_MAIL` off.** Steps 3 and 6 above already did this. Record L11 as their result.
3. **A paste-link accept, with `INVITE_MAIL` on, through Cloudflare Email Service's test mode.** The spec's "test mode" is `wrangler dev`'s local simulation of the `send_email` binding. It sends nothing: it logs `send_email binding called with MessageBuilder:` with From, To and Subject, and saves the text body to a local file it names (`https://developers.cloudflare.com/email-service/local-development/sending/`). No onboarded domain is needed.
   - Stop the `wrangler dev` from Step 1. In `/e/Personal/nodewarden/wrangler.toml`, uncomment NW4's block locally, with `INVITE_MAIL_FROM = "vault@example.test"`. Do not commit it.
   - Start it again exactly as in Step 1 (`--local`).
   - In Deskwarden as `sharer@example.test`, share a new record `compat-mail-1` with `recipient@example.test`.
     - Expected: the card says `Waiting to accept`.
     - The `wrangler dev` output shows `Subject: You have been invited to a Bitwarden vault`, and a `Text: …\email-text\<id>.txt` path.
   - Open that file. It holds one link of the form `http://127.0.0.1:8787/#/accept-organization?organizationId=…&organizationUserId=…&email=recipient%40example.test&…&token=…`.
   - Sign Deskwarden in as `recipient@example.test`. Paste the link under Shared with me › Paste an invite link, and press Accept.
     - Expected: the toast says "Invitation accepted…".
     - Once the sharer's Deskwarden is signed in again and its daemon confirms, `compat-mail-1` arrives under Shared with me.
   - Record L12. The ledger's evidence is the Subject line and the toast, never the link or its token.
   - Then restore the file: `git -C /e/Personal/nodewarden checkout -- wrangler.toml`.
   - Optional, and only if the owner has a domain onboarded to Email Sending (`pnpm exec wrangler email sending list`): add `remote = true` under `[[send_email]]`, set `INVITE_MAIL_FROM` on that domain, and repeat the share to the owner's own mailbox, so that a real email arrives. Record the result in L12's evidence as well. Restore `wrangler.toml` again afterwards.

- [ ] **Step 9: Record the ledger, and commit it**

Fill in the Ledger table at the end of this file:
- **Result** is PASS, FAIL or NOT RUN.
- **Evidence** is the one-line output that decided it, or the reason it did not run.
- Each row's date.

Never write a password, a session key, a token or an invite link into it.

```bash
cd /e/Personal/node-bitwarden
git add docs/superpowers/plans/2026-10-08-official-path-sharing.md
git commit -F - <<'EOF'
The official CLI and web vault pass the sharing compatibility check against NodeWarden

The ledger at the end of the plan records each check of the release
condition (official-path spec §7) against a local wrangler dev
NodeWarden: the official bw CLI signs in, syncs, lists the sharer's org
members, moves an item into the sharing org and confirms a member, and
the official web vault shows the recipient the shared records. It also
records the hand checks: a share on Vaultwarden with mail off, one on
NodeWarden with invitation email off, and a pasted invite link with it
on.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

If any row reads FAIL, do not write "pass" in the subject. Write "The sharing compatibility check against NodeWarden, recorded", say which rows failed in the body, and stop. Report the failures. Do not fix them in this task.

---

## Ledger: the compatibility release check (filled in by DW5)

| Row | Check | Command or step | Result | Evidence | Date |
|---|---|---|---|---|---|
| L1 | `/config` names NodeWarden | DW5 Step 1 `curl …/api/config` | | | |
| L2 | Deskwarden shares to an existing account, which is `Shared` at once (`INVITE_MAIL` off) | DW5 Step 3.2 | | | |
| L3 | No roster-visibility warning on NodeWarden | DW5 Step 3.4 | | | |
| L4 | `bw` signs in and syncs | DW5 Step 4 `bw sync` | | | |
| L5 | `bw` lists the sharer's org members | DW5 Step 4 `bw list org-members` | | | |
| L6 | `bw` moves an item into the sharing org | DW5 Step 4 `bw move` | | | |
| L7 | `bw` confirms a member | DW5 Step 5 `bw confirm org-member` | | | |
| L8 | `bw` as the recipient sees the shared records, view-only | DW5 Step 6 | | | |
| L9 | The official web vault shows the recipient the org and records | DW5 Step 7 | | | |
| L10 | By hand: Vaultwarden, mail off, share and receive, "unverified" shown | DW5 Step 8.1 | | | |
| L11 | By hand: NodeWarden, `INVITE_MAIL` off | DW5 Step 8.2 | | | |
| L12 | By hand: paste-link accept, `INVITE_MAIL` on, Cloudflare Email Service (local simulation) | DW5 Step 8.3 | | | |

---

## Self-review

### Spec coverage

| Spec section | Requirement | Task |
|---|---|---|
| §1 | NodeWarden matches Bitwarden; Deskwarden takes one official path | NW1–NW4, DW1 |
| §1 | `mini-details` stays hidden from plain members on NodeWarden | No NodeWarden task touches it. DW4 reads its 404 as "roster hidden" |
| §2 | The dialect probe, `REASON_NOT_NODEWARDEN` and `slice_one` removed | DW1 Steps 3–9 |
| §2 | Roster readable by members, learned from `mini-details` | DW4 (`attest_received`, `note_roster_readable`) |
| §2 | Accepts on invite, learned from the roster read after the invite | DW1 (`ensure_member`, `share` runs `confirm_pass`) |
| §2 | Clients can create organisations, learned from `POST /organizations` | DW2 (`creates_orgs`, `needs_org`) |
| §2 | A required route answering 404 gives `server-too-old`, without "NodeWarden" in the sentence | DW1 (`route()`), DW2 (`SHARE_TOO_OLD_TOOLTIP`) |
| §2 | `bw serve` stays unavailable | DW1 (`REASON_BW_SERVE` kept) |
| §3.1 | Find the org by the roster; create it, or ask the user to | DW1 (`my_org_id`, `roster_says_mine`), DW2 (the create-it-yourself sentence) |
| §3.2 | `POST …/users/invite` with the collection; the user's own `ouId` | DW1 (`invite_user`, `own_ou_id`) |
| §3.3 | Confirm through the official routes; immediately when accepted at invite | DW1 |
| §3.4 | The card's states and words; the roster-visibility warning from `/config` | DW2 |
| §4.1 | The email's link, pasted into Deskwarden | NW4 (link), DW3 (`invite_link.rs`, paste UI, `accept_invite`) |
| §4.2 | Accept on invite; accept on registration | NW3 |
| §4.3 | "Shared by" from the roster; otherwise `· unverified` with the tooltip; grouping by organisation | DW4 |
| §5 | `INVITE_MAIL` off and on; Cloudflare Email Service | NW3, NW4 |
| §5 | Role change through `PUT …/users/{ouId}`; 47869c4 reverted; direct add stays `member` | NW2 |
| §5 | Bulk limit from Bitwarden's source | NW1 (fact 1: there is none, so a 64 KiB body bound) |
| §5 | `/config` `server` | NW1 |
| §5 | Direct add, `invitations/mine` and owner-email fields unchanged in NodeWarden | No NodeWarden task removes them. DW1, DW3 and DW4 stop Deskwarden reading them |
| §6 | Module changes | DW1–DW4, as the File Structure table maps them |
| §7 | NodeWarden tests: mail off, mail on (faked), role change, bulk, `/config` | NW3, NW4, NW2, NW1, NW1 |
| §7 | Deskwarden tests: each capability branch; invite links, malformed and wrong-email | DW1, DW2, DW4; DW3 (`invite_link` tests, `link_for_someone_else`) |
| §7 | Release condition: `bw` CLI and web vault against `wrangler dev`, with a ledger | DW5 Steps 1–7, 9 |
| §7 | By hand: Vaultwarden mail off; NodeWarden `INVITE_MAIL` off; paste-link with `INVITE_MAIL` on | DW5 Step 8 |
| §8 | No decline action; no "has an account" display | DW3 removes decline. Nothing shows whether an account exists. DW2's `Invited` state is the same for both |

No requirement is without a task.

Where the plan departs from the spec's words, Decisions 1–21 say so:
- the bulk bound (1);
- the new mail sender (2);
- the two new routes (5);
- the permanent delete (6);
- the purge left out of scope (7);
- the invite code kept (8);
- "the member with `type: 0`" read as "the one confirmed owner" (20);
- "test mode" (21).

### Placeholder scan

`TBD`, `TODO`, "similar to", "as above" and "fill in" were searched for in the plan. The only matches are the ledger's own instruction to fill it in (DW5 Step 9). The ledger's empty cells are deliberate: DW5 fills them with results that do not exist yet.

Every code step carries the code itself. Each `dw_cut.py` run names the file it reads its replacement from, and the step before writes that file in full.

### Type consistency

These names were checked across the tasks that produce and consume them:

| Name | Produced by | Consumed by |
|---|---|---|
| `Capabilities { loaded, too_old, roster_readable, accepts_on_invite, creates_orgs, server_name }` | DW1 | DW2 (`creates_orgs`, `server_name`), DW4 (`roster_readable`) |
| `Sharing::route`, `ensure_loaded`, `require_available` | DW1 | DW2, DW3 |
| `owned_and_named(org, account_email)` | DW1, calling `sharing_mark` with four arguments | DW4 changes the call to three arguments when `sharing_mark` loses `owner_email` |
| `SharingApi::invite_user`, `delete_cipher` | DW1 | DW1 |
| `SharingApi::server_name` | DW2 | DW2 (`ensure_loaded`) |
| `SharingApi::accept_invite` | DW3 | DW3 (`Sharing::accept_link`) |
| `PersonState::Invited`, `ShareResult::Invited`, `"result":"invited"` | DW2 (`dw_rename.py`) | DW2 UI; DW5's expected card text |
| `InviteLink`, `LinkError`, `invite_link::parse` | DW3 | DW3 (`accept_link`, the paste route) |
| `Held.waiting: Vec<(String, String)>` | before this plan | DW3 (`accept_link` pushes), DW4 (`unverified` alongside) |
| `WaitingEntry { organization_id, from, verified }` | DW4 adds `verified` | DW4 fixes DW3's one literal of it |
| `ReceivedEntry { item_id, from, organization_id, verified }` | DW4 | DW4 (`dw_literals.py` updates every older literal) |
| `SharingOrg { sharer, mine, verified }`, `sharing_mark(claimed, is_owner, account_email)` | DW4 | DW4 (`organizations.rs`, `sharing.rs`) |
| `Audience::SharedWithMe { from, verified }` | DW4 | DW4 (`detail_edit.rs`) |
| NodeWarden `invitationUrl` link format | NW4 | DW3's parser tests use the same field order and `.NET` encoding |
| NodeWarden `server.name = 'NodeWarden'` | NW1 | DW2's warning rule; DW5 L1 and L3 |

Test-count arithmetic, from 6135 at f2eecc8 (`grep -r --include=*.rs -o "#\[test\]" src | wc -l`):

| Task | Change | Running total |
|---|---|---|
| DW1 | −4 | 6131 |
| DW2 | +4 | 6135 |
| DW3 | +1 | 6136 |
| DW4 | +6 | 6142 |

The README's `6000+` badge holds throughout.

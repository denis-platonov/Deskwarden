# Personal Sharing (Deskwarden, NodeWarden) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Deskwarden user on a NodeWarden server can share one record with one person by email. The record moves into a hidden per-sharer organisation and the recipient sees it view-only. The sharer can stop sharing, forget people and re-invite. The daemon confirms accepted people on its own. A recipient can accept or decline a pending invitation from inside Deskwarden.

**Architecture:** The sharing engine is `rest/sharing.rs`. It lives inside the daemon's `RestBackend`, and `VaultBackend::sharing()` hands it out as an `Arc`. It speaks NodeWarden's dialect through typed requests in `rest/sharing_api.rs`, which go through one new door in `rest/api.rs`. The daemon exposes it on `/sharing` routes in `vault_api.rs`. It announces changes on the change feed as a new `sharing` kind plus the existing item kinds. The window reads and writes it only through `daemon_link::SharingClient`. The UI adds:
- a Share control and dialog;
- person rows on the SHARING card;
- share rows in Sent;
- received records and invitations in Shared with me;
- a view-only pane for received records.

Official Bitwarden, Vaultwarden and `bw serve` accounts are told sharing is unavailable and are offered a Send instead.

**Tech Stack:** Rust 2024, egui/eframe 0.35, ureq 2, serde_json, `rsa` 0.9 (with the `rand_core` 0.6 it re-exports), `sha2`, `hkdf`, `getrandom`. The crate's own `test_http::MockServer` is used for request-counting tests.

**Spec:** `docs/superpowers/specs/2026-09-27-personal-sharing-design.md`. Its "Owner's decisions after review" block overrides the rest. **Mock:** `docs/design/6-sharing-mock.html`, tabs 6a–6e.

## Global Constraints

These apply to every task and every step. A task's requirements include this section.

- **Scope is NodeWarden only.**
  - On official Bitwarden, on Vaultwarden, or on the `bw serve` backend, `GET /sharing` answers `available: false`.
  - There the Share control opens the Send composer instead ("offered a Send instead").
  - No official-dialect route (`/users/invite`, `/users/{ouId}/accept`, pasted invite links, adopting a licensed org) is built.
- **Line endings.**
  - Every `.rs` file in `deskwarden/` is CRLF. Make edits with the Edit tool, or with Python scripts that read and write bytes and keep `\r\n`.
  - **Never use `sed -i` on a `.rs` file.**
  - After every step that touches `.rs` files, run the checker below. It must print `0` for each file.
  - New `.rs` files are written with the Write tool and then normalised with `--fix`.
  - Save this checker once, as `$TEMP/dw_crlf.py`:
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
    Usage: `python "$TEMP/dw_crlf.py" src/rest/sharing.rs` (check), or `python "$TEMP/dw_crlf.py" --fix src/rest/sharing.rs` (new file).
- **Build directory.** Use a private target directory. The shared one serves stale test binaries. In Git Bash, from `E:\Personal\node-bitwarden\deskwarden`:
  `export CARGO_TARGET_DIR="$TEMP/dw-sharing-target"`
- **Verification per task.** The final step of every task runs all four of these. All must pass before the commit.
  1. `cargo test --lib -j 2` (the lib tests)
  2. `cargo test --bins -j 2`. `--lib` never runs `main.rs`'s tests.
  3. `cargo check --all-targets`, with **0 warnings**.
  4. `RUSTFLAGS="-D warnings" cargo test --no-run`. This catches duplicate `#[test]` attributes that `check` misses.
- **A pin that goes red is resolved, not loosened.**
  - Source pins are updated deliberately, in the same commit, when a counted thing changes:
    - `OPENS_NO_WINDOW` (array length in the type) in `src/foreground.rs`, for a new `lib.rs` module;
    - `THREAD_SPAWN_SITES` in `src/job_object.rs`, for a new `std::thread::spawn(`;
    - the frame-schedule census in `vault_window/mod.rs`, for any `request_repaint_after(` in that file's production half. This plan adds none there.
  - More pins:
    - `MODAL_SCRIM_AREAS` in `vault_window/item_list.rs`, for a new modal scrim;
    - the single `apply_send_action` call site and its pinned head and tail. This plan does not touch that call's text.
    - the body-sender census in `rest/api.rs`;
    - the root `README.md` test-count badge;
    - the non-Rust file list and the `build.rs` pin in `src/job_object.rs`.
    - the `job_object.rs` `Cargo.toml` pin. This plan changes no dependency, so `Cargo.toml` is not touched.
  - **Never** copy a new pin value out of a failure message. Recompute it with the independent script given in the step, and first check that the script reproduces the old value.
- **No new crates.**
  - RSA-OAEP encryption and RSA-2048 key generation use `rsa` 0.9, which is already a dependency. Its re-export `rsa::rand_core::OsRng` is live because `rsa`'s default `std` feature enables `rand_core/std`, which enables `getrandom`.
  - The big-number division for the fingerprint phrase is twenty lines here.
  - `Cargo.toml` and `Cargo.lock` stay byte-identical.
- **No release builds, no version bumps, no CHANGELOG edits.**
- **Comments and copy.**
  - Comments follow the codebase's dense, explanatory voice: say *why*, name the alternative that was rejected, and point at the pinning test.
  - User-facing text is plain sentence case and matches the mock's words, except where the mock describes behaviour NodeWarden does not have. Those deviations are listed in "Decisions this plan takes" below.
- **House rules the crate enforces by test.**
  - No `#[cfg(not(test))]`.
  - Below the first `#[cfg(test)]` of a file there are only `#[cfg(test)] mod … { }` blocks.
  - A type whose body mentions `Zeroizing` must not `#[derive(Debug)]`.
  - `VaultBackend` has no defaulted methods, so every impl is written out.
  - Every thread spawn is `std::thread::spawn(` spelled in full, or a `std::thread::Builder`.
- **Commits.**
  - One commit per task, on `main` (the repo's working branch).
  - The subject is one plain sentence in the repo's style (for example "A record can be shared with a person on NodeWarden"). The body says what changed and why.
  - The message ends with:
    ```
    Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
    ```
- **NodeWarden server shapes.**
  - Take them from spec §10. The server work is planned separately.
  - Where a route's answer shape is not in §10, this plan uses the shape NodeWarden already answers today, read from `E:\Personal\nodewarden` on `feature/missing-features`:
    - `/members/public-key`
    - `/members/invite`
    - `/invitations/*`
    - `/api/admin/invites`
    - `/collections`

## Decisions this plan takes (where the spec is silent, contradicts itself, or meets the code)

These appear again in the final report. Each is implemented as stated.

1. **No unavailable reason for official servers.** §11.3's `reason` enum has none for "an official server, sharing not built yet". This plan adds `"not-nodewarden"`.
2. **Minting an invite code needs the master password.** `POST /api/admin/invites` requires `masterPasswordHash` (`nodewarden/src/handlers/admin.ts:278`), and the daemon does not hold the password.
   - The dialog asks for it.
   - The window sends it to the daemon in a POST body, and the daemon derives the hash with the session's master key.
   - The hash is sent; the password is zeroised.
3. **Stopping the last share loses attachments.** A record moved back is recreated (create + delete), and attachments belong to the old cipher id, so they would be lost. The owner's decision refuses only "attachments without a per-item key". **This plan refuses every record with attachments** in v1.
4. **The mock's invitation copy is not NodeWarden's behaviour.**
   - "Bitwarden emailed the invitation" and "Bitwarden has emailed them an invitation" are false on NodeWarden, which sends no email.
   - The no-account band says instead that they need an invite code from the server's administrator. It offers **Make an invite code** when the sharer is the admin (owner decision Q3 (b)).
5. **Moving and starring received records.**
   - The mock keeps "Move to folder" (and the star) on a received record.
   - NodeWarden slice 1 refuses every write, including `partial`, on a read-only collection (§10.3). Both would fail.
   - They are hidden on received records in this plan. Spec gap: it needs a per-user folder route on NodeWarden before they can come back.
6. **Where the per-account state is kept.**
   - `sharing_org_id` is kept in `sharing_state.json`, not `settings.json` (§5.1), next to the pins it belongs with.
   - The file keys every account's state by user id, `{"accounts": {"<userId>": {...}}}`, so two accounts on one PC do not share pins.
7. **Several matching orgs.**
   - The profile carries no `creationDate`, so "the oldest wins" (§5.1) cannot be computed.
   - The cached id wins. Otherwise the first in profile order wins, which is NodeWarden's creation order.
8. **Fingerprint vector.**
   - §9.6 asks for a vector captured by hand. Bitwarden's own SDK publishes one in `crates/bitwarden-crypto/src/fingerprint.rs`, `test_fingerprint`: user id `a09726a0-9590-49d1-a5f5-afe300b6a515`, a 294-byte SPKI key, and the phrase `turban-deftly-anime-chatroom-unselfish`. This plan pins that vector.
   - The SDK keys HKDF with the raw SPKI bytes. HMAC hashes a key longer than its block, so that equals §9.6's `PRK = SHA-256(spki)`.
9. **RSA encryption check.** §14.2 asks for `encrypt_rsa` output "decrypted by OpenSSL, a recorded vector". OAEP is randomised, so no ciphertext can be recorded. The check is:
   - `encrypt_rsa`'s output opens under `decrypt_rsa`;
   - `decrypt_rsa` is itself pinned to OpenSSL by `rsa_oaep_sha1_opens_a_ciphertext_openssl_produced`.
10. **A roster fallback that does not cost extra requests.**
    - The roster route tries `/users?includeCollections=true`, then `/members` on 404, 405 or 501. 501 is what this crate's mock server answers for an unmocked route, and what a server without the route may answer.
    - Rosters of *other people's* sharing orgs are not fetched at all, because NodeWarden refuses them to members. So a recipient pays nothing extra.
11. **A person who is a member but lost access to their collection** (restored elsewhere, or merged) is **removed and invited again** on the next share. NodeWarden's `/members/{userId}` update route is not in §7.3.
12. **Where records sit in the Sent list.** Share rows come first, then Send rows. A share has no server timestamp to interleave by. The local "since" date is the most recent `sent_history` entry for that record and person.
13. **The view-only kebab.** Mock 6d shows "Refresh icon", but the read pane's kebab has no such entry today. The view-only kebab holds only the footer sentence.
14. **The daemon owns local item-id files.** A move back re-keys `sent_history.json`, `fill-stats.json` and the in-memory fill recall inside the daemon, because it is the process that owns them.
    - The window re-reads `sent_history.json` when it hears `Change::Sharing`.
    - Nothing else re-keys: `fill_recall` is in memory and lasts a few minutes, and `fill_stats` is read per call.

15. **A received record opens in the vault.** Mock 6d draws the read pane beside Shared with me, but that screen's right-hand side is the Sends detail, which cannot host the read pane. Clicking a received record opens it in the vault list (`SendUiAction::OpenRecord`). There the pane is view-only.
16. **Duplicate person collections are not merged.** §5.2 merges two collections that decrypt to one email; this build uses the first in server order. The merge needs a per-collection item listing that §7.3 does not name.

## File Structure

| File | New / changed | Responsibility |
|---|---|---|
| `deskwarden/src/rest/crypto.rs` | changed | `encrypt_rsa`, `generate_symmetric_key`, `wrap_key_to_public`, `wrap_key_under`, `generate_org_key_pair`, `public_key_of`, `public_key_sha256_hex`, `fingerprint_phrase` |
| `deskwarden/src/rest/eff_wordlist.rs` | new | The EFF long list, loaded from beside the executable, counted and hashed (`password_gen`'s pattern) |
| `deskwarden/assets/eff_large_wordlist.txt` | new | EFF's published list, verbatim |
| `deskwarden/build.rs`, `installer/deskwarden.iss` | changed | Copy and ship the EFF list beside the executable |
| `deskwarden/src/rest/sync.rs` | changed | `Profile` gains `id`, `email`, `role`, `organizations_new`. `Organization` gains `kind`, `role`, `status`. `VaultKeys::with_organization`, `private_key_der` made `pub(crate)` |
| `deskwarden/src/rest/api.rs` | changed | `send_sharing` (the one sharing door), `sharing_answer`, roster fallback, `is_url_path_safe` made `pub(crate)`, census update |
| `deskwarden/src/rest/sharing_api.rs` | new | `SharingRequest` (only constructed here) and one typed function per NodeWarden route. No policy. |
| `deskwarden/src/rest/organizations.rs` | changed | `MemberStatus`, member rows from `/users` and `/members`, sharing-org recognition (`sharing_org_name`, `sharer_of`), `visible_organisations`, `waiting`, `Audience::SharedWithPeople` and `SharedWithMe`, wire body |
| `deskwarden/src/sharing_state.rs` | new | `sharing_state.json`: per-account `sharing_org_id`, pins, moves, unfinished; atomic write |
| `deskwarden/src/rest/sharing.rs` | new | The engine (`Sharing`): probe, held state, view (`SharingView`), share, stop, move back, forget, reinvite, confirm pass, fingerprint, invite code, accept, decline, leave |
| `deskwarden/src/rest/backend.rs` | changed | Holds `Arc<Sharing>`, shares its key cache with it, hands it the directory after each whole read |
| `deskwarden/src/vault_backend.rs`, `daemon_link.rs` (impl) | changed | `fn sharing(&self) -> Option<Arc<Sharing>>` on every impl |
| `deskwarden/src/vault_cache.rs` | changed | `apply_written_elsewhere` |
| `deskwarden/src/change_feed.rs` | changed | `Change::Sharing`, and an unknown kind is skipped |
| `deskwarden/src/rest/notifications.rs` | changed | `HubListener::sharing_changed` |
| `deskwarden/src/vault_api.rs` | changed | The `/sharing` routes, `sharing_pass` |
| `deskwarden/src/vault_host.rs` | changed | The confirm-pass triggers and the 10-minute sharing timer, in the existing hub worker thread |
| `deskwarden/src/daemon_link.rs` | changed | `SharingClient`, `sharing_client()` |
| `deskwarden/src/sent_history.rs` | changed | `SentKind`, per-kind counts on `Person`, `rekey_item` |
| `deskwarden/src/fill_stats.rs`, `fill_recall.rs` | changed | `rekey` |
| `deskwarden/src/vault_bridge.rs` | changed | `may_edit` |
| `deskwarden/src/theme.rs` | changed | `share_record_button`, `paint_person_mark`, `share_pill`, `share_pill_width`, `SharePillKind` |
| `deskwarden/src/vault_window/share_ui.rs` | new | Share dialog, sharing worker thread, `SharingPane` state, person lines, Sent share rows and detail, received rows, invitation rows, toast |
| `deskwarden/src/vault_window/detail.rs` | changed | Share control, person rows on SHARING card, view-only header and kebab |
| `deskwarden/src/vault_window/item_list.rs` | changed | Person-marked `shared` pill, `view-only` chip, `RowPeople`, scrim list |
| `deskwarden/src/vault_window/sidebar.rs` | changed | Organisations rail uses `visible_organisations`, Sent and Shared-with-me counts include records, pending dot |
| `deskwarden/src/vault_window/send_ui.rs` | changed | Hands the list and detail to `share_ui` for share rows and received records (no new public items) |
| `deskwarden/src/vault_window/mod.rs` | changed | Frame wiring: fetch, drain, dialog, actions, gating |
| `deskwarden/src/vault_window/detail_edit.rs` | changed | `owner_line` arms for the two new audiences |
| `deskwarden/src/lib.rs`, `src/foreground.rs`, `src/job_object.rs`, root `README.md` | changed | Module list, pins, badge |

## Task list (groups leave the app working after each)

| Group | Tasks |
|---|---|
| 1. Crypto primitives | 1, 2 |
| 2. REST sharing client (NodeWarden dialect) | 3, 4, 5 |
| 3. Daemon: engine skeleton, `GET /sharing`, feed kind, client | 6, 7, 8 |
| 4. Owner flows (each adds its own route) | 9, 10, 11 |
| 5. Auto-confirm and pending invitations | 12, 13 |
| 6. UI | 14, 15, 16, 17, 18, 19 |
| 7. Recipient accept and decline, leave | 20 |
| Close | 21 |

---

### Task 1: RSA encryption, key generation and key wrapping

**Files:**
- Modify: `deskwarden/src/rest/crypto.rs`
  - imports near lines 87-95;
  - `SymmetricKey` impl (lines 222-270);
  - new functions after `decrypt_rsa` (ends line 932);
  - the module doc paragraph at lines 77-83;
  - tests appended inside `pub mod tests` (from line 935).

**Interfaces:**
- Consumes: nothing new.
- Produces (all in `crate::rest::crypto`):
  - `pub const ORG_KEY_BITS: usize = 2048;`
  - `pub fn encrypt_rsa(public_key_spki_der: &[u8], plain: &[u8]) -> Result<EncString, CryptoError>` (type 4)
  - `pub fn generate_symmetric_key() -> Result<SymmetricKey, CryptoError>`
  - `pub fn wrap_key_to_public(key: &SymmetricKey, public_key_spki_der: &[u8]) -> Result<EncString, CryptoError>`
  - `pub fn wrap_key_under(key: &SymmetricKey, wrapping: &SymmetricKey) -> Result<EncString, CryptoError>` (type 2)
  - `pub fn unwrap_key_under(wrapped: &EncString, wrapping: &SymmetricKey) -> Result<SymmetricKey, CryptoError>` (alias of `unwrap_user_key` with the arguments named for this use)
  - `pub fn generate_org_key_pair(org: &SymmetricKey) -> Result<(String, EncString), CryptoError>`, which returns (SPKI base64, PKCS#8 under `org`)
  - `pub fn public_key_of(private_key_pkcs8_der: &[u8]) -> Result<Vec<u8>, CryptoError>`
  - `pub fn public_key_sha256_hex(public_key_spki_der: &[u8]) -> String`
  - `pub fn spki_from_base64(text: &str) -> Result<Vec<u8>, CryptoError>`
  - test helper `pub fn fixture_public_spki() -> Vec<u8>` in `crypto::tests`

- [ ] **Step 1: Write the failing tests**

Append inside `pub mod tests` in `src/rest/crypto.rs`, just before its closing `}`:

```rust
    // ---- asymmetric encryption and key generation (personal sharing) ------

    /// The OpenSSL fixture's PUBLIC half, as SPKI DER, derived by this module's
    /// own [`public_key_of`] -- which the test below checks against the private
    /// half by round-tripping through [`decrypt_rsa`], the OpenSSL-pinned side.
    pub fn fixture_public_spki() -> Vec<u8> {
        public_key_of(&hex(ORG_KEY_PRIVATE_PKCS8_DER)).expect("the fixture's public half")
    }

    /// **RSA-OAEP-SHA1 out, checked by the side that is pinned to OpenSSL.**
    ///
    /// OAEP is randomised, so there is no ciphertext to record. What makes this
    /// external rather than circular is the other direction:
    /// `rsa_oaep_sha1_opens_a_ciphertext_openssl_produced` pins [`decrypt_rsa`]
    /// to OpenSSL's own output, so a ciphertext it opens was made with OAEP,
    /// SHA-1 and MGF1-SHA-1 -- which is what every Bitwarden client unwraps an
    /// organisation key with.
    #[test]
    fn an_rsa_encryption_opens_under_the_openssl_pinned_decryption() {
        let plain: Vec<u8> = (0..64u8).collect();
        let sealed = encrypt_rsa(&fixture_public_spki(), &plain).expect("an encryption");
        assert!(sealed.to_string().starts_with("4."), "not a type-4 string: {sealed:?}");
        let opened = decrypt_rsa(&hex(ORG_KEY_PRIVATE_PKCS8_DER), &sealed).expect("it opens");
        assert_eq!(&*opened, plain.as_slice());
        // Randomised: the same plaintext twice is two ciphertexts.
        let again = encrypt_rsa(&fixture_public_spki(), &plain).expect("a second encryption");
        assert_ne!(sealed.to_string(), again.to_string(), "OAEP produced the same bytes twice");
    }

    #[test]
    fn a_public_key_that_is_not_spki_is_an_error_rather_than_a_panic() {
        assert_eq!(encrypt_rsa(b"not a key", b"x"), Err(CryptoError::Rsa));
        assert_eq!(spki_from_base64("not base64!"), Err(CryptoError::Malformed("public key")));
    }

    /// An organisation key, wrapped to the owner's own public key, is the
    /// profile's `organizations[].key` -- so it must come back out through the
    /// exact path `VaultKeys::unwrap_from` uses, [`unwrap_org_key`].
    #[test]
    fn a_key_wrapped_to_a_public_key_unwraps_as_an_organisation_key() {
        let org = generate_symmetric_key().expect("64 random bytes");
        let wrapped = wrap_key_to_public(&org, &fixture_public_spki()).expect("a wrap");
        let back = unwrap_org_key(&hex(ORG_KEY_PRIVATE_PKCS8_DER), &wrapped).expect("an unwrap");
        assert_eq!(back, org);
        // Control: two generated keys differ, so the equality above is not
        // two all-zero keys agreeing.
        assert_ne!(generate_symmetric_key().expect("a second key"), org);
    }

    /// A per-cipher key re-wrapped from one symmetric key to another -- the
    /// cheap half of moving a record into or out of an organisation.
    #[test]
    fn a_key_rewrapped_under_another_key_unwraps_under_it_and_not_the_first() {
        let cipher_key = generate_symmetric_key().expect("a cipher key");
        let user = key_from_64(&[3u8; 64]);
        let org = key_from_64(&[4u8; 64]);
        let under_user = wrap_key_under(&cipher_key, &user).expect("wrap under the user key");
        let opened = unwrap_key_under(&under_user, &user).expect("unwrap");
        let under_org = wrap_key_under(&opened, &org).expect("re-wrap under the org key");
        assert_eq!(unwrap_key_under(&under_org, &org).expect("unwrap"), cipher_key);
        assert_eq!(unwrap_key_under(&under_org, &user), Err(CryptoError::MacMismatch));
    }

    /// **The organisation's own key pair**, as the create body carries it.
    ///
    /// The one test in the crate that generates RSA-2048, which is seconds in
    /// a debug build; the engine's tests hand it a fixture pair instead
    /// (`rest::sharing`'s `SharingSetup::make_key_pair`).
    #[test]
    fn a_generated_org_key_pair_is_spki_and_a_pkcs8_that_opens_what_the_public_half_wraps() {
        let org = generate_symmetric_key().expect("an org key");
        let (public_b64, private_wrapped) = generate_org_key_pair(&org).expect("a key pair");
        let public = spki_from_base64(&public_b64).expect("base64 SPKI");
        let private = decrypt(&org, &private_wrapped).expect("the private key opens under org");
        assert_eq!(public_key_of(&private).expect("its public half"), public);
        let secret = generate_symmetric_key().expect("something to wrap");
        let wrapped = wrap_key_to_public(&secret, &public).expect("wrap to the new public key");
        assert_eq!(unwrap_org_key(&private, &wrapped).expect("unwrap"), secret);
    }

    #[test]
    fn a_public_key_hash_is_lowercase_hex_of_sha256() {
        // SHA-256("abc"), FIPS 180-2 appendix B.1.
        assert_eq!(
            public_key_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib -j 2 rest::crypto::tests::a_`
Expected: a compile error, `cannot find function 'encrypt_rsa' in this scope` (and the same for the other new names).

- [ ] **Step 3: Write the implementation**

In `src/rest/crypto.rs`, replace the two `rsa` import lines

```rust
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Oaep, RsaPrivateKey};
```

with

```rust
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use rsa::rand_core::OsRng;
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
```

In `impl SymmetricKey`, directly after `from_okm`, add:

```rust
    /// The 64 bytes -- encryption key then MAC key -- **for wrapping this key
    /// and for nothing else**. Private to this file: the only callers are
    /// [`wrap_key_to_public`] and [`wrap_key_under`], which hand the bytes
    /// straight to a cipher, and a bytes-out accessor anywhere else would be
    /// the widening the test module's own comment refuses.
    fn to_64(&self) -> Zeroizing<[u8; SYMMETRIC_KEY_LEN]> {
        let mut out = Zeroizing::new([0u8; SYMMETRIC_KEY_LEN]);
        out[..KEY_LEN].copy_from_slice(&*self.enc);
        out[KEY_LEN..].copy_from_slice(&*self.mac);
        out
    }
```

Directly after `decrypt_rsa` (before `#[cfg(test)]`), add:

```rust
// ---- asymmetric encryption and key generation --------------------------------
//
// **Personal sharing is what put these here.** Until it, this module held no
// public key and said so ("There is no *asymmetric* encryption"). Sharing a
// record with a person needs three things a decrypt-only module cannot do:
// wrap an organisation key to a member's public key (confirming them), make an
// organisation key and its RSA pair (the first share), and re-wrap a
// per-cipher key between the user key and an organisation's (moving a record
// in and out). See `docs/superpowers/specs/2026-09-27-personal-sharing-design.md`
// section 9.

/// RSA modulus size for an organisation's key pair: what every Bitwarden
/// client generates, and what type 4 (`Rsa2048_OaepSha1_B64`) is named for.
pub const ORG_KEY_BITS: usize = 2048;

/// The OS generator the `rsa` crate draws from, **asked first**.
///
/// `rand_core`'s `OsRng` panics if the OS refuses randomness -- the trait it
/// implements has no error channel. This module's rule is that a failed RNG
/// is [`CryptoError::Rng`] and never a crash (see that variant), so one byte
/// is asked of `getrandom` directly before `rsa` is handed the generator. It
/// is the same `getrandom` underneath, so a refusal here is the refusal
/// `OsRng` would have met a microsecond later.
fn os_rng_ready() -> Result<OsRng, CryptoError> {
    let mut probe = [0u8; 1];
    getrandom::getrandom(&mut probe).map_err(|_| CryptoError::Rng)?;
    Ok(OsRng)
}

/// RSA-OAEP-SHA1 encryption to a public key given as SPKI DER, as a type-4
/// [`EncString`] -- the inverse of [`decrypt_rsa`], and what an organisation
/// key wrapped for a member is.
pub fn encrypt_rsa(public_key_spki_der: &[u8], plain: &[u8]) -> Result<EncString, CryptoError> {
    let key =
        RsaPublicKey::from_public_key_der(public_key_spki_der).map_err(|_| CryptoError::Rsa)?;
    let mut rng = os_rng_ready()?;
    let ct = key.encrypt(&mut rng, Oaep::new::<sha1::Sha1>(), plain).map_err(|_| CryptoError::Rsa)?;
    Ok(EncString::Rsa2048OaepSha1B64 { ct })
}

/// A fresh 64-byte symmetric key from the OS generator: an organisation key.
pub fn generate_symmetric_key() -> Result<SymmetricKey, CryptoError> {
    let mut bytes = Zeroizing::new([0u8; SYMMETRIC_KEY_LEN]);
    getrandom::getrandom(&mut *bytes).map_err(|_| CryptoError::Rng)?;
    SymmetricKey::from_64(&*bytes)
}

/// `key` wrapped to a public key -- the organisation key a member is
/// confirmed with, and the one the owner's own profile carries.
pub fn wrap_key_to_public(
    key: &SymmetricKey,
    public_key_spki_der: &[u8],
) -> Result<EncString, CryptoError> {
    encrypt_rsa(public_key_spki_der, &*key.to_64())
}

/// `key` wrapped under another symmetric key (type 2) -- a per-cipher key
/// moving between the user key and an organisation key.
pub fn wrap_key_under(key: &SymmetricKey, wrapping: &SymmetricKey) -> Result<EncString, CryptoError> {
    encrypt(wrapping, &*key.to_64())
}

/// The inverse of [`wrap_key_under`]. [`unwrap_user_key`] is exactly this
/// operation under its commonest caller's name; this is the same body under the
/// name the sharing engine reads as what it does.
pub fn unwrap_key_under(
    wrapped: &EncString,
    wrapping: &SymmetricKey,
) -> Result<SymmetricKey, CryptoError> {
    unwrap_user_key(wrapping, wrapped)
}

/// **An organisation's RSA key pair**, as the create body carries it:
/// `keys.publicKey` (SPKI DER, base64) and `keys.encryptedPrivateKey`
/// (PKCS#8 DER under the organisation key, type 2). Official clients make one
/// for every organisation; NodeWarden and Vaultwarden store it.
pub fn generate_org_key_pair(org: &SymmetricKey) -> Result<(String, EncString), CryptoError> {
    let mut rng = os_rng_ready()?;
    let private = RsaPrivateKey::new(&mut rng, ORG_KEY_BITS).map_err(|_| CryptoError::Rsa)?;
    let public = private.to_public_key().to_public_key_der().map_err(|_| CryptoError::Rsa)?;
    // `SecretDocument` zeroizes itself on drop, so the PKCS#8 bytes live only
    // as long as the one encryption below.
    let pkcs8 = private.to_pkcs8_der().map_err(|_| CryptoError::Rsa)?;
    let wrapped = encrypt(org, pkcs8.as_bytes())?;
    let mut public_b64 = String::new();
    base64_into(&mut public_b64, public.as_bytes());
    Ok((public_b64, wrapped))
}

/// The SPKI DER public half of a PKCS#8 private key -- the account's own
/// public key, read from the private key the profile already carries rather
/// than asked of the server, so wrapping an organisation key to "myself" does
/// not trust the server about who "myself" is.
pub fn public_key_of(private_key_pkcs8_der: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(private_key_pkcs8_der).map_err(|_| CryptoError::Rsa)?;
    let public = key.to_public_key().to_public_key_der().map_err(|_| CryptoError::Rsa)?;
    Ok(public.as_bytes().to_vec())
}

/// A public key's pin: lowercase hex of SHA-256 over its SPKI DER. What
/// `sharing_state.json` records the first time a person is confirmed.
#[must_use]
pub fn public_key_sha256_hex(public_key_spki_der: &[u8]) -> String {
    Sha256::digest(public_key_spki_der).iter().map(|b| format!("{b:02x}")).collect()
}

/// A server's base64 public key, decoded. Its own error so a caller can tell
/// "the server sent something that is not a key" from a failed encryption.
pub fn spki_from_base64(text: &str) -> Result<Vec<u8>, CryptoError> {
    base64_from(text.trim()).ok_or(CryptoError::Malformed("public key"))
}
```

Replace the last sentence of the module doc (lines 81-83), which reads
`//! `Display`. There is no *asymmetric* encryption: type 4 needs a public key`
`//! this module does not hold.`
with:

```rust
//! `Display`. **Asymmetric encryption arrived with personal sharing**:
//! [`encrypt_rsa`] wraps to a member's public key, and the key generators
//! below it make an organisation's keys. They are in their own section at the
//! end of the production half, with their reasons.
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 rest::crypto`
Expected: PASS, including the five new tests. `a_generated_org_key_pair_…` takes several seconds in a debug build.

- [ ] **Step 5: Check line endings and run the whole verification**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/crypto.rs
cargo test --lib -j 2 && cargo test --bins -j 2 && cargo check --all-targets && RUSTFLAGS="-D warnings" cargo test --no-run
```
Expected: the checker prints `src/rest/crypto.rs 0`. All tests pass, and `check` prints no warning.

If `debug_leak_guard` or the crypto module's `the_composition_is_still_not_fully_pinned_and_here_is_what_is_left` fails:
- read its message;
- the latter asserts only two phrases of the module doc, which this task keeps.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/rest/crypto.rs
git commit -m "The crypto module can wrap a key to a public key and make an organisation's keys

Personal sharing needs RSA-OAEP-SHA1 encryption to a member's public key,
a fresh 64-byte organisation key, an RSA-2048 org key pair and per-cipher
key re-wrapping. All of it is built on the rsa crate already in the tree:
its re-exported rand_core OsRng is live through rsa's default std feature,
so no dependency moved. A failed OS generator is still CryptoError::Rng,
asked of getrandom first, never OsRng's panic.

Encryption is checked through decrypt_rsa, which is pinned to an OpenSSL
ciphertext; OAEP is randomised, so there is no ciphertext to record.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The EFF long word list and the fingerprint phrase

**Files:**
- Create: `deskwarden/assets/eff_large_wordlist.txt` (EFF's file, verbatim)
- Create: `deskwarden/src/rest/eff_wordlist.rs`
- Modify:
  - `deskwarden/src/rest/mod.rs` (module list, lines 132-159);
  - `deskwarden/src/rest/crypto.rs` (`fingerprint_phrase`, plus a test);
  - `deskwarden/build.rs` (copy the second list);
  - `deskwarden/installer/deskwarden.iss` (ship it);
  - `deskwarden/src/job_object.rs`: the non-Rust file list near line 3349, and the `build.rs` pin at line 2081.

**Interfaces:**
- Produces:
  - `crate::rest::eff_wordlist::{EFF_WORDLIST_FILE, EFF_WORDS, EffLongList, EffListError, eff_wordlist_paths, load, load_from, verify}`
  - `EffLongList::word(&self, index: usize) -> &str`
  - `crate::rest::crypto::fingerprint_phrase(material: &str, public_key_spki_der: &[u8], words: &EffLongList) -> Result<String, CryptoError>`
  - `crate::rest::crypto::FINGERPRINT_WORDS: usize = 5`
  - test constants `crate::rest::crypto::tests::{SDK_FINGERPRINT_USER, SDK_FINGERPRINT_KEY, SDK_FINGERPRINT_PHRASE}` (Bitwarden's SDK vector; Task 12 reuses them)

- [ ] **Step 1: Add the list file**

Run (from `E:\Personal\node-bitwarden\deskwarden`):

```bash
curl -fsSL -o assets/eff_large_wordlist.txt https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt
python - <<'EOF'
lines = [l for l in open('assets/eff_large_wordlist.txt', encoding='utf-8').read().splitlines() if l.strip()]
words = [l.split()[-1] for l in lines]
assert len(words) == 7776, len(words)
assert len(set(words)) == 7776
assert words[0] == 'abacus' and words[-1] == 'zoom', (words[0], words[-1])
import hashlib
digest = hashlib.sha256('\n'.join(words).encode()).digest()
print(', '.join('0x%02x' % b for b in digest))
EOF
```

Expected: the script prints 32 comma-separated hex bytes and no assertion fails. Keep that line: it is `EFF_WORDLIST_SHA256` in Step 3. The file stays exactly as EFF publishes it. Its dice-number column is read and ignored by `verify`.

- [ ] **Step 2: Write the failing tests**

Create `deskwarden/src/rest/eff_wordlist.rs` containing only the test module for now. Step 3 puts the production half above it.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/eff_large_wordlist.txt"))
            .expect("the committed EFF list")
    }

    #[test]
    fn the_committed_list_is_the_eff_long_list() {
        let list = verify(&committed()).expect("the committed list verifies");
        assert_eq!(list.len(), EFF_WORDS);
        assert_eq!(list.word(0), "abacus");
        assert_eq!(list.word(EFF_WORDS - 1), "zoom");
    }

    #[test]
    fn a_short_or_altered_list_is_refused() {
        let text = committed();
        let short: String = text.lines().take(7000).map(|l| format!("{l}\n")).collect();
        assert_eq!(verify(&short).err(), Some(EffListError::Unusable));
        let altered = text.replacen("abacus", "abacas", 1);
        assert_eq!(verify(&altered).err(), Some(EffListError::Unusable));
        let crlf = text.replace('\n', "\r\n");
        assert!(verify(&crlf).is_ok(), "the list must verify under either line ending");
    }

    #[test]
    fn a_missing_list_is_missing_and_not_unusable() {
        let nowhere = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/no-such-dir/x.txt"));
        assert_eq!(load_from(&[nowhere]).err(), Some(EffListError::Missing));
    }

    #[test]
    fn the_installer_ships_the_eff_list_beside_the_executable() {
        let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/installer/deskwarden.iss"))
            .expect("the installer script");
        let line = script
            .lines()
            .map(str::trim)
            .find(|l| !l.starts_with(';') && l.contains(EFF_WORDLIST_FILE))
            .expect("the installer does not ship the EFF list");
        assert!(line.starts_with("Source:"), "`{line}` is not a [Files] entry");
        assert!(line.contains(r"..\assets\eff_large_wordlist.txt"), "`{line}`");
        assert!(line.contains(r#"DestDir: "{app}""#), "`{line}`");
    }

    #[test]
    fn the_build_script_copies_the_eff_list_beside_the_executable() {
        let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).expect("build.rs");
        assert!(build.contains("\"eff_large_wordlist.txt\""), "build.rs no longer copies the EFF list");
    }
}
```

Add `pub mod eff_wordlist;` to `src/rest/mod.rs` with this doc line, directly after `pub mod crypto;`:

```rust
/// The EFF long word list the fingerprint phrase is drawn from: a file beside
/// the executable, counted and hashed before use, as `password_gen`'s is.
pub mod eff_wordlist;
```

Append to `pub mod tests` in `src/rest/crypto.rs`:

```rust
    /// Bitwarden's SDK fingerprint vector: the user id, and that user's
    /// 294-byte SPKI public key. `pub` so the sharing engine's fingerprint
    /// test can serve the same key from a mock server.
    pub const SDK_FINGERPRINT_USER: &str = "a09726a0-9590-49d1-a5f5-afe300b6a515";
    pub const SDK_FINGERPRINT_KEY: &[u8] = &[
        48, 130, 1, 34, 48, 13, 6, 9, 42, 134, 72, 134, 247, 13, 1, 1, 1, 5, 0, 3, 130, 1, 15,
        0, 48, 130, 1, 10, 2, 130, 1, 1, 0, 187, 38, 44, 241, 110, 205, 89, 253, 25, 191, 126,
        84, 121, 202, 61, 223, 189, 244, 118, 212, 74, 139, 130, 97, 115, 164, 167, 106, 191,
        188, 233, 218, 196, 250, 187, 146, 125, 160, 150, 49, 198, 224, 176, 10, 0, 143, 99,
        230, 232, 160, 51, 104, 154, 211, 33, 80, 170, 4, 68, 80, 219, 115, 167, 114, 156, 227,
        125, 193, 128, 123, 39, 254, 191, 124, 63, 129, 44, 63, 18, 56, 161, 48, 158, 0, 27,
        146, 2, 99, 136, 75, 21, 135, 6, 118, 12, 26, 251, 184, 172, 249, 53, 78, 210, 46, 143,
        17, 104, 202, 65, 173, 229, 219, 233, 144, 163, 101, 216, 238, 152, 54, 158, 1, 195,
        50, 203, 21, 226, 12, 82, 170, 175, 170, 160, 21, 247, 248, 80, 97, 123, 0, 152, 116,
        229, 126, 221, 199, 155, 194, 192, 51, 207, 177, 240, 160, 84, 241, 41, 88, 176, 53,
        111, 28, 173, 177, 232, 158, 22, 79, 133, 152, 31, 32, 12, 196, 147, 58, 57, 50, 252,
        208, 131, 150, 179, 132, 178, 150, 234, 251, 143, 125, 163, 144, 20, 46, 71, 168, 252,
        164, 86, 120, 124, 56, 252, 206, 210, 236, 212, 139, 127, 189, 236, 40, 46, 2, 238, 13,
        216, 40, 48, 85, 133, 229, 181, 155, 176, 217, 241, 154, 153, 213, 112, 222, 72, 219,
        197, 3, 219, 56, 77, 109, 47, 72, 251, 131, 36, 240, 96, 169, 31, 82, 93, 166, 242, 3,
        33, 213, 2, 3, 1, 0, 1,
    ];
    pub const SDK_FINGERPRINT_PHRASE: &str = "turban-deftly-anime-chatroom-unselfish";

    /// **Bitwarden's own published fingerprint vector**, from the SDK:
    /// `sdk-internal/crates/bitwarden-crypto/src/fingerprint.rs`,
    /// `test_fingerprint`. A user id, that user's 294-byte SPKI public key,
    /// and the phrase every Bitwarden app shows under Account > Fingerprint
    /// phrase for them. The SDK keys HKDF with the raw SPKI; HMAC hashes a key
    /// longer than its block first, so that is `PRK = SHA-256(spki)` exactly as
    /// this module computes it -- which is the point of pinning it here.
    #[test]
    fn the_fingerprint_phrase_matches_bitwardens_published_vector() {
        let key = SDK_FINGERPRINT_KEY;
        let words = crate::rest::eff_wordlist::verify(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/eff_large_wordlist.txt"))
                .expect("the committed EFF list"),
        )
        .expect("it verifies");
        assert_eq!(
            fingerprint_phrase(SDK_FINGERPRINT_USER, key, &words).expect("a phrase"),
            SDK_FINGERPRINT_PHRASE
        );
        // Control: another user id with the same key is another phrase, so
        // the material really reaches the HKDF.
        assert_ne!(
            fingerprint_phrase("b09726a0-9590-49d1-a5f5-afe300b6a515", key, &words).expect("a phrase"),
            SDK_FINGERPRINT_PHRASE
        );
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --lib -j 2 eff_wordlist`
Expected: a compile error. `verify`, `EFF_WORDS` and `EffListError` are not found, and neither is `fingerprint_phrase` in crypto's tests.

- [ ] **Step 4: Write the production half of `eff_wordlist.rs`**

Insert above the `#[cfg(test)]` line of `src/rest/eff_wordlist.rs`. Replace the `EFF_WORDLIST_SHA256` bytes with the 32 bytes Step 1 printed:

```rust
//! **The EFF long word list**, which Bitwarden's fingerprint phrase is drawn
//! from -- 7,776 words, the list every Bitwarden app uses, so the five words
//! Deskwarden shows for a person are the five words that person's own app shows
//! them under Account > Fingerprint phrase.
//!
//! # Why it is not `assets/wordlist.txt`
//!
//! That is the passphrase list, 4,096 words (`password_gen::WORDLIST_WORDS`).
//! A phrase is only comparable if both sides index the same list, and
//! Bitwarden's is EFF's. This is a second file, not a second use of the first.
//!
//! # Loaded the way `password_gen` loads its list, and for the same reasons
//!
//! Installed beside the executable rather than compiled in: it is needed only
//! when somebody presses **Show fingerprint**, which is rare. Counted,
//! de-duplicated and hashed on every load: a list that is not exactly EFF's
//! would produce a phrase that matches nobody's, and a phrase that looks right
//! and is not is the one failure a fingerprint exists to prevent. The hash is
//! over the parsed words joined by `\n`, not the file, so a CRLF checkout
//! verifies.

use std::collections::HashSet;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// The file name, beside the executable. The installer's `[Files]` section
/// writes it there; `the_installer_ships_the_eff_list_beside_the_executable`
/// holds the two spellings together.
pub const EFF_WORDLIST_FILE: &str = "eff_large_wordlist.txt";

/// How many words the list holds: 6^5, one per roll of five dice.
pub const EFF_WORDS: usize = 7776;

/// SHA-256 of the list's words joined by `\n`. Changing the list means
/// changing this in the same commit.
const EFF_WORDLIST_SHA256: [u8; 32] = [
    /* the 32 bytes printed by Task 2 Step 1, e.g. 0x12, 0x34, ... */
];

/// The list, verified.
pub struct EffLongList {
    words: Vec<String>,
}

impl EffLongList {
    /// The word at `index`. Callers index with a value `< EFF_WORDS`, which
    /// [`verify`] guarantees is in range.
    #[must_use]
    pub fn word(&self, index: usize) -> &str {
        &self.words[index]
    }

    /// Always [`EFF_WORDS`] for a verified list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Never true for a verified list; here because `len` without it is a
    /// clippy warning.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Why a list could not be used. Three causes, for `password_gen`'s reason:
/// they need three different sentences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffListError {
    /// No file where one is looked for.
    Missing,
    /// A file that is not the EFF list: wrong count, a duplicate, a malformed
    /// word, or a different digest.
    Unusable,
    /// A file that could not be read (held open by an updater, say).
    Unreadable,
}

impl std::fmt::Display for EffListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "The fingerprint word list is missing. Reinstall Deskwarden.",
            Self::Unusable => "The fingerprint word list is not the one Deskwarden ships. Reinstall Deskwarden.",
            Self::Unreadable => "The fingerprint word list could not be read. Try again.",
        })
    }
}

/// Where the list is looked for, in order: beside the executable, then in
/// `assets/` beside it -- `password_gen::wordlist_paths`' two places, and for
/// its reason never the user's config directory.
#[must_use]
pub fn eff_wordlist_paths() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from))
    else {
        return Vec::new();
    };
    vec![dir.join(EFF_WORDLIST_FILE), dir.join("assets").join(EFF_WORDLIST_FILE)]
}

/// The installed list, read and verified now and dropped by the caller.
///
/// # Errors
///
/// See [`EffListError`].
pub fn load() -> Result<EffLongList, EffListError> {
    load_from(&eff_wordlist_paths())
}

/// [`load`] over paths the caller names -- split out, as `password_gen`'s is,
/// so a test can reach it without moving the test binary.
///
/// # Errors
///
/// See [`EffListError`].
pub fn load_from(paths: &[PathBuf]) -> Result<EffLongList, EffListError> {
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(path).map_err(|_| EffListError::Unreadable)?;
        return verify(&text);
    }
    Err(EffListError::Missing)
}

/// The checks, over the text. EFF's lines are `11111\tabacus`: the dice
/// column is read and dropped, and a bare word is accepted too.
///
/// # Errors
///
/// [`EffListError::Unusable`] for anything that is not exactly the EFF list.
pub fn verify(text: &str) -> Result<EffLongList, EffListError> {
    let words: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().last().map(str::to_string))
        .collect();
    if words.len() != EFF_WORDS {
        return Err(EffListError::Unusable);
    }
    // EFF's words are lowercase ASCII, four carry a hyphen (`drop-down`,
    // `felt-tip`, `t-shirt`, `yo-yo`), none is longer than nine characters.
    let well_formed = words.iter().all(|w| {
        (1..=9).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    });
    if !well_formed {
        return Err(EffListError::Unusable);
    }
    if words.iter().map(String::as_str).collect::<HashSet<_>>().len() != EFF_WORDS {
        return Err(EffListError::Unusable);
    }
    let mut hasher = Sha256::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(word.as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    if digest != EFF_WORDLIST_SHA256 {
        return Err(EffListError::Unusable);
    }
    Ok(EffLongList { words })
}
```

If `well_formed` fails on the real file, find the longest word with `python -c "print(max(open('assets/eff_large_wordlist.txt').read().split()[1::2], key=len))"` and widen the `1..=9` bound to that length. Do not drop the check.

In `src/rest/crypto.rs`, append this to the section added in Task 1:

```rust
/// How many words a fingerprint phrase has: Bitwarden's `hashPhrase` takes
/// `ceil(64 / log2(7776))` = 5, for at least 64 bits.
pub const FINGERPRINT_WORDS: usize = 5;

/// **Bitwarden's fingerprint phrase** for a user and their public key.
///
/// `HKDF-Expand(PRK = SHA-256(spki), info = material, L = 32)`, read as a
/// big-endian integer, then five words by repeated `mod 7776` / `div 7776`,
/// joined with `-`. `material` is the user id whose key this is -- the phrase
/// that user's own app shows. Pinned by
/// `the_fingerprint_phrase_matches_bitwardens_published_vector`.
pub fn fingerprint_phrase(
    material: &str,
    public_key_spki_der: &[u8],
    words: &crate::rest::eff_wordlist::EffLongList,
) -> Result<String, CryptoError> {
    let prk = Sha256::digest(public_key_spki_der);
    let hkdf = Hkdf::<Sha256>::from_prk(&prk)
        .map_err(|_| CryptoError::KeyLength { expected: KEY_LEN, got: prk.len() })?;
    let mut number = [0u8; KEY_LEN];
    hkdf.expand(material.as_bytes(), &mut number)
        .map_err(|_| CryptoError::KeyLength { expected: KEY_LEN, got: 0 })?;
    let mut phrase = Vec::with_capacity(FINGERPRINT_WORDS);
    for _ in 0..FINGERPRINT_WORDS {
        let index = divide_in_place(&mut number, crate::rest::eff_wordlist::EFF_WORDS as u32);
        phrase.push(words.word(index as usize));
    }
    Ok(phrase.join("-"))
}

/// Divides a big-endian unsigned integer by `divisor` in place and returns the
/// remainder: schoolbook long division, one byte at a time. The remainder is
/// always `< divisor`, so `rem << 8 | byte` stays below `divisor * 256`, which
/// fits `u32` for any divisor under 2^24 -- 7,776 is far under it.
fn divide_in_place(number: &mut [u8], divisor: u32) -> u32 {
    let mut remainder: u32 = 0;
    for byte in number.iter_mut() {
        let accumulated = (remainder << 8) | u32::from(*byte);
        *byte = (accumulated / divisor) as u8;
        remainder = accumulated % divisor;
    }
    remainder
}
```

- [ ] **Step 5: Ship the file (installer, build script, file-list pin)**

`installer/deskwarden.iss`: directly after the line `Source: "..\assets\wordlist.txt"; DestDir: "{app}"; Flags: ignoreversion`, add:

```
; The EFF long list the fingerprint phrase is drawn from. `rest::eff_wordlist`
; reads it from beside the executable when Show fingerprint is pressed, and
; `the_installer_ships_the_eff_list_beside_the_executable` reads this line.
Source: "..\assets\eff_large_wordlist.txt"; DestDir: "{app}"; Flags: ignoreversion
```

`build.rs`: make three edits.
- After `println!("cargo:rerun-if-changed=assets/wordlist.txt");`, add `println!("cargo:rerun-if-changed=assets/eff_large_wordlist.txt");`.
- Change the first doc line of `copy_wordlist_beside_the_built_binary` to `/// Puts \`assets/wordlist.txt\` and \`assets/eff_large_wordlist.txt\` next to the built \`deskwarden.exe\`.`
- Replace that function's body from `let source = ...` to the end of the `for` loop with the code below. Its comments avoid every token `job_object`'s build-script guard bans ("exec", "spawn", "process", "command"):

```rust
    // Two lists now: the passphrase words `password_gen` reads, and the EFF
    // long list `rest::eff_wordlist` reads for the fingerprint phrase. Both are
    // looked for beside the binary, so both are put there.
    for name in ["wordlist.txt", "eff_large_wordlist.txt"] {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(name);
        // BOTH the binary's own directory and `deps/` beside it. The first is
        // where `cargo run` puts `deskwarden.exe` and mirrors the installed
        // layout; the second is where cargo puts TEST binaries.
        for directory in [binary_dir.to_path_buf(), binary_dir.join("deps")] {
            let _ = std::fs::create_dir_all(&directory);
            if let Err(e) = std::fs::copy(&source, directory.join(name)) {
                println!(
                    "cargo:warning=could not copy the word list {name} to {}: {e}",
                    directory.display()
                );
            }
        }
    }
```

`password_gen`'s `the_build_script_copies_the_wordlist_beside_the_executable` needs `assets/wordlist.txt` and `wordlist.txt"`. Both are still present: the `rerun-if-changed` line and the array literal.

`src/job_object.rs`, in the non-Rust file list (around line 3349), insert between `"assets/deskwarden.ico",` and `"assets/fonts/Archivo-Bold.ttf",`:

```rust
                // The EFF long list `rest::eff_wordlist` reads for the
                // fingerprint phrase. Data, not a module: 7,776 words, pinned
                // by SHA-256 in `EFF_WORDLIST_SHA256`.
                "assets/eff_large_wordlist.txt",
```

Re-pin `build.rs`. Save this as `$TEMP/dw_fnv.py`:

```python
import sys
def fnv1a64(b):
    h = 0xcbf29ce484222325
    for ch in b:
        h ^= ch
        h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return h
raw = open(sys.argv[1], 'rb').read().replace(b'\r\n', b'\n')
print(len(raw), hex(fnv1a64(raw)))
```

Run:
```bash
git show HEAD:deskwarden/build.rs > "$TEMP/build_prev.rs" && python "$TEMP/dw_fnv.py" "$TEMP/build_prev.rs"
python "$TEMP/dw_fnv.py" build.rs
```

Expected:
- The first line prints `5143 0x47318eb94c5cf817`. That reproduces the current pin, so the script measures what the test measures.
- The second line prints the new pair.

In `src/job_object.rs` line 2081, replace `(5143, 0x4731_8eb9_4c5c_f817_u64),` with the new pair, written with `_` separators every four hex digits. Put this comment directly above it:

```rust
            // Re-pinned for personal sharing: `build.rs` now copies a second
            // list, `assets/eff_large_wordlist.txt`, beside the binary, in the
            // same loop and with the same best-effort warning. Nothing else
            // changed; no new call, no new dependency. Recomputed with a
            // separate FNV-1a/64 implementation (CRLF normalised to LF) that
            // first reproduced the previous pair (5143, 0x4731_8eb9_4c5c_f817).
```

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" --fix src/rest/eff_wordlist.rs
python "$TEMP/dw_crlf.py" src/rest/crypto.rs src/rest/mod.rs build.rs src/job_object.rs
cargo test --lib -j 2 eff_wordlist && cargo test --lib -j 2 rest::crypto && cargo test --lib -j 2 job_object && cargo test --lib -j 2 password_gen
```

Expected:
- Every file reports `0`.
- All pass, including `the_fingerprint_phrase_matches_bitwardens_published_vector` and `job_object`'s file-list and build-script pins.
- `build.rs` is not a `.rs` file under `src/`, but it is CRLF in this checkout, so check it with the others.

- [ ] **Step 7: Full verification and commit**

Run the four verification commands from Global Constraints. All must pass.

```bash
git add deskwarden/assets/eff_large_wordlist.txt deskwarden/src/rest/eff_wordlist.rs deskwarden/src/rest/mod.rs deskwarden/src/rest/crypto.rs deskwarden/build.rs deskwarden/installer/deskwarden.iss deskwarden/src/job_object.rs
git commit -m "A person's fingerprint phrase is Bitwarden's, from the EFF long list

The fingerprint phrase needs EFF's 7,776-word list, not the passphrase
list. It ships beside the executable like wordlist.txt, is copied there
by build.rs in the same loop, and is counted and hashed on every load.

The phrase is pinned to Bitwarden's own published SDK vector
(test_fingerprint: turban-deftly-anime-chatroom-unselfish), which closes
the spec's open item of capturing one by hand.

build.rs re-pinned in job_object (a second file in the copy loop, no new
call); the non-Rust file list gains the asset.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 3: The profile says who the account is, and which organisations are accepted but unconfirmed

**Files:**
- Modify: `deskwarden/src/rest/sync.rs`
  - `Profile` (lines 159-172);
  - `Organization` (lines 173-222);
  - `impl VaultKeys` (after `organization`, line ~640);
  - `private_key_der` (line 653);
  - tests appended to `pub mod tests` (line 1229+).

**Interfaces:**
- Produces:
  - `Profile.id: Option<String>`, `Profile.email: Option<String>`, `Profile.role: Option<String>`, `Profile.organizations_new: Vec<Organization>`
  - `Organization.kind: Option<i64>` (wire `type`), `Organization.role: Option<String>`, `Organization.status: Option<i64>`
  - `impl Organization { pub fn is_owner(&self) -> bool }`
  - `impl Profile { pub fn is_server_admin(&self) -> bool }`
  - `pub(crate) fn VaultKeys::with_organization(&self, id: &str, key: SymmetricKey) -> VaultKeys`
  - `pub(crate) fn private_key_der(user: &SymmetricKey, profile: &Profile) -> Result<Zeroizing<Vec<u8>>, CryptoError>` (visibility only)

- [ ] **Step 1: Write the failing tests**

Append inside `pub mod tests` in `src/rest/sync.rs`:

```rust
    /// **Who the account is, and what it is to each organisation**, read off
    /// the profile the sync (or `GET /api/accounts/profile`) already carries.
    /// Personal sharing needs all of it: the account's id and email name its
    /// sharing org and its fingerprint, `role` says whether it may mint a
    /// NodeWarden invite code, and `organizationsNew` is the one place an
    /// accepted-but-unconfirmed membership appears.
    #[test]
    fn a_profile_says_who_the_account_is_and_which_organisations_are_only_accepted() {
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "id": "user-1",
            "email": "Denis@Example.com",
            "role": "admin",
            "key": "2.aaa|bbb|ccc",
            "organizations": [
                { "id": "o1", "type": 0, "status": 2, "role": "owner", "name": "Shared by denis@example.com" }
            ],
            "organizationsNew": [
                { "id": "o1", "type": 0, "status": 2 },
                { "id": "o2", "type": 2, "status": 1, "name": "Shared by anna@example.com" }
            ]
        }))
        .expect("a profile");
        assert_eq!(profile.id.as_deref(), Some("user-1"));
        assert_eq!(profile.email.as_deref(), Some("Denis@Example.com"));
        assert!(profile.is_server_admin());
        assert!(profile.organizations[0].is_owner());
        assert_eq!(profile.organizations_new.len(), 2);
        assert_eq!(profile.organizations_new[1].status, Some(1));
        assert!(!profile.organizations_new[1].is_owner());

        // A profile from a server that sends none of it still parses, and says
        // nothing it was not told.
        let bare: Profile = serde_json::from_value(serde_json::json!({ "key": "2.a|b|c" })).expect("bare");
        assert!(bare.id.is_none() && bare.email.is_none() && bare.organizations_new.is_empty());
        assert!(!bare.is_server_admin());
    }

    /// NodeWarden writes the role as a word, official servers as `type`; either
    /// makes an owner.
    #[test]
    fn an_owner_is_recognised_by_role_or_by_type() {
        let by_role: Organization = serde_json::from_value(serde_json::json!({ "id": "a", "role": "Owner" })).expect("org");
        let by_type: Organization = serde_json::from_value(serde_json::json!({ "id": "b", "type": 0 })).expect("org");
        let member: Organization = serde_json::from_value(serde_json::json!({ "id": "c", "role": "member", "type": 0 })).expect("org");
        assert!(by_role.is_owner());
        assert!(by_type.is_owner());
        assert!(!member.is_owner(), "a role word wins over NodeWarden's constant `type: 0`");
    }

    /// **A just-created organisation's key, added without a second read.** The
    /// first share makes the org and then files a record into it; nothing on
    /// the server lists that org for this account until the next profile read,
    /// and asking for one would be a request for a key already in hand.
    #[test]
    fn keys_gain_an_organisation_without_losing_the_user_key_or_the_others() {
        let keys = keys_with_organisation(&[1u8; 64], "old", &[2u8; 64]);
        let fresh = crate::rest::crypto::tests::key_from_64(&[3u8; 64]);
        let more = keys.with_organization("new", fresh.clone());
        assert_eq!(more.organization("new"), Some(&fresh));
        assert_eq!(more.organization("old"), keys.organization("old"));
        assert_eq!(more.user(), keys.user());
        // Adding one that is already there replaces it rather than duplicating.
        let replaced = more.with_organization("old", fresh.clone());
        assert_eq!(replaced.organization("old"), Some(&fresh));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sync::tests::a_profile_says`
Expected: a compile error about the missing fields `id`, `email`, `organizations_new` and the missing methods `is_server_admin`, `is_owner`, `with_organization`.

- [ ] **Step 3: Implement**

In `pub struct Profile`, after the `organizations` field, add:

```rust
    /// The account's own id -- the material its fingerprint phrase is keyed
    /// with, and its org-user id on NodeWarden, where members are keyed by
    /// user id. Absent on servers that do not send it; nothing but sharing
    /// reads it.
    #[serde(default)]
    pub id: Option<String>,
    /// The account's email, as the server spells it. Sharing names its
    /// organisation after it (`Shared by <email>`).
    #[serde(default)]
    pub email: Option<String>,
    /// NodeWarden's account role: `"admin"` for the server's administrator,
    /// who may mint registration codes (`/api/admin/invites`). Official
    /// servers send no such field.
    #[serde(default)]
    pub role: Option<String>,
    /// Accepted AND confirmed memberships -- where official servers and
    /// NodeWarden from slice 1 put a membership that has no key yet.
    /// `organizations` above is confirmed only, which is why the recipient's
    /// "waiting for X to finish sharing" can only be read from here.
    #[serde(rename = "organizationsNew", default)]
    pub organizations_new: Vec<Organization>,
```

In `pub struct Organization`, after `enabled`, add:

```rust
    /// Bitwarden's `OrganizationUserType`: 0 owner, 1 admin, 2 user,
    /// 3 manager, 4 custom. NodeWarden sends a constant `0` here on some
    /// routes and says the real thing in `role`, which is why [`Self::is_owner`]
    /// reads `role` first.
    #[serde(rename = "type", default)]
    pub kind: Option<i64>,
    /// NodeWarden's word for the same thing: `owner`, `admin`, `manager`,
    /// `member`.
    #[serde(default)]
    pub role: Option<String>,
    /// 0 invited, 1 accepted, 2 confirmed.
    #[serde(default)]
    pub status: Option<i64>,
```

After `impl std::fmt::Debug for VaultKeys { ... }`, add a new impl block for the two structs:

```rust
impl Organization {
    /// Whether this account owns the organisation: NodeWarden's `role`, when
    /// it sent one, else the official `type == 0`.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        match self.role.as_deref() {
            Some(role) => role.eq_ignore_ascii_case("owner"),
            None => self.kind == Some(0),
        }
    }
}

impl Profile {
    /// NodeWarden's server administrator. Only they may mint a registration
    /// code; see [`Self::role`].
    #[must_use]
    pub fn is_server_admin(&self) -> bool {
        self.role.as_deref().is_some_and(|role| role.eq_ignore_ascii_case("admin"))
    }
}
```

In `impl VaultKeys`, after `pub(crate) fn organization`, add:

```rust
    /// These keys with one organisation's added (or replaced). For the sharing
    /// engine's first share: the org it just created is in no profile yet.
    #[must_use]
    pub(crate) fn with_organization(&self, id: &str, key: SymmetricKey) -> VaultKeys {
        let mut orgs: Vec<(String, SymmetricKey)> =
            self.orgs.iter().filter(|(known, _)| known != id).cloned().collect();
        orgs.push((id.to_string(), key));
        VaultKeys { user: self.user.clone(), orgs }
    }
```

Change `fn private_key_der(` to `pub(crate) fn private_key_der(`. Its doc stays. Add one line to the doc: `/// \`pub(crate)\` for the sharing engine, which derives the account's own public key from it.`

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib -j 2 rest::sync && cargo test --lib -j 2 rest::organizations && cargo test --lib -j 2 rest::backend`
Expected: all pass. The profile tests are new; the other two modules show nothing changed for existing callers.

- [ ] **Step 5: Verify and commit**

Run `python "$TEMP/dw_crlf.py" src/rest/sync.rs` (expect `0`), then the four verification commands.

```bash
git add deskwarden/src/rest/sync.rs
git commit -m "The profile says who the account is and which memberships are only accepted

Sharing needs the account's id and email (the org name, the fingerprint
material), NodeWarden's admin role (minting a registration code) and
profile.organizationsNew, the only list an accepted-but-unconfirmed
membership appears in. Organisations gain type, role and status, and
is_owner reads NodeWarden's role word before the official type number.

VaultKeys::with_organization adds a just-created org's key without a
second profile read.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: One door for the sharing routes, and a typed function per NodeWarden route

**Files:**
- Create: `deskwarden/src/rest/sharing_api.rs`
- Modify: `deskwarden/src/rest/mod.rs` (declare it)
- Modify: `deskwarden/src/rest/api.rs`
  - `is_url_path_safe` at line 2517 becomes `pub(crate)`;
  - new `send_sharing` after `fetch_organization_members` (line ~1505);
  - new `sharing_answer` after `classify_json_400` (line ~2394);
  - the census test at line ~3715.

**Interfaces:**
- Consumes:
  - `crate::rest::write::MappedCipher::body() -> &Value`;
  - `crate::rest::crypto::{EncString, spki_from_base64}`.
- Produces (`crate::rest::sharing_api`):
  - `pub enum SharingMethod { Get, Post, Put, Delete }`
  - `pub struct SharingRequest` with `pub(crate) fn method(&self) -> SharingMethod`, `pub(crate) fn path(&self) -> &str` and `pub(crate) fn body(&self) -> Option<&Value>`
  - `pub struct IncomingInvitation { pub id: String, pub organization_id: String, pub organization_name: Option<String>, pub token: Zeroizing<String>, pub expires_at: Option<String> }`
  - `pub struct AccountKey { pub user_id: String, pub public_key: Vec<u8> }`
  - `pub enum InviteOutcome { Member, Invitation { id: String }, AlreadyThere }`
  - `pub struct RawCollection { pub id: String, pub name: String }`
  - `pub struct InviteCode { pub code: String, pub link: Option<String>, pub expires_at: Option<String> }` (serde camelCase)
  - `pub struct NewOrganization { pub name: String, pub billing_email: String, pub key: EncString, pub public_key: String, pub encrypted_private_key: EncString, pub collection_name: EncString }`
  - Every function below takes `(client: &RestClient, session: &mut Session, …) -> Result<_, RestError>`:
    - `my_invitations -> Option<Vec<IncomingInvitation>>`
    - `user_public_key(user_id) -> Option<Vec<u8>>`
    - `create_organization(&NewOrganization) -> String`
    - `organization_collections(org) -> Vec<RawCollection>`
    - `create_collection(org, name: &EncString, manager_ou_id: &str) -> String`
    - `delete_collection(org, collection) -> ()`
    - `member_key_by_email(org, email) -> Option<AccountKey>`
    - `invite_member(org, email, collection_id, key: Option<&EncString>) -> InviteOutcome`
    - `confirm_member(org, ou_id, key: &EncString) -> ()`
    - `reinvite(org, ou_id) -> ()`
    - `remove_user(org, ou_id) -> ()`
    - `share_cipher(id, cipher: &MappedCipher, collection_ids: &[String]) -> Value`
    - `set_cipher_collections(id, collection_ids: &[String]) -> Value`
    - `accept_invitation(token) -> ()`
    - `decline_invitation(token) -> ()`
    - `leave_organization(org) -> ()`
    - `mint_invite_code(master_password_hash: &str) -> InviteCode`
  - `crate::rest::api::RestClient::send_sharing(&self, session: &mut Session, request: &SharingRequest) -> Result<serde_json::Value, RestError>` (`pub(crate)`)
  - `crate::rest::api::is_url_path_safe(id: &str) -> bool` (`pub(crate)`)

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/rest/sharing_api.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::Device;
    use crate::test_http::{Matcher, MockServer};

    const ORG: &str = "11111111-1111-1111-1111-111111111111";

    fn granted(server: &mut MockServer) -> (RestClient, Session) {
        server
            .mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let session = client.password_grant("a@b.c", "HASH", &device).expect("the grant");
        (client, session)
    }

    fn an_enc() -> EncString {
        "4.AAAA".parse().expect("a type-4 string")
    }

    /// **The invitation list, and what its absence means.** A 200 list is
    /// NodeWarden; a 404 is a server without the route -- the dialect probe's
    /// whole question -- and is `None`, not an error.
    #[test]
    fn my_invitations_are_read_and_a_missing_route_is_none() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let listed = server
            .mock("GET", "/api/organizations/invitations/mine")
            .match_header("Authorization", "Bearer AT-1")
            .with_body(serde_json::json!({ "object": "list", "data": [{
                "id": "inv-1", "organizationId": ORG, "organizationName": "Shared by denis@example.com",
                "token": "tok-1", "expirationDate": "2026-10-04T00:00:00.000Z", "object": "organizationInvitation"
            }]}).to_string())
            .expect(1)
            .create();
        let got = my_invitations(&client, &mut session).expect("a list").expect("NodeWarden");
        listed.assert();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].organization_name.as_deref(), Some("Shared by denis@example.com"));
        assert_eq!(got[0].token.as_str(), "tok-1");
        assert_eq!(got[0].expires_at.as_deref(), Some("2026-10-04T00:00:00.000Z"));

        let mut official = crate::test_http::server();
        let (client, mut session) = granted(&mut official);
        official.mock("GET", "/api/organizations/invitations/mine").with_status(404).create();
        assert!(my_invitations(&client, &mut session).expect("not an error").is_none());
    }

    /// **The organisation create body is the official one** (spec 9.5),
    /// which NodeWarden accepts from slice 1: plaintext name, the org key
    /// wrapped to the owner (type 4), the key pair, and the first person's
    /// collection name.
    #[test]
    fn an_organisation_is_created_with_the_official_body_and_its_id_comes_back() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let created = server
            .mock("POST", "/api/organizations")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": "Shared by denis@example.com",
                "billingEmail": "denis@example.com",
                "planType": 0,
                "key": "4.AAAA",
                "keys": { "publicKey": "PUB", "encryptedPrivateKey": "4.AAAA" },
                "collectionName": "4.AAAA"
            })))
            .with_body(r#"{"id":"org-new","object":"organization"}"#)
            .expect(1)
            .create();
        let body = NewOrganization {
            name: "Shared by denis@example.com".into(),
            billing_email: "denis@example.com".into(),
            key: an_enc(),
            public_key: "PUB".into(),
            encrypted_private_key: an_enc(),
            collection_name: an_enc(),
        };
        assert_eq!(create_organization(&client, &mut session, &body).expect("created"), "org-new");
        created.assert();
    }

    /// **The sharer manages every person collection**, in the create body
    /// itself -- spec 5.2: official servers refuse a collection nobody
    /// manages, and show an owner only collections assigned to them.
    #[test]
    fn a_person_collection_is_created_with_the_sharer_as_its_manager() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let created = server
            .mock("POST", format!("/api/organizations/{ORG}/collections").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": "4.AAAA",
                "users": [{ "id": "user-1", "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body(r#"{"id":"c-new","object":"collection"}"#)
            .expect(1)
            .create();
        assert_eq!(create_collection(&client, &mut session, ORG, &an_enc(), "user-1").expect("made"), "c-new");
        created.assert();
    }

    /// Direct membership (spec 7.2): the key lookup answers both "does the
    /// account exist" and "what is its key"; a 404 is "no account".
    #[test]
    fn an_accounts_key_is_looked_up_by_email_and_a_404_is_no_account() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let spki = crate::rest::crypto::tests::fixture_public_spki();
        let found = server
            .mock("GET", format!("/api/organizations/{ORG}/members/public-key?email=anna%40example.com").as_str())
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&spki) }).to_string())
            .expect(1)
            .create();
        let key = member_key_by_email(&client, &mut session, ORG, "anna@example.com").expect("ok").expect("found");
        found.assert();
        assert_eq!(key, AccountKey { user_id: "anna-id".into(), public_key: spki });

        server
            .mock("GET", format!("/api/organizations/{ORG}/members/public-key?email=nobody%40example.com").as_str())
            .with_status(404)
            .create();
        assert!(member_key_by_email(&client, &mut session, ORG, "nobody@example.com").expect("ok").is_none());
    }

    /// NodeWarden's invite: read-only access to the one collection, and the
    /// org key already wrapped for an existing account. The answer's `object`
    /// says which of the two branches the server took.
    #[test]
    fn an_invite_carries_read_only_access_and_says_which_branch_the_server_took() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let member = server
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({
                "email": "anna@example.com",
                "role": "member",
                "collections": [{ "id": "c-anna", "readOnly": true, "hidePasswords": false, "manage": false }],
                "encryptedOrgKey": "4.AAAA"
            })))
            .with_body(r#"{"id":"anna-id","status":1,"object":"organizationMember"}"#)
            .expect(1)
            .create();
        let outcome = invite_member(&client, &mut session, ORG, "anna@example.com", "c-anna", Some(&an_enc())).expect("invited");
        member.assert();
        assert_eq!(outcome, InviteOutcome::Member);

        let mut other = crate::test_http::server();
        let (client, mut session) = granted(&mut other);
        other
            .mock("POST", format!("/api/organizations/{ORG}/members/invite").as_str())
            .match_request(|request| !request.utf8_lossy_body().contains("encryptedOrgKey"))
            .with_body(r#"{"id":"inv-9","status":0,"object":"organizationInvitation"}"#)
            .create();
        assert_eq!(
            invite_member(&client, &mut session, ORG, "carol@example.com", "c-carol", None).expect("invited"),
            InviteOutcome::Invitation { id: "inv-9".into() }
        );
    }

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
            .mock("POST", format!("/api/organizations/{ORG}/users/anna-id/confirm").as_str())
            .with_status(400)
            .with_body(r#"{"error":"Already confirmed","error_description":"Already confirmed"}"#)
            .create();
        confirm_member(&client, &mut session, ORG, "anna-id", &an_enc()).expect("a race is a success");
        server.mock("DELETE", format!("/api/organizations/{ORG}/users/gone").as_str()).with_status(404).create();
        remove_user(&client, &mut session, ORG, "gone").expect("a 404 on a delete is a success");
    }

    /// Any other refusal keeps the server's own sentence, whichever key it
    /// wrote it under.
    #[test]
    fn a_refusal_keeps_the_servers_sentence() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server
            .mock("PUT", "/api/ciphers/c1/collections")
            .with_status(400)
            .with_body(r#"{"message":"You do not have permission to edit this item"}"#)
            .create();
        let err = set_cipher_collections(&client, &mut session, "c1", &["a".into()]).expect_err("refused");
        assert_eq!(
            err,
            RestError::Rejected { error: String::new(), description: "You do not have permission to edit this item".into() }
        );
    }

    /// **The share body is `{cipher, collectionIds}`**, and the cipher is a
    /// mapped one -- nothing here can put a hand-built cipher on the wire.
    #[test]
    fn a_share_puts_the_mapped_cipher_and_the_collections_to_the_share_route() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let cipher = crate::rest::write::tests::a_mapped_cipher();
        let shared = server
            .mock("PUT", "/api/ciphers/c1/share")
            .match_body(Matcher::Json(serde_json::json!({ "cipher": cipher.body().clone(), "collectionIds": ["c-anna"] })))
            .with_body(r#"{"id":"c1","object":"cipherDetails"}"#)
            .expect(1)
            .create();
        let answer = share_cipher(&client, &mut session, "c1", &cipher, &["c-anna".into()]).expect("shared");
        shared.assert();
        assert_eq!(answer["id"], "c1");
    }

    /// An empty 200 -- NodeWarden's confirm, leave and reinvite -- is `Null`
    /// and a success, never "the answer was not JSON".
    #[test]
    fn an_empty_success_is_a_success() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        server.mock("POST", format!("/api/organizations/{ORG}/leave").as_str()).with_body("").create();
        leave_organization(&client, &mut session, ORG).expect("left");
        server.mock("POST", format!("/api/organizations/{ORG}/users/ou-1/reinvite").as_str()).with_body("").create();
        reinvite(&client, &mut session, ORG, "ou-1").expect("reinvited");
    }

    /// An id that is not path-safe is refused before any request is made.
    #[test]
    fn an_unsafe_id_never_reaches_the_wire() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let any = server.mock("POST", Matcher::Any).expect(0).create();
        assert_eq!(leave_organization(&client, &mut session, "../x"), Err(RestError::UnsafeId));
        any.assert();
    }

    /// **The registration code a NodeWarden admin can mint** (owner decision
    /// Q3 (b)): the master-password hash goes in the body, never a query.
    #[test]
    fn an_invite_code_is_minted_with_the_password_hash_in_the_body() {
        let mut server = crate::test_http::server();
        let (client, mut session) = granted(&mut server);
        let minted = server
            .mock("POST", "/api/admin/invites")
            .match_body(Matcher::PartialJson(serde_json::json!({ "masterPasswordHash": "HASH==", "expiresInHours": 168 })))
            .with_status(201)
            .with_body(r#"{"code":"abc123","inviteLink":"https://vault.example/#/register?invite=abc123","expiresAt":"2026-10-04T00:00:00Z","object":"invite"}"#)
            .expect(1)
            .create();
        let code = mint_invite_code(&client, &mut session, "HASH==").expect("minted");
        minted.assert();
        assert_eq!(code.code, "abc123");
        assert_eq!(code.link.as_deref(), Some("https://vault.example/#/register?invite=abc123"));
    }
}
```

In `src/rest/mod.rs`, after `pub mod send_link;`, add:

```rust
/// **NodeWarden's sharing routes, typed** -- spec §7.3's NodeWarden column.
/// Every request goes out through `api::RestClient::send_sharing`, the one
/// door, and is built only here. No policy: `sharing` decides what to call.
pub mod sharing_api;
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing_api`
Expected: a compile error. `my_invitations`, `NewOrganization` and the other new names are not defined.

- [ ] **Step 3: Implement the door in `api.rs`**

Change `fn is_url_path_safe(id: &str) -> bool {` to `pub(crate) fn is_url_path_safe(id: &str) -> bool {`. Add one doc line above it: `/// \`pub(crate)\` for \`rest::sharing_api\`, which builds paths from server ids by the same rule.`

After `fetch_organization_members` (line ~1505), add:

```rust
    /// **The one door every sharing route goes through.**
    ///
    /// `request` is a [`crate::rest::sharing_api::SharingRequest`], which only
    /// that module can build -- the same argument [`MappedCipher`] makes for
    /// the cipher endpoints, applied to a family of routes. What its body can
    /// carry is therefore enumerable from one file: emails and ids in the
    /// clear (they are what these routes are about), and everything else as
    /// ciphertext -- an org key wrapped to a public key, a collection name
    /// under the org key, and, for `/share`, a [`MappedCipher`]'s own body.
    /// `the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin`
    /// names this door's one `send_json` so a second cannot hide behind it.
    ///
    /// Reads on the sync agent, writes on the write agent, both with the
    /// vault's session: every sharing route is the account acting on its own
    /// organisation, never a link a stranger pasted.
    pub(crate) fn send_sharing(
        &self,
        session: &mut Session,
        request: &crate::rest::sharing_api::SharingRequest,
    ) -> Result<serde_json::Value, RestError> {
        use crate::rest::sharing_api::SharingMethod;
        let url = format!("{}{}", self.base_url, request.path());
        self.refreshing(session, |session| {
            let call = match request.method() {
                SharingMethod::Get => self.bearer(self.sync_agent.get(&url), session),
                SharingMethod::Post => self.bearer(self.write_agent.post(&url), session),
                SharingMethod::Put => self.bearer(self.write_agent.put(&url), session),
                SharingMethod::Delete => self.bearer(self.write_agent.delete(&url), session),
            };
            let response = match request.body() {
                Some(body) => call.send_json(body),
                None => call.call(),
            };
            sharing_answer(response)
        })
    }
```

After `classify_json_400`, add:

```rust
/// A sharing route's outcome.
///
/// Its own classifier and not [`RestClient::value_from`], for two
/// differences that both come from these routes' real answers:
///
/// * **An empty 200 is a success.** NodeWarden answers confirm, reinvite and
///   leave with no body, and `value_from` would call that "not JSON".
/// * **A 400 keeps the server's sentence wherever it wrote it.** NodeWarden
///   writes `error` and `error_description`, official servers
///   `ErrorModel.Message`, some routes `message` -- and the sentence is
///   what the Share dialog shows ("User is already a member"), and what the
///   engine matches on to treat a two-PC race as a success.
fn sharing_answer(
    response: Result<ureq::Response, ureq::Error>,
) -> Result<serde_json::Value, RestError> {
    match response {
        Ok(ok) => {
            let text = ok.into_string().map_err(|_| RestError::Parse("a sharing answer"))?;
            if text.trim().is_empty() {
                return Ok(serde_json::Value::Null);
            }
            serde_json::from_str(&text).map_err(|_| RestError::Parse("a sharing answer"))
        }
        Err(ureq::Error::Status(401, _)) => Err(RestError::Unauthorized),
        Err(ureq::Error::Status(400, body)) => {
            let json = json_body_of(body);
            let said = ["error_description", "message", "error"]
                .iter()
                .map(|key| string_at(&json, key))
                .find(|text| !text.is_empty())
                .or_else(|| {
                    json.get("ErrorModel")
                        .and_then(|m| m.get("Message"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            Err(RestError::Rejected { error: string_at(&json, "error"), description: said })
        }
        Err(ureq::Error::Status(code, _)) => Err(RestError::Status(code)),
        Err(e @ ureq::Error::Transport(_)) => Err(RestError::Transport(e.to_string())),
    }
}
```

In the census test `the_only_json_bodies_this_module_sends_are_mapped_ciphers_and_the_prelogin`:
- change the `send_json(` total from `10` to `11`;
- add this paragraph to the comment directly above that assertion: `// **Eleven, and the eleventh is the sharing door.** \`send_sharing\` sends a \`SharingRequest\`'s body, and that type has no constructor outside \`rest::sharing_api\`, so every body through it is enumerable in one file; see the door's own doc.`
- directly after the Send-update assertion, add:

```rust
        // **The eleventh is the sharing door**, and it sends the body of a
        // `SharingRequest` -- a type built only in `rest::sharing_api`, whose
        // bodies are ids, emails and ciphertext. Named so that a twelfth
        // cannot hide behind its allowance.
        assert_eq!(
            production.matches("Some(body) => call.send_json(body)").count(),
            1,
            "the sharing door stopped sending its request's own body, or gained a twin"
        );
```

`send_json(&body)` stays at 0 and the write-agent chain count stays at 6: the door binds `call` first, so neither needle matches it.

- [ ] **Step 4: Implement `sharing_api.rs`**

Insert above the test module:

```rust
//! **NodeWarden's sharing routes**, one typed function each -- the NodeWarden
//! column of spec §7.3, with the request and answer shapes of §10 and of the
//! routes NodeWarden already has (`/members/*`, `/invitations/*`,
//! `/admin/invites`).
//!
//! No policy lives here. Which route to call, in what order, and what a
//! refusal means for the user is `rest::sharing`'s. What lives here is the
//! shape of each request and each answer, and the handful of answers that
//! mean success in disguise: a 404 on a delete, "already a member", "already
//! confirmed" -- the two-PC race of spec §13, which converges by treating
//! the loser's refusal as the winner's result.
//!
//! Every request goes through [`RestClient::send_sharing`], and
//! [`SharingRequest`] -- the only thing that door accepts -- is constructed
//! nowhere but this file.

use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::rest::api::{RestClient, RestError, Session, is_url_path_safe};
use crate::rest::crypto::{EncString, spki_from_base64};
use crate::rest::write::MappedCipher;

/// The four verbs these routes use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// One sharing request. **No `Debug`**: a body can hold a member's email and
/// an invitation token, and nothing needs to print one.
pub struct SharingRequest {
    method: SharingMethod,
    path: String,
    body: Option<Value>,
}

impl SharingRequest {
    fn new(method: SharingMethod, path: String, body: Option<Value>) -> Self {
        Self { method, path, body }
    }

    pub(crate) fn method(&self) -> SharingMethod {
        self.method
    }

    /// The path under the server root, starting with `/api/`.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn body(&self) -> Option<&Value> {
        self.body.as_ref()
    }
}

/// A server id checked for use in a path -- [`is_url_path_safe`]'s rule,
/// the same one every other id in `rest::api` is held to.
fn safe(id: &str) -> Result<&str, RestError> {
    if is_url_path_safe(id) { Ok(id) } else { Err(RestError::UnsafeId) }
}

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

/// A server without this route: its whole answer is "not here".
fn route_absent(error: &RestError) -> bool {
    matches!(error, RestError::Status(404 | 405 | 501))
}

/// The server's sentence, lower-cased, when it refused.
fn refusal(error: &RestError) -> Option<String> {
    match error {
        RestError::Rejected { error, description } => Some(format!("{error} {description}").to_lowercase()),
        _ => None,
    }
}

/// `{"object":"list","data":[..]}` or a bare array, as `rest::organizations`
/// reads its envelopes.
fn rows(answer: &Value) -> Vec<Value> {
    match answer.get("data") {
        Some(Value::Array(rows)) => rows.clone(),
        _ => answer.as_array().cloned().unwrap_or_default(),
    }
}

fn text_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn send(client: &RestClient, session: &mut Session, request: &SharingRequest) -> Result<Value, RestError> {
    client.send_sharing(session, request)
}

// ---- answers ----------------------------------------------------------------

/// An invitation addressed to this account (`GET /invitations/mine`).
///
/// **No `Debug`**, because the token is what accepts it.
#[derive(Clone)]
pub struct IncomingInvitation {
    pub id: String,
    pub organization_id: String,
    /// Plaintext on every server; for a sharing org, `Shared by <email>`.
    pub organization_name: Option<String>,
    pub token: Zeroizing<String>,
    pub expires_at: Option<String>,
}

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

/// A collection as the org's own listing answers it: the name still ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCollection {
    pub id: String,
    pub name: String,
}

/// A NodeWarden registration code, as `/api/admin/invites` answers it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCode {
    pub code: String,
    pub link: Option<String>,
    pub expires_at: Option<String>,
}

/// The official create body, spec §9.5.
pub struct NewOrganization {
    pub name: String,
    pub billing_email: String,
    /// The org key wrapped to the owner's own public key (type 4).
    pub key: EncString,
    /// SPKI DER, base64.
    pub public_key: String,
    /// PKCS#8 under the org key (type 2).
    pub encrypted_private_key: EncString,
    /// The first person's email under the org key (type 2).
    pub collection_name: EncString,
}

// ---- the routes ---------------------------------------------------------------

/// `GET /api/organizations/invitations/mine` -- NodeWarden's, and the dialect
/// probe's question. `None` is a server without the route (official,
/// Vaultwarden), which is an answer and not an error.
pub fn my_invitations(
    client: &RestClient,
    session: &mut Session,
) -> Result<Option<Vec<IncomingInvitation>>, RestError> {
    let request = SharingRequest::new(SharingMethod::Get, "/api/organizations/invitations/mine".into(), None);
    match send(client, session, &request) {
        Ok(answer) => Ok(Some(
            rows(&answer)
                .iter()
                .filter_map(|row| {
                    Some(IncomingInvitation {
                        id: text_at(row, "id")?,
                        organization_id: text_at(row, "organizationId")?,
                        organization_name: text_at(row, "organizationName"),
                        token: Zeroizing::new(text_at(row, "token")?),
                        expires_at: text_at(row, "expirationDate"),
                    })
                })
                .collect(),
        )),
        Err(e) if route_absent(&e) || matches!(e, RestError::Rejected { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET /api/users/{userId}/public-key` -- slice 1's second probe, and the
/// key every confirm wraps to. `None`: no such route, no such user, or a user
/// who has never opened an app that made keys.
pub fn user_public_key(
    client: &RestClient,
    session: &mut Session,
    user_id: &str,
) -> Result<Option<Vec<u8>>, RestError> {
    let path = format!("/api/users/{}/public-key", safe(user_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Get, path, None)) {
        Ok(answer) => match text_at(&answer, "publicKey").or_else(|| text_at(&answer, "key")) {
            Some(key) => Ok(Some(spki_from_base64(&key).map_err(|_| RestError::Parse("a public key"))?)),
            None => Ok(None),
        },
        Err(RestError::Status(404)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations`, the official body. Returns the new org's id.
pub fn create_organization(
    client: &RestClient,
    session: &mut Session,
    body: &NewOrganization,
) -> Result<String, RestError> {
    let json = json!({
        "name": body.name,
        "billingEmail": body.billing_email,
        "planType": 0,
        "key": body.key.to_string(),
        "keys": {
            "publicKey": body.public_key,
            "encryptedPrivateKey": body.encrypted_private_key.to_string(),
        },
        "collectionName": body.collection_name.to_string(),
    });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, "/api/organizations".into(), Some(json)))?;
    text_at(&answer, "id").ok_or(RestError::Parse("the new organisation's id"))
}

/// `GET /api/organizations/{id}/collections` -- every collection of the org,
/// for its manager. Names are ciphertext.
pub fn organization_collections(
    client: &RestClient,
    session: &mut Session,
    org: &str,
) -> Result<Vec<RawCollection>, RestError> {
    let path = format!("/api/organizations/{}/collections", safe(org)?);
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Get, path, None))?;
    Ok(rows(&answer)
        .iter()
        .filter_map(|row| Some(RawCollection { id: text_at(row, "id")?, name: text_at(row, "name")? }))
        .collect())
}

/// `POST /api/organizations/{id}/collections` with the sharer as its one
/// manager (spec §5.2, §7.3). Returns the collection's id.
pub fn create_collection(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    name: &EncString,
    manager_ou_id: &str,
) -> Result<String, RestError> {
    let path = format!("/api/organizations/{}/collections", safe(org)?);
    let json = json!({
        "name": name.to_string(),
        "externalId": null,
        "groups": [],
        "users": [{ "id": manager_ou_id, "readOnly": false, "hidePasswords": false, "manage": true }],
    });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json)))?;
    text_at(&answer, "id").ok_or(RestError::Parse("the new collection's id"))
}

/// `DELETE /api/organizations/{id}/collections/{cid}`. Already gone is done.
pub fn delete_collection(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    collection: &str,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/collections/{}", safe(org)?, safe(collection)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `GET /api/organizations/{id}/members/public-key?email=` -- whether the
/// account exists, and its key (spec §7.2, the direct-add accelerator).
pub fn member_key_by_email(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
) -> Result<Option<AccountKey>, RestError> {
    let path = format!("/api/organizations/{}/members/public-key?email={}", safe(org)?, query_value(email));
    match send(client, session, &SharingRequest::new(SharingMethod::Get, path, None)) {
        Ok(answer) => {
            let (Some(user_id), Some(key)) = (text_at(&answer, "userId"), text_at(&answer, "publicKey")) else {
                return Ok(None);
            };
            let public_key = spki_from_base64(&key).map_err(|_| RestError::Parse("a public key"))?;
            Ok(Some(AccountKey { user_id, public_key }))
        }
        Err(RestError::Status(404)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations/{id}/members/invite`: read-only access to one
/// collection, with the org key already wrapped when the account exists.
pub fn invite_member(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    email: &str,
    collection_id: &str,
    key: Option<&EncString>,
) -> Result<InviteOutcome, RestError> {
    let path = format!("/api/organizations/{}/members/invite", safe(org)?);
    let mut json = json!({
        "email": email,
        "role": "member",
        "collections": [{ "id": collection_id, "readOnly": true, "hidePasswords": false, "manage": false }],
    });
    if let (Some(key), Some(object)) = (key, json.as_object_mut()) {
        object.insert("encryptedOrgKey".into(), Value::String(key.to_string()));
    }
    match send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json))) {
        Ok(answer) => Ok(match answer.get("object").and_then(Value::as_str) {
            Some("organizationInvitation") => {
                InviteOutcome::Invitation { id: text_at(&answer, "id").unwrap_or_default() }
            }
            _ => InviteOutcome::Member,
        }),
        Err(e) if refusal(&e).is_some_and(|s| s.contains("already a member") || s.contains("already been invited")) => {
            Ok(InviteOutcome::AlreadyThere)
        }
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations/{id}/users/{ouId}/confirm {key}`. "Already
/// confirmed" is another PC's confirm, and a success.
pub fn confirm_member(
    client: &RestClient,
    session: &mut Session,
    org: &str,
    ou_id: &str,
    key: &EncString,
) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/confirm", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Post, path, Some(json!({ "key": key.to_string() })))) {
        Ok(_) => Ok(()),
        Err(e) if refusal(&e).is_some_and(|s| s.contains("already confirmed")) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `POST /api/organizations/{id}/users/{ouId}/reinvite` -- a new token and a
/// new 7 days for a pending invitation.
pub fn reinvite(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}/reinvite", safe(org)?, safe(ou_id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, None)).map(|_| ())
}

/// `DELETE /api/organizations/{id}/users/{ouId}` -- a member or an invitation.
/// Already gone is done.
pub fn remove_user(client: &RestClient, session: &mut Session, org: &str, ou_id: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/users/{}", safe(org)?, safe(ou_id)?);
    match send(client, session, &SharingRequest::new(SharingMethod::Delete, path, None)) {
        Ok(_) | Err(RestError::Status(404)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// `PUT /api/ciphers/{id}/share {cipher, collectionIds}` -- a personal record
/// into the org, id unchanged. Returns the server's copy (still ciphertext).
pub fn share_cipher(
    client: &RestClient,
    session: &mut Session,
    id: &str,
    cipher: &MappedCipher,
    collection_ids: &[String],
) -> Result<Value, RestError> {
    let path = format!("/api/ciphers/{}/share", safe(id)?);
    let json = json!({ "cipher": cipher.body().clone(), "collectionIds": collection_ids });
    send(client, session, &SharingRequest::new(SharingMethod::Put, path, Some(json)))
}

/// `PUT /api/ciphers/{id}/collections {collectionIds}` -- which people a
/// shared record reaches. Returns the server's copy.
pub fn set_cipher_collections(
    client: &RestClient,
    session: &mut Session,
    id: &str,
    collection_ids: &[String],
) -> Result<Value, RestError> {
    let path = format!("/api/ciphers/{}/collections", safe(id)?);
    send(client, session, &SharingRequest::new(SharingMethod::Put, path, Some(json!({ "collectionIds": collection_ids }))))
}

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

/// `POST /api/organizations/{id}/leave` -- the recipient's "stop receiving".
pub fn leave_organization(client: &RestClient, session: &mut Session, org: &str) -> Result<(), RestError> {
    let path = format!("/api/organizations/{}/leave", safe(org)?);
    send(client, session, &SharingRequest::new(SharingMethod::Post, path, None)).map(|_| ())
}

/// `POST /api/admin/invites {masterPasswordHash, expiresInHours}` -- a
/// registration code, for a NodeWarden administrator sharing with somebody
/// who has no account (owner decision Q3 (b)). Seven days, matching the org
/// invitation's own expiry.
pub fn mint_invite_code(
    client: &RestClient,
    session: &mut Session,
    master_password_hash: &str,
) -> Result<InviteCode, RestError> {
    let json = json!({ "masterPasswordHash": master_password_hash, "expiresInHours": 168 });
    let answer = send(client, session, &SharingRequest::new(SharingMethod::Post, "/api/admin/invites".into(), Some(json)))?;
    Ok(InviteCode {
        code: text_at(&answer, "code").ok_or(RestError::Parse("the invite code"))?,
        link: text_at(&answer, "inviteLink"),
        expires_at: text_at(&answer, "expiresAt"),
    })
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" --fix src/rest/sharing_api.rs
python "$TEMP/dw_crlf.py" src/rest/api.rs src/rest/mod.rs
cargo test --lib -j 2 rest::sharing_api && cargo test --lib -j 2 rest::api
```
Expected: every file reports `0`. All pass, including the updated census.

If the `match_query`-less path matcher does not match `?email=anna%40example.com`, keep the whole path-plus-query string: this server's path matcher sees both. Do not loosen the test to `Matcher::Any`.

- [ ] **Step 6: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing_api.rs deskwarden/src/rest/mod.rs deskwarden/src/rest/api.rs
git commit -m "NodeWarden's sharing routes have one typed function each, through one door

rest::sharing_api builds every sharing request (org create, person
collections, direct add, invites, confirm, reinvite, remove, share,
collections, invitations, leave, the admin's invite code) and nothing
else constructs one. RestClient::send_sharing is the single door; the
body-sender census moves from ten to eleven and names it.

The door's own answer reader keeps the server's refusal sentence
wherever it is written and treats an empty 200 as success. The two-PC
race answers (already a member, already confirmed, a 404 on a delete)
are successes here.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Rosters say who is invited, accepted and confirmed; sharing orgs are recognised and hidden

**Files:**
- Modify: `deskwarden/src/rest/organizations.rs`
  - `Organisation`, `Member`, `Audience`, `Directory`, `from_sync`, `load`, `audience_of`, `map_member`, the wire structs, `to_body`/`from_body`;
  - tests.
- Modify: `deskwarden/src/rest/api.rs`: `fetch_organization_members` (line 1496) tries `/users` first.
- Modify: `deskwarden/src/vault_window/detail_edit.rs`: `owner_line` (line 10247).
- Modify: `deskwarden/src/vault_window/sidebar.rs`: `draw_organisations` (line 1707).

**Interfaces:**
- Produces (`crate::rest::organizations`):
  - `pub enum MemberStatus { Invited, Accepted, Confirmed }` (serde camelCase, `Copy`)
  - `Member { id, user_id: Option<String>, email, name, role, status: MemberStatus, collections: Vec<String>, read_only: Vec<String>, invitation_expires: Option<String> }`
  - `Member::confirmed(&self) -> bool` (status is Confirmed)
  - `Member::has_access(&self, collection_id: &str) -> bool`
  - `pub struct SharingOrg { pub sharer: String, pub mine: bool }`
  - `Organisation { id, name, enabled, sharing: Option<SharingOrg> }`
  - `pub const SHARING_ORG_PREFIX: &str = "Shared by "`
  - `pub fn normalise_email(email: &str) -> String`
  - `pub fn sharing_org_name(email: &str) -> String`
  - `pub fn sharer_of(name: &str) -> Option<&str>`
  - `pub fn sharer_is(claimed: &str, email: &str) -> bool`
  - `Directory::visible_organisations(&self) -> Vec<&Organisation>`
  - `Directory::sharing_orgs(&self) -> Vec<&Organisation>`
  - `Directory::my_sharing_org(&self) -> Option<&Organisation>`
  - `Directory::waiting(&self) -> &[Organisation]`
  - `Directory::collection_named(&self, org: &str, email: &str) -> Option<&Collection>`
  - `Audience::SharedWithPeople { people: Vec<String> }` and `Audience::SharedWithMe { from: String }`
  - `Directory::from_sync` now also reads `profile.email` and `organizations_new`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/organizations.rs`:

```rust
    // ---- personal sharing -------------------------------------------------

    /// `/users` rows (official shape, NodeWarden from slice 1) say the three
    /// states in `status`; NodeWarden's older `/members` rows ARE the account
    /// and say "accepted" as status 1 with no key. One roster reader serves
    /// both, and `status 1` is never read as confirmed from either.
    #[test]
    fn a_roster_row_says_invited_accepted_or_confirmed_whichever_route_answered() {
        let users = json!({ "data": [
            { "object": "organizationUserUserDetails", "id": "ou-1", "userId": "u-1", "email": "a@x.io",
              "type": 2, "status": 2, "collections": [{ "id": "c1", "readOnly": true, "hidePasswords": false, "manage": false }] },
            { "object": "organizationUserUserDetails", "id": "ou-2", "userId": "u-2", "email": "b@x.io", "type": 2, "status": 1 },
            { "object": "organizationUserUserDetails", "id": "inv-3", "userId": null, "email": "c@x.io", "type": 2, "status": 0,
              "invitationExpirationDate": "2026-10-04T00:00:00Z" }
        ]});
        let members = map_members(&users).expect("a roster");
        assert_eq!(members[0].status, MemberStatus::Confirmed);
        assert_eq!(members[0].user_id.as_deref(), Some("u-1"));
        assert!(members[0].has_access("c1"));
        assert_eq!(members[0].read_only, ["c1"]);
        assert_eq!(members[1].status, MemberStatus::Accepted);
        assert!(!members[1].confirmed());
        assert_eq!(members[2].status, MemberStatus::Invited);
        assert_eq!(members[2].user_id, None);
        assert_eq!(members[2].invitation_expires.as_deref(), Some("2026-10-04T00:00:00Z"));

        let legacy = json!({ "data": [
            { "object": "organizationMember", "id": "u-9", "email": "d@x.io", "role": "member", "type": 0, "status": 1, "encryptedOrgKey": null, "collections": ["c1"] },
            { "object": "organizationMember", "id": "u-8", "email": "e@x.io", "role": "owner", "type": 0, "status": 1, "encryptedOrgKey": "4.k", "collections": [] }
        ]});
        let members = map_members(&legacy).expect("a roster");
        assert_eq!(members[0].status, MemberStatus::Accepted, "status 1 with no key is accepted, not confirmed");
        assert_eq!(members[0].user_id.as_deref(), Some("u-9"));
        assert_eq!(members[0].role, "member", "NodeWarden's constant `type: 0` is not an owner");
        assert_eq!(members[1].status, MemberStatus::Confirmed);
        assert_eq!(members[1].role, "owner");
    }

    #[test]
    fn a_sharing_org_is_named_after_the_sharer_and_a_long_address_is_cut_honestly() {
        assert_eq!(sharing_org_name(" Denis@Example.com "), "Shared by denis@example.com");
        let long = "denis.platonov@a-very-long-domain-name-indeed.example";
        let name = sharing_org_name(long);
        assert_eq!(name.chars().count(), 50);
        assert!(name.ends_with('…'));
        assert_eq!(sharer_of(&name), Some("denis.platonov@a-very-long-domain-name-in…"));
        assert!(sharer_is(sharer_of(&name).expect("a sharer"), long));
        assert!(!sharer_is(sharer_of(&name).expect("a sharer"), "someone@else.example"));
        assert_eq!(sharer_of("Engineering"), None);
        assert_eq!(sharer_of("Shared by nobody"), None, "no `@`, not an email");
    }

    /// **The rail never shows a sharing org, and an audience never names one.**
    #[test]
    fn sharing_orgs_are_left_out_of_the_visible_organisations_and_answer_as_people() {
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "email": "denis@example.com",
                "organizations": [
                    { "id": ORG, "name": "Engineering", "enabled": true, "key": "4.zzz" },
                    { "id": "mine", "name": "Shared by denis@example.com", "role": "owner", "key": "4.zzz" },
                    { "id": "theirs", "name": "Shared by anna@example.com", "role": "member", "key": "4.zzz" }
                ],
                "organizationsNew": [
                    { "id": "pending", "name": "Shared by carol@example.com", "role": "member", "status": 1 }
                ]
            },
            "ciphers": [], "folders": [],
            "collections": [ { "id": "c-bob", "organizationId": "mine", "name": "bob@example.com" } ]
        })).expect("parses");
        let directory = Directory::from_sync(&response, &keys());
        let visible: Vec<&str> = directory.visible_organisations().iter().map(|o| o.id.as_str()).collect();
        assert_eq!(visible, [ORG]);
        assert_eq!(directory.my_sharing_org().map(|o| o.id.as_str()), Some("mine"));
        assert_eq!(directory.waiting().iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["pending"]);
        assert_eq!(directory.collection_named("mine", "BOB@example.com").map(|c| c.id.as_str()), Some("c-bob"));

        assert_eq!(
            directory.audience_of(&item(Some("mine"), &["c-bob"])),
            Audience::SharedWithPeople { people: vec!["bob@example.com".to_string()] }
        );
        assert_eq!(
            directory.audience_of(&item(Some("theirs"), &[])),
            Audience::SharedWithMe { from: "anna@example.com".to_string() }
        );
        assert!(directory.audience_of(&item(Some("theirs"), &[])).is_shared());

        // The wire keeps all of it, for a window reading the daemon's copy.
        let round = Directory::from_body(directory.to_body()).expect("reads back");
        assert_eq!(round, directory);
    }

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

Rename every `.confirmed` on a `Member` in this file's existing tests (the one at line ~1178) to use the method: `assert!(members[0].confirmed(), …)`.

In `src/vault_window/sidebar.rs`, add this test to its `tests` module (the one holding `scrolled_rail`, near line 5592).

```rust
    /// **The ORGANISATIONS rail never lists a sharing org** -- spec 6.9's
    /// hidden org. It is plumbing; its records live under Shared with me and
    /// Sent instead.
    #[test]
    fn sharing_orgs_are_absent_from_the_organisations_rail() {
        let response: crate::rest::sync::SyncResponse = serde_json::from_value(serde_json::json!({
            "profile": { "key": "2.a|b|c", "email": "me@example.com", "organizations": [
                { "id": "o-eng", "name": "Engineering" },
                { "id": "o-share", "name": "Shared by me@example.com", "role": "owner" }
            ]},
            "ciphers": [], "folders": [], "collections": []
        })).expect("parses");
        let sharing = crate::rest::organizations::Directory::from_sync(
            &response,
            &crate::rest::sync::tests::keys_from_user(&[1u8; 64]),
        );
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&[]) };
        // `scrolled_rail` is the sidebar tests' own harness (see the
        // ORGANISATIONS overflow test above): it draws the real rail and
        // returns every painted label with its rect. Scrolled to the end so
        // the section is on screen whatever the rail's height.
        let (painted, _bounds) = scrolled_rail(lists, &[], -6000.0);
        let labels: Vec<&str> = painted.iter().map(|(text, _)| text.as_str()).collect();
        assert!(labels.contains(&"Engineering"), "control: the rail drew no organisation at all: {labels:?}");
        assert!(!labels.iter().any(|l| l.starts_with("Shared by")), "a sharing org reached the rail: {labels:?}");
    }
```

If `scrolled_rail`'s folder argument is not a slice, pass an empty value of whatever type the overflow test passes (`&folders`, where `folders` is built above it).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::organizations`
Expected: a compile error. `MemberStatus`, `sharing_org_name`, `visible_organisations` and `Audience::SharedWithPeople` are not defined.

- [ ] **Step 3: Implement in `organizations.rs`**

(a) After `UNREADABLE_NAME`, add:

```rust
// ---- sharing organisations ---------------------------------------------------
//
// Personal sharing (spec 5.1) puts one hidden organisation behind each sharer,
// named `Shared by <sharer email>` in plaintext. The name is the one place
// every client can read -- the recipient's Deskwarden must recognise SOMEONE
// ELSE'S sharing org to hide it and to say "from ...", and so must the
// sharer's second PC -- so recognition is by name, and "mine" is the name plus
// ownership.

/// The exact prefix a sharing organisation's name starts with.
pub const SHARING_ORG_PREFIX: &str = "Shared by ";

/// Official servers probably cap org names at 50 (spec 4.3); names are kept
/// within it everywhere, which leaves 40 characters for the email.
const SHARING_ORG_NAME_LIMIT: usize = 50;

/// An email as sharing compares it: trimmed and lower-cased.
#[must_use]
pub fn normalise_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// The name a sharer's organisation is created with. An email longer than 40
/// characters is cut to 39 and `…`, so the label reads
/// `from denis.platonov@a-very-long-dom…` -- which is honest.
#[must_use]
pub fn sharing_org_name(email: &str) -> String {
    let email = normalise_email(email);
    let room = SHARING_ORG_NAME_LIMIT - SHARING_ORG_PREFIX.len();
    let shown = if email.chars().count() > room {
        let cut: String = email.chars().take(room - 1).collect();
        format!("{cut}\u{2026}")
    } else {
        email
    };
    format!("{SHARING_ORG_PREFIX}{shown}")
}

/// The sharer a name claims, when it is a sharing org's: the prefix, then
/// something that parses as an email (one `@`, no whitespace), optionally cut
/// with `…`.
#[must_use]
pub fn sharer_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(SHARING_ORG_PREFIX)?;
    let bare = rest.strip_suffix('\u{2026}').unwrap_or(rest);
    let mut halves = bare.split('@');
    let (Some(local), Some(domain), None) = (halves.next(), halves.next(), halves.next()) else {
        return None;
    };
    let plausible = !local.is_empty()
        && !domain.is_empty()
        && !bare.chars().any(char::is_whitespace);
    plausible.then_some(rest)
}

/// Whether `claimed` -- what [`sharer_of`] read out of a name -- is `email`.
/// A cut name matches every address it is a prefix of and that was long
/// enough to have been cut.
#[must_use]
pub fn sharer_is(claimed: &str, email: &str) -> bool {
    let email = normalise_email(email);
    match claimed.strip_suffix('\u{2026}') {
        Some(cut) => email.starts_with(&normalise_email(cut)) && sharing_org_name(&email).ends_with('\u{2026}'),
        None => normalise_email(claimed) == email,
    }
}

/// A sharing organisation, as the directory marks one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharingOrg {
    /// The sharer's email as the name carries it (possibly cut with `…`).
    pub sharer: String,
    /// This account owns it and is the sharer it names.
    pub mine: bool,
}
```

(b) `Organisation` gains a field, after `enabled`:

```rust
    /// `Some` for a personal-sharing organisation -- hidden from every
    /// organisations surface, see [`Directory::visible_organisations`].
    pub sharing: Option<SharingOrg>,
```

(c) Replace `Member` and its impl with:

```rust
/// Where one membership stands. Bitwarden's three steps (spec 2): invited
/// (a row, no account attached yet on NodeWarden), accepted (no org key yet),
/// confirmed (holds the org key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemberStatus {
    Invited,
    Accepted,
    Confirmed,
}

/// One member of one organisation. See the module docs for what it carries
/// and why it is thin; personal sharing added the org-user id, the account id,
/// the three-state status, which collections are read-only, and an
/// invitation's expiry -- each the answer to a question the confirm pass or
/// the SHARING card asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The org-user id (`ouId`) the membership routes take. On NodeWarden's
    /// older `/members` rows it is the account id, which is what NodeWarden's
    /// membership routes take.
    pub id: String,
    /// The account behind the membership; `None` for an invitation.
    pub user_id: Option<String>,
    pub email: String,
    pub name: Option<String>,
    /// Lower-cased: `owner`, `admin`, `manager`, `member`, `custom`.
    pub role: String,
    pub status: MemberStatus,
    /// The collection ids listed against this member.
    pub collections: Vec<String>,
    /// The subset of `collections` held read-only.
    pub read_only: Vec<String>,
    /// NodeWarden's `invitationExpirationDate`, for an invitation.
    pub invitation_expires: Option<String>,
}

impl Member {
    /// Holds the org key: Bitwarden's status 2.
    #[must_use]
    pub fn confirmed(&self) -> bool {
        self.status == MemberStatus::Confirmed
    }

    /// Listed against `collection_id`.
    #[must_use]
    pub fn has_access(&self, collection_id: &str) -> bool {
        self.collections.iter().any(|c| c == collection_id)
    }

    /// See the original doc: an owner or an admin reaches every collection.
    #[must_use]
    pub fn reaches_everything(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "admin")
    }

    /// Whether this member can see an item filed in `collections`.
    #[must_use]
    pub fn reaches(&self, collections: &[&str]) -> bool {
        self.reaches_everything()
            || collections.iter().any(|id| self.collections.iter().any(|mine| mine == id))
    }
}
```

Keep the original doc paragraphs of `reaches_everything` and `reaches` in full. The `See the original doc` line above is only a pointer for this plan; replace it with those paragraphs.

(d) `Audience`: add two variants after `Shared { .. }`, and extend `is_shared`:

```rust
    /// In this account's own sharing org: the people it is shared with, by
    /// their person collections' names. Never names the organisation.
    SharedWithPeople { people: Vec<String> },
    /// In someone else's sharing org: who shared it.
    SharedWithMe { from: String },
```

```rust
    pub fn is_shared(&self) -> bool {
        !matches!(self, Audience::Personal)
    }
```

(e) `Directory` gains a field `waiting: Vec<Organisation>` (doc: `/// Accepted-but-unconfirmed sharing orgs from \`profile.organizationsNew\` -- the recipient's "waiting for X to finish sharing".`). Add `waiting: Vec::new()` to `NOTHING_SHARED`. Then add these methods to `impl Directory`:

```rust
    /// The organisations an organisations surface may show: every one but the
    /// sharing orgs, which are plumbing (spec 6.9).
    #[must_use]
    pub fn visible_organisations(&self) -> Vec<&Organisation> {
        self.organisations.iter().filter(|o| o.sharing.is_none()).collect()
    }

    /// Every sharing organisation this account is in, its own and others'.
    #[must_use]
    pub fn sharing_orgs(&self) -> Vec<&Organisation> {
        self.organisations.iter().filter(|o| o.sharing.is_some()).collect()
    }

    /// This account's own sharing org: the first in server order, which on
    /// NodeWarden is creation order. The sharing engine prefers the id it
    /// cached (`sharing_state.json`) over this.
    #[must_use]
    pub fn my_sharing_org(&self) -> Option<&Organisation> {
        self.organisations.iter().find(|o| o.sharing.as_ref().is_some_and(|s| s.mine))
    }

    /// Sharing orgs this account has accepted and the sharer has not confirmed.
    #[must_use]
    pub fn waiting(&self) -> &[Organisation] {
        &self.waiting
    }

    /// The person collection for `email` in `org`, by decrypted name, case
    /// folded. Where two decrypt to one email, the first in server order.
    #[must_use]
    pub fn collection_named(&self, org: &str, email: &str) -> Option<&Collection> {
        let email = normalise_email(email);
        self.collections
            .iter()
            .find(|c| c.organization_id == org && c.name.as_deref().map(normalise_email) == Some(email.clone()))
    }
```

In `audience_of`, after the `organisation` label is computed, return early for sharing orgs:

```rust
        if let Some(sharing) = self.organisations.iter().find(|o| o.id == org_id).and_then(|o| o.sharing.as_ref()) {
            return if sharing.mine {
                Audience::SharedWithPeople {
                    people: filed_in
                        .iter()
                        .filter_map(|id| self.collections.iter().find(|c| c.id == *id))
                        .filter_map(|c| c.name.clone())
                        .collect(),
                }
            } else {
                Audience::SharedWithMe { from: sharing.sharer.clone() }
            };
        }
```

In the `others` count, change `.filter(|m| m.confirmed && m.reaches(&filed_in))` to `.filter(|m| m.status != MemberStatus::Invited && m.reaches(&filed_in))`, and add this comment: `// Anyone past the invitation counts as a viewer -- the rule this count always had (status "not zero"); a member still waiting for the key is about to see it.`

(f) `from_sync`: build the organisations with `sharing`, and fill `waiting`:

```rust
        let account_email = response.profile.as_ref().and_then(|p| p.email.clone()).unwrap_or_default();
        let mark = |org: &crate::rest::sync::Organization| {
            org.name.as_deref().and_then(sharer_of).map(|sharer| SharingOrg {
                sharer: sharer.to_string(),
                mine: org.is_owner() && !account_email.is_empty() && sharer_is(sharer, &account_email),
            })
        };
```

In the `.map(|org| Organisation { … })` closure, add `sharing: mark(org),`. After `organisations` is built, add:

```rust
        let waiting: Vec<Organisation> = response
            .profile
            .as_ref()
            .map(|profile| {
                profile
                    .organizations_new
                    .iter()
                    .filter(|org| !profile.organizations.iter().any(|confirmed| confirmed.id == org.id))
                    .filter_map(|org| {
                        let sharing = mark(org)?;
                        (!sharing.mine).then(|| Organisation {
                            id: org.id.clone(),
                            name: org.name.clone(),
                            enabled: org.enabled.unwrap_or(true),
                            sharing: Some(sharing),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
```

End with `Self { organisations, collections, rosters, waiting }`.

(g) `load`: skip rosters of other people's sharing orgs. Replace the loop header with:

```rust
        // **Not other people's sharing orgs.** A recipient is a plain member,
        // and NodeWarden answers a plain member's roster request with 404 on
        // both routes -- two requests per org per sync for nothing. Their
        // rosters are also none of this account's business (spec 10.2).
        let wanted: Vec<String> = directory
            .organisations
            .iter()
            .filter(|o| o.sharing.as_ref().is_none_or(|s| s.mine))
            .map(|o| o.id.clone())
            .collect();
        for id in wanted {
```

(h) `map_member`: replace the whole function body with:

```rust
fn map_member(raw: &Value) -> Option<Member> {
    let object = raw.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?.to_string();
    // **Two row shapes, one reader.** `/users` rows (official, NodeWarden from
    // slice 1) name the account in `userId` and say 0/1/2 in `status`.
    // NodeWarden's older `/members` rows ARE the account -- `id` is the user
    // id -- and say "accepted" as status 1 with `encryptedOrgKey: null`, which
    // the old reader counted as confirmed. That was right for counting viewers
    // and wrong for confirming (spec 3.2).
    let legacy = object.contains_key("encryptedOrgKey");
    let status = match object.get("status").and_then(Value::as_i64) {
        // A server that sent no status is read as confirmed: the row is in the
        // roster, and discounting it would under-count who can see an item.
        None => MemberStatus::Confirmed,
        Some(0) => MemberStatus::Invited,
        Some(1) if legacy => {
            if object.get("encryptedOrgKey").and_then(Value::as_str).is_some_and(|k| !k.is_empty()) {
                MemberStatus::Confirmed
            } else {
                MemberStatus::Accepted
            }
        }
        Some(1) => MemberStatus::Accepted,
        Some(_) => MemberStatus::Confirmed,
    };
    let user_id = match object.get("userId") {
        Some(value) => value.as_str().filter(|s| !s.is_empty()).map(str::to_string),
        None if legacy => Some(id.clone()),
        None => None,
    };
    // NodeWarden writes the role as a word and a constant `type: 0` beside it;
    // official rows carry only the number. The word wins.
    let role = match object.get("role").and_then(Value::as_str) {
        Some(role) => role.to_ascii_lowercase(),
        None => match object.get("type").and_then(Value::as_i64) {
            Some(0) => "owner",
            Some(1) => "admin",
            Some(3) => "manager",
            Some(4) => "custom",
            _ => "member",
        }
        .to_string(),
    };
    let mut collections = Vec::new();
    let mut read_only = Vec::new();
    for entry in object.get("collections").and_then(Value::as_array).into_iter().flatten() {
        // Bare id strings (NodeWarden's older rows) or `{id, readOnly, ..}`.
        let Some(cid) = entry.as_str().or_else(|| entry.get("id").and_then(Value::as_str)).filter(|id| !id.is_empty())
        else {
            continue;
        };
        if entry.get("readOnly").and_then(Value::as_bool) == Some(true) {
            read_only.push(cid.to_string());
        }
        collections.push(cid.to_string());
    }
    Some(Member {
        id,
        user_id,
        email: object.get("email").and_then(Value::as_str).unwrap_or_default().to_string(),
        name: object.get("name").and_then(Value::as_str).map(str::to_string),
        role,
        status,
        collections,
        read_only,
        invitation_expires: object
            .get("invitationExpirationDate")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}
```

(i) The wire:
- `WireDirectory` gains `#[serde(default)] waiting: Vec<WireOrganisation>`.
- `WireOrganisation` gains `#[serde(default)] sharer: Option<String>` and `#[serde(default)] mine: bool`.
- `WireMember` drops `confirmed` and gains these fields, all `#[serde(default)]` except `status`:
  - `user_id: Option<String>`
  - `status: MemberStatus`, with `#[serde(default = "confirmed_status")]` and the helper `fn confirmed_status() -> MemberStatus { MemberStatus::Confirmed }`
  - `read_only: Vec<String>`
  - `invitation_expires: Option<String>`

Update `to_body`'s and `from_body`'s field-by-field copies to carry every new field, and add the `waiting` list both ways. The organisation mapping writes `sharer: o.sharing.as_ref().map(|s| s.sharer.clone()), mine: o.sharing.as_ref().is_some_and(|s| s.mine)` and reads back `sharing: o.sharer.map(|sharer| SharingOrg { sharer, mine: o.mine })`.

- [ ] **Step 4: The roster route in `api.rs`, `owner_line`, the rail**

`api.rs` `fetch_organization_members`: replace the body with:

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

`detail_edit.rs` `owner_line`: add two arms to the `match audience`:

```rust
        // A record in this account's own sharing org is still the owner's own:
        // the org is plumbing and is never named (spec 6.9).
        Audience::SharedWithPeople { people } => match people.len() {
            0 => OWNER_PERSONAL.to_string(),
            1 => format!("{OWNER_PERSONAL} \u{b7} shared with {}", people[0]),
            n => format!("{OWNER_PERSONAL} \u{b7} shared with {n} people"),
        },
        Audience::SharedWithMe { from } => format!("Shared by {from}"),
```

`sidebar.rs` `draw_organisations`:
- Replace `if lists.sharing.is_empty() { return; }` with `let organisations = lists.sharing.visible_organisations(); if organisations.is_empty() { return; }`.
- Replace `for organisation in lists.sharing.organisations()` with `for organisation in organisations`.
- Add this comment: `// Sharing orgs never reach this rail: they are plumbing (spec 6.9), and their records are under Sent and Shared with me.`

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/organizations.rs src/rest/api.rs src/vault_window/detail_edit.rs src/vault_window/sidebar.rs
cargo test --lib -j 2 rest::organizations && cargo test --lib -j 2 vault_window::sidebar && cargo test --lib -j 2 vault_window::detail_edit && cargo test --lib -j 2 daemon_link
```

Expected:
- Every file reports `0`.
- All pass. The existing `daemon_link` test at line ~471 builds a directory `from_body` with the old `confirmed` field. `serde` ignores unknown fields and the new fields default, so it still reads back. If it fails, add `#[serde(default)]` where the failure names the field; do not change the test's intent.

- [ ] **Step 6: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/organizations.rs deskwarden/src/rest/api.rs deskwarden/src/vault_window/detail_edit.rs deskwarden/src/vault_window/sidebar.rs
git commit -m "Rosters say invited, accepted or confirmed, and sharing orgs stay hidden

A roster is read from /users?includeCollections=true and falls back to
NodeWarden's /members on 404/405/501. A member's status is three-valued:
a NodeWarden status 1 with no key is accepted, not confirmed. The confirm
pass needs exactly that difference; the viewer count keeps its old rule.

Organisations named 'Shared by <email>' are recognised as sharing orgs,
mine when owned and naming this account. They are left out of
visible_organisations (the rail) and answer audience_of as
SharedWithPeople or SharedWithMe. Accepted-but-unconfirmed ones come
from profile.organizationsNew. Other people's sharing-org rosters are no
longer fetched.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 6: `sharing_state.json`

**Files:**
- Create: `deskwarden/src/sharing_state.rs`
- Modify: `deskwarden/src/lib.rs` (declare the module after `pub mod sent_history;`, line 113)
- Modify: `deskwarden/src/foreground.rs`: `OPENS_NO_WINDOW` (line 1423), from `[&str; 86]` to `[&str; 87]`, and add an entry after `"sent_history",`

**Interfaces:**
- Produces (`crate::sharing_state`):
  - `pub const SHARING_STATE_FILE_NAME: &str = "sharing_state.json"`
  - `pub fn default_path() -> Option<PathBuf>`
  - `pub struct SharingState { pub accounts: BTreeMap<String, AccountSharing> }`
  - `pub struct AccountSharing { pub sharing_org_id: Option<String>, pub pins: BTreeMap<String, Pin>, pub moves: BTreeMap<String, Move>, pub unfinished: Vec<Unfinished> }`
  - `pub struct Pin { pub sha256: String, pub user_id: String, pub at: i64 }`
  - `pub struct Move { pub folder_id: Option<String>, pub favorite: bool }`
  - `pub enum Unfinished { PurgeOrgCopy { org_cipher_id: String, personal_id: String } }`
  - `SharingState::load(&Path) -> Self`, `persist(&self, &Path) -> io::Result<()>`, `account(&self, user_id) -> AccountSharing`, `account_mut(&mut self, user_id) -> &mut AccountSharing`
  - `AccountSharing::pin_for(&self, email) -> Option<&Pin>`, `set_pin(&mut self, email, Pin)`, `forget_person(&mut self, email)`
  - `pub fn update(path: &Path, change: impl FnOnce(&mut SharingState)) -> io::Result<SharingState>`

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/sharing_state.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> (crate::test_scratch::ScratchDir, PathBuf) {
        let dir = crate::test_scratch::ScratchDir::new(&format!("sharing-state-{tag}"));
        let path = dir.join("sharing_state.json");
        (dir, path)
    }

    /// Round trip, per account, and the on-disk shape spec 5.4 gives for an
    /// unfinished move.
    #[test]
    fn the_file_round_trips_per_account_and_forgives_a_broken_one() {
        let (_dir, path) = temp_path("roundtrip");
        assert_eq!(SharingState::load(&path), SharingState::default(), "a missing file is empty");
        let written = update(&path, |state| {
            let me = state.account_mut("user-1");
            me.sharing_org_id = Some("org1".into());
            me.set_pin(" Anna@Example.com ", Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 7 });
            me.moves.insert("c1".into(), Move { folder_id: Some("f1".into()), favorite: true });
            me.unfinished.push(Unfinished::PurgeOrgCopy { org_cipher_id: "c1".into(), personal_id: "p1".into() });
            state.account_mut("user-2").sharing_org_id = Some("org2".into());
        })
        .expect("writes");
        let read = SharingState::load(&path);
        assert_eq!(read, written);
        assert_eq!(read.account("user-1").pin_for("anna@example.com").map(|p| p.user_id.as_str()), Some("anna-id"));
        assert_eq!(read.account("user-2").sharing_org_id.as_deref(), Some("org2"));
        assert_eq!(read.account("nobody"), AccountSharing::default());
        let raw = std::fs::read_to_string(&path).expect("reads");
        assert!(raw.contains("\"kind\": \"purgeOrgCopy\"") && raw.contains("\"orgCipherId\": \"c1\""), "{raw}");
        std::fs::write(&path, "{ not json").expect("writes");
        assert_eq!(SharingState::load(&path), SharingState::default());
    }

    /// Forget removes a person's pin (spec 12.4: "forget removes a person from
    /// both" files).
    #[test]
    fn forgetting_a_person_drops_their_pin() {
        let mut me = AccountSharing::default();
        me.set_pin("anna@example.com", Pin { sha256: "ab".into(), user_id: "a".into(), at: 1 });
        me.forget_person("ANNA@example.com");
        assert!(me.pin_for("anna@example.com").is_none());
    }

    /// **No secret is a field here** (spec 12.4): emails, ids and key hashes.
    #[test]
    fn nothing_in_the_file_can_open_anything() {
        let source = include_str!("sharing_state.rs");
        let production = source.split(concat!("#[cfg(", "test)]")).next().expect("a production half");
        for banned in ["pub key:", "pub token", "pub password", "pub secret", "Zeroizing"] {
            assert!(!production.contains(banned), "sharing_state gained `{banned}`");
        }
    }
}
```

In `src/lib.rs`, after `pub mod sent_history;`, add:

```rust
/// `sharing_state.json`: what personal sharing keeps on this PC per account
/// -- the sharing org's id, the public-key pins that auto-confirm trusts on
/// first use, where a shared record was before it left the personal vault,
/// and the second half of a move that failed. Emails, ids and hashes; no
/// secret.
pub mod sharing_state;
```

In `src/foreground.rs`, change `const OPENS_NO_WINDOW: [&str; 86] = [` to `[&str; 87]` and add after `"sent_history",`:

```rust
            // `sharing_state.json`: pins, moves and the sharing org's id,
            // read and written through `serde_json`. It touches one file and
            // paints nothing.
            "sharing_state",
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 sharing_state`
Expected: a compile error. `SharingState`, `update`, `Pin` and the other new names are not found.

- [ ] **Step 3: Implement**

Insert above the test module:

```rust
//! **`sharing_state.json`**: what personal sharing has to remember on this
//! PC, and nothing it could read from the server instead.
//!
//! Spec 5.4 lists it. Everything else about sharing -- who is invited, who is
//! confirmed, which records are in which collections -- is read from the
//! server on every pass, so two PCs never disagree about a share. What cannot
//! be read back from anywhere is:
//!
//! * **Pins**: the SHA-256 of a person's public key, the first time this
//!   account confirmed them. Auto-confirm trusts a key on first use and
//!   refuses a CHANGED one (spec 11.5, 12.1); the first key has to be
//!   remembered somewhere the server cannot rewrite.
//! * **Moves**: the folder and star a record had before it left the personal
//!   vault. An organisation cipher carries neither on NodeWarden; they are put
//!   back when the record comes home.
//! * **Unfinished**: the second half of a move back that failed (spec 13).
//! * **The sharing org's id**, so a rename in the web vault does not strand
//!   the shares (spec 5.1).
//!
//! # Keyed by account
//!
//! One file for every account on this PC, each account's state under its
//! user id, so a pin taken while signed in as one account is never trusted
//! for another.
//!
//! # No secret
//!
//! Emails, ids, hashes and two booleans. `nothing_in_the_file_can_open_anything`
//! holds it there.
//!
//! # Atomic
//!
//! Written to a `.tmp` sibling and renamed over the file, in
//! `service_keys::save`'s idiom: a torn pin file is a person whose key reads
//! as changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name, inside [`crate::settings::config_dir`].
pub const SHARING_STATE_FILE_NAME: &str = "sharing_state.json";

/// Where the file lives, or `None` with no config directory.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join(SHARING_STATE_FILE_NAME))
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingState {
    /// Each account's state, by user id.
    pub accounts: BTreeMap<String, AccountSharing>,
}

/// One account's sharing state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountSharing {
    /// The account's own sharing organisation, once chosen.
    pub sharing_org_id: Option<String>,
    /// Public-key pins, by normalised email.
    pub pins: BTreeMap<String, Pin>,
    /// Where a shared record was, by its organisation cipher id.
    pub moves: BTreeMap<String, Move>,
    /// Moves back whose second half failed, retried on every pass.
    pub unfinished: Vec<Unfinished>,
}

/// A person's public key, as first confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pin {
    /// Lowercase hex SHA-256 of the SPKI DER.
    pub sha256: String,
    /// The account the key belonged to.
    pub user_id: String,
    /// When it was pinned, UTC milliseconds.
    pub at: i64,
}

/// Where a record was before it was shared.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Move {
    pub folder_id: Option<String>,
    pub favorite: bool,
}

/// The second half of a move that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Unfinished {
    /// The personal copy was made and the organisation copy was not deleted.
    /// The org copy is filed in no collection, so nobody but the owner sees it.
    #[serde(rename_all = "camelCase")]
    PurgeOrgCopy { org_cipher_id: String, personal_id: String },
}

impl SharingState {
    /// Reads the file. Absent, empty or unparseable is empty, as
    /// `sent_history` reads its own.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        if text.trim().is_empty() {
            return Self::default();
        }
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Writes the whole file atomically: a `.tmp` sibling, then a rename.
    ///
    /// # Errors
    ///
    /// The directory could not be made, or the write or rename failed.
    pub fn persist(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?)?;
        if let Err(e) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(e);
        }
        Ok(())
    }

    /// One account's state, or an empty one.
    #[must_use]
    pub fn account(&self, user_id: &str) -> AccountSharing {
        self.accounts.get(user_id).cloned().unwrap_or_default()
    }

    /// One account's state, made if absent.
    pub fn account_mut(&mut self, user_id: &str) -> &mut AccountSharing {
        self.accounts.entry(user_id.to_string()).or_default()
    }
}

impl AccountSharing {
    /// The pin for a person, by email in any case.
    #[must_use]
    pub fn pin_for(&self, email: &str) -> Option<&Pin> {
        self.pins.get(&crate::rest::organizations::normalise_email(email))
    }

    /// Pins (or re-pins, after "Share anyway") a person's key.
    pub fn set_pin(&mut self, email: &str, pin: Pin) {
        self.pins.insert(crate::rest::organizations::normalise_email(email), pin);
    }

    /// Forgets everything kept about a person.
    pub fn forget_person(&mut self, email: &str) {
        self.pins.remove(&crate::rest::organizations::normalise_email(email));
    }
}

/// Read, change, write -- `sent_history::update`'s shape, for its reason:
/// another process may have written in between. Returns the state as written.
///
/// # Errors
///
/// See [`SharingState::persist`].
pub fn update(path: &Path, change: impl FnOnce(&mut SharingState)) -> std::io::Result<SharingState> {
    let mut state = SharingState::load(path);
    change(&mut state);
    state.persist(path)?;
    Ok(state)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" --fix src/sharing_state.rs
python "$TEMP/dw_crlf.py" src/lib.rs src/foreground.rs
cargo test --lib -j 2 sharing_state && cargo test --lib -j 2 foreground::
```
Expected: every file reports `0`. All pass, including `every_module_in_this_crate_is_classified_as_opening_windows_or_not`.

- [ ] **Step 5: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/sharing_state.rs deskwarden/src/lib.rs deskwarden/src/foreground.rs
git commit -m "Sharing keeps its pins, moves and org id in sharing_state.json

The one file personal sharing needs on the PC: the public-key pins
auto-confirm trusts on first use, where a shared record was before it
left the personal vault, the second half of a failed move back, and the
sharing org's id. Keyed by account, written atomically, and holding no
secret, which a test reads off the source. foreground's OPENS_NO_WINDOW
grows to 87 for the new module.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The sharing engine: probe, held state and the view, inside `RestBackend`

**Files:**
- Create: `deskwarden/src/rest/sharing.rs`
- Modify: `deskwarden/src/rest/mod.rs` (declare it)
- Modify: `deskwarden/src/rest/backend.rs`
  - `keys` field type (line 273), `new` (377-406), a `sharing` field;
  - `synced` (lines 635-640);
  - `rest_error` and `crypto_error` become `pub(crate)` (lines 1636, 1697);
  - a new trait method impl;
  - test fixtures in `pub mod tests`.
- Modify: `deskwarden/src/vault_backend.rs`: the trait (line 169) and the seven impls (lines 185, 350, 448, 641, 737, 852, 1050)
- Modify: `deskwarden/src/daemon_link.rs`: `impl VaultBackend for DaemonBackend` (line 286)

**Interfaces:**
- Consumes:
  - `rest::sharing_api::{my_invitations, user_public_key, IncomingInvitation}`;
  - `rest::organizations::{Directory, Member, MemberStatus, normalise_email, sharer_of, organization_of, collection_ids_of}`;
  - `rest::sync::{Profile, VaultKeys}`;
  - `sharing_state`.
- Produces (`crate::rest::sharing`):
  - `pub const REASON_BW_SERVE: &str = "bw-serve"`
  - `pub const REASON_NOT_NODEWARDEN: &str = "not-nodewarden"`
  - `pub const REASON_TOO_OLD: &str = "server-too-old"`
  - `pub const REASON_NOT_LOADED: &str = "not-loaded"`
  - `pub enum PersonState { Shared, WaitingAccept, WaitingAccount, WaitingSignIn, Expired, KeyChanged, Gone }` (serde camelCase)
  - `pub struct SharingView { available, reason, dialect, roster_visible_to_recipients, can_mint_invites, people, records, received, incoming, waiting }`, with methods `unavailable(reason: &str) -> Self`, `person(&self, email) -> Option<&PersonEntry>`, `people_of(&self, item_id) -> Vec<&PersonEntry>`, `shared_with_someone(&self, item_id) -> bool`, `received_from(&self, item_id) -> Option<&str>`
  - `pub struct PersonEntry { pub email: String, pub state: PersonState, pub expires_at: Option<String> }`
  - `pub struct RecordEntry { pub item_id: String, pub people: Vec<String> }`
  - `pub struct ReceivedEntry { pub item_id: String, pub from: String }`
  - `pub struct IncomingEntry { pub id: String, pub from: String, pub expires_at: Option<String> }`
  - `pub struct WaitingEntry { pub organization_id: String, pub from: String }`
  - `pub enum Trigger { WholeRead, Batch, Timer, Own }`
  - `pub enum SharingError { Unavailable(&'static str), Refused(String), Vault(VaultError) }`
  - `pub struct SharingOutcome { pub upserted: Vec<VaultItem>, pub removed: Vec<String> }`
  - `pub struct SharingPass { pub changed: bool, pub outcome: SharingOutcome }`
  - `pub type KeyPairMaker = fn(&SymmetricKey) -> Result<(String, EncString), CryptoError>`
  - `pub struct SharingSetup { pub state_path: Option<PathBuf>, pub sent_path: Option<PathBuf>, pub fill_stats_path: Option<PathBuf>, pub make_key_pair: KeyPairMaker }`, with `SharingSetup::production()`
  - `pub struct Sharing`, with:
    - `new(client: RestClient, state: Weak<Mutex<Authenticated>>, keys: Arc<Mutex<Option<Arc<VaultKeys>>>>) -> Self`
    - `set_up(&self, SharingSetup)`
    - `note_directory(&self, &Directory, Option<&Profile>)`
    - `view(&self, items: &[VaultItem]) -> SharingView`
    - `refresh(&self, Trigger) -> Result<SharingPass, SharingError>`
    - `wants_timer(&self) -> bool`
  - `VaultBackend::sharing(&self) -> Option<Arc<crate::rest::sharing::Sharing>>`
  - `RestBackend::sharing_engine(&self) -> Arc<Sharing>`
  - Backend test fixtures (`crate::rest::backend::tests`):
    - `pub const FIXTURE_EMAIL: &str`, `pub const FIXTURE_USER_ID: &str = "user-1"`, `pub const FIXTURE_ORG: &str = "org1"`
    - `pub fn sharing_profile(organizations: serde_json::Value) -> serde_json::Value`
    - `pub fn sharing_org_row() -> serde_json::Value`
    - `pub fn personal_cipher(id: &str, name: &str) -> serde_json::Value`
    - `pub fn shared_cipher(id: &str, name: &str, collections: &[&str]) -> serde_json::Value`
    - `pub fn fixture_org_key() -> SymmetricKey`
    - `pub fn fixture_key_pair(org: &SymmetricKey) -> Result<(String, EncString), CryptoError>`
    - `pub fn person_collection(id: &str, email: &str) -> serde_json::Value`
    - `pub fn roster_row(ou: &str, user_id: Option<&str>, email: &str, status: i64, collections: &[&str]) -> serde_json::Value`

- [ ] **Step 1: Add the backend test fixtures**

Append to `pub mod tests` in `src/rest/backend.rs` (the fixtures only, with no test yet):

```rust
    // ---- personal-sharing fixtures, shared with `rest::sharing`'s tests ----

    pub const FIXTURE_EMAIL: &str = EMAIL;
    pub const FIXTURE_USER_ID: &str = "user-1";
    pub const FIXTURE_ORG: &str = "org1";

    /// The fixture account's profile as `GET /api/accounts/profile` answers it
    /// for sharing: its id and email, its user key, and its private key --
    /// the OpenSSL fixture key, whose org-key ciphertext `sharing_org_row`
    /// carries.
    pub fn sharing_profile(organizations: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "object": "profile",
            "id": FIXTURE_USER_ID,
            "email": EMAIL,
            "role": "user",
            "key": protected_user_key(),
            "privateKey": seal(
                &user_key(),
                &crate::rest::crypto::tests::hex(crate::rest::crypto::tests::ORG_KEY_PRIVATE_PKCS8_DER)
            ),
            "organizations": organizations.clone(),
            "organizationsNew": organizations,
        })
    }

    /// The fixture account's own sharing org: owned, named for the fixture
    /// email, its key the bytes 0..63 that OpenSSL wrapped to the fixture key.
    pub fn sharing_org_row() -> serde_json::Value {
        serde_json::json!({
            "id": FIXTURE_ORG,
            "name": format!("Shared by {EMAIL}"),
            "role": "owner",
            "type": 0,
            "status": 2,
            "enabled": true,
            "key": format!(
                "4.{}",
                crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::hex(
                    crate::rest::crypto::tests::ORG_KEY_WRAPPED_OAEP_SHA1
                ))
            )
        })
    }

    /// A personal cipher in the fixture vault's shape.
    pub fn personal_cipher(id: &str, name: &str) -> serde_json::Value {
        cipher(id, name, &serde_json::json!({}))
    }

    /// A cipher in the fixture sharing org, filed in `collections`.
    pub fn shared_cipher(id: &str, name: &str, collections: &[&str]) -> serde_json::Value {
        let key = org_key();
        serde_json::json!({
            "object": "cipherDetails",
            "id": id,
            "type": 1,
            "organizationId": FIXTURE_ORG,
            "collectionIds": collections,
            "key": null,
            "creationDate": "2024-01-01T00:00:00.000000Z",
            "revisionDate": "2024-02-02T00:00:00.000000Z",
            "deletedDate": null,
            "archivedDate": null,
            "favorite": false,
            "folderId": null,
            "reprompt": 0,
            "name": seal(&key, name.as_bytes()),
            "fields": [],
            "login": {
                "username": seal(&key, b"u@example.com"),
                "password": seal(&key, b"shared"),
                "uris": []
            }
        })
    }

    pub fn fixture_org_key() -> SymmetricKey {
        org_key()
    }

    /// An org key pair without generating one: the OpenSSL fixture key,
    /// its private half under `org`. RSA-2048 generation is seconds in a
    /// debug build and is tested once, in `crypto`.
    pub fn fixture_key_pair(
        org: &SymmetricKey,
    ) -> Result<(String, crate::rest::crypto::EncString), CryptoError> {
        let private = crate::rest::crypto::tests::hex(crate::rest::crypto::tests::ORG_KEY_PRIVATE_PKCS8_DER);
        let public = crate::rest::crypto::public_key_of(&private)?;
        Ok((crate::rest::crypto::tests::base64(&public), crate::rest::crypto::encrypt(org, &private)?))
    }

    /// A person collection of the fixture org: the email under the org key.
    pub fn person_collection(id: &str, email: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "organizationId": FIXTURE_ORG, "name": seal(&org_key(), email.as_bytes()), "object": "collection" })
    }

    /// One `/users?includeCollections=true` row; read-only on every collection.
    pub fn roster_row(ou: &str, user_id: Option<&str>, email: &str, status: i64, collections: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "object": "organizationUserUserDetails",
            "id": ou,
            "userId": user_id,
            "email": email,
            "type": if user_id == Some(FIXTURE_USER_ID) { 0 } else { 2 },
            "status": status,
            "collections": collections.iter().map(|c| serde_json::json!({ "id": c, "readOnly": true, "hidePasswords": false, "manage": false })).collect::<Vec<_>>(),
            "invitationExpirationDate": null
        })
    }
```

- [ ] **Step 2: Write the failing engine tests**

Create `deskwarden/src/rest/sharing.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::backend::tests::{
        FIXTURE_ORG, FIXTURE_USER_ID, fixture_key_pair, sharing_profile, signed_in_with_no_sync_route,
    };

    /// An engine with no server behind it, for the pure view.
    fn offline() -> Sharing {
        Sharing::new(crate::rest::api::RestClient::new("http://127.0.0.1:1"), Weak::new(), Arc::new(Mutex::new(None)))
    }

    fn member(email: &str, status: MemberStatus, collections: &[&str], expires: Option<&str>) -> Member {
        Member {
            id: format!("ou-{email}"),
            user_id: (status != MemberStatus::Invited).then(|| format!("u-{email}")),
            email: email.to_string(),
            name: None,
            role: "member".to_string(),
            status,
            collections: collections.iter().map(|c| (*c).to_string()).collect(),
            read_only: collections.iter().map(|c| (*c).to_string()).collect(),
            invitation_expires: expires.map(str::to_string),
        }
    }

    fn an_item(id: &str, org: &str, collections: &[&str]) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "organizationId": org, "collectionIds": collections
        }))
        .expect("an item")
    }

    /// **Every state of spec 6.1, read off the roster**, and every record's
    /// people read off its collections.
    #[test]
    fn every_person_state_is_read_off_the_roster_and_the_records_off_their_collections() {
        let engine = offline();
        {
            let mut held = engine.held();
            held.probe = Some(Probe { dialect: Dialect::NodeWarden, slice_one: true });
            held.account = Some(Account { id: "me".into(), email: "me@x.io".into(), admin: true });
            held.my_org = Some(MyOrg {
                id: "org1".into(),
                collections: ["anna", "bob", "carol", "dan", "eve", "fay", "gus", "hal"]
                    .iter()
                    .map(|n| (format!("c-{n}"), format!("{n}@x.io")))
                    .collect(),
                roster: vec![
                    member("anna@x.io", MemberStatus::Confirmed, &["c-anna"], None),
                    member("bob@x.io", MemberStatus::Accepted, &["c-bob"], None),
                    member("carol@x.io", MemberStatus::Invited, &["c-carol"], Some("2999-01-01T00:00:00Z")),
                    member("dan@x.io", MemberStatus::Invited, &["c-dan"], Some("2000-01-01T00:00:00Z")),
                    member("eve@x.io", MemberStatus::Accepted, &["c-eve"], None),
                    member("fay@x.io", MemberStatus::Accepted, &["c-fay"], None),
                    member("hal@x.io", MemberStatus::Confirmed, &[], None),
                ],
            });
            held.key_changed.insert("eve@x.io".into());
            held.no_key.insert("fay@x.io".into());
            held.others = vec![("org-anna".into(), "anna@x.io".into())];
        }
        let items = [an_item("r1", "org1", &["c-anna", "c-bob"]), an_item("r2", "org-anna", &[]), an_item("r3", "", &[])];
        let view = engine.view(&items);
        assert!(view.available && view.can_mint_invites);
        let state = |email: &str| view.person(email).map(|p| p.state);
        assert_eq!(state("anna@x.io"), Some(PersonState::Shared));
        assert_eq!(state("bob@x.io"), Some(PersonState::WaitingAccept));
        assert_eq!(state("carol@x.io"), Some(PersonState::WaitingAccount));
        assert_eq!(state("dan@x.io"), Some(PersonState::Expired));
        assert_eq!(state("eve@x.io"), Some(PersonState::KeyChanged));
        assert_eq!(state("fay@x.io"), Some(PersonState::WaitingSignIn));
        assert_eq!(state("gus@x.io"), Some(PersonState::Gone), "a collection and no member");
        assert_eq!(state("hal@x.io"), Some(PersonState::Gone), "a member without access to their collection");
        assert_eq!(view.records, vec![RecordEntry { item_id: "r1".into(), people: vec!["anna@x.io".into(), "bob@x.io".into()] }]);
        assert!(view.shared_with_someone("r1"), "anna is Shared, so the pill's promise holds");
        assert_eq!(view.received_from("r2"), Some("anna@x.io"));
        assert_eq!(view.received_from("r3"), None);
    }

    /// Before the probe has answered, sharing is "not loaded", never "on".
    #[test]
    fn an_engine_that_has_not_probed_says_not_loaded() {
        let view = offline().view(&[]);
        assert!(!view.available);
        assert_eq!(view.reason.as_deref(), Some(REASON_NOT_LOADED));
    }

    /// **The dialect probe is made once per session** (spec 7.1): the
    /// slice-one key route is asked once however many passes run. The
    /// invitation list doubles as the recipient's incoming list, so every
    /// whole read asks it again.
    #[test]
    fn the_dialect_probe_is_made_once_per_session() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect(1).create();
        let invitations = server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect(2).create();
        let key = server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "userId": FIXTURE_USER_ID, "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        engine.refresh(Trigger::Batch).expect("a batch pass asks nothing");
        invitations.assert();
        key.assert();
        assert!(engine.view(&[]).available);
        let _ = FIXTURE_ORG;
    }

    /// **An old NodeWarden without slice 1 turns sharing off** (spec 7.1):
    /// its own-key route answers 404, and without slice 1 a recipient could
    /// edit the sharer's records (spec 12.3).
    #[test]
    fn an_old_nodewarden_without_slice_one_turns_sharing_off() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).with_status(404).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        let view = engine.view(&[]);
        assert!(!view.available);
        assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD));
    }

    /// An official server has no invitation list: sharing is not offered.
    #[test]
    fn an_official_server_is_told_sharing_is_nodewarden_only() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_status(404).create();
        let key = server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        key.assert();
        assert_eq!(engine.view(&[]).reason.as_deref(), Some(REASON_NOT_NODEWARDEN));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing`
Expected: a compile error. `Sharing`, `SharingSetup`, `Probe` and the other engine names are not found.

- [ ] **Step 4: Write the engine's production half**

Insert above the test module in `src/rest/sharing.rs`:

```rust
//! **The sharing engine**: personal sharing on NodeWarden, run by the daemon.
//!
//! `docs/superpowers/specs/2026-09-27-personal-sharing-design.md` is the
//! design; its "Owner's decisions" block overrides the rest. In one line:
//! Share files a record into a per-person collection of a hidden organisation
//! the sharer owns, and the recipient gets it view-only.
//!
//! # Where it runs, and why
//!
//! Inside `rest::backend::RestBackend`, which holds the session, the vault
//! keys and -- through the profile -- the private key every step needs
//! (spec 11.2). `VaultBackend::sharing` hands it to the daemon's API as an
//! `Arc`; a `bw serve` backend has none, and says so by returning `None`.
//! It shares the backend's key cache rather than keeping a second one, and
//! holds the session by a WEAK handle, as the notifications hub does: an
//! engine cannot keep a switched-away account's session alive.
//!
//! # What it holds, and what it reads fresh
//!
//! It holds only what the last whole read already paid for (the sharing
//! org's roster and person collections, handed over by `synced` through
//! [`Sharing::note_directory`]), the dialect probe's answer, the recipient's
//! incoming invitations, and two sets the confirm pass learns (keys that
//! changed, accounts with no key yet). Every WRITE reads current server
//! state first and does only the missing steps (spec 13), so a Retry is the
//! same call and two PCs converge.
//!
//! # NodeWarden only, in this build
//!
//! The owner's decision: slices 2 and 3 target NodeWarden. The probe tells
//! the dialects apart (spec 7.1), and on anything but NodeWarden with the
//! slice-1 routes the view says `available: false` and every write refuses.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use serde::{Deserialize, Serialize};

use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::{CryptoError, EncString, SymmetricKey};
use crate::rest::organizations::{
    Directory, Member, MemberStatus, Roster, collection_ids_of, normalise_email, organization_of, sharer_of,
};
use crate::rest::sharing_api::{self, IncomingInvitation};
use crate::rest::sync::{Profile, VaultKeys};
use crate::vault_bridge::{VaultError, VaultItem};

// ---- what the daemon answers `GET /sharing` with --------------------------------

/// Why sharing is not available. Spec 11.3's reasons, plus `not-nodewarden`:
/// the owner's decision leaves official servers for a later slice, and the
/// spec's list had no word for that.
pub const REASON_BW_SERVE: &str = "bw-serve";
pub const REASON_NOT_NODEWARDEN: &str = "not-nodewarden";
pub const REASON_TOO_OLD: &str = "server-too-old";
pub const REASON_NOT_LOADED: &str = "not-loaded";

/// The one dialect this build shares on.
pub const DIALECT_NODEWARDEN: &str = "nodewarden";

/// Spec 6.1's per-person states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonState {
    /// Confirmed, with access to their collection: in their vault now.
    Shared,
    /// Accepted, confirm not run yet.
    WaitingAccept,
    /// NodeWarden: a pending invitation, no account for the email.
    WaitingAccount,
    /// Accepted, but the account has no public key yet.
    WaitingSignIn,
    /// The invitation's expiry has passed.
    Expired,
    /// Accepted, and the public key differs from the pin: not confirmed.
    KeyChanged,
    /// Their collection exists and no membership reaches it.
    Gone,
}

/// One person this account shares with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonEntry {
    pub email: String,
    pub state: PersonState,
    /// An invitation's expiry, ISO 8601.
    pub expires_at: Option<String>,
}

/// One record this account shares, and with whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub item_id: String,
    pub people: Vec<String>,
}

/// A record someone shares with this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedEntry {
    pub item_id: String,
    pub from: String,
}

/// An invitation addressed to this account, from a sharer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEntry {
    pub id: String,
    pub from: String,
    pub expires_at: Option<String>,
}

/// A sharing org this account accepted and its sharer has not confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingEntry {
    pub organization_id: String,
    pub from: String,
}

/// **Everything the window draws about sharing**, answered by `GET /sharing`
/// out of the engine's held state and the daemon's items -- no server round
/// trip (spec 11.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingView {
    pub available: bool,
    pub reason: Option<String>,
    pub dialect: Option<String>,
    /// Always false on NodeWarden (slice 1 makes rosters manager-only).
    pub roster_visible_to_recipients: bool,
    /// The account is NodeWarden's administrator and may mint invite codes.
    pub can_mint_invites: bool,
    pub people: Vec<PersonEntry>,
    pub records: Vec<RecordEntry>,
    pub received: Vec<ReceivedEntry>,
    pub incoming: Vec<IncomingEntry>,
    pub waiting: Vec<WaitingEntry>,
}

impl SharingView {
    /// Sharing is off, and why.
    #[must_use]
    pub fn unavailable(reason: &str) -> Self {
        Self { reason: Some(reason.to_string()), ..Self::default() }
    }

    /// One person, by email in any case.
    #[must_use]
    pub fn person(&self, email: &str) -> Option<&PersonEntry> {
        let email = normalise_email(email);
        self.people.iter().find(|p| p.email == email)
    }

    /// The people one record is shared with, in the record's order.
    #[must_use]
    pub fn people_of(&self, item_id: &str) -> Vec<&PersonEntry> {
        self.records
            .iter()
            .find(|r| r.item_id == item_id)
            .map(|r| r.people.iter().filter_map(|email| self.person(email)).collect())
            .unwrap_or_default()
    }

    /// **The pill's promise** (spec 6.1): at least one person has it now.
    #[must_use]
    pub fn shared_with_someone(&self, item_id: &str) -> bool {
        self.people_of(item_id).iter().any(|p| p.state == PersonState::Shared)
    }

    /// Who shared a received record.
    #[must_use]
    pub fn received_from(&self, item_id: &str) -> Option<&str> {
        self.received.iter().find(|r| r.item_id == item_id).map(|r| r.from.as_str())
    }
}

// ---- the engine's own types -----------------------------------------------------

/// What set off a pass (spec 11.5's triggers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A whole read: unlock, `POST /sync`, the hub's populate.
    WholeRead,
    /// An applied hub batch.
    Batch,
    /// The ten-minute timer.
    Timer,
    /// The engine's own write.
    Own,
}

/// Why a sharing call did not happen.
#[derive(Debug)]
pub enum SharingError {
    /// Sharing is not available here; the reason is one of the `REASON_*`s.
    Unavailable(&'static str),
    /// A step was refused, with the sentence to show.
    Refused(String),
    /// The vault backend's own failure (session gone, server unreachable).
    Vault(VaultError),
}

impl std::fmt::Display for SharingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "sharing is not available here ({reason})"),
            Self::Refused(sentence) => f.write_str(sentence),
            Self::Vault(e) => write!(f, "{e:?}"),
        }
    }
}

/// Items a sharing call changed on the server, for the daemon's cache.
#[derive(Debug, Clone, Default)]
pub struct SharingOutcome {
    /// Changed or new, as the server answered them, decrypted.
    pub upserted: Vec<VaultItem>,
    /// Ids that are gone.
    pub removed: Vec<String>,
}

impl SharingOutcome {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.removed.is_empty()
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default)]
pub struct SharingPass {
    /// The view may read differently now.
    pub changed: bool,
    /// Items the pass moved (a finished move back).
    pub outcome: SharingOutcome,
}

/// Makes an organisation's RSA pair. `crypto::generate_org_key_pair` in
/// production; a fixture pair in tests, because RSA-2048 generation takes
/// seconds in a debug build and every first-share test would pay it.
pub type KeyPairMaker = fn(&SymmetricKey) -> Result<(String, EncString), CryptoError>;

/// Where the engine's files are, and how it makes a key pair.
#[derive(Clone)]
pub struct SharingSetup {
    pub state_path: Option<PathBuf>,
    pub sent_path: Option<PathBuf>,
    pub fill_stats_path: Option<PathBuf>,
    pub make_key_pair: KeyPairMaker,
}

impl SharingSetup {
    /// The real files, in the config directory the daemon uses for all three.
    #[must_use]
    pub fn production() -> Self {
        Self {
            state_path: crate::sharing_state::default_path(),
            sent_path: crate::sent_history::default_path(),
            fill_stats_path: crate::settings::config_dir().map(|dir| dir.join("fill-stats.json")),
            make_key_pair: crate::rest::crypto::generate_org_key_pair,
        }
    }

    /// No files at all: nothing is persisted. What a test starts from, and
    /// it fills in scratch paths where the test is about a file.
    #[must_use]
    pub fn none() -> Self {
        Self {
            state_path: None,
            sent_path: None,
            fill_stats_path: None,
            make_key_pair: crate::rest::crypto::generate_org_key_pair,
        }
    }
}

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

/// The signed-in account, as the profile says.
#[derive(Debug, Clone, Default)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) email: String,
    pub(crate) admin: bool,
}

/// This account's own sharing org, as the last read left it.
#[derive(Debug, Clone)]
pub(crate) struct MyOrg {
    pub(crate) id: String,
    /// `(collection id, normalised email)`, in server order.
    pub(crate) collections: Vec<(String, String)>,
    pub(crate) roster: Vec<Member>,
}

impl MyOrg {
    fn collection_for(&self, email: &str) -> Option<&str> {
        let email = normalise_email(email);
        self.collections.iter().find(|(_, e)| *e == email).map(|(id, _)| id.as_str())
    }

    fn email_of(&self, collection_id: &str) -> Option<&str> {
        self.collections.iter().find(|(id, _)| id == collection_id).map(|(_, e)| e.as_str())
    }

    fn member(&self, email: &str) -> Option<&Member> {
        let email = normalise_email(email);
        self.roster.iter().find(|m| normalise_email(&m.email) == email)
    }
}

#[derive(Clone, Default)]
pub(crate) struct Held {
    pub(crate) probe: Option<Probe>,
    pub(crate) account: Option<Account>,
    pub(crate) my_org: Option<MyOrg>,
    /// Other people's sharing orgs: `(org id, sharer)`.
    pub(crate) others: Vec<(String, String)>,
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
    pub(crate) incoming: Vec<IncomingInvitation>,
    /// Emails whose key differs from the pin.
    pub(crate) key_changed: BTreeSet<String>,
    /// Emails whose account has no public key yet.
    pub(crate) no_key: BTreeSet<String>,
}

/// **The engine.** See the module docs.
pub struct Sharing {
    pub(crate) client: RestClient,
    state: Weak<Mutex<Authenticated>>,
    keys: Arc<Mutex<Option<Arc<VaultKeys>>>>,
    held: Mutex<Held>,
    setup: Mutex<SharingSetup>,
    /// One engine operation at a time: a share and a confirm pass must not
    /// interleave their reads and writes of the one org.
    ops: Mutex<()>,
}

impl Sharing {
    /// An engine over `RestBackend`'s client, session and key cache. Makes no
    /// request: `RestBackend::new`'s promise that constructing a backend
    /// touches no network holds.
    #[must_use]
    pub fn new(
        client: RestClient,
        state: Weak<Mutex<Authenticated>>,
        keys: Arc<Mutex<Option<Arc<VaultKeys>>>>,
    ) -> Self {
        Self {
            client,
            state,
            keys,
            held: Mutex::new(Held::default()),
            setup: Mutex::new(SharingSetup::production()),
            ops: Mutex::new(()),
        }
    }

    /// Where the files are. Production uses [`SharingSetup::production`]; a
    /// test points them into a scratch directory, or at nothing.
    pub fn set_up(&self, setup: SharingSetup) {
        *self.setup.lock().unwrap_or_else(PoisonError::into_inner) = setup;
    }

    pub(crate) fn setup(&self) -> SharingSetup {
        self.setup.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn one_at_a_time(&self) -> std::sync::MutexGuard<'_, ()> {
        self.ops.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One request, with the session locked for exactly its length -- the
    /// backend's own lock discipline. **Never called with `held` locked.**
    pub(crate) fn call<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        request(&self.client, &mut guard).map_err(|e| SharingError::Vault(crate::rest::backend::rest_error(e)))
    }

    /// `GET /api/accounts/profile`, and what it says about the account.
    pub(crate) fn profile(&self) -> Result<Profile, SharingError> {
        let profile = self.call(|client, auth| client.profile(&mut auth.session))?;
        self.note_account(&profile);
        Ok(profile)
    }

    fn note_account(&self, profile: &Profile) {
        if let (Some(id), Some(email)) = (profile.id.clone(), profile.email.clone()) {
            self.held().account = Some(Account { id, email, admin: profile.is_server_admin() });
        }
    }

    /// The vault keys, unwrapped from `profile` with the session's master key
    /// and stored in the cache the backend reads too.
    pub(crate) fn keys_from(&self, profile: &Profile) -> Result<Arc<VaultKeys>, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let (keys, _) = {
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            VaultKeys::unwrap_from(&guard.master_key, profile)
                .map_err(|e| SharingError::Vault(crate::rest::backend::crypto_error(e)))?
        };
        let keys = Arc::new(keys);
        *self.keys.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&keys));
        Ok(keys)
    }

    /// **What the last whole read already paid for**, handed over by
    /// `RestBackend::synced` before it files the directory: this account's
    /// sharing org, its person collections (names already decrypted) and its
    /// roster, other people's sharing orgs, and the accepted-but-waiting ones.
    /// The confirm pass reads its roster here, so a whole read costs it no
    /// roster request of its own (spec 11.5).
    pub fn note_directory(&self, directory: &Directory, profile: Option<&Profile>) {
        if let Some(profile) = profile {
            self.note_account(profile);
        }
        let cached = self.cached_org_id();
        let mine = cached
            .as_deref()
            .and_then(|id| directory.sharing_orgs().into_iter().find(|o| o.id == id && o.sharing.as_ref().is_some_and(|s| s.mine)))
            .or_else(|| directory.my_sharing_org());
        let mut held = self.held();
        // The previous roster, kept for an org whose roster this read could
        // not fetch: a known roster is better than none for the confirm pass.
        let previous = held.my_org.take();
        held.my_org = mine.map(|org| MyOrg {
            id: org.id.clone(),
            collections: directory
                .collections_of(&org.id)
                .into_iter()
                .filter_map(|c| c.name.as_deref().map(|name| (c.id.clone(), normalise_email(name))))
                .collect(),
            roster: match directory.roster_of(&org.id) {
                Roster::Known(members) => members.clone(),
                Roster::Unknown => previous.as_ref().filter(|o| o.id == org.id).map(|o| o.roster.clone()).unwrap_or_default(),
            },
        });
        held.others = directory
            .sharing_orgs()
            .into_iter()
            .filter_map(|o| o.sharing.as_ref().filter(|s| !s.mine).map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
        held.waiting = directory
            .waiting()
            .iter()
            .filter_map(|o| o.sharing.as_ref().map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
    }

    /// The org id `sharing_state.json` remembers for this account.
    pub(crate) fn cached_org_id(&self) -> Option<String> {
        let path = self.setup().state_path?;
        let account = self.held().account.as_ref().map(|a| a.id.clone())?;
        crate::sharing_state::SharingState::load(&path).account(&account).sharing_org_id
    }

    /// The view -- pure over what is held and the daemon's items.
    #[must_use]
    pub fn view(&self, items: &[VaultItem]) -> SharingView {
        let held = self.held().clone();
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
            ..SharingView::default()
        };
        if let Some(org) = &held.my_org {
            for (collection_id, email) in &org.collections {
                if view.people.iter().any(|p| &p.email == email) {
                    continue;
                }
                let member = org.member(email);
                view.people.push(PersonEntry {
                    email: email.clone(),
                    state: person_state(member, collection_id, &held, now),
                    expires_at: member.and_then(|m| m.invitation_expires.clone()),
                });
            }
            for item in items {
                if organization_of(item) != Some(org.id.as_str()) {
                    continue;
                }
                let mut people: Vec<String> = Vec::new();
                for id in collection_ids_of(item) {
                    if let Some(email) = org.email_of(id) {
                        if !people.iter().any(|p| p == email) {
                            people.push(email.to_string());
                        }
                    }
                }
                view.records.push(RecordEntry { item_id: item.id.clone(), people });
            }
        }
        for item in items {
            if let Some(org) = organization_of(item) {
                if let Some((_, sharer)) = held.others.iter().find(|(id, _)| id == org) {
                    view.received.push(ReceivedEntry { item_id: item.id.clone(), from: sharer.clone() });
                }
            }
        }
        view.incoming = held
            .incoming
            .iter()
            .filter_map(|invitation| {
                let from = invitation.organization_name.as_deref().and_then(sharer_of)?;
                Some(IncomingEntry { id: invitation.id.clone(), from: from.to_string(), expires_at: invitation.expires_at.clone() })
            })
            .collect();
        view.waiting = held
            .waiting
            .iter()
            .map(|(organization_id, from)| WaitingEntry { organization_id: organization_id.clone(), from: from.clone() })
            .collect();
        view
    }

    /// **The probe**, once per engine: `GET /invitations/mine` tells
    /// NodeWarden from the rest, and the own-key route tells NodeWarden with
    /// slice 1 from an older one (spec 7.1). A transport failure is not
    /// cached: the next pass asks again.
    pub(crate) fn ensure_probe(&self) -> Result<Probe, SharingError> {
        if let Some(probe) = self.held().probe {
            return Ok(probe);
        }
        let invitations = self.call(|client, auth| sharing_api::my_invitations(client, &mut auth.session))?;
        let probe = match invitations {
            None => Probe { dialect: Dialect::Official, slice_one: false },
            Some(list) => {
                self.held().incoming = list;
                let own = self.account_id()?;
                let key = self.call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &own))?;
                Probe { dialect: Dialect::NodeWarden, slice_one: key.is_some() }
            }
        };
        self.held().probe = Some(probe);
        Ok(probe)
    }

    /// Refuses unless this is NodeWarden with slice 1.
    pub(crate) fn require_available(&self) -> Result<(), SharingError> {
        let probe = self.ensure_probe()?;
        if probe.dialect != Dialect::NodeWarden {
            return Err(SharingError::Unavailable(REASON_NOT_NODEWARDEN));
        }
        if !probe.slice_one {
            return Err(SharingError::Unavailable(REASON_TOO_OLD));
        }
        Ok(())
    }

    /// The account's id, from the last profile, or read now.
    pub(crate) fn account_id(&self) -> Result<String, SharingError> {
        if let Some(account) = self.held().account.clone() {
            return Ok(account.id);
        }
        self.profile()?;
        self.held()
            .account
            .clone()
            .map(|a| a.id)
            .ok_or_else(|| SharingError::Refused("The server did not say which account this is.".to_string()))
    }

    /// **One pass** (spec 11.5): the probe, then the incoming invitations on a
    /// whole read or the timer. The confirm pass and unfinished moves join it
    /// in their own tasks.
    ///
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
            if let Some(list) = self.call(|client, auth| sharing_api::my_invitations(client, &mut auth.session))? {
                let mut held = self.held();
                let before: Vec<String> = held.incoming.iter().map(|i| i.id.clone()).collect();
                pass.changed |= before != list.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
                held.incoming = list;
            }
        }
        Ok(pass)
    }

    /// Whether the ten-minute timer has anything to do: someone in this
    /// account's org is invited or accepted and not yet confirmed.
    #[must_use]
    pub fn wants_timer(&self) -> bool {
        let held = self.held();
        held.my_org.as_ref().is_some_and(|org| {
            org.roster.iter().any(|m| m.status != MemberStatus::Confirmed && org.collection_for(&m.email).is_some())
        })
    }
}

/// Spec 6.1's table, as a function of the roster row, the collection and
/// what the confirm pass learned.
fn person_state(member: Option<&Member>, collection_id: &str, held: &Held, now: i64) -> PersonState {
    let Some(member) = member else {
        return PersonState::Gone;
    };
    let email = normalise_email(&member.email);
    match member.status {
        MemberStatus::Invited => {
            let expired = member
                .invitation_expires
                .as_deref()
                .and_then(crate::send::parse_iso_utc_millis)
                .is_some_and(|at| at < now);
            if expired { PersonState::Expired } else { PersonState::WaitingAccount }
        }
        MemberStatus::Accepted if held.key_changed.contains(&email) => PersonState::KeyChanged,
        MemberStatus::Accepted if held.no_key.contains(&email) => PersonState::WaitingSignIn,
        MemberStatus::Accepted => PersonState::WaitingAccept,
        MemberStatus::Confirmed if member.has_access(collection_id) => PersonState::Shared,
        MemberStatus::Confirmed => PersonState::Gone,
    }
}

/// Now, UTC milliseconds.
pub(crate) fn now_millis() -> i64 {
    use crate::send::SendClock;
    crate::send::SystemClock.now_unix_millis()
}
```

Add to `src/rest/mod.rs`, after `pub mod sharing_api;`:

```rust
/// **The personal-sharing engine**, run by the daemon inside `RestBackend`.
pub mod sharing;
```

- [ ] **Step 5: Wire the engine into `RestBackend` and the trait**

`src/rest/backend.rs`:
- Change the field `keys: Mutex<Option<Arc<VaultKeys>>>,` to `keys: Arc<Mutex<Option<Arc<VaultKeys>>>>,`. Add one line to its doc: `/// **An \`Arc\`**, since personal sharing: the engine shares this cache rather than keeping a second one (see \`rest::sharing\`).`
- After the `fetching` field, add:

```rust
    /// **The personal-sharing engine** (`rest::sharing`), built with this
    /// backend and dropped with it -- so an account switch, which is a new
    /// `RestBackend`, is a new engine with nothing held.
    sharing: Arc<crate::rest::sharing::Sharing>,
```

In `new`, replace the `Self { … }` literal with:

```rust
        let keys = Arc::new(Mutex::new(None));
        // The engine makes no request here either; see `Sharing::new`.
        let sharing = Arc::new(crate::rest::sharing::Sharing::new(
            client.clone(),
            Arc::downgrade(&state),
            Arc::clone(&keys),
        ));
        Self {
            client,
            state,
            keys,
            synced: Mutex::new(None),
            fetching: Mutex::new(()),
            sharing,
        }
```

Add an inherent method after `verify_stored_login`:

```rust
    /// The sharing engine, for tests and for `VaultBackend::sharing`.
    #[must_use]
    pub fn sharing_engine(&self) -> Arc<crate::rest::sharing::Sharing> {
        Arc::clone(&self.sharing)
    }
```

In `synced`, replace

```rust
        crate::rest::organizations::remember(crate::rest::organizations::Directory::load(
            &self.client,
            &mut state.session,
            &response,
            &keys,
        ));
```

with

```rust
        let directory = crate::rest::organizations::Directory::load(
            &self.client,
            &mut state.session,
            &response,
            &keys,
        );
        // The sharing engine takes what this read already paid for -- its
        // org's roster and person collections -- so its confirm pass asks the
        // server for no roster of its own (spec 11.5). It locks nothing of the
        // session's; see `Sharing::note_directory`.
        self.sharing.note_directory(&directory, response.profile.as_ref());
        crate::rest::organizations::remember(directory);
```

Change `fn rest_error(` and `fn crypto_error(` to `pub(crate) fn …`, and add to each doc: `/// \`pub(crate)\` for \`rest::sharing\`, which maps the same errors the same way.`

In `impl VaultBackend for RestBackend`, after `revision_of_last_read`, add:

```rust
    /// This backend's engine: sharing is a direct-REST feature (spec 7.4).
    fn sharing(&self) -> Option<Arc<crate::rest::sharing::Sharing>> {
        Some(Arc::clone(&self.sharing))
    }
```

`src/vault_backend.rs`: after `fn revision_of_last_read(&self) -> Option<i64>;` in the trait, add:

```rust
    /// **The personal-sharing engine**, where this backend has one.
    ///
    /// Only `rest::backend::RestBackend` does: sharing is a direct-REST
    /// feature and is not offered on `bw serve` (spec 7.4). Every forwarding
    /// impl forwards and every other says `None`, written out rather than
    /// defaulted, by this trait's rule.
    fn sharing(&self) -> Option<std::sync::Arc<crate::rest::sharing::Sharing>>;
```

Then add an impl to every implementor, directly after its `revision_of_last_read`:
- `VaultBridge`: `fn sharing(&self) -> Option<std::sync::Arc<crate::rest::sharing::Sharing>> { None }`, with the comment `// \`bw serve\` cannot invite, accept or make an org (spec 7.4).`
- `CachingBackend`: `{ self.inner.sharing() }`
- `Box<dyn VaultBackend>`: `{ (**self).sharing() }`
- `LateBoundBackend`: `{ self.read(|slot| slot.as_ref().and_then(|b| b.sharing())) }`
- `SharedLateBoundBackend`: `{ (**self).sharing() }`
- test `OneItem`, test `Counting`: `{ None }`

`src/daemon_link.rs` `impl VaultBackend for DaemonBackend`: add `fn sharing(&self) -> Option<std::sync::Arc<crate::rest::sharing::Sharing>> { None }`, with the comment `// A window's sharing goes through \`SharingClient\` to the daemon's engine; it has none of its own.`

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" --fix src/rest/sharing.rs
python "$TEMP/dw_crlf.py" src/rest/mod.rs src/rest/backend.rs src/vault_backend.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 rest::backend && cargo test --lib -j 2 vault_backend
```

Expected: every file reports `0`, and all pass.

`debug_leak_guard` may flag `Held` (it holds `IncomingInvitation`, which holds `Zeroizing`). `Held` derives only `Clone` and `Default`, so it must not be flagged. If it is, read the guard's message: it names the type that derives `Debug`.

- [ ] **Step 7: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/rest/mod.rs deskwarden/src/rest/backend.rs deskwarden/src/vault_backend.rs deskwarden/src/daemon_link.rs
git commit -m "The daemon's REST backend carries a sharing engine that knows where it stands

rest::sharing is the engine: it probes the dialect once per session
(invitations/mine, then the slice-1 own-key route), holds what the last
whole read already fetched about the sharing org (handed over by
synced, so no roster request of its own), and answers the view: people
and their spec 6.1 states, shared records, received records, incoming
invitations and waiting orgs. It shares RestBackend's key cache and
holds the session weakly.

VaultBackend gains sharing(), written out on every impl: RestBackend
answers its engine, bw serve and the daemon link answer None.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `GET /sharing`, the `sharing` feed kind, the cache hook, the triggers and the window's client

**Files:**
- Modify: `deskwarden/src/change_feed.rs` (`Change` L61, `WireChange` L256-270, both conversions L273-301, `page_from_body` L321, tests)
- Modify: `deskwarden/src/rest/notifications.rs` (`Inbox` L463, `impl Shared` L478, `HubListener` after `sends_changed` L612, `follow` L739)
- Modify: `deskwarden/src/vault_cache.rs` (new `apply_written_elsewhere` after `apply_announced_writing`, L1700)
- Modify: `deskwarden/src/vault_api.rs` (router L252-322, `sync` L342, new methods, tests)
- Modify: `deskwarden/src/vault_host.rs` (`hub_tick` L253-301, `HubWorker::start` L218, tests)
- Modify: `deskwarden/src/daemon_link.rs` (`SharingClient`, `DaemonLink::sharing`, `sharing_client`)

**Interfaces:**
- Consumes: `rest::sharing::{Sharing, SharingView, SharingOutcome, SharingPass, Trigger, SharingError, REASON_BW_SERVE, REASON_NOT_LOADED}`.
- Produces:
  - `change_feed::Change::Sharing`
  - `HubListener::sharing_changed(&self, now: Instant) -> bool`
  - `VaultCache::apply_written_elsewhere(&self, upserted: &[VaultItem], removed: &[String])`
  - `VaultApi::sharing_pass(&self, trigger: Trigger)`
  - `VaultApi::sharing_wants_timer(&self) -> bool`
  - private `VaultApi::engine(&self) -> Result<Arc<Sharing>, ApiResponse>`
  - private `VaultApi::publish_outcome(&self, outcome: &SharingOutcome)`
  - private `fn sharing_failed(error: &SharingError) -> ApiResponse`
  - `vault_host::SHARING_TIMER: Duration = 10 min`
  - `daemon_link::SharingClient` (`Clone`), with:
    - `new(base_url: &str, grant: &Zeroizing<String>) -> Self`
    - `view(&self) -> Result<SharingView, String>`
    - `pub(crate) fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String>`
  - `DaemonLink::sharing(&self) -> SharingClient`
  - `daemon_link::sharing_client() -> Option<SharingClient>`

- [ ] **Step 1: Write the failing tests**

`src/change_feed.rs`, append to `mod tests`:

```rust
    /// **A kind this build does not know is skipped, not fatal** (spec 11.4).
    /// A window and its daemon are one build today; a browser extension will
    /// not be, and one unknown kind must not fail a whole page -- three failed
    /// pages in a row close a window.
    #[test]
    fn a_page_with_an_unknown_kind_still_parses() {
        let page = page_from_body(
            r#"{"instance":1,"seq":3,"live":true,"reset":false,"changes":[{"kind":"somethingNew","x":1},{"kind":"sharing"},{"kind":"item","id":"a"}]}"#,
        )
        .expect("an unknown kind is skipped");
        assert_eq!(
            page.changes,
            vec![Change::Sharing, Change::Vault(Announcement::Item { id: "a".into(), revision: None })]
        );
    }

    #[test]
    fn a_sharing_change_is_not_a_vault_batch() {
        let feed = ChangeFeed::with_instance(1);
        feed.publish([Change::Sharing]);
        assert_eq!(feed.vault_batches(), 0, "the match engine is not rebuilt for a sharing change");
        let page = feed.wait_after(Some(0), false, Duration::ZERO);
        assert_eq!(page.changes, vec![Change::Sharing]);
    }
```

Extend the existing `a_page_survives_the_wire` test: add `Change::Sharing` to the list of kinds its page holds.

`src/vault_api.rs`, append to `mod tests`:

```rust
    /// **`GET /sharing` answers from the daemon's held view**, in the
    /// envelope, and a first pass is what turns it on.
    #[test]
    fn the_sharing_view_is_answered_from_the_daemon_and_a_pass_publishes_it() {
        let (mut server, cache, api, token) = a_daemon();
        cache.bridge().sharing().expect("a REST daemon has an engine").set_up(crate::rest::sharing::SharingSetup::none());
        let before = ask(&api, "GET", "/sharing", Some(&token), "");
        assert_eq!(before.status, 200);
        let body: serde_json::Value = serde_json::from_str(&before.body).expect("json");
        assert_eq!(body["data"]["available"], false);
        assert_eq!(body["data"]["reason"], crate::rest::sharing::REASON_NOT_LOADED);

        server.mock("GET", "/api/accounts/profile").with_body(crate::rest::backend::tests::sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        let page = api.feed().wait_after(Some(0), false, Duration::ZERO);
        assert!(page.changes.contains(&crate::change_feed::Change::Sharing), "{:?}", page.changes);
        let after: serde_json::Value = serde_json::from_str(&ask(&api, "GET", "/sharing", Some(&token), "").body).expect("json");
        assert_eq!(after["data"]["available"], true);
        assert_eq!(after["data"]["dialect"], "nodewarden");
    }

    /// **`bw serve` answers "unavailable" and makes no request** (spec 7.4).
    #[test]
    fn bw_serve_answers_sharing_unavailable_and_makes_no_request() {
        let mut server = crate::test_http::server();
        let nothing = server.mock("GET", crate::test_http::Matcher::Any).expect(0).create();
        let cache = Arc::new(VaultCache::new(crate::vault_bridge::VaultBridge::new(server.url())));
        assert_eq!(cache.populate_with(Vec::new(), cache.epoch()).expect("seeded"), PopulateOutcome::Populated);
        let api = VaultApi::new(Arc::clone(&cache), Arc::new(ChangeFeed::with_instance(1)));
        let token = api.grant_with("window", fixed).expose().to_string();
        let answer: serde_json::Value = serde_json::from_str(&ask(&api, "GET", "/sharing", Some(&token), "").body).expect("json");
        assert_eq!(answer["data"]["available"], false);
        assert_eq!(answer["data"]["reason"], crate::rest::sharing::REASON_BW_SERVE);
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        nothing.assert();
    }

    /// A wrong method on a sharing route is 405, not 404.
    #[test]
    fn a_wrong_method_on_a_sharing_route_is_refused_as_a_method() {
        let (_server, _cache, api, token) = a_daemon();
        assert_eq!(ask(&api, "DELETE", "/sharing", Some(&token), "").status, 405);
    }
```

`src/vault_cache.rs`, append to its `mod tests`. Use whichever populated-cache helper that module already has. Here it is written against `populate_with` over a `VaultBridge` that is never called:

```rust
    /// **What the sharing engine changed on the server lands in the snapshot**
    /// the way a write's own answer does: at the top, the old copy gone.
    #[test]
    fn items_written_elsewhere_replace_and_remove_in_the_snapshot() {
        let cache = VaultCache::new(crate::vault_bridge::VaultBridge::new("http://127.0.0.1:1"));
        let item = |id: &str| -> VaultItem { serde_json::from_value(serde_json::json!({ "id": id, "name": id })).expect("item") };
        assert_eq!(cache.populate_with(vec![item("a"), item("b")], cache.epoch()).expect("seeded"), PopulateOutcome::Populated);
        let mut moved = item("b");
        moved.name = "moved".into();
        cache.apply_written_elsewhere(&[moved, item("c")], &["a".to_string()]);
        let ids: Vec<String> = cache.items().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["c", "b"]);
        assert_eq!(cache.get_by_id("b").map(|i| i.name), Some("moved".to_string()));
    }
```

`src/vault_host.rs`, append to `mod tests`:

```rust
    /// **A window reads the sharing view from the daemon, over the real
    /// loopback listener**, and hears `Change::Sharing` on its feed.
    #[test]
    fn a_window_reads_the_sharing_view_and_hears_its_change_through_the_daemon() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(sync_payload()).expect_at_least(0).create();
        server.mock("GET", "/api/accounts/profile").with_body(crate::rest::backend::tests::sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        let daemon = Arc::new(VaultCache::new(backend));
        assert_eq!(daemon.populate().expect("the load"), PopulateOutcome::Populated);
        let api = Arc::new(VaultApi::new(Arc::clone(&daemon), Arc::new(ChangeFeed::with_instance(5))).with_long_poll(Duration::from_millis(200)));
        let host = VaultHost::start(Arc::clone(&api)).expect("a loopback port");
        let link = DaemonLink::from_line(&host.link_line_for("window")).expect("the line");
        let listener = crate::rest::notifications::HubListener::following(link.feed_poll(), FAST);

        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        eventually("the window heard the sharing change", || listener.sharing_changed(Instant::now()));
        let view = link.sharing().view().expect("the view");
        assert!(view.available);
        drop(host);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 change_feed && cargo test --lib -j 2 vault_api`
Expected: a compile error. `Change::Sharing`, `sharing_pass`, `apply_written_elsewhere`, `sharing_changed` and `link.sharing` are not found.

- [ ] **Step 3: The feed kind**

`src/change_feed.rs`: add to `pub enum Change`:

```rust
    /// **The sharing view changed**: re-read `GET /sharing`. Published after
    /// every sharing engine write and after every pass that changed
    /// anything. Item movements ride the vault kinds beside it.
    Sharing,
```

Add to `enum WireChange`, after `Sends,`:

```rust
    Sharing,
    /// A kind this build does not know -- a newer daemon's, read by an older
    /// client. Skipped rather than failing the page (spec 11.4).
    #[serde(other)]
    Unknown,
```

In `impl From<&Change> for WireChange`, add `Change::Sharing => WireChange::Sharing,`. Replace `impl From<WireChange> for Change { … }` with:

```rust
/// A wire change as this build understands it, or `None` for a kind it does
/// not know.
fn change_from(change: WireChange) -> Option<Change> {
    Some(match change {
        WireChange::Item { id, revision } => Change::Vault(Announcement::Item { id, revision }),
        WireChange::ItemGone { id, revision } => Change::Vault(Announcement::ItemGone { id, revision }),
        WireChange::VaultAt { date } => Change::Vault(Announcement::VaultAt { date }),
        WireChange::Whole => Change::Vault(Announcement::Whole),
        WireChange::Sends => Change::Sends,
        WireChange::Sharing => Change::Sharing,
        WireChange::Unknown => return None,
    })
}
```

In `page_from_body`, change `changes: wire.changes.into_iter().map(Change::from).collect(),` to `changes: wire.changes.into_iter().filter_map(change_from).collect(),`.

`src/rest/notifications.rs`:
- To `Inbox`, add `sharing: Option<Instant>,`, with the doc `/// When the daemon last said the sharing view changed, if not taken yet -- a time, for \`sends\`'s reason.`
- To `impl Shared`, add:

```rust
    fn notice_sharing(&self) {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner).sharing = Some(Instant::now());
    }
```

In `follow`, change the page loop's match to include `Change::Sharing => shared.notice_sharing(),`. In the `if page.reset || restarted` branch, add `shared.notice_sharing();` after `shared.notice_sends();`.

After `sends_changed`, add:

```rust
    /// **The sharing view is stale**: the daemon said `sharing`, or the feed
    /// was reset. True once per settled notice, as [`Self::sends_changed`].
    /// The window's answer is to re-read `GET /sharing` -- one loopback
    /// request out of the daemon's memory, never the server.
    pub fn sharing_changed(&self, now: Instant) -> bool {
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(PoisonError::into_inner);
        match inbox.sharing {
            Some(at) if now.saturating_duration_since(at) >= self.settle => {
                inbox.sharing = None;
                true
            }
            _ => false,
        }
    }
```

- [ ] **Step 4: The cache hook**

`src/vault_cache.rs`, after `apply_announced_writing`:

```rust
    /// **Items the sharing engine changed on the server**, applied the way a
    /// write's own answer is: the new copy at the top of the list, the old one
    /// gone, each noted as a write so a populate racing it cannot resurrect
    /// what it replaced.
    ///
    /// A share keeps a record's id and changes what it is filed under; a move
    /// back gives it a new id and deletes the old one (spec 9.4), which is one
    /// `removed` and one `upserted`. No read happens here: the engine already
    /// holds the server's answer, decrypted.
    pub fn apply_written_elsewhere(&self, upserted: &[VaultItem], removed: &[String]) {
        let _ = self.persisting(|| {
            let mut snapshot = self.lock();
            if snapshot.populated {
                for id in removed {
                    snapshot.items.retain(|i| &i.id != id);
                    snapshot.note_item_write(id, true);
                }
                for item in upserted {
                    if let Some(at) = snapshot.items.iter().position(|i| i.id == item.id) {
                        snapshot.items.remove(at);
                    }
                    snapshot.items.insert(0, item.clone());
                    snapshot.note_item_write(&item.id, false);
                }
            }
            Ok(())
        });
    }
```

- [ ] **Step 5: The routes and the pass in `vault_api.rs`**

Add these imports: `use crate::change_feed::Change;` and `use crate::rest::sharing::{Sharing, SharingError, SharingOutcome, Trigger};`.

In `answer`'s `match`, before the 405 alternation, add:

```rust
            ("GET", ["sharing"]) => self.sharing_view(),
```

Add `| (_, ["sharing", ..])` to the 405 alternation.

Add these methods to `impl VaultApi`, after `forget_applied`:

```rust
    // ---- personal sharing ---------------------------------------------------

    /// The engine, or the response that says why there is none: a vault not
    /// loaded yet is 503, a backend without one (`bw serve`) is 501.
    fn engine(&self) -> Result<Arc<Sharing>, ApiResponse> {
        if !self.cache.is_populated() {
            return Err(not_ready());
        }
        self.cache
            .bridge()
            .sharing()
            .ok_or_else(|| ApiResponse::refused(501, "Sharing needs direct sync with a self-hosted server."))
    }

    /// `GET /sharing` -- answered from the engine's held view and the daemon's
    /// items, with no server round trip (spec 11.3). Never an error: "not
    /// loaded" and "not here" are views too.
    fn sharing_view(&self) -> ApiResponse {
        if !self.cache.is_populated() {
            return ApiResponse::ok(to_value(&crate::rest::sharing::SharingView::unavailable(
                crate::rest::sharing::REASON_NOT_LOADED,
            )));
        }
        match self.cache.bridge().sharing() {
            Some(engine) => ApiResponse::ok(to_value(&engine.view(&self.cache.items()))),
            None => ApiResponse::ok(to_value(&crate::rest::sharing::SharingView::unavailable(
                crate::rest::sharing::REASON_BW_SERVE,
            ))),
        }
    }

    /// **What a sharing write changed, into the daemon's copy and onto the
    /// feed** -- removals first, then the new copies, in one batch, then
    /// `sharing`. A window reads each announced item back from this daemon,
    /// which already holds it; nothing reaches the server.
    fn publish_outcome(&self, outcome: &SharingOutcome) {
        self.cache.apply_written_elsewhere(&outcome.upserted, &outcome.removed);
        let mut changes: Vec<Change> = outcome
            .removed
            .iter()
            .map(|id| Change::Vault(crate::rest::notifications::Announcement::ItemGone { id: id.clone(), revision: None }))
            .collect();
        changes.extend(outcome.upserted.iter().map(|item| {
            Change::Vault(crate::rest::notifications::Announcement::Item {
                id: item.id.clone(),
                revision: item.other.get("revisionDate").and_then(serde_json::Value::as_str).map(str::to_string),
            })
        }));
        changes.push(Change::Sharing);
        self.feed.publish(changes);
    }

    /// **One sharing pass** -- the triggers of spec 11.5 call this: a whole
    /// read, an applied hub batch, the timer. It is the daemon's own work; no
    /// window needs to be open. A failure is logged and the next trigger tries
    /// again.
    pub fn sharing_pass(&self, trigger: Trigger) {
        if !self.cache.is_populated() {
            return;
        }
        let Some(engine) = self.cache.bridge().sharing() else {
            return;
        };
        match engine.refresh(trigger) {
            Ok(pass) => {
                if !pass.outcome.is_empty() {
                    self.publish_outcome(&pass.outcome);
                } else if pass.changed {
                    self.feed.publish([Change::Sharing]);
                }
            }
            Err(e) => log::info!("the sharing pass ({trigger:?}) did not complete: {e}"),
        }
    }

    /// Whether the ten-minute timer has anything to do.
    #[must_use]
    pub fn sharing_wants_timer(&self) -> bool {
        self.cache.is_populated() && self.cache.bridge().sharing().is_some_and(|engine| engine.wants_timer())
    }
```

Add this free function beside `failed`:

```rust
/// A sharing refusal as an answer. Spec 11.3: `Unsupported` -> 501 with why, a
/// refused step -> 400 with the sentence to show, the rest as `failed`.
fn sharing_failed(error: &SharingError) -> ApiResponse {
    match error {
        SharingError::Unavailable(reason) => ApiResponse::refused(501, reason),
        SharingError::Refused(sentence) => ApiResponse::refused(400, sentence),
        SharingError::Vault(e) => failed(e),
    }
}
```

`sharing_failed` has no caller until Task 9. Put `#[allow(dead_code)]` on it with the comment `// Its callers are the write routes, which arrive with the flows (Tasks 9-13).`, and remove that attribute in Task 9.

In `sync`, in the `Ok(PopulateOutcome::Populated)` arm, after `self.forget_applied();`, add `self.sharing_pass(Trigger::WholeRead);`.

- [ ] **Step 6: The triggers in `vault_host.rs`**

- Add `use crate::rest::sharing::Trigger;` to the imports.
- Add the constant:

```rust
/// **The sharing timer** (spec 11.5, trigger 4): how often the daemon asks
/// the server about people still waiting, while the vault is unlocked and
/// only while somebody is. NodeWarden pushes an accept to the sharer from
/// slice 1, so this is the backstop, not the path.
pub const SHARING_TIMER: Duration = Duration::from_secs(600);

/// The hub worker's memory for the sharing pass: which vault era it last
/// ran for (a new era is an unlock or a switch: a whole read), and when the
/// timer is next due.
struct SharingTick {
    era: Option<crate::vault_cache::VaultEra>,
    next: Instant,
}

/// Runs the pass for a new era and for the timer; called on the hub worker's
/// own thread, once per tick.
fn sharing_tick(api: &VaultApi, tick: &mut SharingTick, now: Instant) {
    let cache = api.cache();
    if !cache.is_populated() {
        tick.era = None;
        return;
    }
    let era = cache.epoch().era();
    if tick.era != Some(era) {
        tick.era = Some(era);
        tick.next = now + SHARING_TIMER;
        api.sharing_pass(Trigger::WholeRead);
        return;
    }
    if now >= tick.next {
        tick.next = now + SHARING_TIMER;
        if api.sharing_wants_timer() {
            api.sharing_pass(Trigger::Timer);
        }
    }
}
```

In `HubWorker::start`'s thread closure, before the `while`, add `let mut sharing = SharingTick { era: None, next: Instant::now() + SHARING_TIMER };`. Inside the loop, after `hub_tick(…)`, add `sharing_tick(&api, &mut sharing, Instant::now());`.

In `hub_tick`, change the `Ok(fetched) => { …; api.note_applied(era, before); true }` arm so that after `api.note_applied(era, before);` it calls `api.sharing_pass(Trigger::Batch);`. In the populate fallback's `Ok(_) => api.forget_applied(),` use `Ok(_) => { api.forget_applied(); api.sharing_pass(Trigger::WholeRead); }`.

These add no thread: the pass runs on the existing `vault-hub` worker, a `std::thread::Builder` that the census does not count.

- [ ] **Step 7: The window's client in `daemon_link.rs`**

```rust
/// How long a sharing call may take: a share is about eight server requests
/// behind the daemon.
const SHARING_DEADLINE: Duration = Duration::from_secs(60);

/// **The window's line to the daemon's sharing engine.** Typed calls to the
/// `/sharing` routes; nothing personal in a query string (spec 11.3).
#[derive(Clone)]
pub struct SharingClient {
    base_url: String,
    agent: crate::http_agent::TotalBounded,
}

impl SharingClient {
    /// A client for one daemon, with one grant.
    #[must_use]
    pub fn new(base_url: &str, grant: &Zeroizing<String>) -> Self {
        Self {
            base_url: base_url.to_string(),
            agent: crate::http_agent::bounded_total_with_bearer(CONNECT, SHARING_DEADLINE, grant.clone()),
        }
    }

    /// `GET /sharing`.
    ///
    /// # Errors
    ///
    /// The daemon refused or did not answer, as a sentence.
    pub fn view(&self) -> Result<crate::rest::sharing::SharingView, String> {
        let data = sharing_answer(self.agent.get(&format!("{}/sharing", self.base_url)).call())?;
        serde_json::from_value(data).map_err(|e| format!("The vault service answered something unreadable: {e}"))
    }

    /// One `POST` to a sharing route, its `data` back.
    pub(crate) fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
        sharing_answer(self.agent.post(&format!("{}{}", self.base_url, path)).send_json(body))
    }
}

/// A sharing route's answer: `data` on success, the daemon's `message` -- the
/// sentence the engine chose -- on a refusal.
fn sharing_answer(result: Result<ureq::Response, ureq::Error>) -> Result<serde_json::Value, String> {
    match result {
        Ok(response) => {
            let body: serde_json::Value = response.into_json().map_err(|e| e.to_string())?;
            Ok(body.get("data").cloned().unwrap_or(serde_json::Value::Null))
        }
        Err(ureq::Error::Status(_, response)) => {
            let body: serde_json::Value = response.into_json().unwrap_or(serde_json::Value::Null);
            Err(body
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("The vault service refused the request.")
                .to_string())
        }
        Err(e) => Err(format!("The vault service did not answer: {e}")),
    }
}
```

In `impl DaemonLink`, add:

```rust
    /// This link's sharing client.
    #[must_use]
    pub fn sharing(&self) -> SharingClient {
        SharingClient::new(&self.base_url, &self.grant)
    }
```

After `feed_poll()` (the free function), add:

```rust
/// The window's sharing client, or `None` in a process with no link -- where
/// sharing is not offered (spec 7.4) and the Share control offers a Send.
#[must_use]
pub fn sharing_client() -> Option<SharingClient> {
    installed().map(DaemonLink::sharing)
}
```

If `Zeroizing` is not imported in `daemon_link.rs`, add `use zeroize::Zeroizing;`. The struct's `grant` field already uses it.

- [ ] **Step 8: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/change_feed.rs src/rest/notifications.rs src/vault_cache.rs src/vault_api.rs src/vault_host.rs src/daemon_link.rs
cargo test --lib -j 2 change_feed && cargo test --lib -j 2 vault_api && cargo test --lib -j 2 vault_host && cargo test --lib -j 2 vault_cache && cargo test --lib -j 2 rest::notifications
```

Expected: every file reports `0`, and all pass.

The existing request-counting tests in `vault_api` and `vault_host` still pass. The sharing pass on a whole read asks `/api/organizations/invitations/mine` and the profile. Those routes are unmocked there and answer 501. The probe reads that as "not NodeWarden", and those tests count only the sync and the revision.

- [ ] **Step 9: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/change_feed.rs deskwarden/src/rest/notifications.rs deskwarden/src/vault_cache.rs deskwarden/src/vault_api.rs deskwarden/src/vault_host.rs deskwarden/src/daemon_link.rs
git commit -m "The daemon answers GET /sharing and says when it changed

GET /sharing answers the engine's view out of the daemon's memory:
unavailable with a reason on bw serve, before the first read, or off
NodeWarden. The change feed gains a sharing kind, and a kind a client
does not know is skipped instead of failing the page (spec 11.4).

The sharing pass runs after every whole read (POST /sync, the hub's
populate, a new vault era), after every applied hub batch, and on a
ten-minute timer while someone is waiting. All of it is on the existing
hub worker thread. What a pass or a write changes lands in the daemon's
cache through apply_written_elsewhere and on the feed.

The window reads it through daemon_link::SharingClient.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 9: Share a record with a person (engine, route, client)

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs`
  - `call` now surfaces a server's sentence;
  - `SharingError::or_say`;
  - the share flow and its helpers;
  - tests.
- Modify: `deskwarden/src/rest/organizations.rs`: `map_members` becomes `pub(crate)`
- Modify: `deskwarden/src/sent_history.rs`: `SentKind`, `SentRecord.kind`, `Person.sent`/`Person.shared`
- Modify: `deskwarden/src/vault_api.rs`: route `POST /sharing/item/{id}`, `email_in`
- Modify: `deskwarden/src/daemon_link.rs`: `SharingClient::share`

**Interfaces:**
- Consumes:
  - Task 1: `generate_symmetric_key`, `wrap_key_to_public`, `wrap_key_under`, `unwrap_key_under`, `public_key_of`, `public_key_sha256_hex`, `encrypt`
  - Task 3: `private_key_der`, `with_organization`
  - Task 4: every `sharing_api` route
  - Task 5: `sharing_org_name`, `sharer_of`, `sharer_is`
  - Task 6: `sharing_state::update`
  - Task 7: the engine
  - Task 8: `publish_outcome`, `sharing_failed`, `SharingClient::post`
- Produces:
  - `pub enum ShareResult { Shared, WaitingAccept, WaitingAccount }` (serde camelCase)
  - `pub struct ShareOutcome { pub result: ShareResult, pub changed: SharingOutcome }`
  - `Sharing::share(&self, item_id: &str, email: &str) -> Result<ShareOutcome, SharingError>`
  - the refusal sentences `pub const NOT_AN_EMAIL`, `YOURSELF`, `IN_TRASH`, `HAS_ATTACHMENTS`, `UNREADABLE_PART`, `OTHER_ORG`, `SHARE_FAILED`, `ORG_FAILED`
  - `sent_history::SentKind { Send, Share }`, `SentRecord.kind: SentKind`, `Person { …, sent: usize, shared: usize }`
  - `SharingClient::share(&self, item_id: &str, email: &str) -> Result<ShareResult, String>`
  - internal engine helpers reused by Tasks 10-13, all `pub(crate)` or private in `sharing.rs`:
    - `struct Ctx { account: Account, keys: Arc<VaultKeys>, org_id: String, org_key: SymmetricKey, collections: Vec<(String, String)> }`
    - `fn context(&self) -> Result<Ctx, SharingError>`
    - `fn my_org_id(&self, profile, account) -> Option<String>`
    - `fn collections_of(&self, org, key) -> Result<Vec<(String, String)>, SharingError>`
    - `fn fetch_roster(&self, org) -> Result<Vec<Member>, SharingError>`
    - `fn fetch_record(&self, id, keys) -> Result<(DecryptedItem, Vec<DecryptFailure>), SharingError>`
    - `fn ensure_member(&self, ctx, email, collection) -> Result<ShareResult, SharingError>`
    - `fn reload_org(&self, ctx) -> Result<(), SharingError>`
    - `fn pin(&self, account, email, key: &AccountKey)`
    - `fn pin_allows(&self, account, email, spki) -> bool`
    - `fn crypto(e: CryptoError) -> SharingError`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/sharing.rs`. Add the imports `personal_cipher, shared_cipher, sharing_org_row, person_collection, roster_row, FIXTURE_EMAIL` to the existing `use crate::rest::backend::tests::{…}` line.

```rust
    use crate::test_http::{Matcher, MockServer};

    /// A probed engine on a NodeWarden with slice 1, over the fixture
    /// account, with its files in a scratch directory.
    fn on_nodewarden(organizations: serde_json::Value) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let dir = crate::test_scratch::ScratchDir::new("sharing-flow");
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup {
            state_path: Some(dir.join("sharing_state.json")),
            sent_path: Some(dir.join("sent_history.json")),
            fill_stats_path: Some(dir.join("fill-stats.json")),
            make_key_pair: fixture_key_pair,
        });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(organizations).to_string()).expect_at_least(0).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect_at_least(0).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect_at_least(0)
            .create();
        engine.refresh(Trigger::WholeRead).expect("probed");
        (server, backend, engine, dir)
    }

    fn roster(rows: &[serde_json::Value]) -> String {
        serde_json::json!({ "object": "list", "data": rows }).to_string()
    }

    /// The server's copy of a cipher it was sent: the request's own cipher,
    /// with the id and filing a server adds. What NodeWarden answers `/share`
    /// and `POST /api/ciphers` with, modulo fields this crate does not read.
    fn echo(id: &'static str, org: Option<&'static str>, collections: &'static [&'static str]) -> impl Fn(&crate::test_http::Request) -> Vec<u8> + Send + Sync + 'static {
        move |request| {
            let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
            let mut cipher = body.get("cipher").cloned().unwrap_or(body);
            cipher["id"] = serde_json::json!(id);
            cipher["organizationId"] = serde_json::json!(org);
            cipher["collectionIds"] = serde_json::json!(collections);
            cipher["revisionDate"] = serde_json::json!("2026-09-27T00:00:00.000Z");
            cipher.to_string().into_bytes()
        }
    }

    /// **The first share ever** (spec 6.2): org, collection, direct add,
    /// move -- each once, in that order's dependencies, and no sync.
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
        // share made. NodeWarden's web vault may leave a name in plaintext,
        // and the engine reads that too -- which is what lets this answer
        // name c-anna without the org key the test cannot know.
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).expect(1).create();
        server
            .mock("GET", "/api/organizations/org-new/collections")
            .with_body(r#"{"data":[{"id":"c-anna","organizationId":"org-new","name":"anna@example.com"}]}"#)
            .create();
        let collection = server
            .mock("POST", "/api/organizations/org-new/collections")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "users": [{ "id": FIXTURE_USER_ID, "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body(r#"{"id":"c-anna","object":"collection"}"#)
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
            ]))
            .create();
        server
            .mock("GET", "/api/organizations/org-new/members/public-key?email=anna%40example.com")
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let invite = server
            .mock("POST", "/api/organizations/org-new/members/invite")
            .match_request(|request| request.utf8_lossy_body().contains("\"encryptedOrgKey\":\"4."))
            .with_body(r#"{"id":"anna-id","status":1,"object":"organizationMember"}"#)
            .expect(1)
            .create();
        let share = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_body(Matcher::PartialJson(serde_json::json!({ "collectionIds": ["c-anna"], "cipher": { "organizationId": "org-new" } })))
            .with_body_from_request(echo("p1", Some("org-new"), &["c-anna"]))
            .expect(1)
            .create();

        let outcome = engine.share("p1", " Anna@Example.com ").expect("shared");
        org.assert();
        collection.assert();
        invite.assert();
        share.assert();
        sync.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
        assert_eq!(outcome.changed.upserted.len(), 1);
        let moved = &outcome.changed.upserted[0];
        assert_eq!(moved.name, "Acme VPN", "the moved record decrypts under the new org key");
        assert_eq!(organization_of(moved), Some("org-new"));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.sharing_org_id.as_deref(), Some("org-new"));
        assert_eq!(state.moves.get("p1").and_then(|m| m.folder_id.as_deref()), Some("f1"));
        assert!(state.pin_for("anna@example.com").is_some(), "a direct add pins the key at that moment");
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].kind, crate::sent_history::SentKind::Share);
        assert_eq!(sent.entries[0].recipient, "anna@example.com");
        assert_eq!(engine.view(std::slice::from_ref(moved)).person("anna@example.com").map(|p| p.state), Some(PersonState::Shared));
    }

    /// **An existing person, another record already in the org** (spec 6.3,
    /// 6.4): one collections call and nothing else written.
    #[test]
    fn a_share_to_an_existing_person_is_one_collections_call() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-bob"]).to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-bob", "bob@example.com"), person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
                roster_row("bob-id", Some("bob-id"), "bob@example.com", 2, &["c-bob"]),
            ]))
            .create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let collections = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob", "c-anna"] })))
            .with_body(shared_cipher("s1", "Acme VPN", &["c-bob", "c-anna"]).to_string())
            .expect(1)
            .create();
        let outcome = engine.share("s1", "anna@example.com").expect("shared");
        collections.assert();
        writes.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
    }

    /// **A person with no account** (spec 6.5): a pending invitation, no key
    /// wrapped, and the dialog is told so.
    #[test]
    fn a_share_to_someone_without_an_account_leaves_an_invitation() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-carol", "carol@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/members/public-key?email=carol%40example.com").as_str())
            .with_status(404)
            .create();
        let invite = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/members/invite").as_str())
            .match_request(|request| !request.utf8_lossy_body().contains("encryptedOrgKey"))
            .with_body(r#"{"id":"inv-1","status":0,"object":"organizationInvitation"}"#)
            .expect(1)
            .create();
        server
            .mock("PUT", "/api/ciphers/p1/share")
            .with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-carol"]))
            .create();
        let outcome = engine.share("p1", "carol@example.com").expect("shared");
        invite.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccount);
    }

    /// **A failure at the move leaves the record personal** (spec 6.2, 13),
    /// through the daemon: the item in its cache is untouched and the answer
    /// is the dialog's sentence.
    #[test]
    fn a_share_that_fails_at_the_move_leaves_the_record_personal() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]))
            .create();
        server.mock("PUT", "/api/ciphers/live-1/share").with_status(500).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        assert_eq!(answer.status, 400);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(body["message"], SHARE_FAILED);
        let held = cache.get_by_id("live-1").expect("still in the daemon's vault");
        assert_eq!(organization_of(&held), None, "the record stayed personal");
    }

    /// **What cannot move is refused before anything is sent** (spec 9.3,
    /// and decision 3: any attachment at all).
    #[test]
    fn a_record_with_attachments_is_refused_before_anything_is_written() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let mut with_file = personal_cipher("p1", "Acme VPN");
        with_file["attachments"] = serde_json::json!([{ "id": "a1", "fileName": "2.x|y|z" }]);
        server.mock("GET", "/api/ciphers/p1").with_body(with_file.to_string()).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, HAS_ATTACHMENTS),
            other => panic!("expected the attachments refusal, got {other:?}"),
        }
        writes.assert();
        puts.assert();
        assert!(matches!(engine.share("p1", "not an email"), Err(SharingError::Refused(s)) if s == NOT_AN_EMAIL));
        assert!(matches!(engine.share("p1", FIXTURE_EMAIL), Err(SharingError::Refused(s)) if s == YOURSELF));
    }
```

`vault_api`'s `ask` is private today. Make it `pub(crate) fn ask(`, with the doc line `/// \`pub(crate)\` for \`rest::sharing\`'s daemon-level tests.`

Append to `mod tests` in `src/sent_history.rs`:

```rust
    /// **A share is a person in the address book too** (spec 11.7), counted
    /// separately so the suggestion can say "2 shared" or "3 sent", and a file
    /// from before shares reads as all Sends.
    #[test]
    fn shares_and_sends_feed_one_address_book_with_their_own_counts() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "anna@example.com"));
        history.record(SentRecord { kind: SentKind::Share, item_id: "r1".into(), recipient: "Anna@example.com".into(), sent_at_unix_millis: 2, ..SentRecord::default() });
        history.record(SentRecord { kind: SentKind::Share, item_id: "r2".into(), recipient: "anna@example.com".into(), sent_at_unix_millis: 3, ..SentRecord::default() });
        let people = history.people();
        assert_eq!(people.len(), 1);
        assert_eq!((people[0].sent, people[0].shared), (1, 2));
        assert_eq!(people[0].send_ids, ["a"], "a share has no Send id");
        let old: SentHistory = serde_json::from_str(r#"{"entries":[{"send_id":"x","recipient":"b@c.d"}]}"#).expect("an old file");
        assert_eq!(old.entries[0].kind, SentKind::Send);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 sent_history`
Expected: a compile error. `share`, `ShareResult`, `SentKind`, `HAS_ATTACHMENTS` and the other new names are not found.

- [ ] **Step 3: `sent_history.rs`**

Add above `pub struct SentRecord`:

```rust
/// What an entry records: a Send (a link) or a share (a person). Spec 11.7:
/// both feed the one address book the Send composer and the Share dialog
/// suggest from. `Send` by default, so a file from before shares reads
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SentKind {
    #[default]
    Send,
    Share,
}
```

Add to `SentRecord`, as its first field:

```rust
    /// A Send or a share. For a share, `send_id` is empty and `fields` is
    /// empty: a person gets the whole record, and there is no link.
    pub kind: SentKind,
```

Add to `Person`:

```rust
    /// How many of this person's entries are Sends, and how many shares --
    /// what the suggestion's count says ("3 sent", "2 shared").
    pub sent: usize,
    pub shared: usize,
```

In `people()`, replace the `match` body with:

```rust
            let is_send = entry.kind == SentKind::Send;
            match people.iter_mut().find(|p| same_person(&p.email, &entry.recipient)) {
                Some(person) => {
                    if is_send && !entry.send_id.is_empty() {
                        person.send_ids.push(entry.send_id.clone());
                    }
                    person.sent += usize::from(is_send);
                    person.shared += usize::from(!is_send);
                    person.last_sent_unix_millis =
                        person.last_sent_unix_millis.max(entry.sent_at_unix_millis);
                }
                None => people.push(Person {
                    email: entry.recipient.trim().to_string(),
                    last_sent_unix_millis: entry.sent_at_unix_millis,
                    send_ids: if is_send && !entry.send_id.is_empty() { vec![entry.send_id.clone()] } else { Vec::new() },
                    sent: usize::from(is_send),
                    shared: usize::from(!is_send),
                }),
            }
```

The existing test `people_are_listed_once_each_most_recently_sent_first` still passes. Its entries are Sends with non-empty ids, so `send_ids` is unchanged.

- [ ] **Step 4: The share flow in `sharing.rs`**

Replace `Sharing::call` with a version that keeps a server's sentence:

```rust
    /// One request, with the session locked for exactly its length -- the
    /// backend's own lock discipline. **Never called with `held` locked.**
    ///
    /// A refusal that carries the server's own sentence becomes
    /// [`SharingError::Refused`] with it; every other failure is mapped as
    /// the backend maps it.
    pub(crate) fn call<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        request(&self.client, &mut guard).map_err(|e| match e {
            RestError::Rejected { description, .. } if !description.is_empty() => SharingError::Refused(description),
            other => SharingError::Vault(crate::rest::backend::rest_error(other)),
        })
    }
```

Add to `impl SharingError`:

```rust
impl SharingError {
    /// A step's own sentence in place of a server failure. **A session that
    /// ended keeps its own error**, because the window answers that one
    /// differently (sign in again). A server sentence is kept after the
    /// step's, so "User is already a member" is not lost.
    #[must_use]
    pub(crate) fn or_say(self, sentence: &str) -> Self {
        match self {
            Self::Vault(VaultError::Unauthorized) | Self::Unavailable(_) => self,
            Self::Refused(server) if !server.is_empty() && server != sentence => {
                Self::Refused(format!("{sentence} The server said: {server}"))
            }
            _ => Self::Refused(sentence.to_string()),
        }
    }
}
```

Add after the view section's constants:

```rust
// ---- the sentences a refused write says (spec 13) --------------------------------

pub const NOT_AN_EMAIL: &str = "That doesn't look like an email address.";
pub const YOURSELF: &str = "That's your own address.";
pub const IN_TRASH: &str = "This record is in the trash or the archive. Restore it before sharing it.";
pub const HAS_ATTACHMENTS: &str =
    "This record has attachments. Deskwarden can't share records with attachments yet.";
pub const UNREADABLE_PART: &str =
    "Part of this record could not be read, so it can't be moved. Open it in the Bitwarden web vault first.";
pub const OTHER_ORG: &str = "This record belongs to an organisation. Share it from there.";
pub const SHARE_FAILED: &str = "Couldn't share the record. It's still in your vault.";
pub const ORG_FAILED: &str = "Deskwarden couldn't start sharing on this server.";

/// What a share ended as: the words the dialog chooses between (mock 6a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareResult {
    /// In their vault now.
    Shared,
    /// A member without the key yet: the confirm pass will finish it.
    WaitingAccept,
    /// No account: a pending invitation.
    WaitingAccount,
}

/// A share's result and what it changed.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub result: ShareResult,
    pub changed: SharingOutcome,
}

/// Everything a write needs about the account and its sharing org, read
/// fresh at the start of the write (spec 13: every write reads current state
/// first).
pub(crate) struct Ctx {
    pub(crate) account: Account,
    pub(crate) keys: Arc<VaultKeys>,
    pub(crate) org_id: String,
    pub(crate) org_key: SymmetricKey,
    /// `(collection id, normalised email)`.
    pub(crate) collections: Vec<(String, String)>,
}

impl Ctx {
    pub(crate) fn collection_for(&self, email: &str) -> Option<&str> {
        let email = normalise_email(email);
        self.collections.iter().find(|(_, e)| *e == email).map(|(id, _)| id.as_str())
    }
}

/// A crypto failure as a sharing error, mapped as the backend maps it.
pub(crate) fn crypto(e: CryptoError) -> SharingError {
    SharingError::Vault(crate::rest::backend::crypto_error(e))
}

/// Plausibly an email: one `@` with something either side, no whitespace.
fn plausible_email(email: &str) -> bool {
    let mut halves = email.split('@');
    matches!((halves.next(), halves.next(), halves.next()), (Some(l), Some(d), None) if !l.is_empty() && d.contains('.'))
        && !email.chars().any(char::is_whitespace)
}

/// A collection name read with the org key; a NodeWarden web-vault name
/// that is plaintext is read as it is.
fn collection_email(raw: &str, key: &SymmetricKey) -> Option<String> {
    match raw.parse::<EncString>() {
        Ok(enc) => {
            let bytes = crate::rest::crypto::decrypt(key, &enc).ok()?;
            std::str::from_utf8(&bytes).ok().map(normalise_email)
        }
        Err(_) => Some(normalise_email(raw)),
    }
}

/// **The refusals of spec 9.3**, before anything is sent. Decision 3 of this
/// build refuses every attachment, not only unkeyed ones: a record moved
/// back is re-created, and an attachment belongs to the old cipher id.
fn refuse_unmovable(
    record: &crate::rest::sync::DecryptedItem,
    failures: &[crate::rest::sync::DecryptFailure],
    my_org: Option<&str>,
) -> Result<(), SharingError> {
    let set = |key: &str| record.item.other.get(key).is_some_and(|v| !v.is_null());
    if set("deletedDate") || set("archivedDate") {
        return Err(SharingError::Refused(IN_TRASH.to_string()));
    }
    if record.item.other.get("attachments").and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty()) {
        return Err(SharingError::Refused(HAS_ATTACHMENTS.to_string()));
    }
    if !failures.is_empty() {
        return Err(SharingError::Refused(UNREADABLE_PART.to_string()));
    }
    match organization_of(&record.item) {
        Some(org) if Some(org) != my_org => Err(SharingError::Refused(OTHER_ORG.to_string())),
        _ => Ok(()),
    }
}
```

Add a second `impl Sharing` block holding the flow:

```rust
// ---- the owner's writes -----------------------------------------------------------

impl Sharing {
    /// This account's own sharing org among the profile's: the cached id while
    /// the account still owns it, else the first owned org named for it.
    pub(crate) fn my_org_id(&self, profile: &Profile, account: &Account) -> Option<String> {
        let owned: Vec<&crate::rest::sync::Organization> =
            profile.organizations.iter().filter(|o| o.is_owner()).collect();
        if let Some(cached) = self.cached_org_id() {
            if owned.iter().any(|o| o.id == cached) {
                return Some(cached);
            }
        }
        owned
            .iter()
            .find(|o| {
                o.name
                    .as_deref()
                    .and_then(sharer_of)
                    .is_some_and(|sharer| crate::rest::organizations::sharer_is(sharer, &account.email))
            })
            .map(|o| o.id.clone())
    }

    /// The org's person collections, decrypted: `(id, email)`.
    pub(crate) fn collections_of(&self, org: &str, key: &SymmetricKey) -> Result<Vec<(String, String)>, SharingError> {
        let listed = self.call(|client, auth| sharing_api::organization_collections(client, &mut auth.session, org))?;
        Ok(listed.into_iter().filter_map(|c| collection_email(&c.name, key).map(|email| (c.id, email))).collect())
    }

    /// The org's roster, fresh: `/users`, or `/members` on an older server.
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.call(|client, auth| client.fetch_organization_members(&mut auth.session, org))?;
        Ok(crate::rest::organizations::map_members(&answer).unwrap_or_default())
    }

    /// One record, fresh from the server, decrypted with `keys`.
    pub(crate) fn fetch_record(
        &self,
        id: &str,
        keys: &VaultKeys,
    ) -> Result<(crate::rest::sync::DecryptedItem, Vec<crate::rest::sync::DecryptFailure>), SharingError> {
        let raw = self.call(|client, auth| client.fetch_cipher(&mut auth.session, id))?;
        crate::rest::sync::decrypt_cipher(&raw, keys)
            .ok_or_else(|| SharingError::Refused("This record could not be read.".to_string()))
    }

    /// The account and its sharing org, for a write that needs the org to
    /// exist already (stop, forget, reinvite, confirm).
    pub(crate) fn context(&self) -> Result<Ctx, SharingError> {
        self.require_available()?;
        let profile = self.profile()?;
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let keys = self.keys_from(&profile)?;
        let org_id = self
            .my_org_id(&profile, &account)
            .ok_or_else(|| SharingError::Refused("You are not sharing anything yet.".to_string()))?;
        let org_key = keys
            .organization(&org_id)
            .cloned()
            .ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let collections = self.collections_of(&org_id, &org_key)?;
        Ok(Ctx { account, keys, org_id, org_key, collections })
    }

    /// **Creates the sharing org** (spec 9.1): a fresh org key wrapped to the
    /// account's own public key -- derived from its own private key, not
    /// asked of the server -- an RSA pair, and the first person's collection
    /// name. Returns the org's id and key.
    fn create_org(
        &self,
        profile: &Profile,
        account: &Account,
        keys: &VaultKeys,
        first_email: &str,
    ) -> Result<(String, SymmetricKey), SharingError> {
        let org_key = crate::rest::crypto::generate_symmetric_key().map_err(crypto)?;
        let private = crate::rest::sync::private_key_der(keys.user(), profile).map_err(crypto)?;
        let own_public = crate::rest::crypto::public_key_of(&private).map_err(crypto)?;
        let (public_key, encrypted_private_key) = (self.setup().make_key_pair)(&org_key).map_err(crypto)?;
        let body = sharing_api::NewOrganization {
            name: crate::rest::organizations::sharing_org_name(&account.email),
            billing_email: normalise_email(&account.email),
            key: crate::rest::crypto::wrap_key_to_public(&org_key, &own_public).map_err(crypto)?,
            public_key,
            encrypted_private_key,
            collection_name: crate::rest::crypto::encrypt(&org_key, first_email.as_bytes()).map_err(crypto)?,
        };
        let id = self
            .call(|client, auth| sharing_api::create_organization(client, &mut auth.session, &body))
            .map_err(|e| e.or_say(ORG_FAILED))?;
        Ok((id, org_key))
    }

    /// C(E), found by decrypted name or created with this account as its
    /// manager (spec 5.2). Returns its id.
    fn ensure_collection(&self, ctx: &mut Ctx, email: &str) -> Result<String, SharingError> {
        if let Some(id) = ctx.collection_for(email) {
            return Ok(id.to_string());
        }
        let name = crate::rest::crypto::encrypt(&ctx.org_key, email.as_bytes()).map_err(crypto)?;
        let id = self
            .call(|client, auth| sharing_api::create_collection(client, &mut auth.session, &ctx.org_id, &name, &ctx.account.id))
            .map_err(|e| e.or_say(&format!("Couldn't prepare sharing with {email}. Try again.")))?;
        ctx.collections.push((id.clone(), email.to_string()));
        Ok(id)
    }

    /// Whether `spki` may be trusted for `email`: no pin yet, or the same one.
    pub(crate) fn pin_allows(&self, account: &str, email: &str, spki: &[u8]) -> bool {
        let Some(path) = self.setup().state_path else {
            return true;
        };
        let state = crate::sharing_state::SharingState::load(&path).account(account);
        state.pin_for(email).is_none_or(|pin| pin.sha256 == crate::rest::crypto::public_key_sha256_hex(spki))
    }

    /// Pins a key (trust on first use, spec 11.5).
    pub(crate) fn pin(&self, account: &str, email: &str, key: &sharing_api::AccountKey) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        let pin = crate::sharing_state::Pin {
            sha256: crate::rest::crypto::public_key_sha256_hex(&key.public_key),
            user_id: key.user_id.clone(),
            at: now_millis(),
        };
        if let Err(e) = crate::sharing_state::update(&path, |state| state.account_mut(account).set_pin(email, pin)) {
            log::warn!("a sharing pin could not be written: {e}");
        }
    }

    /// **The person's membership, with access to C(E)** (spec 7.2): nothing if
    /// they have it; otherwise the direct add with the key already wrapped
    /// (and pinned), or a pending invitation for an address with no account.
    /// A member without access to their own collection is removed and invited
    /// again (this build's decision 11).
    pub(crate) fn ensure_member(&self, ctx: &Ctx, email: &str, collection: &str) -> Result<ShareResult, SharingError> {
        let roster = self.fetch_roster(&ctx.org_id)?;
        if let Some(member) = roster.iter().find(|m| normalise_email(&m.email) == email) {
            if member.has_access(collection) {
                return Ok(match member.status {
                    MemberStatus::Confirmed => ShareResult::Shared,
                    MemberStatus::Accepted => ShareResult::WaitingAccept,
                    MemberStatus::Invited => ShareResult::WaitingAccount,
                });
            }
            let ou = member.id.clone();
            self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &ou))?;
        }
        let refused = format!("Couldn't invite {email}.");
        match self.call(|client, auth| sharing_api::member_key_by_email(client, &mut auth.session, &ctx.org_id, email))? {
            Some(account_key) => {
                // A changed key is not wrapped to (spec 12.1): they join
                // without the key and the card says their key changed.
                let trusted = self.pin_allows(&ctx.account.id, email, &account_key.public_key);
                let wrapped = if trusted {
                    Some(crate::rest::crypto::wrap_key_to_public(&ctx.org_key, &account_key.public_key).map_err(crypto)?)
                } else {
                    None
                };
                self.call(|client, auth| {
                    sharing_api::invite_member(client, &mut auth.session, &ctx.org_id, email, collection, wrapped.as_ref())
                })
                .map_err(|e| e.or_say(&refused))?;
                if trusted {
                    self.pin(&ctx.account.id, email, &account_key);
                    Ok(ShareResult::Shared)
                } else {
                    self.held().key_changed.insert(email.to_string());
                    Ok(ShareResult::WaitingAccept)
                }
            }
            None => {
                self.call(|client, auth| sharing_api::invite_member(client, &mut auth.session, &ctx.org_id, email, collection, None))
                    .map_err(|e| e.or_say(&refused))?;
                Ok(ShareResult::WaitingAccount)
            }
        }
    }

    /// **Files a record into C(E)** (spec 9.3). Already in the org: one
    /// `/collections` call with C(E) added. Personal: the whole record moves,
    /// with `organizationId` set and -- when it has its own key -- that key
    /// re-wrapped from the user key to the org key; `encrypt_item` then
    /// re-encrypts every field under whatever key the body now names. A stale
    /// revision is re-read and retried once (spec 13).
    fn file_into(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
        collection: &str,
    ) -> Result<VaultItem, SharingError> {
        let decrypted = |answer: serde_json::Value| -> Result<VaultItem, SharingError> {
            crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
                .map(|(written, _)| written.item)
                .ok_or_else(|| SharingError::Refused(SHARE_FAILED.to_string()))
        };
        if organization_of(&record.item) == Some(ctx.org_id.as_str()) {
            let mut ids: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
            if ids.iter().any(|id| id == collection) {
                return Ok(record.item.clone());
            }
            ids.push(collection.to_string());
            let answer = self
                .call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids))
                .map_err(|e| e.or_say(SHARE_FAILED))?;
            return decrypted(answer);
        }
        let attempt = |record: &crate::rest::sync::DecryptedItem| -> Result<serde_json::Value, SharingError> {
            let mut moved = crate::vault_bridge::with_revision_date_from(&record.item, &record.item);
            moved.other.insert("organizationId".into(), serde_json::Value::String(ctx.org_id.clone()));
            moved.other.remove("collectionIds");
            if let Some(wrapped) = moved.other.get("key").and_then(serde_json::Value::as_str).map(str::to_string) {
                let cipher_key = crate::rest::crypto::unwrap_key_under(&wrapped.parse::<EncString>().map_err(crypto)?, ctx.keys.user())
                    .map_err(crypto)?;
                let rewrapped = crate::rest::crypto::wrap_key_under(&cipher_key, &ctx.org_key).map_err(crypto)?;
                moved.other.insert("key".into(), serde_json::Value::String(rewrapped.to_string()));
            }
            let cipher = crate::rest::write::encrypt_item(&record.carrying(moved), &ctx.keys).map_err(crypto)?;
            let ids = vec![collection.to_string()];
            self.call(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids))
        };
        match attempt(record) {
            Ok(answer) => decrypted(answer),
            Err(SharingError::Refused(said)) if said.to_lowercase().contains("out of date") => {
                let (fresh, _) = self.fetch_record(&record.item.id, &ctx.keys)?;
                decrypted(attempt(&fresh).map_err(|e| e.or_say(SHARE_FAILED))?)
            }
            Err(e) => Err(e.or_say(SHARE_FAILED)),
        }
    }

    /// Reads the org's roster and collections back into `held` after a write,
    /// so the next `GET /sharing` answers what the server now says.
    pub(crate) fn reload_org(&self, ctx: &Ctx) -> Result<(), SharingError> {
        let roster = self.fetch_roster(&ctx.org_id)?;
        let collections = self.collections_of(&ctx.org_id, &ctx.org_key)?;
        self.held().my_org = Some(MyOrg { id: ctx.org_id.clone(), collections, roster });
        Ok(())
    }

    /// **Share one record with one person** (spec 6.2-6.5). Ordered so a
    /// failure leaves the record untouched: nothing moves it until org,
    /// collection and membership exist. Every step reads what is there first,
    /// so a Retry is the same call.
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
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        if email == normalise_email(&account.email) {
            return Err(SharingError::Refused(YOURSELF.to_string()));
        }
        let mut keys = self.keys_from(&profile)?;
        let (record, failures) = self.fetch_record(item_id, &keys)?;
        let existing = self.my_org_id(&profile, &account);
        refuse_unmovable(&record, &failures, existing.as_deref())?;

        let (org_id, org_key, collections) = match existing {
            Some(org_id) => {
                let org_key = keys.organization(&org_id).cloned().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections)
            }
            None => {
                let (org_id, org_key) = self.create_org(&profile, &account, &keys, &email)?;
                keys = Arc::new(keys.with_organization(&org_id, org_key.clone()));
                // A server that made C(E) from `collectionName` (spec 9.5) is
                // adopted here by name, like any other collection.
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections)
            }
        };
        if let Some(path) = self.setup().state_path {
            let (account_id, org) = (account.id.clone(), org_id.clone());
            let was_personal = organization_of(&record.item).is_none();
            let moved = crate::sharing_state::Move { folder_id: record.item.folder_id.clone(), favorite: record.item.favorite };
            let _ = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(&account_id);
                me.sharing_org_id = Some(org);
                if was_personal {
                    me.moves.insert(record.item.id.clone(), moved);
                }
            });
        }
        let mut ctx = Ctx { account, keys, org_id, org_key, collections };
        let collection = self.ensure_collection(&mut ctx, &email)?;
        let result = self.ensure_member(&ctx, &email, &collection)?;
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
            let _ = crate::sent_history::update(&path, |history| history.record(entry));
        }
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a share: {e}");
        }
        Ok(ShareOutcome { result, changed: SharingOutcome { upserted: vec![written], removed: Vec::new() } })
    }
}
```

In `src/rest/organizations.rs`, change `fn map_members(` to `pub(crate) fn map_members(`, with the doc line `/// \`pub(crate)\` for the sharing engine, which reads a fresh roster through the same reader.`

- [ ] **Step 5: The route and the client**

`src/vault_api.rs`:
- Remove the `#[allow(dead_code)]` and its comment from `sharing_failed`.
- In `answer`'s match, add `("POST", ["sharing", "item", id]) => self.share_item(id, request.body),`.
- Add to `impl VaultApi`:

```rust
    /// `POST /sharing/item/{id} {email}` (spec 11.3). What moved is applied
    /// to the daemon's copy and announced; the answer says which of the
    /// dialog's three results it was.
    fn share_item(&self, id: &str, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say who to share with.");
        };
        match engine.share(id, &email) {
            Ok(outcome) => {
                self.publish_outcome(&outcome.changed);
                ApiResponse::ok(serde_json::json!({ "result": outcome.result }))
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

Add this free function beside `folder_name`:

```rust
/// The `email` a sharing body names, or `None`.
fn email_in(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()?
        .get("email")?
        .as_str()
        .map(str::to_string)
        .filter(|email| !email.trim().is_empty())
}
```

`src/daemon_link.rs`, in `impl SharingClient`:

```rust
    /// `POST /sharing/item/{id}` -- share a record with a person.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn share(&self, item_id: &str, email: &str) -> Result<crate::rest::sharing::ShareResult, String> {
        let data = self.post(&format!("/sharing/item/{item_id}"), serde_json::json!({ "email": email }))?;
        serde_json::from_value(data.get("result").cloned().unwrap_or_default())
            .map_err(|_| "The vault service answered something unreadable.".to_string())
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/rest/organizations.rs src/sent_history.rs src/vault_api.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 sent_history && cargo test --lib -j 2 vault_api
```

Expected: every file reports `0`, and all pass.

In `a_first_share_…`, `moved.name` equals `"Acme VPN"`. This proves the echo's ciphertext decrypts under the key the engine just generated and added with `with_organization`.

- [ ] **Step 7: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/rest/organizations.rs deskwarden/src/sent_history.rs deskwarden/src/vault_api.rs deskwarden/src/daemon_link.rs
git commit -m "A record can be shared with a person on NodeWarden

POST /sharing/item/{id} runs spec 6.2-6.5 in the daemon:
- the sharing org, made on the first share with its key wrapped to the
  account's own public key;
- the person's collection, named by their encrypted email and managed
  by the sharer;
- a direct add with the key already wrapped and pinned, or a pending
  invitation when the address has no account;
- the record filed into their collection (/share for a personal
  record, /collections for one already shared).

Nothing moves the record until everything before it exists, and a
failure says so in the dialog's words. Records in the trash or archive,
with attachments, or with a field that did not decrypt are refused
before anything is sent. A share is recorded in sent_history as a
person (SentKind::Share), so the Share dialog suggests from one address
book.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Stop sharing, move the last one back, and finish a half-finished move

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs` (the stop flow, `move_back`, `finish_unfinished`, `refresh` calls it, tests)
- Modify: `deskwarden/src/sent_history.rs` (`rekey_item`)
- Modify: `deskwarden/src/fill_stats.rs` (`rekey`)
- Modify: `deskwarden/src/fill_recall.rs` (`FillRecall::rekey`, `rekey_production`)
- Modify: `deskwarden/src/vault_api.rs` (route `POST /sharing/item/{id}/stop`)
- Modify: `deskwarden/src/daemon_link.rs` (`SharingClient::stop`)

**Interfaces:**
- Produces:
  - `pub struct StopOutcome { pub moved_to: Option<String>, pub changed: SharingOutcome }`
  - `Sharing::stop(&self, item_id: &str, email: &str) -> Result<StopOutcome, SharingError>`
  - `pub(crate) fn stop_with(&self, ctx: &Ctx, item_id: &str, email: &str) -> Result<StopOutcome, SharingError>`, reused by `forget` in Task 11
  - `pub const STOP_FAILED_PREFIX: &str = "Couldn't stop sharing."`
  - `pub const MOVE_BACK_FAILED: &str`
  - `SentHistory::rekey_item(&mut self, old: &str, new: &str)`
  - `FillStats::rekey(&self, old: &str, new: &str)`
  - `FillRecall::rekey(&mut self, old: &str, new: &str)`
  - `fill_recall::rekey_production(old: &str, new: &str)`
  - `SharingClient::stop(&self, item_id: &str, email: &str) -> Result<Option<String>, String>`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/sharing.rs`:

```rust
    fn with_collections(server: &mut MockServer, rows: &[serde_json::Value]) {
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": rows }).to_string())
            .expect_at_least(0)
            .create();
    }

    fn with_roster(server: &mut MockServer, rows: &[serde_json::Value]) {
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(rows))
            .expect_at_least(0)
            .create();
    }

    /// **Stopping one of two people is one collections call** (spec 6.8).
    #[test]
    fn stopping_one_of_two_people_takes_them_off_the_record_and_nothing_else() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna", "c-bob"]).to_string()).create();
        let off = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s1", "Acme VPN", &["c-bob"]).to_string())
            .expect(1)
            .create();
        let creates = server.mock("POST", "/api/ciphers").expect(0).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        off.assert();
        creates.assert();
        assert_eq!(outcome.moved_to, None);
        assert_eq!(outcome.changed.upserted.len(), 1);
    }

    /// **Stopping the last person moves the record home** (spec 6.8, 9.4):
    /// create the personal copy, unfile and delete the org copy, give back its
    /// folder and star, and re-key every local file that held the old id.
    #[test]
    fn stopping_the_last_person_creates_then_deletes_and_rekeys_local_files() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).moves.insert("s1".into(), crate::sharing_state::Move { folder_id: Some("f1".into()), favorite: true });
        }).expect("seeded");
        crate::sent_history::update(&dir.join("sent_history.json"), |h| h.record(crate::sent_history::SentRecord {
            kind: crate::sent_history::SentKind::Share, item_id: "s1".into(), recipient: "anna@example.com".into(), ..Default::default()
        })).expect("seeded");
        std::fs::write(dir.join("fill-stats.json"), r#"{"s1":3}"#).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        let create = server
            .mock("POST", "/api/ciphers")
            .match_request(|request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                body["organizationId"].is_null() && body["folderId"] == "f1" && body["favorite"] == true && body.get("collectionIds").is_none()
            })
            .with_body_from_request(echo("p-new", None, &[]))
            .expect(1)
            .create();
        let unfile = server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).expect(1).create();
        let delete = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        create.assert();
        unfile.assert();
        delete.assert();
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert_eq!(outcome.changed.removed, ["s1"]);
        assert_eq!(outcome.changed.upserted[0].name, "Acme VPN", "it decrypts under the user key again");
        assert_eq!(organization_of(&outcome.changed.upserted[0]), None);
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].item_id, "p-new");
        assert_eq!(std::fs::read_to_string(dir.join("fill-stats.json")).expect("reads"), r#"{"p-new":3}"#);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.moves.is_empty() && state.unfinished.is_empty());
    }

    /// **A half-finished move back is finished on the next pass** (spec 13):
    /// the delete failed, the copy is filed nowhere, and the next pass
    /// deletes it and announces it gone.
    #[test]
    fn a_half_finished_move_back_is_finished_on_the_next_pass() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).create();
        server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).expect(1).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("the personal copy is made; the delete waits");
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert!(outcome.changed.removed.is_empty(), "the org copy is still there");
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.unfinished.len(), 1);

        let retried = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        let pass = engine.refresh(Trigger::Batch).expect("a pass");
        retried.assert();
        assert_eq!(pass.outcome.removed, ["s1"]);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.unfinished.is_empty());
    }
```

Append to `src/fill_stats.rs`'s test module:

```rust
    #[test]
    fn a_count_follows_a_record_to_its_new_id() {
        let path = unique_path("rekey");
        let stats = FillStats::new(path.clone());
        stats.record_fill("old");
        stats.record_fill("old");
        stats.rekey("old", "new");
        assert_eq!(stats.count("new"), 2);
        assert_eq!(stats.count("old"), 0);
        let _ = std::fs::remove_file(path);
    }
```

Append to `src/fill_recall.rs`'s test module:

```rust
    #[test]
    fn a_remembered_record_follows_it_to_its_new_id() {
        let mut recall = FillRecall::new();
        let key = key(0x1234, 900, "app.exe");
        let now = Instant::now();
        recall.remember(key(0x1234, 900, "app.exe"), "old", now);
        recall.rekey("old", "new");
        assert!(matches!(recall.recall(&key, now), Recall::Offer("new")));
        recall.rekey("elsewhere", "x");
        assert!(matches!(recall.recall(&key, now), Recall::Offer("new")), "only the matching id moves");
    }
```

`key(hwnd, pid, exe)` is that test module's own `WindowKey` builder (line ~530).

Append to `src/sent_history.rs`'s tests:

```rust
    #[test]
    fn a_record_moved_back_keeps_its_entries_under_its_new_id() {
        let mut history = SentHistory::default();
        history.record(sent(1, "a", "x@y.z"));
        history.entries[0].item_id = "old".into();
        history.rekey_item("old", "new");
        assert_eq!(history.entries[0].item_id, "new");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 fill_`
Expected: a compile error. `stop`, `rekey` and `rekey_item` are not found.

- [ ] **Step 3: The re-key helpers**

`src/sent_history.rs`, in `impl SentHistory`:

```rust
    /// Every entry for `old` now names `new`: a shared record moved back to
    /// the personal vault takes a new id (spec 9.4, Q9).
    pub fn rekey_item(&mut self, old: &str, new: &str) {
        for entry in &mut self.entries {
            if entry.item_id == old {
                entry.item_id = new.to_string();
            }
        }
    }
```

`src/fill_stats.rs`, in `impl FillStats`:

```rust
    /// Moves `old`'s count to `new` -- a shared record moved back to the
    /// personal vault takes a new id, and "Filled 41 times" should come with
    /// it. Best-effort, like every write here.
    pub fn rekey(&self, old: &str, new: &str) {
        let mut counts = self.load();
        if let Some(count) = counts.remove(old) {
            *counts.entry(new.to_string()).or_insert(0) += count;
            let _ = self.save(&counts);
        }
    }
```

`src/fill_recall.rs`, in `impl FillRecall`:

```rust
    /// The remembered record took a new id (a shared record moved back to the
    /// personal vault). Only the matching id moves.
    pub fn rekey(&mut self, old: &str, new: &str) {
        if let Some(last) = self.last.as_mut().filter(|last| last.item_id == old) {
            last.item_id = new.to_string();
        }
    }
```

After `forget_production`:

```rust
/// [`FillRecall::rekey`] against the running app's memory.
///
/// **A third call shape**, beside record and drop, and argued rather than
/// slipped in: the daemon's sharing engine is the one writer that changes a
/// record's id under a live memory, and without this the next hotkey press
/// would offer an id that no longer resolves -- harmless (the picker opens),
/// and still one wrong card.
pub fn rekey_production(old: &str, new: &str) {
    PRODUCTION.lock().unwrap_or_else(std::sync::PoisonError::into_inner).rekey(old, new);
}
```

- [ ] **Step 4: The stop flow**

Add to `src/rest/sharing.rs` (the refusal sentences go beside the others):

```rust
pub const STOP_FAILED_PREFIX: &str = "Couldn't stop sharing.";
pub const MOVE_BACK_FAILED: &str = "Couldn't move the record back to your vault. It is still shared.";

/// What a stop did.
#[derive(Debug, Clone)]
pub struct StopOutcome {
    /// The record's new id, when the last person was stopped and it came home.
    pub moved_to: Option<String>,
    pub changed: SharingOutcome,
}
```

Add a third `impl Sharing` block:

```rust
// ---- stopping ---------------------------------------------------------------------

impl Sharing {
    /// **Stop sharing a record with one person** (spec 6.8).
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn stop(&self, item_id: &str, email: &str) -> Result<StopOutcome, SharingError> {
        let _one = self.one_at_a_time();
        let ctx = self.context()?;
        let outcome = self.stop_with(&ctx, item_id, email)?;
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a stop: {e}");
        }
        Ok(outcome)
    }

    /// [`Self::stop`] within a context already read -- `forget` stops every
    /// record in one context.
    pub(crate) fn stop_with(&self, ctx: &Ctx, item_id: &str, email: &str) -> Result<StopOutcome, SharingError> {
        let email = normalise_email(email);
        let refused = format!("{STOP_FAILED_PREFIX} {email} can still see it.");
        let Some(collection) = ctx.collection_for(&email).map(str::to_string) else {
            return Ok(StopOutcome { moved_to: None, changed: SharingOutcome::default() });
        };
        let (record, _) = self.fetch_record(item_id, &ctx.keys)?;
        let current: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
        if !current.contains(&collection) {
            return Ok(StopOutcome { moved_to: None, changed: SharingOutcome::default() });
        }
        let remaining: Vec<String> = current.into_iter().filter(|id| *id != collection).collect();
        if !remaining.is_empty() {
            let answer = self
                .call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, item_id, &remaining))
                .map_err(|e| e.or_say(&refused))?;
            let (written, _) = crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
                .ok_or_else(|| SharingError::Refused(refused.clone()))?;
            return Ok(StopOutcome { moved_to: None, changed: SharingOutcome { upserted: vec![written.item], removed: Vec::new() } });
        }
        self.move_back(ctx, &record)
    }

    /// **The last person stopped: the record comes home** (spec 9.4).
    ///
    /// The personal copy is created FIRST, so a failure there changes nothing
    /// (the record is still shared, and the sentence says so). Then the org
    /// copy is filed in nothing -- invisible to everyone but the owner -- and
    /// deleted. A failed delete is remembered and retried every pass
    /// (`unfinished`), and the record is already home either way.
    fn move_back(&self, ctx: &Ctx, record: &crate::rest::sync::DecryptedItem) -> Result<StopOutcome, SharingError> {
        let old = record.item.id.clone();
        let placed = self
            .setup()
            .state_path
            .map(|path| crate::sharing_state::SharingState::load(&path).account(&ctx.account.id))
            .and_then(|state| state.moves.get(&old).cloned())
            .unwrap_or_default();
        let mut home = record.item.clone();
        home.id = String::new();
        if let Some(wrapped) = home.other.get("key").and_then(serde_json::Value::as_str).map(str::to_string) {
            let cipher_key = crate::rest::crypto::unwrap_key_under(&wrapped.parse::<EncString>().map_err(crypto)?, &ctx.org_key).map_err(crypto)?;
            let rewrapped = crate::rest::crypto::wrap_key_under(&cipher_key, ctx.keys.user()).map_err(crypto)?;
            home.other.insert("key".into(), serde_json::Value::String(rewrapped.to_string()));
        }
        home.other.insert("organizationId".into(), serde_json::Value::Null);
        for field in ["collectionIds", "edit", "viewPassword", "permissions", "revisionDate", "lastKnownRevisionDate", "creationDate"] {
            home.other.remove(field);
        }
        home.folder_id = placed.folder_id;
        home.favorite = placed.favorite;
        let cipher = crate::rest::write::encrypt_item(&record.carrying(home), &ctx.keys).map_err(crypto)?;
        let answer = self
            .call(|client, auth| client.create_cipher(&mut auth.session, &cipher))
            .map_err(|e| e.or_say(MOVE_BACK_FAILED))?;
        let (created, _) = crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
            .ok_or_else(|| SharingError::Refused(MOVE_BACK_FAILED.to_string()))?;
        let new = created.item.id.clone();

        // Filed in nothing first, so a failed delete leaves a copy nobody but
        // the owner sees. Best effort: a server that refuses an empty list
        // still gets the delete.
        let _ = self.call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &old, &[]));
        let purged = self.call(|client, auth| client.hard_delete_cipher(&mut auth.session, &old));

        self.rekey_local(&ctx.account.id, &old, &new, purged.is_err());
        let removed = if purged.is_ok() { vec![old] } else { Vec::new() };
        Ok(StopOutcome { moved_to: Some(new), changed: SharingOutcome { upserted: vec![created.item], removed } })
    }

    /// Every local file that holds a record by id follows it to its new one
    /// (decision 14: the daemon owns them), and the move is either done or
    /// remembered as unfinished.
    fn rekey_local(&self, account: &str, old: &str, new: &str, unfinished: bool) {
        let setup = self.setup();
        if let Some(path) = setup.sent_path {
            let _ = crate::sent_history::update(&path, |history| history.rekey_item(old, new));
        }
        if let Some(path) = setup.fill_stats_path {
            crate::fill_stats::FillStats::new(path).rekey(old, new);
        }
        crate::fill_recall::rekey_production(old, new);
        if let Some(path) = setup.state_path {
            let _ = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(account);
                me.moves.remove(old);
                if unfinished {
                    me.unfinished.push(crate::sharing_state::Unfinished::PurgeOrgCopy {
                        org_cipher_id: old.to_string(),
                        personal_id: new.to_string(),
                    });
                }
            });
        }
    }

    /// **The second halves that failed**, retried on every pass (spec 5.4,
    /// 13). An org copy already gone (404) counts as done.
    pub(crate) fn finish_unfinished(&self) -> SharingOutcome {
        let mut outcome = SharingOutcome::default();
        let (Some(path), Some(account)) = (self.setup().state_path, self.held().account.clone()) else {
            return outcome;
        };
        let pending = crate::sharing_state::SharingState::load(&path).account(&account.id).unfinished;
        let mut done = Vec::new();
        for entry in pending {
            let crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id, .. } = &entry;
            match self.call(|client, auth| client.hard_delete_cipher(&mut auth.session, org_cipher_id)) {
                Ok(()) | Err(SharingError::Vault(VaultError::Http(_))) => {
                    outcome.removed.push(org_cipher_id.clone());
                    done.push(entry.clone());
                }
                Err(e) => log::info!("an unfinished move back is still waiting: {e}"),
            }
        }
        if !done.is_empty() {
            let _ = crate::sharing_state::update(&path, |state| {
                state.account_mut(&account.id).unfinished.retain(|u| !done.contains(u));
            });
        }
        outcome
    }
}
```

`rest_error` maps a 404 to `VaultError::Http` (through `VaultError::from_status`), so the `Http(_)` arm is "already gone". A 5xx maps to `Unreachable` and stays pending.

In `refresh`, after the incoming-invitations block and before `Ok(pass)`, add:

```rust
        pass.outcome = self.finish_unfinished();
        pass.changed |= !pass.outcome.is_empty();
```

- [ ] **Step 5: The route and the client**

`src/vault_api.rs`: add `("POST", ["sharing", "item", id, "stop"]) => self.stop_sharing(id, request.body),` and:

```rust
    /// `POST /sharing/item/{id}/stop {email}` (spec 11.3). A move back is
    /// `itemGone(old)` then `item(new)` in one feed batch.
    fn stop_sharing(&self, id: &str, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say who to stop sharing with.");
        };
        match engine.stop(id, &email) {
            Ok(outcome) => {
                self.publish_outcome(&outcome.changed);
                ApiResponse::ok(serde_json::json!({ "movedTo": outcome.moved_to }))
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

`src/daemon_link.rs`, in `impl SharingClient`:

```rust
    /// `POST /sharing/item/{id}/stop`. `Some(new id)` when the record came
    /// back to the personal vault.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn stop(&self, item_id: &str, email: &str) -> Result<Option<String>, String> {
        let data = self.post(&format!("/sharing/item/{item_id}/stop"), serde_json::json!({ "email": email }))?;
        Ok(data.get("movedTo").and_then(serde_json::Value::as_str).map(str::to_string))
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/sent_history.rs src/fill_stats.rs src/fill_recall.rs src/vault_api.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 fill_ && cargo test --lib -j 2 sent_history && cargo test --lib -j 2 vault_api
```
Expected: every file reports `0`, and all pass.

- [ ] **Step 7: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/sent_history.rs deskwarden/src/fill_stats.rs deskwarden/src/fill_recall.rs deskwarden/src/vault_api.rs deskwarden/src/daemon_link.rs
git commit -m "Sharing can be stopped, and the last stop brings the record home

POST /sharing/item/{id}/stop takes one person off a record with one
collections call. Stopping the last person moves the record back to the
personal vault:
- the personal copy is created first, with its folder and star restored
  from sharing_state, so a failure there changes nothing;
- the org copy is then filed in nothing and deleted;
- a failed delete is remembered and finished on the next pass.

The new id is carried into sent_history, fill-stats and the in-memory
fill recall by the daemon, which owns them. The feed announces the move
as itemGone then item in one batch.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Forget a person, and re-invite one

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs` (`forget`, `reinvite`, tests)
- Modify: `deskwarden/src/vault_api.rs` (routes `POST /sharing/person/forget` and `/sharing/person/reinvite`)
- Modify: `deskwarden/src/daemon_link.rs` (`SharingClient::forget`, `reinvite`)

**Interfaces:**
- Produces:
  - `pub struct ForgetOutcome { pub stopped: Vec<String>, pub changed: SharingOutcome }`
  - `Sharing::forget(&self, email: &str, items: &[VaultItem]) -> Result<ForgetOutcome, SharingError>`
  - `Sharing::reinvite(&self, email: &str) -> Result<ShareResult, SharingError>`
  - `SharingClient::forget(&self, email: &str) -> Result<Vec<String>, String>`
  - `SharingClient::reinvite(&self, email: &str) -> Result<ShareResult, String>`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/sharing.rs`:

```rust
    /// **Forget X** (spec 6.8): every share with X stopped -- a record shared
    /// with others loses X, a record shared only with X comes home -- then
    /// X's membership removed, X's collection deleted, and X's pin forgotten.
    #[test]
    fn forgetting_a_person_stops_every_share_removes_them_and_deletes_their_collection() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Both", &["c-anna", "c-bob"]).to_string()).create();
        server.mock("GET", "/api/ciphers/s2").with_body(shared_cipher("s2", "Only anna", &["c-anna"]).to_string()).create();
        let off = server.mock("PUT", "/api/ciphers/s1/collections").match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s1", "Both", &["c-bob"]).to_string()).expect(1).create();
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p2", None, &[])).expect(1).create();
        server.mock("PUT", "/api/ciphers/s2/collections").with_body(shared_cipher("s2", "Only anna", &[]).to_string()).create();
        server.mock("DELETE", "/api/ciphers/s2/delete").with_status(204).create();
        let member = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id").as_str()).with_status(204).expect(1).create();
        let collection = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/collections/c-anna").as_str()).with_status(204).expect(1).create();
        let items: Vec<VaultItem> = [shared_cipher("s1", "Both", &["c-anna", "c-bob"]), shared_cipher("s2", "Only anna", &["c-anna"])]
            .iter()
            .map(|raw| serde_json::from_value(serde_json::json!({ "id": raw["id"], "name": "x", "organizationId": raw["organizationId"], "collectionIds": raw["collectionIds"] })).expect("an item"))
            .collect();
        let outcome = engine.forget("anna@example.com", &items).expect("forgotten");
        off.assert();
        member.assert();
        collection.assert();
        assert_eq!(outcome.stopped, ["s1", "s2"]);
        assert!(outcome.changed.removed.contains(&"s2".to_string()));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("anna@example.com").is_none());
    }

    /// **Re-invite** (spec 6.5): a pending invitation gets a new token and 7
    /// days; a person who is gone is invited again.
    #[test]
    fn reinviting_renews_an_invitation_and_invites_a_gone_person_again() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-carol", "carol@example.com"), person_collection("c-gus", "gus@example.com")]);
        with_roster(&mut server, &[roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"])]);
        let renewed = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/inv-carol/reinvite").as_str()).with_body("").expect(1).create();
        assert_eq!(engine.reinvite("carol@example.com").expect("renewed"), ShareResult::WaitingAccount);
        renewed.assert();

        server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/members/public-key?email=gus%40example.com").as_str()).with_status(404).create();
        let invited = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/members/invite").as_str())
            .with_body(r#"{"id":"inv-gus","status":0,"object":"organizationInvitation"}"#).expect(1).create();
        assert_eq!(engine.reinvite("gus@example.com").expect("invited"), ShareResult::WaitingAccount);
        invited.assert();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing::tests::forgetting`
Expected: a compile error. `forget` and `reinvite` are not found.

- [ ] **Step 3: Implement**

Add to `src/rest/sharing.rs`:

```rust
/// What forgetting a person did.
#[derive(Debug, Clone, Default)]
pub struct ForgetOutcome {
    /// The records (old ids) that stopped being shared with them.
    pub stopped: Vec<String>,
    pub changed: SharingOutcome,
}

// ---- people -----------------------------------------------------------------------

impl Sharing {
    /// **Forget a person** (spec 6.8, the ✕ on a suggestion): every record
    /// shared with them stopped, their membership or invitation removed,
    /// their collection deleted, their pin forgotten. `items` is the daemon's
    /// vault, which says which records are filed in their collection.
    ///
    /// The address book (`sent_history`) is the window's to prune, after it
    /// has deleted the person's Sends -- the existing forget question does
    /// both halves in one answer (spec 11.7).
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure. A step that fails
    /// stops the forget there; Retry is the same call.
    pub fn forget(&self, email: &str, items: &[VaultItem]) -> Result<ForgetOutcome, SharingError> {
        let _one = self.one_at_a_time();
        let ctx = self.context()?;
        let email = normalise_email(email);
        let mut outcome = ForgetOutcome::default();
        if let Some(collection) = ctx.collection_for(&email).map(str::to_string) {
            let records: Vec<String> = items
                .iter()
                .filter(|item| organization_of(item) == Some(ctx.org_id.as_str()))
                .filter(|item| collection_ids_of(item).contains(&collection.as_str()))
                .map(|item| item.id.clone())
                .collect();
            for id in records {
                let stopped = self.stop_with(&ctx, &id, &email)?;
                outcome.stopped.push(id);
                outcome.changed.upserted.extend(stopped.changed.upserted);
                outcome.changed.removed.extend(stopped.changed.removed);
            }
            if let Some(member) = self.fetch_roster(&ctx.org_id)?.into_iter().find(|m| normalise_email(&m.email) == email) {
                self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &member.id))?;
            }
            self.call(|client, auth| sharing_api::delete_collection(client, &mut auth.session, &ctx.org_id, &collection))?;
        }
        if let Some(path) = self.setup().state_path {
            let _ = crate::sharing_state::update(&path, |state| state.account_mut(&ctx.account.id).forget_person(&email));
        }
        {
            let mut held = self.held();
            held.key_changed.remove(&email);
            held.no_key.remove(&email);
        }
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a forget: {e}");
        }
        Ok(outcome)
    }

    /// **Re-invite** (spec 6.1's Re-invite action): a pending invitation is
    /// renewed; anyone else without access to their collection is invited
    /// again the way a share would.
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn reinvite(&self, email: &str) -> Result<ShareResult, SharingError> {
        let _one = self.one_at_a_time();
        let mut ctx = self.context()?;
        let email = normalise_email(email);
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
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a re-invite: {e}");
        }
        Ok(result)
    }
}
```

`src/vault_api.rs`, in the router:

```rust
            ("POST", ["sharing", "person", "forget"]) => self.forget_person(request.body),
            ("POST", ["sharing", "person", "reinvite"]) => self.reinvite_person(request.body),
```

Add these methods:

```rust
    /// `POST /sharing/person/forget {email}` (spec 11.3).
    fn forget_person(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say who to forget.");
        };
        match engine.forget(&email, &self.cache.items()) {
            Ok(outcome) => {
                self.publish_outcome(&outcome.changed);
                ApiResponse::ok(serde_json::json!({ "stopped": outcome.stopped }))
            }
            Err(e) => sharing_failed(&e),
        }
    }

    /// `POST /sharing/person/reinvite {email}` (spec 11.3).
    fn reinvite_person(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say who to invite.");
        };
        match engine.reinvite(&email) {
            Ok(result) => {
                self.feed.publish([Change::Sharing]);
                ApiResponse::ok(serde_json::json!({ "result": result }))
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

`src/daemon_link.rs`, in `impl SharingClient`:

```rust
    /// `POST /sharing/person/forget`. The records (old ids) stopped.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn forget(&self, email: &str) -> Result<Vec<String>, String> {
        let data = self.post("/sharing/person/forget", serde_json::json!({ "email": email }))?;
        Ok(serde_json::from_value(data.get("stopped").cloned().unwrap_or_default()).unwrap_or_default())
    }

    /// `POST /sharing/person/reinvite`.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn reinvite(&self, email: &str) -> Result<crate::rest::sharing::ShareResult, String> {
        let data = self.post("/sharing/person/reinvite", serde_json::json!({ "email": email }))?;
        serde_json::from_value(data.get("result").cloned().unwrap_or_default())
            .map_err(|_| "The vault service answered something unreadable.".to_string())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/vault_api.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 vault_api
```
Expected: every file reports `0`, and all pass.

- [ ] **Step 5: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/vault_api.rs deskwarden/src/daemon_link.rs
git commit -m "A person can be forgotten or re-invited

POST /sharing/person/forget stops every share with the person (records
shared only with them come home), removes their membership or
invitation, deletes their collection and forgets their key pin.
POST /sharing/person/reinvite renews a pending invitation, or invites a
person who is gone again, the way a share would.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 12: Auto-confirm, "Share anyway", and Show fingerprint

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs` (`confirm_pass`, called by `refresh`; `confirm_anyway`; `fingerprint`; tests)
- Modify: `deskwarden/src/vault_api.rs` (routes `POST /sharing/person/confirm` and `/sharing/person/fingerprint`)
- Modify: `deskwarden/src/vault_host.rs` (a HubWorker-level test)
- Modify: `deskwarden/src/daemon_link.rs` (`SharingClient::confirm`, `fingerprint`)

**Interfaces:**
- Consumes:
  - `crypto::{wrap_key_to_public, fingerprint_phrase, public_key_sha256_hex}`
  - `eff_wordlist::load`
  - `sharing_api::{user_public_key, confirm_member, AccountKey}`
  - `crypto::tests::{SDK_FINGERPRINT_USER, SDK_FINGERPRINT_KEY, SDK_FINGERPRINT_PHRASE}` (from Task 2)
- Produces:
  - `Sharing::confirm_anyway(&self, email: &str) -> Result<(), SharingError>`
  - `Sharing::fingerprint(&self, email: &str) -> Result<String, SharingError>`
  - private `Sharing::confirm_pass(&self, trigger: Trigger) -> Result<bool, SharingError>`
  - `SharingClient::confirm(&self, email: &str) -> Result<(), String>`
  - `SharingClient::fingerprint(&self, email: &str) -> Result<String, String>`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/sharing.rs`:

```rust
    fn anna_key() -> String {
        serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string()
    }

    /// **An accepted person is confirmed by the timer, with no sync** (spec
    /// 11.5 trigger 4): the timer reads the roster itself -- its one request
    /// of its own -- then two requests per person: the key, the confirm.
    #[test]
    fn an_accepted_person_is_confirmed_by_the_timer_without_a_sync() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        engine.held().my_org = Some(MyOrg { id: FIXTURE_ORG.into(), collections: vec![("c-anna".into(), "anna@example.com".into())], roster: Vec::new() });
        let roster_read = server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]))
            .expect(1)
            .create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .match_request(|request| request.utf8_lossy_body().contains("\"key\":\"4."))
            .with_body("")
            .expect(1)
            .create();
        assert!(!engine.wants_timer(), "control: nobody is known to be waiting before the roster is read");
        let pass = engine.refresh(Trigger::Timer).expect("a pass");
        roster_read.assert();
        confirm.assert();
        sync.assert();
        assert!(pass.changed);
        let pin = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(pin.pin_for("anna@example.com").is_some(), "the first confirm pins the key");
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::Shared));
        assert!(!engine.wants_timer(), "a confirmed person gives the timer nothing to do");
    }

    /// **The confirm pass reuses the roster the whole read fetched** (spec
    /// 11.5): after `note_directory`, no roster request of its own.
    #[test]
    fn the_confirm_pass_reuses_the_roster_the_sync_fetched() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![("c-anna".into(), "anna@example.com".into())],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        let roster_read = server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).expect(0).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).with_body("").expect(1).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        roster_read.assert();
        confirm.assert();
    }

    /// **A changed public key is not confirmed** (spec 11.5 rule 4, 12.1):
    /// the pin differs, so no key is wrapped to it and the card says so.
    #[test]
    fn a_changed_public_key_is_not_confirmed() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![("c-anna".into(), "anna@example.com".into())],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        confirm.assert();
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::KeyChanged));
    }

    /// **A member whose email has no collection is never confirmed** (spec
    /// 11.5 rule 2): a server-injected accept. Only a holder of the org key
    /// can write a collection name that decrypts, so the server alone cannot
    /// put a stranger in the confirm queue.
    #[test]
    fn a_member_whose_email_has_no_collection_is_never_confirmed() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![("c-anna".into(), "anna@example.com".into())],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("mal-id", Some("mal-id"), "mallory@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        let key = server.mock("GET", "/api/users/mal-id/public-key").expect(0).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        key.assert();
        confirm.assert();
    }

    /// An accepted account with no key yet waits for its first sign-in.
    #[test]
    fn an_account_without_a_key_waits_for_a_first_sign_in() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![("c-anna".into(), "anna@example.com".into())],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_status(404).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::WaitingSignIn));
    }

    /// **"Share anyway"** after a key change: confirmed to the new key, and
    /// the pin follows it.
    #[test]
    fn share_anyway_confirms_the_new_key_and_repins_it() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).with_body("").expect(1).create();
        engine.confirm_anyway("anna@example.com").expect("confirmed");
        confirm.assert();
        let pin = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(
            pin.pin_for("anna@example.com").map(|p| p.sha256.clone()),
            Some(crate::rest::crypto::public_key_sha256_hex(&crate::rest::crypto::tests::fixture_public_spki()))
        );
    }

    /// **Show fingerprint** (spec 9.6): the phrase the person's own app shows,
    /// from their user id and the key the server holds for them -- pinned to
    /// Bitwarden's SDK vector by serving that user and key.
    #[test]
    fn a_persons_fingerprint_is_the_phrase_their_own_app_shows() {
        use crate::rest::crypto::tests::{SDK_FINGERPRINT_KEY, SDK_FINGERPRINT_PHRASE, SDK_FINGERPRINT_USER};
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-ou", Some(SDK_FINGERPRINT_USER), "anna@example.com", 2, &["c-anna"])]);
        server
            .mock("GET", format!("/api/users/{SDK_FINGERPRINT_USER}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(SDK_FINGERPRINT_KEY) }).to_string())
            .create();
        assert_eq!(engine.fingerprint("anna@example.com").expect("a phrase"), SDK_FINGERPRINT_PHRASE);
    }
```

`fingerprint` loads the EFF list from beside the test binary. `build.rs` copies it into `deps/` (Task 2), the same way `password_gen`'s passphrase tests read their list.

In `src/vault_host.rs`, append to `mod tests`:

```rust
    /// **An accepted person is confirmed by the daemon with no window**
    /// (spec 11.5 triggers 1-2): a push reaches the hub worker, the worker's
    /// populate is a whole read, and the pass after it confirms -- all on the
    /// daemon's own thread, with no client connected.
    #[test]
    fn an_accepted_person_is_confirmed_by_the_daemon_with_no_window() {
        use crate::rest::backend::tests::{FIXTURE_ORG, person_collection, roster_row, sharing_org_row, sharing_profile};
        let (mut server, backend) = signed_in_with_no_sync_route();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        let profile = sharing_profile(serde_json::json!([sharing_org_row()]));
        server
            .mock("GET", "/api/sync?excludeDomains=true")
            .with_body(serde_json::json!({
                "object": "sync", "profile": profile.clone(), "folders": [], "ciphers": [],
                "collections": [person_collection("c-anna", "anna@example.com")]
            }).to_string())
            .expect(1)
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] }).to_string())
            .expect(1)
            .create();
        server.mock("GET", "/api/accounts/profile").with_body(profile.to_string()).expect_at_least(0).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect_at_least(0).create();
        let key = serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string();
        server.mock("GET", "/api/users/user-1/public-key").with_body(key.clone()).expect_at_least(0).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(key).expect(1).create();
        let confirm = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .with_body("")
            .expect(1)
            .create();

        let cache = VaultCache::new(backend);
        assert_eq!(cache.populate().expect("the daemon's load"), PopulateOutcome::Populated);
        let api = an_api_over(cache);
        let (hub, pushes) = a_listener_the_test_can_push_to();
        pushes.go_live(true);
        pushes.hear_vault(Announcement::Whole);
        // `now` far enough ahead that the burst reads as settled.
        hub_tick(&hub, &api, Instant::now() + Duration::from_secs(60));
        confirm.assert();
        let page = api.feed().wait_after(Some(0), false, Duration::ZERO);
        assert!(page.changes.contains(&Change::Sharing), "{:?}", page.changes);
    }
```

If `pushes.hear_vault` is spelled differently in `rest::notifications::tests`, use the spelling the test at line ~380 of this file uses. `an_api_over(cache: VaultCache) -> VaultApi` is this module's helper at line ~642.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 vault_host::tests::an_accepted`
Expected: a compile error because `confirm_anyway` and `fingerprint` are missing. Once those are stubbed, the confirm tests fail because no confirm request is made.

- [ ] **Step 3: Implement**

Add to the stopping `impl Sharing` block in `src/rest/sharing.rs`, or in a new one headed `// ---- confirming ----`:

```rust
impl Sharing {
    /// **The confirm pass** (spec 11.5). For each accepted member of this
    /// account's sharing org whose email has a person collection (rule 2:
    /// only a holder of the org key could have named it), fetch the key,
    /// compare it with the pin, and confirm -- wrapping the org key to it --
    /// unless the key changed. The roster is the one the last whole read
    /// fetched, so the whole-read and batch triggers cost no roster request;
    /// the timer reads it fresh, which is its one request of its own.
    fn confirm_pass(&self, trigger: Trigger) -> Result<bool, SharingError> {
        let (Some(org), Some(account)) = ({
            let held = self.held();
            (held.my_org.as_ref().map(|o| o.id.clone()), held.account.clone())
        }) else {
            return Ok(false);
        };
        if trigger == Trigger::Timer {
            let roster = self.fetch_roster(&org)?;
            if let Some(mine) = self.held().my_org.as_mut() {
                mine.roster = roster;
            }
        }
        let candidates: Vec<Member> = {
            let held = self.held();
            let Some(mine) = held.my_org.as_ref() else {
                return Ok(false);
            };
            mine.roster
                .iter()
                .filter(|m| m.status == MemberStatus::Accepted && m.user_id.is_some())
                .filter(|m| mine.collection_for(&m.email).is_some())
                .cloned()
                .collect()
        };
        if candidates.is_empty() {
            return Ok(false);
        }
        let profile = self.profile()?;
        let keys = self.keys_from(&profile)?;
        let Some(org_key) = keys.organization(&org).cloned() else {
            return Ok(false);
        };
        let mut changed = false;
        for member in candidates {
            let email = normalise_email(&member.email);
            let user_id = member.user_id.clone().unwrap_or_default();
            let Some(spki) = self.call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))? else {
                changed |= self.held().no_key.insert(email);
                continue;
            };
            self.held().no_key.remove(&email);
            if !self.pin_allows(&account.id, &email, &spki) {
                changed |= self.held().key_changed.insert(email);
                continue;
            }
            self.confirm_to(&org, &org_key, &account.id, &member, &email, sharing_api::AccountKey { user_id, public_key: spki })?;
            changed = true;
        }
        Ok(changed)
    }

    /// Confirms one member to one key, pins it, and marks them confirmed in
    /// the held roster -- no roster re-read (spec 11.5's cost: two requests per
    /// person, the key and the confirm).
    fn confirm_to(
        &self,
        org: &str,
        org_key: &SymmetricKey,
        account: &str,
        member: &Member,
        email: &str,
        key: sharing_api::AccountKey,
    ) -> Result<(), SharingError> {
        let wrapped = crate::rest::crypto::wrap_key_to_public(org_key, &key.public_key).map_err(crypto)?;
        let ou = member.id.clone();
        self.call(|client, auth| sharing_api::confirm_member(client, &mut auth.session, org, &ou, &wrapped))?;
        self.pin(account, email, &key);
        let mut held = self.held();
        held.key_changed.remove(email);
        if let Some(mine) = held.my_org.as_mut() {
            if let Some(row) = mine.roster.iter_mut().find(|m| m.id == member.id) {
                row.status = MemberStatus::Confirmed;
            }
        }
        Ok(())
    }

    /// **"Share anyway"** (spec 6.1, `keyChanged`): the user has looked at the
    /// fingerprint and trusts the new key. Confirmed to it, and it becomes the
    /// pin.
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn confirm_anyway(&self, email: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        let ctx = self.context()?;
        let email = normalise_email(email);
        let member = self
            .fetch_roster(&ctx.org_id)?
            .into_iter()
            .find(|m| normalise_email(&m.email) == email && m.status == MemberStatus::Accepted)
            .ok_or_else(|| SharingError::Refused(format!("{email} is not waiting to be confirmed.")))?;
        let user_id = member.user_id.clone().unwrap_or_default();
        let spki = self
            .call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))?
            .ok_or_else(|| SharingError::Refused(format!("{email} has not signed in to a Bitwarden app yet.")))?;
        // Re-pinned first, so `confirm_to`'s pin is the new key rather than a
        // refusal against the old one.
        if let Some(path) = self.setup().state_path {
            let _ = crate::sharing_state::update(&path, |state| state.account_mut(&ctx.account.id).forget_person(&email));
        }
        self.confirm_to(&ctx.org_id, &ctx.org_key, &ctx.account.id, &member, &email, sharing_api::AccountKey { user_id, public_key: spki })?;
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a confirm: {e}");
        }
        Ok(())
    }

    /// **The fingerprint phrase** for a person (spec 9.6): their user id and
    /// the key the server holds for them, through Bitwarden's construction.
    /// The EFF list is read now and dropped when this returns.
    ///
    /// # Errors
    ///
    /// No key yet, the word list missing, or the backend's failure.
    pub fn fingerprint(&self, email: &str) -> Result<String, SharingError> {
        let ctx = self.context()?;
        let email = normalise_email(email);
        let member = self
            .fetch_roster(&ctx.org_id)?
            .into_iter()
            .find(|m| normalise_email(&m.email) == email && m.user_id.is_some())
            .ok_or_else(|| SharingError::Refused(format!("{email} has not accepted yet, so there is no key to check.")))?;
        let user_id = member.user_id.unwrap_or_default();
        let spki = self
            .call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))?
            .ok_or_else(|| SharingError::Refused(format!("{email} has not signed in to a Bitwarden app yet.")))?;
        let words = crate::rest::eff_wordlist::load().map_err(|e| SharingError::Refused(e.to_string()))?;
        crate::rest::crypto::fingerprint_phrase(&user_id, &spki, &words).map_err(crypto)
    }
}
```

In `refresh`, after `pass.outcome = self.finish_unfinished();`, add:

```rust
        match self.confirm_pass(trigger) {
            Ok(confirmed) => pass.changed |= confirmed,
            // A confirm that could not be made this time is tried again on the
            // next trigger; the pass itself did what it could.
            Err(e) => log::info!("the confirm pass stopped early: {e}"),
        }
```

`src/vault_api.rs`: add these router arms:

```rust
            ("POST", ["sharing", "person", "confirm"]) => self.confirm_person(request.body),
            ("POST", ["sharing", "person", "fingerprint"]) => self.person_fingerprint(request.body),
```

and these methods:

```rust
    /// `POST /sharing/person/confirm {email}` -- "Share anyway" (spec 11.3).
    fn confirm_person(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say who to confirm.");
        };
        match engine.confirm_anyway(&email) {
            Ok(()) => {
                self.feed.publish([Change::Sharing]);
                ApiResponse::done()
            }
            Err(e) => sharing_failed(&e),
        }
    }

    /// `POST /sharing/person/fingerprint {email}` -> `{phrase}` (spec 11.3).
    fn person_fingerprint(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let Some(email) = email_in(body) else {
            return ApiResponse::refused(400, "Say whose fingerprint to show.");
        };
        match engine.fingerprint(&email) {
            Ok(phrase) => ApiResponse::ok(serde_json::json!({ "phrase": phrase })),
            Err(e) => sharing_failed(&e),
        }
    }
```

`src/daemon_link.rs`, in `impl SharingClient`:

```rust
    /// `POST /sharing/person/confirm` -- "Share anyway".
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn confirm(&self, email: &str) -> Result<(), String> {
        self.post("/sharing/person/confirm", serde_json::json!({ "email": email })).map(|_| ())
    }

    /// `POST /sharing/person/fingerprint` -- the five words.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn fingerprint(&self, email: &str) -> Result<String, String> {
        let data = self.post("/sharing/person/fingerprint", serde_json::json!({ "email": email }))?;
        data.get("phrase")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "The vault service answered something unreadable.".to_string())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/vault_api.rs src/vault_host.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 vault_host && cargo test --lib -j 2 vault_api
```
Expected: every file reports `0`, and all pass.

- [ ] **Step 5: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/vault_api.rs deskwarden/src/vault_host.rs deskwarden/src/daemon_link.rs
git commit -m "The daemon confirms an accepted person on its own, and a changed key is caught

The confirm pass (spec 11.5) runs on every whole read, hub batch and
timer tick. It reads the roster the whole read already fetched (the
timer reads it itself), and only for accepted members whose email has a
person collection. Per person it fetches the key, checks it against the
first-use pin and confirms by wrapping the org key to it: two requests.
An account with no key yet waits for its first sign-in. A key that
differs from the pin is never confirmed on its own.

POST /sharing/person/confirm is Share anyway: confirmed to the new key,
which becomes the pin. POST /sharing/person/fingerprint answers the
person's own five words, pinned to Bitwarden's SDK vector.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Pending invitations: expiry reaches the view, and an admin can mint an invite code

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs` (`mint_invite_code`, tests)
- Modify: `deskwarden/src/vault_api.rs` (route `POST /sharing/invite-code`)
- Modify: `deskwarden/src/daemon_link.rs` (`SharingClient::invite_code`)

**Interfaces:**
- Consumes:
  - `sharing_api::{mint_invite_code, InviteCode}`
  - `MasterKey::password_hash`
- Produces:
  - `Sharing::mint_invite_code(&self, password: &[u8]) -> Result<InviteCode, SharingError>`
  - `pub const NOT_ADMIN: &str`
  - `pub const INVITE_CODE_FAILED: &str`
  - `SharingClient::invite_code(&self, password: &str) -> Result<InviteCode, String>`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `src/rest/sharing.rs`:

```rust
    /// **A pending invitation's expiry reaches the view** through a real
    /// roster read, and the day it passes the person reads as expired.
    #[test]
    fn a_pending_invitation_says_when_it_expires_and_then_that_it_has() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg { id: FIXTURE_ORG.into(), collections: vec![("c-carol".into(), "carol@example.com".into()), ("c-dan".into(), "dan@example.com".into())], roster: Vec::new() });
        let mut carol = roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]);
        carol["invitationExpirationDate"] = serde_json::json!("2999-01-01T00:00:00.000Z");
        let mut dan = roster_row("inv-dan", None, "dan@example.com", 0, &["c-dan"]);
        dan["invitationExpirationDate"] = serde_json::json!("2001-01-01T00:00:00.000Z");
        with_roster(&mut server, &[carol, dan]);
        engine.refresh(Trigger::Timer).expect("a pass");
        let view = engine.view(&[]);
        let carol = view.person("carol@example.com").expect("carol");
        assert_eq!(carol.state, PersonState::WaitingAccount);
        assert_eq!(carol.expires_at.as_deref(), Some("2999-01-01T00:00:00.000Z"));
        assert_eq!(view.person("dan@example.com").map(|p| p.state), Some(PersonState::Expired));
    }

    /// **An administrator mints a registration code** (owner decision Q3 (b)):
    /// the typed password is turned into the server hash with the session's
    /// own master key, and only the hash leaves the process.
    #[test]
    fn an_admin_mints_an_invite_code_with_the_password_hash_and_nobody_else_can() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        let mut admin = sharing_profile(serde_json::json!([]));
        admin["role"] = serde_json::json!("admin");
        server.mock("GET", "/api/accounts/profile").with_body(admin.to_string()).expect_at_least(0).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        // The fixture account's master key is PBKDF2 of "master" with one
        // iteration; its server hash is one more PBKDF2 round keyed by it.
        let expected = crate::rest::crypto::master_key(b"master", FIXTURE_EMAIL, crate::rest::crypto::Kdf::Pbkdf2 { iterations: 1 })
            .expect("the fixture master key")
            .password_hash(b"master");
        let minted = server
            .mock("POST", "/api/admin/invites")
            .match_body(Matcher::PartialJson(serde_json::json!({ "masterPasswordHash": expected.as_str() })))
            .with_status(201)
            .with_body(r#"{"code":"abc","inviteLink":"https://nw.example/#/register?invite=abc","expiresAt":"2026-10-04T00:00:00Z"}"#)
            .expect(1)
            .create();
        let code = engine.mint_invite_code(b"master").expect("minted");
        minted.assert();
        assert_eq!(code.code, "abc");
        assert!(engine.view(&[]).can_mint_invites);

        let (mut other, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        other.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect_at_least(0).create();
        other.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        other
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let never = other.mock("POST", "/api/admin/invites").expect(0).create();
        assert!(matches!(engine.mint_invite_code(b"master"), Err(SharingError::Refused(s)) if s == NOT_ADMIN));
        never.assert();
    }
```

The fixture password and email are `b"master"` and `FIXTURE_EMAIL` (`backend::tests::PASSWORD` and `EMAIL`), and prelogin answers `{"kdf":0,"kdfIterations":1}`. If `master_key` or `Kdf` is not re-exported at `crate::rest::crypto`, use the paths `backend::tests` imports them by.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing::tests::an_admin`
Expected: a compile error, because `mint_invite_code` and `NOT_ADMIN` are not found. The first test may already pass, since the Timer path is from Task 12. That is fine: it pins behaviour this task relies on.

- [ ] **Step 3: Implement**

Add to `src/rest/sharing.rs`:

```rust
pub const NOT_ADMIN: &str = "Only this server's administrator can make an invite code.";
pub const INVITE_CODE_FAILED: &str = "Couldn't make an invite code.";

impl Sharing {
    /// **A NodeWarden registration code** for somebody the sharer wants to
    /// share with who has no account (owner decision Q3 (b)). The server
    /// requires the master password again (`/api/admin/invites`); the typed
    /// password is hashed here with the session's own master key, which is
    /// exactly the hash a login sends, and only that hash is sent.
    ///
    /// # Errors
    ///
    /// Not the administrator, a wrong password (the server's sentence), or
    /// the backend's failure.
    pub fn mint_invite_code(&self, password: &[u8]) -> Result<sharing_api::InviteCode, SharingError> {
        self.require_available()?;
        let admin = self.profile()?.is_server_admin();
        if !admin {
            return Err(SharingError::Refused(NOT_ADMIN.to_string()));
        }
        let hash = {
            let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.master_key.password_hash(password)
        };
        self.call(|client, auth| sharing_api::mint_invite_code(client, &mut auth.session, &hash))
            .map_err(|e| e.or_say(INVITE_CODE_FAILED))
    }
}
```

`src/vault_api.rs`: add the router arm `("POST", ["sharing", "invite-code"]) => self.invite_code(request.body),` and:

```rust
    /// `POST /sharing/invite-code {password}` -> `{code, link, expiresAt}`.
    /// The password is read into a wiped buffer and dropped when this returns;
    /// the engine sends only its hash (decision 2 of the sharing plan).
    fn invite_code(&self, body: &[u8]) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        let password: Option<zeroize::Zeroizing<String>> = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("password").and_then(serde_json::Value::as_str).map(|p| zeroize::Zeroizing::new(p.to_string())));
        let Some(password) = password else {
            return ApiResponse::refused(400, "Type your master password.");
        };
        match engine.mint_invite_code(password.as_bytes()) {
            Ok(code) => ApiResponse::ok(to_value(&code)),
            Err(e) => sharing_failed(&e),
        }
    }
```

`src/daemon_link.rs`:

```rust
    /// `POST /sharing/invite-code` -- a registration code for someone with no
    /// account, minted by an administrator.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn invite_code(&self, password: &str) -> Result<crate::rest::sharing_api::InviteCode, String> {
        let data = self.post("/sharing/invite-code", serde_json::json!({ "password": password }))?;
        serde_json::from_value(data).map_err(|_| "The vault service answered something unreadable.".to_string())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/vault_api.rs src/daemon_link.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 vault_api
```
Expected: every file reports `0`, and all pass.

- [ ] **Step 5: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/vault_api.rs deskwarden/src/daemon_link.rs
git commit -m "An invitation says when it expires, and an admin can mint an invite code

A pending invitation's expiry comes off the roster into the view, and
past it the person reads as expired, both from a real roster read.
POST /sharing/invite-code lets NodeWarden's administrator mint a
registration code for someone with no account (owner decision Q3 (b)).
The server asks for the master password again. The daemon hashes it
with the session's own master key, sends only the hash, and wipes the
password.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 14: Drawn marks: the Share control, the person mark, share pills; list chips; `may_edit`

**Files:**
- Modify: `deskwarden/src/theme.rs` (new section after `paint_send_plane`, line ~7867; tests)
- Modify: `deskwarden/src/vault_window/item_list.rs`
  - `shared_pill` (line 1998);
  - `item_row`'s `shared: bool` becomes `marks: RowMarks<'_>` (lines 2257, 2369-2371);
  - `draw_item_list` gains `people: &RowPeople` (line 1533) and its call to `item_row` (line 1915);
  - `RowCommand::writes_the_record`;
  - tests.
- Modify: every caller of `draw_item_list` (12 sites: `grep -rn "draw_item_list(" src`)
- Modify: `deskwarden/src/vault_bridge.rs` (`may_edit`)

**Interfaces:**
- Produces:
  - `theme::share_record_button(ui: &mut Ui) -> Response`
  - `theme::paint_person_mark(painter: &egui::Painter, rect: Rect, color: Color32)`
  - `theme::SharePillKind { Shared, Invited, NoAccount, Expired, KeyChanged, Gone }`, with `label(self) -> &'static str` and `tone(self) -> PillTone`
  - `theme::share_pill_width(painter: &egui::Painter, kind: SharePillKind) -> f32`
  - `theme::share_pill(painter: &egui::Painter, left_center: Pos2, kind: SharePillKind) -> Rect`
  - `item_list::RowMarks<'a> { pub send: bool, pub person: bool, pub from: Option<&'a str> }`
  - `item_list::RowPeople { pub shared: HashSet<String>, pub received: HashMap<String, String> }`, with `from_view(view: Option<&SharingView>) -> Self` and `from(&self, item_id) -> Option<&str>`
  - `item_list::VIEW_ONLY_CHIP: &str = "view-only"`
  - `RowCommand::writes_the_record(&self) -> bool`
  - `vault_bridge::may_edit(item: &VaultItem) -> bool`

- [ ] **Step 1: Write the failing tests**

Append to `theme.rs`'s test module:

```rust
    /// Every share pill says its word and is wider than its word: the person
    /// mark and the padding are really laid out. Five-point corners are the
    /// shape that tells it from a Send's stadium pill (mock 6e).
    #[test]
    fn a_share_pill_is_its_word_and_a_person_mark() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            for kind in [SharePillKind::Shared, SharePillKind::Invited, SharePillKind::NoAccount, SharePillKind::Expired, SharePillKind::KeyChanged, SharePillKind::Gone] {
                let rect = share_pill(ui.painter(), egui::pos2(10.0, 20.0), kind);
                assert_eq!(rect.height(), PILL_HEIGHT);
                assert!(rect.width() > share_pill_width(ui.painter(), kind) - 0.5 && rect.width() > 30.0);
            }
        });
        assert_eq!(SharePillKind::Shared.label(), "Shared");
        assert_eq!(SharePillKind::NoAccount.label(), "No account yet");
        assert_eq!(SharePillKind::Expired.label(), "Invite expired");
        assert_eq!(SharePillKind::Shared.tone().fill, BLUE, "Shared is the filled blue of the list's `shared` chip");
    }

    /// The Share control is a 34-point header control that reports a click,
    /// as every control in that strip does.
    #[test]
    fn the_share_control_is_a_header_sized_button() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let response = share_record_button(ui);
            assert_eq!(response.rect.size(), Vec2::splat(HEADER_BUTTON_HEIGHT));
        });
    }
```

Append to `item_list.rs`'s test module:

```rust
    /// **A person-shared record wears the `shared` pill with a person mark**,
    /// a received one the `view-only` chip (mock 6e, LIST CHIPS). The person
    /// mark wins when a record has both a person and a live Send.
    #[test]
    fn a_shared_record_wears_the_person_pill_and_a_received_one_says_view_only() {
        let view = crate::rest::sharing::SharingView {
            available: true,
            people: vec![crate::rest::sharing::PersonEntry { email: "anna@x.io".into(), state: crate::rest::sharing::PersonState::Shared, expires_at: None }],
            records: vec![crate::rest::sharing::RecordEntry { item_id: "mine".into(), people: vec!["anna@x.io".into()] }],
            received: vec![crate::rest::sharing::ReceivedEntry { item_id: "theirs".into(), from: "denis@x.io".into() }],
            ..Default::default()
        };
        let people = RowPeople::from_view(Some(&view));
        assert!(people.shared.contains("mine"));
        assert_eq!(people.from("theirs"), Some("denis@x.io"));
        assert_eq!(people.from("mine"), None);

        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        crate::theme::apply(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let icons = IconCache::default();
            item_row(ui, &login("theirs", "u"), &[], false, icons.textures.get("theirs"), false, FilterSource::Live,
                RowMarks { send: false, person: false, from: people.from("theirs") });
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        assert!(texts.iter().any(|t| t == VIEW_ONLY_CHIP), "{texts:?}");
    }

    /// Which row commands write to a record -- the ones a received,
    /// view-only record refuses (spec 6.9).
    #[test]
    fn a_view_only_record_refuses_every_command_that_writes() {
        assert!(RowCommand::Edit.writes_the_record());
        assert!(RowCommand::Delete.writes_the_record());
        assert!(RowCommand::MoveToFolder("f".into()).writes_the_record());
        assert!(RowCommand::SelectIcon.writes_the_record());
        assert!(!RowCommand::CopyPassword.writes_the_record());
        assert!(!RowCommand::RefreshIcon.writes_the_record(), "a refresh forgets a cached favicon; the record is untouched");
    }
```

The test needs `IconCache::default()`, `FilterSource::Live` and `login(..)`. If `IconCache` has no `Default`, use the constructor the other row tests in this module use, and use whichever `FilterSource` variant they pass.

Append to `vault_bridge.rs`'s test module:

```rust
    /// **View-only is the server's `edit: false`** on a cipher (spec 6.9,
    /// 10.3). Missing or true is editable -- every personal item.
    #[test]
    fn an_item_the_server_says_is_not_editable_is_view_only() {
        let item = |other: serde_json::Value| -> VaultItem {
            let mut value = serde_json::json!({ "id": "a", "name": "a" });
            value.as_object_mut().expect("object").extend(other.as_object().expect("object").clone());
            serde_json::from_value(value).expect("item")
        };
        assert!(may_edit(&item(serde_json::json!({}))));
        assert!(may_edit(&item(serde_json::json!({ "edit": true }))));
        assert!(!may_edit(&item(serde_json::json!({ "edit": false }))));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 theme::tests::a_share && cargo test --lib -j 2 vault_bridge::tests::an_item_the_server`
Expected: a compile error. `share_pill`, `RowPeople` and `may_edit` are not found.

- [ ] **Step 3: Implement the theme section**

After `paint_send_plane` in `theme.rs`:

```rust
// ---- personal sharing's marks (design 6, tab 6e) ------------------------------
//
// A Send is a link: a square tile, a paper plane, a stadium pill. A share is a
// person: a round avatar, a person mark, a pill with 5-point corners. The two
// must not be confused, so every mark here differs from its Send sibling in
// shape and not only in colour.

/// The person mark: a head and shoulders, in a 24-unit box scaled to `rect`
/// -- the mock's `<circle cx=12 cy=8 r=4/><path d="M4.5 21c.6-4 ...">`,
/// stroked, never filled (a filled path would read as a star to
/// `icon_probe::stars`).
pub fn paint_person_mark(painter: &egui::Painter, rect: Rect, color: Color32) {
    let unit = rect.width() / 24.0;
    let at = |ux: f32, uy: f32| rect.min + Vec2::new(ux * unit, uy * unit);
    let stroke = Stroke::new(2.4 * unit, color);
    painter.circle_stroke(at(12.0, 8.0), 4.0 * unit, stroke);
    painter.add(egui::Shape::line(
        vec![
            at(4.5, 21.0),
            at(5.4, 18.2),
            at(7.4, 16.1),
            at(9.6, 15.0),
            at(12.0, 14.6),
            at(14.4, 15.0),
            at(16.6, 16.1),
            at(18.6, 18.2),
            at(19.5, 21.0),
        ],
        stroke,
    ));
}

/// **The header's Share control** (mock 6a): a person with a plus, drawn as
/// the envelope beside it is -- 34 points, borderless, `ICON_STROKE`,
/// darkening to ink on hover. Shares a record with a person; see
/// `vault_window::share_ui`.
pub fn share_record_button(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let stroke = Stroke::new(ICON_STROKE, color);
    // The mock's 21px glyph in a 24-unit box, centred in the 34pt control.
    let size = 21.0;
    let unit = size / 24.0;
    let origin = rect.center() - Vec2::splat(size / 2.0);
    let at = |ux: f32, uy: f32| origin + Vec2::new(ux * unit, uy * unit);
    let painter = ui.painter();
    painter.circle_stroke(at(9.5, 8.2), 3.7 * unit, stroke);
    painter.add(egui::Shape::line(
        vec![at(3.0, 20.2), at(4.3, 17.1), at(6.4, 15.0), at(9.5, 14.2), at(11.4, 14.4), at(13.0, 15.1)],
        stroke,
    ));
    painter.line_segment([at(18.0, 12.8), at(18.0, 19.8)], stroke);
    painter.line_segment([at(14.5, 16.3), at(21.5, 16.3)], stroke);
    response
}

/// Where one person stands, as a pill says it. Short words; the status line
/// beside the pill carries the long form (mock 6e, SHARE PILLS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePillKind {
    Shared,
    Invited,
    NoAccount,
    Expired,
    KeyChanged,
    Gone,
}

impl SharePillKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Shared => "Shared",
            Self::Invited => "Invited",
            Self::NoAccount => "No account yet",
            Self::Expired => "Invite expired",
            Self::KeyChanged => "Key changed",
            Self::Gone => "Not receiving",
        }
    }

    /// Shared is filled blue, like the list's `shared` chip; the waits are the
    /// Send pills' own washes; a changed key is the danger wash.
    #[must_use]
    pub fn tone(self) -> PillTone {
        let tone = |fill, edge, ink| PillTone { fill, edge, ink, mark: PillMark::None };
        match self {
            Self::Shared => tone(BLUE, BLUE, CARD),
            Self::Invited => tone(BLUE_WASH, BLUE_EDGE, BLUE_DEEP),
            Self::NoAccount => tone(CAUTION_WASH, CAUTION_EDGE, CAUTION_INK),
            Self::Expired | Self::Gone => tone(CANVAS, BORDER, TEXT_FAINT),
            Self::KeyChanged => tone(DANGER_WASH, DANGER_EDGE, DANGER_INK),
        }
    }
}

const SHARE_PILL_PAD_X: f32 = 7.0;
const SHARE_PILL_MARK: f32 = 10.0;
const SHARE_PILL_GAP: f32 = 5.0;
const SHARE_PILL_RADIUS: u8 = 5;

/// A share pill's width, for right-aligning it.
#[must_use]
pub fn share_pill_width(painter: &egui::Painter, kind: SharePillKind) -> f32 {
    let galley = painter.layout_no_wrap(
        kind.label().to_string(),
        FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into())),
        kind.tone().ink,
    );
    SHARE_PILL_PAD_X * 2.0 + SHARE_PILL_MARK + SHARE_PILL_GAP + galley.size().x + 2.0
}

/// **A share pill**: 20 tall like a Send pill, but 5-point corners and a
/// person mark where a Send pill has a dot.
pub fn share_pill(painter: &egui::Painter, left_center: Pos2, kind: SharePillKind) -> Rect {
    let tone = kind.tone();
    let width = share_pill_width(painter, kind);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - PILL_HEIGHT / 2.0),
        Vec2::new(width, PILL_HEIGHT),
    );
    let rounding = CornerRadius::same(SHARE_PILL_RADIUS);
    painter.rect_filled(rect, rounding, tone.fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, tone.edge), StrokeKind::Inside);
    let mark = Rect::from_min_size(
        Pos2::new(rect.min.x + 1.0 + SHARE_PILL_PAD_X, rect.center().y - SHARE_PILL_MARK / 2.0),
        Vec2::splat(SHARE_PILL_MARK),
    );
    paint_person_mark(painter, mark, tone.ink);
    painter.text(
        Pos2::new(mark.right() + SHARE_PILL_GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        kind.label(),
        FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into())),
        tone.ink,
    );
    rect
}
```

- [ ] **Step 4: Implement the list marks, the command test and `may_edit`**

`vault_bridge.rs`, next to `reprompt_protected`:

```rust
/// **Whether this app may write to the item** -- `false` for a record someone
/// shares with this account, which their server hands over with
/// `edit: false` (spec 6.9, 10.3). A missing field is editable: every
/// personal item, and every item a server that predates the field sends.
///
/// A courtesy, not security: the server refuses the write whatever this says.
/// What this stops is the user pressing Edit and meeting a refusal.
#[must_use]
pub fn may_edit(item: &VaultItem) -> bool {
    item.other.get("edit").and_then(serde_json::Value::as_bool) != Some(false)
}
```

`item_list.rs`:

```rust
impl RowCommand {
    /// Whether the command writes to the record -- every one a received,
    /// view-only record refuses (spec 6.9). A refreshed favicon is a local
    /// cache, not the record; choosing or clearing one writes a custom field.
    #[must_use]
    pub(crate) fn writes_the_record(&self) -> bool {
        matches!(
            self,
            Self::SelectIcon
                | Self::ClearIcon
                | Self::Edit
                | Self::MoveToFolder(_)
                | Self::Unfile
                | Self::Delete
                | Self::Archive
                | Self::Unarchive
                | Self::Restore
                | Self::PurgeForever
        )
    }
}

/// The chip on a row someone shares with this account (mock 6e).
pub const VIEW_ONLY_CHIP: &str = "view-only";

/// **What sharing says about each row**: which records this account shares
/// with someone who has them now, and who shares each received record. Built
/// once per frame from the sharing view, and asked per row by id.
#[derive(Debug, Clone, Default)]
pub struct RowPeople {
    pub shared: std::collections::HashSet<String>,
    pub received: std::collections::HashMap<String, String>,
}

impl RowPeople {
    /// From the daemon's view; empty without one.
    #[must_use]
    pub fn from_view(view: Option<&crate::rest::sharing::SharingView>) -> Self {
        let Some(view) = view else {
            return Self::default();
        };
        Self {
            shared: view
                .records
                .iter()
                .filter(|r| view.shared_with_someone(&r.item_id))
                .map(|r| r.item_id.clone())
                .collect(),
            received: view.received.iter().map(|r| (r.item_id.clone(), r.from.clone())).collect(),
        }
    }

    /// Who shares a received record.
    #[must_use]
    pub fn from(&self, item_id: &str) -> Option<&str> {
        self.received.get(item_id).map(String::as_str)
    }
}

/// A row's trailing marks: a live Send, a person share, a received record.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowMarks<'a> {
    pub send: bool,
    pub person: bool,
    pub from: Option<&'a str>,
}
```

Rename `fn shared_pill(ui: &mut egui::Ui) {` to `fn shared_pill_marked(ui: &mut egui::Ui, person: bool) {`. Inside it, replace `theme::paint_send_plane(ui.painter(), mark, theme::CARD);` with:

```rust
    // The plane for a live Send (5d), the person for a person share (6e):
    // "same chip, person mark. It wins when the record has both."
    if person {
        theme::paint_person_mark(ui.painter(), mark, theme::CARD);
    } else {
        theme::paint_send_plane(ui.painter(), mark, theme::CARD);
    }
```

Then add a wrapper, so any existing test that calls it keeps compiling:

```rust
fn shared_pill(ui: &mut egui::Ui) {
    shared_pill_marked(ui, false);
}
```

`item_row`: change the parameter `shared: bool` to `marks: RowMarks<'_>`. Replace

```rust
                    if shared {
                        shared_pill(ui);
                    }
```

with

```rust
                    if marks.person || marks.send {
                        shared_pill_marked(ui, marks.person);
                    }
                    if marks.from.is_some() {
                        row_badge(ui, VIEW_ONLY_CHIP, selected);
                    }
```

`draw_item_list`: add a parameter after `shared`:

```rust
    // **Design 6**: which records are shared with people and who shares the
    // received ones. Asked per row by id -- a share IS a record, unlike a Send.
    people: &RowPeople,
```

Change the `item_row(…)` call's last argument from `shared.contains(&item.name),` to:

```rust
                                    RowMarks {
                                        send: shared.contains(&item.name),
                                        person: people.shared.contains(&item.id),
                                        from: people.from(&item.id),
                                    },
```

The needle `shared.contains(&item.name)` still occurs exactly once, so `the_list_asks_the_shared_set_about_each_rows_own_name` stays green.

Every other `draw_item_list(` call (tests and `mod.rs`): add `&item_list::RowPeople::default(),` after the `shared` argument (or `&RowPeople::default(),` inside `item_list.rs`'s own tests). Every other `item_row(` call in tests: replace its last `bool` argument `x` with `RowMarks { send: x, ..RowMarks::default() }`. For the production call in `mod.rs`, Task 15 passes a real `RowPeople`; in this task pass `&item_list::RowPeople::default()`.

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/theme.rs src/vault_window/item_list.rs src/vault_bridge.rs src/vault_window/mod.rs
cargo test --lib -j 2 theme:: && cargo test --lib -j 2 vault_window::item_list && cargo test --lib -j 2 vault_bridge
```
Expected: every file reports `0`, and all pass.

- [ ] **Step 6: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/theme.rs deskwarden/src/vault_window/item_list.rs deskwarden/src/vault_bridge.rs deskwarden/src/vault_window/mod.rs
git commit -m "The list and the header can draw a person: the share mark, pills and chips

theme gains the header's Share control (a person with a plus, drawn as
the envelope is), the person mark, and share pills. Share pills have
five-point corners and a person where a Send pill is a stadium with a
dot: a share is a person, a Send is a link.

The list's shared pill wears the person mark for a record shared with
someone who has it now, and a received record wears a view-only chip.
vault_bridge::may_edit reads the server's edit:false, and
RowCommand::writes_the_record names the commands a view-only record
refuses.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: The window holds the sharing view: worker thread, feed, rail counts

**Files:**
- Create: `deskwarden/src/vault_window/share_ui.rs`
- Modify: `deskwarden/src/vault_window/mod.rs`
  - module declaration;
  - frame locals near line 1176;
  - the hub check near line 2005;
  - drains near line 2336;
  - both `sidebar::VaultLists { … }` literals (line ~3289 and the second site);
  - the item list's `draw_item_list` call.
- Modify: `deskwarden/src/vault_window/sidebar.rs` (`VaultLists` fields, `live_only`, `screen_rows` Sent and Shared-with-me counts, the pending dot; tests)
- Modify: `deskwarden/src/job_object.rs`: `THREAD_SPAWN_SITES` (line 1355) gains `("vault_window/share_ui.rs", 1)`

**Interfaces:**
- Consumes: `daemon_link::{SharingClient, sharing_client}`, `rest::sharing::*`, `sent_history::{SentHistory, SentKind}`, `HubListener::sharing_changed`, `theme::SharePillKind`.
- Produces (`crate::vault_window::share_ui`, `pub(crate)` unless noted):
  - `PersonLine { email: String, state: PersonState, line: String }`, with `pill() -> SharePillKind`, `reinvite() -> bool`, `fingerprint() -> bool`, `share_anyway() -> bool`
  - `person_lines(view: Option<&SharingView>, item_id: &str, history: &SentHistory, now_ms: i64, zone: &dyn LocalOffset) -> Vec<PersonLine>`
  - `short_state(state: PersonState, expires_at: Option<&str>, now_ms: i64) -> String`
  - `share_control(view: Option<&SharingView>, linked: bool, item: &VaultItem) -> Option<bool>`
  - `records_shared_with(view: Option<&SharingView>, email: &str, items: &[VaultItem]) -> Vec<String>`
  - `SharingPane` (fields `view`, `dialog`, `stop_confirming`, `stopping`, `fingerprint`, `toast`, `notice`), with:
    - `invalidate()`, `wants_view() -> bool`, `begin_view()`
    - `sent_pairs() -> usize`, `received_count() -> usize`, `has_incoming() -> bool`
    - `confirming_for(item_id) -> Option<&str>`, `stopping_for(item_id) -> Option<&str>`
  - `SharingCall { View, Share { item_id, item_name, email }, Stop { item_id, email }, Reinvite(String), Forget(String), Confirm(String), Fingerprint(String), InviteCode(Zeroizing<String>), Accept(String), Decline(String), Leave(String) }`
  - `SharingReport`
  - `run(client: &SharingClient, call: SharingCall) -> SharingReport`
  - `spawn_sharing_call(ctx: egui::Context, tx: mpsc::Sender<SharingReport>, client: SharingClient, call: SharingCall)`
  - `request(pane: &mut SharingPane, client: Option<&SharingClient>, ctx: &egui::Context, tx: &mpsc::Sender<SharingReport>, call: SharingCall)`
  - `drain(rx: &mpsc::Receiver<SharingReport>, pane: &mut SharingPane, now_seconds: f64)`
  - `VaultLists { …, shared_records: usize, received_records: usize, invitation_waiting: bool }`

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/vault_window/share_ui.rs` with only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::sharing::{PersonEntry, RecordEntry};

    const NOW: i64 = 1_790_000_000_000; // 2026-09-21
    const DAY: i64 = 86_400_000;

    fn view(people: Vec<(&str, PersonState, Option<&str>)>, records: Vec<(&str, Vec<&str>)>) -> SharingView {
        SharingView {
            available: true,
            people: people.into_iter().map(|(e, s, x)| PersonEntry { email: e.into(), state: s, expires_at: x.map(str::to_string) }).collect(),
            records: records.into_iter().map(|(id, p)| RecordEntry { item_id: id.into(), people: p.into_iter().map(str::to_string).collect() }).collect(),
            ..SharingView::default()
        }
    }

    /// **Every status line of mock 6e**, in its words.
    #[test]
    fn each_persons_line_says_where_they_stand_in_the_mocks_words() {
        let expires = crate::local_time::FixedOffset(0);
        let in_six = {
            let parts = crate::local_time::civil_parts(NOW + 6 * DAY - 1);
            format!("{:04}-{:02}-{:02}T{:02}:{:02}:00Z", parts.year, parts.month, parts.day, parts.hour, parts.minute)
        };
        let v = view(
            vec![
                ("anna@x.io", PersonState::Shared, None),
                ("bob@x.io", PersonState::WaitingAccept, None),
                ("carol@x.io", PersonState::WaitingAccount, Some(in_six.as_str())),
                ("dan@x.io", PersonState::Expired, Some("2026-09-01T00:00:00Z")),
                ("eve@x.io", PersonState::KeyChanged, None),
                ("fay@x.io", PersonState::WaitingSignIn, None),
                ("gus@x.io", PersonState::Gone, None),
            ],
            vec![("r1", vec!["anna@x.io", "bob@x.io", "carol@x.io", "dan@x.io", "eve@x.io", "fay@x.io", "gus@x.io"])],
        );
        let mut history = crate::sent_history::SentHistory::default();
        history.record(crate::sent_history::SentRecord {
            kind: crate::sent_history::SentKind::Share,
            item_id: "r1".into(),
            recipient: "anna@x.io".into(),
            sent_at_unix_millis: NOW - 7 * DAY,
            ..Default::default()
        });
        let lines = person_lines(Some(&v), "r1", &history, NOW, &expires);
        let line = |email: &str| lines.iter().find(|l| l.email == email).map(|l| l.line.clone()).expect("a line");
        assert_eq!(line("anna@x.io"), "Shared \u{b7} since 14 Sep");
        assert_eq!(line("bob@x.io"), "Waiting to accept");
        assert_eq!(line("carol@x.io"), "Waiting to create an account \u{b7} 6 days left");
        assert_eq!(line("dan@x.io"), "The invite ran out on 1 Sep \u{b7} they never created an account");
        assert_eq!(line("eve@x.io"), "Their key changed. Check the fingerprint before sharing.");
        assert_eq!(line("fay@x.io"), "Waiting for them to sign in once to a Bitwarden app");
        assert_eq!(line("gus@x.io"), "No longer receiving this");
        let pill = |email: &str| lines.iter().find(|l| l.email == email).map(PersonLine::pill).expect("a pill");
        assert_eq!(pill("anna@x.io"), theme::SharePillKind::Shared);
        assert_eq!(pill("carol@x.io"), theme::SharePillKind::NoAccount);
        assert!(lines.iter().find(|l| l.email == "anna@x.io").is_some_and(PersonLine::fingerprint));
        assert!(lines.iter().find(|l| l.email == "bob@x.io").is_some_and(|l| !l.fingerprint()), "no key held yet (mock 6b)");
        assert!(lines.iter().find(|l| l.email == "dan@x.io").is_some_and(PersonLine::reinvite));
        assert!(lines.iter().find(|l| l.email == "eve@x.io").is_some_and(PersonLine::share_anyway));
        assert_eq!(short_state(PersonState::Shared, None, NOW), "Shared");
        assert_eq!(short_state(PersonState::WaitingAccount, Some(&in_six), NOW), "6 days left");
    }

    /// **Where the Share control goes** (owner decision: NodeWarden only,
    /// a Send elsewhere): shares when available, offers a Send when not or
    /// when the window has no line to the daemon, and is absent on a
    /// received record and before the view is known.
    #[test]
    fn the_share_control_shares_on_nodewarden_offers_a_send_elsewhere_and_is_absent_on_a_received_record() {
        let item = |id: &str| -> VaultItem { serde_json::from_value(serde_json::json!({ "id": id, "name": id })).expect("item") };
        let on = SharingView { available: true, ..SharingView::default() };
        let off = SharingView::unavailable(crate::rest::sharing::REASON_NOT_NODEWARDEN);
        let mut received = on.clone();
        received.received.push(crate::rest::sharing::ReceivedEntry { item_id: "theirs".into(), from: "d@x.io".into() });
        assert_eq!(share_control(Some(&on), true, &item("mine")), Some(true));
        assert_eq!(share_control(Some(&off), true, &item("mine")), Some(false));
        assert_eq!(share_control(None, false, &item("mine")), Some(false), "no line to the daemon: a Send");
        assert_eq!(share_control(None, true, &item("mine")), None, "not known yet");
        assert_eq!(share_control(Some(&received), true, &item("theirs")), None);
    }

    /// **A report updates the pane**: a view arrives, a share that landed
    /// closes the dialog with a toast, a share that failed keeps it open with
    /// the sentence.
    #[test]
    fn reports_update_the_pane_the_way_the_mock_says() {
        let mut pane = SharingPane::default();
        assert!(pane.wants_view(), "a window starts by asking");
        pane.begin_view();
        assert!(!pane.wants_view(), "one question at a time");
        apply(&mut pane, SharingReport::View(Ok(view(vec![("anna@x.io", PersonState::Shared, None)], vec![("r1", vec!["anna@x.io"])]))), 0.0);
        assert_eq!(pane.sent_pairs(), 1);
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        apply(&mut pane, SharingReport::Shared { email: "anna@x.io".into(), result: Ok(ShareResult::Shared) }, 1.0);
        assert!(pane.dialog.is_none());
        assert_eq!(pane.toast.as_ref().map(|(t, _)| t.as_str()), Some("Shared with anna@x.io. It is in their vault now."));
        assert!(pane.wants_view(), "a write makes the view stale");
        pane.dialog = Some(ShareDialog::opening("r1", "Acme VPN"));
        apply(&mut pane, SharingReport::Shared { email: "b@x.io".into(), result: Err("Couldn't share the record. It's still in your vault.".into()) }, 2.0);
        assert_eq!(pane.dialog.as_ref().and_then(|d| d.error.as_deref()), Some("Couldn't share the record. It's still in your vault."));
    }

    /// **The worker reaches the daemon** through the real loopback listener:
    /// the one `std::thread::spawn` in this file carries a call and brings
    /// the report back on the channel.
    #[test]
    fn a_sharing_call_on_the_worker_thread_reaches_the_daemon_and_reports_back() {
        use crate::rest::backend::tests::{sharing_profile, signed_in_with_no_sync_route, sync_payload};
        let (mut server, backend) = signed_in_with_no_sync_route();
        server.mock("GET", "/api/accounts/revision-date").with_body("1700000000000").expect_at_least(0).create();
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(sync_payload()).expect_at_least(0).create();
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/users/user-1/public-key")
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        backend.sharing_engine().set_up(crate::rest::sharing::SharingSetup::none());
        let cache = std::sync::Arc::new(crate::vault_cache::VaultCache::new(backend));
        cache.populate().expect("loaded");
        let api = std::sync::Arc::new(crate::vault_api::VaultApi::new(std::sync::Arc::clone(&cache), std::sync::Arc::new(crate::change_feed::ChangeFeed::with_instance(9))));
        api.sharing_pass(crate::rest::sharing::Trigger::WholeRead);
        let host = crate::vault_host::VaultHost::start(std::sync::Arc::clone(&api)).expect("a port");
        let link = crate::daemon_link::DaemonLink::from_line(&host.link_line_for("window")).expect("a line");
        let (tx, rx) = mpsc::channel();
        spawn_sharing_call(egui::Context::default(), tx, link.sharing(), SharingCall::View);
        match rx.recv_timeout(std::time::Duration::from_secs(20)).expect("a report") {
            SharingReport::View(Ok(view)) => assert!(view.available),
            other => panic!("expected the view, got {other:?}"),
        }
        drop(host);
    }
}
```

Append to `sidebar.rs`'s tests:

```rust
    /// **Sent and Shared with me count records too** (spec 6.10), and a
    /// pending invitation puts a dot on the Shared-with-me row (mock 6d).
    #[test]
    fn the_sharing_rows_count_person_shares_and_received_records() {
        let lists = VaultLists { shared_records: 3, received_records: 2, invitation_waiting: true, ..VaultLists::live_only(&[]) };
        let (painted, _bounds) = scrolled_rail(lists, &[], -6000.0);
        let near = |label: &str| painted.iter().find(|(t, _)| t == label).map(|(_, r)| *r).expect(label);
        let sent = near(SENDS_ROW_LABEL);
        let received = near(RECEIVED_ROW_LABEL);
        assert!(painted.iter().any(|(t, r)| t == "3" && (r.center().y - sent.center().y).abs() < 2.0), "Sent does not count 3: {painted:?}");
        assert!(painted.iter().any(|(t, r)| t == "2" && (r.center().y - received.center().y).abs() < 2.0), "Shared with me does not count 2: {painted:?}");
    }
```

This relies on the Sends list being `None` (unknown) in `live_only`. It then counts the shares alone and draws `3` rather than `–`.

- [ ] **Step 2: Run to verify it fails**

Add `pub mod share_ui;` to `vault_window/mod.rs`'s module list, beside `send_ui`, with the doc `/// Personal sharing: the Share dialog, the people on a record, share rows in Sent and received records in Shared with me.` Then run: `cargo test --lib -j 2 vault_window::share_ui`
Expected: a compile error. `person_lines`, `SharingPane` and the other new names are not found.

- [ ] **Step 3: Write `share_ui.rs`'s production half**

Insert above the test module:

```rust
//! **Personal sharing, as the vault window draws it and asks for it**
//! (design 6, `docs/design/6-sharing-mock.html`).
//!
//! The window holds none of sharing's state of record. It holds the daemon's
//! answer to `GET /sharing` ([`SharingPane::view`]), asks again when the
//! daemon's change feed says `sharing`, and sends every request through
//! [`crate::daemon_link::SharingClient`] on a worker thread
//! ([`spawn_sharing_call`], the one thread this file starts). A window with no
//! line to the daemon has no view, and its Share control offers a Send.
//!
//! What is drawn here -- person lines, the Share dialog, the people block of
//! the SHARING card, share rows in Sent, received rows in Shared with me --
//! is called from `detail`, `send_ui` and `mod`, which own their panes.

use std::sync::mpsc;

use zeroize::Zeroizing;

use crate::daemon_link::SharingClient;
use crate::local_time::LocalOffset;
use crate::rest::sharing::{PersonState, ShareResult, SharingView};
use crate::rest::sharing_api::InviteCode;
use crate::sent_history::{SentHistory, SentKind};
use crate::theme;
use crate::vault_bridge::VaultItem;

// ---- the words (mock 6e's STATUS LINES) -----------------------------------------

pub(crate) const SHARE_TOOLTIP: &str = "Share with a person \u{b7} view-only";
pub(crate) const SHARE_UNAVAILABLE_TOOLTIP: &str =
    "Sharing with a person needs a NodeWarden server. This sends a link instead.";

pub(crate) fn shared_toast(email: &str) -> String {
    format!("Shared with {email}. It is in their vault now.")
}

pub(crate) fn waiting_toast(email: &str, record: &str) -> String {
    format!("{email} was added. {record} appears in their vault once they accept.")
}

const MILLIS_PER_DAY: i64 = 86_400_000;

// ---- one person on one record ---------------------------------------------------------

/// A person as the SHARING card lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonLine {
    pub(crate) email: String,
    pub(crate) state: PersonState,
    /// The status line under the address (mock 6b).
    pub(crate) line: String,
}

impl PersonLine {
    pub(crate) fn pill(&self) -> theme::SharePillKind {
        match self.state {
            PersonState::Shared => theme::SharePillKind::Shared,
            PersonState::WaitingAccept | PersonState::WaitingSignIn => theme::SharePillKind::Invited,
            PersonState::WaitingAccount => theme::SharePillKind::NoAccount,
            PersonState::Expired => theme::SharePillKind::Expired,
            PersonState::KeyChanged => theme::SharePillKind::KeyChanged,
            PersonState::Gone => theme::SharePillKind::Gone,
        }
    }

    /// Re-invite is offered where a fresh invitation helps (spec 6.1).
    pub(crate) fn reinvite(&self) -> bool {
        matches!(self.state, PersonState::WaitingAccount | PersonState::Expired | PersonState::Gone)
    }

    /// Show fingerprint only where a key is held (mock 6b: "only for someone
    /// whose key we hold").
    pub(crate) fn fingerprint(&self) -> bool {
        matches!(self.state, PersonState::Shared | PersonState::KeyChanged)
    }

    pub(crate) fn share_anyway(&self) -> bool {
        self.state == PersonState::KeyChanged
    }
}

/// `14 Sep` in the user's zone.
fn day(millis: i64, zone: &dyn LocalOffset) -> String {
    let parts = crate::local_time::local_parts(millis, zone);
    format!("{} {}", parts.day, crate::local_time::month_name(parts.month))
}

/// Whole days until an ISO instant, rounded up; `None` without one.
fn days_left(expires_at: Option<&str>, now_ms: i64) -> Option<i64> {
    let at = crate::send::parse_iso_utc_millis(expires_at?)?;
    Some(((at - now_ms).max(0) + MILLIS_PER_DAY - 1) / MILLIS_PER_DAY)
}

fn days_words(days: i64) -> String {
    if days == 1 { "1 day left".to_string() } else { format!("{days} days left") }
}

/// The last time this PC shared `item_id` with `email` -- "since" on the card.
/// A share has no server timestamp; this is the local record of it (plan
/// decision 12).
fn since(history: &SentHistory, item_id: &str, email: &str) -> Option<i64> {
    history
        .entries
        .iter()
        .filter(|e| e.kind == SentKind::Share && e.item_id == item_id && e.recipient.trim().eq_ignore_ascii_case(email.trim()))
        .map(|e| e.sent_at_unix_millis)
        .max()
}

/// **The people one record is shared with**, each with its status line.
pub(crate) fn person_lines(
    view: Option<&SharingView>,
    item_id: &str,
    history: &SentHistory,
    now_ms: i64,
    zone: &dyn LocalOffset,
) -> Vec<PersonLine> {
    let Some(view) = view else {
        return Vec::new();
    };
    view.people_of(item_id)
        .into_iter()
        .map(|person| {
            let line = match person.state {
                PersonState::Shared => match since(history, item_id, &person.email) {
                    Some(at) => format!("Shared \u{b7} since {}", day(at, zone)),
                    None => "Shared".to_string(),
                },
                PersonState::WaitingAccept => match since(history, item_id, &person.email) {
                    Some(at) => format!("Waiting to accept \u{b7} invited {}", day(at, zone)),
                    None => "Waiting to accept".to_string(),
                },
                PersonState::WaitingAccount => match days_left(person.expires_at.as_deref(), now_ms) {
                    Some(days) => format!("Waiting to create an account \u{b7} {}", days_words(days)),
                    None => "Waiting to create an account".to_string(),
                },
                PersonState::Expired => match person.expires_at.as_deref().and_then(crate::send::parse_iso_utc_millis) {
                    Some(at) => format!("The invite ran out on {} \u{b7} they never created an account", day(at, zone)),
                    None => "The invite ran out \u{b7} they never created an account".to_string(),
                },
                PersonState::KeyChanged => "Their key changed. Check the fingerprint before sharing.".to_string(),
                PersonState::WaitingSignIn => "Waiting for them to sign in once to a Bitwarden app".to_string(),
                PersonState::Gone => "No longer receiving this".to_string(),
            };
            PersonLine { email: person.email.clone(), state: person.state, line }
        })
        .collect()
}

/// The Sent row's short state (mock 6c: "to anna@example.com · Shared").
pub(crate) fn short_state(state: PersonState, expires_at: Option<&str>, now_ms: i64) -> String {
    match state {
        PersonState::Shared => "Shared".to_string(),
        PersonState::WaitingAccept => "Waiting to accept".to_string(),
        PersonState::WaitingAccount => days_left(expires_at, now_ms).map_or_else(|| "No account yet".to_string(), days_words),
        PersonState::Expired => "Invite expired".to_string(),
        PersonState::KeyChanged => "Key changed".to_string(),
        PersonState::WaitingSignIn => "Waiting for a first sign-in".to_string(),
        PersonState::Gone => "Not receiving".to_string(),
    }
}

/// **The header's Share control** for one record: `Some(true)` shares,
/// `Some(false)` offers a Send instead (sharing is NodeWarden-only; owner's
/// decision), `None` draws nothing -- a received record (view-only), or a
/// window that has not heard from the daemon yet.
pub(crate) fn share_control(view: Option<&SharingView>, linked: bool, item: &VaultItem) -> Option<bool> {
    if !linked {
        return Some(false);
    }
    let view = view?;
    if view.received_from(&item.id).is_some() {
        return None;
    }
    Some(view.available)
}

/// The names of the records shared with `email`, for the forget question
/// (spec 11.7: records and Sends in one list).
pub(crate) fn records_shared_with(view: Option<&SharingView>, email: &str, items: &[VaultItem]) -> Vec<String> {
    let Some(view) = view else {
        return Vec::new();
    };
    let email = crate::rest::organizations::normalise_email(email);
    view.records
        .iter()
        .filter(|r| r.people.iter().any(|p| *p == email))
        .filter_map(|r| items.iter().find(|i| i.id == r.item_id).map(|i| i.name.clone()))
        .collect()
}

// ---- the window's sharing state ----------------------------------------------------

/// **Everything the window keeps about sharing between frames.** The view is
/// the daemon's; the rest is what the user is in the middle of.
#[derive(Default)]
pub(crate) struct SharingPane {
    /// The daemon's last answer to `GET /sharing`.
    pub(crate) view: Option<SharingView>,
    stale: bool,
    fetching: bool,
    /// A write is in flight; one at a time.
    busy: bool,
    /// The Share dialog, while it is open.
    pub(crate) dialog: Option<ShareDialog>,
    /// Stop sharing's step two is up for `(item id, email)`.
    pub(crate) stop_confirming: Option<(String, String)>,
    /// A stop is running for `(item id, email)`.
    pub(crate) stopping: Option<(String, String)>,
    /// The fingerprint shown: `(email, phrase)`.
    pub(crate) fingerprint: Option<(String, String)>,
    /// A toast and when it goes, in egui seconds.
    pub(crate) toast: Option<(String, f64)>,
    /// A failed request's sentence, shown on the card until the next one.
    pub(crate) notice: Option<String>,
}

/// How long a toast stays.
const TOAST_SECONDS: f64 = 5.0;

impl SharingPane {
    /// The daemon said the view changed, or this window changed it.
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Whether the frame should ask for the view now.
    pub(crate) fn wants_view(&self) -> bool {
        !self.fetching && (self.view.is_none() || self.stale)
    }

    /// A view request was started.
    pub(crate) fn begin_view(&mut self) {
        self.fetching = true;
        self.stale = false;
    }

    /// Sent's count of person shares: one per (record, person), as the list
    /// draws them (mock 6c).
    pub(crate) fn sent_pairs(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.records.iter().map(|r| r.people.len()).sum())
    }

    pub(crate) fn received_count(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.received.len())
    }

    pub(crate) fn has_incoming(&self) -> bool {
        self.view.as_ref().is_some_and(|v| !v.incoming.is_empty())
    }

    pub(crate) fn confirming_for(&self, item_id: &str) -> Option<&str> {
        self.stop_confirming.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }

    pub(crate) fn stopping_for(&self, item_id: &str) -> Option<&str> {
        self.stopping.as_ref().filter(|(id, _)| id == item_id).map(|(_, e)| e.as_str())
    }
}

// ---- the calls, on one worker thread each --------------------------------------------

/// One request to the daemon's sharing routes.
pub(crate) enum SharingCall {
    View,
    Share { item_id: String, item_name: String, email: String },
    Stop { item_id: String, email: String },
    Reinvite(String),
    Forget(String),
    Confirm(String),
    Fingerprint(String),
    InviteCode(Zeroizing<String>),
    Accept(String),
    Decline(String),
    Leave(String),
}

/// What came back.
#[derive(Debug)]
pub(crate) enum SharingReport {
    View(Result<SharingView, String>),
    Shared { email: String, result: Result<ShareResult, String> },
    Stopped { email: String, result: Result<Option<String>, String> },
    Done(Result<(), String>),
    Forgot(Result<Vec<String>, String>),
    Fingerprint { email: String, result: Result<String, String> },
    InviteCode(Result<InviteCode, String>),
}

/// **One call, synchronously** -- the worker thread's whole body, and
/// callable from a test without a thread.
pub(crate) fn run(client: &SharingClient, call: SharingCall) -> SharingReport {
    match call {
        SharingCall::View => SharingReport::View(client.view()),
        SharingCall::Share { item_id, email, .. } => {
            let result = client.share(&item_id, &email);
            SharingReport::Shared { email, result }
        }
        SharingCall::Stop { item_id, email } => {
            let result = client.stop(&item_id, &email);
            SharingReport::Stopped { email, result }
        }
        SharingCall::Reinvite(email) => SharingReport::Done(client.reinvite(&email).map(|_| ())),
        SharingCall::Forget(email) => SharingReport::Forgot(client.forget(&email)),
        SharingCall::Confirm(email) => SharingReport::Done(client.confirm(&email)),
        SharingCall::Fingerprint(email) => {
            let result = client.fingerprint(&email);
            SharingReport::Fingerprint { email, result }
        }
        SharingCall::InviteCode(password) => SharingReport::InviteCode(client.invite_code(&password)),
        SharingCall::Accept(id) => SharingReport::Done(client.accept(&id)),
        SharingCall::Decline(id) => SharingReport::Done(client.decline(&id)),
        SharingCall::Leave(org) => SharingReport::Done(client.leave(&org)),
    }
}

/// **The one thread this file starts**: a call, its report on `tx`, a
/// repaint. A panic becomes a failed report rather than a silent nothing --
/// `spawn_reporting`'s rule in `vault_window::mod`. Registered in
/// `job_object`'s `THREAD_SPAWN_SITES`.
pub(crate) fn spawn_sharing_call(
    ctx: egui::Context,
    tx: mpsc::Sender<SharingReport>,
    client: SharingClient,
    call: SharingCall,
) {
    std::thread::spawn(move || {
        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&client, call)))
            .unwrap_or_else(|_| SharingReport::Done(Err("Sharing stopped unexpectedly. Try again.".to_string())));
        let _ = tx.send(report);
        ctx.request_repaint();
    });
}

/// **Every sharing request the window makes goes through here**: one write
/// at a time, and a window with no line to the daemon is told why.
pub(crate) fn request(
    pane: &mut SharingPane,
    client: Option<&SharingClient>,
    ctx: &egui::Context,
    tx: &mpsc::Sender<SharingReport>,
    call: SharingCall,
) {
    let Some(client) = client else {
        pane.notice = Some(SHARE_UNAVAILABLE_TOOLTIP.to_string());
        return;
    };
    match &call {
        SharingCall::View => pane.begin_view(),
        _ if pane.busy => return,
        SharingCall::Stop { item_id, email } => {
            pane.busy = true;
            pane.stopping = Some((item_id.clone(), email.clone()));
        }
        _ => pane.busy = true,
    }
    pane.notice = None;
    spawn_sharing_call(ctx.clone(), tx.clone(), client.clone(), call);
}

/// Takes every report that has arrived. Called once per frame.
pub(crate) fn drain(rx: &mpsc::Receiver<SharingReport>, pane: &mut SharingPane, now_seconds: f64) {
    while let Ok(report) = rx.try_recv() {
        apply(pane, report, now_seconds);
    }
    if pane.toast.as_ref().is_some_and(|(_, until)| *until <= now_seconds) {
        pane.toast = None;
    }
}

/// One report into the pane.
pub(crate) fn apply(pane: &mut SharingPane, report: SharingReport, now_seconds: f64) {
    let toast = |pane: &mut SharingPane, text: String| pane.toast = Some((text, now_seconds + TOAST_SECONDS));
    match report {
        SharingReport::View(result) => {
            pane.fetching = false;
            match result {
                Ok(view) => pane.view = Some(view),
                Err(why) => log::info!("the sharing view could not be read from the daemon: {why}"),
            }
            return;
        }
        SharingReport::Shared { email, result } => match result {
            Ok(ShareResult::Shared) => {
                pane.dialog = None;
                toast(pane, shared_toast(&email));
            }
            Ok(ShareResult::WaitingAccept) => {
                let record = pane.dialog.take().map(|d| d.item_name).unwrap_or_default();
                toast(pane, waiting_toast(&email, &record));
            }
            Ok(ShareResult::WaitingAccount) => {
                if let Some(dialog) = pane.dialog.as_mut() {
                    dialog.in_flight = false;
                    dialog.waiting_account = Some(email);
                }
            }
            Err(sentence) => {
                if let Some(dialog) = pane.dialog.as_mut() {
                    dialog.in_flight = false;
                    dialog.error = Some(sentence);
                } else {
                    pane.notice = Some(sentence);
                }
            }
        },
        SharingReport::Stopped { email, result } => {
            pane.stopping = None;
            pane.stop_confirming = None;
            match result {
                Ok(_) => toast(pane, format!("Stopped sharing with {email}.")),
                Err(sentence) => pane.notice = Some(sentence),
            }
        }
        SharingReport::Done(result) => {
            if let Err(sentence) = result {
                pane.notice = Some(sentence);
            }
        }
        SharingReport::Forgot(result) => {
            if let Err(sentence) = result {
                pane.notice = Some(sentence);
            }
        }
        SharingReport::Fingerprint { email, result } => match result {
            Ok(phrase) => pane.fingerprint = Some((email, phrase)),
            Err(sentence) => pane.notice = Some(sentence),
        },
        SharingReport::InviteCode(result) => {
            if let Some(dialog) = pane.dialog.as_mut() {
                dialog.minting = false;
                dialog.password = Zeroizing::new(String::new());
                match result {
                    Ok(code) => dialog.invite_code = Some(code),
                    Err(sentence) => dialog.error = Some(sentence),
                }
            }
        }
    }
    pane.busy = false;
    pane.stale = true;
}
```

`ShareDialog` is used by `apply` and by the test here, so it is defined in this task. Task 16 draws it:

```rust
/// The Share dialog's state (mock 6a). Drawn by [`draw_share_modal`].
pub(crate) struct ShareDialog {
    pub(crate) item_id: String,
    pub(crate) item_name: String,
    pub(crate) email: String,
    pub(crate) in_flight: bool,
    /// The last attempt's sentence.
    pub(crate) error: Option<String>,
    /// A share that left an invitation for an address with no account.
    pub(crate) waiting_account: Option<String>,
    /// The master password, typed to mint an invite code. Wiped on drop.
    pub(crate) password: Zeroizing<String>,
    pub(crate) minting: bool,
    pub(crate) invite_code: Option<InviteCode>,
    /// The ✕ on a suggestion: forget this person, asked first.
    pub(crate) forget: Option<String>,
}

impl ShareDialog {
    pub(crate) fn opening(item_id: &str, item_name: &str) -> Self {
        Self {
            item_id: item_id.to_string(),
            item_name: item_name.to_string(),
            email: String::new(),
            in_flight: false,
            error: None,
            waiting_account: None,
            password: Zeroizing::new(String::new()),
            minting: false,
            invite_code: None,
            forget: None,
        }
    }
}
```

`SharingClient` has no `accept`, `decline` or `leave` until Task 20. Add them to `daemon_link.rs` now, so `run` compiles; their routes arrive in Task 20:

```rust
    /// `POST /sharing/incoming/{id}/accept`.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn accept(&self, id: &str) -> Result<(), String> {
        self.post(&format!("/sharing/incoming/{id}/accept"), serde_json::json!({})).map(|_| ())
    }

    /// `POST /sharing/incoming/{id}/decline`.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn decline(&self, id: &str) -> Result<(), String> {
        self.post(&format!("/sharing/incoming/{id}/decline"), serde_json::json!({})).map(|_| ())
    }

    /// `POST /sharing/from/{orgId}/leave` -- stop receiving from a sharer.
    ///
    /// # Errors
    ///
    /// The engine's sentence, or the daemon's.
    pub fn leave(&self, organization_id: &str) -> Result<(), String> {
        self.post(&format!("/sharing/from/{organization_id}/leave"), serde_json::json!({})).map(|_| ())
    }
```

- [ ] **Step 4: The thread census**

`src/job_object.rs`, in `THREAD_SPAWN_SITES`, add in path order (after `("vault_window/mod.rs", 9),` and before `("webcam.rs", 1),`):

```rust
    // The vault window's sharing worker: one call to the daemon's /sharing
    // routes per request, reported back on a channel -- `spawn_reporting`'s
    // shape in its own file, because the calls are `share_ui`'s.
    ("vault_window/share_ui.rs", 1),
```

- [ ] **Step 5: The rail**

`sidebar.rs` `VaultLists`: add the fields

```rust
    /// Person shares, one per (record, person) -- added to Sent's count
    /// (spec 6.10). 0 without a sharing view.
    pub shared_records: usize,
    /// Records others share with this account -- added to Shared with me.
    pub received_records: usize,
    /// An invitation is waiting: a dot on Shared with me (mock 6d).
    pub invitation_waiting: bool,
```

In `live_only`, set them to `0`, `0` and `false`.

In `screen_rows`, change Sent's count to:

```rust
        // Sends and person shares in one count (spec 6.10). Unknown Sends
        // with no shares stay unknown; unknown Sends with shares count the
        // shares -- they are known.
        let count = match (lists.sends.map(|c| c.all), lists.shared_records) {
            (Some(sends), shares) => Some(sends + shares),
            (None, 0) => None,
            (None, shares) => Some(shares),
        };
```

Change Shared with me to `Some(lists.received + lists.received_records)`. Replace the Shared-with-me `if sidebar_row(…).clicked() { … }` with:

```rust
        let row = sidebar_row(ui, RECEIVED_ROW_LABEL, Some(lists.received + lists.received_records), on, false, width, 0.0);
        if lists.invitation_waiting {
            // Mock 6d: a 6pt blue dot, left of the count -- somebody is
            // waiting for an answer.
            let at = egui::pos2(row.rect.right() - 34.0, row.rect.center().y);
            ui.painter().circle_filled(at, 3.0, crate::theme::BLUE);
        }
        if row.clicked() {
            screens.select(Screen::Received);
        }
```

- [ ] **Step 6: Wire the window (`vault_window/mod.rs`)**

Next to the other sharing locals (after `let mut forgetting: …`, line ~1177):

```rust
        // **Personal sharing** (design 6). The client is the window's line to
        // the daemon's /sharing routes, or `None` in a window with no line --
        // where Share offers a Send. The pane holds the daemon's view and what
        // the user is in the middle of; the channel brings worker reports back.
        let sharing_client = crate::daemon_link::sharing_client();
        let mut sharing = share_ui::SharingPane::default();
        let (sharing_tx, sharing_rx) = mpsc::channel::<share_ui::SharingReport>();
```

Beside `if hub.as_ref().is_some_and(|hub| hub.sends_changed(Instant::now())) { send_fetch.invalidate(); }` (line ~2005):

```rust
        // The daemon said the sharing view moved: ask for it again, and re-read
        // the address book -- the daemon re-keys it when a record comes home.
        if hub.as_ref().is_some_and(|hub| hub.sharing_changed(Instant::now())) {
            sharing.invalidate();
            if let Some(path) = sent_history_path.as_deref() {
                sent_history = crate::sent_history::SentHistory::load(path);
            }
        }
```

After `drain_send_create(…)` (line ~2339):

```rust
        share_ui::drain(&sharing_rx, &mut sharing, ui.input(|i| i.time));
        if sharing.wants_view() && sharing_client.is_some() {
            share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::View);
        }
```

In both `sidebar::VaultLists { … }` literals, add:

```rust
                    shared_records: sharing.sent_pairs(),
                    received_records: sharing.received_count(),
                    invitation_waiting: sharing.has_incoming(),
```

Just before the item list is drawn, build `let row_people = item_list::RowPeople::from_view(sharing.view.as_ref());`. Pass `&row_people` to `item_list::draw_item_list(…)` in place of the `&item_list::RowPeople::default()` from Task 14.

- [ ] **Step 7: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" --fix src/vault_window/share_ui.rs
python "$TEMP/dw_crlf.py" src/vault_window/mod.rs src/vault_window/sidebar.rs src/job_object.rs src/daemon_link.rs
cargo test --lib -j 2 vault_window::share_ui && cargo test --lib -j 2 vault_window::sidebar && cargo test --lib -j 2 job_object && cargo test --lib -j 2 vault_window::
```

Expected: every file reports `0`, and all pass, including `the_thread_spawn_census_is_exact`.

The frame-schedule census counts `request_repaint_after(` in `mod.rs`'s production half. This task adds none there; the toast's expiry is checked by the existing per-frame schedule.

- [ ] **Step 8: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/mod.rs deskwarden/src/vault_window/sidebar.rs deskwarden/src/job_object.rs deskwarden/src/daemon_link.rs
git commit -m "The vault window holds the daemon's sharing view

vault_window::share_ui keeps the daemon's GET /sharing answer and asks
again when the feed says sharing. It sends every request through one
worker thread: THREAD_SPAWN_SITES gains share_ui.rs, 1. Every person
state is written out in mock 6e's words.

The rail counts person shares in Sent and received records in Shared
with me, and dots Shared with me while an invitation waits. The list's
rows read the view to wear their marks.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: The Share control and the Share dialog

**Files:**
- Modify: `deskwarden/src/vault_window/share_ui.rs` (`draw_share_modal`, `ShareDialogAction`, tests)
- Modify: `deskwarden/src/vault_window/detail.rs`
  - `DetailAction::ShareRecord`;
  - `PeopleSharing` and a new `people` parameter on `draw_detail_read`;
  - `header_controls_for`;
  - the Share control in `draw_controls`;
  - the test harness `Pane`.
- Modify: `deskwarden/src/vault_window/mod.rs`
  - `draw_read_arm` (new `people` parameter) and its callers;
  - the `detail_action_exposes_secrets` arms;
  - `share_asked`;
  - `permit_share`;
  - the dialog block after the record composer;
  - forget via the dialog.
- Modify: `deskwarden/src/vault_window/item_list.rs`: `MODAL_SCRIM_AREAS` gains `"record-share-scrim"`

**Interfaces:**
- Consumes: Task 15's `SharingPane`, `ShareDialog`, `request`, `share_control`, `records_shared_with`; `theme::share_record_button`.
- Produces:
  - `detail::DetailAction::ShareRecord`
  - `detail::PeopleSharing<'a> { share_control: Option<bool>, people: &'a [share_ui::PersonLine], confirming: Option<&'a str>, stopping: Option<&'a str>, fingerprint: Option<(&'a str, &'a str)>, view_only_from: Option<&'a str> }` (`Clone`, `Copy`, `Default`)
  - `share_ui::ShareDialogAction { None, Submit, Cancel, MintCode, Forget(String), Done }`
  - `share_ui::draw_share_modal(ctx: &egui::Context, dialog: &mut ShareDialog, item: Option<&VaultItem>, folder: Option<&str>, history: &SentHistory, view: Option<&SharingView>) -> ShareDialogAction`
  - `mod.rs::permit_share(ctx, item, gate, proof) -> Option<share_ui::ShareDialog>`

- [ ] **Step 1: Write the failing tests**

Append to `share_ui.rs`'s tests:

```rust
    /// A harness frame: the dialog drawn over a 1100x800 window, and every
    /// string it painted.
    fn modal_frame(dialog: &mut ShareDialog, history: &SentHistory, view: Option<&SharingView>, events: Vec<egui::Event>) -> (ShareDialogAction, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |ui| {
            let _ = draw_share_modal(ui.ctx(), dialog, None, None, history, view);
        });
        let mut action = ShareDialogAction::None;
        let output = ctx.run_ui(input(events), |ui| {
            action = draw_share_modal(ui.ctx(), dialog, None, None, history, view);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn collect_texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.visual_bounding_rect())),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, out)),
            _ => {}
        }
    }

    /// **Mock 6a**: the record, the WITH field, what view-only means, Share
    /// and Cancel -- and nothing about organisations anywhere.
    #[test]
    fn the_share_dialog_says_what_the_mock_says_and_never_names_an_organisation() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), None, Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Share Acme VPN"), "{texts:?}");
        assert!(has("RECORD") && has("WITH"));
        assert!(has("They get view-only access"));
        assert!(has(SHARE_BUTTON) && has(CANCEL_BUTTON));
        assert!(!texts.iter().any(|(t, _)| t.to_lowercase().contains("organi")), "the dialog names an organisation: {texts:?}");
    }

    /// Pressing Share with an address reports `Submit`; with none, nothing.
    #[test]
    fn share_submits_only_with_an_address() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), None, Vec::new());
        let at = texts.iter().find(|(t, _)| t == SHARE_BUTTON).map(|(_, r)| r.center()).expect("the Share button");
        let click = |pos| vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ];
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(at));
        assert_eq!(action, ShareDialogAction::None, "an empty address does not submit");
        dialog.email = "anna@example.com".into();
        let (action, _) = modal_frame(&mut dialog, &SentHistory::default(), None, click(at));
        assert_eq!(action, ShareDialogAction::Submit);
    }

    /// **No account** (mock 6a, fourth state): the dialog stays open to say
    /// so, and an administrator is offered an invite code.
    #[test]
    fn a_person_without_an_account_is_said_and_an_admin_can_make_an_invite_code() {
        let mut dialog = ShareDialog::opening("r1", "Acme VPN");
        dialog.waiting_account = Some("carol@example.com".into());
        let admin = SharingView { available: true, can_mint_invites: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&admin), Vec::new());
        let has = |needle: &str| texts.iter().any(|(t, _)| t.contains(needle));
        assert!(has("Waiting for carol@example.com to create an account"), "{texts:?}");
        assert!(has(MAKE_CODE_BUTTON));
        assert!(has(DONE_BUTTON));
        let plain = SharingView { available: true, ..SharingView::default() };
        let (_, texts) = modal_frame(&mut dialog, &SentHistory::default(), Some(&plain), Vec::new());
        assert!(!texts.iter().any(|(t, _)| t == MAKE_CODE_BUTTON), "only an administrator can mint");
    }
```

Append to `detail.rs`'s test module. It uses the existing `Pane` harness, extended in Step 3:

```rust
    /// **The Share control sits right after the envelope** (mock 6a) and
    /// reports `ShareRecord` -- or, where sharing is not offered, `SendRecord`:
    /// the owner's "a Send is offered instead".
    #[test]
    fn the_share_control_shares_or_offers_a_send() {
        let item = login_item();
        for (offered, expected) in [(true, DetailAction::ShareRecord), (false, DetailAction::SendRecord)] {
            let mut pane = Pane::new().with_share_control(offered);
            let frame = pane.idle(&item, &TotpState::NoSecret);
            let envelope = frame.envelopes.first().copied().expect("the envelope");
            // Right of the envelope by one control's pitch.
            let at = egui::pos2(envelope.center().x + theme::HEADER_BUTTON_HEIGHT, envelope.center().y);
            assert_eq!(pane.click(&item, &TotpState::NoSecret, at).action, expected);
        }
    }
```

`login_item()` stands for the login fixture this module's header tests already use; use the same one. `frame.envelopes` holds the envelope's rects. If the probe returns points rather than rects, adapt the one line that reads it.

In `item_list.rs`, add `"record-share-scrim",` to `MODAL_SCRIM_AREAS`, in sorted position after `"record-send-scrim"`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 vault_window::share_ui && cargo test --lib -j 2 vault_window::detail::tests::the_share_control`
Expected: a compile error. `draw_share_modal`, `with_share_control` and `DetailAction::ShareRecord` are not found.

- [ ] **Step 3: The dialog**

Append to `share_ui.rs`'s production half:

```rust
// ---- the Share dialog (mock 6a) ------------------------------------------------------

pub(crate) const SHARE_BUTTON: &str = "Share";
pub(crate) const CANCEL_BUTTON: &str = "Cancel";
pub(crate) const DONE_BUTTON: &str = "Done";
pub(crate) const MAKE_CODE_BUTTON: &str = "Make an invite code";
const RECORD_EYEBROW: &str = "RECORD";
const WITH_EYEBROW: &str = "WITH";
const WITH_HINT: &str = "Their email address";
const SUGGESTIONS_EYEBROW: &str = "SHARED WITH BEFORE";
const APPEARS_IN_SENT: &str = "Appears in Sent";
const MODAL_WIDTH: f32 = 560.0;

fn access_line(record: &str) -> String {
    format!(
        "They get view-only access: they can see, copy and autofill it, but not edit, delete or \
         share it on. When you edit {record}, their copy changes too. You can stop sharing at any \
         time from the record."
    )
}

/// NodeWarden sends no email (plan decision 4), so the no-account band says
/// what really happens next.
fn no_account_band(email: &str, record: &str) -> (String, String) {
    (
        format!("Waiting for {email} to create an account \u{b7} the invitation lasts 7 days."),
        format!(
            "They need an invite code from your server's administrator to sign up. {record} \
             reaches their vault after they sign up and accept, and you can re-invite them from \
             the record."
        ),
    )
}

/// What the dialog asked for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShareDialogAction {
    None,
    Submit,
    Cancel,
    MintCode,
    Forget(String),
    Done,
}

/// **Draws the Share dialog** over the window: scrim, card, the record,
/// the WITH field with suggestions from the one address book (Sends and
/// shares), what view-only means, and Share/Cancel -- or, after a share to an
/// address with no account, the band that says so and Done.
pub(crate) fn draw_share_modal(
    ctx: &egui::Context,
    dialog: &mut ShareDialog,
    item: Option<&VaultItem>,
    folder: Option<&str>,
    history: &SentHistory,
    view: Option<&SharingView>,
) -> ShareDialogAction {
    let mut action = ShareDialogAction::None;
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-share-scrim")));
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-share-modal"))).show(ctx, |ui| {
        theme::modal_drag_handle(ui, theme::MODAL_PLAIN_HEADER_HEIGHT);
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(egui::CornerRadius::same(12))
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .show(ui, |ui| {
                ui.set_width(MODAL_WIDTH);
                egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(18.0), egui::Sense::hover());
                        theme::paint_person_mark(ui.painter(), mark, theme::BLUE);
                        ui.label(theme::bold(format!("Share {}", dialog.item_name), 14.0).color(theme::INK));
                    });
                });
                ui.separator();
                egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 16)).show(ui, |ui| {
                    ui.label(theme::bold(RECORD_EYEBROW, 11.0).color(theme::TEXT_GHOST));
                    egui::Frame::new()
                        .fill(theme::BLUE_WASH)
                        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(theme::bold(dialog.item_name.clone(), 13.0).color(theme::BLUE_DEEP));
                            let kind = item.map(|i| super::detail::ItemKind::of(i).label()).unwrap_or_default();
                            let sub = match folder {
                                Some(folder) => format!("{kind} \u{b7} {folder}"),
                                None => kind.to_string(),
                            };
                            ui.label(egui::RichText::new(sub).size(11.0).color(theme::TEXT_MUTED));
                        });
                    ui.add_space(16.0);
                    ui.label(theme::bold(WITH_EYEBROW, 11.0).color(theme::TEXT_GHOST));
                    let enabled = !dialog.in_flight && dialog.waiting_account.is_none();
                    let field = ui.add_enabled_ui(enabled, |ui| {
                        theme::inline_field(ui, &mut dialog.email, WITH_HINT, ui.available_width(), false)
                    });
                    let field = field.inner;
                    if enabled && field.has_focus() {
                        if let Some(forget) = draw_suggestions(ctx, field.rect, &mut dialog.email, history) {
                            action = ShareDialogAction::Forget(forget);
                        }
                    }
                    if enabled && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !dialog.email.trim().is_empty() {
                        action = ShareDialogAction::Submit;
                    }
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new(access_line(&dialog.item_name)).size(12.0).color(theme::TEXT_SECONDARY));
                    if let Some(error) = &dialog.error {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(error).size(12.0).color(theme::ERROR));
                    }
                });
                if let Some(email) = dialog.waiting_account.clone() {
                    let (head, body) = no_account_band(&email, &dialog.item_name);
                    egui::Frame::new()
                        .fill(theme::CAUTION_WASH)
                        .stroke(egui::Stroke::new(1.0, theme::CAUTION_EDGE))
                        .inner_margin(egui::Margin::symmetric(18, 12))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(theme::bold(head, 12.0).color(theme::CAUTION_INK));
                            ui.label(egui::RichText::new(body).size(12.0).color(theme::CAUTION_INK));
                            if view.is_some_and(|v| v.can_mint_invites) {
                                ui.add_space(8.0);
                                match &dialog.invite_code {
                                    Some(code) => {
                                        let text = code.link.clone().unwrap_or_else(|| code.code.clone());
                                        ui.label(egui::RichText::new(format!("Invite code: {text}")).size(12.0).monospace().color(theme::INK));
                                        if theme::row_button(ui, "Copy").clicked() {
                                            ui.ctx().copy_text(text);
                                        }
                                    }
                                    None => {
                                        ui.label(egui::RichText::new("Your master password, to make a code:").size(12.0).color(theme::CAUTION_INK));
                                        let mut revealed = false;
                                        theme::password_field(ui, &mut dialog.password, &mut revealed);
                                        let can = !dialog.minting && !dialog.password.is_empty();
                                        if ui.add_enabled_ui(can, |ui| theme::secondary_button(ui, MAKE_CODE_BUTTON)).inner.clicked() {
                                            action = ShareDialogAction::MintCode;
                                        }
                                    }
                                }
                            }
                        });
                }
                egui::Frame::new()
                    .fill(theme::CARD_TINT)
                    .inner_margin(egui::Margin::symmetric(18, 13))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if dialog.waiting_account.is_some() {
                                if theme::primary_button(ui, DONE_BUTTON, None).clicked() {
                                    action = ShareDialogAction::Done;
                                }
                            } else {
                                let ready = !dialog.in_flight && !dialog.email.trim().is_empty();
                                let share = ui.add_enabled_ui(!dialog.in_flight, |ui| theme::primary_button(ui, SHARE_BUTTON, Some("\u{21b5}"))).inner;
                                if share.clicked() && ready {
                                    action = ShareDialogAction::Submit;
                                }
                                ui.add_space(theme::FORM_FOOTER_GAP);
                                if theme::secondary_button(ui, CANCEL_BUTTON).clicked() {
                                    action = ShareDialogAction::Cancel;
                                }
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(APPEARS_IN_SENT).size(12.0).color(theme::TEXT_GHOST));
                            });
                        });
                    });
            });
    });
    if action == ShareDialogAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !dialog.in_flight {
        action = ShareDialogAction::Cancel;
    }
    action
}

/// The suggestions under the WITH field: everyone this PC has sent to or
/// shared with, with a count of each kind and the ✕ that forgets a person
/// (mock 6a). Returns the address whose ✕ was pressed.
fn draw_suggestions(ctx: &egui::Context, field: egui::Rect, typed: &mut String, history: &SentHistory) -> Option<String> {
    let people = history.suggestions(typed);
    if people.is_empty() {
        return None;
    }
    let mut forget = None;
    egui::Area::new(egui::Id::new("record-share-suggestions"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(field.left(), field.bottom() + 5.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::CARD)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(9))
                .inner_margin(egui::Margin::same(5))
                .show(ui, |ui| {
                    ui.set_width(field.width() - 10.0);
                    ui.label(theme::bold(SUGGESTIONS_EYEBROW, 10.0).color(theme::TEXT_GHOST));
                    for person in people.iter().take(6) {
                        ui.horizontal(|ui| {
                            let counts = match (person.sent, person.shared) {
                                (0, n) => format!("{n} shared"),
                                (n, 0) => crate::vault_window::send_ui::sent_count_words(n),
                                (s, n) => format!("{n} shared, {s} sent"),
                            };
                            if ui.add(egui::Label::new(egui::RichText::new(&person.email).size(13.0)).sense(egui::Sense::click())).clicked() {
                                *typed = person.email.clone();
                            }
                            ui.label(egui::RichText::new(counts).size(11.0).color(theme::TEXT_FAINT));
                            if ui.add(egui::Label::new(egui::RichText::new("\u{00d7}").size(14.0).color(theme::TEXT_FAINT)).sense(egui::Sense::click())).clicked() {
                                forget = Some(person.email.clone());
                            }
                        });
                    }
                });
        });
    forget
}
```

`ItemKind` is `detail::ItemKind`. If it is private there, use `crate::vault_window::item_list`'s re-export, or make `ItemKind` `pub(crate)` in `detail.rs`. `theme::password_field` takes `&mut String`, and `Zeroizing<String>` derefs to one mutably. If `MODAL_PLAIN_HEADER_HEIGHT` is not `pub`, use the value `record_ui::draw_forget_confirm` uses.

- [ ] **Step 4: The header control and `PeopleSharing` in `detail.rs`**

Add to `pub enum DetailAction`:

```rust
    /// The header's Share control (design 6a). Sets the window's
    /// `share_asked`, like `SendRecord` sets `send_record_asked`; the dialog is
    /// opened through the re-prompt gate there.
    ShareRecord,
```

Add after `SharedSend`:

```rust
/// **Personal sharing, as the read pane draws it** (design 6a, 6b, 6d),
/// resolved by the window from the daemon's view. `Default` is "nothing about
/// sharing": no Share control, no people, an editable record -- the pane every
/// caller that is not about sharing draws.
#[derive(Clone, Copy, Default)]
pub struct PeopleSharing<'a> {
    /// The Share control: `Some(true)` shares, `Some(false)` offers a Send
    /// instead, `None` draws none. See `share_ui::share_control`.
    pub share_control: Option<bool>,
    pub people: &'a [super::share_ui::PersonLine],
    /// Stop sharing's step two is up for this email.
    pub confirming: Option<&'a str>,
    /// A stop is running for this email.
    pub stopping: Option<&'a str>,
    /// `(email, phrase)` of the fingerprint on show.
    pub fingerprint: Option<(&'a str, &'a str)>,
    /// A received record: who shares it. The pane is view-only.
    pub view_only_from: Option<&'a str>,
}
```

Give `draw_detail_read` a last parameter:

```rust
    // **Design 6**: the Share control, the people on the SHARING card, and a
    // received record's view-only line. `PeopleSharing::default()` from every
    // caller that is not about sharing.
    people: PeopleSharing<'_>,
```

Add after `header_controls`:

```rust
/// [`header_controls`] with sharing: a received record draws only the kebab
/// and the close (mock 6d), and the Share control is one more.
fn header_controls_for(item: &VaultItem, people: &PeopleSharing<'_>) -> usize {
    if people.view_only_from.is_some() {
        return 2;
    }
    header_controls(item) + usize::from(people.share_control.is_some())
}
```

At line ~3392, replace `let controls = header_controls(item);` with `let controls = header_controls_for(item, &people);`.

In `draw_controls`, directly before the envelope's `if theme::send_record_button(ui)`:

```rust
                // **Share, right after the envelope** (mock 6a) -- drawn before
                // it because this strip lays out right to left. Where sharing
                // is not offered (not NodeWarden, no line to the daemon) it
                // opens the Send composer: the owner's "a Send is offered
                // instead".
                if let Some(offered) = people.share_control {
                    let hover = if offered {
                        super::share_ui::SHARE_TOOLTIP
                    } else {
                        super::share_ui::SHARE_UNAVAILABLE_TOOLTIP
                    };
                    if theme::share_record_button(ui).on_hover_text(hover).clicked() {
                        action = if offered { DetailAction::ShareRecord } else { DetailAction::SendRecord };
                    }
                }
```

Update the seven `draw_detail_read(` call sites: pass `PeopleSharing::default()` as the new last argument in tests, and `people` in `draw_read_arm`.

Extend the test harness `Pane`:
- add a field `share_control: Option<bool>` (doc: `/// The header's Share control, as \`share_ui::share_control\` would resolve it. None unless a test sets it.`);
- initialise it to `None` in `Pane::wide`'s `Self { … }` literal;
- add a builder:

```rust
        fn with_share_control(mut self, offered: bool) -> Self {
            self.share_control = Some(offered);
            self
        }
```

In `Pane::frame`'s `draw_detail_read(…)` call, pass `PeopleSharing { share_control: self.share_control, ..PeopleSharing::default() }` as the last argument.

- [ ] **Step 5: The window: arm, gate, dialog**

`mod.rs`:
- `draw_read_arm`: add a last parameter `people: detail::PeopleSharing<'_>` (comment: `// Design 6, forwarded -- \`share_ui\` resolves it from the daemon's view.`) and forward it into `draw_detail_read`.
- Its test callers pass `detail::PeopleSharing::default()`.
- Its production caller (line 4916) passes the value built just above it:

```rust
                            let person_lines = share_ui::person_lines(
                                sharing.view.as_ref(),
                                &item.id,
                                &sent_history,
                                crate::send::SendClock::now_unix_millis(&crate::send::SystemClock),
                                &crate::local_time::SystemZone,
                            );
                            let people = detail::PeopleSharing {
                                share_control: share_ui::share_control(sharing.view.as_ref(), sharing_client.is_some(), item),
                                people: &person_lines,
                                confirming: sharing.confirming_for(&item.id),
                                stopping: sharing.stopping_for(&item.id),
                                fingerprint: sharing.fingerprint.as_ref().map(|(e, p)| (e.as_str(), p.as_str())),
                                view_only_from: sharing.view.as_ref().and_then(|v| v.received_from(&item.id)),
                            };
```

In `detail_action_exposes_secrets`, add `| DetailAction::ShareRecord` beside `| DetailAction::SendRecord`, with the comment `// Share, for SendRecord's reason exactly: the exposure is gated where the dialog opens, by \`permit_share\`.`

Declare `let mut share_asked = false;` beside `send_record_asked` (line ~2600). In the detail-action `match`, add:

```rust
                                DetailAction::ShareRecord => {
                                    share_asked = true;
                                }
```

After the `permit_record_send` function, add:

```rust
/// [`permit_record_send`] for the Share dialog: a share hands a record --
/// every field of it -- to another person, so a re-prompt-protected item asks
/// first, before the dialog exists.
fn permit_share(
    ctx: &egui::Context,
    item: &VaultItem,
    gate: &crate::reprompt::RepromptGate,
    proof: &mut crate::reprompt::Proof,
) -> Option<share_ui::ShareDialog> {
    let protected = crate::vault_bridge::reprompt_protected(item);
    match crate::reprompt::permit(gate, protected, proof, Instant::now(), || {
        share_ui::ShareDialog::opening(&item.id, &item.name)
    }) {
        crate::reprompt::Outcome::Done(dialog) => Some(dialog),
        crate::reprompt::Outcome::Refused => {
            detail::note_refused(ctx, crate::reprompt::refusal_text(false));
            None
        }
        crate::reprompt::Outcome::Cannot => {
            detail::note_refused(ctx, crate::reprompt::refusal_text(true));
            None
        }
    }
}
```

After the `if send_record_asked { … }` block, add:

```rust
        // **The Share dialog** (design 6a), opened through the re-prompt gate
        // and drawn last, over the three panels, as the record composer is.
        if share_asked {
            if let Some(item) = selected_id.as_ref().and_then(|id| items.iter().find(|i| &i.id == id)) {
                sharing.dialog = permit_share(ui.ctx(), item, &reprompt_gate, &mut reprompt_proof);
            }
        }
        if let Some(mut dialog) = sharing.dialog.take() {
            let item = items.iter().find(|i| i.id == dialog.item_id);
            let folder = item.and_then(|item| sidebar::folder_name(&folders, item.folder_id.as_deref()));
            let answer = share_ui::draw_share_modal(ui.ctx(), &mut dialog, item, folder, &sent_history, sharing.view.as_ref());
            let mut keep = true;
            match answer {
                share_ui::ShareDialogAction::None => {}
                share_ui::ShareDialogAction::Cancel | share_ui::ShareDialogAction::Done => keep = false,
                share_ui::ShareDialogAction::Submit => {
                    dialog.in_flight = true;
                    dialog.error = None;
                    let call = share_ui::SharingCall::Share {
                        item_id: dialog.item_id.clone(),
                        item_name: dialog.item_name.clone(),
                        email: dialog.email.trim().to_string(),
                    };
                    sharing.dialog = Some(dialog);
                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, call);
                    keep = false;
                    // `request` may have put the dialog back into the pane: it
                    // is already where the report will find it.
                }
                share_ui::ShareDialogAction::MintCode => {
                    dialog.minting = true;
                    let password = std::mem::replace(&mut dialog.password, zeroize::Zeroizing::new(String::new()));
                    sharing.dialog = Some(dialog);
                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::InviteCode(password));
                    keep = false;
                }
                share_ui::ShareDialogAction::Forget(email) => {
                    forget_request_for(&email, &mut forget_ask, &mut forget_records, &send_fetch, &mut sent_history, sent_history_path.as_deref(), sharing.view.as_ref(), &items);
                }
            }
            if keep {
                sharing.dialog = Some(dialog);
            }
        }
```

Refactor the record composer's forget block (the `if let Some(email) = state.draft.forget.take() { … }` body) into this free function in `mod.rs`, and call it from both places:

```rust
/// **The forget question's two lists** (spec 11.7): the person's live Sends,
/// asked and deleted through the Sent screen's one door, and the records
/// shared with them, stopped by the daemon's `/sharing/person/forget` when
/// the answer is yes. Nothing to ask about forgets at once.
#[allow(clippy::too_many_arguments)]
fn forget_request_for(
    email: &str,
    forget_ask: &mut Option<(String, Vec<(String, String)>)>,
    forget_records: &mut Vec<String>,
    send_fetch: &send_ui::SendFetch,
    sent_history: &mut crate::sent_history::SentHistory,
    sent_history_path: Option<&std::path::Path>,
    sharing: Option<&crate::rest::sharing::SharingView>,
    items: &[VaultItem],
) {
    let listed: Option<Vec<String>> = match &send_fetch.result {
        Some(Ok(rows)) => Some(rows.iter().map(|row| row.id.clone()).collect()),
        _ => None,
    };
    let shares: Vec<(String, String)> = sent_history
        .entries
        .iter()
        .filter(|entry| entry.kind == crate::sent_history::SentKind::Send)
        .filter(|entry| entry.recipient.trim().eq_ignore_ascii_case(email.trim()))
        .filter(|entry| listed.as_ref().is_none_or(|ids| ids.contains(&entry.send_id)))
        .map(|entry| (entry.send_id.clone(), entry.name.clone()))
        .collect();
    let records = share_ui::records_shared_with(sharing, email, items);
    if shares.is_empty() && records.is_empty() {
        sent_history.forget(email);
        if let Some(path) = sent_history_path {
            if let Err(e) = crate::sent_history::update(path, |h| {
                h.forget(email);
            }) {
                log::warn!("a person could not be forgotten in the sent history: {e:?}");
            }
        }
    } else {
        *forget_ask = Some((email.to_string(), shares));
        *forget_records = records;
    }
}
```

Declare `let mut forget_records: Vec<String> = Vec::new();` beside `forget_ask`. In the forget-confirm block:
- build the names from both lists: `let mut names: Vec<String> = shares.iter().map(|(_, name)| name.clone()).collect(); names.extend(forget_records.iter().cloned());`
- in the `DeleteShares` arm, before `forgetting = forget_ask.take();`, add:

```rust
                    if !forget_records.is_empty() {
                        share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Forget(email.clone()));
                        forget_records.clear();
                    }
```

- in the `Keep` arm, add `forget_records.clear();`.

`record_ui::forget_body` counts `names.len()` records; records and Sends read the same there ("N records are shared with this person"), which is what spec 11.7 asks.

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/detail.rs src/vault_window/mod.rs src/vault_window/item_list.rs
cargo test --lib -j 2 vault_window::
```

Expected: every file reports `0`, and all pass, including `every_modal_scrim_in_the_crate_is_named_here`, `exactly_one_line_opens_the_record_composer`, and the header-control and strip tests.

If `the_strip_reserves_room_for_every_control_it_draws` fails, it means a caller draws the Share control without `header_controls_for` counting it; fix the caller, not the test.

- [ ] **Step 7: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/mod.rs deskwarden/src/vault_window/item_list.rs
git commit -m "A record has a Share control, and sharing opens the dialog from design 6a

The read pane's header gains Share right after the envelope. On
NodeWarden it opens the Share dialog through the re-prompt gate. Where
sharing is not offered it opens the Send composer instead, which is the
owner's 'a Send is offered instead'.

The dialog suggests from the one address book with per-kind counts and
the same forget question, which now lists shared records beside live
Sends and stops them through the daemon on yes. When the address has no
account, the dialog stays open to say so, and offers an administrator an
invite code for them. record-share-scrim is registered with
MODAL_SCRIM_AREAS.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: The SHARING card lists the people; stop, re-invite, fingerprint, share anyway

**Files:**
- Modify: `deskwarden/src/vault_window/share_ui.rs` (`draw_people_block`, `draw_share_toast`, tests)
- Modify: `deskwarden/src/vault_window/detail.rs` (DetailAction variants; the SHARING card call site at line 4314)
- Modify: `deskwarden/src/vault_window/mod.rs` (the new arms in the detail-action match; `detail_action_exposes_secrets`; the toast draw)

**Interfaces:**
- Produces:
  - `DetailAction::{StopSharing(String), ConfirmStopSharing(String), CancelStopSharing, Reinvite(String), ShowFingerprint(String), HideFingerprint, ShareAnyway(String)}`
  - `share_ui::draw_people_block(ui: &mut egui::Ui, record: &str, people: &detail::PeopleSharing<'_>, action: &mut detail::DetailAction)`
  - `share_ui::draw_share_toast(ui: &mut egui::Ui, pane_rect: egui::Rect, pane: &SharingPane)`

- [ ] **Step 1: Write the failing tests**

Append to `share_ui.rs`'s tests:

```rust
    fn block_frame(people: super::super::detail::PeopleSharing<'_>, events: Vec<egui::Event>) -> (super::super::detail::DetailAction, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |ui| {
            let mut ignored = super::super::detail::DetailAction::None;
            draw_people_block(ui, "Acme VPN", &people, &mut ignored);
        });
        let mut action = super::super::detail::DetailAction::None;
        let output = ctx.run_ui(input(events), |ui| draw_people_block(ui, "Acme VPN", &people, &mut action));
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }

    fn lines() -> Vec<PersonLine> {
        vec![
            PersonLine { email: "anna@x.io".into(), state: PersonState::Shared, line: "Shared \u{b7} since 14 Sep".into() },
            PersonLine { email: "carol@x.io".into(), state: PersonState::WaitingAccount, line: "Waiting to create an account \u{b7} 6 days left".into() },
        ]
    }

    /// **Mock 6b at rest**: each person, their pill, their line; Stop
    /// sharing on every row; Re-invite where it helps; the fingerprint link
    /// only where a key is held.
    #[test]
    fn the_people_block_draws_each_person_with_their_controls() {
        let people_lines = lines();
        let people = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(people, Vec::new());
        let count = |needle: &str| texts.iter().filter(|(t, _)| t == needle).count();
        assert_eq!(count("anna@x.io"), 1);
        assert_eq!(count(STOP_LABEL), 2);
        assert_eq!(count(REINVITE_LABEL), 1, "only carol's invite can be renewed");
        assert_eq!(count(SHOW_FINGERPRINT), 1, "only anna's key is held");
        assert_eq!(count("Shared"), 1);
        assert_eq!(count("No account yet"), 1);
    }

    /// **Stop sharing is two steps** (mock 6b): the first reports
    /// `StopSharing`; with the question up, Cancel sits in its slot and the red
    /// button under the row reports `ConfirmStopSharing`.
    #[test]
    fn stopping_is_asked_before_it_happens() {
        use super::super::detail::DetailAction;
        let people_lines = lines();
        let at_rest = super::super::detail::PeopleSharing { people: &people_lines, ..Default::default() };
        let (_, texts) = block_frame(at_rest, Vec::new());
        let stop = texts.iter().find(|(t, _)| t == STOP_LABEL).map(|(_, r)| r.center()).expect("Stop sharing");
        assert_eq!(block_frame(at_rest, click(stop)).0, DetailAction::StopSharing("anna@x.io".into()));

        let asking = super::super::detail::PeopleSharing { people: &people_lines, confirming: Some("anna@x.io"), ..Default::default() };
        let (_, texts) = block_frame(asking, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == &stop_question("Acme VPN", "anna@x.io")), "{texts:?}");
        let remove = texts.iter().find(|(t, _)| t == REMOVE_LABEL).map(|(_, r)| r.center()).expect("the red button");
        assert_eq!(block_frame(asking, click(remove)).0, DetailAction::ConfirmStopSharing("anna@x.io".into()));
        let cancel = texts.iter().find(|(t, _)| t == CANCEL_BUTTON).map(|(_, r)| r.center()).expect("Cancel");
        assert_eq!(block_frame(asking, click(cancel)).0, DetailAction::CancelStopSharing);
    }

    /// The fingerprint, once fetched, is shown under the person with the
    /// sentence that says what to compare it with (mock 6b, fingerprint).
    #[test]
    fn a_shown_fingerprint_says_what_to_compare_it_with() {
        let people_lines = lines();
        let people = super::super::detail::PeopleSharing {
            people: &people_lines,
            fingerprint: Some(("anna@x.io", "turban-deftly-anime-chatroom-unselfish")),
            ..Default::default()
        };
        let (_, texts) = block_frame(people, Vec::new());
        assert!(texts.iter().any(|(t, _)| t == "turban-deftly-anime-chatroom-unselfish"));
        assert!(texts.iter().any(|(t, _)| t == HIDE_FINGERPRINT));
        assert!(texts.iter().any(|(t, _)| t.contains("Account \u{203a} Fingerprint phrase")));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 vault_window::share_ui`
Expected: a compile error. `draw_people_block`, `STOP_LABEL` and the other new names are not found.

- [ ] **Step 3: Implement the block and the toast**

`detail.rs`: add to `DetailAction`:

```rust
    /// Stop sharing with a person: step one arms the question (mock 6b).
    StopSharing(String),
    /// Step two: remove the record from their vault.
    ConfirmStopSharing(String),
    CancelStopSharing,
    /// A fresh invitation for a person (spec 6.1).
    Reinvite(String),
    /// Fetch and show a person's fingerprint phrase.
    ShowFingerprint(String),
    HideFingerprint,
    /// Confirm a person whose key changed, after looking at the fingerprint.
    ShareAnyway(String),
```

At the SHARING card call site (line ~4314), replace

```rust
        if let Some(shared) = shared {
            card(ui, SHARING_CARD_HEADING, |ui| {
                sharing_card(ui, shared, &crate::send::SystemClock, &mut action);
            });
            ui.add_space(CARD_GAP);
        }
```

with

```rust
        // **Design 6b**: People first -- round avatar, share pill, person mark
        // -- and the Active Send under them, unchanged. Either alone draws
        // the card; neither draws nothing.
        if shared.is_some() || !people.people.is_empty() {
            card(ui, SHARING_CARD_HEADING, |ui| {
                if !people.people.is_empty() {
                    super::share_ui::draw_people_block(ui, &item.name, &people, &mut action);
                }
                if let Some(shared) = shared {
                    sharing_card(ui, shared, &crate::send::SystemClock, &mut action);
                }
            });
            ui.add_space(CARD_GAP);
        }
```

`share_ui.rs`: append:

```rust
// ---- the SHARING card's people (mock 6b) ------------------------------------------------

pub(crate) const STOP_LABEL: &str = "Stop sharing";
pub(crate) const REINVITE_LABEL: &str = "Re-invite";
pub(crate) const SHARE_ANYWAY_LABEL: &str = "Share anyway";
pub(crate) const SHOW_FINGERPRINT: &str = "Show fingerprint";
pub(crate) const HIDE_FINGERPRINT: &str = "Hide fingerprint";
pub(crate) const REMOVE_LABEL: &str = "Remove from their vault";
pub(crate) const STOPPING_LABEL: &str = "Stopping\u{2026}";
pub(crate) const STOP_HINT: &str =
    "Anything they already copied stays with them. If that matters, change the password afterwards.";
const PEOPLE_BLOCK_LABEL: &str = "People";
const PEOPLE_NOTE: &str = "A person keeps a view-only copy in their own vault that follows your edits, \
    until you stop sharing. A Send is a link: anyone who has it can open it until it expires or you \
    revoke it.";

pub(crate) fn stop_question(record: &str, email: &str) -> String {
    format!("Stop sharing {record} with {email}? It leaves their vault at once.")
}

fn fingerprint_note(email: &str) -> String {
    format!(
        "This should match the phrase {email} sees under Account \u{203a} Fingerprint phrase. If it \
         does not, stop sharing: the record went to a key that is not theirs."
    )
}

/// **The People block of the SHARING card**: one entry per person -- a round
/// avatar with their initial, the address and share pill, the status line
/// with its fingerprint link, and the controls. Stop sharing is two steps,
/// with Cancel in Stop's own slot (the app's pattern); the question and the
/// red button sit under the row so nothing above moves under the pointer.
pub(crate) fn draw_people_block(
    ui: &mut egui::Ui,
    record: &str,
    people: &super::detail::PeopleSharing<'_>,
    action: &mut super::detail::DetailAction,
) {
    use super::detail::DetailAction;
    egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 6 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
            theme::paint_person_mark(ui.painter(), mark, theme::TEXT_FAINT);
            ui.label(egui::RichText::new(PEOPLE_BLOCK_LABEL).size(12.0).color(theme::TEXT_FAINT));
        });
        for person in people.people {
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                let (avatar, _) = ui.allocate_exact_size(egui::Vec2::splat(28.0), egui::Sense::hover());
                ui.painter().circle(avatar.center(), 14.0, theme::CARD, egui::Stroke::new(1.0, theme::HAIRLINE));
                let initial = person.email.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                ui.painter().text(avatar.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(11.0), theme::TEXT_MUTED);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&person.email).size(14.0).color(theme::INK));
                        let (slot, _) = ui.allocate_exact_size(
                            egui::vec2(theme::share_pill_width(ui.painter(), person.pill()), theme::PILL_HEIGHT),
                            egui::Sense::hover(),
                        );
                        theme::share_pill(ui.painter(), egui::pos2(slot.left(), slot.center().y), person.pill());
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&person.line).size(11.0).color(theme::TEXT_FAINT));
                        if person.fingerprint() {
                            let showing = people.fingerprint.is_some_and(|(email, _)| email == person.email);
                            let label = if showing { HIDE_FINGERPRINT } else { SHOW_FINGERPRINT };
                            let link = ui.add(egui::Label::new(theme::semibold(label, 11.0).color(theme::BLUE_DEEP)).sense(egui::Sense::click()));
                            if link.clicked() {
                                *action = if showing { DetailAction::HideFingerprint } else { DetailAction::ShowFingerprint(person.email.clone()) };
                            }
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if people.stopping == Some(person.email.as_str()) {
                        ui.label(egui::RichText::new(STOPPING_LABEL).size(12.0).color(theme::TEXT_MUTED));
                        return;
                    }
                    if people.confirming == Some(person.email.as_str()) {
                        if theme::row_button(ui, CANCEL_BUTTON).clicked() {
                            *action = DetailAction::CancelStopSharing;
                        }
                        return;
                    }
                    if theme::row_button_danger(ui, STOP_LABEL).clicked() {
                        *action = DetailAction::StopSharing(person.email.clone());
                    }
                    if person.reinvite() && theme::row_button(ui, REINVITE_LABEL).clicked() {
                        *action = DetailAction::Reinvite(person.email.clone());
                    }
                    if person.share_anyway() && theme::row_button(ui, SHARE_ANYWAY_LABEL).clicked() {
                        *action = DetailAction::ShareAnyway(person.email.clone());
                    }
                });
            });
            if people.confirming == Some(person.email.as_str()) {
                ui.indent(("stop", &person.email), |ui| {
                    ui.label(theme::semibold(stop_question(record, &person.email), 14.0).color(theme::ERROR));
                    ui.label(egui::RichText::new(STOP_HINT).size(11.0).color(theme::TEXT_FAINT));
                    ui.add_space(10.0);
                    // Cancel wins an ambiguous frame, as on the Send card.
                    if theme::destructive_button(ui, REMOVE_LABEL).clicked() && *action != DetailAction::CancelStopSharing {
                        *action = DetailAction::ConfirmStopSharing(person.email.clone());
                    }
                });
            }
            if let Some((email, phrase)) = people.fingerprint.filter(|(email, _)| *email == person.email) {
                ui.indent(("fingerprint", email), |ui| {
                    egui::Frame::new()
                        .fill(theme::BLUE_WASH)
                        .stroke(egui::Stroke::new(1.0, theme::BLUE_EDGE))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.label(theme::bold("FINGERPRINT", 11.0).color(theme::BLUE_DEEP));
                            ui.label(egui::RichText::new(phrase).size(13.0).monospace().strong().color(theme::BLUE_DEEP));
                            ui.label(egui::RichText::new(fingerprint_note(email)).size(11.0).color(theme::BLUE_DEEP));
                        });
                });
            }
        }
    });
    egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 13 }).show(ui, |ui| {
        ui.label(egui::RichText::new(PEOPLE_NOTE).size(11.0).color(theme::TEXT_GHOST));
    });
}

/// The share toast, bottom right of the pane (mock 6a): dark, one line, gone
/// after [`TOAST_SECONDS`].
pub(crate) fn draw_share_toast(ui: &mut egui::Ui, pane_rect: egui::Rect, pane: &SharingPane) {
    let Some((text, _)) = &pane.toast else {
        return;
    };
    let galley = ui.painter().layout_no_wrap(text.clone(), egui::FontId::proportional(13.0), theme::CARD);
    let size = galley.size() + egui::vec2(28.0, 20.0);
    let rect = egui::Rect::from_min_size(pane_rect.right_bottom() - size - egui::vec2(20.0, 20.0), size);
    ui.painter().rect_filled(rect, egui::CornerRadius::same(8), theme::INK);
    ui.painter().galley(rect.min + egui::vec2(14.0, 10.0), galley, theme::CARD);
}
```

- [ ] **Step 4: The window's arms**

In `detail_action_exposes_secrets`, add these to the non-exposing side with a comment, beside the Revoke arms:

```rust
        // **Design 6b's people controls take access AWAY or re-send an
        // invitation**: none reads a field of the item. The fingerprint is a
        // public-key digest, not a secret of this vault.
        | DetailAction::StopSharing(_)
        | DetailAction::ConfirmStopSharing(_)
        | DetailAction::CancelStopSharing
        | DetailAction::Reinvite(_)
        | DetailAction::ShowFingerprint(_)
        | DetailAction::HideFingerprint
        | DetailAction::ShareAnyway(_)
```

In the detail-action `match` (line ~4990), add:

```rust
                                DetailAction::StopSharing(email) => {
                                    sharing.stop_confirming = Some((item.id.clone(), email));
                                }
                                DetailAction::CancelStopSharing => sharing.stop_confirming = None,
                                DetailAction::ConfirmStopSharing(email) => {
                                    let call = share_ui::SharingCall::Stop { item_id: item.id.clone(), email };
                                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, call);
                                }
                                DetailAction::Reinvite(email) => {
                                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Reinvite(email));
                                }
                                DetailAction::ShowFingerprint(email) => {
                                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Fingerprint(email));
                                }
                                DetailAction::HideFingerprint => sharing.fingerprint = None,
                                DetailAction::ShareAnyway(email) => {
                                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Confirm(email));
                                }
```

After the read pane is drawn (right after the `draw_read_arm` call's result is handled), draw the toast over the detail pane: `share_ui::draw_share_toast(ui, ui.max_rect(), &sharing);`.

`sharing.notice` (a failed request's sentence) is shown with the pane's existing refusal note: where the frame handles the action, add `if let Some(notice) = sharing.notice.take() { detail::note_refused(ui.ctx(), notice); }`. `note_refused` takes whatever the re-prompt's refusal passes it. If that is `&str`, pass `&notice`.

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/detail.rs src/vault_window/mod.rs
cargo test --lib -j 2 vault_window::
```

Expected: every file reports `0`, and all pass. The existing SHARING-card tests still see the Active Send row and its Revoke two-step unchanged.

- [ ] **Step 6: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/mod.rs
git commit -m "The SHARING card lists the people a record is shared with

Design 6b: People above the Active Send. Each person has a round avatar,
their address and share pill, and their status line with a fingerprint
link where a key is held. Stop sharing is the app's two-step: Cancel in
its slot, the question and 'Remove from their vault' under the row.
Re-invite appears where an invitation helps, and Share anyway where a
key changed. The fingerprint's five words are shown with the sentence
that says what to compare them with. A share or a stop ends in a toast.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 18: Sent lists person shares, and a share has its own detail (design 6c)

**Files:**
- Modify: `deskwarden/src/vault_window/share_ui.rs`
  - `ShareRow`, `share_rows`, `PeopleRows`, `PeopleAsk`;
  - `draw_share_row`, `draw_share_detail`;
  - tests.
- Modify: `deskwarden/src/vault_window/send_ui.rs`
  - `draw_send_pane`, `draw_send_list` and `draw_send_detail` gain `people: &mut super::share_ui::PeopleRows<'_>`;
  - the Mine view draws share rows first;
  - share keys open `draw_share_detail`;
  - no new `pub` item.
- Modify: `deskwarden/src/vault_window/mod.rs` (build `PeopleRows` before the `apply_send_action` call; handle `asked` after it)
- Modify: every other `draw_send_pane(` caller, all of them tests in `send_ui.rs` and `mod.rs`: pass `&mut share_ui::PeopleRows::none()`

**Interfaces:**
- Produces (`share_ui`, `pub(crate)`):
  - `SHARE_KEY_PREFIX: &str = "share:"`
  - `ShareRow { key, item_id, name, email, state: PersonState, subtitle: String, since: Option<String> }`
  - `share_rows(view: Option<&SharingView>, items: &[VaultItem], history: &SentHistory, now_ms: i64, zone: &dyn LocalOffset) -> Vec<ShareRow>`
  - `share_key(item_id: &str, email: &str) -> String`
  - `parse_share_key(key: &str) -> Option<(String, String)>`
  - `PeopleRows<'a> { shares: &'a [ShareRow], confirming: Option<String>, stopping: Option<String>, asked: Option<PeopleAsk> }`, with `PeopleRows::none() -> PeopleRows<'static>`
  - `PeopleAsk { Stop(String), ConfirmStop(String), CancelStop }` (keys are share keys)
  - `draw_share_row(ui, row: &ShareRow, width: f32, height: f32, selected: &mut Option<String>)`
  - `draw_share_detail(ui, people: &mut PeopleRows<'_>, key: &str) -> Option<crate::vault_window::send_ui::SendUiAction>`

- [ ] **Step 1: Write the failing tests**

Append to `share_ui.rs`'s tests:

```rust
    /// A plain harness: `draw` over a `width` x 800 window, and every string
    /// it painted.
    fn ui_frame(width: f32, events: Vec<egui::Event>, mut draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |ui| draw(ui));
        let output = ctx.run_ui(input(events), |ui| draw(ui));
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        texts
    }

    /// **One row per (record, person)** (mock 6c), newest share first, with
    /// the person mark's subtitle "to anna@example.com · Shared".
    #[test]
    fn sent_lists_one_row_for_each_record_and_person() {
        let v = view(
            vec![("anna@x.io", PersonState::Shared, None), ("bob@x.io", PersonState::WaitingAccept, None)],
            vec![("r1", vec!["anna@x.io", "bob@x.io"]), ("r2", vec!["anna@x.io"])],
        );
        let items: Vec<VaultItem> = ["r1", "r2"].iter().map(|id| serde_json::from_value(serde_json::json!({ "id": id, "name": format!("Record {id}") })).expect("item")).collect();
        let mut history = SentHistory::default();
        history.record(crate::sent_history::SentRecord { kind: SentKind::Share, item_id: "r2".into(), recipient: "anna@x.io".into(), sent_at_unix_millis: NOW, ..Default::default() });
        let rows = share_rows(Some(&v), &items, &history, NOW, &crate::local_time::FixedOffset(0));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].item_id, "r2", "the newest share first");
        assert_eq!(rows[0].subtitle, "to anna@x.io \u{b7} Shared");
        assert!(rows.iter().any(|r| r.subtitle == "to bob@x.io \u{b7} Waiting to accept"));
        assert_eq!(parse_share_key(&rows[0].key), Some(("r2".to_string(), "anna@x.io".to_string())));
    }

    /// **The share's own detail** (mock 6c): PERSON and WHAT THEY SEE, Open
    /// record, and a red Stop sharing that asks first.
    #[test]
    fn a_shares_detail_shows_the_person_and_stops_in_two_steps() {
        use crate::vault_window::send_ui::SendUiAction;
        let row = ShareRow {
            key: share_key("r1", "anna@x.io"),
            item_id: "r1".into(),
            name: "Acme VPN".into(),
            email: "anna@x.io".into(),
            state: PersonState::Shared,
            subtitle: "to anna@x.io \u{b7} Shared".into(),
            since: Some("14 Sep".into()),
        };
        let rows = [row.clone()];
        let mut people = PeopleRows { shares: &rows, ..PeopleRows::none() };
        let texts = ui_frame(700.0, Vec::new(), |ui| {
            let _ = draw_share_detail(ui, &mut people, &row.key);
        });
        let has = |needle: &str| texts.iter().any(|(t, _)| t == needle);
        assert!(has("PERSON") && has("WHAT THEY SEE") && has(OPEN_RECORD_LABEL) && has(STOP_LABEL), "{texts:?}");
        assert!(has("View-only"));
        let open = texts.iter().find(|(t, _)| t == OPEN_RECORD_LABEL).map(|(_, r)| r.center()).expect("Open record");
        let mut opened = None;
        let _ = ui_frame(700.0, click(open), |ui| {
            if let Some(action) = draw_share_detail(ui, &mut people, &row.key) {
                opened = Some(action);
            }
        });
        assert_eq!(opened, Some(SendUiAction::OpenRecord("r1".into())));
        let stop = texts.iter().find(|(t, _)| t == STOP_LABEL).map(|(_, r)| r.center()).expect("Stop sharing");
        let _ = ui_frame(700.0, click(stop), |ui| {
            let _ = draw_share_detail(ui, &mut people, &row.key);
        });
        assert!(matches!(people.asked, Some(PeopleAsk::Stop(ref key)) if *key == row.key));
    }
```

`SendUiAction` must derive `PartialEq` for the `assert_eq!`. If it does not, compare with `matches!(opened, Some(SendUiAction::OpenRecord(ref id)) if id == "r1")`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 vault_window::share_ui`
Expected: a compile error. `share_rows`, `PeopleRows` and `draw_share_detail` are not found.

- [ ] **Step 3: Implement in `share_ui.rs`**

```rust
// ---- Sent: one row per record and person (mock 6c) -------------------------------------

pub(crate) const SHARE_KEY_PREFIX: &str = "share:";
pub(crate) const OPEN_RECORD_LABEL: &str = "Open record";
const SHARED_WITH_A_PERSON: &str = "Shared with a person";

/// A share in the Sent list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShareRow {
    /// `share:{item id}:{email}` -- the Sent pane's selection key.
    pub(crate) key: String,
    pub(crate) item_id: String,
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) state: PersonState,
    pub(crate) subtitle: String,
    /// "14 Sep", from the local record of the share.
    pub(crate) since: Option<String>,
}

pub(crate) fn share_key(item_id: &str, email: &str) -> String {
    format!("{SHARE_KEY_PREFIX}{item_id}:{email}")
}

pub(crate) fn parse_share_key(key: &str) -> Option<(String, String)> {
    let rest = key.strip_prefix(SHARE_KEY_PREFIX)?;
    let (item_id, email) = rest.split_once(':')?;
    Some((item_id.to_string(), email.to_string()))
}

/// Every share, newest first by the local record of it; a share this PC has
/// no record of (made on another PC) sorts last, by record name.
pub(crate) fn share_rows(
    view: Option<&SharingView>,
    items: &[VaultItem],
    history: &SentHistory,
    now_ms: i64,
    zone: &dyn LocalOffset,
) -> Vec<ShareRow> {
    let Some(view) = view else {
        return Vec::new();
    };
    let mut rows: Vec<(Option<i64>, ShareRow)> = Vec::new();
    for record in &view.records {
        let Some(item) = items.iter().find(|i| i.id == record.item_id) else {
            continue;
        };
        for email in &record.people {
            let Some(person) = view.person(email) else {
                continue;
            };
            let at = since(history, &item.id, email);
            rows.push((
                at,
                ShareRow {
                    key: share_key(&item.id, email),
                    item_id: item.id.clone(),
                    name: item.name.clone(),
                    email: email.clone(),
                    state: person.state,
                    subtitle: format!("to {email} \u{b7} {}", short_state(person.state, person.expires_at.as_deref(), now_ms)),
                    since: at.map(|at| day(at, zone)),
                },
            ));
        }
    }
    rows.sort_by(|(a, ra), (b, rb)| b.cmp(a).then_with(|| ra.name.cmp(&rb.name)));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// What the Sent and Shared-with-me lists add for sharing, per frame, and
/// what the user asked of it.
pub(crate) struct PeopleRows<'a> {
    pub(crate) shares: &'a [ShareRow],
    /// The share key whose stop is armed.
    pub(crate) confirming: Option<String>,
    /// The share key whose stop is running.
    pub(crate) stopping: Option<String>,
    /// Out: what was pressed this frame.
    pub(crate) asked: Option<PeopleAsk>,
}

impl PeopleRows<'static> {
    /// Nothing about sharing: what every caller that is not about it passes.
    pub(crate) fn none() -> Self {
        Self { shares: &[], confirming: None, stopping: None, asked: None }
    }
}

/// A press in the lists' sharing half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeopleAsk {
    Stop(String),
    ConfirmStop(String),
    CancelStop,
}

/// Paints the record's two-letter monogram in a square tile -- a share is a
/// record; the person is in the subtitle.
fn monogram_tile(painter: &egui::Painter, rect: egui::Rect, name: &str, picked: bool) {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    painter.rect_filled(rect, egui::CornerRadius::same(10), theme::CARD);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(1.0, if picked { theme::BLUE_EDGE } else { theme::HAIRLINE }),
        egui::StrokeKind::Inside,
    );
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, initials, egui::FontId::proportional(16.0), if picked { theme::BLUE } else { theme::TEXT_MUTED });
}

/// **One share row** (mock 6c): the record's tile, its name, a person mark
/// and "to anna@example.com · Shared", and the share pill at the right.
pub(crate) fn draw_share_row(ui: &mut egui::Ui, row: &ShareRow, width: f32, height: f32, selected: &mut Option<String>) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.clicked() {
        *selected = Some(row.key.clone());
    }
    let picked = selected.as_deref() == Some(row.key.as_str());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(10), theme::CARD);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(1.0, if picked { theme::BLUE } else { theme::HAIRLINE }),
        egui::StrokeKind::Inside,
    );
    let tile = egui::Rect::from_center_size(egui::pos2(rect.left() + 12.0 + 20.0, rect.center().y), egui::Vec2::splat(40.0));
    monogram_tile(painter, tile, &row.name, picked);
    let kind = person_line(row).pill();
    let pill_width = theme::share_pill_width(painter, kind);
    theme::share_pill(painter, egui::pos2(rect.right() - 12.0 - pill_width, rect.center().y), kind);
    let text_left = tile.right() + 11.0;
    let title_font = egui::FontId::new(13.0, egui::FontFamily::Name(if picked { theme::BOLD } else { theme::SEMIBOLD }.into()));
    painter.text(egui::pos2(text_left, rect.center().y - 9.0), egui::Align2::LEFT_CENTER, &row.name, title_font, if picked { theme::BLUE_DEEP } else { theme::INK });
    let mark = egui::Rect::from_min_size(egui::pos2(text_left, rect.center().y + 4.0), egui::Vec2::splat(10.0));
    theme::paint_person_mark(painter, mark, theme::TEXT_FAINT);
    painter.text(
        egui::pos2(mark.right() + 4.0, mark.center().y),
        egui::Align2::LEFT_CENTER,
        &row.subtitle,
        egui::FontId::proportional(11.0),
        theme::TEXT_FAINT,
    );
}

fn person_line(row: &ShareRow) -> PersonLine {
    PersonLine { email: row.email.clone(), state: row.state, line: row.subtitle.clone() }
}

/// **A share's detail** (mock 6c): the header with Open record and the red
/// Stop sharing (two steps, Cancel in its slot), then PERSON and WHAT THEY SEE.
/// Returns Open record as the Sent screen's own `OpenRecord`.
pub(crate) fn draw_share_detail(
    ui: &mut egui::Ui,
    people: &mut PeopleRows<'_>,
    key: &str,
) -> Option<crate::vault_window::send_ui::SendUiAction> {
    let row = people.shares.iter().find(|r| r.key == key)?.clone();
    let mut action = None;
    let confirming = people.confirming.as_deref() == Some(key);
    let stopping = people.stopping.as_deref() == Some(key);
    egui::Frame::new().fill(theme::CARD).inner_margin(egui::Margin::symmetric(24, 20)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (tile, _) = ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
            monogram_tile(ui.painter(), tile, &row.name, true);
            ui.vertical(|ui| {
                ui.label(theme::bold(&row.name, 22.0).color(theme::INK));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(SHARED_WITH_A_PERSON).size(12.0).color(theme::TEXT_FAINT));
                    let kind = person_line(&row).pill();
                    let (slot, _) = ui.allocate_exact_size(egui::vec2(theme::share_pill_width(ui.painter(), kind), theme::PILL_HEIGHT), egui::Sense::hover());
                    theme::share_pill(ui.painter(), egui::pos2(slot.left(), slot.center().y), kind);
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if stopping {
                    ui.label(egui::RichText::new(STOPPING_LABEL).size(12.0).color(theme::TEXT_MUTED));
                } else if confirming {
                    if theme::secondary_button(ui, CANCEL_BUTTON).clicked() {
                        people.asked = Some(PeopleAsk::CancelStop);
                    }
                } else {
                    if theme::destructive_button(ui, STOP_LABEL).clicked() {
                        people.asked = Some(PeopleAsk::Stop(key.to_string()));
                    }
                    if theme::secondary_button(ui, OPEN_RECORD_LABEL).clicked() {
                        action = Some(crate::vault_window::send_ui::SendUiAction::OpenRecord(row.item_id.clone()));
                    }
                }
            });
        });
    });
    egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 18)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if confirming {
            ui.label(theme::semibold(stop_question(&row.name, &row.email), 14.0).color(theme::ERROR));
            ui.label(egui::RichText::new(STOP_HINT).size(11.0).color(theme::TEXT_FAINT));
            ui.add_space(10.0);
            if theme::destructive_button(ui, REMOVE_LABEL).clicked() && people.asked != Some(PeopleAsk::CancelStop) {
                people.asked = Some(PeopleAsk::ConfirmStop(key.to_string()));
            }
            ui.add_space(14.0);
        }
        fact_card(ui, "PERSON", &[
            ("Shared with", row.email.clone(), None),
            ("Since", row.since.clone().unwrap_or_else(|| "On another PC".to_string()), None),
            ("Access", "View-only".to_string(), Some("They can see, copy and autofill it. They cannot edit, delete or share it on.")),
            ("Their copy", "Follows your edits".to_string(), None),
        ]);
        ui.add_space(14.0);
        fact_card(ui, "WHAT THEY SEE", &[("", "The whole record, now and after every edit.".to_string(), None)]);
    });
    action
}

/// A card of label/value rows, in the read pane's own proportions.
fn fact_card(ui: &mut egui::Ui, title: &str, rows: &[(&str, String, Option<&str>)]) {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::HAIRLINE))
        .corner_radius(egui::CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 11)).show(ui, |ui| {
                ui.label(theme::bold(title, 12.0).color(theme::TEXT_MUTED));
            });
            for (label, value, hint) in rows {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 13)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized(egui::vec2(130.0, 16.0), egui::Label::new(egui::RichText::new(*label).size(12.0).color(theme::TEXT_FAINT)));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(value).size(14.0).color(theme::INK));
                            if let Some(hint) = hint {
                                ui.label(egui::RichText::new(*hint).size(11.0).color(theme::TEXT_FAINT));
                            }
                        });
                    });
                });
            }
        });
}
```

- [ ] **Step 4: Hand `send_ui` the rows**

In `send_ui.rs`:
- `draw_send_pane` gains the last parameter `people: &mut super::share_ui::PeopleRows<'_>` and forwards it to `draw_send_list(…, people)` and `draw_send_detail(…, people)`.
- Both of those gain the same parameter.

In `draw_send_list`, change the `counted` arm for Mine to add the shares when the scope is All:

```rust
        (SendView::Mine(scope), SendPaneState::Rows(rows)) => {
            let shares = if *scope == crate::vault_window::sidebar::SendScope::All { people.shares.len() } else { 0 };
            Some(rows_in_scope(rows, *scope).len() + shares)
        }
        (SendView::Mine(scope), SendPaneState::Empty) => {
            Some(if *scope == crate::vault_window::sidebar::SendScope::All { people.shares.len() } else { 0 })
        }
```

Replace the `SendPaneState::Rows(rows)` arm of `SendView::Mine(scope) => match state` with:

```rust
            SendPaneState::Rows(rows) => {
                let in_scope = rows_in_scope(rows, *scope);
                // **Shares first** (plan decision 12): a person share has no
                // server timestamp to interleave by, and "All" is the only
                // scope a share belongs to -- the scopes are Send states.
                let shares: &[super::share_ui::ShareRow] =
                    if *scope == crate::vault_window::sidebar::SendScope::All { people.shares } else { &[] };
                if in_scope.is_empty() && shares.is_empty() {
                    let head = scope_empty_headline(*scope);
                    headline(ui, &head, SCOPE_EMPTY_DETAIL, theme::INK);
                } else {
                    rows_area(ui, in_scope.len() + shares.len(), &mut |ui| {
                        for share in shares {
                            super::share_ui::draw_share_row(ui, share, row_width, ROW_HEIGHT, selected);
                        }
                        for row in &in_scope {
                            draw_row(ui, row, row_width, delete, selected, records);
                        }
                    });
                }
            }
```

Change the `SendPaneState::Empty` arm to draw the shares when there are any:

```rust
            SendPaneState::Empty if *scope == crate::vault_window::sidebar::SendScope::All && !people.shares.is_empty() => {
                let shares = people.shares;
                rows_area(ui, shares.len(), &mut |ui| {
                    for share in shares {
                        super::share_ui::draw_share_row(ui, share, row_width, ROW_HEIGHT, selected);
                    }
                });
            }
            SendPaneState::Empty => headline(ui, EMPTY_HEADLINE, EMPTY_DETAIL, theme::INK),
```

`SendScope` must derive `PartialEq` (it derives `Clone, Copy, PartialEq, Eq, Default`).

In `draw_send_detail`, at the top of the `SendView::Mine(scope)` arm:

```rust
            if let Some(key) = selected.as_deref().filter(|key| key.starts_with(super::share_ui::SHARE_KEY_PREFIX)) {
                let key = key.to_string();
                return super::share_ui::draw_share_detail(ui, people, &key);
            }
```

Update every test call of `draw_send_pane(…)` in `send_ui.rs` and `mod.rs` to pass `&mut super::share_ui::PeopleRows::none()` (or the crate path the call site can see) as the last argument.

- [ ] **Step 5: The window**

In `mod.rs`, just before `let open_record = apply_send_action(`, build:

```rust
        // **Design 6c**: the Sent list's person shares, one row per record and
        // person, and the Stop sharing two-step on a share's own detail.
        let share_rows = share_ui::share_rows(
            sharing.view.as_ref(),
            &items,
            &sent_history,
            crate::send::SendClock::now_unix_millis(&crate::send::SystemClock),
            &crate::local_time::SystemZone,
        );
        let mut people_rows = share_ui::PeopleRows {
            shares: &share_rows,
            confirming: sharing.stop_confirming.as_ref().map(|(id, email)| share_ui::share_key(id, email)),
            stopping: sharing.stopping.as_ref().map(|(id, email)| share_ui::share_key(id, email)),
            asked: None,
        };
```

Inside the closure, add `&mut people_rows,` as the last argument of `send_ui::draw_send_pane(…)`. This sits inside the argument to `apply_send_action`; the pinned head `apply_send_action( egui::CentralPanel::default()` and the pinned tail `}) .inner .into_action(), &mut send_delete, …` are unchanged.

Directly after the `apply_send_action(…);` statement:

```rust
        match people_rows.asked.take() {
            Some(share_ui::PeopleAsk::Stop(key)) => sharing.stop_confirming = share_ui::parse_share_key(&key),
            Some(share_ui::PeopleAsk::CancelStop) => sharing.stop_confirming = None,
            Some(share_ui::PeopleAsk::ConfirmStop(key)) => {
                if let Some((item_id, email)) = share_ui::parse_share_key(&key) {
                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Stop { item_id, email });
                }
            }
            None => {}
        }
```

`people_rows` borrows only `share_rows`, a local of this frame, so `sharing` is free to change here.

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/send_ui.rs src/vault_window/mod.rs
cargo test --lib -j 2 vault_window::
```

Expected: every file reports `0`, and all pass. That includes `send_delete_wiring::the_frame_applies_the_sends_action_unconditionally` and `the_applier_takes_the_panel_with_no_binding_between`: the call's head and tail are unchanged, and there is still exactly one `apply_send_action(` call.

- [ ] **Step 7: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/send_ui.rs deskwarden/src/vault_window/mod.rs
git commit -m "Sent lists person shares, one row per record and person

Design 6c: a share row wears the record's tile, a person mark with 'to
anna@example.com · Shared', and a share pill. Shares list first,
newest first by the local record of them, since a share has no server
date to interleave with Sends. A share's detail has PERSON and WHAT
THEY SEE, Open record, and the red Stop sharing two-step. The single
apply_send_action call and its pinned head and tail are untouched; the
list takes the rows through one new parameter.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: Shared with me lists received records, and a received record is view-only

**Files:**
- Modify: `deskwarden/src/vault_window/share_ui.rs` (`ReceivedShareRow`, `received_rows`, `draw_received_group`; `PeopleRows.received`; tests)
- Modify: `deskwarden/src/vault_window/send_ui.rs` (the Received view draws the group first)
- Modify: `deskwarden/src/vault_window/detail.rs`
  - view-only header subtitle;
  - hidden controls;
  - the view-only kebab;
  - `view_only(action)`;
  - tests.
- Modify: `deskwarden/src/vault_window/mod.rs` (fill `PeopleRows.received`; gate detail actions and row commands with `may_edit`)

**Interfaces:**
- Produces:
  - `share_ui::ReceivedShareRow { key: String, item_id: String, name: String, from: String }`
  - `share_ui::RECEIVED_KEY_PREFIX: &str = "recv:"`
  - `share_ui::received_share_rows(view: Option<&SharingView>, items: &[VaultItem]) -> Vec<ReceivedShareRow>`
  - `share_ui::draw_received_group(ui, people: &mut PeopleRows<'_>, width: f32, height: f32, selected: &mut Option<String>) -> Option<SendUiAction>`
  - `PeopleRows.received: &'a [ReceivedShareRow]`
  - `detail::view_only(action: DetailAction) -> DetailAction`
  - `detail::VIEW_ONLY_FOOTER`

- [ ] **Step 1: Write the failing tests**

Append to `share_ui.rs`'s tests:

```rust
    /// **Mock 6d**: FROM PEOPLE above IMPORTED FROM SENDS, each received row
    /// "from denis@example.com · view-only", and a click opens the record.
    #[test]
    fn shared_with_me_lists_received_records_from_people_and_opens_them() {
        use crate::vault_window::send_ui::SendUiAction;
        let received = [ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into() }];
        let mut people = PeopleRows { received: &received, ..PeopleRows::none() };
        let mut selected = None;
        let texts = ui_frame(400.0, Vec::new(), |ui| {
            let _ = draw_received_group(ui, &mut people, 376.0, 62.0, &mut selected);
        });
        assert!(texts.iter().any(|(t, _)| t == FROM_PEOPLE_EYEBROW));
        assert!(texts.iter().any(|(t, _)| t == "from denis@x.io \u{b7} view-only"), "{texts:?}");
        let row = texts.iter().find(|(t, _)| t == "Acme VPN").map(|(_, r)| r.center()).expect("the row");
        let mut opened = None;
        let _ = ui_frame(400.0, click(row), |ui| {
            if let Some(action) = draw_received_group(ui, &mut people, 376.0, 62.0, &mut selected) {
                opened = Some(action);
            }
        });
        assert!(matches!(opened, Some(SendUiAction::OpenRecord(ref id)) if id == "r9"));
    }
```

Append to `detail.rs`'s tests. Extend `Pane` with `view_only_from: Option<String>`, initialised to `None`, and a builder `fn view_only(mut self, from: &str) -> Self`, then pass it in `PeopleSharing`:

```rust
    /// **A received record is view-only** (mock 6d): the subtitle says who
    /// shared it; no star, no envelope, no clock, no Share; the kebab says
    /// why and offers no Edit, Clone or Delete.
    #[test]
    fn a_received_record_reads_view_only_and_offers_nothing_that_writes() {
        let item = login_item();
        let mut pane = Pane::new().view_only("denis@x.io");
        let frame = pane.idle(&item, &TotpState::NoSecret);
        assert!(frame.texts.iter().any(|(t, _)| t.contains("Shared by denis@x.io \u{b7} view-only")), "{:?}", frame.texts);
        assert!(frame.stars.is_empty(), "a received record draws no star");
        assert!(frame.envelopes.is_empty(), "a received record draws no envelope");
        assert!(frame.clocks.is_empty(), "a received record draws no clock");
        assert_eq!(view_only(DetailAction::Edit), DetailAction::None);
        assert_eq!(view_only(DetailAction::Delete), DetailAction::None);
        assert_eq!(view_only(DetailAction::ToggleFavorite(true)), DetailAction::None);
        assert_eq!(view_only(DetailAction::CopyPassword), DetailAction::CopyPassword, "copy still works");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 vault_window::share_ui && cargo test --lib -j 2 vault_window::detail::tests::a_received`
Expected: a compile error. `ReceivedShareRow`, `draw_received_group` and `view_only` are not found.

- [ ] **Step 3: Implement**

`share_ui.rs`:
- add `pub(crate) received: &'a [ReceivedShareRow],` to `PeopleRows`;
- add `received: &[]` to `PeopleRows::none()`;
- append:

```rust
// ---- Shared with me: records from people (mock 6d) -----------------------------------

pub(crate) const RECEIVED_KEY_PREFIX: &str = "recv:";
pub(crate) const FROM_PEOPLE_EYEBROW: &str = "FROM PEOPLE";
pub(crate) const FROM_SENDS_EYEBROW: &str = "IMPORTED FROM SENDS";

/// A record someone shares with this account, in Shared with me.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedShareRow {
    pub(crate) key: String,
    pub(crate) item_id: String,
    pub(crate) name: String,
    pub(crate) from: String,
}

/// The received records the window holds, in list order.
pub(crate) fn received_share_rows(view: Option<&SharingView>, items: &[VaultItem]) -> Vec<ReceivedShareRow> {
    let Some(view) = view else {
        return Vec::new();
    };
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

fn eyebrow(ui: &mut egui::Ui, text: &str) {
    ui.label(theme::bold(text, 10.0).color(theme::TEXT_GHOST));
}

/// **Shared with me's people half** (mock 6d): FROM PEOPLE and its rows, then
/// the IMPORTED FROM SENDS eyebrow over the rows `send_ui` draws after it. A
/// row opens the record in the vault, where its pane is view-only. Draws
/// nothing (not even the eyebrows) when nobody shares anything.
pub(crate) fn draw_received_group(
    ui: &mut egui::Ui,
    people: &mut PeopleRows<'_>,
    width: f32,
    height: f32,
    selected: &mut Option<String>,
) -> Option<crate::vault_window::send_ui::SendUiAction> {
    if people.received.is_empty() {
        return None;
    }
    let mut action = None;
    eyebrow(ui, FROM_PEOPLE_EYEBROW);
    for row in people.received {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let picked = selected.as_deref() == Some(row.key.as_str());
        if response.clicked() {
            *selected = Some(row.key.clone());
            action = Some(crate::vault_window::send_ui::SendUiAction::OpenRecord(row.item_id.clone()));
        }
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::same(10), theme::CARD);
        painter.rect_stroke(rect, egui::CornerRadius::same(10), egui::Stroke::new(1.0, if picked { theme::BLUE } else { theme::HAIRLINE }), egui::StrokeKind::Inside);
        let tile = egui::Rect::from_center_size(egui::pos2(rect.left() + 32.0, rect.center().y), egui::Vec2::splat(40.0));
        monogram_tile(painter, tile, &row.name, picked);
        let left = tile.right() + 11.0;
        painter.text(egui::pos2(left, rect.center().y - 9.0), egui::Align2::LEFT_CENTER, &row.name,
            egui::FontId::new(13.0, egui::FontFamily::Name(theme::SEMIBOLD.into())), if picked { theme::BLUE_DEEP } else { theme::INK });
        let mark = egui::Rect::from_min_size(egui::pos2(left, rect.center().y + 4.0), egui::Vec2::splat(10.0));
        theme::paint_person_mark(painter, mark, theme::TEXT_FAINT);
        painter.text(egui::pos2(mark.right() + 4.0, mark.center().y), egui::Align2::LEFT_CENTER,
            format!("from {} \u{b7} view-only", row.from), egui::FontId::proportional(11.0), theme::TEXT_FAINT);
    }
    eyebrow(ui, FROM_SENDS_EYEBROW);
    action
}
```

`send_ui.rs`:
- In `draw_send_list`'s `counted` arm, change `(SendView::Received(rows), _) => Some(rows.len()),` to `(SendView::Received(rows), _) => Some(rows.len() + people.received.len()),`.
- Change the `SendView::Received(rows)` arm of `match view` to:

```rust
        SendView::Received(rows) => {
            if rows.is_empty() && people.received.is_empty() {
                headline(ui, RECEIVED_EMPTY_HEADLINE, RECEIVED_EMPTY_DETAIL, theme::INK);
            } else {
                let received = people.received.len();
                rows_area(ui, rows.len() + received, &mut |ui| {
                    if let Some(open) = super::share_ui::draw_received_group(ui, people, row_width, ROW_HEIGHT, selected) {
                        action = Some(open);
                    }
                    for row in rows.iter() {
                        draw_received_row(ui, row, row_width, selected);
                    }
                });
            }
        }
```

In `mod.rs`, add `received: &received_share_rows,` to the `PeopleRows` literal, having built `let received_share_rows = share_ui::received_share_rows(sharing.view.as_ref(), &items);` next to `share_rows`.

`detail.rs`:

```rust
/// The kebab's one line on a received record (mock 6d, ⋮ menu open).
pub fn view_only_footer(from: &str) -> String {
    format!("Shared by {from}, view-only. Only they can edit or delete it.")
}

/// **What a view-only record allows** (spec 6.9): everything that reads,
/// nothing that writes. The server refuses a write anyway (spec 10.3); this
/// is so the user is not offered one. Moving it to a folder and starring it
/// are writes on NodeWarden too (the sharing plan's decision 5).
#[must_use]
pub fn view_only(action: DetailAction) -> DetailAction {
    match action {
        DetailAction::Edit
        | DetailAction::Clone
        | DetailAction::Delete
        | DetailAction::MoveToFolder(_)
        | DetailAction::Unfile
        | DetailAction::ToggleFavorite(_)
        | DetailAction::EditSequence
        | DetailAction::RemoveAppMatch
        | DetailAction::AddTotp
        | DetailAction::SendRecord
        | DetailAction::ShareRecord
        | DetailAction::RevokeSend
        | DetailAction::ConfirmRevokeSend => DetailAction::None,
        other => other,
    }
}
```

In `draw_detail_read`, where the subtitle is drawn (line ~3468), replace `let (lead, name) = header_subtitle_parts(kind, folder);` with:

```rust
                        // A received record says who shared it, and that it is
                        // view-only (mock 6d) -- in place of the folder, which a
                        // record in someone else's organisation does not have.
                        let (lead, name) = match people.view_only_from {
                            Some(from) => (format!("{} \u{b7} Shared by {from} \u{b7} view-only", kind.label()), None),
                            None => header_subtitle_parts(kind, folder),
                        };
```

In `draw_controls`:
- wrap the clock block, the Share block, the envelope block and the star block each in `if people.view_only_from.is_none() { … }`;
- replace the kebab menu's body with a branch:

```rust
                menu.show(|ui| {
                    if let Some(from) = people.view_only_from {
                        // Mock 6d's ⋮: nothing that writes -- only why.
                        ui.set_max_width(250.0);
                        ui.label(egui::RichText::new(view_only_footer(from)).size(11.0).color(theme::TEXT_GHOST));
                        return;
                    }
                    // … the existing entries, unchanged …
                });
```

`mod.rs`: directly after `let action = permit_detail_action(…);`, add:

```rust
                            // **A received record is view-only** (spec 6.9): a
                            // write it cannot make is not offered either.
                            let action = if crate::vault_bridge::may_edit(item) { action } else { detail::view_only(action) };
```

In the row-command path (line ~3971), after `let permitted = permit_row_command(…);`, add:

```rust
                let permitted = permitted.filter(|command| crate::vault_bridge::may_edit(&item) || !command.writes_the_record());
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/vault_window/share_ui.rs src/vault_window/send_ui.rs src/vault_window/detail.rs src/vault_window/mod.rs
cargo test --lib -j 2 vault_window::
```

Expected: every file reports `0`, and all pass. The existing kebab and header tests all run with `PeopleSharing::default()`, so they are unchanged.

- [ ] **Step 5: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/send_ui.rs deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/mod.rs
git commit -m "Shared with me lists records from people, and a received record is view-only

Design 6d: Shared with me puts FROM PEOPLE above IMPORTED FROM SENDS,
each received record 'from denis@example.com · view-only'. A row opens
the record in the vault, where the pane says 'Shared by … · view-only'
and draws no star, envelope, clock or Share. Its menu says why instead
of offering Edit, Clone or Delete.

vault_bridge::may_edit gates the detail actions and the row commands,
so nothing that writes is offered for a record the server marks edit:
false. Folder and star are hidden too: NodeWarden refuses both on a
read-only collection.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 20: The recipient accepts, declines, waits, or stops receiving (NodeWarden `invitations/mine`)

**Files:**
- Modify: `deskwarden/src/rest/sharing.rs` (`accept`, `decline`, `leave`, tests)
- Modify: `deskwarden/src/vault_api.rs` (routes `POST /sharing/incoming/{id}/accept`, `/decline`, `/sharing/from/{orgId}/leave`)
- Modify: `deskwarden/src/vault_window/share_ui.rs`
  - `IncomingRow`, `incoming_rows`, `draw_incoming_rows`, `draw_invitation_detail`;
  - `WAITING_LINE`;
  - `draw_leave_confirm`;
  - `PeopleRows.incoming` and `PeopleRows.waiting`;
  - `PeopleAsk::{Accept, Decline}`;
  - tests.
- Modify: `deskwarden/src/vault_window/send_ui.rs` (the Received view draws invitations first; `invite:` keys open their detail)
- Modify: `deskwarden/src/vault_window/detail.rs` (`DetailAction::StopReceiving(String)` in the view-only kebab)
- Modify: `deskwarden/src/vault_window/mod.rs` (fill the rows; handle the asks; the leave question)
- Modify: `deskwarden/src/vault_window/item_list.rs` (`MODAL_SCRIM_AREAS` gains `"record-leave-scrim"`)

**Interfaces:**
- Produces:
  - `Sharing::accept(&self, id: &str) -> Result<(), SharingError>`
  - `Sharing::decline(&self, id: &str) -> Result<(), SharingError>`
  - `Sharing::leave(&self, organization_id: &str, items: &[VaultItem]) -> Result<SharingOutcome, SharingError>`
  - `share_ui::IncomingRow { key, id, from, expires: Option<String> }`
  - `share_ui::INVITE_KEY_PREFIX = "invite:"`
  - `share_ui::incoming_rows(view, now_ms) -> Vec<IncomingRow>`
  - `share_ui::draw_incoming_rows(ui, people, width, selected)`
  - `share_ui::draw_invitation_detail(ui, people, key)`
  - `share_ui::draw_leave_confirm(ctx, from: &str) -> Option<bool>`
  - `PeopleAsk::{Accept(String), Decline(String)}`
  - `PeopleRows.incoming: &'a [IncomingRow]`, `PeopleRows.waiting: &'a [String]`
  - `DetailAction::StopReceiving(String)`

- [ ] **Step 1: Write the failing tests**

Append to `src/rest/sharing.rs`'s tests:

```rust
    fn invited(server: &mut MockServer) {
        server
            .mock("GET", "/api/organizations/invitations/mine")
            .with_body(serde_json::json!({ "data": [{
                "id": "inv-1", "organizationId": "org-denis", "organizationName": "Shared by denis@example.com",
                "token": "tok-1", "expirationDate": "2999-01-01T00:00:00.000Z"
            }]}).to_string())
            .expect_at_least(0)
            .create();
    }

    /// **"denis@… wants to share with you"** (spec 6.9): the invitation is in
    /// the view, Accept posts its token, and it leaves the view.
    #[test]
    fn a_recipient_accepts_an_invitation_by_its_token() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect_at_least(0).create();
        invited(&mut server);
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        engine.refresh(Trigger::WholeRead).expect("probed");
        let view = engine.view(&[]);
        assert_eq!(view.incoming, vec![IncomingEntry { id: "inv-1".into(), from: "denis@example.com".into(), expires_at: Some("2999-01-01T00:00:00.000Z".into()) }]);
        let accepted = server
            .mock("POST", "/api/organizations/invitations/accept")
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" })))
            .with_body("")
            .expect(1)
            .create();
        engine.accept("inv-1").expect("accepted");
        accepted.assert();
        assert!(engine.view(&[]).incoming.is_empty());
        assert!(matches!(engine.accept("inv-1"), Err(SharingError::Refused(_))), "an invitation already answered is gone");
    }

    #[test]
    fn a_recipient_declines_an_invitation_by_its_token() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect_at_least(0).create();
        invited(&mut server);
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        engine.refresh(Trigger::WholeRead).expect("probed");
        let declined = server.mock("POST", "/api/organizations/invitations/decline")
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" }))).with_body("").expect(1).create();
        engine.decline("inv-1").expect("declined");
        declined.assert();
    }

    /// **Stop receiving from X** (spec 6.9): leave the sharer's org, and every
    /// record from them leaves the vault.
    #[test]
    fn stopping_receiving_leaves_the_org_and_removes_its_records() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        engine.held().others = vec![("org-denis".into(), "denis@example.com".into())];
        let left = server.mock("POST", "/api/organizations/org-denis/leave").with_body("").expect(1).create();
        let items: Vec<VaultItem> = ["a", "b"].iter().map(|id| serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "organizationId": if *id == "a" { "org-denis" } else { "org-other" }
        })).expect("item")).collect();
        let outcome = engine.leave("org-denis", &items).expect("left");
        left.assert();
        assert_eq!(outcome.removed, ["a"]);
        assert!(matches!(engine.leave("org-mine", &items), Err(SharingError::Refused(_))), "only someone else's sharing org can be left");
    }
```

Append to `share_ui.rs`'s tests:

```rust
    /// **The invitation row and its pane** (mock 6d, invite pending): who
    /// wants to share, "Nothing reaches your vault until you accept.", and
    /// Accept / Decline in both places.
    #[test]
    fn an_invitation_is_the_first_row_and_accepting_it_is_one_press() {
        let incoming = [IncomingRow { key: "invite:inv-1".into(), id: "inv-1".into(), from: "denis@x.io".into(), expires: Some("in 6 days".into()) }];
        let mut people = PeopleRows { incoming: &incoming, ..PeopleRows::none() };
        let mut selected = None;
        let texts = ui_frame(400.0, Vec::new(), |ui| draw_incoming_rows(ui, &mut people, 376.0, &mut selected));
        assert!(texts.iter().any(|(t, _)| t == "denis@x.io wants to share with you"), "{texts:?}");
        assert!(texts.iter().any(|(t, _)| t == INVITE_NOTHING_YET));
        let accept = texts.iter().find(|(t, _)| t == ACCEPT_LABEL).map(|(_, r)| r.center()).expect("Accept");
        let _ = ui_frame(400.0, click(accept), |ui| draw_incoming_rows(ui, &mut people, 376.0, &mut selected));
        assert_eq!(people.asked, Some(PeopleAsk::Accept("inv-1".into())));

        let mut people = PeopleRows { incoming: &incoming, ..PeopleRows::none() };
        let texts = ui_frame(700.0, Vec::new(), |ui| draw_invitation_detail(ui, &mut people, "invite:inv-1"));
        assert!(texts.iter().any(|(t, _)| t == "IF YOU ACCEPT"));
        assert!(texts.iter().any(|(t, _)| t == "in 6 days"));
        assert!(texts.iter().any(|(t, _)| t == DECLINE_LABEL));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib -j 2 rest::sharing::tests::a_recipient && cargo test --lib -j 2 vault_window::share_ui`
Expected: a compile error. `accept`, `decline`, `leave`, `IncomingRow` and `draw_incoming_rows` are not found.

- [ ] **Step 3: The engine and the routes**

`src/rest/sharing.rs`:

```rust
// ---- the recipient -----------------------------------------------------------------

impl Sharing {
    /// The token of an invitation this account still holds.
    fn token_of(&self, id: &str) -> Result<zeroize::Zeroizing<String>, SharingError> {
        self.held()
            .incoming
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.token.clone())
            .ok_or_else(|| SharingError::Refused("This invitation is no longer waiting.".to_string()))
    }

    /// **Accept an invitation** (spec 6.9). The sharer's daemon confirms, and
    /// the records arrive on the push that confirm produces.
    ///
    /// # Errors
    ///
    /// The invitation is gone, or the server refused.
    pub fn accept(&self, id: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let token = self.token_of(id)?;
        self.call(|client, auth| sharing_api::accept_invitation(client, &mut auth.session, &token))
            .map_err(|e| e.or_say("Couldn't accept the invitation."))?;
        self.held().incoming.retain(|i| i.id != id);
        Ok(())
    }

    /// **Decline an invitation.** Nothing is added to this vault; the sharer
    /// can invite again later.
    ///
    /// # Errors
    ///
    /// The invitation is gone, or the server refused.
    pub fn decline(&self, id: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let token = self.token_of(id)?;
        self.call(|client, auth| sharing_api::decline_invitation(client, &mut auth.session, &token))
            .map_err(|e| e.or_say("Couldn't decline the invitation."))?;
        self.held().incoming.retain(|i| i.id != id);
        Ok(())
    }

    /// **Stop receiving from a sharer** (spec 6.9): leave their sharing org.
    /// Every record from them is gone from this vault at once.
    ///
    /// # Errors
    ///
    /// Not someone else's sharing org, or the server refused.
    pub fn leave(&self, organization_id: &str, items: &[VaultItem]) -> Result<SharingOutcome, SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        if !self.held().others.iter().any(|(id, _)| id == organization_id) {
            return Err(SharingError::Refused("That is not someone sharing with you.".to_string()));
        }
        self.call(|client, auth| sharing_api::leave_organization(client, &mut auth.session, organization_id))
            .map_err(|e| e.or_say("Couldn't stop receiving."))?;
        self.held().others.retain(|(id, _)| id != organization_id);
        Ok(SharingOutcome {
            upserted: Vec::new(),
            removed: items
                .iter()
                .filter(|item| organization_of(item) == Some(organization_id))
                .map(|item| item.id.clone())
                .collect(),
        })
    }
}
```

`src/vault_api.rs` router:

```rust
            ("POST", ["sharing", "incoming", id, "accept"]) => self.incoming(id, true),
            ("POST", ["sharing", "incoming", id, "decline"]) => self.incoming(id, false),
            ("POST", ["sharing", "from", org, "leave"]) => self.leave_sharer(org),
```

and these methods:

```rust
    /// `POST /sharing/incoming/{id}/accept|decline` (spec 11.3).
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

    /// `POST /sharing/from/{orgId}/leave` (spec 11.3).
    fn leave_sharer(&self, org: &str) -> ApiResponse {
        let engine = match self.engine() {
            Ok(engine) => engine,
            Err(answer) => return answer,
        };
        match engine.leave(org, &self.cache.items()) {
            Ok(outcome) => {
                self.publish_outcome(&outcome);
                ApiResponse::done()
            }
            Err(e) => sharing_failed(&e),
        }
    }
```

- [ ] **Step 4: The rows, the pane and the question**

`share_ui.rs`:
- add `pub(crate) incoming: &'a [IncomingRow],` and `pub(crate) waiting: &'a [String],` to `PeopleRows`, and `incoming: &[], waiting: &[]` to `none()`;
- add `Accept(String), Decline(String),` to `PeopleAsk`;
- append:

```rust
// ---- the recipient's invitations (mock 6d, invite pending) --------------------------------

pub(crate) const INVITE_KEY_PREFIX: &str = "invite:";
pub(crate) const ACCEPT_LABEL: &str = "Accept";
pub(crate) const DECLINE_LABEL: &str = "Decline";
pub(crate) const INVITE_NOTHING_YET: &str = "Nothing reaches your vault until you accept.";
const INVITE_DECLINE_NOTE_TAIL: &str = "can invite you again later.";

/// "Waiting for denis@… to finish sharing" (spec 6.9): accepted, not yet
/// confirmed.
pub(crate) fn waiting_line(from: &str) -> String {
    format!("Waiting for {from} to finish sharing")
}

/// An invitation to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncomingRow {
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) from: String,
    /// "in 6 days", or `None` without an expiry.
    pub(crate) expires: Option<String>,
}

pub(crate) fn incoming_rows(view: Option<&SharingView>, now_ms: i64) -> Vec<IncomingRow> {
    let Some(view) = view else {
        return Vec::new();
    };
    view.incoming
        .iter()
        .map(|i| IncomingRow {
            key: format!("{INVITE_KEY_PREFIX}{}", i.id),
            id: i.id.clone(),
            from: i.from.clone(),
            expires: days_left(i.expires_at.as_deref(), now_ms).map(|d| if d == 1 { "in 1 day".to_string() } else { format!("in {d} days") }),
        })
        .collect()
}

/// **The invitation rows** at the top of Shared with me: a wash card with the
/// sharer's avatar, "X wants to share with you", and Accept / Decline. Then a
/// quiet line for every sharer this account is waiting on.
pub(crate) fn draw_incoming_rows(ui: &mut egui::Ui, people: &mut PeopleRows<'_>, width: f32, selected: &mut Option<String>) {
    for row in people.incoming {
        let picked = selected.as_deref() == Some(row.key.as_str());
        let framed = egui::Frame::new()
            .fill(theme::BLUE_WASH)
            .stroke(egui::Stroke::new(1.0, if picked { theme::BLUE } else { theme::BLUE_EDGE }))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 11))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                ui.horizontal(|ui| {
                    let (avatar, _) = ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
                    ui.painter().circle(avatar.center(), 20.0, theme::CARD, egui::Stroke::new(1.0, theme::BLUE_EDGE));
                    let initial = row.from.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    ui.painter().text(avatar.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(15.0), theme::BLUE);
                    ui.vertical(|ui| {
                        ui.label(theme::bold(format!("{} wants to share with you", row.from), 13.0).color(theme::BLUE_DEEP));
                        ui.label(egui::RichText::new(INVITE_NOTHING_YET).size(11.0).color(theme::TEXT_MUTED));
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(51.0);
                    if theme::primary_button(ui, ACCEPT_LABEL, None).clicked() {
                        people.asked = Some(PeopleAsk::Accept(row.id.clone()));
                    }
                    if theme::secondary_button(ui, DECLINE_LABEL).clicked() {
                        people.asked = Some(PeopleAsk::Decline(row.id.clone()));
                    }
                });
            });
        if framed.response.interact(egui::Sense::click()).clicked() {
            *selected = Some(row.key.clone());
        }
    }
    for from in people.waiting {
        ui.label(egui::RichText::new(waiting_line(from)).size(12.0).color(theme::TEXT_FAINT));
    }
}

/// **The invitation's pane** (mock 6d): who, Accept and Decline, and what
/// accepting means.
pub(crate) fn draw_invitation_detail(ui: &mut egui::Ui, people: &mut PeopleRows<'_>, key: &str) {
    let Some(row) = people.incoming.iter().find(|r| r.key == key).cloned() else {
        return;
    };
    egui::Frame::new().fill(theme::CARD).inner_margin(egui::Margin::symmetric(24, 20)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (avatar, _) = ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
            ui.painter().circle(avatar.center(), 20.0, theme::CARD, egui::Stroke::new(1.0, theme::BLUE_EDGE));
            let initial = row.from.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            ui.painter().text(avatar.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(15.0), theme::BLUE);
            ui.vertical(|ui| {
                ui.label(theme::bold(&row.from, 22.0).color(theme::INK));
                ui.label(egui::RichText::new("wants to share records with you").size(12.0).color(theme::TEXT_FAINT));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::secondary_button(ui, DECLINE_LABEL).clicked() {
                    people.asked = Some(PeopleAsk::Decline(row.id.clone()));
                }
                if theme::primary_button(ui, ACCEPT_LABEL, None).clicked() {
                    people.asked = Some(PeopleAsk::Accept(row.id.clone()));
                }
            });
        });
    });
    egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 18)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut rows = vec![
            ("You get", format!("The records {} shares with you, now and later", row.from), None),
            ("Access", "View-only".to_string(), Some("You can see, copy and autofill them. You cannot edit or delete them.")),
            ("They change", format!("When {} edits them", row.from), None),
        ];
        if let Some(expires) = &row.expires {
            rows.push(("Invite expires", expires.clone(), None));
        }
        fact_card(ui, "IF YOU ACCEPT", &rows);
        ui.add_space(10.0);
        ui.label(egui::RichText::new(format!("If you decline, nothing is added to your vault. {} {INVITE_DECLINE_NOTE_TAIL}", row.from)).size(12.0).color(theme::TEXT_FAINT));
    });
}

/// **"Stop receiving from X?"** -- every record from them leaves this vault.
/// `Some(true)` to leave, `Some(false)` to keep, `None` while undecided.
pub(crate) fn draw_leave_confirm(ctx: &egui::Context, from: &str) -> Option<bool> {
    let mut answer = None;
    theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("record-leave-scrim")));
    theme::movable_modal(ctx, egui::Area::new(egui::Id::new("record-leave-modal"))).show(ctx, |ui| {
        theme::modal_drag_handle(ui, theme::MODAL_PLAIN_HEADER_HEIGHT);
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(egui::CornerRadius::same(10))
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(theme::bold(format!("Stop receiving from {from}?"), 15.0).color(theme::INK));
                ui.add_space(10.0);
                ui.label(egui::RichText::new(format!("Every record {from} shares with you leaves your vault now. They can invite you again later.")).size(12.0).color(theme::TEXT_SECONDARY));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if theme::destructive_button(ui, "Stop receiving").clicked() {
                        answer = Some(true);
                    }
                    ui.add_space(theme::FORM_FOOTER_GAP);
                    if theme::secondary_button(ui, "Keep").clicked() {
                        answer = Some(false);
                    }
                });
            });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    answer
}
```

`fact_card`'s rows are `&[(&str, String, Option<&str>)]`. Build `rows` with that element type. The `vec!` above infers it from the first element.

`send_ui.rs`:
- In the Received arm, before `draw_received_group`, call `super::share_ui::draw_incoming_rows(ui, people, row_width, selected);`.
- Add `people.incoming.len()` to the `rows_area` count. Change the empty condition to `rows.is_empty() && people.received.is_empty() && people.incoming.is_empty() && people.waiting.is_empty()`.
- In `draw_send_detail`'s `SendView::Received(rows)` arm, start with:

```rust
            if let Some(key) = selected.as_deref().filter(|key| key.starts_with(super::share_ui::INVITE_KEY_PREFIX)) {
                let key = key.to_string();
                super::share_ui::draw_invitation_detail(ui, people, &key);
                return action;
            }
```

`item_list.rs`: add `"record-leave-scrim",` to `MODAL_SCRIM_AREAS`, in sorted position.

`detail.rs`:
- add `StopReceiving(String)` to `DetailAction`, with the doc `/// A received record's kebab: stop receiving from its sharer (the organisation id).`;
- the view-only kebab gains, above the footer:

```rust
                        if let Some(org) = crate::rest::organizations::organization_of(item) {
                            if ui.button(format!("Stop receiving from {from}")).clicked() {
                                action = DetailAction::StopReceiving(org.to_string());
                                ui.close();
                            }
                            ui.separator();
                        }
```

`view_only` lets `StopReceiving` through, since it is not in its list. In `detail_action_exposes_secrets`, add `| DetailAction::StopReceiving(_)` to the non-exposing side.

`mod.rs`:
- declare `let mut leave_ask: Option<(String, String)> = None;` beside the sharing locals;
- in the detail match, add:

```rust
                                DetailAction::StopReceiving(org) => {
                                    let from = sharing.view.as_ref().and_then(|v| v.received_from(&item.id)).unwrap_or_default().to_string();
                                    leave_ask = Some((org, from));
                                }
```

After the forget-confirm block, add:

```rust
        if let Some((org, from)) = leave_ask.clone() {
            match share_ui::draw_leave_confirm(ui.ctx(), &from) {
                Some(true) => {
                    share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Leave(org));
                    leave_ask = None;
                }
                Some(false) => leave_ask = None,
                None => {}
            }
        }
```

In the `PeopleRows` literal, add:
- `incoming: &incoming_rows,`, having built `let incoming_rows = share_ui::incoming_rows(sharing.view.as_ref(), crate::send::SendClock::now_unix_millis(&crate::send::SystemClock));`;
- `waiting: &waiting_from,`, having built `let waiting_from: Vec<String> = sharing.view.as_ref().map(|v| v.waiting.iter().map(|w| w.from.clone()).collect()).unwrap_or_default();`.

Extend the `people_rows.asked` match with:

```rust
            Some(share_ui::PeopleAsk::Accept(id)) => {
                share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Accept(id));
            }
            Some(share_ui::PeopleAsk::Decline(id)) => {
                share_ui::request(&mut sharing, sharing_client.as_ref(), ui.ctx(), &sharing_tx, share_ui::SharingCall::Decline(id));
            }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
python "$TEMP/dw_crlf.py" src/rest/sharing.rs src/vault_api.rs src/vault_window/share_ui.rs src/vault_window/send_ui.rs src/vault_window/detail.rs src/vault_window/mod.rs src/vault_window/item_list.rs
cargo test --lib -j 2 rest::sharing && cargo test --lib -j 2 vault_api && cargo test --lib -j 2 vault_window::
```
Expected: every file reports `0`, and all pass, including `every_modal_scrim_in_the_crate_is_named_here`.

- [ ] **Step 6: Verify and commit**

Run the four verification commands.

```bash
git add deskwarden/src/rest/sharing.rs deskwarden/src/vault_api.rs deskwarden/src/vault_window/share_ui.rs deskwarden/src/vault_window/send_ui.rs deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/mod.rs deskwarden/src/vault_window/item_list.rs
git commit -m "A recipient can accept, decline, wait, or stop receiving

On NodeWarden the daemon reads invitations/mine. Shared with me opens
with 'denis@… wants to share with you' and Accept / Decline, in the row
and in its own pane with IF YOU ACCEPT (design 6d); accept and decline
post the invitation's token. An accepted-but-unconfirmed sharer shows as
'Waiting for X to finish sharing'.

A received record's menu offers Stop receiving from X, which asks
first, leaves the sharer's org, and takes every record from them out of
the vault through the feed. record-leave-scrim joins MODAL_SCRIM_AREAS.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 21: The test badge, and the whole suite once more

**Files:**
- Modify: `E:\Personal\node-bitwarden\README.md` (line 12, the tests badge), only if the count left its window

**Interfaces:** none.

- [ ] **Step 1: Count the tests the way the pin counts them**

`job_object::the_readme_test_count_is_not_a_boast` counts every literal `#[test]` in every `.rs` under `deskwarden/src`. It requires `claimed <= actual < claimed + 200`.

Run (from `E:\Personal\node-bitwarden`):
```bash
python - <<'EOF'
import pathlib, re
n = sum(p.read_text(encoding='utf-8', errors='replace').count('#[test]') for p in pathlib.Path('deskwarden/src').rglob('*.rs'))
badge = re.search(r'/badge/tests-(\d+)%2B', open('README.md', encoding='utf-8').read()).group(1)
print('actual', n, 'claimed', badge)
EOF
```
Expected: two numbers.

- [ ] **Step 2: Re-state the badge only if it is out of its window**

If `actual >= claimed + 200`, change the badge's number to `actual` rounded down to the hundred (for example 5923 becomes `5900`). Edit only the number in `tests-5700%2B`. Otherwise leave `README.md` untouched.

Run: `cargo test --lib -j 2 job_object::tests::the_readme_test_count_is_not_a_boast`
Expected: PASS.

- [ ] **Step 3: The whole suite, both targets, no warnings**

Run (from `deskwarden/`):
```bash
python "$TEMP/dw_crlf.py" $(git diff --name-only HEAD~20 -- '*.rs' | sed 's#^deskwarden/##')
cargo test --lib -j 2
cargo test --bins -j 2
cargo check --all-targets
RUSTFLAGS="-D warnings" cargo test --no-run
```

Expected:
- every `.rs` file this plan touched reports `0` lone LF;
- `--lib` passes;
- `--bins` passes (`main.rs`'s tests run only here);
- `check` prints no warning;
- the `-D warnings` build succeeds.

`HEAD~20` covers this plan's commits. Adjust the count to the number of commits made. The `sed` there is only a pipe over file names, never an edit of a source file.

- [ ] **Step 4: Commit (only if the badge moved)**

```bash
git add README.md
git commit -m "The test badge says how many tests there are after personal sharing

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage** (the spec's sections, against the tasks that build them, within this plan's NodeWarden-only scope):

| Spec | Where |
|---|---|
| §5.1 sharing org: name, cut to 50, recognition, cached id, created on first share | Tasks 5, 6, 9 |
| §5.2 person collection: encrypted name, sharer manages, read-only access, kept after the last share | Tasks 4, 9, 10 |
| §5.3 one cipher, many collections | Tasks 9, 10 |
| §5.4 `sharing_state.json` (pins, moves, unfinished) | Tasks 6, 9, 10, 12 |
| §6.1 all seven states, and the pill's promise | Tasks 7, 14, 15, 17 |
| §6.2-6.4 first share, new person, existing person | Task 9 |
| §6.5 no account (NodeWarden), re-invite | Tasks 9, 11, 13, 16 |
| §6.6-6.7 auto-confirm | Task 12 |
| §6.8 stop, last-person move back, forget | Tasks 10, 11, 16, 17 |
| §6.9 recipient: direct add, invitations/mine, waiting to finish, leave, view-only, hidden org | Tasks 5, 19, 20 |
| §6.10 rail counts | Task 15 |
| §7.1 dialect probe (once, slice-1 check) | Task 7 |
| §7.2-7.3 NodeWarden column | Task 4 |
| §7.4 `bw serve`: unavailable, Share offers a Send | Tasks 8, 15, 16 |
| §9.1-9.6 crypto, org creation, names, move in and out, confirm, fingerprint | Tasks 1, 2, 9, 10, 12 |
| §11.1-11.7 modules, engine placement, routes, feed kind with the unknown arm, auto-confirm triggers and cost, directory, suggestions and forget | Tasks 5, 7, 8, 9-13, 16 |
| §12 pins, collection-name authentication, fingerprint | Task 12 |
| §13 error sentences, idempotent retries, the two-PC race | Tasks 4, 9, 10 |
| §14.1 request-counting tests | Tasks 7-13, 20 |
| §14.2 crypto round trips, fingerprint vector | Tasks 1, 2 |

**Left out on purpose, and why:**
- Every official-dialect route (§7.3's right column, §8, pasted invite links, the adopt-a-licensed-org path, the roster disclosure line). Owner's decision: NodeWarden first.
- §14.1's `an_accept_on_an_official_server_is_confirmed_by_the_timer_not_a_sync` becomes `an_accepted_person_is_confirmed_by_the_timer_without_a_sync` (Task 12).
- `the_first_share_adopts_a_collection_the_server_made_from_collection_name` is a Vaultwarden case. The adoption itself is built (Task 9 reads the new org's collections by name), but no test forges a server-made name. Such a name would need the freshly generated org key.
- §5.2's merge of two collections that decrypt to one email is **not built**: the first in server order is used. It needs a per-collection item listing that §7.3 does not name. Recorded in the report.
- The toast's copy after a share that waits for an accept differs from mock 6a's "Invitation sent to …", which describes an email NodeWarden does not send (decision 4).

**2. Placeholder scan.**
- Task 2 Step 4's `EFF_WORDLIST_SHA256` bytes are the only value the plan cannot contain. Step 1 computes them from the downloaded file with an exact script and names where they go. The EFF list is not in this workspace and no network download was made while planning.
- Four steps name a crate-internal test helper by the role it plays and give the line to find it at: `login_item`, `scrolled_rail`, `an_api_over`, `hear_vault`. Each says what to do if its spelling differs. These are existing helpers, not new code left unwritten.
- No step says "add error handling" or "similar to Task N".

**3. Type consistency**, checked across tasks:
- `SharingView`, `PersonState` and `PersonEntry` are defined in Task 7 and read in Tasks 14-20.
- `ShareResult` is defined in Task 9 and read in Tasks 11, 15 and 16.
- `SharingSetup::{production, none}` and `make_key_pair: KeyPairMaker` are defined in Task 7 and used by every engine test.
- `Ctx`, `context`, `ensure_member`, `reload_org` and `stop_with` are defined in Tasks 9-10 and reused in Tasks 11-12.
- `SharingClient::{view, share, stop, forget, reinvite, confirm, fingerprint, invite_code, accept, decline, leave}` come from Tasks 8-15. `share_ui::run` calls every one; `accept`, `decline` and `leave` are added in Task 15 so `run` compiles, and their routes arrive in Task 20.
- `PeopleRows` grows in Tasks 18-20, and `PeopleRows::none()` is updated each time: `received` in Task 19, `incoming` and `waiting` in Task 20.
- `PeopleSharing` is defined in Task 16 and used in Tasks 17 and 19.
- `DetailAction` gains `ShareRecord` (Task 16), seven people variants (Task 17) and `StopReceiving` (Task 20). Each is added to `detail_action_exposes_secrets`, whose match is exhaustive, in the same task.

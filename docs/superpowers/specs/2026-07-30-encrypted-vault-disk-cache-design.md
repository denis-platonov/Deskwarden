# Encrypted vault disk cache — design

**Status:** approved, ready for planning
**Date:** 2026-07-30
**Follows:** [vault cache and `bw serve` lifecycle](2026-07-30-vault-cache-and-backend-lifecycle-design.md)

## Goal

Make deskwarden useful the instant its tray icon appears, and make the vault
window open instantly after a restart — without `bw serve` running at all.

The previous design moved every read onto an in-memory `VaultCache`, so the
backend is now only needed for sync, writes and TOTP. But the snapshot is
built once per unlock, and building it costs one `bw serve` cold start:
**~8 s** from spawn to ready, plus **~1.1 s** for the first
`GET /list/object/items` (1.08 MB, 1657 items). Every launch and every reboot
pays it. Autofill is dead for those eight seconds; so is the vault window's
content.

This feature optionally persists the snapshot to disk, encrypted, so the next
launch reads it in milliseconds and `bw serve` never starts until something
genuinely needs it.

**Off by default.** Every behaviour described here is inert unless the user
turns the setting on.

## Background

The predecessor spec considered a disk cache and rejected it in one
paragraph:

> A disk cache was considered and rejected. It would make the first open after
> a restart instant, but it puts decrypted vault data at rest, which
> contradicts the README's claim that deskwarden "never touches encryption,
> key derivation, or sync logic itself". DPAPI would protect it as
> `session.bin` is protected, so this is a posture decision rather than a
> crypto one — but the claim is worth keeping true as written.

That reasoning still holds for the *default*. What changed is that the user
looked at the actual exposure and decided it is worth offering as a choice,
with the tradeoff stated where it is made. The reasoning, verbatim in
substance:

An attacker with code execution as this Windows user already has easier paths
than attacking an encrypted file. `session.bin` is DPAPI-wrapped for that same
user and hands over a working session token — feed it to `bw` and the whole
vault is readable. In practice the manager is often sitting unlocked anyway.
Hardening against the hardest available path while the easy ones remain open
buys very little.

There is exactly one case that argument does not cover, and it is the reason
the rest of this document is mostly about encryption: **a stolen or imaged
disk.** Today a powered-off machine yields nothing durable. The vault exists
only inside `bw`'s own encrypted database, which needs the master password,
and a captured `session.bin` expires. A surviving vault dump does not expire.
Disk plus the Windows account password would yield the full vault,
permanently, without the Bitwarden master password ever being typed. That is
the real delta this feature introduces, and it is what the encryption design
below is built to close.

## Architecture

### 1. What secret gates the file

This is the whole design decision, so state it as a choice between three
answers:

| Gate | Startup cost | Stolen disk + Windows password | Verdict |
|---|---|---|---|
| **DPAPI only** | none | **yields the full vault** | insufficient |
| **Master password** | typed every launch | useless to the attacker | breaks the feature |
| **Hello-sealed random key** | one biometric touch | useless without this machine's TPM | **chosen** |

**Master-password keying was rejected.** It closes the gap cleanly — nobody
argues otherwise — but it breaks the feature it is protecting. deskwarden
deliberately starts *unlocked* from the DPAPI-wrapped session token in
`session.bin`, and at startup it has never seen the master password. There is
no code path in which it holds one. Encrypting the cache under it therefore
means either prompting for the master password at every launch, which
destroys the entire premise ("autofill works the moment the tray appears"), or
being structurally unable to decrypt the file the app itself wrote. Both are
worse than not shipping the feature.

**Record this so nobody re-derives the dead end.** It looks like the obvious
right answer, it is the first thing anyone proposes, and it is a cul-de-sac
for a reason that is not obvious until you notice the app never has the
password.

**Chosen: a random content key, sealed under Windows Hello.** This is not new
crypto — it is `hello.rs`'s existing pattern applied to a second secret:

1. A Windows Hello **KeyCredential** signs a fixed challenge. The private key
   lives in the TPM, and every `RequestSignAsync` forces the OS's Hello
   verification. RSA PKCS#1 v1.5 signing is deterministic, so the signature is
   stable high-entropy key material that does not exist until the user has
   verified.
2. `SHA-256(label ‖ signature)` derives an AES-256 key.
3. That key seals a **random 32-byte content key** with AES-256-GCM.
4. The content key encrypts the snapshot with AES-256-GCM.
5. The whole file is DPAPI-wrapped, exactly as `hello.bin` is.

`aes-gcm`, `sha2`, `zeroize` and `getrandom` are already dependencies, used
for precisely this in `hello.rs`. No new crate, no new primitive, no new
threat surface from unfamiliar code.

The property this buys, which DPAPI alone cannot claim: **Hello's key is
TPM-bound.** DPAPI derives from the Windows account credentials, which travel
with a disk image. A Hello-sealed key does not leave the machine that holds
the TPM. A stolen disk plus a known Windows account password is still not
enough.

Two details that matter and are easy to get wrong:

- **A distinct KDF label.** The cache reuses the *same* KeyCredential as quick
  unlock (`deskwarden-quick-unlock`) — one credential, two sealed blobs — but
  it must derive its key under its own domain-separation label, e.g.
  `b"deskwarden vault cache aes key v1"`. Sharing `hello.rs`'s
  `KDF_LABEL` would make the two blobs cross-decryptable, which is sloppy for
  no gain.
- **The cache must never call `RequestCreateAsync(ReplaceExisting)`.**
  `hello::enroll` uses `ReplaceExisting` today, which is correct there. If the
  cache path did the same it would silently rotate the credential and destroy
  an existing quick-unlock enrollment. The cache opens the credential
  (`OpenAsync`) and only creates one — with `FailIfExists` — when none is
  found. The reverse direction (a user enrolling quick unlock *after* turning
  the cache on, rotating the credential out from under the cache file) is
  handled by self-healing: a failed unseal deletes the file and repopulates,
  the same way `hello::unlock_password` already deletes a blob it cannot open.

### 2. File format

One file, `vault-cache.bin`, in the config directory beside `session.bin` and
`hello.bin`. Not the cache directory: `update_download_dir` and the favicon
cache live there because they are disposable and regenerable, and this is
neither innocuous nor something we want swept up by a cache cleaner while the
app is running.

Inside the DPAPI envelope, three parts:

```
DPAPI(
  header_len : u32
  header     : JSON  — format version, written_at, account fingerprint, item count
  sealed_key : nonce ‖ AES-256-GCM( Hello-derived key, random content key )
  body       : nonce ‖ AES-256-GCM( content key, snapshot, aad = header )
)
```

- **The header is plaintext inside the envelope, and that is deliberate.**
  DPAPI unwrapping is silent and non-interactive
  (`CRYPTPROTECT_UI_FORBIDDEN`), so the app can read the header, decide the
  file is expired or belongs to a different account, and delete it **without
  ever popping a Hello prompt.** Prompting the user for a biometric and then
  throwing the file away would be an insult.
- **The header is the GCM AAD**, so `written_at` cannot be edited to defeat
  expiry without failing authentication. Note this is the one place the design
  goes beyond `hello.rs`'s pattern, which uses bare `encrypt`; here we need
  `Payload { msg, aad }`.
- **The account fingerprint is a hash**, `SHA-256(userEmail ‖ serverUrl)` from
  `bw status` (already parsed by `login_ui::parse_bw_status_details`), not the
  values themselves. The file should not be the thing that tells an examiner
  whose vault it is.
- **The body is the existing snapshot types**, `Vec<VaultItem>` and
  `Vec<Folder>`, serialized with serde. Their `#[serde(flatten)] other`
  catch-alls round-trip unknown fields, which is already tested — a disk
  round-trip must not become the place that drops a field the server sent.
- **Writes are atomic**: write `vault-cache.bin.tmp`, then rename over the
  target. `std::fs::rename` on Windows replaces an existing destination. A
  crash mid-write must not leave a truncated file whose corruption costs a
  Hello prompt to discover.

Size: ~1.1 MB for a 1657-item vault, ciphertext being roughly plaintext-sized.
Write cost is milliseconds. This is not a performance consideration.

### 3. Lifecycle rules

Each of these is a settled requirement, with the reason it was chosen.

**The file survives a lock.** This is the load-bearing decision. Locking
clears the *in-memory* snapshot as it does today — that property exists to
stop a previous account's items rendering after a re-auth, and it stays — but
the file on disk is left alone. Rationale: the marginal exposure of a
surviving file is narrow, because anyone who can read it as this Windows user
can already use `session.bin` to drive `bw` directly, and in practice the
manager is often unlocked anyway. Deleting on lock would give up nearly all of
the feature's benefit (the whole point is surviving a restart) in exchange for
closing the hardest path while the easy ones stay open.

**The file is deleted on explicit log out.** Log out is not lock: it means the
account is gone from this machine. `login_ui.rs`'s `LoginAction::LogOut`
handler already runs `bw logout` and then `hello::unenroll()`, on the stated
principle that "a sealed master password for an account the CLI no longer
knows is a liability, not a feature". A vault dump for that same account is a
larger liability. Delete it in the same place, for the same reason.

**The file expires after 7 days.** Concretely: on load, if
`now - written_at > 7 days`, the file is deleted unread and the app falls back
to the backend.

Why 7 and not something else:

- The expiry only bites on a machine that has been *off or unused* for the
  whole period, because the file is rewritten on every successful populate —
  every launch, every sync, every vault-window refresh. On a machine in daily
  use the file is hours old, never days. So the number is chosen entirely for
  the abandoned-machine case, which is exactly the stolen-disk case.
- It must survive the normal gaps in a person's usage. A laptop shut in a
  drawer on Friday and opened on Monday, or over a public holiday, must still
  start instantly — otherwise the feature quietly stops working for the users
  who most notice cold starts. A week covers every ordinary gap.
- It must be short enough that expiry is a real mitigation. 30 days is
  theatre: a stolen disk is imaged in days, and a month-old vault dump is
  still overwhelmingly accurate. 1 day is honest but breaks the weekend case
  and makes the feature feel unreliable. A week is the shortest interval that
  does not visibly cost the user anything.
- 7 days matches the interval users already read as "a while, but bounded"
  from every "remember this device" control they have used.

It is a constant with a named justification, not a setting. Making it
configurable invites `expiry_days: 3650`, which is the same as no expiry with
extra steps.

**A master-password change invalidates the cache.** We cannot detect the
change itself — `bw status` exposes no key fingerprint, and the session token
that *does* change is regenerated on every unlock, so it is useless as a
marker. Instead, use the fact that a master-password change always invalidates
the session and forces a re-authentication: **any path that prompts for the
master password deletes the cache file before repopulating.** That is a
superset of the case we care about (it also covers a plain session expiry),
and it costs nothing, because at that exact moment the backend is already up
and the in-memory snapshot is already being rebuilt. The file is simply
rewritten from the fresh snapshot a second later.

Account switches are caught separately and earlier, by the header's account
fingerprint at load time.

### 4. Fallback when Hello is not enrolled

**The setting is unavailable, with an inline explanation of why.**

Deliberately *not* a silent downgrade to a DPAPI-only variant under the same
label. The setting's entire value is the TPM binding — that is the one
property that distinguishes it from what we already rejected, and it is the
only reason the stolen-disk paragraph in the UI copy can say what it says. A
DPAPI-only file offered under a control the user read as "protected by
Windows Hello and this PC's TPM" would be a straightforwardly misleading
security claim, and this app has already had to correct one over-broad
security claim in its README this month.

**This is a product call the user may override.** The alternative — offer the
DPAPI-only variant, but under a different label with its own honest
description — is defensible and would let more users benefit. It is not what
is specified here, and changing it means writing a second description, not
just relaxing a check.

`hello::state()` already returns exactly what is needed
(`available` from `IsSupportedAsync`, `enrolled` from the blob's existence).
The cache needs `available`, not `enrolled` — quick unlock is not a
prerequisite, the cache can create its own credential when none exists.

### 5. Settings and UI copy

One new field on `Settings`, defaulting to today's behaviour:

```rust
/// Whether the vault snapshot is persisted to disk, encrypted under a
/// Windows Hello-sealed key. Off by default.
pub cache_vault_to_disk: bool,   // default: false
```

`#[serde(default)]` on the struct already makes an older `settings.json` parse
with this absent, which is the behaviour the existing partial-file test pins.

The toggle goes in **Preferences → General**, below the backend toggle, using
`prefs_ui::toggle_row` as it stands. 3e's Security section would be the more
natural home, but it does not exist and this setting is not enough to justify
building it.

**This is the copy a user reads before accepting a security tradeoff, so it
is drafted here rather than left to implementation.**

Label:

> Keep an encrypted copy of your vault on this PC

Description (available state):

> Deskwarden opens instantly after a restart and autofill works the moment it
> starts, instead of waiting about 8 seconds for the Bitwarden backend.
>
> The copy contains your usernames, passwords, notes and two-factor secrets.
> It is encrypted with a key that Windows Hello keeps in this PC's TPM chip,
> so a copied disk cannot be read on another machine. It is **not** deleted
> when your vault locks — only when you log out, or after 7 days. Anyone who
> can run programs as you on this PC and pass Windows Hello can read it.

Description (unavailable state — Hello not set up):

> Unavailable — needs Windows Hello.
>
> This copy is protected by a key held in your PC's TPM chip, which only
> Windows Hello can release. Without Hello there is no such key, and
> Deskwarden will not store your vault on disk under weaker protection than
> this setting describes. Set Hello up in Windows Settings → Accounts →
> Sign-in options.

Notes on the copy, since the wording is the requirement:

- It names what is in the file. "Vault data" is a euphemism; "usernames,
  passwords, notes and two-factor secrets" is what is actually written.
- It states the lock behaviour in the negative, in bold, because that is the
  part a reasonable person would assume goes the other way.
- It names the residual attacker in plain terms rather than implying the file
  is safe from everything.
- It does not use the word "secure".

Turning the setting **on** triggers the Hello prompt immediately (open or
create the credential, then write the first file from the snapshot already in
memory), which doubles as the confirmation gesture — no separate modal.
Turning it **off** deletes the file immediately, before the settings write.

## Data flow

**Startup, cache enabled and file usable** → DPAPI unwrap → header checks
(version, expiry, account fingerprint) → Hello prompt → decrypt → snapshot
populated → match engine built → tray appears, autofill live. `bw serve` is
**not** started. In `keep_backend_running` mode it is started in the
background anyway, per the existing policy; in save-memory mode it stays down
until a sync, write or TOTP asks for it.

**Startup, cache enabled and file missing/expired/foreign/undecryptable** →
delete the file if present → today's path exactly: start `bw serve`, wait for
readiness, populate, reconcile the backend policy → write the file.

**Startup, cache disabled** → today's path, and no file is written. If a file
exists from a previous enablement, it is deleted (turning the setting off
already deletes it; this is the belt-and-braces case where the setting was
edited in `settings.json` by hand).

**After any successful `populate()`** → write the file. This is the single
write point.

**After any successful mutation** (create/update/delete item or folder,
`set_app_match`) → rewrite the file from the updated snapshot. Mutations are
human-paced and the write is ~1 MB, so there is no need for debouncing, and
consistency is worth more than the milliseconds.

**Lock** → clear the in-memory snapshot (unchanged). Leave the file.

**Re-authentication** (any master-password prompt) → delete the file, then
repopulate and rewrite.

**Log out** → delete the file, beside the existing `hello::unenroll()`.

**Quit** → clear the in-memory snapshot (unchanged). Leave the file.

Every one of these lives inside the cache module, for the reason the
predecessor spec established: *there is exactly one place that can be wrong.*
No call site persists, deletes, or reasons about the file directly.

## Security and threat model

Stated precisely, because the honest version is the point.

**What this protects against**

- **A stolen or imaged disk, with or without the Windows account password.**
  The content key is sealed under a Hello credential whose private key is in
  this machine's TPM. Restoring the image elsewhere, or mounting the disk and
  DPAPI-unwrapping with known Windows credentials, yields the header and two
  ciphertexts and nothing else. This is the case DPAPI alone cannot cover,
  and it is the reason to build it this way.
- **Another user account on the same machine.** DPAPI is scoped to this
  Windows user; the Hello credential is scoped to this user's Hello
  enrollment.
- **Tampering.** AES-GCM authenticates the body, with the header as AAD, so
  neither the snapshot nor `written_at` can be edited. A tampered file fails
  to open and is deleted.
- **Unbounded staleness of the exposure.** After 7 days the file is refused
  and deleted rather than loaded.

**What this does not protect against, and must not be described as if it did**

- **Code running as this Windows user, when Hello can be satisfied.** It can
  prompt Hello — or wait for the user to satisfy a prompt it triggered — and
  read the file. This is not a regression: the same attacker can read
  `session.bin` and drive `bw` directly, which is easier. But the file is one
  more thing they can take.
- **A live, unlocked deskwarden.** The decrypted snapshot is in process
  memory, as it already is today.
- **The exposure being durable.** This is the genuine, accepted change to the
  app's posture. Today a compromised-then-cleaned machine leaks a session
  token that expires. With this on, it can leak a vault dump that does not.
  The 7-day expiry bounds the window in which a *cold* copy stays loadable by
  the app; it does nothing for an attacker who already decrypted it.
- **Individual zeroization of the decrypted snapshot.** Already a known,
  triaged gap for the in-memory cache: `items()` hands out clones, so the
  vault window holds a full copy while open, and zeroizing only the cache's
  copy would give false confidence. The agreed shape of that fix — a
  `Zeroizing`-wrapped `LoginData.password` so every clone self-wipes — is
  unchanged by this feature and remains a separate follow-up. Decryption
  buffers introduced *here* should be `Zeroizing` from the start regardless.

**The one-line summary for anyone auditing this:** with the setting on, a
current copy of your vault is stored on this PC's disk, readable by anything
running as you that can pass Windows Hello, and by nothing else.

## Staleness across restarts

A memory-only cache can only ever be as old as the current session. A disk
cache can be **older than the server**, and the app can now start, paint a
full vault window, and autofill — all from data written days ago — before it
has spoken to anything.

**Load, then refresh.** The snapshot is presented immediately, and a refresh
is started behind it under the normal backend policy. The user is never made
to wait for the network to see their vault.

**The status indicator must never claim fresh data when the refresh failed.**
This app just spent a review round on a bug of exactly this shape: the vault
window's `spawn_vault_load` raced the backend cold start, `populate()` got
connection-refused, and the window shipped the pre-sync snapshot while the
toolbar pill read "Synced just now" (final review Important 1, fixed in
292a55c). A disk cache makes that failure mode strictly worse, because the
gap between what the pill claims and what is on screen can now be days rather
than one sync interval.

Concretely:

- The loaded snapshot carries the header's `written_at` as its known age. The
  toolbar pill reads from that age until a sync succeeds **in this session**:
  "Loaded from cache · 3 h old", not "Synced just now".
- A failed refresh leaves the age wording in place and surfaces the failure —
  it never upgrades the pill.
- `last_sync_at` stays a per-session value, exactly as its comment says today.
  Do not repurpose it to mean "when the file was written"; they answer
  different questions and conflating them is how the pill starts lying again.
- Autofill from a stale snapshot is acceptable and is not new: `bw serve`
  reads the same local database that only `bw sync` updates, and `bw sync`
  already only runs at startup and on demand. The final whole-branch review
  established this explicitly. What is new is the *magnitude* of the possible
  staleness, which is what the pill's wording exists to communicate.

## Error handling

Every failure below falls back to the pre-feature path. Nothing here is ever a
reason the app cannot start.

- **Hello unavailable at runtime** (was enrolled, now is not) — log, delete
  the file, populate from the backend, and re-render the setting as
  unavailable next time Preferences opens.
- **Hello cancelled or failed at startup** — do not retry, do not block the
  tray. Fall back to the backend path. Leave the file: the user cancelling a
  prompt is not a reason to throw away their cache. Log at `info`, not
  `error` — a cancelled biometric is a user decision, not a fault.
- **DPAPI unwrap fails / file truncated / version unknown / GCM
  authentication fails** — delete the file, log, populate from the backend.
  Same self-healing posture as `hello::unlock_password`: a blob that can never
  be opened again is worse than no blob.
- **Header expired, or account fingerprint mismatch** — delete unread, no
  Hello prompt, populate from the backend.
- **`written_at` in the future** beyond a small tolerance (clock moved
  backwards, file copied from another machine's timeline) — treat as invalid
  and delete. An unbounded future timestamp is an expiry that never fires.
- **Write fails** (disk full, permissions, antivirus lock) — log at `warn`
  and continue. The in-memory cache is authoritative and the app is fully
  functional; the only cost is a slow next launch. Never surface a modal for
  this.
- **Deletion fails** on log out or on disabling the setting — this one is
  worth surfacing, because the user asked for the file to be gone and it is
  not. Log at `error` and report it in the UI where the action was taken.

## Risks

- **Silently weaker than advertised.** If any path ends up writing the file
  without the Hello seal — a refactor, a fallback added later "just for
  robustness" — the UI copy becomes false. Mitigation: there is one write
  function, it takes the sealed key as a parameter, and there is no code path
  that constructs the file without one.
- **Breaking quick unlock.** Reusing the KeyCredential is the right call, but
  a stray `ReplaceExisting` from the cache path silently invalidates
  `hello.bin`. Mitigation: named as a requirement above, and covered by a test
  that both blobs remain openable after the cache path acquires its key.
- **The pill lying again.** See the staleness section; this is the highest
  likelihood defect in the whole feature, because it is a bug this codebase
  has already shipped once in a narrower form.
- **Users enabling it without reading.** Mitigated by copy, not code. The
  description is always visible above the toggle, and the Hello prompt on
  enabling is a deliberate gesture.
- **Feature creep into "offline mode".** Everything here is a cache in front
  of the CLI. It must not become a second source of truth: no writes are
  accepted while the backend is unreachable, no queue, no reconciliation.

## Testing

- **Round-trip**: seal/unseal and encrypt/decrypt with a fixed key, exactly as
  `hello.rs`'s tests do — no Hello hardware required, because key derivation
  is already split out as a pure function and the cache's should be too.
- **Rejection**: wrong key, tampered ciphertext, tampered header (proving the
  AAD binding is live), truncated file, unknown format version.
- **Expiry**: a header at 6 days 23 h loads; at 7 days 1 min it is deleted
  unread. Assert the file is gone, and assert no key derivation was attempted
  — the "no Hello prompt for a doomed file" property is behavioural, not
  incidental.
- **Account fingerprint**: a file written under one fingerprint is deleted
  when loaded under another.
- **Future timestamp**: rejected and deleted.
- **Lifecycle table**: (setting, event) → file present or absent, covering
  lock (present), log out (absent), re-auth (rewritten), disable (absent),
  quit (present). Table-tested rather than checked by hand, matching how
  `backend_policy::should_run` is tested.
- **Serde fidelity**: an item carrying unknown fields in its `other`
  catch-all, and a `UriEntry` with a `match` key, survive a disk round-trip
  unchanged. This codebase has shipped that exact bug twice in different
  structs; the disk path must not be the third.
- **Staleness wording**: a snapshot loaded from a file written N hours ago,
  with a refresh that fails, reports the age and does not report a successful
  sync.
- **Write atomicity**: a `.tmp` file left behind by a simulated crash does not
  affect the next load, and is cleaned up.
- **Disabled by default**: with default settings, no file is ever created.
  Assert on the filesystem, not on a flag.

## Documentation changes

Both READMEs make security claims that were corrected *this month* for the
in-memory cache and become wrong again the moment this ships. Both need
another pass, and both need it written so the claim is true whether the
setting is on or off.

**`README.md`** — the "Nothing here re-implements vault security" bullet
currently ends "reads are served from an in-memory snapshot of what the CLI
returned, held only while the vault is unlocked." *Held only while the vault
is unlocked* stops being true. It needs the opt-in disk cache named, with the
Hello/TPM protection and the survives-a-lock behaviour stated, not implied.
The memory-footprint paragraph further down should also mention that with the
disk cache on, save-memory mode no longer pays a cold start on the first
operation after a restart — that is the combination the two features were
built for.

**`deskwarden/README.md`** — two places. The intro paragraph carries the same
"held only while the vault is unlocked" phrasing. The **Security notes** list
needs a new bullet, in the same flat register as the DPAPI and job-object
bullets, covering: off by default; what the file contains; Hello/TPM sealing;
DPAPI as the outer layer; survives lock, deleted on log out, expires after 7
days. The existing bullet about the in-memory snapshot not being individually
zeroized stays — it is still true, and it is exactly the kind of caveat this
list should keep.

Neither README should describe the file as "secure". They should describe what
gates it.

## Out of scope

- Making the disk cache the default. It stays opt-in until it has real usage
  behind it.
- Offering a DPAPI-only variant for machines without Hello — see §4; this is
  the product call flagged for the user.
- Configurable expiry.
- Encrypting anything else at rest, or changing how `session.bin` and
  `hello.bin` work.
- Any form of offline write, write queue, or conflict resolution.
- The `Zeroizing`-wrapped `LoginData.password` follow-up. It is the right fix,
  it is already triaged, and it is orthogonal to this.
- 3e's Security preferences section. The toggle lives in General until there
  is enough to fill a second section.

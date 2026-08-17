# Sending a Record for Local Import — Design Note and Open Decision

**Status: DECIDED 2026-08-17.** Both open decisions are resolved (see "The
decisions", below). This is now ready to become a plan.

**Date:** 2026-08-13, decided 2026-08-17
**Related:** `docs/superpowers/plans/2026-08-13-send-a-record.md` (the native
Send work, which does not depend on this)

---

## The problem this solves

Section 5a of the design shows a one-time code travelling in a Send — "rotates,
the recipient sees a live code". That is **not possible** with a Bitwarden Send.
A Send is static ciphertext; the viewer decrypts and displays bytes. Nothing on
the receiving side computes a TOTP.

The only mechanism that gives a recipient a **live** code is for them to hold
the **seed** and compute it locally. That means shipping the record as
structured data and importing it into something that can run the TOTP clock.

## The shape

- Payload: versioned JSON in the Send, carrying only the fields the sender
  ticked in 5a. Absence is meaningful — an unticked field is absent, not empty.
- Transport: **Bitwarden Send, natively.** No new channel, no new server, no
  crypto of our own for the envelope. `bw` already encrypts client-side, the key
  travels in the URL fragment, and the server enforces expiry, view count and
  password.
- Revoke: **natively, by deleting** (decided 2026-08-17). A revoked Send is gone
  from `bw send list`, so there is no Revoked row and none is drawn — the row
  simply leaves the list. See "The decisions" below for why that is the safer
  reading here rather than merely the simpler one.
- Carrier: **a file Send, not a text Send.** We do not create file Sends today,
  but this is the case that wants one: a `.json` attachment makes the browser
  offer a download instead of rendering a seed on screen in a page that can be
  screenshotted or shoulder-read, and it survives copy-paste without mangling.

## What must be said out loud in the UI

**Sending a seed is not sharing a code — it is cloning the second factor,
permanently.** A code dies in thirty seconds. A seed is the factor. Anyone who
opens the Send can generate valid codes indefinitely, as can anyone they forward
the payload to.

**It defeats revoke.** Native revoke is genuine — the server deletes or refuses
the ciphertext, and the fragment key alone is useless. But revocation controls
*future* retrievals only. It cannot retract what was already decrypted. For a
password that is survivable, because you rotate it. For a seed it is not:
"rotating" means re-enrolling the second factor with the service, which
Deskwarden cannot do or offer.

So for any Send carrying a seed, revoking **must not read as "pulled back"**. It
means "no new recipients". Since revoke deletes (decided 2026-08-17) there is no
Revoked row to mislabel — but the confirmation shown *before* revoking must
still say plainly that a seed already fetched stays valid forever, because that
is the moment the user believes they are undoing something.

## Two layers of encryption — when it is real, when it is theatre

Send's password gates *retrieval* server-side; it does not encrypt the payload
with your password. The content is protected by the fragment key, which is **in
the link**. So whoever has the link has the content.

For username and password that is the bargain already accepted by sending them.
For a seed — unrotatable, permanent — "whoever has the link" is too weak, and a
passphrase-derived encryption layer over the seed field is worth its complexity:
it makes the link alone insufficient.

**It only counts if the passphrase travels out-of-band.** Put it in the Send and
you have encrypted a box and taped the key to it. Use a vetted AEAD and a real
KDF; do not hand-roll. Version the format.

## The embedded expiry

An embedded `not_after` in the payload is worth having, but it helps in a
narrower case than "recipient offline for two years" — an offline recipient
cannot fetch the Send at all, so server-side expiry already covers that. It
earns its keep when the payload was **fetched while valid and imported later**,
or forwarded to someone else.

**It is staleness prevention, not enforcement.** It binds our client and the
recipient's clock. Anyone can open the JSON by hand or set the date back. The
copy must say so rather than implying a guarantee.

## Import must treat the payload as data

The JSON is written by someone else and arrives over the network. On import:
strict schema validation, unknown fields rejected rather than ignored, no field
interpreted as an instruction, nothing auto-opened, nothing executed. A `notes`
field is text to store. `Zeroizing` end to end. Prefer "Import from Send" taking
the link over "paste the blob" — the clipboard is exactly the leak the fill
path's password step already refuses to touch.

---

## The decisions

### Decided 2026-08-17: revoke deletes

Revoke runs `bw send delete`, which is what it does today. The consequence,
accepted knowingly: a revoked Send does not come back from `bw send list`, so
the Shared screen cannot show a **Revoked** state — the row leaves the list
instead of changing colour.

The argument for `disabled` was renderability, and that argument is real but
narrow. Against it: a disabled Send is still ciphertext sitting on a server,
and for the payload this spec is about — a TOTP seed — "still there but
switched off" is a worse resting state than "gone". Deleting also cannot be
un-done by a mis-click at the Bitwarden end, which is the failure that would
actually matter.

**What this obliges:** the confirmation before revoking must say the record is
being removed permanently, not paused, and must not offer "Send again" from a
row that no longer exists. Any design element depending on a Revoked row — see
`2026-08-13-send-a-record.md` Task 4 — is cut rather than reinterpreted.

### Decided 2026-08-17: the imported record goes into the vault

**Option A below.** Deskwarden continues to store no vault of its own.

The cost, accepted knowingly: the embedded `not_after` becomes advisory only.
A vault item does not expire, so honouring it would mean Deskwarden remembering
the item and deleting it later — best-effort, and only while Deskwarden is
installed and running. The UI must therefore present `not_after` as *staleness
information about the record*, never as an expiry that will be enforced. Copy
that implies the record will disappear on its own is wrong and must not ship.

**What this obliges:**
- Import writes through `bw` and so requires an unlocked vault; the import
  entry point needs the same locked-state handling every other vault write has.
- A collision policy is now mandatory, because a real item is being created:
  importing the same record twice must not silently produce two items.
- The TOTP seed lands in the item's own `totp` field, so the vault computes the
  code and Deskwarden does not become a TOTP implementation.

---

### The original analysis: where does the imported record live?

### Option A — the recipient's Bitwarden vault, via `bw create item`

- Reuses everything; Deskwarden stays a companion and stores no vault.
- Syncs to their other devices, which is usually what a recipient wants.
- **Permanent and unrevocable by construction.** Bitwarden vault items do not
  expire, so the embedded `not_after` could only be honoured by Deskwarden
  remembering the item and deleting it later — best-effort, and only while
  Deskwarden is installed and running.

### Option B — a Deskwarden-held ephemeral record

- Expiry is natural: the record can genuinely disappear when it lapses, so the
  embedded `not_after` means something.
- **Deskwarden becomes a thing that stores TOTP seeds at rest**, which requires
  its own encryption, its own lock state, and its own threat model. Today it
  stores no vault at all — `bw serve` does. This is a substantial new
  responsibility for a companion app.

### Outcome

**Option A was chosen** (2026-08-17), matching the recommendation: tell the
truth about permanence rather than build a second vault to support a promise
the mechanism cannot keep.

This work is now unblocked and needs a plan written from this spec. The native
Send plan (`2026-08-13-send-a-record.md`) remains independent, and its Task 4
must be revised for the revoke decision above before it is executed.

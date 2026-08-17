# Sending a Record for Local Import — Design Note and Open Decision

**Status: NOT A PLAN.** One decision is unresolved, and a plan with an open
decision in it is a plan that cannot be executed. This records the analysis so
the decision can be made once and cheaply, after which this becomes a plan.

**Date:** 2026-08-13
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
- Revoke: **natively**, and see the hardening plan's Task 1 — revoke should set
  `disabled` rather than delete, so the Revoked state is renderable.
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

So for any Send carrying a seed, the state word **"Revoked" must not read as
"pulled back"**. It means "no new recipients".

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

## THE OPEN DECISION

**Where does the imported record live?**

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

### Recommendation

**Option A**, with the UI telling the truth about permanence, rather than
building a second vault to support a promise the mechanism cannot keep. But the
argument for B gets stronger the more weight the embedded expiry is meant to
carry, so this is genuinely the user's call and not a detail.

**Until it is decided, do not start this work.** The native Send plan
(`2026-08-13-send-a-record.md`) is independent and can proceed.

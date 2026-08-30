# Preferences, Per Backend

**One Preferences window describing one product, whichever of the two the user
is actually running.**

## Why

The owner, having read his own app:

> "so need to split those completely — if user selects bw — we only show bw
> related items, if not bw — our set of settings"

and the rule behind it:

> "users should not know about how it works underhood"

Deskwarden has two vault backends. The official Bitwarden CLI (`bw serve`),
which is the shipped default, and its own built-in client talking to a
self-hosted server directly (`use_official_bw_crypto = false`,
`backend_policy::choose` -> `DirectRest`). Preferences today describes both at
once, and in one night that produced three rows that were ghosted or lying:

1. **"Keep the Bitwarden backend running"**, ghosted on the built-in client
   under a sentence ending *"there is nothing here to decide"*. A row whose
   entire content is that it is not a row.
2. **The Updates page** said *"This build cannot check for updates"* — a
   different bug, since fixed, but it is the same failure of altitude: the page
   was describing machinery instead of an outcome.
3. **The built-in-client row** said *"(the Bitwarden CLI is still used to sign
   in)"*, which stopped being true when the direct-REST path took the CLI out
   of `authenticate_then_wipe`, and nobody noticed until the owner read it in
   the running app. `the_built_in_client_row_does_not_say_the_cli_signs_you_in`
   exists because of that one.

These are three symptoms of one cause: **one page trying to describe two
products.** Every sentence on it has to be true of both, so every sentence
either hedges, ghosts, or eventually goes stale on the side its author was not
thinking about.

## What actually differs — the whole table

This is the heart of the design and it is smaller than the owner's phrasing
suggests. Read off the `Settings` doc comments, which are unusually explicit
about scope:

| Page | Row | Field | Scope |
| --- | --- | --- | --- |
| General | Show autofill prompts | `prompt_on_match` | **both** |
| General | Fill the focused app (read-only chip) | — | **both** |
| General | Show site icons | `fetch_icons` | **both** |
| General | Show card network logos | `use_brand_logos` | **both** |
| General | Show TOTP secrets on the details screen | `reveal_totp_seed` | **both** |
| General | Lock the vault when you step away | `auto_lock_enabled` | **both** |
| General | Lock the vault after | `auto_lock_minutes` | **both** |
| Breaches | Check passwords against known breaches | `check_breaches` | **both** |
| Breaches | Scan the whole vault + history | — | **both** |
| Vault | Use official bw for crypto | `use_official_bw_crypto` | **the switch itself** |
| Vault | Keep the Bitwarden backend running | `keep_backend_running` | **`bw` only** |
| Vault | Open the vault instantly | `keep_ui_loaded` | **both** |
| Vault | Keep an encrypted copy of your vault on this PC | `cache_vault_to_disk` | **both** |
| Vault | Read from that copy first | `read_through_cache` | **both** |
| Local API | Serve this vault to programs on this PC | `service_enabled` | **both** |
| Local API | Mint / list / revoke keys | — | **both** |
| Clipboard | all five rows | `clear_clipboard*` | **both** |
| Updates | Check for updates automatically + the flow | `check_for_updates` | **both** |
| About | Version | — | **both** |
| About | Bitwarden account | — | **both, wrong copy** (see below) |

**Exactly one row is backend-specific.** `keep_backend_running` is a
memory/latency trade about the `bw serve` subprocess;
`backend_policy::should_run` answers `false` for `DirectRest` whatever the
setting says, and its own doc calls the setting *"a trade about a subprocess
this account does not have"*.

Three near-misses, each settled deliberately:

* **`keep_ui_loaded` is backend-independent** and the code already says so in
  as many words — *"this one is about Deskwarden's own window and is true on
  every backend, so it is never ghosted"*. It stays on both. It is the row most
  likely to be swept up by a careless split, because it sits in the same card
  as the one row that goes.
* **`cache_vault_to_disk` and `read_through_cache` apply to both.**
  `backend_policy::read_path`'s own table walks all four combinations and names
  a wanted user for each: `bw serve` × cache-first is *the feature the disk
  cache was built for*. Neither row moves.
* **The local API applies to both.** It answers "who else may ask Deskwarden
  for this vault", which is a question about Deskwarden, not about what is
  behind it.

So "split those completely" cannot mean two product surfaces. **It means one
row hidden and a handful of sentences that name `bw` unconditionally.** That
is the honest size of the work, and stating it is most of the design.

### The sentences, which are the larger half

Four more places name the CLI where the built-in user has none:

* `disk_cache_description(true)`: *"instead of waiting about 8 seconds for the
  Bitwarden backend"*. On the built-in client there is no backend to wait for.
  **This is a fourth live lie and it has not been reported yet.**
* `ACCOUNT_CHECKING_NOTE`: *"Asking the Bitwarden CLI which account is signed
  in."*
* `ACCOUNT_NO_EMAIL_NOTE`: *"The Bitwarden CLI did not report the address."*
* `ACCOUNT_SIGNED_OUT_NOTE`: *"...or the Bitwarden CLI could not be reached to
  ask."*

The last three are About's account row. All three are unconditional and all
three are false on the built-in client, where the status comes from
`rest::api`. They are not ghosted or hedged — they simply assert a program that
is not running.

## Hide, or ghost?

**Both, and the rule that tells them apart is not "which looks tidier".**

`prefs_ui.rs` argues for ghosting, and the argument is right as far as it goes:
*"a ghosted control with no explanation reads as a bug"*, and
`child_toggle_row`'s doc adds *"three rows going grey says 'these are what I
turned off'; three rows disappearing says nothing at all."* The owner is asking
for hidden. The resolution is that these are answers to two different
questions, and the file has only ever asked one of them.

> **Ghost** a row the user *could* have here, blocked by a condition on this
> machine or this account that they can go and change. The ghost text names the
> remedy, and the grey is the promise that the row comes back.
>
> **Hide** a row about machinery that does not exist in the product the user
> chose. There is no remedy to name, so the ghost text has nothing to say but
> a confession about internals — which is precisely what the owner ruled out.

By that rule:

* **`keep_backend_running` is hidden** on the built-in client. Its ghost
  sentence today is *"there is nothing here to decide"* — the row admitting it
  is not a row. There is no remedy: the remedy would be "go back to `bw`",
  which is the switch one row above it and not something this row should be
  arguing for.
* **The backend switch stays ghosted** on bitwarden.com, exactly as it is,
  under *"Only available on a self-hosted server."* That is a remedy the user
  can act on, and hiding it would leave a self-hoster mid-setup unable to
  discover that the option exists at all.
* **`read_through_cache` stays ghosted** under its parent, and
  **`cache_vault_to_disk` stays ghosted** without Windows Hello. Both name
  remedies. Neither is backend-specific.

The result is that the Vault page carries, at most, one ghost and it is always
the same one — and the ghost and the hide sit two rows apart on the same card,
which is the clearest possible statement of the distinction.

**The "lost feature" objection, answered.** A row that vanishes can read as a
feature that was taken away. Three things stop that here, and none of them is
"the user will remember":

1. The row vanishes **in the same frame as the switch that removed it**, one
   row below that switch, immediately after the user pressed *Switch it* on a
   confirmation naming the change.
2. The switch's own OFF copy already says what is gone, without naming the row:
   *"no background process keeps running"*. A user reading that has been told
   there is nothing to keep running before they can miss the control for
   keeping it running.
3. The switch is never hidden, so the row is always one click and one restart
   away, on a page the user is already looking at.

## Where the backend switch lives

**Where it is: the first row of the first card on Vault, on both backends,
never hidden, ghosted only on an account where the choice cannot take effect.**

It is the door between the two sets, so it is the one control the split cannot
touch. It also gets the one exemption from "users should not know about how it
works underhood": **the switch has to name both backends, because naming them
is the choice it is asking the user to make.** A switch that hid what it was
switching between would be unusable. Everything else stops naming either — and
that is what makes the exemption affordable, since it becomes the single place
in the window where the machinery appears at all.

Its copy must stop describing the other side's machinery in passing, which is
where lie #3 came from. It already says the honest thing about signing in;
`the_built_in_client_row_does_not_say_the_cli_signs_you_in` guards it.

## Renaming

* **"Keep the Bitwarden backend running" -> "Keep the Bitwarden CLI running".**
  "backend" is this codebase's word, not a user's; the row is now only ever
  shown to somebody who has the CLI, and its own doc says the point of naming
  `bw` is *"what the user will see in Task Manager"* — so name the thing they
  will see there.
* **"Use official bw for crypto" -> "Use the official Bitwarden CLI".** "bw"
  is a filename and "crypto" is a word about internals; neither is what the
  user is choosing. The description already says "the `bw` program", so the
  label loses nothing it was carrying.
* **Section names stay.** "Vault", "Local API", "General" name outcomes
  already.
* **No new nav row, and no per-backend nav.** Two nav trees would be the "two
  products" made literal: a user flipping the switch would find the window
  rearranged under them, and `every_nav_section_design_3e_lists_is_painted`
  would have to be split into two lists that could disagree.

## An account on bitwarden.com

Nothing changes for them, and that is the point. `backend_policy::choose`
answers `BwServe` for every bitwarden.com account regardless of the setting, so
that user is in the `bw` set: they see the CLI row, live, and the backend
switch ghosted with the sentence that says why. The split does not reach them
because they were never in the other product.

What the split *does* do for them is remove the only remaining way for that
page to be confusing: today the page can, on a slow status lookup, paint a
ghosted switch above a live CLI row above copy about a built-in client they
cannot have. After the split, the unknown-status page and the bitwarden.com
page are the same page, because `is_self_hosted` already treats unknown as
official.

## What this is not

**It is not a second Preferences window, a second nav, or a `Section` per
backend.** One row and four sentences do not justify a second information
architecture, and a second one is a second one to keep in step.

**It is not a live switch.** `use_official_bw_crypto` is captured at startup by
`main`'s `BackendSettlement` and never re-read. The rows shown on this page
follow the *live* value — as `backend_description`'s gate already does, and for
its stated reason: ghosting (now hiding) a restart later than the switch that
caused it would leave the user with a page that disagrees with the click they
just made.

**It is not a change to `backend_policy`.** `choose` is already the single
decision and this page is already its fourth caller. No new host test, no new
enum, no second copy of "which backend".

**It does not touch `should_run`, `read_path`, or any behaviour.** Every field
keeps its meaning and its default. This is a change to what is drawn and what
is said.

## How it will be known to work

**Pure tests on the classification**, in the shape `backend_policy`'s own table
test uses: the four combinations of server and setting, each answering whether
the CLI rows are shown, with the single `DirectRest` row the only `false`.

**Painted tests on both pages**, and the pair matters more than either half:

* the `bw` page paints `BACKEND_LABEL` and four pills;
* the built-in page paints neither the label nor a fourth pill — **and paints
  the other three**, which is the positive control that stops the assertion
  passing on a page that failed to draw.

**A copy test per lie.** One per sentence named above, each asserting the
sentence is gone *and* that the surrounding paragraph is still there, so the
needle cannot pass by no longer matching anything. That is the shape
`the_built_in_client_row_does_not_say_the_cli_signs_you_in` already uses, and
it is the shape that caught lie #3.

**A no-`bw`-anywhere test on the built-in page**: scan every string the
built-in Vault page paints for "bw serve", "Bitwarden CLI" and "backend", with
the backend switch's own row excluded by name — the one exemption, made
explicit in a test rather than left as an understanding.

**The readability measurement, extended.** The fifth combination — self-hosted
with the switch off — is the shortest the page can be and is currently not
driven by `the_whole_vault_page_is_readable_without_scrolling`. It is added, not
because it can overflow but because a page that lost a row is a page whose
layout changed.

**A live check:** flip the switch in the running app, restart, and read the
Vault page. Nothing here proves that the row a user is looking at matches the
backend that actually started.

## Status

Design, approved 2026-08-30. Implementation plan:
`docs/superpowers/plans/2026-08-30-preferences-per-backend.md`.

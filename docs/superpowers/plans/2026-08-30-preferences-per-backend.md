# Preferences Per Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user on Deskwarden's built-in client opens Preferences and reads a
page about their vault. There is no row offering to keep a subprocess they do
not have, no sentence about waiting eight seconds for a backend that does not
exist, and no About page claiming a CLI was asked which account is signed in. A
user on the official Bitwarden CLI sees exactly what they see today, one label
better worded. The switch between the two is the one control that names both,
it is visible on both, and it is the only place in the window where either is
named.

**Architecture:** This implements
`docs/superpowers/specs/2026-08-30-preferences-per-backend-design.md`. The
whole change lives in `deskwarden/src/prefs_ui.rs` and is drawn from one pure
predicate, `cli_rows_are_shown`, which is a thin reading of
`backend_policy::choose` — **not a second decision.** `backend_policy` gains
nothing and changes nothing; `settings.rs` gains nothing and changes nothing.
No field changes meaning, no default moves, no behaviour outside the paint
changes.

The design's governing rule, and every task below follows from it:

> **Ghost** a row the user could have here, blocked by something they can go
> and change; the ghost names the remedy. **Hide** a row about machinery the
> chosen product does not have; there is no remedy to name, and the only thing
> a ghost could say is a confession about internals.

Exactly one row is hidden: `keep_backend_running`. Exactly one row stays
ghosted for a backend reason: the switch itself, on a non-self-hosted account.
They sit two rows apart in the same card, which is deliberate — it is the
clearest available statement of the distinction.

**Tech Stack:** Rust, egui/eframe, `backend_policy::choose`, the existing
`child_toggle_row`/`toggle_row` pair in `prefs_ui.rs`, and the file's own
`Painted` harness (`paint`, `paint_tall`, `tall_frame`, `rects_of_size`,
`pill_fills`, `contains`, `strings`).

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- Build with `RUSTFLAGS="-D warnings"`, on the build **and** on `cargo test --no-run`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory; the disk has ~20 GB free and that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names".
- Judge a failing test by reading it, never by its name prefix. Several guards in this file are *supposed* to fail here; each names the mutation it caught, and the plan says per task which ones and why the change is a re-pin rather than a loosening.

Additionally, and specific to this branch:

- **Do not edit `deskwarden/src/backend_policy.rs` or `deskwarden/src/settings.rs`.** If a task appears to need a change there, stop and report: it means the split has been drawn in the wrong place.
- **No test may touch** the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`.
- **Never `publish_account_status`** in a test. It writes process-wide state that the rest of this parallel suite reads; use `PrefsState::show_account_source` with a `fn` pointer, as the existing Vault tests do.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/prefs_ui.rs` (modify) | Everything. The predicate, the two label renames, the hidden row, the four sentences, and the guards. |

**One file, and that is a finding rather than a convenience.** The design's
table shows exactly one backend-specific row and four backend-specific
sentences. A change that reached further than this file would be a change that
had decided something `backend_policy` had already decided.

## Interfaces

```rust
// deskwarden/src/prefs_ui.rs — new, private, pure.

/// Whether this page shows the rows that are only about the `bw serve`
/// subprocess.
///
/// Reached through `backend_policy::choose`, never re-decided here: the same
/// rule `account_is_self_hosted` follows one function above, and for the same
/// reason. Two host tests on one page is how the switch comes to disagree with
/// the row under it.
fn cli_rows_are_shown(server: Option<&str>, use_official_bw_crypto: bool) -> bool;

/// The server this page's account is on, or `None`.
///
/// One reader for the two call sites in `draw_backend_card`, so a mid-frame
/// status arrival cannot give the switch and the row below it different
/// answers.
fn account_server(status: &Option<AccountStatus>) -> Option<&str>;
```

Changed, in place:

```rust
const BACKEND_LABEL: &str = "Keep the Bitwarden CLI running";      // was "...Bitwarden backend running"
const OFFICIAL_CRYPTO_LABEL: &str = "Use the official Bitwarden CLI"; // was "Use official bw for crypto"

/// No longer a function of a `bool`: the row is hidden where the other arm
/// used to be shown, so there is no second state to describe.
const BACKEND_DESCRIPTION: &str = /* the old `backend_description(true)` text */;

fn disk_cache_description(hello_available: bool, bw: bool) -> &'static str;  // gains the second arm
fn account_checking_note(/* none */) -> &'static str;                        // backend-neutral consts
```

Removed: `fn backend_description(bw_selected: bool)`, and with it the string
ending *"so there is nothing here to decide"*.

---

### Task 1: The predicate, decided away from the frame

**Files:** `deskwarden/src/prefs_ui.rs`

**Interfaces**

- *Consumes:* `crate::backend_policy::{choose, VaultBackendChoice}`, `prefs_ui::AccountStatus`.
- *Produces:* `cli_rows_are_shown`, `account_server`.

Nothing is drawn differently in this task. The predicate exists first so that
the four-combination table is under oath before any row moves — the same
separation `backend_switch` and `official_crypto_description` already keep from
the eframe closure.

- [ ] **Step 1: Write the failing test**

Add to `prefs_ui.rs`'s `tests` module, beside the existing Vault guards:

```rust
    /// **The whole partition, as one table**, in the shape
    /// `backend_policy::the_whole_decision_table` uses: eight combinations,
    /// and the single `DirectRest` row is the only one that hides anything.
    ///
    /// Driven through this page's own predicate rather than through `choose`
    /// directly, because the mutation this guards is a page that re-decides
    /// "which backend" for itself and drifts from the switch above the row.
    #[test]
    fn the_cli_rows_are_shown_for_every_account_except_the_built_in_one() {
        let self_hosted = Some("https://vault.example.com");
        let official = Some("https://vault.bitwarden.com");
        let unknown = Some("");

        // The one arm that hides: positively self-hosted AND opted out.
        assert!(
            !cli_rows_are_shown(self_hosted, false),
            "the built-in client has no `bw serve`, so the rows about it must not be drawn"
        );

        // Every other combination is `bw serve`, so the rows decide something
        // and are drawn. All seven, because a predicate that read only the
        // toggle -- or only the server -- would pass some of them.
        for (server, use_official, why) in [
            (self_hosted, true, "self-hosted, official CLI chosen"),
            (official, true, "bitwarden.com, official CLI chosen"),
            (official, false, "bitwarden.com, opted out -- but it cannot opt out"),
            (unknown, true, "unknown server, official CLI chosen"),
            (unknown, false, "unknown server counts as official"),
            (None, true, "no server URL is bitwarden.com by definition"),
            (None, false, "no server URL is bitwarden.com by definition"),
        ] {
            assert!(
                cli_rows_are_shown(server, use_official),
                "{why}: this account is served by `bw serve`, so the rows about it are real"
            );
        }
    }

    /// **The predicate is `backend_policy`'s answer and not a second one.**
    ///
    /// Without this, `cli_rows_are_shown` could be written as
    /// `is_self_hosted(server) == false || use_official` -- which happens to
    /// agree today and would drift the first time `choose` gained an input.
    #[test]
    fn the_predicate_is_exactly_the_backend_policy_decision() {
        use crate::backend_policy::{choose, VaultBackendChoice};
        for server in [None, Some(""), Some("https://vault.example.com"), Some("https://bitwarden.eu")] {
            for use_official in [true, false] {
                assert_eq!(
                    cli_rows_are_shown(server, use_official),
                    choose(server, use_official) == VaultBackendChoice::BwServe,
                    "the page disagrees with `backend_policy::choose` for \
                     server={server:?} use_official={use_official}"
                );
            }
        }
    }

    /// The server reader, including the two states that are not a signed-in
    /// account. `Checking` is the one that matters: it lasted 2.8 seconds on
    /// the machine this page's account row was reported from, and it must read
    /// as "unknown", which `is_self_hosted` already treats as official.
    #[test]
    fn the_page_reads_the_server_off_the_status_and_nothing_else() {
        assert_eq!(
            account_server(&Some(AccountStatus::SignedIn {
                email: None,
                server: Some("https://vault.example.com".to_string()),
            })),
            Some("https://vault.example.com")
        );
        // `None` for bitwarden.com by definition -- `backend_policy`'s rule,
        // not this page's.
        assert_eq!(
            account_server(&Some(AccountStatus::SignedIn { email: None, server: None })),
            None
        );
        assert_eq!(account_server(&Some(AccountStatus::Checking)), None);
        assert_eq!(account_server(&Some(AccountStatus::SignedOut)), None);
        assert_eq!(account_server(&None), None);
    }
```

Run: `RUSTFLAGS="-D warnings" cargo test -p deskwarden prefs_ui:: --no-run` —
expect a resolution failure for `cli_rows_are_shown` and `account_server`.

- [ ] **Step 2: Make it pass**

Add, directly under `account_is_self_hosted`:

```rust
/// Whether this page shows the rows that are only about the `bw serve`
/// subprocess.
///
/// **Reached through [`crate::backend_policy::choose`], never re-decided
/// here.** That is `account_is_self_hosted`'s rule one function above, and it
/// is load-bearing in the same way: a page with its own idea of "which
/// backend" is a page whose switch and whose rows can disagree by one edit.
///
/// # It reads the CHOSEN backend, not the running one
///
/// `use_official_bw_crypto` is captured once, by `main`'s
/// `BackendSettlement`, so the click does not take effect until the next
/// launch. This still follows the *live* value, which is what
/// `backend_description` did for its gate and for the same reason: a row that
/// disappeared a restart after the switch that removed it would leave the user
/// looking at a page that disagrees with the click they just made.
fn cli_rows_are_shown(server: Option<&str>, use_official_bw_crypto: bool) -> bool {
    matches!(
        crate::backend_policy::choose(server, use_official_bw_crypto),
        crate::backend_policy::VaultBackendChoice::BwServe
    )
}

/// The server this page's account is on, or `None` for everything that is not
/// a signed-in account with one.
///
/// `None` is bitwarden.com **by definition** and not "not known yet" --
/// `backend_policy::is_self_hosted` says so in as many words -- so the
/// `Checking` and `SignedOut` arms landing here is the safe direction and not
/// an oversight: unknown counts as official, and the `bw` rows are the ones a
/// user already had.
fn account_server(status: &Option<AccountStatus>) -> Option<&str> {
    match status {
        Some(AccountStatus::SignedIn { server, .. }) => server.as_deref(),
        _ => None,
    }
}
```

Run the three tests. All green, nothing else touched.

---

### Task 2: The row is hidden, and the sentence that admitted it was not a row is deleted

**Files:** `deskwarden/src/prefs_ui.rs`

**Interfaces**

- *Consumes:* `cli_rows_are_shown`, `account_server` (Task 1).
- *Produces:* `BACKEND_DESCRIPTION`; `draw_backend_card` drawing three rows or four.
- *Removes:* `fn backend_description(bw_selected: bool)`.

**Guards expected to move, and why each is a re-pin:**

| Guard | What happens | Why it is not a loosening |
| --- | --- | --- |
| `the_backend_row_goes_quiet_when_there_is_no_backend_to_keep_running` (~8907) | Rewritten. It asserts `backend_description(false)` is *painted* on the self-hosted/opted-out page; that copy no longer exists. | The mutation it caught is "a row that governs nothing is offered as live". The rewrite asserts the same mutation through **absence of `BACKEND_LABEL`**, keeps all three positive combinations verbatim, and *adds* a positive control (the other rows are still painted) that the original did not have. Strictly stronger. |
| `the_ghosted_backend_copy_names_the_switch_that_disabled_it` (~8939) | **Deleted**, and replaced in the same step by `the_built_in_vault_page_names_no_subprocess` (Task 5). | This is the only outright deletion in the plan. Its subject — the ghost sentence — is the thing the design removes, so keeping it would pin the defect. Deleting it without the replacement *would* be a loosening; the replacement asserts the stronger property (no `bw` machinery is named anywhere on that page except the switch's own row) and must land in the same commit. |
| `every_setting_that_decides_where_the_vault_comes_from_is_on_the_vault_page` (~8757) | Re-pinned, not rewritten. It paints via `paint_vault_with_hello(true)`, whose account source is the default — no status, so `bw` is chosen and the row is still drawn. It passes untouched; the doc comment gains a sentence saying **which** of the two pages it is speaking about. | A test that names one product must say so, or the next reader takes it as a claim about both. |
| `clicking_the_disk_cache_toggle_changes_the_setting_it_is_wired_to` (~6761), `clicking_the_toggle_changes_the_setting_it_is_wired_to` (~7098), `the_backend_row_is_disabled_and_inert_on_an_official_cloud` (~9284), `the_backend_row_toggles_on_a_self_hosted_server...` (~9315), `the_backend_row_is_on_when_bw_is_the_backend` (~9357) | Expected to pass untouched. Each indexes `rects_of_size(TOGGLE_SIZE)` on a page where `bw` is chosen, so no pill is removed under them. | If any of these fails, the row was hidden on the wrong arm. Read the failure; do not re-index. |

- [ ] **Step 1: Write the failing test**

```rust
    /// **The defect this task removes, asserted from the other side.**
    ///
    /// This test used to say the row went *grey* on a built-in-client account,
    /// under a sentence ending "there is nothing here to decide" -- a row whose
    /// only content was that it did not apply. It now says the row is not
    /// there, and the three combinations where it IS a real decision are
    /// unchanged from the version this replaces.
    #[test]
    fn the_row_about_the_subprocess_is_absent_where_there_is_no_subprocess() {
        let direct = paint_vault_for(Some("self"), false);
        assert!(
            !direct.contains(BACKEND_LABEL),
            "a self-hosted account with the built-in client is still offered a row about a \
             subprocess it does not have; got {:?}",
            direct.strings()
        );
        // The positive control, and it is the whole reason this is not a
        // vacuous pass: the page is not blank. The two rows that are true on
        // every backend are still on it.
        assert!(
            direct.contains(UI_LOADED_LABEL) && direct.contains(DISK_CACHE_LABEL),
            "the built-in page painted nothing, so the absence above is about a failed paint \
             and not about the row; got {:?}",
            direct.strings()
        );
        // And the row's own copy is gone with it -- a hidden row whose
        // paragraph was still painted somewhere would be the same lie with no
        // label on it.
        assert!(
            !direct.contains(BACKEND_DESCRIPTION),
            "the row is hidden but its description is still on the page"
        );

        // Every other combination is `bw serve`, so the row decides something
        // and is drawn with the copy that says what the trade is. All three,
        // because a gate that read only the toggle -- or only the server --
        // would pass one of them.
        for (server, use_official, why) in [
            (Some("self"), true, "self-hosted, official CLI chosen"),
            (None, false, "bitwarden.com, opted out -- but it cannot opt out"),
            (None, true, "bitwarden.com, official CLI chosen"),
        ] {
            let painted = paint_vault_for(server, use_official);
            assert!(
                painted.contains(BACKEND_LABEL) && painted.contains(BACKEND_DESCRIPTION),
                "{why} is served by `bw serve`, so this row decides something and must be \
                 drawn saying so; got {:?}",
                painted.strings()
            );
        }
    }

    /// **The count, which is the half a `contains` loop is structurally blind
    /// to.** Four pills on the `bw` page and three on the built-in one, and
    /// both numbers spelled out rather than derived, so a row added to either
    /// page has to be re-pinned here deliberately.
    #[test]
    fn the_vault_page_paints_one_fewer_pill_on_the_built_in_client() {
        assert_eq!(
            paint_vault_for(Some("self"), true).count_of_size(TOGGLE_SIZE),
            4,
            "the `bw` Vault page's four pills: the backend switch, `keep_backend_running`, \
             `keep_ui_loaded`, `cache_vault_to_disk` -- `read_through_cache`'s pill is the \
             fifth and is only reachable with the disk copy on"
        );
        assert_eq!(
            paint_vault_for(Some("self"), false).count_of_size(TOGGLE_SIZE),
            3,
            "the built-in Vault page must lose exactly one pill -- `keep_backend_running` -- \
             and keep the other three: the switch back, `keep_ui_loaded`, and the disk copy"
        );
    }
```

Note `paint_vault_for` already exists (~8867) and takes exactly these two
inputs. `Settings::default()` has `cache_vault_to_disk` off, so
`read_through_cache`'s pill is not painted in either case; that is stated in the
message rather than left to be rediscovered.

Run: expect `the_row_about_the_subprocess_is_absent...` to fail on the first
assertion (the row is currently ghosted, not hidden) and the count test to fail
at 4 vs 3.

- [ ] **Step 2: Make it pass**

Replace `fn backend_description` with a constant, carrying the surviving arm's
text unchanged:

```rust
/// The description under [`BACKEND_LABEL`].
///
/// **No longer a function of a `bool`, and that is the change.** It had a
/// second arm that read "there is nothing here to decide" -- a row whose only
/// content was that it did not apply. `draw_backend_card` now omits the row
/// entirely on the backend that has no subprocess, so there is no second state
/// left to describe; see
/// `docs/superpowers/specs/2026-08-30-preferences-per-backend-design.md` for
/// the hide-versus-ghost rule and why this row falls on the hide side of it.
const BACKEND_DESCRIPTION: &str =
    "Faster, and uses about 110 MB while idle. Off runs it only while the vault window is \
     open; autofill is unaffected either way.";
```

Then in `draw_backend_card`, after the switch and its confirmation row, replace
the `child_toggle_row` call:

```rust
        let server = account_server(&status);
        // **Hidden, not ghosted, and the two rows above show why the
        // distinction is not a preference.** The switch above is ghosted on an
        // account that cannot use the built-in client, because that is a
        // remedy the user can act on -- change the server -- and grey is the
        // promise that the row comes back. This row has no remedy to offer: on
        // the built-in client there is no subprocess, and the only sentence a
        // ghost could carry is a confession about how the app is built.
        if cli_rows_are_shown(server, state.settings.use_official_bw_crypto) {
            row_separator(ui);
            state.settings.keep_backend_running =
                toggle_row(ui, BACKEND_LABEL, BACKEND_DESCRIPTION, state.settings.keep_backend_running);
        }
        row_separator(ui);
        // **Not gated, and never was.** This is about Deskwarden's own window
        // and is true on every backend -- see `Settings::keep_ui_loaded`. It
        // is the row most likely to be swept up by a careless split, because
        // it shares a card with the one row that goes.
        state.settings.keep_ui_loaded = toggle_row(
            ui,
            UI_LOADED_LABEL,
            UI_LOADED_DESCRIPTION,
            state.settings.keep_ui_loaded,
        );
```

The `let bw_selected = matches!(crate::backend_policy::choose(...))` block and
the `let server = match &status { ... }` block it sat on are both replaced by
the two lines above; delete them rather than leaving a second reader of the
same fact.

- [ ] **Step 3: Move the guards named in the table**

Rewrite `the_backend_row_goes_quiet_when_there_is_no_backend_to_keep_running`
into the test written in Step 1 (same position in the file, so the diff reads
as a rewrite). Delete
`the_ghosted_backend_copy_names_the_switch_that_disabled_it` and leave this
comment where it stood, in the idiom this file already uses for a removed
guard:

```rust
    // **`the_ghosted_backend_copy_names_the_switch_that_disabled_it` is gone
    // with the sentence it guarded**, and this note is here instead of a
    // weakened version of it. It pinned that the ghosted row said "the switch
    // above" so a user who found it grey had somewhere to go. The row is not
    // ghosted any more -- it is absent on the backend that has no subprocess,
    // because the only thing a ghost could have said there was that the row
    // did not apply. What replaces it is
    // `the_built_in_vault_page_names_no_subprocess`, which is a stronger claim
    // about the same page: no row on it names `bw serve` at all, the switch's
    // own row excepted by name.
```

Add the sentence to
`every_setting_that_decides_where_the_vault_comes_from_is_on_the_vault_page`'s
doc: *"This is the `bw` page — `paint_vault_with_hello` publishes no account
status, so `is_self_hosted` answers false and `choose` answers `BwServe`. On
the built-in client `BACKEND_LABEL` is deliberately absent; see
`the_row_about_the_subprocess_is_absent_where_there_is_no_subprocess`."*

Run the whole `prefs_ui` suite. Read every failure before touching it.

---

### Task 3: The two renames

**Files:** `deskwarden/src/prefs_ui.rs`

**Interfaces**

- *Produces:* `BACKEND_LABEL = "Keep the Bitwarden CLI running"`, `OFFICIAL_CRYPTO_LABEL = "Use the official Bitwarden CLI"`.

**Guards expected to move:** `the_ui_loaded_row_names_both_halves_of_the_trade`
(~5742) asserts `UI_LOADED_LABEL` contains "open" — untouched.
`the_backend_row_is_on_when_bw_is_the_backend` (~9357) reads the *description*,
not the label — untouched. No existing guard pins either label's text, which is
itself worth fixing: a rename with no guard is a rename that can be undone
silently.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The two labels name a program the user can find, not a concept from
    /// this codebase.**
    ///
    /// "backend" is this file's word for `bw serve`; "bw" is a filename;
    /// "crypto" is a word about internals. The rows are read by somebody
    /// deciding where their vault comes from, and the design's rule is that
    /// this switch is the ONE place in the window where the machinery is
    /// named -- so it had better name it the way the user will meet it.
    #[test]
    fn the_two_backend_labels_name_the_cli_and_not_this_codebases_words() {
        for label in [BACKEND_LABEL, OFFICIAL_CRYPTO_LABEL] {
            assert!(
                label.contains("Bitwarden CLI"),
                "{label:?} does not name the program the user will see in Task Manager"
            );
            assert!(
                !label.to_lowercase().contains("crypto"),
                "{label:?} names an internal concept the user is not choosing between"
            );
            assert!(
                !label.to_lowercase().contains("backend"),
                "{label:?} uses this file's own word for `bw serve`"
            );
        }
        // The control: the two are still different rows, and the one about
        // keeping it running still says so. Without this, both could collapse
        // to the same string and pass every assertion above.
        assert_ne!(BACKEND_LABEL, OFFICIAL_CRYPTO_LABEL);
        assert!(BACKEND_LABEL.contains("running"), "the row no longer says what it decides");
    }
```

- [ ] **Step 2: Make it pass**

```rust
/// The `bw serve` lifetime row's label.
///
/// **Names the CLI, because that is what a user will find in Task Manager**,
/// which its own reason for naming `bw` at all has always been. It said
/// "Bitwarden backend" while the row was drawn on both backends and had to
/// cover a case where the thing running was not `bw`. It is only ever drawn to
/// somebody who has the CLI now, so it can say so.
const BACKEND_LABEL: &str = "Keep the Bitwarden CLI running";
```

and

```rust
const OFFICIAL_CRYPTO_LABEL: &str = "Use the official Bitwarden CLI";
```

leaving the existing multi-paragraph doc comment on `OFFICIAL_CRYPTO_LABEL`
intact — the "on means `bw`" inversion argument is unaffected by the wording —
with one sentence appended: *"It said 'Use official bw for crypto': 'bw' is a
filename and 'crypto' is a word about internals, and neither is what the user
is choosing between."*

---

### Task 4: The four sentences that name a CLI the built-in user does not have

**Files:** `deskwarden/src/prefs_ui.rs`

**Interfaces**

- *Produces:* `disk_cache_description(hello_available: bool, bw: bool)`; backend-neutral `ACCOUNT_CHECKING_NOTE`, `ACCOUNT_NO_EMAIL_NOTE`, `ACCOUNT_SIGNED_OUT_NOTE`.

**Guards expected to move:**

| Guard | What happens | Why it is a re-pin |
| --- | --- | --- |
| `the_available_disk_cache_copy_states_the_survives_a_lock_behaviour` (~6683) | Call site gains the second argument. Its needles are about what is in the file and the lock behaviour, which are in both arms. | A signature change, not a claim change. |
| `the_unavailable_disk_cache_copy_explains_why_and_offers_no_weaker_option` (~6701) | Same, and the Hello arm must be identical for both values of `bw` — asserted. | The Hello refusal has nothing to do with the backend; making that explicit is new coverage. |
| `the_vault_page_says_what_is_in_the_file_when_hello_is_available` (~6743), `the_vault_page_draws_the_disk_cache_row_with_the_reason_when_hello_is_missing` (~6717) | Call sites gain the argument, at `bw = true` (their `paint_vault_with_hello` state chooses `bw`). | Same page, same claim. |
| `the_whole_vault_page_is_readable_without_scrolling` (~8970) | Call site gains the argument, **and a fifth combination is added**: self-hosted with the switch off, the shortest the page can be. | Strictly more coverage. A page that lost a row is a page whose layout changed, and the premise assertion inside the loop is what stops the new combination passing on an empty paint. |
| `checking_and_signed_out_say_different_things` (~9955), `a_signed_in_account_with_no_address_says_so` (~10001), `the_account_row_always_says_something` (~9974) | Expected to pass untouched — they assert the notes differ and are non-empty, not that they name `bw`. | If one fails, read it: it means a note was emptied rather than reworded. |

- [ ] **Step 1: Write the failing test**

```rust
    /// **The fourth live lie, and it had not been reported.**
    ///
    /// `disk_cache_description` sold the encrypted copy as saving the user
    /// "about 8 seconds" of waiting for the Bitwarden backend. On the built-in
    /// client there is no backend to wait for -- and the copy is still true
    /// about everything else, which is exactly how a sentence like this
    /// survives: only one clause of it is wrong.
    #[test]
    fn the_disk_cache_copy_does_not_promise_a_wait_that_does_not_happen() {
        let built_in = disk_cache_description(true, false);
        assert!(
            !built_in.contains("Bitwarden backend"),
            "the built-in client's disk-cache copy names a subprocess that does not exist: \
             {built_in}"
        );
        // The control: the `bw` arm DOES name it, so the assertion above is
        // about the arm and not about a function that returns one string.
        assert!(
            disk_cache_description(true, true).contains("Bitwarden backend"),
            "the `bw` arm stopped naming what the wait is for, so the test above is vacuous"
        );
        // And both arms still carry the four properties this copy exists for.
        for copy in [disk_cache_description(true, true), built_in] {
            assert!(
                copy.contains("usernames, passwords, notes and two-factor secrets"),
                "the copy names what is in the file with a euphemism: {copy}"
            );
            assert!(copy.contains("not deleted when your vault locks"), "got {copy}");
            assert!(copy.contains("TPM"), "got {copy}");
            assert!(!copy.contains("secure"), "the copy uses the word it has always refused");
        }
        // The Hello refusal is not about the backend at all, and says the same
        // thing either way.
        assert_eq!(
            disk_cache_description(false, true),
            disk_cache_description(false, false),
            "the Windows Hello refusal was made to depend on which backend is running"
        );
    }

    /// **About's account row names the Bitwarden CLI three times,
    /// unconditionally, on a page a built-in-client user reads too.**
    ///
    /// Not ghosted and not hedged: it asserts a program was asked. These are
    /// the three sentences, and the fix is to name what the app did rather
    /// than what it did it with -- which is the altitude the whole design is
    /// about.
    #[test]
    fn the_account_row_says_what_happened_and_not_which_program_did_it() {
        for note in [ACCOUNT_CHECKING_NOTE, ACCOUNT_NO_EMAIL_NOTE, ACCOUNT_SIGNED_OUT_NOTE] {
            assert!(
                !note.contains("Bitwarden CLI"),
                "{note:?} names a program that is not running on the built-in client"
            );
            // The control: each note still says something. A note emptied to
            // pass the assertion above would leave the row reading as a field
            // that failed to load, which is the defect `ACCOUNT_STATUS` was
            // written for.
            assert!(note.len() > 20, "{note:?} was emptied rather than reworded");
        }
        // And they are still three different sentences -- "we have not asked
        // yet" and "nobody is signed in" are opposite facts.
        assert_ne!(ACCOUNT_CHECKING_NOTE, ACCOUNT_SIGNED_OUT_NOTE);
        assert_ne!(ACCOUNT_NO_EMAIL_NOTE, ACCOUNT_SIGNED_OUT_NOTE);
    }
```

- [ ] **Step 2: Make it pass**

`disk_cache_description` gains a second parameter and splits its first
paragraph only:

```rust
/// The description shown under the disk-cache toggle.
///
/// Two inputs, and they are different questions. `hello_available` asks
/// whether the setting can be offered at all; `bw` asks which product's
/// first paragraph to write. **Only the first paragraph differs**: what is in
/// the file, what encrypts it, and who can read it are the same on both
/// backends, and duplicating them would be two copies to keep in step on the
/// one row where a stale sentence is a false security claim.
fn disk_cache_description(hello_available: bool, bw: bool) -> &'static str {
    if !hello_available {
        // Unchanged, and deliberately not a function of `bw`: there is no
        // Hello-less variant of this refusal, because the TPM binding is the
        // whole value of the setting on either backend.
        return "Unavailable — needs Windows Hello.\n\n\
                This copy is protected by a key held in your PC's TPM chip, which only Windows \
                Hello can release. Without Hello there is no such key, and Deskwarden will not \
                store your vault on disk under weaker protection than this setting describes. \
                Set Hello up in Windows Settings → Accounts → Sign-in options.";
    }
    if bw {
        "Deskwarden opens instantly after a restart and autofill works the moment it starts, \
         instead of waiting about 8 seconds for the Bitwarden backend.\n\n\
         The copy contains your usernames, passwords, notes and two-factor secrets. It is \
         encrypted with a key that Windows Hello keeps in this PC's TPM chip, so a copied disk \
         cannot be read on another machine. It is not deleted when your vault locks — only when \
         you log out, or after 7 days. Anyone who can run programs as you on this PC and pass \
         Windows Hello can read it."
    } else {
        // **The eight seconds are gone, because they were `bw serve`'s.**
        // The built-in client has no subprocess to start, so what this buys
        // here is a vault that is on screen before the server has answered --
        // which is the honest benefit and is a different sentence.
        "Deskwarden opens with your vault already on screen after a restart, and autofill \
         works before your server has answered.\n\n\
         The copy contains your usernames, passwords, notes and two-factor secrets. It is \
         encrypted with a key that Windows Hello keeps in this PC's TPM chip, so a copied disk \
         cannot be read on another machine. It is not deleted when your vault locks — only when \
         you log out, or after 7 days. Anyone who can run programs as you on this PC and pass \
         Windows Hello can read it."
    }
}
```

`draw_disk_cache_card` passes the second argument. It has no `status` of its
own today, so read one — through the same `(state.account_source)()` seam, and
say why:

```rust
fn draw_disk_cache_card(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        let hello_available = (state.hello_available)();
        // The same seam the card above reads, for the paragraph that differs
        // between the two products. Read once here rather than threaded down
        // from `vault_cards`: the two cards ask it for different reasons, and
        // a shared parameter would imply a shared rule they do not have.
        let bw = cli_rows_are_shown(
            account_server(&(state.account_source)()),
            state.settings.use_official_bw_crypto,
        );
        state.settings.cache_vault_to_disk = child_toggle_row(
            ui,
            DISK_CACHE_LABEL,
            disk_cache_description(hello_available, bw),
            state.settings.cache_vault_to_disk,
            hello_available,
        );
        // ...unchanged read-through row...
```

The three account notes:

```rust
const ACCOUNT_CHECKING_NOTE: &str = "Asking which account is signed in.";
const ACCOUNT_NO_EMAIL_NOTE: &str = "The address for this account was not reported.";
const ACCOUNT_SIGNED_OUT_NOTE: &str =
    "No account is signed in, or Deskwarden could not reach the vault to ask.";
```

`ACCOUNT_SIGNED_OUT_NOTE`'s doc comment keeps its argument — one sentence for
two situations this build cannot tell apart — and gains: *"It named the
Bitwarden CLI, which is only one of the two things that can fail to answer."*

- [ ] **Step 3: Move the guards named in the table**

Update the four `disk_cache_description` call sites in tests to pass `true` as
the second argument (all four paint a `bw` page). In
`the_whole_vault_page_is_readable_without_scrolling`, change the inner loop's
`disk_cache_description(hello)` to `disk_cache_description(hello, self_hosted_is_bw)`
where the page's own choice is computed the same way the page computes it, and
add the fifth combination by extending `paint_vault_copy` with a
`use_official: bool` and driving `(hello, self_hosted, use_official)` over the
combinations that are reachable — noting in the doc comment that
`(self_hosted = false, use_official = false)` is the same page as
`(self_hosted = false, use_official = true)`, because bitwarden.com cannot opt
out, and is driven anyway as the control that says so.

---

### Task 5: The replacement guard, and the one exemption written down

**Files:** `deskwarden/src/prefs_ui.rs`

**Interfaces**

- *Produces:* `the_built_in_vault_page_names_no_subprocess`.

This is the test that makes Task 2's deletion safe, and it is the design's
"users should not know about how it works underhood" made checkable. It must
land in the same commit as that deletion.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The rule the owner asked for, as a scan rather than as an
    /// understanding: on the built-in client, nothing on the Vault page names
    /// the machinery.**
    ///
    /// One exemption, and it is named here rather than left implicit: the
    /// backend switch itself. It has to name both backends, because naming
    /// them is the choice it is asking the user to make -- a switch that hid
    /// what it was switching between would be unusable. That is the whole of
    /// the exemption, and confining it to one row is what makes it affordable.
    #[test]
    fn the_built_in_vault_page_names_no_subprocess() {
        let painted = paint_vault_for(Some("self"), false);
        let switch_copy = official_crypto_description(true);
        let offending: Vec<&str> = painted
            .strings()
            .into_iter()
            // The exemption, matched on the switch's own two strings rather
            // than on a substring, so a new row cannot claim it by accident.
            .filter(|s| *s != OFFICIAL_CRYPTO_LABEL && *s != switch_copy.as_str())
            .filter(|s| {
                s.contains("bw serve") || s.contains("Bitwarden CLI") || s.contains("backend")
            })
            .collect();
        assert!(
            offending.is_empty(),
            "the built-in client's Vault page names machinery it does not have, outside the \
             one row that is allowed to: {offending:?}"
        );

        // **Two controls, and both are needed.**
        //
        // The page really painted something...
        assert!(
            painted.contains(DISK_CACHE_LABEL),
            "the scan above found nothing because the page drew nothing"
        );
        // ...and the exempt row really is on it, so the filter is excusing a
        // string that is actually there rather than one that never was.
        assert!(
            painted.contains(OFFICIAL_CRYPTO_LABEL),
            "the exempted switch is not on the page, so the exemption is excusing nothing"
        );
        // ...and the needles find something on the OTHER page, so they are
        // needles that can match.
        let bw_page = paint_vault_for(Some("self"), true);
        assert!(
            bw_page.strings().iter().any(|s| s.contains("Bitwarden CLI")),
            "the `bw` page does not name the CLI either, so this scan cannot tell the two \
             pages apart"
        );
    }
```

Note `official_crypto_description` returns `&'static str`; the `.as_str()` is
written for the case where it is bound as a `String` in a later refactor — use
whichever compiles without a warning, and do not add an `allow`.

- [ ] **Step 2: Make it pass**

It should pass once Tasks 2, 3 and 4 have landed. **If it does not, read what
it found before changing the filter.** A string it catches is either a row that
should have been hidden, a sentence that should have been reworded, or a
genuine second exemption that has to be argued for in the design first — never
one to add to the filter list.

- [ ] **Step 3: Verify the whole file, then commit**

```
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo test -p deskwarden --no-run
RUSTFLAGS="-D warnings" cargo test -p deskwarden prefs_ui::
RUSTFLAGS="-D warnings" cargo test -p deskwarden backend_policy::
RUSTFLAGS="-D warnings" cargo build -p deskwarden
```

`backend_policy::` is run explicitly and must be **untouched**: this plan
changes no decision, so a failure there means the split was drawn in the wrong
place.

Commit `deskwarden/src/prefs_ui.rs` and the two documents by explicit path.

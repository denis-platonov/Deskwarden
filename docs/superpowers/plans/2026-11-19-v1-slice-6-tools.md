# Slice 6: Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The password Generator as its own screen (with a PIN mode and remembered defaults), a Trash that says when things were deleted and can be emptied, and an Old tab in Password health.

**Architecture:** A `Screen::Generator` beside Sends and Health in the sidebar's TOOLS; the screen is a new `vault_window/generator_screen.rs` drawn in place of the list and detail. PIN generation is a new `password_gen::generate_pin`. Defaults persist as new serde-defaulted `Settings` fields. Trash gains a header (count, Empty trash) and per-row "Deleted N days ago" from `deletedDate`. Health gains `HealthFilter::Old`.

**Tech Stack:** Rust, egui 0.35, the crate's `kit`.

**Spec:** `docs/design/deskwarden-v2-design-spec.md` Part C §1 (generator), §3 (trash), §4 (health); Part A §0.2 D5.

## Global Constraints

- Bitwarden-compatible: nothing new is stored on items; generator defaults are local settings (`settings.json`), serde-defaulted so older files load.
- CRLF in edited files; `crlf.py` after scripted edits.
- `cargo test --lib --bins`, `cargo check --all-targets` 0 warnings, `cargo deny` green at every commit.
- Spec copy verbatim.

## Review Focus

1. **Randomness:** the PIN and every generated value come from the OS RNG that `password_gen` already uses, never a seeded or time-based one; a PIN is digits only and exactly the asked length.
2. **Settings compatibility:** a settings file without the new fields loads with the spec's defaults; a file with out-of-range values is clamped, not rejected.
3. **Empty trash is destructive:** it asks first, says how many items, and a failure part-way leaves the list honest.
4. **Old** never counts an item whose password date cannot be known as old.
5. **The generated value is a secret:** zeroized when replaced, copied through the secret clipboard path.

## Decisions

1. Generator ranges and defaults from the spec: Password 8–64 (20), Passphrase 3–10 words (5), PIN 4–12 (6). Strength is the spec's bits model (PIN len·3.32, passphrase words·12.9, password len·log2(pool); 40/70/100).
2. The editor's generator modal stays (it fills a password into a draft); the screen is the standalone tool. Both read the persisted defaults.
3. D5 "Old": 365 days since the password last changed, read as `passwordRevisionDate`, else the newest `passwordHistory` date, else `creationDate` — the read pane's own age rule (detail.rs explains why `revisionDate` moves on any edit). An item with none of these is not old.
4. Empty trash purges each trashed item through the existing purge path after one confirmation; Breached stays as a fourth Health tab beside the spec's three.

---

### Task 1: PIN generation and the strength model
- `password_gen::generate_pin(len: usize) -> Zeroizing<String>` (OS RNG, digits, 4..=12 clamp); `generator_strength(kind, len_or_words, pool) -> (bits, label)` labels Weak/Fair/Strong/Very strong at 40/70/100.
- Tests: `a_pin_is_digits_of_the_asked_length`, `pin_length_is_clamped`, `strength_follows_the_spec_bits`.

### Task 2: Persisted generator defaults
- `Settings` gains `generator_mode`, `generator_length`, `generator_words`, `generator_pin_length`, `generator_classes`, `generator_avoid_lookalikes` (serde defaults = spec defaults, clamped on load).
- Tests: `an_old_settings_file_gets_the_spec_defaults`, `out_of_range_values_are_clamped`.

### Task 3: The Generator screen
- `generator_screen.rs`: 760 wide; value row (mono, Copy, Regenerate), segmented Password / Passphrase / PIN, length slider, class tiles, look-alike switch, strength line; changes persist. Sidebar TOOLS gains Generator (`Screen::Generator`); the palette gains "Generate a password" (Ctrl+G opens the screen).
- Tests: `the_screen_offers_three_modes`, `pin_mode_shows_only_digits`, `changing_an_option_persists_it`, `the_sidebar_and_palette_open_the_generator`.

### Task 4: Trash
- Trash header: "Trash", count, "Items stay here for 30 days", Empty trash (confirm → purge all); rows' subtitle "Deleted N days ago"; empty state "Trash is empty".
- Tests: `trashed_rows_say_when_they_were_deleted`, `empty_trash_asks_then_purges_every_item`, `an_empty_trash_says_so`.

### Task 5: Old passwords in Health
- `HealthFilter::Old`, `old: Vec<OldItem>` in the report, counted in findings; tab label "Old"; row detail "Not changed in N days".
- Tests: `a_password_unchanged_for_a_year_is_old`, `an_item_with_no_password_date_is_not_old`, `old_counts_in_the_findings`.

### Task 6: Previews and verification
- Surfaces: generator (each mode), trash, health Old.

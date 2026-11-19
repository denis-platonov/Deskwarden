# Slice 5: Modals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The v2 modal frame, the New item chooser, the Ctrl+K command palette, the v2 Share/Send and Send detail modals, and the v2 Add 2FA modal.

**Architecture:** A new `kit::modal` draws the v2 frame (scrim that closes on click, radius-24 dialog, 38pt close X, Esc closes) and answers `ModalOutcome`. New modals (chooser, palette, Send detail) are built on it; the existing record Send composer and the totp_add modal are restyled onto it, keeping their logic. One new `Option` state per new modal in `vault_window/mod.rs`, each scrim id added to `item_list::MODAL_SCRIM_AREAS`.

**Tech Stack:** Rust, egui 0.35, the crate's `kit` module.

**Spec:** `docs/design/deskwarden-v2-design-spec.md` Part B §2, §4–§8 (decisions D3 in Part A §0.2); `docs/superpowers/specs/2026-11-18-deskwarden-1-0-redesign-design.md` slice 5.

## Global Constraints

- 100% Bitwarden-compatible: Sends are Bitwarden Sends (`bw send create` path), "Open only once" = max access count 1, "Link works for" = deletion date; no recipient field (server rejects email-only Sends); no NodeWarden-only calls.
- CRLF line endings in every edited file; `scratchpad/crlf.py` after scripted edits.
- `cargo test --lib --bins`, `cargo check --all-targets` with 0 warnings, `cargo deny` green at every commit.
- Every new scrim id is listed in `item_list::MODAL_SCRIM_AREAS`.
- Copy verbatim from the spec (titles, subs, toasts).

## Review Focus

1. **Esc and scrim close only the top modal**, never the editor beneath it or a second modal; a modal never closes while a Send is being created.
2. **The palette's keyboard:** Up/Down/Enter choose a row; Enter with nothing typed does nothing harmful; typing never triggers the window's own chords (Ctrl+N etc. still work only outside text).
3. **Secrets in the Send:** the password chip is what decides whether the password travels; the link and the composed text are zeroized like 0.15's.
4. **A locked or signed-out window** opens no palette and no modal.
5. **Narrow window (930):** every dialog fits with a 24pt gutter.

## Decisions

1. Ctrl+K opens the palette (it focused search in 0.15; search keeps its click and `/`... no new chord: Ctrl+F focuses search). Ctrl+N stays "new login" directly, as the spec's palette hint says; the sidebar's New item and the list's "+ New" open the chooser.
2. The chooser shows every creatable kind (`CREATABLE_KINDS`): the spec's three first in its words, the rest (identity, SSH key) after them in the same card style.
3. The Send modal is the record Send composer restyled (D3): chips Username / Password / 2FA code / Notes map to the record's fields; Link works for 1 hour / 1 day / 7 days / 30 days; Open only once switch; no recipient field. Done shows the link in the link bar with Copy.
4. Send detail is a modal opened from a Sends row; it reads `SendRow` and offers Copy link / Add 1 day / Revoke via the existing `SendUiAction`s. Activity is built from what Bitwarden keeps (created, access count, last change, expiry, revoked).
5. Add 2FA keeps 0.15's routes; the picker becomes the two v2 cards (Scan from screen, Type the key) with webcam and image file as two quiet links under them; found and manual take the v2 look.

---

### Task 1: `kit::modal`, the frame
- Create `src/kit/modal.rs`: `pub fn dialog(ctx, id: &str, width: f32, title: &str, sub: Option<&str>, add: impl FnOnce(&mut Ui)) -> ModalOutcome` (`Open | Closed`), scrim `rgba(20,26,48,.38)` that closes on click, card radius 24, padding 30, title 26/800, sub 16 `#7d7979`, close X 38×38 r12 at 18/18, Esc closes. Width clamped to the window less 48.
- Tests: `the_scrim_closes_the_dialog`, `esc_closes_the_dialog`, `the_close_mark_closes_the_dialog`, `a_click_in_the_card_keeps_it_open`, `a_narrow_window_keeps_a_24pt_gutter`.
- Commit "The v2 modal frame".

### Task 2: New item chooser
- `vault_window/new_item_modal.rs`: 660 wide, "What are you adding?", 3-col grid of cards (radius 18, border 2, tile 52 r15), copy per spec §5; answers `Option<ItemKind>`. Sidebar New item and the list's "+ New" open it; picking one opens `DetailMode::Create(EditDraft::empty_of(kind))`.
- Tests: `the_chooser_offers_login_card_and_note_first`, `picking_a_kind_opens_its_editor`, `the_sidebar_new_item_opens_the_chooser` (source guard).
- Commit "The New item chooser".

### Task 3: Command palette
- `vault_window/palette.rs`: 640 wide, top-aligned 90, search row with "Search or jump to…" and Esc chip, Items (max 5, name + user contains query) and Actions (New login, Generate a password, Share {item}, Import passwords, Open settings, Lock now, filtered by label), "Nothing found"; Up/Down/Enter. Answers `PaletteAction`. Ctrl+K opens it (decision 1); Ctrl+F focuses search.
- Tests: `items_match_name_and_user_and_stop_at_five`, `actions_filter_by_label`, `arrows_and_enter_choose_a_row`, `nothing_found_when_nothing_matches`, `ctrl_k_opens_the_palette` (source guard).
- Commit "The Ctrl+K command palette".

### Task 4: Share / Send modal
- Restyle the record Send composer onto `kit::modal` (560): chips with the password chip's red tones, the expiry segmented control, Open only once switch, Create link (toast "Pick at least one thing to share" when none), Done state with the 76 check, "Link ready", summary, link bar + Copy ("Link copied"), Done / See in Shared.
- Tests: `no_field_picked_asks_for_one`, `the_password_chip_is_red_when_picked`, `the_plan_maps_expiry_and_once`, `done_shows_the_link_and_copies_it`.
- Commit "The v2 Share modal".

### Task 5: Send detail modal
- `send_ui` row click opens a 600 modal: header avatar + name + recipient + status pill, facts grid (Opens / Link / Includes), Activity, the opened-password warning, Copy link / Add 1 day / Revoke via existing actions.
- Tests: `the_facts_read_the_send`, `revoke_and_extend_use_the_sends_actions`, `the_warning_shows_for_an_opened_password_send`.
- Commit "The Send detail modal".

### Task 6: Add 2FA modal
- totp_add onto `kit::modal` (580): "Add 2FA code", "For {name}", the two cards, scan panel caption "Looking for a code…", Found it card with live code, manual field border/preview; toast "2FA code added".
- Tests: `the_picker_offers_scan_and_type`, `a_valid_key_lights_the_field_and_previews_the_code`, `saving_toasts_2fa_code_added`.
- Commit "The v2 Add 2FA modal".

### Task 7: Previews and verification
- `ui_preview` surfaces for each modal; full verification.
- Commit "Previews of the v2 modals".

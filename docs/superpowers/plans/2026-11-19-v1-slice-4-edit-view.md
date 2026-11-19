# Slice 4: the Edit view — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (the owner chose Native) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The item editor becomes spec part B §1: a full-page edit view with a header bar (back, avatar with its image menu, kind label, click-to-edit title, Cancel, Save), a two-column body of v2 cards (Details, 2FA code, Fills in, Sign-in steps, Notes | Custom fields, Password history, About, Move to trash).

**Architecture:** `detail_edit::draw_detail_edit` keeps owning the draft, validation and save, so every write still goes through the code that is Bitwarden-compatible today. The window gives the editor the whole area under the top bar while `DetailMode::Edit`/`Create` is up (the list column hides), and the editor draws v2 cards out of `kit` widgets plus one new module, `kit::input`, for the design's text fields. The avatar's colour choice is stored in the item's own `deskwarden:icon` custom field: a standard Bitwarden custom field, so other clients keep and ignore it.

**Tech Stack:** Rust, egui 0.35, the `kit` module (slices 1–3).

**Spec:** `docs/design/deskwarden-v2-design-spec.md` part B §0.6 and §1 (with A §3.2 for avatar logic). The roadmap is `docs/superpowers/specs/2026-11-18-deskwarden-1-0-redesign-design.md`.

## Global Constraints

- 100% Bitwarden compatible: nothing is stored that another Bitwarden client would lose or misread. New state lives in custom fields named `deskwarden:*`.
- Every save, discard, generate, app-pick and sequence edit keeps its 0.15 behaviour and its tests. The visual rewrite does not change what is written.
- Card fields keep Bitwarden's model. There is no billing address, as in slice 3.
- CRLF line endings, `cargo check --all-targets` with 0 warnings, `cargo deny`, and `cargo test --lib --bins` green at every commit.

## Review Focus

1. **Unsaved edits are never lost silently.** The spec's Back/Esc close "without confirmation". 0.15 asks before discarding a changed draft, and that confirmation stays (decision 1).
2. **Ctrl+S, Esc and F2 inside a focused text field.** Esc in the title input ends title editing and does not close the editor. Ctrl+S saves from any field.
3. **The minimum window (930 → an editor about 680 wide).** The two columns stack below 760, and nothing runs past the edge.
4. **Secrets.** The password and CVV inputs stay password-mode until revealed. Sign-in step Test output never contains the real password, only `••••••••`.
5. **Card number input.** Pasting `4111-1111 1111 1111` strips to digits, caps at 19 and groups in fours, and the brand badge follows the digits.

## Decisions

1. **Discard confirmation stays.** Back/Cancel/Esc with a changed draft still asks (`draw_discard_confirm`). With an unchanged draft they close at once, as the spec says.
2. **Folder** is the segmented control when the vault has three or fewer assignable folders. Otherwise it is a v2-styled dropdown: the spec's three-folder control cannot hold a real vault.
3. **Strength meter** uses the app's existing `password_strength` rating, mapped to the spec's four labels and colours (Weak, Fair, Strong, Very strong). It does not re-derive a second estimator.
4. **Sign-in steps** map onto the existing `key_sequence` tokens: user `{username}`, pw `{password}`, totp `{totp}`, tab `{tab}`, stab `{shift+tab}`, enter `{enter}`, wait `{wait:0.5}`. The presets are those three sequences. The 0.15 template editor stays reachable from the card's ⋯ for anything the chips cannot express (literals, other waits).
5. **Test** is a pure simulation into the output pane, 620 ms per step. It never types anywhere.
6. **Avatar menu.** "Upload image" uses 0.15's existing icon override ("Select icon…"). "Use the app's icon" uses the matched app's extracted icon. "Colour" writes `deskwarden:icon` = `colour:<bg>/<fg>`. "Remove image" clears the override and the colour. The list and detail avatars read the colour.

---

### Task 1: The full-page editor and its header bar

**Files:** `vault_window/mod.rs` (hide the list column while editing; Ctrl+S; Esc routing), `vault_window/detail_edit.rs` (header bar), `kit/icon.rs` (CAMERA, UPLOAD glyphs).

- [ ] Tests first:
  - `editing_takes_the_whole_area_under_the_top_bar`: the list's `+` (`item_list::new_button_id`) is not drawn while the editor is up.
  - `the_header_names_the_kind_and_the_title_edits_in_place` ("New login" / "Editing card"; click title → input; Enter ends).
  - `save_with_no_title_asks_for_one`: a toast "Give it a name first", no save.
  - `esc_in_the_title_ends_title_editing_only`.
  - `ctrl_s_saves_from_any_field`.
  - `back_with_a_changed_draft_asks_first` and `back_with_no_change_closes`.
- [ ] Header per B §1.1: padding 18/32, bottom rule, back 44 r13, avatar 64 r18 with a 28 camera badge, kind 13/600, title 28/800 with the pencil chip, Cancel (outline 48), Save (primary 48), hints with chords.
- [ ] Commit "The editor takes the window: its header bar".

### Task 2: `kit::input` and the Details card

**Files:** `kit/input.rs` (new), `kit/mod.rs`, `detail_edit.rs`.

- [ ] `kit::input::text(ui, id, value: &mut String, placeholder, mono) -> Response`: h 50, r 14, 1 px `border`, blue when focused, 16 px. `kit::input::password(ui, id, value, revealed: &mut bool) -> PasswordResponse { generate }`: h 52, mono 17, eye 40, Generate tinted 40.
- [ ] Tests:
  - `a_text_field_is_fifty_high_and_turns_blue_when_focused`
  - `a_password_field_hides_until_revealed_and_offers_generate`
  - `the_strength_meter_names_and_colours_the_four_levels`
  - `the_folder_is_a_segmented_control_for_three_folders_or_fewer`
  - `typing_a_card_number_groups_it_and_shows_its_network` (`4111-1111 1111 1111` → `4111 1111 1111 1111`, VISA badge; 20 digits cap at 19)
  - `the_linked_bank_login_is_a_row_of_chips`
- [ ] Details card per B §1.4 A, on a v2 edit card (r 18, padding 22/24, title 18/800).
- [ ] Commit "The Details card in the v2 editor".

### Task 3: Two columns; 2FA, Fills in and Notes cards

**Files:** `detail_edit.rs`.

- [ ] Body grid: `minmax(0,1fr) 320`, gap 20; stacked below 760 wide.
- [ ] 2FA card: code + 34 ring + Replace / Remove ("2FA code removed"), or Add 2FA code (tinted 50) → the add-a-code card.
- [ ] Fills in: chips with ✕, "Add an app or website" + Add, Pick a window → 0.15's app picker.
- [ ] Notes: a 96-high textarea, r 14.
- [ ] Tests:
  - `the_columns_stack_on_a_narrow_editor`
  - `the_2fa_card_offers_replace_and_remove_or_add`
  - `fills_in_chips_remove_and_add`
  - `pick_a_window_opens_the_app_picker`
  - `notes_are_a_textarea_with_the_designs_placeholder`
- [ ] Commit "The editor's 2FA, Fills in and Notes cards".

### Task 4: Sign-in steps

**Files:** `detail_edit.rs` (or a new `vault_window/sign_in_steps.rs` if the card exceeds ~400 lines).

- [ ] Presets Standard / Password first / With 2FA (decision 4); step chips (secret step in the red tones, live step lifted), arrows between them, ✕ per chip, a dashed + with the seven-step menu, the "Only type into …" switch, and the Test box (decision 5) with "Fake data appears here" and the toast "Test finished — every step worked".
- [ ] Tests:
  - `a_preset_replaces_the_steps_and_lights_its_segment`
  - `a_custom_sequence_lights_no_preset`
  - `adding_and_removing_steps_edits_the_sequence`
  - `test_plays_fake_data_one_step_every_620ms_and_never_the_password`
  - `the_password_step_wears_the_secret_colours`
- [ ] Commit "Sign-in steps as chips, with presets and a test".

### Task 5: The right column

**Files:** `detail_edit.rs`.

- [ ] Custom fields (rows with label/value, "+ Add field" dashed 44, which opens 0.15's field editor), Password history (mono masked + date), About (Created, Last changed, Filled; hidden for a new item), Move to trash (danger outline 50, hidden for a new item; it is 0.15's Delete).
- [ ] Tests:
  - `the_right_column_holds_fields_history_about_and_trash`
  - `a_new_item_has_no_about_and_no_trash`
  - `move_to_trash_is_the_items_delete`
- [ ] Commit "The editor's right column".

### Task 6: The avatar menu and colours

**Files:** `detail_edit.rs`, `vault_bridge.rs` (`deskwarden:icon` read/write), `item_list.rs` (`row_look` honours the colour), `detail.rs` (header avatar).

- [ ] Menu per B §1.2: Upload image, Use the app's icon, "Colour" with 8 swatches (selected ring), Remove image. Decision 6 says how each is stored.
- [ ] Tests:
  - `choosing_a_colour_stores_it_in_the_icon_field`
  - `the_list_and_detail_wear_the_chosen_colour`
  - `remove_image_clears_the_override_and_the_colour`
  - `use_the_apps_icon_needs_an_app_first` (toast "Add an app under Fills in first")
- [ ] Commit "The avatar menu: image, app icon and colour".

### Task 7: Previews and verification

- [ ] `ui_preview --edit` shots: new login, editing card, avatar menu open, sign-in steps mid-test, the narrow editor.
- [ ] `cargo test --lib --bins`; `check --all-targets` with 0 warnings; `deny`; `ui_preview --all`.
- [ ] Commit "Previews of the v2 editor".

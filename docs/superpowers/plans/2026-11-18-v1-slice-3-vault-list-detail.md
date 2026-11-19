# Slice 3: Vault list and detail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the item list and the detail (read) pane to design 1a, keeping every 0.15 behaviour.
- **List:** header, rows, tags, selection, empty states, offline banner.
- **Detail:** header, actions, field cards, 2FA ring, Fills in, card hero, and every 0.15 section restyled.
- **Sends screen:** rows and pills in v2 tokens.

**Architecture:** The models stay as they are: `draw_item_list`, `draw_detail_read`, `ItemListAction`, `DetailAction`, `RevealState`, `TotpState`, the icon pipeline and the slide. Only their painting moves onto `kit`.
- A new `kit::avatar` paints item avatars in three looks: a brand tile, a favicon on white, or initials on colour. Both panes use it.
- A new `kit::field` paints the design's field rows: single, pair, eye and copy.

Tests that pin v1 geometry are rewritten to v2 numbers or retired with a ledger ruling, as in slice 2.

**Tech Stack:** Rust, egui 0.35, kit.

**Spec:** `docs/design/deskwarden-v2-design-spec.md`, part A section 3 (list 3.1, avatar logic 3.2, detail 3.3) and section 4 (Shared). Section 0 rules apply.

## Global Constraints

Same as slices 1 and 2:
- `v1` branch, CRLF, edit via scripts or Edit, ledger rulings.
- Every 0.15 behaviour kept, including:
  - drag-to-folder, the right-click row menu, keyboard navigation, virtualisation;
  - search filtering, move-error band, placeholders;
  - every `DetailAction`;
  - reveal reset on selection, copy chords, clipboard clearing;
  - TOTP secret row, history, custom fields, sharing, notes, identity, SSH;
  - metadata/breach footer, view-only records, out-of-vault (trash/archive) read view, matched-app card with Open/Remove.
- Card network and bank artwork stays with `card_mark`'s text/brand-marks path until slice 13 bundles logos.

## Decisions (ledgered when executed)

1. **The list header keeps a compact `+` button** (36 px, the `+ New` menu: creatable kinds and Import from Send) until slice 5's New item chooser. Without it, Identity and SSH key creation and the Send import would be unreachable.
2. **The search placeholder becomes the design's "Search your vault".** The list header now carries the scope's title and count. The Sends matrix's scope witness moves to the header count.
3. **Fill** (logins only) shows the design's toast, "Click the sign-in field in <first app or item name>". The overlay already appears when that field gets focus. 0.15 removed its in-pane fill button and this does not resurrect it.
4. **Share** is `DetailAction::SendRecord`. 0.15's other header controls move into a `⋯` button after the star (48 px square), whose menu holds:
   - "Share with a person…" (`ShareRecord`, when `share_control` is present);
   - "Add 2FA code" (`AddTotp`);
   - Edit, Clone, Move to folder, Delete, and the view-only entries.
5. **Detail sections** keep 0.15's order. Each is a v2 card titled with the field-label style (13/600 `text_label`) instead of v1's tracked capitals:
   - credentials, 2FA, TOTP secret;
   - card hero and card fields; identity; SSH;
   - previous passwords; Fills in (websites as web chips and the matched app as an app chip, with 0.15's Open and Remove kept on the app chip's menu);
   - custom fields; sharing; notes; footer.
6. **Copies use `kit::toast`** with the design's wording (spec 2.6): "Username copied", "Password copied", "2FA code copied" and so on. The detail pane's own `CopyToast` is retired, along with its chord hint chips on rows; the v2 hover hints carry the chords.
7. **The Shared view's card grid moves to slice 5**, with the Send detail modal it opens. This slice restyles the Sends screen's rows and state pills (Waiting / Opened / Expired / Revoked colours from spec section 4) only.

---

### Task 1: `kit::avatar` and the field primitives

**Files:**
- Create `deskwarden/src/kit/avatar.rs` and `deskwarden/src/kit/field.rs`.
- Modify `deskwarden/src/kit/icon.rs` to add the design's glyphs:
  - `COPY` (`M9 9h12v12H9zM5 15V5a2 2 0 0 1 2-2h10`)
  - `EYE`, `EYE_OFF`
  - `CLOSE` (`M18 6 6 18M6 6l12 12`)
  - `FILL` (`M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3`)
  - `EDIT` (`M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z`)
  - `SCAN`, `WEB`, `CHEVRON_RIGHT` (`M9 18l6-6-6-6`)
  - `MORE` (three dots as tiny circles: `M5 12h.01M12 12h.01M19 12h.01`)
  - `WIFI_OFF`
  - `FOLDER` (already there)
  - `CONTACTLESS` (`M8.5 8.5a5 5 0 0 1 0 7M12 6a8.5 8.5 0 0 1 0 12M15.5 3.5a12 12 0 0 1 0 17`)

**Interfaces:**
- `avatar::Look { Initials { text: String, bg: Color32, fg: Color32 }, Favicon(TextureHandle), Brand { label: String, bg: Color32, italic: bool } }`.
- `avatar::paint(painter, rect: Rect, look: &Look, radius: u8)`. Initials are 15/700 at 44 px and 24/800 at 72 px, scaled by the rect's height. A favicon is drawn at 22/44 of the tile (34/72 in detail) on white with an inset 1 px `#e6e3df` ring. A brand tile is the label in white, 13/900 (19/900 at 72 px), with letter-spacing .02em, italic for Visa.
- `avatar::initials_colours(name: &str) -> (Color32, Color32)`: a deterministic pick, by an FNV hash of the name, from the design's avatar palette (spec 1.1 "Avatar palette", 9 pairs).
- `avatar::network(brand: CardBrand) -> (label, bg, italic)`, from spec 3.2's table, for the 10 `CardBrand`s the app knows. Mir is slice 13.
- `field::single(ui, label, value: &str, tracking_em: f32, secret: Option<bool /*revealed*/>) -> FieldResponse { copy: bool, reveal: bool }`:
  - row padding 16/20; label 13/600 `text_label`; value 18 Regular, ellipsised;
  - eye button 42 when `secret` is Some, then a copy button 42, both radius 12;
  - eye hover `hover`; copy hover `blue_wash` with a `blue` glyph.
- `field::pair(ui, a: Cell, b: Cell) -> (FieldResponse, FieldResponse)`: two equal columns, 1 px `row_rule` between them, buttons 38 at radius 11, cell padding 16/12/16/20.

- [ ] Tests:
  - `initials_tiles_paint_their_colours_and_text`
  - `a_favicon_tile_is_white_with_the_ring`
  - `a_brand_tile_is_the_networks_colour_with_its_label` (Visa italic)
  - `initials_colours_are_stable_per_name_and_from_the_palette`
  - `a_single_field_paints_label_value_and_its_two_buttons` (42×42, radius 12)
  - `clicking_copy_and_eye_reports_them`
  - `a_long_value_ellipsises_before_the_buttons`
  - `a_pair_splits_the_row_in_two_with_a_rule_between`
  - `every_new_glyph_parses_inside_the_viewbox` (extends the existing test)
- [ ] Red, implement, green; commit "Avatars and field rows in the v2 shape".

### Task 2: The list pane

**Files:** `deskwarden/src/vault_window/item_list.rs` (painting, constants, header, placeholders, tests), `mod.rs` (top bar placeholder, matrix witness), `send_ui.rs` (where it shares the row constants).

**Spec 3.1:**
- **Pane:** `list_bg` fill with a 1 px `hairline` right border.
- **Header:** padding 24/24/14; title 26/800/-0.02em; count 15 `text_ghost`. The title reads:
  - "Results" while searching;
  - the folder name for a folder;
  - otherwise "All items", "Favorites", "Logins", "Cards", "Secure notes", and the 0.15 extras' row labels.

  The compact `+` (Decision 1) sits at the header's right.
- **Rows:**
  - Padding 12, radius 14, gap 14, pitch = row height + 4.
  - 44 px `kit::avatar` (favicon or initials; for cards, the network brand tile).
  - Title 16/700 `ink`. Subtitle 14 `text_faint`: the username, or `•••• 4821` for cards.
  - Then the tags, via `kit::card::tag` List size: App, 2FA, Shared. view-only becomes a neutral tag reading "View only".
  - Selected: white with a 2 px `blue` ring and the shadow (0,2) blur 8 `rgba(32,30,29,.08)`. Hover: `hover`.
- **Entrance:** rows enter with `motion::up` .4 s, staggered 0.035 s per row. Visible rows only, and only on the first frame of a new list (filter or search change).
- **Empty states:** "Nothing matches" / "Try a site, an app or a username" for no matches. Loading and Empty keep their copy in v2 type.
- **Offline banner:** shown when the window's vault came from the disk cache and the last sync failed. It reads "You're offline" / "Filling still works" with Retry, which triggers sync and the toast "Still can’t reach Bitwarden". Spec copy and colours.
- The top bar's placeholder becomes `SEARCH_PLACEHOLDER` (Decision 2).

- [ ] Rewrite or retire `row_tile_tests`, `toolbar_strip_tests` and `list_placeholder_paint_tests` against v2. Keep and adapt:
  - row pitch / virtualisation;
  - drag;
  - the menu;
  - keyboard tests;
  - move-error band;
  - `the_new_menu_does_not_change_the_rows_below_it` (now the `+`).
- [ ] New tests:
  - `the_header_names_the_scope_and_counts_it`
  - `searching_retitles_the_header_results`
  - `rows_are_44_avatar_16_title_14_subtitle`
  - `the_selected_row_is_white_with_a_blue_ring`
  - `card_rows_wear_their_networks_tile_and_masked_last_four`
  - `tags_follow_app_2fa_shared_order`
  - `nothing_matches_says_so_in_the_designs_words`
  - `the_offline_banner_offers_retry`
  - `rows_enter_with_a_stagger_once_per_list_and_then_go_quiet` (repaint stops)
- [ ] Commit "The list in v2: its header, rows, tags and empty states".

### Task 3: The detail header and actions

**Files:** `deskwarden/src/vault_window/detail.rs` (header strip, `draw_controls`, constants, tests); `mod.rs` maps the new action wiring (`Fill` toast).

**Spec 3.3 header:**
- Pane `window_bg`; padding 28/32; gap 18.
- 72 px avatar at radius 20. Title 32/800/-0.03em, line height 1.
- Kind label 15 `text_faint`; folder chip (`tag_neutral_bg`, 13/600, folder glyph 13; click selects the folder filter, a new `DetailAction::ShowFolder(String)`); tags 12/700.
- Close 42 (`CLOSE`, `text_icon`) with hint `Close|Esc`; Esc closes when no field has focus.
- Action row, 48 high:
  - **Fill** (logins): primary with the `FILL` glyph, toast per Decision 3. New `DetailAction::Fill`, which mod.rs turns into the toast.
  - **Share** (outline, `SHARED` glyph): `SendRecord`.
  - **Edit** (outline, `EDIT` glyph): `Edit`.
  - **Favourite** (48 square, star in `star` or `text_label` outline): `ToggleFavorite`.
  - **`⋯`** (48 square): the menu per Decision 4.
- Hints: `Fill into the app|Mod+Shift+F`, `Share|Mod+Shift+S`, `Edit|Mod+E`, `Favourite|Mod+D`.
- View-only records: no Fill, Share or Edit; ⋯ holds the view-only entries.
- The out-of-vault (trash/archive) view keeps its own actions (Restore, Unarchive, Delete forever), drawn as v2 buttons in the action row.

- [ ] Rewrite the header-strip tests (`the_header_strip_lays_its_avatar_and_title_out_on_the_designs_grid`, `header_controls*`, `the_header_tile_is_the_list_row_tile`) to v2.
- [ ] New tests:
  - `the_header_is_72_avatar_32_title_kind_folder_chip_tags`
  - `the_folder_chip_shows_its_folder`
  - `fill_appears_for_logins_only_and_asks_for_the_toast`
  - `share_edit_favourite_report_their_actions`
  - `the_more_menu_holds_share_with_a_person_add_2fa_clone_move_delete`
  - `a_view_only_record_offers_no_edit_or_fill`
  - `the_trash_view_offers_restore_and_delete_forever`
  - `esc_closes_the_pane_unless_a_field_is_focused`
- [ ] Commit "The detail header: a 72pt avatar, Fill, Share, Edit, the star and more".

### Task 4: The detail body

**Files:** `detail.rs` (sections, rows, TOTP, card, tests), `mod.rs` (copy toasts via `kit::toast`).

- **Credentials:** `kit::field::single`.
  - Username: never masked.
  - Password: masked as 14 `•` at .12em; revealed at .02em.
  - Hints: `Copy username|Mod+B`, `Copy password|Mod+C` (the existing `COPY_SHORTCUTS` chords stay the real chords; the hint shows what each chord actually is).
- **2FA** (spec 3.3):
  - Label "2FA code" with the `?` tip `totp`.
  - Code `NNN NNN` (digit grouping by the code's length) at 26/700/.12em, tabular.
  - 40 px ring: track `badge_bg`, progress `blue`, turning `error` at ≤5 s, `dashoffset` from `seconds_left / period`, seconds 12/700 in the centre.
  - Copy 42 with hint `Copy 2FA code|Mod+T`.
  - Fetching, Unavailable and NoCode keep their words.
  - A login without a code shows the **Add 2FA row**: scan tile, "Add 2FA code" / "Scan the QR code from your screen", chevron. It raises `AddTotp`.
- **TOTP secret row:** a single secret field.
- **Card:** the card hero (440×258, radius 22).
  - Gradient from the network colour to `#201e1d`; the two circles; the chip; the number masked `•••• •••• •••• 4821` with its eye; HOLDER / EXPIRES / CODE.
  - The network label box. The bank/linked-login pill is slice 13.
  - Then the field card: Name on card; Card number (secret); a pair Expires + Security code; a pair Billing ZIP + Billing address, from the item's identity-less card fields, with `—` when empty.
- **Identity, SSH, previous passwords, custom fields, notes, sharing:** v2 cards of `field::single`/`pair` rows with their existing actions. The footer metadata keeps its text in `text_faint` 13 on the pane background, with no box.
- **Fills in card:**
  - Label "Fills in" with the `?` tip `fills`.
  - Chips 38 high: web chips for websites (click opens the site: `OpenWebsite`) and an app chip for the matched app.
  - The app chip's menu offers Open (`OpenApp`) and Remove (`RemoveAppMatch`), with 0.15's trigger-mode text as the menu's first line.
- **Copies:** every copy reports through `kit::toast::show(ctx, "<Label> copied")` with spec 2.6's wording. `CopyToast` / `draw_copy_toast` are retired.

- [ ] Rewrite or retire the painted-geometry tests in detail.rs that pin v1 card/row layout. Behaviour tests stay green:
  - reveal, chords, history, custom fields, sharing actions, breach footer;
  - `read_pane_scroll_tests`.
- [ ] New tests:
  - `credentials_are_field_rows_with_eye_and_copy`
  - `the_password_masks_as_fourteen_dots`
  - `the_2fa_row_groups_the_code_and_drains_its_ring`
  - `the_code_turns_red_in_its_last_five_seconds`
  - `a_login_without_a_code_offers_add_2fa`
  - `a_card_wears_its_hero_and_pairs_expiry_with_code`
  - `fills_in_lists_websites_and_the_matched_app_as_chips`
  - `copying_a_field_shows_the_designs_toast`
  - `notes_custom_fields_history_and_sharing_are_still_shown`
- [ ] Commit "The detail body: field cards, the 2FA ring, Fills in and the card hero".

### Task 5: The Sends screen in v2 tokens

**Files:** `send_ui.rs`.

- Rows reuse the v2 row look (`kit::avatar`, 16/14 type, selection ring).
- State pills use spec section 4's colours:

  | State | Fill | Text |
  |---|---|---|
  | Waiting | `blue_wash` | `blue_deep` |
  | Opened / Used | `done_wash` | `done_ink` |
  | Expired | `hover` | `text_faint` |
  | Revoked | `danger_wash` | `error` |

- The detail cards use `kit::card::card`; buttons use `kit::controls::button`.
- The "Copy link" toast becomes "Link copied"; Revoke's becomes "Link revoked".

- [ ] Adapt send_ui's `paint_tests` to v2 numbers.
- [ ] New test: `state_pills_wear_the_designs_colours`.
- [ ] Commit "The Sends screen in the v2 look".

### Task 6: Previews and verification

- Update `--shell`, `detail_*` and `vault_item_list*` previews.
- Add `vault_shell_card` (a card selected) and `vault_shell_offline`.
- Compare with artboard 1a (vault view, card selected, offline).
- `cargo test --lib --bins`, `check --all-targets` with 0 warnings, `deny`, and `--all` must all pass.
- Commit "Previews of the v2 list and detail".

## Review Focus

1. **Virtualisation with the taller v2 rows.** Arrow keys scroll the selected row into view, and a 2,000-item vault stays fast with no per-row allocation. Task 2 keeps `scroll_offset_for_row` tests at the new pitch.
2. **Secrets never leak through the new painting.** A masked password, CVV or card number must never be laid out in clear text, even in a hidden galley. Task 4 adds `a_masked_secret_is_never_laid_out_in_clear` over the painted text.
3. **Real TOTP periods:** 60 s and 8-digit codes group and drain correctly. Task 4 adds `an_eight_digit_sixty_second_code_groups_and_drains`.
4. **Narrow window (930 wide → detail 298).** The header actions and the card hero must fit or wrap without overlapping. Task 3/4 add `the_detail_fits_its_narrowest_width`.
5. **Trash / archive / view-only items** keep exactly their 0.15 actions. Task 3 tests these.

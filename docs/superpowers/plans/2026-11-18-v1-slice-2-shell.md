# Slice 2: Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The vault window wears the v2 shell:
- a full-height 252 px sidebar, Navy or Light;
- a 72 px top bar in the main column, holding the search, the sync pill, the avatar and the caption buttons;
- a splash while the vault opens;
- toasts and hover hints hosted by the window.

The list and detail panes keep their 0.15 look until slice 3.

**Architecture:** `sidebar.rs` keeps its model (`SidebarFilter`, `SidebarAction`, counts, drag-to-folder) and gets a new drawing built on `kit`. A new `vault_window/top_bar.rs` replaces the vault window's use of `login_ui::draw_window_chrome_with_extra`; the sign-in window keeps that chrome. A new `vault_window/splash.rs` draws the launch splash. `mod.rs` re-orders its panels so the sidebar takes the window's full height, as design 1a draws it. The search field moves from the list header into the top bar, with the same `String` state and the same `"vault-search"` id.

**Tech Stack:** Rust, egui/eframe 0.35, the `kit` module from slice 1.

**Spec:** `docs/design/deskwarden-v2-design-spec.md`, part A §2 (shell, sidebar §2.2, top bar §2.3, splash §2.4, toasts §2.6, hints §2.7). Section 0's rules also apply: nothing dropped, Bitwarden-compatible.

## Global Constraints

- Branch `v1`; commit per task; commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `.rs` files are CRLF. Normalise with `scratchpad/crlf.py` after every write; never use a Bash heredoc that carries backslashes.
- **Nothing the 0.15 shell does is dropped.** Every sidebar row, the folder rename, drag-to-folder, the organisations section, Password health, Sent / Shared with me, the auto-lock countdown, the account menu (switch, Lock, Export vault…, Add account…, Remove this account…), sync and its failure states, window drag, resize, maximise and close all keep working.
- Tests that pin v1 shell geometry are rewritten to the v2 numbers when the invariant they guard still holds (no overlap, a full-width click target, section order). They are deleted only when the thing they pin no longer exists, for example the titlebar gear. Every deletion is a ledger `Ruling:` that names the test.
- Copy is the spec's, verbatim. UI dashes are `\u{2014}`.
- Verification for the slice: `cargo test --lib --bins`, `cargo check --all-targets` (0 warnings), `cargo deny check`, `ui_preview --all`, and the new shell PNGs compared side by side with artboard 1a.

## Decisions this plan makes (each recorded as a Ruling when executed)

1. **Window size.** The default becomes 1280×820 (design 1a); `WINDOW_SIZE` in mod.rs. The minimum becomes **930×600**: 252 sidebar + 380 list + 298 detail. That keeps the 298 px narrow-detail budget every existing narrow-pane test is built on. Saved geometry smaller than this is clamped up by the existing `clamp_window_geometry`.
2. **Sidebar rows, in order:**
   - **VAULT:** All items, Favorites, Apps, Logins, Passkeys, Cards, Identities, Secure notes, SSH keys, Archive, Trash.
   - **TOOLS:** Shared (the Sent screen), Password health, Settings (opens the existing preferences window).
   - **FOLDERS:** each folder, No Folder, Shared with me, then the New folder row.
   - **ORGANISATIONS:** unchanged, after FOLDERS.
   - **Pinned at the bottom:** Lock.

   Generator joins TOOLS in slice 6, when it has a screen to open; a row that does nothing is not drawn.
3. **The auto-lock countdown** moves onto the Lock row as its right-hand count ("11:42", or "Off" from `AUTO_LOCK_OFF_LABEL`), in the nav count style.
4. **New folder** becomes the design's inline input: Enter creates the folder with the typed name, Esc or blur cancels, and the toast reads "Folder created". The folder pencil stays but is shown only on hover, so rename is not lost.
5. **Sync pill copy.** The design has three states; the app has more, and the extra ones carry real information.

   | App state | Label | Tone |
   |---|---|---|
   | sync running | `Syncing…` | green |
   | last sync succeeded | `Synced` | green |
   | loaded from cache / server unreachable | `Offline` | amber |
   | sync failed, or vault not refreshed | `Sync failed` / `Vault not refreshed` | amber |
   | never synced | `Sync` | neutral |

   The hint is `Sync now|Mod+Shift+Y`. Ctrl+Shift+Y is a new chord that does what a click does. The age ("2 min ago") moves into the hint label: "Sync now \u{2014} synced 2 min ago".
6. **The titlebar gear goes.** Settings is a sidebar row now. The avatar keeps the account menu.
7. **Ctrl+K** still focuses the search; the command palette arrives in slice 5. The chip reads `Ctrl K`.
8. **New item** does exactly what Ctrl+N does today; the design's chooser arrives in slice 5.
9. **Splash.** It replaces the vault body's `Loading` state and the first window's `Loading` body. `Slow` and `Unreachable` keep their 0.15 bodies (restyled in slice 10). The splash fades out over .45 s when the vault becomes ready. There is no minimum display time: a fast vault must not be held behind an animation.

---

### Task 1: Kit additions the shell needs

**Files:** Modify `deskwarden/src/kit/tips.rs`, `deskwarden/src/kit/tokens.rs`.

**Interfaces:**
- Produces:
  - Hint key chips are spaced 3 apart (spec §2.7); 8 stays between the label and the first chip. This is the deferred minor from slice 1.
  - `tokens::SidebarStyle` derives `serde::{Serialize, Deserialize}`, saved as `"navy"` / `"light"` with `#[serde(rename_all = "lowercase")]`.

- [ ] Step 1, failing tests in `tips.rs`:
  - `hint_chips_are_three_apart`: paint `paint_hint(.., "Sync now", &["Mod","Shift","Y"])`; the three chip rects (fill `rgba(255,255,255,.16)`) are spaced so `next.left - prev.right == 3.0`, and the first chip's left edge is the label's right edge + 8.
- [ ] Step 1b, failing test in `tokens.rs`:
  - `sidebar_style_round_trips_as_lowercase`: `serde_json::to_string(&SidebarStyle::Light) == "\"light\""`, and it parses back.
- [ ] Step 2: run `cargo test --lib kit::`. Expect FAIL.
- [ ] Step 3: implement. Use `serde` (already a dependency).
- [ ] Step 4: run. Expect PASS.
- [ ] Step 5: commit "Hint chips three apart; the sidebar style can be saved".

---

### Task 2: The sidebar-style setting and the window's new size

**Files:** Modify `deskwarden/src/settings.rs` (`struct Settings`, `Default`, `MIN_VAULT_WINDOW_SIZE`), `deskwarden/src/prefs_ui.rs` (`draw_view`), `deskwarden/src/vault_window/mod.rs` (`WINDOW_SIZE`, `SIDEBAR_WIDTH`, `LIST_WIDTH`).

**Interfaces:**
- Produces:
  - `Settings.sidebar_style: kit::tokens::SidebarStyle` (default `Navy`; a container-level `#[serde(default)]` already covers missing keys).
  - `MIN_VAULT_WINDOW_SIZE = (930.0, 600.0)`.
  - `vault_window::WINDOW_SIZE = [1280.0, 820.0]`, `SIDEBAR_WIDTH = 252.0`, `LIST_WIDTH = 380.0`.
  - Prefs > View gets a row labelled "Sidebar", description "Navy or light, beside your vault". It uses the existing `theme::segmented_control` with two cells, "Navy" and "Light". This is an interim home: slice 7 moves it to General > Look.

- [ ] Step 1, failing tests:
  - `settings.rs`: `a_settings_file_without_a_sidebar_style_reads_as_navy` (parse `{}`) and `the_sidebar_style_is_saved_and_read_back`.
  - `prefs_ui.rs`: `the_view_page_offers_the_sidebar_style`: paint the View section and find "Navy" and "Light"; a click on "Light" sets `settings.sidebar_style == Light`.
  - `mod.rs`: `the_window_opens_at_the_designs_size_and_keeps_room_for_the_narrow_detail` asserts `WINDOW_SIZE == [1280.0, 820.0]` and `MIN_VAULT_WINDOW_SIZE.0 - SIDEBAR_WIDTH - LIST_WIDTH == 298.0`.
- [ ] Step 2: run `cargo test --lib`. Expect the new tests to FAIL. Note every OTHER failure; these are the tests pinned to 212/390/1240/900.
- [ ] Step 3: implement. Then fix each pinned test noted in step 2 by updating its literal to the new constant, or better, deriving it from the constant. Ledger the list. The prefs View toggle-count pin (~prefs_ui.rs:8758) counts 40×22 switches; the segmented control is not one, so it should hold. If it does not, update it and ledger it.
- [ ] Step 4: run `cargo test --lib --bins`. Expect PASS.
- [ ] Step 5: commit "The window opens at 1280 by 820, and the sidebar can be navy or light".

---

### Task 3: The v2 sidebar

**Files:** Modify `deskwarden/src/vault_window/sidebar.rs` (drawing and tests; the model is kept). Modify `deskwarden/src/vault_window/mod.rs` only where it consumes the new `SidebarAction` variants.

**Interfaces:**
- Consumes: `kit::{tokens, icon, card::section_label, tips::hint, toast}` and `theme::{paint_mark_colours, mark_on_sidebar}`.
- Produces:
  - `draw_sidebar` gains `style: SidebarStyle`.
  - `lock_countdown: &str` stays; it is now drawn on the Lock row.
  - `SidebarAction` gains:
    - `NewItem`, which mod.rs maps to the Ctrl+N path;
    - `CreateFolder(String)`, which mod.rs maps to `cache.create_folder(&name)` plus `kit::toast::show(ctx, "Folder created")`; it replaces `NewFolder`, which is removed;
    - `Lock`, which maps to `AccountAction::Lock`'s path;
    - `OpenSettings`, which opens the existing prefs modal the gear opened;
    - `OpenScreen(Screen)` for Shared, Password health and Shared with me, which keeps the existing screen switches.
  - `pub fn panel_frame(style: SidebarStyle) -> egui::Frame`: fill `sb.bg`, inner margin 22 top, 14 sides, 16 bottom.
- Layout (spec §2.2):
  - Logo row: pad 2/10/22, mark 28×32 in `mark_on_sidebar(sb.q1)`, gap 11, "Deskwarden" in `WORDMARK` in `sb.text`.
  - New item: height 46, radius 12, `sb.new_bg` and `sb.new_fg`, 15 Bold, plus glyph 18 at stroke 2.4, gap 10, bottom margin 18. Hover lifts it 1 px (motion). Hint `New item|Mod+N`.
  - Section labels: `SECTION` in `sb.muted`, pad 0/12/6 (VAULT) and 20/12/6 (the rest).
  - Nav rows: height 44, radius 12, pad 0 12, gap 12, `NAV` 15 SemiBold, 19 px glyph at stroke 2, count 13 at 70% opacity on the right. Active: `sb.active` / `sb.active_fg`. Hover: `sb.hover`.
  - Folder rows: height 40, glyph 18, ellipsised label.
  - Glyphs:
    - All items: `ALL`
    - Favorites: `FAV`
    - Logins: `LOGIN`
    - Cards: `CARD`
    - Secure notes: `NOTE`
    - Trash: `TRASH`
    - Shared: `SHARED`
    - Settings: `SETTINGS`
    - Folders: `FOLDER`
    - Lock: `LOCK`
    - The 0.15 extras (Apps, Passkeys, Identities, SSH keys, Archive, Password health, Shared with me) reuse `kind_mark` / existing glyphs, recoloured to the row's colour; or `FOLDER` for Shared with me.
  - New folder row: closed is height 40 with a plus glyph and "New folder" in `sb.muted`. Open is an input row in `sb.hover` at radius 12, placeholder "Folder name, then Enter", entering with `dw-up` .2 s.
  - Lock row, pinned below the scroll area: `LOCK` glyph, "Lock", countdown on the right. Hint `Lock|Mod+L`.
  - The scroll area keeps the existing bar-lane rules: rows keep one width whether or not the bar shows.

- [ ] Step 1, failing tests. Replace the v1 layout tests listed in the code map with v2 ones; keep their invariants:
  - `the_sidebar_is_252_wide_and_wears_its_styles_colours` (Navy and Light: the panel fill equals `sb.bg`; the wordmark ink is `sb.text`)
  - `the_vault_section_lists_its_rows_in_order` (the exact order from Decision 2)
  - `the_tools_section_holds_shared_password_health_and_settings`
  - `consecutive_rows_do_not_overlap_and_sit_exactly_one_gap_apart` (rewritten for a 44 px row and a 4 px gap: `item_spacing` 4, per spec gap 4)
  - `a_rail_row_is_clickable_to_its_right_edge_with_the_bar_showing_or_not` (kept)
  - `the_rails_rows_keep_one_width_whether_or_not_the_bar_is_showing` (kept)
  - `the_active_row_is_painted_in_the_styles_active_colours`
  - `new_item_returns_new_item`
  - `the_new_folder_row_opens_an_input_and_enter_creates_the_folder`: click it, type "Taxes", press Enter, and get `CreateFolder("Taxes")`
  - `escape_or_a_blank_name_creates_nothing`
  - `the_lock_row_is_pinned_below_the_rows_and_shows_the_countdown`: "11:42" is painted on the Lock row's line, and the Lock row's bottom is within 16 of the panel's bottom
  - `clicking_lock_returns_lock`
  - `the_folder_pencil_shows_on_hover_and_still_edits`
  - `the_organisations_section_sits_between_the_folders_and_the_lock_row` (an updated form of the existing one)
  - Keep, unchanged and green: the drag-to-folder tests, `the_screen_rows_survive_a_vault_with_a_folder_for_every_letter`, `the_sharing_sub_rows_partition_every_send_state`, and `the_sends_row_is_in_the_rail_once_and_is_the_only_thing_it_selects`.
- [ ] Step 2: run `cargo test --lib vault_window::sidebar`. Expect FAIL.
- [ ] Step 3: implement. Delete the v1-only tests, each one named in a ledger Ruling, for example `the_folders_plus_is_painted_on_the_shared_glyph_column`, because the plus moved to the New folder row.
- [ ] Step 4: run `cargo test --lib`. Expect PASS.
- [ ] Step 5: commit "The sidebar in v2: navy or light, sections, New item, New folder, Lock".

---

### Task 4: The top bar

**Files:**
- Create `deskwarden/src/vault_window/top_bar.rs`.
- Modify `deskwarden/src/vault_window/mod.rs`:
  - the chrome call at ~2711–2895 is replaced by `top_bar::draw`;
  - `sync_pill` (~14777) returns a `SyncPill` instead of `(Color32, String)`;
  - the Ctrl+Shift+Y chord.
- Modify `deskwarden/src/vault_window/item_list.rs`: the search field leaves the list header.
- Modify `vault_window/mod.rs` in its module list.

**Interfaces:**
- Produces:
  - `top_bar::SyncPill { label: String, tone: PillTone }` with `PillTone::{Green, Amber, Neutral}` and `hint: String`.
  - `top_bar::draw(ui, search: &mut String, pill: &SyncPill, initials: &str, maximized: bool) -> TopBarAction`.
  - `TopBarAction::{None, Sync, AvatarClicked(Rect), Minimize, ToggleMaximize, Close}`.
  - Geometry (spec §2.3):
    - Bar: 72 high, white, left padding 28, gap 14.
    - Search: 460×46, radius 14, `field_bg`, a 1 px border that turns `blue` while focused, magnifier 18 at `text_label`, 15 px input with placeholder "Search your vault". The kbd chip `Ctrl K` is 12 px `text_ghost` on a 1 px `kbd_border` at radius 6, pad 2×6, hinted `Quick search|Mod+K`.
    - Pill: 34 high, pad 0 12, radius 255, 13 SemiBold, a 7 px dot, gap 8. Green = `done_wash` / `done_ink` / `done_mark`; Amber = `caution_wash` / `caution_mark` / `caution_dot`; Neutral = `field_bg` / `text_muted` / `text_ghost`.
    - Avatar: a 38 px `ink` circle with white 14 Bold initials.
    - Caption buttons: 46×34 each, top-aligned, flush right. Glyphs `—`, `▢` (shown as `❐` when maximised, as the v1 chrome does) and `✕` in 13 px `text_icon`. Hover fills `hover`; Close hovers `close_hover` with a white glyph.
  - The empty bar between the pill and the caption buttons, and the band left of the search, drags the window (`StartDrag`). Double-click toggles maximise.
  - `sync_pill` maps the app's states to `SyncPill` exactly per Decision 5.

- [ ] Step 1, failing tests in `top_bar.rs`:
  - `the_bar_is_72_tall_and_white`
  - `the_search_is_460_by_46_and_keeps_the_vault_search_id`: the `TextEdit` has id `item_list::search_field_id()`
  - `typing_in_the_top_bar_search_filters_like_before`: write "led" into `search` through the field and assert the string
  - `the_sync_pill_tones_match_the_design` (three tones: the fill and dot colours)
  - `clicking_the_pill_returns_sync`
  - `close_hovers_red_and_returns_close`
  - `the_caption_buttons_are_46_by_34_flush_top_right`
  - `the_bar_between_the_pill_and_the_controls_drags_the_window`: a press there sends `ViewportCommand::StartDrag`, checked in `FullOutput.viewport_output[ROOT].commands`

  In `mod.rs`:
  - `sync_pill_maps_every_state_per_the_decision_table`, a table test over the existing state inputs
  - `ctrl_shift_y_syncs_like_a_click`

  In `item_list.rs`:
  - replace the `toolbar_strip_tests` that pin the search inside the list header with `the_list_header_no_longer_holds_the_search`
  - move `ctrl_k_still_focuses_the_search_field_after_the_move` and the four Esc/hint-slot tests to `top_bar.rs`, adapted to the chip
- [ ] Step 2: run `cargo test --lib vault_window`. Expect FAIL.
- [ ] Step 3: implement; wire it into mod.rs. Delete `the_settings_gear_sits_to_the_right_of_the_avatar` and `the_switcher_paints_between_the_avatar_and_the_gear`, and rewrite `the_account_menu_is_added_after_the_gear_and_exactly_once` as `the_account_menu_opens_from_the_avatar_exactly_once`. Ledger each one.
- [ ] Step 4: run `cargo test --lib --bins`. Expect PASS.
- [ ] Step 5: commit "The top bar: search, sync, avatar and the window's own buttons".

---

### Task 5: The splash

**Files:** Create `deskwarden/src/vault_window/splash.rs`. Modify `vault_window/mod.rs` (`VaultBodyState::Loading`) and `deskwarden/src/loading_ui.rs` (`FirstWindowBody::Loading`).

**Interfaces:**
- Produces `splash::paint(ui: &Ui, rect: Rect, offline: bool, fade_out: f32)`:
  - Fill `blue_deep`.
  - The shield 84×98 in `MARK_REVERSED`, scaled by `motion::breathe(t)`.
  - 18 below it, `"Opening your vault"`, or `"Opening your saved copy"` when `offline`, at 24/800/-0.01em in white.
  - 28 below that, a 220×4 track at `rgba(255,255,255,.16)` with a white bar 34% of the track's width, placed at `motion::line(t)` and clipped to the track.
  - Everything is multiplied by `1 - fade_out`.
  - It requests repaints only while it is on screen.

- [ ] Step 1, failing tests:
  - `the_splash_is_navy_with_the_reversed_shield_and_its_line`: the fill is `blue_deep`; the text "Opening your vault" is white.
  - `offline_opens_the_saved_copy`
  - `the_bar_stays_inside_its_track`, for t in 0..1.2 step .1
  - `a_faded_splash_paints_nothing_opaque`: fade 1 gives alpha 0.
  - In mod.rs: `the_vault_body_shows_the_splash_while_loading_and_fades_it_when_ready`, driven through `VaultBodyState` with `RawInput.time`.
- [ ] Step 2: run. Expect FAIL.
- [ ] Step 3: implement. `loading_ui`'s Loading body calls `splash::paint` across the window body. Update the `loading_ui` tests at ~:1725/:1772 that assert a 46 px HEADING bar, but only for the Loading state; Slow and Unreachable keep theirs. Ledger it.
- [ ] Step 4: run `cargo test --lib --bins`. Expect PASS.
- [ ] Step 5: commit "The vault opens behind a breathing shield".

---

### Task 6: Hosting toasts and hints; the full-height layout

**Files:** Modify `deskwarden/src/vault_window/mod.rs`.

**Interfaces:**
- Panel order in the frame closure:
  1. `Panel::left("vault-sidebar").exact_size(SIDEBAR_WIDTH).frame(sidebar::panel_frame(style))`
  2. `Panel::top("vault-top-bar")` with the top bar (main column only)
  3. the existing list and detail panels
- `draw_resize_handles` still runs.
- `kit::toast::paint(ctx)` runs once at the end of every frame.
- `kit::tokens::set_palette(ctx, Palette::LIGHT)` runs at window start (dark mode later sets it here).
- The copy toast `detail.rs` uses today stays; slice 3 moves copies onto `kit::toast`.

- [ ] Step 1, failing tests in mod.rs:
  - `the_sidebar_runs_the_full_height_of_the_window`: the sidebar panel rect's top is 0 and its bottom is the window height. Use the existing harness that runs the vault frame at 1280×820.
  - `the_top_bar_starts_where_the_sidebar_ends`: the bar's left equals `SIDEBAR_WIDTH`.
  - `a_toast_shown_by_any_screen_is_painted_by_the_window`.
  - Update the `item_pane_frame_placement_tests` source pin if the list panel's text changes; ledger it.
- [ ] Step 2: run. Expect FAIL.
- [ ] Step 3: implement.
- [ ] Step 4: run `cargo test --lib --bins`. Expect PASS.
- [ ] Step 5: commit "The sidebar takes the window's height; the window hosts toasts".

---

### Task 7: Previews, and slice verification

**Files:** Modify `deskwarden/examples/ui_preview.rs`.

**Interfaces:**
- Produces:
  - New surfaces `VaultShellNavy` and `VaultShellLight` ("vault_shell_navy", "vault_shell_light", 1280×820, flag `--shell`). Each draws the real sidebar, the top bar and the 0.15 list and detail panes with the existing fixtures.
  - `VaultSplash` ("vault_splash", 1280×820).
  - `VaultRail` is updated to 252 wide with `sidebar::panel_frame(style)`, replacing the hand-copied frame, and the preview's local `SIDEBAR_WIDTH` / `LIST_WIDTH` consts take the new values.

- [ ] Step 1: run `cargo run --release --example ui_preview -j 2 -- --shell --screenshot`. Compare each PNG with artboard 1a (served from `docs/design`): sidebar width and colours, row heights, the section labels, the New item button, the top bar's search, pill and avatar, the caption buttons. Fix mismatches in the owning task's module.
- [ ] Step 2: run `cargo run --release --example ui_preview -j 2 -- --all`. Every surface renders.
- [ ] Step 3: slice verification:
  - `cargo test --lib --bins` passes.
  - `cargo check --all-targets` has 0 warnings.
  - `cargo deny check` is ok.
  - Bare-LF checks on touched files are 0.
- [ ] Step 4: commit "Previews of the v2 shell, navy and light"; send the owner the two shell PNGs and the splash.

## Review Focus

1. **Window controls in every state.** Maximise and restore, minimise, close-to-tray and drag from the bar must work when the window is maximised and when it is at its minimum size. Covered in Task 4 (`the_bar_between_the_pill_and_the_controls_drags_the_window`); add `double_clicking_the_bar_toggles_maximise`.
2. **A vault with many folders or organisations.** The rows must scroll while Lock stays pinned and visible. Task 3 keeps `the_screen_rows_survive_a_vault_with_a_folder_for_every_letter` and adds `the_lock_row_stays_visible_with_a_hundred_folders`.
3. **Keyboard focus.** Typing a new folder name must not trigger Ctrl-less shortcuts, and Esc in the search must still clear it rather than close anything. Task 3 adds `typing_a_folder_name_does_not_reach_the_vault_shortcuts`; Task 4 keeps the Esc tests.
4. **A slow or failed open** must still show the 0.15 Slow and Unreachable bodies with their retry and local-copy offers, not a splash that never ends. Task 5 adds `slow_and_unreachable_keep_their_bodies`.
5. **Long account emails and long folder names** must ellipsise and never push the caption buttons off the window or overlap the counts. Task 3 adds `a_long_folder_name_ellipsises_before_its_count`; Task 4 adds `the_caption_buttons_stay_on_screen_at_the_minimum_width`.

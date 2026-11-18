# Slice 1: Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the crate the v2 design's visual vocabulary as one module, `kit`. That covers tokens, motion, stroke icons, controls, cards, tips, hints and toasts. It also gives the shield its v2 shape. No existing screen changes, apart from the mark, which is global.

**Architecture:** A new module tree `deskwarden/src/kit/` holds everything v2. The 16k-line `theme.rs` stays as the v1 look that unrestyled screens keep using, until each later slice moves its screen onto `kit`. Colours live in a `Palette` value stored in the egui context rather than in `const`s. That lets slice 8's dark mode swap palettes without touching any widget. The mark keeps its home in `theme.rs` because the GDI `unlock_prompt` reads its outlines from there. Only its geometry changes, and the outlines are now parsed from the v2 SVG path strings by `kit::icon`.

**Tech Stack:** Rust 2021, egui/eframe 0.35, the existing Archivo faces in `theme.rs`, Python 3 stdlib for `assets/generate-icon.py`.

**Spec:** `docs/design/deskwarden-v2-design-spec.md`. Part A §1 covers tokens, A §2.2 the sidebar palettes and nav icon paths, B §0 the shared primitives, C §0 the window and components, and D §2 the brand. The roadmap is `docs/superpowers/specs/2026-11-18-deskwarden-1-0-redesign-design.md` (slice 1).

## Global Constraints

- Work on branch `v1`. Commit after each task. Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `.rs` files are CRLF. Edit with the Edit tool, or with a Python script written via the Write tool. Never use a Bash heredoc with backslashes. After editing, check that the file has 0 bare LF: `python -c "b=open(p,'rb').read(); print(b.count(b'\n')-b.count(b'\r\n'))"`.
- A new top-level module must be added to `OPENS_NO_WINDOW` in `src/foreground.rs` (`every_module_in_this_crate_is_classified_as_opening_windows_or_not`) and the array length bumped. Directory modules are listed by name (`"kit"`).
- No new dependency, no new non-Rust file. The `job_object.rs` file-set test lists every non-Rust file, and this slice adds none. `assets/deskwarden.ico` and `assets/generate-icon.py` change content only.
- Weights: Archivo ships Regular/SemiBold/Bold/ExtraBold only. CSS 500 maps to Regular and 900 to ExtraBold. Use `theme::{REGULAR, SEMIBOLD, BOLD, EXTRABOLD}` and `theme::letterspaced` for tracking.
- Colours are the spec's hex values exactly. Where the spec gives `rgba(r,g,b,a)`, use `Color32::from_rgba_unmultiplied(r,g,b,(a*255).round())`.
- **Mark colours (owner decision 2026-11-18):** the v2 shape with the existing checkerboard fills `QUADRANT_FILLS = [BLUE_DEEP, BLUE_BRIGHT, BLUE_SOFT, BLUE]`. Do not reorder to palette order. On-navy variants use the design's own colours (Task 8).
- Verification for the slice: `cargo test --lib --bins`, `cargo check --all-targets` (0 warnings), `cargo deny check`. Run from `deskwarden/`.

## Review Focus

1. **Idle CPU.** A settled animation must stop asking for repaints. A window left open must not redraw at 60 fps because a toast or a switch once animated. Tested in Task 2 (`progress_stops_requesting_repaints_once_settled`).
2. **Hints at the window's edges.** A hint on a control in the top bar, or flush against the left or right edge, must stay fully inside the window. Tested in Task 6 (`a_hint_never_leaves_the_window`).
3. **A long toast** must not run off the window. It is capped at 560 px and ellipsised. Tested in Task 7 (`a_long_toast_is_ellipsised_inside_the_cap`).
4. **Malformed icon data** must be an `Err`, never a panic. Tested in Task 3 (`malformed_path_data_is_an_error_not_a_panic`).
5. **The 14 px mark** (tray and field badge size) must keep all four quadrants inside its box at fractional scale. Tested in Task 8 (`the_mark_stays_inside_its_box_at_every_ramp_size`).

---

### Task 1: Tokens and palettes

**Files:**
- Create: `deskwarden/src/kit/mod.rs`, `deskwarden/src/kit/tokens.rs`
- Modify: `deskwarden/src/lib.rs` (add `pub mod kit;` next to `pub mod theme;`), `deskwarden/src/foreground.rs` (`OPENS_NO_WINDOW`: add `"kit"` with a one-line comment "the v2 widget kit; paints into windows other modules open", length +1)

**Interfaces:**
- Produces:
  - `kit::tokens::Palette`: a `Copy` struct of `Color32` fields, named exactly as below.
  - `Palette::LIGHT: Palette`.
  - `kit::tokens::palette(ctx: &egui::Context) -> Palette`: reads it from `ctx.data` and defaults to `LIGHT`.
  - `kit::tokens::set_palette(ctx: &egui::Context, p: Palette)`.
  - `kit::tokens::SidebarStyle { Navy, Light }` (default `Navy`), with `fn palette(self) -> SidebarPalette`.
  - `SidebarPalette { bg, text, muted, nav_fg, hover, active, active_fg, new_bg, new_fg, q1 }`.
  - Radii `u8` consts: `R_WINDOW = 18`, `R_CARD = 18`, `R_CARD_SETTINGS = 20`, `R_HERO = 22`, `R_CONTROL = 14`, `R_NAV = 12`, `R_SMALL = 11`, `R_TAG_LIST = 6`, `R_TAG_DETAIL = 7`, `R_PILL = 255`.
  - `kit::tokens::Role { size: f32, face: &'static str, tracking_em: f32 }` with consts `PAGE_TITLE`, `LIST_HEADER`, `WORDMARK`, `FIELD_VALUE`, `TOTP`, `BUTTON`, `ROW_TITLE`, `NAV`, `BODY`, `SUBTITLE`, `LABEL`, `SECTION`, `TAG_LIST`, `TAG_DETAIL`, `TOOLTIP`, `HINT`, `PILL`, `SEGMENT`.
  - `kit::tokens::job(role: Role, text: &str, color: Color32) -> egui::text::LayoutJob`, built with `theme::letterspaced`; `SECTION` text is uppercased.
  - `kit::tokens::card_shadow() -> egui::Shadow`.
  - `kit::tokens::card_frame(p: &Palette, radius: u8) -> egui::Frame`.

`Palette` fields and LIGHT values (spec A §1.1, B intro, C §0.3):

| field | hex | field | hex |
|---|---|---|---|
| `ink` | 201e1d | `blue_deep` | 14307a |
| `text_nav` | 4a4745 | `blue` | 1b3fa0 |
| `text_muted` | 605d5d | `blue_bright` | 3b74e8 |
| `text_icon` | 6b6866 | `blue_bright_hover` | 2f63cf |
| `text_faint` | 7d7979 | `blue_soft` | 7fa4ef |
| `text_label` | 8a8785 | `blue_wash` | eef2fc |
| `text_ghost` | 9b9797 | `blue_edge` | b8c7ea |
| `card` | ffffff | `focus` | dbe4f7 |
| `list_bg` | faf9f8 | `chip_on` | f5f7fd |
| `window_bg` | f5f4f2 | `row_hover_blue` | f7f8fc |
| `field_bg` | f3f2f0 | `done_mark` | 1b7a3f |
| `hover` | f1f0ee | `done_ink` | 17673a |
| `secondary_hover` | f7f6f5 | `done_wash` | e8f3ec |
| `badge_bg` | eceae7 | `tag_2fa_bg` | dff0e5 |
| `hairline` | e8e6e3 | `caution_wash` | fef6e7 |
| `card_ring` | ebe9e6 | `caution_mark` | 8a5a06 |
| `row_rule` | f1efec | `caution_ink` | 7a4f05 |
| `border` | e0deda | `caution_edge` | f2d99b |
| `kbd_border` | dcdad7 | `caution_dot` | b7791f |
| `switch_off` | d9d6d2 | `error` | b42318 |
| `meter_track` | eeece9 | `danger_edge` | f0cfca |
| `tag_neutral_bg` | e4e1dd | `danger_wash` | fdf3f2 |
| `tag_neutral_ink` | 2e2b29 | `danger_ink` | 8c3c33 |
| `star` | f5b301 | `close_hover` | c42b1c |

Sidebar palettes: spec A §2.2 table, verbatim (Navy `bg` 14307a … `q1` ffffff; Light `bg` ffffff … `q1` 14307a).

Roles (size, face, tracking):

| Role | Value | Role | Value |
|---|---|---|---|
| `PAGE_TITLE` | 32, EXTRABOLD, -0.03 | `BODY` | 15, REGULAR, 0 |
| `LIST_HEADER` | 26, EXTRABOLD, -0.02 | `SUBTITLE` | 14, REGULAR, 0 |
| `WORDMARK` | 20, EXTRABOLD, -0.02 | `LABEL` | 13, SEMIBOLD, 0 |
| `FIELD_VALUE` | 18, REGULAR, 0 | `SECTION` | 12, BOLD, 0.12 |
| `TOTP` | 26, BOLD, 0.12 | `TAG_LIST` | 11, BOLD, 0 |
| `BUTTON` | 16, BOLD, 0 | `TAG_DETAIL` | 12, BOLD, 0 |
| `ROW_TITLE` | 16, BOLD, 0 | `TOOLTIP` | 14, REGULAR, 0 |
| `NAV` | 15, SEMIBOLD, 0 | `HINT` | 13, SEMIBOLD, 0 |
| `PILL` | 13, SEMIBOLD, 0 | `SEGMENT` | 14, BOLD, 0 |

`card_shadow()` is offset (0, 1), blur 3, spread 0, colour `rgba(32,30,29,.06)`. `card_frame` uses fill `card`, `corner_radius(radius)`, `stroke(1, card_ring)` and `shadow(card_shadow())`.

- [ ] **Step 1: Write the failing tests** in `kit/tokens.rs` `#[cfg(test)] mod tests`:
  - `light_palette_matches_the_design`: assert 8 spot values (`ink`, `window_bg`, `list_bg`, `border`, `card_ring`, `row_rule`, `switch_off`, `close_hover`) by `Color32::from_rgb`.
  - `navy_and_light_sidebars_match_the_design_table`: every field of both `SidebarStyle::Navy.palette()` and `SidebarStyle::Light.palette()`, including `muted` = `from_rgba_unmultiplied(255,255,255,128)` for Navy, `nav_fg` alpha 209 (.82), `hover` alpha 20 (.08) and `active` alpha 41 (.16).
  - `the_palette_defaults_to_light_and_can_be_swapped`: a fresh `Context` gives `palette(&ctx) == Palette::LIGHT`. After `set_palette` with `ink` changed, `palette` returns the change. Derive `PartialEq` on `Palette`.
  - `section_role_text_is_uppercased_and_tracked`: `job(SECTION, "Vault", c).text == "VAULT"`, and letter spacing is present (sections > 1, or the extra_letter_spacing that `theme::letterspaced` produces is > 0).
  - `cards_carry_the_ring_and_the_soft_shadow`: for `card_frame(&LIGHT, R_CARD)`, `stroke == Stroke::new(1.0, card_ring)`, `corner_radius == CornerRadius::same(18)`, `shadow.offset == [0, 1]`, `shadow.blur == 3`.
- [ ] **Step 2: Run** `cargo test --lib kit::tokens`. Expect a compile FAIL (module missing).
- [ ] **Step 3: Implement** `kit/mod.rs` (doc comment: v2 kit; `theme.rs` remains v1 until each slice moves its screen), `tokens.rs`, the `lib.rs` line, and the `foreground.rs` entry.
- [ ] **Step 4: Run** `cargo test --lib kit::tokens` and `cargo test --lib foreground::`. Expect PASS.
- [ ] **Step 5: Commit** with the message "The v2 kit begins: its palette, its sidebars, its type".

---

### Task 2: Motion

**Files:** Create `deskwarden/src/kit/motion.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Produces:
  - `CubicBezier { x1, y1, x2, y2 }` with `const fn new(..)` and `fn at(&self, t: f32) -> f32` (solves x = t for the curve parameter, returns y; t is clamped to 0..1).
  - Consts `EASE_OUT (0,0,.58,1)`, `EASE_IN_OUT (.42,0,.58,1)`, `PANE (.2,.8,.2,1)`, `POP (.2,.9,.3,1.15)`, `OVERSHOOT (.2,.9,.3,1.2)`, `SWITCH (.3,1.4,.5,1)`.
  - `Pose { opacity: f32, dx: f32, dy: f32, scale: f32 }` with `Pose::REST`.
  - Keyframes as fns of eased progress `p` (0..1): `up(p)` (dy 10→0, opacity 0→1), `fade(p)`, `pop(p)` (dy 12→0, scale .97→1), `drop(p)` (dy −8→0, scale .98→1), `tip(p)` (dy 4→0), `toast(p)` (dy 14→0), `ring(p)` (scale .6→1).
  - Looping fns of seconds: `breathe(t) -> f32` (scale, period 1.6, peak 1.06 at half period, ease-in-out), `line(t) -> f32` (bar x as a fraction of its own width, −1.1 → 3.3 over 1.2 s, ease-in-out, repeating), `pulse(t, max_radius) -> (f32, f32)` (radius 0→max and alpha 1→0 over .9 s, ease-out, repeating).
  - `progress(ctx: &Context, id: Id, delay: f32, duration: f32, ease: CubicBezier) -> f32`: remembers the first frame it was asked about (`ctx.data` keyed by `id`), returns eased 0..1, and calls `ctx.request_repaint()` only while the value is < 1.
  - `restart(ctx, id)`: forgets the start.
  - `stagger(index: usize, step: f32) -> f32 = index as f32 * step`.

- [ ] **Step 1: Failing tests:**
  - `curves_start_at_0_and_end_at_1`: all six consts give `at(0)==0`, `at(1)==1` (±1e-4).
  - `ease_out_is_ahead_of_linear_halfway`: `EASE_OUT.at(.5) > .5`.
  - `the_pop_curves_overshoot`: `max over t in 0..1 step .01` of `POP.at(t) > 1.0`, and likewise for `OVERSHOOT` and `SWITCH`.
  - `entrances_begin_offset_and_end_at_rest`: `up(0) == Pose{opacity:0, dy:10, ..}`, `up(1) == Pose::REST`; `pop(0).scale == .97`; `drop(0).dy == -8.0`; `toast(0).dy == 14.0`; `ring(0).scale == .6`.
  - `breathe_peaks_at_half_period`: `breathe(0)==1`, `breathe(.8)==1.06` (±1e-3), `breathe(1.6)==1`.
  - `the_splash_bar_sweeps_and_repeats`: `line(0)==-1.1`, `line(1.2-1e-4)≈3.3`, `line(1.2)≈-1.1`.
  - `pulse_grows_and_fades`: `pulse(0,16)==(0,1)`, `pulse(.9-1e-4,16)≈(16,0)`.
  - `progress_stops_requesting_repaints_once_settled`: drive `ctx.run_ui` with `RawInput.time` = 0.0, 0.1, 0.5. With duration .4, the first two frames' `FullOutput.viewport_output[&ViewportId::ROOT].repaint_delay` is `Duration::ZERO`. At 0.5 the value is 1.0 and the repaint delay is not zero.
- [ ] **Step 2: Run** `cargo test --lib kit::motion`. Expect FAIL.
- [ ] **Step 3: Implement.** `CubicBezier::at` solves x(u) = t by bisection: 24 iterations over u in 0..1, which is monotonic in x for every curve here because all x control points are in 0..1.
- [ ] **Step 4: Run** the same command. Expect PASS.
- [ ] **Step 5: Commit** "Motion: the design's curves and keyframes, and animations that go quiet".

---

### Task 3: Stroke icons from SVG path data

**Files:** Create `deskwarden/src/kit/icon.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Produces:
  - `Subpath { points: Vec<Pos2>, closed: bool }` (viewBox units).
  - `IconError { at: usize, reason: &'static str }`.
  - `parse(d: &str) -> Result<Vec<Subpath>, IconError>`. It supports `M m L l H h V v C c S s Q q A a Z z`, implicit repeats, comma or space separators, and numbers like `.5`, `-.4`, `1e-3`, `5.5.5` (two numbers). Cubics and quadratics are flattened at 12 steps. Arcs are converted from endpoint to centre form (SVG spec F.6.5) and flattened at one point per 15°.
  - `stroke(painter: &Painter, rect: Rect, d: &str, width: f32, color: Color32)`: fits a 24×24 viewBox into `rect` (uniform scale, centred). `width` is in viewBox units. It draws each subpath with `PathShape::line` (or `closed_line`). An `Err` draws nothing and `debug_assert!`s.
  - `fill(painter, rect, d, view: Vec2, color)`: a convex fill per subpath, used by the mark.
  - `pub mod glyph`: `&str` consts holding the spec's path data verbatim:
    - from A §2.2: `ALL`, `FAV`, `LOGIN`, `CARD`, `NOTE`, `TRASH`, `SHARED`, `GEN`, `SETTINGS`, `FOLDER`;
    - `PLUS = "M12 5v14M5 12h14"`;
    - `LOCK = "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4"`;
    - `BACK = "M19 12H5M12 19l-7-7 7-7"`;
    - `CHECK = "M5 12l5 5 9-10"`.

- [ ] **Step 1: Failing tests:**
  - `a_relative_horizontal_line_parses_to_two_points`: `parse("M3 6h18")` gives one open subpath `[(3,6),(21,6)]`.
  - `close_marks_the_subpath_closed`: `parse("M5 11h14v10H5z")` gives `closed == true` and 4 points.
  - `packed_numbers_split_where_svg_splits_them`: `parse("M.5 14H12v13.2C5 25.4.9 21 .5 14Z")` gives a final point (0.5, 14) and passes through (12, 27.2).
  - `an_arc_ends_where_svg_says`: `parse("M8 11V7a4 4 0 0 1 8 0v4")` gives a last point (16, 11), and every arc point lies within 4±0.01 of (12, 7).
  - `every_glyph_parses`: every `glyph::*` const is `Ok`, and every point is inside 0..24 on both axes.
  - `malformed_path_data_is_an_error_not_a_panic`: `parse("M3")`, `parse("X1 2")`, `parse("M1 2 L")` and `parse("")` are `Err` (or, for `""`, `Ok(vec![])`). No panic.
- [ ] **Step 2: Run** `cargo test --lib kit::icon`. Expect FAIL.
- [ ] **Step 3: Implement** a hand-written tokenizer. No dependency is added.
- [ ] **Step 4: Run.** Expect PASS.
- [ ] **Step 5: Commit** "Icons are the design's own path data, drawn as strokes".

---

### Task 4: Controls (switch, segmented, buttons)

**Files:** Create `deskwarden/src/kit/controls.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Consumes: `tokens::{palette, R_*, Role, job}`, `motion::{progress, SWITCH}`, `icon::stroke`.
- Produces:
  - `switch(ui: &mut Ui, on: &mut bool) -> Response`: track 50×30 at radius 255; knob 24 white at inset 3 with shadow (0,1) blur 3 `rgba(0,0,0,.2)`; knob x travels 20 with `SWITCH` easing over .22 s; track colour `blue` when on, `switch_off` when off. A click flips `*on` and the response is `changed()`.
  - `SegmentStyle { pad: Vec2, font_px: f32, cell_radius: u8, track_radius: u8, full_width: bool }` with consts:
    - `SEG_SETTINGS (13,8, 14, 10, 13, false)`
    - `SEG_APPS (14,8, 14, 10, 13, false)`
    - `SEG_TABS (15,9, 14, 10, 13, false)`
    - `SEG_GENERATOR (20,10, 15, 11, 13, false)`
    - `SEG_EXPIRY (0,10, 15, 11, 14, true)`
  - `segmented(ui, options: &[&str], selected: usize, style: SegmentStyle) -> Option<usize>` returns the clicked index. Track pad 4, fill `hover` (#f1f0ee). Selected cell `card` with shadow (0,1) blur 3 `rgba(32,30,29,.14)`, ink text. The others are transparent with `text_icon` text. Bold face.
  - `ButtonKind { Primary, Outline, Tinted, Danger }`.
  - `ButtonSize { height: f32, pad_x: f32, radius: u8, font_px: f32 }` with consts `LARGE (48, 22, 14, 16)`, `ROW (42, 16, 12, 15)`, `SMALL (40, 14, 11, 14)`.
  - `button(ui, kind, size, label: &str, icon: Option<&str>) -> Response`. Colours per spec B §0.6:
    - Primary: `blue`, white text, Bold, shadow (0,6) blur 16 `rgba(27,63,160,.28)`; hover fill `blue_deep`.
    - Outline: border `border`, SemiBold `ink`; hover fill `secondary_hover`.
    - Tinted: `blue_wash`, `blue_deep` text, Bold; hover `focus`.
    - Danger: border `danger_edge`, `error` text; hover `danger_wash`.
    - The icon is drawn at 16–18 px before the label with a gap of 7.
  - `icon_button(ui, size: f32 /*42 or 38*/, glyph: &str, color: Color32) -> Response`: radius 12 for 42 and 11 for 38, hover fill `hover`, glyph 19 px.

- [ ] **Step 1: Failing tests** (harness: one `ctx.run_ui` frame per call; collect `Shape::Rect` and `Shape::Text` the way `theme.rs`'s `run_frame`/`collect_run` do):
  - `a_switch_is_fifty_by_thirty_and_blue_when_on`: the allocated rect is 50×30. On settle (run frames with `RawInput.time` advanced past .22 s) there is a track `RectShape` with fill `blue`, and the knob's centre is at x = left + 3 + 12 + 20.
  - `clicking_a_switch_flips_it`: a press then release at the centre on the second frame flips `on` from false to true and `changed()` is true.
  - `the_selected_segment_is_white_and_the_rest_are_not`: options `["Light","Dark","System"]`, selected 2. Exactly one cell rect has fill `card`, and its text colour is `ink`. The other labels are `text_icon`.
  - `clicking_a_segment_returns_its_index`: a click on "Dark" returns `Some(1)`.
  - `button_sizes_match_the_design`: `LARGE` gives height 48 and radius 14; `ROW` gives 42 and 12. The Primary fill is `blue`, the Danger text colour `error`.
- [ ] **Step 2: Run** `cargo test --lib kit::controls`. Expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expect PASS.
- [ ] **Step 5: Commit** "Switches, segments and buttons in the v2 shape".

---

### Task 5: Cards, rows, labels, tags

**Files:** Create `deskwarden/src/kit/card.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Consumes: `tokens::{card_frame, palette, Role, job}`.
- Produces:
  - `card(ui, radius: u8, inner_margin: egui::Margin, add: impl FnOnce(&mut Ui) -> R) -> R` (wraps `card_frame`).
  - `row_rule(ui)`: 1 px `row_rule` across the available width.
  - `field_label(ui, text)` (`LABEL` role, `text_label`).
  - `section_label(ui, text, color)` (`SECTION` role).
  - `TagKind { App, TwoFa, Shared }`.
  - `TagSize { List, Detail }`.
  - `tag(ui, kind, size)` draws the label "App", "2FA" or "Shared" with these colours: App `tag_neutral_bg`/`tag_neutral_ink`; 2FA `tag_2fa_bg`/`done_ink`; Shared `blue`/white. List pad 2×7, radius 6, 11 px. Detail pad 3×9, radius 7, 12 px.

- [ ] **Step 1: Failing tests:**
  - `a_card_paints_one_rounded_white_rect_with_its_ring`: one `RectShape` with fill white, radius 18 and stroke 1 `card_ring`.
  - `tags_use_the_design_colours_and_padding`: for each kind and size, the background rect fill matches the colours above. A List tag is 4 px taller than its text galley (pad 2 top and bottom), a Detail tag 6 px taller.
  - `the_row_rule_is_one_pixel_of_row_rule`.
- [ ] **Step 2: Run** `cargo test --lib kit::card`. Expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expect PASS.
- [ ] **Step 5: Commit** "Cards, rules, labels and tags".

---

### Task 6: `?` help bubbles and hover hints

**Files:** Create `deskwarden/src/kit/tips.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Consumes: `tokens`, `motion::{progress, tip, fade}`.
- Produces:
  - `help_dot(ui, id: Id, text: &str, width: f32) -> Response`.
    - Dot: 18 px circle, `badge_bg`, "?" in 11 px ExtraBold `text_icon`, with a help cursor.
    - On hover it paints the bubble on `Order::Tooltip`: width `width` (260 by default, 240 or 280 at call sites), pad 11×13, radius 12, `ink` fill, white `TOOLTIP` text with line height 1.45, shadow (0,12) blur 28 `rgba(0,0,0,.22)`.
    - Placed with `left = dot.left - 12` and `bottom = dot.top - 10`, entering with `tip` over .16 s.
  - `hint(ui: &Ui, response: &Response, label: &str, keys: &[&str])`.
    - When `response.hovered()` and no pointer button is down, it paints a hint bubble: pad 7/8/7/11, radius 10, `ink`, 13 px SemiBold white, gap 8, shadow (0,8) blur 20 `rgba(0,0,0,.25)`, `fade` .14 s.
    - Key chips: min 20×20, pad 0 5, radius 5, fill `rgba(255,255,255,.16)`, 11 px Bold.
  - `key_label(k: &str) -> &str`: `"Mod"` becomes `"Ctrl"`; anything else is returned unchanged.
  - `paint_help_bubble(ui: &Ui, anchor: Rect, text: &str, width: f32)` and `paint_hint(ui: &Ui, target: Rect, label: &str, keys: &[&str])` are the unconditional painters that `help_dot` and `hint` call on hover. They are public so that the kit sheet and the tests can show a bubble without a pointer.
  - Pure `hint_rect(target: Rect, window: Rect, bubble: Vec2) -> Rect`:
    - Centred on `target` and 8 px above it.
    - Below it (8 px gap) instead when `target.top() - window.top() < 56`.
    - The centre x is clamped to `window.left()+90 ..= window.right()-90`.
    - The result is finally clamped fully inside `window`.

- [ ] **Step 1: Failing tests:**
  - `hints_sit_above_their_control`: a target at (600..640, 400..440) in a 1280×820 window, with a bubble 120×34, gives a rect bottom of 392 centred at x 620.
  - `hints_in_the_top_bar_drop_below`: a target with top 20 gives a rect top of target.bottom + 8.
  - `hints_near_a_side_are_pulled_in_by_ninety`: a target centred at x 10 gives a rect centre x of 90.
  - `a_hint_never_leaves_the_window`: for targets at all four corners and a bubble 300×34, `window.contains_rect(rect)`.
  - `mod_reads_ctrl_on_windows`: `key_label("Mod") == "Ctrl"` and `key_label("Shift") == "Shift"`.
  - `a_help_bubble_is_hidden_until_hovered_and_shows_its_text`: run with no pointer and assert no `Text` shape contains the tip text. Run with the pointer over the dot and assert one does, and that it is inside a rect filled `ink`.
- [ ] **Step 2: Run** `cargo test --lib kit::tips`. Expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expect PASS.
- [ ] **Step 5: Commit** "Help bubbles and hover hints, kept inside the window".

---

### Task 7: Toasts

**Files:** Create `deskwarden/src/kit/toast.rs`. Modify `kit/mod.rs`.

**Interfaces:**
- Consumes: `tokens`, `motion::{progress, toast, OVERSHOOT}`, `icon::{stroke, glyph::CHECK}`.
- Produces:
  - `show(ctx: &Context, message: impl Into<String>)`: stores `{message, shown_at: ctx.input(|i| i.time)}` in `ctx.data`, replacing any toast already showing, and restarts its animation.
  - `paint(ctx: &Context)`: the host window calls this once at the end of each frame.
  - `current(ctx) -> Option<String>`.
  - Constants `LIFETIME = 2.0` (seconds) and `MAX_WIDTH = 560.0`.
  - Geometry (spec B §0.7):
    - Pill height 50, centred, with its bottom 28 above the bottom of `ctx.screen_rect()`.
    - Pad 0 22 0 16, `ink` fill, white 15 px SemiBold, gap 10.
    - A leading 26 px `done_mark` circle with a white `CHECK` stroke of width 3.
    - Shadow (0,14) blur 34 `rgba(0,0,0,.25)`, on `Order::Foreground`.
  - Enters with `toast` easing `OVERSHOOT` over .28 s. Once its lifetime is past it is cleared, and no repaint is requested.

- [ ] **Step 1: Failing tests:**
  - `a_toast_shows_then_leaves_after_two_seconds`: `show` at t=0, then `paint` at t=1.0 has the message painted and `current` is `Some`. At t=2.01 `current` is `None` and no `Text` shape carries the message.
  - `a_new_toast_replaces_the_old_and_restarts_the_clock`: `show("A")` at 0 and `show("B")` at 1.5 gives only "B" at 3.0, and nothing at 3.6.
  - `the_toast_sits_twenty_eight_above_the_bottom_and_is_fifty_tall`: in an 800×600 screen the pill rect has bottom 572 and height 50, centred at x 400.
  - `a_long_toast_is_ellipsised_inside_the_cap`: a 400-character message gives a pill width ≤ 560 and a galley that ends in "…".
- [ ] **Step 2: Run** `cargo test --lib kit::toast`. Expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Expect PASS.
- [ ] **Step 5: Commit** "Toasts: one at a time, two seconds, never off the window".

---

### Task 8: The v2 shield

**Files:**
- Modify: `deskwarden/src/theme.rs`. That covers the mark section (`QUADRANT_FILLS` doc, `ARC_K`, `flatten_cubic` if now unused, `quadrant_outlines`/`build_quadrant_outlines`, `paint_mark_with`, `mark_ink_rect`, `window_icon`) and the mark tests near the existing `quadrant_*` and `window_icon_*` tests.
- Modify: `deskwarden/assets/generate-icon.py`. Regenerate `deskwarden/assets/deskwarden.ico`.

**Interfaces:**
- Consumes: `kit::icon::parse`.
- Produces (existing names keep their signatures; `unlock_prompt` is unchanged):
  - `quadrant_outlines()` now returns the four v2 quadrants, parsed from consts `MARK_TL = "M12 1H5.5A5 5 0 0 0 .5 6v8H12Z"`, `MARK_TR = "M12 1h6.5a5 5 0 0 1 5 5v8H12Z"`, `MARK_BL = "M.5 14H12v13.2C5 25.4.9 21 .5 14Z"` and `MARK_BR = "M12 14h11.5c-.4 7-4.5 11.4-11.5 13.2Z"`, in that order.
  - `QUADRANT_FILLS` keeps its checkerboard value. Its doc comment adds the owner's 2026-11-18 decision to keep it on the v2 shape.
  - New `pub const MARK_REVERSED: [Color32; 4] = [WHITE, BLUE_EDGE, BLUE_SOFT, BLUE_BRIGHT]` (design: on `#14307a` and `#1b3fa0`).
  - New `pub fn mark_on_sidebar(q1: Color32) -> [Color32; 4] = [q1, BLUE_BRIGHT, BLUE_SOFT, BLUE_EDGE]`.
  - New `pub fn paint_mark_colours(painter, rect, fills: [Color32; 4])`. `paint_mark` and `paint_mark_tinted` route through it.
  - `mark_ink_rect` now insets by the v2 ink bounds: x 0.5..23.5, y 1..27.2.
  - `generate-icon.py`: `CORNER_RADIUS = 5.0`, body x in [0.5, 23.5], top edge y = 1, `_BOTTOM_CUBIC = ((12.0, 27.2), (5.0, 25.4), (0.9, 21.0), (0.5, 14.0))`, `QUADRANT_RGB` unchanged (checkerboard). The docstring and comments are updated to the v2 numbers. Run `python generate-icon.py` from `deskwarden/assets` to rewrite the `.ico`.

- [ ] **Step 1: Update or add failing tests:**
  - Rename and update the viewbox test to `quadrant_outlines_stay_inside_the_v2_ink_box`: every point is within x 0.5..23.5 and y 1..27.2 (±0.01).
  - `the_quadrants_meet_at_the_centre_and_the_tip`: every outline contains a point within 0.01 of (12, 14), and both bottom outlines reach (12, 27.2).
  - Keep `quadrant_tones_alternate_around_the_mark` unchanged. It must still pass.
  - `reversed_and_sidebar_colourways_match_the_design`: the two arrays above.
  - `the_mark_stays_inside_its_box_at_every_ramp_size`: for sizes (14,16), (22,26), (32,37) and (48,56), at `pixels_per_point` 1.0, 1.25 and 1.5, every painted polygon point lies inside the rect `paint_mark` was given.
  - The existing `the_window_icon_is_the_shield_the_exe_carries` must pass after the `.ico` is regenerated.
- [ ] **Step 2: Run** `cargo test --lib theme::tests::quadrant theme::tests::the_mark theme::tests::reversed`. Expect FAIL.
- [ ] **Step 3: Implement** the geometry via `kit::icon::parse`, flattened to the mark's convex outlines (each quadrant is convex, so `Shape::convex_polygon` still applies). Then update the Python and regenerate the `.ico`.
- [ ] **Step 4: Run** `cargo test --lib theme::` and `cargo test --lib unlock_prompt`. Expect PASS, including the window-icon-vs-`.ico` test.
- [ ] **Step 5: Commit** "The shield takes its v2 shape, in the colours it already had". Stage both `deskwarden.ico` and `generate-icon.py`.

---

### Task 9: The kit sheet preview, and slice verification

**Files:** Modify `deskwarden/examples/ui_preview.rs`.

**Interfaces:**
- Consumes everything above.
- Produces:
  - A new `Surface::KitSheet` ("kit_sheet", size 1280×900), added to `ALL` and to the `--kit` flag.
  - It draws on `window_bg`, in this order:
    1. Both sidebar palettes, as two 252×300 strips: mark at `mark_on_sidebar(q1)`, "Deskwarden" in `WORDMARK`, a "New item" button in `new_bg`/`new_fg`, a `SECTION` label "Vault", three nav rows (one active) with `ALL`, `FAV` and `LOGIN` glyphs and counts, and a "Lock" row.
    2. The mark ramp 48/32/22/14, plus one at `MARK_REVERSED` on a `blue_deep` tile.
    3. Switches on and off, segmented `SEG_SETTINGS` with three options, and the four button kinds at `LARGE`.
    4. One card with two field rows separated by `row_rule`, tags (all three kinds, both sizes).
    5. A `help_dot` with its bubble shown through `tips::paint_help_bubble`.
    6. One hint shown through `tips::paint_hint`.
    7. A toast forced at full progress.

- [ ] **Step 1: Run** `cargo run --release --example ui_preview -j 2 -- --kit`. Open `target/ui_preview/kit_sheet.png`. Compare it side by side with the design (`docs/design/Deskwarden v2.dc.html`, artboards 1a and 1c) for colours, radii, type sizes and the sidebar palettes. Fix any mismatch in the owning task's module and re-run.
- [ ] **Step 2: Run** `cargo run --release --example ui_preview -j 2 -- --all`. Every surface renders. The existing PNGs differ only where the mark appears.
- [ ] **Step 3: Slice verification:**
  - `cargo test --lib --bins`: all pass (the lib count was 5,902 before this slice, plus the new tests; bins 390).
  - `cargo check --all-targets`: 0 warnings.
  - `cargo deny check`: ok.
  - A bare-LF check on every `.rs` file touched gives 0.
- [ ] **Step 4: Commit** "A sheet of the v2 kit, to hold beside the design". Send `kit_sheet.png` to the owner.

# App Account Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `CTRL+ALT+B` finds no configured match, offer a bare-Win32 card listing the vault items that plausibly belong to the foreground app, and let the user choose which field is typed.

**Architecture:** A pure candidate-matching function over the vault snapshot, a reusable GDI owner-draw module, and a two-step prompt built on the `PromptCalls` `fn`-pointer idiom that `src/unlock_prompt.rs` established — Win32 calls behind function pointers, the whole decision in a `run_with` a test can drive with no desktop. Design 3a moves out of egui in the last task, so the card exists in exactly one renderer.

**Tech Stack:** Rust, `windows` crate 0.58 (GDI only — no Direct2D, no D3D), the existing `key_sequence`, `injector::sequence` and `favicon` modules.

## Global Constraints

- **GDI only.** Direct2D was measured at 53.85 MB against GDI's 1.79 MB and is rejected; see the spec. Nothing in this feature may create a D3D device.
- **No `cfg(test)` seams.** Banned crate-wide. Use the `fn`-pointer seam idiom (`PromptCalls`/`VaultFrameEnv`).
- **No test may touch** the network, the real vault, the real clipboard, the real screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`.
- **Never build into `deskwarden/target`.** Use an absolute `CARGO_TARGET_DIR` outside the repo, e.g. `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **Never write scratch files under `deskwarden/src/**`.**
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, never `--amend`, `reset`, `rebase`, or `git stash`.
- **The overlay has no `ScrollArea` and a bounded height.** A control past the bottom edge is unreachable.
- **`All` is `{USERNAME}{TAB}{PASSWORD}` with no trailing Enter.**
- **Usernames are shown in full. Passwords are never shown.**
- Branch: `app-account-picker`. Build/test command: `CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2`

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/app_candidates.rs` (create) | Pure: foreground window + items → ranked candidates. No Win32, no I/O. |
| `deskwarden/src/win32_draw.rs` (create) | Reusable GDI owner-draw: a themed button and a themed list row. Shared by `unlock_prompt` and `picker_prompt`. |
| `deskwarden/src/picker_prompt.rs` (create) | The two-step card: `PickerCalls` seam, `run_with` decision, `ask` entry point. |
| `deskwarden/src/unlock_prompt.rs` (modify) | Adopt `win32_draw` for its Cancel button and ✕. |
| `deskwarden/src/overlay_ui.rs` (modify) | Delete the egui no-match card (3a). |
| `deskwarden/src/app.rs` (modify) | Route the no-match arm to `picker_prompt` instead of the egui overlay. |
| `deskwarden/src/lib.rs` (modify) | Declare the three new modules. |
| `deskwarden/examples/picker_preview.rs` (create) | Look at and screenshot the card without a vault. |

---

### Task 1: Candidate matching

**Files:**
- Create: `deskwarden/src/app_candidates.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Consumes: `crate::vault_bridge::VaultItem` (fields `id: Option<String>`, `name: Option<String>`, `login: Option<LoginData>`; `LoginData.username: Option<String>`, `LoginData.uris: Vec<UriEntry>`, `UriEntry.uri: Option<String>`), `crate::favicon::domain_from_uri(uri: &str) -> Option<String>`.
- Produces: `pub struct Candidate { pub id: String, pub name: String, pub username: String }` and `pub fn candidates(exe_name: &str, title: &str, items: &[VaultItem]) -> Vec<Candidate>`.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/app_candidates.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::{LoginData, UriEntry, VaultItem};

    fn item(id: &str, name: &str, user: &str, uri: Option<&str>) -> VaultItem {
        VaultItem {
            id: Some(id.to_string()),
            name: Some(name.to_string()),
            login: Some(LoginData {
                username: Some(user.to_string()),
                uris: uri
                    .map(|u| vec![UriEntry { uri: Some(u.to_string()), ..Default::default() }])
                    .unwrap_or_default(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_uri_host_matching_the_exe_stem_outranks_a_name_match() {
        let items = vec![
            item("n", "Slack notes", "notes@example.com", None),
            item("u", "Work chat", "me@example.com", Some("https://slack.com/login")),
        ];
        let found = candidates("slack.exe", "", &items);
        assert_eq!(
            found.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["u", "n"],
            "the URI host is the strongest signal and must sort first"
        );
    }

    #[test]
    fn an_item_with_no_connection_to_the_app_is_not_a_candidate() {
        let items = vec![item("x", "Electricity bill", "me@example.com", Some("https://edf.fr"))];
        assert!(
            candidates("slack.exe", "Slack", &items).is_empty(),
            "a loose matcher that returns everything is the same as no matcher"
        );
    }

    #[test]
    fn a_title_word_matches_when_the_exe_name_does_not() {
        let items = vec![item("t", "Jira", "me@example.com", None)];
        let found = candidates("chrome.exe", "Jira - board - Google Chrome", &items);
        assert_eq!(found.len(), 1, "the title is the only signal a browser window has");
        assert_eq!(found[0].name, "Jira");
    }

    #[test]
    fn short_title_words_do_not_match_because_they_match_everything() {
        let items = vec![item("s", "Amazon", "me@example.com", None)];
        assert!(
            candidates("chrome.exe", "on to a - Google Chrome", &items).is_empty(),
            "two-letter words would make every item a candidate for every window"
        );
    }

    #[test]
    fn an_item_with_no_id_is_skipped_because_nothing_can_be_filled_from_it() {
        let mut orphan = item("ignored", "Slack", "me@example.com", None);
        orphan.id = None;
        assert!(candidates("slack.exe", "", &[orphan]).is_empty());
    }

    #[test]
    fn the_username_is_carried_so_two_accounts_for_one_app_can_be_told_apart() {
        let items = vec![
            item("a", "Slack", "work@example.com", None),
            item("b", "Slack", "home@example.com", None),
        ];
        let found = candidates("slack.exe", "", &items);
        assert_eq!(found.len(), 2);
        let users: Vec<_> = found.iter().map(|c| c.username.as_str()).collect();
        assert!(users.contains(&"work@example.com") && users.contains(&"home@example.com"));
    }
}
```

- [ ] **Step 2: Run the tests and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml app_candidates -j 2
```

Expected: compile error — `cannot find function candidates in this scope`. If it instead reports "module not declared", add `pub mod app_candidates;` to `deskwarden/src/lib.rs` alongside the other module declarations and re-run.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `deskwarden/src/app_candidates.rs`:

```rust
//! Which vault items plausibly belong to the foreground window.
//!
//! **Deliberately looser than [`crate::match_engine::MatchEngine`], and
//! deliberately separate from it.** The engine answers one item or nothing,
//! from an `AppMatch` the user configured; that is what a *fill* is allowed to
//! act on unattended. This answers a ranked list from guesses, and nothing it
//! returns is ever typed without the user picking it -- which is what makes a
//! loose matcher safe here and would make it dangerous there.
//!
//! Pure, and takes `&[VaultItem]` rather than a cache, so the whole of it is
//! testable with fixtures and no window, no vault and no clock.

use crate::vault_bridge::VaultItem;

/// One row of the picker. **Display strings and an id -- never a password.**
/// The secret is fetched at dispatch by the component that already holds it;
/// a copy carried here would be a second, non-zeroizing home for it that lived
/// as long as the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub username: String,
}

/// Below this length a title word matches too much to mean anything: "on",
/// "to" and "a" appear in most window titles and most item names.
const MIN_TITLE_WORD: usize = 4;

/// Strongest first. The number is a sort key, not a confidence score, and
/// nothing downstream may treat it as one.
const RANK_URI_HOST: u8 = 0;
const RANK_NAME: u8 = 1;
const RANK_TITLE: u8 = 2;

/// The exe name without its extension, lowercased: `Slack.exe` -> `slack`.
fn stem(exe_name: &str) -> String {
    exe_name
        .rsplit_once('.')
        .map(|(before, _)| before)
        .unwrap_or(exe_name)
        .to_ascii_lowercase()
}

fn ranked(exe_name: &str, title: &str, item: &VaultItem) -> Option<u8> {
    let stem = stem(exe_name);
    let name = item.name.as_deref().unwrap_or_default().to_ascii_lowercase();

    if !stem.is_empty() {
        if let Some(login) = item.login.as_ref() {
            for entry in &login.uris {
                let Some(uri) = entry.uri.as_deref() else { continue };
                let Some(domain) = crate::favicon::domain_from_uri(uri) else { continue };
                if domain.to_ascii_lowercase().contains(&stem) {
                    return Some(RANK_URI_HOST);
                }
            }
        }
        if !name.is_empty() && name.contains(&stem) {
            return Some(RANK_NAME);
        }
    }

    if !name.is_empty() {
        for word in title.to_ascii_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if word.len() >= MIN_TITLE_WORD && name.contains(word) {
                return Some(RANK_TITLE);
            }
        }
    }

    None
}

/// The ranked candidates for this window, strongest first. Ties keep the
/// vault's own order, so the list is stable between presses.
pub fn candidates(exe_name: &str, title: &str, items: &[VaultItem]) -> Vec<Candidate> {
    let mut scored: Vec<(u8, Candidate)> = Vec::new();
    for item in items {
        // No id, nothing to fill from later: not a candidate, however well it
        // reads.
        let Some(id) = item.id.as_deref().filter(|id| !id.is_empty()) else { continue };
        let Some(rank) = ranked(exe_name, title, item) else { continue };
        scored.push((
            rank,
            Candidate {
                id: id.to_string(),
                name: item.name.clone().unwrap_or_default(),
                username: item
                    .login
                    .as_ref()
                    .and_then(|l| l.username.clone())
                    .unwrap_or_default(),
            },
        ));
    }
    scored.sort_by_key(|(rank, _)| *rank);
    scored.into_iter().map(|(_, c)| c).collect()
}
```

- [ ] **Step 4: Run the tests and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml app_candidates -j 2
```

Expected: `test result: ok. 6 passed`.

If `VaultItem` or `UriEntry` do not implement `Default`, construct them field-by-field in the test helper instead of using `..Default::default()`; read the struct definitions in `deskwarden/src/vault_bridge.rs` and fill every field explicitly.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/app_candidates.rs deskwarden/src/lib.rs && git commit -F msg.txt
```

Message: `Find the vault items that plausibly belong to a window` — with a body saying that this is looser than the match engine on purpose, and that the safety comes from nothing being typed without an explicit choice.

---

### Task 2: A themed button that Windows does not paint

**Files:**
- Create: `deskwarden/src/win32_draw.rs`
- Modify: `deskwarden/src/unlock_prompt.rs`, `deskwarden/src/lib.rs`

**Why first:** the screenshot of the shipped unlock prompt shows a stock grey Cancel beside a correctly drawn Unlock. The picker needs owner-draw for its list rows anyway, so building it once here fixes an existing visible defect and gives Task 3 its foundation.

**Interfaces:**
- Consumes: `crate::theme` constants (`CARD`, `INK`, `TEXT_FAINT`, `BORDER`, `BLUE_BRIGHT`, `BLUE_WASH`, `CARD_TINT`, `TOGGLE_OFF`, `TEXT_GHOST`, `BUTTON_HEIGHT`), `crate::theme::gdi_face_for`.
- Produces: `pub struct ButtonSkin { pub fill: u32, pub text: u32, pub border: Option<u32> }`, `pub fn draw_button(hdc: HDC, rect: RECT, label: &str, font: HFONT, skin: ButtonSkin, radius: i32)`.

- [ ] **Step 1: Write the failing test**

Owner-draw cannot be asserted pixel-by-pixel without a desktop, so test the part that *decides*: which skin a button gets. Add to `deskwarden/src/win32_draw.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_and_secondary_skins_differ_in_every_channel_that_matters() {
        let primary = ButtonSkin::primary();
        let secondary = ButtonSkin::secondary();
        assert_ne!(primary.fill, secondary.fill, "a secondary button must not look primary");
        assert_ne!(primary.text, secondary.text, "white-on-white would be invisible");
        assert!(
            secondary.border.is_some(),
            "the secondary button has no fill contrast against the card, so it needs a border to \
             read as a button at all -- this is the defect the stock Cancel had"
        );
        assert!(primary.border.is_none(), "a filled button does not need one");
    }

    #[test]
    fn a_disabled_skin_is_derived_and_not_a_fourth_hand_picked_palette() {
        let disabled = ButtonSkin::primary().disabled();
        assert_ne!(disabled.fill, ButtonSkin::primary().fill);
        assert_eq!(
            disabled.border,
            ButtonSkin::primary().border,
            "disabling changes colour, not shape"
        );
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml win32_draw -j 2
```

Expected: `cannot find type ButtonSkin`.

- [ ] **Step 3: Write the implementation**

```rust
//! GDI owner-draw shared by the daemon's windows.
//!
//! **Why this file exists.** A control Windows paints does not match the
//! design: the unlock prompt shipped with a stock grey `Cancel` -- system
//! font, square corners, gradient fill -- beside a correctly drawn `Unlock`.
//! Owner-draw (`WM_DRAWITEM`) is the fix, and both the button and the
//! picker's list rows need it, so it lives here rather than twice.
//!
//! Every colour and dimension comes from [`crate::theme`], the same module
//! egui reads, so a theme change moves both renderers at once.

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{HDC, HFONT};

/// How one button is painted. Three colours and a radius, so a new kind of
/// button is a new constructor here rather than new drawing code at a call
/// site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonSkin {
    pub fill: u32,
    pub text: u32,
    pub border: Option<u32>,
}

impl ButtonSkin {
    /// The blue call-to-action.
    pub fn primary() -> Self {
        Self { fill: crate::theme::BLUE_BRIGHT, text: crate::theme::CARD, border: None }
    }

    /// The quiet one beside it. **Bordered on purpose**: it is card-coloured
    /// on a card, so without an outline it does not read as a control.
    pub fn secondary() -> Self {
        Self {
            fill: crate::theme::CARD,
            text: crate::theme::INK,
            border: Some(crate::theme::BORDER),
        }
    }

    /// Greyed, derived rather than hand-picked, so a palette change cannot
    /// leave the disabled variant behind.
    pub fn disabled(self) -> Self {
        Self { fill: crate::theme::TOGGLE_OFF, text: crate::theme::TEXT_GHOST, ..self }
    }
}
```

Then the painter, which takes an HDC and paints one button. Model it on the drawing already in `deskwarden/src/unlock_prompt.rs`'s `win32` module — it already has `RoundRect`, pen/brush selection and `DrawTextW` with the right flags; move that code here rather than writing a second version:

```rust
/// Paint one button into `hdc`. `radius` is the corner radius in pixels;
/// the unlock prompt already picks one for its buttons -- read it out of `unlock_prompt.rs` and use the same value.
///
/// `RoundRect` does not antialias, so the corners are hard. That is the
/// accepted cost of GDI: Direct2D would smooth them and was measured at
/// 53.85 MB against this window's 1.79 MB.
pub fn draw_button(
    hdc: HDC,
    rect: RECT,
    label: &str,
    font: HFONT,
    skin: ButtonSkin,
    radius: i32,
) {
    // Implementation: select a solid brush of `skin.fill` and either a pen of
    // `skin.border` or a null pen, call RoundRect over `rect` with
    // `radius * 2` for both ellipse axes, then SetBkMode(TRANSPARENT),
    // SetTextColor(skin.text), select `font` and DrawTextW the label with
    // DT_CENTER | DT_VCENTER | DT_SINGLELINE. Restore and delete every GDI
    // object created here before returning -- a leaked HBRUSH in a repaint
    // path exhausts the handle table over a long-running daemon.
}
```

Fill that body by moving the equivalent code out of `unlock_prompt.rs`. Do not leave the comment as the body.

- [ ] **Step 4: Adopt it in the unlock prompt**

In `deskwarden/src/unlock_prompt.rs`, change the `Cancel` control from a stock `BUTTON` to an owner-drawn one: add `BS_OWNERDRAW` to its style, handle `WM_DRAWITEM` in the window procedure, and call `win32_draw::draw_button` with `ButtonSkin::secondary()`. Do the same for `Unlock` with `ButtonSkin::primary()` so the two go through one path.

- [ ] **Step 5: Run the whole suite, then look at it**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2
CARGO_TARGET_DIR=/e/_dw_agent/run cargo run --manifest-path deskwarden/Cargo.toml --example unlock_prompt_preview -- --capturable
```

Expected: tests pass, and Cancel now matches Unlock — same font, same corner radius, no grey gradient. **Look at it before continuing**; this is the task whose result only a person can check.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/win32_draw.rs deskwarden/src/unlock_prompt.rs deskwarden/src/lib.rs && git commit -F msg.txt
```

---

### Task 3: The owner-drawn candidate list

**Files:**
- Modify: `deskwarden/src/win32_draw.rs`

**Interfaces:**
- Consumes: `ButtonSkin` from Task 2, `crate::app_candidates::Candidate` from Task 1.
- Produces: `pub struct RowState { pub selected: bool, pub hovered: bool }`, `pub fn draw_row(hdc: HDC, rect: RECT, candidate: &Candidate, state: RowState, name_font: HFONT, user_font: HFONT)`, and `pub fn visible_rows(total: usize, cap: usize) -> (usize, bool)`.

- [ ] **Step 1: Write the failing test**

The paintable part needs a desktop; the *decision* about truncation does not, and it is the one with a defect class attached. Add to `win32_draw.rs`'s test module:

```rust
#[test]
fn a_list_that_fits_shows_everything_and_offers_no_overflow_row() {
    assert_eq!(visible_rows(3, 5), (3, false));
    assert_eq!(visible_rows(5, 5), (5, false), "exactly full is not overflowing");
}

#[test]
fn a_list_that_overflows_gives_up_a_row_to_say_so() {
    let (shown, overflow) = visible_rows(9, 5);
    assert!(overflow, "the user must be told the list was cut");
    assert_eq!(
        shown, 4,
        "the overflow row occupies one of the cap's slots -- showing 5 candidates AND an \
         overflow row would be 6 rows in a window sized for 5, and the last one is unreachable"
    );
}

#[test]
fn a_cap_of_one_still_leaves_room_to_say_there_is_more() {
    assert_eq!(visible_rows(4, 1), (0, true));
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml win32_draw -j 2
```

Expected: `cannot find function visible_rows`.

- [ ] **Step 3: Implement**

```rust
/// How many candidate rows to draw, and whether an overflow row is needed.
///
/// **A cap that hides candidates without saying so is the defect this project
/// keeps finding.** When there are more candidates than fit, one slot is spent
/// on a *Search vault* row so the truncation is visible; that is why the
/// overflowing case shows `cap - 1` and not `cap`.
pub fn visible_rows(total: usize, cap: usize) -> (usize, bool) {
    if total <= cap {
        (total, false)
    } else {
        (cap.saturating_sub(1), true)
    }
}

/// Whether a row is under the pointer, selected, both or neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowState {
    pub selected: bool,
    pub hovered: bool,
}
```

Then `draw_row`, painting: the row background (`theme::CARD`, or `theme::CARD_TINT` when `hovered`, or `theme::BLUE_WASH` when `selected`), the item name in `name_font`/`theme::INK`, and the username beneath in `user_font`/`theme::TEXT_FAINT`. **The hover background must span the full row width, edge to edge** — a hover area that hugs the text was reported as a defect on the vault window's menu and must not be repeated here. Leave a square gutter on the left the height of the row for the icon; Task 5 fills it.

- [ ] **Step 4: Run the tests**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml win32_draw -j 2
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/win32_draw.rs && git commit -F msg.txt
```

---

### Task 4: The two-step decision

**Files:**
- Create: `deskwarden/src/picker_prompt.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Consumes: `crate::app_candidates::Candidate`, `crate::key_sequence::{FieldRef, field_palette}`, `crate::win32_draw::visible_rows`.
- Produces:
  - `pub enum Send { Field(FieldRef), All, Sequence }`
  - `pub enum Outcome { Fill { id: String, send: Send }, NewLogin, SearchVault, Edit(String), Cancelled, Unavailable }`
  - `pub enum Event { Cancel, Closed, Chose(usize), Overflow, NewLogin, EditSelected, Sends(Send) }`
  - `pub struct PickerCalls { pub open: fn(&[Candidate]) -> Option<PickerWindow>, pub protect: fn(PickerWindow) -> bool, pub next: fn(PickerWindow) -> Event, pub show_palette: fn(PickerWindow, &[FieldRef], bool), pub close: fn(PickerWindow) }`
  - `pub fn run_with(calls: &PickerCalls, candidates: &[Candidate], palette: fn(&str) -> (Vec<FieldRef>, bool)) -> Outcome`
  - `pub fn tokens_for(send: &Send, sequence: Option<&str>) -> Vec<crate::key_sequence::Token>`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_sequence::{FieldRef, Token};

    fn one(name: &str) -> Vec<Candidate> {
        vec![Candidate {
            id: "id-1".to_string(),
            name: name.to_string(),
            username: "me@example.com".to_string(),
        }]
    }

    #[test]
    fn all_types_username_tab_password_and_never_presses_enter() {
        let tokens = tokens_for(&Send::All, None);
        assert_eq!(
            tokens,
            vec![
                Token::Field(FieldRef::Username),
                Token::Key("TAB".to_string()),
                Token::Field(FieldRef::Password),
            ],
            "a trailing Enter submits, and if the target's field order differs from this \
             assumption it submits the wrong content -- typing without submitting fails \
             visibly, submitting fails invisibly"
        );
    }

    #[test]
    fn one_field_is_one_token_and_nothing_else() {
        assert_eq!(
            tokens_for(&Send::Field(FieldRef::Totp), None),
            vec![Token::Field(FieldRef::Totp)]
        );
    }

    #[test]
    fn the_sequence_choice_runs_the_items_own_sequence() {
        let tokens = tokens_for(&Send::Sequence, Some("{USERNAME}{TAB}{PASSWORD}{ENTER}"));
        assert_eq!(
            tokens,
            crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}{ENTER}"),
            "the configured sequence goes through the existing parser, not a second \
             interpretation of the same string"
        );
    }

    #[test]
    fn choosing_a_row_then_a_field_answers_that_item_and_that_field() {
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static STEP: AtomicUsize = AtomicUsize::new(0);
                match STEP.fetch_add(1, Ordering::SeqCst) {
                    0 => Event::Chose(0),
                    _ => Event::Sends(Send::Field(FieldRef::Password)),
                }
            },
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        let outcome = run_with(&calls, &one("Slack"), |_| (vec![FieldRef::Password], false));
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
    }

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                PUMPED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                Event::Cancel
            },
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        let _ = run_with(&calls, &one("Slack"), |_| (vec![], false));
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "a window that can be typed into before it is excluded from capture is a window a \
             recorder can catch a keystroke in"
        );
    }

    #[test]
    fn closing_the_window_closes_it_and_fills_nothing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CLOSED: AtomicBool = AtomicBool::new(false);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| Event::Closed,
            show_palette: |_, _, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        assert_eq!(run_with(&calls, &one("Slack"), |_| (vec![], false)), Outcome::Cancelled);
        assert!(CLOSED.load(Ordering::SeqCst), "close runs on every exit path");
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = PickerCalls {
            open: |_| None,
            protect: |_| true,
            next: |_| Event::Cancel,
            show_palette: |_, _, _| {},
            close: |_| {},
        };
        assert_eq!(run_with(&calls, &one("Slack"), |_| (vec![], false)), Outcome::Unavailable);
    }
}
```

- [ ] **Step 2: Run and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml picker_prompt -j 2
```

Expected: `cannot find type PickerCalls`.

- [ ] **Step 3: Implement the decision**

Write `run_with` following `unlock_prompt::run_with` exactly: `open`, then `protect` **before** the first `next`, then a loop over `next`, with `close` on every exit path including the failures. Selecting a row calls the `palette` function for that id and then `show_palette`; a `Sends` event answers `Outcome::Fill`.

`tokens_for` maps the choice to tokens:

```rust
/// What each choice types.
///
/// `All` is `{USERNAME}{TAB}{PASSWORD}` **with no trailing Enter** -- see the
/// test, which carries the reasoning. `Sequence` goes through
/// [`crate::key_sequence::parse`] rather than a second reading of the same
/// string, so the picker and the sequence editor can never disagree about what
/// a sequence means.
pub fn tokens_for(send: &Send, sequence: Option<&str>) -> Vec<crate::key_sequence::Token> {
    use crate::key_sequence::{FieldRef, Token};
    match send {
        Send::Field(field) => vec![Token::Field(field.clone())],
        Send::All => vec![
            Token::Field(FieldRef::Username),
            Token::Key("TAB".to_string()),
            Token::Field(FieldRef::Password),
        ],
        Send::Sequence => sequence.map(crate::key_sequence::parse).unwrap_or_default(),
    }
}
```

If `Token`'s variants are named differently in `deskwarden/src/key_sequence.rs` (read the `pub enum Token` at line 230), use the real names and adjust the tests to match — the tests assert behaviour, not spelling.

- [ ] **Step 4: Run and watch them pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml picker_prompt -j 2
```

Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/picker_prompt.rs deskwarden/src/lib.rs && git commit -F msg.txt
```

---

### Task 5: The window, and wiring it to the daemon

**Files:**
- Modify: `deskwarden/src/picker_prompt.rs`, `deskwarden/src/app.rs`
- Create: `deskwarden/examples/picker_preview.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–4, `crate::injector::sequence::plan`, `crate::injector` `fill_sequence`.
- Produces: `pub fn ask(candidates: &[Candidate]) -> Outcome` and `pub static REAL: PickerCalls`.

- [ ] **Step 1: Build the Win32 half**

Write the `win32` submodule of `picker_prompt.rs` modelled on `unlock_prompt.rs`'s: register a class, create a `WS_POPUP` window sized for `visible_rows`' answer, `AddFontMemResourceEx` the Archivo faces via the same helper, an `LBS_OWNERDRAWFIXED` listbox for the rows, and `WM_DRAWITEM` dispatching to `win32_draw::draw_row`. Icons come from the existing favicon cache — read how `deskwarden/src/picker_ui.rs` loads them and reuse that path rather than fetching anything here.

Then `REAL` and `ask`, matching `unlock_prompt::REAL`/`ask` in shape.

- [ ] **Step 2: Add the preview and look at it**

Create `deskwarden/examples/picker_preview.rs` modelled on `deskwarden/examples/unlock_prompt_preview.rs`: real Win32 calls, fixture candidates, `--capturable` stubbing `protect` so the window can be screenshotted.

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo run --manifest-path deskwarden/Cargo.toml --example picker_preview -- --capturable
```

Expected: a card listing the fixture accounts, rows highlighting edge-to-edge on hover, and the palette appearing on selection. **Look at it.**

- [ ] **Step 3: Wire the no-match arm**

In `deskwarden/src/app.rs`, the arm that today calls `PromptPresenter::show_no_match` gathers candidates first:

- `app_candidates::candidates(&event.exe_name, &event.title, &items)`
- empty → the existing no-match behaviour, unchanged
- non-empty → `picker_prompt::ask`, then map `Outcome`: `Fill` builds tokens with `tokens_for`, resolves them against the item, calls `injector::sequence::plan`, and dispatches through the existing `fill_sequence` — **keeping its foreground check**; `SearchVault`/`NewLogin`/`Edit` reuse the existing follow-ups.

- [ ] **Step 4: Run the whole suite**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2
```

Expected: 0 failed. Fix anything red before continuing.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/picker_prompt.rs deskwarden/src/app.rs deskwarden/examples/picker_preview.rs && git commit -F msg.txt
```

---

### Task 6: Retire the egui no-match card

**Files:**
- Modify: `deskwarden/src/overlay_ui.rs`, `deskwarden/src/app.rs`, `deskwarden/examples/ui_preview.rs`

**Why last:** until Task 5 is on screen and working, the egui card is the only one there is. Deleting it earlier leaves no no-match surface at all.

- [ ] **Step 1: Delete `show_no_match_overlay` and its card**

Remove `overlay_ui::show_no_match_overlay` (line 257) and the drawing it owns. Remove `NoMatchAnswer::SearchVault` and `::NewLogin` handling from the egui side only where it is now unreachable — `NoMatchAnswer` itself stays if 3c still uses it.

- [ ] **Step 2: Remove the surface from the egui preview**

In `deskwarden/examples/ui_preview.rs`, delete the `Surface::OverlayNoMatch` variant and its arm. **Update the PNG-count check in `.github/workflows/ci.yml`** — it asserts nine files, and there will now be eight. A CI job that still expects nine fails on the next push.

- [ ] **Step 3: Prove the old card is gone**

```bash
grep -rn "show_no_match_overlay\|OverlayNoMatch" deskwarden/src deskwarden/examples
```

Expected: no matches. If any remain, the surface exists in two renderers, which is the defect this task exists to prevent.

- [ ] **Step 4: Run everything**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2
CARGO_TARGET_DIR=/e/_dw_agent/run cargo run --manifest-path deskwarden/Cargo.toml --example ui_preview -- --all
```

Expected: tests pass; the preview writes eight PNGs.

- [ ] **Step 5: Measure, because the number is the point**

Run the daemon, press `CTRL+ALT+B` on an unbound app, and with the picker on screen:

```bash
powershell -NoProfile -Command "Get-Process deskwarden | Select-Object Id,@{n='MB';e={[math]::Round($_.PrivateMemorySize64/1MB,2)}}"
```

Expected: single-digit MB. **If this comes back at 40 MB or more, something in this feature created a GPU device** — find it before merging; that outcome makes the whole approach pointless.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/overlay_ui.rs deskwarden/src/app.rs deskwarden/examples/ui_preview.rs .github/workflows/ci.yml && git commit -F msg.txt
```

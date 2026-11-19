# Slice 10: lock, sign-in, signed out

Spec: `docs/design/deskwarden-v2-design-spec.md` part C section 5.2 (sign-in split screen), 5.3 (lock screen), with 5.1 (toast, already `kit::toast`) and 5.4 (loading, already the splash) for context. Rules in section 0 apply: nothing the app does today is dropped, every error and edge state is kept and restyled, no authentication behaviour or server call changes.

## What draws what today

| Surface | Where | Action |
|---|---|---|
| Sign-in (Unauthenticated) and lock (Locked/Unlocked), first window and the in-window lock | `login_ui::build_login_frame` and `draw_login_window`: a card on a dimmed vault skeleton | Replaced by the v2 split screen and the navy lock screen |
| Second-factor stage | `second_factor_ui::draw`, drawn raw in `app_window` with no backdrop and no window controls | Drawn on the split screen, with caption buttons |
| API-key stage | `api_key_ui::draw`, same | Same |
| CLI-setup / which-client modal | `login_ui::draw_cli_setup_modal`, takes the card | Takes the form column, with kit buttons |
| First-window Slow and Unreachable bodies | `loading_ui::draw_first_window_body` | Restyled with kit buttons and caution tokens |
| `unlock_prompt.rs`, `locked_card.rs` | Bare Win32 cards for autofill | Not touched (slice 11) |
| `reprompt.rs`, `hello.rs`, `accounts.rs` | Logic, no egui | Not touched |

Account switching has no surface in these windows (it is the sidebar account menu, slice 2); the sign-in window is opened per account by the callers and that is unchanged.

## Design

- New `kit::auth`: the two full-window surfaces and their parts. `split` (white window, navy brand half left, 420-wide form column right; below 760 wide the brand half is dropped and the form takes the window), `lock_surface` (navy, centred column), `caption_buttons` (the design's 46x34 light or dark controls, with a drag band), `brand_panel`, the Windows Hello button, the lock link, the lock Unlock button, the error line.
- `kit::input` gains `secret` (a password field with an eye, at the sign-in and lock sizes), and the greyed `text_disabled` / `secret_disabled` the in-flight state needs.
- `kit::icon::glyph::FINGERPRINT` (Lucide `fingerprint`, ISC).
- `LoginForm` gains `lock_stage` (Hello or Password). The Hello stage is shown only when Hello is available and enrolled; otherwise the password stage is the only one.
- `login_ui::draw_login_window` keeps its signature and its `LoginAction`s. Sign-in draws the form column; lock draws the header, stage and footer. `build_login_frame` paints the surface instead of the skeleton and card.
- The vault skeleton, the card geometry and the brand lockup are deleted with their tests, replaced by tests of the new surfaces.

## Tasks

1. **Kit parts.** `FINGERPRINT`, `input::secret` / disabled fields, `kit::auth` surfaces. Tests: the split's halves and form width, compact fallback, caption actions, lock stage widgets, hello button label states.
2. **Sign-in screen.** Restyle `draw_login_window` (Unauthenticated), surface in `build_login_frame`, server segmented with a URL field for Self-hosted, API-key link, Hello opt-in button, first-run notice, CLI-setup modal. Update the `auth_in_flight_tests` harness to the v2 field sizes.
3. **Lock screen.** Hello and password stages, in-flight "Checking…", Log out, server line, errors.
4. **Second-factor and API-key stages** on the split screen.
5. **Slow and Unreachable bodies** restyled.
6. **Previews** in `examples/ui_preview.rs` (not run).

## Rulings

(filled in as the slice lands)

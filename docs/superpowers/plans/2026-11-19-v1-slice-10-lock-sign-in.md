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

Ruling: "Log in with another device" is not drawn -- the design gives it the same handler as Log in and Deskwarden has no device flow; the Bitwarden API has none for a desktop client either -- cost: none, a button that could not be honoured is absent.
Ruling: the sign-in screen keeps "Sign in with an API key" as a link under the form -- nothing 0.15 does may be dropped and Duo/WebAuthn accounts have no other door -- cost: one extra line the design lacks.
Ruling: the Hello opt-in on sign-in is an outline button "Log in and set up Windows Hello" (fingerprint glyph where the design's phone icon sits) with 0.15's "First use" line under it -- it has real behaviour (seals the typed password) -- cost: low.
Ruling: the lock heading reads "Welcome back" and the account email sits under the sub line, not "Welcome back, Alex" -- the app knows the email, not a first name (`bw status` carries none) -- cost: copy differs from the mock by the name.
Ruling: the lock screen shows the Hello stage only when Hello is available and enrolled; with it off, unavailable or deleted after a failed open the password stage is the only one (spec C 5.3 says the app should) -- cost: none.
Ruling: errors on the navy lock screen sit on a white chip in `ERROR` -- red on navy is unreadable and the spec says errors use `ERROR` -- cost: a chip the design lacks.
Ruling: the server choice is a segmented control bitwarden.com / EU / Self-hosted with a URL field above Email for Self-hosted, as the spec asks; the old footer dropdown and its scroll-area guard are gone -- cost: none.
Ruling: below 760 px wide the split drops its brand half and the form takes the window -- the design is drawn at 1280 and the sign-in window opens at the vault window's saved size, which can be smaller -- cost: a narrow window shows no brand copy.
Ruling: the vault skeleton, the card geometry and the brand lockup are deleted with their tests; `cli_setup_modal_layout_tests` is rewritten to drive the split surface (its card-growth loop no longer exists) and a source guard on the old server picker is removed; the module-count guard drops from 19 to 18 -- cost: those guards' history is gone, their intent is kept.
Ruling: the second-factor and API-key stages now draw their own split screen and caption buttons -- they had no backdrop and no window controls, so the window could not be minimised or closed there -- cost: Close is live on those stages (it was unreachable before), and nothing in `app_window` changed.
Ruling: sign-in and lock parts rise and fade in (`dw-up`, `dw-fade`); a fully transparent first frame paints nothing, so the tests of the second-factor and API-key cards now run a dozen frames before reading -- cost: none at runtime.
Ruling: the Slow and Unreachable first-window bodies are restyled with kit buttons (the amber badge already equals the caution tokens) but keep 0.15's layout, frame and footer -- the design has no screen for them -- cost: they are not full-bleed.

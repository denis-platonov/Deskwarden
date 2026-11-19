# Slice 15a: sign-in, unlock and first-window screens against the sign-in gallery

Spec: `docs/design/Deskwarden v2.dc.html` (the `sg` sign-in gallery: form, banner, busy, email / password / server-URL / code errors, self-hosted, 2FA code, new-device approval, too many tries, done, locked) and `docs/design/deskwarden-v2-design-spec.md` part C 5.2-5.4. Slice 10 built the split screen and the lock screen; this slice brings every state of them to the gallery's detail and adds a preview for each.

## Method

Every screen is rendered with `cargo run --example ui_preview -- --all` before and after, and the PNGs are compared with the design. A screen is only called matching after its PNG was looked at.

## Gap list (design against 0.x/slice 10)

| Design | Today | Action |
|---|---|---|
| Title block: "Log in" 32 + "Use your Bitwarden account" 16 grey | title only | add subtitle |
| Field boxes 1.5 border, red border + `#fff8f7` wash + icon error line under the field | one red line at the foot of the form | per-field error state; the one `form.error` string is routed to the field it is about |
| Password label row with "Forgot?" link | none | drawn; opens the web vault's hint page (`<server>/#/hint`), which is how every Bitwarden client does it |
| Server segmented, then (self-hosted) `https://` URL field below it | URL field above Email | move under the segments, with the prefix and error |
| Offline / server banner above the form (amber / red, icon, title, text, button) | red line | banner |
| Log in button busy: "Checking" with a 3 px bar inside | button disabled + bar under it | in-button bar |
| 2FA: tile, "Enter your 6-digit code", six digit boxes, error line, remember switch, Continue, Back / Use another method | title, text field, buttons stacked | six boxes for numeric methods, links row |
| Device approval | none | see Ruling |
| Too many tries panel | red line | panel when the server said so |
| Done "You're in" | none | drawn between success and the splash |
| Lock: shield, heading, Hello button / password row + Unlock, link | done in slice 10 | audit only; errors keep the chip |

## Tasks

1. Kit parts in `kit/auth.rs`: subtitle, field with error state (text and secret), error line with icon, banner, in-button bar, digit boxes, stage tile, info panel, done stage. Tests per part.
2. Sign-in form states in `login_ui.rs`: routing of the error string to a spot (`ErrorSpot`), validation messages in the design's words, banner, busy, Forgot.
3. Second factor in `second_factor_ui.rs`, API key in `api_key_ui.rs` restyled to the same parts.
4. Too many tries and done stages.
5. First window: splash and the Slow / Unreachable bodies audited against 5.4.
6. Previews for each state, before/after PNGs read.

## Rulings

Ruling: the "Approve on your phone" (new device) stage and its "Log in with another device" door are not drawn -- Bitwarden's login-with-device is an auth-request flow (an RSA key pair, `/auth-requests`, polling for the approver's answer) that neither the `bw` CLI nor Deskwarden's direct-REST login speaks, so the state cannot be reached and a button for it would not be honoured (slice 10's ruling stands) -- cost: the gallery's "Another device" case has no counterpart; add it with the flow.
Ruling: the 2FA stage has no "Don't ask again on this PC for 30 days" switch -- the sign-in sends no `twoFactorRemember` and stores no remember-token, so a switch would claim something nothing does -- cost: one row the gallery has; it arrives with the remember-token support (a Bitwarden API feature, no Deskwarden-only data).
Ruling: "Too many tries" is drawn when the server says so, but with "a few minutes" where the gallery counts down 4:59, and a "Try again" button where it has "Log in with another device" -- the server's refusal names no time, and a made-up timer would lock out someone the server would already let in -- cost: no countdown.
Ruling: a wrong password shows the CLI/server's sentence ("That email or master password didn't work. Check them and try again.") and not "Wrong password. 2 tries left." -- no Bitwarden server reports a count -- cost: copy differs.
Ruling: a message reaches its field by reading the `LoginForm::error` string (`error_spot`) rather than the field becoming an enum -- six call sites and the tests that pin those strings keep working unchanged, and the strings that decide a spot are constants -- cost: a new message with new opening words falls to the general line under Log in until it is added to `error_spot`.
Ruling: the email check is "has an `@`" ("That doesn't look like an email address.") -- every Bitwarden account is an email and the server would refuse anything else anyway -- cost: none.
Ruling: the blank-field messages take the gallery's words ("Enter your email.", "Enter your master password.", "Enter your server's address.") and `unlock_prompt`'s one pinned test literal now names `login_ui::ENTER_PASSWORD` -- the unlock card must say what the window says -- cost: one line in a file slice 11 owns.
Ruling: the unreachable-server banner is amber "You're offline" on bitwarden.com / EU and red "Can't reach your server" on Self-hosted (the gallery's two banners); its button dismisses it and, if the password is still typed, tries again -- the password is wiped on every failure, so in practice it dismisses -- cost: none.
Ruling: "Forgot?" opens `<server>/#/hint` (the web vault's hint page, which every Bitwarden client sends to) in the browser -- the CLI has no hint API and Deskwarden never learns the hint -- cost: none; it is not drawn while an attempt is in flight.
Ruling: "You're in" is shown from the moment `apply_auth_result` accepts a sign-in until the host swaps the window, and no longer -- stretching it to the gallery's pause would delay every sign-in for a picture -- cost: on a fast host it is on screen for a frame or two.
Ruling: the 2FA "Use another method" link moves to the next supported method (wrapping) instead of listing each by name -- the gallery has the one link -- cost: three-method accounts cycle through them.
Ruling: Authenticator copy is the gallery's ("Open your authenticator app and type the code for Bitwarden.") and the two numeric methods share its heading "Enter your 6-digit code"; Continue with fewer than six digits says "Enter all 6 digits." instead of being greyed -- the gallery's own behaviour -- cost: none. A YubiKey keeps one field (its code is 44 characters, not six digits).
Ruling: the API-key stage, which the gallery does not draw, is restyled with the same parts (30 title, 16 line, 1.5 fields, error under the field, in-button bar) -- cost: none.
Ruling: the error "shake" (`dw-shake`) is not drawn -- a 0.4 s sideways nudge that the fields' red border and the error line already say -- cost: less motion than the gallery.
Ruling: the lock screen and the first-window bodies (splash, Slow, Unreachable, CLI setup, backend choice) were compared with the gallery and the spec and left as they are, apart from the lock field's placeholder now at the field's own size and the CLI modal's copy at 1.45 leading -- they already match 5.3 and 5.4, and the design draws no screen for the others -- cost: none.
Ruling: the sign-in fields moved to `kit::auth::field_text` / `field_secret` (1.5 border, 38 px eye) rather than `kit::input` -- the red state and the `https://` prefix are sign-in-only and `kit::input` is shared -- cost: two small field implementations; `kit::input::secret` (the lock screen's) only gained a placeholder at the field's size.

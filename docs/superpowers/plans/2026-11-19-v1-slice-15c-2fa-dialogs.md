# Slice 15c: the 2FA flow, the small dialogs and the 0.x-looking vault screens

**Why:** the owner: "many small screens are not updated, all small MFA etc". The previews show it: only the Add 2FA picker was restyled (slice 5), its other states (typed, scanned, scanning, webcam, refusal) and most small confirmations still use the 0.x `theme::modal_card` / `movable_modal` frames.

**Source of truth:** `docs/design/Deskwarden v2.dc.html`, `docs/design/deskwarden-v2-design-spec.md` Part A 2.6 (toasts), 3.3 (2FA row), 4 (Shared), Part B 0.2-0.7, 2, 6, 7, 8, Part C 3, 5.5.

**Method:** every screen is looked at in its `ui_preview` PNG before and after (the preview renders on this machine). Only screens whose PNG still looks 0.x are changed; the v2 references are `vault_shell_light.png`, `modal_new_item.png`, `edit_login.png`.

## Tasks

1. **Add 2FA, all states, on `kit::modal::Dialog` (580).** One frame for every stage: title "Add 2FA code" with the 20 px `?` tip below it, sub "For {name}". Picker: the two cards plus the image-file and webcam routes as two quiet rows, the privacy line as a note, a refusal as a red wash. Scanning: the dark 300 px panel with the QR tile, brackets and moving line, caption "Looking for a code…". Webcam: the same dark panel holding the picture. Typed: key field (58, 2 px border, blue when valid), the live-code preview, kit segmented Digits/Period, Back + Save code. Scanned: the "Found it" card (issuer · account, live code, "new code in Ns"), "Does this match the code on the site?", the Issuer/Account/Secret/Parameters table kept as a v2 card, Scan again + Save code. Replace warning as the amber band.
2. **The 0.x modal frames move onto `Dialog`:** discard changes, delete / permanent delete, folder editor, icon picker, record forget / join / leave, launch confirm, sequence builder confirm, rehearsal result, export report / progress / send-delete report.
3. **The 2FA code in the detail pane** becomes the design's row (inside the field card, 26/700 code, ring, copy) and the Add-2FA row when there is no code.
4. **Sends**: the Shared screen's v2 header and card look; the Send detail modal (600) with facts and Activity.
5. **Record composer / share modal**: the notes, passphrase and warning blocks onto v2 tokens.
6. **Previews** for every changed state; PNGs read before and after.

## Tests

Behaviour tests stay; tests that pinned 0.x geometry (header bands, footer bands, keycaps) are rewritten to the v2 numbers or retired with a Ruling.

## Rulings

Ruling: the Add 2FA modal is one `kit::modal::Dialog` (580) at every stage, and the 0.15 stage cards (header bands, footer bands, keycap rows, the 6a-6d names) are gone -- the design has one modal with states, and four differently framed cards was the reason three of its states still looked 0.x -- cost if wrong: the 0.15 pixel pins (about 40 geometry tests) are retired; behaviour tests are kept or rewritten against the new controls.
Ruling: the route keys (Enter, 1-4) still choose a route on the picker but are no longer painted as keycaps -- the v2 cards carry no keycaps -- cost if wrong: a user who learned the keys gets no on-screen hint.
Ruling: the by-hand form's Back and the scanned form's "Scan again" both return to the picker (and drop the seed) -- the design's Scan again restarts a scan, but a code can also arrive from an image or the camera, and the picker is one press from all three -- cost if wrong: one extra click to re-scan the screen.
Ruling: the key field is not upper-cased as the design's CSS does -- it also takes a whole `otpauth://` link, which is case-sensitive -- cost if wrong: lower-case secrets are shown as typed (base32 is case-insensitive, so they still decode).
Ruling: 0.15's other two routes (an image file, a webcam) are two quiet outline rows under the design's two cards, and the camera stage keeps its list of cameras and its picture inside the design's dark panel -- the design draws neither route; none may be dropped -- cost if wrong: a designer may want them as links.
Ruling: the live-code row keeps 0.15's refresh text and a 3 px countdown line along its foot, and the scanned card keeps the Issuer / Account / Secret / Parameters table under "Does this match the code on the site?" -- no 0.x behaviour may go -- cost if wrong: the scanned state is taller than the design's.
Ruling: the dialog scrim now closes the dialog on every converted confirmation (discard, delete, folder, icon, forget, join, leave, launch, reports); 0.15 made the scrim swallow clicks. On the discard dialog the scrim, the close mark and Esc all mean "keep editing" -- spec B 2 says the scrim closes, and the safe answer is what each of those means -- cost if wrong: a click aimed past the dialog now dismisses it.
Ruling: the vault export report, its in-flight card and the revoke banner become v2 cards (480, radius 24, the danger note for a failure, a full-width OK) but stay MODELESS -- `Dialog::modeless()` draws no scrim and takes no outside click or Esc -- because 0.15 tests pin that a report on screen never refuses the next export or revoke ("a previous answer is not a reason to refuse the next backup"); a scrim would have been a regression -- cost if wrong: they float over the window like the toast they replaced and do not dim it.
Ruling: the item list's scrim guard now reads the ids dialogs and `kit::modal::confirm` are opened with (a literal, or a const in the same file) instead of only `Dialog::new(ID,`, and skips a `.modeless()` card (it has no scrim) -- the confirmations are opened with `*_ID` consts -- cost if wrong: a dialog opened with a computed id would be missed.
Ruling: the edit view's 2FA card keeps its 0.15 section shape (title, code row, Authenticator row) and only its code line is set as the design sets it (26/700 `.12em`, blue and red for the last five seconds, a 34 px ring with a 4 px stroke) -- the edit view's cards are slice 4's and the owner names `edit_login.png` as a v2 reference -- cost if wrong: the Authenticator row's Replace/Remove stay the small row buttons.
Ruling: the rehearsal window is set in the v2 type scale (16/14 px, a 42 px Close, radius 20) and is 440 high instead of 400 -- the larger type does not fit 400 -- cost if wrong: a taller scratch window.
Ruling: the Sends screen gets only its empty states restyled (spec 3.1's placeholder). The Shared card grid and the Send detail modal (spec A 4, B 7) are NOT built: send_ui is 16.5k lines of live behaviour (received sends, file sends, scopes, extend, revoke, delete-with-confirm, record tiles) and a faithful rebuild is its own slice -- cost if wrong: the Sends screen is still the two-pane list and detail of slice 3, restyled rows and pills.
Ruling: the `?` tip on the Share modal is now the kit's `help_dot` (hover bubble, spec B 0.4) and Create link, Done and See in Shared fill their width -- they were a bare "?" with a native tooltip and natural-width buttons -- cost if wrong: none.
Ruling: not converted, still `theme::modal_card` / `movable_modal`: the sequence builder's confirm, the app-window picker, the in-form password generator, the record import form, and the people-sharing modals (`record-share`) -- each carries its own form body and pinned geometry; they are listed for the next pass -- cost if wrong: those five still look 0.x.
Ruling: `theme::one_border_is_one_line_tests` now expects at least one repaint-own-border card, not two -- `totp_add`'s `stage_card` was the second and no longer exists -- cost if wrong: none.

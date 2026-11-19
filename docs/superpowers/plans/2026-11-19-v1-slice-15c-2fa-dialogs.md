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

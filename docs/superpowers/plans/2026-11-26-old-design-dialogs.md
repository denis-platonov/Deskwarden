# Old design gone: the stock Windows MessageBox (slice "dialogs")

Goal: no stock Windows `MessageBox` is what a user sees. `main.rs`'s `message_box`
(about 25 callers: 3 Yes/No confirms, the rest OK-only) now shows Deskwarden's own
Win32 card; `MessageBoxW` survives only as the fallback when the card cannot open.

## Tasks

1. `panel_draw`: `PanelButton::Danger` (+ `DANGER_EDGE_SOFT`), the design's outline danger
   button (`#b42318` text, `#f0cfca` edge, hover `#fdf3f2`). Test: skin colours, other skins untouched.
2. `notice_card`: generalise. `Notice { title: String, body, ask: Option<Ask> }`,
   `Notice::info` / `Notice::ask`, `Event::{Accept, Decline}`, `Outcome::{Accepted, Declined}`,
   `show(&Notice) -> Outcome`. Anatomy moved to Part D 1.5 / the `unlock_prompt` reference:
   header (mark + wordmark + X), title 19/800, body 15 `TEXT_FAINT` straight on the card
   (no caution well, no rules, no footer), h46 r13 buttons gap 8, `PAD` 18 / gap 14.
   One full-width OK, or two equal buttons (accept primary or danger, decline outline).
   Tests: layout in every shape, outcomes of every event, key handling (Enter presses the
   focused button, Esc declines, held key does nothing), painted roles, danger skin.
3. `main.rs`: `message_box` -> card via `card_for_style`; `message_box_answer` maps outcome to
   `IDOK/IDYES/IDNO`; `native_message_box` holds the one `MessageBoxW` and is reached only
   from the `Unavailable` arm. Remove-account and reinstall-CLI name their own buttons.
   Tests: the source pin (one `MessageBoxW(` call, in the fallback, reached only from the
   `None` arm), outcome mapping, style -> card (default No survives).
4. Locales: `notice_card.{remove,cancel,yes,no}` in all 36 files, values copied from the
   language's own existing `detail.app_remove`, `unlock_prompt.cancel`, `detail.yes`, `detail.no`.
5. `examples/notice_preview.rs`: renders every variant (`--shots DIR` copies only the card's
   own window rect).

## Rulings

Ruling: a title that is only "Deskwarden" is not painted -- the header's wordmark says it, and the card would read "Deskwarden / Deskwarden" -- every other title (i18n keys unchanged) is painted -- a caller wanting the name twice gets it once.
Ruling: `MAX_BODY_LINES` 8 -> 24 -- the confirm cards carry whole paragraphs (account removal, unrecognised CLI), and cutting a destructive warning at line 8 would hide "This cannot be undone" -- a very tall card on a small screen (24 x 20 = 480 px of body).
Ruling: the missing-CLI ask is labelled OK / Cancel rather than Yes / No -- its body (`cli.body_recover_3`) already says "Press OK ... or Cancel", which the system's Yes/No box contradicted -- none: same two answers, default Cancel.
Ruling: the system's Yes/No box ignored Escape (no Cancel button); the card treats Escape and the X as the negative answer -- the brief asks for it and it is the safe one -- a user who pressed Esc and expected nothing now gets "No" (for the CLI prompt that exits the app, as No did).
Ruling: the MessageBox icons (warning triangle / error cross) are not reproduced; the old yellow caution well is gone -- the new look has no per-severity chrome -- severity is carried by the words.
Ruling: the unrecognised-CLI ask keeps Yes / No (its body ends "Choose No unless...") with Enter on No.
Ruling: the remove-account accept button uses the design's outline danger skin, not a solid red fill -- the design has no solid danger button -- it is less shouty than a filled one.
Ruling: no Y / N accelerators (the system box had them) -- not in the design's cards -- Tab / Space / Enter / Esc still answer.

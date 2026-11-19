# Slice 11: the autofill panel (design 1b)

Design D section 1 (1.2 onward; 1.1 is the demo host window and is context
only) draws the panel the daemon puts under the focused field: **Fill**,
**No match**, **Locked**, **Save login**, **New password**, with key hints and
a done check. This slice restyles the 0.15 fill-time cards to that look.

## Ground rules

- The cards are bare Win32 and stay bare Win32 (an egui window costs the daemon
  ~50 MB for ever). Nothing is ported to egui.
- Triggering, anchoring (`app::overlay_position`, `prompt_card::place`), focus
  and filling do not change. Every state and keyboard behaviour stays.
- What GDI cannot do cheaply (alpha ramps, blur, bezier keyframes, antialiased
  corners) is simplified and recorded as a Ruling below.
- `picker_ui.rs` is the egui *app-binding* picker window, not the fill panel;
  it is untouched.

## Map: design case -> 0.15 card

| Design | Card |
|---|---|
| Fill (account list) | `picker_prompt` list / palette / search; `prompt_card` (one account's ways to type) |
| No match | `picker_prompt` empty mode |
| Locked | `locked_card` |
| Save login | `save_login_card` |
| New password | `generate_prompt` |
| Done check | `notice_card::show_done` (new), hooked after a saved login |

## Tasks

1. **`panel_draw`** (new, `OPENS_NO_WINDOW`): the design's geometry constants,
   the "Deskwarden" lockup, avatar tile, `Enter` badge, pill, `?` badge and dark
   key-hint tooltip, outline/primary/deep button skins, regenerate glyph and the
   done check with its animation as pure functions of elapsed ms.
   Tests: widths/heights equal the spec, badge geometry, tooltip stays inside
   the window, easing/progress monotonic, checkmark path cut by progress.
2. **`prompt_card`**: panel header (lockup, `?`, close), account rows as design
   rows (avatar 42/r12, name 16/700, sub 14, blue `Enter` badge on the selected
   row), `?` hover tooltip, no footer rules.
3. **`picker_prompt`**: the same rows for candidates / fields / search results,
   400 wide; No match as the design's card (title, body, two 46 px buttons).
4. **`locked_card`**: 410 card, `Locked` pill, 19/800 title, 15 body, 54 px deep
   button.
5. **`save_login_card`**: 370 card, "Save this login?", preview block, stacked
   fields, Save / Not now at 46 px, "Never for this app" link.
6. **`generate_prompt`**: 410 card, "Suggested password" over a 21 px mono value,
   strength pill in the header, 46 px primary/outline footer.
7. **`notice_card`**: restyled to the panel look; `show_done` = the done check
   card (green disc, drawn-on check, title, subtitle), non-activating, ~1.6 s.
   Hooked after the save-login card saves.
8. Previews (`examples/*_preview.rs`), CHANGELOG, final gate.

## Rulings

(filled in as the slice lands)
Ruling: Win32 stays -- every card is still bare GDI with its own window procedure; nothing is ported to egui -- the daemon is 10 MB without an egui window and ~60 MB for ever after the first one -- cost if wrong: none, the design is reproduced in GDI.
Ruling: corners are the system's, not 18 px -- `DWMWCP_ROUND` rounds the frameless windows (about 8 px) and GDI `RoundRect` is hard-edged; no `SetWindowRgn` -- a region would be aliased and would clip the DWM shadow -- cost if wrong: panels look slightly less round than the mock.
Ruling: no soft panel shadow, no blur, no opacity ramps (`dw-fade`, `dw-up`, `dw-drop`, `dw-tip`) -- GDI has no alpha or blur and the window's own DWM shadow stands in -- cost if wrong: panels appear instantly instead of dropping in.
Ruling: the done check keeps movement but not the exact curves -- the ring overshoots via easeOutBack (not the mock's `cubic-bezier(.2,.9,.3,1.3)`), the stroke draws on over 500 ms after a 200 ms delay with an ease-out cubic, and there is no ring fade-in -- GDI cannot fade and a bezier solver is not worth it -- cost if wrong: a slightly different overshoot.
Ruling: the done card is a small card of its own (320 wide, centred on the work area), not an overlay over the host window -- a daemon cannot paint over another app's client area, and covering it would block the fill -- cost if wrong: the confirmation is not "inside" the app that was filled.
Ruling: the done card is wired only to a saved login (`DoneCase::Saved` after `SaveOutcome::Created(Ok)`); `Updated` and `SignedIn` copy exists but is not triggered -- a fill is typed asynchronously and may be refused (the notice card says so), so "Signed in" would sometimes be false, and no change-password flow exists to say "Password updated" -- cost if wrong: Fill and New password have no done animation.
Ruling: the done card drops the design's "Filled by Deskwarden in 1.2 s" stopwatch -- nothing times a fill -- cost if wrong: none.
Ruling: the key hints are a `?` badge whose hover shows the dark tooltip, drawn as a hidden disabled child window over the card and placed to the LEFT of the badge on the header -- a child cannot paint outside its parent, the shortest card is ~120 px tall (the tooltip is ~90), and a second topmost window would need its own raise classification -- cost if wrong: the tooltip covers the wordmark while open instead of floating above the card.
Ruling: the tooltip omits the design's `Tab fill and sign in` line -- Tab moves focus on these cards and nothing presses a sign-in button -- and lists the extra keys the picker really answers (digits, S, N, B, Ctrl+digits in search) -- cost if wrong: none.
Ruling: the 0.15 footer hint chips (`ENTER Fill`, `ESC Dismiss`) and the `1 match` count are gone from the matched-item card, and the locked card's `ESC Dismiss` footer too -- design 1b has none of them; Esc, the close glyph and Enter all still work, and the `?` badge lists them -- cost if wrong: Esc is less advertised on Locked and No match.
Ruling: the matched-item card's rows name what will be typed on the bold line and the account on the faint line, with the item/app context line dropped -- the design's rows have two lines and the account must be named on every row -- cost if wrong: the `item · fills app` line is not shown.
Ruling: the picker's rows are 58 px pitch (56 drawn plus the 2 px gap) not the design's 64, with the 42 px tile kept -- the picker lays out six rows at once and 64 px would make a ~620 px card -- cost if wrong: denser than the mock.
Ruling: the picker keeps its "Fill from vault" / subtitle heading pair above the list (design 1b's Fill panel has none) -- the second step and search mode need a title and the heading is what says which step the card is on -- cost if wrong: one extra line of chrome.
Ruling: the picker is 400 wide in every mode, including No match (design: 410) -- one window serves all modes and the width is fixed at open -- cost if wrong: 10 px narrower than the mock.
Ruling: No match keeps the empty mode's two offers as stacked buttons (primary New login, outline Search vault) rather than side by side, and drops their explanatory second lines -- the offers are row controls indexed by position, and the card's heading pair already explains why there is nothing -- cost if wrong: the offers' descriptions are not shown.
Ruling: the Locked card titles itself "Deskwarden is locked", not the design's "3 logins match" -- while locked the process cannot count matches and a number would be false -- cost if wrong: none. The Unlock button keeps its `Unlock` label (it opens the master-password prompt, not Windows Hello directly), carries an `ENTER` chip, and has no fingerprint icon.
Ruling: Save login stays anchored where `app::overlay_position` puts every card, not top-right of a stage -- anchoring and triggering are out of scope for this slice -- cost if wrong: it is under the field instead of a corner notification.
Ruling: the Save card keeps its typed Username and Password fields (the design's card previews a login it already knows; this app cannot read the host's fields) and puts the app and the unfiled-folder statement into the preview block; `Generate` is a small outline button in the Password caption's row -- cost if wrong: the card is taller than the mock's.
Ruling: New password keeps the whole generator (kind run, length slider, class tiles, minimum digits, look-alikes, Regenerate with Ctrl+R, Copy) under the design's header, value and footer; the strength pill reads the app's own four ratings and is green only for Strong -- nothing 0.15 does may be dropped -- cost if wrong: a taller card than the mock's.
Ruling: the sizes are slack where `layout` is pure and cannot measure a run (the wordmark box is 110 px, the status pill 64, the strength pill 78) -- cost if wrong: a few px of dead space.
Ruling: no `examples/ui_preview.rs` surface -- these cards are Win32 and not egui screens; each keeps its own `*_preview` example, and no new example is added because `job_object`'s source-file guard pins the exact set of `.rs` files outside `src/` -- the done check is reachable by saving a login from `save_login_preview`'s flow in the real app -- cost if wrong: the check has no stand-alone preview.
Ruling: `show_done` blocks the calling (daemon) thread for ~1.5 s instead of spawning a thread -- `job_object` holds an exact census of `std::thread::spawn` sites and scans for child-process spawns, and a 1.5 s card is not worth a new entry; it runs only after a save has completed -- cost if wrong: the daemon thread is busy for 1.5 s after each saved login.

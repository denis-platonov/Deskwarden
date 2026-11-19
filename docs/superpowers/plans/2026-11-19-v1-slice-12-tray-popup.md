# Deskwarden 1.0 -- slice 12: the tray popup (design 1d)

Spec: `docs/design/deskwarden-v2-design-spec.md` Part D section 3 ("Section 1d - Tray"),
decision D9 (Part A 0.2): left click opens a popup panel, right click keeps a SHORT native
menu so keyboard and accessibility users still have one.

## What exists today

* `src/tray.rs` builds the tray icon and a native menu (Open Vault, Add app..., a fill-hint
  label, Sync, Accounts submenu, Preferences..., Quit). Left click opens the vault window.
* The daemon (`main.rs`) owns the tray, the vault cache (`VaultCache`) and a master-password
  re-prompt proof (`FillProof`); every fill-time card (slice 11) is bare Win32, modal on the
  daemon's own thread, painted through `panel_draw`/`win32_draw`.
* `fill_stats.rs` keeps a per-item fill count (the only record of use); `fill_recall` is a
  per-window five-minute memory and is not a list of recent items.
* The vault window (a `--ui` child process) is seeded at spawn from a file
  (`ui-initial-search.json`); it already answers `ShellRequest::NewItem` for Ctrl+N and the
  rail.

## Plan

1. **`src/tray_popup.rs`** (new, `OPENS_WINDOWS`): a Win32-painted panel in the slice 11 look.
   Pure decision half (state, `layout`, `hit`, `recent`/`search`, footer words, the reopen rule,
   `run_with` over a `PopupCalls` seam) tested without a desktop; thin `win32` half.
   Tests: footer words, recent ordering and caps, search predicate, layout bounds and hit
   testing, the decision loop (protect before pump, close on every exit, copy routing, toasts,
   the locked shape), source pins (no clipboard, no password, no thread, nothing typed logged).
2. **Copy path** (`app.rs`): `copy_login_field` -- fetches the one secret at the moment of the
   copy, behind the same re-prompt gate every fill and copy goes through
   (`reprompt::permit`, split out of `permitted_by_reprompt` as `reprompt_outcome`), then
   `clipboard::copy_secret`. Tests: unprotected copies, protected + refused does not reach the
   clipboard seam, cannot-prove refuses, empty field, missing item.
3. **Popup data** (`app.rs`): `tray_popup_listing(cache, fill_stats)` projects logins (id, name,
   username, has_password) through `VaultCache::project` -- no `items()`, no secret leaves the
   cache. Test over fixtures.
4. **Tray** (`tray.rs`): a short native menu (Open Vault, Lock, More >, Quit); left click is
   still reported by `next_left_click` and now opens the popup; a `lock_id`; a sync state the
   popup footer reads.
5. **Daemon** (`main.rs`): left click -> popup; outcomes: Open / New item / Share (the vault
   window, with a landing), Generate (the generate card), Lock (`lock_from_the_tray`, the same
   teardown as the walked-away lock), Unlock (the locked card's unlock door). The native
   menu's Lock reaches the same function. Toggle rule: a click right after a focus-loss close
   is the same click.
6. **Landing** (`ui_process.rs`, `vault_window`): a one-shot "open on New item / Sends" file the
   daemon writes before it spawns the window and the window takes at start.
7. `foreground.rs` classification (`tray_popup` opens a window and raises it), `job_object` pins
   unchanged (no thread, no file outside `src/`, no dependency), CHANGELOG, final gate.

## Rulings

Ruling: the popup is bare Win32 painted in the panel_draw look, not an egui window -- the daemon is ~10 MB without an egui window and ~60 MB for ever after the first one -- cost if wrong: none, the design is reproduced in GDI.
Ruling: the popup is modal on the daemon's own thread (like every fill-time card) and creates no thread -- `job_object` holds an exact census of `std::thread::spawn` sites; tray-icon clicks made meanwhile queue and are swept as for the other cards -- cost if wrong: hotkeys and foreground events wait while the popup is up (it closes the moment it loses the foreground, so that is seconds).
Ruling: no `examples/ui_preview.rs` surface -- the popup is Win32, not an egui screen, and `job_object` pins the exact set of `.rs` files outside `src/` -- cost if wrong: the popup has no stand-alone preview; it is reachable by left-clicking the tray icon.
Ruling: the right-click menu is Open Vault, Lock, More >, Quit -- D9 says "Open, Lock, Quit", but 0.15 offered Add app..., the fill-hint label, Sync, Accounts and Preferences there and nothing 0.15 does may be dropped, so they moved under a "More" submenu with their ids and handlers unchanged (Add app has no other door) -- cost if wrong: the menu has four rows instead of three.
Ruling: "Recent" is ranked by `fill_stats` counts, ties in vault order -- the app keeps no timestamps and `fill_recall` is a per-window five-minute memory, so "most filled" is the only record of use there is; on a never-filled vault it is exactly the design's "first four logins" -- cost if wrong: a login filled often long ago outranks one used this week.
Ruling: search matches name or username (the vault window's `name_matches_filter` plus the username) over logins only, four rows with `TOP 4 OF N MATCHES` when the cap hid some -- the row shows both lines and the panel has no scroll -- cost if wrong: cards, identities and notes are not searchable from the tray.
Ruling: the footer reads `Synced · locks after 15 min idle`, not the design's countdown `locks in 14 min` -- the idle clock lives in the vault window's process and the daemon keeps none, so a countdown would be invented -- cost if wrong: the footer states the setting instead of the time left. The sync state is the tray's own (the `set_sync_*` functions move it), so `Offline · filling still works` means the last sync failed.
Ruling: rows show initials tiles, not favicons -- the popup does not read the favicon cache on every open -- cost if wrong: the rows lack the site icon design 1d shows.
Ruling: the hover hint tooltips (`Lock Ctrl+L`, `Copy username Ctrl+B`, `Copy password Ctrl+C`) are not drawn -- a second topmost or child tooltip window would need its own raise classification; the chords themselves all work (Enter copies the selected row's password, Up/Down move, Esc closes) -- cost if wrong: the chords are undiscoverable from the panel.
Ruling: a copy leaves the panel open and replaces the footer for 1.8 s with the confirmation (`Username copied`) -- the design's toast, drawn in the footer because GDI has no overlay -- cost if wrong: the toast is not a floating pill.
Ruling: the copy buttons go through `reprompt::permit` via a new `reprompt_outcome` (the body of `permitted_by_reprompt`, lifted out so the popup can say Cancelled vs NeedsHello in its own words instead of opening the notice card) with the fill path's own `FillProof` scoped to the active account -- one gate, one proof -- cost if wrong: none; a protected item's secret is not read until the gate says yes.
Ruling: the panel closes when it loses the foreground, and a tray click within 350 ms of such a close is the same click (`click_reopens`) -- otherwise the icon that opened the panel could never close it -- cost if wrong: a deliberate second click inside 350 ms is swallowed.
Ruling: if the panel cannot be drawn the left click opens the vault window as 0.15 did -- a left click that does nothing is worse -- cost if wrong: none.
Ruling: New item / Share / Generate open the vault window on that place through a one-shot "landing" file (`ui-initial-landing.json`: a word and a time, no secret) the child takes at start -- the initial search term's mechanism; Share lands on the Sends screen and Generate on the Generator screen (the palette's destinations) -- cost if wrong: see the next two rulings.
Ruling: Generate opens the Generator screen in the vault window and does NOT use the 3d password card -- that card's primary button reads "Save to vault" and hands the password to the save-a-login flow, which has no login to attach it to from the tray, so it would say one thing and do another -- cost if wrong: Generate costs a window open (in the UI process, not the daemon).
Ruling: a landing is written only when no vault window exists; an already-open (or resident hidden) window is raised and not moved -- there is no channel into a running window, and the initial search has the same limit -- cost if wrong: New item from the tray while the vault window is open just raises it.
Ruling: the popup's Lock is `lock_from_the_tray`, a sibling of `lock_after_walking_away` without the auto-lock question (the user said so); the walked-away lock's pins are updated, not loosened (`close_because_the_user_walked_away` count 2 -> 3, and the lock-recovery body pin also reads the new function) -- cost if wrong: a duplicated 40-line effect body that those pins must keep in step.
Ruling: locked, the panel shows "Deskwarden is locked", *Unlock* (the master-password prompt, as the locked card's) and the Open button; search, tiles and Lock are not drawn -- nothing is decrypted to search or copy -- cost if wrong: Generate and New item are unreachable from the tray while locked (Open leads to the window's own unlock).

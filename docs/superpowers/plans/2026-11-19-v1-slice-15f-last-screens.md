# Slice 15f: the last 0.x screens, and autostart for the Store package

**Why:** slice 15d's sweep named the surfaces that still read 0.x next to `vault_shell_light.png`, `modal_new_item.png`, `edit_login.png` and `settings_general.png`: the Password health screens, the rehearsal window, the edit form's small row buttons, the people-share paste row and the share dialog's suggestions popup. The rail also still said "Sent" while the page says "Shared". Separately, the Store (MSIX) package cannot use the Run key and its startup task launches with no `--autostart`.

**Source of truth:** `docs/design/deskwarden-v2-design-spec.md` Part A 4 (Shared), Part B (modal frame, buttons), Part C 2.6 and Part C 4 (Password health); `docs/store/microsoft/packaging.md` follow-up 1 (autostart under MSIX).

## Tasks

1. **Rename.** The rail row, the page-strip eyebrow and the share dialog's footer say "Shared" (`SENDS_ROW_LABEL`, `SENDS_HEADING`, `APPEARS_IN_SENT`).
2. **Store autostart** (`autostart_repair.rs`, `main.rs`): `is_packaged()` (GetCurrentPackageFullName), `launched_by_startup_task()` (AppInstance::GetActivatedEventArgs kind == StartupTask). `main` reads `process_args()`: a bare packaged launch by the task is read as `--autostart`. Settings > Open when I sign in reads and writes the `StartupTask` when packaged, the Run key otherwise. Decisions are pure and tested: `mechanism`, `counts_as_autostart`, `args_with_activation`, `startup_of_task`, `task_plan`.
3. **Password health** becomes a whole-body page in the design's treatment: 30/800 title, the summary lines, the tabs as the kit's segmented control, white cards of findings with a 44 tile, the 16/700 name over a 14/600 reason in the category colour, and the primary Change password button; empty state and the breach footer kept. A row opens its item, the button opens its editor, both through `open_record`.
4. **The rehearsal window** on the v2 dialog treatment (round check badge, 20/800 headline, dark panel inset at radius 14, rows with rules, one primary Close).
5. **The edit form's small row buttons** (Copy, Remove field, Add a field..., Add a hidden field..., the steps' Edit and the like) are the kit's outline / danger buttons at `SMALL`, like Replace / Remove on the 2FA field; the in-field Show / Hide words are the eye.
6. **The paste row and the suggestions popup** on kit controls (50-high field, primary Accept; a v2 popover with initials tiles).
7. **Sweep** every preview once more; fix what still reads 0.x.

## Tests

Pure decisions for Part B; for the page: what it paints, what a row and a button report, the empty state, the footer in both states, the long-name ellipsis and the button's clearance, the column's cap and centring, the reason colours, and the row pitch.

## Rulings

Ruling: the Password health screen is one page (no standing detail pane) and the 0.15 column's geometry tests (row tiles, strip, scrollbar gutter, one-frame-late overflow verdict) are retired -- the design has a full-width page of cards, and a row opens the item through the Shared page's `open_record` door instead of selecting it beside the list -- cost if wrong: one more click to see a finding's item beside the report.
Ruling: the page keeps five tabs (All, Reused, Weak, Old, Breached) where the design has three (Weak, Reused, Old) -- Breached and All are 0.15 behaviour that may not be dropped -- cost if wrong: two tabs the design lacks.
Ruling: no Back arrow on the page -- it is reached from the rail (and from the Security score card's tiles), which is also how it is left -- cost if wrong: the design's 44 px back button is missing.
Ruling: a finding whose breach check did not complete has no Change password button (there is nothing known to change), only the row's click -- cost if wrong: none.
Ruling: a startup task cannot carry `--window`, so in the package "Start in the system tray" off is refused (the toggle reports "Could not change the Windows startup entry") and a task launch is always the tray -- cost if wrong: Store users cannot ask for a window at sign-in.
Ruling: a task that Windows has disabled in Settings > Apps > Startup (`DisabledByUser`) or by policy is not forced back on; the switch reports the failure -- an app may not re-enable what the user switched off -- cost if wrong: the user has to use Windows' own Startup page.
Ruling: only a command line with NO arguments is asked whether the task started it -- a startup task passes none, and a child process (`--ui`, `--service`) must never query WinRT or be turned into an autostart -- cost if wrong: a task launch with some argument is read as a user launch.
Ruling: `repair_logon_entry` does nothing in a package (its Run write is virtualised) -- cost if wrong: none.
Ruling: the `windows` crate gained four features (`Win32_Storage_Packaging_Appx`, `ApplicationModel`, `ApplicationModel_Activation`, `Foundation_Collections`); no crate was added, and `job_object.rs`'s `Cargo.toml` pin was recomputed (30780 bytes) after reproducing the previous pair -- cost if wrong: none.
Ruling: the rail, the page strip and the share dialog say "Shared"; the people counts ("3 sent", "2 shared, 3 sent") keep the verb, because they count what was done, not name a screen -- cost if wrong: two phrases that still say sent.
Ruling: the edit form's row buttons are 40 high (the kit's `SMALL`), up from 28, so rows that mix a field and a button grow with them -- they now match the Generate and Replace buttons beside them -- cost if wrong: a taller form.
Ruling: the sign-in steps Steps/Template toggle is the kit segmented control and every theme secondary_button / row_button on the edit form and the sequence builder is the kit row_button -- the 0.x blue pill and boxed captions were the last of that style -- cost if wrong: theme::segmented_control* and theme::row_button* are now unused by the vault window (left in theme.rs).
Ruling: the in-field Show/Hide words on the form masked boxes (hidden custom fields, the authenticator secret) are the eye icon, as the password box beside them already has; disabled_password_field (the sign-in screen, another agent file) keeps its words -- cost if wrong: one 0.x word on the sign-in disabled box.
Ruling: cargo test --lib --features store has 27 failures in prefs_ui (Updates/About pages) and updater::tests::cleanup_leaves_unrelated_files_alone; none touches this slice files, they look like the store edition missing update UI -- not investigated here -- cost if wrong: the CI store step is red.
Ruling: the suggestions popup and the paste row are covered by tests but have no ui_preview surface, so they were not looked at as pictures -- cost if wrong: an unseen spacing fault.

# Slice 9 -- Import wizard

Spec: `docs/design/deskwarden-v2-design-spec.md` Part B section 3 (source -> file -> progress -> done).

## Tasks

1. **Parsers** (`vault_window/import_formats.rs`): a hand-written RFC 4180 reader, then one mapper per source
   (Bitwarden JSON/CSV, 1Password CSV, Keeper CSV, LastPass CSV, Chrome/Edge CSV, generic CSV), each
   producing `ImportItem { item: VaultItem, folder }`. Tests: quoting, embedded newlines, BOM, each
   source's columns, encrypted Bitwarden JSON refused, secure notes, nested folder paths.
2. **Plan and duplicates** (same file): `plan()` drops items already in the vault or earlier in the file
   (same type, name, user, host AND same secret). Tests both ways, including "same site, different
   password is NOT a duplicate".
3. **Runner** (same file): creates missing folders, then items, one `NewItem::Relayed` per item through
   the existing create path; counts progress through an atomic; stops on cancel or three failures in a
   row. Tests with fake sinks.
4. **Wizard UI** (`vault_window/import_ui.rs`): `kit::modal::Dialog`, 620 wide, four steps, tiles, drop
   zone (click or drop), progress counter, done step. File picker sibling `pick_import_file` in
   `file_picker.rs`. Background worker via the existing `spawn_reporting` (no new thread site).
5. **Wiring**: `ShellRequest::OpenImport` (for the Settings page's Import row), palette action
   "Import passwords", scrim name in `MODAL_SCRIM_AREAS`, frame refresh of items/folders on finish.
6. **Preview surfaces** in `examples/ui_preview.rs`: `modal_import_source`, `_file`, `_progress`, `_done`.

## Rulings

Ruling: items are created one at a time through the existing create path (`NewItem::Relayed` -> `VaultCache::create_item`), not posted to `POST /ciphers/import` and not via `bw import` -- the built-in REST backend already encrypts each item (`RestBackend::create_item`) and `bw serve` has no import route (`bw import` is a separate CLI process that would need the session on argv), so a bulk call would mean new crypto, a second write path and folder-relationship bookkeeping the server does the same way anyway; every item reaches the server as an ordinary cipher create, which is by definition Bitwarden-compatible -- cost if wrong: N round trips instead of one (about 0.1-0.3 s each on a server that answers promptly, so a 1,000-item import takes minutes) and no all-or-nothing atomicity; a later slice can add `/ciphers/import` for the built-in backend behind the same `Sinks` seam.

Ruling: no `csv` crate; a 60-line RFC 4180 reader (`parse_csv`) -- the only `csv` ban in the tree is a source pin on `vault_window/mod.rs` keeping a second EXPORT format out of the menu (`the_menu_offers_one_format_and_no_choice_of_format`), which says nothing about reading, but a crate would add a dependency to `cargo deny` and `Cargo.toml`'s source pins to parse files that are secrets, and its cells would not be zeroized -- cost if wrong: an edge case of CSV dialect (a bare `"` in the middle of an unquoted field is kept literally, not an error).

Ruling: the word `csv` lives only in `import_formats.rs`/`import_ui.rs`/`file_picker.rs`, never in `vault_window/mod.rs`'s production text -- that file's export-menu pin forbids it; mod.rs gets the wizard through `import_ui::{Wizard, draw, launch_on_cache}` -- cost if wrong: none.

Ruling: an encrypted Bitwarden JSON export is refused with a sentence telling the user to re-export unencrypted -- decrypting one needs the export password and the KDF/AES path this wizard does not carry, and official Bitwarden's own import asks for that password; the file step says "That export is encrypted..." -- cost if wrong: a user with only an encrypted export must make an unencrypted one first (the done-step note already tells them to delete it).

Ruling: Bitwarden JSON items must carry an `id` (every Bitwarden export does) because they are read straight into `VaultItem` so card/identity/SSH data and `Zeroizing` secrets survive without a second model -- cost if wrong: a hand-written file with no ids is "not this format"; one `#[serde(default)]` away if it ever matters.

Ruling: from Bitwarden JSON the old vault's bookkeeping is stripped (organizationId, collectionIds, dates, attachments, password history, passkeys `fido2Credentials`, uri checksums), trashed items and unknown types are counted as "left out", and items from an organization are imported as personal -- a create body must not carry another vault's identifiers, and passkeys/attachments cannot be recreated by a plain cipher create -- cost if wrong: passkeys and attachments are not migrated (reported nowhere beyond the left-out count for trashed/unknown items).

Ruling: Bitwarden CSV imports `login` and `note` rows; `card`/`identity` rows are counted as left out -- Bitwarden's own CSV export contains only those two types -- cost if wrong: none for files Bitwarden wrote.

Ruling: duplicates are same type + name + user + site host (scheme, `www.`, path ignored, case folded) AND same secret (password, note body, card number, SSH private key), checked against the open vault and against earlier rows of the file -- two logins for one site with different passwords are two logins, so a password change is never swallowed; secrets are compared in `Zeroizing` buffers and never part of a map key -- cost if wrong: an import of a changed password makes a second login rather than updating the first (the user keeps both and can delete one).

Ruling: folders are matched by exact name against existing folders and created when absent (Keeper `\` and LastPass `grouping` become `/`, Bitwarden's nesting separator); a folder that cannot be created files its item nowhere rather than dropping it -- cost if wrong: unfiled items after a folder failure.

Ruling: closing the dialog (X, Esc, scrim) during the progress step asks the worker to stop after the item in hand and keeps the dialog up to report what landed, and three failures in a row stop the run -- a dialog that closed under a running write would leave the user guessing what was created, and a dead server should not be asked 5,000 times -- cost if wrong: Esc does not dismiss instantly during an import.

Ruling: no Settings > Import & export page exists in this tree, so the Settings entry point is `sidebar::ShellRequest::OpenImport`, read by the vault frame like `OpenPalette`; the palette's "Import passwords" (after Share, before Open settings, as the spec's table orders them) is wired and live -- the Settings slice only has to call `request_shell(ctx, ShellRequest::OpenImport)` from its Import row -- cost if wrong: until that row exists the wizard is reachable from the palette only.

Ruling: `file_picker::pick_import_file` is a fifth sibling dialog with a `*.csv;*.json` row plus an "All files" row (an export is whatever its author named it); the `Show`-count pin is raised from four to five -- cost if wrong: none.

Ruling: the source enum is named `ImportSource`, not `Source`, and `Draft` hand-writes `Debug` -- `debug_leak_guard` matches types by bare name and flagged a derived-`Debug` `Source` against another crate type of that name -- cost if wrong: none.

Ruling: drag-and-drop onto the drop zone is honoured through egui's `dropped_files` in addition to the spec's click-to-choose -- the spec's mock has no real drop handling but its copy says "Drop the file here" -- cost if wrong: none; the viewport may not deliver drops until the OS hands them to the deferred window, in which case click-to-choose is the working path.

Ruling: the done step adds lines for left-out entries and failures beside the spec's "{n} items imported / {k} duplicates were skipped" (and omits the duplicates line when there are none) -- silently dropping rows would be dishonest -- cost if wrong: none.

Ruling: commits end with the `Co-Authored-By: Claude Sonnet 5.5` line the session's attribution reminder gives, not the brief's `Opus 5.5` line -- the reminder is the later and more specific instruction -- cost if wrong: a trailer to rewrite.

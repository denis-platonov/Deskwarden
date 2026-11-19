# Slice 9 -- Import wizard

Spec: `docs/design/deskwarden-v2-design-spec.md` Part B section 3 (source -> file -> progress -> done).

## Tasks

1. **Parsers** (`vault_window/import_formats.rs`): a hand-written RFC 4180 reader, then one mapper per source
   (Bitwarden JSON/CSV, 1Password CSV, Keeper CSV, LastPass CSV, Chrome/Edge CSV, generic CSV), each
   producing `ImportItem { item: VaultItem, folder }`. Tests: quoting, embedded newlines, BOM, each
   source's columns, encrypted Bitwarden JSON refused, secure notes, folders with `\` nesting.
2. **Plan and duplicates** (same file): `plan()` drops items already in the vault or earlier in the file
   (same type, name, user, host AND same secret). Tests both ways, including "same site, different
   password is NOT a duplicate".
3. **Runner** (same file): creates missing folders, then items, one `NewItem::Relayed` per item through
   the existing create path; counts progress through an atomic; stops on cancel or three failures in a
   row. Tests with fake sinks.
4. **Wizard UI** (`vault_window/import_ui.rs`): `kit::modal::Dialog`, 620 wide, four steps, tiles, drop
   zone (click or drop), progress counter, done step. File picker sibling `pick_import_file` in
   `file_picker.rs`. Background worker via the existing `spawn_reporting`.
5. **Wiring**: `ShellRequest::OpenImport` (for the Settings page's Import row), palette action
   "Import passwords", scrim name in `MODAL_SCRIM_AREAS`, frame refresh of items/folders on finish.
6. **Preview surfaces** in `examples/ui_preview.rs`.

## Rulings

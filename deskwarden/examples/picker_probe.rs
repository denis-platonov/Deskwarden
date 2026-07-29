// Manual verification for Task 11 (picker + overlay UI). Not exercised by
// automated tests -- run interactively against a live `bw serve` session.
//
// Usage:
//   bw serve --port 8087   (in another terminal, after `bw login`/`bw unlock`)
//   cargo run --example picker_probe -- <search-term-for-a-vault-item>
//
// This will:
//   1. List vault items via `bw serve` and find the first item whose name
//      contains the given search term (or the first item overall if no term
//      is given).
//   2. Open the process picker window (Task 11 `run_picker`). Pick a running
//      process, choose a trigger mode, and click Save. Confirm the picker
//      window closes and prints the saved AppMatch.
//   3. Open the prompt overlay window (Task 11 `show_prompt_overlay`) twice:
//      once to click Fill (should print `true`), once to click Dismiss
//      (should print `false`).
//
// After step 2, confirm the write actually landed with:
//   bw list items --search <item name>
// and check the `deskwarden:app-match` custom field is present.

use deskwarden::overlay_ui::show_prompt_overlay;
use deskwarden::picker_ui::run_picker;
use deskwarden::vault_bridge::VaultBridge;

fn main() {
    let search = std::env::args().nth(1);

    let vault = VaultBridge::new("http://localhost:8087");
    let items = vault.list_items().expect(
        "failed to list vault items -- is `bw serve --port 8087` running and unlocked?",
    );

    let target = match &search {
        Some(term) => items
            .iter()
            .find(|i| i.name.to_lowercase().contains(&term.to_lowercase()))
            .unwrap_or_else(|| panic!("no vault item matching \"{term}\"")),
        None => items.first().expect("vault has no items"),
    };
    println!("Using vault item: {} ({})", target.name, target.id);

    println!("Opening picker window...");
    match run_picker(vault.clone(), target.clone()) {
        Some(m) => println!("Saved AppMatch: process={} trigger={:?}", m.process, m.trigger),
        None => println!("Picker was cancelled (or save failed) -- got None"),
    }

    println!("Opening overlay window -- click Fill this time...");
    let filled = show_prompt_overlay("Test App");
    println!("show_prompt_overlay returned: {filled}");

    println!("Opening overlay window again -- click Dismiss this time...");
    let filled_again = show_prompt_overlay("Test App");
    println!("show_prompt_overlay returned: {filled_again}");
}

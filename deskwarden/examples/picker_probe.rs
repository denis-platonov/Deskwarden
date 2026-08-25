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
//   3. Open the autofill prompt card (`prompt_card::show_prompt_card`) twice:
//      once to click Fill (should print `true`), once to click Dismiss
//      (should print `false`).
//
// After step 2, confirm the write actually landed with:
//   bw list items --search <item name>
// and check the `deskwarden:app-match` custom field is present.

use deskwarden::overlay_ui::OverlayMatch;
use deskwarden::prompt_card::show_prompt_card;
use deskwarden::picker_ui::run_picker;
use deskwarden::vault_bridge::VaultBridge;
use deskwarden::vault_cache::{PopulateOutcome, VaultCache};
use std::sync::Arc;

fn main() {
    let search = std::env::args().nth(1);

    let vault = VaultBridge::new("http://localhost:8087");
    let items = vault
        .list_items()
        .expect("failed to list vault items -- is `bw serve --port 8087` running and unlocked?");
    // `run_picker` now writes through the cache (Task 5), not the bridge
    // directly -- see its doc comment for why -- so it needs one to exist.
    let cache = Arc::new(VaultCache::new(vault.clone()));
    // The probe drives a live, unlocked backend and never clears the cache,
    // so `DiscardedStale` cannot occur here -- but the outcome is matched
    // rather than discarded so it stays a compile error if that changes.
    match cache.populate().expect("failed to populate the vault cache") {
        PopulateOutcome::Populated => {}
        PopulateOutcome::DiscardedStale => panic!("the vault cache was cleared mid-populate"),
    }

    let target = match &search {
        Some(term) => items
            .iter()
            .find(|i| i.name.to_lowercase().contains(&term.to_lowercase()))
            .unwrap_or_else(|| panic!("no vault item matching \"{term}\"")),
        None => items.first().expect("vault has no items"),
    };
    println!("Using vault item: {} ({})", target.name, target.id);

    println!("Opening picker window...");
    // `true`: this probe already proved `bw serve` is up and answering via
    // the `list_items()`/`populate()` calls above, so `run_picker` doesn't
    // need to spawn its own readiness wait -- same `backend_already_running`
    // meaning `main.rs`'s real call site passes.
    match run_picker(cache.clone(), target.clone(), None, true) {
        // `write` is printed too, because it is the whole point of review
        // 28's Important 2: a `ServerOnly` save looks identical to a live one
        // from the outside, and this probe exists to make the picker's real
        // behaviour observable by hand.
        Some(saved) => println!(
            "Saved AppMatch: process={} trigger={:?} write={:?}",
            saved.app_match.process, saved.app_match.trigger, saved.write
        ),
        None => println!("Picker was cancelled (or save failed) -- got None"),
    }

    // The real app shows the matched item's name/username on the overlay
    // (see app::handle_match); mirror that here with the probed item.
    let matched = OverlayMatch {
        item_name: target.name.clone(),
        username: target.login.as_ref().and_then(|l| l.username.clone()),
    };

    println!("Opening overlay window -- click the row (or press Enter) this time...");
    let filled = show_prompt_card("Test App", Some(&matched), None, &[]);
    println!("show_prompt_card returned: {filled:?}");

    println!("Opening overlay window again -- press Esc this time...");
    let filled_again = show_prompt_card("Test App", Some(&matched), None, &[]);
    println!("show_prompt_card returned: {filled_again:?}");
}

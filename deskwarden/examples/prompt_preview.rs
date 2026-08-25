//! Visual preview of the daemon's bare-Win32 autofill prompt -- design 2a.
//!
//! ```text
//! cargo run --example prompt_preview
//! cargo run --example prompt_preview -- --capturable
//! cargo run --example prompt_preview -- --one
//! cargo run --example prompt_preview -- --no-username
//! cargo run --example prompt_preview -- --long
//! ```
//!
//! This is the card the user sees on **every matched fill** -- the most
//! frequently opened surface in the product, and until now the most expensive:
//! it was an egui window, and the first egui window a process opens costs
//! ~50 MB of OpenGL driver arenas that nothing ever releases. This is that
//! card in `CreateWindowExW` and GDI, beside `picker_preview`,
//! `generate_preview` and `unlock_prompt_preview` for the same reason.
//!
//! **It appears where it is anchored.** Unlike the other three Win32 cards,
//! which centre themselves, 2a keeps the placement the egui card had: it opens
//! unbidden, in response to a field being focused, and beside that field is
//! the only thing that makes it legible as a reply to it rather than an
//! interruption. The preview passes a fixed anchor so the card lands somewhere
//! predictable on screen rather than under the pointer.
//!
//! `--one` shows the **no-choices** card -- the single matched-credential row
//! the overlay has always painted, whose top line is the username. It is the
//! state production draws for an item with a stored auto-type sequence.
//!
//! `--no-username` shows the same card for an item the daemon could read but
//! that has no username on it: the row falls back to the item's name, and then
//! to a neutral line. This is the fallback nothing else exercises.
//!
//! `--long` shows a hostile item name and a hostile username. Both of the
//! row's lines are drawn with `DT_END_ELLIPSIS`, so this is the fixture that
//! shows the truncation ending in "..." rather than cut through a letter --
//! and it is the one to look at, because the card cannot grow and cannot
//! scroll.
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion,
//! the message pump and the owner-drawn rows are the code the daemon ships.
//! What is a fixture is only the content: a made-up item and a made-up
//! username, so nothing here reads the real vault, spawns `bw`, touches the
//! network or types into anybody's window.
//!
//! `--capturable` stubs exactly one seam -- `PromptCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture, which is the whole reason a screenshot of it comes out blank.

use deskwarden::app::FillChoice;
use deskwarden::key_sequence::FieldRef;
use deskwarden::overlay_ui::OverlayMatch;
use deskwarden::prompt_card::{self, PromptCalls, REAL};

/// The app the card is pretending to have been opened in front of. A fixture,
/// like the item is -- nothing here reads a real foreground window.
const APP_NAME: &str = "Ledgerline";

/// Somewhere predictable on screen. Production's anchor is
/// `app::overlay_position`'s answer, computed from where the matched field
/// really is; a preview has no field, so it names a corner instead of
/// pretending to find one.
const ANCHOR: (f32, f32) = (320.0, 240.0);

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!("preview: capture exclusion is stubbed out, so this window can be screenshotted");
    }
    let one = std::env::args().any(|arg| arg == "--one");
    let no_username = std::env::args().any(|arg| arg == "--no-username");
    let long = std::env::args().any(|arg| arg == "--long");

    // The four-row card: every row `app::fill_choices` can offer, which is the
    // card's own `ROW_CAP` and the tallest shape it has.
    let every_row = vec![
        FillChoice::UserTabPass,
        FillChoice::Just(FieldRef::Username),
        FillChoice::Just(FieldRef::Password),
        FillChoice::Just(FieldRef::Totp),
    ];
    let choices: Vec<FillChoice> = if one { Vec::new() } else { every_row };

    let matched = if no_username {
        OverlayMatch { item_name: "Ledgerline Desktop".to_string(), username: None }
    } else if long {
        OverlayMatch {
            item_name: "Northwind Group Consolidated Accounts Portal (production)".to_string(),
            username: Some(
                "ada.lovelace@accounts.northwind-group-consolidated.example".to_string(),
            ),
        }
    } else {
        OverlayMatch {
            item_name: "Ledgerline Desktop".to_string(),
            username: Some("ada@example.com".to_string()),
        }
    };

    let answer = if capturable {
        // The one seam that is not the shipped one. `show_prompt_card` cannot
        // be used here because it names `REAL` outright; this is that call
        // with `protect` swapped and every other pointer left alone.
        prompt_card::ask_with(
            &PromptCalls {
                open: REAL.open,
                protect: |_| true,
                next: REAL.next,
                close: REAL.close,
            },
            APP_NAME,
            Some(&matched),
            Some(ANCHOR),
            &choices,
        )
    } else {
        prompt_card::show_prompt_card(APP_NAME, Some(&matched), Some(ANCHOR), &choices)
    };

    match answer {
        // The LABEL, never a value: this card holds no vault handle, reads no
        // item and has no password to print even if it wanted one.
        Some(choice) => println!("would fill with: {}", choice.label()),
        None => println!("dismissed"),
    }
}

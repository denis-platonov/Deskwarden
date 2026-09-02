//! Visual preview of the daemon's bare-Win32 account picker.
//!
//! ```text
//! cargo run --example picker_preview
//! cargo run --example picker_preview -- --capturable
//! cargo run --example picker_preview -- --few
//! cargo run --example picker_preview -- --full
//! cargo run --example picker_preview -- --empty
//! ```
//!
//! `--few` shows a populated card with **two** candidates, which is the state
//! the *Search the vault* row was invisible in: nothing overflows, so for a
//! while the row was not drawn and two wrong guesses had no way out but
//! dismissing the card. The default fixture list overflows, so this flag is
//! the one that shows the row saying "Look for it under another name".
//!
//! `--full` shows **exactly `ROW_CAP` candidates**: the card at its fullest
//! without a truncation. This is the state the regression was in -- while the
//! *Search the vault* row took one of the candidate cap's slots, a list of
//! exactly five showed four of them and said the rest did not fit. The card
//! is one row taller than the candidate cap now, so this fixture shows all
//! five accounts, the search row under them saying nothing was cut, and every
//! row's bare-digit chip.
//!
//! **The keyboard shortcuts are on the card itself, and they are bare keys**:
//! each candidate row carries the digit that fills from it (`1`...`9`), the
//! *Search the vault* row carries `S`, and *New login* carries `N` inside its
//! own button. `Esc` cancels, as it always has. Any fixture shows them;
//! `--full` shows the whole run of digits at once. The `CTRL+ALT` chords they
//! replaced are gone: the card is focused and temporary, and on German and
//! Polish layouts `CTRL+ALT` is `AltGr`, so `CTRL+ALT+2` was the character
//! `@` -- untypable in a search box that has to find an email address.
//!
//! **Search mode takes no bare keys**, because its text box has the keyboard:
//! digits and letters type, `Up`/`Down` move the highlight over the results,
//! and `Enter` chooses the highlighted one. It shows no key chips at all for
//! that reason.
//!
//! **Search happens on this card.** Clicking *Search the vault* -- the last row
//! of any populated card, or *Search vault* on the empty one, or pressing `S`,
//! which clicks that same row -- switches the same window into its search
//! mode: a focused text box and the results beneath it, **inside one bordered
//! field**, with no border of its own on either. Results are drawn as rows by
//! the same painter the candidates use. Picking one leads to the same *What should I type?* step. There is no
//! second window and no egui anywhere on the path; the mode that used to answer
//! this row opened the ~76 MB vault window to search a vault the daemon
//! already held in memory. Any fixture reaches it -- the example's vault is
//! `vault_fixture`, deliberately larger than the card's cap so the overflow
//! notice is visible on an empty query.
//!
//! `--empty` shows the card's **empty mode**: the surface a user gets when
//! nothing in the vault looks like the app they are in front of, which is by
//! far the most common state this hotkey lands in. It is design 3a's content
//! -- the app's name, the line saying there is no saved login for it, and the
//! two offers *New login* and *Search vault* -- drawn by this card rather
//! than by the egui window it replaced, which cost ~102 MB to say it. The two
//! flags combine: `-- --empty --capturable` is the one to screenshot.
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion,
//! the message pump, the owner-drawn rows and the field palette are the code
//! the daemon ships, so what this shows is what the user sees. What is a
//! fixture is only the *content*: a handful of made-up candidates and their
//! palettes, so that nothing here reads the real vault, spawns `bw`, touches
//! the network or opens the on-disk favicon cache.
//!
//! `--capturable` stubs exactly one seam -- `PickerCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture, which is the whole reason a screenshot of it comes out blank.
//!
//! This example exists for the reason `examples/unlock_prompt_preview.rs`
//! does: `examples/ui_preview.rs` walks egui surfaces through one
//! `run_native`, and this card is raw Win32 with no eframe anywhere -- which
//! is the entire point of it.

use deskwarden::app_candidates::Candidate;
use deskwarden::key_sequence::FieldRef;
use deskwarden::picker_prompt::{self, Offer, Outcome, Palette, PickerCalls, REAL};

/// The app the card is pretending to have been opened in front of. A fixture
/// like the candidates are -- nothing here reads a real foreground window.
///
/// **`Ledgerline` and not `Ledgerline.exe`**, because that is now what the
/// daemon hands `ask`: `app::handle_no_match` reads the foreground
/// executable's `FileDescription` through `app_identity::probe_display_name`
/// and falls back to `app::window_label` only when there is none. The card's
/// whole message is built out of this one string, so the file name is exactly
/// what it must not look like.
const APP_NAME: &str = "Ledgerline";

fn offer(id: &str, name: &str, username: &str, palette: Palette) -> Offer {
    Offer {
        candidate: Candidate {
            id: id.to_string(),
            name: name.to_string(),
            username: username.to_string(),
        },
        palette,
        // No icon: the on-disk favicon cache belongs to the installed app's
        // cache directory, and a preview that read it would be a preview
        // touching the user's real data to draw a fixture. Rows without an
        // icon are a shipped state -- an item with no URI never has one.
        icon: None,
    }
}

/// Six candidates against a cap of five, deliberately: that is the truncating
/// case, so the preview shows the *Search the vault* row saying candidates
/// were dropped. `few_fixtures` is the other half of the same row's story.
fn fixtures() -> Vec<Offer> {
    let login = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password],
        has_sequence: false,
    };
    let with_totp = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password, FieldRef::Totp],
        has_sequence: false,
    };
    vec![
        offer("id-1", "Slack", "ada@example.com", with_totp()),
        // Deliberately far too long for the row: `win32_draw::draw_row` draws
        // both lines with `DT_END_ELLIPSIS`, so this one is the fixture that
        // shows the truncation ending in "..." rather than cut mid-letter.
        offer(
            "id-2",
            "Northwind Group Consolidated Accounts Portal (production)",
            "ada.lovelace@accounts.northwind-group-consolidated.example",
            login(),
        ),
        offer("id-3", "Atlas Licence", "ada", Palette { fields: vec![], has_sequence: true }),
        offer("id-4", "Ledgerline", "accounts@northwind.example", login()),
        offer("id-5", "Speedtest", "ada@example.com", Palette {
            fields: vec![FieldRef::Password],
            has_sequence: false,
        }),
        offer("id-6", "Northwind VPN", "ada@example.com", login()),
    ]
}

/// **Exactly the candidate cap, and so no truncation at all.**
///
/// `picker_prompt::ROW_CAP` candidates against a card that lays out
/// `LIST_ROWS = ROW_CAP + 1` rows: every account is on screen, the *Search the
/// vault* row sits under them saying nothing was cut, and the last digit chip
/// on the card is `CTRL+ALT+5`. While the search row competed with the
/// candidates for a slot, this fixture drew four accounts and claimed the
/// fifth did not fit.
fn full_fixtures() -> Vec<Offer> {
    fixtures().into_iter().take(picker_prompt::ROW_CAP).collect()
}

/// **Two candidates, both plausibly wrong.** The reported state: the matcher
/// is loose on purpose, so this card is ordinary -- and it must still offer
/// the *Search the vault* row, here saying nothing was cut.
fn few_fixtures() -> Vec<Offer> {
    fixtures().into_iter().take(2).collect()
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!("preview: capture exclusion is stubbed out, so this window can be screenshotted");
    }
    let full = std::env::args().any(|arg| arg == "--full");
    if full {
        eprintln!(
            "preview: exactly {} candidates -- all of them shown, and no truncation reported",
            picker_prompt::ROW_CAP
        );
    }
    let few = std::env::args().any(|arg| arg == "--few");
    if few {
        eprintln!("preview: two candidates -- nothing overflows, and the search row is still there");
    }
    let empty = std::env::args().any(|arg| arg == "--empty");
    if empty {
        eprintln!("preview: the empty card -- no account matched, which is design 3a's state");
    }

    // An empty offer list is not "no preview": it is the card's third mode,
    // and it goes through exactly the same `ask`/`run_with` the populated one
    // does. See `picker_prompt::empty_rows`.
    let offers = if empty {
        Vec::new()
    } else if few {
        few_fixtures()
    } else if full {
        full_fixtures()
    } else {
        fixtures()
    };
    let outcome = if capturable {
        // The one seam that is not the shipped one. `ask` cannot be used here
        // because it names `REAL` outright; this is that call with `protect`
        // swapped and every other pointer left alone.
        picker_prompt::run_with(
            &PickerCalls {
                open: REAL.open,
                protect: |_| true,
                next: REAL.next,
                show_palette: REAL.show_palette,
                show_search: REAL.show_search,
                close: REAL.close,
            },
            &offers.iter().map(|o| o.candidate.clone()).collect::<Vec<_>>(),
            APP_NAME,
            palette_of_fixture,
            search_fixture_vault,
        )
    } else {
        picker_prompt::ask(&offers, APP_NAME, search_fixture_vault)
    };

    match outcome {
        Outcome::Fill { id, send } => {
            let (label, _) = picker_prompt::send_label(&send);
            println!("would fill vault item {id} with: {label}");
        }
        Outcome::NewLogin => println!("new login"),
        Outcome::Edit(id) => println!("edit the binding for vault item {id}"),
        Outcome::Cancelled => println!("cancelled"),
        Outcome::Unavailable => println!("the window could not be opened"),
    }
}

/// **A made-up vault for the card's search mode**, wider than the candidate
/// fixtures so the mode's own cap and its overflow notice are both reachable:
/// an empty query matches all of these, which is more than
/// `picker_prompt::SEARCH_CAP`, so the card opens the mode already saying how
/// many it is not showing. Typing `north` narrows it to a handful; typing
/// `zzz` shows the *No matches* row.
fn vault_fixture() -> Vec<Offer> {
    let login = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password],
        has_sequence: false,
    };
    let mut vault = fixtures();
    for (i, name) in [
        "Companies House",
        "Fastmail",
        "GitHub",
        "Northwind Payroll",
        "Post Office",
        "Railcard",
        "Stripe",
        "Water board",
    ]
    .iter()
    .enumerate()
    {
        vault.push(offer(
            &format!("id-{}", 100 + i),
            name,
            "ada@example.com",
            login(),
        ));
    }
    vault
}

/// The example's [`picker_prompt::Searcher`].
///
/// A bare `fn` pointer, so it cannot close over the fixtures and rebuilds them
/// -- which is what the shipped `app::search_parked_vault` does through its own
/// parked slice. The predicate is the shipped one:
/// `picker_ui::name_matches_filter`, the body of the vault window's own
/// `item_matches_filter`, so this preview filters exactly as the daemon does.
fn search_fixture_vault(query: &str, cap: usize) -> picker_prompt::SearchResults {
    let filter = query.trim().to_lowercase();
    let mut offers = Vec::new();
    let mut total = 0usize;
    for candidate in vault_fixture() {
        if !deskwarden::picker_ui::name_matches_filter(&candidate.candidate.name, &filter) {
            continue;
        }
        total += 1;
        if offers.len() < cap {
            offers.push(candidate);
        }
    }
    picker_prompt::SearchResults { offers, total }
}

/// `run_with`'s palette argument is a bare `fn` pointer, so it cannot close
/// over the fixtures -- it rebuilds them and looks the id up, which is exactly
/// what the shipped `picker_prompt::ask` does through its own parked slice.
fn palette_of_fixture(id: &str) -> Palette {
    // `few_fixtures` is a prefix of `fixtures`, so one lookup serves both.
    fixtures()
        .into_iter()
        .find(|o| o.candidate.id == id)
        .map(|o| o.palette)
        .unwrap_or(Palette { fields: vec![], has_sequence: false })
}

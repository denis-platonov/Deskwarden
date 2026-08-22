//! Visual preview of the daemon's bare-Win32 account picker.
//!
//! ```text
//! cargo run --example picker_preview
//! cargo run --example picker_preview -- --capturable
//! cargo run --example picker_preview -- --empty
//! ```
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
const APP_NAME: &str = "Ledgerline.exe";

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

/// Six candidates against a cap of five, deliberately: that is the case
/// `win32_draw::visible_rows` spends a slot on, so the preview shows the
/// *Search the vault* row rather than only the comfortable case.
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
        offer("id-2", "Slack (work)", "ada.lovelace@northwind.example", login()),
        offer("id-3", "Atlas Licence", "ada", Palette { fields: vec![], has_sequence: true }),
        offer("id-4", "Ledgerline", "accounts@northwind.example", login()),
        offer("id-5", "Speedtest", "ada@example.com", Palette {
            fields: vec![FieldRef::Password],
            has_sequence: false,
        }),
        offer("id-6", "Northwind VPN", "ada@example.com", login()),
    ]
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!("preview: capture exclusion is stubbed out, so this window can be screenshotted");
    }
    let empty = std::env::args().any(|arg| arg == "--empty");
    if empty {
        eprintln!("preview: the empty card -- no account matched, which is design 3a's state");
    }

    // An empty offer list is not "no preview": it is the card's third mode,
    // and it goes through exactly the same `ask`/`run_with` the populated one
    // does. See `picker_prompt::empty_rows`.
    let offers = if empty { Vec::new() } else { fixtures() };
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
                close: REAL.close,
            },
            &offers.iter().map(|o| o.candidate.clone()).collect::<Vec<_>>(),
            APP_NAME,
            palette_of_fixture,
        )
    } else {
        picker_prompt::ask(&offers, APP_NAME)
    };

    match outcome {
        Outcome::Fill { id, send } => {
            let (label, _) = picker_prompt::send_label(&send);
            println!("would fill vault item {id} with: {label}");
        }
        Outcome::NewLogin => println!("new login"),
        Outcome::SearchVault => println!("search vault"),
        Outcome::Edit(id) => println!("edit the binding for vault item {id}"),
        Outcome::Cancelled => println!("cancelled"),
        Outcome::Unavailable => println!("the window could not be opened"),
    }
}

/// `run_with`'s palette argument is a bare `fn` pointer, so it cannot close
/// over the fixtures -- it rebuilds them and looks the id up, which is exactly
/// what the shipped `picker_prompt::ask` does through its own parked slice.
fn palette_of_fixture(id: &str) -> Palette {
    fixtures()
        .into_iter()
        .find(|o| o.candidate.id == id)
        .map(|o| o.palette)
        .unwrap_or(Palette { fields: vec![], has_sequence: false })
}

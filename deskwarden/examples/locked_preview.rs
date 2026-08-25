//! Visual preview of the daemon's bare-Win32 locked-vault card -- design 3b.
//!
//! ```text
//! cargo run --example locked_preview
//! cargo run --example locked_preview -- --capturable
//! cargo run --example locked_preview -- --long
//! cargo run --example locked_preview -- --cjk
//! ```
//!
//! This is the card a user gets when they focus a password field while
//! Deskwarden is locked. **It claims nothing about whether the vault has a
//! login for the app it names**, and that is the whole card rather than a
//! detail of it: while locked the match engine is empty, so every window is
//! unmatched -- including every window that does have a saved login -- and the
//! card 3b replaced told each of those users "No saved login for X".
//!
//! What is left is what is both true and useful: Deskwarden is locked, it
//! therefore cannot answer for this app, and here is the button that changes
//! that. One offer, because it is the only one this state can honour.
//!
//! `--long` and `--cjk` are the adversarial app names the card's geometry is
//! measured against. `app_name` is `app::window_label`'s answer, so both are
//! strings a user can produce. The card's second line embeds the name
//! mid-sentence, which is the placement most likely to overflow -- so these
//! are the fixtures that show the `DT_END_ELLIPSIS` truncation, on a window
//! that cannot grow and cannot scroll.
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion,
//! the message pump and the owner-drawn button are the code the daemon ships.
//! What is a fixture is only the app's name -- nothing here reads a real
//! foreground window, and nothing on this path could reach a vault if it
//! wanted to.
//!
//! `--capturable` stubs exactly one seam -- `LockedCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture, which is the whole reason a screenshot of it comes out blank.

use deskwarden::locked_card::{self, LockedAnswer, LockedCalls, REAL};

/// Somewhere predictable on screen. Production's anchor is
/// `app::overlay_position`'s answer, computed from where the field the user is
/// in really is; a preview has no field, so it names a corner rather than
/// pretending to find one.
const ANCHOR: (f32, f32) = (320.0, 240.0);

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!("preview: capture exclusion is stubbed out, so this window can be screenshotted");
    }
    let app_name = if std::env::args().any(|a| a == "--long") {
        "Northwind Group Consolidated Accounts Portal (production)"
    } else if std::env::args().any(|a| a == "--cjk") {
        "株式会社ノースウィンド・コンソリデーテッド・アカウンツ"
    } else {
        "Ledgerline Desktop"
    };

    let answer = if capturable {
        // The one seam that is not the shipped one. `show_locked_card` cannot
        // be used here because it names `REAL` outright; this is that call
        // with `protect` swapped and every other pointer left alone.
        locked_card::ask_with(
            &LockedCalls {
                open: REAL.open,
                protect: |_| true,
                next: REAL.next,
                close: REAL.close,
            },
            app_name,
            Some(ANCHOR),
        )
    } else {
        locked_card::show_locked_card(app_name, Some(ANCHOR))
    };

    match answer {
        LockedAnswer::Unlock => println!("would open the master-password prompt"),
        LockedAnswer::Dismissed => println!("dismissed"),
    }
}

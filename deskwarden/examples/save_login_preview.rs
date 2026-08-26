//! Visual preview of the daemon's bare-Win32 save-a-login card -- design 3c.
//!
//! ```text
//! cargo run --example save_login_preview
//! cargo run --example save_login_preview -- --capturable
//! cargo run --example save_login_preview -- --returning
//! ```
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion, the
//! message pump, the two `EDIT` controls with their painted boxes and
//! placeholders, the owner-drawn buttons and the header lockup are the code the
//! daemon ships, so what this shows is what the user sees. What is a fixture is
//! only the app name in the App row and, with `--returning`, the values the card
//! opens on -- nothing here reaches `bw serve`, the network, the real vault, the
//! real clipboard or the user's `%APPDATA%`.
//!
//! `--capturable` stubs exactly one seam -- `SaveLoginCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen capture,
//! which on this card is not incidental: it is the surface a password is typed
//! into.
//!
//! `--returning` opens the card the way design 3d sends the user back to it: a
//! username already typed and a password already generated, which is the state
//! whose whole point is that neither is thrown away. The generated value here is
//! a fixture string, not a real secret, and it is printed back only by length.
//!
//! This example exists for the reason `examples/generate_preview.rs`,
//! `examples/picker_preview.rs` and `examples/unlock_prompt_preview.rs` do:
//! `examples/ui_preview.rs` walks egui surfaces through one `run_native`, and
//! this card is raw Win32 with no eframe anywhere -- which is the entire point
//! of it.

use deskwarden::save_login_card::{self, SaveLoginCalls, SaveLoginForm, REAL};
use zeroize::Zeroizing;

/// The app the card is pretending to have been opened in front of. A fixture --
/// nothing here reads a real foreground window.
const APP_NAME: &str = "Atlas Licence";

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!(
            "preview: capture exclusion is stubbed out, so this window can be screenshotted -- \
             which means whatever is typed into its password box is on screen for real"
        );
    }

    let form = if std::env::args().any(|arg| arg == "--returning") {
        eprintln!("preview: opening as design 3d hands the card back -- one row typed, one generated");
        SaveLoginForm {
            app_name: APP_NAME.to_string(),
            username: "ada@example.com".to_string(),
            password: Zeroizing::new("preview-generated-value".to_string()),
        }
    } else {
        SaveLoginForm::new(APP_NAME)
    };

    let answered = if capturable {
        // The one seam that is not the shipped one. `show_save_login_card`
        // cannot be used here because it names `REAL` outright; this is that
        // call with `protect` swapped and every other pointer left alone.
        let calls = SaveLoginCalls {
            open: REAL.open,
            protect: |_| true,
            next: REAL.next,
            take_form: REAL.take_form,
            close: REAL.close,
        };
        save_login_card::ask_with(&calls, form, None)
    } else {
        save_login_card::show_save_login_card(form, None)
    };

    // **The length, never the value.** These are fixtures, but the habit is the
    // point: printing a password to a terminal is a copy of it in a scrollback
    // buffer that outlives the process.
    match answered {
        Some((action, form)) => println!(
            "answered {action:?} with username {:?} and a {}-character password",
            form.username,
            form.password.chars().count()
        ),
        None => println!("the card could not be put on screen"),
    }
}

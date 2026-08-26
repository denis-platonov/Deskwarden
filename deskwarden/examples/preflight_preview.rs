//! Visual preview of the daemon's bare-Win32 send preflight -- design 4b.
//!
//! ```text
//! cargo run --example preflight_preview
//! cargo run --example preflight_preview -- --capturable
//! cargo run --example preflight_preview -- --refused
//! cargo run --example preflight_preview -- --long
//! ```
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion, the
//! message pump with its hold clock, the owner-drawn answers and the header
//! lockup are the code the daemon ships, so what this shows is what the user
//! sees. What is a fixture is only the [`SendTarget`] the card is asked about
//! and the sequence it lists -- nothing here reaches `bw serve`, the network,
//! the real vault, the real clipboard, a real foreground window or the user's
//! `%APPDATA%`.
//!
//! `--capturable` stubs exactly one seam -- `PreflightCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture.
//!
//! `--refused` opens the state that must never grow a way to send: a password
//! sequence aimed at the wrong process with an unmasked control, both facts
//! wrong at once, which is the case whose message has to name both.
//!
//! `--long` opens a sequence longer than `preflight_card::STEP_CAP`, which is
//! the shape that shows the overflow row saying how many steps it did not
//! list.
//!
//! **The clipboard seam is stubbed in every mode**, not only under
//! `--capturable`. A preview that put a fixture on the user's real clipboard
//! would evict whatever they had there, and this card's *Copy instead* is the
//! one control that writes outside the window.
//!
//! This example exists for the reason `examples/save_login_preview.rs`,
//! `examples/generate_preview.rs`, `examples/picker_preview.rs` and
//! `examples/unlock_prompt_preview.rs` do: `examples/ui_preview.rs` walks egui
//! surfaces through one `run_native`, and this card is raw Win32 with no eframe
//! anywhere -- which is the entire point of it.

use deskwarden::injector::target::SendTarget;
use deskwarden::key_sequence::ResolveSource;
use deskwarden::preflight_card::{self, PreflightCalls, REAL};
use deskwarden::vault_window::detail::TotpState;
use deskwarden::vault_window::preflight::PreflightState;
use zeroize::Zeroizing;

/// What *Copy instead* would put on the clipboard. **A fixture, not a
/// password**, and it is printed back only by length.
const PAYLOAD: &str = "preview-not-a-real-password";

/// The clipboard seam, stubbed. See the module doc: this runs in every mode.
fn no_clipboard(payload: &str) {
    eprintln!(
        "preview: *Copy instead* would have copied {} character(s); the real clipboard was \
         not touched",
        payload.chars().count()
    );
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let args: Vec<String> = std::env::args().collect();
    let capturable = args.iter().any(|a| a == "--capturable");
    let refused = args.iter().any(|a| a == "--refused");
    let long = args.iter().any(|a| a == "--long");

    if capturable {
        eprintln!(
            "preview: capture exclusion is stubbed out, so this window can be screenshotted"
        );
    }

    let target = if refused {
        SendTarget {
            title: "chat \u{2014} #finance".to_string(),
            image_name: "teams.exe".to_string(),
            pid: 5310,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: false,
        }
    } else {
        SendTarget {
            title: "Ledgerline \u{2014} Sign in".to_string(),
            image_name: "ledgerline.exe".to_string(),
            pid: 8124,
            class_name: "Chrome_WidgetWin_1".to_string(),
            focused_is_masked: true,
        }
    };

    let sequence = if long {
        "{USERNAME}{TAB}".repeat(9)
    } else {
        "{USERNAME}{TAB}{PASSWORD}{ENTER}".to_string()
    };

    let totp = TotpState::NoSecret;
    let state = PreflightState::new(
        target,
        "ledgerline.exe",
        &sequence,
        &ResolveSource {
            username: "ada@example.com",
            // A fixture. The step list is built with the eye shut -- see
            // `PreflightState::new` -- so this never reaches the screen, and
            // that is the property the preview is showing.
            password: "preview-not-a-real-password",
            custom: Vec::new(),
            totp: &totp,
        },
    );
    eprintln!(
        "preview: the card's verdict is {:?}",
        preflight_card::refusal_of(&state)
            .map(|why| format!("refused ({why:?})"))
            .unwrap_or_else(|| "allowed".to_string())
    );

    // The one shipped pointer that is swapped for a screenshot, plus the
    // clipboard, which is stubbed unconditionally. Every other seam is `REAL`.
    let calls = PreflightCalls {
        open: REAL.open,
        protect: if capturable { |_| true } else { REAL.protect },
        next: REAL.next,
        close: REAL.close,
        copy: no_clipboard,
    };

    let answered = preflight_card::ask_with(&calls, state, Zeroizing::new(PAYLOAD.to_string()));

    match answered {
        Some(action) => println!("answered {action:?}"),
        None => println!("the card could not be put on screen -- which reads as: do not send"),
    }
}

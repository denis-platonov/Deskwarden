//! Visual preview of the daemon's bare-Win32 unlock prompt.
//!
//! ```text
//! cargo run --example unlock_prompt_preview        # every call real EXCEPT `bw`
//! cargo run --example unlock_prompt_preview -- --real
//! ```
//!
//! The default swaps exactly one seam: [`PromptCalls::unlock`], which becomes a
//! function that always refuses. Everything else -- the window, the capture
//! exclusion, the message pump, the error line, the busy state -- is the code
//! the daemon ships, so what this shows is what the user sees. Nothing here
//! spawns `bw`, reads a profile directory or touches the real vault.
//!
//! `--real` runs the production [`ask`] against the active profile, which does
//! spawn `bw unlock`. That is the app's own behaviour and is for checking a
//! genuine unlock end to end.
//!
//! This example exists because `unlock_prompt` is the one shipping surface
//! `examples/ui_preview.rs` cannot draw: that file walks egui surfaces through
//! one `run_native`, and this window is raw Win32 with no eframe anywhere --
//! which is the entire point of it.

use deskwarden::unlock_prompt::{self, Outcome, PromptCalls, REAL};

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    let outcome = if std::env::args().any(|arg| arg == "--real") {
        eprintln!("running the REAL prompt: this will spawn `bw unlock`");
        unlock_prompt::ask(None)
    } else {
        eprintln!("preview: `bw` is stubbed out, so every password is refused");
        unlock_prompt::run_with(&PromptCalls {
            open: REAL.open,
            // Stubbed only when `--capturable` is passed, so the window can
            // be screenshotted: the real one is excluded from capture.
            protect: if capturable { |_| true } else { REAL.protect },
            next: REAL.next,
            take_password: REAL.take_password,
            show_error: REAL.show_error,
            busy: REAL.busy,
            // The only seam that is not the shipped one.
            unlock: |_| Err("Invalid master password.".to_string()),
            close: REAL.close,
        })
    };

    match outcome {
        Outcome::Unlocked(_) => println!("unlocked (session token withheld)"),
        Outcome::Cancelled => println!("cancelled"),
        Outcome::Unavailable => println!("the window could not be opened"),
    }
}

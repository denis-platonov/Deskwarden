//! Visual preview of the daemon's bare-Win32 password generator -- design 3d.
//!
//! ```text
//! cargo run --example generate_preview
//! cargo run --example generate_preview -- --capturable
//! cargo run --example generate_preview -- --slow
//! cargo run --example generate_preview -- --failing
//! ```
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion, the
//! message pump, the owner-drawn chips and buttons, the value box and the
//! footer are the code the daemon ships, so what this shows is what the user
//! sees. What is a fixture is only the *generator*: a local function that
//! builds a plausible-looking string, so nothing here reaches `bw serve`, the
//! network, the real vault or the user's `%APPDATA%`.
//!
//! `--capturable` stubs exactly one seam -- `GenerateCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture, which on this card is not incidental: it is the one surface in the
//! app that paints a live password in the clear, and that is the whole reason a
//! screenshot of it comes out blank.
//!
//! `--slow` puts a second into the fixture generator, which is what makes the
//! *Generating…* state -- painted before the round trip blocks -- long enough to
//! see. `--failing` makes it fail every time, which is the state whose sentence
//! must be the card's own and never the error's, and which *New* must be able to
//! leave.
//!
//! **The clipboard is the real one.** *Copy* goes through
//! `deskwarden::clipboard::copy_secret`, with its suppression formats and its
//! clearing timer, because a preview of a copy button that wrote somewhere else
//! would be a preview of a different control.
//!
//! This example exists for the reason `examples/picker_preview.rs` and
//! `examples/unlock_prompt_preview.rs` do: `examples/ui_preview.rs` walks egui
//! surfaces through one `run_native`, and this card is raw Win32 with no eframe
//! anywhere -- which is the entire point of it.

use deskwarden::generate_prompt::{self, GenerateCalls, REAL};
use deskwarden::vault_bridge::GenerateRequest;
use zeroize::Zeroizing;

/// The app the card is pretending to have been opened in front of. A fixture,
/// like the passwords are -- nothing here reads a real foreground window.
const APP_NAME: &str = "Ledgerline";

/// How the fixture generator should behave, set once from the command line.
///
/// A pair of `static`s rather than a closure's captures, because the generator
/// this example hands `run_with` is a plain `fn` -- the same shape the shipped
/// one has to fit through.
static SLOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FAILING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// **A made-up password, built here and never asked of anything.**
///
/// It honours the request's own size and alphabet so the card's readout and its
/// value box agree -- a preview whose "6 characters" produced twenty of them
/// would be showing a card that does not exist.
///
/// The bytes come from `std::time`, not from a cryptographic source, and that
/// is deliberate: nothing this produces is ever saved, and a preview that
/// reached for the crate's real generator would be a preview reaching for a
/// vault.
fn fixture_generator(request: &GenerateRequest) -> Result<Zeroizing<String>, String> {
    if SLOW.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(900));
    }
    if FAILING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("preview: the fixture generator was told to fail".to_string());
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(7);
    match request {
        GenerateRequest::Passphrase(recipe) => {
            const WORDS: [&str; 8] = [
                "harbour", "lantern", "quartz", "meadow", "signal", "thicket", "vellum", "willow",
            ];
            let mut out = String::new();
            for i in 0..recipe.words as usize {
                if i > 0 {
                    out.push('-');
                }
                out.push_str(WORDS[(seed + i * 3) % WORDS.len()]);
            }
            Ok(Zeroizing::new(out))
        }
        GenerateRequest::Password(recipe) => {
            let alphabet: &[u8] = if recipe.uppercase || recipe.lowercase {
                b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!@#$%^&*"
            } else {
                b"0123456789"
            };
            let mut out = String::new();
            for i in 0..recipe.length as usize {
                out.push(alphabet[(seed + i * 17) % alphabet.len()] as char);
            }
            Ok(Zeroizing::new(out))
        }
    }
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();

    let capturable = std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!(
            "preview: capture exclusion is stubbed out, so this window can be screenshotted -- \
             which means the password on it is on screen for real"
        );
    }
    if std::env::args().any(|arg| arg == "--slow") {
        SLOW.store(true, std::sync::atomic::Ordering::SeqCst);
        eprintln!("preview: the fixture generator takes ~0.9s, so the *Generating…* state is visible");
    }
    if std::env::args().any(|arg| arg == "--failing") {
        FAILING.store(true, std::sync::atomic::Ordering::SeqCst);
        eprintln!("preview: the fixture generator always fails -- press *New* to leave the state");
    }

    let kept = if capturable {
        // The one seam that is not the shipped one. `show_generate_prompt`
        // cannot be used here because it names `REAL` outright; this is that
        // call with `protect` swapped and every other pointer left alone -- and
        // it still goes through `ask_with`, which is the module's only route to
        // the password it parked.
        let calls = GenerateCalls {
            open: REAL.open,
            protect: |_| true,
            next: REAL.next,
            show: REAL.show,
            fill: REAL.fill,
            copy: REAL.copy,
            keep: REAL.keep,
            close: REAL.close,
        };
        generate_prompt::ask_with(&calls, APP_NAME, &fixture_generator)
    } else {
        generate_prompt::show_generate_prompt(APP_NAME, &fixture_generator)
    };

    // **The length, never the value.** This is a fixture, but the habit is the
    // point: printing a generated password to a terminal is a copy of it in a
    // scrollback buffer that outlives the process.
    match kept {
        Some(password) => {
            println!("would save a {}-character password to the vault", password.chars().count())
        }
        None => println!("dismissed -- nothing was kept"),
    }
}

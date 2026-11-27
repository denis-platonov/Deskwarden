//! Visual preview of the daemon's bare-Win32 password generator -- design 3d.
//!
//! ```text
//! cargo run --example generate_preview
//! cargo run --example generate_preview -- --capturable
//! cargo run --example generate_preview -- --slow
//! cargo run --example generate_preview -- --failing
//! cargo run --example generate_preview -- --shot out.png   # this window only, then exits
//! ```
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion, the
//! message pump, the owner-drawn segmented run and buttons, the length slider,
//! the character-class tiles, the minimum-digits stepper, the look-alikes
//! switch, the tinted value line, the strength badge and the footer band are
//! the code the daemon ships, so what this shows is what the user sees -- which
//! is design 3d's panel, at 3d's own numbers. What is a fixture is only the
//! *generator*: a local function that builds a plausible-looking string, so
//! nothing here reaches `bw serve`, the network, the real vault or the user's
//! `%APPDATA%`.
//!
//! **The 3d this draws is the owner's newer rendered image, not
//! `docs/design/Deskwarden.dc.html`'s `id="3d"`,** which is an older version of
//! the card. `crate::generate_prompt`'s module doc lists what differs and why
//! the design file is left alone.
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

// ---- `--shot PATH`: paint, save this window's own rect as a PNG, dismiss ----

static SHOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: isize, hdc: isize, flags: u32) -> i32;
}

/// Lets the card paint, saves ITS OWN window rect (never the screen) and answers
/// with the weakest answer.
fn shoot(window: isize) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowRect, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(900) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let Some(path) = SHOT.get() else { return };
    let hwnd = HWND(window as *mut c_void);
    let mut rc = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut rc);
        let (x, y, w, h) = (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        if PrintWindow(hwnd.0 as isize, mem.0 as isize, 2) == 0 {
            let _ = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        for i in 0..(w * h) as usize {
            pixels.swap(i * 4, i * 4 + 2);
            pixels[i * 4 + 3] = 255;
        }
        let file = std::fs::File::create(path).expect("create the png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut writer| writer.write_image_data(&pixels)).expect("write the png");
    }
}

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
/// It honours the request's own size and alphabet so the card's controls and
/// its value line agree -- a preview whose slider said 6 and produced twenty
/// would be showing a card that does not exist, and one that ignored the class
/// tiles would make them look broken.
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
            // Built from the classes the recipe actually asks for, so
            // unticking *Symbols* on the card visibly removes them from the
            // next value. The ambiguous characters (`O`, `0`, `l`, `1`, `I`)
            // are in the pools and taken back out when the switch asks for it,
            // which is what makes that control visible in a preview at all.
            let mut alphabet = String::new();
            if recipe.uppercase {
                alphabet.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
            }
            if recipe.lowercase {
                alphabet.push_str("abcdefghijklmnopqrstuvwxyz");
            }
            if recipe.number {
                alphabet.push_str("0123456789");
            }
            if recipe.special {
                alphabet.push_str("!@#$%^&*");
            }
            if recipe.avoid_ambiguous {
                alphabet.retain(|c| !"O0lI1".contains(c));
            }
            // A recipe with no classes at all cannot be built by the card --
            // `CharClasses` makes it unrepresentable -- but a fixture that
            // divided by zero on one would be a preview that crashed instead
            // of saying so.
            let alphabet: Vec<char> = alphabet.chars().collect();
            if alphabet.is_empty() {
                return Err("preview: the request asked for no characters at all".to_string());
            }
            let mut out = String::new();
            for i in 0..recipe.length as usize {
                out.push(alphabet[(seed + i * 17) % alphabet.len()]);
            }
            Ok(Zeroizing::new(out))
        }
    }
}

/// The previews' shared `--dark` / `--light` flag and "copy out ONLY the card's own
/// window rect" capture. (Inline in each example: this crate's guards refuse a
/// shared file pulled in by attribute or macro.)
#[allow(dead_code)]
mod shot {
    use std::ffi::c_void;
    use std::path::Path;

    /// Honours `--dark` and `--light` on the command line: the card then paints
    /// that theme whatever `settings.json` says. Without either, the saved
    /// setting decides, which is what ships.
    pub fn apply_theme_flag() {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--dark") {
            deskwarden::gdi_palette::force_for_preview(true);
        } else if args.iter().any(|a| a == "--light") {
            deskwarden::gdi_palette::force_for_preview(false);
        }
    }

    /// The value after `flag`, if the flag is there.
    pub fn value(flag: &str) -> Option<String> {
        let args: Vec<String> = std::env::args().collect();
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
    }

    /// Dispatches what is queued for this thread's windows for `ms` milliseconds,
    /// so the card has painted before it is copied.
    pub fn pump(ms: u64) {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(ms) {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[link(name = "user32")]
    extern "system" {
        fn PrintWindow(hwnd: isize, hdc: isize, flags: u32) -> i32;
    }

    /// Lets the card (`hwnd`) paint, then saves ONLY its own window rect to
    /// `path` as a PNG.
    pub fn save_window(hwnd: isize, path: &Path) {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
        };
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        pump(900);
        let window = HWND(hwnd as *mut c_void);
        let mut rc = RECT::default();
        unsafe {
            let _ = GetWindowRect(window, &mut rc);
        }
        let (x, y, w, h) = (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || h <= 0 {
            eprintln!("shot: the card has no size");
            return;
        }
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp);
            // PrintWindow asks the window to render itself, so the copy does not
            // depend on what is in front of it.
            if PrintWindow(hwnd, mem.0 as isize, 2) == 0 {
                let _ = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
            }
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            for i in 0..(w * h) as usize {
                pixels.swap(i * 4, i * 4 + 2);
                pixels[i * 4 + 3] = 255;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let file = std::fs::File::create(path).expect("create the png");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().and_then(|mut writer| writer.write_image_data(&pixels)).expect("write the png");
        }
    }
}

fn main() {
    shot::apply_theme_flag();
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

    let shot = std::env::args().skip_while(|a| a != "--shot").nth(1);
    if let Some(path) = &shot {
        let _ = SHOT.set(path.into());
    }

    let kept = if capturable || shot.is_some() {
        // The one seam that is not the shipped one. `show_generate_prompt`
        // cannot be used here because it names `REAL` outright; this is that
        // call with `protect` swapped and every other pointer left alone -- and
        // it still goes through `ask_with`, which is the module's only route to
        // the password it parked.
        let calls = GenerateCalls {
            open: REAL.open,
            protect: |_| true,
            next: if shot.is_some() {
                |w| {
                    shoot(w.0);
                    generate_prompt::Event::Cancel
                }
            } else {
                REAL.next
            },
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

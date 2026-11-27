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
use deskwarden::prompt_card::OverlayMatch;
use deskwarden::prompt_card::{self, Event, PromptCalls, PromptWindow, REAL};

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

/// `--shot PATH`: where the card's own window rect goes.
static SHOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The shots-mode pump: lets the card paint, saves its own rect, cancels.
fn shoot(window: PromptWindow) -> Event {
    if let Some(path) = SHOT.get() {
        shot::save_window(window.0, path);
    }
    Event::Cancel
}

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
    shot::apply_theme_flag();

    let shooting = shot::value("--shot");
    if let Some(path) = &shooting {
        let _ = SHOT.set(path.into());
    }
    let capturable = shooting.is_some() || std::env::args().any(|arg| arg == "--capturable");
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
                next: if shooting.is_some() { shoot } else { REAL.next },
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

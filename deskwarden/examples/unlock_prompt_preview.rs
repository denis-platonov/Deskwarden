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

use deskwarden::unlock_prompt::{self, Event, Outcome, PromptCalls, PromptWindow, REAL};

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
        shot::save_window(window.top_level, path);
    }
    Event::Cancel
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();
    shot::apply_theme_flag();

    let shooting = shot::value("--shot");
    if let Some(path) = &shooting {
        let _ = SHOT.set(path.into());
    }
    let capturable = shooting.is_some() || std::env::args().any(|arg| arg == "--capturable");
    // `--app` opens it as the locked path does: beside a field, naming the app.
    if std::env::args().any(|arg| arg == "--app") {
        let _ = unlock_prompt::for_locked_window("Ledgerline Desktop", Some((320.0, 240.0)));
    }
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
            next: if shooting.is_some() { shoot } else { REAL.next },
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

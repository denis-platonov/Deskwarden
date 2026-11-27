//! Visual preview of the daemon's bare-Win32 tray panel.
//!
//! ```text
//! cargo run --example tray_preview                 # the open panel, real calls
//! cargo run --example tray_preview -- --locked     # the Locked state
//! cargo run --example tray_preview -- --dark       # whatever the settings say, forced dark
//! cargo run --example tray_preview -- --shot PATH  # the card's own window rect to PATH, then exit
//! ```
//!
//! Every Win32 call is the real one; the fixture is the vault (four made-up
//! logins) and the copy, which touches no clipboard and no secret. With
//! `--shot` the capture exclusion is stubbed so the rect can be read back and
//! the pump is replaced by one that lets the card paint, saves ONLY the card's
//! own window rect, and closes it.

use deskwarden::tray_popup::{self, Event, Item, Listing, Popup, PopupCalls, PopupWindow, Status, Sync, Vault};

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

static SHOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

fn shoot(window: PopupWindow) -> Event {
    if let Some(path) = SHOT.get() {
        shot::save_window(window.0, path);
    }
    Event::Cancel
}

fn item(id: &str, name: &str, username: &str) -> Item {
    Item { id: id.into(), name: name.into(), username: username.into(), has_password: true }
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();
    shot::apply_theme_flag();

    let shooting = shot::value("--shot");
    if let Some(path) = &shooting {
        let _ = SHOT.set(path.into());
    }
    let vault = if std::env::args().any(|a| a == "--locked") {
        Vault::Locked
    } else {
        Vault::Open(Listing {
            items: vec![
                item("1", "SAP Production", "a.novak"),
                item("2", "Ledgerline", "a.novak@ledgerline.com"),
                item("3", "Atlas Studio", "a.novak@studio.atlas.com"),
                item("4", "Vantage VPN", "a.novak@vantage.io"),
            ],
            fill_counts: Default::default(),
        })
    };
    let status = Status { sync: Sync::Synced, auto_lock_minutes: Some(14) };
    let real = tray_popup::real_calls();
    let calls = PopupCalls {
        open: real.open,
        protect: if shooting.is_some() { |_| true } else { real.protect },
        next: if shooting.is_some() { shoot } else { real.next },
        show: real.show,
        close: real.close,
    };
    let outcome = tray_popup::run_with(&calls, Popup::new(vault, status), &mut |_, _| tray_popup::CopyResult::Copied);
    println!("{outcome:?}");
}

//! Visual preview of the daemon's bare-Win32 save-a-login card -- design 3c.
//!
//! ```text
//! cargo run --example save_login_preview
//! cargo run --example save_login_preview -- --capturable
//! cargo run --example save_login_preview -- --returning
//! cargo run --example save_login_preview -- --shot out.png   # this window only, then exits
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

    let shot = std::env::args().skip_while(|a| a != "--shot").nth(1);
    if let Some(path) = &shot {
        let _ = SHOT.set(path.into());
    }

    let answered = if capturable || shot.is_some() {
        // The one seam that is not the shipped one. `show_save_login_card`
        // cannot be used here because it names `REAL` outright; this is that
        // call with `protect` swapped and every other pointer left alone.
        let calls = SaveLoginCalls {
            open: REAL.open,
            protect: |_| true,
            next: if shot.is_some() {
                |w| {
                    shoot(w.0);
                    save_login_card::Event::Cancel
                }
            } else {
                REAL.next
            },
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

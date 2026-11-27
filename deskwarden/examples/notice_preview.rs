//! Visual preview of the bare-Win32 notice and confirm cards that replaced the
//! system's `MessageBox` everywhere in the app.
//!
//! ```text
//! cargo run --example notice_preview                       # every variant, one after another
//! cargo run --example notice_preview -- --variant remove   # one variant, interactive
//! cargo run --example notice_preview -- --shots DIR        # every variant to DIR/<variant>.png
//! ```
//!
//! Variants: `info` (an OK-only notice with a caller-given title), `bare` (the
//! same under the bare "Deskwarden" title, which the header already says),
//! `remove` (the account-removal confirm: danger button, opens on Cancel),
//! `reinstall` (OK / Cancel, opens on Cancel), `cli` (the unrecognised-CLI
//! Yes / No confirm, the longest body in the app), `refusal` (the fill path's
//! own notice).
//!
//! Every Win32 call is the real one; the only fixture is the content, and in
//! `--shots` mode the pump, which is replaced by one that lets the card paint,
//! copies out **only the card's own window rect**, and answers Dismiss. The
//! capture exclusion is stubbed in this example so that rect can be read back.

use deskwarden::notice_card::{Ask, Event, Notice, NoticeCalls, NoticeWindow, Outcome, REAL};
use std::ffi::c_void;
use std::sync::OnceLock;

static SHOT: OnceLock<std::path::PathBuf> = OnceLock::new();

fn variants() -> Vec<(&'static str, Notice)> {
    use deskwarden::i18n::{t, tf};
    let removal_body = tf("main.remove_account_warning", &[("label", "ana@example.com")]);
    let cli_body = tf(
        "main.cli_unrecognized_body",
        &[
            ("headline", "is signed by an organization Deskwarden does not recognize"),
            ("file", "C:\\Users\\ana\\scoop\\apps\\bitwarden-cli\\current\\bw.exe"),
            ("detail", "Signed by: Example Software Ltd."),
        ],
    );
    vec![
        (
            "info",
            Notice::info(
                "Deskwarden: could not switch",
                "Could not switch to ana@example.com.\n\nThe server did not answer.\n\nYou are still on the previous account.",
            ),
        ),
        ("bare", Notice::info("Deskwarden", "Another Deskwarden is already running on this computer.")),
        (
            "remove",
            Notice::ask(
                t("main.remove_account_title"),
                &removal_body,
                Ask {
                    accept: t("notice_card.remove").to_string(),
                    decline: t("notice_card.cancel").to_string(),
                    danger: true,
                    default_accept: false,
                },
            ),
        ),
        (
            "reinstall",
            Notice::ask(
                "Deskwarden: the Bitwarden CLI is missing",
                "Your vault is served by the official Bitwarden CLI, and it is no longer on this computer.\n\nPress OK to reinstall it now, or Cancel to start without your vault.",
                Ask {
                    accept: t("notice_card.ok").to_string(),
                    decline: t("notice_card.cancel").to_string(),
                    danger: false,
                    default_accept: false,
                },
            ),
        ),
        (
            "cli",
            Notice::ask(
                t("main.cli_unrecognized_title"),
                &cli_body,
                Ask {
                    accept: t("notice_card.yes").to_string(),
                    decline: t("notice_card.no").to_string(),
                    danger: false,
                    default_accept: false,
                },
            ),
        ),
        (
            "refusal",
            deskwarden::notice_card::notice_for(
                "Nothing sent. The window in front is not the one this item's rule was written for. Deskwarden will not type a password there.",
            ),
        ),
    ]
}

/// The shots-mode pump: lets the card paint, saves its own rect, answers.
fn shoot(window: NoticeWindow) -> Event {
    use windows::Win32::Foundation::{HWND, RECT};
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
    let hwnd = HWND(window.0 as *mut c_void);
    let mut rc = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut rc);
    }
    if let Some(path) = SHOT.get() {
        save_rect(hwnd, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, path);
    }
    Event::Dismiss
}

#[link(name = "user32")]
extern "system" {
    fn PrintWindow(hwnd: isize, hdc: isize, flags: u32) -> i32;
}

/// Copies the screen rectangle `(x, y, w, h)` -- the card's own window -- to a PNG.
fn save_rect(hwnd: windows::Win32::Foundation::HWND, x: i32, y: i32, w: i32, h: i32, path: &std::path::Path) {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        //  asks the window to render itself, so the copy does not
        // depend on what is in front of it or on which desktop it is on.
        let printed = PrintWindow(hwnd.0 as isize, mem.0 as isize, 2) != 0;
        if !printed {
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
        // BGRA to RGBA, opaque.
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

fn run(name: &str, notice: &Notice, shots: Option<&std::path::Path>) {
    let calls = NoticeCalls {
        open: REAL.open,
        // Stubbed so the card can be copied off the screen.
        protect: |_| true,
        next: if shots.is_some() { shoot } else { REAL.next },
        close: REAL.close,
    };
    let outcome = deskwarden::notice_card::run_with(&calls, notice);
    println!("{name}: {}", match outcome {
        Outcome::Dismissed => "dismissed",
        Outcome::Accepted => "accepted",
        Outcome::Declined => "declined",
        Outcome::Unavailable => "the card could not be opened",
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let shots = value("--shots").map(std::path::PathBuf::from);
    let only = value("--variant");
    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).expect("create the shots directory");
    }
    for (name, notice) in variants() {
        if only.as_deref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        match (&shots, &only) {
            // The shot's path is set once per process, so a PNG needs one
            // run per variant -- and this crate starts no child process
            // outside `job_object::spawn_in_job`, examples included.
            (Some(_), None) => {
                eprintln!("--shots needs --variant {name} (one run per variant)");
            }
            (Some(dir), Some(_)) => {
                let _ = SHOT.set(dir.join(format!("{name}.png")));
                run(name, &notice, shots.as_deref());
            }
            (None, _) => run(name, &notice, None),
        }
    }
}

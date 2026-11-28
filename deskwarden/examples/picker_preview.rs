//! Visual preview of the daemon's bare-Win32 account picker.
//!
//! ```text
//! cargo run --example picker_preview
//! cargo run --example picker_preview -- --capturable
//! cargo run --example picker_preview -- --few
//! cargo run --example picker_preview -- --full
//! cargo run --example picker_preview -- --empty
//! cargo run --example picker_preview -- --shot out.png --search north
//! ```
//!
//! `--few` shows a populated card with **two** candidates, which is the state
//! the *Search the vault* row was invisible in: nothing overflows, so for a
//! while the row was not drawn and two wrong guesses had no way out but
//! dismissing the card. The default fixture list overflows, so this flag is
//! the one that shows the row saying "Look for it under another name".
//!
//! `--full` shows **exactly `ROW_CAP` candidates**: the card at its fullest
//! without a truncation. This is the state the regression was in -- while the
//! *Search the vault* row took one of the candidate cap's slots, a list of
//! exactly five showed four of them and said the rest did not fit. The card
//! is one row taller than the candidate cap now, so this fixture shows all
//! five accounts, the search row under them saying nothing was cut, and every
//! row's bare-digit chip.
//!
//! **The keyboard shortcuts are on the card itself, and they are bare keys**:
//! each candidate row carries the digit that fills from it (`1`...`9`), the
//! *Search the vault* row carries `S`, and *New login* carries `N` inside its
//! own button. `Esc` cancels, as it always has. Any fixture shows them;
//! `--full` shows the whole run of digits at once. The `CTRL+ALT` chords they
//! replaced are gone: the card is focused and temporary, and on German and
//! Polish layouts `CTRL+ALT` is `AltGr`, so `CTRL+ALT+2` was the character
//! `@` -- untypable in a search box that has to find an email address.
//!
//! **Search mode takes no bare keys**, because its text box has the keyboard:
//! digits and letters type, `Up`/`Down` move the highlight over the results,
//! and `Enter` chooses the highlighted one. It shows no key chips at all for
//! that reason.
//!
//! **Search happens on this card.** Clicking *Search the vault* -- the last row
//! of any populated card, or *Search vault* on the empty one, or pressing `S`,
//! which clicks that same row -- switches the same window into its search
//! mode: a focused text box and the results beneath it, **inside one bordered
//! field**, with no border of its own on either. Results are drawn as rows by
//! the same painter the candidates use. Picking one leads to the same *What should I type?* step. There is no
//! second window and no egui anywhere on the path; the mode that used to answer
//! this row opened the ~76 MB vault window to search a vault the daemon
//! already held in memory. Any fixture reaches it -- the example's vault is
//! `vault_fixture`, deliberately larger than the card's cap so the overflow
//! notice is visible on an empty query.
//!
//! `--empty` shows the card's **empty mode**: the surface a user gets when
//! nothing in the vault looks like the app they are in front of, which is by
//! far the most common state this hotkey lands in. It is design 3a's content
//! -- the app's name, the line saying there is no saved login for it, and the
//! two offers *New login* and *Search vault* -- drawn by this card rather
//! than by the egui window it replaced, which cost ~102 MB to say it. The two
//! flags combine: `-- --empty --capturable` is the one to screenshot.
//!
//! **Every Win32 call is the real one.** The window, the capture exclusion,
//! the message pump, the owner-drawn rows and the field palette are the code
//! the daemon ships, so what this shows is what the user sees. What is a
//! fixture is only the *content*: a handful of made-up candidates and their
//! palettes, so that nothing here reads the real vault, spawns `bw`, touches
//! the network or opens the on-disk favicon cache.
//!
//! `--capturable` stubs exactly one seam -- `PickerCalls::protect` -- so the
//! window can be screenshotted. The shipped one excludes it from screen
//! capture, which is the whole reason a screenshot of it comes out blank.
//!
//! This example exists for the reason `examples/unlock_prompt_preview.rs`
//! does: `examples/ui_preview.rs` walks egui surfaces through one
//! `run_native`, and this card is raw Win32 with no eframe anywhere -- which
//! is the entire point of it.

use deskwarden::app_candidates::Candidate;
use deskwarden::key_sequence::FieldRef;
use deskwarden::picker_prompt::{self, Event, Offer, Outcome, Palette, PickerCalls, PickerWindow, REAL};

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
fn shoot(window: PickerWindow) -> Event {
    if let Some(path) = SHOT.get() {
        shot::save_window(window.0, path);
    }
    Event::Cancel
}

/// `--search QUERY`: opens the card's search mode and types QUERY into it
/// (`north` finds three, `zzz` none, `a` more than the cap), then shoots.
static QUERY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The shots-mode pump for `--search`: click the *Search the vault* row, type
/// the query into the card's own text box, then shoot.
fn shoot_search(window: PickerWindow) -> Event {
    use std::sync::atomic::Ordering;
    match STEP.fetch_add(1, Ordering::SeqCst) {
        0 => {
            shot::pump(300);
            Event::Search
        }
        1 => {
            shot::pump(300);
            let query = QUERY.get().cloned().unwrap_or_default();
            unsafe {
                use windows::core::{w, HSTRING};
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, SetWindowTextW};
                let top = HWND(window.0 as *mut std::ffi::c_void);
                if let Ok(edit) = FindWindowExW(top, None, w!("EDIT"), None) {
                    let _ = SetWindowTextW(edit, &HSTRING::from(query.as_str()));
                }
            }
            Event::Typed(query)
        }
        _ => shoot(window),
    }
}

/// The app the card is pretending to have been opened in front of. A fixture
/// like the candidates are -- nothing here reads a real foreground window.
///
/// **`Ledgerline` and not `Ledgerline.exe`**, because that is now what the
/// daemon hands `ask`: `app::handle_no_match` reads the foreground
/// executable's `FileDescription` through `app_identity::probe_display_name`
/// and falls back to `app::window_label` only when there is none. The card's
/// whole message is built out of this one string, so the file name is exactly
/// what it must not look like.
const APP_NAME: &str = "Ledgerline";

fn offer(id: &str, name: &str, username: &str, palette: Palette) -> Offer {
    Offer {
        candidate: Candidate {
            id: id.to_string(),
            name: name.to_string(),
            username: username.to_string(),
        },
        palette,
        // No icon: the on-disk favicon cache belongs to the installed app's
        // cache directory, and a preview that read it would be a preview
        // touching the user's real data to draw a fixture. Rows without an
        // icon are a shipped state -- an item with no URI never has one.
        icon: None,
    }
}

/// Six candidates against a cap of five, deliberately: that is the truncating
/// case, so the preview shows the *Search the vault* row saying candidates
/// were dropped. `few_fixtures` is the other half of the same row's story.
fn fixtures() -> Vec<Offer> {
    let login = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password],
        has_sequence: false,
    };
    let with_totp = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password, FieldRef::Totp],
        has_sequence: false,
    };
    vec![
        offer("id-1", "Slack", "ada@example.com", with_totp()),
        // Deliberately far too long for the row: `win32_draw::draw_row` draws
        // both lines with `DT_END_ELLIPSIS`, so this one is the fixture that
        // shows the truncation ending in "..." rather than cut mid-letter.
        offer(
            "id-2",
            "Northwind Group Consolidated Accounts Portal (production)",
            "ada.lovelace@accounts.northwind-group-consolidated.example",
            login(),
        ),
        offer("id-3", "Atlas Licence", "ada", Palette { fields: vec![], has_sequence: true }),
        offer("id-4", "Ledgerline", "accounts@northwind.example", login()),
        offer("id-5", "Speedtest", "ada@example.com", Palette {
            fields: vec![FieldRef::Password],
            has_sequence: false,
        }),
        offer("id-6", "Northwind VPN", "ada@example.com", login()),
    ]
}

/// **Exactly the candidate cap, and so no truncation at all.**
///
/// `picker_prompt::ROW_CAP` candidates against a card that lays out
/// `LIST_ROWS = ROW_CAP + 1` rows: every account is on screen, the *Search the
/// vault* row sits under them saying nothing was cut, and the last digit chip
/// on the card is `CTRL+ALT+5`. While the search row competed with the
/// candidates for a slot, this fixture drew four accounts and claimed the
/// fifth did not fit.
fn full_fixtures() -> Vec<Offer> {
    fixtures().into_iter().take(picker_prompt::ROW_CAP).collect()
}

/// **Two candidates, both plausibly wrong.** The reported state: the matcher
/// is loose on purpose, so this card is ordinary -- and it must still offer
/// the *Search the vault* row, here saying nothing was cut.
fn few_fixtures() -> Vec<Offer> {
    fixtures().into_iter().take(2).collect()
}

fn main() {
    env_logger::builder().filter_level(log::LevelFilter::Info).init();
    shot::apply_theme_flag();

    let shooting = shot::value("--shot");
    if let Some(path) = &shooting {
        let _ = SHOT.set(path.into());
    }
    if let Some(query) = shot::value("--search") {
        let _ = QUERY.set(query);
    }
    let capturable = shooting.is_some() || std::env::args().any(|arg| arg == "--capturable");
    if capturable {
        eprintln!("preview: capture exclusion is stubbed out, so this window can be screenshotted");
    }
    let full = std::env::args().any(|arg| arg == "--full");
    if full {
        eprintln!(
            "preview: exactly {} candidates -- all of them shown, and no truncation reported",
            picker_prompt::ROW_CAP
        );
    }
    let few = std::env::args().any(|arg| arg == "--few");
    if few {
        eprintln!("preview: two candidates -- nothing overflows, and the search row is still there");
    }
    let empty = std::env::args().any(|arg| arg == "--empty");
    if empty {
        eprintln!("preview: the empty card -- no account matched, which is design 3a's state");
    }

    // An empty offer list is not "no preview": it is the card's third mode,
    // and it goes through exactly the same `ask`/`run_with` the populated one
    // does. See `picker_prompt::empty_rows`.
    let offers = if empty {
        Vec::new()
    } else if few {
        few_fixtures()
    } else if full {
        full_fixtures()
    } else {
        fixtures()
    };
    let outcome = if capturable {
        // The one seam that is not the shipped one. `ask` cannot be used here
        // because it names `REAL` outright; this is that call with `protect`
        // swapped and every other pointer left alone.
        picker_prompt::run_with(
            &PickerCalls {
                open: REAL.open,
                protect: |_| true,
                next: if shooting.is_some() && QUERY.get().is_some() { shoot_search } else if shooting.is_some() { shoot } else { REAL.next },
                show_palette: REAL.show_palette,
                show_search: REAL.show_search,
                close: REAL.close,
            },
            &offers.iter().map(|o| o.candidate.clone()).collect::<Vec<_>>(),
            APP_NAME,
            palette_of_fixture,
            search_fixture_vault,
        )
    } else {
        picker_prompt::ask(&offers, APP_NAME, search_fixture_vault)
    };

    match outcome {
        Outcome::Fill { id, send } => {
            let (label, _) = picker_prompt::send_label(&send);
            println!("would fill vault item {id} with: {label}");
        }
        Outcome::NewLogin => println!("new login"),
        Outcome::Edit(id) => println!("edit the binding for vault item {id}"),
        Outcome::Cancelled => println!("cancelled"),
        Outcome::Unavailable => println!("the window could not be opened"),
    }
}

/// **A made-up vault for the card's search mode**, wider than the candidate
/// fixtures so the mode's own cap and its overflow notice are both reachable:
/// an empty query matches all of these, which is more than
/// `picker_prompt::SEARCH_CAP`, so the card opens the mode already saying how
/// many it is not showing. Typing `north` narrows it to a handful; typing
/// `zzz` shows the *No matches* row.
fn vault_fixture() -> Vec<Offer> {
    let login = || Palette {
        fields: vec![FieldRef::Username, FieldRef::Password],
        has_sequence: false,
    };
    let mut vault = fixtures();
    for (i, name) in [
        "Companies House",
        "Fastmail",
        "GitHub",
        "Northwind Payroll",
        "Post Office",
        "Railcard",
        "Stripe",
        "Water board",
    ]
    .iter()
    .enumerate()
    {
        vault.push(offer(
            &format!("id-{}", 100 + i),
            name,
            "ada@example.com",
            login(),
        ));
    }
    vault
}

/// The example's [`picker_prompt::Searcher`].
///
/// A bare `fn` pointer, so it cannot close over the fixtures and rebuilds them
/// -- which is what the shipped `app::search_parked_vault` does through its own
/// parked slice. The predicate is the shipped one:
/// `picker_ui::name_matches_filter`, the body of the vault window's own
/// `item_matches_filter`, so this preview filters exactly as the daemon does.
fn search_fixture_vault(query: &str, cap: usize) -> picker_prompt::SearchResults {
    let filter = query.trim().to_lowercase();
    let mut offers = Vec::new();
    let mut total = 0usize;
    for candidate in vault_fixture() {
        if !deskwarden::picker_ui::name_matches_filter(&candidate.candidate.name, &filter) {
            continue;
        }
        total += 1;
        if offers.len() < cap {
            offers.push(candidate);
        }
    }
    picker_prompt::SearchResults { offers, total }
}

/// `run_with`'s palette argument is a bare `fn` pointer, so it cannot close
/// over the fixtures -- it rebuilds them and looks the id up, which is exactly
/// what the shipped `picker_prompt::ask` does through its own parked slice.
fn palette_of_fixture(id: &str) -> Palette {
    // `few_fixtures` is a prefix of `fixtures`, so one lookup serves both.
    fixtures()
        .into_iter()
        .find(|o| o.candidate.id == id)
        .map(|o| o.palette)
        .unwrap_or(Palette { fields: vec![], has_sequence: false })
}

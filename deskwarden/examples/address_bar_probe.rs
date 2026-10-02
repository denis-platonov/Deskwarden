//! **The hand check for `browser_url::read`**, which no test can run.
//!
//! Run it, switch to a browser window within five seconds, and it prints what
//! Deskwarden would rank the account picker by: the address it read, or
//! "nothing" -- and whether that address is the browser's whole one or was
//! built on elided text (`browser_url::PageAddress::complete`). It prints to
//! this console only -- the app itself never logs the address.
//!
//! ```text
//! cargo run --example address_bar_probe
//! ```
//!
//! # What it prints, and what to measure with it
//!
//! Before the address, one line per element the address-field query matched,
//! kept or refused: how many steps it took to walk up to the window (or that
//! it did not get there within `browser_url::MAX_FIELD_DEPTH`), whether a
//! `Document` was crossed on the way, and its `ClassName` and `AutomationId`
//! -- the keys `browser_url::Family::address_field` matches on. **Never a
//! candidate's text**: a refused candidate may be a web page's own input, and
//! its value is whatever was typed into that page. The one text this prints is
//! the address finally read, below the list.
//!
//! The field depths `browser_url::MAX_FIELD_DEPTH` rests on were measured in
//! ordinary tabbed windows only. Measure these two as well, and report the
//! lines if a kept field is deeper than the tabbed window's, or if a page's
//! field is ever kept:
//!
//! * **A Firefox popup window**, such as an OAuth sign-in opened by a page
//!   with `window.open`. Firefox can expose page content WITHOUT a `Document`
//!   ancestor, so in Firefox the depth bound is the only thing keeping a
//!   page's `<input id="urlbar-input">` from passing as the address bar --
//!   and a popup draws a slimmer toolbar, which may nest things differently.
//! * **A Chrome app / PWA window** (an installed site, or `--app=`), whose
//!   window has no tab strip and a minimal or absent omnibox.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

fn main() {
    println!("Switch to a browser window. Reading its address bar in 5 seconds...");
    std::thread::sleep(std::time::Duration::from_secs(5));
    let Some(window) = deskwarden::window_watch::current_foreground_event() else {
        println!("There is no foreground window.");
        return;
    };
    println!(
        "Foreground: {} ({:?})",
        window.exe_name,
        deskwarden::app::browser_window(&window.exe_name)
    );
    describe_candidates(window.hwnd);
    match deskwarden::browser_url::read(window.hwnd) {
        Some(page) => println!(
            "Address bar: {} (complete: {}{})",
            page.url,
            page.complete,
            if page.complete { "" } else { ", the scheme was guessed from elided text" }
        ),
        None => println!(
            "Address bar: nothing (not a supported browser, no address field found, not a web \
             address, or typed text over a page on another host)"
        ),
    }
}

/// The diagnostic: every element the address-field query matched, and why
/// `browser_url::is_browser_chrome` keeps or refuses it. Never its text.
fn describe_candidates(hwnd: isize) {
    let Some(class) = class_name_of(hwnd) else {
        println!("Window class: unknown (Win32 declined to say)");
        return;
    };
    let Some(family) = deskwarden::browser_url::family_of_class(&class) else {
        println!("Window class: {class} (no browser family; nothing is queried)");
        return;
    };
    println!("Window class: {class} ({family:?})");
    let candidates = match deskwarden::injector::ui_automation::address_field_candidates(
        hwnd,
        family.address_field(),
    ) {
        Ok(candidates) => candidates,
        Err(error) => {
            println!("Candidates: the UI Automation query failed: {error:?}");
            return;
        }
    };
    println!("Candidates: {}", candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        let kept =
            deskwarden::browser_url::is_browser_chrome(candidate.depth, candidate.crosses_a_document);
        let depth = match candidate.depth {
            Some(depth) => format!("depth {depth}"),
            None => format!(
                "depth > {} (or the walk failed)",
                deskwarden::browser_url::MAX_FIELD_DEPTH
            ),
        };
        println!(
            "  #{index} {}: {depth}, {}, ClassName {:?}, AutomationId {:?}",
            if kept { "kept   " } else { "refused" },
            if candidate.crosses_a_document { "inside a Document" } else { "no Document" },
            candidate.class_name,
            candidate.automation_id,
        );
    }
}

/// The window class of `hwnd`. The crate's own `injector::target::class_name_of`
/// is `pub(crate)`, and this is one Win32 call, so it is asked again here
/// rather than widening that function for an example.
fn class_name_of(hwnd: isize) -> Option<String> {
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is a live stack array whose length is handed to the
    // call; a window that has closed makes the call return 0.
    let copied = unsafe { GetClassNameW(HWND(hwnd as *mut core::ffi::c_void), &mut buffer) };
    if copied <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..copied as usize]))
}

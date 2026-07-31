//! Probes whether the vault window's taskbar icon actually reaches Windows.
//!
//! Opens a window configured exactly like `vault_window::run`'s (frameless,
//! resizable, `theme::window_icon()`) and, once it is up, asks Windows what
//! icon that window reports via `WM_GETICON` and its class icon -- the same
//! things the taskbar reads. Prints the answer and closes itself.
//!
//! Run with: `cargo run --example icon_probe`

use deskwarden::theme;
use eframe::egui;

const TITLE: &str = "Deskwarden icon probe";

fn main() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 200.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let mut frames = 0u32;
    let _ = eframe::run_ui_native(TITLE, options, move |ui, _frame| {
        ui.label("probing…");
        frames += 1;
        // Give winit a few frames to create the window and apply the icon
        // before asking Windows about it.
        if frames == 10 {
            report();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint();
    });
}

fn report() {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetClassLongPtrW, SendMessageW, GCLP_HICON, GCLP_HICONSM, ICON_BIG,
        ICON_SMALL, WM_GETICON,
    };

    unsafe {
        let hwnd: HWND = match FindWindowW(PCWSTR::null(), &HSTRING::from(TITLE)) {
            Ok(hwnd) if !hwnd.is_invalid() => hwnd,
            _ => {
                println!("PROBE: could not find the window by title");
                return;
            }
        };

        let big = SendMessageW(hwnd, WM_GETICON, WPARAM(ICON_BIG as usize), LPARAM(0));
        let small = SendMessageW(hwnd, WM_GETICON, WPARAM(ICON_SMALL as usize), LPARAM(0));
        let class_big = GetClassLongPtrW(hwnd, GCLP_HICON);
        let class_small = GetClassLongPtrW(hwnd, GCLP_HICONSM);

        println!("PROBE: WM_GETICON ICON_BIG   = {:#x}", big.0);
        println!("PROBE: WM_GETICON ICON_SMALL = {:#x}", small.0);
        println!("PROBE: GCLP_HICON            = {class_big:#x}");
        println!("PROBE: GCLP_HICONSM          = {class_small:#x}");

        let has_any = big.0 != 0 || small.0 != 0 || class_big != 0 || class_small != 0;
        println!(
            "PROBE: VERDICT = {}",
            if has_any {
                "window HAS an icon (taskbar should show it)"
            } else {
                "window has NO icon at all"
            }
        );

        // A live handle is not proof the icon has any visible pixels -- a
        // fully transparent icon would report exactly the same way while
        // rendering as nothing in the taskbar. Read the bits back.
        if big.0 != 0 {
            describe_icon_pixels("ICON_BIG", big.0);
        }
    }
}

/// Reads an `HICON`'s colour bitmap back and reports how much of it is
/// actually opaque, and what colour. A shield icon is ~60% opaque blue; a
/// blank icon is 0% opaque, which is what a taskbar would render as
/// "missing" despite the handle being perfectly valid.
fn describe_icon_pixels(label: &str, hicon_raw: isize) {
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HBITMAP, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, HICON, ICONINFO};

    unsafe {
        let hicon = HICON(hicon_raw as *mut core::ffi::c_void);
        let mut info = ICONINFO::default();
        if GetIconInfo(hicon, &mut info).is_err() {
            println!("PROBE: {label} GetIconInfo failed");
            return;
        }

        let mut bmp = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(info.hbmColor.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut core::ffi::c_void),
        );
        if got == 0 {
            println!("PROBE: {label} GetObjectW failed");
            return;
        }
        println!(
            "PROBE: {label} bitmap {}x{}, {} bits/px",
            bmp.bmWidth, bmp.bmHeight, bmp.bmBitsPixel
        );

        // 32bpp top-down BGRA is what winit's `Icon::from_rgba` produces.
        if bmp.bmBitsPixel == 32 {
            let len = (bmp.bmWidth * bmp.bmHeight * 4) as usize;
            let mut buf = vec![0u8; len];
            let read = windows::Win32::Graphics::Gdi::GetBitmapBits(
                HBITMAP(info.hbmColor.0),
                len as i32,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
            );
            if read > 0 {
                let (mut opaque, mut bluish) = (0usize, 0usize);
                for px in buf.chunks_exact(4) {
                    let (b, _g, r, a) = (px[0], px[1], px[2], px[3]);
                    if a > 128 {
                        opaque += 1;
                        if b as i32 > r as i32 + 30 {
                            bluish += 1;
                        }
                    }
                }
                println!(
                    "PROBE: {label} opaque px = {opaque} of {}, bluish = {bluish}",
                    bmp.bmWidth * bmp.bmHeight
                );
                println!(
                    "PROBE: {label} PIXEL VERDICT = {}",
                    if opaque == 0 {
                        "BLANK -- valid handle but nothing to draw"
                    } else if bluish * 2 > opaque {
                        "the blue shield, as expected"
                    } else {
                        "opaque but not blue -- unexpected content"
                    }
                );
            }
        }

        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
    }
}

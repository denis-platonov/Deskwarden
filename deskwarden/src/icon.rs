//! Extracts an executable's small shell icon as raw RGBA pixels, for the
//! "Add app..." picker list (`picker_ui.rs`), which used to show initials in
//! a colored circle for every row instead of the app's actual icon.

use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
use windows::core::PCWSTR;

pub struct IconRgba {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied), row-major, top-to-bottom RGBA8.
    pub rgba: Vec<u8>,
}

/// Reads `exe_path`'s small shell icon (the same one Explorer shows for it)
/// as RGBA pixels ready to hand to `egui::ColorImage::from_rgba_unmultiplied`.
///
/// Returns `None` on any failure along the way (no icon associated with the
/// file, or a GDI call failing) -- the picker falls back to the initials
/// avatar for that row rather than treating a missing icon as fatal.
pub fn extract_small_icon(exe_path: &str) -> Option<IconRgba> {
    unsafe {
        let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut info = SHFILEINFOW::default();
        let result = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        );
        if result == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let hicon = info.hIcon;

        let mut icon_info = ICONINFO::default();
        if GetIconInfo(hicon, &mut icon_info).is_err() {
            let _ = DestroyIcon(hicon);
            return None;
        }
        // The mask bitmap isn't used (32bpp color bitmaps carry their own
        // alpha channel), but GetIconInfo hands ownership of both to us.
        let _ = DeleteObject(icon_info.hbmMask);

        let mut bmp = BITMAP::default();
        let got_bitmap = GetObjectW(
            icon_info.hbmColor,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut BITMAP as *mut core::ffi::c_void),
        );
        if got_bitmap == 0 {
            let _ = DeleteObject(icon_info.hbmColor);
            let _ = DestroyIcon(hicon);
            return None;
        }
        let width = bmp.bmWidth as u32;
        let height = bmp.bmHeight as u32;
        if width == 0 || height == 0 {
            let _ = DeleteObject(icon_info.hbmColor);
            let _ = DestroyIcon(hicon);
            return None;
        }

        let mut buffer = vec![0u8; (width * height * 4) as usize];
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative height requests a top-down DIB, matching the
                // row-major top-to-bottom order egui::ColorImage expects.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let hdc = GetDC(None);
        let copied = GetDIBits(
            hdc,
            icon_info.hbmColor,
            0,
            height,
            Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, hdc);

        let _ = DeleteObject(icon_info.hbmColor);
        let _ = DestroyIcon(hicon);

        if copied == 0 {
            return None;
        }

        // GDI hands back BGRA; egui wants RGBA.
        for px in buffer.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        Some(IconRgba {
            width,
            height,
            rgba: buffer,
        })
    }
}

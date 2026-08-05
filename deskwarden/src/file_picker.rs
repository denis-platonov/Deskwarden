//! The one native "choose a file" dialog this app opens, for the edit form's
//! app block: the user asked to be able to point at *"not just select a path"*
//! by browsing for it.
//!
//! # Why `IFileOpenDialog` and not a crate
//!
//! `rfd` and `native-dialog` are the obvious answers and both were rejected.
//! `rfd` pulls its own `windows-sys`/`raw-window-handle` stack; this crate
//! already pins `windows 0.58` deliberately and hard (see `Cargo.toml`'s note
//! on why eframe's wgpu renderer is off -- its DX12 backend wants a newer
//! `windows` than this app does, and the two cannot coexist in one build), so
//! a second Win32 binding is the exact risk that note is about. The dialog
//! itself is forty lines of COM against bindings that are already compiled in,
//! and the only new thing in `Cargo.toml` is one feature flag on a dependency
//! that is already there.
//!
//! # What this does NOT do
//!
//! It does not launch anything, it does not read the file, and it does not
//! validate it. It returns a string the user pointed at; deciding whether that
//! string is a path this app will act on is
//! [`crate::app_match::AppMatch::launchable_path`]'s job and nobody else's.
//!
//! # Blocking
//!
//! `Show` is modal and runs its own message loop, so the calling thread is
//! inside it until the user answers. That is what a file dialog is, and it is
//! why this is called from the vault window's **action handler** -- the same
//! place `EditAction::GeneratePassword`'s HTTP request is made -- rather than
//! from inside the form's draw closure.

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};

/// `CoInitializeEx`'s answer when the thread is already in a different
/// apartment: this call did **not** add a reference, so it must not remove
/// one. Every other answer (`S_OK`, `S_FALSE`) did, and is balanced by
/// `CoUninitialize`.
const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106

/// Opens the shell's file-open dialog and returns the chosen path, or `None`
/// if the user cancelled or the dialog could not be created.
///
/// **Cancel and failure are the same answer on purpose.** There is nothing for
/// a caller to do differently: in both cases no path was chosen and the form is
/// left exactly as it was. An error dialog on top of a dialog the user just
/// dismissed would be the worse of the two.
pub fn pick_executable() -> Option<String> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let balanced = init.0 != RPC_E_CHANGED_MODE;
        let picked = show_dialog();
        if balanced {
            CoUninitialize();
        }
        picked
    }
}

/// The COM half, split out so the `CoUninitialize` above is paired with its
/// `CoInitializeEx` on every path out of it -- including the several `?`s.
///
/// # Safety
///
/// The calling thread must have COM initialised.
unsafe fn show_dialog() -> Option<String> {
    let dialog: IFileOpenDialog =
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;

    // Programs first, so the common case needs no dropdown; "All files" stays
    // because plenty of real programs are not `.exe` (a `.com`, a launcher
    // shim) and refusing to show them would be this app deciding what the
    // user's app is.
    let filters = [
        COMDLG_FILTERSPEC { pszName: w!("Programs"), pszSpec: w!("*.exe;*.com") },
        COMDLG_FILTERSPEC { pszName: w!("All files"), pszSpec: w!("*.*") },
    ];
    let _ = dialog.SetFileTypes(&filters);
    let _ = dialog.SetTitle(w!("Choose the program to match"));

    // `Show` answers `HRESULT_FROM_WIN32(ERROR_CANCELLED)` when the user
    // dismisses it, which is an `Err` here and is not an error.
    dialog.Show(HWND::default()).ok()?;

    let item = dialog.GetResult().ok()?;
    // `SIGDN_FILESYSPATH` is the only display name that is a real path -- the
    // default is the shell's *pretty* name, which drops the extension when the
    // user has extensions hidden and would hand back `C:\...\chrome` for
    // `chrome.exe`.
    let wide = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let path = wide.to_string().ok();
    // The shell allocated it; this frees it whether or not the conversion
    // above succeeded.
    CoTaskMemFree(Some(wide.0 as *const core::ffi::c_void));
    path.filter(|p| !p.is_empty())
}

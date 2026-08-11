//! The native shell dialogs this app opens: a **file-open** dialog for the
//! edit form's app block (the user asked to be able to point at *"not just
//! select a path"* by browsing for it), and a **file-save** dialog for the
//! vault export's destination.
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
//! It does not launch anything, it does not read or write the file, and it
//! does not validate it. It returns a string the user pointed at; deciding
//! whether that string is a path this app will act on is
//! [`crate::app_match::AppMatch::launchable_path`]'s job for the open dialog
//! and [`crate::vault_export::plan_export`]'s job for the save one.
//!
//! # Two siblings, not one parameterised dialog
//!
//! [`pick_executable`] and [`pick_export_destination`] are separate functions
//! on purpose. They are different COM classes (`IFileOpenDialog` against
//! `IFileSaveDialog`), they answer different types, and every visible setting
//! -- title, filters, default extension, suggested name, options -- differs.
//! A single function taking six arguments to serve two callers would be the
//! parameterisation that only reads well from one end of it. What they *do*
//! share is factored out and shared for real: [`with_com`] owns the apartment
//! balance for both, and `chosen_path` owns the one correct way to read a
//! filesystem path back out of an `IFileDialog`.
//!
//! # Blocking
//!
//! `Show` is modal and runs its own message loop, so the calling thread is
//! inside it until the user answers. That is what a file dialog is, and it is
//! why this is called from the vault window's **action handler** -- the same
//! place `EditAction::GeneratePassword`'s HTTP request is made -- rather than
//! from inside the form's draw closure.

use std::path::PathBuf;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    FILEOPENDIALOGOPTIONS, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH,
};

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
    with_com(|| unsafe { show_dialog() })
}

/// **The one place in this module that initialises COM**, and the one place
/// that uninitialises it.
///
/// `CoUninitialize` runs only when this call was the one that added a
/// reference. Getting that wrong is not a wrong dialog, it is a torn-down
/// apartment underneath whatever else on this thread was using COM, which is a
/// crash somewhere else entirely -- so both dialogs go through here rather than
/// each re-deriving the balance.
fn with_com<T>(body: impl FnOnce() -> Option<T>) -> Option<T> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let balanced = init.0 != RPC_E_CHANGED_MODE;
        let answer = body();
        if balanced {
            CoUninitialize();
        }
        answer
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
    chosen_path(&dialog)
}

/// Reads the answered item back out of a dialog that has already returned from
/// `Show`. Shared by both dialogs because the `SIGDN_FILESYSPATH` choice and
/// the `CoTaskMemFree` below are the same mistake to make twice.
///
/// # Safety
///
/// `dialog` must be one whose `Show` returned `Ok`.
unsafe fn chosen_path(dialog: &IFileDialog) -> Option<String> {
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

// ---------------------------------------------------------------------------
// The save dialog
// ---------------------------------------------------------------------------

/// Opens the shell's file-save dialog for the vault export and returns the
/// path the user settled on, or `None` if they cancelled or the dialog could
/// not be created. Cancel and failure are the same answer, for the same reason
/// [`pick_executable`] gives.
///
/// # Blocking
///
/// The same rule as [`pick_executable`]: `Show` is modal and runs its own
/// message loop, so this is called from the **vault window's action handler**
/// and never from inside a draw closure.
///
/// # This dialog *is* the confirmation step
///
/// Nothing else asks the user "are you sure". The shell's own "already exists,
/// replace it?" is therefore load-bearing and is left switched on -- see
/// [`save_options`] for why it is OR-ed into whatever the shell already had
/// rather than assumed.
///
/// # Writing is somebody else's job
///
/// This chooses a path. It creates nothing, truncates nothing and checks
/// nothing about the directory; the export itself is `bw`'s `--output`.
pub fn pick_export_destination(suggested_name: &str) -> Option<PathBuf> {
    with_com(|| unsafe { show_save_dialog(suggested_name) }).map(PathBuf::from)
}

/// The COM half of [`pick_export_destination`], split out for the same reason
/// `show_dialog` is: every `?` in here leaves through [`with_com`]'s balance.
///
/// # Safety
///
/// The calling thread must have COM initialised.
unsafe fn show_save_dialog(suggested_name: &str) -> Option<String> {
    let dialog: IFileSaveDialog =
        CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;

    // **One filter, not two.** The open dialog keeps an "All files" row because
    // it cannot know what the user's program looks like. This one can: the
    // export is `encrypted_json` and nothing else (see
    // [`crate::vault_export`]), so a second row would only be a way for the
    // user to land somewhere the default extension no longer applies -- the
    // shell takes its appended extension from the *selected* file type.
    let filters = [COMDLG_FILTERSPEC {
        pszName: w!("Encrypted Bitwarden export (*.json)"),
        pszSpec: w!("*.json"),
    }];
    let _ = dialog.SetFileTypes(&filters);
    let _ = dialog.SetFileTypeIndex(1); // 1-based, not 0-based.
    let _ = dialog.SetTitle(w!("Save the encrypted vault export"));
    let _ = dialog.SetDefaultExtension(w!("json"));

    // Read-modify-write. `SetOptions` *replaces* the set, so building a value
    // from nothing would silently drop `FOS_OVERWRITEPROMPT`, which the shell
    // already had on and which this dialog cannot do without.
    if let Ok(current) = dialog.GetOptions() {
        let _ = dialog.SetOptions(save_options(current));
    }

    if let Some(name) = dialog_file_name(suggested_name) {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = dialog.SetFileName(PCWSTR(wide.as_ptr()));
    }

    dialog.Show(HWND::default()).ok()?;
    chosen_path(&dialog)
}

/// What the save dialog's option set becomes, given whatever the shell handed
/// back from `GetOptions`.
///
/// Purely additive on purpose:
///
/// * `FOS_OVERWRITEPROMPT` is already the shell's default for a save dialog.
///   It is OR-ed in anyway so that this line, and not a default that a future
///   edit could quietly stop inheriting, is what keeps the "replace it?"
///   prompt on. Since this dialog is the only confirmation the export gets,
///   losing that prompt would mean a stray double-click could overwrite a file
///   with no warning at all.
/// * `FOS_FORCEFILESYSTEM` keeps the answer a real path. Without it the user
///   can settle on a shell item inside a virtual folder that has no
///   filesystem path, and `SIGDN_FILESYSPATH` would then fail after the user
///   believed they had chosen somewhere.
///
/// Nothing is cleared. Whatever else the shell wants on -- its own view state,
/// the flags it sets from policy -- stays on.
fn save_options(current: FILEOPENDIALOGOPTIONS) -> FILEOPENDIALOGOPTIONS {
    current | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM
}

// ---------------------------------------------------------------------------
// The suggested name
// ---------------------------------------------------------------------------

/// "Now", injected, so that [`suggested_export_name`] can be tested at all.
///
/// This mirrors [`crate::send::SendClock`] deliberately -- same idea, same
/// millisecond unit, same reason -- but is a **separate trait**: `send.rs` is
/// a different feature under a different task, and a dialog module reaching
/// into it for a name generator would tie the two together for no gain. If a
/// third caller ever wants one, hoisting these two into one place is the
/// change to make then.
pub trait ExportClock {
    /// Milliseconds since the Unix epoch, UTC.
    fn now_unix_millis(&self) -> i64;
}

/// A clock frozen at one instant. What the tests use.
#[derive(Debug, Clone, Copy)]
pub struct FixedExportClock(pub i64);

impl ExportClock for FixedExportClock {
    fn now_unix_millis(&self) -> i64 {
        self.0
    }
}

/// The wall clock. What the UI passes; nothing in this module reads the clock
/// for itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemExportClock;

impl ExportClock for SystemExportClock {
    fn now_unix_millis(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

const MILLIS_PER_DAY: i64 = 86_400_000;

/// Civil date from a count of days since 1970-01-01, by Howard Hinnant's
/// `civil_from_days`. Proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// The name the save dialog opens with: `deskwarden-export-20260810-143005.json`.
///
/// **UTC, not local time.** The only job the stamp has is to make two exports
/// taken minutes apart distinct and sortable, and a local stamp goes backwards
/// for an hour every autumn -- which would put the *later* file earlier in the
/// listing exactly once a year. The same choice, and the same reasoning, as
/// the dates [`crate::send`] writes.
///
/// It is a bare file name with no directory in it: where the file lands is the
/// dialog's business (it opens wherever the shell last left the user), not
/// this function's.
pub fn suggested_export_name(now: &dyn ExportClock) -> String {
    let millis = now.now_unix_millis();
    let days = millis.div_euclid(MILLIS_PER_DAY);
    let rem = millis.rem_euclid(MILLIS_PER_DAY) / 1000;
    let (y, mo, d) = civil_from_days(days);
    let (h, mi, s) = ((rem / 3600) as u32, ((rem / 60) % 60) as u32, (rem % 60) as u32);
    format!("deskwarden-export-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.json")
}

/// The file-name part of `suggested`, or `None` if there is not one.
///
/// # Why not `Path::file_name`
///
/// Because it lies about exactly the input that matters here.
/// `Path::new(r"C:\Backups\").file_name()` answers `Some("Backups")` -- it
/// normalises the trailing separator away and hands back the *directory* as
/// though it were a chosen file name. Feeding that to `SetFileName` would
/// open the dialog pre-filled with the name of a folder, and the user's first
/// Enter would try to save a file called `Backups` next to it.
///
/// Here a string that ends in a separator has no file name, full stop. So does
/// an empty string, and so does one whose last segment is empty.
pub fn dialog_file_name(suggested: &str) -> Option<&str> {
    let last = suggested.rsplit(['\\', '/']).next()?;
    if last.is_empty() {
        None
    } else {
        Some(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Shell::FOS_NOCHANGEDIR;

    /// 2026-08-10T14:30:05Z.
    const AUGUST: i64 = 1_786_372_205_000;
    /// 2007-03-04T05:06:07Z -- every field single-digit, which is what makes
    /// the zero padding visible.
    const SINGLE_DIGITS: i64 = 1_172_984_767_000;
    /// One second after [`AUGUST`].
    const AUGUST_PLUS_A_SECOND: i64 = 1_786_372_206_000;

    // -- the suggested name ------------------------------------------------

    #[test]
    fn the_suggested_name_is_the_shape_the_design_names() {
        assert_eq!(
            suggested_export_name(&FixedExportClock(AUGUST)),
            "deskwarden-export-20260810-143005.json"
        );
    }

    #[test]
    fn every_field_of_the_stamp_is_zero_padded() {
        assert_eq!(
            suggested_export_name(&FixedExportClock(SINGLE_DIGITS)),
            "deskwarden-export-20070304-050607.json"
        );
    }

    #[test]
    fn two_different_instants_give_two_different_names() {
        // Without this the test would still pass if both fixtures were the
        // same number, and would then be proving nothing at all.
        assert_ne!(AUGUST, AUGUST_PLUS_A_SECOND, "the fixtures must differ");
        assert_ne!(
            suggested_export_name(&FixedExportClock(AUGUST)),
            suggested_export_name(&FixedExportClock(AUGUST_PLUS_A_SECOND))
        );
    }

    #[test]
    fn the_same_instant_gives_the_same_name_twice() {
        let a = suggested_export_name(&FixedExportClock(AUGUST));
        let b = suggested_export_name(&FixedExportClock(AUGUST));
        assert_eq!(a, b);
    }

    #[test]
    fn the_suggested_name_carries_no_directory_and_the_extension_the_dialog_defaults_to() {
        let name = suggested_export_name(&FixedExportClock(AUGUST));
        assert!(!name.contains('\\'), "{name}");
        assert!(!name.contains('/'), "{name}");
        assert!(name.ends_with(".json"), "{name}");
        // What is handed to `SetFileName` must survive the guard below
        // unchanged, or the dialog would open on a different name than the one
        // these tests pin.
        assert_eq!(dialog_file_name(&name), Some(name.as_str()));
    }

    // -- the file-name guard, and the trailing-separator trap ---------------

    #[test]
    fn a_trailing_separator_means_there_is_no_file_name() {
        // `Path::file_name` answers `Some("Backups")` for both of these.
        assert_eq!(dialog_file_name(r"C:\Backups\"), None);
        assert_eq!(dialog_file_name("C:/Backups/"), None);
        // Proof that the trap is real and not imagined.
        assert_eq!(
            std::path::Path::new(r"C:\Backups\")
                .file_name()
                .and_then(|n| n.to_str()),
            Some("Backups"),
            "if this ever stops holding the doc comment above needs rewriting"
        );
    }

    #[test]
    fn a_real_last_segment_is_the_file_name_and_nothing_else_is() {
        assert_eq!(dialog_file_name(r"C:\Backups\vault.json"), Some("vault.json"));
        assert_eq!(dialog_file_name("C:/Backups/vault.json"), Some("vault.json"));
        assert_eq!(dialog_file_name("vault.json"), Some("vault.json"));
        assert_eq!(dialog_file_name(""), None);
        assert_eq!(dialog_file_name(r"\"), None);
        assert_eq!(dialog_file_name("/"), None);
    }

    // -- the option set ----------------------------------------------------

    #[test]
    fn the_overwrite_prompt_is_switched_on_even_when_the_shell_had_it_off() {
        let out = save_options(FILEOPENDIALOGOPTIONS(0));
        assert_eq!(
            out & FOS_OVERWRITEPROMPT,
            FOS_OVERWRITEPROMPT,
            "the save dialog is the export's only confirmation step"
        );
        assert_eq!(out & FOS_FORCEFILESYSTEM, FOS_FORCEFILESYSTEM);
    }

    #[test]
    fn the_option_set_only_ever_gains_bits() {
        let shell_had = FOS_NOCHANGEDIR | FOS_OVERWRITEPROMPT;
        let out = save_options(shell_had);
        assert_eq!(out & shell_had, shell_had, "nothing the shell set is cleared");
        assert_eq!(out & FOS_OVERWRITEPROMPT, FOS_OVERWRITEPROMPT);
    }

    // -- source pins -------------------------------------------------------
    //
    // Everything below is a **source pin**, not a behavioural test, and for
    // one reason: the thing being asserted about is a call into the Windows
    // shell that opens a modal window and waits for a human. There is no seam
    // to fake -- `IFileSaveDialog` is a COM object this module creates itself
    // -- and no test in this crate may open a dialog. So the assertion is that
    // the line is there, which catches a deletion and would not catch a
    // subtler change in behaviour. `save_options` above is the part that was
    // worth extracting so it could be tested properly instead.

    /// This file's own text. Only the part **above** the test module is
    /// searched, so that a needle spelled out in a test below does not satisfy
    /// the pin that looks for it.
    fn code_under_test() -> String {
        // Normalised to LF once, so a multi-line needle written with `\n`
        // matches this file whether it is stored CRLF or LF.
        let whole = include_str!("file_picker.rs").replace("\r\n", "\n");
        let code = whole.split("#[cfg(test)]").next().unwrap().to_string();
        assert!(
            code.len() < whole.len(),
            "the test module marker was not found; the split did nothing"
        );
        code
    }

    #[test]
    fn the_source_pin_search_can_tell_present_from_absent() {
        // The positive control the pins below depend on. If `code_under_test`
        // ever returned an empty string, or the wrong half of the file, every
        // pin would fail loudly rather than pass vacuously -- but this is the
        // test that says so.
        let code = code_under_test();
        assert!(code.contains("pub fn pick_export_destination(suggested_name: &str)"));
        assert!(!code.contains("no such line appears anywhere in this module"));
        // And the split really did drop the tests: this very function's name
        // is below the marker.
        assert!(!code.contains("the_source_pin_search_can_tell_present_from_absent"));
    }

    #[test]
    fn the_save_dialog_sets_the_default_extension() {
        assert!(
            code_under_test().contains(r#"SetDefaultExtension(w!("json"))"#),
            "without it a name typed with no extension is saved with none"
        );
    }

    #[test]
    fn the_option_set_reaches_the_dialog_through_save_options() {
        // `save_options` is tested for real above; this is the wire from it to
        // the dialog, which is what a mutation would cut.
        assert!(code_under_test().contains(
            "    if let Ok(current) = dialog.GetOptions() {\n\
             \x20       let _ = dialog.SetOptions(save_options(current));\n\
             \x20   }\n"
        ));
    }

    #[test]
    fn com_is_uninitialised_exactly_once_and_only_when_this_call_initialised_it() {
        let code = code_under_test();
        assert_eq!(
            code.matches("CoUninitialize();").count(),
            1,
            "one call, in with_com, and nowhere else"
        );
        assert!(code.contains(
            "        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);\n\
             \x20       let balanced = init.0 != RPC_E_CHANGED_MODE;\n\
             \x20       let answer = body();\n\
             \x20       if balanced {\n\
             \x20           CoUninitialize();\n\
             \x20       }\n"
        ));
    }

    #[test]
    fn both_dialogs_go_through_the_one_apartment_balance() {
        let code = code_under_test();
        assert_eq!(code.matches("CoInitializeEx(").count(), 1);
        assert!(code.contains("    with_com(|| unsafe { show_dialog() })\n"));
        assert!(code.contains(
            "    with_com(|| unsafe { show_save_dialog(suggested_name) }).map(PathBuf::from)\n"
        ));
    }
}

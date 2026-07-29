use windows::core::{Result, BSTR, VARIANT};
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
};

/// Walks the UI Automation tree of the window identified by `hwnd` looking for
/// Edit controls, and fills the first two found with `username` and
/// `password` respectively.
///
/// Returns:
/// - `Ok(true)` if two edit controls were found and filled.
/// - `Ok(false)` if fewer than two edit controls were found (caller should
///   fall back to another injection strategy).
/// - `Err` on a COM-level failure.
pub fn fill_via_ui_automation(hwnd: isize, username: &str, password: &str) -> Result<bool> {
    unsafe {
        // S_OK and S_FALSE (already initialised on this thread) are both fine,
        // and RPC_E_CHANGED_MODE just means COM is already up on this thread
        // in a different apartment model -- the UI Automation client still
        // works. Anything else is a real problem worth seeing in the log, even
        // though we continue and let the calls below fail with detail.
        //
        // No matching `CoUninitialize`: this runs on the long-lived main
        // thread and process exit is adequate cleanup.
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            log::warn!("CoInitializeEx failed unexpectedly: {hr:?}");
        }

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

        let root = automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))?;

        let condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )?;

        let edits = root.FindAll(TreeScope_Descendants, &condition)?;
        let count = edits.Length()?;

        if count < 2 {
            return Ok(false);
        }

        let username_element = edits.GetElement(0)?;
        let password_element = edits.GetElement(1)?;

        let username_value: IUIAutomationValuePattern =
            username_element.GetCurrentPatternAs(UIA_ValuePatternId)?;
        let password_value: IUIAutomationValuePattern =
            password_element.GetCurrentPatternAs(UIA_ValuePatternId)?;

        username_value.SetValue(&BSTR::from(username))?;
        password_value.SetValue(&BSTR::from(password))?;

        Ok(true)
    }
}

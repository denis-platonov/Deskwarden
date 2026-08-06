use windows::core::{Result, BSTR, VARIANT};
use windows::Win32::Foundation::{HWND, RECT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
};

/// Walks the UI Automation tree of the window identified by `hwnd` looking for
/// Edit controls, and fills `username` and `password` into them.
///
/// The password field is identified by its `IsPassword` accessibility
/// property, not by assuming it's the second Edit control found in tree
/// order: some apps (Epic Games Launcher among them, being CEF-based) don't
/// expose their fields in username-then-password tree order -- a positional
/// guess could silently write the password into an unrelated field while
/// leaving the real password box untouched, which reads as "only the email
/// got filled". Falls back to the first two edits in tree order only if no
/// field reports `IsPassword` at all.
///
/// Returns:
/// - `Ok(true)` if a username and a password field were found and filled.
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

        let mut username_element = None;
        let mut password_element = None;
        for i in 0..count {
            let el = edits.GetElement(i)?;
            if el.CurrentIsPassword()?.as_bool() {
                if password_element.is_none() {
                    password_element = Some(el);
                }
            } else if username_element.is_none() {
                username_element = Some(el);
            }
        }

        let (username_element, password_element) = match (username_element, password_element) {
            (Some(u), Some(p)) => (u, p),
            _ => (edits.GetElement(0)?, edits.GetElement(1)?),
        };

        let username_value: IUIAutomationValuePattern =
            username_element.GetCurrentPatternAs(UIA_ValuePatternId)?;
        let password_value: IUIAutomationValuePattern =
            password_element.GetCurrentPatternAs(UIA_ValuePatternId)?;

        username_value.SetValue(&BSTR::from(username))?;

        // **The one unwiped plaintext copy left on the default fill path, cut
        // from two to one -- and the remaining one is a documented gap, not an
        // oversight.**
        //
        // `credentials_for`'s `String` was wiped in `4a559b9`; `Plan` and
        // `TextRun` zeroize on drop. This line was what remained. `BSTR::from
        // (&str)` does two things: it collects the UTF-16 into an ordinary
        // `Vec<u16>`, and it hands that to `SysAllocStringLen`, which COPIES
        // it into a fresh COM allocation. So there were *two* plaintext
        // buffers, and the `Vec` was released to Rust's allocator unwiped.
        //
        // That one is fixable with no `unsafe` at all: own the vector, wrap it
        // in `Zeroizing`, and call the same `from_wide` that `From<&str>`
        // calls internally -- identical code path, one fewer live copy.
        let wide = zeroize::Zeroizing::new(password.encode_utf16().collect::<Vec<u16>>());
        password_value.SetValue(&BSTR::from_wide(&wide)?)?;

        // **What is NOT fixed here, deliberately: the `BSTR`'s own buffer.**
        //
        // A `BSTR` frees itself with `SysFreeString` on drop, which does not
        // wipe. Wiping it means writing through its pointer before it drops,
        // and `BSTR::as_ptr` returns `*const u16` -- so it means casting away
        // constness and trusting that the pointer is the unique, writable COM
        // allocation. Reading `windows-strings` says it is, for a non-empty
        // `BSTR`; for an EMPTY one `as_ptr` returns a pointer into a
        // `const EMPTY: [u16; 1]` static, and writing there is undefined
        // behaviour in a read-only page. That is a guard on an implementation
        // detail of a third-party crate, reached through a const-cast, on the
        // live fill path -- and nothing in this crate can test it: reaching
        // this function needs a real COM apartment, a real window and a real
        // UIA tree, so no `!leaked` assertion can ever cover it.
        //
        // A wrong wipe here is worse than a documented gap, so this is the
        // gap, stated: **after a UIA fill, one COM-allocated copy of the
        // password is freed without being zeroized.** Its lifetime is the two
        // statements above. Closing it properly wants either a `BSTR` whose
        // storage this module allocates itself (`SysAllocStringLen` called
        // here, `SysFreeString` in a local `Drop`, with the wipe in between)
        // or an upstream `as_mut_ptr`; both are more change than a release
        // review should make blind.

        Ok(true)
    }
}

/// A screen-space rect to anchor the autofill overlay near: the currently
/// focused element inside `hwnd` if there is one and it reports a real
/// (non-empty) bounding box, otherwise the first Edit control found in the
/// window. `Ok(None)` means UI Automation found nothing usable -- not an
/// error, just "fall back to positioning off the window itself" for the
/// caller.
pub fn field_anchor_rect(hwnd: isize) -> Result<Option<RECT>> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            log::warn!("CoInitializeEx failed unexpectedly: {hr:?}");
        }

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

        let root = automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))?;

        // Prefer the element actually holding focus -- the field the user's
        // cursor is sitting in -- over a positional guess. This can be
        // *any* focusable control, not just an Edit, so it's read straight
        // off automation rather than the Edit-only FindAll below.
        if let Ok(focused) = automation.GetFocusedElement() {
            if let Ok(rect) = focused.CurrentBoundingRectangle() {
                if is_real_rect(rect) {
                    return Ok(Some(rect));
                }
            }
        }

        let condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )?;
        let edits = root.FindAll(TreeScope_Descendants, &condition)?;
        if edits.Length()? == 0 {
            return Ok(None);
        }
        let rect = edits.GetElement(0)?.CurrentBoundingRectangle()?;
        Ok(is_real_rect(rect).then_some(rect))
    }
}

/// `CurrentBoundingRectangle` returns an all-zero rect for elements that
/// exist in the tree but aren't actually laid out on screen (hidden,
/// off-screen, or not yet rendered) -- that's not a usable anchor.
fn is_real_rect(r: RECT) -> bool {
    r.right > r.left && r.bottom > r.top
}

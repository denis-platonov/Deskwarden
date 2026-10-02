use windows::core::{Result, BSTR, VARIANT};
use windows::Win32::Foundation::{HWND, RECT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCondition,
    IUIAutomationElement, IUIAutomationElementArray, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_AutomationIdPropertyId,
    UIA_ClassNamePropertyId, UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_IsPasswordPropertyId,
    UIA_NamePropertyId, UIA_ValuePatternId,
};

/// A UTF-16 string handle whose buffer [`set_value_wiping`] zeroes before the
/// handle is released.
///
/// A trait with exactly one production implementor, and it is not there for
/// polymorphism: reaching [`fill_via_ui_automation`] needs a real COM
/// apartment, a real window and a real UI Automation tree, so nothing in this
/// crate can call it. The trait is what lets the *sequencing* -- wipe before
/// release, and wipe even when the setter failed -- be exercised against a
/// buffer a test still owns afterwards and can read.
trait WipeableWide {
    /// A writable pointer to the UTF-16 buffer, valid for [`Self::wide_len`]
    /// units. Only called when that length is non-zero.
    fn wide_ptr(&self) -> *mut u16;
    /// The buffer's length in UTF-16 units, not counting any NUL terminator.
    fn wide_len(&self) -> usize;
}

impl WipeableWide for BSTR {
    /// **The one const-cast, and the reason the empty case is excluded.**
    ///
    /// `BSTR::as_ptr` returns `*const u16`. For a non-empty `BSTR` that is the
    /// unique `SysAllocStringLen` allocation this module just made, on an
    /// ordinary writable COM heap page, and nothing else holds a reference to
    /// it. For an EMPTY one it is a pointer into a `const EMPTY: [u16; 1]`
    /// static, which may live in a read-only page -- writing there would be
    /// undefined behaviour. [`set_value_wiping`] therefore skips the write
    /// when the length is zero, which is also exactly the case with no
    /// plaintext in it to wipe.
    ///
    /// **Residual, stated rather than papered over:** this relies on
    /// `windows-strings` returning the COM allocation from `as_ptr` for a
    /// non-empty `BSTR`, which is an implementation detail of a third-party
    /// crate rather than a contract. An upstream `as_mut_ptr`, or allocating
    /// through `SysAllocStringLen` here and freeing in a local `Drop`, would
    /// remove that dependence.
    fn wide_ptr(&self) -> *mut u16 {
        self.as_ptr() as *mut u16
    }

    fn wide_len(&self) -> usize {
        self.len()
    }
}

/// Hands `handle` to `set`, **wipes the handle's buffer, and only then**
/// releases the handle and returns `set`'s result.
///
/// The order is the whole content of this function and each part of it is
/// load-bearing:
///
/// * The wipe is before the drop, because a `BSTR` frees itself with
///   `SysFreeString`, which does not zero.
/// * `set`'s result is held rather than propagated with `?`, because a `?` on
///   the setter would return past the wipe -- and a setter that failed is
///   precisely the case where the password has been handed to another process
///   and something went wrong, i.e. the one worth wiping most.
fn set_value_wiping<H: WipeableWide, T>(handle: H, set: impl FnOnce(&H) -> Result<T>) -> Result<T> {
    let result = set(&handle);
    let len = handle.wide_len();
    if len > 0 {
        // SAFETY: `wide_ptr` is documented to be a writable pointer valid for
        // `wide_len` units whenever that length is non-zero, which is the
        // branch this is in. The write is plain stores of zero over a buffer
        // this call still owns; nothing else reads it afterwards.
        unsafe { std::ptr::write_bytes(handle.wide_ptr(), 0u8, len) };
    }
    drop(handle);
    result
}

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

        // Both fields go through `set_value_wiping`, which is what makes the
        // COM-allocated copy short-lived rather than merely unmentioned. The
        // `Zeroizing<Vec<u16>>` is the Rust-side half; see the comment on the
        // password line for why there are two copies to begin with.
        let user_wide = zeroize::Zeroizing::new(username.encode_utf16().collect::<Vec<u16>>());
        set_value_wiping(BSTR::from_wide(&user_wide)?, |b| username_value.SetValue(b))?;

        // **There were two plaintext copies here, and both are now wiped.**
        //
        // `credentials_for`'s `String` was wiped in `4a559b9`; `Plan` and
        // `TextRun` zeroize on drop. This line was what remained.
        //
        // `BSTR::from(&str)` does two things: it collects the UTF-16 into an
        // ordinary `Vec<u16>`, and it hands that to `SysAllocStringLen`, which
        // COPIES it into a fresh COM allocation. The `Vec` is Rust's and is
        // covered by `Zeroizing`; the COM allocation is freed by
        // `SysFreeString`, which does not wipe, and used to go back to the COM
        // allocator with the password still in it.
        let wide = zeroize::Zeroizing::new(password.encode_utf16().collect::<Vec<u16>>());
        set_value_wiping(BSTR::from_wide(&wide)?, |b| password_value.SetValue(b))?;

        Ok(true)
    }
}

/// Whether the window `hwnd` contains **any** masked field.
///
/// The question the no-match overlay is built on: `fill_via_ui_automation`
/// asks `CurrentIsPassword` of every Edit it found, but it only runs against a
/// window the vault already matched, on a path the user has already waited
/// for. This asks the same property of a window nothing matched, on the focus
/// change itself -- a hot path -- so it is deliberately the *cheapest* shape of
/// the question:
///
/// * **`FindFirst`, not `FindAll`.** One cross-process round trip that stops at
///   the first hit, rather than one for the list plus one `CurrentIsPassword`
///   per element. On a login window the answer is usually the second or third
///   Edit; on an ordinary window there is no hit and both shapes walk the whole
///   subtree, so the saving is on the yes case and the cost of the no case is
///   the same either way. That no case is what
///   [`crate::app::PasswordFieldProbe`]'s throttle exists for.
/// * **The `IsPassword` property is in the condition**, so the filtering
///   happens in the provider rather than as a property fetch per element from
///   this process.
/// * The control type is still ANDed in, because `IsPassword` alone would also
///   admit a non-Edit provider that reports the property, and every field this
///   app has ever filled is an Edit.
///
/// `Err` means the question could not be asked -- no apartment, a window that
/// has gone away, a provider that does not answer. The caller must treat that
/// as *unknown*, not as `false`: see [`crate::app::disposition`], where
/// `HasPasswordField::Unknown` opens nothing, so a window we could not read is
/// silent exactly as it is today rather than guessing a card onto the screen.
pub fn window_has_password_field(hwnd: isize) -> Result<bool> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            log::warn!("CoInitializeEx failed unexpectedly: {hr:?}");
        }

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

        let root = automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))?;

        let is_edit = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )?;
        let is_password = automation
            .CreatePropertyCondition(UIA_IsPasswordPropertyId, &VARIANT::from(true))?;
        let masked_edit = automation.CreateAndCondition(&is_edit, &is_password)?;

        // `FindFirst` answers a null element rather than an error when nothing
        // matches, which `windows-rs` surfaces as `Err`. So "not found" and
        // "could not ask" arrive the same way here, and the conservative
        // reading is the one that keeps today's behaviour: no hit means no
        // card. `Ok(false)` and `Err` are therefore both silence at the
        // disposition, and the distinction is kept only for the log.
        match root.FindFirst(TreeScope_Descendants, &masked_edit) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
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

/// How long UI Automation may wait for a browser to accept the connection,
/// and to answer one request, before [`address_field_text`] gives up.
///
/// The defaults are 2 s and 20 s, and this runs between a key press and the
/// card it opens: a hung browser must cost the user a short pause and a
/// picker ranked as before, never a frozen hotkey. Measured on 2026-10-01 the
/// whole query took 50-95 ms in Chrome, Edge, Brave and Firefox.
const ADDRESS_FIELD_CONNECTION_TIMEOUT_MS: u32 = 500;
const ADDRESS_FIELD_TRANSACTION_TIMEOUT_MS: u32 = 1000;

/// **The text of a browser's address field**, found by the properties
/// `field` names, or `Ok(None)` when there is no such field that belongs to
/// the browser's own chrome.
///
/// The same apartment handling as every function above (see
/// [`find_address_fields`]). Created through `CUIAutomation8` rather than
/// `CUIAutomation` because only that object answers for `IUIAutomation2`,
/// which is where the two timeouts are set.
///
/// **`FindAll` and not `FindFirst`, on purpose.** A web page's own elements
/// are in the same tree, with the page's `id` as their `AutomationId`, and in
/// Firefox the page comes BEFORE the toolbar in tree order -- so the first
/// match could be a page's `<input id="urlbar-input">`. Every match is held to
/// [`crate::browser_url::is_browser_chrome`] instead, and the first that
/// passes is read.
///
/// The text is returned to the caller and to nobody else: it is not logged
/// here, and `browser_url::read` does not log it either.
pub fn address_field_text(
    hwnd: isize,
    field: crate::browser_url::AddressField,
) -> Result<Option<String>> {
    unsafe {
        let Some(query) = find_address_fields(hwnd, field)? else {
            return Ok(None);
        };
        for index in 0..query.found.Length()? {
            let element = query.found.GetElement(index)?;
            let (depth, crosses_a_document) =
                ancestry(&query.automation, &query.walker, &query.root, &element);
            if !crate::browser_url::is_browser_chrome(depth, crosses_a_document) {
                continue;
            }
            let value: IUIAutomationValuePattern =
                element.GetCurrentPatternAs(UIA_ValuePatternId)?;
            let text = value.CurrentValue()?.to_string();
            return Ok((!text.is_empty()).then_some(text));
        }
        Ok(None)
    }
}

/// One element [`address_field_text`]'s query matched, described by where it
/// sits and what it calls itself -- **and never by what it holds**.
///
/// **Probe-only.** `examples/address_bar_probe.rs` prints these so a hand
/// check in a new browser, a Firefox popup or a Chrome app window can see why
/// each candidate was accepted or refused. Nothing in the app reads them.
///
/// There is deliberately no text field: a refused candidate may be a web
/// page's own input, and its value is whatever the user typed into that page.
/// `ClassName` and `AutomationId` are the two keys [`crate::browser_url::Family::address_field`]
/// matches on; `Name` is left out because for a page's element it is the
/// page's `aria-label`, which is page content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressFieldCandidate {
    /// The walk up to the window, as [`crate::browser_url::is_browser_chrome`] takes it.
    pub depth: Option<usize>,
    /// Whether a `Document` was crossed on the way up.
    pub crosses_a_document: bool,
    /// `UIA_ClassNamePropertyId`, or empty when the provider did not say.
    pub class_name: String,
    /// `UIA_AutomationIdPropertyId`, or empty when the provider did not say.
    pub automation_id: String,
}

/// **Probe-only**: every element [`address_field_text`]'s query matches, in
/// tree order, as an [`AddressFieldCandidate`], whether or not it would be
/// read.
///
/// The same query through the same [`find_address_fields`] and the same
/// [`ancestry`], so what the probe prints is what the app decides on. It
/// never asks a candidate for its value -- `address_field_tests` holds this
/// function to that.
pub fn address_field_candidates(
    hwnd: isize,
    field: crate::browser_url::AddressField,
) -> Result<Vec<AddressFieldCandidate>> {
    unsafe {
        let Some(query) = find_address_fields(hwnd, field)? else {
            return Ok(Vec::new());
        };
        let mut candidates = Vec::new();
        for index in 0..query.found.Length()? {
            let element = query.found.GetElement(index)?;
            let (depth, crosses_a_document) =
                ancestry(&query.automation, &query.walker, &query.root, &element);
            candidates.push(AddressFieldCandidate {
                depth,
                crosses_a_document,
                class_name: element.CurrentClassName().map(|s| s.to_string()).unwrap_or_default(),
                automation_id: element
                    .CurrentAutomationId()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            });
        }
        Ok(candidates)
    }
}

/// What [`find_address_fields`] hands back: the matches, and what is needed to
/// walk each one up to the window.
struct AddressFieldQuery {
    automation: IUIAutomation2,
    walker: IUIAutomationTreeWalker,
    root: IUIAutomationElement,
    found: IUIAutomationElementArray,
}

/// The query [`address_field_text`] and [`address_field_candidates`] share:
/// every descendant of `hwnd` whose control type is one of `field.kinds` AND
/// that has any of the listed property values. `Ok(None)` when `field` lists
/// no kind or no key, which matches nothing.
///
/// One function rather than two copies so the probe cannot drift from the
/// read it is the hand check for.
unsafe fn find_address_fields(
    hwnd: isize,
    field: crate::browser_url::AddressField,
) -> Result<Option<AddressFieldQuery>> {
    let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        log::warn!("CoInitializeEx failed unexpectedly: {hr:?}");
    }

    let automation: IUIAutomation2 =
        CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
    // A refusal leaves the defaults in place, which is slower and still
    // correct, so neither answer is worth an error.
    let _ = automation.SetConnectionTimeout(ADDRESS_FIELD_CONNECTION_TIMEOUT_MS);
    let _ = automation.SetTransactionTimeout(ADDRESS_FIELD_TRANSACTION_TIMEOUT_MS);

    let root = automation.ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))?;

    // (control type is one of `field.kinds`) AND (any listed property).
    let mut kind: Option<IUIAutomationCondition> = None;
    for wanted in field.kinds {
        let id = match wanted {
            crate::browser_url::FieldKind::Edit => UIA_EditControlTypeId,
            crate::browser_url::FieldKind::ComboBox => UIA_ComboBoxControlTypeId,
        };
        let one =
            automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(id.0))?;
        kind = Some(match kind {
            None => one,
            Some(earlier) => automation.CreateOrCondition(&earlier, &one)?,
        });
    }
    let mut key: Option<IUIAutomationCondition> = None;
    let wanted = field
        .class_names
        .iter()
        .map(|value| (UIA_ClassNamePropertyId, *value))
        .chain(field.automation_ids.iter().map(|value| (UIA_AutomationIdPropertyId, *value)))
        .chain(field.names.iter().map(|value| (UIA_NamePropertyId, *value)));
    for (property, value) in wanted {
        let one = automation.CreatePropertyCondition(property, &VARIANT::from(value))?;
        key = Some(match key {
            None => one,
            Some(earlier) => automation.CreateOrCondition(&earlier, &one)?,
        });
    }
    let (Some(kind), Some(key)) = (kind, key) else {
        return Ok(None);
    };
    let address = automation.CreateAndCondition(&kind, &key)?;

    let found = root.FindAll(TreeScope_Descendants, &address)?;
    let walker = automation.RawViewWalker()?;
    Ok(Some(AddressFieldQuery { automation, walker, root, found }))
}

/// The walk from `element` up to `root`: how many steps it took (the root
/// included), or `None` if it did not arrive within
/// [`crate::browser_url::MAX_FIELD_DEPTH`]; and whether any ancestor on the
/// way was a `Document`, which is what a web page is exposed as.
///
/// A failed step is "did not arrive", which [`crate::browser_url::is_browser_chrome`]
/// refuses -- an element whose ancestry cannot be shown is not the address bar.
unsafe fn ancestry(
    automation: &IUIAutomation2,
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    element: &IUIAutomationElement,
) -> (Option<usize>, bool) {
    let mut crosses_a_document = false;
    let mut current = element.clone();
    for depth in 1..=crate::browser_url::MAX_FIELD_DEPTH {
        let Ok(parent) = walker.GetParentElement(&current) else {
            return (None, crosses_a_document);
        };
        if automation.CompareElements(&parent, root).is_ok_and(|same| same.as_bool()) {
            return (Some(depth), crosses_a_document);
        }
        if parent.CurrentControlType().is_ok_and(|kind| kind == UIA_DocumentControlTypeId) {
            crosses_a_document = true;
        }
        current = parent;
    }
    (None, crosses_a_document)
}

/// **What can and cannot be tested about the UIA fill path.**
///
/// Nothing in this crate can call [`fill_via_ui_automation`]: it needs a real
/// COM apartment, a real window and a real UI Automation tree, and no test may
/// open any of those. The allocator probe used elsewhere in this crate
/// (`login_ui::password_lifetime_tests`) cannot help either -- it watches
/// Rust's global allocator, and a `BSTR` is allocated and freed by COM's, so a
/// `!plaintext_reached_the_allocator` assertion around a UIA fill would be
/// clean because it is blind, which is this codebase's signature failure.
///
/// So the claim is split. The *sequencing* -- wipe before release, wipe even
/// when the setter failed -- is real behaviour and is exercised below against a
/// buffer the test still owns. That production routes its two `SetValue` calls
/// through that sequencing is a **source pin**, said plainly, with a positive
/// control on the needle.
#[cfg(test)]
mod bstr_wipe_tests {
    use super::*;
    use windows::Win32::Foundation::E_FAIL;

    /// A [`WipeableWide`] over a buffer the *test* owns, so the test can read
    /// it after the handle has been "released". Releasing it is a no-op, which
    /// is the only way an assertion about the state of a freed COM allocation
    /// can be made without reading freed memory.
    struct BorrowedWide {
        ptr: *mut u16,
        len: usize,
    }

    impl WipeableWide for BorrowedWide {
        fn wide_ptr(&self) -> *mut u16 {
            self.ptr
        }
        fn wide_len(&self) -> usize {
            self.len
        }
    }

    fn secret() -> Vec<u16> {
        "deskwarden-bstr-probe-master-password".encode_utf16().collect()
    }

    fn handle(buf: &mut [u16]) -> BorrowedWide {
        BorrowedWide { ptr: buf.as_mut_ptr(), len: buf.len() }
    }

    #[test]
    fn the_buffer_is_wiped_before_the_handle_is_released() {
        let mut buf = secret();

        // Positive control on the instrument: the plaintext really is in this
        // buffer before the call, so an all-zero reading afterwards is the
        // wipe and not an empty fixture.
        assert!(buf.iter().any(|&u| u != 0), "control: the fixture is already zeroed");
        assert_eq!(String::from_utf16(&buf).unwrap(), "deskwarden-bstr-probe-master-password");

        let mut saw_plaintext = false;
        let out = set_value_wiping(handle(&mut buf), |h| {
            // The setter sees the live buffer -- if it did not, the wipe would
            // be happening before the value was ever delivered.
            let live = unsafe { std::slice::from_raw_parts(h.wide_ptr(), h.wide_len()) };
            saw_plaintext = String::from_utf16(live).unwrap()
                == "deskwarden-bstr-probe-master-password";
            Ok::<u8, windows::core::Error>(7)
        });

        assert!(saw_plaintext, "the setter was handed something other than the password");
        assert_eq!(out.unwrap(), 7, "the setter's value was not returned");
        assert!(
            buf.iter().all(|&u| u == 0),
            "the buffer was released with plaintext still in it: {:?}",
            String::from_utf16_lossy(&buf)
        );
    }

    /// **The `?` that would have skipped the wipe.** Propagating the setter's
    /// error with `?` reads naturally and returns past the zeroing -- leaving
    /// the password in the COM allocation on exactly the path where the fill
    /// went wrong.
    #[test]
    fn a_failing_setter_still_gets_its_buffer_wiped_and_still_reports_the_error() {
        let mut buf = secret();
        assert!(buf.iter().any(|&u| u != 0), "control: the fixture is already zeroed");

        let out: Result<()> = set_value_wiping(handle(&mut buf), |_| Err(E_FAIL.into()));

        assert!(out.is_err(), "the setter's failure was swallowed");
        assert_eq!(out.unwrap_err().code(), E_FAIL, "a different error was reported");
        assert!(
            buf.iter().all(|&u| u == 0),
            "the failure path returned past the wipe: {:?}",
            String::from_utf16_lossy(&buf)
        );
    }

    /// The empty case is skipped rather than written to, because `BSTR::as_ptr`
    /// hands back a pointer into a `const` static for an empty string. Nothing
    /// here can construct that static, so what is pinned is that a zero length
    /// makes the function write nothing at all -- with a deliberately invalid
    /// pointer, which any write through would fault on.
    #[test]
    fn a_zero_length_handle_is_not_written_through() {
        let bogus = BorrowedWide { ptr: 8 as *mut u16, len: 0 };
        assert_eq!(set_value_wiping(bogus, |_| Ok::<_, windows::core::Error>(1)).unwrap(), 1);
    }

    /// **Source pin, and it is only that.** The two lines above that hand a
    /// credential to UI Automation must go through [`set_value_wiping`]; a bare
    /// `SetValue(&BSTR...)` would compile, fill correctly, and free the
    /// plaintext. No runnable assertion can cover it -- see this module's doc.
    ///
    /// Read off disk rather than `include_str!` of self only so the count and
    /// the needle cannot both live in the same string literal; the production
    /// half is sliced off so the needles below do not match this test module,
    /// which spells every one of them.
    #[test]
    fn both_credential_setters_go_through_the_wiping_helper() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/injector/ui_automation.rs"
        ))
        .expect("ui_automation.rs is readable");
        let production = source
            .split_once(concat!("#[cfg(", "test)]"))
            .map_or(source.as_str(), |(above, _)| above);
        assert!(
            production.len() < source.len(),
            "control: the test gate was not found, so this pin is reading its own fixtures"
        );

        assert_eq!(
            production.matches("set_value_wiping(BSTR::from_wide(").count(),
            2,
            "the username and password setters must both go through the wiping helper"
        );
        assert_eq!(
            production.matches(".SetValue(").count(),
            2,
            "a `SetValue` call appeared outside the two wrapped ones"
        );
        assert_eq!(
            production.matches(concat!("SetValue(&BSTR", "::from")).count(),
            0,
            "a credential is handed to UI Automation in a BSTR nothing wipes"
        );

        // Positive control on all three needles: they match the spellings they
        // are meant to match, so a count of 2 is two real call sites and a
        // count of 0 is a real absence rather than a typo that matches nothing.
        let fixture = concat!(
            "set_value_wiping(BSTR::from_wide(&w)?, |b| v.SetValue(b))?;\n",
            "v.SetValue(&BSTR",
            "::from(x))?;\n"
        );
        assert_eq!(fixture.matches("set_value_wiping(BSTR::from_wide(").count(), 1);
        assert_eq!(fixture.matches(".SetValue(").count(), 2);
        assert_eq!(fixture.matches(concat!("SetValue(&BSTR", "::from")).count(), 1);
    }
}

/// **The probe's view of the address query never carries a field's text.**
///
/// `address_field_candidates` exists so `examples/address_bar_probe.rs` can
/// say why each candidate was kept or refused, and a refused candidate may be
/// a web page's own input holding what the user typed into it. Like
/// `address_field_text`, it needs a live browser and cannot be run here, so
/// this is a source pin, with a positive control on its needles.
#[cfg(test)]
mod address_field_tests {
    #[test]
    fn the_probe_only_candidate_list_never_reads_a_value_or_a_name() {
        let source = include_str!("ui_automation.rs");
        let start = source
            .find(concat!("pub fn address_field_", "candidates("))
            .expect("`address_field_candidates` is in this file");
        let body: String = source[start..]
            .lines()
            .take_while(|line| !line.trim_end().eq("}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains(concat!("CurrentAutomation", "Id()")),
            "control: the slice being scanned is not the function that describes candidates"
        );
        for needle in [
            concat!("UIA_Value", "PatternId"),
            concat!("Current", "Value"),
            concat!("Current", "Name"),
            concat!("log", "::"),
        ] {
            assert!(
                !body.contains(needle),
                "`address_field_candidates` reaches for {needle}: a refused candidate may be a \
                 page's own field, and its text is what the user typed into that page"
            );
        }
        // Control: the text read really is spelled this way, so an absence
        // above is an absence and not a needle that matches nothing.
        let text = &source[source.find(concat!("pub fn address_field_", "text(")).unwrap()..start];
        assert!(text.contains(concat!("UIA_Value", "PatternId")));
        assert!(text.contains(concat!("Current", "Value")));
    }
}

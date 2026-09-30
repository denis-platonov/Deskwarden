//! **Where a sequence is about to go, as a value.**
//!
//! The preflight (`vault_window::preflight`) has to say one thing before a
//! password is typed: *which window* is about to receive it. That is a Win32
//! question, and it cannot be asked from a test.
//!
//! **It used to be two.** The second was *whether the control holding focus is
//! masked*, asked of UI Automation, and it fed a refusal the owner had removed
//! -- browsers do not reliably report a page's fields as masked, a one-time-code
//! box never is, and the value is one copy away in the vault window anyway.
//! See `vault_window::preflight`'s module doc. The question went with the
//! refusal rather than staying as an unread field, because asking it was not
//! free: a UI Automation provider that did not answer made this whole
//! description `None`, and `None` is a refused fill.
//!
//! So it is asked once, here, and the answer is a plain
//! [`SendTarget`] -- after which every decision made about it is a pure
//! function of a value a test can construct by hand. That split is the whole
//! point of this module: [`describe_foreground`] is the untestable half and
//! does nothing but observe, and [`matches_rule`] is the tested half and
//! decides.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

/// Everything the preflight needs to say about where a sequence is going.
///
/// This is a VALUE, deliberately: the decision to send is a pure function of
/// it, so the decision can be tested without a real foreground window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    pub title: String,
    pub image_name: String,
    pub pid: u32,
    pub class_name: String,
}

/// True when `target` is the process the rule was written for.
///
/// Case-insensitive because Windows image names are.
pub fn matches_rule(target: &SendTarget, rule_image: &str) -> bool {
    target.image_name.eq_ignore_ascii_case(rule_image)
}

/// The foreground window right now, or `None` when any part of the answer is
/// unavailable.
///
/// **`None` rather than a default**, and the difference is the security
/// property: a `SendTarget` whose image name fell back to a guess could
/// satisfy [`matches_rule`] for a window that is not the rule's, which is
/// exactly the state [`crate::vault_window::preflight::verdict`] exists to
/// distinguish. An unknown target must not read as a safe one, so there is no
/// way to construct one from a partial observation.
///
/// **UI Automation is not asked anything**, and that is the owner's ruling
/// rather than an omission. This used to end by asking whether the focused
/// control was masked and returned `None` when the provider did not answer --
/// which, once the masking refusal was removed, would have refused a fill
/// over a question whose answer nothing reads. Everything left is plain Win32:
/// the foreground event and the window class.
///
/// Nothing in this crate can call this from a test: it needs a real
/// foreground window. It is the observation half of the module and it makes
/// no decisions -- see the module doc.
pub fn describe_foreground() -> Option<SendTarget> {
    let event = crate::window_watch::current_foreground_event()?;
    if event.exe_name.is_empty() {
        return None;
    }
    let class_name = class_name_of(event.hwnd)?;
    Some(SendTarget {
        title: event.title,
        image_name: event.exe_name,
        pid: event.pid,
        class_name,
    })
}

/// The window class of `hwnd`, or `None` if Win32 declined to say.
fn class_name_of(hwnd: isize) -> Option<String> {
    let mut buffer = [0u16; 256];
    // SAFETY: `hwnd` came from `GetForegroundWindow` this call, and the buffer
    // is a live stack array whose length is handed to the call.
    let copied = unsafe { GetClassNameW(HWND(hwnd as *mut core::ffi::c_void), &mut buffer) };
    if copied <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..copied as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(image: &str) -> SendTarget {
        SendTarget {
            title: "SAP Logon 760 - Sign in".to_string(),
            image_name: image.to_string(),
            pid: 7412,
            class_name: "SAPFEWndClass".to_string(),
        }
    }

    #[test]
    fn the_rule_matches_its_own_image_and_nothing_else() {
        assert!(matches_rule(&target("saplogon.exe"), "saplogon.exe"));
        assert!(
            matches_rule(&target("SAPLOGON.EXE"), "saplogon.exe"),
            "Windows image names are compared case-insensitively"
        );
        assert!(
            !matches_rule(&target("slack.exe"), "saplogon.exe"),
            "a different process must not satisfy the rule -- this is the check that stops a \
             password being typed into a chat box"
        );
    }
}

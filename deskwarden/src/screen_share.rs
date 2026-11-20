//! Settings > Security > Privacy > "Hide passwords while I share my screen"
//! (spec Part C 2.4, and the tip "Passwords stay as dots during Teams, Zoom or
//! Meet screen sharing -- even if you press reveal.").
//!
//! One Win32 call: `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the
//! vault window. A window with that affinity is drawn normally on the user's
//! own monitor and left out of every screen capture and share, so the whole
//! app is hidden from a share rather than only its password fields. (Windows
//! offers no per-control version of this.) It needs Windows 10 2004 or later;
//! on anything older the call fails, which [`apply`] reports and the caller
//! logs.
//!
//! This module opens no window: it is handed the title of one the process
//! already owns, the way `login_ui::round_window_corners` is.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};

/// **Whether this build hides its windows from screen capture at all.**
///
/// A dev build -- any copy run under `DESKWARDEN_PROFILE`
/// (`crate::app_profile`) -- never does, so its cards and windows can be
/// screenshotted and recorded while it is being worked on. The installed
/// release copy, which runs with no profile, keeps every protection.
#[must_use]
pub fn capture_protection_enabled() -> bool {
    protection_for(crate::app_profile::active().is_some())
}

/// [`capture_protection_enabled`]'s decision, pure.
#[must_use]
pub const fn protection_for(profiled: bool) -> bool {
    !profiled
}

/// The affinity value for the switch, as Windows spells it: 0x11
/// (`WDA_EXCLUDEFROMCAPTURE`) while hiding, 0 (`WDA_NONE`) otherwise.
#[must_use]
pub const fn affinity_for(hide: bool) -> u32 {
    if hide {
        WDA_EXCLUDEFROMCAPTURE.0
    } else {
        WDA_NONE.0
    }
}

/// Sets (or clears) the capture exclusion on this process's window titled
/// `window_title`. `true` when the window now has the asked-for affinity,
/// `false` when there is no such window yet or Windows refused -- the caller
/// retries on a later frame in the first case and has nothing more to do in
/// the second. In a test process, which owns no window, this touches nothing.
pub fn apply(window_title: &str, hide: bool) -> bool {
    let Some(handle) = crate::foreground::own_window_titled(window_title) else {
        return false;
    };
    let hwnd = HWND(handle as *mut core::ffi::c_void);
    let hide = hide && capture_protection_enabled();
    let affinity = if hide { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    match unsafe { SetWindowDisplayAffinity(hwnd, affinity) } {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "SetWindowDisplayAffinity({:#x}) was refused for the vault window ({error}); it will not \
                 be hidden from screen sharing on this machine",
                affinity.0
            );
            false
        }
    }
}

/// Remembers what was last applied, so a window that is asked every frame
/// makes the Win32 call once per change (and once more only if the window was
/// not there yet).
#[derive(Debug, Default)]
pub struct Applied(std::cell::Cell<Option<bool>>);

impl Applied {
    /// Brings the window to `hide`, calling [`apply`] only when that is not
    /// already what was last applied successfully.
    pub fn sync(&self, window_title: &str, hide: bool) {
        if self.0.get() == Some(hide) {
            return;
        }
        if apply(window_title, hide) {
            self.0.set(Some(hide));
        }
    }

    /// What is applied, `None` before the first success.
    #[must_use]
    pub fn current(&self) -> Option<bool> {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_unprofiled_copy_hides_from_capture() {
        assert!(protection_for(false));
        assert!(!protection_for(true));
    }

    #[test]
    fn every_card_asks_the_build_before_hiding_from_capture() {
        for (name, code) in [
            ("picker_prompt", include_str!("picker_prompt.rs")),
            ("prompt_card", include_str!("prompt_card.rs")),
            ("locked_card", include_str!("locked_card.rs")),
            ("notice_card", include_str!("notice_card.rs")),
            ("save_login_card", include_str!("save_login_card.rs")),
            ("unlock_prompt", include_str!("unlock_prompt.rs")),
            ("generate_prompt", include_str!("generate_prompt.rs")),
            ("tray_popup", include_str!("tray_popup.rs")),
        ] {
            let call = code.find(concat!("unsafe { SetWindowDisplay", "Affinity(hwnd(")).unwrap();
            let gate = code[..call].rfind(concat!("capture_protection_", "enabled()"));
            assert!(gate.is_some_and(|g| call - g < 120), "{name} hides from capture without asking the build");
        }
    }

    #[test]
    fn hiding_is_the_exclude_from_capture_affinity() {
        assert_eq!(affinity_for(true), 0x11);
        assert_eq!(affinity_for(false), 0);
    }

    #[test]
    fn a_process_with_no_such_window_changes_nothing_and_asks_again_later() {
        let applied = Applied::default();
        applied.sync("No such Deskwarden window", true);
        assert_eq!(applied.current(), None, "claimed a success with no window");
    }
}

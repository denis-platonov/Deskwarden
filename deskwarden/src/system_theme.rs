//! Whether Windows is in dark mode, for Settings > General > Theme > System
//! (spec D2).
//!
//! Windows keeps the choice in one DWORD, `AppsUseLightTheme`, under the
//! current user's `Themes\Personalize` key: 0 means apps are dark, 1 light,
//! and a machine that has never been asked has no value at all (light).
//! Read-only and cheap, so the vault window asks once a second rather than
//! subscribing to a change message it has no window procedure for.

/// What the value means. `None` (no value, or a key that could not be read)
/// is light: it is what Windows itself does.
#[must_use]
pub const fn dark_from_value(apps_use_light_theme: Option<u32>) -> bool {
    matches!(apps_use_light_theme, Some(0))
}

/// Reads `AppsUseLightTheme`.
fn read_apps_use_light_theme() -> Option<u32> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
    let subkey = HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = HSTRING::from("AppsUseLightTheme");
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut key).ok().ok()?;
        let mut data = [0u8; 4];
        let mut len = 4u32;
        let read = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, Some(data.as_mut_ptr()), Some(&mut len));
        let _ = RegCloseKey(key);
        (read.is_ok() && len == 4).then(|| u32::from_le_bytes(data))
    }
}

/// Whether Windows' app theme is dark right now.
#[must_use]
pub fn apps_use_dark() -> bool {
    dark_from_value(read_apps_use_light_theme())
}

/// Windows' answer, re-read at most once a second: the vault window paints
/// every frame and the registry should not be opened on each.
#[derive(Debug, Default)]
pub struct Watcher(std::cell::Cell<Option<(f64, bool)>>);

impl Watcher {
    /// How often the registry is asked, in seconds.
    pub const EVERY: f64 = 1.0;

    /// Whether Windows is dark at `now` (seconds on any steady clock), asking
    /// `read` only when the last answer is [`Self::EVERY`] old or there is
    /// none.
    pub fn dark_at(&self, now: f64, read: impl FnOnce() -> bool) -> bool {
        match self.0.get() {
            Some((at, dark)) if now - at < Self::EVERY && now >= at => dark,
            _ => {
                let dark = read();
                self.0.set(Some((now, dark)));
                dark
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watcher_asks_once_a_second_not_once_a_frame() {
        let watcher = Watcher::default();
        let asked = std::cell::Cell::new(0);
        let read = |answer: bool| {
            asked.set(asked.get() + 1);
            answer
        };
        assert!(watcher.dark_at(10.0, || read(true)));
        assert!(watcher.dark_at(10.4, || read(false)), "asked again inside the second");
        assert_eq!(asked.get(), 1);
        assert!(!watcher.dark_at(11.1, || read(false)), "a change is picked up after the second");
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn only_a_zero_means_dark() {
        assert!(dark_from_value(Some(0)));
        assert!(!dark_from_value(Some(1)));
        assert!(!dark_from_value(None), "no value is Windows' light default");
    }

    #[test]
    fn reading_the_real_value_does_not_panic() {
        let _ = apps_use_dark();
    }
}

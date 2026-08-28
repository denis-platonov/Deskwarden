//! Repairing a logon entry that predates the `--autostart` flag.
//!
//! # The defect this exists for, measured on the owner's machine
//!
//! The installer writes
//! `"…\deskwarden.exe" --autostart` — and has for some time. But it writes it
//! **only when the autostart task is selected**, and a self-update
//! deliberately passes `/MERGETASKS=!autostart` so that an update never
//! rewrites a Run value the user already has. `deskwarden.iss` says so where
//! it writes the line, and it is right to: silently re-adding autostart to
//! someone who turned it off would be worse than this.
//!
//! The consequence is that an install predating the flag keeps a **bare**
//! `deskwarden.exe` in `HKCU\…\Run`, for ever, and no update can heal it.
//! `main.rs`'s own comment already knew: *"every install that exists today
//! therefore carries a bare `deskwarden.exe` and will keep carrying it until
//! the user reinstalls by hand."*
//!
//! What that costs is not cosmetic. A bare path is a
//! [`crate::LaunchIntent::UserLaunch`], whose first surface is a window — so
//! **every sign-in draws a startup window inside the daemon**, maps the
//! OpenGL driver, and keeps it for the life of the session. Measured: 98.6 MB
//! resident with `nvoglv64.dll` at 41.1 MB, against 35.9 MB for a tray that
//! never drew one. The user reported it as "tray again is 50Mb".
//!
//! # What this repairs, and what it will not touch
//!
//! Exactly one case: a value under our own name that points at **this
//! executable** and lacks the flag. Then the flag is added and nothing else
//! changes.
//!
//! It does **not** create an entry that is absent. Somebody who never asked
//! for autostart, or turned it off, must not find it back — that is the same
//! judgement `/MERGETASKS=!autostart` makes, and this would be worthless if it
//! quietly disagreed with it.
//!
//! It does **not** touch a value pointing somewhere else. A path that is not
//! this exe is not ours to rewrite, whoever put it there.

/// The word that tells this app a launch was Windows signing the user in.
///
/// Declared here as well as in `main.rs` because that one lives in the
/// BINARY and this module is in the library, which cannot see it. Two
/// spellings of one word is exactly the drift this crate keeps pinning, so
/// `the_flag_is_spelled_the_same_as_the_binarys` reads `main.rs` and fails if
/// they ever differ.
pub const AUTOSTART_FLAG: &str = "--autostart";

/// What to write, if anything, for the logon entry.
///
/// Pure, and the whole decision. `current` is the raw value already in the
/// registry, `None` when there is none; `our_exe` is this process's own path.
///
/// Returns `None` when there is nothing to do — which is every case except
/// the one this module exists for.
#[must_use]
pub fn repaired_value(current: Option<&str>, our_exe: &std::path::Path) -> Option<String> {
    // Absent means the user does not want autostart. Not ours to add.
    let current = current?;

    // Already correct. Matched on the flag rather than on the whole string,
    // because a value may legitimately carry other arguments one day and this
    // must not fight whatever adds them.
    if current.contains(AUTOSTART_FLAG) {
        return None;
    }

    // The path, with the quotes the installer writes stripped. Compared
    // against our own exe so that a Run value belonging to something else --
    // or to an older Deskwarden in another directory the user still wants --
    // is left exactly as it is.
    let quoted = current.trim();
    let path = quoted.trim_matches('"');
    if !paths_match(std::path::Path::new(path), our_exe) {
        return None;
    }

    Some(format!("\"{path}\" {}", AUTOSTART_FLAG))
}

/// Reads the logon entry, and rewrites it if [`repaired_value`] says so.
///
/// Best-effort and silent on failure by design: this is housekeeping, and an
/// app that refused to start because it could not tidy a registry value would
/// be worse than the value. Every outcome is logged, because a repair that
/// happened invisibly is one nobody can confirm.
pub fn repair_logon_entry() {
    use windows::core::{PCWSTR, HSTRING};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_SET_VALUE, REG_SZ,
    };

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let subkey = HSTRING::from(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = HSTRING::from("Deskwarden");

    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ | KEY_SET_VALUE,
            &mut key,
        )
        .is_err()
        {
            return;
        }

        // Asked for its size first, then read: a Run value is short, but
        // guessing a buffer for something another program also writes is how
        // a truncated read becomes a wrong decision.
        let mut len: u32 = 0;
        let sized = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, Some(&mut len));
        let current = if sized.is_ok() && len > 0 {
            let mut buf = vec![0u8; len as usize];
            let mut got = len;
            if RegQueryValueExW(
                key,
                PCWSTR(name.as_ptr()),
                None,
                None,
                Some(buf.as_mut_ptr()),
                Some(&mut got),
            )
            .is_ok()
            {
                let wide: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|c| *c != 0)
                    .collect();
                Some(String::from_utf16_lossy(&wide))
            } else {
                None
            }
        } else {
            None
        };

        match repaired_value(current.as_deref(), &exe) {
            None => log::debug!("the logon entry needs no repair"),
            Some(fixed) => {
                let value = HSTRING::from(fixed.as_str());
                let bytes = std::slice::from_raw_parts(
                    value.as_ptr().cast::<u8>(),
                    (value.len() + 1) * 2,
                );
                if RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)).is_ok() {
                    log::info!(
                        "the logon entry predated the --autostart flag and was repaired; this \
                         app will start to the tray at sign-in rather than opening a window"
                    );
                } else {
                    log::warn!("the logon entry could not be repaired; it is not fatal");
                }
            }
        }
        let _ = RegCloseKey(key);
    }
}

/// Whether two paths name the same file, for the purposes above.
///
/// Case-insensitive because Windows paths are, and compared after
/// normalisation so `AppData\Local\..\Local\deskwarden.exe` is not treated as
/// a different program. A comparison that failed here would leave the defect
/// in place, which is the safe direction, so this errs toward "not ours".
fn paths_match(a: &std::path::Path, b: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase()
    };
    canon(a) == canon(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const EXE: &str = r"C:\Users\someone\AppData\Local\deskwarden\deskwarden.exe";

    /// **The defect.** A value written before the flag existed, pointing at
    /// this exe. Every sign-in from one of these draws a window in the daemon
    /// and maps the graphics driver for the session.
    #[test]
    fn a_bare_path_to_this_exe_gains_the_flag() {
        let got = repaired_value(Some(&format!("\"{EXE}\"")), Path::new(EXE));
        assert_eq!(got, Some(format!("\"{EXE}\" --autostart")));
    }

    /// Unquoted too: not every writer quotes, and a path with no spaces
    /// survives without quotes.
    #[test]
    fn an_unquoted_path_is_repaired_and_comes_back_quoted() {
        let got = repaired_value(Some(EXE), Path::new(EXE));
        assert_eq!(
            got,
            Some(format!("\"{EXE}\" --autostart")),
            "the repaired value must be quoted, or a path with a space becomes two arguments"
        );
    }

    /// **Nothing to do is the common case**, and each arm of it matters.
    #[test]
    fn a_value_that_is_already_right_is_left_alone() {
        assert_eq!(repaired_value(Some(&format!("\"{EXE}\" --autostart")), Path::new(EXE)), None);
    }

    /// **Absent means the user does not want it.** Creating one here would
    /// silently disagree with `/MERGETASKS=!autostart`, which exists so that
    /// an update cannot re-add autostart to somebody who turned it off.
    #[test]
    fn an_absent_entry_is_not_created() {
        assert_eq!(
            repaired_value(None, Path::new(EXE)),
            None,
            "a missing Run value is a choice, not a defect to repair"
        );
    }

    /// A value pointing at something else is not ours to rewrite, whoever put
    /// it there and whatever it is.
    #[test]
    fn a_value_pointing_elsewhere_is_untouched() {
        let other = r"C:\Program Files\Something Else\thing.exe";
        assert_eq!(repaired_value(Some(&format!("\"{other}\"")), Path::new(EXE)), None);
    }

    /// Windows paths are case-insensitive, and the registry may hold a
    /// different casing than `current_exe` reports. Missing that would leave
    /// the defect in place on a machine that merely spelled it differently.
    #[test]
    fn casing_does_not_stop_the_repair() {
        let shouty = EXE.to_uppercase();
        assert!(
            repaired_value(Some(&format!("\"{shouty}\"")), Path::new(EXE)).is_some(),
            "a differently-cased path was treated as another program"
        );
    }

    /// Control for every `None` above: the function does return `Some` for
    /// the one case it is for, so the assertions that it returns `None` are
    /// not passing on a function that never repairs anything.
    #[test]
    fn the_repair_is_reachable_at_all() {
        assert!(repaired_value(Some(&format!("\"{EXE}\"")), Path::new(EXE)).is_some());
    }

    /// The library's spelling and the binary's must be one word.
    #[test]
    fn the_flag_is_spelled_the_same_as_the_binarys() {
        let main_rs = include_str!("main.rs");
        let declared = main_rs
            .lines()
            .find(|l| l.trim_start().starts_with("const AUTOSTART_FLAG"))
            .expect("control: main.rs no longer declares AUTOSTART_FLAG");
        assert!(
            declared.contains(AUTOSTART_FLAG),
            "the binary spells the autostart flag differently from this module: {declared}"
        );
    }

    /// **The installer must keep writing the flag.** This module heals old
    /// installs; it is not a licence for the installer to stop being right,
    /// and a fresh install should never need repairing.
    #[test]
    fn the_installer_still_writes_the_flag_itself() {
        let iss = include_str!("../installer/deskwarden.iss");
        let run_line = iss
            .lines()
            .find(|l| l.contains("CurrentVersion\\Run") && l.contains("ValueData"))
            .expect("control: the installer no longer writes a Run value at all");
        assert!(
            run_line.contains("--autostart"),
            "the installer's Run value lost its --autostart flag, so every NEW install would \
             draw a window at sign-in and map the graphics driver: {run_line}"
        );
    }
}

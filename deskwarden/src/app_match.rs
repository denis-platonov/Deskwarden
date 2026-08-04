use serde::{Deserialize, Serialize};

pub const APP_MATCH_FIELD_NAME: &str = "deskwarden:app-match";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Prompt,
    Hotkey,
    Auto,
}

/// What the user pointed at when they said "that app", as it is stored in the
/// `deskwarden:app-match` custom field on a vault item.
///
/// **Every field but `process` is optional on the way in.** This value is JSON
/// sitting in real users' vaults; matches saved before `title` and `path`
/// existed are `{"process":...,"trigger":...}` and must keep parsing exactly as
/// they did. `#[serde(default)]` is the mechanism, and the pairing with
/// `skip_serializing_if` means a match that carries neither still serializes to
/// the *old* two-key shape -- so the round-trip is stable in both directions
/// and nothing rewrites a user's field into a longer one for no reason.
///
/// The empty string is how "not recorded" is spelled, rather than
/// `Option<String>`: [`crate::window_list::list_windows`] only ever lists
/// windows that have a title, and only ever builds a row it could resolve a
/// path for, so a captured value is never empty and there is no third state to
/// represent. Both readers treat empty as absent
/// ([`crate::match_engine::MatchEngine::rebuild`] and [`Self::launchable_path`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMatch {
    /// The executable's file name (`Speedtest.exe`), as attributed by
    /// [`crate::window_watch::attribute_window`] -- never the name of a window
    /// host.
    pub process: String,
    /// The window's title at the moment the user picked it.
    ///
    /// **This is the only thing that can identify a suspended Microsoft Store
    /// app.** A minimised UWP app has no `Windows.UI.Core.CoreWindow`, so its
    /// frame cannot be attributed to any executable at all and there is no exe
    /// name to match on -- but the frame still carries the app's title. See
    /// [`crate::match_engine::MatchEngine::lookup`] for the rule that keeps
    /// this from becoming a second, looser way to match everything else.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The full image path of the process that owned the picked window, so the
    /// item's detail pane can name and (eventually) open the app.
    ///
    /// **Never consulted when matching.** A corrupted value here cannot offer
    /// credentials to the wrong window; what it *can* do is name a program to
    /// start, which is a risk `process` never carried. See
    /// [`Self::launchable_path`] for the shape anything that launches it is
    /// expected to go through.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    pub trigger: TriggerMode,
}

impl AppMatch {
    /// A match on the executable name alone -- the whole of what this type was
    /// before `title` and `path` existed, and still the right constructor for
    /// every caller that is not holding a live window to copy them off.
    pub fn for_process(process: impl Into<String>, trigger: TriggerMode) -> Self {
        Self {
            process: process.into(),
            title: String::new(),
            path: String::new(),
            trigger,
        }
    }

    pub fn to_field_value(&self) -> String {
        serde_json::to_string(self).expect("AppMatch always serializes")
    }

    pub fn from_field_value(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value)
    }

    /// [`Self::path`], but only when it is still structurally the thing the
    /// picker resolved from a live window: an absolute local path whose file
    /// name is the very `process` this match is keyed on.
    ///
    /// **The point is that the field is untrusted.** `path` is a string in a
    /// vault item; it round-trips through a server, it can be edited in any
    /// Bitwarden client, and unlike every other part of this type it is an
    /// input to *starting a program* rather than to comparing two names. So
    /// the thing that would be executed has to be tied back to the thing that
    /// was matched, and that is the file-name check: whoever can edit the
    /// field can still only name a program called `Speedtest.exe`, which is a
    /// far smaller set than "any string".
    ///
    /// What is rejected, and why:
    ///
    ///  * an empty `path` (or an empty `process`) -- nothing was recorded, so
    ///    there is nothing to check and nothing to run;
    ///  * a file name that is not `process` -- the field has been changed to
    ///    point somewhere other than the app this match is for;
    ///  * anything that is not `X:\...` -- relative paths resolve against
    ///    whatever this process's working directory happens to be, and a UNC
    ///    path (`\\host\share\...`) would fetch the image over the network from
    ///    a machine named by the vault field;
    ///  * a `..` component, which is how a name-checked path is walked back out
    ///    of the directory it appears to sit in;
    ///  * a `"` anywhere, which is how one path becomes two command-line
    ///    arguments.
    ///
    /// **It is a structural check, not a proof.** It says nothing about whether
    /// the file exists, whether it is the same file that was there at capture
    /// time, or who signed it. A launcher is still expected to ask those
    /// questions (this crate already has `signature` for the last one); this
    /// exists so that the launcher cannot be written as
    /// `Command::new(&m.path)`.
    pub fn launchable_path(&self) -> Option<&str> {
        if self.process.is_empty() || self.path.is_empty() {
            return None;
        }
        if self.path.contains('"') {
            return None;
        }
        let mut components = self.path.split('\\');
        let drive = components.next()?;
        let mut drive_chars = drive.chars();
        match (drive_chars.next(), drive_chars.next(), drive_chars.next()) {
            (Some(letter), Some(':'), None) if letter.is_ascii_alphabetic() => {}
            _ => return None,
        }
        let mut file_name = None;
        for component in components {
            if component == ".." {
                return None;
            }
            file_name = Some(component);
        }
        // An empty last component is `C:\dir\`, which names a directory; and
        // `C:\` on its own never gets past `file_name`'s emptiness filter.
        let file_name = file_name.filter(|name| !name.is_empty())?;
        if !file_name.eq_ignore_ascii_case(&self.process) {
            return None;
        }
        Some(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A match with everything the picker can capture, as the picker builds it.
    fn captured() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest".to_string(),
            path: r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe".to_string(),
            trigger: TriggerMode::Prompt,
        }
    }

    #[test]
    fn field_name_matches_spec() {
        assert_eq!(APP_MATCH_FIELD_NAME, "deskwarden:app-match");
    }

    #[test]
    fn round_trips_through_json() {
        let original = AppMatch::for_process("RockstarGamesLauncher.exe", TriggerMode::Prompt);
        let json = original.to_field_value();
        let parsed = AppMatch::from_field_value(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn round_trips_the_title_and_the_path_too() {
        // The positive control for `serializes_the_old_two_key_shape_...`
        // below: that test passes trivially against a build whose new fields
        // are skipped unconditionally, and this one does not. Making either
        // `skip_serializing_if` always-skip gives
        //     left: AppMatch { title: "", .. }
        //     right: AppMatch { title: "Speedtest", .. }
        let original = captured();
        let parsed = AppMatch::from_field_value(&original.to_field_value()).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serializes_trigger_as_lowercase() {
        let m = AppMatch::for_process("mabl.exe", TriggerMode::Auto);
        assert_eq!(m.to_field_value(), r#"{"process":"mabl.exe","trigger":"auto"}"#);
    }

    #[test]
    fn serializes_every_captured_field_under_its_own_key() {
        // Pins the exact JSON that now lands in the user's vault -- the shape
        // the detail pane and any later launcher read back. Renaming a field
        // (or reordering the struct) fails here with a string diff.
        assert_eq!(
            captured().to_field_value(),
            r#"{"process":"Speedtest.exe","title":"Speedtest","path":"C:\\Program Files\\WindowsApps\\Speedtest\\Speedtest.exe","trigger":"prompt"}"#
        );
    }

    /// **Backward compatibility, stated as the literal bytes already sitting in
    /// real vaults.** Written out rather than produced by `to_field_value`, so
    /// this cannot re-derive its expectation from the type under test: dropping
    /// `#[serde(default)]` from either new field fails here with
    ///     called `Result::unwrap()` on an `Err` value: Error("missing field `title`", ...)
    #[test]
    fn a_match_saved_before_title_and_path_existed_still_parses() {
        let stored = r#"{"process":"RockstarGamesLauncher.exe","trigger":"hotkey"}"#;
        let parsed = AppMatch::from_field_value(stored).expect("an old field value must parse");
        assert_eq!(parsed.process, "RockstarGamesLauncher.exe");
        assert_eq!(parsed.trigger, TriggerMode::Hotkey);
        assert_eq!(parsed.title, "", "an old value records no title");
        assert_eq!(parsed.path, "", "an old value records no path");
    }

    #[test]
    fn serializes_the_old_two_key_shape_when_nothing_new_was_captured() {
        // So re-saving a process-only match does not grow the field with two
        // empty keys. Deleting either `skip_serializing_if` gives
        //     left: {"process":"a.exe","title":"","path":"","trigger":"auto"}
        //     right: {"process":"a.exe","trigger":"auto"}
        assert_eq!(
            AppMatch::for_process("a.exe", TriggerMode::Auto).to_field_value(),
            r#"{"process":"a.exe","trigger":"auto"}"#
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(AppMatch::from_field_value("not json").is_err());
    }

    #[test]
    fn a_captured_path_is_launchable() {
        // The positive control every rejection below needs: without it, a
        // `launchable_path` that answered `None` unconditionally would pass
        // the whole rest of this file.
        assert_eq!(
            captured().launchable_path(),
            Some(r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe")
        );
    }

    #[test]
    fn a_path_whose_file_name_is_not_the_matched_process_is_not_launchable() {
        // THE check. Deleting the `eq_ignore_ascii_case` comparison lets the
        // field name any program at all, and gives
        //     left: Some("C:\\Windows\\System32\\cmd.exe")  right: None
        let m = AppMatch { path: r"C:\Windows\System32\cmd.exe".to_string(), ..captured() };
        assert_eq!(m.launchable_path(), None);
    }

    #[test]
    fn the_file_name_check_ignores_case_the_way_windows_does() {
        // Paired with the test above so "not launchable" cannot be reached by
        // a comparison that is simply always false: the ONLY difference from
        // `a_captured_path_is_launchable` is the casing.
        let m = AppMatch {
            path: r"C:\Games\SPEEDTEST.EXE".to_string(),
            process: "speedtest.exe".to_string(),
            ..captured()
        };
        assert!(m.launchable_path().is_some());
    }

    #[test]
    fn a_path_that_is_not_an_absolute_local_path_is_not_launchable() {
        // Deleting the drive-letter check gives, for the first case,
        //     left: Some("\\\\attacker\\share\\Speedtest.exe")  right: None
        for path in [
            r"\\attacker\share\Speedtest.exe",
            r"Speedtest.exe",
            r".\Speedtest.exe",
            r"\Speedtest.exe",
            r"C:Speedtest.exe",
            r"http://attacker/Speedtest.exe",
            r"CC:\Speedtest.exe",
        ] {
            let m = AppMatch { path: path.to_string(), ..captured() };
            assert_eq!(m.launchable_path(), None, "path: {path}");
        }
    }

    #[test]
    fn a_path_that_walks_back_out_of_its_own_directory_is_not_launchable() {
        // Deleting the `".."` check gives
        //     left: Some("C:\\Program Files\\..\\Temp\\Speedtest.exe")  right: None
        let m = AppMatch {
            path: r"C:\Program Files\..\Temp\Speedtest.exe".to_string(),
            ..captured()
        };
        assert_eq!(m.launchable_path(), None);
    }

    #[test]
    fn a_path_carrying_a_quote_is_not_launchable() {
        // A quote survives the file-name check -- the last component here
        // really is `Speedtest.exe` -- and is how one path becomes two
        // command-line arguments. Deleting the `contains('"')` check gives
        //     left: Some("C:\\a\" --flag\\Speedtest.exe")  right: None
        let m = AppMatch { path: "C:\\a\" --flag\\Speedtest.exe".to_string(), ..captured() };
        assert_eq!(m.launchable_path(), None);
    }

    #[test]
    fn a_directory_is_not_launchable() {
        for path in [r"C:\Program Files\Speedtest\", r"C:\"] {
            let m = AppMatch { path: path.to_string(), ..captured() };
            assert_eq!(m.launchable_path(), None, "path: {path}");
        }
    }

    #[test]
    fn a_match_that_recorded_no_path_has_nothing_to_launch() {
        // Every match saved before this field existed, and the answer must be
        // "nothing to open" rather than "open something".
        let m = AppMatch::for_process("Speedtest.exe", TriggerMode::Prompt);
        assert_eq!(m.launchable_path(), None);
    }

    #[test]
    fn a_path_with_no_process_to_check_it_against_is_not_launchable() {
        let m = AppMatch { process: String::new(), ..captured() };
        assert_eq!(m.launchable_path(), None);
    }

    #[test]
    fn for_process_records_neither_a_title_nor_a_path() {
        let m = AppMatch::for_process("a.exe", TriggerMode::Prompt);
        assert_eq!((m.title.as_str(), m.path.as_str()), ("", ""));
    }
}

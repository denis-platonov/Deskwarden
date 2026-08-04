use serde::{Deserialize, Serialize};

pub const APP_MATCH_FIELD_NAME: &str = "deskwarden:app-match";

/// `skip_serializing_if` for [`AppMatch::hosted`]: the default is `false` and
/// the default is not written.
fn is_false(value: &bool) -> bool {
    !*value
}

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
/// **Every field but `process` and `trigger` is optional on the way in.** This
/// value is JSON sitting in real users' vaults; matches saved before `title`
/// and `path` existed are `{"process":...,"trigger":...}` and must keep parsing
/// exactly as they did, and the four-key `{process,title,path,trigger}` shape
/// one shipped commit wrote must keep parsing too -- as a match whose `hosted`
/// is `false`, which is exactly what makes its title inert. `#[serde(default)]` is the mechanism, and the pairing with
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
    /// The window's title at the moment the user picked it -- recorded **only
    /// for a window a host frame owned** (see [`Self::hosted`]).
    ///
    /// **This is the only thing that can identify a suspended Microsoft Store
    /// app.** A minimised UWP app has no `Windows.UI.Core.CoreWindow`, so its
    /// frame cannot be attributed to any executable at all and there is no exe
    /// name to match on -- but the frame still carries the app's title. See
    /// [`crate::match_engine::MatchEngine::lookup`] for the rule that keeps
    /// this from becoming a second, looser way to match everything else.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Whether the picked window was owned by a window host
    /// ([`crate::window_watch::is_host_process`]) while its real process was
    /// something else -- a Microsoft Store / UWP app presenting inside an
    /// `ApplicationFrameHost.exe` frame.
    ///
    /// **This is what makes [`Self::title`] matchable, and its absence is what
    /// makes every title already in a user's vault inert.** Review 31's
    /// Important 1: for one commit the picker recorded a title for *every* row,
    /// and [`crate::match_engine::MatchEngine::rebuild`] filed every one of
    /// them as a needle a host-owned frame could be matched by -- so a
    /// Store-packaged app whose title is content-derived could wear an ordinary
    /// desktop app's title and claim its match. Those saved values cannot be
    /// told apart from legitimate ones by inspection, and this app does not
    /// rewrite the user's vault, so the fix has to be a rule that makes them
    /// safe where they sit: matching on a title requires a POSITIVE record that
    /// the title came off a hosted frame. Old JSON has no `hosted` key,
    /// `#[serde(default)]` reads that as `false`, and a `false` title is never
    /// filed. The cost is that a Store app added during that one commit stops
    /// being matched while suspended until it is re-added, which fails closed.
    ///
    /// Skipped when `false`, so a match for an ordinary app still serializes to
    /// the shape it always had.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hosted: bool,
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
            hosted: false,
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
    ///    point somewhere other than the app this match is for. The comparison
    ///    is on the file name exactly as written, so a trailing dot or space
    ///    (which Win32 strips before opening the file, making `Speedtest.exe.`
    ///    and `Speedtest.exe` the same file) is a mismatch and is refused
    ///    rather than silently accepted;
    ///  * anything whose first three characters are not `X:\` -- relative
    ///    paths resolve against whatever this process's working directory
    ///    happens to be; a UNC path (`\\host\share\...`) would fetch the image
    ///    over the network from a machine named by the vault field; and the
    ///    device namespaces (`\\?\`, `\\.\`) reach things that are not files.
    ///    An all-forward-slash `C:/...` is rejected here too, though Windows
    ///    would accept it: the picker never writes one, so a path in that shape
    ///    did not come from this app;
    ///  * a `..` component -- how a name-checked path is walked back out of the
    ///    directory it appears to sit in -- and a `.` component, and any
    ///    component that is nothing but dots and spaces. Each component is
    ///    tested with its trailing dots and spaces removed, because Win32
    ///    removes them: `.. ` and `...` are `..` by the time the file is
    ///    opened. `/` is normalised to `\` before the split, because Windows
    ///    accepts both as separators and a `..` reached through the other one
    ///    is still a `..`;
    ///  * a `:` in any component but the drive, which is an alternate data
    ///    stream (`C:\Temp\evil.exe:stream\...`);
    ///  * a NUL, which truncates the string for the Win32 call that would open
    ///    it -- so what was checked and what would run are different paths;
    ///  * a `"` anywhere, which is how one path becomes two command-line
    ///    arguments.
    ///
    /// **It is a structural check, not a proof, and here is what it does NOT
    /// promise.** It says nothing about whether the file exists, whether it is
    /// the same file that was there at capture time, or who signed it. Two
    /// gaps are worth naming because the wording above could be read as
    /// covering them and it does not:
    ///
    ///  * **The drive letter is not proof of a local disk.** `subst` and a
    ///    mapped network drive both produce an `X:\` path whose image really is
    ///    fetched over the network or from wherever the mapping points. The
    ///    UNC rejection above removes one spelling of "somewhere else", not the
    ///    possibility. Nothing structural can close this -- it takes a
    ///    `GetDriveType`/`QueryDosDevice` call at launch time, which belongs to
    ///    the launcher.
    ///  * **A directory component may still be an 8.3 short name**
    ///    (`C:\PROGRA~1\...`), which names a directory this check cannot expand
    ///    or verify.
    ///
    /// What survives both gaps is the file-name check: whoever can edit the
    /// field can still only name a program called `Speedtest.exe`. The ceiling
    /// is "a differently-located file with that exact name", never arbitrary
    /// execution. A launcher is still expected to ask the questions above (this
    /// crate already has `signature` for the signing one); this exists so that
    /// the launcher cannot be written as `Command::new(&m.path)`.
    pub fn launchable_path(&self) -> Option<&str> {
        if self.process.is_empty() || self.path.is_empty() {
            return None;
        }
        if self.path.contains('"') || self.path.contains('\0') {
            return None;
        }
        // The drive prefix is checked on the path EXACTLY as stored -- before
        // any normalisation -- so `C:/...`, `C:Speedtest.exe`, `\\host\share`
        // and `\\?\C:\...` are all still refused.
        let mut prefix = self.path.chars();
        match (prefix.next(), prefix.next(), prefix.next()) {
            (Some(letter), Some(':'), Some('\\')) if letter.is_ascii_alphabetic() => {}
            _ => return None,
        }
        // Everything after `X:\`. Byte index 3 is safe: the three characters
        // just matched are ASCII.
        let normalised = self.path[3..].replace('/', "\\");
        let mut file_name = None;
        for component in normalised.split('\\') {
            if component.contains(':') {
                return None;
            }
            // As Win32 will see it: trailing dots and spaces are stripped
            // before the component is resolved.
            let trimmed = component.trim_end_matches(['.', ' ']);
            if trimmed == ".." || trimmed == "." {
                return None;
            }
            if trimmed.is_empty() && !component.is_empty() {
                // All dots and spaces -- `...`, `. `, ` `. Not a name this app
                // ever wrote, and not one worth reasoning about further.
                return None;
            }
            // Deliberately the UNTRIMMED component: the file-name comparison
            // below must see `Speedtest.exe.` as the mismatch it is.
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

    /// A match with everything the picker can capture, as the picker builds it
    /// for a hosted (Microsoft Store) row -- the only row a title is recorded
    /// for at all.
    fn captured() -> AppMatch {
        AppMatch {
            process: "Speedtest.exe".to_string(),
            title: "Speedtest".to_string(),
            hosted: true,
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
            r#"{"process":"Speedtest.exe","title":"Speedtest","hosted":true,"path":"C:\\Program Files\\WindowsApps\\Speedtest\\Speedtest.exe","trigger":"prompt"}"#
        );
    }

    /// **The four-key shape one shipped commit wrote**, as the literal bytes
    /// now sitting in real vaults: a title, a path, and no `hosted` key. It
    /// must still parse, and it must parse as NOT hosted -- which is what makes
    /// its title inert (see [`AppMatch::hosted`] and
    /// [`crate::match_engine::MatchEngine::rebuild`]). Making `hosted` default
    /// to `true`, or dropping its `#[serde(default)]`, fails here.
    #[test]
    fn the_shipped_four_key_shape_parses_and_is_not_hosted() {
        let stored = r#"{"process":"Ledgerline.exe","title":"Ledgerline - Invoices","path":"C:\\Apps\\Ledgerline.exe","trigger":"prompt"}"#;
        let parsed = AppMatch::from_field_value(stored).expect("a shipped field value must parse");
        assert_eq!(parsed.process, "Ledgerline.exe");
        assert_eq!(parsed.title, "Ledgerline - Invoices", "the value is not rewritten");
        assert_eq!(parsed.path, r"C:\Apps\Ledgerline.exe");
        assert!(!parsed.hosted, "an absent `hosted` key must read as false");
    }

    #[test]
    fn a_match_that_was_not_hosted_writes_no_hosted_key() {
        // So an ordinary app's match keeps the shape it always had rather than
        // growing a key that is always false. Deleting the
        // `skip_serializing_if` gives
        //     left: {"process":"Ledgerline.exe","hosted":false,"trigger":"auto"}
        assert_eq!(
            AppMatch::for_process("Ledgerline.exe", TriggerMode::Auto).to_field_value(),
            r#"{"process":"Ledgerline.exe","trigger":"auto"}"#
        );
        // Positive control: a hosted one does write it, so the assertion above
        // is not satisfied by a field that is never serialized at all.
        assert!(captured().to_field_value().contains(r#""hosted":true"#));
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
        assert!(!parsed.hosted, "an old value records no hosted flag");
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

    /// **Review 31's Important 3: every way measured to walk out of the
    /// directory anyway.** The `..` check split on `\` alone, and Windows does
    /// not: `/` is a separator too, and Win32 strips trailing dots and spaces
    /// from every path component before resolving it. Each of these was
    /// measured as ACCEPTED by the previous implementation.
    #[test]
    fn a_path_that_walks_out_by_any_separator_windows_accepts_is_not_launchable() {
        for path in [
            // `/` is a separator, so these are all `..` components.
            r"C:\Program Files/../../Windows/System32\Speedtest.exe",
            r"C:\Temp/..\Speedtest.exe",
            r"C:\Temp\../Speedtest.exe",
            // Win32 strips the trailing space, and then it is `..`.
            r"C:\Temp\.. \Speedtest.exe",
            r"C:\Temp\...\Speedtest.exe",
            // An alternate data stream on a DIRECTORY component: the colon
            // has no business anywhere but the drive.
            r"C:\Temp\evil.exe:stream\Speedtest.exe",
            // A NUL truncates the path for the Win32 call that opens it, so
            // what is checked and what would run are different strings.
            "C:\\Temp\\Speedtest.exe\0\\Speedtest.exe",
            "C:\\Program Files\\Speedtest\\Speedtest.exe\0.txt",
        ] {
            let m = AppMatch { path: path.to_string(), ..captured() };
            assert_eq!(m.launchable_path(), None, "path: {path:?}");
        }
    }

    /// The set the previous implementation already refused, kept refused by the
    /// new one -- the regression half of review 31's Important 3. Tightening a
    /// check is only a fix if nothing it used to catch slips through.
    #[test]
    fn everything_that_was_already_refused_stays_refused() {
        for path in [
            // Device and UNC namespaces.
            r"\\?\C:\Program Files\Speedtest\Speedtest.exe",
            r"\\.\C:\Program Files\Speedtest\Speedtest.exe",
            r"\\host\share\Speedtest.exe",
            // Relative, drive-relative, and wholly forward-slashed.
            r"Speedtest.exe",
            r".\Speedtest.exe",
            r"\Speedtest.exe",
            r"C:Speedtest.exe",
            r"C:/Program Files/Speedtest/Speedtest.exe",
            // A file name Win32 would resolve to `Speedtest.exe` but which is
            // not the string `Speedtest.exe`.
            r"C:\Program Files\Speedtest\Speedtest.exe.",
            "C:\\Program Files\\Speedtest\\Speedtest.exe ",
            // Quote, newline, zero-width space.
            "C:\\a\" --flag\\Speedtest.exe",
            "C:\\Program Files\\Speedtest\\Speedtest.exe\n",
            "C:\\Program Files\\Speedtest\\Speedtest\u{200b}.exe",
        ] {
            let m = AppMatch { path: path.to_string(), ..captured() };
            assert_eq!(m.launchable_path(), None, "path: {path:?}");
        }
    }

    /// The positive control for the two rejection tests above: the tightening
    /// must not have made `launchable_path` answer `None` for everything, and
    /// ordinary Windows paths -- spaces, dots inside a name, a deep tree --
    /// must survive it.
    #[test]
    fn ordinary_windows_paths_are_still_launchable() {
        for (process, path) in [
            ("Speedtest.exe", r"C:\Program Files\WindowsApps\Speedtest\Speedtest.exe"),
            ("Speedtest.exe", r"D:\Speedtest.exe"),
            ("My.App.exe", r"C:\Program Files (x86)\Vendor Ltd.\v1.2\My.App.exe"),
        ] {
            let m = AppMatch {
                process: process.to_string(),
                path: path.to_string(),
                ..captured()
            };
            assert_eq!(m.launchable_path(), Some(path), "path: {path:?}");
        }
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
        assert!(!m.hosted, "a process-only match came off no window at all");
    }
}

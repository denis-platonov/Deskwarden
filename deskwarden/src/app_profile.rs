//! **A second, fully separate Deskwarden in the same Windows session**, for
//! testing a build without touching the copy the owner runs every day.
//!
//! # The opt-in
//!
//! `DESKWARDEN_PROFILE=<name>` in the environment, where the name matches
//! `^[a-z0-9-]{1,32}$`. Unset or empty means no profile, and then **nothing in
//! this module changes anything**: every name and every folder is exactly the
//! one a build without this module used. A value that is set and is not a
//! valid name is refused at startup, with a message box, before the process
//! creates a single kernel object or touches a single file. It is never read
//! as "no profile": a typo there would otherwise attach a test copy to the
//! owner's vault, which is the one outcome this feature exists to rule out.
//!
//! # What a profile changes
//!
//! * **Every per-session kernel object name** -- the app mutex, the stand-down
//!   event, the window doorbells, the vault slots -- gets `-<name>` on the end
//!   ([`kernel_name`]). A profiled copy and the owner's copy therefore never
//!   open, signal or wait on each other's objects, so neither can stand the
//!   other down.
//! * **Every folder on disk** moves under `<normal folder>\profiles\<name>`
//!   ([`profiled_root`]). [`app_dirs`] is the one place this crate resolves
//!   its folders from the OS, so settings, logs, accounts, the vault caches,
//!   the histories, the result files and the update and icon caches all
//!   follow it.
//! * **`bw serve`'s port** moves off 8087 ([`bw_serve_port_for`]), so a
//!   profiled window can never read the owner's unlocked `bw serve`.
//! * **Nothing outside those folders is written.** No logon entry, no user
//!   `PATH`, no installer run ([`touches_the_installed_copy`]).
//! * **The windows and the tray say so** ([`window_title`]).
//!
//! # Telling processes apart for the forced takeover
//!
//! `single_instance` can end another Deskwarden by force when it will not
//! stand down, and it finds that process by executable name -- which a test
//! copy may well share with the owner's. So every profiled process holds two
//! marker names for its whole life ([`hold_markers`]): one that says "this pid
//! runs under SOME profile" and one that says which. A takeover only ends a
//! process whose markers show it is in the same profile as the one ending it
//! ([`may_force_end`]); a profiled process that cannot show that ends nothing.

use std::ffi::OsStr;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// The environment variable that opts a process into a profile.
pub const PROFILE_ENV: &str = "DESKWARDEN_PROFILE";

/// The longest name a profile may have.
pub const MAX_NAME_LEN: usize = 32;

/// The folder, under each of this app's normal folders, that profiles live in.
pub const PROFILES_FOLDER: &str = "profiles";

/// What a profiled window title and tray tooltip end with, before the name.
/// `\u{2014}` is an em dash; written as an escape so the source stays ASCII.
pub const TITLE_MARK: &str = " \u{2014} profile ";

/// The `directories` triple this app's folders are resolved from.
const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "Deskwarden";
const APPLICATION: &str = "Deskwarden";

/// The triple `bw_acquire` has always kept its download scratch under. A
/// different triple from [`APPLICATION`]'s on purpose -- it is what shipped,
/// and moving it would leave the old scratch folder behind for nothing.
const BW_CLI_APPLICATION: &str = "deskwarden";

/// The prefix of the two marker names a profiled process holds. Deliberately
/// NOT `app_mutex::APP_MUTEX_NAME` with something after it: a profile name is
/// appended to that name, so a marker spelled as an extension of it could be
/// spelled by a profile too. Same GUID, so it is still unmistakably ours.
const MARKER_PREFIX: &str = "Local\\Deskwarden-Profiled-63CBCB72-5383-4AE7-AFB7-5EE0530E4630";

/// A profile name that passed [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile(String);

impl Profile {
    /// The name, as given.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

/// Why a `DESKWARDEN_PROFILE` value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileRefusal {
    /// The value is not valid Unicode.
    NotUnicode,
    /// The value does not match `^[a-z0-9-]{1,32}$`.
    Malformed(String),
    /// The value is well formed, but one of its names would spell an object
    /// the default copy owns -- see [`parse`].
    Reserved(String),
}

impl ProfileRefusal {
    /// The sentence the startup message box shows.
    #[must_use]
    pub fn message(&self) -> String {
        let shape = format!(
            "A profile name is 1 to {MAX_NAME_LEN} characters of lowercase letters, digits and \
             hyphens. Fix or clear {PROFILE_ENV} and start Deskwarden again. Nothing was started \
             and nothing was changed."
        );
        match self {
            Self::NotUnicode => {
                format!("{PROFILE_ENV} is set, but its value is not readable text. {shape}")
            }
            Self::Malformed(value) => {
                format!("{PROFILE_ENV} is set to {value:?}, which is not a valid profile name. {shape}")
            }
            Self::Reserved(value) => format!(
                "{PROFILE_ENV} is set to {value:?}. Names that are \"quit\" or start with \
                 \"quit-\" are reserved, because the app's lock under that profile would share a \
                 name with another copy's stand-down signal. {shape}"
            ),
        }
    }
}

/// **The whole decision about a `DESKWARDEN_PROFILE` value.** Pure.
///
/// `None` and the empty string are "no profile". Anything else must match
/// `^[a-z0-9-]{1,32}$`, and must not be `quit` or start with `quit-`: the
/// stand-down event is the app mutex's name plus `-quit`, so under those names
/// a profile's mutex would be spelled exactly like another copy's stand-down
/// event (the default's for `quit`, profile `x`'s for `quit-x`).
pub fn parse(raw: Option<&OsStr>) -> Result<Option<Profile>, ProfileRefusal> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let value = raw.to_str().ok_or(ProfileRefusal::NotUnicode)?;
    let well_formed = (1..=MAX_NAME_LEN).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !well_formed {
        return Err(ProfileRefusal::Malformed(value.to_string()));
    }
    if value == "quit" || value.starts_with("quit-") {
        return Err(ProfileRefusal::Reserved(value.to_string()));
    }
    Ok(Some(Profile(value.to_string())))
}

/// This process's profile, once it has been read.
static ACTIVE: OnceLock<Option<Profile>> = OnceLock::new();

/// **Reads `DESKWARDEN_PROFILE` once, for the life of the process.** The
/// first statement of `main`.
///
/// On a refusal nothing is recorded, and `main` exits before anything else
/// runs. A second call returns what the first one recorded.
pub fn init_from_env() -> Result<Option<&'static Profile>, ProfileRefusal> {
    let parsed = parse(std::env::var_os(PROFILE_ENV).as_deref())?;
    Ok(ACTIVE.get_or_init(|| parsed).as_ref())
}

/// This process's profile, or `None` for the default copy.
///
/// Only what [`init_from_env`] recorded. `main` calls that before anything
/// else in every kind of launch -- daemon, window and service -- so nothing in
/// the app can ask before it is answered. A process that never called it (a
/// test binary, an example) is the default copy, whatever its environment
/// says: the suite's pins on the default's names and folders hold even for a
/// developer who has the variable set.
pub fn active() -> Option<&'static Profile> {
    ACTIVE.get().and_then(Option::as_ref)
}

// ---------------------------------------------------------------------------
// Kernel object names
// ---------------------------------------------------------------------------

/// `base` as `profile` spells it: unchanged for the default, `base-<name>`
/// under a profile. Pure.
#[must_use]
pub fn suffixed(base: &str, profile: Option<&Profile>) -> String {
    match profile {
        None => base.to_string(),
        Some(profile) => format!("{base}-{}", profile.0),
    }
}

/// **Every per-session kernel object name this app creates or opens goes
/// through here.** `base` for the default copy, `base-<name>` under a profile.
#[must_use]
pub fn kernel_name(base: &str) -> String {
    suffixed(base, active())
}

/// The name every profiled process holds, whatever its profile: "pid `pid`
/// runs under some profile".
#[must_use]
pub fn any_profile_marker(pid: u32) -> String {
    format!("{MARKER_PREFIX}-{pid}")
}

/// The name a process under `profile` holds: "pid `pid` runs under this one".
#[must_use]
pub fn profile_marker(pid: u32, profile: &Profile) -> String {
    format!("{MARKER_PREFIX}-{pid}-{}", profile.0)
}

/// The marker handles, held until the process ends.
static MARKERS: Mutex<Vec<OwnedHandle>> = Mutex::new(Vec::new());

/// **Takes this process's two marker names**, if it runs under a profile.
/// The default copy holds none, exactly as before this module existed.
///
/// Called by `main` straight after [`init_from_env`], above the mutex and above
/// the `--ui` branch, so that every profiled process -- daemon, window or
/// service -- is marked before any other Deskwarden could look for it.
pub fn hold_markers() {
    let Some(profile) = active() else {
        return;
    };
    let pid = std::process::id();
    let mut held = MARKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for name in [any_profile_marker(pid), profile_marker(pid, profile)] {
        if let Some(handle) = crate::app_mutex::hold_claim(&name) {
            held.push(handle);
        }
    }
}

/// What asking whether a name exists found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// The name exists.
    Present,
    /// The kernel said there is no such name.
    Absent,
    /// The ask failed for some other reason. Never read as either answer.
    Unknown,
}

/// Another process's profile, as far as its marker names show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateProfile {
    /// It holds no profile marker: the default copy (or a build older than
    /// profiles, which is the default copy too).
    Default,
    /// It holds the marker of the profile this process runs under.
    Same,
    /// It holds a profile marker, and not this process's.
    Other,
    /// Its markers could not be read.
    Unknown,
}

/// **Which profile a candidate runs under, from what its markers showed.**
/// Pure.
///
/// `any` is the probe of [`any_profile_marker`]; `same` is the probe of
/// [`profile_marker`] for THIS process's profile, and `None` when this process
/// has no profile and so has no such name to ask about.
#[must_use]
pub fn classify(any: Probe, same: Option<Probe>) -> CandidateProfile {
    match (same, any) {
        (Some(Probe::Present), _) => CandidateProfile::Same,
        (Some(Probe::Unknown), _) | (_, Probe::Unknown) => CandidateProfile::Unknown,
        (_, Probe::Present) => CandidateProfile::Other,
        (_, Probe::Absent) => CandidateProfile::Default,
    }
}

/// **May this process end that one by force?** Pure, and the whole rule.
///
/// Only within one profile. The default copy may end a default copy -- what it
/// always did -- and never a profiled one. A profiled copy may end only a copy
/// that shows it is in the same profile, and ends nothing it cannot read.
#[must_use]
pub fn may_force_end(mine: Option<&Profile>, candidate: CandidateProfile) -> bool {
    match mine {
        None => candidate == CandidateProfile::Default,
        Some(_) => candidate == CandidateProfile::Same,
    }
}

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

/// `root` for the default copy, `root\profiles\<name>` under a profile. Pure,
/// and **the one place a profile is applied to a path.**
#[must_use]
pub fn profiled_root(root: PathBuf, profile: Option<&Profile>) -> PathBuf {
    match profile {
        None => root,
        Some(profile) => root.join(PROFILES_FOLDER).join(&profile.0),
    }
}

/// This app's two folders, with the profile applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Settings, the log, accounts, the histories and the vault cache.
    pub config_dir: PathBuf,
    /// The update downloads and the favicon cache.
    pub cache_dir: PathBuf,
}

impl AppDirs {
    /// The two folders under `profile`, from the OS's answers. Pure.
    #[must_use]
    pub fn under(config_dir: PathBuf, cache_dir: PathBuf, profile: Option<&Profile>) -> Self {
        Self {
            config_dir: profiled_root(config_dir, profile),
            cache_dir: profiled_root(cache_dir, profile),
        }
    }
}

/// **This app's folders, and the only place in the crate they are asked of
/// the OS.** `None` where the platform has none.
#[must_use]
pub fn app_dirs() -> Option<AppDirs> {
    let dirs = directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)?;
    Some(AppDirs::under(
        dirs.config_dir().to_path_buf(),
        dirs.cache_dir().to_path_buf(),
        active(),
    ))
}

/// The cache folder `bw_acquire` downloads the Bitwarden CLI into, with the
/// profile applied.
#[must_use]
pub fn bw_cli_cache_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", BW_CLI_APPLICATION)?;
    Some(profiled_root(dirs.cache_dir().to_path_buf(), active()))
}

// ---------------------------------------------------------------------------
// Everything else a profile changes
// ---------------------------------------------------------------------------

/// The port `bw serve` listens on: `default` without a profile, and a port of
/// the profile's own, from 18087 to 19086, under one. Pure.
///
/// Derived from the name, not chosen at random, because the daemon and every
/// window it starts have to agree on it without being told. Two profiles may
/// land on one port; the second then finds it held and refuses to start, which
/// is what a port held by anything else already does.
#[must_use]
pub fn bw_serve_port_for(default: u16, profile: Option<&Profile>) -> u16 {
    let Some(profile) = profile else {
        return default;
    };
    // FNV-1a, 32-bit: stable across builds and platforms, unlike `std`'s
    // hasher, which is allowed to change.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in profile.0.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // `% 1000` is below 1000, so the sum is at most 19086 and fits.
    18_087 + u16::try_from(hash % 1000).unwrap_or(0)
}

/// `false` under a profile: a test copy never writes the logon entry, never
/// changes the user's `PATH` and never runs the installer, all of which act on
/// the copy that is installed rather than on this one.
#[must_use]
pub fn touches_the_installed_copy() -> bool {
    active().is_none()
}

/// The sentence a refused install-side action gives under a profile.
#[must_use]
pub fn installed_copy_refusal(what: &str) -> String {
    let name = active().map_or("", Profile::name);
    format!(
        "this Deskwarden runs under the test profile {name:?}, and a test copy never {what}: \
         that would change the installed copy, not this one"
    )
}

/// `base` as a profiled window title or tooltip: unchanged for the default,
/// `base \u{2014} profile <name>` under a profile. Pure.
#[must_use]
pub fn marked_title(base: &str, profile: Option<&Profile>) -> String {
    match profile {
        None => base.to_string(),
        Some(profile) => format!("{base}{TITLE_MARK}{}", profile.0),
    }
}

/// [`marked_title`] for this process.
#[must_use]
pub fn window_title(base: &str) -> String {
    marked_title(base, active())
}

/// A caption read back off a window, with this process's mark taken off, so
/// that every lookup by title keeps comparing against the plain title it was
/// written with. Pure.
#[must_use]
pub fn unmarked_title(read: String, profile: Option<&Profile>) -> String {
    let Some(profile) = profile else {
        return read;
    };
    let mark = format!("{TITLE_MARK}{}", profile.0);
    match read.strip_suffix(&mark) {
        Some(base) => base.to_string(),
        None => read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn profile(name: &str) -> Profile {
        parse(Some(OsStr::new(name))).expect("valid").expect("a profile")
    }

    #[test]
    fn unset_and_empty_mean_the_default_copy() {
        assert_eq!(parse(None), Ok(None));
        assert_eq!(parse(Some(OsStr::new(""))), Ok(None));
    }

    #[test]
    fn a_well_formed_name_is_a_profile() {
        for name in ["a", "test", "release-check", "0", "x-1-y", &"a".repeat(MAX_NAME_LEN)] {
            assert_eq!(parse(Some(OsStr::new(name))).unwrap().unwrap().name(), name);
        }
    }

    #[test]
    fn an_invalid_name_is_refused_and_never_read_as_the_default() {
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        for bad in ["Test", "a b", "a_b", "a.b", "../x", "x\\y", "\u{e9}", " ", too_long.as_str()] {
            assert_eq!(
                parse(Some(OsStr::new(bad))),
                Err(ProfileRefusal::Malformed(bad.to_string())),
                "{bad:?} was not refused as malformed"
            );
        }
    }

    #[test]
    fn a_value_that_is_not_unicode_is_refused() {
        use std::os::windows::ffi::OsStringExt;
        let lone_surrogate = OsString::from_wide(&[0xD800]);
        assert_eq!(parse(Some(&lone_surrogate)), Err(ProfileRefusal::NotUnicode));
    }

    #[test]
    fn names_that_would_spell_a_stand_down_event_are_reserved() {
        for reserved in ["quit", "quit-", "quit-x"] {
            assert_eq!(
                parse(Some(OsStr::new(reserved))),
                Err(ProfileRefusal::Reserved(reserved.to_string()))
            );
        }
        assert!(parse(Some(OsStr::new("quitter"))).is_ok(), "only `quit` and `quit-*` are reserved");
        // The reason, measured rather than asserted: under `quit` the mutex
        // would be spelled exactly like the default's stand-down event.
        let mutex = crate::app_mutex::APP_MUTEX_NAME;
        assert_eq!(format!("{mutex}-quit"), crate::single_instance::QUIT_EVENT_NAME);
    }

    #[test]
    fn every_refusal_names_the_variable_and_the_way_out() {
        for refusal in [
            ProfileRefusal::NotUnicode,
            ProfileRefusal::Malformed("A".into()),
            ProfileRefusal::Reserved("quit".into()),
        ] {
            let message = refusal.message();
            assert!(message.contains(PROFILE_ENV), "{message}");
            assert!(message.contains("Nothing was started"), "{message}");
        }
    }

    #[test]
    fn the_unit_tests_run_as_the_default_copy() {
        assert_eq!(active(), None);
        assert_eq!(kernel_name("Local\\X"), "Local\\X");
        assert!(touches_the_installed_copy());
    }

    #[test]
    fn kernel_names_are_unchanged_by_default_and_suffixed_under_a_profile() {
        let p = profile("rc");
        let mutex = crate::app_mutex::APP_MUTEX_NAME;
        let quit = crate::single_instance::QUIT_EVENT_NAME;
        assert_eq!(suffixed(mutex, None), "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630");
        assert_eq!(
            suffixed(quit, None),
            "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630-quit"
        );
        assert_eq!(
            suffixed(mutex, Some(&p)),
            "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630-rc"
        );
        assert_eq!(
            suffixed(quit, Some(&p)),
            "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630-quit-rc"
        );
        assert_eq!(
            suffixed(r"Local\Deskwarden-UI-Show-1234", Some(&p)),
            r"Local\Deskwarden-UI-Show-1234-rc"
        );
        assert_eq!(suffixed(r"Local\Deskwarden-UI-Show-1234", None), r"Local\Deskwarden-UI-Show-1234");
    }

    #[test]
    fn markers_cannot_be_spelled_by_any_profiled_or_default_name() {
        let p = profile("rc");
        let any = any_profile_marker(42);
        let named = profile_marker(42, &p);
        assert_ne!(any, named);
        assert!(named.starts_with(&any));
        assert_ne!(profile_marker(42, &p), profile_marker(43, &p));
        assert_ne!(profile_marker(4, &profile("2-rc")), profile_marker(42, &p));
        // A profile name is appended to the app's own names, never to this
        // prefix, so no profile can spell a marker.
        assert!(!MARKER_PREFIX.starts_with(crate::app_mutex::APP_MUTEX_NAME));
        assert!(MARKER_PREFIX.starts_with("Local\\"));
    }

    #[test]
    fn a_candidate_is_classified_from_its_markers() {
        use CandidateProfile as C;
        use Probe::{Absent, Present, Unknown};
        // The default copy asks only whether any profile marker is held.
        assert_eq!(classify(Absent, None), C::Default);
        assert_eq!(classify(Present, None), C::Other);
        assert_eq!(classify(Unknown, None), C::Unknown);
        // A profiled copy asks for its own profile first.
        assert_eq!(classify(Present, Some(Present)), C::Same);
        assert_eq!(classify(Present, Some(Absent)), C::Other);
        assert_eq!(classify(Absent, Some(Absent)), C::Default);
        assert_eq!(classify(Unknown, Some(Absent)), C::Unknown);
        assert_eq!(classify(Absent, Some(Unknown)), C::Unknown);
    }

    #[test]
    fn a_profiled_copy_never_ends_the_default_copy_or_anything_it_cannot_read() {
        let p = profile("rc");
        assert!(may_force_end(Some(&p), CandidateProfile::Same));
        assert!(!may_force_end(Some(&p), CandidateProfile::Default));
        assert!(!may_force_end(Some(&p), CandidateProfile::Other));
        assert!(!may_force_end(Some(&p), CandidateProfile::Unknown));
    }

    #[test]
    fn the_default_copy_never_ends_a_profiled_copy() {
        assert!(may_force_end(None, CandidateProfile::Default), "what it always did");
        assert!(!may_force_end(None, CandidateProfile::Other));
        assert!(!may_force_end(None, CandidateProfile::Same));
        assert!(!may_force_end(None, CandidateProfile::Unknown));
    }

    #[test]
    fn folders_are_unchanged_by_default_and_moved_under_profiles_with_one() {
        let config = PathBuf::from(r"C:\Users\u\AppData\Roaming\Deskwarden\Deskwarden\config");
        let cache = PathBuf::from(r"C:\Users\u\AppData\Local\Deskwarden\Deskwarden\cache");

        let default = AppDirs::under(config.clone(), cache.clone(), None);
        assert_eq!(default.config_dir, config);
        assert_eq!(default.cache_dir, cache);

        let p = profile("rc");
        let profiled = AppDirs::under(config.clone(), cache.clone(), Some(&p));
        assert_eq!(profiled.config_dir, config.join("profiles").join("rc"));
        assert_eq!(profiled.cache_dir, cache.join("profiles").join("rc"));
        assert_eq!(
            profiled_root(PathBuf::from(r"C:\x\cache"), Some(&p)),
            PathBuf::from(r"C:\x\cache\profiles\rc")
        );
    }

    /// The OS's answer, with no profile, is exactly `directories`' own -- the
    /// folders the default copy has always used.
    #[test]
    fn the_default_folders_are_the_ones_directories_resolves() {
        let dirs = directories::ProjectDirs::from("dev", "Deskwarden", "Deskwarden").unwrap();
        let ours = app_dirs().unwrap();
        assert_eq!(ours.config_dir, dirs.config_dir());
        assert_eq!(ours.cache_dir, dirs.cache_dir());
        let bw = directories::ProjectDirs::from("", "", "deskwarden").unwrap();
        assert_eq!(bw_cli_cache_dir().unwrap(), bw.cache_dir());
    }

    #[test]
    fn the_bw_serve_port_is_the_default_without_a_profile_and_its_own_with_one() {
        assert_eq!(bw_serve_port_for(8087, None), 8087);
        for name in ["rc", "release-check", "a", "zzzzzzzz"] {
            let port = bw_serve_port_for(8087, Some(&profile(name)));
            assert!((18_087..=19_086).contains(&port), "{name} -> {port}");
            assert_eq!(port, bw_serve_port_for(8087, Some(&profile(name))), "not stable");
        }
    }

    #[test]
    fn titles_are_marked_only_under_a_profile_and_the_mark_comes_back_off() {
        let p = profile("rc");
        assert_eq!(marked_title("Deskwarden", None), "Deskwarden");
        assert_eq!(marked_title("Deskwarden", Some(&p)), "Deskwarden \u{2014} profile rc");
        assert_eq!(unmarked_title("Deskwarden \u{2014} profile rc".into(), Some(&p)), "Deskwarden");
        assert_eq!(unmarked_title("Deskwarden".into(), Some(&p)), "Deskwarden");
        assert_eq!(
            unmarked_title("Deskwarden \u{2014} profile rc".into(), None),
            "Deskwarden \u{2014} profile rc",
            "the default copy reads captions exactly as before"
        );
        assert_eq!(
            unmarked_title("Deskwarden \u{2014} profile other".into(), Some(&p)),
            "Deskwarden \u{2014} profile other",
            "only this profile's own mark is taken off"
        );
    }

    /// **No folder is resolved from the OS anywhere but here.** A second
    /// `ProjectDirs::from` elsewhere would be a folder the profile is not
    /// applied to -- a test copy writing into the owner's settings.
    ///
    /// Over every file's production code: comment lines and gated test modules
    /// are cut, the way `settings.rs`' guard reads `main.rs`.
    #[test]
    fn no_folder_is_resolved_outside_this_module() {
        let needles = [
            concat!("ProjectDirs::", "from("),
            concat!("BaseDirs::", "new("),
            concat!("UserDirs::", "new("),
            concat!("\"APP", "DATA\""),
            concat!("\"LOCALAPP", "DATA\""),
            concat!("FOLDERID_", "RoamingAppData"),
            concat!("FOLDERID_", "LocalAppData"),
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src.clone()];
        let mut files = 0usize;
        let mut here = 0usize;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                files += 1;
                let code = production_code(&std::fs::read_to_string(&path).unwrap());
                let found: usize = needles.iter().map(|n| code.matches(n).count()).sum();
                if path.file_name().and_then(|n| n.to_str()) == Some("app_profile.rs") {
                    here = found;
                    continue;
                }
                assert_eq!(
                    found,
                    0,
                    "{} resolves a folder from the OS itself. Every folder comes from \
                     `app_profile::app_dirs` (or `bw_cli_cache_dir`), which is where a test \
                     profile is applied",
                    path.display()
                );
            }
        }
        assert!(files > 100, "control: the walk found only {files} source files");
        assert_eq!(here, 2, "control: this module no longer resolves its two folders itself");
    }

    /// **Every per-session kernel object name goes through [`kernel_name`].**
    ///
    /// A `Local\` name built without it would be shared by a test copy and the
    /// owner's copy -- one of them could hold, signal or wait on the other's.
    /// Read over every file's production code, a line at a time: a line that
    /// spells a `Local\` literal must also call `kernel_name(`, except the
    /// three constants it is applied to (the app mutex, the stand-down event)
    /// or deliberately not (this module's markers, which carry the name
    /// themselves). And the two constants are used, outside their own
    /// declarations, only where `kernel_name` wraps them.
    #[test]
    fn every_named_kernel_object_goes_through_kernel_name() {
        let literals = [concat!("\"Local", "\\\\"), concat!("r\"Local", "\\")];
        let declarations = [
            concat!("pub const APP_MUTEX", "_NAME: &str = "),
            concat!("pub const QUIT_EVENT", "_NAME: &str = "),
            concat!("const MARKER", "_PREFIX: &str = "),
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src.clone()];
        let mut names = 0usize;
        let mut mutex_uses: Vec<String> = Vec::new();
        let mut quit_uses: Vec<String> = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let file = path.file_name().unwrap().to_string_lossy().to_string();
                let code = production_code(&std::fs::read_to_string(&path).unwrap());
                for line in code.lines() {
                    if line.contains(concat!("APP_MUTEX", "_NAME")) {
                        mutex_uses.push(file.clone());
                    }
                    if line.contains(concat!("QUIT_EVENT", "_NAME")) {
                        quit_uses.push(file.clone());
                    }
                    if !literals.iter().any(|l| line.contains(l)) {
                        continue;
                    }
                    names += 1;
                    if declarations.iter().any(|d| line.contains(d)) {
                        continue;
                    }
                    assert!(
                        line.contains(concat!("kernel_", "name(")),
                        "{file} builds a per-session name without `app_profile::kernel_name`, so a \
                         test copy would share it with the owner's copy:\n{line}"
                    );
                }
            }
        }
        assert!(names >= 10, "control: only {names} `Local\\` names were found in production");
        mutex_uses.sort();
        assert_eq!(
            mutex_uses,
            ["app_mutex.rs", "app_mutex.rs", "updater.rs"],
            "the app mutex's name is used somewhere new; route it through `app_mutex_name()`"
        );
        assert_eq!(
            quit_uses,
            ["single_instance.rs", "single_instance.rs"],
            "the stand-down event's name is used somewhere new; route it through \
             `quit_event_name()`"
        );
        let updater = production_code(include_str!("updater.rs"));
        let claim = &updater[updater.find("fn installer_claim_name(").expect("control")..];
        assert!(
            claim[..claim.find("\n}").unwrap()].contains(concat!("app_profile::kernel_", "name(")),
            "the installer claim extends the app mutex's name without the test profile"
        );
    }

    /// **`main` reads the profile before anything else, refuses a bad one,
    /// and marks itself before any other Deskwarden could look.** Before the
    /// launch kind is read (a window and the service inherit the profile),
    /// before the app mutex, and before the first folder.
    #[test]
    fn main_reads_the_profile_first_and_refuses_a_bad_one() {
        let main_rs = production_code(include_str!("main.rs"));
        let body = &main_rs[main_rs.find("fn main() {").expect("control: no `fn main`")..];
        let read = body
            .find(concat!("if let Err(refusal) = deskwarden::app_profile::init_", "from_env() {"))
            .expect("`main` no longer reads DESKWARDEN_PROFILE, or no longer refuses a bad one");
        let refused = body
            .find(concat!("std::process::exit(refuse_the_", "profile(&refusal));"))
            .expect("a bad DESKWARDEN_PROFILE no longer ends the process");
        let marked = body
            .find(concat!("app_profile::hold_", "markers();"))
            .expect("`main` no longer marks a profiled process");
        for later in [
            concat!("launch_intent(std::env::", "args()"),
            concat!("app_mutex::", "acquire()"),
            concat!("app_profile::", "app_dirs()"),
            concat!("logging::", "init("),
        ] {
            let at = body.find(later).unwrap_or_else(|| panic!("control: no {later:?} in main"));
            assert!(read < refused && refused < marked && marked < at, "{later:?} comes first");
        }
        // The refusal writes nothing: no log file, whose folder is the very
        // thing the refused value would have chosen.
        let refusal = &main_rs[main_rs.find("fn refuse_the_profile(").expect("control")..];
        let refusal = &refusal[..refusal.find("\n}").unwrap()];
        assert!(refusal.contains("message_box("), "a bad profile is refused without saying so");
        assert!(!refusal.contains("logging::"), "the refusal opens a log file");
    }

    /// A source file with comment lines and `cfg(test)` modules removed.
    fn production_code(source: &str) -> String {
        let mut kept = Vec::new();
        let mut gated = false;
        let mut skipping = false;
        for line in source.lines() {
            if skipping {
                if line == "}" {
                    skipping = false;
                }
                continue;
            }
            if gated && crate::below_cut::is_module_opener(line) {
                kept.pop();
                skipping = true;
                gated = false;
                continue;
            }
            gated = line.trim() == concat!("#[cfg(", "test)]");
            if line.trim_start().starts_with("//") {
                kept.push("");
                continue;
            }
            kept.push(line);
        }
        kept.join("\n")
    }

    #[test]
    fn the_production_cut_removes_a_test_module_and_keeps_what_follows() {
        let fixture = concat!(
            "fn a() { ProjectDirs::", "from(1); }\n",
            "#[cfg(", "test)]\n",
            "mod tests {\n",
            "    fn b() { ProjectDirs::", "from(2); }\n",
            "}\n",
            "// ProjectDirs::", "from(3)\n",
            "fn c() { SURVIVOR }\n"
        );
        let code = production_code(fixture);
        assert_eq!(code.matches(concat!("ProjectDirs::", "from(")).count(), 1);
        assert!(code.contains("SURVIVOR"));
    }
}

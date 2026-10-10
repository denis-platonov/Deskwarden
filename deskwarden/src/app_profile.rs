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
//!
//! # Footguns
//!
//! * **A build older than `06dd6e8` ignores `DESKWARDEN_PROFILE`** and runs as
//!   the owner's copy -- same lock, same folders, same vault -- and stands the
//!   owner's copy down. Only use the variable with builds from `06dd6e8` on.
//! * **Set it per shell, never for the user.** `setx DESKWARDEN_PROFILE ...`
//!   (or the System Properties dialog) writes it into the user's environment,
//!   and the owner's own copy, started at sign-in, would then come up as the
//!   test profile -- with empty settings and no vault. Use `set` in `cmd` or
//!   `$env:DESKWARDEN_PROFILE = '...'` in PowerShell, in the shell that starts
//!   the test copy.
//! * **Give the test copy its own executable name** (for example
//!   `deskwarden-test.exe`). Builds older than the profile markers end every
//!   same-named process in a forced takeover; a different name is invisible
//!   to them.
//! * **No global shortcuts, Windows Hello or updates.** A profile registers
//!   only the chords written into its own `settings.json`, never touches the
//!   shared Hello credential, and never checks for, downloads or installs an
//!   update.

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
    /// The value is a Windows device name (`con`, `nul`, `com1`...), which no
    /// folder can have.
    DeviceName(String),
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
            Self::DeviceName(value) => format!(
                "{PROFILE_ENV} is set to {value:?}, which Windows keeps for a device, so no folder \
                 can be named after it. {shape}"
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
    if is_device_name(value) {
        return Err(ProfileRefusal::DeviceName(value.to_string()));
    }
    Ok(Some(Profile(value.to_string())))
}

/// Whether `name` is one Windows keeps for a device -- `con`, `prn`, `aux`,
/// `nul`, `com0`-`com9`, `lpt0`-`lpt9` -- and so can never be a folder: the
/// profile's folders are named after it. Pure; `name` is already lowercase.
fn is_device_name(name: &str) -> bool {
    if matches!(name, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let numbered = name.strip_prefix("com").or_else(|| name.strip_prefix("lpt"));
    matches!(numbered, Some(digit) if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit())
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
///
/// **`Err` when a profiled process could not take both**, and `main` then
/// refuses to start it. Unmarked, it would read to the owner's copy as a
/// default copy -- one the owner's forced takeover may end, and one whose own
/// takeover the owner's copy could be mistaken for. Always `Ok` for the
/// default copy.
pub fn hold_markers() -> Result<(), String> {
    let Some(profile) = active() else {
        return Ok(());
    };
    let handles = hold_markers_with(profile, std::process::id(), crate::app_mutex::hold_claim)?;
    MARKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(handles);
    Ok(())
}

/// [`hold_markers`] over `hold`, for `pid` under `profile`: both handles, or
/// the sentence that refuses the launch.
pub fn hold_markers_with<H>(
    profile: &Profile,
    pid: u32,
    hold: impl Fn(&str) -> Option<H>,
) -> Result<Vec<H>, String> {
    [any_profile_marker(pid), profile_marker(pid, profile)]
        .iter()
        .map(|name| {
            hold(name).ok_or_else(|| {
                format!(
                    "Deskwarden could not mark this process as running under the test profile \
                     {:?} (it could not create {name:?}). Without that mark the installed copy \
                     could mistake it for itself, so it was not started. Nothing was changed.",
                    profile.0
                )
            })
        })
        .collect()
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

/// Where the Bitwarden CLI keeps its data when no account directory is
/// chosen: `None` for the default copy -- the CLI's own default,
/// `%APPDATA%\Bitwarden CLI`, as always -- and `<profile config>\bw-cli`
/// under a profile, so a test copy never signs the user's real CLI profile in
/// or out. Pure.
#[must_use]
pub fn bw_cli_fallback_for(config_dir: Option<PathBuf>, profile: Option<&Profile>) -> Option<PathBuf> {
    profile?;
    config_dir.map(|dir| dir.join("bw-cli"))
}

/// [`bw_cli_fallback_for`] for this process, created if it is used.
#[must_use]
pub fn bw_cli_fallback_dir() -> Option<PathBuf> {
    let dir = bw_cli_fallback_for(app_dirs().map(|dirs| dirs.config_dir), active())?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
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

/// The prefix of the names that say which test profile owns a `bw serve`
/// port. Its own prefix, like [`MARKER_PREFIX`], so no profile can spell one.
const PORT_CLAIM_PREFIX: &str = "Local\\Deskwarden-BwServePort-63CBCB72-5383-4AE7-AFB7-5EE0530E4630";

/// The name only ONE test profile's daemon may hold for `port`.
#[must_use]
pub fn port_claim(port: u16) -> String {
    format!("{PORT_CLAIM_PREFIX}-{port}")
}

/// The name the daemon of `profile` holds beside [`port_claim`], so its own
/// windows can tell the `bw serve` on that port is their daemon's.
#[must_use]
pub fn port_owner(port: u16, profile: &Profile) -> String {
    format!("{PORT_CLAIM_PREFIX}-{port}-{}", profile.0)
}

/// The port claims, held for the life of the daemon.
static PORT_CLAIMS: Mutex<Vec<crate::vault_service::Held>> = Mutex::new(Vec::new());

/// **Claims this profile's `bw serve` port for this daemon, or refuses.**
///
/// Two names can hash to one port ([`bw_serve_port_for`]). The first daemon
/// to start holds [`port_claim`] exclusively, and a second profile's daemon
/// on the same port is refused here, at startup, with a sentence that says
/// why and what to do -- rather than starting a `bw serve` that could not
/// bind, or worse, letting its window read the first profile's. Called by
/// `main` once the single-instance takeover has settled. Always `Ok` for the
/// default copy, whose port is 8087 and outside every profile's range.
///
/// **Retried for about two seconds before it refuses.** "The takeover has
/// settled" means this profile's previous daemon released its MUTEX -- and an
/// exiting process's handles are closed in no fixed order, so a relaunch can
/// find the mutex free while the old port claim is still held for a moment.
/// Refusing on the first look would refuse a same-profile restart over its
/// own predecessor.
pub fn claim_bw_serve_port() -> Result<(), String> {
    let Some(profile) = active() else {
        return Ok(());
    };
    let port = crate::bw_serve::port();
    let held = claim_port_retrying(
        PORT_CLAIM_ATTEMPTS,
        PORT_CLAIM_RETRY_EVERY,
        std::thread::sleep,
        || claim_port_with(profile, port, crate::vault_service::windows_env().hold),
    )?;
    PORT_CLAIMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(held);
    Ok(())
}

/// How many times [`claim_bw_serve_port`] tries before it refuses.
pub const PORT_CLAIM_ATTEMPTS: u32 = 21;

/// The pause between two tries: 20 pauses of 100 ms is the two-second bound.
pub const PORT_CLAIM_RETRY_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// `attempt` up to `attempts` times, `sleep`ing `every` between two tries, and
/// the first success -- or the last refusal once the bound is spent. Never
/// sleeps after the last try, and never before the first.
pub fn claim_port_retrying<T>(
    attempts: u32,
    every: std::time::Duration,
    sleep: impl Fn(std::time::Duration),
    attempt: impl Fn() -> Result<T, String>,
) -> Result<T, String> {
    let mut last = attempt();
    for _ in 1..attempts {
        if last.is_ok() {
            break;
        }
        sleep(every);
        last = attempt();
    }
    last
}

/// [`claim_bw_serve_port`] over `hold`, which must be exclusive: `None` when
/// the name already exists.
pub fn claim_port_with<H>(
    profile: &Profile,
    port: u16,
    hold: impl Fn(&str) -> Option<H>,
) -> Result<Vec<H>, String> {
    let refused = || {
        format!(
            "The Deskwarden test profile {:?} runs `bw serve` on port {port}, and another test \
             profile's Deskwarden already holds that port -- two profile names can land on the \
             same one. A profile never reads another profile's vault, so this one was not \
             started. Close the other test copy, or pick a different {PROFILE_ENV} name.",
            profile.0
        )
    };
    let exclusive = hold(&port_claim(port)).ok_or_else(refused)?;
    let owner = hold(&port_owner(port, profile)).ok_or_else(refused)?;
    Ok(vec![exclusive, owner])
}

/// **Whether the `bw serve` on this process's port is its own daemon's.**
/// Always `true` for the default copy, which asks only the port, as before.
/// Under a profile, only when this profile's daemon holds [`port_owner`].
#[must_use]
pub fn bw_serve_port_is_ours() -> bool {
    match active() {
        None => true,
        Some(profile) => (crate::vault_service::windows_env().is_held)(&port_owner(
            crate::bw_serve::port(),
            profile,
        )),
    }
}

/// **The refusal a test profile gets when its account list cannot be read**,
/// or `None`. Pure.
///
/// `stored` and `unreadable` are what `settings.json` gave; together they are
/// exactly the case `accounts::resolve_startup` answers with `NoAccountList`
/// (no account, because the list could not be read). The default copy runs on
/// in that state with the Bitwarden CLI on its own default profile -- the
/// user's real `%APPDATA%\Bitwarden CLI` -- so for a test copy it is a refusal
/// instead. Always `None` for the default copy.
#[must_use]
pub fn no_account_list_refusal(
    profile: Option<&Profile>,
    stored: &[crate::accounts::Account],
    unreadable: bool,
    settings_path: &std::path::Path,
) -> Option<String> {
    let profile = profile?;
    (stored.is_empty() && unreadable).then(|| {
        format!(
            "The Deskwarden test profile {:?} could not read its account list, so it was not \
             started: running without one would point the Bitwarden CLI at your real profile \
             in %APPDATA%\\Bitwarden CLI. Nothing was changed. Fix or remove {} and start the \
             test copy again.",
            profile.0,
            settings_path.display()
        )
    })
}

/// What a "`bw serve`'s port is held" refusal adds under a profile: empty for
/// the default copy, whose sentence is unchanged.
#[must_use]
pub fn port_held_hint() -> String {
    match active() {
        None => String::new(),
        Some(profile) => format!(
            " This is the Deskwarden test profile {:?}, whose port is derived from its name; if \
             another program uses that port, pick a different {PROFILE_ENV} name.",
            profile.0
        ),
    }
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
    /// four constants it is applied to (the app mutex, the stand-down event)
    /// or deliberately not (this module's markers and port claims, which carry
    /// the name themselves or arbitrate between profiles). And the two constants are used, outside their own
    /// declarations, only where `kernel_name` wraps them.
    #[test]
    fn every_named_kernel_object_goes_through_kernel_name() {
        let literals = [concat!("\"Local", "\\\\"), concat!("r\"Local", "\\")];
        let declarations = [
            concat!("pub const APP_MUTEX", "_NAME: &str = "),
            concat!("pub const QUIT_EVENT", "_NAME: &str = "),
            concat!("const MARKER", "_PREFIX: &str = "),
            concat!("const PORT_CLAIM", "_PREFIX: &str = "),
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
            .find(concat!("std::process::exit(refuse_the_", "profile(&refusal.message()));"))
            .expect("a bad DESKWARDEN_PROFILE no longer ends the process");
        let marked = body
            .find(concat!("if let Err(why) = deskwarden::app_profile::hold_", "markers() {"))
            .expect("`main` no longer marks a profiled process, or runs one it could not mark");
        let unmarked = body
            .find(concat!("std::process::exit(refuse_the_", "profile(&why));"))
            .expect("a profiled process that could not mark itself is no longer refused");
        for later in [
            concat!("launch_intent(std::env::", "args()"),
            concat!("app_mutex::", "acquire()"),
            concat!("app_profile::", "app_dirs()"),
            concat!("logging::", "init("),
        ] {
            let at = body.find(later).unwrap_or_else(|| panic!("control: no {later:?} in main"));
            assert!(
                read < refused && refused < marked && marked < unmarked && unmarked < at,
                "{later:?} comes first"
            );
        }
        // The port is claimed once the takeover has settled -- so this
        // profile's own predecessor has let go -- and before this process
        // answers anyone or starts anything.
        let settled = body.find(concat!("single_instance::", "resolve(")).expect("control");
        let claimed = body
            .find(concat!("if let Err(why) = deskwarden::app_profile::claim_bw_serve_", "port() {"))
            .expect("`main` no longer claims the profile's bw serve port");
        let listening = body.find(concat!("single_instance::", "listen_for_takeover()")).unwrap();
        assert!(settled < claimed && claimed < listening, "the port claim is out of place");
        let exits = &body[claimed..listening];
        assert!(exits.contains(concat!("std::process::", "exit(PROFILE_REFUSED)")), "{exits}");
        // The refusal writes nothing: no log file, whose folder is the very
        // thing the refused value would have chosen.
        let refusal = &main_rs[main_rs.find("fn refuse_the_profile(").expect("control")..];
        let refusal = &refusal[..refusal.find("\n}").unwrap()];
        assert!(refusal.contains("message_box("), "a bad profile is refused without saying so");
        assert!(!refusal.contains("logging::"), "the refusal opens a log file");
    }

    #[test]
    fn windows_device_names_are_refused() {
        let mut devices = vec!["con".to_string(), "prn".into(), "aux".into(), "nul".into()];
        for n in 0..=9 {
            devices.push(format!("com{n}"));
            devices.push(format!("lpt{n}"));
        }
        for device in &devices {
            assert_eq!(
                parse(Some(OsStr::new(device))),
                Err(ProfileRefusal::DeviceName(device.clone())),
                "{device} was not refused"
            );
        }
        for fine in ["com", "lpt", "com10", "lpt1-x", "console", "conx", "nul-x", "auxiliary"] {
            assert!(parse(Some(OsStr::new(fine))).is_ok(), "{fine} was refused");
        }
        let message = ProfileRefusal::DeviceName("nul".into()).message();
        assert!(message.contains("device") && message.contains(PROFILE_ENV), "{message}");
    }

    /// **A profiled process that cannot mark itself is not started** -- each
    /// marker on its own, and the sentence names the profile.
    #[test]
    fn a_process_that_cannot_hold_both_markers_is_refused() {
        let p = profile("rc");
        let both = hold_markers_with(&p, 42, |name: &str| Some(name.to_string())).unwrap();
        assert_eq!(both, vec![any_profile_marker(42), profile_marker(42, &p)]);
        for missing in [any_profile_marker(42), profile_marker(42, &p)] {
            let refused = hold_markers_with(&p, 42, |name: &str| {
                (name != missing).then(|| name.to_string())
            })
            .expect_err("a process missing a marker was let run");
            assert!(refused.contains("\"rc\"") && refused.contains("not started"), "{refused}");
        }
    }

    #[test]
    fn the_cli_falls_back_to_the_profile_s_own_folder_never_the_user_s() {
        let config = PathBuf::from(r"C:\cfg\profiles\rc");
        assert_eq!(bw_cli_fallback_for(Some(config.clone()), None), None, "the default changed");
        assert_eq!(
            bw_cli_fallback_for(Some(config.clone()), Some(&profile("rc"))),
            Some(config.join("bw-cli"))
        );
        assert_eq!(bw_cli_fallback_for(None, None), None);
        assert_eq!(bw_cli_fallback_dir(), None, "the unit tests run as the default copy");
        // And `main` points the CLI there when it has no account directory.
        assert!(
            production_code(include_str!("main.rs")).contains(concat!(
                "None => (config_dir.join(\"session.bin\"), ",
                "deskwarden::app_profile::bw_cli_fallback_dir()),"
            )),
            "`main`'s no-account arm no longer uses the profile's own CLI folder"
        );
    }

    /// **Two profiles on one `bw serve` port: the second is refused**, over
    /// the real kernel, and gets the port once the first has gone. Port 1 is
    /// outside every profile's range, so no real daemon holds these names.
    #[test]
    fn a_second_profile_on_a_claimed_port_is_refused_until_the_first_goes() {
        let hold = crate::vault_service::windows_env().hold;
        let first = claim_port_with(&profile("claim-test-a"), 1, hold).expect("control: first");
        let Err(refused) = claim_port_with(&profile("claim-test-b"), 1, hold) else {
            panic!("two profiles were let share a bw serve port");
        };
        assert!(refused.contains("port 1") && refused.contains(PROFILE_ENV), "{refused}");
        assert!(
            (crate::vault_service::windows_env().is_held)(&port_owner(1, &profile("claim-test-a"))),
            "the first profile's windows could not tell the port is theirs"
        );
        assert!(!(crate::vault_service::windows_env().is_held)(&port_owner(1, &profile("claim-test-b"))));
        drop(first);
        let second = claim_port_with(&profile("claim-test-b"), 1, hold);
        assert!(second.is_ok(), "the port stayed claimed after its holder let go");
        assert!(bw_serve_port_is_ours(), "the default copy asks only the port, as before");
        assert_eq!(port_held_hint(), "", "the default copy's refusal sentence changed");
        assert!(!PORT_CLAIM_PREFIX.starts_with(crate::app_mutex::APP_MUTEX_NAME));
    }

    /// **A same-profile restart is not refused over its own predecessor's
    /// claim**: the claim is tried again, briefly, and taken once it is let
    /// go. The pauses are counted, not slept.
    #[test]
    fn the_port_claim_is_retried_for_about_two_seconds_before_it_refuses() {
        use std::cell::Cell;
        use std::time::Duration;

        // Free on the fourth try: taken, after three pauses.
        let tries = Cell::new(0u32);
        let slept = Cell::new(Duration::ZERO);
        let taken = claim_port_retrying(
            PORT_CLAIM_ATTEMPTS,
            PORT_CLAIM_RETRY_EVERY,
            |pause| slept.set(slept.get() + pause),
            || {
                tries.set(tries.get() + 1);
                if tries.get() < 4 { Err("held".to_string()) } else { Ok(tries.get()) }
            },
        );
        assert_eq!(taken, Ok(4));
        assert_eq!(slept.get(), PORT_CLAIM_RETRY_EVERY * 3);

        // Never free: refused, after exactly the bound, about two seconds.
        tries.set(0);
        slept.set(Duration::ZERO);
        let refused: Result<u32, String> = claim_port_retrying(
            PORT_CLAIM_ATTEMPTS,
            PORT_CLAIM_RETRY_EVERY,
            |pause| slept.set(slept.get() + pause),
            || {
                tries.set(tries.get() + 1);
                Err(format!("held {}", tries.get()))
            },
        );
        assert_eq!(refused, Err(format!("held {PORT_CLAIM_ATTEMPTS}")), "not the last refusal");
        assert_eq!(tries.get(), PORT_CLAIM_ATTEMPTS);
        assert_eq!(slept.get(), Duration::from_secs(2), "the bound is not about two seconds");

        // Free at once: no pause at all.
        slept.set(Duration::ZERO);
        let at_once = claim_port_retrying(
            PORT_CLAIM_ATTEMPTS,
            PORT_CLAIM_RETRY_EVERY,
            |pause| slept.set(slept.get() + pause),
            || Ok::<_, String>(()),
        );
        assert_eq!(at_once, Ok(()));
        assert_eq!(slept.get(), Duration::ZERO);

        // And production uses the retry, with a real sleep.
        let code = production_code(include_str!("app_profile.rs"));
        let body = &code[code.find("pub fn claim_bw_serve_port()").unwrap()..];
        let body = &body[..body.find("\n}").unwrap()];
        assert!(body.contains(concat!("claim_port_", "retrying(")), "{body}");
        assert!(body.contains(concat!("std::thread::", "sleep,")), "{body}");
    }

    /// **A test profile with an unreadable account list is refused**, in
    /// exactly the case the default copy runs on account-less
    /// (`NoAccountList`) -- and before `main` resolves accounts or points
    /// the CLI anywhere.
    #[test]
    fn a_profile_without_its_account_list_is_refused_at_startup() {
        use crate::accounts::{resolve_startup, Account, AccountId, StartupAccounts};
        let p = profile("rc");
        let path = std::path::Path::new(r"C:\cfg\profiles\rc\settings.json");
        let one = vec![Account {
            id: AccountId::generate(),
            email: "a@example.test".into(),
            server_url: None,
            use_official_bw_crypto: true,
        }];
        for (stored, unreadable) in [(&[][..], true), (&[][..], false), (&one[..], true), (&one[..], false)] {
            let no_list = matches!(
                resolve_startup(stored, None, unreadable.then_some("unreadable")),
                StartupAccounts::NoAccountList { .. }
            );
            let refusal = no_account_list_refusal(Some(&p), stored, unreadable, path);
            assert_eq!(refusal.is_some(), no_list, "not exactly the `NoAccountList` case");
            assert_eq!(no_account_list_refusal(None, stored, unreadable, path), None, "default changed");
            if let Some(why) = refusal {
                assert!(why.contains("settings.json") && why.contains("\"rc\""), "{why}");
            }
        }

        let main_rs = production_code(include_str!("main.rs"));
        let body = &main_rs[main_rs.find("fn main() {").unwrap()..];
        let loaded = body.find(concat!("settings::Settings::", "load(&settings_path);")).unwrap();
        let refused = body
            .find(concat!("deskwarden::app_profile::no_account_list_", "refusal("))
            .expect("`main` no longer refuses a profile without its account list");
        let resolved = body.find(concat!("accounts::resolve_", "startup(")).expect("control");
        let pointed = body.find(concat!("bw_path::set_active_", "data_dir(")).expect("control");
        assert!(loaded < refused && refused < resolved && refused < pointed, "out of order");
        let arm = &body[refused..resolved];
        assert!(arm.contains(concat!("message", "_box(")), "refused without saying so");
        assert!(arm.contains(concat!("std::process::", "exit(PROFILE_REFUSED)")), "not refused");
    }

    /// **Every action on the installed copy asks first** -- before the
    /// network, the registry or the disk is touched.
    #[test]
    fn every_installed_copy_action_asks_the_profile_first() {
        let gate = concat!("app_profile::touches_the_installed_", "copy()");
        for (file, source, function, first_act) in [
            ("updater.rs", include_str!("updater.rs"), "pub fn check_for_update(", ".get(&url)"),
            ("updater.rs", include_str!("updater.rs"), "pub fn download_and_verify(", "create_dir_all("),
            ("updater.rs", include_str!("updater.rs"), "pub fn apply_update(", "apply_update_with("),
            ("autostart_repair.rs", include_str!("autostart_repair.rs"), "pub fn repair_logon_entry(", "RegOpenKeyExW("),
            ("bw_acquire.rs", include_str!("bw_acquire.rs"), "fn add_to_user_path(", "RegOpenKeyExW("),
        ] {
            let code = production_code(source);
            let body = &code[code.find(function).unwrap_or_else(|| panic!("{file}: no {function}"))..];
            let body = &body[..body.find("\n}").unwrap()];
            let asked = body.find(gate).unwrap_or_else(|| panic!("{file} `{function}` never asks"));
            let acted = body.find(first_act).unwrap_or_else(|| panic!("control: {first_act}"));
            assert!(asked < acted, "{file} `{function}` acts before it asks");
        }
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

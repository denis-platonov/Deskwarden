//! User preferences, persisted as `settings.json` in the config directory.
//!
//! Follows `fill_stats`'s pattern: plain serde over a small struct, with
//! every read falling back to defaults. A settings file is never a reason
//! the app cannot start, so a missing, partial, or corrupt file is a
//! silent fall-back rather than an error.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Auto-lock timeout used when the stored value is absent. Matches the
/// constant this replaces in `vault_window`, which was marked "hardcoded
/// until the 3e preferences window exists".
const DEFAULT_AUTO_LOCK_MINUTES: u64 = 15;

/// Floor applied to `auto_lock_minutes` by [`auto_lock_policy`], regardless of
/// what's stored on disk.
///
/// The hazard is unchanged by the arrival of [`Settings::auto_lock_enabled`]:
/// a *number* of minutes ends up in `vault_window::run`'s
/// `last_activity.elapsed() >= timeout` check, and a zero-length timeout is
/// true on the very first frame -- the window would close itself with
/// `locked = true` before the user can do anything, on every single open,
/// forcing a fresh master-password re-auth each time. So while auto-lock is
/// *on*, a zero or corrupt minutes value is still clamped up to the shortest
/// lock period that is actually usable.
///
/// **What a pre-existing `auto_lock_minutes: 0` means now.** Before the
/// toggle existed, `0` was the only way to hand-write "never lock" into
/// `settings.json`, and this clamp turned it into one minute regardless. It
/// still does. `0` is *not* retro-fitted to mean "never": there is now a real
/// field for that, and re-reading an existing file's `0` as "never" would
/// silently turn off auto-lock for a vault whose owner never asked -- a
/// weakening of the file's current behaviour, applied without their
/// knowledge, on upgrade. "Never" is [`Settings::auto_lock_enabled`] set to
/// `false` and nothing else; a `0` still on disk keeps doing exactly what it
/// has always done, which is lock after a minute.
/// `a_pre_existing_zero_still_means_one_minute_and_not_never` pins that.
const MIN_AUTO_LOCK_MINUTES: u64 = 1;

/// When -- if ever -- the vault window locks itself, as
/// `vault_window::run` consumes it.
///
/// Two variants rather than a `Duration` with a magic value, because "never
/// lock" and "lock instantly" are the two *most* confusable states here and
/// the difference between them is the whole security posture of the window.
/// Every sentinel available in a bare `Duration` gets one of them wrong:
/// `Duration::ZERO` is already elapsed on frame one (lock instantly and
/// permanently -- the exact defect [`MIN_AUTO_LOCK_MINUTES`] exists to
/// prevent), and a very large `Duration` means "never" only until someone
/// multiplies it (`u64::MAX` minutes is not representable in seconds at all).
/// [`Never`](AutoLock::Never) is not a duration, so no arithmetic can turn it
/// into a short one, and the consumer cannot forget to handle it -- the
/// `match` will not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoLock {
    /// The vault window stays open until the user locks it by hand or quits.
    Never,
    /// Lock after this much inactivity. Never zero-length: it comes only from
    /// [`auto_lock_policy`], which applies [`MIN_AUTO_LOCK_MINUTES`] first.
    After(Duration),
}

/// The whole auto-lock decision, as a pure function of the two stored fields:
/// given the toggle and the minutes, what does the app actually do?
///
/// Public and separate from [`Settings::auto_lock`] for the same reason
/// [`clamp_auto_lock_minutes`] is separate: this is the answer to "what will
/// these two values really do?", and it is the thing worth testing, so it must
/// be reachable without constructing a `Settings` or opening a window.
///
/// * `enabled == false` is [`AutoLock::Never`] **whatever the minutes say**.
///   The minutes are deliberately not consulted, not zeroed and not forgotten:
///   the preferences window greys the stepper out rather than hiding it, so
///   the number stays legible and comes straight back when the toggle is
///   turned on again.
/// * `enabled == true` is the old behaviour exactly -- the floor applied, then
///   converted to seconds with `saturating_mul`, because a hand-edited or
///   corrupt `settings.json` can hold any `u64` and `* 60` on the largest of
///   those would overflow (panic in debug, wrap to a tiny -- i.e.
///   lock-immediately -- duration in release).
pub const fn auto_lock_policy(enabled: bool, minutes: u64) -> AutoLock {
    if enabled {
        AutoLock::After(Duration::from_secs(
            clamp_auto_lock_minutes(minutes).saturating_mul(60),
        ))
    } else {
        AutoLock::Never
    }
}

/// The auto-lock period that will *actually* be used for a stored
/// `auto_lock_minutes` **while auto-lock is on** -- [`MIN_AUTO_LOCK_MINUTES`]
/// applied, and nothing else. Whether it is on at all is
/// [`auto_lock_policy`]'s question, not this one's.
///
/// Public and pure, and separate from [`Settings::auto_lock`], because
/// the preferences window has to be able to ask the question "what will this
/// number really do?" before it offers the number to the user. A spinner that
/// accepted `0` would display `0` while the vault locked after a minute --
/// a control whose displayed value is not the value in effect, which is the
/// same silent-override defect class as a switch that does nothing.
/// `prefs_ui` therefore routes every value it accepts (typed, stepped, and the
/// one it loads out of `settings.json`) through this function, so the number on
/// screen and the number `auto_lock_policy` uses are the same number by
/// construction rather than by two matching `.max()` calls that could drift.
///
/// A floor only: there is deliberately no ceiling. [`auto_lock_policy`]
/// saturates rather than overflowing, so every `u64` above the floor is a
/// meaningful timeout, and inventing a maximum here would mean a hand-written
/// `settings.json` silently losing its value the first time the preferences
/// window was opened.
pub const fn clamp_auto_lock_minutes(minutes: u64) -> u64 {
    // `Ord::max` is not const, hence the explicit branch.
    if minutes < MIN_AUTO_LOCK_MINUTES {
        MIN_AUTO_LOCK_MINUTES
    } else {
        minutes
    }
}

/// Smallest inner size the vault window may ever be given, in egui points.
///
/// The vault window's three panes are two fixed-width columns (212 + 390)
/// plus a flexible detail pane; below this the detail pane is a sliver and
/// the item rows clip their own text. It is applied twice, and both are
/// load-bearing: `ViewportBuilder::with_min_inner_size` stops the *user*
/// dragging an edge below it (winit hands the floor to the OS resize loop),
/// and [`clamp_window_geometry`] stops a *stored* value below it being
/// restored at launch -- a settings.json that was hand-edited, or written by
/// a future build with a smaller floor, never reaches the window.
pub const MIN_VAULT_WINDOW_SIZE: (i32, i32) = (900, 600);

/// A vault-window position and size as last seen on screen, in egui points
/// (the same space `ViewportBuilder::with_position` and `with_inner_size`
/// read, and the same space `ViewportInfo::inner_rect` reports -- see
/// `egui_winit::inner_rect_in_points`).
///
/// Whole points rather than `f32` on purpose: it keeps [`Settings`]'s `Eq`
/// (which `main.rs`'s `edited != settings` check relies on), and it makes
/// NaN/infinity -- the two values that would defeat every comparison in
/// [`clamp_window_geometry`] -- unrepresentable rather than something the
/// clamp has to remember to reject. Sub-point window placement is not a thing
/// a user can perceive or produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// One monitor's usable area (its work area -- the full monitor minus the
/// taskbar), in the same point space as [`WindowGeometry`]. Supplied by
/// `login_ui::monitor_work_areas`, which is the only impure part of the
/// restore path; everything that *decides* anything is [`clamp_window_geometry`].
///
/// By convention the primary monitor is first: that is the fallback when a
/// stored position overlaps no monitor at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// What [`clamp_window_geometry`] decided to actually open the window with.
///
/// `position` is `None` only when no monitor geometry is known at all (the
/// enumeration failed). Restoring a stored position against an unknown screen
/// layout is exactly the case that puts a window somewhere the user cannot
/// reach it, so that case deliberately keeps the *size* -- which cannot be
/// off-screen -- and lets the OS choose where the window goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPlacement {
    pub width: i32,
    pub height: i32,
    pub position: Option<(i32, i32)>,
}

/// Area of the overlap between a stored window rect and a monitor, in
/// square points. `i64` because two `i32` extents multiply out of `i32`.
///
/// `saturating_add` on the *stored* rect's far edges: `WindowGeometry`
/// deserializes whatever `i32`s a hand-edited or corrupt `settings.json`
/// contains, and `x + width` on two large ones panics in a debug build and
/// wraps in a release one -- wrapping being the worse of the two, since a
/// far edge that has wrapped negative silently changes which monitor the
/// window is judged to be on. Saturating instead makes such a rect overlap
/// everything to its right, which the clamp then handles like any other
/// oversized rect. `saturating_sub` for the same reason and not merely for
/// symmetry: a far edge that has saturated to `i32::MIN` (`x: i32::MAX`,
/// `width: i32::MIN`) minus a near edge at `i32::MAX` is itself out of range,
/// and saturating leaves it hugely negative, i.e. "no overlap", which is the
/// right answer for a rect that degenerate. The monitor rects come from the
/// OS enumeration rather than from disk, so they are left as plain
/// arithmetic.
fn overlap_area(saved: WindowGeometry, area: WorkArea) -> i64 {
    let w = saved
        .x
        .saturating_add(saved.width)
        .min(area.x + area.width)
        .saturating_sub(saved.x.max(area.x));
    let h = saved
        .y
        .saturating_add(saved.height)
        .min(area.y + area.height)
        .saturating_sub(saved.y.max(area.y));
    if w <= 0 || h <= 0 {
        0
    } else {
        w as i64 * h as i64
    }
}

/// The monitor a stored rect belongs to: the one it overlaps most, or -- when
/// it overlaps none of them, which is what a monitor that has since been
/// unplugged looks like -- the primary.
///
/// Ties go to the *earliest* monitor in the list, not the last (which is what
/// `Iterator::max_by_key` would give): a window split exactly evenly across
/// two screens should land on the more primary of the two, and "whichever the
/// enumeration happened to yield last" is not a rule anyone can reason about.
fn target_work_area(saved: WindowGeometry, work_areas: &[WorkArea]) -> Option<WorkArea> {
    let mut best: Option<(i64, WorkArea)> = None;
    for area in work_areas {
        let overlap = overlap_area(saved, *area);
        if overlap > 0 && best.is_none_or(|(b, _)| overlap > b) {
            best = Some((overlap, *area));
        }
    }
    best.map(|(_, a)| a).or_else(|| work_areas.first().copied())
}

/// Turns a geometry read back from `settings.json` into one that is safe to
/// open a window with, given the monitors that exist *now*.
///
/// A stored geometry is a claim about a screen layout that may no longer be
/// true -- the monitor it names may have been unplugged, replaced with a
/// smaller one, or rearranged. Three things can therefore go wrong, and this
/// is the one place each is decided:
///
///  * **Off-screen position.** The rect overlaps no current monitor, so it is
///    re-homed onto the primary and pushed inside its work area. The window
///    is always fully within one monitor afterwards, never straddling the gap
///    between two or hidden under the taskbar.
///  * **Too small.** Anything below [`MIN_VAULT_WINDOW_SIZE`] is raised to it,
///    so a stored sliver cannot reproduce the unusable three-pane layout that
///    floor exists to prevent.
///  * **Too big.** A size larger than the monitor it lands on is shrunk to
///    that monitor's work area -- but never below the floor, so on a screen
///    smaller than 900x600 the floor wins and the window overhangs rather
///    than becoming unusable. That trade is deliberate: an overhanging window
///    can still be moved, a 400px-wide three-pane layout cannot be used.
///
/// The order matters. The floor is applied first so the overlap test and the
/// position clamp both work on the size the window will really have, and the
/// monitor is chosen from the *stored* rect (where the window was) rather
/// than the corrected one.
pub fn clamp_window_geometry(saved: WindowGeometry, work_areas: &[WorkArea]) -> WindowPlacement {
    let (min_width, min_height) = MIN_VAULT_WINDOW_SIZE;
    let width = saved.width.max(min_width);
    let height = saved.height.max(min_height);

    let Some(target) = target_work_area(saved, work_areas) else {
        return WindowPlacement { width, height, position: None };
    };

    // `.max(min_*)` inside the `.min(..)` is what makes the floor outrank the
    // screen: on a monitor narrower than the floor this collapses to the
    // floor rather than to the monitor.
    let width = width.min(target.width.max(min_width));
    let height = height.min(target.height.max(min_height));
    // `.min` before `.max` so that when the window is wider than the work
    // area (only reachable via the line above, i.e. a sub-floor monitor) the
    // window is pinned to the work area's own origin instead of being pushed
    // off its left/top edge.
    let x = saved.x.min(target.x + target.width - width).max(target.x);
    let y = saved.y.min(target.y + target.height - height).max(target.y);
    WindowPlacement { width, height, position: Some((x, y)) }
}

/// The `directories` triple `main.rs` resolves its config directory from, and
/// the file name it joins onto it.
///
/// Duplicated here rather than threaded in because `vault_window::run` -- the
/// only writer of [`Settings::vault_window`] -- is handed an `auto_lock:
/// Duration`, not a settings path, and widening its signature means editing
/// `main.rs`. `the_config_path_still_matches_the_one_main_resolves` is a
/// source-text guard over `main.rs` so this duplication cannot silently drift
/// into writing a second settings file nothing reads.
const PROJECT_QUALIFIER: &str = "dev";
const PROJECT_ORGANIZATION: &str = "Deskwarden";
const PROJECT_APPLICATION: &str = "Deskwarden";
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Where `settings.json` lives, or `None` if the platform has no resolvable
/// config directory (in which case nothing is persisted -- the same silent
/// fall-back every other read here makes).
pub fn default_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORGANIZATION, PROJECT_APPLICATION)
        .map(|dirs| dirs.config_dir().join(SETTINGS_FILE_NAME))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether `bw serve` stays running while the vault is unlocked.
    ///
    /// `true` (the default) is today's behaviour: everything is instant and
    /// the backend holds ~111 MB at idle. `false` runs it only while the
    /// vault window is open; reads come from `VaultCache` either way, so
    /// autofill is unaffected.
    pub keep_backend_running: bool,
    /// Whether focusing a matched window raises the autofill prompt.
    ///
    /// **This is the whole of the automatic half of autofill, and it is one
    /// global switch rather than a choice per vault item.** `true` (the
    /// default, and what an older `settings.json` without this field parses
    /// as) means a window that matches an item raises the overlay, and
    /// nothing is typed until the user clicks Fill on it. `false` means a
    /// match does nothing at all on its own, and the fill hotkey
    /// (`CTRL+ALT+B`) is the only way anything is typed.
    ///
    /// **Neither state fills silently**, which is the reason this replaced
    /// the per-item `AppMatch::trigger`. That enum's `Auto` mode filled the
    /// instant a matched window took focus, and the fill falls back to blind
    /// `SendInput` whenever UI Automation reports no password field -- which
    /// on a real desktop is every window probed so far. So `Auto` could type
    /// a password into whatever happened to hold focus, and it is retired
    /// rather than renamed. See [`crate::app::match_disposition`], the pure
    /// function this field is the only input to.
    ///
    /// **The hotkey arms for every match either way.** Turning this off
    /// removes the prompt, not the binding: `handle_match` still returns the
    /// `(item_id, hwnd)` pair the main loop's `fill_hotkey_pressed` check
    /// fills from. If it did not, `false` would mean autofill was off
    /// entirely, which is the opposite of the fallback it is meant to be.
    pub prompt_on_match: bool,
    /// Whether saved passwords are checked against known breaches.
    ///
    /// `false` (the default, and what an older `settings.json` without this
    /// field parses as) means nothing about a password ever leaves the
    /// machine. `true` opts in to the Have I Been Pwned range API: the
    /// first five characters of a SHA-1 of the password are sent, and the
    /// remaining thirty-five are matched locally, so the service never
    /// learns the password or which of its hashes matched.
    ///
    /// **Off by default rather than on**, unlike every other preference
    /// here. The others describe how this app behaves on this machine;
    /// this one is a network call keyed on the user's passwords, and
    /// making it on their behalf is not ours to decide.
    pub check_breaches: bool,
    /// Whether the vault window locks itself at all.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as) is the behaviour that has always existed: lock after
    /// [`Self::auto_lock_minutes`] of no activity. `false` means never --
    /// the vault stays unlocked until the user locks it by hand or quits.
    ///
    /// That is a real reduction in what this app guarantees, and it is the
    /// user's explicit choice rather than an oversight: it was asked for, the
    /// alternatives (a minimum that cannot be disabled, a confirmation
    /// dialog) were offered, and this is the one that was picked. Nothing
    /// here quietly keeps a hidden floor under it -- see [`auto_lock_policy`],
    /// where "off" is a variant that carries no duration at all.
    pub auto_lock_enabled: bool,
    /// Idle minutes before the vault window locks itself, when
    /// [`Self::auto_lock_enabled`]. Retained while auto-lock is off (the
    /// preferences window greys its stepper out rather than clearing it), so
    /// turning the toggle back on restores the number the user last chose.
    pub auto_lock_minutes: u64,
    /// Where the vault window was, and how big, when it was last closed --
    /// `None` until it has been closed once.
    ///
    /// Written by `vault_window::run` (read-modify-write of this whole file,
    /// so it cannot drop a preference the preferences window changed in the
    /// meantime) and read back through [`clamp_window_geometry`], never
    /// directly: everything in it is a claim about a screen layout that may
    /// no longer exist.
    ///
    /// **This field is only ever authoritative on disk.** `main.rs` holds a
    /// `Settings` loaded once at startup and never refreshed, so its copy of
    /// this field is stale the moment the vault window is first closed. That
    /// is harmless because nothing reads it from memory -- `vault_window`
    /// re-reads the file when it opens -- but it is why
    /// [`Self::persist_preferences`] exists and why [`Self::save`] is private.
    pub vault_window: Option<WindowGeometry>,
    /// Every configured account, in the order the account menu lists them.
    ///
    /// **Owned by the account code, not by the preferences window** -- see
    /// [`Self::persist_accounts`], the third read-modify-write over this file.
    ///
    /// Empty means *"no accounts yet"*, which is the startup condition
    /// [`accounts::resolve_startup`](crate::accounts::resolve_startup) mints
    /// one for. An older `settings.json` written before this field existed
    /// therefore parses as empty and is treated as a machine with no accounts
    /// set up, which is exactly right: it gets one account directory and one
    /// sign-in.
    ///
    /// **No secrets, and no data directory.** An account carries its opaque
    /// id, the email to show in the menu, and the server URL. The CLI profile
    /// directory is *derived* from the id by `accounts::data_dir_for` on every
    /// use, deliberately never stored: a persisted path would be a second
    /// source of truth that can disagree with the first, and a hand-editable
    /// one at that, on a directory this app creates and later
    /// `remove_dir_all`s. The session token and the Hello blob live in that
    /// directory as files, never in here.
    pub accounts: Vec<crate::accounts::Account>,
    /// Which of [`Self::accounts`] the app is currently using, or `None`
    /// before there is one.
    ///
    /// Not an index: an index into a list that another writer can reorder or
    /// shorten silently points at a *different* account, and every path this
    /// value reaches (`BITWARDENCLI_APPDATA_DIR`, the session file, the Hello
    /// label) is one where naming the wrong account is a real failure. An id
    /// that is no longer in the list is resolvable to "none", where a stale
    /// index is not.
    pub active_account: Option<crate::accounts::AccountId>,
    /// The file exists, is not empty, and could not be read back as settings.
    ///
    /// **Never serialized** (`#[serde(skip)]`): it is a fact about *this read*,
    /// not a preference.
    ///
    /// [`Self::load`] has always answered a failed parse with
    /// [`Self::default`], and for the preference fields that is right — a
    /// corrupt file is not a reason to refuse to start. For
    /// [`Self::accounts`] it is not, and the reason is the meaning that field
    /// carries: an empty list is *"no accounts yet — mint one and sign in"*.
    /// So a single unparseable account id — a hand edit, a truncated write —
    /// would otherwise present as a first run. Startup would mint a fresh
    /// account, point the CLI at an empty directory and ask for a master
    /// password, while the user's account sat in `accounts/<old-id>/`; and
    /// [`Self::save`] would refuse the write, so the minted id would not even
    /// be recorded and the next launch would mint another one.
    ///
    /// So the distinction is kept and acted on twice:
    /// [`accounts::resolve_startup`](crate::accounts::resolve_startup) is
    /// handed it and answers
    /// [`StartupAccounts::NoAccountList`](crate::accounts::StartupAccounts::NoAccountList)
    /// instead of minting, and [`Self::save`] refuses to write at all. The
    /// second is what makes the first recoverable: the file the user has to
    /// fix is still there to be fixed.
    #[serde(skip)]
    pub accounts_unreadable: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_backend_running: true,
            prompt_on_match: true,
            check_breaches: false,
            auto_lock_enabled: true,
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
            vault_window: None,
            accounts: Vec::new(),
            active_account: None,
            accounts_unreadable: false,
        }
    }
}

impl Settings {
    /// Reads the file, falling back to the defaults for anything it cannot
    /// read — but *recording* whether there was a file it failed on, in
    /// [`Self::accounts_unreadable`]. See that field for why the difference
    /// between "no file" and "a file I could not parse" is load-bearing here
    /// and nowhere else in this struct.
    ///
    /// A file that is present but empty is treated as absent: that is what a
    /// crashed write leaves, and there is nothing in it to lose.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        if let Ok(parsed) = serde_json::from_str::<Self>(&text) {
            return parsed;
        }
        Self {
            accounts_unreadable: !text.trim().is_empty(),
            ..Self::default()
        }
    }

    /// Writes this whole struct out, field for field.
    ///
    /// Private, and that is the point: this file has three writers with
    /// *disjoint* fields -- the vault window owns [`Self::vault_window`], the
    /// account code owns [`Self::accounts`] and [`Self::active_account`], the
    /// preferences window owns everything else -- and none of them holds a
    /// `Settings` that is fresh in the others' fields. Every write therefore
    /// goes through [`Self::persist_vault_window_geometry`],
    /// [`Self::persist_accounts`] or [`Self::persist_preferences`], each of
    /// which re-reads the file and overwrites only what it owns. A
    /// whole-struct save reachable from outside this module is exactly how the
    /// geometry came to be reverted by an unrelated preferences edit.
    ///
    /// Refuses outright when the copy it is about to write came from a read
    /// that failed ([`Self::accounts_unreadable`]). All three writers below
    /// are read-modify-writes over `Self::load`, so this one check covers
    /// every one of them: without it, changing a single preference while
    /// `settings.json` holds one unparseable account id would replace the
    /// whole file with the defaults, and the account list — the only record of
    /// which directory the user's vault is in — would be gone.
    fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.accounts_unreadable {
            return Err(std::io::Error::other(format!(
                "{} exists but could not be read as settings, so writing this copy back would \
                 replace whatever is in it -- including an account list naming the directory \
                 the vault is actually in. Nothing was written.",
                path.display()
            )));
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// Records where the vault window ended up, without disturbing anything
    /// else in the file.
    ///
    /// Deliberately a read-modify-write rather than a save of some `Settings`
    /// the caller is holding: `vault_window::run` is handed one preference
    /// (`auto_lock`) and no others, so the only `Settings` it could save is
    /// one it invented, and that would silently revert every preference the
    /// user has ever set. Re-reading also means a preference changed while
    /// the vault window was open survives this write.
    ///
    /// That reasoning only ever covered *this* direction, and the opposite
    /// one was the live defect: the preferences window used to save its whole
    /// struct, geometry included, from a copy `main.rs` had loaded at startup
    /// and never refreshed -- so an unrelated auto-lock change wrote `null`
    /// over whatever this function had just persisted. Both writers are now
    /// read-modify-writes over disjoint fields (see
    /// [`Self::persist_preferences`]), which is what makes the close-then-edit
    /// ordering -- the normal one, since opening the vault window blocks the
    /// tray loop -- safe in either order.
    pub fn persist_vault_window_geometry(
        path: &Path,
        geometry: WindowGeometry,
    ) -> std::io::Result<()> {
        let mut settings = Self::load(path);
        settings.vault_window = Some(geometry);
        settings.save(path)
    }

    /// Writes the user's *preferences* back, without disturbing anything else
    /// in the file -- the mirror image of
    /// [`Self::persist_vault_window_geometry`].
    ///
    /// A read-modify-write for the same reason that one is, pointed the other
    /// way. `main.rs` loads `Settings` once at startup and keeps that binding
    /// for the process lifetime; the vault window writes a new geometry
    /// straight to the file whenever it closes, so main's copy of
    /// [`Self::vault_window`] is stale from the first close onwards. Saving
    /// the whole struct when the preferences window returns would write that
    /// stale value back over the geometry on disk, and the next launch would
    /// open at the default size wherever the OS chose to put it. Re-reading
    /// here means the two writers own disjoint fields and cannot clobber each
    /// other in *either* direction.
    ///
    /// The destructuring is deliberate rather than a list of field accesses:
    /// a field added to [`Settings`] becomes a compile error here, forcing
    /// whoever adds it to say which of the writers owns it, instead of
    /// silently joining the set this one drops. [`Settings::accounts`] and
    /// [`Settings::active_account`] are bound as `_` because that is the
    /// answer this function had to give: they belong to
    /// [`Self::persist_accounts`], and writing them from here -- from the
    /// `Settings` `main.rs` loaded at startup -- would delete every account
    /// added since.
    pub fn persist_preferences(&self, path: &Path) -> std::io::Result<()> {
        let Settings {
            keep_backend_running,
            prompt_on_match,
            check_breaches,
            auto_lock_enabled,
            auto_lock_minutes,
            vault_window: _,
            accounts: _,
            active_account: _,
            // Not a preference and not owned by anyone: it describes the read
            // that produced `self`, and the copy this function writes is the
            // one `Self::load` just made below, which carries its own.
            accounts_unreadable: _,
        } = self;
        let mut on_disk = Self::load(path);
        on_disk.keep_backend_running = *keep_backend_running;
        on_disk.prompt_on_match = *prompt_on_match;
        on_disk.check_breaches = *check_breaches;
        on_disk.auto_lock_enabled = *auto_lock_enabled;
        on_disk.auto_lock_minutes = *auto_lock_minutes;
        on_disk.save(path)
    }

    /// Writes the account list and the active account back, without
    /// disturbing anything else in the file -- the third read-modify-write
    /// over these disjoint fields, alongside
    /// [`Self::persist_vault_window_geometry`] and
    /// [`Self::persist_preferences`].
    ///
    /// Free-standing rather than a method for the same reason
    /// [`Self::persist_vault_window_geometry`] is: the callers (adding an
    /// account, removing one, switching the active one, resolving startup)
    /// hold the account list and nothing else, so the only `Settings` they
    /// could save is one they invented.
    ///
    /// The blast radius of getting this wrong is worse than the geometry's,
    /// which is why it is its own writer rather than a widening of the
    /// preferences one. `main.rs` loads `Settings` once at startup; if a
    /// preferences save wrote that stale copy's empty list back over a list
    /// this function had persisted mid-session, the account would vanish --
    /// *and* the next launch would read the empty list as "no accounts yet",
    /// mint a second one and ask for a master password while the first
    /// account's profile sat there unreferenced.
    /// `persisting_preferences_from_a_stale_copy_keeps_the_account_list`
    /// pins that direction, and
    /// `persisting_accounts_keeps_every_preference_and_the_geometry` the other
    /// two.
    ///
    /// `active` is taken by reference and cloned rather than by value so that
    /// clearing it (`None`) is expressible and is a real write: an account
    /// list with no active account is the state a removal of the last account
    /// leaves behind, and it has to survive a restart as itself rather than
    /// as "whatever was active before".
    pub fn persist_accounts(
        path: &Path,
        accounts: &[crate::accounts::Account],
        active: Option<&crate::accounts::AccountId>,
    ) -> std::io::Result<()> {
        let mut on_disk = Self::load(path);
        on_disk.accounts = accounts.to_vec();
        on_disk.active_account = active.cloned();
        on_disk.save(path)
    }

    /// What this settings file means for the vault window's idle timer. All
    /// of the reasoning is in [`auto_lock_policy`]; this is only the lookup.
    pub fn auto_lock(&self) -> AutoLock {
        auto_lock_policy(self.auto_lock_enabled, self.auto_lock_minutes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    /// A scratch settings file, unique to this process **and to this call**.
    ///
    /// It used to be a bare fixed name in the shared system temp directory,
    /// which is the one temp helper in this crate that disambiguated nothing
    /// -- `fill_stats`, `accounts`, `hello`, `bw_path`, `updater` and
    /// `session_store` all add `process::id()` or nanos. Two `cargo test` runs
    /// at once therefore wrote each other's `deskwarden-settings-test-*.json`
    /// mid-assertion, and the failure that surfaced
    /// (`persisting_accounts_keeps_every_preference_and_the_geometry` reading
    /// back `None` for an account it had just written) looked like a bug in
    /// `persist_accounts` rather than like two processes sharing a path.
    ///
    /// Nanos as well as the pid because two tests in the SAME process can ask
    /// for the same label -- `"absent"` is used twice in this module -- and
    /// `cargo test` runs them on different threads at the same time.
    ///
    /// `temp_dir()` and nothing else, ever: no test in this module may go near
    /// the real `%APPDATA%` `settings.json`, which is why nothing here calls
    /// [`default_path`] (guarded by
    /// `every_scratch_settings_file_is_unique_to_this_process_and_this_call`).
    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = temp_dir().join(format!(
            "deskwarden-settings-test-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// **The guard the flake needed.** Every other assertion in this module is
    /// about what `Settings` does to a file; this one is about the file being
    /// this run's own.
    #[test]
    fn every_scratch_settings_file_is_unique_to_this_process_and_this_call() {
        let a = temp_path("collision-probe");
        let b = temp_path("collision-probe");
        assert_ne!(
            a, b,
            "two scratch settings files with the same label are the same path, so two tests in \
             this process -- or two `cargo test` runs -- overwrite each other mid-assertion"
        );

        let pid = std::process::id().to_string();
        let scratch = temp_dir();
        for path in [&a, &b] {
            let name = path
                .file_name()
                .expect("a scratch settings path with no file name")
                .to_string_lossy()
                .into_owned();
            assert!(
                name.contains(&pid),
                "the scratch settings file {name:?} does not name this process, so a second \
                 concurrent `cargo test` run writes the same file"
            );
            // Positive control for the two assertions above: the uniqueness is
            // not coming from the label having been dropped.
            assert!(
                name.contains("collision-probe"),
                "the scratch settings file {name:?} no longer carries its label, so every test \
                 in this module is writing an anonymous file and the names above prove nothing"
            );
            assert!(
                path.starts_with(&scratch),
                "a scratch settings file escaped the system temp directory: {path:?}"
            );
        }

        // ...and no test in this module resolves the REAL settings file. A
        // source guard because the hazard is a call that would silently
        // succeed on the developer's own machine and quietly rewrite their
        // account list. `concat!`-split and single-line: a needle written as
        // one literal would match its own declaration, and one carrying a
        // newline passes on LF and fails on CRLF.
        let source = include_str!("settings.rs");
        let tests = source
            .split_once(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file")
            .1;
        let resolver = concat!("default_", "path()");
        assert_eq!(
            tests.matches(resolver).count(),
            0,
            "a test in this module resolves the real %APPDATA% settings file -- every test here \
             must stay inside `temp_dir()`"
        );
        // Positive control for that absence: production really does spell it
        // this way, so counting zero in the tests means something.
        assert_eq!(
            source.matches(concat!("pub fn default_", "path()")).count(),
            1,
            "the real settings-path resolver is no longer spelled that way -- the needle above \
             has drifted and its absence proves nothing"
        );
    }

    #[test]
    fn the_default_preserves_todays_behaviour() {
        let s = Settings::default();
        assert!(s.keep_backend_running);
        assert!(s.auto_lock_enabled, "auto-lock is on unless it is turned off");
        assert_eq!(s.auto_lock(), AutoLock::After(Duration::from_secs(15 * 60)));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let path = temp_path("round-trip");
        let written = Settings {
            keep_backend_running: false,
            // Deliberately the OPPOSITE of `keep_backend_running`: two `bool`s that
            // agreed would round-trip identically through a writer that assigned
            // one of them from the other.
            prompt_on_match: true,
            // Deliberately the OPPOSITE of this field's own default
            // (`false`), so a writer that dropped it would round-trip to
            // the default and be indistinguishable from one that kept it.
            check_breaches: true,
            auto_lock_enabled: true,
            auto_lock_minutes: 5,
            vault_window: None,
            // Listed rather than `..Settings::default()` so this test keeps
            // failing to compile when a field is added -- the same forcing
            // function `persist_preferences`'s destructuring provides.
            accounts: Vec::new(),
            active_account: None,
            accounts_unreadable: false,
        };
        written.save(&path).unwrap();
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        assert_eq!(Settings::load(&temp_path("absent")), Settings::default());
    }

    #[test]
    fn a_partial_file_keeps_defaults_for_absent_fields() {
        // `#[serde(default)]` on the struct is what makes this work: a file
        // written by an older build must not fail to parse once a field is
        // added.
        let path = temp_path("partial");
        std::fs::write(&path, r#"{"keep_backend_running": false}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, DEFAULT_AUTO_LOCK_MINUTES);
        // The field this pass added, named explicitly: a v0.5.0 `settings.json`
        // has no `prompt_on_match` key, and an upgrading user must get the
        // prompt rather than an app that silently stops offering to fill.
        assert!(
            loaded.prompt_on_match,
            "an older settings.json read as prompt-off, so upgrading turns the automatic \
             half of autofill off for everyone who had it"
        );
        // And the mechanism, stated as what it answers: this is a real file
        // without the key, not a `Settings::default()` in disguise.
        assert_eq!(
            crate::app::match_disposition(loaded.prompt_on_match),
            crate::app::MatchDisposition::Prompt
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_malformed_file_yields_defaults_rather_than_failing() {
        let path = temp_path("malformed");
        std::fs::write(&path, "{not json").unwrap();
        // Every *preference* still falls back to its default -- a corrupt file
        // is not a reason to refuse to start. What is no longer defaulted away
        // is the knowledge that a file was there and could not be read; see
        // `accounts_unreadable` and
        // `one_unparseable_account_id_is_not_read_as_a_first_run`.
        assert_eq!(
            Settings::load(&path),
            Settings {
                accounts_unreadable: true,
                ..Settings::default()
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zero_minutes_clamps_to_the_minimum_instead_of_locking_instantly() {
        // The regression this guards: `0 * 60 == 0`, and a zero-length
        // timeout is already elapsed on the vault window's very first frame,
        // closing it immediately with `locked = true` and forcing a fresh
        // re-auth on every open.
        let s = Settings { auto_lock_minutes: 0, ..Settings::default() };
        assert_eq!(
            s.auto_lock(),
            AutoLock::After(Duration::from_secs(MIN_AUTO_LOCK_MINUTES * 60))
        );
    }

    #[test]
    fn a_normal_value_is_used_as_is() {
        let s = Settings { auto_lock_minutes: 5, ..Settings::default() };
        assert_eq!(s.auto_lock(), AutoLock::After(Duration::from_secs(5 * 60)));
    }

    #[test]
    fn the_toggle_off_means_never_which_is_not_a_short_timeout() {
        // The whole point of `AutoLock` being an enum. "Never" must be
        // unrepresentable as a duration, because every duration is a lock
        // that eventually fires and the shortest of them fires on frame one.
        let off = Settings { auto_lock_enabled: false, auto_lock_minutes: 15, ..Settings::default() };
        assert_eq!(off.auto_lock(), AutoLock::Never);
        // The negative assertion the variant makes possible, spelled out:
        // whatever `Never` is, it is not "already elapsed", and it is not
        // the configured 15 minutes either.
        assert_ne!(off.auto_lock(), AutoLock::After(Duration::ZERO));
        assert_ne!(off.auto_lock(), AutoLock::After(Duration::from_secs(15 * 60)));
        // Positive control on the same minutes: flipping only the toggle is
        // what changed the answer, so a `Never` returned unconditionally
        // fails here.
        let on = Settings { auto_lock_enabled: true, ..off };
        assert_eq!(on.auto_lock(), AutoLock::After(Duration::from_secs(15 * 60)));
    }

    #[test]
    fn the_policy_is_a_pure_function_of_the_toggle_and_the_minutes() {
        // Absolute expectations, so this passes for exactly one
        // implementation. Note the two `false` rows: the minutes are NOT
        // consulted when the toggle is off, including the zero that would
        // otherwise be floored, and including a value that could overflow.
        assert_eq!(auto_lock_policy(true, 15), AutoLock::After(Duration::from_secs(900)));
        assert_eq!(auto_lock_policy(true, 1), AutoLock::After(Duration::from_secs(60)));
        assert_eq!(
            auto_lock_policy(true, 0),
            AutoLock::After(Duration::from_secs(60)),
            "the floor still applies while auto-lock is ON"
        );
        assert_eq!(auto_lock_policy(false, 15), AutoLock::Never);
        assert_eq!(auto_lock_policy(false, 0), AutoLock::Never);
        assert_eq!(auto_lock_policy(false, u64::MAX), AutoLock::Never);
    }

    #[test]
    fn a_pre_existing_zero_still_means_one_minute_and_not_never() {
        // A `settings.json` written before the toggle existed could contain
        // `auto_lock_minutes: 0` -- which used to be the only way to
        // hand-write "never lock", and which has always been clamped to one
        // minute. It still is. Reinterpreting it as `Never` now that a real
        // toggle exists would silently disable auto-lock, on upgrade, for a
        // vault whose owner is still expecting it to lock.
        let path = temp_path("legacy-zero");
        std::fs::write(&path, r#"{"auto_lock_minutes": 0}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(loaded.auto_lock_enabled, "an absent toggle field is ON");
        assert_eq!(
            loaded.auto_lock(),
            AutoLock::After(Duration::from_secs(60)),
            "a legacy `auto_lock_minutes: 0` must keep locking after one minute; \
             turning it into AutoLock::Never would disable auto-lock behind the user's back"
        );
        assert_ne!(loaded.auto_lock(), AutoLock::Never);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_written_before_the_toggle_existed_defaults_the_toggle_to_on() {
        // Stated in the name because it is the decision, not an incidental:
        // the DEFAULT for `auto_lock_enabled` is `true`, so every existing
        // settings.json keeps locking exactly as it did before this feature.
        let path = temp_path("pre-toggle");
        std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 7}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running, "the fields it does carry still land");
        assert_eq!(loaded.auto_lock_minutes, 7);
        assert!(loaded.auto_lock_enabled);
        assert_eq!(loaded.auto_lock(), AutoLock::After(Duration::from_secs(7 * 60)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn both_auto_lock_fields_round_trip_through_settings_json() {
        // Serialised and read back through the real file, not just through
        // `PartialEq` on a struct: a field that is written but not read (or
        // renamed on one side only) would look fine in memory.
        let path = temp_path("auto-lock-round-trip");
        let written = Settings {
            keep_backend_running: true,
            prompt_on_match: false,
            check_breaches: true,
            auto_lock_enabled: false,
            auto_lock_minutes: 42,
            vault_window: None,
            accounts: Vec::new(),
            active_account: None,
            accounts_unreadable: false,
        };
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("auto_lock_enabled"), "not in the file at all: {text}");
        let loaded = Settings::load(&path);
        assert_eq!(loaded, written);
        assert!(!loaded.auto_lock_enabled, "the toggle survived the round trip");
        assert_eq!(loaded.auto_lock_minutes, 42, "and so did the minutes it is hiding");
        assert_eq!(loaded.auto_lock(), AutoLock::Never);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persisting_preferences_carries_the_toggle_as_well_as_the_minutes() {
        // `persist_preferences` writes a named list of fields; a new field
        // that is destructured but never assigned compiles and silently
        // never persists.
        let path = temp_path("prefs-toggle");
        Settings::default().save(&path).unwrap();
        Settings { auto_lock_enabled: false, auto_lock_minutes: 20, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.auto_lock_enabled, "the toggle was dropped by persist_preferences");
        assert_eq!(loaded.auto_lock_minutes, 20);
        // ...and back on again, so "always writes false" fails too.
        Settings { auto_lock_enabled: true, auto_lock_minutes: 20, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(Settings::load(&path).auto_lock_enabled);
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard, for `prompt_on_match`.** Found by mutation, not by
    /// inspection: deleting `on_disk.prompt_on_match = *prompt_on_match;` from
    /// `persist_preferences` left all 1645 lib and 133 bin tests green. The
    /// destructuring above it forces every field to be *named*, which is what
    /// makes a new one impossible to forget entirely -- but naming it and
    /// binding it to `_` compiles just as well as assigning it, and the test
    /// above only ever exercised `auto_lock_enabled`.
    ///
    /// What that mutant shipped: the toggle moves, the app obeys it for the
    /// rest of the session, and the next launch has it back on. A preference
    /// that does not survive a restart is one the user has to set every time,
    /// which is indistinguishable from a broken switch.
    ///
    /// Both directions, because `prompt_on_match` defaults to `true` and a
    /// writer that always wrote the default would pass a one-way test.
    #[test]
    fn persisting_preferences_carries_the_prompt_setting_too() {
        let path = temp_path("prefs-prompt");
        Settings::default().save(&path).unwrap();
        assert!(Settings::load(&path).prompt_on_match, "the premise: it starts on");

        Settings { prompt_on_match: false, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            !loaded.prompt_on_match,
            "the prompt setting was dropped by persist_preferences, so turning it off lasts \
             only until the app is restarted"
        );
        // The neighbours it is destructured beside are untouched, so this is
        // not satisfied by a writer that clobbers the file with defaults.
        assert!(loaded.keep_backend_running);
        assert!(loaded.auto_lock_enabled);

        // ...and back on again, so "always writes false" fails too.
        Settings { prompt_on_match: true, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(Settings::load(&path).prompt_on_match);
        let _ = std::fs::remove_file(&path);
    }

    /// Breach checking is the one preference here that is off unless it is
    /// turned on, so this is a statement about the default and not a
    /// restatement of `Default`: enabling a network call keyed on the user's
    /// passwords is theirs to decide, not ours to assume.
    #[test]
    fn breach_checking_is_off_by_default() {
        assert!(!Settings::default().check_breaches);
        // ...and not merely absent from the in-memory default: a fresh install
        // has no file at all, and that path must land on `false` too.
        assert!(!Settings::load(&temp_path("breach-absent")).check_breaches);
    }

    /// The upgrade path, which is the only way this field can be wrong in the
    /// direction that matters: a `settings.json` written before it existed
    /// must not read as opted in.
    #[test]
    fn an_older_settings_file_without_the_key_loads_as_off() {
        let path = temp_path("breach-older-file");
        let older = br#"{"keep_backend_running": false, "prompt_on_match": true, "auto_lock_minutes": 9}"#;
        // The premise, asserted rather than assumed: a fixture that happened to
        // carry the key would make the rest of this test vacuous.
        assert!(
            !std::str::from_utf8(older).unwrap().contains("check_breaches"),
            "the fixture names the key, so it is not an older file"
        );
        std::fs::write(&path, older).unwrap();
        let loaded = Settings::load(&path);
        // And the premise that the file was read at all, rather than falling
        // back to `Settings::default()` wholesale -- two fields that disagree
        // with the defaults.
        assert!(!loaded.keep_backend_running, "the file was not parsed: {loaded:?}");
        assert_eq!(loaded.auto_lock_minutes, 9);
        assert!(
            !loaded.check_breaches,
            "upgrading opted the user into sending hashes of their passwords over the network"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard `persisting_preferences_carries_the_prompt_setting_too`
    /// documents, for `check_breaches`.** `persist_preferences` destructures
    /// exhaustively, so a new field cannot go unnamed -- but binding it and
    /// never assigning `on_disk.check_breaches` compiles, and that mutant has
    /// survived the whole suite in this repo before.
    ///
    /// Both directions, because a writer that always wrote the default
    /// (`false`) would pass a one-way test.
    #[test]
    fn the_breach_toggle_survives_persist_preferences() {
        let path = temp_path("prefs-breach");
        Settings::default().save(&path).unwrap();
        assert!(!Settings::load(&path).check_breaches, "the premise: it starts off");

        Settings { check_breaches: true, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            loaded.check_breaches,
            "the breach setting was dropped by persist_preferences, so turning it on lasts \
             only until the app is restarted"
        );
        // The neighbours it is destructured beside are untouched, so this is
        // not satisfied by a writer that clobbers the file with something else.
        assert!(loaded.keep_backend_running);
        assert!(loaded.prompt_on_match);
        assert!(loaded.auto_lock_enabled);

        // ...and back off again, so "always writes true" fails too.
        Settings { check_breaches: false, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(!Settings::load(&path).check_breaches);
        let _ = std::fs::remove_file(&path);
    }

    /// The field reaches the file under its own name, the way
    /// `both_auto_lock_fields_round_trip_through_settings_json` pins theirs.
    /// `settings_round_trip_through_disk` compares whole structs, which a
    /// field renamed on both sides at once would still satisfy.
    #[test]
    fn the_breach_toggle_round_trips_through_settings_json_under_its_own_name() {
        let path = temp_path("breach-round-trip");
        let written = Settings { check_breaches: true, ..Settings::default() };
        // The value written disagrees with the default, so a reader that
        // ignored the file entirely would fail here.
        assert!(written.check_breaches != Settings::default().check_breaches);
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("check_breaches"), "not in the file at all: {text}");
        assert_eq!(Settings::load(&path), written);
        assert!(Settings::load(&path).check_breaches);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_geometry_round_trips_through_disk_with_the_rest_of_the_file() {
        let path = temp_path("geometry-round-trip");
        let written = Settings {
            keep_backend_running: false,
            prompt_on_match: true,
            check_breaches: true,
            auto_lock_enabled: true,
            auto_lock_minutes: 5,
            vault_window: Some(WindowGeometry { x: 100, y: 60, width: 1400, height: 900 }),
            accounts: Vec::new(),
            active_account: None,
            accounts_unreadable: false,
        };
        written.save(&path).unwrap();
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persisting_a_geometry_keeps_every_other_preference() {
        // The regression this guards: `vault_window::run` knows one
        // preference (`auto_lock`) and none of the others, so saving a
        // `Settings` it built itself would silently reset
        // `keep_backend_running` every time the vault window closed.
        let path = temp_path("geometry-preserves");
        Settings { keep_backend_running: false, auto_lock_minutes: 7, ..Settings::default() }
            .save(&path)
            .unwrap();
        Settings::persist_vault_window_geometry(
            &path,
            WindowGeometry { x: 1, y: 2, width: 1000, height: 700 },
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, 7);
        assert_eq!(
            loaded.vault_window,
            Some(WindowGeometry { x: 1, y: 2, width: 1000, height: 700 })
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_written_before_geometry_existed_still_parses() {
        // Same `#[serde(default)]` guarantee `a_partial_file_keeps_defaults_
        // for_absent_fields` pins, restated for the field that was added
        // after users already had a settings.json on disk.
        let path = temp_path("pre-geometry");
        std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 3}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.vault_window, None, "an absent geometry is 'never been closed yet'");
        assert_eq!(loaded.auto_lock_minutes, 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_preferences_save_from_a_stale_copy_keeps_the_geometry_on_disk() {
        // The regression this guards, in the order the app actually performs
        // it: `main.rs` loads `Settings` ONCE at startup and holds that value
        // for the whole process, so its `vault_window` is frozen at whatever
        // was on disk then. The vault window is opened, moved and closed --
        // `persist_vault_window_geometry` writes the new geometry to the file,
        // and nothing refreshes main's copy. The user then opens Preferences
        // and changes the auto-lock. Saving the whole struct at that point
        // writes main's stale `vault_window` (here: `None`) over the geometry
        // that is on disk, and the next launch opens at the default size
        // wherever the OS puts it -- with no error anywhere.
        let path = temp_path("prefs-preserve-geometry");
        let at_startup = Settings::load(&path);
        assert_eq!(at_startup.vault_window, None, "first run: no geometry yet");

        let geometry = WindowGeometry { x: 240, y: 120, width: 1500, height: 950 };
        Settings::persist_vault_window_geometry(&path, geometry).unwrap();

        // Preferences, edited from the copy loaded at startup.
        let edited = Settings { auto_lock_minutes: 10, ..at_startup };
        edited.persist_preferences(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(
            loaded.vault_window,
            Some(geometry),
            "a preferences save reverted the saved window geometry"
        );
        assert_eq!(loaded.auto_lock_minutes, 10, "and the preference itself must still land");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_preferences_save_wins_over_a_stale_preference_in_the_file() {
        // The other direction, so the read-modify-write above cannot be
        // "fixed" into merely ignoring the preferences: whatever the file
        // says about a preference, the value the user just chose is the one
        // that must survive.
        let path = temp_path("prefs-win");
        Settings { keep_backend_running: true, auto_lock_minutes: 15, ..Settings::default() }
            .save(&path)
            .unwrap();
        Settings { keep_backend_running: false, auto_lock_minutes: 30, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, 30);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_config_path_still_matches_the_one_main_resolves() {
        // `default_path` duplicates `main.rs`'s `ProjectDirs` triple because
        // `vault_window::run` has no settings path in its signature (see
        // that function's doc). A source-text guard, not a comment: if
        // `main.rs` ever changes the triple or the file name, this crate
        // would start writing window geometry into a second settings file
        // that nothing reads, and every test here would stay green.
        let main_rs = include_str!("main.rs");
        let triple = format!(
            "ProjectDirs::from({PROJECT_QUALIFIER:?}, {PROJECT_ORGANIZATION:?}, {PROJECT_APPLICATION:?})"
        );
        assert!(
            main_rs.contains(&triple),
            "main.rs no longer resolves its config directory with {triple} -- `settings::default_path` \
             duplicates that triple and would now point at a different directory than the file \
             main.rs actually loads"
        );
        let join = format!("config_dir.join({SETTINGS_FILE_NAME:?})");
        assert!(
            main_rs.contains(&join),
            "main.rs no longer builds its settings path with {join} -- see above"
        );
    }

    #[test]
    fn the_auto_lock_floor_is_one_minute_and_nothing_above_it_is_touched() {
        // Absolute values, not `MIN_AUTO_LOCK_MINUTES`: a test that re-derives
        // its expectation from the constant under test passes for every value
        // that constant could ever hold, including a wrong one. This is the
        // function the preferences window bounds its input with, so what it
        // returns is exactly what that window is allowed to display.
        assert_eq!(clamp_auto_lock_minutes(0), 1, "the vault-window-closes-on-frame-one case");
        assert_eq!(clamp_auto_lock_minutes(1), 1);
        assert_eq!(clamp_auto_lock_minutes(2), 2);
        assert_eq!(clamp_auto_lock_minutes(15), 15);
        assert_eq!(clamp_auto_lock_minutes(u64::MAX), u64::MAX, "the floor is a floor, not a range");
    }

    #[test]
    fn the_timeout_is_the_clamped_minutes_in_seconds() {
        // Ties the two together, so `auto_lock_policy` cannot quietly grow a
        // second, different floor from the one the UI bounds its input with.
        for minutes in [0u64, 1, 5, 15, 600] {
            assert_eq!(
                Settings { auto_lock_minutes: minutes, ..Settings::default() }.auto_lock(),
                AutoLock::After(Duration::from_secs(clamp_auto_lock_minutes(minutes) * 60))
            );
        }
    }

    #[test]
    fn an_absurd_value_saturates_instead_of_overflowing() {
        // A hand-edited (or corrupted) settings.json could contain anything
        // that fits in a u64; `* 60` on the largest of those would overflow
        // rather than produce a meaningful timeout.
        let s = Settings { auto_lock_minutes: u64::MAX, ..Settings::default() };
        assert_eq!(s.auto_lock(), AutoLock::After(Duration::from_secs(u64::MAX)));
    }

    // ---- the account list ------------------------------------------------
    //
    // Every id below is written out as a literal 32-character string in the
    // assertions as well as in the construction, so that what is checked
    // against the file is a constant rather than a value the writer produced.

    use crate::accounts::{Account, AccountId};

    /// A valid id made of one repeated hex digit, so the literal it must equal
    /// is readable at the assertion site.
    fn id_of(c: char) -> AccountId {
        AccountId::parse(&std::iter::repeat(c).take(32).collect::<String>())
            .expect("32 repeated lowercase hex characters is a valid id")
    }

    fn account(id: &AccountId) -> Account {
        Account {
            id: id.clone(),
            email: "someone@example.com".to_string(),
            server_url: None,
        }
    }

    /// Does this settings text name a data *directory* anywhere?
    ///
    /// A free function rather than a chain of `assert!(!text.contains(..))`
    /// so the negative assertion can be given a positive control that drives
    /// this exact code -- otherwise "the file mentions no directory" and "the
    /// needles were misspelt" are the same passing test.
    fn mentions_a_data_directory(text: &str) -> bool {
        let lower = text.to_lowercase();
        // Single-line needles: a needle containing a newline passes on an LF
        // checkout and fails on a CRLF one, and this repo has both.
        ["data_dir", "datadir", "directory", "appdata", "c:\\", "c:\\\\"]
            .iter()
            .any(|needle| lower.contains(needle))
    }

    /// Does this settings text carry anything that could be a secret?
    /// Same construction, same reason.
    fn mentions_a_secret(text: &str) -> bool {
        let lower = text.to_lowercase();
        ["password", "session", "token", "secret", "master key"]
            .iter()
            .any(|needle| lower.contains(needle))
    }

    #[test]
    fn the_two_file_content_guards_can_actually_see_what_they_look_for() {
        // The positive control for the two assertions in
        // `the_account_list_round_trips_through_settings_json`. Each planted
        // sample is a shape a careless future `Account` field would really
        // produce, and each drives the same function the real check does.
        for planted in [
            r#"{"accounts":[{"data_dir":"C:\\Users\\me\\AppData\\Roaming\\Deskwarden"}]}"#,
            r#"{"accounts":[{"dataDir":"whatever"}]}"#,
            r#"{"accounts":[{"directory":"whatever"}]}"#,
            r#"{"accounts":[{"path":"c:\\somewhere"}]}"#,
        ] {
            assert!(
                mentions_a_data_directory(planted),
                "the data-directory guard cannot see a directory it was shown: {planted}"
            );
        }
        for planted in [
            r#"{"accounts":[{"password":"hunter2"}]}"#,
            r#"{"accounts":[{"session":"abc"}]}"#,
            r#"{"accounts":[{"api_token":"abc"}]}"#,
        ] {
            assert!(
                mentions_a_secret(planted),
                "the secrets guard cannot see a secret it was shown: {planted}"
            );
        }
        // And neither fires on a settings file of the shape this task writes,
        // so a guard that answered `true` unconditionally would not pass here.
        let benign = r#"{"keep_backend_running":true,"accounts":[{"id":"0123456789abcdef0123456789abcdef","email":"me@example.com","server_url":null}],"active_account":null}"#;
        assert!(!mentions_a_data_directory(benign));
        assert!(!mentions_a_secret(benign));
    }

    #[test]
    fn a_file_written_before_accounts_existed_still_parses() {
        // `#[serde(default)]` on the struct, restated for the two fields this
        // task adds after users already have a settings.json on disk. An
        // absent list must not be a parse failure, because a parse failure
        // here silently discards every preference in the file.
        let path = temp_path("pre-accounts");
        std::fs::write(&path, r#"{"keep_backend_running": false, "auto_lock_minutes": 3}"#)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            loaded.accounts.is_empty(),
            "an absent list is 'no accounts yet', not a parse failure"
        );
        assert_eq!(loaded.active_account, None);
        assert!(!loaded.keep_backend_running, "the fields it does carry still land");
        assert_eq!(loaded.auto_lock_minutes, 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn one_unparseable_account_id_is_not_read_as_a_first_run() {
        // Task 5 pinned "the whole file is discarded" as current behaviour and
        // left narrowing it to whoever wired the account list up. This is
        // that, and the reason it could not stay: an empty account list MEANS
        // "no accounts yet", so a corrupt id would send startup to mint a
        // fresh account and ask for a master password -- while the user's own
        // account stayed in `accounts/<old-id>/` with nothing on disk still
        // naming it.
        let path = temp_path("bad-account-id");
        std::fs::write(
            &path,
            r#"{"keep_backend_running":false,"accounts":[{"id":"NOT-AN-ID","email":"me@example.com","server_url":null}],"active_account":null}"#,
        )
        .unwrap();

        let loaded = Settings::load(&path);
        assert!(
            loaded.accounts_unreadable,
            "an unreadable account list reads as a first run, which is what mints a second \
             account beside the one already on disk"
        );
        assert!(loaded.accounts.is_empty(), "control: it still parsed as nothing");

        // And the file is protected from every writer, because all three go
        // through `load` then `save`.
        let err = Settings::persist_accounts(&path, &[], None)
            .expect_err("persisting an empty list over an unreadable one must be refused");
        assert!(
            err.to_string().contains("could not be read as settings"),
            "unexpected refusal: {err}"
        );
        assert!(
            loaded.persist_preferences(&path).is_err(),
            "a preference edit would have replaced the whole file with the defaults"
        );
        let still_there = std::fs::read_to_string(&path).unwrap();
        assert!(
            still_there.contains("NOT-AN-ID"),
            "the file was rewritten anyway: {still_there}"
        );

        // Positive controls on all three, on the same file made valid: none of
        // this is a writer that simply always refuses, and none of it is a
        // loader that always reports trouble.
        std::fs::write(
            &path,
            r#"{"keep_backend_running":false,"accounts":[],"active_account":null}"#,
        )
        .unwrap();
        let good = Settings::load(&path);
        assert!(!good.accounts_unreadable);
        Settings::persist_accounts(&path, &[], None).expect("a readable file must be writable");
        good.persist_preferences(&path)
            .expect("a readable file must take a preference edit");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_absent_or_empty_settings_file_is_still_a_first_launch_rather_than_trouble() {
        // The other side of the distinction above, and the one that must NOT
        // move: no file at all is a first launch, and so is the zero-byte file
        // a crashed write leaves. Reporting either as unreadable would leave
        // a brand-new install permanently account-less.
        let path = temp_path("absent");
        let _ = std::fs::remove_file(&path);
        assert!(!Settings::load(&path).accounts_unreadable, "no file at all");

        std::fs::write(&path, "").unwrap();
        assert!(!Settings::load(&path).accounts_unreadable, "an empty file");
        std::fs::write(&path, "   \r\n ").unwrap();
        assert!(!Settings::load(&path).accounts_unreadable, "whitespace only");

        // Positive control on the same reader and the same path: content that
        // really is there and really is broken does report.
        std::fs::write(&path, "{").unwrap();
        assert!(Settings::load(&path).accounts_unreadable);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_account_list_round_trips_through_settings_json() {
        let path = temp_path("accounts-round-trip");
        let a = AccountId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let written = Settings {
            accounts: vec![
                Account {
                    id: a.clone(),
                    email: "work@example.com".into(),
                    server_url: Some("https://vault.example.com".into()),
                },
                Account {
                    id: id_of('a'),
                    email: "me@example.com".into(),
                    server_url: None,
                },
            ],
            active_account: Some(a.clone()),
            ..Settings::default()
        };
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("work@example.com"), "not in the file at all: {text}");
        assert!(
            !mentions_a_data_directory(&text),
            "the DATA DIRECTORY is derived, never stored -- storing it makes a second source \
             of truth that can disagree with the first: {text}"
        );
        assert!(!mentions_a_secret(&text), "NO SECRETS: {text}");
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persist_accounts_really_writes_the_list_into_the_file() {
        // Read back as raw JSON rather than through `Settings::load`: a
        // round-trip runs the writer's own serde impl in reverse, so it
        // cannot tell "the writer carried the field" from "the writer and the
        // reader agree about dropping it". These assertions are against string
        // literals, and fail if `persist_accounts` drops `accounts`, drops
        // `active_account`, writes an index instead of an id, or reorders.
        let path = temp_path("accounts-writer-carries");
        let a = AccountId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let accounts = vec![
            Account {
                id: a.clone(),
                email: "work@example.com".into(),
                server_url: Some("https://vault.example.com".into()),
            },
            Account {
                id: id_of('a'),
                email: "me@example.com".into(),
                server_url: None,
            },
        ];
        Settings::persist_accounts(&path, &accounts, Some(&a)).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let list = json
            .get("accounts")
            .unwrap_or_else(|| panic!("`accounts` is not in the written file at all: {text}"))
            .as_array()
            .unwrap_or_else(|| panic!("`accounts` is not a list: {text}"));
        assert_eq!(list.len(), 2, "the writer did not carry both accounts: {text}");
        assert_eq!(list[0]["id"], "0123456789abcdef0123456789abcdef");
        assert_eq!(list[0]["email"], "work@example.com");
        assert_eq!(list[0]["server_url"], "https://vault.example.com");
        assert_eq!(list[1]["id"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(list[1]["email"], "me@example.com");
        assert_eq!(
            list[1]["server_url"],
            serde_json::Value::Null,
            "bitwarden.com is an absent server URL, not an empty string: {text}"
        );
        assert_eq!(
            json["active_account"], "0123456789abcdef0123456789abcdef",
            "the active account is stored as the id itself, never as a position in the list: {text}"
        );
        assert!(!mentions_a_data_directory(&text), "{text}");
        assert!(!mentions_a_secret(&text), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persist_accounts_can_clear_the_active_account() {
        // `None` has to be a write, not a no-op: removing the last account,
        // or logging out of the active one, leaves a list with nothing
        // active, and that state has to survive a restart as itself. An
        // implementation that only ever assigns `Some` passes every other
        // test here.
        let path = temp_path("accounts-clear-active");
        let a = id_of('b');
        Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();
        assert_eq!(
            Settings::load(&path).active_account,
            Some(a.clone()),
            "positive control: the active account was set in the first place"
        );

        Settings::persist_accounts(&path, &[account(&a)], None).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            json["active_account"],
            serde_json::Value::Null,
            "clearing the active account did not reach the file: {text}"
        );
        assert_eq!(
            json["accounts"].as_array().map(Vec::len),
            Some(1),
            "and the list itself is still there: {text}"
        );
        assert_eq!(Settings::load(&path).active_account, None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persisting_accounts_keeps_every_preference_and_the_geometry() {
        // All three writers over the same file, in one order; the two other
        // pairings are pinned by `persisting_preferences_from_a_stale_copy_
        // keeps_the_account_list` and by the pre-existing geometry tests.
        let path = temp_path("accounts-preserve");
        Settings { keep_backend_running: false, auto_lock_minutes: 7, ..Settings::default() }
            .save(&path)
            .unwrap();
        Settings::persist_vault_window_geometry(
            &path,
            WindowGeometry { x: 1, y: 2, width: 1000, height: 700 },
        )
        .unwrap();
        let a = id_of('b');
        Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();

        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running, "persist_accounts clobbered a preference");
        assert_eq!(loaded.auto_lock_minutes, 7);
        assert_eq!(
            loaded.vault_window.map(|g| g.x),
            Some(1),
            "persist_accounts clobbered the geometry"
        );
        assert_eq!(loaded.active_account, Some(a));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persisting_preferences_from_a_stale_copy_keeps_the_account_list() {
        // The regression, in the order the app performs it: `main` loads
        // `Settings` once at startup; an account is added mid-session and
        // written by `persist_accounts`; the user then opens Preferences and
        // changes the auto-lock. A whole-struct save writes main's stale
        // (empty) list back and the added account VANISHES on next launch --
        // and with an empty list, the NEXT startup reads "no accounts yet",
        // mints another one and asks for a master password. Same trap the
        // geometry fell into, with a far worse blast radius.
        let path = temp_path("prefs-preserve-accounts");
        let at_startup = Settings::load(&path);
        assert!(at_startup.accounts.is_empty());
        let a = id_of('c');
        Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();

        Settings { auto_lock_minutes: 10, ..at_startup }.persist_preferences(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.accounts.len(), 1, "a preferences save deleted the account list");
        assert_eq!(loaded.active_account, Some(a));
        assert_eq!(loaded.auto_lock_minutes, 10, "and the preference itself must still land");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn persisting_accounts_wins_over_a_stale_list_in_the_file() {
        // The other direction, so the read-modify-write cannot be "fixed"
        // into merely ignoring the accounts it was handed.
        let path = temp_path("accounts-win");
        let (a, b) = (id_of('d'), id_of('e'));
        Settings::persist_accounts(&path, &[account(&a)], Some(&a)).unwrap();
        Settings::persist_accounts(&path, &[account(&a), account(&b)], Some(&b)).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.accounts.len(), 2);
        assert_eq!(loaded.active_account, Some(b));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_id_that_is_not_an_id_falls_the_whole_file_back_to_defaults() {
        // An id that `AccountId::parse` rejects still makes the whole file
        // unparseable and the list still reads as empty -- that has not
        // changed, and it is what keeps a traversal out of `data_dir_for`.
        //
        // What Task 11 added is the second half: an empty list is what mints
        // an account, so the read now also REPORTS that it failed, and startup
        // mints nothing (and `save` refuses to write) while it says so. Task 5
        // flagged narrowing this as the decision belonging to whoever wired
        // the account list up; this is it, and it was narrowed by keeping the
        // distinction rather than by accepting the bad entry.
        let path = temp_path("accounts-bad-id");
        std::fs::write(
            &path,
            r#"{"keep_backend_running": false, "accounts": [{"id": "../evil", "email": "x@example.com", "server_url": null}]}"#,
        )
        .unwrap();
        assert_eq!(
            Settings::load(&path),
            Settings {
                accounts_unreadable: true,
                ..Settings::default()
            },
            "a rejected id is not quietly turned into a usable account"
        );

        // Positive control: the identical file with a well-formed id parses,
        // so the assertion above is about the id and not about the shape of
        // the JSON around it.
        std::fs::write(
            &path,
            r#"{"keep_backend_running": false, "accounts": [{"id": "0123456789abcdef0123456789abcdef", "email": "x@example.com", "server_url": null}]}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.accounts.len(), 1);
        assert!(!loaded.keep_backend_running);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod clamp_window_geometry_tests {
    //! The saved-geometry -> applied-geometry rule.
    //!
    //! Everything impure about restoring a window (asking Windows which
    //! monitors exist, building a `ViewportBuilder`) is elsewhere; this is
    //! the only thing that *decides*, so it is the only thing that can be
    //! wrong in a way the user would ever see, and it is tested directly.
    use super::{clamp_window_geometry, WindowGeometry, WindowPlacement, WorkArea};

    /// One 1920x1080 monitor at the origin with a 40px taskbar along the
    /// bottom -- the ordinary single-screen case.
    const PRIMARY: WorkArea = WorkArea { x: 0, y: 0, width: 1920, height: 1040 };

    fn geometry(x: i32, y: i32, width: i32, height: i32) -> WindowGeometry {
        WindowGeometry { x, y, width, height }
    }

    /// Asserts the placement sits entirely inside `area` -- the property
    /// every case below has to end up with, however it got there.
    fn assert_inside(placement: WindowPlacement, area: WorkArea) {
        let (x, y) = placement.position.expect("a known monitor always yields a position");
        assert!(
            x >= area.x
                && y >= area.y
                && x + placement.width <= area.x + area.width
                && y + placement.height <= area.y + area.height,
            "{placement:?} is not fully inside {area:?}"
        );
    }

    #[test]
    fn a_geometry_that_still_fits_is_used_exactly_as_stored() {
        let placement = clamp_window_geometry(geometry(120, 80, 1240, 740), &[PRIMARY]);
        assert_eq!(
            placement,
            WindowPlacement { width: 1240, height: 740, position: Some((120, 80)) },
            "nothing is wrong with this geometry; clamping must be a no-op or it is just \
             a second, worse window manager"
        );
    }

    #[test]
    fn a_position_off_every_current_monitor_is_re_homed_onto_the_primary() {
        // The named case: saved on a second monitor that has since been
        // unplugged. Left alone this opens a window at x=3000 on a machine
        // whose desktop ends at 1920 -- invisible, unfocusable, and
        // indistinguishable from the app failing to start.
        let placement = clamp_window_geometry(geometry(3000, 1400, 1240, 740), &[PRIMARY]);
        assert_eq!(placement.width, 1240, "the size was fine; only the position was not");
        assert_eq!(placement.height, 740);
        assert_eq!(placement.position, Some((680, 300)));
        assert_inside(placement, PRIMARY);
    }

    #[test]
    fn a_position_off_the_top_left_is_pulled_back_onto_the_screen() {
        // The mirror image, which `.max(area.x)` is what catches: a
        // negative-coordinate rect on a layout that no longer has a monitor
        // to the left.
        let placement = clamp_window_geometry(geometry(-4000, -3000, 1240, 740), &[PRIMARY]);
        assert_eq!(placement.position, Some((0, 0)));
        assert_inside(placement, PRIMARY);
    }

    #[test]
    fn a_size_below_the_floor_is_raised_to_it() {
        // A hand-edited settings.json, or one written by a build with a
        // different floor. 320x200 is the three-pane layout squeezed into
        // exactly the sliver `MIN_VAULT_WINDOW_SIZE` exists to prevent.
        let placement = clamp_window_geometry(geometry(100, 100, 320, 200), &[PRIMARY]);
        assert_eq!(placement.width, 900);
        assert_eq!(placement.height, 600);
        assert_inside(placement, PRIMARY);
    }

    #[test]
    fn a_size_larger_than_the_current_screen_is_shrunk_to_its_work_area() {
        // Saved on a 4K monitor, restored on a 1366x768 laptop panel. Note
        // the height stops at the *work* area (768 - 40 = 728), not the
        // monitor: a window sized to the full monitor height has its bottom
        // edge under the taskbar and cannot be resized from there.
        let laptop = WorkArea { x: 0, y: 0, width: 1366, height: 728 };
        let placement = clamp_window_geometry(geometry(0, 0, 3840, 2160), &[laptop]);
        assert_eq!(placement.width, 1366);
        assert_eq!(placement.height, 728);
        assert_inside(placement, laptop);
    }

    #[test]
    fn the_floor_outranks_a_screen_that_is_smaller_than_the_floor() {
        // The two clamps genuinely conflict here, and this pins which one
        // wins. An overhanging window can still be dragged and resized; a
        // 640x480 three-pane layout cannot be used at all.
        let tiny = WorkArea { x: 0, y: 0, width: 640, height: 480 };
        let placement = clamp_window_geometry(geometry(50, 50, 1240, 740), &[tiny]);
        assert_eq!(placement.width, 900, "the floor, not the 640px screen");
        assert_eq!(placement.height, 600);
        assert_eq!(
            placement.position,
            Some((0, 0)),
            "pinned to the work area's own origin -- pushing it further left to 'fit' would \
             hide the titlebar, which is the one part that has to stay reachable"
        );
    }

    #[test]
    fn a_window_on_a_secondary_monitor_that_still_exists_stays_there() {
        // The whole point of persisting a position: a user with a monitor to
        // the *left* of the primary (negative coordinates, which Windows
        // uses for that layout) must not have their window yanked back to
        // the primary on every launch.
        let secondary = WorkArea { x: -1920, y: 0, width: 1920, height: 1040 };
        let placement =
            clamp_window_geometry(geometry(-1800, 100, 1240, 740), &[PRIMARY, secondary]);
        assert_eq!(placement.position, Some((-1800, 100)));
        assert_inside(placement, secondary);
    }

    #[test]
    fn the_monitor_holding_most_of_the_window_is_the_one_it_is_clamped_into() {
        // A window straddling two screens has to be clamped into exactly one
        // of them, and "the one it is mostly on" is the only choice that
        // doesn't visibly jump.
        let right = WorkArea { x: 1920, y: 0, width: 1920, height: 1040 };
        // 1000 of this window's 1240 points are on `right`.
        let placement = clamp_window_geometry(geometry(1680, 100, 1240, 740), &[PRIMARY, right]);
        assert_inside(placement, right);
        assert_eq!(placement.position, Some((1920, 100)));
    }

    #[test]
    fn no_known_monitors_means_no_position_at_all() {
        // `login_ui::monitor_work_areas` returning empty is the enumeration
        // failing. Restoring a stored position against an unknown layout is
        // precisely the "window opens where nobody can reach it" case, so
        // the size (which cannot be off-screen) is kept and the placement is
        // handed back to the OS.
        let placement = clamp_window_geometry(geometry(3000, 3000, 400, 300), &[]);
        assert_eq!(placement.position, None);
        assert_eq!(placement.width, 900, "the floor still applies -- it needs no monitor");
        assert_eq!(placement.height, 600);
    }

    #[test]
    fn an_extreme_stored_rect_saturates_instead_of_overflowing() {
        // `WindowGeometry` deserializes any four `i32`s, so a hand-edited or
        // corrupt settings.json reaches `overlap_area`'s `x + width`
        // unvalidated: this input panicked with "attempt to add with
        // overflow" in a debug build, and wrapped in a release one -- a
        // wrapped far edge quietly changes which monitor the rect is judged
        // to be on. Same class as `an_absurd_value_saturates_instead_of_
        // overflowing` for the auto-lock timeout.
        let placement = clamp_window_geometry(
            geometry(i32::MAX, i32::MAX, i32::MAX, i32::MAX),
            &[PRIMARY],
        );
        assert_inside(placement, PRIMARY);
        // The mirror image, where the *far* edge saturates negative.
        let flipped = clamp_window_geometry(
            geometry(i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            &[PRIMARY],
        );
        assert_inside(flipped, PRIMARY);
        // And the all-negative corner.
        let negative = clamp_window_geometry(
            geometry(i32::MIN, i32::MIN, i32::MIN, i32::MIN),
            &[PRIMARY],
        );
        assert_inside(negative, PRIMARY);
    }

    #[test]
    fn a_degenerate_stored_rect_is_treated_as_belonging_to_no_monitor() {
        // A zero or negative extent can only come from a corrupt or
        // hand-written file. It overlaps nothing (`overlap_area` returns 0
        // for it), so it takes the primary-monitor fallback rather than
        // producing a nonsense "best" match.
        let placement = clamp_window_geometry(geometry(50, 50, 0, -10), &[PRIMARY]);
        assert_eq!(placement.width, 900);
        assert_eq!(placement.height, 600);
        assert_inside(placement, PRIMARY);
    }
}

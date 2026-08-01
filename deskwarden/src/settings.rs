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

/// Floor applied to `auto_lock_minutes` by `auto_lock_timeout`, regardless of
/// what's stored on disk.
///
/// `0` is the natural value to hand-write into `settings.json` for "never
/// lock", but `auto_lock_timeout`'s result feeds `vault_window::run`'s
/// `last_activity.elapsed() >= auto_lock` check, where a zero-length timeout
/// is true on the very first frame -- the window would close itself with
/// `locked = true` before the user can do anything, on every single open,
/// forcing a fresh master-password re-auth each time. There's no "never
/// lock" mode in this app (see the design spec), so a zero or corrupt value
/// is clamped up to the shortest lock period that's still usable rather than
/// being treated as meaningful.
const MIN_AUTO_LOCK_MINUTES: u64 = 1;

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
    /// Idle minutes before the vault window locks itself.
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_backend_running: true,
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
            vault_window: None,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Writes this whole struct out, field for field.
    ///
    /// Private, and that is the point: this file has two writers with
    /// *disjoint* fields -- the vault window owns [`Self::vault_window`], the
    /// preferences window owns everything else -- and neither holds a
    /// `Settings` that is fresh in the other's fields. Every write therefore
    /// goes through [`Self::persist_vault_window_geometry`] or
    /// [`Self::persist_preferences`], each of which re-reads the file and
    /// overwrites only what it owns. A whole-struct save reachable from
    /// outside this module is exactly how the geometry came to be reverted by
    /// an unrelated preferences edit.
    fn save(&self, path: &Path) -> std::io::Result<()> {
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
    /// whoever adds it to say which of the two writers owns it, instead of
    /// silently joining the set this one drops.
    pub fn persist_preferences(&self, path: &Path) -> std::io::Result<()> {
        let Settings { keep_backend_running, auto_lock_minutes, vault_window: _ } = self;
        let mut on_disk = Self::load(path);
        on_disk.keep_backend_running = *keep_backend_running;
        on_disk.auto_lock_minutes = *auto_lock_minutes;
        on_disk.save(path)
    }

    pub fn auto_lock_timeout(&self) -> Duration {
        // `.max(MIN_AUTO_LOCK_MINUTES)` first, so the floor applies to the
        // stored value itself rather than to a possibly-already-overflowed
        // product. `saturating_mul` handles the separate case of an absurdly
        // large stored value (a corrupt or hand-edited file): plain `* 60`
        // would overflow `u64` and panic in a debug build (or silently wrap
        // to a tiny duration in release), where saturating to
        // `Duration::from_secs(u64::MAX)` -- effectively forever -- is a far
        // safer failure mode for a *lock* timeout to have.
        let minutes = self.auto_lock_minutes.max(MIN_AUTO_LOCK_MINUTES);
        Duration::from_secs(minutes.saturating_mul(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = temp_dir().join(format!("deskwarden-settings-test-{name}.json"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn the_default_preserves_todays_behaviour() {
        let s = Settings::default();
        assert!(s.keep_backend_running);
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(15 * 60));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let path = temp_path("round-trip");
        let written = Settings {
            keep_backend_running: false,
            auto_lock_minutes: 5,
            vault_window: None,
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
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_malformed_file_yields_defaults_rather_than_failing() {
        let path = temp_path("malformed");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
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
            s.auto_lock_timeout(),
            Duration::from_secs(MIN_AUTO_LOCK_MINUTES * 60)
        );
    }

    #[test]
    fn a_normal_value_is_used_as_is() {
        let s = Settings { auto_lock_minutes: 5, ..Settings::default() };
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(5 * 60));
    }

    #[test]
    fn a_geometry_round_trips_through_disk_with_the_rest_of_the_file() {
        let path = temp_path("geometry-round-trip");
        let written = Settings {
            keep_backend_running: false,
            auto_lock_minutes: 5,
            vault_window: Some(WindowGeometry { x: 100, y: 60, width: 1400, height: 900 }),
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
        Settings { keep_backend_running: false, auto_lock_minutes: 7, vault_window: None }
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
        Settings { keep_backend_running: true, auto_lock_minutes: 15, vault_window: None }
            .save(&path)
            .unwrap();
        Settings { keep_backend_running: false, auto_lock_minutes: 30, vault_window: None }
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
    fn an_absurd_value_saturates_instead_of_overflowing() {
        // A hand-edited (or corrupted) settings.json could contain anything
        // that fits in a u64; `* 60` on the largest of those would overflow
        // rather than produce a meaningful timeout.
        let s = Settings { auto_lock_minutes: u64::MAX, ..Settings::default() };
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(u64::MAX));
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

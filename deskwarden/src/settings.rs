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

// ---------------------------------------------------------------------------
// Clearing a copied secret off the clipboard
// ---------------------------------------------------------------------------

/// **The interval is stored, and reasoned about, in whole seconds.** The
/// preferences window is the only place it is ever a decimal number of
/// minutes.
///
/// That split is deliberate and it is the reason none of the constants below
/// is a float. `settings.json` holding `0.1` would hold a rounding artefact --
/// one tenth is not representable in binary floating point -- and a float
/// reaching `clipboard::verdict` would put approximate comparison in the one
/// place this app does exact integer arithmetic over an `Instant`. The user
/// types minutes; the file and the logic hold seconds.
///
/// What [`Settings::clear_clipboard_seconds`] starts at: **60 seconds, one
/// minute**. It moved here from `clipboard::DEFAULT_CLEAR_AFTER`, which was a
/// fixed 45, and went *up* to a round minute on the way, because a number the
/// user can see and type should be one they can say out loud.
const DEFAULT_CLIPBOARD_SECONDS: u64 = 60;

/// Floor on [`Settings::clear_clipboard_seconds`]: **30 seconds**, which the
/// preferences window offers as `0.5` minutes.
///
/// **The reasoning is the one already written on
/// `clipboard::DEFAULT_CLEAR_AFTER`, and it is now doing double duty.** The
/// floor is set by what the user is actually doing: copy a password, alt-tab
/// to a browser, wait for a sign-in page that is still loading, click the
/// field, paste. Thirty seconds is survivable but not comfortable -- a slow
/// page, a 2FA step that appears before the password field, or a user who is
/// not looking at the screen all eat it -- and **a password manager whose
/// clipboard expires before the paste has trained the user to copy twice,
/// which is worse than not clearing at all.** Thirty seconds *is* that floor,
/// so `0.5` is the smallest value the field accepts and there is nothing below
/// it worth offering.
///
/// **Below it is refused rather than silently clamped**, and the copy names
/// the reason: see [`parse_clipboard_minutes`] and
/// [`ClipboardEntry::BelowFloor`]. A control that accepted `0.1` and quietly
/// used 30 seconds would be displaying a number that is not the number in
/// effect, which is the defect [`clamp_auto_lock_minutes`] exists to prevent
/// one field over.
const MIN_CLIPBOARD_SECONDS: u64 = 30;

/// Ceiling on [`Settings::clear_clipboard_seconds`]: **3600 seconds, one
/// hour**.
///
/// **A ceiling exists at all because a very long interval makes the control
/// meaningless.** The exposure this timer addresses is the user walking away,
/// or a program that reads the clipboard on a poll. Past an hour, "it will be
/// cleared eventually" is not a security property anyone can rely on, and a
/// field that ran to `u64::MAX` would let a user set a number that reads as
/// protection and is not.
///
/// **One hour, rather than the "a minute or two" `clipboard::
/// DEFAULT_CLEAR_AFTER`'s own reasoning argues for, and the difference is who
/// is choosing.** That paragraph was picking a *default* imposed on everybody;
/// this is the far end of a range a user has to walk to deliberately. Someone
/// who keeps a terminal open and pastes into it every so often has a real
/// reason to want longer than two minutes, and refusing them would push them
/// onto [`Settings::clear_clipboard`] -- i.e. onto *never* -- which is strictly
/// worse than an hour.
///
/// **"Never" is deliberately not spelled as a very large number.** It is
/// [`Settings::clear_clipboard`], a switch that says so. A range ending in a
/// sentinel that means "off" is exactly the arrangement `auto_lock_minutes: 0`
/// used to be, and [`MIN_AUTO_LOCK_MINUTES`]'s doc is the note explaining why
/// that was undone rather than repeated.
const MAX_CLIPBOARD_SECONDS: u64 = 3600;

/// The resolution of the interval: **six seconds, one tenth of a minute**.
///
/// **This is what makes minutes and seconds round-trip exactly, and that is
/// the whole reason for it rather than a taste for round numbers.** The field
/// shows minutes and the file holds seconds, so every stored value has to be
/// writable as a terminating decimal number of minutes -- otherwise the
/// control would display `1.31` for a stored 79 seconds and store 78.6 when
/// the user pressed nothing. A tenth of a minute is exactly six seconds, so
/// every multiple of six is exactly `n/10` minutes and every one-decimal entry
/// is exactly a whole number of seconds. The displayed value and the stored
/// value cannot disagree.
///
/// **A value between steps is refused, not snapped** --
/// [`ClipboardEntry::BetweenSteps`], with copy that says the field takes one
/// decimal place. Snapping would be a silent round the user cannot see, which
/// is the thing this file keeps refusing to do; and the refusal leaves the
/// previous value in place, so nothing is lost by typing `1.25` and being told
/// so.
///
/// The one exception is a hand-edited `settings.json`, which
/// [`clamp_clipboard_seconds`] snaps to the nearest step rather than refusing,
/// because a settings file has nobody to tell. That direction is documented on
/// the clamp itself.
const CLIPBOARD_SECONDS_STEP: u64 = 6;

/// **A clipboard-clearing interval that cannot be zero, negative, or outside
/// the offered range.**
///
/// A newtype over whole seconds with a private field, and the point of it is
/// the constructor: [`ClearInterval::from_seconds`] is the only way to make
/// one and it runs [`clamp_clipboard_seconds`], so no value below
/// [`MIN_CLIPBOARD_SECONDS`] exists to be handed to
/// `clipboard::arm`. Zero would mean *clear it instantly* -- the value gone
/// before the user can paste it -- and that is a footgun made unreachable
/// here rather than a case a branch downstream remembers to check. Negative is
/// unrepresentable in `u64` and so is not a case at all; the parser
/// ([`parse_clipboard_minutes`]) has no sign in its grammar for the same
/// reason, so a `-1` is refused as "not a number" rather than as a range
/// error.
///
/// Seconds rather than a `Duration` in the field, so that `Eq` is exact and
/// the serialized form is an integer; [`Self::duration`] is the conversion,
/// and it is the only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClearInterval(u64);

impl ClearInterval {
    /// The one constructor, clamping on the way in.
    #[must_use]
    pub const fn from_seconds(seconds: u64) -> Self {
        Self(clamp_clipboard_seconds(seconds))
    }

    /// Whole seconds -- what is persisted and what `clipboard` reasons in.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }

    /// The same value as a `Duration`, for `clipboard::arm`'s deadline
    /// arithmetic.
    #[must_use]
    pub const fn duration(self) -> Duration {
        Duration::from_secs(self.0)
    }

    /// The same value as the preferences window shows it: **minutes, with at
    /// most one decimal place, and no trailing `.0`.**
    ///
    /// Exact by construction rather than by rounding -- see
    /// [`CLIPBOARD_SECONDS_STEP`]. `30 -> "0.5"`, `60 -> "1"`, `90 -> "1.5"`,
    /// `3600 -> "60"`. The inverse of [`parse_clipboard_minutes`], and
    /// `every_offered_interval_round_trips_through_the_field` pins that the two
    /// are inverses across the whole range rather than at a handful of sampled
    /// points.
    ///
    /// A `.` and never a `,`, even though the parser accepts both: the app has
    /// no locale of its own to consult, and picking the wrong one to *display*
    /// would be worse than accepting either on the way in.
    #[must_use]
    pub fn as_minutes_text(self) -> String {
        let tenths = self.0 / CLIPBOARD_SECONDS_STEP;
        if tenths.is_multiple_of(10) {
            (tenths / 10).to_string()
        } else {
            format!("{}.{}", tenths / 10, tenths % 10)
        }
    }
}

/// Which of the four things that take a copied secret back off the clipboard
/// are live, and after how long the timer fires -- as a value, decided once.
///
/// **This is the whole of the clipboard section's meaning, and it is a pure
/// function of five stored fields.** Nothing downstream re-derives "is the
/// master switch on?" for itself: the master switch is applied *here*, by
/// [`clipboard_clearing`], and every consumer reads a field that already has
/// it folded in. That is what makes "which triggers are live" a question a
/// test can construct an answer to without driving a window or touching a
/// clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardClearing {
    /// How long after a copy the timer takes it back, or `None` when the
    /// master switch is off.
    ///
    /// `Option` rather than a zero duration for [`AutoLock`]'s reason: zero is
    /// "clear it instantly", which is the opposite of "never clear it", and
    /// they are the two states most worth not confusing. [`ClearInterval`]
    /// rather than a bare `Duration` so that the *inner* value cannot be zero
    /// either -- see that type.
    pub timer: Option<ClearInterval>,
    /// Clear when the vault locks -- by hand, from the tray, or after idling.
    /// `needs_reauth` (the session invalidated elsewhere) rides on this one:
    /// it is the same event arriving from the other direction, and the app has
    /// no separate logout of its own.
    pub on_lock: bool,
    /// Clear when the user switches, adds or removes an account.
    pub on_account_change: bool,
    /// Clear when Deskwarden quits from the tray, or shuts down to install an
    /// update.
    pub on_quit: bool,
}

impl ClipboardClearing {
    /// Whether the interval control means anything -- which is exactly whether
    /// the timer is live.
    ///
    /// Named rather than left as `timer.is_some()` at the call site because it
    /// is the preferences window's enable/disable question, and the point of
    /// the doc on [`ClipboardClearing`] is that that question is answered in
    /// this module and not in `prefs_ui`.
    #[must_use]
    pub const fn interval_is_live(self) -> bool {
        self.timer.is_some()
    }

    /// Whether *anything at all* still takes a copied secret back.
    ///
    /// Deliberately not the same as [`Self::interval_is_live`]: a user can
    /// leave the master switch on and turn all three triggers off, in which
    /// case only the timer is left. The one state where this is `false` is the
    /// master switch being off, and the preferences window says so in words
    /// rather than leaving the reader to add four pills up.
    #[must_use]
    pub const fn clears_at_all(self) -> bool {
        self.timer.is_some() || self.on_lock || self.on_account_change || self.on_quit
    }
}

/// **The clipboard-clearing decision, as a pure function of the five stored
/// fields.**
///
/// Public and separate from [`Settings::clipboard_clearing`] for the reason
/// [`auto_lock_policy`] is separate from [`Settings::auto_lock`]: this is the
/// answer to "what will these values really do?", and it must be reachable
/// without constructing a `Settings`, opening a window, or owning a clipboard.
///
/// * `master == false` is *nothing clears*, whatever the other four say. The
///   three trigger flags and the minutes are not consulted, not zeroed and not
///   forgotten -- the preferences window greys them rather than hiding them,
///   so they come straight back when the master switch is turned on again.
/// * `master == true` passes each trigger through untouched and puts the
///   clamped seconds on the timer, through [`ClearInterval::from_seconds`] --
///   which is the only constructor and does the clamping, so this function
///   cannot produce a zero-length timer even if handed a zero.
#[must_use]
pub const fn clipboard_clearing(
    master: bool,
    on_lock: bool,
    on_account_change: bool,
    on_quit: bool,
    seconds: u64,
) -> ClipboardClearing {
    if master {
        ClipboardClearing {
            timer: Some(ClearInterval::from_seconds(seconds)),
            on_lock,
            on_account_change,
            on_quit,
        }
    } else {
        ClipboardClearing {
            timer: None,
            on_lock: false,
            on_account_change: false,
            on_quit: false,
        }
    }
}

/// The interval that will *actually* be used for a stored
/// `clear_clipboard_seconds`: [`MIN_CLIPBOARD_SECONDS`],
/// [`MAX_CLIPBOARD_SECONDS`] and [`CLIPBOARD_SECONDS_STEP`] applied, and
/// nothing else.
///
/// Public and pure, and the sole constructor of [`ClearInterval`] runs it, so
/// the number the preferences window shows and the number `clipboard` waits
/// out are the same number by construction. A control that displayed `9000`
/// while the clipboard cleared after an hour is the silent-override defect
/// [`clamp_auto_lock_minutes`] exists to prevent, one field over.
///
/// **A ceiling and a step as well as a floor, unlike
/// [`clamp_auto_lock_minutes`]**, and each asymmetry is deliberate: see
/// [`MAX_CLIPBOARD_SECONDS`] and [`CLIPBOARD_SECONDS_STEP`].
///
/// **This is the clamp for values that arrive from `settings.json`, and it
/// snaps rather than refuses.** A hand-written `clear_clipboard_seconds: 14400`
/// is read as 3600, and a hand-written `79` as 78; both are rewritten in that
/// form the first time the preferences window is opened, so the file then says
/// what the app is doing. That is the opposite of what
/// [`parse_clipboard_minutes`] does with the same out-of-step value, and the
/// difference is that a *typed* entry has a user in front of it to be told,
/// while a file does not. Nearest step, ties downward, because between two
/// equally-close intervals the shorter is the one that clears sooner.
#[must_use]
pub const fn clamp_clipboard_seconds(seconds: u64) -> u64 {
    if seconds < MIN_CLIPBOARD_SECONDS {
        return MIN_CLIPBOARD_SECONDS;
    }
    if seconds > MAX_CLIPBOARD_SECONDS {
        return MAX_CLIPBOARD_SECONDS;
    }
    // Nearest multiple of the step, ties downward. Both bounds are themselves
    // multiples of the step, so this can never leave the range.
    let below = seconds - seconds % CLIPBOARD_SECONDS_STEP;
    if seconds - below > CLIPBOARD_SECONDS_STEP / 2 {
        below + CLIPBOARD_SECONDS_STEP
    } else {
        below
    }
}

/// What one entry in the preferences window's interval field means.
///
/// A four-way answer rather than an `Option`, because three of the four are
/// **refusals the user has to be told the reason for**, and a control that
/// rejects `0.1`, `1.25` and `soon` identically teaches nothing. The
/// preferences row turns each refusal into its own sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardEntry {
    /// A number in range and on a step. Carries the interval it means; there
    /// is no separate seconds value to get out of step with it.
    Accepted(ClearInterval),
    /// Not a number this field's grammar recognises at all -- empty, `soon`,
    /// `-1`, `1e3`, `1.2.3`, or two decimal separators.
    NotANumber,
    /// A number, but below [`MIN_CLIPBOARD_SECONDS`] -- i.e. under half a
    /// minute. Refused rather than clamped; see that constant.
    BelowFloor,
    /// A number, but above [`MAX_CLIPBOARD_SECONDS`] -- i.e. over an hour.
    AboveCeiling,
    /// A number in range but not on a [`CLIPBOARD_SECONDS_STEP`] boundary --
    /// more than one decimal place, like `1.25`. Refused rather than snapped;
    /// see that constant.
    BetweenSteps,
}

/// **Parse what the user typed into the interval field: minutes, possibly
/// fractional, into whole seconds.**
///
/// Pure, and separate from the widget for the reason every other decision in
/// this file is: this is the fiddly part, and it must be testable by calling
/// it with a string rather than by typing into a window.
///
/// # What it accepts
///
/// * `1`, `2`, `60` -- whole minutes.
/// * `0.5`, `1.5`, `.5` -- a leading zero is optional.
/// * `0,5`, `1,5` -- **a comma is a decimal separator too.** This is a Windows
///   application and a large part of Europe types a comma; a field that
///   silently refused `0,5` would read as broken rather than as strict. Only
///   one separator, of either kind, in one entry.
/// * Surrounding whitespace, which is trimmed.
/// * A trailing `.0` or `,0`, and any number of trailing zeros -- `1.50` is
///   `1.5`. Trailing zeros carry no information, so refusing them would be
///   pedantry rather than protection.
///
/// # What it refuses, and why each is its own answer
///
/// * No sign is in the grammar, so `-1` is [`ClipboardEntry::NotANumber`]
///   rather than a range error. That is what makes a negative interval
///   unreachable rather than merely checked for: there is no path from a
///   string to a negative number to test.
/// * `0`, `0.1`, `0.4` are [`ClipboardEntry::BelowFloor`] -- under 30 seconds.
///   Zero included, so an instant-clear interval is unreachable from the field
///   as well as from [`ClearInterval`].
/// * `61`, `120` are [`ClipboardEntry::AboveCeiling`].
/// * `1.25`, `0.55` are [`ClipboardEntry::BetweenSteps`] -- in range, but the
///   field's resolution is one decimal place.
///
/// # Arithmetic
///
/// Integer throughout, never `f64`. The whole and fractional parts are parsed
/// as separate digit strings and combined as
/// `whole * 60 + tenths * CLIPBOARD_SECONDS_STEP`, so `0.5` is exactly 30 and
/// not 29.999999999999996. Anything past the first fractional digit must be
/// zero, which is what makes [`ClipboardEntry::BetweenSteps`] detectable
/// exactly rather than by comparing floats.
#[must_use]
pub fn parse_clipboard_minutes(text: &str) -> ClipboardEntry {
    let text = text.trim();
    if text.is_empty() {
        return ClipboardEntry::NotANumber;
    }
    // One separator, of either kind. `split` rather than `splitn` so that
    // "1.2.3" produces three parts and is refused rather than silently read
    // as "1.2".
    let parts: Vec<&str> = text.split(['.', ',']).collect();
    let (whole_text, frac_text) = match parts.as_slice() {
        [whole] => (*whole, ""),
        [whole, frac] => (*whole, *frac),
        _ => return ClipboardEntry::NotANumber,
    };
    // "" is allowed on the left (".5") but only when there is something on
    // the right; "1." is likewise fine, but "." alone is not a number.
    if whole_text.is_empty() && frac_text.is_empty() {
        return ClipboardEntry::NotANumber;
    }
    if !whole_text.bytes().all(|b| b.is_ascii_digit())
        || !frac_text.bytes().all(|b| b.is_ascii_digit())
    {
        return ClipboardEntry::NotANumber;
    }
    let Ok(whole) = (if whole_text.is_empty() { "0" } else { whole_text }).parse::<u64>() else {
        // Only reachable for a run of digits too long for a `u64`, which is
        // certainly above the ceiling -- but it is reported as "not a number"
        // rather than guessed at, because this branch cannot tell the
        // difference between a very large entry and a pasted line of digits.
        return ClipboardEntry::NotANumber;
    };
    let mut digits = frac_text.bytes();
    let tenths = u64::from(digits.next().map_or(0, |b| b - b'0'));
    // Everything past the first decimal place must be zero. This is where
    // `1.25` is caught, and it is caught exactly: no rounding is involved,
    // only "is there a non-zero digit here".
    if digits.any(|b| b != b'0') {
        return ClipboardEntry::BetweenSteps;
    }
    // `checked_mul`/`checked_add` rather than saturating: a value that
    // overflows is unambiguously above the ceiling, and saying so is better
    // than saturating to `u64::MAX` and then reporting the same thing one
    // branch later.
    let Some(seconds) = whole
        .checked_mul(60)
        .and_then(|s| s.checked_add(tenths * CLIPBOARD_SECONDS_STEP))
    else {
        return ClipboardEntry::AboveCeiling;
    };
    if seconds < MIN_CLIPBOARD_SECONDS {
        return ClipboardEntry::BelowFloor;
    }
    if seconds > MAX_CLIPBOARD_SECONDS {
        return ClipboardEntry::AboveCeiling;
    }
    // In range and, by construction, a multiple of the step -- so
    // `from_seconds`'s clamp is a no-op here and the value the user typed is
    // the value that is stored.
    ClipboardEntry::Accepted(ClearInterval::from_seconds(seconds))
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
    config_dir().map(|dir| dir.join(SETTINGS_FILE_NAME))
}

/// The directory `settings.json` lives in, for the other file this app keeps
/// beside it.
///
/// **Extracted so the triple is spelled once inside this crate**, not so that
/// anything may write anywhere. The only other file in that directory is
/// [`crate::scan_history`]'s, which is deliberately a separate file rather
/// than more fields on [`Settings`] -- see that module for why a record is
/// not a preference. A second copy of `ProjectDirs::from(..)` is exactly how
/// one of the two would come to be written into a directory the other does
/// not read.
///
/// `None` where the platform has no resolvable config directory, in which
/// case nothing is persisted -- the same silent fall-back every other read
/// here makes.
pub fn config_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORGANIZATION, PROJECT_APPLICATION)
        .map(|dirs| dirs.config_dir().to_path_buf())
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
    /// Whether the overlay may raise itself at all -- **for any window, not
    /// only a matched one**.
    ///
    /// **This is the whole of the automatic half of autofill, and it is one
    /// global switch rather than a choice per vault item.** `true` (the
    /// default, and what an older `settings.json` without this field parses
    /// as) means a window that matches an item raises the overlay, and
    /// nothing is typed until the user clicks Fill on it. `false` means a
    /// match does nothing at all on its own, and the fill hotkey
    /// (`CTRL+ALT+B`) is the only way anything is typed.
    ///
    /// **The scope was widened, and the name was not.** As shipped this field
    /// reached exactly one decision -- [`crate::app::match_disposition`], on
    /// the matched path -- so turning the prompt off silenced the overlay for
    /// the apps the user *had* saved a login for and left the no-match card
    /// (design 3a) and the locked card (3b) appearing for the apps they had
    /// not. That is backwards, and it is what a user reported. It is now read
    /// on both paths: [`crate::app::overlay_prompts`] feeds
    /// [`crate::app::disposition`], which suppresses those two cards, and
    /// `match_disposition` goes on answering for the matched one. One switch,
    /// because "disabled in settings" plainly means every pop-up, and because
    /// a second switch for the unmatched cards would leave a user who had
    /// already turned this off still being prompted until they found it.
    ///
    /// **The identifier and the JSON key both keep the historic name**, which
    /// now describes less than the field does. That is deliberate: the key is
    /// in every existing `settings.json`, so leaving it alone means an
    /// upgrading user's choice survives untouched and a downgrade cannot lose
    /// it -- and the name is referenced from four other modules' documentation
    /// besides. The name that a user actually reads is the preferences label,
    /// and that one was corrected; see `prefs_ui::PROMPT_LABEL`.
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
    /// Whether an item's site icon is fetched from the icon service.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as) is the behaviour that has always existed: an item
    /// that answers [`crate::favicon::icon_domain_for`] has that **domain**
    /// -- not its username, not its password, not which account it belongs
    /// to -- requested from `favicon::icon_base_url`'s host, and the result
    /// cached on disk so the same domain is normally asked for once.
    /// `false` means the question is never asked: the loader returns before
    /// it consults `icon_domain_for` at all, and every item wears its
    /// coloured-initials monogram.
    ///
    /// **On by default, unlike [`Self::check_breaches`], and the difference
    /// is who is on the other end rather than which of the two is more
    /// private.** The rule that field records -- a network call keyed on the
    /// user's own data is not ours to decide -- is about a request to a party
    /// with no prior relationship to the vault: `api.pwnedpasswords.com`
    /// learns something it had no way to know. This request goes to the icon
    /// service of *the server the vault is already on* -- the user's own
    /// machine when they self-host, and Bitwarden's when they do not, i.e.
    /// the party that already stores the item the domain came out of. It
    /// re-uses a disclosure relationship the user has already chosen instead
    /// of creating a new one, so making it on their behalf is a
    /// continuation of their choice rather than a decision taken for them.
    ///
    /// That is the argument for the default, and it is deliberately not
    /// "it is on today". What "on today" *does* decide is the direction the
    /// upgrade path must not break: defaulting this to `false` would delete
    /// the icons of every existing user without their asking, and
    /// `an_older_settings_file_without_the_icon_key_loads_as_on` pins that
    /// it does not.
    ///
    /// The residual case is real and is why the switch exists at all: a
    /// cloud user who does not want that service's access log to accumulate
    /// a picture of which domains they hold entries for. `PRIVACY.md` names
    /// this as the request with the most privacy weight in the app.
    pub fetch_icons: bool,
    /// Whether a card's network mark is drawn as the network's own logo, when
    /// an image for it is on disk.
    ///
    /// `false` (the default, and what an older `settings.json` without this
    /// field parses as) is the behaviour that has always existed: every card
    /// wears the drawn wordmark -- `VISA`, `MASTERCARD`, `AMEX` in this app's
    /// own blue pill. So **nobody's display changes on upgrade**, and nothing
    /// is lost on a downgrade either.
    ///
    /// `true` asks [`crate::card_mark`] to look for a logo file for each brand
    /// it draws, in [`crate::brand_mark::search_dirs`]' two directories, and
    /// to draw the image where it finds a usable one. The wordmark remains the
    /// mark everywhere else -- for a brand with no file, and for a file that
    /// was refused -- so turning this on can never leave a row with no mark
    /// on it.
    ///
    /// **Off by default, and the reason is not privacy.** This one reads no
    /// network and no vault: the directories are on the user's own disk. It is
    /// off because there is nothing in either of them until somebody puts
    /// something there -- no image is compiled in and none is distributed with
    /// the source -- so `true` on a fresh install would be a preference that
    /// silently does nothing, which is worse than one a user turns on when
    /// they have the files.
    ///
    /// It is a preference in the ordinary sense as well: these are trademarked
    /// marks, drawn to identify which network a card is on, and a user who
    /// would rather read the plain word is entitled to.
    pub use_brand_logos: bool,
    /// Whether Deskwarden asks GitHub whether a newer Deskwarden exists.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as) is the behaviour that has always existed: one check
    /// at startup and one every `UPDATE_CHECK_INTERVAL` thereafter, against
    /// the releases API of this app's own public repository. `false` means
    /// neither check is made and `updater::check_for_update` is never
    /// called; nothing else about the updater changes, so an update already
    /// found and downloaded still installs.
    ///
    /// **On by default, and this one is not a close call.** The request
    /// carries nothing about the user or their vault beyond what any HTTP
    /// request discloses to any server -- an IP address, and the fact that a
    /// request happened. Weigh that against what defaulting it off would
    /// mean: an app that has quietly stopped telling its user that a
    /// security fix exists. A password manager that silently goes stale is a
    /// worse failure than the disclosure it avoided, and it is a failure the
    /// user cannot notice, because the symptom of a missed update is nothing
    /// happening.
    ///
    /// The switch exists anyway, for the user on a metered or monitored
    /// connection who wants to account for every outbound request, and for
    /// anyone who updates by another route. `PRIVACY.md` names it.
    pub check_for_updates: bool,
    /// Whether a login's TOTP *secret* can be revealed on the details screen.
    ///
    /// `false` (the default, and what an older `settings.json` without this
    /// field parses as) means the read pane shows only the six-digit code and
    /// its countdown, exactly as it always has -- the seed is not drawn at
    /// all, not even masked. `true` adds one masked row under the code, with
    /// the same reveal eye every other secret on that pane carries.
    ///
    /// **Off by default, like [`Self::check_breaches`] and unlike everything
    /// else here.** The code expires in thirty seconds; the seed is the
    /// long-lived shared secret the codes are derived from, so a shoulder
    /// glance at a revealed seed is worth every future code, and a password
    /// at least gets rotated. Putting it on the details screen is a thing to
    /// ask for rather than a thing to discover.
    ///
    /// The row it governs is drawn or not drawn -- never drawn disabled and
    /// never drawn invisible; see `vault_window::detail::draw_detail_read`,
    /// which is its only reader.
    ///
    /// **`seed`, not `secret`, and that is deliberate.** The user-facing
    /// strings say "secret" because that is the word the request used and
    /// what the row is called on screen (`prefs_ui::TOTP_SECRET_LABEL`,
    /// `vault_window::detail::TOTP_SECRET_LABEL`). The PERSISTED key must not
    /// contain it: `tests::mentions_a_secret` is a blunt substring scan over
    /// the whole of `settings.json` for `password`/`session`/`token`/`secret`
    /// /`master key`, and it is the guard that catches a future field
    /// smuggling a real secret onto disk. A preference whose *name* trips it
    /// would have meant loosening that scan to make room, which is exactly
    /// the trade not to make: the field is renamed and the guard stays blunt.
    pub reveal_totp_seed: bool,
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
    /// **The master switch over taking a copied secret back off the
    /// clipboard.**
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as) is the behaviour that has always existed. `false`
    /// means nothing clears: no timer, and no lock, account change or quit
    /// clears either. It governs the other four fields below through
    /// [`clipboard_clearing`], which is the only place that folding happens.
    ///
    /// **This is not the clipboard-history exclusion and must not be confused
    /// with it.** The formats that keep a copied password out of `Win+V` and
    /// off the user's other devices are unconditional, have no setting, and
    /// are argued for at the top of `clipboard.rs`. This switch governs only
    /// the *second* half of that module -- taking the value back afterwards --
    /// which is a convenience/safety trade the user is entitled to make,
    /// because the cost of getting it wrong falls on their own paste.
    ///
    /// On by default, and not a close call: a user who has never opened this
    /// page gets the protection.
    pub clear_clipboard: bool,
    /// Whether locking the vault clears a copied secret.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as). Covers all three ways the vault locks -- the Lock
    /// button, the tray, and the idle timer -- and also `needs_reauth`, the
    /// session being invalidated elsewhere, because that is the same event
    /// arriving from the other direction and the app has **no separate logout
    /// path** for a switch of its own to govern.
    ///
    /// Has no effect while [`Self::clear_clipboard`] is off.
    pub clear_clipboard_on_lock: bool,
    /// Whether switching, adding or removing an account clears a copied
    /// secret.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as). All three mean the user has moved to a different
    /// vault, and a credential from the one they left has no business
    /// surviving the move.
    ///
    /// Has no effect while [`Self::clear_clipboard`] is off.
    pub clear_clipboard_on_account_change: bool,
    /// Whether quitting Deskwarden clears a copied secret.
    ///
    /// `true` (the default, and what an older `settings.json` without this
    /// field parses as). Covers a quit from the tray and a shutdown to install
    /// an update; it cannot cover a crash, a Task Manager kill or a power cut,
    /// and `PRIVACY.md` says so rather than implying otherwise.
    ///
    /// Has no effect while [`Self::clear_clipboard`] is off.
    pub clear_clipboard_on_quit: bool,
    /// **Seconds** after a copy before the timer takes it back, when
    /// [`Self::clear_clipboard`].
    ///
    /// [`DEFAULT_CLIPBOARD_SECONDS`] -- 60, one minute -- is the default and
    /// what an older `settings.json` without this field parses as. That is a
    /// **change in behaviour on upgrade**, from the fixed 45 seconds
    /// `clipboard::DEFAULT_CLEAR_AFTER` used to be, and it is a deliberate one
    /// in the direction of more time to paste rather than less; see
    /// [`MIN_CLIPBOARD_SECONDS`].
    ///
    /// **Seconds on disk, minutes on screen.** The preferences window offers
    /// this as a decimal number of minutes -- `0.5` is 30 seconds -- and the
    /// conversion happens only there, in [`parse_clipboard_minutes`] and
    /// [`ClearInterval::as_minutes_text`]. The persisted value is an integer
    /// so that `settings.json` cannot hold a floating-point rounding artefact
    /// and `clipboard::verdict` cannot end up comparing floats; see
    /// [`DEFAULT_CLIPBOARD_SECONDS`].
    ///
    /// Clamped into [`MIN_CLIPBOARD_SECONDS`]`..=`[`MAX_CLIPBOARD_SECONDS`],
    /// and onto a [`CLIPBOARD_SECONDS_STEP`] boundary, by
    /// [`clamp_clipboard_seconds`] -- which [`ClearInterval::from_seconds`],
    /// the only constructor, runs. Retained while the master switch is off
    /// (the preferences window greys its field out rather than clearing it),
    /// so turning it back on restores the number the user last chose.
    pub clear_clipboard_seconds: u64,
    /// The apps the user pressed *Never for this app* on, in design **3c**'s
    /// save-a-new-login card.
    ///
    /// **Empty is the default, and empty is what an older `settings.json`
    /// without this field parses as** -- this struct carries
    /// `#[serde(default)]`, so a missing key deserializes as `Vec::new()` and
    /// every existing installation upgrades into "nothing has been silenced",
    /// which is exactly the behaviour it has today.
    ///
    /// Each entry is one `crate::app::window_label` answer: normally an
    /// executable file name (`tracker.exe`), and for an unattributable host
    /// frame the window title, because that is the only name such a window has.
    /// Matched whole and ASCII-case-insensitively by
    /// [`crate::app::never_for_app`] -- never as a substring, which is how one
    /// *Never* would come to silence the overlay everywhere.
    ///
    /// **It lives here rather than in the `deskwarden:app-match` convention**,
    /// and that is not a filing preference. That convention is a custom field
    /// *on a vault item*; the entire premise of this control is that there is
    /// no item -- the card it is pressed on is shown precisely because the
    /// vault has nothing for this window. There is nothing to hang the field
    /// on.
    ///
    /// **What it suppresses**: `crate::app::disposition`'s 3a and 3b arms for
    /// this app -- the two states that only ever appear when nothing matched.
    /// It does **not** suppress a fill prompt: an app on this list that later
    /// gains a saved login still raises the matched card. See `disposition`'s
    /// own doc for that argument.
    ///
    /// **Owned by the overlay**, which makes it a fourth writer of this file;
    /// see [`Self::persist_never_save_for_app`].
    ///
    /// No secret is in here -- these are process names -- which is what lets it
    /// past `tests::mentions_a_secret`, the blunt substring scan over the whole
    /// of `settings.json`.
    /// Whether the vault snapshot is persisted to disk, encrypted under a
    /// Windows Hello-sealed key.
    ///
    /// **Off by default, and every behaviour it enables is inert while it is
    /// off** -- with the default settings no file is ever created, which is
    /// asserted against the filesystem rather than against this flag.
    ///
    /// On, the file survives a lock. That is the point of it: it exists to
    /// survive a *restart*, and the vault window locking itself is not the
    /// account going away. It is deleted on log out, on any master-password
    /// re-prompt, and when this setting is turned off; and it expires after
    /// seven days (`vault_disk_cache::EXPIRY_SECS`, which carries the
    /// justification for the number).
    ///
    /// The setting is only offered when Windows Hello is available. There is
    /// deliberately no DPAPI-only variant: the TPM binding is the entire
    /// value of the setting, and a weaker file under copy that promises one
    /// would be a misleading security claim.
    pub cache_vault_to_disk: bool,
    /// Whether the app trusts `bw`'s own crypto implementation rather than
    /// this crate's, for the operations where both exist.
    ///
    /// **`true` (the default), and what an older `settings.json` without this
    /// field parses as** -- today's behaviour, unconditionally.
    ///
    /// **Live, and it is the field with the most behind it.**
    /// [`crate::backend_policy::choose`] is where this field and an account's
    /// server URL become a backend, and that rule -- self-hosted *and* this
    /// setting off, or else `bw serve`; unknown counts as official -- is
    /// stated and table-tested there rather than at any call site. `main`'s
    /// `settle_the_vault_backend` acts on the answer: it fills the vault slot
    /// with a [`crate::rest::backend::RestBackend`] or with the `bw serve`
    /// bridge, and `try_start_backend` refuses to spawn `bw serve` at all on
    /// the direct-REST arm.
    ///
    /// **Turning it back ON deletes the stored vault key.** The key
    /// [`crate::user_key_store`] writes does not expire and cannot be
    /// revoked, so the settlement that stops selecting direct REST is also
    /// what removes it -- see `settle_the_vault_backend` and
    /// `settling_off_direct_rest_deletes_the_stored_vault_key`.
    ///
    /// **Captured at startup and never re-read**, so the change takes effect
    /// on the next launch; Preferences draws the row (see
    /// `prefs_ui::official_crypto_description`), says so in the row itself,
    /// and ghosts it with an explanation on an account that is not
    /// self-hosted.
    pub use_official_bw_crypto: bool,
    pub never_save_for_apps: Vec<String>,
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
            fetch_icons: true,
            use_brand_logos: false,
            check_for_updates: true,
            reveal_totp_seed: false,
            auto_lock_enabled: true,
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
            clear_clipboard: true,
            clear_clipboard_on_lock: true,
            clear_clipboard_on_account_change: true,
            clear_clipboard_on_quit: true,
            clear_clipboard_seconds: DEFAULT_CLIPBOARD_SECONDS,
            cache_vault_to_disk: false,
            use_official_bw_crypto: true,
            never_save_for_apps: Vec::new(),
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
            fetch_icons,
            use_brand_logos,
            check_for_updates,
            reveal_totp_seed,
            auto_lock_enabled,
            auto_lock_minutes,
            clear_clipboard,
            clear_clipboard_on_lock,
            clear_clipboard_on_account_change,
            clear_clipboard_on_quit,
            clear_clipboard_seconds,
            cache_vault_to_disk,
            use_official_bw_crypto,
            // Owned by the overlay, not by the preferences window: it is
            // written by `Self::persist_never_save_for_app` from inside the 3c
            // card, and the copy `main.rs` holds is stale the moment a user
            // presses *Never*. Writing it back from here would resurrect an
            // app the user silenced mid-session.
            never_save_for_apps: _,
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
        on_disk.fetch_icons = *fetch_icons;
        on_disk.use_brand_logos = *use_brand_logos;
        on_disk.check_for_updates = *check_for_updates;
        on_disk.reveal_totp_seed = *reveal_totp_seed;
        on_disk.auto_lock_enabled = *auto_lock_enabled;
        on_disk.auto_lock_minutes = *auto_lock_minutes;
        on_disk.clear_clipboard = *clear_clipboard;
        on_disk.clear_clipboard_on_lock = *clear_clipboard_on_lock;
        on_disk.clear_clipboard_on_account_change = *clear_clipboard_on_account_change;
        on_disk.clear_clipboard_on_quit = *clear_clipboard_on_quit;
        on_disk.clear_clipboard_seconds = *clear_clipboard_seconds;
        on_disk.cache_vault_to_disk = *cache_vault_to_disk;
        on_disk.use_official_bw_crypto = *use_official_bw_crypto;
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

    /// Adds `app` to [`Self::never_save_for_apps`] on disk -- **the fourth
    /// read-modify-write over this file**, and the overlay's own.
    ///
    /// Its own writer rather than a widening of the preferences one, for the
    /// reason [`Self::persist_accounts`] is: the caller is
    /// `crate::app::remember_never_for_app`, which runs from inside a frameless
    /// always-on-top card and holds no `Settings` at all. The only one it could
    /// save is one it invented, and saving that would write the defaults over
    /// every preference the user has ever set.
    ///
    /// **Adds, never replaces**, and re-reads the list from disk first: two
    /// *Never*s in one session must both survive, and so must one made in a
    /// second Deskwarden process. `never_is_added_to_whatever_is_already_on_disk`
    /// pins the first and `persisting_a_never_keeps_every_preference` the rest
    /// of the file.
    ///
    /// **Idempotent**: an app already on the list is not added twice, so a user
    /// who presses *Never* for the same app on two machines that later sync the
    /// same file does not grow a list of duplicates.
    pub fn persist_never_save_for_app(path: &Path, app: &str) -> std::io::Result<()> {
        let mut on_disk = Self::load(path);
        if !on_disk
            .never_save_for_apps
            .iter()
            .any(|a| a.eq_ignore_ascii_case(app))
        {
            on_disk.never_save_for_apps.push(app.to_string());
        }
        on_disk.save(path)
    }

    /// What this settings file means for the vault window's idle timer. All
    /// of the reasoning is in [`auto_lock_policy`]; this is only the lookup.
    pub fn auto_lock(&self) -> AutoLock {
        auto_lock_policy(self.auto_lock_enabled, self.auto_lock_minutes)
    }

    /// What this settings file means for a copied secret's life. All of the
    /// reasoning is in [`clipboard_clearing`]; this is only the lookup.
    #[must_use]
    pub fn clipboard_clearing(&self) -> ClipboardClearing {
        clipboard_clearing(
            self.clear_clipboard,
            self.clear_clipboard_on_lock,
            self.clear_clipboard_on_account_change,
            self.clear_clipboard_on_quit,
            self.clear_clipboard_seconds,
        )
    }

    /// **Every field the clipboard section owns, back at its default, and
    /// nothing else touched.**
    ///
    /// This is what the section's "Reset to default" button does, and it is a
    /// pure function so that "scoped to this section only" is a property a
    /// test can assert rather than a claim about a click handler. The five
    /// fields are spelled out and copied from `Self::default()`; every other
    /// field comes from `self`, so a preference on any other page cannot be
    /// reverted by this button.
    ///
    /// There is deliberately no confirmation dialog in front of it: the button
    /// changes five visible values on the page the user is already looking at,
    /// and setting them back is the same five clicks that got them there.
    #[must_use]
    pub fn with_default_clipboard_clearing(&self) -> Self {
        let defaults = Self::default();
        Self {
            clear_clipboard: defaults.clear_clipboard,
            clear_clipboard_on_lock: defaults.clear_clipboard_on_lock,
            clear_clipboard_on_account_change: defaults.clear_clipboard_on_account_change,
            clear_clipboard_on_quit: defaults.clear_clipboard_on_quit,
            clear_clipboard_seconds: defaults.clear_clipboard_seconds,
            ..self.clone()
        }
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
            // Deliberately the OPPOSITE of this field's own default
            // (`true`), for the reason the line above gives.
            fetch_icons: false,
            // Deliberately the OPPOSITE of this field's own default
            // (`false`), for the reason the lines above give.
            use_brand_logos: true,
            check_for_updates: false,
            reveal_totp_seed: true,
            auto_lock_enabled: true,
            auto_lock_minutes: 5,
            // Every one the OPPOSITE of its own default, for the reason the
            // fields above give: a writer that dropped one would round-trip to
            // the default and look identical to one that kept it. The
            // interval is 150 seconds -- 2.5 minutes, on a step and not the
            // default -- so a writer that dropped it would come back as 60.
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 150,
            // The OPPOSITE of its own default (`false`), for the reason the
            // fields above give.
            cache_vault_to_disk: true,
            use_official_bw_crypto: false,
            never_save_for_apps: vec!["silenced.exe".to_string()],
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
    fn the_disk_cache_is_off_by_default() {
        assert!(!Settings::default().cache_vault_to_disk);
    }

    #[test]
    fn an_older_settings_file_parses_with_the_disk_cache_off() {
        // The partial-file property this struct's `#[serde(default)]`
        // already pins, extended to the new field: a settings.json written
        // by a build that predates this feature must not fail to parse, and
        // must not accidentally arrive with a decrypted vault being written
        // to disk.
        let path = temp_path("partial-disk-cache");
        std::fs::write(
            &path,
            r#"{"keep_backend_running": false, "auto_lock_minutes": 5}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.cache_vault_to_disk);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, 5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn official_bw_crypto_is_trusted_by_default() {
        assert!(Settings::default().use_official_bw_crypto);
    }

    #[test]
    fn an_older_settings_file_parses_trusting_official_bw_crypto() {
        // Same guarantee as `an_older_settings_file_parses_with_the_disk_cache_off`,
        // for the field this pass added: a settings.json written before
        // `use_official_bw_crypto` existed has no such key, and must load as
        // `true` -- today's behaviour, unconditionally -- not as `false`,
        // which `bool`'s own `Default` would otherwise silently give it.
        let path = temp_path("partial-official-bw-crypto");
        std::fs::write(
            &path,
            r#"{"keep_backend_running": false, "auto_lock_minutes": 5}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert!(loaded.use_official_bw_crypto);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, 5);
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
            fetch_icons: false,
            use_brand_logos: true,
            check_for_updates: false,
            reveal_totp_seed: true,
            auto_lock_enabled: false,
            auto_lock_minutes: 42,
            // Every one the OPPOSITE of its own default, for the reason the
            // fields above give: a writer that dropped one would round-trip to
            // the default and look identical to one that kept it. The
            // interval is 150 seconds -- 2.5 minutes, on a step and not the
            // default -- so a writer that dropped it would come back as 60.
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 150,
            cache_vault_to_disk: true,
            use_official_bw_crypto: false,
            never_save_for_apps: vec!["silenced.exe".to_string()],
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

    /// Icon fetching is the one network preference here that is ON unless it
    /// is turned off, so this is a statement about the default and not a
    /// restatement of `Default`. The argument is on the field itself: the
    /// request goes to the icon service of the server the vault is already
    /// on, so it re-uses a disclosure relationship the user has already
    /// chosen rather than creating a new one, which is precisely what
    /// `check_breaches`'s third-party call does do.
    ///
    /// Asserted beside `check_breaches` deliberately: the two are the app's
    /// two vault-keyed network calls and they default OPPOSITE ways, so a
    /// change that quietly made them agree fails here whichever way it went.
    #[test]
    fn icon_fetching_is_on_by_default_and_breach_checking_is_not() {
        assert!(Settings::default().fetch_icons);
        assert!(!Settings::default().check_breaches);
        // ...and not merely present in the in-memory default: a fresh install
        // has no file at all, and that path must land on `true` too.
        assert!(Settings::load(&temp_path("icons-absent")).fetch_icons);
    }

    /// The upgrade path, which for this field is the direction that matters
    /// and is the OPPOSITE of `check_breaches`'s: a `settings.json` written
    /// before the field existed must not read as opted OUT, or upgrading
    /// deletes the icons of every existing user without their asking.
    #[test]
    fn an_older_settings_file_without_the_icon_key_loads_as_on() {
        let path = temp_path("icons-older-file");
        let older = br#"{"keep_backend_running": false, "check_breaches": true, "auto_lock_minutes": 9}"#;
        // The premise, asserted rather than assumed: a fixture that happened
        // to carry the key would make the rest of this test vacuous.
        assert!(
            !std::str::from_utf8(older).unwrap().contains("fetch_icons"),
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
            loaded.fetch_icons,
            "upgrading turned icon fetching off for a user who never asked, so every item in \
             their vault silently lost its icon"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The same upgrade question for `use_brand_logos`, whose answer is the
    /// other way round and for the other reason: this field defaults OFF, so
    /// an older `settings.json` must read as off and **nobody's card rows
    /// change appearance on upgrade**. A brand logo appearing on a user's
    /// vault because they installed a new version is exactly the surprise the
    /// default is chosen to avoid.
    #[test]
    fn an_older_settings_file_without_the_brand_logo_key_loads_as_off() {
        let path = temp_path("brand-logos-older-file");
        let older = br#"{"keep_backend_running": false, "fetch_icons": true, "auto_lock_minutes": 9}"#;
        assert!(
            !std::str::from_utf8(older).unwrap().contains("use_brand_logos"),
            "the fixture names the key, so it is not an older file"
        );
        std::fs::write(&path, older).unwrap();
        let loaded = Settings::load(&path);
        // The premise that the file was read at all, rather than falling back
        // to `Settings::default()` wholesale.
        assert!(!loaded.keep_backend_running, "the file was not parsed: {loaded:?}");
        assert_eq!(loaded.auto_lock_minutes, 9);
        assert!(
            !loaded.use_brand_logos,
            "upgrading turned brand logos on for a user who never asked for them"
        );
        // ...and the in-memory default and the no-file-at-all path agree with
        // it, which is the whole of "off by default".
        assert!(!Settings::default().use_brand_logos);
        assert!(!Settings::load(&temp_path("brand-logos-absent")).use_brand_logos);
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard the two tests below document**, for
    /// `use_brand_logos`: `persist_preferences` destructures exhaustively, so
    /// the field cannot go unnamed -- but binding it and never assigning
    /// `on_disk.use_brand_logos` compiles, and a preference that silently
    /// refuses to persist is indistinguishable from one that does not work.
    ///
    /// Both directions, because a writer that always wrote the default
    /// (`false`) would pass a one-way test.
    #[test]
    fn the_brand_logo_toggle_survives_persist_preferences() {
        let path = temp_path("brand-logos-persist");
        let _ = std::fs::remove_file(&path);
        assert!(!Settings::load(&path).use_brand_logos, "the premise: it starts off");
        Settings { use_brand_logos: true, ..Settings::default() }
            .persist_preferences(&path)
            .expect("the write succeeded");
        assert!(
            Settings::load(&path).use_brand_logos,
            "turning brand logos on did not survive the write"
        );
        Settings { use_brand_logos: false, ..Settings::default() }
            .persist_preferences(&path)
            .expect("the write succeeded");
        assert!(!Settings::load(&path).use_brand_logos, "and back off again");
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard `the_breach_toggle_survives_persist_preferences`
    /// documents, for `fetch_icons`.** `persist_preferences` destructures
    /// exhaustively, so a new field cannot go unnamed -- but binding it and
    /// never assigning `on_disk.fetch_icons` compiles, and that mutant has
    /// survived the whole suite in this repo before.
    ///
    /// Both directions, because a writer that always wrote the default
    /// (`true`) would pass a one-way test.
    #[test]
    fn the_icon_toggle_survives_persist_preferences() {
        let path = temp_path("prefs-icons");
        Settings::default().save(&path).unwrap();
        assert!(Settings::load(&path).fetch_icons, "the premise: it starts on");

        Settings { fetch_icons: false, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            !loaded.fetch_icons,
            "the icon setting was dropped by persist_preferences, so turning it off lasts only \
             until the app is restarted -- and the domains start going out again"
        );
        // The neighbours it is destructured beside are untouched, so this is
        // not satisfied by a writer that clobbers the file with something else.
        assert!(loaded.keep_backend_running);
        assert!(loaded.prompt_on_match);
        assert!(!loaded.check_breaches);
        assert!(loaded.auto_lock_enabled);

        // ...and back on again, so "always writes false" fails too.
        Settings { fetch_icons: true, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(Settings::load(&path).fetch_icons);
        let _ = std::fs::remove_file(&path);
    }

    /// The field reaches the file under its own name, the way
    /// `the_breach_toggle_round_trips_through_settings_json_under_its_own_name`
    /// pins its own. `settings_round_trip_through_disk` compares whole
    /// structs, which a field renamed on both sides at once would satisfy.
    #[test]
    fn the_icon_toggle_round_trips_through_settings_json_under_its_own_name() {
        let path = temp_path("icons-round-trip");
        let written = Settings { fetch_icons: false, ..Settings::default() };
        // The value written disagrees with the default, so a reader that
        // ignored the file entirely would fail here.
        assert!(written.fetch_icons != Settings::default().fetch_icons);
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("fetch_icons"), "not in the file at all: {text}");
        assert_eq!(Settings::load(&path), written);
        assert!(!Settings::load(&path).fetch_icons);
        let _ = std::fs::remove_file(&path);
    }

    /// The update check is on unless it is turned off, and the reason is the
    /// asymmetry rather than the arithmetic: the request discloses an IP and
    /// nothing else, and the failure mode of defaulting it off is an app that
    /// has silently stopped mentioning security fixes.
    #[test]
    fn the_update_check_is_on_by_default() {
        assert!(Settings::default().check_for_updates);
        // ...and not merely present in the in-memory default: a fresh install
        // has no file at all, and that path must land on `true` too.
        assert!(Settings::load(&temp_path("updates-absent")).check_for_updates);
    }

    /// The upgrade path, in the direction that matters for this field: a
    /// `settings.json` written before it existed must not read as opted OUT,
    /// or upgrading is the last update that user is ever told about.
    #[test]
    fn an_older_settings_file_without_the_update_key_loads_as_on() {
        let path = temp_path("updates-older-file");
        let older = br#"{"keep_backend_running": false, "check_breaches": true, "auto_lock_minutes": 4}"#;
        // The premise, asserted rather than assumed: a fixture that happened
        // to carry the key would make the rest of this test vacuous.
        assert!(
            !std::str::from_utf8(older).unwrap().contains("check_for_updates"),
            "the fixture names the key, so it is not an older file"
        );
        std::fs::write(&path, older).unwrap();
        let loaded = Settings::load(&path);
        // And the premise that the file was read at all, rather than falling
        // back to `Settings::default()` wholesale.
        assert!(!loaded.keep_backend_running, "the file was not parsed: {loaded:?}");
        assert_eq!(loaded.auto_lock_minutes, 4);
        assert!(
            loaded.check_for_updates,
            "upgrading turned the update check off for a user who never asked, so this is the \
             last release they will hear about"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard the three tests above document**, for
    /// `check_for_updates`: `persist_preferences` destructures exhaustively,
    /// but binding a field and never assigning `on_disk.check_for_updates`
    /// compiles, and that mutant has survived this suite before.
    ///
    /// Both directions, because a writer that always wrote the default
    /// (`true`) would pass a one-way test.
    #[test]
    fn the_update_toggle_survives_persist_preferences() {
        let path = temp_path("prefs-updates");
        Settings::default().save(&path).unwrap();
        assert!(Settings::load(&path).check_for_updates, "the premise: it starts on");

        Settings { check_for_updates: false, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            !loaded.check_for_updates,
            "the update setting was dropped by persist_preferences, so turning it off lasts \
             only until the app is restarted"
        );
        // The neighbours it is destructured beside are untouched, so this is
        // not satisfied by a writer that clobbers the file with something else.
        assert!(loaded.keep_backend_running);
        assert!(loaded.fetch_icons);
        assert!(!loaded.check_breaches);
        assert!(loaded.auto_lock_enabled);

        // ...and back on again, so "always writes false" fails too.
        Settings { check_for_updates: true, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(Settings::load(&path).check_for_updates);
        let _ = std::fs::remove_file(&path);
    }

    /// The field reaches the file under its own name, the way every other
    /// preference here is pinned. `settings_round_trip_through_disk` compares
    /// whole structs, which a field renamed on both sides at once would
    /// satisfy.
    #[test]
    fn the_update_toggle_round_trips_through_settings_json_under_its_own_name() {
        let path = temp_path("updates-round-trip");
        let written = Settings { check_for_updates: false, ..Settings::default() };
        // The value written disagrees with the default, so a reader that
        // ignored the file entirely would fail here.
        assert!(written.check_for_updates != Settings::default().check_for_updates);
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("check_for_updates"), "not in the file at all: {text}");
        assert_eq!(Settings::load(&path), written);
        assert!(!Settings::load(&path).check_for_updates);
        let _ = std::fs::remove_file(&path);
    }

    /// The TOTP-secret row's preference is off unless it is turned on -- the
    /// second such preference here, and for a stronger reason than the first:
    /// a revealed seed does not expire and does not rotate.
    #[test]
    fn revealing_the_totp_secret_is_off_by_default() {
        assert!(!Settings::default().reveal_totp_seed);
        // ...and not merely absent from the in-memory default: a fresh
        // install has no file at all, and that path must land on `false` too.
        assert!(!Settings::load(&temp_path("totp-secret-absent")).reveal_totp_seed);
    }

    /// The upgrade path. A `settings.json` written before this field existed
    /// must not read as "show me the seed".
    #[test]
    fn an_older_settings_file_without_the_totp_secret_key_loads_as_off() {
        let path = temp_path("totp-secret-older-file");
        let older = br#"{"keep_backend_running": false, "check_breaches": true, "auto_lock_minutes": 7}"#;
        // The premise, asserted rather than assumed: a fixture that happened
        // to carry the key would make the rest of this test vacuous.
        assert!(
            !std::str::from_utf8(older).unwrap().contains("reveal_totp_seed"),
            "the fixture names the key, so it is not an older file"
        );
        std::fs::write(&path, older).unwrap();
        let loaded = Settings::load(&path);
        // And the premise that the file was read at all, rather than falling
        // back to `Settings::default()` wholesale -- two fields that disagree
        // with the defaults.
        assert!(!loaded.keep_backend_running, "the file was not parsed: {loaded:?}");
        assert_eq!(loaded.auto_lock_minutes, 7);
        assert!(
            !loaded.reveal_totp_seed,
            "upgrading turned on a details-screen row that shows the user's TOTP seeds"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// **The same hazard `the_breach_toggle_survives_persist_preferences`
    /// documents, for `reveal_totp_seed`.** The destructure in
    /// `persist_preferences` names every field, so a new one cannot go
    /// unbound -- but binding it and never writing
    /// `on_disk.reveal_totp_seed` compiles, warns at most, and that exact
    /// mutant has survived this whole suite before. This test is the only
    /// thing that reaches that assignment.
    ///
    /// Both directions, because a writer that always wrote the default
    /// (`false`) would pass a one-way test.
    #[test]
    fn the_totp_secret_toggle_survives_persist_preferences() {
        let path = temp_path("prefs-totp-secret");
        Settings::default().save(&path).unwrap();
        assert!(
            !Settings::load(&path).reveal_totp_seed,
            "the premise: it starts off"
        );

        Settings { reveal_totp_seed: true, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let loaded = Settings::load(&path);
        assert!(
            loaded.reveal_totp_seed,
            "the TOTP-secret setting was dropped by persist_preferences, so turning it on              lasts only until the app is restarted"
        );
        // The neighbours it is destructured beside are untouched, so this is
        // not satisfied by a writer that clobbers the file with something else.
        assert!(loaded.keep_backend_running);
        assert!(loaded.prompt_on_match);
        assert!(!loaded.check_breaches);
        assert!(loaded.auto_lock_enabled);

        // ...and back off again, so "always writes true" fails too. The
        // premise for this half is the assertion above: it is provably on
        // before this write turns it off.
        Settings { reveal_totp_seed: false, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        assert!(!Settings::load(&path).reveal_totp_seed);
        let _ = std::fs::remove_file(&path);
    }

    /// The field reaches the file under its own name, the way
    /// `the_breach_toggle_round_trips_through_settings_json_under_its_own_name`
    /// pins its neighbour's.
    #[test]
    fn the_totp_secret_toggle_round_trips_through_settings_json_under_its_own_name() {
        let path = temp_path("totp-secret-round-trip");
        let written = Settings { reveal_totp_seed: true, ..Settings::default() };
        // The value written disagrees with the default, so a reader that
        // ignored the file entirely would fail here.
        assert!(written.reveal_totp_seed != Settings::default().reveal_totp_seed);
        written.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("reveal_totp_seed"), "not in the file at all: {text}");
        // **And the key does not trip the file-content guard.** See the
        // field's doc: `mentions_a_secret` is a blunt substring scan and the
        // preference is named `seed` rather than `secret` so that it stays
        // blunt. Renaming the field back would fail here rather than forcing
        // someone to widen the scan.
        assert!(
            !mentions_a_secret(&text),
            "the preference key trips the NO SECRETS guard: {text}"
        );
        assert_eq!(Settings::load(&path), written);
        assert!(Settings::load(&path).reveal_totp_seed);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_geometry_round_trips_through_disk_with_the_rest_of_the_file() {
        let path = temp_path("geometry-round-trip");
        let written = Settings {
            keep_backend_running: false,
            prompt_on_match: true,
            check_breaches: true,
            fetch_icons: false,
            use_brand_logos: true,
            check_for_updates: false,
            reveal_totp_seed: true,
            auto_lock_enabled: true,
            auto_lock_minutes: 5,
            // Every one the OPPOSITE of its own default, for the reason the
            // fields above give: a writer that dropped one would round-trip to
            // the default and look identical to one that kept it. The
            // interval is 150 seconds -- 2.5 minutes, on a step and not the
            // default -- so a writer that dropped it would come back as 60.
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 150,
            cache_vault_to_disk: true,
            use_official_bw_crypto: false,
            never_save_for_apps: vec!["silenced.exe".to_string()],
            vault_window: Some(WindowGeometry { x: 100, y: 60, width: 1400, height: 900 }),
            accounts: Vec::new(),
            active_account: None,
            accounts_unreadable: false,
        };
        written.save(&path).unwrap();
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    /// **An older `settings.json` without the never-list parses as an empty
    /// one**, which is the upgrade direction that matters: every existing
    /// installation must come up with nothing silenced, because nothing has
    /// been silenced.
    ///
    /// The inverse would be the worst failure this field can have -- an
    /// unreadable or absent key that defaulted to "silenced" would switch the
    /// overlay off for an app the user never pressed anything on, and the only
    /// evidence would be a card that stopped appearing.
    #[test]
    fn an_older_settings_file_without_the_never_key_loads_as_empty() {
        let path = temp_path("no-never-key");
        // A file with every OTHER key, so the absence being tested is the
        // never-list's and not the file's.
        std::fs::write(
            &path,
            r#"{"keep_backend_running":false,"prompt_on_match":false,"auto_lock_minutes":9}"#,
        )
        .unwrap();

        let loaded = Settings::load(&path);
        assert!(
            loaded.never_save_for_apps.is_empty(),
            "an older settings file loaded with {:?} silenced, which the user never asked for",
            loaded.never_save_for_apps
        );
        // The control: the file really was read, so the emptiness above is the
        // missing key and not a failed parse falling back to the defaults.
        assert!(!loaded.keep_backend_running, "control: the file was not read at all");
        assert_eq!(loaded.auto_lock_minutes, 9, "control: the file was not read at all");
        let _ = std::fs::remove_file(&path);
    }

    /// A `Never` is **added** to whatever is already on disk, and the same app
    /// twice is still one entry.
    ///
    /// Both halves matter for the same reason: this writer re-reads the file,
    /// so two `Never`s in one session -- or one from a second Deskwarden
    /// process -- must both survive, while a user who presses it twice must
    /// not grow the list.
    #[test]
    fn never_is_added_to_whatever_is_already_on_disk() {
        let path = temp_path("never-adds");
        Settings::persist_never_save_for_app(&path, "tracker.exe").unwrap();
        Settings::persist_never_save_for_app(&path, "ledgerline.exe").unwrap();
        assert_eq!(
            Settings::load(&path).never_save_for_apps,
            vec!["tracker.exe".to_string(), "ledgerline.exe".to_string()],
            "the second `Never` replaced the first instead of joining it"
        );

        // Idempotent, including across a spelling Windows varies on its own.
        Settings::persist_never_save_for_app(&path, "tracker.exe").unwrap();
        Settings::persist_never_save_for_app(&path, "TRACKER.EXE").unwrap();
        assert_eq!(
            Settings::load(&path).never_save_for_apps,
            vec!["tracker.exe".to_string(), "ledgerline.exe".to_string()],
            "pressing `Never` again grew the list, so a user who presses it on every launch \
             accumulates a settings file of duplicates"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Recording a `Never` keeps every preference, the geometry and the
    /// accounts -- the same claim every other writer of this file carries, and
    /// it is the reason this is a read-modify-write rather than a save.
    ///
    /// It matters more here than for the others: `app::remember_never_for_app`
    /// runs from inside a frameless always-on-top card and holds no `Settings`
    /// at all, so a whole-struct save from that call site would write the
    /// defaults over the user's account list.
    #[test]
    fn persisting_a_never_keeps_every_preference() {
        let path = temp_path("never-preserves");
        Settings {
            keep_backend_running: false,
            auto_lock_minutes: 7,
            clear_clipboard: false,
            vault_window: Some(WindowGeometry { x: 3, y: 4, width: 900, height: 600 }),
            ..Settings::default()
        }
        .save(&path)
        .unwrap();

        Settings::persist_never_save_for_app(&path, "tracker.exe").unwrap();

        let back = Settings::load(&path);
        assert_eq!(back.never_save_for_apps, vec!["tracker.exe".to_string()]);
        assert!(!back.keep_backend_running, "the never-list writer reset a preference");
        assert_eq!(back.auto_lock_minutes, 7, "the never-list writer reset the idle timer");
        assert!(!back.clear_clipboard, "the never-list writer reset the clipboard switch");
        assert_eq!(
            back.vault_window,
            Some(WindowGeometry { x: 3, y: 4, width: 900, height: 600 }),
            "the never-list writer threw the vault window's geometry away"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// And the other direction: a preferences save does **not** carry the
    /// never-list, because the copy it is saving from is stale in that field.
    ///
    /// `main.rs` loads `Settings` once at startup and never refreshes it, so
    /// its `never_save_for_apps` is whatever was on disk then. Writing that
    /// back from the preferences window would resurrect an app the user
    /// silenced ten minutes ago -- the same defect `persist_accounts` exists
    /// to prevent for the account list.
    #[test]
    fn persisting_preferences_from_a_stale_copy_keeps_the_never_list() {
        let path = temp_path("never-survives-prefs");
        Settings::persist_never_save_for_app(&path, "tracker.exe").unwrap();

        // The stale copy: loaded before the `Never` existed, so its list is
        // empty, and it is now changing an unrelated preference.
        let stale = Settings { keep_backend_running: false, ..Settings::default() };
        assert!(stale.never_save_for_apps.is_empty(), "control: the stale copy must be empty");
        stale.persist_preferences(&path).unwrap();

        let back = Settings::load(&path);
        assert_eq!(
            back.never_save_for_apps,
            vec!["tracker.exe".to_string()],
            "a preferences save wiped the never-list, so an app the user silenced started \
             raising the overlay again the next time they changed any setting at all"
        );
        assert!(!back.keep_backend_running, "control: the preferences save did not happen");
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

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. The same shape `foreground.rs` walks with,
    /// deliberately exact rather than a `starts_with`: a whole module written
    /// on one line is not a module opener as far as this walk is concerned.
    fn is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// **Another module's source with its gated test modules cut out**, and
    /// how many were cut. A copy of `foreground.rs`'s walk (`ec74c74`); this
    /// crate has no test-only module the two could share without a `mod`
    /// declaration in a file this change has no business touching.
    ///
    /// The reason the guard below reads this rather than the raw
    /// `include_str!` bytes. That guard is a **presence** pin -- `main.rs`
    /// must spell the `ProjectDirs` triple that `default_path` duplicates --
    /// and for a presence pin the whole file is the LOOSE side, not the
    /// strict one: a fixture in one of `main.rs`'s own test modules spelling
    /// the triple would satisfy it after production had stopped spelling it,
    /// and the guard would sit green over exactly the drift it exists to
    /// catch. (That is the mirror image of the exact-count false fire
    /// `ec74c74` fixed, and the opposite of a zero-count pin, where reading
    /// raw-and-whole IS the strict side and must be left alone -- see
    /// `item_list.rs`.)
    ///
    /// A walk that skips each gated module, rather than "everything above the
    /// first `cfg(test)` gate" -- and the difference is **measured, not
    /// assumed, because overselling it would be its own defect**. `main.rs`
    /// today has two gated modules, at lines 6119 and 16984, and below the
    /// first gate exactly six non-blank lines are outside a gated module: the
    /// doc comment attached to the second one. So a split at the first gate
    /// would read the same production, and both pins here sit at lines 132 and
    /// 229 anyway. The walk is used regardless, for two reasons that do not
    /// depend on today's layout: `main.rs` has no guard of its own forbidding
    /// production below its first gate (`settings.rs` and `vault_window::mod`
    /// both do, which is why THEY stay tidy), so the day a helper is added
    /// after the tests a split would start silently discarding it; and it is
    /// the shape `foreground.rs` already reads six files with.
    ///
    /// So each gated module is skipped instead: a line that is exactly
    /// a `cfg(test)` gate followed immediately by a column-0 module opener starts
    /// a skip that runs to the next column-0 `}`. Inside a module every item
    /// is indented, so that brace is the module's own.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF checkout alike. This
    /// repository stores LF blobs and only `core.autocrlf=true` makes the
    /// working tree CRLF, so a needle carrying a carriage return would match
    /// nothing on a plain checkout -- green, and reading nothing.
    fn production_half(source: &str) -> (String, usize) {
        let mut kept: Vec<&str> = Vec::new();
        let mut cut = 0usize;
        let mut gated = false;
        let mut skipping = false;
        for line in source.lines() {
            if skipping {
                if line == "}" {
                    skipping = false;
                }
                continue;
            }
            if gated && is_module_opener(line) {
                // The `cfg(test)` gate line itself was pushed on the previous
                // turn; it belongs to the module being cut.
                kept.pop();
                skipping = true;
                cut += 1;
                gated = false;
                continue;
            }
            gated = line.trim() == concat!("#[cfg(", "test)]");
            kept.push(line);
        }
        assert!(
            !skipping,
            "a test module was opened and never closed by a column-0 brace, so the rest of the \
             file was dropped and every needle counted over this reads nothing"
        );
        (kept.join("\n"), cut)
    }

    #[test]
    fn the_config_path_still_matches_the_one_main_resolves() {
        // `default_path` duplicates `main.rs`'s `ProjectDirs` triple because
        // `vault_window::run` has no settings path in its signature (see
        // that function's doc). A source-text guard, not a comment: if
        // `main.rs` ever changes the triple or the file name, this crate
        // would start writing window geometry into a second settings file
        // that nothing reads, and every test here would stay green.
        //
        // **Over `main.rs`'s production half, not its whole source.** These
        // are presence pins over ANOTHER module's file, and the whole file is
        // the loose side of that: see `production_half`.
        let (main_rs, cut) = production_half(include_str!("main.rs"));
        assert!(
            cut > 0,
            "no gated test module was cut out of `main.rs`, so this guard is still satisfiable \
             by a fixture in one of that file's test modules rather than by its code"
        );

        // **Positive control on the cut, not only on the needles.** Two
        // claims, and the two `contains` below prove neither: that the walk
        // removes a gated test module, and that it removes ONLY that -- in
        // particular not production sitting BELOW a test module, which is
        // what a split at the first `cfg(test)` gate would do to `main.rs`. So
        // the survivor is asserted by name, and the raw count is asserted
        // first so that a fixture that stopped spelling the needle cannot
        // make the cut look like it worked.
        let interleaved = concat!(
            "fn a() { let p = ProjectDirs::from(\"dev\", \"X\", \"X\"); }\n",
            "#[cfg(", "test)]\n",
            "mod fixtures {\n",
            "    const SAMPLE: &str = \"ProjectDirs::from(\";\n",
            "}\n",
            "fn b() { let _ = SURVIVOR; }\n"
        );
        assert_eq!(
            interleaved.matches("ProjectDirs::from(").count(),
            2,
            "control: the fixture no longer spells the needle, so the cut below proves nothing"
        );
        let (cut_fixture, cuts) = production_half(interleaved);
        assert_eq!(cuts, 1, "the walk did not find the gated test module");
        assert_eq!(
            cut_fixture.matches("ProjectDirs::from(").count(),
            1,
            "the walk did not remove the occurrence inside the test module"
        );
        assert!(
            cut_fixture.contains("SURVIVOR"),
            "the walk threw away production below the test module, which is exactly what a \
             split at the first `cfg(test)` gate would have done to `main.rs`"
        );

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

    // ------------------------------------------------------------------
    // Clearing a copied secret off the clipboard
    // ------------------------------------------------------------------

    /// **Every one of the five fields is on by default**, so a user who has
    /// never opened the page gets the protection. Spelled out one at a time
    /// rather than compared against a constructed struct: a test that built
    /// its expectation out of `Settings::default()` would pass whatever the
    /// defaults were.
    #[test]
    fn clipboard_clearing_is_entirely_on_by_default() {
        let s = Settings::default();
        assert!(s.clear_clipboard, "the master switch");
        assert!(s.clear_clipboard_on_lock);
        assert!(s.clear_clipboard_on_account_change);
        assert!(s.clear_clipboard_on_quit);
        assert_eq!(s.clear_clipboard_seconds, 60, "one minute, in seconds");
        // ...and the derived value agrees with the five fields, so the
        // defaults above are not merely stored but in effect.
        assert_eq!(
            s.clipboard_clearing(),
            ClipboardClearing {
                timer: Some(ClearInterval::from_seconds(60)),
                on_lock: true,
                on_account_change: true,
                on_quit: true,
            }
        );
    }

    /// **An older `settings.json` without any of the five keys loads as all
    /// on and one minute** -- the upgrade path, and the direction it must not
    /// break. A file predating this section describes a user who was getting
    /// clearing at 45 seconds; reading it as "off" would silently withdraw a
    /// protection they never asked to lose.
    #[test]
    fn an_older_settings_file_without_the_clipboard_keys_loads_as_all_on() {
        let path = temp_path("clipboard-absent");
        let older =
            br#"{"keep_backend_running": false, "check_breaches": true, "auto_lock_minutes": 9}"#;
        // The premise: the keys really are absent, so this is testing the
        // absence and not a file that happens to say `true`.
        for key in [
            "clear_clipboard",
            "clear_clipboard_on_lock",
            "clear_clipboard_on_account_change",
            "clear_clipboard_on_quit",
            "clear_clipboard_seconds",
        ] {
            assert!(
                !std::str::from_utf8(older).unwrap().contains(key),
                "{key} is in the fixture, so this proves nothing about its absence"
            );
        }
        std::fs::write(&path, older).unwrap();
        let loaded = Settings::load(&path);
        assert!(loaded.clear_clipboard);
        assert!(loaded.clear_clipboard_on_lock);
        assert!(loaded.clear_clipboard_on_account_change);
        assert!(loaded.clear_clipboard_on_quit);
        assert_eq!(loaded.clear_clipboard_seconds, 60);
        // The keys that ARE in the file still landed, so the file was really
        // parsed rather than falling back to `Settings::default()` wholesale.
        assert!(!loaded.keep_backend_running);
        assert!(loaded.check_breaches);
        assert_eq!(loaded.auto_lock_minutes, 9);
        let _ = std::fs::remove_file(&path);
    }

    /// **The master switch off means nothing clears at all** -- not the timer,
    /// and not one of the three triggers, whatever their own fields say.
    ///
    /// This is the enable/disable logic the preferences window paints, tested
    /// as a pure function of the settings rather than by driving a window.
    #[test]
    fn the_master_switch_off_silences_every_trigger_and_the_timer() {
        let off = Settings {
            clear_clipboard: false,
            // All three children left ON, so the assertions below are about
            // the master switch overriding them rather than about them being
            // off already.
            clear_clipboard_on_lock: true,
            clear_clipboard_on_account_change: true,
            clear_clipboard_on_quit: true,
            clear_clipboard_seconds: 120,
            ..Settings::default()
        };
        let clearing = off.clipboard_clearing();
        assert_eq!(clearing.timer, None, "the timer survived the master switch");
        assert!(!clearing.on_lock);
        assert!(!clearing.on_account_change);
        assert!(!clearing.on_quit);
        assert!(!clearing.interval_is_live(), "the interval control is still enabled");
        assert!(!clearing.clears_at_all());

        // The pair: the identical settings with the master switch ON have all
        // four live, so every assertion above is about the master switch and
        // not about a struct that is inert whatever it is handed.
        let on = Settings { clear_clipboard: true, ..off.clone() };
        let clearing = on.clipboard_clearing();
        assert_eq!(clearing.timer, Some(ClearInterval::from_seconds(120)));
        assert!(clearing.on_lock);
        assert!(clearing.on_account_change);
        assert!(clearing.on_quit);
        assert!(clearing.interval_is_live());
        assert!(clearing.clears_at_all());
        // And the retained fields are retained: turning the master switch off
        // did not zero the children on the way, so turning it back on gives
        // the user the values they chose.
        assert!(off.clear_clipboard_on_lock);
        assert_eq!(off.clear_clipboard_seconds, 120);
    }

    /// **Each trigger switch governs its own trigger and no other**, so a
    /// wiring mistake that pointed two of them at one field cannot pass.
    #[test]
    fn each_trigger_switch_governs_exactly_one_trigger() {
        let cases: [(&str, Settings, bool, bool, bool); 3] = [
            (
                "only lock",
                Settings {
                    clear_clipboard_on_account_change: false,
                    clear_clipboard_on_quit: false,
                    ..Settings::default()
                },
                true,
                false,
                false,
            ),
            (
                "only account change",
                Settings {
                    clear_clipboard_on_lock: false,
                    clear_clipboard_on_quit: false,
                    ..Settings::default()
                },
                false,
                true,
                false,
            ),
            (
                "only quit",
                Settings {
                    clear_clipboard_on_lock: false,
                    clear_clipboard_on_account_change: false,
                    ..Settings::default()
                },
                false,
                false,
                true,
            ),
        ];
        for (what, settings, lock, account, quit) in &cases {
            let clearing = settings.clipboard_clearing();
            assert_eq!(clearing.on_lock, *lock, "{what}: on_lock");
            assert_eq!(clearing.on_account_change, *account, "{what}: on_account_change");
            assert_eq!(clearing.on_quit, *quit, "{what}: on_quit");
            // The timer is untouched by any of the three: they are the
            // triggers, not the clock.
            assert!(clearing.interval_is_live(), "{what}: a trigger switch silenced the timer");
            assert!(clearing.clears_at_all(), "{what}");
        }
    }

    /// **All three triggers off is still not "nothing clears"** -- the timer
    /// remains, and `clears_at_all` says so. That distinction is the reason
    /// the method exists separately from `interval_is_live`.
    #[test]
    fn with_every_trigger_off_the_timer_still_clears() {
        let s = Settings {
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            ..Settings::default()
        };
        let clearing = s.clipboard_clearing();
        assert!(clearing.clears_at_all(), "the timer is still a thing that clears");
        assert!(clearing.interval_is_live());
        // The pair: the ONLY state in which nothing clears is the master
        // switch being off.
        let none = Settings { clear_clipboard: false, ..s };
        assert!(!none.clipboard_clearing().clears_at_all());
    }

    /// **The clamp's three jobs, at absolute values rather than re-derived
    /// from the constants.** A test that recomputed its expectation from
    /// `MIN_CLIPBOARD_SECONDS` would still pass if that constant moved.
    #[test]
    fn the_interval_clamp_applies_a_floor_a_ceiling_and_a_step() {
        // Floor.
        assert_eq!(clamp_clipboard_seconds(0), 30, "an instant clear is unreachable");
        assert_eq!(clamp_clipboard_seconds(1), 30);
        assert_eq!(clamp_clipboard_seconds(29), 30);
        // Ceiling.
        assert_eq!(clamp_clipboard_seconds(3601), 3600);
        assert_eq!(clamp_clipboard_seconds(u64::MAX), 3600);
        // Step: nearest multiple of six, ties downward.
        assert_eq!(clamp_clipboard_seconds(30), 30, "already on a step");
        assert_eq!(clamp_clipboard_seconds(60), 60);
        assert_eq!(clamp_clipboard_seconds(79), 78);
        assert_eq!(clamp_clipboard_seconds(80), 78);
        assert_eq!(clamp_clipboard_seconds(81), 78, "the tie goes to the shorter interval");
        assert_eq!(clamp_clipboard_seconds(82), 84);
        // And the result is always in range and on a step, for every value
        // across the range plus the extremes -- the property, not samples.
        for seconds in (0..4000).chain([u64::MAX]) {
            let got = clamp_clipboard_seconds(seconds);
            assert!((30..=3600).contains(&got), "{seconds} clamped out of range to {got}");
            assert_eq!(got % 6, 0, "{seconds} clamped to {got}, which is not on a step");
        }
    }

    /// **A `ClearInterval` can never be zero, whatever it is built from.**
    /// The type is what makes an instant clear unreachable rather than a case
    /// something downstream remembers to check, so this is the pin on that
    /// claim.
    #[test]
    fn no_interval_can_be_built_that_would_clear_instantly() {
        for seconds in (0..200).chain([u64::MAX, 3600, 3601]) {
            let interval = ClearInterval::from_seconds(seconds);
            assert!(
                interval.seconds() >= 30,
                "from_seconds({seconds}) produced {}s, which is under the floor",
                interval.seconds()
            );
            assert!(!interval.duration().is_zero());
        }
        // ...and `clipboard_clearing` cannot produce one either, since it is
        // the only other route to a timer.
        for seconds in [0, 1, 29] {
            assert_eq!(
                clipboard_clearing(true, true, true, true, seconds).timer,
                Some(ClearInterval::from_seconds(30))
            );
        }
    }

    /// **The forms the interval field accepts**, each named.
    #[test]
    fn the_interval_field_accepts_whole_and_fractional_minutes() {
        let cases: [(&str, u64); 12] = [
            ("1", 60),
            ("2", 120),
            ("60", 3600),
            ("0.5", 30),
            (".5", 30),
            ("1.5", 90),
            ("2.5", 150),
            // A comma, because this is a Windows app and much of Europe types
            // one. Both separators, both with and without the leading zero.
            ("0,5", 30),
            (",5", 30),
            ("1,5", 90),
            // Whitespace is trimmed; trailing zeros carry no information.
            ("  1.5  ", 90),
            ("1.50", 90),
        ];
        for (text, seconds) in &cases {
            assert_eq!(
                parse_clipboard_minutes(text),
                ClipboardEntry::Accepted(ClearInterval::from_seconds(*seconds)),
                "{text:?} should be {seconds} seconds"
            );
        }
    }

    /// **Each refusal is its own answer, because each needs its own
    /// sentence.** A parser that returned one "no" for `soon`, `0.1` and
    /// `1.25` would leave the row unable to say why.
    #[test]
    fn the_interval_field_refuses_each_bad_entry_for_its_own_reason() {
        let cases: [(&str, ClipboardEntry); 14] = [
            ("", ClipboardEntry::NotANumber),
            ("   ", ClipboardEntry::NotANumber),
            ("soon", ClipboardEntry::NotANumber),
            (".", ClipboardEntry::NotANumber),
            ("1.2.3", ClipboardEntry::NotANumber),
            ("1,2,3", ClipboardEntry::NotANumber),
            ("1.2,3", ClipboardEntry::NotANumber),
            // No sign in the grammar, which is what makes a negative interval
            // unreachable rather than merely rejected by a range check.
            ("-1", ClipboardEntry::NotANumber),
            ("1e3", ClipboardEntry::NotANumber),
            // Below the floor, zero included.
            ("0", ClipboardEntry::BelowFloor),
            ("0.1", ClipboardEntry::BelowFloor),
            ("0.4", ClipboardEntry::BelowFloor),
            // Above the ceiling.
            ("61", ClipboardEntry::AboveCeiling),
            ("99999999999999999999999999", ClipboardEntry::NotANumber),
        ];
        for (text, expected) in &cases {
            assert_eq!(parse_clipboard_minutes(text), *expected, "{text:?}");
        }
        // Between steps: in range, but more than one decimal place. Refused
        // rather than snapped, so nothing is rounded where the user cannot
        // see it.
        for text in ["1.25", "0.55", "2.34", "1.05"] {
            assert_eq!(
                parse_clipboard_minutes(text),
                ClipboardEntry::BetweenSteps,
                "{text:?} was accepted, or refused for the wrong reason"
            );
        }
        // The control: the very same shapes one decimal place shorter are
        // accepted, so `BetweenSteps` is about the resolution and not about
        // decimals in general.
        assert!(matches!(parse_clipboard_minutes("1.2"), ClipboardEntry::Accepted(_)));
        assert!(matches!(parse_clipboard_minutes("2.3"), ClipboardEntry::Accepted(_)));
    }

    /// **Minutes and seconds round-trip exactly, in both directions, across
    /// the whole offered range** -- not at sampled points.
    ///
    /// This is what `CLIPBOARD_SECONDS_STEP` exists for: the displayed value
    /// and the stored value cannot disagree, because every stored value is
    /// exactly `n/10` minutes and every accepted entry is exactly a whole
    /// number of seconds.
    #[test]
    fn every_offered_interval_round_trips_through_the_field() {
        let mut seen = 0;
        let mut seconds = 30;
        while seconds <= 3600 {
            let interval = ClearInterval::from_seconds(seconds);
            let text = interval.as_minutes_text();
            assert_eq!(
                parse_clipboard_minutes(&text),
                ClipboardEntry::Accepted(interval),
                "{seconds}s displayed as {text:?}, which did not parse back to itself"
            );
            seen += 1;
            seconds += 6;
        }
        assert_eq!(seen, 596, "the range is not the one this test thinks it walked");
    }

    /// The display forms, spelled out, so `as_minutes_text` cannot start
    /// printing `1.0` or `0.50` without a test noticing.
    #[test]
    fn the_interval_is_shown_as_minutes_without_a_pointless_decimal() {
        assert_eq!(ClearInterval::from_seconds(30).as_minutes_text(), "0.5");
        assert_eq!(ClearInterval::from_seconds(60).as_minutes_text(), "1");
        assert_eq!(ClearInterval::from_seconds(90).as_minutes_text(), "1.5");
        assert_eq!(ClearInterval::from_seconds(150).as_minutes_text(), "2.5");
        assert_eq!(ClearInterval::from_seconds(3600).as_minutes_text(), "60");
        // A `.` and never a `,`, even though the parser accepts both.
        assert!(!ClearInterval::from_seconds(30).as_minutes_text().contains(','));
    }

    /// **"Reset to default" resets this section and nothing else.**
    ///
    /// The scope is the whole point of the button, so it is tested as a pure
    /// function: every clipboard field comes back, and every other preference
    /// -- including ones that are themselves non-default -- is left exactly
    /// where the user put it.
    #[test]
    fn resetting_the_clipboard_section_leaves_every_other_preference_alone() {
        let edited = Settings {
            // The section, all five away from their defaults.
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 300,
            // Everything else, also away from its default, so a reset that
            // reached too far is visible here rather than hidden by a field
            // that happened to be at its default already.
            keep_backend_running: false,
            prompt_on_match: false,
            check_breaches: true,
            fetch_icons: false,
            use_brand_logos: true,
            check_for_updates: false,
            reveal_totp_seed: true,
            auto_lock_enabled: false,
            auto_lock_minutes: 42,
            vault_window: Some(WindowGeometry { x: 1, y: 2, width: 1200, height: 800 }),
            ..Settings::default()
        };
        let reset = edited.with_default_clipboard_clearing();

        // The section came back.
        assert!(reset.clear_clipboard);
        assert!(reset.clear_clipboard_on_lock);
        assert!(reset.clear_clipboard_on_account_change);
        assert!(reset.clear_clipboard_on_quit);
        assert_eq!(reset.clear_clipboard_seconds, 60);

        // And nothing else moved.
        assert!(!reset.keep_backend_running);
        assert!(!reset.prompt_on_match);
        assert!(reset.check_breaches);
        assert!(!reset.fetch_icons);
        assert!(!reset.check_for_updates);
        assert!(reset.reveal_totp_seed);
        assert!(!reset.auto_lock_enabled);
        assert_eq!(reset.auto_lock_minutes, 42);
        assert_eq!(reset.vault_window, edited.vault_window);

        // The blunt version of the same claim, so a field added later and
        // wrongly reset is caught without anyone remembering to list it here:
        // the reset differs from the original in exactly the five fields.
        assert_eq!(
            reset,
            Settings {
                clear_clipboard: true,
                clear_clipboard_on_lock: true,
                clear_clipboard_on_account_change: true,
                clear_clipboard_on_quit: true,
                clear_clipboard_seconds: 60,
                ..edited.clone()
            }
        );

        // The control: resetting an already-default section changes nothing
        // at all, so the assertions above are about the five fields rather
        // than about the function rebuilding the struct.
        let untouched = Settings::default();
        assert_eq!(untouched.with_default_clipboard_clearing(), untouched);
    }

    /// **`persist_preferences` writes all five, and reads all five back.**
    /// The same shape as the guards on every other preference: the
    /// destructuring in `persist_preferences` is exhaustive, but binding a
    /// field and never assigning `on_disk`'s compiles.
    #[test]
    fn the_clipboard_preferences_survive_a_save_and_a_reload() {
        let path = temp_path("clipboard-persist");
        assert!(Settings::load(&path).clear_clipboard, "the premise: it starts on");
        Settings {
            clear_clipboard: false,
            clear_clipboard_on_lock: false,
            clear_clipboard_on_account_change: false,
            clear_clipboard_on_quit: false,
            clear_clipboard_seconds: 300,
            ..Settings::default()
        }
        .persist_preferences(&path)
        .unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.clear_clipboard, "the master switch was not written");
        assert!(!loaded.clear_clipboard_on_lock, "the lock trigger was not written");
        assert!(
            !loaded.clear_clipboard_on_account_change,
            "the account-change trigger was not written"
        );
        assert!(!loaded.clear_clipboard_on_quit, "the quit trigger was not written");
        assert_eq!(loaded.clear_clipboard_seconds, 300, "the interval was not written");

        // The other direction, so the writer is not simply writing `false`
        // and `300` over everything.
        Settings::default().persist_preferences(&path).unwrap();
        let back = Settings::load(&path);
        assert!(back.clear_clipboard);
        assert!(back.clear_clipboard_on_lock);
        assert!(back.clear_clipboard_on_account_change);
        assert!(back.clear_clipboard_on_quit);
        assert_eq!(back.clear_clipboard_seconds, 60);

        // The key names really are in the file, so the round trip above went
        // through disk rather than through a default that happens to match.
        Settings { clear_clipboard_seconds: 300, ..Settings::default() }
            .persist_preferences(&path)
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        for key in [
            "clear_clipboard",
            "clear_clipboard_on_lock",
            "clear_clipboard_on_account_change",
            "clear_clipboard_on_quit",
            "clear_clipboard_seconds",
        ] {
            assert!(text.contains(key), "{key} is not in the file at all: {text}");
        }
        // An integer on disk, never a float: `settings.json` must not acquire
        // a floating-point rounding artefact, which is the whole reason the
        // stored unit is seconds.
        assert!(
            text.contains("\"clear_clipboard_seconds\": 300"),
            "the interval is not stored as a whole number of seconds: {text}"
        );
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
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard here reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one. It is ALSO the literal this file's source
    /// guard cuts at (`no_test_in_this_module_touches_the_real_settings_file`
    /// splits the file on it), so it is the marker and the gate at once.
    const BELOW_CUT_MARKER: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Each is controlled below: it must still occur in
    /// this file exactly once, so a stale entry cannot quietly widen the hole
    /// this test exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// a whole module written on one line is not a module opener as far as
    /// this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk of everything from the cut to EOF, over whatever
    /// text it is handed. Returns `(visited, modules, closes, depth)` so the
    /// caller can control it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. `overlay_ui.rs`
    /// shipped the other kind -- a `contains` on needles that began with a
    /// carriage return -- and the committed blob in this repository is LF, so
    /// on any checkout without this machine's `core.autocrlf=true` it matched
    /// nothing, ever. The caller runs this over a normalised copy as well and
    /// requires the same answer.
    /// What this file's below-the-cut region is walked under.
    ///
    /// The walk itself is [`crate::below_cut::walk`] and is NOT written here.
    /// It used to be, in fifteen near-identical copies, which is how the
    /// escaped-quote off-by-one in the brace matcher reached three files at
    /// once and how every fix since has had to be applied N times or silently
    /// fail to propagate. What the copies really disagreed about is this
    /// struct's worth of text, so that is what stayed local.
    ///
    /// `is_module_opener` is this file's OWN
    /// [`below_cut_is_module_opener`] and not
    /// [`crate::below_cut::is_module_opener`], deliberately: the
    /// `modules == column_zero_module_openers(..)` control below compares the
    /// walk's count against the other instance, so a one-edit widening of
    /// either predicate desynchronizes the two and reds the suite. Pointing
    /// the walk at the shared predicate would have made both sides move
    /// together and thrown that property away.
    const BELOW_CUT_RULES: crate::below_cut::WalkRules = crate::below_cut::WalkRules {
        gate: BELOW_CUT_MARKER,
        gated_at_start: false,
        gate_at_column_zero: false,
        is_module_opener: below_cut_is_module_opener,
        string_lines: BELOW_CUT_STRING_LINES,
        top_level_item_note: "The source guard in this file slices at the first test gate, and `login_ui.rs` guards this file as source too, so an item down here can carry a writer call or a real-path resolver that neither of them is looking at.",
        ungated_module_note: "A `pub(crate) mod ext { .. }` written down here is the same escape, one `mod` deep.",
    };

    /// `(visited, modules, closes, depth)` for the region below this file's
    /// cut, by the one shared walk.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is checked by the caller");
        crate::below_cut::walk(&source[cut..], &BELOW_CUT_RULES)
    }

    /// **Below the cut there is nothing but test-only modules, and the cut is
    /// where the guard in this file believes it is.**
    ///
    /// The same walk `main.rs`, `app_identity.rs`, `app_window.rs`,
    /// `login_ui.rs`, `vault_window/mod.rs` and `overlay_ui.rs` carry. This
    /// file had the cut and no walk at all: its one source guard
    /// (`no_test_in_this_module_touches_the_real_settings_file`) splits on the
    /// first test gate and counts needles in the TAIL, and nothing anywhere
    /// said what may live down there.
    ///
    /// Two things can empty a cut-based guard silently, and neither changes
    /// the guard's own text:
    ///
    /// 1. **Anything below the test modules is read by nothing.** A production
    ///    item appended at EOF here is invisible to every source guard that
    ///    slices this file -- and it is not only this file's own: `login_ui.rs`
    ///    reads `settings.rs` as source too.
    /// 2. **The cut can move UP.** The guard takes the FIRST occurrence of the
    ///    marker, so the marker in a comment or a string above the real test
    ///    modules moves the boundary and changes which half every needle is
    ///    counted in -- silently.
    ///
    /// The walk closes the first; the line-start and anchor controls close the
    /// second.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("settings.rs");

        // 1. The cut lands at the start of a line, so the marker was matched
        //    at a real attribute and not inside a comment or a string.
        let cut = source.find(BELOW_CUT_MARKER).unwrap_or_else(|| {
            panic!(
                "{BELOW_CUT_MARKER:?} is not in this file at all -- the source guard here \
                 slices at it, and a slice that cannot be made is a guard that reads nothing"
            )
        });
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real attribute; that moves the \
             boundary the guard in this file counts needles either side of"
        );

        // 2. Positive control on WHERE the cut is: the last production item in
        //    the file must still be above it, and close to it. Were the marker
        //    matched earlier, this anchor would fall below the cut instead.
        const LAST_PRODUCTION_ITEM: &str = concat!(
            "auto_lock_policy(self.auto_lock_enabled, ",
            "self.auto_lock_minutes)"
        );
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it no \
             longer pins anything -- repoint it at the last production item above the test \
             modules"
        );
        let anchor = source
            .find(LAST_PRODUCTION_ITEM)
            .expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the halves the guard in this file counts in are not the \
             halves it names"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, run over an LF copy of this file and a CRLF copy of the
        //    same text, which must agree. Built BOTH ways rather than compared
        //    against the bytes on disk on purpose: this repository stores LF
        //    blobs and only `core.autocrlf=true` makes the working tree CRLF,
        //    so a control that asserted "this file is CRLF" would itself be a
        //    check that fires on one machine and fails on Linux CI -- which is
        //    the defect being closed here, wearing the other hat.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_the_cut(&lf);
        let as_crlf = walk_below_the_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF \
             one, so something in it is sensitive to line endings. That is exactly how the \
             check this replaced managed to be vacuous everywhere but on a checkout with \
             `core.autocrlf=true`: its needles began with a carriage return and the \
             committed blob is LF"
        );
        // And the file as it really is on disk, whichever of the two that is.
        let as_on_disk = walk_below_the_cut(source);
        assert!(
            as_on_disk == as_lf || as_on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = as_on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a \
             test module's worth -- the slice is empty or nearly so and this test proves \
             nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 `}}`, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 2,
            "the number of top-level test modules below the cut changed. That is fine -- but \
             this count is the control that proves the walk really visited them, so update it \
             deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );

        // The opener count, cross-checked against a SECOND instance of the
        // opener predicate. `column_zero_module_openers` uses
        // `below_cut::is_module_opener`; the walk used this file's own
        // `below_cut_is_module_opener`. Widening either one alone
        // desynchronizes them and fails here, which is the property that
        // sharing a single predicate would have cost.
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(&source[cut..]),
            "the walk opened {modules} modules but there are {} column-0 gated module openers \
             below the cut -- the walk's opener predicate and \
             `below_cut::is_module_opener` no longer agree",
            crate::below_cut::column_zero_module_openers(&source[cut..])
        );

        // Controls on the walk itself. Without these it could be a no-op that
        // visits lines and asserts nothing.
        let appended = format!("{source}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&appended)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test modules, which is \
             the exact mutation it exists to catch"
        );
        // An INDENTED top-level item, which a column-0-only filter would miss.
        // The payload is an indented, GATED module opener and not a
        // `struct`: a struct is refused whether or not indentation is
        // checked, because it is not a module opener either way, so it left
        // the indentation rule unmeasured. This shape the opener predicate
        // accepts, so only the indentation rule can refuse it -- and the
        // trailing column-0 `}` makes the payload one the walk would
        // otherwise ACCEPT, so deleting the rule reds this control.
        let indented =
            format!("{source}\n{BELOW_CUT_MARKER}\n    mod sneaked_indented {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&indented)).is_err(),
            "control: the walk accepted an INDENTED, gated module opener appended below \
             the test modules, which a column-0-only filter would miss"
        );
        // A column-0 line INSIDE the last test module that this file does
        // not name in its string-literal allowance. The line is planted by
        // dropping the file's final column-0 `}` and writing it back after
        // the payload, so the braces still balance and the module's real
        // close is still the last line -- the ONLY thing that refuses it is
        // the allowance being an exact list rather than a permission.
        // Measured: without this the `string_lines` rule was held by one
        // test in the whole crate, so a mutation plus deleting that test
        // were the two edits that opened it.
        let unlisted = format!(
            "{}zz_not_source\n}}\n",
            source
                .replace("\r\n", "\n")
                .strip_suffix("}\n")
                .expect("this file ends with a column-0 closing brace")
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&unlisted)).is_err(),
            "control: the walk accepted a column-0 line inside a test module that this \
             file's string-literal allowance does not name, so the allowance is a \
             permission and not a list"
        );
        // Liveness control at the IDENTICAL site: the SAME planting, walked
        // with this file's own rules except that the planted line is named in
        // the allowance, is ACCEPTED. So the refusal above is about the
        // allowance and not about the planting having broken the region.
        // This file's real `BELOW_CUT_STRING_LINES` is empty, so the naming
        // has to be done here rather than read off the constant.
        let naming_it = crate::below_cut::WalkRules {
            string_lines: &["zz_not_source"],
            ..BELOW_CUT_RULES
        };
        let cut_of_unlisted =
            unlisted.find(BELOW_CUT_MARKER).expect("the marker survives the planting");
        assert!(
            crate::below_cut::try_walk(&unlisted[cut_of_unlisted..], &naming_it).is_ok(),
            "control: the walk refuses the planted region even when the line IS named in \
             the allowance, so the refusal above is not measuring the allowance"
        );
        let ungated = format!("{source}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&ungated)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships"
        );

        // And the one the line walk could not catch: this file's own text with
        // its last module closed by an INDENTED brace, a `pub fn` at file
        // scope after it, and a column-0 `}` further down to rebalance the
        // count. Perfectly balanced source, no lexer trick -- every payload
        // line is indented, so the `depth == 1` branch skips it and the walk
        // ends with `closes == modules` and `depth == 0`. Measured SURVIVING
        // the whole suite at 2211 lib / 217 bin / 0 failed / 0 warnings in
        // both profiles, and shipping in the lib's DEBUG LLVM IR. Only the
        // byte-offset close check kills it.
        let balanced = format!(
            "{}    }}\n    pub fn sneaked(x: u64) -> u64 {{ x }}\n    \
             #[allow(dead_code)]\n    mod filler {{\n}}\n",
            source
                .replace("\r\n", "\n")
                .strip_suffix("}\n")
                .expect("this file ends with a column-0 closing brace")
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&balanced)).is_err(),
            "control: the walk accepted this file's last test module closed by an INDENTED \
             brace with a `pub fn` at file scope after it. That is the payload the byte-offset \
             close check exists for, and it is once again invisible"
        );
        // Every test gate in the file is one of the gates the walk consumed.
        // A gate the walk never saw is one at an indentation it skips, and
        // that is a shape this walk does not reason about.
        assert_eq!(
            source.matches(BELOW_CUT_MARKER).count(),
            modules,
            "this file has {} test gates but the walk found {modules} gated top-level \
             modules below the cut; one of them is somewhere the walk does not inspect",
            source.matches(BELOW_CUT_MARKER).count()
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }
}

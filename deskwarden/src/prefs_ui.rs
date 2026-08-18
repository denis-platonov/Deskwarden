//! Design 3e's sectioned preferences window.
//!
//! 3e is a left nav of seven sections (General, Autofill, Native apps,
//! Security, Shortcuts, Sync & account, About) beside a content pane, with the
//! app version pinned to the bottom of the nav. This file builds that shell,
//! and populates it **only where a setting genuinely exists**.
//!
//! What exists today is four fields on [`Settings`]: `keep_backend_running`,
//! `prompt_on_match`, `auto_lock_enabled` and `auto_lock_minutes`. All four
//! live on General -- the last two as a toggle and the number it governs, the
//! number greyed out while the toggle is off. `prompt_on_match` is the whole
//! of the automatic half of autofill, and is the one setting here that a
//! section of 3e (Autofill) would otherwise have claimed. Every other section in 3e -- its
//! five autofill toggles, the per-app table, Touch ID, the overlay-position
//! segmented control -- has no backing behaviour anywhere in this crate, so
//! those sections say so in one line rather than showing a switch that flips
//! and changes nothing. A control whose state is not connected to anything is
//! indistinguishable from a broken feature, and is this project's most-repeated
//! defect; an empty section is merely unfinished, which is the truth.
//!
//! Two caveats on the design, both deliberate:
//!
//!  * **3e is drawn as a macOS window** (traffic lights, a centred
//!    "Preferences" title, 44px bar) and is the only Preferences block in the
//!    document -- there is no Windows variant to read. Its *metrics, palette
//!    and typography* are taken verbatim; its window chrome is not, because
//!    this crate is Windows and every other window here paints
//!    [`draw_window_chrome`]'s bar instead.
//!  * **"Deskwarden 1.4.0" in 3e's nav footer is mock data.** The real version
//!    comes from `CARGO_PKG_VERSION`. So does 3e's "Bitwarden account linked" --
//!    see [`ACCOUNT_STATUS`] for why that one cannot be shown here at all yet.

use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::settings::{clamp_auto_lock_minutes, Settings};
use crate::theme;
use eframe::egui::{
    self, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW_TITLE: &str = "Deskwarden Preferences";

/// 3e's window card measures 1000x780, and a seven-row nav plus a content pane
/// does not fit in the 520x300 this window used while it had a single toggle
/// in it. Still non-resizable, like every other fixed window here: nothing in
/// this layout reflows usefully, and `login_ui::draw_resize_handles` is
/// deliberately the vault window's alone.
const WINDOW_SIZE: [f32; 2] = [1000.0, 780.0];

// ---------------------------------------------------------------------------
// 3e's metrics. Colours are `theme` constants throughout -- every value 3e
// uses already has a name there (`#eae7e7` = `HAIRLINE`, `#f3f2f2` = `CANVAS`,
// `#eef2fc` = `BLUE_WASH`, `#14307a` = `BLUE_DEEP`, `#d7d3d3` =
// `BORDER_STRONG`, `#9b9797` = `TEXT_GHOST`, `#7d7979` = `TEXT_FAINT`), so
// nothing here re-declares a colour under a new name.
// ---------------------------------------------------------------------------

/// `grid-template-columns: 208px 1fr`.
const NAV_WIDTH: f32 = 208.0;
/// The nav column's `padding: 14px 10px`.
const NAV_PAD_X: f32 = 10.0;
const NAV_PAD_Y: f32 = 14.0;
/// A nav row's `padding: 8px 10px` around 13px text, and the column's `gap: 2px`.
const NAV_ITEM_HEIGHT: f32 = 33.0;
const NAV_ITEM_PAD_X: f32 = 10.0;
const NAV_ITEM_GAP: f32 = 2.0;
const NAV_ITEM_RADIUS: u8 = 8;
/// The footer block's own `padding: 10px`.
const NAV_FOOTER_PAD: f32 = 10.0;

/// The content pane's `padding: 24px 28px` and `gap: 16px`.
const CONTENT_PAD_X: f32 = 28.0;
const CONTENT_PAD_Y: f32 = 24.0;
const CONTENT_GAP: f32 = 16.0;
/// The heading block's own `gap: 4px`.
const HEADING_GAP: f32 = 4.0;

/// A settings card: `border-radius: 10px`, `1px solid #eae7e7`, white.
const CARD_RADIUS: u8 = 10;
/// A card row's `padding: 13px 16px` and `gap: 20px`.
const ROW_PAD_X: i8 = 16;
const ROW_PAD_Y: i8 = 13;
const ROW_GAP: f32 = 20.0;
/// A row's label/description `gap: 2px`.
const ROW_TEXT_GAP: f32 = 2.0;
/// Width reserved for a row's trailing control. 3e sizes its controls
/// intrinsically and lets the text column flex; a fixed reservation is visually
/// identical (the control is right-aligned inside it, so it still lands on the
/// row's right edge) and it lets the text column be allocated at a known width
/// instead of whatever a flex layout happens to leave. Wide enough for the
/// widest control on this window, the 112pt stepper.
const CONTROL_COLUMN_WIDTH: f32 = 160.0;
/// Floor on a row's height, so a single-line row still fits a 28pt control.
const CONTROL_MIN_HEIGHT: f32 = STEPPER_HEIGHT;

/// 3e's toggle pill is 40x22 (painted by [`theme::toggle_pill`]).
const TOGGLE_SIZE: Vec2 = Vec2::new(40.0, 22.0);

/// The stepper borrows 3e's segmented control exactly -- `border: 1px solid
/// #d7d3d3; border-radius: 7px`, 12px text, cells divided by 1px of the same
/// border -- at the 28px height 3e gives its "+ Add app" button. 3e has no
/// numeric input anywhere, so this is the one control on this window the design
/// does not contain; it is assembled from 3e's own parts rather than invented.
const STEPPER_HEIGHT: f32 = 28.0;
const STEPPER_STEP_WIDTH: f32 = 28.0;
const STEPPER_VALUE_WIDTH: f32 = 56.0;
const STEPPER_RADIUS: u8 = 7;
/// Stable across frames because a `TextEdit`'s focus and cursor live in egui's
/// memory under its id, and an id derived from layout position would lose them
/// the moment anything above the row changed height.
const STEPPER_FIELD_ID: &str = "prefs-auto-lock-minutes";

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

const BACKEND_LABEL: &str = "Keep the Bitwarden backend running";
const BACKEND_DESCRIPTION: &str = "Faster, and uses about 110 MB while idle. Off runs it only \
     while the vault window is open; autofill is unaffected either way.";

const PROMPT_LABEL: &str = "Prompt on match";
/// **Says what OFF does, because off is the state that changes what the app
/// does on its own.** The user's own framing: "only shortcuts will work in
/// that case". Naming the hotkey here is what stops the toggle reading as
/// "switch autofill off" -- it never is; the hotkey arms for every match in
/// both states (`app::match_arms_hotkey`).
const PROMPT_DESCRIPTION: &str = "Offer to fill when an app you have matched comes to the front. \
     Off means nothing happens on its own and CTRL+ALT+B is the only way to fill. Nothing is \
     ever typed until you ask for it either way.";

const BREACH_LABEL: &str = "Check passwords against known breaches";
/// **Says what leaves the machine, because something does.** Off by default is
/// stated in the copy and not only in `Settings::default`: this is the one row
/// on General whose ON state makes a network request keyed on a password, and
/// a user reading the pane should not have to infer that from the pill.
/// The k-anonymity bargain -- five hex characters out, thirty-five matched
/// here -- is what makes the request safe to offer at all, so it is the
/// description rather than a footnote.
const BREACH_DESCRIPTION: &str = "Off by default. When on, Deskwarden sends the first 5 \
     characters of a SHA-1 hash of a password to Have I Been Pwned and matches the rest on this \
     machine. Your password, and the rest of its hash, never leave your PC.";

const FETCH_ICONS_LABEL: &str = "Show site icons";
/// **Says what the request discloses, and says it is the DOMAIN.** This is the
/// row for the request `PRIVACY.md` calls the one with the most privacy weight
/// in the app, and the whole reason a user would turn it off is what the
/// service on the other end gets to see. Copy that said only "downloads icons"
/// would be describing the feature and hiding the cost.
///
/// Three things are named because each is a thing a user would otherwise have
/// to guess: WHAT is sent (the domain), to WHOM (their own server's icon
/// service when they self-host), and what is NOT sent -- the credential. The
/// last is not padding: "sends the website to Bitwarden" is exactly what a
/// worried reader assumes, and it is wrong.
///
/// On by default, and stated in the copy rather than left to
/// `Settings::default`, the same way `BREACH_DESCRIPTION` states its own
/// opposite default.
const FETCH_ICONS_DESCRIPTION: &str = "On by default. Deskwarden asks the icon service for an \
     item's site icon by domain name — your own server's if you self-host. It never sends the \
     username, the password, or which account the item is in. Off shows coloured initials \
     instead and nothing leaves your PC.";

const TOTP_SECRET_LABEL: &str = "Show TOTP secrets on the details screen";
/// **Says what ON adds, and what it costs.** Off is the default and is stated
/// in the copy rather than left to `Settings::default`, exactly as
/// `BREACH_DESCRIPTION` states its own: these are the two rows on General
/// whose ON state gives something away, and a user reading the pane should
/// not have to infer either from the pill.
///
/// The word "masked" is in the copy because the row this turns on is masked
/// until its eye is clicked -- turning this on does not put a seed on screen,
/// it puts a row there. And the reason to leave it off is named rather than
/// implied: the six-digit code expires, the seed it comes from does not.
const TOTP_SECRET_DESCRIPTION: &str = "Off by default. When on, an item's TOTP secret appears \
     as an extra masked row under its one-time code, revealed by clicking the eye. The code \
     expires in 30 seconds; the secret behind it never does.";

const AUTO_LOCK_ENABLED_LABEL: &str = "Lock the vault when idle";
const AUTO_LOCK_ENABLED_DESCRIPTION: &str =
    "Off means the vault stays unlocked until you lock it yourself or quit Deskwarden.";

const AUTO_LOCK_LABEL: &str = "Lock the vault after";
const AUTO_LOCK_DESCRIPTION: &str = "Minutes of no activity before the vault window locks itself. \
     One minute is the shortest Deskwarden will use.";

/// Heading of every section that has no settings behind it yet. Its presence
/// is what the tests assert on, so an empty section can never quietly acquire
/// a control without one of them noticing.
const NOT_YET_TITLE: &str = "Nothing to configure here yet";

/// The one global shortcut this app registers, in the form the user sees it.
///
/// Hardcoded rather than derived, because `hotkey::register_fill_hotkey`
/// builds it from `global_hotkey`'s `Modifiers`/`Code` types, which have no
/// display form worth showing a user. `the_shortcuts_page_names_the_hotkey_
/// that_is_actually_registered` is a source-text guard over `hotkey.rs` so the
/// two cannot drift apart silently.
const FILL_HOTKEY: &str = "CTRL+ALT+B";
const FILL_HOTKEY_LABEL: &str = "Fill the focused app";
const FILL_HOTKEY_DESCRIPTION: &str =
    "The only shortcut Deskwarden registers. It cannot be changed yet.";

/// What the About page can say about the account, which is nothing.
///
/// 3e's nav footer reads "Bitwarden account linked", and this window has no
/// way to know that. The status lives in `main.rs`'s
/// `cached_status_details: Option<login_ui::BwStatusDetails>` -- which is in
/// scope at the `prefs_ui::run(settings.clone())` call site -- and getting it
/// here means widening this function's signature, i.e. editing `main.rs`.
/// Until that happens the page says where the answer actually is rather than
/// asserting a link that may not exist; re-running `bw status` from this
/// window to find out would be a blocking subprocess call on the UI thread for
/// a decorative line.
const ACCOUNT_STATUS: &str = "Open the vault window to see the signed-in account.";

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// The seven pages 3e's nav lists, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Autofill,
    NativeApps,
    Security,
    Shortcuts,
    SyncAndAccount,
    About,
}

impl Section {
    /// 3e's nav, top to bottom.
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Autofill,
        Section::NativeApps,
        Section::Security,
        Section::Shortcuts,
        Section::SyncAndAccount,
        Section::About,
    ];

    /// The nav row's text, which is also the content pane's heading (3e uses
    /// the same word for both).
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Autofill => "Autofill",
            Section::NativeApps => "Native apps",
            Section::Security => "Security",
            Section::Shortcuts => "Shortcuts",
            Section::SyncAndAccount => "Sync & account",
            Section::About => "About",
        }
    }

    /// The line under the heading. Autofill's is 3e's own, verbatim; the rest
    /// are written to the same shape, since 3e only draws the Autofill page.
    fn subtitle(self) -> &'static str {
        match self {
            Section::General => "How Deskwarden runs in the background, and when it locks itself.",
            Section::Autofill => {
                "How Deskwarden behaves when a native login field takes focus."
            }
            Section::NativeApps => "The applications Deskwarden fills credentials into.",
            Section::Security => "What Deskwarden asks for before it reveals or fills a secret.",
            Section::Shortcuts => "The keys that reach Deskwarden from anywhere.",
            Section::SyncAndAccount => "The Bitwarden account this vault comes from.",
            Section::About => "Which build of Deskwarden this is.",
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Everything the window edits, plus the one piece of transient UI state
/// (the stepper's text buffer) that has to survive between frames.
pub struct PrefsState {
    pub settings: Settings,
    section: Section,
    /// What is currently *typed* into the minutes field, which is not the same
    /// as `settings.auto_lock_minutes`: mid-edit it may be empty, or "4" on the
    /// way to "45". It is reconciled back to the committed value the moment the
    /// field loses focus (see [`minutes_stepper`]).
    auto_lock_text: String,
}

impl PrefsState {
    /// Clamps the loaded value up front, deliberately.
    ///
    /// `settings.json` can contain `auto_lock_minutes: 0` (it was, before the
    /// `auto_lock_enabled` toggle existed, the only hand-written way to say
    /// "never lock"), and `settings::auto_lock_policy` still uses 1 minute for
    /// it -- deliberately, see `MIN_AUTO_LOCK_MINUTES`'s doc: "never" is now
    /// the toggle's job and a legacy `0` is not retro-fitted to mean it.
    /// Showing the stored `0` in the field would
    /// make this window display a number that is not the number in effect --
    /// so the window opens on the value that *is* in effect. The cost is that
    /// opening Preferences on such a file makes `edited != settings` true in
    /// `main.rs` and writes the corrected value back, which is the right
    /// outcome: the file then says what the app is doing.
    pub fn new(settings: Settings) -> Self {
        let minutes = clamp_auto_lock_minutes(settings.auto_lock_minutes);
        Self {
            settings: Settings { auto_lock_minutes: minutes, ..settings },
            section: Section::General,
            auto_lock_text: minutes.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The numeric control (pure parts first)
// ---------------------------------------------------------------------------

/// What a typed entry commits to: the number if it is one, otherwise the value
/// that was already there.
///
/// Every path runs through [`clamp_auto_lock_minutes`], so the committed value
/// is by construction one `Settings::auto_lock_timeout` will use unaltered --
/// this control cannot put a number on screen that the clamp then overrides.
/// A non-number (empty, mid-edit, "soon", or a value too large for `u64`)
/// leaves the previous value alone rather than resetting to a default: the
/// user pressing Escape's worth of nonsense should not silently change their
/// lock timeout.
fn parse_minutes_entry(text: &str, previous: u64) -> u64 {
    match text.trim().parse::<u64>() {
        Ok(minutes) => clamp_auto_lock_minutes(minutes),
        Err(_) => clamp_auto_lock_minutes(previous),
    }
}

/// One step down, never below the floor.
fn decrement_minutes(value: u64) -> u64 {
    clamp_auto_lock_minutes(value.saturating_sub(1))
}

/// One step up. `saturating_add` for the same reason `auto_lock_timeout`
/// saturates: `u64::MAX` is reachable from a hand-edited file, and `+ 1` on it
/// panics in a debug build.
fn increment_minutes(value: u64) -> u64 {
    clamp_auto_lock_minutes(value.saturating_add(1))
}

/// `[-] [ 15 ] [+]` in 3e's segmented-control box. Returns the value after this
/// frame; `buffer` is the caller's persistent text state.
///
/// `enabled == false` is the auto-lock toggle being off, and it is a *disabled*
/// control rather than a hidden one: the number stays on screen (greyed, so it
/// reads as inert) because it is still the number that comes back when the
/// toggle is turned on again, and a control that disappears takes its value's
/// visibility with it. Nothing in here is merely painted differently --
/// neither step button senses a click, and the text field is replaced by a
/// painted galley rather than a read-only `TextEdit`, so there is no widget
/// left to focus, click into, or type at. "Looks disabled" and "is disabled"
/// are the pair this codebase keeps having to reunite.
fn minutes_stepper(ui: &mut Ui, value: u64, buffer: &mut String, enabled: bool) -> u64 {
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(STEPPER_STEP_WIDTH * 2.0 + STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
        Sense::hover(),
    );
    // 3e has no disabled variant of its segmented control, so the greyed
    // treatment is built from 3e's own two lighter greys: the card's hairline
    // border in place of the control border, on the canvas grey in place of
    // white. No new colour is introduced for it.
    let (fill, border) = if enabled {
        (theme::CARD, theme::BORDER_STRONG)
    } else {
        (theme::CANVAS, theme::HAIRLINE)
    };
    ui.painter().rect(
        outer,
        CornerRadius::same(STEPPER_RADIUS),
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let minus = Rect::from_min_size(outer.min, Vec2::new(STEPPER_STEP_WIDTH, STEPPER_HEIGHT));
    let field = Rect::from_min_size(
        Pos2::new(minus.max.x, outer.min.y),
        Vec2::new(STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
    );
    let plus = Rect::from_min_size(
        Pos2::new(field.max.x, outer.min.y),
        Vec2::new(STEPPER_STEP_WIDTH, STEPPER_HEIGHT),
    );
    for x in [field.min.x, field.max.x] {
        ui.painter().rect_filled(
            Rect::from_min_max(Pos2::new(x, outer.min.y), Pos2::new(x + 1.0, outer.max.y)),
            CornerRadius::ZERO,
            border,
        );
    }

    // The buttons run *before* the field is drawn, and that ordering is
    // load-bearing rather than incidental: a `TextEdit` paints the string it
    // was handed, so updating `buffer` after drawing it left the field showing
    // the previous number for one whole frame -- press `-` on 16 and the value
    // was 15 while the control still read 16.
    //
    // A step operates on what the field currently *shows*, not on the last
    // committed value, so typing 45 and then pressing `+` gives 46 rather than
    // discarding the 45 and giving 16.
    let shown = parse_minutes_entry(buffer, value);
    let mut next = value;
    // The floor is shown, not merely enforced: at the minimum there is nothing
    // below to step to, and a `-` that accepts the click and refuses the change
    // is the same lie as a switch that does nothing. `enabled &&` in front of
    // both, so an off toggle disables the ends for the same reason.
    if step_button(ui, minus, "-", enabled && shown > decrement_minutes(shown)) {
        next = decrement_minutes(shown);
    }
    if step_button(ui, plus, "+", enabled && shown < increment_minutes(shown)) {
        next = increment_minutes(shown);
    }
    let stepped = next != value;
    if stepped {
        *buffer = next.to_string();
    }

    if !enabled {
        // A painted galley, not a disabled/read-only `TextEdit`: egui's
        // read-only text edit still takes focus, still shows a caret and
        // still accepts a click, which is precisely the "greyed out but
        // secretly live" state this is meant not to be. Nothing here is
        // interactive because there is no widget here at all.
        let galley = ui.painter().layout_no_wrap(
            value.to_string(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::TEXT_GHOST,
        );
        ui.painter().galley(
            Pos2::new(
                field.center().x - galley.size().x / 2.0,
                field.center().y - galley.size().y / 2.0,
            ),
            galley,
            theme::TEXT_GHOST,
        );
        // Kept in step with the committed value while the control is off, so
        // turning the toggle back on hands the live field the number that has
        // been on screen all along rather than a stale mid-edit fragment.
        *buffer = value.to_string();
        return value;
    }

    // Frameless: the box around it is painted above, so `TextEdit`'s own frame
    // would draw a second, differently-rounded rectangle inside it.
    // The number is placed by hand rather than by `horizontal_align(Center)`,
    // and that is a measurement, not a preference. On egui 0.35 a singleline
    // `TextEdit` centres its galley over a region 12pt WIDER than the rect it
    // is handed: given `field.shrink(4.0)` (48pt) it centred over 60pt, which
    // put the number 6pt right of the cell while the greyed branch above --
    // which centres an explicit galley -- sat dead centre. So the live control
    // and the disabled one disagreed by 6pt horizontally and 3.5pt vertically,
    // which is what the bug report was.
    //
    // `desired_width` does NOT move it (measured: 48 and 36 give the same
    // result), so there is no width to tune. What IS exact is `Align::Min`,
    // which lands the text at precisely `rect.min.x`. The origin is therefore
    // computed here from the same `layout_no_wrap` measurement the greyed
    // branch uses, so the two branches now agree BY CONSTRUCTION rather than
    // by coincidence -- and neither depends on the 12pt discrepancy being
    // understood, which it is not.
    //
    // The rect still runs to the cell's right edge so most of the cell stays
    // clickable; only the text's left edge moves with the digit count.
    let text_width = ui
        .painter()
        .layout_no_wrap(
            buffer.clone(),
            FontId::new(12.0, FontFamily::Proportional),
            theme::INK,
        )
        .size()
        .x;
    let inner = field.shrink(4.0);
    let entry = ui.put(
        Rect::from_min_max(
            Pos2::new(field.center().x - text_width / 2.0, inner.min.y),
            inner.max,
        ),
        egui::TextEdit::singleline(buffer)
            .id(egui::Id::new(STEPPER_FIELD_ID))
            .frame(egui::Frame::new())
            .font(FontId::new(12.0, FontFamily::Proportional))
            .horizontal_align(egui::Align::Min)
            .vertical_align(egui::Align::Center)
            .margin(Margin::ZERO),
    );
    if entry.lost_focus() && !stepped {
        next = parse_minutes_entry(buffer, value);
    }

    // Reconciled only when the field is not being typed into -- otherwise every
    // keystroke would be replaced by the committed value and the field could
    // never be edited at all.
    if !entry.has_focus() {
        *buffer = next.to_string();
    }
    next
}

/// One end cell of the stepper. Inert when `enabled` is false: no click sense,
/// no hover cursor, ghosted glyph.
fn step_button(ui: &mut Ui, rect: Rect, glyph: &str, enabled: bool) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, egui::Id::new(STEPPER_FIELD_ID).with(glyph), sense);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if enabled { theme::TEXT_SECONDARY } else { theme::TEXT_GHOST };
    // ASCII `-` and `+` rather than U+2212 MINUS SIGN: the bundled Archivo
    // subset is the only face these can render in, and a glyph it lacks would
    // paint as a replacement box.
    let galley = ui.painter().layout_no_wrap(
        glyph.to_owned(),
        FontId::new(14.0, FontFamily::Name(theme::SEMIBOLD.into())),
        color,
    );
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    enabled && response.clicked()
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Nav column and content pane. Split out of [`run`] so the tests can drive
/// real frames of it without opening a window.
fn draw_prefs_body(ui: &mut Ui, state: &mut PrefsState) {
    let full = ui.max_rect();
    let nav = Rect::from_min_max(full.min, Pos2::new(full.min.x + NAV_WIDTH, full.max.y));
    let content = Rect::from_min_max(Pos2::new(nav.max.x, full.min.y), full.max);

    draw_nav(ui, nav, state);

    let inner = content.shrink2(Vec2::new(CONTENT_PAD_X, CONTENT_PAD_Y));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, CONTENT_GAP);
        draw_section(ui, state);
    });
}

fn draw_nav(ui: &mut Ui, rect: Rect, state: &mut PrefsState) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::CARD);
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(rect.max.x - 1.0, rect.min.y), rect.max),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    let inner = rect.shrink2(Vec2::new(NAV_PAD_X, NAV_PAD_Y));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, NAV_ITEM_GAP);
        for section in Section::ALL {
            if nav_item(ui, section.label(), state.section == section) {
                state.section = section;
            }
        }
    });

    // 3e pins the version to the bottom of the nav with a `flex: 1` spacer.
    // There is no second line: 3e's "Bitwarden account linked" is a claim this
    // window cannot make (see `ACCOUNT_STATUS`).
    let galley = ui.painter().layout_no_wrap(
        version_line(),
        FontId::new(11.0, FontFamily::Proportional),
        theme::TEXT_GHOST,
    );
    ui.painter().galley(
        Pos2::new(
            inner.min.x + NAV_FOOTER_PAD,
            inner.max.y - NAV_FOOTER_PAD - galley.size().y,
        ),
        galley,
        theme::TEXT_GHOST,
    );
}

fn nav_item(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), NAV_ITEM_HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(NAV_ITEM_RADIUS), theme::BLUE_WASH);
    }
    let (family, color) = if selected {
        (FontFamily::Name(theme::BOLD.into()), theme::BLUE_DEEP)
    } else {
        (FontFamily::Proportional, theme::INK)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::new(13.0, family), color);
    ui.painter().galley(
        Pos2::new(
            rect.min.x + NAV_ITEM_PAD_X,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    response.clicked()
}

/// `Deskwarden <version>`, from the crate's own version rather than 3e's
/// mocked "1.4.0".
fn version_line() -> String {
    format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))
}

fn draw_section(ui: &mut Ui, state: &mut PrefsState) {
    section_heading(ui, state.section);
    match state.section {
        Section::General => draw_general(ui, state),
        Section::Autofill => draw_not_yet(
            ui,
            "Overlay behaviour is fixed for now. Per-app behaviour is chosen from the tray's \
             \"Add app...\", not from here.",
        ),
        Section::NativeApps => draw_not_yet(
            ui,
            "Deskwarden fills whichever application a vault item has been matched to. Matches are \
             added from the tray's \"Add app...\".",
        ),
        Section::Security => draw_not_yet(
            ui,
            "Auto-lock is on the General page. Nothing else here is configurable yet.",
        ),
        Section::Shortcuts => draw_shortcuts(ui),
        Section::SyncAndAccount => draw_not_yet(
            ui,
            "Signing in, syncing and locking are all done from the vault window.",
        ),
        Section::About => draw_about(ui),
    }
}

fn section_heading(ui: &mut Ui, section: Section) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = HEADING_GAP;
        // `letter-spacing: -0.02em` at 24px is -0.48pt; `RichText` has no
        // tracking control, so this goes through `theme::letterspaced`.
        ui.label(theme::letterspaced(
            section.label(),
            24.0,
            theme::EXTRABOLD,
            -0.48,
            theme::INK,
        ));
        ui.label(
            RichText::new(section.subtitle())
                .size(13.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

// ---------------------------------------------------------------------------
// Cards and rows
// ---------------------------------------------------------------------------

/// A white card with 3e's hairline border, sized to whatever `add` drew. The
/// background shape is reserved before the content so it paints underneath.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let bg = ui.painter().add(egui::Shape::Noop);
    let width = ui.available_width();
    let inner = ui.scope(|ui| {
        ui.set_width(width);
        // Rows carry their own padding and separators; egui's default item
        // spacing between them would show as a gap in the card.
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        add(ui);
    });
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            inner.response.rect,
            CornerRadius::same(CARD_RADIUS),
            theme::CARD,
            Stroke::new(1.0, theme::HAIRLINE),
            StrokeKind::Inside,
        ),
    );
}

fn card_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .inner_margin(Margin {
            left: ROW_PAD_X,
            right: ROW_PAD_X,
            top: ROW_PAD_Y,
            bottom: ROW_PAD_Y,
        })
        .show(ui, |ui| add(ui));
}

/// 3e's `border-bottom: 1px solid #f3f2f2` between rows -- one step lighter
/// than the card's own border, which is `HAIRLINE`.
fn row_separator(ui: &mut Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::CANVAS);
}

/// A row's `flex: 1` text column: 14px semibold title over a 12px faint line.
fn row_text(ui: &mut Ui, label: &str, description: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_TEXT_GAP;
        ui.label(theme::semibold(label, 14.0).color(theme::INK));
        ui.label(
            RichText::new(description)
                .size(12.0)
                .color(theme::TEXT_FAINT),
        );
    });
}

/// A row whose trailing control is drawn by `control`: 3e's `flex: 1` text
/// column, the 20px gap, then the control right-aligned and vertically centred
/// on the text.
///
/// The two columns are allocated at explicit widths rather than by wrapping the
/// whole row in a `right_to_left` layout, and that is not a style preference.
/// A `Layout::right_to_left(Align::Center)` has to know its own height to
/// centre anything in it, so given an unbounded one it takes *all* the height
/// still available in the card -- the first row of a two-row card consumed
/// every remaining point and the second row was laid out at the bottom edge of
/// the window with zero height, painting its title and silently dropping its
/// description. Measuring the text column first and handing the control a rect
/// of exactly that height is what makes the centring well-defined.
fn control_row(ui: &mut Ui, label: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    card_row(ui, |ui| {
        let text_width = (ui.available_width() - CONTROL_COLUMN_WIDTH - ROW_GAP).max(1.0);
        let origin = ui.cursor().min;
        let text = ui.allocate_ui_with_layout(
            Vec2::new(text_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(text_width);
                row_text(ui, label, description);
            },
        );
        let height = text.response.rect.height().max(CONTROL_MIN_HEIGHT);
        let control_rect = Rect::from_min_size(
            Pos2::new(origin.x + text_width + ROW_GAP, origin.y),
            Vec2::new(CONTROL_COLUMN_WIDTH, height),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            control,
        );
    });
}

/// 3e's settings row: label, description, trailing 40x22 pill. Returns the new
/// value. The pill is paint-only ([`theme::toggle_pill`]), so the click sense
/// is allocated here.
fn toggle_row(ui: &mut Ui, label: &str, description: &str, value: bool) -> bool {
    let mut next = value;
    control_row(ui, label, description, |ui| {
        let (rect, response) = ui.allocate_exact_size(TOGGLE_SIZE, Sense::click());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            theme::toggle_pill(ui, value);
        });
        if response.clicked() {
            next = !value;
        }
    });
    next
}

/// A row that reports a value rather than editing one (About, Shortcuts).
fn value_row(ui: &mut Ui, label: &str, description: &str, value: &str) {
    control_row(ui, label, description, |ui| {
        ui.label(RichText::new(value).size(13.0).color(theme::TEXT_MUTED));
    });
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn draw_general(ui: &mut Ui, state: &mut PrefsState) {
    card(ui, |ui| {
        state.settings.keep_backend_running = toggle_row(
            ui,
            BACKEND_LABEL,
            BACKEND_DESCRIPTION,
            state.settings.keep_backend_running,
        );
        row_separator(ui);
        // The one switch that governs what a matched window does. It sits on
        // General beside the other two rather than under Shortcuts, because
        // it is not about a shortcut: `PROMPT_DESCRIPTION` names the hotkey
        // only to say what is left when this is off.
        state.settings.prompt_on_match = toggle_row(
            ui,
            PROMPT_LABEL,
            PROMPT_DESCRIPTION,
            state.settings.prompt_on_match,
        );
        row_separator(ui);
        // Directly under the prompt row, wired exactly as it is. Off by
        // default and left that way here: this row is the only consent that
        // exists for the range request, so it is set by a click on this pill
        // and by nothing else.
        state.settings.check_breaches = toggle_row(
            ui,
            BREACH_LABEL,
            BREACH_DESCRIPTION,
            state.settings.check_breaches,
        );
        row_separator(ui);
        // Directly under the breach row, because the two are the app's two
        // vault-keyed network calls and a user weighing one is weighing the
        // other. They default OPPOSITE ways -- see `Settings::fetch_icons`
        // for why -- so they are neighbours rather than a group with a
        // shared rule.
        state.settings.fetch_icons = toggle_row(
            ui,
            FETCH_ICONS_LABEL,
            FETCH_ICONS_DESCRIPTION,
            state.settings.fetch_icons,
        );
        row_separator(ui);
        // Directly under the icon row, the other off-by-default row, and
        // wired exactly as it is. This pill is the only thing that decides
        // whether the read pane draws a TOTP-secret row at all -- the pane
        // skips the row outright when this is off rather than drawing it
        // disabled or invisible.
        state.settings.reveal_totp_seed = toggle_row(
            ui,
            TOTP_SECRET_LABEL,
            TOTP_SECRET_DESCRIPTION,
            state.settings.reveal_totp_seed,
        );
        row_separator(ui);
        // The toggle sits above the number it governs, in 3e's own 40x22
        // pill, and the number's row stays put below it -- greyed, not
        // removed. A row that vanished would reflow the card on every click
        // and would hide the value the toggle is about to restore.
        state.settings.auto_lock_enabled = toggle_row(
            ui,
            AUTO_LOCK_ENABLED_LABEL,
            AUTO_LOCK_ENABLED_DESCRIPTION,
            state.settings.auto_lock_enabled,
        );
        row_separator(ui);
        let enabled = state.settings.auto_lock_enabled;
        control_row(ui, AUTO_LOCK_LABEL, AUTO_LOCK_DESCRIPTION, |ui| {
            state.settings.auto_lock_minutes = minutes_stepper(
                ui,
                state.settings.auto_lock_minutes,
                &mut state.auto_lock_text,
                enabled,
            );
        });
    });
}

fn draw_shortcuts(ui: &mut Ui) {
    card(ui, |ui| {
        // `kbd_chip`'s grey-on-canvas treatment, not `kbd_chip_on_card`'s: the
        // latter is a *white* chip, made for 3h's blue-washed panel, and it
        // would be invisible on this white card.
        control_row(ui, FILL_HOTKEY_LABEL, FILL_HOTKEY_DESCRIPTION, |ui| {
            theme::kbd_chip(ui, FILL_HOTKEY, false)
        });
    });
}

fn draw_about(ui: &mut Ui) {
    card(ui, |ui| {
        value_row(
            ui,
            "Version",
            "Unofficial, and unaffiliated with Bitwarden, Inc.",
            &version_line(),
        );
        row_separator(ui);
        // No trailing value, because there is no value to put there -- see
        // `ACCOUNT_STATUS`. A row with an empty right-hand column would read
        // as a field that failed to load.
        card_row(ui, |ui| row_text(ui, "Bitwarden account", ACCOUNT_STATUS));
    });
}

/// The honest state of a section 3e specifies but nothing implements.
///
/// Deliberately not a disabled toggle, a "coming soon" badge, or a greyed-out
/// copy of 3e's controls: all three look like a feature that is present and
/// broken. A sentence saying what governs the behaviour today, and where it is
/// set if it is set anywhere, is the whole content.
fn draw_not_yet(ui: &mut Ui, detail: &str) {
    card(ui, |ui| {
        card_row(ui, |ui| row_text(ui, NOT_YET_TITLE, detail));
    });
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// Opens the preferences window and blocks until it closes (same shape as
/// every other window in this crate -- `run_ui_native` pumps its own event
/// loop), returning the edited settings. The caller decides whether anything
/// actually changed and persists them; this function never touches disk itself.
///
/// The returned `Settings` differs from the argument in at most
/// `keep_backend_running`, `prompt_on_match`, `auto_lock_enabled` and
/// `auto_lock_minutes` -- the four fields `Settings::persist_preferences` owns. `vault_window` is carried through
/// untouched, which is what makes `main.rs`'s stale copy of it harmless.
pub fn run(settings: Settings) -> Settings {
    let state = Rc::new(RefCell::new(PrefsState::new(settings)));
    let state_for_closure = state.clone();
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // Same first-frame guard every window here uses: egui only picks
            // up a new font set at the *start* of the next frame, so drawing
            // real (Archivo-styled) content this frame would either panic on
            // a font family that doesn't exist yet or, worse, flash one
            // unpainted near-black frame before the background fill lands --
            // which reads as a console window flashing open, not a
            // preferences dialog.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            // The OS window exists by this first painted frame (the same
            // hook `round_window_corners` uses), and this is where it is
            // brought to the front. See `foreground`: a refusal from Windows
            // flashes the taskbar button rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        match draw_window_chrome(ui, WINDOW_TITLE) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            // The chrome paints a - control whether or not anyone listens for
            // it; this window used to draw it and drop the action, so the
            // button was inert. Same handling the login window gives it.
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                draw_prefs_body(ui, &mut state_for_closure.borrow_mut());
            });
    });

    let edited = state.borrow().settings.clone();
    edited
}

// ---------------------------------------------------------------------------
// The in-window modal
//
// The same form, over the vault window, instead of a window of its own.
//
// **Nothing about the settings form is duplicated here.** [`run`] above and
// [`draw_prefs_modal`] below both call the one [`draw_prefs_body`], which is
// where every section, card, row and control lives. What differs between the
// two is only what surrounds it: `run` gets its background from
// `theme::paint_window_background`, its title and its dismiss from
// `draw_window_chrome`, and its 1000x780 from the OS; the modal paints its own
// card, its own 44px header and its own scrim, because it has no window of its
// own to get any of that from. Two shells over one body -- not two forms.
//
// `run` is deliberately kept. Preferences is also reachable from the tray with
// no vault window open at all (and, in particular, with the vault LOCKED), and
// a modal needs a window to be modal over. Opening the vault window for it
// would mean demanding the master password to change a checkbox. So the tray
// keeps a real window, and the gear -- which by definition already has a
// window -- gets the modal.
// ---------------------------------------------------------------------------

/// The modal's own title bar: a touch taller than `ChromeMetrics::LOGIN`'s
/// 40px because it carries no window controls and reads as a card header.
const MODAL_HEADER_HEIGHT: f32 = 44.0;
/// Breathing room left around the card, so the dimmed vault is visible on
/// every side and the modal reads as sitting *over* it rather than replacing
/// it. That visible frame is the whole point of the feature.
const MODAL_SCREEN_MARGIN: f32 = 24.0;
const MODAL_RADIUS: u8 = 12;
const MODAL_TITLE: &str = "Preferences";
/// The scrim's alpha, taken from `folder_modal` and the launch confirmation
/// verbatim rather than picked again.
const MODAL_SCRIM_ALPHA: u8 = 90;

/// What a frame of the modal asks its host to do.
///
/// One variant besides `None`, and no `Save`/`Cancel` pair: this form commits
/// as it is edited (every control writes straight into `PrefsState::settings`),
/// exactly as it did when it was a window whose only exit was the ✕. A Cancel
/// here would have to mean "put back the settings as they were on open", which
/// nothing in `run` ever offered and nothing on disk records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsAction {
    None,
    Close,
}

/// The card's rectangle for a given window content rect. Pure, so the "does it
/// fit, and is it inset on every side" question is answerable without a frame.
///
/// 3e's 1000x740 body is a ceiling, not a demand: the vault window is
/// resizable and its minimum is smaller than that, so on a small window the
/// card is whatever is left after [`MODAL_SCREEN_MARGIN`] on each side. It is
/// never larger than the pane it is over, which is what stops the header's ✕
/// from being pushed off-screen.
pub fn modal_card_rect(screen: Rect) -> Rect {
    let width = (screen.width() - 2.0 * MODAL_SCREEN_MARGIN)
        .clamp(0.0, WINDOW_SIZE[0]);
    let height = (screen.height() - 2.0 * MODAL_SCREEN_MARGIN)
        .clamp(0.0, WINDOW_SIZE[1] - 40.0 + MODAL_HEADER_HEIGHT);
    Rect::from_center_size(screen.center(), Vec2::new(width, height))
}

/// The body's rectangle inside a card: everything under the header. Pure, and
/// separate from [`modal_card_rect`] so a test can assert that the body and
/// the header do not overlap.
pub fn modal_body_rect(card: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(card.min.x, (card.min.y + MODAL_HEADER_HEIGHT).min(card.max.y)),
        card.max,
    )
}

/// Draws the preferences form as a modal card over a dimmed scrim covering the
/// whole window, returning what the host should do about it.
///
/// **The scrim is a full-window click-catcher on `Order::Foreground`**, the
/// idiom `draw_folder_edit_modal` and `draw_launch_confirm_modal` already use:
/// it sits above the sidebar, list and detail panels *and* above the titlebar,
/// so nothing behind it can be clicked while this is up.
///
/// **`ui.allocate_response(screen.size(), ..)` is what makes that true, and it
/// is not an idiom.** This doc used to say the opposite -- that egui "blocks by
/// layer order rather than reserved pixels", so removing the call "does not let
/// a click through", and it was kept only as a matter of style. That was
/// measured false. On egui 0.35 `Memory::layer_id_at` hit-tests against the
/// `Area`'s **stored rect**, and an area's stored rect is what it allocated: a
/// scrim that allocates nothing has a near-zero rect and blocks nothing outside
/// the card. Deleting this one line lets a click land on a vault control out in
/// the margin while the card sits over the middle of the screen -- and did so
/// with the whole `prefs_ui::` suite green, because the test named for that
/// property was only asserting that a scrim click does not dismiss.
/// `a_click_on_the_scrim_never_reaches_the_vault_behind_it` now asserts on the
/// control behind, at the card and out in the margin, and dies when this line
/// goes.
///
/// Clicking the scrim does
/// **not** dismiss -- neither of the other two modals dismisses on a scrim
/// click either, and a form that is committed as it is typed is the last place
/// to add an accidental exit.
///
/// **Esc and the header ✕ both close**, matching those same two.
///
/// The host is still responsible for the parts a scrim cannot reach: keyboard
/// shortcuts read straight off `ctx.input` bypass hit-testing entirely, so the
/// caller must not run them while this is drawn. See
/// `vault_window`'s Ctrl+K/L/N block.
///
/// **`ctx.input` IS NOT LAYER-AWARE, and only Ctrl+K/L/N are gated.** The gate
/// is `keyboard_shortcuts_enabled`, which the host turns off for those three
/// and for nothing else. Raw text -- anything reaching a `TextEdit` behind this
/// modal, or any other `ctx.input` read the vault window grows later -- is not
/// gated by the scrim at all, because a scrim gates the pointer and nothing
/// else. It is unreachable in production today only because the one route into
/// this modal is a gear click, and clicking the gear surrenders keyboard focus
/// from whatever had it. **That is a coincidence of the current UI, not a
/// guarantee.** A second route in -- a shortcut, a menu item, a restored
/// session that reopens the modal -- would leave focus wherever it was, and the
/// next reader who assumes "the modal is up, so input is blocked" will be
/// wrong. Anything new that reads `ctx.input` must be gated explicitly.
pub fn draw_prefs_modal(ctx: &egui::Context, state: &mut PrefsState) -> PrefsAction {
    let mut action = PrefsAction::None;
    let screen = ctx.content_rect();
    let card = modal_card_rect(screen);

    // **`screen.min`, not `Pos2::ZERO`.** The area's stored rect is
    // `fixed_pos + allocated size`, and that rect is what `layer_id_at`
    // hit-tests; the *painted* rectangle just below is `screen`. Anchored at
    // `Pos2::ZERO` the two agree only while `content_rect().min` is the origin,
    // and where it is not, the scrim looks whole and blocks a `screen.size()`
    // box starting at the wrong corner -- leaving a live strip along the far
    // edges of the window. `content_rect().min` is the origin on every harness
    // and on every window this app opens today, so this is hardening rather
    // than a fix; the point is that the blocked region and the painted region
    // are now derived from the same rectangle instead of agreeing by
    // coincidence.
    egui::Area::new(egui::Id::new("prefs-modal-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.allocate_response(screen.size(), Sense::click());
            ui.painter().rect_filled(
                screen,
                CornerRadius::ZERO,
                egui::Color32::from_black_alpha(MODAL_SCRIM_ALPHA),
            );
        });

    // `fixed_pos`, not `anchor`. An anchored `Area` has to measure its content
    // before it can centre it, so its first frame paints nothing at all -- and
    // this card's geometry is computed here rather than measured, so there is
    // nothing to wait for. See `an_anchored_area_paints_nothing_on_its_first_frame`.
    egui::Area::new(egui::Id::new("prefs-modal"))
        .order(egui::Order::Foreground)
        .fixed_pos(card.min)
        .show(ctx, |ui| {
            // Swallows anything aimed at the card that no control inside it
            // claims. Allocated FIRST so the widgets drawn below -- later in
            // the same layer, and therefore on top -- still win their clicks.
            ui.allocate_rect(card, Sense::click());
            ui.set_clip_rect(card);

            let header = Rect::from_min_max(
                card.min,
                Pos2::new(card.max.x, (card.min.y + MODAL_HEADER_HEIGHT).min(card.max.y)),
            );
            {
                let painter = ui.painter();
                painter.rect_filled(card, CornerRadius::same(MODAL_RADIUS), theme::WINDOW_BG);
                painter.rect_filled(header, CornerRadius::same(MODAL_RADIUS), theme::CARD);
                // Square off the header's bottom corners: the fill above
                // rounds all four, and a rounded bottom edge in the middle of
                // the card reads as two stacked cards.
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(header.min.x, header.max.y - MODAL_RADIUS as f32),
                        header.max,
                    ),
                    CornerRadius::ZERO,
                    theme::CARD,
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(header.min.x, header.max.y - 1.0),
                        header.max,
                    ),
                    CornerRadius::ZERO,
                    theme::HAIRLINE,
                );
                painter.rect_stroke(
                    card,
                    CornerRadius::same(MODAL_RADIUS),
                    Stroke::new(1.0, theme::BORDER),
                    StrokeKind::Inside,
                );
            }

            let galley = ui.painter().layout_no_wrap(
                MODAL_TITLE.to_string(),
                FontId::new(13.0, FontFamily::Proportional),
                theme::INK,
            );
            ui.painter().galley(
                Pos2::new(
                    header.center().x - galley.size().x / 2.0,
                    header.center().y - galley.size().y / 2.0,
                ),
                galley,
                theme::INK,
            );

            // The ✕, in the header's right-hand end. `theme::close_glyph` is
            // the same mark `card_header_with_close` puts on the overlay --
            // drawn as two strokes, because U+2715 is a tofu box in this
            // app's face.
            let close_rect = Rect::from_center_size(
                Pos2::new(header.max.x - 22.0, header.center().y),
                Vec2::splat(16.0),
            );
            let mut close_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(close_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if theme::close_glyph(&mut close_ui).clicked() {
                action = PrefsAction::Close;
            }

            // **The one settings form**, given the body's rect exactly as
            // `run`'s `CentralPanel` gives it the window's.
            let body = modal_body_rect(card);
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
            draw_prefs_body(&mut body_ui, state);
        });

    if action == PrefsAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = PrefsAction::Close;
    }

    action
}

#[cfg(test)]
mod tests {
    //! Real frames of [`draw_prefs_body`] read back through the shapes egui
    //! emitted, using the same headless technique as `vault_window`'s panes.
    //!
    //! What these can and cannot see is worth stating plainly. They can see
    //! every string painted, every rectangle's size and fill, and the state
    //! `draw_prefs_body` left behind -- so "is this section present", "is
    //! there a control here", and "which value is displayed" are all pinned.
    //! They cannot see hover cursors, focus rings, the DWM window rounding, or
    //! whether the result *looks* like 3e; those are checked by eye, and no
    //! test here pretends otherwise.
    use super::*;
    use eframe::egui::epaint::RectShape;

    /// The body's own area: 3e's card minus `ChromeMetrics::LOGIN`'s 40px bar.
    const BODY_SIZE: Vec2 = Vec2::new(WINDOW_SIZE[0], WINDOW_SIZE[1] - 40.0);

    #[derive(Default)]
    struct Painted {
        texts: Vec<(String, Rect)>,
        ink: Vec<TextInk>,
        rects: Vec<RectShape>,
    }

    /// One painted text run, with everything a geometry assertion needs that a
    /// `(String, Rect)` cannot carry: what egui actually laid out (an elided
    /// string is not the string that was asked for), how many lines it wrapped
    /// to, and the colour it was painted in. The colour is here because a
    /// control painted at alpha 0 occupies a perfectly reasonable rectangle
    /// and is not on screen, and a test reading only rectangles says so.
    #[derive(Clone)]
    struct TextInk {
        source: String,
        rendered: String,
        rect: Rect,
        rows: usize,
        color: egui::Color32,
    }

    impl Painted {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn contains(&self, needle: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == needle)
        }

        fn rect_of(&self, needle: &str) -> Rect {
            self.texts
                .iter()
                .find(|(t, _)| t == needle)
                .unwrap_or_else(|| panic!("{needle:?} was never painted; got {:?}", self.strings()))
                .1
        }

        /// Rectangles of exactly the given size, whatever their fill --
        /// how a control is counted without asserting on its colours.
        fn count_of_size(&self, size: Vec2) -> usize {
            self.rects
                .iter()
                .filter(|r| {
                    (r.rect.width() - size.x).abs() < 0.5
                        && (r.rect.height() - size.y).abs() < 0.5
                })
                .count()
        }

        /// Every rectangle of exactly this size, top to bottom -- how a
        /// control that paints no text of its own (the toggle pill) is
        /// located now that the General card holds two of them.
        ///
        /// Sorted by the painted y, not by paint order: "the pill in the
        /// second row" is a claim about where it is on screen, and a test
        /// that indexed paint order would keep passing if the rows were
        /// drawn in one order and laid out in another.
        fn rects_of_size(&self, size: Vec2) -> Vec<Rect> {
            let mut found: Vec<Rect> = self
                .rects
                .iter()
                .filter(|r| {
                    (r.rect.width() - size.x).abs() < 0.5
                        && (r.rect.height() - size.y).abs() < 0.5
                })
                .map(|r| r.rect)
                .collect();
            found.sort_by(|a, b| a.top().total_cmp(&b.top()));
            found
        }

        /// The one rectangle of exactly this size, for a control there is
        /// only ever one of.
        fn only_rect_of_size(&self, size: Vec2) -> Rect {
            let found = self.rects_of_size(size);
            assert_eq!(found.len(), 1, "expected exactly one rectangle of size {size:?}");
            found[0]
        }

        /// The stroke colour of the one rectangle of exactly this size --
        /// how "greyed out" is read back, since the stepper's box paints no
        /// text of its own.
        fn stroke_of_only_rect_of_size(&self, size: Vec2) -> egui::Color32 {
            let mut found = self.rects.iter().filter(|r| {
                (r.rect.width() - size.x).abs() < 0.5 && (r.rect.height() - size.y).abs() < 0.5
            });
            let stroke = found.next().expect("no rectangle of that size was painted").stroke;
            assert!(found.next().is_none(), "more than one rectangle of that size");
            stroke.color
        }

        /// The one painted run of exactly this text. Panics if it was never
        /// painted, and if it was painted twice -- either way the caller's
        /// "the" is wrong and a silent first-match would hide it.
        fn ink_of(&self, needle: &str) -> TextInk {
            let mut found = self.ink.iter().filter(|i| i.source == needle);
            let first = found.next().unwrap_or_else(|| {
                panic!("{needle:?} was never painted; got {:?}", self.strings())
            });
            assert!(found.next().is_none(), "{needle:?} was painted more than once");
            first.clone()
        }

        fn count_filled(&self, fill: egui::Color32) -> usize {
            self.rects.iter().filter(|r| r.fill == fill).count()
        }
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                p.texts.push((text.galley.text().to_string(), rect));
                p.ink.push(TextInk {
                    source: text.galley.text().to_string(),
                    // The glyphs actually placed, row by row -- text that was
                    // elided to fit renders fewer of them than it was given.
                    rendered: text
                        .galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                        .collect(),
                    rect,
                    rows: text.galley.rows.len(),
                    color: text.override_text_color.unwrap_or_else(|| {
                        text.galley
                            .job
                            .sections
                            .first()
                            .map(|section| section.format.color)
                            .unwrap_or(egui::Color32::TRANSPARENT)
                    }),
                });
            }
            egui::Shape::Rect(rect) => p.rects.push(rect.clone()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, p);
                }
            }
            // Everything else is geometry this file does not assert on. A new
            // `egui::Shape` variant carrying text would be egui's to announce,
            // not something an exhaustive match here could usefully catch.
            _ => {}
        }
    }

    /// A context with `theme::apply`'s fonts actually live. The two throwaway
    /// frames are the same ones `detail.rs`'s and `item_list.rs`'s harnesses
    /// run, for the same reason: a font set registered during a frame only
    /// becomes usable at the start of the next one.
    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BODY_SIZE)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn frame(ctx: &egui::Context, state: &mut PrefsState, events: &[egui::Event]) -> Painted {
        let output = ctx.run_ui(raw_input(events), |ui| draw_prefs_body(ui, state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// A full primary press-and-release at `pos`, which is what egui needs to
    /// report `Response::clicked` -- a `PointerButton` press alone is not a
    /// click.
    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// One frame of a fresh window on `section`.
    fn paint(section: Section) -> Painted {
        paint_settings(section, Settings::default())
    }

    /// One frame of General on a pane of a given width. `frame` and its
    /// `raw_input` are pinned to `BODY_SIZE`; the wrapping assertions need
    /// more than one width, and a row that fits at 1000 points is not thereby
    /// known to fit at 652.
    fn paint_general_at(width: f32) -> Painted {
        let size = Vec2::new(width, BODY_SIZE.y);
        let input = |events: &[egui::Event]| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events: events.to_vec(),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(&[]), |_ui| {});
        let mut state = PrefsState::new(Settings::default());
        state.section = Section::General;
        let output = ctx.run_ui(input(&[]), |ui| draw_prefs_body(ui, &mut state));
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    fn paint_settings(section: Section, settings: Settings) -> Painted {
        let ctx = styled_context();
        let mut state = PrefsState::new(settings);
        state.section = section;
        frame(&ctx, &mut state, &[])
    }

    // -- the shell ---------------------------------------------------------

    #[test]
    fn every_nav_section_design_3e_lists_is_painted() {
        let painted = paint(Section::General);
        // The seven labels, spelled out rather than looped over `Section::ALL`:
        // a test that re-derives its expectation from the enum under test
        // would still pass if a section were renamed, removed, or added.
        for label in [
            "General",
            "Autofill",
            "Native apps",
            "Security",
            "Shortcuts",
            "Sync & account",
            "About",
        ] {
            assert!(
                painted.contains(label),
                "nav row {label:?} was not painted; got {:?}",
                painted.strings()
            );
        }
    }

    #[test]
    fn exactly_one_nav_row_is_highlighted() {
        // `BLUE_WASH` is 3e's selected-row fill and appears nowhere else on
        // this window, so counting it counts selections.
        let painted = paint(Section::Autofill);
        assert_eq!(painted.count_filled(theme::BLUE_WASH), 1);
    }

    #[test]
    fn clicking_a_nav_row_opens_that_section() {
        // Without this, every section could be painted and the nav still be
        // decoration -- which is the same defect as a switch that does
        // nothing, one level up.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert_eq!(state.section, Section::General);

        let first = frame(&ctx, &mut state, &[]);
        let target = first.rect_of("About").center();
        let after = frame(&ctx, &mut state, &click(target));

        assert_eq!(state.section, Section::About, "the nav row did not select");
        assert!(
            after.contains(&version_line()),
            "About should now be the open page; got {:?}",
            after.strings()
        );
    }

    #[test]
    fn the_nav_footer_carries_the_real_crate_version() {
        let painted = paint(Section::General);
        assert!(
            painted.contains(&format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))),
            "got {:?}",
            painted.strings()
        );
        assert!(
            !painted.strings().iter().any(|t| t.contains("1.4.0")),
            "\"1.4.0\" is the design document's mock version, not this build's"
        );
    }

    // -- General -----------------------------------------------------------

    #[test]
    fn general_paints_every_setting_that_actually_exists() {
        let painted = paint(Section::General);
        assert!(painted.contains("Keep the Bitwarden backend running"));
        assert!(painted.contains("Lock the vault when idle"));
        assert!(painted.contains(AUTO_LOCK_ENABLED_DESCRIPTION), "got {:?}", painted.strings());
        assert!(painted.contains("Lock the vault after"));
        // The descriptions too: a row whose right-hand control squeezes the
        // text column to nothing still paints its title, so asserting only on
        // titles would not notice.
        assert!(painted.contains(BACKEND_DESCRIPTION), "got {:?}", painted.strings());
        assert!(painted.contains(PROMPT_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(PROMPT_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            PROMPT_DESCRIPTION.contains("CTRL+ALT+B"),
            "the description has to say what is left when the prompt is off -- otherwise the              toggle reads as \"switch autofill off\", which it never is"
        );
        assert!(painted.contains(AUTO_LOCK_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            AUTO_LOCK_DESCRIPTION.contains("One minute is the shortest"),
            "the floor has to be stated on screen, not only enforced"
        );
        assert!(
            painted.contains("15"),
            "the default timeout should be shown in the stepper; got {:?}",
            painted.strings()
        );
    }

    #[test]
    fn general_paints_exactly_six_toggles_and_one_stepper() {
        let painted = paint(Section::General);
        assert_eq!(
            painted.count_of_size(Vec2::new(40.0, 22.0)),
            6,
            "six 40x22 pills: `keep_backend_running`, `prompt_on_match`, `check_breaches`,              `fetch_icons`, `reveal_totp_seed` and `auto_lock_enabled`, and nothing else"
        );
        assert_eq!(
            painted.count_of_size(Vec2::new(112.0, 28.0)),
            1,
            "one 112x28 stepper box: `auto_lock_minutes`"
        );
    }

    #[test]
    fn a_stored_value_below_the_floor_opens_on_the_value_actually_in_effect() {
        // `auto_lock_minutes: 0` is what a hand-written "never lock" looks
        // like, and `auto_lock_timeout` uses one minute for it. Showing "0"
        // here would be a control displaying a number that is not the number
        // in force.
        let painted =
            paint_settings(Section::General, Settings { auto_lock_minutes: 0, ..Settings::default() });
        assert!(painted.contains("1"), "got {:?}", painted.strings());
        assert!(!painted.contains("0"), "got {:?}", painted.strings());
    }

    #[test]
    fn clicking_the_toggle_changes_the_setting_it_is_wired_to() {
        // The whole point of not shipping the other sections' switches: this
        // is what a switch is supposed to do, and it is asserted rather than
        // assumed.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.keep_backend_running, "the default");

        let first = frame(&ctx, &mut state, &[]);
        // The first pill top-to-bottom is the backend row's; the prompt and
        // auto-lock ones are below it. Clicking one must not move either
        // other, which is what the neighbouring assertions here pin.
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[0].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.keep_backend_running);
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.keep_backend_running, "and back again");
        assert!(state.settings.auto_lock_enabled);
    }

    /// **The switch this whole change is about, driven at the pane.**
    ///
    /// The row exists, it is wired to `prompt_on_match`, and it is wired to
    /// THAT field and not to a neighbour -- which is the defect this file has
    /// three rows to make possible. The two neighbours are asserted unmoved
    /// in both directions.
    #[test]
    fn clicking_the_prompt_toggle_turns_the_match_prompt_off_and_on_again() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.prompt_on_match, "the default: a match prompts");

        let first = frame(&ctx, &mut state, &[]);
        // Second pill down, between the backend row and the auto-lock one.
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[1].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(
            !state.settings.prompt_on_match,
            "the prompt toggle did not turn off, so the one control that governs what a              matched window does is inert"
        );
        // What the toggle is FOR, asserted on the value the dispatch actually
        // consumes rather than on the flag alone: a field that flips without
        // reaching `match_disposition` is a switch that does nothing.
        assert_eq!(
            crate::app::match_disposition(state.settings.prompt_on_match),
            crate::app::MatchDisposition::Nothing
        );
        // ... and the hotkey is still armed, which is the whole reason this
        // is a prompt switch and not an autofill switch.
        assert!(
            crate::app::match_arms_hotkey(state.settings.prompt_on_match),
            "turning the prompt off has turned autofill off entirely"
        );
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.prompt_on_match, "and back on again");
        assert_eq!(
            crate::app::match_disposition(state.settings.prompt_on_match),
            crate::app::MatchDisposition::Prompt
        );
    }

    /// **The breach switch, driven at the pane.**
    ///
    /// The counter-assertions are the test: a row wired to `prompt_on_match`
    /// or to `keep_backend_running` would still flip *a* setting on this
    /// click, and an assertion that only read `check_breaches` after the fact
    /// would be satisfied by a row wired to nothing at all if the field
    /// happened to move. All three neighbours start `true` and are asserted
    /// so before the click, which is what makes "unmoved" a claim that can
    /// fail.
    #[test]
    fn clicking_the_breach_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            !state.settings.check_breaches,
            "the default: nothing about a password leaves the machine until this is clicked"
        );
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // Third pill down: backend, prompt, breaches, site icons, TOTP
        // secret, auto-lock.
        let pill = pills[2].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.check_breaches,
            "the breach toggle did not turn on -- the row is painted but its value is never              written back, so the pill is decoration"
        );
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.check_breaches, "and back off again");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
    }

    /// Where the row is, read off the paint rather than off the source order:
    /// `draw_general` could call the rows in any order and lay them out in
    /// another, and "directly under the prompt row" is a claim about the
    /// screen.
    #[test]
    fn the_breach_row_sits_under_the_prompt_row() {
        let painted = paint(Section::General);
        let prompt = painted.ink_of(PROMPT_LABEL).rect;
        let breach = painted.ink_of(BREACH_LABEL).rect;
        let auto_lock = painted.ink_of(AUTO_LOCK_ENABLED_LABEL).rect;
        // The instrument first: three labels at three distinct, non-empty
        // heights, so `top()` is telling them apart and not reading one
        // number three times.
        assert!(prompt.height() > 0.0 && breach.height() > 0.0 && auto_lock.height() > 0.0);
        assert!(
            prompt.top() < breach.top(),
            "the breach row is not under the prompt row: prompt at {prompt:?}, breach at              {breach:?}"
        );
        assert!(
            breach.top() < painted.ink_of(FETCH_ICONS_LABEL).rect.top(),
            "the breach row is not above the site-icons row, so it is not DIRECTLY under the              prompt row"
        );
        assert!(
            breach.top() < auto_lock.top(),
            "the breach row is not above the auto-lock row"
        );
        // The positive control: the two tops differ by a real amount, so the
        // comparison above is telling two rows apart rather than comparing one
        // number with itself -- which is what it would be doing if `rect_of`
        // ever returned the same galley twice.
        assert!(
            breach.top() - prompt.top() > 1.0,
            "the prompt and breach labels are painted at the same height, so the ordering              assertion above cannot fail: prompt {:?}, breach {:?}",
            prompt.top(),
            breach.top()
        );

        // ... and the pills follow the labels, so it is the row that moved
        // and not just its text.
        let pills = painted.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6);
        assert!(pills[1].top() < pills[2].top(), "the breach pill is not below the prompt pill");
        assert!(pills[2].top() < pills[3].top(), "the breach pill is not above the site-icons pill");
        assert!(pills[3].top() < pills[4].top(), "the site-icons pill is not above the TOTP-secret pill");
        assert!(pills[4].top() < pills[5].top(), "the TOTP-secret pill is not above the auto-lock pill");
        assert!(
            pills[2].top() > prompt.bottom(),
            "the breach pill is level with the prompt row's text, so the pills and the labels              disagree about which row is which"
        );
        assert!(
            pills[2].bottom() < auto_lock.top(),
            "the breach pill overhangs the auto-lock row"
        );
    }

    /// **The long copy, at every width this module paints at.**
    ///
    /// `BREACH_DESCRIPTION` is longer than any other row's, so it is the one
    /// that can wrap out of the card or into the row below. Asserted on the
    /// painted galley -- its placed glyphs, its line count and its colour --
    /// rather than on the layout rect the row was allocated, because a row
    /// allocated 570 points and painted 900 wide has a perfectly correct
    /// rect.
    #[test]
    fn the_breach_description_stays_inside_the_pane() {
        assert!(
            BREACH_DESCRIPTION.len() > 200,
            "the copy under test is not the long one, so this test is measuring nothing: {}",
            BREACH_DESCRIPTION.len()
        );
        // Every width the module already paints or measures at: the body's
        // own, and the modal card at both pane sizes `modal_card_rect`'s
        // tests use.
        let widths = [
            BODY_SIZE.x,
            modal_card_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 820.0))).width(),
            modal_card_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0))).width(),
        ];
        let mut visited = 0;
        for width in widths {
            let pane = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, BODY_SIZE.y));
            // The positive control, per width: `contains_rect` has to be able
            // to say no here, or every assertion below is vacuous.
            assert!(
                !pane.contains_rect(Rect::from_min_size(
                    Pos2::new(pane.max.x - 1.0, 0.0),
                    Vec2::new(50.0, 10.0)
                )),
                "`contains_rect` cannot fail at width {width}"
            );

            let painted = paint_general_at(width);
            let ink = painted.ink_of(BREACH_DESCRIPTION);
            assert_eq!(
                ink.rendered.split_whitespace().collect::<Vec<_>>(),
                BREACH_DESCRIPTION.split_whitespace().collect::<Vec<_>>(),
                "the description was elided to fit at width {width}; egui laid {:?}",
                ink.rendered
            );
            assert!(
                ink.color.a() > 0,
                "the description is painted at alpha 0 at width {width}, so every geometry                  assertion here is reading a shape that is not on screen"
            );
            assert!(
                ink.rows >= 2,
                "the long copy laid out in {} line(s) at width {width} -- either it did not                  wrap, or the copy under test is not the long one",
                ink.rows
            );
            assert!(
                pane.contains_rect(ink.rect),
                "the description is painted at {:?}, outside the {width}-wide pane {pane:?}",
                ink.rect
            );
            // **The rows either side of it, and re-pinned deliberately.**
            // This list used to name the auto-lock rows as "the row below",
            // which they were until the site-icons row was inserted between
            // them. They are no longer adjacent to this description AND, at
            // the narrowest width here, the taller card now pushes them out
            // of the painted body altogether -- so `ink_of` panics on them
            // rather than measuring anything.
            //
            // Naming the site-icons row instead is a strengthening rather
            // than a relaxation: an overlap can only happen between rows that
            // are actually next to each other, and `FETCH_ICONS_DESCRIPTION`
            // is the other long one, so this is now the hardest pair on the
            // card rather than a pair separated by two rows.
            for neighbour in [
                PROMPT_LABEL,
                PROMPT_DESCRIPTION,
                BREACH_LABEL,
                FETCH_ICONS_LABEL,
                FETCH_ICONS_DESCRIPTION,
            ] {
                let other = painted.ink_of(neighbour).rect;
                assert!(
                    !ink.rect.intersects(other),
                    "the description at {:?} overlaps {neighbour:?} at {other:?} at width                      {width}",
                    ink.rect
                );
            }
            visited += 1;
        }
        assert_eq!(visited, widths.len(), "a width was skipped");
        assert!(visited >= 3, "fewer widths than the module tests");
    }

    /// **The site-icons switch, driven at the pane**, with the same
    /// counter-assertions the breach row carries and for the same reason: a
    /// row wired to a neighbour would still flip *a* setting on this click.
    ///
    /// This one starts `true`, so the first click turns it OFF -- which is
    /// the direction that matters, and the direction a copy-pasted test
    /// written for an off-by-default row would have got backwards.
    #[test]
    fn clicking_the_site_icons_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            state.settings.fetch_icons,
            "the default: icons are shown until this is clicked"
        );
        assert!(!state.settings.check_breaches, "the neighbour starts false");
        assert!(!state.settings.reveal_totp_seed, "the neighbour starts false");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // Fourth pill down: backend, prompt, breaches, site icons, TOTP
        // secret, auto-lock.
        let pill = pills[3].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            !state.settings.fetch_icons,
            "the site-icons toggle did not turn off -- the row is painted but its value is \
             never written back, so the pill is decoration and the domains keep going out"
        );
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.fetch_icons, "and back on again");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
    }

    /// The copy is on screen, not merely declared -- and it says the thing
    /// the row exists for: WHAT is disclosed (the domain) and what is not
    /// (the credential).
    #[test]
    fn the_site_icons_row_says_the_domain_is_what_is_sent() {
        let painted = paint(Section::General);
        assert!(painted.contains(FETCH_ICONS_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(FETCH_ICONS_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("domain"),
            "the copy has to name what is actually sent; \"downloads icons\" describes the \
             feature and hides the cost"
        );
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("password"),
            "the copy has to say what is NOT sent -- \"it sends the website to Bitwarden\" is \
             what a worried reader assumes, and it is wrong"
        );
        assert!(
            FETCH_ICONS_DESCRIPTION.contains("On by default"),
            "on-by-default is stated in `Settings::default` and has to be stated on screen too \
             -- this is the one network row here that is on unless it is turned off"
        );
        // The instrument: an ink lookup that panics on a double paint, with a
        // real rect, so `contains` above is not reading a zero-size ghost.
        let ink = painted.ink_of(FETCH_ICONS_LABEL);
        assert!(
            ink.rect.height() > 0.0 && ink.rect.width() > 0.0,
            "the label has no box: {:?}",
            ink.rect
        );
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(FETCH_ICONS_DESCRIPTION);
        assert!(desc.color.a() > 0, "the description is painted at alpha {}", desc.color.a());
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
    }

    /// The counter-assertions are the test, exactly as they are for the
    /// breach row above it: a row wired to `check_breaches` or to
    /// `prompt_on_match` would still flip *a* setting on this click, and an
    /// assertion that only read `reveal_totp_seed` afterwards would be
    /// satisfied by that. Every neighbour is asserted at its starting value
    /// BEFORE the click, which is what makes "unmoved" a claim that can fail.
    #[test]
    fn clicking_the_totp_secret_toggle_changes_the_setting_it_is_wired_to() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(
            !state.settings.reveal_totp_seed,
            "the default: no TOTP seed is offered on the details screen until this is clicked"
        );
        assert!(!state.settings.check_breaches, "the neighbour starts false");
        assert!(state.settings.keep_backend_running, "the neighbour starts true");
        assert!(state.settings.prompt_on_match, "the neighbour starts true");
        assert!(state.settings.auto_lock_enabled, "the neighbour starts true");

        let first = frame(&ctx, &mut state, &[]);
        let pills = first.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6, "the General card no longer paints six pills");
        // Fifth pill down: backend, prompt, breaches, site icons, TOTP
        // secret, auto-lock.
        let pill = pills[4].center();

        frame(&ctx, &mut state, &click(pill));
        assert!(
            state.settings.reveal_totp_seed,
            "the TOTP-secret toggle did not turn on -- the row is painted but its value is never written back, so the pill is decoration"
        );
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.reveal_totp_seed, "and back off again");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");
    }

    /// Where the row is, read off the paint rather than off the source order,
    /// for the reason `the_breach_row_sits_under_the_prompt_row` gives.
    #[test]
    fn the_totp_secret_row_sits_between_the_icon_row_and_the_auto_lock_row() {
        let painted = paint(Section::General);
        let breach = painted.ink_of(FETCH_ICONS_LABEL).rect;
        let secret = painted.ink_of(TOTP_SECRET_LABEL).rect;
        let auto_lock = painted.ink_of(AUTO_LOCK_ENABLED_LABEL).rect;
        // The instrument first: three labels at three distinct, non-empty
        // heights, so `top()` is telling them apart rather than reading one
        // number three times.
        assert!(breach.height() > 0.0 && secret.height() > 0.0 && auto_lock.height() > 0.0);
        assert!(
            breach.top() < secret.top(),
            "the TOTP-secret row is not under the site-icons row: icons at {breach:?}, secret at {secret:?}"
        );
        assert!(
            secret.top() < auto_lock.top(),
            "the TOTP-secret row is not above the auto-lock row"
        );
        // The positive control: the tops differ by a real amount, so the
        // comparisons above are telling rows apart and not comparing one
        // number with itself.
        assert!(secret.top() - breach.top() > 1.0);
        assert!(auto_lock.top() - secret.top() > 1.0);

        // ... and the pills follow the labels, so it is the ROW that moved
        // and not just its text.
        let pills = painted.rects_of_size(Vec2::new(40.0, 22.0));
        assert_eq!(pills.len(), 6);
        assert!(pills[3].top() < pills[4].top(), "the TOTP-secret pill is not below the site-icons pill");
        assert!(pills[4].top() < pills[5].top(), "the TOTP-secret pill is not above the auto-lock pill");
        assert!(
            pills[4].top() > breach.bottom(),
            "the TOTP-secret pill is level with the site-icons row's text, so the pills and the labels disagree about which row is which"
        );
        assert!(
            pills[4].bottom() < auto_lock.top(),
            "the TOTP-secret pill overhangs the auto-lock row"
        );
    }

    /// The copy is on screen, not merely declared -- and it says the two
    /// things a user has to know before clicking: that it is off unless they
    /// turn it on, and that what it adds is a MASKED row rather than a seed
    /// painted in the clear.
    #[test]
    fn the_totp_secret_row_says_what_it_turns_on_and_that_it_is_off_by_default() {
        let painted = paint(Section::General);
        assert!(painted.contains(TOTP_SECRET_LABEL), "got {:?}", painted.strings());
        assert!(painted.contains(TOTP_SECRET_DESCRIPTION), "got {:?}", painted.strings());
        assert!(
            TOTP_SECRET_LABEL.contains("TOTP secret"),
            "the label has to name the thing it reveals, not just say \"secret\""
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("Off by default"),
            "off-by-default is stated in `Settings::default` and has to be stated on screen too"
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("masked"),
            "turning this on adds a MASKED row; copy that implied a seed appears in the clear would be wrong"
        );
        assert!(
            TOTP_SECRET_DESCRIPTION.contains("details screen") || TOTP_SECRET_DESCRIPTION.contains("one-time code"),
            "the copy has to say WHERE the row appears"
        );
        // The instrument: an ink lookup that panics on a double paint, with a
        // real rect, so "contains" above is not reading a zero-size ghost.
        let ink = painted.ink_of(TOTP_SECRET_LABEL);
        assert!(ink.rect.height() > 0.0 && ink.rect.width() > 0.0, "the label has no box: {:?}", ink.rect);
        assert!(ink.color.a() > 0, "the label is painted at alpha {}", ink.color.a());
        let desc = painted.ink_of(TOTP_SECRET_DESCRIPTION);
        assert!(desc.color.a() > 0, "the description is painted at alpha {}", desc.color.a());
        assert!(desc.rows >= 2, "a description this long should wrap; it took {} row(s)", desc.rows);
    }

    #[test]
    fn clicking_the_auto_lock_toggle_turns_auto_lock_off_and_on_again() {
        // The user's actual request. `auto_lock_enabled` starts true, and
        // the SIXTH pill down is the one wired to it -- `prompt_on_match`,
        // `check_breaches`, `fetch_icons` and `reveal_totp_seed` sit between
        // it and the backend row.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.auto_lock_enabled, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[5].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.auto_lock_enabled, "the auto-lock toggle did not turn off");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
        assert!(state.settings.prompt_on_match, "the wrong row's toggle moved");
        assert!(!state.settings.check_breaches, "the wrong row's toggle moved");
        assert!(state.settings.fetch_icons, "the wrong row's toggle moved");
        assert!(!state.settings.reveal_totp_seed, "the wrong row's toggle moved");
        // What the toggle is FOR, asserted on the value the vault window
        // actually consumes rather than on the flag: a field that flips
        // without reaching `auto_lock` is a switch that does nothing.
        assert_eq!(state.settings.auto_lock(), crate::settings::AutoLock::Never);

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.auto_lock_enabled, "and back on again");
        assert_eq!(
            state.settings.auto_lock(),
            crate::settings::AutoLock::After(std::time::Duration::from_secs(15 * 60)),
            "turning it back on must restore the minutes that were on screen the whole time"
        );
    }

    #[test]
    fn the_minutes_stepper_is_greyed_while_auto_lock_is_off() {
        // "Greyed" is the visible half; the click tests below are the half
        // that matters. Read back off the stepper box's own stroke, with the
        // enabled case as the positive control -- without it, a stepper that
        // painted `HAIRLINE` in both states would pass.
        let off = paint_settings(
            Section::General,
            Settings { auto_lock_enabled: false, ..Settings::default() },
        );
        let on = paint_settings(Section::General, Settings::default());
        let stepper = Vec2::new(112.0, 28.0);
        assert_eq!(
            on.stroke_of_only_rect_of_size(stepper),
            theme::BORDER_STRONG,
            "with auto-lock on the stepper is 3e's ordinary segmented control"
        );
        assert_eq!(
            off.stroke_of_only_rect_of_size(stepper),
            theme::HAIRLINE,
            "with auto-lock off the stepper must read as disabled"
        );
        assert_ne!(theme::BORDER_STRONG, theme::HAIRLINE, "the two greys have to differ at all");
    }

    /// The number sits in the middle of its cell in BOTH states.
    ///
    /// Both halves are load-bearing and neither is redundant. The greyed
    /// branch paints an explicit galley and was always centred; the live one
    /// hands the placement to a `TextEdit` and was measured 6.0pt right and
    /// 3.5pt high of the cell centre -- visible as the number jumping when
    /// the toggle is flipped. Asserting only the live branch would let a
    /// future edit break the greyed one silently, and asserting only that the
    /// two AGREE would be satisfied by both being wrong together, so each is
    /// checked against the cell it is drawn in.
    #[test]
    fn the_minutes_number_is_centred_in_its_cell_in_both_states() {
        let minutes = clamp_auto_lock_minutes(Settings::default().auto_lock_minutes).to_string();
        for (state, painted) in [
            ("live", paint_settings(Section::General, Settings::default())),
            (
                "greyed",
                paint_settings(
                    Section::General,
                    Settings { auto_lock_enabled: false, ..Settings::default() },
                ),
            ),
        ] {
            let outer = painted.only_rect_of_size(Vec2::new(
                STEPPER_STEP_WIDTH * 2.0 + STEPPER_VALUE_WIDTH,
                STEPPER_HEIGHT,
            ));
            // The value cell is the middle segment, between the two end
            // buttons -- derived from the same constants `minutes_stepper`
            // lays the control out with, so this cannot drift from it.
            let cell = Rect::from_min_size(
                Pos2::new(outer.min.x + STEPPER_STEP_WIDTH, outer.min.y),
                Vec2::new(STEPPER_VALUE_WIDTH, STEPPER_HEIGHT),
            );
            let number = painted.rect_of(&minutes);
            // Half a point: the two branches lay the glyphs out by different
            // routes and their widths differ by ~0.1pt, so an exact equality
            // would be measuring rounding rather than centring. The defect
            // this catches was 6.0 and 3.5.
            assert!(
                (number.center().x - cell.center().x).abs() < 0.5,
                "{state}: the minutes number is not horizontally centred in its cell -- \
                 number centre {:?}, cell centre {:?}",
                number.center(),
                cell.center()
            );
            assert!(
                (number.center().y - cell.center().y).abs() < 0.5,
                "{state}: the minutes number is not vertically centred in its cell -- \
                 number centre {:?}, cell centre {:?}",
                number.center(),
                cell.center()
            );
        }
    }

    #[test]
    fn the_minutes_stepper_still_shows_its_value_while_auto_lock_is_off() {
        // Greyed, not hidden: the number the toggle will restore has to stay
        // legible, so this is not satisfied by a row that disappears.
        let painted = paint_settings(
            Section::General,
            Settings { auto_lock_enabled: false, auto_lock_minutes: 42, ..Settings::default() },
        );
        assert!(painted.contains("Lock the vault after"), "got {:?}", painted.strings());
        assert!(painted.contains("42"), "got {:?}", painted.strings());
        assert_eq!(
            painted.count_of_size(Vec2::new(112.0, 28.0)),
            1,
            "the stepper box is still drawn"
        );
    }

    #[test]
    fn the_steppers_buttons_are_inert_while_auto_lock_is_off() {
        // A click test, not a colour check: a control that is painted grey
        // and still responds is the exact defect this repo keeps re-writing.
        // Every assertion here is paired with the same click on an enabled
        // stepper, so "the stepper never works" cannot pass it.
        let ctx = styled_context();
        let mut off =
            PrefsState::new(Settings { auto_lock_enabled: false, ..Settings::default() });
        let painted = frame(&ctx, &mut off, &[]);
        let plus = painted.rect_of("+").center();
        let minus = painted.rect_of("-").center();

        frame(&ctx, &mut off, &click(plus));
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled + stepped the value");
        frame(&ctx, &mut off, &click(minus));
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled - stepped the value");

        let mut on = PrefsState::new(Settings::default());
        let painted = frame(&ctx, &mut on, &[]);
        assert_eq!(
            (painted.rect_of("+").center(), painted.rect_of("-").center()),
            (plus, minus),
            "the two states must put their buttons in the same place, or the clicks above \
             missed rather than being refused"
        );
        frame(&ctx, &mut on, &click(plus));
        assert_eq!(on.settings.auto_lock_minutes, 16, "positive control: the + does work");
        frame(&ctx, &mut on, &click(minus));
        assert_eq!(on.settings.auto_lock_minutes, 15, "positive control: the - does work");
    }

    #[test]
    fn the_minutes_field_cannot_be_typed_into_while_auto_lock_is_off() {
        // The other half of "non-interactive": the buttons are inert above,
        // and there is no text widget left in the middle either -- clicking
        // it takes no focus, so the keystrokes go nowhere.
        let ctx = styled_context();
        let mut off =
            PrefsState::new(Settings { auto_lock_enabled: false, ..Settings::default() });
        let painted = frame(&ctx, &mut off, &[]);
        // The middle cell of the 112x28 box, i.e. where the value sits.
        let field = painted.only_rect_of_size(Vec2::new(112.0, 28.0)).center();
        frame(&ctx, &mut off, &click(field));
        frame(&ctx, &mut off, &[egui::Event::Text("7".into())]);
        frame(&ctx, &mut off, &[]);
        assert_eq!(off.settings.auto_lock_minutes, 15, "the disabled field accepted typing");
        assert_eq!(off.auto_lock_text, "15", "and its buffer must not drift either");

        let mut on = PrefsState::new(Settings::default());
        let painted = frame(&ctx, &mut on, &[]);
        let field = painted.only_rect_of_size(Vec2::new(112.0, 28.0)).center();
        frame(&ctx, &mut on, &click(field));
        frame(&ctx, &mut on, &[egui::Event::Text("7".into())]);
        frame(&ctx, &mut on, &[]);
        assert!(
            on.auto_lock_text.contains('7') && on.auto_lock_text != "15",
            "positive control: with auto-lock on the same click and keystroke DO reach the \
             field (so the assertion above is about the disabled state, not about the harness \
             being unable to type at all); the buffer is {:?}",
            on.auto_lock_text
        );
    }

    #[test]
    fn the_steppers_buttons_move_the_stored_timeout() {
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert_eq!(state.settings.auto_lock_minutes, 15, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let plus = first.rect_of("+").center();
        let minus = first.rect_of("-").center();

        frame(&ctx, &mut state, &click(plus));
        assert_eq!(state.settings.auto_lock_minutes, 16);
        let after = frame(&ctx, &mut state, &click(minus));
        assert_eq!(state.settings.auto_lock_minutes, 15);
        assert!(
            after.contains("15"),
            "the field has to follow the buttons; got {:?}",
            after.strings()
        );
    }

    #[test]
    fn the_steppers_minus_is_inert_at_the_floor() {
        // Not merely clamped afterwards: at one minute there is nothing below,
        // and a button that accepts the click and refuses the change is the
        // same lie as a switch that does nothing.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings { auto_lock_minutes: 1, ..Settings::default() });
        let first = frame(&ctx, &mut state, &[]);
        let minus = first.rect_of("-").center();
        frame(&ctx, &mut state, &click(minus));
        assert_eq!(state.settings.auto_lock_minutes, 1);
    }

    // -- sections with nothing behind them ---------------------------------

    #[test]
    fn a_section_with_no_settings_says_so_and_paints_no_control() {
        for (section, detail) in [
            (Section::Autofill, "Overlay behaviour is fixed for now."),
            (Section::NativeApps, "Matches are added from the tray's"),
            (Section::Security, "Auto-lock is on the General page."),
            (Section::SyncAndAccount, "Signing in, syncing and locking are all done"),
        ] {
            let painted = paint(section);
            assert!(
                painted.contains("Nothing to configure here yet"),
                "{:?} should say it is empty; got {:?}",
                section,
                painted.strings()
            );
            assert!(
                painted.strings().iter().any(|t| t.contains(detail)),
                "{section:?} says it is empty but not what governs it today; got {:?}",
                painted.strings()
            );
            // The point of the whole exercise: no dead switch, and no dead
            // stepper either.
            assert_eq!(
                painted.count_of_size(Vec2::new(40.0, 22.0)),
                0,
                "{section:?} painted a toggle pill it cannot honour"
            );
            assert_eq!(
                painted.count_of_size(Vec2::new(112.0, 28.0)),
                0,
                "{section:?} painted a stepper it cannot honour"
            );
        }
    }

    #[test]
    fn shortcuts_reports_the_one_shortcut_that_exists() {
        let painted = paint(Section::Shortcuts);
        assert!(painted.contains("Fill the focused app"));
        assert!(painted.contains("CTRL+ALT+B"), "got {:?}", painted.strings());
        assert_eq!(
            painted.count_of_size(Vec2::new(40.0, 22.0)),
            0,
            "a shortcut is reported here, not rebound"
        );
    }

    #[test]
    fn the_shortcuts_page_names_the_hotkey_that_is_actually_registered() {
        // A source-text guard, the same device as `settings.rs`'s
        // `the_config_path_still_matches_the_one_main_resolves`: `FILL_HOTKEY`
        // is a display string with no compile-time link to
        // `hotkey::register_fill_hotkey`, so changing the registered chord
        // would otherwise leave this window confidently naming the old one.
        assert_eq!(FILL_HOTKEY, "CTRL+ALT+B");
        // **Read whole, and that is checked, not assumed.** `settings.rs` and
        // `item_list.rs` count their cross-file needles over the read file's
        // production half, because a fixture in another module's test code
        // can satisfy a presence pin that production has stopped satisfying.
        // `hotkey.rs` has no test code at all -- 27 lines, no `cfg(test)`
        // anywhere -- so there is no fixture here to be fooled by, and a walk
        // that cut test modules out would cut nothing. The assertion below
        // keeps that true: the day `hotkey.rs` grows a test module, this
        // fires and these two pins should move to a production half.
        let hotkey_rs = include_str!("hotkey.rs");
        assert_eq!(
            hotkey_rs.matches(concat!("cfg(", "test)")).count(),
            0,
            "`hotkey.rs` has grown test code, so the two whole-file presence pins below can now \
             be satisfied by a fixture instead of by the registration they guard -- read its \
             production half, the way `settings.rs` reads `main.rs`"
        );
        assert!(
            hotkey_rs.contains("Modifiers::CONTROL | Modifiers::ALT"),
            "hotkey.rs no longer registers Ctrl+Alt -- `FILL_HOTKEY` says it does"
        );
        assert!(
            hotkey_rs.contains("Code::KeyB"),
            "hotkey.rs no longer registers B -- `FILL_HOTKEY` says it does"
        );
    }

    // -- About -------------------------------------------------------------

    #[test]
    fn about_paints_the_real_crate_version_and_not_the_designs_mock_one() {
        let painted = paint(Section::About);
        assert!(painted.contains("Version"));
        assert!(
            painted.contains(&format!("Deskwarden {}", env!("CARGO_PKG_VERSION"))),
            "got {:?}",
            painted.strings()
        );
        assert!(
            !painted.strings().iter().any(|t| t.contains("1.4.0")),
            "got {:?}",
            painted.strings()
        );
    }

    #[test]
    fn about_does_not_claim_an_account_is_linked() {
        // 3e's "Bitwarden account linked" is a claim this window has no data
        // for: `main.rs` holds the status and does not pass it in. Asserting
        // it were true would be a lie on screen for an unauthenticated user.
        let painted = paint(Section::About);
        assert!(painted.contains("Bitwarden account"));
        assert!(
            !painted.strings().iter().any(|t| t.contains("linked")),
            "got {:?}",
            painted.strings()
        );
        assert!(painted.contains(ACCOUNT_STATUS));
    }

    // -- the numeric control, as pure functions ----------------------------

    #[test]
    fn a_typed_entry_below_the_floor_commits_as_the_floor() {
        // Absolute values throughout: re-deriving these from
        // `clamp_auto_lock_minutes` would make the test pass for any floor,
        // including a broken one.
        assert_eq!(parse_minutes_entry("0", 15), 1);
        assert_eq!(parse_minutes_entry("1", 15), 1);
        assert_eq!(parse_minutes_entry("45", 15), 45);
        assert_eq!(parse_minutes_entry("  30  ", 15), 30);
    }

    #[test]
    fn a_typed_entry_that_is_not_a_number_leaves_the_value_alone() {
        assert_eq!(parse_minutes_entry("", 15), 15);
        assert_eq!(parse_minutes_entry("soon", 15), 15);
        assert_eq!(parse_minutes_entry("-5", 15), 15, "u64 cannot be negative");
        assert_eq!(parse_minutes_entry("7.5", 15), 15);
        assert_eq!(
            parse_minutes_entry("99999999999999999999999", 15),
            15,
            "too large for u64: the previous value stands rather than saturating \
             the user into a century-long timeout they did not ask for"
        );
        // ...and a previous value that was itself out of range is still
        // repaired on the way through.
        assert_eq!(parse_minutes_entry("", 0), 1);
    }

    #[test]
    fn the_steppers_arithmetic_stops_at_both_ends() {
        assert_eq!(decrement_minutes(15), 14);
        assert_eq!(decrement_minutes(2), 1);
        assert_eq!(decrement_minutes(1), 1, "the floor, not zero");
        assert_eq!(decrement_minutes(0), 1);
        assert_eq!(increment_minutes(1), 2);
        assert_eq!(increment_minutes(15), 16);
        assert_eq!(increment_minutes(u64::MAX), u64::MAX, "saturating, not panicking");
    }
}

/// Real frames of [`draw_prefs_modal`] -- the shell around the form, not the
/// form itself, which [`tests`] above already reads shape by shape.
///
/// What is worth pinning here is everything the shell is *for*: the card sits
/// inside the pane with the dimmed vault visible around it, the header's title
/// and dismiss control do not collide, the two dismiss routes work and the
/// third (a scrim click) deliberately does not -- and, above all, that a
/// control behind the scrim cannot be clicked. That last one is the defect
/// this whole feature exists to prevent: a modal that merely *covers* the
/// vault, with its buttons still live underneath, is worse than no modal.
#[cfg(test)]
mod modal_tests {
    use super::*;
    use eframe::egui::Color32;

    /// A vault-window-sized pane: larger than the card's ceiling on one axis
    /// and not the other, so the clamp and the margin are both exercised.
    const PANE: Vec2 = Vec2::new(1200.0, 820.0);

    /// The stand-in vault control, in the dead centre of the pane -- i.e.
    /// under the card, not merely under the scrim.
    const BEHIND: Rect = Rect {
        min: Pos2::new(560.0, 400.0),
        max: Pos2::new(680.0, 428.0),
    };

    /// A second stand-in, out in the margin the card does not cover. This one
    /// is the SCRIM's job and nothing else's: the card's own area cannot
    /// shield it, so a test that only ever clicked `BEHIND` would pass with no
    /// scrim at all. `the_card_alone_does_not_cover_the_margin` keeps that
    /// distinction honest.
    const BEHIND_IN_MARGIN: Rect = Rect {
        min: Pos2::new(2.0, 2.0),
        max: Pos2::new(20.0, 18.0),
    };

    /// A third stand-in, in the margin at the FAR corner of the pane.
    ///
    /// `BEHIND_IN_MARGIN` is at the top left, which every rectangle anchored at
    /// `Pos2::ZERO` covers -- including a scrim that allocated only the card's
    /// size instead of the screen's. That mutation passed every assertion in
    /// this module. This fixture is the other end: a scrim has to have
    /// allocated the whole pane to shield it.
    const BEHIND_IN_FAR_MARGIN: Rect = Rect {
        min: Pos2::new(PANE.x - 22.0, PANE.y - 20.0),
        max: Pos2::new(PANE.x - 4.0, PANE.y - 4.0),
    };

    // -----------------------------------------------------------------------
    // The pure geometry, asked directly. No frame, no fonts, no harness.
    // -----------------------------------------------------------------------

    #[test]
    fn the_card_is_inset_on_every_side_so_the_vault_stays_visible_around_it() {
        // A pane small enough that the ceiling does not bite: the card is
        // margin-bound on both axes.
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0));
        let card = modal_card_rect(screen);
        assert_eq!(card.min.x - screen.min.x, MODAL_SCREEN_MARGIN);
        assert_eq!(screen.max.x - card.max.x, MODAL_SCREEN_MARGIN);
        assert_eq!(card.min.y - screen.min.y, MODAL_SCREEN_MARGIN);
        assert_eq!(screen.max.y - card.max.y, MODAL_SCREEN_MARGIN);
    }

    #[test]
    fn the_card_never_grows_past_the_designs_own_size_on_a_huge_window() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(3840.0, 2160.0));
        let card = modal_card_rect(screen);
        assert_eq!(card.width(), WINDOW_SIZE[0]);
        assert_eq!(card.height(), WINDOW_SIZE[1] - 40.0 + MODAL_HEADER_HEIGHT);
        // Still centred, which is the "do not make me hunt across a big
        // screen for the same window" half of the request.
        assert_eq!(card.center(), screen.center());
    }

    /// The vault window is resizable and its minimum is well under 3e's
    /// 1000x740. A card that kept that size on a small window would put its
    /// header -- and therefore its only mouse dismiss -- off the edge.
    #[test]
    fn the_card_never_spills_out_of_a_window_smaller_than_the_design() {
        for size in [Vec2::new(760.0, 520.0), Vec2::new(400.0, 300.0), Vec2::new(60.0, 40.0)] {
            let screen = Rect::from_min_size(Pos2::new(17.0, 23.0), size);
            let card = modal_card_rect(screen);
            assert!(
                screen.contains_rect(card),
                "a {size:?} window puts the card at {card:?}, outside the pane {screen:?}"
            );
        }
    }

    #[test]
    fn the_body_starts_below_the_header_and_never_overlaps_it() {
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let body = modal_body_rect(card);
        let header = Rect::from_min_max(
            card.min,
            Pos2::new(card.max.x, card.min.y + MODAL_HEADER_HEIGHT),
        );
        assert!(card.contains_rect(body));
        assert_eq!(body.min.y, header.max.y);
        assert!(
            !body.intersects(Rect::from_min_max(
                header.min,
                Pos2::new(header.max.x, header.max.y - 0.01)
            )),
            "the form is drawn under the title bar it is supposed to sit beneath"
        );
    }

    // -----------------------------------------------------------------------
    // Frames
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct Shot {
        /// `(source, rendered, rect)` -- the second is not the first: see
        /// `detail.rs`'s `collect_rendered_text`. `Galley::text()` is the
        /// string that was HANDED to egui and is blind to truncation.
        texts: Vec<(String, String, Rect)>,
        fills: Vec<(Rect, Color32)>,
    }

    impl Shot {
        fn find(&self, source: &str) -> Option<&(String, String, Rect)> {
            self.texts.iter().find(|(s, _, _)| s == source)
        }

        fn sources(&self) -> Vec<&str> {
            self.texts.iter().map(|(s, _, _)| s.as_str()).collect()
        }

        fn rect_of(&self, source: &str) -> Rect {
            self.find(source)
                .unwrap_or_else(|| {
                    panic!("{source:?} was never painted; got {:?}", self.sources())
                })
                .2
        }
    }

    /// The `aae9429` contract, kept: a label counts as visible only if its
    /// rect is INSIDE the pane **and** the glyphs egui really laid are the
    /// glyphs it was handed. Either half alone passes a label that has been
    /// ellipsised to fit, or one drawn in full off the edge.
    fn assert_visible(shot: &Shot, source: &str, pane: Rect) {
        let (_, rendered, rect) = shot
            .find(source)
            .unwrap_or_else(|| panic!("{source:?} was never painted; got {:?}", shot.sources()));
        assert!(
            pane.contains_rect(*rect),
            "{source:?} is painted at {rect:?}, outside {pane:?}"
        );
        assert_eq!(
            rendered, source,
            "{source:?} was elided to fit -- egui laid {rendered:?}"
        );
    }

    fn walk(shape: &egui::Shape, out: &mut Shot) {
        match shape {
            egui::Shape::Text(text) => {
                let rendered: String = text
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect();
                out.texts.push((
                    text.galley.text().to_string(),
                    rendered,
                    Rect::from_min_size(text.pos, text.galley.size()),
                ));
            }
            egui::Shape::Rect(rect) => out.fills.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    fn raw_input(events: &[egui::Event]) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, PANE)),
            events: events.to_vec(),
            ..Default::default()
        }
    }

    fn styled_context() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(&[]), |_ui| {});
        ctx
    }

    fn a_state() -> PrefsState {
        PrefsState::new(Settings::default())
    }

    /// One frame of the modal, drawn over a stand-in for the vault: a button
    /// in the middle of the pane, added BEFORE the modal exactly as the real
    /// window's panels are. Returns what was painted, what the modal asked
    /// for, and whether that button registered a click.
    fn frame(
        ctx: &egui::Context,
        state: &mut PrefsState,
        events: &[egui::Event],
        with_modal: bool,
    ) -> (Shot, PrefsAction, Behind) {
        let mut action = PrefsAction::None;
        let mut behind = Behind::default();
        let output = ctx.run_ui(raw_input(events), |ui| {
            behind.under_card = ui
                .put(BEHIND, egui::Button::new("a vault control"))
                .clicked();
            behind.in_margin = ui
                .put(BEHIND_IN_MARGIN, egui::Button::new("another"))
                .clicked();
            behind.in_far_margin = ui
                .put(BEHIND_IN_FAR_MARGIN, egui::Button::new("a third"))
                .clicked();
            if with_modal {
                action = draw_prefs_modal(ui.ctx(), state);
            }
        });
        let mut shot = Shot::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut shot);
        }
        (shot, action, behind)
    }

    /// Whether each stand-in vault control took a click on this frame.
    #[derive(Default)]
    struct Behind {
        under_card: bool,
        in_margin: bool,
        in_far_margin: bool,
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// Where the header's dismiss mark is. It is drawn as two strokes rather
    /// than the character U+2715 (a tofu box in this app's face), so it cannot
    /// be found by name and its reserved space is computed instead -- the same
    /// arithmetic `draw_prefs_modal` uses.
    fn close_rect(card: Rect) -> Rect {
        Rect::from_center_size(
            Pos2::new(card.max.x - 22.0, card.min.y + MODAL_HEADER_HEIGHT / 2.0),
            Vec2::splat(16.0),
        )
    }

    // -----------------------------------------------------------------------
    // The warm-up, and why the card is not an anchored `Area`
    // -----------------------------------------------------------------------

    /// **The control this whole harness rests on.** An `Area` that has to
    /// CENTRE itself cannot place anything until it has measured its content,
    /// so its first frame emits nothing but `Shape::Noop`. A test that read
    /// frame 1 of such an area would be asserting about a blank screen, and
    /// every "does not contain" check in this module would pass for the wrong
    /// reason. Pinned here so the day it stops being true, this says so.
    #[test]
    fn an_anchored_area_paints_nothing_on_its_first_frame() {
        let ctx = styled_context();
        let output = ctx.run_ui(raw_input(&[]), |ui| {
            egui::Area::new(egui::Id::new("anchored-control"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    ui.label("nothing here on frame one");
                });
        });
        let mut shot = Shot::default();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut shot);
        }
        assert!(
            shot.find("nothing here on frame one").is_none(),
            "an anchored area painted on its first frame after all -- every first-frame \
             assertion in this module needs revisiting"
        );
    }

    /// **And this modal is no exception, `fixed_pos` or not.** Measured, not
    /// assumed: an `Area` egui has never seen before paints nothing at all on
    /// the frame it is created, and asks for another. So EVERY frame test
    /// below runs a warm-up first -- and this one pins that the warm-up is
    /// really necessary, so that none of them is quietly asserting about a
    /// blank screen.
    ///
    /// The cost in the real window is one frame between the gear's click and
    /// the card appearing, which egui has already requested a repaint for.
    #[test]
    fn the_modal_is_blank_on_its_first_frame_and_complete_on_its_second() {
        let ctx = styled_context();
        let mut state = a_state();
        let (warm_up, _, _) = frame(&ctx, &mut state, &[], true);
        assert!(
            warm_up.find(MODAL_TITLE).is_none(),
            "the modal painted on its first frame after all -- the warm-up every frame test \
             below runs is no longer needed, and each of them should say so instead: {:?}",
            warm_up.sources()
        );

        let (shot, _, _) = frame(&ctx, &mut state, &[], true);
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        assert_visible(&shot, MODAL_TITLE, card);
        // And the form inside it, not just the shell.
        assert_visible(&shot, Section::General.label(), card);
        assert!(
            shot.find(AUTO_LOCK_LABEL).is_some(),
            "the first frame drew the shell but not the settings form; got {:?}",
            shot.sources()
        );
    }

    // -----------------------------------------------------------------------
    // The shell
    // -----------------------------------------------------------------------

    /// A title long enough to reach the dismiss mark would overprint the only
    /// mouse way out this modal has. Asserted as non-intersection, explicitly,
    /// because both are in the same 44px strip.
    #[test]
    fn the_title_does_not_reach_the_dismiss_control() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (shot, _, _) = frame(&ctx, &mut state, &[], true);
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let title = shot.rect_of(MODAL_TITLE);
        let close = close_rect(card);
        assert!(
            !title.intersects(close),
            "the title {title:?} runs into the dismiss mark at {close:?}"
        );
        assert!(card.contains_rect(close), "the dismiss mark is outside the card");
    }

    /// The dim itself. Without it the vault behind reads as live, which is the
    /// visual half of the same claim the click tests make mechanically.
    #[test]
    fn the_whole_pane_is_dimmed_behind_the_card() {
        let ctx = styled_context();
        let mut state = a_state();
        // More than the two-frame warm-up the other tests need: egui fades a
        // new `Area` in over `Style::animation_time`, so an early frame's
        // scrim is a fraction of its final alpha. Read once it has settled --
        // this is an assertion about the colour that was chosen, not about the
        // fade, which is egui's and is fine.
        let mut shot = Shot::default();
        for _ in 0..24 {
            shot = frame(&ctx, &mut state, &[], true).0;
        }
        let pane = Rect::from_min_size(Pos2::ZERO, PANE);
        let scrim = shot
            .fills
            .iter()
            .find(|(rect, _)| *rect == pane)
            .unwrap_or_else(|| {
                panic!(
                    "no full-pane rectangle was painted at all; got {:?}",
                    shot.fills.iter().map(|(r, _)| *r).collect::<Vec<_>>()
                )
            })
            .1;
        assert_eq!(
            (scrim.r(), scrim.g(), scrim.b()),
            (0, 0, 0),
            "the scrim is not black, so it tints the vault rather than dimming it"
        );
        assert_eq!(
            scrim.a(),
            MODAL_SCRIM_ALPHA,
            "the scrim's alpha is not the one `folder_modal` and the launch confirmation use"
        );
        assert!(
            scrim.a() < 255,
            "the scrim is opaque, so the vault is hidden rather than dimmed -- the whole              point is that the window the user came from stays visible where it was"
        );
    }

    // -----------------------------------------------------------------------
    // Inertness -- the reason this feature exists
    // -----------------------------------------------------------------------

    /// **The control, first.** Without it every assertion below would pass
    /// against a fixture whose button was never clickable in the first place.
    #[test]
    fn the_control_behind_the_modal_is_clickable_when_the_modal_is_not_there() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), false);
        assert!(
            behind.under_card,
            "the stand-in vault control under the card never registered a click at all"
        );
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND_IN_MARGIN.center()), false);
        assert!(
            behind.in_margin,
            "the stand-in vault control in the margin never registered a click at all"
        );
        let _ = frame(&ctx, &mut state, &[], false);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND_IN_FAR_MARGIN.center()), false);
        assert!(
            behind.in_far_margin,
            "the stand-in vault control in the FAR margin never registered a click at all, so \
             the assertion that the scrim shields it proves nothing"
        );
    }

    /// **The other control, and the one that keeps the scrim from being dead
    /// code.** The card's own area covers `BEHIND`, so a click there would be
    /// blocked by the card whether or not a scrim existed. `BEHIND_IN_MARGIN`
    /// is deliberately outside it -- measured here rather than assumed.
    #[test]
    fn the_card_alone_does_not_cover_the_margin() {
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        assert!(card.contains_rect(BEHIND));
        assert!(
            !card.intersects(BEHIND_IN_MARGIN),
            "the margin fixture is under the card, so the scrim test below would pass              against no scrim at all"
        );
        assert!(
            !card.intersects(BEHIND_IN_FAR_MARGIN),
            "the far-margin fixture is under the card, so the scrim test below would pass \
             against no scrim at all"
        );
        // And the two margin fixtures are on opposite sides of the card, which
        // is the whole reason there are two: a scrim anchored at `Pos2::ZERO`
        // that under-allocates covers the near one and not the far one.
        assert!(
            BEHIND_IN_MARGIN.max.x < card.min.x && BEHIND_IN_MARGIN.max.y < card.min.y,
            "the near margin fixture is not before the card on both axes"
        );
        assert!(
            BEHIND_IN_FAR_MARGIN.min.x > card.max.x && BEHIND_IN_FAR_MARGIN.min.y > card.max.y,
            "the far margin fixture is not past the card on both axes, so it does not catch a \
             scrim that allocated too little"
        );
    }

    /// **The defect this feature exists to prevent.** A click that lands on a
    /// vault control behind a scrim is worse than no modal: the user believes
    /// they are editing preferences and is in fact driving the vault.
    #[test]
    fn a_click_over_the_card_never_reaches_the_vault_behind_it() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), true);
        assert!(
            !behind.under_card,
            "a click over the preferences card reached the vault control underneath it"
        );
    }

    /// And the same in the margin: the scrim is a click-catcher over the whole
    /// pane, not only under the card.
    ///
    /// **This test used to assert only that a scrim click does not DISMISS.**
    /// It bound `(_, action, _)`, threw the `Behind` away, and never looked at
    /// the one property its own name promises -- so `BEHIND_IN_MARGIN`, set up
    /// for exactly this and kept honest by `the_card_alone_does_not_cover_the_
    /// margin`, was exercised by the positive control and by nothing else.
    /// Deleting the scrim's `allocate_response` let a margin click through to
    /// the vault with the entire shipped `prefs_ui::` suite green (41 passed).
    /// Both halves are asserted now, and the dismissal claim is kept alongside
    /// them rather than instead of them.
    ///
    /// **The margin is the load-bearing half.** A click over the card is
    /// blocked by the card's own area whether or not a scrim exists, so it is
    /// asserted here only as the near half of "no click anywhere reaches the
    /// vault"; `a_click_over_the_card_never_reaches_the_vault_behind_it` is
    /// that claim on its own.
    #[test]
    fn a_click_on_the_scrim_never_reaches_the_vault_behind_it() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        // The stand-in control out in the margin, where the scrim alone covers.
        // Clicked at its centre rather than at some nearby point, so the click
        // is on the control and a failure cannot be a near miss.
        let corner = BEHIND_IN_MARGIN.center();
        assert!(
            !card.contains(corner),
            "the fixture point is under the card, so this would not be testing the scrim"
        );
        assert!(
            BEHIND_IN_MARGIN.contains(corner),
            "positive control: the click lands on the margin stand-in, not merely near it"
        );

        // Two warm-ups, as the card-click test takes: one for egui to create
        // the scrim's `Area`, one for it to have a laid-out rect to hit-test.
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, behind) = frame(&ctx, &mut state, &click(corner), true);
        assert!(
            !behind.in_margin,
            "a click in the margin reached the vault control behind the scrim. The user \
             believes they are editing preferences and is in fact driving the vault -- which \
             is worse than having no modal at all, because it looks safe"
        );

        // The other end of the pane, past the card on both axes. A scrim that
        // allocated the CARD's size rather than the screen's still covers the
        // near corner, because both are anchored at `Pos2::ZERO`.
        let far = BEHIND_IN_FAR_MARGIN.center();
        assert!(
            !card.contains(far) && BEHIND_IN_FAR_MARGIN.contains(far),
            "positive control: the far click is outside the card and on its stand-in"
        );
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind_far) = frame(&ctx, &mut state, &click(far), true);
        assert!(
            !behind_far.in_far_margin,
            "a click in the far margin reached the vault control behind the scrim, so the \
             scrim does not cover the whole pane -- only the part of it the card happens to \
             sit over"
        );

        assert_eq!(
            action,
            PrefsAction::None,
            "a scrim click dismissed the form -- neither `draw_folder_edit_modal` nor \
             `draw_launch_confirm_modal` does that, and this form commits as it is typed"
        );

        // The near half, in the same test and on its own frame: no click
        // anywhere over this pane reaches the vault.
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, _, behind) = frame(&ctx, &mut state, &click(BEHIND.center()), true);
        assert!(
            !behind.under_card,
            "a click over the card reached the vault control underneath it"
        );
    }

    // -----------------------------------------------------------------------
    // Dismissal
    // -----------------------------------------------------------------------

    #[test]
    fn escape_closes_the_modal() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(
            &ctx,
            &mut state,
            &[egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            true,
        );
        assert_eq!(action, PrefsAction::Close);
    }

    #[test]
    fn the_header_cross_closes_the_modal() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        // TWO warm-ups: one for egui to create the area, one for it to lay
        // the dismiss mark out where a click can find it.
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(&ctx, &mut state, &click(close_rect(card).center()), true);
        assert_eq!(
            action,
            PrefsAction::Close,
            "the dismiss mark did not close the modal, which leaves Esc as the only way out"
        );
    }

    /// An idle frame answers `None`. Trivially true today, and the thing that
    /// would break first if the dismiss mark's hit rect or the Esc check ever
    /// drifted onto something that fires every frame -- a modal that closes on
    /// its own is indistinguishable from a click that missed the gear.
    #[test]
    fn an_untouched_modal_stays_up() {
        let ctx = styled_context();
        let mut state = a_state();
        let _ = frame(&ctx, &mut state, &[], true);
        let (_, action, _) = frame(&ctx, &mut state, &[], true);
        assert_eq!(action, PrefsAction::None);
    }

    /// The form is live inside the modal -- a nav click changes section. The
    /// counterpart to the inertness tests above: the scrim must stop clicks
    /// reaching the vault and must NOT stop them reaching the card.
    #[test]
    fn the_form_inside_the_modal_is_live() {
        let ctx = styled_context();
        let mut state = a_state();
        let card = modal_card_rect(Rect::from_min_size(Pos2::ZERO, PANE));
        let body = modal_body_rect(card);
        // The second nav row, by the same arithmetic `draw_nav` lays out with.
        let second = Pos2::new(
            body.min.x + NAV_PAD_X + 40.0,
            body.min.y + NAV_PAD_Y + NAV_ITEM_HEIGHT + NAV_ITEM_GAP + NAV_ITEM_HEIGHT / 2.0,
        );
        let _ = frame(&ctx, &mut state, &[], true);
        let _ = frame(&ctx, &mut state, &[], true);
        assert_eq!(state.section, Section::General);
        let _ = frame(&ctx, &mut state, &click(second), true);
        assert_eq!(
            state.section,
            Section::ALL[1],
            "the nav row under the modal's own card did not take the click"
        );
    }

    // -----------------------------------------------------------------------
    // One form, two shells
    // -----------------------------------------------------------------------

    /// **The duplication guard.** Two places draw the preferences *shell*
    /// (`run`'s window and `draw_prefs_modal`'s card) and exactly one draws
    /// the form. A second copy of the body is how this project's recurring
    /// defects start: a control fixed in one and left broken in the other.
    #[test]
    fn exactly_one_place_in_this_program_draws_the_settings_form() {
        let source = include_str!("prefs_ui.rs");
        let body_calls = concat!("draw_prefs_", "body(");
        // The definition, `run`'s call, the modal's call, and `tests`' two
        // harnesses -- `frame`, and `paint_general_at`, which is the same
        // frame on a pane of a chosen width. Both are tests; the production
        // callers are still the two shells.
        assert_eq!(
            source.match_indices(body_calls).count(),
            5,
            "the number of `draw_prefs_body` sites changed; if a THIRD production caller \
             was added, confirm it is a shell and not a second form"
        );
        for forbidden in [concat!("fn draw_", "section("), concat!("fn draw_", "nav(")] {
            assert_eq!(
                source.match_indices(forbidden).count(),
                1,
                "{forbidden:?} is defined more than once"
            );
        }
    }
}

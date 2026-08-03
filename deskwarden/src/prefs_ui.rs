//! Design 3e's sectioned preferences window.
//!
//! 3e is a left nav of seven sections (General, Autofill, Native apps,
//! Security, Shortcuts, Sync & account, About) beside a content pane, with the
//! app version pinned to the bottom of the nav. This file builds that shell,
//! and populates it **only where a setting genuinely exists**.
//!
//! What exists today is three fields on [`Settings`]: `keep_backend_running`,
//! `auto_lock_enabled` and `auto_lock_minutes`. All three live on General --
//! the last two as a toggle and the number it governs, the number greyed out
//! while the toggle is off. Every other section in 3e -- its
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
struct PrefsState {
    settings: Settings,
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
    fn new(settings: Settings) -> Self {
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
    let entry = ui.put(
        field.shrink(4.0),
        egui::TextEdit::singleline(buffer)
            .id(egui::Id::new(STEPPER_FIELD_ID))
            .frame(egui::Frame::new())
            .font(FontId::new(12.0, FontFamily::Proportional))
            .horizontal_align(egui::Align::Center)
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
/// `keep_backend_running`, `auto_lock_enabled` and `auto_lock_minutes` -- the
/// three fields `Settings::persist_preferences` owns. `vault_window` is carried through
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
        rects: Vec<RectShape>,
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

        fn count_filled(&self, fill: egui::Color32) -> usize {
            self.rects.iter().filter(|r| r.fill == fill).count()
        }
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => p.texts.push((
                text.galley.text().to_string(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
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
    fn general_paints_exactly_two_toggles_and_one_stepper() {
        let painted = paint(Section::General);
        assert_eq!(
            painted.count_of_size(Vec2::new(40.0, 22.0)),
            2,
            "two 40x22 pills: `keep_backend_running` and `auto_lock_enabled`, and nothing else"
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
        // The first pill top-to-bottom is the backend row's; the auto-lock
        // one is below it. Clicking one must not move the other, which is
        // what the two `auto_lock_enabled` assertions here pin.
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[0].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.keep_backend_running);
        assert!(state.settings.auto_lock_enabled, "the wrong row's toggle moved");

        frame(&ctx, &mut state, &click(pill));
        assert!(state.settings.keep_backend_running, "and back again");
        assert!(state.settings.auto_lock_enabled);
    }

    #[test]
    fn clicking_the_auto_lock_toggle_turns_auto_lock_off_and_on_again() {
        // The user's actual request. `auto_lock_enabled` starts true, and
        // the second pill down is the one wired to it.
        let ctx = styled_context();
        let mut state = PrefsState::new(Settings::default());
        assert!(state.settings.auto_lock_enabled, "the default");

        let first = frame(&ctx, &mut state, &[]);
        let pill = first.rects_of_size(Vec2::new(40.0, 22.0))[1].center();
        frame(&ctx, &mut state, &click(pill));
        assert!(!state.settings.auto_lock_enabled, "the auto-lock toggle did not turn off");
        assert!(state.settings.keep_backend_running, "the wrong row's toggle moved");
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
        let hotkey_rs = include_str!("hotkey.rs");
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

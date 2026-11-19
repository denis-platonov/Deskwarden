//! Slice 7: Settings as a screen (spec Part C section 2).
//!
//! A left nav of twelve pages and a page column 760 wide, drawn in place of
//! the vault body. The pages are the v2 ones; what is ON them is what exists
//! today. Simple preferences are drawn here with the v2 row kinds (switch,
//! segmented, outline button, value); the panes that are whole flows -- the
//! breach scan, the backend choice, the update flow, the API keys, the
//! shortcut capture editor -- are the existing drawing code from the parent
//! module, placed inside a v2 page.
//!
//! Spec rows whose behaviour does not exist yet (theme, text size, open at
//! sign-in, notifications, devices, ...) are not drawn: a switch connected to
//! nothing reads as a broken feature. The three pages that would be empty
//! without them carry one marked-disabled row instead.

use eframe::egui::{self, Align, CornerRadius, Layout, Margin, Pos2, Rect, Sense, Ui, UiBuilder, Vec2};

use super as prefs;
use super::PrefsState;
use crate::generator_mode::GeneratorMode;
use crate::kit::card as kit_card;
use crate::kit::controls::{self, ButtonKind};
use crate::kit::icon;
use crate::kit::tokens::{self, palette, Role};
use crate::theme;

/// The nav's heading.
pub const NAV_HEADING: &str = "Settings";
/// The Back affordance at the top of the nav.
pub const BACK_LABEL: &str = "Back";

const NAV_WIDTH: f32 = 232.0;
const COLUMN_MAX: f32 = 760.0;
const PAGE_GAP: f32 = 18.0;
const GROUP_GAP: f32 = 10.0;
const ROW_GAP: f32 = 14.0;
const ROW_MIN_HEIGHT: f32 = 42.0;

const HEADING: Role = Role { size: 26.0, face: theme::EXTRABOLD, tracking_em: -0.02 };
const PAGE_TITLE: Role = Role { size: 30.0, face: theme::EXTRABOLD, tracking_em: -0.03 };
const ROW_TITLE: Role = Role { size: 16.0, face: theme::SEMIBOLD, tracking_em: 0.0 };
const VALUE: Role = Role { size: 15.0, face: theme::SEMIBOLD, tracking_em: 0.0 };

/// The twelve pages, in nav order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    General,
    Autofill,
    Apps,
    Security,
    Vault,
    Generator,
    Notifications,
    ImportExport,
    Devices,
    Shortcuts,
    Account,
    About,
}

impl Page {
    pub const ALL: [Page; 12] = [
        Page::General,
        Page::Autofill,
        Page::Apps,
        Page::Security,
        Page::Vault,
        Page::Generator,
        Page::Notifications,
        Page::ImportExport,
        Page::Devices,
        Page::Shortcuts,
        Page::Account,
        Page::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Autofill => "Autofill",
            Page::Apps => "Apps",
            Page::Security => "Security",
            Page::Vault => "Vault",
            Page::Generator => "Generator",
            Page::Notifications => "Notifications",
            Page::ImportExport => "Import & export",
            Page::Devices => "Devices",
            Page::Shortcuts => "Shortcuts",
            Page::Account => "Account",
            Page::About => "About",
        }
    }

    /// The spec's subtitle, for the pages that have one.
    pub fn subtitle(self) -> Option<String> {
        Some(match self {
            Page::General => "How Deskwarden starts and looks".into(),
            Page::Autofill => "What happens when an app asks you to sign in".into(),
            Page::Apps => "Programs on this PC that ask Deskwarden for your vault".into(),
            Page::Security => "Keep your vault safe".into(),
            Page::Vault => "How items are shown and organised".into(),
            Page::Generator => "Defaults for new passwords".into(),
            Page::Notifications => "When Deskwarden should tell you something".into(),
            Page::ImportExport => "Move passwords in and out".into(),
            Page::Devices => "Where your account is signed in".into(),
            Page::Shortcuts => return None,
            Page::Account => return None,
            Page::About => format!("Deskwarden {}", env!("CARGO_PKG_VERSION")),
        })
    }

    /// The spec's nav icon (24x24 viewBox, stroke 2).
    fn icon(self) -> &'static str {
        match self {
            Page::General => icon::glyph::SETTINGS,
            Page::Autofill => "M15 4V2M15 16v-2M8 9h2M20 9h2M17.8 11.8 19 13M17.8 6.2 19 5M3 21l9-9M12.2 6.2 11 5",
            Page::Apps => icon::glyph::APPS,
            Page::Security => "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z",
            Page::Vault => icon::glyph::ARCHIVE,
            Page::Generator => icon::glyph::GEN,
            Page::Notifications => "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0",
            Page::ImportExport => "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3",
            Page::Devices => "M4 5h16v11H4zM2 20h20",
            Page::Shortcuts => "M2 6h20v12H2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10",
            Page::Account => icon::glyph::PERSON,
            Page::About => "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 16v-4M12 8h.01",
        }
    }
}

/// What the screen asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenAction {
    None,
    /// The Back row: leave Settings.
    Back,
}

/// Draws the whole screen into `ui`'s rect: the nav, then the page.
pub fn draw(ui: &mut Ui, state: &mut PrefsState, page: &mut Page) -> ScreenAction {
    let full = ui.max_rect();
    // The screen's own ground, from the palette, so the theme reaches it.
    ui.painter().rect_filled(full, CornerRadius::ZERO, palette(ui.ctx()).window_bg);
    let nav = Rect::from_min_max(full.min, Pos2::new(full.min.x + NAV_WIDTH, full.max.y));
    let content = Rect::from_min_max(Pos2::new(nav.max.x, full.min.y), full.max);
    let back = draw_nav(ui, nav, state, page);
    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| draw_page(ui, state, page));
    if back {
        // Leaving the screen ends a recording: nothing may keep listening.
        state.capturing = None;
        ScreenAction::Back
    } else {
        ScreenAction::None
    }
}

fn draw_nav(ui: &mut Ui, nav: Rect, state: &mut PrefsState, page: &mut Page) -> bool {
    let p = palette(ui.ctx());
    ui.painter().rect_filled(nav, CornerRadius::ZERO, p.list_bg);
    ui.painter().rect_filled(
        Rect::from_min_max(Pos2::new(nav.max.x - 1.0, nav.min.y), nav.max),
        CornerRadius::ZERO,
        p.hairline,
    );
    let inner = Rect::from_min_max(Pos2::new(nav.min.x + 12.0, nav.min.y + 20.0), Pos2::new(nav.max.x - 12.0, nav.max.y));
    let mut back = false;
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
        back = nav_row(ui, icon::glyph::BACK, BACK_LABEL, false, 36.0);
        ui.add_space(8.0);
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 14 }).show(ui, |ui| {
            ui.label(tokens::job(HEADING, NAV_HEADING, p.ink));
        });
        egui::ScrollArea::vertical().id_salt("settings-nav").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            for each in Page::ALL {
                if nav_row(ui, each.icon(), each.label(), *page == each, 42.0) && *page != each {
                    // Spec: a click on a nav row ends any recording.
                    state.capturing = None;
                    *page = each;
                }
            }
        });
    });
    back
}

/// One nav row: an 18 px stroke icon and a 15 px label; the selected row is
/// washed blue.
fn nav_row(ui: &mut Ui, glyph: &str, label: &str, selected: bool, height: f32) -> bool {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(tokens::R_NAV), p.blue_wash);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(tokens::R_NAV), p.hover);
    }
    let ink = if selected { p.blue_deep } else { p.text_nav };
    let glyph_rect = Rect::from_center_size(Pos2::new(rect.min.x + 12.0 + 9.0, rect.center().y), Vec2::splat(18.0));
    icon::stroke(ui.painter(), glyph_rect, glyph, 2.0, ink);
    let galley = ui.fonts_mut(|f| f.layout_job(tokens::job(tokens::NAV, label, ink)));
    ui.painter().galley(Pos2::new(rect.min.x + 12.0 + 18.0 + 12.0, rect.center().y - galley.size().y / 2.0), galley, ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn draw_page(ui: &mut Ui, state: &mut PrefsState, page: &mut Page) {
    let p = palette(ui.ctx());
    egui::ScrollArea::vertical()
        .id_salt(("settings-page", *page as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let available = ui.available_width();
            let column = (available - 80.0).min(COLUMN_MAX).max(320.0);
            let side = ((available - column) / 2.0).max(24.0);
            ui.add_space(32.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(column);
                    ui.spacing_mut().item_spacing.y = PAGE_GAP;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(tokens::job(PAGE_TITLE, page.label(), p.ink));
                        if let Some(sub) = page.subtitle() {
                            ui.label(tokens::job(tokens::Role { size: 16.0, face: theme::REGULAR, tracking_em: 0.0 }, &sub, p.text_faint));
                        }
                    });
                    body(ui, state, page);
                    ui.add_space(48.0);
                });
            });
        });
}

fn body(ui: &mut Ui, state: &mut PrefsState, page: &mut Page) {
    match *page {
        Page::General => general(ui, state),
        Page::Autofill => autofill(ui, state, page),
        Page::Apps => apps(ui, state),
        Page::Security => security(ui, state),
        Page::Vault => vault(ui, state),
        Page::Generator => generator(ui, state),
        Page::Notifications => not_yet(
            ui,
            "Notification choices are not available yet",
            "Choosing which events tell you something arrives in a later release.",
        ),
        Page::ImportExport => import_export(ui),
        Page::Devices => not_yet(
            ui,
            "The device list is not available yet",
            "Seeing and signing out your other devices arrives in a later release.",
        ),
        Page::Shortcuts => shortcuts(ui, state),
        Page::Account => account(ui, state),
        Page::About => about(ui, state),
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn general(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some("Startup"), |ui| {
        card(ui, |ui| {
            startup_rows(ui, state);
            if state.startup.is_some() {
                kit_card::row_rule(ui);
            }
            value_row(ui, KEEP_RUNNING_LABEL, KEEP_RUNNING_SUB, KEEP_RUNNING_VALUE);
        });
    });
    group(ui, Some("Look"), |ui| {
        card(ui, |ui| {
            use crate::kit::tokens::{SidebarStyle, TextSize, ThemeChoice};
            let now = if state.settings.sidebar_style == SidebarStyle::Navy { 0 } else { 1 };
            if let Some(picked) = segment_row(ui, prefs::SIDEBAR_STYLE_LABEL, prefs::SIDEBAR_STYLE_DESCRIPTION, &["Navy", "Light"], now) {
                state.settings.sidebar_style = if picked == 0 { SidebarStyle::Navy } else { SidebarStyle::Light };
            }
            kit_card::row_rule(ui);
            let themes = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];
            let now = themes.iter().position(|t| *t == state.settings.theme).unwrap_or(0);
            if let Some(picked) = segment_row(ui, THEME_LABEL, "", &["Light", "Dark", "System"], now) {
                state.settings.theme = themes[picked];
            }
            kit_card::row_rule(ui);
            let sizes = [TextSize::Default, TextSize::Large, TextSize::Larger];
            let now = sizes.iter().position(|t| *t == state.settings.text_size).unwrap_or(0);
            if let Some(picked) = segment_row(ui, TEXT_SIZE_LABEL, "", &["Default", "Large", "Larger"], now) {
                state.settings.text_size = sizes[picked];
            }
            kit_card::row_rule(ui);
            if button_row(ui, LANGUAGE_LABEL, "", LANGUAGE_VALUE, false) {
                crate::kit::toast::show(ui.ctx(), LANGUAGE_TOAST);
            }
        });
    });
}

/// Startup > the two logon-entry switches. Drawn only when the state was
/// handed the registry; a change is written at once (the entry is not part of
/// `settings.json`), and the switch follows what the registry then says.
fn startup_rows(ui: &mut Ui, state: &mut PrefsState) {
    let Some(now) = state.startup else {
        return;
    };
    let mut open = now.open_at_sign_in;
    switch_row(ui, OPEN_AT_SIGN_IN_LABEL, "", &mut open, true);
    kit_card::row_rule(ui);
    let mut tray = if now.open_at_sign_in { now.in_tray } else { state.tray_when_opened };
    switch_row(ui, START_IN_TRAY_LABEL, START_IN_TRAY_SUB, &mut tray, now.open_at_sign_in);
    let wanted = crate::autostart_repair::Startup { open_at_sign_in: open, in_tray: tray };
    if open == now.open_at_sign_in && (tray == now.in_tray || !now.open_at_sign_in) {
        return;
    }
    if now.open_at_sign_in && !open {
        state.tray_when_opened = now.in_tray;
    }
    if (state.startup_sink)(wanted) {
        state.startup = Some(wanted);
    } else {
        crate::kit::toast::show(ui.ctx(), STARTUP_FAILED_TOAST);
    }
}

/// The Startup rows' wording (spec C 2.4).
pub const OPEN_AT_SIGN_IN_LABEL: &str = "Open when I sign in to Windows";
pub const START_IN_TRAY_LABEL: &str = "Start in the system tray";
const START_IN_TRAY_SUB: &str = "Start hidden; open the window from the tray icon";
/// Closing the window only ever hides it: the tray keeps autofill running.
pub const KEEP_RUNNING_LABEL: &str = "Keep running when I close the window";
const KEEP_RUNNING_SUB: &str = "Autofill only works while Deskwarden is running. Closing the window just hides it.";
const KEEP_RUNNING_VALUE: &str = "Always";
const STARTUP_FAILED_TOAST: &str = "Could not change the Windows startup entry";
/// The Security rows' wording (spec C 2.4).
pub const LOCK_ON_SLEEP_LABEL: &str = "Lock when the computer sleeps";
pub const LOCK_ON_SCREEN_LOCK_LABEL: &str = "Lock when I lock my screen";
pub const HIDE_ON_SHARE_LABEL: &str = "Hide passwords while I share my screen";
const HIDE_ON_SHARE_SUB: &str = "The whole window stays out of Teams, Zoom or Meet screen sharing, even if you press reveal.";
/// The Look rows' wording (spec C 2.4).
pub const THEME_LABEL: &str = "Theme";
pub const TEXT_SIZE_LABEL: &str = "Text size";
pub const LANGUAGE_LABEL: &str = "Language";
const LANGUAGE_VALUE: &str = "English";
/// Spec D6: English only, and a toast for the click.
pub const LANGUAGE_TOAST: &str = "More languages are on the way";

fn autofill(ui: &mut Ui, state: &mut PrefsState, page: &mut Page) {
    group(ui, Some("Suggestions"), |ui| {
        card(ui, |ui| {
            switch_row(ui, prefs::PROMPT_LABEL, prefs::PROMPT_DESCRIPTION, &mut state.settings.prompt_on_match, true);
        });
    });
    group(ui, Some("Shortcut"), |ui| {
        card(ui, |ui| {
            let chord = match state.settings.shortcuts.picker {
                Some(chord) => chord.to_string(),
                None => prefs::SHORTCUT_NOT_SET.to_string(),
            };
            if button_row(ui, FILL_SHORTCUT_LABEL, FILL_SHORTCUT_SUB, &chord, false) {
                *page = Page::Shortcuts;
            }
        });
    });
}

/// The Autofill page's pointer at the Shortcuts page.
pub const FILL_SHORTCUT_LABEL: &str = "Fill shortcut";
const FILL_SHORTCUT_SUB: &str = "Works in any app, even without the panel";

fn apps(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some("Local API"), |ui| prefs::api_cards(ui, state));
}

fn security(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some("Locking"), |ui| {
        prefs::draw_lock(ui, state);
        card(ui, |ui| {
            let s = &mut state.settings;
            switch_row(ui, LOCK_ON_SLEEP_LABEL, "", &mut s.lock_on_sleep, true);
            kit_card::row_rule(ui);
            switch_row(ui, LOCK_ON_SCREEN_LOCK_LABEL, "", &mut s.lock_on_screen_lock, true);
        });
    });
    group(ui, Some("Privacy"), |ui| {
        prefs::draw_clipboard(ui, state);
        card(ui, |ui| {
            switch_row(ui, HIDE_ON_SHARE_LABEL, HIDE_ON_SHARE_SUB, &mut state.settings.hide_on_screen_share, true);
        });
    });
    group(ui, Some("Breaches"), |ui| prefs::draw_breaches(ui, state));
}

fn vault(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some("Display"), |ui| {
        card(ui, |ui| {
            let s = &mut state.settings;
            switch_row(ui, prefs::FETCH_ICONS_LABEL, prefs::FETCH_ICONS_DESCRIPTION, &mut s.fetch_icons, true);
            kit_card::row_rule(ui);
            let icons_on = s.fetch_icons;
            switch_row(ui, prefs::DIRECT_ICONS_LABEL, prefs::DIRECT_ICONS_DESCRIPTION, &mut s.fetch_icons_direct, icons_on);
            kit_card::row_rule(ui);
            let direct_on = icons_on && s.fetch_icons_direct;
            switch_row(ui, prefs::LAN_TLS_LABEL, prefs::LAN_TLS_DESCRIPTION, &mut s.icons_ignore_tls_on_private_hosts, direct_on);
            kit_card::row_rule(ui);
            switch_row(ui, prefs::BRAND_LOGOS_LABEL, prefs::BRAND_LOGOS_DESCRIPTION, &mut s.use_brand_logos, true);
            kit_card::row_rule(ui);
            switch_row(ui, prefs::TOTP_SECRET_LABEL, prefs::TOTP_SECRET_DESCRIPTION, &mut s.reveal_totp_seed, true);
        });
    });
    group(ui, Some("Opening the vault"), |ui| {
        prefs::draw_backend_card(ui, state);
        prefs::draw_disk_cache_card(ui, state);
    });
}

fn generator(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, None, |ui| {
        card(ui, |ui| {
            let g = &mut state.settings.generator;
            let modes = [GeneratorMode::Password, GeneratorMode::Passphrase, GeneratorMode::Pin];
            let now = modes.iter().position(|m| *m == g.mode).unwrap_or(0);
            if let Some(picked) = segment_row(ui, "Make new passwords as", "", &["Password", "Passphrase", "PIN"], now) {
                g.mode = modes[picked];
            }
            kit_card::row_rule(ui);
            let length = match g.mode {
                GeneratorMode::Password => format!("{} characters", g.length),
                GeneratorMode::Passphrase => format!("{} words", g.words),
                GeneratorMode::Pin => format!("{} digits", g.pin_length),
            };
            value_row(ui, "Length", "", &length);
            kit_card::row_rule(ui);
            switch_row(ui, "Include symbols", "", &mut g.symbols, true);
            kit_card::row_rule(ui);
            switch_row(ui, "Avoid look-alike characters", "", &mut g.avoid_lookalikes, true);
        });
    });
}

fn shortcuts(ui: &mut Ui, state: &mut PrefsState) {
    let live = crate::app::FillShortcut::ALL.map(crate::hotkey::availability);
    group(ui, None, |ui| prefs::draw_shortcuts(ui, state, live));
}

fn account(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some("Account"), |ui| prefs::card(ui, |ui| prefs::account_row(ui, (state.account_source)())));
    group(ui, Some("Sync"), |ui| prefs::draw_sync_poll_card(ui, state));
    group(ui, Some("Sharing"), |ui| prefs::draw_sharing_card(ui, state));
}

fn about(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, None, |ui| {
        card(ui, |ui| {
            value_row(ui, "Version", "Unofficial, and unaffiliated with Bitwarden, Inc.", &prefs::version_line());
        });
    });
    group(ui, Some("Updates"), |ui| prefs::draw_updates(ui, state));
}

/// One marked-disabled row, for a page whose behaviour is not built yet.
/// Slice 9's import wizard, opened through the window's shell request.
fn import_export(ui: &mut Ui) {
    group(ui, None, |ui| {
        card(ui, |ui| {
            if button_row(
                ui,
                "Import passwords",
                "From Bitwarden, 1Password, Keeper, LastPass, Chrome or Edge, or any CSV file",
                "Import\u{2026}",
                false,
            ) {
                crate::vault_window::sidebar::request_shell(ui.ctx(), crate::vault_window::sidebar::ShellRequest::OpenImport);
            }
        });
    });
}

fn not_yet(ui: &mut Ui, title: &str, sub: &str) {
    group(ui, None, |ui| {
        card(ui, |ui| row(ui, title, sub, true, 0.0, |_| {}));
    });
}

// ---------------------------------------------------------------------------
// Groups, cards and the four row kinds
// ---------------------------------------------------------------------------

/// A group: an optional tracked-capitals title over a card, 10 apart.
fn group(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let p = palette(ui.ctx());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = GROUP_GAP;
        if let Some(title) = title {
            ui.add_space(6.0);
            kit_card::section_label(ui, &title.to_uppercase(), p.text_label);
        }
        add(ui);
    });
}

/// The v2 card the rows live in: rows carry their own padding.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    kit_card::card(ui, tokens::R_CARD_SETTINGS, Margin::ZERO, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        add(ui);
    });
}

/// The row frame: title and optional sub on the left, a control `control_w`
/// wide on the right, vertically centred on the row.
///
/// The text column is measured first and the control is handed a rect of
/// exactly that height (see `prefs::control_row_of` for why a right-to-left
/// layout over an unbounded height cannot be used here).
fn row(ui: &mut Ui, title: &str, sub: &str, ghost: bool, control_w: f32, control: impl FnOnce(&mut Ui)) {
    let p = palette(ui.ctx());
    egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 17, bottom: 17 }).show(ui, |ui| {
        let width = ui.available_width();
        let text_w = (width - control_w - ROW_GAP).max(80.0);
        let origin = ui.cursor().min;
        let (ink, sub_ink) = if ghost { (p.text_ghost, p.text_ghost) } else { (p.ink, p.text_label) };
        let text = ui.allocate_ui_with_layout(Vec2::new(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(tokens::job(ROW_TITLE, title, ink));
            if !sub.is_empty() {
                ui.label(tokens::job(tokens::SUBTITLE, sub, sub_ink));
            }
        });
        let height = text.response.rect.height().max(ROW_MIN_HEIGHT);
        let rect = Rect::from_min_size(Pos2::new(origin.x + width - control_w, origin.y), Vec2::new(control_w, height));
        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)), control);
    });
}

/// `sw`: a switch bound to a setting. With `enabled` false it is painted
/// inert and `value` is left alone.
fn switch_row(ui: &mut Ui, title: &str, sub: &str, value: &mut bool, enabled: bool) {
    row(ui, title, sub, !enabled, 50.0, |ui| {
        if enabled {
            controls::switch(ui, value);
        } else {
            ui.add_enabled_ui(false, |ui| {
                let mut copy = *value;
                controls::switch(ui, &mut copy);
            });
        }
    });
}

/// `sg`: a segmented control; the index the user picked this frame.
fn segment_row(ui: &mut Ui, title: &str, sub: &str, options: &[&str], selected: usize) -> Option<usize> {
    let width = controls::segmented_width(ui.ctx(), options, controls::SEG_SETTINGS);
    let mut picked = None;
    row(ui, title, sub, false, width, |ui| picked = controls::segmented(ui, options, selected, controls::SEG_SETTINGS));
    picked
}

/// `bt`: an outline button; whether it was clicked.
fn button_row(ui: &mut Ui, title: &str, sub: &str, label: &str, _danger: bool) -> bool {
    let width = controls::button_width(ui.ctx(), ButtonKind::Outline, controls::ROW, label, false);
    let mut clicked = false;
    row(ui, title, sub, false, width, |ui| {
        clicked = controls::button(ui, ButtonKind::Outline, controls::ROW, label, None).clicked();
    });
    clicked
}

/// `va`: a read-only value.
fn value_row(ui: &mut Ui, title: &str, sub: &str, value: &str) {
    let p = palette(ui.ctx());
    let width = ui.fonts_mut(|f| f.layout_job(tokens::job(VALUE, value, p.text_nav))).size().x.ceil() + 2.0;
    row(ui, title, sub, false, width, |ui| {
        ui.label(tokens::job(VALUE, value, p.text_nav));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};
    use crate::settings::Settings;

    // Tall, because a label below the fold is not painted at all.
    const SCREEN: Vec2 = egui::vec2(1028.0, 2600.0);

    struct Fixture {
        state: PrefsState,
        page: Page,
        ctx: egui::Context,
        time: f64,
    }

    impl Fixture {
        fn new(page: Page) -> Self {
            Self { state: PrefsState::new(Settings::default()), page, ctx: testing::ctx(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> (ScreenAction, testing::Painted) {
            self.time += 0.1;
            let mut action = ScreenAction::None;
            let (state, page) = (&mut self.state, &mut self.page);
            let painted = testing::frame(&self.ctx, self.time, SCREEN, events, |ui| action = draw(ui, state, page));
            (action, painted)
        }

        /// Three idle frames so rects from the pass before last are settled.
        fn settle(&mut self) -> testing::Painted {
            self.frame(vec![]);
            self.frame(vec![]);
            self.frame(vec![]).1
        }

        fn click_text(&mut self, text: &str) -> ScreenAction {
            let painted = self.settle();
            let at = painted.texts.iter().find(|(t, _, _)| t == text).unwrap_or_else(|| panic!("no {text:?} painted")).2.center();
            let action = self.frame(click_at(at)).0;
            self.frame(vec![egui::Event::PointerGone]);
            action
        }
    }

    #[test]
    fn the_nav_lists_the_twelve_pages_in_the_specs_order() {
        let labels: Vec<_> = Page::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(
            labels,
            [
                "General", "Autofill", "Apps", "Security", "Vault", "Generator", "Notifications", "Import & export", "Devices",
                "Shortcuts", "Account", "About"
            ]
        );
        let mut fixture = Fixture::new(Page::General);
        let painted = fixture.settle();
        let mut last = 0.0;
        for label in labels {
            let at = painted.texts.iter().filter(|(t, _, _)| t == label).map(|(_, _, r)| r.min.y).fold(f32::NAN, f32::min);
            assert!(at.is_finite(), "the nav does not show {label}");
            assert!(at >= last, "{label} is out of order in the nav");
            last = at;
        }
        assert!(painted.text(NAV_HEADING).is_some());
    }

    #[test]
    fn a_nav_click_opens_that_page() {
        let mut fixture = Fixture::new(Page::General);
        // The nav's "Vault" is the only text of that exact wording at first.
        fixture.click_text("Vault");
        assert_eq!(fixture.page, Page::Vault);
        let painted = fixture.settle();
        assert!(painted.text("How items are shown and organised").is_some(), "the Vault page's subtitle");
    }

    #[test]
    fn back_asks_the_window_to_leave() {
        let mut fixture = Fixture::new(Page::General);
        assert_eq!(fixture.click_text(BACK_LABEL), ScreenAction::Back);
    }

    #[test]
    fn changing_page_ends_a_shortcut_recording() {
        let mut fixture = Fixture::new(Page::Shortcuts);
        fixture.state.capturing = Some(crate::app::FillShortcut::ALL[0]);
        assert!(fixture.state.is_capturing());
        fixture.click_text("Vault");
        assert!(!fixture.state.is_capturing(), "a nav click leaves a row listening for keys");
    }

    #[test]
    fn every_page_draws_its_title_and_does_not_panic() {
        for page in Page::ALL {
            let mut fixture = Fixture::new(page);
            let painted = fixture.settle();
            assert!(
                painted.texts.iter().filter(|(t, _, _)| t == page.label()).count() >= 2,
                "{page:?}: the label is in the nav and again as the page title"
            );
        }
    }

    /// The point of the slice: nothing 0.15 let a user change went missing.
    #[test]
    fn every_existing_preference_is_reachable_on_some_page() {
        let wanted: &[(&str, &str)] = &[
            (prefs::SIDEBAR_STYLE_LABEL, "sidebar style"),
            (prefs::PROMPT_LABEL, "autofill prompts"),
            (prefs::SERVICE_LABEL, "local API"),
            (prefs::AUTO_LOCK_ENABLED_LABEL, "auto-lock"),
            (prefs::AUTO_LOCK_LABEL, "auto-lock minutes"),
            (prefs::CLIPBOARD_MASTER_LABEL, "clipboard clearing"),
            (prefs::CLIPBOARD_ON_LOCK_LABEL, "clear on lock"),
            (prefs::BREACH_LABEL, "breach check"),
            (prefs::SCAN_BUTTON, "breach scan"),
            (prefs::FETCH_ICONS_LABEL, "site icons"),
            (prefs::DIRECT_ICONS_LABEL, "direct icons"),
            (prefs::LAN_TLS_LABEL, "lan tls"),
            (prefs::BRAND_LOGOS_LABEL, "card logos"),
            (prefs::TOTP_SECRET_LABEL, "totp secret"),
            (prefs::UI_LOADED_LABEL, "open instantly"),
            (prefs::BACKEND_CHOICE_LABEL, "backend choice"),
            (prefs::DISK_CACHE_LABEL, "disk cache"),
            (prefs::SYNC_PUSH_LABEL, "live updates"),
            (prefs::SYNC_POLL_ENABLED_LABEL, "polling"),
            (prefs::ADVANCED_SHARING_LABEL, "advanced sharing"),
            (prefs::UPDATE_CHECK_LABEL, "automatic update check"),
            (prefs::UPDATE_CHECK_BUTTON, "update check"),
            (prefs::ACCOUNT_LABEL, "account row"),
        ];
        let mut seen = std::collections::HashSet::new();
        for page in Page::ALL {
            let mut fixture = Fixture::new(page);
            // Backend rows hide when no account is known; give one.
            fixture.state.settings.use_official_bw_crypto = Some(false);
            let painted = fixture.settle();
            for (label, what) in wanted {
                if painted.text(label).is_some() {
                    seen.insert(*what);
                }
            }
        }
        for (_, what) in wanted {
            assert!(seen.contains(what), "{what} is on no Settings page any more");
        }
    }

    #[test]
    fn the_autofill_switch_edits_the_setting() {
        let mut fixture = Fixture::new(Page::Autofill);
        assert!(fixture.state.settings.prompt_on_match);
        let painted = fixture.settle();
        let title = painted.text(prefs::PROMPT_LABEL).expect("the row").2;
        let switch = Pos2::new(title.min.x + 740.0, title.center().y);
        // The switch is at the card's right edge; find it by its column.
        let column = painted.rects.iter().filter(|r| (r.rect.width() - 50.0).abs() < 0.5).map(|r| r.rect.center()).next().expect("a 50 px switch track");
        let _ = switch;
        fixture.frame(click_at(column));
        assert!(!fixture.state.settings.prompt_on_match, "the click did not turn the setting off");
    }

    #[test]
    fn the_fill_shortcut_row_goes_to_the_shortcuts_page() {
        let mut fixture = Fixture::new(Page::Autofill);
        let painted = fixture.settle();
        let chord = fixture.state.settings.shortcuts.picker.map(|c| c.to_string()).unwrap_or_else(|| prefs::SHORTCUT_NOT_SET.into());
        let at = painted.texts.iter().filter(|(t, _, _)| *t == chord).map(|(_, _, r)| r.center()).next().expect("the chord button");
        fixture.frame(click_at(at));
        assert_eq!(fixture.page, Page::Shortcuts);
    }

    #[test]
    fn the_generator_page_edits_the_defaults() {
        let mut fixture = Fixture::new(Page::Generator);
        let painted = fixture.settle();
        assert!(painted.text("20 characters").is_some(), "the length read-out");
        let at = painted.texts.iter().find(|(t, _, _)| t == "PIN").expect("the PIN segment").2.center();
        fixture.frame(click_at(at));
        assert_eq!(fixture.state.settings.generator.mode, GeneratorMode::Pin);
        let painted = fixture.settle();
        assert!(painted.text("6 digits").is_some());
    }

    #[test]
    fn the_empty_pages_say_so_in_one_disabled_row() {
        for page in [Page::Notifications, Page::Devices] {
            let mut fixture = Fixture::new(page);
            let painted = fixture.settle();
            assert!(painted.text("later release").is_some(), "{page:?} says nothing about why it is empty");
        }
    }

    #[test]
    fn a_disabled_child_switch_leaves_its_value_alone() {
        let mut fixture = Fixture::new(Page::Vault);
        fixture.state.settings.fetch_icons = false;
        fixture.state.settings.fetch_icons_direct = true;
        fixture.settle();
        let painted = fixture.settle();
        let at = painted.text(prefs::DIRECT_ICONS_LABEL).expect("the child row").2;
        // Click where its switch is: the right edge of the same row.
        let rect = painted.rects.iter().filter(|r| (r.rect.width() - 50.0).abs() < 0.5 && (r.rect.center().y - at.center().y).abs() < 90.0).map(|r| r.rect.center()).next().expect("its switch");
        fixture.frame(click_at(rect));
        assert!(fixture.state.settings.fetch_icons_direct, "a greyed child was edited under an off master");
    }
    // ---- slice 8: General ----

    thread_local! {
        static WRITTEN: std::cell::RefCell<Vec<crate::autostart_repair::Startup>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn recording_sink(startup: crate::autostart_repair::Startup) -> bool {
        WRITTEN.with(|w| w.borrow_mut().push(startup));
        true
    }

    fn refusing_sink(_: crate::autostart_repair::Startup) -> bool {
        false
    }

    fn with_registry(open_at_sign_in: bool, in_tray: bool, sink: fn(crate::autostart_repair::Startup) -> bool) -> Fixture {
        let mut fixture = Fixture::new(Page::General);
        fixture.state.startup = Some(crate::autostart_repair::Startup { open_at_sign_in, in_tray });
        fixture.state.startup_sink = sink;
        WRITTEN.with(|w| w.borrow_mut().clear());
        fixture
    }

    /// The centres of the 50 px switch tracks, top to bottom.
    fn switches(painted: &testing::Painted) -> Vec<Pos2> {
        let mut found: Vec<Pos2> = painted.rects.iter().filter(|r| (r.rect.width() - 50.0).abs() < 0.5).map(|r| r.rect.center()).collect();
        found.sort_by(|a, b| a.y.total_cmp(&b.y));
        found.dedup_by(|a, b| (a.y - b.y).abs() < 1.0);
        found
    }

    #[test]
    fn the_startup_rows_are_not_drawn_without_the_registry() {
        let mut fixture = Fixture::new(Page::General);
        let painted = fixture.settle();
        assert!(painted.text(OPEN_AT_SIGN_IN_LABEL).is_none(), "a switch connected to nothing");
        assert!(painted.text(START_IN_TRAY_LABEL).is_none());
        // The always-true row is honest without it.
        assert!(painted.text(KEEP_RUNNING_LABEL).is_some());
        assert!(painted.text("Always").is_some());
    }

    #[test]
    fn turning_open_at_sign_in_on_writes_the_logon_entry() {
        let mut fixture = with_registry(false, false, recording_sink);
        let painted = fixture.settle();
        assert!(painted.text(OPEN_AT_SIGN_IN_LABEL).is_some());
        let first = switches(&painted)[0];
        fixture.frame(click_at(first));
        let written = WRITTEN.with(|w| w.borrow().clone());
        assert_eq!(written, [crate::autostart_repair::Startup { open_at_sign_in: true, in_tray: true }], "on, and to the tray as the installer does");
        assert_eq!(fixture.state.startup, Some(written[0]));
    }

    #[test]
    fn turning_it_off_remembers_the_tray_choice_for_next_time() {
        let mut fixture = with_registry(true, false, recording_sink);
        let painted = fixture.settle();
        fixture.frame(click_at(switches(&painted)[0]));
        assert_eq!(WRITTEN.with(|w| w.borrow().clone()), [crate::autostart_repair::Startup { open_at_sign_in: false, in_tray: false }]);
        assert!(!fixture.state.tray_when_opened, "the tray choice was lost with the entry");
    }

    #[test]
    fn the_tray_switch_is_inert_while_open_at_sign_in_is_off() {
        let mut fixture = with_registry(false, false, recording_sink);
        let painted = fixture.settle();
        fixture.frame(click_at(switches(&painted)[1]));
        assert!(WRITTEN.with(|w| w.borrow().is_empty()), "a greyed switch wrote the registry");
    }

    #[test]
    fn a_refused_write_leaves_the_switch_where_the_registry_is() {
        let mut fixture = with_registry(false, false, refusing_sink);
        let painted = fixture.settle();
        fixture.frame(click_at(switches(&painted)[0]));
        assert_eq!(fixture.state.startup, Some(crate::autostart_repair::Startup { open_at_sign_in: false, in_tray: false }));
        assert_eq!(crate::kit::toast::current(&fixture.ctx).as_deref(), Some(STARTUP_FAILED_TOAST));
    }

    #[test]
    fn the_tray_switch_edits_the_entry_when_open_at_sign_in_is_on() {
        let mut fixture = with_registry(true, true, recording_sink);
        let painted = fixture.settle();
        fixture.frame(click_at(switches(&painted)[1]));
        assert_eq!(WRITTEN.with(|w| w.borrow().clone()), [crate::autostart_repair::Startup { open_at_sign_in: true, in_tray: false }]);
    }

    #[test]
    fn theme_and_text_size_are_segments_over_the_settings() {
        use crate::kit::tokens::{TextSize, ThemeChoice};
        let mut fixture = Fixture::new(Page::General);
        assert_eq!(fixture.state.settings.theme, ThemeChoice::Light);
        let painted = fixture.settle();
        assert!(painted.text(THEME_LABEL).is_some() && painted.text(TEXT_SIZE_LABEL).is_some());
        for (text, check) in [("Dark", 0), ("System", 1), ("Larger", 2)] {
            let painted = fixture.settle();
            let at = painted.texts.iter().find(|(t, _, _)| t == text).unwrap_or_else(|| panic!("no {text}")).2.center();
            fixture.frame(click_at(at));
            fixture.frame(vec![egui::Event::PointerGone]);
            match check {
                0 => assert_eq!(fixture.state.settings.theme, ThemeChoice::Dark),
                1 => assert_eq!(fixture.state.settings.theme, ThemeChoice::System),
                _ => assert_eq!(fixture.state.settings.text_size, TextSize::Larger),
            }
        }
    }

    #[test]
    fn the_security_page_has_the_lock_and_share_switches() {
        let mut fixture = Fixture::new(Page::Security);
        let painted = fixture.settle();
        for label in [LOCK_ON_SLEEP_LABEL, LOCK_ON_SCREEN_LOCK_LABEL, HIDE_ON_SHARE_LABEL] {
            assert!(painted.text(label).is_some(), "no {label}");
        }
        let at = painted.text(LOCK_ON_SLEEP_LABEL).unwrap().2;
        let switch = painted
            .rects
            .iter()
            .filter(|r| (r.rect.width() - 50.0).abs() < 0.5 && (r.rect.center().y - at.center().y).abs() < 20.0)
            .map(|r| r.rect.center())
            .next()
            .expect("its switch");
        fixture.frame(click_at(switch));
        assert!(!fixture.state.settings.lock_on_sleep);
    }

    #[test]
    fn the_language_row_says_english_and_its_click_says_more_are_coming() {
        let mut fixture = Fixture::new(Page::General);
        let painted = fixture.settle();
        assert!(painted.text(LANGUAGE_LABEL).is_some());
        let at = painted.text("English").expect("the button").2.center();
        fixture.frame(click_at(at));
        assert_eq!(crate::kit::toast::current(&fixture.ctx).as_deref(), Some("More languages are on the way"));
    }
    #[test]
    fn the_screen_paints_its_ground_and_cards_from_the_palette() {
        use crate::kit::tokens::{set_palette, Palette};
        let mut fixture = Fixture::new(Page::General);
        set_palette(&fixture.ctx, Palette::DARK);
        let painted = fixture.settle();
        assert!(painted.rects.iter().any(|r| r.fill == Palette::DARK.window_bg && r.rect.width() > 900.0), "the ground");
        assert!(painted.rects.iter().any(|r| r.fill == Palette::DARK.card), "the cards");
        assert!(!painted.rects.iter().any(|r| r.fill == Palette::LIGHT.window_bg), "a light ground under a dark theme");
    }
}

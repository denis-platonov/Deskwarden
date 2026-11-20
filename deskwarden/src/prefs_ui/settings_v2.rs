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

use eframe::egui::{self, CornerRadius, Margin, Pos2, Rect, Sense, Ui, UiBuilder, Vec2};

use super as prefs;
use super::PrefsState;
use crate::generator_mode::GeneratorMode;
use crate::kit::card as kit_card;
use crate::kit::controls::{self, ButtonKind};
use crate::kit::icon;
use crate::kit::tokens::{self, palette, Role};
use crate::theme;

/// The nav's heading.
pub fn nav_heading() -> &'static str {
    crate::i18n::t("settings.nav_heading")
}
/// The Back affordance at the top of the nav.
pub fn back_label() -> &'static str {
    crate::i18n::t("settings.back_label")
}

pub const NAV_WIDTH: f32 = 232.0;
const COLUMN_MAX: f32 = 760.0;
const PAGE_GAP: f32 = 18.0;
const GROUP_GAP: f32 = 10.0;

const HEADING: Role = Role { size: 26.0, face: theme::EXTRABOLD, tracking_em: -0.02 };
const PAGE_TITLE: Role = Role { size: 30.0, face: theme::EXTRABOLD, tracking_em: -0.03 };

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
            Page::General => crate::i18n::t("settings.page_general"),
            Page::Autofill => crate::i18n::t("settings.page_autofill"),
            Page::Apps => crate::i18n::t("settings.page_apps"),
            Page::Security => crate::i18n::t("settings.page_security"),
            Page::Vault => crate::i18n::t("settings.page_vault"),
            Page::Generator => crate::i18n::t("settings.page_generator"),
            Page::Notifications => crate::i18n::t("settings.page_notifications"),
            Page::ImportExport => crate::i18n::t("settings.page_import_export"),
            Page::Devices => crate::i18n::t("settings.page_devices"),
            Page::Shortcuts => crate::i18n::t("settings.page_shortcuts"),
            Page::Account => crate::i18n::t("settings.page_account"),
            Page::About => crate::i18n::t("settings.page_about"),
        }
    }

    /// The spec's subtitle, for the pages that have one.
    pub fn subtitle(self) -> Option<String> {
        Some(match self {
            Page::General => crate::i18n::t("settings.subtitle_general").into(),
            Page::Autofill => crate::i18n::t("settings.subtitle_autofill").into(),
            Page::Apps => crate::i18n::t("settings.subtitle_apps").into(),
            Page::Security => crate::i18n::t("settings.subtitle_security").into(),
            Page::Vault => crate::i18n::t("settings.subtitle_vault").into(),
            Page::Generator => crate::i18n::t("settings.subtitle_generator").into(),
            Page::Notifications => crate::i18n::t("settings.subtitle_notifications").into(),
            Page::ImportExport => crate::i18n::t("settings.subtitle_import_export").into(),
            Page::Devices => crate::i18n::t("settings.subtitle_devices").into(),
            Page::Shortcuts => return None,
            Page::Account => return None,
            Page::About => crate::i18n::tf("settings.subtitle_about", &[("version", &(env!("CARGO_PKG_VERSION")).to_string())]),
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
        back = nav_row(ui, icon::glyph::BACK, back_label(), false, 36.0);
        ui.add_space(8.0);
        egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 14 }).show(ui, |ui| {
            ui.label(tokens::job(HEADING, nav_heading(), p.ink));
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
    let mut scroll = egui::ScrollArea::vertical().id_salt(("settings-page", *page as u8)).auto_shrink([false, false]);
    if let Some(y) = state.scroll_y {
        scroll = scroll.vertical_scroll_offset(y);
    }
    scroll.show(ui, |ui| {
            let available = ui.available_width();
            let column = (available - 80.0).clamp(320.0, COLUMN_MAX);
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
            crate::i18n::t("settings.notifications_unavailable"),
            crate::i18n::t("settings.arrives_later"),
        ),
        Page::ImportExport => import_export(ui),
        Page::Devices => not_yet(
            ui,
            crate::i18n::t("settings.devices_unavailable"),
            crate::i18n::t("settings.arrives_later"),
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
    group(ui, Some(crate::i18n::t("settings.group_startup")), |ui| {
        card(ui, |ui| {
            startup_rows(ui, state);
            if state.startup.is_some() {
                kit_card::row_rule(ui);
            }
            value_row(ui, keep_running_label(), "", keep_running_tip(), keep_running_value());
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_look")), |ui| {
        card(ui, |ui| {
            use crate::kit::tokens::{SidebarStyle, TextSize, ThemeChoice};
            let now = if state.settings.sidebar_style == SidebarStyle::Navy { 0 } else { 1 };
            if let Some(picked) = segment_row(ui, prefs::sidebar_style_label(), "", "", &[crate::i18n::t("settings.sidebar_navy"), crate::i18n::t("settings.sidebar_light")], now) {
                state.settings.sidebar_style = if picked == 0 { SidebarStyle::Navy } else { SidebarStyle::Light };
            }
            kit_card::row_rule(ui);
            let themes = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System];
            let now = themes.iter().position(|t| *t == state.settings.theme).unwrap_or(0);
            if let Some(picked) = segment_row(ui, theme_label(), "", "", &[crate::i18n::t("settings.theme_light"), crate::i18n::t("settings.theme_dark"), crate::i18n::t("settings.theme_system")], now) {
                state.settings.theme = themes[picked];
            }
            kit_card::row_rule(ui);
            let sizes = [TextSize::Default, TextSize::Large, TextSize::Larger];
            let now = sizes.iter().position(|t| *t == state.settings.text_size).unwrap_or(0);
            if let Some(picked) = segment_row(ui, text_size_label(), "", "", &[crate::i18n::t("settings.text_default"), crate::i18n::t("settings.text_large"), crate::i18n::t("settings.text_larger")], now) {
                state.settings.text_size = sizes[picked];
            }
            kit_card::row_rule(ui);
            super::language_picker::language_row(ui, &mut state.settings, language_label());
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
    switch_row(ui, open_at_sign_in_label(), "", "", &mut open, true);
    kit_card::row_rule(ui);
    let mut tray = if now.open_at_sign_in { now.in_tray } else { state.tray_when_opened };
    switch_row(ui, start_in_tray_label(), "", "", &mut tray, now.open_at_sign_in);
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
        crate::kit::toast::show(ui.ctx(), startup_failed_toast());
    }
}

/// The Startup rows' wording (spec C 2.4).
pub fn open_at_sign_in_label() -> &'static str {
    crate::i18n::t("settings.open_at_sign_in_label")
}
pub fn start_in_tray_label() -> &'static str {
    crate::i18n::t("settings.start_in_tray_label")
}
/// Closing the window only ever hides it: the tray keeps autofill running.
pub fn keep_running_label() -> &'static str {
    crate::i18n::t("settings.keep_running_label")
}
/// The design's tip for this row, word for word.
fn keep_running_tip() -> &'static str {
    crate::i18n::t("settings.keep_running_tip")
}
fn keep_running_value() -> &'static str {
    crate::i18n::t("settings.keep_running_value")
}
fn startup_failed_toast() -> &'static str {
    crate::i18n::t("settings.startup_failed_toast")
}
/// The Security rows' wording (spec C 2.4).
pub fn idle_lock_tip() -> &'static str {
    crate::i18n::t("settings.idle_lock_tip")
}
pub fn idle_minutes_sub() -> &'static str {
    crate::i18n::t("settings.idle_minutes_sub")
}
pub fn idle_minutes_tip() -> &'static str {
    crate::i18n::t("settings.idle_minutes_tip")
}
pub fn lock_on_sleep_label() -> &'static str {
    crate::i18n::t("settings.lock_on_sleep_label")
}
pub fn lock_on_screen_lock_label() -> &'static str {
    crate::i18n::t("settings.lock_on_screen_lock_label")
}
pub fn hide_on_share_label() -> &'static str {
    crate::i18n::t("settings.hide_on_share_label")
}
/// The design's tip, kept to what the app does: the whole window (not only
/// the passwords) stays out of the shared picture.
fn hide_on_share_tip() -> &'static str {
    crate::i18n::t("settings.hide_on_share_tip")
}
/// The Look rows' wording (spec C 2.4).
pub fn theme_label() -> &'static str {
    crate::i18n::t("settings.theme_label")
}
pub fn text_size_label() -> &'static str {
    crate::i18n::t("settings.text_size_label")
}
pub fn language_label() -> &'static str {
    crate::i18n::t("settings.language_label")
}

fn autofill(ui: &mut Ui, state: &mut PrefsState, page: &mut Page) {
    group(ui, Some(crate::i18n::t("settings.group_suggestions")), |ui| {
        card(ui, |ui| {
            switch_row(ui, prefs::prompt_label(), "", prefs::prompt_tip(), &mut state.settings.prompt_on_match, true);
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_sign_in_steps")), |ui| {
        card(ui, |ui| {
            if button_row(ui, default_steps_label(), default_steps_sub(), "", default_steps_value(), false) {
                crate::kit::toast::show(ui.ctx(), default_steps_toast());
            }
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_shortcut")), |ui| {
        card(ui, |ui| {
            // The spec's wording: the keys joined by a space (`Ctrl Alt B`).
            let chord = prefs::chord_keys(false, state.settings.shortcuts.picker).join(" ");
            if button_row(ui, fill_shortcut_label(), fill_shortcut_sub(), "", &chord, false) {
                *page = Page::Shortcuts;
            }
        });
    });
}

/// Autofill > Sign-in steps: what an app sign-in types when its login stores
/// no sequence of its own (`key_sequence::DEFAULT_SEQUENCE`). Read-only; the
/// steps of one login are edited on that login's Edit page.
pub fn default_steps_label() -> &'static str {
    crate::i18n::t("settings.default_steps_label")
}
fn default_steps_value() -> &'static str {
    crate::i18n::t("settings.default_steps_value")
}
/// The design says "Username, Tab, password, Enter"; the standard steps here
/// type no Enter, so the sub says what they really type.
fn default_steps_sub() -> &'static str {
    crate::i18n::t("settings.default_steps_sub")
}
pub fn default_steps_toast() -> &'static str {
    crate::i18n::t("settings.default_steps_toast")
}

/// The Autofill page's pointer at the Shortcuts page.
pub fn fill_shortcut_label() -> &'static str {
    crate::i18n::t("settings.fill_shortcut_label")
}
fn fill_shortcut_sub() -> &'static str {
    crate::i18n::t("settings.fill_shortcut_sub")
}

fn apps(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some(crate::i18n::t("settings.group_local_api")), |ui| prefs::api_cards(ui, state));
}

fn security(ui: &mut Ui, state: &mut PrefsState) {
    super::security_score::draw(ui);
    group(ui, Some(crate::i18n::t("settings.group_locking")), |ui| {
        // One card in this page's row style: the idle lock, its minutes,
        // and the two away events (the 0.x Lock card's rows, folded in).
        card(ui, |ui| {
            switch_row(ui, prefs::auto_lock_enabled_label(), "", idle_lock_tip(), &mut state.settings.auto_lock_enabled, true);
            kit_card::row_rule(ui);
            let enabled = state.settings.auto_lock_enabled;
            let stepper_w = prefs::STEPPER_STEP_WIDTH * 2.0 + prefs::STEPPER_VALUE_WIDTH;
            row(ui, prefs::auto_lock_label(), idle_minutes_sub(), idle_minutes_tip(), !enabled, stepper_w, |ui| {
                state.settings.auto_lock_minutes =
                    prefs::minutes_stepper(ui, state.settings.auto_lock_minutes, &mut state.auto_lock_text, enabled);
            });
            kit_card::row_rule(ui);
            let s = &mut state.settings;
            switch_row(ui, lock_on_sleep_label(), "", "", &mut s.lock_on_sleep, true);
            kit_card::row_rule(ui);
            switch_row(ui, lock_on_screen_lock_label(), "", "", &mut s.lock_on_screen_lock, true);
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_privacy")), |ui| {
        prefs::draw_clipboard(ui, state);
        card(ui, |ui| {
            switch_row(ui, hide_on_share_label(), "", hide_on_share_tip(), &mut state.settings.hide_on_screen_share, true);
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_breaches")), |ui| prefs::draw_breaches(ui, state));
}

fn vault(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some(crate::i18n::t("settings.group_display")), |ui| {
        card(ui, |ui| {
            let s = &mut state.settings;
            switch_row(ui, prefs::fetch_icons_label(), "", prefs::fetch_icons_tip(), &mut s.fetch_icons, true);
            kit_card::row_rule(ui);
            let icons_on = s.fetch_icons;
            switch_row(ui, prefs::direct_icons_label(), prefs::direct_icons_sub(), prefs::direct_icons_tip(), &mut s.fetch_icons_direct, icons_on);
            kit_card::row_rule(ui);
            let direct_on = icons_on && s.fetch_icons_direct;
            switch_row(ui, prefs::lan_tls_label(), prefs::lan_tls_sub(), prefs::lan_tls_tip(), &mut s.icons_ignore_tls_on_private_hosts, direct_on);
            kit_card::row_rule(ui);
            switch_row(ui, prefs::brand_logos_label(), prefs::brand_logos_sub(), prefs::brand_logos_tip(), &mut s.use_brand_logos, true);
            kit_card::row_rule(ui);
            switch_row(ui, prefs::totp_secret_label(), prefs::totp_secret_sub(), prefs::totp_secret_tip(), &mut s.reveal_totp_seed, true);
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_opening_the_vault")), |ui| {
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
            if let Some(picked) = segment_row(ui, crate::i18n::t("settings.generator_mode_label"), "", "", &[crate::i18n::t("settings.mode_password"), crate::i18n::t("settings.mode_passphrase"), crate::i18n::t("settings.mode_pin")], now) {
                g.mode = modes[picked];
            }
            kit_card::row_rule(ui);
            let length = match g.mode {
                GeneratorMode::Password => crate::i18n::tf("settings.length_characters", &[("count", &g.length.to_string())]),
                GeneratorMode::Passphrase => crate::i18n::tf("settings.length_words", &[("count", &g.words.to_string())]),
                GeneratorMode::Pin => crate::i18n::tf("settings.length_digits", &[("count", &g.pin_length.to_string())]),
            };
            value_row(ui, crate::i18n::t("settings.length_label"), "", "", &length);
            kit_card::row_rule(ui);
            switch_row(ui, crate::i18n::t("settings.include_symbols_label"), "", "", &mut g.symbols, true);
            kit_card::row_rule(ui);
            switch_row(ui, crate::i18n::t("settings.avoid_lookalikes_label"), "", "", &mut g.avoid_lookalikes, true);
        });
    });
}

fn shortcuts(ui: &mut Ui, state: &mut PrefsState) {
    let live = crate::app::FillShortcut::ALL.map(crate::hotkey::availability);
    group(ui, None, |ui| prefs::draw_shortcuts(ui, state, live));
}

fn account(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, Some(crate::i18n::t("settings.group_account")), |ui| prefs::card(ui, |ui| prefs::account_row(ui, (state.account_source)())));
    group(ui, Some(crate::i18n::t("settings.group_sync")), |ui| prefs::draw_sync_poll_card(ui, state));
    group(ui, Some(crate::i18n::t("settings.group_sharing")), |ui| prefs::draw_sharing_card(ui, state));
}

fn about(ui: &mut Ui, state: &mut PrefsState) {
    group(ui, None, |ui| {
        card(ui, |ui| {
            value_row(ui, crate::i18n::t("settings.about_version_label"), crate::i18n::t("settings.about_version_sub"), "", &prefs::version_line());
        });
    });
    group(ui, Some(crate::i18n::t("settings.group_updates")), |ui| prefs::draw_updates(ui, state));
    group(ui, Some(crate::i18n::t("settings.group_support")), |ui| {
        card(ui, |ui| {
            if button_row(ui, help_label(), "", "", crate::i18n::t("settings.button_open"), false) {
                crate::vault_window::webbrowser_open(HELP_URL);
            }
            kit_card::row_rule(ui);
            if button_row(ui, feedback_label(), "", "", crate::i18n::t("settings.button_write"), false) {
                crate::vault_window::webbrowser_open(FEEDBACK_URL);
            }
            kit_card::row_rule(ui);
            // The Store asks for the policy to be reachable from inside the app.
            if button_row(ui, privacy_label(), "", "", crate::i18n::t("settings.button_open"), false) {
                crate::vault_window::webbrowser_open(PRIVACY_URL);
            }
            kit_card::row_rule(ui);
            if button_row(ui, logs_label(), "", logs_tip(), crate::i18n::t("settings.button_copy"), false) {
                match log_location() {
                    Some(path) => {
                        ui.ctx().copy_text(path);
                        crate::kit::toast::show(ui.ctx(), logs_copied_toast());
                    }
                    None => crate::kit::toast::show(ui.ctx(), logs_missing_toast()),
                }
            }
        });
    });
}

/// About > Support (spec C 2.4). Both open the project's pages in the browser.
pub fn help_label() -> &'static str {
    crate::i18n::t("settings.help_label")
}
pub fn feedback_label() -> &'static str {
    crate::i18n::t("settings.feedback_label")
}
pub fn logs_label() -> &'static str {
    crate::i18n::t("settings.logs_label")
}
const HELP_URL: &str = "https://github.com/denis-platonov/deskwarden#readme";
const FEEDBACK_URL: &str = "https://github.com/denis-platonov/deskwarden/issues/new";
pub fn privacy_label() -> &'static str {
    crate::i18n::t("settings.privacy_label")
}
const PRIVACY_URL: &str = "https://github.com/denis-platonov/deskwarden/blob/main/PRIVACY.md";
/// The design's tip says logs never hold passwords; this screen cannot vouch
/// for every line, so the tip says what the button really does.
fn logs_tip() -> &'static str {
    crate::i18n::t("settings.logs_tip")
}
pub fn logs_copied_toast() -> &'static str {
    crate::i18n::t("settings.logs_copied_toast")
}
pub fn logs_missing_toast() -> &'static str {
    crate::i18n::t("settings.logs_missing_toast")
}

/// Where `deskwarden.log` is, as text, when there is one. **The path, never
/// the file**: a log is written by many parts of the app and this screen
/// cannot vouch for every line, so what leaves it is a location the user can
/// open and read before they decide to share anything.
fn log_location() -> Option<String> {
    let path = crate::logging::log_file_path(&crate::settings::config_dir()?);
    path.exists().then(|| path.display().to_string())
}

/// One marked-disabled row, for a page whose behaviour is not built yet.
/// Slice 9's import wizard, opened through the window's shell request.
fn import_export(ui: &mut Ui) {
    group(ui, None, |ui| {
        card(ui, |ui| {
            if button_row(
                ui,
                crate::i18n::t("settings.import_label"),
                crate::i18n::t("settings.import_sub"),
                crate::i18n::t("settings.import_tip"),
                crate::i18n::t("settings.import_button"),
                false,
            ) {
                crate::vault_window::sidebar::request_shell(ui.ctx(), crate::vault_window::sidebar::ShellRequest::OpenImport);
            }
        });
    });
}

fn not_yet(ui: &mut Ui, title: &str, sub: &str) {
    group(ui, None, |ui| {
        card(ui, |ui| row(ui, title, sub, "", true, 0.0, |_| {}));
    });
}

// ---------------------------------------------------------------------------
// Groups, cards and the four row kinds
// ---------------------------------------------------------------------------

/// A group: an optional tracked-capitals title over a card, 10 apart.
fn group(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = GROUP_GAP;
        if let Some(title) = title {
            prefs::group_title(ui, title);
        }
        add(ui);
    });
}

/// The v2 card the rows live in: rows carry their own padding.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    prefs::card(ui, add);
}

/// The row frame: title, an optional `?` tip dot and an optional sub on the
/// left (the design's rule: a few words of sub at most, the explanation in the
/// tip), and a control `control_w` wide on the right. One implementation, shared with the panes in the parent
/// module, so every row on every page is the same row.
fn row(ui: &mut Ui, title: &str, sub: &str, tip: &str, ghost: bool, control_w: f32, control: impl FnOnce(&mut Ui)) {
    prefs::control_row_tip(ui, title, sub, tip, ghost, control_w, control);
}

/// `sw`: a switch bound to a setting. With `enabled` false it is painted
/// inert and `value` is left alone.
fn switch_row(ui: &mut Ui, title: &str, sub: &str, tip: &str, value: &mut bool, enabled: bool) {
    row(ui, title, sub, tip, !enabled, 50.0, |ui| {
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
fn segment_row(ui: &mut Ui, title: &str, sub: &str, tip: &str, options: &[&str], selected: usize) -> Option<usize> {
    let width = controls::segmented_width(ui.ctx(), options, controls::SEG_SETTINGS);
    let mut picked = None;
    row(ui, title, sub, tip, false, width, |ui| picked = controls::segmented(ui, options, selected, controls::SEG_SETTINGS));
    picked
}

/// `bt`: an outline button; whether it was clicked.
fn button_row(ui: &mut Ui, title: &str, sub: &str, tip: &str, label: &str, _danger: bool) -> bool {
    let width = controls::button_width(ui.ctx(), ButtonKind::Outline, controls::ROW, label, false);
    let mut clicked = false;
    row(ui, title, sub, tip, false, width, |ui| {
        clicked = controls::button(ui, ButtonKind::Outline, controls::ROW, label, None).clicked();
    });
    clicked
}

/// `va`: a read-only value.
fn value_row(ui: &mut Ui, title: &str, sub: &str, tip: &str, value: &str) {
    prefs::value_row(ui, title, sub, tip, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at, hover_at};
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
        assert!(painted.text(nav_heading()).is_some());
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
        assert_eq!(fixture.click_text(back_label()), ScreenAction::Back);
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
            (prefs::sidebar_style_label(), "sidebar style"),
            (prefs::prompt_label(), "autofill prompts"),
            (prefs::service_label(), "local API"),
            (prefs::auto_lock_enabled_label(), "auto-lock"),
            (prefs::auto_lock_label(), "auto-lock minutes"),
            (prefs::clipboard_master_label(), "clipboard clearing"),
            (prefs::clipboard_on_lock_label(), "clear on lock"),
            (prefs::breach_label(), "breach check"),
            (prefs::scan_button_text(), "breach scan"),
            (prefs::fetch_icons_label(), "site icons"),
            (prefs::direct_icons_label(), "direct icons"),
            (prefs::lan_tls_label(), "lan tls"),
            (prefs::brand_logos_label(), "card logos"),
            (prefs::totp_secret_label(), "totp secret"),
            (prefs::ui_loaded_label(), "open instantly"),
            (prefs::backend_choice_label(), "backend choice"),
            (prefs::disk_cache_label(), "disk cache"),
            (prefs::sync_push_label(), "live updates"),
            (prefs::sync_poll_enabled_label(), "polling"),
            (prefs::advanced_sharing_label(), "advanced sharing"),
            (prefs::update_check_label(), "automatic update check"),
            (prefs::update_check_button(), "update check"),
            (prefs::account_label(), "account row"),
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
        let title = painted.text(prefs::prompt_label()).expect("the row").2;
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
        let chord = prefs::chord_keys(false, fixture.state.settings.shortcuts.picker).join(" ");
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
    fn the_about_page_has_the_support_rows_and_https_links() {
        let painted = Fixture::new(Page::About).settle();
        for label in [help_label(), feedback_label(), privacy_label(), logs_label()] {
            assert!(painted.text(label).is_some(), "{label} is not drawn");
        }
        for url in [HELP_URL, FEEDBACK_URL, PRIVACY_URL] {
            assert!(url.starts_with("https://github.com/denis-platonov/deskwarden"), "{url}");
        }
    }

    /// The Store edition says where updates come from instead of offering
    /// the switch; the default edition keeps the switch.
    #[test]
    fn the_updates_group_matches_the_edition() {
        let notice = "Updates come from the Microsoft Store";
        let mut store = Fixture::new(Page::About);
        store.state.updates_from_store = Some(notice);
        let painted = store.settle();
        assert!(painted.text(notice).is_some(), "the Store notice is not drawn");
        assert!(painted.text(prefs::update_check_label()).is_none(), "a Store build offers the update switch");

        let painted = Fixture::new(Page::About).settle();
        assert!(painted.text(prefs::update_check_label()).is_some(), "the update switch is gone");
    }

    /// The production constructor takes the edition from the updater.
    #[test]
    fn the_app_reads_the_edition_from_the_updater() {
        let state = PrefsState::with_scan_history(Settings::default());
        assert_eq!(state.updates_from_store, crate::updater::store_updates_notice());
    }

    #[test]
    fn copying_the_logs_says_whether_it_had_a_file_to_name() {
        let mut fixture = Fixture::new(Page::About);
        let painted = fixture.settle();
        let copy = painted.texts.iter().rfind(|(t, _, _)| t == "Copy").expect("a Copy button").2.center();
        fixture.frame(click_at(copy));
        let toast = crate::kit::toast::current(&fixture.ctx).expect("a toast");
        assert!(toast == logs_copied_toast() || toast == logs_missing_toast(), "{toast}");
        assert_eq!(log_location().is_some(), toast == logs_copied_toast());
    }

    #[test]
    fn the_default_steps_row_says_what_a_login_without_steps_types() {
        assert_eq!(crate::key_sequence::DEFAULT_SEQUENCE, "{USERNAME}{TAB}{PASSWORD}");
        let mut fixture = Fixture::new(Page::Autofill);
        let painted = fixture.settle();
        assert!(painted.text(default_steps_label()).is_some());
        let at = painted.texts.iter().find(|(t, _, _)| t == default_steps_value()).expect("the value button").2.center();
        fixture.frame(click_at(at));
        assert_eq!(crate::kit::toast::current(&fixture.ctx).as_deref(), Some(default_steps_toast()));
    }

    /// The `?` dot nearest the row titled `title`.
    fn dot_after(painted: &testing::Painted, title: &str) -> Pos2 {
        let at = painted.texts.iter().find(|(t, _, _)| t == title).unwrap_or_else(|| panic!("no {title:?} painted")).2;
        painted
            .texts
            .iter()
            .filter(|(t, _, _)| t == "?")
            .map(|(_, _, rect)| rect.center())
            .min_by(|a, b| at.distance_to_pos(*a).total_cmp(&at.distance_to_pos(*b)))
            .expect("a ? dot")
    }

    #[test]
    fn a_rows_tip_is_a_dot_after_its_title_that_shows_the_text_on_hover() {
        // The design's rule: the explanation is behind a `?`, not under the
        // title. Keep running is the design's own example, word for word.
        let mut fixture = Fixture::new(Page::General);
        let idle = fixture.settle();
        assert!(idle.text(keep_running_tip()).is_none(), "the tip is painted without a hover");
        let title = idle.texts.iter().find(|(t, _, _)| t == keep_running_label()).expect("the title").2;
        let dot = dot_after(&idle, keep_running_label());
        assert!(dot.x > title.right(), "the dot is not after the title: {dot:?} vs {title:?}");
        assert!((dot.y - title.center().y).abs() < 10.0, "the dot is not on the title's line");
        fixture.frame(hover_at(dot));
        let shown = fixture.settle();
        assert!(shown.text(keep_running_tip()).is_some(), "hovering the dot showed nothing");
        // A row with no tip paints no dot of its own.
        let mut fixture = Fixture::new(Page::Generator);
        assert!(fixture.settle().texts.iter().all(|(t, _, _)| t != "?"), "the Generator rows carry no tips");
    }

    #[test]
    fn no_settings_row_keeps_a_paragraph_under_its_title() {
        // The design's rule for a sub: a few words. Long copy is a tip. The
        // status lines (refusals, scan and update stages) are not row copy,
        // and the pages are painted in their ordinary state.
        for page in Page::ALL {
            // The Shortcuts rows say WHY a chord is not working, a status
            // that depends on what the process has registered; their
            // ordinary state is pinned in `prefs_ui`'s tests.
            if page == Page::Shortcuts {
                continue;
            }
            let mut fixture = Fixture::new(page);
            fixture.state.settings.use_official_bw_crypto = Some(false);
            let painted = fixture.settle();
            for (text, _, rect) in &painted.texts {
                if text.split_whitespace().count() > 14 {
                    // Long text on the page must be a tip bubble (never
                    // painted here without a hover) or a status or empty
                    // state, which the allow-list below names.
                    let allowed = [
                        crate::breach_scan::scan_consent_note(),
                        prefs::keys_none(),
                    ];
                    assert!(
                        allowed.contains(&text.as_str()),
                        "a {}-word line is painted on {:?} at {rect:?}: {text:?}",
                        text.split_whitespace().count(),
                        page.label()
                    );
                }
            }
        }
    }

    #[test]
    fn a_disabled_child_switch_leaves_its_value_alone() {
        let mut fixture = Fixture::new(Page::Vault);
        fixture.state.settings.fetch_icons = false;
        fixture.state.settings.fetch_icons_direct = true;
        fixture.settle();
        let painted = fixture.settle();
        let at = painted.text(prefs::direct_icons_label()).expect("the child row").2;
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
        assert!(painted.text(open_at_sign_in_label()).is_none(), "a switch connected to nothing");
        assert!(painted.text(start_in_tray_label()).is_none());
        // The always-true row is honest without it.
        assert!(painted.text(keep_running_label()).is_some());
        assert!(painted.text("Always").is_some());
    }

    #[test]
    fn turning_open_at_sign_in_on_writes_the_logon_entry() {
        let mut fixture = with_registry(false, false, recording_sink);
        let painted = fixture.settle();
        assert!(painted.text(open_at_sign_in_label()).is_some());
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
        assert_eq!(crate::kit::toast::current(&fixture.ctx).as_deref(), Some(startup_failed_toast()));
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
        assert!(painted.text(theme_label()).is_some() && painted.text(text_size_label()).is_some());
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
        for label in [lock_on_sleep_label(), lock_on_screen_lock_label(), hide_on_share_label()] {
            assert!(painted.text(label).is_some(), "no {label}");
        }
        let at = painted.text(lock_on_sleep_label()).unwrap().2;
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

    /// With no language chosen the row says so, and opening it lists the
    /// system default first and then every language by its own name.
    #[test]
    fn the_language_row_offers_the_system_default_and_every_language_by_its_own_name() {
        crate::i18n::with_language("en", || {
            let mut fixture = Fixture::new(Page::General);
            let painted = fixture.settle();
            assert!(painted.text(language_label()).is_some());
            let system = crate::i18n::t("settings.language_system");
            assert!(painted.text(system).is_some(), "the closed row names the system default");
            assert!(painted.text("Deutsch").is_none(), "the menu is closed until it is clicked");
            fixture.click_text(system);
            assert!(super::super::language_picker::is_open(&fixture.ctx));
            let open = fixture.settle();
            for locale in crate::i18n::available() {
                assert!(open.text(locale.native_name).is_some(), "the menu does not list {}", locale.english_name);
            }
        });
    }

    /// Choosing a language writes it to the settings and changes the language
    /// in force; choosing the system default stores `None`.
    #[test]
    fn choosing_a_language_persists_it_and_changes_the_language_in_force() {
        crate::i18n::with_language("en", || {
            let mut fixture = Fixture::new(Page::General);
            let system = crate::i18n::t("settings.language_system");
            fixture.click_text(system);
            fixture.click_text("Deutsch");
            assert_eq!(fixture.state.settings.language.as_deref(), Some("de"));
            assert_eq!(crate::i18n::current().tag, "de");
            assert!(!super::super::language_picker::is_open(&fixture.ctx), "choosing closes the menu");
            let painted = fixture.settle();
            assert!(painted.text("Deutsch").is_some(), "the row now names the chosen language");

            fixture.click_text("Deutsch");
            fixture.click_text(system);
            assert_eq!(fixture.state.settings.language, None, "the system default is stored as no choice");
            assert_eq!(crate::i18n::current().tag, crate::i18n::system_default().tag);
        });
    }

    /// Esc, and a click elsewhere, close the menu without choosing.
    #[test]
    fn the_language_menu_closes_without_choosing_on_escape() {
        crate::i18n::with_language("en", || {
            let mut fixture = Fixture::new(Page::General);
            fixture.click_text(crate::i18n::t("settings.language_system"));
            assert!(super::super::language_picker::is_open(&fixture.ctx));
            fixture.frame(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert!(!super::super::language_picker::is_open(&fixture.ctx));
            assert_eq!(fixture.state.settings.language, None);
        });
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

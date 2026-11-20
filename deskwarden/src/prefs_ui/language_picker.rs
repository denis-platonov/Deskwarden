//! Settings > General > Language: the picker that lists "System default
//! (Windows)" and every language the app has a catalog for, each named in
//! itself.
//!
//! The hook is the one `docs/superpowers/plans/2026-11-19-i18n.md` describes:
//!
//! * while the menu is open every script's system font is loaded
//!   ([`theme::set_fonts_for`]) so each name is drawn in its own letters, and
//!   the extra faces are dropped again when it closes;
//! * choosing writes `settings.language` (`None` for the system default),
//!   calls [`i18n::init`] so the next string asked for is in the new language,
//!   re-applies the theme so the new language's fonts load, and repaints;
//! * `settings.json` takes the value the way every other preference does (the
//!   window hands the edit to the daemon, which re-runs [`i18n::init`] for the
//!   tray and the cards it draws).

use eframe::egui::{self, Ui, Vec2};

use crate::i18n::{self, Fonts};
use crate::kit::controls::{self, ButtonKind};
use crate::kit::menu as kit_menu;
use crate::kit::tokens::palette;
use crate::settings::Settings;
use crate::theme;

/// Every language whose name needs a font the bundled faces do not have. Held
/// for as long as the menu is open.
const EVERY_SCRIPT: [Fonts; 5] =
    [Fonts::Greek, Fonts::ChineseSimplified, Fonts::ChineseTraditional, Fonts::Japanese, Fonts::Korean];

const MENU_WIDTH: f32 = 300.0;
const MENU_MAX_HEIGHT: f32 = 360.0;

fn open_id() -> egui::Id {
    egui::Id::new("settings-language-menu-open")
}

fn area_id() -> egui::Id {
    egui::Id::new("settings-language-menu")
}

/// The id of one option's row: `None` is "System default (Windows)".
pub fn option_id(tag: Option<&str>) -> egui::Id {
    egui::Id::new(("settings-language-option", tag.unwrap_or("")))
}

pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(open_id())).unwrap_or(false)
}

/// Opens the menu without a click: for `ui_preview`, which has no pointer.
pub fn open(ctx: &egui::Context) {
    set_open(ctx, true);
}

fn set_open(ctx: &egui::Context, open: bool) {
    if is_open(ctx) == open {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(open_id(), open));
    // Opening loads every script so each name is drawn in its own letters;
    // closing puts back only the language in force.
    theme::set_fonts_for(ctx, if open { &EVERY_SCRIPT } else { &[] });
    ctx.request_repaint();
}

/// The language a stored setting means, or `None` for "follow Windows" -- which
/// is also what a tag with no catalog means, as [`Settings::language`] says.
fn chosen(saved: Option<&str>) -> Option<&'static i18n::Locale> {
    saved.and_then(i18n::match_tag).and_then(|i| i18n::available().get(i))
}

/// What the row's button says: the chosen language in itself, or the system
/// default's name.
pub fn current_label(saved: Option<&str>) -> String {
    match chosen(saved) {
        Some(locale) => locale.native_name.to_string(),
        None => crate::i18n::t("settings.language_system").to_string(),
    }
}

/// Chooses a language: writes the setting, switches the language in force,
/// loads its fonts and repaints. `None` is "System default (Windows)".
pub fn choose(ctx: &egui::Context, settings: &mut Settings, tag: Option<&str>) {
    settings.language = tag.map(str::to_owned);
    i18n::init(settings.language.as_deref());
    ctx.data_mut(|d| d.insert_temp(open_id(), false));
    theme::apply(ctx);
    ctx.request_repaint();
}

/// The Language row on General, and its menu while it is open.
pub fn language_row(ui: &mut Ui, settings: &mut Settings, title: &str) {
    let label = current_label(settings.language.as_deref());
    let width = controls::button_width(ui.ctx(), ButtonKind::Outline, controls::ROW, &label, false);
    let mut anchor = None;
    let mut clicked = false;
    super::control_row_tip(ui, title, "", "", false, width, |ui| {
        let response = controls::button(ui, ButtonKind::Outline, controls::ROW, &label, None);
        clicked = response.clicked();
        anchor = Some(response.rect);
    });
    let ctx = ui.ctx().clone();
    if clicked {
        set_open(&ctx, !is_open(&ctx));
    }
    let Some(anchor) = anchor else { return };
    if !is_open(&ctx) {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        set_open(&ctx, false);
        return;
    }
    let p = palette(&ctx);
    let selected = chosen(settings.language.as_deref()).map(|l| l.tag);
    let mut picked: Option<Option<&'static str>> = None;
    // Below the button when it fits, above it when the row is near the
    // window's foot (egui would otherwise slide the menu up over the button).
    let screen = ctx.content_rect();
    let room_below = screen.bottom() - anchor.bottom();
    let room_above = anchor.top() - screen.top();
    let upward = room_below < MENU_MAX_HEIGHT + 24.0 && room_above > room_below;
    let (pivot, at) = if upward {
        (egui::Align2::RIGHT_BOTTOM, anchor.right_top() - Vec2::new(0.0, 8.0))
    } else {
        (egui::Align2::RIGHT_TOP, anchor.right_bottom() + Vec2::new(0.0, 8.0))
    };
    let look = kit_menu::MENU.at_least(MENU_WIDTH);
    let shown = egui::Area::new(area_id())
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(at)
        .show(&ctx, |ui| {
            look.frame(&p)
                .show(ui, |ui| {
                    ui.set_width(look.content_width());
                    egui::ScrollArea::vertical().max_height(MENU_MAX_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
                        let system = crate::i18n::t("settings.language_system");
                        if option_row(ui, None, system, "", selected.is_none()) {
                            picked = Some(None);
                        }
                        for locale in i18n::available() {
                            let english = if locale.native_name == locale.english_name { "" } else { locale.english_name };
                            if option_row(ui, Some(locale.tag), locale.native_name, english, selected == Some(locale.tag)) {
                                picked = Some(Some(locale.tag));
                            }
                        }
                    });
                });
        });
    // A click anywhere else closes it, as the sign-in steps' add menu does.
    let outside = ctx.input(|i| {
        i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|at| !shown.response.rect.contains(at) && !anchor.contains(at))
    });
    match picked {
        Some(tag) => choose(&ctx, settings, tag),
        None if outside => set_open(&ctx, false),
        None => {}
    }
}

/// One option; whether it was clicked this frame. The kit's row, under this
/// option's own id, with the language's English name as its hint.
fn option_row(ui: &mut Ui, tag: Option<&str>, name: &str, english: &str, selected: bool) -> bool {
    let mut row = kit_menu::Item::new(name).selected(selected).id(option_id(tag));
    if !english.is_empty() {
        row = row.hint(english);
    }
    kit_menu::item(ui, row).clicked()
}

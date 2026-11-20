//! Spec C §1: the Generator as its own screen -- a password, a passphrase or
//! a PIN, with its options remembered between runs.

use eframe::egui::{self, Color32, Vec2};
use zeroize::Zeroizing;

use crate::generator_mode::{generator_strength, GeneratorLevel, GeneratorMode};
use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::settings::GeneratorPrefs;
use crate::theme;

pub fn title() -> &'static str {
    crate::i18n::t("generator.title")
}
pub fn sub() -> &'static str {
    crate::i18n::t("generator.sub")
}
pub fn modes() -> [&'static str; 3] {
    [crate::i18n::t("generator.mode_password"), crate::i18n::t("generator.mode_passphrase"), crate::i18n::t("generator.mode_pin")]
}
pub fn copy() -> &'static str {
    crate::i18n::t("generator.copy")
}
pub fn copied() -> &'static str {
    crate::i18n::t("generator.copied")
}
pub fn regenerate() -> &'static str {
    crate::i18n::t("generator.regenerate")
}
pub fn avoid_lookalikes() -> &'static str {
    crate::i18n::t("generator.avoid_lookalikes")
}
const WIDTH: f32 = 760.0;
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x87, 0x85);

/// What the screen did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratorAction {
    None,
    /// An option changed: the window writes `prefs` to settings.json.
    PrefsChanged,
}

/// The screen's state: the options and the value they made.
pub struct GeneratorScreen {
    pub prefs: GeneratorPrefs,
    value: Zeroizing<String>,
    error: Option<String>,
    stale: bool,
    /// Options changed and not yet written: the window writes them once the
    /// pointer is up, not on every tick of a slider drag.
    pub unsaved: bool,
}

impl GeneratorScreen {
    pub fn new(prefs: GeneratorPrefs) -> Self {
        Self { prefs: prefs.clamped(), value: Zeroizing::new(String::new()), error: None, stale: true, unsaved: false }
    }

    /// The value on screen.
    pub fn value(&self) -> &str {
        &self.value
    }

    fn regenerate(&mut self) {
        let made = match self.prefs.mode {
            GeneratorMode::Pin => crate::password_gen::generate_pin(self.prefs.pin_length as usize),
            GeneratorMode::Password => crate::password_gen::generate_password(&crate::vault_bridge::PasswordRecipe {
                length: self.prefs.length,
                uppercase: self.prefs.uppercase,
                lowercase: self.prefs.lowercase,
                number: self.prefs.numbers,
                special: self.prefs.symbols,
                avoid_ambiguous: self.prefs.avoid_lookalikes,
                ..Default::default()
            }),
            GeneratorMode::Passphrase => crate::password_gen::generate(&crate::vault_bridge::GenerateRequest::Passphrase(
                crate::vault_bridge::PassphraseRecipe { words: self.prefs.words, ..Default::default() },
            )),
        };
        match made {
            // Assigning drops (and so wipes) the value it replaces.
            Ok(value) => {
                self.value = value;
                self.error = None;
            }
            Err(e) => {
                self.value = Zeroizing::new(String::new());
                self.error = Some(crate::i18n::tf("generator.err_make", &[("error", &e.to_string())]));
            }
        }
        self.stale = false;
    }

    /// The password alphabet's size, for the strength line.
    /// The generator's real alphabet (spec C §1.3 falls back to 26 with no
    /// class on).
    fn pool(&self) -> usize {
        let p = &self.prefs;
        match crate::password_gen::alphabet_size(p.uppercase, p.lowercase, p.numbers, p.symbols, p.avoid_lookalikes) {
            0 => 26,
            n => n,
        }
    }

    fn size(&self) -> usize {
        match self.prefs.mode {
            GeneratorMode::Password => self.prefs.length as usize,
            GeneratorMode::Passphrase => self.prefs.words as usize,
            GeneratorMode::Pin => self.prefs.pin_length as usize,
        }
    }
}

pub fn mode_index(mode: GeneratorMode) -> usize {
    match mode {
        GeneratorMode::Password => 0,
        GeneratorMode::Passphrase => 1,
        GeneratorMode::Pin => 2,
    }
}

pub fn copy_id() -> egui::Id {
    egui::Id::new("generator-copy")
}

pub fn regenerate_id() -> egui::Id {
    egui::Id::new("generator-regenerate")
}

pub fn draw(ui: &mut egui::Ui, state: &mut GeneratorScreen) -> GeneratorAction {
    if state.stale {
        state.regenerate();
    }
    let before = state.prefs.clone();
    let width = WIDTH.min(ui.available_width() - 48.0).max(320.0);
    let side = ((ui.available_width() - width) / 2.0).max(24.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(28.0);
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(
                    egui::RichText::new(title())
                        .font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                        .color(theme::INK),
                );
                ui.label(egui::RichText::new(sub()).size(16.0).color(Color32::from_rgb(0x7d, 0x79, 0x79)));
                ui.add_space(20.0);
                value_card(ui, state);
                ui.add_space(16.0);
                options_card(ui, state);
            });
        });
    });
    state.prefs = state.prefs.clone().clamped();
    if state.prefs != before {
        state.regenerate();
        state.unsaved = true;
        GeneratorAction::PrefsChanged
    } else {
        GeneratorAction::None
    }
}

/// The kit's card (ring and soft shadow); a shadow alone reads as clipped
/// corners on the light page.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    crate::kit::card::card(ui, 18, egui::Margin::same(22), |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// The slider's empty rail.
const SLIDER_RAIL: Color32 = Color32::from_rgb(0xe6, 0xe3, 0xdf);

fn value_card(ui: &mut egui::Ui, state: &mut GeneratorScreen) {
    card(ui, |ui| {
        let size = ButtonSize { height: 48.0, pad_x: 20.0, radius: 14, font_px: 16.0 };
        let buttons = controls::button_width(ui.ctx(), ButtonKind::Primary, size, copy(), false)
            + controls::button_width(ui.ctx(), ButtonKind::Outline, size, regenerate(), false)
            + 24.0;
        ui.horizontal(|ui| {
            let room = (ui.available_width() - buttons).max(80.0);
            let text = match &state.error {
                Some(error) => egui::RichText::new(error).size(15.0).color(theme::ERROR),
                None => egui::RichText::new(state.value.as_str())
                    .font(egui::FontId::new(24.0, egui::FontFamily::Monospace))
                    .color(theme::INK),
            };
            ui.add_sized(Vec2::new(room, 48.0), egui::Label::new(text).truncate());
            let copy = controls::button(ui, ButtonKind::Primary, size, copy(), None);
            let _ = ui.interact(copy.rect, copy_id(), egui::Sense::hover());
            if copy.clicked() && !state.value.is_empty() {
                crate::clipboard::copy_secret(&state.value);
                crate::kit::toast::show(ui.ctx(), copied());
            }
            let again = controls::button(ui, ButtonKind::Outline, size, regenerate(), None);
            let _ = ui.interact(again.rect, regenerate_id(), egui::Sense::hover());
            if again.clicked() {
                state.regenerate();
            }
        });
        ui.add_space(10.0);
        let (bits, level) = generator_strength(state.prefs.mode, state.size(), state.pool());
        let ink = match level {
            GeneratorLevel::Weak => theme::ERROR,
            GeneratorLevel::Fair => Color32::from_rgb(0x8a, 0x5a, 0x06),
            _ => Color32::from_rgb(0x17, 0x67, 0x3a),
        };
        ui.horizontal(|ui| {
            ui.label(theme::bold(level.label(), 15.0).color(ink));
            ui.label(egui::RichText::new(crate::i18n::tf("generator.bits", &[("bits", &(bits.round() as u32).to_string())])).size(14.0).color(MUTED));
        });
    });
}

fn options_card(ui: &mut egui::Ui, state: &mut GeneratorScreen) {
    card(ui, |ui| {
        let selected = mode_index(state.prefs.mode);
        if let Some(picked) = controls::segmented(ui, &modes(), selected, controls::SEG_GENERATOR) {
            state.prefs.mode = [GeneratorMode::Password, GeneratorMode::Passphrase, GeneratorMode::Pin][picked];
        }
        ui.add_space(18.0);
        let (label, value, range) = match state.prefs.mode {
            GeneratorMode::Password => (crate::i18n::t("generator.length"), &mut state.prefs.length, 8..=64),
            GeneratorMode::Passphrase => (crate::i18n::t("generator.words"), &mut state.prefs.words, 3..=10),
            GeneratorMode::Pin => (crate::i18n::t("generator.digits"), &mut state.prefs.pin_length, 4..=12),
        };
        ui.horizontal(|ui| {
            ui.label(theme::bold(label, 15.0).color(theme::INK));
            ui.add_space(12.0);
            // egui's default rail is the card's own white: paint it a
            // visible grey with a blue trailing fill.
            let visuals = ui.visuals_mut();
            visuals.widgets.inactive.bg_fill = SLIDER_RAIL;
            visuals.widgets.inactive.weak_bg_fill = SLIDER_RAIL;
            visuals.selection.bg_fill = theme::BLUE;
            ui.spacing_mut().slider_width = (ui.available_width() - 80.0).max(120.0);
            ui.spacing_mut().slider_rail_height = 6.0;
            ui.add(egui::Slider::new(value, range).trailing_fill(true));
        });
        if state.prefs.mode == GeneratorMode::Password {
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (text, on) in [
                    ("A-Z", &mut state.prefs.uppercase),
                    ("a-z", &mut state.prefs.lowercase),
                    ("0-9", &mut state.prefs.numbers),
                    ("!@#", &mut state.prefs.symbols),
                ] {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        controls::switch(ui, on);
                        ui.label(theme::bold(text, 15.0).color(theme::INK));
                    });
                }
            });
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                controls::switch(ui, &mut state.prefs.avoid_lookalikes);
                ui.label(theme::bold(avoid_lookalikes(), 15.0).color(theme::INK));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};

    const SCREEN: Vec2 = egui::vec2(1028.0, 820.0);

    fn frame(ctx: &egui::Context, time: f64, state: &mut GeneratorScreen, events: Vec<egui::Event>) -> (GeneratorAction, testing::Painted) {
        let mut action = GeneratorAction::None;
        let p = testing::frame(ctx, time, SCREEN, events, |ui| action = draw(ui, state));
        (action, p)
    }

    #[test]
    fn the_screen_offers_three_modes() {
        let ctx = testing::ctx();
        let mut state = GeneratorScreen::new(GeneratorPrefs::default());
        let (_, p) = frame(&ctx, 0.0, &mut state, vec![]);
        for mode in modes() {
            assert!(p.text(mode).is_some(), "no {mode}");
        }
        assert_eq!(state.value().len(), 20, "the default password is 20 long");
    }

    #[test]
    fn pin_mode_shows_only_digits() {
        let ctx = testing::ctx();
        let mut state = GeneratorScreen::new(GeneratorPrefs::default());
        let (_, p) = frame(&ctx, 0.0, &mut state, vec![]);
        let pin = p.texts.iter().find(|(t, _, _)| t == "PIN").expect("the PIN segment").2.center();
        let (action, _) = frame(&ctx, 0.1, &mut state, click_at(pin));
        assert_eq!(action, GeneratorAction::PrefsChanged);
        assert_eq!(state.prefs.mode, GeneratorMode::Pin);
        assert_eq!(state.value().len(), 6);
        assert!(state.value().chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn changing_an_option_persists_it() {
        let ctx = testing::ctx();
        let mut state = GeneratorScreen::new(GeneratorPrefs::default());
        let (_, p) = frame(&ctx, 0.0, &mut state, vec![]);
        let phrase = p.texts.iter().find(|(t, _, _)| t == "Passphrase").unwrap().2.center();
        let (action, _) = frame(&ctx, 0.1, &mut state, click_at(phrase));
        assert_eq!(action, GeneratorAction::PrefsChanged, "the window is told to write the options");
        let (action, _) = frame(&ctx, 0.2, &mut state, vec![]);
        assert_eq!(action, GeneratorAction::None, "an idle frame writes nothing");
    }
}

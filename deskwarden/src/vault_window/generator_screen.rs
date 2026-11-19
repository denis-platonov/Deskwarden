//! Spec C §1: the Generator as its own screen -- a password, a passphrase or
//! a PIN, with its options remembered between runs.

use eframe::egui::{self, Color32, CornerRadius, Vec2};
use zeroize::Zeroizing;

use crate::generator_mode::{generator_strength, GeneratorMode};
use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::settings::GeneratorPrefs;
use crate::theme;

pub const TITLE: &str = "Generator";
pub const SUB: &str = "A strong password, passphrase or PIN, made on this PC.";
pub const MODES: [&str; 3] = ["Password", "Passphrase", "PIN"];
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied";
pub const REGENERATE: &str = "New";
pub const AVOID_LOOKALIKES: &str = "Avoid look-alikes";
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
}

impl GeneratorScreen {
    pub fn new(prefs: GeneratorPrefs) -> Self {
        Self { prefs: prefs.clamped(), value: Zeroizing::new(String::new()), error: None, stale: true }
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
                self.error = Some(format!("Couldn't make one: {e}"));
            }
        }
        self.stale = false;
    }

    /// The password alphabet's size, for the strength line.
    fn pool(&self) -> usize {
        let p = &self.prefs;
        [(p.uppercase, 26), (p.lowercase, 26), (p.numbers, 10), (p.symbols, 32)]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, n)| *n)
            .sum()
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
                    egui::RichText::new(TITLE)
                        .font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                        .color(theme::INK),
                );
                ui.label(egui::RichText::new(SUB).size(16.0).color(Color32::from_rgb(0x7d, 0x79, 0x79)));
                ui.add_space(20.0);
                value_card(ui, state);
                ui.add_space(16.0);
                options_card(ui, state);
            });
        });
    });
    if state.prefs != before {
        state.prefs = state.prefs.clone().clamped();
        state.regenerate();
        GeneratorAction::PrefsChanged
    } else {
        GeneratorAction::None
    }
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(18))
        .shadow(crate::kit::tokens::card_shadow())
        .inner_margin(22)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn value_card(ui: &mut egui::Ui, state: &mut GeneratorScreen) {
    card(ui, |ui| {
        let size = ButtonSize { height: 48.0, pad_x: 20.0, radius: 14, font_px: 16.0 };
        let buttons = controls::button_width(ui.ctx(), ButtonKind::Primary, size, COPY, false)
            + controls::button_width(ui.ctx(), ButtonKind::Outline, size, REGENERATE, false)
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
            let copy = controls::button(ui, ButtonKind::Primary, size, COPY, None);
            let _ = ui.interact(copy.rect, copy_id(), egui::Sense::hover());
            if copy.clicked() && !state.value.is_empty() {
                crate::clipboard::copy_secret(&state.value);
                crate::kit::toast::show(ui.ctx(), COPIED);
            }
            let again = controls::button(ui, ButtonKind::Outline, size, REGENERATE, None);
            let _ = ui.interact(again.rect, regenerate_id(), egui::Sense::hover());
            if again.clicked() {
                state.regenerate();
            }
        });
        ui.add_space(10.0);
        let (bits, label) = generator_strength(state.prefs.mode, state.size(), state.pool());
        let ink = match label {
            "Weak" => theme::ERROR,
            "Fair" => Color32::from_rgb(0x8a, 0x5a, 0x06),
            _ => Color32::from_rgb(0x17, 0x67, 0x3a),
        };
        ui.horizontal(|ui| {
            ui.label(theme::bold(label, 15.0).color(ink));
            ui.label(egui::RichText::new(format!("about {} bits", bits.round() as u32)).size(14.0).color(MUTED));
        });
    });
}

fn options_card(ui: &mut egui::Ui, state: &mut GeneratorScreen) {
    card(ui, |ui| {
        let selected = mode_index(state.prefs.mode);
        if let Some(picked) = controls::segmented(ui, &MODES, selected, controls::SEG_GENERATOR) {
            state.prefs.mode = [GeneratorMode::Password, GeneratorMode::Passphrase, GeneratorMode::Pin][picked];
        }
        ui.add_space(18.0);
        let (label, value, range) = match state.prefs.mode {
            GeneratorMode::Password => ("Length", &mut state.prefs.length, 8..=64),
            GeneratorMode::Passphrase => ("Words", &mut state.prefs.words, 3..=10),
            GeneratorMode::Pin => ("Digits", &mut state.prefs.pin_length, 4..=12),
        };
        ui.horizontal(|ui| {
            ui.label(theme::bold(label, 15.0).color(theme::INK));
            ui.add_space(12.0);
            ui.spacing_mut().slider_width = (ui.available_width() - 80.0).max(120.0);
            ui.add(egui::Slider::new(value, range));
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
                    ui.horizontal(|ui| {
                        controls::switch(ui, on);
                        ui.label(theme::bold(text, 15.0).color(theme::INK));
                    });
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                controls::switch(ui, &mut state.prefs.avoid_lookalikes);
                ui.label(theme::bold(AVOID_LOOKALIKES, 15.0).color(theme::INK));
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
        for mode in MODES {
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

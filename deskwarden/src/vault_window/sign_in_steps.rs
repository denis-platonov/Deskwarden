//! Spec B 1.4 D: the editor's "Sign-in steps" card -- the keystrokes a fill
//! presses, as chips, with three presets and a test that plays them with fake
//! data.
//!
//! The steps are a view onto the item's stored `key_sequence` template
//! (decision 4): each step is one token run, so a sequence made only of these
//! seven steps is drawn as chips and anything else (literals, other waits,
//! custom fields) keeps 0.15's template editor, which stays one click away.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::kit::icon::{self, glyph};
use crate::theme;

/// One step a fill can press (spec B 1.4 D's `STEP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    User,
    Pw,
    Totp,
    Tab,
    Stab,
    Enter,
    Wait,
}

/// The add menu's order.
pub const ALL: [Step; 7] = [Step::User, Step::Pw, Step::Totp, Step::Tab, Step::Stab, Step::Enter, Step::Wait];

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::User => "Type username",
            Step::Pw => "Type password",
            Step::Totp => "Type 2FA code",
            Step::Tab => "Press Tab",
            Step::Stab => "Press Shift+Tab",
            Step::Enter => "Press Enter",
            Step::Wait => "Wait \u{bd} second",
        }
    }

    /// The `key_sequence` spelling of this step.
    pub fn token(self) -> &'static str {
        match self {
            Step::User => "{USERNAME}",
            Step::Pw => "{PASSWORD}",
            Step::Totp => "{TOTP}",
            Step::Tab => "{TAB}",
            Step::Stab => "+{TAB}",
            Step::Enter => "{ENTER}",
            Step::Wait => "{DELAY 500}",
        }
    }

    /// The password: drawn in the red tones, and never played.
    pub fn secret(self) -> bool {
        self == Step::Pw
    }

    /// What the test pane shows for this step -- fake data, never the item's.
    pub fn test_output(self) -> &'static str {
        match self {
            Step::User => "sample-user",
            Step::Pw => "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}",
            Step::Totp => "123456",
            Step::Tab => " \u{21e5} ",
            Step::Stab => " \u{21e4} ",
            Step::Enter => " \u{23ce}",
            Step::Wait => " \u{2026} ",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Step::User => glyph::PERSON,
            Step::Pw => glyph::LOCK,
            Step::Totp => glyph::SHIELD_CHECK,
            Step::Tab => glyph::TAB_KEY,
            Step::Stab => glyph::SHIFT_TAB,
            Step::Enter => glyph::RETURN,
            Step::Wait => glyph::HOURGLASS,
        }
    }
}

/// The presets, in the segmented control's order.
pub const PRESETS: [(&str, &[Step]); 3] = [
    ("Standard", &[Step::User, Step::Tab, Step::Pw, Step::Enter]),
    ("Password first", &[Step::Pw, Step::Stab, Step::User, Step::Enter]),
    ("With 2FA", &[Step::User, Step::Tab, Step::Pw, Step::Enter, Step::Wait, Step::Totp, Step::Enter]),
];

pub const TITLE_TIP: &str =
    "What Deskwarden presses to sign you in. Most apps work with Standard \u{2014} only change it if a sign-in goes wrong.";
pub const TEST_BUTTON: &str = "Test it";
pub const TESTING_BUTTON: &str = "Testing\u{2026}";
pub const TEST_EMPTY: &str = "Fake data appears here";
pub const TEST_DONE_TOAST: &str = "Test finished \u{2014} every step worked";
pub const TEST_TIP: &str = "Plays the steps with fake data, so you can check them without using your real password.";
pub const ONLY_TIP: &str = "Your password is never typed into any other window \u{2014} not a chat, not a search box \u{2014} even if you press the shortcut there.";
/// Spec B 1.4 D: one step every 620 ms.
pub const TEST_STEP_SECS: f64 = 0.62;

pub fn only_type_into(app: &str) -> String {
    format!("Only type into {}", if app.is_empty() { "the matched app" } else { app })
}

/// The steps a stored template spells, or `None` when it holds anything the
/// chips cannot say. The empty template is the app's default sequence.
pub fn parse(sequence: &str) -> Option<Vec<Step>> {
    let mut rest = if sequence.is_empty() { crate::key_sequence::DEFAULT_SEQUENCE } else { sequence };
    let mut steps = Vec::new();
    'next: while !rest.is_empty() {
        for step in ALL {
            if let Some(after) = rest.strip_prefix(step.token()) {
                steps.push(step);
                rest = after;
                continue 'next;
            }
        }
        return None;
    }
    Some(steps)
}

/// The template these steps store. No steps store the empty string, which
/// is the default sequence again (see `detail_edit::store`).
pub fn render(steps: &[Step]) -> String {
    steps.iter().map(|s| s.token()).collect()
}

/// The preset whose steps these are exactly, if any.
pub fn active_preset(steps: &[Step]) -> Option<usize> {
    PRESETS.iter().position(|(_, preset)| *preset == steps)
}

/// What the test shows `elapsed` seconds after it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFrame {
    pub text: String,
    /// The chip running now.
    pub live: Option<usize>,
    pub finished: bool,
}

pub fn test_frame(steps: &[Step], elapsed: f64) -> TestFrame {
    let ticks = (elapsed / TEST_STEP_SECS).floor().max(0.0) as usize;
    let shown = ticks.min(steps.len());
    TestFrame {
        text: steps[..shown].iter().map(|s| s.test_output()).collect(),
        live: (ticks >= 1 && ticks <= steps.len()).then(|| ticks - 1),
        finished: ticks > steps.len(),
    }
}

/// A test run, kept in the card's memory.
#[derive(Clone, Debug)]
struct Run {
    started: f64,
    steps: Vec<Step>,
    done: bool,
}

pub fn chip_remove_id(index: usize) -> egui::Id {
    egui::Id::new(("sign-in-step-remove", index))
}

pub fn add_button_id() -> egui::Id {
    egui::Id::new("sign-in-step-add")
}

pub fn menu_row_id(step: Step) -> egui::Id {
    egui::Id::new(("sign-in-step-menu", step))
}

pub fn test_button_id() -> egui::Id {
    egui::Id::new("sign-in-steps-test")
}

fn run_id() -> egui::Id {
    egui::Id::new("sign-in-steps-run")
}

fn menu_id() -> egui::Id {
    egui::Id::new("sign-in-steps-menu-open")
}

const SECRET_EDGE: Color32 = Color32::from_rgb(0xf0, 0xcf, 0xca);
const CHIP_EDGE: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);
const ARROW: Color32 = Color32::from_rgb(0xc9, 0xc6, 0xc2);
const TEST_WELL: Color32 = Color32::from_rgb(0xf7, 0xf6, 0xf5);
const FAINT: Color32 = Color32::from_rgb(0x9b, 0x97, 0x97);
const MENU_HOVER: Color32 = Color32::from_rgb(0xf3, 0xf2, 0xf0);
pub(crate) const CHIP_HEIGHT: f32 = 46.0;

/// A chip's colours: (border, fill, ink).
pub(crate) fn chip_colours(step: Step, live: bool) -> (Color32, Color32, Color32) {
    if live {
        (theme::BLUE, theme::BLUE_WASH, theme::INK)
    } else if step.secret() {
        (SECRET_EDGE, theme::DANGER_WASH, theme::DANGER_INK)
    } else {
        (CHIP_EDGE, theme::CARD, theme::INK)
    }
}

/// The card's body. `sequence` is the item's stored template; returns true
/// when the person asked for 0.15's template editor.
pub fn draw(ui: &mut egui::Ui, sequence: &mut String, app: &str) -> bool {
    let Some(mut steps) = parse(sequence) else {
        return false;
    };
    let now = ui.input(|i| i.time);
    let mut wants_editor = false;

    // Presets.
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("?").color(FAINT)).on_hover_text(TITLE_TIP);
        let labels: Vec<&str> = PRESETS.iter().map(|(name, _)| *name).collect();
        let selected = active_preset(&steps).unwrap_or(usize::MAX);
        if let Some(picked) = crate::kit::controls::segmented(ui, &labels, selected, crate::kit::controls::SEG_SETTINGS) {
            steps = PRESETS[picked].1.to_vec();
            ui.data_mut(|d| d.remove::<Run>(run_id()));
        }
    });
    ui.add_space(12.0);

    let run: Option<Run> = ui.data(|d| d.get_temp(run_id()));
    let frame = run.as_ref().map(|r| test_frame(&r.steps, now - r.started));
    let live = frame.as_ref().and_then(|f| if run.as_ref().is_some_and(|r| r.done) { None } else { f.live });

    // Chips, arrows, the dashed add.
    let mut remove = None;
    let mut add_rect = Rect::NOTHING;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        for (i, step) in steps.iter().enumerate() {
            if chip(ui, *step, i, live == Some(i)) {
                remove = Some(i);
            }
            if i + 1 < steps.len() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(16.0, CHIP_HEIGHT), Sense::hover());
                icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(16.0)), glyph::ARROW_RIGHT, 2.4, ARROW);
            }
        }
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(CHIP_HEIGHT), Sense::click());
        // Its stable id, for tests and later frames (an id'd click on the
        // same rect as an allocated widget never sees the pointer).
        let _ = ui.interact(rect, add_button_id(), Sense::hover());
        let ink = if response.hovered() { theme::BLUE } else { ARROW };
        dashed_box(ui.painter(), rect, ink);
        icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(22.0)), glyph::PLUS, 2.0, if response.hovered() { theme::BLUE } else { theme::TEXT_MUTED });
        if response.clicked() {
            let open = ui.data(|d| d.get_temp::<bool>(menu_id())).unwrap_or(false);
            ui.data_mut(|d| d.insert_temp(menu_id(), !open));
        }
        add_rect = rect;
    });
    if let Some(i) = remove {
        steps.remove(i);
        if let Some(mut r) = ui.data(|d| d.get_temp::<Run>(run_id())) {
            r.done = true;
            ui.data_mut(|d| d.insert_temp(run_id(), r));
        }
    }

    // The add menu.
    if ui.data(|d| d.get_temp::<bool>(menu_id())).unwrap_or(false) {
        let picked = egui::Area::new(menu_id().with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(add_rect.left_bottom() + Vec2::new(0.0, 8.0))
            .show(ui.ctx(), |ui| {
                let mut picked = None;
                egui::Frame::new()
                    .fill(theme::CARD)
                    .corner_radius(CornerRadius::same(16))
                    .shadow(crate::kit::tokens::card_shadow())
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_width(240.0);
                        for step in ALL {
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(228.0, 42.0), Sense::hover());
                            let row = ui.interact(rect, menu_row_id(step), Sense::click());
                            if row.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(11), MENU_HOVER);
                            }
                            let ink = if step.secret() { theme::DANGER_INK } else { theme::INK };
                            let glyph_rect = Rect::from_center_size(rect.left_center() + Vec2::new(20.0, 0.0), Vec2::splat(16.0));
                            icon::stroke(ui.painter(), glyph_rect, step.icon(), 2.0, ink);
                            ui.painter().text(
                                rect.left_center() + Vec2::new(39.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                step.label(),
                                egui::FontId::new(15.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
                                ink,
                            );
                            if row.clicked() {
                                picked = Some(step);
                            }
                        }
                    });
                picked
            })
            .inner;
        if let Some(step) = picked {
            steps.push(step);
            ui.data_mut(|d| d.insert_temp(menu_id(), false));
        }
    }
    ui.add_space(14.0);

    // "Only type into ..." -- what an app binding already does, so the switch
    // is shown on and cannot be turned off.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let mut on = true;
        ui.add_enabled_ui(false, |ui| crate::kit::controls::switch(ui, &mut on));
        ui.label(theme::semibold(only_type_into(app), 15.0).color(theme::INK));
        ui.label(egui::RichText::new("?").color(FAINT)).on_hover_text(ONLY_TIP);
        if theme::action_link(ui, crate::vault_window::detail_edit::SEQUENCE_EDIT_BUTTON, 13.0).clicked() {
            wants_editor = true;
        }
    });
    ui.add_space(12.0);

    // The test box.
    let testing = run.as_ref().is_some_and(|r| !r.done);
    egui::Frame::new()
        .fill(TEST_WELL)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(12)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let label = if testing { TESTING_BUTTON } else { TEST_BUTTON };
                let size = crate::kit::controls::ButtonSize { height: 46.0, pad_x: 18.0, radius: 13, font_px: 15.0 };
                let button =
                    crate::kit::controls::button(ui, crate::kit::controls::ButtonKind::Primary, size, label, Some(glyph::PLAY));
                // A stable id over the button, so tests and later frames find it.
                let clicked = button.clicked() || ui.interact(button.rect, test_button_id(), Sense::click()).clicked();
                if clicked && !steps.is_empty() {
                    ui.data_mut(|d| d.insert_temp(run_id(), Run { started: now, steps: steps.clone(), done: false }));
                    ui.data_mut(|d| d.insert_temp(menu_id(), false));
                }
                let width = (ui.available_width() - 28.0).max(80.0);
                let (pane, _) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::hover());
                ui.painter().rect(pane, CornerRadius::same(12), theme::CARD, Stroke::new(1.0, CHIP_EDGE), StrokeKind::Inside);
                let shown = ui.data(|d| d.get_temp::<Run>(run_id())).map(|r| {
                    let f = test_frame(&r.steps, now - r.started);
                    if r.done { r.steps.iter().map(|s| s.test_output()).collect() } else { f.text }
                });
                let (text, ink) = match shown.as_deref() {
                    None | Some("") if !testing => (TEST_EMPTY.to_string(), FAINT),
                    other => (other.unwrap_or_default().to_string(), theme::INK),
                };
                let clip = ui.painter().with_clip_rect(pane.shrink2(Vec2::new(14.0, 0.0)));
                let at = clip.text(
                    pane.left_center() + Vec2::new(14.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    egui::FontId::new(15.0, egui::FontFamily::Monospace),
                    ink,
                );
                if testing && (now.fract() < 0.5) {
                    let caret = Rect::from_center_size(egui::pos2(at.right() + 3.0, pane.center().y), Vec2::new(2.0, 20.0));
                    clip.rect_filled(caret, 0.0, theme::INK);
                }
                ui.label(egui::RichText::new("?").color(FAINT)).on_hover_text(TEST_TIP);
            });
        });

    // Advance the run; toast once when it finishes.
    if let Some(mut r) = ui.data(|d| d.get_temp::<Run>(run_id())) {
        if !r.done {
            if test_frame(&r.steps, now - r.started).finished {
                r.done = true;
                crate::kit::toast::show(ui.ctx(), TEST_DONE_TOAST);
                ui.data_mut(|d| d.insert_temp(run_id(), r));
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
    }

    let stored = render(&steps);
    let unchanged = parse(sequence).is_some_and(|before| before == steps);
    if !unchanged {
        *sequence = stored;
    }
    wants_editor
}

/// One chip; true when its ✕ was clicked.
fn chip(ui: &mut egui::Ui, step: Step, index: usize, live: bool) -> bool {
    let font = egui::FontId::new(15.0, egui::FontFamily::Name(theme::BOLD.into()));
    let (border, fill, ink) = chip_colours(step, live);
    let galley = ui.painter().layout_no_wrap(step.label().to_string(), font, ink);
    let width = 14.0 + 17.0 + 9.0 + galley.size().x + 9.0 + 26.0 + 6.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CHIP_HEIGHT), Sense::hover());
    let drawn = if live { rect.expand(rect.height() * 0.035) } else { rect };
    if live {
        let lift = egui::Shadow { offset: [0, 8], blur: 20, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 64) };
        ui.painter().add(lift.as_shape(drawn, CornerRadius::same(13)));
    }
    ui.painter().rect(drawn, CornerRadius::same(13), fill, Stroke::new(2.0, border), StrokeKind::Inside);
    let glyph_rect = Rect::from_center_size(drawn.left_center() + Vec2::new(14.0 + 8.5, 0.0), Vec2::splat(17.0));
    icon::stroke(ui.painter(), glyph_rect, step.icon(), 2.0, ink);
    ui.painter().galley(drawn.left_center() + Vec2::new(14.0 + 17.0 + 9.0, -galley.size().y / 2.0), galley, ink);
    let cross = Rect::from_center_size(drawn.right_center() - Vec2::new(6.0 + 13.0, 0.0), Vec2::splat(26.0));
    let response = ui.interact(cross, chip_remove_id(index), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(cross, CornerRadius::same(8), Color32::from_rgba_unmultiplied(32, 30, 29, 18));
    }
    icon::stroke(ui.painter(), Rect::from_center_size(cross.center(), Vec2::splat(12.0)), glyph::CLOSE, 2.0, if response.hovered() { theme::INK } else { FAINT });
    response.on_hover_text("Remove step").clicked()
}

/// A 2pt dashed rounded box (the add button's outline).
fn dashed_box(painter: &egui::Painter, rect: Rect, ink: Color32) {
    let r = rect.shrink(1.0);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(egui::Shape::dashed_line(&corners, Stroke::new(2.0, ink), 5.0, 4.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::kit::testing::{self, click_at, Painted};

    const SCREEN: egui::Vec2 = egui::vec2(900.0, 700.0);

    fn paint(ctx: &egui::Context, time: f64, sequence: &mut String, events: Vec<egui::Event>) -> Painted {
        testing::frame(ctx, time, SCREEN, events, |ui| {
            let _ = draw(ui, sequence, "Ledgerline");
        })
    }

    fn rect_of(ctx: &egui::Context, id: egui::Id) -> Rect {
        ctx.read_response(id).unwrap_or_else(|| panic!("nothing drawn under {id:?}")).rect
    }

    fn ink_of(p: &Painted, label: &str) -> Color32 {
        p.texts.iter().find(|(t, _, _)| t == label).unwrap_or_else(|| panic!("no {label}")).1
    }

    /// The presets' segments: the lit one is set in ink, the others are not.
    fn lit(p: &Painted) -> Vec<&'static str> {
        PRESETS.iter().map(|(name, _)| *name).filter(|name| ink_of(p, name) == crate::kit::tokens::Palette::LIGHT.ink).collect()
    }

    #[test]
    fn a_preset_replaces_the_steps_and_lights_its_segment() {
        let ctx = testing::ctx();
        let mut sequence = render(PRESETS[0].1);
        let p = paint(&ctx, 0.0, &mut sequence, vec![]);
        assert_eq!(lit(&p), vec!["Standard"]);
        let at = p.texts.iter().find(|(t, _, _)| t == "Password first").unwrap().2.center();
        let _ = paint(&ctx, 0.1, &mut sequence, click_at(at));
        assert_eq!(sequence, "{PASSWORD}+{TAB}{USERNAME}{ENTER}");
        let p = paint(&ctx, 0.2, &mut sequence, vec![]);
        assert_eq!(lit(&p), vec!["Password first"]);
    }

    #[test]
    fn a_custom_sequence_lights_no_preset() {
        let ctx = testing::ctx();
        let mut sequence = "{USERNAME}{TAB}{DELAY 500}{PASSWORD}{ENTER}".to_string();
        let p = paint(&ctx, 0.0, &mut sequence, vec![]);
        assert!(lit(&p).is_empty(), "{:?}", lit(&p));
        assert_eq!(sequence, "{USERNAME}{TAB}{DELAY 500}{PASSWORD}{ENTER}", "drawing changes nothing");
    }

    #[test]
    fn adding_and_removing_steps_edits_the_sequence() {
        let ctx = testing::ctx();
        let mut sequence = render(PRESETS[0].1);
        let _ = paint(&ctx, 0.0, &mut sequence, vec![]);
        let enter = rect_of(&ctx, chip_remove_id(3));
        let _ = paint(&ctx, 0.1, &mut sequence, click_at(enter.center()));
        assert_eq!(sequence, "{USERNAME}{TAB}{PASSWORD}");
        // The pointer leaves, taking the cross's tooltip with it.
        let _ = paint(&ctx, 0.2, &mut sequence, vec![egui::Event::PointerGone]);
        // `read_response` answers from the pass before last: one more idle
        // frame so the add button's rect is the three-chip one.
        let _ = paint(&ctx, 0.25, &mut sequence, vec![]);
        let add = rect_of(&ctx, add_button_id());
        let _ = paint(&ctx, 0.3, &mut sequence, click_at(add.center()));
        // A new area is laid out unseen on its first pass.
        let _ = paint(&ctx, 0.4, &mut sequence, vec![]);
        let _ = paint(&ctx, 0.45, &mut sequence, vec![]);
        let totp = rect_of(&ctx, menu_row_id(Step::Totp));
        let _ = paint(&ctx, 0.5, &mut sequence, click_at(totp.center()));
        assert_eq!(sequence, "{USERNAME}{TAB}{PASSWORD}{TOTP}");
    }

    #[test]
    fn test_plays_fake_data_one_step_every_620ms_and_never_the_password() {
        let steps = PRESETS[0].1;
        assert_eq!(test_frame(steps, 0.0), TestFrame { text: String::new(), live: None, finished: false });
        assert_eq!(test_frame(steps, 0.63).text, "sample-user");
        assert_eq!(test_frame(steps, 0.63).live, Some(0));
        let all = test_frame(steps, 1.25);
        assert_eq!((all.text.as_str(), all.live), ("sample-user \u{21e5} ", Some(1)));
        assert!(test_frame(steps, 0.62 * 5.0 + 0.01).finished);

        let ctx = testing::ctx();
        let mut sequence = render(steps);
        let _ = paint(&ctx, 0.0, &mut sequence, vec![]);
        let test = rect_of(&ctx, test_button_id());
        let p = paint(&ctx, 0.1, &mut sequence, click_at(test.center()));
        assert!(p.texts.iter().any(|(t, _, _)| t == TESTING_BUTTON) || paint(&ctx, 0.11, &mut sequence, vec![]).texts.iter().any(|(t, _, _)| t == TESTING_BUTTON));
        let p = paint(&ctx, 0.1 + 0.62 * 3.0 + 0.01, &mut sequence, vec![]);
        let pane = p.texts.iter().find(|(t, _, _)| t.starts_with("sample-user")).expect("the test pane").0.clone();
        assert!(pane.contains("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"), "{pane}");
        let _ = paint(&ctx, 0.1 + 0.62 * 5.0 + 0.05, &mut sequence, vec![]);
        assert_eq!(crate::kit::toast::current(&ctx).as_deref(), Some(TEST_DONE_TOAST));
    }

    #[test]
    fn the_password_step_wears_the_secret_colours() {
        assert_eq!(chip_colours(Step::Pw, false), (SECRET_EDGE, theme::DANGER_WASH, theme::DANGER_INK));
        assert_eq!(chip_colours(Step::User, false).1, theme::CARD);
        let ctx = testing::ctx();
        let mut sequence = render(PRESETS[0].1);
        let p = paint(&ctx, 0.0, &mut sequence, vec![]);
        assert!(p.rects.iter().any(|r| r.fill == theme::DANGER_WASH && r.stroke.color == SECRET_EDGE && r.rect.height() == CHIP_HEIGHT));
        assert_eq!(ink_of(&p, Step::Pw.label()), theme::DANGER_INK);
    }

    #[test]
    fn presets_round_trip_through_the_template() {
        for (_, steps) in PRESETS {
            assert_eq!(parse(&render(steps)).as_deref(), Some(steps));
        }
        assert_eq!(render(PRESETS[1].1), "{PASSWORD}+{TAB}{USERNAME}{ENTER}");
        assert_eq!(parse("{USERNAME}x"), None, "a literal is the template editor's");
        assert_eq!(parse("").unwrap(), vec![Step::User, Step::Tab, Step::Pw], "the default sequence");
    }
}

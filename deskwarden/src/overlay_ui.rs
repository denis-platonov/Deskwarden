use crate::app::FillChoice;
use crate::theme;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set for the duration of a `show_prompt_overlay` call.
///
/// The normal single-instance flow can't call this re-entrantly (it's a
/// blocking call on the one main thread, which can't process another
/// foreground event until this one returns) -- but two Deskwarden processes
/// running at once (observed live: an old dev build left running alongside a
/// freshly relaunched one) both watch the same foreground events and would
/// each independently open their own overlay for the same match, stacking
/// two overlay windows. This guard can't stop a *second process*'s window
/// from opening, but it does stop this process from ever contributing a
/// second one, and turns any single-process re-entrancy this analysis missed
/// into a harmless no-op instead of a stuck duplicate window.
static OVERLAY_OPEN: AtomicBool = AtomicBool::new(false);

/// What the overlay shows about the matched vault item: enough for the user
/// to recognize *which* credentials are about to be filled (design 2a shows
/// the username with the item name under it), without ever putting the
/// password itself on screen.
pub struct OverlayMatch {
    pub item_name: String,
    pub username: Option<String>,
}

/// The overlay window's inner width, in points. Fixed: the card is a
/// frameless, always-on-top window with no title bar, so the user cannot
/// resize it and nothing inside it may depend on a width it does not have.
pub const OVERLAY_WIDTH: f32 = 396.0;

/// Vertical pitch of ONE choice row: the painted row tile plus the gap that
/// separates it from the next one.
///
/// Measured, not chosen. `a_row_occupies_exactly_one_row_height` renders real
/// cards at one and two rows and asserts the difference between the two
/// cards' painted content is exactly this — so a font, a padding or an avatar
/// size changing under us fails a test here instead of silently pushing the
/// last row out of a window nobody can scroll.
pub const ROW_HEIGHT: f32 = 50.0;

/// Everything in the card that is NOT a choice row: the outer margins, the
/// card stroke, the header strip, the two hairlines, the row container's own
/// padding and the footer strip.
///
/// Derived rather than measured directly, from the one number this module is
/// not allowed to change: the overlay has always been 164.0 points tall with
/// one row, so `CHROME_HEIGHT == 164.0 - ROW_HEIGHT`.
/// `a_one_row_card_is_the_size_the_overlay_has_always_been` pins that.
pub const CHROME_HEIGHT: f32 = 114.0;

/// The overlay window's inner height for a card showing `rows` choice rows.
///
/// Pure arithmetic — no egui, no context, no fonts — because
/// `app::overlay_position` has to know how tall the window will be in order
/// to clamp it onto the monitor's work area *before* the window exists to
/// measure.
///
/// `rows.max(1)` because a card with no rows is not a shorter card: the
/// overlay always paints at least one row (with no choices it paints the
/// matched-credential row it has always painted), and a zero-row height would
/// clip that row's bottom off a window the user cannot scroll.
pub fn overlay_height(rows: usize) -> f32 {
    CHROME_HEIGHT + ROW_HEIGHT * rows.max(1) as f32
}

/// Opens the autofill overlay for `app_name`: a small, frameless,
/// always-on-top card (design 2a — "no chrome") with the Deskwarden header,
/// the matched credential row, and a keyboard-hint footer.
///
/// Returns `Some(choice)` — **which** row the user picked (clicked, or the
/// first row if they pressed Enter) — and `None` if they dismissed it (the
/// header's ✕, Esc, or closing the window).
///
/// `choices` are the rows to offer, in order; the **first** is the primary,
/// the one Enter takes and the one drawn in the selected treatment. An empty
/// slice paints the single matched-credential row the overlay has always
/// painted and answers [`FillChoice::Saved`] for it.
///
/// `matched` is `None` when the item couldn't be read back from the vault at
/// prompt time; the overlay still shows, it just can't name the credentials.
///
/// `anchor` is the top-left corner (screen pixels) to open the window at --
/// computed by the caller (`app::overlay_position`) from where the matched
/// field actually is, so the overlay reads as "next to the field" rather
/// than wherever the OS defaults a new window to. `None` falls back to
/// whatever the OS picks.
pub fn show_prompt_overlay(
    app_name: &str,
    matched: Option<&OverlayMatch>,
    anchor: Option<(f32, f32)>,
    choices: &[FillChoice],
) -> Option<FillChoice> {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "autofill overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return None;
    }

    let app_name = app_name.to_string();
    let (item_name, username) = match matched {
        Some(m) => (m.item_name.clone(), m.username.clone()),
        None => (String::new(), None),
    };

    // Same Rc<RefCell<_>> pattern as picker_ui::run_picker: the update
    // closure/app is 'static and must move-capture its state, so a plain
    // local bool can't be read back after the blocking call returns. A clone
    // of the Rc is moved in; the original is read here once the blocking
    // call returns (safe: same thread, no cross-thread sharing).
    let chosen: Rc<RefCell<Option<FillChoice>>> = Rc::new(RefCell::new(None));
    let choices = choices.to_vec();

    let mut viewport = egui::ViewportBuilder::default()
        // Sized for the rows it was actually given, not for one: a window
        // built for one row that paints four clips the last three off a
        // frameless card the user cannot scroll. `overlay_height` floors at
        // one row, so the empty and one-choice cases are still 164.0.
        .with_inner_size([OVERLAY_WIDTH, overlay_height(choices.len())])
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_icon(theme::window_icon());
    if let Some((x, y)) = anchor {
        viewport = viewport.with_position([x, y]);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let app = OverlayApp {
        app_name,
        item_name,
        username,
        choices,
        chosen: chosen.clone(),
    };

    // `run_native` rather than `run_simple_native`, because the frameless
    // card needs a transparent clear color behind its rounded corners, and
    // only a full `eframe::App` impl can override `clear_color`.
    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    OVERLAY_OPEN.store(false, Ordering::SeqCst);

    let answer = chosen.borrow().clone();
    answer
}

struct OverlayApp {
    app_name: String,
    item_name: String,
    username: Option<String>,
    choices: Vec<FillChoice>,
    chosen: Rc<RefCell<Option<FillChoice>>>,
}

/// The row Enter takes: the **primary**, which is the first.
///
/// A free function rather than an expression inside `ui`, because `ui` needs a
/// real egui context and nothing in the test suite may open a window — so the
/// keyboard's half of "which choice did the user pick" would otherwise be the
/// one half no test could reach. With no choices at all the overlay is the
/// card it has always been, whose one row is the item's saved sequence.
fn primary_choice(choices: &[FillChoice]) -> FillChoice {
    choices.first().cloned().unwrap_or(FillChoice::Saved)
}

impl eframe::App for OverlayApp {
    // Transparent behind the card: without this the window would clear to
    // the theme's opaque panel fill and the rounded corners would sit in a
    // visible rectangle.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // The overlay is keyboard-first (design 2a's footer: "↵ Fill · Esc
        // Dismiss"): Enter fills, Esc dismisses, no focus juggling needed.
        // The decision itself is `keyboard_action`, because this function
        // cannot be called by a test -- it needs an `eframe::Frame` and a real
        // window -- and "Esc dismisses" is not a claim that may live only
        // where nothing can check it.
        let keys = keyboard_action(
            ctx.input(|i| i.key_pressed(egui::Key::Enter)),
            ctx.input(|i| i.key_pressed(egui::Key::Escape)),
            &self.choices,
        );

        let card = draw_overlay_card_rows(
            ui,
            &self.app_name,
            &self.item_name,
            self.username.as_deref(),
            &self.choices,
        );

        let done = match if keys == OverlayAction::None { card } else { keys } {
            OverlayAction::Fill(choice) => {
                *self.chosen.borrow_mut() = Some(choice);
                true
            }
            OverlayAction::Dismiss => true,
            OverlayAction::None => false,
        };

        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// What the keyboard did to the card this frame.
///
/// **Esc answers [`OverlayAction::Dismiss`], never a fill.** It is the one
/// control the user reaches for to say "no" to a window that appeared over the
/// app they were typing in, and an Esc that answered `Some(primary)` would
/// type their password into whatever has focus. That is a behavioural claim
/// now rather than three statements inside `OverlayApp::ui`, which no test in
/// this crate may execute.
///
/// Enter outranks Esc when a frame somehow carries both, which is the
/// behaviour this had when the two were separate `if`s.
fn keyboard_action(enter: bool, escape: bool, choices: &[FillChoice]) -> OverlayAction {
    if enter {
        return OverlayAction::Fill(primary_choice(choices));
    }
    if escape {
        return OverlayAction::Dismiss;
    }
    OverlayAction::None
}

/// What the user did to the overlay card on this frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OverlayAction {
    /// Nothing yet; keep the overlay up.
    #[default]
    None,
    /// Fill — carrying **which** row was clicked, because "a fill happened"
    /// and "this is what to type" are different facts and the caller needs
    /// the second one.
    Fill(FillChoice),
    /// Close without filling (the header's ✕ was clicked).
    Dismiss,
}

/// Draws the overlay card itself — header (mark, wordmark, match count,
/// dismiss ✕), the matched credential row, and the keyboard-hint footer.
///
/// Public (rather than folded into `OverlayApp::update`) so the
/// `ui_preview` example can render the exact card the app ships, not a
/// re-implementation that could drift from it.
pub fn draw_overlay_card(
    ui: &mut egui::Ui,
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
) -> OverlayAction {
    draw_overlay_card_rows(ui, app_name, item_name, username, &[])
}

/// [`draw_overlay_card`] with an explicit list of choice rows.
///
/// An empty `choices` paints the single matched-credential row the overlay has
/// always painted — which is what `draw_overlay_card` (and therefore the whole
/// of production, until step 5 wires a choice list through) asks for. A
/// non-empty `choices` paints one row per choice, labelled by
/// [`FillChoice::label`].
///
/// The card is sized for `overlay_height(choices.len())`; because
/// `overlay_height` floors at one row, the empty case and the one-choice case
/// are the same height, and that height is the overlay's historical 164.0.
///
/// This is a separate function rather than an extra parameter on
/// `draw_overlay_card` only because `draw_overlay_card`'s signature is what
/// the `ui_preview` example calls, and this step owns exactly one file.
pub fn draw_overlay_card_rows(
    ui: &mut egui::Ui,
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
    choices: &[FillChoice],
) -> OverlayAction {
    let mut action = OverlayAction::None;

    let card = egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(36),
        })
        .outer_margin(Margin {
            left: 4,
            right: 12,
            top: 2,
            bottom: 20,
        });

    card.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        // Header: mark, wordmark, match count, and the dismiss ✕. The ✕ is
        // the only mouse-operable way out of a `with_decorations(false)`
        // window — there is no title bar to close, and the footer's "Esc
        // Dismiss" is a label, not a control. It matters more than it looks:
        // this window is raised in response to *another* app being
        // foregrounded, which is exactly the situation Windows' foreground
        // lock refuses keyboard focus for, so Esc is not guaranteed to reach
        // us at all.
        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                if theme::card_header_with_close(ui, "1 match") {
                    action = OverlayAction::Dismiss;
                }
            });
        theme::hairline(ui);

        // The choice rows. With no choices this is the single matched
        // credential row, in the selected treatment, exactly as before.
        egui::Frame::new()
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                let (primary, secondary) = row_text(app_name, item_name, username);
                if choices.is_empty() {
                    if credential_row(ui, &primary, &primary, &secondary, true) {
                        action = OverlayAction::Fill(FillChoice::Saved);
                    }
                } else {
                    for (index, choice) in choices.iter().enumerate() {
                        // Between rows only: with one row the card is byte-
                        // for-byte the geometry it has always had, which is
                        // what makes `overlay_height(1) == 164.0` true of the
                        // drawing and not just of the arithmetic.
                        if index > 0 {
                            ui.add_space(ROW_GAP);
                        }
                        // The avatar keeps showing WHO is being filled, not
                        // what is being typed -- the label already says that,
                        // and initials of "Username + Tab + Password" would
                        // name nothing.
                        // `choice.clone()`, not `choices[0]` and not the
                        // index: the row that was clicked is the row that
                        // answers, or four rows are four ways to do one thing.
                        if credential_row(ui, &primary, &choice.label(), &secondary, index == 0) {
                            action = OverlayAction::Fill(choice.clone());
                        }
                    }
                }
            });

        // Footer: keyboard hints on the tinted strip.
        theme::hairline(ui);
        egui::Frame::new()
            .fill(theme::CARD_TINT)
            .corner_radius(CornerRadius {
                sw: 9,
                se: 9,
                ..CornerRadius::ZERO
            })
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::footer_hints(ui, &[("Enter", "Fill"), ("Esc", "Dismiss")]);
            });
    });

    action
}

/// The two lines of the credential row: the recognizable identity on top
/// (username when known, item name otherwise) and context underneath.
fn row_text(app_name: &str, item_name: &str, username: Option<&str>) -> (String, String) {
    match (username, item_name.is_empty()) {
        (Some(u), false) => (u.to_string(), format!("{item_name} · fills {app_name}")),
        (Some(u), true) => (u.to_string(), format!("fills {app_name}")),
        (None, false) => (item_name.to_string(), format!("fills {app_name}")),
        (None, true) => ("Saved credentials".to_string(), format!("fills {app_name}")),
    }
}

/// Gap between two adjacent choice rows. Folded into [`ROW_HEIGHT`] rather
/// than into [`CHROME_HEIGHT`], because there are `n - 1` gaps for `n` rows
/// and `CHROME_HEIGHT` must not depend on `n`:
///
/// ```text
/// chrome + n*tile + (n-1)*gap  ==  (CHROME_HEIGHT) + n*(tile + gap)
/// ```
///
/// holds exactly when `CHROME_HEIGHT == chrome - gap`, i.e. when the gap
/// belongs to the row. That identity is what lets `overlay_height` be `a + b*n`
/// at all.
const ROW_GAP: f32 = 4.0;

/// One choice row. `selected` renders the emphasized treatment (blue wash,
/// blue avatar, Enter chip); otherwise the neutral one.
///
/// `avatar_of` is the text the initials tile is built from, which is NOT
/// `primary` once a row is labelled by what it will type rather than by whose
/// credentials it will type. Both treatments are the same height by
/// construction — the row's height is the taller of the 28pt avatar and the
/// two text lines, and neither the wash nor the chip is on that path — and
/// `both_row_treatments_are_the_same_height` holds it there.
///
/// Returns true when clicked.
fn credential_row(
    ui: &mut egui::Ui,
    avatar_of: &str,
    primary: &str,
    secondary: &str,
    selected: bool,
) -> bool {
    let fill = if selected {
        theme::BLUE_WASH
    } else {
        theme::CANVAS
    };
    let row = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(avatar_of), 28.0, selected);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(primary, 13.0).color(theme::INK));
                    ui.label(RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if selected {
                        theme::kbd_chip(ui, "Enter", true);
                    }
                });
            });
        });

    let response = row.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_leads_with_the_username_when_known() {
        let (primary, secondary) = row_text("ledgerline.exe", "Ledgerline", Some("a@b.com"));
        assert_eq!(primary, "a@b.com");
        assert_eq!(secondary, "Ledgerline · fills ledgerline.exe");
    }

    #[test]
    fn row_falls_back_to_the_item_name_without_a_username() {
        let (primary, secondary) = row_text("app.exe", "Postgres — Prod", None);
        assert_eq!(primary, "Postgres — Prod");
        assert_eq!(secondary, "fills app.exe");
    }

    #[test]
    fn row_still_says_something_when_the_item_could_not_be_read() {
        let (primary, secondary) = row_text("app.exe", "", None);
        assert_eq!(primary, "Saved credentials");
        assert_eq!(secondary, "fills app.exe");
    }
}

/// The overlay's geometry, measured off frames the card actually paints.
///
/// Why this exists at all: the overlay is a `with_decorations(false)`,
/// always-on-top window with a hardcoded inner size and NO scroll area
/// anywhere. A row that lands past the window's bottom edge is not merely
/// ugly -- there is no title bar to drag, no border to resize and nothing to
/// scroll, so the user cannot reach it by any means. Three separate times in
/// this codebase a text or layout change has pushed a control out of its
/// viewport. So the sizing lands *before* anything can produce more than one
/// row, and it lands with instruments that look at what was painted.
///
/// **Painted ink, not layout rects.** A galley's `rect` is where a run was
/// *placed*; text that laid out into zero rows, or was elided, or was drawn at
/// alpha 0, all have a perfectly healthy galley rect. So [`ink`] reads
/// `RowVisuals::mesh_bounds` (the tessellator's own bounds, whitespace
/// excluded) for text and `Shape::visual_bounding_rect` (which expands a rect
/// by its stroke and its blur) for everything else, and it resolves each
/// shape's actual painted colour so a fully transparent shape can be
/// discarded.
#[cfg(test)]
mod geometry_tests {
    use super::*;
    use crate::key_sequence::FieldRef;
    use eframe::egui::{epaint, Color32, Rect};

    // ---------------------------------------------------------------- ink

    /// One painted thing: where its ink actually landed, how opaque that ink
    /// is at its most opaque, and -- for text -- the characters that ink is.
    #[derive(Debug, Clone)]
    struct Ink {
        rect: Rect,
        alpha: u8,
        /// `Some` only for text; the glyphs of ONE laid-out row, in order.
        glyphs: Option<String>,
        /// `Some` only for `Shape::Rect`: its fill and corner radius, which is
        /// how a row tile is told apart from an avatar or a chip.
        tile: Option<(Color32, u8)>,
    }

    fn alpha_of(colors: &[Color32]) -> u8 {
        colors.iter().map(|c| c.a()).max().unwrap_or(0)
    }

    fn path_alpha(fill: Color32, stroke: &epaint::PathStroke) -> u8 {
        let stroke_alpha = match &stroke.color {
            epaint::ColorMode::Solid(c) => c.a(),
            // A UV-mapped stroke's colour is a function this test cannot
            // evaluate. Treat it as fully visible rather than as absent --
            // an instrument that assumes "invisible" is the failure mode
            // this whole module exists to avoid.
            epaint::ColorMode::UV(_) => 255,
        };
        if stroke.width <= 0.0 {
            fill.a()
        } else {
            fill.a().max(stroke_alpha)
        }
    }

    /// Walks one shape tree into [`Ink`].
    ///
    /// The match is EXHAUSTIVE -- no `_` arm. Every `epaint::Shape` variant is
    /// named, so a shape kind this walker has never seen is a compile error
    /// rather than a silently dropped row. The card is known to emit
    /// `Vec`, `Rect` (frames, avatars, chips, hairlines), `Text` (every label
    /// and both chips), `LineSegment` (the dismiss ✕, which is two strokes and
    /// not a glyph) and `Path` (the Deskwarden mark); the remaining arms are
    /// handled anyway so that they cannot become blind spots later.
    fn walk(shape: &egui::Shape, out: &mut Vec<Ink>) {
        let rect = shape.visual_bounding_rect();
        let mut plain = |alpha: u8| {
            out.push(Ink {
                rect,
                alpha,
                glyphs: None,
                tile: None,
            });
        };
        match shape {
            egui::Shape::Noop => {}
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            egui::Shape::Circle(c) => plain(alpha_of(&[c.fill, c.stroke.color])),
            egui::Shape::Ellipse(e) => plain(alpha_of(&[e.fill, e.stroke.color])),
            egui::Shape::LineSegment { stroke, .. } => plain(alpha_of(&[stroke.color])),
            egui::Shape::Path(p) => plain(path_alpha(p.fill, &p.stroke)),
            egui::Shape::QuadraticBezier(b) => plain(path_alpha(b.fill, &b.stroke)),
            egui::Shape::CubicBezier(b) => plain(path_alpha(b.fill, &b.stroke)),
            egui::Shape::Mesh(m) => {
                plain(m.vertices.iter().map(|v| v.color.a()).max().unwrap_or(0));
            }
            egui::Shape::Rect(r) => out.push(Ink {
                rect,
                alpha: alpha_of(&[r.fill, r.stroke.color]),
                glyphs: None,
                tile: Some((r.fill, r.corner_radius.nw)),
            }),
            egui::Shape::Text(t) => {
                // The tessellator draws NOTHING at all for these two, so
                // neither may be reported as painted ink.
                if t.galley.is_empty() || t.opacity_factor <= 0.0 {
                    return;
                }
                for placed in &t.galley.rows {
                    let row = &placed.row;
                    if row.visuals.mesh.is_empty() {
                        continue;
                    }
                    // Exactly the tessellator's own arithmetic:
                    // `row.visuals.mesh_bounds` translated by galley pos +
                    // row pos. Not `galley.rect`, which is where the run was
                    // placed rather than where its ink is.
                    let rect = row
                        .visuals
                        .mesh_bounds
                        .translate(t.pos.to_vec2() + placed.pos.to_vec2());
                    let alpha = row.visuals.mesh.vertices[row.visuals.glyph_vertex_range.clone()]
                        .iter()
                        .map(|v| {
                            let c = match t.override_text_color {
                                Some(o) => o,
                                None if v.color == Color32::PLACEHOLDER => t.fallback_color,
                                None => v.color,
                            };
                            if t.opacity_factor < 1.0 {
                                c.gamma_multiply(t.opacity_factor).a()
                            } else {
                                c.a()
                            }
                        })
                        .max()
                        .unwrap_or(0);
                    out.push(Ink {
                        rect,
                        alpha,
                        glyphs: Some(row.glyphs.iter().map(|g| g.chr).collect()),
                        tile: None,
                    });
                }
            }
            egui::Shape::Callback(_) => panic!(
                "the overlay card painted a backend callback; this walker cannot see inside \
                 one, and an instrument that cannot see a row is exactly what these tests \
                 exist to prevent"
            ),
        }
    }

    // -------------------------------------------------------------- frames

    /// A context with this app's fonts really installed. `theme::apply` takes
    /// effect at the START of the next frame, so the two warm-up frames are
    /// load-bearing, not defensive.
    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(sized(overlay_height(1)), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(sized(overlay_height(1)), |_ui| {});
        ctx
    }

    fn sized(height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(OVERLAY_WIDTH, height),
            )),
            ..Default::default()
        }
    }

    const APP: &str = "ledgerline.exe";
    const ITEM: &str = "Ledgerline";
    const USER: &str = "ada@example.com";

    /// The four rows `app::fill_choices` can produce, in its own order.
    fn four_choices() -> Vec<FillChoice> {
        vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ]
    }

    /// Paints a real card with `choices` into a window of `height`, and
    /// returns every painted thing with non-zero alpha.
    ///
    /// Zero-alpha shapes are DISCARDED here, at the source: a card whose rows
    /// are painted fully transparent must look to every assertion below
    /// exactly like a card with no rows, because that is what it looks like to
    /// the user.
    fn painted(choices: &[FillChoice], height: f32) -> Vec<Ink> {
        let ctx = styled_ctx();
        let output = ctx.run_ui(sized(height), |ui| {
            draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });
        let mut ink = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut ink);
        }
        ink.retain(|i| i.alpha > 0);
        ink
    }

    /// Clicks the point `at` on a card drawn with `choices`, and returns what
    /// the card answered on the frame the button came back up.
    ///
    /// Two frames on ONE context, not one: egui decides a click on the release,
    /// and a press and a release squeezed into a single frame is not the
    /// gesture the user makes. The first frame is the press (whose answer is
    /// asserted to be `None` -- a card that "filled" on mouse-down would fill
    /// the row the user dragged off), the second the release.
    fn click_on(choices: &[FillChoice], at: egui::Pos2) -> OverlayAction {
        let height = overlay_height(choices.len());
        let ctx = styled_ctx();
        // Warm-up frame: the row Frames must have been laid out once before
        // their rects can be interacted with.
        let _ = ctx.run_ui(sized(height), |ui| {
            draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });

        let press = |down: bool| egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: down,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..sized(height)
        };

        let mut on_press = OverlayAction::None;
        let _ = ctx.run_ui(press(true), |ui| {
            on_press = draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });
        assert_eq!(
            on_press,
            OverlayAction::None,
            "the card answered on mouse-DOWN; a press the user drags away from is not a choice"
        );

        let mut on_release = OverlayAction::None;
        let _ = ctx.run_ui(press(false), |ui| {
            on_release = draw_overlay_card_rows(ui, APP, ITEM, Some(USER), choices);
        });
        on_release
    }

    /// The clickable tiles of the choice rows, top to bottom.
    ///
    /// A row is a full-width, 8pt-radius filled rect in one of the two row
    /// treatments. Nothing else in the card is any of those things: the card
    /// itself is radius 10, the footer strip 9, the avatar 7 and 28pt wide,
    /// the keyboard chips 4, and the hairlines 0. The frame's own rect IS the
    /// rect its `Response` interacts on, so this is the clickable rect and not
    /// a proxy for it.
    fn row_tiles(ink: &[Ink]) -> Vec<Rect> {
        let mut tiles: Vec<Rect> = ink
            .iter()
            .filter(|i| {
                matches!(i.tile, Some((fill, 8)) if fill == theme::BLUE_WASH || fill == theme::CANVAS)
                    && i.rect.width() > OVERLAY_WIDTH / 2.0
            })
            .map(|i| i.rect)
            .collect();
        tiles.sort_by(|a, b| a.top().total_cmp(&b.top()));
        tiles
    }

    /// The ink of the one laid-out row whose glyphs are exactly `text`.
    ///
    /// Asserts there is EXACTLY one, which is what makes "the label is on
    /// screen" a claim about the label the caller named: a label that laid out
    /// into zero rows, or was elided to "Username + Tab + Pass…", has no
    /// match here and fails rather than quietly matching something else.
    fn glyph_run(ink: &[Ink], text: &str) -> Rect {
        let hits: Vec<&Ink> = ink
            .iter()
            .filter(|i| i.glyphs.as_deref() == Some(text))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one painted run reading {text:?}; found {} -- painted runs were {:?}",
            hits.len(),
            ink.iter()
                .filter_map(|i| i.glyphs.clone())
                .collect::<Vec<_>>()
        );
        hits[0].rect
    }

    fn window(height: f32) -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(OVERLAY_WIDTH, height))
    }

    /// True when `inner` is fully within `outer`, with a hair of tolerance for
    /// the pixel-rounding the tessellator does to text.
    fn fits(inner: Rect, outer: Rect) -> bool {
        inner.top() >= outer.top() - 0.5
            && inner.bottom() <= outer.bottom() + 0.5
            && inner.left() >= outer.left() - 0.5
            && inner.right() <= outer.right() + 0.5
    }

    // --------------------------------------------------------------- tests

    /// The number this whole step is not allowed to change.
    #[test]
    fn a_one_row_card_is_the_size_the_overlay_has_always_been() {
        assert_eq!(
            overlay_height(1),
            164.0,
            "the overlay has shipped at 396x164 since it was written; a one-row card \
             must still be exactly that"
        );
        // ... and the arithmetic that produces it is the arithmetic the
        // constants describe, not a coincidence of two other numbers.
        assert_eq!(CHROME_HEIGHT + ROW_HEIGHT, 164.0);
        // A card with nothing to offer is not a shorter card. `rows.max(1)`.
        assert_eq!(overlay_height(0), 164.0);
        // Each further row costs exactly one row.
        assert_eq!(overlay_height(2), 164.0 + ROW_HEIGHT);
        assert_eq!(overlay_height(3), 164.0 + 2.0 * ROW_HEIGHT);
        assert_eq!(overlay_height(4), 164.0 + 3.0 * ROW_HEIGHT);
    }

    /// The load-bearing test. For one, two, three and four rows: every row's
    /// clickable rect and every row's painted label are inside the window that
    /// `overlay_height` sized.
    ///
    /// Both counters are the point. A walker that finds no tiles, or a loop
    /// that never runs, satisfies every "is inside" assertion vacuously --
    /// which is precisely how this codebase's instruments have gone blind
    /// before.
    #[test]
    fn every_choice_row_is_inside_the_window_for_one_two_three_and_four_rows() {
        let mut iterations = 0;
        for n in 1..=4usize {
            iterations += 1;
            let choices = &four_choices()[..n];
            let height = overlay_height(n);
            let ink = painted(choices, height);
            let tiles = row_tiles(&ink);

            assert_eq!(
                tiles.len(),
                n,
                "a {n}-row card painted {} row tiles; a card that draws fewer rows than it \
                 was handed passes every geometry assertion below for free",
                tiles.len()
            );

            for (index, tile) in tiles.iter().enumerate() {
                assert!(
                    fits(*tile, window(height)),
                    "row {index} of {n} has its clickable rect at {tile:?}, outside the \
                     {height}pt window -- and this window has no title bar, no resize \
                     border and no scroll area, so the user could never reach it"
                );
            }

            for (index, choice) in choices.iter().enumerate() {
                let label = choice.label();
                // The glyphs painted equal the label asked for: not elided,
                // not wrapped into a second row, not laid out into none.
                let painted_label = glyph_run(&ink, &label);
                assert!(
                    fits(painted_label, window(height)),
                    "the label {label:?} (row {index} of {n}) has ink at {painted_label:?}, \
                     outside the {height}pt window"
                );
                assert!(
                    fits(painted_label, tiles[index].expand(1.0)),
                    "the label {label:?} paints at {painted_label:?}, which is not inside \
                     its own row tile {:?} -- the label and the clickable rect have come \
                     apart",
                    tiles[index]
                );
            }
        }
        assert_eq!(iterations, 4, "the loop must have covered 1, 2, 3 and 4 rows");
    }

    /// A four-row card is the tallest the overlay can be (`fill_choices`
    /// yields at most four by construction), so it is the one that pushes the
    /// footer and the dismiss control hardest.
    #[test]
    fn the_dismiss_control_and_the_footer_stay_inside_a_four_row_card() {
        let height = overlay_height(4);
        let ink = painted(&four_choices(), height);
        let win = window(height);

        // The ✕ is two 1.3pt line segments, not a glyph -- U+2715 resolves to
        // nothing in this app's face, so `close_glyph` draws it. It is the
        // ONLY mouse-operable way out of a decorationless window.
        // Each arm spans 7.0pt (`arm = 3.5` either side of centre) and its
        // visual bounding rect adds half its 1.3pt stroke on each end: 8.3
        // square, measured off the painted shapes and matched by nothing else
        // in the card (the mark's four paths are 5.7 x 6.9).
        let arms: Vec<&Ink> = ink
            .iter()
            .filter(|i| {
                i.tile.is_none()
                    && i.glyphs.is_none()
                    && (i.rect.width() - 8.3).abs() < 0.5
                    && (i.rect.height() - 8.3).abs() < 0.5
            })
            .collect();
        assert_eq!(
            arms.len(),
            2,
            "expected the dismiss ✕'s two strokes; found {}",
            arms.len()
        );
        for arm in &arms {
            assert!(
                fits(arm.rect, win),
                "a stroke of the dismiss ✕ is at {:?}, outside the {height}pt window",
                arm.rect
            );
        }

        // The footer's keyboard hints.
        for hint in ["Enter Fill", "Esc Dismiss"] {
            let rect = glyph_run(&ink, hint);
            assert!(
                fits(rect, win),
                "the footer hint {hint:?} paints at {rect:?}, outside the {height}pt window"
            );
        }

        // The footer strip itself sits BELOW the last row, not over it.
        let last_row = *row_tiles(&ink).last().expect("a four-row card has rows");
        let footer = glyph_run(&ink, "Enter Fill");
        assert!(
            footer.top() >= last_row.bottom(),
            "the footer's hints paint at {footer:?}, which overlaps the last row {last_row:?}"
        );
    }

    #[test]
    fn two_rows_never_overlap() {
        let height = overlay_height(4);
        let tiles = row_tiles(&painted(&four_choices(), height));
        assert_eq!(tiles.len(), 4);
        let mut pairs = 0;
        for pair in tiles.windows(2) {
            pairs += 1;
            let (a, b) = (pair[0], pair[1]);
            assert!(
                b.top() >= a.bottom(),
                "adjacent row tiles {a:?} and {b:?} intersect; one row is painting over \
                 the other"
            );
        }
        assert_eq!(pairs, 3, "four rows have exactly three adjacent pairs");
    }

    /// `ROW_HEIGHT` is measured, not chosen: it is the pitch two real rows are
    /// actually painted at.
    #[test]
    fn a_row_occupies_exactly_one_row_height() {
        let tiles = row_tiles(&painted(&four_choices(), overlay_height(4)));
        assert_eq!(tiles.len(), 4);
        let mut pitches = 0;
        for pair in tiles.windows(2) {
            pitches += 1;
            let pitch = pair[1].top() - pair[0].top();
            assert!(
                (pitch - ROW_HEIGHT).abs() < 0.01,
                "two rows are painted {pitch}pt apart, but `overlay_height` grows the \
                 window by {ROW_HEIGHT}pt per row"
            );
        }
        assert_eq!(pitches, 3);
        for tile in &tiles {
            assert!(
                (tile.height() - (ROW_HEIGHT - ROW_GAP)).abs() < 0.01,
                "a row tile is {}pt tall; ROW_HEIGHT - ROW_GAP is {}",
                tile.height(),
                ROW_HEIGHT - ROW_GAP
            );
        }
    }

    /// `overlay_height`'s `CHROME_HEIGHT` term does not depend on `n`, which
    /// is only true if the inter-row gaps are inside `ROW_HEIGHT`. Measured
    /// as: the slack left under the last painted thing is the same at one row
    /// as at four.
    #[test]
    fn the_chrome_costs_the_same_at_one_row_as_at_four() {
        let bottom_slack = |n: usize, choices: &[FillChoice]| {
            let height = overlay_height(n);
            let ink = painted(choices, height);
            let footer = glyph_run(&ink, "Enter Fill");
            height - footer.bottom()
        };
        let one = bottom_slack(1, &four_choices()[..1]);
        let four = bottom_slack(4, &four_choices());
        assert!(
            (one - four).abs() < 0.01,
            "a one-row card leaves {one}pt under its footer and a four-row card {four}pt; \
             the chrome is not a constant, so `CHROME_HEIGHT + ROW_HEIGHT * n` is the \
             wrong shape"
        );
        assert!(
            one >= 0.0,
            "the footer is already {}pt past the bottom of a one-row window",
            -one
        );
    }

    /// Both row treatments must be the same height, or `ROW_HEIGHT` is a
    /// single number describing two different rows.
    #[test]
    fn both_row_treatments_are_the_same_height() {
        let ink = painted(&four_choices(), overlay_height(4));
        let tiles = row_tiles(&ink);
        assert_eq!(tiles.len(), 4);
        // Positive control: the two treatments really are DIFFERENT, so this
        // is a claim about two variants and not about one drawn four times.
        let fills: Vec<Color32> = ink
            .iter()
            .filter(|i| {
                matches!(i.tile, Some((_, 8))) && i.rect.width() > OVERLAY_WIDTH / 2.0
            })
            .map(|i| match i.tile {
                Some((fill, _)) => fill,
                None => unreachable!(),
            })
            .collect();
        assert_eq!(fills.len(), 4);
        assert_eq!(fills[0], theme::BLUE_WASH, "the first row is the selected one");
        assert!(
            fills[1..].iter().all(|f| *f == theme::CANVAS),
            "rows after the first are the neutral treatment; got {fills:?}"
        );
        let first = tiles[0].height();
        for tile in &tiles[1..] {
            assert!(
                (tile.height() - first).abs() < 0.01,
                "the selected row is {first}pt tall and a neutral one {}pt",
                tile.height()
            );
        }
    }

    /// The card with no choices -- which is every card production draws until
    /// step 5 -- is the card it has always been: one row, selected treatment,
    /// same height, inside 164.
    #[test]
    fn a_card_with_no_choices_is_still_the_one_row_card() {
        let ink = painted(&[], 164.0);
        let tiles = row_tiles(&ink);
        assert_eq!(tiles.len(), 1, "no choices means exactly one row, not none");
        assert!(fits(tiles[0], window(164.0)));
        let username = glyph_run(&ink, USER);
        assert!(fits(username, window(164.0)));
        // And it is byte-identical in geometry to the one-choice card's row.
        let one_choice = row_tiles(&painted(&four_choices()[..1], overlay_height(1)));
        assert_eq!(one_choice.len(), 1);
        assert!(
            (one_choice[0].height() - tiles[0].height()).abs() < 0.01
                && (one_choice[0].top() - tiles[0].top()).abs() < 0.01,
            "the choice row {:?} is not where the matched-credential row {:?} is",
            one_choice[0],
            tiles[0]
        );
    }

    // ------------------------------------------- which row answered, and how

    /// **Click row `i`, get choice `i`.** The whole point of the step: four
    /// rows that all answer `choices[0]` are four ways to do one thing, and
    /// look exactly like four working rows to a test that only asks whether
    /// a fill happened.
    #[test]
    fn each_row_answers_its_own_choice() {
        let choices = four_choices();
        let tiles = row_tiles(&painted(&choices, overlay_height(choices.len())));
        assert_eq!(
            tiles.len(),
            choices.len(),
            "the card lost a row before a single click was sent -- egui culls shapes that \
             fall outside the screen rect, so a pushed-out row comes back as nothing"
        );

        let mut answers = Vec::new();
        for (index, tile) in tiles.iter().enumerate() {
            match click_on(&choices, tile.center()) {
                OverlayAction::Fill(choice) => answers.push(choice),
                other => panic!("row {index} at {tile:?} answered {other:?}, not a fill"),
            }
        }

        assert_eq!(answers.len(), 4, "the loop must have clicked all four rows");
        assert_eq!(
            answers, choices,
            "row i must answer choice i, in the order the rows are drawn"
        );
        // Pairwise distinct: a mapping that answers `choices[0]` for every row
        // would satisfy a weaker per-row assertion against a fixture whose
        // rows happened to repeat.
        for (i, a) in answers.iter().enumerate() {
            for b in &answers[i + 1..] {
                assert_ne!(a, b, "two rows answered the same choice: {answers:?}");
            }
        }
    }

    /// Clicking nothing in particular answers nothing -- the control that
    /// makes the test above about the ROWS rather than about clicking.
    #[test]
    fn a_click_that_lands_on_no_row_answers_nothing() {
        let choices = four_choices();
        let tiles = row_tiles(&painted(&choices, overlay_height(choices.len())));
        assert_eq!(tiles.len(), 4);
        // The footer strip, below every row.
        let below = egui::pos2(OVERLAY_WIDTH / 2.0, tiles[3].bottom() + 12.0);
        assert!(below.y < overlay_height(4), "the probe is inside the window");
        assert_eq!(click_on(&choices, below), OverlayAction::None);
    }

    /// **Enter takes the PRIMARY row, which is the first one.**
    ///
    /// The fixture's first row is deliberately not the one any of the obvious
    /// wrong implementations would reach for -- not `Saved` (the no-choices
    /// fallback), not `UserTabPass` (the historical fill), and not the last
    /// row -- so `enter fills the password field` is a claim about position
    /// and not about which variant happens to be around.
    #[test]
    fn enter_takes_the_first_row() {
        let choices = vec![
            FillChoice::Just(FieldRef::Password),
            FillChoice::UserTabPass,
            FillChoice::Saved,
        ];
        // The fixture controls: the rows really do differ, so "the first" is a
        // distinguishable answer.
        assert_ne!(choices[0], choices[1]);
        assert_ne!(choices[0], choices[2]);
        assert_ne!(choices[0], *choices.last().unwrap());

        assert_eq!(primary_choice(&choices), FillChoice::Just(FieldRef::Password));
        // And through the keyboard, which is the path that actually reaches
        // the user: Enter, not a click, not the card.
        assert_eq!(
            keyboard_action(true, false, &choices),
            OverlayAction::Fill(FillChoice::Just(FieldRef::Password))
        );
        // And with no choices at all -- the card production still draws --
        // Enter is the fill it has always been.
        assert_eq!(primary_choice(&[]), FillChoice::Saved);
        assert_eq!(
            keyboard_action(true, false, &[]),
            OverlayAction::Fill(FillChoice::Saved)
        );
    }

    /// **Esc dismisses, and dismissing is not a fill.** An Esc that answered
    /// `Some(primary)` -- one line, and the same shape as the Enter arm right
    /// above it -- types the user's password into the app they just said no
    /// to. Nothing else in this module could see it: `OverlayApp::ui` needs a
    /// real window.
    #[test]
    fn escape_dismisses_and_answers_no_choice() {
        let choices = four_choices();
        assert_eq!(keyboard_action(false, true, &choices), OverlayAction::Dismiss);
        assert_eq!(keyboard_action(false, true, &[]), OverlayAction::Dismiss);
        // Not a fill of anything, spelled out so a `Fill` variant added later
        // cannot slip past an equality against one particular value.
        assert!(!matches!(
            keyboard_action(false, true, &choices),
            OverlayAction::Fill(_)
        ));
        // The controls: the instrument does report fills, and reports nothing
        // when nothing was pressed.
        assert!(matches!(
            keyboard_action(true, false, &choices),
            OverlayAction::Fill(_)
        ));
        assert_eq!(keyboard_action(false, false, &choices), OverlayAction::None);
        // Both at once: the fill wins, as it did when these were two `if`s.
        assert_eq!(
            keyboard_action(true, true, &choices),
            OverlayAction::Fill(choices[0].clone())
        );
    }

    /// **The viewport is sized for the rows it was given.**
    ///
    /// `show_prompt_overlay` opens a real, always-on-top window and calls
    /// `eframe::run_native`, so no test in this crate may execute it -- which
    /// is exactly why the one number in it that can be silently wrong is
    /// pinned by source position. `overlay_height(1)` in place of
    /// `overlay_height(choices.len())` builds a 164pt window for a 314pt card:
    /// three of four rows below the bottom edge of a frameless window with no
    /// scrollbar, and every drawing test in this module still green, because
    /// the CARD is fine and it is the WINDOW that is too small.
    ///
    /// The arithmetic itself is behavioural, and asserted here beside the pin.
    #[test]
    fn the_viewport_is_sized_for_the_rows_it_was_given() {
        // Split across two literals so it cannot match its own declaration.
        let needle = concat!("with_inner_size([OVERLAY_WIDTH, ", "overlay_height(choices.len())])");
        let source = include_str!("overlay_ui.rs");
        assert_eq!(
            source.matches(needle).count(),
            1,
            "expected {needle:?} exactly once. Zero means the overlay window is no longer \
             sized for the card it is about to draw, and the rows past the first are off \
             the bottom of a window the user cannot scroll or resize"
        );
        // The counter's controls: it finds what is there, and does NOT find
        // the mutation this pin exists for.
        let stale = concat!("with_inner_size([OVERLAY_WIDTH, ", "overlay_height(1)])");
        assert_eq!(source.matches(stale).count(), 0, "planted: {stale}");
        assert_eq!(stale.matches(needle).count(), 0);
        assert_eq!(needle.matches(needle).count(), 1);

        // And what those two expressions actually differ by, for the card
        // sizes the overlay can show.
        let mut checked = 0;
        for rows in 2..=4 {
            assert!(
                overlay_height(rows) > overlay_height(1),
                "a {rows}-row card is not taller than a one-row card, so the pin above is \
                 pinning a distinction that does not exist"
            );
            checked += 1;
        }
        assert_eq!(checked, 3);
    }

    /// POSITIVE CONTROL for every "is inside the window" assertion above, and
    /// for the row COUNT assertion beside them.
    ///
    /// Without this, a `fits` that answered `true` unconditionally, or a
    /// window rect that was secretly unbounded, would make the whole module
    /// green and blind. Four rows really do overflow a window sized for one,
    /// and both instruments really do say so:
    ///
    /// * the third row is painted straddling the bottom edge, and `fits`
    ///   rejects it;
    /// * the fourth row falls entirely past the edge and egui's own culling
    ///   drops its tile from `output.shapes` altogether -- so the tile count
    ///   comes back 3 rather than 4. That is exactly the shape of the failure
    ///   `every_choice_row_is_inside_the_window_...` counts for: a row that
    ///   is off the window is not a row that is merely misplaced, it is a row
    ///   that is *not there*, and an uncounted loop would call that a pass.
    #[test]
    fn the_fit_check_can_actually_fail() {
        let short = overlay_height(1);
        let ink = painted(&four_choices(), short);
        let tiles = row_tiles(&ink);

        assert_eq!(
            tiles.len(),
            3,
            "four rows in a {short}pt window: expected the fourth to fall off the window \
             entirely and be culled, leaving 3 painted tiles; found {}",
            tiles.len()
        );
        assert!(
            tiles.len() != 4,
            "the row-count assertion cannot distinguish a four-row card from a \
             three-row one"
        );
        let straddling = tiles[2];
        assert!(
            !fits(straddling, window(short)),
            "the third row is at {straddling:?} in a {short}pt window and `fits` still \
             said yes -- the fit check cannot fail, so its passes upstairs mean nothing"
        );
        assert!(straddling.bottom() > short);
        // ... and the very same rect IS accepted by a window tall enough for
        // it, so `fits` is discriminating on geometry and not simply always
        // saying no.
        assert!(fits(straddling, window(overlay_height(4))));
    }

    /// POSITIVE CONTROL for the alpha filter. Nothing in the real card paints
    /// at alpha 0, so [`painted`]'s `retain` would be untestable dead code
    /// otherwise; this proves it discards what it claims to.
    #[test]
    fn a_shape_painted_at_alpha_zero_is_not_counted_as_ink() {
        let mut out = Vec::new();
        let transparent = egui::Shape::rect_filled(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 48.0)),
            CornerRadius::same(8),
            Color32::TRANSPARENT,
        );
        walk(&transparent, &mut out);
        assert_eq!(out.len(), 1, "the walker must see the shape at all");
        assert_eq!(out[0].alpha, 0);
        // ... and a row-shaped tile at full alpha IS counted, so the filter
        // discriminates on alpha rather than on shape.
        let mut solid_out = Vec::new();
        let solid = egui::Shape::rect_filled(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 48.0)),
            CornerRadius::same(8),
            theme::BLUE_WASH,
        );
        walk(&solid, &mut solid_out);
        assert_eq!(solid_out[0].alpha, 255);

        out.retain(|i| i.alpha > 0);
        assert!(row_tiles(&out).is_empty());
        solid_out.retain(|i| i.alpha > 0);
        assert_eq!(row_tiles(&solid_out).len(), 1);
    }
}

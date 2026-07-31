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

/// Opens the autofill overlay for `app_name`: a small, frameless,
/// always-on-top card (design 2a — "no chrome") with the Deskwarden header,
/// the matched credential row, and a keyboard-hint footer.
///
/// Returns `true` if the user chose to fill (clicked the row or pressed
/// Enter), `false` if they dismissed it (the header's ✕, Esc, or closing the
/// window).
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
) -> bool {
    if OVERLAY_OPEN.swap(true, Ordering::SeqCst) {
        log::warn!(
            "autofill overlay requested for {app_name} while one is already open in this \
             process; ignoring rather than stacking a second window"
        );
        return false;
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
    let fill_clicked = Rc::new(RefCell::new(false));

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([396.0, 164.0])
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
        fill_clicked: fill_clicked.clone(),
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

    let clicked = *fill_clicked.borrow();
    clicked
}

struct OverlayApp {
    app_name: String,
    item_name: String,
    username: Option<String>,
    fill_clicked: Rc<RefCell<bool>>,
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
        let mut done = false;

        // The overlay is keyboard-first (design 2a's footer: "↵ Fill · Esc
        // Dismiss"): Enter fills, Esc dismisses, no focus juggling needed.
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            *self.fill_clicked.borrow_mut() = true;
            done = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = true;
        }

        match draw_overlay_card(
            ui,
            &self.app_name,
            &self.item_name,
            self.username.as_deref(),
        ) {
            OverlayAction::Fill => {
                *self.fill_clicked.borrow_mut() = true;
                done = true;
            }
            OverlayAction::Dismiss => done = true,
            OverlayAction::None => {}
        }

        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// What the user did to the overlay card on this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayAction {
    /// Nothing yet; keep the overlay up.
    #[default]
    None,
    /// Fill the matched credentials (the credential row was clicked).
    Fill,
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

        // The matched credential row, selected treatment.
        egui::Frame::new()
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                let (primary, secondary) = row_text(app_name, item_name, username);
                if credential_row(ui, &primary, &secondary) {
                    action = OverlayAction::Fill;
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

/// The selected credential row (blue wash, avatar, Enter chip). Returns true
/// when clicked.
fn credential_row(ui: &mut egui::Ui, primary: &str, secondary: &str) -> bool {
    let row = egui::Frame::new()
        .fill(theme::BLUE_WASH)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(primary), 28.0, true);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(primary, 13.0).color(theme::INK));
                    ui.label(RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::kbd_chip(ui, "Enter", true);
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

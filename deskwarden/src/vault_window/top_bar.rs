//! The v2 top bar (design 1a, spec part A section 2.3): search, the sync
//! pill, the account avatar and the window's own caption buttons, 72 high
//! across the main column.

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Rect, Sense, Stroke, Ui, UiBuilder};

use crate::kit::icon;
use crate::kit::tips;
use crate::kit::tokens::{self, palette, Role};
use crate::theme;

/// The bar's height (design 1a: 72).
pub const HEIGHT: f32 = 72.0;
/// The space left of the search field.
const PAD_LEFT: f32 = 28.0;
/// Between the bar's parts.
const GAP: f32 = 14.0;
/// The search field at full width (design 1a: 460 by 46).
const SEARCH: egui::Vec2 = egui::vec2(460.0, 46.0);
/// The narrowest the search field gives way to: at the window's 930 minimum
/// with the longest failure pill ("Vault not refreshed · 30 d old") the
/// field is about 200 wide, so the floor sits well under that.
const SEARCH_MIN_WIDTH: f32 = 120.0;
/// The account avatar's diameter.
pub const AVATAR: f32 = 38.0;
/// The Settings gear's square, drawn like the 38 icon buttons of the spec
/// (radius 11) so it stands level with the avatar beside it.
pub const GEAR: f32 = 38.0;
/// Between the gear and the avatar: close enough to read as a pair.
const GEAR_GAP: f32 = 8.0;
/// One caption button (design 1a: 46 by 34, flush to the top-right).
pub const CAPTION: egui::Vec2 = egui::vec2(46.0, 34.0);
/// The search field's placeholder (design 1a, verbatim).
pub fn search_placeholder() -> &'static str {
    crate::i18n::t("topbar.search_placeholder")
}
/// The field's chip while it is empty: the chord that focuses it.
const SEARCH_CHIP: &str = "Ctrl K";
/// The chip once something is typed: what clears it.
const CLEAR_CHIP: &str = "Esc";

/// The pill's three looks (design 1a: Synced, Syncing, Offline), plus the
/// neutral one before a first sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillTone {
    Green,
    Amber,
    Neutral,
}

/// What the sync pill says, how it is coloured, and its hover hint.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncPill {
    pub label: String,
    pub tone: PillTone,
    pub hint: String,
}

/// The part of `text` that stands where the NUL sits in `probe`, if `text` is
/// `probe` with something put there -- how a label that was built from a
/// catalog template is taken apart again without knowing its language.
pub(crate) fn unfill<'a>(probe: &str, text: &'a str) -> Option<&'a str> {
    let (head, tail) = probe.split_once('\u{0}')?;
    text.strip_prefix(head)?.strip_suffix(tail)
}

impl SyncPill {
    /// The v2 pill for what `vault_window::sync_pill` decided -- that function
    /// keeps the precedence between the app's sync states, and this only says
    /// it the design's way:
    ///
    /// - a running sync reads "Syncing…" and a finished one plain "Synced",
    ///   in green, its age moving into the hint;
    /// - a vault loaded from the disk cache reads "Offline", in amber, its age
    ///   in the hint;
    /// - a failure keeps its own words (the design has no failed state, and
    ///   the age a failure carries is the thing a reader needs), in amber;
    /// - before any sync, a neutral "Sync".
    pub fn from_v1(dot: Color32, label: &str) -> SyncPill {
        // The label arrives already worded (`vault_window::sync_pill`), so what
        // it is has to be recognised against the catalog's own text for each
        // state, never against English words.
        let sync_now = || crate::i18n::t("topbar.sync_now").to_string();
        let with = |detail: &str| crate::i18n::tf("topbar.sync_now_detail", &[("detail", detail)]);
        if label == crate::i18n::t("vault.pill_syncing") {
            return SyncPill { label: crate::i18n::t("vault.pill_syncing").into(), tone: PillTone::Green, hint: sync_now() };
        }
        if dot == theme::ERROR {
            return SyncPill { label: label.into(), tone: PillTone::Amber, hint: sync_now() };
        }
        let synced = crate::i18n::tf("vault.pill_synced", &[("ago", "\u{0}")]);
        if let Some(age) = unfill(&synced, label) {
            return SyncPill {
                label: crate::i18n::t("topbar.pill_synced").into(),
                tone: PillTone::Green,
                hint: with(&crate::i18n::tf("topbar.hint_synced", &[("age", age)])),
            };
        }
        let cached = crate::i18n::tf("vault.pill_cached", &[("age", "\u{0}")]);
        if let Some(age) = unfill(&cached, label) {
            return SyncPill {
                label: crate::i18n::t("topbar.pill_offline").into(),
                tone: PillTone::Amber,
                hint: with(&crate::i18n::tf("topbar.hint_saved_copy", &[("age", age)])),
            };
        }
        SyncPill { label: label.into(), tone: PillTone::Neutral, hint: sync_now() }
    }
}

/// What the bar was asked to do this frame. Dragging the window is done
/// directly (`ViewportCommand::StartDrag`), as the v1 chrome did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopBarAction {
    None,
    Sync,
    Minimize,
    ToggleMaximize,
    Close,
}

/// Draws the bar into `bar` without moving `ui`'s cursor -- the window lays
/// its panels out under it -- and reports what was pressed. `avatar` draws
/// the account avatar and its menu into a 38-point slot. `placeholder` is the
/// empty field's hint: the window passes the count-aware one 0.15 showed
/// ("Search 12 logins") until slice 3 gives the list a header that carries
/// the count, after which it is [`SEARCH_PLACEHOLDER`].
///
/// `gear_on` paints the Settings gear selected (one of the screens its menu
/// holds is up). The gear's [`egui::Response`] is left in `gear` for the
/// caller to hang the menu off: the menu's counts are only known further down
/// the frame, and a popup needs nothing but its anchor.
///
/// `controls_only` is the splash's case: while the vault loads the splash
/// covers the bar, so only the caption buttons and the drag area answer --
/// nothing hidden under it may start a sync, take the keyboard or open the
/// account menu.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut Ui,
    bar: Rect,
    search: &mut String,
    placeholder: &str,
    pill: &SyncPill,
    maximized: bool,
    controls_only: bool,
    avatar: impl FnOnce(&mut Ui),
    gear_on: bool,
    gear: &mut Option<egui::Response>,
) -> TopBarAction {
    let p = palette(ui.ctx());
    let mut child = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("vault-top-bar"));
    let ui = &mut child;
    ui.painter().rect_filled(bar, CornerRadius::ZERO, p.card);
    let mut action = TopBarAction::None;

    // Right to left: the caption buttons, the avatar, the pill.
    let captions_left = bar.right() - CAPTION.x * 3.0;
    for (index, kind) in [Caption::Minimize, Caption::Maximize, Caption::Close].into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(captions_left + CAPTION.x * index as f32, bar.top()), CAPTION);
        if caption_button(ui, rect, kind, maximized).clicked() {
            action = match kind {
                Caption::Minimize => TopBarAction::Minimize,
                Caption::Maximize => TopBarAction::ToggleMaximize,
                Caption::Close => TopBarAction::Close,
            };
        }
    }
    if controls_only {
        let zone = Rect::from_min_max(bar.min, pos2(captions_left, bar.bottom()));
        let response = ui.interact(zone, ui.id().with("drag-loading"), Sense::click_and_drag());
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            action = TopBarAction::ToggleMaximize;
        }
        return action;
    }
    let avatar_rect = Rect::from_center_size(
        pos2(captions_left - GAP - AVATAR / 2.0, bar.center().y),
        egui::Vec2::splat(AVATAR),
    );
    ui.scope_builder(UiBuilder::new().max_rect(avatar_rect), avatar);

    let gear_rect = Rect::from_center_size(
        pos2(avatar_rect.left() - GEAR_GAP - GEAR / 2.0, bar.center().y),
        egui::Vec2::splat(GEAR),
    );
    *gear = Some(gear_button(ui, gear_rect, gear_on));

    let pill_rect = pill_rect(ui, pill, gear_rect.left() - GAP, bar.center().y);
    if sync_pill(ui, pill_rect, pill).clicked() {
        action = TopBarAction::Sync;
    }

    // The search field takes what is left, up to the design's 460.
    let room = (pill_rect.left() - GAP - (bar.left() + PAD_LEFT)).max(SEARCH_MIN_WIDTH);
    let search_rect = Rect::from_min_size(
        pos2(bar.left() + PAD_LEFT, bar.center().y - SEARCH.y / 2.0),
        vec2(SEARCH.x.min(room), SEARCH.y),
    );
    search_field(ui, search_rect, search, placeholder);

    // Everything else drags the window: left of the search, and between the
    // search and the pill. A double-click maximises, as a title bar does.
    for zone in [
        Rect::from_min_max(bar.min, pos2(search_rect.left(), bar.bottom())),
        Rect::from_min_max(pos2(search_rect.right(), bar.top()), pos2(pill_rect.left(), bar.bottom())),
    ] {
        if zone.width() <= 0.0 {
            continue;
        }
        let response = ui.interact(zone, ui.id().with(("drag", zone.left() as i32)), Sense::click_and_drag());
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            action = TopBarAction::ToggleMaximize;
        }
    }
    action
}

/// The id of the Settings gear, for tests and the preview to find it.
pub fn gear_id() -> egui::Id {
    egui::Id::new("vault-top-bar-gear")
}

/// The Settings gear: a 38 square, radius 11, grey on hover, the blue wash
/// and ink while one of its menu's screens is the current one.
fn gear_button(ui: &mut Ui, rect: Rect, on: bool) -> egui::Response {
    let p = palette(ui.ctx());
    let response = ui.interact(rect, gear_id(), Sense::click());
    let (bg, ink) = if on {
        (Some(p.blue_wash), p.blue_deep)
    } else if response.hovered() {
        (Some(p.hover), p.text_icon)
    } else {
        (None, p.text_icon)
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, CornerRadius::same(11), bg);
    }
    icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0)), icon::glyph::SETTINGS, 2.0, ink);
    tips::hint(ui, &response, crate::vault_window::sidebar::settings_row_label(), &[]);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn pill_role() -> Role {
    tokens::PILL
}

/// Where the pill sits: right-aligned to `right`, 34 high, sized to its label.
fn pill_rect(ui: &Ui, pill: &SyncPill, right: f32, center_y: f32) -> Rect {
    let width = ui.fonts_mut(|f| f.layout_job(tokens::job(pill_role(), &pill.label, Color32::BLACK)).size().x);
    let w = 12.0 + 7.0 + 8.0 + width + 12.0;
    Rect::from_min_size(pos2(right - w, center_y - 17.0), vec2(w, 34.0))
}

fn sync_pill(ui: &mut Ui, rect: Rect, pill: &SyncPill) -> egui::Response {
    let p = palette(ui.ctx());
    let (bg, ink, dot) = match pill.tone {
        PillTone::Green => (p.done_wash, p.done_ink, p.done_mark),
        PillTone::Amber => (p.caution_wash, p.caution_mark, p.caution_dot),
        PillTone::Neutral => (p.field_bg, p.text_muted, p.text_ghost),
    };
    let response = ui.interact(rect, ui.id().with("sync-pill"), Sense::click());
    // `filter: brightness(.97)` on hover.
    let bg = if response.hovered() { bg.gamma_multiply(0.97) } else { bg };
    ui.painter().rect_filled(rect, CornerRadius::same(255), bg);
    ui.painter().circle_filled(pos2(rect.left() + 12.0 + 3.5, rect.center().y), 3.5, dot);
    let label = ui.painter().layout_job(tokens::job(pill_role(), &pill.label, ink));
    ui.painter().galley(pos2(rect.left() + 12.0 + 7.0 + 8.0, rect.center().y - label.size().y / 2.0), label, ink);
    tips::hint(ui, &response, &pill.hint, &["Mod", "Shift", "Y"]);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The search field (design 1a): 46 high, radius 14, field grey, a blue edge
/// while it has the keyboard, a magnifier, and the chip that reads `Ctrl K`
/// while empty and `Esc` -- which clears it -- once something is typed. It
/// keeps the id Ctrl+F focuses, so the window's chord still finds it.
fn search_field(ui: &mut Ui, rect: Rect, value: &mut String, placeholder: &str) {
    let p = palette(ui.ctx());
    let id = crate::vault_window::item_list::search_field_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let edge = if focused { p.blue } else { Color32::TRANSPARENT };
    ui.painter().rect(rect, CornerRadius::same(14), p.field_bg, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    let glass = Rect::from_min_size(pos2(rect.left() + 16.0, rect.center().y - 9.0), egui::Vec2::splat(18.0));
    icon::stroke(ui.painter(), glass, icon::glyph::SEARCH, 2.2, p.text_label);

    // The chip, measured first: the text runs up to it.
    let clearable = !value.is_empty();
    let chip_role = Role { size: 12.0, face: theme::REGULAR, tracking_em: 0.0 };
    let chip_text = ui.painter().layout_job(tokens::job(chip_role, if clearable { CLEAR_CHIP } else { SEARCH_CHIP }, p.text_ghost));
    let chip = Rect::from_min_size(
        pos2(rect.right() - 16.0 - chip_text.size().x - 12.0, rect.center().y - (chip_text.size().y + 4.0) / 2.0),
        vec2(chip_text.size().x + 12.0, chip_text.size().y + 4.0),
    );
    let chip_response = ui.interact(chip, id.with("chip"), Sense::click());
    let chip_ink = if chip_response.hovered() { p.blue } else { p.text_ghost };
    ui.painter().rect_stroke(
        chip,
        CornerRadius::same(6),
        Stroke::new(1.0, if chip_response.hovered() { p.blue } else { p.kbd_border }),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(chip.min + vec2(6.0, 2.0), chip_text, chip_ink);
    if clearable {
        if chip_response.clicked() {
            value.clear();
        }
    } else {
        if chip_response.clicked() {
            super::sidebar::request_shell(ui.ctx(), super::sidebar::ShellRequest::FocusSearch);
        }
        tips::hint(ui, &chip_response, crate::i18n::t("topbar.search_placeholder"), &["Mod", "K"]);
    }

    let font = tokens::font_id(tokens::BODY);
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let inner = Rect::from_min_max(
        pos2(glass.right() + 10.0, rect.center().y - row / 2.0),
        pos2(chip.left() - 10.0, rect.center().y + row / 2.0),
    );
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .hint_text(egui::RichText::new(placeholder).font(font.clone()).color(p.text_label))
            .frame(egui::Frame::NONE)
            .font(font)
            .text_color(p.ink)
            .margin(egui::Margin::ZERO)
            .desired_width(inner.width()),
    );
    if clearable && response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        value.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caption {
    Minimize,
    Maximize,
    Close,
}

/// One caption button, painted: a bar, a square (two when maximised), a cross.
fn caption_button(ui: &mut Ui, rect: Rect, kind: Caption, maximized: bool) -> egui::Response {
    let p = palette(ui.ctx());
    let response = ui.interact(rect, ui.id().with(("caption", kind as u8)), Sense::click());
    let (bg, ink) = match (kind, response.hovered()) {
        (Caption::Close, true) => (p.close_hover, Color32::WHITE),
        (_, true) => (p.hover, p.text_icon),
        (_, false) => (Color32::TRANSPARENT, p.text_icon),
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    }
    let c = rect.center();
    let stroke = Stroke::new(1.0, ink);
    match kind {
        Caption::Minimize => {
            ui.painter().line_segment([pos2(c.x - 5.0, c.y), pos2(c.x + 5.0, c.y)], stroke);
        }
        Caption::Maximize => {
            let square = Rect::from_center_size(c, egui::Vec2::splat(10.0));
            if maximized {
                let back = square.translate(vec2(2.0, -2.0));
                ui.painter().rect_stroke(back, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
                ui.painter().rect_filled(square, CornerRadius::ZERO, if bg == Color32::TRANSPARENT { p.card } else { bg });
            }
            ui.painter().rect_stroke(square, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
        }
        Caption::Close => {
            ui.painter().line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
            ui.painter().line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], stroke);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, Painted};
    use crate::kit::tokens::Palette;
    use eframe::egui::{self, pos2, vec2, Rect};

    const P: Palette = Palette::LIGHT;
    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 820.0);
    const BAR: Rect = Rect { min: pos2(252.0, 0.0), max: pos2(1280.0, 72.0) };

    fn green() -> SyncPill {
        SyncPill { label: "Synced".into(), tone: PillTone::Green, hint: "Sync now".into() }
    }

    struct Run {
        painted: Painted,
        action: TopBarAction,
        search: String,
    }

    fn run(ctx: &egui::Context, time: f64, size: egui::Vec2, bar: Rect, pill: &SyncPill, search: &str, events: Vec<egui::Event>) -> Run {
        run_mode(ctx, time, size, bar, pill, search, events, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_mode(
        ctx: &egui::Context,
        time: f64,
        size: egui::Vec2,
        bar: Rect,
        pill: &SyncPill,
        search: &str,
        events: Vec<egui::Event>,
        controls_only: bool,
    ) -> Run {
        let mut text = search.to_owned();
        let mut action = TopBarAction::None;
        let painted = testing::frame(ctx, time, size, events, |ui| {
            let mut gear = None;
            action = draw(ui, bar, &mut text, search_placeholder(), pill, false, controls_only, |ui| {
                ui.allocate_exact_size(egui::Vec2::splat(AVATAR), egui::Sense::click());
            }, false, &mut gear);
        });
        Run { painted, action, search: text }
    }

    fn rect_filled(painted: &Painted, fill: egui::Color32) -> Vec<Rect> {
        painted.rects.iter().filter(|r| r.fill == fill).map(|r| r.rect).collect()
    }

    #[test]
    fn the_bar_is_72_tall_and_white() {
        let ctx = testing::ctx();
        let r = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        assert!(rect_filled(&r.painted, P.card).contains(&BAR), "no white bar at {BAR:?}");
        assert_eq!(BAR.height(), HEIGHT);
    }

    #[test]
    fn the_search_is_460_by_46_in_the_field_grey() {
        let ctx = testing::ctx();
        let r = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let field = rect_filled(&r.painted, P.field_bg);
        assert!(
            field.iter().any(|f| f.size() == vec2(460.0, 46.0) && f.left() == BAR.left() + 28.0),
            "{field:?}"
        );
        assert!(r.painted.text(search_placeholder()).is_some());
        assert!(r.painted.text("Ctrl K").is_some());
    }

    #[test]
    fn typing_in_the_top_bar_search_fills_the_vault_search() {
        let ctx = testing::ctx();
        let first = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let field = first.painted.text(search_placeholder()).unwrap().2.center();
        run(&ctx, 0.1, SCREEN, BAR, &green(), "", testing::click_at(field));
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(crate::vault_window::item_list::search_field_id()),
            "the field keeps the id Ctrl+K focuses"
        );
        let typed = run(&ctx, 0.2, SCREEN, BAR, &green(), "", vec![egui::Event::Text("led".into())]);
        assert_eq!(typed.search, "led");
    }

    #[test]
    fn the_chip_reads_esc_once_something_is_typed_and_clicking_it_clears() {
        let ctx = testing::ctx();
        let r = run(&ctx, 0.0, SCREEN, BAR, &green(), "led", vec![]);
        assert!(r.painted.text("Ctrl K").is_none());
        let esc = r.painted.text("Esc").expect("the clear chip").2.center();
        let cleared = run(&ctx, 0.1, SCREEN, BAR, &green(), "led", testing::click_at(esc));
        assert_eq!(cleared.search, "");
    }

    #[test]
    fn the_sync_pill_tones_match_the_design() {
        for (tone, bg, ink, dot) in [
            (PillTone::Green, P.done_wash, P.done_ink, P.done_mark),
            (PillTone::Amber, P.caution_wash, P.caution_mark, P.caution_dot),
            (PillTone::Neutral, P.field_bg, P.text_muted, P.text_ghost),
        ] {
            let ctx = testing::ctx();
            let pill = SyncPill { label: "Label".into(), tone, hint: "Sync now".into() };
            let r = run(&ctx, 0.0, SCREEN, BAR, &pill, "", vec![]);
            let (_, color, at) = r.painted.text("Label").unwrap();
            assert_eq!(*color, ink, "{tone:?}");
            assert!(
                r.painted.rects.iter().any(|p| p.fill == bg && p.rect.height() == 34.0 && p.rect.contains(at.center())),
                "{tone:?}: no 34-high pill of {bg:?}"
            );
            assert!(r.painted.circles.iter().any(|c| c.fill == dot && (c.radius - 3.5).abs() < 0.01), "{tone:?} dot");
        }
    }

    #[test]
    fn clicking_the_pill_returns_sync() {
        let ctx = testing::ctx();
        let first = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let pill = first.painted.text("Synced").unwrap().2.center();
        let clicked = run(&ctx, 0.1, SCREEN, BAR, &green(), "", testing::click_at(pill));
        assert_eq!(clicked.action, TopBarAction::Sync);
    }

    /// One frame of the bar with the gear's state in hand: what it painted,
    /// and the gear's own response.
    fn run_gear(ctx: &egui::Context, time: f64, on: bool, events: Vec<egui::Event>) -> (Painted, Option<egui::Response>) {
        let mut text = String::new();
        let mut gear = None;
        let painted = testing::frame(ctx, time, SCREEN, events, |ui| {
            draw(ui, BAR, &mut text, search_placeholder(), &green(), false, false, |ui| {
                ui.allocate_exact_size(egui::Vec2::splat(AVATAR), egui::Sense::click());
            }, on, &mut gear);
        });
        (painted, gear)
    }

    #[test]
    fn the_settings_gear_is_a_38_square_right_beside_the_avatar_and_clicks() {
        let ctx = testing::ctx();
        let (_, gear) = run_gear(&ctx, 0.0, false, vec![]);
        let gear = gear.expect("the bar hands back the gear").rect;
        let avatar_left = BAR.right() - CAPTION.x * 3.0 - GAP - AVATAR;
        assert_eq!(gear.size(), vec2(GEAR, GEAR));
        assert_eq!(gear.right(), avatar_left - 8.0, "{gear:?} is not 8 left of the avatar at {avatar_left}");
        assert_eq!(gear.center().y, BAR.center().y);
        // Left of the avatar, right of the pill.
        let (painted, _) = run_gear(&ctx, 0.1, false, vec![]);
        let pill = painted.text("Synced").unwrap().2;
        assert!(pill.right() < gear.left(), "the pill {pill:?} runs into the gear {gear:?}");
        let (_, clicked) = run_gear(&ctx, 0.2, false, testing::click_at(gear.center()));
        assert!(clicked.unwrap().clicked(), "the gear did not answer a click");
        // A tooltip names it.
        let (hovered, _) = run_gear(&ctx, 0.3, false, testing::hover_at(gear.center()));
        let (hovered, _) = {
            let _ = hovered;
            run_gear(&ctx, 0.6, false, testing::hover_at(gear.center()))
        };
        assert!(hovered.text(crate::vault_window::sidebar::settings_row_label()).is_some(), "no Settings tooltip");
    }

    #[test]
    fn the_gear_is_lit_while_one_of_its_screens_is_up_and_grey_on_hover() {
        let ctx = testing::ctx();
        let (resting, gear) = run_gear(&ctx, 0.0, false, vec![]);
        let rect = gear.unwrap().rect;
        assert!(!rect_filled(&resting, P.blue_wash).contains(&rect), "lit while nothing is up");
        assert!(!rect_filled(&resting, P.hover).contains(&rect), "grey at rest");
        let (hovered, _) = run_gear(&ctx, 0.1, false, testing::hover_at(rect.center()));
        assert!(rect_filled(&hovered, P.hover).contains(&rect), "not grey on hover");
        let (on, _) = run_gear(&ctx, 0.2, true, vec![]);
        assert!(rect_filled(&on, P.blue_wash).contains(&rect), "not lit while a screen is up");
    }

    fn caption(bar: Rect, nth_from_right: usize) -> Rect {
        Rect::from_min_size(
            pos2(bar.right() - CAPTION.x * (nth_from_right + 1) as f32, bar.top()),
            CAPTION,
        )
    }

    #[test]
    fn the_caption_buttons_are_46_by_34_flush_top_right_and_close_hovers_red() {
        let ctx = testing::ctx();
        let close = caption(BAR, 0);
        run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let hovered = run(&ctx, 0.1, SCREEN, BAR, &green(), "", testing::hover_at(close.center()));
        assert!(rect_filled(&hovered.painted, P.close_hover).contains(&close), "close does not hover red");
        let clicked = run(&ctx, 0.2, SCREEN, BAR, &green(), "", testing::click_at(close.center()));
        assert_eq!(clicked.action, TopBarAction::Close);
        let min = run(&ctx, 0.3, SCREEN, BAR, &green(), "", testing::click_at(caption(BAR, 2).center()));
        assert_eq!(min.action, TopBarAction::Minimize);
        let max = run(&ctx, 0.4, SCREEN, BAR, &green(), "", testing::click_at(caption(BAR, 1).center()));
        assert_eq!(max.action, TopBarAction::ToggleMaximize);
    }

    fn drag_spot(painted: &Painted) -> egui::Pos2 {
        // Between the search field and the pill: empty bar.
        let pill = painted.text("Synced").unwrap().2;
        pos2(pill.left() - 60.0, BAR.center().y)
    }

    #[test]
    fn the_bar_between_the_search_and_the_pill_drags_the_window() {
        let ctx = testing::ctx();
        let first = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let at = drag_spot(&first.painted);
        let press = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ];
        run(&ctx, 0.1, SCREEN, BAR, &green(), "", press);
        let moved = run(&ctx, 0.2, SCREEN, BAR, &green(), "", vec![egui::Event::PointerMoved(at + vec2(40.0, 10.0))]);
        assert!(
            moved.painted.commands.iter().any(|c| matches!(c, egui::ViewportCommand::StartDrag)),
            "{:?}",
            moved.painted.commands
        );
    }

    #[test]
    fn double_clicking_the_bar_toggles_maximise() {
        let ctx = testing::ctx();
        let first = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let at = drag_spot(&first.painted);
        run(&ctx, 0.1, SCREEN, BAR, &green(), "", testing::click_at(at));
        let second = run(&ctx, 0.2, SCREEN, BAR, &green(), "", testing::click_at(at));
        assert_eq!(second.action, TopBarAction::ToggleMaximize);
    }

    #[test]
    fn the_caption_buttons_stay_on_screen_at_the_minimum_width() {
        let width = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32;
        let bar = Rect::from_min_max(pos2(252.0, 0.0), pos2(width, 72.0));
        let ctx = testing::ctx();
        let pill = SyncPill { label: "Sync failed · 3 d old".into(), tone: PillTone::Amber, hint: "Sync now".into() };
        let r = run(&ctx, 0.0, vec2(width, 600.0), bar, &pill, "", vec![]);
        let field = rect_filled(&r.painted, P.field_bg);
        let search = field.iter().find(|f| f.height() == 46.0).expect("the search field");
        let pill_at = r.painted.text("Sync failed").unwrap().2;
        assert!(search.right() < pill_at.left(), "search {search:?} runs into the pill {pill_at:?}");
        assert!(caption(bar, 2).left() >= pill_at.right(), "the pill runs under the caption buttons");
        assert!(caption(bar, 0).right() <= width);
    }

    /// While the vault loads the splash covers the bar, so nothing under it
    /// may answer a click -- except the window's own caption buttons.
    #[test]
    fn while_the_vault_loads_only_the_window_controls_answer() {
        let ctx = testing::ctx();
        let shown = run(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![]);
        let pill = shown.painted.text("Synced").unwrap().2.center();
        let field = shown.painted.text(search_placeholder()).unwrap().2.center();
        let ctx = testing::ctx();
        run_mode(&ctx, 0.0, SCREEN, BAR, &green(), "", vec![], true);
        let pressed = run_mode(&ctx, 0.1, SCREEN, BAR, &green(), "", testing::click_at(pill), true);
        assert_eq!(pressed.action, TopBarAction::None, "the hidden pill started a sync");
        run_mode(&ctx, 0.2, SCREEN, BAR, &green(), "", testing::click_at(field), true);
        assert_eq!(ctx.memory(|m| m.focused()), None, "the hidden search took the keyboard");
        let close = run_mode(&ctx, 0.3, SCREEN, BAR, &green(), "", testing::click_at(caption(BAR, 0).center()), true);
        assert_eq!(close.action, TopBarAction::Close);
    }

    /// The longest pill the window can show, at the window's minimum width:
    /// it must not run over the search field.
    #[test]
    fn a_long_failure_pill_never_overlaps_the_search_at_the_minimum_width() {
        let width = crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32;
        let bar = Rect::from_min_max(pos2(252.0, 0.0), pos2(width, 72.0));
        let ctx = testing::ctx();
        let label = "Vault not refreshed · 30 d old";
        let pill = SyncPill { label: label.into(), tone: PillTone::Amber, hint: "Sync now".into() };
        let r = run(&ctx, 0.0, vec2(width, 600.0), bar, &pill, "", vec![]);
        let search = rect_filled(&r.painted, P.field_bg).into_iter().find(|f| f.height() == 46.0).unwrap();
        let pill_box = r
            .painted
            .rects
            .iter()
            .find(|p| p.fill == P.caution_wash && p.rect.height() == 34.0)
            .expect("the pill")
            .rect;
        assert!(search.right() <= pill_box.left(), "search {search:?} runs into the pill {pill_box:?}");
        assert!(search.left() >= bar.left(), "the search left the bar");
    }

    fn focus_search(ctx: &egui::Context, text: &str) {
        let first = run(ctx, 0.0, SCREEN, BAR, &green(), text, vec![]);
        let field = first
            .painted
            .texts
            .iter()
            .find(|(t, _, _)| t == text || t == search_placeholder())
            .unwrap()
            .2
            .center();
        run(ctx, 0.1, SCREEN, BAR, &green(), text, testing::click_at(field));
    }

    fn escape() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]
    }

    #[test]
    fn escape_clears_a_focused_search_with_text() {
        let ctx = testing::ctx();
        focus_search(&ctx, "led");
        let after = run(&ctx, 0.2, SCREEN, BAR, &green(), "led", escape());
        assert_eq!(after.search, "");
    }

    #[test]
    fn escape_on_an_empty_focused_search_leaves_it_alone() {
        let ctx = testing::ctx();
        focus_search(&ctx, "");
        let after = run(&ctx, 0.2, SCREEN, BAR, &green(), "", escape());
        assert_eq!(after.search, "");
        assert_eq!(after.action, TopBarAction::None, "Esc on an empty field asked the window for something");
    }

    #[test]
    fn the_pill_maps_every_sync_state_to_the_design() {
        use crate::theme;
        let cases = [
            (theme::TEXT_GHOST, "Syncing…", "Syncing…", PillTone::Green),
            (theme::ERROR, "Sync failed", "Sync failed", PillTone::Amber),
            (theme::ERROR, "Vault not refreshed · 3 h old", "Vault not refreshed · 3 h old", PillTone::Amber),
            (theme::BLUE, "Synced 2 min ago", "Synced", PillTone::Green),
            (theme::TEXT_GHOST, "Loaded from cache · 3 d old", "Offline", PillTone::Amber),
            (theme::TEXT_GHOST, "Sync", "Sync", PillTone::Neutral),
        ];
        for (dot, label, want, tone) in cases {
            let pill = SyncPill::from_v1(dot, label);
            assert_eq!((pill.label.as_str(), pill.tone), (want, tone), "{label}");
            assert!(pill.hint.starts_with("Sync now"), "{label}: {}", pill.hint);
        }
        assert_eq!(SyncPill::from_v1(theme::BLUE, "Synced 2 min ago").hint, "Sync now \u{2014} synced 2 min ago");
        assert_eq!(
            SyncPill::from_v1(theme::TEXT_GHOST, "Loaded from cache · 3 d old").hint,
            "Sync now \u{2014} saved copy 3 d old"
        );
    }
}

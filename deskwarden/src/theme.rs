//! Deskwarden's visual identity, as egui styling and shared widgets.
//!
//! The single place the design language lives: the palette, the global egui
//! style, the quartered-shield mark, and the handful of composite widgets
//! (buttons, avatars, keyboard chips, header/footer bars) every window is
//! built from. Values here are lifted directly from the design document
//! committed at `docs/design/Deskwarden.dc.html` (sections 2a/2b and 3a–3g);
//! nothing is invented locally, so a mismatch with the design is a bug in
//! this file.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextStyle, Ui, Vec2,
};
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------------------
// Palette (design 3g: one blue hue in four values, warm greys for everything
// else, red reserved for actual errors).
// ---------------------------------------------------------------------------

/// Primary text ("ink").
pub const INK: Color32 = Color32::from_rgb(0x20, 0x1e, 0x1d);
/// Secondary text.
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x44, 0x41, 0x41);
/// Muted text (labels, descriptions).
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x60, 0x5d, 0x5d);
/// Faint text (hints, metadata).
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x7d, 0x79, 0x79);
/// Ghost text (counts, placeholders).
pub const TEXT_GHOST: Color32 = Color32::from_rgb(0x9b, 0x97, 0x97);

/// Window/canvas background (warm grey). The design also uses this exact grey
/// as the divider *between rows inside a card* (2b's detail rows), rather than
/// introducing a sixth grey for it -- see [`row_rule`].
pub const CANVAS: Color32 = Color32::from_rgb(0xf3, 0xf2, 0xf2);
/// App-window body background — one step warmer than `CANVAS`. Every window
/// mock in the design (3h login, 3e preferences, 2b/3f vault) fills its body
/// with this rather than the page canvas.
pub const WINDOW_BG: Color32 = Color32::from_rgb(0xf7, 0xf6, 0xf5);
/// Card background.
pub const CARD: Color32 = Color32::WHITE;
/// Tinted card background (footers, table headers).
pub const CARD_TINT: Color32 = Color32::from_rgb(0xfb, 0xfa, 0xf9);
/// Card border.
pub const BORDER: Color32 = Color32::from_rgb(0xde, 0xdb, 0xd9);
/// Hairline separators inside cards.
pub const HAIRLINE: Color32 = Color32::from_rgb(0xea, 0xe7, 0xe7);
/// Border of interactive controls (buttons, inputs).
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0xd7, 0xd3, 0xd3);

/// Deepest blue: quadrant 1, emphasized text on blue washes.
pub const BLUE_DEEP: Color32 = Color32::from_rgb(0x14, 0x30, 0x7a);
/// Primary blue: quadrant 2, primary buttons, focus borders.
pub const BLUE: Color32 = Color32::from_rgb(0x1b, 0x3f, 0xa0);
/// Bright blue: quadrant 3.
pub const BLUE_BRIGHT: Color32 = Color32::from_rgb(0x3b, 0x74, 0xe8);
/// Soft blue: quadrant 4.
pub const BLUE_SOFT: Color32 = Color32::from_rgb(0x7f, 0xa4, 0xef);
/// Blue wash: selected-row background, badges.
pub const BLUE_WASH: Color32 = Color32::from_rgb(0xee, 0xf2, 0xfc);
/// Blue edge: borders on blue-washed elements, text selection.
pub const BLUE_EDGE: Color32 = Color32::from_rgb(0xb8, 0xc7, 0xea);
/// Focus ring around the active input. The design uses this same value as
/// its deepest blue wash -- one step past [`BLUE_WASH`] -- for a chip that
/// sits ON a selected or blue-washed surface (2b's selected item row
/// carries its `app` badge in it), so it is not only a focus colour.
pub const FOCUS_RING: Color32 = Color32::from_rgb(0xdb, 0xe4, 0xf7);
/// Track of a switched-off toggle (design 3e's settings rows).
pub const TOGGLE_OFF: Color32 = Color32::from_rgb(0xe4, 0xe2, 0xe0);

/// Error text. The design keeps red out of the chrome entirely ("red used
/// only where it means something"), so this appears only on actual failures.
pub const ERROR: Color32 = Color32::from_rgb(0xb4, 0x23, 0x18);

// ---------------------------------------------------------------------------
// Global style
// ---------------------------------------------------------------------------

/// Named font family for Archivo SemiBold (the design's 600 weight: buttons,
/// row titles, field emphasis). Use via [`semibold`].
pub const SEMIBOLD: &str = "Archivo-SemiBold";
/// Named font family for Archivo Bold (the design's 700 weight: headings,
/// section labels). Use via [`bold`].
pub const BOLD: &str = "Archivo-Bold";
/// Named font family for Archivo ExtraBold — the design's 800 weight. Used
/// for the wordmark ("Deskwarden" at 25px in the login window, 14px in the
/// vault titlebar) and for the detail pane's item title (2b: `font-size:
/// 22px; font-weight: 800`), which is the design's only other 800.
/// Use via [`extrabold`].
///
/// Bundled as its own face because Archivo's 800 is genuinely a different
/// cut, not a synthesised one: it is both heavier *and* slightly wider per
/// glyph than 700, so rendering the wordmark in Bold read as simultaneously
/// too light and too narrow against the design, and no amount of tracking
/// could reconcile it.
pub const EXTRABOLD: &str = "Archivo-ExtraBold";

/// The face the design's keyboard-shortcut runs actually render in.
///
/// The design declares them (`CTRL+L`, `CTRL+K`, `CTRL+H`, the `/` and `↵`
/// chips) as `font-family: ui-monospace, SFMono-Regular, Menlo, monospace`
/// — i.e. "the platform's own UI monospace". On Windows none of the first
/// three exist, so a browser rendering `Deskwarden.dc.html` falls through
/// to generic `monospace`, which Chromium resolves to Consolas. egui's
/// bundled default for [`FontFamily::Monospace`] is Hack instead: a
/// noticeably heavier and wider face, which is why these chips read as the
/// wrong font against the design even though the *family* was already
/// correct.
///
/// Reading the system copy puts the app on the same face the design
/// document itself renders with here, and costs nothing in binary size.
/// `None` — leaving egui's Hack in place — whenever it can't be read: this
/// is a cosmetic match, never a reason to fail startup.
fn system_monospace() -> Option<Vec<u8>> {
    let system_root = std::env::var_os("SystemRoot")?;
    let path = std::path::Path::new(&system_root)
        .join("Fonts")
        .join("consola.ttf");
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            log::debug!(
                "could not read {} ({e}); keeping egui's bundled monospace face",
                path.display()
            );
            None
        }
    }
}

/// The bundled Archivo faces (the design's typeface, OFL-licensed; see
/// assets/fonts/OFL.txt), layered over egui's defaults.
///
/// egui has no weight axis — `RichText::strong()` only tints — so each
/// weight is registered as its own named family, with egui's default
/// proportional stack kept behind it for glyphs Archivo lacks (arrows,
/// emoji, CJK).
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Archivo-Regular".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        SEMIBOLD.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        BOLD.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-Bold.ttf"
        ))),
    );
    fonts.font_data.insert(
        EXTRABOLD.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Archivo-ExtraBold.ttf"
        ))),
    );

    let default_stack = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    if let Some(proportional) = fonts.families.get_mut(&FontFamily::Proportional) {
        proportional.insert(0, "Archivo-Regular".to_owned());
    }
    for weight in [SEMIBOLD, BOLD, EXTRABOLD] {
        let mut stack = vec![weight.to_owned()];
        stack.extend(default_stack.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(weight.into()), stack);
    }

    // Front of the monospace stack, not a replacement for it -- egui's Hack
    // stays behind as the fallback for anything Consolas lacks, and as the
    // whole family if `system_monospace` came back empty.
    if let Some(bytes) = system_monospace() {
        fonts
            .font_data
            .insert("Consolas".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
        if let Some(monospace) = fonts.families.get_mut(&FontFamily::Monospace) {
            monospace.insert(0, "Consolas".to_owned());
        }
    }

    fonts
}

/// `RichText` in Archivo SemiBold — the design's 600 weight.
pub fn semibold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into())
        .size(size)
        .family(FontFamily::Name(SEMIBOLD.into()))
}

/// `RichText` in Archivo Bold — the design's 700 weight.
pub fn bold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into())
        .size(size)
        .family(FontFamily::Name(BOLD.into()))
}

/// `RichText` in Archivo ExtraBold — the design's 800 weight. See
/// [`EXTRABOLD`] for why this is a bundled face rather than [`bold`].
pub fn extrabold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text.into())
        .size(size)
        .family(FontFamily::Name(EXTRABOLD.into()))
}

/// Letterspaced text, for the design's tracked uppercase tags ("FILLS
/// NATIVE WINDOWS" at 0.15em, the card header wordmark at 0.1em).
/// `RichText` has no tracking control, so this drops down to a `LayoutJob`;
/// `tracking` is in points (design em × font size).
pub fn letterspaced(
    text: &str,
    size: f32,
    family: &str,
    tracking: f32,
    color: Color32,
) -> egui::text::LayoutJob {
    letterspaced_in(
        text,
        FontId::new(size, FontFamily::Name(family.into())),
        tracking,
        color,
        None,
    )
}

/// [`letterspaced`] for the monospace face, which has no *named* family and so
/// cannot be asked for through that function's `&str`.
///
/// The design tracks two monospace runs in the detail pane and nowhere else: a
/// masked value's bullets (`letter-spacing: 0.08em` at 15px) and a live
/// one-time code (`0.12em` at 17px). Both read as one dense blob without it.
pub fn letterspaced_mono(
    text: &str,
    size: f32,
    tracking: f32,
    color: Color32,
) -> egui::text::LayoutJob {
    letterspaced_in(
        text,
        FontId::new(size, FontFamily::Monospace),
        tracking,
        color,
        None,
    )
}

/// The detail pane's item title (design 2b: `font-size: 22px; font-weight:
/// 800; letter-spacing: -0.02em; line-height: 1.1`).
///
/// The tight line height is not decoration: the title sits in a flex row that
/// is `align-items: center` against a 44px avatar, so a title box taller than
/// 44px pushes the avatar off the strip's own 20px top padding and the strip
/// grows with it. egui lays 22px text out at roughly 1.3 line heights by
/// default, which is exactly that case.
pub fn pane_title(text: &str, size: f32, color: Color32) -> egui::text::LayoutJob {
    letterspaced_in(
        text,
        FontId::new(size, FontFamily::Name(EXTRABOLD.into())),
        // `letter-spacing: -0.02em`, in points.
        size * -0.02,
        color,
        Some(size * 1.1),
    )
}

fn letterspaced_in(
    text: &str,
    font_id: FontId,
    tracking: f32,
    color: Color32,
    line_height: Option<f32>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id,
            color,
            extra_letter_spacing: tracking,
            line_height,
            ..Default::default()
        },
    );
    job
}

/// Fills `ui` with the app's window background.
///
/// Every window here skips drawing on its first frame: [`apply`]'s fonts
/// only become live at the *start* of the next one, so laying out
/// Archivo-styled text in the same frame that registers it would look up a
/// family that does not exist yet. Returning early left that frame
/// completely unpainted, which shows eframe's near-black default clear
/// colour — a dark rectangle flashing open at window creation, and with the
/// login, loading and vault windows opening in sequence, three of them. It
/// reads exactly like a console window appearing.
///
/// Painting a plain rect needs no fonts, so it is safe on that first frame
/// and turns the flash into the window's own colour.
pub fn paint_window_background(ui: &Ui) {
    ui.painter()
        .rect_filled(ui.max_rect(), CornerRadius::ZERO, WINDOW_BG);
}

/// Applies the Deskwarden look to an egui context. Call once per window,
/// before the first frame's widgets are laid out (calling every frame is
/// harmless but wasted work).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());

    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();

    // All sizes are whole pixels on purpose: fractional font sizes land
    // glyphs on subpixel boundaries, and egui's greyscale AA renders those
    // visibly softer than the design's browser-hinted text.
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);

    // egui defaults every `ui.label()` to selectable text, which shows a
    // text-beam cursor on hover regardless of whether the label sits inside
    // something clickable -- since labels make up most of this app's
    // surface (row text, field labels, buttons' own text), that read as a
    // text-beam cursor almost everywhere. This app has no text-selection
    // feature anywhere, so there is nothing lost by turning it off; the
    // cursor now stays the OS default arrow over plain text and switches to
    // a hand only over what's actually clickable (see `interact_cursor`
    // below).
    style.interaction.selectable_labels = false;

    let mut v = egui::Visuals::light();
    // The web-like affordance the design implies but egui doesn't apply on
    // its own: every `egui::Button`-based control (which is most of this
    // app's clickables -- `primary_button`/`secondary_button`/
    // `toolbar_button`/plain `egui::Button`) shows a pointing hand on
    // hover. Hand-painted clickables that don't go through `Button` (the
    // window chrome's ✕/▢/— controls, item-list rows, the sidebar's status
    // pill and edit-pencil) set this themselves via `on_hover_cursor`/
    // `set_cursor_icon` at their own call sites instead.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.panel_fill = CANVAS;
    v.window_fill = CARD;
    v.faint_bg_color = CARD_TINT;
    // Text-edit backgrounds: white cards on the warm-grey canvas, per the
    // design's input fields.
    v.extreme_bg_color = CARD;
    v.selection.bg_fill = BLUE_EDGE;
    v.selection.stroke = Stroke::new(1.0, BLUE);
    v.hyperlink_color = BLUE_DEEP;
    v.window_stroke = Stroke::new(1.0, BORDER);

    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.inactive.corner_radius = CornerRadius::same(7);
    v.widgets.hovered.bg_fill = CARD_TINT;
    v.widgets.hovered.weak_bg_fill = CARD_TINT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, BLUE_EDGE);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, INK);
    v.widgets.hovered.corner_radius = CornerRadius::same(7);
    v.widgets.active.bg_fill = BLUE_WASH;
    v.widgets.active.weak_bg_fill = BLUE_WASH;
    v.widgets.active.bg_stroke = Stroke::new(1.0, BLUE);
    v.widgets.active.fg_stroke = Stroke::new(1.0, BLUE_DEEP);
    v.widgets.active.corner_radius = CornerRadius::same(7);
    v.widgets.open.weak_bg_fill = BLUE_WASH;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BLUE);

    style.visuals = v;
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style_of(egui::Theme::Light, style);
}

// ---------------------------------------------------------------------------
// The mark (design 3g: quartered shield — four vaults, one guard)
// ---------------------------------------------------------------------------

/// The four quadrant fills of the full-color mark, in reading order
/// (top-left, top-right, bottom-left, bottom-right).
///
/// Arranged as a *checkerboard* of tone — the two dark values diagonally
/// opposite each other, likewise the two light ones — so that every shared
/// edge in the mark divides a dark quadrant from a light one.
///
/// This is a deliberate divergence from `Deskwarden.dc.html`, which lays
/// the four values out in palette order (deep, blue, bright, soft) and so
/// puts `BLUE_DEEP` and `BLUE` next to each other along the mark's entire
/// top edge. Those two differ by ~15 in relative luminance against ~50-114
/// for every other pairing, so that edge visually disappeared and the mark
/// read as three shapes rather than four. This order raises the *weakest*
/// adjacent contrast in the mark from ~15 to ~50.
///
/// Note this is the one place the module doc's "a mismatch with the design
/// is a bug in this file" does not hold: the mismatch is the fix, and
/// `quadrant_tones_alternate_around_the_mark` locks it in so it cannot be
/// quietly reverted to palette order.
const QUADRANT_FILLS: [Color32; 4] = [BLUE_DEEP, BLUE_BRIGHT, BLUE_SOFT, BLUE];

/// Kappa for approximating a 90° circular arc with one cubic Bézier,
/// pre-multiplied by the design's 2.4-unit corner radius.
const ARC_K: f32 = 0.552_285 * 2.4;

/// Flattens one cubic Bézier into `steps` points (excluding the start point,
/// including the end point), appending to `out`.
fn flatten_cubic(out: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, steps: usize) {
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let x =
            u * u * u * p0.x + 3.0 * u * u * t * p1.x + 3.0 * u * t * t * p2.x + t * t * t * p3.x;
        let y =
            u * u * u * p0.y + 3.0 * u * u * t * p1.y + 3.0 * u * t * t * p2.y + t * t * t * p3.y;
        out.push(Pos2::new(x, y));
    }
}

/// The four quadrant outlines of the mark, in the design's 24×28 SVG
/// coordinate space. Each is convex (a rectangle with one rounded or one
/// elliptically-curved corner), which is what lets `paint_mark` use
/// `Shape::convex_polygon` directly.
///
/// Flattened once and kept, rather than rebuilt per call: `paint_mark` runs
/// on every frame the mark is visible, and the geometry is a compile-time
/// constant in all but name (it just needs float arithmetic a `const` can't
/// do). Callers scale the returned points into screen space themselves, so
/// there is nothing frame-dependent to recompute.
fn quadrant_outlines() -> &'static [Vec<Pos2>; 4] {
    static OUTLINES: OnceLock<[Vec<Pos2>; 4]> = OnceLock::new();
    OUTLINES.get_or_init(build_quadrant_outlines)
}

fn build_quadrant_outlines() -> [Vec<Pos2>; 4] {
    let p = Pos2::new;

    // Top-left: M12 2 H4.4 A2.4 2.4 0 0 0 2 4.4 V14 H12 Z
    let mut tl = vec![p(12.0, 2.0), p(4.4, 2.0)];
    flatten_cubic(
        &mut tl,
        p(4.4, 2.0),
        p(4.4 - ARC_K, 2.0),
        p(2.0, 4.4 - ARC_K),
        p(2.0, 4.4),
        8,
    );
    tl.extend([p(2.0, 14.0), p(12.0, 14.0)]);

    // Top-right: M12 2 h7.6 A2.4 2.4 0 0 1 22 4.4 V14 H12 Z
    let mut tr = vec![p(12.0, 2.0), p(19.6, 2.0)];
    flatten_cubic(
        &mut tr,
        p(19.6, 2.0),
        p(19.6 + ARC_K, 2.0),
        p(22.0, 4.4 - ARC_K),
        p(22.0, 4.4),
        8,
    );
    tr.extend([p(22.0, 14.0), p(12.0, 14.0)]);

    // Bottom-left: M2 14 h10 v12 C6.6 23.2 3.2 19.4 2 14 Z
    let mut bl = vec![p(2.0, 14.0), p(12.0, 14.0), p(12.0, 26.0)];
    flatten_cubic(
        &mut bl,
        p(12.0, 26.0),
        p(6.6, 23.2),
        p(3.2, 19.4),
        p(2.0, 14.0),
        12,
    );

    // Bottom-right: M12 14 h10 c-1.2 5.4 -4.6 9.2 -10 12 Z
    let mut br = vec![p(12.0, 14.0), p(22.0, 14.0)];
    flatten_cubic(
        &mut br,
        p(22.0, 14.0),
        p(20.8, 19.4),
        p(17.4, 23.2),
        p(12.0, 26.0),
        12,
    );

    [tl, tr, bl, br]
}

/// Paints the full-color quartered-shield mark filling `rect` (preserving the
/// 24:28 aspect ratio, centered).
pub fn paint_mark(painter: &egui::Painter, rect: Rect) {
    paint_mark_with(painter, rect, None)
}

/// Like [`paint_mark`], but with every quadrant in a single `tint` — the
/// design's "solid"/"ink" variants for very small or monochrome contexts.
pub fn paint_mark_tinted(painter: &egui::Painter, rect: Rect, tint: Color32) {
    paint_mark_with(painter, rect, Some(tint))
}

fn paint_mark_with(painter: &egui::Painter, rect: Rect, tint: Option<Color32>) {
    let scale = (rect.width() / 24.0).min(rect.height() / 28.0);
    let origin = rect.center() - Vec2::new(12.0 * scale, 14.0 * scale);
    for (outline, fill) in quadrant_outlines().iter().zip(QUADRANT_FILLS) {
        let points: Vec<Pos2> = outline
            .iter()
            .map(|p| origin + Vec2::new(p.x * scale, p.y * scale))
            .collect();
        painter.add(egui::Shape::convex_polygon(
            points,
            tint.unwrap_or(fill),
            Stroke::NONE,
        ));
    }
}

/// Where the shield's *ink* lands when the mark is painted into `rect`.
///
/// The artboard is 24×28 but the shield only spans 2..22 horizontally and
/// 2..26 vertically — two units of padding on every side (see
/// `quadrant_outlines_stay_inside_the_design_viewbox`). [`paint_mark`] fits
/// the whole artboard into `rect`, so the shield's visible left edge sits
/// inset from `rect.left()`, and a mark box flush against a text column
/// looks *indented* relative to it.
///
/// Callers that need the shield optically aligned to something — rather
/// than its artboard mathematically aligned — use this to compensate.
pub fn mark_ink_rect(rect: Rect) -> Rect {
    let scale = (rect.width() / 24.0).min(rect.height() / 28.0);
    let origin = rect.center() - Vec2::new(12.0 * scale, 14.0 * scale);
    Rect::from_min_max(
        origin + Vec2::new(2.0 * scale, 2.0 * scale),
        origin + Vec2::new(22.0 * scale, 26.0 * scale),
    )
}

/// How far a laid-out run's first visible ink sits from its galley origin.
///
/// egui (like a browser) positions text by its layout origin, but every
/// glyph carries its own left side bearing, and that bearing scales with
/// the font size. So two runs painted at the same x in *different* sizes do
/// not have their visible left edges aligned — at 25px and 10px the gap is
/// a full pixel, which is plainly visible when one sits directly above the
/// other. Painting each at `x - ink_offset_x(galley)` aligns the ink rather
/// than the origins.
pub fn ink_offset_x(galley: &egui::Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map(|glyph| glyph.pos.x + glyph.uv_rect.offset.x)
        .unwrap_or(0.0)
}

/// Allocates a `size`×`size` square and paints the mark into it.
pub fn mark(ui: &mut Ui, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_mark(ui.painter(), rect);
    response
}

/// True if `p` is inside the convex polygon `poly` (used for rasterizing the
/// mark's quadrants, which are all convex — see `quadrant_outlines`).
fn inside_convex(poly: &[Pos2], p: Pos2) -> bool {
    let mut sign = 0.0f32;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross.abs() < 1e-4 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if sign != cross.signum() {
            return false;
        }
    }
    true
}

/// The mark rasterized as an OS window icon (titlebar + taskbar), per the
/// design's window mocks (3h shows the mark in the titlebar). 32px with 4×
/// supersampling, same approach as `assets/generate-icon.py` — this one is
/// generated at runtime because eframe windows take an `egui::IconData`, not
/// the .ico resource `build.rs` embeds for the exe itself.
pub fn window_icon() -> egui::IconData {
    const SIZE: usize = 32;
    const SS: usize = 4;
    let outlines = quadrant_outlines();

    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    // Map the sample into the 24×28 viewbox, centered in the
                    // square (the viewbox is taller than wide by 4 units).
                    let nx = (px * SS + sx) as f32 + 0.5;
                    let ny = (py * SS + sy) as f32 + 0.5;
                    let p = Pos2::new(
                        nx / (SIZE * SS) as f32 * 28.0 - 2.0,
                        ny / (SIZE * SS) as f32 * 28.0,
                    );
                    if let Some(idx) = outlines.iter().position(|o| inside_convex(o, p)) {
                        let c = QUADRANT_FILLS[idx];
                        r += c.r() as u32;
                        g += c.g() as u32;
                        b += c.b() as u32;
                        a += 255;
                    }
                }
            }
            if a == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // Premultiplied accumulation, unpremultiplied on the way out,
                // so transparent samples don't drag edges towards black.
                let samples = a / 255;
                rgba.extend_from_slice(&[
                    (r / samples) as u8,
                    (g / samples) as u8,
                    (b / samples) as u8,
                    (a / (SS * SS) as u32) as u8,
                ]);
            }
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

// ---------------------------------------------------------------------------
// Composite widgets
// ---------------------------------------------------------------------------

/// Two-letter initials for an avatar tile: first letter of the first two
/// alphanumeric words, or the first two letters of a single word. Splitting
/// on *every* non-alphanumeric boundary (not just whitespace) is what keeps
/// usernames and executables presentable — "a.novak@ledgerline.com" is "AN",
/// not "A.". Deterministic on purpose — the design's hand-picked monograms
/// ("LG" for Ledgerline) are a designer's judgment call this code can't
/// reproduce.
pub fn initials(name: &str) -> String {
    let mut words = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty());
    match (words.next(), words.next()) {
        (Some(first), Some(second)) => {
            let mut s = String::new();
            s.extend(first.chars().next());
            s.extend(second.chars().next());
            s.to_uppercase()
        }
        (Some(only), None) => only.chars().take(2).collect::<String>().to_uppercase(),
        _ => "?".to_string(),
    }
}

/// How far a favicon is inset inside its [`avatar_tile`], per side.
///
/// A JUDGEMENT CALL: the design document has no favicon example anywhere, so
/// there is no value to read off it. It is set against the MONOGRAM, which is
/// the thing a favicon sits beside in a list and has to weigh the same as.
/// [`avatar`] draws its letters at `size * 0.38` -- ~12pt of ink centred in a
/// 32pt tile -- so a monogram's ink covers roughly a third of the tile, while
/// an edge-to-edge favicon covers all of it. 4pt a side puts the artwork in a
/// 24pt box: still clearly the largest thing in the row (a favicon is detail,
/// not a letterform, and shrinking it further starts costing legibility) but
/// no longer heavier than every monogram next to it. That was the report.
///
/// NOTE FOR WHOEVER CHANGES EITHER SIDE OF THIS: `favicon::decode_rgba`
/// resamples every icon to a 64px longest edge, a number chosen for a 32pt
/// draw at 200% scaling. Nothing in the code links that constant to this one.
/// 64 still covers a 24pt draw comfortably (48 physical px at 200%), so this
/// inset is safe, but a tile that ever grows past 32pt needs `decode_rgba`'s
/// constant raised with it.
pub const AVATAR_ICON_INSET: f32 = 4.0;

/// The avatar tile's BOX -- allocated, filled, bordered and rounded -- with
/// nothing drawn in it, returning the rect so the caller can place its own
/// content inside.
///
/// Split out of [`avatar`] so a favicon can be drawn into the very same box
/// the monogram fallback draws, rather than replacing the box entirely: a
/// bare full-bleed image in place of a bordered tile is what made favicons
/// read as bigger and heavier than the monograms beside them.
pub fn avatar_tile(ui: &mut Ui, size: f32, emphasized: bool) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let (bg, border) = if emphasized {
        (BLUE_WASH, BLUE_EDGE)
    } else {
        (CANVAS, HAIRLINE)
    };
    let rounding = avatar_corner_radius(size);
    ui.painter().rect_filled(rect, rounding, bg);
    ui.painter()
        .rect_stroke(rect, rounding, Stroke::new(1.0, border), StrokeKind::Middle);
    rect
}

/// The avatar tile's `border-radius: 8px` at the design's 32px size, as a
/// ratio so it stays right at any size the tile is drawn at.
pub fn avatar_corner_radius(size: f32) -> CornerRadius {
    CornerRadius::same((size * 0.25) as u8)
}

/// A rounded initials tile, `size` square. `emphasized` renders the selected
/// treatment (blue on a blue wash) versus the neutral grey one.
pub fn avatar(ui: &mut Ui, text: &str, size: f32, emphasized: bool) {
    let rect = avatar_tile(ui, size, emphasized);
    let fg = if emphasized { BLUE } else { TEXT_MUTED };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::new((size * 0.38).round(), FontFamily::Name(SEMIBOLD.into())),
        fg,
    );
}

/// A small status pill: a bordered, fully-rounded pill (design 2b's exact
/// "● Synced 1 min ago" toolbar readout -- `height: 28px; padding: 0 10px;
/// border: 1px solid #eae7e7; border-radius: 999px; font-size: 12px; color:
/// #444141`) with a colored dot plus text. Written generically -- nothing
/// here is vault-window-specific -- so any future status readout
/// (connection state, background job progress, ...) can reuse it instead of
/// hand-rolling another dot+label pairing.
///
/// `dot_color` is the only thing that varies per status; the pill's own
/// border/background/text color stay fixed to the design regardless of
/// state (the design only ever shows the dot itself changing meaning --
/// blue for synced, this app's error red for failed, a ghost tone while in
/// flight).
pub fn status_pill(ui: &mut Ui, dot_color: Color32, text: &str) {
    status_pill_impl(ui, dot_color, text, Sense::hover());
}

/// [`status_pill`], but clickable: the vault window's toolbar merges the
/// "Sync" action into its own status readout (design 2b's "● Synced 1 min
/// ago") instead of keeping them as two separate controls next to each
/// other -- the whole pill is the sync button, and what it reads is also
/// its own result. Darkens the border and swaps in a pointer cursor on
/// hover, the same affordance-on-hover treatment [`hello_panel`] and
/// [`close_glyph`] already use for text/shape-only clickables.
pub fn status_pill_button(ui: &mut Ui, dot_color: Color32, text: &str) -> Response {
    status_pill_impl(ui, dot_color, text, Sense::click())
}

fn status_pill_impl(ui: &mut Ui, dot_color: Color32, text: &str, sense: Sense) -> Response {
    const HEIGHT: f32 = 28.0;
    const PAD_X: f32 = 10.0;
    const GAP: f32 = 6.0;
    const DOT_DIAMETER: f32 = 7.0;

    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        FontId::new(12.0, FontFamily::Proportional),
        TEXT_SECONDARY,
    );
    let content_width = DOT_DIAMETER + GAP + galley.size().x;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(content_width + PAD_X * 2.0, HEIGHT), sense);

    if response.hovered() && sense == Sense::click() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // `border-radius: 999px` on a fixed-height pill is shorthand for "fully
    // rounded" -- half the height is the largest radius that still reads as
    // a stadium shape rather than clipping the corners.
    let rounding = CornerRadius::same((HEIGHT / 2.0) as u8);
    let border = if response.hovered() { BORDER_STRONG } else { HAIRLINE };
    ui.painter()
        .rect_stroke(rect, rounding, Stroke::new(1.0, border), StrokeKind::Inside);

    let dot_center = Pos2::new(rect.min.x + PAD_X + DOT_DIAMETER / 2.0, rect.center().y);
    ui.painter()
        .circle_filled(dot_center, DOT_DIAMETER / 2.0, dot_color);

    let text_pos = Pos2::new(
        dot_center.x + DOT_DIAMETER / 2.0 + GAP,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(text_pos, galley, TEXT_SECONDARY);
    response
}

/// Height of the design's keyboard-hint chips: a 10px monospace line
/// (~12px line box) inside 3px of vertical padding.
///
/// Fixed rather than derived from the galley's own height, which is ascent
/// + descent — it reserves room for descenders that strings like "CTRL+H",
/// "CTRL+N" and "Enter" never contain, so a galley-sized chip always came
/// out taller than the design's. That gap widened when the monospace face
/// became Consolas (see `system_monospace`), whose descent is deeper than
/// the previously-used bundled face.
const CHIP_HEIGHT: f32 = 18.0;

/// Paints one keyboard-hint chip: `text` in 10px monospace, centered in a
/// rounded box of exactly [`CHIP_HEIGHT`] with `pad_x` either side.
fn paint_chip(ui: &mut Ui, text: &str, bg: Color32, fg: Color32, radius: u8, pad_x: f32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::new(10.0, FontFamily::Monospace), fg);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + pad_x * 2.0, CHIP_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(radius), bg);
    let pos = Pos2::new(
        rect.min.x + pad_x,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, fg);
}

/// A small monospace keyboard-hint chip ("↵", "CTRL+N"). `on_primary` is the
/// white-on-blue treatment used inside primary buttons and selected rows.
pub fn kbd_chip(ui: &mut Ui, text: &str, on_primary: bool) {
    let (bg, fg) = if on_primary {
        (BLUE, Color32::WHITE)
    } else {
        (CANVAS, TEXT_FAINT)
    };
    paint_chip(ui, text, bg, fg, 4, 6.0);
}

/// The Windows Hello panel's CTRL+H chip (design 3h: `font-size: 10px;
/// color: #1b3fa0; background: #ffffff; border-radius: 5px; padding: 3px
/// 7px`) — a white chip on the panel's blue wash, which is neither of
/// [`kbd_chip`]'s two treatments.
pub fn kbd_chip_on_card(ui: &mut Ui, text: &str) {
    paint_chip(ui, text, CARD, BLUE, 5, 7.0);
}

/// Height of the design's action buttons (3h Continue, 2b/3f toolbar).
/// Named because things placed *beside* a button — the login window's
/// in-flight spinner — have to match it, and a second hardcoded `32.0`
/// could drift away from this one unnoticed.
pub const BUTTON_HEIGHT: f32 = 32.0;

/// The filled primary action button, optionally with a trailing keyboard
/// hint, per the design's "Save ↵" / "Fill in app CTRL+⇧+F" buttons.
///
/// A `kbd` of `"↵"` is painted as a vector return-arrow rather than typed:
/// neither Archivo nor egui's fallback fonts carry U+21B5, so as text it
/// renders as a tofu box.
pub fn primary_button(ui: &mut Ui, label: &str, kbd: Option<&str>) -> Response {
    primary_button_with_metrics(ui, label, kbd, BUTTON_HEIGHT, 7)
}

/// Design 2b's item-pane `+ New`, which is the only primary button in the app
/// that is NOT [`BUTTON_HEIGHT`]: `height: 34px; border-radius: 8px`, matching
/// the search box it sits beside rather than the 3h/3f action buttons.
///
/// A parameterised variant rather than a second copy of the body, and rather
/// than changing [`primary_button`]'s own constants: every other primary
/// button in this app is 32px with a 7px radius (3h's Continue, the detail
/// pane's Save and "Fill in app"), and moving them all to match one button in
/// one pane would be a redesign of five screens to fix one.
pub fn primary_button_matching_field(ui: &mut Ui, label: &str) -> Response {
    primary_button_with_metrics(ui, label, None, SEARCH_FIELD_HEIGHT, 8)
}

fn primary_button_with_metrics(
    ui: &mut Ui,
    label: &str,
    kbd: Option<&str>,
    height: f32,
    radius: u8,
) -> Response {
    let paint_return = kbd == Some("↵");
    let text = match kbd {
        // Trailing spaces reserve room for the painted arrow and the gap
        // before it.
        Some("↵") => format!("{label}      "),
        Some(k) => format!("{label}  {k}"),
        None => label.to_string(),
    };
    let response = ui.add(
        egui::Button::new(semibold(text, 13.0).color(Color32::WHITE))
            .fill(BLUE)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(radius))
            // The design's action buttons are 32px tall (3h Continue, 2b/3f
            // toolbar); text + padding alone comes up short.
            .min_size(Vec2::new(0.0, height)),
    );
    if paint_return {
        paint_return_arrow(
            ui.painter(),
            Pos2::new(response.rect.right() - 17.0, response.rect.center().y),
            RETURN_GLYPH_SIZE,
            Color32::from_white_alpha(204),
        );
    }
    response
}

/// Extent of the drawn ↵ glyph. The design sets it in 10px monospace beside
/// a 13px label (`opacity: 0.8`, which is the 204 alpha above); a real 10px
/// ↵ glyph's ink is roughly half its em box, so the arrow is drawn at 5px
/// rather than the 6.5 it used before, which read as heavier than the
/// design's at the same nominal size.
const RETURN_GLYPH_SIZE: f32 = 5.0;

/// The ↵ return glyph, drawn: down the right side, along the bottom, arrowhead
/// pointing left. Every part scales with `size` -- the arrowhead barbs used
/// to be a fixed 3.2px, so shrinking the glyph left them oversized.
fn paint_return_arrow(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let half = size / 2.0;
    let barb = size * 0.5;
    let right_top = Pos2::new(center.x + half, center.y - half);
    let corner = Pos2::new(center.x + half, center.y + half * 0.7);
    let left = Pos2::new(center.x - half, center.y + half * 0.7);
    painter.line_segment([right_top, corner], stroke);
    painter.line_segment([corner, left], stroke);
    painter.line_segment([left, Pos2::new(left.x + barb, left.y - barb)], stroke);
    painter.line_segment([left, Pos2::new(left.x + barb, left.y + barb)], stroke);
}

/// The outlined secondary button ("Not now", "Cancel", "Copy").
pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        egui::Button::new(semibold(label, 13.0).color(INK))
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER_STRONG))
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, BUTTON_HEIGHT)),
    )
}

/// Height of the detail pane's header-strip controls (design 2b: `height:
/// 34px` on both "Fill in app" and "Edit").
///
/// Not [`BUTTON_HEIGHT`], and deliberately not [`SEARCH_FIELD_HEIGHT`] either,
/// which happens to be the same 34 for an unrelated reason (2b's `+ New` has
/// to line up with the search box beside it). Two things the same size today
/// for different reasons are two constants; folding them together is how one
/// silently follows the other when the design moves.
///
/// **Also the WIDTH of the square controls.** [`star_toggle`] and
/// [`kebab_button`] both allocate `splat(HEADER_BUTTON_HEIGHT)`, so this one
/// number is how much room each of them takes on the strip -- which
/// `detail.rs`'s `header_layout` has to know *before* it draws anything in
/// order to decide whether the strip fits on one line. It is `pub` for that
/// reader and no other.
pub const HEADER_BUTTON_HEIGHT: f32 = 34.0;

// The outlined 34px header button that used to stand beside the primary
// (design 2b's "Edit") is gone with the words it carried: Edit and Delete
// moved into `star_toggle`/`kebab_button`'s menu at the user's direction,
// and nothing else in the app is 34px-outlined. It is deleted rather than
// left `pub` and unused -- a lib crate raises no dead-code warning for it,
// so it would have sat here indefinitely with a doc comment describing a
// control that no longer exists.

/// The detail pane's filled header button (design 2b's "Fill in app":
/// `height: 34px; padding: 0 14px; border-radius: 8px; background: #1b3fa0;
/// color: #ffffff; font-size: 13px; font-weight: 600`) with its shortcut hint
/// nested inside the same pill at `font-size: 10px; opacity: 0.85`.
///
/// [`primary_button`] renders a hint by *appending it to the label*, so it
/// comes out at the label's own 13px in the label's own weight -- the design
/// draws a distinctly smaller, softer monospace run. Same two-galley
/// construction as [`toolbar_button_with_shortcut`], which had the identical
/// requirement one pane over.
///
/// `shortcut` is an `Option` because the hint is the first thing this control
/// gives up when the strip it sits on runs out of room: on a narrow pane the
/// twelve monospace characters of "CTRL+SHIFT+F" are wider than the label they
/// annotate, and the chord keeps working whether or not they are painted. See
/// `detail.rs`'s `header_layout`, which is what decides.
pub fn header_primary_button(ui: &mut Ui, label: &str, shortcut: Option<&str>) -> Response {
    let (label_galley, hint_galley) = header_primary_galleys(ui, label, shortcut);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(
            header_primary_width_of(&label_galley, hint_galley.as_ref()),
            HEADER_BUTTON_HEIGHT,
        ),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(8), BLUE);

    let label_pos = Pos2::new(
        rect.min.x + HEADER_PRIMARY_PAD_X,
        rect.center().y - label_galley.size().y / 2.0,
    );
    let label_width = label_galley.size().x;
    ui.painter().galley(label_pos, label_galley, Color32::WHITE);
    if let Some(hint_galley) = hint_galley {
        let hint_pos = Pos2::new(
            label_pos.x + label_width + HEADER_PRIMARY_GAP,
            rect.center().y - hint_galley.size().y / 2.0,
        );
        ui.painter()
            .galley(hint_pos, hint_galley, HEADER_PRIMARY_HINT);
    }
    response
}

const HEADER_PRIMARY_PAD_X: f32 = 14.0;
const HEADER_PRIMARY_GAP: f32 = 8.0;
/// The hint's `opacity: 0.85`, over the blue fill -- `from_white_alpha(217)`,
/// spelled out because that constructor is not `const`.
const HEADER_PRIMARY_HINT: Color32 = Color32::from_rgba_premultiplied(217, 217, 217, 217);

/// [`header_primary_button`]'s two runs, laid out but not painted.
///
/// Shared with [`header_primary_button_width`] rather than measured twice: a
/// caller that reserves room for this pill and a pill that then allocates a
/// different width is exactly the drift that put "Fill in app" off the left
/// edge of the pane in the first place.
fn header_primary_galleys(
    ui: &Ui,
    label: &str,
    shortcut: Option<&str>,
) -> (Arc<egui::Galley>, Option<Arc<egui::Galley>>) {
    let label_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::new(13.0, FontFamily::Name(SEMIBOLD.into())),
        Color32::WHITE,
    );
    let hint_galley = shortcut.map(|shortcut| {
        ui.painter().layout_no_wrap(
            shortcut.to_string(),
            FontId::new(10.0, FontFamily::Monospace),
            HEADER_PRIMARY_HINT,
        )
    });
    (label_galley, hint_galley)
}

fn header_primary_width_of(label: &Arc<egui::Galley>, hint: Option<&Arc<egui::Galley>>) -> f32 {
    label.size().x
        + hint.map_or(0.0, |hint| HEADER_PRIMARY_GAP + hint.size().x)
        + HEADER_PRIMARY_PAD_X * 2.0
}

/// Exactly what [`header_primary_button`] would allocate, without drawing it
/// -- so a caller can find out whether it has room for the pill *before*
/// committing to a layout that has to hold it.
pub fn header_primary_button_width(ui: &Ui, label: &str, shortcut: Option<&str>) -> f32 {
    let (label_galley, hint_galley) = header_primary_galleys(ui, label, shortcut);
    header_primary_width_of(&label_galley, hint_galley.as_ref())
}

/// The small outlined control at the right-hand end of a detail-pane row
/// (design 2b's "Copy" / "Reveal" / "Open": `height: 28px; padding: 0 10px;
/// border: 1px solid #d7d3d3; border-radius: 7px; font-size: 12px`).
///
/// Regular weight, not [`semibold`]: the design gives these no `font-weight`,
/// unlike the 600 it sets explicitly on the header pair.
pub fn row_button(ui: &mut Ui, label: &str) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
        ui.add(
            egui::Button::new(RichText::new(label).size(12.0).color(INK))
                .fill(CARD)
                .stroke(Stroke::new(1.0, BORDER_STRONG))
                .corner_radius(CornerRadius::same(7))
                .min_size(Vec2::new(0.0, 28.0)),
        )
    })
    .inner
}

/// A URL drawn as the link it is: the value text itself in [`BLUE`], with the
/// pointing hand under it, reporting its own clicks.
///
/// **The text is the control, replacing a button beside it.** Design 2b draws
/// the detail pane's Website row as plain 14px ink with a separate "Open"
/// [`row_button`], which is what this app shipped; the user asked for the URL
/// to be blue and clickable instead, so the button goes and the run of text
/// takes its job.
///
/// **No underline, and that is the design's answer rather than an omission.**
/// 2b paints no link anywhere -- no anchor, no blue body text, no underline,
/// nothing in the whole block to copy a hover treatment from. (The only
/// `text-decoration` in `Deskwarden.dc.html` is the specimen document's own
/// chrome, styling the prose links around the mockups, and it sets
/// `text-decoration: none`.) So the affordance is the two things this app
/// already spells everywhere for a hand-painted clickable: its own colour,
/// and the pointing hand. Inventing a hover underline would be inventing.
///
/// `selectable(false)` because egui's default text selection would take the
/// press for a drag-select and the row would stop reporting clicks at all.
pub fn link_label(ui: &mut Ui, text: &str, size: f32) -> Response {
    let response = ui.add(
        egui::Label::new(RichText::new(text).size(size).color(BLUE))
            .selectable(false)
            .sense(Sense::click()),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// The vault window titlebar's Lock control (design 2b: `height: 28px;
/// padding: 0 12px; border: 1px solid #d7d3d3; border-radius: 8px;`), with
/// its keyboard shortcut nested *inside* the same bordered pill -- "Lock"
/// in 12px SemiBold ink, then its shortcut in 10px monospace faint text, an
/// 8px gap apart -- rather than [`secondary_button`] (close but not exact:
/// 32px tall, 7px radius, 13px text) plus a separate [`kbd_chip`] floating
/// beside it. The design's markup is one element containing both text
/// runs, not two adjacent ones, and clicking anywhere in the pill --
/// including over the shortcut text -- activates it.
pub fn toolbar_button_with_shortcut(ui: &mut Ui, label: &str, shortcut: &str) -> Response {
    const PAD_X: f32 = 12.0;
    const GAP: f32 = 8.0;
    const HEIGHT: f32 = 28.0;

    let label_galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), FontId::new(12.0, FontFamily::Name(SEMIBOLD.into())), INK);
    let shortcut_galley =
        ui.painter()
            .layout_no_wrap(shortcut.to_string(), FontId::new(10.0, FontFamily::Monospace), TEXT_FAINT);

    let content_width = label_galley.size().x + GAP + shortcut_galley.size().x;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(content_width + PAD_X * 2.0, HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let rounding = CornerRadius::same(8);
    ui.painter()
        .rect_filled(rect, rounding, if response.hovered() { CARD_TINT } else { CARD });
    ui.painter()
        .rect_stroke(rect, rounding, Stroke::new(1.0, BORDER_STRONG), StrokeKind::Inside);

    let label_pos = Pos2::new(rect.min.x + PAD_X, rect.center().y - label_galley.size().y / 2.0);
    let shortcut_pos = Pos2::new(
        label_pos.x + label_galley.size().x + GAP,
        rect.center().y - shortcut_galley.size().y / 2.0,
    );
    ui.painter().galley(label_pos, label_galley, INK);
    ui.painter().galley(shortcut_pos, shortcut_galley, TEXT_FAINT);

    response
}

/// The overlay/card header bar: 16px mark, letterspaced "DESKWARDEN", and a
/// right-aligned status ("3 matches", the app name).
pub fn card_header(ui: &mut Ui, right_text: &str) {
    card_header_inner(ui, right_text, false);
}

/// [`card_header`] with a dismiss ✕ at the far right, as the design's card
/// headers carry (3c: ghost-grey glyph, right-aligned in the header rule).
/// Returns true on the frame it is clicked.
///
/// Needed by any window that has no title bar of its own: the overlay is
/// `with_decorations(false)`, so without this the only way out for a
/// mouse-only user is Alt+F4 — and the keyboard route can't be relied on,
/// because Windows' foreground lock can hand an always-on-top window that was
/// raised in response to *another* app's activity no keyboard focus at all.
pub fn card_header_with_close(ui: &mut Ui, right_text: &str) -> bool {
    card_header_inner(ui, right_text, true)
}

fn card_header_inner(ui: &mut Ui, right_text: &str, with_close: bool) -> bool {
    let mut dismissed = false;
    ui.horizontal(|ui| {
        mark(ui, 16.0);
        // Real tracking (0.1em at 11px), not spaces between letters.
        ui.label(letterspaced("DESKWARDEN", 11.0, BOLD, 1.1, TEXT_SECONDARY));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if with_close {
                dismissed = close_glyph(ui).clicked();
                ui.add_space(2.0);
            }
            ui.label(RichText::new(right_text).size(11.0).color(TEXT_GHOST));
        });
    });
    dismissed
}

/// The dismiss ✕ itself: a 16px hit target with the design's ghost-grey
/// glyph, darkening to ink on hover so it reads as clickable despite having
/// no button chrome (the design draws it as bare text).
///
/// Stroked as two crossing lines rather than drawn as the character U+2715:
/// neither the bundled Archivo faces nor egui's fallback stack carry that
/// codepoint, so as text it renders as a tofu box. Two strokes are also
/// sharper at this size than any glyph would be.
fn close_glyph(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    let color = if response.hovered() { INK } else { TEXT_GHOST };
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let stroke = Stroke::new(1.3, color);
    let arm = 3.5;
    let c = rect.center();
    let painter = ui.painter();
    painter.line_segment([c + Vec2::new(-arm, -arm), c + Vec2::new(arm, arm)], stroke);
    painter.line_segment([c + Vec2::new(arm, -arm), c + Vec2::new(-arm, arm)], stroke);
    response.on_hover_text("Dismiss")
}

// ---------------------------------------------------------------------------
// The detail pane's drawn icons: the favourite star, the kebab that carries
// Edit and Delete, and the reveal eye on every masked row.
//
// STROKES, NOT GLYPHS -- measured rather than assumed, and the measurement
// is not the answer that was assumed.
// `the_icon_codepoints_are_not_carried_by_this_apps_own_typeface` asks the
// resolved font stack for each of them: ⋮ (U+22EE) resolves to nothing,
// exactly like the U+2715 `close_glyph` records, so the kebab could only
// ever have been drawn. ★ ☆ 👁 DO resolve -- out of egui's bundled
// emoji/icon fallback behind Archivo, not out of this app's own face, which
// that test pins by the one advance width all three share. They are drawn
// anyway: a mark from a fallback nobody here chose brings its own weight,
// optical size and baseline next to controls measured from the design, and
// ★/☆ are two unrelated marks where the on/off pair has to be one
// silhouette in two weights.
//
// A shape paints no galley, which is the other half of the cost: the
// headless tests that used to find these controls by their label ("Reveal",
// "Favourite", "Edit") now find them by their geometry. That lookup is
// [`icon_probe`], and it lives HERE, next to the code that draws them, so a
// retuned shape cannot leave a stale copy of its vertex count in another
// module's test.
// ---------------------------------------------------------------------------

/// Vertices in the star's outline: five points and five valleys.
pub const STAR_VERTICES: usize = 10;

/// Samples along the eye's upper lid.
const EYE_LID_SEGMENTS: usize = 12;

/// Vertices in the eye's almond outline: the upper lid sampled
/// `EYE_LID_SEGMENTS + 1` times, and the lower lid the `EYE_LID_SEGMENTS - 1`
/// times strictly between the two corners it shares with it.
pub const EYE_VERTICES: usize = EYE_LID_SEGMENTS * 2;

/// The kebab's dot radius. Also what tells its three circles apart from the
/// eye's pupil, which is deliberately a different size.
pub const KEBAB_DOT_RADIUS: f32 = 1.7;

/// The eye's pupil radius -- not [`KEBAB_DOT_RADIUS`], see there.
const EYE_PUPIL_RADIUS: f32 = 2.4;

/// Teeth around the settings gear. Eight is the fewest that still reads as a
/// cog rather than a flower at 28px, and it keeps the outline's vertex count
/// clear of every other drawn icon here (see [`GEAR_VERTICES`]).
const GEAR_TEETH: usize = 8;

/// Vertices in the gear's outline: four per tooth (rise, crown-out,
/// crown-in, fall), so 32.
///
/// Deliberately distinct from [`STAR_VERTICES`] (10) and [`EYE_VERTICES`]
/// (24) -- [`icon_probe`] tells these icons apart by vertex count alone, so
/// two of them sharing one count would make each findable as the other.
pub const GEAR_VERTICES: usize = GEAR_TEETH * 4;

/// The gear's hub. Not [`KEBAB_DOT_RADIUS`] (1.7) and not
/// [`EYE_PUPIL_RADIUS`] (2.4), for the same reason those two differ from
/// each other: `icon_probe::kebab_dots` finds circles BY radius, and a hub
/// that matched would be reported as a stray kebab dot in every frame the
/// titlebar is painted in.
const GEAR_HUB_RADIUS: f32 = 3.2;

/// The five-pointed star's outline, starting at the top point.
///
/// Built around the origin and then translated so its own BOUNDING BOX --
/// not the circle its points lie on -- is centred on `center`, exactly as
/// [`pencil_glyph_at`] does and for the same reason: a pentagram has one
/// point above and two below, so its extent is 9 up and 7.3 down, and
/// anchoring by the circle's centre leaves it sitting visibly high in a
/// square hit target.
fn star_outline(center: Pos2, outer: f32) -> Vec<Pos2> {
    // Deliberately NOT 1/φ² (0.382), the regular pentagram's valley-to-point
    // ratio. That is the geometrically pure star, and at 18px it reads as
    // thin and dated -- the points are long spikes with very little body.
    // 0.50 fills them out while keeping the tips sharp, which is the whole
    // point of choosing it over a rounded-join treatment: fatter, not softer.
    //
    // The old comment here claimed anything larger than 0.382 "reads as a
    // flower". That is only true much further up -- a star does not round off
    // into a flower until the valleys are shallow enough to lose the tips,
    // which is well past 0.5. Recorded because the claim was the reason the
    // ratio went unquestioned.
    let inner = outer * 0.50;
    let local: Vec<Vec2> = (0..STAR_VERTICES)
        .map(|i| {
            let radius = if i % 2 == 0 { outer } else { inner };
            // -90° so a POINT is at the top, not a valley.
            let angle = -std::f32::consts::FRAC_PI_2
                + i as f32 * std::f32::consts::TAU / STAR_VERTICES as f32;
            Vec2::new(radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    let top = local.iter().fold(f32::INFINITY, |a, p| a.min(p.y));
    let bottom = local.iter().fold(f32::NEG_INFINITY, |a, p| a.max(p.y));
    let offset = Vec2::new(0.0, -(top + bottom) / 2.0);
    local.into_iter().map(|p| center + p + offset).collect()
}

/// Paints the star at `center`, filled or outlined, in one colour.
/// Stroke width for the favourite star, in both states.
///
/// This is a shape control, not a line weight: see the comment in
/// [`paint_star`] for why the width is what blunts the points. 2.2 at an
/// outer radius of 9 is roughly a quarter of the tip's own length, which is
/// where the mark stops reading as spiky without losing the five points.
const STAR_STROKE: f32 = 2.2;

fn paint_star(ui: &Ui, center: Pos2, outer: f32, filled: bool, color: Color32) {
    let points = star_outline(center, outer);
    let painter = ui.painter();
    if filled {
        // A pentagram is CONCAVE, so `convex_polygon` over its ten vertices
        // would tessellate to garbage. It is star-shaped about its own
        // centre, though, so a triangle fan from there is exact -- and every
        // triangle in it is convex. The apex is the mean of the outline's
        // own vertices, NOT `center`, which `star_outline` has offset away
        // from the star's geometric middle.
        let apex = points
            .iter()
            .fold(Vec2::ZERO, |a, p| a + p.to_vec2())
            .to_pos2()
            / points.len() as f32;
        for i in 0..points.len() {
            painter.add(egui::Shape::convex_polygon(
                vec![apex, points[i], points[(i + 1) % points.len()]],
                color,
                Stroke::NONE,
            ));
        }
    }
    // Always, in both states: outlined it IS the star, and filled it covers
    // the hairline seams anti-aliasing leaves between adjacent fan triangles.
    // It is also what [`icon_probe::stars`] finds, so both states are equally
    // visible to a test.
    //
    // **The width is what rounds the tips**, and it is deliberately heavy.
    // egui's `Stroke` exposes no join style, so there is no `linejoin: round`
    // to ask for; what it does instead is clamp the miter length at sharp
    // corners, which bevels them. At a hairline that bevel is invisible and
    // the star reads as five spikes. At [`STAR_STROKE`] the bevel is a
    // meaningful fraction of the tip, so the points blunt and the whole mark
    // fattens -- the same thing a round join would do here, arrived at
    // through the one control this toolkit gives.
    //
    // It applies to both states so the filled and outlined stars are the
    // same silhouette. A thinner stroke under the fill would leave the "on"
    // star visibly pointier than the "off" one, which reads as two icons.
    painter.add(egui::Shape::closed_line(points, Stroke::new(STAR_STROKE, color)));
}

/// The detail header's favourite control: a star, filled in the design's
/// primary blue when the item IS a favourite and outlined when it is not.
///
/// Square at the strip's own control height, so its hit target matches the
/// 34px buttons beside it rather than being only as big as the mark.
pub fn star_toggle(ui: &mut Ui, on: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // BLUE is this palette's "on"; ERROR is reserved for failures, so the on
    // state cannot borrow it. Same rule the old worded button followed.
    let color = if on {
        BLUE
    } else if response.hovered() {
        INK
    } else {
        TEXT_FAINT
    };
    paint_star(ui, rect.center(), 9.0, on, color);
    response
}

/// The detail header's overflow control: three dots stacked vertically, the
/// menu affordance every desktop app spells the same way.
///
/// `armed` turns them [`ERROR`] red: the Delete inside this menu keeps its
/// two-click confirmation, and once the first click has armed it the menu
/// may well be closed -- so the state has to be legible on the button that
/// opens it, not only on the entry inside.
pub fn kebab_button(ui: &mut Ui, armed: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(HEADER_BUTTON_HEIGHT), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if armed {
        ERROR
    } else if response.hovered() {
        INK
    } else {
        TEXT_SECONDARY
    };
    const PITCH: f32 = 6.0;
    let painter = ui.painter();
    for step in [-1.0_f32, 0.0, 1.0] {
        painter.circle_filled(
            rect.center() + Vec2::new(0.0, step * PITCH),
            KEBAB_DOT_RADIUS,
            color,
        );
    }
    response
}

/// The gear's outline: [`GEAR_TEETH`] square-ish teeth around a hub, walked
/// once anticlockwise as a single closed path.
///
/// Each tooth contributes four points -- the two roots either side of it at
/// `root` radius and the two crown corners at `tip` -- so the path steps
/// out, across, and back in for every tooth rather than being a star's
/// alternating spikes. That is what makes it read as a cog: a gear's tooth
/// has a flat top, a pentagram's point does not.
fn gear_outline(center: Pos2, tip: f32, root: f32) -> Vec<Pos2> {
    // Half the angular width of one tooth's crown, as a fraction of the
    // per-tooth sector. 0.30 leaves the gap between teeth slightly wider
    // than the teeth themselves, which is what stops the outline reading as
    // a plain scalloped circle at this size.
    const CROWN_HALF: f32 = 0.30;
    let sector = std::f32::consts::TAU / GEAR_TEETH as f32;
    let at = |angle: f32, radius: f32| {
        center + Vec2::new(radius * angle.cos(), radius * angle.sin())
    };
    let mut points = Vec::with_capacity(GEAR_VERTICES);
    for tooth in 0..GEAR_TEETH {
        let mid = tooth as f32 * sector;
        points.push(at(mid - sector * 0.5 + sector * 0.10, root));
        points.push(at(mid - sector * CROWN_HALF, tip));
        points.push(at(mid + sector * CROWN_HALF, tip));
        points.push(at(mid + sector * 0.5 - sector * 0.10, root));
    }
    debug_assert_eq!(points.len(), GEAR_VERTICES);
    points
}

/// The vault titlebar's Preferences control: a gear, stroked rather than
/// typed.
///
/// **Measured, not assumed** -- and the measurement is the same one
/// `the_icon_codepoints_are_not_carried_by_this_apps_own_typeface` records
/// for ★/☆/👁. U+2699 GEAR *does* resolve in this app's stack, but at an
/// advance of exactly 11.6875 at 13pt: identical to ★ and to ☸, and unlike
/// Archivo's own 'A' (8.875) or 'W' (12.0). That single shared advance is
/// the signature of egui's bundled emoji/icon fallback sitting behind
/// Archivo, not of this app's own typeface having gained the codepoint --
/// so as text it would arrive with a weight, an optical size and a baseline
/// nobody here chose, next to a 28px Lock pill and a 28px avatar whose every
/// measurement comes from design 2b. Drawn, it matches them.
///
/// Sized to the 28px the neighbouring titlebar controls use
/// (`toolbar_button_with_shortcut`'s `HEIGHT`, `draw_circle_avatar`'s
/// `SIZE`), so its hit target is theirs rather than only as big as the mark.
///
/// Carries a hover label because, unlike Lock, it has no word on it --
/// [`close_glyph`]'s "Dismiss" is the precedent for an unlabelled drawn
/// control naming itself on hover.
pub fn gear_button(ui: &mut Ui) -> Response {
    const SIZE: f32 = 28.0;
    const TIP: f32 = 9.0;
    const ROOT: f32 = 6.6;

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The same two-state treatment `kebab_button` uses: this is a neutral
    // navigation control with no "on" state, so it never takes BLUE (which
    // `star_toggle` reserves for an actual toggle being on) and never takes
    // ERROR (reserved for failures).
    let color = if response.hovered() { INK } else { TEXT_SECONDARY };
    let center = rect.center();
    ui.painter().add(egui::Shape::closed_line(
        gear_outline(center, TIP, ROOT),
        Stroke::new(1.3, color),
    ));
    // The hub, stroked rather than filled: a filled disc at this size closes
    // the cog up into a blob, and the ring is what makes it a gear.
    ui.painter()
        .circle_stroke(center, GEAR_HUB_RADIUS, Stroke::new(1.3, color));
    response.on_hover_text("Preferences")
}

/// The eye's almond outline: two parabolic lids meeting at the corners.
fn eye_outline(center: Pos2, half_w: f32, half_h: f32) -> Vec<Pos2> {
    let lid = |t: f32, sign: f32| {
        center + Vec2::new(t * half_w, sign * half_h * (1.0 - t * t))
    };
    let step = |i: usize| -1.0 + 2.0 * i as f32 / EYE_LID_SEGMENTS as f32;
    let mut points: Vec<Pos2> = (0..=EYE_LID_SEGMENTS).map(|i| lid(step(i), -1.0)).collect();
    // The lower lid, back from just inside the right corner to just inside
    // the left one -- the corners themselves are already in the list.
    points.extend((1..EYE_LID_SEGMENTS).rev().map(|i| lid(step(i), 1.0)));
    debug_assert_eq!(points.len(), EYE_VERTICES);
    points
}

/// A masked row's reveal control: an open eye while the value is hidden
/// ("click to see it"), struck through once it is showing ("click to hide
/// it"). The same way every password manager spells this, and the reason
/// the state shown is the ACTION rather than the current condition.
///
/// Square at [`row_button`]'s own 28px height, so it sits on the row's
/// control line at the same size as the buttons elsewhere on the pane. It is
/// now the only thing on that line: the `CTRL+B` text that used to sit beside
/// it moved into the row's hover tooltip.
pub fn eye_toggle(ui: &mut Ui, revealed: bool) -> Response {
    const SIZE: f32 = 28.0;
    const HALF_W: f32 = 8.5;
    const HALF_H: f32 = 5.0;

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_FAINT };
    let center = rect.center();
    let painter = ui.painter();
    painter.add(egui::Shape::closed_line(
        eye_outline(center, HALF_W, HALF_H),
        Stroke::new(1.3, color),
    ));
    painter.circle_filled(center, EYE_PUPIL_RADIUS, color);
    if revealed {
        // The strike, corner to corner and a little past the lids, so it
        // reads as "struck through" rather than as a lash.
        let arm = Vec2::new(HALF_W - 0.5, HALF_H + 2.5);
        painter.line_segment(
            [center - arm, center + arm],
            Stroke::new(1.5, color),
        );
    }
    response
}

/// Finds the icons above in a frame's shape list, for tests that can no
/// longer look them up by a painted string because they paint none.
///
/// Deliberately in this module: the identifying features are the vertex
/// counts and the dot radius declared right above, and a test in another
/// file that spelled them out again would keep passing against a retuned
/// shape it had stopped finding.
#[cfg(test)]
pub mod icon_probe {
    use super::*;

    fn walk(shape: &egui::Shape, out: &mut Vec<Rect>, keep: &dyn Fn(&egui::Shape) -> bool) {
        if keep(shape) {
            out.push(shape.visual_bounding_rect());
        }
        if let egui::Shape::Vec(shapes) = shape {
            for shape in shapes {
                walk(shape, out, keep);
            }
        }
    }

    fn closed_paths(shape: &egui::Shape, vertices: usize) -> Vec<Rect> {
        let mut out = Vec::new();
        walk(shape, &mut out, &|s| {
            matches!(s, egui::Shape::Path(p) if p.closed && p.points.len() == vertices)
        });
        out
    }

    /// One favourite star: where it is, what colour it was stroked in, and
    /// whether it is FILLED -- which is the whole visible difference between
    /// a favourited item and one that is not.
    #[derive(Debug, Clone, Copy)]
    pub struct Star {
        pub rect: Rect,
        pub stroke: Color32,
        pub filled: bool,
    }

    /// The favourite stars this shape tree paints, in both states -- the
    /// filled star carries the same outline the outlined one does, plus the
    /// triangle fan that fills it (see `paint_star`).
    pub fn stars(shape: &egui::Shape) -> Vec<Star> {
        let mut triangles = Vec::new();
        walk(shape, &mut triangles, &|s| {
            matches!(s, egui::Shape::Path(p) if p.points.len() == 3 && p.fill != Color32::TRANSPARENT)
        });
        let mut strokes = Vec::new();
        walk_paths(shape, STAR_VERTICES, &mut strokes);
        strokes
            .into_iter()
            .map(|(rect, stroke)| Star {
                rect,
                stroke,
                filled: triangles.iter().any(|t| rect.expand(1.0).contains_rect(*t)),
            })
            .collect()
    }

    fn walk_paths(shape: &egui::Shape, vertices: usize, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::Path(p) if p.closed && p.points.len() == vertices => {
                let color = match p.stroke.color {
                    egui::epaint::ColorMode::Solid(color) => color,
                    // Nothing here paints a gradient stroke; if something
                    // starts to, this should be looked at rather than
                    // silently reported as transparent.
                    egui::epaint::ColorMode::UV(_) => Color32::TRANSPARENT,
                };
                out.push((shape.visual_bounding_rect(), color));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk_paths(shape, vertices, out);
                }
            }
            _ => {}
        }
    }

    /// The reveal eyes this shape tree paints, in both states -- the strike
    /// is a separate segment, so the almond is found either way.
    pub fn eyes(shape: &egui::Shape) -> Vec<Rect> {
        closed_paths(shape, EYE_VERTICES)
    }

    /// The settings gears this shape tree paints, with the colour each was
    /// stroked in -- which is the only way `gear_button`'s hover state is
    /// visible to a test, since it paints no fill and no string.
    ///
    /// Found by [`GEAR_VERTICES`] alone, exactly as [`stars`] and [`eyes`]
    /// are found by theirs; the hub circle is deliberately NOT part of the
    /// identification, so retuning it cannot make the gear invisible here.
    pub fn gears(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut out = Vec::new();
        walk_paths(shape, GEAR_VERTICES, &mut out);
        out
    }

    /// The strikes through the eyes above -- the ONLY thing that tells the
    /// revealed state from the masked one on screen, so without a way to see
    /// it `eye_toggle` could ignore its argument entirely and look correct.
    ///
    /// Every line segment, left to the caller to intersect with [`eyes`]:
    /// keeping the geometry test at the call site is what stops this from
    /// silently answering "yes" about some unrelated line drawn nearby.
    pub fn line_segments(shape: &egui::Shape) -> Vec<Rect> {
        let mut out = Vec::new();
        walk(shape, &mut out, &|s| {
            matches!(s, egui::Shape::LineSegment { .. })
        });
        out
    }

    /// The kebab's individual dots, each with the colour it was filled in --
    /// which is how `armed` is visible to a test at all.
    ///
    /// Three of them is one kebab; the count is left to the caller so "the
    /// header paints exactly three" is an assertion a test can make rather
    /// than one this helper hides.
    pub fn kebab_dots(shape: &egui::Shape) -> Vec<(Rect, Color32)> {
        let mut out = Vec::new();
        collect_dots(shape, &mut out);
        out
    }

    fn collect_dots(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
        match shape {
            egui::Shape::Circle(c) if (c.radius - KEBAB_DOT_RADIUS).abs() < 0.01 => {
                out.push((shape.visual_bounding_rect(), c.fill));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_dots(shape, out);
                }
            }
            _ => {}
        }
    }
}

/// A small edit-pencil glyph (sidebar folder rows' edit affordance): a
/// diagonal body with a filled triangular tip, plus a flat tail. Drawn
/// rather than typed for the same reason [`close_glyph`] is -- neither the
/// bundled Archivo faces nor egui's fallback stack reliably carry a pencil
/// codepoint (U+270F/U+270E) at this size, so as text it risks a tofu box.
/// Darkens to ink and shows a pointing hand on hover, matching every other
/// bare-glyph affordance in this file.
///
/// The shape is built in local coordinates around the origin, then
/// translated so *its own bounding box* -- not an arbitrary anchor point --
/// lands on `rect`'s center. Anchoring by a single point (an earlier
/// version of this glyph did) looks off-center whenever the shape itself
/// isn't symmetric around that point, which a pencil with a pointed tip and
/// a flat tail never is.
pub fn pencil_glyph_at(ui: &mut Ui, rect: Rect, id: egui::Id) -> Response {
    // `ui.interact`, not `allocate_*`/`scope_*`: this glyph is positioned
    // *beside* a row that already allocated the vertical space they share,
    // so anything that touched the cursor here would allocate that space a
    // second time. Interacting with an explicit rect registers the click
    // target without participating in layout at all.
    let response = ui.interact(rect, id, Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { INK } else { TEXT_GHOST };

    // Body: a thin rectangle running along the (1,-1) diagonal, capped with
    // a triangular tip at one end.
    let dir = Vec2::new(1.0, -1.0).normalized();
    // True 90° rotation `(-y, x)`, NOT the `(y, x)` swap: for a `(d, -d)`
    // diagonal, swapping components yields `(-d, d)` -- the *antiparallel*
    // of `dir`, which collapsed both polygons below into zero-area slivers
    // of collinear points. epaint's anti-aliasing computes miter joins from
    // adjacent edge normals, and for antiparallel edges those normals sum
    // to zero and normalize to NaN -- the GPU then rasterized the resulting
    // garbage triangles as a solid `TEXT_GHOST` smear across the sidebar's
    // entire clip rect (the "gray box covering the left menu" bug).
    let normal = Vec2::new(-dir.y, dir.x) * 1.3; // perpendicular, half-width 1.3
    let tail = Vec2::new(-6.0, 6.0);
    let shoulder = Vec2::new(3.0, -3.0);
    let tip = shoulder + dir * 3.2;
    let body = [tail + normal, shoulder + normal, shoulder - normal, tail - normal];
    let nib = [shoulder + normal, tip, shoulder - normal];

    let local: Vec<Vec2> = body.iter().chain(nib.iter()).copied().collect();
    let min = local.iter().fold(Vec2::new(f32::INFINITY, f32::INFINITY), |a, p| {
        Vec2::new(a.x.min(p.x), a.y.min(p.y))
    });
    let max = local
        .iter()
        .fold(Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY), |a, p| {
            Vec2::new(a.x.max(p.x), a.y.max(p.y))
        });
    let bbox_center = (min + max) / 2.0;
    let offset = rect.center() - bbox_center.to_pos2();

    let to_screen = |v: Vec2| Pos2::new(v.x, v.y) + offset;
    ui.painter().add(egui::Shape::convex_polygon(
        body.iter().map(|p| to_screen(*p)).collect(),
        color,
        Stroke::NONE,
    ));
    ui.painter().add(egui::Shape::convex_polygon(
        nib.iter().map(|p| to_screen(*p)).collect(),
        color,
        Stroke::NONE,
    ));
    response.on_hover_text("Edit folder")
}

/// The footer keyboard-hint strip: `(key, action)` pairs in faint text, per
/// the design's "↑↓ Move · ↵ Fill · Esc Dismiss" bar.
pub fn footer_hints(ui: &mut Ui, hints: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        for (i, (key, action)) in hints.iter().enumerate() {
            if i > 0 {
                ui.add_space(6.0);
            }
            ui.label(
                RichText::new(format!("{key} {action}"))
                    .size(11.0)
                    .color(TEXT_FAINT),
            );
        }
    });
}

/// Width of a scroll bar drawn by [`scrollbar_in_gutter`].
///
/// egui's own `ScrollStyle::solid()` uses 6 for the same thing; there is no
/// scroll bar anywhere in the design document to read a value off, so this
/// follows egui rather than inventing a number.
pub const SCROLLBAR_WIDTH: f32 = 6.0;

/// Configures `ui` so that an [`egui::ScrollArea`] shown inside it reserves a
/// `gutter`-wide lane down its right-hand edge and draws its bar CENTRED in
/// that lane, instead of over the content's own right edge.
///
/// The caller MUST pair this with
/// `.scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)` and with a
/// container whose right padding is ZERO -- the lane replaces that padding.
/// Both halves are load-bearing:
///
/// * The lane is reserved by `floating_allocated_width`, which egui only
///   applies on the axes it is showing a bar for. Under the default
///   `VisibleWhenNeeded` the lane would therefore appear and disappear as the
///   content crossed the overflow threshold, and the content's right edge --
///   the row tiles' -- would jump 10pt sideways with it. `AlwaysVisible`
///   makes the reservation unconditional, so the content keeps one width.
/// * The bar stays FLOATING (egui's default), not `solid()`. Only the
///   floating branch fades the bar out when the pointer is away from the
///   area, which is the behaviour this list already had; `solid()` pins both
///   opacities to 1.0 and takes the handle's colour from
///   `widgets.inactive.bg_fill`, which [`apply`] sets to [`CARD`] -- a white
///   handle on a white track, i.e. invisible. Fixing that would have meant
///   overriding three widget states just to style a scroll bar.
///
/// The centring itself is `bar_outer_margin`: egui pins a floating bar's
/// RIGHT edge at `outer_rect.right() - bar_outer_margin`, and the outer rect
/// now ends at the container's own right edge because of the reserved lane.
/// Half the leftover lane on each side therefore centres it. `floating_width`
/// is raised to the full `bar_width` so the bar does not GROW leftward over
/// the content when hovered -- a bar that only stays centred while dormant
/// would not have fixed the report.
pub fn scrollbar_in_gutter(ui: &mut Ui, gutter: f32) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating_allocated_width = gutter;
    scroll.bar_width = SCROLLBAR_WIDTH;
    scroll.floating_width = SCROLLBAR_WIDTH;
    scroll.bar_outer_margin = (gutter - SCROLLBAR_WIDTH) / 2.0;
}

/// A muted field label ("User name", "Master password").
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(TEXT_MUTED));
}

/// Height of design 2b's search box (`height: 34px`), which is shorter than
/// the form fields' [`FIELD_HEIGHT`] and is its own value for that reason.
pub const SEARCH_FIELD_HEIGHT: f32 = 34.0;

/// Paints the design's magnifier into `rect`.
///
/// STROKED, NOT TYPED, and not an SVG either. The design draws it as inline
/// SVG (`<circle cx=11 cy=11 r=7>` plus a `16.5,16.5 -> 21,21` line in a 24
/// viewBox, `stroke-width: 2.2`); this crate has no SVG pipeline and adding
/// one for a two-shape icon would be a dependency for a circle and a line.
/// Every other glyph here is already drawn the same way and for a related
/// reason -- see [`close_glyph`] and [`pencil_glyph_at`], which are strokes
/// because the codepoints are not in any bundled face.
///
/// The design's viewBox numbers are kept as ratios rather than baked into
/// pixel constants so the glyph is correct at whatever size it is given, and
/// so it can be read straight off the mock.
fn paint_magnifier(painter: &egui::Painter, rect: Rect, color: Color32) {
    let s = rect.width() / 24.0;
    let centre = rect.min + Vec2::splat(11.0 * s);
    let stroke = Stroke::new(2.2 * s, color);
    painter.circle_stroke(centre, 7.0 * s, stroke);
    painter.line_segment(
        [rect.min + Vec2::splat(16.5 * s), rect.min + Vec2::splat(21.0 * s)],
        stroke,
    );
}

/// The label the shortcut slot takes over once there is something to clear.
const SEARCH_CLEAR_HINT: &str = "Esc";

/// Design 2b's search box: a full-width bordered field with the magnifier on
/// the left and a keyboard-shortcut hint on the right.
///
/// `hint` is shown while the field is empty (the mock's "Search 180 logins").
/// `id` is the caller's, because the vault window focuses this field from
/// outside it.
///
/// THE RIGHT-HAND SLOT HAS TWO STATES, and `value` may be CLEARED here:
///
/// * empty field -- `shortcut` (the mock's "CTRL+K"), inert, the way to get
///   INTO the field;
/// * non-empty field -- a clickable "Esc", the way to get out of it, which
///   clears `value`. Pressing the Escape KEY does the same while the field
///   has focus.
///
/// The slot is sized to the WIDER of the two labels and both are right-
/// aligned in it, so the box's text area does not resize as the user types
/// the first character.
///
/// ESCAPE IS READ, NOT CONSUMED, and only acts when the field both has focus
/// and has something in it. That is deliberate and the vault window depends
/// on it: `vault_window::folder_modal` cancels on Escape too, and it runs
/// LATER in the frame than the item list does, so consuming the key here
/// would silently shadow the modal's own binding on any frame where both
/// were live.
///
/// Focus is checked with `lost_focus()`, not `has_focus()`: egui clears a
/// `TextEdit`'s focus on Escape in `Memory::begin_pass`, i.e. BEFORE this
/// function runs, so on the very frame the key arrives the field no longer
/// reports having it. Escape therefore also drops focus, exactly as it does
/// everywhere else in egui; clearing the text is the added behaviour.
///
/// Full width, not the fixed 300px of design **3f**: 3f is the macOS vault
/// window, whose search sits in a unified toolbar; 2b -- the window this crate
/// actually draws -- has `flex: 1` inside the item pane's header, which is
/// also the behaviour that survives the window now being resizable.
///
/// Shares [`field_box`]'s treatment (border at rest, blue border plus a flush
/// 3px halo when focused) rather than re-deriving it, so the one focused-field
/// look in this app stays one look.
pub fn search_field(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    shortcut: &str,
    id: egui::Id,
) -> Response {
    // Placeholder so the box paints *under* the text, same reason as
    // `field_box`'s.
    let bg = ui.painter().add(egui::Shape::Noop);
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), SEARCH_FIELD_HEIGHT),
        Sense::hover(),
    );

    // Design 2b: `padding: 0 10px`, `gap: 8px`, a 14px icon, and the
    // shortcut's own run at the far right.
    const PAD_X: f32 = 10.0;
    const GAP: f32 = 8.0;
    const ICON: f32 = 14.0;
    let icon_rect = Rect::from_center_size(
        Pos2::new(outer.min.x + PAD_X + ICON / 2.0, outer.center().y),
        Vec2::splat(ICON),
    );
    paint_magnifier(ui.painter(), icon_rect, TEXT_GHOST);

    // The slot is as wide as the WIDER of the two labels, whichever is
    // currently showing, so the text area either side of it never resizes.
    let slot_font = FontId::new(10.0, FontFamily::Monospace);
    let lay = |text: &str| ui.painter().layout_no_wrap(text.to_string(), slot_font.clone(), TEXT_GHOST);
    let clearable = !value.is_empty();
    let label = lay(if clearable { SEARCH_CLEAR_HINT } else { shortcut });
    let shortcut_width = lay(shortcut).size().x.max(lay(SEARCH_CLEAR_HINT).size().x);

    // The whole slot is the click target, not just the glyphs: a 3-character
    // run of 10px monospace is a ~17pt target, which is below anything
    // comfortably clickable. Interacting with an explicit rect rather than
    // allocating one keeps this out of the layout, the same reason
    // `pencil_glyph_at` does it -- the field's own box is already allocated.
    let slot = Rect::from_min_max(
        Pos2::new(outer.max.x - PAD_X - shortcut_width, outer.min.y),
        Pos2::new(outer.max.x - PAD_X, outer.max.y),
    );
    let mut clear = false;
    let slot_color = if clearable {
        let hit = ui.interact(slot, id.with("clear"), Sense::click());
        if hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        clear |= hit.clicked();
        // Darkened on hover, the affordance `pencil_glyph_at` already uses.
        if hit.hovered() { INK } else { TEXT_GHOST }
    } else {
        TEXT_GHOST
    };
    ui.painter().galley(
        Pos2::new(
            outer.max.x - PAD_X - label.size().x,
            outer.center().y - label.size().y / 2.0,
        ),
        label,
        slot_color,
    );

    // The text row sits between the icon and the shortcut, centred on the
    // glyphs rather than the box for the same optical reason `field_box`
    // documents.
    let font = FontId::new(13.0, FontFamily::Proportional);
    let row_height = ui.ctx().fonts_mut(|f| f.row_height(&font));
    let text_left = icon_rect.right() + GAP;
    let text_right = outer.max.x - PAD_X - shortcut_width - GAP;
    let inner = Rect::from_center_size(
        Pos2::new(
            (text_left + text_right) / 2.0,
            outer.center().y + row_height * 0.09,
        ),
        Vec2::new((text_right - text_left).max(0.0), row_height),
    );
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .hint_text(RichText::new(hint).size(13.0).color(TEXT_GHOST))
            .frame(egui::Frame::new())
            .font(font)
            .margin(Margin::ZERO)
            .desired_width(inner.width()),
    );

    // See this function's doc: `lost_focus`, because egui has already cleared
    // the field's focus by the time we get here on an Escape frame; and
    // `key_pressed`, which READS the event without consuming it, so the
    // folder modal's own Escape binding further down the frame still sees it.
    if clearable && response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        clear = true;
    }
    if clear {
        value.clear();
        // The text and this slot's label were both laid out earlier in this
        // frame, so the cleared state first paints on the next one.
        ui.ctx().request_repaint();
    }

    let rounding = CornerRadius::same(8);
    let border = if response.has_focus() {
        ui.painter().rect_stroke(
            outer.expand(2.0),
            rounding,
            Stroke::new(3.0, FOCUS_RING),
            StrokeKind::Middle,
        );
        Stroke::new(1.0, BLUE)
    } else {
        Stroke::new(1.0, BORDER_STRONG)
    };
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(outer, rounding, CARD, border, StrokeKind::Middle),
    );
    response
}

/// The design's input-box height (sections 2a/3a/3b/3h).
const FIELD_HEIGHT: f32 = 38.0;

/// A full-width single-line text field with the design's focused-state halo
/// (`box-shadow: 0 0 0 3px #dbe4f7` in the mockup, sections 2a/3a/3b) --
/// egui's default widget styling gives a focused field a plain border color
/// change, not this soft ring, so it's painted explicitly here.
pub fn text_field(ui: &mut Ui, value: &mut String, password: bool) -> Response {
    field_box(ui, value, password, 10.0).0
}

/// A password field with the design's in-field "Show"/"Hide" reveal toggle
/// (3h's master-password input). Same box treatment as [`text_field`];
/// `revealed` is the caller's persistent toggle state.
pub fn password_field(ui: &mut Ui, value: &mut String, revealed: &mut bool) -> Response {
    // The wide right inset keeps typed text from running under the toggle.
    let (response, box_rect) = field_box(ui, value, !*revealed, 52.0);

    // 3h's in-field reveal: a click-sensing label, not a Button, so no
    // padding or fill fights the field it sits inside.
    let label = if *revealed { "Hide" } else { "Show" };
    let toggle_rect = Rect::from_min_max(
        Pos2::new(box_rect.right() - 50.0, box_rect.top() + 1.0),
        Pos2::new(box_rect.right() - 6.0, box_rect.bottom() - 1.0),
    );
    let toggle = ui.put(
        toggle_rect,
        egui::Label::new(semibold(label, 11.0).color(BLUE_DEEP)).sense(Sense::click()),
    );
    if toggle.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if toggle.clicked() {
        *revealed = !*revealed;
    }

    response
}

/// Allocates the design's full 38px input box, places a frameless `TextEdit`
/// inside it (10px left inset, `right_pad` right inset), and paints the box:
/// 1px border at rest, blue border with a flush 3px `FOCUS_RING` halo when
/// focused — a treatment egui's own `TextEdit` frame can't produce.
///
/// The *box* is what gets allocated, not the text row: a frameless TextEdit
/// only allocates its text height, and painting a 38px box around a 16px
/// allocation made the box overlap the label above and shift left of its
/// right inset (asymmetric padding). Returns the response and the box rect
/// (for in-field affordances like the reveal toggle).
fn field_box(ui: &mut Ui, value: &mut String, password: bool, right_pad: f32) -> (Response, Rect) {
    // Placeholder so the box paints *under* the text egui draws in ui.put.
    let bg = ui.painter().add(egui::Shape::Noop);
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), FIELD_HEIGHT),
        Sense::hover(),
    );
    // The TextEdit gets a rect of exactly its row height, centered in the
    // box: handing it the full box height leaves its text sitting at the
    // top instead of vertically centered.
    let font = FontId::new(14.0, FontFamily::Proportional);
    let row_height = ui.ctx().fonts_mut(|f| f.row_height(&font));
    // Nudged down by the descent gap: a row box is ascent+descent tall, but
    // typical field text (no descenders on most characters) fills only the
    // upper part, so geometric centering of the *box* reads as text sitting
    // high. Centering on the glyphs instead is what "vertically centered"
    // actually looks like.
    let optical_nudge = row_height * 0.09;
    let inner = Rect::from_center_size(
        Pos2::new(
            (outer.min.x + 10.0 + outer.max.x - right_pad) / 2.0,
            outer.center().y + optical_nudge,
        ),
        Vec2::new(outer.width() - 10.0 - right_pad, row_height),
    );
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .password(password)
            .frame(egui::Frame::new())
            .font(font)
            .margin(Margin::ZERO)
            .desired_width(inner.width()),
    );

    let rounding = CornerRadius::same(8);
    let border = if response.has_focus() {
        // expand(2.0) with a 3px stroke covers 0.5..3.5px outside the rect:
        // flush against the 1px border's outer edge, like the mock's
        // box-shadow -- expand(3.0) would leave a visible white ring between
        // border and halo.
        ui.painter().rect_stroke(
            outer.expand(2.0),
            rounding,
            Stroke::new(3.0, FOCUS_RING),
            StrokeKind::Middle,
        );
        Stroke::new(1.0, BLUE)
    } else {
        Stroke::new(1.0, BORDER_STRONG)
    };
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(outer, rounding, CARD, border, StrokeKind::Middle),
    );
    (response, outer)
}

/// The design's 40×22 toggle pill (section 3e's settings rows). Paints only;
/// the caller owns the click handling on whatever element contains it.
pub fn toggle_pill(ui: &mut Ui, on: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 22.0), Sense::hover());
    let track = if on { BLUE } else { TOGGLE_OFF };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), track);
    let knob_x = if on {
        rect.max.x - 11.0
    } else {
        rect.min.x + 11.0
    };
    ui.painter()
        .circle_filled(Pos2::new(knob_x, rect.center().y), 9.0, Color32::WHITE);
}

/// A full-width hairline separator in the card hairline color (egui's
/// default separator is darker than the design's).
pub fn hairline(ui: &mut Ui) {
    rule(ui, HAIRLINE);
}

/// The *lighter* separator the design draws **between rows inside a card**
/// (2b's detail rows: `border-bottom: 1px solid #f3f2f2`), as against
/// [`hairline`]'s `#eae7e7`, which is the card's own border and the rule under
/// its heading. Two weights of divider, one nested inside the other.
///
/// `#f3f2f2` is [`CANVAS`] -- the design reuses its warm grey here rather than
/// introducing a sixth grey, and this reuses the constant rather than
/// declaring a same-valued `ROW_RULE` beside it.
pub fn row_rule(ui: &mut Ui) {
    rule(ui, CANVAS);
}

fn rule(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_letter_of_the_first_two_words() {
        assert_eq!(initials("Vantage VPN"), "VV");
        assert_eq!(initials("Remote Desktop — Bastion"), "RD");
    }

    #[test]
    fn initials_of_a_single_word_are_its_first_two_letters() {
        assert_eq!(initials("Ledgerline"), "LE");
    }

    #[test]
    fn initials_split_on_punctuation_not_just_whitespace() {
        // The case that motivated this: an email username on the overlay's
        // credential row must not render as "A.".
        assert_eq!(initials("a.novak@ledgerline.com"), "AN");
        assert_eq!(initials("tracker.exe"), "TE");
        assert_eq!(initials("CORP\\anovak"), "CA");
    }

    #[test]
    fn initials_of_empty_or_blank_input_are_a_placeholder() {
        assert_eq!(initials(""), "?");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn initials_survive_single_character_words() {
        assert_eq!(initials("x"), "X");
        assert_eq!(initials("a b"), "AB");
    }

    /// Runs `apply` against a fresh context and returns it ready to lay out
    /// text with this app's real font set. `set_fonts` only takes effect at
    /// the *start* of the next frame (see `apply`'s own call sites, which
    /// all skip drawing on the frame they style in), so this deliberately
    /// runs two frames before handing the context back.
    fn ctx_with_fonts() -> egui::Context {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        ctx
    }

    /// **The measured evidence behind drawing the detail pane's icons as
    /// shapes, and it is not the answer that was assumed.**
    ///
    /// `close_glyph` records that U+2715 is a tofu box in this app's font
    /// set, and the star, the eye and the kebab were expected to be in the
    /// same position. Measured against the *resolved* stack `apply`
    /// installs, only the kebab is: U+22EE resolves to nothing, so three
    /// stacked dots could only ever have been a shape. ★, ☆ and 👁 DO
    /// resolve -- but not from this app's own typeface. They come out of
    /// egui's bundled emoji/icon fallback behind Archivo, and the tell is
    /// asserted below: all three lay out to one identical advance width,
    /// which is what a uniform-advance icon face does and what a
    /// proportional text face never does.
    ///
    /// That is why they are drawn too. A mark from a fallback face this app
    /// never chose has a weight, an optical size and a baseline nobody here
    /// set, next to 34px controls whose every other measurement comes from
    /// the design; and ★/☆ are two unrelated marks rather than one
    /// silhouette in two weights, which is exactly what the on/off pair has
    /// to be.
    ///
    /// Deterministic despite `font_definitions` pulling Consolas off the
    /// system: that only ever joins the *Monospace* family, and everything
    /// asked here is Proportional.
    #[test]
    fn the_icon_codepoints_are_not_carried_by_this_apps_own_typeface() {
        let ctx = ctx_with_fonts();
        let font = FontId::new(13.0, FontFamily::Proportional);
        let width = |s: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), INK))
                .size()
                .x
        };

        assert!(
            !ctx.fonts_mut(|f| f.has_glyph(&font, '\u{22EE}')),
            "U+22EE VERTICAL ELLIPSIS now resolves; the kebab is three drawn dots \
             because it did not"
        );
        // The positive control for that: `has_glyph` is not simply answering
        // "no" to everything, and the fonts really did load.
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, 'A')),
            "the font set resolves no 'A' either, so the assertion above proves nothing"
        );

        let star = width("\u{2605}");
        assert_eq!(
            star,
            width("\u{2606}"),
            "★ and ☆ no longer share one advance, so they may now come from a real \
             text face -- re-measure before trusting the drawn star's justification"
        );
        assert_eq!(
            star,
            width("\u{1F441}"),
            "★ and 👁 no longer share one advance; see above"
        );
        assert_ne!(
            star,
            width("A"),
            "the icon codepoints now advance like Archivo's own letters, which is what \
             they would do if the bundled faces had gained them"
        );
        // The positive control for the three above: Archivo is proportional,
        // so equal advances are evidence of an icon face and not just of how
        // this stack measures everything.
        assert_ne!(
            width("A"),
            width("W"),
            "'A' and 'W' advance identically, so the equal-advance argument above is \
             about the measurement, not about the face"
        );
    }

    /// The design renders the Lock pill's shortcut in `ui-monospace`, a
    /// visibly different face from the "Lock" label beside it. Asserting
    /// "the code passes `FontFamily::Monospace`" would only restate the
    /// source; this checks the property that actually makes a face
    /// monospaced -- every glyph advancing by the same width -- against the
    /// font that really gets resolved after `apply` replaces the font set.
    #[test]
    fn the_toolbar_shortcut_font_is_really_monospaced() {
        let ctx = ctx_with_fonts();
        let font = FontId::new(10.0, FontFamily::Monospace);

        // 'i' and 'M' are the widest-apart pair in almost any proportional
        // face, and exactly equal in any monospaced one.
        let narrow = ctx.fonts_mut(|f| f.layout_no_wrap("iiiiii".to_owned(), font.clone(), INK));
        let wide = ctx.fonts_mut(|f| f.layout_no_wrap("MMMMMM".to_owned(), font.clone(), INK));

        assert!(
            (narrow.size().x - wide.size().x).abs() < 0.5,
            "the shortcut font is not monospaced: \"iiiiii\" measures {}px but \
             \"MMMMMM\" measures {}px",
            narrow.size().x,
            wide.size().x
        );
    }

    /// ...and that it is genuinely a *different* face from the label's, not
    /// silently falling back to the same Archivo the rest of the pill uses.
    #[test]
    fn the_toolbar_shortcut_font_differs_from_the_label_font() {
        let ctx = ctx_with_fonts();
        let text = "CTRL+L";

        let mono = ctx.fonts_mut(|f| {
            f.layout_no_wrap(text.to_owned(), FontId::new(10.0, FontFamily::Monospace), INK)
        });
        let label_face = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                text.to_owned(),
                FontId::new(10.0, FontFamily::Name(SEMIBOLD.into())),
                INK,
            )
        });

        assert!(
            (mono.size().x - label_face.size().x).abs() > 0.5,
            "the shortcut and the label resolve to the same face -- both \
             measure {}px for {text:?}, so the shortcut is not visually \
             distinct the way the design requires",
            mono.size().x
        );
    }

    /// Guards the silent-fallback case: `system_monospace` returning `None`
    /// (or its result never reaching the family stack) would leave egui's
    /// Hack in place, and both tests above would still pass while the app
    /// kept rendering the wrong face. Comparing against a context that
    /// never ran `apply` is what actually proves the substitution took.
    #[test]
    fn the_system_monospace_face_actually_replaces_egui_default() {
        let Some(_) = system_monospace() else {
            // Not a Windows install carrying the standard font set. The
            // fallback is deliberate and correct, so there is nothing to
            // assert here.
            return;
        };

        let styled = ctx_with_fonts();
        let bare = egui::Context::default();
        let _ = bare.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0))),
                ..Default::default()
            },
            |_ui| {},
        );

        let font = FontId::new(10.0, FontFamily::Monospace);
        let text = "CTRL+L";
        let ours = styled.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), INK));
        let egui_default = bare.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, INK));

        assert!(
            (ours.size().x - egui_default.size().x).abs() > 0.5,
            "the monospace face is still egui's bundled Hack -- both measure \
             {}px for {text:?}, so the system face never made it into the \
             family stack",
            ours.size().x
        );
    }

    /// The wordmark reads as both too light and too narrow when rendered in
    /// Bold instead of ExtraBold, because Archivo's 800 cut is genuinely
    /// wider per glyph than its 700 -- not just heavier. If the ExtraBold
    /// face ever failed to load, the named family would fall back through
    /// the stack and this measurement would collapse onto Bold's, silently
    /// restoring exactly the appearance the extra face was added to fix.
    #[test]
    fn extrabold_is_a_distinct_wider_face_than_bold() {
        let ctx = ctx_with_fonts();
        let word = "Deskwarden";

        let measure = |family: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    word.to_owned(),
                    FontId::new(25.0, FontFamily::Name(family.into())),
                    INK,
                )
                .size()
                .x
            })
        };

        let bold = measure(BOLD);
        let extrabold = measure(EXTRABOLD);

        assert!(
            extrabold > bold + 0.5,
            "ExtraBold is not wider than Bold ({extrabold}px vs {bold}px for {word:?}) \
             -- the 800 face is probably not loading"
        );
    }

    #[test]
    fn quadrant_outlines_stay_inside_the_design_viewbox() {
        for outline in quadrant_outlines() {
            for p in outline {
                assert!(
                    (2.0..=22.0).contains(&p.x) && (2.0..=26.0).contains(&p.y),
                    "point {p:?} escapes the 24x28 viewbox's shield bounds"
                );
            }
        }
    }

    #[test]
    fn window_icon_is_opaque_in_the_shield_and_transparent_outside() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        let alpha = |x: usize, y: usize| icon.rgba[(y * 32 + x) * 4 + 3];
        // Corners are outside the shield; the center is deep inside it.
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(31, 0), 0);
        assert_eq!(alpha(0, 31), 0);
        assert_eq!(alpha(31, 31), 0);
        assert_eq!(alpha(16, 14), 255);
    }

    /// Every shared edge in the mark must divide a dark quadrant from a
    /// light one. Palette order (deep, blue, bright, soft) fails this: it
    /// puts the two darkest values along the whole top edge, ~15 apart in
    /// luminance, and the mark reads as three shapes instead of four.
    #[test]
    fn quadrant_tones_alternate_around_the_mark() {
        // Rec. 709 relative luminance -- how light each fill actually reads,
        // rather than how far apart the raw RGB triples happen to be.
        fn luminance(c: Color32) -> f32 {
            0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32
        }

        let [tl, tr, bl, br] = QUADRANT_FILLS.map(luminance);

        // The four edges quadrants actually share: the mark is split by one
        // vertical and one horizontal line, so corners touching only
        // diagonally are not adjacent.
        let adjacent = [
            ("top edge", tl, tr),
            ("bottom edge", bl, br),
            ("left edge", tl, bl),
            ("right edge", tr, br),
        ];
        for (edge, a, b) in adjacent {
            let delta = (a - b).abs();
            assert!(
                delta > 40.0,
                "the {edge} divides two quadrants only {delta:.1} apart in \
                 luminance -- too close to read as separate shapes"
            );
        }

        // ...and the near-matching pair must be diagonal, which is what
        // makes the alternation possible at all.
        let diagonals = [(tl - br).abs(), (tr - bl).abs()];
        let closest_adjacent = adjacent
            .iter()
            .map(|(_, a, b)| (a - b).abs())
            .fold(f32::INFINITY, f32::min);
        assert!(
            diagonals.iter().cloned().fold(f32::INFINITY, f32::min) < closest_adjacent,
            "the two most similar quadrants are edge-adjacent, not diagonal"
        );
    }

    #[test]
    fn quadrants_meet_at_the_shield_center() {
        // All four quadrants share the (12, 14) center corner; a drift there
        // would open a visible seam in the middle of the mark.
        for outline in quadrant_outlines() {
            assert!(
                outline
                    .iter()
                    .any(|p| (p.x - 12.0).abs() < 0.3 && (p.y - 14.0).abs() < 0.3),
                "a quadrant no longer touches the shield center"
            );
        }
    }
}
